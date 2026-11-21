//! Battery and power source (PLAN.md section 7). Change notifications come as
//! WM_POWERBROADCAST / PBT_POWERSETTINGCHANGE on the UI thread.

use windows::Win32::Foundation::{HANDLE, HWND};
use windows::Win32::System::Power::{
    GetSystemPowerStatus, HPOWERNOTIFY, RegisterPowerSettingNotification, SYSTEM_POWER_STATUS,
    UnregisterPowerSettingNotification,
};
use windows::Win32::UI::WindowsAndMessaging::DEVICE_NOTIFY_WINDOW_HANDLE;
use windows::core::GUID;

const GUID_BATTERY_PERCENTAGE_REMAINING: GUID = GUID::from_u128(0xa7ad8041_b45a_4cae_87a3_eecbb468a9e1);
const GUID_ACDC_POWER_SOURCE: GUID = GUID::from_u128(0x5d3e9a59_e9d5_4b00_a6bd_ff34ff516548);
const GUID_POWER_SAVING_STATUS: GUID = GUID::from_u128(0xe00958c0_c213_4ace_ac77_fecced2eeea5);

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct BatteryState {
    pub present: bool,
    pub percent: u8,
    pub on_ac: bool,
    pub charging: bool,
    pub saver: bool,
    /// Estimated seconds remaining on battery, when Windows knows.
    pub seconds_left: Option<u32>,
}

pub fn battery() -> BatteryState {
    let mut s = SYSTEM_POWER_STATUS::default();
    if unsafe { GetSystemPowerStatus(&mut s) }.is_err() {
        return BatteryState::default();
    }
    let present = s.BatteryFlag != 128 && s.BatteryFlag != 255 && s.BatteryLifePercent != 255;
    BatteryState {
        present,
        percent: s.BatteryLifePercent.min(100),
        on_ac: s.ACLineStatus == 1,
        charging: s.BatteryFlag & 8 != 0,
        saver: s.SystemStatusFlag == 1,
        seconds_left: (s.BatteryLifeTime != u32::MAX).then_some(s.BatteryLifeTime),
    }
}

pub struct PowerNotifications(Vec<HPOWERNOTIFY>);

impl PowerNotifications {
    pub fn register(hwnd: HWND) -> Self {
        let mut v = Vec::new();
        for g in [GUID_BATTERY_PERCENTAGE_REMAINING, GUID_ACDC_POWER_SOURCE, GUID_POWER_SAVING_STATUS] {
            match unsafe { RegisterPowerSettingNotification(HANDLE(hwnd.0), &g, DEVICE_NOTIFY_WINDOW_HANDLE) } {
                Ok(h) => v.push(h),
                Err(e) => crate::log!("power: notification {g:?} unavailable ({e})"),
            }
        }
        Self(v)
    }

    pub fn unregister(&mut self) {
        for h in self.0.drain(..) {
            unsafe {
                let _ = UnregisterPowerSettingNotification(h);
            }
        }
    }
}

impl Drop for PowerNotifications {
    fn drop(&mut self) {
        self.unregister();
    }
}
