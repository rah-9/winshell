//! WinShellTheme: a user-mode Windows 11 shell overlay (top bar + dock).
//!
//! Milestone 1 provides the shared building blocks used by both the main
//! `winshell` binary and the throwaway `spike` binary:
//! logging, memory measurement, the shared D3D/D2D/DComp device, DWM
//! backdrop helpers, AppBar reservation windows and the emergency hotkey.
#![cfg(windows)]

pub mod appbar;
pub mod dwm;
pub mod hotkey;
pub mod instance;
pub mod log;
pub mod mem;
pub mod render;
pub mod win;
