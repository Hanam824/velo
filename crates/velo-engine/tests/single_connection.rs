//! Probing and single-connection downloads against the local test server.

mod support;

use std::fs;

use reqwest::{StatusCode, Url};
use velo_core::DownloadId;
use velo_engine::planner::initial_segments;
use velo_engine::{Error, client, fetch_range, part_path, probe};

use support::{ETAG_VALUE, LAST_MODIFIED_VALUE, SIZE, body};

#[tokio::test]
async fn probe_reads_size_range_support_name_and_version() {
    let base = support::start().await;
    let probe = probe(&client().unwrap(), &format!("{base}/files/data.bin"))
        .await
        .unwrap();
    assert_eq!(probe.total_size, Some(SIZE as u64));
    assert!(probe.resumable);
    assert_eq!(probe.file_name, "data.bin");
    assert_eq!(probe.etag.as_deref(), Some(ETAG_VALUE));
    assert_eq!(probe.last_modified.as_deref(), Some(LAST_MODIFIED_VALUE));
}

#[tokio::test]
async fn probe_falls_back_to_a_range_request_when_head_is_rejected() {
    let base = support::start().await;
    let probe = probe(&client().unwrap(), &format!("{base}/nohead/data.bin"))
        .await
        .unwrap();
    assert_eq!(probe.total_size, Some(SIZE as u64));
    assert!(probe.resumable);
    assert_eq!(probe.etag.as_deref(), Some(ETAG_VALUE));
}

#[tokio::test]
async fn probe_detects_a_server_without_range_support() {
    let base = support::start().await;
    let probe = probe(&client().unwrap(), &format!("{base}/norange/data.bin"))
        .await
        .unwrap();
    assert_eq!(probe.total_size, Some(SIZE as u64));
    assert!(!probe.resumable);
}

#[tokio::test]
async fn probe_follows_redirects_and_reads_the_content_disposition_name() {
    let base = support::start().await;
    let client = client().unwrap();

    let redirected = probe(&client, &format!("{base}/redirect")).await.unwrap();
    assert_eq!(redirected.url.path(), "/files/redirected.bin");
    assert_eq!(redirected.file_name, "redirected.bin");

    let attachment = probe(&client, &format!("{base}/attachment")).await.unwrap();
    assert_eq!(attachment.file_name, "résumé.pdf");
}

#[tokio::test]
async fn probe_handles_empty_files_and_reports_error_statuses() {
    let base = support::start().await;
    let client = client().unwrap();

    let empty = probe(&client, &format!("{base}/empty.txt")).await.unwrap();
    assert_eq!(empty.total_size, Some(0));
    assert!(!empty.resumable);

    let missing = probe(&client, &format!("{base}/missing.bin")).await;
    assert!(
        matches!(missing, Err(Error::Status(StatusCode::NOT_FOUND))),
        "{missing:?}"
    );
}

#[tokio::test]
async fn a_probed_file_downloads_into_its_part_file() {
    let base = support::start().await;
    let temp = tempfile::tempdir().unwrap();
    let client = client().unwrap();
    let probe = probe(&client, &format!("{base}/files/data.bin"))
        .await
        .unwrap();
    let total = probe.total_size.unwrap();
    let segment = &initial_segments(total)[0];
    let part = part_path(temp.path(), DownloadId(7), segment);
    assert!(part.ends_with(std::path::Path::new("7").join("0.part")));

    let mut progress = 0;
    let len = fetch_range(
        &client,
        &probe.url,
        &part,
        segment.start,
        Some(segment.end),
        |n| progress += n,
    )
    .await
    .unwrap();
    assert_eq!((len, progress), (total, total));
    assert!(fs::read(&part).unwrap() == body());
}

#[tokio::test]
async fn a_fetch_resumes_after_the_bytes_already_in_the_part_file() {
    let base = support::start().await;
    let temp = tempfile::tempdir().unwrap();
    let client = client().unwrap();
    let url = Url::parse(&format!("{base}/files/data.bin")).unwrap();
    let expected = body();

    // The whole file, with the first 1000 bytes already on disk.
    let whole = temp.path().join("whole.part");
    fs::write(&whole, &expected[..1000]).unwrap();
    let mut progress = 0;
    let len = fetch_range(&client, &url, &whole, 0, Some(SIZE as u64), |n| {
        progress += n
    })
    .await
    .unwrap();
    assert_eq!((len, progress), (SIZE as u64, SIZE as u64 - 1000));
    assert!(fs::read(&whole).unwrap() == expected);

    // A middle range, to the end of the file.
    let middle = temp.path().join("middle.part");
    fs::write(&middle, &expected[5000..5100]).unwrap();
    fetch_range(&client, &url, &middle, 5000, None, |_| {})
        .await
        .unwrap();
    assert!(fs::read(&middle).unwrap() == expected[5000..]);
}

#[tokio::test]
async fn a_part_file_longer_than_its_range_is_cut_back() {
    let base = support::start().await;
    let temp = tempfile::tempdir().unwrap();
    let url = Url::parse(&format!("{base}/files/data.bin")).unwrap();
    let part = temp.path().join("long.part");
    fs::write(&part, &body()[100..3100]).unwrap();

    let len = fetch_range(&client().unwrap(), &url, &part, 100, Some(2100), |_| {})
        .await
        .unwrap();
    assert_eq!(len, 2000);
    assert!(fs::read(&part).unwrap() == body()[100..2100]);
}

#[tokio::test]
async fn a_server_without_ranges_downloads_whole_but_cannot_resume() {
    let base = support::start().await;
    let temp = tempfile::tempdir().unwrap();
    let client = client().unwrap();
    let url = Url::parse(&format!("{base}/norange/data.bin")).unwrap();

    let part = temp.path().join("whole.part");
    let len = fetch_range(&client, &url, &part, 0, None, |_| {})
        .await
        .unwrap();
    assert_eq!(len, SIZE as u64);
    assert!(fs::read(&part).unwrap() == body());

    let partial = temp.path().join("partial.part");
    fs::write(&partial, &body()[..10]).unwrap();
    let resumed = fetch_range(&client, &url, &partial, 0, None, |_| {}).await;
    assert!(matches!(resumed, Err(Error::RangeIgnored)), "{resumed:?}");
}
