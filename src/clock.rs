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

/// Format a date with a GetDateFormatEx picture ("dddd, d MMMM", "MMMM yyyy", "ddd").
pub fn format_date(year: i32, month: u32, day: u32, picture: &str) -> String {
    let t = SYSTEMTIME { wYear: year as u16, wMonth: month as u16, wDay: day as u16, ..Default::default() };
    let pic = windows::core::HSTRING::from(picture);
    format_with(|buf| unsafe {
        GetDateFormatEx(PCWSTR::null(), ENUM_DATE_FORMATS_FLAGS(0), Some(&t), &pic, buf, PCWSTR::null())
    })
}

/// Today's (year, month, day) in local time.
pub fn today() -> (i32, u32, u32) {
    let t = unsafe { GetLocalTime() };
    (t.wYear as i32, t.wMonth as u32, t.wDay as u32)
}

/// First day of the week for the user's locale: 0 = Sunday .. 6 = Saturday.
pub fn first_weekday() -> u32 {
    use windows::Win32::Globalization::{GetLocaleInfoEx, LOCALE_IFIRSTDAYOFWEEK};
    let mut buf = [0u16; 4];
    let n = unsafe { GetLocaleInfoEx(PCWSTR::null(), LOCALE_IFIRSTDAYOFWEEK, Some(&mut buf)) };
    if n < 2 {
        return 1;
    }
    // LOCALE_IFIRSTDAYOFWEEK: 0 = Monday .. 6 = Sunday.
    let v = (buf[0] as u32).saturating_sub('0' as u32).min(6);
    (v + 1) % 7
}
