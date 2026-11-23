# Pilot Roadmap

WinShellTheme is a starting point for a modern, interactive, memory-conscious Windows desktop built in Rust. **The desktop replacement is still under development.** The items below are open work, not a completed-feature checklist or a release-date commitment.

Start with reliable shell behavior and usable interactions. Broader phone integration and Terminal customization follow that foundation. Discuss proposals in [GitHub issues](https://github.com/rah-9/winshell/issues); see [CONTRIBUTING.md](CONTRIBUTING.md) to participate.

## Current implementation and gaps

- The bar, left dock, system-status panels, themes, and reversible immersive windows have initial implementations.
- Native-taskbar hiding and work-area restoration have recovery mechanisms and initial smoke tests. User-reported inconsistencies and broader hardware/Explorer behavior remain open; taskbar replacement is not considered finished.
- Shell layout currently targets the **primary monitor**. Full per-monitor bars/docks, mixed-DPI placement, and monitor transitions need implementation and verification.
- Wi-Fi can connect to saved profiles. Unsaved-network credentials use the native Windows picker. Bluetooth lists paired classic devices and uses Windows settings for pairing. These handoffs need clear labels and better in-app feedback.
- Phone connection workflows, a unified cross-app sizing policy, and Terminal customization are not implemented as complete features.
- The latest validation passed 67 tests. These cover selected logic and native-window behavior, not the full compatibility matrix below. Memory efficiency remains a measured engineering goal, not a universal memory guarantee.

## P0: Reliable desktop replacement

| ID | Open improvement | Acceptance criteria |
| --- | --- | --- |
| P0-01 | Finish native-taskbar hiding | Identify and fix cases where the primary or secondary taskbar reappears, or a reserved strip remains. Record before/running/after visibility and work-area geometry. |
| P0-02 | Verify restoration and shutdown | Clean exit and emergency exit restore state and leave no running process. Recovery after a forced termination restores valid saved state; failed restoration keeps the recovery file. |
| P0-03 | Prevent desktop obstruction | Bar/dock visibility, edge reveals, and any invisible windows must leave folders, icons, and clicks usable. Reproduce the reported line crossing desktop items and add a regression check. |
| P0-04 | Handle shell lifecycle changes | Explorer restart, sleep/resume, display changes, and settings reload must keep the shell reachable without polling, repeated flicker, or loss of recovery state. |

Suggested contribution: provide a minimal reproduction for a taskbar-hiding failure with Windows build, display geometry, launch mode, and diagnostic output.

## P1: Multi-monitor layout and interaction

| ID | Open improvement | Acceptance criteria |
| --- | --- | --- |
| P1-01 | Define per-monitor behavior | Agree on which displays host bars/docks and how users choose the active display. Avoid duplicate controls and unrelated-monitor visibility changes. |
| P1-02 | Implement mixed-DPI placement | Support 100%, 125%, 150%, and 200% scaling, negative coordinates, portrait displays, primary-display changes, and monitor reconnects. Rebuild device-sized resources where needed. |
| P1-03 | Fit every panel to its screen | Place panels near their triggers, keep interactive content inside the target monitor, and provide reachable scrolling when content exceeds available height. Verify long lists and long names. |
| P1-04 | Fix button and scroll behavior | Ensure hit targets match rendered controls at each DPI. Verify sliders, wheel input, overflow arrows, repeated clicks, selection, and focus loss. Preserve scroll position during updates. |
| P1-05 | Improve Wi-Fi/Bluetooth workflows | In-shell actions give progress, success, and failure feedback. Buttons that open Windows explicitly say so. Unsaved-network and pairing flows must have a defined, honest scope. |
| P1-06 | Improve accessibility | Provide keyboard navigation, visible focus, readable contrast, meaningful control labels, and accessible names/roles for custom-drawn controls. |

Suggested contributions: one reproducible clipping bug, one control with mismatched hit testing, an empty-state improvement, or a written two-monitor test report. Confirm task scope in an issue before larger layout changes.

## P2: App compatibility and device experience

| ID | Open improvement | Acceptance criteria |
| --- | --- | --- |
| P2-01 | Browser and email app fitting | Test traditional desktop, packaged, and browser-installed apps. Launch, activation, minimize/restore, grouping, and immersive exit must preserve the correct window and original bounds. |
| P2-02 | Consistent dimension handling | Fit shell-owned content across small laptop screens, portrait, ultrawide, and large displays. Handle long titles, font sizes, and localized text without covering controls. |
| P2-03 | Immersive compatibility and user choice | Avoid forcing dialogs and unsupported apps into broken layouts. Define per-app opt-outs and test app-drawn title bars. Do not claim the shell can redesign every application's internal UI. |
| P2-04 | Phone connection discovery | First define USB detection, Bluetooth status, and Phone Link handoffs as separate tasks. Handle connect/disconnect and unsupported devices honestly. Choose supported APIs before implementation. |
| P2-05 | Phone actions and permissions | Define which actions are intended, such as opening a supported file-transfer location or native device settings. Keep user consent, disconnection, and permission/error states visible; do not imply file transfer or notification sync already works. |
| P2-06 | Media and audio edge cases | Test multiple media sessions, absent devices, device switches, long track names, artwork changes, and reconnection. Avoid stale controls or unreadable cards. |

App fitting means reliable native-window placement and shell layout. Restyling arbitrary websites, email content, and third-party client areas is outside the current shell implementation.

## P3: Performance, distribution, and community

- [ ] Publish reproducible idle CPU, private working set, and panel-open measurements with Windows build, GPU, DPI, and workload. Compare regressions; do not meet targets by trimming the working set.
- [ ] Investigate memory/resource growth during repeated opening, closing, display changes, and device reconnects; keep the icon-cache budget enforced.
- [ ] Add Windows CI for formatting, builds, and automated tests; describe the manual cases CI cannot cover.
- [ ] Add the full MIT and Apache-2.0 license files matching the existing manifest declaration.
- [ ] Add reviewed, small GitHub issues and appropriate `good first issue` / `help wanted` labels.
- [ ] Add a code of conduct, repository topics, and a short demonstration of actual behavior with known limitations disclosed.
- [ ] Provide verified downloadable Windows builds, checksums, release notes, and clear install/uninstall/recovery instructions.
- [ ] Keep pilot status and known problems prominent so contributors can distinguish existing prototypes from unfinished workflows.

## Later: Terminal and command-prompt customization

After the shell's reliability and layout stages, design a separate, opt-in customization workflow for Windows Terminal and supported command-prompt hosts. Use the reference images to define typography, spacing, transparency, and interaction requirements. Back up and preserve user profiles and shortcuts, offer a preview and restore path, and distinguish Terminal host styling from styling applications running inside it.

No Terminal settings should be changed as part of unrelated shell fixes.

## Validation matrix

A pull request need not test every combination. Select the cases affected by the change, record the actual results, and call out missing coverage.

| Area | Scenarios to record |
| --- | --- |
| Desktop lifecycle | Normal launch/exit, emergency exit, second launch, forced termination and relaunch, Explorer restart, sleep/resume |
| Native taskbar | Initially visible/auto-hidden; replacement enabled/disabled; primary and secondary displays; original work-area restoration |
| Display topology | One/two/three monitors, primary display change, disconnect/reconnect, portrait, negative coordinates |
| Resolution and scale | Small laptop screen, 1920x1080, 2560x1440, 4K, ultrawide; 100%, 125%, 150%, 200%; mixed scales |
| Interaction | Mouse, wheel, keyboard, sliders, overflow navigation, focus changes, repeated activation, panel dismissal |
| Content | Long app/device/network names, long media titles, localization, empty lists, errors, changing data while scrolling |
| Applications | Explorer, a browser, an email app, a packaged app, a browser-installed app, fixed-size dialogs, elevated apps, external full-screen apps |
| Devices | Wi-Fi absent/off/connected; saved and unsaved networks; Bluetooth absent/off/paired; phone attach/detach; audio switch; no battery |
| Recovery | Original taskbar/window geometry restored, interrupted restoration retained for retry, monitor topology changed before recovery |
| Resources | Idle CPU/private working set, repeated panel cycles, icon/art cache use, handles and resource lifetime |

An item is ready to close when its agreed acceptance criteria are met, relevant checks pass, and someone can reproduce the verification. Partial implementations should identify the remaining work instead of marking the whole area complete.
