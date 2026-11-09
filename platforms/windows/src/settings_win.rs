//! The settings window. Native controls in Orbit's colours, created when opened and freed when
//! closed.
//!
//! Changes save as they're made. It posts a complete new `Settings` to the launcher, which applies
//! it in one place and answers through `applied`, so this window never reaches into the launcher's
//! state and only says "Saved" when that's true. Only the hotkey tables, edited as text, can hold
//! something that makes no sense. They keep their last good value, and nothing else waits for them.

use crate::app_picker::{self, Candidate, Row as PickerRow};
use crate::app::{WM_ORBIT_ENTE_ACCOUNT, WM_ORBIT_IMPORT, WM_ORBIT_LINK_REQUEST, WM_ORBIT_RUN_BUILTIN, WM_ORBIT_SET_ALIAS, WM_ORBIT_SET_HIDDEN, WM_ORBIT_SETTINGS_APPLY, WM_ORBIT_SETTINGS_RECORDING, WM_ORBIT_SETTINGS_REOPEN, WM_ORBIT_SETTINGS_TOOL_DONE};
use crate::editor::{EditorContext, register_form, unregister_form};
use crate::forms::{self, Builder, ButtonKind, ListPart, ManagedRow, Style, Tile, get_lines, get_text, is_checked, selected, set_status, set_text};
use crate::link_form::LinkRequest;
use crate::worker::{DataRow, Req};
use crate::gfx::Backbuffer;
use crate::keys;
use crate::integrations::{self, Tool};
use crate::screen_picker;
use crate::rows::{GLYPH_SEARCH, GLYPH_CLIPBOARD, GLYPH_COMMAND, GLYPH_FOLDER, GLYPH_KEY, GLYPH_KEYBOARD, GLYPH_LINK, GLYPH_NOTE, GLYPH_SNIPPET};
use crate::util::{SendHwnd, wide};
use orbit_core::bindings_text::*;
use orbit_core::hotkeys::{self, Hotkey, same_target};
use orbit_core::settings::Appearance;
use orbit_core::{Action, Builtin, Settings};
use orbit_hotkeys::chord::{is_reserved_on_windows, is_usable_global};
use orbit_hotkeys::{Chord, HyperMode, Mods, TapBehavior};
use std::cell::Cell;
use windows::Win32::Foundation::*;
use windows::Win32::Graphics::Gdi::*;
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::Controls::Dialogs::{GetOpenFileNameW, OFN_FILEMUSTEXIST, OFN_PATHMUSTEXIST, OPENFILENAMEW};
use windows::Win32::UI::Controls::{DRAWITEMSTRUCT, MEASUREITEMSTRUCT, ODS_SELECTED, WM_MOUSELEAVE};
use windows::Win32::UI::HiDpi::{AdjustWindowRectExForDpi, GetDpiForMonitor, GetDpiForSystem, MDT_EFFECTIVE_DPI};
use windows::Win32::UI::Input::KeyboardAndMouse::{
    EnableWindow, GetDoubleClickTime, GetFocus, GetKeyState, IsWindowEnabled, ReleaseCapture, SetCapture, SetFocus, TME_LEAVE, TRACKMOUSEEVENT, TrackMouseEvent, VK_BACK, VK_CONTROL, VK_DELETE, VK_DOWN, VK_ESCAPE,
    VK_F2, VK_LBUTTON, VK_LEFT, VK_LWIN, VK_MENU, VK_RETURN, VK_RIGHT, VK_RWIN, VK_SHIFT, VK_SPACE, VK_TAB, VK_UP,
};
use windows::Win32::UI::Shell::{DefSubclassProc, RemoveWindowSubclass, SetWindowSubclass};
use windows::Win32::UI::WindowsAndMessaging::*;
use windows::core::{PCWSTR, PWSTR, w};

const PAGES: &[(&str, char)] = &[
    ("General", '\u{E713}'),
    ("Search", GLYPH_SEARCH),
    ("Hotkeys", GLYPH_KEYBOARD),
    ("HyperKey", '\u{E945}'),
    ("macOS feel", '\u{E7F8}'),
    ("Clipboard", GLYPH_CLIPBOARD),
    ("Snippets", GLYPH_SNIPPET),
    ("Links & commands", GLYPH_LINK),
    ("Integrations", GLYPH_KEY),
    ("Data", GLYPH_FOLDER),
];

/// Each page's colour, for its badge and its square in the page list. No orange.
const PAGE_COLORS: [crate::gfx::Rgb; 10] = [
    crate::gfx::Rgb(142, 142, 150),
    crate::gfx::Rgb(10, 132, 255),
    crate::gfx::Rgb(94, 92, 230),
    crate::gfx::Rgb(175, 82, 222),
    crate::gfx::Rgb(50, 173, 230),
    crate::gfx::Rgb(48, 176, 120),
    crate::gfx::Rgb(255, 55, 95),
    crate::gfx::Rgb(0, 168, 160),
    crate::gfx::Rgb(232, 62, 62),
    crate::gfx::Rgb(110, 110, 118),
];
/// What each page is for, under its name.
const PAGE_ABOUT: [&str; 10] = [
    "How Orbit starts, how it looks, and how the launcher behaves.",
    "Choose what comes up when you search in Orbit, give anything an alias to type, or a hotkey that runs it from anywhere. Turning something off only hides it from search.",
    "Keys that run an app, a link or a command from anywhere.",
    "One key becomes a modifier of its own, for hotkeys that never clash with other apps.",
    "Mac shortcuts in every app, with Alt where a Mac uses Cmd.",
    "What you copy, kept on this PC to find and paste again.",
    "Text you use often, typed for you when you type its keyword.",
    "Websites, searches and programs you open by name or alias.",
    "Orbit fetches one value when you pick it, and keeps none. Copies clear after 30 seconds.",
    "Where Orbit keeps your things, and how to move them or start over.",
];
/// The strip across the top that the window is dragged by, with its own buttons at the right.
const TOPBAR_DIP: i32 = 44;
const CAPTION_BUTTON_DIP: i32 = 46;
const CAPTION_HEIGHT_DIP: i32 = 32;
const PAGE_BADGE_DIP: i32 = 22;
const PAGE_GENERAL: usize = 0;
const PAGE_SEARCH: usize = 1;
const PAGE_HOTKEYS: usize = 2;
const PAGE_HYPER: usize = 3;
const PAGE_CLIPBOARD: usize = 5;
const PAGE_SNIPPETS: usize = 6;
const PAGE_LINKS: usize = 7;
const PAGE_INTEGRATIONS: usize = 8;

const SIDEBAR_DIP: i32 = 208;
const WINDOW_WIDTH_DIP: i32 = 892;
/// Narrower than this, the controls wouldn't fit beside their names any more.
const WINDOW_MIN_WIDTH_DIP: i32 = 640;
const WINDOW_HEIGHT_DIP: i32 = 720;
/// The message area at the foot of the page list.
const STATUS_DIP: i32 = 96;
const PAGE_ROW_DIP: i32 = 36;
const HOTKEY_ROW_DIP: i32 = 40;
const PICKER_ROW_DIP: i32 = 36;
const PICKER_ICON_DIP: i32 = 22;
/// The field on a hotkey row that's listening, before any keys are held.
const HOTKEY_FIELD_DIP: i32 = 112;
/// The strip at the right of the page where the scroll thumb lives.
const SCROLL_GUTTER_DIP: i32 = 14;
const THUMB_WIDTH_DIP: i32 = 6;
/// Between the ends of the track and the top and bottom of the view.
const THUMB_INSET_DIP: i32 = 8;
const THUMB_MIN_DIP: i32 = 32;
/// How far a line of wheel scrolling moves the page.
const WHEEL_LINE_DIP: i32 = 20;
/// The part of a picker row that says Add or Remove.
const PICKER_ACTION_DIP: i32 = 84;

/// The window pages scroll inside, and the page that moves within it.
pub const ID_VIEW: i32 = 90;
pub const ID_CONTENT: i32 = 91;
const ID_PAGES: i32 = 100;
const ID_HOTKEY: i32 = 110;
const ID_SUGGEST: i32 = 120;
const ID_HOTKEY_LIST: i32 = 121;
const ID_HOTKEY_TEXT: i32 = 124;
const ID_ACTION_SEARCH: i32 = 125;
const HOTKEY_LIST_SUBCLASS: usize = 0x4F48;
const SEARCH_LIST_SUBCLASS: usize = 0x4F4A;
const ID_SEARCH_FILTER: i32 = 170;
const ID_SEARCH_LIST: i32 = 171;
const ID_ALIAS_EDIT: i32 = 172;
const ALIAS_EDIT_SUBCLASS: usize = 0x4F4B;
const EM_SETSEL: u32 = 0x00B1;
const ACTION_SEARCH_SUBCLASS: usize = 0x4F49;
const NO_HOTKEYS: &str = "No hotkeys yet. Search above, or press Ctrl+K on anything in the launcher.";
const HYPER_OFF_ROW: &str = "HyperKey is off, so these won't run. Open HyperKey settings";
const IDLE_HOTKEY_NOTE: &str = "Click, then press keys. Backspace goes back to the default.";
const RECORDING_NOTE: &str = "Press the keys you want. Esc cancels, and Backspace goes back to the default.";
/// How many matches show at once while searching. More than this scrolls.
const SEARCH_ROWS: usize = 6;
const LABEL_CLEAR: &str = "Clear";
const LABEL_RESET: &str = "Reset";
const LABEL_DELETE: &str = "Delete";
const LABEL_UNINSTALL: &str = "Uninstall";
const LABEL_INSTALLING: &str = "Installing…";
const ID_CLEAR_CLIPS: i32 = 130;
const ID_UNHIDE: i32 = 131;
const ID_RESET_SHORTCUTS: i32 = 132;
const ID_DELETE_DATA: i32 = 133;
const ID_EXPORT: i32 = 140;
const ID_IMPORT: i32 = 141;
const ID_OPEN_DIR: i32 = 142;
const ID_REINDEX: i32 = 143;
const ID_DIAGNOSTICS: i32 = 144;
const ID_TOOL_OP: i32 = 150;
const ID_TOOL_ENTE: i32 = 151;
const ID_ENTE_SIGN_IN: i32 = 152;
const ID_SNIPPET_LIST: i32 = 160;
const ID_LINK_LIST: i32 = 161;
const ID_COMMAND_LIST: i32 = 162;
const ID_LINK_NEW: i32 = 163;
const ID_COMMAND_NEW: i32 = 164;
const ID_SNIPPET_NEW: i32 = 165;
/// The managed lists, in the order of their ids.
const LIST_SNIPPETS: usize = 0;
const LIST_LINKS: usize = 1;
const LIST_COMMANDS: usize = 2;
/// Each picker has three controls from here on: search box, Browse, list.
const ID_PICKERS: i32 = 200;
const PICKER_IDS: i32 = 10;
const PICKER_HYPER: usize = 0;
const PICKER_CLIPBOARD: usize = 1;
const PICKER_SNIPPETS: usize = 2;
const PICKER_MAC: usize = 3;
const PICKER_LAUNCHER: usize = 4;

/// Runs once, after a change, and saves it.
const SAVE_TIMER: usize = 1;
/// Clears a message that has been seen.
const STATUS_TIMER: usize = 2;
/// Takes back a first press on something dangerous.
const ARM_TIMER: usize = 3;
/// How long typing has to pause before a text box saves.
const TEXT_SAVE_DELAY_MS: u32 = 500;
const SAVED_MS: u32 = 2200;
const MESSAGE_MS: u32 = 7000;
const ARM_MS: u32 = 4000;
/// Which drop-down of offered values is which, in `State::presets`.
const PRESET_TAP: usize = 0;
const PRESET_ITEMS: usize = 1;
const PRESET_SIZE: usize = 2;
const PRESET_DAYS: usize = 3;
const TAP_TIMES: &[(u32, &str)] = &[(150, "150 ms"), (200, "200 ms"), (250, "250 ms"), (300, "300 ms"), (400, "400 ms"), (500, "500 ms")];
const CLIP_ITEMS: &[(u32, &str)] = &[(100, "100 items"), (250, "250 items"), (500, "500 items"), (1000, "1,000 items"), (5000, "5,000 items"), (10000, "10,000 items")];
const CLIP_SIZES: &[(u32, &str)] = &[(64 * 1024, "64 KB"), (256 * 1024, "256 KB"), (1024 * 1024, "1 MB"), (10 * 1024 * 1024, "10 MB")];
const CLIP_DAYS: &[(u32, &str)] = &[(7, "7 days"), (30, "30 days"), (90, "90 days"), (365, "1 year"), (0, "Forever")];

thread_local! {
    static CURRENT: Cell<Option<HWND>> = const { Cell::new(None) };
    /// The page the next `open` starts on.
    static START_PAGE: Cell<usize> = const { Cell::new(PAGE_GENERAL) };
    /// Where the window that the next `open` replaces was.
    static NEXT_PLACE: Cell<Option<RECT>> = const { Cell::new(None) };
    /// The next `open` shows the window behind whatever's in front, because it's replacing one.
    static QUIET_OPEN: Cell<bool> = const { Cell::new(false) };
    /// While the pages are being made: the page list's colour, the page's, and the line between.
    static EARLY_FILL: Cell<Option<(crate::gfx::Rgb, crate::gfx::Rgb, crate::gfx::Rgb)>> = const { Cell::new(None) };
    /// The DPI of the window being opened, for its lists to measure rows before it has any state.
    static OPEN_DPI: Cell<u32> = const { Cell::new(96) };
    /// What each tool's button says while it's being installed or removed. Kept here because that
    /// can outlast a window that gets rebuilt.
    static TOOLS_BUSY: Cell<[Option<&'static str>; 2]> = const { Cell::new([None, None]) };
    /// How wide a preview may be, in DIP, standing in for a narrow screen.
    static PREVIEW_ROOM: Cell<Option<i32>> = const { Cell::new(None) };
}

struct Controls {
    /// What the Search page has to say about an alias or a hotkey being changed there.
    search_note: HWND,
    hotkey: HWND,
    hotkey_note: HWND,
    login: HWND,
    focus_running: HWND,
    toggles_app: HWND,
    preserve: HWND,
    mac_on: HWND,
    mac_text: HWND,
    mac_apps: HWND,
    favorites_only: HWND,
    glass: HWND,
    fit_results: HWND,
    launcher_games: HWND,
    appearance: HWND,
    keyboard: HWND,
    keyboard_note: HWND,
    /// What the launcher follows to choose its screen, and the picture of the screens.
    screen_mode: HWND,
    screen: HWND,
    low_resource: HWND,
    link_icons: HWND,
    exchange_rates: HWND,
    copy_formatted: HWND,
    weather: HWND,
    hotkey_list: HWND,
    hotkeys_note: HWND,
    action_search: HWND,
    text_error: HWND,
    edit_as_text: HWND,
    shortcuts: HWND,
    hyper_on: HWND,
    hyper_trigger: HWND,
    hyper_mode: HWND,
    hyper_tap: HWND,
    hyper_games: HWND,
    hyper_table: HWND,
    clip_on: HWND,
    clip_persist: HWND,
    clip_images: HWND,
    expansion: HWND,
    op_on: HWND,
    ente_on: HWND,
    op_status: HWND,
    op_note: HWND,
    ente_status: HWND,
    op_button: HWND,
    ente_button: HWND,
    ente_sign_in: HWND,
    ente_sign_in_note: HWND,
    hidden_note: HWND,
    hidden_button: HWND,
    suggest: HWND,
}

/// A drop-down of sensible values. One set by hand in settings.json is offered too, so that opening
/// Settings never changes it.
struct Preset {
    hwnd: HWND,
    values: Vec<u32>,
}

impl Preset {
    fn value(&self) -> u32 {
        self.values[selected(self.hwnd).min(self.values.len() - 1)]
    }
}

fn preset_combo(b: &mut Builder, label: &str, table: &[(u32, &str)], current: u32, custom: impl Fn(u32) -> String) -> Preset {
    let mut values: Vec<u32> = table.iter().map(|entry| entry.0).collect();
    let mut names: Vec<String> = table.iter().map(|entry| entry.1.to_string()).collect();
    if !values.contains(&current) {
        values.push(current);
        names.push(custom(current));
    }
    let at = values.iter().position(|value| *value == current).unwrap_or(0);
    let names: Vec<&str> = names.iter().map(String::as_str).collect();
    let hwnd = b.combo(label, &names, at, 0);
    Preset { hwnd, values }
}

fn tool_slot(tool: Tool) -> usize {
    if tool == Tool::OnePassword { 0 } else { 1 }
}

fn set_tool_busy(tool: Tool, label: Option<&'static str>) {
    TOOLS_BUSY.with(|busy| {
        let mut now = busy.get();
        now[tool_slot(tool)] = label;
        busy.set(now);
    });
}

/// A tool's row as it stands: what to say, and what its button does.
fn tool_row(tool: Tool) -> (String, &'static str, bool) {
    if let Some(label) = TOOLS_BUSY.with(|busy| busy.get()[tool_slot(tool)]) {
        return ("This can take a minute.".to_string(), label, false);
    }
    match integrations::find(tool) {
        None => ("Not installed".to_string(), "Install", true),
        Some(path) => {
            if integrations::can_remove(tool, &path) {
                ("Installed".to_string(), LABEL_UNINSTALL, true)
            } else {
                // Installed some other way, so it isn't Orbit's to remove.
                ("Installed outside Orbit".to_string(), LABEL_UNINSTALL, false)
            }
        }
    }
}

/// What's still to be done for 1Password, which depends on whether the tool is there yet.
fn one_password_note() -> &'static str {
    if integrations::find(Tool::OnePassword).is_none() {
        "Installs with Windows Package Manager. 1Password's terms apply."
    } else {
        "In the 1Password app, turn on Settings, Developer, Integrate with 1Password CLI."
    }
}

fn refresh_tool_row(s: &State, tool: Tool) {
    let (status, button) = match tool {
        Tool::OnePassword => (s.controls.op_status, s.controls.op_button),
        Tool::EnteAuth => (s.controls.ente_status, s.controls.ente_button),
    };
    let (text, label, can_press) = tool_row(tool);
    forms::set_text_only(status, &text);
    set_text(button, label);
    if !can_press {
        move_focus_off(s, button);
    }
    unsafe {
        let _ = EnableWindow(button, can_press);
        if tool == Tool::EnteAuth {
            let _ = EnableWindow(s.controls.ente_sign_in, integrations::find(tool).is_some());
        } else {
            set_status(s.controls.op_note, one_password_note());
        }
        let _ = InvalidateRect(Some(button), None, false);
    }
}

/// Install or Uninstall was pressed. It runs off this thread, because it can take a minute.
fn change_tool(s: &mut State, tool: Tool) {
    let button = if tool == Tool::OnePassword { s.controls.op_button } else { s.controls.ente_button };
    let install = integrations::find(tool).is_none();
    if !install && !confirm_in_place(s, button, LABEL_UNINSTALL) {
        return;
    }
    set_tool_busy(tool, Some(if install { LABEL_INSTALLING } else { "Uninstalling…" }));
    refresh_tool_row(s, tool);
    integrations::change_tool(tool, install, s.context.main, WM_ORBIT_SETTINGS_TOOL_DONE);
}

const ENTE_SIGN_IN_NOTE: &str = "Opens Ente's own sign-in in a terminal. Orbit never sees your password.";

/// Ente's tool has said who's signed in. `false` means there's no window to show it in.
pub fn ente_account(account: &integrations::EnteAccount) -> bool {
    let Some(s) = CURRENT.with(|c| c.get()).and_then(state) else { return false };
    let (note, label) = match account {
        integrations::EnteAccount::SignedIn(email) => (format!("Signed in as {email}."), "Sign in again\u{2026}"),
        integrations::EnteAccount::SignedOut => ("Not signed in. Ente's sign-in opens in a terminal.".to_string(), "Sign in\u{2026}"),
        integrations::EnteAccount::Unknown => (ENTE_SIGN_IN_NOTE.to_string(), "Sign in\u{2026}"),
    };
    forms::set_text_only(s.controls.ente_sign_in_note, &note);
    set_text(s.controls.ente_sign_in, label);
    unsafe {
        let _ = InvalidateRect(Some(s.controls.ente_sign_in), None, false);
    }
    true
}

/// A tool has finished. `false` means there's no window to say so in.
pub fn tool_done(tool: Tool, message: &str, succeeded: bool) -> bool {
    let installing = TOOLS_BUSY.with(|busy| busy.get()[tool_slot(tool)]) == Some(LABEL_INSTALLING);
    set_tool_busy(tool, None);
    let Some(s) = CURRENT.with(|c| c.get()).and_then(state) else { return false };
    refresh_tool_row(s, tool);
    // Having just installed it, the user wants to see it, and once it's gone there's nothing to
    // show. One that failed leaves the switch as the user had it.
    let switch = if tool == Tool::OnePassword { s.controls.op_on } else { s.controls.ente_on };
    if succeeded {
        forms::set_checked(switch, installing);
    }
    if tool == Tool::EnteAuth && installing && succeeded {
        integrations::check_ente_account(s.context.main, WM_ORBIT_ENTE_ACCOUNT);
    }
    apply_dependents(s);
    save_soon(s, 1);
    show_result(s, message);
    true
}

/// Something the user asked for has finished. A save that follows doesn't talk over it.
fn show_result(s: &mut State, message: &str) {
    show_status(s, message, MESSAGE_MS);
    s.result_until = Some(std::time::Instant::now() + std::time::Duration::from_millis(MESSAGE_MS as u64));
}

/// A control about to be disabled can't hold the keyboard, and Windows wouldn't move it on.
fn move_focus_off(s: &State, control: HWND) {
    if unsafe { GetFocus() } == control {
        if let Some(next) = next_in_tab_order(s, control) {
            forms::focus(next);
        }
    }
}

fn next_in_tab_order(s: &State, control: HWND) -> Option<HWND> {
    unsafe { GetNextDlgTabItem(window_of(s), Some(control), false) }.ok().filter(|next| *next != control && !next.is_invalid())
}

/// Something finished that the user asked for from Settings. `false` means Settings isn't in front,
/// so this wouldn't be seen here.
pub fn show_message(message: &str) -> bool {
    let Some(hwnd) = CURRENT.with(|c| c.get()) else { return false };
    if unsafe { GetForegroundWindow() } != hwnd {
        return false;
    }
    let Some(s) = state(hwnd) else { return false };
    show_result(s, message);
    true
}

/// A search box over a list of apps.
struct Picker {
    edit: HWND,
    list: HWND,
    chosen: Vec<String>,
    rows: Vec<PickerRow>,
    /// The row under the pointer.
    hover: Option<usize>,
    /// Windows has been asked to say when the pointer leaves.
    tracking: bool,
    /// All of a long list is showing.
    expanded: bool,
    /// The selected row has just been added, so it's lit until the next key or click.
    just_added: bool,
}

/// A friendly list of things to edit or delete, drawn by this window.
struct ManagedList {
    hwnd: HWND,
    rows: Vec<ManagedRow>,
    /// The row whose Delete has been pressed once.
    armed: Option<usize>,
    hover: Option<(usize, ListPart)>,
    /// Windows has been asked to say when the pointer leaves.
    tracking: bool,
    /// What it says with no rows.
    empty: &'static str,
}

/// The kinds of thing the Search page lists, in the order it lists them.
const SEARCH_GROUPS: [&str; 5] = ["Orbit commands", "Apps", "Quicklinks", "Your commands", "Integrations"];
const GROUP_LINKS: usize = 2;
const GROUP_COMMANDS: usize = 3;
const SEARCH_HINT: &str = "Press the keys you want. Backspace removes the hotkey, and Esc cancels.";

/// Something that can come up when searching in Orbit, which the Search page can turn off, give
/// an alias, or give a hotkey.
struct SearchItem {
    /// The same key the launcher keeps in `hidden` and `aliases`.
    key: String,
    name: String,
    group: usize,
    icon: Option<u64>,
    tile: Tile,
    /// What a hotkey for it runs. `None` for what can't have one, such as something that asks first.
    action: Option<Action>,
}

/// One row of the Search page's list.
#[derive(Clone, Copy, PartialEq, Debug)]
enum SearchRow {
    Header(usize),
    Item(usize),
}

/// The parts of a Search row that do something, left to right.
#[derive(Clone, Copy, PartialEq, Debug)]
enum SearchCell {
    Alias,
    Hotkey,
    Switch,
}

/// The quicklinks and commands you made, as Search page items.
fn own_search_items(settings: &Settings) -> Vec<SearchItem> {
    let links = settings.links.iter().map(|link| SearchItem {
        key: crate::catalog::link_key(&link.id),
        name: link.name.clone(),
        group: GROUP_LINKS,
        icon: None,
        tile: Tile::Glyph(GLYPH_LINK),
        action: Some(Action::Url { url: link.url.clone() }),
    });
    let commands = settings.custom_commands.iter().map(|command| SearchItem {
        key: crate::catalog::custom_key(&command.id),
        name: command.name.clone(),
        group: GROUP_COMMANDS,
        icon: None,
        tile: Tile::Glyph(GLYPH_COMMAND),
        action: Some(Action::Custom { id: command.id.clone() }),
    });
    links.chain(commands).collect()
}

/// Everything the launcher can find, grouped and in name order.
fn search_items(apps: &[crate::indexer::AppEntry], settings: &Settings) -> Vec<SearchItem> {
    let mut items: Vec<SearchItem> = Builtin::ALL
        .iter()
        .map(|&(command, key, title, _, _)| SearchItem {
            key: key.to_string(),
            name: title.to_string(),
            group: 0,
            icon: None,
            tile: Tile::Glyph(crate::catalog::glyph_for_builtin(command)),
            // What asks first in the launcher shouldn't be a key away.
            action: (!command.needs_confirmation()).then_some(Action::Builtin { command }),
        })
        .collect();
    items.extend(apps.iter().map(|app| SearchItem {
        key: crate::catalog::app_key(&app.id),
        name: app.name.clone(),
        group: 1,
        icon: Some(crate::indexer::icon_hash(&app.id)),
        tile: forms::letter_tile(&app.name),
        action: Some(Action::FocusOrLaunchApp { id: app.id.clone(), name: app.name.clone() }),
    }));
    items.extend(own_search_items(settings));
    items.extend(Tool::ALL.iter().map(|tool| SearchItem { key: tool.key().to_string(), name: tool.name().to_string(), group: 4, icon: None, tile: Tile::Glyph(GLYPH_KEY), action: None }));
    sort_search_items(&mut items);
    items
}

fn sort_search_items(items: &mut [SearchItem]) {
    items.sort_by_cached_key(|item| (item.group, item.name.to_lowercase()));
}

/// The rows for what's typed in the filter: each group that has a match, under its heading. An
/// alias counts as a match too.
fn search_rows(items: &[SearchItem], settings: &Settings, query: &str) -> Vec<SearchRow> {
    let query = query.trim().to_lowercase();
    let matches = |item: &SearchItem| query.is_empty() || item.name.to_lowercase().contains(&query) || item_alias(settings, &item.key).to_lowercase() == query;
    let mut rows = Vec::new();
    for group in 0..SEARCH_GROUPS.len() {
        let matching: Vec<usize> = (0..items.len()).filter(|&at| items[at].group == group && matches(&items[at])).collect();
        if !matching.is_empty() {
            rows.push(SearchRow::Header(group));
            rows.extend(matching.into_iter().map(SearchRow::Item));
        }
    }
    rows
}

/// The alias an item answers to: a quicklink's or command's own, or one given to it since.
fn item_alias(settings: &Settings, key: &str) -> String {
    let own = settings
        .links
        .iter()
        .find(|link| crate::catalog::link_key(&link.id) == key)
        .map(|link| link.alias.clone())
        .or_else(|| settings.custom_commands.iter().find(|command| crate::catalog::custom_key(&command.id) == key).map(|command| command.alias.clone()))
        .unwrap_or_default();
    if own.is_empty() { settings.aliases.get(key).cloned().unwrap_or_default() } else { own }
}

/// The keys that run something, as key caps text.
fn item_hotkey(settings: &Settings, action: &Action) -> Option<String> {
    settings
        .hyper
        .bindings
        .iter()
        .find(|binding| same_target(&binding.action, action))
        .map(|binding| format!("Hyper+{}", binding.key))
        .or_else(|| settings.shortcuts.iter().find(|binding| same_target(&binding.action, action)).map(|binding| binding.chord.display("Win")))
}

/// Where a Search row's alias, hotkey and switch go, as left and right edges, in a list this wide.
fn search_columns(dpi: u32, width: i32) -> [(i32, i32); 3] {
    let px = |value: i32| forms::scale(value, dpi);
    let narrow = width < px(520);
    let switch = (width - px(10) - px(36), width - px(10));
    let hotkey_width = if narrow { px(108) } else { px(140) };
    let hotkey = (switch.0 - px(20) - hotkey_width, switch.0 - px(20));
    let alias_width = if narrow { px(76) } else { px(100) };
    let alias = (hotkey.0 - px(12) - alias_width, hotkey.0 - px(12));
    [alias, hotkey, switch]
}

fn search_cell_at(dpi: u32, width: i32, x: i32) -> Option<SearchCell> {
    let [alias, hotkey, switch] = search_columns(dpi, width);
    let slack = forms::scale(6, dpi);
    if x >= alias.0 && x < alias.1 {
        Some(SearchCell::Alias)
    } else if x >= hotkey.0 && x < hotkey.1 {
        Some(SearchCell::Hotkey)
    } else if x >= switch.0 - slack && x < switch.1 + slack {
        Some(SearchCell::Switch)
    } else {
        None
    }
}

/// Why an alias can't be used for this item, if it can't.
fn alias_problem(s: &State, key: &str, alias: &str) -> Option<String> {
    if alias.chars().any(char::is_whitespace) {
        return Some("An alias is one word, with no spaces.".to_string());
    }
    if alias.chars().count() > 24 {
        return Some("An alias is a short word, 24 letters at most.".to_string());
    }
    if alias.is_empty() {
        return None;
    }
    s.search_items
        .iter()
        .find(|item| item.key != key && item_alias(&s.base, &item.key).eq_ignore_ascii_case(alias))
        .map(|item| format!("\u{201C}{}\u{201D} already uses that alias.", item.name))
}

/// Says something under the Search page's description, or nothing.
fn say_search_note(s: &mut State, text: &str, alert: bool) {
    let note = s.controls.search_note;
    s.style.set_alert(note, alert);
    set_note_text(s, note, text);
}

/// Fills the Search page's list from the filter, and sizes it to fit.
fn refresh_search_list(s: &mut State) {
    let list = s.search_list;
    s.search_filled = true;
    finish_alias_edit(s, false);
    let chosen = unsafe { SendMessageW(list, LB_GETCURSEL, None, None) }.0;
    s.search_rows = search_rows(&s.search_items, &s.base, &get_text(s.search_filter));
    s.search_hover = None;
    unsafe {
        SendMessageW(list, WM_SETREDRAW, Some(WPARAM(0)), None);
        SendMessageW(list, LB_RESETCONTENT, None, None);
        for row in &s.search_rows {
            let text = match row {
                SearchRow::Header(group) => SEARCH_GROUPS[*group].to_string(),
                SearchRow::Item(at) => search_row_text(s, *at),
            };
            let text = wide(&text);
            SendMessageW(list, LB_ADDSTRING, None, Some(LPARAM(text.as_ptr() as isize)));
        }
        if chosen >= 0 && !s.search_rows.is_empty() {
            let at = (chosen as usize).min(s.search_rows.len() - 1);
            let at = nearest_search_row(&s.search_rows, at, 1).unwrap_or(at);
            SendMessageW(list, LB_SETCURSEL, Some(WPARAM(at)), None);
        }
        SendMessageW(list, WM_SETREDRAW, Some(WPARAM(1)), None);
    }
    let height = forms::scale(HOTKEY_ROW_DIP * s.search_rows.len().max(1) as i32, s.style.dpi);
    resize_on_page(s, list, height);
    unsafe {
        let _ = InvalidateRect(Some(list), None, true);
    }
}

/// What a screen reader hears for a row.
fn search_row_text(s: &State, at: usize) -> String {
    let item = &s.search_items[at];
    let mut text = format!("{}, {}", item.name, if s.base.hidden.contains(&item.key) { "off" } else { "on" });
    let alias = item_alias(&s.base, &item.key);
    if !alias.is_empty() {
        text.push_str(&format!(", alias {alias}"));
    }
    if let Some(keys) = item.action.as_ref().and_then(|action| item_hotkey(&s.base, action)) {
        text.push_str(&format!(", hotkey {keys}"));
    }
    text
}

/// The first item row going this way from `from`, or the other way if there's none.
fn nearest_search_row(rows: &[SearchRow], from: usize, step: isize) -> Option<usize> {
    let usable = |at: isize| at >= 0 && (at as usize) < rows.len() && matches!(rows[at as usize], SearchRow::Item(_));
    for direction in [step, -step] {
        let mut at = from as isize;
        while at >= 0 && (at as usize) < rows.len() {
            if usable(at) {
                return Some(at as usize);
            }
            at += direction;
        }
    }
    None
}

fn redraw_search_row(s: &State, at: usize) {
    unsafe {
        let mut row = RECT::default();
        if SendMessageW(s.search_list, LB_GETITEMRECT, Some(WPARAM(at)), Some(LPARAM(&mut row as *mut RECT as isize))).0 >= 0 {
            let _ = InvalidateRect(Some(s.search_list), Some(&row), false);
        }
    }
}

/// The row's text changed, so a screen reader hears the new one.
fn retext_search_row(s: &State, at: usize) {
    let Some(SearchRow::Item(item)) = s.search_rows.get(at).copied() else { return };
    let text = wide(&search_row_text(s, item));
    unsafe {
        let chosen = SendMessageW(s.search_list, LB_GETCURSEL, None, None).0;
        SendMessageW(s.search_list, WM_SETREDRAW, Some(WPARAM(0)), None);
        SendMessageW(s.search_list, LB_DELETESTRING, Some(WPARAM(at)), None);
        SendMessageW(s.search_list, LB_INSERTSTRING, Some(WPARAM(at)), Some(LPARAM(text.as_ptr() as isize)));
        if chosen >= 0 {
            SendMessageW(s.search_list, LB_SETCURSEL, Some(WPARAM(chosen as usize)), None);
        }
        SendMessageW(s.search_list, WM_SETREDRAW, Some(WPARAM(1)), None);
    }
    redraw_search_row(s, at);
}

/// Turns a row's item on or off. The launcher keeps the list and says when it changes.
fn toggle_search_row(s: &mut State, at: usize) {
    let Some(SearchRow::Item(item)) = s.search_rows.get(at).copied() else { return };
    let key = s.search_items[item].key.clone();
    let hide = !s.base.hidden.contains(&key);
    if hide {
        s.base.hidden.push(key.clone());
    } else {
        s.base.hidden.retain(|hidden| *hidden != key);
    }
    s.last_saved.hidden = s.base.hidden.clone();
    s.context.main.post(WM_ORBIT_SET_HIDDEN, crate::util::box_for_post((key, hide)), 0);
    set_status(s.controls.hidden_note, &hidden_count(s.base.hidden.len()));
    unsafe {
        let _ = EnableWindow(s.controls.hidden_button, !s.base.hidden.is_empty());
    }
    retext_search_row(s, at);
}

/// Does what a row's part is for: edit the alias, listen for a hotkey, or turn it on or off.
fn act_on_search_cell(s: &mut State, at: usize, cell: SearchCell) {
    s.search_cell = cell;
    match cell {
        SearchCell::Alias => start_alias_edit(s, at),
        SearchCell::Hotkey => start_search_record(s, at),
        SearchCell::Switch => toggle_search_row(s, at),
    }
    redraw_search_row(s, at);
}

/// Opens the box for a row's alias, over its alias column.
fn start_alias_edit(s: &mut State, at: usize) {
    let Some(SearchRow::Item(item)) = s.search_rows.get(at).copied() else { return };
    stop_hotkey_record(s);
    finish_alias_edit(s, true);
    let dpi = s.style.dpi;
    let px = |value: i32| forms::scale(value, dpi);
    let mut row = RECT::default();
    unsafe {
        if SendMessageW(s.search_list, LB_GETITEMRECT, Some(WPARAM(at)), Some(LPARAM(&mut row as *mut RECT as isize))).0 < 0 {
            return;
        }
        SendMessageW(s.search_list, LB_SETCURSEL, Some(WPARAM(at)), None);
    }
    let [alias, _, _] = search_columns(s.style.dpi, row.right - row.left);
    let box_height = px(20);
    let middle = (row.top + row.bottom) / 2;
    let text = item_alias(&s.base, &s.search_items[item].key);
    s.alias_row = Some(at);
    s.search_cell = SearchCell::Alias;
    say_search_note(s, "Type an alias, then press Enter. Esc cancels, and an empty one removes it.", false);
    unsafe {
        let _ = SetWindowPos(s.alias_edit, Some(HWND_TOP), alias.0 + px(10), middle - box_height / 2, alias.1 - alias.0 - px(16), box_height, SWP_SHOWWINDOW);
        set_text(s.alias_edit, &text);
        SendMessageW(s.alias_edit, EM_SETSEL, Some(WPARAM(0)), Some(LPARAM(-1)));
        let _ = SetFocus(Some(s.alias_edit));
    }
    redraw_search_row(s, at);
}

/// Closes the alias box, keeping what's in it if `keep` and it can be used. Gives false if it
/// can't, and the box stays open to fix it.
fn finish_alias_edit(s: &mut State, keep: bool) -> bool {
    let Some(at) = s.alias_row else { return true };
    let Some(SearchRow::Item(item)) = s.search_rows.get(at).copied() else {
        s.alias_row = None;
        unsafe {
            let _ = ShowWindow(s.alias_edit, SW_HIDE);
        }
        return true;
    };
    let key = s.search_items[item].key.clone();
    let typed = get_text(s.alias_edit).trim().to_string();
    if keep {
        if let Some(problem) = alias_problem(s, &key, &typed) {
            say_search_note(s, &problem, true);
            return false;
        }
    }
    s.alias_row = None;
    say_search_note(s, "", false);
    let had_focus = unsafe { GetFocus() } == s.alias_edit;
    unsafe {
        if had_focus {
            let _ = SetFocus(Some(s.search_list));
        }
        let _ = ShowWindow(s.alias_edit, SW_HIDE);
    }
    if keep && typed != item_alias(&s.base, &key) {
        set_item_alias(&mut s.base, &key, &typed);
        set_item_alias(&mut s.last_saved, &key, &typed);
        s.context.main.post(WM_ORBIT_SET_ALIAS, crate::util::box_for_post((key, typed.clone())), 0);
        show_status(s, if typed.is_empty() { "Alias removed" } else { "Alias saved" }, SAVED_MS);
        retext_search_row(s, at);
    }
    redraw_search_row(s, at);
    true
}

/// Gives an item this alias, where the launcher keeps it: in a quicklink or command of its own, or
/// otherwise in the list of aliases. Empty takes it away.
pub fn set_item_alias(settings: &mut Settings, key: &str, alias: &str) {
    let mut own = false;
    if let Some(link) = settings.links.iter_mut().find(|link| crate::catalog::link_key(&link.id) == key) {
        link.alias = alias.to_string();
        own = true;
    } else if let Some(command) = settings.custom_commands.iter_mut().find(|command| crate::catalog::custom_key(&command.id) == key) {
        command.alias = alias.to_string();
        own = true;
    }
    if own || alias.is_empty() {
        settings.aliases.remove(key);
    } else {
        settings.aliases.insert(key.to_string(), alias.to_string());
    }
}

/// Starts listening for a row's hotkey, in the row itself.
fn start_search_record(s: &mut State, at: usize) {
    let Some(SearchRow::Item(item)) = s.search_rows.get(at).copied() else { return };
    finish_alias_edit(s, true);
    let Some(action) = s.search_items[item].action.clone() else {
        say_search_note(s, "This can't have a hotkey, because it asks before it does anything.", false);
        return;
    };
    let starting = s.hotkey_target.is_none();
    s.hotkey_target = Some(RecordTarget::New(action));
    s.search_recording = Some(s.search_items[item].key.clone());
    s.hotkey_problem = None;
    s.hotkey_asking = None;
    s.held_keys.clear();
    if starting {
        pause_shortcuts(s, true);
    }
    say_hotkey_problem(s, None);
    forms::focus(s.search_list);
    redraw_search_row(s, at);
}

/// Takes away whatever hotkey runs a row's item.
fn remove_search_hotkey(s: &mut State, at: usize) {
    let Some(SearchRow::Item(item)) = s.search_rows.get(at).copied() else { return };
    let Some(action) = s.search_items[item].action.clone() else { return };
    let before = s.base.hyper.bindings.len() + s.base.shortcuts.len();
    s.base.hyper.bindings.retain(|binding| !same_target(&binding.action, &action));
    s.base.shortcuts.retain(|binding| !same_target(&binding.action, &action));
    if s.base.hyper.bindings.len() + s.base.shortcuts.len() != before {
        refresh_hotkey_list(s);
        fill_hotkey_text(s);
        save_soon(s, 1);
        retext_search_row(s, at);
    }
}

/// The row whose item is listening for its hotkey, if one is.
fn search_recording_row(s: &State) -> Option<usize> {
    let key = s.search_recording.as_ref()?;
    s.search_rows.iter().position(|row| matches!(row, SearchRow::Item(at) if s.search_items[*at].key == *key))
}

fn draw_search_item(s: &mut State, item: &DRAWITEMSTRUCT) {
    let (width, height) = (item.rcItem.right - item.rcItem.left, item.rcItem.bottom - item.rcItem.top);
    let Some(mut buffer) = Backbuffer::new(width, height) else { return };
    let dpi = s.style.dpi;
    let px = |value: i32| forms::scale(value, dpi);
    let at = item.itemID as usize;
    let row = s.search_rows.get(at).copied();
    let tile_size = px(PICKER_ICON_DIP);
    let pixels = match row {
        Some(SearchRow::Item(at)) => s.search_items[at].icon.and_then(|hash| icon_pixels(s, hash, tile_size as usize)),
        _ => None,
    };
    let s: &State = s;
    let style = &s.style;
    let theme = &style.theme;
    buffer.fill(theme.card);
    let [alias_col, hotkey_col, switch_col] = search_columns(dpi, width);
    let middle = height / 2;
    match row {
        Some(SearchRow::Header(group)) => {
            let top = height / 3;
            buffer.text(SEARCH_GROUPS[group], style.small, theme.secondary, RECT { left: px(8), top, right: alias_col.0 - px(8), bottom: height }, false);
            buffer.text("Alias", style.small, theme.tertiary, RECT { left: alias_col.0 + px(10), top, right: alias_col.1, bottom: height }, false);
            buffer.text("Hotkey", style.small, theme.tertiary, RECT { left: hotkey_col.0 + px(10), top, right: hotkey_col.1, bottom: height }, false);
            buffer.text("Shown", style.small, theme.tertiary, RECT { left: switch_col.0 - px(20), top, right: switch_col.1, bottom: height }, true);
        }
        Some(SearchRow::Item(index)) => {
            let entry = &s.search_items[index];
            let on = !s.base.hidden.contains(&entry.key);
            let focused = unsafe { GetFocus() } == s.search_list;
            let editing = s.alias_row == Some(at);
            let recording = search_recording_row(s) == Some(at);
            let chosen = item.itemState.0 & ODS_SELECTED.0 != 0 && (focused || editing);
            let hover = s.search_hover.filter(|hover| hover.0 == at).map(|hover| hover.1);
            let surface = if chosen {
                theme.selection
            } else if hover.is_some() {
                theme.selection.mix(theme.card, 0.5)
            } else {
                theme.card
            };
            if surface != theme.card {
                buffer.fill_round_rect(0, px(2), width, height - px(4), px(forms::RADIUS_SELECTION) as f32, surface);
            }
            let (tile_x, tile_y) = (px(8), (height - tile_size) / 2);
            match pixels {
                Some(pixels) => buffer.blit_premultiplied(tile_x, tile_y, tile_size, &pixels),
                None => draw_small_tile(style, &mut buffer, (tile_x, tile_y, tile_size), entry.tile, if chosen { theme.keycap_on_selection } else { theme.keycap }),
            }
            let text_left = tile_x + tile_size + px(10);
            let ink = if on { theme.text } else { theme.secondary };
            buffer.text(&entry.name, style.font, ink, RECT { left: text_left, top: 0, right: alias_col.0 - px(10), bottom: height }, false);

            let cap = if chosen { theme.keycap_on_selection } else { theme.keycap };
            let field_height = px(26);
            let field_top = middle - field_height / 2;
            let radius = px(forms::RADIUS_ROW) as f32;
            let inset = 1.max(dpi as i32 / 96);
            // A field with an accent edge, for a part that's being changed.
            let field = |buffer: &mut Backbuffer, (left, right): (i32, i32)| {
                buffer.fill_round_rect(left, field_top, right - left, field_height, radius, theme.accent);
                buffer.fill_round_rect(left + inset, field_top + inset, right - left - inset * 2, field_height - inset * 2, radius - inset as f32, theme.field);
            };
            // The keyboard's place in the row, when the keyboard is how it got there.
            let ring = |buffer: &mut Backbuffer, (left, right): (i32, i32)| {
                buffer.fill_round_rect(left - inset * 2, field_top - inset * 2, right - left + inset * 4, field_height + inset * 4, radius + 2.0, theme.text);
                buffer.fill_round_rect(left - inset, field_top - inset, right - left + inset * 2, field_height + inset * 2, radius + 1.0, surface);
            };
            let keyboard_here = |cell: SearchCell| chosen && focused && style.focus_by_keyboard && s.search_cell == cell && !recording;

            // Alias.
            let alias = item_alias(&s.base, &entry.key);
            if editing {
                field(&mut buffer, alias_col);
            } else {
                let lit = hover == Some(Some(SearchCell::Alias));
                let pill = if alias.is_empty() {
                    None
                } else {
                    let room = alias_col.1 - alias_col.0;
                    Some((alias_col.0, alias_col.0 + (buffer.text_width(&alias, style.small) + px(20)).min(room)))
                };
                let span = pill.unwrap_or(alias_col);
                if keyboard_here(SearchCell::Alias) {
                    ring(&mut buffer, span);
                }
                match pill {
                    Some(pill) => {
                        buffer.fill_round_rect(pill.0, field_top, pill.1 - pill.0, field_height, radius, if lit { cap.mix(theme.text, 0.15) } else { cap });
                        buffer.text(&alias, style.small, ink, RECT { left: pill.0 + px(10), top: field_top, right: pill.1 - px(6), bottom: field_top + field_height }, false);
                    }
                    None if lit || keyboard_here(SearchCell::Alias) => {
                        buffer.fill_round_rect(alias_col.0, field_top, alias_col.1 - alias_col.0, field_height, radius, cap.mix(surface, 0.5));
                        buffer.text("Add alias", style.small, theme.secondary, RECT { left: alias_col.0 + px(10), top: field_top, right: alias_col.1 - px(4), bottom: field_top + field_height }, false);
                    }
                    None if hover.is_some() || chosen => {
                        buffer.text("Add alias", style.small, theme.tertiary, RECT { left: alias_col.0 + px(10), top: field_top, right: alias_col.1 - px(4), bottom: field_top + field_height }, false);
                    }
                    None => {}
                }
            }

            // Hotkey.
            if recording {
                field(&mut buffer, hotkey_col);
                let shown = if s.held_keys.is_empty() { s.hotkey_asking.as_ref().map(|hotkey| hotkey.label("Win")).unwrap_or_default() } else { s.held_keys.clone() };
                if shown.is_empty() {
                    buffer.text("Press keys\u{2026}", style.small, theme.accent, RECT { left: hotkey_col.0 + px(10), top: field_top, right: hotkey_col.1 - px(4), bottom: field_top + field_height }, false);
                } else {
                    crate::ui::draw_keycaps(&mut buffer, &shown, hotkey_col.0 + px(6), middle, style.small, style.icon_small, (theme.keycap, 255), theme.text, dpi);
                }
            } else if let Some(action) = &entry.action {
                let lit = hover == Some(Some(SearchCell::Hotkey));
                match item_hotkey(&s.base, action) {
                    Some(keys) => {
                        let caps = crate::ui::keycaps_width(&buffer, &keys, style.small, style.icon_small, dpi);
                        // Long ones run into the space before the switch, never over it.
                        let x = hotkey_col.0.min(switch_col.0 - px(12) - caps);
                        if keyboard_here(SearchCell::Hotkey) {
                            ring(&mut buffer, (x, x + caps));
                        }
                        let fill = if lit { cap.mix(theme.text, 0.15) } else { cap };
                        crate::ui::draw_keycaps(&mut buffer, &keys, x, middle, style.small, style.icon_small, (fill, 255), ink, dpi);
                    }
                    None if lit || keyboard_here(SearchCell::Hotkey) => {
                        if keyboard_here(SearchCell::Hotkey) {
                            ring(&mut buffer, hotkey_col);
                        }
                        buffer.fill_round_rect(hotkey_col.0, field_top, hotkey_col.1 - hotkey_col.0, field_height, radius, cap.mix(surface, 0.5));
                        buffer.text("Record hotkey", style.small, theme.secondary, RECT { left: hotkey_col.0 + px(10), top: field_top, right: hotkey_col.1 - px(4), bottom: field_top + field_height }, false);
                    }
                    None if hover.is_some() || chosen => {
                        buffer.text("Record hotkey", style.small, theme.tertiary, RECT { left: hotkey_col.0 + px(10), top: field_top, right: hotkey_col.1 - px(4), bottom: field_top + field_height }, false);
                    }
                    None => {}
                }
            }

            // On or off.
            let switch_top = middle - px(10);
            if keyboard_here(SearchCell::Switch) {
                let ring_px = px(2);
                buffer.fill_round_rect(switch_col.0 - ring_px, switch_top - ring_px, px(36) + ring_px * 2, px(20) + ring_px * 2, (px(10) + ring_px) as f32, theme.text);
                buffer.fill_round_rect(switch_col.0 - ring_px / 2, switch_top - ring_px / 2, px(36) + ring_px, px(20) + ring_px, px(10) as f32, surface);
            }
            style.draw_switch(&mut buffer, switch_col.0, switch_top, on, false, surface);
        }
        None => {}
    }
    buffer.copy_to_dc_at(item.hDC, item.rcItem.left, item.rcItem.top);
}

/// The Search page's list. A click on a row's alias, hotkey or switch changes it. From the keyboard,
/// Left and Right move between those, and Enter or Space changes the one that's lit.
unsafe extern "system" fn search_list_proc(hwnd: HWND, message: u32, wparam: WPARAM, lparam: LPARAM, _id: usize, _data: usize) -> LRESULT {
    unsafe {
        if message == WM_NCDESTROY {
            let _ = RemoveWindowSubclass(hwnd, Some(search_list_proc), SEARCH_LIST_SUBCLASS);
            return DefSubclassProc(hwnd, message, wparam, lparam);
        }
        let Some(s) = state(GetAncestor(hwnd, GA_ROOT)) else {
            return DefSubclassProc(hwnd, message, wparam, lparam);
        };
        let key = wparam.0 as u16;
        let recording = s.search_recording.is_some() && s.hotkey_target.is_some();
        if s.swallow_keys && swallow_after_recording(s, message, lparam) {
            return LRESULT(0);
        }
        let width = || {
            let mut client = RECT::default();
            let _ = GetClientRect(hwnd, &mut client);
            client.right
        };
        let x_of = |lparam: LPARAM| (lparam.0 & 0xFFFF) as i16 as i32;
        let chosen = || SendMessageW(hwnd, LB_GETCURSEL, None, None).0;
        match message {
            WM_PAINT if s.search_rows.is_empty() => {
                s.style.paint_empty_list(hwnd, "Nothing matches that.");
                LRESULT(0)
            }
            WM_ERASEBKGND if s.search_rows.is_empty() => LRESULT(1),
            // The alias box sits on the list, so its colours are asked for here.
            WM_CTLCOLOREDIT => s.style.handle(hwnd, message, wparam, lparam).unwrap_or_else(|| DefSubclassProc(hwnd, message, wparam, lparam)),
            WM_GETDLGCODE => {
                if recording {
                    return LRESULT(DLGC_WANTALLKEYS as isize);
                }
                let base = DefSubclassProc(hwnd, message, wparam, lparam);
                let wanted = (lparam.0 as *const MSG).as_ref().is_some_and(|m| {
                    let key = m.wParam.0 as u16;
                    m.message == WM_KEYDOWN && (key == VK_RETURN.0 || key == VK_DELETE.0 || key == VK_BACK.0)
                });
                if wanted { LRESULT(base.0 | DLGC_WANTMESSAGE as isize) } else { base }
            }
            WM_KEYDOWN | WM_SYSKEYDOWN if recording => {
                if is_repeat(lparam) {
                    return LRESULT(0);
                }
                let plain = held_keys_text().is_empty();
                if key == VK_ESCAPE.0 {
                    stop_hotkey_record(s);
                    s.swallow_keys = true;
                } else if plain && (key == VK_BACK.0 || key == VK_DELETE.0) {
                    let at = search_recording_row(s);
                    stop_hotkey_record(s);
                    if let Some(at) = at {
                        remove_search_hotkey(s, at);
                    }
                    s.swallow_keys = true;
                } else if let Some((mods, name)) = pressed_keys(key) {
                    record_hotkey_keys(s, mods, &name);
                    if s.hotkey_target.is_none() {
                        s.swallow_keys = true;
                    } else {
                        note_held_keys(s, hwnd);
                    }
                } else {
                    note_held_keys(s, hwnd);
                }
                LRESULT(0)
            }
            WM_KEYUP | WM_SYSKEYUP if recording => {
                note_held_keys(s, hwnd);
                LRESULT(0)
            }
            WM_CHAR | WM_SYSCHAR if recording => LRESULT(0),
            // Space would otherwise jump to a row starting with a space.
            WM_CHAR if wparam.0 == 0x20 || wparam.0 == 0x0D || wparam.0 == 0x08 => LRESULT(0),
            WM_KEYDOWN => {
                let at = chosen();
                let cells = [SearchCell::Alias, SearchCell::Hotkey, SearchCell::Switch];
                let place = cells.iter().position(|cell| *cell == s.search_cell).unwrap_or(2);
                if key == VK_LEFT.0 || key == VK_RIGHT.0 {
                    let next = if key == VK_LEFT.0 { place.saturating_sub(1) } else { (place + 1).min(2) };
                    s.search_cell = cells[next];
                    set_focus_rings(s, true);
                    if at >= 0 {
                        redraw_search_row(s, at as usize);
                    }
                    return LRESULT(0);
                }
                if key == VK_UP.0 || key == VK_DOWN.0 {
                    let step: isize = if key == VK_DOWN.0 { 1 } else { -1 };
                    let from = at.max(0) + step;
                    let found = (from >= 0 && (from as usize) < s.search_rows.len())
                        .then(|| nearest_search_row(&s.search_rows, from as usize, step))
                        .flatten()
                        .filter(|next| (*next as isize - at) * step > 0);
                    match found {
                        Some(next) => {
                            SendMessageW(hwnd, LB_SETCURSEL, Some(WPARAM(next)), None);
                            reveal_list_row(s, hwnd);
                        }
                        None if step < 0 => {
                            let _ = SetFocus(Some(s.search_filter));
                        }
                        None => {}
                    }
                    return LRESULT(0);
                }
                if at >= 0 && (key == VK_RETURN.0 || key == VK_SPACE.0) {
                    act_on_search_cell(s, at as usize, s.search_cell);
                    return LRESULT(0);
                }
                if at >= 0 && key == VK_F2.0 {
                    act_on_search_cell(s, at as usize, SearchCell::Alias);
                    return LRESULT(0);
                }
                if at >= 0 && (key == VK_DELETE.0 || key == VK_BACK.0) {
                    match s.search_cell {
                        SearchCell::Alias => {
                            if let Some(SearchRow::Item(item)) = s.search_rows.get(at as usize).copied() {
                                let key = s.search_items[item].key.clone();
                                if !item_alias(&s.base, &key).is_empty() {
                                    set_item_alias(&mut s.base, &key, "");
                                    set_item_alias(&mut s.last_saved, &key, "");
                                    s.context.main.post(WM_ORBIT_SET_ALIAS, crate::util::box_for_post((key, String::new())), 0);
                                    show_status(s, "Alias removed", SAVED_MS);
                                    retext_search_row(s, at as usize);
                                }
                            }
                        }
                        SearchCell::Hotkey => remove_search_hotkey(s, at as usize),
                        SearchCell::Switch => {}
                    }
                    return LRESULT(0);
                }
                DefSubclassProc(hwnd, message, wparam, lparam)
            }
            WM_SETFOCUS | WM_KILLFOCUS => {
                let result = DefSubclassProc(hwnd, message, wparam, lparam);
                if message == WM_KILLFOCUS && recording {
                    stop_hotkey_record(s);
                } else if message == WM_SETFOCUS && chosen() < 0 {
                    if let Some(first) = nearest_search_row(&s.search_rows, 0, 1) {
                        SendMessageW(hwnd, LB_SETCURSEL, Some(WPARAM(first)), None);
                    }
                }
                let _ = InvalidateRect(Some(hwnd), None, false);
                result
            }
            WM_MOUSEMOVE => {
                let row = picker_row_at(hwnd, lparam);
                let hover = (row >= 0 && matches!(s.search_rows.get(row as usize), Some(SearchRow::Item(_)))).then(|| (row as usize, search_cell_at(s.style.dpi, width(), x_of(lparam))));
                if hover != s.search_hover {
                    for old in [s.search_hover.map(|h| h.0), hover.map(|h| h.0)].into_iter().flatten() {
                        redraw_search_row(s, old);
                    }
                    s.search_hover = hover;
                }
                if !s.search_tracking {
                    s.search_tracking = true;
                    let mut track = TRACKMOUSEEVENT { cbSize: std::mem::size_of::<TRACKMOUSEEVENT>() as u32, dwFlags: TME_LEAVE, hwndTrack: hwnd, dwHoverTime: 0 };
                    let _ = TrackMouseEvent(&mut track);
                }
                DefSubclassProc(hwnd, message, wparam, lparam)
            }
            WM_MOUSELEAVE => {
                s.search_tracking = false;
                if let Some((old, _)) = s.search_hover.take() {
                    redraw_search_row(s, old);
                }
                LRESULT(0)
            }
            WM_LBUTTONDOWN | WM_LBUTTONDBLCLK => {
                let row = picker_row_at(hwnd, lparam);
                if recording {
                    stop_hotkey_record(s);
                }
                // A heading can't be chosen, so the press never reaches the list.
                if !matches!(s.search_rows.get(row.max(0) as usize), Some(SearchRow::Item(_))) || row < 0 {
                    return LRESULT(0);
                }
                let result = DefSubclassProc(hwnd, message, wparam, lparam);
                if let Some(cell) = search_cell_at(s.style.dpi, width(), x_of(lparam)) {
                    act_on_search_cell(s, row as usize, cell);
                }
                result
            }
            _ => DefSubclassProc(hwnd, message, wparam, lparam),
        }
    }
}

/// The box for an alias, over the Search list: Enter keeps it, Esc puts it back, and Tab keeps it
/// and goes back to the list.
unsafe extern "system" fn alias_edit_proc(hwnd: HWND, message: u32, wparam: WPARAM, lparam: LPARAM, _id: usize, _data: usize) -> LRESULT {
    unsafe {
        if message == WM_NCDESTROY {
            let _ = RemoveWindowSubclass(hwnd, Some(alias_edit_proc), ALIAS_EDIT_SUBCLASS);
            return DefSubclassProc(hwnd, message, wparam, lparam);
        }
        let Some(s) = state(GetAncestor(hwnd, GA_ROOT)) else {
            return DefSubclassProc(hwnd, message, wparam, lparam);
        };
        let key = wparam.0 as u16;
        match message {
            WM_GETDLGCODE => LRESULT(DLGC_WANTALLKEYS as isize | DefSubclassProc(hwnd, message, wparam, lparam).0),
            WM_KEYDOWN if key == VK_RETURN.0 || key == VK_TAB.0 => {
                finish_alias_edit(s, true);
                LRESULT(0)
            }
            WM_KEYDOWN if key == VK_ESCAPE.0 => {
                finish_alias_edit(s, false);
                LRESULT(0)
            }
            // A space can't be in an alias, so it isn't typed.
            WM_CHAR if wparam.0 == 0x0D || wparam.0 == 0x1B || wparam.0 == 0x09 || wparam.0 == 0x20 => LRESULT(0),
            WM_KILLFOCUS => {
                let result = DefSubclassProc(hwnd, message, wparam, lparam);
                // Leaving with something that can't be used puts back what was there.
                if !finish_alias_edit(s, true) {
                    let problem = get_text(s.controls.search_note);
                    finish_alias_edit(s, false);
                    say_search_note(s, &problem, true);
                }
                result
            }
            _ => DefSubclassProc(hwnd, message, wparam, lparam),
        }
    }
}

/// One row of the hotkey list.
#[derive(Clone, Copy, PartialEq)]
enum HotkeyRow {
    HyperHeader,
    DirectHeader,
    Hyper(usize),
    Direct(usize),
    /// An option from the search.
    Match(usize),
    /// Something chosen from the search that's waiting for its keys.
    Pending,
}

impl HotkeyRow {
    fn is_header(self) -> bool {
        matches!(self, HotkeyRow::HyperHeader | HotkeyRow::DirectHeader)
    }

    /// What a row for an existing hotkey runs.
    fn binding_action(self, settings: &Settings) -> Option<Action> {
        match self {
            HotkeyRow::Hyper(i) => settings.hyper.bindings.get(i).map(|b| b.action.clone()),
            HotkeyRow::Direct(i) => settings.shortcuts.get(i).map(|b| b.action.clone()),
            _ => None,
        }
    }
}

/// What keys pressed in the hotkey list are for.
#[derive(Clone)]
enum RecordTarget {
    Row(HotkeyRow),
    New(Action),
}

struct State {
    context: EditorContext,
    base: Settings,
    pages: Vec<Vec<HWND>>,
    page: usize,
    list: HWND,
    status: HWND,
    style: Box<Style>,
    controls: Controls,
    dir: std::path::PathBuf,
    /// The launcher's shortcut as chosen here. `None` means the default.
    launcher_chord: Option<Chord>,
    /// What opens the launcher at the moment, which Orbit says.
    active_hotkey: String,
    /// Orbit's words for shortcuts it couldn't register.
    hotkey_problems: String,
    recording_launcher: bool,
    /// What the hotkey list is listening for, if it is.
    hotkey_target: Option<RecordTarget>,
    /// Why the last keys pressed for that can't be used, or what they'd replace.
    hotkey_problem: Option<String>,
    /// These keys are in use, and pressing them again takes them.
    hotkey_asking: Option<Hotkey>,
    /// The modifiers down while listening, as `Ctrl+Alt`, so that keys can be seen arriving.
    held_keys: String,
    /// The keys that finished a recording are still arriving, and mustn't do anything else.
    swallow_keys: bool,
    /// The list and row that a press landed on. A click needs the press and release to agree, so
    /// the second half of a double click, or a release left over from a closing dialog, does nothing.
    list_press: Option<(HWND, isize)>,
    /// The hotkey row whose Remove has been pressed once.
    hotkey_armed: Option<usize>,
    hotkey_hover: Option<(usize, ListPart)>,
    hotkey_tracking: bool,
    /// Says what a new hotkey took over from, once it's saved.
    replaced_note: Option<String>,
    /// A result the user is reading, and until when.
    result_until: Option<std::time::Instant>,
    action_options: Vec<ActionOption>,
    presets: Vec<Preset>,
    /// A switch, and what means nothing while it's off.
    dependents: Vec<(HWND, Vec<HWND>)>,
    /// A dangerous button pressed once, and what it said before.
    armed_button: Option<(HWND, &'static str)>,
    armed_at: std::time::Instant,
    /// How tall the Hotkeys page is as a list, and as text.
    hotkeys_list_bottom: i32,
    hotkeys_text_bottom: i32,
    /// The window's being dragged, and has reached a screen with a different DPI.
    moving: bool,
    dpi_change_waiting: bool,
    /// "Show hidden items again" was pressed, and that hasn't been saved yet.
    unhide_all: bool,
    /// What Orbit was last sent, so that only real changes are saved.
    last_saved: Settings,
    /// The last write failed, so saving tries again even with no new changes.
    save_pending: bool,
    /// The status line is saying why something can't be saved.
    showing_problem: bool,
    /// Closing has already warned about what can't be saved, so the next try closes.
    close_warned: bool,
    hotkey_rows: Vec<HotkeyRow>,
    /// The hotkeys are shown as editable text instead of a list.
    text_mode: bool,
    /// Shown only in text mode.
    text_controls: Vec<HWND>,
    /// Shown only in list mode.
    list_controls: Vec<HWND>,
    pickers: Vec<Picker>,
    candidates: Vec<Candidate>,
    /// The window pages scroll inside, and the page that moves within it.
    view: HWND,
    content: HWND,
    /// How tall each page's controls run, as laid out.
    page_heights: Vec<i32>,
    scroll: i32,
    /// Where on the thumb the pointer took hold of it, while dragging.
    thumb_grab: Option<i32>,
    /// The control the page last scrolled to show.
    last_focus: HWND,
    lists: Vec<ManagedList>,
    /// Which snippet each row of the snippet list is.
    snippet_ids: Vec<i64>,
    /// Scaled icons read so far. Freed with the window.
    icons: std::collections::HashMap<u64, Option<Vec<u8>>>,
    /// The Search page: its filter, its list, what it lists and the rows shown. The list is
    /// filled the first time the page is shown, so it costs nothing until then.
    search_filter: HWND,
    search_list: HWND,
    search_items: Vec<SearchItem>,
    search_rows: Vec<SearchRow>,
    search_filled: bool,
    /// The Search row under the pointer, and which of its parts.
    search_hover: Option<(usize, Option<SearchCell>)>,
    search_tracking: bool,
    /// The part of the chosen Search row that the keyboard acts on.
    search_cell: SearchCell,
    /// The box for typing an alias, and the row it's open on.
    alias_edit: HWND,
    alias_row: Option<usize>,
    /// The item whose hotkey is being listened for on the Search page.
    search_recording: Option<String>,
    /// The window's own Minimize and Close: which the pointer is over, and which is held down.
    caption_hover: Option<usize>,
    caption_press: Option<usize>,
    caption_tracking: bool,
}

fn state(hwnd: HWND) -> Option<&'static mut State> {
    unsafe { (GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *mut State).as_mut() }
}

const TRIGGERS: &[&str] = &[
    "CapsLock", "RightCtrl", "RightAlt", "RightSuper", "Menu", "F13", "F14", "F15", "F16", "F17", "F18", "F19", "F20",
    "F21", "F22", "F23", "F24", "F1", "F2", "F3", "F4", "F5", "F6", "F7", "F8", "F9", "F10", "F11", "F12",
];
const MODES: &[(&str, HyperMode)] = &[
    ("Orbit handles it", HyperMode::Internal),
    ("Also sends Ctrl+Alt+Shift+Win", HyperMode::Synthetic),
    ("Another remapper sends the chord", HyperMode::External),
];
const TAPS: &[(&str, TapBehavior)] = &[
    ("Sends Escape", TapBehavior::Escape),
    ("Keeps its normal behaviour", TapBehavior::Original),
    ("Does nothing", TapBehavior::None),
    ("Leader: tap, then a mapped key", TapBehavior::Leader),
];
const KEYBOARD: &[(&str, orbit_core::vim::KeyboardMode)] = &[
    ("Standard", orbit_core::vim::KeyboardMode::Standard),
    ("Vim keys with Alt", orbit_core::vim::KeyboardMode::VimKeys),
    ("Full Vim", orbit_core::vim::KeyboardMode::Vim),
];
const APPEARANCE: &[(&str, Appearance)] =
    &[("Match Windows", Appearance::System), ("Light", Appearance::Light), ("Dark", Appearance::Dark)];

fn show_page(state: &mut State, page: usize) {
    state.page = page;
    if page == PAGE_SEARCH && !state.search_filled {
        refresh_search_list(state);
    }
    for (i, controls) in state.pages.iter().enumerate() {
        for control in controls {
            let hidden_by_mode = if state.text_mode { state.list_controls.contains(control) } else { state.text_controls.contains(control) };
            let show = i == page && !hidden_by_mode;
            unsafe {
                let _ = ShowWindow(*control, if show { SW_SHOW } else { SW_HIDE });
            }
        }
    }
    lay_out_page(state);
    // Text box fields are painted by the page, so they change with it too.
    unsafe {
        let _ = InvalidateRect(Some(state.content), None, false);
    }
}

fn view_height(s: &State) -> i32 {
    let mut client = RECT::default();
    unsafe {
        let _ = GetClientRect(s.view, &mut client);
    }
    client.bottom
}

fn page_height(s: &State) -> i32 {
    s.page_heights.get(s.page).copied().unwrap_or(0)
}

/// Sizes the moving window to the page and goes back to its top.
fn lay_out_page(s: &mut State) {
    let view = view_height(s);
    s.scroll = 0;
    s.thumb_grab = None;
    let mut bounds = RECT::default();
    unsafe {
        let _ = GetWindowRect(s.content, &mut bounds);
        let _ = SetWindowPos(s.content, None, 0, 0, bounds.right - bounds.left, page_height(s).max(view), SWP_NOZORDER | SWP_NOACTIVATE);
        let _ = InvalidateRect(Some(s.view), None, false);
    }
}

fn scroll_to(s: &mut State, wanted: i32) {
    let scroll = forms::clamp_scroll(wanted, page_height(s), view_height(s));
    if scroll == s.scroll {
        return;
    }
    s.scroll = scroll;
    unsafe {
        let _ = SetWindowPos(s.content, None, 0, -scroll, 0, 0, SWP_NOSIZE | SWP_NOZORDER | SWP_NOACTIVATE);
        let _ = InvalidateRect(Some(s.view), None, false);
    }
}

/// Where the thumb is now, as top and height. `None` when the page fits.
fn thumb(s: &State) -> Option<(i32, i32)> {
    let px = |value: i32| forms::scale(value, s.style.dpi);
    forms::thumb_geometry(s.scroll, page_height(s), view_height(s), px(THUMB_INSET_DIP), px(THUMB_MIN_DIP))
}

/// Scrolls the page so this control can be seen, if it's on the page.
fn reveal_control(s: &mut State, control: HWND) {
    unsafe {
        if !IsChild(s.content, control).as_bool() {
            return;
        }
        let mut rect = RECT::default();
        let _ = GetWindowRect(control, &mut rect);
        let mut top_left = POINT { x: rect.left, y: rect.top };
        let _ = ScreenToClient(s.content, &mut top_left);
        let height = rect.bottom - rect.top;
        let margin = forms::scale(16, s.style.dpi);
        let wanted = forms::reveal_scroll(s.scroll, top_left.y, top_left.y + height, page_height(s), view_height(s), margin);
        scroll_to(s, wanted);
    }
}

/// Keyboard focus may have moved in a form. If it's in Settings and a Tab just took it out of
/// sight, this brings it back. A click leaves the page still, or the button would move under it.
pub fn focus_moved(dialog: HWND) {
    if CURRENT.with(|c| c.get()) != Some(dialog) {
        return;
    }
    let Some(s) = state(dialog) else { return };
    unsafe {
        let focus = GetFocus();
        if focus == s.last_focus {
            return;
        }
        s.last_focus = focus;
        if GetKeyState(VK_LBUTTON.0 as i32) >= 0 {
            reveal_control(s, focus);
            if is_row_list(s, focus) {
                reveal_list_row(s, focus);
            }
        }
    }
}

/// One of the lists on a page, as opposed to any other control.
fn is_row_list(s: &State, control: HWND) -> bool {
    control == s.controls.hotkey_list || control == s.search_list || s.lists.iter().any(|list| list.hwnd == control) || s.pickers.iter().any(|picker| picker.list == control)
}

/// Scrolls the page so the row that's selected in this list can be seen.
fn reveal_list_row(s: &mut State, list: HWND) {
    unsafe {
        let at = SendMessageW(list, LB_GETCURSEL, None, None).0;
        if at < 0 {
            return;
        }
        let mut row = RECT::default();
        if SendMessageW(list, LB_GETITEMRECT, Some(WPARAM(at as usize)), Some(LPARAM(&mut row as *mut RECT as isize))).0 < 0 {
            return;
        }
        let mut top_left = POINT { x: row.left, y: row.top };
        let _ = ClientToScreen(list, &mut top_left);
        let _ = ScreenToClient(s.content, &mut top_left);
        let margin = forms::scale(16, s.style.dpi);
        let wanted = forms::reveal_scroll(s.scroll, top_left.y, top_left.y + row.bottom - row.top, page_height(s), view_height(s), margin);
        scroll_to(s, wanted);
    }
}

/// The window pages scroll inside. All it draws is the scroll thumb, in its right-hand gutter.
unsafe extern "system" fn view_proc(hwnd: HWND, message: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    unsafe {
        let Some(s) = state(GetAncestor(hwnd, GA_ROOT)) else {
            return DefWindowProcW(hwnd, message, wparam, lparam);
        };
        let px = |value: i32| forms::scale(value, s.style.dpi);
        let y_of = |lparam: LPARAM| ((lparam.0 >> 16) & 0xFFFF) as i16 as i32;
        match message {
            WM_ERASEBKGND => LRESULT(1),
            WM_PAINT => {
                let mut ps = PAINTSTRUCT::default();
                let dc = BeginPaint(hwnd, &mut ps);
                let mut client = RECT::default();
                let _ = GetClientRect(hwnd, &mut client);
                let gutter = px(SCROLL_GUTTER_DIP);
                if let Some(mut buffer) = Backbuffer::new(gutter, client.bottom) {
                    let theme = s.style.theme;
                    buffer.fill(theme.sidebar);
                    if let Some((top, height)) = thumb(s) {
                        let width = px(THUMB_WIDTH_DIP);
                        let color = if s.thumb_grab.is_some() { theme.secondary } else { theme.tertiary };
                        buffer.fill_round_rect((gutter - width) / 2, top, width, height, width as f32 / 2.0, color);
                    }
                    buffer.copy_to_dc_at(dc, client.right - gutter, 0);
                }
                let _ = EndPaint(hwnd, &ps);
                LRESULT(0)
            }
            WM_MOUSEWHEEL => {
                let delta = ((wparam.0 >> 16) & 0xFFFF) as i16 as i32;
                let mut lines = 3u32;
                let _ = SystemParametersInfoW(SPI_GETWHEELSCROLLLINES, 0, Some(&mut lines as *mut u32 as *mut _), SYSTEM_PARAMETERS_INFO_UPDATE_FLAGS(0));
                // "One screen at a time" comes through as a huge number.
                let line_count = if lines > 100 { 15 } else { lines as i32 };
                let wanted = s.scroll - forms::wheel_pixels(delta, line_count, px(WHEEL_LINE_DIP));
                scroll_to(s, wanted);
                LRESULT(0)
            }
            WM_LBUTTONDOWN => {
                if let Some((top, height)) = thumb(s) {
                    let y = y_of(lparam);
                    // A press off the thumb jumps there, ready to carry on dragging.
                    let grab = if y >= top && y < top + height { y - top } else { height / 2 };
                    s.thumb_grab = Some(grab);
                    SetCapture(hwnd);
                    let wanted = forms::scroll_for_thumb(y - grab, page_height(s), view_height(s), px(THUMB_INSET_DIP), px(THUMB_MIN_DIP));
                    scroll_to(s, wanted);
                    let _ = InvalidateRect(Some(hwnd), None, false);
                }
                LRESULT(0)
            }
            WM_MOUSEMOVE => {
                if let Some(grab) = s.thumb_grab {
                    let wanted = forms::scroll_for_thumb(y_of(lparam) - grab, page_height(s), view_height(s), px(THUMB_INSET_DIP), px(THUMB_MIN_DIP));
                    scroll_to(s, wanted);
                }
                LRESULT(0)
            }
            WM_LBUTTONUP => {
                if s.thumb_grab.is_some() {
                    let _ = ReleaseCapture();
                }
                LRESULT(0)
            }
            WM_CAPTURECHANGED => {
                s.thumb_grab = None;
                let _ = InvalidateRect(Some(hwnd), None, false);
                LRESULT(0)
            }
            _ => DefWindowProcW(hwnd, message, wparam, lparam),
        }
    }
}

/// The page itself. It paints the cards and fields, and passes everything else from its controls
/// up to the main window, so that handling doesn't care which window a control sits on.
unsafe extern "system" fn content_proc(hwnd: HWND, message: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    unsafe {
        match message {
            WM_ERASEBKGND => LRESULT(1),
            WM_PAINT => {
                match state(GetAncestor(hwnd, GA_ROOT)) {
                    Some(s) => s.style.paint(hwnd, |_| {}),
                    None => {
                        let mut ps = PAINTSTRUCT::default();
                        let _ = BeginPaint(hwnd, &mut ps);
                        let _ = EndPaint(hwnd, &ps);
                    }
                }
                LRESULT(0)
            }
            WM_COMMAND | WM_NOTIFY | WM_DRAWITEM | WM_MEASUREITEM | WM_VKEYTOITEM | WM_CTLCOLORSTATIC | WM_CTLCOLORBTN | WM_CTLCOLOREDIT | WM_CTLCOLORLISTBOX => {
                SendMessageW(GetAncestor(hwnd, GA_ROOT), message, Some(wparam), Some(lparam))
            }
            WM_LBUTTONDOWN => {
                if let Some(s) = state(GetAncestor(hwnd, GA_ROOT)) {
                    clicked_away(s);
                }
                LRESULT(0)
            }
            _ => DefWindowProcW(hwnd, message, wparam, lparam),
        }
    }
}

fn register_page_classes(module: HMODULE) {
    let cursor = unsafe { LoadCursorW(None, IDC_ARROW) }.unwrap_or_default();
    for (name, proc) in [
        (w!("OrbitSettingsView"), view_proc as unsafe extern "system" fn(HWND, u32, WPARAM, LPARAM) -> LRESULT),
        (w!("OrbitSettingsPage"), content_proc),
        (w!("OrbitRecorder"), recorder_proc),
    ] {
        let class = WNDCLASSEXW {
            cbSize: std::mem::size_of::<WNDCLASSEXW>() as u32,
            lpfnWndProc: Some(proc),
            hInstance: module.into(),
            hCursor: cursor,
            lpszClassName: name,
            ..Default::default()
        };
        unsafe {
            RegisterClassExW(&class);
        }
    }
}

/// Quicklink rows, with each site's icon when Orbit has it and `icons` says where they're kept.
fn link_rows(links: &[orbit_core::settings::QuickLink], icons: Option<&std::path::Path>) -> Vec<ManagedRow> {
    links
        .iter()
        .map(|link| {
            let picture = icons
                .zip(crate::favicon::link_key(&link.url))
                .and_then(|(dir, key)| std::fs::read(crate::favicon::icon_file(dir, key)).ok())
                .filter(|pixels| pixels.len() == crate::indexer::ICON_BYTES);
            ManagedRow { tile: forms::letter_tile(&link.name), title: link.name.clone(), pill: link.alias.clone(), detail: link.url.clone(), picture }
        })
        .collect()
}

/// Quicklink icons have arrived, or gone. Settings shows them on its list too.
pub fn link_icons_changed(enabled: bool) {
    let Some(s) = CURRENT.with(|c| c.get()).and_then(state) else { return };
    let dir = s.dir.clone();
    let rows = link_rows(&s.base.links, enabled.then_some(dir.as_path()));
    fill_managed(s, LIST_LINKS, rows);
}

fn command_rows(commands: &[orbit_core::settings::CustomCommand]) -> Vec<ManagedRow> {
    commands
        .iter()
        .map(|command| {
            let detail = if command.arguments.is_empty() { command.program.clone() } else { format!("{} {}", command.program, command.arguments) };
            ManagedRow { tile: Tile::Glyph(GLYPH_COMMAND), title: command.name.clone(), pill: command.alias.clone(), detail, picture: None }
        })
        .collect()
}

/// Only snippets make rows here, so the ids line up with what the list shows.
fn snippet_rows(rows: Vec<DataRow>) -> (Vec<ManagedRow>, Vec<i64>) {
    let mut out = Vec::with_capacity(rows.len());
    let mut ids = Vec::with_capacity(rows.len());
    for row in rows {
        if let DataRow::Snippet { id, name, trigger, preview } = row {
            ids.push(id);
            out.push(ManagedRow { tile: Tile::Glyph(GLYPH_SNIPPET), title: name, pill: trigger, detail: preview, picture: None });
        }
    }
    (out, ids)
}

fn fill_managed(s: &mut State, index: usize, rows: Vec<ManagedRow>) {
    let list = &mut s.lists[index];
    list.armed = None;
    list.hover = None;
    list.rows = rows;
    unsafe {
        let chosen = SendMessageW(list.hwnd, LB_GETCURSEL, None, None).0;
        SendMessageW(list.hwnd, LB_RESETCONTENT, None, None);
        for row in &list.rows {
            // What a screen reader says: the same things the row shows.
            let text = wide(&format!("{}, {}, {}", row.title, row.pill, row.detail));
            SendMessageW(list.hwnd, LB_ADDSTRING, None, Some(LPARAM(text.as_ptr() as isize)));
        }
        if chosen >= 0 && !list.rows.is_empty() {
            let at = (chosen as usize).min(list.rows.len() - 1);
            SendMessageW(list.hwnd, LB_SETCURSEL, Some(WPARAM(at)), None);
        }
        let _ = InvalidateRect(Some(list.hwnd), None, true);
    }
    // The list is as long as what's in it, and the page scrolls.
    let (hwnd, rows) = (list.hwnd, list.rows.len().max(1) as i32);
    let height = forms::scale(forms::MANAGED_ROW_DIP * rows, s.style.dpi);
    resize_on_page(s, hwnd, height);
}

/// The launcher saved a link or a command. Show it, and keep it in what's saved from here.
pub fn links_changed(settings: &Settings) {
    let Some(hwnd) = CURRENT.with(|c| c.get()) else { return };
    let Some(s) = state(hwnd) else { return };
    s.base.links = settings.links.clone();
    s.base.custom_commands = settings.custom_commands.clone();
    s.last_saved.links = settings.links.clone();
    s.last_saved.custom_commands = settings.custom_commands.clone();
    let dir = s.dir.clone();
    fill_managed(s, LIST_LINKS, link_rows(&settings.links, settings.fetch_link_icons.then_some(dir.as_path())));
    fill_managed(s, LIST_COMMANDS, command_rows(&settings.custom_commands));
    refresh_hotkey_list(s);
    // The Search page lists them too.
    s.search_items.retain(|item| item.group != GROUP_LINKS && item.group != GROUP_COMMANDS);
    s.search_items.extend(own_search_items(settings));
    sort_search_items(&mut s.search_items);
    if s.search_filled {
        refresh_search_list(s);
    }
}

/// An alias was given in the launcher.
pub fn aliases_changed(settings: &Settings) {
    let Some(hwnd) = CURRENT.with(|c| c.get()) else { return };
    let Some(s) = state(hwnd) else { return };
    s.base.aliases = settings.aliases.clone();
    s.last_saved.aliases = settings.aliases.clone();
    if s.search_filled {
        unsafe {
            let _ = InvalidateRect(Some(s.search_list), None, false);
        }
    }
}

/// Snippets may have changed. If Settings is showing them, it asks for the list again.
pub fn snippets_changed() {
    let Some(hwnd) = CURRENT.with(|c| c.get()) else { return };
    if let Some(s) = state(hwnd) {
        let _ = s.context.tx.send(Req::SnippetList);
    }
}

/// The data thread's answer to `Req::SnippetList`.
pub fn snippets_listed(rows: Vec<DataRow>) {
    let Some(hwnd) = CURRENT.with(|c| c.get()) else { return };
    let Some(s) = state(hwnd) else { return };
    let (rows, ids) = snippet_rows(rows);
    s.snippet_ids = ids;
    fill_managed(s, LIST_SNIPPETS, rows);
}

#[cfg(debug_assertions)]
fn snippet_context(s: &State) -> EditorContext {
    EditorContext { tx: s.context.tx.clone(), main: s.context.main, theme: s.context.theme }
}

fn request_link_action(s: &State, request: LinkRequest) {
    s.context.main.post(WM_ORBIT_LINK_REQUEST, crate::util::box_for_post(request), 0);
}

/// Opens the form for this row.
fn edit_managed_row(s: &mut State, index: usize, row: usize) {
    match index {
        LIST_SNIPPETS => {
            if let Some(id) = s.snippet_ids.get(row) {
                let _ = s.context.tx.send(Req::SnippetGet { id: *id, purpose: crate::worker::Purpose::Edit });
            }
        }
        LIST_LINKS => {
            if let Some(link) = s.base.links.get(row) {
                request_link_action(s, LinkRequest::EditLink(link.id.clone()));
            }
        }
        _ => {
            if let Some(command) = s.base.custom_commands.get(row) {
                request_link_action(s, LinkRequest::EditCommand(command.id.clone()));
            }
        }
    }
}

/// Delete was pressed on this row: arms it the first time, and goes ahead the second.
fn delete_managed_row(s: &mut State, index: usize, row: usize) {
    if row >= s.lists[index].rows.len() {
        return;
    }
    if !forms::arm_or_confirm(&mut s.lists[index].armed, row) {
        unsafe {
            let _ = InvalidateRect(Some(s.lists[index].hwnd), None, false);
        }
        return;
    }
    match index {
        LIST_SNIPPETS => {
            if let Some(id) = s.snippet_ids.get(row) {
                let _ = s.context.tx.send(Req::SnippetDelete { id: *id });
            }
        }
        LIST_LINKS => request_link_action(s, LinkRequest::DeleteLink(s.base.links[row].id.clone())),
        _ => request_link_action(s, LinkRequest::DeleteCommand(s.base.custom_commands[row].id.clone())),
    }
    unsafe {
        let _ = InvalidateRect(Some(s.lists[index].hwnd), None, false);
    }
}

fn disarm(list: &mut ManagedList) {
    if list.armed.take().is_some() {
        unsafe {
            let _ = InvalidateRect(Some(list.hwnd), None, false);
        }
    }
}

/// Mouse and keyboard for the hotkey list, which can also be listening for keys.
unsafe extern "system" fn hotkey_list_proc(hwnd: HWND, message: u32, wparam: WPARAM, lparam: LPARAM, _id: usize, _data: usize) -> LRESULT {
    unsafe {
        if message == WM_NCDESTROY {
            let _ = RemoveWindowSubclass(hwnd, Some(hotkey_list_proc), HOTKEY_LIST_SUBCLASS);
            return DefSubclassProc(hwnd, message, wparam, lparam);
        }
        let Some(s) = state(GetAncestor(hwnd, GA_ROOT)) else {
            return DefSubclassProc(hwnd, message, wparam, lparam);
        };
        let key = wparam.0 as u16;
        let x_of = |lparam: LPARAM| (lparam.0 & 0xFFFF) as i16 as i32;
        let width = || {
            let mut client = RECT::default();
            let _ = GetClientRect(hwnd, &mut client);
            client.right
        };
        let recording = s.hotkey_target.is_some();
        if s.swallow_keys && swallow_after_recording(s, message, lparam) {
            return LRESULT(0);
        }
        match message {
            WM_PAINT if s.hotkey_rows.is_empty() => {
                let searching = !get_text(s.controls.action_search).trim().is_empty();
                s.style.paint_empty_list(hwnd, if searching { "Nothing matches that." } else { NO_HOTKEYS });
                return LRESULT(0);
            }
            WM_ERASEBKGND if s.hotkey_rows.is_empty() => return LRESULT(1),
            WM_GETDLGCODE => {
                if recording {
                    return LRESULT(DLGC_WANTALLKEYS as isize);
                }
                let base = DefSubclassProc(hwnd, message, wparam, lparam);
                let wanted = (lparam.0 as *const MSG).as_ref().is_some_and(|m| {
                    let key = m.wParam.0 as u16;
                    let in_results = matches!(s.hotkey_rows.first(), Some(HotkeyRow::Match(_)));
                    m.message == WM_KEYDOWN && !s.hotkey_rows.is_empty() && (key == VK_RETURN.0 || key == VK_DELETE.0 || (key == VK_ESCAPE.0 && (s.hotkey_armed.is_some() || in_results)))
                });
                return if wanted { LRESULT(base.0 | DLGC_WANTMESSAGE as isize) } else { base };
            }
            WM_KEYDOWN | WM_SYSKEYDOWN if recording => {
                // Still the Enter that started this, or the keys that were just refused.
                if is_repeat(lparam) {
                    return LRESULT(0);
                }
                if key == VK_ESCAPE.0 {
                    stop_hotkey_record(s);
                    s.swallow_keys = true;
                } else if let Some((mods, name)) = pressed_keys(key) {
                    record_hotkey_keys(s, mods, &name);
                    if s.hotkey_target.is_none() {
                        s.swallow_keys = true;
                    } else {
                        note_held_keys(s, hwnd);
                    }
                } else {
                    note_held_keys(s, hwnd);
                }
                return LRESULT(0);
            }
            WM_KEYUP | WM_SYSKEYUP if recording => {
                note_held_keys(s, hwnd);
                return LRESULT(0);
            }
            WM_CHAR | WM_SYSCHAR if recording => return LRESULT(0),
            WM_CHAR if key == 13 || key == 27 => return LRESULT(0),
            WM_KEYDOWN => {
                let at = SendMessageW(hwnd, LB_GETCURSEL, None, None).0;
                if key == VK_RETURN.0 && at >= 0 {
                    act_on_hotkey_row(s, at as usize);
                    return LRESULT(0);
                }
                if key == VK_DELETE.0 && at >= 0 {
                    // Holding it down isn't pressing it twice.
                    if !is_repeat(lparam) {
                        delete_hotkey_at(s, at as usize);
                    }
                    return LRESULT(0);
                }
                let was_armed = s.hotkey_armed.take().is_some();
                if was_armed {
                    let _ = InvalidateRect(Some(hwnd), None, false);
                }
                if key == VK_ESCAPE.0 && was_armed {
                    return LRESULT(0);
                }
                // Out of the search results, back to the search, rather than out of Settings.
                if key == VK_ESCAPE.0 && matches!(s.hotkey_rows.first(), Some(HotkeyRow::Match(_))) {
                    set_text(s.controls.action_search, "");
                    let _ = SetFocus(Some(s.controls.action_search));
                    return LRESULT(0);
                }
                if key == VK_DOWN.0 || key == VK_UP.0 {
                    let step: isize = if key == VK_DOWN.0 { 1 } else { -1 };
                    let from = at.max(0) + step;
                    let found = if from >= 0 && (from as usize) < s.hotkey_rows.len() {
                        // Past a heading, but not back the other way at the end of the list.
                        nearest_hotkey_row(&s.hotkey_rows, from as usize, step).filter(|next| (*next as isize - at) * step > 0)
                    } else {
                        None
                    };
                    match found {
                        Some(next) => {
                            SendMessageW(hwnd, LB_SETCURSEL, Some(WPARAM(next)), None);
                            reveal_list_row(s, hwnd);
                        }
                        None if step < 0 => {
                            let _ = SetFocus(Some(s.controls.action_search));
                        }
                        None => {}
                    }
                    return LRESULT(0);
                }
            }
            WM_SETFOCUS | WM_KILLFOCUS => {
                let result = DefSubclassProc(hwnd, message, wparam, lparam);
                if message == WM_KILLFOCUS {
                    s.hotkey_armed = None;
                    stop_hotkey_record(s);
                } else if SendMessageW(hwnd, LB_GETCURSEL, None, None).0 < 0 {
                    if let Some(first) = nearest_hotkey_row(&s.hotkey_rows, 0, 1) {
                        SendMessageW(hwnd, LB_SETCURSEL, Some(WPARAM(first)), None);
                    }
                }
                let _ = InvalidateRect(Some(hwnd), None, false);
                return result;
            }
            WM_MOUSEMOVE => {
                let row = picker_row_at(hwnd, lparam);
                let hover = (row >= 0 && (row as usize) < s.hotkey_rows.len()).then(|| (row as usize, hotkey_part_at(s, row as usize, x_of(lparam), width())));
                if hover != s.hotkey_hover {
                    if s.hotkey_armed.is_some() && hover.map(|h| h.0) != s.hotkey_armed {
                        s.hotkey_armed = None;
                    }
                    s.hotkey_hover = hover;
                    let _ = InvalidateRect(Some(hwnd), None, false);
                }
                if !s.hotkey_tracking {
                    s.hotkey_tracking = true;
                    let mut track = TRACKMOUSEEVENT { cbSize: std::mem::size_of::<TRACKMOUSEEVENT>() as u32, dwFlags: TME_LEAVE, hwndTrack: hwnd, dwHoverTime: 0 };
                    let _ = TrackMouseEvent(&mut track);
                }
            }
            WM_MOUSELEAVE => {
                s.hotkey_tracking = false;
                s.hotkey_hover = None;
                s.hotkey_armed = None;
                let _ = InvalidateRect(Some(hwnd), None, false);
            }
            // A click anywhere else in the list takes back the listening.
            WM_LBUTTONDOWN if recording => stop_hotkey_record(s),
            // A heading can't be chosen, so the press never reaches the list.
            WM_LBUTTONDOWN => {
                let row = picker_row_at(hwnd, lparam);
                s.list_press = Some((hwnd, row));
                if s.hotkey_rows.get(row.max(0) as usize).is_some_and(|row| row.is_header()) {
                    return LRESULT(0);
                }
            }
            WM_LBUTTONUP if !recording => {
                let result = DefSubclassProc(hwnd, message, wparam, lparam);
                let row = picker_row_at(hwnd, lparam);
                if row >= 0 && s.list_press.take() == Some((hwnd, row)) {
                    let at = row as usize;
                    match s.hotkey_rows.get(at).copied() {
                        // The key caps are what's being changed, so a click on them changes them.
                        Some(HotkeyRow::Hyper(_) | HotkeyRow::Direct(_)) => match hotkey_part_at(s, at, x_of(lparam), width()) {
                            ListPart::Edit => act_on_hotkey_row(s, at),
                            ListPart::Delete => delete_hotkey_at(s, at),
                            ListPart::Body => {}
                        },
                        Some(_) => act_on_hotkey_row(s, at),
                        None => {}
                    }
                }
                return result;
            }
            WM_LBUTTONDBLCLK if !recording => {
                let result = DefSubclassProc(hwnd, message, wparam, lparam);
                let row = picker_row_at(hwnd, lparam);
                if row >= 0 && hotkey_part_at(s, row as usize, x_of(lparam), width()) == ListPart::Body {
                    if matches!(s.hotkey_rows.get(row as usize), Some(HotkeyRow::Hyper(_) | HotkeyRow::Direct(_))) {
                        act_on_hotkey_row(s, row as usize);
                    }
                }
                return result;
            }
            _ => {}
        }
        DefSubclassProc(hwnd, message, wparam, lparam)
    }
}

/// Mouse and keyboard for a managed list. `data` is which list.
unsafe extern "system" fn managed_proc(hwnd: HWND, message: u32, wparam: WPARAM, lparam: LPARAM, _id: usize, data: usize) -> LRESULT {
    unsafe {
        if message == WM_NCDESTROY {
            let _ = RemoveWindowSubclass(hwnd, Some(managed_proc), data);
            return DefSubclassProc(hwnd, message, wparam, lparam);
        }
        let Some(s) = state(GetAncestor(hwnd, GA_ROOT)) else {
            return DefSubclassProc(hwnd, message, wparam, lparam);
        };
        let Some(list) = s.lists.get_mut(data) else {
            return DefSubclassProc(hwnd, message, wparam, lparam);
        };
        let key = wparam.0 as u16;
        let x_of = |lparam: LPARAM| (lparam.0 & 0xFFFF) as i16 as i32;
        let width = || {
            let mut client = RECT::default();
            let _ = GetClientRect(hwnd, &mut client);
            client.right
        };
        let dpi = s.style.dpi;
        match message {
            WM_PAINT if list.rows.is_empty() => {
                s.style.paint_empty_list(hwnd, list.empty);
                return LRESULT(0);
            }
            WM_ERASEBKGND if list.rows.is_empty() => return LRESULT(1),
            WM_SETFOCUS | WM_KILLFOCUS => {
                let result = DefSubclassProc(hwnd, message, wparam, lparam);
                if message == WM_KILLFOCUS {
                    list.armed = None;
                } else if SendMessageW(hwnd, LB_GETCURSEL, None, None).0 < 0 && !list.rows.is_empty() {
                    SendMessageW(hwnd, LB_SETCURSEL, Some(WPARAM(0)), None);
                }
                let _ = InvalidateRect(Some(hwnd), None, false);
                return result;
            }
            // Enter, Delete and Esc would otherwise go to the window's buttons.
            WM_GETDLGCODE => {
                let base = DefSubclassProc(hwnd, message, wparam, lparam);
                let wanted = (lparam.0 as *const MSG).as_ref().is_some_and(|m| {
                    let key = m.wParam.0 as u16;
                    m.message == WM_KEYDOWN && !list.rows.is_empty() && (key == VK_RETURN.0 || key == VK_DELETE.0 || (key == VK_ESCAPE.0 && list.armed.is_some()))
                });
                return if wanted { LRESULT(base.0 | DLGC_WANTMESSAGE as isize) } else { base };
            }
            WM_CHAR if key == 13 || key == 27 => return LRESULT(0),
            WM_KEYDOWN => {
                let row = SendMessageW(hwnd, LB_GETCURSEL, None, None).0;
                if key == VK_RETURN.0 && row >= 0 {
                    edit_managed_row(s, data, row as usize);
                    return LRESULT(0);
                }
                if key == VK_DELETE.0 && row >= 0 {
                    // Holding it down isn't pressing it twice.
                    if !is_repeat(lparam) {
                        delete_managed_row(s, data, row as usize);
                    }
                    return LRESULT(0);
                }
                let was_armed = list.armed.is_some();
                disarm(list);
                if key == VK_ESCAPE.0 && was_armed {
                    return LRESULT(0);
                }
            }
            WM_MOUSEMOVE => {
                let row = picker_row_at(hwnd, lparam);
                let hover = (row >= 0 && (row as usize) < list.rows.len()).then(|| (row as usize, forms::managed_part_at(x_of(lparam), width(), dpi)));
                if hover != list.hover {
                    // Moving off a row or its button takes back a Delete pressed once.
                    if list.armed.is_some() && hover.map(|h| h.0) != list.armed {
                        list.armed = None;
                    }
                    list.hover = hover;
                    let _ = InvalidateRect(Some(hwnd), None, false);
                }
                if !list.tracking {
                    list.tracking = true;
                    let mut track = TRACKMOUSEEVENT { cbSize: std::mem::size_of::<TRACKMOUSEEVENT>() as u32, dwFlags: TME_LEAVE, hwndTrack: hwnd, dwHoverTime: 0 };
                    let _ = TrackMouseEvent(&mut track);
                }
            }
            WM_MOUSELEAVE => {
                list.tracking = false;
                list.hover = None;
                list.armed = None;
                let _ = InvalidateRect(Some(hwnd), None, false);
            }
            WM_LBUTTONDOWN => s.list_press = Some((hwnd, picker_row_at(hwnd, lparam))),
            WM_LBUTTONUP => {
                let result = DefSubclassProc(hwnd, message, wparam, lparam);
                let row = picker_row_at(hwnd, lparam);
                if row >= 0 && (row as usize) < list.rows.len() && s.list_press.take() == Some((hwnd, row)) {
                    match forms::managed_part_at(x_of(lparam), width(), dpi) {
                        ListPart::Edit => edit_managed_row(s, data, row as usize),
                        ListPart::Delete => delete_managed_row(s, data, row as usize),
                        ListPart::Body => disarm(list),
                    }
                }
                return result;
            }
            WM_LBUTTONDBLCLK => {
                let result = DefSubclassProc(hwnd, message, wparam, lparam);
                let row = picker_row_at(hwnd, lparam);
                if row >= 0 && (row as usize) < list.rows.len() && forms::managed_part_at(x_of(lparam), width(), dpi) == ListPart::Body {
                    edit_managed_row(s, data, row as usize);
                }
                return result;
            }
            _ => {}
        }
        DefSubclassProc(hwnd, message, wparam, lparam)
    }
}

fn draw_managed_item(s: &State, index: usize, item: &DRAWITEMSTRUCT) {
    let Some(list) = s.lists.get(index) else { return };
    let Some(row) = list.rows.get(item.itemID as usize) else { return };
    let selected = item.itemState.0 & ODS_SELECTED.0 != 0 && unsafe { GetFocus() } == list.hwnd;
    let hover = list.hover.filter(|h| h.0 == item.itemID as usize).map(|h| h.1);
    s.style.draw_managed_row(item, row, list.armed == Some(item.itemID as usize), hover, selected);
}

/// A card with a title, a New button and a managed list.
fn build_managed(b: &mut Builder, title: &str, note: Option<&str>, new_id: i32, list_id: i32) -> HWND {
    b.begin_card();
    b.card_header(title, "New", new_id);
    if let Some(note) = note {
        b.note(note);
    }
    let height = b.px(forms::MANAGED_ROW_DIP);
    let list = b.owner_list((b.x, b.y, b.width, height), 0, list_id);
    b.skip(height);
    b.end_card();
    list
}

fn select_page(state: &mut State, page: usize) {
    unsafe {
        SendMessageW(state.list, LB_SETCURSEL, Some(WPARAM(page)), None);
    }
    show_page(state, page);
}

/// One thing a new hotkey can run.
struct ActionOption {
    name: String,
    kind: &'static str,
    action: Action,
}

/// Everything the Hotkeys page can offer. Built when Settings opens, and freed with it.
fn action_options(apps: &[crate::indexer::AppEntry], settings: &Settings) -> Vec<ActionOption> {
    let mut out = Vec::with_capacity(apps.len() + Builtin::ALL.len());
    for app in apps {
        out.push(ActionOption { name: app.name.clone(), kind: "App", action: Action::FocusOrLaunchApp { id: app.id.clone(), name: app.name.clone() } });
    }
    for entry in Builtin::ALL {
        // What asks first in the launcher shouldn't be a key away.
        if !entry.0.needs_confirmation() {
            out.push(ActionOption { name: entry.2.to_string(), kind: "Orbit", action: Action::Builtin { command: entry.0 } });
        }
    }
    for link in &settings.links {
        out.push(ActionOption { name: link.name.clone(), kind: "Link", action: Action::Url { url: link.url.clone() } });
    }
    for command in &settings.custom_commands {
        out.push(ActionOption { name: command.name.clone(), kind: "Command", action: Action::Custom { id: command.id.clone() } });
    }
    out
}

/// Which options match, best first: the start of the name, then the start of a word, then anywhere.
fn matching_actions(options: &[ActionOption], query: &str) -> Vec<usize> {
    let query = query.trim().to_lowercase();
    let mut found: Vec<(u8, String, usize)> = options
        .iter()
        .enumerate()
        .filter_map(|(at, option)| {
            let name = option.name.to_lowercase();
            let score = if name.starts_with(&query) {
                0
            } else if name.split(|c: char| !c.is_alphanumeric()).any(|word| word.starts_with(&query)) {
                1
            } else if name.contains(&query) {
                2
            } else {
                return None;
            };
            Some((score, name, at))
        })
        .collect();
    found.sort();
    found.truncate(100);
    found.into_iter().map(|entry| entry.2).collect()
}

/// Fills the hotkey list from the bindings being edited, or from what's being searched for, and
/// sizes it to fit.
fn refresh_hotkey_list(state: &mut State) {
    let list = state.controls.hotkey_list;
    let chosen = unsafe { SendMessageW(list, LB_GETCURSEL, None, None) }.0;
    unsafe {
        SendMessageW(list, LB_RESETCONTENT, None, None);
    }
    state.hotkey_rows.clear();
    state.hotkey_armed = None;
    state.hotkey_hover = None;
    let query = get_text(state.controls.action_search);
    let searching = !query.trim().is_empty();
    let mut texts: Vec<String> = Vec::new();
    if searching {
        for at in matching_actions(&state.action_options, &query) {
            let option = &state.action_options[at];
            texts.push(format!("{}, {}", option.name, option.kind));
            state.hotkey_rows.push(HotkeyRow::Match(at));
        }
    } else {
        let hyper_on = is_checked(state.controls.hyper_on);
        let (has_hyper, has_direct) = (!state.base.hyper.bindings.is_empty(), !state.base.shortcuts.is_empty());
        if has_hyper && (has_direct || !hyper_on) {
            texts.push(if hyper_on { "HyperKey".to_string() } else { HYPER_OFF_ROW.to_string() });
            state.hotkey_rows.push(HotkeyRow::HyperHeader);
        }
        for (i, binding) in state.base.hyper.bindings.iter().enumerate() {
            texts.push(format!("Hyper+{}, {}", binding.key, hotkeys::describe(&state.base, &binding.action)));
            state.hotkey_rows.push(HotkeyRow::Hyper(i));
        }
        if has_direct && has_hyper {
            texts.push("Shortcuts".to_string());
            state.hotkey_rows.push(HotkeyRow::DirectHeader);
        }
        for (i, binding) in state.base.shortcuts.iter().enumerate() {
            texts.push(format!("{}, {}", binding.chord.display("Win"), hotkeys::describe(&state.base, &binding.action)));
            state.hotkey_rows.push(HotkeyRow::Direct(i));
        }
        if let Some(RecordTarget::New(action)) = &state.hotkey_target {
            texts.push(format!("{}, press keys", hotkeys::describe(&state.base, action)));
            state.hotkey_rows.push(HotkeyRow::Pending);
        }
    }
    unsafe {
        for text in &texts {
            let text = wide(text);
            SendMessageW(list, LB_ADDSTRING, None, Some(LPARAM(text.as_ptr() as isize)));
        }
    }
    let at = if state.hotkey_target.is_some() {
        state.hotkey_rows.iter().position(|row| match &state.hotkey_target {
            Some(RecordTarget::New(_)) => matches!(row, HotkeyRow::Pending),
            Some(RecordTarget::Row(target)) => target == row,
            None => false,
        })
    } else if searching {
        (!state.hotkey_rows.is_empty()).then_some(0)
    } else if chosen >= 0 {
        nearest_hotkey_row(&state.hotkey_rows, (chosen as usize).min(state.hotkey_rows.len().saturating_sub(1)), 1)
    } else {
        None
    };
    if let Some(at) = at {
        unsafe {
            SendMessageW(list, LB_SETCURSEL, Some(WPARAM(at)), None);
        }
    }
    // A search scrolls in place, so that the page doesn't jump about while typing.
    let shown = if searching { state.hotkey_rows.len().clamp(1, SEARCH_ROWS) } else { state.hotkey_rows.len().max(1) };
    let height = forms::scale(HOTKEY_ROW_DIP * shown as i32, state.style.dpi);
    resize_on_page(state, list, height);
    unsafe {
        let _ = InvalidateRect(Some(list), None, true);
        let _ = EnableWindow(state.controls.suggest, !suggestions(&state.base).is_empty());
        let _ = InvalidateRect(Some(state.controls.suggest), None, false);
    }
    // The page follows what's happening in the list.
    if state.page == PAGE_HOTKEYS {
        if searching {
            reveal_control(state, list);
        } else if state.hotkey_target.is_some() || unsafe { GetFocus() } == list {
            reveal_list_row(state, list);
        }
    }
}

/// The keys a row of the list shows.
fn hotkey_row_keys(s: &State, row: HotkeyRow) -> Option<String> {
    match row {
        HotkeyRow::Hyper(i) => s.base.hyper.bindings.get(i).map(|binding| format!("Hyper+{}", binding.key)),
        HotkeyRow::Direct(i) => s.base.shortcuts.get(i).map(|binding| binding.chord.display("Win")),
        _ => None,
    }
}

/// Which part of a hotkey row a point is on. `Edit` is the key caps.
fn hotkey_part_at(s: &State, at: usize, x: i32, width: i32) -> ListPart {
    let px = |value: i32| forms::scale(value, s.style.dpi);
    if s.hotkey_target.is_some() {
        return ListPart::Body;
    }
    let Some(keys) = s.hotkey_rows.get(at).and_then(|row| hotkey_row_keys(s, *row)) else { return ListPart::Body };
    let (caps, remove) = hotkey_spans(s, &keys, width);
    if x >= remove.0 && x < remove.1 {
        ListPart::Delete
    } else if x >= caps.0 - px(4) && x < caps.1 + px(4) {
        ListPart::Edit
    } else {
        ListPart::Body
    }
}

/// Where a hotkey row's key caps and Remove go: the caps at the right edge, in line with every other
/// control on the page, and Remove just before them.
fn hotkey_spans(s: &State, keys: &str, width: i32) -> ((i32, i32), (i32, i32)) {
    let px = |value: i32| forms::scale(value, s.style.dpi);
    let caps_width = Backbuffer::new(1, 1).map_or(0, |buffer| crate::ui::keycaps_width(&buffer, keys, s.style.small, s.style.icon_small, s.style.dpi));
    let right = width - px(8);
    let caps = (right - caps_width, right);
    let remove = (caps.0 - px(6) - forms::row_action_width(s.style.dpi), caps.0 - px(6));
    (caps, remove)
}

/// The first row that isn't a heading, going this way from `from`, and the other way if there's none.
fn nearest_hotkey_row(rows: &[HotkeyRow], from: usize, step: isize) -> Option<usize> {
    let usable = |at: isize| at >= 0 && (at as usize) < rows.len() && !rows[at as usize].is_header();
    let mut at = from as isize;
    while at >= 0 && (at as usize) < rows.len() {
        if usable(at) {
            return Some(at as usize);
        }
        at += step;
    }
    let mut at = from as isize - step;
    while at >= 0 && (at as usize) < rows.len() {
        if usable(at) {
            return Some(at as usize);
        }
        at -= step;
    }
    None
}

fn fill_hotkey_text(state: &State) {
    set_text(state.controls.hyper_table, &format_hyper(&state.base.hyper.bindings));
    set_text(state.controls.shortcuts, &format_shortcuts(&state.base.shortcuts));
}

/// The launcher changed a hotkey while this window is open. Show it, and keep it in what's saved
/// from here.
pub fn hotkeys_changed(settings: &Settings) {
    let Some(hwnd) = CURRENT.with(|c| c.get()) else { return };
    let Some(s) = state(hwnd) else { return };
    s.base.hyper.bindings = settings.hyper.bindings.clone();
    s.base.shortcuts = settings.shortcuts.clone();
    s.last_saved.hyper.bindings = settings.hyper.bindings.clone();
    s.last_saved.shortcuts = settings.shortcuts.clone();
    refresh_hotkey_list(s);
    fill_hotkey_text(s);
}

fn remove_hotkey_row(s: &mut State, row: HotkeyRow) {
    match row {
        HotkeyRow::Hyper(i) if i < s.base.hyper.bindings.len() => {
            s.base.hyper.bindings.remove(i);
        }
        HotkeyRow::Direct(i) if i < s.base.shortcuts.len() => {
            s.base.shortcuts.remove(i);
        }
        _ => return,
    }
    refresh_hotkey_list(s);
    fill_hotkey_text(s);
    save_soon(s, 1);
}

/// Delete was pressed on this row: arms it the first time, and goes ahead the second.
fn delete_hotkey_at(s: &mut State, at: usize) {
    let Some(row) = s.hotkey_rows.get(at).copied() else { return };
    if row.binding_action(&s.base).is_none() {
        return;
    }
    if forms::arm_or_confirm(&mut s.hotkey_armed, at) {
        remove_hotkey_row(s, row);
    } else {
        unsafe {
            let _ = InvalidateRect(Some(s.controls.hotkey_list), None, false);
        }
    }
}

/// Tells Orbit to let go of its shortcuts while keys are being recorded, or they'd never get here.
fn pause_shortcuts(s: &State, paused: bool) {
    s.context.main.post(WM_ORBIT_SETTINGS_RECORDING, paused as usize, 0);
}

/// Starts listening for keys for a row, or for something new.
fn start_hotkey_record(s: &mut State, target: RecordTarget) {
    let starting = s.hotkey_target.is_none();
    s.hotkey_target = Some(target);
    s.hotkey_problem = None;
    s.hotkey_asking = None;
    s.held_keys.clear();
    if starting {
        pause_shortcuts(s, true);
    }
    // This also refreshes the list, through the box's change notification.
    set_text(s.controls.action_search, "");
    refresh_hotkey_list(s);
    forms::focus(s.controls.hotkey_list);
}

fn stop_hotkey_record(s: &mut State) {
    if s.hotkey_target.is_some() && s.search_recording.is_some() {
        let at = search_recording_row(s);
        s.search_recording = None;
        say_search_note(s, "", false);
        if let Some(at) = at {
            redraw_search_row(s, at);
        }
    }
    if s.hotkey_target.take().is_some() {
        say_hotkey_problem(s, None);
        s.hotkey_asking = None;
        s.held_keys.clear();
        pause_shortcuts(s, false);
        refresh_hotkey_list(s);
    }
}

/// How much room a listening row has for its message, in a list this wide.
fn hotkey_message_room(s: &State, width: i32) -> i32 {
    let px = |value: i32| forms::scale(value, s.style.dpi);
    width - px(8) - px(PICKER_ICON_DIP) - px(10) - px(10) - px(HOTKEY_FIELD_DIP) - px(8)
}

/// Says why keys can't be used, or what they'd replace, on the row that's listening. If that would
/// cut it short, the whole thing goes above the list as well.
fn say_hotkey_problem(s: &mut State, problem: Option<String>) {
    if s.search_recording.is_some() && s.hotkey_target.is_some() {
        let alert = problem.is_some() && s.hotkey_asking.is_none();
        let text = problem.clone().unwrap_or_else(|| SEARCH_HINT.to_string());
        s.hotkey_problem = problem;
        say_search_note(s, &text, alert);
        if let Some(at) = search_recording_row(s) {
            redraw_search_row(s, at);
        }
        return;
    }
    s.hotkey_problem = problem;
    let list = s.controls.hotkey_list;
    let mut client = RECT::default();
    unsafe {
        let _ = GetClientRect(list, &mut client);
    }
    let room = hotkey_message_room(s, client.right);
    let long = s.hotkey_problem.as_ref().filter(|text| Backbuffer::new(1, 1).is_some_and(|buffer| buffer.text_width(text, s.style.small) > room)).cloned();
    let note = s.controls.hotkeys_note;
    match long {
        Some(text) => {
            s.style.set_alert(note, s.hotkey_asking.is_none());
            set_note_text(s, note, &text);
        }
        None => {
            let problems = s.hotkey_problems.clone();
            s.style.set_alert(note, !problems.is_empty());
            set_note_text(s, note, &problems);
        }
    }
    unsafe {
        let _ = InvalidateRect(Some(list), None, false);
    }
    if s.page == PAGE_HOTKEYS {
        reveal_list_row(s, list);
    }
}

/// A click on nothing in particular takes back whatever was waiting for a next step.
fn clicked_away(s: &mut State) {
    stop_launcher_record(s);
    stop_hotkey_record(s);
    disarm_button(s);
}

/// This key was already down, and Windows is repeating it.
fn is_repeat(lparam: LPARAM) -> bool {
    lparam.0 & (1 << 30) != 0
}

/// The modifiers down now, as `Ctrl+Alt`.
fn held_keys_text() -> String {
    let held = |key: u16| unsafe { GetKeyState(key as i32) } < 0;
    let mut names = Vec::new();
    if held(VK_CONTROL.0) {
        names.push("Ctrl");
    }
    if held(VK_MENU.0) {
        names.push("Alt");
    }
    if held(VK_SHIFT.0) {
        names.push("Shift");
    }
    if held(VK_LWIN.0) || held(VK_RWIN.0) {
        names.push("Win");
    }
    names.join("+")
}

/// Shows the modifiers that are down while listening.
fn note_held_keys(s: &mut State, control: HWND) {
    let held = held_keys_text();
    if held != s.held_keys {
        s.held_keys = held;
        unsafe {
            let _ = InvalidateRect(Some(control), None, false);
        }
    }
}

/// Whether a message belongs to the keys that just finished a recording: what they'd type, their
/// repeats and their release. Otherwise Alt+Space opens the window's menu and Alt+letter beeps.
fn swallow_after_recording(s: &mut State, message: u32, lparam: LPARAM) -> bool {
    match message {
        WM_CHAR | WM_SYSCHAR | WM_DEADCHAR | WM_SYSDEADCHAR => true,
        WM_KEYDOWN | WM_SYSKEYDOWN if is_repeat(lparam) => true,
        WM_KEYDOWN | WM_SYSKEYDOWN => {
            s.swallow_keys = false;
            false
        }
        WM_KEYUP | WM_SYSKEYUP => {
            if held_keys_text().is_empty() {
                s.swallow_keys = false;
            }
            true
        }
        _ => false,
    }
}

/// For the message loop, because a dialog's handling would act on this before the control sees it.
pub fn eats_key_text(message: &MSG) -> bool {
    match message.message {
        WM_SYSCHAR => {
            let Some(s) = CURRENT.with(|c| c.get()).and_then(state) else { return false };
            s.swallow_keys || s.recording_launcher || s.hotkey_target.is_some()
        }
        WM_KEYDOWN => {
            let key = message.wParam.0 as u16;
            let repeat = is_repeat(message.lParam);
            // Holding Esc or Enter is one press. It's already done what it's for, and the repeats
            // would go on to close the window, or confirm the very thing it just asked about.
            // Enter is only a line break in multi-line text, where holding it can repeat.
            let held_enter = repeat && key == VK_RETURN.0 && !is_text_area(message.hwnd);
            let root = unsafe { GetAncestor(message.hwnd, GA_ROOT) };
            // Enter that opened a form mustn't go on to save it, still empty.
            if held_enter && (is_of_class(root, "OrbitLinkForm") || is_of_class(root, "OrbitSnippet")) {
                return true;
            }
            let Some(s) = settings_of(message.hwnd) else { return false };
            if key == VK_TAB.0 || (VK_LEFT.0..=VK_DOWN.0).contains(&key) {
                set_focus_rings(s, true);
            }
            if !repeat {
                end_just_added(s);
            }
            held_enter || (repeat && key == VK_ESCAPE.0)
        }
        WM_LBUTTONDOWN => {
            if let Some(s) = settings_of(message.hwnd) {
                set_focus_rings(s, false);
                end_just_added(s);
            }
            false
        }
        _ => false,
    }
}

fn is_text_area(control: HWND) -> bool {
    const ES_MULTILINE_STYLE: isize = 0x4;
    is_of_class(control, "edit") && unsafe { GetWindowLongPtrW(control, GWL_STYLE) } & ES_MULTILINE_STYLE != 0
}

/// Settings' state, if this window is part of it.
fn settings_of(control: HWND) -> Option<&'static mut State> {
    let hwnd = CURRENT.with(|c| c.get())?;
    if unsafe { GetAncestor(control, GA_ROOT) } != hwnd {
        return None;
    }
    state(hwnd)
}

/// A click leaves the focus where it is without a ring round it. The keyboard needs to see it.
fn set_focus_rings(s: &mut State, on: bool) {
    if s.style.focus_by_keyboard != on {
        s.style.focus_by_keyboard = on;
        unsafe {
            let focus = GetFocus();
            if !focus.is_invalid() {
                let _ = InvalidateRect(Some(focus), None, false);
            }
        }
    }
}

/// A modifier alone isn't a shortcut yet, so `None` keeps listening.
fn pressed_keys(vk: u16) -> Option<(Mods, String)> {
    if matches!(vk, 0x10..=0x12 | 0x5B | 0x5C | 0xA0..=0xA5) {
        return None;
    }
    let held = |key: u16| unsafe { GetKeyState(key as i32) } < 0;
    let mut mods = Mods::default();
    if held(VK_CONTROL.0) {
        mods = mods.union(Mods::CTRL);
    }
    if held(VK_MENU.0) {
        mods = mods.union(Mods::ALT);
    }
    if held(VK_SHIFT.0) {
        mods = mods.union(Mods::SHIFT);
    }
    if held(VK_LWIN.0) || held(VK_RWIN.0) {
        mods = mods.union(Mods::SUPER);
    }
    keys::name_from_vk(vk as u8).map(|name| (mods, name))
}

/// Keys pressed while a hotkey row is listening.
fn record_hotkey_keys(s: &mut State, mods: Mods, key: &str) {
    let hotkey = hotkeys::from_recorded(mods, key, is_checked(s.controls.hyper_on));
    finish_hotkey_record(s, hotkey);
}

/// The HyperKey and another key were held while listening.
pub fn recorded_hyper(key: &str) {
    let Some(hwnd) = CURRENT.with(|c| c.get()) else { return };
    let Some(s) = state(hwnd) else { return };
    if s.hotkey_target.is_some() {
        finish_hotkey_record(s, Ok(Hotkey::Hyper(key.to_string())));
    }
}

fn finish_hotkey_record(s: &mut State, hotkey: Result<Hotkey, String>) {
    let Some(target) = s.hotkey_target.clone() else { return };
    let action = match &target {
        RecordTarget::New(action) => Some(action.clone()),
        RecordTarget::Row(row) => row.binding_action(&s.base),
    };
    let Some(action) = action else {
        stop_hotkey_record(s);
        return;
    };
    let launcher = launcher_chord_of(s);
    let hotkey = hotkey.and_then(|hotkey| {
        if matches!(&hotkey, Hotkey::Direct(chord) if launcher.as_ref() == Some(chord)) {
            Err("That shortcut already opens Orbit. Try another.".to_string())
        } else {
            Ok(hotkey)
        }
    });
    match hotkey {
        Err(problem) => {
            s.hotkey_asking = None;
            say_hotkey_problem(s, Some(problem));
        }
        Ok(hotkey) => {
            let label = hotkey.label("Win");
            let replaced = hotkeys::in_use_by(&s.base, &hotkey).filter(|old| !same_target(old, &action)).map(|old| hotkeys::describe(&s.base, &old));
            // Something else has these keys. Taking them needs saying so, and Esc keeps both.
            if let Some(name) = &replaced {
                if s.hotkey_asking.as_ref() != Some(&hotkey) {
                    let question = format!("Press again to replace {name}.");
                    s.hotkey_asking = Some(hotkey);
                    say_hotkey_problem(s, Some(question));
                    return;
                }
            }
            hotkeys::assign(&mut s.base, hotkey, action.clone());
            let search_row = search_recording_row(s);
            if s.search_recording.take().is_some() {
                say_search_note(s, "", false);
            }
            say_hotkey_problem(s, None);
            s.hotkey_asking = None;
            s.hotkey_target = None;
            s.held_keys.clear();
            pause_shortcuts(s, false);
            refresh_hotkey_list(s);
            if let Some(at) = search_row {
                retext_search_row(s, at);
            }
            // The keyboard carries on from the row that was just changed, on the Hotkeys page.
            if let Some(at) = s.hotkey_rows.iter().position(|row| row.binding_action(&s.base).is_some_and(|bound| same_target(&bound, &action))).filter(|_| search_row.is_none()) {
                let list = s.controls.hotkey_list;
                unsafe {
                    SendMessageW(list, LB_SETCURSEL, Some(WPARAM(at)), None);
                }
                reveal_list_row(s, list);
            }
            fill_hotkey_text(s);
            s.replaced_note = replaced.map(|name| format!("{label} ran {name}, and now runs this instead."));
            save_soon(s, 1);
        }
    }
}

/// What opens the launcher right now, as far as this window knows.
fn launcher_chord_of(s: &State) -> Option<Chord> {
    s.launcher_chord.clone().or_else(|| Chord::parse(&s.active_hotkey).ok()).or_else(|| Chord::parse("Alt+Space").ok())
}

/// The suggested mappings that wouldn't take a key or an item that already has one.
fn suggestions(settings: &Settings) -> Vec<orbit_core::settings::HyperBinding> {
    let Ok(suggested) = parse_hyper(SUGGESTED_HYPER) else { return Vec::new() };
    let mut out: Vec<orbit_core::settings::HyperBinding> = Vec::new();
    for binding in suggested {
        let key_taken = settings.hyper.bindings.iter().chain(&out).any(|b| b.key.eq_ignore_ascii_case(&binding.key));
        let item_taken = settings.hyper.bindings.iter().chain(&out).any(|b| same_target(&b.action, &binding.action))
            || settings.shortcuts.iter().any(|b| same_target(&b.action, &binding.action));
        if !key_taken && !item_taken {
            out.push(binding);
        }
    }
    out
}

fn add_suggested(s: &mut State) {
    let mut more = suggestions(&s.base);
    s.base.hyper.bindings.append(&mut more);
    refresh_hotkey_list(s);
    fill_hotkey_text(s);
    // That button has nothing left to offer, and a disabled control can't hold the keyboard.
    forms::focus(s.controls.action_search);
}

/// The hotkeys as a list or as text. Going back to the list needs text that makes sense.
fn toggle_hotkey_text(s: &mut State) {
    if s.text_mode {
        match parse_hotkey_text(s, launcher_chord_of(s).as_ref()) {
            Ok((hyper, direct)) => {
                s.base.hyper.bindings = hyper;
                s.base.shortcuts = direct;
                s.text_mode = false;
                refresh_hotkey_list(s);
                set_text(s.controls.edit_as_text, "Edit as text");
            }
            // Saving says what's wrong, next to the text.
            Err(_) => return,
        }
    } else {
        fill_hotkey_text(s);
        s.text_mode = true;
        set_text(s.controls.edit_as_text, "Show as list");
    }
    s.page_heights[PAGE_HOTKEYS] = if s.text_mode { s.hotkeys_text_bottom } else { s.hotkeys_list_bottom };
    show_page(s, s.page);
}

/// The search box above the hotkey list: Enter takes the first match, Down moves to the list, and
/// Esc clears it before it would close the window.
unsafe extern "system" fn action_search_proc(hwnd: HWND, message: u32, wparam: WPARAM, lparam: LPARAM, _id: usize, _data: usize) -> LRESULT {
    unsafe {
        if message == WM_NCDESTROY {
            let _ = RemoveWindowSubclass(hwnd, Some(action_search_proc), ACTION_SEARCH_SUBCLASS);
            return DefSubclassProc(hwnd, message, wparam, lparam);
        }
        // Reading the box's text sends messages back here, so only look when it matters.
        if !matches!(message, WM_GETDLGCODE | WM_KEYDOWN | WM_CHAR) {
            return DefSubclassProc(hwnd, message, wparam, lparam);
        }
        let Some(s) = state(GetAncestor(hwnd, GA_ROOT)) else {
            return DefSubclassProc(hwnd, message, wparam, lparam);
        };
        let searching = !get_text(hwnd).trim().is_empty();
        let key = wparam.0 as u16;
        match message {
            WM_GETDLGCODE => {
                let base = DefSubclassProc(hwnd, message, wparam, lparam);
                let wanted = (lparam.0 as *const MSG).as_ref().is_some_and(|m| m.message == WM_KEYDOWN && searching && (m.wParam.0 as u16 == VK_RETURN.0 || m.wParam.0 as u16 == VK_ESCAPE.0));
                return if wanted { LRESULT(base.0 | DLGC_WANTMESSAGE as isize) } else { base };
            }
            WM_KEYDOWN if searching && key == VK_RETURN.0 => {
                let at = SendMessageW(s.controls.hotkey_list, LB_GETCURSEL, None, None).0;
                act_on_hotkey_row(s, at.max(0) as usize);
                return LRESULT(0);
            }
            WM_KEYDOWN if searching && key == VK_ESCAPE.0 => {
                set_text(hwnd, "");
                return LRESULT(0);
            }
            WM_KEYDOWN if key == VK_DOWN.0 => {
                let list = s.controls.hotkey_list;
                if let Some(first) = nearest_hotkey_row(&s.hotkey_rows, 0, 1) {
                    SendMessageW(list, LB_SETCURSEL, Some(WPARAM(first)), None);
                    let _ = SetFocus(Some(list));
                }
                return LRESULT(0);
            }
            WM_CHAR if searching && (key == 13 || key == 27) => return LRESULT(0),
            _ => {}
        }
        DefSubclassProc(hwnd, message, wparam, lparam)
    }
}

/// Enter, or a click on a row: a match gets its keys, and a hotkey gets new ones.
fn act_on_hotkey_row(s: &mut State, at: usize) {
    match s.hotkey_rows.get(at).copied() {
        Some(HotkeyRow::Match(option)) => {
            if let Some(option) = s.action_options.get(option) {
                let action = option.action.clone();
                start_hotkey_record(s, RecordTarget::New(action));
            }
        }
        Some(row @ (HotkeyRow::Hyper(_) | HotkeyRow::Direct(_))) => start_hotkey_record(s, RecordTarget::Row(row)),
        Some(HotkeyRow::HyperHeader) if !is_checked(s.controls.hyper_on) => select_page(s, PAGE_HYPER),
        _ => {}
    }
}

fn picker_query(picker: &Picker) -> String {
    get_text(picker.edit)
}

/// Rebuilds a picker's list. While searching, the first row is ready for Enter.
fn refresh_picker(s: &mut State, index: usize) {
    let Some(picker) = s.pickers.get_mut(index) else { return };
    let query = picker_query(picker);
    let chosen_row = unsafe { SendMessageW(picker.list, LB_GETCURSEL, None, None) }.0;
    picker.rows = app_picker::rows(&picker.chosen, &s.candidates, &query);
    if query.trim().is_empty() && !picker.expanded {
        picker.rows = app_picker::collapsed(std::mem::take(&mut picker.rows));
    }
    unsafe {
        SendMessageW(picker.list, LB_RESETCONTENT, None, None);
        for row in &picker.rows {
            let text = match row {
                PickerRow::App { name, program, .. } => wide(&format!("{name}, {program}")),
                PickerRow::Message(text) => wide(text),
                PickerRow::ShowAll(count) => wide(&format!("Show all {count}")),
            };
            SendMessageW(picker.list, LB_ADDSTRING, None, Some(LPARAM(text.as_ptr() as isize)));
        }
        let has_apps = matches!(picker.rows.first(), Some(PickerRow::App { .. }));
        let at = if !has_apps {
            -1
        } else if !query.trim().is_empty() {
            0
        } else {
            chosen_row.min(picker.rows.len() as isize - 1)
        };
        SendMessageW(picker.list, LB_SETCURSEL, Some(WPARAM(at as usize)), None);
    }
    picker.hover = None;
    // What's chosen always shows whole. Only a search has more to scroll through.
    let shown = if query.trim().is_empty() { picker.rows.len().max(1) } else { picker.rows.len().clamp(1, SEARCH_ROWS) };
    let list = picker.list;
    let height = forms::scale(PICKER_ROW_DIP * shown as i32, s.style.dpi);
    resize_on_page(s, list, height);
    // What's found is what the typing is for, even at the foot of the page.
    if !query.trim().is_empty() {
        reveal_control(s, list);
    }
}

/// Adds or removes the app on this row. Adding from a search clears it, so the app shows in the list.
fn act_on_picker_row(s: &mut State, index: usize, row: isize) {
    let Some(picker) = s.pickers.get_mut(index) else { return };
    if matches!(usize::try_from(row).ok().and_then(|row| picker.rows.get(row)), Some(PickerRow::ShowAll(_))) {
        picker.expanded = true;
        refresh_picker(s, index);
        return;
    }
    let Some(PickerRow::App { program, .. }) = usize::try_from(row).ok().and_then(|row| picker.rows.get(row)) else { return };
    let program = program.clone();
    let added = app_picker::toggle(&mut picker.chosen, &program);
    let searching = !picker_query(picker).trim().is_empty();
    let edit = picker.edit;
    if searching {
        // The box reports this change, which refreshes the list.
        set_text(edit, "");
    }
    refresh_picker(s, index);
    if searching {
        unsafe {
            s.last_focus = edit;
            let _ = SetFocus(Some(edit));
        }
    }
    if added {
        show_added_app(s, index, &program);
    }
    save_soon(s, 1);
}

/// What's just been added has to be seen, even at the end of a long list on a long page.
fn show_added_app(s: &mut State, index: usize, program: &str) {
    let Some(picker) = s.pickers.get_mut(index) else { return };
    picker.expanded = true;
    refresh_picker(s, index);
    let Some(picker) = s.pickers.get(index) else { return };
    let list = picker.list;
    if let Some(at) = picker.rows.iter().position(|row| matches!(row, PickerRow::App { program: p, .. } if p == program)) {
        if let Some(picker) = s.pickers.get_mut(index) {
            picker.just_added = true;
        }
        unsafe {
            SendMessageW(list, LB_SETCURSEL, Some(WPARAM(at)), None);
            let _ = InvalidateRect(Some(list), None, false);
        }
        reveal_list_row(s, list);
    }
}

/// The next key or click ends the light on a row that's just been added.
fn end_just_added(s: &mut State) {
    for picker in s.pickers.iter_mut().filter(|picker| picker.just_added) {
        picker.just_added = false;
        unsafe {
            let _ = InvalidateRect(Some(picker.list), None, false);
        }
    }
}

fn move_picker_selection(picker: &Picker, by: isize) {
    unsafe {
        let now = SendMessageW(picker.list, LB_GETCURSEL, None, None).0;
        let last = picker.rows.len() as isize - 1;
        SendMessageW(picker.list, LB_SETCURSEL, Some(WPARAM((now + by).clamp(0, last.max(0)) as usize)), None);
    }
}

fn pick_program_file(owner: HWND) -> Option<String> {
    pick_file(owner, "Programs (*.exe)\0*.exe\0").map(|path| crate::util::exe_name(&path.to_string_lossy()).to_lowercase())
}

/// A row's area, from the point of the list that was clicked.
fn picker_row_at(list: HWND, lparam: LPARAM) -> isize {
    let hit = unsafe { SendMessageW(list, LB_ITEMFROMPOINT, None, Some(lparam)) }.0;
    if (hit >> 16) & 0xFFFF != 0 { -1 } else { hit & 0xFFFF }
}

/// The search box and list of a picker. `data` is the picker's index, times two, plus 1 for the list.
/// Looks the picker up each time, because the window's state goes before its children do.
unsafe extern "system" fn picker_proc(hwnd: HWND, message: u32, wparam: WPARAM, lparam: LPARAM, _id: usize, data: usize) -> LRESULT {
    unsafe {
        if message == WM_NCDESTROY {
            let _ = RemoveWindowSubclass(hwnd, Some(picker_proc), data);
            return DefSubclassProc(hwnd, message, wparam, lparam);
        }
        let (index, is_list) = (data / 2, data % 2 == 1);
        let Some(s) = state(GetAncestor(hwnd, GA_ROOT)) else {
            return DefSubclassProc(hwnd, message, wparam, lparam);
        };
        if is_list && (message == WM_MOUSEMOVE || message == WM_MOUSELEAVE) {
            if let Some(picker) = s.pickers.get_mut(index) {
                let row = if message == WM_MOUSEMOVE { picker_row_at(hwnd, lparam) } else { -1 };
                let hover = (row >= 0 && (row as usize) < picker.rows.len()).then_some(row as usize);
                if hover != picker.hover {
                    picker.hover = hover;
                    let _ = InvalidateRect(Some(hwnd), None, false);
                }
                if message == WM_MOUSELEAVE {
                    picker.tracking = false;
                } else if !picker.tracking {
                    picker.tracking = true;
                    let mut track = TRACKMOUSEEVENT { cbSize: std::mem::size_of::<TRACKMOUSEEVENT>() as u32, dwFlags: TME_LEAVE, hwndTrack: hwnd, dwHoverTime: 0 };
                    let _ = TrackMouseEvent(&mut track);
                }
            }
            return DefSubclassProc(hwnd, message, wparam, lparam);
        }
        let Some(picker) = s.pickers.get(index) else {
            return DefSubclassProc(hwnd, message, wparam, lparam);
        };
        // A row shows as selected only while it's what the keyboard acts on.
        if is_list && (message == WM_SETFOCUS || message == WM_KILLFOCUS) {
            let result = DefSubclassProc(hwnd, message, wparam, lparam);
            if message == WM_SETFOCUS
                && SendMessageW(hwnd, LB_GETCURSEL, None, None).0 < 0
                && matches!(picker.rows.first(), Some(PickerRow::App { .. }))
            {
                SendMessageW(hwnd, LB_SETCURSEL, Some(WPARAM(0)), None);
            }
            let _ = InvalidateRect(Some(hwnd), None, false);
            return result;
        }
        // Reading the box's text sends messages back here, so only look when it matters.
        if !matches!(message, WM_GETDLGCODE | WM_KEYDOWN | WM_CHAR | WM_LBUTTONDOWN | WM_LBUTTONUP) {
            return DefSubclassProc(hwnd, message, wparam, lparam);
        }
        let searching = !picker_query(picker).trim().is_empty();
        let key = wparam.0 as u16;
        let on_show_all = is_list && {
            let at = SendMessageW(hwnd, LB_GETCURSEL, None, None).0;
            matches!(picker.rows.get(at.max(0) as usize), Some(PickerRow::ShowAll(_)))
        };
        // Enter and Esc go to the window's buttons unless the control asks for them.
        let wanted = |key: u16| {
            if is_list {
                // With nothing typed, Enter must not remove a row. Esc in results goes back to the search.
                key == VK_DELETE.0 || ((searching || on_show_all) && key == VK_RETURN.0) || (searching && key == VK_ESCAPE.0)
            } else {
                searching && (key == VK_RETURN.0 || key == VK_ESCAPE.0)
            }
        };
        match message {
            WM_GETDLGCODE => {
                let base = DefSubclassProc(hwnd, message, wparam, lparam);
                let asked = (lparam.0 as *const MSG).as_ref().is_some_and(|m| m.message == WM_KEYDOWN && wanted(m.wParam.0 as u16));
                return if asked { LRESULT(base.0 | DLGC_WANTMESSAGE as isize) } else { base };
            }
            WM_KEYDOWN if is_list => {
                let row = SendMessageW(hwnd, LB_GETCURSEL, None, None).0;
                if key == VK_ESCAPE.0 && searching {
                    let edit = picker.edit;
                    set_text(edit, "");
                    let _ = SetFocus(Some(edit));
                    return LRESULT(0);
                }
                if key == VK_DELETE.0 || (key == VK_RETURN.0 && (searching || on_show_all)) {
                    // Holding it down isn't asking for each app below to go as well.
                    if is_repeat(lparam) {
                        return LRESULT(0);
                    }
                    let is_chosen = picker.rows.get(row.max(0) as usize).is_some_and(|r| matches!(r, PickerRow::App { chosen: true, .. }));
                    if key == VK_RETURN.0 || is_chosen {
                        act_on_picker_row(s, index, row);
                    }
                    return LRESULT(0);
                }
                if key == VK_UP.0 && row <= 0 {
                    let _ = SetFocus(Some(picker.edit));
                    return LRESULT(0);
                }
            }
            WM_KEYDOWN => {
                if key == VK_RETURN.0 && searching {
                    let row = SendMessageW(picker.list, LB_GETCURSEL, None, None).0;
                    act_on_picker_row(s, index, row);
                    return LRESULT(0);
                }
                if key == VK_ESCAPE.0 && searching {
                    set_text(hwnd, "");
                    return LRESULT(0);
                }
                if key == VK_DOWN.0 || key == VK_UP.0 {
                    if searching {
                        move_picker_selection(picker, if key == VK_DOWN.0 { 1 } else { -1 });
                        let list = picker.list;
                        reveal_list_row(s, list);
                    } else if key == VK_DOWN.0 && matches!(picker.rows.first(), Some(PickerRow::App { .. })) {
                        let list = picker.list;
                        if SendMessageW(list, LB_GETCURSEL, None, None).0 < 0 {
                            SendMessageW(list, LB_SETCURSEL, Some(WPARAM(0)), None);
                        }
                        let _ = SetFocus(Some(list));
                    }
                    return LRESULT(0);
                }
            }
            // Without this the box beeps at the Enter or Esc it just handled.
            WM_CHAR if !is_list && searching && (key == 13 || key == 27) => return LRESULT(0),
            WM_CHAR if is_list && (key == 13 || key == 27) => return LRESULT(0),
            WM_LBUTTONDOWN if is_list => s.list_press = Some((hwnd, picker_row_at(hwnd, lparam))),
            WM_LBUTTONUP if is_list => {
                let result = DefSubclassProc(hwnd, message, wparam, lparam);
                let row = picker_row_at(hwnd, lparam);
                let mut client = RECT::default();
                let _ = GetClientRect(hwnd, &mut client);
                let x = (lparam.0 & 0xFFFF) as i16 as i32;
                let on_action = x >= client.right - forms::scale(PICKER_ACTION_DIP, s.style.dpi);
                let show_all = matches!(s.pickers.get(index).and_then(|p| p.rows.get(row.max(0) as usize)), Some(PickerRow::ShowAll(_)));
                if s.list_press.take() == Some((hwnd, row)) && (searching || on_action || show_all) {
                    act_on_picker_row(s, index, row);
                }
                return result;
            }
            _ => {}
        }
        DefSubclassProc(hwnd, message, wparam, lparam)
    }
}

fn draw_picker_item(s: &mut State, index: usize, item: &DRAWITEMSTRUCT) {
    let dpi = s.style.dpi;
    let px = |value: i32| forms::scale(value, dpi);
    let (width, height) = (item.rcItem.right - item.rcItem.left, item.rcItem.bottom - item.rcItem.top);
    let Some(mut buffer) = Backbuffer::new(width, height) else { return };
    let theme = s.style.theme;
    buffer.fill(theme.card);
    let Some(row) = s.pickers.get(index).and_then(|p| p.rows.get(item.itemID as usize)).cloned() else {
        buffer.copy_to_dc_at(item.hDC, item.rcItem.left, item.rcItem.top);
        return;
    };
    let enabled = unsafe { IsWindowEnabled(item.hwndItem) }.as_bool();
    let searching = s.pickers.get(index).is_some_and(|p| !picker_query(p).trim().is_empty());
    let active = searching || s.pickers.get(index).is_some_and(|p| p.just_added || unsafe { GetFocus() } == p.list);
    let selected_now = active && item.itemState.0 & ODS_SELECTED.0 != 0;
    let hovered = s.pickers.get(index).is_some_and(|p| p.hover == Some(item.itemID as usize));
    match row {
        PickerRow::Message(text) => {
            let ink = if enabled { theme.secondary } else { theme.tertiary };
            // A list that's off has no search to point at.
            let text = if enabled { text.as_str() } else { "No apps yet." };
            buffer.text(text, s.style.font, ink, RECT { left: px(8), top: 0, right: width - px(8), bottom: height }, false);
        }
        PickerRow::ShowAll(count) => {
            if selected_now {
                buffer.fill_round_rect(0, px(2), width, height - px(4), px(forms::RADIUS_SELECTION) as f32, theme.selection);
            } else if hovered && enabled {
                buffer.fill_round_rect(0, px(2), width, height - px(4), px(forms::RADIUS_SELECTION) as f32, theme.selection.mix(theme.card, 0.5));
            }
            let ink = if enabled { theme.accent } else { theme.tertiary };
            buffer.text(&format!("Show all {count}"), s.style.font, ink, RECT { left: px(8), top: 0, right: width - px(8), bottom: height }, false);
        }
        PickerRow::App { name, program, icon, chosen } => {
            if selected_now {
                buffer.fill_round_rect(0, px(2), width, height - px(4), px(forms::RADIUS_SELECTION) as f32, theme.selection);
            } else if hovered && enabled {
                buffer.fill_round_rect(0, px(2), width, height - px(4), px(forms::RADIUS_SELECTION) as f32, theme.selection.mix(theme.card, 0.5));
            }
            let size = px(PICKER_ICON_DIP);
            let (icon_x, icon_y) = (px(8), (height - size) / 2);
            let pixels = icon.and_then(|hash| icon_pixels(s, hash, size as usize));
            match pixels {
                Some(pixels) => buffer.blit_premultiplied(icon_x, icon_y, size, &pixels),
                None => draw_small_tile(&s.style, &mut buffer, (icon_x, icon_y, size), forms::letter_tile(&name), theme.keycap),
            }
            let action_left = width - px(PICKER_ACTION_DIP);
            let text_left = icon_x + size + px(10);
            let name_width = buffer.text_width(&name, s.style.font);
            let name_right = (text_left + name_width).min(action_left - px(8));
            let (ink, quiet) = if enabled { (theme.text, theme.secondary) } else { (theme.tertiary, theme.tertiary) };
            buffer.text(&name, s.style.font, ink, RECT { left: text_left, top: 0, right: name_right, bottom: height }, false);
            if name != format!("Add \u{201C}{program}\u{201D}") && !app_picker::name_says_program(&name, &program) {
                buffer.text(&program, s.style.small, quiet, RECT { left: name_right + px(10), top: 0, right: action_left, bottom: height }, false);
            }
            // What's already on the list only offers to go when it's being pointed at.
            if enabled && (searching || hovered || selected_now) {
                let (label, color) = if chosen { ("Remove", theme.secondary) } else { ("Add", theme.accent) };
                buffer.text(label, s.style.small, color, RECT { left: action_left, top: 0, right: width - px(10), bottom: height }, true);
            }
        }
    }
    buffer.copy_to_dc_at(item.hDC, item.rcItem.left, item.rcItem.top);
}

/// A path has no spaces to wrap at, so a long one keeps its start and end.
fn short_path(path: &str, limit: usize) -> String {
    let chars: Vec<char> = path.chars().collect();
    if chars.len() <= limit {
        return path.to_string();
    }
    let keep = (limit - 1) / 2;
    let start: String = chars[..keep].iter().collect();
    let end: String = chars[chars.len() - keep..].iter().collect();
    format!("{start}\u{2026}{end}")
}

/// The launcher's shortcut: shows the keys, and listens for new ones when asked.
unsafe extern "system" fn recorder_proc(hwnd: HWND, message: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    unsafe {
        let Some(s) = state(GetAncestor(hwnd, GA_ROOT)) else {
            return DefWindowProcW(hwnd, message, wparam, lparam);
        };
        let key = wparam.0 as u16;
        match message {
            WM_ERASEBKGND => LRESULT(1),
            WM_PAINT => {
                let mut ps = PAINTSTRUCT::default();
                let dc = BeginPaint(hwnd, &mut ps);
                let mut client = RECT::default();
                let _ = GetClientRect(hwnd, &mut client);
                if let Some(mut buffer) = Backbuffer::new(client.right, client.bottom) {
                    draw_recorder(s, &mut buffer, client.right, client.bottom, GetFocus() == hwnd);
                    buffer.copy_to_dc(dc);
                }
                let _ = EndPaint(hwnd, &ps);
                LRESULT(0)
            }
            WM_GETDLGCODE => {
                if s.recording_launcher {
                    return LRESULT(DLGC_WANTALLKEYS as isize);
                }
                let wanted = (lparam.0 as *const MSG).as_ref().is_some_and(|m| m.message == WM_KEYDOWN && (m.wParam.0 as u16 == VK_RETURN.0 || m.wParam.0 as u16 == VK_SPACE.0));
                LRESULT(if wanted { DLGC_WANTMESSAGE as isize } else { 0 })
            }
            WM_LBUTTONDOWN => {
                let _ = SetFocus(Some(hwnd));
                start_launcher_record(s);
                LRESULT(0)
            }
            WM_SETFOCUS | WM_KILLFOCUS => {
                if message == WM_KILLFOCUS {
                    stop_launcher_record(s);
                }
                let _ = InvalidateRect(Some(hwnd), None, false);
                LRESULT(0)
            }
            WM_MOUSEWHEEL => {
                if let Ok(parent) = GetParent(hwnd) {
                    return SendMessageW(parent, message, Some(wparam), Some(lparam));
                }
                LRESULT(0)
            }
            WM_KEYDOWN | WM_SYSKEYDOWN | WM_KEYUP | WM_SYSKEYUP | WM_CHAR | WM_SYSCHAR | WM_DEADCHAR | WM_SYSDEADCHAR if s.swallow_keys && swallow_after_recording(s, message, lparam) => LRESULT(0),
            WM_KEYDOWN | WM_SYSKEYDOWN if s.recording_launcher => {
                // Still the Enter or Space that started this, or the keys that were just refused.
                if is_repeat(lparam) {
                    return LRESULT(0);
                }
                let no_mods = pressed_keys(key).is_some_and(|(mods, _)| mods == Mods::NONE);
                if key == VK_ESCAPE.0 {
                    stop_launcher_record(s);
                    s.swallow_keys = true;
                } else if no_mods && (key == VK_BACK.0 || key == VK_DELETE.0) {
                    s.launcher_chord = None;
                    stop_launcher_record(s);
                    s.swallow_keys = true;
                    save_soon(s, 1);
                } else if let Some((mods, name)) = pressed_keys(key) {
                    match launcher_chord_from(s, mods, &name) {
                        Ok(chord) => {
                            s.launcher_chord = Some(chord);
                            stop_launcher_record(s);
                            s.swallow_keys = true;
                            save_soon(s, 1);
                        }
                        Err(problem) => {
                            set_hotkey_note(s, &problem, true);
                            note_held_keys(s, hwnd);
                        }
                    }
                } else {
                    note_held_keys(s, hwnd);
                }
                LRESULT(0)
            }
            WM_KEYDOWN if (key == VK_RETURN.0 || key == VK_SPACE.0) && !is_repeat(lparam) => {
                start_launcher_record(s);
                LRESULT(0)
            }
            WM_KEYUP | WM_SYSKEYUP if s.recording_launcher => {
                note_held_keys(s, hwnd);
                LRESULT(0)
            }
            // Without this, Alt+Space opens the window's own menu while recording.
            WM_CHAR | WM_SYSCHAR if s.recording_launcher => LRESULT(0),
            WM_CHAR => LRESULT(0),
            _ => DefWindowProcW(hwnd, message, wparam, lparam),
        }
    }
}

fn launcher_chord_from(s: &State, mods: Mods, name: &str) -> Result<Chord, String> {
    match hotkeys::from_recorded(mods, name, false)? {
        Hotkey::Direct(chord) => {
            if s.base.shortcuts.iter().any(|binding| binding.chord == chord) {
                return Err(format!("{} already runs another hotkey. Try another.", chord.display("Win")));
            }
            Ok(chord)
        }
        Hotkey::Hyper(_) => Err("A shortcut needs Ctrl, Alt or Win, so that typing still works.".to_string()),
    }
}

fn start_launcher_record(s: &mut State) {
    if !s.recording_launcher {
        s.recording_launcher = true;
        s.held_keys.clear();
        pause_shortcuts(s, true);
        set_hotkey_note(s, RECORDING_NOTE, false);
        unsafe {
            let _ = InvalidateRect(Some(s.controls.hotkey), None, false);
        }
    }
}

fn stop_launcher_record(s: &mut State) {
    if s.recording_launcher {
        s.recording_launcher = false;
        s.held_keys.clear();
        pause_shortcuts(s, false);
        show_hotkey_problems(s);
        unsafe {
            let _ = InvalidateRect(Some(s.controls.hotkey), None, false);
        }
    }
}

fn draw_recorder(s: &State, buffer: &mut Backbuffer, width: i32, height: i32, focused: bool) {
    let style = &s.style;
    let theme = &style.theme;
    let px = |value: i32| forms::scale(value, style.dpi);
    buffer.fill(theme.card);
    let inset = 1.max(style.dpi as i32 / 96);
    let radius = px(forms::RADIUS_FIELD) as f32;
    let border = if s.recording_launcher || focused { theme.accent } else { theme.divider };
    buffer.fill_round_rect(0, 0, width, height, radius, border);
    buffer.fill_round_rect(inset, inset, width - inset * 2, height - inset * 2, radius - inset as f32, theme.field);
    let text = RECT { left: px(10), top: 0, right: width - px(10), bottom: height };
    if s.recording_launcher {
        if s.held_keys.is_empty() {
            buffer.text("Press keys\u{2026}", style.font, theme.accent, text, false);
        } else {
            crate::ui::draw_keycaps(buffer, &s.held_keys, px(8), height / 2, style.small, style.icon_small, (theme.keycap, 255), theme.text, style.dpi);
        }
        return;
    }
    // What really opens Orbit, which isn't what was asked for if something else has that key.
    let shown = if !s.active_hotkey.is_empty() {
        s.active_hotkey.clone()
    } else {
        s.launcher_chord.as_ref().map_or_else(|| "Alt+Space".to_string(), |chord| chord.display("Win"))
    };
    crate::ui::draw_keycaps(buffer, &shown, px(8), height / 2, style.small, style.icon_small, (theme.keycap, 255), theme.text, style.dpi);
    if s.launcher_chord.is_none() {
        buffer.text("Default", style.small, theme.secondary, text, true);
    }
}

fn window_of(s: &State) -> HWND {
    unsafe { GetAncestor(s.list, GA_ROOT) }
}

/// Saves after a moment, however many more changes arrive before then.
fn save_soon(s: &State, delay_ms: u32) {
    unsafe {
        SetTimer(Some(window_of(s)), SAVE_TIMER, delay_ms, None);
    }
}

/// Puts text at the very foot of the page list, however many lines it takes.
fn set_foot_message(s: &State, text: &str) {
    let px = |value: i32| forms::scale(value, s.style.dpi);
    let mut client = RECT::default();
    let mut bounds = RECT::default();
    unsafe {
        let _ = GetClientRect(window_of(s), &mut client);
        let _ = GetWindowRect(s.status, &mut bounds);
        let width = bounds.right - bounds.left;
        let height = forms::text_height_in(s.status, text, width).min(px(STATUS_DIP));
        let _ = SetWindowPos(s.status, None, px(24), client.bottom - px(FOOT_DIP) - height, width, height, SWP_NOZORDER | SWP_NOACTIVATE);
    }
    set_status(s.status, text);
}

/// A message at the foot of the page list, which goes away by itself.
fn show_status(s: &mut State, text: &str, for_ms: u32) {
    s.style.set_alert(s.status, false);
    set_foot_message(s, text);
    unsafe {
        SetTimer(Some(window_of(s)), STATUS_TIMER, for_ms, None);
    }
}

/// A message that stays until something replaces it, because it's a problem.
fn show_problem(s: &mut State, text: &str) {
    s.style.set_alert(s.status, true);
    set_foot_message(s, text);
    unsafe {
        let _ = KillTimer(Some(window_of(s)), STATUS_TIMER);
    }
}

/// Sends what's on the form to Orbit, if that's changed, and says what can't be saved. Text that
/// doesn't make sense keeps its last good value and holds nothing else back. Orbit answers through
/// `applied`, so "Saved" means it really was.
fn save_now(s: &mut State) -> Option<String> {
    unsafe {
        let _ = KillTimer(Some(window_of(s)), SAVE_TIMER);
    }
    let (settings, problem) = collect(s);
    // This is now the last good text, which a later mistake falls back to.
    if s.text_mode && problem.is_none() {
        s.base.hyper.bindings = settings.hyper.bindings.clone();
        s.base.shortcuts = settings.shortcuts.clone();
    }
    let (hyper_table, shortcuts, text_error) = (s.controls.hyper_table, s.controls.shortcuts, s.controls.text_error);
    let hyper_wrong = problem.as_deref().is_some_and(|text| text.starts_with("HyperKey"));
    s.style.set_alert(hyper_table, hyper_wrong);
    s.style.set_alert(shortcuts, problem.is_some() && !hyper_wrong);
    s.style.set_alert(text_error, problem.is_some());
    if problem.is_some() || s.showing_problem {
        set_status(text_error, problem.as_deref().unwrap_or(""));
        unsafe {
            let _ = InvalidateRect(Some(s.content), None, false);
        }
    }
    s.showing_problem = problem.is_some();
    if problem.is_none() {
        s.close_warned = false;
    }
    if settings != s.last_saved || s.unhide_all || s.save_pending {
        let unhide = s.unhide_all;
        s.last_saved = settings.clone();
        if unhide {
            s.base.hidden.clear();
            s.unhide_all = false;
        }
        s.context.main.post(WM_ORBIT_SETTINGS_APPLY, crate::util::box_for_post(settings), unhide as isize);
    }
    problem
}

/// Saves what's waiting, then closes. If some text can't be saved it says so and stays, and closing
/// again leaves that text unsaved.
fn save_and_close(s: &mut State, hwnd: HWND) {
    match save_now(s) {
        Some(problem) if !s.close_warned => {
            s.close_warned = true;
            select_page(s, PAGE_HOTKEYS);
            set_status(s.controls.text_error, &format!("{problem} Close again to leave this text unsaved."));
        }
        _ => unsafe {
            let _ = DestroyWindow(hwnd);
        },
    }
}

/// Orbit's answer after settings change: whether they reached the disk, if it was asked to save,
/// and what's true now.
pub fn applied(saved: Option<bool>, problems: &[String], active_hotkey: &str, hidden_items: &[String]) {
    let Some(hwnd) = CURRENT.with(|c| c.get()) else { return };
    let Some(s) = state(hwnd) else { return };
    // What's hidden belongs to the launcher, which may have hidden something itself.
    let hidden = hidden_items.len();
    if s.base.hidden != hidden_items {
        s.base.hidden = hidden_items.to_vec();
        s.last_saved.hidden = s.base.hidden.clone();
        unsafe {
            let _ = InvalidateRect(Some(s.search_list), None, false);
        }
    }
    s.active_hotkey = active_hotkey.to_string();
    s.hotkey_problems = problems.join(" ");
    if !s.recording_launcher {
        show_hotkey_problems(s);
    }
    if hidden == 0 {
        move_focus_off(s, s.controls.hidden_button);
    }
    unsafe {
        let _ = InvalidateRect(Some(s.controls.hotkey), None, false);
        let _ = EnableWindow(s.controls.hidden_button, hidden > 0);
        let _ = InvalidateRect(Some(s.controls.hidden_button), None, false);
    }
    set_status(s.controls.hidden_note, &hidden_count(hidden));
    match saved {
        Some(true) => {
            s.save_pending = false;
            match s.replaced_note.take() {
                Some(note) => show_status(s, &note, MESSAGE_MS),
                // What the user just asked for matters more than that it's saved.
                None if s.result_until.is_some_and(|until| std::time::Instant::now() < until) => {}
                None => show_status(s, "Saved", SAVED_MS),
            }
        }
        Some(false) => {
            s.save_pending = true;
            show_problem(s, SAVE_FAILED);
        }
        None => {}
    }
}

const SAVE_FAILED: &str = "Couldn't save, so this lasts until Orbit closes. Orbit tries again with your next change.";

fn hidden_count(count: usize) -> String {
    match count {
        0 => "Nothing is hidden.".to_string(),
        1 => "1 item".to_string(),
        n => format!("{n} items"),
    }
}

fn keyboard_description(mode: orbit_core::vim::KeyboardMode) -> &'static str {
    use orbit_core::vim::KeyboardMode::*;
    match mode {
        Standard => "Type to search, and use the arrow keys to move.",
        VimKeys => "Type to search, and use Alt+H J K L to move.",
        Vim => "Opens ready to navigate. Press i to type, and Esc to navigate again.",
    }
}

/// The line under the launcher's shortcut, in red if it's a problem.
fn set_hotkey_note(s: &mut State, text: &str, alert: bool) {
    let note = s.controls.hotkey_note;
    s.style.set_alert(note, alert);
    set_note_text(s, note, text);
}

/// Sets text that takes only the room it needs.
fn set_note_text(s: &mut State, note: HWND, text: &str) {
    let mut bounds = RECT::default();
    unsafe {
        let _ = GetWindowRect(note, &mut bounds);
    }
    let gap = forms::scale(8, s.style.dpi);
    let height = if text.is_empty() { 0 } else { forms::text_height_in(note, text, bounds.right - bounds.left) + gap };
    set_status(note, text);
    resize_on_page(s, note, height);
}

/// What the launcher's shortcut and the Hotkeys page should say now.
fn show_hotkey_problems(s: &mut State) {
    let problems = s.hotkey_problems.clone();
    if problems.is_empty() {
        set_hotkey_note(s, IDLE_HOTKEY_NOTE, false);
    } else {
        set_hotkey_note(s, &problems, true);
    }
    let note = s.controls.hotkeys_note;
    s.style.set_alert(note, !problems.is_empty());
    set_note_text(s, note, &problems);
}

/// A switch that other settings depend on has changed, or has just been set.
fn apply_dependents(s: &mut State) {
    for at in 0..s.dependents.len() {
        let on = is_checked(s.dependents[at].0);
        for index in 0..s.dependents[at].1.len() {
            let child = s.dependents[at].1[index];
            unsafe {
                if is_plain_text(child) {
                    s.style.set_dimmed(child, !on);
                } else {
                    let _ = EnableWindow(child, on);
                }
                let _ = InvalidateRect(Some(child), None, true);
            }
        }
    }
    // A switch has nothing to search until its tool is installed, unless it's already on.
    for (switch, tool) in [(s.controls.op_on, Tool::OnePassword), (s.controls.ente_on, Tool::EnteAuth)] {
        let usable = is_checked(switch) || integrations::find(tool).is_some();
        unsafe {
            let _ = EnableWindow(switch, usable);
            let _ = InvalidateRect(Some(switch), None, false);
        }
    }
    unsafe {
        let _ = InvalidateRect(Some(s.content), None, false);
    }
}

fn is_plain_text(control: HWND) -> bool {
    is_of_class(control, "static")
}

fn is_of_class(control: HWND, name: &str) -> bool {
    let mut class = [0u16; 32];
    let length = unsafe { GetClassNameW(control, &mut class) };
    String::from_utf16_lossy(&class[..length.max(0) as usize]).eq_ignore_ascii_case(name)
}

/// First press on something dangerous asks for a second, right there. `true` means go ahead.
fn confirm_in_place(s: &mut State, button: HWND, label: &'static str) -> bool {
    if s.armed_button.is_some_and(|(armed, _)| armed == button) {
        // A double click is one press, however fast the second half comes.
        if s.armed_at.elapsed() < std::time::Duration::from_millis(unsafe { GetDoubleClickTime() } as u64) {
            return false;
        }
        disarm_button(s);
        return true;
    }
    disarm_button(s);
    s.armed_button = Some((button, label));
    s.armed_at = std::time::Instant::now();
    s.style.set_alert(button, true);
    set_text(button, "Press again");
    unsafe {
        let _ = InvalidateRect(Some(button), None, false);
        SetTimer(Some(window_of(s)), ARM_TIMER, ARM_MS, None);
    }
    false
}

fn disarm_button(s: &mut State) {
    if let Some((button, label)) = s.armed_button.take() {
        s.style.set_alert(button, false);
        set_text(button, label);
        unsafe {
            let _ = InvalidateRect(Some(button), None, false);
            let _ = KillTimer(Some(window_of(s)), ARM_TIMER);
        }
    }
}

/// A control's left, top and height, in the page's coordinates.
fn place_on_page(s: &State, control: HWND) -> (i32, i32, i32) {
    let mut rect = RECT::default();
    unsafe {
        let _ = GetWindowRect(control, &mut rect);
        let mut top_left = POINT { x: rect.left, y: rect.top };
        let _ = ScreenToClient(s.content, &mut top_left);
        (top_left.x, top_left.y, rect.bottom - rect.top)
    }
}

/// Makes a control on a page this tall, and moves everything below it to suit, so that a card is
/// always the size of what's in it.
fn resize_on_page(s: &mut State, control: HWND, height: i32) {
    let Some(page) = s.pages.iter().position(|controls| controls.contains(&control)) else { return };
    let (_, top, old) = place_on_page(s, control);
    let delta = height - old;
    if delta == 0 {
        return;
    }
    let bottom = top + old;
    let mut rect = RECT::default();
    unsafe {
        let _ = GetWindowRect(control, &mut rect);
        let _ = SetWindowPos(control, None, 0, 0, rect.right - rect.left, height, SWP_NOMOVE | SWP_NOZORDER | SWP_NOACTIVATE);
    }
    // The text view shares the page but not the list, so it stays still when the list changes.
    let list_only = control == s.controls.hotkey_list;
    for other in s.pages[page].clone() {
        if other == control || (list_only && s.text_controls.contains(&other)) {
            continue;
        }
        let (x, other_top, _) = place_on_page(s, other);
        if other_top >= bottom {
            unsafe {
                let _ = SetWindowPos(other, None, x, other_top + delta, 0, 0, SWP_NOSIZE | SWP_NOZORDER | SWP_NOACTIVATE);
            }
        }
    }
    let controls = s.pages[page].clone();
    s.style.shift_cards(&controls, bottom, delta);
    if page == PAGE_HOTKEYS {
        s.hotkeys_list_bottom += delta;
        if !list_only {
            s.hotkeys_text_bottom += delta;
        }
        s.page_heights[page] = if s.text_mode { s.hotkeys_text_bottom } else { s.hotkeys_list_bottom };
    } else {
        s.page_heights[page] += delta;
    }
    if page == s.page {
        refit_page(s);
    }
}

/// The page's height changed: sizes the moving window to suit, and stays where it was.
fn refit_page(s: &mut State) {
    let view = view_height(s);
    let scroll = forms::clamp_scroll(s.scroll, page_height(s), view);
    s.scroll = scroll;
    let mut bounds = RECT::default();
    unsafe {
        let _ = GetWindowRect(s.content, &mut bounds);
        let _ = SetWindowPos(s.content, None, 0, -scroll, bounds.right - bounds.left, page_height(s).max(view), SWP_NOZORDER | SWP_NOACTIVATE);
        let _ = InvalidateRect(Some(s.content), None, false);
        let _ = InvalidateRect(Some(s.view), None, false);
    }
}

/// Builds the window again from `settings`, in the same place and on the same page, if it's open.
/// For a new colour scheme or screen, which its controls take when they're made, and for settings
/// replaced from outside, which the form would otherwise overwrite with its older values.
pub fn reopen(context: EditorContext, settings: Settings, dir: std::path::PathBuf, apps: &[crate::indexer::AppEntry]) {
    let Some(old) = CURRENT.with(|c| c.get()) else { return };
    let Some(s) = state(old) else { return };
    let (page, scroll, retry) = (s.page, s.scroll, s.save_pending);
    // Text being edited isn't in `settings`, and may not make sense yet. It's still the user's.
    let draft = s.text_mode.then(|| (get_text(s.controls.hyper_table), get_text(s.controls.shortcuts)));
    let focus = unsafe { GetFocus() };
    let focus_at = s.pages.get(page).and_then(|controls| controls.iter().position(|control| *control == focus));
    let in_front = unsafe { GetForegroundWindow() } == old;
    let minimized = unsafe { IsIconic(old) }.as_bool();
    let mut rect = RECT::default();
    unsafe {
        let _ = GetWindowRect(old, &mut rect);
        // A minimized window is parked far off the screen, and shouldn't come back there.
        if minimized {
            let mut placement = WINDOWPLACEMENT { length: std::mem::size_of::<WINDOWPLACEMENT>() as u32, ..Default::default() };
            if GetWindowPlacement(old, &mut placement).is_ok() {
                rect = placement.rcNormalPosition;
            }
        }
        let _ = DestroyWindow(old);
    }
    START_PAGE.with(|start| start.set(page));
    NEXT_PLACE.with(|place| place.set(Some(rect)));
    // Something the launcher did shouldn't pull Settings in front of it.
    QUIET_OPEN.with(|quiet| quiet.set(!in_front));
    open(context, settings, dir, apps);
    let Some(hwnd) = CURRENT.with(|c| c.get()) else { return };
    let Some(s) = state(hwnd) else { return };
    select_page(s, page);
    if let Some((hyper, shortcuts)) = draft {
        toggle_hotkey_text(s);
        set_text(s.controls.hyper_table, &hyper);
        set_text(s.controls.shortcuts, &shortcuts);
        let _ = save_now(s);
    }
    s.save_pending = retry;
    // It's still true, and this is a new window.
    if retry {
        show_problem(s, SAVE_FAILED);
    }
    scroll_to(s, scroll);
    unsafe {
        if in_front {
            if let Some(control) = focus_at.and_then(|at| s.pages.get(page).and_then(|controls| controls.get(at))) {
                forms::focus(*control);
            }
        }
        if minimized {
            let _ = ShowWindow(hwnd, SW_SHOWMINNOACTIVE);
        }
    }
}

/// Whether this is the Settings window.
pub fn is_settings(hwnd: HWND) -> bool {
    CURRENT.with(|c| c.get()) == Some(hwnd)
}

pub fn is_open() -> bool {
    CURRENT.with(|c| c.get()).is_some()
}

/// Reads the hotkey tables, when they're being edited as text.
fn parse_hotkey_text(state: &State, launcher: Option<&Chord>) -> Result<(Vec<orbit_core::settings::HyperBinding>, Vec<orbit_core::settings::ShortcutBinding>), String> {
    let hyper = parse_hyper(&get_lines(state.controls.hyper_table)).map_err(|e| format!("HyperKey: {e}"))?;
    let direct = parse_shortcuts(&get_lines(state.controls.shortcuts)).map_err(|e| format!("Shortcuts: {e}"))?;
    for binding in &direct {
        let name = binding.chord.display("Win");
        if is_reserved_on_windows(&binding.chord) {
            return Err(format!("{name} belongs to Windows."));
        }
        if !is_usable_global(&binding.chord) {
            return Err(format!("{name} needs Ctrl, Alt or Win, so that typing still works."));
        }
        if launcher == Some(&binding.chord) {
            return Err(format!("{name} already opens Orbit."));
        }
    }
    Ok((hyper, direct))
}

/// Reads the form into a new `Settings`, and says what couldn't be used. Text that doesn't make
/// sense keeps its last good value.
fn collect(state: &State) -> (Settings, Option<String>) {
    let c = &state.controls;
    let mut s = state.base.clone();
    let mut problem = None;

    s.launcher.hotkey = state.launcher_chord.clone();
    s.launch_at_login = is_checked(c.login);
    s.launcher.focus_running_app = is_checked(c.focus_running);
    s.launcher.hotkey_toggles_app = is_checked(c.toggles_app);
    s.launcher.preserve_query = is_checked(c.preserve);
    s.launcher.favorites_only = is_checked(c.favorites_only);
    s.integrations.one_password = is_checked(c.op_on);
    s.integrations.ente_auth = is_checked(c.ente_on);
    s.mac_feel.enabled = is_checked(c.mac_on);
    s.mac_feel.text_keys = is_checked(c.mac_text);
    s.mac_feel.app_keys = is_checked(c.mac_apps);
    s.mac_feel.excluded_apps = state.pickers[PICKER_MAC].chosen.clone();
    s.launcher.glass = is_checked(c.glass);
    s.launcher.fit_results = is_checked(c.fit_results);
    s.launcher.pass_through_in_games = is_checked(c.launcher_games);
    s.launcher.excluded_apps = state.pickers[PICKER_LAUNCHER].chosen.clone();
    s.appearance = APPEARANCE[selected(c.appearance).min(APPEARANCE.len() - 1)].1;
    s.launcher.keyboard = KEYBOARD[selected(c.keyboard).min(KEYBOARD.len() - 1)].1;
    if let Some(screen) = screen_picker::choice(c.screen) {
        s.launcher.screen = screen;
    }
    s.low_resource_mode = is_checked(c.low_resource);
    s.fetch_link_icons = is_checked(c.link_icons);
    s.fetch_exchange_rates = is_checked(c.exchange_rates);
    s.launcher.copy_formatted = is_checked(c.copy_formatted);
    s.fetch_weather = is_checked(c.weather);

    if state.text_mode {
        match parse_hotkey_text(state, launcher_chord_of(state).as_ref()) {
            Ok((hyper, direct)) => {
                s.hyper.bindings = hyper;
                s.shortcuts = direct;
            }
            Err(message) => problem = Some(message),
        }
    }

    s.hyper.enabled = is_checked(c.hyper_on);
    // A key this list doesn't offer, set in settings.json, stays unless another is picked.
    let trigger_index = selected(c.hyper_trigger).min(TRIGGERS.len() - 1);
    if trigger_index != 0 || TRIGGERS.iter().any(|t| t.eq_ignore_ascii_case(&s.hyper.trigger)) {
        s.hyper.trigger = TRIGGERS[trigger_index].to_string();
    }
    s.hyper.mode = MODES[selected(c.hyper_mode).min(MODES.len() - 1)].1;
    s.hyper.tap = TAPS[selected(c.hyper_tap).min(TAPS.len() - 1)].1;
    s.hyper.tap_timeout_ms = state.presets[PRESET_TAP].value();
    s.hyper.pass_through_in_games = is_checked(c.hyper_games);
    s.hyper.excluded_apps = state.pickers[PICKER_HYPER].chosen.clone();

    s.clipboard.enabled = is_checked(c.clip_on);
    s.clipboard.persist = is_checked(c.clip_persist);
    s.clipboard.capture_images = is_checked(c.clip_images);
    s.clipboard.max_items = state.presets[PRESET_ITEMS].value();
    s.clipboard.max_item_bytes = state.presets[PRESET_SIZE].value();
    s.clipboard.retention_days = state.presets[PRESET_DAYS].value();
    s.clipboard.excluded_apps = state.pickers[PICKER_CLIPBOARD].chosen.clone();

    s.snippets.expansion_enabled = is_checked(c.expansion);
    s.snippets.excluded_apps = state.pickers[PICKER_SNIPPETS].chosen.clone();

    // Links, commands and emoji are kept by the launcher, which changes them at once.
    if state.unhide_all {
        s.hidden.clear();
    }
    (s, problem)
}

fn post_builtin(main: SendHwnd, command: Builtin) {
    if let Some(index) = Builtin::ALL.iter().position(|entry| entry.0 == command) {
        main.post(WM_ORBIT_RUN_BUILTIN, index, 0);
    }
}

/// For a button that has already had its second press, so Orbit needn't ask again.
fn post_confirmed(main: SendHwnd, command: Builtin) {
    if let Some(index) = Builtin::ALL.iter().position(|entry| entry.0 == command) {
        main.post(WM_ORBIT_RUN_BUILTIN, index, 1);
    }
}

fn pick_import_file(owner: HWND) -> Option<std::path::PathBuf> {
    pick_file(owner, "Orbit export (*.json)\0*.json\0All files\0*.*\0")
}

fn pick_file(owner: HWND, filter: &str) -> Option<std::path::PathBuf> {
    let mut buffer = vec![0u16; 1024];
    let filter = wide(filter);
    let mut dialog = OPENFILENAMEW {
        lStructSize: std::mem::size_of::<OPENFILENAMEW>() as u32,
        hwndOwner: owner,
        lpstrFilter: PCWSTR(filter.as_ptr()),
        lpstrFile: PWSTR(buffer.as_mut_ptr()),
        nMaxFile: buffer.len() as u32,
        Flags: OFN_FILEMUSTEXIST | OFN_PATHMUSTEXIST,
        ..Default::default()
    };
    unsafe { GetOpenFileNameW(&mut dialog) }.as_bool().then(|| {
        let end = buffer.iter().position(|u| *u == 0).unwrap_or(0);
        std::path::PathBuf::from(String::from_utf16_lossy(&buffer[..end]))
    })
}

fn draw_page_item(s: &State, item: &DRAWITEMSTRUCT) {
    let style = &s.style;
    let theme = &style.theme;
    let (width, height) = (item.rcItem.right - item.rcItem.left, item.rcItem.bottom - item.rcItem.top);
    let Some(mut buffer) = Backbuffer::new(width, height) else { return };
    let px = |value: i32| forms::scale(value, style.dpi);
    buffer.fill(rail_color(theme));
    let Some((name, glyph)) = PAGES.get(item.itemID as usize) else { return };
    let chosen = item.itemState.0 & ODS_SELECTED.0 != 0;
    if chosen {
        buffer.fill_round_rect(0, px(2), width, height - px(4), px(forms::RADIUS_SELECTION) as f32, if theme.high_contrast { theme.selection } else if theme.background.0 < 128 { theme.selection.mix(rail_color(theme), 0.15) } else { theme.card });
    }
    let size = px(PAGE_BADGE_DIP);
    let color = PAGE_COLORS.get(item.itemID as usize).copied().unwrap_or(theme.secondary);
    forms::draw_badge(&mut buffer, theme, style.icon_small, px(10), (height - size) / 2, size, color, *glyph);
    let text = RECT { left: px(10) + size + px(10), top: 0, right: width - px(6), bottom: height };
    buffer.text(name, style.font, theme.text, text, false);
    buffer.copy_to_dc_at(item.hDC, item.rcItem.left, item.rcItem.top);
}

/// Room at the foot of the page list for the version, under any message.
const FOOT_DIP: i32 = 40;

/// The side rail's colour: a step off the page's, as a sidebar is.
fn rail_color(theme: &crate::ui::Theme) -> crate::gfx::Rgb {
    if theme.high_contrast {
        theme.sidebar
    } else if theme.background.0 < 128 {
        theme.sidebar.mix(theme.card, 0.6)
    } else {
        theme.sidebar.mix(theme.text, 0.035)
    }
}

/// Where the window's Minimize and Close are, in a window this wide.
fn caption_buttons(dpi: u32, width: i32) -> [RECT; 2] {
    let px = |value: i32| forms::scale(value, dpi);
    let (button, height) = (px(CAPTION_BUTTON_DIP), px(CAPTION_HEIGHT_DIP));
    [
        RECT { left: width - button * 2, top: 0, right: width - button, bottom: height },
        RECT { left: width - button, top: 0, right: width, bottom: height },
    ]
}

fn caption_button_at(dpi: u32, width: i32, x: i32, y: i32) -> Option<usize> {
    caption_buttons(dpi, width).iter().position(|place| x >= place.left && x < place.right && y >= place.top && y < place.bottom)
}

fn redraw_caption(hwnd: HWND, dpi: u32) {
    let mut client = RECT::default();
    unsafe {
        let _ = GetClientRect(hwnd, &mut client);
    }
    let [minimize, close] = caption_buttons(dpi, client.right);
    let both = RECT { left: minimize.left, top: 0, right: close.right, bottom: close.bottom };
    unsafe {
        let _ = InvalidateRect(Some(hwnd), Some(&both), false);
    }
}

/// What the window draws itself, around its pages: the side rail with its title and the version,
/// and Minimize and Close at the top right.
fn paint_chrome(s: &State, buffer: &mut Backbuffer, width: i32, height: i32) {
    let style = &s.style;
    let theme = &style.theme;
    let px = |value: i32| forms::scale(value, style.dpi);
    let rail = px(SIDEBAR_DIP);
    let line = 1.max(style.dpi as i32 / 96);
    buffer.fill_rect(0, 0, rail, height, rail_color(theme));
    buffer.fill_rect(rail - line, 0, line, height, theme.divider);
    buffer.text("Settings", style.title, theme.text, RECT { left: px(22), top: px(10), right: rail - px(8), bottom: px(TOPBAR_DIP + 2) }, false);
    let version = format!("Orbit {}", env!("CARGO_PKG_VERSION"));
    buffer.text(&version, style.small, theme.tertiary, RECT { left: px(24), top: height - px(FOOT_DIP - 6), right: rail - px(8), bottom: height - px(10) }, false);
    for (at, place) in caption_buttons(style.dpi, width).iter().enumerate() {
        let hover = s.caption_hover == Some(at);
        let pressed = hover && s.caption_press == Some(at);
        let closing = at == 1 && hover && !theme.high_contrast;
        if hover {
            let fill = if closing { crate::gfx::Rgb(196, 43, 28) } else { theme.selection };
            let fill = if pressed { fill.mix(theme.sidebar, 0.25) } else { fill };
            buffer.fill_rect(place.left, place.top, place.right - place.left, place.bottom - place.top, fill);
        }
        let glyph = if at == 0 { "\u{E921}" } else { "\u{E8BB}" };
        let ink = if closing { crate::gfx::Rgb(255, 255, 255) } else if hover { theme.text } else { theme.secondary };
        buffer.text_centered(glyph, style.icon_small, ink, *place);
    }
}

/// Where a window's left edge is, in its parent's coordinates.
fn window_left(control: HWND) -> i32 {
    unsafe {
        let mut place = RECT::default();
        let _ = GetWindowRect(control, &mut place);
        let mut origin = POINT { x: place.left, y: place.top };
        if let Ok(parent) = GetParent(control) {
            let _ = ScreenToClient(parent, &mut origin);
        }
        place.left - origin.x
    }
}

/// What a row of the hotkey list runs.
fn hotkey_row_action(s: &State, row: HotkeyRow) -> Option<Action> {
    match row {
        HotkeyRow::Match(option) => s.action_options.get(option).map(|option| option.action.clone()),
        HotkeyRow::Pending => match &s.hotkey_target {
            Some(RecordTarget::New(action)) => Some(action.clone()),
            _ => None,
        },
        other => other.binding_action(&s.base),
    }
}

/// A row's picture: an app's own icon if it has one, or otherwise what the launcher would show.
fn action_tile(action: &Action) -> (Option<u64>, Tile) {
    match action {
        Action::FocusOrLaunchApp { id, name } | Action::LaunchApp { id, name } => (Some(crate::indexer::icon_hash(id)), forms::letter_tile(name)),
        Action::OpenPath { .. } => (None, Tile::Glyph(GLYPH_FOLDER)),
        Action::Url { .. } => (None, Tile::Glyph(GLYPH_LINK)),
        Action::Builtin { command } => (None, Tile::Glyph(crate::catalog::glyph_for_builtin(*command))),
        Action::Custom { .. } => (None, Tile::Glyph(GLYPH_COMMAND)),
        Action::OpenNote { .. } => (None, Tile::Glyph(GLYPH_NOTE)),
        Action::InsertSnippet { .. } => (None, Tile::Glyph(GLYPH_SNIPPET)),
    }
}

/// The small rounded square for a row with no icon of its own. `place` is left, top and size.
fn draw_small_tile(style: &Style, buffer: &mut Backbuffer, place: (i32, i32, i32), tile: Tile, fill: crate::gfx::Rgb) {
    let (x, y, size) = place;
    let radius = forms::scale(5, style.dpi) as f32;
    let area = RECT { left: x, top: y, right: x + size, bottom: y + size };
    // A command has the same coloured tile as in the launcher, except in high contrast.
    if let Tile::Glyph(glyph) = tile
        && !style.theme.high_contrast
    {
        let (light, deep) = crate::rows::tile_colors(glyph);
        buffer.fill_tile(x, y, size, radius, (style.dpi as f32 / 96.0).max(1.0), light, deep);
        buffer.symbol_centered(&glyph.to_string(), style.icon_small, forms::scale(13, style.dpi), true, crate::gfx::Rgb(255, 255, 255), area);
        return;
    }
    buffer.fill_round_rect(x, y, size, size, radius, fill);
    match tile {
        Tile::Glyph(glyph) => buffer.symbol_centered(&glyph.to_string(), style.icon_small, forms::scale(13, style.dpi), false, style.theme.secondary, area),
        Tile::Letter(letter) => buffer.text_centered(&letter.to_string(), style.small, style.theme.secondary, area),
    }
}

fn icon_pixels(s: &mut State, hash: u64, size: usize) -> Option<Vec<u8>> {
    let dir = &s.dir;
    s.icons.entry(hash).or_insert_with(|| crate::worker::load_icon(dir, hash, size)).clone()
}

fn draw_hotkey_item(s: &mut State, item: &DRAWITEMSTRUCT) {
    let (width, height) = (item.rcItem.right - item.rcItem.left, item.rcItem.bottom - item.rcItem.top);
    let Some(mut buffer) = Backbuffer::new(width, height) else { return };
    let dpi = s.style.dpi;
    let px = |value: i32| forms::scale(value, dpi);
    let at = item.itemID as usize;
    let Some(row) = s.hotkey_rows.get(at).copied() else {
        buffer.fill(s.style.theme.card);
        buffer.copy_to_dc_at(item.hDC, item.rcItem.left, item.rcItem.top);
        return;
    };
    let tile_size = px(PICKER_ICON_DIP);
    let (icon, tile) = hotkey_row_action(s, row).as_ref().map_or((None, Tile::Glyph(GLYPH_COMMAND)), action_tile);
    let pixels = icon.and_then(|hash| icon_pixels(s, hash, tile_size as usize));
    let s: &State = s;
    let style = &s.style;
    let theme = &style.theme;
    buffer.fill(theme.card);
    let focused = unsafe { GetFocus() } == s.controls.hotkey_list;
    let searching = matches!(row, HotkeyRow::Match(_));
    let chosen = item.itemState.0 & ODS_SELECTED.0 != 0 && (focused || searching);
    let hover = s.hotkey_hover.filter(|h| h.0 == at).map(|h| h.1);
    if row.is_header() {
        let hyper_off = matches!(row, HotkeyRow::HyperHeader) && !is_checked(s.controls.hyper_on);
        let (text, ink) = match row {
            HotkeyRow::HyperHeader if hyper_off => (HYPER_OFF_ROW, theme.accent),
            HotkeyRow::HyperHeader => ("HyperKey", theme.secondary),
            _ => ("Shortcuts", theme.secondary),
        };
        // The one heading that does something looks like it.
        if hyper_off && hover.is_some() {
            buffer.fill_round_rect(0, px(2), width, height - px(4), px(forms::RADIUS_SELECTION) as f32, theme.selection.mix(theme.card, 0.5));
        }
        buffer.text(text, style.small, ink, RECT { left: px(8), top: height / 3, right: width - px(8), bottom: height }, false);
        buffer.copy_to_dc_at(item.hDC, item.rcItem.left, item.rcItem.top);
        return;
    }
    let listening = match &s.hotkey_target {
        Some(RecordTarget::Row(target)) => *target == row,
        Some(RecordTarget::New(_)) => matches!(row, HotkeyRow::Pending),
        None => false,
    };
    if chosen || listening {
        buffer.fill_round_rect(0, px(2), width, height - px(4), px(forms::RADIUS_SELECTION) as f32, theme.selection);
    } else if hover.is_some() {
        buffer.fill_round_rect(0, px(2), width, height - px(4), px(forms::RADIUS_SELECTION) as f32, theme.selection.mix(theme.card, 0.5));
    }
    let (tile_x, tile_y) = (px(8), (height - tile_size) / 2);
    match pixels {
        Some(pixels) => buffer.blit_premultiplied(tile_x, tile_y, tile_size, &pixels),
        None => {
            let fill = if chosen || listening { theme.keycap_on_selection } else { theme.keycap };
            draw_small_tile(style, &mut buffer, (tile_x, tile_y, tile_size), tile, fill);
        }
    }
    let text_left = tile_x + tile_size + px(10);
    if let HotkeyRow::Match(option) = row {
        let Some(option) = s.action_options.get(option) else { return };
        let name_width = buffer.text_width(&option.name, style.font);
        let right = (text_left + name_width).min(width - px(170));
        buffer.text(&option.name, style.font, theme.text, RECT { left: text_left, top: 0, right, bottom: height }, false);
        buffer.text(option.kind, style.small, theme.secondary, RECT { left: right + px(10), top: 0, right: width - px(90), bottom: height }, false);
        if chosen || hover.is_some() {
            buffer.text("Choose", style.small, theme.accent, RECT { left: width - px(90), top: 0, right: width - px(10), bottom: height }, true);
        }
        buffer.copy_to_dc_at(item.hDC, item.rcItem.left, item.rcItem.top);
        return;
    }
    let (keys, name, on) = match row {
        HotkeyRow::Hyper(i) => {
            let binding = &s.base.hyper.bindings[i];
            (format!("Hyper+{}", binding.key), hotkeys::describe(&s.base, &binding.action), binding.enabled && is_checked(s.controls.hyper_on))
        }
        HotkeyRow::Direct(i) => {
            let binding = &s.base.shortcuts[i];
            (binding.chord.display("Win"), hotkeys::describe(&s.base, &binding.action), binding.enabled)
        }
        _ => {
            let name = match &s.hotkey_target {
                Some(RecordTarget::New(action)) => hotkeys::describe(&s.base, action),
                _ => String::new(),
            };
            (String::new(), name, true)
        }
    };
    if listening {
        let (message, ink) = match &s.hotkey_problem {
            Some(problem) if s.hotkey_asking.is_some() => (problem.as_str(), theme.text),
            Some(problem) => (problem.as_str(), theme.danger),
            None if is_checked(s.controls.hyper_on) => ("Any shortcut works. One key on its own means Hyper with it. Esc cancels.", theme.secondary),
            None => ("Esc cancels.", theme.secondary),
        };
        // The same field as the launcher's shortcut, where the key caps are about to be. While asking
        // whether to replace something, it's the keys in question.
        let asked = s.hotkey_asking.as_ref().map(|hotkey| hotkey.label("Win"));
        let shown = if s.held_keys.is_empty() { asked.as_deref().unwrap_or("") } else { s.held_keys.as_str() };
        let held_width = if shown.is_empty() { 0 } else { crate::ui::keycaps_width(&buffer, shown, style.small, style.icon_small, style.dpi) };
        let field_width = px(HOTKEY_FIELD_DIP).max(held_width + px(16));
        let (field_x, field_y, field_height) = (width - px(8) - field_width, (height - px(30)) / 2, px(30));
        let radius = px(forms::RADIUS_FIELD) as f32;
        let inset = 1.max(style.dpi as i32 / 96);
        buffer.fill_round_rect(field_x, field_y, field_width, field_height, radius, theme.accent);
        buffer.fill_round_rect(field_x + inset, field_y + inset, field_width - inset * 2, field_height - inset * 2, radius - inset as f32, theme.field);
        if shown.is_empty() {
            buffer.text("Press keys\u{2026}", style.font, theme.accent, RECT { left: field_x + px(10), top: field_y, right: field_x + field_width - px(4), bottom: field_y + field_height }, false);
        } else {
            crate::ui::draw_keycaps(&mut buffer, shown, field_x + px(8), height / 2, style.small, style.icon_small, (theme.keycap, 255), theme.text, style.dpi);
        }
        // Two lines, so that the message has the width of the row whatever the name is.
        let (text_right, middle) = (field_x - px(10), height / 2);
        // A hint can go, but never how to get out of this.
        let message = if s.hotkey_problem.is_none() && buffer.text_width(message, style.small) > text_right - text_left { "Esc cancels." } else { message };
        buffer.text(&name, style.font, theme.text, RECT { left: text_left, top: middle - px(18), right: text_right, bottom: middle + px(1) }, false);
        buffer.text(message, style.small, ink, RECT { left: text_left, top: middle + px(1), right: text_right, bottom: middle + px(18) }, false);
        buffer.copy_to_dc_at(item.hDC, item.rcItem.left, item.rcItem.top);
        return;
    }
    let (caps, remove) = hotkey_spans(s, &keys, width);
    let caps_x = caps.0;
    let armed = s.hotkey_armed == Some(at);
    let remove_showing = armed || chosen || hover.is_some();
    let text = RECT { left: text_left, top: 0, right: if remove_showing { remove.0 - px(8) } else { caps_x - px(8) }, bottom: height };
    buffer.text(&name, style.font, if on { theme.text } else { theme.secondary }, text, false);
    let plain = if chosen { theme.keycap_on_selection } else { theme.keycap };
    // The key caps are the part to click to change them.
    let fill = if hover == Some(ListPart::Edit) { plain.mix(theme.text, 0.15) } else { plain };
    crate::ui::draw_keycaps(&mut buffer, &keys, caps_x, height / 2, style.small, style.icon_small, (fill, 255), if on { theme.text } else { theme.secondary }, style.dpi);
    style.draw_row_remove(&mut buffer, remove, height, armed, hover, chosen);
    buffer.copy_to_dc_at(item.hDC, item.rcItem.left, item.rcItem.top);
}

unsafe extern "system" fn settings_proc(hwnd: HWND, message: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    unsafe {
        if message == WM_ERASEBKGND && state(hwnd).is_none() {
            if let Some((rail, page, divider)) = EARLY_FILL.with(|fill| fill.get()) {
                // The same layout the finished window has, without its words, so nothing jumps.
                let mut client = RECT::default();
                let _ = GetClientRect(hwnd, &mut client);
                let dpi = OPEN_DPI.with(|open_dpi| open_dpi.get());
                let edge = forms::scale(SIDEBAR_DIP, dpi);
                let line = 1.max(dpi as i32 / 96);
                let dc = HDC(wparam.0 as *mut _);
                for (color, area) in [
                    (rail, RECT { right: edge - line, ..client }),
                    (divider, RECT { left: edge - line, right: edge, ..client }),
                    (page, RECT { left: edge, ..client }),
                ] {
                    let brush = CreateSolidBrush(color.colorref());
                    FillRect(dc, &area, brush);
                    let _ = DeleteObject(brush.into());
                }
                return LRESULT(1);
            }
        }
        // No frame of Windows' own: the strip across the top is part of the page, and drawn here.
        if message == WM_NCCALCSIZE && wparam.0 != 0 && !forms::is_preview() {
            return LRESULT(0);
        }
        // Without a frame there's nothing to repaint when the window gains or loses the focus.
        if message == WM_NCACTIVATE && !forms::is_preview() {
            return DefWindowProcW(hwnd, message, wparam, LPARAM(-1));
        }
        if message == WM_MEASUREITEM {
            // Sent while the lists are created, before the window has any state.
            let item = &mut *(lparam.0 as *mut MEASUREITEMSTRUCT);
            let id = item.CtlID as i32;
            let dip = if id == ID_PAGES {
                PAGE_ROW_DIP
            } else if (ID_SNIPPET_LIST..=ID_COMMAND_LIST).contains(&id) {
                forms::MANAGED_ROW_DIP
            } else if id >= ID_PICKERS {
                PICKER_ROW_DIP
            } else {
                HOTKEY_ROW_DIP
            };
            item.itemHeight = forms::scale(dip, OPEN_DPI.with(|dpi| dpi.get())) as u32;
            return LRESULT(1);
        }
        if let Some(s) = state(hwnd) {
            match message {
                WM_PAINT => {
                    let mut client = RECT::default();
                    let _ = GetClientRect(hwnd, &mut client);
                    let s: &State = s;
                    s.style.paint(hwnd, |buffer| paint_chrome(s, buffer, client.right, client.bottom));
                    return LRESULT(0);
                }
                WM_NCHITTEST => {
                    let mut point = POINT { x: (lparam.0 & 0xFFFF) as i16 as i32, y: ((lparam.0 >> 16) & 0xFFFF) as i16 as i32 };
                    let _ = ScreenToClient(hwnd, &mut point);
                    let mut client = RECT::default();
                    let _ = GetClientRect(hwnd, &mut client);
                    let on_strip = point.y >= 0 && point.y < forms::scale(TOPBAR_DIP, s.style.dpi);
                    let on_button = caption_button_at(s.style.dpi, client.right, point.x, point.y).is_some();
                    return LRESULT(if on_strip && !on_button { HTCAPTION as isize } else { HTCLIENT as isize });
                }
                WM_MOUSEMOVE => {
                    let mut client = RECT::default();
                    let _ = GetClientRect(hwnd, &mut client);
                    let hover = caption_button_at(s.style.dpi, client.right, (lparam.0 & 0xFFFF) as i16 as i32, ((lparam.0 >> 16) & 0xFFFF) as i16 as i32);
                    if hover != s.caption_hover {
                        s.caption_hover = hover;
                        redraw_caption(hwnd, s.style.dpi);
                    }
                    if !s.caption_tracking {
                        s.caption_tracking = true;
                        let mut track = TRACKMOUSEEVENT { cbSize: std::mem::size_of::<TRACKMOUSEEVENT>() as u32, dwFlags: TME_LEAVE, hwndTrack: hwnd, dwHoverTime: 0 };
                        let _ = TrackMouseEvent(&mut track);
                    }
                }
                WM_MOUSELEAVE => {
                    s.caption_tracking = false;
                    if s.caption_hover.take().is_some() || s.caption_press.take().is_some() {
                        redraw_caption(hwnd, s.style.dpi);
                    }
                }
                WM_LBUTTONUP => {
                    let pressed = s.caption_press.take();
                    if pressed.is_some() {
                        let _ = ReleaseCapture();
                        redraw_caption(hwnd, s.style.dpi);
                    }
                    if pressed.is_some() && pressed == s.caption_hover {
                        let command = if pressed == Some(0) { SC_MINIMIZE } else { SC_CLOSE };
                        let _ = PostMessageW(Some(hwnd), WM_SYSCOMMAND, WPARAM(command as usize), LPARAM(0));
                    }
                }
                // Controls take their size from the screen when they're made, so a new screen means
                // making them again, once the window has stopped moving.
                WM_LBUTTONDOWN => {
                    if let Some(button) = s.caption_hover {
                        s.caption_press = Some(button);
                        SetCapture(hwnd);
                        redraw_caption(hwnd, s.style.dpi);
                        return LRESULT(0);
                    }
                    clicked_away(s)
                }
                WM_ENTERSIZEMOVE => s.moving = true,
                WM_DISPLAYCHANGE => screen_picker::refresh(s.controls.screen),
                WM_EXITSIZEMOVE => {
                    s.moving = false;
                    if std::mem::take(&mut s.dpi_change_waiting) {
                        s.context.main.post(WM_ORBIT_SETTINGS_REOPEN, 0, 0);
                    }
                }
                WM_DPICHANGED => {
                    if s.moving {
                        s.dpi_change_waiting = true;
                    } else {
                        s.context.main.post(WM_ORBIT_SETTINGS_REOPEN, 0, 0);
                    }
                    return LRESULT(0);
                }
                WM_DRAWITEM => {
                    let item = &*(lparam.0 as *const DRAWITEMSTRUCT);
                    match item.CtlID as i32 {
                        ID_PAGES => draw_page_item(s, item),
                        ID_HOTKEY_LIST => draw_hotkey_item(s, item),
                        ID_SEARCH_LIST => draw_search_item(s, item),
                        id if (ID_SNIPPET_LIST..=ID_COMMAND_LIST).contains(&id) => draw_managed_item(s, (id - ID_SNIPPET_LIST) as usize, item),
                        id if id >= ID_PICKERS => draw_picker_item(s, ((id - ID_PICKERS) / PICKER_IDS) as usize, item),
                        _ => {
                            let _ = s.style.handle(hwnd, message, wparam, lparam);
                        }
                    }
                    return LRESULT(1);
                }
                WM_CTLCOLORLISTBOX if HWND(lparam.0 as *mut _) == s.list => {
                    return LRESULT(s.style.rail_brush().unwrap_or(s.style.side).0 as isize);
                }
                // From over the page list: the page still scrolls.
                WM_MOUSEWHEEL => return SendMessageW(s.view, message, Some(wparam), Some(lparam)),
                _ => {}
            }
            // Handled messages that also need to reach the `match` below return `None` here.
            if let Some(result) = s.style.handle(hwnd, message, wparam, lparam) {
                return result;
            }
        }
        match message {
            WM_COMMAND => {
                let id = (wparam.0 & 0xFFFF) as i32;
                let notification = (wparam.0 >> 16) as u32;
                let Some(s) = state(hwnd) else { return LRESULT(0) };
                match id {
                    id if id >= ID_PICKERS => {
                        let (index, part) = (((id - ID_PICKERS) / PICKER_IDS) as usize, (id - ID_PICKERS) % PICKER_IDS);
                        if part == 0 && notification == EN_CHANGE {
                            refresh_picker(s, index);
                        }
                        if part == 1 && notification == BN_CLICKED {
                            if let Some(program) = pick_program_file(hwnd) {
                                if let Some(picker) = s.pickers.get_mut(index) {
                                    if !picker.chosen.iter().any(|entry| entry.eq_ignore_ascii_case(&program)) {
                                        picker.chosen.push(program.clone());
                                    }
                                    // A search left in the box would hide what's just been added.
                                    let edit = picker.edit;
                                    set_text(edit, "");
                                }
                                show_added_app(s, index, &program);
                                save_soon(s, 1);
                            }
                        }
                    }
                    ID_PAGES if notification == LBN_SELCHANGE => {
                        let page = SendMessageW(s.list, LB_GETCURSEL, None, None).0.max(0) as usize;
                        disarm_button(s);
                        show_page(s, page);
                    }
                    // Enter presses a button that has the focus, as it would in a keyboard-first app.
                    // Otherwise it just means "that's it", in a window that has no main button.
                    x if x == IDOK.0 => {
                        let focus = GetFocus();
                        if is_of_class(focus, "button") {
                            SendMessageW(focus, BM_CLICK, None, None);
                        } else {
                            let _ = save_now(s);
                        }
                    }
                    // Backing out of a first press comes before leaving.
                    x if x == IDCANCEL.0 && s.armed_button.is_some() => disarm_button(s),
                    x if x == IDCANCEL.0 => save_and_close(s, hwnd),
                    ID_LINK_NEW => request_link_action(s, LinkRequest::NewLink),
                    ID_COMMAND_NEW => request_link_action(s, LinkRequest::NewCommand),
                    ID_SNIPPET_NEW => request_link_action(s, LinkRequest::NewSnippet),
                    ID_TOOL_OP => change_tool(s, Tool::OnePassword),
                    ID_TOOL_ENTE => change_tool(s, Tool::EnteAuth),
                    ID_ENTE_SIGN_IN => {
                        if let Err(reason) = integrations::open_sign_in(Tool::EnteAuth, s.context.main, WM_ORBIT_ENTE_ACCOUNT) {
                            show_problem(s, &reason);
                        }
                    }
                    ID_SUGGEST => add_suggested(s),
                    ID_ACTION_SEARCH if notification == EN_CHANGE => refresh_hotkey_list(s),
                    ID_SEARCH_FILTER if notification == EN_CHANGE => refresh_search_list(s),
                    ID_HOTKEY_TEXT => toggle_hotkey_text(s),
                    ID_CLEAR_CLIPS => {
                        if confirm_in_place(s, HWND(lparam.0 as *mut _), LABEL_CLEAR) {
                            post_confirmed(s.context.main, Builtin::ClearClipboard);
                            show_status(s, "Clearing history.", SAVED_MS);
                        }
                    }
                    ID_UNHIDE => {
                        s.unhide_all = true;
                    }
                    ID_RESET_SHORTCUTS => {
                        if confirm_in_place(s, HWND(lparam.0 as *mut _), LABEL_RESET) {
                            post_confirmed(s.context.main, Builtin::ResetShortcuts);
                        }
                    }
                    ID_DELETE_DATA => {
                        if confirm_in_place(s, HWND(lparam.0 as *mut _), LABEL_DELETE) {
                            post_confirmed(s.context.main, Builtin::DeleteLocalData);
                        }
                    }
                    ID_EXPORT => {
                        post_builtin(s.context.main, Builtin::ExportData);
                        show_status(s, "Exporting. Orbit will open the file when it's ready.", MESSAGE_MS);
                    }
                    ID_IMPORT => {
                        if let Some(path) = pick_import_file(hwnd) {
                            s.context.main.post(WM_ORBIT_IMPORT, crate::util::box_for_post(path), 0);
                            show_status(s, "Importing. Orbit will report the result.", MESSAGE_MS);
                        }
                    }
                    ID_OPEN_DIR => {
                        crate::launch::spawn(
                            crate::launch::LaunchRequest::open(&s.dir.to_string_lossy()),
                            s.context.main,
                            crate::app::WM_ORBIT_LAUNCH_FAILED,
                        );
                    }
                    ID_REINDEX => post_builtin(s.context.main, Builtin::RebuildAppIndex),
                    ID_DIAGNOSTICS => {
                        post_builtin(s.context.main, Builtin::Diagnostics);
                        save_and_close(s, hwnd);
                    }
                    _ => {}
                }
                // The window may have just closed. A search box isn't a setting: only the hotkey
                // tables have no id.
                if let Some(s) = state(hwnd) {
                    let control = HWND(lparam.0 as *mut _);
                    if notification == EN_CHANGE {
                        if id == 0 {
                            save_soon(s, TEXT_SAVE_DELAY_MS);
                        }
                    } else if notification == BN_CLICKED {
                        apply_dependents(s);
                        if control == s.controls.hyper_on {
                            refresh_hotkey_list(s);
                        }
                        let _ = save_now(s);
                    } else if notification == LBN_SELCHANGE && is_row_list(s, control) {
                        // By keyboard. A click stays put, or the row would move out from under it.
                        if GetKeyState(VK_LBUTTON.0 as i32) >= 0 {
                            reveal_list_row(s, control);
                        }
                    } else if notification == CBN_SELCHANGE {
                        if control == s.controls.keyboard {
                            let mode = KEYBOARD[selected(control).min(KEYBOARD.len() - 1)].1;
                            set_status(s.controls.keyboard_note, keyboard_description(mode));
                        }
                        if control == s.controls.screen_mode {
                            // Only a specific screen shows the picture. The card grows or closes up.
                            let screen = s.controls.screen;
                            screen_picker::set_mode(screen, selected(control));
                            resize_on_page(s, screen, screen_picker::height(screen));
                        }
                        // Arrowing through an open list isn't choosing. Closing it is, below.
                        if SendMessageW(control, CB_GETDROPPEDSTATE, None, None).0 == 0 {
                            let _ = save_now(s);
                        }
                    } else if notification == CBN_CLOSEUP {
                        save_soon(s, 1);
                    }
                }
                LRESULT(0)
            }
            WM_TIMER if wparam.0 == SAVE_TIMER => {
                if let Some(s) = state(hwnd) {
                    let _ = save_now(s);
                }
                LRESULT(0)
            }
            WM_TIMER if wparam.0 == STATUS_TIMER => {
                let _ = KillTimer(Some(hwnd), STATUS_TIMER);
                if let Some(s) = state(hwnd) {
                    // A failed write outlasts whatever came along since.
                    if s.save_pending {
                        show_problem(s, SAVE_FAILED);
                    } else {
                        set_foot_message(s, "");
                    }
                }
                LRESULT(0)
            }
            WM_TIMER if wparam.0 == ARM_TIMER => {
                if let Some(s) = state(hwnd) {
                    disarm_button(s);
                }
                LRESULT(0)
            }
            WM_SETTINGS_CANDIDATES => {
                if let Some(found) = crate::util::take_posted::<Vec<Candidate>>(wparam.0) {
                    if let Some(s) = state(hwnd) {
                        s.candidates = *found;
                        for index in 0..s.pickers.len() {
                            refresh_picker(s, index);
                        }
                    }
                }
                LRESULT(0)
            }
            WM_CLOSE => {
                match state(hwnd) {
                    Some(s) => save_and_close(s, hwnd),
                    None => {
                        let _ = DestroyWindow(hwnd);
                    }
                }
                LRESULT(0)
            }
            WM_DESTROY => {
                unregister_form(hwnd);
                CURRENT.with(|c| c.set(None));
                let pointer = SetWindowLongPtrW(hwnd, GWLP_USERDATA, 0) as *mut State;
                if !pointer.is_null() {
                    let owned = Box::from_raw(pointer);
                    // Closing while listening still gives Orbit its shortcuts back.
                    if owned.recording_launcher || owned.hotkey_target.is_some() {
                        pause_shortcuts(&owned, false);
                    }
                }
                LRESULT(0)
            }
            _ => DefWindowProcW(hwnd, message, wparam, lparam),
        }
    }
}

/// A card holding a search box, a Browse button and a list of apps. The list sizes itself to fit.
fn build_picker(b: &mut Builder, index: usize) -> (HWND, HWND, HWND) {
    let id = ID_PICKERS + index as i32 * PICKER_IDS;
    b.begin_card();
    let browse_width = b.px(96);
    let row = b.y;
    let edit = b.search_box("Search apps to add", b.x, row, b.width - browse_width - b.px(8), id);
    let browse = b.button_at("Browse\u{2026}", id + 1, b.x + b.width - browse_width, row, browse_width, ButtonKind::Normal);
    b.y = row + b.px(30) + b.px(6);
    let height = b.px(PICKER_ROW_DIP);
    let list = b.owner_list((b.x, b.y, b.width, height), 0, id + 2);
    b.skip(height);
    b.end_card();
    (edit, browse, list)
}

fn wire_picker(index: usize, edit: HWND, list: HWND) {
    unsafe {
        let _ = SetWindowSubclass(edit, Some(picker_proc), index * 2, index * 2);
        let _ = SetWindowSubclass(list, Some(picker_proc), index * 2 + 1, index * 2 + 1);
    }
}

/// Opens Settings on the page where a tool can be installed.
pub fn open_on_integrations(context: EditorContext, settings: Settings, dir: std::path::PathBuf, apps: &[crate::indexer::AppEntry]) {
    START_PAGE.with(|page| page.set(PAGE_INTEGRATIONS));
    open(context, settings, dir, apps);
}

/// The DPI and work area of the screen the window will open on: the one it was on, if it's being
/// replaced, or else the one with the pointer. A preview uses the system's and has no work area.
fn screen_to_open_on() -> (u32, Option<RECT>) {
    if forms::is_preview() {
        return (unsafe { GetDpiForSystem() }, None);
    }
    unsafe {
        let mut point = POINT::default();
        match NEXT_PLACE.with(|place| place.get()) {
            // A window belongs to the screen that has its middle.
            Some(old) => point = POINT { x: (old.left + old.right) / 2, y: (old.top + old.bottom) / 2 },
            None => {
                let _ = GetCursorPos(&mut point);
            }
        }
        let monitor = MonitorFromPoint(point, MONITOR_DEFAULTTONEAREST);
        let (mut dpi_x, mut dpi_y) = (96u32, 96u32);
        let _ = GetDpiForMonitor(monitor, MDT_EFFECTIVE_DPI, &mut dpi_x, &mut dpi_y);
        let mut info = MONITORINFO { cbSize: std::mem::size_of::<MONITORINFO>() as u32, ..Default::default() };
        let work = GetMonitorInfoW(monitor, &mut info).as_bool().then_some(info.rcWork);
        (dpi_x, work)
    }
}


/// The apps the pickers offer, found on a thread of their own after the window opens.
const WM_SETTINGS_CANDIDATES: u32 = WM_APP + 50;

pub fn open(context: EditorContext, settings: Settings, dir: std::path::PathBuf, apps: &[crate::indexer::AppEntry]) {
    let start_page = START_PAGE.with(|page| page.replace(PAGE_GENERAL));
    if let Some(existing) = CURRENT.with(|c| c.get()) {
        if start_page != PAGE_GENERAL {
            if let Some(s) = state(existing) {
                select_page(s, start_page);
            }
        }
        unsafe {
            let _ = ShowWindow(existing, SW_RESTORE);
            let _ = SetForegroundWindow(existing);
        }
        return;
    }
    unsafe {
        let Ok(module) = GetModuleHandleW(None) else { return };
        let class = WNDCLASSEXW {
            cbSize: std::mem::size_of::<WNDCLASSEXW>() as u32,
            lpfnWndProc: Some(settings_proc),
            hInstance: module.into(),
            hCursor: LoadCursorW(None, IDC_ARROW).unwrap_or_default(),
            lpszClassName: w!("OrbitSettings"),
            hIcon: crate::tray::load_app_icon(32, 32),
            ..Default::default()
        };
        RegisterClassExW(&class);
        register_page_classes(module);
        let (dpi, work) = screen_to_open_on();
        OPEN_DPI.with(|open_dpi| open_dpi.set(dpi));
        let px = |v: i32| forms::scale(v, dpi);
        // The size asked for is the client area, so the page layout doesn't depend on the title bar.
        // A normal window to Windows, for its shadow, corners, snapping and taskbar button, which says
        // it has no frame (WM_NCCALCSIZE). The strip across the top is drawn here and drags it.
        // A preview keeps the frame, because an off-screen capture leaves a frameless
        // window's controls out, and its picture is cut to the inside, which is the same.
        let frame = WS_OVERLAPPED | WS_CAPTION | WS_SYSMENU | WS_MINIMIZEBOX | WS_CLIPCHILDREN;
        // The whole window is client area: the strip across the top is drawn here (see WM_NCCALCSIZE).
        let mut size = RECT { left: 0, top: 0, right: px(WINDOW_WIDTH_DIP), bottom: px(WINDOW_HEIGHT_DIP + 24) };
        if forms::is_preview() {
            let _ = AdjustWindowRectExForDpi(&mut size, frame, false, WINDOW_EX_STYLE(0), dpi);
        }
        // On a small screen the window gets shorter and the page scrolls, rather than running off it.
        let mut window_height = px(WINDOW_HEIGHT_DIP + 24);
        if let Some(work) = work {
            let room = work.bottom - work.top - px(16);
            let too_tall = (size.bottom - size.top - room).max(0);
            window_height -= too_tall;
            size.bottom -= too_tall;
        }
        // On a narrow one, the pages get narrower, and their text wraps to suit.
        let room_across = match work {
            Some(work) => work.right - work.left - px(16),
            None => PREVIEW_ROOM.with(|room| room.get()).map_or(i32::MAX, px),
        };
        let too_wide = (size.right - size.left - room_across).clamp(0, px(WINDOW_WIDTH_DIP - WINDOW_MIN_WIDTH_DIP));
        let window_width = px(WINDOW_WIDTH_DIP) - too_wide;
        size.right -= too_wide;
        let (window_w, window_h) = (size.right - size.left, size.bottom - size.top);
        let (origin_x, origin_y) = match (NEXT_PLACE.with(|place| place.take()), work) {
            // Same middle as the window it replaces, which may be a different size, and all on screen.
            (Some(old), Some(work)) => (
                ((old.left + old.right) / 2 - window_w / 2).clamp(work.left, (work.right - window_w).max(work.left)),
                ((old.top + old.bottom) / 2 - window_h / 2).clamp(work.top, (work.bottom - window_h).max(work.top)),
            ),
            (Some(old), None) => (old.left, old.top),
            (None, Some(work)) => (work.left + (work.right - work.left - window_w) / 2, work.top + (work.bottom - work.top - window_h) / 2),
            (None, None) => forms::form_origin(),
        };
        let Ok(hwnd) = CreateWindowExW(
            forms::form_ex_style(),
            w!("OrbitSettings"),
            w!("Orbit Settings"),
            frame,
            origin_x,
            origin_y,
            size.right - size.left,
            size.bottom - size.top,
            None,
            None,
            None,
            None,
        ) else {
            return;
        };
        if !forms::is_preview() {
            // Windows works the frame out again, now that this window says it has none, and a
            // pixel of frame kept in the page is what keeps its shadow.
            let _ = SetWindowPos(hwnd, None, 0, 0, 0, 0, SWP_FRAMECHANGED | SWP_NOMOVE | SWP_NOSIZE | SWP_NOZORDER | SWP_NOACTIVATE);
            let margins = windows::Win32::UI::Controls::MARGINS { cxLeftWidth: 0, cxRightWidth: 0, cyTopHeight: 1, cyBottomHeight: 0 };
            let _ = windows::Win32::Graphics::Dwm::DwmExtendFrameIntoClientArea(hwnd, &margins);
        }
        let mut style = Box::new(Style::new(context.theme, dpi));
        style.apply_to_window(hwnd);
        style.set_rail(hwnd, rail_color(&style.theme));
        style.focus_by_keyboard = false;
        // The window comes up now, in its own colour, and its pages are made behind it. They take
        // most of the time, and Windows' opening animation hides it.
        let early = !forms::is_preview() && !QUIET_OPEN.with(|quiet| quiet.get());
        if early {
            EARLY_FILL.with(|fill| fill.set(Some((rail_color(&style.theme), style.theme.sidebar, style.theme.divider))));
            let _ = ShowWindow(hwnd, SW_SHOW);
            if !SetForegroundWindow(hwnd).as_bool() {
                crate::launch::force_foreground(hwnd);
            }
            let _ = UpdateWindow(hwnd);
            // Nothing more is drawn until every page is made and only the chosen one shows. Making
            // them can let paint messages through (icons, for one), and half-made pages drawn over
            // each other is what the screen showed otherwise.
            SendMessageW(hwnd, WM_SETREDRAW, Some(WPARAM(0)), None);
        }

        // Pages sit on a window that moves up and down inside another. Its left edge is the
        // sidebar's right, and the right-hand gutter is left free for the scroll thumb.
        let view_width = window_width - px(SIDEBAR_DIP);
        let view_height = window_height - px(TOPBAR_DIP);
        let content_width = view_width - px(SCROLL_GUTTER_DIP);
        let left = px(20);
        let top = px(6);
        let column = view_width - left - px(24);
        let mut b = Builder::new(hwnd, &mut style, left, top, column);

        let list = b.owner_list((px(12), px(TOPBAR_DIP + 8), px(SIDEBAR_DIP - 24), px(PAGE_ROW_DIP * PAGES.len() as i32)), 0, ID_PAGES);
        for (name, _) in PAGES {
            let text = wide(name);
            SendMessageW(list, LB_ADDSTRING, None, Some(LPARAM(text.as_ptr() as isize)));
        }
        b.take_controls();

        // Created after the page list, which is the Tab order.
        let view = CreateWindowExW(
            WS_EX_CONTROLPARENT,
            w!("OrbitSettingsView"),
            None,
            WS_CHILD | WS_VISIBLE | WS_CLIPCHILDREN,
            px(SIDEBAR_DIP),
            px(TOPBAR_DIP),
            view_width,
            view_height,
            Some(hwnd),
            Some(HMENU(ID_VIEW as isize as *mut _)),
            None,
            None,
        )
        .unwrap_or_default();
        let content = CreateWindowExW(
            WS_EX_CONTROLPARENT,
            w!("OrbitSettingsPage"),
            None,
            WS_CHILD | WS_VISIBLE | WS_CLIPCHILDREN,
            0,
            0,
            content_width,
            view_height,
            Some(view),
            Some(HMENU(ID_CONTENT as isize as *mut _)),
            None,
            None,
        )
        .unwrap_or_default();
        b.parent = content;
        // Room below a page's last card.
        let tail = px(12);

        let mut pages = Vec::new();
        let mut page_heights = Vec::new();

        // General
        b.page_header("General", PAGE_ABOUT[PAGE_GENERAL], PAGES[PAGE_GENERAL].1, PAGE_COLORS[PAGE_GENERAL]);
        b.heading("Startup and look");
        b.begin_card();
        let login = b.switch("Start Orbit when I sign in", settings.launch_at_login, 0);
        b.note("It waits in the background until you press its shortcut.");
        b.line();
        let appearance_names: Vec<&str> = APPEARANCE.iter().map(|a| a.0).collect();
        let appearance_at = APPEARANCE.iter().position(|a| a.1 == settings.appearance).unwrap_or(0);
        let appearance = b.combo("Appearance", &appearance_names, appearance_at, 0);
        b.note("For the launcher and this window.");
        b.line();
        let glass = b.switch("See-through launcher", settings.launcher.glass, 0);
        b.note("Solid in high contrast, over Remote Desktop, or with Windows' transparency effects off.");
        b.line();
        let fit_results = b.switch("Shrink to fit results", settings.launcher.fit_results, 0);
        b.note("Off keeps the launcher one height, so it doesn't move while you type.");
        b.end_card();

        b.heading("Where the launcher opens");
        b.begin_card();
        let screen_at = screen_picker::mode_of(&settings.launcher.screen);
        let screen_mode = b.combo("Open on", &screen_picker::MODES, screen_at, 0);
        let screen_note = b.note(screen_picker::mode_note(screen_at));
        // The picture draws the line above itself, so the line folds away with it.
        let screen = screen_picker::create(&mut b, &settings.launcher.screen, screen_note);
        b.end_card();

        b.heading("Launcher shortcut");
        b.begin_card();
        let rect = b.labelled_row("Open Orbit with", forms::COMBO_DIP, 34);
        let hotkey = b.custom(w!("OrbitRecorder"), rect, ID_HOTKEY);
        b.gap(-4);
        let hotkey_note = b.collapsed_note(forms::ID_NOTE);
        b.line();
        let launcher_games = b.switch("Leave it to fullscreen games", settings.launcher.pass_through_in_games, 0);
        b.note("The game gets the keys while it fills the screen.");
        b.end_card();
        b.caption("Apps that keep this shortcut for themselves");
        let (launcher_edit, _, launcher_list) = build_picker(&mut b, PICKER_LAUNCHER);

        b.heading("In the launcher");
        b.begin_card();
        let keyboard_names: Vec<&str> = KEYBOARD.iter().map(|k| k.0).collect();
        let keyboard_at = KEYBOARD.iter().position(|k| k.1 == settings.launcher.keyboard).unwrap_or(0);
        let keyboard = b.combo("Keyboard", &keyboard_names, keyboard_at, 0);
        let keyboard_note = b.note(keyboard_description(KEYBOARD[keyboard_at].1));
        b.line();
        let focus_running = b.switch("Enter switches to an app that's already running", settings.launcher.focus_running_app, 0);
        b.line();
        let toggles_app = b.switch("Pressing an app's hotkey again minimizes it", settings.launcher.hotkey_toggles_app, 0);
        b.line();
        let preserve = b.switch("Keep my last search when reopening", settings.launcher.preserve_query, 0);
        b.line();
        let favorites_only = b.switch("Open on favourites only", settings.launcher.favorites_only, 0);
        b.line();
        let copy_formatted = b.switch("Copy answers with formatting", settings.launcher.copy_formatted, 0);
        b.note("Enter copies \u{201C}\u{00A3}1,234.50\u{201D} or \u{201C}3.66 m\u{201D} instead of the bare number. Ctrl+Shift+Enter copies the other one.");
        b.line();
        let exchange_rates = b.switch("Convert currencies", settings.fetch_exchange_rates, 0);
        b.note("Such as \u{201C}50 euro in pounds\u{201D}. Orbit gets the European Central Bank's daily rates when you convert, at most twice a day. Nothing you type is sent.");
        b.line();
        let weather = b.switch("Weather", settings.fetch_weather, 0);
        b.note("Type \u{201C}weather\u{201D} and a place. Orbit sends the place to Open-Meteo to find it, then asks for its forecast, and remembers it for next time.");
        b.end_card();

        b.heading("Performance");
        b.begin_card();
        let low_resource = b.switch("Low resource mode", settings.low_resource_mode, 0);
        b.note("Smaller caches and no clipboard images. Orbit returns memory to Windows sooner.");
        b.end_card();
        pages.push(b.take_controls());
        page_heights.push(b.y + tail);

        // Search
        b.y = top;
        b.page_header("Search", PAGE_ABOUT[PAGE_SEARCH], PAGES[PAGE_SEARCH].1, PAGE_COLORS[PAGE_SEARCH]);
        let search_note = b.collapsed_note(forms::ID_NOTE);
        b.gap(6);
        b.begin_card();
        let search_filter = b.search_box("Filter apps and commands", b.x, b.y, b.width, ID_SEARCH_FILTER);
        b.y += px(36);
        let search_list = b.owner_list((b.x, b.y, b.width, px(HOTKEY_ROW_DIP)), 0, ID_SEARCH_LIST);
        b.skip(px(HOTKEY_ROW_DIP));
        b.end_card();
        // The alias box lives on the list, over the row being changed, and hides until then.
        SetWindowLongPtrW(search_list, GWL_STYLE, GetWindowLongPtrW(search_list, GWL_STYLE) | WS_CLIPCHILDREN.0 as isize);
        let alias_edit = CreateWindowExW(
            WS_EX_NOPARENTNOTIFY,
            w!("EDIT"),
            None,
            WS_CHILD | WINDOW_STYLE(ES_AUTOHSCROLL as u32),
            0,
            0,
            0,
            0,
            Some(search_list),
            Some(HMENU(ID_ALIAS_EDIT as isize as *mut _)),
            None,
            None,
        )
        .unwrap_or_default();
        SendMessageW(alias_edit, WM_SETFONT, Some(WPARAM(b.style.small.0 as usize)), Some(LPARAM(0)));
        SendMessageW(alias_edit, 0x00C5, Some(WPARAM(24)), None);
        pages.push(b.take_controls());
        page_heights.push(b.y + tail);

        // Hotkeys
        b.y = top;
        // Always in the same place, because it's what brings the list back.
        let text_width = b.px(120);
        let (hotkeys_title, hotkeys_about) = b.page_header("Hotkeys", PAGE_ABOUT[PAGE_HOTKEYS], PAGES[PAGE_HOTKEYS].1, PAGE_COLORS[PAGE_HOTKEYS]);
        for part in [hotkeys_title, hotkeys_about] {
            let mut place = RECT::default();
            let _ = GetWindowRect(part, &mut place);
            let _ = SetWindowPos(part, None, 0, 0, b.x + b.width - text_width - b.px(12) - (place.left - window_left(part)), place.bottom - place.top, SWP_NOMOVE | SWP_NOZORDER | SWP_NOACTIVATE);
        }
        let edit_as_text = b.button_at("Edit as text", ID_HOTKEY_TEXT, b.x + b.width - text_width, top + b.px(7), text_width, ButtonKind::Normal);
        let hotkeys_note = b.collapsed_note(forms::ID_NOTE);
        let list_top = b.y;
        b.begin_card();
        let action_search = b.search_box("Add a hotkey: search apps and commands", b.x, b.y, b.width, ID_ACTION_SEARCH);
        b.y += px(36);
        let hotkey_list = b.owner_list((b.x, b.y, b.width, px(HOTKEY_ROW_DIP)), 0, ID_HOTKEY_LIST);
        b.skip(px(HOTKEY_ROW_DIP));
        b.end_card();
        let mut list_controls = vec![action_search, hotkey_list];
        let mut list_buttons = b.buttons(&[("Add suggested", ID_SUGGEST, ButtonKind::Normal)]);
        let suggest = list_buttons[0];
        list_controls.append(&mut list_buttons);
        let list_mode_bottom = b.y;
        // The same bindings as text, for pasting, or for what the search can't offer, such as a path.
        b.y = list_top;
        let mut text_controls = Vec::new();
        text_controls.push(b.caption("HyperKey mappings, as key = action"));
        let hyper_table = b.text_area(&format_hyper(&settings.hyper.bindings), 7, 0);
        text_controls.push(hyper_table);
        text_controls.push(b.caption("Shortcuts, as shortcut = action"));
        let shortcuts = b.text_area(&format_shortcuts(&settings.shortcuts), 6, 0);
        text_controls.push(shortcuts);
        text_controls.push(b.note("Actions: app:Firefox, url:https://example.com, path:C:\\file.txt, cmd:clipboard, cmd:new-note, custom:id. Start a line with # to turn it off."));
        let text_error = b.note_area("", 3);
        text_controls.push(text_error);
        let text_mode_bottom = b.y;
        pages.push(b.take_controls());
        page_heights.push(list_mode_bottom + tail);

        // HyperKey
        b.y = top;
        b.page_header("HyperKey", PAGE_ABOUT[PAGE_HYPER], PAGES[PAGE_HYPER].1, PAGE_COLORS[PAGE_HYPER]);
        b.begin_card();
        let hyper_on = b.switch("Turn on HyperKey", settings.hyper.enabled, 0);
        b.note("Hold this key plus another to run a hotkey. Orbit stores and logs no keystrokes.");
        b.line();
        let trigger_at = TRIGGERS.iter().position(|t| t.eq_ignore_ascii_case(&settings.hyper.trigger)).unwrap_or(0);
        let hyper_trigger = b.combo("Key to use", TRIGGERS, trigger_at, 0);
        let mut hyper_children = vec![hyper_trigger, b.last_label];
        hyper_children.push(b.note("Caps Lock is the usual choice."));
        b.line();
        let mode_names: Vec<&str> = MODES.iter().map(|m| m.0).collect();
        let hyper_mode = b.combo("How it works", &mode_names, MODES.iter().position(|m| m.1 == settings.hyper.mode).unwrap_or(0), 0);
        hyper_children.extend([hyper_mode, b.last_label]);
        hyper_children.push(b.note("Leave this to Orbit unless another program already remaps the key."));
        b.line();
        let tap_names: Vec<&str> = TAPS.iter().map(|t| t.0).collect();
        let hyper_tap = b.combo("Tapped on its own", &tap_names, TAPS.iter().position(|t| t.1 == settings.hyper.tap).unwrap_or(0), 0);
        hyper_children.extend([hyper_tap, b.last_label]);
        hyper_children.push(b.note("What a quick press does when no other key follows."));
        b.line();
        let tap_time = preset_combo(&mut b, "Longest tap", TAP_TIMES, settings.hyper.tap_timeout_ms, |value| format!("{value} ms"));
        hyper_children.extend([tap_time.hwnd, b.last_label]);
        hyper_children.push(b.note("Held for longer, it counts as holding the key."));
        b.line();
        let hyper_games = b.switch("Turn off while a game fills the screen", settings.hyper.pass_through_in_games, 0);
        hyper_children.push(hyper_games);
        b.end_card();
        hyper_children.push(b.caption("Apps where the key keeps its normal behaviour"));
        let (hyper_edit, hyper_browse, hyper_list) = build_picker(&mut b, PICKER_HYPER);
        hyper_children.extend([hyper_edit, hyper_browse, hyper_list]);
        pages.push(b.take_controls());
        page_heights.push(b.y + tail);

        // macOS feel
        b.y = top;
        b.page_header("macOS feel", PAGE_ABOUT[4], PAGES[4].1, PAGE_COLORS[4]);
        b.begin_card();
        let mac_on = b.switch("Use macOS-style shortcuts in every app", settings.mac_feel.enabled, 0);
        b.note("Alt does what Cmd does on a Mac. Stays out of games and remote sessions.");
        b.line();
        let mac_text = b.switch("Text keys", settings.mac_feel.text_keys, 0);
        let mut mac_children = vec![mac_text];
        mac_children.push(b.note("Alt+arrows go to line ends, replacing Back and Forward in browsers."));
        b.line();
        let mac_apps = b.switch("App keys", settings.mac_feel.app_keys, 0);
        mac_children.push(mac_apps);
        mac_children.push(b.note("Alt+Q closes the app's windows, and Alt+H minimizes them."));
        b.end_card();
        mac_children.push(b.caption("Apps where these keys keep their normal behaviour"));
        let (mac_edit, mac_browse, mac_list) = build_picker(&mut b, PICKER_MAC);
        mac_children.extend([mac_edit, mac_browse, mac_list]);
        pages.push(b.take_controls());
        page_heights.push(b.y + tail);

        // Clipboard
        b.y = top;
        b.page_header("Clipboard", PAGE_ABOUT[PAGE_CLIPBOARD], PAGES[PAGE_CLIPBOARD].1, PAGE_COLORS[PAGE_CLIPBOARD]);
        b.begin_card();
        let clip_on = b.switch("Keep clipboard history", settings.clipboard.enabled, 0);
        b.note("Stays on this PC. Password managers that mark their copies, and the apps below, are skipped.");
        b.line();
        let clip_persist = b.switch("Keep history after Orbit exits", settings.clipboard.persist, 0);
        b.note("Off clears it whenever Orbit closes.");
        b.line();
        let clip_images = b.switch("Include images", settings.clipboard.capture_images, 0);
        b.note("Screenshots and copied pictures.");
        b.line();
        let clip_max = preset_combo(&mut b, "Items to keep", CLIP_ITEMS, settings.clipboard.max_items, |value| format!("{value} items"));
        let mut clip_children = vec![clip_persist, clip_images, clip_max.hwnd, b.last_label];
        clip_children.push(b.note("The oldest go first. Pinned items always stay."));
        b.line();
        let clip_size = preset_combo(&mut b, "Largest text item", CLIP_SIZES, settings.clipboard.max_item_bytes, |value| format!("{} KB", value / 1024));
        clip_children.extend([clip_size.hwnd, b.last_label]);
        clip_children.push(b.note("Bigger copies still paste, but aren't kept."));
        b.line();
        let clip_days = preset_combo(&mut b, "Keep for", CLIP_DAYS, settings.clipboard.retention_days, |value| format!("{value} days"));
        clip_children.extend([clip_days.hwnd, b.last_label]);
        clip_children.push(b.note("Older items are removed, except pinned ones."));
        b.end_card();
        clip_children.push(b.caption("Never record copies from these apps"));
        let (clip_edit, clip_browse, clip_list) = build_picker(&mut b, PICKER_CLIPBOARD);
        clip_children.extend([clip_edit, clip_browse, clip_list]);
        b.begin_card();
        b.titled_action("Clear history", "Removes everything except pinned items.", LABEL_CLEAR, ID_CLEAR_CLIPS, ButtonKind::Danger);
        b.end_card();
        pages.push(b.take_controls());
        page_heights.push(b.y + tail);

        // Snippets
        b.y = top;
        b.page_header("Snippets", PAGE_ABOUT[PAGE_SNIPPETS], PAGES[PAGE_SNIPPETS].1, PAGE_COLORS[PAGE_SNIPPETS]);
        b.begin_card();
        let expansion = b.switch("Expand snippet keywords as I type", settings.snippets.expansion_enabled, 0);
        b.note("Keeps only the last few characters in memory. Skips password fields and games.");
        b.end_card();
        let snippets_managed = build_managed(&mut b, "Your snippets", None, ID_SNIPPET_NEW, ID_SNIPPET_LIST);
        let snippet_caption = b.caption("Don't expand in these apps");
        let (snippet_edit, snippet_browse, snippet_list) = build_picker(&mut b, PICKER_SNIPPETS);
        pages.push(b.take_controls());
        page_heights.push(b.y + tail);

        // Links and commands
        b.y = top;
        b.page_header("Links & commands", PAGE_ABOUT[PAGE_LINKS], PAGES[PAGE_LINKS].1, PAGE_COLORS[PAGE_LINKS]);
        let links_managed = build_managed(&mut b, "Quicklinks", Some("Websites and searches you can open by name or alias."), ID_LINK_NEW, ID_LINK_LIST);
        b.begin_card();
        let link_icons = b.switch("Show website icons", settings.fetch_link_icons, 0);
        b.note("Orbit asks each quicklink's own website for its icon, once, and keeps it. Only the site is asked, never anyone else, with no cookies or sign-in. Turning this off deletes the icons.");
        b.end_card();
        let commands_managed = build_managed(&mut b, "Commands", Some("Programs you can run from Orbit, only when you choose them, without a shell."), ID_COMMAND_NEW, ID_COMMAND_LIST);
        b.begin_card();
        let (hidden_note, hidden_button) = b.titled_action("Hidden from search", &hidden_count(settings.hidden.len()), "Restore all", ID_UNHIDE, ButtonKind::Normal);
        let _ = EnableWindow(hidden_button, !settings.hidden.is_empty());
        b.end_card();
        pages.push(b.take_controls());
        page_heights.push(b.y + tail);

        // Integrations
        b.y = top;
        b.page_header("Integrations", PAGE_ABOUT[PAGE_INTEGRATIONS], PAGES[PAGE_INTEGRATIONS].1, PAGE_COLORS[PAGE_INTEGRATIONS]);
        b.begin_card();
        let op_on = b.switch("Show 1Password in Orbit", settings.integrations.one_password, 0);
        b.note("Search your vaults, then copy a password, username or one-time code.");
        b.line();
        let (text, label, can_press) = tool_row(Tool::OnePassword);
        let (op_status, op_button) = b.titled_action("Command-line tool", &text, label, ID_TOOL_OP, ButtonKind::Normal);
        let _ = EnableWindow(op_button, can_press);
        let op_note = b.note(one_password_note());
        b.end_card();
        b.begin_card();
        let ente_on = b.switch("Show Ente Auth in Orbit", settings.integrations.ente_auth, 0);
        b.note("Search your codes, then copy one.");
        b.line();
        let (text, label, can_press) = tool_row(Tool::EnteAuth);
        let (ente_status, ente_button) = b.titled_action("Command-line tool", &text, label, ID_TOOL_ENTE, ButtonKind::Normal);
        let _ = EnableWindow(ente_button, can_press);
        b.line();
        let (ente_sign_in_note, ente_sign_in) = b.titled_action("Sign in", ENTE_SIGN_IN_NOTE, "Sign in\u{2026}", ID_ENTE_SIGN_IN, ButtonKind::Normal);
        let _ = EnableWindow(ente_sign_in, integrations::find(Tool::EnteAuth).is_some());
        b.end_card();
        pages.push(b.take_controls());
        page_heights.push(b.y + tail);

        // Data
        b.y = top;
        b.page_header("Data", PAGE_ABOUT[9], PAGES[9].1, PAGE_COLORS[9]);
        b.heading("Your data");
        b.begin_card();
        // Windows shortens the whole path to fit, keeping its start and its end (SS_PATHELLIPSIS).
        let (folder_note, _) = b.titled_action("Data folder", &short_path(&dir.to_string_lossy(), 40), "Open folder", ID_OPEN_DIR, ButtonKind::Normal);
        SetWindowLongPtrW(folder_note, GWL_STYLE, GetWindowLongPtrW(folder_note, GWL_STYLE) | 0x8000);
        forms::set_text_only(folder_note, &dir.to_string_lossy());
        b.line();
        b.titled_action("Export", "Notes, snippets and settings, in a new folder in Documents.", "Export", ID_EXPORT, ButtonKind::Normal);
        b.line();
        b.titled_action("Import", "Adds notes and snippets, or replaces settings from a settings.json.", "Import\u{2026}", ID_IMPORT, ButtonKind::Normal);
        b.end_card();
        b.heading("Troubleshooting");
        b.begin_card();
        b.titled_action("App index", "Finds newly installed apps.", "Rebuild", ID_REINDEX, ButtonKind::Normal);
        b.line();
        b.titled_action("Diagnostics", "Memory, index and timing, shown in the launcher.", "Show", ID_DIAGNOSTICS, ButtonKind::Normal);
        b.end_card();
        b.heading("Start over");
        b.begin_card();
        b.titled_action("Reset shortcuts", "Back to the default launcher key, with no other shortcuts or mappings.", LABEL_RESET, ID_RESET_SHORTCUTS, ButtonKind::Danger);
        b.line();
        b.titled_action("Delete all data", "Everything Orbit keeps, for good. Orbit then quits.", LABEL_DELETE, ID_DELETE_DATA, ButtonKind::Danger);
        b.end_card();
        pages.push(b.take_controls());
        page_heights.push(b.y + tail);

        // Messages sit at the foot of the page list. They don't need a bar of their own.
        b.parent = hwnd;
        let status = b.quiet_at(px(24), window_height - px(FOOT_DIP), px(SIDEBAR_DIP - 40), 0);
        b.take_controls();

        let controls = Controls {
            search_note,
            hotkey,
            hotkey_note,
            login,
            focus_running,
            toggles_app,
            preserve,
            mac_on,
            mac_text,
            mac_apps,
            favorites_only,
            glass,
            fit_results,
            launcher_games,
            appearance,
            keyboard,
            keyboard_note,
            screen_mode,
            screen,
            low_resource,
            link_icons,
            exchange_rates,
            copy_formatted,
            weather,
            hotkey_list,
            hotkeys_note,
            action_search,
            text_error,
            edit_as_text,
            shortcuts,
            hyper_on,
            hyper_trigger,
            hyper_mode,
            hyper_tap,
            hyper_games,
            hyper_table,
            clip_on,
            clip_persist,
            clip_images,
            expansion,
            op_on,
            ente_on,
            op_status,
            op_note,
            ente_status,
            op_button,
            ente_button,
            ente_sign_in,
            ente_sign_in_note,
            hidden_note,
            hidden_button,
            suggest,
        };
        // The apps the pickers offer. Finding Steam games' programs reads Steam's folders, which
        // takes a while, so it happens on its own thread and the pickers fill in when it's done.
        // Freed when Settings closes.
        let running = crate::launch::running_programs();
        let all_apps = apps.to_vec();
        let reply = crate::util::SendHwnd::new(hwnd);
        let _ = std::thread::Builder::new().name("orbit-settings-apps".into()).spawn(move || {
            let found = app_picker::candidates(&all_apps, &running);
            reply.post(WM_SETTINGS_CANDIDATES, crate::util::box_for_post(found), 0);
        });
        let candidates = Vec::new();
        let action_options = action_options(apps, &settings);
        let search_items = search_items(apps, &settings);
        let picker = |edit: HWND, list: HWND, chosen: &[String]| Picker { edit, list, chosen: chosen.to_vec(), rows: Vec::new(), hover: None, tracking: false, expanded: false, just_added: false };
        let pickers = vec![
            picker(hyper_edit, hyper_list, &settings.hyper.excluded_apps),
            picker(clip_edit, clip_list, &settings.clipboard.excluded_apps),
            picker(snippet_edit, snippet_list, &settings.snippets.excluded_apps),
            picker(mac_edit, mac_list, &settings.mac_feel.excluded_apps),
            picker(launcher_edit, launcher_list, &settings.launcher.excluded_apps),
        ];
        // Something that's off keeps its value, and shows that it isn't doing anything.
        let dependents = vec![
            (hyper_on, hyper_children),
            (mac_on, mac_children),
            (clip_on, clip_children),
            (expansion, vec![snippet_caption, snippet_edit, snippet_browse, snippet_list]),
        ];
        let owned = Box::new(State {
            context,
            last_saved: settings.clone(),
            save_pending: false,
            hotkey_asking: None,
            held_keys: String::new(),
            swallow_keys: false,
            list_press: None,
            armed_at: std::time::Instant::now(),
            showing_problem: false,
            close_warned: false,
            launcher_chord: settings.launcher.hotkey.clone(),
            active_hotkey: String::new(),
            recording_launcher: false,
            hotkey_problems: String::new(),
            hotkey_target: None,
            hotkey_problem: None,
            hotkey_armed: None,
            hotkey_hover: None,
            hotkey_tracking: false,
            replaced_note: None,
            result_until: None,
            action_options,
            presets: vec![tap_time, clip_max, clip_size, clip_days],
            dependents,
            armed_button: None,
            hotkeys_list_bottom: list_mode_bottom + tail,
            hotkeys_text_bottom: text_mode_bottom + tail,
            dpi_change_waiting: false,
            moving: false,
            base: settings,
            pages,
            page: 0,
            list,
            status,
            style,
            controls,
            dir,
            unhide_all: false,
            hotkey_rows: Vec::new(),
            text_mode: false,
            text_controls,
            list_controls,
            pickers,
            candidates,
            icons: std::collections::HashMap::new(),
            view,
            content,
            page_heights,
            scroll: 0,
            thumb_grab: None,
            last_focus: HWND::default(),
            lists: vec![
                ManagedList { hwnd: snippets_managed, rows: Vec::new(), armed: None, hover: None, tracking: false, empty: "No snippets yet. Press New to write your first one." },
                ManagedList { hwnd: links_managed, rows: Vec::new(), armed: None, hover: None, tracking: false, empty: "No quicklinks yet. Press New to add a website or a search." },
                ManagedList { hwnd: commands_managed, rows: Vec::new(), armed: None, hover: None, tracking: false, empty: "No commands yet. Press New to add a program to run." },
            ],
            snippet_ids: Vec::new(),
            search_filter,
            search_list,
            search_items,
            search_rows: Vec::new(),
            search_filled: false,
            search_hover: None,
            search_tracking: false,
            search_cell: SearchCell::Switch,
            alias_edit,
            alias_row: None,
            search_recording: None,
            caption_hover: None,
            caption_press: None,
            caption_tracking: false,
        });
        let pointer = Box::into_raw(owned);
        SetWindowLongPtrW(hwnd, GWLP_USERDATA, pointer as isize);
        let owned = &mut *pointer;
        let _ = SetWindowSubclass(owned.controls.hotkey_list, Some(hotkey_list_proc), HOTKEY_LIST_SUBCLASS, 0);
        let _ = SetWindowSubclass(owned.search_list, Some(search_list_proc), SEARCH_LIST_SUBCLASS, 0);
        let _ = SetWindowSubclass(owned.alias_edit, Some(alias_edit_proc), ALIAS_EDIT_SUBCLASS, 0);
        let _ = SetWindowSubclass(owned.controls.action_search, Some(action_search_proc), ACTION_SEARCH_SUBCLASS, 0);
        refresh_hotkey_list(owned);
        for index in 0..owned.pickers.len() {
            refresh_picker(owned, index);
            wire_picker(index, owned.pickers[index].edit, owned.pickers[index].list);
        }
        for index in 0..owned.lists.len() {
            let _ = SetWindowSubclass(owned.lists[index].hwnd, Some(managed_proc), index, index);
        }
        let dir = owned.dir.clone();
        let rows = link_rows(&owned.base.links, owned.base.fetch_link_icons.then_some(dir.as_path()));
        fill_managed(owned, LIST_LINKS, rows);
        let rows = command_rows(&owned.base.custom_commands);
        fill_managed(owned, LIST_COMMANDS, rows);
        fill_managed(owned, LIST_SNIPPETS, Vec::new());
        let _ = owned.context.tx.send(Req::SnippetList);
        apply_dependents(owned);
        show_hotkey_problems(owned);
        // Opening isn't a change, even if the form tidies how something's written.
        owned.last_saved = collect(owned).0;
        select_page(owned, start_page);
        register_form(hwnd);
        CURRENT.with(|c| c.set(Some(hwnd)));
        // Previews never run a real tool.
        if !forms::is_preview() && integrations::find(Tool::EnteAuth).is_some() {
            integrations::check_ente_account(owned.context.main, WM_ORBIT_ENTE_ACCOUNT);
        }
        EARLY_FILL.with(|fill| fill.set(None));
        if early {
            SendMessageW(hwnd, WM_SETREDRAW, Some(WPARAM(1)), None);
        }
        if QUIET_OPEN.with(|quiet| quiet.replace(false)) {
            let _ = ShowWindow(hwnd, SW_SHOWNOACTIVATE);
        } else {
            forms::show_form(hwnd, list);
        }
        if early {
            let _ = RedrawWindow(Some(hwnd), None, None, RDW_ERASE | RDW_FRAME | RDW_INVALIDATE | RDW_ALLCHILDREN | RDW_UPDATENOW);
        }
    }
}

/// For the developer preview: opens on a page without taking focus. See `settings_preview.rs`.
pub fn preview_page_index(name: &str) -> Option<usize> {
    PAGES.iter().position(|page| page.0.eq_ignore_ascii_case(name) || page.0.split_whitespace().next().is_some_and(|word| word.eq_ignore_ascii_case(name)))
}

/// Makes the window that a preview opens fit a screen this wide, in DIP.
pub fn preview_limit_width(dip: i32) {
    PREVIEW_ROOM.with(|room| room.set(Some(dip)));
}

pub fn preview_open(context: EditorContext, settings: Settings, dir: std::path::PathBuf, page: usize) {
    START_PAGE.with(|start| start.set(page));
    let apps = crate::indexer::load_index(&dir).unwrap_or_default();
    open(context, settings, dir, &apps);
    // `open` treats General as "nothing asked for", so this covers every page the same way.
    if let Some(s) = CURRENT.with(|c| c.get()).and_then(state) {
        select_page(s, page);
    }
}

pub fn preview_scroll(pixels: i32) {
    if let Some(s) = CURRENT.with(|c| c.get()).and_then(state) {
        scroll_to(s, pixels);
    }
}

/// Fills the hotkey list with the suggested keys, so a preview can show its rows.
pub fn preview_fill_hotkeys() {
    if let Some(s) = CURRENT.with(|c| c.get()).and_then(state) {
        if let Ok(suggested) = parse_hyper(SUGGESTED_HYPER) {
            s.base.hyper.bindings = suggested;
        }
        refresh_hotkey_list(s);
        fill_hotkey_text(s);
    }
}

/// Puts a form into a state a picture of it can't otherwise show.
pub fn preview_state(what: &str) {
    let Some(s) = CURRENT.with(|c| c.get()).and_then(state) else { return };
    match what {
        "recording" => {
            if let Some(first) = s.hotkey_rows.iter().position(|row| !row.is_header()) {
                unsafe {
                    SendMessageW(s.controls.hotkey_list, LB_SETCURSEL, Some(WPARAM(first)), None);
                }
                let row = s.hotkey_rows[first];
                start_hotkey_record(s, RecordTarget::Row(row));
            }
        }
        "held" => {
            preview_state("recording");
            s.held_keys = "Ctrl+Alt".to_string();
        }
        "replace" => {
            preview_state("recording");
            let taken = s.hotkey_rows.iter().filter_map(|row| match row {
                HotkeyRow::Hyper(i) => s.base.hyper.bindings.get(*i).map(|b| b.key.clone()),
                _ => None,
            }).nth(1);
            if let Some(key) = taken {
                record_hotkey_keys(s, Mods::NONE, &key);
            }
        }
        "search" => {
            s.action_options.insert(0, ActionOption { name: "Example App".to_string(), kind: "App", action: Action::Builtin { command: Builtin::NewNote } });
            set_text(s.controls.action_search, "e");
        }
        "search-filled" | "search-alias" | "search-record" => {
            // A stand-in alias, and a filter that shows a few rows, one of them with a hotkey.
            if let Some(item) = s.search_items.iter().find(|item| item.name == "Clipboard History") {
                let key = item.key.clone();
                set_item_alias(&mut s.base, &key, "cb");
            }
            show_page(s, PAGE_SEARCH);
            set_text(s.search_filter, "clip");
            refresh_search_list(s);
            let first = nearest_search_row(&s.search_rows, 0, 1);
            if let Some(first) = first {
                unsafe {
                    SendMessageW(s.search_list, LB_SETCURSEL, Some(WPARAM(first)), None);
                }
                match what {
                    "search-alias" => start_alias_edit(s, first),
                    "search-record" => {
                        let bindable = (first..s.search_rows.len()).find(|at| matches!(s.search_rows[*at], SearchRow::Item(item) if s.search_items[item].action.is_some() && s.search_items[item].name.starts_with("Pause")));
                        start_search_record(s, bindable.unwrap_or(first));
                    }
                    _ => {
                        forms::focus(s.search_list);
                        s.style.focus_by_keyboard = true;
                        s.search_cell = SearchCell::Hotkey;
                        s.search_hover = nearest_search_row(&s.search_rows, first + 1, 1).map(|at| (at, Some(SearchCell::Alias)));
                    }
                }
            }
            unsafe {
                let _ = InvalidateRect(Some(s.search_list), None, false);
            }
        }
        "problem" => set_hotkey_note(s, "Alt+Space is in use by another program, so Orbit is using Ctrl+Alt+Space. Free right now: Ctrl+Shift+Space, Alt+F1.", true),
        "listening" => start_launcher_record(s),
        "saved" => show_status(s, "Saved", SAVED_MS),
        "failed" => show_problem(s, SAVE_FAILED),
        "hover" => {
            if let Some(at) = s.hotkey_rows.iter().position(|row| !row.is_header()) {
                s.hotkey_hover = Some((at, ListPart::Body));
                unsafe {
                    let _ = InvalidateRect(Some(s.controls.hotkey_list), None, false);
                }
            }
        }
        "text-error" => {
            toggle_hotkey_text(s);
            set_text(s.controls.hyper_table, "not a mapping");
            let _ = save_now(s);
        }
        "armed" => {
            if let Ok(button) = unsafe { GetDlgItem(Some(s.content), ID_DELETE_DATA) } {
                confirm_in_place(s, button, LABEL_DELETE);
            }
        }
        _ => {}
    }
}

/// Presses "Edit as text" on the Hotkeys page.
pub fn preview_hotkeys_as_text() {
    if let Some(hwnd) = CURRENT.with(|c| c.get()) {
        unsafe {
            SendMessageW(hwnd, WM_COMMAND, Some(WPARAM(ID_HOTKEY_TEXT as usize)), None);
        }
    }
}

/// Presses Delete once on a row, so a preview can show it asking "Sure?".
pub fn preview_arm_first_link() {
    if let Some(s) = CURRENT.with(|c| c.get()).and_then(state) {
        s.lists[LIST_LINKS].armed = Some(0);
        unsafe {
            let _ = InvalidateRect(Some(s.lists[LIST_LINKS].hwnd), None, false);
        }
    }
}

/// Drives the real window with real messages, without focus or the real pointer, and says what
/// happened. Only in development builds. Each line starts with PASS or FAIL.
#[cfg(debug_assertions)]
pub fn preview_selfcheck() -> Vec<String> {
    use windows::Win32::UI::Input::KeyboardAndMouse::{GetKeyboardState, SetKeyboardState, VK_BACK};
    let mut out = Vec::new();
    let Some(root) = CURRENT.with(|c| c.get()) else { return vec!["FAIL no window".to_string()] };
    let Some(s) = state(root) else { return vec!["FAIL no state".to_string()] };
    let mut check = |name: &str, ok: bool, detail: String| out.push(format!("{} {name} ({detail})", if ok { "PASS" } else { "FAIL" }));
    let notch_down = WPARAM(((-120i16 as u16 as usize) << 16) | 0);
    let notch_up = WPARAM((120usize) << 16);
    let pump = || unsafe {
        let mut message = MSG::default();
        while PeekMessageW(&mut message, None, 0, 0, PM_REMOVE).as_bool() {
            if message.message == WM_ORBIT_LINK_REQUEST {
                continue;
            }
            let _ = TranslateMessage(&message);
            DispatchMessageW(&message);
        }
    };
    unsafe {
        // The Search page: aliases and hotkeys, changed in its rows.
        select_page(s, PAGE_SEARCH);
        set_text(s.search_filter, "");
        refresh_search_list(s);
        let row_named = |s: &State, name: &str| s.search_rows.iter().position(|row| matches!(row, SearchRow::Item(at) if s.search_items[*at].name == name));
        let builtin_name = |command: Builtin| Builtin::ALL.iter().find(|entry| entry.0 == command).map(|entry| entry.2).unwrap_or("");
        let (first, second) = (builtin_name(Builtin::NewNote), builtin_name(Builtin::SearchNotes));
        match (row_named(s, first), row_named(s, second)) {
            (Some(one), Some(two)) => {
                start_alias_edit(s, one);
                check("an alias box opens on its row", s.alias_row == Some(one) && IsWindowVisible(s.alias_edit).as_bool(), format!("{:?}", s.alias_row));
                set_text(s.alias_edit, "nn");
                let kept = finish_alias_edit(s, true);
                let key_one = match s.search_rows[one] { SearchRow::Item(at) => s.search_items[at].key.clone(), _ => String::new() };
                check("Enter keeps the alias", kept && item_alias(&s.base, &key_one) == "nn" && !IsWindowVisible(s.alias_edit).as_bool(), item_alias(&s.base, &key_one));
                start_alias_edit(s, two);
                set_text(s.alias_edit, "NN");
                let refused = !finish_alias_edit(s, true);
                check("the same alias on something else is refused", refused && s.alias_row == Some(two) && s.style.is_alert(s.controls.search_note), get_text(s.controls.search_note));
                set_text(s.alias_edit, "two words");
                check("and so is one with a space", !finish_alias_edit(s, true), get_text(s.controls.search_note));
                finish_alias_edit(s, false);
                check("Esc puts it back", s.alias_row.is_none() && get_text(s.controls.search_note).is_empty(), get_text(s.controls.search_note));
                start_alias_edit(s, one);
                set_text(s.alias_edit, "");
                finish_alias_edit(s, true);
                check("an empty alias takes it away", item_alias(&s.base, &key_one).is_empty(), item_alias(&s.base, &key_one));

                start_search_record(s, one);
                check("a hotkey listens in its row", s.search_recording.is_some() && s.hotkey_target.is_some() && get_text(s.controls.search_note) == SEARCH_HINT, get_text(s.controls.search_note));
                record_hotkey_keys(s, Mods::CTRL.union(Mods::ALT), "J");
                let action = Action::Builtin { command: Builtin::NewNote };
                let keys = item_hotkey(&s.base, &action);
                check("keys pressed there become its hotkey", keys.as_deref() == Some("Ctrl+Alt+J") && s.search_recording.is_none() && s.hotkey_target.is_none(), format!("{keys:?}"));
                check("and the Hotkeys page lists it", s.hotkey_rows.iter().any(|row| row.binding_action(&s.base).is_some_and(|bound| same_target(&bound, &action))), String::new());
                remove_search_hotkey(s, one);
                check("Delete takes it away", item_hotkey(&s.base, &action).is_none(), String::new());
                let asks = (0..s.search_rows.len()).find(|at| matches!(s.search_rows[*at], SearchRow::Item(item) if s.search_items[item].action.is_none()));
                if let Some(asks) = asks {
                    start_search_record(s, asks);
                    check("what asks first can't have a hotkey", s.hotkey_target.is_none() && !get_text(s.controls.search_note).is_empty(), get_text(s.controls.search_note));
                    say_search_note(s, "", false);
                }
            }
            _ => check("the Search page lists Orbit's commands", false, format!("{} rows", s.search_rows.len())),
        }

        // The wheel, on the General page.
        select_page(s, PAGE_GENERAL);
        check("General page overflows its view", page_height(s) > view_height(s), format!("page {} view {}", page_height(s), view_height(s)));
        let before = selected(s.controls.appearance);
        SendMessageW(s.controls.appearance, WM_MOUSEWHEEL, Some(notch_down), None);
        check("wheel over a closed drop-down scrolls the page", s.scroll > 0, format!("scroll {}", s.scroll));
        check("and leaves its value alone", selected(s.controls.appearance) == before, format!("{before}"));
        scroll_to(s, 0);
        SendMessageW(s.controls.glass, WM_MOUSEWHEEL, Some(notch_down), None);
        check("wheel over a switch scrolls the page", s.scroll > 0, format!("scroll {}", s.scroll));
        scroll_to(s, 0);
        SendMessageW(s.controls.hotkey, WM_MOUSEWHEEL, Some(notch_down), None);
        check("wheel over the shortcut recorder scrolls the page", s.scroll > 0, format!("scroll {}", s.scroll));
        let at = s.scroll;
        SendMessageW(root, WM_MOUSEWHEEL, Some(notch_up), None);
        check("wheel over the main window scrolls the page back", s.scroll < at, format!("scroll {}", s.scroll));
        let mut position = RECT::default();
        let _ = GetWindowRect(s.content, &mut position);
        let mut point = POINT { x: position.left, y: position.top };
        let _ = ScreenToClient(s.view, &mut point);
        check("the page window really moved", point.y == -s.scroll, format!("y {} scroll {}", point.y, s.scroll));

        // The thumb.
        scroll_to(s, 0);
        let (top, height) = thumb(s).unwrap_or((0, 0));
        check("the thumb shows on a page that overflows", height > 0, format!("top {top} height {height}"));
        let gutter_x = view_height(s).min(0) + forms::scale(WINDOW_WIDTH_DIP - SIDEBAR_DIP - 5, s.style.dpi);
        let at = |y: i32| LPARAM(((y as isize & 0xFFFF) << 16) | gutter_x as isize);
        SendMessageW(s.view, WM_LBUTTONDOWN, Some(WPARAM(1)), Some(at(top + height / 2)));
        check("pressing the thumb takes hold and doesn't jump", s.thumb_grab.is_some() && s.scroll < 3, format!("scroll {}", s.scroll));
        SendMessageW(s.view, WM_MOUSEMOVE, Some(WPARAM(1)), Some(at(top + height / 2 + 100)));
        let dragged = s.scroll;
        check("dragging it scrolls", dragged > 0, format!("scroll {dragged}"));
        SendMessageW(s.view, WM_MOUSEMOVE, Some(WPARAM(1)), Some(at(5000)));
        check("dragging past the end stops at the end", s.scroll == forms::max_scroll(page_height(s), view_height(s)), format!("scroll {}", s.scroll));
        SendMessageW(s.view, WM_LBUTTONUP, Some(WPARAM(0)), Some(at(5000)));
        check("letting go ends the drag", s.thumb_grab.is_none(), String::new());
        scroll_to(s, 0);

        // A list with nothing to scroll passes the wheel on.
        select_page(s, PAGE_SNIPPETS);
        let list = s.lists[LIST_SNIPPETS].hwnd;
        SendMessageW(list, WM_MOUSEWHEEL, Some(notch_down), None);
        check("wheel over a short list scrolls the page", s.scroll > 0 || page_height(s) <= view_height(s), format!("scroll {}", s.scroll));
        scroll_to(s, 0);

        // Lists are as long as what's in them.
        let list_height = |list: HWND| {
            let mut client = RECT::default();
            let _ = GetClientRect(list, &mut client);
            client.bottom
        };
        let snippet_rows_shown = s.lists[LIST_SNIPPETS].rows.len().max(1) as i32;
        check("the snippet list fits its rows", list_height(list) == forms::scale(forms::MANAGED_ROW_DIP * snippet_rows_shown, s.style.dpi), format!("{} for {snippet_rows_shown} rows", list_height(list)));
        let before_height = page_height(s);
        let extra = s.lists[LIST_SNIPPETS].rows.clone();
        let mut more = extra.clone();
        more.extend(extra.iter().cloned());
        let added = extra.len().max(0) as i32;
        fill_managed(s, LIST_SNIPPETS, more);
        check("adding rows makes the page taller, not the list scroll", page_height(s) == before_height + forms::scale(forms::MANAGED_ROW_DIP * added, s.style.dpi) && !forms::list_overflows(list), format!("{before_height} to {}", page_height(s)));
        fill_managed(s, LIST_SNIPPETS, extra);
        check("and taking them away puts it back", page_height(s) == before_height, format!("{}", page_height(s)));

        // Settings that mean nothing while their switch is off say so.
        forms::set_checked(s.controls.hyper_on, false);
        apply_dependents(s);
        check("with HyperKey off its settings are off too", !IsWindowEnabled(s.controls.hyper_trigger).as_bool() && !IsWindowEnabled(s.pickers[PICKER_HYPER].list).as_bool(), String::new());
        forms::set_checked(s.controls.hyper_on, true);
        apply_dependents(s);
        check("and they come back with it", IsWindowEnabled(s.controls.hyper_trigger).as_bool() && IsWindowEnabled(s.pickers[PICKER_HYPER].list).as_bool(), String::new());

        // A drop-down of values keeps what it was given, even one it doesn't offer.
        check("a value the list offers reads back the same", s.presets[PRESET_ITEMS].value() == s.base.clipboard.max_items, format!("{}", s.presets[PRESET_ITEMS].value()));
        check("Forever is offered for keeping history", s.presets[PRESET_DAYS].values.contains(&0), String::new());

        // Tab order, the way the Tab key finds it. The feature tests tab 7 times to the search box.
        select_page(s, PAGE_HYPER);
        let mut next = s.list;
        for _ in 0..7 {
            next = GetNextDlgTabItem(root, Some(next), false).unwrap_or_default();
        }
        check("seven Tabs from the page list reach the HyperKey search box", next == s.pickers[PICKER_HYPER].edit, String::new());
        let mut walked = 0;
        let mut item = s.list;
        while walked < 200 {
            item = GetNextDlgTabItem(root, Some(item), false).unwrap_or_default();
            walked += 1;
            if item == s.list {
                break;
            }
        }
        check("Tab goes through the page's controls and back round to the page list", item == s.list, format!("{walked} steps"));

        // Every Tab stop on every page can be brought into view.
        let mut bad = Vec::new();
        for page in 0..PAGES.len() {
            select_page(s, page);
            let mut item = s.list;
            for _ in 0..200 {
                item = GetNextDlgTabItem(root, Some(item), false).unwrap_or_default();
                if item == s.list || item.is_invalid() {
                    break;
                }
                reveal_control(s, item);
                let (mut shown, mut view) = (RECT::default(), RECT::default());
                let _ = GetWindowRect(item, &mut shown);
                let _ = GetWindowRect(s.view, &mut view);
                let fits = shown.bottom - shown.top < view.bottom - view.top;
                if fits && (shown.top < view.top || shown.bottom > view.bottom) {
                    bad.push(format!("{} id {}", PAGES[page].0, GetDlgCtrlID(item)));
                }
            }
        }
        check("every Tab stop scrolls into view", bad.is_empty(), bad.join("; "));

        // Finding controls the way the feature tests do.
        let view = FindWindowExW(Some(root), None, w!("OrbitSettingsView"), None).unwrap_or_default();
        let page = FindWindowExW(Some(view), None, w!("OrbitSettingsPage"), None).unwrap_or_default();
        let found = [200, 201, 202, ID_TOOL_ENTE].iter().all(|id| GetDlgItem(Some(page), *id).is_ok());
        check("controls keep their ids under the page", found, String::new());

        // Saving as things change. The message goes nowhere here, since nothing's listening.
        select_page(s, PAGE_GENERAL);
        let wait_for_timer = || {
            for _ in 0..70 {
                pump();
                std::thread::sleep(std::time::Duration::from_millis(10));
            }
        };
        let was_glass = s.last_saved.launcher.glass;
        forms::set_checked(s.controls.glass, !was_glass);
        SendMessageW(root, WM_COMMAND, Some(WPARAM((BN_CLICKED as usize) << 16)), Some(LPARAM(s.controls.glass.0 as isize)));
        check("a switch saves at once", s.last_saved.launcher.glass != was_glass, String::new());
        forms::set_checked(s.controls.glass, was_glass);
        SendMessageW(root, WM_COMMAND, Some(WPARAM((BN_CLICKED as usize) << 16)), Some(LPARAM(s.controls.glass.0 as isize)));
        // Text that makes no sense holds back only itself.
        select_page(s, PAGE_HOTKEYS);
        let as_list = get_text(s.controls.edit_as_text);
        toggle_hotkey_text(s);
        check("the Hotkeys page changes to text in place", s.text_mode && get_text(s.controls.edit_as_text) != as_list, String::new());
        let good_table = get_lines(s.controls.hyper_table);
        let saved_bindings = s.last_saved.hyper.bindings.clone();
        set_text(s.controls.hyper_table, "not a mapping");
        let _ = save_now(s);
        check("text that makes no sense isn't saved, and says why", s.showing_problem && s.last_saved.hyper.bindings == saved_bindings && !get_text(s.controls.text_error).is_empty(), get_text(s.controls.text_error));
        let was_glass_now = s.last_saved.launcher.glass;
        forms::set_checked(s.controls.glass, !was_glass_now);
        SendMessageW(root, WM_COMMAND, Some(WPARAM((BN_CLICKED as usize) << 16)), Some(LPARAM(s.controls.glass.0 as isize)));
        check("everything else still saves meanwhile", s.last_saved.launcher.glass != was_glass_now, String::new());
        forms::set_checked(s.controls.glass, was_glass_now);
        SendMessageW(root, WM_COMMAND, Some(WPARAM((BN_CLICKED as usize) << 16)), Some(LPARAM(s.controls.glass.0 as isize)));
        toggle_hotkey_text(s);
        check("it can't go back to the list like that", s.text_mode, String::new());
        SendMessageW(root, WM_COMMAND, Some(WPARAM(IDCANCEL.0 as usize)), None);
        check("closing warns once and stays open", state(root).is_some() && s.close_warned, String::new());
        set_text(s.controls.hyper_table, &good_table);
        // A multi-line box doesn't report a change made from code, so this says it did, as typing would.
        SendMessageW(root, WM_COMMAND, Some(WPARAM((EN_CHANGE as usize) << 16)), Some(LPARAM(s.controls.hyper_table.0 as isize)));
        wait_for_timer();
        check("fixing it clears the problem", !s.showing_problem && get_text(s.controls.text_error).is_empty(), String::new());
        toggle_hotkey_text(s);
        check("and the list comes back", !s.text_mode, String::new());
        SendMessageW(root, WM_COMMAND, Some(WPARAM(IDOK.0 as usize)), None);
        check("Enter doesn't close Settings", state(root).is_some(), String::new());

        // A dangerous button asks in place.
        select_page(s, PAGES.len() - 1);
        let danger = GetDlgItem(Some(s.content), ID_RESET_SHORTCUTS).unwrap_or_default();
        let double_click = std::time::Duration::from_millis(GetDoubleClickTime() as u64 + 60);
        check("the first press on a dangerous button asks for a second", !confirm_in_place(s, danger, LABEL_RESET) && get_text(danger) == "Press again", get_text(danger));
        check("and shows red while it waits", s.style.is_alert(danger), String::new());
        check("the other half of a double click doesn't count", !confirm_in_place(s, danger, LABEL_RESET), String::new());
        std::thread::sleep(double_click);
        check("the second goes ahead and puts the label back", confirm_in_place(s, danger, LABEL_RESET) && get_text(danger) == LABEL_RESET && !s.style.is_alert(danger), get_text(danger));

        // Hotkeys are chosen and recorded right there.
        select_page(s, PAGE_HOTKEYS);
        s.action_options.insert(0, ActionOption { name: "Stand-in app".to_string(), kind: "App", action: Action::FocusOrLaunchApp { id: "standin".to_string(), name: "Stand-in app".to_string() } });
        set_text(s.controls.action_search, "stand-in");
        pump();
        check("searching offers what matches", matches!(s.hotkey_rows.first(), Some(HotkeyRow::Match(0))), format!("{} rows", s.hotkey_rows.len()));
        act_on_hotkey_row(s, 0);
        check("choosing one listens for its keys", matches!(s.hotkey_target, Some(RecordTarget::New(_))) && matches!(s.hotkey_rows.last(), Some(HotkeyRow::Pending)), String::new());
        record_hotkey_keys(s, Mods::NONE, "A");
        let hyper_on = is_checked(s.controls.hyper_on);
        check("one key means Hyper with HyperKey on", hyper_on && s.base.hyper.bindings.iter().any(|b| b.key == "A" && b.action == Action::FocusOrLaunchApp { id: "standin".to_string(), name: "Stand-in app".to_string() }), String::new());
        let at = s.hotkey_rows.iter().position(|row| matches!(row, HotkeyRow::Hyper(i) if s.base.hyper.bindings[*i].key == "A")).unwrap_or(0);
        act_on_hotkey_row(s, at);
        forms::set_checked(s.controls.hyper_on, false);
        record_hotkey_keys(s, Mods::NONE, "B");
        check("with HyperKey off one key is refused and it keeps listening", s.hotkey_problem.is_some() && s.hotkey_target.is_some(), s.hotkey_problem.clone().unwrap_or_default());
        record_hotkey_keys(s, Mods::CTRL.union(Mods::ALT), "J");
        let direct = Chord::parse("Ctrl+Alt+J").ok();
        check("a real shortcut is taken, and moves the item's hotkey", s.hotkey_target.is_none() && s.base.shortcuts.iter().any(|b| Some(&b.chord) == direct.as_ref()) && !s.base.hyper.bindings.iter().any(|b| b.key == "A"), String::new());
        forms::set_checked(s.controls.hyper_on, true);
        // Keys something else has need pressing twice, and Esc keeps both.
        s.base.hyper.bindings.push(orbit_core::settings::HyperBinding { key: "Y".to_string(), action: Action::Url { url: "https://example.com/y".to_string() }, enabled: true });
        refresh_hotkey_list(s);
        let at = s.hotkey_rows.iter().position(|row| matches!(row, HotkeyRow::Direct(_))).unwrap_or(0);
        act_on_hotkey_row(s, at);
        record_hotkey_keys(s, Mods::NONE, "Y");
        let still_there = s.base.hyper.bindings.iter().any(|b| b.key == "Y" && matches!(b.action, Action::Url { .. }));
        check("keys in use ask first, and change nothing yet", still_there && s.hotkey_target.is_some() && s.hotkey_asking.is_some(), s.hotkey_problem.clone().unwrap_or_default());
        stop_hotkey_record(s);
        check("Esc keeps both", s.base.hyper.bindings.iter().any(|b| b.key == "Y") && s.base.shortcuts.iter().any(|b| Some(&b.chord) == direct.as_ref()), String::new());
        let at = s.hotkey_rows.iter().position(|row| matches!(row, HotkeyRow::Direct(_))).unwrap_or(0);
        act_on_hotkey_row(s, at);
        record_hotkey_keys(s, Mods::NONE, "Y");
        record_hotkey_keys(s, Mods::NONE, "Y");
        check("pressing them again takes them", s.hotkey_target.is_none() && s.base.hyper.bindings.iter().any(|b| b.key == "Y" && !matches!(b.action, Action::Url { .. })), String::new());
        s.base.hyper.bindings.retain(|b| b.key != "Y");
        s.base.shortcuts.push(orbit_core::settings::ShortcutBinding { chord: direct.clone().unwrap(), action: Action::Builtin { command: Builtin::NewNote }, enabled: true });
        refresh_hotkey_list(s);
        // The list goes back to its own row while listening, and the page follows.
        let many: Vec<_> = ('A'..='Z').map(|key| orbit_core::settings::HyperBinding { key: key.to_string(), action: Action::Url { url: format!("https://example.com/{key}") }, enabled: true }).collect();
        let kept = std::mem::replace(&mut s.base.hyper.bindings, many);
        refresh_hotkey_list(s);
        scroll_to(s, 0);
        let last = s.hotkey_rows.iter().rposition(|row| matches!(row, HotkeyRow::Hyper(_))).unwrap_or(0);
        SendMessageW(s.controls.hotkey_list, LB_SETCURSEL, Some(WPARAM(last)), None);
        act_on_hotkey_row(s, last);
        check("recording a row far down brings it into view", s.scroll > 0, format!("scroll {}", s.scroll));
        stop_hotkey_record(s);
        s.base.hyper.bindings = kept;
        refresh_hotkey_list(s);
        scroll_to(s, 0);
        // A press and release have to agree before a list acts.
        let hotkey_list = s.controls.hotkey_list;
        let first = s.hotkey_rows.iter().position(|row| matches!(row, HotkeyRow::Hyper(_) | HotkeyRow::Direct(_))).unwrap_or(0);
        let mut row_rect = RECT::default();
        SendMessageW(hotkey_list, LB_GETITEMRECT, Some(WPARAM(first)), Some(LPARAM(&mut row_rect as *mut RECT as isize)));
        let mut list_client = RECT::default();
        let _ = GetClientRect(hotkey_list, &mut list_client);
        let first_keys = hotkey_row_keys(s, s.hotkey_rows[first]).unwrap_or_default();
        let (_, remove) = hotkey_spans(s, &first_keys, list_client.right);
        let on_remove = LPARAM((((row_rect.top + 5) as isize) << 16) | ((remove.0 + 4) as isize));
        SendMessageW(hotkey_list, WM_LBUTTONUP, Some(WPARAM(0)), Some(on_remove));
        check("a release with no press does nothing", s.hotkey_armed.is_none(), String::new());
        SendMessageW(hotkey_list, WM_LBUTTONDOWN, Some(WPARAM(1)), Some(on_remove));
        SendMessageW(hotkey_list, WM_LBUTTONUP, Some(WPARAM(0)), Some(on_remove));
        check("a click on Remove asks", s.hotkey_armed == Some(first), String::new());
        SendMessageW(hotkey_list, WM_LBUTTONDBLCLK, Some(WPARAM(1)), Some(on_remove));
        SendMessageW(hotkey_list, WM_LBUTTONUP, Some(WPARAM(0)), Some(on_remove));
        check("the other half of a double click doesn't remove it", s.hotkey_armed == Some(first), String::new());
        s.hotkey_armed = None;
        let count = s.base.shortcuts.len();
        let at = s.hotkey_rows.iter().position(|row| matches!(row, HotkeyRow::Direct(_))).unwrap_or(0);
        delete_hotkey_at(s, at);
        check("the first Remove asks", s.hotkey_armed == Some(at) && s.base.shortcuts.len() == count, String::new());
        delete_hotkey_at(s, at);
        check("the second removes it", s.base.shortcuts.len() == count - 1, String::new());
        let _ = save_now(s);

        // Ctrl+Backspace. The keyboard state set here is only this thread's idea of it.
        select_page(s, PAGE_GENERAL);
        let box_for_keys = s.pickers[PICKER_HYPER].edit;
        let mut keys = [0u8; 256];
        let _ = GetKeyboardState(&mut keys);
        keys[VK_CONTROL.0 as usize] = 0x80;
        let _ = SetKeyboardState(&keys);
        set_text(box_for_keys, "hello wor.ld");
        SendMessageW(box_for_keys, 0xB1, Some(WPARAM(12)), Some(LPARAM(12)));
        SendMessageW(box_for_keys, WM_KEYDOWN, Some(WPARAM(VK_BACK.0 as usize)), None);
        SendMessageW(box_for_keys, WM_CHAR, Some(WPARAM(0x7F)), None);
        check("Ctrl+Backspace deletes the last word and types nothing", get_text(box_for_keys) == "hello wor.", get_text(box_for_keys));
        SendMessageW(box_for_keys, 0xB1, Some(WPARAM(0)), Some(LPARAM(5)));
        SendMessageW(box_for_keys, WM_KEYDOWN, Some(WPARAM(VK_BACK.0 as usize)), None);
        check("with a selection, it deletes the selection", get_text(box_for_keys) == " wor.", get_text(box_for_keys));
        SendMessageW(box_for_keys, 0x304, None, None); // EM_UNDO would be 0xC7, and WM_UNDO is this
        check("and Undo brings it back", get_text(box_for_keys) == "hello wor.", get_text(box_for_keys));
        SendMessageW(box_for_keys, WM_KEYDOWN, Some(WPARAM('A' as usize)), None);
        SendMessageW(box_for_keys, WM_CHAR, Some(WPARAM(0x01)), None);
        let (mut from, mut to) = (0u32, 0u32);
        SendMessageW(box_for_keys, 0xB0, Some(WPARAM(&mut from as *mut u32 as usize)), Some(LPARAM(&mut to as *mut u32 as isize)));
        check("Ctrl+A selects everything and types nothing", from == 0 && to as usize == get_text(box_for_keys).len(), format!("{from}..{to}"));
        // The recorder takes the chord instead.
        let recorder = s.controls.hotkey;
        let was_chord = s.launcher_chord.clone();
        SendMessageW(recorder, WM_LBUTTONDOWN, Some(WPARAM(1)), None);
        check("a click starts the recorder", s.recording_launcher, String::new());
        SendMessageW(recorder, WM_KEYDOWN, Some(WPARAM(VK_BACK.0 as usize)), None);
        check("it records Ctrl+Backspace as a chord", !s.recording_launcher && s.launcher_chord.as_ref().is_some_and(|c| c.display("Win").to_lowercase().contains("ctrl")), s.launcher_chord.as_ref().map(|c| c.display("Win")).unwrap_or_default());
        keys[VK_CONTROL.0 as usize] = 0;
        let _ = SetKeyboardState(&keys);
        SendMessageW(recorder, WM_LBUTTONDOWN, Some(WPARAM(1)), None);
        SendMessageW(recorder, WM_KEYDOWN, Some(WPARAM('A' as usize)), None);
        check("a plain key is refused, and it keeps listening", s.recording_launcher && s.launcher_chord != None && s.style.is_alert(s.controls.hotkey_note), get_text(s.controls.hotkey_note));
        SendMessageW(recorder, WM_KEYDOWN, Some(WPARAM(VK_ESCAPE.0 as usize)), None);
        check("Esc stops listening", !s.recording_launcher, String::new());
        SendMessageW(recorder, WM_LBUTTONDOWN, Some(WPARAM(1)), None);
        SendMessageW(recorder, WM_KEYDOWN, Some(WPARAM(VK_BACK.0 as usize)), None);
        check("Backspace goes back to the default", !s.recording_launcher && s.launcher_chord.is_none(), String::new());
        s.launcher_chord = was_chord;
        let _ = save_now(s);

        // Delete asks first, and Edit and Delete reach the launcher as a request.
        select_page(s, PAGE_LINKS);
        pump();
        let links = s.lists[LIST_LINKS].hwnd;
        let mut client = RECT::default();
        let _ = GetClientRect(links, &mut client);
        let (edit_span, delete_span) = forms::managed_buttons(client.right, s.style.dpi);
        let row_y = forms::scale(forms::MANAGED_ROW_DIP / 2, s.style.dpi);
        let press = |x: i32| {
            SendMessageW(links, WM_LBUTTONDOWN, Some(WPARAM(1)), Some(LPARAM(((row_y as isize) << 16) | x as isize)));
            SendMessageW(links, WM_LBUTTONUP, Some(WPARAM(0)), Some(LPARAM(((row_y as isize) << 16) | x as isize)));
        };
        let take_request = || {
            let mut message = MSG::default();
            if PeekMessageW(&mut message, None, WM_ORBIT_LINK_REQUEST, WM_ORBIT_LINK_REQUEST, PM_REMOVE).as_bool() {
                crate::util::take_posted::<LinkRequest>(message.wParam.0).map(|boxed| *boxed)
            } else {
                None
            }
        };
        press((delete_span.0 + delete_span.1) / 2);
        check("the first press on Delete arms the row", s.lists[LIST_LINKS].armed == Some(0) && take_request().is_none(), format!("{:?}", s.lists[LIST_LINKS].armed));
        press((delete_span.0 + delete_span.1) / 2);
        let id = s.base.links.first().map(|l| l.id.clone()).unwrap_or_default();
        check("the second asks the launcher to delete it", take_request() == Some(LinkRequest::DeleteLink(id.clone())), id.clone());
        press((edit_span.0 + edit_span.1) / 2);
        check("Edit asks the launcher to open the form", take_request() == Some(LinkRequest::EditLink(id.clone())), id);
        press(forms::scale(100, s.style.dpi));
        check("a press on the row itself asks for nothing", take_request().is_none(), String::new());
        scroll_to(s, 0);

        // A long list of apps shows its start, and a row for the rest.
        let picker_list = s.pickers[PICKER_HYPER].list;
        select_page(s, PAGE_HYPER);
        let was_chosen = s.pickers[PICKER_HYPER].chosen.clone();
        s.pickers[PICKER_HYPER].chosen = (0..10).map(|at| format!("standin{at}.exe")).collect();
        s.pickers[PICKER_HYPER].expanded = false;
        set_text(s.pickers[PICKER_HYPER].edit, "");
        refresh_picker(s, PICKER_HYPER);
        let shown = s.pickers[PICKER_HYPER].rows.len();
        check("ten apps show as five and a row for the rest", shown == 6 && matches!(s.pickers[PICKER_HYPER].rows[5], PickerRow::ShowAll(10)), format!("{shown} rows"));
        act_on_picker_row(s, PICKER_HYPER, 5);
        check("and that row shows them all", s.pickers[PICKER_HYPER].rows.len() == 10 && list_height(picker_list) == forms::scale(PICKER_ROW_DIP * 10, s.style.dpi), format!("{} rows", s.pickers[PICKER_HYPER].rows.len()));
        let mut picker_client = RECT::default();
        let _ = GetClientRect(picker_list, &mut picker_client);
        let on_remove = LPARAM(((forms::scale(PICKER_ROW_DIP / 2, s.style.dpi) as isize) << 16) | (picker_client.right - forms::scale(20, s.style.dpi)) as isize);
        SendMessageW(picker_list, WM_LBUTTONDOWN, Some(WPARAM(1)), Some(on_remove));
        SendMessageW(picker_list, WM_LBUTTONUP, Some(WPARAM(0)), Some(on_remove));
        check("a click on Remove removes one app", s.pickers[PICKER_HYPER].chosen.len() == 9, format!("{} left", s.pickers[PICKER_HYPER].chosen.len()));
        SendMessageW(picker_list, WM_LBUTTONDBLCLK, Some(WPARAM(1)), Some(on_remove));
        SendMessageW(picker_list, WM_LBUTTONUP, Some(WPARAM(0)), Some(on_remove));
        check("and the other half of a double click removes no more", s.pickers[PICKER_HYPER].chosen.len() == 9, format!("{} left", s.pickers[PICKER_HYPER].chosen.len()));
        let delete_held = LPARAM(1 << 30);
        SendMessageW(picker_list, LB_SETCURSEL, Some(WPARAM(0)), None);
        SendMessageW(picker_list, WM_KEYDOWN, Some(WPARAM(VK_DELETE.0 as usize)), Some(delete_held));
        check("holding Delete doesn't remove the apps below", s.pickers[PICKER_HYPER].chosen.len() == 9, format!("{} left", s.pickers[PICKER_HYPER].chosen.len()));
        s.pickers[PICKER_HYPER].expanded = false;
        refresh_picker(s, PICKER_HYPER);
        set_text(s.pickers[PICKER_HYPER].edit, "standin-new.exe");
        pump();
        act_on_picker_row(s, PICKER_HYPER, 0);
        check("an app just added shows, at the end of a long list", s.pickers[PICKER_HYPER].expanded && s.pickers[PICKER_HYPER].rows.iter().any(|row| matches!(row, PickerRow::App { program, .. } if program == "standin-new.exe")), format!("{} rows", s.pickers[PICKER_HYPER].rows.len()));
        let added_at = SendMessageW(picker_list, LB_GETCURSEL, None, None).0;
        let mut added_row = RECT::default();
        SendMessageW(picker_list, LB_GETITEMRECT, Some(WPARAM(added_at.max(0) as usize)), Some(LPARAM(&mut added_row as *mut RECT as isize)));
        let mut added_top = POINT { x: 0, y: added_row.top };
        let _ = ClientToScreen(picker_list, &mut added_top);
        let mut view_rect = RECT::default();
        let _ = GetWindowRect(s.view, &mut view_rect);
        check("and the page has moved to it", added_top.y >= view_rect.top && added_top.y + added_row.bottom - added_row.top <= view_rect.bottom, format!("row at {} in {}..{}", added_top.y, view_rect.top, view_rect.bottom));
        check("and it's lit", s.pickers[PICKER_HYPER].just_added, String::new());
        let before_release = s.pickers[PICKER_HYPER].chosen.len();
        SendMessageW(picker_list, WM_LBUTTONUP, Some(WPARAM(0)), Some(on_remove));
        check("nor does a release left over from a closing dialog", s.pickers[PICKER_HYPER].chosen.len() == before_release, String::new());
        s.pickers[PICKER_HYPER].chosen = was_chosen;
        refresh_picker(s, PICKER_HYPER);

        // A click on nothing takes back a recording.
        select_page(s, PAGE_GENERAL);
        SendMessageW(s.controls.hotkey, WM_LBUTTONDOWN, Some(WPARAM(1)), None);
        SendMessageW(s.content, WM_LBUTTONDOWN, Some(WPARAM(1)), None);
        check("clicking on the page stops listening", !s.recording_launcher, String::new());
        // Holding Esc is one press, and Esc on a first press only takes that back.
        let repeat_esc = MSG { hwnd: s.controls.hotkey, message: WM_KEYDOWN, wParam: WPARAM(VK_ESCAPE.0 as usize), lParam: LPARAM(1 << 30), ..Default::default() };
        check("repeats of a held Esc don't reach the window", eats_key_text(&repeat_esc), String::new());
        let fresh_esc = MSG { lParam: LPARAM(0), ..repeat_esc };
        check("a new press of Esc does", !eats_key_text(&fresh_esc), String::new());
        let repeat_enter = MSG { hwnd: s.controls.hotkey, message: WM_KEYDOWN, wParam: WPARAM(VK_RETURN.0 as usize), lParam: LPARAM(1 << 30), ..Default::default() };
        check("repeats of a held Enter don't either, so holding it can't confirm anything", eats_key_text(&repeat_enter), String::new());
        check("but it still repeats in the hotkey tables", !eats_key_text(&MSG { hwnd: s.controls.hyper_table, ..repeat_enter }), String::new());
        // The ring round what has the focus is for the keyboard.
        let click_message = MSG { hwnd: s.controls.hotkey, message: WM_LBUTTONDOWN, ..Default::default() };
        let _ = eats_key_text(&click_message);
        check("a click hides the focus ring", !s.style.focus_by_keyboard, String::new());
        let _ = eats_key_text(&MSG { hwnd: s.controls.hotkey, message: WM_KEYDOWN, wParam: WPARAM(VK_TAB.0 as usize), ..Default::default() });
        check("Tab shows it", s.style.focus_by_keyboard, String::new());
        let reset_button = GetDlgItem(Some(s.content), ID_RESET_SHORTCUTS).unwrap_or_default();
        confirm_in_place(s, reset_button, LABEL_RESET);
        SendMessageW(root, WM_COMMAND, Some(WPARAM(IDCANCEL.0 as usize)), None);
        check("Esc takes back a first press and leaves Settings open", state(root).is_some() && s.armed_button.is_none(), String::new());
        // Keys that finished a recording do nothing more.
        s.swallow_keys = true;
        let swallowed = SendMessageW(s.controls.hotkey, WM_SYSCHAR, Some(WPARAM(' ' as usize)), None).0 == 0;
        check("what they'd type doesn't reach the window's menu", swallowed && eats_key_text(&MSG { message: WM_SYSCHAR, ..Default::default() }), String::new());
        s.swallow_keys = false;

        // A write that failed is tried again, with nothing new to save.
        let mut message = MSG::default();
        while PeekMessageW(&mut message, None, WM_ORBIT_SETTINGS_APPLY, WM_ORBIT_SETTINGS_APPLY, PM_REMOVE).as_bool() {}
        applied(Some(false), &[], "", &[]);
        check("a failed write says so and stays", s.save_pending && s.style.is_alert(s.status), get_text(s.status));
        let _ = save_now(s);
        // In a preview this window is its own Orbit, so the message may land on any window of the thread.
        let mut posted = false;
        for _ in 0..20 {
            if PeekMessageW(&mut message, None, WM_ORBIT_SETTINGS_APPLY, WM_ORBIT_SETTINGS_APPLY, PM_REMOVE).as_bool() {
                posted = true;
                let _ = crate::util::take_posted::<Settings>(message.wParam.0);
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        check("and Enter tries it again with nothing new", posted, String::new());
        s.replaced_note = None;
        applied(Some(true), &[], "", &[]);
        check("a good write clears that", !s.save_pending && !s.style.is_alert(s.status) && get_text(s.status) == "Saved", get_text(s.status));
        // A message that comes along meanwhile doesn't end the warning.
        applied(Some(false), &[], "", &[]);
        show_status(s, "Exported", 1);
        check("another message can show meanwhile", get_text(s.status) == "Exported", get_text(s.status));
        std::thread::sleep(std::time::Duration::from_millis(30));
        pump();
        check("but the failed write comes back after it", s.style.is_alert(s.status) && get_text(s.status) == SAVE_FAILED, get_text(s.status));
        s.replaced_note = None;
        applied(Some(true), &[], "", &[]);
        // A result stays up through the save that follows it, but a failed save still shows.
        show_result(s, "Installed for the check");
        applied(Some(true), &[], "", &[]);
        check("a save doesn't talk over a result", get_text(s.status) == "Installed for the check", get_text(s.status));
        applied(Some(false), &[], "", &[]);
        check("but a failed save does", get_text(s.status) == SAVE_FAILED, get_text(s.status));
        s.result_until = None;
        applied(Some(true), &[], "", &[]);
        // A button that disables itself hands the keyboard on.
        let page_before = s.page;
        show_page(s, PAGE_LINKS);
        let hidden_button = s.controls.hidden_button;
        let _ = EnableWindow(hidden_button, true);
        // A preview doesn't move the focus itself, so this asks where it would go.
        let handed_to = next_in_tab_order(s, hidden_button);
        check("a button that disables itself has somewhere to hand the focus", handed_to.is_some(), format!("{handed_to:?}"));
        let hidden_left = s.base.hidden.clone();
        applied(None, &[], "", &hidden_left);
        show_page(s, page_before);
        // The next key ends the light on a row just added.
        let lit = s.pickers[PICKER_HYPER].just_added;
        s.pickers[PICKER_HYPER].just_added = true;
        let _ = eats_key_text(&MSG { hwnd: s.controls.hotkey, message: WM_KEYDOWN, wParam: WPARAM(VK_DOWN.0 as usize), ..Default::default() });
        check("the next key ends the light on an added row", !s.pickers[PICKER_HYPER].just_added, format!("was {lit}"));

        // The row says what the launcher opens on, and only "Specific screen" shows the picture.
        {
            use orbit_core::settings::LauncherScreen;
            select_page(s, PAGE_GENERAL);
            let screens = crate::displays::list();
            let specific = |at: usize| LauncherScreen::Specific { id: screens[at].id.clone(), name: screens[at].name.clone() };
            let (screen, screen_mode, below) = (s.controls.screen, s.controls.screen_mode, s.controls.hotkey);
            // As choosing in the row does, whether it's side by side or a drop-down.
            let pick = |mode: usize| {
                SendMessageW(screen_mode, CB_SETCURSEL, Some(WPARAM(mode)), None);
                SendMessageW(root, WM_COMMAND, Some(WPARAM((CBN_SELCHANGE as usize) << 16)), Some(LPARAM(screen_mode.0 as isize)));
            };
            let folded = |s: &State| place_on_page(s, screen).2 == 0 && !IsWindowEnabled(screen).as_bool();
            pick(0);
            let (page_before, below_before) = (s.page_heights[PAGE_GENERAL], place_on_page(s, below).1);
            check("the active window's screen hides the picture", s.last_saved.launcher.screen == LauncherScreen::Active && folded(s), format!("{:?}, {} high", s.last_saved.launcher.screen, place_on_page(s, screen).2));
            pick(2);
            check("Main display opens on Windows' main screen, with no picture", s.last_saved.launcher.screen == LauncherScreen::Main && folded(s), format!("{:?}", s.last_saved.launcher.screen));
            pick(3);
            let tall = screen_picker::height(screen);
            check("Specific screen picks one and shows the picture", matches!(s.last_saved.launcher.screen, LauncherScreen::Specific { .. }) && tall > 0 && place_on_page(s, screen).2 == tall && IsWindowEnabled(screen).as_bool(), format!("{:?}, {} high", s.last_saved.launcher.screen, place_on_page(s, screen).2));
            check("and the page grows to fit it", s.page_heights[PAGE_GENERAL] == page_before + tall && place_on_page(s, below).1 == below_before + tall, format!("{} then {}", page_before, s.page_heights[PAGE_GENERAL]));
            if screens.len() >= 2 {
                let last = screens.len() - 1;
                let (x, y) = screen_picker::preview_point(screen, last).unwrap_or((-1, -1));
                SendMessageW(screen, WM_LBUTTONDOWN, Some(WPARAM(1)), Some(LPARAM(((y as isize) << 16) | (x as isize & 0xFFFF))));
                check("clicking a screen picks it", s.last_saved.launcher.screen == specific(last), format!("{:?}", s.last_saved.launcher.screen));
                check("and the line under the picture names it", get_text(screen) == screens[last].describe(), get_text(screen));
                SendMessageW(screen, WM_KEYDOWN, Some(WPARAM(VK_LEFT.0 as usize)), None);
                check("Left picks the screen to its left", s.last_saved.launcher.screen == specific(last - 1), format!("{:?}", s.last_saved.launcher.screen));
            } else {
                check("the picture shows the one screen", screens.len() == 1 && screen_picker::preview_point(screen, 0).is_some(), format!("{} screens", screens.len()));
            }
            let picked = s.last_saved.launcher.screen.clone();
            pick(1);
            check("Pointer hides the picture again", s.last_saved.launcher.screen == LauncherScreen::Pointer && folded(s), format!("{:?}", s.last_saved.launcher.screen));
            check("and the page closes up", s.page_heights[PAGE_GENERAL] == page_before && place_on_page(s, below).1 == below_before, format!("{} then {}", page_before, s.page_heights[PAGE_GENERAL]));
            pick(3);
            check("Specific screen again brings back the screen picked before", s.last_saved.launcher.screen == picked, format!("{:?}", s.last_saved.launcher.screen));
            SendMessageW(root, WM_DISPLAYCHANGE, None, None);
            let _ = save_now(s);
            check("the screens changing keeps it", s.last_saved.launcher.screen == picked, format!("{:?}", s.last_saved.launcher.screen));
        }

        // The last good text is what a later mistake falls back to.
        select_page(s, PAGE_HOTKEYS);
        toggle_hotkey_text(s);
        set_text(s.controls.hyper_table, "Z = cmd:new-note");
        set_text(s.controls.shortcuts, "");
        let _ = save_now(s);
        let good = s.last_saved.hyper.bindings.clone();
        check("good text saves", good.iter().any(|b| b.key == "Z"), String::new());
        set_text(s.controls.hyper_table, "not a mapping");
        let _ = save_now(s);
        check("text gone bad doesn't take the good text back", s.last_saved.hyper.bindings == good, String::new());
        check("and only that table goes red", s.style.is_alert(s.controls.hyper_table) && !s.style.is_alert(s.controls.shortcuts), String::new());

        // Rebuilding the window, as a new theme or screen does, keeps text being edited.
        let (context, settings, dir) = (snippet_context(s), s.base.clone(), s.dir.clone());
        s.save_pending = true;
        reopen(context, settings, dir, &[]);
        match CURRENT.with(|c| c.get()).and_then(state) {
            Some(rebuilt) => {
                check("a rebuild keeps the text view and what's in it", rebuilt.text_mode && get_lines(rebuilt.controls.hyper_table) == "not a mapping", get_lines(rebuilt.controls.hyper_table));
                check("and says what's wrong with it", rebuilt.showing_problem && !get_text(rebuilt.controls.text_error).is_empty(), get_text(rebuilt.controls.text_error));
                check("and stays on the page", rebuilt.page == PAGE_HOTKEYS, format!("page {}", rebuilt.page));
                check("and a failed write still shows in red", rebuilt.save_pending && rebuilt.style.is_alert(rebuilt.status) && get_text(rebuilt.status) == SAVE_FAILED, get_text(rebuilt.status));
            }
            None => check("a rebuild leaves a window", false, String::new()),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn links_and_commands_make_friendly_rows() {
        let rows = link_rows(&[orbit_core::settings::QuickLink { id: "a".into(), name: "github".into(), url: "https://github.com".into(), alias: "gh".into(), open_with: String::new() }], None);
        assert_eq!(rows[0], ManagedRow { tile: Tile::Letter('G'), title: "github".into(), pill: "gh".into(), detail: "https://github.com".into(), picture: None });
        let command = |arguments: &str| orbit_core::settings::CustomCommand { id: "c".into(), name: "Build".into(), program: "cargo".into(), arguments: arguments.into(), working_dir: String::new(), alias: String::new() };
        assert_eq!(command_rows(&[command("build")])[0].detail, "cargo build");
        assert_eq!(command_rows(&[command("")])[0].detail, "cargo");
    }

    #[test]
    fn snippet_rows_keep_only_snippets_and_line_up_with_their_ids() {
        let rows = vec![
            DataRow::Snippet { id: 7, name: "Sign-off".into(), trigger: "sig".into(), preview: "Thanks".into() },
            DataRow::Snippet { id: 9, name: "Address".into(), trigger: String::new(), preview: "1 Road".into() },
        ];
        let (rows, ids) = snippet_rows(rows);
        assert_eq!(ids, [7, 9]);
        assert_eq!(rows[0].pill, "sig");
        assert_eq!(rows[1].pill, "");
        assert_eq!(rows[1].title, "Address");
    }

    #[test]
    fn the_preview_finds_pages_by_short_names() {
        assert_eq!(preview_page_index("general"), Some(PAGE_GENERAL));
        assert_eq!(preview_page_index("Links"), Some(PAGE_LINKS));
        assert_eq!(preview_page_index("links & commands"), Some(PAGE_LINKS));
        assert_eq!(preview_page_index("snippets"), Some(PAGE_SNIPPETS));
        assert_eq!(preview_page_index("search"), Some(PAGE_SEARCH));
        assert_eq!(preview_page_index("nope"), None);
    }

    #[test]
    fn an_alias_goes_where_the_launcher_reads_it() {
        let mut settings = Settings::default();
        settings.links.push(orbit_core::settings::QuickLink { id: "l1".into(), name: "Docs".into(), url: "https://example.com".into(), alias: String::new(), open_with: String::new() });
        let link = crate::catalog::link_key("l1");
        set_item_alias(&mut settings, &link, "dd");
        assert_eq!(settings.links.iter().find(|l| l.id == "l1").map(|l| l.alias.as_str()), Some("dd"), "a quicklink keeps its own");
        assert!(settings.aliases.is_empty());
        assert_eq!(item_alias(&settings, &link), "dd");
        set_item_alias(&mut settings, "app:code", "ed");
        assert_eq!(settings.aliases.get("app:code").map(String::as_str), Some("ed"), "anything else goes in the list");
        set_item_alias(&mut settings, "app:code", "");
        assert!(settings.aliases.is_empty(), "empty takes it away");
    }

    #[test]
    fn a_search_row_has_its_parts_left_to_right() {
        let dpi = 96;
        let width = 600;
        let [alias, hotkey, switch] = search_columns(dpi, width);
        assert!(alias.1 <= hotkey.0 && hotkey.1 <= switch.0 && switch.1 <= width);
        assert_eq!(search_cell_at(dpi, width, alias.0 + 1), Some(SearchCell::Alias));
        assert_eq!(search_cell_at(dpi, width, hotkey.0 + 1), Some(SearchCell::Hotkey));
        assert_eq!(search_cell_at(dpi, width, switch.0 + 1), Some(SearchCell::Switch));
        assert_eq!(search_cell_at(dpi, width, 20), None, "the name is just the name");
    }

    #[test]
    fn every_page_has_a_name_and_an_icon() {
        assert_eq!(PAGES.len(), 10);
        assert_eq!(PAGES[PAGE_SEARCH].0, "Search");
        assert_eq!(PAGES[PAGE_INTEGRATIONS].0, "Integrations");
        assert_eq!(PAGES[PAGE_HOTKEYS].0, "Hotkeys");
        assert_eq!(PAGES[PAGE_HYPER].0, "HyperKey");
        assert_eq!(PAGES[PAGE_CLIPBOARD].0, "Clipboard");
        assert_eq!(PAGES[PAGE_LINKS].0, "Links & commands");
    }

    #[test]
    fn hotkey_search_puts_the_start_of_a_name_first() {
        let option = |name: &str| ActionOption { name: name.to_string(), kind: "App", action: Action::Builtin { command: Builtin::NewNote } };
        let options = [option("Notepad"), option("Search Notes"), option("Keynote"), option("Calculator")];
        assert_eq!(matching_actions(&options, "note"), [0, 1, 2]);
        assert!(matching_actions(&options, "zzz").is_empty());
    }

    #[test]
    fn moving_through_hotkey_rows_skips_headings() {
        let rows = [HotkeyRow::HyperHeader, HotkeyRow::Hyper(0), HotkeyRow::DirectHeader, HotkeyRow::Direct(0)];
        assert_eq!(nearest_hotkey_row(&rows, 0, 1), Some(1));
        assert_eq!(nearest_hotkey_row(&rows, 2, 1), Some(3));
        assert_eq!(nearest_hotkey_row(&rows, 2, -1), Some(1));
        assert_eq!(nearest_hotkey_row(&[HotkeyRow::HyperHeader], 0, 1), None);
    }

    #[test]
    fn offered_values_include_each_default() {
        let defaults = Settings::default();
        assert!(TAP_TIMES.iter().any(|t| t.0 == defaults.hyper.tap_timeout_ms));
        assert!(CLIP_ITEMS.iter().any(|t| t.0 == defaults.clipboard.max_items));
        assert!(CLIP_SIZES.iter().any(|t| t.0 == defaults.clipboard.max_item_bytes));
        assert!(CLIP_DAYS.iter().any(|t| t.0 == defaults.clipboard.retention_days));
    }

    #[test]
    fn offered_values_cover_every_setting() {
        assert!(TAPS.iter().any(|t| t.1 == TapBehavior::Leader));
        assert!(MODES.iter().any(|m| m.1 == HyperMode::External));
        assert!(APPEARANCE.iter().any(|a| a.1 == Appearance::Dark));
    }
}
