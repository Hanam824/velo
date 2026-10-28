//! Logging for every Velo program: one setup, size-rotated log files, and helpers that keep
//! secrets out of them.
//!
//! Libraries log with the `tracing` macros (`tracing::warn!(%error, "couldn't save")`) and
//! never print; each program calls [`init`] first thing in `main`. Events go to
//! `<dir>/<app>.log`, and also to standard error when asked. Describe addresses, headers and
//! paths through [`redact`] before logging them.
//!
//! The level comes from the `VELO_LOG` environment variable when set, else from
//! [`Config::level`]; either is a level (`error`, `warn`, `info`, `debug`, `trace`) or a list
//! of `target=level` directives such as `velo_engine=debug,info`.

pub mod redact;
mod rotate;

use std::path::PathBuf;
use std::sync::Arc;

use tracing_subscriber::EnvFilter;
use tracing_subscriber::fmt;
use tracing_subscriber::prelude::*;

pub use rotate::Rotating;

/// The biggest a log file grows before it is moved aside.
const MAX_BYTES: u64 = 4 << 20;

/// How many old log files are kept next to the current one.
const KEEP: u32 = 3;

/// Noisy libraries stay at `info` unless a directive names them, so `debug` shows Velo's own
/// detail, not every HTTP/2 frame.
const QUIET: &str = "h2=info,hyper=info,hyper_util=info,rustls=info,reqwest=info";

/// How a program logs.
#[derive(Debug, Clone)]
pub struct Config {
    /// The program's name, which names the file: `velo` writes `velo.log`.
    pub app: &'static str,
    /// The program's version, written in the first line of each run.
    pub version: &'static str,
    /// The folder for log files; created if missing.
    pub dir: PathBuf,
    /// The level to use when `VELO_LOG` isn't set, such as from the settings; `info` if `None`.
    pub level: Option<String>,
    /// Whether to also write to standard error.
    pub stderr: bool,
}

/// Keeps logging working while it lives, and flushes the file when dropped. Hold it in `main`.
#[derive(Debug)]
pub struct Guard(Option<Arc<Rotating>>);

impl Drop for Guard {
    fn drop(&mut self) {
        if let Some(file) = &self.0 {
            file.flush();
        }
    }
}

/// The filter directives for `spec`: a bare level gets the noisy libraries held at `info`.
fn directives(spec: &str) -> String {
    let spec = spec.trim();
    if spec.is_empty() {
        format!("info,{QUIET}")
    } else if spec.contains('=') || spec.contains(',') {
        spec.to_string()
    } else {
        format!("{spec},{QUIET}")
    }
}

/// The filter for `config`, from `VELO_LOG` or `config.level`; an unreadable value means `info`.
fn filter(config: &Config) -> EnvFilter {
    let spec = std::env::var("VELO_LOG")
        .ok()
        .filter(|v| !v.trim().is_empty())
        .or_else(|| config.level.clone())
        .unwrap_or_default();
    EnvFilter::try_new(directives(&spec)).unwrap_or_else(|_| EnvFilter::new("info"))
}

/// Starts logging for the program described by `config`, and hooks panics into the log.
///
/// Never fails: when the file can't be opened the program just runs without a file log. Only
/// the first call in a process has an effect.
pub fn init(config: Config) -> Guard {
    let file = Arc::new(Rotating::open(
        &config.dir.join(format!("{}.log", config.app)),
        MAX_BYTES,
        KEEP,
    ));
    let writer = {
        let file = file.clone();
        move || rotate::Handle(file.clone())
    };
    let file_layer = fmt::layer().with_writer(writer).with_ansi(false);
    let stderr_layer = config
        .stderr
        .then(|| fmt::layer().with_writer(std::io::stderr));
    let started = tracing_subscriber::registry()
        .with(filter(&config))
        .with(file_layer)
        .with(stderr_layer)
        .try_init()
        .is_ok();
    if started {
        let previous = std::panic::take_hook();
        std::panic::set_hook(Box::new(move |info| {
            let place = info
                .location()
                .map(|l| format!("{}:{}", l.file(), l.line()))
                .unwrap_or_default();
            let message = info
                .payload()
                .downcast_ref::<&str>()
                .map(|s| s.to_string())
                .or_else(|| info.payload().downcast_ref::<String>().cloned())
                .unwrap_or_default();
            tracing::error!(place, message, "panic");
            previous(info);
        }));
        tracing::info!(
            app = config.app,
            version = config.version,
            os = std::env::consts::OS,
            arch = std::env::consts::ARCH,
            pid = std::process::id(),
            "started"
        );
    }
    Guard(Some(file))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_bare_level_quiets_the_libraries_and_directives_are_kept() {
        assert_eq!(directives("debug"), format!("debug,{QUIET}"));
        assert_eq!(directives(""), format!("info,{QUIET}"));
        assert_eq!(
            directives("velo_engine=debug,info"),
            "velo_engine=debug,info"
        );
    }
}
