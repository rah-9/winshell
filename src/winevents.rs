//! Window lifecycle WinEvent hooks for the dock (PLAN.md sections 6 and 7).
//!
//! Hooks `EVENT_OBJECT_DESTROY..EVENT_OBJECT_HIDE` (destroy, show, hide) and
//! `EVENT_OBJECT_CLOAKED..EVENT_OBJECT_UNCLOAKED`, out of context, skipping our
//! own process. The callback runs on the UI thread: it filters to whole
//! windows, queues the HWND and posts at most one coalesced message; the
//! window procedure then re-evaluates only the queued windows.

use std::cell::{Cell, RefCell};

use windows::Win32::Foundation::{HWND, LPARAM, WPARAM};
use windows::Win32::UI::Accessibility::{HWINEVENTHOOK, SetWinEventHook, UnhookWinEvent};
use windows::Win32::UI::WindowsAndMessaging::*;

pub const WM_SHELL_WINDOWS: u32 = WM_APP + 0x22;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Pending {
    pub hwnd: isize,
    pub destroyed: bool,
}

thread_local! {
    static QUEUE: RefCell<Vec<Pending>> = const { RefCell::new(Vec::new()) };
    static POSTED: Cell<bool> = const { Cell::new(false) };
    static TARGET: Cell<isize> = const { Cell::new(0) };
}

pub struct WindowEventHooks {
    hooks: Vec<HWINEVENTHOOK>,
}

impl WindowEventHooks {
    pub fn install(post_to: HWND) -> Self {
        TARGET.set(post_to.0 as isize);
        let flags = WINEVENT_OUTOFCONTEXT | WINEVENT_SKIPOWNPROCESS;
        let mut hooks = Vec::new();
        for (min, max) in [(EVENT_OBJECT_DESTROY, EVENT_OBJECT_HIDE), (EVENT_OBJECT_CLOAKED, EVENT_OBJECT_UNCLOAKED)] {
            let h = unsafe { SetWinEventHook(min, max, None, Some(on_event), 0, 0, flags) };
            if h.is_invalid() {
                crate::log!(
                    "WARNING: window event hook {min:#x}..{max:#x} failed; the dock will only refresh on rescans"
                );
            } else {
                hooks.push(h);
            }
        }
        Self { hooks }
    }

    /// Take the queued windows (deduplicated, last event wins) and re-arm posting.
    pub fn take_pending(&self) -> Vec<Pending> {
        POSTED.set(false);
        let mut q = QUEUE.with(|q| std::mem::take(&mut *q.borrow_mut()));
        let mut out: Vec<Pending> = Vec::with_capacity(q.len());
        while let Some(p) = q.pop() {
            if !out.iter().any(|o| o.hwnd == p.hwnd) {
                out.push(p);
            }
        }
        out
    }

    pub fn uninstall(&mut self) {
        for h in self.hooks.drain(..) {
            unsafe {
                let _ = UnhookWinEvent(h);
            }
        }
    }
}

impl Drop for WindowEventHooks {
    fn drop(&mut self) {
        self.uninstall();
    }
}

fn queue(hwnd: HWND, destroyed: bool) {
    QUEUE.with(|q| q.borrow_mut().push(Pending { hwnd: hwnd.0 as isize, destroyed }));
    if !POSTED.replace(true) {
        let target = TARGET.get();
        if target != 0 {
            unsafe {
                let _ = PostMessageW(Some(HWND(target as _)), WM_SHELL_WINDOWS, WPARAM(0), LPARAM(0));
            }
        }
    }
}

unsafe extern "system" fn on_event(
    _hook: HWINEVENTHOOK,
    event: u32,
    hwnd: HWND,
    id_object: i32,
    id_child: i32,
    _thread: u32,
    _time: u32,
) {
    if id_object != OBJID_WINDOW.0 || id_child != CHILDID_SELF as i32 || hwnd.0.is_null() {
        return;
    }
    if event == EVENT_OBJECT_DESTROY {
        queue(hwnd, true); // cannot inspect a destroyed window; the handler just drops it if tracked
        return;
    }
    unsafe {
        if GetAncestor(hwnd, GA_PARENT) == GetDesktopWindow() {
            queue(hwnd, false);
        } else if event == EVENT_OBJECT_SHOW {
            // A UWP CoreWindow attaching to its ApplicationFrameWindow makes the frame identifiable.
            let root = GetAncestor(hwnd, GA_ROOT);
            if !root.0.is_null() && crate::foreground::class_name(root) == "ApplicationFrameWindow" {
                queue(root, false);
            }
        }
    }
}
