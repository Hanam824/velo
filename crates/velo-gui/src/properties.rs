//! A download's Properties dialog.

use std::cell::RefCell;
use std::collections::BTreeMap;
use std::rc::Rc;

use slint::ComponentHandle;
use velo_core::{Download, DownloadId, DownloadState};
use velo_engine::DownloadEdit;

use crate::app::Commands;
use crate::complete::Desktop;
use crate::list::file_kind;
use crate::{PropertiesWindow, format};

/// The open Properties dialogs.
pub struct PropertiesDialogs {
    commands: Rc<dyn Commands>,
    desktop: Rc<dyn Desktop>,
    windows: RefCell<BTreeMap<DownloadId, PropertiesWindow>>,
}

impl PropertiesDialogs {
    /// Creates the dialog manager.
    pub fn new(commands: Rc<dyn Commands>, desktop: Rc<dyn Desktop>) -> Rc<Self> {
        Rc::new(Self {
            commands,
            desktop,
            windows: RefCell::default(),
        })
    }

    /// Shows `download`'s Properties, opening them if needed.
    pub fn open(self: &Rc<Self>, download: &Download) -> Result<(), slint::PlatformError> {
        if let Some(window) = self.windows.borrow().get(&download.id) {
            return window.show();
        }
        let window = PropertiesWindow::new()?;
        let editable = !download.is_running() && download.state != DownloadState::Complete;
        window.set_file_name(download.file_name.as_str().into());
        window.set_location(download.save_dir.to_string_lossy().as_ref().into());
        window.set_url(download.url.as_str().into());
        window.set_description(download.description.as_str().into());
        window.set_status(status(&download.state).into());
        window.set_size(
            download
                .total_size
                .map(format::size)
                .unwrap_or_default()
                .into(),
        );
        window.set_kind(file_kind(&download.file_name));
        window.set_editable(editable);
        window.set_finished(download.is_finished());

        let id = download.id;
        let (this, original) = (Rc::downgrade(self), download.clone());
        window.on_apply(move || {
            let Some(this) = this.upgrade() else { return };
            let Some(window) = this.windows.borrow_mut().remove(&id) else {
                return;
            };
            let changed = |new: slint::SharedString, old: &str| (new != old).then(|| new.into());
            this.commands.edit(
                id,
                DownloadEdit {
                    file_name: changed(window.get_file_name(), &original.file_name)
                        .filter(|_| editable),
                    url: changed(window.get_url(), &original.url).filter(|_| editable),
                    description: changed(window.get_description(), &original.description),
                },
            );
            let _ = window.hide();
        });
        let this = Rc::downgrade(self);
        window.on_cancel(move || {
            if let Some(this) = this.upgrade()
                && let Some(window) = this.windows.borrow_mut().remove(&id)
            {
                let _ = window.hide();
            }
        });
        let (desktop, path) = (self.desktop.clone(), download.save_path());
        window.on_open_file(move || desktop.open(&path));
        let (desktop, path) = (self.desktop.clone(), download.save_path());
        window.on_open_folder(move || desktop.reveal(&path));
        let this = Rc::downgrade(self);
        window.window().on_close_requested(move || {
            if let Some(this) = this.upgrade() {
                this.windows.borrow_mut().remove(&id);
            }
            slint::CloseRequestResponse::HideWindow
        });
        window.show()?;
        self.windows.borrow_mut().insert(id, window);
        Ok(())
    }

    /// Download `id`'s Properties window, if open.
    #[cfg(test)]
    pub fn window(&self, id: DownloadId) -> Option<PropertiesWindow> {
        self.windows.borrow().get(&id).map(|w| w.clone_strong())
    }
}

/// A short description of a download's state.
fn status(state: &DownloadState) -> String {
    match state {
        DownloadState::Queued => "Waiting".into(),
        DownloadState::Probing | DownloadState::Downloading | DownloadState::Merging => {
            "Downloading".into()
        }
        DownloadState::Paused => "Paused".into(),
        DownloadState::Complete => "Complete".into(),
        DownloadState::Failed(message) => format!("Error: {message}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::tests::Recorder;
    use crate::complete::tests::FakeDesktop;
    use crate::list::tests::sample;

    fn dialogs() -> (Rc<PropertiesDialogs>, Recorder) {
        i_slint_backend_testing::init_no_event_loop();
        let recorder = Recorder::default();
        let dialogs =
            PropertiesDialogs::new(Rc::new(recorder.clone()), Rc::new(FakeDesktop::default()));
        (dialogs, recorder)
    }

    #[test]
    fn a_paused_download_can_be_renamed_and_moved_to_another_address() {
        let (dialogs, recorder) = dialogs();
        let download = sample(2, "a.iso", DownloadState::Paused, 1, 10);
        dialogs.open(&download).unwrap();
        let window = dialogs.window(download.id).unwrap();
        assert!(window.get_editable() && !window.get_finished());
        window.set_file_name("b.iso".into());
        window.set_url("http://mirror.test/a.iso".into());
        window.invoke_apply();
        assert!(dialogs.window(download.id).is_none());
        assert_eq!(
            *recorder.0.borrow(),
            ["edit 2 name=b.iso url=http://mirror.test/a.iso desc="]
        );
    }

    #[test]
    fn a_complete_download_only_changes_its_description() {
        let (dialogs, recorder) = dialogs();
        let download = sample(3, "a.iso", DownloadState::Complete, 10, 10);
        dialogs.open(&download).unwrap();
        let window = dialogs.window(download.id).unwrap();
        assert!(!window.get_editable() && window.get_finished());
        window.set_file_name("b.iso".into());
        window.set_description("keep".into());
        window.invoke_apply();
        assert_eq!(*recorder.0.borrow(), ["edit 3 name= url= desc=keep"]);
    }
}
