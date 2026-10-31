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
//!
//! It also shows those addresses a bot check ("Sign in to confirm you're not a bot") instead of
//! the video. That says nothing about Velo, so when every client failed that way the result is
//! `INCONCLUSIVE` (printed, not a failure). A client that failed for another reason, such as a
//! player script of a shape Velo can't read, is a real failure. The player script itself has no
//! bot check, so `current_player_can_be_solved` tests it from any address.

use reqwest::header::{RANGE, USER_AGENT};
use velo_extract::{Client, Error, Extractor, StreamKind, Video};

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

/// Why a client got no streams for a video.
enum Failure {
    /// YouTube showed a bot check; nothing is known about Velo.
    Blocked,
    /// Anything else: Velo or YouTube's format is at fault.
    Broken,
}

impl Failure {
    fn of(error: &Error) -> Self {
        match error {
            Error::LoginRequired(_) => Self::Blocked,
            _ => Self::Broken,
        }
    }
}

/// What the clients' answers add up to.
#[derive(Debug, PartialEq)]
enum Verdict {
    /// Some client got streams; go on to check them.
    Streams,
    /// No client got streams, and every one was shown a bot check.
    Inconclusive,
    /// No client got streams, and some failed for a reason that is Velo's to fix.
    Broken,
}

fn verdict(served: &[Served], failures: &[Failure]) -> Verdict {
    if !served.is_empty() {
        Verdict::Streams
    } else if failures.iter().all(|f| matches!(f, Failure::Blocked)) && !failures.is_empty() {
        Verdict::Inconclusive
    } else {
        Verdict::Broken
    }
}

/// Asks every client alone about `url` and prints what each one's best stream serves.
async fn clients_report(
    http: &reqwest::Client,
    label: &str,
    url: &str,
) -> (Vec<Served>, Vec<Failure>) {
    let mut report = Vec::new();
    let mut failures = Vec::new();
    for client in Client::DEFAULT_ORDER {
        let alone = Extractor::new(http.clone())
            .with_clients(vec![client])
            .with_stream_check(false);
        let video = match alone.video_at(url).await {
            Ok(video) => video,
            Err(error) => {
                eprintln!("{label} {}: {error}", client.name);
                failures.push(Failure::of(&error));
                continue;
            }
        };
        let choices = video.choices();
        let Some(best) = choices.first() else {
            eprintln!("{label} {}: no format to download", client.name);
            failures.push(Failure::Broken);
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
    (report, failures)
}

/// Checks the video at `url`, which must exist when `required`; returns it as the extractor
/// gives it.
async fn check(label: &str, url: &str, required: bool) -> Option<Video> {
    let http = velo_engine::client().unwrap();
    let (report, failures) = clients_report(&http, label, url).await;

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
        if verdict(&report, &failures) == Verdict::Inconclusive {
            eprintln!(
                "INCONCLUSIVE {label}: YouTube shows this address a bot check, so extraction                  can't be verified from here"
            );
            return None;
        }
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

    let Some(video) = check("short", SHORT, true).await else {
        return;
    };
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

/// The player script is a static file without a bot check, so this tells a changed player from
/// a blocked address.
#[tokio::test]
#[ignore = "talks to YouTube; set VELO_LIVE_YOUTUBE=1 and run with --ignored"]
async fn current_player_can_be_solved() {
    if std::env::var_os("VELO_LIVE_YOUTUBE").is_none() {
        eprintln!("skipped: VELO_LIVE_YOUTUBE isn't set");
        return;
    }
    let player = Extractor::new(velo_engine::client().unwrap())
        .player()
        .await
        .unwrap_or_else(|e| panic!("player: {e}"));
    let signature = "0123456789ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz";
    let n = "abcdefghij12";
    let (signatures, ns) = player
        .solve(&[signature.into()], &[n.into()])
        .unwrap_or_else(|e| panic!("player {} solve: {e}", player.id));
    eprintln!("player {}: solved", player.id);
    assert!(!signatures[0].is_empty() && signatures[0] != signature);
    assert!(!ns[0].is_empty() && ns[0] != n);
}

#[test]
fn only_bot_checks_make_a_check_inconclusive() {
    let served = Served {
        first: 206,
        deep: 206,
        big: 206,
    };
    assert_eq!(verdict(&[served], &[Failure::Broken]), Verdict::Streams);
    assert_eq!(
        verdict(&[], &[Failure::Blocked, Failure::Blocked]),
        Verdict::Inconclusive
    );
    assert_eq!(
        verdict(&[], &[Failure::Blocked, Failure::Broken]),
        Verdict::Broken
    );
    assert_eq!(verdict(&[], &[]), Verdict::Broken);
}
