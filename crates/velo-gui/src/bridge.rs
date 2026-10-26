//! Runs the engine on a tokio runtime in a background thread and connects it to the UI thread.
//!
//! The UI thread sends commands through the [`Engine`] handle, which never blocks. Events come
//! back as plain data on a `std` channel that the UI drains on a timer, since Slint models can't
//! leave the UI thread. A task on the runtime also writes every event to the [`Store`], so the
//! download list survives a restart.

use std::path::PathBuf;
use std::sync::mpsc;
use std::thread::JoinHandle;

use tokio::sync::{broadcast, oneshot};
use velo_core::{Download, EngineEvent};
use velo_engine::{Engine, EngineConfig, Options};
use velo_store::Store;

/// What the bridge needs to start the engine.
pub struct BridgeConfig {
    /// Folder for part files.
    pub temp_dir: PathBuf,
    /// Connection and retry settings.
    pub options: Options,
    /// The database to keep the download list in.
    pub store: Store,
    /// Downloads restored from the store.
    pub downloads: Vec<Download>,
}

/// The running engine thread.
pub struct Bridge {
    engine: Engine,
    events: mpsc::Receiver<EngineEvent>,
    stop: Option<oneshot::Sender<()>>,
    thread: Option<JoinHandle<()>>,
}

impl Bridge {
    /// Starts the runtime thread and the engine on it.
    pub fn start(config: BridgeConfig) -> std::io::Result<Self> {
        let runtime = tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .thread_name("velo-engine")
            .build()?;
        let client = velo_engine::client().map_err(std::io::Error::other)?;
        let (engine_tx, engine_rx) = mpsc::channel();
        let (events_tx, events) = mpsc::channel();
        let (stop, stopped) = oneshot::channel();
        let BridgeConfig {
            temp_dir,
            options,
            store,
            downloads,
        } = config;

        let thread = std::thread::Builder::new()
            .name("velo-runtime".into())
            .spawn(move || {
                runtime.block_on(async move {
                    let engine = Engine::start(
                        EngineConfig {
                            client,
                            temp_dir,
                            options,
                        },
                        downloads,
                    );
                    let (persist_stop, persist_stopped) = oneshot::channel();
                    let persist = tokio::spawn(persist(
                        store,
                        engine.subscribe(),
                        engine.clone(),
                        persist_stopped,
                    ));
                    tokio::spawn(forward(engine.subscribe(), engine.clone(), events_tx));
                    let _ = engine_tx.send(engine.clone());

                    let _ = stopped.await;
                    // Pause everything; the final Changed events are queued once this returns.
                    engine.shutdown().await;
                    let _ = persist_stop.send(());
                    let _ = persist.await;
                });
            })?;
        let engine = engine_rx
            .recv()
            .map_err(|_| std::io::Error::other("the engine thread stopped while starting"))?;
        Ok(Self {
            engine,
            events,
            stop: Some(stop),
            thread: Some(thread),
        })
    }

    /// A handle for sending commands to the engine.
    pub fn engine(&self) -> &Engine {
        &self.engine
    }

    /// Every event that has arrived since the last call, without waiting.
    pub fn drain(&self) -> Vec<EngineEvent> {
        self.events.try_iter().collect()
    }

    /// Pauses every download, waits until their state is saved, and stops the thread.
    pub fn shutdown(mut self) {
        if let Some(stop) = self.stop.take() {
            let _ = stop.send(());
        }
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

/// Writes every engine event to `store` until `stop` fires, then writes whatever is still
/// queued and returns.
async fn persist(
    mut store: Store,
    mut events: broadcast::Receiver<EngineEvent>,
    engine: Engine,
    mut stop: oneshot::Receiver<()>,
) {
    loop {
        tokio::select! {
            biased;
            event = events.recv() => match event {
                Ok(event) => save(&mut store, &event),
                // Missed events: ask for every download again so nothing is lost.
                Err(broadcast::error::RecvError::Lagged(_)) => engine.resync(),
                Err(broadcast::error::RecvError::Closed) => return,
            },
            _ = &mut stop => break,
        }
    }
    while let Ok(event) = events.try_recv() {
        save(&mut store, &event);
    }
}

/// Writes one event to the store; a failed write is reported and the next event tries again.
fn save(store: &mut Store, event: &EngineEvent) {
    if let Err(error) = store.apply(event) {
        eprintln!("velo: couldn't save the download list: {error}");
    }
}

/// Passes engine events to the UI thread until the UI side is gone.
async fn forward(
    mut events: broadcast::Receiver<EngineEvent>,
    engine: Engine,
    ui: mpsc::Sender<EngineEvent>,
) {
    loop {
        match events.recv().await {
            Ok(event) => {
                if ui.send(event).is_err() {
                    return;
                }
            }
            Err(broadcast::error::RecvError::Lagged(_)) => engine.resync(),
            Err(broadcast::error::RecvError::Closed) => return,
        }
    }
}

#[cfg(test)]
mod tests {
    use std::time::{Duration, Instant};

    use velo_core::DownloadState;
    use velo_engine::NewDownload;

    use super::*;

    /// Collects UI events until `done` accepts one, for up to 10 s.
    fn wait_for(bridge: &Bridge, done: impl Fn(&EngineEvent) -> bool) -> EngineEvent {
        let deadline = Instant::now() + Duration::from_secs(10);
        while Instant::now() < deadline {
            if let Some(event) = bridge.drain().into_iter().find(&done) {
                return event;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        panic!("timed out waiting for an engine event");
    }

    #[test]
    fn events_reach_the_ui_and_the_store_and_survive_a_restart() {
        let dir = tempfile::tempdir().unwrap();
        let database = dir.path().join("velo.db");
        let config = |downloads| BridgeConfig {
            temp_dir: dir.path().join("parts"),
            options: Options {
                max_retries: 0,
                ..Options::default()
            },
            store: Store::open(&database).unwrap(),
            downloads,
        };

        let bridge = Bridge::start(config(Vec::new())).unwrap();
        // Nothing listens on port 1, so the download fails straight away.
        let id = bridge.engine().add(NewDownload {
            url: "http://127.0.0.1:1/file.bin".into(),
            file_name: String::new(),
            save_dir: dir.path().to_path_buf(),
            category: None,
            description: "unreachable".into(),
            start: true,
        });
        wait_for(
            &bridge,
            |e| matches!(e, EngineEvent::Changed(d) if d.id == id && matches!(d.state, DownloadState::Failed(_))),
        );
        bridge.shutdown();

        let saved = Store::open(&database).unwrap().load_downloads().unwrap();
        assert_eq!(saved.len(), 1);
        assert_eq!(saved[0].description, "unreachable");
        assert!(matches!(saved[0].state, DownloadState::Failed(_)));

        // A new bridge picks up where the last one stopped.
        let bridge = Bridge::start(config(saved)).unwrap();
        bridge.engine().resync();
        wait_for(
            &bridge,
            |e| matches!(e, EngineEvent::Changed(d) if d.id == id),
        );
        bridge.shutdown();
    }
}
