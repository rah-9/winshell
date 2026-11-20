//! Private Working Set measurement (PLAN.md section 8). This is the number
//! every memory gate is judged by. Nothing here trims the working set.

use windows::Win32::System::ProcessStatus::{
    GetProcessMemoryInfo, PROCESS_MEMORY_COUNTERS, PROCESS_MEMORY_COUNTERS_EX2,
};
use windows::Win32::System::Threading::GetCurrentProcess;

#[derive(Clone, Copy, Debug, Default)]
pub struct MemSnapshot {
    /// Private Working Set in bytes (Task Manager "Memory (private working set)").
    pub private_ws: usize,
    /// Total working set (shared + private), for context only.
    pub working_set: usize,
    /// Private commit, for context only.
    pub private_commit: usize,
}

impl MemSnapshot {
    pub fn now() -> Self {
        let mut c =
            PROCESS_MEMORY_COUNTERS_EX2 { cb: size_of::<PROCESS_MEMORY_COUNTERS_EX2>() as u32, ..Default::default() };
        let ok = unsafe {
            GetProcessMemoryInfo(GetCurrentProcess(), &mut c as *mut _ as *mut PROCESS_MEMORY_COUNTERS, c.cb)
        };
        if ok.is_err() {
            return Self::default();
        }
        Self { private_ws: c.PrivateWorkingSetSize, working_set: c.WorkingSetSize, private_commit: c.PrivateUsage }
    }

    pub fn private_mb(&self) -> f64 {
        self.private_ws as f64 / (1024.0 * 1024.0)
    }

    pub fn describe(&self) -> String {
        let mb = |b: usize| b as f64 / (1024.0 * 1024.0);
        format!(
            "private WS {:.1} MB (WS {:.1} MB, private commit {:.1} MB)",
            mb(self.private_ws),
            mb(self.working_set),
            mb(self.private_commit)
        )
    }
}
