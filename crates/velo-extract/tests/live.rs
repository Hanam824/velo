//! A check against the real YouTube, which changes without notice. It is ignored by default,
//! so normal test runs never touch the internet; the nightly smoke workflow runs it with
//! `VELO_LIVE_YOUTUBE=1 cargo test -p velo-extract --test live -- --ignored`, and
//! `scripts/youtube-check.sh` runs it from a home connection.
//!
//! For a short and a long video it asks every client alone for the first bytes, the middle and
//! a whole 10 MiB request from the middle of the best stream (what the engine asks for), and
//! prints one line each. Some clients get addresses that are served for the first few megabytes
//! and then refused, so the first bytes alone prove nothing: the test fails when streams are
//! served at the start but refused deeper everywhere.
//!
//! YouTube refuses every stream to datacenter addresses such as GitHub's runners. When even the
//! first bytes are refused to every client, the download part is skipped (and says so).

use reqwest::header::{RANGE, USER_AGENT};
use velo_extract::{Client, Extractor, StreamKind, Video};

/// "Me at the zoo", the first video on YouTube, which is unlikely to go away.
const SHORT: &str = "https://www.youtube.com/watch?v=jNQXAC9IVRw";

/// A video of several hours with a stream of hundreds of megabytes, for what only long videos
/// show. `VELO_LIVE_LONG_VIDEO` (an ID or an address) replaces it.
const LONG: &str = "JHF4MPoADSs";

/// The answer to asking `url` as `user_agent` for `length` bytes from `at`; 0 if it didn't answer.
async fn status(http: &reqwest::Client, user_agent: &str, url: &str, at: u64, length: u64) -> u16 {
    let range = format!("bytes={at}-{}", at + length - 1);
    match http
        .get(url)
        .header(USER_AGENT, user_agent)
        .header(RANGE, range)
        .send()
        .await
    {
        Ok(response) => response.status().as_u16(),
        Err(error) => {
            eprintln!("  request failed: {error}");
            0
        }
    }
}

/// What a client's best stream does when asked for bytes.
struct Served {
    first: u16,
    deep: u16,
    big: u16,
}

/// Asks every client alone about `url` and prints what each one's best stream serves.
async fn clients_report(http: &reqwest::Client, label: &str, url: &str) -> Vec<Served> {
    let mut report = Vec::new();
    for client in Client::DEFAULT_ORDER {
        let alone = Extractor::new(http.clone())
            .with_clients(vec![client])
            .with_stream_check(false);
        let video = match alone.video_at(url).await {
            Ok(video) => video,
            Err(error) => {
                eprintln!("{label} {}: {error}", client.name);
                continue;
            }
        };
        let choices = video.choices();
        let Some(best) = choices.first() else {
            eprintln!("{label} {}: no format to download", client.name);
            continue;
        };
        let format = best.formats()[0];
        let (Some(address), Some(length)) = (format.url.as_deref(), format.content_length) else {
            eprintln!("{label} {}: no address or size", client.name);
            continue;
        };
        let middle = length / 2;
        let served = Served {
            first: status(http, client.user_agent, address, 0, 1024).await,
            deep: status(http, client.user_agent, address, middle, 1024).await,
            big: status(http, client.user_agent, address, middle, 10 << 20).await,
        };
        eprintln!(
            "{label} {}: itag {} ({length} bytes) first-KiB={} deep={} big-range={}",
            client.name, format.itag, served.first, served.deep, served.big
        );
        report.push(served);
    }
    report
}

/// Checks the video at `url`, which must exist when `required`; returns it as the extractor
/// gives it.
async fn check(label: &str, url: &str, required: bool) -> Option<Video> {
    let http = velo_engine::client().unwrap();
    let report = clients_report(&http, label, url).await;

    let refused_at_start = !report.is_empty() && report.iter().all(|s| s.first == 403);
    if refused_at_start {
        eprintln!(
            "SKIPPED the download check of {label}: YouTube refuses every stream to this address \
             (a datacenter?)"
        );
        let video = Extractor::new(http)
            .with_stream_check(false)
            .video_at(url)
            .await
            .unwrap();
        return Some(video);
    }
    if report.is_empty() {
        assert!(!required, "{label}: no client got the video's streams");
        eprintln!("SKIPPED {label}: no client got the video's streams");
        return None;
    }
    assert!(
        report.iter().any(|s| s.deep == 206 && s.big == 206),
        "{label}: streams are served at the start but refused deeper by every client; \
         YouTube probably wants a PO token"
    );

    // The extractor, with its own stream check, must now end up with a client that works.
    let video = Extractor::new(http.clone()).video_at(url).await.unwrap();
    eprintln!("{label}: the extractor chose {}", video.client);
    let client = Client::DEFAULT_ORDER
        .into_iter()
        .find(|c| c.name == video.client)
        .expect("a known client");
    let choices = video.choices();
    let format = choices[0].formats()[0];
    let length = format.content_length.unwrap_or(2048);
    let deep = status(
        &http,
        client.user_agent,
        format.url.as_deref().unwrap(),
        length / 2,
        1024,
    )
    .await;
    assert_eq!(
        deep, 206,
        "{label}: the chosen client's best stream isn't served"
    );
    Some(video)
}

#[tokio::test]
#[ignore = "talks to YouTube; set VELO_LIVE_YOUTUBE=1 and run with --ignored"]
async fn real_videos_have_streams_that_download() {
    if std::env::var_os("VELO_LIVE_YOUTUBE").is_none() {
        eprintln!("skipped: VELO_LIVE_YOUTUBE isn't set");
        return;
    }

    let video = check("short", SHORT, true).await.unwrap();
    eprintln!(
        "{} by {} via {}: {} formats",
        video.title,
        video.author,
        video.client,
        video.formats.len()
    );
    assert_eq!(video.id, "jNQXAC9IVRw");
    assert!(video.duration.is_some_and(|d| d > 0));
    assert!(video.formats.iter().any(|f| f.kind == StreamKind::Audio));
    assert!(video.formats.iter().any(|f| f.kind != StreamKind::Audio));

    let long = std::env::var("VELO_LIVE_LONG_VIDEO").unwrap_or_else(|_| LONG.into());
    let long = if long.contains('/') {
        long
    } else {
        format!("https://www.youtube.com/watch?v={long}")
    };
    // A long video may be removed one day; that is a reason to pick another, not a failure.
    if let Some(video) = check("long", &long, false).await {
        eprintln!(
            "long video: {:?} seconds, best stream {:?} bytes",
            video.duration,
            video.choices()[0].size()
        );
    }
}
