//! winshell: the shell overlay (PLAN.md section 13).
//!
//! M1: invisible click-through AppBars (`TopReserve` / `BottomReserve`), the
//! blurred `TopBar` and placeholder `Dock` laid out inside them, and the
//! emergency hotkey Ctrl+Shift+Alt+Esc (F12 fallback).
//!
//! M2: the top bar's left zone shows the user handle and the live active
//! window icon + title (WinEvent hooks, no polling); the right zone shows a
//! clock that updates exactly on the minute boundary with a one-shot timer and
//! stays correct across sleep/resume and manual time changes.
//!
//! M3: the dock lists running apps, one slot per app (AUMID or exe path),
//! tracked by WinEvent hooks with full rescans only on startup, resume,
//! display change and Explorer restart. Icons come from the shell on the asset
//! thread and live in a bounded LRU of GPU bitmaps. Indicators: running dot,
//! multi-window dots, focused accent pill.
//!
//! The native taskbar is not touched until M5.

#![cfg_attr(all(windows, not(debug_assertions)), windows_subsystem = "windows")]

#[cfg(not(windows))]
fn main() {
    eprintln!("winshell only runs on Windows 11 (22H2 or newer).");
}

#[cfg(windows)]
fn main() {
    app::main();
}

#[cfg(windows)]
mod app {
    use std::cell::{Cell, RefCell};
    use std::collections::{HashMap, HashSet};
    use std::sync::atomic::{AtomicIsize, Ordering};

    use windows::Win32::Foundation::*;
    use windows::Win32::Graphics::Direct2D::Common::D2D_RECT_F;
    use windows::Win32::Graphics::Direct2D::{ID2D1Bitmap1, ID2D1DeviceContext};
    use windows::Win32::Graphics::DirectWrite::*;
    use windows::Win32::Graphics::Gdi::{BeginPaint, EndPaint, PAINTSTRUCT};
    use windows::Win32::System::Com::{COINIT_APARTMENTTHREADED, COINIT_DISABLE_OLE1DDE, CoInitializeEx};
    use windows::Win32::UI::Shell::{ABN_FULLSCREENAPP, ABN_POSCHANGED};
    use windows::Win32::UI::WindowsAndMessaging::*;
    use windows::core::{PCWSTR, Result, w};

    use winshell::appbar::{self, AppBar, Edge, ReserveMode, WM_APPBAR_CALLBACK};
    use winshell::apps;
    use winshell::assets::{AssetLoader, AssetRequest, WM_SHELL_ASSET};
    use winshell::clock::{self, Clock};
    use winshell::dock_model::DockModel;
    use winshell::dwm::{self, Backdrop, Corners, DwmStyle};
    use winshell::foreground::{self, ForegroundTracker, Kind, WM_SHELL_FOREGROUND, WM_SHELL_TITLE};
    use winshell::instance::SingleInstance;
    use winshell::lru::Lru;
    use winshell::mem::MemSnapshot;
    use winshell::render::{self, Gfx, WindowSurface, rectf};
    use winshell::util::truncate_chars;
    use winshell::win::{self, MonitorGeometry, px};
    use winshell::winevents::{WM_SHELL_WINDOWS, WindowEventHooks};
    use winshell::{hotkey, icon, log};

    const TOPBAR_CLASS: PCWSTR = w!("WinShellTheme_TopBarClass");
    const WINDOW_CLASS: PCWSTR = w!("WinShellTheme_Window");

    const TIMER_CLOCK: usize = 1;

    // Layout constants at 100% scale (PLAN.md section 2).
    const TOP_RESERVE: f32 = 44.0;
    const BOTTOM_RESERVE: f32 = 74.0;
    const BAR_MARGIN: f32 = 8.0;
    const BAR_HEIGHT: f32 = 36.0;
    const BAR_EDGE_INSET: f32 = 4.0;
    const PILL_HEIGHT: f32 = 28.0;
    const PILL_RADIUS: f32 = 14.0;
    const PILL_PAD_X: f32 = 10.0;
    const PILL_GAP: f32 = 6.0;
    const DIVIDER_HEIGHT: f32 = 16.0;
    const ACTIVE_ICON: f32 = 16.0;
    const ICON_TEXT_GAP: f32 = 6.0;
    const TITLE_MAX_CHARS: usize = 40;
    const DOCK_HEIGHT: f32 = 64.0;
    const DOCK_MARGIN_BOTTOM: f32 = 10.0;
    const DOCK_PAD_X: f32 = 10.0;
    const DOCK_SLOT: f32 = 48.0;
    const DOCK_SLOT_GAP: f32 = 4.0;
    const DOCK_SLOT_RADIUS: f32 = 12.0;
    const DOCK_SEPARATOR_HEIGHT: f32 = 32.0;
    const DOCK_SEPARATOR_MARGIN: f32 = 6.0;
    /// Drawn icon size inside the 48 px slot.
    const DOCK_ICON: f32 = 44.0;
    /// Hover magnification (M4). Icons are fetched big enough to scale up cleanly.
    const DOCK_HOVER_SCALE: f32 = 1.15;
    /// PLAN.md section 2: icons cached at 64 x 64 at least; larger at high DPI.
    const DOCK_ICON_FETCH_MIN: u32 = 64;
    const ICON_CACHE_BYTES: usize = 8 * 1024 * 1024;
    const INDICATOR_GAP: f32 = 2.0;
    const RUNNING_DOT: f32 = 4.0;
    const MULTI_DOT: f32 = 3.0;
    const FOCUS_PILL_W: f32 = 12.0;
    const FOCUS_PILL_H: f32 = 4.0;

    // Theme tokens (PLAN.md section 3, dark). Themes become configurable in M8.
    const GLASS_BG: u32 = 0x14161CB7;
    const PILL_BG: u32 = 0x1E212A99;
    const BORDER: u32 = 0xFFFFFF1F;
    const TEXT_PRIMARY: u32 = 0xF2F4F8FF;
    const TEXT_SECONDARY: u32 = 0xA8AEBBFF;
    const TEXT_DISABLED: u32 = 0x6A7080FF;
    const ACCENT: u32 = 0x6EA8FEFF;
    const RUNNING_DOT_COLOR: u32 = 0xFFFFFF99;
    const START_GLYPH_COLOR: u32 = 0xF2F4F8E6;

    /// Reservation HWNDs for the panic hook, which cannot borrow the app state.
    static TOP_RESERVE_HWND: AtomicIsize = AtomicIsize::new(0);
    static BOTTOM_RESERVE_HWND: AtomicIsize = AtomicIsize::new(0);

    thread_local! {
        static APP: RefCell<Option<App>> = const { RefCell::new(None) };
        static PLAN_MODE: Cell<bool> = const { Cell::new(false) };
        static TASKBAR_CREATED: Cell<u32> = const { Cell::new(0) };
    }

    struct Options {
        reserve: ReserveMode,
        style: DwmStyle,
    }

    impl Options {
        fn from_args() -> std::result::Result<Self, String> {
            let mut o = Options { reserve: ReserveMode::Layered, style: DwmStyle::default() };
            let mut args = std::env::args().skip(1);
            while let Some(a) = args.next() {
                let mut val = |n: &str| args.next().ok_or(format!("{n} needs a value"));
                match a.as_str() {
                    "--reserve" => o.reserve = ReserveMode::parse(&val("--reserve")?).ok_or("bad --reserve")?,
                    "--backdrop" => o.style.backdrop = Backdrop::parse(&val("--backdrop")?).ok_or("bad --backdrop")?,
                    "--corners" => o.style.corners = Corners::parse(&val("--corners")?).ok_or("bad --corners")?,
                    "--autostart" => {}
                    other => return Err(format!("unknown argument {other}")),
                }
            }
            Ok(o)
        }
    }

    struct Visible {
        hwnd: HWND,
        surface: WindowSurface,
        rect: RECT,
    }

    struct Fonts {
        /// Active window title, 13 px, trims with "…".
        title: IDWriteTextFormat,
        /// Pills (user handle, clock), 12 px.
        pill: IDWriteTextFormat,
        scale: f32,
    }

    /// What the left zone currently shows.
    struct Active {
        hwnd: HWND,
        kind: Kind,
        title: String,
        icon: Option<ID2D1Bitmap1>,
    }

    /// Top bar geometry in surface pixels, recomputed before each draw.
    #[derive(Clone, Copy, Default)]
    struct BarLayout {
        user: D2D_RECT_F,
        divider: D2D_RECT_F,
        active: D2D_RECT_F,
        icon: D2D_RECT_F,
        title: D2D_RECT_F,
        clock: D2D_RECT_F,
    }

    struct App {
        opts: Options,
        gfx: Gfx,
        fonts: Fonts,
        monitor: MonitorGeometry,
        top_reserve: AppBar,
        bottom_reserve: AppBar,
        top_bar: Visible,
        dock: Visible,
        user_handle: String,
        fullscreen: bool,
        clock: Clock,
        clock_text: String,
        tracker: ForegroundTracker,
        active: Active,
        bar: BarLayout,
        first_tick_logged: bool,
        // M3: running apps in the dock.
        dock_model: DockModel,
        window_hooks: WindowEventHooks,
        assets: AssetLoader,
        icons: Lru<ID2D1Bitmap1>,
        icon_requested: HashSet<String>,
        app_names: HashMap<String, String>,
        /// Bottom reservation band and usable horizontal span, from `layout`.
        bottom_band: RECT,
        span: (i32, i32),
    }

    pub fn main() {
        let dir = log::app_dir();
        log::init(dir.join("winshell.log"), cfg!(debug_assertions));
        install_panic_hook(dir.join("crash.log"));
        win::enable_per_monitor_dpi();

        let opts = match Options::from_args() {
            Ok(o) => o,
            Err(e) => {
                log!("{e}");
                std::process::exit(2);
            }
        };

        let Some(_instance) = SingleInstance::acquire(w!("Local\\WinShellTheme.SingleInstance")) else {
            log!("another instance is already running; exiting");
            return;
        };

        log!("winshell M3 starting (reserve={}, {:?})", opts.reserve.name(), opts.style);
        if let Err(e) = run(opts) {
            log!("fatal: {e:?}");
            cleanup_raw();
            std::process::exit(1);
        }
    }

    fn install_panic_hook(path: std::path::PathBuf) {
        std::panic::set_hook(Box::new(move |info| {
            cleanup_raw();
            let line = format!("[{}] panic: {info}\n", log::timestamp());
            let _ = std::fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open(&path)
                .and_then(|mut f| std::io::Write::write_all(&mut f, line.as_bytes()));
            log::write_line(&format!("panic: {info}"));
        }));
    }

    /// Remove both AppBars using only the stored HWNDs (panic / fatal paths).
    fn cleanup_raw() {
        for slot in [&TOP_RESERVE_HWND, &BOTTOM_RESERVE_HWND] {
            let h = slot.swap(0, Ordering::SeqCst);
            if h != 0 {
                appbar::remove_raw(HWND(h as _));
            }
        }
    }

    fn run(opts: Options) -> Result<()> {
        win::register_class(TOPBAR_CLASS, wndproc)?;
        win::register_class(WINDOW_CLASS, wndproc)?;
        PLAN_MODE.set(opts.reserve == ReserveMode::Plan);
        TASKBAR_CREATED.set(unsafe { RegisterWindowMessageW(w!("TaskbarCreated")) });

        let visible_ex = WS_EX_NOREDIRECTIONBITMAP | WS_EX_TOOLWINDOW | WS_EX_TOPMOST | WS_EX_NOACTIVATE;
        let zero = win::rect(0, 0, 1, 1);

        // The hotkey is registered before anything can get stuck on screen.
        let top_hwnd = win::create_popup(TOPBAR_CLASS, w!("WinShellTheme TopBar"), visible_ex, zero)?;
        match hotkey::register_emergency(top_hwnd) {
            Some(c) => log!("emergency hotkey: {c}"),
            None => log!("WARNING: emergency hotkey unavailable (Esc and F12 combinations both taken)"),
        }

        // UI thread is an STA (PLAN.md section 6); shell property stores need COM.
        unsafe {
            let _ = CoInitializeEx(None, COINIT_APARTMENTTHREADED | COINIT_DISABLE_OLE1DDE);
        }

        let gfx = Gfx::new()?;
        log!("graphics: shared devices created ({})", gfx.driver);

        let dock_hwnd = win::create_popup(WINDOW_CLASS, w!("WinShellTheme Dock"), visible_ex, zero)?;
        let top_res_hwnd =
            win::create_popup(WINDOW_CLASS, w!("WinShellTheme TopReserve"), opts.reserve.ex_style(), zero)?;
        let bot_res_hwnd =
            win::create_popup(WINDOW_CLASS, w!("WinShellTheme BottomReserve"), opts.reserve.ex_style(), zero)?;
        TOP_RESERVE_HWND.store(top_res_hwnd.0 as isize, Ordering::SeqCst);
        BOTTOM_RESERVE_HWND.store(bot_res_hwnd.0 as isize, Ordering::SeqCst);

        for h in [top_hwnd, dock_hwnd] {
            let rep = dwm::apply(h, opts.style);
            if !rep.all_ok() {
                log!("dwm attributes partly rejected: {}", rep.describe());
            }
        }

        let mut top_reserve = AppBar::new(top_res_hwnd, Edge::Top);
        let mut bottom_reserve = AppBar::new(bot_res_hwnd, Edge::Bottom);
        top_reserve.register();
        bottom_reserve.register();

        let monitor = MonitorGeometry::primary();
        let fonts = make_fonts(&gfx, monitor.scale())?;
        let top_bar = Visible { hwnd: top_hwnd, surface: WindowSurface::new(&gfx, top_hwnd)?, rect: zero };
        let dock = Visible { hwnd: dock_hwnd, surface: WindowSurface::new(&gfx, dock_hwnd)?, rect: zero };

        let user_handle =
            std::env::var("USERNAME").map(|u| format!("@{}", u.to_lowercase())).unwrap_or_else(|_| "@you".into());

        let mut app = App {
            opts,
            gfx,
            fonts,
            monitor,
            top_reserve,
            bottom_reserve,
            top_bar,
            dock,
            user_handle,
            fullscreen: false,
            clock: Clock::new(top_hwnd, TIMER_CLOCK),
            clock_text: clock::now_text(),
            tracker: ForegroundTracker::install(top_hwnd),
            active: Active { hwnd: HWND::default(), kind: Kind::Desktop, title: "Desktop".into(), icon: None },
            bar: BarLayout::default(),
            first_tick_logged: false,
            dock_model: DockModel::default(),
            window_hooks: WindowEventHooks::install(top_hwnd),
            assets: AssetLoader::start(top_hwnd),
            icons: Lru::new(ICON_CACHE_BYTES),
            icon_requested: HashSet::new(),
            app_names: HashMap::new(),
            bottom_band: zero,
            span: (0, 1),
        };
        app.rescan("startup");
        app.layout();
        app.opts.reserve.finish(top_res_hwnd);
        app.opts.reserve.finish(bot_res_hwnd);
        app.update_active(unsafe { GetForegroundWindow() });
        unsafe {
            let _ = ShowWindow(top_hwnd, SW_SHOWNOACTIVATE);
            let _ = ShowWindow(dock_hwnd, SW_SHOWNOACTIVATE);
        }
        app.redraw_all();
        app.clock.arm();
        app.refresh_clock(); // the minute may have turned since clock_text was built
        log!("ready; {}", MemSnapshot::now().describe());

        APP.with(|a| *a.borrow_mut() = Some(app));

        unsafe {
            let mut msg = MSG::default();
            while GetMessageW(&mut msg, None, 0, 0).as_bool() {
                let _ = TranslateMessage(&msg);
                DispatchMessageW(&msg);
            }
        }
        log!("message loop ended");
        Ok(())
    }

    fn make_fonts(gfx: &Gfx, scale: f32) -> Result<Fonts> {
        Ok(Fonts {
            title: gfx.text_format_ellipsis(w!("Segoe UI Variable"), 13.0 * scale, DWRITE_FONT_WEIGHT_NORMAL)?,
            pill: gfx.text_format(w!("Segoe UI Variable"), 12.0 * scale, DWRITE_FONT_WEIGHT_NORMAL)?,
            scale,
        })
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
            render::fill_round(dc, r, 1.5 * s, START_GLYPH_COLOR)?;
        }
        Ok(())
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

    impl App {
        /// Reserve the edges, then place the visible windows inside the granted bands.
        fn layout(&mut self) {
            self.monitor = MonitorGeometry::primary();
            let s = self.monitor.scale();
            if (self.fonts.scale - s).abs() > f32::EPSILON {
                match make_fonts(&self.gfx, s) {
                    Ok(f) => self.fonts = f,
                    Err(e) => log!("font rebuild failed: {e:?}"),
                }
                // Icon pixels are rendered at the exact device size; redo them for the new scale.
                self.active.icon = self.load_icon(self.active.hwnd, self.active.kind);
                self.icons.clear();
                self.icon_requested.clear();
                self.sync_assets();
            }

            let top = self.top_reserve.set_pos(self.monitor.monitor, px(TOP_RESERVE, s));
            let bottom = self.bottom_reserve.set_pos(self.monitor.monitor, px(BOTTOM_RESERVE, s));

            // Horizontal span: the work area excludes a left/right native taskbar.
            let work = win::work_area();
            let left = work.left.max(self.monitor.monitor.left);
            let right = work.right.min(self.monitor.monitor.right);
            let m = px(BAR_MARGIN, s);

            let bar = win::rect(left + m, top.top + m, (right - left - 2 * m).max(1), px(BAR_HEIGHT, s));

            self.bottom_band = bottom;
            self.span = (left, right);
            let dock = self.dock_rect();

            log!(
                "layout: dpi {} top reserve {} bottom reserve {} bar {} dock {}",
                self.monitor.dpi,
                win::rect_str(&top),
                win::rect_str(&bottom),
                win::rect_str(&bar),
                win::rect_str(&dock)
            );

            for (v, r) in [(&mut self.top_bar, bar), (&mut self.dock, dock)] {
                v.rect = r;
                win::move_window(v.hwnd, r, true);
                if let Err(e) = v.surface.resize(&self.gfx, (r.right - r.left) as u32, (r.bottom - r.top) as u32) {
                    log!("surface resize failed: {e:?}");
                }
            }
        }

        // ------------------------------------------------------------------
        // Active window (left zone)
        // ------------------------------------------------------------------

        fn load_icon(&self, hwnd: HWND, kind: Kind) -> Option<ID2D1Bitmap1> {
            if kind != Kind::App {
                return None;
            }
            let size = px(ACTIVE_ICON, self.monitor.scale()).max(1) as u32;
            let hicon = icon::window_icon(hwnd, true)?;
            let px = icon::render(hicon, size)?;
            self.gfx.bitmap_from_bgra(px.width, px.height, &px.pixels).ok()
        }

        /// Foreground changed: show the new window (or keep the old one for shell chrome).
        /// Returns true if the left zone changed.
        fn update_active(&mut self, hwnd: HWND) -> bool {
            let kind = foreground::classify(hwnd);
            match kind {
                Kind::Ignore => false,
                Kind::Desktop => {
                    if self.active.kind == Kind::Desktop {
                        return false;
                    }
                    self.tracker.follow(HWND::default());
                    self.active = Active { hwnd, kind, title: "Desktop".into(), icon: None };
                    true
                }
                Kind::App => {
                    let title = foreground::title(hwnd);
                    if hwnd == self.active.hwnd && title == self.active.title {
                        return false;
                    }
                    self.tracker.follow(hwnd);
                    let icon =
                        if hwnd == self.active.hwnd { self.active.icon.take() } else { self.load_icon(hwnd, kind) };
                    self.active = Active { hwnd, kind, title, icon };
                    true
                }
            }
        }

        fn on_foreground(&mut self) {
            self.tracker.ack_foreground();
            let fg = unsafe { GetForegroundWindow() };
            if self.update_active(fg) {
                self.redraw_bar(None);
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

        // ------------------------------------------------------------------
        // Dock: running apps
        // ------------------------------------------------------------------

        /// Full window rescan. Only on startup, resume, display change and Explorer restart.
        fn rescan(&mut self, why: &str) {
            self.dock_model.replace_all(apps::enumerate());
            let fg = apps::root_owner(unsafe { GetForegroundWindow() });
            self.dock_model.set_focused(fg.0 as isize);
            let windows: usize = self.dock_model.groups().iter().map(|g| g.windows.len()).sum();
            log!("window rescan ({why}): {} apps, {windows} windows", self.dock_model.groups().len());
            self.sync_assets();
        }

        /// Queued window events: re-evaluate only the windows that changed.
        fn on_window_events(&mut self) {
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
            let fg = apps::root_owner(unsafe { GetForegroundWindow() });
            changed |= self.dock_model.set_focused(fg.0 as isize);
            if changed {
                self.dock_changed();
            }
        }

        fn dock_changed(&mut self) {
            self.sync_assets();
            self.relayout_dock();
            self.redraw_dock();
        }

        fn icon_fetch_size(&self) -> u32 {
            let want = (DOCK_ICON * self.monitor.scale() * DOCK_HOVER_SCALE).ceil() as u32;
            want.max(DOCK_ICON_FETCH_MIN)
        }

        /// Fill in known names and ask the asset thread for icons not yet cached.
        fn sync_assets(&mut self) {
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

        fn on_assets(&mut self) {
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

        /// How many running apps fit beside the Start slot within the usable span.
        fn visible_app_count(&self) -> usize {
            let s = self.monitor.scale();
            let avail = (self.span.1 - self.span.0 - 2 * px(BAR_MARGIN, s)) as f32;
            let n = self.dock_model.groups().len();
            (0..=n).rev().find(|&k| self.dock_width(k) <= avail).unwrap_or(0)
        }

        fn dock_width(&self, apps: usize) -> f32 {
            let s = self.monitor.scale();
            let mut w = 2.0 * DOCK_PAD_X * s + DOCK_SLOT * s; // padding + Start
            if apps > 0 {
                w += 2.0 * DOCK_SEPARATOR_MARGIN * s + 1.0;
                w += apps as f32 * DOCK_SLOT * s + (apps - 1) as f32 * DOCK_SLOT_GAP * s;
            }
            w.ceil()
        }

        fn dock_rect(&self) -> RECT {
            let s = self.monitor.scale();
            let (left, right) = self.span;
            let dock_w = self.dock_width(self.visible_app_count()) as i32;
            let dock_h = px(DOCK_HEIGHT, s);
            win::rect(
                left + (right - left - dock_w) / 2,
                self.bottom_band.bottom - px(DOCK_MARGIN_BOTTOM, s) - dock_h,
                dock_w,
                dock_h,
            )
        }

        /// The dock window resizes as apps come and go; the reservation never does.
        fn relayout_dock(&mut self) {
            let r = self.dock_rect();
            if r == self.dock.rect {
                return;
            }
            self.dock.rect = r;
            win::move_window(self.dock.hwnd, r, true);
            if let Err(e) = self.dock.surface.resize(&self.gfx, (r.right - r.left) as u32, (r.bottom - r.top) as u32) {
                log!("dock surface resize failed: {e:?}");
            }
        }

        fn redraw_dock(&mut self) {
            if let Err(e) = self.draw_dock().and_then(|_| self.gfx.commit()) {
                self.on_draw_error(e);
            }
        }

        fn on_title(&mut self) {
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

        // ------------------------------------------------------------------
        // Clock (right zone)
        // ------------------------------------------------------------------

        fn on_clock_timer(&mut self) {
            self.clock.on_timer(); // KillTimer, then re-arm for the next boundary
            self.refresh_clock();
            if !self.first_tick_logged {
                self.first_tick_logged = true;
                log!("first minute tick; {}", MemSnapshot::now().describe());
            }
        }

        /// Re-read the time and redraw only the clock pill if its width is unchanged.
        fn refresh_clock(&mut self) {
            let text = clock::now_text();
            if text == self.clock_text {
                return;
            }
            let old = self.bar;
            self.clock_text = text;
            let new = self.compute_bar_layout();
            let same_width = ((new.clock.right - new.clock.left) - (old.clock.right - old.clock.left)).abs() < 0.5;
            if same_width {
                self.redraw_bar(Some(union(rect_to_pixels(old.clock), rect_to_pixels(new.clock))));
            } else {
                self.redraw_bar(None); // the title may gain or lose room
            }
        }

        /// Time base changed (WM_TIMECHANGE, resume): re-arm and redraw.
        fn resync_clock(&mut self, why: &str) {
            log!("clock resync ({why})");
            self.clock.arm();
            self.refresh_clock();
        }

        // ------------------------------------------------------------------
        // Drawing
        // ------------------------------------------------------------------

        fn compute_bar_layout(&self) -> BarLayout {
            let s = self.monitor.scale();
            let bw = self.top_bar.surface.width as f32;
            let bh = self.top_bar.surface.height as f32;
            let pill_h = PILL_HEIGHT * s;
            let y = ((bh - pill_h) / 2.0).round();
            let pad = PILL_PAD_X * s;
            let inset = BAR_EDGE_INSET * s;

            let user_w = (self.gfx.measure(&self.user_handle, &self.fonts.pill) + 2.0 * pad).ceil();
            let user = rectf(inset, y, user_w, pill_h);

            let div_x = (user.right + PILL_GAP * s).round();
            let div_h = DIVIDER_HEIGHT * s;
            let divider = rectf(div_x, ((bh - div_h) / 2.0).round(), 1.0, div_h);

            let clock_w = (self.gfx.measure(&self.clock_text, &self.fonts.pill) + 2.0 * pad).ceil();
            let clock = rectf(bw - inset - clock_w, y, clock_w, pill_h);

            let active_x = divider.right + PILL_GAP * s;
            let icon_sz = px(ACTIVE_ICON, s) as f32;
            let has_icon = self.active.kind == Kind::App;
            let lead = pad + if has_icon { icon_sz + ICON_TEXT_GAP * s } else { 0.0 };
            let title_room = (clock.left - 2.0 * PILL_GAP * s - active_x - lead - pad).max(0.0);
            let title_w = self.gfx.measure(&self.display_title(), &self.fonts.title).ceil().min(title_room);
            let active = rectf(active_x, y, lead + title_w + pad, pill_h);
            let icon = rectf((active_x + pad).round(), (y + (pill_h - icon_sz) / 2.0).round(), icon_sz, icon_sz);
            let title = rectf(active_x + lead, y, title_w, pill_h);

            BarLayout { user, divider, active, icon, title, clock }
        }

        fn display_title(&self) -> String {
            truncate_chars(&self.active.title, TITLE_MAX_CHARS)
        }

        fn redraw_all(&mut self) {
            self.redraw_bar(None);
            self.redraw_dock();
        }

        /// Redraw the top bar (`dirty` = surface pixels, `None` = everything) and commit.
        fn redraw_bar(&mut self, dirty: Option<RECT>) {
            self.bar = self.compute_bar_layout();
            let result =
                self.top_bar.surface.draw_region(dirty, |dc| self.paint_bar(dc)).and_then(|_| self.gfx.commit());
            if let Err(e) = result {
                self.on_draw_error(e);
            }
        }

        fn paint_bar(&self, dc: &ID2D1DeviceContext) -> Result<()> {
            let s = self.monitor.scale();
            let radius = self.opts.style.corners.approx_radius() * s;
            let (bw, bh) = (self.top_bar.surface.width as f32, self.top_bar.surface.height as f32);
            let l = &self.bar;
            let pill_r = PILL_RADIUS * s;
            let pad = PILL_PAD_X * s;

            let full = rectf(0.0, 0.0, bw, bh);
            render::fill_round(dc, full, radius, GLASS_BG)?;
            render::stroke_round(dc, full, radius, BORDER)?;

            // Left zone: user pill, divider, active window pill.
            render::fill_round(dc, l.user, pill_r, PILL_BG)?;
            let user_text = D2D_RECT_F { left: l.user.left + pad, right: l.user.right - pad + 1.0, ..l.user };
            render::text(dc, &self.user_handle, &self.fonts.pill, user_text, TEXT_SECONDARY)?;
            render::fill_round(dc, l.divider, 0.0, BORDER)?;

            render::fill_round(dc, l.active, pill_r, PILL_BG)?;
            if self.active.kind == Kind::App {
                match &self.active.icon {
                    Some(bmp) => render::draw_bitmap(dc, bmp, l.icon)?,
                    None => render::stroke_round(dc, l.icon, 3.0 * s, TEXT_DISABLED)?,
                }
            }
            let title_color = if self.active.kind == Kind::Desktop { TEXT_SECONDARY } else { TEXT_PRIMARY };
            let title_box = D2D_RECT_F { right: l.title.right + 1.0, ..l.title };
            render::text(dc, &self.display_title(), &self.fonts.title, title_box, title_color)?;

            // Right zone: clock pill (status pills arrive in M6).
            render::fill_round(dc, l.clock, pill_r, PILL_BG)?;
            let clock_text = D2D_RECT_F { left: l.clock.left + pad, right: l.clock.right - pad + 1.0, ..l.clock };
            render::text(dc, &self.clock_text, &self.fonts.pill, clock_text, TEXT_PRIMARY)?;
            Ok(())
        }

        fn draw_dock(&mut self) -> Result<()> {
            let s = self.monitor.scale();
            let n = self.visible_app_count();
            // Look icons up first (the LRU touch needs &mut); COM pointers are cheap to clone.
            let ids: Vec<String> = self.dock_model.groups().iter().take(n).map(|g| g.id.clone()).collect();
            let bitmaps: Vec<Option<ID2D1Bitmap1>> = ids.iter().map(|id| self.icons.get(id).cloned()).collect();
            let this = &*self;
            let radius = this.opts.style.corners.approx_radius() * s;
            let (dw, dh) = (this.dock.surface.width as f32, this.dock.surface.height as f32);
            let focused = this.dock_model.focused();
            this.dock.surface.draw(|dc| {
                let full = rectf(0.0, 0.0, dw, dh);
                render::fill_round(dc, full, radius, GLASS_BG)?;
                render::stroke_round(dc, full, radius, BORDER)?;

                let slot = DOCK_SLOT * s;
                let y = ((dh - slot) / 2.0).round();
                let mut x = DOCK_PAD_X * s;

                // Start button (opens the native Start menu from M4).
                paint_start_glyph(dc, rectf(x, y, slot, slot), s)?;
                x += slot;

                if n > 0 {
                    x += DOCK_SEPARATOR_MARGIN * s;
                    let sep_h = DOCK_SEPARATOR_HEIGHT * s;
                    render::fill_round(dc, rectf(x.round(), ((dh - sep_h) / 2.0).round(), 1.0, sep_h), 0.0, BORDER)?;
                    x += 1.0 + DOCK_SEPARATOR_MARGIN * s;
                }

                for (i, g) in this.dock_model.groups().iter().take(n).enumerate() {
                    let sx = x.round();
                    let icon_sz = (DOCK_ICON * s).round();
                    let inset = ((slot - icon_sz) / 2.0).round();
                    let dest = rectf(sx + inset, y + inset, icon_sz, icon_sz);
                    match &bitmaps[i] {
                        Some(bmp) => render::draw_bitmap(dc, bmp, dest)?,
                        None => render::fill_round(dc, dest, DOCK_SLOT_RADIUS * s, PILL_BG)?,
                    }

                    // Below the slot: focused pill, else multi-window dots, else running dot.
                    let iy = y + slot + INDICATOR_GAP * s;
                    let cx = sx + slot / 2.0;
                    if focused == Some(i) {
                        let (w, h) = (FOCUS_PILL_W * s, FOCUS_PILL_H * s);
                        render::fill_round(dc, rectf(cx - w / 2.0, iy, w, h), h / 2.0, ACCENT)?;
                    } else if g.windows.len() > 1 {
                        let d = MULTI_DOT * s;
                        let gap = INDICATOR_GAP * s;
                        let x0 = cx - d - gap / 2.0;
                        render::fill_round(dc, rectf(x0, iy, d, d), d / 2.0, RUNNING_DOT_COLOR)?;
                        render::fill_round(dc, rectf(x0 + d + gap, iy, d, d), d / 2.0, RUNNING_DOT_COLOR)?;
                    } else {
                        let d = RUNNING_DOT * s;
                        render::fill_round(dc, rectf(cx - d / 2.0, iy, d, d), d / 2.0, RUNNING_DOT_COLOR)?;
                    }
                    x += slot + DOCK_SLOT_GAP * s;
                }
                Ok(())
            })
        }

        fn on_draw_error(&mut self, e: windows::core::Error) {
            log!("draw failed: {e:?}");
            if self.gfx.is_lost() {
                self.rebuild_graphics();
            }
        }

        /// Device loss: rebuild the shared devices and every window's composition tree, redraw once.
        fn rebuild_graphics(&mut self) {
            log!("graphics device lost; rebuilding");
            let result = (|| -> Result<()> {
                let gfx = Gfx::new()?;
                let fonts = make_fonts(&gfx, self.monitor.scale())?;
                for v in [&mut self.top_bar, &mut self.dock] {
                    let mut surf = WindowSurface::new(&gfx, v.hwnd)?;
                    surf.resize(&gfx, (v.rect.right - v.rect.left) as u32, (v.rect.bottom - v.rect.top) as u32)?;
                    v.surface = surf;
                }
                self.gfx = gfx;
                self.fonts = fonts;
                self.active.icon = self.load_icon(self.active.hwnd, self.active.kind);
                // Dock bitmaps belonged to the old device: drop them and fetch again.
                self.icons.clear();
                self.icon_requested.clear();
                self.sync_assets();
                self.bar = self.compute_bar_layout();
                self.top_bar.surface.draw(|dc| self.paint_bar(dc))?;
                self.draw_dock()?;
                self.gfx.commit()
            })();
            if let Err(e) = result {
                log!("graphics rebuild failed: {e:?}");
            }
        }

        // ------------------------------------------------------------------
        // System events
        // ------------------------------------------------------------------

        fn set_fullscreen(&mut self, on: bool) {
            if self.fullscreen == on {
                return;
            }
            self.fullscreen = on;
            let cmd = if on { SW_HIDE } else { SW_SHOWNOACTIVATE };
            unsafe {
                let _ = ShowWindow(self.top_bar.hwnd, cmd);
                let _ = ShowWindow(self.dock.hwnd, cmd);
            }
            log!(
                "fullscreen app {}: bar and dock {}",
                if on { "opened" } else { "closed" },
                if on { "hidden" } else { "shown" }
            );
        }

        fn on_appbar(&mut self, from: HWND, wp: WPARAM, lp: LPARAM) {
            log!("appbar: {}", appbar::describe_notification(wp.0, lp));
            match wp.0 as u32 {
                ABN_POSCHANGED => {
                    self.layout();
                    self.redraw_all();
                }
                // Both reservations get this; only act on the top one to avoid doing it twice.
                ABN_FULLSCREENAPP if from == self.top_reserve.hwnd => self.set_fullscreen(lp.0 != 0),
                _ => {}
            }
        }

        /// Explorer restarted: its AppBar list is gone, so register again.
        fn on_taskbar_created(&mut self) {
            log!("Explorer restarted; re-registering AppBars");
            self.top_reserve.registered = false;
            self.bottom_reserve.registered = false;
            self.top_reserve.register();
            self.bottom_reserve.register();
            self.rescan("Explorer restart");
            self.layout();
            self.redraw_all();
        }

        /// Resume from sleep/hibernate: the clock is stale and the foreground may differ.
        fn on_resume(&mut self) {
            self.resync_clock("resume");
            self.update_active(unsafe { GetForegroundWindow() });
            self.redraw_bar(None);
            self.rescan("resume");
            self.relayout_dock();
            self.redraw_dock();
        }

        fn shutdown(&mut self) {
            log!("clean exit; {}", MemSnapshot::now().describe());
            self.tracker.uninstall();
            self.window_hooks.uninstall();
            self.assets.shutdown();
            self.clock.disarm();
            self.top_reserve.remove();
            self.bottom_reserve.remove();
            hotkey::unregister(self.top_bar.hwnd, hotkey::ID_EMERGENCY);
            TOP_RESERVE_HWND.store(0, Ordering::SeqCst);
            BOTTOM_RESERVE_HWND.store(0, Ordering::SeqCst);
            for h in [self.top_bar.hwnd, self.dock.hwnd, self.top_reserve.hwnd, self.bottom_reserve.hwnd] {
                unsafe {
                    let _ = DestroyWindow(h);
                }
            }
        }

        fn reserve(&self, hwnd: HWND) -> Option<&AppBar> {
            [&self.top_reserve, &self.bottom_reserve].into_iter().find(|r| r.hwnd == hwnd)
        }
    }

    /// Run `f` on the app state, or re-post the message if the state is busy
    /// (re-entrant delivery while an SHAppBarMessage call is in progress).
    fn with_app(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM, f: impl FnOnce(&mut App)) {
        APP.with(|a| match a.try_borrow_mut() {
            Ok(mut g) => {
                if let Some(app) = g.as_mut() {
                    f(app);
                }
            }
            Err(_) => unsafe {
                let _ = PostMessageW(Some(hwnd), msg, wp, lp);
            },
        });
    }

    /// Like `with_app`, but only for messages addressed to the top bar. Broadcasts
    /// (WM_TIMECHANGE, WM_POWERBROADCAST, ...) reach every top-level window; this
    /// makes sure each is handled once.
    fn with_app_on_bar(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM, f: impl FnOnce(&mut App)) {
        with_app(hwnd, msg, wp, lp, |a| {
            if a.top_bar.hwnd == hwnd {
                f(a);
            }
        });
    }

    fn is_reserve(hwnd: HWND) -> bool {
        let h = hwnd.0 as isize;
        h != 0 && (h == TOP_RESERVE_HWND.load(Ordering::Relaxed) || h == BOTTOM_RESERVE_HWND.load(Ordering::Relaxed))
    }

    /// WM_SETTINGCHANGE with lParam "intl": regional format changed.
    fn is_intl_change(lp: LPARAM) -> bool {
        if lp.0 == 0 {
            return false;
        }
        let s = unsafe { PCWSTR(lp.0 as *const u16).to_string() }.unwrap_or_default();
        s.eq_ignore_ascii_case("intl")
    }

    unsafe extern "system" fn wndproc(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
        if msg != 0 && msg == TASKBAR_CREATED.get() {
            with_app_on_bar(hwnd, msg, wp, lp, |a| a.on_taskbar_created());
            return LRESULT(0);
        }
        match msg {
            WM_NCHITTEST if PLAN_MODE.get() && is_reserve(hwnd) => LRESULT(HTTRANSPARENT as isize),
            WM_MOUSEACTIVATE => LRESULT(MA_NOACTIVATE as isize),
            WM_ERASEBKGND => LRESULT(1),
            WM_PAINT => {
                let mut ps = PAINTSTRUCT::default();
                unsafe {
                    BeginPaint(hwnd, &mut ps);
                    let _ = EndPaint(hwnd, &ps);
                }
                LRESULT(0)
            }
            WM_HOTKEY if wp.0 as i32 == hotkey::ID_EMERGENCY => {
                log!("emergency hotkey pressed");
                with_app(hwnd, msg, wp, lp, |a| a.shutdown());
                unsafe { PostQuitMessage(0) };
                LRESULT(0)
            }
            WM_TIMER if wp.0 == TIMER_CLOCK => {
                with_app_on_bar(hwnd, msg, wp, lp, |a| a.on_clock_timer());
                LRESULT(0)
            }
            WM_SHELL_FOREGROUND => {
                with_app_on_bar(hwnd, msg, wp, lp, |a| a.on_foreground());
                LRESULT(0)
            }
            WM_SHELL_TITLE => {
                with_app_on_bar(hwnd, msg, wp, lp, |a| a.on_title());
                LRESULT(0)
            }
            WM_SHELL_WINDOWS => {
                with_app_on_bar(hwnd, msg, wp, lp, |a| a.on_window_events());
                LRESULT(0)
            }
            WM_SHELL_ASSET => {
                with_app_on_bar(hwnd, msg, wp, lp, |a| a.on_assets());
                LRESULT(0)
            }
            WM_TIMECHANGE => {
                with_app_on_bar(hwnd, msg, wp, lp, |a| a.resync_clock("WM_TIMECHANGE"));
                LRESULT(0)
            }
            WM_SETTINGCHANGE if is_intl_change(lp) => {
                with_app_on_bar(hwnd, msg, wp, lp, |a| {
                    a.clock_text.clear(); // force a redraw in the new format
                    a.refresh_clock();
                });
                LRESULT(0)
            }
            WM_POWERBROADCAST => {
                if wp.0 as u32 == PBT_APMRESUMEAUTOMATIC {
                    with_app_on_bar(hwnd, msg, wp, lp, |a| a.on_resume());
                }
                LRESULT(1)
            }
            WM_APPBAR_CALLBACK => {
                with_app(hwnd, msg, wp, lp, |a| a.on_appbar(hwnd, wp, lp));
                LRESULT(0)
            }
            WM_ACTIVATE if is_reserve(hwnd) => {
                APP.with(|a| {
                    if let Ok(g) = a.try_borrow()
                        && let Some(r) = g.as_ref().and_then(|app| app.reserve(hwnd))
                    {
                        r.on_activate();
                    }
                });
                unsafe { DefWindowProcW(hwnd, msg, wp, lp) }
            }
            WM_WINDOWPOSCHANGED if is_reserve(hwnd) => {
                APP.with(|a| {
                    if let Ok(g) = a.try_borrow()
                        && let Some(r) = g.as_ref().and_then(|app| app.reserve(hwnd))
                    {
                        r.on_window_pos_changed();
                    }
                });
                unsafe { DefWindowProcW(hwnd, msg, wp, lp) }
            }
            WM_DPICHANGED | WM_DISPLAYCHANGE => {
                with_app_on_bar(hwnd, msg, wp, lp, |a| {
                    if msg == WM_DISPLAYCHANGE {
                        a.rescan("display change");
                    }
                    a.layout();
                    a.redraw_all();
                });
                LRESULT(0)
            }
            WM_CLOSE => LRESULT(0),
            _ => unsafe { DefWindowProcW(hwnd, msg, wp, lp) },
        }
    }
}
