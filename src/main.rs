//! winshell: M1 scaffolding (PLAN.md section 13, milestone 1, steps 2 and 3).
//!
//! * `TopReserve` / `BottomReserve`: invisible click-through AppBars that keep
//!   maximised windows off the top 44 px and bottom 74 px.
//! * `TopBar` / `Dock`: visible blurred windows laid out inside those bands,
//!   respecting the native taskbar edge (read, never changed, in M1).
//! * Emergency hotkey Ctrl+Shift+Alt+Esc (F12 fallback) registered first; it
//!   removes the AppBars and exits.
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
    use std::sync::atomic::{AtomicIsize, Ordering};

    use windows::Win32::Foundation::*;
    use windows::Win32::Graphics::DirectWrite::*;
    use windows::Win32::Graphics::Gdi::{BeginPaint, EndPaint, PAINTSTRUCT};
    use windows::Win32::UI::Shell::{ABN_FULLSCREENAPP, ABN_POSCHANGED};
    use windows::Win32::UI::WindowsAndMessaging::*;
    use windows::core::{PCWSTR, Result, w};

    use winshell::appbar::{self, AppBar, Edge, ReserveMode, WM_APPBAR_CALLBACK};
    use winshell::dwm::{self, Backdrop, Corners, DwmStyle};
    use winshell::instance::SingleInstance;
    use winshell::mem::MemSnapshot;
    use winshell::render::{self, Gfx, WindowSurface, rectf};
    use winshell::win::{self, MonitorGeometry, px};
    use winshell::{hotkey, log};

    const TOPBAR_CLASS: PCWSTR = w!("WinShellTheme_TopBarClass");
    const WINDOW_CLASS: PCWSTR = w!("WinShellTheme_Window");

    // Layout constants at 100% scale (PLAN.md section 2).
    const TOP_RESERVE: f32 = 44.0;
    const BOTTOM_RESERVE: f32 = 74.0;
    const BAR_MARGIN: f32 = 8.0;
    const BAR_HEIGHT: f32 = 36.0;
    const DOCK_HEIGHT: f32 = 64.0;
    const DOCK_MARGIN_BOTTOM: f32 = 10.0;
    const DOCK_PAD_X: f32 = 10.0;
    const DOCK_SLOT: f32 = 48.0;
    const DOCK_SLOT_GAP: f32 = 4.0;
    const PLACEHOLDER_SLOTS: usize = 5;

    // Theme tokens used in M1 (PLAN.md section 3, dark).
    const GLASS_BG: u32 = 0x14161CB7;
    const PILL_BG: u32 = 0x1E212A99;
    const BORDER: u32 = 0xFFFFFF1F;
    const TEXT_PRIMARY: u32 = 0xF2F4F8FF;
    const TEXT_SECONDARY: u32 = 0xA8AEBBFF;

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
        bar: IDWriteTextFormat,
        pill: IDWriteTextFormat,
        scale: f32,
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
        hotkey_combo: Option<&'static str>,
        fullscreen: bool,
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

        log!("winshell M1 starting (reserve={}, {:?})", opts.reserve.name(), opts.style);
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

        // Step 3 first: the hotkey is registered before anything can get stuck on screen.
        let top_hwnd = win::create_popup(TOPBAR_CLASS, w!("WinShellTheme TopBar"), visible_ex, zero)?;
        let hotkey_combo = hotkey::register_emergency(top_hwnd);
        match hotkey_combo {
            Some(c) => log!("emergency hotkey: {c}"),
            None => log!("WARNING: emergency hotkey unavailable (Esc and F12 combinations both taken)"),
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
            hotkey_combo,
            fullscreen: false,
        };
        app.layout();
        app.opts.reserve.finish(top_res_hwnd);
        app.opts.reserve.finish(bot_res_hwnd);
        unsafe {
            let _ = ShowWindow(top_hwnd, SW_SHOWNOACTIVATE);
            let _ = ShowWindow(dock_hwnd, SW_SHOWNOACTIVATE);
        }
        app.redraw();
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
            bar: gfx.text_format(w!("Segoe UI Variable"), 13.0 * scale, DWRITE_FONT_WEIGHT_NORMAL)?,
            pill: gfx.text_format(w!("Segoe UI Variable"), 12.0 * scale, DWRITE_FONT_WEIGHT_NORMAL)?,
            scale,
        })
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
            }

            let top = self.top_reserve.set_pos(self.monitor.monitor, px(TOP_RESERVE, s));
            let bottom = self.bottom_reserve.set_pos(self.monitor.monitor, px(BOTTOM_RESERVE, s));

            // Horizontal span: the work area excludes a left/right native taskbar.
            let work = win::work_area();
            let left = work.left.max(self.monitor.monitor.left);
            let right = work.right.min(self.monitor.monitor.right);
            let m = px(BAR_MARGIN, s);

            let bar = win::rect(left + m, top.top + m, (right - left - 2 * m).max(1), px(BAR_HEIGHT, s));

            let n = PLACEHOLDER_SLOTS as f32;
            let dock_w = px(2.0 * DOCK_PAD_X + n * DOCK_SLOT + (n - 1.0) * DOCK_SLOT_GAP, s);
            let dock_h = px(DOCK_HEIGHT, s);
            let dock = win::rect(
                left + (right - left - dock_w) / 2,
                bottom.bottom - px(DOCK_MARGIN_BOTTOM, s) - dock_h,
                dock_w,
                dock_h,
            );

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

        fn redraw(&mut self) {
            if let Err(e) = self.try_redraw() {
                log!("redraw failed: {e:?}");
                if self.gfx.is_lost() {
                    self.rebuild_graphics();
                }
            }
        }

        fn try_redraw(&self) -> Result<()> {
            let s = self.monitor.scale();
            let radius = self.opts.style.corners.approx_radius() * s;

            let (bw, bh) = (self.top_bar.surface.width as f32, self.top_bar.surface.height as f32);
            let handle = &self.user_handle;
            let fonts = &self.fonts;
            let hint = match self.hotkey_combo {
                Some(c) => format!("M1 scaffold · {c} to exit"),
                None => "M1 scaffold · emergency hotkey unavailable".to_string(),
            };
            self.top_bar.surface.draw(|dc| {
                let full = rectf(0.0, 0.0, bw, bh);
                render::fill_round(dc, full, radius, GLASS_BG)?;
                render::stroke_round(dc, full, radius, BORDER)?;
                // Left zone: user pill.
                let pill_h = 28.0 * s;
                let pill_y = (bh - pill_h) / 2.0;
                let pill_w = (handle.chars().count() as f32 * 7.0 + 20.0) * s;
                render::fill_round(dc, rectf(4.0 * s, pill_y, pill_w, pill_h), 14.0 * s, PILL_BG)?;
                render::text(dc, handle, &fonts.pill, rectf(14.0 * s, 0.0, pill_w - 20.0 * s, bh), TEXT_SECONDARY)?;
                // Divider.
                let div_x = 4.0 * s + pill_w + 6.0 * s;
                render::fill_round(dc, rectf(div_x, (bh - 16.0 * s) / 2.0, 1.0, 16.0 * s), 0.0, BORDER)?;
                // Active window placeholder (M2 fills this in).
                let title_x = div_x + 7.0 * s;
                render::text(dc, "Active window (M2)", &fonts.bar, rectf(title_x, 0.0, bw * 0.5, bh), TEXT_PRIMARY)?;
                // Right zone placeholder (M6 pills).
                unsafe {
                    fonts.pill.SetTextAlignment(DWRITE_TEXT_ALIGNMENT_TRAILING)?;
                }
                let r =
                    render::text(dc, &hint, &fonts.pill, rectf(bw * 0.5, 0.0, bw * 0.5 - 14.0 * s, bh), TEXT_SECONDARY);
                unsafe {
                    fonts.pill.SetTextAlignment(DWRITE_TEXT_ALIGNMENT_LEADING)?;
                }
                r
            })?;

            let (dw, dh) = (self.dock.surface.width as f32, self.dock.surface.height as f32);
            self.dock.surface.draw(|dc| {
                let full = rectf(0.0, 0.0, dw, dh);
                render::fill_round(dc, full, radius, GLASS_BG)?;
                render::stroke_round(dc, full, radius, BORDER)?;
                let slot = DOCK_SLOT * s;
                let y = (dh - slot) / 2.0;
                for i in 0..PLACEHOLDER_SLOTS {
                    let x = DOCK_PAD_X * s + i as f32 * (slot + DOCK_SLOT_GAP * s);
                    render::fill_round(dc, rectf(x, y, slot, slot), 12.0 * s, PILL_BG)?;
                }
                Ok(())
            })?;

            self.gfx.commit()
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
                self.try_redraw()
            })();
            if let Err(e) = result {
                log!("graphics rebuild failed: {e:?}");
            }
        }

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
                    self.redraw();
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
            self.layout();
            self.redraw();
        }

        fn shutdown(&mut self) {
            log!("clean exit; {}", MemSnapshot::now().describe());
            hotkey::unregister(self.top_bar.hwnd, hotkey::ID_EMERGENCY);
            self.top_reserve.remove();
            self.bottom_reserve.remove();
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

    fn is_reserve(hwnd: HWND) -> bool {
        let h = hwnd.0 as isize;
        h != 0 && (h == TOP_RESERVE_HWND.load(Ordering::Relaxed) || h == BOTTOM_RESERVE_HWND.load(Ordering::Relaxed))
    }

    unsafe extern "system" fn wndproc(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
        if msg != 0 && msg == TASKBAR_CREATED.get() {
            with_app(hwnd, msg, wp, lp, |a| {
                if a.top_bar.hwnd == hwnd {
                    a.on_taskbar_created();
                }
            });
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
                with_app(hwnd, msg, wp, lp, |a| {
                    if a.top_bar.hwnd == hwnd {
                        a.layout();
                        a.redraw();
                    }
                });
                LRESULT(0)
            }
            WM_CLOSE => LRESULT(0),
            _ => unsafe { DefWindowProcW(hwnd, msg, wp, lp) },
        }
    }
}
