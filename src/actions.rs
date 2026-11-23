//! Things the shell does to other windows and the system (PLAN.md sections 4 and 7).
//! All documented user-mode APIs; nothing here needs elevation.

use windows::Win32::Foundation::{HWND, LPARAM, WPARAM};
use windows::Win32::Storage::FileSystem::SearchPathW;
use windows::Win32::System::Threading::{AttachThreadInput, GetCurrentThreadId};
use windows::Win32::UI::Input::KeyboardAndMouse::*;
use windows::Win32::UI::Shell::ShellExecuteW;
use windows::Win32::UI::WindowsAndMessaging::*;
use windows::core::{BOOL, HSTRING, PCWSTR, w};

use crate::dock_model::PinSpec;

/// Press and release `keys` in order (e.g. [LWIN, N] = Win+N).
pub fn send_keys(keys: &[VIRTUAL_KEY]) {
    let mk = |vk: VIRTUAL_KEY, up: bool| INPUT {
        r#type: INPUT_KEYBOARD,
        Anonymous: INPUT_0 {
            ki: KEYBDINPUT {
                wVk: vk,
                dwFlags: if up { KEYEVENTF_KEYUP } else { KEYBD_EVENT_FLAGS(0) } | extended(vk),
                ..Default::default()
            },
        },
    };
    let mut inputs: Vec<INPUT> = keys.iter().map(|&k| mk(k, false)).collect();
    inputs.extend(keys.iter().rev().map(|&k| mk(k, true)));
    unsafe {
        SendInput(&inputs, size_of::<INPUT>() as i32);
    }
}

fn extended(vk: VIRTUAL_KEY) -> KEYBD_EVENT_FLAGS {
    match vk {
        VK_LWIN | VK_RWIN | VK_LEFT | VK_RIGHT | VK_UP | VK_DOWN => KEYEVENTF_EXTENDEDKEY,
        _ => KEYBD_EVENT_FLAGS(0),
    }
}

/// Start button: tap the Windows key (native Start menu, no shell GUID).
pub fn open_start() {
    send_keys(&[VK_LWIN]);
}

/// v1 stopgaps (PLAN.md section 4): native notification centre / quick settings.
pub fn open_notifications() {
    send_keys(&[VK_LWIN, VK_N]);
}

pub fn open_quick_settings() {
    send_keys(&[VK_LWIN, VK_A]);
}

/// Open a URI or path with the shell (`ms-settings:...`, exe paths, AppsFolder items).
pub fn shell_open(target: &str, params: Option<&str>) -> bool {
    let t = HSTRING::from(target);
    let p = params.map(HSTRING::from);
    let r = unsafe {
        ShellExecuteW(
            None,
            w!("open"),
            &t,
            p.as_ref().map(|p| PCWSTR(p.as_ptr())).unwrap_or(PCWSTR::null()),
            PCWSTR::null(),
            SW_SHOWNORMAL,
        )
    };
    let ok = r.0 as isize > 32;
    if !ok {
        crate::log!("ShellExecute failed for {target} ({})", r.0 as isize);
    }
    ok
}

/// Launch an app: packaged apps through `shell:AppsFolder\<AUMID>`, others by exe path.
pub fn launch(exe_path: &str, aumid: Option<&str>) -> bool {
    if let Some(id) = aumid
        && crate::util::is_safe_aumid(id)
        && shell_open(&format!("shell:AppsFolder\\{id}"), None)
    {
        return true;
    }
    !exe_path.is_empty() && shell_open(exe_path, None)
}

/// Bring `hwnd` to the front, restoring it if minimised. Uses the documented
/// `AttachThreadInput` fallback when the foreground lock refuses.
pub fn activate(hwnd: HWND) -> bool {
    unsafe {
        if IsIconic(hwnd).as_bool() {
            let _ = ShowWindowAsync(hwnd, SW_RESTORE);
        }
        if SetForegroundWindow(hwnd).as_bool() {
            return true;
        }
        let fg = GetForegroundWindow();
        let fg_thread = GetWindowThreadProcessId(fg, None);
        let me = GetCurrentThreadId();
        let attached = fg_thread != 0 && fg_thread != me && AttachThreadInput(me, fg_thread, true).as_bool();
        let _ = BringWindowToTop(hwnd);
        let ok = SetForegroundWindow(hwnd).as_bool();
        if attached {
            let _ = AttachThreadInput(me, fg_thread, false);
        }
        ok
    }
}

pub fn minimize(hwnd: HWND) {
    unsafe {
        let _ = ShowWindowAsync(hwnd, SW_MINIMIZE);
    }
}

pub fn toggle_maximize(hwnd: HWND) {
    unsafe {
        let cmd = if IsZoomed(hwnd).as_bool() { SW_RESTORE } else { SW_MAXIMIZE };
        let _ = ShowWindowAsync(hwnd, cmd);
    }
}

/// Native Snap: focus the window, then Win+Left / Win+Right.
pub fn snap(hwnd: HWND, left: bool) {
    if activate(hwnd) {
        send_keys(&[VK_LWIN, if left { VK_LEFT } else { VK_RIGHT }]);
    }
}

/// Polite close (the app may ask to save).
pub fn close(hwnd: HWND) {
    unsafe {
        let _ = PostMessageW(Some(hwnd), WM_CLOSE, WPARAM(0), LPARAM(0));
    }
}

/// "Quit App": politely close every visible top-level window of these processes,
/// including ones the dock does not show. Never terminates a process.
pub fn quit_processes(pids: &[u32]) {
    struct Ctx<'a> {
        pids: &'a [u32],
        hwnds: Vec<HWND>,
    }
    unsafe extern "system" fn each(hwnd: HWND, lp: LPARAM) -> BOOL {
        let ctx = unsafe { &mut *(lp.0 as *mut Ctx) };
        let mut pid = 0u32;
        unsafe {
            GetWindowThreadProcessId(hwnd, Some(&mut pid));
            if ctx.pids.contains(&pid) && IsWindowVisible(hwnd).as_bool() && GetWindow(hwnd, GW_OWNER).is_err() {
                ctx.hwnds.push(hwnd);
            }
        }
        BOOL(1)
    }
    let mut ctx = Ctx { pids, hwnds: Vec::new() };
    unsafe {
        let _ = EnumWindows(Some(each), LPARAM(&mut ctx as *mut _ as isize));
    }
    for h in ctx.hwnds {
        close(h);
    }
}

/// True if a pinned entry is an AUMID ("Publisher.App_hash!App") rather than a path.
pub fn looks_like_aumid(entry: &str) -> bool {
    entry.contains('!') && !entry.contains('\\') && !entry.contains('/')
}

/// Resolve a `pinned_apps` entry to the app id the running-window side computes.
pub fn resolve_pin(entry: &str) -> PinSpec {
    let entry_t = entry.trim();
    if looks_like_aumid(entry_t) {
        return PinSpec {
            app_id: entry_t.to_lowercase(),
            exe_path: String::new(),
            aumid: Some(entry_t.into()),
            entry: entry.into(),
        };
    }
    let expanded = expand_env(entry_t);
    let path = if std::path::Path::new(&expanded).is_absolute() {
        expanded
    } else {
        search_path(&expanded).unwrap_or(expanded)
    };
    PinSpec { app_id: path.to_lowercase(), exe_path: path, aumid: None, entry: entry.into() }
}

fn search_path(name: &str) -> Option<String> {
    let mut buf = [0u16; 1024];
    let n = unsafe { SearchPathW(PCWSTR::null(), &HSTRING::from(name), w!(".exe"), Some(&mut buf), None) };
    (n > 0 && (n as usize) < buf.len()).then(|| String::from_utf16_lossy(&buf[..n as usize]))
}

/// Expand `%VAR%` references using the process environment.
fn expand_env(s: &str) -> String {
    let mut out = String::new();
    let mut rest = s;
    while let Some(start) = rest.find('%') {
        out.push_str(&rest[..start]);
        let after = &rest[start + 1..];
        match after.find('%') {
            Some(end) => {
                let var = &after[..end];
                match std::env::var(var) {
                    Ok(v) => out.push_str(&v),
                    Err(_) => {
                        out.push('%');
                        out.push_str(var);
                        out.push('%');
                    }
                }
                rest = &after[end + 1..];
            }
            None => {
                out.push_str(&rest[start..]);
                rest = "";
            }
        }
    }
    out.push_str(rest);
    out
}

/// Native desktop toggle, without moving or changing desktop icons.
pub fn show_desktop() {
    send_keys(&[VK_LWIN, VK_D]);
}
