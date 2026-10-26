//! User settings that the engine and the app read.

use std::path::{Path, PathBuf};

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
    /// Addresses entered in Add URL, newest first, at most [`Settings::URL_HISTORY`].
    pub url_history: Vec<String>,
    /// Total download speed limit in bytes per second; no limit if unset.
    pub speed_limit: Option<u64>,
    /// User agent sent to servers; Velo's own (`Velo/<version>`) if unset.
    pub user_agent: Option<String>,
    /// How to reach servers.
    pub proxy: ProxySettings,
    /// Connection counts for particular servers, overriding [`Settings::max_connections`].
    pub connection_limits: Vec<ConnectionLimit>,
    /// Whether a download that starts shows its progress dialog.
    pub show_progress_dialog: bool,
    /// Whether a download that finishes shows the Download complete dialog.
    pub show_complete_dialog: bool,
    /// Light, dark, or as the operating system is set.
    pub appearance: Appearance,
    /// Whether closing the main window keeps Velo running in the tray.
    pub close_to_tray: bool,
    /// Whether an address copied to the clipboard is offered for download if its file has one
    /// of [`Settings::capture_extensions`].
    pub monitor_clipboard: bool,
    /// File extensions, lowercase and without the dot, that Velo offers to download when such
    /// an address is copied.
    pub capture_extensions: Vec<String>,
}

/// The colour scheme of Velo's windows.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Appearance {
    /// As the operating system is set.
    #[default]
    System,
    /// Light.
    Light,
    /// Dark.
    Dark,
}

/// How Velo connects to servers.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ProxyMode {
    /// Directly, ignoring any system proxy.
    None,
    /// Through the proxy the operating system or the environment (`HTTPS_PROXY`…) names.
    #[default]
    System,
    /// Through [`ProxySettings::url`].
    Manual,
}

/// Proxy settings.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct ProxySettings {
    /// Which proxy to use.
    pub mode: ProxyMode,
    /// The proxy for [`ProxyMode::Manual`]: `http://`, `https://` or `socks5://` with host and
    /// port, such as `socks5://127.0.0.1:1080`.
    pub url: String,
}

/// A connection count for one server and its subdomains.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ConnectionLimit {
    /// The server, such as `example.com`; it also covers `dl.example.com`.
    pub host: String,
    /// Connections per download from that server, from 1 to [`Settings::MAX_CONNECTIONS`].
    pub max_connections: u32,
}

impl Settings {
    /// Highest allowed [`Settings::max_connections`].
    pub const MAX_CONNECTIONS: u32 = 32;

    /// How many addresses [`Settings::url_history`] keeps.
    pub const URL_HISTORY: usize = 20;

    /// Puts `url` at the front of the address history, removing an older copy and the oldest
    /// entry if the history is full.
    pub fn remember_url(&mut self, url: &str) {
        self.url_history.retain(|u| u != url);
        self.url_history.insert(0, url.to_string());
        self.url_history.truncate(Self::URL_HISTORY);
    }

    /// The folder files in `category` are saved to: the category's own folder, else
    /// [`Settings::download_dir`], else `default_downloads` (the system's downloads folder).
    pub fn save_dir_for(&self, category: Option<&str>, default_downloads: &Path) -> PathBuf {
        category
            .and_then(|name| self.categories.iter().find(|c| c.name == name))
            .and_then(|c| c.save_dir.clone())
            .or_else(|| self.download_dir.clone())
            .unwrap_or_else(|| default_downloads.to_path_buf())
    }

    /// Connections for a download from `host`: the most specific matching
    /// [`ConnectionLimit`], else [`Settings::max_connections`].
    pub fn connections_for(&self, host: &str) -> u32 {
        let host = host.to_ascii_lowercase();
        self.connection_limits
            .iter()
            .filter(|limit| {
                let rule = limit.host.trim_start_matches('.').to_ascii_lowercase();
                host == rule || host.ends_with(&format!(".{rule}"))
            })
            .max_by_key(|limit| limit.host.len())
            .map_or(self.max_connections, |limit| limit.max_connections)
    }

    /// Checks that every value is in range.
    pub fn validate(&self) -> Result<()> {
        let connections = std::iter::once(self.max_connections)
            .chain(self.connection_limits.iter().map(|l| l.max_connections));
        for count in connections {
            if !(1..=Self::MAX_CONNECTIONS).contains(&count) {
                return Err(Error::Invalid(format!(
                    "connection counts must be between 1 and {}, got {count}",
                    Self::MAX_CONNECTIONS,
                )));
            }
        }
        if self.speed_limit == Some(0) {
            return Err(Error::Invalid(
                "the speed limit must be greater than 0".into(),
            ));
        }
        if self.proxy.mode == ProxyMode::Manual
            && !["http://", "https://", "socks5://"]
                .iter()
                .any(|scheme| self.proxy.url.starts_with(scheme))
        {
            return Err(Error::Invalid(
                "the proxy must start with http://, https:// or socks5://".into(),
            ));
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
            url_history: Vec::new(),
            speed_limit: None,
            user_agent: None,
            proxy: ProxySettings::default(),
            connection_limits: Vec::new(),
            show_progress_dialog: true,
            show_complete_dialog: true,
            appearance: Appearance::System,
            close_to_tray: true,
            monitor_clipboard: true,
            capture_extensions: default_capture_extensions(),
        }
    }
}

/// Every extension of the built-in categories, sorted.
fn default_capture_extensions() -> Vec<String> {
    let mut extensions: Vec<String> = Category::defaults()
        .into_iter()
        .flat_map(|c| c.extensions)
        .collect();
    extensions.sort();
    extensions.dedup();
    extensions
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn server_limits_cover_subdomains_and_the_most_specific_wins() {
        let settings = Settings {
            connection_limits: vec![
                ConnectionLimit {
                    host: "example.com".into(),
                    max_connections: 2,
                },
                ConnectionLimit {
                    host: "cdn.example.com".into(),
                    max_connections: 16,
                },
            ],
            ..Settings::default()
        };
        assert_eq!(settings.connections_for("example.com"), 2);
        assert_eq!(settings.connections_for("DL.Example.com"), 2);
        assert_eq!(settings.connections_for("a.cdn.example.com"), 16);
        assert_eq!(settings.connections_for("notexample.com"), 8);
        assert_eq!(settings.connections_for("other.org"), 8);
    }

    #[test]
    fn speed_limits_proxies_and_server_limits_are_validated() {
        let bad = [
            Settings {
                speed_limit: Some(0),
                ..Settings::default()
            },
            Settings {
                proxy: ProxySettings {
                    mode: ProxyMode::Manual,
                    url: "127.0.0.1:8080".into(),
                },
                ..Settings::default()
            },
            Settings {
                connection_limits: vec![ConnectionLimit {
                    host: "x.org".into(),
                    max_connections: 0,
                }],
                ..Settings::default()
            },
        ];
        for settings in bad {
            assert!(settings.validate().is_err(), "{settings:?}");
        }
        let good = Settings {
            speed_limit: Some(100 * 1024),
            proxy: ProxySettings {
                mode: ProxyMode::Manual,
                url: "socks5://127.0.0.1:1080".into(),
            },
            ..Settings::default()
        };
        assert!(good.validate().is_ok());
    }

    #[test]
    fn the_url_history_keeps_the_newest_once() {
        let mut settings = Settings::default();
        for n in 0..25 {
            settings.remember_url(&format!("http://example.test/{n}"));
        }
        settings.remember_url("http://example.test/10");
        assert_eq!(settings.url_history.len(), Settings::URL_HISTORY);
        assert_eq!(settings.url_history[0], "http://example.test/10");
        assert_eq!(settings.url_history[1], "http://example.test/24");
        assert_eq!(
            settings
                .url_history
                .iter()
                .filter(|u| u.ends_with("/10"))
                .count(),
            1
        );
    }

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
