//! Settings' picture of the screens, laid out as Windows lays them out, to choose the one the
//! launcher opens on.
//!
//! Each screen is an outline on its stand, numbered as Windows numbers it, with the launcher a small
//! bar in the accent colour on the chosen one and a dot where it would go on the others. The row
//! above it says what the launcher opens on, and the picture shows only while that's "Specific
//! screen". Otherwise it's folded away to nothing, and the card closes up under the row.
//!
//! It tells its parent `CBN_SELCHANGE` when a screen is chosen, as a drop-down would, so Settings
//! saves it the same way.

use crate::displays::{self, Display};
use crate::forms::{Builder, Style, set_status};
use crate::gfx::{Backbuffer, Rgb};
use crate::util::wide;
use orbit_core::settings::LauncherScreen;
use std::cell::Cell;
use windows::Win32::Foundation::*;
use windows::Win32::Graphics::Gdi::*;
use windows::Win32::UI::Input::KeyboardAndMouse::{EnableWindow, GetFocus, SetFocus, TME_LEAVE, TRACKMOUSEEVENT, TrackMouseEvent};
use windows::Win32::UI::WindowsAndMessaging::*;
use windows::core::{PCWSTR, w};

const CLASS: PCWSTR = w!("OrbitScreenPicker");
/// The whole control, while it shows: the line over it, the picture and the line under it.
const HEIGHT_DIP: i32 = 194;
/// The line between the row above and the picture, drawn here so it folds away with the picture.
const DIVIDER_DIP: i32 = 2;
/// Where the screens may go: from the top, how tall, and room at the sides.
const ROOM_TOP_DIP: i32 = 18;
const ROOM_HEIGHT_DIP: i32 = 138;
const ROOM_SIDE_DIP: i32 = 16;
/// Half the gap left between screens that touch.
const GAP_DIP: f32 = 2.5;
/// A screen's stand: its gap below the screen, its width and its thickness.
const STAND_GAP_DIP: f32 = 4.0;
const STAND_WIDTH_DIP: f32 = 22.0;
const STAND_DIP: f32 = 3.0;
/// The line under the picture.
const CAPTION_DIP: i32 = 24;

/// What the row above the picture offers, in its order.
pub const MODES: [&str; 4] = ["Active window", "Pointer", "Main display", "Specific screen"];
const MODE_NOTES: [&str; 4] = [
    "The screen of the window you're in.",
    "The screen the pointer is on.",
    "The screen Windows marks as main.",
    "The screen you pick below.",
];
/// The one of `MODES` that shows the picture.
const SPECIFIC: usize = 3;

pub fn mode_of(choice: &LauncherScreen) -> usize {
    match choice {
        LauncherScreen::Active => 0,
        LauncherScreen::Pointer => 1,
        LauncherScreen::Main => 2,
        LauncherScreen::Specific { .. } => SPECIFIC,
    }
}

/// How tall the picture is for `choice`: all of it for a specific screen, and otherwise nothing.
fn height_for(style: &Style, choice: &LauncherScreen) -> i32 {
    if mode_of(choice) == SPECIFIC { style.px(HEIGHT_DIP) } else { 0 }
}

/// How tall the picture should be now. Settings sizes it, and moves what's below, to suit.
pub fn height(hwnd: HWND) -> i32 {
    picker_of(hwnd).map_or(0, |picker| height_for(unsafe { &*picker.style }, &picker.choice))
}

pub fn mode_note(mode: usize) -> &'static str {
    MODE_NOTES[mode.min(MODE_NOTES.len() - 1)]
}

struct Picker {
    displays: Vec<Display>,
    choice: LauncherScreen,
    /// The specific screen last picked, kept while another choice is on, to go back to.
    last_specific: Option<(String, String)>,
    /// The screen the keyboard is on.
    cursor: usize,
    hover: Option<usize>,
    tracking: bool,
    /// The note under the row above, which says what the launcher opens on.
    note: HWND,
    /// The form's look. The form's `Style` outlives its controls.
    style: *const Style,
}

thread_local! {
    static READY: Cell<bool> = const { Cell::new(false) };
    /// The screen a preview shows the pointer over.
    static PREVIEW_HOVER: Cell<Option<usize>> = const { Cell::new(None) };
}

fn register() {
    if READY.with(|ready| ready.replace(true)) {
        return;
    }
    unsafe {
        let module = windows::Win32::System::LibraryLoader::GetModuleHandleW(None).unwrap_or_default();
        let class = WNDCLASSEXW {
            cbSize: std::mem::size_of::<WNDCLASSEXW>() as u32,
            style: CS_DBLCLKS,
            lpfnWndProc: Some(picker_proc),
            hInstance: module.into(),
            lpszClassName: CLASS,
            ..Default::default()
        };
        RegisterClassExW(&class);
    }
}

fn picker_of(hwnd: HWND) -> Option<&'static mut Picker> {
    unsafe { (GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *mut Picker).as_mut() }
}

/// Adds the picture across the card, under the row that says what the launcher opens on and
/// its note `note`. It's folded away unless the row says a specific screen.
pub fn create(b: &mut Builder, choice: &LauncherScreen, note: HWND) -> HWND {
    register();
    let height = height_for(b.style, choice);
    let rect = (b.x, b.y, b.width, height);
    let hwnd = b.custom(CLASS, rect, 0);
    let displays = displays::list();
    let last_specific = match choice {
        LauncherScreen::Specific { id, name } => Some((id.clone(), name.clone())),
        _ => None,
    };
    let cursor = specific_index(&displays, choice).or_else(|| displays.iter().position(|display| display.primary)).unwrap_or(0);
    let picker = Box::new(Picker { displays, choice: choice.clone(), last_specific, cursor, hover: PREVIEW_HOVER.with(|hover| hover.get()), tracking: false, note, style: &*b.style as *const Style });
    unsafe {
        SetWindowLongPtrW(hwnd, GWLP_USERDATA, Box::into_raw(picker) as isize);
        // Folded away, it's out of the Tab order too.
        let _ = EnableWindow(hwnd, height > 0);
    }
    retext(hwnd);
    // Not `skip`, which would forget the room after the row above: the card ends the same way
    // whether the picture starts folded or folds later.
    b.y += height;
    hwnd
}

/// What's chosen now.
pub fn choice(hwnd: HWND) -> Option<LauncherScreen> {
    picker_of(hwnd).map(|picker| picker.choice.clone())
}

/// The row above changed to `mode`, from `MODES`. Settings then sizes the picture to `height`.
pub fn set_mode(hwnd: HWND, mode: usize) {
    let Some(picker) = picker_of(hwnd) else { return };
    picker.choice = match mode {
        0 => LauncherScreen::Active,
        1 => LauncherScreen::Pointer,
        2 => LauncherScreen::Main,
        _ => match (&picker.choice, &picker.last_specific) {
            (LauncherScreen::Specific { .. }, _) => picker.choice.clone(),
            (_, Some((id, name))) => LauncherScreen::Specific { id: id.clone(), name: name.clone() },
            // Nothing picked before: the screen this window is on, where the user is looking.
            _ => match displays::of_window(&picker.displays, hwnd).or_else(|| picker.displays.iter().position(|display| display.primary)) {
                Some(at) => specific(&picker.displays[at]),
                None => LauncherScreen::Active,
            },
        },
    };
    if let Some(at) = specific_index(&picker.displays, &picker.choice) {
        picker.cursor = at;
    }
    if let LauncherScreen::Specific { id, name } = &picker.choice {
        picker.last_specific = Some((id.clone(), name.clone()));
    }
    picker.hover = None;
    let shown = mode_of(&picker.choice) == SPECIFIC;
    set_status(picker.note, mode_note(mode_of(&picker.choice)));
    unsafe {
        let _ = EnableWindow(hwnd, shown);
    }
    retext(hwnd);
}

/// Reads the screens again, because Windows says they changed.
pub fn refresh(hwnd: HWND) {
    let Some(picker) = picker_of(hwnd) else { return };
    picker.displays = displays::list();
    picker.hover = None;
    picker.cursor = specific_index(&picker.displays, &picker.choice).unwrap_or(0).min(picker.displays.len().saturating_sub(1));
    retext(hwnd);
}

/// The middle of screen `at` in the picture, for a self-check to click on.
#[cfg(debug_assertions)]
pub fn preview_point(hwnd: HWND, at: usize) -> Option<(i32, i32)> {
    let picker = picker_of(hwnd)?;
    let mut client = RECT::default();
    unsafe { GetClientRect(hwnd, &mut client).ok()? };
    let r = *screen_rects(picker, client.right).get(at)?;
    Some(((r.left + r.right) / 2, (r.top + r.bottom) / 2))
}

/// Shows the pointer over this screen in the next preview.
pub fn preview_hover(at: Option<usize>) {
    PREVIEW_HOVER.with(|hover| hover.set(at));
}

fn specific(display: &Display) -> LauncherScreen {
    LauncherScreen::Specific { id: display.id.clone(), name: display.name.clone() }
}

fn specific_index(displays: &[Display], choice: &LauncherScreen) -> Option<usize> {
    let LauncherScreen::Specific { id, name } = choice else { return None };
    let found = displays::find(displays, id, name)?;
    displays.iter().position(|display| std::ptr::eq(display, found))
}

/// Picks screen `at` and tells the parent. The picture only shows while the row above says
/// "Specific screen", so that stays as it is.
fn choose(hwnd: HWND, at: usize) {
    let Some(picker) = picker_of(hwnd) else { return };
    let Some(display) = picker.displays.get(at) else { return };
    picker.cursor = at;
    let next = specific(display);
    if next == picker.choice {
        let _ = unsafe { InvalidateRect(Some(hwnd), None, false) };
        return;
    }
    picker.choice = next;
    picker.last_specific = Some((display.id.clone(), display.name.clone()));
    retext(hwnd);
    unsafe {
        if let Ok(parent) = GetParent(hwnd) {
            let id = GetDlgCtrlID(hwnd) as usize & 0xFFFF;
            SendMessageW(parent, WM_COMMAND, Some(WPARAM(id | ((CBN_SELCHANGE as usize) << 16))), Some(LPARAM(hwnd.0 as isize)));
        }
    }
}

/// The line under the picture: the screen under the pointer, or the one picked, or why there's
/// none. Empty while the picture is folded away.
fn caption(picker: &Picker) -> String {
    if let Some(display) = picker.hover.and_then(|at| picker.displays.get(at)) {
        return display.describe();
    }
    match &picker.choice {
        LauncherScreen::Specific { name, .. } => match specific_index(&picker.displays, &picker.choice) {
            Some(at) => picker.displays[at].describe(),
            None if name.is_empty() => "That screen isn't connected, so the launcher opens on the active screen for now.".into(),
            None => format!("{name} isn't connected, so the launcher opens on the active screen for now."),
        },
        _ => String::new(),
    }
}

/// Keeps the window's own text, which screen readers read, the same as the line under the picture,
/// and draws it again.
fn retext(hwnd: HWND) {
    let Some(picker) = picker_of(hwnd) else { return };
    let text = wide(&caption(picker));
    unsafe {
        let _ = SetWindowTextW(hwnd, PCWSTR(text.as_ptr()));
        let _ = InvalidateRect(Some(hwnd), None, false);
    }
}

/// Where each screen is drawn: the same scale for all, as large as fits in the room `(x, y, w, h)`,
/// and centred in it. In the order given.
pub fn fit(bounds: &[RECT], room: (f32, f32, f32, f32)) -> Vec<(f32, f32, f32, f32)> {
    let Some(left) = bounds.iter().map(|b| b.left).min() else { return Vec::new() };
    let top = bounds.iter().map(|b| b.top).min().unwrap_or(0);
    let right = bounds.iter().map(|b| b.right).max().unwrap_or(left + 1);
    let bottom = bounds.iter().map(|b| b.bottom).max().unwrap_or(top + 1);
    let (width, height) = ((right - left).max(1) as f32, (bottom - top).max(1) as f32);
    let (x, y, w, h) = room;
    let scale = (w / width).min(h / height);
    let (ox, oy) = (x + (w - width * scale) / 2.0, y + (h - height * scale) / 2.0);
    bounds.iter().map(|b| (ox + (b.left - left) as f32 * scale, oy + (b.top - top) as f32 * scale, (b.right - b.left) as f32 * scale, (b.bottom - b.top) as f32 * scale)).collect()
}

/// Whether nothing sits right under screen `at`, so it has room for its stand.
fn stands_free(bounds: &[RECT], at: usize) -> bool {
    let me = bounds[at];
    !bounds.iter().enumerate().any(|(other, b)| other != at && b.top >= me.bottom && b.top - me.bottom < (me.bottom - me.top) / 8 + 1 && b.left < me.right && b.right > me.left)
}

/// Each screen as drawn, gaps taken off, in `displays` order.
fn screen_rects(picker: &Picker, width: i32) -> Vec<RECT> {
    let style = unsafe { &*picker.style };
    let px = |v: f32| v * style.dpi as f32 / 96.0;
    let side = px(ROOM_SIDE_DIP as f32);
    // Room below for the stands.
    let below = px(STAND_GAP_DIP + STAND_DIP);
    let room = (side, px(ROOM_TOP_DIP as f32), width as f32 - side * 2.0, px(ROOM_HEIGHT_DIP as f32) - below);
    let bounds: Vec<RECT> = picker.displays.iter().map(|display| display.bounds).collect();
    let gap = px(GAP_DIP);
    fit(&bounds, room)
        .into_iter()
        .map(|(x, y, w, h)| RECT { left: (x + gap).round() as i32, top: (y + gap).round() as i32, right: (x + w - gap).round() as i32, bottom: (y + h - gap).round() as i32 })
        .collect()
}

fn screen_at(picker: &Picker, width: i32, x: i32, y: i32) -> Option<usize> {
    let style = unsafe { &*picker.style };
    // The gaps between screens count as the nearer one, so no click falls through a crack.
    let slack = style.px(3);
    screen_rects(picker, width).iter().position(|r| x >= r.left - slack && x < r.right + slack && y >= r.top - slack && y < r.bottom + slack)
}

/// The next screen from the keyboard's in the direction `(dx, dy)`: the nearest whose middle is
/// that way, with straying off the line counting double.
fn next_screen(displays: &[Display], from: usize, dx: i32, dy: i32) -> Option<usize> {
    let middle = |display: &Display| ((display.bounds.left + display.bounds.right) / 2, (display.bounds.top + display.bounds.bottom) / 2);
    let (fx, fy) = middle(displays.get(from)?);
    displays
        .iter()
        .enumerate()
        .filter(|(at, _)| *at != from)
        .filter_map(|(at, display)| {
            let (x, y) = middle(display);
            let (along, across) = ((x - fx) * dx + (y - fy) * dy, ((x - fx) * dy).abs() + ((y - fy) * dx).abs());
            (along > 0).then_some((along as i64 + across as i64 * 2, at))
        })
        .min()
        .map(|(_, at)| at)
}

fn paint(hwnd: HWND, picker: &Picker) {
    let style = unsafe { &*picker.style };
    let theme = &style.theme;
    unsafe {
        let mut ps = PAINTSTRUCT::default();
        let dc = BeginPaint(hwnd, &mut ps);
        let mut client = RECT::default();
        let _ = GetClientRect(hwnd, &mut client);
        if let Some(mut buffer) = Backbuffer::new(client.right, client.bottom) {
            let surface = theme.card;
            buffer.fill(surface);
            let ink = |amount: f32| surface.mix(theme.text, amount);
            let pxf = |v: f32| v * style.dpi as f32 / 96.0;
            let line = 1.0f32.max((style.dpi as f32 / 96.0).round());
            // The same line a card draws between its rows.
            buffer.fill_rect(0, style.px(DIVIDER_DIP), client.right, 1.max(style.dpi as i32 / 96), theme.divider);
            let focused = GetFocus() == hwnd && style.focus_by_keyboard;
            let chosen = specific_index(&picker.displays, &picker.choice);
            let bounds: Vec<RECT> = picker.displays.iter().map(|display| display.bounds).collect();
            let radius = pxf(7.0);
            for (at, r) in screen_rects(picker, client.right).iter().enumerate() {
                let (x, y, w, h) = (r.left, r.top, r.right - r.left, r.bottom - r.top);
                let is_chosen = chosen == Some(at);
                let lit = picker.hover == Some(at);
                let fill = if is_chosen {
                    surface.mix(theme.accent, 0.10)
                } else if lit {
                    ink(0.09)
                } else {
                    ink(0.05)
                };
                buffer.fill_round_rect(x, y, w, h, radius, fill);
                if is_chosen {
                    buffer.stroke_round_rect_alpha(x, y, w, h, radius, line * 2.0, theme.accent, 255);
                } else {
                    buffer.stroke_round_rect_alpha(x, y, w, h, radius, line, theme.text, if lit { 66 } else { 41 });
                }
                if focused && at == picker.cursor {
                    let ring = style.px(3);
                    buffer.stroke_round_rect_alpha(x - ring, y - ring, w + ring * 2, h + ring * 2, radius + ring as f32, line, theme.text, 255);
                }
                if stands_free(&bounds, at) {
                    let stand_w = pxf(STAND_WIDTH_DIP).min(w as f32 * 0.5);
                    let stand_y = r.bottom as f32 + pxf(STAND_GAP_DIP);
                    buffer.fill_round_rect(x + ((w as f32 - stand_w) / 2.0).round() as i32, stand_y.round() as i32, stand_w.round() as i32, pxf(STAND_DIP).round() as i32, pxf(STAND_DIP / 2.0), ink(0.18));
                }
                // The launcher, where Orbit puts it: across the middle, a third of the way down.
                let bar_w = (w as f32 * 0.34).clamp(pxf(12.0), pxf(44.0));
                let bar_h = pxf(7.0).min(h as f32 * 0.16).max(pxf(4.0));
                let (bx, by) = (x as f32 + (w as f32 - bar_w) / 2.0, y as f32 + h as f32 * 0.26 - bar_h / 2.0);
                let (bx, by, bw, bh) = (bx.round() as i32, by.round() as i32, bar_w.round() as i32, bar_h.round() as i32);
                if is_chosen {
                    buffer.fill_round_rect(bx, by, bw, bh, bh as f32 / 2.0, theme.accent);
                } else if lit {
                    buffer.fill_round_rect(bx, by, bw, bh, bh as f32 / 2.0, ink(0.35));
                } else {
                    let dot = pxf(4.0).round() as i32;
                    buffer.fill_round_rect(bx + (bw - dot) / 2, by + (bh - dot) / 2, dot, dot, dot as f32 / 2.0, ink(0.22));
                }
                // Its number, as Windows' display settings show it, when there's room.
                if h >= style.px(40) && w >= style.px(28) {
                    let number = picker.displays[at].number.to_string();
                    let ink_number = if is_chosen || lit { theme.text } else { theme.secondary };
                    let below_bar = RECT { left: x, top: by + bh, right: x + w, bottom: y + h - style.px(4) };
                    buffer.text_centered(&number, style.title, ink_number, below_bar);
                }
            }
            let caption_top = client.bottom - style.px(CAPTION_DIP);
            let words = RECT { left: style.px(ROOM_SIDE_DIP), top: caption_top, right: client.right - style.px(ROOM_SIDE_DIP), bottom: client.bottom };
            // A picked screen that's unplugged is ordinary, such as a laptop away from its desk, so
            // it's said plainly rather than as a problem.
            draw_caption(&buffer, &caption(picker), style.note, theme.secondary, words);
            buffer.copy_to_dc(dc);
        }
        let _ = EndPaint(hwnd, &ps);
    }
}

/// Centred under the picture, or cut short with an ellipsis when the window is too narrow for it.
fn draw_caption(buffer: &Backbuffer, text: &str, font: HFONT, color: Rgb, rect: RECT) {
    if buffer.text_width(text, font) <= rect.right - rect.left {
        buffer.text_centered(text, font, color, rect);
    } else {
        buffer.text(text, font, color, rect, false);
    }
}

unsafe extern "system" fn picker_proc(hwnd: HWND, message: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    unsafe {
        let Some(picker) = picker_of(hwnd) else {
            return DefWindowProcW(hwnd, message, wparam, lparam);
        };
        let at_pointer = || {
            let mut client = RECT::default();
            let _ = GetClientRect(hwnd, &mut client);
            screen_at(picker_of(hwnd)?, client.right, (lparam.0 & 0xFFFF) as i16 as i32, ((lparam.0 >> 16) & 0xFFFF) as i16 as i32)
        };
        match message {
            WM_ERASEBKGND => LRESULT(1),
            WM_PAINT => {
                paint(hwnd, picker);
                LRESULT(0)
            }
            WM_GETDLGCODE => LRESULT(DLGC_WANTARROWS as isize),
            WM_KEYDOWN => {
                let step = match wparam.0 as u16 {
                    0x25 => Some((-1, 0)),
                    0x27 => Some((1, 0)),
                    0x26 => Some((0, -1)),
                    0x28 => Some((0, 1)),
                    _ => None,
                };
                let next = match wparam.0 as u16 {
                    // Space and Enter choose the screen the keyboard is on, as when it follows something.
                    0x20 | 0x0D => Some(picker.cursor),
                    0x24 => (!picker.displays.is_empty()).then_some(0),
                    0x23 => picker.displays.len().checked_sub(1),
                    _ => step.map(|(dx, dy)| next_screen(&picker.displays, picker.cursor, dx, dy).unwrap_or(picker.cursor)),
                };
                match next {
                    Some(at) => {
                        choose(hwnd, at);
                        LRESULT(0)
                    }
                    None => DefWindowProcW(hwnd, message, wparam, lparam),
                }
            }
            WM_LBUTTONDOWN | WM_LBUTTONDBLCLK => {
                let _ = SetFocus(Some(hwnd));
                if let Some(at) = at_pointer() {
                    choose(hwnd, at);
                }
                LRESULT(0)
            }
            WM_SETCURSOR => {
                let mut point = POINT::default();
                let _ = windows::Win32::UI::WindowsAndMessaging::GetCursorPos(&mut point);
                let _ = ScreenToClient(hwnd, &mut point);
                let mut client = RECT::default();
                let _ = GetClientRect(hwnd, &mut client);
                let over = screen_at(picker, client.right, point.x, point.y).is_some();
                let _ = SetCursor(LoadCursorW(None, if over { IDC_HAND } else { IDC_ARROW }).ok());
                LRESULT(1)
            }
            WM_MOUSEMOVE => {
                let hover = at_pointer();
                if hover != picker.hover {
                    picker.hover = hover;
                    retext(hwnd);
                }
                if !picker.tracking {
                    picker.tracking = true;
                    let mut track = TRACKMOUSEEVENT { cbSize: std::mem::size_of::<TRACKMOUSEEVENT>() as u32, dwFlags: TME_LEAVE, hwndTrack: hwnd, dwHoverTime: 0 };
                    let _ = TrackMouseEvent(&mut track);
                }
                LRESULT(0)
            }
            0x02A3 => {
                // WM_MOUSELEAVE
                picker.tracking = false;
                if picker.hover.take().is_some() {
                    retext(hwnd);
                }
                LRESULT(0)
            }
            WM_SETFOCUS | WM_KILLFOCUS | WM_ENABLE => {
                let _ = InvalidateRect(Some(hwnd), None, false);
                LRESULT(0)
            }
            WM_NCDESTROY => {
                SetWindowLongPtrW(hwnd, GWLP_USERDATA, 0);
                drop(Box::from_raw(picker as *mut Picker));
                DefWindowProcW(hwnd, message, wparam, lparam)
            }
            _ => DefWindowProcW(hwnd, message, wparam, lparam),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rect(left: i32, top: i32, width: i32, height: i32) -> RECT {
        RECT { left, top, right: left + width, bottom: top + height }
    }

    #[test]
    fn screens_keep_their_shapes_and_places_and_fill_the_room() {
        // A 1440p main screen with a 1080p one to its right, lower down.
        let bounds = [rect(0, 0, 2560, 1440), rect(2560, 360, 1920, 1080)];
        let drawn = fit(&bounds, (0.0, 0.0, 448.0, 200.0));
        let (a, b) = (drawn[0], drawn[1]);
        assert!((a.2 / a.3 - 2560.0 / 1440.0).abs() < 0.01, "same shape: {a:?}");
        assert!((b.0 - (a.0 + a.2)).abs() < 0.01, "side by side");
        assert!((b.1 + b.3 - (a.1 + a.3)).abs() < 0.01, "bottoms line up, as on the desk");
        assert!((a.0 - 0.0).abs() < 0.01 && (b.0 + b.2 - 448.0).abs() < 0.01, "as wide as the room");
        assert!(a.3 <= 200.0 && a.1 >= 0.0);
    }

    #[test]
    fn one_screen_sits_in_the_middle() {
        let drawn = fit(&[rect(0, 0, 1920, 1080)], (10.0, 0.0, 400.0, 90.0));
        let (x, y, w, h) = drawn[0];
        assert!((h - 90.0).abs() < 0.01 && y.abs() < 0.01, "as tall as the room");
        assert!((x - 10.0 - (400.0 - w) / 2.0).abs() < 0.01, "centred across");
        assert!(fit(&[], (0.0, 0.0, 10.0, 10.0)).is_empty());
    }

    #[test]
    fn a_screen_on_top_of_another_has_no_stand_but_the_one_below_does() {
        let bounds = [rect(0, 0, 1920, 1080), rect(0, -1080, 1920, 1080), rect(1920, 0, 1920, 1080)];
        assert!(stands_free(&bounds, 0));
        assert!(!stands_free(&bounds, 1), "the screen above stands on the one below");
        assert!(stands_free(&bounds, 2));
    }

    #[test]
    fn arrows_go_to_the_nearest_screen_that_way() {
        let displays = [
            Display::stand_in(1, "Middle", rect(0, 0, 2560, 1440), true),
            Display::stand_in(2, "Right", rect(2560, 180, 1920, 1080), false),
            Display::stand_in(3, "Left", rect(-1080, -240, 1080, 1920), false),
            Display::stand_in(4, "Above", rect(320, -1080, 1920, 1080), false),
        ];
        assert_eq!(next_screen(&displays, 0, 1, 0), Some(1));
        assert_eq!(next_screen(&displays, 0, -1, 0), Some(2));
        assert_eq!(next_screen(&displays, 0, 0, -1), Some(3));
        assert_eq!(next_screen(&displays, 1, 1, 0), None, "nothing further right");
        assert_eq!(next_screen(&displays, 3, 0, 1), Some(0));
    }

    #[test]
    fn each_choice_has_its_place_in_the_row_above() {
        assert_eq!(mode_of(&LauncherScreen::Active), 0);
        assert_eq!(mode_of(&LauncherScreen::Pointer), 1);
        assert_eq!(mode_of(&LauncherScreen::Main), 2);
        assert_eq!(mode_of(&LauncherScreen::Specific { id: String::new(), name: String::new() }), SPECIFIC);
        assert_eq!(MODES[SPECIFIC], "Specific screen");
        assert_eq!(MODES.len(), MODE_NOTES.len());
    }
}
