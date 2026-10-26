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

/// Shows the system's Open dialog for one file, starting in `dir` and offering files with the
/// extensions in `filter` (named `filter_name`) first, and returns the chosen file, or `None`
/// if the user cancelled.
///
/// Blocks until the dialog closes; call it from the UI thread, which macOS requires.
pub fn pick_file(dir: &Path, filter_name: &str, filter: &[&str]) -> Option<PathBuf> {
    rfd::FileDialog::new()
        .set_directory(dir)
        .add_filter(filter_name, filter)
        .add_filter("All files", &["*"])
        .pick_file()
}
