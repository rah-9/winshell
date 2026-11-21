//! Process hardening applied first thing at startup (M9).
//!
//! * DLL search: only System32 for later LoadLibrary calls, and the current
//!   directory removed from the search path (DLL planting).
//! * Image load policy: refuse DLLs from remote (UNC) paths and low-integrity
//!   files, prefer System32 copies.
//! * Extension points disabled: legacy injection routes such as AppInit DLLs and
//!   Winsock LSPs are not loaded into this process.
//!
//! All of these only restrict this process; failures are logged and ignored.

use windows::Win32::System::LibraryLoader::{LOAD_LIBRARY_SEARCH_SYSTEM32, SetDefaultDllDirectories, SetDllDirectoryW};
use windows::Win32::System::Threading::{PROCESS_MITIGATION_POLICY, SetProcessMitigationPolicy};
use windows::core::w;

const PROCESS_EXTENSION_POINT_DISABLE_POLICY: PROCESS_MITIGATION_POLICY = PROCESS_MITIGATION_POLICY(6);
const PROCESS_IMAGE_LOAD_POLICY: PROCESS_MITIGATION_POLICY = PROCESS_MITIGATION_POLICY(10);

// PROCESS_MITIGATION_IMAGE_LOAD_POLICY bit fields.
const NO_REMOTE_IMAGES: u32 = 1 << 0;
const NO_LOW_MANDATORY_LABEL_IMAGES: u32 = 1 << 1;
const PREFER_SYSTEM32_IMAGES: u32 = 1 << 2;
// PROCESS_MITIGATION_EXTENSION_POINT_DISABLE_POLICY bit field.
const DISABLE_EXTENSION_POINTS: u32 = 1 << 0;

/// Returns a short report of what could not be applied (empty = all applied).
pub fn harden_process() -> Vec<String> {
    let mut failed = Vec::new();
    unsafe {
        if let Err(e) = SetDefaultDllDirectories(LOAD_LIBRARY_SEARCH_SYSTEM32) {
            failed.push(format!("SetDefaultDllDirectories: {e}"));
        }
        if let Err(e) = SetDllDirectoryW(w!("")) {
            failed.push(format!("SetDllDirectoryW: {e}"));
        }
        let image = NO_REMOTE_IMAGES | NO_LOW_MANDATORY_LABEL_IMAGES | PREFER_SYSTEM32_IMAGES;
        if let Err(e) = SetProcessMitigationPolicy(PROCESS_IMAGE_LOAD_POLICY, &image as *const u32 as _, 4) {
            failed.push(format!("image load policy: {e}"));
        }
        let ext = DISABLE_EXTENSION_POINTS;
        if let Err(e) = SetProcessMitigationPolicy(PROCESS_EXTENSION_POINT_DISABLE_POLICY, &ext as *const u32 as _, 4) {
            failed.push(format!("extension point policy: {e}"));
        }
    }
    failed
}
