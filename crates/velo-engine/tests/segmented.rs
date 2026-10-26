//! Multi-connection downloads, retries and merging against the local test server.

mod support;

use std::fs;
use std::path::Path;
use std::time::Duration;

use reqwest::{StatusCode, Url};
use velo_core::{DownloadId, Segment, SegmentId, SegmentState};
use velo_engine::{Error, Options, Progress, client, download_file, download_segments, merge};

use support::{SIZE, body};

/// Four connections, small segments and fast retries.
fn options() -> Options {
    Options {
        max_connections: 4,
        min_segment_size: 16 * 1024,
        max_retries: 3,
        retry_delay: Duration::from_millis(5),
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
async fn a_file_downloads_over_several_connections_and_merges() {
    let (root, progress, result) = download("/slow/data.bin", &options()).await;
    let path = result.unwrap();
    assert_eq!(path, root.path().join("save").join("data.bin"));
    assert!(fs::read(&path).unwrap() == body());

    // Idle workers split the file; every part was merged and the temp folder removed.
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
async fn a_server_that_keeps_failing_gives_up_and_keeps_the_parts() {
    let (root, progress, result) = download("/fail/data.bin", &options()).await;
    assert!(
        matches!(result, Err(Error::Status(StatusCode::SERVICE_UNAVAILABLE))),
        "{result:?}"
    );
    assert!(!root.path().join("save").join("data.bin").exists());
    assert!(root.path().join("temp").join("1").is_dir());
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
async fn a_download_resumes_from_its_part_files() {
    let base = support::start().await;
    let root = tempfile::tempdir().unwrap();
    let dir = root.path().join("parts");
    fs::create_dir_all(&dir).unwrap();
    let half = SIZE as u64 / 2;
    let expected = body();

    // Part 0 has 1000 bytes; part 1 is complete and one byte too long.
    fs::write(dir.join("0.part"), &expected[..1000]).unwrap();
    let mut long = expected[half as usize..].to_vec();
    long.push(0);
    fs::write(dir.join("1.part"), &long).unwrap();
    let progress = Progress::new(vec![
        Segment::new(SegmentId(0), 0, half),
        Segment::new(SegmentId(1), half, SIZE as u64),
    ]);

    let url = Url::parse(&format!("{base}/files/data.bin")).unwrap();
    download_segments(&client().unwrap(), &url, &dir, &progress, &options())
        .await
        .unwrap();
    let save_path = root.path().join("data.bin");
    merge(&dir, &progress.snapshot(), Some(SIZE as u64), &save_path)
        .await
        .unwrap();
    assert!(fs::read(&save_path).unwrap() == expected);
}

#[tokio::test]
async fn merge_refuses_unfinished_downloads_and_existing_files() {
    let root = tempfile::tempdir().unwrap();
    let dir = root.path().join("parts");
    fs::create_dir_all(&dir).unwrap();
    fs::write(dir.join("0.part"), b"abc").unwrap();
    let save_path = root.path().join("out.txt");
    let mut segment = Segment::new(SegmentId(0), 0, 3);

    let unfinished = merge(&dir, std::slice::from_ref(&segment), Some(3), &save_path).await;
    assert!(
        matches!(unfinished, Err(Error::NotFinished)),
        "{unfinished:?}"
    );

    segment.downloaded = 3;
    segment.state = SegmentState::Done;
    let wrong_size = merge(&dir, std::slice::from_ref(&segment), Some(4), &save_path).await;
    assert!(
        matches!(wrong_size, Err(Error::NotFinished)),
        "{wrong_size:?}"
    );

    fs::write(&save_path, b"old").unwrap();
    let exists = merge(&dir, std::slice::from_ref(&segment), Some(3), &save_path).await;
    assert!(matches!(exists, Err(Error::Exists(_))), "{exists:?}");
    assert_eq!(fs::read(&save_path).unwrap(), b"old");
    assert!(Path::new(&dir.join("0.part")).exists());

    fs::remove_file(&save_path).unwrap();
    merge(&dir, &[segment], Some(3), &save_path).await.unwrap();
    assert_eq!(fs::read(&save_path).unwrap(), b"abc");
    assert!(!dir.exists());
}
