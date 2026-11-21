//! System monitor thread (PLAN.md section 6, thread 2, MTA).
//!
//! Owns the WinRT objects whose APIs are asynchronous:
//! * Bluetooth radio state (`Windows.Devices.Radios`), via `StateChanged`.
//! * Media sessions (`GlobalSystemMediaTransportControlsSessionManager`), via
//!   sessions / current-session / media-properties / playback-info events.
//!
//! WinRT event handlers run on thread-pool threads; they only send a request to
//! this thread's channel. The thread does the (blocking `join`) WinRT calls, decodes
//! album art with WIC only when the track changes, and sends snapshots to the
//! UI with one `PostMessageW(WM_SHELL_MONITOR)`. It blocks on its channel and
//! never wakes on a timer.

use std::collections::HashMap;
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::{Arc, Mutex};
use std::thread;

use windows::Devices::Radios::{Radio, RadioKind, RadioState};
use windows::Foundation::TypedEventHandler;
use windows::Media::Control::{
    GlobalSystemMediaTransportControlsSession as Session, GlobalSystemMediaTransportControlsSessionManager as Manager,
    GlobalSystemMediaTransportControlsSessionPlaybackStatus as Status,
};
use windows::Win32::Foundation::{HWND, LPARAM, WPARAM};
use windows::Win32::Graphics::Imaging::*;
use windows::Win32::System::Com::{
    CLSCTX_INPROC_SERVER, COINIT_MULTITHREADED, CoCreateInstance, CoInitializeEx, CoUninitialize, IStream,
};
use windows::Win32::System::WinRT::CreateStreamOverRandomAccessStream;
use windows::Win32::UI::WindowsAndMessaging::{PostMessageW, WM_APP};
use windows::core::Result;

use crate::icon::Bgra;
use crate::util::dominant_tint;

pub const WM_SHELL_MONITOR: u32 = WM_APP + 0x26;

/// Per-app art cache entry: (title, artist, art, tint).
type ArtEntry = (String, String, Option<Arc<Bgra>>, Option<u32>);

/// Album art is decoded once at this size; the card (52 px) and the dock pill
/// (36 px) draw it scaled down, crisp up to 200% display scale.
const ART_SIZE: u32 = 104;

#[derive(Clone, Debug)]
pub struct MediaSession {
    pub app_id: String,
    pub title: String,
    pub artist: String,
    pub playing: bool,
    pub can_play_pause: bool,
    pub can_next: bool,
    pub can_previous: bool,
    pub is_current: bool,
    /// Premultiplied BGRA album art, shared so snapshots stay cheap to clone.
    pub art: Option<Arc<Bgra>>,
    /// Dominant colour of the art, 0xRRGGBB.
    pub tint: Option<u32>,
}

pub enum MonitorEvent {
    /// `None` = no Bluetooth radio (hide the pill).
    Bluetooth(Option<bool>),
    /// All sessions; the current one first.
    Media(Vec<MediaSession>),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MediaCommand {
    PlayPause,
    Next,
    Previous,
}

enum Request {
    Bluetooth,
    Media,
    Command(String, MediaCommand),
}

pub struct SystemMonitor {
    tx: Option<Sender<Request>>,
    rx: Receiver<MonitorEvent>,
}

impl SystemMonitor {
    pub fn start(notify: HWND) -> Self {
        let (req_tx, req_rx) = mpsc::channel::<Request>();
        let (ev_tx, ev_rx) = mpsc::channel::<MonitorEvent>();
        let target = notify.0 as isize;
        let loop_tx = req_tx.clone();
        let spawned = thread::Builder::new().name("system-monitor".into()).spawn(move || {
            unsafe {
                let _ = CoInitializeEx(None, COINIT_MULTITHREADED);
            }
            let mut w = Worker::new(loop_tx, ev_tx, target);
            w.setup();
            while let Ok(first) = req_rx.recv() {
                // Coalesce bursts (media apps fire several events per track change).
                let mut reqs = vec![first];
                reqs.extend(req_rx.try_iter());
                let (mut bt, mut media) = (false, false);
                for r in reqs {
                    match r {
                        Request::Bluetooth => bt = true,
                        Request::Media => media = true,
                        Request::Command(app, cmd) => w.command(&app, cmd),
                    }
                }
                if bt {
                    w.send(MonitorEvent::Bluetooth(w.bluetooth()));
                }
                if media {
                    w.resubscribe_sessions();
                    let snapshot = w.media();
                    w.send(MonitorEvent::Media(snapshot));
                }
            }
            drop(w);
            unsafe { CoUninitialize() };
        });
        if let Err(e) = spawned {
            crate::log!("WARNING: system monitor thread failed to start: {e}");
        }
        Self { tx: Some(req_tx), rx: ev_rx }
    }

    pub fn drain(&self) -> Vec<MonitorEvent> {
        self.rx.try_iter().collect()
    }

    pub fn media_command(&self, app_id: &str, cmd: MediaCommand) {
        if let Some(tx) = &self.tx {
            let _ = tx.send(Request::Command(app_id.to_string(), cmd));
        }
    }

    /// Ask for fresh snapshots (e.g. after resume).
    pub fn refresh(&self) {
        if let Some(tx) = &self.tx {
            let _ = tx.send(Request::Bluetooth);
            let _ = tx.send(Request::Media);
        }
    }

    /// Close the request channel. Event handlers still hold senders, so the
    /// thread is not joined; it ends with the process.
    pub fn shutdown(&mut self) {
        self.tx = None;
    }
}

struct Worker {
    req: Arc<Mutex<Sender<Request>>>,
    events: Sender<MonitorEvent>,
    target: isize,
    radios: Vec<(Radio, i64)>,
    manager: Option<Manager>,
    sessions: Vec<(Session, i64, i64)>,
    /// app id -> (title, artist, art, tint): art is decoded only when the track changes.
    art_cache: HashMap<String, ArtEntry>,
    wic: Option<IWICImagingFactory>,
}

impl Worker {
    fn new(req: Sender<Request>, events: Sender<MonitorEvent>, target: isize) -> Self {
        Self {
            req: Arc::new(Mutex::new(req)),
            events,
            target,
            radios: Vec::new(),
            manager: None,
            sessions: Vec::new(),
            art_cache: HashMap::new(),
            wic: None,
        }
    }

    fn send(&self, ev: MonitorEvent) {
        if self.events.send(ev).is_ok() {
            unsafe {
                let _ = PostMessageW(Some(HWND(self.target as _)), WM_SHELL_MONITOR, WPARAM(0), LPARAM(0));
            }
        }
    }

    fn requester(&self, r: fn() -> Request) -> impl Fn() + Send + 'static {
        let tx = self.req.clone();
        move || {
            if let Ok(t) = tx.lock() {
                let _ = t.send(r());
            }
        }
    }

    fn setup(&mut self) {
        // Bluetooth radios.
        match Radio::GetRadiosAsync().and_then(|op| op.join()) {
            Ok(list) => {
                for radio in list {
                    if radio.Kind().ok() != Some(RadioKind::Bluetooth) {
                        continue;
                    }
                    let fire = self.requester(|| Request::Bluetooth);
                    let token = radio
                        .StateChanged(&TypedEventHandler::new(move |_, _| {
                            fire();
                            Ok(())
                        }))
                        .unwrap_or(0);
                    self.radios.push((radio, token));
                }
            }
            Err(e) => crate::log!("monitor: radios unavailable ({e})"),
        }
        self.send(MonitorEvent::Bluetooth(self.bluetooth()));

        // Media sessions.
        match Manager::RequestAsync().and_then(|op| op.join()) {
            Ok(m) => {
                let a = self.requester(|| Request::Media);
                let b = self.requester(|| Request::Media);
                let _ = m.SessionsChanged(&TypedEventHandler::new(move |_, _| {
                    a();
                    Ok(())
                }));
                let _ = m.CurrentSessionChanged(&TypedEventHandler::new(move |_, _| {
                    b();
                    Ok(())
                }));
                self.manager = Some(m);
            }
            Err(e) => crate::log!("monitor: media sessions unavailable ({e})"),
        }
        self.resubscribe_sessions();
        let snapshot = self.media();
        self.send(MonitorEvent::Media(snapshot));
    }

    fn bluetooth(&self) -> Option<bool> {
        if self.radios.is_empty() {
            return None;
        }
        Some(self.radios.iter().any(|(r, _)| r.State().ok() == Some(RadioState::On)))
    }

    /// Re-attach property/playback handlers to the current set of sessions.
    fn resubscribe_sessions(&mut self) {
        for (s, t1, t2) in self.sessions.drain(..) {
            let _ = s.RemoveMediaPropertiesChanged(t1);
            let _ = s.RemovePlaybackInfoChanged(t2);
        }
        let Some(m) = &self.manager else { return };
        let Ok(list) = m.GetSessions() else { return };
        let mut subs = Vec::new();
        for s in list {
            let a = self.requester(|| Request::Media);
            let b = self.requester(|| Request::Media);
            let t1 = s
                .MediaPropertiesChanged(&TypedEventHandler::new(move |_, _| {
                    a();
                    Ok(())
                }))
                .unwrap_or(0);
            let t2 = s
                .PlaybackInfoChanged(&TypedEventHandler::new(move |_, _| {
                    b();
                    Ok(())
                }))
                .unwrap_or(0);
            subs.push((s, t1, t2));
        }
        self.sessions = subs;
    }

    fn media(&mut self) -> Vec<MediaSession> {
        let Some(m) = self.manager.clone() else { return Vec::new() };
        let current = m.GetCurrentSession().ok().and_then(|s| s.SourceAppUserModelId().ok()).map(|h| h.to_string());
        let sessions: Vec<Session> = self.sessions.iter().map(|(s, _, _)| s.clone()).collect();
        let mut out = Vec::new();
        let mut live_ids = Vec::new();
        for s in sessions {
            match self.snapshot(&s, current.as_deref()) {
                Ok(snap) => {
                    live_ids.push(snap.app_id.clone());
                    out.push(snap);
                }
                Err(e) => crate::log!("monitor: media session read failed ({e})"),
            }
        }
        self.art_cache.retain(|k, _| live_ids.contains(k));
        out.sort_by_key(|s| !s.is_current);
        out
    }

    fn snapshot(&mut self, s: &Session, current: Option<&str>) -> Result<MediaSession> {
        let app_id = s.SourceAppUserModelId()?.to_string();
        let props = s.TryGetMediaPropertiesAsync()?.join()?;
        let title = props.Title().map(|h| h.to_string()).unwrap_or_default();
        let artist = props.Artist().map(|h| h.to_string()).unwrap_or_default();
        let info = s.GetPlaybackInfo()?;
        let playing = info.PlaybackStatus().ok() == Some(Status::Playing);
        let controls = info.Controls()?;

        let cached = self.art_cache.get(&app_id).filter(|c| c.0 == title && c.1 == artist).map(|c| (c.2.clone(), c.3));
        let (art, tint) = match cached {
            Some(c) => c,
            None => {
                let art = props.Thumbnail().ok().and_then(|r| self.decode_thumbnail(&r)).map(Arc::new);
                let tint = art.as_ref().and_then(|a| dominant_tint(&a.pixels));
                self.art_cache.insert(app_id.clone(), (title.clone(), artist.clone(), art.clone(), tint));
                (art, tint)
            }
        };

        Ok(MediaSession {
            is_current: current == Some(app_id.as_str()),
            app_id,
            title,
            artist,
            playing,
            can_play_pause: controls.IsPlayPauseToggleEnabled().unwrap_or(false),
            can_next: controls.IsNextEnabled().unwrap_or(false),
            can_previous: controls.IsPreviousEnabled().unwrap_or(false),
            art,
            tint,
        })
    }

    /// Thumbnail stream -> WIC decode -> scale -> premultiplied BGRA.
    fn decode_thumbnail(&mut self, r: &windows::Storage::Streams::IRandomAccessStreamReference) -> Option<Bgra> {
        let stream = r.OpenReadAsync().ok()?.join().ok()?;
        unsafe {
            if self.wic.is_none() {
                self.wic = CoCreateInstance(&CLSID_WICImagingFactory, None, CLSCTX_INPROC_SERVER).ok();
            }
            let wic = self.wic.as_ref()?;
            let istream: IStream = CreateStreamOverRandomAccessStream(&stream).ok()?;
            let decoder =
                wic.CreateDecoderFromStream(&istream, std::ptr::null(), WICDecodeMetadataCacheOnDemand).ok()?;
            let frame = decoder.GetFrame(0).ok()?;
            let scaler = wic.CreateBitmapScaler().ok()?;
            scaler.Initialize(&frame, ART_SIZE, ART_SIZE, WICBitmapInterpolationModeHighQualityCubic).ok()?;
            let conv = wic.CreateFormatConverter().ok()?;
            conv.Initialize(
                &scaler,
                &GUID_WICPixelFormat32bppPBGRA,
                WICBitmapDitherTypeNone,
                None,
                0.0,
                WICBitmapPaletteTypeCustom,
            )
            .ok()?;
            let stride = ART_SIZE * 4;
            let mut px = vec![0u8; (stride * ART_SIZE) as usize];
            conv.CopyPixels(std::ptr::null(), stride, &mut px).ok()?;
            Some(Bgra { width: ART_SIZE, height: ART_SIZE, pixels: px })
        }
    }

    fn command(&mut self, app_id: &str, cmd: MediaCommand) {
        let Some((s, _, _)) = self
            .sessions
            .iter()
            .find(|(s, _, _)| s.SourceAppUserModelId().map(|h| h.to_string()).ok().as_deref() == Some(app_id))
        else {
            return;
        };
        let r = match cmd {
            MediaCommand::PlayPause => s.TryTogglePlayPauseAsync().and_then(|o| o.join()),
            MediaCommand::Next => s.TrySkipNextAsync().and_then(|o| o.join()),
            MediaCommand::Previous => s.TrySkipPreviousAsync().and_then(|o| o.join()),
        };
        if let Err(e) = r {
            crate::log!("monitor: media command {cmd:?} failed ({e})");
        }
    }
}

impl Drop for Worker {
    fn drop(&mut self) {
        for (r, t) in self.radios.drain(..) {
            let _ = r.RemoveStateChanged(t);
        }
        for (s, t1, t2) in self.sessions.drain(..) {
            let _ = s.RemoveMediaPropertiesChanged(t1);
            let _ = s.RemovePlaybackInfoChanged(t2);
        }
    }
}
