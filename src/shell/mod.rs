//! The shell app (PLAN.md section 13).
//!
//! * M1: invisible click-through AppBars, blurred top bar and dock, emergency hotkey.
//! * M2: live active window icon + title, minute-boundary clock.
//! * M3: running apps in the dock (WinEvent hooks, shell icons, LRU cache).
//! * M4: pinning (config.toml via toml_edit), launch / activate / minimise,
//!   middle-click new instance, right-click menu, multi-window list, hover + tooltip.
//! * M5: native taskbar autohide with exact restore (state file first).
//! * M6: Bluetooth, Wi-Fi, volume and battery pills, all callback-driven.
//! * M7: sound / battery / calendar flyouts with media session cards, and the
//!   dock's mini media player.
//!
//! Threads: this UI thread (STA), the shell asset loader (STA) and the system
//! monitor (MTA, WinRT). Workers report through channels + PostMessageW.

mod autohide;
mod bar;
mod dock;
mod immersive;
mod panel;
mod popup;
mod theme;

use std::cell::{Cell, RefCell};
use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::atomic::{AtomicIsize, Ordering};

use windows::Win32::Foundation::*;
use windows::Win32::Graphics::Direct2D::ID2D1Bitmap1;
use windows::Win32::Graphics::DirectWrite::*;
use windows::Win32::Graphics::Gdi::{BeginPaint, EndPaint, PAINTSTRUCT};
use windows::Win32::System::Com::{COINIT_APARTMENTTHREADED, COINIT_DISABLE_OLE1DDE, CoInitializeEx};
use windows::Win32::UI::Input::KeyboardAndMouse::{TME_LEAVE, TRACKMOUSEEVENT, TrackMouseEvent, VK_ESCAPE, VK_F};
use windows::Win32::UI::Shell::{ABN_FULLSCREENAPP, ABN_POSCHANGED};
use windows::Win32::UI::WindowsAndMessaging::*;
use windows::core::{HSTRING, PCWSTR, Result, w};

use winshell::appbar::{self, AppBar, Edge, ReserveMode, WM_APPBAR_CALLBACK};
use winshell::assets::{AssetLoader, WM_SHELL_ASSET};
use winshell::audio::{AUDIO_DEVICES_CHANGED, Audio, AudioDevice, AudioState, WM_SHELL_AUDIO};
use winshell::clock::{self, Clock};
use winshell::config::{self, Config};
use winshell::dock_model::DockModel;
use winshell::dwm::{self, Backdrop, Corners, DwmStyle};
use winshell::foreground::{ForegroundTracker, WM_SHELL_FOREGROUND, WM_SHELL_TITLE};
use winshell::instance::SingleInstance;
use winshell::lru::Lru;
use winshell::mem::MemSnapshot;
use winshell::network::{NetState, Network, WM_SHELL_NETWORK};
use winshell::power::{self, BatteryState, PowerNotifications};
use winshell::render::Gfx;
use winshell::sysmon::{MediaSession, SystemMonitor, WM_SHELL_MONITOR};
use winshell::taskbar::{self, TaskbarGuard};
use winshell::theme_file::{self, Palette};
use winshell::watcher::{ConfigWatcher, WM_SHELL_CONFIG};
use winshell::win::{self, MonitorGeometry, px};
use winshell::winevents::{WM_SHELL_WINDOWS, WindowEventHooks};
use winshell::{hotkey, log};

use bar::{Active, BarHit, BarLayout};
use dock::{DockHit, DockLayout};
use panel::PanelState;
use popup::Popup;
use theme::*;

const TOPBAR_CLASS: PCWSTR = w!("WinShellTheme_TopBarClass");
const WINDOW_CLASS: PCWSTR = w!("WinShellTheme_Window");
const FLYOUT_CLASS: PCWSTR = w!("WinShellTheme_FlyoutClass");

const HOTKEY_IMMERSIVE: i32 = 0x5746;
const TIMER_CLOCK: usize = 1;
const TIMER_TOOLTIP_HIDE: usize = 2;
const TIMER_AUTOHIDE_BAR: usize = 3;
const TIMER_AUTOHIDE_DOCK: usize = 4;

/// Second launch: ask the running instance to show its windows again.
const WM_SHELL_RESTORE: u32 = WM_APP + 0x28;
/// `winshell.exe --quit`: ask the running instance to exit cleanly (restoring the taskbar).
/// UIPI stops lower-integrity processes from posting WM_APP messages to this window.
const WM_SHELL_QUIT: u32 = WM_APP + 0x29;
/// A decoded WM_SETTINGCHANGE (wParam = SETTING_*), posted to ourselves so the
/// original lParam pointer is never stored or re-posted (security review F1).
const WM_SHELL_SETTING: u32 = WM_APP + 0x2A;
const SETTING_INTL: usize = 1;
const SETTING_COLORS: usize = 2;
const WM_ENDSESSION_MSG: u32 = 0x0016;

const USAGE: &str = "WinShellTheme (winshell.exe)

  winshell.exe                      start the shell (a second launch shows the running one)
  winshell.exe --quit               ask the running shell to exit cleanly
  winshell.exe --restore-taskbar    put the native taskbar back from a leftover state file
  winshell.exe --enable-autostart   start with Windows (HKCU Run value)
  winshell.exe --disable-autostart  stop starting with Windows
  winshell.exe --help

Start options: --preview, --no-taskbar, --reserve layered|hidden|plan,
               --backdrop acrylic|mica|tabbed|none, --corners round|small|none";

const PBT_POWERSETTINGCHANGE: u32 = 0x8013;
const WM_MOUSELEAVE: u32 = 0x02A3;
const PBT_APMPOWERSTATUSCHANGE: u32 = 0x000A;

/// Reservation HWNDs for the panic hook, which cannot borrow the app state.
static TOP_RESERVE_HWND: AtomicIsize = AtomicIsize::new(0);
static BOTTOM_RESERVE_HWND: AtomicIsize = AtomicIsize::new(0);

thread_local! {
    static APP: RefCell<Option<App>> = const { RefCell::new(None) };
    static PLAN_MODE: Cell<bool> = const { Cell::new(false) };
    static TASKBAR_CREATED: Cell<u32> = const { Cell::new(0) };
}

pub struct Options {
    /// Visible, targetable surfaces for development / visual QA. Never touches the taskbar.
    preview: bool,
    reserve: ReserveMode,
    style: DwmStyle,
    /// `--no-taskbar`: never touch the native taskbar this run.
    keep_taskbar: bool,
    /// `--corners` given: overrides the theme's window_corners.
    corners_override: bool,
}

impl Options {
    fn from_args() -> std::result::Result<Self, String> {
        let mut o = Options {
            preview: false,
            reserve: ReserveMode::Layered,
            style: DwmStyle::default(),
            keep_taskbar: false,
            corners_override: false,
        };
        let mut args = std::env::args().skip(1);
        while let Some(a) = args.next() {
            let mut val = |n: &str| args.next().ok_or(format!("{n} needs a value"));
            match a.as_str() {
                "--reserve" => o.reserve = ReserveMode::parse(&val("--reserve")?).ok_or("bad --reserve")?,
                "--backdrop" => o.style.backdrop = Backdrop::parse(&val("--backdrop")?).ok_or("bad --backdrop")?,
                "--corners" => {
                    o.style.corners = Corners::parse(&val("--corners")?).ok_or("bad --corners")?;
                    o.corners_override = true;
                }
                "--no-taskbar" => o.keep_taskbar = true,
                "--preview" => {
                    o.preview = true;
                    o.keep_taskbar = true;
                }
                "--autostart" => {}
                other => return Err(format!("unknown argument {other}")),
            }
        }
        Ok(o)
    }
}

pub struct Fonts {
    pub title: IDWriteTextFormat,
    pub pill: IDWriteTextFormat,
    pub icon: IDWriteTextFormat,
    pub icon_small: IDWriteTextFormat,
    pub header: IDWriteTextFormat,
    pub body: IDWriteTextFormat,
    pub body_bold: IDWriteTextFormat,
    pub sub: IDWriteTextFormat,
    pub big: IDWriteTextFormat,
    pub scale: f32,
}

fn make_fonts(gfx: &Gfx, scale: f32) -> Result<Fonts> {
    let ui = w!("Segoe UI Variable");
    let icons = if gfx.font_exists("Segoe Fluent Icons") { w!("Segoe Fluent Icons") } else { w!("Segoe MDL2 Assets") };
    Ok(Fonts {
        title: gfx.text_format_ellipsis(ui, 12.0 * scale, DWRITE_FONT_WEIGHT_NORMAL)?,
        pill: gfx.text_format(ui, 11.0 * scale, DWRITE_FONT_WEIGHT_NORMAL)?,
        icon: gfx.text_format(icons, 14.0 * scale, DWRITE_FONT_WEIGHT_NORMAL)?,
        icon_small: gfx.text_format(icons, 12.0 * scale, DWRITE_FONT_WEIGHT_NORMAL)?,
        header: gfx.text_format_ellipsis(ui, 14.0 * scale, DWRITE_FONT_WEIGHT_SEMI_BOLD)?,
        body: gfx.text_format_ellipsis(ui, 13.0 * scale, DWRITE_FONT_WEIGHT_NORMAL)?,
        body_bold: gfx.text_format_ellipsis(ui, 13.0 * scale, DWRITE_FONT_WEIGHT_SEMI_BOLD)?,
        sub: gfx.text_format_ellipsis(ui, 11.0 * scale, DWRITE_FONT_WEIGHT_NORMAL)?,
        big: gfx.text_format(ui, 28.0 * scale, DWRITE_FONT_WEIGHT_SEMI_BOLD)?,
        scale,
    })
}

/// A context menu to run once the app state is no longer borrowed
/// (TrackPopupMenu runs a modal loop that re-enters the window procedure).
pub struct MenuRequest {
    owner: HWND,
    x: i32,
    y: i32,
    above: bool,
    /// (command id, label); id 0 = separator.
    items: Vec<(u32, String)>,
    context: MenuContext,
}

#[derive(Clone)]
pub enum MenuContext {
    Dock { group_id: String },
    ActiveWindow { hwnd: isize },
    Shell,
}

pub struct App {
    pub opts: Options,
    pub config: Config,
    pub config_path: PathBuf,
    pub gfx: Gfx,
    pub fonts: Fonts,
    pub monitor: MonitorGeometry,
    pub top_reserve: AppBar,
    pub bottom_reserve: AppBar,
    pub top_bar: Popup,
    pub dock: Popup,
    pub tooltip: Popup,
    pub flyout: Popup,
    pub user_handle: String,
    pub fullscreen: bool,
    // M2
    pub clock: Clock,
    pub clock_text: String,
    pub tracker: ForegroundTracker,
    pub active: Active,
    pub bar: BarLayout,
    pub bar_hover: Option<BarHit>,
    pub first_tick_logged: bool,
    // M3 / M4
    pub dock_model: DockModel,
    pub window_hooks: WindowEventHooks,
    pub assets: AssetLoader,
    pub icons: Lru<ID2D1Bitmap1>,
    pub icon_requested: HashSet<String>,
    /// Apps whose icon lookup failed (not retried until a rescan).
    pub icon_failed: HashSet<String>,
    pub app_names: HashMap<String, String>,
    pub dock_area: RECT,
    pub span: (i32, i32),
    pub dock_layout: DockLayout,
    pub dock_hover: Option<DockHit>,
    pub dock_scroll: usize,
    pub pending_launches: Vec<(String, String, std::time::Instant)>,
    pub tooltip_text: String,
    // M5
    pub taskbar: TaskbarGuard,
    // M6
    pub audio: Option<Audio>,
    pub audio_state: AudioState,
    pub network: Network,
    pub net_state: NetState,
    pub battery: BatteryState,
    pub power_notify: PowerNotifications,
    pub sysmon: SystemMonitor,
    pub bluetooth: Option<bool>,
    pub wifi_radio: Option<bool>,
    pub wifi_networks: Vec<winshell::network::WifiNetwork>,
    pub bt_devices: Vec<winshell::sysmon::BtDevice>,
    // Auto-hide
    pub edges: autohide::Edges,
    pub reveal_bar: bool,
    pub reveal_dock: bool,
    // M7
    pub media: Vec<MediaSession>,
    pub media_bitmaps: HashMap<String, (usize, ID2D1Bitmap1)>,
    pub devices: Vec<AudioDevice>,
    pub panel: Option<PanelState>,
    // M8
    pub app_dir: PathBuf,
    pub watcher: Option<ConfigWatcher>,
    /// Exact text we last wrote to config.toml; the watcher's echo of it is ignored.
    pub last_self_write: Option<String>,
}

pub fn main() {
    let hardening_failures = winshell::hardening::harden_process();
    let dir = log::app_dir();
    log::init(dir.join("winshell.log"), cfg!(debug_assertions));
    for f in hardening_failures {
        log!("hardening not applied: {f}");
    }

    if let Some(code) = run_command_line(&dir) {
        std::process::exit(code);
    }

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
        log!("another instance is already running; asking it to show itself");
        if let Some(h) = running_instance() {
            unsafe {
                let _ = PostMessageW(Some(h), WM_SHELL_RESTORE, WPARAM(0), LPARAM(0));
            }
        }
        return;
    };

    // M5 step 1: repair the taskbar if the previous run died, before anything else.
    let taskbar = TaskbarGuard::repair(dir.join("taskbar_state.toml"));
    immersive::repair();

    let config_path = dir.join("config.toml");
    let (config, err) = config::load_or_create(&config_path);
    if let Some(e) = err {
        log!("config error, using defaults: {e}");
    }
    ensure_theme_files(&dir);

    log!("winshell starting (reserve={}, {:?})", opts.reserve.name(), opts.style);
    if let Err(e) = run(opts, config, config_path, taskbar) {
        log!("fatal: {e:?}");
        cleanup_raw();
        std::process::exit(1);
    }
}

fn install_panic_hook(path: PathBuf) {
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

/// Panic / fatal paths: restore the taskbar from its state file and remove the
/// AppBars using only stored handles (the app state may be unreachable).
fn cleanup_raw() {
    immersive::restore_all();
    taskbar::restore_from_file();
    for slot in [&TOP_RESERVE_HWND, &BOTTOM_RESERVE_HWND] {
        let h = slot.swap(0, Ordering::SeqCst);
        if h != 0 {
            appbar::remove_raw(HWND(h as _));
        }
    }
}

/// The running shell's top bar window, if any.
fn running_instance() -> Option<HWND> {
    unsafe { FindWindowW(TOPBAR_CLASS, None) }.ok().filter(|h| !h.0.is_null())
}

/// Print to the console that started us (the release exe has no console of its own).
fn cli_print(msg: &str) {
    use windows::Win32::System::Console::{ATTACH_PARENT_PROCESS, AttachConsole};
    unsafe {
        let _ = AttachConsole(ATTACH_PARENT_PROCESS);
    }
    println!("{msg}");
}

/// One-shot maintenance commands. Returns an exit code, or None to start the shell.
fn run_command_line(dir: &std::path::Path) -> Option<i32> {
    let first = std::env::args().nth(1)?;
    match first.as_str() {
        "--help" | "-h" | "/?" => {
            cli_print(USAGE);
            Some(0)
        }
        "--quit" => {
            let Some(h) = running_instance() else {
                cli_print("WinShellTheme is not running.");
                return Some(0);
            };
            let mut pid = 0u32;
            unsafe {
                GetWindowThreadProcessId(h, Some(&mut pid));
                let _ = PostMessageW(Some(h), WM_SHELL_QUIT, WPARAM(0), LPARAM(0));
            }
            // Wait (bounded) for it to finish restoring the taskbar and exit.
            use windows::Win32::System::Threading::{OpenProcess, PROCESS_SYNCHRONIZE, WaitForSingleObject};
            let exited = unsafe {
                match OpenProcess(PROCESS_SYNCHRONIZE, false, pid) {
                    Ok(p) => {
                        let r = WaitForSingleObject(p, 10_000);
                        let _ = CloseHandle(p);
                        r == WAIT_OBJECT_0
                    }
                    Err(_) => true,
                }
            };
            cli_print(if exited { "WinShellTheme exited." } else { "WinShellTheme did not exit within 10 seconds." });
            Some(if exited { 0 } else { 1 })
        }
        "--restore-taskbar" => {
            if running_instance().is_some() {
                cli_print("WinShellTheme is running; use --quit (it restores the taskbar itself).");
                return Some(1);
            }
            let path = dir.join("taskbar_state.toml");
            let had_state = path.exists();
            let _guard = TaskbarGuard::repair(path);
            cli_print(if had_state { "Taskbar restored from the saved state." } else { "Nothing to restore." });
            Some(0)
        }
        "--enable-autostart" => {
            let exe = std::env::current_exe().map(|p| p.to_string_lossy().into_owned()).unwrap_or_default();
            match winshell::registry::enable_autostart(&exe) {
                Ok(()) => {
                    cli_print(&format!("Autostart enabled for {exe}"));
                    Some(0)
                }
                Err(e) => {
                    cli_print(&format!("Could not enable autostart: {e}"));
                    Some(1)
                }
            }
        }
        "--disable-autostart" => match winshell::registry::disable_autostart() {
            Ok(()) => {
                cli_print("Autostart disabled.");
                Some(0)
            }
            Err(e) => {
                cli_print(&format!("Could not disable autostart: {e}"));
                Some(1)
            }
        },
        _ => None,
    }
}

/// Write the built-in theme files on first run (never overwrites edits).
fn ensure_theme_files(dir: &std::path::Path) {
    let themes = dir.join("themes");
    let _ = std::fs::create_dir_all(&themes);
    for (name, text) in [
        ("dark_glass.toml", theme_file::DARK_GLASS_TOML),
        ("light_clean.toml", theme_file::LIGHT_CLEAN_TOML),
        ("midnight.toml", theme_file::MIDNIGHT_TOML),
        ("pearl.toml", theme_file::PEARL_TOML),
    ] {
        let p = themes.join(name);
        if !p.exists() {
            let _ = std::fs::write(p, text);
        }
    }
}

/// The palette for `config`: a named theme file, or Windows light/dark mode for
/// `auto_system` / `follow_windows_theme`. Invalid names or files fall back to
/// the built-in theme (errors are logged, never fatal).
fn resolve_palette(config: &Config, dir: &std::path::Path) -> Palette {
    let g = &config.general;
    let follow = g.follow_windows_theme || g.theme.eq_ignore_ascii_case("auto_system");
    let name = if follow {
        if winshell::registry::apps_use_light_theme() { "light_clean".to_string() } else { "dark_glass".to_string() }
    } else if g.theme.trim().is_empty() {
        "midnight".to_string()
    } else {
        g.theme.trim().to_string()
    };
    let base = if name.eq_ignore_ascii_case("light_clean") || name.eq_ignore_ascii_case("pearl") {
        theme_file::LIGHT_CLEAN
    } else {
        theme_file::DARK_GLASS
    };
    if !theme_file::valid_theme_name(&name) {
        log!("theme: \"{name}\" is not a valid theme name (letters, digits, _ and - only); using built-in");
        return base;
    }
    let path = dir.join("themes").join(format!("{name}.toml"));
    match std::fs::read_to_string(&path) {
        Ok(text) => {
            let (p, problems) = theme_file::parse(&text, base);
            for pr in problems {
                log!("theme {name}: {pr}");
            }
            p
        }
        Err(_) => {
            log!("theme: {} not found; using built-in", path.display());
            base
        }
    }
}

fn user_handle_for(config: &Config) -> String {
    if config.general.user_handle.trim().is_empty() {
        std::env::var("USERNAME").map(|u| format!("@{}", u.to_lowercase())).unwrap_or_else(|_| "@you".into())
    } else {
        config.general.user_handle.clone()
    }
}

fn style_for(opts: &Options, p: &Palette) -> DwmStyle {
    let mut style = opts.style;
    style.dark = p.dark;
    if !opts.corners_override {
        style.corners = match p.window_corners {
            theme_file::Corners::Round => Corners::Round,
            theme_file::Corners::Small => Corners::Small,
            theme_file::Corners::Square => Corners::Square,
        };
    }
    style
}

fn run(mut opts: Options, config: Config, config_path: PathBuf, taskbar: TaskbarGuard) -> Result<()> {
    let app_dir = config_path.parent().map(|p| p.to_path_buf()).unwrap_or_default();
    let palette = resolve_palette(&config, &app_dir);
    set_pal(palette);
    opts.style = style_for(&opts, &palette);

    win::register_class(TOPBAR_CLASS, wndproc)?;
    win::register_class(WINDOW_CLASS, wndproc)?;
    win::register_class(FLYOUT_CLASS, wndproc)?;
    PLAN_MODE.set(opts.reserve == ReserveMode::Plan);
    TASKBAR_CREATED.set(unsafe { RegisterWindowMessageW(w!("TaskbarCreated")) });

    // UI thread is an STA (PLAN.md section 6); shell property stores and Core Audio need COM.
    unsafe {
        let _ = CoInitializeEx(None, COINIT_APARTMENTTHREADED | COINIT_DISABLE_OLE1DDE);
    }

    let gfx = Gfx::new()?;
    log!("graphics: shared devices created ({})", gfx.driver);

    let bar_ex = WS_EX_NOREDIRECTIONBITMAP
        | WS_EX_TOPMOST
        | if opts.preview { WS_EX_APPWINDOW } else { WS_EX_TOOLWINDOW | WS_EX_NOACTIVATE };
    // The hotkey is registered on the first window, before anything can get stuck on screen.
    let top_bar = Popup::create(&gfx, TOPBAR_CLASS, w!("WinShellTheme TopBar"), bar_ex, opts.style)?;
    match hotkey::register_emergency(top_bar.hwnd) {
        Some(c) => log!("emergency hotkey: {c}"),
        None => log!("WARNING: emergency hotkey unavailable (Esc and F12 combinations both taken)"),
    }
    if !hotkey::register(top_bar.hwnd, HOTKEY_IMMERSIVE, VK_F) {
        log!("Ctrl+Shift+Alt+F unavailable; use the active window menu to restore immersive windows");
    }
    let dock = Popup::create(&gfx, WINDOW_CLASS, w!("WinShellTheme Dock"), bar_ex, opts.style)?;
    let tooltip = Popup::create(&gfx, WINDOW_CLASS, w!("WinShellTheme Tooltip"), bar_ex, opts.style)?;
    // The flyout takes focus so it can close when you click elsewhere (WA_INACTIVE) or press Esc.
    let flyout_ex =
        WS_EX_NOREDIRECTIONBITMAP | WS_EX_TOPMOST | if opts.preview { WS_EX_APPWINDOW } else { WS_EX_TOOLWINDOW };
    let flyout = Popup::create(&gfx, FLYOUT_CLASS, w!("WinShellTheme Flyout"), flyout_ex, opts.style)?;

    let zero = win::rect(0, 0, 1, 1);
    let top_res_hwnd = win::create_popup(WINDOW_CLASS, w!("WinShellTheme TopReserve"), opts.reserve.ex_style(), zero)?;
    let bot_res_hwnd =
        win::create_popup(WINDOW_CLASS, w!("WinShellTheme BottomReserve"), opts.reserve.ex_style(), zero)?;
    TOP_RESERVE_HWND.store(top_res_hwnd.0 as isize, Ordering::SeqCst);
    BOTTOM_RESERVE_HWND.store(bot_res_hwnd.0 as isize, Ordering::SeqCst);
    let top_reserve = AppBar::new(top_res_hwnd, Edge::Top);
    let bottom_reserve = AppBar::new(bot_res_hwnd, Edge::Bottom);

    let monitor = MonitorGeometry::primary();
    let fonts = make_fonts(&gfx, monitor.scale())?;
    let user_handle = user_handle_for(&config);
    let top_hwnd = top_bar.hwnd;
    let audio = Audio::new(top_hwnd);
    if audio.is_none() {
        log!("audio: Core Audio unavailable; volume pill hidden");
    }

    let mut app = App {
        opts,
        config,
        config_path,
        gfx,
        fonts,
        monitor,
        top_reserve,
        bottom_reserve,
        top_bar,
        dock,
        tooltip,
        flyout,
        user_handle,
        fullscreen: false,
        clock: Clock::new(top_hwnd, TIMER_CLOCK),
        clock_text: clock::now_text(),
        tracker: ForegroundTracker::install(top_hwnd),
        active: Active::desktop(),
        bar: BarLayout::default(),
        bar_hover: None,
        first_tick_logged: false,
        dock_model: DockModel::default(),
        window_hooks: WindowEventHooks::install(top_hwnd),
        assets: AssetLoader::start(top_hwnd),
        icons: Lru::new(ICON_CACHE_BYTES),
        icon_requested: HashSet::new(),
        icon_failed: HashSet::new(),
        app_names: HashMap::new(),
        dock_area: zero,
        span: (0, 1),
        dock_layout: DockLayout::default(),
        dock_hover: None,
        dock_scroll: 0,
        pending_launches: Vec::new(),
        tooltip_text: String::new(),
        taskbar,
        audio_state: audio.as_ref().map(|a| a.state()).unwrap_or_default(),
        audio,
        network: Network::start(top_hwnd),
        net_state: NetState::Disconnected,
        battery: power::battery(),
        power_notify: PowerNotifications::register(top_hwnd),
        sysmon: SystemMonitor::start(top_hwnd),
        bluetooth: None,
        wifi_radio: None,
        wifi_networks: Vec::new(),
        bt_devices: Vec::new(),
        edges: autohide::Edges::create()?,
        reveal_bar: false,
        reveal_dock: false,
        media: Vec::new(),
        media_bitmaps: HashMap::new(),
        devices: Vec::new(),
        panel: None,
        watcher: ConfigWatcher::start(&app_dir, top_hwnd),
        app_dir,
        last_self_write: None,
    };
    if app.watcher.is_none() {
        log!("config watcher unavailable; changes apply on restart");
    }
    app.net_state = app.network.state();
    app.sync_reservations();
    app.log_autohide();
    app.load_pins();
    app.rescan("startup");
    app.layout();
    app.opts.reserve.finish(top_res_hwnd);
    app.opts.reserve.finish(bot_res_hwnd);
    app.update_active(unsafe { GetForegroundWindow() });
    app.update_visibility();
    app.redraw_all();
    app.clock.arm();
    app.refresh_clock(); // the minute may have turned since clock_text was built

    // M5: only now that the dock can launch and switch apps.
    if app.config.taskbar.autohide_native && !app.opts.keep_taskbar {
        app.taskbar.enable_autohide();
        app.layout();
        app.redraw_all();
    }
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

impl App {
    pub fn scale(&self) -> f32 {
        self.monitor.scale()
    }

    /// Reserve the edges, then place the visible windows inside the granted bands.
    pub fn layout(&mut self) {
        self.monitor = MonitorGeometry::primary();
        let s = self.scale();
        if (self.fonts.scale - s).abs() > f32::EPSILON {
            match make_fonts(&self.gfx, s) {
                Ok(f) => self.fonts = f,
                Err(e) => log!("font rebuild failed: {e:?}"),
            }
            // Bitmaps are rendered at exact device sizes; redo them for the new scale.
            self.active.icon = self.load_active_icon(self.active.hwnd, self.active.kind);
            self.icons.clear();
            self.icon_requested.clear();
            self.icon_failed.clear();
            self.sync_assets();
        }

        // Overlays never reserve desktop space. Immersive apps use the monitor,
        // while the desktop rail stays clear of the native taskbar in --no-taskbar mode.
        let screen = self.monitor.monitor;
        let work = win::work_area();
        let m = px(BAR_MARGIN, s);
        let bar =
            win::rect(screen.left + m, screen.top + m, (screen.right - screen.left - 2 * m).max(1), px(BAR_HEIGHT, s));
        let margin = px(DOCK_MARGIN, s);
        self.dock_area = RECT {
            left: work.left.max(screen.left) + margin,
            right: screen.right - margin,
            top: work.top.max(bar.bottom) + margin,
            bottom: work.bottom.min(screen.bottom) - margin,
        };
        self.span = (screen.left, screen.right);
        self.place_edges();
        log!(
            "layout: dpi {} bar {} rail area {}",
            self.monitor.dpi,
            win::rect_str(&bar),
            win::rect_str(&self.dock_area)
        );
        self.top_bar.place(&self.gfx, bar);
        self.relayout_dock();
        if self.panel.is_some() {
            self.close_panel();
        }
    }

    pub fn redraw_all(&mut self) {
        self.redraw_bar(None);
        self.redraw_dock();
    }

    pub fn on_draw_error(&mut self, e: windows::core::Error) {
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
            let fonts = make_fonts(&gfx, self.scale())?;
            for p in [&mut self.top_bar, &mut self.dock, &mut self.tooltip, &mut self.flyout] {
                p.surface = winshell::render::WindowSurface::new(&gfx, p.hwnd)?;
                let r = p.rect;
                p.surface.resize(&gfx, (r.right - r.left).max(1) as u32, (r.bottom - r.top).max(1) as u32)?;
            }
            self.gfx = gfx;
            self.fonts = fonts;
            Ok(())
        })();
        if let Err(e) = result {
            log!("graphics rebuild failed: {e:?}");
            return;
        }
        // Every bitmap belonged to the old device.
        self.active.icon = self.load_active_icon(self.active.hwnd, self.active.kind);
        self.icons.clear();
        self.icon_requested.clear();
        self.icon_failed.clear();
        self.media_bitmaps.clear();
        self.sync_media_bitmaps();
        self.sync_assets();
        self.tooltip.hide();
        self.close_panel();
        self.redraw_bar(None);
        self.redraw_dock();
    }

    // ------------------------------------------------------------------
    // Context menus (run outside the state borrow, see MenuRequest)
    // ------------------------------------------------------------------

    pub fn on_menu_command(&mut self, ctx: MenuContext, cmd: u32) {
        match ctx {
            MenuContext::Dock { group_id } => self.on_dock_menu(&group_id, cmd),
            MenuContext::ActiveWindow { hwnd } => self.on_active_menu(HWND(hwnd as _), cmd),
            MenuContext::Shell => self.on_shell_menu(cmd),
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
        if on {
            self.tooltip.hide();
            self.close_panel();
        }
        self.update_visibility();
        log!(
            "fullscreen app {}: bar and dock {}",
            if on { "opened" } else { "closed" },
            if on { "hidden" } else { "shown" }
        );
    }

    fn on_appbar(&mut self, from: HWND, wp: WPARAM, lp: LPARAM) {
        log!("appbar: {}", appbar::describe_notification(wp.0, lp));
        match wp.0 as u32 {
            ABN_POSCHANGED if from == self.primary_reserve() => {
                self.layout();
                self.redraw_all();
            }
            // Both reservations get this; only act on the top one to avoid doing it twice.
            ABN_FULLSCREENAPP
                if from == self.primary_reserve()
                    && (self.top_reserve.registered || self.bottom_reserve.registered) =>
            {
                self.set_fullscreen(lp.0 != 0)
            }
            _ => {}
        }
    }

    /// Explorer restarted: its AppBar list is gone, so register again.
    fn on_taskbar_created(&mut self) {
        log!("Explorer restarted; re-registering AppBars");
        self.top_reserve.registered = false;
        self.bottom_reserve.registered = false;
        self.sync_reservations();
        self.rescan("Explorer restart");
        immersive::reflow();
        self.layout();
        self.redraw_all();
    }

    /// Resume from sleep/hibernate: clock stale, foreground and windows may differ.
    fn on_resume(&mut self) {
        self.resync_clock("resume");
        self.update_active(unsafe { GetForegroundWindow() });
        self.battery = power::battery();
        self.net_state = self.network.state();
        self.sysmon.refresh();
        self.rescan("resume");
        self.relayout_dock();
        self.redraw_all();
    }

    fn on_audio(&mut self, devices: bool) {
        let sound_open = self.panel_is(panel::PanelKind::Sound);
        let Some(a) = self.audio.as_mut() else { return };
        a.ack(devices);
        if devices {
            a.rebind();
        }
        let state = a.state();
        let devices_now = if devices && sound_open { Some(a.render_devices()) } else { None };
        if let Some(d) = devices_now {
            self.devices = d;
        }
        if state != self.audio_state || devices {
            self.audio_state = state;
            self.redraw_bar(None);
            self.refresh_panel_if(panel::PanelKind::Sound);
        }
    }

    fn on_network(&mut self) {
        self.network.ack();
        let s = self.network.state();
        if s != self.net_state {
            self.net_state = s;
            self.redraw_bar(None);
        }
        if self.panel_is(panel::PanelKind::Wifi) {
            self.wifi_networks = self.network.available_networks();
            self.refresh_panel_if(panel::PanelKind::Wifi);
        }
    }

    /// The reservation whose ABN_* notifications we act on (both get them).
    fn primary_reserve(&self) -> HWND {
        if self.top_reserve.registered { self.top_reserve.hwnd } else { self.bottom_reserve.hwnd }
    }

    fn on_power_change(&mut self) {
        let b = power::battery();
        if b != self.battery {
            self.battery = b;
            self.redraw_bar(None);
            self.refresh_panel_if(panel::PanelKind::Battery);
        }
    }

    fn on_monitor(&mut self) {
        let mut bar = false;
        let mut media = false;
        for ev in self.sysmon.drain() {
            match ev {
                winshell::sysmon::MonitorEvent::Radios { bluetooth, wifi } => {
                    if bluetooth != self.bluetooth || wifi != self.wifi_radio {
                        self.bluetooth = bluetooth;
                        self.wifi_radio = wifi;
                        bar = true;
                        if self.panel_is(panel::PanelKind::Wifi) {
                            self.wifi_networks = self.network.available_networks();
                        }
                        self.refresh_panel_if(panel::PanelKind::Wifi);
                        if self.panel_is(panel::PanelKind::Bluetooth) {
                            self.sysmon.refresh_bt_devices();
                        }
                        self.refresh_panel_if(panel::PanelKind::Bluetooth);
                    }
                }
                winshell::sysmon::MonitorEvent::BluetoothDevices(d) => {
                    self.bt_devices = d;
                    self.refresh_panel_if(panel::PanelKind::Bluetooth);
                }
                winshell::sysmon::MonitorEvent::Media(m) => {
                    self.media = m;
                    media = true;
                }
            }
        }
        if bar {
            self.redraw_bar(None);
        }
        if media {
            self.sync_media_bitmaps();
            self.relayout_dock();
            self.redraw_dock();
            self.refresh_panel_if(panel::PanelKind::Sound);
        }
    }

    /// GPU bitmaps for album art, rebuilt only when a session's art changes.
    fn sync_media_bitmaps(&mut self) {
        let mut next = HashMap::new();
        for m in &self.media {
            let Some(art) = &m.art else { continue };
            let key = std::sync::Arc::as_ptr(art) as usize;
            let existing = self.media_bitmaps.get(&m.app_id).filter(|(k, _)| *k == key).map(|(_, b)| b.clone());
            let bmp = existing.or_else(|| self.gfx.bitmap_from_bgra(art.width, art.height, &art.pixels).ok());
            if let Some(b) = bmp {
                next.insert(m.app_id.clone(), (key, b));
            }
        }
        self.media_bitmaps = next;
    }

    pub fn media_bitmap(&self, app_id: &str) -> Option<&ID2D1Bitmap1> {
        self.media_bitmaps.get(app_id).map(|(_, b)| b)
    }

    // ------------------------------------------------------------------
    // M8: live reload
    // ------------------------------------------------------------------

    /// config.toml or a theme file changed on disk.
    fn on_config_changed(&mut self) {
        if let Some(w) = &self.watcher {
            w.ack();
        }
        let text = std::fs::read_to_string(&self.config_path).ok();
        if text.is_some() && text == self.last_self_write {
            // Our own pin/unpin write: already applied. Theme files may still have changed.
            self.last_self_write = None;
            self.apply_theme();
            self.redraw_everything();
            return;
        }
        let new = match text.as_deref().map(config::parse) {
            Some(Ok(c)) => c,
            Some(Err(e)) => {
                log!("config.toml has an error, keeping the previous settings: {e}");
                self.apply_theme();
                self.redraw_everything();
                return;
            }
            None => self.config.clone(), // deleted or locked mid-save: keep current settings
        };
        log!("config reloaded");
        self.apply_settings(new);
    }

    fn apply_settings(&mut self, new: Config) {
        let pins_changed = new.dock.pinned_apps != self.config.dock.pinned_apps;
        let autohide_changed = new.taskbar.autohide_native != self.config.taskbar.autohide_native;
        if self.config.dock.immersive && !new.dock.immersive {
            immersive::restore_all();
            self.pending_launches.clear();
        }
        self.config = new;
        self.user_handle = user_handle_for(&self.config);
        if pins_changed {
            self.load_pins();
        }
        self.sync_reservations();
        self.log_autohide();
        if autohide_changed && !self.opts.keep_taskbar {
            if self.config.taskbar.autohide_native {
                self.taskbar.enable_autohide();
            } else {
                self.taskbar.restore();
            }
        }
        self.apply_theme();
        self.layout();
        self.redraw_everything();
    }

    /// Re-resolve the palette and re-apply DWM attributes (dark frame, corners).
    pub fn apply_theme(&mut self) {
        let p = resolve_palette(&self.config, &self.app_dir);
        set_pal(p);
        let style = style_for(&self.opts, &p);
        if style.dark != self.opts.style.dark || style.corners != self.opts.style.corners {
            self.opts.style = style;
            for h in [self.top_bar.hwnd, self.dock.hwnd, self.tooltip.hwnd, self.flyout.hwnd] {
                dwm::apply(h, style);
            }
        }
    }

    fn redraw_everything(&mut self) {
        self.tooltip.hide();
        if self.panel.is_some() {
            self.close_panel();
        }
        self.redraw_all();
    }

    fn shutdown(&mut self) {
        immersive::restore_all();
        log!("clean exit; {}", MemSnapshot::now().describe());
        if let Some(w) = self.watcher.as_mut() {
            w.stop();
        }
        self.taskbar.restore();
        self.tracker.uninstall();
        self.window_hooks.uninstall();
        self.assets.shutdown();
        self.sysmon.shutdown();
        self.network.shutdown();
        self.power_notify.unregister();
        if let Some(a) = self.audio.as_mut() {
            a.shutdown();
        }
        self.clock.disarm();
        self.top_reserve.remove();
        self.bottom_reserve.remove();
        hotkey::unregister(self.top_bar.hwnd, hotkey::ID_EMERGENCY);
        hotkey::unregister(self.top_bar.hwnd, HOTKEY_IMMERSIVE);
        TOP_RESERVE_HWND.store(0, Ordering::SeqCst);
        BOTTOM_RESERVE_HWND.store(0, Ordering::SeqCst);
        for p in [&self.flyout, &self.tooltip, &self.dock, &self.top_bar] {
            p.destroy();
        }
        self.edges.destroy();
        for h in [self.top_reserve.hwnd, self.bottom_reserve.hwnd] {
            unsafe {
                let _ = DestroyWindow(h);
            }
        }
    }

    fn reserve(&self, hwnd: HWND) -> Option<&AppBar> {
        [&self.top_reserve, &self.bottom_reserve].into_iter().find(|r| r.hwnd == hwnd)
    }

    /// The window currently in front belongs to this process (flyout, menu owner).
    pub fn foreground_is_ours(fg: HWND) -> bool {
        let mut pid = 0u32;
        unsafe {
            GetWindowThreadProcessId(fg, Some(&mut pid));
            pid == windows::Win32::System::Threading::GetCurrentProcessId()
        }
    }
}

// ----------------------------------------------------------------------
// Window procedure plumbing
// ----------------------------------------------------------------------

/// Run `f` on the app state, or re-post the message if the state is busy
/// (re-entrant delivery while e.g. SHAppBarMessage or SetForegroundWindow runs).
fn with_app<R>(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM, f: impl FnOnce(&mut App) -> R) -> Option<R> {
    APP.with(|a| match a.try_borrow_mut() {
        Ok(mut g) => g.as_mut().map(f),
        Err(_) => {
            unsafe {
                let _ = PostMessageW(Some(hwnd), msg, wp, lp);
            }
            None
        }
    })
}

/// Like `with_app`, only for messages addressed to the top bar. Broadcasts
/// (WM_TIMECHANGE, WM_POWERBROADCAST, ...) reach every top-level window.
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

/// The area name in a WM_SETTINGCHANGE lParam ("intl", "ImmersiveColorSet", ...).
/// Only called while the message is being *sent* (InSendMessage), when the system
/// has marshalled the string into this process. A *posted* WM_SETTINGCHANGE can
/// carry an arbitrary pointer from another process, so it is never dereferenced.
fn setting_name(lp: LPARAM) -> String {
    if lp.0 == 0 || !unsafe { InSendMessage() }.as_bool() {
        return String::new();
    }
    let p = lp.0 as *const u16;
    let mut out = Vec::new();
    for i in 0..64 {
        let c = unsafe { *p.add(i) };
        if c == 0 {
            break;
        }
        out.push(c);
    }
    String::from_utf16_lossy(&out)
}

fn mouse_point(lp: LPARAM) -> (f32, f32) {
    ((lp.0 & 0xFFFF) as i16 as f32, ((lp.0 >> 16) & 0xFFFF) as i16 as f32)
}

/// Exit now, restoring the taskbar no matter what (security review F3): if the app
/// state is busy, fall back to the raw cleanup instead of skipping shutdown.
fn quit_now() {
    let done = APP.with(|a| match a.try_borrow_mut() {
        Ok(mut g) => {
            if let Some(app) = g.as_mut() {
                app.shutdown();
            }
            true
        }
        Err(_) => false,
    });
    if !done {
        cleanup_raw();
    }
    unsafe { PostQuitMessage(0) };
}

/// Run a context menu with no app borrow held. Returns the chosen id (0 = none).
fn run_menu(req: &MenuRequest) -> u32 {
    unsafe {
        let Ok(menu) = CreatePopupMenu() else { return 0 };
        for (id, label) in &req.items {
            if *id == 0 {
                let _ = AppendMenuW(menu, MF_SEPARATOR, 0, PCWSTR::null());
            } else {
                let _ = AppendMenuW(menu, MF_STRING, *id as usize, &HSTRING::from(label.as_str()));
            }
        }
        // Required for the menu to close when clicking elsewhere (Win32 menu contract).
        let previous = GetForegroundWindow();
        let _ = SetForegroundWindow(req.owner);
        let align = if req.above { TPM_BOTTOMALIGN } else { TPM_TOPALIGN };
        let cmd = TrackPopupMenu(
            menu,
            TPM_RETURNCMD | TPM_NONOTIFY | TPM_RIGHTBUTTON | TPM_CENTERALIGN | align,
            req.x,
            req.y,
            None,
            req.owner,
            None,
        )
        .0 as u32;
        let _ = PostMessageW(Some(req.owner), WM_NULL, WPARAM(0), LPARAM(0));
        let _ = DestroyMenu(menu);
        if cmd == 0 && !previous.0.is_null() {
            let _ = SetForegroundWindow(previous);
        }
        cmd
    }
}

fn track_leave(hwnd: HWND) {
    let mut t = TRACKMOUSEEVENT {
        cbSize: size_of::<TRACKMOUSEEVENT>() as u32,
        dwFlags: TME_LEAVE,
        hwndTrack: hwnd,
        dwHoverTime: 0,
    };
    unsafe {
        let _ = TrackMouseEvent(&mut t);
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Surface {
    Bar,
    Dock,
    Flyout,
    Other,
}

fn which(a: &App, hwnd: HWND) -> Surface {
    if hwnd == a.top_bar.hwnd {
        Surface::Bar
    } else if hwnd == a.dock.hwnd {
        Surface::Dock
    } else if hwnd == a.flyout.hwnd {
        Surface::Flyout
    } else {
        Surface::Other
    }
}

#[derive(Clone, Copy)]
pub enum Button {
    Left,
    Middle,
    Right,
}

unsafe extern "system" fn wndproc(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    if msg != 0 && msg == TASKBAR_CREATED.get() {
        with_app_on_bar(hwnd, msg, wp, lp, |a| a.on_taskbar_created());
        return LRESULT(0);
    }
    match msg {
        WM_NCHITTEST if PLAN_MODE.get() && is_reserve(hwnd) => LRESULT(HTTRANSPARENT as isize),
        WM_MOUSEACTIVATE => {
            // The flyout takes focus; bar, dock and tooltip never do.
            let flyout = APP.with(|a| a.try_borrow().ok().and_then(|g| g.as_ref().map(|a| a.flyout.hwnd == hwnd)));
            if flyout == Some(true) { LRESULT(MA_ACTIVATE as isize) } else { LRESULT(MA_NOACTIVATE as isize) }
        }
        WM_ERASEBKGND => LRESULT(1),
        WM_PAINT => {
            let mut ps = PAINTSTRUCT::default();
            unsafe {
                BeginPaint(hwnd, &mut ps);
                let _ = EndPaint(hwnd, &ps);
            }
            LRESULT(0)
        }
        WM_SHELL_QUIT => {
            log!("quit requested (--quit)");
            quit_now();
            LRESULT(0)
        }
        WM_ENDSESSION_MSG if wp.0 != 0 => {
            // Logoff / shutdown: restore the taskbar so the user never logs in to a hidden one.
            log!("session ending");
            quit_now();
            LRESULT(0)
        }
        WM_SHELL_RESTORE => {
            with_app_on_bar(hwnd, msg, wp, lp, |a| {
                log!("second launch: showing the shell");
                a.fullscreen = true;
                a.set_fullscreen(false);
                a.layout();
                a.redraw_all();
            });
            LRESULT(0)
        }
        WM_HOTKEY if wp.0 as i32 == HOTKEY_IMMERSIVE => {
            with_app_on_bar(hwnd, msg, wp, lp, |a| {
                a.toggle_immersive();
            });
            LRESULT(0)
        }
        WM_MOUSEWHEEL => {
            let delta = ((wp.0 >> 16) & 0xFFFF) as i16;
            with_app(hwnd, msg, wp, lp, |a| {
                if delta != 0 {
                    match which(a, hwnd) {
                        Surface::Dock => a.scroll_dock(if delta > 0 { -1 } else { 1 }),
                        Surface::Flyout => a.scroll_panel(if delta > 0 { -1.0 } else { 1.0 }),
                        _ => {}
                    }
                }
            });
            LRESULT(0)
        }
        WM_HOTKEY if wp.0 as i32 == hotkey::ID_EMERGENCY => {
            log!("emergency hotkey pressed");
            quit_now();
            LRESULT(0)
        }
        WM_MOUSEMOVE
            if APP.with(|a| {
                a.try_borrow().ok().and_then(|g| g.as_ref().and_then(|app| app.edges.is_edge(hwnd))).is_some()
            }) =>
        {
            with_app(hwnd, msg, wp, lp, |a| {
                if let Some(top) = a.edges.is_edge(hwnd) {
                    a.on_edge_hover(top);
                }
            });
            LRESULT(0)
        }
        WM_MOUSEMOVE => {
            let (x, y) = mouse_point(lp);
            with_app(hwnd, msg, wp, lp, |a| {
                let surface = which(a, hwnd);
                let needs_track = match surface {
                    Surface::Bar => !std::mem::replace(&mut a.top_bar.tracking_leave, true),
                    Surface::Dock => !std::mem::replace(&mut a.dock.tracking_leave, true),
                    Surface::Flyout => !std::mem::replace(&mut a.flyout.tracking_leave, true),
                    Surface::Other => false,
                };
                if needs_track {
                    track_leave(hwnd);
                }
                match surface {
                    Surface::Bar => a.bar_mouse_move(x, y),
                    Surface::Dock => a.dock_mouse_move(x, y),
                    Surface::Flyout => a.panel_mouse_move(x, y),
                    Surface::Other => {}
                }
            });
            LRESULT(0)
        }
        WM_MOUSELEAVE => {
            with_app(hwnd, msg, wp, lp, |a| match which(a, hwnd) {
                Surface::Bar => {
                    a.top_bar.tracking_leave = false;
                    a.bar_mouse_move(-1.0, -1.0);
                    a.on_surface_left(true);
                }
                Surface::Dock => {
                    a.dock.tracking_leave = false;
                    a.dock_mouse_move(-1.0, -1.0);
                    a.on_surface_left(false);
                }
                Surface::Flyout => {
                    a.flyout.tracking_leave = false;
                    a.panel_mouse_move(-1.0, -1.0);
                }
                Surface::Other => {}
            });
            LRESULT(0)
        }
        WM_LBUTTONDOWN => {
            let (x, y) = mouse_point(lp);
            with_app(hwnd, msg, wp, lp, |a| {
                if which(a, hwnd) == Surface::Flyout {
                    a.panel_mouse_down(x, y);
                }
            });
            LRESULT(0)
        }
        WM_LBUTTONUP | WM_MBUTTONUP | WM_RBUTTONUP => {
            let (x, y) = mouse_point(lp);
            let button = match msg {
                WM_LBUTTONUP => Button::Left,
                WM_MBUTTONUP => Button::Middle,
                _ => Button::Right,
            };
            let menu = with_app(hwnd, msg, wp, lp, |a| match which(a, hwnd) {
                Surface::Bar => a.bar_click(x, y, button),
                Surface::Dock => a.dock_click(x, y, button),
                Surface::Flyout => {
                    if matches!(button, Button::Left) {
                        a.panel_mouse_up(x, y);
                    }
                    None
                }
                Surface::Other => None,
            })
            .flatten();
            if let Some(req) = menu {
                let cmd = run_menu(&req);
                if cmd != 0 {
                    let ctx = req.context.clone();
                    with_app(hwnd, WM_NULL, WPARAM(0), LPARAM(0), |a| a.on_menu_command(ctx, cmd));
                }
            }
            LRESULT(0)
        }
        WM_CAPTURECHANGED => {
            with_app(hwnd, msg, wp, lp, |a| {
                if which(a, hwnd) == Surface::Flyout {
                    a.panel_capture_lost();
                }
            });
            LRESULT(0)
        }
        WM_ACTIVATE => {
            let inactive = (wp.0 & 0xFFFF) as u32 == WA_INACTIVE;
            if is_reserve(hwnd) {
                APP.with(|a| {
                    if let Ok(g) = a.try_borrow()
                        && let Some(r) = g.as_ref().and_then(|app| app.reserve(hwnd))
                    {
                        r.on_activate();
                    }
                });
            } else if inactive {
                with_app(hwnd, msg, wp, lp, |a| {
                    if a.flyout.hwnd == hwnd {
                        a.close_panel();
                    }
                });
            }
            unsafe { DefWindowProcW(hwnd, msg, wp, lp) }
        }
        WM_KEYDOWN if wp.0 as u16 == VK_ESCAPE.0 => {
            with_app(hwnd, msg, wp, lp, |a| a.close_panel());
            LRESULT(0)
        }
        WM_TIMER => {
            match wp.0 {
                TIMER_CLOCK => with_app_on_bar(hwnd, msg, wp, lp, |a| a.on_clock_timer()),
                TIMER_AUTOHIDE_BAR | TIMER_AUTOHIDE_DOCK => {
                    unsafe {
                        let _ = KillTimer(Some(hwnd), wp.0);
                    }
                    with_app_on_bar(hwnd, msg, wp, lp, |a| a.on_autohide_timer(wp.0));
                }
                TIMER_TOOLTIP_HIDE => {
                    unsafe {
                        let _ = KillTimer(Some(hwnd), TIMER_TOOLTIP_HIDE);
                    }
                    with_app(hwnd, msg, wp, lp, |a| a.hide_tooltip());
                }
                _ => {}
            }
            LRESULT(0)
        }
        WM_SHELL_FOREGROUND => {
            with_app_on_bar(hwnd, msg, wp, lp, |a| a.on_foreground());
            LRESULT(0)
        }
        winshell::foreground::WM_SHELL_FGMOVE => {
            with_app_on_bar(hwnd, msg, wp, lp, |a| {
                a.tracker.ack_move();
                a.update_visibility();
            });
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
        WM_SHELL_AUDIO => {
            with_app_on_bar(hwnd, msg, wp, lp, |a| a.on_audio(wp.0 == AUDIO_DEVICES_CHANGED));
            LRESULT(0)
        }
        WM_SHELL_NETWORK => {
            with_app_on_bar(hwnd, msg, wp, lp, |a| a.on_network());
            LRESULT(0)
        }
        WM_SHELL_MONITOR => {
            with_app_on_bar(hwnd, msg, wp, lp, |a| a.on_monitor());
            LRESULT(0)
        }
        WM_TIMECHANGE => {
            with_app_on_bar(hwnd, msg, wp, lp, |a| a.resync_clock("WM_TIMECHANGE"));
            LRESULT(0)
        }
        WM_SHELL_CONFIG => {
            with_app_on_bar(hwnd, msg, wp, lp, |a| a.on_config_changed());
            LRESULT(0)
        }
        WM_SETTINGCHANGE => {
            let name = setting_name(lp);
            let kind = if name.eq_ignore_ascii_case("intl") {
                SETTING_INTL
            } else if name.eq_ignore_ascii_case("ImmersiveColorSet") {
                SETTING_COLORS
            } else {
                0
            };
            if kind != 0 {
                unsafe {
                    let _ = PostMessageW(Some(hwnd), WM_SHELL_SETTING, WPARAM(kind), LPARAM(0));
                }
            }
            unsafe { DefWindowProcW(hwnd, msg, wp, lp) }
        }
        WM_SHELL_SETTING if wp.0 == SETTING_COLORS => {
            with_app_on_bar(hwnd, msg, wp, lp, |a| {
                let g = &a.config.general;
                if g.follow_windows_theme || g.theme.eq_ignore_ascii_case("auto_system") {
                    a.apply_theme();
                    a.redraw_everything();
                }
            });
            LRESULT(0)
        }
        WM_SHELL_SETTING if wp.0 == SETTING_INTL => {
            with_app_on_bar(hwnd, msg, wp, lp, |a| {
                a.clock_text.clear(); // force a redraw in the new format
                a.refresh_clock();
                a.refresh_panel_if(panel::PanelKind::Calendar);
            });
            LRESULT(0)
        }
        WM_POWERBROADCAST => {
            match wp.0 as u32 {
                PBT_APMRESUMEAUTOMATIC => with_app_on_bar(hwnd, msg, wp, lp, |a| a.on_resume()),
                PBT_POWERSETTINGCHANGE | PBT_APMPOWERSTATUSCHANGE => {
                    with_app_on_bar(hwnd, msg, wp, lp, |a| a.on_power_change())
                }
                _ => {}
            }
            LRESULT(1)
        }
        WM_APPBAR_CALLBACK => {
            with_app(hwnd, msg, wp, lp, |a| a.on_appbar(hwnd, wp, lp));
            LRESULT(0)
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
                immersive::reflow();
                a.layout();
                a.redraw_all();
            });
            LRESULT(0)
        }
        WM_CLOSE => LRESULT(0),
        _ => unsafe { DefWindowProcW(hwnd, msg, wp, lp) },
    }
}
