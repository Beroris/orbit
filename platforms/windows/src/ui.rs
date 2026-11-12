//! Launcher appearance: theme, fonts, layout and the single render function. Everything the
//! window shows is drawn into one backbuffer from `App` state, so a frame is cheap and there is
//! no retained widget tree to update.

use crate::app::App;
use crate::gfx::{Backbuffer, Rgb, Shadow, compose_panel, create_font, delete_font, paint_shadow};
use crate::grid::{COLUMNS, Grid, Line};
use crate::inline_form::InlineForm;
use crate::motion::{self, Crossfade};
use crate::rows::*;
use crate::text::{self, Align, Face};
use crate::view::{MenuKind, MenuSide, Scope};
use crate::worker::{ClipPreview, PreviewBody};
use std::collections::HashMap;
use std::time::Instant;
use std::sync::atomic::{AtomicBool, AtomicIsize, AtomicU32, Ordering};
use windows::Win32::Foundation::{HWND, RECT};
use windows::Win32::Graphics::Dwm::{DWMSBT_NONE, DWMSBT_TRANSIENTWINDOW, DWMWA_SYSTEMBACKDROP_TYPE, DwmSetWindowAttribute};
use windows::Win32::Graphics::Gdi::{CreateSolidBrush, DeleteObject, FW_NORMAL, GetSysColor, HFONT, HGDIOBJ, SYS_COLOR_INDEX};
use windows::Win32::System::LibraryLoader::{GetModuleHandleW, GetProcAddress};
use windows::Win32::System::Registry::{HKEY_CURRENT_USER, RRF_RT_REG_BINARY, RRF_RT_REG_DWORD, RegGetValueW};
use windows::Win32::UI::Accessibility::{HCF_HIGHCONTRASTON, HIGHCONTRASTW};
use windows::Win32::UI::WindowsAndMessaging::{GetSystemMetrics, SM_REMOTESESSION, SPI_GETHIGHCONTRAST, SYSTEM_PARAMETERS_INFO_UPDATE_FLAGS, SystemParametersInfoW};
use windows::core::{s, w};

use orbit_core::vim::VimMode;

pub const BASE_WIDTH: i32 = 720;
pub const INPUT_HEIGHT: i32 = 64;
pub const ROW_HEIGHT: i32 = 44;
pub const HEADER_HEIGHT: i32 = 30;
/// A section title at the very top has no section above it to stand apart from. Its words go up
/// into the empty room under the search box, and it takes only this much of the list, so the
/// first row starts about where it does once something's typed.
pub const FIRST_HEADER_HEIGHT: i32 = 8;
/// How far the first title's words reach up past the top of the list.
const FIRST_HEADER_LIFT: i32 = 14;
/// The room over a section title's words, between one section and the next.
const HEADER_SPACE: i32 = 8;
pub const FOOTER_HEIGHT: i32 = 56;
/// An answer that's the only result is drawn as a card this tall.
pub const ANSWER_CARD_HEIGHT: i32 = 150;
pub const PADDING: i32 = 6;
/// How far rows and pills sit in from the window's edge.
const ROW_INSET: i32 = 10;
pub const ICON_DIP: i32 = 26;
const LOGO_DIP: i32 = 16;
/// A clipboard image gets more room than an icon, so you can tell what it is.
pub const THUMB_DIP: i32 = 36;
const QUERY_DIP: i32 = 20;
const WINDOW_RADIUS: i32 = 22;
const ROW_RADIUS: i32 = 12;
const TILE_RADIUS: i32 = 8;
/// A tile in the emoji grid.
const CELL_RADIUS: i32 = 13;
const CAP_RADIUS: i32 = 6;
const PILL_HEIGHT: i32 = 36;
/// How far down the light from above reaches.
const SHEEN_DIP: i32 = 110;

// The soft shadow, which the window has to be bigger than the launcher to hold.
const SHADOW_MARGIN: i32 = 36;
const SHADOW_DROP: i32 = 8;
const SHADOW_BLUR: i32 = 28;
const SHADOW_DARK: f32 = 0.42;
const SHADOW_LIGHT: f32 = 0.20;

// The small menu.
const POPOVER_WIDTH: i32 = 330;
const POPOVER_RADIUS: i32 = 16;
const POPOVER_ROW: i32 = 36;
const POPOVER_ROW_RADIUS: i32 = 10;
const POPOVER_PAD: i32 = 6;
/// How far a menu's rows sit inside its sides.
const POPOVER_ROW_INSET: i32 = 6;
/// How far a menu's shadow reaches.
const POPOVER_SHADOW_DIP: i32 = 22;
/// How far a menu travels as it comes in.
const MENU_RISE_DIP: i32 = 10;
const POPOVER_TITLE: i32 = 26;
const POPOVER_SEARCH: i32 = 42;
const POPOVER_GAP: i32 = 8;
const POPOVER_DARK: Rgb = Rgb(44, 44, 49);
const POPOVER_LIGHT: Rgb = Rgb(253, 253, 254);
const ALPHA_POPOVER_RIM: u8 = 30;
/// The longest a menu gets before it scrolls.
pub const POPOVER_MAX_ROWS: usize = 8;

/// A quicklink's boxes in the search bar.
const ARG_BOX_DIP: i32 = 34;
const ARG_BOX_MAX_DIP: i32 = 240;
/// A form's labels end here, and its single-line boxes are this tall.
const FORM_LABEL_DIP: i32 = 112;
const FORM_BOX_DIP: i32 = 38;

/// Of the room between the search box and the footer, how much a menu may take, in tenths.
const POPOVER_SHARE: i32 = 7;

// The clipboard's list, with what's selected shown beside it.
const SPLIT_LIST_DIP: i32 = 300;
const PANEL_PAD: i32 = 18;
const DETAIL_TITLE: i32 = 30;
const DETAIL_ROW: i32 = 30;
const FILE_ROW: i32 = 44;
const PREVIEW_RADIUS: i32 = 8;
/// Rows of details the picture leaves room for when its size is asked for. Pictures have at most
/// this many, so one is never scaled for more room than it gets.
const DETAIL_ROWS_FOR_IMAGES: usize = 4;

// The emoji grid.
const EMOJI_DIP: i32 = 44;
const EMOJI_CACHE_MAX: usize = 240;

/// A menu is open, so clicks in the search band are for it and not the search box. Read while `App`
/// may be borrowed.
pub static POPOVER_OPEN: AtomicBool = AtomicBool::new(false);

/// The launcher's sheet. Lighter than the page behind it, so it stands off even a black window.
const SURFACE_DARK: Rgb = Rgb(32, 32, 36);
const SURFACE_LIGHT: Rgb = Rgb(246, 246, 248);
/// How much of that sheet covers the blurred desktop behind it. Enough to keep text readable over
/// anything, and little enough that the desktop's colours still come through.
const GLASS_ALPHA_DARK: u8 = 214;
const GLASS_ALPHA_LIGHT: u8 = 224;
/// The colour Windows mixes into its blur, as 0xAABBGGRR.
const BLUR_TINT_DARK: u32 = 0x6030_2C2C;
const BLUR_TINT_LIGHT: u32 = 0x60F8_F6F6;
// Layers over the glass, as how much of the overlay colour shows through.
const ALPHA_DIVIDER: u8 = 20;
const ALPHA_SELECTION: u8 = 22;
const ALPHA_PRESSED: u8 = 40;
const ALPHA_KEYCAP: u8 = 8;
const ALPHA_KEYCAP_ON_SELECTION: u8 = 16;
const ALPHA_CAP_RIM: u8 = 40;
const ALPHA_TILE: u8 = 14;
/// Around the selected emoji: enough to find it, without a hard box round it.
const ALPHA_CELL_RIM: u8 = 60;
const ALPHA_PILL: u8 = 12;
const ALPHA_PILL_RIM: u8 = 16;
const ALPHA_SHEEN: u8 = 8;
const ALPHA_EDGE: u8 = 26;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Theme {
    pub background: Rgb,
    pub text: Rgb,
    pub secondary: Rgb,
    /// Quieter than `secondary`: labels that repeat down the list, and hints in an empty box.
    pub tertiary: Rgb,
    pub selection: Rgb,
    pub selection_text: Rgb,
    pub divider: Rgb,
    /// The line Windows draws around the launcher.
    pub edge: Rgb,
    pub accent: Rgb,
    pub footer: Rgb,
    /// Text on an accent fill.
    pub on_accent: Rgb,
    /// Behind a key name, and behind an icon drawn from the icon font.
    pub keycap: Rgb,
    pub keycap_on_selection: Rgb,
    /// Text boxes in forms.
    pub field: Rgb,
    /// Behind a form's whole window, including its page list.
    pub sidebar: Rgb,
    /// A group of settings, a step up from the window behind it.
    pub card: Rgb,
    pub danger: Rgb,
    /// White on dark, black on light: what the launcher's layers are made of.
    pub overlay: Rgb,
    /// Colours must stay exactly as Windows gives them, so nothing is layered or see-through.
    pub high_contrast: bool,
}

/// Windows' default blue, for when the accent can't be read.
const FALLBACK_ACCENT_DARK: Rgb = Rgb(96, 205, 255);
const FALLBACK_ACCENT_LIGHT: Rgb = Rgb(0, 95, 184);

const DARK: Theme = Theme {
    background: Rgb(19, 19, 21),
    text: Rgb(240, 240, 242),
    secondary: Rgb(160, 160, 168),
    tertiary: Rgb(122, 122, 130),
    selection: Rgb(44, 44, 49),
    selection_text: Rgb(255, 255, 255),
    divider: Rgb(44, 44, 49),
    edge: Rgb(74, 74, 80),
    accent: FALLBACK_ACCENT_DARK,
    footer: Rgb(15, 15, 17),
    on_accent: Rgb(0, 0, 0),
    keycap: Rgb(46, 46, 51),
    keycap_on_selection: Rgb(64, 64, 70),
    field: Rgb(35, 35, 39),
    sidebar: Rgb(14, 14, 16),
    card: Rgb(25, 25, 28),
    danger: Rgb(248, 113, 113),
    overlay: Rgb(255, 255, 255),
    high_contrast: false,
};

const LIGHT: Theme = Theme {
    background: Rgb(250, 250, 251),
    text: Rgb(26, 26, 29),
    secondary: Rgb(96, 96, 105),
    tertiary: Rgb(128, 128, 136),
    selection: Rgb(228, 228, 233),
    selection_text: Rgb(16, 16, 18),
    divider: Rgb(226, 226, 231),
    edge: Rgb(196, 196, 202),
    accent: FALLBACK_ACCENT_LIGHT,
    footer: Rgb(243, 243, 245),
    on_accent: Rgb(255, 255, 255),
    keycap: Rgb(233, 233, 238),
    keycap_on_selection: Rgb(214, 214, 221),
    field: Rgb(249, 249, 251),
    sidebar: Rgb(241, 241, 244),
    card: Rgb(255, 255, 255),
    danger: Rgb(185, 28, 28),
    overlay: Rgb(0, 0, 0),
    high_contrast: false,
};

pub fn is_high_contrast() -> bool {
    let mut info = HIGHCONTRASTW {
        cbSize: std::mem::size_of::<HIGHCONTRASTW>() as u32,
        ..Default::default()
    };
    unsafe {
        SystemParametersInfoW(
            SPI_GETHIGHCONTRAST,
            info.cbSize,
            Some(&mut info as *mut _ as *mut _),
            SYSTEM_PARAMETERS_INFO_UPDATE_FLAGS(0),
        )
        .is_ok()
            && info.dwFlags.contains(HCF_HIGHCONTRASTON)
    }
}

fn sys_color(index: i32) -> Rgb {
    let value = unsafe { GetSysColor(SYS_COLOR_INDEX(index)) };
    Rgb(value as u8, (value >> 8) as u8, (value >> 16) as u8)
}

pub fn system_uses_dark_apps() -> bool {
    let mut value = 1u32;
    let mut size = 4u32;
    let status = unsafe {
        RegGetValueW(
            HKEY_CURRENT_USER,
            w!("Software\\Microsoft\\Windows\\CurrentVersion\\Themes\\Personalize"),
            w!("AppsUseLightTheme"),
            RRF_RT_REG_DWORD,
            None,
            Some(&mut value as *mut _ as *mut _),
            Some(&mut size),
        )
    };
    status.is_ok() && value == 0
}

/// Whether the desktop may show through. Windows' own switch for transparency effects decides,
/// and blur isn't available over Remote Desktop.
pub fn glass_is_allowed() -> bool {
    let mut value = 1u32;
    let mut size = 4u32;
    let status = unsafe {
        RegGetValueW(
            HKEY_CURRENT_USER,
            w!("Software\\Microsoft\\Windows\\CurrentVersion\\Themes\\Personalize"),
            w!("EnableTransparency"),
            RRF_RT_REG_DWORD,
            None,
            Some(&mut value as *mut _ as *mut _),
            Some(&mut size),
        )
    };
    let transparency_on = status.is_err() || value != 0;
    transparency_on && !is_high_contrast() && unsafe { GetSystemMetrics(SM_REMOTESESSION) } == 0
}

/// Two ways to ask Windows for a blurred backdrop.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Backdrop {
    /// `DWMWA_SYSTEMBACKDROP_TYPE`, the documented way, from Windows 11 22H2.
    System,
    /// `SetWindowCompositionAttribute`, undocumented but available since Windows 10 1803.
    Accent,
}

impl Backdrop {
    pub fn parse(text: &str) -> Option<Backdrop> {
        match text {
            "system" => Some(Backdrop::System),
            "accent" => Some(Backdrop::Accent),
            _ => None,
        }
    }
}

#[repr(C)]
struct AccentPolicy {
    state: u32,
    flags: u32,
    gradient: u32,
    animation: u32,
}

#[repr(C)]
struct CompositionData {
    attribute: u32,
    data: *mut core::ffi::c_void,
    size: usize,
}

/// Turns the blur behind a window on or off. The window paints its own tint over it.
pub fn set_backdrop(window: HWND, backdrop: Option<Backdrop>, dark: bool) {
    const ACCENT_DISABLED: u32 = 0;
    const ACCENT_ACRYLIC: u32 = 4;
    const WCA_ACCENT_POLICY: u32 = 19;
    type SetAttribute = unsafe extern "system" fn(HWND, *mut CompositionData) -> i32;
    unsafe {
        let system = if backdrop == Some(Backdrop::System) { DWMSBT_TRANSIENTWINDOW } else { DWMSBT_NONE };
        let _ = DwmSetWindowAttribute(window, DWMWA_SYSTEMBACKDROP_TYPE, &system as *const _ as *const _, std::mem::size_of_val(&system) as u32);
        let Ok(module) = GetModuleHandleW(w!("user32.dll")) else { return };
        let Some(address) = GetProcAddress(module, s!("SetWindowCompositionAttribute")) else { return };
        let set_attribute: SetAttribute = std::mem::transmute(address);
        let mut policy = AccentPolicy {
            state: if backdrop == Some(Backdrop::Accent) { ACCENT_ACRYLIC } else { ACCENT_DISABLED },
            flags: 2,
            // Windows draws a nearly clear tint here as solid black, hiding the desktop. Measured
            // on screen on 2026-11-06: 0x01000000 showed nothing, and 0x20 and up show it properly.
            gradient: if dark { BLUR_TINT_DARK } else { BLUR_TINT_LIGHT },
            animation: 0,
        };
        let mut data = CompositionData {
            attribute: WCA_ACCENT_POLICY,
            data: &mut policy as *mut _ as *mut _,
            size: std::mem::size_of::<AccentPolicy>(),
        };
        set_attribute(window, &mut data);
    }
}

pub fn resolve_theme(appearance: orbit_core::settings::Appearance) -> Theme {
    use orbit_core::settings::Appearance;
    if is_high_contrast() {
        const COLOR_WINDOW: i32 = 5;
        const COLOR_WINDOWTEXT: i32 = 8;
        const COLOR_HIGHLIGHT: i32 = 13;
        const COLOR_HIGHLIGHTTEXT: i32 = 14;
        const COLOR_GRAYTEXT: i32 = 17;
        return Theme {
            background: sys_color(COLOR_WINDOW),
            text: sys_color(COLOR_WINDOWTEXT),
            secondary: sys_color(COLOR_GRAYTEXT),
            tertiary: sys_color(COLOR_GRAYTEXT),
            selection: sys_color(COLOR_HIGHLIGHT),
            selection_text: sys_color(COLOR_HIGHLIGHTTEXT),
            divider: sys_color(COLOR_WINDOWTEXT),
            edge: sys_color(COLOR_WINDOWTEXT),
            accent: sys_color(COLOR_HIGHLIGHT),
            footer: sys_color(COLOR_WINDOW),
            on_accent: sys_color(COLOR_HIGHLIGHTTEXT),
            keycap: sys_color(COLOR_WINDOW),
            keycap_on_selection: sys_color(COLOR_HIGHLIGHT),
            field: sys_color(COLOR_WINDOW),
            sidebar: sys_color(COLOR_WINDOW),
            card: sys_color(COLOR_WINDOW),
            danger: sys_color(COLOR_WINDOWTEXT),
            overlay: sys_color(COLOR_WINDOWTEXT),
            high_contrast: true,
        };
    }
    let dark = match appearance {
        Appearance::Dark => true,
        Appearance::Light => false,
        Appearance::System => system_uses_dark_apps(),
    };
    with_accent(if dark { DARK } else { LIGHT }, dark, system_accent_palette().as_ref())
}

/// Windows' accent colour in its seven shades, lightest first, with the accent itself fourth.
fn system_accent_palette() -> Option<[Rgb; 7]> {
    let mut bytes = [0u8; 32];
    let mut size = bytes.len() as u32;
    let status = unsafe {
        RegGetValueW(
            HKEY_CURRENT_USER,
            w!("Software\\Microsoft\\Windows\\CurrentVersion\\Explorer\\Accent"),
            w!("AccentPalette"),
            RRF_RT_REG_BINARY,
            None,
            Some(bytes.as_mut_ptr() as *mut _),
            Some(&mut size),
        )
    };
    if status.is_err() || size != 32 {
        return None;
    }
    let shade = |index: usize| Rgb(bytes[index * 4], bytes[index * 4 + 1], bytes[index * 4 + 2]);
    Some([shade(0), shade(1), shade(2), shade(3), shade(4), shade(5), shade(6)])
}

/// How far apart two colours read, from 1 to 21, as WCAG defines it.
/// Black or white, whichever reads better on this colour.
pub fn ink_on(fill: Rgb) -> Rgb {
    let (black, white) = (Rgb(0, 0, 0), Rgb(255, 255, 255));
    if contrast(fill, black) >= contrast(fill, white) { black } else { white }
}

fn contrast(a: Rgb, b: Rgb) -> f32 {
    fn luminance(color: Rgb) -> f32 {
        let channel = |value: u8| {
            let value = value as f32 / 255.0;
            if value <= 0.03928 { value / 12.92 } else { ((value + 0.055) / 1.055).powf(2.4) }
        };
        0.2126 * channel(color.0) + 0.7152 * channel(color.1) + 0.0722 * channel(color.2)
    }
    let (a, b) = (luminance(a), luminance(b));
    (a.max(b) + 0.05) / (a.min(b) + 0.05)
}

/// The shades Windows itself uses on each background. An accent that would still be hard to see,
/// such as a pale grey on a light theme, moves towards the text colour until it isn't.
fn with_accent(mut theme: Theme, dark: bool, palette: Option<&[Rgb; 7]>) -> Theme {
    const MINIMUM_CONTRAST: f32 = 3.0;
    if let Some(palette) = palette {
        theme.accent = if dark { palette[1] } else { palette[4] };
    }
    let mut amount = 0.0;
    let chosen = theme.accent;
    while contrast(theme.accent, theme.background) < MINIMUM_CONTRAST && amount < 1.0 {
        amount += 0.1;
        theme.accent = chosen.mix(theme.text, amount);
    }
    let (black, white) = (Rgb(0, 0, 0), Rgb(255, 255, 255));
    theme.on_accent = if contrast(theme.accent, black) >= contrast(theme.accent, white) { black } else { white };
    theme
}

// The input's colours, readable while `App` is borrowed: Windows asks for them re-entrantly.
pub static EDIT_BRUSH: AtomicIsize = AtomicIsize::new(0);
pub static EDIT_TEXT: AtomicU32 = AtomicU32::new(0);
pub static EDIT_BACK: AtomicU32 = AtomicU32::new(0);

pub fn publish_edit_colors(theme: &Theme) {
    unsafe {
        let brush = CreateSolidBrush(theme.background.colorref());
        let old = EDIT_BRUSH.swap(brush.0 as isize, Ordering::Relaxed);
        if old != 0 {
            let _ = DeleteObject(HGDIOBJ(old as *mut _));
        }
    }
    EDIT_TEXT.store(theme.text.colorref().0, Ordering::Relaxed);
    EDIT_BACK.store(theme.background.colorref().0, Ordering::Relaxed);
}

pub struct Fonts {
    /// For the search box itself, which takes clicks and typing but is drawn over.
    pub edit: HFONT,
    pub query: Face,
    pub section: Face,
    pub title: Face,
    pub small: Face,
    /// What's typed in a form's boxes.
    pub body: Face,
    pub hint: Face,
    pub hint_strong: Face,
    pub keycap: Face,
    pub icon_large: Face,
    pub icon_row: Face,
    pub icon_tile: Face,
    /// A command's symbol on its coloured tile.
    pub icon_on_tile: Face,
    pub icon_keycap: Face,
    /// The weather's temperature, large and light.
    pub display: Face,
    pub heading: Face,
    /// The two sides of an answer card.
    pub answer: Face,
    /// The timer's and stopwatch's time.
    pub clock: Face,
}

impl Fonts {
    pub fn new(dpi: u32) -> Option<Fonts> {
        let px = |dip: f32| dip * dpi as f32 / 96.0;
        let ui = |dip: f32, weight: u32| Face::ui(px(dip), dip, weight);
        Some(Fonts {
            edit: create_font(crate::gfx::ui_face(), scale(QUERY_DIP, dpi), FW_NORMAL.0 as i32),
            query: ui(QUERY_DIP as f32, 400)?,
            section: ui(12.0, 560)?,
            title: ui(14.0, 500)?,
            small: ui(13.0, 400)?,
            body: ui(14.0, 400)?,
            hint: ui(12.0, 400)?,
            hint_strong: ui(12.0, 560)?,
            keycap: ui(11.0, 500)?,
            icon_large: Face::icons(px(18.0))?,
            icon_row: Face::icons(px(16.0))?,
            icon_tile: Face::icons(px(15.0))?,
            icon_on_tile: Face::icons_bold(px(15.0))?,
            icon_keycap: Face::icons(px(12.5))?,
            display: ui(46.0, 300)?,
            heading: ui(16.0, 560)?,
            answer: ui(30.0, 560)?,
            clock: ui(56.0, 300)?,
        })
    }
}

impl Drop for Fonts {
    fn drop(&mut self) {
        delete_font(self.edit);
    }
}

pub fn scale(dip: i32, dpi: u32) -> i32 {
    dip * dpi as i32 / 96
}

pub fn window_radius(dpi: u32) -> i32 {
    scale(WINDOW_RADIUS, dpi)
}

pub fn window_size(dpi: u32, rows: i32) -> (i32, i32) {
    let height = INPUT_HEIGHT + PADDING * 2 + ROW_HEIGHT * rows + FOOTER_HEIGHT;
    (scale(BASE_WIDTH, dpi), scale(height, dpi))
}

/// How tall the window needs to be for these rows, up to its full height. A short list gets a short
/// window rather than a dark gap, and an empty one keeps room for one line to say so.
pub fn fitted_height(dpi: u32, rows: &[Row], visible_rows: i32) -> i32 {
    let (_, full) = window_size(dpi, visible_rows);
    let list: i32 = (0..rows.len()).map(|at| scale(row_height_dip(rows, at), dpi)).sum();
    let room = scale(ROW_HEIGHT, dpi) * visible_rows;
    if list >= room {
        return full;
    }
    full - room + list.max(scale(ROW_HEIGHT, dpi))
}

/// The fewest rows a menu shows.
const POPOVER_MIN_ROWS: usize = 3;

/// How tall the launcher is for this list. A grid and the clipboard's preview always get the full
/// height, and other lists fit their rows unless that's turned off. A short list still leaves room
/// for a small menu, so opening one never makes the launcher jump taller and back.
pub fn launcher_height(dpi: u32, scope: &Scope, rows: &[Row], grid: bool, fit: bool, visible_rows: i32) -> i32 {
    // The weather and the speed test are drawn rather than listed, so they always get the room.
    let drawn = matches!(scope, Scope::Form { .. } | Scope::Weather | Scope::SpeedTest | Scope::Timer | Scope::Stopwatch | Scope::Colour(_));
    let height = if grid || !fit || is_split(scope, rows) || drawn { window_size(dpi, visible_rows).1 } else { fitted_height(dpi, rows, visible_rows) };
    height.max(popover_needs_height(POPOVER_MIN_ROWS, dpi, MenuSide::Right))
}

/// The clipboard shows what's selected beside its list, once there's anything in it.
pub fn is_split(scope: &Scope, rows: &[Row]) -> bool {
    *scope == Scope::Clipboard && rows.iter().any(|row| matches!(row.source, RowSource::Clip { .. }))
}

/// Where the list ends and the preview starts.
pub fn split_x(dpi: u32) -> i32 {
    scale(SPLIT_LIST_DIP, dpi)
}

/// The two parts of the clipboard's preview: what was copied, and the facts about it below.
pub struct SidePanel {
    pub preview: RECT,
    pub details: RECT,
}

pub fn side_panel(width: i32, height: i32, dpi: u32, detail_rows: usize) -> SidePanel {
    let left = split_x(dpi) + scale(PANEL_PAD, dpi);
    let right = width - scale(PANEL_PAD, dpi);
    let top = scale(INPUT_HEIGHT + PADDING, dpi) + scale(6, dpi);
    let bottom = height - scale(FOOTER_HEIGHT, dpi) - scale(PADDING, dpi);
    let details_top = (bottom - scale(DETAIL_TITLE + DETAIL_ROW * detail_rows as i32, dpi)).max(top);
    SidePanel {
        preview: RECT { left, top, right, bottom: (details_top - scale(14, dpi)).max(top) },
        details: RECT { left, top: details_top, right, bottom },
    }
}

/// The largest a copied picture is shown, in pixels, so the data thread scales it once.
pub fn preview_room(dpi: u32, visible_rows: i32) -> (usize, usize) {
    let (width, height) = window_size(dpi, visible_rows);
    let room = side_panel(width, height, dpi, DETAIL_ROWS_FOR_IMAGES).preview;
    ((room.right - room.left).max(0) as usize, (room.bottom - room.top).max(0) as usize)
}

/// How many rows a menu shows in a launcher this tall. It takes about 70% of the room between the
/// search box and the footer, so it never reaches the top, and it scrolls for the rest. It always
/// gets three, and only a list shorter than that makes the launcher grow for it.
pub fn popover_rows_for(height: i32, dpi: u32) -> usize {
    let room = height - scale(INPUT_HEIGHT + FOOTER_HEIGHT, dpi);
    let fixed = scale(POPOVER_PAD * 2 + POPOVER_TITLE + POPOVER_SEARCH, dpi);
    let row = scale(POPOVER_ROW, dpi).max(1);
    let share = (room * POPOVER_SHARE / 10 - fixed + row / 2) / row;
    let whole = (room - scale(POPOVER_GAP, dpi) - fixed) / row;
    share.min(whole).clamp(POPOVER_MIN_ROWS as i32, POPOVER_MAX_ROWS as i32) as usize
}

/// The same for a short menu that's always the same, such as Orbit's own, which is better whole
/// than scrolling. It takes all the room up to the search box.
pub fn popover_rows_most(height: i32, dpi: u32) -> usize {
    let room = height - scale(INPUT_HEIGHT + FOOTER_HEIGHT, dpi);
    let fixed = scale(POPOVER_PAD * 2 + POPOVER_TITLE + POPOVER_SEARCH, dpi);
    let whole = (room - scale(POPOVER_GAP, dpi) - fixed) / scale(POPOVER_ROW, dpi).max(1);
    whole.clamp(POPOVER_MIN_ROWS as i32, POPOVER_MAX_ROWS as i32) as usize
}

/// Where the rows from `scroll` on are drawn, as (index, top, height). Section titles are shorter
/// than rows, so more can fit than `visible_rows`, but never fewer.
pub fn row_layout(rows: &[Row], scroll: usize, dpi: u32, visible_rows: usize) -> Vec<(usize, i32, i32)> {
    let top = scale(INPUT_HEIGHT + PADDING, dpi);
    rows_down_to(rows, scroll, dpi, top + scale(ROW_HEIGHT, dpi) * visible_rows as i32)
}

/// Whether row `index` is drawn whole with the list scrolled to `scroll`, as `row_layout` lays it out.
pub fn row_fits(rows: &[Row], scroll: usize, index: usize, dpi: u32, visible_rows: usize) -> bool {
    if index < scroll || index >= rows.len() {
        return false;
    }
    let room = scale(ROW_HEIGHT, dpi) * visible_rows as i32;
    let used: i32 = (scroll..=index).map(|at| scale(row_height_dip(rows, at), dpi)).sum();
    used <= room
}

/// An answer that's the only result is a card.
pub fn is_answer_card(rows: &[Row]) -> bool {
    rows.len() == 1 && matches!(rows[0].source, RowSource::Answer { .. })
}

/// How tall a row is, in DIPs: section titles are short, the first shorter still, and a lone
/// answer is a card.
fn row_height_dip(rows: &[Row], index: usize) -> i32 {
    if rows[index].is_header() {
        if index == 0 { FIRST_HEADER_HEIGHT } else { HEADER_HEIGHT }
    } else if index == 0 && is_answer_card(rows) {
        ANSWER_CARD_HEIGHT
    } else {
        ROW_HEIGHT
    }
}

/// The rows from `scroll` that fit above `bottom`: where each starts, and its height.
fn rows_down_to(rows: &[Row], scroll: usize, dpi: u32, bottom: i32) -> Vec<(usize, i32, i32)> {
    let mut top = scale(INPUT_HEIGHT + PADDING, dpi);
    let mut out = Vec::with_capacity(16);
    for index in (0..rows.len()).skip(scroll) {
        let height = scale(row_height_dip(rows, index), dpi);
        if top + height > bottom {
            break;
        }
        out.push((index, top, height));
        top += height;
    }
    // A title with no room for anything under it says nothing.
    if out.last().is_some_and(|&(index, _, _)| rows[index].is_header()) {
        out.pop();
    }
    out
}

/// Which row is under a client-area y, if any. Section titles don't count.
pub fn row_at(y: i32, dpi: u32, rows: &[Row], scroll: usize, visible_rows: usize) -> Option<usize> {
    row_layout(rows, scroll, dpi, visible_rows)
        .into_iter()
        .find(|&(index, top, height)| y >= top && y < top + height && !rows[index].is_header())
        .map(|(index, _, _)| index)
}

/// Where a row's icon starts.
const ICON_LEFT: i32 = ROW_INSET + 10;
/// Where a row's title starts. Typing lines up with it, whatever the scope.
const TEXT_LEFT: i32 = ICON_LEFT + ICON_DIP + 12;

/// Where typing starts.
pub fn input_left(_scope: &Scope, dpi: u32) -> i32 {
    scale(TEXT_LEFT, dpi)
}

const KEYCAP_DIP: i32 = 20;

/// One key name as it appears on a cap. Enter is a symbol, as on most keyboards.
fn keycap_text(key: &str) -> (&str, bool) {
    match key {
        "Enter" => ("\u{E751}", true),
        "Up" => ("\u{E74A}", true),
        "Down" => ("\u{E74B}", true),
        _ => (key, false),
    }
}

/// HyperKey's cap carries its mark, a filled bolt, rather than the word.
const HYPER: &str = "Hyper";

/// The bolt, as the outline is on HyperKey's Settings page, in the unit square.
const BOLT: [(f32, f32); 6] = [(0.62, 0.0), (0.14, 0.57), (0.45, 0.57), (0.36, 1.0), (0.86, 0.41), (0.55, 0.41)];

/// How wide one cap is: square for a single character or a symbol, wider for a word.
fn cap_width(key: &str, height: i32, pad: i32, measure: impl Fn(&str, bool) -> i32) -> i32 {
    if key == HYPER {
        return height;
    }
    let (text, is_icon) = keycap_text(key);
    (measure(text, is_icon) + pad * 2).max(height)
}

/// HyperKey's bolt in the middle of a square cap.
fn draw_bolt(buffer: &mut Backbuffer, left: i32, top: i32, size: i32, ink: Rgb) {
    let mark = size as f32 * 0.6;
    let x = left as f32 + size as f32 / 2.0 - mark / 2.0;
    let y = top as f32 + (size as f32 - mark) / 2.0;
    let points = BOLT.map(|(px, py)| (x + px * mark, y + py * mark));
    buffer.fill_polygon_alpha(&points, ink, 255);
}

/// How wide `keys`, such as `Hyper+C`, will be when drawn as key caps.
pub fn keycaps_width(buffer: &Backbuffer, keys: &str, font: HFONT, icon_font: HFONT, dpi: u32) -> i32 {
    let (height, pad, gap) = (scale(KEYCAP_DIP, dpi), scale(6, dpi), scale(3, dpi));
    let mut width = 0;
    for key in keys.split('+') {
        width += cap_width(key, height, pad, |text, is_icon| buffer.text_width(text, if is_icon { icon_font } else { font })) + gap;
    }
    width - gap
}

/// Draws `keys` as key caps, with the row's middle at `middle_y`. Returns where they end.
#[allow(clippy::too_many_arguments)]
pub fn draw_keycaps(buffer: &mut Backbuffer, keys: &str, x: i32, middle_y: i32, font: HFONT, icon_font: HFONT, fill: (Rgb, u8), ink: Rgb, dpi: u32) -> i32 {
    let (height, pad, gap) = (scale(KEYCAP_DIP, dpi), scale(6, dpi), scale(3, dpi));
    let top = middle_y - height / 2;
    let mut at = x;
    for key in keys.split('+') {
        let width = cap_width(key, height, pad, |text, is_icon| buffer.text_width(text, if is_icon { icon_font } else { font }));
        buffer.fill_round_rect_alpha(at, top, width, height, scale(5, dpi) as f32, fill.0, fill.1);
        if key == HYPER {
            draw_bolt(buffer, at, top, height, ink);
        } else {
            let (text, is_icon) = keycap_text(key);
            let rect = RECT { left: at, top, right: at + width, bottom: top + height };
            if is_icon {
                buffer.symbol_centered(text, icon_font, scale(12, dpi), false, ink, rect);
            } else {
                buffer.text_centered(text, font, ink, rect);
            }
        }
        at += width + gap;
    }
    at - gap
}

/// How wide the launcher's own key caps will be.
fn caps_width(keys: &str, fonts: &Fonts, dpi: u32) -> i32 {
    let (height, pad, gap) = (scale(KEYCAP_DIP, dpi), scale(6, dpi), scale(3, dpi));
    let mut width = 0;
    for key in keys.split('+') {
        width += cap_width(key, height, pad, |label, is_icon| text::width(label, if is_icon { &fonts.icon_keycap } else { &fonts.keycap })) + gap;
    }
    width - gap
}

/// The launcher's key caps, each a soft fill with a lighter rim, like a key seen from above.
#[allow(clippy::too_many_arguments)]
fn draw_caps(buffer: &mut Backbuffer, keys: &str, x: i32, middle_y: i32, fonts: &Fonts, fill: (Rgb, u8), rim: Option<(Rgb, u8)>, ink: Rgb, dpi: u32) {
    let (height, pad, gap) = (scale(KEYCAP_DIP, dpi), scale(6, dpi), scale(3, dpi));
    let top = middle_y - height / 2;
    let radius = scale(CAP_RADIUS, dpi) as f32;
    let mut at = x;
    for key in keys.split('+') {
        let face = |is_icon: bool| if is_icon { &fonts.icon_keycap } else { &fonts.keycap };
        let width = cap_width(key, height, pad, |label, is_icon| text::width(label, face(is_icon)));
        buffer.fill_round_rect_alpha(at, top, width, height, radius, fill.0, fill.1);
        if let Some(rim) = rim {
            buffer.stroke_round_rect_alpha(at, top, width, height, radius, hairline(dpi) as f32, rim.0, rim.1);
        }
        if key == HYPER {
            draw_bolt(buffer, at, top, height, ink);
        } else {
            let (label, is_icon) = keycap_text(key);
            text::draw(buffer, label, face(is_icon), ink, RECT { left: at, top, right: at + width, bottom: top + height }, Align::Center);
        }
        at += width + gap;
    }
}

fn hairline(dpi: u32) -> i32 {
    1.max(dpi as i32 / 96)
}

/// A command's symbol, in the icon style chosen in Settings.
fn draw_tile(buffer: &mut Backbuffer, paint: &Paint, glyph: char, x: i32, y: i32, size: i32, selected: bool) {
    draw_tile_in(buffer, paint, glyph, tile_colors(glyph), false, x, y, size, selected);
}

/// The same in given colours. A symbol whose colour `means` something, such as a pull request's
/// state, keeps it however plain the icon style is.
#[allow(clippy::too_many_arguments)]
fn draw_tile_in(buffer: &mut Backbuffer, paint: &Paint, glyph: char, colours: (Rgb, Rgb), means: bool, x: i32, y: i32, size: i32, selected: bool) {
    use orbit_core::settings::IconStyle;
    let radius = scale(TILE_RADIUS, paint.dpi) as f32;
    let rect = RECT { left: x, top: y, right: x + size, bottom: y + size };
    let theme = paint.theme;
    let coloured = crate::rows::coloured_symbol(colours, paint.dark);
    let (face, ink) = match crate::rows::icon_style() {
        IconStyle::Plain if means => (&paint.fonts.icon_row, coloured),
        IconStyle::Plain => (&paint.fonts.icon_row, if selected { theme.text } else { theme.secondary }),
        IconStyle::Coloured => (&paint.fonts.icon_row, coloured),
        IconStyle::GreyTiles => {
            let fill = if selected { paint.tile_on_selection } else { paint.tile };
            buffer.fill_round_rect_alpha(x, y, size, size, radius, fill.0, fill.1);
            (&paint.fonts.icon_tile, if means { coloured } else { theme.text })
        }
        IconStyle::TintedTiles => {
            let (fill, alpha, ink) = crate::rows::tinted_tile(colours, paint.dark);
            buffer.fill_round_rect_alpha(x, y, size, size, radius, fill, alpha);
            (&paint.fonts.icon_on_tile, ink)
        }
    };
    text::draw(buffer, &glyph.to_string(), face, ink, rect, Align::Center);
}

/// The search box's text and caret. The box itself isn't drawn, because a standard control can't
/// paint over glass. It still handles all the typing.
pub struct Field {
    pub text: String,
    pub selection: (usize, usize),
}

/// Which box a stretch of drawn text is typed in.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SpotOf {
    /// The search box.
    Search,
    /// The box along the bottom of a menu, which is the search box too, while the menu is open.
    MenuSearch,
    /// One of a quicklink's blanks, in the search bar.
    LinkArg(usize),
    /// One of a form's fields.
    Field(usize),
}

/// Which of the fonts a box's text is in.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SpotFace {
    Query,
    Title,
    Body,
    Small,
}

/// Text someone can type in, as it was last drawn, so a click in it can put the caret where it
/// landed. The boxes that take the typing are hidden, so they can't tell where that is.
#[derive(Clone, Debug)]
pub struct TextSpot {
    pub of: SpotOf,
    /// Where a click counts as in it: the whole box.
    pub hit: RECT,
    /// Where its text was drawn.
    pub clip: RECT,
    pub text: String,
    pub face: SpotFace,
    /// Several lines, as a snippet's box has.
    pub multiline: bool,
    /// How far one line was slid left, or several lines scrolled up, to keep the caret in view.
    pub shift: i32,
}

impl TextSpot {
    pub fn contains(&self, x: i32, y: i32) -> bool {
        x >= self.hit.left && x < self.hit.right && y >= self.hit.top && y < self.hit.bottom
    }

    /// Where in its text a point is, in UTF-16 units, as the box that takes the typing counts.
    pub fn index_at(&self, fonts: &Fonts, x: i32, y: i32) -> usize {
        let face = match self.face {
            SpotFace::Query => &fonts.query,
            SpotFace::Title => &fonts.title,
            SpotFace::Body => &fonts.body,
            SpotFace::Small => &fonts.small,
        };
        if self.multiline {
            text::editable_index_at(&self.text, face, self.clip, self.shift, x, y)
        } else {
            text::index_at(&self.text, face, x - (self.clip.left - self.shift))
        }
    }
}

/// The text spots drawn this time, and the last time's, whose slides carry on.
struct Spots {
    prior: Vec<TextSpot>,
    now: Vec<TextSpot>,
}

impl Spots {
    fn shift(&self, of: SpotOf) -> i32 {
        self.prior.iter().find(|spot| spot.of == of).map_or(0, |spot| spot.shift)
    }

    #[allow(clippy::too_many_arguments)]
    fn add(&mut self, of: SpotOf, hit: RECT, clip: RECT, text: &str, face: SpotFace, multiline: bool, shift: i32) {
        self.now.push(TextSpot { of, hit, clip, text: text.to_string(), face, multiline, shift });
    }
}

/// The first `units` UTF-16 units of `text`.
fn prefix(text: &str, units: usize) -> String {
    let head: Vec<u16> = text.encode_utf16().take(units).collect();
    String::from_utf16_lossy(&head)
}

/// How far the window reaches past the launcher's edge on every side, to hold its soft shadow.
pub fn frame_margin(dpi: u32) -> i32 {
    scale(SHADOW_MARGIN, dpi)
}

/// The window's size for a launcher of this size.
pub fn frame_size(dpi: u32, panel: (i32, i32)) -> (i32, i32) {
    let margin = frame_margin(dpi);
    (panel.0 + margin * 2, panel.1 + margin * 2)
}

/// Where the small menu goes and how its parts are laid out, inside the launcher.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Popover {
    pub rect: RECT,
    /// Where the first row starts. Rows are `row_height` tall.
    pub list_top: i32,
    pub row_height: i32,
    /// The box you type in to narrow the menu down, from its top to its bottom: along the bottom of
    /// a menu from the footer, and at the top of a drop-down, next to the button it came from.
    pub search_top: i32,
    pub search_bottom: i32,
    /// Whether it says what it's about above its rows. A drop-down's button says that already.
    pub titled: bool,
}

fn popover_height(shown: usize, dpi: u32, side: MenuSide) -> i32 {
    let title = if side == MenuSide::Drop { 0 } else { POPOVER_TITLE };
    scale(POPOVER_PAD * 2 + title + POPOVER_ROW * shown as i32 + POPOVER_SEARCH, dpi)
}

/// How tall the launcher has to be so the menu fits between the search box and the footer.
pub fn popover_needs_height(shown: usize, dpi: u32, side: MenuSide) -> i32 {
    let above = if side == MenuSide::Drop { drop_top(dpi) } else { scale(INPUT_HEIGHT, dpi) };
    above + scale(POPOVER_GAP + FOOTER_HEIGHT, dpi) + popover_height(shown, dpi, side)
}

/// The search box's right edge, where its filter drop-down ends.
const BAND_RIGHT_DIP: i32 = 18;
/// How tall the filter drop-down is, and how far below it its menu starts.
const FILTER_BUTTON_DIP: i32 = 30;
const DROP_GAP_DIP: i32 = 5;
const DROP_WIDTH: i32 = 290;

/// Where a drop-down's menu starts: just under the button, which sits in the middle of the
/// search box's height.
fn drop_top(dpi: u32) -> i32 {
    (scale(INPUT_HEIGHT, dpi) + scale(FILTER_BUTTON_DIP, dpi)) / 2 + scale(DROP_GAP_DIP, dpi)
}

/// How many rows a drop-down's menu shows in a launcher this tall: as many as fit above the
/// footer, since it hangs from the top rather than covering the list from below.
pub fn drop_rows_for(height: i32, dpi: u32) -> usize {
    let room = height - drop_top(dpi) - scale(POPOVER_GAP + FOOTER_HEIGHT, dpi);
    let fixed = popover_height(0, dpi, MenuSide::Drop);
    let whole = (room - fixed) / scale(POPOVER_ROW, dpi).max(1);
    whole.clamp(POPOVER_MIN_ROWS as i32, POPOVER_MAX_ROWS as i32) as usize
}

pub fn popover_layout(width: i32, height: i32, dpi: u32, side: MenuSide, shown: usize) -> Popover {
    let h = popover_height(shown, dpi, side);
    let inset = scale(ROW_INSET, dpi);
    if side == MenuSide::Drop {
        // Hangs from the drop-down at the right of the search box, lined up with its right edge,
        // with the box to narrow it down at the top, nearest the button.
        let (w, right, top) = (scale(DROP_WIDTH, dpi), width - scale(BAND_RIGHT_DIP, dpi), drop_top(dpi));
        let search_bottom = top + scale(POPOVER_SEARCH, dpi);
        return Popover {
            rect: RECT { left: right - w, top, right, bottom: top + h },
            list_top: search_bottom + scale(POPOVER_PAD, dpi),
            row_height: scale(POPOVER_ROW, dpi),
            search_top: top,
            search_bottom,
            titled: false,
        };
    }
    let w = scale(POPOVER_WIDTH, dpi);
    let left = if side == MenuSide::Left { inset } else { width - inset - w };
    let bottom = height - scale(FOOTER_HEIGHT, dpi);
    let top = bottom - h;
    Popover {
        rect: RECT { left, top, right: left + w, bottom },
        list_top: top + scale(POPOVER_TITLE + POPOVER_PAD, dpi),
        row_height: scale(POPOVER_ROW, dpi),
        search_top: bottom - scale(POPOVER_SEARCH, dpi),
        search_bottom: bottom,
        titled: true,
    }
}

impl Popover {
    /// The same menu, `dy` lower down, as it is on its way in.
    pub fn shifted(self, dy: i32) -> Popover {
        let rect = RECT { top: self.rect.top + dy, bottom: self.rect.bottom + dy, ..self.rect };
        Popover { rect, list_top: self.list_top + dy, search_top: self.search_top + dy, search_bottom: self.search_bottom + dy, ..self }
    }

    pub fn contains(&self, x: i32, y: i32) -> bool {
        x >= self.rect.left && x < self.rect.right && y >= self.rect.top && y < self.rect.bottom
    }

    /// Which row is under a point, given how far the list has scrolled.
    pub fn row_at(&self, x: i32, y: i32, rows: usize, scroll: usize, shown: usize) -> Option<usize> {
        if !self.contains(x, y) || y < self.list_top || y >= self.list_top + self.row_height * shown as i32 {
            return None;
        }
        let row = ((y - self.list_top) / self.row_height) as usize;
        (row < shown && scroll + row < rows).then_some(scroll + row)
    }
}

/// Which tile is under a point in the launcher, for a list drawn as a grid.
pub fn grid_row_at(grid: &Grid, first: usize, x: i32, y: i32, width: i32, dpi: u32) -> Option<usize> {
    let inset = scale(ROW_INSET, dpi);
    if x < inset || x >= width - inset {
        return None;
    }
    let top = scale(INPUT_HEIGHT + PADDING, dpi);
    let across = (x - inset) * COLUMNS as i32 / (width - inset * 2).max(1);
    // In DIPs down, and in columns across, so the same arithmetic works at any scale.
    grid.row_at(first, across * 100, (y - top) * 96 / dpi as i32, COLUMNS as i32 * 100)
}

/// Colours and sizes everything below shares.
struct Paint<'a> {
    theme: Theme,
    fonts: &'a Fonts,
    dpi: u32,
    hc: bool,
    dark: bool,
    keycap: (Rgb, u8),
    keycap_on_selection: (Rgb, u8),
    selection: (Rgb, u8),
    pressed: (Rgb, u8),
    tile: (Rgb, u8),
    tile_on_selection: (Rgb, u8),
    pill: (Rgb, u8),
    divider: (Rgb, u8),
    cap_rim: Option<(Rgb, u8)>,
    thin: i32,
}


/// A list's rows, and which of them is chosen.
struct ListState<'a> {
    rows: &'a [Row],
    scroll: usize,
    selected: usize,
    scope: &'a Scope,
    query: &'a str,
    grid: Option<&'a Grid>,
    /// Where the selection is drawn, which glides behind the selected row. `None` draws it on the row.
    highlight: Option<RECT>,
}

/// Draws the current state into the frame. Allocates what was shed while the launcher was hidden.
pub fn render(app: &mut App) {
    let dpi = app.dpi;
    let visible = app.settings.launcher.max_visible_rows.clamp(3, 12) as i32;
    let (width, full_height) = window_size(dpi, visible);
    let theme = app.theme;
    let dark = theme.overlay == Rgb(255, 255, 255);
    let hc = theme.high_contrast;
    POPOVER_OPEN.store(app.menu.is_some(), Ordering::Relaxed);

    // Whatever is behind the menu decides the window's size, so opening one doesn't jolt it.
    let behind_rows = app.menu.as_ref().map_or(&app.view.rows, |menu| &menu.behind.rows);
    let behind_scope = app.menu.as_ref().map_or(&app.view.scope, |menu| &menu.behind.scope);
    let is_grid = app.menu.as_ref().map_or(app.view.grid.is_some(), |menu| menu.behind.grid.is_some());
    let mut height = launcher_height(dpi, behind_scope, behind_rows, is_grid, app.settings.launcher.fit_results, visible);
    if let Some(menu) = &app.menu {
        height = height.max(popover_needs_height(menu.shown, dpi, menu.side));
    }

    // What moves: the bottom edge eases to where the results want it, a new page fades in over the
    // last, and a menu rises into place. Everything is laid out where it's going straight away, so
    // keys and clicks never wait for any of it.
    let now = app.motion_clock.unwrap_or_else(Instant::now);
    let animate = app.motion_on;
    let height = app.motion.height.aim(height as f32, motion::RESIZE, now, animate).round() as i32;
    let depth = app.view.stack.len() - usize::from(app.menu.is_some());
    let page = motion::key((page_key(behind_scope), depth));
    let menu_key = app.menu.as_ref().map(|menu| motion::key((menu.side as u8, page_key(&app.view.scope))));
    let menu_was = app.motion.menu_open.map(|(key, _)| key);
    // The first frame since everything settled shows things as they are, a menu included.
    let first = app.motion.page.is_none();
    if menu_key != menu_was {
        app.motion.menu_open = menu_key.map(|key| (key, (animate && !first).then_some(now)));
        app.motion.menu_selection.reset();
    }
    let page_changed = app.motion.page.is_some_and(|last| last != page);
    let menu_closed = menu_was.is_some() && menu_key.is_none();
    app.motion.page = Some(page);
    if animate
        && (page_changed || menu_closed)
        && let Some(last) = app.backbuffer.as_ref().filter(|last| last.width == width)
    {
        app.motion.crossfade = Some(Crossfade { pixels: last.snapshot(), width, height: last.height, start: now });
    }

    // The bitmap has room for the launcher at its tallest, so a height that changes on every frame
    // doesn't make a new one each time.
    let room = full_height.max(height);
    let fits = app.backbuffer.as_mut().is_some_and(|buffer| buffer.width == width && buffer.set_height(height));
    if !fits {
        app.backbuffer = None;
        app.backbuffer = Backbuffer::with_room(width, height, room);
    }
    let vim = app.vim_mode();
    let input_x = input_left(&app.view.scope, dpi);
    app.place_edit(input_x);
    let field = app.read_field();
    let area_field = app.read_area_field();
    let filter_label = app.filter_label();
    let filter_open = app.filter_menu_open();
    let placeholder = match &app.menu {
        Some(menu) => menu.behind.scope.placeholder(),
        None => app.placeholder_text(),
    };
    let glass = app.glass;
    let caret_on = app.caret_on && !matches!(app.view.scope, Scope::SetHotkey { .. } | Scope::Confirm(_));

    let surface = if hc { theme.background } else if dark { SURFACE_DARK } else { SURFACE_LIGHT };
    let tint = if !glass { 255 } else if dark { GLASS_ALPHA_DARK } else { GLASS_ALPHA_LIGHT };
    let radius = scale(WINDOW_RADIUS, dpi) as f32;

    // In high contrast, layers become the exact colours Windows asks for.
    let layer = |solid: Rgb, alpha: u8| if hc { (solid, 255) } else { (theme.overlay, alpha) };
    {
        let Some(fonts) = app.fonts.as_ref() else { return };
        let Some(buffer) = app.backbuffer.as_mut() else { return };
        let paint = Paint {
            theme,
            fonts,
            dpi,
            hc,
            dark,
            keycap: layer(theme.keycap, ALPHA_KEYCAP),
            keycap_on_selection: layer(theme.keycap_on_selection, ALPHA_KEYCAP_ON_SELECTION),
            selection: layer(theme.selection, ALPHA_SELECTION),
            pressed: layer(theme.keycap_on_selection, ALPHA_PRESSED),
            tile: layer(theme.keycap, ALPHA_TILE),
            tile_on_selection: layer(theme.keycap_on_selection, ALPHA_KEYCAP),
            pill: layer(theme.footer, ALPHA_PILL),
            divider: layer(theme.divider, ALPHA_DIVIDER),
            cap_rim: (!hc).then_some((theme.overlay, ALPHA_CAP_RIM)),
            thin: hairline(dpi),
        };
        let thin = paint.thin;
        let mut spots = Spots { prior: std::mem::take(&mut app.text_spots), now: Vec::new() };

        buffer.clear(surface, tint);
        if hc {
            buffer.stroke_round_rect_alpha(0, 0, width, height, radius, thin as f32, theme.edge, 255);
        } else {
            // Light falling from above, and an edge that catches it, lift the sheet off what's behind.
            buffer.wash_down(0, scale(SHEEN_DIP, dpi), theme.overlay, ALPHA_SHEEN);
            buffer.stroke_round_rect_alpha(0, 0, width, height, radius, thin as f32, theme.overlay, ALPHA_EDGE);
        }

        // The search box, or with a menu open, what it held when the menu opened.
        let depth = app.view.stack.len() - usize::from(app.menu.is_some());
        let shown_scope = app.menu.as_ref().map_or(&app.view.scope, |menu| &menu.behind.scope);
        app.footer_buttons.clear();
        if depth > 0 {
            // Backing out is a button here, where the search symbol was.
            let size = scale(ICON_DIP + 6, dpi);
            let left = scale(ICON_LEFT, dpi) - scale(3, dpi);
            let top = (scale(INPUT_HEIGHT, dpi) - size) / 2;
            let area = RECT { left, top, right: left + size, bottom: top + size };
            if app.hovered_button == Some(BUTTON_BACK) {
                let fill = if app.pressed_button == Some(BUTTON_BACK) { paint.pressed } else { paint.selection };
                buffer.fill_round_rect_alpha(left, top, size, size, size as f32 / 2.0, fill.0, fill.1);
            }
            text::draw(buffer, &GLYPH_BACK.to_string(), &fonts.icon_row, theme.text, area, Align::Center);
            app.footer_buttons.push((area, BUTTON_BACK));
        } else {
            let icon_size = scale(ICON_DIP, dpi);
            let icon_x = scale(ICON_LEFT, dpi);
            let rect = RECT { left: icon_x, top: 0, right: icon_x + icon_size, bottom: scale(INPUT_HEIGHT, dpi) };
            text::draw(buffer, &shown_scope.input_glyph().to_string(), &fonts.icon_large, theme.secondary, rect, Align::Center);
        }
        let mut band = RECT { left: input_x, top: 0, right: width - scale(BAND_RIGHT_DIP, dpi), bottom: scale(INPUT_HEIGHT, dpi) };
        if let Some((label, narrowed)) = &filter_label {
            let look = ButtonLook {
                hovered: app.hovered_button == Some(BUTTON_FILTER),
                pressed: app.pressed_button == Some(BUTTON_FILTER),
                open: filter_open,
            };
            let area = draw_filter_button(buffer, &paint, label, *narrowed, band, look);
            app.footer_buttons.push((area, BUTTON_FILTER));
            band.right = area.left - scale(12, dpi);
        }
        match (&app.menu, &app.view.scope, &app.link_args) {
            (Some(menu), _, _) => {
                let parked = Field { text: menu.behind.query.clone(), selection: (0, 0) };
                draw_field(buffer, &paint, &parked, &placeholder, band, &fonts.query, QUERY_DIP, false, None, 0);
            }
            (None, Scope::LinkArgs { of, .. }, Some(args)) => {
                draw_link_args(buffer, &paint, of, args, &field, band, caret_on, vim, &mut app.link_arg_boxes, &mut spots);
            }
            (None, Scope::Form { title, .. }, _) => {
                text::draw(buffer, title, &fonts.query, theme.text, band, Align::Left);
            }
            _ => {
                let shift = draw_field(buffer, &paint, &field, &placeholder, band, &fonts.query, QUERY_DIP, caret_on, vim, spots.shift(SpotOf::Search));
                spots.add(SpotOf::Search, band, band, &field.text, SpotFace::Query, false, shift);
            }
        }

        // The list: the view's own rows, or what the menu opened over.
        let mut list = match &app.menu {
            Some(menu) => ListState {
                rows: &menu.behind.rows,
                scroll: menu.behind.scroll,
                selected: menu.behind.selected,
                scope: &menu.behind.scope,
                query: &menu.behind.query,
                grid: menu.behind.grid.as_ref(),
                highlight: None,
            },
            None => ListState {
                rows: &app.view.rows,
                scroll: app.view.scroll,
                selected: app.view.selected,
                scope: &app.view.scope,
                query: &field.text,
                grid: app.view.grid.as_ref(),
                highlight: None,
            },
        };
        let form = match (&app.menu, &app.form) {
            (None, Some(form)) if matches!(app.view.scope, Scope::Form { .. }) => Some(form),
            _ => None,
        };
        // These stay drawn behind their Ctrl+K menu.
        if *list.scope == Scope::Weather {
            draw_weather(buffer, &paint, &mut app.weather, width, height);
        } else if *list.scope == Scope::SpeedTest {
            draw_speedtest(buffer, &paint, &app.speedtest, app.settings.speedtest_accepted, width, height);
        } else if matches!(list.scope, Scope::Timer | Scope::Stopwatch) {
            let stopwatch = *list.scope == Scope::Stopwatch;
            draw_clock(buffer, &paint, app.countdown.as_ref(), &app.stopwatch, stopwatch, list.query, width, height);
        } else if let Scope::Colour(colour) = list.scope {
            draw_colour(buffer, &paint, *colour, width, height);
        } else if let Some(form) = form {
            let boxes = draw_form(buffer, &paint, form, &field, &area_field, width, height, caret_on, &app.icons, &mut spots);
            app.form_boxes = boxes.0;
            app.form_chips = boxes.1;
        } else {
        // The selection glides from where it was to the row now chosen, as long as it's the same list,
        // scrolled the same: a list that's changed under it has it jump instead.
        let glide = motion::key((page, list.query, list.rows.len(), list.scroll, width));
        match list.grid {
            Some(grid) => {
                let cells = grid_cells(&list, grid, width, dpi);
                let target = cells.iter().find(|cell| cell.index == list.selected).map(|cell| cell.rect);
                list.highlight = target.map(|target| app.motion.selection.aim(glide, target, now, animate).to_rect());
                if target.is_none() {
                    app.motion.selection.reset();
                }
                draw_grid(buffer, &paint, &list, grid, &cells, width, &app.emoji_table, &mut app.emoji_cache);
            }
            None => {
                // The list carries on under the footer and fades out there, rather than stopping
                // short with an empty band. Only when there's more below what can be chosen.
                let laid = row_layout(list.rows, list.scroll, dpi, visible as usize);
                let fade = laid.last().and_then(|&(index, top, row_h)| FooterFade::new(top + row_h, height, index + 1 < list.rows.len()));
                let saved = fade.as_ref().map(|fade| buffer.save_rows(fade.from, height));
                // Past the bottom too, so a title near it keeps the row under it, faint as that is.
                let reach = fade.as_ref().map_or(height, |_| height + scale(ROW_HEIGHT, dpi) * 2);
                let split = is_split(list.scope, list.rows);
                let list_width = if split { split_x(dpi) } else { width };
                let target = selection_rect(&list, list_width, reach, dpi);
                list.highlight = target.map(|target| app.motion.selection.aim(glide, target, now, animate).to_rect());
                if target.is_none() {
                    app.motion.selection.reset();
                }
                draw_list(buffer, &paint, &list, list_width, visible, reach, &app.icons, split);
                if let (Some(fade), Some(saved)) = (fade, saved) {
                    buffer.fade_back(fade.from, &saved, fade.from, fade.to);
                }
                if split {
                    draw_side_panel(buffer, &paint, app.clip_preview.as_deref(), width, height);
                }
            }
        }
        }

        // Footer: a pill for Orbit on the left, and one for what the keys do on the right.
        // Ctrl+K's actions come up on the right; the app picker does too, but isn't them.
        let picking_app = matches!(&app.view.scope, Scope::Actions { of } if of == APP_PICKER_TITLE);
        let menu_kind = app.menu.as_ref().map(|menu| if menu.side == MenuSide::Right && !picking_app { MenuKind::Actions } else { MenuKind::Other });
        let hints = app.view.footer_hints(vim, menu_kind);
        let status: std::borrow::Cow<str> = match (&app.status, list.grid.is_some()) {
            (Some(status), _) => status.as_str().into(),
            // The name of what's selected, since tiles have nowhere else to show it.
            (None, true) => match list.rows.get(list.selected) {
                Some(row) if !row.is_header() => format!("{} · {}", list.scope.label(), row.title).into(),
                _ => list.scope.label().into(),
            },
            (None, false) => list.scope.label().into(),
        };
        let orbit_open = app.menu.as_ref().is_some_and(|menu| menu.side == MenuSide::Left);
        draw_footer(
            buffer,
            &paint,
            FooterState {
                hints: &hints,
                status: &status,
                is_status: app.status.is_some(),
                vim,
                logo: app.logo.as_deref(),
                hovered: app.hovered_button,
                pressed: app.pressed_button,
                orbit_open,
                actions_open: menu_kind == Some(MenuKind::Actions),
            },
            width,
            height,
            &mut app.footer_buttons,
        );

        if let Some(menu) = &app.menu {
            let of = match &app.view.scope {
                Scope::Actions { of } => of.as_str(),
                _ => "",
            };
            let geometry = popover_layout(width, height, dpi, menu.side, menu.shown);
            // A menu rises from the footer, or drops from its button, and is there at once: it's
            // mostly opaque by the second frame, and only its last few pixels of travel take longer.
            let arrived = app.motion.menu_in(now);
            let travel = (scale(MENU_RISE_DIP, dpi) as f32 * (1.0 - arrived)).round() as i32;
            let shift = if menu.side == MenuSide::Drop { -travel } else { travel };
            let at = geometry.shifted(shift);
            let target = popover_selection(geometry, &app.view, menu.shown, dpi);
            let glide = motion::key((menu_key, &field.text, app.view.rows.len(), app.view.scroll));
            let highlight = target.map(|target| {
                let mut rect = app.motion.menu_selection.aim(glide, target, now, animate).to_rect();
                rect.top += shift;
                rect.bottom += shift;
                rect
            });
            if arrived < 1.0 {
                let reach = scale(POPOVER_SHADOW_DIP + 4, dpi);
                let area = RECT { left: at.rect.left - reach, top: at.rect.top - reach, right: at.rect.right + reach, bottom: at.rect.bottom + reach };
                let saved = buffer.save_area(area);
                draw_popover(buffer, &paint, at, menu.shown, of, &field, caret_on, &app.view, &app.icons, &mut spots, highlight);
                buffer.mix_back_area(area, &saved, (0.3 + arrived * 1.4).min(1.0));
            } else {
                draw_popover(buffer, &paint, at, menu.shown, of, &field, caret_on, &app.view, &app.icons, &mut spots, highlight);
            }
        }

        // The last page, fading out over this one. Not over the footer, which stays where it is
        // while the bottom edge moves, so an old one left fading where it was would read as two.
        if let Some(fade) = &app.motion.crossfade {
            let left = fade.remaining(now);
            let reach = fade.height.min(height) - scale(FOOTER_HEIGHT, dpi);
            buffer.fade_over(&fade.pixels, fade.width, reach, left);
        }
        app.text_spots = spots.now;
    }
    if app.motion.crossfade.as_ref().is_some_and(|fade| fade.remaining(now) <= 0.0) {
        app.motion.crossfade = None;
    }
    app.motion.unsettled = app.motion.moving(now);

    // Round it off and put the shadow around it. The window only ever shows this finished frame.
    let shadow = Shadow {
        margin: frame_margin(dpi),
        drop: scale(SHADOW_DROP, dpi) as f32,
        blur: scale(SHADOW_BLUR, dpi) as f32,
        strength: if hc { 0.0 } else if dark { SHADOW_DARK } else { SHADOW_LIGHT },
    };
    let (frame_w, frame_h) = frame_size(dpi, (width, height));
    // The shadow only depends on the launcher's size, so it's painted again only when that changes:
    // on every frame while the bottom edge moves, and not otherwise.
    let kept = app.frame_shadow == Some(shadow)
        && app.frame.as_mut().is_some_and(|frame| frame.width == frame_w && frame.room() >= frame_h);
    let reshaped = match app.frame.as_mut() {
        Some(frame) if kept => {
            let before = frame.height;
            frame.set_height(frame_h);
            before != frame_h
        }
        _ => {
            app.frame = None;
            app.frame = Backbuffer::with_room(frame_w, frame_h, frame_size(dpi, (width, room)).1);
            app.frame_shadow = Some(shadow);
            true
        }
    };
    let (Some(frame), Some(panel)) = (app.frame.as_mut(), app.backbuffer.as_ref()) else { return };
    if reshaped {
        paint_shadow(frame, width, height, radius, shadow);
    }
    compose_panel(frame, panel, radius, shadow);
}

/// What makes a page itself, for telling when another has replaced it. A form moving to its next
/// field, or a colour being picked again, is still the same page.
fn page_key(scope: &Scope) -> u64 {
    let kind = std::mem::discriminant(scope);
    match scope {
        Scope::Extension { ext, command, .. } => motion::key((kind, ext, command)),
        Scope::Actions { of } => motion::key((kind, of)),
        Scope::Vault(tool) => motion::key((kind, format!("{tool:?}"))),
        _ => motion::key(kind),
    }
}

/// Where the selected row's highlight goes in a list `width` wide, if it's one that has one.
fn selection_rect(list: &ListState, width: i32, reach: i32, dpi: u32) -> Option<RECT> {
    let (index, y, row_h) = rows_down_to(list.rows, list.scroll, dpi, reach).into_iter().find(|&(index, _, _)| index == list.selected)?;
    let row = list.rows.get(index)?;
    if row.is_header() || (index == 0 && is_answer_card(list.rows)) {
        return None;
    }
    let row_x = scale(ROW_INSET, dpi);
    Some(RECT { left: row_x, top: y, right: width - row_x, bottom: y + row_h })
}

/// Where the highlight goes in a menu, under its selected row, if that's showing.
fn popover_selection(at: Popover, view: &crate::view::View, shown: usize, dpi: u32) -> Option<RECT> {
    if view.selected >= view.rows.len() {
        return None;
    }
    let slot = view.selected.checked_sub(view.scroll).filter(|&slot| slot < shown)?;
    let inset = scale(POPOVER_ROW_INSET, dpi);
    let y = at.list_top + slot as i32 * at.row_height;
    Some(RECT { left: at.rect.left + inset, top: y, right: at.rect.right - inset, bottom: y + at.row_height })
}

/// How a button is being pointed at.
#[derive(Clone, Copy, Default)]
struct ButtonLook {
    hovered: bool,
    pressed: bool,
    /// What it opens is open.
    open: bool,
}

/// The longest a filter's name gets in its drop-down before it's cut short.
const FILTER_LABEL_MOST_DIP: i32 = 190;

/// The drop-down at the right of the search box that says which of an extension's filters is in
/// force, such as "Waiting for your review", and opens the rest. One that narrows the list is in
/// the accent colour. Gives back where it was drawn.
fn draw_filter_button(buffer: &mut Backbuffer, paint: &Paint, label: &str, narrowed: bool, band: RECT, look: ButtonLook) -> RECT {
    let (theme, fonts, dpi, hc) = (paint.theme, paint.fonts, paint.dpi, paint.hc);
    let height = scale(FILTER_BUTTON_DIP, dpi);
    let pad = scale(12, dpi);
    let chevron = scale(14, dpi);
    let text_width = (text::width(label, &fonts.small) + scale(1, dpi)).min(scale(FILTER_LABEL_MOST_DIP, dpi));
    let width = pad + text_width + scale(6, dpi) + chevron + scale(9, dpi);
    let (right, top) = (band.right, band.top + (band.bottom - band.top - height) / 2);
    let left = right - width;
    let radius = height as f32 / 2.0;
    let fill = if look.pressed { paint.pressed } else if look.hovered || look.open { paint.selection } else { paint.tile };
    buffer.fill_round_rect_alpha(left, top, width, height, radius, fill.0, fill.1);
    let (rim, alpha, thickness) = if look.open || narrowed {
        (theme.accent, if hc { 255 } else { 200 }, scale(2, dpi).max(1) as f32 * 0.75)
    } else if hc {
        (theme.edge, 255, paint.thin as f32)
    } else {
        (theme.overlay, ALPHA_PILL_RIM, paint.thin as f32)
    };
    buffer.stroke_round_rect_alpha(left, top, width, height, radius, thickness, rim, alpha);
    let ink = if narrowed && !hc { theme.accent } else { theme.text };
    text::draw(buffer, label, &fonts.small, ink, RECT { left: left + pad, top, right: left + pad + text_width, bottom: top + height }, Align::Left);
    let mark = RECT { left: right - scale(9, dpi) - chevron, top, right: right - scale(9, dpi), bottom: top + height };
    text::draw(buffer, &GLYPH_DOWN.to_string(), &fonts.icon_keycap, theme.secondary, mark, Align::Center);
    RECT { left, top, right, bottom: top + height }
}

/// A line of text someone can type in: what's there, the selection and the caret. Standard controls
/// can't paint over a see-through window, so the real box stays hidden and handles the typing.
/// A line too long for its box slides left to keep the caret in view, and otherwise stays where it
/// was, `prior`, so that a click in it doesn't jolt it. Gives back how far it's slid.
#[allow(clippy::too_many_arguments)]
fn draw_field(buffer: &mut Backbuffer, paint: &Paint, field: &Field, placeholder: &str, clip: RECT, face: &Face, line_dip: i32, caret_on: bool, vim: Option<VimMode>, prior: i32) -> i32 {
    let (theme, dpi, hc) = (paint.theme, paint.dpi, paint.hc);
    let (start, end) = field.selection;
    let caret_units = text::width(&prefix(&field.text, end), face);
    let (room, margin) = (clip.right - clip.left, scale(2, dpi));
    let least = (caret_units + margin - room).max(0);
    let slide = if prior > least {
        // No further than shows the caret at the left, or the end of the text at the right.
        prior.min(caret_units.min(text::width(&field.text, face) + margin - room)).max(least)
    } else {
        least
    };
    let origin = clip.left - slide;
    let line_height = scale(line_dip + 6, dpi);
    let line_top = clip.top + (clip.bottom - clip.top - line_height) / 2;
    if start != end {
        let left = (origin + text::width(&prefix(&field.text, start), face)).max(clip.left);
        let right = (origin + caret_units).min(clip.right);
        buffer.fill_round_rect_alpha(left, line_top, right - left, line_height, scale(4, dpi) as f32, theme.accent, if hc { 255 } else { 96 });
    }
    if field.text.is_empty() {
        text::draw(buffer, placeholder, face, theme.tertiary, clip, Align::Left);
    } else {
        text::draw_clipped(buffer, &field.text, face, theme.text, origin, clip);
    }
    if caret_on && start == end {
        let x = origin + caret_units - if field.text.is_empty() { scale(3, dpi) } else { 0 };
        if vim == Some(VimMode::Normal) {
            // Vim's block over the letter, or a letter's width past the end.
            let letter = field.text.encode_utf16().skip(end).take(1).collect::<Vec<u16>>();
            let block = if letter.is_empty() { 0 } else { text::width(&String::from_utf16_lossy(&letter), face) };
            buffer.fill_rect_alpha(x, line_top, block.max(scale(9, dpi)), line_height, theme.accent, if hc { 255 } else { 150 });
        } else {
            buffer.fill_round_rect(x + paint.thin, line_top, 2.max(scale(2, dpi)), line_height, 1.0, theme.text);
        }
    }
    slide
}

/// A quicklink's name, then a rounded box for each of its blanks. The one being typed in has the
/// accent around it and the caret, and an empty one shows what goes in it.
#[allow(clippy::too_many_arguments)]
fn draw_link_args(buffer: &mut Backbuffer, paint: &Paint, name: &str, args: &crate::app::LinkArgs, field: &Field, band: RECT, caret_on: bool, vim: Option<VimMode>, boxes: &mut Vec<RECT>, spots: &mut Spots) {
    let (theme, fonts, dpi) = (paint.theme, paint.fonts, paint.dpi);
    boxes.clear();
    let name_width = (text::width(name, &fonts.query) + scale(2, dpi)).min((band.right - band.left) / 3);
    text::draw(buffer, name, &fonts.query, theme.text, RECT { right: band.left + name_width, ..band }, Align::Left);
    let height = scale(ARG_BOX_DIP, dpi);
    let top = band.top + (band.bottom - band.top - height) / 2;
    let pad = scale(12, dpi);
    let mut x = band.left + name_width + scale(14, dpi);
    for (index, param) in args.params.iter().enumerate() {
        let focused = index == args.at;
        let value = if focused { field.text.as_str() } else { args.values.get(index).map_or("", String::as_str) };
        let hint = match &param.default {
            Some(default) if !default.trim().is_empty() => default.clone(),
            Some(_) => format!("{} (optional)", param.label),
            None => param.label.clone(),
        };
        let shown = if value.is_empty() { hint.as_str() } else { value };
        let width = (text::width(shown, &fonts.title).max(scale(36, dpi)) + pad * 2 + scale(4, dpi)).min(scale(ARG_BOX_MAX_DIP, dpi)).min(band.right - x);
        if width < pad * 3 {
            break;
        }
        let radius = height as f32 / 2.0;
        let fill = if focused { paint.selection } else { paint.tile };
        buffer.fill_round_rect_alpha(x, top, width, height, radius, fill.0, fill.1);
        if focused {
            buffer.stroke_round_rect_alpha(x, top, width, height, radius, scale(2, dpi).max(1) as f32 * 0.75, theme.accent, if paint.hc { 255 } else { 220 });
        } else {
            let rim = if paint.hc { (theme.edge, 255) } else { (theme.overlay, ALPHA_PILL_RIM) };
            buffer.stroke_round_rect_alpha(x, top, width, height, radius, paint.thin as f32, rim.0, rim.1);
        }
        let inner = RECT { left: x + pad, top, right: x + width - pad, bottom: top + height };
        let whole = RECT { left: x, top, right: x + width, bottom: top + height };
        if focused {
            let shift = draw_field(buffer, paint, field, &hint, inner, &fonts.title, 14, caret_on, vim, spots.shift(SpotOf::LinkArg(index)));
            spots.add(SpotOf::LinkArg(index), whole, inner, value, SpotFace::Title, false, shift);
        } else {
            let ink = if value.is_empty() { theme.tertiary } else { theme.text };
            text::draw(buffer, shown, &fonts.title, ink, inner, Align::Left);
            spots.add(SpotOf::LinkArg(index), whole, inner, value, SpotFace::Title, false, 0);
        }
        boxes.push(whole);
        x += width + scale(8, dpi);
    }
}

/// A form's fields, one under another: a label on the left and a box to type in on the right. The
/// box being typed in has the accent around it, and one with a problem the danger colour. Under
/// them is what the item will do, or what's wrong. Returns where the boxes and the snippet's
/// placeholder chips were drawn, for clicks.
#[allow(clippy::too_many_arguments)]
fn draw_form(buffer: &mut Backbuffer, paint: &Paint, form: &InlineForm, field: &Field, area: &Field, width: i32, height: i32, caret_on: bool, icons: &crate::icons::IconCache, spots: &mut Spots) -> (Vec<RECT>, Vec<RECT>) {
    let (theme, fonts, dpi, hc) = (paint.theme, paint.fonts, paint.dpi, paint.hc);
    let top = scale(INPUT_HEIGHT + 10, dpi);
    let bottom = height - scale(FOOTER_HEIGHT + 8, dpi);
    let label_left = scale(20, dpi);
    let label_right = scale(FORM_LABEL_DIP, dpi);
    let box_left = label_right + scale(16, dpi);
    let box_right = width - scale(28, dpi);
    let line = scale(FORM_BOX_DIP, dpi);
    let gap = scale(12, dpi);
    let chip_height = scale(26, dpi);
    let note_height = scale(36, dpi);
    let pad = scale(12, dpi);
    let radius = scale(10, dpi) as f32;
    let snippet = !form.is_link();
    let singles = form.fields.iter().filter(|f| !f.multiline).count() as i32;
    let chips_room = if snippet { chip_height + gap } else { 0 };
    let tall = (bottom - top - note_height - chips_room - singles * (line + gap) - gap).max(line * 2);
    let (mut boxes, mut chips) = (Vec::with_capacity(form.fields.len()), Vec::new());
    // The divider the list would start under.
    buffer.fill_rect_alpha(0, scale(INPUT_HEIGHT, dpi) - paint.thin, width, paint.thin, paint.divider.0, paint.divider.1);
    let mut y = top;
    for (index, item) in form.fields.iter().enumerate() {
        let h = if item.multiline { tall } else { line };
        let focused = index == form.at;
        let wrong = form.problem.as_ref().is_some_and(|(at, _)| *at == index);
        let label = RECT { left: label_left, top: y, right: label_right, bottom: y + line };
        let label_ink = if wrong { theme.danger } else if focused { theme.text } else { theme.secondary };
        text::draw(buffer, item.label, &fonts.small, label_ink, label, Align::Right);
        let fill = if focused { paint.selection } else { paint.tile };
        buffer.fill_round_rect_alpha(box_left, y, box_right - box_left, h, radius, fill.0, fill.1);
        let strong = scale(2, dpi).max(1) as f32 * 0.75;
        let (rim, alpha, thickness) = if wrong {
            (theme.danger, if hc { 255 } else { 230 }, strong)
        } else if focused {
            (theme.accent, if hc { 255 } else { 220 }, strong)
        } else if hc {
            (theme.edge, 255, paint.thin as f32)
        } else {
            (theme.overlay, ALPHA_PILL_RIM, paint.thin as f32)
        };
        buffer.stroke_round_rect_alpha(box_left, y, box_right - box_left, h, radius, thickness, rim, alpha);
        let whole = RECT { left: box_left, top: y, right: box_right, bottom: y + h };
        if item.multiline {
            let inner = RECT { left: box_left + pad, top: y + scale(9, dpi), right: box_right - pad, bottom: y + h - scale(9, dpi) };
            let (text, selection) = if focused { (area.text.as_str(), area.selection) } else { (item.value.as_str(), (0, 0)) };
            if text.is_empty() {
                text::draw_wrapped(buffer, item.hint, &fonts.body, theme.tertiary, inner);
            }
            if focused {
                let look = text::EditLook {
                    selection: theme.accent,
                    selection_alpha: if hc { 255 } else { 96 },
                    caret: caret_on.then_some(theme.text),
                    caret_width: 2.max(scale(2, dpi)),
                };
                let scroll = text::draw_editable(buffer, text, &fonts.body, theme.text, inner, selection, look, spots.shift(SpotOf::Field(index)));
                spots.add(SpotOf::Field(index), whole, inner, text, SpotFace::Body, true, scroll);
            } else {
                if !text.is_empty() {
                    text::draw_wrapped(buffer, text, &fonts.body, theme.text, inner);
                }
                // Its box counts a line break as two, as Windows' own boxes do.
                spots.add(SpotOf::Field(index), whole, inner, &text.replace('\n', "\r\n"), SpotFace::Body, true, 0);
            }
        } else if item.choice {
            // Picked from a list: the app's icon and name, and a chevron saying there's a list.
            let inner = RECT { left: box_left + pad, top: y, right: box_right - pad, bottom: y + h };
            let mut text_left = inner.left;
            let size = scale(POPOVER_ICON_DIP, dpi);
            if !item.value.is_empty() {
                if let Some(pixels) = icons.peek(crate::indexer::icon_hash(&item.value), size as usize) {
                    buffer.blit_premultiplied(inner.left, y + (h - size) / 2, size, pixels);
                    text_left += size + scale(8, dpi);
                }
            }
            let (shown, ink) = if item.shown.is_empty() { (item.hint, theme.tertiary) } else { (item.shown.as_str(), theme.text) };
            let chevron = scale(16, dpi);
            text::draw(buffer, shown, &fonts.body, ink, RECT { left: text_left, right: inner.right - chevron - scale(6, dpi), ..inner }, Align::Left);
            let rect = RECT { left: inner.right - chevron, right: inner.right, ..inner };
            text::draw(buffer, &GLYPH_DOWN.to_string(), &fonts.icon_keycap, if focused { theme.text } else { theme.secondary }, rect, Align::Center);
        } else {
            let inner = RECT { left: box_left + pad, top: y, right: box_right - pad, bottom: y + h };
            if focused {
                let shift = draw_field(buffer, paint, field, item.hint, inner, &fonts.body, 14, caret_on, None, spots.shift(SpotOf::Field(index)));
                spots.add(SpotOf::Field(index), whole, inner, &field.text, SpotFace::Body, false, shift);
            } else {
                let (shown, ink) = if item.value.is_empty() { (item.hint, theme.tertiary) } else { (item.value.as_str(), theme.text) };
                text::draw(buffer, shown, &fonts.body, ink, inner, Align::Left);
                spots.add(SpotOf::Field(index), whole, inner, &item.value, SpotFace::Body, false, 0);
            }
        }
        boxes.push(whole);
        y += h + gap;
        if item.multiline && snippet {
            // What a snippet can fill in for itself, put in at the caret with a click.
            let label = RECT { left: label_left, top: y, right: label_right, bottom: y + chip_height };
            text::draw(buffer, "Insert", &fonts.hint, theme.tertiary, label, Align::Right);
            let mut x = box_left;
            for (name, _) in crate::inline_form::PLACEHOLDERS {
                let chip_width = text::width(name, &fonts.hint) + scale(22, dpi);
                buffer.fill_round_rect_alpha(x, y, chip_width, chip_height, chip_height as f32 / 2.0, paint.pill.0, paint.pill.1);
                let rim = if hc { (theme.edge, 255) } else { (theme.overlay, ALPHA_PILL_RIM) };
                buffer.stroke_round_rect_alpha(x, y, chip_width, chip_height, chip_height as f32 / 2.0, paint.thin as f32, rim.0, rim.1);
                let rect = RECT { left: x, top: y, right: x + chip_width, bottom: y + chip_height };
                text::draw(buffer, name, &fonts.hint, theme.secondary, rect, Align::Center);
                chips.push(rect);
                x += chip_width + scale(6, dpi);
            }
            y += chip_height + gap;
        }
    }
    let note = RECT { left: box_left + scale(2, dpi), top: y - scale(2, dpi), right: box_right, bottom };
    let ink = if form.problem.is_some() { theme.danger } else { theme.tertiary };
    text::draw_wrapped(buffer, &form.note(), &fonts.small, ink, note);
    (boxes, chips)
}

/// The rows, across `width`. A `compact` list, beside the clipboard's preview, shows titles only.
/// `reach` is how far down rows are drawn. Past the rows that can be chosen, which end above the
/// footer, they carry on under it, and the caller fades them out (see [`fade_under_footer`]).
#[allow(clippy::too_many_arguments)]
fn draw_list(buffer: &mut Backbuffer, paint: &Paint, list: &ListState, width: i32, visible: i32, reach: i32, icons: &crate::icons::IconCache, compact: bool) {
    let (theme, fonts, dpi, hc) = (paint.theme, paint.fonts, paint.dpi, paint.hc);
    let list_top = scale(INPUT_HEIGHT + PADDING, dpi);
    let row_height = scale(ROW_HEIGHT, dpi);
    let row_x = scale(ROW_INSET, dpi);
    let row_width = width - 2 * row_x;
    let text_x = scale(TEXT_LEFT, dpi);
    let right_edge = row_x + row_width - scale(14, dpi);
    let row_radius = scale(ROW_RADIUS, dpi) as f32;
    let icon_size = scale(ICON_DIP, dpi);
    let icon_x = scale(ICON_LEFT, dpi);

    if list.rows.is_empty() {
        let rect = RECT { left: row_x, top: list_top, right: row_x + row_width, bottom: list_top + row_height };
        if let Some(message) = list.scope.empty_message() {
            text::draw(buffer, message, &fonts.small, theme.secondary, rect, Align::Center);
        }
    }

    // Commands get a coloured tile in the main lists. A menu of actions or a page of information
    // reads better as plain symbols.
    let tiles = !hc && !matches!(list.scope, Scope::Actions { .. } | Scope::Confirm(_) | Scope::Help | Scope::Diagnostics);
    let can_mark = list.scope.can_filter() && !list.query.trim().is_empty();

    // The highlight goes down first, wherever it's got to on its way to the selected row.
    if let Some(rect) = list.highlight {
        let (h, x) = (rect.bottom - rect.top, rect.left);
        buffer.fill_round_rect_alpha(x, rect.top, rect.right - x, h, row_radius, paint.selection.0, paint.selection.1);
        if hc {
            // Marks the selection without relying on telling two colours apart.
            let bar = scale(14, dpi);
            buffer.fill_round_rect(x + scale(3, dpi), rect.top + (h - bar) / 2, scale(3, dpi), bar, scale(2, dpi) as f32, theme.accent);
        }
    }

    for (index, y, row_h) in rows_down_to(list.rows, list.scroll, dpi, reach) {
        let row = &list.rows[index];
        if row.is_header() {
            let top = if index == 0 { y - scale(FIRST_HEADER_LIFT, dpi) } else { y + scale(HEADER_SPACE, dpi) };
            let rect = RECT { left: icon_x + scale(2, dpi), top, right: right_edge, bottom: y + row_h };
            text::draw(buffer, &row.title, &fonts.section, theme.tertiary, rect, Align::Left);
            continue;
        }
        if let (true, RowSource::Answer { card, .. }) = (index == 0 && is_answer_card(list.rows), &row.source) {
            draw_answer_card(buffer, paint, card, RECT { left: row_x, top: y, right: row_x + row_width, bottom: y + row_h });
            continue;
        }
        let selected = index == list.selected;
        if selected && list.highlight.is_none() {
            buffer.fill_round_rect_alpha(row_x, y, row_width, row_h, row_radius, paint.selection.0, paint.selection.1);
            if hc {
                // Marks the selection without relying on telling two colours apart.
                let bar = scale(14, dpi);
                buffer.fill_round_rect(row_x + scale(3, dpi), y + (row_h - bar) / 2, scale(3, dpi), bar, scale(2, dpi) as f32, theme.accent);
            }
        }
        let title_color = if selected { theme.selection_text } else { theme.text };
        let icon_y = y + (row_h - icon_size) / 2;
        // A website icon that isn't there, or isn't yet, is drawn as any link is.
        let icon = match row.icon {
            RowIcon::Favicon(key) if icons.peek(crate::favicon::row_hash(key), icon_size as usize).is_none() => RowIcon::Glyph(GLYPH_LINK),
            // A file's own icon that isn't there yet is drawn as its symbol.
            RowIcon::File(hash, glyph) if icons.peek(hash, icon_size as usize).is_none() => RowIcon::Glyph(glyph),
            other => other,
        };
        // Thumbnails are wider than icons, so everything in that list shares their column.
        let thumb_extra = if *list.scope == Scope::Clipboard { scale(THUMB_DIP, dpi) - icon_size } else { 0 };
        let text_x = text_x + thumb_extra;
        let glyph_x = icon_x + thumb_extra / 2;
        match icon {
            RowIcon::Favicon(key) => {
                if let Some(pixels) = icons.peek(crate::favicon::row_hash(key), icon_size as usize) {
                    buffer.blit_premultiplied(glyph_x, icon_y, icon_size, pixels);
                }
            }
            RowIcon::Image(clip) => {
                let size = scale(THUMB_DIP, dpi);
                let top = y + (row_h - size) / 2;
                if let Some(pixels) = icons.peek(thumb_hash(clip), size as usize) {
                    buffer.blit_premultiplied(icon_x, top, size, pixels);
                } else {
                    let fill = if selected { paint.tile_on_selection } else { paint.tile };
                    buffer.fill_round_rect_alpha(icon_x, top, size, size, scale(TILE_RADIUS, dpi) as f32, fill.0, fill.1);
                    let rect = RECT { left: icon_x, top, right: icon_x + size, bottom: top + size };
                    text::draw(buffer, "\u{EB9F}", &fonts.icon_row, theme.secondary, rect, Align::Center);
                }
            }
            RowIcon::App(hash) => {
                if let Some(pixels) = icons.peek(hash, icon_size as usize) {
                    buffer.blit_premultiplied(icon_x, icon_y, icon_size, pixels);
                }
            }
            RowIcon::File(hash, _) => {
                if let Some(pixels) = icons.peek(hash, icon_size as usize) {
                    buffer.blit_premultiplied(glyph_x, icon_y, icon_size, pixels);
                }
            }
            RowIcon::Glyph(glyph) => {
                let rect = RECT { left: glyph_x, top: icon_y, right: glyph_x + icon_size, bottom: icon_y + icon_size };
                if tiles {
                    draw_tile(buffer, paint, glyph, glyph_x, icon_y, icon_size, selected);
                } else {
                    let color = if row.accent { theme.accent } else if selected { theme.text } else { theme.secondary };
                    text::draw(buffer, &glyph.to_string(), &fonts.icon_row, color, rect, Align::Center);
                }
            }
            RowIcon::Tinted(glyph, tone) => {
                let colours = crate::rows::tone_colors(tone);
                if tiles {
                    draw_tile_in(buffer, paint, glyph, colours, true, glyph_x, icon_y, icon_size, selected);
                } else {
                    let rect = RECT { left: glyph_x, top: icon_y, right: glyph_x + icon_size, bottom: icon_y + icon_size };
                    let color = if !hc { crate::rows::coloured_symbol(colours, paint.dark) } else if selected { theme.text } else { theme.secondary };
                    text::draw(buffer, &glyph.to_string(), &fonts.icon_row, color, rect, Align::Center);
                }
            }
            // Only ever drawn in a grid.
            RowIcon::Emoji(_) => {}
        }

        // An extension's meters take the place of the words on the right.
        let (meters, tags) = match &row.source {
            RowSource::Extension { meters, tags, .. } if !compact => (meters.as_slice(), tags.as_slice()),
            _ => (&[][..], &[][..]),
        };
        let meters_width = if meters.is_empty() { 0 } else { draw_meters(buffer, paint, meters, right_edge, y, row_h, selected) };
        let kind_text: &str = if meters.is_empty() { row.accessory.as_deref().unwrap_or(row.kind) } else { "" };
        let kind_width = if compact || kind_text.is_empty() { 0 } else { text::width(kind_text, &fonts.hint) };
        let used_right = kind_width.max(meters_width);
        let mut text_right = right_edge - used_right - if used_right > 0 { scale(12, dpi) } else { 0 };
        if !tags.is_empty() {
            // Its tags sit just before the time, leaving the title most of the row.
            let width = draw_tags(buffer, paint, tags, text_right, text_x + scale(TAGS_LEAVE_DIP, dpi), y, row_h, selected);
            if width > 0 {
                text_right -= width + scale(10, dpi);
            }
        }
        if let Some(keys) = &row.keys {
            let width = caps_width(keys, fonts, dpi);
            let fill = if selected { paint.keycap_on_selection } else { paint.keycap };
            draw_caps(buffer, keys, text_right - width, y + row_h / 2, fonts, fill, paint.cap_rim, theme.secondary, dpi);
            text_right -= width + scale(12, dpi);
        }
        let title_width = text::width(&row.title, &fonts.title);
        let title_limit = if row.subtitle.is_empty() || compact { text_right - text_x } else { (text_right - text_x) * 7 / 10 };
        let title_right = (text_x + title_width + scale(1, dpi)).min(text_x + title_limit);
        let title_rect = RECT { left: text_x, top: y, right: title_right, bottom: y + row_h };
        let found = if can_mark { text::found_spans(&row.title, list.query) } else { Vec::new() };
        if found.is_empty() {
            text::draw(buffer, &row.title, &fonts.title, title_color, title_rect, Align::Left);
        } else {
            // What you typed stands out, by everything else stepping back a little.
            let rest = title_color.mix(theme.secondary, 0.45);
            text::draw_marked(buffer, &row.title, &fonts.title, rest, title_rect, Align::Left, &found, theme.selection_text);
        }
        if !row.subtitle.is_empty() && !compact {
            let subtitle_x = title_right + scale(10, dpi);
            if subtitle_x + scale(24, dpi) < text_right {
                text::draw(buffer, &row.subtitle, &fonts.small, theme.secondary, RECT { left: subtitle_x, top: y, right: text_right, bottom: y + row_h }, Align::Left);
            }
        }
        if kind_width > 0 {
            let ink = if selected { theme.secondary } else { theme.tertiary };
            text::draw(buffer, kind_text, &fonts.hint, ink, RECT { left: right_edge - kind_width, top: y, right: right_edge, bottom: y + row_h }, Align::Right);
        }
    }

    // A small scrollbar, only when there is more than fits.
    let total = list.rows.len();
    if total > visible as usize {
        let track_height = visible * row_height;
        let thumb_height = (track_height as i64 * visible as i64 / total as i64).max(scale(16, dpi) as i64) as i32;
        let thumb_y = list_top + ((track_height - thumb_height) as i64 * list.scroll as i64 / (total - visible as usize) as i64) as i32;
        buffer.fill_round_rect_alpha(width - scale(6, dpi), thumb_y, scale(3, dpi), thumb_height, scale(2, dpi) as f32, paint.pressed.0, paint.pressed.1);
    }
}

/// How much of a row tags leave for its title, at the least.
const TAGS_LEAVE_DIP: i32 = 180;
const TAG_HEIGHT_DIP: i32 = 20;

/// An extension item's tags, such as "Conflicts" or "Approved", each a word or two in a pill of its
/// colour, in a row ending at `right`. Ones that would reach past `leftmost` are left off, the last
/// first. Gives back how wide they are, or 0 if none fit.
#[allow(clippy::too_many_arguments)]
fn draw_tags(buffer: &mut Backbuffer, paint: &Paint, tags: &[orbit_core::extension::Tag], right: i32, leftmost: i32, y: i32, row_h: i32, selected: bool) -> i32 {
    let (theme, dpi, hc) = (paint.theme, paint.dpi, paint.hc);
    let face = &paint.fonts.hint;
    let pad = scale(8, dpi);
    let gap = scale(6, dpi);
    let height = scale(TAG_HEIGHT_DIP, dpi);
    let top = y + (row_h - height) / 2;
    let widths: Vec<i32> = tags.iter().map(|tag| text::width(&tag.text, face) + pad * 2).collect();
    // As many as fit, from the first, which matters most.
    let mut shown = widths.len();
    while shown > 0 && right - (widths[..shown].iter().sum::<i32>() + gap * (shown as i32 - 1)) < leftmost {
        shown -= 1;
    }
    if shown == 0 {
        return 0;
    }
    let total = widths[..shown].iter().sum::<i32>() + gap * (shown as i32 - 1);
    let mut x = right - total;
    for (tag, &width) in tags.iter().zip(&widths).take(shown) {
        let radius = height as f32 / 2.0;
        let rect = RECT { left: x, top, right: x + width, bottom: top + height };
        if hc {
            // Told apart by their words, since colours are Windows' to choose here.
            buffer.stroke_round_rect_alpha(x, top, width, height, radius, paint.thin as f32, theme.text, 255);
            text::draw(buffer, &tag.text, face, if selected { theme.selection_text } else { theme.text }, rect, Align::Center);
        } else {
            let colours = crate::rows::tone_colors(tag.tone);
            let (fill, alpha, ink) = crate::rows::tinted_tile(colours, paint.dark);
            buffer.fill_round_rect_alpha(x, top, width, height, radius, fill, alpha.saturating_add(if selected { 14 } else { 0 }));
            text::draw(buffer, &tag.text, face, ink, rect, Align::Center);
        }
        x += width + gap;
    }
    total
}

/// How wide one meter's bar is, and how wide a long label can stretch it.
const METER_DIP: i32 = 64;
const METER_MOST_DIP: i32 = 112;

/// An extension item's meters, such as how much of a limit is used, each a label over a bar, in a
/// row ending at `right`. Gives back how wide they are.
fn draw_meters(buffer: &mut Backbuffer, paint: &Paint, meters: &[orbit_core::extension::Meter], right: i32, y: i32, row_h: i32, selected: bool) -> i32 {
    let dpi = paint.dpi;
    let theme = &paint.theme;
    let gap = scale(14, dpi);
    let bar_h = scale(4, dpi).max(3);
    let middle = y + row_h / 2;
    let mut x = right;
    for meter in meters.iter().rev() {
        // Each bar is as wide as its label, so "Sonnet 12%" reads whole.
        let label_width = text::width(&meter.label, &paint.fonts.hint) + scale(2, dpi);
        let width = label_width.clamp(scale(METER_DIP, dpi), scale(METER_MOST_DIP, dpi));
        let left = x - width;
        let ink = if selected { theme.text } else { theme.secondary };
        let label = RECT { left, top: middle - scale(17, dpi), right: x, bottom: middle + scale(1, dpi) };
        text::draw(buffer, &meter.label, &paint.fonts.hint, ink, label, Align::Left);
        let bar_y = middle + scale(4, dpi);
        let track = if selected { paint.keycap_on_selection } else { paint.keycap };
        buffer.fill_round_rect_alpha(left, bar_y, width, bar_h, bar_h as f32 / 2.0, track.0, track.1);
        let filled = (width as f32 * meter.value).round() as i32;
        if filled > 0 {
            buffer.fill_round_rect_alpha(left, bar_y, filled.max(bar_h), bar_h, bar_h as f32 / 2.0, meter_color(paint, meter.value), 255);
        }
        x = left - gap;
    }
    right - (x + gap)
}

/// A meter's colour: the accent while there's room, amber when it's getting full, red near the end.
fn meter_color(paint: &Paint, value: f32) -> Rgb {
    if paint.hc {
        paint.theme.text
    } else if value >= 0.9 {
        paint.theme.danger
    } else if value >= 0.7 {
        if paint.dark { Rgb(245, 176, 66) } else { Rgb(206, 122, 6) }
    } else {
        paint.theme.accent
    }
}

/// An emoji at a size, worked out the first time it's drawn and kept with the weather.
fn weather_icon(buffer: &mut Backbuffer, icons: &mut Vec<(&'static str, i32, Vec<u8>)>, glyph: &'static str, x: i32, y: i32, size: i32) {
    if !icons.iter().any(|(kept, at, _)| *kept == glyph && *at == size) {
        let Some(pixels) = text::emoji_pixels(glyph, size) else { return };
        icons.push((glyph, size, pixels));
    }
    if let Some((_, _, pixels)) = icons.iter().find(|(kept, at, _)| *kept == glyph && *at == size) {
        buffer.blit_premultiplied(x, y, size, pixels);
    }
}

/// Part of a ring, `from` and `sweep` in turns clockwise from twelve o'clock, shading from one
/// colour to another along it, with rounded ends.
#[allow(clippy::too_many_arguments)]
fn draw_arc(buffer: &mut Backbuffer, center: (f32, f32), radius: f32, thick: f32, from: f32, sweep: f32, colours: (Rgb, Rgb), alpha: u8) {
    if sweep <= 0.0 {
        return;
    }
    let point = |turn: f32, r: f32| {
        let angle = (turn * 360.0 - 90.0).to_radians();
        (center.0 + r * angle.cos(), center.1 + r * angle.sin())
    };
    let (outer, inner) = (radius + thick / 2.0, radius - thick / 2.0);
    if alpha < 255 || colours.0 == colours.1 {
        // One shape in one colour, so a see-through ring doesn't darken where pieces would meet.
        let steps = ((sweep * 160.0).ceil() as usize).max(2);
        let mut shape: Vec<(f32, f32)> = (0..=steps).map(|step| point(from + sweep * step as f32 / steps as f32, outer)).collect();
        shape.extend((0..=steps).rev().map(|step| point(from + sweep * step as f32 / steps as f32, inner)));
        buffer.fill_polygon_alpha(&shape, colours.0.mix(colours.1, 0.5), alpha);
        return;
    }
    let pieces = ((sweep * 96.0).ceil() as usize).max(1);
    for piece in 0..pieces {
        // Each piece overlaps the next a little, so no seam shows between them.
        let start = from + sweep * piece as f32 / pieces as f32;
        let end = (from + sweep * (piece + 1) as f32 / pieces as f32 + 0.002).min(from + sweep);
        let colour = colours.0.mix(colours.1, (piece as f32 + 0.5) / pieces as f32);
        let mut shape = Vec::with_capacity(8);
        for step in 0..=2 {
            shape.push(point(start + (end - start) * step as f32 / 2.0, outer));
        }
        for step in (0..=2).rev() {
            shape.push(point(start + (end - start) * step as f32 / 2.0, inner));
        }
        buffer.fill_polygon_alpha(&shape, colour, alpha);
    }
    if sweep < 1.0 {
        let cap = thick.round() as i32;
        for (turn, colour) in [(from, colours.0), (from + sweep, colours.1)] {
            let (x, y) = point(turn, radius);
            buffer.fill_round_rect_alpha((x - thick / 2.0).round() as i32, (y - thick / 2.0).round() as i32, cap, cap, thick / 2.0, colour, alpha);
        }
    }
}

/// Text with every digit the same width, so a running clock doesn't shuffle from side to side.
fn draw_steady(buffer: &mut Backbuffer, text: &str, face: &Face, ink: Rgb, middle: i32, top: i32, bottom: i32) {
    let digit = text::width("0", face);
    let width_of = |c: char| if c.is_ascii_digit() { digit } else { text::width(&c.to_string(), face) };
    let total: i32 = text.chars().map(width_of).sum();
    let mut x = middle - total / 2;
    for c in text.chars() {
        let w = width_of(c);
        text::draw(buffer, &c.to_string(), face, ink, RECT { left: x, top, right: x + w, bottom }, Align::Center);
        x += w;
    }
}

/// The timer or the stopwatch, where the list would be. The timer's ring empties as it runs down;
/// the stopwatch's goes round once a minute. The time sits large in the middle.
#[allow(clippy::too_many_arguments)]
fn draw_clock(buffer: &mut Backbuffer, paint: &Paint, countdown: Option<&crate::clock::Countdown>, watch: &crate::clock::Stopwatch, stopwatch: bool, query: &str, width: i32, height: i32) {
    use orbit_core::timer::{clock, describe, parse_duration};
    let (theme, fonts, dpi) = (paint.theme, paint.fonts, paint.dpi);
    let px = |dip: i32| scale(dip, dpi);
    let top = px(INPUT_HEIGHT + PADDING);
    let bottom = height - px(FOOTER_HEIGHT + PADDING);
    let room = bottom - top;
    let diameter = (room - px(36)).min(px(248));
    let middle = (width / 2, top + room / 2);
    let (radius, thick) = (diameter as f32 / 2.0, px(12) as f32);
    let colours = tile_colors(if stopwatch { GLYPH_STOPWATCH } else { GLYPH_TIMER });

    let (seconds, filled, caption, waiting) = if stopwatch {
        let elapsed = watch.elapsed();
        let caption = if watch.running() { "Stopwatch" } else if watch.is_reset() { "Press Enter to start" } else { "Paused" };
        (elapsed.as_secs(), (elapsed.as_secs_f32() % 60.0) / 60.0, caption.to_string(), watch.is_reset())
    } else if let Some(timer) = countdown {
        let caption = if timer.done { "Done".to_string() } else if timer.running() { describe(timer.length) } else { "Paused".to_string() };
        (timer.shown_seconds(), timer.fraction_left(), caption, timer.done)
    } else {
        let typed = parse_duration(query);
        (typed.unwrap_or(0), if typed.is_some() { 1.0 } else { 0.0 }, "Type how long above".to_string(), true)
    };

    // The track, then how much is left (or, for the stopwatch, how far round this minute it is).
    let centre = (middle.0 as f32, middle.1 as f32);
    draw_arc(buffer, centre, radius, thick, 0.0, 1.0, (theme.text, theme.text), if paint.dark { 26 } else { 22 });
    let (start, sweep) = if stopwatch { (0.0, filled) } else { (1.0 - filled, filled) };
    let alpha = if waiting { 110 } else { 255 };
    draw_arc(buffer, centre, radius, thick, start, sweep, (colours.0, colours.1), alpha);

    let ink = if waiting { theme.tertiary } else { theme.text };
    let clock_h = px(66);
    let clock_top = middle.1 - clock_h / 2 - px(10);
    draw_steady(buffer, &clock(seconds), &fonts.clock, ink, middle.0, clock_top, clock_top + clock_h);
    let caption_top = clock_top + clock_h + px(2);
    let lane = RECT { left: middle.0 - diameter / 2 + px(20), top: caption_top, right: middle.0 + diameter / 2 - px(20), bottom: caption_top + px(20) };
    text::draw(buffer, &caption, &fonts.small, theme.secondary, lane, Align::Center);
}

/// A colour picked from the screen: a large swatch with its hex on it, and the other ways to write
/// it underneath.
fn draw_colour(buffer: &mut Backbuffer, paint: &Paint, colour: orbit_core::colour::Colour, width: i32, height: i32) {
    let (theme, fonts, dpi) = (paint.theme, paint.fonts, paint.dpi);
    let px = |dip: i32| scale(dip, dpi);
    let top = px(INPUT_HEIGHT + PADDING) + px(6);
    let (left, right) = (px(ROW_INSET + 8), width - px(ROW_INSET + 8));
    let _ = height;
    let swatch = Rgb(colour.r, colour.g, colour.b);
    let swatch_h = px(176);
    let radius = px(16) as f32;
    buffer.shadow_round_rect(left, top + px(2), right - left, swatch_h, radius, px(10) as f32, Rgb(0, 0, 0), if paint.dark { 90 } else { 40 });
    buffer.fill_round_rect(left, top, right - left, swatch_h, radius, swatch);
    buffer.stroke_round_rect_alpha(left, top, right - left, swatch_h, radius, paint.thin as f32, theme.text, 30);
    let on = if colour.is_light() { Rgb(20, 20, 24) } else { Rgb(255, 255, 255) };
    let label = RECT { left: left + px(24), top: top + swatch_h - px(64), right: right - px(24), bottom: top + swatch_h - px(20) };
    text::draw(buffer, &colour.hex(), &fonts.answer, on, label, Align::Left);

    // HEX, RGB and HSL, one to a line.
    let line_h = px(40);
    let mut y = top + swatch_h + px(14);
    let lines = [("HEX", colour.hex(), "Enter"), ("RGB", colour.rgb(), "Ctrl+Enter"), ("HSL", colour.hsl(), "Ctrl+Shift+Enter")];
    for (at, (name, value, keys)) in lines.iter().enumerate() {
        if at > 0 {
            buffer.fill_rect_alpha(left + px(12), y, right - left - px(24), paint.thin, paint.divider.0, paint.divider.1);
        }
        let row = RECT { left: left + px(12), top: y, right: right - px(12), bottom: y + line_h };
        text::draw(buffer, name, &fonts.hint_strong, theme.secondary, RECT { right: row.left + px(48), ..row }, Align::Left);
        text::draw(buffer, value, &fonts.body, theme.text, RECT { left: row.left + px(56), ..row }, Align::Left);
        let caps = caps_width(keys, fonts, dpi);
        draw_caps(buffer, keys, row.right - caps, y + line_h / 2, fonts, paint.keycap, paint.cap_rim, theme.secondary, dpi);
        y += line_h;
    }
}

/// The colour of a temperature: blue when it's freezing, through green, to amber when it's hot.
fn temperature_color(value: f64, fahrenheit: bool) -> Rgb {
    let celsius = if fahrenheit { (value - 32.0) * 5.0 / 9.0 } else { value };
    let stops = [(-5.0, Rgb(96, 165, 250)), (8.0, Rgb(45, 212, 191)), (18.0, Rgb(163, 230, 53)), (28.0, Rgb(251, 191, 36)), (36.0, Rgb(248, 113, 113))];
    let (first, last) = (stops[0], stops[stops.len() - 1]);
    if celsius <= first.0 {
        return first.1;
    }
    for pair in stops.windows(2) {
        let ((low, from), (high, to)) = (pair[0], pair[1]);
        if celsius <= high {
            return from.mix(to, ((celsius - low) / (high - low)) as f32);
        }
    }
    last.1
}

/// The weather, where the list would be: now, the next 24 hours, and the week.
fn draw_weather(buffer: &mut Backbuffer, paint: &Paint, weather: &mut crate::weather::Weather, width: i32, height: i32) {
    use orbit_core::weather::{degrees, describe, emoji};
    let (theme, fonts, dpi) = (paint.theme, paint.fonts, paint.dpi);
    let px = |dip: i32| scale(dip, dpi);
    let top = px(INPUT_HEIGHT + PADDING) + px(4);
    let bottom = height - px(FOOTER_HEIGHT + PADDING);
    let (left, right) = (px(ROW_INSET + 14), width - px(ROW_INSET + 14));
    let line = |y: i32, h: i32| RECT { left, top: y, right, bottom: y + h };

    let Some(forecast) = weather.forecast.clone() else {
        let message = match (&weather.problem, weather.busy) {
            (Some(problem), _) => problem.clone(),
            (None, true) => "Looking up the weather\u{2026}".to_string(),
            (None, false) => "Type a place, then press Enter.".to_string(),
        };
        text::draw(buffer, &message, &fonts.body, theme.secondary, line(top + px(40), px(24)), Align::Center);
        return;
    };
    let place = weather.place.as_ref().map(|place| place.label()).unwrap_or_default();
    let wind_unit = if forecast.fahrenheit { "mph" } else { "km/h" };

    // Now: the picture, the temperature, what it's like, and where.
    let big = px(64);
    weather_icon(buffer, &mut weather.icons, emoji(forecast.now.code, forecast.now.is_day), left - px(4), top + px(4), big);
    let temperature = degrees(forecast.now.temperature);
    let temp_x = left + big + px(10);
    let temp_w = text::width(&temperature, &fonts.display);
    text::draw(buffer, &temperature, &fonts.display, theme.text, RECT { left: temp_x, top, right: temp_x + temp_w + px(4), bottom: top + px(72) }, Align::Left);
    let words_x = temp_x + temp_w + px(18);
    let side_w = px(200);
    let words = RECT { left: words_x, top: top + px(14), right: right - side_w, bottom: top + px(36) };
    text::draw(buffer, describe(forecast.now.code), &fonts.heading, theme.text, words, Align::Left);
    text::draw(buffer, &place, &fonts.small, theme.secondary, RECT { top: top + px(38), bottom: top + px(56), ..words }, Align::Left);
    let side = |y: i32, h: i32| RECT { left: right - side_w, top: y, right, bottom: y + h };
    if let Some(today) = forecast.days.first() {
        text::draw(buffer, &format!("H {}   L {}", degrees(today.high), degrees(today.low)), &fonts.title, theme.text, side(top + px(8), px(20)), Align::Right);
    }
    text::draw(buffer, &format!("Feels like {}", degrees(forecast.now.feels_like)), &fonts.small, theme.secondary, side(top + px(30), px(18)), Align::Right);
    text::draw(buffer, &format!("Wind {} {wind_unit} \u{00B7} Humidity {}%", forecast.now.wind.round(), forecast.now.humidity.round()), &fonts.small, theme.secondary, side(top + px(50), px(18)), Align::Right);
    let status = if weather.busy { Some(("Updating\u{2026}".to_string(), theme.tertiary)) } else { weather.problem.clone().map(|problem| (problem, theme.secondary)) };

    // The next 24 hours, every three, over a faint curve of the temperature through all of them.
    let hours_top = top + px(84);
    text::draw(buffer, "Next 24 hours", &fonts.section, theme.tertiary, line(hours_top, px(16)), Align::Left);
    let strip_top = hours_top + px(20);
    let strip_h = px(74);
    let shown: Vec<&orbit_core::weather::Hour> = forecast.hours.iter().step_by(3).take(8).collect();
    let slot = (right - left) / shown.len().max(1) as i32;
    let (cold, warm) = forecast.hours.iter().fold((f64::MAX, f64::MIN), |(lo, hi), h| (lo.min(h.temperature), hi.max(h.temperature)));
    let span = (warm - cold).max(1.0);
    let curve_top = strip_top + px(22);
    let curve_h = px(26);
    let curve_y = |value: f64| curve_top as f32 + curve_h as f32 * (1.0 - ((value - cold) / span) as f32);
    let hour_x = |at: usize| left as f32 + slot as f32 / 2.0 + (at as f32 / 3.0) * slot as f32;
    // From now to the last hour shown, so the curve ends under it.
    let count = forecast.hours.len().min(shown.len().saturating_sub(1) * 3 + 1);
    if count > 1 && !paint.hc {
        let mut area: Vec<(f32, f32)> = (0..count).map(|at| (hour_x(at), curve_y(forecast.hours[at].temperature))).collect();
        let base = (curve_top + curve_h + px(10)) as f32;
        area.push((hour_x(count - 1), base));
        area.push((hour_x(0), base));
        buffer.fill_polygon_alpha(&area, theme.accent, if paint.dark { 34 } else { 26 });
    }
    for (at, hour) in shown.iter().enumerate() {
        let x = left + slot * at as i32;
        let cell = |y: i32, h: i32| RECT { left: x, top: y, right: x + slot, bottom: y + h };
        let ink = if at == 0 { theme.text } else { theme.secondary };
        text::draw(buffer, &hour.label, &fonts.hint, ink, cell(strip_top, px(16)), Align::Center);
        let icon = px(24);
        weather_icon(buffer, &mut weather.icons, emoji(hour.code, hour.is_day), x + (slot - icon) / 2, strip_top + px(20), icon);
        text::draw(buffer, &degrees(hour.temperature), &fonts.title, theme.text, cell(strip_top + strip_h - px(24), px(20)), Align::Center);
    }

    // The week: each day's range on one scale, so warmer and colder days stand out.
    let days_top = strip_top + strip_h + px(10);
    text::draw(buffer, "7 days", &fonts.section, theme.tertiary, line(days_top, px(16)), Align::Left);
    let rows_top = days_top + px(20);
    let row_h = ((bottom - rows_top) / forecast.days.len().max(1) as i32).clamp(px(18), px(30));
    let (week_low, week_high) = forecast.days.iter().fold((f64::MAX, f64::MIN), |(lo, hi), d| (lo.min(d.low), hi.max(d.high)));
    let week_span = (week_high - week_low).max(1.0);
    let bar_left = left + px(220);
    let bar_right = right - px(54);
    for (at, day) in forecast.days.iter().enumerate() {
        let y = rows_top + row_h * at as i32;
        if y + row_h > bottom + px(PADDING) {
            break;
        }
        let row = |l: i32, r: i32| RECT { left: l, top: y, right: r, bottom: y + row_h };
        text::draw(buffer, &day.label, &fonts.body, theme.text, row(left, left + px(70)), Align::Left);
        let icon = px(20);
        weather_icon(buffer, &mut weather.icons, emoji(day.code, true), left + px(78), y + (row_h - icon) / 2, icon);
        if let Some(rain) = day.rain.filter(|chance| *chance >= 20) {
            text::draw(buffer, &format!("{rain}%"), &fonts.hint, Rgb(96, 165, 250), row(left + px(106), left + px(150)), Align::Left);
        }
        text::draw(buffer, &degrees(day.low), &fonts.body, theme.secondary, row(bar_left - px(52), bar_left - px(12)), Align::Right);
        text::draw(buffer, &degrees(day.high), &fonts.body, theme.text, row(bar_right + px(12), right), Align::Left);
        let track_h = px(5);
        let track_y = y + (row_h - track_h) / 2;
        let radius = track_h as f32 / 2.0;
        buffer.fill_round_rect_alpha(bar_left, track_y, bar_right - bar_left, track_h, radius, paint.divider.0, paint.divider.1.saturating_mul(2));
        let at_x = |value: f64| bar_left + ((value - week_low) / week_span * (bar_right - bar_left) as f64) as i32;
        let (from, to) = (at_x(day.low), at_x(day.high).max(at_x(day.low) + track_h));
        let warmth = temperature_color((day.low + day.high) / 2.0, forecast.fahrenheit);
        let fill = if paint.hc { theme.text } else { warmth };
        buffer.fill_round_rect_alpha(from, track_y, to - from, track_h, radius, fill, 255);
    }
    if let Some((status, ink)) = status {
        text::draw(buffer, &status, &fonts.hint, ink, RECT { left: right - side_w, top: hours_top, right, bottom: hours_top + px(16) }, Align::Right);
    }
}

/// A test's speeds as it went: a soft area under a line, rising to the right as far as it's got.
fn draw_speed_trace(buffer: &mut Backbuffer, paint: &Paint, samples: &[f32], reached: f32, color: Rgb, area: RECT) {
    if samples.is_empty() || reached <= 0.0 {
        return;
    }
    let width = (area.right - area.left) as f32;
    let height = (area.bottom - area.top) as f32;
    let most = samples.iter().copied().fold(1.0f32, f32::max) * 1.15;
    let count = samples.len() as f32;
    let points: Vec<(f32, f32)> = samples
        .iter()
        .enumerate()
        .map(|(at, speed)| {
            let x = area.left as f32 + width * reached.clamp(0.0, 1.0) * (at as f32 + 1.0) / count;
            // Kept inside the chart, whatever the reading, or the fill would run off it.
            (x, area.bottom as f32 - height * (speed / most).clamp(0.0, 1.0))
        })
        .collect();
    let ink = if paint.hc { paint.theme.text } else { color };
    if !paint.hc {
        let mut fill = Vec::with_capacity(points.len() + 3);
        fill.push((area.left as f32, points[0].1));
        fill.extend(points.iter().copied());
        fill.push((points[points.len() - 1].0, area.bottom as f32));
        fill.push((area.left as f32, area.bottom as f32));
        buffer.fill_polygon_alpha(&fill, ink, if paint.dark { 56 } else { 44 });
    }
    // The line along the top, as a thin band.
    let thick = scale(2, paint.dpi) as f32;
    let mut band = Vec::with_capacity(points.len() * 2 + 2);
    band.push((area.left as f32, points[0].1));
    band.extend(points.iter().copied());
    band.extend(points.iter().rev().map(|&(x, y)| (x, y + thick)));
    band.push((area.left as f32, points[0].1 + thick));
    buffer.fill_polygon_alpha(&band, ink, 255);
}

/// The speed test, where the list would be: Ookla's licence the first time, then download and
/// upload as they're measured, then ping and where it was tested.
fn draw_speedtest(buffer: &mut Backbuffer, paint: &Paint, speed: &crate::speedtest::SpeedTest, accepted: bool, width: i32, height: i32) {
    use orbit_core::speedtest::{Phase, format_speed};
    let (theme, fonts, dpi) = (paint.theme, paint.fonts, paint.dpi);
    let px = |dip: i32| scale(dip, dpi);
    let top = px(INPUT_HEIGHT + PADDING) + px(4);
    let bottom = height - px(FOOTER_HEIGHT + PADDING);
    let (left, right) = (px(ROW_INSET + 14), width - px(ROW_INSET + 14));
    let line = |y: i32, h: i32| RECT { left, top: y, right, bottom: y + h };

    if !accepted {
        let y = top + px(14);
        text::draw(buffer, "Before the first test", &fonts.heading, theme.text, line(y, px(24)), Align::Left);
        let how = if speed.missing {
            "Orbit tests your connection with Speedtest CLI by Ookla, against Speedtest.net\u{2019}s servers. It isn\u{2019}t installed yet, so Windows Package Manager installs it first."
        } else {
            "Orbit tests your connection with Speedtest CLI by Ookla, against Speedtest.net\u{2019}s servers."
        };
        text::draw_wrapped(buffer, how, &fonts.body, theme.secondary, RECT { top: y + px(36), bottom: y + px(84), ..line(0, 0) });
        let terms = "Ookla allows it for personal, non-commercial use, under its End User License Agreement, Terms of Use and Privacy Policy. Speedtest.net sees your IP address, and keeps each result with a link to it. Ctrl+K has links to all three.";
        text::draw_wrapped(buffer, terms, &fonts.body, theme.secondary, RECT { top: y + px(82), bottom: y + px(150), ..line(0, 0) });
        text::draw(buffer, "Press Enter to accept Ookla\u{2019}s terms and start.", &fonts.body, theme.text, line(y + px(160), px(20)), Align::Left);
        return;
    }
    let message = if speed.installing {
        Some("Installing Speedtest CLI with Windows Package Manager\u{2026}".to_string())
    } else if speed.missing {
        Some(speed.install_problem.clone().unwrap_or_else(|| "Speedtest CLI isn\u{2019}t installed. Press Enter to install it with Windows Package Manager.".to_string()))
    } else if speed.test.phase == Phase::Idle {
        Some("Press Enter to start the test.".to_string())
    } else {
        None
    };
    if let Some(message) = message {
        text::draw_wrapped(buffer, &message, &fonts.body, theme.secondary, RECT { top: top + px(40), bottom: top + px(90), ..line(0, 0) });
        return;
    }

    let test = &speed.test;
    let gap = px(12);
    let card_w = (right - left - gap) / 2;
    let card_top = top + px(4);
    let card_h = px(226);
    let done_with = |phase: Phase| match phase {
        Phase::Download => matches!(test.phase, Phase::Upload | Phase::Done) || (test.phase == Phase::Failed && !test.download_samples.is_empty()),
        _ => test.phase == Phase::Done || (test.phase == Phase::Failed && !test.upload_samples.is_empty()),
    };
    let cards = [
        ("\u{2193}  Download", test.download, &test.download_samples, Rgb(56, 189, 248), Phase::Download),
        ("\u{2191}  Upload", test.upload, &test.upload_samples, Rgb(167, 139, 250), Phase::Upload),
    ];
    for (at, (label, value, samples, color, phase)) in cards.into_iter().enumerate() {
        let x = left + at as i32 * (card_w + gap);
        buffer.fill_round_rect_alpha(x, card_top, card_w, card_h, px(14) as f32, paint.divider.0, paint.divider.1);
        let active = test.phase == phase;
        let inner = |y: i32, h: i32| RECT { left: x + px(18), top: y, right: x + card_w - px(18), bottom: y + h };
        text::draw(buffer, label, &fonts.section, if active { theme.text } else { theme.tertiary }, inner(card_top + px(16), px(16)), Align::Left);
        let number = value.map(format_speed).unwrap_or_else(|| "\u{2014}".to_string());
        let ink = if value.is_some() { theme.text } else { theme.tertiary };
        let number_w = text::width(&number, &fonts.display);
        let number_x = x + px(16);
        text::draw(buffer, &number, &fonts.display, ink, RECT { left: number_x, top: card_top + px(34), right: number_x + number_w + px(4), bottom: card_top + px(100) }, Align::Left);
        if value.is_some() {
            text::draw(buffer, "Mbps", &fonts.body, theme.secondary, RECT { left: number_x + number_w + px(8), top: card_top + px(66), right: x + card_w, bottom: card_top + px(86) }, Align::Left);
        }
        let reached = if active { test.progress } else if done_with(phase) { 1.0 } else { 0.0 };
        let chart = RECT { left: x + px(14), top: card_top + px(112), right: x + card_w - px(14), bottom: card_top + card_h - px(14) };
        draw_speed_trace(buffer, paint, samples, reached, color, chart);
    }

    // Ping, jitter and packet loss, under the cards.
    let stats_top = card_top + card_h + px(16);
    let millis = |value: Option<f64>| value.map_or("\u{2014}".to_string(), |ms| if ms < 10.0 { format!("{ms:.1} ms") } else { format!("{ms:.0} ms") });
    let loss = match (test.packet_loss, test.phase) {
        (Some(loss), _) if loss == 0.0 => "0%".to_string(),
        (Some(loss), _) => format!("{loss:.1}%"),
        (None, Phase::Done) => "Not measured".to_string(),
        (None, _) => "\u{2014}".to_string(),
    };
    let stat_w = (right - left) / 3;
    for (at, (label, value)) in [("Ping", millis(test.ping)), ("Jitter", millis(test.jitter)), ("Packet loss", loss)].into_iter().enumerate() {
        let x = left + px(4) + stat_w * at as i32;
        let cell = |y: i32, h: i32| RECT { left: x, top: y, right: x + stat_w, bottom: y + h };
        text::draw(buffer, label, &fonts.hint, theme.tertiary, cell(stats_top, px(16)), Align::Left);
        text::draw(buffer, &value, &fonts.title, theme.text, cell(stats_top + px(18), px(22)), Align::Left);
    }

    // Where it was tested, and how far it's got.
    let status_top = bottom - px(18);
    let place = [test.server.as_str(), test.location.as_str(), test.isp.as_str()].into_iter().filter(|part| !part.is_empty()).collect::<Vec<_>>().join(" \u{00B7} ");
    let (status, status_ink) = match test.phase {
        Phase::Starting => ("Finding a server\u{2026}".to_string(), theme.secondary),
        Phase::Ping => ("Testing ping\u{2026}".to_string(), theme.secondary),
        Phase::Download => (format!("Testing download\u{2026} {:.0}%", test.progress * 100.0), theme.secondary),
        Phase::Upload => (format!("Testing upload\u{2026} {:.0}%", test.progress * 100.0), theme.secondary),
        Phase::Done => ("Done".to_string(), theme.tertiary),
        Phase::Failed => (test.problem.clone().unwrap_or_default(), theme.secondary),
        Phase::Idle => (String::new(), theme.tertiary),
    };
    let status_w = text::width(&status, &fonts.small).min((right - left) * 2 / 3);
    text::draw(buffer, &place, &fonts.small, theme.tertiary, RECT { right: right - status_w - px(16), ..line(status_top, px(18)) }, Align::Left);
    text::draw(buffer, &status, &fonts.small, status_ink, RECT { left: right - status_w, ..line(status_top, px(18)) }, Align::Right);
    if test.running() {
        // Ping is quick; download and upload take about as long as each other.
        let overall = match test.phase {
            Phase::Ping => 0.08 * test.progress,
            Phase::Download => 0.08 + 0.46 * test.progress,
            Phase::Upload => 0.54 + 0.46 * test.progress,
            _ => 0.0,
        };
        let track_y = status_top - px(12);
        let track_h = px(3);
        buffer.fill_round_rect_alpha(left, track_y, right - left, track_h, track_h as f32 / 2.0, paint.divider.0, paint.divider.1.saturating_mul(2));
        let filled = ((right - left) as f32 * overall) as i32;
        if filled > 0 {
            buffer.fill_round_rect_alpha(left, track_y, filled.max(track_h), track_h, track_h as f32 / 2.0, if paint.hc { theme.text } else { theme.accent }, 255);
        }
    }
}

/// An answer that's the only result: the question on the left, the answer on the right, each with
/// what it's in under it, an arrow between them, and a note, such as the day of a rate, below.
fn draw_answer_card(buffer: &mut Backbuffer, paint: &Paint, card: &AnswerCard, rect: RECT) {
    let (theme, fonts, dpi) = (paint.theme, paint.fonts, paint.dpi);
    let px = |dip: i32| scale(dip, dpi);
    let (width, height) = (rect.right - rect.left, rect.bottom - rect.top);
    buffer.fill_round_rect_alpha(rect.left, rect.top, width, height, px(14) as f32, paint.selection.0, paint.selection.1);
    if paint.hc {
        let bar = px(28);
        buffer.fill_round_rect(rect.left + px(3), rect.top + (height - bar) / 2, px(3), bar, px(2) as f32, theme.accent);
    }

    // The question and the answer, centred in their halves, and the note along the bottom.
    let note_h = if card.note.is_empty() { 0 } else { px(30) };
    let block = px(40) + px(4) + px(18);
    let block_top = rect.top + (height - note_h - block) / 2 + if note_h > 0 { px(4) } else { 0 };
    let middle = rect.left + width / 2;
    let gap = px(28);
    let sides = [
        (rect.left + px(16), middle - gap, &card.question, &card.question_label, theme.secondary),
        (middle + gap, rect.right - px(16), &card.answer, &card.answer_label, theme.text),
    ];
    for (left, right, value, label, ink) in sides {
        text::draw(buffer, value, &fonts.answer, ink, RECT { left, top: block_top, right, bottom: block_top + px(40) }, Align::Center);
        text::draw(buffer, label, &fonts.small, theme.secondary, RECT { left, top: block_top + px(44), right, bottom: block_top + px(62) }, Align::Center);
    }

    // The arrow between them, in a small circle.
    let size = px(30);
    let (circle_x, circle_y) = (middle - size / 2, block_top + px(20) - size / 2);
    buffer.fill_round_rect_alpha(circle_x, circle_y, size, size, size as f32 / 2.0, paint.keycap_on_selection.0, paint.keycap_on_selection.1);
    text::draw(buffer, "\u{E72A}", &fonts.icon_keycap, theme.text, RECT { left: circle_x, top: circle_y, right: circle_x + size, bottom: circle_y + size }, Align::Center);

    if note_h > 0 {
        let top = rect.bottom - note_h - px(6);
        text::draw(buffer, &card.note, &fonts.hint, theme.tertiary, RECT { left: rect.left + px(16), top, right: rect.right - px(16), bottom: top + note_h }, Align::Center);
    }
}

/// Where the rows running on under the footer fade out: fully there at the end of the rows that
/// can be chosen, and gone by the launcher's bottom edge, so the row past the end shows faintly
/// between the footer's pills.
struct FooterFade {
    from: i32,
    to: i32,
}

impl FooterFade {
    /// `from` is the bottom of the last row that can be chosen. Section titles are shorter than rows, so
    /// that isn't always a whole number of rows down.
    fn new(from: i32, height: i32, more_below: bool) -> Option<FooterFade> {
        let to = height;
        (more_below && to > from).then_some(FooterFade { from, to })
    }
}

/// Beside the clipboard's list: what the selected item holds, and the facts about it.
fn draw_side_panel(buffer: &mut Backbuffer, paint: &Paint, preview: Option<&ClipPreview>, width: i32, height: i32) {
    let (theme, fonts, dpi, thin) = (paint.theme, paint.fonts, paint.dpi, paint.thin);
    let divider = split_x(dpi);
    let list_top = scale(INPUT_HEIGHT + PADDING, dpi);
    let footer_top = height - scale(FOOTER_HEIGHT, dpi);
    buffer.fill_rect_alpha(divider, list_top, thin, footer_top - list_top - scale(PADDING, dpi), paint.divider.0, paint.divider.1);
    // Until the first one arrives, which is a few milliseconds, the panel stays empty.
    let Some(preview) = preview else { return };
    let details = clip_details(preview, crate::util::when_label(preview.item.created_at, crate::app::now_unix()));
    let panel = side_panel(width, height, dpi, details.len());
    let room = panel.preview;
    let (room_w, room_h) = (room.right - room.left, room.bottom - room.top);
    match &preview.body {
        PreviewBody::Image { width: image_w, height: image_h, pixels } => {
            // Scaled for this room already. A smaller room, after a change of scale, crops it.
            let (w, h) = ((*image_w as i32).min(room_w), (*image_h as i32).min(room_h));
            let (x, y) = (room.left + (room_w - w).max(0) / 2, room.top + (room_h - h).max(0) / 2);
            let radius = scale(PREVIEW_RADIUS, dpi).min(w / 2).min(h / 2) as f32;
            buffer.blit_premultiplied_rect(x, y, w, h, radius, &crop(pixels, *image_w, w, h));
            if !paint.hc {
                buffer.stroke_round_rect_alpha(x, y, w, h, radius, thin as f32, theme.overlay, ALPHA_EDGE);
            }
        }
        PreviewBody::Text { shown, .. } => text::draw_wrapped(buffer, shown, &fonts.small, theme.text, room),
        PreviewBody::Files { paths, .. } => {
            let line = scale(FILE_ROW, dpi);
            let icon = scale(ICON_DIP, dpi);
            let fits = (room_h / line).max(1) as usize;
            let more = paths.len().saturating_sub(fits);
            let shown = if more > 0 { fits.saturating_sub(1) } else { paths.len() };
            for (index, path) in paths.iter().take(shown).enumerate() {
                let y = room.top + line * index as i32;
                let place = std::path::Path::new(path);
                let name = place.file_name().map_or_else(|| path.clone(), |name| name.to_string_lossy().to_string());
                let folder = place.parent().map(|folder| folder.to_string_lossy().to_string()).unwrap_or_default();
                let icon_y = y + (line - icon) / 2;
                buffer.fill_round_rect_alpha(room.left, icon_y, icon, icon, scale(TILE_RADIUS, dpi) as f32, paint.tile.0, paint.tile.1);
                let glyph = if place.extension().is_some() { '\u{E8A5}' } else { GLYPH_FOLDER };
                text::draw(buffer, &glyph.to_string(), &fonts.icon_tile, theme.secondary, RECT { left: room.left, top: icon_y, right: room.left + icon, bottom: icon_y + icon }, Align::Center);
                let text_left = room.left + icon + scale(12, dpi);
                let half = line / 2;
                text::draw(buffer, &name, &fonts.title, theme.text, RECT { left: text_left, top: y + scale(3, dpi), right: room.right, bottom: y + half + scale(1, dpi) }, Align::Left);
                text::draw(buffer, &folder, &fonts.hint, theme.secondary, RECT { left: text_left, top: y + half - scale(1, dpi), right: room.right, bottom: y + line - scale(3, dpi) }, Align::Left);
            }
            if more > 0 {
                let y = room.top + line * shown as i32;
                let rest = paths.len() - shown;
                text::draw(buffer, &format!("and {rest} more"), &fonts.small, theme.secondary, RECT { left: room.left, top: y, right: room.right, bottom: y + line }, Align::Left);
            }
        }
        PreviewBody::Nothing => {
            let middle = room.top + room_h / 2;
            let glyph = RECT { left: room.left, top: middle - scale(30, dpi), right: room.right, bottom: middle };
            text::draw(buffer, &GLYPH_IMAGE.to_string(), &fonts.icon_large, theme.tertiary, glyph, Align::Center);
            let words = RECT { left: room.left, top: middle + scale(2, dpi), right: room.right, bottom: middle + scale(26, dpi) };
            text::draw(buffer, "No preview for this picture", &fonts.small, theme.secondary, words, Align::Center);
        }
    }

    let area = panel.details;
    let title = RECT { left: area.left, top: area.top, right: area.right, bottom: area.top + scale(DETAIL_TITLE, dpi) };
    text::draw(buffer, "Information", &fonts.section, theme.tertiary, title, Align::Left);
    let row_height = scale(DETAIL_ROW, dpi);
    let mut y = title.bottom;
    for (label, value) in &details {
        buffer.fill_rect_alpha(area.left, y, area.right - area.left, thin, paint.divider.0, paint.divider.1);
        let row = RECT { left: area.left, top: y, right: area.right, bottom: y + row_height };
        let label_width = text::width(label, &fonts.small);
        text::draw(buffer, label, &fonts.small, theme.secondary, row, Align::Left);
        let value_area = RECT { left: row.left + label_width + scale(16, dpi), ..row };
        text::draw(buffer, value, &fonts.small, theme.text, value_area, Align::Right);
        y += row_height;
    }
}

/// The top left `w` by `h` of a picture `stride` pixels wide, for one bigger than its room.
fn crop(pixels: &[u8], stride: usize, w: i32, h: i32) -> std::borrow::Cow<'_, [u8]> {
    let (w, h) = (w.max(0) as usize, h.max(0) as usize);
    if w == stride {
        return std::borrow::Cow::Borrowed(&pixels[..(w * h * 4).min(pixels.len())]);
    }
    let mut out = Vec::with_capacity(w * h * 4);
    for row in 0..h {
        let start = row * stride * 4;
        if let Some(line) = pixels.get(start..start + w * 4) {
            out.extend_from_slice(line);
        }
    }
    std::borrow::Cow::Owned(out)
}

/// One emoji's tile in the grid, where it's drawn this frame.
struct GridCell {
    index: usize,
    at: u16,
    rect: RECT,
}

/// Where each emoji showing goes, so the highlight can glide to one before any are drawn.
fn grid_cells(list: &ListState, grid: &Grid, width: i32, dpi: u32) -> Vec<GridCell> {
    let list_top = scale(INPUT_HEIGHT + PADDING, dpi);
    let inset = scale(ROW_INSET, dpi);
    let across = width - inset * 2;
    let cell_width = across / COLUMNS as i32;
    let gap = scale(3, dpi);
    let mut cells = Vec::new();
    for &(line, top, height) in &grid.layout(list.scroll) {
        let Line::Cells { start, count } = line else { continue };
        let y = list_top + scale(top, dpi);
        let line_height = scale(height, dpi);
        for column in 0..count {
            let index = start + column;
            let Some(RowIcon::Emoji(at)) = list.rows.get(index).map(|row| row.icon) else { continue };
            let left = inset + across * column as i32 / COLUMNS as i32 + gap;
            let rect = RECT { left, top: y + gap, right: left + cell_width - gap * 2, bottom: y + line_height - gap };
            cells.push(GridCell { index, at, rect });
        }
    }
    cells
}

#[allow(clippy::too_many_arguments)]
fn draw_grid(buffer: &mut Backbuffer, paint: &Paint, list: &ListState, grid: &Grid, cells: &[GridCell], width: i32, emoji: &[orbit_core::emoji::Emoji], cache: &mut HashMap<u16, Vec<u8>>) {
    let (theme, fonts, dpi) = (paint.theme, paint.fonts, paint.dpi);
    let list_top = scale(INPUT_HEIGHT + PADDING, dpi);
    let inset = scale(ROW_INSET, dpi);
    let tile_radius = scale(CELL_RADIUS, dpi) as f32;
    let size = scale(EMOJI_DIP, dpi);

    if list.rows.is_empty() {
        let rect = RECT { left: inset, top: list_top, right: width - inset, bottom: list_top + scale(ROW_HEIGHT, dpi) };
        if let Some(message) = list.scope.empty_message() {
            text::draw(buffer, message, &fonts.small, theme.secondary, rect, Align::Center);
        }
    }
    let layout = grid.layout(list.scroll);
    for &(line, top, height) in &layout {
        if let Line::Title(row) = line {
            let Some(title) = list.rows.get(row).map(|row| &row.title) else { continue };
            let y = list_top + scale(top, dpi);
            let rect = RECT { left: inset + scale(6, dpi), top: y + scale(6, dpi), right: width - inset, bottom: y + scale(height, dpi) };
            text::draw(buffer, title, &fonts.section, theme.tertiary, rect, Align::Left);
        }
    }
    // Every tile, then the highlight over them, wherever it's got to, then the emoji over that.
    let highlight = list.highlight.or_else(|| cells.iter().find(|cell| cell.index == list.selected).map(|cell| cell.rect));
    for cell in cells {
        let RECT { left, top, right, bottom } = cell.rect;
        buffer.fill_round_rect_alpha(left, top, right - left, bottom - top, tile_radius, paint.tile.0, paint.tile.1);
    }
    if let Some(RECT { left, top, right, bottom }) = highlight {
        let (w, h) = (right - left, bottom - top);
        // Over a tile, this comes to the same as the pressed look the selected tile used to have.
        let lift = if paint.hc { paint.pressed } else { (paint.pressed.0, ALPHA_PRESSED - ALPHA_TILE) };
        buffer.fill_round_rect_alpha(left, top, w, h, tile_radius, lift.0, lift.1);
        let (rim, alpha) = if paint.hc { (theme.accent, 255) } else { (theme.overlay, ALPHA_CELL_RIM) };
        buffer.stroke_round_rect_alpha(left, top, w, h, tile_radius, (paint.thin as f32 * 1.25).max(1.25), rim, alpha);
    }
    for cell in cells {
        if cache.len() >= EMOJI_CACHE_MAX && !cache.contains_key(&cell.at) {
            cache.clear();
        }
        let Some(entry) = emoji.get(cell.at as usize) else { continue };
        let pixels = cache.entry(cell.at).or_insert_with(|| text::emoji_pixels(entry.glyph, size).unwrap_or_default());
        let RECT { left, top, right, bottom } = cell.rect;
        buffer.blit_premultiplied(left + (right - left - size) / 2, top + (bottom - top - size) / 2, size, pixels);
    }
    // The same small scrollbar as a list.
    if layout.len() < grid.lines.len() {
        let track = scale(grid.room, dpi);
        let thumb = (track as i64 * layout.len() as i64 / grid.lines.len() as i64).max(scale(16, dpi) as i64) as i32;
        let last = grid.last_first().max(1);
        let thumb_y = list_top + ((track - thumb) as i64 * list.scroll.min(last) as i64 / last as i64) as i32;
        buffer.fill_round_rect_alpha(width - scale(6, dpi), thumb_y, scale(3, dpi), thumb, scale(2, dpi) as f32, paint.pressed.0, paint.pressed.1);
    }
}

struct FooterState<'a> {
    hints: &'a [(&'static str, &'static str)],
    status: &'a str,
    is_status: bool,
    vim: Option<VimMode>,
    logo: Option<&'a [u8]>,
    hovered: Option<&'static str>,
    pressed: Option<&'static str>,
    orbit_open: bool,
    /// Ctrl+K's actions are open, so its key stays lit.
    actions_open: bool,
}

fn draw_pill(buffer: &mut Backbuffer, paint: &Paint, area: RECT) {
    let (w, h) = (area.right - area.left, area.bottom - area.top);
    buffer.fill_round_rect_alpha(area.left, area.top, w, h, h as f32 / 2.0, paint.pill.0, paint.pill.1);
    let rim = if paint.hc { (paint.theme.edge, 255) } else { (paint.theme.overlay, ALPHA_PILL_RIM) };
    buffer.stroke_round_rect_alpha(area.left, area.top, w, h, h as f32 / 2.0, paint.thin as f32, rim.0, rim.1);
}

fn draw_footer(buffer: &mut Backbuffer, paint: &Paint, state: FooterState, width: i32, height: i32, buttons: &mut Vec<(RECT, &'static str)>) {
    let (theme, fonts, dpi) = (paint.theme, paint.fonts, paint.dpi);
    let inset = scale(ROW_INSET, dpi);
    let pill_height = scale(PILL_HEIGHT, dpi);
    let footer_top = height - scale(FOOTER_HEIGHT, dpi);
    let pill_top = footer_top + (scale(FOOTER_HEIGHT, dpi) - pill_height) / 2;
    let pill_bottom = pill_top + pill_height;
    let middle = pill_top + pill_height / 2;
    let hover_gap = scale(3, dpi);

    // On the right, what the keys do. Each is a button: a click does what the key does.
    let pad = scale(12, dpi);
    let space = scale(8, dpi);
    let measured: Vec<(i32, i32)> = state
        .hints
        .iter()
        .enumerate()
        .map(|(position, (keys, label))| {
            let face = if position == 0 { &fonts.hint_strong } else { &fonts.hint };
            (text::width(label, face), caps_width(keys, fonts, dpi))
        })
        .collect();
    let total: i32 = measured.iter().map(|(label, caps)| pad * 2 + label + space + caps).sum();
    let right_pill_left = width - inset - total;
    if !state.hints.is_empty() {
        draw_pill(buffer, paint, RECT { left: right_pill_left, top: pill_top, right: width - inset, bottom: pill_bottom });
    }
    let mut x = right_pill_left;
    for (position, ((keys, label), (label_width, caps))) in state.hints.iter().zip(&measured).enumerate() {
        let first_hint = position == 0;
        let face = if first_hint { &fonts.hint_strong } else { &fonts.hint };
        let area = RECT { left: x, top: pill_top, right: x + pad * 2 + label_width + space + caps, bottom: pill_bottom };
        let hovered = state.hovered == Some(*keys);
        // With the actions open, their key stays lit, as Orbit's pill does for its menu.
        let lit = hovered || (*keys == "Ctrl+K" && state.actions_open);
        if lit {
            let fill = if hovered && state.pressed == Some(*keys) { paint.pressed } else { paint.selection };
            let inner_height = area.bottom - area.top - hover_gap * 2;
            buffer.fill_round_rect_alpha(area.left + hover_gap, area.top + hover_gap, area.right - area.left - hover_gap * 2, inner_height, inner_height as f32 / 2.0, fill.0, fill.1);
        }
        let ink = if lit || first_hint { theme.text } else { theme.secondary };
        let label_rect = RECT { left: area.left + pad, top: pill_top, right: area.left + pad + label_width, bottom: pill_bottom };
        text::draw(buffer, label, face, ink, label_rect, Align::Left);
        let cap = if lit { paint.keycap_on_selection } else { paint.keycap };
        draw_caps(buffer, keys, label_rect.right + space, middle, fonts, cap, paint.cap_rim, if first_hint { theme.text } else { theme.secondary }, dpi);
        buttons.push((area, *keys));
        if position > 0 {
            // A short line between one hint and the next.
            let tall = scale(14, dpi);
            buffer.fill_rect_alpha(area.left, middle - tall / 2, paint.thin, tall, paint.divider.0, paint.divider.1);
        }
        x = area.right;
    }

    // On the left, Orbit itself. Clicking it opens Orbit's menu.
    let logo_size = scale(LOGO_DIP, dpi);
    let room_right = right_pill_left - scale(8, dpi);
    let mut left = inset;
    let label_face = &fonts.hint_strong;
    let label_width = text::width(state.status, label_face);
    let inner = scale(11, dpi) + logo_size + scale(8, dpi) + label_width + scale(14, dpi);
    let vim_width = state.vim.map_or(0, |_| text::width("NORMAL", &fonts.keycap) + scale(18, dpi) + scale(6, dpi));
    let pill_right = (left + inner).min(room_right - vim_width);
    let area = RECT { left, top: pill_top, right: pill_right, bottom: pill_bottom };
    draw_pill(buffer, paint, area);
    let hovered = state.hovered == Some(BUTTON_ORBIT_MENU);
    if hovered || state.orbit_open {
        let fill = if hovered && state.pressed == Some(BUTTON_ORBIT_MENU) { paint.pressed } else { paint.selection };
        let inner_height = pill_height - hover_gap * 2;
        buffer.fill_round_rect_alpha(left + hover_gap, pill_top + hover_gap, pill_right - left - hover_gap * 2, inner_height, inner_height as f32 / 2.0, fill.0, fill.1);
    }
    if let Some(logo) = state.logo {
        buffer.blit_premultiplied(left + scale(11, dpi), middle - logo_size / 2, logo_size, logo);
    }
    let text_left = left + scale(11, dpi) + logo_size + scale(8, dpi);
    let ink = if state.is_status { theme.accent } else if hovered { theme.text } else { theme.secondary };
    text::draw(buffer, state.status, label_face, ink, RECT { left: text_left, top: pill_top, right: pill_right - scale(12, dpi), bottom: pill_bottom }, Align::Left);
    buttons.push((area, BUTTON_ORBIT_MENU));
    left = pill_right + scale(6, dpi);

    if let Some(mode) = state.vim {
        // Which mode you're in, as Vim shows it. A click switches.
        let (label, fill, ink) = match mode {
            VimMode::Normal => ("NORMAL", (theme.accent, 255), theme.on_accent),
            VimMode::Insert => ("INSERT", paint.pill, theme.text),
        };
        let w = text::width(label, &fonts.keycap) + scale(18, dpi);
        let pill = RECT { left, top: pill_top, right: left + w, bottom: pill_bottom };
        buffer.fill_round_rect_alpha(left, pill_top, w, pill_height, pill_height as f32 / 2.0, fill.0, fill.1);
        text::draw(buffer, label, &fonts.keycap, ink, pill, Align::Center);
        buttons.push((pill, BUTTON_VIM_MODE));
    }
}

/// The title of the menu that picks an app, which searches apps rather than actions.
pub const APP_PICKER_TITLE: &str = "Open with";

/// How big an app's icon is in a menu.
pub const POPOVER_ICON_DIP: i32 = 20;

/// The small menu that Ctrl+K and the Orbit pill open, floating over the list.
#[allow(clippy::too_many_arguments)]
fn draw_popover(buffer: &mut Backbuffer, paint: &Paint, at: Popover, shown: usize, of: &str, field: &Field, caret_on: bool, view: &crate::view::View, icons: &crate::icons::IconCache, spots: &mut Spots, highlight: Option<RECT>) {
    let (theme, fonts, dpi, hc) = (paint.theme, paint.fonts, paint.dpi, paint.hc);
    let RECT { left, top, right, bottom } = at.rect;
    let (w, h) = (right - left, bottom - top);
    let radius = scale(POPOVER_RADIUS, dpi) as f32;
    let (surface, rim) = if hc {
        (theme.background, (theme.edge, 255))
    } else if paint.dark {
        (POPOVER_DARK, (theme.overlay, ALPHA_POPOVER_RIM))
    } else {
        (POPOVER_LIGHT, (theme.overlay, ALPHA_POPOVER_RIM))
    };
    if !hc {
        buffer.shadow_round_rect(left, top + scale(4, dpi), w, h, radius, scale(POPOVER_SHADOW_DIP, dpi) as f32, Rgb(0, 0, 0), if paint.dark { 120 } else { 60 });
    }
    buffer.fill_round_rect(left, top, w, h, radius, surface);
    buffer.stroke_round_rect_alpha(left, top, w, h, radius, paint.thin as f32, rim.0, rim.1);

    // What the menu is about, unless a drop-down's button says so already.
    let inner = scale(14, dpi);
    if at.titled {
        let title_rect = RECT { left: left + inner, top: top + scale(POPOVER_PAD, dpi), right: right - inner, bottom: top + scale(POPOVER_PAD + POPOVER_TITLE, dpi) };
        text::draw(buffer, of, &fonts.section, theme.tertiary, title_rect, Align::Left);
    }

    let row_x = left + scale(POPOVER_ROW_INSET, dpi);
    let row_width = w - scale(POPOVER_ROW_INSET * 2, dpi);
    let row_radius = scale(POPOVER_ROW_RADIUS, dpi) as f32;
    let glyph_size = scale(20, dpi);
    // The highlight goes down first, wherever it's got to on its way to the selected row.
    if let Some(rect) = highlight {
        buffer.fill_round_rect_alpha(rect.left, rect.top, rect.right - rect.left, rect.bottom - rect.top, row_radius, paint.selection.0, paint.selection.1);
    }
    if view.rows.is_empty() {
        let rect = RECT { left: row_x, top: at.list_top, right: row_x + row_width, bottom: at.list_top + at.row_height };
        let empty = if of == APP_PICKER_TITLE { "No matching apps" } else if of == FILTER_MENU_TITLE { "No matching filters" } else { "No matching actions" };
        text::draw(buffer, empty, &fonts.small, theme.secondary, rect, Align::Center);
    }
    for (slot, index) in (view.scroll..view.rows.len()).take(shown).enumerate() {
        let row = &view.rows[index];
        let y = at.list_top + slot as i32 * at.row_height;
        let selected = index == view.selected;
        if selected && highlight.is_none() {
            buffer.fill_round_rect_alpha(row_x, y, row_width, at.row_height, row_radius, paint.selection.0, paint.selection.1);
        }
        let danger = matches!(row.icon, RowIcon::Glyph(GLYPH_DELETE));
        let ink = if danger { theme.danger } else if selected { theme.selection_text } else { theme.text };
        let glyph_x = row_x + scale(8, dpi);
        if let RowIcon::App(hash) = row.icon {
            let size = scale(POPOVER_ICON_DIP, dpi);
            if let Some(pixels) = icons.peek(hash, size as usize) {
                buffer.blit_premultiplied(glyph_x, y + (at.row_height - size) / 2, size, pixels);
            }
        }
        let symbol = match row.icon {
            RowIcon::Glyph(glyph) => {
                Some((glyph, if danger { theme.danger } else if row.accent { theme.accent } else if selected { theme.text } else { theme.secondary }))
            }
            RowIcon::Tinted(glyph, tone) if !hc => Some((glyph, crate::rows::coloured_symbol(crate::rows::tone_colors(tone), paint.dark))),
            RowIcon::Tinted(glyph, _) => Some((glyph, if selected { theme.text } else { theme.secondary })),
            _ => None,
        };
        if let Some((glyph, color)) = symbol {
            let rect = RECT { left: glyph_x, top: y, right: glyph_x + glyph_size, bottom: y + at.row_height };
            text::draw(buffer, &glyph.to_string(), &fonts.icon_tile, color, rect, Align::Center);
        }
        let text_x = glyph_x + glyph_size + scale(8, dpi);
        let mut text_right = row_x + row_width - scale(8, dpi);
        if let Some(keys) = &row.keys {
            let width = caps_width(keys, fonts, dpi);
            let fill = if selected { paint.keycap_on_selection } else { paint.keycap };
            draw_caps(buffer, keys, text_right - width, y + at.row_height / 2, fonts, fill, paint.cap_rim, theme.secondary, dpi);
            text_right -= width + scale(8, dpi);
        }
        let title_width = text::width(&row.title, &fonts.small);
        let title_right = (text_x + title_width + scale(1, dpi)).min(text_right);
        text::draw(buffer, &row.title, &fonts.small, ink, RECT { left: text_x, top: y, right: title_right, bottom: y + at.row_height }, Align::Left);
        if !row.subtitle.is_empty() && title_right + scale(40, dpi) < text_right {
            let rect = RECT { left: title_right + scale(8, dpi), top: y, right: text_right, bottom: y + at.row_height };
            text::draw(buffer, &row.subtitle, &fonts.hint, theme.tertiary, rect, Align::Left);
        }
    }
    if view.rows.len() > shown {
        let track = at.row_height * shown as i32;
        let thumb = (track as i64 * shown as i64 / view.rows.len() as i64).max(scale(14, dpi) as i64) as i32;
        let thumb_y = at.list_top + ((track - thumb) as i64 * view.scroll as i64 / (view.rows.len() - shown) as i64) as i32;
        buffer.fill_round_rect_alpha(right - scale(5, dpi), thumb_y, scale(3, dpi), thumb, scale(2, dpi) as f32, paint.pressed.0, paint.pressed.1);
    }

    // The box that narrows it down: along the bottom, or a drop-down's at the top, with a line
    // between it and the rows.
    let line = if at.search_bottom < bottom { at.search_bottom - paint.thin } else { at.search_top };
    buffer.fill_rect_alpha(left, line, w, paint.thin, paint.divider.0, paint.divider.1);
    let clip = RECT { left: left + inner, top: at.search_top, right: right - inner, bottom: at.search_bottom };
    let placeholder = if of == APP_PICKER_TITLE { "Search apps…" } else if of == FILTER_MENU_TITLE { "Search filters…" } else { "Search actions…" };
    let shift = draw_field(buffer, paint, field, placeholder, clip, &fonts.small, 13, caret_on, None, spots.shift(SpotOf::MenuSearch));
    spots.add(SpotOf::MenuSearch, RECT { left, top: at.search_top, right, bottom: at.search_bottom }, clip, &field.text, SpotFace::Small, false, shift);
}

/// The logo in the footer, at this DPI.
pub fn logo_pixels(dpi: u32) -> Option<Vec<u8>> {
    let size = scale(LOGO_DIP, dpi);
    let icon = crate::tray::load_app_icon(size, size);
    if icon.is_invalid() {
        return None;
    }
    let pixels = Backbuffer::icon_pixels(icon, size);
    unsafe {
        let _ = windows::Win32::UI::WindowsAndMessaging::DestroyIcon(icon);
    }
    pixels
}

/// The name of the footer's Vim mode indicator, which is also a button.
pub const BUTTON_VIM_MODE: &str = "vim-mode";
/// The pill with Orbit's logo on it.
pub const BUTTON_ORBIT_MENU: &str = "orbit-menu";
/// The chevron where the search symbol usually is.
pub const BUTTON_BACK: &str = "back";
/// The drop-down at the right of the search box that picks an extension's filter.
pub const BUTTON_FILTER: &str = "filter";
/// The title of the menu it opens, which Ctrl+P opens too.
pub const FILTER_MENU_TITLE: &str = "Show";

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_page_is_still_itself_while_what_is_in_it_changes() {
        let form = |multiline| Scope::Form { title: "New quicklink".into(), link: true, multiline, choice: false };
        assert_eq!(page_key(&form(false)), page_key(&form(true)), "moving between a form's fields");
        let colour = |r| Scope::Colour(orbit_core::colour::Colour { r, g: 0, b: 0 });
        assert_eq!(page_key(&colour(10)), page_key(&colour(200)), "picking another colour");
        assert_ne!(page_key(&Scope::Root), page_key(&Scope::Clipboard));
        let list = |command: &str| Scope::Extension { ext: "github".into(), command: command.into(), title: String::new(), placeholder: String::new() };
        assert_ne!(page_key(&list("issues")), page_key(&list("pulls")), "another of an extension's lists");
    }

    #[test]
    fn a_menu_on_its_way_in_keeps_its_parts_together() {
        let at = popover_layout(720, 520, 96, MenuSide::Right, 5);
        let moved = at.shifted(7);
        assert_eq!(moved.rect.top - at.rect.top, 7);
        assert_eq!(moved.rect.bottom - at.rect.bottom, 7);
        assert_eq!((moved.list_top - at.list_top, moved.search_top - at.search_top, moved.search_bottom - at.search_bottom), (7, 7, 7));
        assert_eq!((moved.rect.left, moved.rect.right, moved.row_height), (at.rect.left, at.rect.right, at.row_height));
    }

    fn plain(n: usize) -> Vec<Row> {
        (0..n).map(|i| Row::info(format!("{i}"), "", ' ')).collect()
    }

    #[test]
    fn row_hit_testing_respects_scroll_and_bounds() {
        let top = INPUT_HEIGHT + PADDING;
        let rows = plain(20);
        assert_eq!(row_at(top - 1, 96, &rows, 0, 8), None);
        assert_eq!(row_at(top, 96, &rows, 0, 8), Some(0));
        assert_eq!(row_at(top + ROW_HEIGHT, 96, &rows, 0, 8), Some(1));
        assert_eq!(row_at(top + ROW_HEIGHT, 96, &rows, 5, 8), Some(6));
        assert_eq!(row_at(top + ROW_HEIGHT * 8, 96, &rows, 0, 8), None);
        assert_eq!(row_at(top + ROW_HEIGHT * 2, 96, &plain(2), 0, 8), None, "past the last row");
    }

    #[test]
    fn hit_testing_scales_with_dpi() {
        let top = (INPUT_HEIGHT + PADDING) * 2;
        assert_eq!(row_at(top + ROW_HEIGHT * 2, 192, &plain(20), 0, 8), Some(1));
    }

    #[test]
    fn section_titles_take_less_room_and_can_not_be_clicked() {
        let mut rows = plain(3);
        rows.insert(0, Row::header("Favourites"));
        rows.insert(2, Row::header("Suggestions"));
        let top = INPUT_HEIGHT + PADDING;
        assert_eq!(row_at(top, 96, &rows, 0, 8), None, "on a title");
        assert_eq!(row_at(top + FIRST_HEADER_HEIGHT, 96, &rows, 0, 8), Some(1));
        assert_eq!(row_at(top + FIRST_HEADER_HEIGHT + ROW_HEIGHT, 96, &rows, 0, 8), None, "on the second title");
        assert_eq!(row_at(top + FIRST_HEADER_HEIGHT + HEADER_HEIGHT + ROW_HEIGHT, 96, &rows, 0, 8), Some(3));
    }

    #[test]
    fn the_first_title_sits_right_under_the_search_box() {
        let mut rows = plain(3);
        rows.insert(0, Row::header("Favourites"));
        rows.insert(2, Row::header("Suggestions"));
        let layout = row_layout(&rows, 0, 96, 8);
        assert_eq!(layout[0], (0, INPUT_HEIGHT + PADDING, FIRST_HEADER_HEIGHT), "no gap over the first title");
        assert_eq!(layout[2].2, HEADER_HEIGHT, "a title between sections keeps its room");
        // Scrolled so a later title is at the top, it's still the one between two sections.
        assert_eq!(row_layout(&rows, 2, 96, 8)[0].2, HEADER_HEIGHT);
    }

    #[test]
    fn every_visible_row_fits_and_a_title_is_never_left_alone_at_the_bottom() {
        let mut rows = plain(4);
        rows.insert(3, Row::header("Commands"));
        let layout = row_layout(&rows, 0, 96, 3);
        assert_eq!(layout.iter().map(|l| l.0).collect::<Vec<_>>(), [0, 1, 2], "the title has nothing under it");
        let bottom = INPUT_HEIGHT + PADDING + ROW_HEIGHT * 3;
        assert!(layout.iter().all(|l| l.1 + l.2 <= bottom));
        let mixed: Vec<Row> = (0..16).map(|i| if i % 4 == 0 { Row::header("") } else { Row::info("", "", ' ') }).collect();
        assert!(row_layout(&mixed, 0, 96, 8).len() >= 8, "titles only ever make room");
    }

    #[test]
    fn a_short_list_gets_a_short_window() {
        let full = window_size(96, 8).1;
        assert_eq!(fitted_height(96, &plain(20), 8), full, "more than fits");
        assert_eq!(fitted_height(96, &plain(8), 8), full, "exactly fits");
        assert_eq!(fitted_height(96, &plain(2), 8), full - ROW_HEIGHT * 6);
        assert_eq!(fitted_height(96, &plain(0), 8), full - ROW_HEIGHT * 7, "room to say there's nothing");
        let mut titled = plain(2);
        titled.insert(0, Row::header("Suggestions"));
        assert_eq!(fitted_height(96, &titled, 8), full - ROW_HEIGHT * 6 + FIRST_HEADER_HEIGHT);
        assert_eq!(fitted_height(192, &plain(2), 8), (full - ROW_HEIGHT * 6) * 2);
    }

    #[test]
    fn menus_fit_the_launcher_instead_of_growing_it() {
        for dpi in [96, 120, 144, 192] {
            for visible in 3..=12 {
                for count in 0..14 {
                    let rows = plain(count);
                    let height = launcher_height(dpi, &Scope::Root, &rows, false, true, visible);
                    let shown = popover_rows_for(height, dpi);
                    assert!(popover_needs_height(shown, dpi, MenuSide::Right) <= height, "{dpi} dpi, {visible} rows, {count} results");
                }
            }
        }
    }

    #[test]
    fn a_menu_takes_about_seven_tenths_of_the_room() {
        let height = window_size(96, 8).1;
        let room = height - INPUT_HEIGHT - FOOTER_HEIGHT;
        let shown = popover_rows_for(height, 96);
        let taken = popover_needs_height(shown, 96, MenuSide::Right) - INPUT_HEIGHT - FOOTER_HEIGHT - POPOVER_GAP;
        assert!(taken * 10 <= room * 8 && taken * 10 >= room * 6, "{taken} of {room}");
        assert_eq!(popover_rows_for(window_size(96, 12).1, 96), POPOVER_MAX_ROWS, "a tall launcher shows the most");
    }

    #[test]
    fn every_row_in_a_short_window_is_drawn_and_above_the_footer() {
        let mut rows = plain(4);
        rows.insert(0, Row::header("Suggestions"));
        rows.insert(3, Row::header("Commands"));
        let layout = row_layout(&rows, 0, 96, 8);
        assert_eq!(layout.len(), rows.len());
        let last = layout.last().unwrap();
        assert!(last.1 + last.2 <= fitted_height(96, &rows, 8) - FOOTER_HEIGHT - PADDING);
    }

    #[test]
    fn the_menu_sits_over_the_footer_on_the_side_it_belongs_to() {
        let (width, height) = window_size(96, 8);
        let right = popover_layout(width, height, 96, MenuSide::Right, 6);
        assert_eq!(right.rect.right, width - ROW_INSET);
        assert_eq!(right.rect.bottom, height - FOOTER_HEIGHT);
        let left = popover_layout(width, height, 96, MenuSide::Left, 6);
        assert_eq!(left.rect.left, ROW_INSET);
        assert!(popover_needs_height(POPOVER_MAX_ROWS, 96, MenuSide::Right) <= window_size(96, 9).1, "the longest menu fits a normal window");
        let tall = popover_needs_height(8, 96, MenuSide::Right);
        assert!(popover_layout(width, tall, 96, MenuSide::Right, 8).rect.top >= INPUT_HEIGHT, "and never reaches the search box");
    }

    #[test]
    fn the_menu_finds_rows_only_on_rows() {
        let (width, height) = window_size(96, 8);
        let menu = popover_layout(width, height, 96, MenuSide::Right, 5);
        let x = menu.rect.left + 5;
        assert_eq!(menu.row_at(x, menu.list_top, 5, 0, 5), Some(0));
        assert_eq!(menu.row_at(x, menu.list_top + POPOVER_ROW, 5, 0, 5), Some(1));
        assert_eq!(menu.row_at(x, menu.list_top + POPOVER_ROW, 20, 7, 5), Some(8), "scrolled");
        assert_eq!(menu.row_at(x, menu.list_top - 1, 5, 0, 5), None, "the title");
        assert_eq!(menu.row_at(x, menu.search_top, 5, 0, 5), None, "the search box");
        assert_eq!(menu.row_at(menu.rect.left - 1, menu.list_top, 5, 0, 5), None, "outside it");
        assert_eq!(menu.row_at(x, menu.list_top + POPOVER_ROW * 2, 2, 0, 5), None, "past the last row");
        assert!(menu.contains(x, menu.search_top));
        assert!(!menu.contains(menu.rect.right, menu.search_top));
    }

    #[test]
    fn a_drop_down_hangs_from_its_button_at_the_top_right() {
        for dpi in [96, 144] {
            let (width, height) = window_size(dpi, 8);
            let shown = drop_rows_for(height, dpi);
            let menu = popover_layout(width, height, dpi, MenuSide::Drop, shown);
            let button_bottom = (scale(INPUT_HEIGHT, dpi) + scale(FILTER_BUTTON_DIP, dpi)) / 2;
            assert_eq!(menu.rect.right, width - scale(BAND_RIGHT_DIP, dpi), "lined up with the button's right edge");
            assert!(menu.rect.top > button_bottom && menu.rect.top - button_bottom <= scale(8, dpi), "just under the button");
            assert!(menu.rect.bottom + scale(POPOVER_GAP, dpi) <= height - scale(FOOTER_HEIGHT, dpi), "{dpi}: clear of the footer");
            assert!(!menu.titled, "its button says what it's for");
            assert_eq!(menu.search_top, menu.rect.top, "its search box is at the top, by the button");
            assert!(menu.list_top >= menu.search_bottom);
            let x = menu.rect.left + 5;
            assert_eq!(menu.row_at(x, menu.list_top, 9, 0, shown), Some(0));
            assert_eq!(menu.row_at(x, menu.search_top + 2, 9, 0, shown), None, "the search box isn't a row");
            assert_eq!(menu.row_at(x, menu.list_top + menu.row_height * shown as i32, 99, 0, shown), None, "below the last row");
        }
        let (width, _) = window_size(96, 8);
        let short = popover_needs_height(POPOVER_MIN_ROWS, 96, MenuSide::Drop);
        assert!(popover_layout(width, short, 96, MenuSide::Drop, POPOVER_MIN_ROWS).rect.bottom <= short - FOOTER_HEIGHT);
    }

    #[test]
    fn a_tile_is_found_at_any_scale() {
        let rows: Vec<Row> = (0..25).map(|i| Row::info(format!("{i}"), "", GLYPH_EMOJI)).collect();
        let grid = Grid::new(&rows, 352);
        let (width, _) = window_size(96, 8);
        let top = INPUT_HEIGHT + PADDING;
        assert_eq!(grid_row_at(&grid, 0, ROW_INSET + 2, top + 2, width, 96), Some(0));
        assert_eq!(grid_row_at(&grid, 0, width - ROW_INSET - 2, top + 2, width, 96), Some(9));
        assert_eq!(grid_row_at(&grid, 0, ROW_INSET + 2, top + crate::grid::CELL_DIP + 2, width, 96), Some(10));
        assert_eq!(grid_row_at(&grid, 0, (ROW_INSET + 2) * 2, (top + crate::grid::CELL_DIP + 2) * 2, width * 2, 192), Some(10));
        assert_eq!(grid_row_at(&grid, 0, ROW_INSET + 2, top - 2, width, 96), None, "above the grid");
        assert_eq!(grid_row_at(&grid, 0, ROW_INSET - 2, top + 2, width, 96), None, "left of it");
    }

    #[test]
    fn the_window_is_bigger_than_the_launcher_by_its_shadow() {
        assert_eq!(frame_size(96, (720, 500)), (720 + SHADOW_MARGIN * 2, 500 + SHADOW_MARGIN * 2));
        assert_eq!(frame_margin(192), SHADOW_MARGIN * 2);
    }

    #[test]
    fn typing_lines_up_with_the_titles_below_in_every_scope() {
        assert_eq!(input_left(&Scope::Root, 96), TEXT_LEFT);
        assert_eq!(input_left(&Scope::Clipboard, 96), TEXT_LEFT);
    }

    #[test]
    fn prefixes_count_utf16() {
        assert_eq!(prefix("a🙂b", 3), "a🙂");
        assert_eq!(prefix("abc", 9), "abc");
        assert_eq!(prefix("abc", 0), "");
    }

    #[test]
    fn size_scales_with_dpi() {
        assert_eq!(window_size(192, 8).0, BASE_WIDTH * 2);
        assert_eq!(window_size(96, 8).1, INPUT_HEIGHT + PADDING * 2 + ROW_HEIGHT * 8 + FOOTER_HEIGHT);
    }
}

#[cfg(test)]
mod accent_tests {
    use super::*;

    /// Shades of the pale grey this was first run with: #C4BFBA.
    const PALE: [Rgb; 7] = [
        Rgb(0xEB, 0xEA, 0xE8), Rgb(0xDB, 0xD8, 0xD5), Rgb(0xCF, 0xCB, 0xC7), Rgb(0xC4, 0xBF, 0xBA),
        Rgb(0xB8, 0xB2, 0xAC), Rgb(0xAC, 0xA5, 0x9E), Rgb(0x9C, 0x93, 0x8B),
    ];
    /// Windows' default blue.
    const BLUE: [Rgb; 7] = [
        Rgb(0x99, 0xEB, 0xFF), Rgb(0x4C, 0xC2, 0xFF), Rgb(0x00, 0x91, 0xF8), Rgb(0x00, 0x78, 0xD4),
        Rgb(0x00, 0x67, 0xC0), Rgb(0x00, 0x3E, 0x92), Rgb(0x00, 0x1A, 0x68),
    ];

    #[test]
    fn dark_and_light_use_the_shades_windows_uses() {
        assert_eq!(with_accent(DARK, true, Some(&BLUE)).accent, BLUE[1]);
        assert_eq!(with_accent(LIGHT, false, Some(&BLUE)).accent, BLUE[4]);
    }

    #[test]
    fn a_pale_accent_stays_visible_on_a_light_theme() {
        let theme = with_accent(LIGHT, false, Some(&PALE));
        assert!(contrast(theme.accent, theme.background) >= 3.0);
    }

    #[test]
    fn text_on_the_accent_is_readable() {
        for (palette, dark) in [(&PALE, true), (&PALE, false), (&BLUE, true), (&BLUE, false)] {
            let theme = with_accent(if dark { DARK } else { LIGHT }, dark, Some(palette));
            assert!(contrast(theme.accent, theme.on_accent) >= 4.5, "{theme:?}");
        }
    }

    #[test]
    fn without_a_palette_the_accent_is_blue_not_orange() {
        let theme = with_accent(DARK, true, None);
        assert!(theme.accent.2 > theme.accent.0);
        let theme = with_accent(LIGHT, false, None);
        assert!(theme.accent.2 > theme.accent.0);
    }
}
