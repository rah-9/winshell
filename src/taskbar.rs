//! Reversible native-taskbar suppression. Explorer and desktop icons stay running.
//! Original state and per-monitor visibility are flushed before the first change.
use crate::appbar::{set_taskbar_state, taskbar_state};
use crate::log;
use crate::taskbar_state::{self, SavedState, TaskbarWindow, WorkArea};
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use windows::Win32::Foundation::{HWND, LPARAM, RECT};
use windows::Win32::Graphics::Gdi::{EnumDisplayMonitors, GetMonitorInfoW, HDC, HMONITOR, MONITORINFO, MONITORINFOEXW};
use windows::Win32::UI::Shell::{ABS_ALWAYSONTOP, ABS_AUTOHIDE};
use windows::Win32::UI::WindowsAndMessaging::*;
use windows::core::BOOL;

static STATE_PATH: Mutex<Option<PathBuf>> = Mutex::new(None);

pub fn is_native_taskbar(hwnd: HWND) -> bool {
    if !matches!(crate::foreground::class_name(hwnd).as_str(), "Shell_TrayWnd" | "Shell_SecondaryTrayWnd") {
        return false;
    }
    // Validate the owner executable rather than depending on GetShellWindow:
    // Explorer can briefly detach its desktop HWND during a shell transition.
    use windows::Win32::Foundation::CloseHandle;
    use windows::Win32::System::Threading::{
        OpenProcess, PROCESS_NAME_WIN32, PROCESS_QUERY_LIMITED_INFORMATION, QueryFullProcessImageNameW,
    };
    let mut owner = 0;
    unsafe {
        GetWindowThreadProcessId(hwnd, Some(&mut owner));
        let Ok(process) = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, owner) else { return false };
        let mut path = [0u16; 1024];
        let mut len = path.len() as u32;
        let ok =
            QueryFullProcessImageNameW(process, PROCESS_NAME_WIN32, windows::core::PWSTR(path.as_mut_ptr()), &mut len)
                .is_ok();
        let _ = CloseHandle(process);
        if !ok {
            return false;
        }
        let path = String::from_utf16_lossy(&path[..len as usize]);
        let root = std::env::var("SystemRoot").unwrap_or_else(|_| "C:\\Windows".into());
        path.eq_ignore_ascii_case(&format!("{root}\\explorer.exe"))
    }
}

pub fn diagnostics() -> String {
    let bars = native_taskbars();
    let visible = bars.iter().filter(|h| unsafe { IsWindowVisible(**h) }.as_bool()).count();
    format!(
        "native taskbars: {} found, {visible} visible, state {}; work areas {:?}",
        bars.len(),
        taskbar_state(),
        work_areas().iter().map(|w| (w.original, w.monitor)).collect::<Vec<_>>()
    )
}

fn native_taskbars() -> Vec<HWND> {
    unsafe extern "system" fn each(h: HWND, lp: LPARAM) -> BOOL {
        if is_native_taskbar(h) {
            unsafe { &mut *(lp.0 as *mut Vec<HWND>) }.push(h);
        }
        BOOL(1)
    }
    let mut out = Vec::new();
    unsafe {
        let _ = EnumWindows(Some(each), LPARAM(&mut out as *mut _ as isize));
    }
    out
}

fn work_areas() -> Vec<WorkArea> {
    unsafe extern "system" fn each(h: HMONITOR, _: HDC, _: *mut RECT, lp: LPARAM) -> BOOL {
        let mut info = MONITORINFOEXW::default();
        info.monitorInfo.cbSize = size_of::<MONITORINFOEXW>() as u32;
        if unsafe { GetMonitorInfoW(h, &mut info as *mut _ as *mut MONITORINFO) }.as_bool() {
            let name_end = info.szDevice.iter().position(|c| *c == 0).unwrap_or(info.szDevice.len());
            let rect = |r: RECT| [r.left, r.top, r.right, r.bottom];
            unsafe { &mut *(lp.0 as *mut Vec<WorkArea>) }.push(WorkArea {
                device: String::from_utf16_lossy(&info.szDevice[..name_end]),
                original: rect(info.monitorInfo.rcWork),
                monitor: rect(info.monitorInfo.rcMonitor),
            });
        }
        BOOL(1)
    }
    let mut areas = Vec::new();
    unsafe {
        let _ = EnumDisplayMonitors(None, None, Some(each), LPARAM(&mut areas as *mut _ as isize));
    }
    areas
}

fn set_work_area(a: [i32; 4]) -> bool {
    let mut r = RECT { left: a[0], top: a[1], right: a[2], bottom: a[3] };
    unsafe {
        SystemParametersInfoW(SPI_SETWORKAREA, 0, Some(&mut r as *mut _ as _), SYSTEM_PARAMETERS_INFO_UPDATE_FLAGS(0))
            .is_ok()
    }
}

fn snapshot(hwnd: HWND) -> TaskbarWindow {
    let mut pid = 0;
    unsafe {
        GetWindowThreadProcessId(hwnd, Some(&mut pid));
    }
    TaskbarWindow { hwnd: hwnd.0 as isize, pid, visible: unsafe { IsWindowVisible(hwnd) }.as_bool() }
}

fn restore_saved(path: &Path, saved: &SavedState) -> bool {
    // Restore windows before the ordinary autohide setting, so Explorer can lay itself out.
    let mut ok = true;
    for w in &saved.windows {
        let h = HWND(w.hwnd as _);
        if !is_native_taskbar(h) {
            continue;
        }
        let now = snapshot(h);
        if now.pid != w.pid {
            continue;
        }
        unsafe {
            let _ = ShowWindow(h, if w.visible { SW_SHOWNOACTIVATE } else { SW_HIDE });
        }
        ok &= unsafe { IsWindowVisible(h) }.as_bool() == w.visible;
    }
    set_taskbar_state(saved.original_abm_state);
    let mask = ABS_AUTOHIDE | ABS_ALWAYSONTOP;
    ok &= taskbar_state() & mask == saved.original_abm_state & mask;
    // Restore only work areas still equal to the value this shell applied.
    // Preserve later layout changes by Windows or another desktop tool.
    for current in work_areas() {
        if let Some(target) = saved.work_areas.iter().find_map(|old| old.restore_target(&current)) {
            ok &= set_work_area(target);
        }
    }
    if ok {
        let _ = std::fs::remove_file(path);
    } else {
        log!("taskbar: restore incomplete; recovery file retained");
    }
    ok
}

pub struct TaskbarGuard {
    path: PathBuf,
    saved: Option<SavedState>,
    suppress: bool,
}

impl TaskbarGuard {
    pub fn repair(path: PathBuf) -> Self {
        *STATE_PATH.lock().unwrap_or_else(|e| e.into_inner()) = Some(path.clone());
        let saved = taskbar_state::load(&path).filter(|s| !restore_saved(&path, s));
        Self { path, saved, suppress: false }
    }

    fn protect(&mut self) -> bool {
        if self.saved.is_some() {
            return true;
        }
        // Never overwrite an unreadable recovery file with an already-modified state.
        if self.path.exists() {
            log!("taskbar: unreadable recovery state; native taskbar left unchanged");
            return false;
        }
        let state = SavedState { original_abm_state: taskbar_state(), windows: Vec::new(), work_areas: work_areas() };
        if let Err(e) = taskbar_state::save(&self.path, &state, &log::timestamp()) {
            log!("taskbar: cannot save recovery state ({e}); unchanged");
            return false;
        }
        self.saved = Some(state);
        true
    }

    pub fn configure(&mut self, autohide: bool, hidden: bool) {
        self.suppress = false;
        if !autohide && !hidden {
            self.restore();
            return;
        }
        if !self.protect() {
            return;
        }
        let original = self.saved.as_ref().unwrap().original_abm_state;
        // Autohide releases Explorer's work-area reservation even when its windows are hidden.
        set_taskbar_state(original | ABS_AUTOHIDE);
        self.suppress = hidden;
        if hidden {
            self.enforce();
        }
        log!("taskbar: {}", diagnostics());
    }

    pub fn observe_window(&mut self, hwnd: HWND) {
        if self.suppress && is_native_taskbar(hwnd) && unsafe { IsWindowVisible(hwnd) }.as_bool() {
            // Explorer re-showed a taskbar: reclaim its margin as well as its window.
            self.enforce();
        }
    }

    fn hide_window(&mut self, hwnd: HWND) {
        if !unsafe { IsWindowVisible(hwnd) }.as_bool() {
            return;
        }
        let window = snapshot(hwnd);
        let Some(saved) = self.saved.as_mut() else { return };
        if !saved.windows.iter().any(|w| w.hwnd == window.hwnd && w.pid == window.pid) {
            saved.windows.push(window);
            if let Err(e) = taskbar_state::save(&self.path, saved, &log::timestamp()) {
                saved.windows.pop();
                log!("taskbar: cannot protect visibility ({e}); window unchanged");
                return;
            }
        }
        unsafe {
            let _ = ShowWindow(hwnd, SW_HIDE);
        }
    }

    /// Called on Explorer restart, display changes and taskbar SHOW events, never on a timer.
    pub fn enforce(&mut self) {
        if self.suppress {
            let areas = work_areas();
            if let Some(saved) = self.saved.as_mut() {
                for area in &areas {
                    if !saved.work_areas.iter().any(|w| w.device == area.device && w.monitor == area.monitor) {
                        saved.work_areas.push(area.clone());
                    }
                }
                if let Err(e) = taskbar_state::save(&self.path, saved, &log::timestamp()) {
                    log!("taskbar: cannot protect work areas ({e}); unchanged");
                    return;
                }
            }
            for h in native_taskbars() {
                self.hide_window(h);
            }
            for area in areas {
                if area.original != area.monitor && !set_work_area(area.monitor) {
                    log!("taskbar: work area rejected for {}", area.device);
                }
            }
        }
    }

    pub fn is_suppressed(&self) -> bool {
        self.suppress
    }
    pub fn is_active(&self) -> bool {
        self.saved.is_some()
    }

    pub fn restore(&mut self) {
        self.suppress = false;
        if let Some(saved) = self.saved.take()
            && !restore_saved(&self.path, &saved)
        {
            self.saved = Some(saved);
        }
    }
}

pub fn restore_from_file() {
    let path = STATE_PATH.lock().unwrap_or_else(|e| e.into_inner()).clone();
    if let Some(path) = path
        && let Some(saved) = taskbar_state::load(&path)
    {
        restore_saved(&path, &saved);
    }
}
