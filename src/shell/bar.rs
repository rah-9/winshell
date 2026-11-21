//! Top bar (PLAN.md sections 2 and 4): user pill, active window pill, status
//! pills and clock, with hover states and click actions.

use windows::Win32::Foundation::{HWND, POINT, RECT};
use windows::Win32::Graphics::Direct2D::Common::D2D_RECT_F;
use windows::Win32::Graphics::Direct2D::{ID2D1Bitmap1, ID2D1DeviceContext};
use windows::Win32::UI::WindowsAndMessaging::{GetCursorPos, GetForegroundWindow, IsIconic};
use windows::core::Result;

use winshell::actions;
use winshell::apps;
use winshell::clock;
use winshell::foreground::{self, Kind};
use winshell::icon;
use winshell::log;
use winshell::mem::MemSnapshot;
use winshell::network::NetState;
use winshell::render::{self, rectf};
use winshell::util::truncate_chars;
use winshell::win::px;

use super::panel::PanelKind;
use super::theme::*;
use super::{App, Button, MenuContext, MenuRequest};

/// What the left zone currently shows.
pub struct Active {
    pub hwnd: HWND,
    pub kind: Kind,
    pub title: String,
    pub icon: Option<ID2D1Bitmap1>,
}

impl Active {
    pub fn desktop() -> Self {
        Self { hwnd: HWND::default(), kind: Kind::Desktop, title: "Desktop".into(), icon: None }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PillKind {
    Bluetooth,
    Wifi,
    Volume,
    Battery,
    Clock,
    Notifications,
    QuickSettings,
}

impl PillKind {
    fn config_name(self) -> &'static str {
        match self {
            PillKind::Bluetooth => "bluetooth",
            PillKind::Wifi => "wifi",
            PillKind::Volume => "volume",
            PillKind::Battery => "battery",
            PillKind::Clock => "clock",
            PillKind::Notifications => "notifications",
            PillKind::QuickSettings => "quick_settings",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BarHit {
    Active,
    Pill(PillKind),
}

/// Top bar geometry in surface pixels, recomputed before each draw.
#[derive(Clone, Default)]
pub struct BarLayout {
    pub user: D2D_RECT_F,
    pub divider: D2D_RECT_F,
    pub active: D2D_RECT_F,
    pub icon: D2D_RECT_F,
    pub title: D2D_RECT_F,
    pub pills: Vec<(PillKind, D2D_RECT_F)>,
}

impl BarLayout {
    fn pill(&self, k: PillKind) -> Option<D2D_RECT_F> {
        self.pills.iter().find(|(p, _)| *p == k).map(|(_, r)| *r)
    }

    fn hit(&self, x: f32, y: f32) -> Option<BarHit> {
        let inside = |r: &D2D_RECT_F| x >= r.left && x < r.right && y >= r.top && y < r.bottom;
        if inside(&self.active) {
            return Some(BarHit::Active);
        }
        self.pills.iter().find(|(_, r)| inside(r)).map(|(k, _)| BarHit::Pill(*k))
    }
}

fn rect_to_pixels(r: D2D_RECT_F) -> RECT {
    RECT {
        left: r.left.floor() as i32 - 1,
        top: r.top.floor() as i32 - 1,
        right: r.right.ceil() as i32 + 1,
        bottom: r.bottom.ceil() as i32 + 1,
    }
}

fn union(a: RECT, b: RECT) -> RECT {
    RECT {
        left: a.left.min(b.left),
        top: a.top.min(b.top),
        right: a.right.max(b.right),
        bottom: a.bottom.max(b.bottom),
    }
}

pub fn volume_glyph(volume: f32, muted: bool) -> char {
    if muted {
        glyph::MUTE
    } else if volume <= 0.0 {
        glyph::VOLUME_0
    } else if volume < 0.34 {
        glyph::VOLUME_1
    } else if volume < 0.67 {
        glyph::VOLUME_2
    } else {
        glyph::VOLUME_3
    }
}

/// (glyph, colour) for the battery pill.
pub fn battery_glyph(percent: u8, charging: bool) -> (char, u32) {
    let step = (percent as u32 + 5) / 10; // 0..=10
    if charging {
        let g = if step >= 10 {
            glyph::BATTERY_CHARGING_FULL
        } else {
            char::from_u32(glyph::BATTERY_CHARGING_0 + step.min(8)).unwrap()
        };
        return (g, pal().success);
    }
    let g = if step >= 10 { glyph::BATTERY_FULL } else { char::from_u32(glyph::BATTERY_0 + step).unwrap() };
    let c = if percent < 10 {
        pal().danger
    } else if percent < 20 {
        pal().warning
    } else {
        pal().text_primary
    };
    (g, c)
}

fn network_glyph(s: &NetState) -> (char, u32) {
    match s {
        NetState::Wifi { quality, .. } => {
            let g = match quality {
                0..=32 => glyph::WIFI_1,
                33..=65 => glyph::WIFI_2,
                66..=89 => glyph::WIFI_3,
                _ => glyph::WIFI,
            };
            (g, pal().text_primary)
        }
        NetState::Ethernet => (glyph::ETHERNET, pal().text_primary),
        NetState::Disconnected => (glyph::WIFI, pal().text_disabled),
    }
}

impl App {
    // ------------------------------------------------------------------
    // Active window (left zone)
    // ------------------------------------------------------------------

    pub fn load_active_icon(&self, hwnd: HWND, kind: Kind) -> Option<ID2D1Bitmap1> {
        if kind != Kind::App {
            return None;
        }
        let size = px(ACTIVE_ICON, self.scale()).max(1) as u32;
        let hicon = icon::window_icon(hwnd, true)?;
        let px = icon::render(hicon, size)?;
        self.gfx.bitmap_from_bgra(px.width, px.height, &px.pixels).ok()
    }

    /// Foreground changed: show the new window (or keep the old one for shell chrome).
    /// Returns true if the left zone changed.
    pub fn update_active(&mut self, hwnd: HWND) -> bool {
        let kind = foreground::classify(hwnd);
        match kind {
            Kind::Ignore => false,
            Kind::Desktop => {
                if self.active.kind == Kind::Desktop {
                    return false;
                }
                self.tracker.follow(HWND::default());
                self.active = Active { hwnd, ..Active::desktop() };
                true
            }
            Kind::App => {
                let title = foreground::title(hwnd);
                if hwnd == self.active.hwnd && title == self.active.title {
                    return false;
                }
                self.tracker.follow(hwnd);
                let icon =
                    if hwnd == self.active.hwnd { self.active.icon.take() } else { self.load_active_icon(hwnd, kind) };
                self.active = Active { hwnd, kind, title, icon };
                true
            }
        }
    }

    pub fn on_foreground(&mut self) {
        self.tracker.ack_foreground();
        let fg = unsafe { GetForegroundWindow() };
        // Another app came forward (the hook skips our own windows). Normally the flyout
        // already closed on WA_INACTIVE; this covers a flyout Windows refused to focus.
        if self.panel.is_some() && !App::foreground_is_ours(fg) {
            self.close_panel();
        }
        if self.update_active(fg) {
            self.redraw_bar(None);
        }
        self.dock_on_foreground(fg);
    }

    pub fn on_title(&mut self) {
        self.tracker.ack_title();
        if self.active.kind != Kind::App {
            return;
        }
        let title = foreground::title(self.active.hwnd);
        if title != self.active.title {
            self.active.title = title;
            self.redraw_bar(None);
        }
    }

    fn display_title(&self) -> String {
        truncate_chars(&self.active.title, TITLE_MAX_CHARS)
    }

    // ------------------------------------------------------------------
    // Clock
    // ------------------------------------------------------------------

    pub fn on_clock_timer(&mut self) {
        self.clock.on_timer(); // KillTimer, then re-arm for the next boundary
        self.refresh_clock();
        if !self.first_tick_logged {
            self.first_tick_logged = true;
            log!("first minute tick; {}", MemSnapshot::now().describe());
        }
    }

    /// Re-read the time; redraw only the clock pill when nothing else moves.
    pub fn refresh_clock(&mut self) {
        let text = clock::now_text();
        if text == self.clock_text {
            return;
        }
        let old = self.bar.pill(PillKind::Clock);
        self.clock_text = text;
        let layout = self.compute_bar_layout();
        let new = layout.pill(PillKind::Clock);
        match (old, new) {
            (Some(o), Some(n))
                if ((n.right - n.left) - (o.right - o.left)).abs() < 0.5 && (n.left - o.left).abs() < 0.5 =>
            {
                self.redraw_bar(Some(union(rect_to_pixels(o), rect_to_pixels(n))));
            }
            _ => self.redraw_bar(None),
        }
        if self.panel_is(PanelKind::Calendar) {
            self.refresh_panel_if(PanelKind::Calendar); // "today" may have changed
        }
    }

    /// Time base changed (WM_TIMECHANGE, resume): re-arm and redraw.
    pub fn resync_clock(&mut self, why: &str) {
        log!("clock resync ({why})");
        self.clock.arm();
        self.refresh_clock();
    }

    // ------------------------------------------------------------------
    // Layout and drawing
    // ------------------------------------------------------------------

    fn pill_visible(&self, k: PillKind) -> bool {
        if !self.config.top_bar.pill_enabled(k.config_name()) {
            return false;
        }
        match k {
            PillKind::Bluetooth => self.bluetooth.is_some(),
            PillKind::Volume => self.audio.is_some(),
            PillKind::Battery => self.battery.present,
            _ => true,
        }
    }

    fn battery_text(&self) -> String {
        format!("{}%", self.battery.percent)
    }

    pub fn compute_bar_layout(&self) -> BarLayout {
        let s = self.scale();
        let bw = self.top_bar.width();
        let bh = self.top_bar.height();
        let pill_h = PILL_HEIGHT * s;
        let y = ((bh - pill_h) / 2.0).round();
        let pad = PILL_PAD_X * s;
        let inset = BAR_EDGE_INSET * s;
        let gap = PILL_GAP * s;

        let user_w = (self.gfx.measure(&self.user_handle, &self.fonts.pill) + 2.0 * pad).ceil();
        let user = rectf(inset, y, user_w, pill_h);
        let div_x = (user.right + gap).round();
        let div_h = DIVIDER_HEIGHT * s;
        let divider = rectf(div_x, ((bh - div_h) / 2.0).round(), 1.0, div_h);

        // Right zone, laid out from the right edge leftwards.
        let order = [
            PillKind::QuickSettings,
            PillKind::Notifications,
            PillKind::Clock,
            PillKind::Battery,
            PillKind::Volume,
            PillKind::Wifi,
            PillKind::Bluetooth,
        ];
        let mut pills = Vec::new();
        let mut right = bw - inset;
        for k in order {
            if !self.pill_visible(k) {
                continue;
            }
            let w = match k {
                PillKind::Clock => (self.gfx.measure(&self.clock_text, &self.fonts.pill) + 2.0 * pad).ceil(),
                PillKind::Battery => {
                    (pad + ACTIVE_ICON * s + 4.0 * s + self.gfx.measure(&self.battery_text(), &self.fonts.pill) + pad)
                        .ceil()
                }
                _ => (ICON_PILL_W * s).round(),
            };
            let r = rectf(right - w, y, w, pill_h);
            right = r.left - gap;
            pills.push((k, r));
        }
        pills.reverse();
        let zone_left = pills.first().map(|(_, r)| r.left).unwrap_or(bw - inset);

        let active_x = divider.right + gap;
        let icon_sz = px(ACTIVE_ICON, s) as f32;
        let has_icon = self.active.kind == Kind::App;
        let lead = pad + if has_icon { icon_sz + ICON_TEXT_GAP * s } else { 0.0 };
        let title_room = (zone_left - 2.0 * gap - active_x - lead - pad).max(0.0);
        let title_w = self.gfx.measure(&self.display_title(), &self.fonts.title).ceil().min(title_room);
        let active = rectf(active_x, y, lead + title_w + pad, pill_h);
        let icon = rectf((active_x + pad).round(), (y + (pill_h - icon_sz) / 2.0).round(), icon_sz, icon_sz);
        let title = rectf(active_x + lead, y, title_w, pill_h);

        BarLayout { user, divider, active, icon, title, pills }
    }

    /// Redraw the top bar (`dirty` = surface pixels, `None` = everything) and commit.
    pub fn redraw_bar(&mut self, dirty: Option<RECT>) {
        self.bar = self.compute_bar_layout();
        let result = self.top_bar.surface.draw_region(dirty, |dc| self.paint_bar(dc)).and_then(|_| self.gfx.commit());
        if let Err(e) = result {
            self.on_draw_error(e);
        }
    }

    fn paint_bar(&self, dc: &ID2D1DeviceContext) -> Result<()> {
        let s = self.scale();
        let radius = self.opts.style.corners.approx_radius() * s;
        let (bw, bh) = (self.top_bar.width(), self.top_bar.height());
        let l = &self.bar;
        let pill_r = pal().pill_radius * s;
        let pad = PILL_PAD_X * s;
        let bg = |hit: BarHit| if self.bar_hover == Some(hit) { pal().pill_hover } else { pal().pill_bg };

        let full = rectf(0.0, 0.0, bw, bh);
        render::fill_round(dc, full, radius, pal().glass_bg)?;
        render::stroke_round(dc, full, radius, pal().border)?;

        // Left zone.
        render::fill_round(dc, l.user, pill_r, pal().pill_bg)?;
        let user_text = D2D_RECT_F { left: l.user.left + pad, right: l.user.right - pad + 1.0, ..l.user };
        render::text(dc, &self.user_handle, &self.fonts.pill, user_text, pal().text_secondary)?;
        render::fill_round(dc, l.divider, 0.0, pal().border)?;

        render::fill_round(dc, l.active, pill_r, bg(BarHit::Active))?;
        if self.active.kind == Kind::App {
            match &self.active.icon {
                Some(bmp) => render::draw_bitmap(dc, bmp, l.icon)?,
                None => render::stroke_round(dc, l.icon, 3.0 * s, pal().text_disabled)?,
            }
        }
        let title_color = if self.active.kind == Kind::Desktop { pal().text_secondary } else { pal().text_primary };
        let title_box = D2D_RECT_F { right: l.title.right + 1.0, ..l.title };
        render::text(dc, &self.display_title(), &self.fonts.title, title_box, title_color)?;

        // Right zone.
        for (k, r) in &l.pills {
            render::fill_round(dc, *r, pill_r, bg(BarHit::Pill(*k)))?;
            match k {
                PillKind::Bluetooth => {
                    let c = if self.bluetooth == Some(true) { pal().text_primary } else { pal().text_disabled };
                    render::glyph(dc, glyph::BLUETOOTH, &self.fonts.icon, *r, c)?;
                }
                PillKind::Wifi => {
                    let (g, c) = network_glyph(&self.net_state);
                    render::glyph(dc, g, &self.fonts.icon, *r, c)?;
                }
                PillKind::Volume => {
                    let a = &self.audio_state;
                    let c = if a.muted { pal().text_secondary } else { pal().text_primary };
                    render::glyph(dc, volume_glyph(a.volume, a.muted), &self.fonts.icon, *r, c)?;
                }
                PillKind::Battery => {
                    let (g, c) = battery_glyph(self.battery.percent, self.battery.charging);
                    let icon_r = rectf(r.left + pad, r.top, ACTIVE_ICON * s, r.bottom - r.top);
                    render::glyph(dc, g, &self.fonts.icon, icon_r, c)?;
                    let tx = icon_r.right + 4.0 * s;
                    render::text(
                        dc,
                        &self.battery_text(),
                        &self.fonts.pill,
                        D2D_RECT_F { left: tx, right: r.right, ..*r },
                        pal().text_primary,
                    )?;
                }
                PillKind::Clock => {
                    let t = D2D_RECT_F { left: r.left + pad, right: r.right - pad + 1.0, ..*r };
                    render::text(dc, &self.clock_text, &self.fonts.pill, t, pal().text_primary)?;
                }
                PillKind::Notifications => render::glyph(dc, glyph::BELL, &self.fonts.icon, *r, pal().text_primary)?,
                PillKind::QuickSettings => {
                    render::glyph(dc, glyph::SETTINGS, &self.fonts.icon, *r, pal().text_primary)?
                }
            }
        }
        Ok(())
    }

    // ------------------------------------------------------------------
    // Input
    // ------------------------------------------------------------------

    pub fn bar_mouse_move(&mut self, x: f32, y: f32) {
        let hit = self.bar.hit(x, y);
        if hit != self.bar_hover {
            self.bar_hover = hit;
            self.redraw_bar(None);
        }
    }

    pub fn bar_click(&mut self, x: f32, y: f32, button: Button) -> Option<MenuRequest> {
        match (self.bar.hit(x, y)?, button) {
            (BarHit::Active, Button::Left) => {
                self.toggle_active_window();
                None
            }
            (BarHit::Active, Button::Right) if self.active.kind == Kind::App => {
                let maximized =
                    unsafe { windows::Win32::UI::WindowsAndMessaging::IsZoomed(self.active.hwnd).as_bool() };
                let items = vec![
                    (1, "Minimize".to_string()),
                    (2, if maximized { "Restore" } else { "Maximize" }.to_string()),
                    (3, "Snap left".to_string()),
                    (4, "Snap right".to_string()),
                    (0, String::new()),
                    (5, "Close".to_string()),
                ];
                let mut pt = POINT::default();
                unsafe {
                    let _ = GetCursorPos(&mut pt);
                }
                Some(MenuRequest {
                    owner: self.top_bar.hwnd,
                    x: pt.x,
                    y: self.top_bar.rect.bottom,
                    above: false,
                    items,
                    context: MenuContext::ActiveWindow { hwnd: self.active.hwnd.0 as isize },
                })
            }
            (BarHit::Pill(PillKind::Volume), Button::Right | Button::Middle) => {
                if let Some(a) = &self.audio {
                    a.toggle_mute();
                }
                None
            }
            (BarHit::Pill(k), Button::Left) => {
                self.pill_action(k);
                None
            }
            _ => None,
        }
    }

    fn pill_action(&mut self, k: PillKind) {
        match k {
            PillKind::Volume => self.toggle_panel(PanelKind::Sound),
            PillKind::Battery => self.toggle_panel(PanelKind::Battery),
            PillKind::Clock => self.toggle_panel(PanelKind::Calendar),
            other => {
                self.close_panel();
                match other {
                    PillKind::Wifi => {
                        let uri = if self.net_state == NetState::Ethernet {
                            "ms-settings:network-ethernet"
                        } else {
                            "ms-settings:network-wifi"
                        };
                        actions::shell_open(uri, None);
                    }
                    PillKind::Bluetooth => {
                        actions::shell_open("ms-settings:bluetooth", None);
                    }
                    PillKind::Notifications => actions::open_notifications(),
                    PillKind::QuickSettings => actions::open_quick_settings(),
                    _ => {}
                }
            }
        }
    }

    /// Left click on the active window pill: minimise it if it is in front,
    /// otherwise bring it to the front.
    fn toggle_active_window(&mut self) {
        if self.active.kind != Kind::App {
            return;
        }
        let hwnd = self.active.hwnd;
        let fg = apps::root_owner(unsafe { GetForegroundWindow() });
        let iconic = unsafe { IsIconic(hwnd).as_bool() };
        if fg == apps::root_owner(hwnd) && !iconic {
            actions::minimize(hwnd);
        } else if !actions::activate(hwnd) {
            self.show_message_tooltip("Can't switch to that window");
        }
    }

    pub fn on_active_menu(&mut self, hwnd: HWND, cmd: u32) {
        match cmd {
            1 => actions::minimize(hwnd),
            2 => actions::toggle_maximize(hwnd),
            3 => actions::snap(hwnd, true),
            4 => actions::snap(hwnd, false),
            5 => actions::close(hwnd),
            _ => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn volume_glyph_steps() {
        assert_eq!(volume_glyph(0.5, true), glyph::MUTE);
        assert_eq!(volume_glyph(0.0, false), glyph::VOLUME_0);
        assert_eq!(volume_glyph(0.2, false), glyph::VOLUME_1);
        assert_eq!(volume_glyph(0.5, false), glyph::VOLUME_2);
        assert_eq!(volume_glyph(1.0, false), glyph::VOLUME_3);
    }

    #[test]
    fn battery_glyph_colours() {
        assert_eq!(battery_glyph(5, false).1, pal().danger);
        assert_eq!(battery_glyph(15, false).1, pal().warning);
        assert_eq!(battery_glyph(100, false).0, glyph::BATTERY_FULL);
        assert_eq!(battery_glyph(50, true).1, pal().success);
    }
}
