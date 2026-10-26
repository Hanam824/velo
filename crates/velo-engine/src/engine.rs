//! The engine: a task that owns every download, runs them on request and reports what happens.
//!
//! The app talks to it through an [`Engine`] handle, which only sends commands and never waits,
//! so it can be used from the UI thread. Everything that happens comes back as
//! [`EngineEvent`]s on a broadcast channel: the app's list and its store both listen there.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicU32, AtomicU64, Ordering};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use chrono::NaiveDateTime;
use reqwest::Client;
use tokio::sync::{broadcast, mpsc, oneshot};
use tokio::time::MissedTickBehavior;
use tokio_util::sync::CancellationToken;
use velo_core::{
    Download, DownloadId, DownloadState, EngineEvent, ProgressUpdate, Queue, QueueId, Resolver,
    Source,
};

use crate::download::{RunContext, Stage, remove_dir, run};
use crate::scheduler;
use velo_core::Settings;

use crate::{Error, Limiter, Options, Progress, RequestOptions, Result, client_with};

/// Gives `download` its current address from its source, if it has one.
async fn refresh(
    download: &mut Download,
    resolver: Option<&dyn Resolver>,
    cancel: &CancellationToken,
) -> Result<()> {
    let Some(source) = download.source.clone() else {
        return Ok(());
    };
    let resolver =
        resolver.ok_or_else(|| Error::Resolve("this download needs YouTube support".into()))?;
    let urls = crate::fetch::cancellable(cancel, async {
        resolver.resolve(&source).await.map_err(Error::Resolve)
    })
    .await?;
    download.url = urls
        .into_iter()
        .next()
        .ok_or_else(|| Error::Resolve("no address".into()))?;
    Ok(())
}

/// How often running downloads report [`EngineEvent::Progress`].
const PROGRESS_INTERVAL: Duration = Duration::from_millis(250);

/// How many progress intervals pass between [`EngineEvent::Changed`] events that save a running
/// download's segments (every 2 seconds).
const SAVE_EVERY: u64 = 8;

/// How many progress intervals pass between checks of the queues' schedules (every second).
const SCHEDULE_EVERY: u64 = 4;

/// What the engine needs to run downloads.
#[derive(Debug, Clone)]
pub struct EngineConfig {
    /// Folder holding each download's part-file folder.
    pub temp_dir: PathBuf,
    /// Connections, segment size, speed limit, user agent and proxy.
    pub settings: Settings,
    /// How failed requests are retried.
    pub retry: RetryPolicy,
    /// Works out the current address of downloads with a [`Source`] before each start; such
    /// downloads fail without one.
    pub resolver: Option<Arc<dyn Resolver>>,
}

/// How a segment is retried after a network error, a cut-off connection or a `408`, `429` or
/// `5xx` answer.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RetryPolicy {
    /// Retries for a segment; the count starts over whenever an attempt makes progress.
    pub max_retries: u32,
    /// Wait before the first retry; it doubles with each further retry.
    pub delay: Duration,
}

impl Default for RetryPolicy {
    fn default() -> Self {
        let options = Options::default();
        Self {
            max_retries: options.max_retries,
            delay: options.retry_delay,
        }
    }
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
    /// Login, referer and cookies to send. Kept in memory only, so a download that needs them
    /// can't resume after the app restarts until they are given again.
    pub request: RequestOptions,
    /// What `url` is worked out from again before each start, if it expires.
    pub source: Option<Source>,
}

/// Changes to a download's details, from its Properties.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct DownloadEdit {
    /// A new file name; made safe for every platform. Only while not running or complete.
    pub file_name: Option<String>,
    /// A new address for the same file, such as a mirror. Only while not running or complete;
    /// if the file there differs, the next resume starts over.
    pub url: Option<String>,
    /// A new description; any time.
    pub description: Option<String>,
    /// A new category, or `Some(None)` for none; any time.
    pub category: Option<Option<String>>,
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
    next_queue: Arc<AtomicU32>,
}

impl Engine {
    /// Starts the engine task on the current tokio runtime with `downloads` and `queues`
    /// restored from storage.
    ///
    /// Downloads that were running when the app last stopped come back paused, and no queue
    /// runs until started or scheduled. The main download queue is created if missing. Fails if
    /// the HTTP client can't be built from the settings, such as for a malformed proxy.
    ///
    /// # Panics
    /// If called outside a tokio runtime.
    pub fn start(
        config: EngineConfig,
        downloads: Vec<Download>,
        queues: Vec<Queue>,
    ) -> Result<Self> {
        let client = client_with(&config.settings, &RequestOptions::default())?;
        let global = Limiter::new(config.settings.speed_limit);
        let next_id = downloads.iter().map(|d| d.id.0 + 1).max().unwrap_or(1);
        let known: HashSet<DownloadId> = downloads.iter().map(|d| d.id).collect();
        let mut queues: BTreeMap<QueueId, Queue> = queues
            .into_iter()
            .map(|mut queue| {
                queue.downloads.retain(|id| known.contains(id));
                (queue.id, queue)
            })
            .collect();
        queues.entry(QueueId::MAIN).or_insert_with(Queue::main);
        let next_queue = queues.keys().map(|id| id.0 + 1).max().unwrap_or(1);
        let (commands, command_rx) = mpsc::unbounded_channel();
        let (events, _) = broadcast::channel(1024);
        let (task_tx, task_rx) = mpsc::unbounded_channel();
        let actor = Actor {
            config,
            client,
            global,
            limiters: HashMap::new(),
            requests: HashMap::new(),
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
            queues,
            active: HashSet::new(),
            held: HashSet::new(),
            last_start: HashMap::new(),
            last_stop: HashMap::new(),
            shutdown: Vec::new(),
        };
        tokio::spawn(actor.run(command_rx, task_rx));
        Ok(Self {
            commands,
            events,
            next_id: Arc::new(AtomicU64::new(next_id)),
            next_queue: Arc::new(AtomicU32::new(next_queue)),
        })
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
        download.source = new.source;
        self.send(Command::Add(Box::new(download), new.start, new.request));
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

    /// Uses `settings` from now on. The speed limit applies at once; connection counts apply to
    /// downloads started afterwards. A client that can't be built from them, such as for a
    /// malformed proxy, keeps the old one.
    pub fn set_settings(&self, settings: Settings) {
        self.send(Command::SetSettings(Box::new(settings)));
    }

    /// Limits download `id` to `rate` bytes per second, on top of the global limit; `None`
    /// removes its own limit. Applies at once, also while it runs.
    pub fn set_speed_limit(&self, id: DownloadId, rate: Option<u64>) {
        self.send(Command::SetSpeedLimit(id, rate));
    }

    /// Changes download `id`'s details; see [`DownloadEdit`] for what applies when.
    pub fn edit(&self, id: DownloadId, edit: DownloadEdit) {
        self.send(Command::Edit(id, edit));
    }

    /// Adds an empty queue named `name` and returns its id.
    pub fn add_queue(&self, name: impl Into<String>) -> QueueId {
        let id = QueueId(self.next_queue.fetch_add(1, Ordering::Relaxed));
        self.send(Command::AddQueue(Queue::new(id, name)));
        id
    }

    /// Saves a queue's name, concurrency, schedule and the order of its downloads. Downloads
    /// it doesn't already hold are ignored; ones added meanwhile go at the end.
    pub fn save_queue(&self, queue: Queue) {
        self.send(Command::SaveQueue(Box::new(queue)));
    }

    /// Deletes queue `id`, stopping it; its downloads stay. The main queue can't be deleted.
    pub fn delete_queue(&self, id: QueueId) {
        self.send(Command::DeleteQueue(id));
    }

    /// Moves `downloads` to the end of queue `queue`, out of any other queue.
    pub fn enqueue(&self, queue: QueueId, downloads: Vec<DownloadId>) {
        self.send(Command::Enqueue(queue, downloads));
    }

    /// Takes `downloads` out of their queues.
    pub fn dequeue(&self, downloads: Vec<DownloadId>) {
        self.send(Command::Dequeue(downloads));
    }

    /// Starts queue `id`: it runs its waiting and paused downloads in order, at most its
    /// concurrency at a time, until none is left.
    pub fn start_queue(&self, id: QueueId) {
        self.send(Command::StartQueue(id));
    }

    /// Stops queue `id`, pausing the downloads it runs.
    pub fn stop_queue(&self, id: QueueId) {
        self.send(Command::StopQueue(id));
    }

    /// Sends [`EngineEvent::Changed`] for every download and [`EngineEvent::QueueChanged`] and
    /// [`EngineEvent::QueueRunning`] for every queue.
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
    Add(Box<Download>, bool, RequestOptions),
    Resume(DownloadId),
    Pause(DownloadId),
    PauseAll,
    Delete { id: DownloadId, delete_file: bool },
    SetSettings(Box<Settings>),
    SetSpeedLimit(DownloadId, Option<u64>),
    Edit(DownloadId, DownloadEdit),
    AddQueue(Queue),
    SaveQueue(Box<Queue>),
    DeleteQueue(QueueId),
    Enqueue(QueueId, Vec<DownloadId>),
    Dequeue(Vec<DownloadId>),
    StartQueue(QueueId),
    StopQueue(QueueId),
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
    /// The client for downloads without request options of their own.
    client: Client,
    /// The global speed limit.
    global: Limiter,
    /// Each download's own speed limit, created when first needed.
    limiters: HashMap<DownloadId, Limiter>,
    /// Downloads' request options and the clients that send them.
    requests: HashMap<DownloadId, (RequestOptions, Client)>,
    queues: BTreeMap<QueueId, Queue>,
    /// The queues that run.
    active: HashSet<QueueId>,
    /// Downloads paused by hand, which a running queue leaves alone until started again.
    held: HashSet<DownloadId>,
    /// When each queue's schedule last started and stopped it, in local time.
    last_start: HashMap<QueueId, NaiveDateTime>,
    last_stop: HashMap<QueueId, NaiveDateTime>,
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
                    if ticks.is_multiple_of(SCHEDULE_EVERY) {
                        self.check_schedules(chrono::Local::now().naive_local());
                    }
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
            Command::Add(download, start, request) => {
                let download = *download;
                let id = download.id;
                if !request.is_empty() {
                    match client_with(&self.config.settings, &request) {
                        Ok(client) => {
                            self.requests.insert(id, (request, client));
                        }
                        Err(error) => eprintln!("velo: couldn't set up the request: {error}"),
                    }
                }
                self.emit(EngineEvent::Added(download.clone()));
                self.downloads.insert(id, Entry::new(download));
                if start {
                    self.start(id);
                }
            }
            Command::Resume(id) => {
                self.held.remove(&id);
                self.start(id);
            }
            Command::Pause(id) => {
                self.held.insert(id);
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
            Command::SetSettings(settings) => self.set_settings(*settings),
            Command::SetSpeedLimit(id, rate) => self.limiter(id).set_rate(rate),
            Command::Edit(id, edit) => self.edit(id, edit),
            Command::AddQueue(queue) => {
                let id = queue.id;
                self.queues.insert(id, queue);
                self.emit_queue(id);
            }
            Command::SaveQueue(queue) => self.save_queue(*queue),
            Command::DeleteQueue(id) => {
                if id != QueueId::MAIN && self.queues.contains_key(&id) {
                    self.stop_queue(id);
                    self.queues.remove(&id);
                    self.emit(EngineEvent::QueueRemoved(id));
                }
            }
            Command::Enqueue(target, ids) => {
                if !self.queues.contains_key(&target) {
                    return;
                }
                let ids: Vec<DownloadId> = ids
                    .into_iter()
                    .filter(|id| self.downloads.contains_key(id))
                    .collect();
                self.take_out_of_queues(&ids, Some(target));
                if let Some(queue) = self.queues.get_mut(&target) {
                    for id in ids {
                        if !queue.downloads.contains(&id) {
                            queue.downloads.push(id);
                        }
                    }
                }
                self.emit_queue(target);
                self.pump(target);
            }
            Command::Dequeue(ids) => self.take_out_of_queues(&ids, None),
            Command::StartQueue(id) => self.start_queue(id),
            Command::StopQueue(id) => self.stop_queue(id),
            Command::Resync => {
                let all: Vec<_> = self
                    .downloads
                    .values()
                    .map(|e| e.download.clone())
                    .collect();
                for download in all {
                    self.emit(EngineEvent::Changed(download));
                }
                let ids: Vec<QueueId> = self.queues.keys().copied().collect();
                for id in ids {
                    self.emit_queue(id);
                    self.emit(EngineEvent::QueueRunning(id, self.active.contains(&id)));
                }
            }
            Command::Shutdown(reply) => {
                self.shutdown.push(reply);
                self.active.clear();
                self.handle(Command::PauseAll);
            }
        }
    }

    /// Starts a task for download `id`, unless it is running, complete or the engine is
    /// shutting down.
    fn start(&mut self, id: DownloadId) {
        if !self.shutdown.is_empty() || !self.downloads.contains_key(&id) {
            return;
        }
        let own_limit = self.limiter(id);
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
        let client = match self.requests.get(&id) {
            Some((_, client)) => client.clone(),
            None => self.client.clone(),
        };
        let temp_dir = self.config.temp_dir.clone();
        let host = reqwest::Url::parse(&entry.download.url)
            .ok()
            .and_then(|url| url.host_str().map(str::to_string))
            .unwrap_or_default();
        let settings = &self.config.settings;
        let options = Options {
            max_connections: settings.connections_for(&host),
            min_segment_size: settings.min_segment_size,
            max_retries: self.config.retry.max_retries,
            retry_delay: self.config.retry.delay,
            limiters: vec![self.global.clone(), own_limit],
            max_request_size: entry
                .download
                .source
                .as_ref()
                .and_then(Source::max_request_size),
        };
        let resolver = self.config.resolver.clone();
        let progress = running.progress.clone();
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
            let result = match refresh(&mut download, resolver.as_deref(), &cancel).await {
                Ok(()) => {
                    run(&context, &mut download, |stage, d| {
                        let _ = tx.send(TaskMessage::Stage(stage, d.clone()));
                    })
                    .await
                }
                Err(error) => Err(error),
            };
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
        if stopped && let Some(queue) = self.queue_of(id) {
            self.pump(queue);
        }
    }

    /// The queue download `id` is in, if any.
    fn queue_of(&self, id: DownloadId) -> Option<QueueId> {
        self.queues
            .values()
            .find(|q| q.downloads.contains(&id))
            .map(|q| q.id)
    }

    /// Starts queue `id`, forgetting which of its downloads were paused by hand.
    fn start_queue(&mut self, id: QueueId) {
        let Some(queue) = self.queues.get(&id) else {
            return;
        };
        for download in &queue.downloads {
            self.held.remove(download);
        }
        if self.active.insert(id) {
            self.emit(EngineEvent::QueueRunning(id, true));
        }
        self.pump(id);
    }

    /// Stops queue `id` and pauses what it runs.
    fn stop_queue(&mut self, id: QueueId) {
        if !self.active.remove(&id) {
            return;
        }
        if let Some(queue) = self.queues.get(&id) {
            for download in &queue.downloads {
                if let Some(running) = self
                    .downloads
                    .get(download)
                    .and_then(|e| e.running.as_ref())
                {
                    running.cancel.cancel();
                }
            }
        }
        self.emit(EngineEvent::QueueRunning(id, false));
    }

    /// Starts queue `id`'s next downloads while it has room, and marks it finished when nothing
    /// is left to run.
    fn pump(&mut self, id: QueueId) {
        if !self.active.contains(&id) || !self.shutdown.is_empty() {
            return;
        }
        let Some(queue) = self.queues.get(&id) else {
            return;
        };
        let ids = queue.downloads.clone();
        let limit = queue.max_concurrent.max(1) as usize;
        let is_running = |actor: &Self, id: &DownloadId| {
            actor.downloads.get(id).is_some_and(|e| e.running.is_some())
        };
        let mut free = limit.saturating_sub(ids.iter().filter(|d| is_running(self, d)).count());
        for download in &ids {
            if free == 0 {
                break;
            }
            let waiting = self.downloads.get(download).is_some_and(|e| {
                e.running.is_none()
                    && matches!(
                        e.download.state,
                        DownloadState::Queued | DownloadState::Paused
                    )
            });
            if waiting && !self.held.contains(download) {
                self.start(*download);
                free -= 1;
            }
        }
        if !ids.iter().any(|d| is_running(self, d)) {
            self.active.remove(&id);
            self.emit(EngineEvent::QueueRunning(id, false));
            self.emit(EngineEvent::QueueFinished(id));
        }
    }

    /// Saves a queue's settings from the app, keeping its downloads to those it holds.
    fn save_queue(&mut self, update: Queue) {
        let Some(queue) = self.queues.get_mut(&update.id) else {
            return;
        };
        if !update.name.trim().is_empty() {
            queue.name = update.name.trim().to_string();
        }
        queue.max_concurrent = update.max_concurrent.max(1);
        queue.schedule = update.schedule;
        let mut order: Vec<DownloadId> = update
            .downloads
            .into_iter()
            .filter(|id| queue.downloads.contains(id))
            .collect();
        for id in &queue.downloads {
            if !order.contains(id) {
                order.push(*id);
            }
        }
        queue.downloads = order;
        let id = queue.id;
        self.emit_queue(id);
        self.pump(id);
    }

    /// Takes `ids` out of every queue but `keep`, and reports the queues that changed.
    fn take_out_of_queues(&mut self, ids: &[DownloadId], keep: Option<QueueId>) {
        let mut changed = Vec::new();
        for queue in self.queues.values_mut() {
            if Some(queue.id) == keep {
                continue;
            }
            let before = queue.downloads.len();
            queue.downloads.retain(|id| !ids.contains(id));
            if queue.downloads.len() != before {
                changed.push(queue.id);
            }
        }
        for id in changed {
            self.emit_queue(id);
            self.pump(id);
        }
    }

    /// Starts and stops queues whose schedule says so at `now`, in local time.
    fn check_schedules(&mut self, now: NaiveDateTime) {
        let mut starts = Vec::new();
        let mut stops = Vec::new();
        for (id, queue) in &self.queues {
            if scheduler::start_due(&queue.schedule, now, self.last_start.get(id).copied()) {
                starts.push(*id);
            }
            if scheduler::stop_due(&queue.schedule, now, self.last_stop.get(id).copied()) {
                stops.push(*id);
            }
        }
        for id in stops {
            self.last_stop.insert(id, now);
            self.stop_queue(id);
        }
        for id in starts {
            self.last_start.insert(id, now);
            // A once-only start happens once.
            if let Some(queue) = self.queues.get_mut(&id)
                && queue.schedule.is_once()
            {
                queue.schedule.start_at = None;
                self.emit_queue(id);
            }
            self.start_queue(id);
        }
    }

    /// Reports queue `id` as it is now.
    fn emit_queue(&self, id: QueueId) {
        if let Some(queue) = self.queues.get(&id) {
            self.emit(EngineEvent::QueueChanged(queue.clone()));
        }
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
        self.requests.remove(&id);
        self.limiters.remove(&id);
        self.held.remove(&id);
        self.take_out_of_queues(&[id], None);
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

    /// Uses `settings` from now on: rebuilds the clients and sets the global speed limit.
    fn set_settings(&mut self, settings: Settings) {
        match client_with(&settings, &RequestOptions::default()) {
            Ok(client) => self.client = client,
            Err(error) => {
                eprintln!("velo: keeping the old connection settings: {error}");
                return;
            }
        }
        for (request, client) in self.requests.values_mut() {
            if let Ok(new) = client_with(&settings, request) {
                *client = new;
            }
        }
        self.global.set_rate(settings.speed_limit);
        self.config.settings = settings;
    }

    /// Applies `edit` to download `id` as far as its state allows, and reports the change.
    fn edit(&mut self, id: DownloadId, edit: DownloadEdit) {
        let Some(entry) = self.downloads.get_mut(&id) else {
            return;
        };
        let download = &mut entry.download;
        let settled = entry.running.is_none() && download.state != DownloadState::Complete;
        if let Some(description) = edit.description {
            download.description = description;
        }
        if let Some(category) = edit.category {
            download.category = category;
        }
        if settled {
            if let Some(name) = edit
                .file_name
                .as_deref()
                .and_then(crate::sanitize_file_name)
            {
                download.file_name = name;
            }
            if let Some(url) = edit.url.filter(|u| reqwest::Url::parse(u).is_ok()) {
                download.url = url;
            }
        }
        let snapshot = download.clone();
        self.emit(EngineEvent::Changed(snapshot));
    }

    /// Download `id`'s own speed limit, created without a limit if it has none yet.
    fn limiter(&mut self, id: DownloadId) -> Limiter {
        self.limiters.entry(id).or_default().clone()
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
