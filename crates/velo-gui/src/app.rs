//! Connects the main window to the app's state: the download table, the Categories tree and
//! the engine's commands.
//!
//! Everything here runs on the UI thread. Engine events reach it as plain data through
//! [`App::apply`].

use std::cell::RefCell;
use std::rc::Rc;

use slint::{ComponentHandle, Model, VecModel};
use velo_core::{DownloadId, DownloadState, EngineEvent, Settings};
use velo_engine::{Engine, NewDownload};

use crate::categories::CategoryTree;
use crate::list::{Column, DownloadTable};
use crate::toolbar::ToolbarState;
use crate::{AppWindow, CategoryItem, DownloadRow, format};

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
}

/// The main window's state and the code that keeps the window in step with it.
pub struct App {
    ui: slint::Weak<AppWindow>,
    commands: Rc<dyn Commands>,
    table: RefCell<DownloadTable>,
    rows: Rc<VecModel<DownloadRow>>,
    tree: RefCell<CategoryTree>,
    category_rows: Rc<VecModel<CategoryItem>>,
}

impl App {
    /// Fills `ui` from `settings` and connects its callbacks, sending commands to `commands`.
    pub fn new(ui: &AppWindow, settings: &Settings, commands: Rc<dyn Commands>) -> Rc<Self> {
        let tree = CategoryTree::new(&settings.categories);
        let app = Rc::new(Self {
            ui: ui.as_weak(),
            commands,
            table: RefCell::default(),
            rows: Rc::new(VecModel::default()),
            category_rows: Rc::new(VecModel::from(tree.visible())),
            tree: RefCell::new(tree),
        });
        ui.set_downloads(app.rows.clone().into());
        ui.set_categories(app.category_rows.clone().into());
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
        ui.on_selection_changed(with(self, |app, _| app.refresh()));
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
        {
            let mut table = self.table.borrow_mut();
            for event in events {
                changed = true;
                match event {
                    EngineEvent::Added(download) | EngineEvent::Changed(download) => {
                        table.upsert(download)
                    }
                    EngineEvent::Removed(id) => table.remove(id),
                    EngineEvent::Progress(update) => table.set_progress(update),
                }
            }
        }
        if changed {
            self.refresh();
        }
    }

    /// The selected download, if it still exists.
    pub fn selected(&self) -> Option<DownloadId> {
        let id = download_id(self.ui.upgrade()?.get_selected_id())?;
        self.table.borrow().get(id).map(|d| d.id)
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

        let count = table.downloads().len();
        ui.set_status_message(match count {
            1 => "1 download".into(),
            n => format!("{n} downloads").into(),
        });
        let total_rate: u64 = table.downloads().iter().map(|d| table.rate_of(d.id)).sum();
        ui.set_status_summary(format::rate(total_rate).into());
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

/// A download id from the UI's `int`, which is -1 for none.
fn download_id(raw: i32) -> Option<DownloadId> {
    u64::try_from(raw).ok().map(DownloadId)
}

#[cfg(test)]
pub mod tests {
    use velo_core::{DownloadState, ProgressUpdate};

    use super::*;
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
    }

    fn app() -> (AppWindow, Rc<App>, Recorder) {
        i_slint_backend_testing::init_no_event_loop();
        let ui = AppWindow::new().unwrap();
        let recorder = Recorder::default();
        let app = App::new(&ui, &Settings::default(), Rc::new(recorder.clone()));
        (ui, app, recorder)
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
