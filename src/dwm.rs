//! Windows 11 DWM backdrop and corner helpers (PLAN.md sections 6 and 7).

use windows::Win32::Foundation::HWND;
use windows::Win32::Graphics::Dwm::*;
use windows::Win32::UI::Controls::MARGINS;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Backdrop {
    None,
    /// Transient acrylic, the plan's choice (stays translucent when inactive).
    Acrylic,
    /// Mica, for comparison in the spike (expected to grey out on inactive windows).
    Mica,
    /// Tabbed / Mica Alt, for comparison only.
    Tabbed,
}

impl Backdrop {
    pub const ALL: [Backdrop; 4] = [Backdrop::Acrylic, Backdrop::Mica, Backdrop::Tabbed, Backdrop::None];
    pub fn name(self) -> &'static str {
        match self {
            Backdrop::None => "none",
            Backdrop::Acrylic => "acrylic",
            Backdrop::Mica => "mica",
            Backdrop::Tabbed => "tabbed",
        }
    }
    pub fn parse(s: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|b| b.name() == s)
    }
    fn raw(self) -> DWM_SYSTEMBACKDROP_TYPE {
        match self {
            Backdrop::None => DWMSBT_NONE,
            Backdrop::Acrylic => DWMSBT_TRANSIENTWINDOW,
            Backdrop::Mica => DWMSBT_MAINWINDOW,
            Backdrop::Tabbed => DWMSBT_TABBEDWINDOW,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Corners {
    Round,
    Small,
    Square,
}

impl Corners {
    pub const ALL: [Corners; 3] = [Corners::Round, Corners::Small, Corners::Square];
    pub fn name(self) -> &'static str {
        match self {
            Corners::Round => "dwm_round",
            Corners::Small => "dwm_small",
            Corners::Square => "none",
        }
    }
    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "round" | "dwm_round" => Some(Corners::Round),
            "small" | "dwm_small" => Some(Corners::Small),
            "none" | "square" => Some(Corners::Square),
            _ => None,
        }
    }
    /// Approximate DWM outline radius at 100% scale, used to match D2D-drawn tint shapes.
    pub fn approx_radius(self) -> f32 {
        match self {
            Corners::Round => 8.0,
            Corners::Small => 4.0,
            Corners::Square => 0.0,
        }
    }
    fn raw(self) -> DWM_WINDOW_CORNER_PREFERENCE {
        match self {
            Corners::Round => DWMWCP_ROUND,
            Corners::Small => DWMWCP_ROUNDSMALL,
            Corners::Square => DWMWCP_DONOTROUND,
        }
    }
}

#[derive(Clone, Copy, Debug)]
pub struct DwmStyle {
    pub backdrop: Backdrop,
    pub corners: Corners,
    pub extend_frame: bool,
    pub dark: bool,
}

impl Default for DwmStyle {
    fn default() -> Self {
        Self { backdrop: Backdrop::Acrylic, corners: Corners::Round, extend_frame: true, dark: true }
    }
}

/// Per-attribute HRESULT outcome, so the spike can record exactly what DWM accepted.
#[derive(Debug, Default)]
pub struct DwmApplyReport {
    pub dark: Option<String>,
    pub extend: Option<String>,
    pub backdrop: Option<String>,
    pub corners: Option<String>,
}

impl DwmApplyReport {
    pub fn all_ok(&self) -> bool {
        [&self.dark, &self.extend, &self.backdrop, &self.corners].iter().all(|r| r.is_none())
    }
    pub fn describe(&self) -> String {
        let f = |r: &Option<String>| r.clone().unwrap_or_else(|| "ok".into());
        format!(
            "dark={} extend={} backdrop={} corners={}",
            f(&self.dark),
            f(&self.extend),
            f(&self.backdrop),
            f(&self.corners)
        )
    }
}

pub fn apply(hwnd: HWND, style: DwmStyle) -> DwmApplyReport {
    let mut rep = DwmApplyReport::default();
    let err = |r: windows::core::Result<()>| r.err().map(|e| format!("{:#010x}", e.code().0));
    unsafe {
        let dark: i32 = style.dark as i32;
        rep.dark = err(DwmSetWindowAttribute(hwnd, DWMWA_USE_IMMERSIVE_DARK_MODE, &dark as *const _ as _, 4));

        let m = if style.extend_frame {
            MARGINS { cxLeftWidth: -1, cxRightWidth: -1, cyTopHeight: -1, cyBottomHeight: -1 }
        } else {
            MARGINS::default()
        };
        rep.extend = err(DwmExtendFrameIntoClientArea(hwnd, &m));

        let b = style.backdrop.raw();
        rep.backdrop = err(DwmSetWindowAttribute(hwnd, DWMWA_SYSTEMBACKDROP_TYPE, &b as *const _ as _, 4));

        let c = style.corners.raw();
        rep.corners = err(DwmSetWindowAttribute(hwnd, DWMWA_WINDOW_CORNER_PREFERENCE, &c as *const _ as _, 4));
    }
    rep
}

/// Hide a window from Alt+Tab / Win+Tab peek and live previews (reservation windows).
pub fn exclude_from_peek(hwnd: HWND) {
    unsafe {
        let on: i32 = 1;
        let _ = DwmSetWindowAttribute(hwnd, DWMWA_EXCLUDED_FROM_PEEK, &on as *const _ as _, 4);
    }
}
