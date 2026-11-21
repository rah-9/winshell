//! winshell: the Windows 11 shell overlay. See `shell` and PLAN.md.

#![cfg_attr(all(windows, not(debug_assertions)), windows_subsystem = "windows")]

#[cfg(windows)]
mod shell;

#[cfg(not(windows))]
fn main() {
    eprintln!("winshell only runs on Windows 11 (22H2 or newer).");
}

#[cfg(windows)]
fn main() {
    shell::main();
}
