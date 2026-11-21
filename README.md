# WinShellTheme

A lightweight, user-mode Windows 11 shell overlay in Rust: a translucent top bar,
a floating dock and flyout panels, drawn with Direct2D + DirectComposition over
the DWM backdrop. `explorer.exe` keeps running; nothing needs admin rights.
See [PLAN.md](PLAN.md) for the full design.

**Status: Milestones 1–7 built** (M8 themes / live reload and M9 packaging remain). Nothing has been run on Windows yet; see the checks under each milestone.

## Requirements

* Windows 11 22H2 (build 22621) or newer.
* To build on Windows: stable Rust with the MSVC toolchain (Visual Studio Build
  Tools 2022 + Windows 11 SDK).

```powershell
cargo build --release
```

Outputs `target\release\winshell.exe` and `target\release\spike.exe`.

## Emergency exit

**Ctrl + Shift + Alt + Esc** removes the reserved screen edges and quits
(falls back to **Ctrl + Shift + Alt + F12** if Esc is taken; the log says which).
M1 never touches the native taskbar.

## M1 step 1: run the spike first

The spike answers the open questions in PLAN.md (blur on borderless
DirectComposition windows, achievable corner radius, whether an invisible
click-through AppBar holds space, memory baseline).

```powershell
.\spike.exe                      # defaults: acrylic, dwm_round, layered reservation
.\spike.exe --reserve plan       # PLAN.md rev 3 reservation (WS_EX_TRANSPARENT + HTTRANSPARENT)
.\spike.exe --reserve hidden     # registered but never-shown reservation window
.\spike.exe --pills 8            # price the floating-pills idea
.\spike.exe --redirect           # compare against a normal redirected window
.\spike.exe --help
```

While it runs, **Ctrl + Shift + Alt +**:

| Key | Action |
| :-- | :-- |
| B | cycle backdrop (acrylic, mica, tabbed, none) |
| C | cycle corners (dwm_round, dwm_small, none) |
| T | cycle tint alpha (72, 40, 0, 100 %) |
| S | toggle tint shape (full rectangle / rounded) |
| F | toggle `DwmExtendFrameIntoClientArea` |
| M | log a memory snapshot and the current work area |
| R | write the report now |
| Esc | quit and write the report |

Things to try while it runs:

1. Click into other apps: the strip must stay blurred (transient acrylic).
   Press B to Mica and confirm it greys out when inactive (expected).
2. Zoom a screenshot of the dock corner to measure the radius.
3. Maximise a window: it should stop below the top 44 px.
4. Click desktop icons or windows under the reserved band (outside the strip).
5. Open a fullscreen video or press F11 in a browser: the log should show
   `ABN_FULLSCREENAPP`.
6. Leave it idle for 30 s (memory is logged at +5 s and +30 s).

On exit it writes `spike-report.txt` in the current folder: DWM attribute
results, work area before/after, AppBar notifications, a Private Working Set
timeline and a checklist for what you saw. Run each `--reserve` variant once.
The results go into PLAN.md as the "spike output" note.

> Why three reservation variants: `HTTRANSPARENT` only forwards a click to
> windows on the *same thread*, and `WS_EX_TRANSPARENT` only makes a window
> click-through when it is also `WS_EX_LAYERED`. The plan's combination may
> therefore swallow clicks on the desktop; `layered` (alpha 0) is the
> documented click-through form and is the current default.

## M1 steps 2 and 3: the scaffold

```powershell
.\winshell.exe                   # --reserve layered|hidden|plan, --backdrop, --corners
```

Shows the top bar and a placeholder dock, reserves 44 px at the top and 74 px
at the bottom (maximised windows stop at both), lays out around the native
taskbar edge, hides on fullscreen apps and re-registers after an Explorer
restart. Exit with the emergency hotkey.

Options: `--reserve layered|hidden|plan`, `--backdrop acrylic|mica|tabbed|none`,
`--corners round|small|none`, `--no-taskbar` (leave the native taskbar alone this run).

## M2: active window and clock

The top bar's left zone shows your user handle and the active window's icon
and title (truncated at 40 characters). It follows focus changes and title
changes (e.g. browser tabs) through WinEvent hooks; the title hook is scoped to
the foreground app's process. Clicking the taskbar or the bar itself keeps
the previous app shown; the desktop shows "Desktop".

The right zone shows the clock ("Fri 20 Nov, 02:06 PM", day and month names in
your locale). It redraws only the clock pill, once per minute, from a one-shot
timer re-armed for the next boundary. It re-syncs after sleep/resume, a manual
time or time-zone change, and a regional-format change.

Things to check:

1. Switch between apps and browser tabs: icon and title follow immediately.
2. Watch the clock turn over: it changes within a few ms of the minute.
3. Change the time or time zone in Settings, then sleep and wake the PC:
   the clock is correct straight away.
4. Memory gate: idle Private Working Set ≤ 30 MB (Task Manager, Details tab,
   "Memory (private working set)" column). The log also records it at
   startup and at the first minute tick.
5. Idle CPU: 0 % between minute ticks (Process Explorer shows no context
   switches apart from the tick and real focus/title changes).

Packaged (Store) apps may show a generic icon until M3 adds shell icon lookup.

## M3: running apps in the dock

The dock shows a Start slot, a separator and one icon per running app. Windows
are grouped by app: the AUMID for packaged/Store apps (and apps that set one,
like browser profiles), otherwise the exe path. A window counts if it is
visible, not cloaked (so other virtual desktops and suspended Store apps are
left out), has a title, and is unowned or marked as an app window.

* Tracking is event-driven: WinEvent hooks for show, hide, destroy and
  cloak/uncloak. A full window scan runs only at startup, on resume, on a
  display change and when Explorer restarts.
* Icons are loaded by a background shell thread (`IShellItemImageFactory`),
  at least 64 x 64 and larger at high DPI so the M4 hover zoom stays sharp.
  They are kept as GPU bitmaps in an 8 MB LRU cache.
* Indicators: a dot for a running app, two dots for several windows, an
  accent pill under the focused app.
* The dock window resizes as apps come and go; the reserved bottom band stays
  fixed. Clicking icons arrives in M4.

Things to check:

1. Open and close apps: icons appear and disappear without delay.
2. Open a second window of the same app: it shares one slot with two dots.
3. Store apps (Calculator, Settings, Terminal) show their own icons.
4. Move a window to another virtual desktop: its icon leaves the dock.
5. Memory gate: idle Private Working Set ≤ 32 MB with your usual apps open.

## Configuration

`%APPDATA%\WinShellTheme\config.toml` is created on first run (schema from
PLAN.md section 11). Changes apply on restart until M8 adds live reload.

```toml
[general]
user_handle = ""                 # empty = "@" + your Windows user name

[top_bar]
enabled_pills = ["bluetooth", "wifi", "volume", "battery", "clock", "notifications", "quick_settings"]

[dock]
magnification = false            # draw the hovered icon at 1.15x
pinned_apps = ["explorer.exe"]   # exe paths, bare names on PATH, or AUMIDs

[taskbar]
autohide_native = true
```

## M4: pinning, launching, switching

* **Start** taps the Windows key (native Start menu).
* **Click an icon:** not running → launch; one window → bring it forward, or
  minimise it if it is already in front; several windows → a window list
  above the icon (plus "New window").
* **Middle-click:** always a new instance.
* **Right-click:** New window / Open, Pin to Dock / Unpin from Dock, Close
  window(s), Quit app (closes every window of the app politely; never kills).
* **Pinned apps** sit before the running ones, separated by a divider, and stay
  when closed. Pinning edits `config.toml` with `toml_edit`, so your comments
  and layout survive.
* **Hover:** highlight plus a capsule tooltip with the app's name. With
  `magnification = true` the hovered icon is drawn at 1.15x.
* **Administrator windows** get a shield badge; if Windows refuses to switch
  to one, a short message appears above the dock instead of failing silently.
* **Active window pill (top bar):** click to minimise / restore, right-click for
  Minimize, Maximize, Snap left, Snap right, Close.

## M5: native taskbar autohide

With `autohide_native = true` (default), once the bar and dock are up the
native taskbar is switched to autohide, the normal user setting. Its original
state is written to `taskbar_state.toml` **before** the change and restored
exactly on clean exit, on a crash (panic hook), via the emergency hotkey, and
on the next launch after a kill (a leftover state file means the last run did
not exit cleanly). The taskbar window itself is never hidden or modified.

Check with autohide **off** and **on** before launch: after exit (and after
killing `winshell.exe` in Task Manager and starting it again), the setting in
Settings → Personalization → Taskbar must be back to what it was.

## M6: status pills

Left to right: Bluetooth, Wi-Fi, volume, battery, clock, notifications, quick
settings. All callback-driven: Core Audio volume/device callbacks, WLAN and IP
interface notifications, power-setting notifications, and the WinRT Bluetooth
radio on the system monitor thread. Bluetooth hides without a radio, battery
hides without a battery; Ethernet shows its own glyph.

* Volume → Sound flyout (right-click toggles mute). Battery → Battery flyout.
  Clock → Calendar flyout.
* Wi-Fi / Bluetooth → their Settings pages (in-flyout lists are v2).
* Bell / gear → Win+N / Win+A (v1 stopgap: the native flyouts open at the
  native taskbar's edge).

## M7: flyouts and media

One 340 px flyout under the top bar, sliding in 8 px and fading over 140 ms
with DirectComposition animations (no CPU frame loop). It closes when you
click elsewhere or press Esc.

* **Sound:** volume and microphone sliders (drag), mute toggles, Volume mixer
  link, output device list (switches the default device via the undocumented
  `IPolicyConfig`; any failure opens Sound settings instead), and up to three
  media session cards with album art, a tint taken from the art, and
  previous / play-pause / next.
* **Battery:** percentage, charging / time left, energy saver, settings link.
* **Calendar:** today's date, month grid in your locale's week order, month
  navigation (click the month title to jump back to today).
* **Dock mini player:** album art, title, artist and play/pause for the
  current media session; click it for the Sound flyout.

## Files

| Path | Contents |
| :-- | :-- |
| `%APPDATA%\WinShellTheme\winshell.log` | app log (layout, AppBar events, memory) |
| `%APPDATA%\WinShellTheme\spike.log` | spike log |
| `%APPDATA%\WinShellTheme\crash.log` | panic log |
| `%APPDATA%\WinShellTheme\config.toml` | settings and pinned apps |
| `%APPDATA%\WinShellTheme\taskbar_state.toml` | original taskbar state (only while running or after a crash) |

## Source layout

| File | Role |
| :-- | :-- |
| `src/render.rs` | shared D3D11/D2D/DComp/DWrite devices, per-window DComp surfaces |
| `src/dwm.rs` | backdrop, corner, dark-mode and frame attributes |
| `src/appbar.rs` | reservation AppBars, native taskbar edge |
| `src/hotkey.rs` | emergency hotkey |
| `src/mem.rs` | Private Working Set measurement |
| `src/clock.rs` | minute-boundary clock (one-shot timer) |
| `src/foreground.rs` | active window WinEvent hooks |
| `src/icon.rs` | window icon -> BGRA pixels |
| `src/apps.rs` | dock eligibility and app identity (AUMID / exe) |
| `src/winevents.rs` | window lifecycle WinEvent hooks |
| `src/assets.rs` | shell icon / name loader thread |
| `src/dock_model.rs` | running-app grouping (unit-tested) |
| `src/lru.rs` | byte-budgeted icon cache (unit-tested) |
| `src/util.rs` | pure helpers with unit tests |
| `src/config.rs` | config.toml schema and comment-preserving pin edits (unit-tested) |
| `src/taskbar.rs`, `src/taskbar_state.rs` | taskbar autohide and exact restore |
| `src/actions.rs` | launch, activate, minimise, snap, close, send keys |
| `src/audio.rs` | Core Audio volume, devices, default-device switching |
| `src/network.rs` | Wi-Fi / Ethernet status |
| `src/power.rs` | battery status and notifications |
| `src/sysmon.rs` | system monitor thread: Bluetooth radio, media sessions, album art |
| `src/calendar.rs` | month grid maths (unit-tested) |
| `src/main.rs`, `src/shell/` | the shell app: bar, dock, flyout panels, tooltip |
| `src/bin/spike.rs` | M1 spike |
