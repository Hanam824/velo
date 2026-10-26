//! The engine task: adding, pausing, resuming across a restart and deleting downloads.

mod support;

use std::fs;
use std::path::{Path, PathBuf};
use std::time::Duration;

use tokio::sync::broadcast::Receiver;
use velo_core::{
    ConnectionLimit, Download, DownloadId, DownloadState, EngineEvent, Queue, QueueId, Resolver,
    Resolving, Settings, Source,
};
use velo_engine::{
    Credentials, DownloadEdit, Engine, EngineConfig, NewDownload, RequestOptions, RetryPolicy,
};

use support::{SIZE, body, versioned_body};

/// Two connections and small segments.
fn settings() -> Settings {
    Settings {
        max_connections: 2,
        min_segment_size: 16 * 1024,
        ..Settings::default()
    }
}

/// An engine with [`settings`] and fast retries, its temp folder in `root/temp`.
fn engine(root: &Path, downloads: Vec<Download>) -> Engine {
    engine_with(root, settings(), downloads)
}

/// An engine with `settings` and fast retries, its temp folder in `root/temp`.
fn engine_with(root: &Path, settings: Settings, downloads: Vec<Download>) -> Engine {
    let config = EngineConfig {
        temp_dir: root.join("temp"),
        settings,
        retry: RetryPolicy {
            max_retries: 1,
            delay: Duration::from_millis(5),
        },
        resolver: None,
    };
    Engine::start(config, downloads, Vec::new()).unwrap()
}

/// A download of `url` into `root/save`, started at once if `start`.
fn new_download(root: &Path, url: String, start: bool) -> NewDownload {
    let save_dir = root.join("save");
    fs::create_dir_all(&save_dir).unwrap();
    NewDownload {
        url,
        file_name: String::new(),
        save_dir,
        category: None,
        description: String::new(),
        start,
        request: RequestOptions::default(),
        source: None,
    }
}

/// Waits up to 10 s for an event that `pick` maps to `Some`.
async fn wait_for<T>(
    events: &mut Receiver<EngineEvent>,
    mut pick: impl FnMut(&EngineEvent) -> Option<T>,
) -> T {
    tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            if let Some(found) = pick(&events.recv().await.unwrap()) {
                return found;
            }
        }
    })
    .await
    .expect("timed out waiting for an engine event")
}

/// Waits for download `id` to reach a state that `done` accepts and returns it.
async fn wait_for_state(
    events: &mut Receiver<EngineEvent>,
    id: DownloadId,
    done: impl Fn(&DownloadState) -> bool,
) -> Download {
    wait_for(events, |event| match event {
        EngineEvent::Changed(d) if d.id == id && done(&d.state) => Some(d.clone()),
        _ => None,
    })
    .await
}

/// Waits until download `id` has at least `bytes` downloaded.
async fn wait_for_bytes(events: &mut Receiver<EngineEvent>, id: DownloadId, bytes: u64) {
    wait_for(events, |event| match event {
        EngineEvent::Progress(p) if p.id == id && p.downloaded >= bytes => Some(()),
        _ => None,
    })
    .await;
}

/// The path of the finished file of `download`.
fn finished(download: &Download) -> PathBuf {
    assert_eq!(download.state, DownloadState::Complete);
    download.save_path()
}

#[tokio::test]
async fn an_added_download_runs_to_completion() {
    let base = support::start().await;
    let root = tempfile::tempdir().unwrap();
    let engine = engine(root.path(), Vec::new());
    let mut events = engine.subscribe();

    let id = engine.add(new_download(
        root.path(),
        format!("{base}/slow/data.bin"),
        true,
    ));
    let added = wait_for(&mut events, |e| match e {
        EngineEvent::Added(d) => Some(d.clone()),
        _ => None,
    })
    .await;
    assert_eq!(added.id, id);
    assert!(added.added_at.is_some());

    let probed = wait_for_state(&mut events, id, |s| *s == DownloadState::Downloading).await;
    assert_eq!(probed.total_size, Some(SIZE as u64));
    assert_eq!(probed.file_name, "data.bin");
    let done = wait_for_state(&mut events, id, |s| *s == DownloadState::Complete).await;
    assert!(done.last_try_at.is_some());
    assert!(fs::read(finished(&done)).unwrap() == body());
    assert!(!root.path().join("temp").join(id.0.to_string()).exists());
}

#[tokio::test]
async fn a_paused_download_resumes_after_an_engine_restart() {
    let base = support::start().await;
    let root = tempfile::tempdir().unwrap();
    let first = engine(root.path(), Vec::new());
    let mut events = first.subscribe();
    let id = first.add(new_download(
        root.path(),
        format!("{base}/slower/data.bin"),
        true,
    ));

    wait_for_bytes(&mut events, id, 50_000).await;
    first.pause(id);
    let paused = wait_for_state(&mut events, id, |s| *s == DownloadState::Paused).await;
    let saved = paused.downloaded();
    assert!(saved >= 50_000 && saved < SIZE as u64, "{saved}");
    assert!(root.path().join("temp").join(id.0.to_string()).is_dir());
    first.shutdown().await;

    // What a store would hold: the last Changed event.
    let second = engine(root.path(), vec![paused]);
    let mut events = second.subscribe();
    second.resume(id);
    let done = wait_for_state(&mut events, id, |s| *s == DownloadState::Complete).await;
    assert!(fs::read(finished(&done)).unwrap() == body());
}

#[tokio::test]
async fn a_changed_file_on_the_server_restarts_the_download() {
    let base = support::start().await;
    let root = tempfile::tempdir().unwrap();
    let engine = engine(root.path(), Vec::new());
    let mut events = engine.subscribe();
    let id = engine.add(new_download(
        root.path(),
        format!("{base}/versioned/data.bin"),
        true,
    ));

    wait_for_bytes(&mut events, id, 50_000).await;
    engine.pause(id);
    let paused = wait_for_state(&mut events, id, |s| *s == DownloadState::Paused).await;
    assert_eq!(paused.etag.as_deref(), Some("\"v0\""));

    reqwest::get(format!("{base}/bump")).await.unwrap();
    engine.resume(id);
    let done = wait_for_state(&mut events, id, |s| *s == DownloadState::Complete).await;
    assert_eq!(done.etag.as_deref(), Some("\"v1\""));
    assert!(fs::read(finished(&done)).unwrap() == versioned_body(1));
}

#[tokio::test]
async fn deleting_a_running_download_removes_its_parts() {
    let base = support::start().await;
    let root = tempfile::tempdir().unwrap();
    let engine = engine(root.path(), Vec::new());
    let mut events = engine.subscribe();
    let id = engine.add(new_download(
        root.path(),
        format!("{base}/slower/data.bin"),
        true,
    ));

    wait_for_bytes(&mut events, id, 20_000).await;
    engine.delete(id, true);
    wait_for(&mut events, |e| {
        (*e == EngineEvent::Removed(id)).then_some(())
    })
    .await;
    // The parts are deleted in the background.
    let parts = root.path().join("temp").join(id.0.to_string());
    for _ in 0..100 {
        if !parts.exists() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    assert!(!parts.exists());
    assert!(!root.path().join("save").join("data.bin").exists());
}

#[tokio::test]
async fn a_failing_server_marks_the_download_failed() {
    let base = support::start().await;
    let root = tempfile::tempdir().unwrap();
    let engine = engine(root.path(), Vec::new());
    let mut events = engine.subscribe();
    let id = engine.add(new_download(
        root.path(),
        format!("{base}/fail/data.bin"),
        true,
    ));

    let failed = wait_for_state(&mut events, id, |s| matches!(s, DownloadState::Failed(_))).await;
    assert_eq!(
        failed.state,
        DownloadState::Failed("server answered 503 Service Unavailable".into())
    );
}

#[tokio::test]
async fn a_download_for_later_waits_and_finishes_under_a_free_name() {
    let base = support::start().await;
    let root = tempfile::tempdir().unwrap();
    let engine = engine(root.path(), Vec::new());
    let mut events = engine.subscribe();
    let new = new_download(root.path(), format!("{base}/files/data.bin"), false);
    fs::write(new.save_dir.join("data.bin"), b"someone else's file").unwrap();

    let id = engine.add(new);
    let added = wait_for(&mut events, |e| match e {
        EngineEvent::Added(d) => Some(d.clone()),
        _ => None,
    })
    .await;
    assert_eq!(added.state, DownloadState::Queued);

    engine.resume(id);
    let done = wait_for_state(&mut events, id, |s| *s == DownloadState::Complete).await;
    assert_eq!(done.file_name, "data (1).bin");
    assert!(fs::read(finished(&done)).unwrap() == body());
    assert_eq!(
        fs::read(root.path().join("save").join("data.bin")).unwrap(),
        b"someone else's file"
    );
}

#[tokio::test]
async fn credentials_given_when_adding_are_sent_to_the_server() {
    let base = support::start().await;
    let root = tempfile::tempdir().unwrap();
    let engine = engine(root.path(), Vec::new());
    let mut events = engine.subscribe();

    let without = engine.add(new_download(
        root.path(),
        format!("{base}/auth/data.bin"),
        true,
    ));
    let failed = wait_for_state(&mut events, without, |s| {
        matches!(s, DownloadState::Failed(_))
    })
    .await;
    assert_eq!(
        failed.state,
        DownloadState::Failed("server answered 401 Unauthorized".into())
    );

    let mut new = new_download(root.path(), format!("{base}/auth/data.bin"), true);
    new.request.credentials = Some(Credentials {
        user: support::USER.into(),
        password: support::PASSWORD.into(),
    });
    let with = engine.add(new);
    let done = wait_for_state(&mut events, with, |s| *s == DownloadState::Complete).await;
    assert!(fs::read(finished(&done)).unwrap() == body());
    // Credentials never reach what is saved.
    assert!(!format!("{done:?}").contains(support::PASSWORD));
}

#[tokio::test]
async fn the_global_speed_limit_holds_within_ten_percent() {
    let base = support::start().await;
    let root = tempfile::tempdir().unwrap();
    let rate = 100 * 1024;
    let engine = engine_with(
        root.path(),
        Settings {
            speed_limit: Some(rate),
            ..settings()
        },
        Vec::new(),
    );
    let mut events = engine.subscribe();
    let start = std::time::Instant::now();
    let id = engine.add(new_download(
        root.path(),
        format!("{base}/files/data.bin"),
        true,
    ));
    let done = wait_for_state(&mut events, id, |s| *s == DownloadState::Complete).await;
    let elapsed = start.elapsed().as_secs_f64();
    assert!(fs::read(finished(&done)).unwrap() == body());
    // One second of data passes at once; the rest at the limit.
    let expected = (SIZE as u64 - rate) as f64 / rate as f64;
    // Slower machines only add time, so the upper bound is looser.
    assert!(
        elapsed > expected * 0.9 && elapsed < expected * 1.3,
        "{elapsed:.2} s, expected {expected:.2} s"
    );
}

#[tokio::test]
async fn a_download_limit_set_while_running_applies_at_once() {
    let base = support::start().await;
    let root = tempfile::tempdir().unwrap();
    let engine = engine(root.path(), Vec::new());
    let mut events = engine.subscribe();
    let id = engine.add(new_download(
        root.path(),
        format!("{base}/files/data.bin"),
        false,
    ));
    // Small enough that the file takes about a second and a half after the first burst.
    engine.set_speed_limit(id, Some(120 * 1024));
    let start = std::time::Instant::now();
    engine.resume(id);
    wait_for_state(&mut events, id, |s| *s == DownloadState::Complete).await;
    let elapsed = start.elapsed().as_secs_f64();
    assert!(elapsed > 1.2, "{elapsed:.2} s");
}

#[tokio::test]
async fn a_server_limit_overrides_the_connection_count() {
    let base = support::start().await;
    let root = tempfile::tempdir().unwrap();
    let mut limited = settings();
    limited.max_connections = 4;
    limited.connection_limits = vec![ConnectionLimit {
        host: "127.0.0.1".into(),
        max_connections: 1,
    }];
    let engine = engine_with(root.path(), limited, Vec::new());
    let mut events = engine.subscribe();
    let id = engine.add(new_download(
        root.path(),
        format!("{base}/slow/data.bin"),
        true,
    ));
    let done = wait_for_state(&mut events, id, |s| *s == DownloadState::Complete).await;
    // One connection never splits the file.
    assert_eq!(done.segments.len(), 1);
}

#[tokio::test]
async fn referer_and_cookies_are_sent() {
    let base = support::start().await;
    let root = tempfile::tempdir().unwrap();
    let engine = engine(root.path(), Vec::new());
    let mut events = engine.subscribe();
    let mut new = new_download(root.path(), format!("{base}/headers/data.bin"), true);
    new.request.referer = Some(support::REFERER.into());
    new.request.cookies = Some(support::COOKIES.into());
    let id = engine.add(new);
    let done = wait_for_state(&mut events, id, |s| *s == DownloadState::Complete).await;
    assert!(fs::read(finished(&done)).unwrap() == body());

    let plain = engine.add(new_download(
        root.path(),
        format!("{base}/headers/data.bin"),
        true,
    ));
    let failed = wait_for_state(&mut events, plain, |s| {
        matches!(s, DownloadState::Failed(_))
    })
    .await;
    assert_eq!(
        failed.state,
        DownloadState::Failed("server answered 403 Forbidden".into())
    );
}

#[tokio::test]
async fn properties_change_a_waiting_download_but_only_the_description_of_a_finished_one() {
    let base = support::start().await;
    let root = tempfile::tempdir().unwrap();
    let engine = engine(root.path(), Vec::new());
    let mut events = engine.subscribe();
    let id = engine.add(new_download(
        root.path(),
        format!("{base}/fail/data.bin"),
        false,
    ));

    engine.edit(
        id,
        DownloadEdit {
            file_name: Some("../renamed?.bin".into()),
            url: Some(format!("{base}/files/data.bin")),
            description: Some("mirror".into()),
            category: Some(Some("Programs".into())),
        },
    );
    let edited = wait_for(&mut events, |e| match e {
        EngineEvent::Changed(d) if d.id == id && d.description == "mirror" => Some(d.clone()),
        _ => None,
    })
    .await;
    assert_eq!(edited.file_name, "renamed_.bin");
    assert!(edited.url.ends_with("/files/data.bin"));
    assert_eq!(edited.category.as_deref(), Some("Programs"));

    // The new address works.
    engine.resume(id);
    let done = wait_for_state(&mut events, id, |s| *s == DownloadState::Complete).await;
    assert_eq!(done.file_name, "renamed_.bin");

    engine.edit(
        id,
        DownloadEdit {
            file_name: Some("other.bin".into()),
            description: Some("kept".into()),
            ..DownloadEdit::default()
        },
    );
    let complete = wait_for(&mut events, |e| match e {
        EngineEvent::Changed(d) if d.id == id && d.description == "kept" => Some(d.clone()),
        _ => None,
    })
    .await;
    assert_eq!(complete.file_name, "renamed_.bin");
}

/// Waits for queue `id` to be reported, and returns it as reported.
async fn wait_for_queue(
    events: &mut Receiver<EngineEvent>,
    id: QueueId,
    done: impl Fn(&Queue) -> bool,
) -> Queue {
    wait_for(events, |event| match event {
        EngineEvent::QueueChanged(q) if q.id == id && done(q) => Some(q.clone()),
        _ => None,
    })
    .await
}

#[tokio::test]
async fn a_queue_runs_its_downloads_one_at_a_time_then_finishes() {
    let base = support::start().await;
    let root = tempfile::tempdir().unwrap();
    let engine = engine(root.path(), Vec::new());
    let mut events = engine.subscribe();
    let first = engine.add(new_download(
        root.path(),
        format!("{base}/files/one.bin"),
        false,
    ));
    let second = engine.add(new_download(
        root.path(),
        format!("{base}/files/two.bin"),
        false,
    ));
    engine.enqueue(QueueId::MAIN, vec![first, second]);
    wait_for_queue(&mut events, QueueId::MAIN, |q| {
        q.downloads == [first, second]
    })
    .await;

    engine.start_queue(QueueId::MAIN);
    // Collect what happens until the queue reports it is done.
    let mut seen = Vec::new();
    wait_for(&mut events, |event| {
        match event {
            EngineEvent::Changed(d) if d.state == DownloadState::Probing => {
                seen.push(format!("start {}", d.id.0))
            }
            EngineEvent::Changed(d) if d.state == DownloadState::Complete => {
                seen.push(format!("done {}", d.id.0))
            }
            EngineEvent::QueueFinished(id) if *id == QueueId::MAIN => return Some(()),
            _ => {}
        }
        None
    })
    .await;
    assert_eq!(
        seen,
        [
            format!("start {}", first.0),
            format!("done {}", first.0),
            format!("start {}", second.0),
            format!("done {}", second.0),
        ]
    );
}

#[tokio::test]
async fn stopping_a_queue_pauses_it_and_a_hand_paused_download_stays_paused() {
    let base = support::start().await;
    let root = tempfile::tempdir().unwrap();
    let engine = engine(root.path(), Vec::new());
    let mut events = engine.subscribe();
    let night = engine.add_queue("Night");
    let a = engine.add(new_download(
        root.path(),
        format!("{base}/slower/a.bin"),
        false,
    ));
    let b = engine.add(new_download(
        root.path(),
        format!("{base}/slower/b.bin"),
        false,
    ));
    engine.enqueue(night, vec![a, b]);
    let mut queue = wait_for_queue(&mut events, night, |q| q.downloads.len() == 2).await;
    queue.max_concurrent = 2;
    engine.save_queue(queue);
    engine.start_queue(night);

    wait_for_bytes(&mut events, a, 20_000).await;
    // Paused by hand: the running queue leaves it alone.
    engine.pause(a);
    wait_for_state(&mut events, a, |s| *s == DownloadState::Paused).await;
    wait_for_bytes(&mut events, b, 20_000).await;
    engine.stop_queue(night);
    wait_for(&mut events, |e| {
        (*e == EngineEvent::QueueRunning(night, false)).then_some(())
    })
    .await;
    wait_for_state(&mut events, b, |s| *s == DownloadState::Paused).await;

    // Starting it again runs both, the hand-paused one too, and finishes.
    engine.start_queue(night);
    wait_for(&mut events, |e| {
        (*e == EngineEvent::QueueFinished(night)).then_some(())
    })
    .await;
}

#[tokio::test]
async fn downloads_move_between_queues_and_leave_them_when_deleted() {
    let base = support::start().await;
    let root = tempfile::tempdir().unwrap();
    let engine = engine(root.path(), Vec::new());
    let mut events = engine.subscribe();
    let id = engine.add(new_download(
        root.path(),
        format!("{base}/files/a.bin"),
        false,
    ));
    engine.enqueue(QueueId::MAIN, vec![id]);
    let other = engine.add_queue("Other");
    engine.enqueue(other, vec![id]);
    wait_for_queue(&mut events, QueueId::MAIN, |q| q.downloads.is_empty()).await;
    wait_for_queue(&mut events, other, |q| q.downloads == [id]).await;

    engine.delete(id, false);
    wait_for_queue(&mut events, other, |q| q.downloads.is_empty()).await;
    engine.delete_queue(other);
    wait_for(&mut events, |e| {
        (*e == EngineEvent::QueueRemoved(other)).then_some(())
    })
    .await;

    // The main queue always exists and can't be deleted.
    engine.delete_queue(QueueId::MAIN);
    engine.resync();
    let main = wait_for_queue(&mut events, QueueId::MAIN, |_| true).await;
    assert_eq!(main.name, "Main download queue");
}

/// Hands out one address for every source, counting how often it was asked.
#[derive(Debug)]
struct FixedResolver {
    url: String,
    asked: std::sync::atomic::AtomicUsize,
}

impl Resolver for FixedResolver {
    fn resolve<'a>(&'a self, _source: &'a Source) -> Resolving<'a> {
        self.asked.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        let url = self.url.clone();
        Box::pin(async move { Ok(vec![url]) })
    }
}

#[tokio::test]
async fn a_download_with_a_source_gets_a_fresh_address_before_it_starts() {
    let base = support::start().await;
    let root = tempfile::tempdir().unwrap();
    let resolver = std::sync::Arc::new(FixedResolver {
        url: format!("{base}/files/video.mp4"),
        asked: Default::default(),
    });
    let config = EngineConfig {
        temp_dir: root.path().join("temp"),
        settings: settings(),
        retry: RetryPolicy {
            max_retries: 1,
            delay: Duration::from_millis(5),
        },
        resolver: Some(resolver.clone()),
    };
    let resolving = Engine::start(config, Vec::new(), Vec::new()).unwrap();
    let mut events = resolving.subscribe();
    let source = Source::YouTube {
        video_id: "dQw4w9WgXcQ".into(),
        itags: vec![18],
    };
    let id = resolving.add(NewDownload {
        source: Some(source.clone()),
        ..new_download(root.path(), "https://expired.invalid/old".into(), true)
    });
    let done = wait_for_state(&mut events, id, |s| {
        matches!(s, DownloadState::Complete | DownloadState::Failed(_))
    })
    .await;
    assert_eq!(fs::read(finished(&done)).unwrap(), body());
    assert_eq!(done.url, format!("{base}/files/video.mp4"));
    assert_eq!(done.source, Some(source.clone()));
    assert_eq!(resolver.asked.load(std::sync::atomic::Ordering::SeqCst), 1);

    // Without a resolver, such a download can't start.
    let plain = engine(root.path(), Vec::new());
    let mut events = plain.subscribe();
    let id = plain.add(NewDownload {
        source: Some(source),
        ..new_download(root.path(), "https://expired.invalid/old".into(), true)
    });
    let failed = wait_for_state(&mut events, id, |s| matches!(s, DownloadState::Failed(_))).await;
    assert!(
        matches!(&failed.state, DownloadState::Failed(m) if m.contains("YouTube")),
        "{:?}",
        failed.state
    );
}

/// Hands out the same addresses for every source.
#[derive(Debug)]
struct ListResolver(Vec<String>);

impl Resolver for ListResolver {
    fn resolve<'a>(&'a self, _source: &'a Source) -> Resolving<'a> {
        let urls = self.0.clone();
        Box::pin(async move { Ok(urls) })
    }
}

#[tokio::test]
async fn a_video_and_its_sound_are_downloaded_in_turn_and_joined() {
    use velo_mux::testing::stream;
    let base = support::start().await;
    let root = tempfile::tempdir().unwrap();
    support::put_blob(
        "video.mp4",
        stream(1, 1000, 90_000, b"vide", &[0, 180_000], 0x10),
    );
    support::put_blob(
        "audio.m4a",
        stream(1, 44_100, 44_100, b"soun", &[0, 66_150, 132_300], 0x80),
    );
    let config = EngineConfig {
        temp_dir: root.path().join("temp"),
        settings: settings(),
        retry: RetryPolicy {
            max_retries: 1,
            delay: Duration::from_millis(5),
        },
        resolver: Some(std::sync::Arc::new(ListResolver(vec![
            format!("{base}/blob/video.mp4"),
            format!("{base}/blob/audio.m4a"),
        ]))),
    };
    let joining = Engine::start(config, Vec::new(), Vec::new()).unwrap();
    let mut events = joining.subscribe();
    let id = joining.add(NewDownload {
        file_name: "clip.mp4".into(),
        source: Some(Source::YouTube {
            video_id: "dQw4w9WgXcQ".into(),
            itags: vec![137, 140],
        }),
        ..new_download(root.path(), "https://expired.invalid/old".into(), true)
    });
    let done = wait_for_state(&mut events, id, |s| {
        matches!(s, DownloadState::Complete | DownloadState::Failed(_))
    })
    .await;
    let joined = fs::read(finished(&done)).unwrap();
    assert_eq!(done.file_name, "clip.mp4");
    assert_eq!(done.total_size, Some(joined.len() as u64));
    assert_eq!(&joined[4..8], b"ftyp");
    let traks = joined.windows(4).filter(|w| w == b"trak").count();
    let moofs = joined.windows(4).filter(|w| w == b"moof").count();
    assert_eq!((traks, moofs), (2, 5), "both tracks and all five fragments");
    assert!(
        !root.path().join("temp").join(id.0.to_string()).exists(),
        "the streams and their parts are cleaned up"
    );
}
