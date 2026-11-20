//! Emergency hotkey (PLAN.md section 7): Ctrl+Shift+Alt+Esc, falling back to
//! Ctrl+Shift+Alt+F12 if the first combination is taken.

use windows::Win32::Foundation::HWND;
use windows::Win32::UI::Input::KeyboardAndMouse::*;

pub const ID_EMERGENCY: i32 = 0x5745; // 'WE'

pub fn mods() -> HOT_KEY_MODIFIERS {
    MOD_CONTROL | MOD_SHIFT | MOD_ALT | MOD_NOREPEAT
}

/// Register the emergency hotkey on `hwnd`. Returns the combination that
/// worked, or `None` if both are taken.
pub fn register_emergency(hwnd: HWND) -> Option<&'static str> {
    unsafe {
        if RegisterHotKey(Some(hwnd), ID_EMERGENCY, mods(), VK_ESCAPE.0 as u32).is_ok() {
            return Some("Ctrl+Shift+Alt+Esc");
        }
        if RegisterHotKey(Some(hwnd), ID_EMERGENCY, mods(), VK_F12.0 as u32).is_ok() {
            return Some("Ctrl+Shift+Alt+F12");
        }
    }
    None
}

pub fn register(hwnd: HWND, id: i32, vk: VIRTUAL_KEY) -> bool {
    unsafe { RegisterHotKey(Some(hwnd), id, mods(), vk.0 as u32).is_ok() }
}

pub fn unregister(hwnd: HWND, id: i32) {
    unsafe {
        let _ = UnregisterHotKey(Some(hwnd), id);
    }
}
