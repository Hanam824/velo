//! The user's settings as the running app shares them, and where they are saved.

use std::cell::RefCell;
use std::path::{Path, PathBuf};

use velo_core::{Category, Settings};

/// The settings, shared by the windows on the UI thread.
#[derive(Debug)]
pub struct Config {
    settings: RefCell<Settings>,
    /// The settings file; `None` keeps changes in memory only (tests).
    path: Option<PathBuf>,
    /// The downloads folder when the settings don't name one.
    default_downloads: PathBuf,
}

impl Config {
    /// Wraps `settings`, saving changes to `path` if given.
    pub fn new(settings: Settings, path: Option<PathBuf>, default_downloads: PathBuf) -> Self {
        Self {
            settings: RefCell::new(settings),
            path,
            default_downloads,
        }
    }

    /// A copy of the current settings.
    pub fn settings(&self) -> Settings {
        self.settings.borrow().clone()
    }

    /// Changes the settings with `f` and saves them; a failed save is reported and the change
    /// is kept in memory.
    pub fn update(&self, f: impl FnOnce(&mut Settings)) {
        f(&mut self.settings.borrow_mut());
        if let Some(path) = &self.path
            && let Err(error) = velo_store::save_settings(path, &self.settings.borrow())
        {
            tracing::warn!(%error, "couldn't save the settings");
        }
    }

    /// The folder files in `category` are saved to: the category's own folder, else the
    /// downloads folder from the settings, else the system's.
    pub fn save_dir(&self, category: Option<&str>) -> PathBuf {
        self.settings
            .borrow()
            .save_dir_for(category, &self.default_downloads)
    }

    /// The downloads folder used when the settings don't name one.
    pub fn default_downloads(&self) -> &Path {
        &self.default_downloads
    }

    /// The category whose extensions include `file_name`'s.
    pub fn category_for(&self, file_name: &str) -> Option<String> {
        Category::for_file_name(&self.settings.borrow().categories, file_name)
            .map(|c| c.name.clone())
    }

    /// Makes `dir` the folder of `category` from now on.
    pub fn remember_dir(&self, category: &str, dir: &Path) {
        self.update(|settings| {
            if let Some(c) = settings.categories.iter_mut().find(|c| c.name == category) {
                c.save_dir = Some(dir.to_path_buf());
            }
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn save_folders_fall_back_from_category_to_settings_to_system() {
        let config = Config::new(
            Settings::default(),
            None,
            PathBuf::from("/home/u/Downloads"),
        );
        assert_eq!(
            config.save_dir(Some("Video")),
            Path::new("/home/u/Downloads")
        );
        config.update(|s| s.download_dir = Some("/data".into()));
        assert_eq!(config.save_dir(None), Path::new("/data"));
        config.remember_dir("Video", Path::new("/media/videos"));
        assert_eq!(config.save_dir(Some("Video")), Path::new("/media/videos"));
        assert_eq!(config.save_dir(Some("Music")), Path::new("/data"));
        assert_eq!(config.category_for("clip.MP4").as_deref(), Some("Video"));
    }

    #[test]
    fn changes_are_saved_to_the_settings_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("settings.toml");
        let config = Config::new(Settings::default(), Some(path.clone()), dir.path().into());
        config.update(|s| s.remember_url("http://example.test/a"));
        let saved = velo_store::load_settings(&path).unwrap();
        assert_eq!(saved.url_history, ["http://example.test/a"]);
    }
}
