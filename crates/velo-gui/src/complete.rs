//! What happens when a download finishes: the Download complete dialog, and exiting or turning
//! the computer off if the download asked for it.

use std::cell::RefCell;
use std::collections::BTreeMap;
use std::path::Path;
use std::rc::Rc;
use std::time::Duration;

use slint::ComponentHandle;
use velo_core::{Download, DownloadId};

use crate::config::Config;
use crate::list::file_kind;
use crate::progress::DownloadOptions;
use crate::{CompleteWindow, ShutdownWindow, format, theme};

/// Seconds between a download finishing and the computer turning off, so the user can cancel.
const SHUTDOWN_DELAY: i32 = 30;

/// Actions on the user's desktop.
pub trait Desktop {
    /// Opens `path` with its default app.
    fn open(&self, path: &Path);
    /// Shows `path` in the file manager.
    fn reveal(&self, path: &Path);
    /// Turns the computer off.
    fn shutdown(&self);
    /// Quits Velo, pausing and saving what is still running.
    fn quit(&self);
}

/// The real desktop, through `velo-platform` and Slint.
pub struct SystemDesktop;

impl Desktop for SystemDesktop {
    fn open(&self, path: &Path) {
        if let Err(error) = velo_platform::open(path) {
            eprintln!("velo: {error}");
        }
    }

    fn reveal(&self, path: &Path) {
        if let Err(error) = velo_platform::reveal(path) {
            eprintln!("velo: {error}");
        }
    }

    fn shutdown(&self) {
        if let Err(error) = velo_platform::shutdown() {
            eprintln!("velo: {error}");
        }
    }

    fn quit(&self) {
        let _ = slint::quit_event_loop();
    }
}

/// Handles finished downloads.
pub struct Completion {
    config: Rc<Config>,
    desktop: Rc<dyn Desktop>,
    windows: RefCell<BTreeMap<DownloadId, CompleteWindow>>,
    /// The shutdown countdown, while it runs.
    countdown: RefCell<Option<(ShutdownWindow, slint::Timer)>>,
}

impl Completion {
    /// Creates the handler.
    pub fn new(config: Rc<Config>, desktop: Rc<dyn Desktop>) -> Rc<Self> {
        Rc::new(Self {
            config,
            desktop,
            windows: RefCell::default(),
            countdown: RefCell::default(),
        })
    }

    /// Reacts to `download` having just finished, as `options` say.
    pub fn finished(self: &Rc<Self>, download: &Download, options: &DownloadOptions) {
        if options.show_complete
            && self.config.settings().show_complete_dialog
            && let Err(error) = self.show(download)
        {
            eprintln!("velo: couldn't open Download complete: {error}");
        }
        if options.shutdown {
            if let Err(error) = self.start_countdown() {
                eprintln!("velo: couldn't start the shutdown countdown: {error}");
            }
        } else if options.exit {
            self.desktop.quit();
        }
    }

    /// Shows the Download complete dialog for `download`.
    fn show(self: &Rc<Self>, download: &Download) -> Result<(), slint::PlatformError> {
        let window = CompleteWindow::new()?;
        theme::apply(&window, self.config.settings().appearance);
        let path = download.save_path();
        window.set_address(download.url.as_str().into());
        window.set_size(
            download
                .total_size
                .map(format::size)
                .unwrap_or_default()
                .into(),
        );
        window.set_saved_as(path.to_string_lossy().as_ref().into());
        window.set_kind(file_kind(&download.file_name));

        let (desktop, file) = (self.desktop.clone(), path.clone());
        window.on_open_file(move || desktop.open(&file));
        let (desktop, file) = (self.desktop.clone(), path);
        window.on_open_folder(move || desktop.reveal(&file));
        let (this, id) = (Rc::downgrade(self), download.id);
        window.on_close_clicked(move || {
            if let Some(this) = this.upgrade() {
                this.close(id);
            }
        });
        let (this, id) = (Rc::downgrade(self), download.id);
        window.window().on_close_requested(move || {
            if let Some(this) = this.upgrade() {
                this.close(id);
            }
            slint::CloseRequestResponse::HideWindow
        });
        window.show()?;
        self.windows.borrow_mut().insert(download.id, window);
        Ok(())
    }

    /// Closes download `id`'s dialog, remembering "Don't show this dialog again".
    fn close(&self, id: DownloadId) {
        let Some(window) = self.windows.borrow_mut().remove(&id) else {
            return;
        };
        if window.get_dont_show_again() {
            self.config.update(|s| s.show_complete_dialog = false);
        }
        let _ = window.hide();
    }

    /// Shows the countdown and turns the computer off when it reaches zero.
    fn start_countdown(self: &Rc<Self>) -> Result<(), slint::PlatformError> {
        if self.countdown.borrow().is_some() {
            return Ok(());
        }
        let window = ShutdownWindow::new()?;
        theme::apply(&window, self.config.settings().appearance);
        window.set_seconds(SHUTDOWN_DELAY);
        let this = Rc::downgrade(self);
        window.on_shutdown_now(move || {
            if let Some(this) = this.upgrade() {
                this.shutdown_now();
            }
        });
        let this = Rc::downgrade(self);
        window.on_cancel(move || {
            if let Some(this) = this.upgrade()
                && let Some((window, _)) = this.countdown.take()
            {
                let _ = window.hide();
            }
        });
        let timer = slint::Timer::default();
        let this = Rc::downgrade(self);
        timer.start(
            slint::TimerMode::Repeated,
            Duration::from_secs(1),
            move || {
                let Some(this) = this.upgrade() else { return };
                let seconds = this
                    .countdown
                    .borrow()
                    .as_ref()
                    .map(|(window, _)| window.get_seconds() - 1);
                match seconds {
                    Some(left) if left <= 0 => this.shutdown_now(),
                    Some(left) => {
                        if let Some((window, _)) = this.countdown.borrow().as_ref() {
                            window.set_seconds(left);
                        }
                    }
                    None => {}
                }
            },
        );
        window.show()?;
        *self.countdown.borrow_mut() = Some((window, timer));
        Ok(())
    }

    /// Turns the computer off and quits.
    fn shutdown_now(&self) {
        if let Some((window, _)) = self.countdown.take() {
            let _ = window.hide();
        }
        self.desktop.shutdown();
        self.desktop.quit();
    }

    /// Download `id`'s Download complete dialog, if open.
    #[cfg(test)]
    pub fn window(&self, id: DownloadId) -> Option<CompleteWindow> {
        self.windows.borrow().get(&id).map(|w| w.clone_strong())
    }

    /// The shutdown countdown window, if it runs.
    #[cfg(test)]
    pub fn countdown(&self) -> Option<ShutdownWindow> {
        self.countdown
            .borrow()
            .as_ref()
            .map(|(w, _)| w.clone_strong())
    }
}

#[cfg(test)]
pub mod tests {
    use std::path::PathBuf;

    use velo_core::{DownloadState, Settings};

    use super::*;
    use crate::list::tests::sample;

    /// Records the desktop actions it is asked for.
    #[derive(Default, Clone)]
    pub struct FakeDesktop(pub Rc<RefCell<Vec<String>>>);

    impl Desktop for FakeDesktop {
        fn open(&self, path: &Path) {
            self.0.borrow_mut().push(format!("open {}", path.display()));
        }
        fn reveal(&self, path: &Path) {
            self.0
                .borrow_mut()
                .push(format!("reveal {}", path.display()));
        }
        fn shutdown(&self) {
            self.0.borrow_mut().push("shutdown".into());
        }
        fn quit(&self) {
            self.0.borrow_mut().push("quit".into());
        }
    }

    fn completion() -> (Rc<Completion>, Rc<Config>, FakeDesktop) {
        i_slint_backend_testing::init_no_event_loop();
        let config = Rc::new(Config::new(Settings::default(), None, PathBuf::from("/d")));
        let desktop = FakeDesktop::default();
        let completion = Completion::new(config.clone(), Rc::new(desktop.clone()));
        (completion, config, desktop)
    }

    #[test]
    fn the_complete_dialog_opens_the_file_and_can_be_turned_off() {
        let (completion, config, desktop) = completion();
        let download = sample(5, "a.zip", DownloadState::Complete, 10, 10);
        completion.finished(&download, &DownloadOptions::default());
        let window = completion.window(download.id).unwrap();
        assert_eq!(window.get_size(), "10 B");
        window.invoke_open_file();
        window.invoke_open_folder();
        let path = download.save_path();
        assert_eq!(
            *desktop.0.borrow(),
            [
                format!("open {}", path.display()),
                format!("reveal {}", path.display())
            ]
        );

        window.set_dont_show_again(true);
        window.invoke_close_clicked();
        assert!(completion.window(download.id).is_none());
        assert!(!config.settings().show_complete_dialog);
        completion.finished(&download, &DownloadOptions::default());
        assert!(completion.window(download.id).is_none());
    }

    #[test]
    fn exit_and_shutdown_when_done() {
        let (completion, _, desktop) = completion();
        let download = sample(5, "a.zip", DownloadState::Complete, 10, 10);
        let quiet = DownloadOptions {
            show_complete: false,
            ..DownloadOptions::default()
        };
        completion.finished(
            &download,
            &DownloadOptions {
                exit: true,
                ..quiet.clone()
            },
        );
        assert_eq!(*desktop.0.borrow(), ["quit"]);

        completion.finished(
            &download,
            &DownloadOptions {
                shutdown: true,
                ..quiet.clone()
            },
        );
        let countdown = completion.countdown().unwrap();
        assert_eq!(countdown.get_seconds(), SHUTDOWN_DELAY);
        countdown.invoke_cancel();
        assert!(completion.countdown().is_none());
        assert_eq!(*desktop.0.borrow(), ["quit"]);

        completion.finished(
            &download,
            &DownloadOptions {
                shutdown: true,
                ..quiet
            },
        );
        completion.countdown().unwrap().invoke_shutdown_now();
        assert_eq!(*desktop.0.borrow(), ["quit", "shutdown", "quit"]);
    }
}
