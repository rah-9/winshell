//! Dock (PLAN.md sections 2 and 4): Start, pinned apps, running apps and the
//! mini media player, with hover, tooltip, click, middle-click and context menu.

use windows::Win32::Foundation::HWND;
use windows::Win32::Graphics::Direct2D::Common::D2D_RECT_F;
use windows::Win32::Graphics::Direct2D::{ID2D1Bitmap1, ID2D1DeviceContext};
use windows::Win32::UI::WindowsAndMessaging::{GetForegroundWindow, IsIconic, KillTimer, SetTimer};
use windows::core::Result;

use winshell::actions;
use winshell::apps;
use winshell::assets::AssetRequest;
use winshell::config;
use winshell::dock_model::AppGroup;
use winshell::log;
use winshell::render::{self, rectf};
use winshell::sysmon::MediaCommand;
use winshell::util::truncate_chars;
use winshell::win::{self, px};

use super::panel::PanelKind;
use super::theme::*;
use super::{App, Button, MenuContext, MenuRequest, TIMER_TOOLTIP_HIDE};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MediaPart {
    Body,
    PlayPause,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DockHit {
    Start,
    /// Index into `DockModel::groups()`.
    App(usize),
    Media(MediaPart),
}

#[derive(Clone, Copy, Default)]
pub struct MediaLayout {
    pub pill: D2D_RECT_F,
    pub art: D2D_RECT_F,
    pub title: D2D_RECT_F,
    pub artist: D2D_RECT_F,
    pub play: D2D_RECT_F,
}

/// Dock geometry in surface pixels.
#[derive(Clone, Default)]
pub struct DockLayout {
    pub width: f32,
    pub start: D2D_RECT_F,
    pub apps: Vec<(usize, D2D_RECT_F)>,
    pub separators: Vec<f32>,
    pub media: Option<MediaLayout>,
}

fn inside(r: &D2D_RECT_F, x: f32, y: f32) -> bool {
    x >= r.left && x < r.right && y >= r.top && y < r.bottom
}

impl DockLayout {
    fn hit(&self, x: f32, y: f32) -> Option<DockHit> {
        if inside(&self.start, x, y) {
            return Some(DockHit::Start);
        }
        if let Some((gi, _)) = self.apps.iter().find(|(_, r)| inside(r, x, y)) {
            return Some(DockHit::App(*gi));
        }
        let m = self.media.as_ref()?;
        if inside(&m.play, x, y) {
            Some(DockHit::Media(MediaPart::PlayPause))
        } else if inside(&m.pill, x, y) {
            Some(DockHit::Media(MediaPart::Body))
        } else {
            None
        }
    }

    pub fn slot(&self, group: usize) -> Option<D2D_RECT_F> {
        self.apps.iter().find(|(g, _)| *g == group).map(|(_, r)| *r)
    }
}

/// Friendly name for tooltips and headers, falling back to the exe name or AUMID.
pub fn group_name(g: &AppGroup) -> String {
    if !g.display_name.is_empty() {
        return g.display_name.clone();
    }
    if !g.exe_path.is_empty() {
        return std::path::Path::new(&g.exe_path)
            .file_stem()
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_default();
    }
    g.aumid.clone().unwrap_or_default()
}

/// Start button glyph: a 2 x 2 grid of rounded squares, centred in the slot.
fn paint_start_glyph(dc: &ID2D1DeviceContext, slot: D2D_RECT_F, s: f32) -> Result<()> {
    let cell = (8.0 * s).round();
    let gap = (2.0 * s).round().max(1.0);
    let total = 2.0 * cell + gap;
    let x0 = (slot.left + (slot.right - slot.left - total) / 2.0).round();
    let y0 = (slot.top + (slot.bottom - slot.top - total) / 2.0).round();
    for (cx, cy) in [(0.0, 0.0), (1.0, 0.0), (0.0, 1.0), (1.0, 1.0)] {
        let r = rectf(x0 + cx * (cell + gap), y0 + cy * (cell + gap), cell, cell);
        render::fill_round(dc, r, 1.5 * s, pal().start_glyph)?;
    }
    Ok(())
}

impl App {
    // ------------------------------------------------------------------
    // Model upkeep (M3)
    // ------------------------------------------------------------------

    /// Resolve `[dock] pinned_apps` and apply it to the model.
    pub fn load_pins(&mut self) {
        let pins = self.config.dock.pinned_apps.iter().map(|e| actions::resolve_pin(e)).collect();
        self.dock_model.set_pins(pins);
        self.sync_assets();
    }

    /// Full window rescan. Only on startup, resume, display change and Explorer restart.
    pub fn rescan(&mut self, why: &str) {
        self.dock_model.replace_all(apps::enumerate());
        let fg = unsafe { GetForegroundWindow() };
        if !App::foreground_is_ours(fg) {
            self.dock_model.set_focused(apps::root_owner(fg).0 as isize);
        }
        let windows: usize = self.dock_model.groups().iter().map(|g| g.windows.len()).sum();
        log!("window rescan ({why}): {} slots, {windows} windows", self.dock_model.groups().len());
        self.sync_assets();
    }

    pub fn dock_on_foreground(&mut self, fg: HWND) {
        if App::foreground_is_ours(fg) {
            return;
        }
        // A window can become dock-eligible late (e.g. it gets its title after showing).
        let root = apps::root_owner(fg);
        let mut changed = match apps::window_entry(root) {
            Some(e) => self.dock_model.upsert(e),
            None => false,
        };
        changed |= self.dock_model.set_focused(root.0 as isize);
        if changed {
            self.dock_changed();
        }
    }

    /// Queued window events: re-evaluate only the windows that changed.
    pub fn on_window_events(&mut self) {
        let mut changed = false;
        for p in self.window_hooks.take_pending() {
            if p.destroyed {
                changed |= self.dock_model.remove(p.hwnd);
                continue;
            }
            match apps::window_entry(HWND(p.hwnd as _)) {
                Some(e) => changed |= self.dock_model.upsert(e),
                None => changed |= self.dock_model.remove(p.hwnd),
            }
        }
        let fg = unsafe { GetForegroundWindow() };
        if !App::foreground_is_ours(fg) {
            changed |= self.dock_model.set_focused(apps::root_owner(fg).0 as isize);
        }
        if changed {
            self.dock_changed();
            if let Some(PanelKind::WindowList(id)) = self.panel.as_ref().map(|p| p.kind.clone()) {
                if self.dock_model.groups().iter().any(|g| g.id == id && g.windows.len() > 1) {
                    self.refresh_panel_if(PanelKind::WindowList(id));
                } else {
                    self.close_panel();
                }
            }
        }
    }

    pub fn dock_changed(&mut self) {
        self.sync_assets();
        self.relayout_dock();
        self.redraw_dock();
    }

    fn icon_fetch_size(&self) -> u32 {
        let want = (DOCK_ICON * self.scale() * DOCK_HOVER_SCALE).ceil() as u32;
        want.max(DOCK_ICON_FETCH_MIN)
    }

    /// Fill in known names and ask the asset thread for icons not yet cached.
    pub fn sync_assets(&mut self) {
        let size = self.icon_fetch_size();
        let mut wanted = Vec::new();
        for g in self.dock_model.groups() {
            if !self.icons.contains(&g.id) && !self.icon_requested.contains(&g.id) {
                wanted.push(AssetRequest {
                    app_id: g.id.clone(),
                    aumid: g.aumid.clone(),
                    exe_path: g.exe_path.clone(),
                    size,
                });
            }
        }
        for r in wanted {
            self.icon_requested.insert(r.app_id.clone());
            self.assets.request(r);
        }
        let unnamed: Vec<String> =
            self.dock_model.groups().iter().filter(|g| g.display_name.is_empty()).map(|g| g.id.clone()).collect();
        for id in unnamed {
            if let Some(n) = self.app_names.get(&id).cloned()
                && let Some(g) = self.dock_model.group_mut(&id)
            {
                g.display_name = n;
            }
        }
    }

    pub fn on_assets(&mut self) {
        let mut redraw = false;
        for r in self.assets.drain() {
            self.icon_requested.remove(&r.app_id);
            if !r.display_name.is_empty() {
                self.app_names.insert(r.app_id.clone(), r.display_name.clone());
                if let Some(g) = self.dock_model.group_mut(&r.app_id) {
                    g.display_name = r.display_name.clone();
                }
            }
            let bitmap = r.icon.and_then(|i| {
                let bytes = (i.width * i.height * 4) as usize;
                self.gfx.bitmap_from_bgra(i.width, i.height, &i.pixels).ok().map(|b| (b, bytes))
            });
            match bitmap {
                // The raw pixels are dropped here; only the GPU bitmap is kept.
                Some((bmp, bytes)) => {
                    for evicted in self.icons.insert(r.app_id.clone(), bmp, bytes) {
                        log!("icon cache evicted {evicted}");
                    }
                }
                None => log!("no icon for {}", r.app_id),
            }
            redraw |= self.dock_model.groups().iter().any(|g| g.id == r.app_id);
        }
        if redraw {
            self.redraw_dock();
        }
    }

    // ------------------------------------------------------------------
    // Layout
    // ------------------------------------------------------------------

    fn media_visible(&self) -> bool {
        !self.media.is_empty()
    }

    fn build_dock_layout(&self, max_apps: usize) -> DockLayout {
        let s = self.scale();
        let slot = DOCK_SLOT * s;
        let gap = DOCK_SLOT_GAP * s;
        let sep_m = DOCK_SEPARATOR_MARGIN * s;
        let dh = px(DOCK_HEIGHT, s) as f32;
        let y = ((dh - slot) / 2.0).round();
        let mut l = DockLayout::default();
        let mut x = DOCK_PAD_X * s;

        l.start = rectf(x, y, slot, slot);
        x += slot;

        let pinned = self.dock_model.pinned_count();
        let mut section = None;
        for gi in 0..self.dock_model.groups().len().min(max_apps) {
            let this = if gi < pinned { 0 } else { 1 };
            if section != Some(this) {
                x += sep_m;
                l.separators.push(x.round());
                x += 1.0 + sep_m;
            } else {
                x += gap;
            }
            l.apps.push((gi, rectf(x.round(), y, slot, slot)));
            x += slot;
            section = Some(this);
        }

        if self.media_visible() {
            x += sep_m;
            l.separators.push(x.round());
            x += 1.0 + sep_m;
            let h = MEDIA_PILL_H * s;
            let py = ((dh - h) / 2.0).round();
            let w = (6.0 + MEDIA_ART + 8.0 + MEDIA_TEXT_W + MEDIA_BUTTON + 6.0) * s;
            let pill = rectf(x.round(), py, w.round(), h);
            let art_sz = MEDIA_ART * s;
            let art = rectf(pill.left + 6.0 * s, (py + (h - art_sz) / 2.0).round(), art_sz, art_sz);
            let tx = art.right + 8.0 * s;
            let tw = MEDIA_TEXT_W * s;
            let title = rectf(tx, py + 6.0 * s, tw, 20.0 * s);
            let artist = rectf(tx, py + 24.0 * s, tw, 18.0 * s);
            let b = MEDIA_BUTTON * s;
            let play = rectf(pill.right - 6.0 * s - b, (py + (h - b) / 2.0).round(), b, b);
            l.media = Some(MediaLayout { pill, art, title, artist, play });
            x = pill.right;
        }

        l.width = (x + DOCK_PAD_X * s).ceil();
        l
    }

    /// The widest layout that fits the usable span; extra running apps are dropped from the end.
    fn fitting_dock_layout(&self) -> DockLayout {
        let s = self.scale();
        let avail = (self.span.1 - self.span.0 - 2 * px(BAR_MARGIN, s)) as f32;
        let n = self.dock_model.groups().len();
        for k in (0..=n).rev() {
            let l = self.build_dock_layout(k);
            if l.width <= avail {
                if k < n {
                    log!("dock: {} of {n} apps fit", k);
                }
                return l;
            }
        }
        self.build_dock_layout(0)
    }

    /// The dock window resizes as apps come and go; the reservation never does.
    pub fn relayout_dock(&mut self) {
        self.dock_layout = self.fitting_dock_layout();
        let s = self.scale();
        let (left, right) = self.span;
        let w = self.dock_layout.width as i32;
        let h = px(DOCK_HEIGHT, s);
        let r = win::rect(left + (right - left - w) / 2, self.bottom_band.bottom - px(DOCK_MARGIN_BOTTOM, s) - h, w, h);
        self.dock.place(&self.gfx, r);
        if let Some(DockHit::App(gi)) = self.dock_hover
            && gi >= self.dock_model.groups().len()
        {
            self.dock_hover = None;
            self.hide_tooltip();
        }
    }

    // ------------------------------------------------------------------
    // Drawing
    // ------------------------------------------------------------------

    pub fn redraw_dock(&mut self) {
        if let Err(e) = self.draw_dock().and_then(|_| self.gfx.commit()) {
            self.on_draw_error(e);
        }
    }

    fn draw_dock(&mut self) -> Result<()> {
        // Look icons up first (the LRU touch needs &mut); COM pointers are cheap to clone.
        let ids: Vec<String> =
            self.dock_layout.apps.iter().map(|(gi, _)| self.dock_model.groups()[*gi].id.clone()).collect();
        let bitmaps: Vec<Option<ID2D1Bitmap1>> = ids.iter().map(|id| self.icons.get(id).cloned()).collect();
        let this = &*self;
        this.dock.surface.draw(|dc| this.paint_dock(dc, &bitmaps))
    }

    fn paint_dock(&self, dc: &ID2D1DeviceContext, bitmaps: &[Option<ID2D1Bitmap1>]) -> Result<()> {
        let s = self.scale();
        let radius = self.opts.style.corners.approx_radius() * s;
        let (dw, dh) = (self.dock.width(), self.dock.height());
        let l = &self.dock_layout;
        let full = rectf(0.0, 0.0, dw, dh);
        render::fill_round(dc, full, radius, pal().glass_bg)?;
        render::stroke_round(dc, full, radius, pal().border)?;

        let hover_bg = |r: D2D_RECT_F| render::fill_round(dc, r, pal().icon_slot_radius * s, pal().pill_hover);

        if self.dock_hover == Some(DockHit::Start) {
            hover_bg(l.start)?;
        }
        paint_start_glyph(dc, l.start, s)?;

        let sep_h = DOCK_SEPARATOR_HEIGHT * s;
        for x in &l.separators {
            render::fill_round(dc, rectf(*x, ((dh - sep_h) / 2.0).round(), 1.0, sep_h), 0.0, pal().border)?;
        }

        let focused = self.dock_model.focused();
        let groups = self.dock_model.groups();
        for (i, (gi, slot)) in l.apps.iter().enumerate() {
            let g = &groups[*gi];
            let hovered = self.dock_hover == Some(DockHit::App(*gi));
            if hovered {
                hover_bg(*slot)?;
            }
            let base = (DOCK_ICON * s).round();
            let icon_sz =
                if hovered && self.config.dock.magnification { (base * DOCK_HOVER_SCALE).round() } else { base };
            let cx = (slot.left + slot.right) / 2.0;
            let cy = (slot.top + slot.bottom) / 2.0;
            // Magnified icons grow upwards so they stay clear of the indicators.
            let grow = icon_sz - base;
            let dest = rectf((cx - icon_sz / 2.0).round(), (cy - base / 2.0 - grow).round(), icon_sz, icon_sz);
            match &bitmaps[i] {
                Some(bmp) => render::draw_bitmap(dc, bmp, dest)?,
                None => render::fill_round(dc, dest, pal().icon_slot_radius * s, pal().pill_bg)?,
            }
            if g.windows.iter().any(|w| w.is_elevated) {
                let b = 16.0 * s;
                let badge = rectf(dest.right - b + 2.0 * s, dest.bottom - b + 2.0 * s, b, b);
                render::fill_round(dc, badge, b / 2.0, (pal().glass_bg & 0xFFFFFF00) | 0xE6)?;
                render::glyph(dc, glyph::ADMIN, &self.fonts.icon_small, badge, pal().warning)?;
            }

            // Below the slot: focused pill, else multi-window dots, else running dot.
            if !g.is_running() {
                continue;
            }
            let iy = slot.bottom + INDICATOR_GAP * s;
            if focused == Some(*gi) {
                let (w, h) = (FOCUS_PILL_W * s, FOCUS_PILL_H * s);
                render::fill_round(dc, rectf(cx - w / 2.0, iy, w, h), h / 2.0, pal().accent)?;
            } else if g.windows.len() > 1 {
                let d = MULTI_DOT * s;
                let gap = INDICATOR_GAP * s;
                let x0 = cx - d - gap / 2.0;
                render::fill_round(dc, rectf(x0, iy, d, d), d / 2.0, pal().running_dot)?;
                render::fill_round(dc, rectf(x0 + d + gap, iy, d, d), d / 2.0, pal().running_dot)?;
            } else {
                let d = RUNNING_DOT * s;
                render::fill_round(dc, rectf(cx - d / 2.0, iy, d, d), d / 2.0, pal().running_dot)?;
            }
        }

        if let (Some(m), Some(session)) = (&l.media, self.media.first()) {
            let bg =
                if self.dock_hover == Some(DockHit::Media(MediaPart::Body)) { pal().pill_hover } else { pal().pill_bg };
            render::fill_round(dc, m.pill, pal().icon_slot_radius * s, bg)?;
            match self.media_bitmap(&session.app_id) {
                Some(b) => render::draw_bitmap(dc, b, m.art)?,
                None => {
                    render::fill_round(dc, m.art, 6.0 * s, pal().pill_hover)?;
                    render::glyph(dc, glyph::MUSIC, &self.fonts.icon, m.art, pal().text_secondary)?;
                }
            }
            let title = if session.title.is_empty() { "Unknown title" } else { session.title.as_str() };
            render::text(dc, title, &self.fonts.body_bold, m.title, pal().text_primary)?;
            render::text(dc, &session.artist, &self.fonts.sub, m.artist, pal().text_secondary)?;
            if self.dock_hover == Some(DockHit::Media(MediaPart::PlayPause)) {
                render::fill_round(dc, m.play, (MEDIA_BUTTON * s) / 2.0, pal().pill_hover)?;
            }
            let g = if session.playing { glyph::PAUSE } else { glyph::PLAY };
            let c = if session.can_play_pause { pal().text_primary } else { pal().text_disabled };
            render::glyph(dc, g, &self.fonts.icon, m.play, c)?;
        }
        Ok(())
    }

    // ------------------------------------------------------------------
    // Tooltip
    // ------------------------------------------------------------------

    /// Show `text` in the capsule tooltip centred above `anchor` (dock surface coordinates).
    fn show_tooltip_at(&mut self, text: &str, anchor: D2D_RECT_F) {
        let s = self.scale();
        let w = (self.gfx.measure(text, &self.fonts.pill) + 2.0 * TOOLTIP_PAD_X * s).ceil() as i32;
        let h = px(TOOLTIP_H, s);
        let cx = self.dock.rect.left + ((anchor.left + anchor.right) / 2.0) as i32;
        let x = (cx - w / 2).clamp(self.span.0, (self.span.1 - w).max(self.span.0));
        let y = self.dock.rect.top - px(TOOLTIP_GAP, s) - h;
        self.tooltip.place(&self.gfx, win::rect(x, y, w, h));
        self.tooltip_text = text.to_string();
        let radius = self.opts.style.corners.approx_radius() * s;
        let (tw, th) = (self.tooltip.width(), self.tooltip.height());
        let fonts = &self.fonts;
        let result = self
            .tooltip
            .surface
            .draw(|dc| {
                let full = rectf(0.0, 0.0, tw, th);
                render::fill_round(dc, full, radius, pal().glass_bg)?;
                render::stroke_round(dc, full, radius, pal().border)?;
                render::text_aligned(
                    dc,
                    text,
                    &fonts.pill,
                    full,
                    pal().text_primary,
                    windows::Win32::Graphics::DirectWrite::DWRITE_TEXT_ALIGNMENT_CENTER,
                )
            })
            .and_then(|_| self.gfx.commit());
        if let Err(e) = result {
            self.on_draw_error(e);
            return;
        }
        if !self.tooltip.visible {
            self.tooltip.show(false);
        }
    }

    pub fn hide_tooltip(&mut self) {
        unsafe {
            let _ = KillTimer(Some(self.top_bar.hwnd), TIMER_TOOLTIP_HIDE);
        }
        self.tooltip.hide();
        self.tooltip_text.clear();
    }

    /// A short message above the dock that hides itself after a few seconds
    /// (one-shot timer, killed when it fires).
    pub fn show_message_tooltip(&mut self, text: &str) {
        let anchor = rectf(0.0, 0.0, self.dock.width(), self.dock.height());
        self.show_tooltip_at(text, anchor);
        unsafe {
            SetTimer(Some(self.top_bar.hwnd), TIMER_TOOLTIP_HIDE, 2500, None);
        }
    }

    // ------------------------------------------------------------------
    // Input
    // ------------------------------------------------------------------

    pub fn dock_mouse_move(&mut self, x: f32, y: f32) {
        let hit = self.dock_layout.hit(x, y);
        if hit == self.dock_hover {
            return;
        }
        self.dock_hover = hit;
        self.redraw_dock();
        match hit {
            Some(DockHit::Start) => self.show_tooltip_at("Start", self.dock_layout.start),
            Some(DockHit::App(gi)) => {
                if let (Some(g), Some(slot)) = (self.dock_model.groups().get(gi), self.dock_layout.slot(gi)) {
                    let mut name = group_name(g);
                    if g.windows.iter().any(|w| w.is_elevated) {
                        name.push_str(" (administrator)");
                    }
                    self.show_tooltip_at(&name, slot);
                }
            }
            Some(DockHit::Media(_)) => {
                if let (Some(m), Some(session)) = (self.dock_layout.media, self.media.first()) {
                    let t = truncate_chars(&format!("{} · {}", session.title, session.artist), 60);
                    self.show_tooltip_at(t.trim_end_matches(" · "), m.pill);
                }
            }
            None => self.hide_tooltip(),
        }
    }

    pub fn dock_click(&mut self, x: f32, y: f32, button: Button) -> Option<MenuRequest> {
        let hit = self.dock_layout.hit(x, y)?;
        self.hide_tooltip();
        match (hit, button) {
            (DockHit::Start, Button::Left) => {
                self.close_panel();
                actions::open_start();
                None
            }
            (DockHit::Media(MediaPart::PlayPause), Button::Left) => {
                if let Some(s) = self.media.first() {
                    self.sysmon.media_command(&s.app_id, MediaCommand::PlayPause);
                }
                None
            }
            (DockHit::Media(MediaPart::Body), Button::Left) => {
                self.toggle_panel(PanelKind::Sound);
                None
            }
            (DockHit::App(gi), Button::Left) => {
                self.app_left_click(gi);
                None
            }
            (DockHit::App(gi), Button::Middle) => {
                self.close_panel();
                self.launch_group(gi);
                None
            }
            (DockHit::App(gi), Button::Right) => self.app_menu(gi),
            _ => None,
        }
    }

    fn app_left_click(&mut self, gi: usize) {
        let Some(g) = self.dock_model.groups().get(gi).cloned() else { return };
        match g.windows.len() {
            0 => {
                self.close_panel();
                self.launch_group(gi);
            }
            1 => {
                self.close_panel();
                self.toggle_window(HWND(g.windows[0].hwnd as _), g.windows[0].is_elevated);
            }
            _ => self.toggle_panel(PanelKind::WindowList(g.id.clone())),
        }
    }

    /// Single-window app: minimise it if it is in front, otherwise bring it forward.
    pub fn toggle_window(&mut self, hwnd: HWND, elevated: bool) {
        let fg = apps::root_owner(unsafe { GetForegroundWindow() });
        let iconic = unsafe { IsIconic(hwnd).as_bool() };
        if fg == hwnd && !iconic {
            actions::minimize(hwnd);
        } else if !actions::activate(hwnd) {
            let msg = if elevated {
                "This is an administrator window: switch to it with Alt+Tab"
            } else {
                "Windows didn't allow switching to that window"
            };
            self.show_message_tooltip(msg);
        }
    }

    fn launch_group(&mut self, gi: usize) {
        let Some(g) = self.dock_model.groups().get(gi).cloned() else { return };
        if !actions::launch(&g.exe_path, g.aumid.as_deref()) {
            self.show_message_tooltip(&format!("Couldn't start {}", group_name(&g)));
        }
    }

    fn app_menu(&mut self, gi: usize) -> Option<MenuRequest> {
        self.close_panel();
        let g = self.dock_model.groups().get(gi)?;
        let slot = self.dock_layout.slot(gi)?;
        let mut items = vec![
            (1, if g.is_running() { "New window" } else { "Open" }.to_string()),
            (2, if g.is_pinned { "Unpin from Dock" } else { "Pin to Dock" }.to_string()),
        ];
        if g.is_running() {
            items.push((0, String::new()));
            items.push((3, if g.windows.len() > 1 { "Close all windows" } else { "Close window" }.to_string()));
            items.push((4, "Quit app".to_string()));
        }
        Some(MenuRequest {
            owner: self.dock.hwnd,
            x: self.dock.rect.left + ((slot.left + slot.right) / 2.0) as i32,
            y: self.dock.rect.top - px(4.0, self.scale()),
            above: true,
            items,
            context: MenuContext::Dock { group_id: g.id.clone() },
        })
    }

    pub fn on_dock_menu(&mut self, group_id: &str, cmd: u32) {
        let Some(gi) = self.dock_model.groups().iter().position(|g| g.id == group_id) else { return };
        let g = self.dock_model.groups()[gi].clone();
        match cmd {
            1 => self.launch_group(gi),
            2 => self.toggle_pin(&g),
            3 => {
                for w in &g.windows {
                    actions::close(HWND(w.hwnd as _));
                }
            }
            4 => {
                let mut pids: Vec<u32> = g.windows.iter().map(|w| w.process_id).collect();
                pids.sort_unstable();
                pids.dedup();
                actions::quit_processes(&pids);
            }
            _ => {}
        }
    }

    /// Pin or unpin, editing config.toml in place so comments and formatting survive.
    fn toggle_pin(&mut self, g: &AppGroup) {
        let (entry, pin) = match self.dock_model.pin_for(&g.id) {
            Some(p) => (p.entry.clone(), false),
            None => (g.aumid.clone().unwrap_or_else(|| g.exe_path.clone()), true),
        };
        if entry.is_empty() {
            return;
        }
        let text = std::fs::read_to_string(&self.config_path).unwrap_or_else(|_| config::DEFAULT_CONFIG.to_string());
        let updated = match config::set_pinned(&text, &entry, pin) {
            Ok(t) => t,
            Err(e) => {
                log!("pin: config.toml could not be edited ({e})");
                self.show_message_tooltip("Couldn't update config.toml (see the log)");
                return;
            }
        };
        // Remember the exact text so the file watcher ignores its own echo.
        self.last_self_write = Some(updated.clone());
        if let Err(e) = std::fs::write(&self.config_path, &updated) {
            self.last_self_write = None;
            log!("pin: write failed ({e})");
            return;
        }
        if let Ok(c) = config::parse(&updated) {
            self.config.dock.pinned_apps = c.dock.pinned_apps;
        }
        log!("{} {entry}", if pin { "pinned" } else { "unpinned" });
        self.load_pins();
        self.dock_changed();
    }
}
