//! Note and snippet editor windows.
//!
//! Notes use RichEdit in plain-text mode, for multi-level undo and very long text. The library
//! loads the first time a note opens, so people who never write a note never pay for it.
//! Each window owns its state through `GWLP_USERDATA` and talks to the data thread through a
//! channel, so these windows never touch the launcher's `App`.

use crate::forms::{self, Builder};
use crate::tray;
use crate::ui::Theme;
use crate::util::{SendHwnd, wide};
use crate::worker::Req;
use orbit_core::snippet::{TriggerProblem, validate_trigger};
use orbit_data::{Snippet, SnippetInput};
use std::cell::RefCell;
use std::sync::mpsc::Sender;
use std::sync::atomic::{AtomicU64, Ordering};
use windows::Win32::Foundation::*;
use windows::Win32::Graphics::Dwm::{DWMWA_BORDER_COLOR, DWMWA_CAPTION_COLOR, DWMWA_USE_IMMERSIVE_DARK_MODE, DwmSetWindowAttribute};
use windows::Win32::Graphics::Gdi::*;
use windows::Win32::System::LibraryLoader::{GetModuleHandleW, LoadLibraryW};
use windows::Win32::UI::Controls::{EM_SETLIMITTEXT, EM_SETRECT, EM_SETSEL, SetWindowTheme};
use windows::Win32::UI::Controls::RichEdit::{
    CFM_COLOR, CHARFORMATW, EM_EXLIMITTEXT, EM_SETBKGNDCOLOR, EM_SETCHARFORMAT, EM_SETEVENTMASK, EM_SETTEXTMODE,
    ENM_CHANGE, SCF_ALL, TM_MULTILEVELUNDO, TM_PLAINTEXT,
};
use windows::Win32::UI::HiDpi::{GetDpiForSystem, GetDpiForWindow};
use windows::Win32::UI::Input::KeyboardAndMouse::{GetKeyState, SetFocus, VK_CONTROL, VK_ESCAPE};
use windows::Win32::UI::WindowsAndMessaging::*;
use windows::core::{BOOL, PCWSTR, w};

pub const WM_EDITOR_SAVED: u32 = WM_APP + 40;
const TIMER_AUTOSAVE: usize = 1;
const AUTOSAVE_MS: u32 = 400;

const ID_NAME: i32 = 10;
const ID_TRIGGER: i32 = 11;
const ID_CONTENT: i32 = 12;
const ID_STATUS: i32 = 13;
const ID_SAVE: i32 = 14;
const ID_CANCEL: i32 = 15;
/// The buttons that drop a placeholder into a snippet's text.
const ID_CHIPS: i32 = 20;
/// Each chip's name and what it inserts, as `orbit_core::snippet::expand_template` reads it.
const PLACEHOLDERS: [(&str, &str); 4] = [("Date", "{date}"), ("Time", "{time}"), ("Clipboard", "{clipboard}"), ("Cursor", "{cursor}")];
const SNIPPET_WIDTH_DIP: i32 = 560;
const SNIPPET_SIDE_DIP: i32 = 24;

static NEXT_TOKEN: AtomicU64 = AtomicU64::new(1);

pub struct EditorContext {
    pub tx: Sender<Req>,
    pub main: SendHwnd,
    pub theme: Theme,
}

thread_local! {
    static WINDOWS: RefCell<Vec<HWND>> = const { RefCell::new(Vec::new()) };
    /// Windows that take Tab between fields, for the message loop's `IsDialogMessage`.
    static FORMS: RefCell<Vec<HWND>> = const { RefCell::new(Vec::new()) };
}

pub fn register_form(hwnd: HWND) {
    FORMS.with(|forms| forms.borrow_mut().push(hwnd));
}

pub fn unregister_form(hwnd: HWND) {
    FORMS.with(|forms| forms.borrow_mut().retain(|f| *f != hwnd));
}

/// The form this message belongs to, if any.
pub fn form_for(message: &MSG) -> Option<HWND> {
    if message.hwnd.is_invalid() {
        return None;
    }
    FORMS.with(|forms| {
        let forms = forms.borrow();
        if forms.is_empty() {
            return None;
        }
        let root = unsafe { GetAncestor(message.hwnd, GA_ROOT) };
        forms.iter().copied().find(|form| *form == root)
    })
}

/// Open note windows. RichEdit can be unloaded when this reaches zero.
fn open_note_count() -> usize {
    WINDOWS.with(|list| {
        list.borrow()
            .iter()
            .filter(|hwnd| unsafe { (GetWindowLongPtrW(**hwnd, GWLP_USERDATA) as *mut NoteState).is_null() == false && is_note(**hwnd) })
            .count()
    })
}

fn is_note(hwnd: HWND) -> bool {
    let mut name = [0u16; 16];
    let length = unsafe { GetClassNameW(hwnd, &mut name) } as usize;
    String::from_utf16_lossy(&name[..length]) == "OrbitNote"
}

/// Unloads RichEdit once no note is open, so a one-off note doesn't cost memory for the rest of
/// the session. It loads again in a few milliseconds when the next note opens.
pub fn release_rich_edit() {
    if open_note_count() != 0 {
        return;
    }
    unsafe {
        if let Ok(module) = GetModuleHandleW(w!("msftedit.dll")) {
            let _ = FreeLibrary(module);
        }
    }
}

fn track(hwnd: HWND) {
    WINDOWS.with(|list| list.borrow_mut().push(hwnd));
}

fn untrack(hwnd: HWND) {
    WINDOWS.with(|list| list.borrow_mut().retain(|w| *w != hwnd));
}

/// Closes every editor, which saves anything unsaved. Called before the data thread stops.
pub fn close_all() {
    let windows: Vec<HWND> = WINDOWS.with(|list| list.borrow().clone());
    for hwnd in windows {
        unsafe {
            SendMessageW(hwnd, WM_CLOSE, None, None);
        }
    }
}

pub fn apply_title_bar_theme(hwnd: HWND, theme: &Theme) {
    let dark: BOOL = (theme.background.0 < 128).into();
    unsafe {
        let _ = DwmSetWindowAttribute(
            hwnd,
            DWMWA_USE_IMMERSIVE_DARK_MODE,
            &dark as *const _ as *const _,
            std::mem::size_of::<BOOL>() as u32,
        );
        // The title bar is part of the window, not a strip above it. Windows 10 ignores these, and
        // high contrast keeps its own colours.
        if !theme.high_contrast {
            let color = theme.sidebar.colorref();
            for attribute in [DWMWA_CAPTION_COLOR, DWMWA_BORDER_COLOR] {
                let _ = DwmSetWindowAttribute(hwnd, attribute, &color as *const _ as *const _, std::mem::size_of_val(&color) as u32);
            }
        }
    }
}

fn bring_forward(hwnd: HWND, focus: HWND) {
    forms::show_form(hwnd, focus);
}

// --- Notes --------------------------------------------------------------------------------

struct NoteState {
    tx: Sender<Req>,
    token: u64,
    id: Option<i64>,
    edit: HWND,
    font: HFONT,
    background: Option<HBRUSH>,
    dirty: bool,
    original_edit_proc: isize,
    self_handle: SendHwnd,
    main: SendHwnd,
}

impl NoteState {
    fn save_now(&mut self, hwnd: HWND) {
        unsafe {
            let _ = KillTimer(Some(hwnd), TIMER_AUTOSAVE);
        }
        if !self.dirty {
            return;
        }
        self.dirty = false;
        let body = forms::get_lines(self.edit);
        update_note_title(hwnd, &body);
        let _ = self.tx.send(Req::NoteSave { token: self.token, id: self.id, body, editor: self.self_handle });
    }
}

fn update_note_title(hwnd: HWND, body: &str) {
    let first = orbit_data::title_from_body(body);
    let title = if first.is_empty() { "New note".to_string() } else { first };
    let text = wide(&format!("{title} - Orbit"));
    unsafe {
        let _ = SetWindowTextW(hwnd, PCWSTR(text.as_ptr()));
    }
}

fn note_state(hwnd: HWND) -> Option<&'static mut NoteState> {
    unsafe {
        let pointer = GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *mut NoteState;
        pointer.as_mut()
    }
}

/// Esc closes, Ctrl+S saves. Everything else belongs to the edit control.
unsafe extern "system" fn note_edit_proc(hwnd: HWND, message: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    unsafe {
        let parent = GetParent(hwnd).unwrap_or_default();
        let Some(state) = note_state(parent) else {
            return DefWindowProcW(hwnd, message, wparam, lparam);
        };
        if message == WM_KEYDOWN {
            let ctrl = GetKeyState(VK_CONTROL.0 as i32) < 0;
            if wparam.0 == VK_ESCAPE.0 as usize {
                let _ = PostMessageW(Some(parent), WM_CLOSE, WPARAM(0), LPARAM(0));
                return LRESULT(0);
            }
            if ctrl && wparam.0 == 0x53 {
                state.save_now(parent);
                return LRESULT(0);
            }
            if ctrl && wparam.0 == 0x57 {
                let _ = PostMessageW(Some(parent), WM_CLOSE, WPARAM(0), LPARAM(0));
                return LRESULT(0);
            }
        }
        // Ctrl+S and Ctrl+W must not also insert control characters.
        if message == WM_CHAR && matches!(wparam.0, 0x13 | 0x17 | 0x1B) {
            return LRESULT(0);
        }
        let original: WNDPROC = std::mem::transmute(state.original_edit_proc);
        CallWindowProcW(original, hwnd, message, wparam, lparam)
    }
}

/// Keeps text off the window's edges. The control resets this whenever it's resized.
fn set_text_margins(edit: HWND, width: i32, height: i32) {
    let dpi = unsafe { GetDpiForWindow(edit) };
    let margin = forms::scale(16, dpi);
    let scroll_bar = forms::scale(18, dpi);
    let area = RECT {
        left: margin,
        top: forms::scale(12, dpi),
        right: (width - margin - scroll_bar).max(margin + 1),
        bottom: (height - forms::scale(8, dpi)).max(1),
    };
    unsafe {
        SendMessageW(edit, EM_SETRECT, Some(WPARAM(0)), Some(LPARAM(&area as *const RECT as isize)));
    }
}

unsafe extern "system" fn note_proc(hwnd: HWND, message: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    unsafe {
        match message {
            WM_SIZE => {
                if let Some(state) = note_state(hwnd) {
                    let (width, height) = ((lparam.0 & 0xFFFF) as i32, ((lparam.0 >> 16) & 0xFFFF) as i32);
                    let _ = SetWindowPos(state.edit, None, 0, 0, width, height, SWP_NOZORDER | SWP_NOACTIVATE);
                    set_text_margins(state.edit, width, height);
                }
                LRESULT(0)
            }
            WM_SETFOCUS => {
                if let Some(state) = note_state(hwnd) {
                    let _ = SetFocus(Some(state.edit));
                }
                LRESULT(0)
            }
            WM_COMMAND => {
                if (wparam.0 >> 16) as u32 == EN_CHANGE {
                    if let Some(state) = note_state(hwnd) {
                        state.dirty = true;
                        let _ = SetTimer(Some(hwnd), TIMER_AUTOSAVE, AUTOSAVE_MS, None);
                    }
                }
                LRESULT(0)
            }
            WM_TIMER => {
                if wparam.0 == TIMER_AUTOSAVE {
                    if let Some(state) = note_state(hwnd) {
                        state.save_now(hwnd);
                    }
                }
                LRESULT(0)
            }
            WM_EDITOR_SAVED => {
                if let Some(state) = note_state(hwnd) {
                    state.id = Some(lparam.0 as i64);
                }
                LRESULT(0)
            }
            WM_CLOSE => {
                if let Some(state) = note_state(hwnd) {
                    state.save_now(hwnd);
                    if let Some(id) = state.id {
                        // A note emptied by the user goes to the trash rather than staying blank.
                        let _ = state.tx.send(Req::NoteDeleteIfEmpty { id });
                    }
                }
                let _ = DestroyWindow(hwnd);
                LRESULT(0)
            }
            WM_DESTROY => {
                untrack(hwnd);
                let pointer = SetWindowLongPtrW(hwnd, GWLP_USERDATA, 0) as *mut NoteState;
                if !pointer.is_null() {
                    let state = Box::from_raw(pointer);
                    let _ = DeleteObject(state.font.into());
                    if let Some(brush) = state.background {
                        let _ = DeleteObject(brush.into());
                    }
                    if open_note_count() == 0 {
                        state.main.post(crate::app::WM_ORBIT_RECLAIM, 1, 0);
                    }
                }
                LRESULT(0)
            }
            _ => DefWindowProcW(hwnd, message, wparam, lparam),
        }
    }
}

fn register_class(name: PCWSTR, proc: WNDPROC, background: HBRUSH) {
    unsafe {
        let Ok(module) = GetModuleHandleW(None) else { return };
        let class = WNDCLASSEXW {
            cbSize: std::mem::size_of::<WNDCLASSEXW>() as u32,
            lpfnWndProc: proc,
            hInstance: module.into(),
            hCursor: LoadCursorW(None, IDC_ARROW).unwrap_or_default(),
            hbrBackground: background,
            lpszClassName: name,
            hIcon: tray::load_app_icon(32, 32),
            ..Default::default()
        };
        // Already registered is fine: the class lasts as long as the process.
        RegisterClassExW(&class);
    }
}

pub fn open_note(context: EditorContext, id: Option<i64>, body: &str) {
    unsafe {
        // Windows includes the DLL only for people who actually open a note.
        let has_rich_edit = GetModuleHandleW(w!("msftedit.dll")).is_ok() || LoadLibraryW(w!("msftedit.dll")).is_ok();
        register_class(w!("OrbitNote"), Some(note_proc), HBRUSH((COLOR_WINDOW.0 + 1) as isize as *mut _));
        let dpi = GetDpiForSystem();
        let hwnd = match CreateWindowExW(
            WINDOW_EX_STYLE(0),
            w!("OrbitNote"),
            w!("New note - Orbit"),
            WS_OVERLAPPEDWINDOW,
            CW_USEDEFAULT,
            CW_USEDEFAULT,
            forms::scale(620, dpi),
            forms::scale(640, dpi),
            None,
            None,
            None,
            None,
        ) {
            Ok(hwnd) => hwnd,
            Err(_) => return,
        };
        apply_title_bar_theme(hwnd, &context.theme);
        let class = if has_rich_edit { w!("RICHEDIT50W") } else { w!("EDIT") };
        let style = WS_CHILD | WS_VISIBLE | WS_VSCROLL
            | WINDOW_STYLE((ES_MULTILINE | ES_AUTOVSCROLL | ES_WANTRETURN | ES_NOHIDESEL) as u32);
        let edit = CreateWindowExW(WINDOW_EX_STYLE(0), class, w!(""), style, 0, 0, 100, 100, Some(hwnd), None, None, None)
            .unwrap_or_default();
        let font = crate::gfx::create_font(crate::gfx::ui_face(), -forms::scale(15, dpi), 400);
        SendMessageW(edit, WM_SETFONT, Some(WPARAM(font.0 as usize)), Some(LPARAM(1)));
        if has_rich_edit {
            SendMessageW(
                edit,
                EM_SETTEXTMODE,
                Some(WPARAM((TM_PLAINTEXT.0 | TM_MULTILEVELUNDO.0) as usize)),
                None,
            );
            // No practical limit on length. The default is only 32k characters.
            SendMessageW(edit, EM_EXLIMITTEXT, None, Some(LPARAM(isize::MAX)));
            SendMessageW(edit, EM_SETBKGNDCOLOR, Some(WPARAM(0)), Some(LPARAM(context.theme.background.colorref().0 as isize)));
            let format = CHARFORMATW {
                cbSize: std::mem::size_of::<CHARFORMATW>() as u32,
                dwMask: CFM_COLOR,
                crTextColor: context.theme.text.colorref(),
                ..Default::default()
            };
            SendMessageW(edit, EM_SETCHARFORMAT, Some(WPARAM(SCF_ALL as usize)), Some(LPARAM(&format as *const _ as isize)));
        } else {
            SendMessageW(edit, EM_SETLIMITTEXT, Some(WPARAM(0)), None);
        }
        forms::set_text_only(edit, body);
        // The rich edit control shows and hides its own scroll bar.
        if context.theme.background.0 < 128 {
            let _ = SetWindowTheme(edit, w!("DarkMode_Explorer"), PCWSTR::null());
        }
        if has_rich_edit {
            // Set after the initial text, so loading a note doesn't count as editing it.
            SendMessageW(edit, EM_SETEVENTMASK, None, Some(LPARAM(ENM_CHANGE as isize)));
        }
        let original = SetWindowLongPtrW(edit, GWLP_WNDPROC, note_edit_proc as *const () as isize);
        let state = Box::new(NoteState {
            tx: context.tx,
            token: NEXT_TOKEN.fetch_add(1, Ordering::Relaxed),
            id,
            edit,
            font,
            background: None,
            // A seeded new note counts as unsaved. An existing note is unchanged until edited.
            dirty: id.is_none() && !body.is_empty(),
            original_edit_proc: original,
            self_handle: SendHwnd::new(hwnd),
            main: context.main,
        });
        SetWindowLongPtrW(hwnd, GWLP_USERDATA, Box::into_raw(state) as isize);
        update_note_title(hwnd, body);
        let mut rect = RECT::default();
        let _ = GetClientRect(hwnd, &mut rect);
        let _ = SetWindowPos(edit, None, 0, 0, rect.right, rect.bottom, SWP_NOZORDER | SWP_NOACTIVATE);
        set_text_margins(edit, rect.right, rect.bottom);
        track(hwnd);
        // A fresh note starts at the end, ready to type. An existing one starts at the top.
        if id.is_none() {
            let end = forms::get_text(edit).encode_utf16().count();
            SendMessageW(edit, EM_SETSEL, Some(WPARAM(end)), Some(LPARAM(end as isize)));
        }
        bring_forward(hwnd, edit);
    }
}

// --- Snippets -----------------------------------------------------------------------------

struct SnippetState {
    tx: Sender<Req>,
    id: Option<i64>,
    name: HWND,
    trigger: HWND,
    content: HWND,
    description: HWND,
    category: HWND,
    favourite: HWND,
    enabled: HWND,
    status: HWND,
    style: Box<forms::Style>,
    self_handle: SendHwnd,
}

fn snippet_state(hwnd: HWND) -> Option<&'static mut SnippetState> {
    unsafe { (GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *mut SnippetState).as_mut() }
}

impl SnippetState {
    fn submit(&mut self) {
        let name = forms::get_text(self.name).trim().to_string();
        let trigger = forms::get_text(self.trigger).trim().to_string();
        let content = forms::get_lines(self.content);
        if name.is_empty() {
            forms::set_status(self.status, "Give the snippet a name.");
            forms::focus(self.name);
            return;
        }
        if content.is_empty() {
            forms::set_status(self.status, "Add the text this snippet inserts.");
            forms::focus(self.content);
            return;
        }
        if !trigger.is_empty() {
            if let Err(problem) = validate_trigger(&trigger) {
                let message = match problem {
                    TriggerProblem::Empty => "The keyword is empty.",
                    TriggerProblem::TooShort => "Use at least two characters for a keyword, so ordinary typing won't set it off.",
                    TriggerProblem::TooLong => "That keyword is too long.",
                    TriggerProblem::ContainsWhitespace => "Keywords can't contain spaces.",
                };
                forms::set_status(self.status, message);
                forms::focus(self.trigger);
                return;
            }
        }
        let input = SnippetInput {
            name,
            trigger,
            content,
            description: forms::get_text(self.description).trim().to_string(),
            category: forms::get_text(self.category).trim().to_string(),
            favourite: forms::is_checked(self.favourite),
            enabled: forms::is_checked(self.enabled),
        };
        let _ = self.tx.send(Req::SnippetSave { id: self.id, input, editor: self.self_handle });
    }
}

unsafe extern "system" fn snippet_proc(hwnd: HWND, message: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    unsafe {
        if let Some(state) = snippet_state(hwnd) {
            if message == WM_PAINT {
                state.style.paint(hwnd, |_| {});
                return LRESULT(0);
            }
            if let Some(result) = state.style.handle(hwnd, message, wparam, lparam) {
                return result;
            }
        }
        match message {
            WM_COMMAND => {
                let id = (wparam.0 & 0xFFFF) as i32;
                match id {
                    x if x == ID_SAVE || x == IDOK.0 => {
                        if let Some(state) = snippet_state(hwnd) {
                            state.submit();
                        }
                    }
                    x if x == ID_CANCEL || x == IDCANCEL.0 => {
                        let _ = DestroyWindow(hwnd);
                    }
                    x if (ID_CHIPS..ID_CHIPS + PLACEHOLDERS.len() as i32).contains(&x) => {
                        if let Some(state) = snippet_state(hwnd) {
                            forms::replace_selection(state.content, PLACEHOLDERS[(x - ID_CHIPS) as usize].1);
                            forms::focus(state.content);
                        }
                    }
                    _ => {}
                }
                LRESULT(0)
            }
            WM_EDITOR_SAVED => {
                let _ = DestroyWindow(hwnd);
                LRESULT(0)
            }
            WM_CLOSE => {
                let _ = DestroyWindow(hwnd);
                LRESULT(0)
            }
            WM_DESTROY => {
                untrack(hwnd);
                unregister_form(hwnd);
                let pointer = SetWindowLongPtrW(hwnd, GWLP_USERDATA, 0) as *mut SnippetState;
                if !pointer.is_null() {
                    drop(Box::from_raw(pointer));
                }
                LRESULT(0)
            }
            _ => DefWindowProcW(hwnd, message, wparam, lparam),
        }
    }
}

pub fn open_snippet(context: EditorContext, snippet: Option<Snippet>) {
    unsafe {
        register_class(w!("OrbitSnippet"), Some(snippet_proc), HBRUSH::default());
        let dpi = GetDpiForSystem();
        let title = if snippet.is_some() { w!("Edit snippet - Orbit") } else { w!("New snippet - Orbit") };
        let (origin_x, origin_y) = forms::form_origin();
        // Sized to what's in it, once that's laid out.
        let Ok(hwnd) = CreateWindowExW(
            forms::form_ex_style(),
            w!("OrbitSnippet"),
            title,
            WS_OVERLAPPED | WS_CAPTION | WS_SYSMENU | WS_MINIMIZEBOX | WS_CLIPCHILDREN,
            origin_x,
            origin_y,
            forms::scale(SNIPPET_WIDTH_DIP, dpi),
            forms::scale(640, dpi),
            None,
            None,
            None,
            None,
        ) else {
            return;
        };
        let mut style = Box::new(forms::Style::new(context.theme, dpi));
        style.apply_to_window(hwnd);
        let px = |value: i32| forms::scale(value, dpi);
        let (window_width, side) = (px(SNIPPET_WIDTH_DIP), px(SNIPPET_SIDE_DIP));
        let mut b = Builder::new(hwnd, &mut style, side, px(20), window_width - side * 2);
        let source = snippet.as_ref();
        b.title(if source.is_some() { "Edit snippet" } else { "New snippet" });
        b.begin_card();
        b.caption("Name");
        let name = b.edit(source.map_or("", |s| &s.name), ID_NAME);
        b.y -= b.px(4);
        b.caption("Keyword");
        let trigger = b.edit(source.map_or("", |s| &s.trigger), ID_TRIGGER);
        b.y -= b.px(8);
        b.note("Optional. Type it anywhere to put the text there, if expansion is on in Settings.");
        b.end_card();
        b.begin_card();
        b.caption("Text");
        let content = b.text_area(source.map_or("", |s| &s.content), 7, ID_CONTENT);
        b.y -= b.px(6);
        let chips: Vec<(&str, i32)> = PLACEHOLDERS.iter().enumerate().map(|(i, chip)| (chip.0, ID_CHIPS + i as i32)).collect();
        b.chips("Insert", &chips);
        b.end_card();
        b.begin_card();
        let description = b.field("Description", source.map_or("", |s| &s.description), 0, 340);
        let category = b.field("Category", source.map_or("", |s| &s.category), 0, 340);
        b.line();
        let favourite = b.switch("Favourite: list it first", source.is_some_and(|s| s.favourite), 0);
        b.line();
        let enabled = b.switch("Enabled: its keyword expands", source.is_none_or(|s| s.enabled), 0);
        b.end_card();
        let row = b.y + b.px(2);
        let (button, gap) = (b.px(100), b.px(8));
        let right = b.x + b.width;
        let status = b.status_at(b.x, row, b.width - button * 2 - gap - b.px(16), b.px(34));
        let _ = ID_STATUS;
        b.button_at("Cancel", ID_CANCEL, right - button * 2 - gap, row, button, forms::ButtonKind::Normal);
        b.button_at("Save", ID_SAVE, right - button, row, button, forms::ButtonKind::Primary);
        forms::fit_client(hwnd, window_width, row + b.px(30) + b.px(20));
        let state = Box::new(SnippetState {
            tx: context.tx,
            id: source.map(|s| s.id),
            name,
            trigger,
            content,
            description,
            category,
            favourite,
            enabled,
            status,
            style,
            self_handle: SendHwnd::new(hwnd),
        });
        SetWindowLongPtrW(hwnd, GWLP_USERDATA, Box::into_raw(state) as isize);
        register_form(hwnd);
        track(hwnd);
        bring_forward(hwnd, name);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use orbit_core::snippet::{TemplateContext, expand_template};

    #[test]
    fn every_chip_inserts_something_snippets_understand() {
        let context = TemplateContext { date: "D", time: "T", clipboard: Some("C") };
        for (label, placeholder) in PLACEHOLDERS {
            let expanded = expand_template(placeholder, &context);
            assert!(!expanded.text.contains('{'), "{label} stayed as typed");
        }
    }
}
