//! Core Audio (PLAN.md section 7): master volume/mute and microphone level for
//! the default endpoints, output device list, and default device switching.
//!
//! Change notifications (`IAudioEndpointVolumeCallback`, `IMMNotificationClient`)
//! arrive on Core Audio's own threads; the callbacks only post one coalesced
//! message to the top bar. Reads and writes happen on the UI thread.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use windows::Win32::Devices::FunctionDiscovery::PKEY_Device_FriendlyName;
use windows::Win32::Foundation::{HWND, LPARAM, PROPERTYKEY, WPARAM};
use windows::Win32::Media::Audio::Endpoints::{
    IAudioEndpointVolume, IAudioEndpointVolumeCallback, IAudioEndpointVolumeCallback_Impl,
};
use windows::Win32::Media::Audio::*;
use windows::Win32::System::Com::StructuredStorage::PropVariantToStringAlloc;
use windows::Win32::System::Com::{CLSCTX_ALL, CLSCTX_INPROC_SERVER, CoCreateInstance, CoTaskMemFree, STGM_READ};
use windows::Win32::UI::WindowsAndMessaging::{PostMessageW, WM_APP};
use windows::core::{GUID, HRESULT, IUnknown, Interface, PCWSTR, Result, implement};

pub const WM_SHELL_AUDIO: u32 = WM_APP + 0x24;
/// wParam of WM_SHELL_AUDIO: devices changed (re-bind endpoints), else just levels.
pub const AUDIO_DEVICES_CHANGED: usize = 1;

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct AudioState {
    pub available: bool,
    pub volume: f32,
    pub muted: bool,
    pub mic_available: bool,
    pub mic_volume: f32,
    pub mic_muted: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AudioDevice {
    pub id: String,
    pub name: String,
    pub is_default: bool,
}

struct Notify {
    target: isize,
    levels_pending: AtomicBool,
    devices_pending: AtomicBool,
}

impl Notify {
    fn post(&self, devices: bool) {
        let flag = if devices { &self.devices_pending } else { &self.levels_pending };
        if !flag.swap(true, Ordering::SeqCst) {
            let wp = if devices { AUDIO_DEVICES_CHANGED } else { 0 };
            unsafe {
                let _ = PostMessageW(Some(HWND(self.target as _)), WM_SHELL_AUDIO, WPARAM(wp), LPARAM(0));
            }
        }
    }
}

#[implement(IAudioEndpointVolumeCallback)]
struct VolumeCallback(Arc<Notify>);

impl IAudioEndpointVolumeCallback_Impl for VolumeCallback_Impl {
    fn OnNotify(&self, _data: *mut AUDIO_VOLUME_NOTIFICATION_DATA) -> Result<()> {
        self.0.post(false);
        Ok(())
    }
}

#[implement(IMMNotificationClient)]
struct DeviceCallback(Arc<Notify>);

impl IMMNotificationClient_Impl for DeviceCallback_Impl {
    fn OnDeviceStateChanged(&self, _id: &PCWSTR, _state: DEVICE_STATE) -> Result<()> {
        self.0.post(true);
        Ok(())
    }
    fn OnDeviceAdded(&self, _id: &PCWSTR) -> Result<()> {
        self.0.post(true);
        Ok(())
    }
    fn OnDeviceRemoved(&self, _id: &PCWSTR) -> Result<()> {
        self.0.post(true);
        Ok(())
    }
    fn OnDefaultDeviceChanged(&self, _flow: EDataFlow, role: ERole, _id: &PCWSTR) -> Result<()> {
        if role == eConsole {
            self.0.post(true);
        }
        Ok(())
    }
    fn OnPropertyValueChanged(&self, _id: &PCWSTR, _key: &PROPERTYKEY) -> Result<()> {
        Ok(()) // fires very often (e.g. levels metadata); names are re-read on device events
    }
}

struct Endpoint {
    volume: IAudioEndpointVolume,
    callback: IAudioEndpointVolumeCallback,
}

impl Endpoint {
    fn bind(enumerator: &IMMDeviceEnumerator, flow: EDataFlow, notify: &Arc<Notify>) -> Option<Self> {
        unsafe {
            let device = enumerator.GetDefaultAudioEndpoint(flow, eConsole).ok()?;
            let volume: IAudioEndpointVolume = device.Activate(CLSCTX_ALL, None).ok()?;
            let callback: IAudioEndpointVolumeCallback = VolumeCallback(notify.clone()).into();
            volume.RegisterControlChangeNotify(&callback).ok()?;
            Some(Self { volume, callback })
        }
    }

    fn level(&self) -> (f32, bool) {
        unsafe {
            let v = self.volume.GetMasterVolumeLevelScalar().unwrap_or(0.0);
            let m = self.volume.GetMute().map(|b| b.as_bool()).unwrap_or(false);
            (v, m)
        }
    }
}

impl Drop for Endpoint {
    fn drop(&mut self) {
        unsafe {
            let _ = self.volume.UnregisterControlChangeNotify(&self.callback);
        }
    }
}

pub struct Audio {
    enumerator: IMMDeviceEnumerator,
    notify: Arc<Notify>,
    device_callback: IMMNotificationClient,
    render: Option<Endpoint>,
    capture: Option<Endpoint>,
}

impl Audio {
    /// Requires COM on the calling thread.
    pub fn new(post_to: HWND) -> Option<Self> {
        let notify = Arc::new(Notify {
            target: post_to.0 as isize,
            levels_pending: AtomicBool::new(false),
            devices_pending: AtomicBool::new(false),
        });
        unsafe {
            let enumerator: IMMDeviceEnumerator = CoCreateInstance(&MMDeviceEnumerator, None, CLSCTX_ALL).ok()?;
            let device_callback: IMMNotificationClient = DeviceCallback(notify.clone()).into();
            if enumerator.RegisterEndpointNotificationCallback(&device_callback).is_err() {
                crate::log!("audio: device notifications unavailable");
            }
            let mut a = Self { enumerator, notify, device_callback, render: None, capture: None };
            a.rebind();
            Some(a)
        }
    }

    /// Clear the coalescing flags. Call first in the WM_SHELL_AUDIO handler.
    pub fn ack(&self, devices: bool) {
        self.notify.levels_pending.store(false, Ordering::SeqCst);
        if devices {
            self.notify.devices_pending.store(false, Ordering::SeqCst);
        }
    }

    /// Default device changed (or appeared/disappeared): follow the new defaults.
    pub fn rebind(&mut self) {
        self.render = None;
        self.capture = None;
        self.render = Endpoint::bind(&self.enumerator, eRender, &self.notify);
        self.capture = Endpoint::bind(&self.enumerator, eCapture, &self.notify);
    }

    pub fn state(&self) -> AudioState {
        let mut s = AudioState::default();
        if let Some(r) = &self.render {
            s.available = true;
            (s.volume, s.muted) = r.level();
        }
        if let Some(c) = &self.capture {
            s.mic_available = true;
            (s.mic_volume, s.mic_muted) = c.level();
        }
        s
    }

    fn set(ep: &Option<Endpoint>, level: Option<f32>, mute: Option<bool>) {
        if let Some(e) = ep {
            unsafe {
                if let Some(v) = level {
                    let _ = e.volume.SetMasterVolumeLevelScalar(v.clamp(0.0, 1.0), std::ptr::null());
                }
                if let Some(m) = mute {
                    let _ = e.volume.SetMute(m, std::ptr::null());
                }
            }
        }
    }

    pub fn set_volume(&self, v: f32) {
        // Dragging the slider up from zero un-mutes, like the native flyout.
        let unmute = (v > 0.0 && self.state().muted).then_some(false);
        Self::set(&self.render, Some(v), unmute);
    }

    pub fn toggle_mute(&self) {
        let s = self.state();
        Self::set(&self.render, None, Some(!s.muted));
    }

    pub fn set_mic_volume(&self, v: f32) {
        Self::set(&self.capture, Some(v), None);
    }

    pub fn toggle_mic_mute(&self) {
        let s = self.state();
        Self::set(&self.capture, None, Some(!s.mic_muted));
    }

    /// Active output devices with friendly names; the default one flagged.
    pub fn render_devices(&self) -> Vec<AudioDevice> {
        let mut out = Vec::new();
        unsafe {
            let default_id = self
                .enumerator
                .GetDefaultAudioEndpoint(eRender, eConsole)
                .ok()
                .and_then(|d| device_id(&d))
                .unwrap_or_default();
            let Ok(coll) = self.enumerator.EnumAudioEndpoints(eRender, DEVICE_STATE_ACTIVE) else { return out };
            let n = coll.GetCount().unwrap_or(0);
            for i in 0..n {
                let Ok(dev) = coll.Item(i) else { continue };
                let Some(id) = device_id(&dev) else { continue };
                let name = friendly_name(&dev).unwrap_or_else(|| "Audio device".into());
                out.push(AudioDevice { is_default: id == default_id, id, name });
            }
        }
        out
    }

    /// Make `id` the default output for all roles via the undocumented
    /// `IPolicyConfig`. Every HRESULT is checked; returns false on any failure
    /// so the caller can open `ms-settings:sound` instead.
    pub fn set_default_output(&self, id: &str) -> bool {
        match policy_config_set_default(id) {
            Ok(()) => true,
            Err(e) => {
                crate::log!("audio: IPolicyConfig::SetDefaultEndpoint failed ({:#010x})", e.code().0);
                false
            }
        }
    }

    pub fn shutdown(&mut self) {
        self.render = None;
        self.capture = None;
        unsafe {
            let _ = self.enumerator.UnregisterEndpointNotificationCallback(&self.device_callback);
        }
    }
}

impl Drop for Audio {
    fn drop(&mut self) {
        self.shutdown();
    }
}

fn device_id(d: &IMMDevice) -> Option<String> {
    unsafe {
        let p = d.GetId().ok()?;
        let s = p.to_string().ok();
        CoTaskMemFree(Some(p.0 as _));
        s
    }
}

fn friendly_name(d: &IMMDevice) -> Option<String> {
    unsafe {
        let store = d.OpenPropertyStore(STGM_READ).ok()?;
        let v = store.GetValue(&PKEY_Device_FriendlyName).ok()?;
        let p = PropVariantToStringAlloc(&v).ok()?;
        let s = p.to_string().ok();
        CoTaskMemFree(Some(p.0 as _));
        s.filter(|s| !s.is_empty())
    }
}

// ---------------------------------------------------------------------------
// IPolicyConfig (undocumented; Windows 7+ layout). Called through its vtable:
// IUnknown (3) + GetMixFormat, GetDeviceFormat, ResetDeviceFormat,
// SetDeviceFormat, GetProcessingPeriod, SetProcessingPeriod, GetShareMode,
// SetShareMode, GetPropertyValue, SetPropertyValue, SetDefaultEndpoint (13).
// ---------------------------------------------------------------------------

const CLSID_POLICY_CONFIG_CLIENT: GUID = GUID::from_u128(0x870af99c_171d_4f9e_af0d_e63df40c2bc9);
const IID_POLICY_CONFIG: GUID = GUID::from_u128(0xf8679f50_850a_41cf_9c72_430f290290c8);
const SET_DEFAULT_ENDPOINT_SLOT: usize = 13;

type SetDefaultEndpointFn = unsafe extern "system" fn(this: *mut core::ffi::c_void, id: PCWSTR, role: ERole) -> HRESULT;

fn policy_config_set_default(id: &str) -> Result<()> {
    unsafe {
        let unknown: IUnknown = CoCreateInstance(&CLSID_POLICY_CONFIG_CLIENT, None, CLSCTX_INPROC_SERVER)?;
        let mut raw: *mut core::ffi::c_void = std::ptr::null_mut();
        unknown.query(&IID_POLICY_CONFIG, &mut raw).ok()?;
        if raw.is_null() {
            return Err(windows::Win32::Foundation::E_NOINTERFACE.into());
        }
        // Take ownership of the reference QueryInterface added.
        let policy = IUnknown::from_raw(raw);
        let vtable = *(policy.as_raw() as *const *const usize);
        let f: SetDefaultEndpointFn = std::mem::transmute(*vtable.add(SET_DEFAULT_ENDPOINT_SLOT));
        let wide: Vec<u16> = id.encode_utf16().chain(std::iter::once(0)).collect();
        for role in [eConsole, eMultimedia, eCommunications] {
            f(policy.as_raw(), PCWSTR(wide.as_ptr()), role).ok()?;
        }
        Ok(())
    }
}
