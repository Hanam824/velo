//! The extractor against a local stand-in for YouTube's player API, with synthetic answers.

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use axum::Router;
use axum::extract::State;
use axum::http::{HeaderMap, StatusCode};
use axum::routing::{get, post};
use serde_json::Value;
use velo_extract::{Client, Error, Extractor, StreamKind};

/// What the stand-in received: the client name header and the request body.
type Requests = Arc<Mutex<Vec<(String, Value)>>>;

/// The fixture that answers `client_name` asking about `video_id`, or an HTTP error.
fn answer(video_id: &str, client_name: &str) -> Result<&'static str, StatusCode> {
    Ok(match (video_id, client_name) {
        ("okokokokok1", "ANDROID_VR") => include_str!("fixtures/unplayable.json"),
        ("okokokokok1", _) => include_str!("fixtures/ok.json"),
        ("agegateage1", _) => include_str!("fixtures/age.json"),
        ("botcheckbot", _) => include_str!("fixtures/bot.json"),
        ("privatevid1", _) => include_str!("fixtures/private.json"),
        ("removedvid1", _) => include_str!("fixtures/removed.json"),
        ("brokenvid11", "ANDROID_VR") => return Err(StatusCode::INTERNAL_SERVER_ERROR),
        ("webwebweb01", "WEB") => include_str!("fixtures/web.json"),
        ("webwebweb01", _) => include_str!("fixtures/unplayable.json"),
        _ => "not json",
    })
}

/// How many times the stand-in sent the player script.
static PLAYER_DOWNLOADS: AtomicUsize = AtomicUsize::new(0);

/// Starts the stand-in and returns an extractor pointed at it and what it receives.
async fn start() -> (Extractor, Requests) {
    let requests = Requests::default();
    let app = Router::new()
        .route(
            "/iframe_api",
            get(|| async { include_str!("fixtures/iframe_api") }),
        )
        .route(
            "/s/player/a1b2c3d4/player_ias.vflset/en_US/base.js",
            get(|| async {
                PLAYER_DOWNLOADS.fetch_add(1, Ordering::SeqCst);
                include_str!("fixtures/base.js")
            }),
        )
        .route(
            "/youtubei/v1/player",
            post(
                |State(requests): State<Requests>, headers: HeaderMap, body: String| async move {
                    let body: Value = serde_json::from_str(&body).unwrap();
                    let client_name = headers
                        .get("x-youtube-client-name")
                        .and_then(|v| v.to_str().ok())
                        .unwrap_or_default()
                        .to_string();
                    requests.lock().unwrap().push((client_name, body.clone()));
                    answer(
                        body["videoId"].as_str().unwrap_or_default(),
                        body["context"]["client"]["clientName"]
                            .as_str()
                            .unwrap_or_default(),
                    )
                },
            ),
        )
        .with_state(requests.clone());
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}/", listener.local_addr().unwrap());
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    let extractor = Extractor::new(velo_engine::client().unwrap()).with_base(base.parse().unwrap());
    (extractor, requests)
}

#[tokio::test]
async fn the_next_client_is_tried_when_one_is_refused() {
    let (extractor, requests) = start().await;
    let video = extractor
        .video_at("https://youtu.be/okokokokok1")
        .await
        .unwrap();
    assert_eq!(video.id, "okokokokok1");
    assert_eq!(video.title, "Velo test video");
    assert_eq!(video.author, "Velo");
    assert_eq!(video.duration, Some(212));
    assert_eq!(video.client, "ios");

    // The scrambled Opus stream needs the player's JavaScript, so it is left out.
    let itags: Vec<u32> = video.formats.iter().map(|f| f.itag).collect();
    assert_eq!(itags, [18, 137, 140]);
    let kinds: Vec<StreamKind> = video.formats.iter().map(|f| f.kind).collect();
    assert_eq!(
        kinds,
        [StreamKind::Muxed, StreamKind::Video, StreamKind::Audio]
    );
    assert_eq!(video.formats[1].height, Some(1080));
    assert_eq!(video.formats[2].content_length, Some(3_430_000));

    let choices: Vec<Vec<u32>> = video.choices().iter().map(|c| c.itags()).collect();
    assert_eq!(
        choices,
        [vec![137, 140], vec![18], vec![140]],
        "1080p joined, 360p with sound, then sound only"
    );
    let joined = video.choice(&[137, 140]).unwrap();
    assert_eq!(joined.size(), Some(92_000_000 + 3_430_000));
    assert_eq!(video.file_name(&joined), "Velo test video.mp4");
    let sound = video.choice(&[140]).unwrap();
    assert!(sound.is_audio_only());
    assert_eq!(video.file_name(&sound), "Velo test video.m4a");
    assert_eq!(video.choice(&[1, 2, 3]), None);

    let requests = requests.lock().unwrap();
    let names: Vec<&str> = requests.iter().map(|(name, _)| name.as_str()).collect();
    assert_eq!(names, ["28", "5"], "ANDROID_VR, then IOS");
    assert_eq!(requests[1].1["videoId"], "okokokokok1");
}

#[tokio::test]
async fn refusals_become_the_most_telling_error() {
    let (extractor, requests) = start().await;
    assert!(matches!(
        extractor.video("agegateage1").await,
        Err(Error::AgeRestricted)
    ));
    assert!(matches!(
        extractor.video("botcheckbot").await,
        Err(Error::LoginRequired(reason)) if reason.contains("bot")
    ));

    requests.lock().unwrap().clear();
    assert!(matches!(
        extractor.video("privatevid1").await,
        Err(Error::Private)
    ));
    assert!(matches!(
        extractor.video("removedvid1").await,
        Err(Error::Unavailable(reason)) if reason == "This video is unavailable"
    ));
    assert_eq!(
        requests.lock().unwrap().len(),
        2,
        "a private or removed video isn't asked about again"
    );

    let Err(Error::AllClientsFailed(summary)) = extractor.video("brokenvid11").await else {
        panic!("expected every client to fail");
    };
    assert!(
        summary.contains("android_vr: ") && summary.contains("500"),
        "{summary}"
    );
    assert!(summary.contains("ios: "), "{summary}");
}

#[tokio::test]
async fn other_addresses_are_refused_without_asking() {
    let (extractor, requests) = start().await;
    assert!(matches!(
        extractor
            .video_at("https://example.com/watch?v=okokokokok1")
            .await,
        Err(Error::NotYouTube(_))
    ));
    let only_ios = extractor.with_clients(vec![Client::IOS]);
    assert_eq!(only_ios.video("okokokokok1").await.unwrap().client, "ios");
    assert_eq!(requests.lock().unwrap().len(), 1);
}

#[tokio::test]
async fn web_addresses_are_unscrambled_with_the_player_and_the_player_is_cached() {
    let (extractor, requests) = start().await;
    let cache = std::env::temp_dir().join(format!("velo-extract-test-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&cache);
    std::fs::create_dir_all(&cache).unwrap();
    for old in ["old1.js", "old2.js", "old3.js"] {
        std::fs::write(cache.join(old), "// an older player").unwrap();
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
    let web = extractor
        .with_cache_dir(cache.clone())
        .with_clients(vec![Client::WEB]);
    let before = PLAYER_DOWNLOADS.load(Ordering::SeqCst);

    let video = web.video("webwebweb01").await.unwrap();
    assert_eq!(video.client, "web");
    let urls: Vec<&str> = video
        .formats
        .iter()
        .map(|f| f.url.as_deref().unwrap())
        .collect();
    // A replaced parameter moves to the end of the query.
    assert_eq!(
        urls,
        [
            "https://media.test/videoplayback?itag=18&id=webwebweb01&n=hgfedcba21ji",
            "https://media.test/videoplayback?itag=140&id=webwebweb01&sig=ewvutsrqponmlkjihgfxdcbaZYXWVUTSRQPONMLKJIHGFEDCBA9876540213&n=stuvwxyzopqr",
        ]
    );
    assert!(video.formats.iter().all(|f| !f.needs_deciphering()));
    {
        let requests = requests.lock().unwrap();
        let (_, body) = requests.last().unwrap();
        assert_eq!(
            body["playbackContext"]["contentPlaybackContext"]["signatureTimestamp"],
            20160
        );
    }

    // The script is downloaded once, kept with the two newest older ones, and read from the
    // cache by a new extractor.
    web.video("webwebweb01").await.unwrap();
    assert_eq!(PLAYER_DOWNLOADS.load(Ordering::SeqCst) - before, 1);
    let mut cached: Vec<String> = std::fs::read_dir(&cache)
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    cached.sort();
    assert_eq!(cached, ["a1b2c3d4.js", "old2.js", "old3.js"]);
    let (fresh, _) = start().await;
    fresh
        .with_cache_dir(cache.clone())
        .with_clients(vec![Client::WEB])
        .video("webwebweb01")
        .await
        .unwrap();
    assert_eq!(PLAYER_DOWNLOADS.load(Ordering::SeqCst) - before, 1);
    std::fs::remove_dir_all(&cache).unwrap();
}

#[tokio::test]
async fn the_extractor_resolves_sources_to_current_addresses() {
    use velo_core::{Resolver, Source};
    let (extractor, _) = start().await;
    let source = |itags: Vec<u32>| Source::YouTube {
        video_id: "okokokokok1".into(),
        itags,
    };
    assert_eq!(
        extractor.resolve(&source(vec![137, 140])).await.unwrap(),
        [
            "https://media.test/videoplayback?itag=137&id=okokokokok1",
            "https://media.test/videoplayback?itag=140&id=okokokokok1"
        ]
    );
    let missing = extractor.resolve(&source(vec![22])).await.unwrap_err();
    assert!(missing.contains("22"), "{missing}");
}
