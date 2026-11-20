# WinShellTheme

A lightweight, user-mode Windows 11 shell overlay in Rust: a translucent top bar,
a floating dock and flyout panels, drawn with Direct2D + DirectComposition over
the DWM backdrop. `explorer.exe` keeps running; nothing needs admin rights.
See [PLAN.md](PLAN.md) for the full design.

**Status: Milestone 1** (spike + window scaffolding + recovery hotkey).

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

## Files

| Path | Contents |
| :-- | :-- |
| `%APPDATA%\WinShellTheme\winshell.log` | app log (layout, AppBar events, memory) |
| `%APPDATA%\WinShellTheme\spike.log` | spike log |
| `%APPDATA%\WinShellTheme\crash.log` | panic log |

## Source layout

| File | Role |
| :-- | :-- |
| `src/render.rs` | shared D3D11/D2D/DComp/DWrite devices, per-window DComp surfaces |
| `src/dwm.rs` | backdrop, corner, dark-mode and frame attributes |
| `src/appbar.rs` | reservation AppBars, native taskbar edge |
| `src/hotkey.rs` | emergency hotkey |
| `src/mem.rs` | Private Working Set measurement |
| `src/main.rs` | M1 scaffold app |
| `src/bin/spike.rs` | M1 spike |
