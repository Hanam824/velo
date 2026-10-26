//! `velo-cli`: adds downloads to a running Velo app, or runs them headless.

use std::process::ExitCode;

use clap::Parser;
use velo_cli::{Cli, Places, run};

fn main() -> ExitCode {
    let cli = Cli::parse();
    let result = Places::system()
        .map_err(|e| e.to_string())
        .and_then(|places| run(&cli, &places).map_err(|e| e.to_string()));
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("velo-cli: {error}");
            ExitCode::FAILURE
        }
    }
}
