//! A shell-owned visible window: popup + DWM backdrop + DComp surface.
//! Used for the top bar, dock, tooltip and flyout.

use windows::Win32::Foundation::{HWND, RECT};
use windows::Win32::UI::WindowsAndMessaging::*;
use windows::core::{PCWSTR, Result};

use winshell::dwm::{self, DwmStyle};
use winshell::render::{Gfx, WindowSurface};
use winshell::{log, win};

pub struct Popup {
    pub hwnd: HWND,
    pub surface: WindowSurface,
    pub rect: RECT,
    pub visible: bool,
    /// TrackMouseEvent(TME_LEAVE) armed.
    pub tracking_leave: bool,
}

impl Popup {
    pub fn create(gfx: &Gfx, class: PCWSTR, title: PCWSTR, ex: WINDOW_EX_STYLE, style: DwmStyle) -> Result<Self> {
        let zero = win::rect(0, 0, 1, 1);
        let hwnd = win::create_popup(class, title, ex, zero)?;
        let rep = dwm::apply(hwnd, style);
        if !rep.all_ok() {
            log!("dwm attributes partly rejected: {}", rep.describe());
        }
        let surface = WindowSurface::new(gfx, hwnd)?;
        Ok(Self { hwnd, surface, rect: zero, visible: false, tracking_leave: false })
    }

    /// Move/resize the window and its surface (no redraw).
    pub fn place(&mut self, gfx: &Gfx, r: RECT) {
        if r != self.rect {
            self.rect = r;
            win::move_window(self.hwnd, r, true);
        }
        if let Err(e) = self.surface.resize(gfx, (r.right - r.left).max(1) as u32, (r.bottom - r.top).max(1) as u32) {
            log!("surface resize failed: {e:?}");
        }
    }

    pub fn show(&mut self, activate: bool) {
        unsafe {
            if activate {
                let _ = ShowWindow(self.hwnd, SW_SHOW);
                let _ = SetForegroundWindow(self.hwnd);
            } else {
                let _ = ShowWindow(self.hwnd, SW_SHOWNOACTIVATE);
            }
        }
        self.visible = true;
    }

    pub fn hide(&mut self) {
        if self.visible {
            unsafe {
                let _ = ShowWindow(self.hwnd, SW_HIDE);
            }
            self.visible = false;
        }
        self.tracking_leave = false;
    }

    pub fn width(&self) -> f32 {
        self.surface.width as f32
    }

    pub fn height(&self) -> f32 {
        self.surface.height as f32
    }

    pub fn destroy(&self) {
        unsafe {
            let _ = DestroyWindow(self.hwnd);
        }
    }
}
