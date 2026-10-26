//! Adding, editing and deleting categories.

use std::cell::RefCell;
use std::path::{Path, PathBuf};
use std::rc::Rc;

use slint::{ComponentHandle, SharedString};
use velo_core::{Category, Download};
use velo_engine::DownloadEdit;

use crate::app::Commands;
use crate::config::Config;
use crate::{CategoryWindow, theme};

/// The category dialog, and what changes when categories change.
pub struct CategoryDialog {
    config: Rc<Config>,
    commands: Rc<dyn Commands>,
    default_downloads: PathBuf,
    window: RefCell<Option<CategoryWindow>>,
    /// Runs after the categories change, to rebuild what shows them.
    on_changed: RefCell<Option<Box<dyn Fn()>>>,
}

impl CategoryDialog {
    /// Creates the dialog manager.
    pub fn new(
        config: Rc<Config>,
        commands: Rc<dyn Commands>,
        default_downloads: PathBuf,
    ) -> Rc<Self> {
        Rc::new(Self {
            config,
            commands,
            default_downloads,
            window: RefCell::default(),
            on_changed: RefCell::default(),
        })
    }

    /// Calls `f` each time the categories change.
    pub fn on_changed(&self, f: impl Fn() + 'static) {
        *self.on_changed.borrow_mut() = Some(Box::new(f));
    }

    /// Opens the dialog to add a category (`editing` is `None`) or edit the one named
    /// `editing`. `downloads` are the downloads to re-file if it is renamed.
    pub fn open(
        self: &Rc<Self>,
        editing: Option<String>,
        downloads: Rc<dyn Fn() -> Vec<Download>>,
    ) -> Result<(), slint::PlatformError> {
        if let Some(window) = self.window.take() {
            let _ = window.hide();
        }
        let window = CategoryWindow::new()?;
        theme::apply(&window, self.config.settings().appearance);
        window.set_default_folder(self.default_downloads.to_string_lossy().as_ref().into());
        let existing = editing.as_ref().and_then(|name| {
            self.config
                .settings()
                .categories
                .into_iter()
                .find(|c| &c.name == name)
        });
        if let Some(category) = &existing {
            window.set_name(category.name.as_str().into());
            window.set_extensions(category.extensions.join(" ").into());
            if let Some(dir) = &category.save_dir {
                window.set_folder(dir.to_string_lossy().as_ref().into());
            }
        }

        let weak = window.as_weak();
        window.on_browse(move || {
            let Some(window) = weak.upgrade() else { return };
            let current = window.get_folder();
            let start = if current.is_empty() {
                window.get_default_folder()
            } else {
                current
            };
            if let Some(dir) = velo_platform::pick_folder(Path::new(start.as_str())) {
                window.set_folder(dir.to_string_lossy().as_ref().into());
            }
        });
        let this = Rc::downgrade(self);
        window.on_accepted(move || {
            if let Some(this) = this.upgrade() {
                this.save(editing.as_deref(), &downloads);
            }
        });
        let this = Rc::downgrade(self);
        window.on_cancelled(move || {
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
        window.show()?;
        *self.window.borrow_mut() = Some(window);
        Ok(())
    }

    /// Deletes category `name`; its downloads keep their files but lose the category.
    pub fn delete(&self, name: &str, downloads: &[Download]) {
        self.config
            .update(|s| s.categories.retain(|c| c.name != name));
        self.refile(downloads, name, None);
        self.changed();
    }

    /// Saves the dialog's category if valid; otherwise shows why not.
    fn save(&self, editing: Option<&str>, downloads: &Rc<dyn Fn() -> Vec<Download>>) {
        let Some(window) = self.window.borrow().as_ref().map(|w| w.clone_strong()) else {
            return;
        };
        let name = window.get_name().trim().to_string();
        let taken = self
            .config
            .settings()
            .categories
            .iter()
            .any(|c| c.name.eq_ignore_ascii_case(&name) && Some(c.name.as_str()) != editing);
        let error = if name.is_empty() {
            "Give the category a name."
        } else if name.contains('/') {
            "A category name can't contain “/”."
        } else if taken {
            "There is already a category with that name."
        } else {
            ""
        };
        if !error.is_empty() {
            window.set_error(SharedString::from(error));
            return;
        }
        let folder = window.get_folder().trim().to_string();
        let category = Category {
            name: name.clone(),
            extensions: parse_extensions(&window.get_extensions()),
            save_dir: (!folder.is_empty()).then(|| PathBuf::from(folder)),
        };
        self.config.update(|s| match editing {
            Some(old) => {
                if let Some(slot) = s.categories.iter_mut().find(|c| c.name == old) {
                    *slot = category;
                }
            }
            None => s.categories.push(category),
        });
        if let Some(old) = editing.filter(|old| *old != name) {
            self.refile(&downloads(), old, Some(name));
        }
        self.window.take();
        let _ = window.hide();
        self.changed();
    }

    /// Moves `downloads` filed under `from` to `to`.
    fn refile(&self, downloads: &[Download], from: &str, to: Option<String>) {
        for download in downloads
            .iter()
            .filter(|d| d.category.as_deref() == Some(from))
        {
            self.commands.edit(
                download.id,
                DownloadEdit {
                    category: Some(to.clone()),
                    ..DownloadEdit::default()
                },
            );
        }
    }

    fn changed(&self) {
        if let Some(on_changed) = self.on_changed.borrow().as_ref() {
            on_changed();
        }
    }

    /// The open dialog.
    #[cfg(test)]
    pub fn window(&self) -> Option<CategoryWindow> {
        self.window.borrow().as_ref().map(|w| w.clone_strong())
    }
}

/// Extensions from text such as `".MP4, mkv avi mkv"`: lowercase, without dots, each once.
pub fn parse_extensions(text: &str) -> Vec<String> {
    let mut extensions: Vec<String> = Vec::new();
    for ext in text.split(|c: char| c.is_whitespace() || c == ',' || c == ';') {
        let ext = ext.trim().trim_start_matches('.').to_lowercase();
        if !ext.is_empty() && !extensions.contains(&ext) {
            extensions.push(ext);
        }
    }
    extensions
}

#[cfg(test)]
mod tests {
    use std::cell::Cell;

    use velo_core::{DownloadState, Settings};

    use super::*;
    use crate::app::tests::Recorder;
    use crate::list::tests::sample;

    fn dialog() -> (Rc<CategoryDialog>, Rc<Config>, Recorder, Rc<Cell<u32>>) {
        i_slint_backend_testing::init_no_event_loop();
        let config = Rc::new(Config::new(Settings::default(), None, "/d".into()));
        let recorder = Recorder::default();
        let dialog = CategoryDialog::new(config.clone(), Rc::new(recorder.clone()), "/d".into());
        let changes = Rc::new(Cell::new(0));
        let counter = changes.clone();
        dialog.on_changed(move || counter.set(counter.get() + 1));
        (dialog, config, recorder, changes)
    }

    fn filed(id: u64, category: &str) -> Download {
        let mut download = sample(id, "x.iso", DownloadState::Paused, 0, 1);
        download.category = Some(category.into());
        download
    }

    #[test]
    fn extensions_are_normalized() {
        assert_eq!(
            parse_extensions(" .MP4, mkv;avi  mkv "),
            ["mp4", "mkv", "avi"]
        );
        assert!(parse_extensions("  ").is_empty());
    }

    #[test]
    fn a_new_category_is_added_and_a_duplicate_refused() {
        let (dialog, config, _, changes) = dialog();
        dialog.open(None, Rc::new(Vec::new)).unwrap();
        let window = dialog.window().unwrap();
        window.set_name("video".into());
        window.invoke_accepted();
        assert!(
            window.get_error().contains("already"),
            "{}",
            window.get_error()
        );

        window.set_name("Images".into());
        window.set_extensions("png .JPG".into());
        window.set_folder("/pics".into());
        window.invoke_accepted();
        assert!(dialog.window().is_none());
        let added = config.settings().categories.pop().unwrap();
        assert_eq!(added.name, "Images");
        assert_eq!(added.extensions, ["png", "jpg"]);
        assert_eq!(added.save_dir.as_deref(), Some(Path::new("/pics")));
        assert_eq!(changes.get(), 1);
    }

    #[test]
    fn renaming_and_deleting_refile_the_downloads() {
        let (dialog, config, recorder, changes) = dialog();
        let downloads = vec![filed(1, "Music"), filed(2, "Video")];
        let list = downloads.clone();
        dialog
            .open(Some("Music".into()), Rc::new(move || list.clone()))
            .unwrap();
        let window = dialog.window().unwrap();
        assert_eq!(window.get_name(), "Music");
        assert!(window.get_extensions().contains("mp3"));
        window.set_name("Audio".into());
        window.invoke_accepted();
        assert!(
            config
                .settings()
                .categories
                .iter()
                .any(|c| c.name == "Audio")
        );
        assert!(
            !config
                .settings()
                .categories
                .iter()
                .any(|c| c.name == "Music")
        );

        dialog.delete("Video", &downloads);
        assert!(
            !config
                .settings()
                .categories
                .iter()
                .any(|c| c.name == "Video")
        );
        assert_eq!(changes.get(), 2);
        assert_eq!(recorder.0.borrow().len(), 2, "{:?}", recorder.0.borrow());
    }
}
