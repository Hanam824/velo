//! Velo Download Manager desktop app.

// No console window behind the app in Windows release builds (no-op elsewhere).
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod add;
mod app;
mod batch;
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

use crate::add::{Adder, Preset, is_download_url};
use crate::app::{App, Commands};
use crate::batch::BatchDialog;
use crate::bridge::{Bridge, BridgeConfig};
use crate::complete::{Desktop, SystemDesktop};
use crate::config::Config;
use crate::options::OptionsDialog;

slint::include_modules!();

/// How often the UI takes in engine events.
const UI_REFRESH: Duration = Duration::from_millis(250);

/// How often the clipboard is checked for a copied download address.
const CLIPBOARD_POLL: Duration = Duration::from_secs(1);

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
    // Velo runs once per user: a second launch shows the first one's window and exits.
    let endpoint = velo_ipc::Endpoint::for_data_dir(dirs.data());
    if velo_ipc::send(&endpoint, &velo_ipc::Message::Activate).is_ok() {
        return Ok(());
    }
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
    let open_add = {
        let adder = adder.clone();
        move || {
            if let Err(error) = adder.open() {
                eprintln!("velo: couldn't open Add URL: {error}");
            }
        }
    };
    ui.on_add_url(open_add.clone());
    let batch = BatchDialog::new(config.clone(), adder.clone());
    ui.on_add_batch(move || {
        if let Err(error) = batch.open() {
            eprintln!("velo: couldn't open Add Batch Download: {error}");
        }
    });
    {
        let (adder, config) = (adder.clone(), config.clone());
        ui.on_import_list(move || import_list(&adder, &config));
    }
    {
        let (app, config) = (Rc::downgrade(&app), config.clone());
        ui.on_export_list(move || {
            if let Some(app) = app.upgrade() {
                export_list(&app.urls(), &config);
            }
        });
    }

    // Messages from other processes wait here for the UI thread, which takes them on its timer.
    let (requests_tx, requests) = std::sync::mpsc::channel();
    velo_ipc::listen(&endpoint, move |message| {
        if let velo_ipc::Message::Add { url, .. } = &message
            && !is_download_url(url)
        {
            return Err(format!("{url} isn't an http or https address"));
        }
        requests_tx
            .send(message)
            .map_err(|_| "Velo is closing".to_string())
    })?;
    let tray = match VeloTray::new() {
        Ok(tray) => {
            let shown = ui.as_weak();
            tray.on_open(move || {
                if let Some(ui) = shown.upgrade() {
                    show_main(&ui);
                }
            });
            tray.on_add_url(open_add);
            tray.on_exit(|| {
                let _ = slint::quit_event_loop();
            });
            Some(tray)
        }
        Err(error) => {
            eprintln!("velo: no tray icon: {error}");
            None
        }
    };
    // Closing the window keeps Velo in the tray if the user wants that and there is a tray.
    let has_tray = tray.is_some();
    let close_config = config.clone();
    ui.window().on_close_requested(move || {
        if !(has_tray && close_config.settings().close_to_tray) {
            let _ = slint::quit_event_loop();
        }
        slint::CloseRequestResponse::HideWindow
    });
    let timer = slint::Timer::default();
    {
        let (app, bridge, weak, adder) = (
            Rc::clone(&app),
            Rc::clone(&bridge),
            ui.as_weak(),
            adder.clone(),
        );
        timer.start(slint::TimerMode::Repeated, UI_REFRESH, move || {
            app.apply(bridge.drain());
            for message in requests.try_iter() {
                if let Some(ui) = weak.upgrade() {
                    handle_request(&ui, &adder, &*commands, message);
                }
            }
        });
    }
    // Offers to download an address copied anywhere, if its file type is one to capture.
    let clipboard_timer = slint::Timer::default();
    {
        let (adder, config) = (adder.clone(), config.clone());
        let mut clipboard = velo_platform::ClipboardWatcher::new();
        clipboard_timer.start(slint::TimerMode::Repeated, CLIPBOARD_POLL, move || {
            let settings = config.settings();
            if !settings.monitor_clipboard {
                return;
            }
            if let Some(url) = clipboard
                .copied()
                .and_then(|text| velo_core::links::capture(&text, &settings.capture_extensions))
                && let Err(error) = adder.open_info(url, None, Preset::default())
            {
                eprintln!("velo: couldn't open Download File Info: {error}");
            }
        });
    }
    ui.show()?;
    slint::run_event_loop_until_quit()?;

    // Pause what is running and save it before exiting.
    drop(timer);
    drop(clipboard_timer);
    drop(tray);
    bridge.shutdown();
    Ok(())
}

/// Adds every address in a list file the user picks to the main queue.
fn import_list(adder: &Adder, config: &Config) {
    let Some(path) = velo_platform::pick_file(&config.save_dir(None), "Address lists", &["txt"])
    else {
        return;
    };
    let message = match std::fs::read_to_string(&path) {
        Ok(text) => {
            let urls = velo_core::links::read_list(&text);
            adder.add_many(&urls, None, false);
            match urls.len() {
                1 => "Added 1 download to the main queue.".to_string(),
                n => format!("Added {n} downloads to the main queue."),
            }
        }
        Err(error) => format!("Couldn't read {}: {error}", path.display()),
    };
    tell("Import List", &message);
}

/// Writes `urls` to a list file the user picks, one per line.
fn export_list(urls: &[String], config: &Config) {
    let Some(path) = velo_platform::pick_save_path(&config.save_dir(None), "velo-downloads.txt")
    else {
        return;
    };
    let text = velo_core::links::write_list(urls.iter().map(String::as_str));
    let message = match std::fs::write(&path, text) {
        Ok(()) => format!("Saved {} addresses to {}.", urls.len(), path.display()),
        Err(error) => format!("Couldn't save {}: {error}", path.display()),
    };
    tell("Export List", &message);
}

/// Shows `message` as a notification titled `title`, or on stderr if notifications fail.
fn tell(title: &str, message: &str) {
    if velo_platform::notify(title, message).is_err() {
        eprintln!("velo: {message}");
    }
}

/// Carries out a request from another process, such as `velo-cli` or a second launch.
fn handle_request(
    ui: &AppWindow,
    adder: &Rc<Adder>,
    commands: &dyn Commands,
    message: velo_ipc::Message,
) {
    match message {
        velo_ipc::Message::Activate => show_main(ui),
        velo_ipc::Message::Ping => {}
        velo_ipc::Message::Add {
            url,
            save_dir,
            file_name,
            start,
            silent,
        } => {
            let preset = Preset {
                save_dir,
                file_name,
            };
            if silent {
                adder.add_silently(url, preset, start);
            } else if let Err(error) = adder.open_info(url, None, preset) {
                eprintln!("velo: couldn't open Download File Info: {error}");
            }
        }
        velo_ipc::Message::StartQueue { queue } => {
            commands.start_queue(queue.unwrap_or(velo_core::QueueId::MAIN));
        }
    }
}

/// Shows the main window, also if it was closed to the tray or minimized.
fn show_main(ui: &AppWindow) {
    if let Err(error) = ui.show() {
        eprintln!("velo: couldn't show the window: {error}");
    }
    ui.window().set_minimized(false);
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

        let batch = BatchWindow::new().unwrap();
        batch.set_pattern("https://photos.example.org/trip/IMG_*.jpg".into());
        batch.set_from("1".into());
        batch.set_to("120".into());
        batch.set_digits(4);
        batch.set_default_folder("/Users/me/Downloads".into());
        batch.set_count(120);
        batch.set_preview(slint::ModelRc::new(slint::VecModel::from(
            (1..=8)
                .map(|n| {
                    slint::StandardListViewItem::from(
                        format!("https://photos.example.org/trip/IMG_{n:04}.jpg").as_str(),
                    )
                })
                .collect::<Vec<_>>(),
        )));
        snapshot::save(&window, &batch, 560, 460, &dir.join("batch.png"));
        batch.hide().unwrap();

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
