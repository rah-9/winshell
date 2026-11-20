//! Which top-level windows belong in the dock, and which app each belongs to
//! (PLAN.md section 7: initial window scan, packaged app detection, cloaking).

use windows::Win32::Foundation::PROPERTYKEY;
use windows::Win32::Foundation::{CloseHandle, HANDLE, HWND, LPARAM};
use windows::Win32::Graphics::Dwm::{DWMWA_CLOAKED, DwmGetWindowAttribute};
use windows::Win32::Security::{GetTokenInformation, TOKEN_ELEVATION, TOKEN_QUERY, TokenElevation};
use windows::Win32::Storage::Packaging::Appx::GetApplicationUserModelId;
use windows::Win32::System::Com::CoTaskMemFree;
use windows::Win32::System::Com::StructuredStorage::PropVariantToStringAlloc;
use windows::Win32::System::Threading::{
    OpenProcess, OpenProcessToken, PROCESS_NAME_WIN32, PROCESS_QUERY_LIMITED_INFORMATION, QueryFullProcessImageNameW,
};
use windows::Win32::UI::Shell::PropertiesSystem::{IPropertyStore, SHGetPropertyStoreForWindow};
use windows::Win32::UI::WindowsAndMessaging::*;
use windows::core::{BOOL, GUID, PWSTR};

use crate::dock_model::WindowEntry;
use crate::foreground::{self, Kind};

/// `PKEY_AppUserModel_ID` (propkey.h).
const PKEY_APP_USER_MODEL_ID: PROPERTYKEY =
    PROPERTYKEY { fmtid: GUID::from_u128(0x9F4C2855_9F79_4B39_A8D0_E1D42DE1D5F3), pid: 5 };

const FRAME_HOST_CLASS: &str = "ApplicationFrameWindow";

pub fn is_cloaked(hwnd: HWND) -> bool {
    let mut cloaked = 0u32;
    unsafe { DwmGetWindowAttribute(hwnd, DWMWA_CLOAKED, &mut cloaked as *mut _ as _, 4).is_ok() && cloaked != 0 }
}

/// Taskbar-style eligibility: visible, not cloaked (other virtual desktop or
/// suspended UWP), titled, and either unowned or explicitly `WS_EX_APPWINDOW`.
pub fn is_eligible(hwnd: HWND) -> bool {
    unsafe {
        if !IsWindowVisible(hwnd).as_bool() {
            return false;
        }
        let ex = WINDOW_EX_STYLE(GetWindowLongPtrW(hwnd, GWL_EXSTYLE) as u32);
        if !ex.contains(WS_EX_APPWINDOW) {
            if ex.contains(WS_EX_TOOLWINDOW) || ex.contains(WS_EX_NOACTIVATE) {
                return false;
            }
            if GetWindow(hwnd, GW_OWNER).is_ok_and(|o| !o.0.is_null()) {
                return false;
            }
        }
    }
    if is_cloaked(hwnd) || foreground::title(hwnd).trim().is_empty() {
        return false;
    }
    foreground::classify(hwnd) == Kind::App
}

/// Build the dock entry for `hwnd`, or `None` if it does not belong in the dock.
pub fn window_entry(hwnd: HWND) -> Option<WindowEntry> {
    if !is_eligible(hwnd) {
        return None;
    }
    let mut pid = 0u32;
    unsafe {
        GetWindowThreadProcessId(hwnd, Some(&mut pid));
    }
    // A UWP app's top-level window belongs to ApplicationFrameHost.exe; the app
    // itself owns a child CoreWindow. Until that child exists the app cannot be
    // identified, so the frame is skipped (its child's SHOW event re-queues it).
    let app_pid = if foreground::class_name(hwnd) == FRAME_HOST_CLASS { hosted_app_pid(hwnd, pid)? } else { pid };

    let process = Process::open(app_pid);
    let exe_path = process.as_ref().and_then(|p| p.image_path()).unwrap_or_default();
    let aumid = window_aumid(hwnd).or_else(|| process.as_ref().and_then(|p| p.package_aumid()));
    let is_elevated = process.as_ref().map(|p| p.is_elevated()).unwrap_or(true);
    if exe_path.is_empty() && aumid.is_none() {
        return None;
    }
    let app_id = aumid.clone().unwrap_or_else(|| exe_path.clone()).to_lowercase();
    Some(WindowEntry {
        hwnd: hwnd.0 as isize,
        app_id,
        exe_path,
        aumid,
        title: foreground::title(hwnd),
        process_id: app_pid,
        is_elevated,
    })
}

/// Every eligible top-level window, in z-order (full rescan; event-triggered only).
pub fn enumerate() -> Vec<WindowEntry> {
    let mut hwnds: Vec<HWND> = Vec::new();
    unsafe extern "system" fn collect(hwnd: HWND, lp: LPARAM) -> BOOL {
        let v = unsafe { &mut *(lp.0 as *mut Vec<HWND>) };
        v.push(hwnd);
        BOOL(1)
    }
    unsafe {
        let _ = EnumWindows(Some(collect), LPARAM(&mut hwnds as *mut _ as isize));
    }
    hwnds.into_iter().filter_map(window_entry).collect()
}

/// The top-level window that represents `hwnd` in the dock (owned dialogs map to their owner).
pub fn root_owner(hwnd: HWND) -> HWND {
    let r = unsafe { GetAncestor(hwnd, GA_ROOTOWNER) };
    if r.0.is_null() { hwnd } else { r }
}

fn hosted_app_pid(frame: HWND, frame_pid: u32) -> Option<u32> {
    struct Ctx {
        frame_pid: u32,
        found: u32,
    }
    unsafe extern "system" fn find(child: HWND, lp: LPARAM) -> BOOL {
        let ctx = unsafe { &mut *(lp.0 as *mut Ctx) };
        let mut pid = 0u32;
        unsafe {
            GetWindowThreadProcessId(child, Some(&mut pid));
        }
        if pid != 0 && pid != ctx.frame_pid {
            ctx.found = pid;
            return BOOL(0);
        }
        BOOL(1)
    }
    let mut ctx = Ctx { frame_pid, found: 0 };
    unsafe {
        let _ = EnumChildWindows(Some(frame), Some(find), LPARAM(&mut ctx as *mut _ as isize));
    }
    (ctx.found != 0).then_some(ctx.found)
}

/// Explicit per-window AUMID (set by apps such as browsers for profiles / PWAs).
fn window_aumid(hwnd: HWND) -> Option<String> {
    unsafe {
        let store: IPropertyStore = SHGetPropertyStoreForWindow(hwnd).ok()?;
        let value = store.GetValue(&PKEY_APP_USER_MODEL_ID).ok()?;
        let p = PropVariantToStringAlloc(&value).ok()?;
        let s = p.to_string().unwrap_or_default();
        CoTaskMemFree(Some(p.0 as _));
        (!s.is_empty()).then_some(s)
    }
}

struct Process(HANDLE);

impl Process {
    fn open(pid: u32) -> Option<Self> {
        unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid) }.ok().map(Self)
    }

    fn image_path(&self) -> Option<String> {
        let mut buf = [0u16; 1024];
        let mut len = buf.len() as u32;
        unsafe { QueryFullProcessImageNameW(self.0, PROCESS_NAME_WIN32, PWSTR(buf.as_mut_ptr()), &mut len) }.ok()?;
        Some(String::from_utf16_lossy(&buf[..len as usize]))
    }

    /// AUMID of a packaged (MSIX/UWP) process.
    fn package_aumid(&self) -> Option<String> {
        let mut buf = [0u16; 512];
        let mut len = buf.len() as u32;
        let rc = unsafe { GetApplicationUserModelId(self.0, &mut len, Some(PWSTR(buf.as_mut_ptr()))) };
        if rc.0 != 0 || len == 0 {
            return None;
        }
        Some(String::from_utf16_lossy(&buf[..(len as usize).saturating_sub(1)]))
    }

    /// True if the process is elevated, or if its token cannot be read (treated as elevated
    /// so M4 can show the shield badge rather than fail silently).
    fn is_elevated(&self) -> bool {
        unsafe {
            let mut token = HANDLE::default();
            if OpenProcessToken(self.0, TOKEN_QUERY, &mut token).is_err() {
                return true;
            }
            let mut elev = TOKEN_ELEVATION::default();
            let mut ret = 0u32;
            let ok = GetTokenInformation(
                token,
                TokenElevation,
                Some(&mut elev as *mut _ as _),
                size_of::<TOKEN_ELEVATION>() as u32,
                &mut ret,
            )
            .is_ok();
            let _ = CloseHandle(token);
            !ok || elev.TokenIsElevated != 0
        }
    }
}

impl Drop for Process {
    fn drop(&mut self) {
        unsafe {
            let _ = CloseHandle(self.0);
        }
    }
}
