//! `velo-cli`: adds downloads to a running Velo app, or runs them headless.
//!
//! With the app running, the command is sent over [`velo_ipc`] and `velo-cli` exits at once.
//! Without it (or with `--headless`), `velo-cli` runs the engine itself on the app's download
//! list and settings, shows progress on stderr and exits when the work is done, so the
//! download also appears in the app's list later.

use std::io::Write;
use std::path::PathBuf;
use std::sync::Arc;

use clap::{ArgGroup, Parser};
use tokio::sync::broadcast::error::RecvError;
use velo_core::{DownloadState, EngineEvent, QueueId, format};
use velo_engine::{Engine, EngineConfig, NewDownload, RequestOptions, RetryPolicy};
use velo_extract::Extractor;
use velo_ipc::{Endpoint, Message};
use velo_store::Store;

/// Errors that end `velo-cli` with a non-zero exit code.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// `--headless` was asked for while the app runs, which owns the download list.
    #[error("Velo is running; leave out --headless to hand the download to it")]
    AppRunning,
    /// A download failed.
    #[error("the download failed: {0}")]
    Failed(String),
    /// The user pressed Ctrl+C; what was downloaded is kept for a resume.
    #[error("interrupted; the download is paused and can be resumed")]
    Interrupted,
    /// The app refused the request, or the socket failed.
    #[error(transparent)]
    Ipc(#[from] velo_ipc::Error),
    /// YouTube didn't give the video's streams.
    #[error(transparent)]
    YouTube(#[from] velo_extract::Error),
    /// The asked-for format isn't offered.
    #[error("YouTube doesn't offer that format; it offers: {0}")]
    NoFormat(String),
    /// The engine couldn't start.
    #[error(transparent)]
    Engine(#[from] velo_engine::Error),
    /// The download list or settings couldn't be read.
    #[error(transparent)]
    Store(#[from] velo_store::Error),
    /// A file or the runtime failed.
    #[error(transparent)]
    Io(#[from] std::io::Error),
}

/// Result with [`Error`] as the default error type.
pub type Result<T, E = Error> = std::result::Result<T, E>;

/// The command line.
#[derive(Debug, Clone, PartialEq, Eq, Parser)]
#[command(
    name = "velo-cli",
    version,
    about = "Adds downloads to Velo Download Manager"
)]
#[command(group(ArgGroup::new("action").required(true).multiple(true).args(["download", "start_queue"])))]
pub struct Cli {
    /// Download URL.
    #[arg(short, long, value_name = "URL")]
    pub download: Option<String>,
    /// Save into DIR instead of the folder for the file's category.
    #[arg(short, long, value_name = "DIR", requires = "download")]
    pub path: Option<PathBuf>,
    /// Save as NAME instead of the server's file name.
    #[arg(short, long, value_name = "NAME", requires = "download")]
    pub file: Option<String>,
    /// Add without asking: skip the Download File Info dialog.
    #[arg(short, long, requires = "download")]
    pub no_questions: bool,
    /// Add to the main queue without starting it.
    #[arg(short, long, requires = "download")]
    pub add_only: bool,
    /// For a YouTube address: the format (itag) to download; the best video with sound if not
    /// given.
    #[arg(long, value_name = "ITAG", requires = "download")]
    pub itag: Option<u32>,
    /// Start the main queue.
    #[arg(short, long)]
    pub start_queue: bool,
    /// Download here and exit when done, without the app.
    #[arg(short = 'q', long, visible_alias = "quit-when-done")]
    pub headless: bool,
}

/// Where the app keeps its files; [`Places::system`] gives the real ones.
#[derive(Debug, Clone)]
pub struct Places {
    /// The app's data folder, which also names its socket.
    pub data: PathBuf,
    /// The download list.
    pub database: PathBuf,
    /// The settings file.
    pub settings: PathBuf,
    /// Part files when the settings don't name a folder.
    pub parts: PathBuf,
    /// Finished files when the settings don't name a folder.
    pub downloads: PathBuf,
}

impl Places {
    /// The folders the app uses on this system.
    pub fn system() -> Result<Self, velo_platform::Error> {
        let dirs = velo_platform::AppDirs::new()?;
        Ok(Self {
            data: dirs.data().to_path_buf(),
            database: dirs.database(),
            settings: dirs.settings(),
            parts: dirs.parts().to_path_buf(),
            downloads: dirs.downloads().to_path_buf(),
        })
    }
}

/// Carries out `cli`: hands it to the running app, or else does it here.
pub fn run(cli: &Cli, places: &Places) -> Result<()> {
    let endpoint = Endpoint::for_data_dir(&places.data);
    if cli.headless {
        if velo_ipc::is_running(&endpoint) {
            return Err(Error::AppRunning);
        }
    } else {
        match forward(cli, &endpoint) {
            Err(Error::Ipc(velo_ipc::Error::NotRunning)) => {
                eprintln!("velo-cli: Velo isn't running; working here instead");
            }
            other => return other,
        }
    }
    tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()?
        .block_on(headless(cli, places))
}

/// Sends `cli` to the app at `endpoint`.
fn forward(cli: &Cli, endpoint: &Endpoint) -> Result<()> {
    if let Some(url) = &cli.download {
        velo_ipc::send(
            endpoint,
            &Message::Add {
                url: url.clone(),
                // The app runs in another folder, so a relative path must be made absolute here.
                save_dir: cli.path.as_deref().map(std::path::absolute).transpose()?,
                file_name: cli.file.clone(),
                start: !cli.add_only,
                silent: cli.no_questions,
                itag: cli.itag,
            },
        )?;
    }
    if cli.start_queue {
        velo_ipc::send(endpoint, &Message::StartQueue { queue: None })?;
    }
    Ok(())
}

/// Does `cli` with an engine of its own on the app's download list.
async fn headless(cli: &Cli, places: &Places) -> Result<()> {
    let settings = velo_store::load_settings(&places.settings)?;
    let mut store = Store::open(&places.database)?;
    let extractor = Extractor::new(velo_engine::client_with(
        &settings,
        &RequestOptions::default(),
    )?)
    .with_cache_dir(places.data.join("player-cache"));
    let engine = Engine::start(
        EngineConfig {
            temp_dir: settings
                .temp_dir
                .clone()
                .unwrap_or_else(|| places.parts.clone()),
            settings: settings.clone(),
            retry: RetryPolicy::default(),
            resolver: Some(Arc::new(extractor.clone())),
        },
        store.load_downloads()?,
        store.load_queues()?,
    )?;
    let mut events = engine.subscribe();
    // Saves the main queue if the list has none yet, as the app does when it starts.
    engine.resync();

    let mut waiting = None;
    if let Some(url) = &cli.download {
        let video = match velo_extract::video_id(url) {
            Some(_) => Some(choose_format(&extractor, url, cli.itag).await?),
            None => None,
        };
        let file_name = match &video {
            Some((video, itags)) => {
                let choice = video
                    .choice(itags)
                    .expect("the itags were chosen from the video");
                Some(cli.file.clone().unwrap_or_else(|| video.file_name(&choice)))
            }
            None => cli.file.clone(),
        };
        let (save_dir, category) = match &cli.path {
            Some(dir) => (std::path::absolute(dir)?, None),
            None => {
                let name = match &file_name {
                    Some(name) => name.clone(),
                    None => server_name(&settings, url).await,
                };
                let category = velo_core::Category::for_file_name(&settings.categories, &name)
                    .map(|c| c.name.clone());
                let dir = settings.save_dir_for(category.as_deref(), &places.downloads);
                (dir, category)
            }
        };
        let (address, description, source) = match &video {
            Some((video, itags)) => (
                video
                    .format(itags[0])
                    .and_then(|f| f.url.clone())
                    .unwrap_or_default(),
                video.title.clone(),
                Some(velo_core::Source::YouTube {
                    video_id: video.id.clone(),
                    itags: itags.clone(),
                }),
            ),
            None => (url.clone(), String::new(), None),
        };
        let id = engine.add(NewDownload {
            url: address,
            file_name: file_name.unwrap_or_default(),
            save_dir,
            category,
            description,
            start: !cli.add_only,
            request: RequestOptions::default(),
            source,
        });
        if cli.add_only {
            engine.enqueue(QueueId::MAIN, vec![id]);
            eprintln!("velo-cli: added {url} to the main queue");
        } else {
            waiting = Some(id);
        }
    }
    let mut queue_running = cli.start_queue;
    let mut queue_started = false;
    if cli.start_queue {
        engine.start_queue(QueueId::MAIN);
    }

    let mut outcome = Ok(());
    let interrupt = tokio::signal::ctrl_c();
    tokio::pin!(interrupt);
    while waiting.is_some() || queue_running {
        let event = tokio::select! {
            event = events.recv() => event,
            _ = &mut interrupt => {
                outcome = Err(Error::Interrupted);
                break;
            }
        };
        let event = match event {
            Ok(event) => event,
            Err(RecvError::Lagged(_)) => {
                engine.resync();
                continue;
            }
            Err(RecvError::Closed) => break,
        };
        save(&mut store, &event);
        match &event {
            EngineEvent::Progress(p) if Some(p.id) == waiting => show_progress(p),
            EngineEvent::Changed(d) if Some(d.id) == waiting => match &d.state {
                DownloadState::Complete => {
                    eprintln!();
                    println!("{}", d.save_path().display());
                    waiting = None;
                }
                DownloadState::Failed(reason) => {
                    eprintln!();
                    outcome = Err(Error::Failed(reason.clone()));
                    waiting = None;
                }
                _ => {}
            },
            // The resync at the start also reports the queue as stopped, so only a stop after
            // it ran counts.
            EngineEvent::QueueRunning(QueueId::MAIN, true) => queue_started = true,
            EngineEvent::QueueFinished(QueueId::MAIN) => queue_running = false,
            EngineEvent::QueueRunning(QueueId::MAIN, false) if queue_started => {
                queue_running = false;
            }
            _ => {}
        }
        if outcome.is_err() {
            break;
        }
    }

    // Pause what still runs and save its final state.
    engine.shutdown().await;
    while let Ok(event) = events.try_recv() {
        save(&mut store, &event);
    }
    outcome
}

/// The video at YouTube address `url` and the itags to download: format `itag`, or else the
/// best choice (possibly a video and its sound, joined after downloading).
async fn choose_format(
    extractor: &Extractor,
    url: &str,
    itag: Option<u32>,
) -> Result<(velo_extract::Video, Vec<u32>)> {
    let video = extractor.video_at(url).await?;
    let itags = match itag {
        Some(itag) => video.choice(&[itag]).map(|c| c.itags()),
        None => video.choices().first().map(|c| c.itags()),
    }
    .ok_or_else(|| {
        let offered: Vec<String> = video.formats.iter().map(|f| f.itag.to_string()).collect();
        Error::NoFormat(offered.join(", "))
    })?;
    Ok((video, itags))
}

/// The file name the server gives `url`, or an empty name if it can't be asked; the engine
/// then asks again when it starts and reports the error.
async fn server_name(settings: &velo_core::Settings, url: &str) -> String {
    let Ok(client) = velo_engine::client_with(settings, &RequestOptions::default()) else {
        return String::new();
    };
    velo_engine::probe(&client, url)
        .await
        .map(|probe| probe.file_name)
        .unwrap_or_default()
}

/// Writes one event to the download list; a failed write is reported and the next one tries
/// again.
fn save(store: &mut Store, event: &EngineEvent) {
    if let Err(error) = store.apply(event) {
        eprintln!("velo-cli: couldn't save the download list: {error}");
    }
}

/// Rewrites the progress line on stderr.
fn show_progress(update: &velo_core::ProgressUpdate) {
    let done = match update.total {
        Some(total) => format!(
            "{} of {} ({})",
            format::size(update.downloaded),
            format::size(total),
            format::percent(update.downloaded, total)
        ),
        None => format::size(update.downloaded),
    };
    let mut stderr = std::io::stderr();
    let _ = write!(stderr, "\r\x1b[2K{done}  {}", format::rate(update.rate));
    let _ = stderr.flush();
}
