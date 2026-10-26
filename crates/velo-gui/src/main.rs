//! Velo Download Manager desktop app.

// No console window behind the app in Windows release builds (no-op elsewhere).
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod add;
mod app;
mod bridge;
mod categories;
mod category;
mod complete;
mod config;
mod format;
mod list;
mod options;
mod progress;
mod properties;
mod scheduler;
#[cfg(test)]
mod snapshot;
mod theme;
mod toolbar;

use std::rc::Rc;
use std::time::Duration;

use velo_core::EngineEvent;
use velo_engine::RetryPolicy;
use velo_platform::AppDirs;
use velo_store::Store;

use crate::add::Adder;
use crate::app::{App, Commands};
use crate::bridge::{Bridge, BridgeConfig};
use crate::complete::{Desktop, SystemDesktop};
use crate::config::Config;
use crate::options::OptionsDialog;

slint::include_modules!();

/// How often the UI takes in engine events.
const UI_REFRESH: Duration = Duration::from_millis(250);

/// Creates the main window and its state, sending engine commands to `commands` and desktop
/// actions to `desktop`.
fn create_window(
    config: Rc<Config>,
    commands: Rc<dyn Commands>,
    desktop: Rc<dyn Desktop>,
) -> Result<(AppWindow, Rc<App>), slint::PlatformError> {
    let ui = AppWindow::new()?;
    ui.set_version(env!("CARGO_PKG_VERSION").into());
    let app = App::new(&ui, config, commands, desktop);
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
    let queues = store.load_queues()?;
    let bridge = Bridge::start(BridgeConfig {
        temp_dir: settings
            .temp_dir
            .clone()
            .unwrap_or_else(|| dirs.parts().to_path_buf()),
        settings: settings.clone(),
        retry: RetryPolicy::default(),
        store,
        downloads: downloads.clone(),
        queues,
    })?;

    let commands: Rc<dyn Commands> = Rc::new(bridge.engine().clone());
    let config = Rc::new(Config::new(
        settings,
        Some(dirs.settings()),
        dirs.downloads().to_path_buf(),
    ));
    let (ui, app) = create_window(config.clone(), commands.clone(), Rc::new(SystemDesktop))?;
    app.apply(downloads.into_iter().map(EngineEvent::Added));
    let bridge = Rc::new(bridge);
    let adder = Adder::new(config.clone(), commands.clone(), bridge.clone());
    let (shown, show_config) = (Rc::downgrade(&app), config.clone());
    adder.on_started(move |id| {
        if let Some(app) = shown.upgrade()
            && show_config.settings().show_progress_dialog
        {
            app.show_progress(id);
        }
    });
    let options = OptionsDialog::new(
        config.clone(),
        commands.clone(),
        dirs.downloads().to_path_buf(),
        dirs.parts().to_path_buf(),
    );
    let main = ui.as_weak();
    options.on_saved(move |settings| {
        if let Some(ui) = main.upgrade() {
            theme::apply(&ui, settings.appearance);
        }
    });
    ui.on_options(move || {
        if let Err(error) = options.open() {
            eprintln!("velo: couldn't open Options: {error}");
        }
    });
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
    use velo_core::{DownloadId, DownloadState, EngineEvent, ProgressUpdate, Settings};

    use super::*;
    use crate::app::tests::Recorder;
    use crate::complete::tests::FakeDesktop;

    /// Settings kept in memory only.
    fn test_config() -> Rc<Config> {
        Rc::new(Config::new(Settings::default(), None, "/downloads".into()))
    }
    use crate::list::tests::sample;

    #[test]
    fn window_shows_the_version_categories_and_switches_appearance() {
        i_slint_backend_testing::init_no_event_loop();
        let (ui, _app) = create_window(
            test_config(),
            Rc::new(Recorder::default()),
            Rc::new(FakeDesktop::default()),
        )
        .unwrap();
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
        let (ui, app) = create_window(
            test_config(),
            Rc::new(Recorder::default()),
            Rc::new(FakeDesktop::default()),
        )
        .unwrap();
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
            segments: sample_segments(6_100 * mb),
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
        info.hide().unwrap();

        app.show_progress(DownloadId(1));
        let progress = app.progress_window(DownloadId(1)).unwrap();
        snapshot::save(&window, &progress, 560, 520, &dir.join("progress.png"));
        progress.hide().unwrap();

        let complete = CompleteWindow::new().unwrap();
        complete.set_address("https://example.org/report-q3.pdf".into());
        complete.set_size("3.00 MB".into());
        complete.set_saved_as("/Users/me/Downloads/report-q3.pdf".into());
        complete.set_kind(FileKind::Document);
        snapshot::save(&window, &complete, 520, 220, &dir.join("complete.png"));
        complete.hide().unwrap();

        let properties = PropertiesWindow::new().unwrap();
        properties.set_file_name("holiday-video.mp4".into());
        properties.set_location("/Users/me/Movies".into());
        properties.set_url("https://media.example.net/holiday-video.mp4".into());
        properties.set_status("Paused".into());
        properties.set_size("1.22 GB".into());
        properties.set_kind(FileKind::Video);
        properties.set_editable(true);
        snapshot::save(&window, &properties, 560, 260, &dir.join("properties.png"));
        properties.hide().unwrap();

        let options = OptionsWindow::new().unwrap();
        options.global::<Theme>().invoke_apply(Appearance::Dark);
        options.set_connection_choices(slint::ModelRc::new(slint::VecModel::from(vec![
            slint::SharedString::from("8"),
        ])));
        options.set_limit_enabled(true);
        options.set_server_limits(slint::ModelRc::new(slint::VecModel::from(vec![
            ServerLimit {
                host: "cdn.example.net".into(),
                connections: 16,
            },
        ])));
        snapshot::save(&window, &options, 640, 480, &dir.join("options-dark.png"));
        options.hide().unwrap();

        let category = CategoryWindow::new().unwrap();
        category.set_name("Images".into());
        category.set_extensions("png jpg webp".into());
        category.set_default_folder("/Users/me/Downloads".into());
        snapshot::save(&window, &category, 480, 180, &dir.join("category.png"));
        category.hide().unwrap();

        let scheduler = SchedulerWindow::new().unwrap();
        scheduler.set_queue_names(slint::ModelRc::new(slint::VecModel::from(vec![
            slint::StandardListViewItem::from("Main download queue  (running)"),
            slint::StandardListViewItem::from("Night"),
        ])));
        scheduler.set_current_queue(1);
        scheduler.set_name("Night".into());
        scheduler.set_start_enabled(true);
        scheduler.set_start_hour(1);
        scheduler.set_start_minute(30);
        scheduler.set_days(slint::ModelRc::new(slint::VecModel::from(vec![
            true, true, true, true, true, false, false,
        ])));
        scheduler.set_stop_enabled(true);
        scheduler.set_stop_hour(6);
        snapshot::save(&window, &scheduler, 760, 460, &dir.join("scheduler.png"));
    }

    /// Six segments of a `total`-byte file in different states, as the planner would leave them.
    fn sample_segments(total: u64) -> Vec<velo_core::Segment> {
        use velo_core::{Segment, SegmentId, SegmentState};
        let bounds = [0.0, 0.25, 0.5, 0.62, 0.75, 0.87, 1.0];
        let done = [1.0, 0.7, 0.55, 0.4, 0.2, 0.0];
        (0..6)
            .map(|i| {
                let start = (bounds[i] * total as f64) as u64;
                let end = (bounds[i + 1] * total as f64) as u64;
                let mut segment = Segment::new(SegmentId(i as u32), start, end);
                segment.downloaded = ((end - start) as f64 * done[i]) as u64;
                segment.state = if done[i] == 1.0 {
                    SegmentState::Done
                } else {
                    SegmentState::Active
                };
                segment
            })
            .collect()
    }
}
