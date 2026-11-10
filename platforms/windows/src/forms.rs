//! Native forms that match the launcher: real Win32 controls, so tabbing, screen readers and text
//! editing behave as Windows users expect, painted in Orbit's colours.
//!
//! Buttons and switches are real `BUTTON` controls drawn through `NM_CUSTOMDRAW`, which keeps
//! their role, state and hot tracking. Text boxes are borderless, sitting on a rounded field the
//! parent paints. Drop-downs are real `COMBOBOX` controls whose closed face and list rows are
//! painted here. These windows are opened rarely, so their cost appears only when they're used.

use crate::gfx::{Backbuffer, create_font, create_icon_font, delete_font};
use crate::ui::Theme;
use crate::util::wide;
use std::cell::Cell;
use windows::Win32::Foundation::{HWND, LPARAM, LRESULT, POINT, RECT, WPARAM};
use windows::Win32::Graphics::Dwm::{DWMWA_WINDOW_CORNER_PREFERENCE, DWMWCP_ROUND, DwmSetWindowAttribute};
use windows::Win32::Graphics::Gdi::*;
use windows::Win32::UI::Controls::{
    BST_CHECKED, BST_UNCHECKED, CDDS_PREPAINT, COMBOBOXINFO, GetComboBoxInfo, CDIS_DISABLED, CDIS_FOCUS, CDIS_HOT, CDIS_SELECTED, CDRF_SKIPDEFAULT, DRAWITEMSTRUCT, EM_GETLINECOUNT, EM_SETCUEBANNER, NM_CUSTOMDRAW,
    NMCUSTOMDRAW, NMHDR, ODS_COMBOBOXEDIT, ODS_SELECTED, ODT_COMBOBOX, SetWindowTheme, ShowScrollBar,
};
use windows::Win32::System::SystemServices::{SS_CENTERIMAGE, SS_NOPREFIX};
use windows::Win32::UI::Input::KeyboardAndMouse::{GetFocus, GetKeyState, IsWindowEnabled, SetFocus, VK_BACK, VK_CONTROL, VK_MENU};
use windows::Win32::UI::Shell::{DefSubclassProc, RemoveWindowSubclass, SetWindowSubclass};
use windows::Win32::UI::WindowsAndMessaging::*;
use windows::core::{PCWSTR, w};

pub fn scale(value: i32, dpi: u32) -> i32 {
    (value * dpi as i32 + 48) / 96
}

/// Quieter text: descriptions and hints.
pub const ID_NOTE: i32 = 9001;
/// Messages about what just happened.
const ID_STATUS_TEXT: i32 = 9002;
/// From a card's edge to what's on it.
const CARD_PAD_DIP: i32 = 16;
/// Extra room at the left of a search box.
const SEARCH_GLASS_DIP: i32 = 18;

/// Corner sizes, at 96 DPI. Everything scales through `px()`.
pub const RADIUS_CARD: i32 = 14;
pub const RADIUS_FIELD: i32 = 10;
pub const RADIUS_SELECTION: i32 = 10;
pub const RADIUS_ROW: i32 = 6;

/// Height of a row in a managed list.
pub const MANAGED_ROW_DIP: i32 = 54;
/// Every drop-down and the launcher's shortcut are this wide, so that they line up.
pub const COMBO_DIP: i32 = 220;
/// Every button at the right of a titled row is this wide, for the same reason.
const ACTION_BUTTON_DIP: i32 = 120;

thread_local! {
    /// Set by the developer preview. Forms then appear off-screen and never take focus.
    static PREVIEW: Cell<bool> = const { Cell::new(false) };
    /// The last form shown, for the developer preview to capture.
    static LAST_FORM: Cell<Option<HWND>> = const { Cell::new(None) };
}

pub fn set_preview(on: bool) {
    PREVIEW.with(|cell| cell.set(on));
}

pub fn is_preview() -> bool {
    PREVIEW.with(|cell| cell.get())
}

pub fn last_form() -> Option<HWND> {
    LAST_FORM.with(|cell| cell.get())
}

/// Where a form's window starts: wherever Windows likes, or far off-screen in a preview.
pub fn form_origin() -> (i32, i32) {
    if is_preview() { (-20000, -20000) } else { (CW_USEDEFAULT, CW_USEDEFAULT) }
}

/// Extended styles for a form's window. A preview must never be able to take focus.
pub fn form_ex_style() -> WINDOW_EX_STYLE {
    if is_preview() { WS_EX_TOOLWINDOW | WS_EX_NOACTIVATE } else { WINDOW_EX_STYLE(0) }
}

/// Moves the keyboard focus, except in a preview.
pub fn focus(control: HWND) {
    if !is_preview() {
        unsafe {
            let _ = SetFocus(Some(control));
        }
    }
}

/// Shows a finished form in front with the focus in `control`. A preview shows it where it is,
/// and does nothing to the foreground window or the focus.
pub fn show_form(hwnd: HWND, control: HWND) {
    LAST_FORM.with(|cell| cell.set(Some(hwnd)));
    unsafe {
        if is_preview() {
            let _ = ShowWindow(hwnd, SW_SHOWNOACTIVATE);
            return;
        }
        let _ = ShowWindow(hwnd, SW_SHOW);
        if !SetForegroundWindow(hwnd).as_bool() {
            crate::launch::force_foreground(hwnd);
        }
        let _ = SetFocus(Some(control));
    }
}

/// Makes the window's client area exactly this size, and keeps the whole window on its screen.
pub fn fit_client(hwnd: HWND, width: i32, height: i32) {
    unsafe {
        let style = WINDOW_STYLE(GetWindowLongPtrW(hwnd, GWL_STYLE) as u32);
        let extended = WINDOW_EX_STYLE(GetWindowLongPtrW(hwnd, GWL_EXSTYLE) as u32);
        let mut size = RECT { left: 0, top: 0, right: width, bottom: height };
        let _ = AdjustWindowRectEx(&mut size, style, false, extended);
        let (outer_width, outer_height) = (size.right - size.left, size.bottom - size.top);
        let mut now = RECT::default();
        let _ = GetWindowRect(hwnd, &mut now);
        let mut top = now.top;
        if !is_preview() {
            let mut info = MONITORINFO { cbSize: std::mem::size_of::<MONITORINFO>() as u32, ..Default::default() };
            if GetMonitorInfoW(MonitorFromWindow(hwnd, MONITOR_DEFAULTTONEAREST), &mut info).as_bool() {
                top = top.min(info.rcWork.bottom - outer_height).max(info.rcWork.top);
            }
        }
        let _ = SetWindowPos(hwnd, None, now.left, top, outer_width, outer_height, SWP_NOZORDER | SWP_NOACTIVATE);
    }
}

/// How far a page can scroll.
pub fn max_scroll(content: i32, view: i32) -> i32 {
    (content - view).max(0)
}

pub fn clamp_scroll(scroll: i32, content: i32, view: i32) -> i32 {
    scroll.clamp(0, max_scroll(content, view))
}

/// The scroll position that shows `top..bottom`, moving as little as possible. Something taller
/// than the view shows its top.
pub fn reveal_scroll(scroll: i32, top: i32, bottom: i32, content: i32, view: i32, margin: i32) -> i32 {
    let (want_top, want_bottom) = (top - margin, bottom + margin);
    let wanted = if want_top < scroll || want_bottom - want_top >= view {
        want_top
    } else if want_bottom > scroll + view {
        want_bottom - view
    } else {
        scroll
    };
    clamp_scroll(wanted, content, view)
}

/// The scroll thumb's top and height within a track as tall as the view. `None` when the page fits.
pub fn thumb_geometry(scroll: i32, content: i32, view: i32, inset: i32, min_height: i32) -> Option<(i32, i32)> {
    let most = max_scroll(content, view);
    if most == 0 {
        return None;
    }
    let track = view - inset * 2;
    let height = ((track as i64 * view as i64 / content as i64) as i32).max(min_height).min(track);
    let travel = track - height;
    let top = inset + (travel as i64 * scroll.clamp(0, most) as i64 / most as i64) as i32;
    Some((top, height))
}

/// The opposite of `thumb_geometry`: the scroll position for a thumb dragged to `thumb_top`.
pub fn scroll_for_thumb(thumb_top: i32, content: i32, view: i32, inset: i32, min_height: i32) -> i32 {
    let most = max_scroll(content, view);
    let Some((_, height)) = thumb_geometry(0, content, view, inset, min_height) else { return 0 };
    let travel = view - inset * 2 - height;
    if travel <= 0 {
        return 0;
    }
    let along = (thumb_top - inset).clamp(0, travel) as i64;
    ((along * most as i64 + travel as i64 / 2) / travel as i64) as i32
}

/// Pixels to move for a wheel turn. A notch is 120, and Windows says how many lines that is.
pub fn wheel_pixels(delta: i32, lines_per_notch: i32, line_height: i32) -> i32 {
    delta * lines_per_notch.max(1) * line_height / 120
}

/// Where Ctrl+Backspace deletes back to: one line break, or else any spaces and then one run of
/// word characters or of punctuation. Counted in UTF-16 units, as an edit control does.
pub fn word_start(units: &[u16], caret: usize) -> usize {
    const CR: u16 = 13;
    const LF: u16 = 10;
    let is_blank = |unit: u16| unit != CR && unit != LF && char::from_u32(unit as u32).is_some_and(char::is_whitespace);
    let is_word = |unit: u16| {
        // Half of a surrogate pair can't be looked up. Treating it as a letter keeps pairs whole.
        char::from_u32(unit as u32).is_none_or(|c| c.is_alphanumeric() || c == '_')
    };
    let mut at = caret.min(units.len());
    if at == 0 {
        return 0;
    }
    if units[at - 1] == LF {
        at -= 1;
        if at > 0 && units[at - 1] == CR {
            at -= 1;
        }
        return at;
    }
    if units[at - 1] == CR {
        return at - 1;
    }
    while at > 0 && is_blank(units[at - 1]) {
        at -= 1;
    }
    if at == 0 || units[at - 1] == CR || units[at - 1] == LF {
        return at;
    }
    let wordy = is_word(units[at - 1]);
    while at > 0 && !is_blank(units[at - 1]) && units[at - 1] != CR && units[at - 1] != LF && is_word(units[at - 1]) == wordy {
        at -= 1;
    }
    at
}

const EM_GETSEL_MSG: u32 = 0xB0;
const EM_SETSEL_MSG: u32 = 0xB1;
const EM_REPLACESEL_MSG: u32 = 0xC2;
const EDIT_KEYS_SUBCLASS: usize = 7;
const LIST_WHEEL_SUBCLASS: usize = 8;

/// Puts text where the caret is, replacing any selection, in a way Undo can take back.
pub fn replace_selection(edit: HWND, text: &str) {
    let value = wide(text);
    unsafe {
        SendMessageW(edit, EM_REPLACESEL_MSG, Some(WPARAM(1)), Some(LPARAM(value.as_ptr() as isize)));
    }
}

fn delete_word_before_caret(edit: HWND) {
    let (mut start, mut end) = (0u32, 0u32);
    unsafe {
        SendMessageW(edit, EM_GETSEL_MSG, Some(WPARAM(&mut start as *mut u32 as usize)), Some(LPARAM(&mut end as *mut u32 as isize)));
    }
    if start == end {
        let units: Vec<u16> = get_text(edit).encode_utf16().collect();
        let from = word_start(&units, end as usize);
        if from == end as usize {
            return;
        }
        unsafe {
            SendMessageW(edit, EM_SETSEL_MSG, Some(WPARAM(from)), Some(LPARAM(end as isize)));
        }
    }
    replace_selection(edit, "");
}

/// True if a multi-line box has more lines than it can show.
fn edit_overflows(edit: HWND) -> bool {
    unsafe {
        let style = GetWindowLongPtrW(edit, GWL_STYLE) as u32;
        style & ES_MULTILINE as u32 != 0 && edit_needs_bar(edit)
    }
}

/// Everything plain Win32 text boxes miss: Ctrl+Backspace and Ctrl+A, without the odd characters
/// they would otherwise type. A wheel turn they can't use goes on to the page behind them.
unsafe extern "system" fn edit_keys_proc(hwnd: HWND, message: u32, wparam: WPARAM, lparam: LPARAM, _id: usize, _data: usize) -> LRESULT {
    unsafe {
        match message {
            WM_CHAR if matches!(wparam.0, 0x7F | 0x01) => return LRESULT(0),
            WM_KEYDOWN if GetKeyState(VK_CONTROL.0 as i32) < 0 && GetKeyState(VK_MENU.0 as i32) >= 0 => {
                if wparam.0 == VK_BACK.0 as usize {
                    delete_word_before_caret(hwnd);
                    return LRESULT(0);
                }
                if wparam.0 == 'A' as usize {
                    SendMessageW(hwnd, EM_SETSEL_MSG, Some(WPARAM(0)), Some(LPARAM(-1)));
                    return LRESULT(0);
                }
            }
            WM_MOUSEWHEEL if !edit_overflows(hwnd) => {
                if let Ok(parent) = GetParent(hwnd) {
                    return SendMessageW(parent, message, Some(wparam), Some(lparam));
                }
            }
            WM_NCDESTROY => {
                let _ = RemoveWindowSubclass(hwnd, Some(edit_keys_proc), EDIT_KEYS_SUBCLASS);
            }
            _ => {}
        }
        DefSubclassProc(hwnd, message, wparam, lparam)
    }
}

/// A list with nothing to scroll passes the wheel on to the page behind it.
unsafe extern "system" fn list_wheel_proc(hwnd: HWND, message: u32, wparam: WPARAM, lparam: LPARAM, _id: usize, _data: usize) -> LRESULT {
    unsafe {
        match message {
            WM_MOUSEWHEEL if !list_overflows(hwnd) => {
                if let Ok(parent) = GetParent(hwnd) {
                    return SendMessageW(parent, message, Some(wparam), Some(lparam));
                }
            }
            WM_NCDESTROY => {
                let _ = RemoveWindowSubclass(hwnd, Some(list_wheel_proc), LIST_WHEEL_SUBCLASS);
            }
            _ => {}
        }
        DefSubclassProc(hwnd, message, wparam, lparam)
    }
}

pub fn list_overflows(list: HWND) -> bool {
    unsafe {
        let count = SendMessageW(list, LB_GETCOUNT, None, None).0.max(0) as i32;
        let height = SendMessageW(list, LB_GETITEMHEIGHT, None, None).0.max(0) as i32;
        let mut client = RECT::default();
        let _ = GetClientRect(list, &mut client);
        count * height > client.bottom
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum ButtonKind {
    Normal = 1,
    Primary = 2,
    Danger = 3,
    Switch = 4,
}

impl ButtonKind {
    fn from_data(value: isize) -> Option<ButtonKind> {
        match value {
            1 => Some(ButtonKind::Normal),
            2 => Some(ButtonKind::Primary),
            3 => Some(ButtonKind::Danger),
            4 => Some(ButtonKind::Switch),
            _ => None,
        }
    }
}

/// Fonts, brushes and painting shared by a form's controls. Owned by its window.
pub struct Style {
    pub theme: Theme,
    pub dpi: u32,
    pub font: HFONT,
    pub bold: HFONT,
    pub title: HFONT,
    pub small: HFONT,
    /// Sentences that explain something. Small text is for labels, and tiring to read at length.
    pub note: HFONT,
    pub icon: HFONT,
    pub icon_small: HFONT,
    /// The symbol on a page's badge.
    pub badge: HFONT,
    field: HBRUSH,
    card: HBRUSH,
    pub side: HBRUSH,
    /// Text boxes whose rounded field the parent paints.
    fields: Vec<HWND>,
    /// The ones of those with a magnifying glass.
    search_fields: Vec<HWND>,
    cards: Vec<Card>,
    /// Focus rings show only once the keyboard has moved focus, not after a click.
    pub focus_by_keyboard: bool,
    /// Text and text boxes saying something's wrong.
    alerts: Vec<HWND>,
    /// Text about something that's off. A disabled static control draws itself embossed, so this
    /// only changes its colour.
    dimmed: Vec<HWND>,
    /// A line under a switch's name, saying what it does, drawn by the switch itself.
    switch_notes: Vec<(HWND, String)>,
    /// The window whose controls sit on the side rail, and the rail's colour.
    rail: Option<(HWND, crate::gfx::Rgb, HBRUSH)>,
    /// Coloured squares with a symbol, beside a page's title.
    badges: Vec<Badge>,
    /// A row outlined for a moment, because a search just went to it: the window it's on, and
    /// where, in that window's coordinates.
    pub flash: Option<(HWND, RECT)>,
}

/// A page's coloured square, painted by the window behind it while `anchor` shows.
struct Badge {
    rect: RECT,
    anchor: HWND,
    parent: HWND,
    color: crate::gfx::Rgb,
    glyph: char,
}

/// The colour of a page's badge, or of its square in the page list. In high contrast it's the
/// text colour, so nothing depends on a colour Windows didn't choose.
pub fn badge_fill(theme: &Theme, color: crate::gfx::Rgb) -> (crate::gfx::Rgb, crate::gfx::Rgb) {
    if theme.high_contrast { (theme.text, theme.background) } else { (color, crate::gfx::Rgb(255, 255, 255)) }
}

/// A rounded square in a page's colour with its symbol on it, `size` across, at `x`, `y`.
pub fn draw_badge(buffer: &mut Backbuffer, theme: &Theme, font: HFONT, x: i32, y: i32, size: i32, color: crate::gfx::Rgb, glyph: char) {
    let (fill, ink) = badge_fill(theme, color);
    let radius = (size as f32 * 0.26).max(3.0);
    if theme.high_contrast {
        buffer.fill_round_rect(x, y, size, size, radius, fill);
    } else {
        // Lit like the launcher's tiles.
        let rim = (size as f32 / 22.0).max(1.0);
        buffer.fill_tile(x, y, size, radius, rim, fill.mix(crate::gfx::Rgb(255, 255, 255), 0.2), fill.mix(crate::gfx::Rgb(0, 0, 0), 0.18));
    }
    let symbol = (size as f32 * 0.6).round() as i32;
    buffer.symbol_centered(&glyph.to_string(), font, symbol, !theme.high_contrast, ink, RECT { left: x, top: y, right: x + size, bottom: y + size });
}

/// A rounded group of controls, painted by the window behind them. It shows while `anchor` does,
/// which is how it follows its page.
struct Card {
    rect: RECT,
    anchor: HWND,
    /// The window it's painted on. `rect` is in this window's coordinates.
    parent: HWND,
    /// Lines between rows, as y positions.
    lines: Vec<i32>,
}

impl Style {
    pub fn new(theme: Theme, dpi: u32) -> Style {
        let px = |value: i32| scale(value, dpi);
        unsafe {
            Style {
                theme,
                dpi,
                font: create_font(crate::gfx::ui_face(), px(14), FW_NORMAL.0 as i32),
                bold: create_font(crate::gfx::ui_face(), px(14), FW_SEMIBOLD.0 as i32),
                title: create_font(crate::gfx::ui_face(), px(22), FW_SEMIBOLD.0 as i32),
                small: create_font(crate::gfx::ui_face(), px(12), FW_NORMAL.0 as i32),
                note: create_font(crate::gfx::ui_face(), px(13), FW_NORMAL.0 as i32),
                icon: create_icon_font(px(16)),
                icon_small: create_icon_font(px(11)),
                badge: create_icon_font(px(20)),
                field: CreateSolidBrush(theme.field.colorref()),
                card: CreateSolidBrush(theme.card.colorref()),
                side: CreateSolidBrush(theme.sidebar.colorref()),
                fields: Vec::new(),
                search_fields: Vec::new(),
                cards: Vec::new(),
                focus_by_keyboard: true,
                alerts: Vec::new(),
                dimmed: Vec::new(),
                switch_notes: Vec::new(),
                rail: None,
                badges: Vec::new(),
                flash: None,
            }
        }
    }

    /// The row of a showing card on `parent` that `y` is in: from the line above it, or the
    /// card's top, to the line below it, or the card's bottom.
    pub fn row_around(&self, parent: HWND, y: i32) -> Option<RECT> {
        let card = self.cards.iter().find(|card| {
            card.parent == parent && card.rect.top <= y && y < card.rect.bottom && unsafe { IsWindowVisible(card.anchor) }.as_bool()
        })?;
        let top = card.lines.iter().copied().filter(|line| *line <= y).max().unwrap_or(card.rect.top);
        let bottom = card.lines.iter().copied().filter(|line| *line > y).min().unwrap_or(card.rect.bottom);
        Some(RECT { left: card.rect.left, top, right: card.rect.right, bottom })
    }

    /// Controls on this window sit on a rail of this colour, rather than the page's.
    pub fn set_rail(&mut self, parent: HWND, color: crate::gfx::Rgb) {
        unsafe {
            if let Some((_, _, old)) = self.rail.take() {
                let _ = DeleteObject(old.into());
            }
            self.rail = Some((parent, color, CreateSolidBrush(color.colorref())));
        }
    }

    pub fn rail_brush(&self) -> Option<HBRUSH> {
        self.rail.map(|rail| rail.2)
    }

    pub fn is_alert(&self, control: HWND) -> bool {
        self.alerts.contains(&control)
    }

    /// Shows this text as being about something that's off.
    pub fn set_dimmed(&mut self, control: HWND, on: bool) {
        self.dimmed.retain(|existing| *existing != control);
        if on {
            self.dimmed.push(control);
        }
    }

    /// Shows this text, or the border of this box, as a problem.
    pub fn set_alert(&mut self, control: HWND, on: bool) {
        self.alerts.retain(|existing| *existing != control);
        if on {
            self.alerts.push(control);
        }
    }

    /// Something on a page got `delta` taller at `y`: the card around it grows, and what's below
    /// moves down. `page` is that page's controls, because pages share a window and coordinates.
    pub fn shift_cards(&mut self, page: &[HWND], y: i32, delta: i32) {
        for badge in &mut self.badges {
            if page.contains(&badge.anchor) && badge.rect.top >= y {
                badge.rect.top += delta;
                badge.rect.bottom += delta;
            }
        }
        for card in &mut self.cards {
            if !page.contains(&card.anchor) {
                continue;
            }
            if card.rect.top >= y {
                card.rect.top += delta;
                card.rect.bottom += delta;
            } else if card.rect.bottom >= y {
                card.rect.bottom += delta;
            } else {
                continue;
            }
            for line in &mut card.lines {
                if *line >= y {
                    *line += delta;
                }
            }
        }
    }

    pub fn is_dark(&self) -> bool {
        self.theme.background.0 < 128
    }

    pub fn px(&self, value: i32) -> i32 {
        scale(value, self.dpi)
    }

    fn field_rect(&self, control: HWND) -> RECT {
        let mut rect = RECT::default();
        unsafe {
            let parent = GetParent(control).unwrap_or_default();
            let _ = GetWindowRect(control, &mut rect);
            let mut origin = POINT { x: rect.left, y: rect.top };
            let _ = windows::Win32::Graphics::Gdi::ScreenToClient(parent, &mut origin);
            let (width, height) = (rect.right - rect.left, rect.bottom - rect.top);
            let (pad_x, pad_y) = (self.px(8), self.px(4));
            let glass = if self.search_fields.contains(&control) { self.px(SEARCH_GLASS_DIP) } else { 0 };
            RECT { left: origin.x - pad_x - glass, top: origin.y - pad_y, right: origin.x + width + pad_x, bottom: origin.y + height + pad_y }
        }
    }

    /// True if this control sits on a card that's showing.
    fn on_card(&self, control: HWND) -> bool {
        unsafe {
            let Ok(parent) = GetParent(control) else { return false };
            let mut rect = RECT::default();
            let _ = GetWindowRect(control, &mut rect);
            let mut middle = POINT { x: (rect.left + rect.right) / 2, y: (rect.top + rect.bottom) / 2 };
            let _ = windows::Win32::Graphics::Gdi::ScreenToClient(parent, &mut middle);
            self.cards.iter().any(|card| {
                // A window's own coordinates mean nothing on another window's card.
                card.parent == parent
                    && IsWindowVisible(card.anchor).as_bool()
                    && middle.x >= card.rect.left
                    && middle.x < card.rect.right
                    && middle.y >= card.rect.top
                    && middle.y < card.rect.bottom
            })
        }
    }

    fn surface_of(&self, control: HWND) -> (crate::gfx::Rgb, HBRUSH) {
        if let Some((parent, color, brush)) = self.rail {
            if unsafe { GetParent(control) }.ok() == Some(parent) {
                return (color, brush);
            }
        }
        if self.on_card(control) { (self.theme.card, self.card) } else { (self.theme.sidebar, self.side) }
    }

    /// Paints the window: `decorate` first, then the fields. Children draw themselves on top.
    pub fn paint(&self, hwnd: HWND, decorate: impl FnOnce(&mut Backbuffer)) {
        unsafe {
            let mut ps = PAINTSTRUCT::default();
            let dc = BeginPaint(hwnd, &mut ps);
            let mut client = RECT::default();
            let _ = windows::Win32::UI::WindowsAndMessaging::GetClientRect(hwnd, &mut client);
            if let Some(mut buffer) = Backbuffer::new(client.right, client.bottom) {
                buffer.fill(self.theme.sidebar);
                decorate(&mut buffer);
                for card in &self.cards {
                    if card.parent != hwnd || !IsWindowVisible(card.anchor).as_bool() {
                        continue;
                    }
                    let (width, height) = (card.rect.right - card.rect.left, card.rect.bottom - card.rect.top);
                    let radius = self.px(RADIUS_CARD) as f32;
                    let inset = 1.max(self.dpi as i32 / 96);
                    buffer.fill_round_rect(card.rect.left, card.rect.top, width, height, radius, self.theme.divider);
                    buffer.fill_round_rect(card.rect.left + inset, card.rect.top + inset, width - inset * 2, height - inset * 2, radius - inset as f32, self.theme.card);
                    let pad = self.px(CARD_PAD_DIP);
                    for line in &card.lines {
                        buffer.fill_rect(card.rect.left + pad, *line, width - pad * 2, inset, self.theme.divider);
                    }
                }
                // In the space between a row's edge and its controls, so they don't cover it.
                if let Some((parent, rect)) = self.flash
                    && parent == hwnd
                {
                    let (inset, thickness) = (self.px(2), self.px(2).max(2));
                    let (width, height) = (rect.right - rect.left - inset * 2, rect.bottom - rect.top - inset * 2);
                    let color = if self.theme.high_contrast { self.theme.text } else { self.theme.accent };
                    buffer.stroke_round_rect_alpha(rect.left + inset, rect.top + inset, width, height, self.px(RADIUS_CARD - 3) as f32, thickness as f32, color, 255);
                }
                for badge in &self.badges {
                    if badge.parent != hwnd || !IsWindowVisible(badge.anchor).as_bool() {
                        continue;
                    }
                    draw_badge(&mut buffer, &self.theme, self.badge, badge.rect.left, badge.rect.top, badge.rect.right - badge.rect.left, badge.color, badge.glyph);
                }
                let focus = GetFocus();
                for control in &self.fields {
                    if GetParent(*control).ok() != Some(hwnd) || !IsWindowVisible(*control).as_bool() {
                        continue;
                    }
                    let rect = self.field_rect(*control);
                    let (width, height) = (rect.right - rect.left, rect.bottom - rect.top);
                    let radius = self.px(RADIUS_FIELD) as f32;
                    let border = if self.alerts.contains(control) {
                        self.theme.danger
                    } else if focus == *control {
                        self.theme.accent
                    } else {
                        self.theme.divider
                    };
                    buffer.fill_round_rect(rect.left, rect.top, width, height, radius, border);
                    let inset = 1.max(self.dpi as i32 / 96);
                    buffer.fill_round_rect(rect.left + inset, rect.top + inset, width - inset * 2, height - inset * 2, radius - inset as f32, self.theme.field);
                    if self.search_fields.contains(control) {
                        let glass = RECT { left: rect.left + self.px(4), top: rect.top, right: rect.left + self.px(8 + SEARCH_GLASS_DIP), bottom: rect.bottom };
                        buffer.symbol_centered("\u{E721}", self.icon_small, self.px(12), false, self.theme.secondary, glass);
                    }
                }
                buffer.copy_to_dc(dc);
            }
            let _ = EndPaint(hwnd, &ps);
        }
    }

    /// The colour and drawing messages every form treats the same way. `None` means not handled.
    pub fn handle(&self, _hwnd: HWND, message: u32, wparam: WPARAM, lparam: LPARAM) -> Option<LRESULT> {
        unsafe {
            match message {
                WM_ERASEBKGND => Some(LRESULT(1)),
                WM_CTLCOLORSTATIC | WM_CTLCOLORBTN => {
                    let dc = HDC(wparam.0 as *mut _);
                    let control = HWND(lparam.0 as *mut _);
                    let id = GetDlgCtrlID(control);
                    let color = if self.alerts.contains(&control) {
                        self.theme.danger
                    } else if self.dimmed.contains(&control) || !IsWindowEnabled(control).as_bool() {
                        self.theme.tertiary
                    } else {
                        match id {
                            ID_NOTE => self.theme.secondary,
                            ID_STATUS_TEXT => self.theme.accent,
                            _ => self.theme.text,
                        }
                    };
                    // A text box that's turned off asks here, but it still sits on its field.
                    if self.fields.contains(&control) {
                        SetTextColor(dc, color.colorref());
                        SetBkColor(dc, self.theme.field.colorref());
                        return Some(LRESULT(self.field.0 as isize));
                    }
                    let (surface, brush) = self.surface_of(control);
                    SetTextColor(dc, color.colorref());
                    SetBkColor(dc, surface.colorref());
                    Some(LRESULT(brush.0 as isize))
                }
                WM_CTLCOLOREDIT => {
                    let dc = HDC(wparam.0 as *mut _);
                    SetTextColor(dc, self.theme.text.colorref());
                    SetBkColor(dc, self.theme.field.colorref());
                    Some(LRESULT(self.field.0 as isize))
                }
                WM_CTLCOLORLISTBOX => {
                    // The list of an open drop-down isn't on a card, and looks like a text box.
                    let control = HWND(lparam.0 as *mut _);
                    let (surface, brush) = if self.on_card(control) { (self.theme.card, self.card) } else { (self.theme.field, self.field) };
                    let dc = HDC(wparam.0 as *mut _);
                    SetTextColor(dc, self.theme.text.colorref());
                    SetBkColor(dc, surface.colorref());
                    Some(LRESULT(brush.0 as isize))
                }
                WM_COMMAND => {
                    // A text box's border follows the keyboard focus.
                    let notification = (wparam.0 >> 16) as u32;
                    let control = HWND(lparam.0 as *mut _);
                    if (notification == EN_SETFOCUS || notification == EN_KILLFOCUS) && self.fields.contains(&control) {
                        // The field is painted by the window the box sits on.
                        let rect = self.field_rect(control);
                        let _ = InvalidateRect(GetParent(control).ok(), Some(&rect), false);
                    }
                    if notification == EN_CHANGE {
                        sync_scroll_bar(control);
                    }
                    if notification == CBN_SELCHANGE || notification == CBN_CLOSEUP || notification == CBN_DROPDOWN {
                        let _ = InvalidateRect(Some(control), None, false);
                    }
                    None
                }
                WM_DRAWITEM => {
                    let item = &*(lparam.0 as *const DRAWITEMSTRUCT);
                    if item.CtlType != ODT_COMBOBOX {
                        return None;
                    }
                    self.draw_combo_item(item);
                    Some(LRESULT(1))
                }
                WM_NOTIFY => {
                    let header = &*(lparam.0 as *const NMHDR);
                    if header.code != NM_CUSTOMDRAW {
                        return None;
                    }
                    let kind = ButtonKind::from_data(GetWindowLongPtrW(header.hwndFrom, GWLP_USERDATA))?;
                    let draw = &*(lparam.0 as *const NMCUSTOMDRAW);
                    if draw.dwDrawStage != CDDS_PREPAINT {
                        return None;
                    }
                    self.draw_button(draw, kind);
                    Some(LRESULT(CDRF_SKIPDEFAULT as isize))
                }
                _ => None,
            }
        }
    }

    fn draw_button(&self, draw: &NMCUSTOMDRAW, kind: ButtonKind) {
        let (width, height) = (draw.rc.right - draw.rc.left, draw.rc.bottom - draw.rc.top);
        let Some(mut buffer) = Backbuffer::new(width, height) else { return };
        let theme = &self.theme;
        let (surface, _) = self.surface_of(draw.hdr.hwndFrom);
        buffer.fill(surface);
        let hot = draw.uItemState.contains(CDIS_HOT);
        let pressed = draw.uItemState.contains(CDIS_SELECTED);
        let focused = draw.uItemState.contains(CDIS_FOCUS) && self.focus_by_keyboard;
        let text = get_text(draw.hdr.hwndFrom);
        let radius = self.px(RADIUS_FIELD) as f32;
        if kind == ButtonKind::Switch {
            let disabled = draw.uItemState.contains(CDIS_DISABLED);
            if hot && !disabled {
                buffer.fill_round_rect(0, 0, width, height, radius, theme.selection);
            }
            let on = is_checked(draw.hdr.hwndFrom);
            let (switch_width, switch_height) = (self.px(36), self.px(20));
            let x = width - switch_width - self.px(8);
            let y = (height - switch_height) / 2;
            if focused {
                let ring = self.px(2);
                buffer.fill_round_rect(x - ring, y - ring, switch_width + ring * 2, switch_height + ring * 2, (switch_height / 2 + ring) as f32, theme.text);
                buffer.fill_round_rect(x - ring / 2, y - ring / 2, switch_width + ring, switch_height + ring, (switch_height / 2) as f32, if hot { theme.selection } else { surface });
            }
            self.draw_switch(&mut buffer, x, y, on, disabled, surface);
            let name_ink = if disabled { theme.tertiary } else { theme.text };
            match self.switch_notes.iter().find(|(control, _)| *control == draw.hdr.hwndFrom) {
                Some((_, note)) => {
                    let (top, name_height) = (self.px(SWITCH_PAD_DIP), self.px(20));
                    buffer.text(&text, self.font, name_ink, RECT { left: self.px(8), top, right: x - self.px(16), bottom: top + name_height }, false);
                    let ink = if disabled { theme.tertiary } else { theme.secondary };
                    buffer.text_wrapped(note, self.note, ink, RECT { left: self.px(8), top: top + name_height, right: x - self.px(16), bottom: height });
                }
                None => {
                    let rect = RECT { left: self.px(8), top: 0, right: x - self.px(10), bottom: height };
                    buffer.text(&text, self.font, name_ink, rect, false);
                }
            }
        } else {
            // Waiting for a second press, so it looks like what it's about to do.
            let (fill, ink) = match kind {
                _ if self.is_alert(draw.hdr.hwndFrom) => (theme.danger, crate::ui::ink_on(theme.danger)),
                ButtonKind::Primary => (theme.accent, theme.on_accent),
                ButtonKind::Danger => (theme.keycap, theme.danger),
                _ => (theme.keycap, theme.text),
            };
            let disabled = draw.uItemState.contains(CDIS_DISABLED);
            let ink = if disabled { theme.secondary } else { ink };
            let fill = if disabled {
                fill.mix(surface, 0.5)
            } else if pressed {
                fill.mix(surface, 0.25)
            } else if hot {
                fill.mix(theme.text, 0.1)
            } else {
                fill
            };
            if focused {
                let ring = 1.max(self.dpi as i32 / 96);
                buffer.fill_round_rect(0, 0, width, height, radius, theme.text);
                buffer.fill_round_rect(ring, ring, width - ring * 2, height - ring * 2, radius - ring as f32, fill);
            } else {
                buffer.fill_round_rect(0, 0, width, height, radius, fill);
            }
            buffer.text_centered(&text, self.font, ink, RECT { left: 0, top: 0, right: width, bottom: height });
        }
        buffer.copy_to_dc_at(draw.hdc, draw.rc.left, draw.rc.top);
    }

    /// A switch's track and knob, 36 by 20 DIP, with its top left at `x`, `y`.
    pub fn draw_switch(&self, buffer: &mut Backbuffer, x: i32, y: i32, on: bool, disabled: bool, surface: crate::gfx::Rgb) {
        let theme = &self.theme;
        let (switch_width, switch_height) = (self.px(36), self.px(20));
        let track = if on { theme.accent } else { theme.keycap };
        let track = if disabled { track.mix(surface, 0.55) } else { track };
        buffer.fill_round_rect(x, y, switch_width, switch_height, (switch_height / 2) as f32, track);
        let knob = switch_height - self.px(6);
        let knob_x = if on { x + switch_width - knob - self.px(3) } else { x + self.px(3) };
        // White reads as a switch on a light surface, where a dark knob looks heavy.
        let plain = if on { theme.on_accent } else { theme.secondary };
        let knob_fill = if self.is_dark() || theme.high_contrast { plain } else { crate::gfx::Rgb(255, 255, 255) };
        let knob_fill = if disabled { knob_fill.mix(surface, 0.45) } else { knob_fill };
        buffer.fill_round_rect(knob_x, y + self.px(3), knob, knob, (knob / 2) as f32, knob_fill);
    }

    /// A row of an open drop-down. The closed face is painted by `combo_proc`.
    fn draw_combo_item(&self, item: &DRAWITEMSTRUCT) {
        if item.itemID == u32::MAX || item.itemState.0 & ODS_COMBOBOXEDIT.0 != 0 {
            return;
        }
        let (width, height) = (item.rcItem.right - item.rcItem.left, item.rcItem.bottom - item.rcItem.top);
        let Some(mut buffer) = Backbuffer::new(width, height) else { return };
        let theme = &self.theme;
        buffer.fill(theme.field);
        if item.itemState.0 & ODS_SELECTED.0 != 0 {
            buffer.fill_round_rect(self.px(2), self.px(1), width - self.px(4), height - self.px(2), self.px(RADIUS_ROW) as f32, theme.selection);
        }
        let text = combo_item_text(item.hwndItem, item.itemID as usize);
        buffer.text(&text, self.font, theme.text, RECT { left: self.px(10), top: 0, right: width - self.px(6), bottom: height }, false);
        buffer.copy_to_dc_at(item.hDC, item.rcItem.left, item.rcItem.top);
    }

    /// Dark title bar and scroll bars to match, on Windows versions that have them.
    pub fn apply_to_window(&self, hwnd: HWND) {
        crate::editor::apply_title_bar_theme(hwnd, &self.theme);
    }
}

impl Drop for Style {
    fn drop(&mut self) {
        for font in [self.font, self.bold, self.title, self.small, self.note, self.icon, self.icon_small, self.badge] {
            delete_font(font);
        }
        unsafe {
            if let Some((_, _, brush)) = self.rail.take() {
                let _ = DeleteObject(brush.into());
            }
            for brush in [self.field, self.card, self.side] {
                let _ = DeleteObject(brush.into());
            }
        }
    }
}

/// A card being built.
struct OpenCard {
    top: i32,
    first_control: usize,
    lines: Vec<i32>,
}

/// A row a form's search can find: its name, what's said under it, and where it is.
#[derive(Clone, Debug, Default)]
pub struct Findable {
    pub title: String,
    pub note: String,
    /// The heading it's under, if any.
    pub heading: String,
    /// Other words that find it, such as a drop-down's choices or a button's name.
    pub words: String,
    /// What to scroll to: the row's name or its control.
    pub anchor: HWND,
}

pub struct Builder<'a> {
    pub parent: HWND,
    pub style: &'a mut Style,
    pub x: i32,
    pub y: i32,
    pub width: i32,
    /// Every control created since the last `take_controls`.
    pub created: Vec<HWND>,
    /// Every row with a name since the last `take_found`.
    found: Vec<Findable>,
    /// The heading rows are under now.
    found_heading: String,
    /// The row a note straight after it belongs to.
    note_for: Option<usize>,
    open_card: Option<OpenCard>,
    /// Space left after the last row, which a card's edge or line replaces with its own.
    trailing: i32,
    /// Where the last switch ended, so that a note right after it can sit close.
    switch_end: i32,
    /// The name beside the row last added, so that it can be dimmed with its control.
    pub last_label: HWND,
    /// The last row with a name on the left and a control on the right, where it started, and
    /// where it ended. A note right after it goes under the name.
    last_row: Option<(HWND, HWND, i32, i32)>,
    /// The last switch, and where it started.
    last_switch: Option<(HWND, i32)>,
}

/// Space above and below a row's name, when a note runs under it.
const SWITCH_PAD_DIP: i32 = 10;

impl<'a> Builder<'a> {
    pub fn new(parent: HWND, style: &'a mut Style, x: i32, y: i32, width: i32) -> Builder<'a> {
        Builder {
            parent,
            style,
            x,
            y,
            width,
            created: Vec::new(),
            found: Vec::new(),
            found_heading: String::new(),
            note_for: None,
            open_card: None,
            trailing: 0,
            switch_end: -1,
            last_label: HWND::default(),
            last_row: None,
            last_switch: None,
        }
    }

    /// Remembers a row for searching. A note right after it is what's said under it.
    fn findable(&mut self, title: &str, anchor: HWND, words: &str) {
        self.note_for = Some(self.found.len());
        self.found.push(Findable { title: title.to_string(), note: String::new(), heading: self.found_heading.clone(), words: words.to_string(), anchor });
    }

    /// The rows made since the last call, in order, for a search to find.
    pub fn take_found(&mut self) -> Vec<Findable> {
        self.note_for = None;
        self.found_heading.clear();
        std::mem::take(&mut self.found)
    }

    fn advance(&mut self, dip: i32) {
        let gap = self.px(dip);
        self.y += gap;
        self.trailing = gap;
    }

    /// Starts a group of rows on a raised surface. The first control created inside decides when
    /// it shows.
    pub fn begin_card(&mut self) {
        let pad = self.px(CARD_PAD_DIP);
        self.note_for = None;
        self.open_card = Some(OpenCard { top: self.y, first_control: self.created.len(), lines: Vec::new() });
        self.x += pad;
        self.width -= pad * 2;
        self.y += self.px(8);
        self.trailing = 0;
    }

    /// A thin line between two rows of a card.
    pub fn line(&mut self) {
        self.note_for = None;
        self.y += self.px(6) - self.trailing;
        let y = self.y;
        if let Some(card) = &mut self.open_card {
            card.lines.push(y);
        }
        self.y += self.px(6);
        self.trailing = 0;
    }

    pub fn end_card(&mut self) {
        self.note_for = None;
        let Some(card) = self.open_card.take() else { return };
        let pad = self.px(CARD_PAD_DIP);
        self.y += self.px(8) - self.trailing;
        self.x -= pad;
        self.width += pad * 2;
        if let Some(anchor) = self.created.get(card.first_control) {
            let rect = RECT { left: self.x, top: card.top, right: self.x + self.width, bottom: self.y };
            self.style.cards.push(Card { rect, anchor: *anchor, parent: self.parent, lines: card.lines });
        }
        self.advance(12);
    }

    /// A box to search in, with a magnifying glass.
    pub fn search_box(&mut self, hint: &str, x: i32, y: i32, width: i32, id: i32) -> HWND {
        let pad = self.px(8);
        let glass = self.px(SEARCH_GLASS_DIP);
        let hwnd = self.line_edit("", x + pad + glass, y + self.px(4), width - pad * 2 - glass, id);
        self.style.search_fields.push(hwnd);
        self.hint(hwnd, hint);
        hwnd
    }

    fn create(&mut self, class: PCWSTR, text: &str, style: WINDOW_STYLE, rect: (i32, i32, i32, i32), id: i32, font: HFONT) -> HWND {
        let wide_text = wide(text);
        let hwnd = unsafe {
            CreateWindowExW(
                WINDOW_EX_STYLE(0),
                class,
                PCWSTR(wide_text.as_ptr()),
                WS_CHILD | WS_VISIBLE | style,
                rect.0,
                rect.1,
                rect.2,
                rect.3,
                Some(self.parent),
                Some(HMENU(id as isize as *mut _)),
                None,
                None,
            )
        }
        .unwrap_or_default();
        unsafe {
            SendMessageW(hwnd, WM_SETFONT, Some(WPARAM(font.0 as usize)), Some(LPARAM(1)));
        }
        self.created.push(hwnd);
        hwnd
    }

    pub fn px(&self, value: i32) -> i32 {
        scale(value, self.style.dpi)
    }

    pub fn gap(&mut self, dip: i32) {
        self.y += self.px(dip);
    }

    fn text_height(&self, text: &str, font: HFONT, width: i32) -> i32 {
        // With no characters there is no real address to hand to DrawTextW.
        if text.is_empty() {
            return 0;
        }
        let mut units: Vec<u16> = text.encode_utf16().collect();
        let mut rect = RECT { left: 0, top: 0, right: width, bottom: 0 };
        unsafe {
            let dc = GetDC(Some(self.parent));
            let previous = SelectObject(dc, font.into());
            DrawTextW(dc, &mut units, &mut rect, DT_CALCRECT | DT_WORDBREAK | DT_NOPREFIX);
            SelectObject(dc, previous);
            ReleaseDC(Some(self.parent), dc);
        }
        rect.bottom
    }

    fn static_text(&mut self, text: &str, font: HFONT, id: i32) -> HWND {
        let height = self.text_height(text, font, self.width).max(self.px(18));
        let rect = (self.x, self.y, self.width, height);
        let style = WINDOW_STYLE(SS_NOPREFIX.0);
        let hwnd = self.create(w!("STATIC"), text, style, rect, id, font);
        self.y += height;
        hwnd
    }

    /// The name of the page.
    pub fn title(&mut self, text: &str) -> HWND {
        let font = self.style.title;
        let hwnd = self.static_text(text, font, 0);
        self.advance(14);
        hwnd
    }

    /// The top of a page: its badge, its name, and a line on what it's for. Gives the name and
    /// the line.
    pub fn page_header(&mut self, title: &str, about: &str, glyph: char, color: crate::gfx::Rgb) -> (HWND, HWND) {
        let size = self.px(44);
        let gap = self.px(14);
        let left = self.x + size + gap;
        let width = self.width - size - gap;
        // Geist's line box is a little tall, so the line under the title sits 2px closer.
        let title_height = self.px(28);
        let about_height = self.text_height(about, self.style.note, width).max(self.px(18));
        let height = (title_height + about_height).max(size);
        let top = self.y + (height - title_height - about_height) / 2;
        let style = WINDOW_STYLE(SS_NOPREFIX.0);
        let title_font = self.style.title;
        let name = self.create(w!("STATIC"), title, style, (left, top, width, title_height), 0, title_font);
        let note_font = self.style.note;
        let line = self.create(w!("STATIC"), about, style, (left, top + title_height, width, about_height), ID_NOTE, note_font);
        self.style.badges.push(Badge { rect: RECT { left: self.x, top: self.y + (height - size) / 2, right: self.x + size, bottom: self.y + (height - size) / 2 + size }, anchor: name, parent: self.parent, color, glyph });
        self.y += height;
        self.advance(18);
        self.found_heading.clear();
        self.note_for = None;
        (name, line)
    }

    /// Starts a group of related settings.
    pub fn heading(&mut self, text: &str) -> HWND {
        self.gap(4);
        let font = self.style.bold;
        let hwnd = self.static_text(text, font, 0);
        self.advance(8);
        self.found_heading = text.to_string();
        self.note_for = None;
        hwnd
    }

    /// A plain caption for what's below it.
    pub fn caption(&mut self, text: &str) -> HWND {
        let font = self.style.font;
        let hwnd = self.static_text(text, font, 0);
        self.advance(6);
        self.findable(text, hwnd, "");
        hwnd
    }

    /// Quieter text that explains something, wrapped to the column. Right after a switch it's
    /// drawn by the switch, under its name, and that switch is what's given back. Right after a
    /// drop-down, it goes under the drop-down's name.
    pub fn note(&mut self, text: &str) -> HWND {
        if let Some(row) = self.note_for.take().and_then(|at| self.found.get_mut(at)) {
            row.note = text.to_string();
        }
        if self.y == self.switch_end {
            if let Some((switch, top)) = self.last_switch.take() {
                return self.note_on_switch(switch, top, text);
            }
        }
        if let Some((label, control, top, end)) = self.last_row.take() {
            if self.y == end {
                return self.note_on_row(label, control, top, text);
            }
        }
        let font = self.style.note;
        let hwnd = self.static_text(text, font, ID_NOTE);
        self.advance(10);
        hwnd
    }

    fn note_on_switch(&mut self, switch: HWND, top: i32, text: &str) -> HWND {
        let room = self.width + self.px(16) - self.px(8) - self.px(36 + 8 + 16);
        let note_height = self.text_height(text, self.style.note, room);
        let height = self.px(SWITCH_PAD_DIP) * 2 + self.px(20) + note_height;
        unsafe {
            let _ = SetWindowPos(switch, None, 0, 0, self.width + self.px(16), height, SWP_NOMOVE | SWP_NOZORDER | SWP_NOACTIVATE);
        }
        self.style.switch_notes.push((switch, text.to_string()));
        self.y = top + height;
        self.advance(2);
        self.switch_end = self.y;
        switch
    }

    fn note_on_row(&mut self, label: HWND, control: HWND, top: i32, text: &str) -> HWND {
        let mut bounds = RECT::default();
        unsafe {
            let _ = GetWindowRect(label, &mut bounds);
        }
        let label_width = bounds.right - bounds.left;
        let pad = self.px(SWITCH_PAD_DIP) - self.px(4);
        let name_height = self.px(20);
        let note_height = self.text_height(text, self.style.note, label_width).max(self.px(17));
        let control_height = self.row_control_height(control);
        let height = (pad * 2 + name_height + note_height).max(control_height + pad * 2);
        let font = self.style.note;
        let style = WINDOW_STYLE(SS_NOPREFIX.0);
        let note = self.create(w!("STATIC"), text, style, (self.x, top + pad + name_height, label_width, note_height), ID_NOTE, font);
        unsafe {
            let _ = SetWindowPos(label, None, self.x, top + pad, label_width, name_height, SWP_NOZORDER | SWP_NOACTIVATE);
            let mut place = RECT::default();
            let _ = GetWindowRect(control, &mut place);
            let mut origin = POINT { x: place.left, y: place.top };
            let _ = windows::Win32::Graphics::Gdi::ScreenToClient(self.parent, &mut origin);
            let _ = SetWindowPos(control, None, origin.x, top + (height - control_height) / 2, 0, 0, SWP_NOSIZE | SWP_NOZORDER | SWP_NOACTIVATE);
        }
        self.y = top + height;
        self.advance(4);
        note
    }

    /// How tall a row's control looks: a drop-down's window is as tall as its open list.
    fn row_control_height(&self, control: HWND) -> i32 {
        unsafe {
            let mut info = COMBOBOXINFO { cbSize: std::mem::size_of::<COMBOBOXINFO>() as u32, ..Default::default() };
            if GetComboBoxInfo(control, &mut info).is_ok() {
                return info.rcItem.bottom - info.rcItem.top + self.px(8);
            }
            let mut place = RECT::default();
            let _ = GetWindowRect(control, &mut place);
            place.bottom - place.top
        }
    }

    /// Quiet text that changes later, with room saved for this many lines.
    pub fn note_area(&mut self, text: &str, lines: i32) -> HWND {
        let font = self.style.note;
        let height = self.px(18 * lines);
        // SS_EDITCONTROL wraps a long address that has no spaces, rather than cutting it off.
        let style = WINDOW_STYLE(SS_NOPREFIX.0 | 0x2000);
        let hwnd = self.create(w!("STATIC"), text, style, (self.x, self.y, self.width, height), ID_NOTE, font);
        self.y += height;
        self.advance(10);
        hwnd
    }

    /// Quiet text placed by hand, such as a window's status. It changes later.
    pub fn quiet_at(&mut self, x: i32, y: i32, width: i32, height: i32) -> HWND {
        let font = self.style.note;
        let style = WINDOW_STYLE(SS_NOPREFIX.0);
        self.create(w!("STATIC"), "", style, (x, y, width, height), ID_NOTE, font)
    }

    /// A control of a window class the form registers itself.
    pub fn custom(&mut self, class: PCWSTR, rect: (i32, i32, i32, i32), id: i32) -> HWND {
        let font = self.style.font;
        self.create(class, "", WS_TABSTOP, rect, id, font)
    }

    /// A row with a name on the left and a control placed by hand on the right. Gives the control's
    /// rectangle.
    pub fn labelled_row(&mut self, label: &str, width_dip: i32, height_dip: i32) -> (i32, i32, i32, i32) {
        let (width, height) = (self.control_width(width_dip), self.px(height_dip));
        self.row_label(label, self.width - width - self.px(12), height);
        self.findable(label, self.last_label, "");
        let rect = (self.x + self.width - width, self.y, width, height);
        self.y += height;
        self.advance(8);
        rect
    }

    /// A row of its own: a name, a line saying what it does, and a button. Gives the line and the
    /// button, for a row that changes.
    pub fn titled_action(&mut self, title: &str, text: &str, label: &str, id: i32, kind: ButtonKind) -> (HWND, HWND) {
        let button = self.px(ACTION_BUTTON_DIP);
        let text_width = self.width - button - self.px(16);
        let (font, small) = (self.style.font, self.style.note);
        let title_height = self.px(20);
        let text_height = self.text_height(text, small, text_width).max(self.px(18));
        let button_height = self.px(30);
        let height = (title_height + text_height).max(button_height);
        self.y += self.px(4);
        let top = self.y + (height - title_height - text_height) / 2;
        let style = WINDOW_STYLE(SS_NOPREFIX.0);
        let name = self.create(w!("STATIC"), title, style, (self.x, top, text_width, title_height), 0, font);
        let note = self.create(w!("STATIC"), text, style, (self.x, top + title_height, text_width, text_height), ID_NOTE, small);
        let hwnd = self.button_at(label, id, self.x + self.width - button, self.y + (height - button_height) / 2, button, kind);
        self.y += height;
        self.advance(14);
        self.findable(title, name, label.trim_end_matches('\u{2026}'));
        if let Some(row) = self.note_for.take().and_then(|at| self.found.get_mut(at)) {
            row.note = text.to_string();
        }
        (note, hwnd)
    }

    /// Text with no height yet, for a message that may never be needed. It makes room when it has
    /// something to say.
    pub fn collapsed_note(&mut self, id: i32) -> HWND {
        let font = self.style.note;
        let style = WINDOW_STYLE(SS_NOPREFIX.0 | 0x2000);
        // Whatever follows starts here, so that it moves down when this makes room.
        self.trailing = 0;
        self.create(w!("STATIC"), "", style, (self.x, self.y, self.width, 0), id, font)
    }

    /// A line for messages. Empty until there's something to say.
    pub fn status_at(&mut self, x: i32, y: i32, width: i32, height: i32) -> HWND {
        let font = self.style.small;
        let style = WINDOW_STYLE(SS_NOPREFIX.0);
        self.create(w!("STATIC"), "", style, (x, y, width, height), ID_STATUS_TEXT, font)
    }

    fn remember_field(&mut self, hwnd: HWND) {
        self.style.fields.push(hwnd);
        if self.style.is_dark() {
            unsafe {
                let _ = SetWindowTheme(hwnd, w!("DarkMode_Explorer"), PCWSTR::null());
            }
        }
    }

    fn row_label(&mut self, text: &str, width: i32, height: i32) {
        let font = self.style.font;
        let style = WINDOW_STYLE(SS_NOPREFIX.0 | SS_CENTERIMAGE.0);
        self.last_label = self.create(w!("STATIC"), text, style, (self.x, self.y, width, height), 0, font);
    }

    fn line_edit(&mut self, text: &str, x: i32, y: i32, width: i32, id: i32) -> HWND {
        let height = self.px(20);
        let style = WS_TABSTOP | WINDOW_STYLE(ES_AUTOHSCROLL as u32);
        let font = self.style.font;
        let hwnd = self.create(w!("EDIT"), text, style, (x, y, width, height), id, font);
        self.remember_field(hwnd);
        add_edit_keys(hwnd);
        hwnd
    }

    /// A text box across the column.
    pub fn edit(&mut self, text: &str, id: i32) -> HWND {
        let pad = self.px(8);
        let top = self.y + self.px(4);
        let hwnd = self.line_edit(text, self.x + pad, top, self.width - pad * 2, id);
        self.y += self.px(28) + self.px(12);
        hwnd
    }

    /// Grey text in an empty box, saying what leaving it empty means.
    pub fn hint(&self, edit: HWND, text: &str) {
        let text = wide(text);
        unsafe {
            SendMessageW(edit, EM_SETCUEBANNER, Some(WPARAM(1)), Some(LPARAM(text.as_ptr() as isize)));
        }
    }

    /// A name on the left and a text box of this width on the right.
    pub fn field(&mut self, label: &str, text: &str, id: i32, width_dip: i32) -> HWND {
        let row = self.px(28);
        let width = self.px(width_dip);
        self.row_label(label, self.width - width - self.px(12), row);
        let pad = self.px(8);
        let hwnd = self.line_edit(text, self.x + self.width - width + pad, self.y + self.px(4), width - pad * 2, id);
        self.y += row;
        self.advance(8);
        self.findable(label, hwnd, "");
        hwnd
    }

    /// A multi-line box, for tables and lists.
    pub fn text_area(&mut self, text: &str, rows: i32, id: i32) -> HWND {
        let (pad_x, pad_y) = (self.px(8), self.px(4));
        let height = self.px(19 * rows);
        let rect = (self.x + pad_x, self.y + pad_y, self.width - pad_x * 2, height);
        let style = WS_TABSTOP | WS_VSCROLL | WINDOW_STYLE((ES_MULTILINE | ES_AUTOVSCROLL | ES_WANTRETURN) as u32);
        let font = self.style.font;
        let hwnd = self.create(w!("EDIT"), &text.replace('\n', "\r\n"), style, rect, id, font);
        self.remember_field(hwnd);
        add_edit_keys(hwnd);
        sync_scroll_bar(hwnd);
        self.y += height + pad_y * 2;
        self.advance(10);
        hwnd
    }

    /// An on/off setting: the name on the left and a switch on the right. A real check box inside.
    pub fn switch(&mut self, text: &str, checked: bool, id: i32) -> HWND {
        let height = self.px(32);
        let rect = (self.x - self.px(8), self.y, self.width + self.px(16), height);
        let style = WS_TABSTOP | WINDOW_STYLE(BS_AUTOCHECKBOX as u32);
        let font = self.style.font;
        let hwnd = self.create(w!("BUTTON"), text, style, rect, id, font);
        set_checked(hwnd, checked);
        unsafe {
            SetWindowLongPtrW(hwnd, GWLP_USERDATA, ButtonKind::Switch as isize);
        }
        self.last_switch = Some((hwnd, self.y));
        self.y += height;
        self.advance(2);
        self.switch_end = self.y;
        self.findable(text, hwnd, "");
        hwnd
    }

    /// How wide a control beside a name is. In a narrow window the name still gets its share.
    fn control_width(&self, dip: i32) -> i32 {
        self.px(dip).min(self.width * 55 / 100)
    }

    /// How wide these choices are side by side, in pixels.
    fn segments_width(&self, items: &[&str]) -> Vec<i32> {
        let Some(buffer) = Backbuffer::new(1, 1) else { return Vec::new() };
        items.iter().map(|item| buffer.text_width(item, self.style.bold) + self.px(SEGMENT_PAD_DIP) * 2).collect()
    }

    /// A name on the left and its choices on the right: side by side when there are a few short
    /// ones, and otherwise a drop-down.
    pub fn combo(&mut self, label: &str, items: &[&str], selected: usize, id: i32) -> HWND {
        let widths = self.segments_width(items);
        let total: i32 = widths.iter().sum::<i32>() + self.px(6);
        let room = self.width * 60 / 100;
        if (2..=5).contains(&items.len()) && total <= room && !self.style.theme.high_contrast {
            return self.segmented(label, items, widths, total, selected, id);
        }
        self.drop_down(label, items, selected, id)
    }

    fn segmented(&mut self, label: &str, items: &[&str], widths: Vec<i32>, total: i32, selected: usize, id: i32) -> HWND {
        register_segmented();
        let row = self.px(32);
        self.row_label(label, self.width - total - self.px(12), row);
        let top = self.y;
        let rect = (self.x + self.width - total, self.y, total, row);
        let hwnd = self.create(SEGMENTED_CLASS, items.get(selected).copied().unwrap_or(""), WS_TABSTOP, rect, id, self.style.font);
        let control = Box::new(Segmented { items: items.iter().map(|item| item.to_string()).collect(), widths, selected: selected.min(items.len().saturating_sub(1)), hover: None, tracking: false, style: &*self.style as *const Style });
        unsafe {
            SetWindowLongPtrW(hwnd, GWLP_USERDATA, Box::into_raw(control) as isize);
            let _ = InvalidateRect(Some(hwnd), None, false);
        }
        self.y += row;
        self.advance(8);
        self.last_row = Some((self.last_label, hwnd, top, self.y));
        self.findable(label, hwnd, &items.join(" "));
        hwnd
    }

    /// A name on the left and a drop-down on the right.
    pub fn drop_down(&mut self, label: &str, items: &[&str], selected: usize, id: i32) -> HWND {
        let row = self.px(30);
        let top = self.y;
        let width = self.control_width(COMBO_DIP);
        self.row_label(label, self.width - width - self.px(12), row);
        let rect = (self.x + self.width - width, self.y, width, self.px(240));
        let style = WS_TABSTOP | WS_VSCROLL | WINDOW_STYLE((CBS_DROPDOWNLIST | CBS_OWNERDRAWFIXED | CBS_HASSTRINGS) as u32);
        let font = self.style.font;
        let hwnd = self.create(w!("COMBOBOX"), "", style, rect, id, font);
        // Filled before it's themed, and without redrawing after each item, which made a long
        // list cost milliseconds.
        unsafe {
            SendMessageW(hwnd, WM_SETREDRAW, Some(WPARAM(0)), None);
            SendMessageW(hwnd, CB_INITSTORAGE, Some(WPARAM(items.len())), Some(LPARAM((items.iter().map(|item| item.len() + 1).sum::<usize>() * 2) as isize)));
        }
        for item in items {
            let text = wide(item);
            unsafe {
                SendMessageW(hwnd, CB_ADDSTRING, None, Some(LPARAM(text.as_ptr() as isize)));
            }
        }
        unsafe {
            SendMessageW(hwnd, WM_SETREDRAW, Some(WPARAM(1)), None);
        }
        if self.style.is_dark() {
            unsafe {
                let _ = SetWindowTheme(hwnd, w!("DarkMode_CFD"), PCWSTR::null());
            }
        }
        unsafe {
            SendMessageW(hwnd, CB_SETCURSEL, Some(WPARAM(selected)), None);
            // The closed face, then the rows of the open list.
            SendMessageW(hwnd, CB_SETITEMHEIGHT, Some(WPARAM(usize::MAX)), Some(LPARAM(self.px(22) as isize)));
            SendMessageW(hwnd, CB_SETITEMHEIGHT, Some(WPARAM(0)), Some(LPARAM(self.px(28) as isize)));
            let surface = if self.open_card.is_some() { self.style.theme.card } else { self.style.theme.sidebar };
            let look = Box::new(ComboLook { theme: self.style.theme, surface, font: self.style.font, chevron: self.style.icon_small, dpi: self.style.dpi });
            let _ = SetWindowSubclass(hwnd, Some(combo_proc), COMBO_SUBCLASS, Box::into_raw(look) as usize);
            // The open list is a window of its own, with square corners unless it's told.
            let mut info = COMBOBOXINFO { cbSize: std::mem::size_of::<COMBOBOXINFO>() as u32, ..Default::default() };
            if GetComboBoxInfo(hwnd, &mut info).is_ok() {
                let corner = DWMWCP_ROUND;
                let _ = DwmSetWindowAttribute(info.hwndList, DWMWA_WINDOW_CORNER_PREFERENCE, &corner as *const _ as *const _, std::mem::size_of_val(&corner) as u32);
            }
        }
        self.y += row;
        self.advance(8);
        self.last_row = Some((self.last_label, hwnd, top, self.y));
        self.findable(label, hwnd, &items.join(" "));
        hwnd
    }

    fn button_width(&self, text: &str) -> i32 {
        self.px(8 * text.chars().count() as i32 + 28)
    }

    pub fn button_at(&mut self, text: &str, id: i32, x: i32, y: i32, width: i32, kind: ButtonKind) -> HWND {
        let default = kind == ButtonKind::Primary;
        let style = WS_TABSTOP | WINDOW_STYLE(if default { BS_DEFPUSHBUTTON } else { BS_PUSHBUTTON } as u32);
        let height = self.px(30);
        let font = self.style.font;
        let hwnd = self.create(w!("BUTTON"), text, style, (x, y, width, height), id, font);
        unsafe {
            SetWindowLongPtrW(hwnd, GWLP_USERDATA, kind as isize);
        }
        hwnd
    }

    /// Buttons side by side, from the left.
    pub fn buttons(&mut self, buttons: &[(&str, i32, ButtonKind)]) -> Vec<HWND> {
        let mut x = self.x;
        let y = self.y;
        let mut out = Vec::with_capacity(buttons.len());
        for (text, id, kind) in buttons {
            let width = self.button_width(text);
            // Only the window's main button may be the default one.
            let kind = if *kind == ButtonKind::Primary { ButtonKind::Normal } else { *kind };
            out.push(self.button_at(text, *id, x, y, width, kind));
            x += width + self.px(8);
        }
        self.y += self.px(30);
        self.advance(10);
        out
    }

    /// A list the parent draws. `LBS_HASSTRINGS` keeps the text readable by screen readers.
    pub fn owner_list(&mut self, rect: (i32, i32, i32, i32), extra_style: u32, id: i32) -> HWND {
        self.list_styled(rect, WS_VSCROLL.0 | LBS_OWNERDRAWFIXED as u32 | extra_style, id)
    }

    /// A list whose rows are as tall as each needs (`WM_MEASUREITEM` as each is added), with no
    /// scroll bar, where there's no room for one. It still scrolls with the keyboard, and its
    /// owner moves it for the wheel. A list only leaves its bar out if it's made without one.
    pub fn variable_list(&mut self, rect: (i32, i32, i32, i32), id: i32) -> HWND {
        self.list_styled(rect, LBS_OWNERDRAWVARIABLE as u32, id)
    }

    fn list_styled(&mut self, rect: (i32, i32, i32, i32), extra_style: u32, id: i32) -> HWND {
        let style = WS_TABSTOP | WINDOW_STYLE((LBS_NOTIFY | LBS_HASSTRINGS | LBS_NOINTEGRALHEIGHT) as u32 | extra_style);
        let font = self.style.font;
        let hwnd = self.create(w!("LISTBOX"), "", style, rect, id, font);
        if self.style.is_dark() {
            unsafe {
                let _ = SetWindowTheme(hwnd, w!("DarkMode_Explorer"), PCWSTR::null());
            }
        }
        unsafe {
            let _ = SetWindowSubclass(hwnd, Some(list_wheel_proc), LIST_WHEEL_SUBCLASS, 0);
        }
        hwnd
    }

    /// Moves past something placed by hand, so the card's edge keeps its usual padding.
    pub fn skip(&mut self, height: i32) {
        self.y += height;
        self.trailing = 0;
    }

    /// A card's title, after a small square in its colour with its symbol, like the page's own badge.
    pub fn card_title(&mut self, text: &str, glyph: char, color: crate::gfx::Rgb) -> HWND {
        let (size, height, gap) = (self.px(26), self.px(30), self.px(10));
        let font = self.style.bold;
        let style = WINDOW_STYLE(SS_NOPREFIX.0 | SS_CENTERIMAGE.0);
        let name = self.create(w!("STATIC"), text, style, (self.x + size + gap, self.y, self.width - size - gap, height), 0, font);
        let top = self.y + (height - size) / 2;
        self.style.badges.push(Badge { rect: RECT { left: self.x, top, right: self.x + size, bottom: top + size }, anchor: name, parent: self.parent, color, glyph });
        self.y += height;
        self.advance(6);
        self.findable(text, name, "");
        name
    }

    /// A card's title, with a small button at its right.
    pub fn card_header(&mut self, text: &str, button: &str, id: i32) -> HWND {
        let height = self.px(30);
        let width = self.button_width(button).max(self.px(68));
        let font = self.style.bold;
        let style = WINDOW_STYLE(SS_NOPREFIX.0 | SS_CENTERIMAGE.0);
        let name = self.create(w!("STATIC"), text, style, (self.x, self.y, self.width - width - self.px(12), height), 0, font);
        let hwnd = self.button_at(button, id, self.x + self.width - width, self.y, width, ButtonKind::Normal);
        self.y += height;
        self.advance(8);
        self.findable(text, name, button.trim_end_matches('\u{2026}'));
        hwnd
    }

    /// A text box with a button at its right, such as Browse.
    pub fn edit_with_button(&mut self, text: &str, id: i32, button: &str, button_id: i32) -> (HWND, HWND) {
        let width = self.px(96);
        let pad = self.px(8);
        let row = self.y;
        let edit = self.line_edit(text, self.x + pad, row + self.px(5), self.width - width - self.px(8) - pad * 2, id);
        let hwnd = self.button_at(button, button_id, self.x + self.width - width, row, width, ButtonKind::Normal);
        self.y = row + self.px(30);
        self.advance(8);
        (edit, hwnd)
    }

    /// Small buttons side by side, for dropping something into a text box.
    pub fn chips(&mut self, label: &str, chips: &[(&str, i32)]) {
        let height = self.px(28);
        let font = self.style.small;
        let style = WINDOW_STYLE(SS_NOPREFIX.0 | SS_CENTERIMAGE.0);
        let label_width = self.px(52);
        self.create(w!("STATIC"), label, style, (self.x, self.y, label_width, height), ID_NOTE, font);
        let mut x = self.x + label_width;
        for (text, id) in chips {
            let width = self.px(8 * text.chars().count() as i32 + 20);
            let style = WS_TABSTOP | WINDOW_STYLE(BS_PUSHBUTTON as u32);
            let hwnd = self.create(w!("BUTTON"), text, style, (x, self.y, width, height), *id, font);
            unsafe {
                SetWindowLongPtrW(hwnd, GWLP_USERDATA, ButtonKind::Normal as isize);
            }
            x += width + self.px(6);
        }
        self.y += height;
        self.advance(10);
    }

    pub fn take_controls(&mut self) -> Vec<HWND> {
        std::mem::take(&mut self.created)
    }
}


/// A few choices side by side, one of them chosen. It answers the same
/// messages as a drop-down list (`CB_GETCURSEL`, `CB_SETCURSEL`, `CB_GETDROPPEDSTATE`) and tells its
/// parent `CBN_SELCHANGE`, so a form can use either without knowing which it has.
struct Segmented {
    items: Vec<String>,
    /// Each choice's width, in pixels, left to right.
    widths: Vec<i32>,
    selected: usize,
    hover: Option<usize>,
    tracking: bool,
    /// The form's look. The form's `Style` outlives its controls.
    style: *const Style,
}

const SEGMENTED_CLASS: PCWSTR = w!("OrbitSegmented");
/// Room either side of a choice's name.
const SEGMENT_PAD_DIP: i32 = 14;

thread_local! {
    static SEGMENTED_READY: Cell<bool> = const { Cell::new(false) };
}

fn register_segmented() {
    if SEGMENTED_READY.with(|ready| ready.replace(true)) {
        return;
    }
    unsafe {
        let module = windows::Win32::System::LibraryLoader::GetModuleHandleW(None).unwrap_or_default();
        let class = WNDCLASSEXW {
            cbSize: std::mem::size_of::<WNDCLASSEXW>() as u32,
            style: CS_DBLCLKS,
            lpfnWndProc: Some(segmented_proc),
            hInstance: module.into(),
            hCursor: LoadCursorW(None, IDC_HAND).unwrap_or_default(),
            lpszClassName: SEGMENTED_CLASS,
            ..Default::default()
        };
        RegisterClassExW(&class);
    }
}

fn segmented_of(hwnd: HWND) -> Option<&'static mut Segmented> {
    unsafe { (GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *mut Segmented).as_mut() }
}

impl Segmented {
    fn at(&self, x: i32, inset: i32) -> Option<usize> {
        let mut left = inset;
        for (at, width) in self.widths.iter().enumerate() {
            if x >= left && x < left + width {
                return Some(at);
            }
            left += width;
        }
        None
    }
}

/// Chooses a segment and tells the parent, as a drop-down would.
fn choose_segment(hwnd: HWND, at: usize) {
    let Some(control) = segmented_of(hwnd) else { return };
    if at >= control.items.len() || at == control.selected {
        return;
    }
    control.selected = at;
    unsafe {
        let _ = InvalidateRect(Some(hwnd), None, false);
        let _ = SetWindowTextW(hwnd, PCWSTR(wide(&control.items[at]).as_ptr()));
        if let Ok(parent) = GetParent(hwnd) {
            let id = GetDlgCtrlID(hwnd) as usize & 0xFFFF;
            SendMessageW(parent, WM_COMMAND, Some(WPARAM(id | ((CBN_SELCHANGE as usize) << 16))), Some(LPARAM(hwnd.0 as isize)));
        }
    }
}

unsafe extern "system" fn segmented_proc(hwnd: HWND, message: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    unsafe {
        let Some(control) = segmented_of(hwnd) else {
            return DefWindowProcW(hwnd, message, wparam, lparam);
        };
        let style = &*control.style;
        let inset = style.px(3);
        match message {
            WM_ERASEBKGND => LRESULT(1),
            WM_PAINT => {
                let mut ps = PAINTSTRUCT::default();
                let dc = BeginPaint(hwnd, &mut ps);
                let mut client = RECT::default();
                let _ = GetClientRect(hwnd, &mut client);
                if let Some(mut buffer) = Backbuffer::new(client.right, client.bottom) {
                    let theme = &style.theme;
                    let (surface, _) = style.surface_of(hwnd);
                    let enabled = IsWindowEnabled(hwnd).as_bool();
                    let focused = GetFocus() == hwnd && style.focus_by_keyboard;
                    buffer.fill(surface);
                    let height = client.bottom;
                    let radius = (height / 2) as f32;
                    let track = if style.is_dark() { theme.field.mix(theme.sidebar, 0.35) } else { theme.keycap };
                    if focused {
                        let ring = 1.max(style.dpi as i32 / 96);
                        buffer.fill_round_rect(0, 0, client.right, height, radius, theme.text);
                        buffer.fill_round_rect(ring, ring, client.right - ring * 2, height - ring * 2, radius - ring as f32, track);
                    } else {
                        buffer.fill_round_rect(0, 0, client.right, height, radius, track);
                    }
                    let mut left = inset;
                    for (at, width) in control.widths.iter().enumerate() {
                        let chosen = at == control.selected;
                        let part = RECT { left, top: 0, right: left + width, bottom: height };
                        if chosen {
                            let lift = if style.is_dark() { theme.keycap_on_selection } else { theme.card };
                            let lift = if enabled { lift } else { lift.mix(track, 0.5) };
                            buffer.fill_round_rect(left, inset, *width, height - inset * 2, radius - inset as f32, lift);
                        }
                        let ink = if !enabled {
                            theme.tertiary
                        } else if chosen || control.hover == Some(at) {
                            theme.text
                        } else {
                            theme.secondary
                        };
                        buffer.text_centered(&control.items[at], if chosen { style.bold } else { style.font }, ink, part);
                        left += width;
                    }
                    buffer.copy_to_dc(dc);
                }
                let _ = EndPaint(hwnd, &ps);
                LRESULT(0)
            }
            CB_GETCURSEL => LRESULT(control.selected as isize),
            CB_GETCOUNT => LRESULT(control.items.len() as isize),
            CB_GETDROPPEDSTATE => LRESULT(0),
            CB_SETCURSEL => {
                if wparam.0 < control.items.len() {
                    control.selected = wparam.0;
                    let _ = SetWindowTextW(hwnd, PCWSTR(wide(&control.items[wparam.0]).as_ptr()));
                    let _ = InvalidateRect(Some(hwnd), None, false);
                }
                LRESULT(control.selected as isize)
            }
            WM_GETDLGCODE => LRESULT(DLGC_WANTARROWS as isize),
            WM_KEYDOWN => {
                let key = wparam.0 as u16;
                let last = control.items.len().saturating_sub(1);
                let next = match key {
                    0x25 | 0x26 => Some(control.selected.saturating_sub(1)),
                    0x27 | 0x28 => Some((control.selected + 1).min(last)),
                    0x24 => Some(0),
                    0x23 => Some(last),
                    _ => None,
                };
                if let Some(next) = next {
                    choose_segment(hwnd, next);
                    return LRESULT(0);
                }
                DefWindowProcW(hwnd, message, wparam, lparam)
            }
            WM_LBUTTONDOWN | WM_LBUTTONDBLCLK => {
                let _ = SetFocus(Some(hwnd));
                if let Some(at) = control.at((lparam.0 & 0xFFFF) as i16 as i32, inset) {
                    choose_segment(hwnd, at);
                }
                LRESULT(0)
            }
            WM_MOUSEMOVE => {
                let hover = control.at((lparam.0 & 0xFFFF) as i16 as i32, inset);
                if hover != control.hover {
                    control.hover = hover;
                    let _ = InvalidateRect(Some(hwnd), None, false);
                }
                if !control.tracking {
                    control.tracking = true;
                    let mut track = windows::Win32::UI::Input::KeyboardAndMouse::TRACKMOUSEEVENT {
                        cbSize: std::mem::size_of::<windows::Win32::UI::Input::KeyboardAndMouse::TRACKMOUSEEVENT>() as u32,
                        dwFlags: windows::Win32::UI::Input::KeyboardAndMouse::TME_LEAVE,
                        hwndTrack: hwnd,
                        dwHoverTime: 0,
                    };
                    let _ = windows::Win32::UI::Input::KeyboardAndMouse::TrackMouseEvent(&mut track);
                }
                LRESULT(0)
            }
            0x02A3 => {
                // WM_MOUSELEAVE
                control.tracking = false;
                control.hover = None;
                let _ = InvalidateRect(Some(hwnd), None, false);
                LRESULT(0)
            }
            WM_SETFOCUS | WM_KILLFOCUS | WM_ENABLE => {
                let _ = InvalidateRect(Some(hwnd), None, false);
                LRESULT(0)
            }
            WM_NCDESTROY => {
                SetWindowLongPtrW(hwnd, GWLP_USERDATA, 0);
                drop(Box::from_raw(control as *mut Segmented));
                DefWindowProcW(hwnd, message, wparam, lparam)
            }
            _ => DefWindowProcW(hwnd, message, wparam, lparam),
        }
    }
}

const COMBO_SUBCLASS: usize = 1;

/// What a drop-down's closed face needs. The fonts belong to the form's `Style`, which outlives its controls.
struct ComboLook {
    theme: Theme,
    /// What's behind the control, showing at its rounded corners.
    surface: crate::gfx::Rgb,
    font: HFONT,
    chevron: HFONT,
    dpi: u32,
}

fn combo_item_text(combo: HWND, index: usize) -> String {
    unsafe {
        let len = SendMessageW(combo, CB_GETLBTEXTLEN, Some(WPARAM(index)), None).0;
        if len <= 0 {
            return String::new();
        }
        let mut buffer = vec![0u16; len as usize + 1];
        let copied = SendMessageW(combo, CB_GETLBTEXT, Some(WPARAM(index)), Some(LPARAM(buffer.as_mut_ptr() as isize))).0;
        String::from_utf16_lossy(&buffer[..copied.max(0) as usize])
    }
}

/// Paints a closed drop-down like the text boxes around it. Everything else stays the control's own.
unsafe extern "system" fn combo_proc(hwnd: HWND, message: u32, wparam: WPARAM, lparam: LPARAM, _id: usize, data: usize) -> LRESULT {
    unsafe {
        let look = &*(data as *const ComboLook);
        match message {
            WM_ERASEBKGND => return LRESULT(1),
            WM_PAINT => {
                let mut ps = PAINTSTRUCT::default();
                let dc = BeginPaint(hwnd, &mut ps);
                let mut client = RECT::default();
                let _ = GetClientRect(hwnd, &mut client);
                if let Some(mut buffer) = Backbuffer::new(client.right, client.bottom) {
                    let theme = &look.theme;
                    let px = |value: i32| scale(value, look.dpi);
                    buffer.fill(look.surface);
                    let enabled = IsWindowEnabled(hwnd).as_bool();
                    let border = if GetFocus() == hwnd { theme.accent } else { theme.divider };
                    let inset = 1.max(look.dpi as i32 / 96);
                    buffer.fill_round_rect(0, 0, client.right, client.bottom, px(RADIUS_FIELD) as f32, border);
                    let fill = if enabled { theme.field } else { theme.field.mix(look.surface, 0.5) };
                    buffer.fill_round_rect(inset, inset, client.right - inset * 2, client.bottom - inset * 2, (px(RADIUS_FIELD) - inset) as f32, fill);
                    let selected = SendMessageW(hwnd, CB_GETCURSEL, None, None).0;
                    if selected >= 0 {
                        let text = combo_item_text(hwnd, selected as usize);
                        let ink = if enabled { theme.text } else { theme.tertiary };
                        buffer.text(&text, look.font, ink, RECT { left: px(10), top: 0, right: client.right - px(28), bottom: client.bottom }, false);
                    }
                    let glyph = "\u{E70D}";
                    let ink = if enabled { theme.secondary } else { theme.tertiary };
                    buffer.text_centered(glyph, look.chevron, ink, RECT { left: client.right - px(28), top: 0, right: client.right - px(4), bottom: client.bottom });
                    buffer.copy_to_dc(dc);
                }
                let _ = EndPaint(hwnd, &ps);
                return LRESULT(0);
            }
            WM_SETFOCUS | WM_KILLFOCUS => {
                let result = DefSubclassProc(hwnd, message, wparam, lparam);
                let _ = InvalidateRect(Some(hwnd), None, false);
                return result;
            }
            // A closed drop-down would change its value on a wheel turn. Scroll the page instead.
            WM_MOUSEWHEEL if SendMessageW(hwnd, CB_GETDROPPEDSTATE, None, None).0 == 0 => {
                if let Ok(parent) = GetParent(hwnd) {
                    return SendMessageW(parent, message, Some(wparam), Some(lparam));
                }
            }
            WM_NCDESTROY => {
                let _ = RemoveWindowSubclass(hwnd, Some(combo_proc), COMBO_SUBCLASS);
                drop(Box::from_raw(data as *mut ComboLook));
            }
            _ => {}
        }
        DefSubclassProc(hwnd, message, wparam, lparam)
    }
}

pub fn add_edit_keys(edit: HWND) {
    unsafe {
        let _ = SetWindowSubclass(edit, Some(edit_keys_proc), EDIT_KEYS_SUBCLASS, 0);
    }
}

/// A multi-line box shows its scroll bar only when its text doesn't fit. Showing or hiding it
/// wraps all the text again, so nothing happens unless it needs to change.
fn sync_scroll_bar(edit: HWND) {
    unsafe {
        let style = GetWindowLongPtrW(edit, GWL_STYLE) as u32;
        if style & ES_MULTILINE as u32 == 0 {
            return;
        }
        let wanted = edit_needs_bar(edit);
        if wanted != (style & WS_VSCROLL.0 != 0) {
            let _ = ShowScrollBar(edit, SB_VERT, wanted);
        }
    }
}

fn edit_needs_bar(edit: HWND) -> bool {
    unsafe {
        let lines = SendMessageW(edit, EM_GETLINECOUNT, None, None).0 as i32;
        let mut client = RECT::default();
        let _ = GetClientRect(edit, &mut client);
        let dc = GetDC(Some(edit));
        let font = HFONT(SendMessageW(edit, WM_GETFONT, None, None).0 as *mut _);
        let previous = SelectObject(dc, font.into());
        let mut metrics = TEXTMETRICW::default();
        let _ = GetTextMetricsW(dc, &mut metrics);
        SelectObject(dc, previous);
        ReleaseDC(Some(edit), dc);
        let fits = client.bottom / metrics.tmHeight.max(1);
        lines > fits
    }
}

pub fn get_text(hwnd: HWND) -> String {
    unsafe {
        let len = GetWindowTextLengthW(hwnd);
        if len <= 0 {
            return String::new();
        }
        let mut buffer = vec![0u16; len as usize + 1];
        let copied = GetWindowTextW(hwnd, &mut buffer);
        String::from_utf16_lossy(&buffer[..copied as usize])
    }
}

/// Text from a multi-line control, with Windows line ends turned back into `\n`.
pub fn get_lines(hwnd: HWND) -> String {
    get_text(hwnd).replace("\r\n", "\n")
}

pub fn set_text(hwnd: HWND, text: &str) {
    set_text_only(hwnd, text);
    // A multi-line box doesn't report a change made this way.
    sync_scroll_bar(hwnd);
}

/// For a control that manages its own scroll bar.
pub fn set_text_only(hwnd: HWND, text: &str) {
    let value = wide(&text.replace("\r\n", "\n").replace('\n', "\r\n"));
    unsafe {
        let _ = SetWindowTextW(hwnd, PCWSTR(value.as_ptr()));
    }
}

/// How tall this text is at this width, in the control's own font.
pub fn text_height_in(control: HWND, text: &str, width: i32) -> i32 {
    if text.is_empty() {
        return 0;
    }
    let mut units: Vec<u16> = text.encode_utf16().collect();
    let mut rect = RECT { left: 0, top: 0, right: width, bottom: 0 };
    unsafe {
        let dc = GetDC(Some(control));
        let font = HFONT(SendMessageW(control, WM_GETFONT, None, None).0 as *mut _);
        let previous = SelectObject(dc, font.into());
        DrawTextW(dc, &mut units, &mut rect, DT_CALCRECT | DT_WORDBREAK | DT_NOPREFIX);
        SelectObject(dc, previous);
        ReleaseDC(Some(control), dc);
    }
    rect.bottom
}

/// A message line changes its text without erasing the old one, so it needs asking to redraw.
pub fn set_status(hwnd: HWND, text: &str) {
    set_text(hwnd, text);
    unsafe {
        let _ = InvalidateRect(Some(hwnd), None, true);
    }
}

pub fn set_checked(hwnd: HWND, checked: bool) {
    let value = if checked { BST_CHECKED } else { BST_UNCHECKED };
    unsafe {
        SendMessageW(hwnd, BM_SETCHECK, Some(WPARAM(value.0 as usize)), None);
    }
}

pub fn is_checked(hwnd: HWND) -> bool {
    unsafe { SendMessageW(hwnd, BM_GETCHECK, None, None) }.0 == BST_CHECKED.0 as isize
}

pub fn selected(hwnd: HWND) -> usize {
    let value = unsafe { SendMessageW(hwnd, CB_GETCURSEL, None, None) }.0;
    value.max(0) as usize
}

/// Which part of a managed list's row a point is on.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ListPart {
    Body,
    Edit,
    Delete,
}

/// A row's picture: an icon-font glyph, or the first letter of its name.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Tile {
    Glyph(char),
    Letter(char),
}

/// The first letter of a name, as a tile shows it.
pub fn letter_tile(name: &str) -> Tile {
    let letter = name.trim().chars().find(|c| c.is_alphanumeric()).unwrap_or('?');
    Tile::Letter(letter.to_uppercase().next().unwrap_or(letter))
}

#[derive(Clone, Debug, PartialEq)]
pub struct ManagedRow {
    pub tile: Tile,
    pub title: String,
    /// An alias or keyword, shown as a pill. Empty shows none.
    pub pill: String,
    pub detail: String,
    /// A picture drawn in place of the tile, as `ICON_SIZE` square premultiplied BGRA.
    pub picture: Option<Vec<u8>>,
}

/// How wide a row's Delete or Remove is. It has to hold "Press again".
const ROW_ACTION_DIP: i32 = 78;

/// The left and right of the Edit and Delete text buttons in a row this wide.
pub fn managed_buttons(width: i32, dpi: u32) -> ((i32, i32), (i32, i32)) {
    let right = width - scale(8, dpi);
    let delete = (right - scale(ROW_ACTION_DIP, dpi), right);
    let edit = (delete.0 - scale(4, dpi) - scale(48, dpi), delete.0 - scale(4, dpi));
    (edit, delete)
}

/// How wide a row's Remove is.
pub fn row_action_width(dpi: u32) -> i32 {
    scale(ROW_ACTION_DIP, dpi)
}

pub fn managed_part_at(x: i32, width: i32, dpi: u32) -> ListPart {
    let (edit, delete) = managed_buttons(width, dpi);
    if x >= delete.0 && x < delete.1 {
        ListPart::Delete
    } else if x >= edit.0 && x < edit.1 {
        ListPart::Edit
    } else {
        ListPart::Body
    }
}

/// The first press on Delete arms that row. A press on the same row while armed means do it.
pub fn arm_or_confirm(armed: &mut Option<usize>, row: usize) -> bool {
    if *armed == Some(row) {
        *armed = None;
        true
    } else {
        *armed = Some(row);
        false
    }
}

impl Style {
    /// One row of a managed list. `hover` says which part the pointer is over, if it's on this row.
    pub fn draw_managed_row(&self, item: &DRAWITEMSTRUCT, row: &ManagedRow, armed: bool, hover: Option<ListPart>, selected: bool) {
        let (width, height) = (item.rcItem.right - item.rcItem.left, item.rcItem.bottom - item.rcItem.top);
        let Some(mut buffer) = Backbuffer::new(width, height) else { return };
        let theme = &self.theme;
        let px = |value: i32| self.px(value);
        buffer.fill(theme.card);
        if selected || hover.is_some() {
            let fill = if selected { theme.selection } else { theme.selection.mix(theme.card, 0.5) };
            buffer.fill_round_rect(0, px(2), width, height - px(4), px(RADIUS_SELECTION) as f32, fill);
        }
        let size = px(32);
        let (tile_x, tile_y) = (px(8), (height - size) / 2);
        let tile_fill = if selected { theme.keycap_on_selection } else { theme.keycap };
        let tile = RECT { left: tile_x, top: tile_y, right: tile_x + size, bottom: tile_y + size };
        match (&row.picture, row.tile) {
            (Some(picture), _) if picture.len() == crate::indexer::ICON_BYTES => {
                let pixels = crate::gfx::downscale_premultiplied(picture, crate::indexer::ICON_SIZE, size as usize);
                buffer.blit_premultiplied(tile_x, tile_y, size, &pixels);
            }
            (_, Tile::Glyph(glyph)) => {
                buffer.fill_round_rect(tile_x, tile_y, size, size, px(9) as f32, tile_fill);
                buffer.symbol_centered(&glyph.to_string(), self.icon, px(16), false, theme.secondary, tile);
            }
            (_, Tile::Letter(letter)) => {
                buffer.fill_round_rect(tile_x, tile_y, size, size, px(9) as f32, tile_fill);
                buffer.text_centered(&letter.to_string(), self.bold, theme.secondary, tile);
            }
        }
        let (edit, delete) = managed_buttons(width, self.dpi);
        let text_left = tile_x + size + px(12);
        let text_right = edit.0 - px(8);
        let line = height / 2;
        // The title takes what it needs, and the pill follows it if there's room.
        // A long keyword shouldn't cost a row the name that says what it is.
        let pill_width = if row.pill.is_empty() { 0 } else { (buffer.text_width(&row.pill, self.small) + px(14)).min(px(140)) };
        let pill_room = if pill_width > 0 { pill_width + px(8) } else { 0 };
        let title_right = (text_left + buffer.text_width(&row.title, self.font) + px(1)).min(text_right - pill_room).max(text_left + px(40));
        let top_line = RECT { left: text_left, top: line - px(20), right: title_right, bottom: line };
        buffer.text(&row.title, self.font, theme.text, top_line, false);
        if pill_width > 0 {
            let pill_x = title_right + px(8);
            let pill_height = px(18);
            let pill_y = top_line.top + (top_line.bottom - top_line.top - pill_height) / 2;
            let shown_width = pill_width.min(text_right - pill_x);
            if shown_width > px(16) {
                buffer.fill_round_rect(pill_x, pill_y, shown_width, pill_height, px(RADIUS_ROW) as f32, tile_fill);
                let pill_rect = RECT { left: pill_x + px(7), top: pill_y, right: pill_x + shown_width - px(4), bottom: pill_y + pill_height };
                buffer.text(&row.pill, self.small, theme.text, pill_rect, false);
            }
        }
        let bottom_line = RECT { left: text_left, top: line + px(1), right: text_right, bottom: line + px(19) };
        buffer.text(&row.detail, self.small, theme.secondary, bottom_line, false);
        let button = |buffer: &mut Backbuffer, span: (i32, i32), text: &str, ink: crate::gfx::Rgb, on: bool| {
            let (top, tall) = ((height - px(26)) / 2, px(26));
            if on {
                let fill = if selected { theme.keycap_on_selection } else { theme.keycap };
                buffer.fill_round_rect(span.0, top, span.1 - span.0, tall, px(RADIUS_ROW) as f32, fill);
            }
            buffer.text_centered(text, self.small, ink, RECT { left: span.0, top, right: span.1, bottom: top + tall });
        };
        // A row's actions show when it's being pointed at or is what the keyboard acts on.
        if armed || selected || hover.is_some() {
            button(&mut buffer, edit, "Edit", theme.text, hover == Some(ListPart::Edit));
            if armed {
                self.draw_armed_action(&mut buffer, delete, height);
            } else {
                button(&mut buffer, delete, "Delete", theme.secondary, hover == Some(ListPart::Delete));
            }
        }
        buffer.copy_to_dc_at(item.hDC, item.rcItem.left, item.rcItem.top);
    }

    /// A row's action that has been pressed once, and is waiting for a second.
    fn draw_armed_action(&self, buffer: &mut Backbuffer, span: (i32, i32), height: i32) {
        let (top, tall) = ((height - self.px(26)) / 2, self.px(26));
        buffer.fill_round_rect(span.0, top, span.1 - span.0, tall, self.px(RADIUS_ROW) as f32, self.theme.danger);
        buffer.text_centered("Press again", self.small, crate::ui::ink_on(self.theme.danger), RECT { left: span.0, top, right: span.1, bottom: top + tall });
    }

    /// The Remove button on a row drawn elsewhere, which says where it goes.
    pub fn draw_row_remove(&self, buffer: &mut Backbuffer, delete: (i32, i32), height: i32, armed: bool, hover: Option<ListPart>, selected: bool) {
        if armed {
            self.draw_armed_action(buffer, delete, height);
            return;
        }
        if !selected && hover.is_none() {
            return;
        }
        let theme = &self.theme;
        let (top, tall) = ((height - self.px(26)) / 2, self.px(26));
        if hover == Some(ListPart::Delete) {
            let fill = if selected { theme.keycap_on_selection } else { theme.keycap };
            buffer.fill_round_rect(delete.0, top, delete.1 - delete.0, tall, self.px(RADIUS_ROW) as f32, fill);
        }
        buffer.text_centered("Remove", self.small, theme.secondary, RECT { left: delete.0, top, right: delete.1, bottom: top + tall });
    }

    /// What a managed list shows with no rows.
    pub fn paint_empty_list(&self, hwnd: HWND, text: &str) {
        unsafe {
            let mut ps = PAINTSTRUCT::default();
            let dc = BeginPaint(hwnd, &mut ps);
            let mut client = RECT::default();
            let _ = GetClientRect(hwnd, &mut client);
            if let Some(mut buffer) = Backbuffer::new(client.right, client.bottom) {
                buffer.fill(self.theme.card);
                buffer.text_centered(text, self.font, self.theme.secondary, client);
                buffer.copy_to_dc(dc);
            }
            let _ = EndPaint(hwnd, &ps);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn units(text: &str) -> Vec<u16> {
        text.encode_utf16().collect()
    }

    fn deleted_from(text: &str, caret: usize) -> usize {
        word_start(&units(text), caret)
    }

    #[test]
    fn word_start_takes_one_word_and_the_spaces_after_it() {
        assert_eq!(deleted_from("hello world", 11), 6);
        assert_eq!(deleted_from("hello world   ", 14), 6);
        assert_eq!(deleted_from("hello world", 5), 0);
        assert_eq!(deleted_from("snake_case2", 11), 0);
        assert_eq!(deleted_from("a\tb", 3), 2);
        assert_eq!(deleted_from("a\t", 2), 0);
    }

    #[test]
    fn word_start_stops_where_punctuation_starts_or_ends() {
        assert_eq!(deleted_from("https://github.com", 18), 15);
        assert_eq!(deleted_from("https://github.", 15), 14);
        assert_eq!(deleted_from("https://", 8), 5);
        assert_eq!(deleted_from("wait...", 7), 4);
        assert_eq!(deleted_from("mix !? ", 7), 4);
    }

    #[test]
    fn word_start_treats_a_line_break_as_its_own_step() {
        assert_eq!(deleted_from("one\r\ntwo", 8), 5);
        assert_eq!(deleted_from("one\r\ntwo", 5), 3);
        assert_eq!(deleted_from("one\ntwo", 4), 3);
        assert_eq!(deleted_from("one\r\n   ", 8), 5, "only the spaces, then the break is next");
        assert_eq!(deleted_from("one\r", 4), 3);
    }

    #[test]
    fn word_start_is_safe_at_the_ends_and_keeps_pairs_whole() {
        assert_eq!(deleted_from("", 0), 0);
        assert_eq!(deleted_from("abc", 0), 0);
        assert_eq!(deleted_from("abc", 99), 0);
        assert_eq!(deleted_from("   ", 3), 0);
        let pair = "a \u{1F600}";
        assert_eq!(deleted_from(pair, units(pair).len()), 2, "both halves of the emoji go together");
        assert_eq!(deleted_from("caf\u{E9} x", 6), 5);
        assert_eq!(deleted_from("caf\u{E9}", 4), 0, "accented letters are letters");
    }

    #[test]
    fn scroll_stays_within_the_page() {
        assert_eq!(max_scroll(100, 400), 0);
        assert_eq!(max_scroll(1000, 400), 600);
        assert_eq!(clamp_scroll(-5, 1000, 400), 0);
        assert_eq!(clamp_scroll(700, 1000, 400), 600);
        assert_eq!(clamp_scroll(50, 300, 400), 0);
    }

    #[test]
    fn revealing_moves_only_as_far_as_it_has_to() {
        assert_eq!(reveal_scroll(100, 200, 230, 1000, 400, 10), 100, "already showing");
        assert_eq!(reveal_scroll(100, 90, 120, 1000, 400, 10), 80, "above");
        assert_eq!(reveal_scroll(100, 480, 510, 1000, 400, 10), 120, "below");
        assert_eq!(reveal_scroll(0, 500, 1000, 1000, 400, 10), 490, "taller than the view shows its top");
        assert_eq!(reveal_scroll(600, 5, 20, 1000, 400, 10), 0, "never past the start");
        assert_eq!(reveal_scroll(0, 980, 995, 1000, 400, 10), 600, "never past the end");
    }

    #[test]
    fn the_thumb_shows_only_when_needed_and_tracks_the_page() {
        assert_eq!(thumb_geometry(0, 400, 400, 4, 24), None);
        assert_eq!(thumb_geometry(0, 300, 400, 4, 24), None);
        let (top, height) = thumb_geometry(0, 800, 400, 4, 24).unwrap();
        assert_eq!((top, height), (4, 196));
        let (top, height) = thumb_geometry(400, 800, 400, 4, 24).unwrap();
        assert_eq!(top + height, 396, "reaches the end of its track");
        let (_, short) = thumb_geometry(0, 100_000, 400, 4, 24).unwrap();
        assert_eq!(short, 24, "never too small to grab");
    }

    #[test]
    fn dragging_the_thumb_gives_back_the_scroll_position() {
        for scroll in [0, 1, 100, 250, 399, 400] {
            let (top, _) = thumb_geometry(scroll, 800, 400, 4, 24).unwrap();
            let back = scroll_for_thumb(top, 800, 400, 4, 24);
            assert!((back - scroll).abs() <= 2, "{scroll} came back as {back}");
        }
        assert_eq!(scroll_for_thumb(-50, 800, 400, 4, 24), 0);
        assert_eq!(scroll_for_thumb(5000, 800, 400, 4, 24), 400);
        assert_eq!(scroll_for_thumb(10, 400, 400, 4, 24), 0, "nothing to scroll");
    }

    #[test]
    fn a_wheel_notch_follows_the_windows_setting() {
        assert_eq!(wheel_pixels(120, 3, 20), 60);
        assert_eq!(wheel_pixels(-120, 3, 20), -60);
        assert_eq!(wheel_pixels(30, 3, 20), 15, "fine-grained wheels move a little");
    }

    #[test]
    fn a_row_knows_its_edit_and_delete_buttons() {
        let width = 600;
        let (edit, delete) = managed_buttons(width, 96);
        assert_eq!(delete.1, 592);
        assert!(edit.1 <= delete.0);
        assert_eq!(managed_part_at(delete.0, width, 96), ListPart::Delete);
        assert_eq!(managed_part_at(delete.1 - 1, width, 96), ListPart::Delete);
        assert_eq!(managed_part_at(edit.0, width, 96), ListPart::Edit);
        assert_eq!(managed_part_at(edit.0 - 1, width, 96), ListPart::Body);
        assert_eq!(managed_part_at(delete.1, width, 96), ListPart::Body);
        assert_eq!(managed_part_at(0, width, 96), ListPart::Body);
        // Twice the pixels at twice the DPI.
        let (_, wide_delete) = managed_buttons(width * 2, 192);
        assert_eq!(wide_delete.1 - wide_delete.0, (delete.1 - delete.0) * 2);
    }

    #[test]
    fn delete_needs_two_presses_on_the_same_row() {
        let mut armed = None;
        assert!(!arm_or_confirm(&mut armed, 2));
        assert_eq!(armed, Some(2));
        assert!(!arm_or_confirm(&mut armed, 3), "another row only moves the arming");
        assert_eq!(armed, Some(3));
        assert!(arm_or_confirm(&mut armed, 3));
        assert_eq!(armed, None);
    }

    #[test]
    fn a_tile_shows_the_first_letter_it_can() {
        assert_eq!(letter_tile("github"), Tile::Letter('G'));
        assert_eq!(letter_tile("  (docs)"), Tile::Letter('D'));
        assert_eq!(letter_tile("..."), Tile::Letter('?'));
    }
}
