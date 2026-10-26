//! `velo-cli` against a stand-in app and a local HTTP server, never the internet.

use std::net::SocketAddr;
use std::path::Path;
use std::sync::mpsc;
use std::time::Duration;

use axum::Router;
use axum::routing::get;
use clap::Parser;
use velo_cli::{Cli, Error, Places, run};
use velo_core::{DownloadState, QueueId};
use velo_ipc::{Endpoint, Message};
use velo_store::Store;

/// The body of every file the server sends.
const BODY: &[u8] = b"velo-cli test file contents\n";

/// The app's folders, all inside `root`.
fn places(root: &Path) -> Places {
    Places {
        data: root.join("data"),
        database: root.join("data/velo.db"),
        settings: root.join("data/settings.toml"),
        parts: root.join("parts"),
        downloads: root.join("downloads"),
    }
}

/// Serves [`BODY`] at every path on a thread of its own and returns its address.
fn serve() -> SocketAddr {
    let (tx, rx) = mpsc::channel();
    std::thread::spawn(move || {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        runtime.block_on(async move {
            let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
            tx.send(listener.local_addr().unwrap()).unwrap();
            let app = Router::new().route("/{name}", get(|| async { BODY }));
            axum::serve(listener, app).await.unwrap();
        });
    });
    rx.recv().unwrap()
}

fn parse(args: &[&str]) -> Result<Cli, clap::Error> {
    Cli::try_parse_from(std::iter::once("velo-cli").chain(args.iter().copied()))
}

#[test]
fn the_command_line_needs_something_to_do() {
    let cli = parse(&[
        "-d",
        "https://a.test/x.zip",
        "-p",
        "dir",
        "-f",
        "y.zip",
        "-n",
        "-a",
    ])
    .unwrap();
    assert_eq!(cli.download.as_deref(), Some("https://a.test/x.zip"));
    assert_eq!(cli.path.as_deref(), Some(Path::new("dir")));
    assert_eq!(cli.file.as_deref(), Some("y.zip"));
    assert!(cli.no_questions && cli.add_only && !cli.start_queue && !cli.headless);
    assert!(parse(&["-s", "-q"]).unwrap().headless);
    assert!(parse(&["--quit-when-done", "-s"]).unwrap().headless);

    assert!(parse(&[]).is_err(), "neither a download nor a queue");
    assert!(parse(&["-q"]).is_err());
    assert!(
        parse(&["-s", "-p", "dir"]).is_err(),
        "a folder without a download"
    );
}

#[test]
fn headless_downloads_into_the_category_folder_and_records_it_in_the_list() {
    let root = tempfile::tempdir().unwrap();
    let places = places(root.path());
    let address = serve();
    let cli = parse(&["-d", &format!("http://{address}/notes.txt"), "-q"]).unwrap();
    run(&cli, &places).unwrap();

    let saved = places.downloads.join("notes.txt");
    assert_eq!(std::fs::read(&saved).unwrap(), BODY);
    let downloads = Store::open(&places.database)
        .unwrap()
        .load_downloads()
        .unwrap();
    assert_eq!(downloads.len(), 1);
    assert_eq!(downloads[0].state, DownloadState::Complete);
    assert_eq!(downloads[0].category.as_deref(), Some("Documents"));
}

#[test]
fn headless_add_only_queues_without_downloading() {
    let root = tempfile::tempdir().unwrap();
    let places = places(root.path());
    let address = serve();
    let cli = parse(&["-d", &format!("http://{address}/later.bin"), "-a", "-q"]).unwrap();
    run(&cli, &places).unwrap();

    let store = Store::open(&places.database).unwrap();
    let downloads = store.load_downloads().unwrap();
    assert_eq!(downloads.len(), 1);
    assert_eq!(downloads[0].state, DownloadState::Queued);
    let main = store
        .load_queues()
        .unwrap()
        .into_iter()
        .find(|q| q.id == QueueId::MAIN)
        .unwrap();
    assert_eq!(main.downloads, [downloads[0].id]);
    assert!(!places.downloads.join("later.bin").exists());

    // Starting the main queue later downloads it.
    run(&parse(&["-s", "-q"]).unwrap(), &places).unwrap();
    assert_eq!(
        std::fs::read(places.downloads.join("later.bin")).unwrap(),
        BODY
    );
}

#[test]
fn a_failed_download_is_an_error() {
    let root = tempfile::tempdir().unwrap();
    // Nothing listens on port 1.
    let cli = parse(&["-d", "http://127.0.0.1:1/x.bin", "-p", "out", "-q"]).unwrap();
    assert!(matches!(
        run(&cli, &places(root.path())),
        Err(Error::Failed(_))
    ));
}

#[test]
fn a_running_app_gets_the_request_and_headless_is_refused() {
    let root = tempfile::tempdir().unwrap();
    let places = places(root.path());
    let (tx, rx) = mpsc::channel();
    velo_ipc::listen(&Endpoint::for_data_dir(&places.data), move |message| {
        let _ = tx.send(message);
        Ok(())
    })
    .unwrap();

    let cli = parse(&[
        "-d",
        "https://a.test/x.zip",
        "-p",
        "rel",
        "-n",
        "-s",
        "--itag",
        "22",
    ])
    .unwrap();
    run(&cli, &places).unwrap();
    let received = rx.recv_timeout(Duration::from_secs(5)).unwrap();
    let Message::Add {
        url,
        save_dir,
        file_name,
        start,
        silent,
        itag,
    } = received
    else {
        panic!("expected an add, got {received:?}");
    };
    assert_eq!(url, "https://a.test/x.zip");
    let save_dir = save_dir.unwrap();
    assert!(save_dir.is_absolute() && save_dir.ends_with("rel"));
    assert_eq!(file_name, None);
    assert!(start && silent);
    assert_eq!(itag, Some(22));
    assert_eq!(
        rx.recv_timeout(Duration::from_secs(5)).unwrap(),
        Message::StartQueue { queue: None }
    );

    assert!(matches!(
        run(&parse(&["-s", "-q"]).unwrap(), &places),
        Err(Error::AppRunning)
    ));
}
