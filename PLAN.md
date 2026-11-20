# PLAN.md: Windows 11 Desktop Shell Theme App in Rust

> **Revision 3 (2026-11-20).** Planning only, ready for M1 spike. Incorporates seven review notes:
> 1. Invisible AppBar reservation windows are click-through (`WS_EX_TRANSPARENT` / `HTTRANSPARENT`) so desktop margins are never blocked; spike verifies empty reservation holds space & receives `ABN_FULLSCREENAPP`.
> 2. `EVENT_OBJECT_LOCATIONCHANGE` hook is installed *only* when dock autohide is enabled, filtering to `OBJID_WINDOW` and the foreground window.
> 3. `SetTimer` explicitly calls `KillTimer` inside the handler before re-arming a fresh one-shot timer to eliminate repeating timer drift.
> 4. Pinning writes `config.toml` using `toml_edit` so formatting and comments survive; file watcher ignores self-written changes.
> 5. Icons cached at 64x64px, drawn downsampled at 44px, scaling to 1.15x cleanly without softness.
> 6. Shared D3D/D2D/DComp devices across all windows; acknowledges 10–20 MB driver overhead and treats M1 spike as empirical baseline for subsequent gates (50 MB ceiling remains).
> 7. Transient acrylic (`DWMSBT_TRANSIENTWINDOW`) selected over Mica so unactivated (`WS_EX_NOACTIVATE`) windows do not grey out.

## 0. Target Platform

* **Minimum OS:** Windows 11 22H2 (build 22621) or newer. Windows 10 is not supported. The blur (`DWMWA_SYSTEMBACKDROP_TYPE`) and corner (`DWMWA_WINDOW_CORNER_PREFERENCE`) attributes do not exist there.
* **Dev machine:** Windows 11 Home 26H2, build 26300.9974 (the registry `ProductName` says "Windows 10", a known quirk; the build number is authoritative).
* **Taskbar position is not assumed.** The dev machine's taskbar is on the **left** edge (per `StuckRects3`; no StartAllBack, ExplorerPatcher or Windhawk detected). The app reads the native taskbar edge at startup with `SHAppBarMessage(ABM_GETTASKBARPOS)` and lays out around it. With the taskbar on the left, the top bar and bottom dock do not collide with it.

---

## 1. Summary and Scope

A lightweight, user-mode desktop shell overlay for Windows 11 written in Rust. It replaces the visual role of the Windows taskbar with a top status bar, a floating bottom dock, and slide-down flyout panels, while leaving `explorer.exe` running, preserving user wallpaper controls, avoiding system file modifications, and requiring no administrative privileges.

### Hard Limits & Boundary Guarantees
* **User-mode execution:** Runs strictly under normal user privileges (UAC `asInvoker`). No drivers, services, or kernel-mode components.
* **Non-destructive overlay:** `explorer.exe` is neither terminated nor modified. The native taskbar is only switched to autohide (a normal user setting), never hidden by window manipulation. Desktop wallpaper stays managed by Windows settings.
* **No network activity:** Purely offline. No sockets, telemetry, background updates, or analytics.
* **Memory budget:** Realistic idle **25 to 40 MB Private Working Set**, hard ceiling **50 MB**. Binary size target < 6 MB. (See Section 8 for how this is measured.)
* **No web engines:** No Chromium, WebView2, Electron, or JavaScript runtimes.
* **No polling:** Every update is triggered by an OS event, a user action, or a one-shot timer. No periodic scans.

### Scope Boundaries

* **v1 Scope (Initial Delivery):**
  * **Top Bar:** Continuous translucent strip with 2 zones. Left: user handle, active window icon + truncated title. Right: Bluetooth, Wi-Fi, Core Audio volume/mute, battery percentage, date/time pill with minute-boundary update, notification bell, quick settings launcher. (The center virtual desktop zone is hidden in v1; see Section 15.)
  * **Bottom Dock:** Centered floating dock with a separate invisible AppBar reservation window, dynamic running app discovery, pinned apps, window grouping, active/focused window indicators, hover magnification and tooltip, middle-click launch, right-click context menu, and mini media player pill.
  * **Flyout Panels:** Sound (master volume, mic slider, default audio endpoint switching, SMTC media session cards with dominant album art tint), Battery, Wi-Fi & Bluetooth (status + deep-links to native `ms-settings:`), and Calendar grid.
  * **Native Taskbar Management:** Autohide via `SHAppBarMessage(ABM_SETSTATE, ABS_AUTOHIDE)`. The original state is read with `ABM_GETSTATE` at startup, saved to a state file, and restored exactly on clean exit, on panic, via the emergency hotkey (`Ctrl + Shift + Alt + Esc`), and on the next launch after a crash or kill.
  * **Rendering:** Direct2D 1.1 + DirectWrite drawing into DirectComposition visuals on `WS_EX_NOREDIRECTIONBITMAP` windows, with Windows 11 DWM backdrop blur (`DWMWA_SYSTEMBACKDROP_TYPE`). One model, used everywhere (see Section 6).
  * **Event-driven rendering:** 0% CPU at idle; no continuous render timers.
  * **TOML configuration and theme files:** In `%APPDATA%\WinShellTheme\` with live hot-reloading via `ReadDirectoryChangesW`.
* **Later (v1.x, gated on the M1 spike):**
  * Floating pills style (transparent bar with individually blurred pills). Only if the spike shows it is feasible at acceptable memory cost.
* **v2 Scope (Planned Subsequent Releases):**
  * In-flyout Wi-Fi network selection and connection list (WLAN API scan).
  * In-flyout Bluetooth device pairing list (Windows Runtime Bluetooth APIs).
  * Virtual desktop indicator and switching via private COM interfaces (`IVirtualDesktopManagerInternal`) with per-build fallbacks.
  * Native system tray notification icon mirror.
  * Custom Quick Settings and notification panels positioned under the top bar (replacing the v1 `Win + A` / `Win + N` stopgap).
  * Built-in Start menu launcher scanning `shell:AppsFolder`.
* **Not Doing (Explicitly Excluded):**
  * Killing or replacing `explorer.exe`, patching OS binaries, or calling `ShowWindow` / `SetWindowLong` on `Shell_TrayWnd`.
  * Kernel-mode drivers, Windows services, or elevation prompts (UAC / `requireAdministrator`).
  * DLL injection into external processes or global keyboard/mouse hooks (`WH_KEYBOARD_LL` / `WH_MOUSE_LL`).
  * Telemetry, network sockets, auto-updaters, or external analytics.
  * Embedded web engines.
  * A separate watchdog process.

---

## 2. Layout & Coordinate Specification (100% Scale Baseline)

> **DPI Note:** All pixel values represent 100% scale (96 DPI). At 125% (120 DPI) every dimension is multiplied by `1.25` with integer rounding for window rects and subpixel Direct2D layout inside them.

> **Corner radius note:** `DWMWA_WINDOW_CORNER_PREFERENCE` only offers the fixed Windows 11 radius (`DWMWCP_ROUND` about 8 px, `DWMWCP_ROUNDSMALL` about 4 px). It cannot produce 14 px or 22 px. Radii drawn *inside* a window by Direct2D (pills, hover boxes) can be any value. Radii of the *window outline itself* (where the blur ends) are limited to the DWM values unless the M1 spike proves an alternative. The values below marked "(spike)" depend on that result.

### ASCII Layout Architecture

```text
+-------------------------------------------------------------------------------------------------------+  Y = 0
|  8px margin                                                                                           |
|  +-------------------------------------------------------------------------------------------------+  |  Y = 8px
|  | @user | [Icon] Active Window Title...                  [BT] [WiFi] [Vol] [Batt 98%] [Fri 20 Nov] |  |  H = 36px
|  +-------------------------------------------------------------------------------------------------+  |  (continuous blurred strip)
|  <----------------- Left Zone ----------------->                  <-------------- Right Zone ------->|  |
+-------------------------------------------------------------------------------------------------------+  Y = 44px
|                                                                                                       |
|                                         DESKTOP WORKSPACE AREA                                        |
|                                (Maximized apps constrained by the two                                 |
|                                   invisible AppBar reservation windows)                               |
|                                                                                                       |
|                                 +---------------------------------------+                             |  Y = H - 74px
|                                 | [Start] [P1][P2] | [R1][R2] | [Media] |                             |  H = 64px
|                                 +---------------------------------------+                             |
|  10px margin                                                                                          |  Y = H - 10px
+-------------------------------------------------------------------------------------------------------+  Y = H
```

### Window Inventory

| Window | Visible? | Purpose |
| :--- | :--- | :--- |
| `TopReserve` | No (zero-paint, `WS_EX_TOOLWINDOW \| WS_EX_TRANSPARENT`, never activated, handles `WM_NCHITTEST` returning `HTTRANSPARENT`) | Registers the top AppBar and owns the full-width top edge rect (height 44 px). Completely click-through so margins around the floating bar do not swallow mouse clicks. |
| `BottomReserve` | No (`WS_EX_TOOLWINDOW \| WS_EX_TRANSPARENT`, handles `WM_NCHITTEST` returning `HTTRANSPARENT`) | Registers the bottom AppBar and owns the full-width bottom edge rect (height 74 px). Completely click-through. Absent when dock autohide is on. |
| `TopBar` | Yes | Continuous blurred strip, `X = 8, Y = 8, W - 16, 36`. Topmost, not an AppBar. |
| `Dock` | Yes | Narrow centered blurred window. Topmost, not an AppBar. Width changes as apps come and go; the reservation window does not. |
| `Flyout` | Yes, on demand | One reusable panel window. |
| `Tooltip` | Yes, on demand | Dock hover label. |

An AppBar always owns a full edge rectangle, so the visible windows never register as AppBars themselves. Making the reservation windows click-through (`WS_EX_TRANSPARENT` + `HTTRANSPARENT`) ensures user clicks in desktop margins pass directly through to desktop icons or underlying windows. `SPI_SETWORKAREA` is not used: it is global, is not restored by Windows if the app dies, and fights with Explorer.

### Detailed Component Dimensions

```text
TOP BAR (Continuous strip, Screen Width: W, Height: 36px, Margins: Top 8px, Left 8px, Right 8px)
=================================================================================================================
Window Dimensions:      X = 8, Y = 8, Width = W - 16, Height = 36
Window Outline Radius:  DWM DWMWCP_ROUND (~8px). 18px only if the spike proves it. (spike)
Pill Properties:        Height = 28px, Corner Radius = 14px (D2D-drawn), Internal Padding = 10px H, Gap = 6px
Left Zone:
  - User Pill:          H = 28px, Font = Segoe UI Variable 12px, Radius = 14px
  - Divider:            Width = 1px, Height = 16px, Margin = 6px H, Color = #FFFFFF @ 12%
  - Active App Pill:    H = 28px, Icon = 16x16px, Text = Segoe UI Variable 13px (Truncate @ ~40 chars + "…")
Center Zone:            Hidden in v1 (virtual desktop dots return in v2)
Right Zone (Left to Right):
  - Bluetooth Pill:     Width = 28px (Fluent Glyph: \u{E702})
  - Wi-Fi Pill:         Width = 28px (Fluent Glyph: \u{E701})
  - Volume Pill:        Width = 28px (Fluent Glyph: \u{E767})
  - Battery Pill:       Width ~54px (Fluent Glyph \u{E83F} + "98%")
  - Clock Pill:         Width ~155px ("Fri 20 Nov, 02:06 PM")
  - Notification Bell:  Width = 28px (Fluent Glyph: \u{EA8F})
  - Quick Settings:     Width = 28px (Fluent Glyph: \u{E713})

BOTTOM DOCK (Horizontally Centered, Screen Height: H, Dock Height: 64px)
=================================================================================================================
Window Dimensions:      X = (W - DockWidth) / 2, Y = H - 74, Width = Dynamic, Height = 64
Window Outline Radius:  DWM DWMWCP_ROUND (~8px). 22px only if the spike proves it. (spike)
Internal Padding:       Horizontal = 10px, Vertical = 8px
Border:                 1px stroke, #FFFFFF @ 12%
Icon Slot:              Width = 48px, Height = 48px, Corner Radius = 12px (D2D-drawn hover background)
Icon Surface:           44x44px target rendering size, Centered in slot. Source icon bitmaps are extracted and cached at 64x64px, drawn downsampled to 44px, and scaled to ~50.6px (1.15x) on hover so icons stay crisp and never look soft.
Separators:             Width = 1px, Height = 32px, Margin = 6px H, Color = #FFFFFF @ 12%
Active Indicator:       Y = Bottom of icon slot + 2px, Width = 4px, Height = 4px circle (#FFFFFF @ 60%)
Focused Indicator:      Width = 12px, Height = 4px pill (accent)
Multi-Window Badge:     Small dual-dot indicator (two 3px dots, gap 2px)
Mini Media Player Pill: Height = 48px, Album Art = 36x36px (Radius 6px), Title/Controls Width ~140px

FLYOUT PANEL (Right-Aligned under Top Bar, Width: 340px, Padding: 14px)
=================================================================================================================
Window Dimensions:      X = W - 348, Y = 52 (8px below Top Bar), Width = 340, Height = Dynamic
Window Outline Radius:  DWM DWMWCP_ROUND (~8px). 18px only if the spike proves it. (spike)
Animation:              Open: Slide Y from 44 to 52 (8px) + Alpha 0.0 to 1.0 in 140ms (Cubic Ease-Out),
                        done with DirectComposition transform/opacity animations (no CPU render loop)
Sound Panel Layout:
  - Master Volume:      Height = 32px (Icon 16px, Slider Bar 200px, Label 36px)
  - Microphone Slider:  Height = 32px (Icon 16px, Slider Bar 200px, Label 36px)
  - Mixer Shortcut:     Height = 24px (Hyperlink row right-aligned)
  - Device List:        Rows = 32px each (Icon, Friendly Name, Radio Indicator)
  - Media Session Card: Height = 72px (Art 52x52px, Title 13px bold, Artist 11px, Controls 3x 24px)
```

---

## 3. Theme Token Table

| Token Identifier | Dark Theme (Default) | Light Theme Variant | Description / Usage |
| :--- | :--- | :--- | :--- |
| `surface.glass.bg` | `#14161C` @ 72% (`0xB714161C`) | `#F5F6FA` @ 70% (`0xB2F5F6FA`) | Tint over the DWM blur; also the solid fallback when blur is unavailable |
| `surface.pill.bg` | `#1E212A` @ 60% (`0x991E212A`) | `#FFFFFF` @ 65% (`0xA6FFFFFF`) | Top bar pills & dock item background |
| `surface.pill.hover`| `#2A2E3A` @ 70% (`0xB22A2E3A`) | `#E8EBF2` @ 80% (`0xCCE8EBF2`) | Hovered pill/icon hit box |
| `surface.pill.press`| `#181A22` @ 80% (`0xCC181A22`) | `#DCE0EA` @ 85% (`0xD9DCE0EA`) | Active press / mousedown state |
| `border.subtle` | `#FFFFFF` @ 12% (`0x1FFFFFFF`) | `#000000` @ 10% (`0x1A000000`) | 1px outer borders and dividers |
| `text.primary` | `#F2F4F8` | `#14161C` | Main window titles, clock, sliders |
| `text.secondary` | `#A8AEBB` | `#5B6170` | User handle, sublabels, inactive status |
| `text.disabled` | `#6A7080` | `#9BA1B0` | Disconnected devices, empty media slots |
| `accent.default` | `#6EA8FE` | `#2563EB` | Focused app indicator, active slider fill |
| `accent.violet` | `#A78BFA` | `#7C3AED` | Selectable accent override |
| `accent.teal` | `#5EEAD4` | `#0D9488` | Selectable accent override |
| `accent.orange` | `#FB923C` | `#EA580C` | Selectable accent override |
| `accent.rose` | `#FB7185` | `#E11D48` | Selectable accent override |
| `status.success` | `#4ADE80` | `#16A34A` | Battery charging, connected networks |
| `status.warning` | `#FACC15` | `#CA8A04` | Battery < 20%, weak signal |
| `status.danger` | `#F87171` | `#DC2626` | Battery < 10%, muted mic |
| `shadow.ambient` | `#000000` @ 35%, Blur 24px, Y: 6px | `#000000` @ 12%, Blur 16px, Y: 4px | Drawn inside the window only; DWM supplies the outer window shadow |
| `radius.window` | `dwm_round` | `dwm_round` | Window outline. `dwm_round` / `dwm_small` / `none`. Pixel values only if the spike proves them |
| `radius.pill` | `14px` | `14px` | Status bar pills (D2D-drawn) |
| `radius.icon_slot` | `12px` | `12px` | Dock hover bounding box (D2D-drawn) |
| `font.family.ui` | `"Segoe UI Variable"` | `"Segoe UI Variable"` | Fallback: `"Segoe UI"` |
| `font.family.icon`| `"Segoe Fluent Icons"` | `"Segoe Fluent Icons"` | Fallback: Segoe MDL2 Assets |
| `font.size.bar` | `13px` | `13px` | Active window title |
| `font.size.pill` | `12px` | `12px` | Clock, battery %, status pills |
| `font.size.panel`| `14px` (Semibold) | `14px` (Semibold) | Flyout headers |
| `font.size.sub` | `11px` | `11px` | Artist names, small metadata |
| `motion.duration` | `140ms` | `140ms` | Range: 120ms to 180ms |
| `motion.easing` | `CubicEaseOut(0.0, 0.0, 0.2, 1.0)` | `CubicEaseOut` | Deceleration curve |

---

## 4. Behavior Specification

### Top Bar Interaction Matrix

1. **Left Zone (Active Window Pill):**
   * Left Click: Brings the active window to top, or minimizes it if already focused (`ShowWindow(hwnd, SW_MINIMIZE)`).
   * Right Click: Context menu with window controls (Minimize, Maximize, Snap Left, Snap Right, Close).
2. **Center Zone:** Hidden in v1. No virtual desktop dots, because keystroke simulation can switch desktops but cannot report how many exist or which is active, so the dots would show false information.
3. **Right Zone (System Status Pills):**
   * Left Click on Volume → Toggles Sound Flyout.
   * Left Click on Battery → Toggles Battery Flyout.
   * Left Click on Clock → Toggles Calendar Flyout.
   * Left Click on Wi-Fi / Bluetooth → v1: Opens `ms-settings:network-wifi` / `ms-settings:bluetooth`. (v2: inline list.)
   * Left Click on Notification Bell → v1 stopgap: `Win + N` via `SendInput`.
   * Left Click on Quick Settings → v1 stopgap: `Win + A` via `SendInput`.
   * **Stopgap caveat:** The native Quick Settings and notification flyouts anchor to the native taskbar's position (on the dev machine, the left edge), not to our top bar, so they will look out of place. Accepted for v1; replaced by custom panels in v2.

### Bottom Dock Interaction Matrix

1. **Start Button (Left Click):** Taps the Windows key (`VK_LWIN` down + up via `SendInput`), opening the native Start menu with its pins and search. No shell GUID is used.
2. **App Launch / Activation (Left Click):**
   * If Target App is **Not Running**: Launch via `ShellExecuteExW` with the stored executable path, or `shell:AppsFolder\<AUMID>` for packaged apps.
   * If Target App has **Single Window**:
     * If Not Focused: Bring to foreground (`SetForegroundWindow` with the lock workaround in Section 7).
     * If Focused: Minimize (`ShowWindow(SW_MINIMIZE)`).
   * If Target App has **Multiple Windows**: Show a window list flyout above the dock icon. Clicking an entry focuses that exact `HWND`.
3. **Middle Click:** Always launches a fresh instance.
4. **Right Click:** Context menu: `[New Window]`, `[Pin to Dock]` / `[Unpin from Dock]`, `[Close All Windows]`, `[Quit App]`.
5. **Hover State:**
   * Scale to 1.15x over 140ms (DirectComposition scale transform).
   * Capsule tooltip above the icon with the app's friendly name.
6. **Reservation & Autohide:**
   * **Default (Fixed Dock):** `BottomReserve` registers the bottom AppBar (`ABM_NEW` + `ABM_SETPOS`) for a full-width 74 px edge. Maximized windows stop 74 px above the screen bottom. The visible `Dock` sits inside that band.
   * **Autohide Mode (Configurable):** `BottomReserve` is not created. The dock slides off-screen (`Y = H + 2px`) when the foreground window is maximized or overlaps the dock. This is evaluated on `EVENT_SYSTEM_FOREGROUND`. Only when dock autohide is enabled in config, the app installs the `EVENT_OBJECT_LOCATIONCHANGE` hook, filtering strictly inside the callback to `idObject == OBJID_WINDOW` and the current foreground window (`hwnd == GetForegroundWindow()`). When autohide is off, this hook is never installed, preventing constant wakeups during window drags. A 2 px transparent hit window along the bottom edge brings the dock back on hover in 140ms.
   * **Fullscreen Bypass (event-triggered only):** The AppBar callback message delivers `ABN_FULLSCREENAPP` when a fullscreen app opens or closes on that monitor. As a backup, `SHQueryUserNotificationState` is checked once on each `EVENT_SYSTEM_FOREGROUND` and once on resume from sleep (`PBT_APMRESUMEAUTOMATIC`). If it reports `QUNS_RUNNING_D3D_FULL_SCREEN` or `QUNS_PRESENTATION_MODE`, the top bar and dock are hidden (`ShowWindow(SW_HIDE)`) and shown again when the state clears. There is no background worker loop.

---

## 5. UI Technology Decision: Native Win32 + Direct2D vs Alternatives

### Evaluation Matrix

> The numbers below are rough estimates from general experience with each stack, not measurements of this app. They are useful for relative comparison only. Real targets are in Section 8.

| Metric / Dimension | Win32 + Direct2D / DWrite + DirectComposition | Slint (Software / Skia) | egui (wgpu / glow) | Skia-safe (DirectX / Vulkan) |
| :--- | :--- | :--- | :--- | :--- |
| **Idle RAM (relative)** | **Lowest** | Low to moderate | Moderate to high | High |
| **Binary Size (Stripped)** | **~2.5 to 4 MB** | ~7 to 12 MB | ~10 to 16 MB | ~14 to 24 MB |
| **Idle CPU Usage** | **0%** (`GetMessageW` blocks) | ~0% | Polling risk | ~0% |
| **Backdrop Blur Integration**| **Native** (`DwmSetWindowAttribute`)| Emulated / External HWND | Hard | Emulated via shader |
| **Text Quality** | **Native DirectWrite** | Good | Fair (atlas) | Excellent |
| **Implementation Effort** | Moderate to High | Low to Moderate | Low | Moderate |

### Why the native stack wins on memory

* DirectWrite uses the system font cache service, so the app holds no private glyph atlas.
* `d2d1.dll`, `dwrite.dll`, `dcomp.dll` and `user32.dll` are shared with the rest of the OS; their code pages are mostly shared, not private.
* DirectComposition surfaces are small: a 1920 x 36 strip at 32 bpp is about 276 KB per buffer.
* Immediate-mode engines (egui) and Skia keep CPU-side vertex caches and glyph atlases alive.

### Final Recommendation

**Selected Architecture: Native Win32 + Direct2D 1.1 + DirectWrite + DirectComposition using Microsoft's official `windows` crate.**

It is the stack most likely to stay inside the 50 MB ceiling, gives per-pixel alpha, and integrates with the Windows 11 DWM backdrop without a render loop or custom blur shaders. Whether the DWM backdrop combines correctly with a DirectComposition `WS_EX_NOREDIRECTIONBITMAP` window is the first thing the M1 spike verifies.

---

## 6. Architecture & System Design

### Rendering Model (single model, used by every visible window)

1. Create the window with `WS_EX_NOREDIRECTIONBITMAP | WS_EX_TOOLWINDOW | WS_EX_TOPMOST` (plus `WS_EX_NOACTIVATE` for bar and dock).
2. Apply `DWMWA_SYSTEMBACKDROP_TYPE` with `DWMSBT_TRANSIENTWINDOW` (transient acrylic) and `DWMWA_WINDOW_CORNER_PREFERENCE`; extend the frame with `DwmExtendFrameIntoClientArea` as the spike requires.
   * *Why transient acrylic over Mica:* The top bar and dock use `WS_EX_NOACTIVATE` and never take keyboard focus. Mica (`DWMSBT_MAINWINDOW`) dims and turns into an opaque grey wash whenever a window is inactive. Transient acrylic maintains its translucent blur regardless of window activation state.
3. **Device Sharing:** Exactly one `ID3D11Device`, one `ID2D1Factory1`, and one `IDCompositionDevice` are shared process-wide across all windows. Never create per-window D3D/D2D devices (GPU driver contexts alone cost 10–20 MB private memory). One target + root visual per window.
4. Content is drawn with Direct2D into `IDCompositionSurface`s (`BeginDraw` / `EndDraw` on a dirty rect only), then `Commit()`.
5. Hover scale, flyout slide and fades are DirectComposition animations, so the compositor runs them without waking our thread each frame.

No `WS_EX_LAYERED`, no `UpdateLayeredWindow`, no `WM_PAINT`-driven `ID2D1HwndRenderTarget`. `WM_PAINT` is validated and ignored.

**Device loss:** On `D2DERR_RECREATE_TARGET` or DXGI device removal, rebuild the shared D3D/D2D/DComp devices and redraw all surfaces once.

### Thread Model

```text
+------------------------------------------------------------------------------------------------+
| MAIN PROCESS                                                                                   |
|                                                                                                |
|  +------------------------------------------------------------------------------------------+  |
|  | THREAD 1: UI & Message Pump (STA)                                                        |  |
|  |  - Win32 message loop (GetMessageW / DispatchMessageW), blocks when idle                 |  |
|  |  - Direct2D + DirectComposition rendering                                                |  |
|  |  - WinEventHook callbacks (out-of-context: foreground, create/destroy, name change)       |  |
|  |  - HWNDs: TopReserve, BottomReserve, TopBar, Dock, Flyout, Tooltip                       |  |
|  |  - Receives worker results via std::sync::mpsc + PostMessageW(WM_SHELL_EVENT)            |  |
|  +------------------------------------------------------------------------------------------+  |
|                                     ^                                    ^                     |
|                                     | std::sync::mpsc                    | std::sync::mpsc     |
|  +----------------------------------------------------+  +-----------------------------------+ |
|  | THREAD 2: System Monitor (MTA)                     |  | THREAD 3: Shell Asset Loader (STA)| |
|  |  - Core Audio callbacks (IMMNotificationClient,     |  |  - Win32 icons: SHGetFileInfo /   | |
|  |    IAudioEndpointVolumeCallback)                    |  |    IShellItemImageFactory         | |
|  |  - WinRT SMTC session events                        |  |  - Packaged app icons:            | |
|  |  - Wi-Fi / Bluetooth change notifications           |  |    IShellItemImageFactory on      | |
|  |  - All callbacks, no loops                          |  |    shell:AppsFolder\<AUMID>       | |
|  |                                                     |  |  - Album art decode via WIC       | |
|  |                                                     |  |  - Icon disk cache read / write   | |
|  +----------------------------------------------------+  +-----------------------------------+ |
+------------------------------------------------------------------------------------------------+
```

Power events (`RegisterPowerSettingNotification`, `WM_POWERBROADCAST`) arrive as window messages on the UI thread, so they need no worker. Each worker thread blocks on its COM callbacks or on its request channel; none of them wake on a timer.

### Event Flow Pipeline

```text
[Windows OS Events]
   ├── WinEventHook (EVENT_SYSTEM_FOREGROUND, EVENT_OBJECT_CREATE/DESTROY/SHOW/HIDE, EVENT_OBJECT_NAMECHANGE)
   ├── AppBar callback (ABN_FULLSCREENAPP, ABN_POSCHANGED, ABN_STATECHANGE)
   ├── IMMNotificationClient (default device change) / IAudioEndpointVolumeCallback (volume, mute)
   ├── SMTC session events (playback state, track change)
   ├── WM_POWERBROADCAST (battery, resume), WM_DISPLAYCHANGE, WM_DPICHANGED, WM_SETTINGCHANGE, WM_TIMECHANGE
   └── ReadDirectoryChangesW (config / theme files)
         │
         ▼
[Worker threads] serialize payload into ShellEvent enum → mpsc::Sender::send → PostMessageW(WM_SHELL_EVENT)
         │
         ▼
[UI Thread]
   ├── Drain the mpsc receiver
   ├── Update in-memory state (WindowList, AudioState, MediaTrack, ...)
   ├── Re-layout, mark dirty rects
   └── Redraw dirty surfaces (BeginDraw/EndDraw) → IDCompositionDevice::Commit → back to GetMessageW
```

### Full Window Rescan (event-triggered only)

The live window list is kept current by WinEvent hooks. A full `EnumWindows` rescan runs only on: startup, resume from sleep (`PBT_APMRESUMEAUTOMATIC`), `WM_DISPLAYCHANGE`, and Explorer restart (`TaskbarCreated` registered message). There is no periodic rescan.

### Core In-Memory Data Structures

```rust
// Window & Running Application Representation
pub struct WindowItem {
    pub hwnd: isize,
    pub title: String,
    pub exe_path: PathBuf,
    pub aumid: Option<String>,
    pub process_id: u32,
    pub icon_cache_key: u64,
    pub is_cloaked: bool,
    pub is_minimized: bool,
    pub is_elevated: bool,
    pub monitor_handle: isize,
}

pub struct DockAppGroup {
    pub id: String, // AUMID or lowercased exe path
    pub display_name: String,
    pub icon_cache_key: u64,
    pub is_pinned: bool,
    pub windows: Vec<WindowItem>,
    pub active_window_idx: usize,
}

// Icon Cache (bounded LRU)
pub struct IconEntry {
    pub d2d_bitmap: Option<ID2D1Bitmap>, // created and used on the UI thread only
    pub raw_bgra: Vec<u8>,               // dropped once d2d_bitmap exists
    pub width: u32,
    pub height: u32,
    pub last_accessed: u64,
}

pub struct IconCache {
    pub entries: HashMap<u64, IconEntry>,
    pub capacity_bytes: usize, // hard ceiling: 8 MB
    pub current_bytes: usize,
}
```

---

## 7. Windows Integration & Exact API Specification

| Feature Area | Windows API Function / Interface | Documented? | Fragility | Fallback Strategy |
| :--- | :--- | :--- | :--- | :--- |
| **Native taskbar position** | `SHAppBarMessage(ABM_GETTASKBARPOS)` | Yes | Very Low | Assume bottom if the call fails. Re-read on `ABN_POSCHANGED` / `TaskbarCreated`. |
| **Reserving screen space** | Invisible `TopReserve` / `BottomReserve` windows with `SHAppBarMessage(ABM_NEW, ABM_QUERYPOS, ABM_SETPOS)`; `ABM_REMOVE` on exit. Windows are click-through (`WS_EX_TRANSPARENT` + `HTTRANSPARENT`) so desktop margins are never blocked. | Yes | Low | If reservation fails, run without it (maximized windows may overlap the bar); log it. `SPI_SETWORKAREA` is not used. |
| **Native taskbar autohide** | `ABM_GETSTATE` at startup → save to state file → `ABM_SETSTATE(ABS_AUTOHIDE)`; restore the saved value on exit | Yes | Low | If Windows refuses autohide, leave the taskbar as it is and continue. Never hide `Shell_TrayWnd` by window manipulation. |
| **Window event tracking** | `SetWinEventHook(..., WINEVENT_OUTOFCONTEXT)`. `EVENT_OBJECT_LOCATIONCHANGE` is installed **only** when dock autohide is enabled, filtering to `OBJID_WINDOW` and foreground window. | Yes | Very Low | Full `EnumWindows` rescan on resume, display change, or Explorer restart only. |
| **Initial window scan** | `EnumWindows` + `IsWindowVisible` + `GetWindowLongPtrW(GWL_EXSTYLE)` + owner check | Yes | Very Low | Skip `WS_EX_TOOLWINDOW` windows and unowned windows with empty titles. |
| **Packaged app detection** | `GetApplicationUserModelId` + `GetPackageFullName`; `SHGetPropertyStoreForWindow(PKEY_AppUserModel_ID)` | Yes | Low | For `ApplicationFrameHost.exe`, use `EnumChildWindows` to find the hosted app's `HWND`. |
| **App icons (Win32)** | `IShellItemImageFactory::GetImage` on the exe path (`SIIGBF_ICONONLY`), requested at 64x64px | Yes | Very Low | `SHGetFileInfoW(SHGFI_ICON)` then `ExtractIconExW`. |
| **App icons (packaged)** | `SHCreateItemFromParsingName("shell:AppsFolder\<AUMID>")` → `IShellItemImageFactory::GetImage` at 64x64px | Yes | Low | Generic app glyph. `ExtractIconEx` is not used for packaged apps (it returns the host's icon). |
| **Cloaked window detection**| `DwmGetWindowAttribute(DWMWA_CLOAKED)` | Yes | Very Low | Filters suspended UWP apps and windows on other virtual desktops. |
| **Window activation** | `SetForegroundWindow`; if refused, `AttachThreadInput` to the foreground thread, retry, detach | Yes | Moderate | For elevated targets (UIPI), show a shield badge and a short message instead of failing silently. |
| **Window minimization**| `ShowWindow(SW_MINIMIZE)` or `PostMessageW(WM_SYSCOMMAND, SC_MINIMIZE)`| Yes | Very Low | None needed. |
| **Start button** | `SendInput` with `VK_LWIN` key down + key up | Yes | Low | None. No shell GUID. |
| **Quick Settings / Notifications (v1 stopgap)** | `SendInput` `Win + A` / `Win + N` | Yes | Low | Native flyouts appear at the native taskbar edge; replaced in v2. |
| **Time & locale** | `GetLocalTime` / `GetDateFormatEx` / `GetTimeFormatEx`; `WM_TIMECHANGE` | Yes | Very Low | One-shot timer. Call `KillTimer` immediately inside the handler before re-arming a fresh timer (or use `CreateThreadpoolTimer`), recomputed as ms to the next minute boundary. |
| **Battery & power** | `GetSystemPowerStatus` + `RegisterPowerSettingNotification(GUID_BATTERY_PERCENTAGE_REMAINING, GUID_ACDC_POWER_SOURCE)` | Yes | Very Low | None needed. |
| **Wi-Fi status** | `INetworkListManager` events (`INetworkListManagerEvents`) + `WlanRegisterNotification` | Yes | Low | `NotifyIpInterfaceChange` + `GetAdaptersAddresses` if the WLAN service is stopped (Ethernet-only). |
| **Bluetooth status** | WinRT `Windows.Devices.Radios.Radio.GetRadiosAsync()` + `StateChanged` event | Yes | Low | Hide the Bluetooth pill if no radio is found. |
| **Audio volume & mute**| `IMMDeviceEnumerator` → `IAudioEndpointVolume` + `IAudioEndpointVolumeCallback` | Yes | Very Low | Standard Core Audio. |
| **Audio endpoint switch**| Undocumented `IPolicyConfig::SetDefaultEndpoint` | **No** | **High** | Check every `HRESULT`; on failure or missing interface open `ms-settings:sound`. Never crash. |
| **Now playing media** | WinRT `GlobalSystemMediaTransportControlsSessionManager` | Yes | Low | No session → hide the mini player pill. |
| **Album art decode** | `IRandomAccessStream` → `CreateStreamOverRandomAccessStream` → WIC `IWICBitmapDecoder` → `IWICFormatConverter` (32bpp PBGRA) → `ID2D1DeviceContext::CreateBitmapFromWicBitmap` | Yes | Very Low | Placeholder art. Dominant tint computed from a downscaled WIC copy. |
| **DWM backdrop blur** | `DwmSetWindowAttribute(DWMWA_SYSTEMBACKDROP_TYPE, DWMSBT_TRANSIENTWINDOW)` (transient acrylic) | Yes (Win 11 22H2+) | Low | Solid semi-transparent fill with `surface.glass.bg`. (Mica not used as unactivated windows turn grey). |
| **Window rounded corners**| `DwmSetWindowAttribute(DWMWA_WINDOW_CORNER_PREFERENCE, DWMWCP_ROUND)` | Yes (Win 11) | Very Low | Fixed radius only (~8 px). Custom radii depend on the M1 spike. |
| **Virtual desktops** | None in v1 | n/a | n/a | Dots hidden in v1. Private COM interface in v2. |
| **Fullscreen detection**| AppBar `ABN_FULLSCREENAPP`; `SHQueryUserNotificationState` on foreground change and resume only | Yes | Low | Geometry check (foreground window rect covers the monitor and has no caption) at the same trigger points. |
| **Emergency hotkey**| `RegisterHotKey(hwnd, ID, MOD_CONTROL \| MOD_ALT \| MOD_SHIFT \| MOD_NOREPEAT, VK_ESCAPE)` | Yes | Very Low | If registration fails (combination taken), log it, show a one-time notice, and fall back to `Ctrl + Shift + Alt + F12`. |

---

## 8. Performance, Memory & CPU Budget

### How memory is measured

* **Metric:** **Private Working Set** (Task Manager → Details tab → add the "Memory (private working set)" column), or `PROCESS_MEMORY_COUNTERS_EX2::PrivateWorkingSetSize`.
* **Not counted as a saving:** Working set trimming (`EmptyWorkingSet` / `SetProcessWorkingSetSize(-1, -1)`). It only lowers the displayed number; the pages come back on the next use. The app does not trim its working set, and no gate may be met by trimming.
* **Why the targets are higher than revision 1:** WinRT (SMTC, radios), Direct2D, DirectWrite, DirectComposition and four to six windows each add private memory. Furthermore, D3D11 device creation alone can allocate 10 to 20 MB of private working set inside the GPU display driver. Because of this initial driver overhead, the 25 MB gate for M1 may be tight; we will treat the M1 spike's measured Private WS as our empirical baseline and adjust subsequent milestone gates from it, while maintaining the strict 50 MB hard ceiling.

### Memory & CPU Scenario Budgets

| Execution Scenario | Target Private WS | Hard Ceiling | CPU (Target) | CPU (Max) |
| :--- | :--- | :--- | :--- | :--- |
| **Full Idle (static screen)** | **25 to 35 MB** (calibrated by M1 spike) | **50 MB** | **0%** | **0%** |
| **Clock minute tick** | same as idle | 50 MB | one frame | one frame |
| **Dock icon hover animation** | +0 to 3 MB | 50 MB | < 1% (compositor-driven) | 2% |
| **Flyout open (audio/media)** | 30 to 40 MB | 50 MB | < 2% during the 140ms open | 3% |
| **Heavy window switching** | 30 to 40 MB | 50 MB | < 1% | 2% |

### Timer Inventory (strict event-driven policy)

There are **no repeating timers** in this application. The complete inventory:

1. **Clock boundary timer:** One-shot `SetTimer` (or `CreateThreadpoolTimer`). `SetTimer` repeats by default in Win32; therefore, the handler explicitly calls `KillTimer` immediately on wake, updates the clock, and arms a fresh one-shot timer for the newly computed ms to the next minute boundary. Recomputed on `WM_TIMECHANGE` and resume from sleep.
2. **Autohide hover delay:** One-shot timer (immediately cancelled with `KillTimer` once fired), short delay before revealing an autohidden dock (avoids flicker). Armed only on hover.
3. **Animations:** Driven by DirectComposition animation objects, so no app-side frame timer is needed. If a case needs CPU-driven frames, the ticker runs only for that animation's 140ms and stops.

There is no working set trimmer, no fullscreen poller, and no periodic window rescan.

### Release Profile & Compiler Optimizations (`Cargo.toml`)

```toml
[profile.release]
opt-level = "z"         # Optimize for smallest binary size
lto = "fat"             # Cross-crate whole-program optimization
codegen-units = 1       # Maximum dead-code elimination
panic = "abort"         # No unwinding code (the panic hook still runs before abort)
strip = true            # Strip debug symbols
incremental = false
overflow-checks = false
```

* **Static CRT linking** via `.cargo/config.toml`:
  ```toml
  [target.x86_64-pc-windows-msvc]
  rustflags = ["-C", "target-feature=+crt-static"]
  ```
  Removes the dependency on `VCRUNTIME140.dll` so the single `.exe` runs on clean installs. Adds roughly 250 KB.

---

## 9. Third-Party Crate Justification & Auditing Plan

No network crates and no UI frameworks. Crate versions are **not hardcoded in this plan**: add each with `cargo add` at implementation time and let `Cargo.lock` pin the exact versions.

| Crate Name | Kind | Primary Purpose / Justification | License |
| :--- | :--- | :--- | :--- |
| `windows` | dependency | Official Microsoft bindings for Win32, D2D, DWrite, DComp, WIC, Core Audio and WinRT, feature-gated to the exact APIs used. | MIT / Apache 2.0 |
| `serde` | dependency | Deserialization for config and theme files (feature `derive`). | MIT / Apache 2.0 |
| `toml` | dependency | Fast parser for config and theme files. | MIT / Apache 2.0 |
| `toml_edit` | dependency | Format-preserving TOML editor used when pinning/unpinning apps so user comments survive. | MIT / Apache 2.0 |
| `winres` (or `embed-resource`) | build-dependency | Embeds the manifest, version info and icon into the `.exe`. | MIT |

Removed from revision 1:
* **`image`:** album art is decoded with WIC through the `windows` crate (no extra decoder, smaller binary).
* **`crossbeam-channel`:** `std::sync::mpsc` is enough for a few low-rate producers and one consumer.
* **`windows-core`:** pulled in transitively by `windows`; not listed separately.

### Dependency Security Auditing

1. **Supply chain lock:** `Cargo.lock` is committed and pins exact versions.
2. **Automated audit:** `cargo audit` before each release to catch RUSTSEC advisories.
3. **`cargo deny` policy (`deny.toml`):**
   * Licenses: allow only MIT, Apache-2.0, BSD-3-Clause, Unicode-3.0 (or whatever the `windows` family requires).
   * Duplicate versions: **warn**, not deny. The `windows-*` crate family often brings in more than one version of `windows-sys` / `windows-targets`.
   * Bans: deny network crates (`reqwest`, `hyper`, `tokio`, `socket2`, `ureq`).

---

## 10. Packaging, Distribution & Lifecycle Management

### Build & Compilation Prerequisites

* Target: `x86_64-pc-windows-msvc` (optional `aarch64-pc-windows-msvc`).
* Toolchain: current stable Rust with MSVC build tools (Visual Studio Build Tools 2022 + Windows 11 SDK 10.0.22621.0 or newer).
* Build: `cargo build --release --target x86_64-pc-windows-msvc`
* Output: `target/x86_64-pc-windows-msvc/release/winshell.exe` (expected 2.5 to 4 MB).

### Manifest & Windows Resources

Embedded at build time:
1. **DPI awareness:** `PerMonitorV2, PerMonitor`.
2. **OS compatibility:** Windows 10/11 `supportedOS` GUID (the same GUID covers both).
3. **Execution level:** `<requestedExecutionLevel level="asInvoker" uiAccess="false" />`.
4. **UTF-8 code page:** `<activeCodePage>UTF-8</activeCodePage>`.

### Single Instance & Crash Recovery (no watchdog)

```rust
// 1. Single instance via a per-user named mutex (Local\ = per session, no username in the name)
let handle = CreateMutexW(None, true, w!("Local\\WinShellTheme.SingleInstance"))?;
if GetLastError() == ERROR_ALREADY_EXISTS {
    // Ask the running instance to show/restore itself, then exit
    let existing = FindWindowW(w!("WinShellTheme_TopBarClass"), None);
    if let Ok(hwnd) = existing { let _ = PostMessageW(hwnd, WM_RESTORE_SHELL, WPARAM(0), LPARAM(0)); }
    return Ok(());
}

// 2. Startup repair: a leftover state file means the last run did not exit cleanly
if let Some(saved) = taskbar_state::load()? {   // %APPDATA%\WinShellTheme\taskbar_state.toml
    taskbar::restore(saved);                     // put back exactly what the user had
}
let original = taskbar::get_state();             // ABM_GETSTATE
taskbar_state::save(original)?;                  // write BEFORE changing anything

// 3. Panic hook (runs even with panic = "abort")
std::panic::set_hook(Box::new(|info| {
    taskbar::restore_from_file();                // restore exactly the saved state
    log_panic_to_disk(info);                     // %APPDATA%\WinShellTheme\crash.log
}));
```

(Illustrative only; exact signatures follow whichever `windows` crate version Cargo resolves.)

**Why no watchdog exe:** Autohide is an ordinary taskbar setting. If the app is killed from Task Manager, the taskbar stays in autohide mode but is still reachable by moving the pointer to its edge (on the dev machine, the left edge). The next launch sees the leftover state file and repairs it. The user can also restore it from Settings → Personalization → Taskbar.

### Taskbar State File

```toml
# %APPDATA%\WinShellTheme\taskbar_state.toml
# Written at startup BEFORE any change; deleted after a successful restore on clean exit.
original_abm_state = 2     # raw ABM_GETSTATE value (ABS_AUTOHIDE = 1, ABS_ALWAYSONTOP = 2)
saved_at = "2026-11-20T14:44:46+05:30"
```

* If the user already had autohide on, the saved value already includes `ABS_AUTOHIDE` and restore keeps it on. Exit never forces `ABS_ALWAYSONTOP`.
* If a state file already exists at startup, it is restored first and *then* re-read, so a crash loop can never save "autohide" as the user's original value.

### Windows Startup Integration

* **HKCU Run key** (`HKCU\Software\Microsoft\Windows\CurrentVersion\Run`) with `"C:\Path\To\winshell.exe" --autostart`. Standard, no elevation, and toggleable from Task Manager's Startup Apps tab.

### Clean Exit Flow

1. Unhook all WinEvent hooks (`UnhookWinEvent`).
2. Unregister both AppBars (`SHAppBarMessage(ABM_REMOVE)` on `TopReserve` and `BottomReserve`).
3. Restore the native taskbar to the value in `taskbar_state.toml` (`ABM_SETSTATE`), then delete the file.
4. Unregister the hotkey.
5. Release the single-instance mutex.

Uninstall additionally deletes the `HKCU\...\Run\WinShellTheme` value if present and offers to delete `%APPDATA%\WinShellTheme\`.

The emergency hotkey runs steps 1 to 5 and exits.

---

## 11. Configuration & Theming System

### Directory Hierarchy

```text
%APPDATA%\WinShellTheme\
  ├── config.toml           # General behavior, positioning, pinned applications
  ├── taskbar_state.toml    # Original native taskbar state (exists only while running or after a crash)
  ├── crash.log             # Panic log
  ├── themes\
  │    ├── dark_glass.toml
  │    └── light_clean.toml
  └── cache\
       └── icons\           # Persistent icon bitmap cache (max 10 MB)
```

> **Config Write Integrity & Hot-Reload:** Pinning and unpinning apps modifies `config.toml` using `toml_edit` rather than re-serializing the entire struct. This preserves user comments, key ordering, and custom formatting. When the app writes to `config.toml`, it records the write timestamp or sets an atomic `ignoring_self_write` flag so the `ReadDirectoryChangesW` file watcher ignores the notification and does not trigger a redundant self-reload.

### Proposed `config.toml` Schema

```toml
[general]
user_handle = "@yourname"
theme = "dark_glass"             # "dark_glass", "light_clean", or "auto_system"
follow_windows_theme = true
extract_wallpaper_accent = false

[top_bar]
enabled = true
style = "solid_strip"            # "solid_strip" (v1 default). "floating_pills" is later, if the M1 spike allows it
height = 36
margin_top = 8
margin_horizontal = 8
clock_format = "%a %d %b, %I:%M %p"  # "Fri 20 Nov, 02:06 PM"
enabled_pills = ["bluetooth", "wifi", "volume", "battery", "clock", "notifications", "quick_settings"]

[dock]
enabled = true
height = 64
margin_bottom = 10
autohide = false
magnification = false
pinned_apps = [
    "C:\\Program Files\\Microsoft Visual Studio\\2022\\Community\\Common7\\IDE\\devenv.exe",
    "Microsoft.WindowsTerminal_8wekyb3d8bbwe!App",
    "C:\\Program Files\\Google\\Chrome\\Application\\chrome.exe",
    "explorer.exe"
]

[taskbar]
autohide_native = true           # switch the native taskbar to autohide while running

[hotkeys]
emergency_restore = "Ctrl+Shift+Alt+Esc"   # falls back to Ctrl+Shift+Alt+F12 if taken

[monitors]
bars_on_all_monitors = true
dock_primary_only = true
```

### Proposed `themes/dark_glass.toml` Schema

```toml
[theme.info]
name = "Dark Glass"
author = "WinShellTheme"
version = "1.0.0"

[colors]
glass_bg = "#14161CB7"          # 72% opacity tint over the blur
pill_surface = "#1E212A99"
hover_surface = "#2A2E3AB2"
press_surface = "#181A22CC"
border = "#FFFFFF1F"
text_primary = "#F2F4F8"
text_secondary = "#A8AEBB"
text_disabled = "#6A7080"
accent = "#6EA8FE"

[geometry]
window_corners = "dwm_round"     # "dwm_round" | "dwm_small" | "none"; pixel radii only if the spike proves them
pill_corner_radius = 14
icon_slot_radius = 12

[motion]
duration_ms = 140
easing = "cubic_ease_out"
```

---

## 12. Safety, Trust & Antivirus Mitigation

1. **No network sockets:** Nothing opens a socket. `cargo deny` bans network crates.
2. **No global hooks:** Only `SetWinEventHook` with `WINEVENT_OUTOFCONTEXT` (events are delivered to our own thread; no code runs in other processes) and `RegisterHotKey`. Never `SetWindowsHookExW` with `WH_KEYBOARD_LL` / `WH_MOUSE_LL`.
3. **No code injection:** Read-only APIs only (`GetWindowThreadProcessId`, `OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION)`, shell icon APIs). No `CreateRemoteThread` or `VirtualAllocEx`.
4. **Privacy:** Window titles are read only for the active window pill and dock grouping. No child controls, document text, or browser address bars are inspected.
5. **Taskbar safety:** The only change to the native taskbar is the documented autohide setting, saved first and restored exactly.
6. **SmartScreen & unsigned executables:** Locally built binaries have no `Zone.Identifier` stream and run without a warning. Downloaded copies can be unblocked with `Unblock-File .\winshell.exe`, or signed with a self-signed certificate in `Cert:\CurrentUser\Root`.

---

## 13. Phased Implementation Milestones

Every milestone is a runnable checkpoint with a visible result and a memory gate measured as Private Working Set (Section 8). **The native taskbar is not touched until M5**, after the dock can launch and switch apps, so a broken dev build never leaves you without a working taskbar. The emergency hotkey exists from M1.

```text
+-----------------------------------------------------------------------------------------------+
| MILESTONES ROADMAP                                                                            |
|                                                                                               |
| [M1: Spike + Windows + Hotkey] ──> [M2: Clock & Active Window] ──> [M3: Dock & Tracking]      |
|    - Blur & corner proof             - Live titles, minute clock      - Running app discovery |
|    - Reservation windows             - ≤ 30 MB                        - ≤ 32 MB               |
|                                                                                               |
| [M4: Pinning & Launcher]      ──> [M5: Native Taskbar Autohide] ──> [M6: Status Pills]        |
|    - Launch, switch, group           - State file + repair            - Volume, battery, WiFi |
|    - ≤ 34 MB                         - ≤ 34 MB                        - ≤ 36 MB               |
|                                                                                               |
| [M7: Audio & Media Panels]    ──> [M8: Themes & Live Reload]  ──> [M9: Polish & Packaging]    |
|    - Core Audio & SMTC cards         - TOML theme swapping            - < 4 MB exe            |
|    - ≤ 40 MB open                    - ≤ 38 MB                        - idle 25–35 MB         |
+-----------------------------------------------------------------------------------------------+
```

### Detailed Milestone Breakdown

* **Milestone 1: Blur & Corner Spike, Window Scaffolding, Reservation Windows, Recovery Hotkey**
  * *Step 1 — Spike (do this before anything else):* Build throwaway test windows on the dev machine using the Section 6 rendering model (`WS_EX_NOREDIRECTIONBITMAP` + DirectComposition + `DWMWA_SYSTEMBACKDROP_TYPE` with `DWMSBT_TRANSIENTWINDOW`):
    1. A full-width 36 px strip. Confirm the blur renders, stays behind the D2D content, survives focus changes (confirms transient acrylic does not grey out on inactive windows), and that the tint alpha looks right.
    2. A narrow 64 px dock-sized window. Confirm the corner radius actually achieved with `DWMWCP_ROUND` and whether any documented method gives 18–22 px with blur clipped to it.
    3. An invisible click-through AppBar reservation window (`WS_EX_TRANSPARENT` + `HTTRANSPARENT`). Confirm that an empty reservation window reliably holds desktop edge space against maximized windows and correctly receives `ABN_FULLSCREENAPP` notifications.
    4. Optional: a pill-sized window group, to measure the memory cost of the floating pills idea.
  * *Spike output:* A short note added to this plan recording what worked, the achieved radius, whether empty click-through AppBars hold space and receive fullscreen events, whether blur survives focus changes, and the Private WS of each test. This decides `window_corners` defaults and whether floating pills move into v1.x.
  * *Step 2 — Scaffolding:* `TopReserve` and `BottomReserve` invisible click-through AppBar windows, plus visible `TopBar` and `Dock` windows with blur. Maximized windows stop cleanly at the reserved edges. Layout respects the native taskbar's edge (left on the dev machine).
  * *Step 3 — Recovery hotkey:* `Ctrl + Shift + Alt + Esc` registered first and verified free on the dev machine (log a clear message and use the fallback if `RegisterHotKey` fails). In M1 it removes the AppBars and exits; from M5 it also restores the taskbar. Building the hotkey first guarantees any test window or stuck AppBar is immediately escapable.
  * *Memory Gate:* Measure the empirical baseline in the M1 spike (acknowledging the 10–20 MB initial GPU display driver private working set for D3D device creation). Subsequent milestone gates will calibrate from this baseline while maintaining the 50 MB hard ceiling. CPU = 0%.
* **Milestone 2: System Clock & Active Window Event Hook**
  * *Deliverable:* Left zone shows the user handle and live active window title/icon. Clock updates exactly on the minute boundary using the recomputed one-shot timer. Correct after sleep/resume and manual time changes.
  * *Memory Gate:* Idle Private WS ≤ 30 MB, CPU = 0%.
* **Milestone 3: Dock Window Tracking & Live App Enumeration**
  * *Deliverable:* Running application windows detected via WinEvent hooks (rescan only on resume / display change / Explorer restart). Win32 and packaged app icons (via `IShellItemImageFactory`), DPI-scaled, with active indicator dots.
  * *Memory Gate:* Idle Private WS ≤ 32 MB, CPU = 0%.
* **Milestone 4: App Pinning, Launching & Window Grouping**
  * *Deliverable:* Clicking dock icons focuses/minimizes apps or launches pinned targets. Start button taps the Win key. Multi-window list flyout works. Right-click context menu works. The dock is now a usable replacement for the taskbar's app switching.
  * *Memory Gate:* Idle Private WS ≤ 34 MB, CPU = 0%.
* **Milestone 5: Native Taskbar Autohide & Exact Restore**
  * *Deliverable:* Original state read with `ABM_GETSTATE` and written to `taskbar_state.toml` before any change. Native taskbar switches to autohide. Clean exit, panic, and the emergency hotkey restore exactly the saved state. After a Task Manager kill, the next launch repairs it from the file. Verified with autohide initially off **and** initially on.
  * *Memory Gate:* Idle Private WS ≤ 34 MB, CPU = 0%.
* **Milestone 6: System Status Indicators**
  * *Deliverable:* Right zone pills show live Bluetooth, Wi-Fi, master volume/mute and battery, all callback-driven.
  * *Memory Gate:* Idle Private WS ≤ 36 MB, CPU = 0%.
* **Milestone 7: Flyout Panels (Sound & SMTC Media)**
  * *Deliverable:* Volume opens the 340 px flyout. Master slider, device list, and media session cards (WIC-decoded album art with dominant tint) render smoothly. Calendar and battery flyouts.
  * *Memory Gate:* Private WS with panel open ≤ 40 MB; after closing, back within 3 MB of the M6 idle value.
* **Milestone 8: TOML Theming System & Live File Reload**
  * *Deliverable:* Editing `themes/dark_glass.toml` or `config.toml` updates colors, sizes and layout without restart.
  * *Memory Gate:* Idle Private WS ≤ 38 MB, CPU = 0%.
* **Milestone 9: Release Hardening & Packaging**
  * *Deliverable:* Single `.exe` (< 4 MB) with embedded manifest and icon, single-instance mutex, HKCU Run key toggle, install/uninstall script.
  * *Memory Gate:* Idle Private WS 25 to 35 MB, peak ≤ 50 MB. No working set trimming.

---

## 14. Risks & Verification Test Plan

### Technical Risks & Mitigations

1. **Risk: DWM blur or custom corner radii do not behave as hoped on borderless DirectComposition windows.**
   * *Mitigation:* M1 spike runs first. Fallbacks: fixed DWM radius, or a solid tinted fill with no blur. Floating pills stay out of v1.
2. **Risk: Native taskbar left in autohide after a crash or kill.**
   * *Mitigation:* The state file is written before any change and repaired on next launch. Autohide never removes the taskbar; it stays reachable at its screen edge.
3. **Risk: Elevated (Run as Administrator) windows cannot be focused.**
   * *Mitigation:* UIPI can reject `SetForegroundWindow` from our non-elevated process. The dock shows a shield badge on elevated windows and a short message if focus cannot move.
4. **Risk: Third-party tray icons (Docker, VPN clients, etc.) are missing from the new bar.**
   * *Mitigation:* Accepted for v1. With the native taskbar in autohide, hovering its edge still shows the tray. A tray mirror is in v2.
5. **Risk: Native Quick Settings / notification flyouts open at the native taskbar position, away from our bar.**
   * *Mitigation:* Accepted as a v1 stopgap; custom panels in v2.
6. **Risk: Emergency hotkey combination already taken by another app.**
   * *Mitigation:* Check `RegisterHotKey` result in M1; configurable fallback (`Ctrl + Shift + Alt + F12`).
7. **Risk: Windows 11 build differences in taskbar behavior.**
   * *Mitigation:* Only the documented `SHAppBarMessage` API is used. Taskbar edge is read at runtime, not assumed.
8. **Risk: Undocumented `IPolicyConfig` changes or disappears.**
   * *Mitigation:* Every call checked; falls back to `ms-settings:sound`.

### Multi-Scenario Test Matrix

* **Display scaling:** 100%, 125%, 150%, 200%; also changing scale while running (`WM_DPICHANGED`).
* **Native taskbar position:** left (dev machine), bottom, and Explorer restart (`TaskbarCreated`).
* **Native taskbar initial state:** autohide off and autohide on at launch; confirm exact restore in both cases, including after a Task Manager kill + relaunch.
* **Multi-monitor:** single, dual (horizontal & vertical), and primary display disconnect/reconnect (`WM_DISPLAYCHANGE`).
* **Power & sleep:** sleep, hibernate, wake. COM endpoints and audio enumerators re-subscribe; clock re-syncs; window list rescanned once.
* **Fullscreen:** exclusive-fullscreen game, borderless-fullscreen game, fullscreen browser video, PowerPoint presentation mode. Bar and dock hide and return, with no polling (verify 0% CPU while idle in fullscreen).
* **Idle CPU check:** Process Explorer shows no context switches from the app over 60 seconds of a static screen, apart from the minute tick.

---

## 15. Decisions Log

Decisions made on the open questions from revision 1:

1. **Start / launcher button:** Option A, using the native Start menu, opened by tapping the Win key with `SendInput`. The shell GUID from revision 1 was wrong (the similar well-known GUID is Show Desktop) and is removed.
2. **Native taskbar recovery:** Option A: single process with panic hook and emergency hotkey, **plus** the saved-state file from Section 10 that also repairs after a crash or kill on next launch. No watchdog exe.
3. **Default bar style:** Continuous translucent strip for v1. Floating pills come later only if the M1 spike proves them feasible.
4. **Virtual desktops in v1:** Option B: the indicator dots are hidden in v1. Switching and the indicator return in v2 with the private COM interface.

### Still open (decided by the M1 spike, not by the user)

* Achievable window outline radius with blur (fixed DWM ~8 px vs. custom).
* Whether floating pills move into v1.x, based on the measured memory of per-pill windows.
