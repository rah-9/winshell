//! Window icon -> premultiplied BGRA pixels, rendered at the exact device size
//! needed so it stays crisp (PLAN.md section 2).
//!
//! M2 uses the window's own icon (WM_GETICON, then the class icon). M3 adds the
//! shell-based path (IShellItemImageFactory) on the asset thread for exe and
//! packaged-app icons.

use windows::Win32::Foundation::{HWND, LPARAM, WPARAM};
use windows::Win32::Graphics::Gdi::*;
use windows::Win32::UI::WindowsAndMessaging::*;

use crate::util::{all_alpha_zero, premultiply_bgra};

#[derive(Debug)]
pub struct Bgra {
    pub width: u32,
    pub height: u32,
    /// Premultiplied BGRA, top-down, `width * 4` bytes per row.
    pub pixels: Vec<u8>,
}

/// The window's icon handle. Not owned: never destroy it.
pub fn window_icon(hwnd: HWND, prefer_small: bool) -> Option<HICON> {
    let order: &[u32] =
        if prefer_small { &[ICON_SMALL2, ICON_SMALL, ICON_BIG] } else { &[ICON_BIG, ICON_SMALL2, ICON_SMALL] };
    for &kind in order {
        let mut result = 0usize;
        let ok = unsafe {
            SendMessageTimeoutW(
                hwnd,
                WM_GETICON,
                WPARAM(kind as usize),
                LPARAM(0),
                SMTO_ABORTIFHUNG | SMTO_BLOCK,
                50,
                Some(&mut result),
            )
        };
        if ok.0 != 0 && result != 0 {
            return Some(HICON(result as _));
        }
    }
    let class_order = if prefer_small { [GCLP_HICONSM, GCLP_HICON] } else { [GCLP_HICON, GCLP_HICONSM] };
    for idx in class_order {
        let h = unsafe { GetClassLongPtrW(hwnd, idx) };
        if h != 0 {
            return Some(HICON(h as _));
        }
    }
    None
}

/// Draw `icon` into a `size` x `size` 32-bit DIB and return premultiplied pixels.
/// Icons without an alpha channel get their transparency from the AND mask.
pub fn render(icon: HICON, size: u32) -> Option<Bgra> {
    let size = size.clamp(1, 256);
    unsafe {
        let hdc = CreateCompatibleDC(None);
        if hdc.is_invalid() {
            return None;
        }
        let bmi = BITMAPINFO {
            bmiHeader: BITMAPINFOHEADER {
                biSize: size_of::<BITMAPINFOHEADER>() as u32,
                biWidth: size as i32,
                biHeight: -(size as i32), // top-down
                biPlanes: 1,
                biBitCount: 32,
                biCompression: BI_RGB.0,
                ..Default::default()
            },
            ..Default::default()
        };
        let mut bits = std::ptr::null_mut();
        let Ok(dib) = CreateDIBSection(Some(hdc), &bmi, DIB_RGB_COLORS, &mut bits, None, 0) else {
            let _ = DeleteDC(hdc);
            return None;
        };
        let old = SelectObject(hdc, dib.into());
        let len = (size * size * 4) as usize;
        let view = || std::slice::from_raw_parts_mut(bits as *mut u8, len);

        let draw = |flags: DI_FLAGS| {
            view().fill(0);
            DrawIconEx(hdc, 0, 0, icon, size as i32, size as i32, 0, None, flags).is_ok()
        };

        let mut out = None;
        if draw(DI_NORMAL) {
            let mut px = view().to_vec();
            if all_alpha_zero(&px) {
                // Legacy icon: colour from DI_IMAGE, alpha from DI_MASK (black = opaque).
                if draw(DI_MASK) {
                    let mask = view().to_vec();
                    if draw(DI_IMAGE) {
                        px = view().to_vec();
                        for (p, m) in px.chunks_exact_mut(4).zip(mask.chunks_exact(4)) {
                            p[3] = if m[0] == 0 && m[1] == 0 && m[2] == 0 { 255 } else { 0 };
                        }
                        premultiply_bgra(&mut px);
                    }
                }
            }
            // DrawIconEx alpha-blends over the zeroed DIB, so alpha icons come out premultiplied.
            out = Some(Bgra { width: size, height: size, pixels: px });
        }

        SelectObject(hdc, old);
        let _ = DeleteObject(dib.into());
        let _ = DeleteDC(hdc);
        out
    }
}

/// Copy a 32-bit HBITMAP (e.g. from IShellItemImageFactory) into premultiplied BGRA.
/// The bitmap is not freed here.
pub fn hbitmap_to_bgra(hbmp: HBITMAP) -> Option<Bgra> {
    unsafe {
        let mut bm = BITMAP::default();
        if GetObjectW(hbmp.into(), size_of::<BITMAP>() as i32, Some(&mut bm as *mut _ as _)) == 0 {
            return None;
        }
        let (w, h) = (bm.bmWidth.max(0) as u32, bm.bmHeight.unsigned_abs());
        if w == 0 || h == 0 || w > 1024 || h > 1024 {
            return None;
        }
        let mut bmi = BITMAPINFO {
            bmiHeader: BITMAPINFOHEADER {
                biSize: size_of::<BITMAPINFOHEADER>() as u32,
                biWidth: w as i32,
                biHeight: -(h as i32),
                biPlanes: 1,
                biBitCount: 32,
                biCompression: BI_RGB.0,
                ..Default::default()
            },
            ..Default::default()
        };
        let mut px = vec![0u8; (w * h * 4) as usize];
        let hdc = CreateCompatibleDC(None);
        let lines = GetDIBits(hdc, hbmp, 0, h, Some(px.as_mut_ptr() as _), &mut bmi, DIB_RGB_COLORS);
        let _ = DeleteDC(hdc);
        if lines != h as i32 {
            return None;
        }
        crate::util::normalize_to_premultiplied(&mut px);
        Some(Bgra { width: w, height: h, pixels: px })
    }
}
