//! The extractor against a local stand-in for YouTube's player API, with synthetic answers.

use std::sync::{Arc, Mutex};

use axum::Router;
use axum::extract::State;
use axum::http::{HeaderMap, StatusCode};
use axum::routing::post;
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
        _ => "not json",
    })
}

/// Starts the stand-in and returns an extractor pointed at it and what it receives.
async fn start() -> (Extractor, Requests) {
    let requests = Requests::default();
    let app = Router::new()
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
