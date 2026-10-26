//! Velo Download Manager desktop app.

// No console window behind the app in Windows release builds (no-op elsewhere).
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod app;
mod categories;
mod format;
mod list;
#[cfg(test)]
mod snapshot;
mod toolbar;

use std::rc::Rc;

use velo_core::Settings;

use crate::app::App;

slint::include_modules!();

/// Creates the main window and its state.
fn create_window(settings: &Settings) -> Result<(AppWindow, Rc<App>), slint::PlatformError> {
    let ui = AppWindow::new()?;
    ui.set_version(env!("CARGO_PKG_VERSION").into());
    let app = App::new(&ui, settings);
    ui.on_quit(|| {
        let _ = slint::quit_event_loop();
    });
    Ok((ui, app))
}

fn main() -> Result<(), slint::PlatformError> {
    let (ui, _app) = create_window(&Settings::default())?;
    ui.run()
}

#[cfg(test)]
mod tests {
    use slint::Model;
    use velo_core::{DownloadId, DownloadState, EngineEvent, ProgressUpdate};

    use super::*;
    use crate::list::tests::sample;

    #[test]
    fn window_shows_the_version_categories_and_switches_appearance() {
        i_slint_backend_testing::init_no_event_loop();
        let (ui, _app) = create_window(&Settings::default()).unwrap();
        assert_eq!(ui.get_version(), env!("CARGO_PKG_VERSION"));
        assert_eq!(ui.get_categories().row_data(0).unwrap().key, "all");
        let rows = ui.get_categories().row_count();
        ui.invoke_category_toggled("finished".into());
        assert!(ui.get_categories().row_count() > rows);
        assert_eq!(ui.get_selected_category(), "all");

        let theme = ui.global::<Theme>();
        assert_eq!(theme.get_appearance(), Appearance::System);
        theme.invoke_apply(Appearance::Dark);
        assert!(theme.get_dark());
        theme.invoke_apply(Appearance::Light);
        assert!(!theme.get_dark());
    }

    /// Writes `main-light.png` and `main-dark.png` to `$VELO_SNAPSHOT_DIR`, with a few sample
    /// downloads in the list; see [`snapshot`].
    #[test]
    #[ignore = "installs its own Slint platform; run alone with --ignored"]
    fn snapshot() {
        let dir = std::path::PathBuf::from(
            std::env::var_os("VELO_SNAPSHOT_DIR").expect("set VELO_SNAPSHOT_DIR"),
        );
        std::fs::create_dir_all(&dir).unwrap();
        let window = snapshot::init();
        let (ui, app) = create_window(&Settings::default()).unwrap();
        let mb = 1024 * 1024;
        for download in [
            sample(
                1,
                "ubuntu-26.04-desktop-amd64.iso",
                DownloadState::Downloading,
                0,
                6_100 * mb,
            ),
            sample(
                2,
                "holiday-video.mp4",
                DownloadState::Paused,
                412 * mb,
                1_250 * mb,
            ),
            sample(3, "report-q3.pdf", DownloadState::Complete, 3 * mb, 3 * mb),
            sample(
                4,
                "setup.exe",
                DownloadState::Failed("server answered 404".into()),
                0,
                80 * mb,
            ),
            sample(5, "album.zip", DownloadState::Queued, 0, 220 * mb),
            sample(
                6,
                "podcast-episode-42.mp3",
                DownloadState::Complete,
                58 * mb,
                58 * mb,
            ),
        ] {
            app.apply(EngineEvent::Added(download));
        }
        app.apply(EngineEvent::Progress(ProgressUpdate {
            id: DownloadId(1),
            downloaded: 2_350 * mb,
            total: Some(6_100 * mb),
            rate: 18 * mb + 400 * 1024,
            segments: Vec::new(),
        }));
        ui.set_selected_id(2);
        ui.invoke_selection_changed(2);
        for (appearance, name) in [(Appearance::Light, "light"), (Appearance::Dark, "dark")] {
            ui.global::<Theme>().invoke_apply(appearance);
            snapshot::save(
                &window,
                &ui,
                1100,
                560,
                &dir.join(format!("main-{name}.png")),
            );
        }
    }
}
