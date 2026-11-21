//! Native taskbar autohide with exact restore (PLAN.md sections 10 and 13, M5).
//!
//! Order of operations, so a crash loop can never record "autohide" as the
//! user's original value:
//! 1. If `taskbar_state.toml` exists, the last run died: restore it first.
//! 2. Read the (now genuine) state and write it to the file.
//! 3. Switch to autohide.
//!
//! Restore puts back exactly the saved value and then deletes the file. It runs
//! on clean exit, from the panic hook, from the emergency hotkey, and (via step
//! 1) on the next launch after a kill.

use std::path::PathBuf;
use std::sync::Mutex;

use windows::Win32::UI::Shell::ABS_AUTOHIDE;

use crate::appbar::{set_taskbar_state, taskbar_state};
use crate::log;
use crate::taskbar_state::{self, SavedState};

static STATE_PATH: Mutex<Option<PathBuf>> = Mutex::new(None);

pub struct TaskbarGuard {
    path: PathBuf,
    saved: Option<SavedState>,
}

impl TaskbarGuard {
    /// Step 1 only: repair a leftover state file. Call once at startup, before
    /// anything else touches the taskbar, whether or not autohide is enabled.
    pub fn repair(path: PathBuf) -> Self {
        *STATE_PATH.lock().unwrap() = Some(path.clone());
        if let Some(saved) = taskbar_state::load(&path) {
            log!("taskbar: previous run did not exit cleanly; restoring saved state {}", saved.original_abm_state);
            set_taskbar_state(saved.original_abm_state);
            let _ = std::fs::remove_file(&path);
        }
        Self { path, saved: None }
    }

    /// Steps 2 and 3: save the genuine state, then enable autohide.
    pub fn enable_autohide(&mut self) {
        let original = taskbar_state();
        let saved = SavedState { original_abm_state: original };
        if let Err(e) = taskbar_state::save(&self.path, saved, &log::timestamp()) {
            log!("taskbar: could not write state file ({e}); leaving the taskbar unchanged");
            return;
        }
        self.saved = Some(saved);
        if original & ABS_AUTOHIDE != 0 {
            log!("taskbar: already autohide (state {original}); nothing to change");
            return;
        }
        set_taskbar_state(original | ABS_AUTOHIDE);
        let now = taskbar_state();
        if now & ABS_AUTOHIDE == 0 {
            log!("taskbar: Windows refused autohide (state {now}); continuing with the taskbar as it is");
        } else {
            log!("taskbar: autohide enabled (original state {original})");
        }
    }

    pub fn is_active(&self) -> bool {
        self.saved.is_some()
    }

    /// Put back exactly the saved value and delete the state file.
    pub fn restore(&mut self) {
        if let Some(saved) = self.saved.take() {
            set_taskbar_state(saved.original_abm_state);
            let _ = std::fs::remove_file(&self.path);
            log!("taskbar: restored original state {}", saved.original_abm_state);
        }
    }
}

/// Restore from the state file alone (panic hook / fatal paths, which cannot
/// reach the guard). Does nothing if no state file exists.
pub fn restore_from_file() {
    let path = match STATE_PATH.lock() {
        Ok(g) => g.clone(),
        Err(p) => p.into_inner().clone(),
    };
    if let Some(path) = path
        && let Some(saved) = taskbar_state::load(&path)
    {
        set_taskbar_state(saved.original_abm_state);
        let _ = std::fs::remove_file(&path);
    }
}
