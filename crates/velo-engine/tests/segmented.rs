//! Multi-connection downloads, retries and resuming against the local test server.

mod support;

use std::fs;
use std::path::Path;
use std::time::Duration;

use reqwest::{StatusCode, Url};
use velo_core::{DownloadId, Segment, SegmentId, SegmentState};
use velo_engine::{
    CancellationToken, Error, Options, Progress, client, download_file, download_segments, finish,
};

use support::{SIZE, body};

/// Four connections, small segments and fast retries.
fn options() -> Options {
    Options {
        max_connections: 4,
        min_segment_size: 16 * 1024,
        max_retries: 3,
        retry_delay: Duration::from_millis(5),
        limiters: Vec::new(),
        max_request_size: None,
    }
}

/// Runs [`download_file`] for `path` on the test server into fresh temp and save folders.
async fn download(
    path: &str,
    options: &Options,
) -> (
    tempfile::TempDir,
    Progress,
    velo_engine::Result<std::path::PathBuf>,
) {
    let base = support::start().await;
    let root = tempfile::tempdir().unwrap();
    let (temp, save) = (root.path().join("temp"), root.path().join("save"));
    fs::create_dir_all(&save).unwrap();
    let progress = Progress::default();
    let result = download_file(
        &client().unwrap(),
        &format!("{base}{path}"),
        &temp,
        DownloadId(1),
        &save,
        options,
        &progress,
    )
    .await;
    (root, progress, result)
}

#[tokio::test]
async fn a_file_downloads_over_several_connections_into_one_file() {
    let (root, progress, result) = download("/slow/data.bin", &options()).await;
    let path = result.unwrap();
    assert_eq!(path, root.path().join("save").join("data.bin"));
    assert!(fs::read(&path).unwrap() == body());

    // Idle workers split the file; it was moved into place and nothing is left behind.
    let segments = progress.snapshot();
    assert!(segments.len() >= 4, "{segments:?}");
    assert!(segments.iter().all(|s| s.state == SegmentState::Done));
    assert_eq!(progress.downloaded(), SIZE as u64);
    assert!(!root.path().join("temp").join("1").exists());
    assert!(!root.path().join("save").join("data.bin.velo-tmp").exists());
}

#[tokio::test]
async fn cut_off_connections_are_retried() {
    let (_root, _, result) = download("/flaky/data.bin", &options()).await;
    assert!(fs::read(result.unwrap()).unwrap() == body());

    // The same server without retries: the first cut-off connection ends the download.
    let no_retries = Options {
        max_retries: 0,
        ..options()
    };
    let (_root, _, result) = download("/flaky/data.bin", &no_retries).await;
    assert!(
        matches!(result, Err(Error::Http(_) | Error::Incomplete { .. })),
        "{result:?}"
    );
}

#[tokio::test]
async fn a_server_that_keeps_failing_gives_up_and_keeps_the_file() {
    let (root, progress, result) = download("/fail/data.bin", &options()).await;
    assert!(
        matches!(result, Err(Error::Status(StatusCode::SERVICE_UNAVAILABLE))),
        "{result:?}"
    );
    assert!(!root.path().join("save").join("data.bin").exists());
    assert!(root.path().join("save").join("data.bin.velo-tmp").is_file());
    assert!(
        progress
            .snapshot()
            .iter()
            .all(|s| s.state == SegmentState::Pending)
    );
}

#[tokio::test]
async fn servers_without_ranges_and_empty_files_use_one_connection() {
    let (_root, progress, result) = download("/norange/data.bin", &options()).await;
    assert!(fs::read(result.unwrap()).unwrap() == body());
    assert_eq!(progress.snapshot().len(), 1);

    let (_root, _, result) = download("/empty.txt", &options()).await;
    assert!(fs::read(result.unwrap()).unwrap().is_empty());
}

#[tokio::test]
async fn a_download_resumes_from_its_saved_progress() {
    let base = support::start().await;
    let root = tempfile::tempdir().unwrap();
    let data = root.path().join("data.bin.velo-tmp");
    let half = SIZE as u64 / 2;
    let expected = body();

    // Segment 0 has its first 1000 bytes; segment 1 is done. The rest of the file is garbage
    // that a resume must overwrite, so only the saved counts can be trusted.
    let mut file = vec![0xAA; SIZE];
    file[..1000].copy_from_slice(&expected[..1000]);
    file[half as usize..].copy_from_slice(&expected[half as usize..]);
    fs::write(&data, &file).unwrap();
    let mut first = Segment::new(SegmentId(0), 0, half);
    first.downloaded = 1000;
    let mut second = Segment::new(SegmentId(1), half, SIZE as u64);
    second.downloaded = second.len();
    second.state = SegmentState::Done;
    let progress = Progress::new(vec![first, second]);

    let url = Url::parse(&format!("{base}/files/data.bin")).unwrap();
    download_segments(
        &client().unwrap(),
        &url,
        &data,
        SIZE as u64,
        &progress,
        &options(),
        &CancellationToken::new(),
    )
    .await
    .unwrap();
    assert_eq!(progress.durable_snapshot(), progress.snapshot());
    let save_path = root.path().join("data.bin");
    finish(&data, &progress.snapshot(), Some(SIZE as u64), &save_path)
        .await
        .unwrap();
    assert!(fs::read(&save_path).unwrap() == expected);
    assert!(!data.exists());
}

#[tokio::test]
async fn a_stopped_download_resumes_from_its_durable_progress() {
    let base = support::start().await;
    let root = tempfile::tempdir().unwrap();
    let data = root.path().join("data.bin.velo-tmp");
    let url = Url::parse(&format!("{base}/slow/data.bin")).unwrap();
    let progress = Progress::new(vec![Segment::new(SegmentId(0), 0, SIZE as u64)]);

    // Stop part way, then resume from what would be saved, as after a restart.
    let cancel = CancellationToken::new();
    let stop = {
        let (cancel, progress) = (cancel.clone(), progress.clone());
        async move {
            while progress.downloaded() < SIZE as u64 / 3 {
                tokio::time::sleep(Duration::from_millis(1)).await;
            }
            cancel.cancel();
        }
    };
    let (client, options) = (client().unwrap(), options());
    let (stopped, ()) = tokio::join!(
        download_segments(
            &client,
            &url,
            &data,
            SIZE as u64,
            &progress,
            &options,
            &cancel
        ),
        stop
    );
    assert!(matches!(stopped, Err(Error::Cancelled)), "{stopped:?}");
    let saved = progress.durable_snapshot();
    assert!(saved.iter().any(|s| s.downloaded > 0), "{saved:?}");
    assert!(saved.iter().any(|s| s.remaining() > 0), "{saved:?}");
    // Every byte counted as durable really is in the file, at its segment's offset.
    let (on_disk, expected) = (fs::read(&data).unwrap(), body());
    for s in &saved {
        let range = s.start as usize..(s.start + s.downloaded) as usize;
        assert!(on_disk[range.clone()] == expected[range], "{s:?}");
    }

    let resumed = Progress::new(saved);
    download_segments(
        &client,
        &url,
        &data,
        SIZE as u64,
        &resumed,
        &options,
        &CancellationToken::new(),
    )
    .await
    .unwrap();
    assert!(fs::read(&data).unwrap() == body());
}

#[tokio::test]
async fn finish_refuses_unfinished_downloads_and_existing_files() {
    let root = tempfile::tempdir().unwrap();
    let data = root.path().join("out.txt.velo-tmp");
    fs::write(&data, b"abc").unwrap();
    let save_path = root.path().join("out.txt");
    let mut segment = Segment::new(SegmentId(0), 0, 3);

    let unfinished = finish(&data, std::slice::from_ref(&segment), Some(3), &save_path).await;
    assert!(
        matches!(unfinished, Err(Error::NotFinished)),
        "{unfinished:?}"
    );

    segment.downloaded = 3;
    segment.state = SegmentState::Done;
    let wrong_size = finish(&data, std::slice::from_ref(&segment), Some(4), &save_path).await;
    assert!(
        matches!(wrong_size, Err(Error::NotFinished)),
        "{wrong_size:?}"
    );

    fs::write(&save_path, b"old").unwrap();
    let exists = finish(&data, std::slice::from_ref(&segment), Some(3), &save_path).await;
    assert!(matches!(exists, Err(Error::Exists(_))), "{exists:?}");
    assert_eq!(fs::read(&save_path).unwrap(), b"old");
    assert!(Path::new(&data).exists());

    fs::remove_file(&save_path).unwrap();
    fs::write(&data, b"abcd").unwrap();
    let short = finish(&data, std::slice::from_ref(&segment), Some(3), &save_path).await;
    assert!(
        matches!(short, Err(Error::SizeMismatch { .. })),
        "{short:?}"
    );

    fs::write(&data, b"abc").unwrap();
    finish(&data, &[segment], Some(3), &save_path)
        .await
        .unwrap();
    assert_eq!(fs::read(&save_path).unwrap(), b"abc");
    assert!(!data.exists());
}

#[tokio::test]
async fn long_ranges_are_fetched_in_capped_requests() {
    let base = support::start().await;
    let one_connection = Options {
        max_connections: 1,
        ..options()
    };
    let root = tempfile::tempdir().unwrap();
    let url = format!("{base}/capped/data.bin");
    let uncapped = download_file(
        &client().unwrap(),
        &url,
        &root.path().join("temp"),
        DownloadId(1),
        root.path(),
        &one_connection,
        &Progress::default(),
    )
    .await;
    assert!(uncapped.is_err(), "the server refuses one long range");

    let capped = Options {
        max_request_size: Some(support::CAP),
        ..one_connection
    };
    let root = tempfile::tempdir().unwrap();
    let path = download_file(
        &client().unwrap(),
        &url,
        &root.path().join("temp"),
        DownloadId(2),
        root.path(),
        &capped,
        &Progress::default(),
    )
    .await
    .unwrap();
    assert_eq!(fs::read(path).unwrap(), body());
}
