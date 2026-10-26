//! The engine: a task that owns every download, runs them on request and reports what happens.
//!
//! The app talks to it through an [`Engine`] handle, which only sends commands and never waits,
//! so it can be used from the UI thread. Everything that happens comes back as
//! [`EngineEvent`]s on a broadcast channel: the app's list and its store both listen there.

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use reqwest::Client;
use tokio::sync::{broadcast, mpsc, oneshot};
use tokio::time::MissedTickBehavior;
use tokio_util::sync::CancellationToken;
use velo_core::{Download, DownloadId, DownloadState, EngineEvent, ProgressUpdate};

use crate::download::{RunContext, Stage, remove_dir, run};
use crate::{Error, Options, Progress, Result};

/// How often running downloads report [`EngineEvent::Progress`].
const PROGRESS_INTERVAL: Duration = Duration::from_millis(250);

/// How many progress intervals pass between [`EngineEvent::Changed`] events that save a running
/// download's segments (every 2 seconds).
const SAVE_EVERY: u64 = 8;

/// What the engine needs to run downloads.
#[derive(Debug, Clone)]
pub struct EngineConfig {
    /// The shared HTTP client, from [`client`](crate::client).
    pub client: Client,
    /// Folder holding each download's part-file folder.
    pub temp_dir: PathBuf,
    /// Connection and retry settings for downloads started from now on.
    pub options: Options,
}

/// A download to add to the engine.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NewDownload {
    /// The URL to fetch.
    pub url: String,
    /// File name to save as; empty to take the server's.
    pub file_name: String,
    /// Folder to save into.
    pub save_dir: PathBuf,
    /// Category the download is filed under.
    pub category: Option<String>,
    /// Free-text description.
    pub description: String,
    /// Whether to start it right away; otherwise it waits as queued.
    pub start: bool,
}

/// A handle to the engine task. Cloning it gives another handle to the same engine.
///
/// Every method returns at once; results arrive as [`EngineEvent`]s from
/// [`Engine::subscribe`]. Commands for a download that doesn't exist are ignored.
#[derive(Debug, Clone)]
pub struct Engine {
    commands: mpsc::UnboundedSender<Command>,
    events: broadcast::Sender<EngineEvent>,
    next_id: Arc<AtomicU64>,
}

impl Engine {
    /// Starts the engine task on the current tokio runtime with `downloads` restored from
    /// storage.
    ///
    /// Downloads that were running when the app last stopped come back paused.
    ///
    /// # Panics
    /// If called outside a tokio runtime.
    pub fn start(config: EngineConfig, downloads: Vec<Download>) -> Self {
        let next_id = downloads.iter().map(|d| d.id.0 + 1).max().unwrap_or(1);
        let (commands, command_rx) = mpsc::unbounded_channel();
        let (events, _) = broadcast::channel(1024);
        let (task_tx, task_rx) = mpsc::unbounded_channel();
        let actor = Actor {
            config,
            downloads: downloads
                .into_iter()
                .map(|mut download| {
                    if download.is_running() {
                        download.state = DownloadState::Paused;
                    }
                    (download.id, Entry::new(download))
                })
                .collect(),
            events: events.clone(),
            task_tx,
            shutdown: Vec::new(),
        };
        tokio::spawn(actor.run(command_rx, task_rx));
        Self {
            commands,
            events,
            next_id: Arc::new(AtomicU64::new(next_id)),
        }
    }

    /// Returns a receiver for everything that happens from now on.
    ///
    /// A receiver that falls more than 1024 events behind skips the oldest ones; call
    /// [`Engine::resync`] to get every download again.
    pub fn subscribe(&self) -> broadcast::Receiver<EngineEvent> {
        self.events.subscribe()
    }

    /// Adds a download and returns its id.
    pub fn add(&self, new: NewDownload) -> DownloadId {
        let id = DownloadId(self.next_id.fetch_add(1, Ordering::Relaxed));
        let mut download = Download::new(id, new.url, new.file_name, new.save_dir);
        download.category = new.category;
        download.description = new.description;
        download.added_at = Some(now());
        self.send(Command::Add(Box::new(download), new.start));
        id
    }

    /// Starts or resumes a download that is queued, paused or failed.
    pub fn resume(&self, id: DownloadId) {
        self.send(Command::Resume(id));
    }

    /// Pauses a running download; its part files stay for a later resume.
    pub fn pause(&self, id: DownloadId) {
        self.send(Command::Pause(id));
    }

    /// Pauses every running download.
    pub fn pause_all(&self) {
        self.send(Command::PauseAll);
    }

    /// Deletes a download and its part files, and with `delete_file` also its finished file.
    pub fn delete(&self, id: DownloadId, delete_file: bool) {
        self.send(Command::Delete { id, delete_file });
    }

    /// Uses `options` for downloads started from now on.
    pub fn set_options(&self, options: Options) {
        self.send(Command::SetOptions(options));
    }

    /// Sends [`EngineEvent::Changed`] for every download.
    pub fn resync(&self) {
        self.send(Command::Resync);
    }

    /// Pauses every running download and waits until they have stopped and their final
    /// [`EngineEvent::Changed`] events are sent; then the engine task ends.
    pub async fn shutdown(&self) {
        let (reply, done) = oneshot::channel();
        self.send(Command::Shutdown(reply));
        let _ = done.await;
    }

    /// Sends a command; a stopped engine ignores it.
    fn send(&self, command: Command) {
        let _ = self.commands.send(command);
    }
}

/// A request from an [`Engine`] handle.
#[derive(Debug)]
enum Command {
    Add(Box<Download>, bool),
    Resume(DownloadId),
    Pause(DownloadId),
    PauseAll,
    Delete { id: DownloadId, delete_file: bool },
    SetOptions(Options),
    Resync,
    Shutdown(oneshot::Sender<()>),
}

/// A report from a download's task.
enum TaskMessage {
    /// The run reached `stage`; `download` is its state at that point.
    Stage(Stage, Download),
    /// The run ended.
    Done(Download, Result<PathBuf>),
}

/// The engine task's state.
struct Actor {
    config: EngineConfig,
    downloads: BTreeMap<DownloadId, Entry>,
    events: broadcast::Sender<EngineEvent>,
    task_tx: mpsc::UnboundedSender<TaskMessage>,
    /// Waiting [`Engine::shutdown`] calls; once set, the engine ends when nothing runs.
    shutdown: Vec<oneshot::Sender<()>>,
}

/// A download and, while it runs, its task.
struct Entry {
    download: Download,
    running: Option<Running>,
    /// Set when the download was deleted while running: delete once the task has stopped,
    /// along with the finished file if `true`.
    delete_when_stopped: Option<bool>,
}

impl Entry {
    fn new(download: Download) -> Self {
        Self {
            download,
            running: None,
            delete_when_stopped: None,
        }
    }
}

/// A running download's task handles.
struct Running {
    cancel: CancellationToken,
    progress: Progress,
    rate: RateMeter,
}

impl Actor {
    /// Handles commands, task reports and the progress clock until shut down or every handle
    /// is dropped.
    async fn run(
        mut self,
        mut commands: mpsc::UnboundedReceiver<Command>,
        mut tasks: mpsc::UnboundedReceiver<TaskMessage>,
    ) {
        let mut clock = tokio::time::interval(PROGRESS_INTERVAL);
        clock.set_missed_tick_behavior(MissedTickBehavior::Skip);
        let mut ticks = 0u64;
        let mut handles_open = true;
        loop {
            tokio::select! {
                command = commands.recv(), if handles_open => match command {
                    Some(command) => self.handle(command),
                    None => {
                        // Every handle is gone: stop the downloads and wait for them.
                        handles_open = false;
                        self.handle(Command::PauseAll);
                    }
                },
                Some(message) = tasks.recv() => self.on_task(message),
                _ = clock.tick() => {
                    ticks += 1;
                    self.report_progress(ticks.is_multiple_of(SAVE_EVERY));
                }
            }
            let idle = self.downloads.values().all(|e| e.running.is_none());
            if idle && (!self.shutdown.is_empty() || !handles_open) {
                for reply in self.shutdown.drain(..) {
                    let _ = reply.send(());
                }
                return;
            }
        }
    }

    fn handle(&mut self, command: Command) {
        match command {
            Command::Add(download, start) => {
                let download = *download;
                let id = download.id;
                self.emit(EngineEvent::Added(download.clone()));
                self.downloads.insert(id, Entry::new(download));
                if start {
                    self.start(id);
                }
            }
            Command::Resume(id) => self.start(id),
            Command::Pause(id) => {
                if let Some(running) = self.downloads.get(&id).and_then(|e| e.running.as_ref()) {
                    running.cancel.cancel();
                }
            }
            Command::PauseAll => {
                for running in self.downloads.values().filter_map(|e| e.running.as_ref()) {
                    running.cancel.cancel();
                }
            }
            Command::Delete { id, delete_file } => {
                let Some(entry) = self.downloads.get_mut(&id) else {
                    return;
                };
                match &entry.running {
                    Some(running) => {
                        running.cancel.cancel();
                        entry.delete_when_stopped = Some(delete_file);
                    }
                    None => self.remove(id, delete_file),
                }
            }
            Command::SetOptions(options) => self.config.options = options,
            Command::Resync => {
                let all: Vec<_> = self
                    .downloads
                    .values()
                    .map(|e| e.download.clone())
                    .collect();
                for download in all {
                    self.emit(EngineEvent::Changed(download));
                }
            }
            Command::Shutdown(reply) => {
                self.shutdown.push(reply);
                self.handle(Command::PauseAll);
            }
        }
    }

    /// Starts a task for download `id`, unless it is running, complete or the engine is
    /// shutting down.
    fn start(&mut self, id: DownloadId) {
        if !self.shutdown.is_empty() {
            return;
        }
        let Some(entry) = self.downloads.get_mut(&id) else {
            return;
        };
        if entry.running.is_some() || entry.download.state == DownloadState::Complete {
            return;
        }
        entry.download.state = DownloadState::Probing;
        entry.download.last_try_at = Some(now());
        let running = Running {
            cancel: CancellationToken::new(),
            progress: Progress::new(entry.download.segments.clone()),
            rate: RateMeter::default(),
        };
        let mut download = entry.download.clone();
        let (client, temp_dir) = (self.config.client.clone(), self.config.temp_dir.clone());
        let (options, progress) = (self.config.options.clone(), running.progress.clone());
        let (cancel, tx) = (running.cancel.clone(), self.task_tx.clone());
        entry.running = Some(running);
        let snapshot = entry.download.clone();
        self.emit(EngineEvent::Changed(snapshot));

        tokio::spawn(async move {
            let context = RunContext {
                client: &client,
                temp_dir: &temp_dir,
                options: &options,
                progress: &progress,
                cancel: &cancel,
            };
            let result = run(&context, &mut download, |stage, d| {
                let _ = tx.send(TaskMessage::Stage(stage, d.clone()));
            })
            .await;
            let _ = tx.send(TaskMessage::Done(download, result));
        });
    }

    fn on_task(&mut self, message: TaskMessage) {
        let (download, state) = match message {
            TaskMessage::Stage(Stage::Probed, download) => (download, DownloadState::Downloading),
            TaskMessage::Stage(Stage::Merging, download) => (download, DownloadState::Merging),
            TaskMessage::Done(download, result) => {
                let state = match result {
                    Ok(_) => DownloadState::Complete,
                    Err(Error::Cancelled) => DownloadState::Paused,
                    Err(error) => DownloadState::Failed(error.to_string()),
                };
                (download, state)
            }
        };
        let id = download.id;
        let Some(entry) = self.downloads.get_mut(&id) else {
            return;
        };
        let stopped = !matches!(state, DownloadState::Downloading | DownloadState::Merging);
        entry.download = Download { state, ..download };
        if stopped {
            entry.running = None;
            if let Some(delete_file) = entry.delete_when_stopped {
                self.remove(id, delete_file);
                return;
            }
        }
        let snapshot = entry.download.clone();
        self.emit(EngineEvent::Changed(snapshot));
    }

    /// Sends progress for every running download, and with `save` also a
    /// [`EngineEvent::Changed`] with its current segments.
    fn report_progress(&mut self, save: bool) {
        let mut events = Vec::new();
        let now = Instant::now();
        for entry in self.downloads.values_mut() {
            let Some(running) = &mut entry.running else {
                continue;
            };
            let segments = running.progress.snapshot();
            let downloaded = segments.iter().map(|s| s.downloaded).sum();
            events.push(EngineEvent::Progress(ProgressUpdate {
                id: entry.download.id,
                downloaded,
                total: entry.download.total_size,
                rate: running.rate.update(now, downloaded),
                segments: segments.clone(),
            }));
            if save && entry.download.state == DownloadState::Downloading {
                entry.download.segments = segments;
                events.push(EngineEvent::Changed(entry.download.clone()));
            }
        }
        for event in events {
            self.emit(event);
        }
    }

    /// Forgets download `id` and deletes its part files, and with `delete_file` its finished
    /// file, in the background.
    fn remove(&mut self, id: DownloadId, delete_file: bool) {
        let Some(entry) = self.downloads.remove(&id) else {
            return;
        };
        let parts = self.config.temp_dir.join(id.0.to_string());
        let file =
            (delete_file && entry.download.is_finished()).then(|| entry.download.save_path());
        tokio::spawn(async move {
            let _ = remove_dir(&parts).await;
            if let Some(file) = file {
                let _ = tokio::fs::remove_file(file).await;
            }
        });
        self.emit(EngineEvent::Removed(id));
    }

    /// Sends `event` to every subscriber; having none is fine.
    fn emit(&self, event: EngineEvent) {
        let _ = self.events.send(event);
    }
}

/// Smooths a download's transfer rate over the last few seconds.
#[derive(Debug, Default)]
struct RateMeter {
    last: Option<(Instant, u64)>,
    rate: f64,
}

impl RateMeter {
    /// Records that `downloaded` bytes are in at `now` and returns the smoothed rate in bytes
    /// per second.
    fn update(&mut self, now: Instant, downloaded: u64) -> u64 {
        if let Some((then, before)) = self.last {
            let seconds = now.duration_since(then).as_secs_f64();
            if seconds > 0.0 {
                let instant = downloaded.saturating_sub(before) as f64 / seconds;
                self.rate = if self.rate == 0.0 {
                    instant
                } else {
                    self.rate * 0.7 + instant * 0.3
                };
            }
        }
        self.last = Some((now, downloaded));
        self.rate.round() as u64
    }
}

/// The current time in seconds since the Unix epoch.
fn now() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_secs() as i64)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_rate_follows_the_bytes_per_second() {
        let mut meter = RateMeter::default();
        let start = Instant::now();
        assert_eq!(meter.update(start, 0), 0);
        assert_eq!(meter.update(start + Duration::from_secs(1), 1000), 1000);
        // A faster second moves the rate part of the way.
        assert_eq!(meter.update(start + Duration::from_secs(2), 3000), 1300);
    }
}
