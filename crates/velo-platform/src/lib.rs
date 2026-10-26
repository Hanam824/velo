//! OS integration (paths, opening files, tray, autostart, shutdown). All OS-specific code lives here.
//!
//! Every function behaves the same on Windows, macOS and Linux; only where things live differs,
//! following each OS's conventions.

mod dialogs;
mod dirs;

pub use dialogs::pick_save_path;
pub use dirs::AppDirs;

/// Errors from OS integration.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// The OS doesn't say where the user's home folder is, so there is nowhere to keep data.
    #[error("couldn't find the home folder")]
    NoHome,
}

/// Result with [`Error`] as the default error type.
pub type Result<T, E = Error> = std::result::Result<T, E>;
