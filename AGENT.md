# Agent Instructions & Repository Guidelines (AGENT.md)

This document contains operational instructions, architecture invariants, and collaboration rules for AI assistants (Claude, Cursor, Copilot, etc.) working on the **WinShellTheme** codebase.

---

## 1. Golden Rules for AI Agents

1. **STRICT PRESERVATION OF `CHAT.md`**:
   - **`CHAT.md` is strictly append-only.**
   - **NEVER** overwrite, delete, truncate, or rewrite existing messages in [`CHAT.md`](CHAT.md).
   - When communicating between agents (e.g. Assistant and Claude) or documenting session handoffs, always append a new entry to the bottom of [`CHAT.md`](CHAT.md) using the standard format:
     ```markdown
     ### [YYYY-MM-DD HH:MM TZ] - <Agent Name>
     **Topic**: <Topic Title>
     <Your message here>
     ```

2. **Non-Elevated / User-Mode Integrity**:
   - WinShellTheme must **never** require administrative privileges. The application manifest specifies `asInvoker`.
   - Do **not** introduce kernel drivers, API hooking (like MinHook/Detours), DLL injection, or undocumented system hacks that trip anti-cheat software or security tools.

3. **Taskbar State Restoration Invariant**:
   - The native taskbar's original state must **always** be protected.
   - Any code that modifies `ABS_AUTOHIDE` must do so through [`src/taskbar.rs`](src/taskbar.rs) (`TaskbarGuard`) ensuring [`taskbar_state.toml`](src/taskbar_state.rs) is written *before* the modification and deleted only after a successful restore.

4. **Zero-Polling / Event-Driven Architecture**:
   - Never add periodic polling loops or busy-wait timers for system state.
   - Use Windows event mechanisms:
     - Foreground and window tracking: `SetWinEventHook` (out-of-context).
     - Audio: `IAudioEndpointVolumeCallback` and `IMMNotificationClient`.
     - Network: `WlanRegisterNotification` and `NotifyIpInterfaceChange`.
     - Power: `RegisterPowerSettingNotification`.
     - Radios & Media: WinRT asynchronous events (`StateChanged`, `PlaybackInfoChanged`).
     - Configuration & Themes: `ReadDirectoryChangesW`.
   - Background threads communicate back to the UI thread via `PostMessageW(WM_SHELL_*)` with atomic debouncing/coalescing.

5. **Memory & Performance Targets**:
   - Target idle Private Working Set: $\le 30\text{--}32\text{ MB}$.
   - Idle CPU: 0%.
   - Keep GPU textures in check: the LRU icon cache in [`src/lru.rs`](src/lru.rs) has an 8 MB hard budget.

6. **Safety & Input Sanitization**:
   - AUMIDs from foreign processes must pass [`is_safe_aumid()`](src/util.rs) before reaching shell namespace APIs to prevent directory traversal or URI spoofing.
   - DLL loading is hardened via [`src/hardening.rs`](src/hardening.rs).

---

## 2. Codebase Organization

```text
F:\theme\
├── Cargo.toml               # Package manifest (Rust 2024 edition)
├── build.rs                 # Compiles winshell.manifest and resources
├── README.md                # Public project documentation
├── CHAT.md                  # Collaborative chat log between AI agents (APPEND-ONLY)
├── AGENT.md                 # This file (Agent instructions & guidelines)
├── PLAN.md                  # Detailed design specification
├── resources/               # Manifest, app icon, resource script
├── scripts/                 # PowerShell installation and uninstallation scripts
└── src/
    ├── main.rs              # winshell.exe entry point (sets windows_subsystem = "windows")
    ├── lib.rs               # Library export of pure logic & platform modules
    ├── bin/
    │   └── spike.rs         # Milestone 1 performance and DWM benchmarking spike
    ├── shell/               # Core shell UI and window management
    │   ├── mod.rs           # Shell app coordinator, WndProc dispatch, popup management
    │   ├── autohide.rs      # Dynamic auto-hide engine and invisible edge triggers
    │   ├── bar.rs           # Top bar rendering and layout
    │   ├── dock.rs          # Bottom dock rendering, slot grouping, hover magnification
    │   ├── panel.rs         # Flyout panels (Sound, Wi-Fi, Bluetooth, Battery, Calendar)
    │   ├── popup.rs         # Wrapper for borderless popup HWNDs + DComp surfaces
    │   └── theme.rs         # Layout constants and thread-local palette access
    ├── actions.rs           # Start menu triggering, app launch, foreground activation
    ├── appbar.rs            # Win32 SHAppBarMessage edge reservations (top & bottom)
    ├── apps.rs              # Top-level window enumeration and dock eligibility
    ├── assets.rs            # Background shell icon and friendly name loader thread
    ├── audio.rs             # Core Audio endpoints and default device routing
    ├── calendar.rs          # Gregorian month grid math (pure, unit-tested)
    ├── clock.rs             # Localized date/time and minute-boundary one-shot timer
    ├── config.rs            # config.toml parsing and comment-preserving pin edits
    ├── dock_model.rs        # App grouping model (pure, unit-tested)
    ├── dwm.rs               # DWM backdrops (Acrylic/Mica), corners, frame extension
    ├── foreground.rs        # Active window WinEvent hooks and shell host filtering
    ├── hardening.rs         # Startup security mitigations (DLL directory policies)
    ├── hotkey.rs            # Emergency exit hotkey registration (Ctrl+Shift+Alt+Esc)
    ├── icon.rs              # HICON / HBITMAP to 32-bit premultiplied BGRA conversion
    ├── instance.rs          # Single-instance mutex
    ├── log.rs               # Lightweight file logger with 1 MB rotation
    ├── lru.rs               # Byte-budgeted LRU cache (pure, unit-tested)
    ├── mem.rs               # Private Working Set profiling
    ├── network.rs           # Wi-Fi scanning, profile connection, IP change notification
    ├── power.rs             # Battery and power status notifications
    ├── registry.rs          # HKCU autostart Run key and theme reading
    ├── render.rs            # D3D11 / D2D 1.1 / DirectWrite / DirectComposition engine
    ├── sysmon.rs            # WinRT worker: Bluetooth radio, GSMTC media sessions & art
    ├── taskbar.rs           # Taskbar autohide guard and crash recovery
    ├── taskbar_state.rs     # taskbar_state.toml serialization (pure, unit-tested)
    ├── theme_file.rs        # Theme palette parser and validator (pure, unit-tested)
    ├── util.rs              # Pure helper functions with unit tests
    ├── watcher.rs           # ReadDirectoryChangesW live reload thread
    ├── win.rs               # Per-Monitor V2 HiDPI, monitor geometry, popup creation
    └── winevents.rs         # Window lifecycle WinEvent hooks (show, hide, cloak, destroy)
```

---

## 3. Workflow & Testing Rules

- **Testing**:
  Always run `cargo test` after modifying any code:
  ```powershell
  cargo test
  ```
  Pure logic modules (`calendar`, `config`, `dock_model`, `lru`, `taskbar_state`, `theme_file`, `util`, and bar glyph step tests) have comprehensive unit tests. Ensure all tests pass.

- **Formatting & Linting**:
  Follow `rustfmt.toml` and standard Rust idioms. Avoid unnecessary dependencies.

- **Cross-Agent Communication**:
  Whenever you make a significant change, fix a bug, or pause a multi-step task, post an entry in [`CHAT.md`](CHAT.md) so the next agent (or the user) has full visibility into what was done and what remains.
