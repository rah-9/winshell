//! Live reload (PLAN.md sections 11 and 13, M8): a thread that blocks in
//! `ReadDirectoryChangesW` on `%APPDATA%\WinShellTheme` and posts one coalesced
//! message when `config.toml` or a `themes\*.toml` file changes. It never
//! polls: the call returns only when the file system reports a change.
//!
//! The returned buffer is parsed with explicit bounds checks; a malformed or
//! overflowed buffer is treated as "something changed".

use std::os::windows::io::AsRawHandle;
use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread::JoinHandle;

use windows::Win32::Foundation::{CloseHandle, HANDLE, HWND, LPARAM, WPARAM};
use windows::Win32::Storage::FileSystem::*;
use windows::Win32::System::IO::CancelSynchronousIo;
use windows::Win32::UI::WindowsAndMessaging::{PostMessageW, WM_APP};
use windows::core::HSTRING;

use crate::util::buffer_has_relevant;

pub const WM_SHELL_CONFIG: u32 = WM_APP + 0x27;

const BUF_WORDS: usize = 4096; // 16 KiB, DWORD-aligned as the API requires

pub struct ConfigWatcher {
    pending: Arc<AtomicBool>,
    stop: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
}

impl ConfigWatcher {
    pub fn start(dir: &Path, notify: HWND) -> Option<Self> {
        let handle = unsafe {
            CreateFileW(
                &HSTRING::from(dir.as_os_str()),
                FILE_LIST_DIRECTORY.0,
                FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE,
                None,
                OPEN_EXISTING,
                FILE_FLAG_BACKUP_SEMANTICS,
                None,
            )
        }
        .ok()?;
        let pending = Arc::new(AtomicBool::new(false));
        let stop = Arc::new(AtomicBool::new(false));
        let (p, st) = (pending.clone(), stop.clone());
        let raw_dir = handle.0 as isize;
        let target = notify.0 as isize;
        let thread = std::thread::Builder::new()
            .name("config-watcher".into())
            .spawn(move || {
                let dir = HANDLE(raw_dir as _);
                let mut buf = vec![0u32; BUF_WORDS];
                let bytes = (BUF_WORDS * 4) as u32;
                while !st.load(Ordering::SeqCst) {
                    let mut returned = 0u32;
                    let ok = unsafe {
                        ReadDirectoryChangesW(
                            dir,
                            buf.as_mut_ptr() as _,
                            bytes,
                            true,
                            FILE_NOTIFY_CHANGE_FILE_NAME | FILE_NOTIFY_CHANGE_LAST_WRITE | FILE_NOTIFY_CHANGE_SIZE,
                            Some(&mut returned),
                            None,
                            None,
                        )
                    };
                    if ok.is_err() || st.load(Ordering::SeqCst) {
                        break;
                    }
                    let view = unsafe { std::slice::from_raw_parts(buf.as_ptr() as *const u8, buf.len() * 4) };
                    if buffer_has_relevant(view, returned as usize) && !p.swap(true, Ordering::SeqCst) {
                        unsafe {
                            let _ = PostMessageW(Some(HWND(target as _)), WM_SHELL_CONFIG, WPARAM(0), LPARAM(0));
                        }
                    }
                }
                unsafe {
                    let _ = CloseHandle(dir);
                }
            })
            .ok()?;
        Some(Self { pending, stop, thread: Some(thread) })
    }

    /// Clear the coalescing flag. Call first in the WM_SHELL_CONFIG handler.
    pub fn ack(&self) {
        self.pending.store(false, Ordering::SeqCst);
    }

    /// Wake the thread out of ReadDirectoryChangesW and let it exit. Not joined.
    pub fn stop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
        if let Some(t) = self.thread.take() {
            unsafe {
                let _ = CancelSynchronousIo(HANDLE(t.as_raw_handle()));
            }
        }
    }
}

impl Drop for ConfigWatcher {
    fn drop(&mut self) {
        self.stop();
    }
}
