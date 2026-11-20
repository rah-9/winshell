//! Shared rendering devices and per-window DirectComposition surfaces
//! (PLAN.md section 6, "Rendering Model").
//!
//! Exactly one D3D11 device, one D2D factory/device and one DComp device per
//! process. Each window gets its own composition target + root visual +
//! surface. Drawing happens only when the caller asks (no render loop).

use windows::Win32::Foundation::{HMODULE, HWND, POINT};
use windows::Win32::Graphics::Direct2D::Common::*;
use windows::Win32::Graphics::Direct2D::*;
use windows::Win32::Graphics::Direct3D::*;
use windows::Win32::Graphics::Direct3D11::*;
use windows::Win32::Graphics::DirectComposition::*;
use windows::Win32::Graphics::DirectWrite::*;
use windows::Win32::Graphics::Dxgi::Common::*;
use windows::Win32::Graphics::Dxgi::*;
use windows::core::{Interface, PCWSTR, Result, w};
use windows_numerics::Matrix3x2;

pub struct Gfx {
    pub d3d: ID3D11Device,
    pub d2d_factory: ID2D1Factory1,
    pub d2d_device: ID2D1Device,
    pub dcomp: IDCompositionDevice,
    pub dwrite: IDWriteFactory,
    pub driver: &'static str,
}

impl Gfx {
    pub fn new() -> Result<Self> {
        let (d3d, driver) = match create_d3d(D3D_DRIVER_TYPE_HARDWARE) {
            Ok(d) => (d, "hardware"),
            Err(_) => (create_d3d(D3D_DRIVER_TYPE_WARP)?, "warp"),
        };
        unsafe {
            let dxgi: IDXGIDevice = d3d.cast()?;
            let opts = D2D1_FACTORY_OPTIONS { debugLevel: D2D1_DEBUG_LEVEL_NONE };
            let d2d_factory: ID2D1Factory1 = D2D1CreateFactory(D2D1_FACTORY_TYPE_SINGLE_THREADED, Some(&opts))?;
            let d2d_device = d2d_factory.CreateDevice(&dxgi)?;
            let dcomp: IDCompositionDevice = DCompositionCreateDevice2(&d2d_device)?;
            let dwrite: IDWriteFactory = DWriteCreateFactory(DWRITE_FACTORY_TYPE_SHARED)?;
            Ok(Self { d3d, d2d_factory, d2d_device, dcomp, dwrite, driver })
        }
    }

    /// True if the D3D device has been removed/reset and everything must be rebuilt.
    pub fn is_lost(&self) -> bool {
        unsafe { self.d3d.GetDeviceRemovedReason().is_err() }
    }

    pub fn commit(&self) -> Result<()> {
        unsafe { self.dcomp.Commit() }
    }

    pub fn text_format(&self, family: PCWSTR, size: f32, weight: DWRITE_FONT_WEIGHT) -> Result<IDWriteTextFormat> {
        unsafe {
            let f = self.dwrite.CreateTextFormat(
                family,
                None,
                weight,
                DWRITE_FONT_STYLE_NORMAL,
                DWRITE_FONT_STRETCH_NORMAL,
                size,
                w!("en-us"),
            )?;
            f.SetWordWrapping(DWRITE_WORD_WRAPPING_NO_WRAP)?;
            f.SetParagraphAlignment(DWRITE_PARAGRAPH_ALIGNMENT_CENTER)?;
            Ok(f)
        }
    }
}

fn create_d3d(kind: D3D_DRIVER_TYPE) -> Result<ID3D11Device> {
    let levels = [D3D_FEATURE_LEVEL_11_1, D3D_FEATURE_LEVEL_11_0, D3D_FEATURE_LEVEL_10_1, D3D_FEATURE_LEVEL_10_0];
    let mut device = None;
    unsafe {
        D3D11CreateDevice(
            None,
            kind,
            HMODULE::default(),
            D3D11_CREATE_DEVICE_BGRA_SUPPORT,
            Some(&levels),
            D3D11_SDK_VERSION,
            Some(&mut device),
            None,
            None,
        )?;
    }
    device.ok_or_else(|| windows::core::Error::from_hresult(windows::Win32::Foundation::E_FAIL))
}

/// One window's composition tree: target -> root visual -> surface.
pub struct WindowSurface {
    _target: IDCompositionTarget,
    visual: IDCompositionVisual,
    surface: Option<IDCompositionSurface>,
    pub width: u32,
    pub height: u32,
}

impl WindowSurface {
    pub fn new(gfx: &Gfx, hwnd: HWND) -> Result<Self> {
        unsafe {
            let target = gfx.dcomp.CreateTargetForHwnd(hwnd, true)?;
            let visual = gfx.dcomp.CreateVisual()?;
            target.SetRoot(&visual)?;
            Ok(Self { _target: target, visual, surface: None, width: 0, height: 0 })
        }
    }

    /// (Re)create the backing surface when the window size changes.
    pub fn resize(&mut self, gfx: &Gfx, width: u32, height: u32) -> Result<()> {
        if self.surface.is_some() && width == self.width && height == self.height {
            return Ok(());
        }
        let (w, h) = (width.max(1), height.max(1));
        unsafe {
            let s = gfx.dcomp.CreateSurface(w, h, DXGI_FORMAT_B8G8R8A8_UNORM, DXGI_ALPHA_MODE_PREMULTIPLIED)?;
            self.visual.SetContent(&s)?;
            self.surface = Some(s);
        }
        self.width = w;
        self.height = h;
        Ok(())
    }

    /// Redraw the whole surface. The closure draws in surface pixel coordinates
    /// starting at (0, 0); the update offset DComp hands back is applied for it.
    pub fn draw(&self, f: impl FnOnce(&ID2D1DeviceContext) -> Result<()>) -> Result<()> {
        let Some(surface) = &self.surface else { return Ok(()) };
        unsafe {
            let mut offset = POINT::default();
            let dc: ID2D1DeviceContext = surface.BeginDraw(None, &mut offset)?;
            dc.SetDpi(96.0, 96.0);
            dc.SetTransform(&Matrix3x2::translation(offset.x as f32, offset.y as f32));
            dc.Clear(Some(&D2D1_COLOR_F { r: 0.0, g: 0.0, b: 0.0, a: 0.0 }));
            let drawn = f(&dc);
            let ended = surface.EndDraw();
            drawn?;
            ended
        }
    }
}

// ---------------------------------------------------------------------------
// Small drawing helpers
// ---------------------------------------------------------------------------

/// `0xRRGGBBAA` -> D2D colour (straight alpha; D2D premultiplies).
pub fn rgba(c: u32) -> D2D1_COLOR_F {
    D2D1_COLOR_F {
        r: ((c >> 24) & 0xFF) as f32 / 255.0,
        g: ((c >> 16) & 0xFF) as f32 / 255.0,
        b: ((c >> 8) & 0xFF) as f32 / 255.0,
        a: (c & 0xFF) as f32 / 255.0,
    }
}

pub fn rectf(x: f32, y: f32, w: f32, h: f32) -> D2D_RECT_F {
    D2D_RECT_F { left: x, top: y, right: x + w, bottom: y + h }
}

pub fn brush(dc: &ID2D1DeviceContext, c: u32) -> Result<ID2D1SolidColorBrush> {
    unsafe { dc.CreateSolidColorBrush(&rgba(c), None) }
}

pub fn fill_round(dc: &ID2D1DeviceContext, r: D2D_RECT_F, radius: f32, c: u32) -> Result<()> {
    let b = brush(dc, c)?;
    unsafe {
        if radius <= 0.0 {
            dc.FillRectangle(&r, &b);
        } else {
            dc.FillRoundedRectangle(&D2D1_ROUNDED_RECT { rect: r, radiusX: radius, radiusY: radius }, &b);
        }
    }
    Ok(())
}

/// 1 px (device) border drawn inside `r`.
pub fn stroke_round(dc: &ID2D1DeviceContext, r: D2D_RECT_F, radius: f32, c: u32) -> Result<()> {
    let b = brush(dc, c)?;
    let inset = D2D_RECT_F { left: r.left + 0.5, top: r.top + 0.5, right: r.right - 0.5, bottom: r.bottom - 0.5 };
    unsafe {
        let rr = D2D1_ROUNDED_RECT { rect: inset, radiusX: radius, radiusY: radius };
        dc.DrawRoundedRectangle(&rr, &b, 1.0, None);
    }
    Ok(())
}

pub fn text(dc: &ID2D1DeviceContext, s: &str, fmt: &IDWriteTextFormat, r: D2D_RECT_F, c: u32) -> Result<()> {
    let b = brush(dc, c)?;
    let w: Vec<u16> = s.encode_utf16().collect();
    unsafe {
        dc.DrawText(&w, fmt, &r, &b, D2D1_DRAW_TEXT_OPTIONS_CLIP, DWRITE_MEASURING_MODE_NATURAL);
    }
    Ok(())
}
