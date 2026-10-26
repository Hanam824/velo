//! Adding a download: the Add URL dialog, then Download File Info while the server is asked
//! about the file.

use std::cell::{Cell, RefCell};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::rc::Rc;

use slint::{ComponentHandle, ModelRc, SharedString, StandardListViewItem, VecModel};
use velo_core::{DownloadId, QueueId, Settings};
use velo_engine::{Credentials, NewDownload, Probe, RequestOptions, Url, unique_file_name};

use crate::app::Commands;
use crate::config::Config;
use crate::list::file_kind;
use crate::{AddUrlDialog, DownloadInfoDialog, format, theme};

/// The outcome of asking a server about a URL: what it said, or an error message.
pub type ProbeResult = Result<Probe, String>;

/// What runs on the UI thread with a probe's result.
pub type ProbeDone = Box<dyn FnOnce(ProbeResult)>;

/// Asks servers about URLs off the UI thread.
pub trait Prober {
    /// Probes `url` with the user agent and proxy from `settings`, sending `credentials` if
    /// any, and calls `done` with the result on the UI thread.
    fn probe(
        &self,
        url: String,
        settings: Settings,
        credentials: Option<Credentials>,
        done: ProbeDone,
    );
}

/// Whether `text` is an http or https address with a host.
pub fn is_download_url(text: &str) -> bool {
    Url::parse(text.trim())
        .is_ok_and(|url| matches!(url.scheme(), "http" | "https") && url.host().is_some())
}

/// Where a download asked for from outside the app (such as by `velo-cli`) should go; what is
/// left `None` the app chooses as usual.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Preset {
    /// The folder to save into.
    pub save_dir: Option<PathBuf>,
    /// The name to save as.
    pub file_name: Option<String>,
}

/// What runs with a download started from Download File Info.
type OnStarted = Box<dyn Fn(DownloadId)>;

/// Opens the add dialogs and turns what the user chose into engine downloads.
pub struct Adder {
    config: Rc<Config>,
    commands: Rc<dyn Commands>,
    prober: Rc<dyn Prober>,
    /// The open Add URL dialog, kept here so it stays alive while shown.
    add_url: RefCell<Option<AddUrlDialog>>,
    /// The open Download File Info dialogs, by a number given when opened.
    infos: RefCell<BTreeMap<u64, DownloadInfoDialog>>,
    /// The number for the next Download File Info dialog.
    next_info: Cell<u64>,
    /// Runs with each download started from Download File Info, to show its progress.
    on_started: RefCell<Option<OnStarted>>,
}

/// What a Download File Info dialog knows besides its fields.
struct Pending {
    url: String,
    credentials: Option<Credentials>,
    /// The server's file name, before it is made unique in the chosen folder.
    server_name: String,
    /// Whether the user changed Save As; then a new category doesn't move the file.
    path_edited: bool,
    /// The folder and name asked for from outside the app.
    preset: Preset,
}

impl Adder {
    /// Creates an adder that sends new downloads to `commands`.
    pub fn new(config: Rc<Config>, commands: Rc<dyn Commands>, prober: Rc<dyn Prober>) -> Rc<Self> {
        Rc::new(Self {
            config,
            commands,
            prober,
            add_url: RefCell::default(),
            infos: RefCell::default(),
            next_info: Cell::new(0),
            on_started: RefCell::default(),
        })
    }

    /// Calls `f` with each download started at once from Download File Info.
    pub fn on_started(&self, f: impl Fn(DownloadId) + 'static) {
        *self.on_started.borrow_mut() = Some(Box::new(f));
    }

    /// Shows the Add URL dialog, or brings it forward if it is already open.
    pub fn open(self: &Rc<Self>) -> Result<(), slint::PlatformError> {
        if let Some(dialog) = self.add_url.borrow().as_ref() {
            return dialog.show();
        }
        let dialog = AddUrlDialog::new()?;
        theme::apply(&dialog, self.config.settings().appearance);
        let history: Vec<StandardListViewItem> = self
            .config
            .settings()
            .url_history
            .iter()
            .map(|url| StandardListViewItem::from(url.as_str()))
            .collect();
        dialog.set_history(ModelRc::new(VecModel::from(history)));

        let weak = dialog.as_weak();
        dialog.on_url_edited(move |text| {
            if let Some(dialog) = weak.upgrade() {
                dialog.set_url_valid(is_download_url(&text));
            }
        });
        let this = Rc::downgrade(self);
        dialog.on_accepted(move || {
            let Some(this) = this.upgrade() else { return };
            let Some(dialog) = this.add_url.take() else {
                return;
            };
            let url = dialog.get_url().trim().to_string();
            let credentials = dialog.get_use_authorization().then(|| Credentials {
                user: dialog.get_user().into(),
                password: dialog.get_password().into(),
            });
            let _ = dialog.hide();
            if let Err(error) = this.open_info(url, credentials, Preset::default()) {
                eprintln!("velo: couldn't open Download File Info: {error}");
            }
        });
        let this = Rc::downgrade(self);
        dialog.on_cancelled(move || {
            if let Some(dialog) = this.upgrade().and_then(|this| this.add_url.take()) {
                let _ = dialog.hide();
            }
        });
        let this = Rc::downgrade(self);
        dialog.window().on_close_requested(move || {
            if let Some(this) = this.upgrade() {
                this.add_url.take();
            }
            slint::CloseRequestResponse::HideWindow
        });
        dialog.show()?;
        *self.add_url.borrow_mut() = Some(dialog);
        Ok(())
    }

    /// Shows Download File Info for `url` and asks the server about it; Save As follows
    /// `preset` where it names a folder or file name.
    pub fn open_info(
        self: &Rc<Self>,
        url: String,
        credentials: Option<Credentials>,
        preset: Preset,
    ) -> Result<(), slint::PlatformError> {
        let dialog = DownloadInfoDialog::new()?;
        let number = self.next_info.get();
        self.next_info.set(number + 1);
        let settings = self.config.settings();
        theme::apply(&dialog, settings.appearance);
        let names: Vec<SharedString> = std::iter::once("None".into())
            .chain(settings.categories.iter().map(|c| c.name.as_str().into()))
            .collect();
        dialog.set_categories(ModelRc::new(VecModel::from(names)));
        dialog.set_url(url.as_str().into());
        let pending = Rc::new(RefCell::new(Pending {
            url: url.clone(),
            credentials: credentials.clone(),
            server_name: String::new(),
            path_edited: false,
            preset,
        }));

        let (weak, config, state) = (dialog.as_weak(), self.config.clone(), pending.clone());
        dialog.on_category_changed(move |_| {
            if let Some(dialog) = weak.upgrade()
                && !state.borrow().path_edited
            {
                let state = state.borrow();
                let dir = state.dir(&config, category(&dialog).as_deref());
                set_save_path(&dialog, &dir, state.name());
            }
        });
        let state = pending.clone();
        dialog.on_save_path_edited(move || state.borrow_mut().path_edited = true);
        let (weak, state) = (dialog.as_weak(), pending.clone());
        dialog.on_browse(move || {
            let Some(dialog) = weak.upgrade() else { return };
            let (dir, name) = split_path(&dialog.get_save_path());
            if let Some(path) = velo_platform::pick_save_path(&dir, &name) {
                dialog.set_save_path(path.to_string_lossy().as_ref().into());
                state.borrow_mut().path_edited = true;
            }
        });
        for start in [true, false] {
            let (this, weak, state) = (Rc::downgrade(self), dialog.as_weak(), pending.clone());
            let add = move || {
                if let (Some(this), Some(dialog)) = (this.upgrade(), weak.upgrade()) {
                    this.add(&dialog, &state.borrow(), start);
                    this.close_info(number);
                }
            };
            if start {
                dialog.on_start_download(add);
            } else {
                dialog.on_download_later(add);
            }
        }
        let this = Rc::downgrade(self);
        dialog.on_cancelled(move || {
            if let Some(this) = this.upgrade() {
                this.close_info(number);
            }
        });
        let this = Rc::downgrade(self);
        dialog.window().on_close_requested(move || {
            if let Some(this) = this.upgrade() {
                this.infos.borrow_mut().remove(&number);
            }
            slint::CloseRequestResponse::HideWindow
        });

        dialog.show()?;
        self.infos
            .borrow_mut()
            .insert(number, dialog.clone_strong());
        let (weak, config) = (dialog.as_weak(), self.config.clone());
        self.prober.probe(
            url,
            settings,
            credentials,
            Box::new(move |result| {
                if let Some(dialog) = weak.upgrade() {
                    probed(&dialog, &config, &mut pending.borrow_mut(), result);
                }
            }),
        );
        Ok(())
    }

    /// Adds a download of `url` without asking, once the server has said what the file is
    /// called: into `preset`'s folder, else its category's. It starts at once with `start`, and
    /// otherwise waits in the main queue.
    pub fn add_silently(self: &Rc<Self>, url: String, preset: Preset, start: bool) {
        let this = Rc::downgrade(self);
        self.prober.probe(
            url.clone(),
            self.config.settings(),
            None,
            Box::new(move |result| {
                let Some(this) = this.upgrade() else { return };
                // If the server didn't answer, the engine tries again and shows the error.
                let server_name = result.map(|probe| probe.file_name).unwrap_or_default();
                let name = preset.file_name.clone().unwrap_or(server_name);
                let category = this.config.category_for(&name);
                let save_dir = preset
                    .save_dir
                    .clone()
                    .unwrap_or_else(|| this.config.save_dir(category.as_deref()));
                let id = this.commands.add(NewDownload {
                    url,
                    file_name: preset.file_name.unwrap_or_default(),
                    save_dir,
                    category,
                    description: String::new(),
                    start,
                    request: RequestOptions::default(),
                });
                if !start {
                    this.commands.enqueue(QueueId::MAIN, vec![id]);
                }
            }),
        );
    }

    /// Adds the download the dialog describes and saves the choices worth remembering.
    fn add(&self, dialog: &DownloadInfoDialog, pending: &Pending, start: bool) {
        let (save_dir, file_name) = split_path(&dialog.get_save_path());
        let category = category(dialog);
        if dialog.get_remember_path()
            && let Some(category) = &category
        {
            self.config.remember_dir(category, &save_dir);
        }
        self.config.update(|s| s.remember_url(&pending.url));
        let id = self.commands.add(NewDownload {
            url: pending.url.clone(),
            file_name,
            save_dir,
            category,
            description: dialog.get_description().into(),
            start,
            request: RequestOptions {
                credentials: pending.credentials.clone(),
                ..RequestOptions::default()
            },
        });
        if start && let Some(on_started) = self.on_started.borrow().as_ref() {
            on_started(id);
        }
    }

    /// Hides Download File Info dialog `number` and forgets it.
    fn close_info(&self, number: u64) {
        if let Some(dialog) = self.infos.borrow_mut().remove(&number) {
            let _ = dialog.hide();
        }
    }
}

impl Pending {
    /// The folder to save into for `category`.
    fn dir(&self, config: &Config, category: Option<&str>) -> PathBuf {
        self.preset
            .save_dir
            .clone()
            .unwrap_or_else(|| config.save_dir(category))
    }

    /// The name to save as, before it is made unique.
    fn name(&self) -> &str {
        self.preset
            .file_name
            .as_deref()
            .unwrap_or(&self.server_name)
    }
}

/// Fills the dialog from the server's answer.
fn probed(
    dialog: &DownloadInfoDialog,
    config: &Config,
    pending: &mut Pending,
    result: ProbeResult,
) {
    dialog.set_probing(false);
    match result {
        Ok(probe) => {
            pending.server_name = probe.file_name.clone();
            let category = config.category_for(pending.name());
            let index = category
                .as_ref()
                .and_then(|name| {
                    config
                        .settings()
                        .categories
                        .iter()
                        .position(|c| &c.name == name)
                })
                .map_or(0, |i| i as i32 + 1);
            dialog.set_category_index(index);
            dialog.set_kind(file_kind(pending.name()));
            dialog.set_size(
                probe
                    .total_size
                    .map_or("Unknown".into(), format::size)
                    .into(),
            );
            if !pending.path_edited {
                let dir = pending.dir(config, category.as_deref());
                set_save_path(dialog, &dir, pending.name());
            }
        }
        Err(message) => dialog.set_error(message.into()),
    }
}

/// The category chosen in the dialog, or `None` for its first entry.
fn category(dialog: &DownloadInfoDialog) -> Option<String> {
    use slint::Model;
    let index = usize::try_from(dialog.get_category_index())
        .ok()
        .filter(|&i| i > 0)?;
    dialog.get_categories().row_data(index).map(String::from)
}

/// Sets Save As to `dir` joined with a name based on `name` that is free in `dir`.
fn set_save_path(dialog: &DownloadInfoDialog, dir: &Path, name: &str) {
    let path = dir.join(unique_file_name(dir, name));
    dialog.set_save_path(path.to_string_lossy().as_ref().into());
}

/// Splits a Save As path into its folder and file name.
fn split_path(path: &str) -> (PathBuf, String) {
    let path = Path::new(path.trim());
    let dir = path.parent().map(Path::to_path_buf).unwrap_or_default();
    let name = path
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    (dir, name)
}

#[cfg(test)]
mod tests {
    use velo_core::Settings;

    use super::*;
    use crate::app::tests::Recorder;

    /// Answers every probe at once with `answer`.
    struct FixedProber(ProbeResult);

    impl Prober for FixedProber {
        fn probe(&self, _: String, _: Settings, _: Option<Credentials>, done: ProbeDone) {
            done(self.0.clone());
        }
    }

    fn probe(name: &str, size: Option<u64>) -> ProbeResult {
        Ok(Probe {
            url: Url::parse("http://example.test/x").unwrap(),
            total_size: size,
            resumable: true,
            file_name: name.into(),
            etag: None,
            last_modified: None,
        })
    }

    fn adder(answer: ProbeResult, downloads: &Path) -> (Rc<Adder>, Rc<Config>, Recorder) {
        i_slint_backend_testing::init_no_event_loop();
        let config = Rc::new(Config::new(Settings::default(), None, downloads.into()));
        let recorder = Recorder::default();
        let adder = Adder::new(
            config.clone(),
            Rc::new(recorder.clone()),
            Rc::new(FixedProber(answer)),
        );
        (adder, config, recorder)
    }

    #[test]
    fn only_http_addresses_are_accepted() {
        assert!(is_download_url(" https://example.test/a.zip "));
        assert!(is_download_url("http://127.0.0.1:8080/"));
        assert!(!is_download_url("ftp://example.test/a"));
        assert!(!is_download_url("example.test/a"));
        assert!(!is_download_url("http://"));
        assert!(!is_download_url(""));
    }

    #[test]
    fn add_url_leads_to_file_info_and_starts_the_download() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("movie.mkv"), "taken").unwrap();
        let (adder, config, recorder) = adder(probe("movie.mkv", Some(3 << 20)), dir.path());

        adder.open().unwrap();
        let dialog = adder.add_url.borrow().as_ref().unwrap().clone_strong();
        dialog.set_url("http://example.test/movie.mkv".into());
        dialog.invoke_url_edited(dialog.get_url());
        assert!(dialog.get_url_valid());
        dialog.set_use_authorization(true);
        dialog.set_user("me".into());
        dialog.set_password("pw".into());
        dialog.invoke_accepted();
        assert!(adder.add_url.borrow().is_none());

        let info = adder.infos.borrow().values().next().unwrap().clone_strong();
        assert!(!info.get_probing() && info.get_error().is_empty());
        assert_eq!(info.get_size(), "3.00 MB");
        assert_eq!(info.get_kind(), crate::FileKind::Video);
        assert_eq!(
            info.get_category_index(),
            5,
            "None, Compressed, Documents, Music, Programs, Video"
        );
        let expected = dir.path().join("movie (1).mkv");
        assert_eq!(info.get_save_path(), expected.to_string_lossy().as_ref());

        info.set_description("holiday".into());
        info.set_remember_path(true);
        info.invoke_start_download();
        assert!(adder.infos.borrow().is_empty());
        let commands = recorder.0.borrow();
        assert_eq!(commands.len(), 1);
        let added = &commands[0];
        assert!(
            added.starts_with("add http://example.test/movie.mkv"),
            "{added}"
        );
        assert!(
            added.contains("movie (1).mkv") && added.contains("holiday"),
            "{added}"
        );
        assert!(
            added.contains("start=true") && added.contains("user=me"),
            "{added}"
        );
        let settings = config.settings();
        assert_eq!(settings.url_history, ["http://example.test/movie.mkv"]);
        let video = settings
            .categories
            .iter()
            .find(|c| c.name == "Video")
            .unwrap();
        assert_eq!(video.save_dir.as_deref(), Some(dir.path()));
    }

    #[test]
    fn a_failed_probe_shows_the_error_and_nothing_is_added() {
        let dir = tempfile::tempdir().unwrap();
        let (adder, _, recorder) = adder(Err("server answered 404 Not Found".into()), dir.path());
        adder
            .open_info(
                "http://example.test/missing".into(),
                None,
                Preset::default(),
            )
            .unwrap();
        let info = adder.infos.borrow().values().next().unwrap().clone_strong();
        assert_eq!(info.get_error(), "server answered 404 Not Found");
        info.invoke_cancelled();
        assert!(adder.infos.borrow().is_empty());
        assert!(recorder.0.borrow().is_empty());
    }

    #[test]
    fn a_preset_folder_and_name_fill_save_as() {
        let dir = tempfile::tempdir().unwrap();
        let (adder, _, _) = adder(probe("server.bin", Some(10)), Path::new("/downloads"));
        let preset = Preset {
            save_dir: Some(dir.path().into()),
            file_name: Some("mine.mp3".into()),
        };
        adder
            .open_info("http://example.test/x".into(), None, preset)
            .unwrap();
        let info = adder.infos.borrow().values().next().unwrap().clone_strong();
        assert_eq!(
            info.get_save_path(),
            dir.path().join("mine.mp3").to_string_lossy().as_ref()
        );
        assert_eq!(info.get_kind(), crate::FileKind::Music);
    }

    #[test]
    fn silent_adds_skip_the_dialogs_and_wait_in_the_main_queue_unless_started() {
        let (adder, _, recorder) = adder(probe("movie.mkv", None), Path::new("/downloads"));
        adder.add_silently(
            "http://example.test/movie.mkv".into(),
            Preset::default(),
            false,
        );
        adder.add_silently(
            "http://example.test/b".into(),
            Preset {
                save_dir: Some("/elsewhere".into()),
                file_name: Some("b.zip".into()),
            },
            true,
        );
        assert!(adder.add_url.borrow().is_none() && adder.infos.borrow().is_empty());
        let commands = recorder.0.borrow();
        assert_eq!(commands.len(), 3, "{commands:?}");
        assert!(
            commands[0].starts_with("add http://example.test/movie.mkv")
                && commands[0].contains("Video")
                && commands[0].contains("start=false"),
            "{}",
            commands[0]
        );
        assert!(commands[1].starts_with("enqueue"), "{}", commands[1]);
        assert!(
            commands[2].contains("/elsewhere") && commands[2].contains("b.zip"),
            "{}",
            commands[2]
        );
    }

    #[test]
    fn changing_the_category_moves_the_file_unless_the_path_was_edited() {
        let dir = tempfile::tempdir().unwrap();
        let (adder, config, _) = adder(probe("song.mp3", None), dir.path());
        config.remember_dir("Documents", Path::new("/docs"));
        adder
            .open_info(
                "http://example.test/song.mp3".into(),
                None,
                Preset::default(),
            )
            .unwrap();
        let info = adder.infos.borrow().values().next().unwrap().clone_strong();
        assert_eq!(info.get_size(), "Unknown");

        info.set_category_index(2);
        info.invoke_category_changed(2);
        assert_eq!(
            info.get_save_path(),
            Path::new("/docs")
                .join("song.mp3")
                .to_string_lossy()
                .as_ref()
        );

        info.set_save_path("/elsewhere/tune.mp3".into());
        info.invoke_save_path_edited();
        info.set_category_index(0);
        info.invoke_category_changed(0);
        assert_eq!(info.get_save_path(), "/elsewhere/tune.mp3");
    }
}
