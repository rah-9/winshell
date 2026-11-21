//! `taskbar_state.toml` (PLAN.md section 10): the native taskbar's original
//! `ABM_GETSTATE` value, written *before* any change and deleted after a
//! successful restore. Its presence at startup means the last run did not exit
//! cleanly. Pure file format; the Win32 side lives in `appbar`.

use serde::Deserialize;
use std::path::Path;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SavedState {
    pub original_abm_state: u32,
}

#[derive(Deserialize)]
struct Raw {
    original_abm_state: u32,
}

pub fn format(state: SavedState, saved_at: &str) -> String {
    format!(
        "# WinShellTheme: the native taskbar's state before this app changed it.\n\
         # Written at startup BEFORE any change; deleted after a successful restore.\n\
         original_abm_state = {}     # ABS_AUTOHIDE = 1, ABS_ALWAYSONTOP = 2\n\
         saved_at = \"{}\"\n",
        state.original_abm_state, saved_at
    )
}

pub fn parse(text: &str) -> Option<SavedState> {
    toml::from_str::<Raw>(text).ok().map(|r| SavedState { original_abm_state: r.original_abm_state })
}

pub fn load(path: &Path) -> Option<SavedState> {
    std::fs::read_to_string(path).ok().and_then(|t| parse(&t))
}

pub fn save(path: &Path, state: SavedState, saved_at: &str) -> std::io::Result<()> {
    std::fs::write(path, format(state, saved_at))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trip() {
        let s = SavedState { original_abm_state: 2 };
        assert_eq!(parse(&format(s, "2026-11-20 14:44:46")), Some(s));
    }

    #[test]
    fn garbage_is_none() {
        assert_eq!(parse("original_abm_state = \"x\""), None);
        assert_eq!(parse(""), None);
    }
}
