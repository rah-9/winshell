//! Per-user registry access (HKCU only; nothing here needs elevation).
//! * Windows app light/dark mode, for `theme = "auto_system"`.
//! * The autostart Run value (PLAN.md section 10).

use windows::Win32::Foundation::ERROR_SUCCESS;
use windows::Win32::System::Registry::*;
use windows::core::{HSTRING, w};

use crate::util::run_command;

const RUN_KEY: &str = "Software\\Microsoft\\Windows\\CurrentVersion\\Run";
const RUN_VALUE: &str = "WinShellTheme";

/// True if Windows apps are set to light mode.
pub fn apps_use_light_theme() -> bool {
    let mut data = 0u32;
    let mut size = 4u32;
    let rc = unsafe {
        RegGetValueW(
            HKEY_CURRENT_USER,
            w!("Software\\Microsoft\\Windows\\CurrentVersion\\Themes\\Personalize"),
            w!("AppsUseLightTheme"),
            RRF_RT_REG_DWORD,
            None,
            Some(&mut data as *mut _ as _),
            Some(&mut size),
        )
    };
    rc == ERROR_SUCCESS && data != 0
}

pub fn enable_autostart(exe: &str) -> Result<(), String> {
    let cmd = run_command(exe).ok_or("unsupported executable path")?;
    let wide: Vec<u16> = cmd.encode_utf16().chain(std::iter::once(0)).collect();
    let rc = unsafe {
        RegSetKeyValueW(
            HKEY_CURRENT_USER,
            &HSTRING::from(RUN_KEY),
            &HSTRING::from(RUN_VALUE),
            REG_SZ.0,
            Some(wide.as_ptr() as _),
            (wide.len() * 2) as u32,
        )
    };
    if rc == ERROR_SUCCESS { Ok(()) } else { Err(format!("RegSetKeyValueW failed ({})", rc.0)) }
}

pub fn disable_autostart() -> Result<(), String> {
    let rc = unsafe { RegDeleteKeyValueW(HKEY_CURRENT_USER, &HSTRING::from(RUN_KEY), &HSTRING::from(RUN_VALUE)) };
    // ERROR_FILE_NOT_FOUND = already absent.
    if rc == ERROR_SUCCESS || rc.0 == 2 { Ok(()) } else { Err(format!("RegDeleteKeyValueW failed ({})", rc.0)) }
}
