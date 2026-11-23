//! Invisible AppBar reservation windows (PLAN.md sections 2 and 7).
//!
//! An AppBar always owns a full edge rectangle, so the visible bar/dock never
//! register themselves; a separate invisible window does. How that window is
//! made invisible *and* click-through is selectable, because the spike has to
//! find out which variant actually holds space and lets clicks pass:
//!
//! * `Layered` (default): `WS_EX_LAYERED | WS_EX_TRANSPARENT`, alpha 0. Documented
//!   to be click-through across processes.
//! * `Hidden`: the window is registered but never shown. No hit-testing at all.
//! * `Plan`: PLAN.md rev 3 as written: non-layered `WS_EX_TRANSPARENT` +
//!   `HTTRANSPARENT`. Note `HTTRANSPARENT` only forwards to windows of the
//!   *same thread*, so this may swallow clicks on the desktop; the spike checks.

use windows::Win32::Foundation::{HWND, LPARAM, RECT};
use windows::Win32::UI::Shell::*;
use windows::Win32::UI::WindowsAndMessaging::*;

use crate::log;
use crate::win;

/// Callback message Explorer uses for ABN_* notifications.
pub const WM_APPBAR_CALLBACK: u32 = WM_APP + 0x10;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Edge {
    Top,
    Bottom,
}

impl Edge {
    fn raw(self) -> u32 {
        match self {
            Edge::Top => ABE_TOP,
            Edge::Bottom => ABE_BOTTOM,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ReserveMode {
    Layered,
    Hidden,
    Plan,
}

impl ReserveMode {
    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "layered" => Some(Self::Layered),
            "hidden" => Some(Self::Hidden),
            "plan" => Some(Self::Plan),
            _ => None,
        }
    }
    pub fn name(self) -> &'static str {
        match self {
            Self::Layered => "layered",
            Self::Hidden => "hidden",
            Self::Plan => "plan",
        }
    }
    pub fn ex_style(self) -> WINDOW_EX_STYLE {
        let base = WS_EX_TOOLWINDOW | WS_EX_NOACTIVATE;
        match self {
            Self::Layered => base | WS_EX_LAYERED | WS_EX_TRANSPARENT,
            Self::Hidden => base,
            Self::Plan => base | WS_EX_TRANSPARENT | WS_EX_NOREDIRECTIONBITMAP,
        }
    }
    /// Make the freshly created window invisible in the chosen way and show it if needed.
    pub fn finish(self, hwnd: HWND) {
        unsafe {
            match self {
                Self::Layered => {
                    let _ = SetLayeredWindowAttributes(hwnd, windows::Win32::Foundation::COLORREF(0), 0, LWA_ALPHA);
                    let _ = ShowWindow(hwnd, SW_SHOWNOACTIVATE);
                }
                Self::Hidden => {}
                Self::Plan => {
                    let _ = ShowWindow(hwnd, SW_SHOWNOACTIVATE);
                }
            }
        }
        crate::dwm::exclude_from_peek(hwnd);
    }
}

fn data(hwnd: HWND) -> APPBARDATA {
    APPBARDATA { cbSize: size_of::<APPBARDATA>() as u32, hWnd: hwnd, ..Default::default() }
}

pub struct AppBar {
    pub hwnd: HWND,
    pub edge: Edge,
    pub registered: bool,
    pub rect: RECT,
}

impl AppBar {
    pub fn new(hwnd: HWND, edge: Edge) -> Self {
        Self { hwnd, edge, registered: false, rect: RECT::default() }
    }

    pub fn register(&mut self) -> bool {
        let mut abd = data(self.hwnd);
        abd.uCallbackMessage = WM_APPBAR_CALLBACK;
        self.registered = unsafe { SHAppBarMessage(ABM_NEW, &mut abd) } != 0;
        if !self.registered {
            log!("appbar {:?}: ABM_NEW failed; running without reservation", self.edge);
        }
        self.registered
    }

    /// Reserve `thickness` device pixels along this edge of `monitor`.
    /// Returns the rectangle Explorer granted (also stored in `self.rect`).
    pub fn set_pos(&mut self, monitor: RECT, thickness: i32) -> RECT {
        let mut abd = data(self.hwnd);
        abd.uEdge = self.edge.raw();
        abd.rc = monitor;
        let clamp = |rc: &mut RECT| match self.edge {
            Edge::Top => rc.bottom = rc.top + thickness,
            Edge::Bottom => rc.top = rc.bottom - thickness,
        };
        clamp(&mut abd.rc);
        if self.registered {
            unsafe {
                SHAppBarMessage(ABM_QUERYPOS, &mut abd);
            }
            clamp(&mut abd.rc);
            unsafe {
                SHAppBarMessage(ABM_SETPOS, &mut abd);
            }
        }
        self.rect = abd.rc;
        win::move_window(self.hwnd, abd.rc, false);
        abd.rc
    }

    /// Forward WM_ACTIVATE as the AppBar contract requires.
    pub fn on_activate(&self) {
        if self.registered {
            let mut abd = data(self.hwnd);
            unsafe {
                SHAppBarMessage(ABM_ACTIVATE, &mut abd);
            }
        }
    }

    /// Forward WM_WINDOWPOSCHANGED as the AppBar contract requires.
    pub fn on_window_pos_changed(&self) {
        if self.registered {
            let mut abd = data(self.hwnd);
            unsafe {
                SHAppBarMessage(ABM_WINDOWPOSCHANGED, &mut abd);
            }
        }
    }

    pub fn remove(&mut self) {
        if self.registered {
            let mut abd = data(self.hwnd);
            unsafe {
                SHAppBarMessage(ABM_REMOVE, &mut abd);
            }
            self.registered = false;
        }
    }
}

/// Remove an AppBar by window handle only. Used by the panic hook, which cannot
/// reach the `AppBar` structs.
pub fn remove_raw(hwnd: HWND) {
    let mut abd = data(hwnd);
    unsafe {
        SHAppBarMessage(ABM_REMOVE, &mut abd);
    }
}

#[derive(Clone, Copy, Debug)]
pub struct TaskbarInfo {
    pub edge: &'static str,
    pub rect: RECT,
    pub autohide: bool,
}

/// Native taskbar edge, rect and autohide state (`ABM_GETTASKBARPOS` / `ABM_GETSTATE`).
/// Read-only: M1 never changes the native taskbar.
pub fn taskbar_info() -> Option<TaskbarInfo> {
    let mut abd = data(HWND::default());
    unsafe {
        if SHAppBarMessage(ABM_GETTASKBARPOS, &mut abd) == 0 {
            return None;
        }
        let state = taskbar_state();
        let edge = match abd.uEdge {
            ABE_LEFT => "left",
            ABE_TOP => "top",
            ABE_RIGHT => "right",
            _ => "bottom",
        };
        Some(TaskbarInfo { edge, rect: abd.rc, autohide: state & ABS_AUTOHIDE != 0 })
    }
}

/// Human-readable ABN_* notification.
pub fn describe_notification(wparam: usize, lparam: LPARAM) -> String {
    match wparam as u32 {
        ABN_STATECHANGE => "ABN_STATECHANGE".into(),
        ABN_POSCHANGED => "ABN_POSCHANGED".into(),
        ABN_FULLSCREENAPP => {
            format!("ABN_FULLSCREENAPP (fullscreen {})", if lparam.0 != 0 { "opened" } else { "closed" })
        }
        ABN_WINDOWARRANGE => format!("ABN_WINDOWARRANGE ({})", if lparam.0 != 0 { "begin" } else { "end" }),
        other => format!("ABN_{other}"),
    }
}

/// Raw `ABM_GETSTATE` value of the native taskbar (`ABS_AUTOHIDE` | `ABS_ALWAYSONTOP`).
pub fn taskbar_state() -> u32 {
    let mut abd = data(unsafe { FindWindowW(windows::core::w!("Shell_TrayWnd"), None) }.unwrap_or_default());
    unsafe { SHAppBarMessage(ABM_GETSTATE, &mut abd) as u32 }
}

/// `ABM_SETSTATE` on the native taskbar. This is the ordinary user setting
/// (Settings > Personalization > Taskbar > Automatically hide); the taskbar
/// window itself is never hidden or restyled.
pub fn set_taskbar_state(state: u32) {
    let tray = unsafe { FindWindowW(windows::core::w!("Shell_TrayWnd"), None) }.unwrap_or_default();
    let mut abd = data(tray);
    // Only the two defined bits; the saved file is user-writable (security review F12).
    abd.lParam = LPARAM((state & (ABS_AUTOHIDE | ABS_ALWAYSONTOP)) as isize);
    unsafe {
        SHAppBarMessage(ABM_SETSTATE, &mut abd);
    }
}
