//! Native file dialogs.

use std::path::{Path, PathBuf};

/// Shows the system's Save dialog, starting in `dir` with `file_name` filled in, and returns
/// the chosen path, or `None` if the user cancelled.
///
/// Blocks until the dialog closes; call it from the UI thread, which macOS requires.
pub fn pick_save_path(dir: &Path, file_name: &str) -> Option<PathBuf> {
    rfd::FileDialog::new()
        .set_directory(dir)
        .set_file_name(file_name)
        .save_file()
}

/// Shows the system's folder picker, starting in `dir`, and returns the chosen folder, or
/// `None` if the user cancelled.
///
/// Blocks until the dialog closes; call it from the UI thread, which macOS requires.
pub fn pick_folder(dir: &Path) -> Option<PathBuf> {
    rfd::FileDialog::new().set_directory(dir).pick_folder()
}
