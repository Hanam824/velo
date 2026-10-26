//! Which toolbar buttons and menu items are enabled.

use velo_core::{Download, DownloadState};

use crate::AppWindow;

/// The enabled state of the toolbar buttons that depend on the downloads.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct ToolbarState {
    /// Resume: the selected download is queued, paused or failed.
    pub resume: bool,
    /// Stop: the selected download is running.
    pub stop: bool,
    /// Stop All: some download is running.
    pub stop_all: bool,
    /// Delete: a download is selected.
    pub delete: bool,
    /// Delete Completed: some download is complete.
    pub delete_completed: bool,
}

impl ToolbarState {
    /// The state for `selected` (if any) among `downloads`.
    pub fn new(selected: Option<&Download>, downloads: &[Download]) -> Self {
        Self {
            resume: selected.is_some_and(|d| {
                matches!(
                    d.state,
                    DownloadState::Queued | DownloadState::Paused | DownloadState::Failed(_)
                )
            }),
            stop: selected.is_some_and(Download::is_running),
            stop_all: downloads.iter().any(Download::is_running),
            delete: selected.is_some(),
            delete_completed: downloads.iter().any(Download::is_finished),
        }
    }

    /// Sets the window's toolbar and menu properties.
    pub fn apply(&self, ui: &AppWindow) {
        ui.set_can_resume(self.resume);
        ui.set_can_stop(self.stop);
        ui.set_can_stop_all(self.stop_all);
        ui.set_can_delete(self.delete);
        ui.set_can_delete_completed(self.delete_completed);
    }
}

#[cfg(test)]
mod tests {
    use velo_core::DownloadId;

    use super::*;

    fn download(id: u64, state: DownloadState) -> Download {
        let mut download = Download::new(DownloadId(id), "http://example.test/f", "f", "/tmp");
        download.state = state;
        download
    }

    #[test]
    fn nothing_selected_enables_only_the_list_wide_buttons() {
        let downloads = [
            download(1, DownloadState::Downloading),
            download(2, DownloadState::Complete),
        ];
        let state = ToolbarState::new(None, &downloads);
        assert_eq!(
            state,
            ToolbarState {
                stop_all: true,
                delete_completed: true,
                ..ToolbarState::default()
            }
        );
        assert_eq!(ToolbarState::new(None, &[]), ToolbarState::default());
    }

    #[test]
    fn the_selection_decides_resume_stop_and_delete() {
        let running = download(1, DownloadState::Downloading);
        let paused = download(2, DownloadState::Paused);
        let failed = download(3, DownloadState::Failed("x".into()));
        let complete = download(4, DownloadState::Complete);
        let all = [
            running.clone(),
            paused.clone(),
            failed.clone(),
            complete.clone(),
        ];

        let state = ToolbarState::new(Some(&running), &all);
        assert!(state.stop && !state.resume && state.delete);
        for stopped in [&paused, &failed] {
            let state = ToolbarState::new(Some(stopped), &all);
            assert!(state.resume && !state.stop && state.delete);
        }
        let state = ToolbarState::new(Some(&complete), &all);
        assert!(!state.resume && !state.stop && state.delete);
    }

    #[test]
    fn the_state_reaches_the_window() {
        i_slint_backend_testing::init_no_event_loop();
        let ui = AppWindow::new().unwrap();
        let state = ToolbarState {
            resume: true,
            delete_completed: true,
            ..ToolbarState::default()
        };
        state.apply(&ui);
        assert!(ui.get_can_resume() && ui.get_can_delete_completed());
        assert!(!ui.get_can_stop() && !ui.get_can_stop_all() && !ui.get_can_delete());
    }
}
