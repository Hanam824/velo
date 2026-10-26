//! The Add Batch Download dialog.

use std::cell::RefCell;
use std::path::{Path, PathBuf};
use std::rc::Rc;

use slint::{ComponentHandle, ModelRc, SharedString, StandardListViewItem, VecModel};
use velo_core::batch::{BatchRange, expand};

use crate::add::Adder;
use crate::config::Config;
use crate::{BatchWindow, theme};

/// How many of a batch's addresses the dialog lists.
const PREVIEW: usize = 200;

/// Opens Add Batch Download and adds what the user chose.
pub struct BatchDialog {
    config: Rc<Config>,
    adder: Rc<Adder>,
    window: RefCell<Option<BatchWindow>>,
}

impl BatchDialog {
    /// Creates the dialog manager; `adder` adds the downloads.
    pub fn new(config: Rc<Config>, adder: Rc<Adder>) -> Rc<Self> {
        Rc::new(Self {
            config,
            adder,
            window: RefCell::default(),
        })
    }

    /// Shows the dialog, or brings it forward if it is open.
    pub fn open(self: &Rc<Self>) -> Result<(), slint::PlatformError> {
        if let Some(window) = self.window.borrow().as_ref() {
            return window.show();
        }
        let window = BatchWindow::new()?;
        theme::apply(&window, self.config.settings().appearance);
        window.set_default_folder(self.config.save_dir(None).to_string_lossy().as_ref().into());
        update(&window);

        let weak = window.as_weak();
        window.on_edited(move || {
            if let Some(window) = weak.upgrade() {
                update(&window);
            }
        });
        let weak = window.as_weak();
        window.on_browse(move || {
            let Some(window) = weak.upgrade() else { return };
            let current = window.get_folder();
            let start = if current.trim().is_empty() {
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
                this.accept();
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

    /// Adds the batch to the main queue and closes, unless it is invalid.
    fn accept(&self) {
        let Some(window) = self.window.borrow().as_ref().map(|w| w.clone_strong()) else {
            return;
        };
        let urls = match addresses(&window) {
            Ok(urls) => urls,
            Err(error) => {
                window.set_error(error.into());
                return;
            }
        };
        let folder = window.get_folder();
        let folder = Some(PathBuf::from(folder.trim())).filter(|p| !p.as_os_str().is_empty());
        self.adder
            .add_many(&urls, folder.as_deref(), window.get_start());
        self.window.take();
        let _ = window.hide();
    }

    /// The open window.
    #[cfg(test)]
    pub fn window(&self) -> Option<BatchWindow> {
        self.window.borrow().as_ref().map(|w| w.clone_strong())
    }
}

/// The addresses the dialog's fields make, or why they make none.
fn addresses(window: &BatchWindow) -> Result<Vec<String>, String> {
    let pattern = window.get_pattern();
    let pattern = pattern.trim();
    if !velo_core::links::is_http_url(&pattern.replace('*', "x")) {
        return Err("Enter an http or https address with a *.".into());
    }
    let range = range(
        window.get_kind(),
        &window.get_from(),
        &window.get_to(),
        window.get_digits(),
    )?;
    expand(pattern, range).map_err(|e| e.to_string())
}

/// The range the dialog's fields describe: `kind` 0 for numbers, 1 for letters.
fn range(kind: i32, from: &str, to: &str, width: i32) -> Result<BatchRange, String> {
    if kind == 1 {
        let letter = |text: &str| {
            let mut chars = text.trim().chars();
            match (chars.next(), chars.next()) {
                (Some(c), None) => Ok(c),
                _ => Err("From and To must be single letters.".to_string()),
            }
        };
        Ok(BatchRange::Letters {
            from: letter(from)?,
            to: letter(to)?,
        })
    } else {
        let number = |text: &str| {
            text.trim()
                .parse::<u64>()
                .map_err(|_| "From and To must be whole numbers.".to_string())
        };
        Ok(BatchRange::Numbers {
            from: number(from)?,
            to: number(to)?,
            width: width.clamp(1, 12) as usize,
        })
    }
}

/// Lists the addresses the fields make, or shows why there are none.
fn update(window: &BatchWindow) {
    let (urls, error) = if window.get_pattern().trim().is_empty() {
        (Vec::new(), String::new())
    } else {
        match addresses(window) {
            Ok(urls) => (urls, String::new()),
            Err(error) => (Vec::new(), error),
        }
    };
    let preview: Vec<StandardListViewItem> = urls
        .iter()
        .take(PREVIEW)
        .map(|url| StandardListViewItem::from(url.as_str()))
        .chain((urls.len() > PREVIEW).then(|| StandardListViewItem::from("…")))
        .collect();
    window.set_preview(ModelRc::new(VecModel::from(preview)));
    window.set_count(urls.len() as i32);
    window.set_error(SharedString::from(error));
}

#[cfg(test)]
mod tests {
    use slint::Model;
    use velo_core::Settings;

    use super::*;
    use crate::add::tests::FixedProber;
    use crate::app::tests::Recorder;

    fn dialog() -> (Rc<BatchDialog>, Recorder) {
        i_slint_backend_testing::init_no_event_loop();
        let config = Rc::new(Config::new(Settings::default(), None, "/downloads".into()));
        let recorder = Recorder::default();
        let adder = Adder::new(
            config.clone(),
            Rc::new(recorder.clone()),
            Rc::new(FixedProber(Err("unused".into()))),
        );
        (BatchDialog::new(config, adder), recorder)
    }

    #[test]
    fn the_preview_follows_the_fields_and_ok_queues_every_address() {
        let (dialog, recorder) = dialog();
        dialog.open().unwrap();
        let window = dialog.window().unwrap();
        assert_eq!(window.get_count(), 0);
        assert!(window.get_error().is_empty());

        window.set_pattern("https://a.test/part*.zip".into());
        window.set_from("8".into());
        window.set_to("10".into());
        window.set_digits(2);
        window.invoke_edited();
        assert_eq!(window.get_count(), 3);
        assert_eq!(
            window.get_preview().row_data(0).unwrap().text,
            "https://a.test/part08.zip"
        );

        window.set_to("x".into());
        window.invoke_edited();
        assert_eq!(window.get_count(), 0);
        assert!(!window.get_error().is_empty());

        window.set_kind(1);
        window.set_from("a".into());
        window.set_to("b".into());
        window.set_start(true);
        window.invoke_edited();
        assert_eq!(window.get_count(), 2);
        window.invoke_accepted();
        assert!(dialog.window().is_none());

        let commands = recorder.0.borrow();
        assert_eq!(commands.len(), 4, "{commands:?}");
        assert!(
            commands[0].starts_with("add https://a.test/parta.zip")
                && commands[0].contains("/downloads")
                && commands[0].contains("Compressed")
                && commands[0].contains("start=false"),
            "{}",
            commands[0]
        );
        assert!(commands[2].starts_with("enqueue"), "{}", commands[2]);
        assert!(commands[3].starts_with("start queue"), "{}", commands[3]);
    }

    #[test]
    fn a_pattern_without_a_star_is_refused() {
        let (dialog, recorder) = dialog();
        dialog.open().unwrap();
        let window = dialog.window().unwrap();
        window.set_pattern("https://a.test/file.zip".into());
        window.invoke_edited();
        assert!(window.get_error().contains('*'), "{}", window.get_error());
        window.invoke_accepted();
        assert!(dialog.window().is_some());
        assert!(recorder.0.borrow().is_empty());
    }
}
