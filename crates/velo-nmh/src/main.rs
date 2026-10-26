//! `velo-nmh`: the native-messaging host the browser starts for Velo's extension.
//!
//! Browsers start it with the extension's origin as an argument (and on Windows a window
//! handle), which it doesn't need.

use std::path::PathBuf;
use std::process::ExitCode;
use std::time::{Duration, Instant};

use velo_core::Settings;
use velo_extract::{Extractor, Video};
use velo_ipc::Endpoint;
use velo_nmh::{Backend, serve};
use velo_platform::AppDirs;

/// How long to wait for the app to start listening after starting it.
const START_TIMEOUT: Duration = Duration::from_secs(10);

/// The real backend: the app over its local socket, and YouTube.
struct Real {
    dirs: AppDirs,
    endpoint: Endpoint,
    settings: Settings,
}

impl Backend for Real {
    fn send(&self, message: velo_ipc::Message) -> Result<(), String> {
        match velo_ipc::send(&self.endpoint, &message) {
            Err(velo_ipc::Error::NotRunning) => {}
            other => return other.map_err(|e| e.to_string()),
        }
        let app = app_path().ok_or("Velo isn't installed next to its browser helper")?;
        velo_platform::launch_detached(&app, &[]).map_err(|e| e.to_string())?;
        let deadline = Instant::now() + START_TIMEOUT;
        loop {
            std::thread::sleep(Duration::from_millis(200));
            match velo_ipc::send(&self.endpoint, &message) {
                Err(velo_ipc::Error::NotRunning) if Instant::now() < deadline => {}
                other => return other.map_err(|e| e.to_string()),
            }
        }
    }

    fn video(&self, url: &str) -> Result<Video, String> {
        let client = velo_engine::client_with(&self.settings, &Default::default())
            .map_err(|e| e.to_string())?;
        let extractor =
            Extractor::new(client).with_cache_dir(self.dirs.data().join("player-cache"));
        tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .map_err(|e| e.to_string())?
            .block_on(extractor.video_at(url))
            .map_err(|e| e.to_string())
    }

    fn capture_extensions(&self) -> Vec<String> {
        self.settings.capture_extensions.clone()
    }
}

/// The app's program, next to this one.
fn app_path() -> Option<PathBuf> {
    let path = std::env::current_exe()
        .ok()?
        .with_file_name(format!("velo{}", std::env::consts::EXE_SUFFIX));
    path.is_file().then_some(path)
}

fn main() -> ExitCode {
    let dirs = match AppDirs::new() {
        Ok(dirs) => dirs,
        Err(error) => {
            eprintln!("velo-nmh: {error}");
            return ExitCode::FAILURE;
        }
    };
    let settings = velo_store::load_settings(&dirs.settings()).unwrap_or_else(|error| {
        eprintln!("velo-nmh: using the default settings: {error}");
        Settings::default()
    });
    let backend = Real {
        endpoint: Endpoint::for_data_dir(dirs.data()),
        dirs,
        settings,
    };
    match serve(
        &mut std::io::stdin().lock(),
        &mut std::io::stdout().lock(),
        &backend,
    ) {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("velo-nmh: {error}");
            ExitCode::FAILURE
        }
    }
}
