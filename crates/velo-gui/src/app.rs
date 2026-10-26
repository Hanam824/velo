//! Connects the main window to the app's state: the download table, the Categories tree and
//! the engine's commands.
//!
//! Everything here runs on the UI thread. Engine events reach it as plain data through
//! [`App::apply`].

use std::cell::RefCell;
use std::rc::Rc;

use slint::{ComponentHandle, Model, VecModel};
use std::collections::HashSet;

use velo_core::{Download, DownloadId, DownloadState, EngineEvent, Queue, QueueId};
use velo_engine::{DownloadEdit, Engine, NewDownload};

use crate::categories::CategoryTree;
use crate::category::CategoryDialog;
use crate::complete::{Completion, Desktop};
use crate::config::Config;
use crate::list::{Column, DownloadTable, Filter};
use crate::progress::ProgressDialogs;
use crate::properties::PropertiesDialogs;
use crate::scheduler::{Queues, SchedulerDialog};
use crate::toolbar::ToolbarState;
use crate::{AppWindow, CategoryItem, DownloadRow, QueueChoice, format, theme};

/// The engine commands the main window sends.
pub trait Commands {
    /// Adds a download and returns its id.
    fn add(&self, new: NewDownload) -> DownloadId;
    /// Starts or resumes download `id`.
    fn resume(&self, id: DownloadId);
    /// Pauses download `id`.
    fn pause(&self, id: DownloadId);
    /// Pauses every download.
    fn pause_all(&self);
    /// Deletes download `id`, and its finished file with `delete_file`.
    fn delete(&self, id: DownloadId, delete_file: bool);
    /// Limits download `id` to `rate` bytes per second, or removes its own limit.
    fn set_speed_limit(&self, id: DownloadId, rate: Option<u64>);
    /// Changes download `id`'s details.
    fn edit(&self, id: DownloadId, edit: DownloadEdit);
    /// Uses `settings` from now on.
    fn set_settings(&self, settings: velo_core::Settings);
    /// Adds an empty queue named `name` and returns its id.
    fn add_queue(&self, name: &str) -> QueueId;
    /// Saves a queue's name, concurrency, schedule and order.
    fn save_queue(&self, queue: Queue);
    /// Deletes queue `id`.
    fn delete_queue(&self, id: QueueId);
    /// Moves `downloads` to queue `queue`.
    fn enqueue(&self, queue: QueueId, downloads: Vec<DownloadId>);
    /// Takes `downloads` out of their queues.
    fn dequeue(&self, downloads: Vec<DownloadId>);
    /// Starts queue `id`.
    fn start_queue(&self, id: QueueId);
    /// Stops queue `id`.
    fn stop_queue(&self, id: QueueId);
}

impl Commands for Engine {
    fn add(&self, new: NewDownload) -> DownloadId {
        Engine::add(self, new)
    }

    fn resume(&self, id: DownloadId) {
        Engine::resume(self, id);
    }

    fn pause(&self, id: DownloadId) {
        Engine::pause(self, id);
    }

    fn pause_all(&self) {
        Engine::pause_all(self);
    }

    fn delete(&self, id: DownloadId, delete_file: bool) {
        Engine::delete(self, id, delete_file);
    }

    fn set_speed_limit(&self, id: DownloadId, rate: Option<u64>) {
        Engine::set_speed_limit(self, id, rate);
    }

    fn edit(&self, id: DownloadId, edit: DownloadEdit) {
        Engine::edit(self, id, edit);
    }

    fn set_settings(&self, settings: velo_core::Settings) {
        Engine::set_settings(self, settings);
    }

    fn add_queue(&self, name: &str) -> QueueId {
        Engine::add_queue(self, name)
    }

    fn save_queue(&self, queue: Queue) {
        Engine::save_queue(self, queue);
    }

    fn delete_queue(&self, id: QueueId) {
        Engine::delete_queue(self, id);
    }

    fn enqueue(&self, queue: QueueId, downloads: Vec<DownloadId>) {
        Engine::enqueue(self, queue, downloads);
    }

    fn dequeue(&self, downloads: Vec<DownloadId>) {
        Engine::dequeue(self, downloads);
    }

    fn start_queue(&self, id: QueueId) {
        Engine::start_queue(self, id);
    }

    fn stop_queue(&self, id: QueueId) {
        Engine::stop_queue(self, id);
    }
}

/// The main window's state and the code that keeps the window in step with it.
pub struct App {
    ui: slint::Weak<AppWindow>,
    commands: Rc<dyn Commands>,
    table: RefCell<DownloadTable>,
    rows: Rc<VecModel<DownloadRow>>,
    tree: RefCell<CategoryTree>,
    category_rows: Rc<VecModel<CategoryItem>>,
    progress: Rc<ProgressDialogs>,
    properties: Rc<PropertiesDialogs>,
    completion: Rc<Completion>,
    desktop: Rc<dyn Desktop>,
    config: Rc<Config>,
    category_dialog: Rc<CategoryDialog>,
    queues: Queues,
    /// The queues that run.
    running_queues: RefCell<HashSet<QueueId>>,
    queue_choices: Rc<VecModel<QueueChoice>>,
    scheduler: Rc<SchedulerDialog>,
}

impl App {
    /// Fills `ui` from the settings in `config` and connects its callbacks, sending commands to
    /// `commands` and desktop actions to `desktop`.
    pub fn new(
        ui: &AppWindow,
        config: Rc<Config>,
        commands: Rc<dyn Commands>,
        desktop: Rc<dyn Desktop>,
    ) -> Rc<Self> {
        let settings = config.settings();
        theme::apply(ui, settings.appearance);
        let tree = CategoryTree::new(&settings.categories, &[]);
        let queues: Queues = Rc::default();
        let app = Rc::new(Self {
            scheduler: SchedulerDialog::new(commands.clone(), config.clone(), queues.clone()),
            queues,
            running_queues: RefCell::default(),
            queue_choices: Rc::new(VecModel::default()),
            ui: ui.as_weak(),
            progress: ProgressDialogs::new(commands.clone(), config.clone()),
            properties: PropertiesDialogs::new(commands.clone(), desktop.clone(), config.clone()),
            category_dialog: CategoryDialog::new(
                config.clone(),
                commands.clone(),
                config.default_downloads().to_path_buf(),
            ),
            config: config.clone(),
            completion: Completion::new(config, desktop.clone()),
            desktop,
            commands,
            table: RefCell::default(),
            rows: Rc::new(VecModel::default()),
            category_rows: Rc::new(VecModel::from(tree.visible())),
            tree: RefCell::new(tree),
        });
        ui.set_downloads(app.rows.clone().into());
        ui.set_categories(app.category_rows.clone().into());
        ui.set_queue_choices(app.queue_choices.clone().into());
        app.connect(ui);
        app.refresh();
        app
    }

    /// Connects the window's callbacks to this app.
    fn connect(self: &Rc<Self>, ui: &AppWindow) {
        ui.on_category_toggled(with(self, |app, key: slint::SharedString| {
            app.tree.borrow_mut().toggle(&key);
            app.category_rows.set_vec(app.tree.borrow().visible());
        }));
        ui.on_category_selected(with(self, |app, key: slint::SharedString| {
            app.table.borrow_mut().set_filter(Filter::from_key(&key));
            app.refresh();
        }));
        ui.on_add_category(with0(self, |app| app.open_category(None)));
        ui.on_edit_category(with(self, |app, key: slint::SharedString| {
            app.open_category(category_name(&key));
        }));
        ui.on_delete_category(with(self, |app, key: slint::SharedString| {
            if let Some(name) = category_name(&key) {
                let downloads = app.table.borrow().downloads().to_vec();
                app.category_dialog.delete(&name, &downloads);
            }
        }));
        let this = Rc::downgrade(self);
        self.category_dialog.on_changed(move || {
            if let Some(app) = this.upgrade() {
                app.rebuild_categories();
            }
        });
        ui.on_selection_changed(with(self, |app, _| app.refresh()));
        ui.on_scheduler(with0(self, |app| {
            let target = app.target_queue();
            let running = app.running_queues.borrow().clone();
            if let Err(error) = app.scheduler.open(target, &running, &app.table.borrow()) {
                eprintln!("velo: couldn't open the Scheduler: {error}");
            }
        }));
        ui.on_start_queue(with0(self, |app| {
            app.commands.start_queue(app.target_queue())
        }));
        ui.on_stop_queue(with0(self, |app| {
            app.commands.stop_queue(app.target_queue())
        }));
        let enqueue = with(self, |app, (raw, queue): (i32, i32)| {
            if let (Some(id), Ok(queue)) = (download_id(raw), u32::try_from(queue)) {
                app.commands.enqueue(QueueId(queue), vec![id]);
            }
        });
        ui.on_enqueue(move |raw, queue| enqueue((raw, queue)));
        ui.on_dequeue(on_row(self, |app, id| app.commands.dequeue(vec![id])));
        ui.on_sort(with(self, |app, column| {
            if let Some(column) = Column::from_index(column) {
                app.table.borrow_mut().sort_by(column);
                app.refresh();
            }
        }));
        ui.on_move_selection(with(self, |app, delta| {
            let next = app.table.borrow().neighbour(app.selected(), delta);
            if let (Some(ui), Some(next)) = (app.ui.upgrade(), next) {
                ui.set_selected_id(next.0 as i32);
                app.refresh();
            }
        }));

        // Toolbar: act on the selection.
        ui.on_resume(on_selected(self, |app, id| app.commands.resume(id)));
        ui.on_stop(on_selected(self, |app, id| app.commands.pause(id)));
        ui.on_delete(on_selected(self, App::ask_delete));
        ui.on_stop_all(with0(self, |app| app.commands.pause_all()));
        ui.on_delete_completed(with0(self, App::delete_completed));

        // Context menu: act on the row's download.
        ui.on_resume_download(on_row(self, |app, id| app.commands.resume(id)));
        ui.on_stop_download(on_row(self, |app, id| app.commands.pause(id)));
        ui.on_delete_download(on_row(self, App::ask_delete));
        let confirmed = with(self, |app, (raw, delete_file)| {
            if let Some(id) = download_id(raw) {
                app.commands.delete(id, delete_file);
            }
        });
        ui.on_download_activated(on_row(self, App::show_progress));
        ui.on_appearance_changed(with(self, |app, appearance| {
            app.config
                .update(|s| s.appearance = theme::from_ui(appearance));
        }));
        ui.on_open_file(on_row(self, |app, id| {
            if let Some(path) = app.finished_file(id) {
                app.desktop.open(&path);
            }
        }));
        ui.on_open_folder(on_row(self, |app, id| {
            if let Some(path) = app.finished_file(id) {
                app.desktop.reveal(&path);
            }
        }));
        ui.on_properties(on_row(self, |app, id| {
            let download = app.table.borrow().get(id).cloned();
            if let Some(download) = download
                && let Err(error) = app.properties.open(&download)
            {
                eprintln!("velo: couldn't open Properties: {error}");
            }
        }));
        ui.on_delete_confirmed(move |raw, delete_file| confirmed((raw, delete_file)));
    }

    /// Asks the user to confirm deleting download `id`.
    fn ask_delete(&self, id: DownloadId) {
        let Some(ui) = self.ui.upgrade() else { return };
        let Some(download) = self.table.borrow().get(id).cloned() else {
            return;
        };
        ui.invoke_ask_delete(
            id.0 as i32,
            download.file_name.into(),
            download.state == DownloadState::Complete,
        );
    }

    /// Opens download `id`'s progress dialog.
    pub fn show_progress(&self, id: DownloadId) {
        if let Err(error) = self.progress.open(id, &self.table.borrow()) {
            eprintln!("velo: couldn't open the progress dialog: {error}");
        }
    }

    /// Download `id`'s progress dialog, if open.
    #[cfg(test)]
    pub fn progress_window(&self, id: DownloadId) -> Option<crate::ProgressWindow> {
        self.progress.window(id)
    }

    /// The queue the toolbar's Start Queue and Stop Queue act on: the one selected in the tree,
    /// else the main queue.
    fn target_queue(&self) -> QueueId {
        let selected = self
            .ui
            .upgrade()
            .map(|ui| ui.get_selected_category().to_string())
            .unwrap_or_default();
        selected
            .strip_prefix("queue/")
            .and_then(|id| id.parse().ok())
            .map(QueueId)
            .filter(|id| self.queues.borrow().contains_key(id))
            .unwrap_or(QueueId::MAIN)
    }

    /// Takes in a change to the queues: the table's membership, the tree and the menu.
    fn queues_changed(&self) {
        let queues = self.queues.borrow();
        self.table.borrow_mut().set_queues(queues.values());
        let choices: Vec<QueueChoice> = queues
            .values()
            .map(|q| QueueChoice {
                id: q.id.0 as i32,
                name: q.name.as_str().into(),
            })
            .collect();
        self.queue_choices.set_vec(choices);
        drop(queues);
        self.rebuild_categories();
    }

    /// Opens the category dialog to add one, or to edit category `name`.
    fn open_category(&self, name: Option<String>) {
        // The downloads as they are when the dialog opens, to re-file on a rename.
        let downloads = self.table.borrow().downloads().to_vec();
        if let Err(error) = self
            .category_dialog
            .open(name, Rc::new(move || downloads.clone()))
        {
            eprintln!("velo: couldn't open the category dialog: {error}");
        }
    }

    /// Rebuilds the Categories tree after the categories changed, keeping the selection if it
    /// still exists.
    fn rebuild_categories(&self) {
        let settings = self.config.settings();
        let queues: Vec<Queue> = self.queues.borrow().values().cloned().collect();
        let mut tree = self.tree.borrow_mut();
        tree.rebuild(&settings.categories, &queues);
        self.category_rows.set_vec(tree.visible());
        if let Some(ui) = self.ui.upgrade()
            && !tree.contains(&ui.get_selected_category())
        {
            ui.set_selected_category("all".into());
            self.table.borrow_mut().set_filter(Filter::default());
        }
        drop(tree);
        self.refresh();
    }

    /// The saved file of download `id`, if it is complete.
    fn finished_file(&self, id: DownloadId) -> Option<std::path::PathBuf> {
        let table = self.table.borrow();
        let download = table.get(id).filter(|d| d.is_finished())?;
        Some(download.save_path())
    }

    /// Removes every complete download from the list, keeping their files.
    fn delete_completed(&self) {
        let complete: Vec<DownloadId> = self
            .table
            .borrow()
            .downloads()
            .iter()
            .filter(|d| d.is_finished())
            .map(|d| d.id)
            .collect();
        for id in complete {
            self.commands.delete(id, false);
        }
    }

    /// Updates the table from engine events and redraws once.
    pub fn apply(&self, events: impl IntoIterator<Item = EngineEvent>) {
        let mut changed = false;
        let mut queues_changed = false;
        let mut finished: Vec<Download> = Vec::new();
        let mut finished_queues: Vec<QueueId> = Vec::new();
        {
            let mut table = self.table.borrow_mut();
            for event in events {
                changed = true;
                match event {
                    EngineEvent::Added(download) | EngineEvent::Changed(download) => {
                        let was_complete = table.get(download.id).map(|d| d.is_finished());
                        if was_complete == Some(false) && download.is_finished() {
                            finished.push(download.clone());
                        }
                        table.upsert(download)
                    }
                    EngineEvent::Removed(id) => table.remove(id),
                    EngineEvent::Progress(update) => table.set_progress(update),
                    EngineEvent::QueueChanged(queue) => {
                        self.queues.borrow_mut().insert(queue.id, queue);
                        queues_changed = true;
                    }
                    EngineEvent::QueueRemoved(id) => {
                        self.queues.borrow_mut().remove(&id);
                        self.running_queues.borrow_mut().remove(&id);
                        queues_changed = true;
                    }
                    EngineEvent::QueueRunning(id, running) => {
                        let mut set = self.running_queues.borrow_mut();
                        if running {
                            set.insert(id);
                        } else {
                            set.remove(&id);
                        }
                    }
                    EngineEvent::QueueFinished(id) => finished_queues.push(id),
                }
            }
        }
        if queues_changed {
            self.queues_changed();
        }
        if changed {
            self.refresh();
        }
        for download in finished {
            let options = self.progress.options_for(download.id);
            self.progress.close(download.id);
            self.completion.finished(&download, &options);
        }
        for id in finished_queues {
            let queue = self.queues.borrow().get(&id).cloned();
            if let Some(queue) = queue {
                self.completion.queue_finished(&queue);
            }
        }
    }

    /// The selected download, if it still exists.
    pub fn selected(&self) -> Option<DownloadId> {
        let id = download_id(self.ui.upgrade()?.get_selected_id())?;
        self.table.borrow().is_visible(id).then_some(id)
    }

    /// Pushes the table into the window: rows, sort arrows, toolbar state and status bar.
    pub fn refresh(&self) {
        let Some(ui) = self.ui.upgrade() else { return };
        let table = self.table.borrow();
        let rows = table.rows();
        // Replace rows in place where possible, so the list keeps its scroll position.
        if rows.len() == self.rows.row_count() {
            for (index, row) in rows.into_iter().enumerate() {
                if self.rows.row_data(index).as_ref() != Some(&row) {
                    self.rows.set_row_data(index, row);
                }
            }
        } else {
            self.rows.set_vec(rows);
        }
        let (column, ascending) = table.sort_order();
        ui.set_sort_column(column.index());
        ui.set_sort_ascending(ascending);

        let selected = self.selected().and_then(|id| table.get(id));
        if selected.is_none() {
            ui.set_selected_id(-1);
        }
        ToolbarState::new(selected, table.downloads()).apply(&ui);
        let target_running = self.running_queues.borrow().contains(&self.target_queue());
        ui.set_can_start_queue(!target_running);
        ui.set_can_stop_queue(target_running);

        let (shown, total) = (table.visible_count(), table.downloads().len());
        let noun = if total == 1 { "download" } else { "downloads" };
        ui.set_status_message(if shown == total {
            format!("{total} {noun}").into()
        } else {
            format!("{shown} of {total} {noun}").into()
        });
        let total_rate: u64 = table.downloads().iter().map(|d| table.rate_of(d.id)).sum();
        ui.set_status_summary(format::rate(total_rate).into());
        self.progress.update(&table);
        self.scheduler.update(&self.running_queues.borrow(), &table);
    }
}

/// Wraps `f` as a window callback that runs with the app, if it still exists.
fn with<A>(app: &Rc<App>, f: impl Fn(&App, A) + 'static) -> impl Fn(A) + 'static {
    let app = Rc::downgrade(app);
    move |arg| {
        if let Some(app) = app.upgrade() {
            f(&app, arg);
        }
    }
}

/// [`with`] for a callback without arguments.
fn with0(app: &Rc<App>, f: impl Fn(&App) + 'static) -> impl Fn() + 'static {
    let callback = with(app, move |app, ()| f(app));
    move || callback(())
}

/// Wraps `f` as a toolbar callback that runs on the selected download, if any.
fn on_selected(app: &Rc<App>, f: impl Fn(&App, DownloadId) + 'static) -> impl Fn() + 'static {
    with0(app, move |app| {
        if let Some(id) = app.selected() {
            f(app, id);
        }
    })
}

/// Wraps `f` as a context-menu callback that runs on the row's download.
fn on_row(app: &Rc<App>, f: impl Fn(&App, DownloadId) + 'static) -> impl Fn(i32) + 'static {
    with(app, move |app, raw| {
        if let Some(id) = download_id(raw) {
            f(app, id);
        }
    })
}

/// The category name in a Categories tree key such as `all/Video`.
fn category_name(key: &str) -> Option<String> {
    key.split_once('/')
        .filter(|(group, _)| matches!(*group, "all" | "unfinished" | "finished"))
        .map(|(_, name)| name.to_string())
}

/// A download id from the UI's `int`, which is -1 for none.
fn download_id(raw: i32) -> Option<DownloadId> {
    u64::try_from(raw).ok().map(DownloadId)
}

#[cfg(test)]
pub mod tests {
    use velo_core::{DownloadState, ProgressUpdate};

    use super::*;
    use crate::complete::tests::FakeDesktop;
    use crate::list::tests::sample;

    /// Records the commands it is sent.
    #[derive(Default, Clone)]
    pub struct Recorder(pub Rc<RefCell<Vec<String>>>);

    impl Commands for Recorder {
        fn add(&self, new: NewDownload) -> DownloadId {
            let user = new.request.credentials.map(|c| c.user).unwrap_or_default();
            self.0.borrow_mut().push(format!(
                "add {} {} {} {} start={} user={user}",
                new.url,
                new.save_dir.join(&new.file_name).display(),
                new.category.unwrap_or_default(),
                new.description,
                new.start,
            ));
            DownloadId(99)
        }
        fn resume(&self, id: DownloadId) {
            self.0.borrow_mut().push(format!("resume {}", id.0));
        }
        fn pause(&self, id: DownloadId) {
            self.0.borrow_mut().push(format!("pause {}", id.0));
        }
        fn pause_all(&self) {
            self.0.borrow_mut().push("pause all".into());
        }
        fn delete(&self, id: DownloadId, delete_file: bool) {
            self.0
                .borrow_mut()
                .push(format!("delete {} {delete_file}", id.0));
        }
        fn set_speed_limit(&self, id: DownloadId, rate: Option<u64>) {
            let rate = rate.map_or("none".into(), |r| r.to_string());
            self.0.borrow_mut().push(format!("limit {} {rate}", id.0));
        }
        fn edit(&self, id: DownloadId, edit: DownloadEdit) {
            self.0.borrow_mut().push(format!(
                "edit {} name={} url={} desc={}",
                id.0,
                edit.file_name.unwrap_or_default(),
                edit.url.unwrap_or_default(),
                edit.description.unwrap_or_default()
            ));
        }
        fn set_settings(&self, settings: velo_core::Settings) {
            self.0
                .borrow_mut()
                .push(format!("settings {}", settings.max_connections));
        }
        fn add_queue(&self, name: &str) -> QueueId {
            self.0.borrow_mut().push(format!("add queue {name}"));
            QueueId(5)
        }
        fn save_queue(&self, queue: Queue) {
            let ids = |ids: &[DownloadId]| {
                ids.iter()
                    .map(|id| id.0.to_string())
                    .collect::<Vec<_>>()
                    .join(",")
            };
            let s = &queue.schedule;
            let days: String = s.days.iter().map(|&d| if d { '1' } else { '0' }).collect();
            self.0.borrow_mut().push(format!(
                "save queue {} order={} max={} start={} days={days} shutdown={}",
                queue.id.0,
                ids(&queue.downloads),
                queue.max_concurrent,
                s.start_at.map(|t| t.to_string()).unwrap_or_default(),
                s.shutdown_when_done
            ));
        }
        fn delete_queue(&self, id: QueueId) {
            self.0.borrow_mut().push(format!("delete queue {}", id.0));
        }
        fn enqueue(&self, queue: QueueId, downloads: Vec<DownloadId>) {
            let ids: Vec<String> = downloads.iter().map(|d| d.0.to_string()).collect();
            self.0
                .borrow_mut()
                .push(format!("enqueue {} {}", queue.0, ids.join(",")));
        }
        fn dequeue(&self, downloads: Vec<DownloadId>) {
            let ids: Vec<String> = downloads.iter().map(|d| d.0.to_string()).collect();
            self.0
                .borrow_mut()
                .push(format!("dequeue {}", ids.join(",")));
        }
        fn start_queue(&self, id: QueueId) {
            self.0.borrow_mut().push(format!("start queue {}", id.0));
        }
        fn stop_queue(&self, id: QueueId) {
            self.0.borrow_mut().push(format!("stop queue {}", id.0));
        }
    }

    fn app() -> (AppWindow, Rc<App>, Recorder) {
        let (ui, app, recorder, _) = app_with_desktop();
        (ui, app, recorder)
    }

    fn app_with_desktop() -> (AppWindow, Rc<App>, Recorder, FakeDesktop) {
        i_slint_backend_testing::init_no_event_loop();
        let ui = AppWindow::new().unwrap();
        let recorder = Recorder::default();
        let desktop = FakeDesktop::default();
        let config = Rc::new(Config::new(
            velo_core::Settings::default(),
            None,
            "/downloads".into(),
        ));
        let app = App::new(
            &ui,
            config,
            Rc::new(recorder.clone()),
            Rc::new(desktop.clone()),
        );
        (ui, app, recorder, desktop)
    }

    #[test]
    fn the_saved_appearance_reaches_every_window_and_menu_changes_are_saved() {
        i_slint_backend_testing::init_no_event_loop();
        let ui = AppWindow::new().unwrap();
        let settings = velo_core::Settings {
            appearance: velo_core::Appearance::Dark,
            ..velo_core::Settings::default()
        };
        let config = Rc::new(Config::new(settings, None, "/downloads".into()));
        let app = App::new(
            &ui,
            config.clone(),
            Rc::new(Recorder::default()),
            Rc::new(FakeDesktop::default()),
        );
        assert!(ui.global::<crate::Theme>().get_dark());

        // A dialog opened later is dark too.
        app.apply([EngineEvent::Added(sample(
            1,
            "a.iso",
            DownloadState::Paused,
            1,
            2,
        ))]);
        ui.invoke_download_activated(1);
        let dialog = app.progress_window(DownloadId(1)).unwrap();
        assert!(dialog.global::<crate::Theme>().get_dark());

        ui.invoke_appearance_changed(crate::Appearance::Light);
        assert_eq!(config.settings().appearance, velo_core::Appearance::Light);
    }

    #[test]
    fn the_tree_filters_the_list_and_category_changes_rebuild_it() {
        let (ui, app, _) = app();
        let mut video = sample(1, "a.mkv", DownloadState::Complete, 1, 1);
        video.category = Some("Video".into());
        app.apply([
            EngineEvent::Added(video),
            EngineEvent::Added(sample(2, "b.dat", DownloadState::Paused, 0, 1)),
        ]);
        ui.set_selected_id(2);
        ui.invoke_category_selected("all/Video".into());
        assert_eq!(ui.get_downloads().row_count(), 1);
        assert_eq!(ui.get_status_message(), "1 of 2 downloads");
        // The selection is hidden by the filter, so nothing is selected.
        assert_eq!(ui.get_selected_id(), -1);

        // Deleting the selected category falls back to All Downloads.
        ui.set_selected_category("all/Video".into());
        ui.invoke_delete_category("all/Video".into());
        assert_eq!(ui.get_selected_category(), "all");
        assert_eq!(ui.get_downloads().row_count(), 2);
        let keys: Vec<String> = ui
            .get_categories()
            .iter()
            .map(|row| row.key.to_string())
            .collect();
        assert!(!keys.contains(&"all/Video".to_string()));
        assert_eq!(category_name("unfinished/Music").as_deref(), Some("Music"));
        assert_eq!(category_name("queue/0"), None);
    }

    #[test]
    fn queues_reach_the_list_tree_toolbar_and_menu() {
        let (ui, app, recorder) = app();
        app.apply([
            EngineEvent::Added(sample(1, "a.zip", DownloadState::Paused, 0, 1)),
            EngineEvent::Added(sample(2, "b.zip", DownloadState::Paused, 0, 1)),
        ]);
        let mut main = Queue::main();
        main.downloads = vec![DownloadId(2)];
        app.apply([
            EngineEvent::QueueChanged(main),
            EngineEvent::QueueChanged(Queue::new(QueueId(3), "Night")),
        ]);
        let keys: Vec<String> = ui
            .get_categories()
            .iter()
            .map(|r| r.key.to_string())
            .collect();
        assert!(keys.contains(&"queue/3".to_string()), "{keys:?}");
        assert_eq!(ui.get_queue_choices().row_count(), 2);
        let queued: Vec<String> = ui
            .get_downloads()
            .iter()
            .map(|r| r.queue.to_string())
            .collect();
        assert!(queued.contains(&"1".to_string()));

        // The toolbar acts on the queue selected in the tree.
        ui.set_selected_category("queue/3".into());
        ui.invoke_category_selected("queue/3".into());
        assert_eq!(ui.get_downloads().row_count(), 0);
        assert!(ui.get_can_start_queue() && !ui.get_can_stop_queue());
        ui.invoke_start_queue();
        app.apply([EngineEvent::QueueRunning(QueueId(3), true)]);
        assert!(!ui.get_can_start_queue() && ui.get_can_stop_queue());
        ui.invoke_stop_queue();
        ui.invoke_enqueue(1, 3);
        ui.invoke_dequeue(2);
        assert_eq!(
            *recorder.0.borrow(),
            ["start queue 3", "stop queue 3", "enqueue 3 1", "dequeue 2"]
        );

        // Deleting the selected queue falls back to All Downloads.
        app.apply([EngineEvent::QueueRemoved(QueueId(3))]);
        assert_eq!(ui.get_selected_category(), "all");
    }

    #[test]
    fn a_finished_download_closes_its_progress_and_shows_complete() {
        let (ui, app, _, desktop) = app_with_desktop();
        let running = sample(6, "a.zip", DownloadState::Downloading, 5, 10);
        app.apply([EngineEvent::Added(running.clone())]);
        ui.invoke_download_activated(6);
        assert!(app.progress.is_open(DownloadId(6)));

        let done = sample(6, "a.zip", DownloadState::Complete, 10, 10);
        app.apply([EngineEvent::Changed(done.clone())]);
        assert!(!app.progress.is_open(DownloadId(6)));
        assert!(app.completion.window(DownloadId(6)).is_some());

        // Open and Open Folder from the context menu act on the saved file.
        ui.invoke_open_file(6);
        ui.invoke_open_folder(6);
        let path = done.save_path().display().to_string();
        assert_eq!(
            *desktop.0.borrow(),
            [format!("open {path}"), format!("reveal {path}")]
        );
        ui.invoke_properties(6);
        assert!(app.properties.window(DownloadId(6)).is_some());
    }

    #[test]
    fn engine_events_reach_the_list_toolbar_and_status_bar() {
        let (ui, app, _) = app();
        assert_eq!(ui.get_downloads().row_count(), 0);
        assert_eq!(ui.get_status_message(), "0 downloads");

        let running = sample(1, "a.iso", DownloadState::Downloading, 0, 10_000);
        app.apply([
            EngineEvent::Added(running.clone()),
            EngineEvent::Added(sample(2, "b.zip", DownloadState::Complete, 1, 1)),
            EngineEvent::Progress(ProgressUpdate {
                id: running.id,
                downloaded: 5_000,
                total: Some(10_000),
                rate: 1024,
                segments: Vec::new(),
            }),
        ]);
        assert_eq!(ui.get_downloads().row_count(), 2);
        assert_eq!(ui.get_status_message(), "2 downloads");
        assert_eq!(ui.get_status_summary(), "1.00 KB/sec");
        assert!(ui.get_can_stop_all() && ui.get_can_delete_completed());

        // Selecting the running download enables Stop.
        ui.set_selected_id(1);
        ui.invoke_selection_changed(1);
        assert!(ui.get_can_stop() && !ui.get_can_resume());

        // Arrow keys move through the rows (newest try first); removing the selection clears it.
        ui.invoke_move_selection(-1);
        assert_eq!(ui.get_selected_id(), 2);
        app.apply([EngineEvent::Removed(DownloadId(2))]);
        assert_eq!(ui.get_selected_id(), -1);
        assert_eq!(ui.get_downloads().row_count(), 1);

        // Sorting is reflected in the header.
        ui.invoke_sort(0);
        assert_eq!((ui.get_sort_column(), ui.get_sort_ascending()), (0, true));
    }

    #[test]
    fn activating_a_row_opens_its_progress_dialog() {
        let (ui, app, _) = app();
        app.apply([EngineEvent::Added(sample(
            4,
            "a.iso",
            DownloadState::Paused,
            5,
            10,
        ))]);
        ui.invoke_download_activated(4);
        assert!(app.progress.is_open(DownloadId(4)));
        app.apply([EngineEvent::Removed(DownloadId(4))]);
        assert!(!app.progress.is_open(DownloadId(4)));
    }

    #[test]
    fn toolbar_and_menus_send_engine_commands() {
        let (ui, app, recorder) = app();
        app.apply([
            EngineEvent::Added(sample(1, "a.iso", DownloadState::Paused, 0, 10)),
            EngineEvent::Added(sample(2, "b.zip", DownloadState::Complete, 1, 1)),
            EngineEvent::Added(sample(3, "c.zip", DownloadState::Complete, 1, 1)),
        ]);
        // With nothing selected, Resume does nothing.
        ui.invoke_resume();
        ui.set_selected_id(1);
        ui.invoke_resume();
        ui.invoke_stop();
        ui.invoke_stop_all();
        ui.invoke_stop_download(2);
        ui.invoke_delete_completed();
        // Deleting asks first; only the confirmation sends the command.
        ui.invoke_delete();
        ui.invoke_delete_confirmed(1, false);
        ui.invoke_delete_confirmed(2, true);
        assert_eq!(
            *recorder.0.borrow(),
            [
                "resume 1",
                "pause 1",
                "pause all",
                "pause 2",
                "delete 2 false",
                "delete 3 false",
                "delete 1 false",
                "delete 2 true",
            ]
        );
    }
}
