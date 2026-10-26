//! Turning the computer off.

use std::process::Command;

use crate::{Error, Result};

/// The command that turns the computer off on this OS, with its arguments.
///
/// Every OS asks running apps to quit first, as a normal shutdown from its menu does.
fn shutdown_command() -> (&'static str, &'static [&'static str]) {
    #[cfg(windows)]
    {
        ("shutdown", &["/s", "/t", "0"])
    }
    #[cfg(target_os = "macos")]
    {
        (
            "osascript",
            &["-e", "tell application \"System Events\" to shut down"],
        )
    }
    #[cfg(all(unix, not(target_os = "macos")))]
    {
        ("systemctl", &["poweroff"])
    }
}

/// Turns the computer off.
///
/// Returns once the OS has accepted the request; the shutdown itself happens after that.
pub fn shutdown() -> Result<()> {
    let (program, args) = shutdown_command();
    let failure = |message: String| Error::Command {
        command: format!("{program} {}", args.join(" ")),
        message,
    };
    let status = Command::new(program)
        .args(args)
        .status()
        .map_err(|e| failure(e.to_string()))?;
    if status.success() {
        Ok(())
    } else {
        Err(failure(status.to_string()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn there_is_a_shutdown_command() {
        let (program, args) = shutdown_command();
        assert!(!program.is_empty() && !args.is_empty());
    }
}
