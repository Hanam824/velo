//! OS integration (paths, opening files, tray, autostart, shutdown). All OS-specific code lives here.
//!
//! Every function behaves the same on Windows, macOS and Linux; only where things live differs,
//! following each OS's conventions.

mod dialogs;
mod dirs;
mod files;
mod power;

pub use dialogs::pick_save_path;
pub use dirs::AppDirs;
pub use files::{open, reveal};
pub use power::shutdown;

/// Errors from OS integration.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// The OS doesn't say where the user's home folder is, so there is nowhere to keep data.
    #[error("couldn't find the home folder")]
    NoHome,
    /// The system couldn't open or show a file.
    #[error("couldn't open {path}: {message}")]
    Open {
        /// The file or folder.
        path: std::path::PathBuf,
        /// What the system said.
        message: String,
    },
    /// A system command failed to start or reported failure.
    #[error("couldn't run {command}: {message}")]
    Command {
        /// The command.
        command: String,
        /// What went wrong.
        message: String,
    },
}

/// Result with [`Error`] as the default error type.
pub type Result<T, E = Error> = std::result::Result<T, E>;
