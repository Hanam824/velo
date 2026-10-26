//! Progress dialogs: one window per download, showing its status, segments and connections.

use std::cell::{Cell, RefCell};
use std::collections::{BTreeMap, HashMap};
use std::rc::Rc;

use slint::{ComponentHandle, ModelRc, VecModel};
use velo_core::{Download, DownloadId, DownloadState, SegmentState};

use crate::app::Commands;
use crate::config::Config;
use crate::list::DownloadTable;
use crate::{ConnectionRow, ProgressWindow, SegmentView, format, theme};

/// What a download's progress dialog lets the user choose besides pausing.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DownloadOptions {
    /// The download's own speed limit in KB/s, if on.
    pub limit_kb: Option<u32>,
    /// Show Download complete when it finishes.
    pub show_complete: bool,
    /// Quit Velo when it finishes.
    pub exit: bool,
    /// Turn the computer off when it finishes.
    pub shutdown: bool,
}

impl Default for DownloadOptions {
    fn default() -> Self {
        Self {
            limit_kb: None,
            show_complete: true,
            exit: false,
            shutdown: false,
        }
    }
}

/// The open progress dialogs, and the options chosen in them for this session.
pub struct ProgressDialogs {
    commands: Rc<dyn Commands>,
    config: Rc<Config>,
    /// Options per download; downloads not here use the defaults.
    options: RefCell<HashMap<DownloadId, DownloadOptions>>,
    /// Each open dialog, and whether its download has been in the table yet: a dialog opened
    /// before the engine reports its download must not close as "deleted".
    windows: RefCell<BTreeMap<DownloadId, (ProgressWindow, Cell<bool>)>>,
}

impl ProgressDialogs {
    /// Creates the dialog manager; dialogs send their commands to `commands`.
    pub fn new(commands: Rc<dyn Commands>, config: Rc<Config>) -> Rc<Self> {
        Rc::new(Self {
            commands,
            config,
            options: RefCell::default(),
            windows: RefCell::default(),
        })
    }

    /// Shows download `id`'s dialog, opening it if needed, filled from `table`.
    pub fn open(
        self: &Rc<Self>,
        id: DownloadId,
        table: &DownloadTable,
    ) -> Result<(), slint::PlatformError> {
        if let Some((window, _)) = self.windows.borrow().get(&id) {
            return window.show();
        }
        let window = ProgressWindow::new()?;
        theme::apply(&window, self.config.settings().appearance);
        let options = self.options_for(id);
        window.set_limit_enabled(options.limit_kb.is_some());
        if let Some(kb) = options.limit_kb {
            window.set_limit_kb(kb as i32);
        }
        window.set_show_complete(options.show_complete);
        window.set_exit_when_done(options.exit);
        window.set_shutdown_when_done(options.shutdown);
        let (this, weak) = (Rc::downgrade(self), window.as_weak());
        window.on_limit_changed(move || {
            let (Some(this), Some(window)) = (this.upgrade(), weak.upgrade()) else {
                return;
            };
            let kb = window
                .get_limit_enabled()
                .then(|| window.get_limit_kb().max(1) as u32);
            this.options.borrow_mut().entry(id).or_default().limit_kb = kb;
            this.commands
                .set_speed_limit(id, kb.map(|kb| u64::from(kb) * 1024));
        });
        let (this, weak) = (Rc::downgrade(self), window.as_weak());
        window.on_completion_changed(move || {
            let (Some(this), Some(window)) = (this.upgrade(), weak.upgrade()) else {
                return;
            };
            let mut options = this.options.borrow_mut();
            let entry = options.entry(id).or_default();
            entry.show_complete = window.get_show_complete();
            entry.exit = window.get_exit_when_done();
            entry.shutdown = window.get_shutdown_when_done();
        });
        let commands = self.commands.clone();
        window.on_pause(move || commands.pause(id));
        let commands = self.commands.clone();
        window.on_resume(move || commands.resume(id));
        let this = Rc::downgrade(self);
        window.on_cancel(move || {
            if let Some(this) = this.upgrade() {
                this.commands.pause(id);
                this.close(id);
            }
        });
        let this = Rc::downgrade(self);
        window.window().on_close_requested(move || {
            if let Some(this) = this.upgrade() {
                this.windows.borrow_mut().remove(&id);
            }
            slint::CloseRequestResponse::HideWindow
        });
        if let Some(download) = table.get(id) {
            fill(&window, download, table);
        }
        window.show()?;
        let seen = Cell::new(table.get(id).is_some());
        self.windows.borrow_mut().insert(id, (window, seen));
        Ok(())
    }

    /// Updates every open dialog from `table`, closing those whose download was deleted.
    pub fn update(&self, table: &DownloadTable) {
        let mut gone = Vec::new();
        for (id, (window, seen)) in self.windows.borrow().iter() {
            match table.get(*id) {
                Some(download) => {
                    seen.set(true);
                    fill(window, download, table);
                }
                None if seen.get() => gone.push(*id),
                None => {}
            }
        }
        for id in gone {
            self.close(id);
        }
    }

    /// The options chosen for download `id`, or the defaults.
    pub fn options_for(&self, id: DownloadId) -> DownloadOptions {
        self.options.borrow().get(&id).cloned().unwrap_or_default()
    }

    /// Hides and forgets download `id`'s dialog.
    pub fn close(&self, id: DownloadId) {
        if let Some((window, _)) = self.windows.borrow_mut().remove(&id) {
            let _ = window.hide();
        }
    }

    /// Whether download `id` has an open dialog.
    #[cfg(test)]
    pub fn is_open(&self, id: DownloadId) -> bool {
        self.windows.borrow().contains_key(&id)
    }

    /// Download `id`'s dialog, if open.
    #[cfg(test)]
    pub fn window(&self, id: DownloadId) -> Option<ProgressWindow> {
        self.windows
            .borrow()
            .get(&id)
            .map(|(w, _)| w.clone_strong())
    }
}

/// Fills `window` with `download`'s current state.
fn fill(window: &ProgressWindow, download: &Download, table: &DownloadTable) {
    let downloaded = table.downloaded(download);
    let running = download.is_running();
    let total = download.total_size.filter(|&t| t > 0);

    window.set_window_title(
        match total {
            Some(total) => format!(
                "{} {}",
                whole_percent(downloaded, total),
                download.file_name
            ),
            None => download.file_name.clone(),
        }
        .into(),
    );
    window.set_url(download.url.as_str().into());
    window.set_status(
        match &download.state {
            DownloadState::Queued => "Waiting".into(),
            DownloadState::Probing => "Connecting…".into(),
            DownloadState::Downloading => "Receiving data…".into(),
            DownloadState::Paused => "Paused".into(),
            DownloadState::Merging => "Joining the parts…".into(),
            DownloadState::Complete => "Complete".into(),
            DownloadState::Failed(message) => message.clone(),
        }
        .into(),
    );
    window.set_failed(matches!(download.state, DownloadState::Failed(_)));
    window.set_size(total.map_or("Unknown".into(), format::size).into());
    window.set_downloaded(
        match total {
            Some(total) => format!(
                "{} ({})",
                format::size(downloaded),
                format::percent(downloaded, total)
            ),
            None => format::size(downloaded),
        }
        .into(),
    );
    window.set_rate(
        if running {
            format::rate(table.rate_of(download.id))
        } else {
            String::new()
        }
        .into(),
    );
    window.set_time_left(
        table
            .time_left(download)
            .filter(|_| running)
            .map(format::duration)
            .unwrap_or_default()
            .into(),
    );
    let probed = download.total_size.is_some() || download.resumable;
    window.set_resume_capability(
        match (probed, download.resumable) {
            (false, _) => "Unknown",
            (true, true) => "Yes",
            (true, false) => "No",
        }
        .into(),
    );
    window.set_progress(match total {
        Some(_) => table.fraction(download) as f32,
        None if download.state == DownloadState::Complete => 1.0,
        None => -1.0,
    });
    window.set_running(running);
    window.set_can_resume(matches!(
        download.state,
        DownloadState::Queued | DownloadState::Paused | DownloadState::Failed(_)
    ));

    let segments = table.segments(download);
    let views: Vec<SegmentView> = total
        .map(|total| {
            segments
                .iter()
                .map(|s| SegmentView {
                    start: (s.start as f64 / total as f64) as f32,
                    done: (s.downloaded as f64 / total as f64) as f32,
                })
                .collect()
        })
        .unwrap_or_default();
    window.set_segments(ModelRc::new(VecModel::from(views)));
    let connections: Vec<ConnectionRow> = segments
        .iter()
        .enumerate()
        .map(|(index, s)| ConnectionRow {
            number: index as i32 + 1,
            downloaded: format::size(s.downloaded).into(),
            info: match s.state {
                SegmentState::Active if running && s.downloaded == 0 => "Connecting…",
                SegmentState::Active if running => "Receiving data…",
                SegmentState::Done => "Finished",
                _ => "Waiting",
            }
            .into(),
        })
        .collect();
    window.set_connections(ModelRc::new(VecModel::from(connections)));
}

/// `done` of `total` as a whole percentage, rounded down, such as `45%`.
fn whole_percent(done: u64, total: u64) -> String {
    format!("{}%", u128::from(done) * 100 / u128::from(total.max(1)))
}

#[cfg(test)]
mod tests {
    use slint::Model;
    use velo_core::{ProgressUpdate, Segment, SegmentId};

    use super::*;
    use crate::app::tests::Recorder;
    use crate::list::tests::sample;

    fn dialogs() -> (Rc<ProgressDialogs>, Recorder) {
        i_slint_backend_testing::init_no_event_loop();
        let recorder = Recorder::default();
        let config = Rc::new(Config::new(Default::default(), None, "/d".into()));
        (
            ProgressDialogs::new(Rc::new(recorder.clone()), config),
            recorder,
        )
    }

    /// A running 1000-byte download with two connections, the first further along.
    fn running() -> (Download, ProgressUpdate) {
        let mut download = sample(7, "big.iso", DownloadState::Downloading, 0, 1000);
        download.resumable = true;
        let mut first = Segment::new(SegmentId(0), 0, 500);
        first.downloaded = 300;
        first.state = SegmentState::Active;
        let mut second = Segment::new(SegmentId(1), 500, 1000);
        second.state = SegmentState::Active;
        let update = ProgressUpdate {
            id: download.id,
            downloaded: 300,
            total: Some(1000),
            rate: 100,
            segments: vec![first, second],
        };
        (download, update)
    }

    #[test]
    fn a_running_download_shows_its_numbers_segments_and_connections() {
        let (dialogs, _) = dialogs();
        let (download, update) = running();
        let mut table = DownloadTable::default();
        table.upsert(download.clone());
        table.set_progress(update);
        dialogs.open(download.id, &table).unwrap();
        let window = dialogs.window(download.id).unwrap();

        assert_eq!(window.get_window_title(), "30% big.iso");
        assert_eq!(window.get_status(), "Receiving data…");
        assert_eq!(window.get_downloaded(), "300 B (30.00%)");
        assert_eq!(window.get_rate(), "100 B/sec");
        assert_eq!(window.get_time_left(), "7 sec");
        assert_eq!(window.get_resume_capability(), "Yes");
        assert!((window.get_progress() - 0.3).abs() < 1e-6);
        assert!(window.get_running());
        let segments = window.get_segments();
        assert_eq!(segments.row_count(), 2);
        assert_eq!(segments.row_data(1).unwrap().start, 0.5);
        assert!((segments.row_data(0).unwrap().done - 0.3).abs() < 1e-6);
        let connections = window.get_connections();
        assert_eq!(connections.row_data(0).unwrap().info, "Receiving data…");
        assert_eq!(connections.row_data(1).unwrap().info, "Connecting…");
    }

    #[test]
    fn the_speed_limiter_and_completion_options_are_kept_per_download() {
        let (dialogs, recorder) = dialogs();
        let (download, _) = running();
        let mut table = DownloadTable::default();
        table.upsert(download.clone());
        dialogs.open(download.id, &table).unwrap();
        let window = dialogs.window(download.id).unwrap();
        window.set_limit_enabled(true);
        window.set_limit_kb(250);
        window.invoke_limit_changed();
        window.set_exit_when_done(true);
        window.invoke_completion_changed();
        assert_eq!(*recorder.0.borrow(), ["limit 7 256000"]);
        let options = dialogs.options_for(download.id);
        assert_eq!(options.limit_kb, Some(250));
        assert!(options.exit && options.show_complete && !options.shutdown);

        // Reopening shows the same choices.
        dialogs.close(download.id);
        dialogs.open(download.id, &table).unwrap();
        let window = dialogs.window(download.id).unwrap();
        assert!(window.get_limit_enabled() && window.get_exit_when_done());
        assert_eq!(window.get_limit_kb(), 250);
        window.set_limit_enabled(false);
        window.invoke_limit_changed();
        assert_eq!(recorder.0.borrow().last().unwrap(), "limit 7 none");
    }

    #[test]
    fn buttons_send_commands_and_cancel_closes() {
        let (dialogs, recorder) = dialogs();
        let (download, _) = running();
        let mut table = DownloadTable::default();
        table.upsert(download.clone());
        dialogs.open(download.id, &table).unwrap();
        let window = dialogs.window(download.id).unwrap();
        window.invoke_pause();
        window.invoke_resume();
        window.invoke_cancel();
        assert!(!dialogs.is_open(download.id));
        assert_eq!(*recorder.0.borrow(), ["pause 7", "resume 7", "pause 7"]);
    }

    #[test]
    fn a_dialog_waits_for_its_download_and_closes_when_it_is_deleted() {
        let (dialogs, _) = dialogs();
        let mut table = DownloadTable::default();
        let id = DownloadId(3);
        // Opened right after adding, before the engine reports the download.
        dialogs.open(id, &table).unwrap();
        dialogs.update(&table);
        assert!(dialogs.is_open(id));

        table.upsert(sample(3, "a.zip", DownloadState::Complete, 10, 10));
        dialogs.update(&table);
        let window = dialogs.window(id).unwrap();
        assert_eq!(window.get_status(), "Complete");
        assert_eq!(window.get_window_title(), "100% a.zip");
        assert!(!window.get_running() && !window.get_can_resume());

        table.remove(id);
        dialogs.update(&table);
        assert!(!dialogs.is_open(id));
    }
}
