//! Flyout panels (PLAN.md sections 2 and 4, M7): Sound (volume, microphone,
//! output devices, media sessions), Battery, Calendar, and the dock's
//! multi-window list (M4). One reusable flyout window.
//!
//! Each panel is built into a small display list (`Item`s plus hit rects) so
//! drawing and hit-testing always agree. Opening slides the content 8 px and
//! fades it in over 140 ms with DirectComposition animations (no CPU frames).

use windows::Win32::Foundation::HWND;
use windows::Win32::Graphics::Direct2D::Common::D2D_RECT_F;
use windows::Win32::Graphics::Direct2D::{ID2D1Bitmap1, ID2D1DeviceContext};
use windows::Win32::Graphics::DirectWrite::*;
use windows::Win32::UI::Input::KeyboardAndMouse::{ReleaseCapture, SetCapture};
use windows::core::Result;

use winshell::actions;
use winshell::calendar;
use winshell::clock;
use winshell::log;
use winshell::render::{self, rectf};
use winshell::sysmon::{MediaCommand, RadioSel};
use winshell::util::darken;
use winshell::win::{self, px};

use super::App;
use super::bar::{battery_glyph, volume_glyph};
use super::dock::group_name;
use super::theme::*;

const MAX_MEDIA_CARDS: usize = 3;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PanelKind {
    Appearance,
    Controls,
    Sound,
    Battery,
    Calendar,
    Wifi,
    Bluetooth,
    /// Window list for the dock group with this id.
    WindowList(String),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SliderKind {
    Volume,
    Mic,
}

#[derive(Clone, Debug, PartialEq)]
enum PanelHit {
    Shell(u32),
    Panel(PanelKind),
    Desktop,
    Activate(isize, bool),
    NewWindow(String),
    ToggleMute,
    ToggleMicMute,
    Slider(SliderKind),
    Device(String),
    Media(String, MediaCommand),
    Link(&'static str),
    CalPrev,
    CalNext,
    CalToday,
    /// Turn a radio on (true) or off.
    Radio(RadioSel, bool),
    /// Connect to a saved Wi-Fi profile.
    WifiConnect(String),
    WifiDisconnect,
    /// A network Windows has no profile for: the native picker asks for the password.
    WifiPicker,
}

#[derive(Clone, Copy)]
enum Font {
    Header,
    Body,
    BodyBold,
    Sub,
    Pill,
    Big,
    Icon,
}

#[derive(Clone, Copy)]
enum Align {
    Leading,
    Center,
    Trailing,
}

enum Item {
    Fill(D2D_RECT_F, f32, u32),
    Stroke(D2D_RECT_F, f32, u32),
    Text(String, Font, D2D_RECT_F, u32, Align),
    Glyph(char, Font, D2D_RECT_F, u32),
    Bitmap(ID2D1Bitmap1, D2D_RECT_F),
}

pub struct PanelState {
    pub kind: PanelKind,
    items: Vec<Item>,
    /// (action, rect, draw hover highlight)
    hits: Vec<(PanelHit, D2D_RECT_F, bool)>,
    hover: Option<usize>,
    /// Slider being dragged: kind and track x-range.
    drag: Option<(SliderKind, f32, f32)>,
    /// Month shown by the calendar.
    cal: (i32, u32),
    height: f32,
    width: f32,
    scroll: f32,
    content_height: f32,
}

struct Builder {
    s: f32,
    w: f32,
    y: f32,
    items: Vec<Item>,
    hits: Vec<(PanelHit, D2D_RECT_F, bool)>,
}

impl Builder {
    fn pad(&self) -> f32 {
        FLYOUT_PAD * self.s
    }
    fn inner(&self) -> (f32, f32) {
        (self.pad(), self.w - self.pad())
    }
    fn fill(&mut self, r: D2D_RECT_F, radius: f32, c: u32) {
        self.items.push(Item::Fill(r, radius, c));
    }
    fn text(&mut self, t: impl Into<String>, f: Font, r: D2D_RECT_F, c: u32, a: Align) {
        self.items.push(Item::Text(t.into(), f, r, c, a));
    }
    fn glyph(&mut self, g: char, r: D2D_RECT_F, c: u32) {
        self.items.push(Item::Glyph(g, Font::Icon, r, c));
    }
    fn hit(&mut self, h: PanelHit, r: D2D_RECT_F, hover: bool) {
        self.hits.push((h, r, hover));
    }
    fn header(&mut self, t: &str) {
        let (l, r) = self.inner();
        let s = self.s;
        self.text(t, Font::Header, rectf(l, self.y, r - l, 24.0 * s), pal().text_primary, Align::Leading);
        self.y += 24.0 * s + 8.0 * s;
    }
    fn toggle(&mut self, label: &str, on: bool, action: PanelHit) {
        let (l, r) = self.inner();
        let s = self.s;
        let row = rectf(l, self.y, r - l, 38.0 * s);
        self.text(
            label,
            Font::Body,
            rectf(l + 6.0 * s, self.y, r - l - 58.0 * s, 38.0 * s),
            pal().text_primary,
            Align::Leading,
        );
        let track = rectf(r - 42.0 * s, self.y + 9.0 * s, 36.0 * s, 20.0 * s);
        self.fill(track, 10.0 * s, if on { pal().accent } else { pal().slider_track });
        let knob = rectf(
            if on { track.right - 17.0 * s } else { track.left + 3.0 * s },
            track.top + 3.0 * s,
            14.0 * s,
            14.0 * s,
        );
        self.fill(knob, 7.0 * s, if on { pal().on_accent } else { pal().text_primary });
        self.hit(action, row, true);
        self.y += 40.0 * s;
    }

    /// Header with an on/off switch at the right (like the native quick settings tiles).
    fn header_toggle(&mut self, t: &str, state: Option<bool>, sel: RadioSel) {
        let (l, r) = self.inner();
        let s = self.s;
        let y = self.y;
        self.text(t, Font::Header, rectf(l, y, r - l - 56.0 * s, 24.0 * s), pal().text_primary, Align::Leading);
        if let Some(on) = state {
            let (w, h) = (40.0 * s, 20.0 * s);
            let track = rectf(r - w, y + 2.0 * s, w, h);
            self.fill(track, h / 2.0, if on { pal().accent } else { pal().slider_track });
            let k = h - 6.0 * s;
            let kx = if on { track.right - 3.0 * s - k } else { track.left + 3.0 * s };
            self.fill(
                rectf(kx, track.top + 3.0 * s, k, k),
                k / 2.0,
                if on { pal().on_accent } else { pal().text_primary },
            );
            self.hit(PanelHit::Radio(sel, !on), rectf(track.left - 6.0 * s, y, w + 12.0 * s, 24.0 * s), true);
        }
        self.y += 24.0 * s + 8.0 * s;
    }
    /// A plain message line.
    fn note(&mut self, t: &str) {
        let (l, r) = self.inner();
        let s = self.s;
        self.text(t, Font::Body, rectf(l, self.y, r - l, 28.0 * s), pal().text_secondary, Align::Leading);
        self.y += 28.0 * s;
    }
    fn divider(&mut self) {
        let (l, r) = self.inner();
        self.y += 6.0 * self.s;
        self.fill(rectf(l, self.y.round(), r - l, 1.0), 0.0, pal().border);
        self.y += 7.0 * self.s;
    }
    fn link(&mut self, label: &str, uri: &'static str) {
        let (l, r) = self.inner();
        let s = self.s;
        let row = rectf(l, self.y, r - l, 28.0 * s);
        self.text(
            label,
            Font::Pill,
            D2D_RECT_F { left: row.left + 6.0 * s, right: row.right - 6.0 * s, ..row },
            pal().accent,
            Align::Trailing,
        );
        self.hit(PanelHit::Link(uri), row, true);
        self.y += 28.0 * s;
    }
    /// Icon button + slider + numeric label (32 px row).
    fn slider_row(&mut self, kind: SliderKind, icon: char, icon_color: u32, value: f32, toggle: PanelHit) {
        let (l, r) = self.inner();
        let s = self.s;
        let h = 32.0 * s;
        let icon_r = rectf(l, self.y, 28.0 * s, h);
        self.glyph(icon, icon_r, icon_color);
        self.hit(toggle, icon_r, true);
        let label_w = 36.0 * s;
        let track_l = icon_r.right + 10.0 * s;
        let track_r = r - label_w - 8.0 * s;
        let cy = self.y + h / 2.0;
        let th = 4.0 * s;
        self.fill(rectf(track_l, cy - th / 2.0, track_r - track_l, th), th / 2.0, pal().slider_track);
        let vx = track_l + (track_r - track_l) * value.clamp(0.0, 1.0);
        self.fill(rectf(track_l, cy - th / 2.0, (vx - track_l).max(th), th), th / 2.0, pal().accent);
        let k = 16.0 * s;
        self.fill(rectf(vx - k / 2.0, cy - k / 2.0, k, k), k / 2.0, pal().text_primary);
        self.fill(rectf(vx - k / 4.0, cy - k / 4.0, k / 2.0, k / 2.0), k / 4.0, pal().accent);
        self.hit(PanelHit::Slider(kind), rectf(track_l - k / 2.0, self.y, track_r - track_l + k, h), false);
        let pct = (value.clamp(0.0, 1.0) * 100.0).round() as u32;
        self.text(
            pct.to_string(),
            Font::Body,
            rectf(r - label_w, self.y, label_w, h),
            pal().text_primary,
            Align::Trailing,
        );
        self.y += h;
    }
}

impl App {
    pub fn panel_is(&self, kind: PanelKind) -> bool {
        self.panel.as_ref().is_some_and(|p| p.kind == kind)
    }

    pub fn toggle_panel(&mut self, kind: PanelKind) {
        if self.panel_is(kind.clone()) {
            self.close_panel();
        } else {
            self.open_panel(kind);
        }
    }

    pub fn open_panel(&mut self, kind: PanelKind) {
        self.hide_tooltip();
        if kind == PanelKind::Sound
            && let Some(a) = &self.audio
        {
            self.devices = a.render_devices();
        }
        if kind == PanelKind::Wifi {
            self.network.set_scan_wanted(true);
            self.network.scan();
            self.wifi_networks = self.network.available_networks();
        } else if self.panel_is(PanelKind::Wifi) {
            self.network.set_scan_wanted(false);
        }
        if kind == PanelKind::Bluetooth {
            self.sysmon.refresh_bt_devices();
        }
        let name = match &kind {
            PanelKind::Appearance => "appearance",
            PanelKind::Controls => "controls",
            PanelKind::Sound => "sound",
            PanelKind::Battery => "battery",
            PanelKind::Calendar => "calendar",
            PanelKind::Wifi => "wifi",
            PanelKind::Bluetooth => "bluetooth",
            PanelKind::WindowList(_) => "window list",
        };
        log!("flyout: open {name}");
        let (y, m, _) = clock::today();
        let was_open = self.panel.is_some();
        self.panel = Some(PanelState {
            kind,
            items: Vec::new(),
            hits: Vec::new(),
            hover: None,
            drag: None,
            cal: (y, m),
            height: 0.0,
            width: 0.0,
            scroll: 0.0,
            content_height: 0.0,
        });
        self.show_panel(!was_open);
    }

    pub fn close_panel(&mut self) {
        let Some(p) = self.panel.take() else { return };
        if p.drag.is_some() {
            unsafe {
                let _ = ReleaseCapture();
            }
        }
        if p.kind == PanelKind::Wifi {
            self.network.set_scan_wanted(false);
        }
        self.flyout.hide();
        // A bar/dock revealed from the edge must not stay up forever after the flyout.
        if self.reveal_bar || self.reveal_dock {
            unsafe {
                windows::Win32::UI::WindowsAndMessaging::SetTimer(
                    Some(self.top_bar.hwnd),
                    super::TIMER_AUTOHIDE_BAR,
                    500,
                    None,
                );
                windows::Win32::UI::WindowsAndMessaging::SetTimer(
                    Some(self.top_bar.hwnd),
                    super::TIMER_AUTOHIDE_DOCK,
                    500,
                    None,
                );
            }
        }
        self.update_visibility();
    }

    /// Rebuild an open panel after its data changed (no animation).
    pub fn refresh_panel_if(&mut self, kind: PanelKind) {
        if self.panel_is(kind) {
            self.show_panel(false);
        }
    }

    fn build_panel(&self, p: &PanelState) -> Builder {
        let s = self.scale();
        let w = match p.kind {
            PanelKind::WindowList(_) => WINDOW_LIST_W,
            _ => FLYOUT_W,
        } * s;
        let mut b = Builder { s, w: w.round(), y: FLYOUT_PAD * s, items: Vec::new(), hits: Vec::new() };
        match &p.kind {
            PanelKind::Appearance => self.build_appearance(&mut b),
            PanelKind::Controls => self.build_controls(&mut b),
            PanelKind::Sound => self.build_sound(&mut b),
            PanelKind::Battery => self.build_battery(&mut b),
            PanelKind::Calendar => self.build_calendar(&mut b, p.cal),
            PanelKind::Wifi => self.build_wifi(&mut b),
            PanelKind::Bluetooth => self.build_bluetooth(&mut b),
            PanelKind::WindowList(id) => self.build_window_list(&mut b, id),
        }
        b.y += FLYOUT_PAD * s;
        b
    }

    fn build_appearance(&self, b: &mut Builder) {
        let s = b.s;
        b.header("Make it yours");
        b.note("Slim glass. More room for your apps.");
        b.divider();
        for (cmd, name, id, color) in [
            (10, "Midnight", "midnight", 0x98C8FFFF),
            (11, "Pearl", "pearl", 0xEDF2F8FF),
            (12, "Dark Glass", "dark_glass", 0x6EA8FEFF),
            (13, "Light Clean", "light_clean", 0x2563EBFF),
            (14, "Follow Windows", "auto_system", pal().text_secondary),
        ] {
            let (l, r) = b.inner();
            let row = rectf(l, b.y, r - l, 36.0 * s);
            let selected = self.config.general.theme.eq_ignore_ascii_case(id)
                || (id == "auto_system" && self.config.general.follow_windows_theme);
            if selected {
                b.fill(row, 8.0 * s, pal().pill_hover);
            }
            b.fill(rectf(l + 8.0 * s, b.y + 11.0 * s, 14.0 * s, 14.0 * s), 7.0 * s, color);
            b.text(
                name,
                Font::Body,
                rectf(l + 32.0 * s, b.y, r - l - 64.0 * s, 36.0 * s),
                pal().text_primary,
                Align::Leading,
            );
            if selected {
                b.glyph('\u{E73E}', rectf(r - 28.0 * s, b.y, 24.0 * s, 36.0 * s), pal().accent);
            }
            b.hit(PanelHit::Shell(cmd), row, true);
            b.y += 38.0 * s;
        }
        b.divider();
        b.header("Desktop & windows");
        for (cmd, label, on) in [
            (20, "Compact floating bar", self.config.top_bar.compact),
            (21, "Keep desktop icons clear", self.config.general.desktop_autohide),
            (22, "Auto-hide top bar over apps", self.config.top_bar.autohide),
            (23, "Auto-hide left dock over apps", self.config.dock.autohide),
            (24, "Magnify hovered icons", self.config.dock.magnification),
            (16, "Immersive app launches", self.config.dock.immersive),
            (18, "Hide Windows taskbars", self.config.taskbar.hide_native),
        ] {
            b.toggle(label, on, PanelHit::Shell(cmd));
        }
        b.divider();
        for (cmd, label) in [(15, "Open settings file"), (17, "Quit and restore Windows")] {
            let (l, r) = b.inner();
            let row = rectf(l, b.y, r - l, 32.0 * s);
            b.text(label, Font::Body, D2D_RECT_F { left: l + 8.0 * s, ..row }, pal().text_primary, Align::Leading);
            b.hit(PanelHit::Shell(cmd), row, true);
            b.y += 34.0 * s;
        }
        b.note("Restore a window: Ctrl + Shift + Alt + F");
    }

    fn build_controls(&self, b: &mut Builder) {
        let s = b.s;
        b.header("Your desktop");
        b.note(&format!("{} running apps", self.dock_model.groups().iter().filter(|g| g.is_running()).count()));
        for (label, glyph, kind) in [
            ("Appearance & behavior", glyph::SETTINGS, PanelKind::Appearance),
            ("Sound & media", glyph::VOLUME_2, PanelKind::Sound),
            ("Wi-Fi networks", glyph::WIFI, PanelKind::Wifi),
            ("Bluetooth devices", glyph::BLUETOOTH, PanelKind::Bluetooth),
            ("Power & battery", glyph::BATTERY_FULL, PanelKind::Battery),
            ("Calendar", glyph::BELL, PanelKind::Calendar),
        ] {
            let (l, r) = b.inner();
            let row = rectf(l, b.y, r - l, 38.0 * s);
            b.glyph(glyph, rectf(l + 4.0 * s, b.y, 28.0 * s, 38.0 * s), pal().accent);
            b.text(
                label,
                Font::Body,
                rectf(l + 40.0 * s, b.y, r - l - 44.0 * s, 38.0 * s),
                pal().text_primary,
                Align::Leading,
            );
            b.hit(PanelHit::Panel(kind), row, true);
            b.y += 40.0 * s;
        }
        b.divider();
        let (l, r) = b.inner();
        let row = rectf(l, b.y, r - l, 34.0 * s);
        b.text("Show desktop", Font::Body, row, pal().accent, Align::Leading);
        b.hit(PanelHit::Desktop, row, true);
        b.y += 36.0 * s;
        b.link("Wallpaper & personalization", "ms-settings:personalization-background");
        b.link("Display settings", "ms-settings:display");
        b.link("Windows settings", "ms-settings:");
    }

    fn build_sound(&self, b: &mut Builder) {
        let s = b.s;
        let a = self.audio_state;
        b.header("Sound");
        if self.audio.is_some() && a.available {
            let c = if a.muted { pal().text_secondary } else { pal().text_primary };
            b.slider_row(SliderKind::Volume, volume_glyph(a.volume, a.muted), c, a.volume, PanelHit::ToggleMute);
            if a.mic_available {
                let c = if a.mic_muted { pal().danger } else { pal().text_primary };
                b.slider_row(SliderKind::Mic, glyph::MIC, c, a.mic_volume, PanelHit::ToggleMicMute);
            }
            b.link("Volume mixer", "ms-settings:apps-volume");
        } else {
            let (l, r) = b.inner();
            b.text(
                "No audio output device",
                Font::Body,
                rectf(l, b.y, r - l, 28.0 * s),
                pal().text_secondary,
                Align::Leading,
            );
            b.y += 28.0 * s;
        }

        if !self.devices.is_empty() {
            b.divider();
            let (l, r) = b.inner();
            b.text("Output device", Font::Sub, rectf(l, b.y, r - l, 20.0 * s), pal().text_secondary, Align::Leading);
            b.y += 22.0 * s;
            for d in &self.devices {
                let row = rectf(l, b.y, r - l, 32.0 * s);
                let k = 14.0 * s;
                let dot = rectf(l + 6.0 * s, b.y + (32.0 * s - k) / 2.0, k, k);
                if d.is_default {
                    b.fill(dot, k / 2.0, pal().accent);
                    b.fill(rectf(dot.left + k * 0.3, dot.top + k * 0.3, k * 0.4, k * 0.4), k * 0.2, pal().on_accent);
                } else {
                    b.items.push(Item::Stroke(dot, k / 2.0, pal().text_secondary));
                }
                let tl = dot.right + 10.0 * s;
                b.text(
                    &d.name,
                    Font::Body,
                    rectf(tl, b.y, r - tl - 6.0 * s, 32.0 * s),
                    pal().text_primary,
                    Align::Leading,
                );
                b.hit(PanelHit::Device(d.id.clone()), row, true);
                b.y += 32.0 * s;
            }
        }

        for m in self.media.iter().take(MAX_MEDIA_CARDS) {
            b.y += 8.0 * s;
            let (l, r) = b.inner();
            let card = rectf(l, b.y, r - l, 92.0 * s);
            let bg = m.tint.map(|t| (darken(t, 0.38) << 8) | 0xE6).unwrap_or(pal().pill_bg);
            b.fill(card, 10.0 * s, bg);
            let art = rectf(card.left + 10.0 * s, card.top + 10.0 * s, 52.0 * s, 52.0 * s);
            match self.media_bitmap(&m.app_id) {
                Some(bmp) => b.items.push(Item::Bitmap(bmp.clone(), art)),
                None => {
                    b.fill(art, 6.0 * s, pal().pill_hover);
                    b.glyph(glyph::MUSIC, art, pal().text_secondary);
                }
            }
            let btn = 24.0 * s;
            let buttons_w = 3.0 * btn + 2.0 * 4.0 * s;
            let tx = art.right + 10.0 * s;
            let tw = (card.right - 10.0 * s - tx).max(0.0);
            let title = if m.title.is_empty() { "Unknown title" } else { m.title.as_str() };
            b.text(
                title,
                Font::BodyBold,
                rectf(tx, card.top + 8.0 * s, tw, 22.0 * s),
                pal().text_primary,
                Align::Leading,
            );
            b.text(
                &m.artist,
                Font::Sub,
                rectf(tx, card.top + 30.0 * s, tw, 20.0 * s),
                pal().text_secondary,
                Align::Leading,
            );
            let by = card.top + 56.0 * s;
            let mut bx = tx + (tw - buttons_w) / 2.0;
            for (g, cmd, enabled) in [
                (glyph::PREVIOUS, MediaCommand::Previous, m.can_previous),
                (if m.playing { glyph::PAUSE } else { glyph::PLAY }, MediaCommand::PlayPause, m.can_play_pause),
                (glyph::NEXT, MediaCommand::Next, m.can_next),
            ] {
                let r = rectf(bx, by, btn, btn);
                b.glyph(g, r, if enabled { pal().text_primary } else { pal().text_disabled });
                if enabled {
                    b.hit(PanelHit::Media(m.app_id.clone(), cmd), r, true);
                }
                bx += btn + 4.0 * s;
            }
            b.y += 92.0 * s;
        }
    }

    fn build_battery(&self, b: &mut Builder) {
        let s = b.s;
        let bat = self.battery;
        b.header("Battery");
        let (l, r) = b.inner();
        if !bat.present {
            b.text(
                "No battery detected",
                Font::Body,
                rectf(l, b.y, r - l, 28.0 * s),
                pal().text_secondary,
                Align::Leading,
            );
            b.y += 28.0 * s;
        } else {
            let (g, c) = battery_glyph(bat.percent, bat.charging);
            b.glyph(g, rectf(l, b.y, 32.0 * s, 44.0 * s), c);
            b.text(
                format!("{}%", bat.percent),
                Font::Big,
                rectf(l + 40.0 * s, b.y, r - l - 40.0 * s, 44.0 * s),
                pal().text_primary,
                Align::Leading,
            );
            b.y += 48.0 * s;
            let status = if bat.charging {
                "Charging".to_string()
            } else if bat.on_ac {
                "Plugged in".to_string()
            } else if let Some(secs) = bat.seconds_left {
                let (h, m) = (secs / 3600, (secs % 3600) / 60);
                if h > 0 { format!("On battery · {h} h {m} min left") } else { format!("On battery · {m} min left") }
            } else {
                "On battery".to_string()
            };
            b.text(status, Font::Body, rectf(l, b.y, r - l, 24.0 * s), pal().text_secondary, Align::Leading);
            b.y += 24.0 * s;
            if bat.saver {
                b.items.push(Item::Glyph(
                    glyph::BATTERY_SAVER,
                    Font::Icon,
                    rectf(l, b.y, 20.0 * s, 24.0 * s),
                    pal().warning,
                ));
                b.text(
                    "Energy saver is on",
                    Font::Body,
                    rectf(l + 26.0 * s, b.y, r - l, 24.0 * s),
                    pal().warning,
                    Align::Leading,
                );
                b.y += 24.0 * s;
            }
        }
        b.divider();
        b.link("Power & battery settings", "ms-settings:batterysaver");
    }

    fn build_calendar(&self, b: &mut Builder, (year, month): (i32, u32)) {
        let s = b.s;
        let (ty, tm, td) = clock::today();
        b.header(&clock::format_date(ty, tm, td, "dddd, d MMMM"));
        let (l, r) = b.inner();

        // Month title and navigation.
        let row_h = 28.0 * s;
        let btn = 28.0 * s;
        let title_r = rectf(l, b.y, r - l - 2.0 * btn - 8.0 * s, row_h);
        b.text(
            clock::format_date(year, month, 1, "MMMM yyyy"),
            Font::BodyBold,
            D2D_RECT_F { left: title_r.left + 6.0 * s, ..title_r },
            pal().text_primary,
            Align::Leading,
        );
        if (year, month) != (ty, tm) {
            b.hit(PanelHit::CalToday, title_r, true);
        }
        let prev = rectf(r - 2.0 * btn - 4.0 * s, b.y, btn, row_h);
        let next = rectf(r - btn, b.y, btn, row_h);
        b.glyph(glyph::CHEVRON_LEFT, prev, pal().text_primary);
        b.glyph(glyph::CHEVRON_RIGHT, next, pal().text_primary);
        b.hit(PanelHit::CalPrev, prev, true);
        b.hit(PanelHit::CalNext, next, true);
        b.y += row_h + 6.0 * s;

        // Weekday names in the locale's order (Nov 2026 starts on a Sunday).
        let first = clock::first_weekday();
        let cell_w = (r - l) / 7.0;
        for i in 0..7u32 {
            let wd = (first + i) % 7; // 0 = Sunday
            let name: String = clock::format_date(2026, 11, 1 + wd, "ddd").chars().take(2).collect();
            b.text(
                name,
                Font::Sub,
                rectf(l + i as f32 * cell_w, b.y, cell_w, 20.0 * s),
                pal().text_secondary,
                Align::Center,
            );
        }
        b.y += 24.0 * s;

        let cell_h = 32.0 * s;
        for (i, (y, m, d, in_month)) in calendar::month_grid(year, month, first).into_iter().enumerate() {
            let col = (i % 7) as f32;
            let row = (i / 7) as f32;
            let cell = rectf(l + col * cell_w, b.y + row * cell_h, cell_w, cell_h);
            let is_today = (y, m, d) == (ty, tm, td);
            let color = if is_today {
                pal().on_accent
            } else if in_month {
                pal().text_primary
            } else {
                pal().text_disabled
            };
            if is_today {
                let k = 28.0 * s;
                let cx = (cell.left + cell.right) / 2.0;
                let cy = (cell.top + cell.bottom) / 2.0;
                b.fill(rectf(cx - k / 2.0, cy - k / 2.0, k, k), k / 2.0, pal().accent);
            }
            b.text(d.to_string(), Font::Body, cell, color, Align::Center);
        }
        b.y += 6.0 * cell_h;
        b.divider();
        b.link("Date & time settings", "ms-settings:dateandtime");
    }

    fn build_wifi(&self, b: &mut Builder) {
        use winshell::network::NetState;
        let s = b.s;
        b.header_toggle("Wi-Fi", self.wifi_radio, RadioSel::Wifi);
        if self.net_state == NetState::Ethernet {
            let (l, r) = b.inner();
            b.glyph(glyph::ETHERNET, rectf(l, b.y, 28.0 * s, 32.0 * s), pal().text_primary);
            b.text(
                "Ethernet connected",
                Font::Body,
                rectf(l + 36.0 * s, b.y, r - l - 36.0 * s, 32.0 * s),
                pal().text_primary,
                Align::Leading,
            );
            b.y += 32.0 * s;
        }
        if self.wifi_radio == Some(false) {
            b.note("Wi-Fi is turned off");
        } else if self.wifi_networks.is_empty() {
            b.note("Looking for networks…");
        } else {
            for n in &self.wifi_networks {
                let (l, r) = b.inner();
                let row_h = 44.0 * s;
                let row = rectf(l, b.y, r - l, row_h);
                let g = match n.quality {
                    0..=32 => glyph::WIFI_1,
                    33..=65 => glyph::WIFI_2,
                    66..=89 => glyph::WIFI_3,
                    _ => glyph::WIFI,
                };
                b.glyph(g, rectf(l, b.y, 28.0 * s, row_h), if n.connected { pal().accent } else { pal().text_primary });
                let tx = l + 36.0 * s;
                let action_w = if n.connected { 84.0 * s } else { 0.0 };
                let tw = r - tx - action_w - 6.0 * s;
                b.text(&n.ssid, Font::Body, rectf(tx, b.y + 4.0 * s, tw, 20.0 * s), pal().text_primary, Align::Leading);
                let status = match (n.connected, n.secured) {
                    (true, true) => "Connected, secured",
                    (true, false) => "Connected, open",
                    (false, true) => "Secured",
                    (false, false) => "Open",
                };
                b.text(
                    status,
                    Font::Sub,
                    rectf(tx, b.y + 23.0 * s, tw, 18.0 * s),
                    pal().text_secondary,
                    Align::Leading,
                );
                if n.connected {
                    let btn = rectf(r - action_w, b.y + 8.0 * s, action_w, 28.0 * s);
                    b.text("Disconnect", Font::Pill, btn, pal().accent, Align::Center);
                    b.hit(PanelHit::WifiDisconnect, btn, true);
                } else {
                    let hit = match &n.profile {
                        Some(p) => PanelHit::WifiConnect(p.clone()),
                        None => PanelHit::WifiPicker,
                    };
                    b.hit(hit, row, true);
                }
                b.y += row_h;
            }
        }
        b.divider();
        b.link("Other networks", "ms-availablenetworks:");
        b.link("More Wi-Fi settings", "ms-settings:network-wifi");
    }

    fn build_bluetooth(&self, b: &mut Builder) {
        let s = b.s;
        b.header_toggle("Bluetooth", self.bluetooth, RadioSel::Bluetooth);
        match self.bluetooth {
            None => b.note("No Bluetooth adapter"),
            Some(false) => b.note("Bluetooth is turned off"),
            Some(true) if self.bt_devices.is_empty() => b.note("No paired devices"),
            Some(true) => {
                for d in &self.bt_devices {
                    let (l, r) = b.inner();
                    let row_h = 44.0 * s;
                    b.glyph(
                        glyph::BLUETOOTH,
                        rectf(l, b.y, 28.0 * s, row_h),
                        if d.connected { pal().accent } else { pal().text_primary },
                    );
                    let tx = l + 36.0 * s;
                    b.text(
                        &d.name,
                        Font::Body,
                        rectf(tx, b.y + 4.0 * s, r - tx, 20.0 * s),
                        pal().text_primary,
                        Align::Leading,
                    );
                    let status = if d.connected { "Connected" } else { "Paired" };
                    b.text(
                        status,
                        Font::Sub,
                        rectf(tx, b.y + 23.0 * s, r - tx, 18.0 * s),
                        pal().text_secondary,
                        Align::Leading,
                    );
                    b.y += row_h;
                }
            }
        }
        b.divider();
        b.link("Add a device", "ms-settings:connecteddevices");
        b.link("More Bluetooth settings", "ms-settings:bluetooth");
    }

    fn build_window_list(&self, b: &mut Builder, group_id: &str) {
        let s = b.s;
        let Some(g) = self.dock_model.groups().iter().find(|g| g.id == group_id) else { return };
        b.header(&group_name(g));
        let (l, r) = b.inner();
        let focused = self
            .dock_model
            .focused()
            .and_then(|fi| self.dock_model.groups().get(fi))
            .map(|fg| fg.id == g.id)
            .unwrap_or(false);
        for (i, w) in g.windows.iter().enumerate() {
            let row = rectf(l, b.y, r - l, 32.0 * s);
            if focused && i == g.active_window_idx {
                b.fill(rectf(l, b.y + 8.0 * s, 3.0 * s, 16.0 * s), 1.5 * s, pal().accent);
            }
            let mut tr = r - 6.0 * s;
            if w.is_elevated {
                b.glyph(glyph::ADMIN, rectf(r - 24.0 * s, b.y, 24.0 * s, 32.0 * s), pal().warning);
                tr -= 24.0 * s;
            }
            let title = if w.title.is_empty() { "Untitled window" } else { w.title.as_str() };
            b.text(
                title,
                Font::Body,
                rectf(l + 12.0 * s, b.y, tr - l - 12.0 * s, 32.0 * s),
                pal().text_primary,
                Align::Leading,
            );
            b.hit(PanelHit::Activate(w.hwnd, w.is_elevated), row, true);
            b.y += 32.0 * s;
        }
        b.divider();
        let row = rectf(l, b.y, r - l, 32.0 * s);
        b.text("New window", Font::Body, D2D_RECT_F { left: l + 12.0 * s, ..row }, pal().accent, Align::Leading);
        b.hit(PanelHit::NewWindow(g.id.clone()), row, true);
        b.y += 32.0 * s;
    }

    /// Lay out, place, draw and (optionally) animate the open panel.
    fn show_panel(&mut self, animate: bool) {
        let Some(mut p) = self.panel.take() else { return };
        let built = self.build_panel(&p);
        p.width = built.w;
        p.content_height = built.y.ceil();
        let max_h =
            (self.monitor.monitor.bottom - self.top_bar.rect.bottom - px(FLYOUT_GAP * 2.0, self.scale())).max(1) as f32;
        p.height = p.content_height.min(max_h);
        p.scroll = p.scroll.clamp(0.0, (p.content_height - p.height).max(0.0));
        let shift = |r: &mut D2D_RECT_F| {
            r.top -= p.scroll;
            r.bottom -= p.scroll;
        };
        p.items = built.items;
        for item in &mut p.items {
            match item {
                Item::Fill(r, ..)
                | Item::Stroke(r, ..)
                | Item::Text(_, _, r, ..)
                | Item::Glyph(_, _, r, _)
                | Item::Bitmap(_, r) => shift(r),
            }
        }
        p.hits = built.hits;
        for (_, r, _) in &mut p.hits {
            shift(r);
        }
        if p.hover.is_some_and(|h| h >= p.hits.len()) {
            p.hover = None;
        }

        let s = self.scale();
        let (w, h) = (p.width as i32, p.height as i32);
        let gap = px(FLYOUT_GAP, s);
        let (rect, from_y) = match &p.kind {
            PanelKind::Appearance => {
                (win::rect(self.top_bar.rect.left, self.top_bar.rect.bottom + gap, w, h), -FLYOUT_SLIDE * s)
            }
            PanelKind::WindowList(id) => {
                let gi = self.dock_model.groups().iter().position(|g| &g.id == id);
                let slot = gi.and_then(|gi| self.dock_layout.slot(gi)).unwrap_or_default();
                let x = self.dock.rect.right + gap;
                let screen = self.monitor.monitor;
                let cy = self.dock.rect.top + ((slot.top + slot.bottom) / 2.0) as i32;
                let y = (cy - h / 2).clamp(screen.top + gap, (screen.bottom - gap - h).max(screen.top + gap));
                (win::rect(x, y, w, h), 0.0)
            }
            _ => (
                win::rect(
                    (self.top_bar.rect.right - w).max(self.monitor.monitor.left),
                    self.top_bar.rect.bottom + gap,
                    w,
                    h,
                ),
                -FLYOUT_SLIDE * s,
            ),
        };
        self.flyout.place(&self.gfx, rect);
        self.panel = Some(p);

        let result = (|| -> Result<()> {
            self.flyout.surface.draw(|dc| self.paint_panel(dc))?;
            if animate {
                self.flyout.surface.animate_open(&self.gfx, from_y, pal().motion_ms)?;
            }
            self.gfx.commit()
        })();
        if let Err(e) = result {
            self.on_draw_error(e);
            return;
        }
        if !self.flyout.visible {
            self.flyout.show(true);
        }
    }

    fn redraw_panel(&mut self) {
        if self.panel.is_none() {
            return;
        }
        let result = self.flyout.surface.draw(|dc| self.paint_panel(dc)).and_then(|_| self.gfx.commit());
        if let Err(e) = result {
            self.on_draw_error(e);
        }
    }

    fn font(&self, f: Font) -> &IDWriteTextFormat {
        match f {
            Font::Header => &self.fonts.header,
            Font::Body => &self.fonts.body,
            Font::BodyBold => &self.fonts.body_bold,
            Font::Sub => &self.fonts.sub,
            Font::Pill => &self.fonts.pill,
            Font::Big => &self.fonts.big,
            Font::Icon => &self.fonts.icon,
        }
    }

    fn paint_panel(&self, dc: &ID2D1DeviceContext) -> Result<()> {
        let Some(p) = &self.panel else { return Ok(()) };
        let s = self.scale();
        let radius = self.opts.style.corners.approx_radius() * s;
        let full = rectf(0.0, 0.0, self.flyout.width(), self.flyout.height());
        render::fill_round(dc, full, radius, pal().glass_bg)?;
        render::stroke_round(dc, full, radius, pal().border)?;
        unsafe {
            dc.PushAxisAlignedClip(
                &rectf(1.0, 1.0, full.right - 2.0, full.bottom - 2.0),
                windows::Win32::Graphics::Direct2D::D2D1_ANTIALIAS_MODE_ALIASED,
            );
        }
        let result = (|| -> Result<()> {
            for item in &p.items {
                match item {
                    Item::Fill(r, rad, c) => render::fill_round(dc, *r, *rad, *c)?,
                    Item::Stroke(r, rad, c) => render::stroke_round(dc, *r, *rad, *c)?,
                    Item::Text(t, f, r, c, a) => {
                        let align = match a {
                            Align::Leading => DWRITE_TEXT_ALIGNMENT_LEADING,
                            Align::Center => DWRITE_TEXT_ALIGNMENT_CENTER,
                            Align::Trailing => DWRITE_TEXT_ALIGNMENT_TRAILING,
                        };
                        render::text_aligned(dc, t, self.font(*f), *r, *c, align)?;
                    }
                    Item::Glyph(g, f, r, c) => render::glyph(dc, *g, self.font(*f), *r, *c)?,
                    Item::Bitmap(b, r) => render::draw_bitmap(dc, b, *r)?,
                }
            }
            if let Some((_, r, true)) = p.hover.and_then(|i| p.hits.get(i)) {
                render::fill_round(dc, *r, 6.0 * s, pal().hover_overlay)?;
            }
            Ok(())
        })();
        unsafe {
            dc.PopAxisAlignedClip();
        }
        result?;
        if p.content_height > p.height {
            let track = p.height - 12.0 * s;
            let thumb = (track * p.height / p.content_height).max(18.0 * s);
            let y = 6.0 * s + (track - thumb) * p.scroll / (p.content_height - p.height);
            render::fill_round(dc, rectf(p.width - 4.0 * s, y, 2.0 * s, thumb), s, pal().text_disabled)?;
        }
        Ok(())
    }

    pub fn scroll_panel(&mut self, direction: f32) {
        let step = 48.0 * self.scale();
        if let Some(p) = self.panel.as_mut() {
            if p.drag.is_some() {
                return;
            }
            p.scroll = (p.scroll + direction * step).clamp(0.0, (p.content_height - p.height).max(0.0));
            p.hover = None;
            self.show_panel(false);
        }
    }

    // ------------------------------------------------------------------
    // Input
    // ------------------------------------------------------------------

    fn panel_hit_at(&self, x: f32, y: f32) -> Option<usize> {
        let p = self.panel.as_ref()?;
        p.hits.iter().position(|(_, r, _)| x >= r.left && x < r.right && y >= r.top && y < r.bottom)
    }

    pub fn panel_mouse_move(&mut self, x: f32, y: f32) {
        if let Some((kind, l, r)) = self.panel.as_ref().and_then(|p| p.drag) {
            self.set_slider(kind, (x - l) / (r - l));
            return;
        }
        let hit = self.panel_hit_at(x, y);
        let Some(p) = self.panel.as_mut() else { return };
        if hit != p.hover {
            p.hover = hit;
            self.redraw_panel();
        }
    }

    pub fn panel_mouse_down(&mut self, x: f32, y: f32) {
        let Some(i) = self.panel_hit_at(x, y) else { return };
        let k = 16.0 * self.scale() / 2.0;
        let flyout = self.flyout.hwnd;
        let Some(p) = self.panel.as_mut() else { return };
        let (PanelHit::Slider(kind), r, _) = &p.hits[i] else { return };
        let (kind, l, rr) = (*kind, r.left + k, r.right - k);
        p.drag = Some((kind, l, rr));
        unsafe {
            SetCapture(flyout);
        }
        self.set_slider(kind, (x - l) / (rr - l));
    }

    pub fn panel_mouse_up(&mut self, x: f32, y: f32) {
        if self.panel.as_ref().is_some_and(|p| p.drag.is_some()) {
            if let Some(p) = self.panel.as_mut() {
                p.drag = None;
            }
            unsafe {
                let _ = ReleaseCapture();
            }
            return;
        }
        let Some(i) = self.panel_hit_at(x, y) else { return };
        let Some(hit) = self.panel.as_ref().map(|p| p.hits[i].0.clone()) else { return };
        self.perform(hit);
    }

    pub fn panel_capture_lost(&mut self) {
        if let Some(p) = self.panel.as_mut() {
            p.drag = None;
        }
    }

    fn set_slider(&mut self, kind: SliderKind, v: f32) {
        let v = v.clamp(0.0, 1.0);
        let Some(a) = &self.audio else { return };
        match kind {
            SliderKind::Volume => {
                a.set_volume(v);
                self.audio_state.volume = v;
                if v > 0.0 {
                    self.audio_state.muted = false;
                }
                self.redraw_bar(None);
            }
            SliderKind::Mic => {
                a.set_mic_volume(v);
                self.audio_state.mic_volume = v;
            }
        }
        self.show_panel(false);
    }

    fn perform(&mut self, hit: PanelHit) {
        match hit {
            PanelHit::Panel(kind) => self.open_panel(kind),
            PanelHit::Desktop => {
                self.close_panel();
                actions::show_desktop();
            }
            PanelHit::Shell(cmd) => {
                let scroll = self.panel.as_ref().map(|p| p.scroll).unwrap_or(0.0);
                self.close_panel();
                self.on_shell_menu(cmd);
                if (10..=14).contains(&cmd) || cmd == 16 || cmd == 18 || (20..=24).contains(&cmd) {
                    self.open_panel(PanelKind::Appearance);
                    if let Some(p) = self.panel.as_mut() {
                        p.scroll = scroll;
                    }
                    self.show_panel(false);
                }
            }
            PanelHit::Activate(hwnd, elevated) => {
                self.close_panel();
                let h = HWND(hwnd as _);
                self.present_window(h, elevated);
            }
            PanelHit::NewWindow(id) => {
                self.close_panel();
                if let Some(g) = self.dock_model.groups().iter().find(|g| g.id == id).cloned()
                    && !self.launch_app(&g.exe_path, g.aumid.as_deref(), &g.id)
                {
                    self.show_message_tooltip(&format!("Couldn't start {}", group_name(&g)));
                }
            }
            PanelHit::ToggleMute => {
                if let Some(a) = &self.audio {
                    a.toggle_mute();
                }
            }
            PanelHit::ToggleMicMute => {
                if let Some(a) = &self.audio {
                    a.toggle_mic_mute();
                }
            }
            PanelHit::Slider(_) => {}
            PanelHit::Device(id) => {
                let ok = self.audio.as_ref().is_some_and(|a| a.set_default_output(&id));
                if !ok {
                    self.close_panel();
                    actions::shell_open("ms-settings:sound", None);
                }
            }
            PanelHit::Media(app, cmd) => self.sysmon.media_command(&app, cmd),
            PanelHit::Link(uri) => {
                self.close_panel();
                actions::shell_open(uri, None);
            }
            PanelHit::Radio(sel, on) => self.sysmon.set_radio(sel, on),
            PanelHit::WifiConnect(profile) => {
                if !self.network.connect(&profile) {
                    self.close_panel();
                    actions::shell_open("ms-availablenetworks:", None);
                }
            }
            PanelHit::WifiDisconnect => {
                self.network.disconnect();
            }
            PanelHit::WifiPicker => {
                self.close_panel();
                actions::shell_open("ms-availablenetworks:", None);
            }
            PanelHit::CalPrev | PanelHit::CalNext | PanelHit::CalToday => {
                if let Some(p) = self.panel.as_mut() {
                    p.cal = match hit {
                        PanelHit::CalPrev => calendar::add_months(p.cal.0, p.cal.1, -1),
                        PanelHit::CalNext => calendar::add_months(p.cal.0, p.cal.1, 1),
                        _ => {
                            let (y, m, _) = clock::today();
                            (y, m)
                        }
                    };
                }
                self.show_panel(false);
            }
        }
    }
}
