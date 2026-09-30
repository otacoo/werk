//! Child-process spawn helpers: Windows must never flash console windows.

use std::ffi::OsStr;
use std::process::Command;

/// `std::process::Command` with the console hidden on Windows.
pub fn command(program: impl AsRef<OsStr>) -> Command {
    let mut cmd = Command::new(program);
    hide_std(&mut cmd);
    cmd
}

#[cfg(windows)]
fn hide_std(cmd: &mut Command) {
    use std::os::windows::process::CommandExt;
    cmd.creation_flags(0x08000000);
}

#[cfg(not(windows))]
fn hide_std(_cmd: &mut Command) {}

/// `tokio::process::Command` with the console hidden on Windows.
#[cfg(windows)]
pub fn hide_tokio(cmd: &mut tokio::process::Command) {
    cmd.creation_flags(0x08000000);
}

#[cfg(not(windows))]
pub fn hide_tokio(_cmd: &mut tokio::process::Command) {}
