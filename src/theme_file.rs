//! Theme files (PLAN.md sections 3 and 11). Pure: parsing, colour syntax,
//! built-in Dark Glass / Light Clean, and theme-name validation.

use serde::Deserialize;

/// Runtime palette (all colours 0xRRGGBBAA) plus geometry and motion.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Palette {
    pub glass_bg: u32,
    pub pill_bg: u32,
    pub pill_hover: u32,
    pub border: u32,
    pub text_primary: u32,
    pub text_secondary: u32,
    pub text_disabled: u32,
    /// Text drawn on top of an accent fill (today in the calendar, etc.).
    pub on_accent: u32,
    pub accent: u32,
    pub success: u32,
    pub warning: u32,
    pub danger: u32,
    pub running_dot: u32,
    pub start_glyph: u32,
    pub slider_track: u32,
    pub hover_overlay: u32,
    /// "dwm_round" | "dwm_small" | "none".
    pub window_corners: Corners,
    pub pill_radius: f32,
    pub icon_slot_radius: f32,
    pub motion_ms: f32,
    /// Dark DWM frame / acrylic tone.
    pub dark: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Corners {
    Round,
    Small,
    Square,
}

pub const DARK_GLASS: Palette = Palette {
    glass_bg: 0x14161CB7,
    pill_bg: 0x1E212A99,
    pill_hover: 0x2A2E3AB2,
    border: 0xFFFFFF1F,
    text_primary: 0xF2F4F8FF,
    text_secondary: 0xA8AEBBFF,
    text_disabled: 0x6A7080FF,
    on_accent: 0x14161CFF,
    accent: 0x6EA8FEFF,
    success: 0x4ADE80FF,
    warning: 0xFACC15FF,
    danger: 0xF87171FF,
    running_dot: 0xFFFFFF99,
    start_glyph: 0xF2F4F8E6,
    slider_track: 0xFFFFFF33,
    hover_overlay: 0xFFFFFF14,
    window_corners: Corners::Round,
    pill_radius: 14.0,
    icon_slot_radius: 12.0,
    motion_ms: 140.0,
    dark: true,
};

pub const LIGHT_CLEAN: Palette = Palette {
    glass_bg: 0xF5F6FAB2,
    pill_bg: 0xFFFFFFA6,
    pill_hover: 0xE8EBF2CC,
    border: 0x0000001A,
    text_primary: 0x14161CFF,
    text_secondary: 0x5B6170FF,
    text_disabled: 0x9BA1B0FF,
    on_accent: 0xFFFFFFFF,
    accent: 0x2563EBFF,
    success: 0x16A34AFF,
    warning: 0xCA8A04FF,
    danger: 0xDC2626FF,
    running_dot: 0x00000099,
    start_glyph: 0x14161CE6,
    slider_track: 0x00000026,
    hover_overlay: 0x0000000F,
    window_corners: Corners::Round,
    pill_radius: 14.0,
    icon_slot_radius: 12.0,
    motion_ms: 140.0,
    dark: false,
};

pub const DARK_GLASS_TOML: &str = r##"# Dark Glass: default WinShellTheme theme. Edit and save: changes apply live.
[theme.info]
name = "Dark Glass"
author = "WinShellTheme"
version = "1.0.0"

[colors]                         # "#RRGGBB" or "#RRGGBBAA"
glass_bg = "#14161CB7"           # tint over the blur
pill_surface = "#1E212A99"
hover_surface = "#2A2E3AB2"
border = "#FFFFFF1F"
text_primary = "#F2F4F8"
text_secondary = "#A8AEBB"
text_disabled = "#6A7080"
accent = "#6EA8FE"
success = "#4ADE80"
warning = "#FACC15"
danger = "#F87171"

[geometry]
window_corners = "dwm_round"     # "dwm_round" | "dwm_small" | "none"
pill_corner_radius = 14
icon_slot_radius = 12

[motion]
duration_ms = 140                # 120 to 180
"##;

pub const LIGHT_CLEAN_TOML: &str = r##"# Light Clean: light WinShellTheme theme. Edit and save: changes apply live.
[theme.info]
name = "Light Clean"
author = "WinShellTheme"
version = "1.0.0"

[colors]
glass_bg = "#F5F6FAB2"
pill_surface = "#FFFFFFA6"
hover_surface = "#E8EBF2CC"
border = "#0000001A"
text_primary = "#14161C"
text_secondary = "#5B6170"
text_disabled = "#9BA1B0"
accent = "#2563EB"
success = "#16A34A"
warning = "#CA8A04"
danger = "#DC2626"

[geometry]
window_corners = "dwm_round"
pill_corner_radius = 14
icon_slot_radius = 12

[motion]
duration_ms = 140
"##;

#[derive(Deserialize, Default)]
#[serde(default)]
struct Raw {
    colors: RawColors,
    geometry: RawGeometry,
    motion: RawMotion,
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct RawColors {
    glass_bg: Option<String>,
    pill_surface: Option<String>,
    hover_surface: Option<String>,
    border: Option<String>,
    text_primary: Option<String>,
    text_secondary: Option<String>,
    text_disabled: Option<String>,
    accent: Option<String>,
    success: Option<String>,
    warning: Option<String>,
    danger: Option<String>,
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct RawGeometry {
    window_corners: Option<String>,
    pill_corner_radius: Option<f32>,
    icon_slot_radius: Option<f32>,
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct RawMotion {
    duration_ms: Option<f32>,
}

/// "#RRGGBB" or "#RRGGBBAA" -> 0xRRGGBBAA.
pub fn parse_color(s: &str) -> Option<u32> {
    let h = s.trim().strip_prefix('#')?;
    if !h.chars().all(|c| c.is_ascii_hexdigit()) {
        return None;
    }
    match h.len() {
        6 => u32::from_str_radix(h, 16).ok().map(|v| (v << 8) | 0xFF),
        8 => u32::from_str_radix(h, 16).ok(),
        _ => None,
    }
}

fn luminance(rgba: u32) -> f32 {
    let c = |shift: u32| ((rgba >> shift) & 0xFF) as f32 / 255.0;
    0.2126 * c(24) + 0.7152 * c(16) + 0.0722 * c(8)
}

/// Parse a theme file over `base` (unknown or invalid values keep the base value).
/// Returns the palette and a list of human-readable problems.
pub fn parse(text: &str, base: Palette) -> (Palette, Vec<String>) {
    let mut problems = Vec::new();
    let raw: Raw = match toml::from_str(text) {
        Ok(r) => r,
        Err(e) => return (base, vec![e.to_string()]),
    };
    let mut p = base;
    let mut set = |slot: &mut u32, v: &Option<String>, key: &str| {
        if let Some(s) = v {
            match parse_color(s) {
                Some(c) => *slot = c,
                None => problems.push(format!("colors.{key}: \"{s}\" is not #RRGGBB or #RRGGBBAA")),
            }
        }
    };
    let c = &raw.colors;
    set(&mut p.glass_bg, &c.glass_bg, "glass_bg");
    set(&mut p.pill_bg, &c.pill_surface, "pill_surface");
    set(&mut p.pill_hover, &c.hover_surface, "hover_surface");
    set(&mut p.border, &c.border, "border");
    set(&mut p.text_primary, &c.text_primary, "text_primary");
    set(&mut p.text_secondary, &c.text_secondary, "text_secondary");
    set(&mut p.text_disabled, &c.text_disabled, "text_disabled");
    set(&mut p.accent, &c.accent, "accent");
    set(&mut p.success, &c.success, "success");
    set(&mut p.warning, &c.warning, "warning");
    set(&mut p.danger, &c.danger, "danger");

    if let Some(wc) = &raw.geometry.window_corners {
        match wc.as_str() {
            "dwm_round" => p.window_corners = Corners::Round,
            "dwm_small" => p.window_corners = Corners::Small,
            "none" => p.window_corners = Corners::Square,
            other => problems.push(format!("geometry.window_corners: unknown value \"{other}\"")),
        }
    }
    if let Some(r) = raw.geometry.pill_corner_radius {
        p.pill_radius = r.clamp(0.0, 14.0);
    }
    if let Some(r) = raw.geometry.icon_slot_radius {
        p.icon_slot_radius = r.clamp(0.0, 24.0);
    }
    if let Some(ms) = raw.motion.duration_ms {
        p.motion_ms = ms.clamp(0.0, 1000.0);
    }

    // Derived values follow the (possibly edited) base colours.
    p.dark = luminance(p.glass_bg) < 0.5;
    let ink = if p.dark { 0xFFFFFF00 } else { 0x00000000 };
    p.running_dot = ink | 0x99;
    p.slider_track = ink | if p.dark { 0x33 } else { 0x26 };
    p.hover_overlay = ink | if p.dark { 0x14 } else { 0x0F };
    p.start_glyph = (p.text_primary & 0xFFFFFF00) | 0xE6;
    p.on_accent = if luminance(p.accent) > 0.55 { 0x14161CFF } else { 0xFFFFFFFF };
    (p, problems)
}

/// Theme names map to file names, so only `[A-Za-z0-9_-]` is accepted
/// (no path separators, dots or drive letters: no path traversal).
pub fn valid_theme_name(name: &str) -> bool {
    !name.is_empty() && name.len() <= 64 && name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn colour_syntax() {
        assert_eq!(parse_color("#14161C"), Some(0x14161CFF));
        assert_eq!(parse_color("#14161CB7"), Some(0x14161CB7));
        assert_eq!(parse_color("14161C"), None);
        assert_eq!(parse_color("#12345"), None);
        assert_eq!(parse_color("#GG0000"), None);
        assert_eq!(parse_color("#+12345"), None);
    }

    #[test]
    fn builtin_files_match_builtin_palettes() {
        let (d, pd) = parse(DARK_GLASS_TOML, LIGHT_CLEAN);
        assert!(pd.is_empty(), "{pd:?}");
        assert_eq!(d.glass_bg, DARK_GLASS.glass_bg);
        assert!(d.dark);
        let (l, pl) = parse(LIGHT_CLEAN_TOML, DARK_GLASS);
        assert!(pl.is_empty(), "{pl:?}");
        assert_eq!(l.text_primary, LIGHT_CLEAN.text_primary);
        assert!(!l.dark);
    }

    #[test]
    fn bad_values_keep_base_and_report() {
        let (p, problems) = parse("[colors]\naccent = \"red\"\n[geometry]\nwindow_corners = \"bubbly\"\n", DARK_GLASS);
        assert_eq!(p.accent, DARK_GLASS.accent);
        assert_eq!(problems.len(), 2);
    }

    #[test]
    fn broken_toml_returns_base() {
        let (p, problems) = parse("[colors\n", DARK_GLASS);
        assert_eq!(p, DARK_GLASS);
        assert_eq!(problems.len(), 1);
    }

    #[test]
    fn numbers_are_clamped() {
        let (p, _) = parse("[geometry]\npill_corner_radius = 900\n[motion]\nduration_ms = -5\n", DARK_GLASS);
        assert_eq!(p.pill_radius, 14.0);
        assert_eq!(p.motion_ms, 0.0);
    }

    #[test]
    fn theme_names_cannot_escape_the_folder() {
        assert!(valid_theme_name("dark_glass"));
        assert!(valid_theme_name("My-Theme2"));
        for bad in ["", "..", "../x", "a/b", "a\\b", "C:x", "x.toml", "a b", &"x".repeat(65)] {
            assert!(!valid_theme_name(bad), "{bad}");
        }
    }
}
