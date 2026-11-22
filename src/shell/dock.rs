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
    Scroll(i32),
    /// Index into `DockModel::groups()`.
    App(usize),
    Media(MediaPart),
}

#[derive(Clone, Copy, Default)]
pub struct MediaLayout {
    pub pill: D2D_RECT_F,
    pub art: D2D_RECT_F,
    pub play: D2D_RECT_F,
}

/// Dock geometry in surface pixels.
#[derive(Clone, Default)]
pub struct DockLayout {
    pub width: f32,
    pub height: f32,
    pub navigation: Vec<(i32, D2D_RECT_F)>,
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
        if let Some((step, _)) = self.navigation.iter().find(|(_, r)| inside(r, x, y)) {
            return Some(DockHit::Scroll(*step));
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

fn build_rail(s: f32, pinned: usize, count: usize, offset: usize, max_apps: usize, media: bool) -> DockLayout {
    let slot = DOCK_SLOT * s;
    let x = DOCK_PAD * s;
    let mut y = DOCK_PAD * s;
    let mut l = DockLayout { width: (DOCK_WIDTH * s).round(), ..Default::default() };
    l.start = rectf(x, y, slot, slot);
    y += slot;
    let mut section = None;
    for gi in (offset..count).take(max_apps) {
        let this = if gi < pinned { 0 } else { 1 };
        if section != Some(this) {
            y += DOCK_SEPARATOR_MARGIN * s;
            l.separators.push(y.round());
            y += 1.0 + DOCK_SEPARATOR_MARGIN * s;
        } else {
            y += DOCK_SLOT_GAP * s;
        }
        l.apps.push((gi, rectf(x, y.round(), slot, slot)));
        y += slot;
        section = Some(this);
    }
    if offset > 0 || offset + l.apps.len() < count {
        y += 4.0 * s;
        for step in [-1, 1] {
            l.navigation.push((step, rectf(x, y, slot, 20.0 * s)));
            y += 20.0 * s;
        }
    }
    if media {
        y += DOCK_SEPARATOR_MARGIN * s;
        l.separators.push(y.round());
        y += DOCK_SEPARATOR_MARGIN * s;
        let art_sz = MEDIA_ART * s;
        let art = rectf((l.width - art_sz) / 2.0, y + 4.0 * s, art_sz, art_sz);
        let btn = MEDIA_BUTTON * s;
        let play = rectf((l.width - btn) / 2.0, art.bottom + 2.0 * s, btn, btn);
        let pill = rectf(x, y, slot, play.bottom - y + 4.0 * s);
        l.media = Some(MediaLayout { pill, art, play });
        y = pill.bottom;
    }
    l.height = (y + DOCK_PAD * s).ceil();
    l
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
        self.icon_failed.clear();
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
        self.check_pending_launch(fg);
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
        self.on_window_events_inner();
        self.update_visibility();
    }

    fn on_window_events_inner(&mut self) {
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
        self.check_pending_launch(fg);
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
            if !self.icons.contains(&g.id) && !self.icon_requested.contains(&g.id) && !self.icon_failed.contains(&g.id)
            {
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
                if self.app_names.len() >= 512 {
                    // Bounded: keep only names of apps currently in the dock.
                    let live: std::collections::HashSet<&str> =
                        self.dock_model.groups().iter().map(|g| g.id.as_str()).collect();
                    self.app_names.retain(|k, _| live.contains(k.as_str()));
                }
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
                None => {
                    // Negative cache: do not ask the shell again until the next rescan.
                    if self.icon_failed.len() < 1024 {
                        self.icon_failed.insert(r.app_id.clone());
                    }
                    log!("no icon for {}", r.app_id);
                }
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
        build_rail(
            self.scale(),
            self.dock_model.pinned_count(),
            self.dock_model.groups().len(),
            self.dock_scroll,
            max_apps,
            self.media_visible(),
        )
    }

    fn fitting_dock_layout(&self) -> DockLayout {
        let avail = (self.dock_area.bottom - self.dock_area.top) as f32;
        let n = self
            .dock_model
            .groups()
            .len()
            .saturating_sub(self.dock_scroll)
            .min((avail / (DOCK_SLOT * self.scale())).max(0.0) as usize);
        let full = self.build_dock_layout(n);
        if full.height <= avail {
            return full;
        }
        let (mut lo, mut hi) = (0usize, n);
        while lo < hi {
            let mid = (lo + hi).div_ceil(2);
            if self.build_dock_layout(mid).height <= avail { lo = mid } else { hi = mid - 1 }
        }
        self.build_dock_layout(lo)
    }

    pub fn relayout_dock(&mut self) {
        self.dock_scroll = self.dock_scroll.min(self.dock_model.groups().len().saturating_sub(1));
        self.dock_hover = None;
        self.hide_tooltip();
        self.dock_layout = self.fitting_dock_layout();
        let w = self.dock_layout.width as i32;
        let h = self.dock_layout.height as i32;
        let area = self.dock_area;
        let r = win::rect(area.left, area.top + ((area.bottom - area.top - h) / 2).max(0), w, h);
        self.dock.place(&self.gfx, r);
        self.update_visibility();
    }

    pub fn scroll_dock(&mut self, step: i32) {
        let max = self.dock_model.groups().len().saturating_sub(self.dock_layout.apps.len().max(1));
        self.dock_scroll = self.dock_scroll.saturating_add_signed(step as isize).min(max);
        self.relayout_dock();
        self.redraw_dock();
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

        for y in &l.separators {
            render::fill_round(dc, rectf(10.0 * s, *y, dw - 20.0 * s, 1.0), 0.0, pal().border)?;
        }
        for (step, r) in &l.navigation {
            let enabled = if *step < 0 {
                self.dock_scroll > 0
            } else {
                self.dock_scroll + l.apps.len() < self.dock_model.groups().len()
            };
            if self.dock_hover == Some(DockHit::Scroll(*step)) {
                hover_bg(*r)?;
            }
            render::glyph(
                dc,
                if *step < 0 { glyph::CHEVRON_UP } else { glyph::CHEVRON_DOWN },
                &self.fonts.icon_small,
                *r,
                if enabled { pal().text_primary } else { pal().text_disabled },
            )?;
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
            // Icons grow inward, clear of the running indicators at the left edge.
            let dest = rectf((cx - base / 2.0).round(), (cy - icon_sz / 2.0).round(), icon_sz, icon_sz);
            match &bitmaps[i] {
                Some(bmp) => render::draw_bitmap(dc, bmp, dest)?,
                None => render::fill_round(dc, dest, pal().icon_slot_radius * s, pal().pill_bg)?,
            }
            if g.windows.iter().any(|w| w.is_elevated) {
                let b = 12.0 * s;
                let badge = rectf(dest.right - b + 2.0 * s, dest.bottom - b + 2.0 * s, b, b);
                render::fill_round(dc, badge, b / 2.0, (pal().glass_bg & 0xFFFFFF00) | 0xE6)?;
                render::glyph(dc, glyph::ADMIN, &self.fonts.icon_small, badge, pal().warning)?;
            }

            if g.is_running() {
                let x = 2.0 * s;
                let (w, h, c) = if focused == Some(*gi) {
                    (2.0 * s, 14.0 * s, pal().accent)
                } else {
                    (2.0 * s, if g.windows.len() > 1 { 7.0 } else { 3.0 } * s, pal().running_dot)
                };
                render::fill_round(dc, rectf(x, cy - h / 2.0, w, h), w / 2.0, c)?;
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
        let available = (self.monitor.monitor.right - self.dock.rect.right - px(TOOLTIP_GAP + 8.0, s)).max(1);
        let w = ((self.gfx.measure(text, &self.fonts.pill) + 2.0 * TOOLTIP_PAD_X * s).ceil() as i32).min(available);
        let h = px(TOOLTIP_H, s);
        let x = self.dock.rect.right + px(TOOLTIP_GAP, s);
        let cy = self.dock.rect.top + ((anchor.top + anchor.bottom) / 2.0) as i32;
        let y = (cy - h / 2)
            .clamp(self.monitor.monitor.top, (self.monitor.monitor.bottom - h).max(self.monitor.monitor.top));
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
            Some(DockHit::Scroll(_)) => self.hide_tooltip(),
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
            (DockHit::Scroll(step), Button::Left) => {
                self.scroll_dock(step);
                None
            }
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
        } else {
            self.present_window(hwnd, elevated);
        }
    }

    fn launch_group(&mut self, gi: usize) {
        let Some(g) = self.dock_model.groups().get(gi).cloned() else { return };
        if !self.launch_app(&g.exe_path, g.aumid.as_deref(), &g.id) {
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
            x: self.dock.rect.right + px(8.0, self.scale()),
            y: self.dock.rect.top + slot.top as i32,
            above: false,
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

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn rail_keeps_icons_inside_thin_bounds_at_supported_scales() {
        for scale in [1.0, 1.25, 1.5, 2.0] {
            let l = build_rail(scale, 3, 10, 0, 10, true);
            assert_eq!(l.width, (48.0 * scale).round());
            for (_, r) in &l.apps {
                assert!(r.left >= 0.0 && r.right <= l.width);
                assert!(r.top >= l.start.bottom && r.bottom <= l.height);
            }
            let media = l.media.unwrap();
            assert!(media.play.bottom < l.height);
            assert!(inside(&media.pill, media.art.left, media.art.top));
        }
    }
    #[test]
    fn overflow_navigation_can_reach_every_app_without_losing_pin_order() {
        let mut seen = std::collections::HashSet::new();
        for offset in 0..=95 {
            let l = build_rail(1.0, 3, 100, offset, 5, false);
            assert_eq!(l.navigation.len(), 2);
            assert!(l.height < 400.0);
            for (index, _) in l.apps {
                seen.insert(index);
            }
        }
        assert_eq!(seen.len(), 100);
        let l = build_rail(1.0, 3, 3, 0, 3, false);
        assert!(l.navigation.is_empty());
        assert_eq!(l.apps.iter().map(|(i, _)| *i).collect::<Vec<_>>(), [0, 1, 2]);
    }
}
