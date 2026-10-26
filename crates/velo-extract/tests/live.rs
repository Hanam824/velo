//! A check against the real YouTube, which changes without notice. It is ignored by default,
//! so normal test runs never touch the internet; the nightly smoke workflow runs it with
//! `VELO_LIVE_YOUTUBE=1 cargo test -p velo-extract --test live -- --ignored`.

use velo_extract::{Client, Extractor, StreamKind};

/// "Me at the zoo", the first video on YouTube, which is unlikely to go away.
const VIDEO: &str = "https://www.youtube.com/watch?v=jNQXAC9IVRw";

#[tokio::test]
#[ignore = "talks to YouTube; set VELO_LIVE_YOUTUBE=1 and run with --ignored"]
async fn a_real_video_has_streams() {
    if std::env::var_os("VELO_LIVE_YOUTUBE").is_none() {
        eprintln!("skipped: VELO_LIVE_YOUTUBE isn't set");
        return;
    }
    let extractor = Extractor::new(velo_engine::client().unwrap());
    // Which clients work today, for the log; only one of them needs to.
    for client in Client::DEFAULT_ORDER {
        let alone = extractor.clone().with_clients(vec![client]);
        match alone.video_at(VIDEO).await {
            Ok(video) => eprintln!("{}: {} formats", client.name, video.formats.len()),
            Err(error) => eprintln!("{}: {error}", client.name),
        }
    }
    let video = extractor.video_at(VIDEO).await.unwrap();
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
}
