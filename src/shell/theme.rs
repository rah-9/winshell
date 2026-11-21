//! Theme tokens and layout constants (PLAN.md sections 2 and 3, dark theme at
//! 100% scale). Themes become configurable in M8.

// Colours, 0xRRGGBBAA.
pub const GLASS_BG: u32 = 0x14161CB7;
pub const PILL_BG: u32 = 0x1E212A99;
pub const PILL_HOVER: u32 = 0x2A2E3AB2;
pub const BORDER: u32 = 0xFFFFFF1F;
pub const TEXT_PRIMARY: u32 = 0xF2F4F8FF;
pub const TEXT_SECONDARY: u32 = 0xA8AEBBFF;
pub const TEXT_DISABLED: u32 = 0x6A7080FF;
pub const ACCENT: u32 = 0x6EA8FEFF;
pub const SUCCESS: u32 = 0x4ADE80FF;
pub const WARNING: u32 = 0xFACC15FF;
pub const DANGER: u32 = 0xF87171FF;
pub const RUNNING_DOT_COLOR: u32 = 0xFFFFFF99;
pub const START_GLYPH_COLOR: u32 = 0xF2F4F8E6;
pub const SLIDER_TRACK: u32 = 0xFFFFFF33;

// Reservations and top bar.
pub const TOP_RESERVE: f32 = 44.0;
pub const BOTTOM_RESERVE: f32 = 74.0;
pub const BAR_MARGIN: f32 = 8.0;
pub const BAR_HEIGHT: f32 = 36.0;
pub const BAR_EDGE_INSET: f32 = 4.0;
pub const PILL_HEIGHT: f32 = 28.0;
pub const PILL_RADIUS: f32 = 14.0;
pub const PILL_PAD_X: f32 = 10.0;
pub const PILL_GAP: f32 = 6.0;
pub const ICON_PILL_W: f32 = 28.0;
pub const DIVIDER_HEIGHT: f32 = 16.0;
pub const ACTIVE_ICON: f32 = 16.0;
pub const ICON_TEXT_GAP: f32 = 6.0;
pub const TITLE_MAX_CHARS: usize = 40;

// Dock.
pub const DOCK_HEIGHT: f32 = 64.0;
pub const DOCK_MARGIN_BOTTOM: f32 = 10.0;
pub const DOCK_PAD_X: f32 = 10.0;
pub const DOCK_SLOT: f32 = 48.0;
pub const DOCK_SLOT_GAP: f32 = 4.0;
pub const DOCK_SLOT_RADIUS: f32 = 12.0;
pub const DOCK_SEPARATOR_HEIGHT: f32 = 32.0;
pub const DOCK_SEPARATOR_MARGIN: f32 = 6.0;
pub const DOCK_ICON: f32 = 44.0;
pub const DOCK_HOVER_SCALE: f32 = 1.15;
pub const DOCK_ICON_FETCH_MIN: u32 = 64;
pub const ICON_CACHE_BYTES: usize = 8 * 1024 * 1024;
pub const INDICATOR_GAP: f32 = 2.0;
pub const RUNNING_DOT: f32 = 4.0;
pub const MULTI_DOT: f32 = 3.0;
pub const FOCUS_PILL_W: f32 = 12.0;
pub const FOCUS_PILL_H: f32 = 4.0;
pub const MEDIA_PILL_H: f32 = 48.0;
pub const MEDIA_ART: f32 = 36.0;
pub const MEDIA_TEXT_W: f32 = 112.0;
pub const MEDIA_BUTTON: f32 = 28.0;

// Tooltip.
pub const TOOLTIP_H: f32 = 28.0;
pub const TOOLTIP_PAD_X: f32 = 12.0;
pub const TOOLTIP_GAP: f32 = 6.0;

// Flyout.
pub const FLYOUT_W: f32 = 340.0;
pub const FLYOUT_PAD: f32 = 14.0;
pub const FLYOUT_GAP: f32 = 8.0;
pub const FLYOUT_SLIDE: f32 = 8.0;
pub const MOTION_MS: f32 = 140.0;
pub const WINDOW_LIST_W: f32 = 320.0;

// Segoe Fluent Icons / Segoe MDL2 Assets code points.
pub mod glyph {
    pub const BLUETOOTH: char = '\u{E702}';
    pub const WIFI: char = '\u{E701}';
    pub const WIFI_1: char = '\u{E872}';
    pub const WIFI_2: char = '\u{E873}';
    pub const WIFI_3: char = '\u{E874}';
    pub const ETHERNET: char = '\u{E839}';
    pub const VOLUME_0: char = '\u{E992}';
    pub const VOLUME_1: char = '\u{E993}';
    pub const VOLUME_2: char = '\u{E994}';
    pub const VOLUME_3: char = '\u{E995}';
    pub const MUTE: char = '\u{E74F}';
    pub const MIC: char = '\u{E720}';
    pub const BELL: char = '\u{EA8F}';
    pub const SETTINGS: char = '\u{E713}';
    pub const PLAY: char = '\u{E768}';
    pub const PAUSE: char = '\u{E769}';
    pub const PREVIOUS: char = '\u{E892}';
    pub const NEXT: char = '\u{E893}';
    pub const CHEVRON_LEFT: char = '\u{E76B}';
    pub const CHEVRON_RIGHT: char = '\u{E76C}';
    pub const ADMIN: char = '\u{E7EF}';
    pub const MUSIC: char = '\u{E8D6}';
    pub const BATTERY_FULL: char = '\u{E83F}';
    pub const BATTERY_0: u32 = 0xE850;
    pub const BATTERY_CHARGING_FULL: char = '\u{E83E}';
    pub const BATTERY_CHARGING_0: u32 = 0xE85A;
    pub const BATTERY_SAVER: char = '\u{E863}';
}
