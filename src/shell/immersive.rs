//! Reversible, user-requested borderless full screen. No injection or keyboard
//! simulation: only the selected window's documented style / placement APIs.
//! The raw registry is also reachable from the panic and emergency exit paths.

use serde::Deserialize;
use std::sync::Mutex;
use std::time::{Duration, Instant};

use windows::Win32::Foundation::{GetLastError, HWND, SetLastError, WIN32_ERROR};
use windows::Win32::Graphics::Gdi::{GetMonitorInfoW, MONITOR_DEFAULTTONEAREST, MONITORINFO, MonitorFromWindow};
use windows::Win32::UI::WindowsAndMessaging::*;

use winshell::{actions, apps, foreground, log};

use super::App;

#[derive(Clone)]
struct SavedWindow {
    hwnd: isize,
    pid: u32,
    thread: u32,
    style: isize,
    immersive_style: isize,
    placement: WINDOWPLACEMENT,
}

static WINDOWS: Mutex<Vec<SavedWindow>> = Mutex::new(Vec::new());

#[derive(Deserialize)]
struct RecoveryFile {
    windows: Vec<RecoveryWindow>,
}

#[derive(Deserialize)]
struct RecoveryWindow {
    hwnd: isize,
    pid: u32,
    thread: u32,
    style: isize,
    immersive_style: isize,
    flags: u32,
    show: u32,
    min: [i32; 2],
    max: [i32; 2],
    rect: [i32; 4],
}

fn recovery_path() -> std::path::PathBuf {
    #[cfg(test)]
    {
        std::env::temp_dir().join(format!("winshell-immersive-test-{}.toml", std::process::id()))
    }
    #[cfg(not(test))]
    {
        winshell::log::app_dir().join("immersive_state.toml")
    }
}

/// Flush and replace the journal before changing a foreign window. A failed
/// write leaves that window untouched; custom app / taskbar state is separate.
fn persist(windows: &[SavedWindow]) -> std::io::Result<()> {
    use std::io::Write;
    let path = recovery_path();
    if windows.is_empty() {
        return match std::fs::remove_file(path) {
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
            result => result,
        };
    }
    let mut text = String::from("# Original windows before WinShellTheme immersive mode.\n");
    for w in windows {
        let p = w.placement;
        text.push_str(&format!("[[windows]]\nhwnd = {}\npid = {}\nthread = {}\nstyle = {}\nimmersive_style = {}\nflags = {}\nshow = {}\nmin = [{}, {}]\nmax = [{}, {}]\nrect = [{}, {}, {}, {}]\n",
            w.hwnd, w.pid, w.thread, w.style, w.immersive_style, p.flags.0, p.showCmd,
            p.ptMinPosition.x, p.ptMinPosition.y, p.ptMaxPosition.x, p.ptMaxPosition.y,
            p.rcNormalPosition.left, p.rcNormalPosition.top, p.rcNormalPosition.right, p.rcNormalPosition.bottom));
    }
    let tmp = path.with_extension("toml.tmp");
    let mut file = std::fs::File::create(&tmp)?;
    file.write_all(text.as_bytes())?;
    file.sync_all()?;
    drop(file);
    std::fs::rename(tmp, path)
}

pub fn repair() {
    let path = recovery_path();
    let Ok(text) = std::fs::read_to_string(&path) else { return };
    let Ok(saved) = toml::from_str::<RecoveryFile>(&text) else {
        log!("immersive: invalid recovery journal; kept at {}", path.display());
        return;
    };
    let mut failed = Vec::new();
    for w in saved.windows {
        use windows::Win32::Foundation::{POINT, RECT};
        let saved = SavedWindow {
            hwnd: w.hwnd,
            pid: w.pid,
            thread: w.thread,
            style: w.style,
            immersive_style: w.immersive_style,
            placement: WINDOWPLACEMENT {
                length: size_of::<WINDOWPLACEMENT>() as u32,
                flags: WINDOWPLACEMENT_FLAGS(w.flags),
                showCmd: w.show,
                ptMinPosition: POINT { x: w.min[0], y: w.min[1] },
                ptMaxPosition: POINT { x: w.max[0], y: w.max[1] },
                rcNormalPosition: RECT { left: w.rect[0], top: w.rect[1], right: w.rect[2], bottom: w.rect[3] },
            },
        };
        if !restore_saved(&saved) {
            failed.push(saved);
        }
    }
    let _ = persist(&failed);
    WINDOWS.lock().unwrap_or_else(|e| e.into_inner()).extend(failed);
}

fn same_window(w: &SavedWindow) -> bool {
    let mut pid = 0;
    let thread = unsafe { GetWindowThreadProcessId(HWND(w.hwnd as _), Some(&mut pid)) };
    pid == w.pid && thread == w.thread && thread != 0
}

pub fn is_managed(hwnd: HWND) -> bool {
    let root = apps::root_owner(hwnd);
    WINDOWS.lock().unwrap_or_else(|e| e.into_inner()).iter().any(|w| w.hwnd == root.0 as isize && same_window(w))
}

fn set_style(hwnd: HWND, style: isize) -> bool {
    unsafe {
        SetLastError(WIN32_ERROR(0));
        let old = SetWindowLongPtrW(hwnd, GWL_STYLE, style);
        old != 0 || GetLastError().0 == 0
    }
}

fn fill_monitor(hwnd: HWND) -> bool {
    unsafe {
        let monitor = MonitorFromWindow(hwnd, MONITOR_DEFAULTTONEAREST);
        let mut info = MONITORINFO { cbSize: size_of::<MONITORINFO>() as u32, ..Default::default() };
        if !GetMonitorInfoW(monitor, &mut info).as_bool() {
            return false;
        }
        let r = info.rcMonitor;
        SetWindowPos(
            hwnd,
            None,
            r.left,
            r.top,
            r.right - r.left,
            r.bottom - r.top,
            SWP_NOZORDER | SWP_NOACTIVATE | SWP_FRAMECHANGED,
        )
        .is_ok()
    }
}

pub fn enter(hwnd: HWND) -> bool {
    let hwnd = apps::root_owner(hwnd);
    if is_managed(hwnd) {
        return true;
    }
    // Fixed-size dialogs, elevated processes and shell surfaces keep their own behavior.
    let Some(entry) = apps::window_entry(hwnd) else { return false };
    if entry.is_elevated {
        return false;
    }
    enter_window(hwnd)
}

fn enter_window(hwnd: HWND) -> bool {
    unsafe {
        let style = GetWindowLongPtrW(hwnd, GWL_STYLE);
        if style as u32 & WS_THICKFRAME.0 == 0 || style as u32 & WS_MAXIMIZEBOX.0 == 0 {
            return false;
        }
        let mut placement = WINDOWPLACEMENT { length: size_of::<WINDOWPLACEMENT>() as u32, ..Default::default() };
        if GetWindowPlacement(hwnd, &mut placement).is_err() {
            return false;
        }
        let mut pid = 0;
        let thread = GetWindowThreadProcessId(hwnd, Some(&mut pid));
        let immersive_style = ((style as u32 & !WS_OVERLAPPEDWINDOW.0 & !WS_MAXIMIZE.0) | WS_POPUP.0) as isize;
        let mut windows = WINDOWS.lock().unwrap_or_else(|e| e.into_inner());
        windows.retain(same_window);
        windows.push(SavedWindow { hwnd: hwnd.0 as isize, pid, thread, style, immersive_style, placement });
        if let Err(e) = persist(&windows) {
            windows.pop();
            log!("immersive: cannot save recovery journal ({e}); window unchanged");
            return false;
        }
        drop(windows);
        if !set_style(hwnd, immersive_style) || !fill_monitor(hwnd) {
            restore(hwnd);
            return false;
        }
    }
    log!("immersive: entered for window {}", hwnd.0 as isize);
    true
}

fn restore_saved(w: &SavedWindow) -> bool {
    if !same_window(w) {
        return true;
    }
    let hwnd = HWND(w.hwnd as _);
    // Do not overwrite a new style the application deliberately chose itself.
    let current = unsafe { GetWindowLongPtrW(hwnd, GWL_STYLE) } as u32;
    let transient = WS_VISIBLE.0 | WS_MINIMIZE.0 | WS_MAXIMIZE.0;
    if current & !transient != w.immersive_style as u32 & !transient
        && current & !transient != w.style as u32 & !transient
    {
        return true;
    }
    let style = (w.style as u32 & !WS_VISIBLE.0) | (current & WS_VISIBLE.0);
    if !set_style(hwnd, style as isize) {
        return false;
    }
    unsafe {
        let placed = SetWindowPlacement(hwnd, &w.placement).is_ok();
        let framed = SetWindowPos(
            hwnd,
            None,
            0,
            0,
            0,
            0,
            SWP_NOMOVE | SWP_NOSIZE | SWP_NOZORDER | SWP_NOACTIVATE | SWP_FRAMECHANGED,
        )
        .is_ok();
        placed && framed
    }
}

pub fn restore(hwnd: HWND) {
    let root = apps::root_owner(hwnd);
    let saved = {
        let mut windows = WINDOWS.lock().unwrap_or_else(|e| e.into_inner());
        windows.iter().position(|w| w.hwnd == root.0 as isize).map(|i| windows.remove(i))
    };
    if let Some(w) = saved
        && !restore_saved(&w)
    {
        WINDOWS.lock().unwrap_or_else(|e| e.into_inner()).push(w);
    }
    let windows = WINDOWS.lock().unwrap_or_else(|e| e.into_inner());
    let _ = persist(&windows);
}

pub fn restore_all() {
    let windows = std::mem::take(&mut *WINDOWS.lock().unwrap_or_else(|e| e.into_inner()));
    let failed: Vec<_> = windows.into_iter().filter(|w| !restore_saved(w)).collect();
    let _ = persist(&failed);
    WINDOWS.lock().unwrap_or_else(|e| e.into_inner()).extend(failed);
}

pub fn reflow() {
    let handles: Vec<isize> = {
        let mut windows = WINDOWS.lock().unwrap_or_else(|e| e.into_inner());
        windows.retain(same_window);
        windows.iter().map(|w| w.hwnd).collect()
    };
    for h in handles {
        let hwnd = HWND(h as _);
        if !unsafe { IsIconic(hwnd) }.as_bool() {
            fill_monitor(hwnd);
        }
    }
}

impl App {
    pub fn present_window(&mut self, hwnd: HWND, elevated: bool) {
        if !actions::activate(hwnd) {
            self.show_message_tooltip(if elevated {
                "Use Alt+Tab to switch to this administrator window"
            } else {
                "Windows didn't allow switching to that window"
            });
            return;
        }
        if self.config.dock.immersive && !enter(hwnd) {
            self.show_message_tooltip("This app keeps its own window controls");
        }
        self.update_visibility();
    }

    pub fn launch_app(&mut self, exe: &str, aumid: Option<&str>, id: &str) -> bool {
        if self.config.dock.immersive {
            self.pending_launches.retain(|(_, _, t)| t.elapsed() < Duration::from_secs(15));
            self.pending_launches.push((id.to_string(), exe.to_lowercase(), Instant::now()));
        }
        let ok = actions::launch(exe, aumid);
        if !ok {
            self.pending_launches.retain(|(app, _, _)| app != id);
        }
        ok
    }

    /// Only a dock launch is eligible, and only its foreground window. A slow
    /// launch expires after 15 seconds; OS window / focus events drive this.
    pub fn check_pending_launch(&mut self, hwnd: HWND) {
        self.pending_launches.retain(|(_, _, t)| t.elapsed() < Duration::from_secs(15));
        if self.pending_launches.is_empty() || !self.config.dock.immersive {
            return;
        }
        let Some(w) = apps::window_entry(apps::root_owner(hwnd)) else { return };
        if let Some(i) = self
            .pending_launches
            .iter()
            .position(|(id, exe, _)| *id == w.app_id || (!exe.is_empty() && exe.eq_ignore_ascii_case(&w.exe_path)))
        {
            self.pending_launches.remove(i);
            enter(HWND(w.hwnd as _));
        }
    }

    pub fn toggle_immersive(&mut self) {
        let fg = unsafe { GetForegroundWindow() };
        let target = if Self::foreground_is_ours(fg) { self.active.hwnd } else { apps::root_owner(fg) };
        if foreground::classify(target) != foreground::Kind::App {
            return;
        }
        if is_managed(target) {
            restore(target);
        } else {
            enter(target);
        }
        self.update_visibility();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use windows::core::w;

    // Exercise actual Win32 styles and placement, including startup recovery.
    // Only windows created by this test are changed; no existing apps are touched.
    #[test]
    fn immersive_restores_style_geometry_and_recovers_after_lost_state() {
        unsafe {
            let hwnd = CreateWindowExW(
                WINDOW_EX_STYLE(0),
                w!("STATIC"),
                w!("WinShellTheme test fixture"),
                WS_OVERLAPPEDWINDOW,
                180,
                140,
                640,
                420,
                None,
                None,
                None,
                None,
            )
            .unwrap();
            let original_style = GetWindowLongPtrW(hwnd, GWL_STYLE);
            let mut original = WINDOWPLACEMENT { length: size_of::<WINDOWPLACEMENT>() as u32, ..Default::default() };
            GetWindowPlacement(hwnd, &mut original).unwrap();
            assert!(enter_window(hwnd));
            assert_eq!(GetWindowLongPtrW(hwnd, GWL_STYLE) as u32 & WS_CAPTION.0, 0);
            assert!(is_managed(hwnd));
            let mut actual = windows::Win32::Foundation::RECT::default();
            GetWindowRect(hwnd, &mut actual).unwrap();
            let mut monitor = MONITORINFO { cbSize: size_of::<MONITORINFO>() as u32, ..Default::default() };
            assert!(GetMonitorInfoW(MonitorFromWindow(hwnd, MONITOR_DEFAULTTONEAREST), &mut monitor).as_bool());
            assert_eq!(actual, monitor.rcMonitor);
            // Simulate a forced exit: only the durable journal survives.
            WINDOWS.lock().unwrap().clear();
            repair();
            assert!(!is_managed(hwnd));
            assert_eq!(
                GetWindowLongPtrW(hwnd, GWL_STYLE) as u32 & !WS_VISIBLE.0,
                original_style as u32 & !WS_VISIBLE.0
            );
            let mut restored = WINDOWPLACEMENT { length: size_of::<WINDOWPLACEMENT>() as u32, ..Default::default() };
            GetWindowPlacement(hwnd, &mut restored).unwrap();
            assert_eq!(restored.rcNormalPosition, original.rcNormalPosition);
            assert!(!recovery_path().exists());
            assert!(enter_window(hwnd));
            restore_all();
            assert!(!is_managed(hwnd));
            DestroyWindow(hwnd).unwrap();
        }
    }
}
