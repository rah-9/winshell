//! Single instance via a per-session named mutex (PLAN.md section 10).

use windows::Win32::Foundation::{CloseHandle, ERROR_ALREADY_EXISTS, GetLastError, HANDLE};
use windows::Win32::System::Threading::CreateMutexW;
use windows::core::PCWSTR;

pub struct SingleInstance(HANDLE);

impl SingleInstance {
    /// Returns `None` if another instance already holds the mutex.
    pub fn acquire(name: PCWSTR) -> Option<Self> {
        let handle = unsafe { CreateMutexW(None, true, name) }.ok()?;
        if unsafe { GetLastError() } == ERROR_ALREADY_EXISTS {
            unsafe {
                let _ = CloseHandle(handle);
            }
            return None;
        }
        Some(Self(handle))
    }
}

impl Drop for SingleInstance {
    fn drop(&mut self) {
        unsafe {
            let _ = CloseHandle(self.0);
        }
    }
}
