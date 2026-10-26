//! The Scheduler dialog: the queues, their schedules and the order of their files.

use std::cell::{Cell, RefCell};
use std::collections::{BTreeMap, HashSet};
use std::rc::Rc;

use slint::{ComponentHandle, Model, ModelRc, SharedString, StandardListViewItem, VecModel};
use velo_core::{Queue, QueueId, Schedule, TimeOfDay};

use crate::app::Commands;
use crate::config::Config;
use crate::list::DownloadTable;
use crate::{QueuedFile, SchedulerWindow, theme};

/// The queues as the app knows them, shared with the main window.
pub type Queues = Rc<RefCell<BTreeMap<QueueId, Queue>>>;

/// The Scheduler dialog.
pub struct SchedulerDialog {
    commands: Rc<dyn Commands>,
    config: Rc<Config>,
    queues: Queues,
    window: RefCell<Option<SchedulerWindow>>,
    /// The queues in the order the list shows them.
    order: RefCell<Vec<QueueId>>,
    /// The queue whose settings fill the right side.
    shown: Cell<Option<QueueId>>,
    /// A new queue to select as soon as the engine reports it.
    select_new: Cell<Option<QueueId>>,
}

impl SchedulerDialog {
    /// Creates the dialog manager.
    pub fn new(commands: Rc<dyn Commands>, config: Rc<Config>, queues: Queues) -> Rc<Self> {
        Rc::new(Self {
            commands,
            config,
            queues,
            window: RefCell::default(),
            order: RefCell::default(),
            shown: Cell::new(None),
            select_new: Cell::new(None),
        })
    }

    /// Shows the Scheduler with `select` (or the first queue) selected.
    pub fn open(
        self: &Rc<Self>,
        select: QueueId,
        running: &HashSet<QueueId>,
        table: &DownloadTable,
    ) -> Result<(), slint::PlatformError> {
        if let Some(window) = self.window.borrow().as_ref() {
            return window.show();
        }
        let window = SchedulerWindow::new()?;
        theme::apply(&window, self.config.settings().appearance);
        self.connect(&window);
        window.show()?;
        *self.window.borrow_mut() = Some(window);
        self.shown.set(Some(select));
        self.update(running, table);
        self.load(select);
        Ok(())
    }

    /// Refreshes the open dialog: the list of queues, which run, and the selected queue's
    /// files. The settings being edited stay as they are.
    pub fn update(&self, running: &HashSet<QueueId>, table: &DownloadTable) {
        let Some(window) = self.window.borrow().as_ref().map(|w| w.clone_strong()) else {
            return;
        };
        let queues = self.queues.borrow();
        let order: Vec<QueueId> = queues.keys().copied().collect();
        let names: Vec<StandardListViewItem> = queues
            .values()
            .map(|q| {
                let mark = if running.contains(&q.id) {
                    "  (running)"
                } else {
                    ""
                };
                StandardListViewItem::from(format!("{}{mark}", q.name).as_str())
            })
            .collect();
        window.set_queue_names(ModelRc::new(VecModel::from(names)));
        *self.order.borrow_mut() = order.clone();

        // A queue just made, or a deleted one: change the selection.
        let mut shown = self.shown.get().filter(|id| queues.contains_key(id));
        if let Some(new) = self.select_new.get().filter(|id| queues.contains_key(id)) {
            self.select_new.set(None);
            shown = Some(new);
            drop(queues);
            self.load(new);
        } else if shown.is_none() {
            shown = order.first().copied();
            drop(queues);
            if let Some(first) = shown {
                self.load(first);
            }
        } else {
            drop(queues);
        }
        let Some(shown) = shown else { return };
        if let Some(index) = order.iter().position(|id| *id == shown) {
            window.set_current_queue(index as i32);
        }
        window.set_running(running.contains(&shown));

        let queues = self.queues.borrow();
        let files: Vec<QueuedFile> = queues
            .get(&shown)
            .map(|q| {
                q.downloads
                    .iter()
                    .filter_map(|id| table.get(*id))
                    .map(|d| QueuedFile {
                        name: d.file_name.as_str().into(),
                        status: status(d),
                    })
                    .collect()
            })
            .unwrap_or_default();
        window.set_files(ModelRc::new(VecModel::from(files)));
    }

    /// Fills the right side with queue `id`'s settings.
    fn load(&self, id: QueueId) {
        let Some(window) = self.window.borrow().as_ref().map(|w| w.clone_strong()) else {
            return;
        };
        let Some(queue) = self.queues.borrow().get(&id).cloned() else {
            return;
        };
        self.shown.set(Some(id));
        let schedule = &queue.schedule;
        window.set_is_main(id == QueueId::MAIN);
        window.set_name(queue.name.as_str().into());
        window.set_max_concurrent(queue.max_concurrent as i32);
        let start = schedule.start_at.unwrap_or_default();
        window.set_start_enabled(schedule.start_at.is_some());
        window.set_start_hour(i32::from(start.hour));
        window.set_start_minute(i32::from(start.minute));
        window.set_days(ModelRc::new(VecModel::from(schedule.days.to_vec())));
        let stop = schedule.stop_at.unwrap_or_default();
        window.set_stop_enabled(schedule.stop_at.is_some());
        window.set_stop_hour(i32::from(stop.hour));
        window.set_stop_minute(i32::from(stop.minute));
        window.set_exit_when_done(schedule.exit_when_done);
        window.set_shutdown_when_done(schedule.shutdown_when_done);
        window.set_current_file(-1);
    }

    /// The queue the dialog shows, as stored.
    fn shown_queue(&self) -> Option<Queue> {
        self.queues.borrow().get(&self.shown.get()?).cloned()
    }

    fn connect(self: &Rc<Self>, window: &SchedulerWindow) {
        let this = Rc::downgrade(self);
        window.on_queue_selected(move |index| {
            let Some(this) = this.upgrade() else { return };
            let id = this.order.borrow().get(index as usize).copied();
            if let Some(id) = id {
                this.load(id);
            }
        });
        let this = Rc::downgrade(self);
        window.on_new_queue(move || {
            let Some(this) = this.upgrade() else { return };
            let n = this.queues.borrow().len() + 1;
            let id = this.commands.add_queue(&format!("Queue {n}"));
            this.select_new.set(Some(id));
        });
        let this = Rc::downgrade(self);
        window.on_delete_queue(move || {
            if let Some(this) = this.upgrade()
                && let Some(id) = this.shown.get().filter(|id| *id != QueueId::MAIN)
            {
                this.commands.delete_queue(id);
            }
        });
        let weak = window.as_weak();
        window.on_day_toggled(move |day, on| {
            if let Some(window) = weak.upgrade() {
                window.get_days().set_row_data(day as usize, on);
            }
        });
        let (this, weak) = (Rc::downgrade(self), window.as_weak());
        window.on_move_file(move |delta| {
            let (Some(this), Some(window)) = (this.upgrade(), weak.upgrade()) else {
                return;
            };
            let Some(mut queue) = this.shown_queue() else {
                return;
            };
            let from = window.get_current_file();
            let to = from + delta;
            if from < 0 || to < 0 || to as usize >= queue.downloads.len() {
                return;
            }
            queue.downloads.swap(from as usize, to as usize);
            // Keep the new order here too, so the next button acts on what is shown before
            // the engine reports it back.
            this.queues.borrow_mut().insert(queue.id, queue.clone());
            this.commands.save_queue(queue);
            window.set_current_file(to);
        });
        let (this, weak) = (Rc::downgrade(self), window.as_weak());
        window.on_remove_file(move || {
            let (Some(this), Some(window)) = (this.upgrade(), weak.upgrade()) else {
                return;
            };
            let Some(queue) = this.shown_queue() else {
                return;
            };
            if let Some(id) = queue.downloads.get(window.get_current_file() as usize) {
                this.commands.dequeue(vec![*id]);
                window.set_current_file(-1);
            }
        });
        let this = Rc::downgrade(self);
        window.on_start_now(move || {
            if let Some(this) = this.upgrade()
                && let Some(id) = this.shown.get()
            {
                this.commands.start_queue(id);
            }
        });
        let this = Rc::downgrade(self);
        window.on_stop(move || {
            if let Some(this) = this.upgrade()
                && let Some(id) = this.shown.get()
            {
                this.commands.stop_queue(id);
            }
        });
        let (this, weak) = (Rc::downgrade(self), window.as_weak());
        window.on_apply(move || {
            let (Some(this), Some(window)) = (this.upgrade(), weak.upgrade()) else {
                return;
            };
            if let Some(queue) = this.shown_queue() {
                this.commands.save_queue(edited(&window, queue));
            }
        });
        let this = Rc::downgrade(self);
        window.on_close_clicked(move || {
            if let Some(window) = this.upgrade().and_then(|this| this.window.take()) {
                let _ = window.hide();
            }
        });
        let this = Rc::downgrade(self);
        window.window().on_close_requested(move || {
            if let Some(this) = this.upgrade() {
                this.window.take();
            }
            slint::CloseRequestResponse::HideWindow
        });
    }

    /// The open window.
    #[cfg(test)]
    pub fn window(&self) -> Option<SchedulerWindow> {
        self.window.borrow().as_ref().map(|w| w.clone_strong())
    }
}

/// `queue` with the settings from the window.
fn edited(window: &SchedulerWindow, mut queue: Queue) -> Queue {
    let time = |on: bool, hour: i32, minute: i32| {
        on.then(|| TimeOfDay::new(hour as u8, minute as u8))
            .flatten()
    };
    let days = window.get_days();
    queue.name = window.get_name().trim().to_string();
    queue.max_concurrent = window.get_max_concurrent().max(1) as u32;
    queue.schedule = Schedule {
        start_at: time(
            window.get_start_enabled(),
            window.get_start_hour(),
            window.get_start_minute(),
        ),
        days: std::array::from_fn(|i| days.row_data(i).unwrap_or(false)),
        stop_at: time(
            window.get_stop_enabled(),
            window.get_stop_hour(),
            window.get_stop_minute(),
        ),
        exit_when_done: window.get_exit_when_done(),
        shutdown_when_done: window.get_shutdown_when_done(),
    };
    queue
}

/// A short status for a file in a queue.
fn status(download: &velo_core::Download) -> SharedString {
    use velo_core::DownloadState::*;
    match &download.state {
        Queued => "Waiting",
        Probing | Downloading | Merging => "Downloading",
        Paused => "Paused",
        Complete => "Complete",
        Failed(_) => "Error",
    }
    .into()
}

#[cfg(test)]
mod tests {
    use velo_core::{DownloadId, DownloadState, Settings};

    use super::*;
    use crate::app::tests::Recorder;
    use crate::list::tests::sample;

    fn dialog() -> (Rc<SchedulerDialog>, Queues, Recorder, DownloadTable) {
        i_slint_backend_testing::init_no_event_loop();
        let recorder = Recorder::default();
        let mut main = Queue::main();
        main.downloads = vec![DownloadId(1), DownloadId(2)];
        let queues: Queues = Rc::new(RefCell::new(BTreeMap::from([(QueueId::MAIN, main)])));
        let config = Rc::new(Config::new(Settings::default(), None, "/d".into()));
        let dialog = SchedulerDialog::new(Rc::new(recorder.clone()), config, queues.clone());
        let mut table = DownloadTable::default();
        table.upsert(sample(1, "one.zip", DownloadState::Paused, 0, 1));
        table.upsert(sample(2, "two.zip", DownloadState::Queued, 0, 1));
        (dialog, queues, recorder, table)
    }

    #[test]
    fn the_schedule_is_edited_and_applied() {
        let (dialog, _, recorder, table) = dialog();
        dialog.open(QueueId::MAIN, &HashSet::new(), &table).unwrap();
        let window = dialog.window().unwrap();
        assert!(window.get_is_main() && !window.get_running());
        assert_eq!(window.get_files().row_count(), 2);
        assert_eq!(window.get_files().row_data(1).unwrap().status, "Waiting");

        window.set_start_enabled(true);
        window.set_start_hour(2);
        window.set_start_minute(30);
        window.invoke_day_toggled(5, true);
        window.set_max_concurrent(3);
        window.set_shutdown_when_done(true);
        window.invoke_apply();
        assert_eq!(
            *recorder.0.borrow(),
            ["save queue 0 order=1,2 max=3 start=02:30 days=0000010 shutdown=true"]
        );
    }

    #[test]
    fn files_move_and_leave_and_queues_come_and_go() {
        let (dialog, queues, recorder, table) = dialog();
        dialog.open(QueueId::MAIN, &HashSet::new(), &table).unwrap();
        let window = dialog.window().unwrap();
        window.set_current_file(0);
        window.invoke_move_file(1);
        assert_eq!(window.get_current_file(), 1);
        window.invoke_remove_file();
        window.invoke_start_now();
        window.invoke_new_queue();
        assert_eq!(
            *recorder.0.borrow(),
            [
                "save queue 0 order=2,1 max=1 start= days=0000000 shutdown=false",
                "dequeue 1",
                "start queue 0",
                "add queue Queue 2",
            ]
        );

        // The engine reports the new queue: the dialog selects it.
        queues
            .borrow_mut()
            .insert(QueueId(5), Queue::new(QueueId(5), "Queue 2"));
        dialog.update(&HashSet::from([QueueId::MAIN]), &table);
        assert_eq!(window.get_name(), "Queue 2");
        assert!(!window.get_is_main());
        assert_eq!(window.get_queue_names().row_count(), 2);
        assert_eq!(
            window.get_queue_names().row_data(0).unwrap().text,
            "Main download queue  (running)"
        );
        window.invoke_delete_queue();
        assert_eq!(recorder.0.borrow().last().unwrap(), "delete queue 5");
    }
}
