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

use super::theme::{BAR_HEIGHT, BAR_MARGIN};
use super::{App, TIMER_AUTOHIDE_BAR, TIMER_AUTOHIDE_DOCK, WINDOW_CLASS};

/// How long a revealed bar/dock stays after the mouse leaves it.
const HIDE_DELAY_MS: u32 = 500;
/// If the mouse never enters the revealed bar/dock, hide again after this.
const UNUSED_REVEAL_MS: u32 = 1500;

pub struct Edges {
    pub top: HWND,
    pub left: HWND,
    top_shown: bool,
    left_shown: bool,
}

impl Edges {
    pub fn create() -> Result<Self> {
        let ex = WS_EX_LAYERED | WS_EX_TOOLWINDOW | WS_EX_TOPMOST | WS_EX_NOACTIVATE;
        let zero = win::rect(0, 0, 1, 1);
        let top = win::create_popup(WINDOW_CLASS, windows::core::w!("WinShellTheme TopEdge"), ex, zero)?;
        let left = win::create_popup(WINDOW_CLASS, windows::core::w!("WinShellTheme LeftEdge"), ex, zero)?;
        for h in [top, left] {
            // Alpha 1 of 255: invisible, but still receives the mouse (alpha 0 would not).
            unsafe {
                let _ = SetLayeredWindowAttributes(h, COLORREF(0), 1, LWA_ALPHA);
            }
            winshell::dwm::exclude_from_peek(h);
        }
        Ok(Self { top, left, top_shown: false, left_shown: false })
    }

    pub fn is_edge(&self, h: HWND) -> Option<bool> {
        if h == self.top {
            Some(true)
        } else if h == self.left {
            Some(false)
        } else {
            None
        }
    }

    fn set(&mut self, top: bool, show: bool) {
        let (h, flag) = if top { (self.top, &mut self.top_shown) } else { (self.left, &mut self.left_shown) };
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
            let _ = DestroyWindow(self.left);
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

fn hidden(
    preview: bool,
    fullscreen: bool,
    desktop_clear: bool,
    covered: bool,
    auto: bool,
    revealed: bool,
    panel: bool,
) -> bool {
    !preview && (fullscreen || ((desktop_clear || (auto && covered)) && !revealed && !panel))
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
        // Visibility is independent from desktop reservations. Even a permanently
        // visible rail overlays the desktop instead of shrinking application windows.
        self.top_reserve.remove();
        self.bottom_reserve.remove();
        // Without a registered AppBar, ABN_FULLSCREENAPP(false) can never arrive.
        if !self.top_reserve.registered && !self.bottom_reserve.registered {
            self.fullscreen = false;
        }
        let any = true; // also track immersive geometry / external fullscreen transitions
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
        // Leave the top-right corner free: a maximised window's close button lives there.
        let corner = win::px(160.0, self.scale());
        win::move_window(self.edges.top, win::rect(left, m.top, (right - left - corner).max(1), 1), true);
        win::move_window(
            self.edges.left,
            win::rect(
                m.left,
                m.top + win::px(BAR_HEIGHT + BAR_MARGIN * 2.0, self.scale()),
                1,
                (m.bottom - m.top - win::px(BAR_HEIGHT + BAR_MARGIN * 2.0, self.scale())).max(1),
            ),
            true,
        );
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
            // Bar and dock live on the primary monitor: apps on other screens don't count.
            use windows::Win32::Graphics::Gdi::{MONITOR_DEFAULTTONEAREST, MonitorFromWindow};
            if MonitorFromWindow(root, MONITOR_DEFAULTTONEAREST) != self.monitor.handle {
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
        // Detected full screen (from the window itself) disables the edge strips; the
        // AppBar flag only hides, it never removes the way back (review: stuck hidden).
        let detected_fs = fs_a || fs_b;
        let managed = super::immersive::is_managed(unsafe { GetForegroundWindow() });
        let fullscreen = (self.fullscreen || detected_fs) && !managed;
        let fg = unsafe { GetForegroundWindow() };
        let desktop = foreground::classify(fg) == Kind::Desktop
            || (self.active.kind == Kind::Desktop && (fg.0.is_null() || App::foreground_is_ours(fg)));
        let clear_desktop = self.config.general.desktop_autohide && desktop;

        let hide_bar = hidden(
            self.opts.preview,
            fullscreen,
            clear_desktop,
            bar_covered,
            self.autohide_bar(),
            self.reveal_bar,
            panel_open,
        );
        let hide_dock = hidden(
            self.opts.preview,
            fullscreen,
            clear_desktop,
            dock_covered,
            self.autohide_dock(),
            self.reveal_dock,
            panel_open,
        );

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
        self.edges.set(true, hide_bar && (!detected_fs || managed));
        self.edges.set(false, hide_dock && (!detected_fs || managed));
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

#[cfg(test)]
mod tests {
    use super::hidden;
    #[test]
    fn desktop_stays_clear_but_edge_and_panel_reveal_work() {
        assert!(hidden(false, false, true, false, false, false, false));
        assert!(!hidden(false, false, true, false, true, true, false));
        assert!(!hidden(false, false, true, false, true, false, true));
        assert!(!hidden(false, false, false, false, true, false, false));
        assert!(hidden(false, false, false, true, true, false, false));
        assert!(hidden(false, true, false, true, true, true, true));
        assert!(!hidden(true, true, true, true, true, false, false));
    }
}
