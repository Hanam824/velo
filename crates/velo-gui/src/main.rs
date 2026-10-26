//! Velo Download Manager desktop app.

// No console window behind the app in Windows release builds (no-op elsewhere).
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod add;
mod app;
mod bridge;
mod categories;
mod config;
mod format;
mod list;
#[cfg(test)]
mod snapshot;
mod toolbar;

use std::rc::Rc;
use std::time::Duration;

use velo_core::{EngineEvent, Settings};
use velo_engine::Options;
use velo_platform::AppDirs;
use velo_store::Store;

use crate::add::Adder;
use crate::app::{App, Commands};
use crate::bridge::{Bridge, BridgeConfig};
use crate::config::Config;

slint::include_modules!();

/// How often the UI takes in engine events.
const UI_REFRESH: Duration = Duration::from_millis(250);

/// Creates the main window and its state, sending engine commands to `commands`.
fn create_window(
    settings: &Settings,
    commands: Rc<dyn Commands>,
) -> Result<(AppWindow, Rc<App>), slint::PlatformError> {
    let ui = AppWindow::new()?;
    ui.set_version(env!("CARGO_PKG_VERSION").into());
    let app = App::new(&ui, settings, commands);
    ui.on_quit(|| {
        let _ = slint::quit_event_loop();
    });
    Ok((ui, app))
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let dirs = AppDirs::new()?;
    let settings = velo_store::load_settings(&dirs.settings())?;
    let store = Store::open(&dirs.database())?;
    let downloads = store.load_downloads()?;
    let bridge = Bridge::start(BridgeConfig {
        temp_dir: settings
            .temp_dir
            .clone()
            .unwrap_or_else(|| dirs.parts().to_path_buf()),
        options: Options::from_settings(&settings),
        store,
        downloads: downloads.clone(),
    })?;

    let commands: Rc<dyn Commands> = Rc::new(bridge.engine().clone());
    let (ui, app) = create_window(&settings, commands.clone())?;
    app.apply(downloads.into_iter().map(EngineEvent::Added));
    let bridge = Rc::new(bridge);
    let config = Rc::new(Config::new(
        settings,
        Some(dirs.settings()),
        dirs.downloads().to_path_buf(),
    ));
    let adder = Adder::new(config, commands, bridge.clone());
    ui.on_add_url(move || {
        if let Err(error) = adder.open() {
            eprintln!("velo: couldn't open Add URL: {error}");
        }
    });
    let timer = slint::Timer::default();
    {
        let (app, bridge) = (Rc::clone(&app), Rc::clone(&bridge));
        timer.start(slint::TimerMode::Repeated, UI_REFRESH, move || {
            app.apply(bridge.drain());
        });
    }
    ui.run()?;

    // Pause what is running and save it before exiting.
    drop(timer);
    if let Ok(bridge) = Rc::try_unwrap(bridge) {
        bridge.shutdown();
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use slint::Model;
    use velo_core::{DownloadId, DownloadState, EngineEvent, ProgressUpdate};

    use super::*;
    use crate::app::tests::Recorder;
    use crate::list::tests::sample;

    #[test]
    fn window_shows_the_version_categories_and_switches_appearance() {
        i_slint_backend_testing::init_no_event_loop();
        let (ui, _app) = create_window(&Settings::default(), Rc::new(Recorder::default())).unwrap();
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
        let (ui, app) = create_window(&Settings::default(), Rc::new(Recorder::default())).unwrap();
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
            app.apply([EngineEvent::Added(download)]);
        }
        app.apply([EngineEvent::Progress(ProgressUpdate {
            id: DownloadId(1),
            downloaded: 2_350 * mb,
            total: Some(6_100 * mb),
            rate: 18 * mb + 400 * 1024,
            segments: Vec::new(),
        })]);
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
        ui.hide().unwrap();

        let add = AddUrlDialog::new().unwrap();
        add.set_url("https://releases.example.org/ubuntu-26.04-desktop-amd64.iso".into());
        add.set_url_valid(true);
        add.set_history(slint::ModelRc::new(slint::VecModel::from(vec![
            slint::StandardListViewItem::from("https://example.org/report-q3.pdf"),
            slint::StandardListViewItem::from("https://cdn.example.net/album.zip"),
        ])));
        snapshot::save(&window, &add, 520, 330, &dir.join("add-url.png"));
        add.hide().unwrap();

        let info = DownloadInfoDialog::new().unwrap();
        info.set_url("https://releases.example.org/ubuntu-26.04-desktop-amd64.iso".into());
        info.set_categories(slint::ModelRc::new(slint::VecModel::from(vec![
            slint::SharedString::from("None"),
            "Compressed".into(),
            "Programs".into(),
        ])));
        info.set_category_index(2);
        info.set_save_path("/Users/me/Downloads/ubuntu-26.04-desktop-amd64.iso".into());
        info.set_size("5.96 GB".into());
        info.set_kind(FileKind::Program);
        info.set_probing(false);
        snapshot::save(&window, &info, 600, 260, &dir.join("download-info.png"));
    }
}
