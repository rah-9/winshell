# Contributing to WinShellTheme

Thanks for helping build a more modern, interactive Windows desktop in Rust.

WinShellTheme is an **early pilot project**. The current bar, dock, and panels are working prototypes; taskbar replacement, multi-monitor behavior, layout, accessibility, and device integration still need work. Passing unit tests does not mean every Windows setup has been verified.

Read the [README](README.md) for current behavior and the [roadmap](ROADMAP.md) for priorities, known limitations, and acceptance criteria. Code, UX feedback, testing, documentation, and performance measurements are all welcome.

## Find something to work on

Browse [existing issues](https://github.com/rah-9/winshell/issues) before opening a new one. Look for `good first issue` and `help wanted` when those labels are available. The roadmap contains suggested tasks; a roadmap item is not automatically an assigned GitHub issue.

For a small, clearly scoped fix, open a pull request. For a new feature or a change to window management, taskbar behavior, dependencies, or architecture, open an issue first so the approach can be discussed. You do not need to email the maintainer before contributing.

Good starting points include reproducing panel overflow, checking buttons at different display scales, documenting Windows-settings handoffs, improving empty/error states, and adding a regression test for a confirmed layout bug. Test reports are useful even if you do not write Rust.

## Set up a development environment

You need Windows 11 22H2 or newer, stable Rust with the MSVC toolchain, Visual Studio Build Tools with C++ support, and the Windows 11 SDK. Windows-specific UI changes need testing on Windows; platform-independent tests alone cannot validate them.

1. Fork the repository on GitHub.
2. Clone your fork and create a branch for your change:

```powershell
git clone https://github.com/YOUR-USERNAME/winshell.git
cd winshell
git remote add upstream https://github.com/rah-9/winshell.git
git switch -c fix/describe-your-change
cargo build --locked
```

Start with preview mode, which leaves the native taskbar unchanged:

```powershell
.\target\debug\winshell.exe --preview --settings
```

Preview keeps the panels visible when focus changes to make inspection easier. Verify normal focus dismissal and auto-hide behavior separately in a normal session. Close the preview before changing launch modes; a second instance reuses the running shell.

For a deliberate native-taskbar replacement test:

```powershell
Start-Process -FilePath .\target\debug\winshell.exe -ArgumentList '--quit' -Wait -WindowStyle Hidden
.\target\debug\winshell.exe --replace-taskbar --settings
```

Know the controls before testing: **Ctrl+Shift+Alt+Esc** restores the desktop and exits; **Ctrl+Shift+Alt+Space** opens desktop controls; **Ctrl+Shift+Alt+F** restores or toggles an immersive window. See the README for command-line recovery after a forced exit.

## Report a bug

Use the [bug-report form](https://github.com/rah-9/winshell/issues/new?template=bug_report.yml). Include:

- A short description of the actual result and the expected result.
- Exact steps, the affected button/panel, and whether you used preview or normal mode.
- Commit/version, Windows build, monitor count, resolutions, display scaling, and primary display.
- The relevant app name/version for browser, email, packaged-app, or immersive-window problems.
- Relevant diagnostics or a cropped screenshot if helpful. Remove unrelated personal information from logs and images.

For taskbar problems, describe its initial visibility, what happened while the shell ran, and whether it returned after exit. For scrolling or screen fitting, state which control became clipped, inaccessible, or misaligned. For phone/device reports, distinguish USB, Bluetooth, and Phone Link; these are different integrations.

`winshell.exe --diagnose` writes native-taskbar visibility and work-area geometry to `%APPDATA%\WinShellTheme\winshell.log`. Share only the lines needed to reproduce the problem.

## Implement a change

Keep pull requests focused. Separate a layout fix from an unrelated feature or refactor. Use the existing Rust/native rendering stack and explain any new dependency.

- Keep normal operation event-driven; avoid periodic polling or a continuous render loop.
- Save original taskbar, work-area, and window state before changing it. Preserve recovery files when restoration fails.
- Check Win32/COM results and keep errors visible to the user when an action fails.
- Scale layout and hit targets together. Keep panels, tooltips, sliders, and scrolling inside the intended monitor's bounds.
- Support keyboard focus and predictable dismissal. Do not hide an external-settings launch behind an unlabeled button.
- Keep the application in normal user mode. Avoid process injection, OS patching, and unnecessary system changes.
- Preserve user settings, pin order, and custom themes. Keep unrelated external apps and their preferences unchanged.

See [AGENT.md](AGENT.md) for additional architecture and recovery invariants. Do not recreate the removed chat log or historical plan; keep current public planning in the roadmap and issues.

## Validate your work

Run these checks after code changes:

```powershell
cargo fmt --all --check
cargo test --locked
cargo build --release --locked
```

Use `cargo clippy --all-targets --all-features --locked` for additional feedback. Some newer toolchains report an existing `chunks_exact_to_as_chunks` lint in pixel helpers; report existing warnings separately from warnings introduced by your change.

Add regression tests for changed behavior where they provide useful coverage. Pure parsing/model tests are valuable, but native taskbar recovery, display changes, and UI interaction also need manual Windows verification.

For UI changes, test the relevant cases from the [roadmap test matrix](ROADMAP.md#validation-matrix). State what you tested and what remains untested. Never mark a device, display setup, or recovery scenario as verified without checking it.

Documentation-only changes do not require a rebuild. Check spelling, relative links, and the accuracy of commands and feature claims.

## Open a pull request

Push your feature branch to your fork, then open a pull request targeting this repository's `main` branch. Include the problem, the resulting behavior, the linked issue if one exists, and validation evidence. Add before/after images for visible changes when possible.

Use a short, descriptive commit subject, such as `fix(panel): keep Wi-Fi actions inside the screen`. Use your own Git author identity. Maintainer review may request changes before merging; an open issue is not a promise that a feature will be accepted.

Keep discussion respectful and specific. Help others reproduce problems, and acknowledge testing and design contributions as well as code.

## Contact

Use GitHub issues for public bugs and feature discussions. For collaboration, contact Rahul at [rahul.dev.xai@gmail.com](mailto:rahul.dev.xai@gmail.com).

The project declares `MIT OR Apache-2.0` in `Cargo.toml`. Publishing the corresponding full license texts is a tracked community-setup task in the roadmap.
