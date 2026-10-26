//! The engine task: adding, pausing, resuming across a restart and deleting downloads.

mod support;

use std::fs;
use std::path::{Path, PathBuf};
use std::time::Duration;

use tokio::sync::broadcast::Receiver;
use velo_core::{Download, DownloadId, DownloadState, EngineEvent};
use velo_engine::{Credentials, Engine, EngineConfig, NewDownload, Options, client};

use support::{SIZE, body, versioned_body};

/// An engine with two connections and fast retries, its temp folder in `root/temp`.
fn engine(root: &Path, downloads: Vec<Download>) -> Engine {
    let config = EngineConfig {
        client: client().unwrap(),
        temp_dir: root.join("temp"),
        options: Options {
            max_connections: 2,
            min_segment_size: 16 * 1024,
            max_retries: 1,
            retry_delay: Duration::from_millis(5),
        },
    };
    Engine::start(config, downloads)
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
        credentials: None,
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
    new.credentials = Some(Credentials {
        user: support::USER.into(),
        password: support::PASSWORD.into(),
    });
    let with = engine.add(new);
    let done = wait_for_state(&mut events, with, |s| *s == DownloadState::Complete).await;
    assert!(fs::read(finished(&done)).unwrap() == body());
    // Credentials never reach what is saved.
    assert!(!format!("{done:?}").contains(support::PASSWORD));
}
