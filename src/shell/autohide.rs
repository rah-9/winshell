//! Auto-hide for the top bar and dock (PLAN.md section 4, extended to the bar).
//!
//! With auto-hide on, no screen space is reserved: maximised apps get the whole
//! screen. The bar / dock hides while the foreground app is maximised, full
//! screen, or overlaps it, and comes back when the mouse touches that screen
//! edge (a 1 px invisible strip). Re-evaluated only on events: foreground change,
//! foreground move (hook scoped to that process), window events, and one-shot
//! timers armed when the mouse leaves a revealed bar or dock.

use windows::Win32::Foundation::{COLORREF, HWND, POINT, RECT};
use windows::Win32::Graphics::Dwm::{DWMWA_EXTENDED_FRAME_BOUNDS, DwmGetWindowAttribute};
use windows::Win32::UI::WindowsAndMessaging::*;
use windows::core::Result;

use winshell::apps;
use winshell::foreground::{self, Kind};
use winshell::{log, win};

use super::{App, TIMER_AUTOHIDE_BAR, TIMER_AUTOHIDE_DOCK, WINDOW_CLASS};

/// How long a revealed bar/dock stays after the mouse leaves it.
const HIDE_DELAY_MS: u32 = 500;
/// If the mouse never enters the revealed bar/dock, hide again after this.
const UNUSED_REVEAL_MS: u32 = 1500;

pub struct Edges {
    pub top: HWND,
    pub bottom: HWND,
    top_shown: bool,
    bottom_shown: bool,
}

impl Edges {
    pub fn create() -> Result<Self> {
        let ex = WS_EX_LAYERED | WS_EX_TOOLWINDOW | WS_EX_TOPMOST | WS_EX_NOACTIVATE;
        let zero = win::rect(0, 0, 1, 1);
        let top = win::create_popup(WINDOW_CLASS, windows::core::w!("WinShellTheme TopEdge"), ex, zero)?;
        let bottom = win::create_popup(WINDOW_CLASS, windows::core::w!("WinShellTheme BottomEdge"), ex, zero)?;
        for h in [top, bottom] {
            // Alpha 1 of 255: invisible, but still receives the mouse (alpha 0 would not).
            unsafe {
                let _ = SetLayeredWindowAttributes(h, COLORREF(0), 1, LWA_ALPHA);
            }
            winshell::dwm::exclude_from_peek(h);
        }
        Ok(Self { top, bottom, top_shown: false, bottom_shown: false })
    }

    pub fn is_edge(&self, h: HWND) -> Option<bool> {
        if h == self.top {
            Some(true)
        } else if h == self.bottom {
            Some(false)
        } else {
            None
        }
    }

    fn set(&mut self, top: bool, show: bool) {
        let (h, flag) = if top { (self.top, &mut self.top_shown) } else { (self.bottom, &mut self.bottom_shown) };
        if *flag != show {
            *flag = show;
            unsafe {
                let _ = ShowWindow(h, if show { SW_SHOWNOACTIVATE } else { SW_HIDE });
            }
        }
    }

    pub fn destroy(&self) {
        unsafe {
            let _ = DestroyWindow(self.top);
            let _ = DestroyWindow(self.bottom);
        }
    }
}

fn intersects(a: &RECT, b: &RECT) -> bool {
    a.left < b.right && b.left < a.right && a.top < b.bottom && b.top < a.bottom
}

fn contains(outer: &RECT, inner: &RECT) -> bool {
    outer.left <= inner.left && outer.top <= inner.top && outer.right >= inner.right && outer.bottom >= inner.bottom
}

/// Visible frame of a window (without the invisible resize borders).
fn frame(hwnd: HWND) -> Option<RECT> {
    let mut r = RECT::default();
    unsafe {
        if DwmGetWindowAttribute(hwnd, DWMWA_EXTENDED_FRAME_BOUNDS, &mut r as *mut _ as _, size_of::<RECT>() as u32)
            .is_ok()
        {
            return Some(r);
        }
        GetWindowRect(hwnd, &mut r).ok()?;
    }
    Some(r)
}

impl App {
    pub fn autohide_bar(&self) -> bool {
        self.config.top_bar.autohide
    }

    pub fn autohide_dock(&self) -> bool {
        self.config.dock.autohide
    }

    /// Register / remove the AppBar reservations to match the config.
    pub fn sync_reservations(&mut self) {
        let (want_top, want_bottom) = (!self.autohide_bar(), !self.autohide_dock());
        for (want, bar) in [(want_top, &mut self.top_reserve), (want_bottom, &mut self.bottom_reserve)] {
            if want && !bar.registered {
                bar.register();
            } else if !want && bar.registered {
                bar.remove();
            }
        }
        let any = self.autohide_bar() || self.autohide_dock();
        if self.tracker.track_moves != any {
            self.tracker.track_moves = any;
            let fg = self.active.hwnd;
            self.tracker.follow(HWND::default());
            if self.active.kind == Kind::App {
                self.tracker.follow(fg);
            }
        }
    }

    pub fn place_edges(&mut self) {
        let (left, right) = self.span;
        let m = self.monitor.monitor;
        win::move_window(self.edges.top, win::rect(left, m.top, right - left, 1), true);
        win::move_window(self.edges.bottom, win::rect(left, m.bottom - 1, right - left, 1), true);
    }

    /// (covers `r`, is full screen) for the current foreground app.
    fn foreground_covers(&self, r: &RECT) -> (bool, bool) {
        let fg = unsafe { GetForegroundWindow() };
        if fg.0.is_null() || App::foreground_is_ours(fg) || foreground::classify(fg) != Kind::App {
            return (false, false);
        }
        let root = apps::root_owner(fg);
        unsafe {
            if IsIconic(root).as_bool() || !IsWindowVisible(root).as_bool() {
                return (false, false);
            }
        }
        let Some(f) = frame(root) else { return (false, false) };
        let monitor = self.monitor.monitor;
        let style = unsafe { GetWindowLongPtrW(root, GWL_STYLE) } as u32;
        let fullscreen = contains(&f, &monitor) && style & WS_CAPTION.0 != WS_CAPTION.0;
        let maximized = unsafe { IsZoomed(root).as_bool() };
        (maximized || fullscreen || intersects(&f, r), fullscreen)
    }

    /// Show or hide bar, dock and edge strips for the current situation.
    pub fn update_visibility(&mut self) {
        let panel_open = self.panel.is_some();
        let (bar_covered, fs_a) = self.foreground_covers(&self.top_bar.rect);
        let (dock_covered, fs_b) = self.foreground_covers(&self.dock.rect);
        let fullscreen = self.fullscreen || fs_a || fs_b;

        let hide_bar = fullscreen || (self.autohide_bar() && bar_covered && !self.reveal_bar && !panel_open);
        let hide_dock = fullscreen || (self.autohide_dock() && dock_covered && !self.reveal_dock && !panel_open);

        if hide_bar == self.top_bar.visible {
            if hide_bar { self.top_bar.hide() } else { self.top_bar.show(false) }
        }
        if hide_dock == self.dock.visible {
            if hide_dock {
                self.dock.hide();
                self.tooltip.hide();
            } else {
                self.dock.show(false);
            }
        }
        // Edge strips only when something is auto-hidden, and never over a
        // full-screen app (games, video): there the edges do nothing.
        self.edges.set(true, hide_bar && self.autohide_bar() && !fullscreen);
        self.edges.set(false, hide_dock && self.autohide_dock() && !fullscreen);
    }

    /// The mouse touched a screen edge strip.
    pub fn on_edge_hover(&mut self, top: bool) {
        let timer = if top {
            self.reveal_bar = true;
            TIMER_AUTOHIDE_BAR
        } else {
            self.reveal_dock = true;
            TIMER_AUTOHIDE_DOCK
        };
        self.update_visibility();
        self.arm_autohide(timer, UNUSED_REVEAL_MS);
    }

    /// The mouse left the bar or dock: hide it again shortly if it is revealed.
    pub fn on_surface_left(&mut self, bar: bool) {
        if bar && self.reveal_bar {
            self.arm_autohide(TIMER_AUTOHIDE_BAR, HIDE_DELAY_MS);
        } else if !bar && self.reveal_dock {
            self.arm_autohide(TIMER_AUTOHIDE_DOCK, HIDE_DELAY_MS);
        }
    }

    fn arm_autohide(&self, id: usize, ms: u32) {
        unsafe {
            SetTimer(Some(self.top_bar.hwnd), id, ms, None);
        }
    }

    /// One-shot hide timer (already killed by the caller).
    pub fn on_autohide_timer(&mut self, id: usize) {
        let mut pt = POINT::default();
        unsafe {
            let _ = GetCursorPos(&mut pt);
        }
        let inside = |r: &RECT| pt.x >= r.left && pt.x < r.right && pt.y >= r.top && pt.y < r.bottom;
        let over_flyout = self.flyout.visible && inside(&self.flyout.rect);
        if id == TIMER_AUTOHIDE_BAR {
            if inside(&self.top_bar.rect) || over_flyout {
                return; // WM_MOUSELEAVE will arm the timer again
            }
            self.reveal_bar = false;
        } else {
            if inside(&self.dock.rect) || over_flyout {
                return;
            }
            self.reveal_dock = false;
        }
        self.update_visibility();
    }

    pub fn log_autohide(&self) {
        log!(
            "auto-hide: bar {} dock {} (reserved space: top {} bottom {})",
            self.autohide_bar(),
            self.autohide_dock(),
            self.top_reserve.registered,
            self.bottom_reserve.registered
        );
    }
}
