//! M1 Step 1 spike (PLAN.md section 13). Throwaway, but kept in the repo so the
//! results can be reproduced.
//!
//! Creates, with the section 6 rendering model:
//!   1. a full-width 36 px strip,
//!   2. a 64 px dock-sized window,
//!   3. an invisible top-edge AppBar reservation window (variant selectable),
//!   4. optionally N pill-sized blurred windows (`--pills N`) to price floating pills.
//!
//! Live controls (Ctrl+Shift+Alt + key):
//!   B backdrop   C corners   T tint alpha   S tint shape   F extend frame
//!   M log memory + work area   R write report   Esc quit (F12 if Esc is taken)
//!
//! On exit it writes `spike-report.txt` in the current directory with the
//! automatic measurements plus a checklist for the visual observations.

#![cfg_attr(not(windows), allow(dead_code))]

#[cfg(not(windows))]
fn main() {
    eprintln!("The spike only runs on Windows 11.");
}

#[cfg(windows)]
fn main() {
    imp::main();
}

#[cfg(windows)]
mod imp {
    use std::cell::{Cell, RefCell};
    use std::fmt::Write as _;

    use windows::Win32::Foundation::*;
    use windows::Win32::Graphics::DirectWrite::*;
    use windows::Win32::Graphics::Gdi::{
        BLACK_BRUSH, BeginPaint, EndPaint, FillRect, GetStockObject, HBRUSH, PAINTSTRUCT,
    };
    use windows::Win32::UI::Input::KeyboardAndMouse::*;
    use windows::Win32::UI::WindowsAndMessaging::*;
    use windows::core::{Result, w};

    use winshell::appbar::{self, AppBar, Edge, ReserveMode, WM_APPBAR_CALLBACK};
    use winshell::dwm::{self, Backdrop, Corners, DwmStyle};
    use winshell::mem::MemSnapshot;
    use winshell::render::{self, Gfx, WindowSurface, rectf};
    use winshell::win::{self, MonitorGeometry, px};
    use winshell::{hotkey, log};

    const CLASS: windows::core::PCWSTR = w!("WinShellTheme_SpikeClass");

    const HK_BACKDROP: i32 = 10;
    const HK_CORNERS: i32 = 11;
    const HK_TINT: i32 = 12;
    const HK_SHAPE: i32 = 13;
    const HK_EXTEND: i32 = 14;
    const HK_MEM: i32 = 15;
    const HK_REPORT: i32 = 16;

    const TIMER_IDLE_5S: usize = 1;
    const TIMER_IDLE_30S: usize = 2;

    const TINTS: [u8; 4] = [72, 40, 0, 100];

    #[derive(Clone, Debug)]
    struct Cfg {
        style: DwmStyle,
        tint_pct: u8,
        tint_rounded: bool,
        reserve: Option<ReserveMode>,
        pills: usize,
        redirect: bool,
    }

    impl Cfg {
        fn from_args() -> std::result::Result<Self, String> {
            let mut c = Cfg {
                style: DwmStyle::default(),
                tint_pct: 72,
                tint_rounded: false,
                reserve: Some(ReserveMode::Layered),
                pills: 0,
                redirect: false,
            };
            let mut args = std::env::args().skip(1);
            while let Some(a) = args.next() {
                let mut val = |name: &str| args.next().ok_or(format!("{name} needs a value"));
                match a.as_str() {
                    "--backdrop" => c.style.backdrop = Backdrop::parse(&val("--backdrop")?).ok_or("bad --backdrop")?,
                    "--corners" => c.style.corners = Corners::parse(&val("--corners")?).ok_or("bad --corners")?,
                    "--no-extend" => c.style.extend_frame = false,
                    "--light" => c.style.dark = false,
                    "--tint" => c.tint_pct = val("--tint")?.parse::<u8>().map_err(|e| e.to_string())?.min(100),
                    "--rounded-tint" => c.tint_rounded = true,
                    "--reserve" => c.reserve = Some(ReserveMode::parse(&val("--reserve")?).ok_or("bad --reserve")?),
                    "--no-reserve" => c.reserve = None,
                    "--pills" => c.pills = val("--pills")?.parse::<usize>().map_err(|e| e.to_string())?.min(32),
                    "--redirect" => c.redirect = true,
                    "-h" | "--help" => return Err(USAGE.into()),
                    other => return Err(format!("unknown argument {other}\n\n{USAGE}")),
                }
            }
            Ok(c)
        }

        fn summary(&self) -> String {
            format!(
                "backdrop={} corners={} extend={} {} tint={}% shape={} redirect={}",
                self.style.backdrop.name(),
                self.style.corners.name(),
                if self.style.extend_frame { "on" } else { "off" },
                if self.style.dark { "dark" } else { "light" },
                self.tint_pct,
                if self.tint_rounded { "rounded" } else { "full" },
                if self.redirect { "yes" } else { "no (WS_EX_NOREDIRECTIONBITMAP)" },
            )
        }
    }

    const USAGE: &str = "spike.exe [--backdrop acrylic|mica|tabbed|none] [--corners round|small|none]
          [--no-extend] [--light] [--tint 0-100] [--rounded-tint]
          [--reserve layered|hidden|plan | --no-reserve] [--pills N] [--redirect]";

    struct SpikeWindow {
        hwnd: HWND,
        surface: WindowSurface,
        kind: Kind,
        dwm_result: String,
    }

    #[derive(Clone, Copy, PartialEq, Eq)]
    enum Kind {
        Strip,
        Dock,
        Pill,
    }

    struct Spike {
        cfg: Cfg,
        gfx: Gfx,
        font: IDWriteTextFormat,
        windows: Vec<SpikeWindow>,
        reserve: Option<AppBar>,
        reserve_hwnd_value: isize,
        hotkey_combo: Option<&'static str>,
        extra_hotkeys_ok: bool,
        work_before: RECT,
        work_after: RECT,
        taskbar: Option<appbar::TaskbarInfo>,
        monitor: MonitorGeometry,
        mem: Vec<(String, MemSnapshot)>,
        events: Vec<String>,
        style_history: Vec<String>,
    }

    thread_local! {
        static STATE: RefCell<Option<Spike>> = const { RefCell::new(None) };
        static RESERVE_HWND: Cell<isize> = const { Cell::new(0) };
        static PLAN_MODE: Cell<bool> = const { Cell::new(false) };
        static REDIRECT: Cell<bool> = const { Cell::new(false) };
    }

    fn mark(mem: &mut Vec<(String, MemSnapshot)>, label: &str) {
        let s = MemSnapshot::now();
        log!("memory [{label}]: {}", s.describe());
        mem.push((label.to_string(), s));
    }

    pub fn main() {
        let cfg = match Cfg::from_args() {
            Ok(c) => c,
            Err(e) => {
                eprintln!("{e}");
                std::process::exit(2);
            }
        };
        log::init(log::app_dir().join("spike.log"), true);
        win::enable_per_monitor_dpi();
        log!("spike starting: {}", cfg.summary());

        if let Err(e) = run(cfg) {
            log!("spike failed: {e:?}");
            std::process::exit(1);
        }
    }

    fn run(cfg: Cfg) -> Result<()> {
        let mut mem = Vec::new();
        mark(&mut mem, "process start");

        win::register_class(CLASS, wndproc)?;
        PLAN_MODE.set(cfg.reserve == Some(ReserveMode::Plan));
        REDIRECT.set(cfg.redirect);

        let gfx = Gfx::new()?;
        log!("D3D driver: {}", gfx.driver);
        mark(&mut mem, "after shared D3D11 + D2D + DComp + DWrite devices");

        let monitor = MonitorGeometry::primary();
        let s = monitor.scale();
        let taskbar = appbar::taskbar_info();
        let work_before = win::work_area();
        log!(
            "primary monitor {} work {} dpi {} ({}%)",
            win::rect_str(&monitor.monitor),
            win::rect_str(&work_before),
            monitor.dpi,
            (s * 100.0).round()
        );
        match &taskbar {
            Some(t) => log!("native taskbar: edge={} rect={} autohide={}", t.edge, win::rect_str(&t.rect), t.autohide),
            None => log!("native taskbar: ABM_GETTASKBARPOS failed (assuming bottom)"),
        }

        // 3. Reservation window first, so the visible windows can be laid out inside it.
        let mut reserve = None;
        let mut reserve_hwnd_value = 0;
        if let Some(mode) = cfg.reserve {
            let hwnd =
                win::create_popup(CLASS, w!("WinShellTheme Spike TopReserve"), mode.ex_style(), win::rect(0, 0, 1, 1))?;
            RESERVE_HWND.set(hwnd.0 as isize);
            reserve_hwnd_value = hwnd.0 as isize;
            let mut bar = AppBar::new(hwnd, Edge::Top);
            bar.register();
            let granted = bar.set_pos(monitor.monitor, px(44.0, s));
            mode.finish(hwnd);
            log!("reserve ({}) granted {}", mode.name(), win::rect_str(&granted));
            reserve = Some(bar);
            mark(&mut mem, &format!("after reservation window ({})", mode.name()));
        }
        let work_after = win::work_area();
        log!("work area after reservation: {}", win::rect_str(&work_after));

        let font = gfx.text_format(w!("Segoe UI Variable"), 12.0 * s, DWRITE_FONT_WEIGHT_NORMAL)?;

        let mut ex = WS_EX_TOOLWINDOW | WS_EX_TOPMOST | WS_EX_NOACTIVATE;
        if !cfg.redirect {
            ex |= WS_EX_NOREDIRECTIONBITMAP;
        }

        let left = work_after.left.max(monitor.monitor.left);
        let right = work_after.right.min(monitor.monitor.right);
        let top = reserve.as_ref().map(|r| r.rect.top).unwrap_or(monitor.monitor.top);

        let mut layout = vec![(
            Kind::Strip,
            win::rect(left + px(8.0, s), top + px(8.0, s), right - left - 2 * px(8.0, s), px(36.0, s)),
        )];
        let dock_w = px(480.0, s);
        let dock_h = px(64.0, s);
        layout.push((
            Kind::Dock,
            win::rect(left + (right - left - dock_w) / 2, work_after.bottom - px(10.0, s) - dock_h, dock_w, dock_h),
        ));
        for i in 0..cfg.pills {
            let pill_w = px(54.0, s);
            layout.push((
                Kind::Pill,
                win::rect(left + px(8.0, s) + i as i32 * (pill_w + px(6.0, s)), top + px(60.0, s), pill_w, px(28.0, s)),
            ));
        }

        let mut windows = Vec::new();
        for (kind, r) in layout {
            let title = match kind {
                Kind::Strip => w!("WinShellTheme Spike Strip"),
                Kind::Dock => w!("WinShellTheme Spike Dock"),
                Kind::Pill => w!("WinShellTheme Spike Pill"),
            };
            let hwnd = win::create_popup(CLASS, title, ex, r)?;
            let rep = dwm::apply(hwnd, cfg.style);
            let mut surface = WindowSurface::new(&gfx, hwnd)?;
            surface.resize(&gfx, (r.right - r.left) as u32, (r.bottom - r.top) as u32)?;
            unsafe {
                let _ = ShowWindow(hwnd, SW_SHOWNOACTIVATE);
            }
            let name = match kind {
                Kind::Strip => "strip",
                Kind::Dock => "dock",
                Kind::Pill => "pill",
            };
            log!("{name} {} dwm: {}", win::rect_str(&r), rep.describe());
            windows.push(SpikeWindow { hwnd, surface, kind, dwm_result: rep.describe() });
            if kind != Kind::Pill {
                mark(&mut mem, &format!("after {name} window"));
            }
        }
        if cfg.pills > 0 {
            mark(&mut mem, &format!("after {} pill windows", cfg.pills));
        }

        let strip_hwnd = windows[0].hwnd;
        let hotkey_combo = hotkey::register_emergency(strip_hwnd);
        match hotkey_combo {
            Some(c) => log!("emergency hotkey registered: {c}"),
            None => log!(
                "WARNING: neither Ctrl+Shift+Alt+Esc nor +F12 could be registered; close the console window to quit"
            ),
        }
        #[allow(clippy::unnecessary_fold)]
        let extra_hotkeys_ok = [
            (HK_BACKDROP, VK_B),
            (HK_CORNERS, VK_C),
            (HK_TINT, VK_T),
            (HK_SHAPE, VK_S),
            (HK_EXTEND, VK_F),
            (HK_MEM, VK_M),
            (HK_REPORT, VK_R),
        ]
        .into_iter()
        .map(|(id, vk)| hotkey::register(strip_hwnd, id, vk))
        .fold(true, |a, b| a && b); // not all(): every hotkey must be attempted
        if !extra_hotkeys_ok {
            log!("note: some Ctrl+Shift+Alt+<letter> spike hotkeys are taken; use command-line flags instead");
        }

        let mut spike = Spike {
            cfg,
            gfx,
            font,
            windows,
            reserve,
            reserve_hwnd_value,
            hotkey_combo,
            extra_hotkeys_ok,
            work_before,
            work_after,
            taskbar,
            monitor,
            mem,
            events: Vec::new(),
            style_history: Vec::new(),
        };
        spike.redraw();
        mark(&mut spike.mem, "after first draw + Commit");
        spike.style_history.push(spike.cfg.summary());

        unsafe {
            SetTimer(Some(strip_hwnd), TIMER_IDLE_5S, 5_000, None);
            SetTimer(Some(strip_hwnd), TIMER_IDLE_30S, 30_000, None);
        }

        STATE.with(|st| *st.borrow_mut() = Some(spike));

        println!();
        println!("Spike running. Check the strip, the dock and the reserved top edge.");
        println!(
            "Ctrl+Shift+Alt + B backdrop | C corners | T tint | S tint shape | F extend frame | M memory | R report"
        );
        println!("Quit with {} (writes spike-report.txt here).", hotkey_combo.unwrap_or("closing this console"));

        unsafe {
            let mut msg = MSG::default();
            while GetMessageW(&mut msg, None, 0, 0).as_bool() {
                let _ = TranslateMessage(&msg);
                DispatchMessageW(&msg);
            }
        }
        Ok(())
    }

    impl Spike {
        fn redraw(&mut self) {
            let s = self.monitor.scale();
            let base = if self.cfg.style.dark { 0x14161C00u32 } else { 0xF5F6FA00u32 };
            let tint = base | ((self.cfg.tint_pct as u32 * 255 + 50) / 100);
            let border = if self.cfg.style.dark { 0xFFFFFF1F } else { 0x0000001A };
            let text_c = if self.cfg.style.dark { 0xF2F4F8FF } else { 0x14161CFF };
            let slot_c = if self.cfg.style.dark { 0x1E212A99 } else { 0xFFFFFFA6 };
            let radius = if self.cfg.tint_rounded { self.cfg.style.corners.approx_radius() * s } else { 0.0 };
            let summary = self.cfg.summary();
            let combo = self.hotkey_combo.unwrap_or("close console");
            for win in &self.windows {
                let (wd, ht) = (win.surface.width as f32, win.surface.height as f32);
                let font = &self.font;
                let res = win.surface.draw(|dc| {
                    let full = rectf(0.0, 0.0, wd, ht);
                    render::fill_round(dc, full, radius, tint)?;
                    render::stroke_round(dc, full, radius, border)?;
                    match win.kind {
                        Kind::Strip => {
                            let label = format!("SPIKE  {summary}   |   Ctrl+Shift+Alt+ B C T S F M R, quit {combo}");
                            render::text(dc, &label, font, rectf(12.0 * s, 0.0, wd - 24.0 * s, ht), text_c)?;
                        }
                        Kind::Dock => {
                            let slot = 48.0 * s;
                            let pad_y = (ht - slot) / 2.0;
                            for i in 0..5 {
                                let x = 10.0 * s + i as f32 * (slot + 4.0 * s);
                                render::fill_round(dc, rectf(x, pad_y, slot, slot), 12.0 * s, slot_c)?;
                            }
                            let tx = 10.0 * s + 5.0 * (slot + 4.0 * s) + 6.0 * s;
                            render::text(dc, "dock 64 px", font, rectf(tx, 0.0, wd - tx - 10.0 * s, ht), text_c)?;
                        }
                        Kind::Pill => {}
                    }
                    Ok(())
                });
                if let Err(e) = res {
                    log!("draw failed: {e:?} (device lost: {})", self.gfx.is_lost());
                }
            }
            if let Err(e) = self.gfx.commit() {
                log!("commit failed: {e:?}");
            }
        }

        fn restyle(&mut self) {
            for win in &mut self.windows {
                win.dwm_result = dwm::apply(win.hwnd, self.cfg.style).describe();
            }
            log!("style -> {} | dwm: {}", self.cfg.summary(), self.windows[0].dwm_result);
            self.style_history.push(self.cfg.summary());
            self.redraw();
        }

        fn on_hotkey(&mut self, id: i32) -> bool {
            match id {
                hotkey::ID_EMERGENCY => return false,
                HK_BACKDROP => {
                    let i = Backdrop::ALL.iter().position(|b| *b == self.cfg.style.backdrop).unwrap_or(0);
                    self.cfg.style.backdrop = Backdrop::ALL[(i + 1) % Backdrop::ALL.len()];
                    self.restyle();
                }
                HK_CORNERS => {
                    let i = Corners::ALL.iter().position(|c| *c == self.cfg.style.corners).unwrap_or(0);
                    self.cfg.style.corners = Corners::ALL[(i + 1) % Corners::ALL.len()];
                    self.restyle();
                }
                HK_TINT => {
                    let i = TINTS.iter().position(|t| *t == self.cfg.tint_pct).map(|i| i + 1).unwrap_or(0);
                    self.cfg.tint_pct = TINTS[i % TINTS.len()];
                    self.restyle();
                }
                HK_SHAPE => {
                    self.cfg.tint_rounded = !self.cfg.tint_rounded;
                    self.restyle();
                }
                HK_EXTEND => {
                    self.cfg.style.extend_frame = !self.cfg.style.extend_frame;
                    self.restyle();
                }
                HK_MEM => {
                    mark(&mut self.mem, "manual (M hotkey)");
                    log!("work area now: {}", win::rect_str(&win::work_area()));
                }
                HK_REPORT => self.write_report(),
                _ => {}
            }
            true
        }

        fn on_appbar(&mut self, wparam: usize, lparam: LPARAM) {
            let what = appbar::describe_notification(wparam, lparam);
            log!("appbar notification: {what}");
            self.events.push(format!("{} {what}", log::timestamp()));
            if wparam as u32 == windows::Win32::UI::Shell::ABN_POSCHANGED {
                let thickness = px(44.0, self.monitor.scale());
                if let Some(r) = self.reserve.as_mut() {
                    r.set_pos(self.monitor.monitor, thickness);
                }
            }
        }

        fn write_report(&self) {
            let mut r = String::new();
            let _ = writeln!(r, "WinShellTheme M1 spike report ({})", log::timestamp());
            let _ = writeln!(r, "==========================================================");
            let _ = writeln!(r, "\n## Configuration");
            let _ = writeln!(r, "start:   {}", self.style_history.first().cloned().unwrap_or_default());
            let _ = writeln!(r, "final:   {}", self.cfg.summary());
            let _ = writeln!(r, "reserve: {}", self.cfg.reserve.map(|m| m.name()).unwrap_or("disabled"));
            let _ = writeln!(r, "pills:   {}", self.cfg.pills);
            let _ = writeln!(r, "\n## Environment");
            let _ = writeln!(r, "D3D driver: {}", self.gfx.driver);
            let _ = writeln!(
                r,
                "primary monitor: {} dpi {} ({}%)",
                win::rect_str(&self.monitor.monitor),
                self.monitor.dpi,
                (self.monitor.scale() * 100.0).round()
            );
            match &self.taskbar {
                Some(t) => {
                    let _ = writeln!(
                        r,
                        "native taskbar: edge={} rect={} autohide={}",
                        t.edge,
                        win::rect_str(&t.rect),
                        t.autohide
                    );
                }
                None => {
                    let _ = writeln!(r, "native taskbar: ABM_GETTASKBARPOS failed");
                }
            }
            let _ = writeln!(r, "emergency hotkey: {}", self.hotkey_combo.unwrap_or("NOT REGISTERED"));
            let _ = writeln!(r, "spike letter hotkeys all registered: {}", self.extra_hotkeys_ok);

            let _ = writeln!(r, "\n## DWM attribute results (last applied)");
            for w in &self.windows {
                let name = match w.kind {
                    Kind::Strip => "strip",
                    Kind::Dock => "dock",
                    Kind::Pill => "pill",
                };
                let _ = writeln!(r, "{name:6} {}", w.dwm_result);
            }

            let _ = writeln!(r, "\n## Reservation (test 3)");
            let _ = writeln!(r, "work area before: {}", win::rect_str(&self.work_before));
            let _ = writeln!(r, "work area after:  {}", win::rect_str(&self.work_after));
            let _ = writeln!(r, "work area now:    {}", win::rect_str(&win::work_area()));
            if let Some(b) = &self.reserve {
                let _ = writeln!(r, "ABM_NEW ok: {}  granted: {}", b.registered, win::rect_str(&b.rect));
                let held = self.work_after.top > self.work_before.top || self.work_after.top >= b.rect.bottom;
                let _ = writeln!(r, "top edge held by reservation: {}", if held { "YES" } else { "NO" });
            }
            let _ = writeln!(r, "AppBar notifications received ({}):", self.events.len());
            for e in &self.events {
                let _ = writeln!(r, "  {e}");
            }
            let fs = self.events.iter().any(|e| e.contains("FULLSCREENAPP"));
            let _ = writeln!(
                r,
                "ABN_FULLSCREENAPP seen: {}",
                if fs { "YES" } else { "no (open a fullscreen video / F11 browser to test)" }
            );

            let _ = writeln!(r, "\n## Private Working Set timeline");
            let mut prev = 0usize;
            for (label, m) in &self.mem {
                let delta = m.private_ws as f64 - prev as f64;
                let _ = writeln!(r, "{:>7.1} MB  ({:+6.1})  {label}", m.private_mb(), delta / (1024.0 * 1024.0));
                prev = m.private_ws;
            }

            let _ = writeln!(r, "\n## Style changes tried");
            for s in &self.style_history {
                let _ = writeln!(r, "  {s}");
            }

            let _ = writeln!(r, "\n## Visual checklist (fill in by hand, then paste into PLAN.md)");
            for q in [
                "1a. Strip: blur visible behind the tint?                         yes / no",
                "1b. Strip: blur stays when you click into other apps (inactive)? yes / no",
                "1c. Strip: D2D text/tint drawn above the blur (not under it)?     yes / no",
                "1d. Tint 72% looks right? Preferred value:                        ___ %",
                "1e. Mica (B to cycle) greys out when inactive, as expected?       yes / no",
                "2a. Dock: outline radius with dwm_round (zoom a screenshot):      ___ px",
                "2b. Dock: is blur clipped to the rounded outline?                 yes / no",
                "2c. Full-rect tint (S) clipped by DWM corners, or square corners? clipped / square",
                "3a. Maximised window stops below the reserved top 44 px?          yes / no",
                "3b. Desktop icons / windows under the reserved band clickable?    yes / no",
                "3c. ABN_FULLSCREENAPP logged for F11 browser / fullscreen video?  yes / no",
                "4.  Pills: memory per pill window (see timeline):                 ___ MB",
            ] {
                let _ = writeln!(r, "{q}");
            }

            let path = std::env::current_dir().unwrap_or_default().join("spike-report.txt");
            match std::fs::write(&path, &r) {
                Ok(_) => log!("report written to {}", path.display()),
                Err(e) => log!("could not write report: {e}"),
            }
        }

        fn shutdown(&mut self) {
            mark(&mut self.mem, "before exit");
            self.write_report();
            if let Some(hwnd) = self.windows.first().map(|w| w.hwnd) {
                for id in
                    [hotkey::ID_EMERGENCY, HK_BACKDROP, HK_CORNERS, HK_TINT, HK_SHAPE, HK_EXTEND, HK_MEM, HK_REPORT]
                {
                    hotkey::unregister(hwnd, id);
                }
            }
            if let Some(r) = self.reserve.as_mut() {
                r.remove();
            }
            let _ = self.reserve_hwnd_value;
            for w in &self.windows {
                unsafe {
                    let _ = DestroyWindow(w.hwnd);
                }
            }
            log!("spike exited cleanly");
        }
    }

    /// Run `f` on the spike state. If the state is already borrowed (a re-entrant
    /// message delivered while Explorer processes an SHAppBarMessage call), the
    /// message is re-posted and handled once the outer call returns.
    fn with_state(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM, f: impl FnOnce(&mut Spike)) {
        STATE.with(|st| match st.try_borrow_mut() {
            Ok(mut guard) => {
                if let Some(s) = guard.as_mut() {
                    f(s);
                }
            }
            Err(_) => unsafe {
                let _ = PostMessageW(Some(hwnd), msg, wp, lp);
            },
        });
    }

    unsafe extern "system" fn wndproc(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
        let is_reserve = RESERVE_HWND.get() == hwnd.0 as isize;
        match msg {
            WM_NCHITTEST if is_reserve && PLAN_MODE.get() => LRESULT(HTTRANSPARENT as isize),
            WM_MOUSEACTIVATE => LRESULT(MA_NOACTIVATE as isize),
            WM_ERASEBKGND => {
                if REDIRECT.get() {
                    // Black is treated as transparent inside an extended DWM frame.
                    let mut rc = RECT::default();
                    unsafe {
                        let _ = GetClientRect(hwnd, &mut rc);
                        FillRect(
                            windows::Win32::Graphics::Gdi::HDC(wp.0 as _),
                            &rc,
                            HBRUSH(GetStockObject(BLACK_BRUSH).0),
                        );
                    }
                }
                LRESULT(1)
            }
            WM_PAINT => {
                let mut ps = PAINTSTRUCT::default();
                unsafe {
                    BeginPaint(hwnd, &mut ps);
                    let _ = EndPaint(hwnd, &ps);
                }
                LRESULT(0)
            }
            WM_HOTKEY => {
                let mut quit = false;
                with_state(hwnd, msg, wp, lp, |s| {
                    if !s.on_hotkey(wp.0 as i32) {
                        s.shutdown();
                        quit = true;
                    }
                });
                if quit {
                    unsafe { PostQuitMessage(0) };
                }
                LRESULT(0)
            }
            WM_TIMER => {
                unsafe {
                    let _ = KillTimer(Some(hwnd), wp.0);
                }
                let label = if wp.0 == TIMER_IDLE_5S { "idle +5 s" } else { "idle +30 s" };
                with_state(hwnd, msg, wp, lp, |s| mark(&mut s.mem, label));
                LRESULT(0)
            }
            WM_APPBAR_CALLBACK => {
                with_state(hwnd, msg, wp, lp, |s| s.on_appbar(wp.0, lp));
                LRESULT(0)
            }
            WM_ACTIVATE if is_reserve => {
                with_state(hwnd, msg, wp, lp, |s| {
                    if let Some(r) = &s.reserve {
                        r.on_activate();
                    }
                });
                unsafe { DefWindowProcW(hwnd, msg, wp, lp) }
            }
            WM_WINDOWPOSCHANGED if is_reserve => {
                STATE.with(|st| {
                    if let Ok(g) = st.try_borrow()
                        && let Some(r) = g.as_ref().and_then(|s| s.reserve.as_ref())
                    {
                        r.on_window_pos_changed();
                    }
                });
                unsafe { DefWindowProcW(hwnd, msg, wp, lp) }
            }
            WM_CLOSE => LRESULT(0),
            _ => unsafe { DefWindowProcW(hwnd, msg, wp, lp) },
        }
    }
}
