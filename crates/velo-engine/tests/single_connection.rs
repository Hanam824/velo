//! Probing and single-connection downloads against the local test server.

mod support;

use std::fs;

use reqwest::{StatusCode, Url};
use velo_engine::{CancellationToken, Error, client, fetch_whole, probe};

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
async fn a_server_without_ranges_downloads_whole_from_the_first_byte() {
    let base = support::start().await;
    let temp = tempfile::tempdir().unwrap();
    let client = client().unwrap();
    let url = Url::parse(&format!("{base}/norange/data.bin")).unwrap();

    // Bytes of an earlier attempt can't be resumed: they are replaced.
    let file = temp.path().join("7").join("data.bin.velo-tmp");
    fs::create_dir_all(file.parent().unwrap()).unwrap();
    fs::write(&file, vec![0xAA; SIZE + 10]).unwrap();
    let mut progress = 0;
    let len = fetch_whole(&client, &url, &file, &[], &CancellationToken::new(), |n| {
        progress += n
    })
    .await
    .unwrap();
    assert_eq!((len, progress), (SIZE as u64, SIZE as u64));
    assert!(fs::read(&file).unwrap() == body());

    let missing = Url::parse(&format!("{base}/missing")).unwrap();
    let refused = fetch_whole(
        &client,
        &missing,
        &file,
        &[],
        &CancellationToken::new(),
        |_| {},
    )
    .await;
    assert!(
        matches!(refused, Err(Error::Status(StatusCode::NOT_FOUND))),
        "{refused:?}"
    );
}
