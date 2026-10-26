//! The settings file.

use std::path::Path;

use velo_core::Settings;

use crate::Result;

/// Loads the settings from the TOML file at `path`; a missing file gives the defaults.
///
/// Fields missing from the file take their defaults, so a file from an older Velo still loads.
pub fn load_settings(path: &Path) -> Result<Settings> {
    match std::fs::read_to_string(path) {
        Ok(text) => Ok(toml::from_str(&text)?),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Settings::default()),
        Err(e) => Err(e.into()),
    }
}

/// Writes `settings` to the TOML file at `path`, creating its folder if needed.
///
/// The file is written next to `path` first and then renamed over it, so a crash never leaves
/// a half-written settings file.
pub fn save_settings(path: &Path, settings: &Settings) -> Result<()> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let mut temp = path.as_os_str().to_os_string();
    temp.push(".tmp");
    std::fs::write(&temp, toml::to_string_pretty(settings)?)?;
    std::fs::rename(&temp, path)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_missing_file_gives_the_defaults() {
        let dir = tempfile::tempdir().unwrap();
        let settings = load_settings(&dir.path().join("settings.toml")).unwrap();
        assert_eq!(settings, Settings::default());
    }

    #[test]
    fn settings_round_trip_and_missing_fields_take_defaults() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config").join("settings.toml");
        let mut settings = Settings {
            max_connections: 16,
            temp_dir: Some(dir.path().join("parts")),
            ..Settings::default()
        };
        settings.categories[0].save_dir = Some(dir.path().join("zips"));
        save_settings(&path, &settings).unwrap();
        assert_eq!(load_settings(&path).unwrap(), settings);

        std::fs::write(&path, "max_connections = 4\n").unwrap();
        let partial = load_settings(&path).unwrap();
        assert_eq!(partial.max_connections, 4);
        assert_eq!(partial.categories, Settings::default().categories);
    }
}
