//! Tiny append-only file logger. No background thread, no buffering beyond
//! one `write_all` per line, so a crash loses at most the current line.

use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::PathBuf;
use std::sync::Mutex;

use windows::Win32::System::SystemInformation::GetLocalTime;

static LOG_PATH: Mutex<Option<PathBuf>> = Mutex::new(None);
static ECHO: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// `%APPDATA%\WinShellTheme`, created on demand.
pub fn app_dir() -> PathBuf {
    let base = std::env::var_os("APPDATA").map(PathBuf::from).unwrap_or_else(|| PathBuf::from("."));
    let dir = base.join("WinShellTheme");
    let _ = fs::create_dir_all(&dir);
    dir
}

/// Route log lines to `path`. If `echo` is set, lines are also printed to stderr.
pub fn init(path: PathBuf, echo: bool) {
    *LOG_PATH.lock().unwrap() = Some(path);
    ECHO.store(echo, std::sync::atomic::Ordering::Relaxed);
}

pub fn timestamp() -> String {
    let t = unsafe { GetLocalTime() };
    format!(
        "{:04}-{:02}-{:02} {:02}:{:02}:{:02}.{:03}",
        t.wYear, t.wMonth, t.wDay, t.wHour, t.wMinute, t.wSecond, t.wMilliseconds
    )
}

pub fn write_line(msg: &str) {
    let line = format!("[{}] {}\n", timestamp(), msg);
    if ECHO.load(std::sync::atomic::Ordering::Relaxed) {
        eprint!("{line}");
    }
    let guard = match LOG_PATH.lock() {
        Ok(g) => g,
        Err(p) => p.into_inner(),
    };
    if let Some(path) = guard.as_ref()
        && let Ok(mut f) = OpenOptions::new().create(true).append(true).open(path)
    {
        let _ = f.write_all(line.as_bytes());
    }
}

#[macro_export]
macro_rules! log {
    ($($arg:tt)*) => { $crate::log::write_line(&format!($($arg)*)) };
}
