//! End-to-end downloads under hostile conditions, checked by SHA-256, and a benchmark of how
//! connections scale.

mod support;

use std::fs;
use std::path::Path;
use std::time::{Duration, Instant};

use sha2::{Digest, Sha256};
use tokio::sync::broadcast::Receiver;
use velo_core::{DownloadId, DownloadState, EngineEvent, Settings};
use velo_engine::{
    Engine, EngineConfig, NewDownload, Options, Progress, RequestOptions, RetryPolicy, client,
    download_file,
};

use support::{BIG_SIZE, big_body, body};

/// The SHA-256 of `bytes`, as hex.
fn sha256(bytes: &[u8]) -> String {
    Sha256::digest(bytes)
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

/// An engine with two connections, small segments and fast retries.
fn engine(root: &Path, downloads: Vec<velo_core::Download>) -> Engine {
    let config = EngineConfig {
        temp_dir: root.join("temp"),
        settings: Settings {
            max_connections: 2,
            min_segment_size: 16 * 1024,
            ..Settings::default()
        },
        retry: RetryPolicy {
            max_retries: 3,
            delay: Duration::from_millis(5),
        },
    };
    Engine::start(config, downloads, Vec::new()).unwrap()
}

/// Waits up to 20 s for download `id` to reach a state `done` accepts.
async fn wait_for_state(
    events: &mut Receiver<EngineEvent>,
    id: DownloadId,
    done: impl Fn(&DownloadState) -> bool,
) -> velo_core::Download {
    tokio::time::timeout(Duration::from_secs(20), async {
        loop {
            if let EngineEvent::Changed(d) = events.recv().await.unwrap()
                && d.id == id
                && done(&d.state)
            {
                return d;
            }
        }
    })
    .await
    .expect("timed out waiting for the download")
}

#[tokio::test]
async fn a_server_that_cuts_every_connection_still_yields_the_exact_file() {
    let base = support::start().await;
    let root = tempfile::tempdir().unwrap();
    let options = Options {
        max_connections: 4,
        min_segment_size: 16 * 1024,
        max_retries: 2,
        retry_delay: Duration::from_millis(5),
        limiters: Vec::new(),
    };
    // Every request is cut after 64 KiB; each retry resumes where the last stopped.
    let path = download_file(
        &client().unwrap(),
        &format!("{base}/choppy/data.bin"),
        &root.path().join("temp"),
        DownloadId(1),
        root.path(),
        &options,
        &Progress::default(),
    )
    .await
    .unwrap();
    assert_eq!(sha256(&fs::read(path).unwrap()), sha256(&body()));
}

#[tokio::test]
async fn a_flaky_download_paused_and_resumed_after_a_restart_yields_the_exact_file() {
    let base = support::start().await;
    let root = tempfile::tempdir().unwrap();
    let save_dir = root.path().join("save");
    fs::create_dir_all(&save_dir).unwrap();
    let first = engine(root.path(), Vec::new());
    let mut events = first.subscribe();
    let id = first.add(NewDownload {
        url: format!("{base}/flakyslow/data.bin"),
        file_name: String::new(),
        save_dir,
        category: None,
        description: String::new(),
        start: true,
        request: RequestOptions::default(),
    });

    // Pause once some data is in, after at least one connection was cut.
    tokio::time::timeout(Duration::from_secs(20), async {
        loop {
            if let EngineEvent::Progress(p) = events.recv().await.unwrap()
                && p.id == id
                && p.downloaded >= 60_000
            {
                break;
            }
        }
    })
    .await
    .unwrap();
    first.pause(id);
    let paused = wait_for_state(&mut events, id, |s| *s == DownloadState::Paused).await;
    first.shutdown().await;

    let second = engine(root.path(), vec![paused]);
    let mut events = second.subscribe();
    second.resume(id);
    let done = wait_for_state(&mut events, id, |s| *s == DownloadState::Complete).await;
    assert_eq!(
        sha256(&fs::read(done.save_path()).unwrap()),
        sha256(&body())
    );
}

/// Downloads `/big` (8 MiB, throttled to about 3 MB/s per connection) with 1, 8 and 16
/// connections and prints the throughput of each.
///
/// Run with `cargo test -p velo-engine --test integration -- --ignored --nocapture`.
#[tokio::test]
#[ignore = "takes several seconds; run on demand"]
async fn benchmark_connections() {
    let base = support::start().await;
    let expected = sha256(big_body());
    let mut rates = Vec::new();
    for connections in [1, 8, 16] {
        let root = tempfile::tempdir().unwrap();
        let options = Options {
            max_connections: connections,
            min_segment_size: 256 * 1024,
            ..Options::default()
        };
        let start = Instant::now();
        let path = download_file(
            &client().unwrap(),
            &format!("{base}/big/big.bin"),
            &root.path().join("temp"),
            DownloadId(1),
            root.path(),
            &options,
            &Progress::default(),
        )
        .await
        .unwrap();
        let seconds = start.elapsed().as_secs_f64();
        assert_eq!(sha256(&fs::read(path).unwrap()), expected);
        let rate = BIG_SIZE as f64 / seconds / (1024.0 * 1024.0);
        println!("{connections:>2} connections: {seconds:6.2} s, {rate:6.2} MB/s");
        rates.push(rate);
    }
    // Eight connections on a per-connection-throttled server should be much faster than one.
    assert!(rates[1] > rates[0] * 3.0, "{rates:?}");
}
