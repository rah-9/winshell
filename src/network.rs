//! Network status for the Wi-Fi pill (PLAN.md section 7).
//!
//! Change sources, all callbacks (no polling):
//! * `WlanRegisterNotification`: connect / disconnect / signal quality, filtered
//!   so routine background scans do not wake the UI.
//! * `NotifyIpInterfaceChange`: covers Ethernet-only machines and a stopped
//!   WLAN service.
//!
//! Callbacks run on system threads and post one coalesced message; the state
//! is read on the UI thread with cheap local queries.

use std::sync::atomic::{AtomicBool, AtomicIsize, Ordering};

use windows::Win32::Foundation::{HANDLE, HWND, LPARAM, WPARAM};
use windows::Win32::NetworkManagement::IpHelper::*;
use windows::Win32::NetworkManagement::Ndis::IfOperStatusUp;
use windows::Win32::NetworkManagement::WiFi::*;
use windows::Win32::Networking::WinSock::AF_UNSPEC;
use windows::Win32::UI::WindowsAndMessaging::{PostMessageW, WM_APP};

pub const WM_SHELL_NETWORK: u32 = WM_APP + 0x25;

static TARGET: AtomicIsize = AtomicIsize::new(0);
static PENDING: AtomicBool = AtomicBool::new(false);

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum NetState {
    Disconnected,
    Ethernet,
    /// Signal quality 0-100.
    Wifi {
        ssid: String,
        quality: u32,
    },
}

pub struct Network {
    wlan: HANDLE,
    ip_notify: HANDLE,
}

fn post() {
    if !PENDING.swap(true, Ordering::SeqCst) {
        let t = TARGET.load(Ordering::SeqCst);
        if t != 0 {
            unsafe {
                let _ = PostMessageW(Some(HWND(t as _)), WM_SHELL_NETWORK, WPARAM(0), LPARAM(0));
            }
        }
    }
}

unsafe extern "system" fn on_wlan(data: *mut L2_NOTIFICATION_DATA, _ctx: *mut core::ffi::c_void) {
    let Some(d) = (unsafe { data.as_ref() }) else { return };
    let src = d.NotificationSource;
    let code = d.NotificationCode as i32;
    let relevant = if src == WLAN_NOTIFICATION_SOURCE_ACM {
        [
            wlan_notification_acm_connection_complete.0,
            wlan_notification_acm_disconnected.0,
            wlan_notification_acm_interface_arrival.0,
            wlan_notification_acm_interface_removal.0,
            wlan_notification_acm_autoconf_enabled.0,
            wlan_notification_acm_autoconf_disabled.0,
        ]
        .contains(&code)
    } else if src == WLAN_NOTIFICATION_SOURCE_MSM {
        [
            wlan_notification_msm_connected.0,
            wlan_notification_msm_disconnected.0,
            wlan_notification_msm_signal_quality_change.0,
        ]
        .contains(&code)
    } else {
        false
    };
    if relevant {
        post();
    }
}

unsafe extern "system" fn on_ip(
    _ctx: *const core::ffi::c_void,
    _row: *const MIB_IPINTERFACE_ROW,
    _kind: MIB_NOTIFICATION_TYPE,
) {
    post();
}

impl Network {
    pub fn start(post_to: HWND) -> Self {
        TARGET.store(post_to.0 as isize, Ordering::SeqCst);
        let mut wlan = HANDLE::default();
        unsafe {
            let mut ver = 0u32;
            if WlanOpenHandle(2, None, &mut ver, &mut wlan) == 0 {
                let rc = WlanRegisterNotification(
                    wlan,
                    WLAN_NOTIFICATION_SOURCE_ACM | WLAN_NOTIFICATION_SOURCE_MSM,
                    true,
                    Some(on_wlan),
                    None,
                    None,
                    None,
                );
                if rc != 0 {
                    crate::log!("network: WLAN notifications unavailable ({rc})");
                }
            } else {
                wlan = HANDLE::default();
                crate::log!("network: WLAN service unavailable; using IP interface changes only");
            }
        }
        let mut ip_notify = HANDLE::default();
        unsafe {
            let rc = NotifyIpInterfaceChange(AF_UNSPEC, Some(on_ip), None, false, &mut ip_notify);
            if rc.is_err() {
                crate::log!("network: NotifyIpInterfaceChange failed ({:?})", rc);
            }
        }
        Self { wlan, ip_notify }
    }

    pub fn ack(&self) {
        PENDING.store(false, Ordering::SeqCst);
    }

    pub fn state(&self) -> NetState {
        if let Some(w) = self.wifi() {
            return w;
        }
        if ethernet_up() { NetState::Ethernet } else { NetState::Disconnected }
    }

    fn wifi(&self) -> Option<NetState> {
        if self.wlan.is_invalid() {
            return None;
        }
        unsafe {
            let mut list: *mut WLAN_INTERFACE_INFO_LIST = std::ptr::null_mut();
            if WlanEnumInterfaces(self.wlan, None, &mut list) != 0 || list.is_null() {
                return None;
            }
            let count = (*list).dwNumberOfItems as usize;
            let items = std::slice::from_raw_parts((*list).InterfaceInfo.as_ptr(), count);
            let mut result = None;
            for info in items {
                if info.isState != wlan_interface_state_connected {
                    continue;
                }
                let mut size = 0u32;
                let mut data: *mut core::ffi::c_void = std::ptr::null_mut();
                if WlanQueryInterface(
                    self.wlan,
                    &info.InterfaceGuid,
                    wlan_intf_opcode_current_connection,
                    None,
                    &mut size,
                    &mut data,
                    None,
                ) == 0
                    && !data.is_null()
                {
                    let attrs = &*(data as *const WLAN_CONNECTION_ATTRIBUTES);
                    let ssid = &attrs.wlanAssociationAttributes.dot11Ssid;
                    let len = (ssid.uSSIDLength as usize).min(ssid.ucSSID.len());
                    result = Some(NetState::Wifi {
                        ssid: String::from_utf8_lossy(&ssid.ucSSID[..len]).into_owned(),
                        quality: attrs.wlanAssociationAttributes.wlanSignalQuality.min(100),
                    });
                    WlanFreeMemory(data);
                }
                if result.is_some() {
                    break;
                }
            }
            WlanFreeMemory(list as _);
            result
        }
    }

    pub fn shutdown(&mut self) {
        unsafe {
            if !self.ip_notify.is_invalid() {
                let _ = CancelMibChangeNotify2(self.ip_notify);
                self.ip_notify = HANDLE::default();
            }
            if !self.wlan.is_invalid() {
                let _ = WlanCloseHandle(self.wlan, None);
                self.wlan = HANDLE::default();
            }
        }
    }
}

impl Drop for Network {
    fn drop(&mut self) {
        self.shutdown();
    }
}

/// An operational Ethernet adapter with a default gateway.
fn ethernet_up() -> bool {
    const IF_TYPE_ETHERNET_CSMACD: u32 = 6;
    unsafe {
        let flags =
            GAA_FLAG_SKIP_ANYCAST | GAA_FLAG_SKIP_MULTICAST | GAA_FLAG_SKIP_DNS_SERVER | GAA_FLAG_INCLUDE_GATEWAYS;
        let mut size = 16 * 1024u32;
        for _ in 0..3 {
            let mut buf = vec![0u8; size as usize];
            let p = buf.as_mut_ptr() as *mut IP_ADAPTER_ADDRESSES_LH;
            let rc = GetAdaptersAddresses(AF_UNSPEC.0 as u32, flags, None, Some(p), &mut size);
            if rc == 111 {
                continue; // ERROR_BUFFER_OVERFLOW: size updated, retry
            }
            if rc != 0 {
                return false;
            }
            let mut cur = p as *const IP_ADAPTER_ADDRESSES_LH;
            while let Some(a) = cur.as_ref() {
                if a.IfType == IF_TYPE_ETHERNET_CSMACD
                    && a.OperStatus == IfOperStatusUp
                    && !a.FirstGatewayAddress.is_null()
                {
                    return true;
                }
                cur = a.Next;
            }
            return false;
        }
        false
    }
}
