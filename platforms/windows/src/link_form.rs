//! The forms for adding or changing a quicklink or a command.
//!
//! Each kind has at most one window open. Save checks what was typed with `orbit_core::links`,
//! shows any problem right there, and hands a good result to the launcher in a posted message.
//! The launcher alone changes `Settings`, as it does for everything else.

use crate::app::WM_ORBIT_LINK_SAVED;
use crate::editor::{EditorContext, register_form, unregister_form};
use crate::forms::{self, Builder, ButtonKind};
use crate::util::SendHwnd;
use orbit_core::links::{Field, Problem, check_command, check_link};
use orbit_core::settings::{CustomCommand, QuickLink};
use orbit_core::Settings;
use std::cell::Cell;
use windows::Win32::Foundation::*;
use windows::Win32::Graphics::Gdi::{InvalidateRect, HBRUSH};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::Controls::Dialogs::{GetOpenFileNameW, OFN_FILEMUSTEXIST, OFN_PATHMUSTEXIST, OPENFILENAMEW};
use windows::Win32::UI::HiDpi::GetDpiForSystem;
use windows::Win32::UI::WindowsAndMessaging::*;
use windows::core::{PCWSTR, PWSTR, w};

const WIDTH_DIP: i32 = 520;
const SIDE_DIP: i32 = 24;

const ID_NAME: i32 = 10;
const ID_TARGET: i32 = 11;
const ID_ALIAS: i32 = 12;
const ID_ARGUMENTS: i32 = 13;
const ID_FOLDER: i32 = 14;
const ID_BROWSE: i32 = 15;
const ID_SAVE: i32 = 16;
const ID_CANCEL: i32 = 17;

/// A finished form's result, for the launcher to keep.
pub enum LinkSaved {
    Link(QuickLink),
    Command(CustomCommand),
}

/// What Settings asks the launcher to do. Ids rather than copies, so it acts on the latest.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum LinkRequest {
    NewLink,
    EditLink(String),
    DeleteLink(String),
    NewCommand,
    NewSnippet,
    EditCommand(String),
    DeleteCommand(String),
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Kind {
    Link,
    Command,
}

thread_local! {
    static OPEN_LINK: Cell<Option<HWND>> = const { Cell::new(None) };
    static OPEN_COMMAND: Cell<Option<HWND>> = const { Cell::new(None) };
}

fn current(kind: Kind) -> Option<HWND> {
    match kind {
        Kind::Link => OPEN_LINK.with(|c| c.get()),
        Kind::Command => OPEN_COMMAND.with(|c| c.get()),
    }
}

fn set_current(kind: Kind, hwnd: Option<HWND>) {
    match kind {
        Kind::Link => OPEN_LINK.with(|c| c.set(hwnd)),
        Kind::Command => OPEN_COMMAND.with(|c| c.set(hwnd)),
    }
}

struct State {
    kind: Kind,
    /// Empty for a new item, which gets its id when it's saved.
    id: String,
    main: SendHwnd,
    links: Vec<QuickLink>,
    commands: Vec<CustomCommand>,
    name: HWND,
    target: HWND,
    alias: HWND,
    arguments: HWND,
    folder: HWND,
    /// How the link will behave, updated while typing. Links only.
    behaviour: HWND,
    status: HWND,
    style: Box<forms::Style>,
}

fn state(hwnd: HWND) -> Option<&'static mut State> {
    unsafe { (GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *mut State).as_mut() }
}

/// A sentence saying what this link will do, for showing while it's typed.
pub fn describe_link(url: &str, alias: &str) -> String {
    let url = url.trim();
    let alias = alias.trim();
    if url.is_empty() {
        return "Add a link above to see how it will work.".to_string();
    }
    // Only the address matters here. The name is a stand-in, and the alias is checked when saving,
    // so nothing is compared with the other items yet.
    let draft = QuickLink { id: String::new(), name: "x".to_string(), url: url.to_string(), alias: "x".to_string(), open_with: String::new() };
    let address = match check_link(draft, &[], &[]) {
        Ok(link) => link.url,
        Err((Field::Target, message)) => return message,
        Err(_) => url.to_string(),
    };
    let params = orbit_core::links::link_params(&address);
    if params.len() == 1 && params[0].name.eq_ignore_ascii_case("query") && params[0].required() {
        let example = address.replace("{query}", "your search");
        return if alias.is_empty() {
            format!("Choosing it asks for your search, then opens {example}")
        } else {
            format!("Type {alias}, then your search, to open {example}")
        };
    }
    if !params.is_empty() {
        let names: Vec<String> = params
            .iter()
            .map(|param| match &param.default {
                Some(default) if !default.trim().is_empty() => format!("{} ({default} if left empty)", param.label),
                Some(_) => format!("{} (optional)", param.label),
                None => param.label.clone(),
            })
            .collect();
        let list = match names.split_last() {
            Some((last, [])) => last.clone(),
            Some((last, rest)) => format!("{} and {last}", rest.join(", ")),
            None => String::new(),
        };
        let asks = format!("Choosing it asks for {list}, then opens the link.");
        return if alias.is_empty() { asks } else { format!("{asks} Or type {alias} and the values in that order.") };
    }
    if alias.is_empty() { format!("Opens {address}") } else { format!("Opens {address}. Type {alias} in Orbit to find it.") }
}

impl State {
    fn update_behaviour(&self) {
        if self.kind != Kind::Link {
            return;
        }
        let text = describe_link(&forms::get_text(self.target), &forms::get_text(self.alias));
        forms::set_status(self.behaviour, &text);
    }

    fn show_problem(&self, (field, message): Problem) {
        forms::set_status(self.status, &message);
        let control = match field {
            Field::Name => self.name,
            Field::Target => self.target,
            Field::Alias => self.alias,
        };
        unsafe {
            // Ready to type over.
            SendMessageW(control, 0xB1, Some(WPARAM(0)), Some(LPARAM(-1)));
        }
        forms::focus(control);
    }

    /// Checks the form and sends off the result. True if the form is finished.
    fn submit(&self) -> bool {
        let result = match self.kind {
            Kind::Link => {
                let link = QuickLink { id: self.id.clone(), name: forms::get_text(self.name), url: forms::get_text(self.target), alias: forms::get_text(self.alias), open_with: String::new() };
                check_link(link, &self.links, &self.commands).map(LinkSaved::Link)
            }
            Kind::Command => {
                let command = CustomCommand {
                    id: self.id.clone(),
                    name: forms::get_text(self.name),
                    program: forms::get_text(self.target),
                    arguments: forms::get_text(self.arguments),
                    working_dir: forms::get_text(self.folder),
                    alias: forms::get_text(self.alias),
                };
                check_command(command, &self.links, &self.commands).map(LinkSaved::Command)
            }
        };
        match result {
            Ok(saved) => {
                self.main.post(WM_ORBIT_LINK_SAVED, crate::util::box_for_post(saved), 0);
                true
            }
            Err(problem) => {
                self.show_problem(problem);
                false
            }
        }
    }

    fn browse(&self, hwnd: HWND) {
        if let Some(path) = pick_program(hwnd) {
            forms::set_text(self.target, &path);
        }
    }
}

fn pick_program(owner: HWND) -> Option<String> {
    let mut buffer = vec![0u16; 1024];
    let filter = crate::util::wide("Programs (*.exe, *.cmd, *.bat)\0*.exe;*.cmd;*.bat\0All files\0*.*\0");
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
        String::from_utf16_lossy(&buffer[..end])
    })
}

unsafe extern "system" fn form_proc(hwnd: HWND, message: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    unsafe {
        if let Some(s) = state(hwnd) {
            if message == WM_PAINT {
                s.style.paint(hwnd, |_| {});
                return LRESULT(0);
            }
            if let Some(result) = s.style.handle(hwnd, message, wparam, lparam) {
                return result;
            }
        }
        match message {
            WM_COMMAND => {
                let id = (wparam.0 & 0xFFFF) as i32;
                let notification = (wparam.0 >> 16) as u32;
                if let Some(s) = state(hwnd) {
                    match id {
                        x if x == ID_SAVE || x == IDOK.0 => {
                            if s.submit() {
                                let _ = DestroyWindow(hwnd);
                            }
                        }
                        x if x == ID_CANCEL || x == IDCANCEL.0 => {
                            let _ = DestroyWindow(hwnd);
                        }
                        ID_BROWSE => s.browse(hwnd),
                        ID_TARGET | ID_ALIAS if notification == EN_CHANGE => s.update_behaviour(),
                        _ => {}
                    }
                }
                LRESULT(0)
            }
            WM_CLOSE => {
                let _ = DestroyWindow(hwnd);
                LRESULT(0)
            }
            WM_DESTROY => {
                unregister_form(hwnd);
                let pointer = SetWindowLongPtrW(hwnd, GWLP_USERDATA, 0) as *mut State;
                if !pointer.is_null() {
                    let owned = Box::from_raw(pointer);
                    set_current(owned.kind, None);
                }
                LRESULT(0)
            }
            _ => DefWindowProcW(hwnd, message, wparam, lparam),
        }
    }
}

/// Starts a window for a form of this kind, sized later from what ends up in it.
fn create_window(kind: Kind, title: PCWSTR) -> Option<(HWND, u32)> {
    unsafe {
        let module = GetModuleHandleW(None).ok()?;
        let class = WNDCLASSEXW {
            cbSize: std::mem::size_of::<WNDCLASSEXW>() as u32,
            lpfnWndProc: Some(form_proc),
            hInstance: module.into(),
            hCursor: LoadCursorW(None, IDC_ARROW).unwrap_or_default(),
            hbrBackground: HBRUSH::default(),
            lpszClassName: w!("OrbitLinkForm"),
            hIcon: crate::tray::load_app_icon(32, 32),
            ..Default::default()
        };
        // Already registered is fine: the class lasts as long as the process.
        RegisterClassExW(&class);
        let dpi = GetDpiForSystem();
        let (x, y) = forms::form_origin();
        let hwnd = CreateWindowExW(
            forms::form_ex_style(),
            w!("OrbitLinkForm"),
            title,
            WS_OVERLAPPED | WS_CAPTION | WS_SYSMENU | WS_MINIMIZEBOX | WS_CLIPCHILDREN,
            x,
            y,
            forms::scale(WIDTH_DIP, dpi),
            forms::scale(400, dpi),
            None,
            None,
            None,
            None,
        )
        .ok()?;
        set_current(kind, Some(hwnd));
        Some((hwnd, dpi))
    }
}

/// Brings an open form of this kind to the front. True if there was one.
fn focus_existing(kind: Kind) -> bool {
    let Some(hwnd) = current(kind) else { return false };
    if let Some(s) = state(hwnd) {
        forms::show_form(hwnd, s.name);
    }
    true
}

/// The labelled box: its name above, the box, and a short hint below.
fn labelled(b: &mut Builder, label: &str, text: &str, id: i32, hint: &str) -> HWND {
    b.caption(label);
    let edit = b.edit(text, id);
    b.y -= b.px(8);
    b.note(hint);
    edit
}

/// The bottom row, then fits the window around everything above it.
fn finish(b: &mut Builder, hwnd: HWND, width: i32) -> HWND {
    let row = b.y + b.px(2);
    let (button, gap) = (b.px(100), b.px(8));
    let right = b.x + b.width;
    let status = b.status_at(b.x, row, b.width - button * 2 - gap - b.px(16), b.px(34));
    b.button_at("Cancel", ID_CANCEL, right - button * 2 - gap, row, button, ButtonKind::Normal);
    b.button_at("Save", ID_SAVE, right - button, row, button, ButtonKind::Primary);
    forms::fit_client(hwnd, width, row + b.px(30) + b.px(20));
    status
}

/// Opens the form for a quicklink, or brings it forward if it's already open.
pub fn open_link(context: EditorContext, existing: Option<QuickLink>, links: Vec<QuickLink>, commands: Vec<CustomCommand>) {
    if focus_existing(Kind::Link) {
        return;
    }
    // A link with no id yet is a new one that already has its address filled in.
    let editing = existing.as_ref().is_some_and(|link| !link.id.is_empty());
    let title = if editing { w!("Edit quicklink - Orbit") } else { w!("New quicklink - Orbit") };
    let Some((hwnd, dpi)) = create_window(Kind::Link, title) else { return };
    let px = |value: i32| forms::scale(value, dpi);
    let mut style = Box::new(forms::Style::new(context.theme, dpi));
    style.apply_to_window(hwnd);
    let (width, side) = (px(WIDTH_DIP), px(SIDE_DIP));
    let mut b = Builder::new(hwnd, &mut style, side, px(20), width - side * 2);
    let source = existing.as_ref();
    b.title(if editing { "Edit quicklink" } else { "New quicklink" });
    b.begin_card();
    let name = labelled(&mut b, "Name", source.map_or("", |l| &l.name), ID_NAME, "What it's called in Orbit.");
    let target = labelled(&mut b, "Link", source.map_or("", |l| &l.url), ID_TARGET, "Put {query} in the link for a search, or {name} for any box to fill in. {name=text} gives it a default.");
    b.hint(target, "https://example.com");
    let alias = labelled(&mut b, "Alias", source.map_or("", |l| &l.alias), ID_ALIAS, "Optional. A short word to find it with, such as gh.");
    b.line();
    let behaviour = b.note_area("", 2);
    b.end_card();
    let status = finish(&mut b, hwnd, width);
    let state = Box::new(State {
        kind: Kind::Link,
        id: source.map_or(String::new(), |l| l.id.clone()),
        main: context.main,
        links,
        commands,
        name,
        target,
        alias,
        arguments: HWND::default(),
        folder: HWND::default(),
        behaviour,
        status,
        style,
    });
    state.update_behaviour();
    unsafe {
        SetWindowLongPtrW(hwnd, GWLP_USERDATA, Box::into_raw(state) as isize);
    }
    register_form(hwnd);
    forms::show_form(hwnd, name);
}

/// Opens the form for a command, or brings it forward if it's already open.
pub fn open_command(context: EditorContext, existing: Option<CustomCommand>, links: Vec<QuickLink>, commands: Vec<CustomCommand>) {
    if focus_existing(Kind::Command) {
        return;
    }
    let title = if existing.is_some() { w!("Edit command - Orbit") } else { w!("New command - Orbit") };
    let Some((hwnd, dpi)) = create_window(Kind::Command, title) else { return };
    let px = |value: i32| forms::scale(value, dpi);
    let mut style = Box::new(forms::Style::new(context.theme, dpi));
    style.apply_to_window(hwnd);
    let (width, side) = (px(WIDTH_DIP), px(SIDE_DIP));
    let mut b = Builder::new(hwnd, &mut style, side, px(20), width - side * 2);
    let source = existing.as_ref();
    b.title(if existing.is_some() { "Edit command" } else { "New command" });
    b.begin_card();
    let name = labelled(&mut b, "Name", source.map_or("", |c| &c.name), ID_NAME, "What it's called in Orbit.");
    let alias = labelled(&mut b, "Alias", source.map_or("", |c| &c.alias), ID_ALIAS, "Optional. A short word to find it with.");
    b.caption("Program");
    let (target, _) = b.edit_with_button(source.map_or("", |c| &c.program), ID_TARGET, "Browse\u{2026}", ID_BROWSE);
    b.note("The program to run. It runs only when you choose it, without a shell.");
    let arguments = labelled(&mut b, "Arguments", source.map_or("", |c| &c.arguments), ID_ARGUMENTS, "Optional. What to pass to it, as you'd type it after the program's name.");
    let folder = labelled(&mut b, "Folder", source.map_or("", |c| &c.working_dir), ID_FOLDER, "Optional. Where it runs. Leave it empty for its usual place.");
    b.end_card();
    let status = finish(&mut b, hwnd, width);
    let state = Box::new(State {
        kind: Kind::Command,
        id: source.map_or(String::new(), |c| c.id.clone()),
        main: context.main,
        links,
        commands,
        name,
        target,
        alias,
        arguments,
        folder,
        behaviour: HWND::default(),
        status,
        style,
    });
    unsafe {
        SetWindowLongPtrW(hwnd, GWLP_USERDATA, Box::into_raw(state) as isize);
    }
    register_form(hwnd);
    forms::show_form(hwnd, name);
}

/// The launcher's lists changed, so an open form checks aliases against the new ones.
pub fn lists_changed(settings: &Settings) {
    for kind in [Kind::Link, Kind::Command] {
        if let Some(s) = current(kind).and_then(state) {
            s.links = settings.links.clone();
            s.commands = settings.custom_commands.clone();
        }
    }
}

/// For the developer preview: presses Save on the open form, to show how a problem looks.
pub fn preview_submit() {
    for kind in [Kind::Link, Kind::Command] {
        if let Some(hwnd) = current(kind) {
            if let Some(s) = state(hwnd) {
                let _ = s.submit();
                unsafe {
                    let _ = InvalidateRect(Some(hwnd), None, false);
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn describe(url: &str, alias: &str) -> String {
        describe_link(url, alias)
    }

    #[test]
    fn the_note_says_what_a_plain_link_does() {
        assert_eq!(describe("", ""), "Add a link above to see how it will work.");
        assert_eq!(describe("https://github.com", ""), "Opens https://github.com");
        assert_eq!(describe("github.com", ""), "Opens https://github.com", "the same https:// that saving adds");
        assert_eq!(describe("github.com", "gh"), "Opens https://github.com. Type gh in Orbit to find it.");
    }

    #[test]
    fn the_note_says_how_to_use_a_search() {
        assert_eq!(describe("x.com/s?q={query}", ""), "Choosing it asks for your search, then opens https://x.com/s?q=your search");
        assert_eq!(describe("x.com/s?q={query}", "x"), "Type x, then your search, to open https://x.com/s?q=your search");
    }

    #[test]
    fn the_note_lists_every_box_a_link_asks_for() {
        assert_eq!(
            describe("x.com/{city=London}/{days}?u={units=}", ""),
            "Choosing it asks for city (London if left empty), days and units (optional), then opens the link."
        );
        assert!(describe("x.com/{repo}", "gh").ends_with("Or type gh and the values in that order."));
    }

    #[test]
    fn the_note_warns_about_a_link_that_cannot_work() {
        assert!(describe("javascript:alert(1)", "").contains("won't open"));
        assert!(describe("not a link", "").contains("isn't an address"));
    }

    #[test]
    fn the_note_shows_the_address_with_https_added() {
        assert_eq!(describe_link("b.example", "x"), "Opens https://b.example. Type x in Orbit to find it.");
    }
}
