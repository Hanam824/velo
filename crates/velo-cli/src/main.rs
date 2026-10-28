//! `velo-cli`: adds downloads to a running Velo app, or runs them headless.

use std::process::ExitCode;

use clap::Parser;
use velo_cli::{Cli, Places, run};

fn main() -> ExitCode {
    let cli = Cli::parse();
    let result = Places::system()
        .map_err(|e| e.to_string())
        .and_then(|places| {
            // `VELO_LOG=debug` also prints the log to stderr.
            let _log = velo_log::init(velo_log::Config {
                app: "velo-cli",
                version: env!("CARGO_PKG_VERSION"),
                dir: places.data.clone(),
                level: velo_store::load_settings(&places.settings)
                    .ok()
                    .map(|s| s.log_level.filter().into()),
                stderr: std::env::var_os("VELO_LOG").is_some(),
            });
            run(&cli, &places).map_err(|e| e.to_string())
        });
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("velo-cli: {error}");
            ExitCode::FAILURE
        }
    }
}
