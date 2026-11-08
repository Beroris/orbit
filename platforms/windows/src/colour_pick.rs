//! Picking a colour from anywhere on screen. A still picture of the screens is taken and shown over
//! them, with a magnifier at the pointer. A click, Enter or Space picks the pixel under it; Esc,
//! a right click or switching away cancels. The picture lives only in memory and is dropped as soon
//! as the pick ends, either way. Nothing of it is kept or written anywhere.

use crate::gfx::{Backbuffer, Rgb};
use crate::text::{self, Align, Face};
use std::cell::RefCell;
use windows::Win32::Foundation::{HWND, LPARAM, LRESULT, POINT, RECT, WPARAM};
use windows::Win32::Graphics::Gdi::{
    BI_RGB, BITMAPINFO, BITMAPINFOHEADER, BeginPaint, BitBlt, CAPTUREBLT, CreateCompatibleDC, CreateDIBSection, DIB_RGB_COLORS, DeleteDC, DeleteObject, EndPaint,
    GetDC, HBITMAP, HDC, HGDIOBJ, InvalidateRect, PAINTSTRUCT, ReleaseDC, SRCCOPY, SelectObject,
};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::HiDpi::GetDpiForWindow;
use windows::Win32::UI::Input::KeyboardAndMouse::{GetKeyState, SetFocus, VK_DOWN, VK_ESCAPE, VK_LEFT, VK_RETURN, VK_RIGHT, VK_SHIFT, VK_SPACE, VK_UP};
use windows::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, DefWindowProcW, DestroyWindow, GetCursorPos, GetSystemMetrics, IDC_CROSS, LoadCursorW, PostMessageW, RegisterClassExW, SM_CXVIRTUALSCREEN,
    SM_CYVIRTUALSCREEN, SM_XVIRTUALSCREEN, SM_YVIRTUALSCREEN, SW_SHOW, SetCursorPos, SetForegroundWindow, ShowWindow, WA_INACTIVE, WM_ACTIVATE, WM_CLOSE,
    WM_KEYDOWN, WM_LBUTTONDOWN, WM_MOUSEMOVE, WM_PAINT, WM_RBUTTONDOWN, WNDCLASSEXW, WS_EX_TOOLWINDOW, WS_EX_TOPMOST, WS_POPUP,
};
use windows::core::w;

/// Pixels either side of the one in the middle of the magnifier.
const REACH: i32 = 6;
/// How many screen pixels each one in the magnifier is drawn as, at 100%.
const ZOOM_DIP: i32 = 10;

/// The still picture of the screens.
struct Picture {
    dc: HDC,
    bitmap: HBITMAP,
    previous: HGDIOBJ,
    bits: *const u32,
    width: i32,
    height: i32,
}

impl Picture {
    fn take(x: i32, y: i32, width: i32, height: i32) -> Option<Picture> {
        unsafe {
            let screen = GetDC(None);
            let dc = CreateCompatibleDC(Some(screen));
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
                ReleaseDC(None, screen);
                let _ = DeleteDC(dc);
                return None;
            };
            let previous = SelectObject(dc, HGDIOBJ(bitmap.0));
            let copied = BitBlt(dc, 0, 0, width, height, Some(screen), x, y, SRCCOPY | CAPTUREBLT).is_ok();
            ReleaseDC(None, screen);
            let picture = Picture { dc, bitmap, previous, bits: bits as *const u32, width, height };
            copied.then_some(picture)
        }
    }

    /// The colour at a point in the picture, or black outside it.
    fn at(&self, x: i32, y: i32) -> Rgb {
        if x < 0 || y < 0 || x >= self.width || y >= self.height {
            return Rgb(0, 0, 0);
        }
        // Safety: inside the bitmap, which lives as long as `self`.
        let pixel = unsafe { *self.bits.add((y * self.width + x) as usize) };
        Rgb((pixel >> 16) as u8, (pixel >> 8) as u8, pixel as u8)
    }
}

impl Drop for Picture {
    fn drop(&mut self) {
        unsafe {
            SelectObject(self.dc, self.previous);
            let _ = DeleteObject(HGDIOBJ(self.bitmap.0));
            let _ = DeleteDC(self.dc);
        }
    }
}

struct Pick {
    window: HWND,
    picture: Picture,
    /// Where the pointer is, in the window, which is also where it is in the picture.
    pointer: (i32, i32),
    /// Where the magnifier was last drawn, to clear it from.
    drawn: RECT,
    label: Face,
    dpi: u32,
    main: HWND,
    message: u32,
}

thread_local! {
    static PICK: RefCell<Option<Pick>> = const { RefCell::new(None) };
}

fn scale(dip: i32, dpi: u32) -> i32 {
    dip * dpi as i32 / 96
}

/// Covers the screens to pick a colour from them. When it ends, `message` is posted to `main` with
/// the colour as 0x01RRGGBB in its wParam, or 0 if nothing was picked. False if it couldn't start.
pub fn start(main: HWND, message: u32) -> bool {
    if PICK.with(|pick| pick.borrow().is_some()) {
        return true;
    }
    unsafe {
        let (x, y) = (GetSystemMetrics(SM_XVIRTUALSCREEN), GetSystemMetrics(SM_YVIRTUALSCREEN));
        let (width, height) = (GetSystemMetrics(SM_CXVIRTUALSCREEN), GetSystemMetrics(SM_CYVIRTUALSCREEN));
        let Some(picture) = Picture::take(x, y, width, height) else { return false };
        let Ok(module) = GetModuleHandleW(None) else { return false };
        let class = WNDCLASSEXW {
            cbSize: std::mem::size_of::<WNDCLASSEXW>() as u32,
            lpfnWndProc: Some(pick_proc),
            hInstance: module.into(),
            hCursor: LoadCursorW(None, IDC_CROSS).unwrap_or_default(),
            lpszClassName: w!("OrbitColourPick"),
            ..Default::default()
        };
        // Already registered is fine, from an earlier pick.
        let _ = RegisterClassExW(&class);
        let Ok(window) = CreateWindowExW(WS_EX_TOPMOST | WS_EX_TOOLWINDOW, w!("OrbitColourPick"), w!("Pick a colour"), WS_POPUP, x, y, width, height, None, None, Some(module.into()), None)
        else {
            return false;
        };
        let dpi = GetDpiForWindow(window).max(96);
        let Some(label) = Face::ui(scale(13, dpi) as f32, 13.0, 560) else {
            let _ = DestroyWindow(window);
            return false;
        };
        let mut cursor = POINT::default();
        let _ = GetCursorPos(&mut cursor);
        PICK.with(|pick| {
            *pick.borrow_mut() = Some(Pick { window, picture, pointer: (cursor.x - x, cursor.y - y), drawn: RECT::default(), label, dpi, main, message });
        });
        let _ = ShowWindow(window, SW_SHOW);
        let _ = SetForegroundWindow(window);
        let _ = SetFocus(Some(window));
    }
    true
}

/// Ends the pick, giving back the colour under the pointer if `take` is set.
fn finish(take: bool) {
    let Some(pick) = PICK.with(|pick| pick.borrow_mut().take()) else { return };
    let colour = take.then(|| pick.picture.at(pick.pointer.0, pick.pointer.1));
    let packed = colour.map_or(0, |Rgb(r, g, b)| 0x0100_0000 | (r as usize) << 16 | (g as usize) << 8 | b as usize);
    unsafe {
        let _ = DestroyWindow(pick.window);
        let _ = PostMessageW(Some(pick.main), pick.message, WPARAM(packed), LPARAM(0));
    }
    // The picture goes here, with `pick`.
}

/// Where the magnifier goes for a pointer: below and to the right of it, or flipped to the other
/// side near the edges, so it never covers the pixel being picked.
fn loupe_place(pointer: (i32, i32), size: (i32, i32), room: (i32, i32), gap: i32) -> RECT {
    let left = if pointer.0 + gap + size.0 > room.0 { pointer.0 - gap - size.0 } else { pointer.0 + gap };
    let top = if pointer.1 + gap + size.1 > room.1 { pointer.1 - gap - size.1 } else { pointer.1 + gap };
    RECT { left, top, right: left + size.0, bottom: top + size.1 }
}

fn loupe_rect(pick: &Pick) -> RECT {
    let cells = REACH * 2 + 1;
    let zoom = scale(ZOOM_DIP, pick.dpi);
    let size = (cells * zoom, cells * zoom + scale(30, pick.dpi));
    loupe_place(pick.pointer, size, (pick.picture.width, pick.picture.height), scale(22, pick.dpi))
}

/// Draws the magnifier into `buffer`, which shows the window from `origin`.
fn draw_loupe(pick: &Pick, buffer: &mut Backbuffer, origin: (i32, i32)) {
    let place = loupe_rect(pick);
    let (x, y) = (place.left - origin.0, place.top - origin.1);
    let (width, height) = (place.right - place.left, place.bottom - place.top);
    let zoom = scale(ZOOM_DIP, pick.dpi);
    let radius = scale(12, pick.dpi) as f32;
    buffer.shadow_round_rect(x, y + scale(2, pick.dpi), width, height, radius, scale(10, pick.dpi) as f32, Rgb(0, 0, 0), 120);

    // The pixels around the pointer, large, with the one being picked outlined.
    let cells = REACH * 2 + 1;
    let mut pixels = vec![0u8; (width * height * 4) as usize];
    let picked = pick.picture.at(pick.pointer.0, pick.pointer.1);
    for py in 0..height {
        for px in 0..width {
            let (cell_x, cell_y) = (px / zoom, py / zoom);
            let Rgb(r, g, b) = if cell_y < cells {
                let colour = pick.picture.at(pick.pointer.0 + cell_x - REACH, pick.pointer.1 + cell_y - REACH);
                // A faint grid between pixels.
                if px % zoom == 0 || py % zoom == 0 { colour.mix(Rgb(0, 0, 0), 0.18) } else { colour }
            } else {
                Rgb(28, 28, 32)
            };
            let at = ((py * width + px) * 4) as usize;
            pixels[at..at + 4].copy_from_slice(&[b, g, r, 255]);
        }
    }
    buffer.blit_premultiplied_rect(x, y, width, height, radius, &pixels);
    let middle = (x + REACH * zoom, y + REACH * zoom);
    let ring = if picked.0 as u32 + picked.1 as u32 + picked.2 as u32 > 384 { Rgb(0, 0, 0) } else { Rgb(255, 255, 255) };
    buffer.stroke_round_rect_alpha(middle.0 - 1, middle.1 - 1, zoom + 2, zoom + 2, 2.0, scale(2, pick.dpi) as f32, ring, 255);
    buffer.stroke_round_rect_alpha(x, y, width, height, radius, 1.0, Rgb(255, 255, 255), 60);

    // What's under the pointer, written out, on a swatch of it.
    let strip_top = y + cells * zoom;
    let swatch = scale(14, pick.dpi);
    let swatch_x = x + scale(10, pick.dpi);
    buffer.fill_round_rect(swatch_x, strip_top + (height - cells * zoom - swatch) / 2, swatch, swatch, scale(4, pick.dpi) as f32, picked);
    let hex = orbit_core::colour::Colour { r: picked.0, g: picked.1, b: picked.2 }.hex();
    let text_rect = RECT { left: swatch_x + swatch + scale(8, pick.dpi), top: strip_top, right: x + width - scale(8, pick.dpi), bottom: y + height };
    text::draw(buffer, &hex, &pick.label, Rgb(240, 240, 245), text_rect, Align::Left);
}

fn paint(window: HWND) {
    PICK.with(|pick| {
        let pick = pick.borrow();
        let Some(pick) = pick.as_ref() else { return };
        unsafe {
            let mut paint = PAINTSTRUCT::default();
            let dc = BeginPaint(window, &mut paint);
            let area = paint.rcPaint;
            let (width, height) = (area.right - area.left, area.bottom - area.top);
            if width > 0 && height > 0 {
                if let Some(mut buffer) = Backbuffer::new(width, height) {
                    // The still picture, then the magnifier over it, drawn off screen and shown at once.
                    let _ = BitBlt(buffer.dc, 0, 0, width, height, Some(pick.picture.dc), area.left, area.top, SRCCOPY);
                    draw_loupe(pick, &mut buffer, (area.left, area.top));
                    buffer.copy_to_dc_at(dc, area.left, area.top);
                }
            }
            let _ = EndPaint(window, &paint);
        }
    });
}

fn move_pointer(to: (i32, i32)) {
    PICK.with(|pick| {
        let mut pick = pick.borrow_mut();
        let Some(pick) = pick.as_mut() else { return };
        let to = (to.0.clamp(0, pick.picture.width - 1), to.1.clamp(0, pick.picture.height - 1));
        if to == pick.pointer {
            return;
        }
        let pad = scale(16, pick.dpi);
        let grow = |rect: RECT| RECT { left: rect.left - pad, top: rect.top - pad, right: rect.right + pad, bottom: rect.bottom + pad };
        let before = grow(pick.drawn);
        pick.pointer = to;
        pick.drawn = loupe_rect(pick);
        let after = grow(pick.drawn);
        unsafe {
            let _ = InvalidateRect(Some(pick.window), Some(&before), false);
            let _ = InvalidateRect(Some(pick.window), Some(&after), false);
        }
    });
}

unsafe extern "system" fn pick_proc(window: HWND, message: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    match message {
        WM_PAINT => {
            paint(window);
            LRESULT(0)
        }
        WM_MOUSEMOVE => {
            let (x, y) = ((lparam.0 & 0xffff) as i16 as i32, ((lparam.0 >> 16) & 0xffff) as i16 as i32);
            move_pointer((x, y));
            LRESULT(0)
        }
        WM_LBUTTONDOWN => {
            finish(true);
            LRESULT(0)
        }
        WM_RBUTTONDOWN | WM_CLOSE => {
            finish(false);
            LRESULT(0)
        }
        WM_ACTIVATE => {
            // Switching to another window cancels, so the picture never sits there unseen.
            if (wparam.0 & 0xffff) as u32 == WA_INACTIVE {
                unsafe {
                    let _ = PostMessageW(Some(window), WM_CLOSE, WPARAM(0), LPARAM(0));
                }
            }
            LRESULT(0)
        }
        WM_KEYDOWN => {
            let key = wparam.0 as u16;
            if key == VK_ESCAPE.0 {
                finish(false);
            } else if key == VK_RETURN.0 || key == VK_SPACE.0 {
                finish(true);
            } else if [VK_LEFT.0, VK_RIGHT.0, VK_UP.0, VK_DOWN.0].contains(&key) {
                // The arrows move the pointer a pixel at a time, or ten with Shift.
                let step = if unsafe { GetKeyState(VK_SHIFT.0 as i32) } < 0 { 10 } else { 1 };
                let (dx, dy) = match key {
                    k if k == VK_LEFT.0 => (-step, 0),
                    k if k == VK_RIGHT.0 => (step, 0),
                    k if k == VK_UP.0 => (0, -step),
                    _ => (0, step),
                };
                let mut cursor = POINT::default();
                unsafe {
                    let _ = GetCursorPos(&mut cursor);
                    let _ = SetCursorPos(cursor.x + dx, cursor.y + dy);
                }
            }
            LRESULT(0)
        }
        _ => unsafe { DefWindowProcW(window, message, wparam, lparam) },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_magnifier_stays_on_screen_and_off_the_pixel() {
        let room = (1920, 1080);
        let near_middle = loupe_place((500, 500), (130, 160), room, 20);
        assert_eq!((near_middle.left, near_middle.top), (520, 520));
        let corner = loupe_place((1900, 1070), (130, 160), room, 20);
        assert_eq!((corner.right, corner.bottom), (1880, 1050));
    }
}
