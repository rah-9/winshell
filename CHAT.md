# Agent Collaboration Log (CHAT.md)

> [!IMPORTANT]
> **STRICT RETENTION RULE**:
> - **DO NOT DELETE OR OVERWRITE ANY PREVIOUS ENTRIES IN THIS FILE.**
> - **ALL NEW MESSAGES MUST BE APPENDED TO THE END OF THIS DOCUMENT.**
> - Every entry must begin with a level-3 header including the timestamp (UTC or Local ISO) and the Agent identity (e.g. `### [2026-11-21 22:30 UTC] - Assistant` or `### [2026-11-21 22:30 UTC] - Claude`).
> - This document serves as the persistent cross-agent chat and handoff log across sessions, models, and tool chains.

---

### [2026-11-21 22:20 +05:30] - Assistant

**Topic**: Welcome & Project State Overview

Hello Claude and fellow agents!

I have just completed an initial pass over the repository and synchronized with the latest upstream commits (`9fecc3a`). Here is the current state of the project for whoever picks up the next task:

1. **Current Codebase State**:
   - The project is **WinShellTheme (`winshell`)** version `0.9.0` (Rust 2024 edition), targeting Windows 11 (22H2 build 22621+).
   - All **61 unit tests** pass cleanly (`cargo test`).
   - The codebase compiles with zero warnings or errors.

2. **Recent Implementations (Milestone 1–9 + Recent Commits)**:
   - **Dynamic Auto-Hide Engine** (`src/shell/autohide.rs`): Automatically hides top bar and dock when windows overlap or maximize, giving apps 100% of the screen. Edge reveals are handled by invisible 1 px strips (`WS_EX_LAYERED`, alpha 1/255) with a 160 px top-right safe zone for maximized window caption buttons.
   - **Interactive Wi-Fi Flyout** (`src/network.rs`, `src/shell/panel.rs`): Live SSID discovery, adapter radio toggle, signal strength glyphs, and one-click connection to saved profiles.
   - **Interactive Bluetooth Flyout** (`src/sysmon.rs`, `src/shell/panel.rs`): Adapter radio toggle and paired device listing with connection states ("Connected" vs "Paired"), bounded by a 5-second timeout guard.
   - **Shell Host Process Filtering** (`src/foreground.rs`): StartMenu, SearchHost, TextInputHost, and ShellExperienceHost are classified as `Kind::Ignore` with system directory verification so system overlays don't overwrite active application titles.
   - **Crash-Proof Taskbar Restoration** (`src/taskbar.rs`, `src/taskbar_state.rs`): Preserves original `ABM_GETSTATE` bitmask to disk before changing, restoring on exit, panic, emergency hotkey (`Ctrl+Shift+Alt+Esc`), or next launch.

3. **Ground Rules for Collaborating in this Repo**:
   - Please read `AGENT.md` before making architectural modifications.
   - **Never delete anything from `CHAT.md`**. Always append new updates, questions, thoughts, or handoff notes at the bottom of this file.
   - Maintain the zero-polling, pure event-driven architecture.
   - Always run `cargo test` after code edits to ensure pure logic tests continue to pass.

Looking forward to collaborating with you here!
