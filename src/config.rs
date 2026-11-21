//! `config.toml` (PLAN.md section 11). Pure: parsing, defaults and the
//! format-preserving pin/unpin edit. Live reload arrives in M8.

use serde::Deserialize;
use std::path::Path;

#[derive(Clone, Debug, Default, Deserialize, PartialEq)]
#[serde(default)]
pub struct Config {
    pub general: General,
    pub top_bar: TopBar,
    pub dock: Dock,
    pub taskbar: Taskbar,
}

#[derive(Clone, Debug, Default, Deserialize, PartialEq)]
#[serde(default)]
pub struct General {
    /// Shown in the top bar's user pill. Empty = "@" + Windows user name.
    pub user_handle: String,
    /// "dark_glass", "light_clean", any file name in themes\ (without .toml), or "auto_system".
    pub theme: String,
    /// Follow Windows light/dark mode (picks dark_glass or light_clean).
    pub follow_windows_theme: bool,
}

#[derive(Clone, Debug, Deserialize, PartialEq)]
#[serde(default)]
pub struct TopBar {
    pub enabled_pills: Vec<String>,
    /// Hide when an app is maximised or overlaps the bar; push the mouse to the top
    /// edge to bring it back. Apps then get the full screen (no reserved space).
    pub autohide: bool,
}

impl Default for TopBar {
    fn default() -> Self {
        Self {
            enabled_pills: ["bluetooth", "wifi", "volume", "battery", "clock", "notifications", "quick_settings"]
                .map(String::from)
                .to_vec(),
            autohide: true,
        }
    }
}

impl TopBar {
    pub fn pill_enabled(&self, name: &str) -> bool {
        self.enabled_pills.iter().any(|p| p.eq_ignore_ascii_case(name))
    }
}

#[derive(Clone, Debug, Deserialize, PartialEq)]
#[serde(default)]
pub struct Dock {
    /// Hovered icon drawn at 1.15x.
    pub magnification: bool,
    /// Exe paths (absolute, or a bare name found on PATH) or AUMIDs.
    pub pinned_apps: Vec<String>,
    /// Hide when an app is maximised or overlaps the dock; push the mouse to the
    /// bottom edge to bring it back. Apps then get the full screen.
    pub autohide: bool,
}

impl Default for Dock {
    fn default() -> Self {
        Self { magnification: false, pinned_apps: Vec::new(), autohide: true }
    }
}

#[derive(Clone, Debug, Deserialize, PartialEq)]
#[serde(default)]
pub struct Taskbar {
    /// Switch the native taskbar to autohide while running (restored on exit).
    pub autohide_native: bool,
}

impl Default for Taskbar {
    fn default() -> Self {
        Self { autohide_native: true }
    }
}

/// Written on first run. Comments survive pin/unpin edits.
pub const DEFAULT_CONFIG: &str = r#"# WinShellTheme configuration
# Saved changes apply live (themes too: see the themes folder next to this file).

[general]
user_handle = ""                 # empty = "@" + your Windows user name
theme = "dark_glass"             # "dark_glass", "light_clean", a file in themes\ (no .toml), or "auto_system"
follow_windows_theme = false     # true = follow Windows light/dark mode

[top_bar]
enabled_pills = ["bluetooth", "wifi", "volume", "battery", "clock", "notifications", "quick_settings"]
autohide = true                  # hide over maximised apps; mouse to the top edge shows it

[dock]
magnification = false            # draw the hovered icon at 1.15x
autohide = true                  # hide over maximised apps; mouse to the bottom edge shows it
# Exe paths, bare exe names on PATH, or AUMIDs (packaged apps).
# Right-click a dock icon > Pin to Dock to add entries here.
pinned_apps = [
    "explorer.exe",
]

[taskbar]
autohide_native = true           # switch the native taskbar to autohide while running
"#;

pub fn parse(text: &str) -> Result<Config, String> {
    toml::from_str(text).map_err(|e| e.to_string())
}

/// Load `path`, creating it with [`DEFAULT_CONFIG`] if missing. A file that
/// fails to parse yields defaults plus the error, so a typo never stops the shell.
pub fn load_or_create(path: &Path) -> (Config, Option<String>) {
    match std::fs::read_to_string(path) {
        Ok(text) => match parse(&text) {
            Ok(c) => (c, None),
            Err(e) => (Config::default(), Some(format!("{}: {e}", path.display()))),
        },
        Err(_) => {
            let _ = std::fs::write(path, DEFAULT_CONFIG);
            (parse(DEFAULT_CONFIG).unwrap_or_default(), None)
        }
    }
}

/// Add (`pin = true`) or remove `entry` in `[dock] pinned_apps`, preserving
/// comments, key order and formatting. Matching is case-insensitive.
pub fn set_pinned(text: &str, entry: &str, pin: bool) -> Result<String, String> {
    use toml_edit::{Array, DocumentMut, Item, Table, Value};
    let mut doc: DocumentMut = text.parse().map_err(|e: toml_edit::TomlError| e.to_string())?;
    if !doc.contains_key("dock") {
        doc.insert("dock", Item::Table(Table::new()));
    }
    let dock = doc["dock"].as_table_like_mut().ok_or("[dock] is not a table")?;
    if dock.get("pinned_apps").is_none() {
        dock.insert("pinned_apps", Item::Value(Value::Array(Array::new())));
    }
    let arr = dock.get_mut("pinned_apps").and_then(|i| i.as_array_mut()).ok_or("dock.pinned_apps is not an array")?;
    let pos = arr.iter().position(|v| v.as_str().is_some_and(|s| s.eq_ignore_ascii_case(entry)));
    match (pin, pos) {
        (true, None) => {
            // Match the layout of the existing entries (one per line or inline).
            let multiline = arr
                .iter()
                .next()
                .is_some_and(|v| v.decor().prefix().and_then(|p| p.as_str()).is_some_and(|p| p.contains('\n')));
            arr.push(entry);
            if multiline && let Some(last) = arr.iter_mut().last() {
                last.decor_mut().set_prefix("\n    ");
            }
            if multiline {
                arr.set_trailing(",\n");
                arr.set_trailing_comma(false);
            }
        }
        (false, Some(i)) => {
            arr.remove(i);
        }
        _ => {}
    }
    Ok(doc.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_config_parses_with_expected_values() {
        let c = parse(DEFAULT_CONFIG).unwrap();
        assert_eq!(c.dock.pinned_apps, ["explorer.exe"]);
        assert!(c.taskbar.autohide_native);
        assert!(!c.dock.magnification);
        assert!(c.top_bar.pill_enabled("WiFi"));
        assert!(c.top_bar.autohide && c.dock.autohide);
    }

    #[test]
    fn empty_file_gives_defaults() {
        assert_eq!(parse("").unwrap(), Config::default());
    }

    #[test]
    fn unknown_keys_are_ignored() {
        let c = parse("[general]\ntheme = \"dark_glass\"\n[monitors]\nbars_on_all_monitors = true\n").unwrap();
        assert_eq!(c.general.user_handle, "");
    }

    #[test]
    fn pin_preserves_comments_and_appends() {
        let out = set_pinned(DEFAULT_CONFIG, "C:\\Apps\\code.exe", true).unwrap();
        assert!(out.contains("# Right-click a dock icon"));
        assert!(out.contains("autohide_native = true           # switch"));
        let c = parse(&out).unwrap();
        assert_eq!(c.dock.pinned_apps, ["explorer.exe", "C:\\Apps\\code.exe"]);
    }

    #[test]
    fn pin_is_idempotent_and_case_insensitive() {
        let once = set_pinned(DEFAULT_CONFIG, "EXPLORER.EXE", true).unwrap();
        assert_eq!(parse(&once).unwrap().dock.pinned_apps, ["explorer.exe"]);
    }

    #[test]
    fn unpin_removes_entry() {
        let out = set_pinned(DEFAULT_CONFIG, "Explorer.exe", false).unwrap();
        assert!(parse(&out).unwrap().dock.pinned_apps.is_empty());
        assert!(out.contains("[taskbar]"));
    }

    #[test]
    fn pin_creates_missing_section() {
        let out = set_pinned("[general]\nuser_handle = \"@me\"\n", "a!b", true).unwrap();
        let c = parse(&out).unwrap();
        assert_eq!(c.dock.pinned_apps, ["a!b"]);
        assert_eq!(c.general.user_handle, "@me");
    }
}
