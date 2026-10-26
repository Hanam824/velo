//! Connects the main window to the app's state: the download table and the Categories tree.
//!
//! Everything here runs on the UI thread. Engine events reach it as plain data through
//! [`App::apply`].

use std::cell::RefCell;
use std::rc::Rc;

use slint::{ComponentHandle, Model, VecModel};
use velo_core::{DownloadId, EngineEvent, Settings};

use crate::categories::CategoryTree;
use crate::list::{Column, DownloadTable};
use crate::toolbar::ToolbarState;
use crate::{AppWindow, CategoryItem, DownloadRow, format};

/// The main window's state and the code that keeps the window in step with it.
pub struct App {
    ui: slint::Weak<AppWindow>,
    table: RefCell<DownloadTable>,
    rows: Rc<VecModel<DownloadRow>>,
    tree: RefCell<CategoryTree>,
    category_rows: Rc<VecModel<CategoryItem>>,
}

impl App {
    /// Fills `ui` from `settings` and connects its list, sort and Categories callbacks.
    pub fn new(ui: &AppWindow, settings: &Settings) -> Rc<Self> {
        let tree = CategoryTree::new(&settings.categories);
        let app = Rc::new(Self {
            ui: ui.as_weak(),
            table: RefCell::default(),
            rows: Rc::new(VecModel::default()),
            category_rows: Rc::new(VecModel::from(tree.visible())),
            tree: RefCell::new(tree),
        });
        ui.set_downloads(app.rows.clone().into());
        ui.set_categories(app.category_rows.clone().into());

        let this = Rc::downgrade(&app);
        ui.on_category_toggled(move |key| {
            if let Some(app) = this.upgrade() {
                app.tree.borrow_mut().toggle(&key);
                app.category_rows.set_vec(app.tree.borrow().visible());
            }
        });
        let this = Rc::downgrade(&app);
        ui.on_selection_changed(move |_| {
            if let Some(app) = this.upgrade() {
                app.refresh();
            }
        });
        let this = Rc::downgrade(&app);
        ui.on_sort(move |column| {
            if let (Some(app), Some(column)) = (this.upgrade(), Column::from_index(column)) {
                app.table.borrow_mut().sort_by(column);
                app.refresh();
            }
        });
        let this = Rc::downgrade(&app);
        ui.on_move_selection(move |delta| {
            let Some(app) = this.upgrade() else { return };
            let next = app.table.borrow().neighbour(app.selected(), delta);
            if let (Some(ui), Some(next)) = (app.ui.upgrade(), next) {
                ui.set_selected_id(next.0 as i32);
                app.refresh();
            }
        });
        app.refresh();
        app
    }

    /// Updates the table from an engine event and redraws.
    #[cfg_attr(
        not(test),
        expect(dead_code, reason = "fed by the engine bridge in D13")
    )]
    pub fn apply(&self, event: EngineEvent) {
        {
            let mut table = self.table.borrow_mut();
            match event {
                EngineEvent::Added(download) | EngineEvent::Changed(download) => {
                    table.upsert(download)
                }
                EngineEvent::Removed(id) => table.remove(id),
                EngineEvent::Progress(update) => table.set_progress(update),
            }
        }
        self.refresh();
    }

    /// The selected download, if it still exists.
    pub fn selected(&self) -> Option<DownloadId> {
        let id = DownloadId(u64::try_from(self.ui.upgrade()?.get_selected_id()).ok()?);
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

#[cfg(test)]
mod tests {
    use velo_core::{DownloadState, ProgressUpdate};

    use super::*;
    use crate::list::tests::sample;

    #[test]
    fn engine_events_reach_the_list_toolbar_and_status_bar() {
        i_slint_backend_testing::init_no_event_loop();
        let ui = AppWindow::new().unwrap();
        let app = App::new(&ui, &Settings::default());
        assert_eq!(ui.get_downloads().row_count(), 0);
        assert_eq!(ui.get_status_message(), "0 downloads");

        let running = sample(1, "a.iso", DownloadState::Downloading, 0, 10_000);
        app.apply(EngineEvent::Added(running.clone()));
        app.apply(EngineEvent::Added(sample(
            2,
            "b.zip",
            DownloadState::Complete,
            1,
            1,
        )));
        app.apply(EngineEvent::Progress(ProgressUpdate {
            id: running.id,
            downloaded: 5_000,
            total: Some(10_000),
            rate: 1024,
            segments: Vec::new(),
        }));
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
        app.apply(EngineEvent::Removed(DownloadId(2)));
        assert_eq!(ui.get_selected_id(), -1);
        assert_eq!(ui.get_downloads().row_count(), 1);

        // Sorting is reflected in the header.
        ui.invoke_sort(0);
        assert_eq!((ui.get_sort_column(), ui.get_sort_ascending()), (0, true));
    }
}
