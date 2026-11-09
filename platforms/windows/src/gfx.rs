//! Drawing primitives. A CPU-writable DIB backbuffer with small hand-written pixel loops for
//! antialiased rounded rectangles and icon blending. Forms draw text with GDI here, and the
//! launcher draws its own through `text.rs`.

use crate::util::wide;
use windows::Win32::Foundation::{COLORREF, HWND, POINT, RECT, SIZE};
use windows::Win32::Graphics::Gdi::{
    AC_SRC_ALPHA, AddFontMemResourceEx, AC_SRC_OVER, BI_RGB, BITMAPINFO, BITMAPINFOHEADER, BLENDFUNCTION, BitBlt, CLEARTYPE_QUALITY, CLIP_DEFAULT_PRECIS,
    DRAW_TEXT_FORMAT, DT_CENTER, GdiFlush,
    CreateCompatibleDC, CreateDIBSection, CreateFontW, DEFAULT_CHARSET, DEFAULT_PITCH,
    DIB_RGB_COLORS, DT_END_ELLIPSIS, DT_LEFT, DT_NOPREFIX, DT_RIGHT, DT_SINGLELINE, DT_VCENTER, DT_WORDBREAK,
    DeleteDC, DeleteObject, DrawTextW, FF_DONTCARE, FW_NORMAL, GetDC, GetTextExtentPoint32W,
    GetTextFaceW, HBITMAP, HDC, HFONT, HGDIOBJ, OUT_TT_PRECIS, ReleaseDC, SRCCOPY, SelectObject,
    SetBkMode, SetTextColor, TRANSPARENT,
};
use windows::Win32::UI::WindowsAndMessaging::{DI_NORMAL, DrawIconEx, HICON, ULW_ALPHA, UpdateLayeredWindow};
use windows::core::PCWSTR;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Rgb(pub u8, pub u8, pub u8);

impl Rgb {
    pub const fn colorref(self) -> COLORREF {
        COLORREF(self.0 as u32 | (self.1 as u32) << 8 | (self.2 as u32) << 16)
    }
    /// This colour moved a fraction of the way towards another.
    pub fn mix(self, other: Rgb, amount: f32) -> Rgb {
        let channel = |a: u8, b: u8| (a as f32 + (b as f32 - a as f32) * amount) as u8;
        Rgb(channel(self.0, other.0), channel(self.1, other.1), channel(self.2, other.2))
    }
    const fn bgrx(self) -> u32 {
        (self.0 as u32) << 16 | (self.1 as u32) << 8 | self.2 as u32 | 0xff00_0000
    }
}

/// A 32-bit top-down bitmap with a device context selected onto it.
struct Surface {
    dc: HDC,
    bitmap: HBITMAP,
    previous: HGDIOBJ,
    bits: *mut u32,
}

impl Surface {
    fn new(width: i32, height: i32) -> Option<Surface> {
        unsafe {
            let screen = GetDC(None);
            let dc = CreateCompatibleDC(Some(screen));
            ReleaseDC(None, screen);
            if dc.is_invalid() {
                return None;
            }
            let info = BITMAPINFO {
                bmiHeader: BITMAPINFOHEADER {
                    biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
                    biWidth: width,
                    biHeight: -height,
                    biPlanes: 1,
                    biBitCount: 32,
                    biCompression: BI_RGB.0,
                    ..Default::default()
                },
                ..Default::default()
            };
            let mut bits = std::ptr::null_mut();
            let Ok(bitmap) = CreateDIBSection(Some(dc), &info, DIB_RGB_COLORS, &mut bits, None, 0) else {
                let _ = DeleteDC(dc);
                return None;
            };
            let previous = SelectObject(dc, HGDIOBJ(bitmap.0));
            let _ = SetBkMode(dc, TRANSPARENT);
            Some(Surface { dc, bitmap, previous, bits: bits as *mut u32 })
        }
    }
}

impl Drop for Surface {
    fn drop(&mut self) {
        unsafe {
            SelectObject(self.dc, self.previous);
            let _ = DeleteObject(HGDIOBJ(self.bitmap.0));
            let _ = DeleteDC(self.dc);
        }
    }
}

pub struct Backbuffer {
    pub dc: HDC,
    /// Keeps the bitmap alive for `dc` and `bits`.
    _surface: Surface,
    bits: *mut u32,
    pub width: i32,
    pub height: i32,
}

impl Backbuffer {
    pub fn new(width: i32, height: i32) -> Option<Backbuffer> {
        let surface = Surface::new(width, height)?;
        Some(Backbuffer { dc: surface.dc, bits: surface.bits, _surface: surface, width, height })
    }

    /// Where the pixels are, for drawing text with real alpha. See `text.rs`.
    pub fn raw(&self) -> (*mut u32, i32, i32) {
        (self.bits, self.width, self.height)
    }

    /// Dumps the frame: width and height as little-endian u32s, then premultiplied BGRA rows.
    pub fn write_raw(&self, path: &std::path::Path) -> std::io::Result<()> {
        let mut out = Vec::with_capacity((self.width * self.height * 4 + 8) as usize);
        out.extend_from_slice(&(self.width as u32).to_le_bytes());
        out.extend_from_slice(&(self.height as u32).to_le_bytes());
        let pixels = unsafe { std::slice::from_raw_parts(self.bits as *const u8, (self.width * self.height * 4) as usize) };
        out.extend_from_slice(pixels);
        std::fs::write(path, out)
    }

    #[inline]
    fn row(&mut self, y: i32) -> &mut [u32] {
        debug_assert!(y >= 0 && y < self.height);
        unsafe {
            std::slice::from_raw_parts_mut(
                self.bits.add((y * self.width) as usize),
                self.width as usize,
            )
        }
    }

    pub fn fill(&mut self, color: Rgb) {
        self.fill_rect(0, 0, self.width, self.height, color);
    }

    pub fn fill_rect(&mut self, x: i32, y: i32, w: i32, h: i32, color: Rgb) {
        let value = color.bgrx();
        let x0 = x.clamp(0, self.width) as usize;
        let x1 = (x + w).clamp(0, self.width) as usize;
        for row in y.max(0)..(y + h).min(self.height) {
            self.row(row)[x0..x1].fill(value);
        }
    }

    /// Sets every pixel to `color` at this opacity.
    pub fn clear(&mut self, color: Rgb, alpha: u8) {
        let value = premultiplied(color, alpha);
        for row in 0..self.height {
            self.row(row).fill(value);
        }
    }

    /// A rectangle over what is already there, at this opacity.
    pub fn fill_rect_alpha(&mut self, x: i32, y: i32, w: i32, h: i32, color: Rgb, alpha: u8) {
        let x0 = x.clamp(0, self.width) as usize;
        let x1 = (x + w).clamp(0, self.width) as usize;
        for row in y.max(0)..(y + h).min(self.height) {
            for pixel in &mut self.row(row)[x0..x1] {
                *pixel = blend_alpha(*pixel, color, alpha as f32 / 255.0);
            }
        }
    }

    /// Antialiased rounded rectangle, blended over what is already there.
    pub fn fill_round_rect(&mut self, x: i32, y: i32, w: i32, h: i32, radius: f32, color: Rgb) {
        self.fill_round_rect_alpha(x, y, w, h, radius, color, 255);
    }

    #[allow(clippy::too_many_arguments)]
    pub fn fill_round_rect_alpha(&mut self, x: i32, y: i32, w: i32, h: i32, radius: f32, color: Rgb, alpha: u8) {
        let opacity = alpha as f32 / 255.0;
        let (fw, fh) = (w as f32, h as f32);
        for py in y.max(0)..(y + h).min(self.height) {
            let ly = py - y;
            let cy = ly as f32 + 0.5;
            let row_inner = cy >= radius && cy <= fh - radius;
            for px in x.max(0)..(x + w).min(self.width) {
                let lx = px - x;
                let cx = lx as f32 + 0.5;
                let coverage = if row_inner || (cx >= radius && cx <= fw - radius) {
                    1.0
                } else {
                    let dx = (radius - cx).max(cx - (fw - radius));
                    let dy = (radius - cy).max(cy - (fh - radius));
                    (radius - (dx * dx + dy * dy).sqrt() + 0.5).clamp(0.0, 1.0)
                };
                if coverage > 0.0 {
                    let pixel = &mut self.row(py)[px as usize];
                    *pixel = blend_alpha(*pixel, color, coverage * opacity);
                }
            }
        }
    }

    /// A filled shape with straight edges, antialiased by sampling each pixel 4 by 4. Meant for
    /// small marks, a few pixels across.
    pub fn fill_polygon_alpha(&mut self, points: &[(f32, f32)], color: Rgb, alpha: u8) {
        if points.len() < 3 {
            return;
        }
        let (mut x0, mut y0, mut x1, mut y1) = (f32::MAX, f32::MAX, f32::MIN, f32::MIN);
        for &(x, y) in points {
            (x0, y0, x1, y1) = (x0.min(x), y0.min(y), x1.max(x), y1.max(y));
        }
        let inside = |sx: f32, sy: f32| {
            let mut odd = false;
            let mut j = points.len() - 1;
            for i in 0..points.len() {
                let ((xi, yi), (xj, yj)) = (points[i], points[j]);
                if (yi > sy) != (yj > sy) && sx < (xj - xi) * (sy - yi) / (yj - yi) + xi {
                    odd = !odd;
                }
                j = i;
            }
            odd
        };
        let opacity = alpha as f32 / 255.0;
        for py in (y0.floor() as i32).max(0)..(y1.ceil() as i32).min(self.height) {
            for px in (x0.floor() as i32).max(0)..(x1.ceil() as i32).min(self.width) {
                let mut hits = 0;
                for sy in 0..4 {
                    for sx in 0..4 {
                        if inside(px as f32 + (sx as f32 + 0.5) / 4.0, py as f32 + (sy as f32 + 0.5) / 4.0) {
                            hits += 1;
                        }
                    }
                }
                if hits > 0 {
                    let pixel = &mut self.row(py)[px as usize];
                    *pixel = blend_alpha(*pixel, color, hits as f32 / 16.0 * opacity);
                }
            }
        }
    }

    /// A copy of the rows of pixels from `top` to `bottom`, for [`Backbuffer::fade_back`].
    pub fn save_rows(&self, top: i32, bottom: i32) -> Vec<u32> {
        let (top, bottom) = (top.clamp(0, self.height), bottom.clamp(0, self.height));
        let pixels = unsafe { std::slice::from_raw_parts(self.bits, (self.width * self.height) as usize) };
        pixels[(top * self.width) as usize..(bottom.max(top) * self.width) as usize].to_vec()
    }

    /// Fades what was drawn since [`Backbuffer::save_rows`] back to what was there: untouched above
    /// `from`, gone below `to`, and eased between. Pixels are premultiplied, so mixing them is exact.
    pub fn fade_back(&mut self, saved_top: i32, saved: &[u32], from: i32, to: i32) {
        let width = self.width as usize;
        let span = (to - from).max(1) as f32;
        let top = saved_top.clamp(0, self.height);
        for (at, before) in saved.chunks_exact(width).enumerate() {
            let y = top + at as i32;
            let t = ((y - from) as f32 / span).clamp(0.0, 1.0);
            // How much of the new drawing stays, from all of it to none. Quickly at first, so what runs
            // under the footer is faint by the time its pills are drawn over it.
            let keep = (1.0 - t) * (1.0 - t).sqrt();
            if keep >= 1.0 {
                continue;
            }
            let row = self.row(y);
            if keep <= 0.0 {
                row.copy_from_slice(before);
                continue;
            }
            for (pixel, old) in row.iter_mut().zip(before) {
                if *pixel != *old {
                    *pixel = mix_premultiplied(*old, *pixel, keep);
                }
            }
        }
    }

    /// A rounded rectangle whose colour runs from `top` to `bottom`.
    #[allow(clippy::too_many_arguments)]

    /// A command's tile, lit the way an app icon is: `light` at the top left deepening to `deep` at
    /// the bottom right, a soft gloss from above, and a `rim` wide edge that catches the light along
    /// the top and falls into shade along the bottom.
    #[allow(clippy::too_many_arguments)]
    pub fn fill_tile(&mut self, x: i32, y: i32, size: i32, radius: f32, rim: f32, light: Rgb, deep: Rgb) {
        const WHITE: Rgb = Rgb(255, 255, 255);
        const BLACK: Rgb = Rgb(0, 0, 0);
        let side = size as f32;
        for py in y.max(0)..(y + size).min(self.height) {
            let cy = (py - y) as f32 + 0.5;
            let v = cy / side;
            for px in x.max(0)..(x + size).min(self.width) {
                let cx = (px - x) as f32 + 0.5;
                let coverage = corner_coverage(cx, cy, side, side, radius);
                if coverage <= 0.0 {
                    continue;
                }
                let u = cx / side;
                let mut color = light.mix(deep, (u * 0.35 + v * 0.65).clamp(0.0, 1.0));
                // The gloss: a broad, soft light just above the top left.
                let (gx, gy) = (u - 0.3, v + 0.2);
                let gloss = (1.0 - (gx * gx + gy * gy).sqrt() / 0.95).clamp(0.0, 1.0);
                color = color.mix(WHITE, gloss * gloss * 0.32);
                // The rim, lit at the top and shaded at the bottom.
                let inside = -round_rect_distance(cx, cy, side, side, radius);
                let edge = (rim + 0.5 - inside).clamp(0.0, 1.0);
                if edge > 0.0 {
                    let lit = (1.0 - v * 1.8).clamp(0.0, 1.0);
                    let shade = ((v - 0.5) * 2.0).clamp(0.0, 1.0);
                    color = color.mix(WHITE, edge * lit * 0.5).mix(BLACK, edge * shade * 0.22);
                }
                let pixel = &mut self.row(py)[px as usize];
                *pixel = blend_alpha(*pixel, color, coverage);
            }
        }
    }

    /// A one-pixel line just inside a rounded rectangle's edge, over what is already there.
    #[allow(clippy::too_many_arguments)]
    pub fn stroke_round_rect_alpha(&mut self, x: i32, y: i32, w: i32, h: i32, radius: f32, thickness: f32, color: Rgb, alpha: u8) {
        let opacity = alpha as f32 / 255.0;
        let (fw, fh) = (w as f32, h as f32);
        let inner_radius = (radius - thickness).max(0.0);
        let band = (thickness.ceil() as i32).max(radius.ceil() as i32);
        for py in y.max(0)..(y + h).min(self.height) {
            let ly = py - y;
            let cy = ly as f32 + 0.5;
            let near_end = ly < band || ly >= h - band;
            for px in x.max(0)..(x + w).min(self.width) {
                let lx = px - x;
                if !near_end && lx >= band && lx < w - band {
                    continue;
                }
                let cx = lx as f32 + 0.5;
                let outer = corner_coverage(cx, cy, fw, fh, radius);
                let inner = corner_coverage(cx - thickness, cy - thickness, fw - thickness * 2.0, fh - thickness * 2.0, inner_radius);
                let inside_inner = cx > thickness && cy > thickness && cx < fw - thickness && cy < fh - thickness;
                let coverage = outer - if inside_inner { inner } else { 0.0 };
                if coverage > 0.0 {
                    let pixel = &mut self.row(py)[px as usize];
                    *pixel = blend_alpha(*pixel, color, coverage * opacity);
                }
            }
        }
    }

    /// A colour laid over whole rows, fading out from `alpha` at the top to nothing at the bottom.
    pub fn wash_down(&mut self, y: i32, h: i32, color: Rgb, alpha: u8) {
        for py in y.max(0)..(y + h).min(self.height) {
            let left = 1.0 - (py - y) as f32 / h as f32;
            let opacity = left * left * alpha as f32 / 255.0;
            for pixel in self.row(py) {
                *pixel = blend_alpha(*pixel, color, opacity);
            }
        }
    }

    /// Blends a premultiplied BGRA square at (x, y).
    pub fn blit_premultiplied(&mut self, x: i32, y: i32, size: i32, pixels: &[u8]) {
        self.blit_premultiplied_rect(x, y, size, size, 0.0, pixels);
    }

    /// The same for a `w` by `h` picture, with its corners rounded off by `radius`.
    pub fn blit_premultiplied_rect(&mut self, x: i32, y: i32, w: i32, h: i32, radius: f32, pixels: &[u8]) {
        if w <= 0 || h <= 0 || pixels.len() < (w * h * 4) as usize {
            return;
        }
        for sy in 0..h {
            let py = y + sy;
            if py < 0 || py >= self.height {
                continue;
            }
            let src = &pixels[(sy * w * 4) as usize..((sy + 1) * w * 4) as usize];
            for sx in 0..w {
                let px = x + sx;
                if px < 0 || px >= self.width {
                    continue;
                }
                let s = &src[(sx * 4) as usize..(sx * 4 + 4) as usize];
                let coverage = if radius > 0.0 { corner_coverage(sx as f32 + 0.5, sy as f32 + 0.5, w as f32, h as f32, radius) } else { 1.0 };
                let keep = |value: u8| if coverage >= 1.0 { value as u32 } else { (value as f32 * coverage + 0.5) as u32 };
                let alpha = keep(s[3]);
                if alpha == 0 {
                    continue;
                }
                let pixel = &mut self.row(py)[px as usize];
                let d = *pixel;
                let inv = 255 - alpha.min(255);
                let b = keep(s[0]) + ((d & 0xff) * inv + 127) / 255;
                let g = keep(s[1]) + (((d >> 8) & 0xff) * inv + 127) / 255;
                let r = keep(s[2]) + (((d >> 16) & 0xff) * inv + 127) / 255;
                let a = alpha + ((d >> 24) * inv + 127) / 255;
                *pixel = a.min(255) << 24 | r.min(255) << 16 | g.min(255) << 8 | b.min(255);
            }
        }
    }

    pub fn text(&self, text: &str, font: HFONT, color: Rgb, rect: RECT, align_right: bool) {
        let align = if align_right { DT_RIGHT } else { DT_LEFT };
        self.draw_text(text, font, color, rect, align | DT_SINGLELINE | DT_VCENTER | DT_END_ELLIPSIS | DT_NOPREFIX);
    }

    /// Text that wraps onto more lines to fit across `rect`.
    pub fn text_wrapped(&self, text: &str, font: HFONT, color: Rgb, rect: RECT) {
        self.draw_text(text, font, color, rect, DT_LEFT | DT_WORDBREAK | DT_NOPREFIX);
    }

    /// A symbol, `size` pixels square: Orbit's own where it draws one, or else from the icon font.
    /// `heavy` is for a symbol on a coloured tile.
    #[allow(clippy::too_many_arguments)]
    pub fn symbol_centered(&mut self, glyph: &str, font: HFONT, size: i32, heavy: bool, color: Rgb, rect: RECT) {
        let mut chars = glyph.chars();
        if let (Some(only), None) = (chars.next(), chars.next())
            && crate::symbols::draw(self, only, size as f32, heavy, color, rect, crate::text::Align::Center)
        {
            return;
        }
        self.text_centered(glyph, font, color, rect);
    }

    pub fn text_centered(&self, text: &str, font: HFONT, color: Rgb, rect: RECT) {
        self.draw_text(text, font, color, rect, DT_CENTER | DT_SINGLELINE | DT_VCENTER | DT_NOPREFIX);
    }

    fn draw_text(&self, text: &str, font: HFONT, color: Rgb, rect: RECT, format: DRAW_TEXT_FORMAT) {
        // An empty Vec's pointer isn't a real address, and with an ellipsis DrawTextW reads from it
        // even for no characters, which crashes.
        if text.is_empty() {
            return;
        }
        let mut wide_text: Vec<u16> = text.encode_utf16().collect();
        let mut rect = rect;
        unsafe {
            SelectObject(self.dc, HGDIOBJ(font.0));
            SetTextColor(self.dc, color.colorref());
            DrawTextW(self.dc, &mut wide_text, &mut rect, format);
        }
    }

    pub fn text_width(&self, text: &str, font: HFONT) -> i32 {
        if text.is_empty() {
            return 0;
        }
        let wide_text: Vec<u16> = text.encode_utf16().collect();
        let mut size = Default::default();
        unsafe {
            SelectObject(self.dc, HGDIOBJ(font.0));
            let _ = GetTextExtentPoint32W(self.dc, &wide_text, &mut size);
        }
        size.cx
    }

    /// The pixels of an icon, as premultiplied BGRA. Square, `size` on each side.
    pub fn icon_pixels(icon: HICON, size: i32) -> Option<Vec<u8>> {
        let buffer = Backbuffer::new(size, size)?;
        unsafe {
            DrawIconEx(buffer.dc, 0, 0, icon, size, size, 0, None, DI_NORMAL).ok()?;
            let _ = GdiFlush();
            Some(std::slice::from_raw_parts(buffer.bits as *const u8, (size * size * 4) as usize).to_vec())
        }
    }

    pub fn copy_to_dc(&self, target: HDC) {
        self.copy_to_dc_at(target, 0, 0);
    }

    pub fn copy_to_dc_at(&self, target: HDC, x: i32, y: i32) {
        unsafe {
            let _ = BitBlt(target, x, y, self.width, self.height, Some(self.dc), 0, 0, SRCCOPY);
        }
    }

    /// Shows this frame as a window's whole picture, see-through wherever its alpha says so.
    /// With `at`, the window moves there as well. It needn't be on screen yet.
    pub fn present(&self, hwnd: HWND, at: Option<(i32, i32)>) {
        let origin = at.map(|(x, y)| POINT { x, y });
        let size = SIZE { cx: self.width, cy: self.height };
        let source = POINT { x: 0, y: 0 };
        let blend = BLENDFUNCTION { BlendOp: AC_SRC_OVER as u8, BlendFlags: 0, SourceConstantAlpha: 255, AlphaFormat: AC_SRC_ALPHA as u8 };
        unsafe {
            let _ = UpdateLayeredWindow(hwnd, None, origin.as_ref().map(|p| p as *const POINT), Some(&size), Some(self.dc), Some(&source), COLORREF(0), Some(&blend), ULW_ALPHA);
        }
    }

    /// Lays a soft shadow around this rounded rectangle, outside and under it.
    /// Only for drawing inside the frame, such as a menu lifting off the list behind it.
    #[allow(clippy::too_many_arguments)]
    pub fn shadow_round_rect(&mut self, x: i32, y: i32, w: i32, h: i32, radius: f32, blur: f32, color: Rgb, alpha: u8) {
        let reach = blur.ceil() as i32;
        for py in (y - reach).max(0)..(y + h + reach).min(self.height) {
            let inside_row = py >= y + radius as i32 && py < y + h - radius as i32;
            for px in (x - reach).max(0)..(x + w + reach).min(self.width) {
                if inside_row && px >= x && px < x + w {
                    continue; // whatever casts this shadow is about to cover it
                }
                let distance = round_rect_distance(px as f32 + 0.5 - x as f32, py as f32 + 0.5 - y as f32, w as f32, h as f32, radius);
                let opacity = shadow_falloff(distance, blur) * alpha as f32 / 255.0;
                if opacity > 0.002 {
                    let pixel = &mut self.row(py)[px as usize];
                    *pixel = blend_alpha(*pixel, color, opacity);
                }
            }
        }
    }
}

/// How far a point is outside a rounded rectangle whose corner is at (0, 0). Negative is inside.
#[inline]
pub fn round_rect_distance(px: f32, py: f32, w: f32, h: f32, radius: f32) -> f32 {
    let dx = (px - w / 2.0).abs() - (w / 2.0 - radius);
    let dy = (py - h / 2.0).abs() - (h / 2.0 - radius);
    let outside = (dx.max(0.0) * dx.max(0.0) + dy.max(0.0) * dy.max(0.0)).sqrt();
    outside + dx.max(dy).min(0.0) - radius
}

/// How strong a shadow is at a distance from its edge: soft all the way out to `blur`.
#[inline]
pub fn shadow_falloff(distance: f32, blur: f32) -> f32 {
    let left = (1.0 - (distance / blur)).clamp(0.0, 1.0);
    left * left * left.min(1.0)
}

/// The shadow under the launcher, as how much black covers a pixel of the frame.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Shadow {
    pub margin: i32,
    pub drop: f32,
    pub blur: f32,
    pub strength: f32,
}

impl Shadow {
    /// A premultiplied black at (fx, fy) in the frame, for a panel of this size.
    #[inline]
    fn pixel(&self, fx: i32, fy: i32, w: f32, h: f32, radius: f32) -> u32 {
        if self.strength <= 0.0 {
            return 0;
        }
        let px = fx as f32 + 0.5 - self.margin as f32;
        let py = fy as f32 + 0.5 - self.margin as f32 - self.drop;
        let distance = round_rect_distance(px, py, w, h, radius);
        let alpha = (shadow_falloff(distance, self.blur) * self.strength * 255.0) as u32;
        alpha << 24
    }
}

/// Paints the shadow into everything around the panel. It only depends on the panel's size, so a
/// frame needs this once.
pub fn paint_shadow(frame: &mut Backbuffer, panel_w: i32, panel_h: i32, radius: f32, shadow: Shadow) {
    let (w, h) = (panel_w as f32, panel_h as f32);
    let m = shadow.margin;
    for fy in 0..frame.height {
        let inside = fy >= m && fy < m + panel_h;
        let width = frame.width;
        let row = frame.row(fy);
        for fx in 0..width {
            if inside && fx >= m && fx < m + panel_w {
                continue;
            }
            row[fx as usize] = shadow.pixel(fx, fy, w, h, radius);
        }
    }
}

/// Puts the panel in the middle of the frame with properly rounded, softened corners.
pub fn compose_panel(frame: &mut Backbuffer, panel: &Backbuffer, radius: f32, shadow: Shadow) {
    let m = shadow.margin;
    let (w, h) = (panel.width, panel.height);
    let corner = radius.ceil() as i32;
    for y in 0..h {
        let source = unsafe { std::slice::from_raw_parts(panel.bits.add((y * w) as usize), w as usize) };
        let target = &mut frame.row(y + m)[m as usize..(m + w) as usize];
        target.copy_from_slice(source);
        if y >= corner && y < h - corner {
            continue;
        }
        for x in (0..corner.min(w)).chain((w - corner).max(0)..w) {
            let coverage = corner_coverage(x as f32 + 0.5, y as f32 + 0.5, w as f32, h as f32, radius);
            if coverage >= 1.0 {
                continue;
            }
            let under = shadow.pixel(x + m, y + m, w as f32, h as f32, radius);
            target[x as usize] = mix_premultiplied(under, source[x as usize], coverage);
        }
    }
}

/// `over` at a coverage from 0 to 1, with `under` showing where it doesn't reach.
#[inline]
fn mix_premultiplied(under: u32, over: u32, coverage: f32) -> u32 {
    let amount = (coverage * 256.0) as u32;
    let inverse = 256 - amount;
    let channel = |shift: u32| ((((under >> shift) & 0xff) * inverse + ((over >> shift) & 0xff) * amount) >> 8) << shift;
    channel(24) | channel(16) | channel(8) | channel(0)
}

/// A colour at an opacity, as a premultiplied BGRA pixel.
fn premultiplied(color: Rgb, alpha: u8) -> u32 {
    let channel = |value: u8| (value as u32 * alpha as u32 + 127) / 255;
    (alpha as u32) << 24 | channel(color.0) << 16 | channel(color.1) << 8 | channel(color.2)
}

/// `color` over a premultiplied pixel, at an opacity from 0 to 1.
#[inline]
fn blend_alpha(dst: u32, color: Rgb, opacity: f32) -> u32 {
    if opacity >= 1.0 {
        return color.bgrx();
    }
    let a = (opacity * 256.0) as u32;
    let inv = 256 - a;
    let b = ((dst & 0xff) * inv + color.2 as u32 * a) >> 8;
    let g = (((dst >> 8) & 0xff) * inv + color.1 as u32 * a) >> 8;
    let r = (((dst >> 16) & 0xff) * inv + color.0 as u32 * a) >> 8;
    let alpha = ((dst >> 24) * inv + 255 * a) >> 8;
    alpha << 24 | r << 16 | g << 8 | b
}

/// `color` over a premultiplied pixel, for text covering `amount` of it out of 255.
#[inline]
pub fn blend_coverage(dst: u32, color: Rgb, amount: u8) -> u32 {
    let amount = amount as u32;
    let mix = |below: u32, value: u8| (value as u32 * amount + below * (255 - amount) + 127) / 255;
    let r = mix((dst >> 16) & 0xff, color.0);
    let g = mix((dst >> 8) & 0xff, color.1);
    let b = mix(dst & 0xff, color.2);
    let alpha = amount + ((dst >> 24) * (255 - amount) + 127) / 255;
    alpha << 24 | r << 16 | g << 8 | b
}

/// How much of a point in a rounded rectangle is inside it, softened over one pixel at the corners.
#[inline]
fn corner_coverage(cx: f32, cy: f32, w: f32, h: f32, radius: f32) -> f32 {
    if (cy >= radius && cy <= h - radius) || (cx >= radius && cx <= w - radius) {
        return if cx >= 0.0 && cy >= 0.0 && cx <= w && cy <= h { 1.0 } else { 0.0 };
    }
    let dx = (radius - cx).max(cx - (w - radius));
    let dy = (radius - cy).max(cy - (h - radius));
    (radius - (dx * dx + dy * dy).sqrt() + 0.5).clamp(0.0, 1.0)
}

/// Downscale of premultiplied BGRA where each new pixel averages exactly the area it covers,
/// fractions included. Sizes that don't divide evenly, such as 64 to 26, stay even and smooth, and
/// averaging in premultiplied space keeps dark fringes off transparent edges.
pub fn downscale_premultiplied(src: &[u8], src_size: usize, dst_size: usize) -> Vec<u8> {
    if dst_size >= src_size || dst_size == 0 {
        return src.to_vec();
    }
    let scale = src_size as f32 / dst_size as f32;
    // For each new pixel, the old pixels it covers on one axis, and how much of each.
    let spans: Vec<Vec<(usize, f32)>> = (0..dst_size)
        .map(|d| {
            let (from, to) = (d as f32 * scale, (d + 1) as f32 * scale);
            (from.floor() as usize..(to.ceil() as usize).min(src_size))
                .map(|s| (s, ((s + 1) as f32).min(to) - (s as f32).max(from)))
                .collect()
        })
        .collect();
    let mut out = vec![0u8; dst_size * dst_size * 4];
    for (dy, rows) in spans.iter().enumerate() {
        for (dx, columns) in spans.iter().enumerate() {
            let mut sum = [0f32; 4];
            for &(sy, wy) in rows {
                for &(sx, wx) in columns {
                    let at = (sy * src_size + sx) * 4;
                    for channel in 0..4 {
                        sum[channel] += src[at + channel] as f32 * wx * wy;
                    }
                }
            }
            let at = (dy * dst_size + dx) * 4;
            for channel in 0..4 {
                out[at + channel] = (sum[channel] / (scale * scale)).round().min(255.0) as u8;
            }
        }
    }
    out
}

/// A bitmap copied from the clipboard (`CF_DIB`), checked and ready to be scaled.
struct Dib<'a> {
    data: &'a [u8],
    first: usize,
    stride: usize,
    step: usize,
    width: usize,
    rows: usize,
    bottom_up: bool,
    has_alpha: bool,
}

impl<'a> Dib<'a> {
    /// `None` for a format this doesn't read.
    fn read(dib: &'a [u8]) -> Option<Dib<'a>> {
        let word = |at: usize| dib.get(at..at + 2).map(|b| u16::from_le_bytes([b[0], b[1]]) as u32);
        let long = |at: usize| dib.get(at..at + 4).map(|b| u32::from_le_bytes([b[0], b[1], b[2], b[3]]));
        let header = long(0)? as usize;
        let width = long(4)? as i32;
        let height = long(8)? as i32;
        let bits = word(14)?;
        let compression = long(16)?;
        const BI_BITFIELDS: u32 = 3;
        if width <= 0 || height == 0 || !matches!(bits, 24 | 32) || !matches!(compression, 0 | BI_BITFIELDS) {
            return None;
        }
        let colors = long(32).unwrap_or(0) as usize;
        let masks = if compression == BI_BITFIELDS && header == 40 { 12 } else { 0 };
        let first = header + masks + colors * 4;
        let (width, rows) = (width as usize, height.unsigned_abs() as usize);
        let stride = (width * bits as usize).div_ceil(32) * 4;
        if dib.len() < first.checked_add(stride.checked_mul(rows)?)? {
            return None;
        }
        let mut read = Dib { data: dib, first, stride, step: bits as usize / 8, width, rows, bottom_up: height > 0, has_alpha: false };
        // Screenshots usually leave alpha at zero, which means opaque rather than invisible.
        read.has_alpha = bits == 32
            && (0..rows).step_by((rows / 64).max(1)).any(|y| (0..width).step_by((width / 64).max(1)).any(|x| read.pixel(x, y)[3] != 0));
        Some(read)
    }

    fn pixel(&self, x: usize, y: usize) -> &[u8] {
        let row = if self.bottom_up { self.rows - 1 - y } else { y };
        let at = self.first + row * self.stride + x * self.step;
        &self.data[at..at + self.step]
    }

    /// Premultiplied BGRA at `out_width` by `out_height`, each pixel averaging up to `samples` by
    /// `samples` points of what it covers. Fewer samples keep it quick on a 4K screenshot.
    fn scaled(&self, out_width: usize, out_height: usize, samples: usize) -> Vec<u8> {
        let (width, rows) = (self.width, self.rows);
        let mut out = vec![0u8; out_width * out_height * 4];
        for oy in 0..out_height {
            let (y0, y1) = (oy * rows / out_height, ((oy + 1) * rows / out_height).max(oy * rows / out_height + 1));
            for ox in 0..out_width {
                let (x0, x1) = (ox * width / out_width, ((ox + 1) * width / out_width).max(ox * width / out_width + 1));
                let (dx, dy) = (((x1 - x0) / samples).max(1), ((y1 - y0) / samples).max(1));
                let mut sum = [0u32; 4];
                let mut count = 0;
                for y in (y0..y1.min(rows)).step_by(dy) {
                    for x in (x0..x1.min(width)).step_by(dx) {
                        let p = self.pixel(x, y);
                        let alpha = if self.has_alpha { p[3] as u32 } else { 255 };
                        for channel in 0..3 {
                            sum[channel] += p[channel] as u32 * alpha / 255;
                        }
                        sum[3] += alpha;
                        count += 1;
                    }
                }
                let at = (oy * out_width + ox) * 4;
                for channel in 0..4 {
                    out[at + channel] = (sum[channel] / count.max(1)) as u8;
                }
            }
        }
        out
    }
}

/// Scales a bitmap copied from the clipboard (`CF_DIB`) to fit a square, keeping its shape, as
/// premultiplied BGRA. `None` for a format this doesn't read, and the row keeps its plain icon.
pub fn dib_thumbnail(dib: &[u8], size: usize) -> Option<Vec<u8>> {
    let read = Dib::read(dib)?;
    if size == 0 {
        return None;
    }
    let longest = read.width.max(read.rows);
    let out_width = (read.width * size / longest).max(1);
    let out_height = (read.rows * size / longest).max(1);
    let scaled = read.scaled(out_width, out_height, 4);
    let (left, top) = ((size - out_width) / 2, (size - out_height) / 2);
    let mut out = vec![0u8; size * size * 4];
    for y in 0..out_height {
        let at = ((top + y) * size + left) * 4;
        out[at..at + out_width * 4].copy_from_slice(&scaled[y * out_width * 4..(y + 1) * out_width * 4]);
    }
    Some(out)
}

/// The same bitmap at the largest size that fits `max_width` by `max_height`, never enlarged, as
/// (width, height, premultiplied BGRA).
pub fn dib_preview(dib: &[u8], max_width: usize, max_height: usize) -> Option<(usize, usize, Vec<u8>)> {
    let read = Dib::read(dib)?;
    if max_width == 0 || max_height == 0 {
        return None;
    }
    let fit = (max_width as f64 / read.width as f64).min(max_height as f64 / read.rows as f64).min(1.0);
    let out_width = ((read.width as f64 * fit).round() as usize).clamp(1, max_width);
    let out_height = ((read.rows as f64 * fit).round() as usize).clamp(1, max_height);
    Some((out_width, out_height, read.scaled(out_width, out_height, 8)))
}

/// A clipboard bitmap as a PNG file, so another app can open it. Written beside, then moved into
/// place, so a half-written file is never opened.
pub fn dib_to_png(dib: &[u8], path: &std::path::Path) -> bool {
    use windows::Win32::Foundation::GENERIC_WRITE;
    use windows::Win32::Graphics::Imaging::*;
    use windows::Win32::System::Com::{CLSCTX_INPROC_SERVER, COINIT_MULTITHREADED, CoCreateInstance, CoInitializeEx};
    let Some(read) = Dib::read(dib) else { return false };
    let (width, height) = (read.width, read.rows);
    if width == 0 || height == 0 {
        return false;
    }
    // Every pixel, at its own size, then back from premultiplied, which is what PNG keeps.
    let mut pixels = read.scaled(width, height, 1);
    for pixel in pixels.chunks_exact_mut(4) {
        let alpha = pixel[3] as u32;
        if alpha > 0 && alpha < 255 {
            for channel in &mut pixel[..3] {
                *channel = ((*channel as u32 * 255 + alpha / 2) / alpha).min(255) as u8;
            }
        }
    }
    let partial = path.with_extension("png.partial");
    let written = unsafe {
        let _ = CoInitializeEx(None, COINIT_MULTITHREADED);
        (|| -> windows::core::Result<()> {
            let factory: IWICImagingFactory = CoCreateInstance(&CLSID_WICImagingFactory, None, CLSCTX_INPROC_SERVER)?;
            let stream = factory.CreateStream()?;
            let name = wide(&partial.to_string_lossy());
            stream.InitializeFromFilename(windows::core::PCWSTR(name.as_ptr()), GENERIC_WRITE.0)?;
            let encoder = factory.CreateEncoder(&GUID_ContainerFormatPng, std::ptr::null())?;
            encoder.Initialize(&stream, WICBitmapEncoderNoCache)?;
            let mut frame = None;
            encoder.CreateNewFrame(&mut frame, std::ptr::null_mut())?;
            let frame = frame.ok_or_else(windows::core::Error::empty)?;
            frame.Initialize(None)?;
            frame.SetSize(width as u32, height as u32)?;
            let mut format = GUID_WICPixelFormat32bppBGRA;
            frame.SetPixelFormat(&mut format)?;
            if format != GUID_WICPixelFormat32bppBGRA {
                return Err(windows::core::Error::empty());
            }
            frame.WritePixels(height as u32, width as u32 * 4, &mut pixels)?;
            frame.Commit()?;
            encoder.Commit()?;
            Ok(())
        })()
        .is_ok()
    };
    if !written || std::fs::rename(&partial, path).is_err() {
        let _ = std::fs::remove_file(&partial);
        return false;
    }
    true
}

pub fn create_font(face: &str, height_px: i32, weight: i32) -> HFONT {
    let name = wide(face);
    unsafe {
        CreateFontW(
            -height_px,
            0,
            0,
            0,
            weight,
            0,
            0,
            0,
            DEFAULT_CHARSET,
            OUT_TT_PRECIS,
            CLIP_DEFAULT_PRECIS,
            CLEARTYPE_QUALITY,
            (DEFAULT_PITCH.0 | FF_DONTCARE.0) as u32,
            PCWSTR(name.as_ptr()),
        )
    }
}

/// Geist, the interface typeface, built into the exe and scaled up 3% so it sits as large as
/// Inter. See `assets/fonts/Geist-LICENSE.txt`.
pub const GEIST: &[u8] = include_bytes!("../../../assets/fonts/GeistVariable.ttf");
pub const GEIST_FACE: &str = "Geist";

/// Makes Geist available to GDI in this process only: nothing is installed on the system.
fn load_geist() -> bool {
    let mut count = 0u32;
    let handle = unsafe { AddFontMemResourceEx(GEIST.as_ptr() as *const _, GEIST.len() as u32, None, &mut count) };
    !handle.is_invalid() && count > 0
}

/// Whether GDI really gives `name` when asked for it, rather than a stand-in.
fn resolves_to(name: &str) -> bool {
    let font = create_font(name, 12, FW_NORMAL.0 as i32);
    let found = font_face_is(font, name);
    delete_font(font);
    found
}

/// Orbit's typeface: Geist, or if it couldn't be loaded, Windows 11's text face, or the older one.
pub fn ui_face() -> &'static str {
    static FACE: std::sync::OnceLock<&'static str> = std::sync::OnceLock::new();
    FACE.get_or_init(|| {
        const NEW: &str = "Segoe UI Variable Text";
        if load_geist() && resolves_to(GEIST_FACE) {
            GEIST_FACE
        } else if resolves_to(NEW) {
            NEW
        } else {
            "Segoe UI"
        }
    })
}

fn font_face_is(font: HFONT, name: &str) -> bool {
    unsafe {
        let screen = GetDC(None);
        let dc = CreateCompatibleDC(Some(screen));
        ReleaseDC(None, screen);
        let previous = SelectObject(dc, HGDIOBJ(font.0));
        let mut face = [0u16; 64];
        let len = GetTextFaceW(dc, Some(&mut face));
        SelectObject(dc, previous);
        let _ = DeleteDC(dc);
        String::from_utf16_lossy(&face[..(len.max(1) - 1) as usize]).eq_ignore_ascii_case(name)
    }
}

pub fn delete_font(font: HFONT) {
    if !font.is_invalid() {
        unsafe {
            let _ = DeleteObject(HGDIOBJ(font.0));
        }
    }
}

/// Uses Segoe Fluent Icons on Windows 11 and Segoe MDL2 Assets on Windows 10. The glyph
/// codepoints used here are shared by both fonts.
pub fn create_icon_font(height_px: i32) -> HFONT {
    let font = create_font("Segoe Fluent Icons", height_px, FW_NORMAL.0 as i32);
    if font_face_is(font, "Segoe Fluent Icons") {
        font
    } else {
        delete_font(font);
        create_font("Segoe MDL2 Assets", height_px, FW_NORMAL.0 as i32)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rows_under_the_footer_fade_back_to_what_was_there() {
        let mut buffer = Backbuffer::new(4, 40).unwrap();
        buffer.clear(Rgb(20, 20, 20), 255);
        let saved = buffer.save_rows(10, 40);
        buffer.fill_rect(0, 0, 4, 40, Rgb(220, 220, 220));
        buffer.fade_back(10, &saved, 10, 30);
        let red = |y: i32| (pixel(&buffer, 0, y) >> 16) & 0xff;
        assert_eq!(red(5), 220, "above the fade, as drawn");
        assert_eq!(red(10), 220, "where it starts, still all there");
        assert!(red(15) > red(20) && red(20) > red(25) && red(25) > 20, "fading, {} {} {}", red(15), red(20), red(25));
        assert_eq!(red(30), 20, "where it ends, gone");
        assert_eq!(red(39), 20, "and below");
    }

    #[test]
    fn a_clipboard_picture_becomes_a_png_to_open() {
        let (width, height) = (3usize, 2usize);
        let mut dib = vec![0u8; 40];
        dib[0] = 40;
        dib[4..8].copy_from_slice(&(width as i32).to_le_bytes());
        dib[8..12].copy_from_slice(&(height as i32).to_le_bytes());
        dib[12] = 1;
        dib[14] = 32;
        for _ in 0..width * height {
            dib.extend_from_slice(&[255, 0, 0, 0]);
        }
        let path = std::env::temp_dir().join(format!("orbit-png-test-{}.png", std::process::id()));
        assert!(dib_to_png(&dib, &path));
        let bytes = std::fs::read(&path).unwrap();
        assert_eq!(&bytes[..8], b"\x89PNG\r\n\x1a\n");
        assert!(!path.with_extension("png.partial").exists());
        let _ = std::fs::remove_file(&path);
        assert!(!dib_to_png(b"not a picture", &path));
        assert!(!path.exists());
    }

    #[test]
    fn downscale_averages_premultiplied() {
        // Two opaque white pixels and two transparent ones scale to half-coverage white.
        let src = [255, 255, 255, 255, 0, 0, 0, 0, 255, 255, 255, 255, 0, 0, 0, 0];
        assert_eq!(downscale_premultiplied(&src, 2, 1), vec![128, 128, 128, 128]);
    }

    #[test]
    fn downscale_to_a_size_that_doesnt_divide_evenly_stays_even() {
        // A flat colour must stay exactly flat whatever the sizes, with no darker seams.
        let src = vec![200u8; 64 * 64 * 4];
        let out = downscale_premultiplied(&src, 64, 26);
        assert_eq!(out.len(), 26 * 26 * 4);
        assert!(out.iter().all(|v| (199..=201).contains(v)), "{:?}", out.iter().min());
    }

    #[test]
    fn upscale_request_returns_original() {
        let src = vec![1u8; 16];
        assert_eq!(downscale_premultiplied(&src, 2, 4), src);
    }

    #[test]
    fn blend_endpoints() {
        let white = Rgb(255, 255, 255);
        assert_eq!(blend_alpha(0xff00_0000, white, 1.0), 0xffff_ffff);
        assert_eq!(blend_alpha(0xff00_0000, white, 0.0), 0xff00_0000);
    }

    /// A 2x2 bitmap: bottom row first in the file, as a positive height means.
    fn dib(height: i32, pixels: &[[u8; 4]]) -> Vec<u8> {
        let mut out = vec![0u8; 40];
        out[0] = 40;
        out[4..8].copy_from_slice(&2i32.to_le_bytes());
        out[8..12].copy_from_slice(&height.to_le_bytes());
        out[14] = 32;
        for pixel in pixels {
            out.extend_from_slice(pixel);
        }
        out
    }

    #[test]
    fn thumbnails_read_top_and_bottom_up_bitmaps() {
        const RED: [u8; 4] = [0, 0, 255, 0];
        const BLUE: [u8; 4] = [255, 0, 0, 0];
        let thumb = dib_thumbnail(&dib(2, &[RED, RED, BLUE, BLUE]), 2).unwrap();
        assert_eq!(&thumb[0..4], [255, 0, 0, 255], "the last row in the file is the top");
        assert_eq!(&thumb[8..12], [0, 0, 255, 255]);
        let thumb = dib_thumbnail(&dib(-2, &[RED, RED, BLUE, BLUE]), 2).unwrap();
        assert_eq!(&thumb[0..4], [0, 0, 255, 255], "a negative height is top-down");
    }

    #[test]
    fn previews_fit_the_box_and_are_never_enlarged() {
        const RED: [u8; 4] = [0, 0, 255, 0];
        let small = dib(1, &[RED, RED]);
        let (w, h, pixels) = dib_preview(&small, 100, 100).unwrap();
        assert_eq!((w, h), (2, 1), "a small picture stays its own size");
        assert_eq!(&pixels[0..4], [0, 0, 255, 255]);
        let (w, h, _) = dib_preview(&small, 1, 100).unwrap();
        assert_eq!((w, h), (1, 1), "narrower than it is, so scaled down, keeping its shape");
        assert_eq!(dib_preview(&small, 0, 10), None);
    }

    #[test]
    fn rounded_pictures_lose_only_their_corners() {
        let mut buffer = Backbuffer::new(20, 20).unwrap();
        let white = vec![255u8; 20 * 20 * 4];
        buffer.blit_premultiplied_rect(0, 0, 20, 20, 6.0, &white);
        let (bits, width, _) = buffer.raw();
        let pixels = unsafe { std::slice::from_raw_parts(bits, 400) };
        assert_eq!(pixels[0] >> 24, 0, "the corner is cut away");
        assert_eq!(pixels[(10 * width + 10) as usize], 0xffff_ffff, "the middle is all there");
        assert_eq!(pixels[(10 * width) as usize] >> 24, 255, "and so is the middle of an edge");
    }

    #[test]
    fn empty_text_draws_nothing_and_doesnt_crash() {
        let rect = RECT { left: 0, top: 0, right: 40, bottom: 20 };
        let buffer = Backbuffer::new(40, 20).unwrap();
        let font = HFONT::default();
        buffer.text("", font, Rgb(255, 255, 255), rect, false);
        buffer.text("", font, Rgb(255, 255, 255), rect, true);
        buffer.text_centered("", font, Rgb(255, 255, 255), rect);
        assert_eq!(buffer.text_width("", font), 0);
    }

    #[test]
    fn thumbnails_use_alpha_when_there_is_any() {
        let thumb = dib_thumbnail(&dib(1, &[[200, 200, 200, 128], [0, 0, 0, 0]]), 2).unwrap();
        assert_eq!(thumb[3], 128, "half transparent");
        assert!(thumb[0] <= 128, "and premultiplied");
    }

    #[test]
    fn thumbnails_keep_the_shape_and_refuse_bad_data() {
        // Twice as wide as tall: letterboxed, with clear rows above and below.
        let thumb = dib_thumbnail(&dib(1, &[[9, 9, 9, 0], [9, 9, 9, 0]]), 4).unwrap();
        assert_eq!(thumb[3], 0);
        assert_eq!(thumb[4 * 4 + 3], 255);
        assert_eq!(dib_thumbnail(&[0; 10], 4), None);
        assert_eq!(dib_thumbnail(&dib(5, &[[0; 4]; 2]), 4), None, "claims more rows than it holds");
        let mut sixteen_bit = dib(1, &[[0; 4]; 2]);
        sixteen_bit[14] = 16;
        assert_eq!(dib_thumbnail(&sixteen_bit, 4), None);
        assert_eq!(dib_preview(&sixteen_bit, 4, 4), None);
    }

    #[test]
    fn translucent_pixels_stay_premultiplied() {
        let tint = premultiplied(Rgb(200, 100, 50), 128);
        assert_eq!(tint, 0x8064_3219);
        // Half-opaque white over half-opaque black: mostly opaque, and half white.
        let over = blend_alpha(premultiplied(Rgb(0, 0, 0), 128), Rgb(255, 255, 255), 0.5);
        let (alpha, red) = (over >> 24, (over >> 16) & 0xff);
        assert!((190..=193).contains(&alpha), "{alpha}");
        assert!((126..=128).contains(&red), "{red}");
        assert!(red <= alpha);
    }

    #[test]
    fn text_coverage_makes_a_pixel_more_opaque() {
        let below = premultiplied(Rgb(18, 18, 20), 184);
        assert_eq!(blend_coverage(below, Rgb(255, 255, 255), 255), 0xffff_ffff);
        assert_eq!(blend_coverage(below, Rgb(255, 255, 255), 0), below);
        let half = blend_coverage(below, Rgb(255, 255, 255), 128);
        assert!(half >> 24 > below >> 24);
        assert!((half >> 16) & 0xff <= half >> 24);
    }

    fn pixel(buffer: &Backbuffer, x: i32, y: i32) -> u32 {
        let (bits, width, _) = buffer.raw();
        unsafe { *bits.add((y * width + x) as usize) }
    }

    #[test]
    fn a_stroke_marks_the_edge_and_leaves_the_middle_alone() {
        let mut buffer = Backbuffer::new(40, 40).unwrap();
        buffer.stroke_round_rect_alpha(0, 0, 40, 40, 8.0, 1.0, Rgb(255, 255, 255), 255);
        assert_ne!(pixel(&buffer, 20, 0), 0, "top edge");
        assert_ne!(pixel(&buffer, 0, 20), 0, "left edge");
        assert_ne!(pixel(&buffer, 39, 20), 0, "right edge");
        assert_ne!(pixel(&buffer, 20, 39), 0, "bottom edge");
        assert_eq!(pixel(&buffer, 20, 20), 0, "middle");
        assert_eq!(pixel(&buffer, 20, 2), 0, "just inside");
        assert_eq!(pixel(&buffer, 0, 0), 0, "outside the rounded corner");
    }

    #[test]
    fn a_tile_runs_from_its_light_colour_to_its_deep_one_and_rounds_its_corners() {
        let mut buffer = Backbuffer::new(40, 40).unwrap();
        buffer.fill_tile(0, 0, 40, 8.0, 1.0, Rgb(200, 0, 0), Rgb(0, 0, 200));
        let (near_top, near_bottom) = (pixel(&buffer, 8, 6), pixel(&buffer, 32, 33));
        assert!((near_top >> 16) & 0xff > (near_bottom >> 16) & 0xff, "redder at the top left");
        assert!(near_bottom & 0xff > near_top & 0xff, "bluer at the bottom right");
        assert_eq!(pixel(&buffer, 0, 0), 0, "outside the rounded corner");
    }

    #[test]
    fn a_wash_fades_to_nothing() {
        let mut buffer = Backbuffer::new(4, 10).unwrap();
        buffer.wash_down(0, 10, Rgb(255, 255, 255), 100);
        let brightness = |y| pixel(&buffer, 1, y) & 0xff;
        assert!(brightness(0) > brightness(5));
        assert!(brightness(5) > brightness(9));
    }
}
