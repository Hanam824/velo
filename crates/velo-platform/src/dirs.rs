//! Where Velo keeps its files.

use std::path::{Path, PathBuf};

use etcetera::{AppStrategy, AppStrategyArgs};

use crate::{Error, Result};

/// The folders Velo uses, in each OS's usual places.
///
/// | | Windows | macOS | Linux |
/// |---|---|---|---|
/// | data | `%APPDATA%\Velo\data` | `~/Library/Application Support/Velo` | `~/.local/share/velo` |
/// | config | `%APPDATA%\Velo\config` | `~/Library/Preferences/Velo` | `~/.config/velo` |
/// | parts | `%LOCALAPPDATA%\Velo\cache\parts` | `~/Library/Caches/Velo/parts` | `~/.cache/velo/parts` |
/// | downloads | `~\Downloads` | `~/Downloads` | `~/Downloads` |
///
/// A Linux system with a translated Downloads folder name still gets `~/Downloads`; the user
/// can pick another folder in the settings.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AppDirs {
    data: PathBuf,
    config: PathBuf,
    parts: PathBuf,
    downloads: PathBuf,
}

impl AppDirs {
    /// Finds the folders for the current user. Nothing is created.
    pub fn new() -> Result<Self> {
        let app = etcetera::app_strategy::choose_native_strategy(AppStrategyArgs {
            top_level_domain: String::new(),
            author: String::new(),
            app_name: "Velo".into(),
        })
        .map_err(|_| Error::NoHome)?;
        Ok(Self {
            data: app.data_dir(),
            config: app.config_dir(),
            parts: app.cache_dir().join("parts"),
            downloads: app.home_dir().join("Downloads"),
        })
    }

    /// The download database, `velo.db` in the data folder.
    pub fn database(&self) -> PathBuf {
        self.data.join("velo.db")
    }

    /// The settings file, `settings.toml` in the config folder.
    pub fn settings(&self) -> PathBuf {
        self.config.join("settings.toml")
    }

    /// The default folder for part files while downloading.
    pub fn parts(&self) -> &Path {
        &self.parts
    }

    /// The default folder finished downloads are saved to.
    pub fn downloads(&self) -> &Path {
        &self.downloads
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn files_live_in_velo_folders() {
        let dirs = AppDirs::new().unwrap();
        assert_eq!(dirs.database().file_name().unwrap(), "velo.db");
        assert_eq!(dirs.settings().file_name().unwrap(), "settings.toml");
        assert!(dirs.parts().ends_with("parts"));
        for path in [dirs.database(), dirs.settings(), dirs.parts().to_path_buf()] {
            let text = path.to_string_lossy().to_lowercase();
            assert!(text.contains("velo"), "{text}");
            assert!(path.is_absolute());
        }
        assert!(dirs.downloads().is_absolute());
    }
}
