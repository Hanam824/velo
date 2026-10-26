//! The Options dialog.

use std::cell::RefCell;
use std::path::{Path, PathBuf};
use std::rc::Rc;

use slint::{ComponentHandle, Model, ModelRc, SharedString, VecModel};
use velo_core::{ConnectionLimit, ProxyMode, ProxySettings, Settings};

use crate::app::Commands;
use crate::config::Config;
use crate::{CategoryFolder, OptionsWindow, ServerLimit, theme};

/// The connection counts offered, as in IDM.
const CONNECTION_CHOICES: [u32; 7] = [1, 2, 4, 8, 16, 24, 32];

/// What runs with the settings Options just saved.
type OnSaved = Box<dyn Fn(&Settings)>;

/// Opens Options and applies what the user saves.
pub struct OptionsDialog {
    config: Rc<Config>,
    commands: Rc<dyn Commands>,
    /// The default folders, shown when the settings leave them empty.
    default_downloads: PathBuf,
    default_temp: PathBuf,
    window: RefCell<Option<OptionsWindow>>,
    /// Runs after the settings are saved, to update what depends on them.
    on_saved: RefCell<Option<OnSaved>>,
}

impl OptionsDialog {
    /// Creates the dialog manager.
    pub fn new(
        config: Rc<Config>,
        commands: Rc<dyn Commands>,
        default_downloads: PathBuf,
        default_temp: PathBuf,
    ) -> Rc<Self> {
        Rc::new(Self {
            config,
            commands,
            default_downloads,
            default_temp,
            window: RefCell::default(),
            on_saved: RefCell::default(),
        })
    }

    /// Calls `f` with the new settings each time Options saves them.
    pub fn on_saved(&self, f: impl Fn(&Settings) + 'static) {
        *self.on_saved.borrow_mut() = Some(Box::new(f));
    }

    /// Shows Options filled from the current settings, or brings it forward if open.
    pub fn open(self: &Rc<Self>) -> Result<(), slint::PlatformError> {
        if let Some(window) = self.window.borrow().as_ref() {
            return window.show();
        }
        let window = OptionsWindow::new()?;
        let settings = self.config.settings();
        theme::apply(&window, settings.appearance);
        fill(
            &window,
            &settings,
            &self.default_downloads,
            &self.default_temp,
        );

        let weak = window.as_weak();
        window.on_browse_download_dir(move || {
            if let Some(window) = weak.upgrade()
                && let Some(dir) = pick(
                    &window.get_download_dir(),
                    &window.get_default_download_dir(),
                )
            {
                window.set_download_dir(dir);
            }
        });
        let weak = window.as_weak();
        window.on_browse_temp_dir(move || {
            if let Some(window) = weak.upgrade()
                && let Some(dir) = pick(&window.get_temp_dir(), &window.get_default_temp_dir())
            {
                window.set_temp_dir(dir);
            }
        });
        let weak = window.as_weak();
        window.on_category_folder_edited(move |index, folder| {
            if let Some(window) = weak.upgrade() {
                set_category_folder(&window, index, folder);
            }
        });
        let weak = window.as_weak();
        window.on_browse_category_folder(move |index| {
            let Some(window) = weak.upgrade() else { return };
            let current = window
                .get_category_folders()
                .row_data(index as usize)
                .map(|row| row.folder)
                .unwrap_or_default();
            if let Some(dir) = pick(&current, &window.get_default_download_dir()) {
                set_category_folder(&window, index, dir);
            }
        });
        let weak = window.as_weak();
        window.on_add_server_limit(move || {
            let Some(window) = weak.upgrade() else { return };
            let host = window.get_new_host().trim().to_lowercase();
            if host.is_empty() {
                return;
            }
            let mut rows: Vec<ServerLimit> = window.get_server_limits().iter().collect();
            rows.retain(|row| row.host != host.as_str());
            rows.push(ServerLimit {
                host: host.into(),
                connections: connections_at(window.get_new_host_index()) as i32,
            });
            window.set_server_limits(ModelRc::new(VecModel::from(rows)));
            window.set_new_host(SharedString::new());
        });
        let weak = window.as_weak();
        window.on_remove_server_limit(move |index| {
            let Some(window) = weak.upgrade() else { return };
            let mut rows: Vec<ServerLimit> = window.get_server_limits().iter().collect();
            if (0..rows.len() as i32).contains(&index) {
                rows.remove(index as usize);
            }
            window.set_server_limits(ModelRc::new(VecModel::from(rows)));
        });
        let this = Rc::downgrade(self);
        window.on_accepted(move || {
            if let Some(this) = this.upgrade() {
                this.save();
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

    /// Saves the dialog's settings if they are valid, sends them to the engine and closes;
    /// otherwise shows why not.
    fn save(&self) {
        let Some(window) = self.window.borrow().as_ref().map(|w| w.clone_strong()) else {
            return;
        };
        let settings = collect(&window, self.config.settings());
        if let Err(error) = settings.validate() {
            window.set_error(error.to_string().into());
            return;
        }
        self.config.update(|current| *current = settings.clone());
        self.commands.set_settings(settings.clone());
        if let Some(on_saved) = self.on_saved.borrow().as_ref() {
            on_saved(&settings);
        }
        self.window.take();
        let _ = window.hide();
    }

    /// The open Options window.
    #[cfg(test)]
    pub fn window(&self) -> Option<OptionsWindow> {
        self.window.borrow().as_ref().map(|w| w.clone_strong())
    }
}

/// Fills `window` from `settings`.
fn fill(window: &OptionsWindow, settings: &Settings, downloads: &Path, temp: &Path) {
    let text = |path: &Option<PathBuf>| -> SharedString {
        path.as_ref()
            .map(|p| p.to_string_lossy().into_owned())
            .unwrap_or_default()
            .into()
    };
    window.set_download_dir(text(&settings.download_dir));
    window.set_default_download_dir(downloads.to_string_lossy().as_ref().into());
    window.set_temp_dir(text(&settings.temp_dir));
    window.set_default_temp_dir(temp.to_string_lossy().as_ref().into());
    let folders: Vec<CategoryFolder> = settings
        .categories
        .iter()
        .map(|c| CategoryFolder {
            name: format!("{}:", c.name).into(),
            folder: text(&c.save_dir),
        })
        .collect();
    window.set_category_folders(ModelRc::new(VecModel::from(folders)));

    window.set_show_progress(settings.show_progress_dialog);
    window.set_show_complete(settings.show_complete_dialog);
    window.set_close_to_tray(settings.close_to_tray);
    window.set_user_agent(settings.user_agent.clone().unwrap_or_default().into());
    window.set_default_user_agent(concat!("Velo/", env!("CARGO_PKG_VERSION")).into());

    let choices: Vec<SharedString> = CONNECTION_CHOICES
        .iter()
        .map(|n| n.to_string().into())
        .collect();
    window.set_connection_choices(ModelRc::new(VecModel::from(choices)));
    window.set_connections_index(choice_index(settings.max_connections));
    window.set_new_host_index(choice_index(2));
    window.set_limit_enabled(settings.speed_limit.is_some());
    if let Some(limit) = settings.speed_limit {
        window.set_limit_kb((limit / 1024).clamp(1, 1_000_000) as i32);
    }
    let limits: Vec<ServerLimit> = settings
        .connection_limits
        .iter()
        .map(|l| ServerLimit {
            host: l.host.as_str().into(),
            connections: l.max_connections as i32,
        })
        .collect();
    window.set_server_limits(ModelRc::new(VecModel::from(limits)));

    window.set_proxy_mode(match settings.proxy.mode {
        ProxyMode::None => 0,
        ProxyMode::System => 1,
        ProxyMode::Manual => 2,
    });
    window.set_proxy_url(settings.proxy.url.as_str().into());
    window.set_appearance(match settings.appearance {
        velo_core::Appearance::System => 0,
        velo_core::Appearance::Light => 1,
        velo_core::Appearance::Dark => 2,
    });
    window.set_error(SharedString::new());
}

/// The settings the dialog describes, starting from `settings` for what it doesn't show.
fn collect(window: &OptionsWindow, mut settings: Settings) -> Settings {
    let path = |text: SharedString| {
        let text = text.trim();
        (!text.is_empty()).then(|| PathBuf::from(text))
    };
    settings.download_dir = path(window.get_download_dir());
    settings.temp_dir = path(window.get_temp_dir());
    for (category, row) in settings
        .categories
        .iter_mut()
        .zip(window.get_category_folders().iter())
    {
        category.save_dir = path(row.folder);
    }
    settings.show_progress_dialog = window.get_show_progress();
    settings.show_complete_dialog = window.get_show_complete();
    settings.close_to_tray = window.get_close_to_tray();
    settings.user_agent =
        Some(window.get_user_agent().trim().to_string()).filter(|s| !s.is_empty());
    settings.max_connections = connections_at(window.get_connections_index());
    settings.speed_limit = window
        .get_limit_enabled()
        .then(|| u64::from(window.get_limit_kb().max(1) as u32) * 1024);
    settings.connection_limits = window
        .get_server_limits()
        .iter()
        .map(|row| ConnectionLimit {
            host: row.host.into(),
            max_connections: row.connections.max(1) as u32,
        })
        .collect();
    settings.proxy = ProxySettings {
        mode: match window.get_proxy_mode() {
            0 => ProxyMode::None,
            2 => ProxyMode::Manual,
            _ => ProxyMode::System,
        },
        url: window.get_proxy_url().trim().to_string(),
    };
    settings.appearance = match window.get_appearance() {
        1 => velo_core::Appearance::Light,
        2 => velo_core::Appearance::Dark,
        _ => velo_core::Appearance::System,
    };
    settings
}

/// The index in [`CONNECTION_CHOICES`] closest to `count` from below.
fn choice_index(count: u32) -> i32 {
    CONNECTION_CHOICES
        .iter()
        .rposition(|&c| c <= count)
        .unwrap_or(0) as i32
}

/// The connection count at `index` in [`CONNECTION_CHOICES`].
fn connections_at(index: i32) -> u32 {
    CONNECTION_CHOICES[(index.max(0) as usize).min(CONNECTION_CHOICES.len() - 1)]
}

/// Sets category row `index`'s folder.
fn set_category_folder(window: &OptionsWindow, index: i32, folder: SharedString) {
    let folders = window.get_category_folders();
    if let Some(mut row) = folders.row_data(index as usize) {
        row.folder = folder;
        folders.set_row_data(index as usize, row);
    }
}

/// Asks for a folder, starting at `current` or else `fallback`.
fn pick(current: &str, fallback: &str) -> Option<SharedString> {
    let start = if current.trim().is_empty() {
        fallback
    } else {
        current
    };
    velo_platform::pick_folder(Path::new(start)).map(|p| p.to_string_lossy().as_ref().into())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::tests::Recorder;

    fn dialog() -> (Rc<OptionsDialog>, Rc<Config>, Recorder) {
        i_slint_backend_testing::init_no_event_loop();
        let config = Rc::new(Config::new(
            Settings::default(),
            None,
            "/home/u/Downloads".into(),
        ));
        let recorder = Recorder::default();
        let dialog = OptionsDialog::new(
            config.clone(),
            Rc::new(recorder.clone()),
            "/home/u/Downloads".into(),
            "/home/u/.cache/velo/parts".into(),
        );
        (dialog, config, recorder)
    }

    #[test]
    fn the_dialog_shows_the_settings_and_saves_changes() {
        let (dialog, config, recorder) = dialog();
        let saved = Rc::new(RefCell::new(None));
        let seen = saved.clone();
        dialog.on_saved(move |s| *seen.borrow_mut() = Some(s.appearance));
        dialog.open().unwrap();
        let window = dialog.window().unwrap();
        assert_eq!(window.get_connections_index(), 3, "8 connections");
        assert_eq!(window.get_proxy_mode(), 1, "system proxy");
        assert!(!window.get_limit_enabled());

        window.set_download_dir("/data/downloads".into());
        window.invoke_category_folder_edited(4, "/data/video".into());
        window.set_connections_index(4);
        window.set_limit_enabled(true);
        window.set_limit_kb(512);
        window.set_new_host("Example.COM ".into());
        window.set_new_host_index(0);
        window.invoke_add_server_limit();
        window.set_proxy_mode(2);
        window.set_proxy_url("socks5://127.0.0.1:1080".into());
        window.set_appearance(2);
        window.set_user_agent("Mozilla/5.0".into());
        assert!(window.get_close_to_tray());
        window.set_close_to_tray(false);
        window.invoke_accepted();
        assert!(dialog.window().is_none());

        let settings = config.settings();
        assert_eq!(
            settings.download_dir.as_deref(),
            Some(Path::new("/data/downloads"))
        );
        assert_eq!(
            settings.categories[4].save_dir.as_deref(),
            Some(Path::new("/data/video"))
        );
        assert_eq!(settings.max_connections, 16);
        assert_eq!(settings.speed_limit, Some(512 * 1024));
        assert_eq!(
            settings.connection_limits,
            [ConnectionLimit {
                host: "example.com".into(),
                max_connections: 1
            }]
        );
        assert_eq!(settings.proxy.mode, ProxyMode::Manual);
        assert_eq!(settings.user_agent.as_deref(), Some("Mozilla/5.0"));
        assert!(!settings.close_to_tray);
        assert_eq!(*saved.borrow(), Some(velo_core::Appearance::Dark));
        assert_eq!(*recorder.0.borrow(), ["settings 16"]);
    }

    #[test]
    fn invalid_settings_stay_open_with_the_reason() {
        let (dialog, config, recorder) = dialog();
        dialog.open().unwrap();
        let window = dialog.window().unwrap();
        window.set_proxy_mode(2);
        window.set_proxy_url("localhost:8080".into());
        window.invoke_accepted();
        assert!(dialog.window().is_some());
        assert!(
            window.get_error().contains("proxy"),
            "{}",
            window.get_error()
        );
        assert_eq!(config.settings(), Settings::default());
        assert!(recorder.0.borrow().is_empty());

        window.invoke_remove_server_limit(0);
        window.invoke_cancelled();
        assert!(dialog.window().is_none());
    }

    #[test]
    fn connection_counts_map_to_the_offered_choices() {
        assert_eq!(choice_index(1), 0);
        assert_eq!(choice_index(8), 3);
        assert_eq!(choice_index(10), 3);
        assert_eq!(choice_index(32), 6);
        assert_eq!(connections_at(-1), 1);
        assert_eq!(connections_at(99), 32);
    }
}
