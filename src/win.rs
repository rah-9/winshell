//! Thin Win32 window helpers shared by the app and the spike.

use windows::Win32::Foundation::{HWND, POINT, RECT};
use windows::Win32::Graphics::Gdi::{
    GetMonitorInfoW, HMONITOR, MONITOR_DEFAULTTOPRIMARY, MONITORINFO, MonitorFromPoint,
};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::HiDpi::{
    DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2, GetDpiForMonitor, MDT_EFFECTIVE_DPI, SetProcessDpiAwarenessContext,
};
use windows::Win32::UI::WindowsAndMessaging::*;
use windows::core::{PCWSTR, Result};

pub type WndProc = unsafe extern "system" fn(
    HWND,
    u32,
    windows::Win32::Foundation::WPARAM,
    windows::Win32::Foundation::LPARAM,
) -> windows::Win32::Foundation::LRESULT;

/// Per-Monitor V2 DPI awareness. The release manifest (M9) declares the same;
/// calling it here keeps M1 builds correct without a resource compiler.
pub fn enable_per_monitor_dpi() {
    unsafe {
        let _ = SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2);
    }
}

pub fn register_class(name: PCWSTR, proc_: WndProc) -> Result<()> {
    unsafe {
        let hinstance = GetModuleHandleW(None)?;
        let wc = WNDCLASSEXW {
            cbSize: size_of::<WNDCLASSEXW>() as u32,
            lpfnWndProc: Some(proc_),
            hInstance: hinstance.into(),
            lpszClassName: name,
            hCursor: LoadCursorW(None, IDC_ARROW)?,
            ..Default::default()
        };
        if RegisterClassExW(&wc) == 0 {
            return Err(windows::core::Error::from_thread());
        }
    }
    Ok(())
}

/// Borderless popup window. Not shown; the caller decides how to show it.
pub fn create_popup(class: PCWSTR, title: PCWSTR, ex_style: WINDOW_EX_STYLE, r: RECT) -> Result<HWND> {
    unsafe {
        let hinstance = GetModuleHandleW(None)?;
        CreateWindowExW(
            ex_style,
            class,
            title,
            WS_POPUP,
            r.left,
            r.top,
            r.right - r.left,
            r.bottom - r.top,
            None,
            None,
            Some(hinstance.into()),
            None,
        )
    }
}

pub fn move_window(hwnd: HWND, r: RECT, topmost: bool) {
    unsafe {
        let _ = SetWindowPos(
            hwnd,
            Some(if topmost { HWND_TOPMOST } else { HWND::default() }),
            r.left,
            r.top,
            r.right - r.left,
            r.bottom - r.top,
            SWP_NOACTIVATE | if topmost { SET_WINDOW_POS_FLAGS(0) } else { SWP_NOZORDER },
        );
    }
}

#[derive(Clone, Copy, Debug)]
pub struct MonitorGeometry {
    pub handle: HMONITOR,
    pub monitor: RECT,
    pub work: RECT,
    pub dpi: u32,
}

impl MonitorGeometry {
    pub fn primary() -> Self {
        unsafe {
            let handle = MonitorFromPoint(POINT { x: 0, y: 0 }, MONITOR_DEFAULTTOPRIMARY);
            let mut mi = MONITORINFO { cbSize: size_of::<MONITORINFO>() as u32, ..Default::default() };
            let _ = GetMonitorInfoW(handle, &mut mi);
            let (mut dx, mut dy) = (96u32, 96u32);
            if GetDpiForMonitor(handle, MDT_EFFECTIVE_DPI, &mut dx, &mut dy).is_err() {
                dx = 96;
            }
            Self { handle, monitor: mi.rcMonitor, work: mi.rcWork, dpi: dx }
        }
    }

    pub fn scale(&self) -> f32 {
        self.dpi as f32 / 96.0
    }
}

/// Scale a 100%-baseline pixel value to device pixels with integer rounding (PLAN.md section 2).
pub fn px(v: f32, scale: f32) -> i32 {
    (v * scale).round() as i32
}

pub fn rect(x: i32, y: i32, w: i32, h: i32) -> RECT {
    RECT { left: x, top: y, right: x + w, bottom: y + h }
}

pub fn rect_str(r: &RECT) -> String {
    format!("({}, {}) {}x{}", r.left, r.top, r.right - r.left, r.bottom - r.top)
}

pub fn work_area() -> RECT {
    let mut r = RECT::default();
    unsafe {
        let _ = SystemParametersInfoW(
            SPI_GETWORKAREA,
            0,
            Some(&mut r as *mut _ as *mut _),
            SYSTEM_PARAMETERS_INFO_UPDATE_FLAGS(0),
        );
    }
    r
}

pub fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().collect()
}
