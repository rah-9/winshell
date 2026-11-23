# WinShellTheme

An experimental desktop customization app for **Windows 11 (22H2+)**, built in Rust with a focus on memory efficiency and interactive controls.

WinShellTheme delivers a compact translucent top bar, a thin vertical left dock, and interactive flyout panels rendered with **Direct2D**, **DirectWrite**, and **DirectComposition** over native **Desktop Window Manager (DWM)** backdrops (Acrylic & Mica).

`explorer.exe` continues running in the background; WinShellTheme requires **no administrator privileges**, uses no kernel hooks or DLL injection, and hides the native Windows taskbars while running, with saved visibility, work areas, and recovery on the next launch after a forced exit.

## Pilot Project & Collaboration

WinShellTheme is a **pilot project**, open to contributions and collaboration. The goal is to refresh the traditional Windows desktop with a modern, interactive experience built in **Rust**, with a focus on memory efficiency and low resource usage.

Developers, designers, and testers are welcome to help improve the interface, features, performance, and reliability. If you would like to collaborate, contact **Rahul** at [rahul.dev.xai@gmail.com](mailto:rahul.dev.xai@gmail.com).

**Status: early pilot, actively under development.** Taskbar hiding, multiple screens, panel placement, button/scroll behavior, and app fitting remain open work. Some Wi-Fi/Bluetooth actions currently open native Windows settings or pickers. Phone integration and Terminal customization are later stages.

Start with the [contribution guide](CONTRIBUTING.md) and [prioritized roadmap](ROADMAP.md), or [report a bug](https://github.com/rah-9/winshell/issues/new?template=bug_report.yml). Contributions can be small: a reliable reproduction, a test report, a UX improvement, or a focused fix.


---

## Highlights & Features

### 🖥️ Compact Floating Top Bar
- **Unobstructed desktop**: The bar is capped at 920 logical pixels and floats at the top center. Desktop auto-hide keeps folders and icons clear; touching the top or left edge reveals the shell.
- **Active Window Status**: Displays the current application icon and title (intelligently truncated), following focus and title changes live via targeted WinEvent hooks. Click to minimize/restore; right-click to access quick window-management actions (Minimize, Maximize, Snap Left, Snap Right, Close).
- **Custom User Badge**: Shows your user handle (or `@` + Windows username) with a translucent capsule background.
- **Localized Minute Clock**: Precise minute turnover synchronized via a one-shot timer with zero drift and zero wakeups between minutes.
- **Dynamic Status Pills**: Live, callback-driven indicators for Bluetooth, Wi-Fi / Ethernet, Audio Volume & Mute, Battery & Power status, Notifications (Win+N), and Quick Settings (Win+A). Unused hardware (e.g. absent battery or missing Bluetooth radio) automatically hides.

### 🚀 Slim Vertical Left Dock
- **Compact layout**: A 48 px glass rail with 28 px app icons at 100% scaling. It sits on the left, keeps pinned apps first, and scrolls through overflow with the mouse wheel or arrow buttons. Tooltips and window pickers open beside it.
- **Immersive apps**: Dock launches and window selections use reversible borderless full screen by default. `Ctrl+Shift+Alt+F` restores the active window. Original styles and positions are restored on exit, panic, emergency exit, or next launch after a forced exit. Fixed-size dialogs and administrator windows keep their own behavior.
- **Start Button**: Integrated Start launcher that cleanly triggers the native Windows Start menu (`VK_LWIN`).
- **Pinned & Running Applications**: Windows are automatically grouped by application identity (AUMID for packaged/Store apps and browser profiles, executable path for desktop apps). Pinned applications stay in place; running applications show dot indicators for active instances and an accent pill under the focused window.
- **Hover Zoom & Tooltips**: Smooth magnification on hover (`1.15x`) and responsive capsule tooltips.
- **Window Management**: Left-click to switch or launch, middle-click to spawn a new instance, right-click to open context menus (Pin/Unpin, Close, Quit app), and a multi-window picker flyout when an application has several open windows.
- **Compact Media Widget**: Album art and play/pause stay on the rail. Hover for track details, or click the art for the Sound panel and full transport controls.

### 🎛️ Interactive Flyout Panels
Fast, hardware-accelerated flyout panels sliding smoothly into view with DirectComposition transitions:
- **Sound Flyout**: Master volume and microphone sliders with drag support, mute toggles, quick link to Windows Volume Mixer, audio output device switcher (`IPolicyConfig`), and active media playback cards with album art and playback controls.
- **Wi-Fi Flyout**: In-shell network discovery listing nearby SSIDs with signal-strength meters, security badges, one-click connection to saved profiles, disconnect toggle, and adapter radio power switch.
- **Bluetooth Flyout**: Radio on/off switch and paired device list displaying connection statuses ("Connected" vs "Paired") with safe 5-second discovery timeouts.
- **Battery Flyout**: Battery charge percentage, AC power status, estimated battery time remaining, and battery saver status.
- **Calendar Flyout**: Current localized date header, Gregorian 6x7 month grid navigation, previous/next month browsing, and one-click return to today.

### 👻 Smart Auto-Hide Engine
- **Full Display Utilization**: The bar and rail never reserve screen space, even with auto-hide disabled. Immersive windows fill their monitor without shell margins; replacement mode also releases the native taskbar margin so ordinary maximized windows can use the full work area.
- **Context-Aware Visibility**: Automatically hides the overlay when an application window is maximized, true full-screen, or overlaps the bar or dock. Windows maximized on secondary monitors will not hide the primary monitor's overlay.
- **Invisible Edge Reveal Triggers**: Move the mouse cursor to the top or left edge of the screen to reveal the bar or dock.
- **Caption-Button Safe Zone**: The top edge reveal strip leaves the top-right 160 px free, allowing you to click maximized window Close, Maximize, and Minimize buttons without accidentally triggering the top bar.
- **Full-Screen Access**: Shell-managed immersive windows still allow top/left edge reveal. External full-screen games and video keep edge pass-through.

### ⚡ Extreme Performance & Efficiency
- **Memory Footprint**: Target idle Private Working Set $\le 30\text{--}32\text{ MB}$.
- **Zero CPU Idle**: Built entirely on event callbacks (`WinEventHooks`, `IMMNotificationClient`, `WlanRegisterNotification`, `PowerSettingNotification`, WinRT `StateChanged`). No periodic polling loops.
- **Hard-Budgeted Texture Cache**: Custom LRU icon texture cache strictly bounded to an 8 MB ceiling.

### 🔒 Safety, Reversibility & Process Hardening
- **Native taskbar replacement**: Primary and secondary Windows taskbar windows are hidden through normal window APIs. Explorer and desktop icons remain running. The taskbar state, visibility, and monitor work areas are flushed and atomically saved before changes. Exit and emergency exit restore them; a forced kill is repaired on the next launch. Failed restoration retains the recovery file. Explorer restart and taskbar show events reapply suppression without polling.
- **Emergency Panic Hotkey**: Press **`Ctrl + Shift + Alt + Esc`** (or `Ctrl + Shift + Alt + F12` fallback) at any time to immediately restore native desktop boundaries and exit.
- **User-Mode Execution**: Operates strictly as `asInvoker`. No administrative privileges required.
- **Process Hardening**: Restricts DLL search directories exclusively to `System32`, strips the current directory from the search path, rejects UNC/low-integrity DLLs, and disables legacy application extension points.

### 🎨 Live Theming & Hot Reload
- Supports custom themes via TOML files in `%APPDATA%\WinShellTheme\themes\`.
- New installs default to **Midnight**, a soft graphite glass theme. **Pearl**, **Dark Glass**, and **Light Clean** are also included.
- Click the user badge to open **Make it yours**: themes, compact layout, desktop clearance, bar/dock auto-hide, magnification, immersive launches, and Windows taskbar visibility. The dock gear opens **Your desktop** for sound, networks, devices, battery, calendar, Show Desktop, and Windows settings. Existing settings and custom theme files are preserved.
- Can follow system dark/light mode automatically (`theme = "auto_system"`).
- Background file watcher (`ReadDirectoryChangesW`) reloads configuration and theme adjustments instantly without restarting the application.

---

## Requirements

- **Operating System**: Windows 11 22H2 (Build 22621) or newer.
- **Build Requirements** (for building from source):
  - Stable Rust (2024 edition).
  - MSVC toolchain (Visual Studio Build Tools 2022 + Windows 11 SDK).

---

## Build, Run & Install

The executable is not committed to this repository. GitHub's **Download ZIP** downloads the source code, not a runnable Windows app. Build `winshell.exe` using the steps below.

### Build from source

Install stable Rust with the MSVC toolchain, Visual Studio Build Tools 2022 with C++ support, and the Windows 11 SDK. Then run these commands in PowerShell:

```powershell
git clone https://github.com/rah-9/winshell.git
cd winshell
cargo build --release --locked
```

The app is built at `target\release\winshell.exe`. No administrator privileges are needed to run it.

### Try the app

From the repository folder, preview the interface and open appearance settings without hiding the native Windows taskbar:

```powershell
.\target\release\winshell.exe --preview --settings
```

Close the preview before starting normal replacement mode:

```powershell
Start-Process -FilePath .\target\release\winshell.exe -ArgumentList '--quit' -Wait -WindowStyle Hidden
.\target\release\winshell.exe --replace-taskbar --settings
```

Replacement mode hides the native Windows taskbars and releases their screen margins while the app runs. Desktop auto-hide keeps folders clear; touch the **top edge** for the bar or the **left edge** for the dock. Press **Ctrl+Shift+Alt+Space** to open the desktop control panel.

Click the user badge for themes and behavior switches, or click the dock gear for sound, networks, devices, calendar, and desktop shortcuts. Existing settings and pinned apps are preserved.

### Exit and recover

Press **Ctrl+Shift+Alt+Esc** to restore the desktop and exit, or run:

```powershell
.\target\release\winshell.exe --quit
```

After a forced termination, relaunching the app repairs saved desktop state. To repair without starting the shell:

```powershell
.\target\release\winshell.exe --restore-taskbar
```

This command also restores saved immersive-window placement. If restoration cannot complete, the recovery files are kept for another attempt.

### Install for the current user

After building, pass the executable's location to the installer:

```powershell
powershell -ExecutionPolicy Bypass -File .\scripts\install.ps1 -Source .\target\release\winshell.exe
```

The installer copies the app to `%LOCALAPPDATA%\Programs\WinShellTheme`, enables autostart, and launches it. Add `-NoAutostart -NoStart` to install without enabling startup or launching the app.

To uninstall and restore the native desktop:

```powershell
powershell -ExecutionPolicy Bypass -File .\scripts\uninstall.ps1
```

Settings are retained unless `-RemoveSettings` is supplied. Uninstallation stops if desktop restoration is incomplete.

### Optional diagnostic utility

The spike is excluded from normal builds. Build it explicitly:

```powershell
cargo build --release --locked --features spike
```

Its executable is `target\release\spike.exe`.

---

## Command-Line Usage

```text
winshell.exe [OPTIONS]

Options:
  --preview            Show targetable preview surfaces without changing the native taskbar
  --settings           Open appearance settings (also works on a running shell)
  --replace-taskbar    Hide native taskbars for this launch, overriding saved preferences
  --diagnose           Report taskbar visibility and monitor work areas
  --quit               Politely ask the running shell to exit and restore the taskbar
  --restore-taskbar    Restore the native Windows taskbar from a leftover state file
  --enable-autostart   Register winshell in HKCU Run to start with Windows
  --disable-autostart  Remove winshell from HKCU Run
  --no-taskbar         Do not modify the native Windows taskbar during this session
  --reserve <MODE>     Edge reservation mode: layered (default), hidden, plan
  --backdrop <TYPE>    DWM backdrop: acrylic (default), mica, tabbed, none
  --corners <SHAPE>    Window corners: round (default), small, none
  --help               Print help information
```

> **Note**: Launching `winshell.exe` while an instance is already running will safely bring the existing instance back to the foreground.

---

## Keyboard Shortcuts

| Shortcut | Action |
| :--- | :--- |
| **`Ctrl + Shift + Alt + Esc`** | **Emergency Exit**: restores native taskbar, frees screen reservations, and exits cleanly (falls back to `F12` if Esc is occupied). |
| **`Win`** (tap Start button) | Toggles the native Windows Start Menu. |
| **`Ctrl + Shift + Alt + F`** | Toggle reversible immersive full screen for the active window. |
| **`Ctrl + Shift + Alt + Space`** | Open the desktop control panel. |
| **Mouse wheel on dock** | Scroll application overflow. |
| **User badge** | Open appearance settings; right-click for the compact menu. |
| **`Esc`** | Closes any open flyout panel. |

---

## Configuration

On first run, configuration is generated at `%APPDATA%\WinShellTheme\config.toml`. Edits made to this file apply live without restarting:

```toml
[general]
user_handle = ""                 # Leave empty for "@" + Windows username
theme = "midnight"               # "midnight", "pearl", "dark_glass", "light_clean", or "auto_system"
follow_windows_theme = false     # Automatically switch theme with Windows light/dark mode
desktop_autohide = true          # Keep desktop icons clear until an edge is touched

[top_bar]
compact = true                  # Floating bar capped at 920 logical pixels
enabled_pills = [
    "bluetooth",
    "wifi",
    "volume",
    "battery",
    "clock",
    "notifications",
    "quick_settings"
]
autohide = true                  # Automatically hides when windows overlap; touch top edge to reveal

[dock]
immersive = true                 # Borderless full screen for dock launches and selections
magnification = false            # Enlarge hovered dock icons to 1.15x
autohide = true                  # Automatically hides over maximized apps; touch left edge to reveal
pinned_apps = [                  # Executable names, full paths, or AUMIDs
    "explorer.exe",
    "msedge.exe"
]

[taskbar]
hide_native = true              # Show only WinShellTheme; restore Windows on exit
autohide_native = true           # Autohides native Windows taskbar while winshell runs
```

---

## File Locations

| Path | Purpose |
| :--- | :--- |
| `%APPDATA%\WinShellTheme\config.toml` | User configuration and pinned applications |
| `%APPDATA%\WinShellTheme\themes\*.toml` | Custom theme palettes and geometry definitions |
| `%APPDATA%\WinShellTheme\winshell.log` | Application log (auto-rotated at 1 MB) |
| `%APPDATA%\WinShellTheme\immersive_state.toml` | Window styles and positions, flushed before immersive changes |
| `%APPDATA%\WinShellTheme\taskbar_state.toml` | Native taskbar state backup (deleted on clean restore) |
| `%LOCALAPPDATA%\Programs\WinShellTheme\` | Default installation directory |

---

## Architecture & Codebase Map

| Module | Description |
| :--- | :--- |
| [`src/shell/mod.rs`](src/shell/mod.rs) | Shell application lifecycle, Win32 window message dispatch, and popup manager |
| [`src/shell/bar.rs`](src/shell/bar.rs) | Top bar layout, active window status pill, status pills rendering |
| [`src/shell/dock.rs`](src/shell/dock.rs) | Vertical rail, app slots, overflow navigation, media controls, tooltips and menus |
| [`src/shell/immersive.rs`](src/shell/immersive.rs) | Borderless full screen, reversible window placement, and recovery journal |
| [`src/shell/panel.rs`](src/shell/panel.rs) | Flyout panels (Sound, Wi-Fi, Bluetooth, Battery, Calendar, Window List) |
| [`src/shell/autohide.rs`](src/shell/autohide.rs) | Dynamic auto-hide engine, invisible edge triggers, safe-zone handling |
| [`src/render.rs`](src/render.rs) | Direct2D 1.1, Direct3D 11, DirectWrite, and DirectComposition surface rendering |
| [`src/dwm.rs`](src/dwm.rs) | Windows 11 DWM backdrop (Acrylic/Mica), corner preferences, and frame extension |
| [`src/appbar.rs`](src/appbar.rs) | Windows AppBar edge reservation engine (`SHAppBarMessage`) |
| [`src/foreground.rs`](src/foreground.rs) | Foreground window tracking, location hooks, and shell host filtering |
| [`src/winevents.rs`](src/winevents.rs) | Window lifecycle hooks (creation, destruction, show, hide, cloaking) |
| [`src/audio.rs`](src/audio.rs) | Core Audio endpoint volume, mute controls, and device routing |
| [`src/network.rs`](src/network.rs) | WLAN scanning, profile connection, and network interface notifications |
| [`src/sysmon.rs`](src/sysmon.rs) | Background WinRT worker for Bluetooth radios, GSMTC media playback, and album art |
| [`src/dock_model.rs`](src/dock_model.rs) | Pure data model grouping windows into application slots (unit-tested) |
| [`src/taskbar.rs`](src/taskbar.rs) | Safe taskbar state persistence and restoration guard |
| [`src/theme_file.rs`](src/theme_file.rs) | Theme file parser and palette validator (unit-tested) |
| [`src/config.rs`](src/config.rs) | TOML configuration parser with format-preserving pin/unpin edits |

---

## License

Dual-licensed under either **MIT** or **Apache-2.0** at your option.

## Validation and limitations

Run `cargo test` after code changes. The latest validated shell update passes **67 tests**. The suite includes a native Win32 fixture that checks immersive monitor bounds, style/placement restoration, and recovery from the saved journal, plus rail geometry tests for 100%, 125%, 150%, and 200% scaling.

Build the diagnostic spike separately with `cargo build --release --features spike`.

Immersive mode uses documented borderless-window APIs, not an application's F11 command. Apps that draw their own title bars may retain those controls. A forced process kill is repaired on the next shell launch. No watchdog process runs. Taskbar recovery now includes visibility and per-monitor work areas. Work-area restoration applies only on matching displays when the current geometry is still the value applied by this shell, preserving later changes by other tools.

Current behavior and settings are described in this README and the source code.

Replacement mode uses documented window and work-area APIs; see [Microsoft work-area documentation](https://learn.microsoft.com/en-us/windows/win32/gdi/multiple-monitor-system-metrics). It does not replace Explorer, install a service, or modify Terminal settings. Terminal customization is a separate stage.
