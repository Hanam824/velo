//! Starting a program that outlives the one that starts it.

use std::path::Path;
use std::process::{Command, Stdio};

use crate::Result;

/// Starts `program` with `args`, detached: no inherited input or output, and it keeps running
/// when the caller exits or is killed.
///
/// Browsers kill a native-messaging host's whole process group (or, on Windows, its job) when
/// the extension disconnects; the app the host starts must survive that.
pub fn launch_detached(program: &Path, args: &[&str]) -> Result<()> {
    let mut command = Command::new(program);
    command
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    detach(&mut command);
    command.spawn()?;
    Ok(())
}

/// Puts the child in a process group of its own.
#[cfg(unix)]
fn detach(command: &mut Command) {
    use std::os::unix::process::CommandExt;
    command.process_group(0);
}

/// Starts the child outside the caller's job and console.
#[cfg(windows)]
fn detach(command: &mut Command) {
    use std::os::windows::process::CommandExt;
    const DETACHED_PROCESS: u32 = 0x0000_0008;
    const CREATE_NEW_PROCESS_GROUP: u32 = 0x0000_0200;
    const CREATE_BREAKAWAY_FROM_JOB: u32 = 0x0100_0000;
    command.creation_flags(DETACHED_PROCESS | CREATE_NEW_PROCESS_GROUP | CREATE_BREAKAWAY_FROM_JOB);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_missing_program_is_an_error() {
        assert!(launch_detached(Path::new("/no/such/velo-program"), &[]).is_err());
    }
}
