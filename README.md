# WinShellTheme

A high-performance, lightweight, and fully customizable desktop shell overlay for **Windows 11 (22H2+)**, crafted in Rust. 

WinShellTheme delivers a clean translucent top bar, an application dock, and interactive flyout panels rendered with **Direct2D**, **DirectWrite**, and **DirectComposition** over native **Desktop Window Manager (DWM)** backdrops (Acrylic & Mica). 

`explorer.exe` continues running in the background; WinShellTheme requires **no administrator privileges**, uses no kernel hooks or DLL injection, and safely manages the native Windows taskbar with crash-proof state restoration.

---

## Highlights & Features

### 🖥️ Translucent Top Bar
- **Active Window Status**: Displays the current application icon and title (intelligently truncated), following focus and title changes live via targeted WinEvent hooks. Click to minimize/restore; right-click to access quick window-management actions (Minimize, Maximize, Snap Left, Snap Right, Close).
- **Custom User Badge**: Shows your user handle (or `@` + Windows username) with a translucent capsule background.
- **Localized Minute Clock**: Precise minute turnover synchronized via a one-shot timer with zero drift and zero wakeups between minutes.
- **Dynamic Status Pills**: Live, callback-driven indicators for Bluetooth, Wi-Fi / Ethernet, Audio Volume & Mute, Battery & Power status, Notifications (Win+N), and Quick Settings (Win+A). Unused hardware (e.g. absent battery or missing Bluetooth radio) automatically hides.

### 🚀 Floating Application Dock
- **Start Button**: Integrated Start launcher that cleanly triggers the native Windows Start menu (`VK_LWIN`).
- **Pinned & Running Applications**: Windows are automatically grouped by application identity (AUMID for packaged/Store apps and browser profiles, executable path for desktop apps). Pinned applications stay in place; running applications show dot indicators for active instances and an accent pill under the focused window.
- **Hover Zoom & Tooltips**: Smooth magnification on hover (`1.15x`) and responsive capsule tooltips.
- **Window Management**: Left-click to switch or launch, middle-click to spawn a new instance, right-click to open context menus (Pin/Unpin, Close, Quit app), and a multi-window picker flyout when an application has several open windows.
- **Mini Media Player Widget**: Shows album art, track title, artist, and play/pause controls for active media sessions (Spotify, browsers, media players) right on the dock.

### 🎛️ Interactive Flyout Panels
Fast, hardware-accelerated flyout panels sliding smoothly into view with DirectComposition transitions:
- **Sound Flyout**: Master volume and microphone sliders with drag support, mute toggles, quick link to Windows Volume Mixer, audio output device switcher (`IPolicyConfig`), and active media playback cards with album art and playback controls.
- **Wi-Fi Flyout**: In-shell network discovery listing nearby SSIDs with signal-strength meters, security badges, one-click connection to saved profiles, disconnect toggle, and adapter radio power switch.
- **Bluetooth Flyout**: Radio on/off switch and paired device list displaying connection statuses ("Connected" vs "Paired") with safe 5-second discovery timeouts.
- **Battery Flyout**: Battery charge percentage, AC power status, estimated battery time remaining, and battery saver status.
- **Calendar Flyout**: Current localized date header, Gregorian 6x7 month grid navigation, previous/next month browsing, and one-click return to today.

### 👻 Smart Auto-Hide Engine
- **Full Display Utilization**: When auto-hide is enabled for the bar or dock (`config.toml`), screen work-area reservations are released, allowing maximized applications to take up 100% of the screen.
- **Context-Aware Visibility**: Automatically hides the overlay when an application window is maximized, true full-screen, or overlaps the bar or dock. Windows maximized on secondary monitors will not hide the primary monitor's overlay.
- **Invisible Edge Reveal Triggers**: Move the mouse cursor to the top or bottom edge of the screen to reveal the bar or dock.
- **Caption-Button Safe Zone**: The top edge reveal strip leaves the top-right 160 px free, allowing you to click maximized window Close, Maximize, and Minimize buttons without accidentally triggering the top bar.
- **Full-Screen Pass-Through**: Edge reveal triggers automatically deactivate when true full-screen applications (games, full-screen video players) are active.

### ⚡ Extreme Performance & Efficiency
- **Memory Footprint**: Target idle Private Working Set $\le 30\text{--}32\text{ MB}$.
- **Zero CPU Idle**: Built entirely on event callbacks (`WinEventHooks`, `IMMNotificationClient`, `WlanRegisterNotification`, `PowerSettingNotification`, WinRT `StateChanged`). No periodic polling loops.
- **Hard-Budgeted Texture Cache**: Custom LRU icon texture cache strictly bounded to an 8 MB ceiling.

### 🔒 Safety, Reversibility & Process Hardening
- **Crash-Proof Taskbar Restoration**: When hiding the native Windows taskbar, its exact state (`ABM_GETSTATE`) is atomically written to `%APPDATA%\WinShellTheme\taskbar_state.toml` *before* any change is made. The native taskbar is restored automatically on clean exit, panic/crash hook, emergency hotkey, or upon next startup if terminated unexpectedly.
- **Emergency Panic Hotkey**: Press **`Ctrl + Shift + Alt + Esc`** (or `Ctrl + Shift + Alt + F12` fallback) at any time to immediately restore native desktop boundaries and exit.
- **User-Mode Execution**: Operates strictly as `asInvoker`. No administrative privileges required.
- **Process Hardening**: Restricts DLL search directories exclusively to `System32`, strips the current directory from the search path, rejects UNC/low-integrity DLLs, and disables legacy application extension points.

### 🎨 Live Theming & Hot Reload
- Supports custom themes via TOML files in `%APPDATA%\WinShellTheme\themes\`.
- Comes with **Dark Glass** and **Light Clean** preconfigured.
- Can follow system dark/light mode automatically (`theme = "auto_system"`).
- Background file watcher (`ReadDirectoryChangesW`) reloads configuration and theme adjustments instantly without restarting the application.

---

## Requirements

- **Operating System**: Windows 11 22H2 (Build 22621) or newer.
- **Build Requirements** (for building from source):
  - Stable Rust (2024 edition).
  - MSVC toolchain (Visual Studio Build Tools 2022 + Windows 11 SDK).

---

## Installation & Quick Start

### Quick Install (Pre-built)
If you have a built executable:
```powershell
# Installs to %LOCALAPPDATA%\Programs\WinShellTheme and registers autostart
powershell -ExecutionPolicy Bypass -File .\scripts\install.ps1
```

To uninstall and restore the native taskbar:
```powershell
powershell -ExecutionPolicy Bypass -File .\scripts\uninstall.ps1
```

### Building From Source
Clone the repository and build with release optimizations:
```powershell
cargo build --release
```
The compiled binaries will be output to:
- `target\release\winshell.exe` (main shell application)
- `target\release\spike.exe` (diagnostic & benchmarking utility)

---

## Command-Line Usage

```text
winshell.exe [OPTIONS]

Options:
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
| **`Esc`** | Closes any open flyout panel. |

---

## Configuration

On first run, configuration is generated at `%APPDATA%\WinShellTheme\config.toml`. Edits made to this file apply live without restarting:

```toml
[general]
user_handle = ""                 # Leave empty for "@" + Windows username
theme = "dark_glass"             # "dark_glass", "light_clean", or a custom theme name
follow_windows_theme = false     # Automatically switch theme with Windows light/dark mode

[top_bar]
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
magnification = false            # Enlarge hovered dock icons to 1.15x
autohide = true                  # Automatically hides over maximized apps; touch bottom edge to reveal
pinned_apps = [                  # Executable names, full paths, or AUMIDs
    "explorer.exe",
    "msedge.exe"
]

[taskbar]
autohide_native = true           # Autohides native Windows taskbar while winshell runs
```

---

## File Locations

| Path | Purpose |
| :--- | :--- |
| `%APPDATA%\WinShellTheme\config.toml` | User configuration and pinned applications |
| `%APPDATA%\WinShellTheme\themes\*.toml` | Custom theme palettes and geometry definitions |
| `%APPDATA%\WinShellTheme\winshell.log` | Application log (auto-rotated at 1 MB) |
| `%APPDATA%\WinShellTheme\taskbar_state.toml` | Atomic native taskbar state backup (deleted on clean restore) |
| `%LOCALAPPDATA%\Programs\WinShellTheme\` | Default installation directory |

---

## Architecture & Codebase Map

| Module | Description |
| :--- | :--- |
| [`src/shell/mod.rs`](src/shell/mod.rs) | Shell application lifecycle, Win32 window message dispatch, and popup manager |
| [`src/shell/bar.rs`](src/shell/bar.rs) | Top bar layout, active window status pill, status pills rendering |
| [`src/shell/dock.rs`](src/shell/dock.rs) | Dock layout, app slots, running dots, mini media player, tooltips, right-click menus |
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
