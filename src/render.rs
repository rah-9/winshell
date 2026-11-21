//! Shared rendering devices and per-window DirectComposition surfaces
//! (PLAN.md section 6, "Rendering Model").
//!
//! Exactly one D3D11 device, one D2D factory/device and one DComp device per
//! process. Each window gets its own composition target + root visual +
//! surface. Drawing happens only when the caller asks (no render loop).

use windows::Win32::Foundation::{HMODULE, HWND, POINT, RECT};
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
    /// Device context used only to create device-dependent resources (bitmaps)
    /// outside a BeginDraw; resources are shared by every context of `d2d_device`.
    pub resource_dc: ID2D1DeviceContext,
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
            let resource_dc = d2d_device.CreateDeviceContext(D2D1_DEVICE_CONTEXT_OPTIONS_NONE)?;
            Ok(Self { d3d, d2d_factory, d2d_device, dcomp, dwrite, resource_dc, driver })
        }
    }

    /// True if the D3D device has been removed/reset and everything must be rebuilt.
    pub fn is_lost(&self) -> bool {
        unsafe { self.d3d.GetDeviceRemovedReason().is_err() }
    }

    pub fn commit(&self) -> Result<()> {
        unsafe { self.dcomp.Commit() }
    }

    /// Premultiplied BGRA pixels -> D2D bitmap on the shared device.
    pub fn bitmap_from_bgra(&self, width: u32, height: u32, pixels: &[u8]) -> Result<ID2D1Bitmap1> {
        let props = D2D1_BITMAP_PROPERTIES1 {
            pixelFormat: D2D1_PIXEL_FORMAT {
                format: DXGI_FORMAT_B8G8R8A8_UNORM,
                alphaMode: D2D1_ALPHA_MODE_PREMULTIPLIED,
            },
            dpiX: 96.0,
            dpiY: 96.0,
            bitmapOptions: D2D1_BITMAP_OPTIONS_NONE,
            colorContext: std::mem::ManuallyDrop::new(None),
        };
        unsafe {
            self.resource_dc.CreateBitmap(
                D2D_SIZE_U { width, height },
                Some(pixels.as_ptr() as *const _),
                width * 4,
                &props,
            )
        }
    }

    /// Width in DIPs (device pixels here, DPI is fixed at 96) of `s` laid out on one line.
    pub fn measure(&self, s: &str, fmt: &IDWriteTextFormat) -> f32 {
        let w: Vec<u16> = s.encode_utf16().collect();
        unsafe {
            let Ok(layout) = self.dwrite.CreateTextLayout(&w, fmt, 100_000.0, 1_000.0) else { return 0.0 };
            let mut m = DWRITE_TEXT_METRICS::default();
            if layout.GetMetrics(&mut m).is_err() {
                return 0.0;
            }
            m.widthIncludingTrailingWhitespace
        }
    }

    /// Text format that trims overflow with "…" instead of clipping mid-glyph.
    pub fn text_format_ellipsis(
        &self,
        family: PCWSTR,
        size: f32,
        weight: DWRITE_FONT_WEIGHT,
    ) -> Result<IDWriteTextFormat> {
        let f = self.text_format(family, size, weight)?;
        unsafe {
            let sign = self.dwrite.CreateEllipsisTrimmingSign(&f)?;
            let trimming =
                DWRITE_TRIMMING { granularity: DWRITE_TRIMMING_GRANULARITY_CHARACTER, delimiter: 0, delimiterCount: 0 };
            f.SetTrimming(&trimming, &sign)?;
        }
        Ok(f)
    }

    /// True if `family` is installed (e.g. Segoe Fluent Icons vs. MDL2 Assets).
    pub fn font_exists(&self, family: &str) -> bool {
        unsafe {
            let mut coll = None;
            if self.dwrite.GetSystemFontCollection(&mut coll, false).is_err() {
                return false;
            }
            let Some(coll) = coll else { return false };
            let (mut index, mut exists) = (0u32, windows::core::BOOL(0));
            coll.FindFamilyName(&windows::core::HSTRING::from(family), &mut index, &mut exists).is_ok()
                && exists.as_bool()
        }
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

    /// Compositor-driven open animation (PLAN.md section 2): content slides
    /// `from_y` px to 0 and fades 0 -> 1 over `ms` with a cubic ease-out. No
    /// CPU frames: DirectComposition evaluates the curves itself.
    pub fn animate_open(&self, gfx: &Gfx, from_y: f32, ms: f32) -> Result<()> {
        let d = (ms / 1000.0) as f64;
        // value(t) = a + delta * (3u - 3u^2 + u^3), u = t / d  (1 - (1 - u)^3)
        let curve = |from: f32, to: f32| -> Result<IDCompositionAnimation> {
            let delta = to - from;
            let df = d as f32;
            unsafe {
                let a = gfx.dcomp.CreateAnimation()?;
                a.AddCubic(0.0, from, 3.0 * delta / df, -3.0 * delta / (df * df), delta / (df * df * df))?;
                a.End(d, to)?;
                Ok(a)
            }
        };
        unsafe {
            let offset = curve(from_y, 0.0)?;
            self.visual.SetOffsetY(&offset)?;
            let fade = curve(0.0, 1.0)?;
            let effect = gfx.dcomp.CreateEffectGroup()?;
            effect.SetOpacity(&fade)?;
            self.visual.SetEffect(&effect)?;
        }
        Ok(())
    }

    /// Redraw the whole surface. The closure draws in surface pixel coordinates
    /// starting at (0, 0); the update offset DComp hands back is applied for it.
    pub fn draw(&self, f: impl FnOnce(&ID2D1DeviceContext) -> Result<()>) -> Result<()> {
        self.draw_region(None, f)
    }

    /// Redraw only `dirty` (surface pixel coordinates; `None` = whole surface).
    /// The closure still draws in whole-surface coordinates; output is clipped
    /// to `dirty`, and pixels outside it keep their previous content.
    pub fn draw_region(&self, dirty: Option<RECT>, f: impl FnOnce(&ID2D1DeviceContext) -> Result<()>) -> Result<()> {
        let Some(surface) = &self.surface else { return Ok(()) };
        let full = RECT { left: 0, top: 0, right: self.width as i32, bottom: self.height as i32 };
        let r = match dirty {
            Some(d) => RECT {
                left: d.left.max(0),
                top: d.top.max(0),
                right: d.right.min(full.right),
                bottom: d.bottom.min(full.bottom),
            },
            None => full,
        };
        if r.right <= r.left || r.bottom <= r.top {
            return Ok(());
        }
        unsafe {
            let mut offset = POINT::default();
            let dc: ID2D1DeviceContext = surface.BeginDraw(dirty.map(|_| &r as *const RECT), &mut offset)?;
            dc.SetDpi(96.0, 96.0);
            dc.SetTransform(&Matrix3x2::translation((offset.x - r.left) as f32, (offset.y - r.top) as f32));
            let clip =
                D2D_RECT_F { left: r.left as f32, top: r.top as f32, right: r.right as f32, bottom: r.bottom as f32 };
            dc.PushAxisAlignedClip(&clip, D2D1_ANTIALIAS_MODE_ALIASED);
            dc.Clear(Some(&D2D1_COLOR_F { r: 0.0, g: 0.0, b: 0.0, a: 0.0 }));
            let drawn = f(&dc);
            dc.PopAxisAlignedClip();
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

pub fn draw_bitmap(dc: &ID2D1DeviceContext, bmp: &ID2D1Bitmap1, dest: D2D_RECT_F) -> Result<()> {
    unsafe { dc.DrawBitmap(bmp, Some(&dest), 1.0, D2D1_INTERPOLATION_MODE_HIGH_QUALITY_CUBIC, None, None) };
    Ok(())
}

/// Draw `s` with the given horizontal alignment (the shared format is reset to leading).
pub fn text_aligned(
    dc: &ID2D1DeviceContext,
    s: &str,
    fmt: &IDWriteTextFormat,
    r: D2D_RECT_F,
    c: u32,
    align: DWRITE_TEXT_ALIGNMENT,
) -> Result<()> {
    unsafe {
        fmt.SetTextAlignment(align)?;
    }
    let res = text(dc, s, fmt, r, c);
    unsafe {
        fmt.SetTextAlignment(DWRITE_TEXT_ALIGNMENT_LEADING)?;
    }
    res
}

/// A single icon-font glyph centred in `r`.
pub fn glyph(dc: &ID2D1DeviceContext, g: char, fmt: &IDWriteTextFormat, r: D2D_RECT_F, c: u32) -> Result<()> {
    let mut buf = [0u8; 4];
    text_aligned(dc, g.encode_utf8(&mut buf), fmt, r, c, DWRITE_TEXT_ALIGNMENT_CENTER)
}
