//! Persistence for the download list, segment state and settings.
//!
//! Downloads and their segments live in a SQLite database ([`Store`]); settings live in a TOML
//! file ([`load_settings`], [`save_settings`]). Both formats are versioned: the database schema
//! changes only through a new entry in its migration list, and settings fields missing from the
//! file take their defaults.

mod db;
mod settings;

use std::path::PathBuf;

pub use db::Store;
pub use settings::{load_settings, save_settings};

/// Errors from the store.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// A database query failed.
    #[error("database error: {0}")]
    Sqlite(#[from] rusqlite::Error),
    /// Reading or writing a file failed.
    #[error("file error: {0}")]
    Io(#[from] std::io::Error),
    /// The settings file isn't valid TOML for [`Settings`](velo_core::Settings).
    #[error("invalid settings file: {0}")]
    SettingsFormat(#[from] toml::de::Error),
    /// The settings couldn't be written as TOML.
    #[error("couldn't write settings: {0}")]
    SettingsWrite(#[from] toml::ser::Error),
    /// The database was written by a newer Velo, with a schema this version doesn't know.
    #[error("the database has schema version {found}, newer than this Velo's {supported}")]
    NewerSchema {
        /// The database's `user_version`.
        found: i64,
        /// The newest version this build can read.
        supported: i64,
    },
    /// A path isn't valid UTF-8, so it can't be stored the same way on every platform.
    #[error("path {0} isn't valid UTF-8")]
    NonUtf8Path(PathBuf),
    /// A stored value isn't one this version knows.
    #[error("unknown stored value: {0}")]
    UnknownValue(String),
}

/// Result with [`Error`] as the default error type.
pub type Result<T, E = Error> = std::result::Result<T, E>;
