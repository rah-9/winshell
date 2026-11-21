//! Active window tracking with out-of-context WinEvent hooks (PLAN.md sections 6 and 7).
//!
//! * `EVENT_SYSTEM_FOREGROUND`: one global hook for the app's lifetime.
//! * `EVENT_OBJECT_NAMECHANGE`: scoped to the *foreground window's process*
//!   and re-installed whenever the foreground changes, so title changes in
//!   other processes never wake this thread.
//!
//! Callbacks only set a flag and post one coalesced message to the top bar;
//! all real work happens in the window procedure.

use std::sync::atomic::{AtomicBool, AtomicIsize, Ordering};

use windows::Win32::Foundation::{HWND, LPARAM, WPARAM};
use windows::Win32::System::Threading::GetCurrentProcessId;
use windows::Win32::UI::Accessibility::{HWINEVENTHOOK, SetWinEventHook, UnhookWinEvent};
use windows::Win32::UI::WindowsAndMessaging::*;

pub const WM_SHELL_FOREGROUND: u32 = WM_APP + 0x20;
pub const WM_SHELL_TITLE: u32 = WM_APP + 0x21;
/// The foreground window moved or resized (only while auto-hide needs it).
pub const WM_SHELL_FGMOVE: u32 = WM_APP + 0x2B;

static POST_TO: AtomicIsize = AtomicIsize::new(0);
static TRACKED: AtomicIsize = AtomicIsize::new(0);
static FOREGROUND_PENDING: AtomicBool = AtomicBool::new(false);
static TITLE_PENDING: AtomicBool = AtomicBool::new(false);
static MOVE_PENDING: AtomicBool = AtomicBool::new(false);

pub struct ForegroundTracker {
    foreground_hook: HWINEVENTHOOK,
    name_hook: HWINEVENTHOOK,
    name_pid: u32,
    /// EVENT_OBJECT_LOCATIONCHANGE, scoped to the foreground process, only while
    /// auto-hide is on (PLAN.md section 4): moves elsewhere never wake us.
    move_hook: HWINEVENTHOOK,
    pub track_moves: bool,
}

impl ForegroundTracker {
    /// Install the foreground hook. Notifications are posted to `post_to`.
    pub fn install(post_to: HWND) -> Self {
        POST_TO.store(post_to.0 as isize, Ordering::SeqCst);
        let foreground_hook = unsafe {
            SetWinEventHook(
                EVENT_SYSTEM_FOREGROUND,
                EVENT_SYSTEM_FOREGROUND,
                None,
                Some(on_event),
                0,
                0,
                WINEVENT_OUTOFCONTEXT | WINEVENT_SKIPOWNPROCESS,
            )
        };
        if foreground_hook.is_invalid() {
            crate::log!("WARNING: EVENT_SYSTEM_FOREGROUND hook failed; active window will not update");
        }
        Self {
            foreground_hook,
            name_hook: HWINEVENTHOOK::default(),
            name_pid: 0,
            move_hook: HWINEVENTHOOK::default(),
            track_moves: false,
        }
    }

    /// Clear the coalescing flag. Call at the start of the WM_SHELL_FOREGROUND handler.
    pub fn ack_foreground(&self) {
        FOREGROUND_PENDING.store(false, Ordering::SeqCst);
    }

    pub fn ack_move(&self) {
        MOVE_PENDING.store(false, Ordering::SeqCst);
    }

    /// Clear the coalescing flag. Call at the start of the WM_SHELL_TITLE handler.
    pub fn ack_title(&self) {
        TITLE_PENDING.store(false, Ordering::SeqCst);
    }

    /// Follow title changes of `hwnd` (re-scoping the name-change hook to its process).
    pub fn follow(&mut self, hwnd: HWND) {
        TRACKED.store(hwnd.0 as isize, Ordering::SeqCst);
        let mut pid = 0u32;
        unsafe {
            GetWindowThreadProcessId(hwnd, Some(&mut pid));
        }
        let moves_ok = !self.track_moves || !self.move_hook.is_invalid();
        if pid == self.name_pid && !self.name_hook.is_invalid() && moves_ok {
            return;
        }
        self.unhook_name();
        if pid == 0 {
            return;
        }
        if self.track_moves {
            self.move_hook = unsafe {
                SetWinEventHook(
                    EVENT_OBJECT_LOCATIONCHANGE,
                    EVENT_OBJECT_LOCATIONCHANGE,
                    None,
                    Some(on_event),
                    pid,
                    0,
                    WINEVENT_OUTOFCONTEXT,
                )
            };
        }
        self.name_hook = unsafe {
            SetWinEventHook(
                EVENT_OBJECT_NAMECHANGE,
                EVENT_OBJECT_NAMECHANGE,
                None,
                Some(on_event),
                pid,
                0,
                WINEVENT_OUTOFCONTEXT,
            )
        };
        self.name_pid = if self.name_hook.is_invalid() { 0 } else { pid };
    }

    fn unhook_name(&mut self) {
        if !self.move_hook.is_invalid() {
            unsafe {
                let _ = UnhookWinEvent(self.move_hook);
            }
            self.move_hook = HWINEVENTHOOK::default();
        }
        if !self.name_hook.is_invalid() {
            unsafe {
                let _ = UnhookWinEvent(self.name_hook);
            }
        }
        self.name_hook = HWINEVENTHOOK::default();
        self.name_pid = 0;
    }

    pub fn uninstall(&mut self) {
        self.unhook_name();
        if !self.foreground_hook.is_invalid() {
            unsafe {
                let _ = UnhookWinEvent(self.foreground_hook);
            }
            self.foreground_hook = HWINEVENTHOOK::default();
        }
        TRACKED.store(0, Ordering::SeqCst);
    }
}

impl Drop for ForegroundTracker {
    fn drop(&mut self) {
        self.uninstall();
    }
}

fn post(flag: &AtomicBool, msg: u32) {
    if flag.swap(true, Ordering::SeqCst) {
        return; // already queued; the handler reads the latest state
    }
    let target = POST_TO.load(Ordering::SeqCst);
    if target != 0 {
        unsafe {
            let _ = PostMessageW(Some(HWND(target as _)), msg, WPARAM(0), LPARAM(0));
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
    match event {
        EVENT_SYSTEM_FOREGROUND => post(&FOREGROUND_PENDING, WM_SHELL_FOREGROUND),
        EVENT_OBJECT_LOCATIONCHANGE
            if id_object == OBJID_WINDOW.0
                && id_child == CHILDID_SELF as i32
                && hwnd.0 as isize == TRACKED.load(Ordering::Relaxed) =>
        {
            post(&MOVE_PENDING, WM_SHELL_FGMOVE)
        }
        EVENT_OBJECT_NAMECHANGE
            if id_object == OBJID_WINDOW.0
                && id_child == CHILDID_SELF as i32
                && hwnd.0 as isize == TRACKED.load(Ordering::Relaxed) =>
        {
            post(&TITLE_PENDING, WM_SHELL_TITLE)
        }
        _ => {}
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    /// A normal application window: show its icon and title.
    App,
    /// The desktop (Progman / WorkerW).
    Desktop,
    /// Shell chrome or our own windows: keep showing the previous app.
    Ignore,
}

pub fn classify(hwnd: HWND) -> Kind {
    if hwnd.0.is_null() {
        return Kind::Ignore;
    }
    let mut pid = 0u32;
    unsafe {
        GetWindowThreadProcessId(hwnd, Some(&mut pid));
    }
    if pid == unsafe { GetCurrentProcessId() } {
        return Kind::Ignore;
    }
    if is_shell_host(pid) {
        return Kind::Ignore;
    }
    match class_name(hwnd).as_str() {
        "Progman" | "WorkerW" => Kind::Desktop,
        "Shell_TrayWnd"
        | "Shell_SecondaryTrayWnd"
        | "NotifyIconOverflowWindow"
        | "TopLevelWindowForOverflowXamlIsland"
        | "XamlExplorerHostIslandWindow"
        | "ForegroundStaging" => Kind::Ignore,
        _ => Kind::App,
    }
}

/// Start, Search, the notification centre, the touch keyboard and similar shell
/// surfaces are separate processes; the bar must not show them as "apps".
fn is_shell_host(pid: u32) -> bool {
    use windows::Win32::Foundation::CloseHandle;
    use windows::Win32::System::Threading::{
        OpenProcess, PROCESS_NAME_WIN32, PROCESS_QUERY_LIMITED_INFORMATION, QueryFullProcessImageNameW,
    };
    const HOSTS: [&str; 8] = [
        "searchhost.exe",
        "searchapp.exe",
        "startmenuexperiencehost.exe",
        "shellexperiencehost.exe",
        "shellhost.exe",
        "textinputhost.exe",
        "lockapp.exe",
        "searchui.exe",
    ];
    unsafe {
        let Ok(h) = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid) else { return false };
        let mut buf = [0u16; 512];
        let mut len = buf.len() as u32;
        let ok =
            QueryFullProcessImageNameW(h, PROCESS_NAME_WIN32, windows::core::PWSTR(buf.as_mut_ptr()), &mut len).is_ok();
        let _ = CloseHandle(h);
        if !ok {
            return false;
        }
        let path = String::from_utf16_lossy(&buf[..len as usize]).to_ascii_lowercase();
        let name = path.rsplit('\\').next().unwrap_or("");
        HOSTS.contains(&name)
    }
}

pub fn class_name(hwnd: HWND) -> String {
    let mut buf = [0u16; 256];
    let n = unsafe { GetClassNameW(hwnd, &mut buf) };
    String::from_utf16_lossy(&buf[..n.max(0) as usize])
}

/// Window title without sending WM_GETTEXT, so a hung app cannot block the UI thread.
pub fn title(hwnd: HWND) -> String {
    let mut buf = [0u16; 512];
    let n = unsafe { InternalGetWindowText(hwnd, &mut buf) };
    String::from_utf16_lossy(&buf[..n.max(0) as usize])
}
