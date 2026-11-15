//! Tests of whole user flows: the real windows, clicked and typed into as a user would, with
//! the real engine behind them and the engine's local test server as the internet.
//!
//! They run without a display and without an event loop (see
//! [`i_slint_backend_testing::init_no_event_loop`]), so two things the app does on its event
//! loop are done by the [`Harness`] instead: engine events reach the window when a test calls
//! [`Harness::pump_until`] (the app's timer does that every 250 ms), and servers are asked about
//! files by [`SyncProber`], which answers before it returns.
//!
//! Controls are found by what assistive technology calls them, which needs the UI compiled with
//! debug info; `build.rs` turns that on for debug builds.

use std::path::PathBuf;
use std::rc::Rc;
use std::time::{Duration, Instant};

use i_slint_backend_testing::{AccessibleRole, ElementHandle, ElementRoot};
use slint::platform::{PointerEventButton, WindowEvent};
use slint::{ComponentHandle, LogicalSize, Model};
use velo_core::Settings;
use velo_engine::testing as support;
use velo_engine::{RequestOptions, RetryPolicy};
use velo_store::Store;

use crate::add::{Adder, ProbeDone, Prober, VideoDone};
use crate::app::{App, Commands};
use crate::bridge::{Bridge, BridgeConfig};
use crate::complete::tests::FakeDesktop;
use crate::config::Config;
use crate::options::OptionsDialog;
use crate::{AppWindow, DownloadRow, create_window, format, wire};

/// How long a test waits for the app to get somewhere. Every wait ends as soon as it has, so
/// this only has to be long enough for the slowest CI runner.
const PATIENCE: Duration = Duration::from_secs(30);

/// Asks servers about files on the engine's runtime and answers before it returns, since
/// nothing can be handed back to the UI thread later without an event loop.
struct SyncProber(tokio::runtime::Handle);

impl Prober for SyncProber {
    fn probe(&self, url: String, settings: Settings, request: RequestOptions, done: ProbeDone) {
        let result = velo_engine::client_with(&settings, &request)
            .map_err(|e| e.to_string())
            .and_then(|client| {
                self.0
                    .block_on(velo_engine::probe(&client, &url))
                    .map_err(|e| e.to_string())
            });
        done(result);
    }

    fn video(&self, _: String, done: VideoDone) {
        done(Err("the flow tests have no YouTube".into()));
    }
}

/// The app as `main` starts it, in a temporary folder: the main window, its dialogs, the engine
/// and a test server.
pub struct Harness {
    /// Holds the settings file, the database and the `downloads` and `parts` folders.
    pub dir: tempfile::TempDir,
    /// The test server's base address, such as `http://127.0.0.1:1234`.
    pub server: String,
    /// The main window.
    pub ui: AppWindow,
    /// Opens Add URL and Download File Info.
    pub adder: Rc<Adder>,
    /// Opens Options.
    pub options: Rc<OptionsDialog>,
    /// The settings the app shares.
    pub config: Rc<Config>,
    /// The main window's state.
    pub app: Rc<App>,
    /// The engine's thread, which the events come from.
    bridge: Rc<Bridge>,
}

impl Harness {
    /// Starts the app with default settings and nothing in the list.
    pub fn start() -> Self {
        i_slint_backend_testing::init_no_event_loop();
        let dir = tempfile::tempdir().unwrap();
        let (downloads, parts) = (dir.path().join("downloads"), dir.path().join("parts"));
        std::fs::create_dir_all(&downloads).unwrap();
        let settings = Settings::default();
        let bridge = Rc::new(
            Bridge::start(BridgeConfig {
                temp_dir: parts.clone(),
                settings: settings.clone(),
                retry: RetryPolicy::default(),
                player_cache: dir.path().join("player-cache"),
                store: Store::open(&dir.path().join("velo.db")).unwrap(),
                downloads: Vec::new(),
                queues: Vec::new(),
            })
            .unwrap(),
        );
        let server = bridge.runtime().block_on(support::start());
        let commands: Rc<dyn Commands> = Rc::new(bridge.engine().clone());
        let config = Rc::new(Config::new(
            settings,
            Some(dir.path().join("settings.toml")),
            downloads.clone(),
        ));
        let (ui, app) = create_window(
            config.clone(),
            commands.clone(),
            Rc::new(FakeDesktop::default()),
        )
        .unwrap();
        let prober = Rc::new(SyncProber(bridge.runtime().clone()));
        let (adder, options) = wire(&ui, &app, &config, &commands, prober, downloads, parts);
        stage(&ui);
        assert!(
            ElementHandle::find_by_accessible_label(&ui, "Add URL")
                .next()
                .is_some(),
            "the toolbar's Add URL can't be found: the UI was compiled without debug info, \
             which build.rs only adds to debug builds, so run these tests without --release"
        );
        Self {
            dir,
            server,
            ui,
            adder,
            options,
            config,
            app,
            bridge,
        }
    }

    /// The folder downloads are saved to.
    pub fn downloads(&self) -> PathBuf {
        self.dir.path().join("downloads")
    }

    /// Passes engine events to the window, as the app's timer does, until `done` holds; panics
    /// with `what` if it doesn't within [`PATIENCE`].
    pub fn pump_until(&self, what: &str, done: impl Fn() -> bool) {
        let deadline = Instant::now() + PATIENCE;
        loop {
            self.app.apply(self.bridge.drain());
            if done() {
                return;
            }
            assert!(Instant::now() < deadline, "timed out waiting until {what}");
            std::thread::sleep(Duration::from_millis(10));
        }
    }

    /// The row of the download saved as `name`, if it is in the list.
    pub fn row(&self, name: &str) -> Option<DownloadRow> {
        self.ui.get_downloads().iter().find(|row| row.name == name)
    }

    /// Waits until the download saved as `name` is in the list and `wanted` holds for its row.
    pub fn wait_for_row(&self, name: &str, what: &str, wanted: impl Fn(&DownloadRow) -> bool) {
        self.pump_until(&format!("{name} {what}"), || {
            self.row(name).is_some_and(|row| wanted(&row))
        });
    }

    /// Adds the test server's `path` as a user does: Add URL in the toolbar, the address typed
    /// in, OK, then Start Download or (without `start`) Download Later in Download File Info.
    pub fn add(&self, path: &str, start: bool) {
        click(&self.ui, "Add URL");
        let add = self.adder.add_url_window().expect("Add URL opens");
        stage(&add);
        assert!(!enabled(&add, "OK"), "nothing typed yet");
        let url = format!("{}{path}", self.server);
        type_into(&add, "https://example.com/file.zip", &url);
        assert_eq!(add.get_url(), url.as_str());
        click(&add, "OK");

        let info = self.adder.info_window().expect("Download File Info opens");
        stage(&info);
        assert_eq!(info.get_error(), "");
        assert_eq!(info.get_size(), format::size(support::SIZE as u64).as_str());
        let button = if start {
            "Start Download"
        } else {
            "Download Later"
        };
        click(&info, button);
        assert!(self.adder.info_window().is_none(), "the dialog closes");
    }
}

impl Drop for Harness {
    fn drop(&mut self) {
        self.bridge.shutdown();
    }
}

/// The size every window is shown at: roomy, so that each control is inside the window, and so
/// can be clicked, whatever the widget style of the operating system makes of the layout.
const STAGE_SIZE: LogicalSize = LogicalSize::new(1100.0, 700.0);

/// Shows `window` at [`STAGE_SIZE`] and makes it the active one, so its controls have a place
/// to be clicked at and it takes key presses.
pub fn stage(window: &impl ComponentHandle) {
    window.show().unwrap();
    window.window().set_size(STAGE_SIZE);
    window
        .window()
        .dispatch_event(WindowEvent::WindowActiveChanged(true));
}

/// The control of `window` that assistive technology calls `label`: a button or a tab by its
/// text, a list row by its file name. Panics unless there is exactly one.
pub fn element(window: &impl ElementRoot, label: &str) -> ElementHandle {
    let all: Vec<ElementHandle> = ElementHandle::find_by_accessible_label(window, label).collect();
    // A button's caption is a text with the same label; the control is the other one. An entry
    // of an open combo box is only a text.
    let controls: Vec<&ElementHandle> = all
        .iter()
        .filter(|e| e.accessible_role() != Some(AccessibleRole::Text))
        .collect();
    match (controls.as_slice(), all.as_slice()) {
        ([control], _) => (*control).clone(),
        ([], [text]) => text.clone(),
        ([], []) => panic!("nothing is labelled “{label}”"),
        ([], _) => panic!("“{label}” is the text of several elements and labels no control"),
        _ => panic!("“{label}” labels several controls; say which with `control`"),
    }
}

/// The control of type `type_name` (such as `Button` or `ToolButton`) labelled `label`, for a
/// label that controls of several types share. Panics unless there is exactly one.
pub fn control(window: &impl ElementRoot, type_name: &str, label: &str) -> ElementHandle {
    let mut found = ElementHandle::find_by_accessible_label(window, label)
        .filter(|e| e.type_name().as_deref() == Some(type_name));
    let control = found
        .next()
        .unwrap_or_else(|| panic!("no {type_name} is labelled “{label}”"));
    assert!(
        found.next().is_none(),
        "“{label}” labels several {type_name}s"
    );
    control
}

/// Clicks `element` with the mouse, in its middle.
pub fn press(element: &ElementHandle) {
    element.mock_single_click(PointerEventButton::Left);
}

/// Clicks the control labelled `label` with the mouse, in its middle.
pub fn click(window: &impl ElementRoot, label: &str) {
    press(&element(window, label));
}

/// Whether the control labelled `label` is enabled. Panics if it doesn't say, which a custom
/// control only does with `accessible-enabled`.
pub fn enabled(window: &impl ElementRoot, label: &str) -> bool {
    element(window, label)
        .accessible_enabled()
        .unwrap_or_else(|| panic!("“{label}” doesn't say whether it is enabled"))
}

/// Clicks the text field of `window` whose placeholder is `placeholder` and types `text` into
/// it, key by key.
pub fn type_into(window: &(impl ComponentHandle + ElementRoot), placeholder: &str, text: &str) {
    let field = window
        .root_element()
        .query_descendants()
        .match_predicate({
            let placeholder = placeholder.to_string();
            move |e| e.accessible_placeholder_text().as_deref() == Some(placeholder.as_str())
        })
        .find_first()
        .unwrap_or_else(|| panic!("no text field shows “{placeholder}”"));
    press(&field);
    for key in text.chars() {
        let text: slint::SharedString = key.into();
        window
            .window()
            .dispatch_event(WindowEvent::KeyPressed { text: text.clone() });
        window
            .window()
            .dispatch_event(WindowEvent::KeyReleased { text });
    }
}

#[test]
fn a_download_added_from_the_toolbar_runs_to_the_end() {
    let app = Harness::start();
    assert!(!enabled(&app.ui, "Resume") && !enabled(&app.ui, "Delete"));

    app.add("/files/data.bin", true);
    app.wait_for_row("data.bin", "is finished", |row| row.finished);
    let saved = app.downloads().join("data.bin");
    assert!(std::fs::read(&saved).unwrap() == support::body());
    assert!(!app.downloads().join("data.bin.velo-tmp").exists());
    assert_eq!(app.ui.get_status_message(), "1 download");
    assert!(enabled(&app.ui, "Delete Completed"));
}

#[test]
fn a_running_download_is_stopped_resumed_and_deleted_from_the_toolbar() {
    let app = Harness::start();
    app.add("/slower/data.bin", true);
    app.wait_for_row("data.bin", "is downloading", |row| row.can_stop);

    // Selecting the row makes the toolbar act on it.
    click(&app.ui, "data.bin");
    assert!(enabled(&app.ui, "Stop") && !enabled(&app.ui, "Resume"));
    click(&app.ui, "Stop");
    app.wait_for_row("data.bin", "is paused", |row| row.can_resume);
    assert!(enabled(&app.ui, "Resume") && !enabled(&app.ui, "Stop"));
    let saved = app.downloads().join("data.bin");
    assert!(app.downloads().join("data.bin.velo-tmp").is_file() && !saved.exists());

    click(&app.ui, "Resume");
    app.wait_for_row("data.bin", "is finished", |row| row.finished);
    assert!(std::fs::read(&saved).unwrap() == support::body());
    assert!(!enabled(&app.ui, "Resume") && !enabled(&app.ui, "Stop"));

    // Deleting asks first, and takes the file along only if the box is ticked.
    press(&control(&app.ui, "ToolButton", "Delete"));
    click(&app.ui, "Also delete the file from disk");
    press(&control(&app.ui, "Button", "Delete"));
    app.pump_until("the download and its file are gone", || {
        app.row("data.bin").is_none() && !saved.exists()
    });
    assert_eq!(app.ui.get_status_message(), "0 downloads");
}

#[test]
fn deleting_an_unfinished_download_removes_what_it_downloaded() {
    let app = Harness::start();
    app.add("/slower/data.bin", true);
    app.wait_for_row("data.bin", "is downloading", |row| row.can_stop);
    click(&app.ui, "data.bin");
    click(&app.ui, "Stop");
    app.wait_for_row("data.bin", "is paused", |row| row.can_resume);
    let unfinished = app.downloads().join("data.bin.velo-tmp");
    assert!(unfinished.is_file());

    // Cancel keeps it; Delete removes it and its unfinished file.
    press(&control(&app.ui, "ToolButton", "Delete"));
    click(&app.ui, "Cancel");
    assert!(app.row("data.bin").is_some());
    press(&control(&app.ui, "ToolButton", "Delete"));
    press(&control(&app.ui, "Button", "Delete"));
    app.pump_until("the download and its unfinished file are gone", || {
        app.row("data.bin").is_none() && !unfinished.exists()
    });
}

#[test]
fn options_are_saved_applied_and_shown_again() {
    let app = Harness::start();
    let saved = || velo_store::load_settings(&app.dir.path().join("settings.toml")).unwrap();
    click(&app.ui, "Options");
    let options = app.options.window().expect("Options opens");
    stage(&options);

    click(&options, "Connection");
    let connections = element(&options, "Connections per download");
    assert_eq!(connections.accessible_value().as_deref(), Some("8"));
    press(&connections);
    click(&options, "16");
    click(&options, "Limit the total speed to");
    // Its up arrow: 1024 KB/sec is offered, one more is 1025.
    element(&options, "Speed limit in KB/sec").invoke_accessible_increment_action();
    click(&options, "OK");
    assert!(app.options.window().is_none(), "the dialog closes");

    for settings in [app.config.settings(), saved()] {
        assert_eq!(settings.max_connections, 16);
        assert_eq!(settings.speed_limit, Some(1025 * 1024));
    }

    // Opened again it shows what was saved; Cancel leaves the settings alone.
    click(&app.ui, "Options");
    let options = app.options.window().expect("Options opens again");
    stage(&options);
    click(&options, "Connection");
    let connections = element(&options, "Connections per download");
    assert_eq!(connections.accessible_value().as_deref(), Some("16"));
    let limit = element(&options, "Limit the total speed to");
    assert_eq!(limit.accessible_checked(), Some(true));
    press(&limit);
    click(&options, "Cancel");
    assert!(app.options.window().is_none());
    assert_eq!(saved().speed_limit, Some(1025 * 1024));
}

#[test]
fn a_queue_is_started_and_stopped_from_the_scheduler_and_the_toolbar() {
    let app = Harness::start();
    app.add("/slower/first.bin", false);
    app.add("/slower/second.bin", false);
    for name in ["first.bin", "second.bin"] {
        app.wait_for_row(name, "is in the list", |_| true);
        // Add to Queue is in the row's context menu, which the test backend can't open: this
        // is the callback its entry for the main queue runs.
        let row = app.row(name).unwrap();
        app.ui
            .invoke_enqueue(row.id, velo_core::QueueId::MAIN.0 as i32);
        app.wait_for_row(name, "is queued", |row| !row.queue.is_empty());
    }
    assert!(enabled(&app.ui, "Start Queue") && !enabled(&app.ui, "Stop Queue"));

    click(&app.ui, "Scheduler");
    let scheduler = app.app.scheduler_window().expect("the Scheduler opens");
    stage(&scheduler);
    click(&scheduler, "Start now");
    // The main queue runs one download at a time, in order.
    app.wait_for_row("first.bin", "is downloading", |row| row.can_stop);
    assert!(app.row("second.bin").is_some_and(|row| !row.can_stop));
    app.pump_until("the toolbar shows the queue running", || {
        enabled(&app.ui, "Stop Queue") && !enabled(&app.ui, "Start Queue")
    });

    click(&scheduler, "Stop");
    app.pump_until("the queue has stopped", || {
        enabled(&app.ui, "Start Queue") && app.row("first.bin").is_some_and(|row| !row.can_stop)
    });
    assert!(app.row("second.bin").is_some_and(|row| !row.finished));

    click(&app.ui, "Start Queue");
    for name in ["first.bin", "second.bin"] {
        app.wait_for_row(name, "is finished", |row| row.finished);
        assert!(std::fs::read(app.downloads().join(name)).unwrap() == support::body());
    }
    app.pump_until("the queue is done", || !enabled(&app.ui, "Stop Queue"));
}
