//! WinShellTheme: a user-mode Windows 11 shell overlay (top bar + dock).
//!
//! Shared building blocks for the `winshell` binary and the throwaway `spike`
//! binary. `dock_model`, `lru` and `util` hold pure logic so they can be
//! unit-tested on any host; everything else is Windows-only.

pub mod dock_model;
pub mod lru;
pub mod util;

#[cfg(windows)]
pub mod appbar;
#[cfg(windows)]
pub mod apps;
#[cfg(windows)]
pub mod assets;
#[cfg(windows)]
pub mod clock;
#[cfg(windows)]
pub mod dwm;
#[cfg(windows)]
pub mod foreground;
#[cfg(windows)]
pub mod hotkey;
#[cfg(windows)]
pub mod icon;
#[cfg(windows)]
pub mod instance;
#[cfg(windows)]
pub mod log;
#[cfg(windows)]
pub mod mem;
#[cfg(windows)]
pub mod render;
#[cfg(windows)]
pub mod win;
#[cfg(windows)]
pub mod winevents;
