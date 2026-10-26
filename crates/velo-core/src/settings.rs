//! User settings that the engine and the app read.

use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use crate::{Category, Error, Result};

/// User settings; `velo-store` keeps them in a TOML file.
///
/// Fields missing from the file take their [`Default`] values, so new settings don't break old
/// files.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    /// Connections per download, from 1 to [`Settings::MAX_CONNECTIONS`].
    pub max_connections: u32,
    /// Smallest range the planner gives a new connection, in bytes; a segment is split only if
    /// both halves would be at least this long.
    pub min_segment_size: u64,
    /// Folder for part files; the platform's cache directory if unset.
    pub temp_dir: Option<PathBuf>,
    /// Folder for downloads whose category has no folder; the platform's downloads folder if unset.
    pub download_dir: Option<PathBuf>,
    /// Categories, in the order they are matched and shown.
    pub categories: Vec<Category>,
}

impl Settings {
    /// Highest allowed [`Settings::max_connections`].
    pub const MAX_CONNECTIONS: u32 = 32;

    /// Checks that every value is in range.
    pub fn validate(&self) -> Result<()> {
        if !(1..=Self::MAX_CONNECTIONS).contains(&self.max_connections) {
            return Err(Error::Invalid(format!(
                "max_connections must be between 1 and {}, got {}",
                Self::MAX_CONNECTIONS,
                self.max_connections
            )));
        }
        if self.min_segment_size == 0 {
            return Err(Error::Invalid(
                "min_segment_size must be greater than 0".into(),
            ));
        }
        Ok(())
    }
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            max_connections: 8,
            min_segment_size: 256 * 1024,
            temp_dir: None,
            download_dir: None,
            categories: Category::defaults(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_are_valid_and_out_of_range_values_are_not() {
        assert!(Settings::default().validate().is_ok());
        for max_connections in [0, Settings::MAX_CONNECTIONS + 1] {
            let settings = Settings {
                max_connections,
                ..Settings::default()
            };
            assert!(settings.validate().is_err());
        }
        let settings = Settings {
            min_segment_size: 0,
            ..Settings::default()
        };
        assert!(settings.validate().is_err());
    }
}
