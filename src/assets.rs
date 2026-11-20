//! Shell asset loader thread (PLAN.md section 6, thread 3, STA).
//!
//! Loads app icons with `IShellItemImageFactory` (exe path, or
//! `shell:AppsFolder\<AUMID>` for packaged apps) and friendly names, off the
//! UI thread. Requests arrive on an mpsc channel; results go back on another
//! channel plus one `PostMessageW(WM_SHELL_ASSET)`. The thread blocks on its
//! channel and never wakes on a timer.

use std::sync::mpsc::{self, Receiver, Sender};
use std::thread;

use windows::Win32::Foundation::{HWND, LPARAM, SIZE, WPARAM};
use windows::Win32::Graphics::Gdi::DeleteObject;
use windows::Win32::Storage::FileSystem::{GetFileVersionInfoSizeW, GetFileVersionInfoW, VerQueryValueW};
use windows::Win32::System::Com::{
    COINIT_APARTMENTTHREADED, COINIT_DISABLE_OLE1DDE, CoInitializeEx, CoTaskMemFree, CoUninitialize,
};
use windows::Win32::UI::Shell::{
    IShellItem, IShellItemImageFactory, SHCreateItemFromParsingName, SIGDN_NORMALDISPLAY, SIIGBF_ICONONLY,
};
use windows::Win32::UI::WindowsAndMessaging::{PostMessageW, WM_APP};
use windows::core::{HSTRING, Interface, PCWSTR, w};

use crate::icon::{self, Bgra};

pub const WM_SHELL_ASSET: u32 = WM_APP + 0x23;

#[derive(Clone, Debug)]
pub struct AssetRequest {
    pub app_id: String,
    pub aumid: Option<String>,
    pub exe_path: String,
    /// Square icon size in pixels.
    pub size: u32,
}

pub struct AssetResult {
    pub app_id: String,
    pub display_name: String,
    pub icon: Option<Bgra>,
}

pub struct AssetLoader {
    tx: Option<Sender<AssetRequest>>,
    rx: Receiver<AssetResult>,
}

impl AssetLoader {
    pub fn start(notify: HWND) -> Self {
        let (req_tx, req_rx) = mpsc::channel::<AssetRequest>();
        let (res_tx, res_rx) = mpsc::channel::<AssetResult>();
        let target = notify.0 as isize;
        let spawned = thread::Builder::new().name("shell-asset-loader".into()).spawn(move || {
            unsafe {
                let _ = CoInitializeEx(None, COINIT_APARTMENTTHREADED | COINIT_DISABLE_OLE1DDE);
            }
            while let Ok(req) = req_rx.recv() {
                let result = load(&req);
                if res_tx.send(result).is_err() {
                    break;
                }
                unsafe {
                    let _ = PostMessageW(Some(HWND(target as _)), WM_SHELL_ASSET, WPARAM(0), LPARAM(0));
                }
            }
            unsafe { CoUninitialize() };
        });
        if let Err(e) = spawned {
            crate::log!("WARNING: asset loader thread failed to start: {e}");
        }
        Self { tx: Some(req_tx), rx: res_rx }
    }

    pub fn request(&self, req: AssetRequest) {
        if let Some(tx) = &self.tx {
            let _ = tx.send(req);
        }
    }

    pub fn drain(&self) -> Vec<AssetResult> {
        self.rx.try_iter().collect()
    }

    /// Close the request channel; the thread exits after its current item.
    /// Not joined: a misbehaving shell extension must not block app exit.
    pub fn shutdown(&mut self) {
        self.tx = None;
    }
}

fn load(req: &AssetRequest) -> AssetResult {
    let mut display_name = String::new();
    let mut icon = None;

    if let Some(aumid) = &req.aumid
        && let Some(item) = shell_item(&format!("shell:AppsFolder\\{aumid}"))
    {
        display_name = item_name(&item).unwrap_or_default();
        icon = item_icon(&item, req.size);
    }
    if icon.is_none()
        && !req.exe_path.is_empty()
        && let Some(item) = shell_item(&req.exe_path)
    {
        icon = item_icon(&item, req.size);
    }
    if display_name.is_empty() {
        display_name = file_description(&req.exe_path).unwrap_or_else(|| file_stem(&req.exe_path));
    }
    AssetResult { app_id: req.app_id.clone(), display_name, icon }
}

fn shell_item(path: &str) -> Option<IShellItem> {
    unsafe { SHCreateItemFromParsingName(&HSTRING::from(path), None) }.ok()
}

fn item_name(item: &IShellItem) -> Option<String> {
    unsafe {
        let p = item.GetDisplayName(SIGDN_NORMALDISPLAY).ok()?;
        let s = p.to_string().ok();
        CoTaskMemFree(Some(p.0 as _));
        s.filter(|s| !s.is_empty())
    }
}

fn item_icon(item: &IShellItem, size: u32) -> Option<Bgra> {
    unsafe {
        let factory: IShellItemImageFactory = item.cast().ok()?;
        let hbmp = factory.GetImage(SIZE { cx: size as i32, cy: size as i32 }, SIIGBF_ICONONLY).ok()?;
        let out = icon::hbitmap_to_bgra(hbmp);
        let _ = DeleteObject(hbmp.into());
        out
    }
}

/// `FileDescription` from the exe's version resource ("Google Chrome", "Notepad", ...).
fn file_description(exe: &str) -> Option<String> {
    if exe.is_empty() {
        return None;
    }
    let path = HSTRING::from(exe);
    unsafe {
        let size = GetFileVersionInfoSizeW(&path, None);
        if size == 0 {
            return None;
        }
        let mut data = vec![0u8; size as usize];
        GetFileVersionInfoW(&path, None, size, data.as_mut_ptr() as _).ok()?;

        let mut ptr = std::ptr::null_mut();
        let mut len = 0u32;
        let mut lang_cp = vec!["040904b0".to_string(), "040904e4".to_string()];
        if VerQueryValueW(data.as_ptr() as _, w!("\\VarFileInfo\\Translation"), &mut ptr, &mut len).as_bool()
            && len >= 4
        {
            let t = std::slice::from_raw_parts(ptr as *const u16, 2);
            lang_cp.insert(0, format!("{:04x}{:04x}", t[0], t[1]));
        }
        for lc in lang_cp {
            let key = HSTRING::from(format!("\\StringFileInfo\\{lc}\\FileDescription"));
            if VerQueryValueW(data.as_ptr() as _, &key, &mut ptr, &mut len).as_bool() && len > 1 {
                let s = PCWSTR(ptr as *const u16).to_string().unwrap_or_default();
                let s = s.trim().to_string();
                if !s.is_empty() {
                    return Some(s);
                }
            }
        }
    }
    None
}

fn file_stem(path: &str) -> String {
    std::path::Path::new(path).file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default()
}
