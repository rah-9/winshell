//! Top bar clock (PLAN.md sections 7 and 8).
//!
//! One-shot timing: `SetTimer` repeats by default, so the WM_TIMER handler must
//! call [`Clock::on_timer`], which kills the timer *before* re-arming a fresh
//! one for the newly computed distance to the next minute boundary. Re-armed
//! on WM_TIMECHANGE and on resume from sleep.

use windows::Win32::Foundation::{HWND, SYSTEMTIME};
use windows::Win32::Globalization::{ENUM_DATE_FORMATS_FLAGS, GetDateFormatEx, GetTimeFormatEx, TIME_FORMAT_FLAGS};
use windows::Win32::System::SystemInformation::GetLocalTime;
use windows::Win32::UI::WindowsAndMessaging::{KillTimer, SetTimer};
use windows::core::{PCWSTR, w};

use crate::util::ms_to_next_minute;

pub struct Clock {
    hwnd: HWND,
    timer_id: usize,
    armed: bool,
}

impl Clock {
    pub fn new(hwnd: HWND, timer_id: usize) -> Self {
        Self { hwnd, timer_id, armed: false }
    }

    /// (Re)arm a single timer for just past the next minute boundary.
    pub fn arm(&mut self) {
        self.disarm();
        let t = unsafe { GetLocalTime() };
        let ms = ms_to_next_minute(t.wSecond, t.wMilliseconds);
        let id = unsafe { SetTimer(Some(self.hwnd), self.timer_id, ms, None) };
        self.armed = id != 0;
    }

    pub fn disarm(&mut self) {
        if self.armed {
            unsafe {
                let _ = KillTimer(Some(self.hwnd), self.timer_id);
            }
            self.armed = false;
        }
    }

    /// WM_TIMER handler: kill first (SetTimer would otherwise repeat), then re-arm.
    pub fn on_timer(&mut self) {
        self.disarm();
        self.arm();
    }

    pub fn timer_id(&self) -> usize {
        self.timer_id
    }
}

impl Drop for Clock {
    fn drop(&mut self) {
        self.disarm();
    }
}

/// "Fri 20 Nov, 02:06 PM" in the user's locale (day/month names localised).
pub fn now_text() -> String {
    let t = unsafe { GetLocalTime() };
    format_time(&t)
}

pub fn format_time(t: &SYSTEMTIME) -> String {
    let date = format_with(|buf| unsafe {
        GetDateFormatEx(PCWSTR::null(), ENUM_DATE_FORMATS_FLAGS(0), Some(t), w!("ddd dd MMM"), buf, PCWSTR::null())
    });
    let time = format_with(|buf| unsafe {
        GetTimeFormatEx(PCWSTR::null(), TIME_FORMAT_FLAGS(0), Some(t), w!("hh:mm tt"), buf)
    });
    match (date.is_empty(), time.is_empty()) {
        (false, false) => format!("{date}, {time}"),
        (true, false) => time,
        _ => format!("{:02}:{:02}", t.wHour, t.wMinute),
    }
}

fn format_with(f: impl Fn(Option<&mut [u16]>) -> i32) -> String {
    let mut buf = [0u16; 64];
    let n = f(Some(&mut buf));
    if n <= 1 {
        return String::new();
    }
    String::from_utf16_lossy(&buf[..(n as usize - 1)])
}
