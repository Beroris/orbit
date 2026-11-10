//! Orbit Setup: one window that installs Orbit, welcomes someone new, brings things over from
//! other launchers, and removes Orbit again.
//!
//! `Orbit-<version>-Setup.exe` is orbit.exe under another name. It shows Welcome and Install,
//! then starts the installed Orbit, which opens this window again where it left off, at Import.
//! So everything after installing goes through the Orbit that owns the settings, and nothing is
//! written behind its back. A portable Orbit's first start opens it at Welcome, Settings → Data
//! opens Import on its own, and `orbit.exe --uninstall` opens Uninstall.
//!
//! The window is Settings' look: the rail on the left with the steps, the page on the right, and
//! the buttons at the foot.

use crate::forms::{self, Builder, ButtonKind, Style};
use crate::gfx::{Backbuffer, Rgb};
use crate::importer::{self, Chosen, Found, Launcher};
use crate::install;
use crate::integrations::{self, EnteAccount, Tool, ToolDone};
use crate::ui::Theme;
use crate::util::SendHwnd;
use orbit_core::import::{Haul, ReadError};
use orbit_hotkeys::Chord;
use std::cell::Cell;
use std::path::PathBuf;
use std::time::{Duration, Instant};
use windows::Win32::Foundation::*;
use windows::Win32::Graphics::Gdi::{GetMonitorInfoW, InvalidateRect, MONITOR_DEFAULTTONEAREST, MONITORINFO, MonitorFromPoint};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::System::Threading::GetCurrentProcessId;
use windows::Win32::UI::Controls::Dialogs::{GetOpenFileNameW, OFN_FILEMUSTEXIST, OFN_PATHMUSTEXIST, OPENFILENAMEW};
use windows::Win32::UI::HiDpi::GetDpiForSystem;
use windows::Win32::UI::Input::KeyboardAndMouse::{EnableWindow, GetFocus, IsWindowEnabled, RegisterHotKey, UnregisterHotKey};
use windows::Win32::UI::WindowsAndMessaging::*;
use windows::core::{PCWSTR, PWSTR, w};

const CLASS: PCWSTR = w!("OrbitSetup");
const WIDTH_DIP: i32 = 820;
const MIN_HEIGHT_DIP: i32 = 580;
const RAIL_DIP: i32 = 236;
const SIDE_DIP: i32 = 44;
const TOP_DIP: i32 = 40;
const FOOTER_DIP: i32 = 70;
const LOGO_DIP: i32 = 64;
const BUTTON_DIP: i32 = 128;

const ID_BACK: i32 = 10;
const ID_NEXT: i32 = 11;
const ID_SIGN_IN: i32 = 12;
const ID_DESKTOP: i32 = 13;
const ID_HOTKEY: i32 = 14;
const ID_FILES_ON: i32 = 15;
const ID_EVERYTHING: i32 = 16;
const ID_REMOVE_DATA: i32 = 17;
const ID_PASS: i32 = 18;
const ID_PASS_OPEN: i32 = 19;
/// The Extensions page's 1Password and Ente Auth rows: a switch and a button each.
const ID_TOOL_ON: [i32; 2] = [20, 21];
const ID_TOOL_BUTTON: [i32; 2] = [22, 23];
/// The Import page's rows: this, plus the row's place.
const ID_ROW: i32 = 100;

/// Installing or removing reached a step: wparam is `step_index`.
const WM_SETUP_STEP: u32 = WM_APP + 60;
/// Installing or removing finished: boxed `Result<(), String>`.
const WM_SETUP_FINISHED: u32 = WM_APP + 61;
/// A chosen file has been read: boxed `Read`.
const WM_SETUP_READ: u32 = WM_APP + 62;
/// Everything has started, or stopped trying to.
const WM_SETUP_EVERYTHING: u32 = WM_APP + 63;
/// Installing 1Password's or Ente's tool finished: boxed `integrations::ToolDone`.
const WM_SETUP_TOOL: u32 = WM_APP + 64;
/// Who's signed in to Ente's tool: boxed `integrations::EnteAccount`.
const WM_SETUP_ENTE: u32 = WM_APP + 65;

const TIMER_ANIMATE: usize = 1;
const TIMER_HANDOVER: usize = 2;
const HOTKEY_PROBE: i32 = 0xBFFC;

/// The launcher's message for what Setup decided: boxed `Change`.
pub const WM_ORBIT_SETUP: u32 = WM_APP + 52;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mode {
    /// Setup.exe: install or update, then hand over to the installed Orbit.
    Install,
    /// Orbit's first start. `installed` when Setup has just installed it, which leaves out the
    /// welcome page already seen there.
    Welcome { installed: bool },
    /// The Import page alone.
    Import,
    Uninstall,
}

/// What Setup asks the launcher to do. The launcher alone changes settings.
pub enum Change {
    /// Bring this over. `row` comes back with the answer, through `imported`.
    Import { row: usize, from: String, haul: Haul },
    /// An Orbit export or settings.json, which the launcher reads as it always has.
    ImportOrbitFile(PathBuf),
    Hotkey(Chord),
    Everything(bool),
    /// Show 1Password or Ente Auth in the launcher, or not.
    Tool { tool: Tool, on: bool },
    /// The window closed. `open` when its last button was pressed, to show the launcher.
    Finished { open: bool },
}

/// What the launcher tells the window when it opens inside Orbit.
pub struct InApp {
    pub main: SendHwnd,
    pub theme: Theme,
    /// The launcher's shortcut, as shown, or empty if it has none working.
    pub active_hotkey: String,
    pub everything_on: bool,
    pub files_keyword: String,
    /// Whether 1Password and Ente Auth are shown in the launcher, in `Tool::ALL`'s order.
    pub tools_on: [bool; 2],
    /// Where Setup's window was, so this one opens in its place.
    pub at: Option<(i32, i32)>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Page {
    Welcome,
    Installing,
    Import,
    Shortcut,
    Extensions,
    Done,
    Uninstall,
    Removing,
    Removed,
}

impl Page {
    fn step_name(self) -> &'static str {
        match self {
            Page::Welcome => "Welcome",
            Page::Installing => "Install",
            Page::Import => "Import",
            Page::Shortcut => "Shortcut",
            Page::Extensions => "Extensions",
            Page::Done | Page::Removed => "Done",
            Page::Uninstall | Page::Removing => "Uninstall",
        }
    }
}

enum RowKind {
    /// A launcher found on this PC, by its place in `found`.
    Launcher(usize),
    /// Choosing a file by hand.
    File,
}

struct Row {
    kind: RowKind,
    note: HWND,
    button: HWND,
    /// The file it reads: Raycast's export, or one chosen.
    path: Option<PathBuf>,
    /// The button's resting label.
    label: &'static str,
}

struct Read {
    row: usize,
    path: PathBuf,
    result: Result<Chosen, ReadError>,
}

/// 1Password's or Ente Auth's row on the Extensions page.
#[derive(Clone, Copy, Default)]
struct ToolRow {
    note: HWND,
    button: HWND,
    switch: HWND,
}

fn tool_slot(tool: Tool) -> usize {
    Tool::ALL.iter().position(|known| *known == tool).unwrap_or(0)
}

/// What a tool's row says, and what its button does: install the command-line tool Orbit uses,
/// then, for Ente, sign in. Orbit never removes one from here; Settings does that.
fn tool_row(tool: Tool, installed: bool, busy: bool, account: Option<&EnteAccount>) -> (String, &'static str, bool) {
    let maker = match tool {
        Tool::OnePassword => "1Password",
        Tool::EnteAuth => "Ente",
    };
    if busy {
        return (format!("Installing {maker}'s command-line tool. This can take a minute."), "Installing\u{2026}", false);
    }
    if !installed {
        let terms = if tool == Tool::OnePassword { ", under 1Password's terms" } else { "" };
        return (format!("Not set up. Orbit can install {maker}'s command-line tool, which it uses{terms}."), "Install", true);
    }
    match (tool, account) {
        (Tool::OnePassword, _) => ("Installed. In the 1Password app, turn on Developer \u{2192} Integrate with 1Password CLI.".to_string(), "Installed", false),
        (Tool::EnteAuth, Some(EnteAccount::SignedIn(email))) => (format!("Signed in as {email}."), "Sign in again\u{2026}", true),
        (Tool::EnteAuth, _) => ("Installed. Sign in with Ente's own sign-in, which never shows Orbit your password.".to_string(), "Sign in\u{2026}", true),
    }
}

struct State {
    mode: Mode,
    main: Option<SendHwnd>,
    style: Box<Style>,
    dpi: u32,
    width: i32,
    height: i32,
    pages: Vec<(Page, Vec<HWND>)>,
    sequence: Vec<Page>,
    at: usize,
    /// The rail's steps. After installing, the first two were done in Setup.exe.
    steps: Vec<Page>,
    back: HWND,
    next: HWND,
    status: HWND,
    // Install
    existing: install::Existing,
    was_running: bool,
    sign_in: HWND,
    desktop: HWND,
    installed: bool,
    handover_since: Option<Instant>,
    // Installing and removing
    step_text: HWND,
    progress: f32,
    shown_progress: f32,
    busy: bool,
    // Import
    found: Vec<Found>,
    rows: Vec<Row>,
    pass_controls: Vec<HWND>,
    pass_caption: HWND,
    pass_edit: HWND,
    pass_for: Option<(usize, PathBuf)>,
    orbit_file_row: Option<usize>,
    // Shortcut
    active_hotkey: String,
    choices: Vec<Chord>,
    hotkey: HWND,
    hotkey_note: HWND,
    caps_top: [i32; 2],
    // Extensions
    files_on: HWND,
    everything_note: HWND,
    everything_button: HWND,
    /// 1Password's and Ente Auth's rows, in `Tool::ALL`'s order.
    tools: [ToolRow; 2],
    /// Which tools are being installed.
    tools_busy: [bool; 2],
    /// Who's signed in to Ente's tool, once it has said.
    ente_account: Option<EnteAccount>,
    // Done
    done_note: HWND,
    // Uninstall
    remove_data: HWND,
    removed_note: HWND,
    // The logo's moon goes round once when a page opens.
    spin_from: Instant,
    logo: Option<Vec<u8>>,
}

thread_local! {
    static OPEN: Cell<Option<HWND>> = const { Cell::new(None) };
    /// Setup.exe runs the window without a launcher, so closing it ends the process.
    static STANDALONE: Cell<bool> = const { Cell::new(false) };
    /// For the developer preview: launchers to show instead of this PC's.
    static PREVIEW_FOUND: std::cell::RefCell<Option<Vec<Found>>> = const { std::cell::RefCell::new(None) };
    /// For the developer preview: the Orbit to show as installed already.
    static PREVIEW_EXISTING: std::cell::RefCell<Option<install::Existing>> = const { std::cell::RefCell::new(None) };
}

fn state(hwnd: HWND) -> Option<&'static mut State> {
    unsafe { (GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *mut State).as_mut() }
}

fn current() -> Option<&'static mut State> {
    OPEN.with(|open| open.get()).and_then(state)
}

pub fn is_open() -> bool {
    OPEN.with(|open| open.get()).is_some()
}

fn step_index(step: install::Step) -> usize {
    match step {
        install::Step::Closing => 0,
        install::Step::Copying => 1,
        install::Step::Shortcuts => 2,
        install::Step::Registering => 3,
        install::Step::Done => 4,
    }
}

fn step_from(index: usize) -> install::Step {
    [install::Step::Closing, install::Step::Copying, install::Step::Shortcuts, install::Step::Registering, install::Step::Done][index.min(4)]
}

/// Whether the installed Orbit has been set up before, so an update needn't welcome anyone.
fn onboarded() -> bool {
    std::fs::read_to_string(install::data_dir().join("settings.json")).is_ok_and(|text| orbit_core::Settings::parse(&text).0.onboarded)
}

/// The keys of a shortcut, one per keycap.
fn keys_of(chord_text: &str) -> Vec<String> {
    if chord_text.is_empty() {
        return Vec::new();
    }
    chord_text.split('+').map(|key| key.trim().to_string()).filter(|key| !key.is_empty()).collect()
}

/// The shortcuts to offer: Orbit's own first, then the usual ones, then what other launchers use.
fn hotkey_choices(active: &str, found: &[Found]) -> Vec<Chord> {
    let mut choices: Vec<Chord> = Vec::new();
    let mut add = |chord: Option<Chord>| {
        if let Some(chord) = chord {
            if !choices.contains(&chord) && !orbit_hotkeys::chord::is_reserved_on_windows(&chord) {
                choices.push(chord);
            }
        }
    };
    add(Chord::parse(active).ok());
    for text in ["Alt+Space", "Ctrl+Space", "Ctrl+Alt+Space", "Alt+Shift+Space", "Ctrl+Shift+Space"] {
        add(Chord::parse(text).ok());
    }
    for launcher in found {
        add(launcher.haul.hotkey.clone());
    }
    choices
}

impl State {
    fn px(&self, value: i32) -> i32 {
        forms::scale(value, self.dpi)
    }

    fn page(&self) -> Page {
        self.sequence[self.at]
    }

    fn rail_width(&self) -> i32 {
        self.px(RAIL_DIP)
    }

    fn content_x(&self) -> i32 {
        self.rail_width() + self.px(SIDE_DIP)
    }

    fn content_width(&self) -> i32 {
        self.width - self.content_x() - self.px(SIDE_DIP)
    }

    fn footer_top(&self) -> i32 {
        self.height - self.px(FOOTER_DIP)
    }

    fn invalidate(&self, hwnd: HWND) {
        unsafe {
            let _ = InvalidateRect(Some(hwnd), None, false);
        }
    }

    fn show_page(&mut self, hwnd: HWND, page: Page) {
        if let Some(at) = self.sequence.iter().position(|known| *known == page) {
            self.at = at;
        }
        for (owner, controls) in &self.pages {
            let show = if *owner == page { SW_SHOW } else { SW_HIDE };
            for control in controls {
                // The passphrase box shows only when a file asks for one.
                let show = if self.pass_controls.contains(control) && self.pass_for.is_none() { SW_HIDE } else { show };
                unsafe {
                    let _ = ShowWindow(*control, show);
                }
            }
        }
        self.problem("");
        self.spin_from = Instant::now();
        self.arm_animation(hwnd);
        self.update_footer();
        match page {
            Page::Shortcut => self.update_hotkey_note(hwnd),
            Page::Extensions => {
                self.refresh_everything();
                for tool in Tool::ALL {
                    self.refresh_tool(tool);
                }
                // Ente's row says who's signed in, which its tool has to be asked.
                if self.ente_account.is_none() && !forms::is_preview() && integrations::find(Tool::EnteAuth).is_some() {
                    integrations::check_ente_account(SendHwnd::new(hwnd), WM_SETUP_ENTE);
                }
            }
            Page::Done => self.update_done_note(),
            _ => {}
        }
        self.invalidate(hwnd);
        self.focus_first();
    }

    fn focus_first(&self) {
        let focus = if unsafe { IsWindowEnabled(self.next) }.as_bool() && unsafe { IsWindowVisible(self.next) }.as_bool() { self.next } else { self.back };
        forms::focus(focus);
    }

    fn arm_animation(&self, hwnd: HWND) {
        if !forms::is_preview() {
            unsafe {
                SetTimer(Some(hwnd), TIMER_ANIMATE, 15, None);
            }
        }
    }

    fn set_button(&self, button: HWND, text: &str, kind: ButtonKind, enabled: bool, visible: bool) {
        unsafe {
            forms::set_text(button, text);
            SetWindowLongPtrW(button, GWLP_USERDATA, kind as isize);
            let _ = EnableWindow(button, enabled);
            let _ = ShowWindow(button, if visible { SW_SHOW } else { SW_HIDE });
            let _ = InvalidateRect(Some(button), None, true);
        }
    }

    /// The foot's buttons say what pressing them does on this page.
    fn update_footer(&self) {
        let first = self.at == 0;
        let (back, next, next_kind, next_on): (Option<&str>, &str, ButtonKind, bool) = match self.page() {
            Page::Welcome if self.mode == Mode::Install => {
                let label = match &self.existing.version {
                    Some(version) => match install::compare_versions(version, install::VERSION) {
                        std::cmp::Ordering::Less => "Update",
                        _ => "Reinstall",
                    },
                    None => "Install",
                };
                (None, label, ButtonKind::Primary, true)
            }
            Page::Welcome => (None, "Get started", ButtonKind::Primary, true),
            Page::Installing => {
                let failed = !self.busy && !self.installed;
                (failed.then_some("Back"), if failed { "Try again" } else { "Installing\u{2026}" }, ButtonKind::Primary, failed)
            }
            Page::Import if self.mode == Mode::Import => (None, "Done", ButtonKind::Primary, true),
            Page::Import | Page::Shortcut | Page::Extensions => ((!first).then_some("Back"), "Next", ButtonKind::Primary, true),
            Page::Done => ((!first && self.mode != Mode::Install).then_some("Back"), "Open Orbit", ButtonKind::Primary, true),
            Page::Uninstall => (Some("Cancel"), "Uninstall", ButtonKind::Danger, true),
            Page::Removing => (None, "Uninstalling\u{2026}", ButtonKind::Danger, false),
            Page::Removed => (None, "Close", ButtonKind::Primary, true),
        };
        self.set_button(self.back, back.unwrap_or("Back"), ButtonKind::Normal, back.is_some(), back.is_some());
        self.set_button(self.next, next, next_kind, next_on, true);
    }

    fn problem(&mut self, text: &str) {
        self.style.set_alert(self.status, !text.is_empty());
        forms::set_status(self.status, text);
        unsafe {
            let _ = InvalidateRect(Some(self.status), None, true);
        }
    }

    fn post(&self, change: Change) {
        if let Some(main) = self.main {
            main.post(WM_ORBIT_SETUP, crate::util::box_for_post(change), 0);
        }
    }

    // --- Install --------------------------------------------------------------------------

    fn start_install(&mut self, hwnd: HWND) {
        let choices = install::Choices { start_at_sign_in: forms::is_checked(self.sign_in), desktop_shortcut: forms::is_checked(self.desktop) };
        self.busy = true;
        self.installed = false;
        self.progress = 0.0;
        self.shown_progress = 0.0;
        self.show_page(hwnd, Page::Installing);
        forms::set_status(self.step_text, install::Step::Closing.label());
        let reply = SendHwnd::new(hwnd);
        let _ = std::thread::Builder::new().name("orbit-setup-install".into()).spawn(move || {
            com_for_this_thread();
            let result = install::install(choices, |step| reply.post(WM_SETUP_STEP, step_index(step), 0));
            reply.post(WM_SETUP_FINISHED, crate::util::box_for_post(result), 0);
        });
    }

    fn start_uninstall(&mut self, hwnd: HWND) {
        let remove_data = forms::is_checked(self.remove_data);
        self.busy = true;
        self.progress = 0.0;
        self.shown_progress = 0.0;
        self.show_page(hwnd, Page::Removing);
        forms::set_status(self.step_text, install::Step::Closing.label());
        let reply = SendHwnd::new(hwnd);
        let _ = std::thread::Builder::new().name("orbit-setup-uninstall".into()).spawn(move || {
            com_for_this_thread();
            let result = install::uninstall(remove_data, |step| reply.post(WM_SETUP_STEP, step_index(step), 0));
            reply.post(WM_SETUP_FINISHED, crate::util::box_for_post(result), 0);
        });
    }

    fn on_step(&mut self, hwnd: HWND, step: install::Step) {
        self.progress = step.fraction();
        let label = if self.mode == Mode::Uninstall {
            match step {
                install::Step::Closing => "Closing Orbit\u{2026}",
                install::Step::Shortcuts => "Taking away its shortcuts\u{2026}",
                install::Step::Registering => "Taking it out of Settings \u{2192} Apps\u{2026}",
                install::Step::Copying => "Deleting its files\u{2026}",
                install::Step::Done => "Done",
            }
        } else {
            step.label()
        };
        // The steps of an uninstall come in another order, so the bar only goes forward.
        if self.mode == Mode::Uninstall {
            self.progress = self.progress.max(self.shown_progress);
        }
        forms::set_status(self.step_text, label);
        self.arm_animation(hwnd);
    }

    fn on_finished(&mut self, hwnd: HWND, result: Result<(), String>) {
        self.busy = false;
        self.progress = 1.0;
        if self.mode == Mode::Uninstall {
            let text = match result {
                Ok(()) => "Orbit has been removed from this PC. Thanks for trying it.".to_string(),
                Err(why) => why,
            };
            forms::set_status(self.removed_note, &text);
            self.show_page(hwnd, Page::Removed);
            return;
        }
        match result {
            Ok(()) => {
                self.installed = true;
                let fresh = self.existing.version.is_none() || !onboarded();
                if fresh && self.hand_over(hwnd) {
                    forms::set_status(self.step_text, "Starting Orbit\u{2026}");
                    self.update_footer();
                } else {
                    self.show_page(hwnd, Page::Done);
                }
            }
            Err(why) => {
                forms::set_status(self.step_text, &why);
                self.update_footer();
            }
        }
    }

    /// Starts the installed Orbit, which carries on from here in a window of its own, opened
    /// where this one is. This one closes once that one shows.
    fn hand_over(&mut self, hwnd: HWND) -> bool {
        let mut rect = RECT::default();
        unsafe {
            let _ = GetWindowRect(hwnd, &mut rect);
        }
        let place = format!("{},{}", rect.left, rect.top);
        let started = std::process::Command::new(install::installed_exe()).args(["--setup-continue", &place]).current_dir(install::install_dir()).spawn().is_ok();
        if started {
            self.handover_since = Some(Instant::now());
            unsafe {
                SetTimer(Some(hwnd), TIMER_HANDOVER, 120, None);
            }
        }
        started
    }

    fn on_handover_tick(&mut self, hwnd: HWND) {
        let Some(since) = self.handover_since else { return };
        if other_setup_window(hwnd) {
            unsafe {
                let _ = KillTimer(Some(hwnd), TIMER_HANDOVER);
                let _ = DestroyWindow(hwnd);
            }
        } else if since.elapsed() > Duration::from_secs(20) {
            // It started, but its window didn't come. Orbit is installed all the same.
            self.handover_since = None;
            unsafe {
                let _ = KillTimer(Some(hwnd), TIMER_HANDOVER);
            }
            self.show_page(hwnd, Page::Done);
        }
    }

    // --- Import ---------------------------------------------------------------------------

    fn row_pressed(&mut self, hwnd: HWND, row: usize) {
        let Some(entry) = self.rows.get(row) else { return };
        match entry.kind {
            RowKind::Launcher(index) => {
                let found = &self.found[index];
                if found.has_things() {
                    let change = Change::Import { row, from: found.launcher.name().to_string(), haul: found.haul.clone() };
                    self.set_button(entry.button, "Importing\u{2026}", ButtonKind::Normal, false, true);
                    self.post(change);
                } else if let Some(path) = entry.path.clone() {
                    self.start_read(hwnd, row, path, None);
                } else if let Some(path) = pick_file(hwnd, "Raycast export (*.rayconfig, *.json)\0*.rayconfig;*.json\0All files\0*.*\0") {
                    self.start_read(hwnd, row, path, None);
                }
            }
            RowKind::File => {
                if let Some(path) = pick_file(hwnd, "Exports (*.rayconfig, *.json)\0*.rayconfig;*.json\0All files\0*.*\0") {
                    self.start_read(hwnd, row, path, None);
                }
            }
        }
    }

    /// Reads on another thread: a Raycast passphrase takes a moment to check, by design.
    fn start_read(&mut self, hwnd: HWND, row: usize, path: PathBuf, passphrase: Option<String>) {
        let Some(entry) = self.rows.get(row) else { return };
        self.set_button(entry.button, "Reading\u{2026}", ButtonKind::Normal, false, true);
        self.problem("");
        let reply = SendHwnd::new(hwnd);
        let _ = std::thread::Builder::new().name("orbit-setup-read".into()).spawn(move || {
            let result = importer::read_file(&path, passphrase.as_deref());
            reply.post(WM_SETUP_READ, crate::util::box_for_post(Read { row, path, result }), 0);
        });
    }

    fn on_read(&mut self, hwnd: HWND, read: Read) {
        let Some(entry) = self.rows.get(read.row) else { return };
        let (note, button, label) = (entry.note, entry.button, entry.label);
        let file_name = read.path.file_name().map(|name| name.to_string_lossy().into_owned()).unwrap_or_default();
        match read.result {
            Ok(Chosen::Haul(haul, from)) => {
                self.hide_passphrase(hwnd);
                if let Some(chord) = &haul.hotkey {
                    self.offer_hotkey(chord.clone());
                }
                if haul.links.is_empty() && haul.snippets.is_empty() && haul.notes.is_empty() {
                    let hotkey = haul.hotkey.as_ref().map(|chord| format!(" Its shortcut, {}, is on the next page.", chord.display("Win"))).unwrap_or_default();
                    forms::set_status(note, &format!("{file_name} has nothing Orbit can bring over.{hotkey}"));
                    self.set_button(button, label, ButtonKind::Normal, true, true);
                    return;
                }
                forms::set_status(note, &format!("Bringing over {}\u{2026}", things(&haul)));
                self.set_button(button, "Importing\u{2026}", ButtonKind::Normal, false, true);
                self.post(Change::Import { row: read.row, from: from.to_string(), haul });
            }
            Ok(Chosen::Orbit) => {
                self.hide_passphrase(hwnd);
                self.orbit_file_row = Some(read.row);
                forms::set_status(note, &format!("Importing {file_name}\u{2026}"));
                self.set_button(button, "Importing\u{2026}", ButtonKind::Normal, false, true);
                self.post(Change::ImportOrbitFile(read.path));
            }
            Err(why @ (ReadError::NeedsPassphrase | ReadError::WrongPassphrase)) => {
                let asked_before = self.pass_for.is_some();
                self.pass_for = Some((read.row, read.path));
                forms::set_status(self.pass_caption, &format!("{file_name} is encrypted. Type the passphrase you chose when you exported it."));
                for control in &self.pass_controls {
                    unsafe {
                        let _ = ShowWindow(*control, SW_SHOW);
                    }
                }
                self.set_button(button, label, ButtonKind::Normal, true, true);
                if asked_before && why == ReadError::WrongPassphrase {
                    self.problem(&why.to_string());
                }
                unsafe {
                    SendMessageW(self.pass_edit, 0x00B1, Some(WPARAM(0)), Some(LPARAM(-1)));
                }
                forms::focus(self.pass_edit);
                self.invalidate(hwnd);
            }
            Err(why) => {
                forms::set_status(note, &why.to_string());
                self.set_button(button, label, ButtonKind::Normal, true, true);
            }
        }
    }

    fn hide_passphrase(&mut self, hwnd: HWND) {
        if self.pass_for.take().is_some() {
            forms::set_text(self.pass_edit, "");
            for control in &self.pass_controls {
                unsafe {
                    let _ = ShowWindow(*control, SW_HIDE);
                }
            }
            self.invalidate(hwnd);
        }
    }

    fn open_with_passphrase(&mut self, hwnd: HWND) {
        let Some((row, path)) = self.pass_for.clone() else { return };
        let passphrase = forms::get_text(self.pass_edit);
        if passphrase.is_empty() {
            forms::focus(self.pass_edit);
            return;
        }
        self.start_read(hwnd, row, path, Some(passphrase));
    }

    fn finish_row(&self, row: usize, text: &str, ok: bool) {
        let Some(entry) = self.rows.get(row) else { return };
        forms::set_status(entry.note, text);
        if ok {
            self.set_button(entry.button, "Imported", ButtonKind::Normal, false, true);
        } else {
            self.set_button(entry.button, entry.label, ButtonKind::Normal, true, true);
        }
    }

    // --- Shortcut -------------------------------------------------------------------------

    fn offer_hotkey(&mut self, chord: Chord) {
        if self.choices.contains(&chord) || orbit_hotkeys::chord::is_reserved_on_windows(&chord) {
            return;
        }
        let text = crate::util::wide(&chord.display("Win"));
        unsafe {
            SendMessageW(self.hotkey, CB_ADDSTRING, None, Some(LPARAM(text.as_ptr() as isize)));
        }
        self.choices.push(chord);
    }

    fn chosen_hotkey(&self) -> Option<Chord> {
        self.choices.get(forms::selected(self.hotkey)).cloned()
    }

    /// Says whether the chosen shortcut is free, and who has it if not.
    fn update_hotkey_note(&mut self, hwnd: HWND) {
        let Some(chord) = self.chosen_hotkey() else { return };
        let shown = chord.display("Win");
        let user = self.found.iter().find(|found| found.haul.hotkey.as_ref() == Some(&chord)).map(|found| found.launcher);
        let text = if shown == self.active_hotkey {
            "Orbit uses it now.".to_string()
        } else if probe_free(hwnd, &chord) {
            match user {
                Some(launcher) => format!("Free now. {} uses it too when it's running, so turn its shortcut off there.", launcher.name()),
                None => "Free to use.".to_string(),
            }
        } else {
            match user.filter(|launcher| importer::is_running(*launcher)) {
                Some(launcher) => format!("{} uses it right now. Quit {} or change its shortcut, or choose another.", launcher.name(), launcher.name()),
                None => "Another program uses it right now. Choose another, or close that program first.".to_string(),
            }
        };
        forms::set_status(self.hotkey_note, &text);
        self.invalidate(hwnd);
    }

    fn apply_hotkey(&mut self) {
        let Some(chord) = self.chosen_hotkey() else { return };
        let shown = chord.display("Win");
        if shown != self.active_hotkey {
            self.active_hotkey = shown;
            self.post(Change::Hotkey(chord));
        }
    }

    fn update_done_note(&self) {
        let text = if self.mode == Mode::Install {
            let updated = self.existing.version.is_some();
            if updated {
                format!("Orbit {} is installed. Your settings and data are as they were.", install::VERSION)
            } else {
                format!("Orbit {} is installed. Open it from Start, or from its icon by the clock.", install::VERSION)
            }
        } else if self.active_hotkey.is_empty() {
            "Open Orbit from its icon by the clock, then type what you want. Choose a shortcut in Settings \u{2192} Shortcuts.".to_string()
        } else {
            format!("Press {} anywhere to open Orbit, then type what you want. Ctrl+K shows what else you can do with what you've found.", self.active_hotkey)
        };
        forms::set_status(self.done_note, &text);
    }

    // --- Extensions -----------------------------------------------------------------------

    fn refresh_everything(&self) {
        if self.everything_note.is_invalid() {
            return;
        }
        let (text, label, can_press) = crate::settings_win::everything_row();
        forms::set_status(self.everything_note, &text);
        forms::set_text(self.everything_button, label);
        unsafe {
            let _ = EnableWindow(self.everything_button, can_press);
            let _ = InvalidateRect(Some(self.everything_button), None, true);
        }
    }

    fn everything_pressed(&mut self, hwnd: HWND) {
        let Some(main) = self.main else { return };
        match crate::settings_win::press_everything(main, SendHwnd::new(hwnd), WM_SETUP_EVERYTHING) {
            Ok(Some(note)) => forms::set_status(self.everything_note, note),
            Ok(None) => {}
            Err(why) => self.problem(&why),
        }
        self.refresh_everything();
    }

    fn refresh_tool(&self, tool: Tool) {
        let slot = tool_slot(tool);
        let row = self.tools[slot];
        if row.note.is_invalid() {
            return;
        }
        let installed = integrations::find(tool).is_some();
        let (text, label, can_press) = tool_row(tool, installed, self.tools_busy[slot], self.ente_account.as_ref());
        forms::set_status(row.note, &text);
        forms::set_text(row.button, label);
        unsafe {
            if !can_press && GetFocus() == row.button {
                forms::focus(self.next);
            }
            let _ = EnableWindow(row.button, can_press);
            // Nothing to search until the tool is there, unless it's on already.
            let _ = EnableWindow(row.switch, installed || forms::is_checked(row.switch));
            let _ = InvalidateRect(Some(row.button), None, true);
            let _ = InvalidateRect(Some(row.switch), None, true);
        }
    }

    /// Install, or for Ente once it's installed, sign in.
    fn tool_pressed(&mut self, hwnd: HWND, tool: Tool) {
        let slot = tool_slot(tool);
        if self.tools_busy[slot] {
            return;
        }
        if integrations::find(tool).is_none() {
            self.tools_busy[slot] = true;
            self.problem("");
            integrations::change_tool(tool, true, SendHwnd::new(hwnd), WM_SETUP_TOOL);
        } else if tool == Tool::EnteAuth {
            // Ente's own sign-in, in a console. Who's signed in comes back here when it closes.
            if let Err(why) = integrations::open_sign_in(tool, SendHwnd::new(hwnd), WM_SETUP_ENTE) {
                self.problem(&why);
            }
        }
        self.refresh_tool(tool);
    }

    fn tool_switched(&self, tool: Tool) {
        self.post(Change::Tool { tool, on: forms::is_checked(self.tools[tool_slot(tool)].switch) });
    }

    /// A tool has been installed, or couldn't be. Having just installed it, the user wants it on.
    fn on_tool_done(&mut self, hwnd: HWND, done: ToolDone) {
        let slot = tool_slot(done.tool);
        self.tools_busy[slot] = false;
        if done.succeeded {
            forms::set_checked(self.tools[slot].switch, true);
            self.tool_switched(done.tool);
            if done.tool == Tool::EnteAuth {
                integrations::check_ente_account(SendHwnd::new(hwnd), WM_SETUP_ENTE);
            }
            self.tell(&done.message);
        } else {
            self.problem(&done.message);
        }
        self.refresh_tool(done.tool);
    }

    fn on_ente_account(&mut self, account: EnteAccount) {
        self.ente_account = Some(account);
        self.refresh_tool(Tool::EnteAuth);
    }

    /// Something that went well, at the foot of the window.
    fn tell(&mut self, text: &str) {
        self.style.set_alert(self.status, false);
        forms::set_status(self.status, text);
        unsafe {
            let _ = InvalidateRect(Some(self.status), None, true);
        }
    }

    // --- Moving between pages -------------------------------------------------------------

    fn next_pressed(&mut self, hwnd: HWND) {
        match self.page() {
            Page::Welcome if self.mode == Mode::Install => self.start_install(hwnd),
            Page::Installing => {
                if !self.busy && !self.installed {
                    self.start_install(hwnd);
                }
            }
            Page::Uninstall => self.start_uninstall(hwnd),
            Page::Removed => unsafe {
                let _ = DestroyWindow(hwnd);
            },
            Page::Done => {
                if self.mode == Mode::Install {
                    install::launch_installed(true, false);
                    self.was_running = false;
                } else {
                    self.post(Change::Finished { open: true });
                    self.main = None;
                }
                unsafe {
                    let _ = DestroyWindow(hwnd);
                }
            }
            Page::Import if self.mode == Mode::Import => unsafe {
                let _ = DestroyWindow(hwnd);
            },
            page => {
                if page == Page::Shortcut {
                    self.apply_hotkey();
                }
                if let Some(next) = self.sequence.get(self.at + 1).copied() {
                    self.show_page(hwnd, next);
                }
            }
        }
    }

    fn back_pressed(&mut self, hwnd: HWND) {
        match self.page() {
            Page::Uninstall => unsafe {
                let _ = DestroyWindow(hwnd);
            },
            Page::Installing => self.show_page(hwnd, Page::Welcome),
            page => {
                if page == Page::Shortcut {
                    self.apply_hotkey();
                }
                if self.at > 0 {
                    let previous = self.sequence[self.at - 1];
                    self.show_page(hwnd, previous);
                }
            }
        }
    }

    // --- Painting -------------------------------------------------------------------------

    fn decorate(&self, buffer: &mut Backbuffer) {
        let theme = &self.style.theme;
        let rail = self.rail_width();
        buffer.fill_rect(0, 0, rail, self.height, rail_color(theme));
        self.draw_logo(buffer);
        let center = RECT { left: 0, top: 0, right: rail, bottom: 0 };
        let name_top = self.px(36 + LOGO_DIP + 26);
        buffer.text_centered("Orbit", self.style.title, theme.text, RECT { top: name_top, bottom: name_top + self.px(30), ..center });
        let under = match self.mode {
            Mode::Install => format!("Setup \u{00B7} {}", install::VERSION),
            Mode::Uninstall => "Uninstall".to_string(),
            Mode::Import => "Import".to_string(),
            Mode::Welcome { .. } => format!("Version {}", install::VERSION),
        };
        buffer.text_centered(&under, self.style.small, theme.secondary, RECT { top: name_top + self.px(30), bottom: name_top + self.px(48), ..center });
        if self.steps.len() > 1 {
            self.draw_steps(buffer, name_top + self.px(84));
        }
        // The foot.
        let inset = 1.max(self.dpi as i32 / 96);
        buffer.fill_rect(rail, self.footer_top(), self.width - rail, inset, theme.divider);
        match self.page() {
            Page::Installing | Page::Removing => self.draw_progress(buffer),
            Page::Shortcut => {
                if let Some(chord) = self.chosen_hotkey() {
                    self.draw_keycaps(buffer, &chord.display("Win"), self.caps_top[0]);
                }
            }
            Page::Done if self.mode != Mode::Install => self.draw_keycaps(buffer, &self.active_hotkey, self.caps_top[1]),
            _ => {}
        }
    }

    /// Orbit's tile with a small moon going round it, behind it at the top of its path.
    fn draw_logo(&self, buffer: &mut Backbuffer) {
        let theme = &self.style.theme;
        let size = self.px(LOGO_DIP);
        let (x, y) = ((self.rail_width() - size) / 2, self.px(36));
        let (cx, cy) = (x as f32 + size as f32 / 2.0, y as f32 + size as f32 / 2.0);
        let elapsed = self.spin_from.elapsed().as_secs_f32();
        let turn = (elapsed / 1.6).min(1.0);
        // Eases out, coming to rest at the lower right.
        let eased = 1.0 - (1.0 - turn).powi(3);
        let angle = 0.55 + eased * std::f32::consts::TAU;
        let (rx, ry) = (size as f32 * 0.82, size as f32 * 0.3);
        let moon = self.px(9);
        let place = |angle: f32| (cx + rx * angle.cos() - moon as f32 / 2.0, cy + ry * angle.sin() - moon as f32 / 2.0);
        let behind = angle.sin() < 0.0;
        let draw_moon = |buffer: &mut Backbuffer| {
            // A short trail behind it while it moves.
            if turn < 1.0 {
                for back in 1..=4 {
                    let (tx, ty) = place(angle - back as f32 * 0.16);
                    let small = moon - back;
                    buffer.fill_round_rect_alpha(tx as i32 + back / 2, ty as i32 + back / 2, small, small, small as f32 / 2.0, theme.accent, (150 - back * 30) as u8);
                }
            }
            let (mx, my) = place(angle);
            buffer.fill_round_rect(mx as i32, my as i32, moon, moon, moon as f32 / 2.0, theme.accent);
        };
        if behind {
            draw_moon(buffer);
        }
        match &self.logo {
            Some(pixels) => buffer.blit_premultiplied(x, y, size, pixels),
            None => buffer.fill_round_rect(x, y, size, size, size as f32 * 0.22, Rgb(217, 119, 6)),
        }
        if !behind {
            draw_moon(buffer);
        }
    }

    fn draw_steps(&self, buffer: &mut Backbuffer, top: i32) {
        let theme = &self.style.theme;
        let current = self.page();
        let at = self.steps.iter().position(|step| step.step_name() == current.step_name()).unwrap_or(0);
        let (left, dot, row) = (self.px(40), self.px(24), self.px(40));
        for (index, step) in self.steps.iter().enumerate() {
            let y = top + index as i32 * row;
            let done = index < at;
            let here = index == at;
            let circle = RECT { left, top: y, right: left + dot, bottom: y + dot };
            if done {
                buffer.fill_round_rect(left, y, dot, dot, dot as f32 / 2.0, theme.accent);
                buffer.symbol_centered("\u{E73E}", self.style.icon_small, self.px(12), true, theme.on_accent, circle);
            } else if here {
                buffer.fill_round_rect(left, y, dot, dot, dot as f32 / 2.0, theme.accent);
                buffer.text_centered(&(index + 1).to_string(), self.style.small, theme.on_accent, circle);
            } else {
                let ring = 1.max(self.dpi as i32 / 96) * 2;
                buffer.fill_round_rect(left, y, dot, dot, dot as f32 / 2.0, theme.divider);
                buffer.fill_round_rect(left + ring / 2, y + ring / 2, dot - ring, dot - ring, (dot - ring) as f32 / 2.0, rail_color(theme));
                buffer.text_centered(&(index + 1).to_string(), self.style.small, theme.tertiary, circle);
            }
            let text = RECT { left: left + dot + self.px(14), top: y, right: self.rail_width() - self.px(12), bottom: y + dot };
            let (font, color) = if here { (self.style.bold, theme.text) } else if done { (self.style.font, theme.secondary) } else { (self.style.font, theme.tertiary) };
            buffer.text(step.step_name(), font, color, text, false);
            // A line down to the next step.
            if index + 1 < self.steps.len() {
                let line = 1.max(self.dpi as i32 / 96);
                buffer.fill_rect(left + dot / 2 - line / 2, y + dot + self.px(3), line, row - dot - self.px(6), if done { theme.accent } else { theme.divider });
            }
        }
    }

    fn draw_progress(&self, buffer: &mut Backbuffer) {
        let theme = &self.style.theme;
        let (x, width, height) = (self.content_x(), self.content_width(), self.px(6));
        let y = self.caps_top[0];
        buffer.fill_round_rect(x, y, width, height, height as f32 / 2.0, if self.style.is_dark() { theme.field } else { theme.keycap });
        let filled = ((width as f32) * self.shown_progress.clamp(0.0, 1.0)) as i32;
        if filled > 0 {
            let failed = !self.busy && !self.installed && self.mode == Mode::Install;
            buffer.fill_round_rect(x, y, filled.max(height), height, height as f32 / 2.0, if failed { theme.danger } else { theme.accent });
        }
    }

    /// The shortcut as keys, large, in the middle of the page.
    fn draw_keycaps(&self, buffer: &mut Backbuffer, text: &str, top: i32) {
        let theme = &self.style.theme;
        let keys = keys_of(text);
        if keys.is_empty() {
            return;
        }
        let (height, gap, pad) = (self.px(52), self.px(12), self.px(22));
        let widths: Vec<i32> = keys.iter().map(|key| (buffer.text_width(key, self.style.title) + pad * 2).max(height)).collect();
        let total: i32 = widths.iter().sum::<i32>() + gap * (keys.len() as i32 - 1);
        let mut x = self.content_x() + (self.content_width() - total) / 2;
        let face = if self.style.is_dark() { theme.card.mix(theme.text, 0.08) } else { theme.card };
        let edge = if self.style.is_dark() { theme.card.mix(Rgb(0, 0, 0), 0.35) } else { theme.divider.mix(theme.text, 0.12) };
        let radius = self.px(10) as f32;
        for (key, width) in keys.iter().zip(widths) {
            buffer.fill_round_rect(x, top, width, height, radius, edge);
            buffer.fill_round_rect(x, top, width, height - self.px(3), radius, face);
            buffer.text_centered(key, self.style.title, theme.text, RECT { left: x, top, right: x + width, bottom: top + height - self.px(3) });
            x += width + gap;
        }
    }

    fn animate(&mut self, hwnd: HWND) {
        let spinning = self.spin_from.elapsed() < Duration::from_millis(1700);
        let target = self.progress;
        let mut moving = false;
        if (self.shown_progress - target).abs() > 0.002 {
            self.shown_progress += (target - self.shown_progress) * 0.12;
            moving = true;
        } else {
            self.shown_progress = target;
        }
        if !spinning && !moving {
            unsafe {
                let _ = KillTimer(Some(hwnd), TIMER_ANIMATE);
            }
        }
        // Only the logo and the bar move.
        let size = self.px(LOGO_DIP);
        let logo = RECT { left: 0, top: self.px(10), right: self.rail_width(), bottom: self.px(36) + size + self.px(26) };
        let bar = RECT { left: self.content_x(), top: self.caps_top[0] - self.px(2), right: self.content_x() + self.content_width(), bottom: self.caps_top[0] + self.px(10) };
        unsafe {
            let _ = InvalidateRect(Some(hwnd), Some(&logo), false);
            if matches!(self.page(), Page::Installing | Page::Removing) {
                let _ = InvalidateRect(Some(hwnd), Some(&bar), false);
            }
        }
    }
}

fn rail_color(theme: &Theme) -> Rgb {
    if theme.high_contrast {
        theme.sidebar
    } else if theme.background.0 < 128 {
        theme.sidebar.mix(theme.card, 0.6)
    } else {
        theme.sidebar.mix(theme.text, 0.035)
    }
}

/// Shortcuts and the Settings → Apps entry are written through COM.
fn com_for_this_thread() {
    unsafe {
        let _ = windows::Win32::System::Com::CoInitializeEx(None, windows::Win32::System::Com::COINIT_APARTMENTTHREADED);
    }
}

/// Whether a shortcut could be registered now. Orbit's own counts as taken here.
fn probe_free(hwnd: HWND, chord: &Chord) -> bool {
    let Some((mods, vk)) = crate::keys::chord_to_registration(chord) else { return false };
    unsafe {
        let free = RegisterHotKey(Some(hwnd), HOTKEY_PROBE, mods, vk).is_ok();
        if free {
            let _ = UnregisterHotKey(Some(hwnd), HOTKEY_PROBE);
        }
        free
    }
}

/// Another process's Setup window: the installed Orbit, carrying on.
fn other_setup_window(mine: HWND) -> bool {
    unsafe {
        let me = GetCurrentProcessId();
        let mut after: Option<HWND> = None;
        while let Ok(window) = FindWindowExW(None, after, CLASS, PCWSTR::null()) {
            if window.is_invalid() {
                break;
            }
            let mut owner = 0u32;
            GetWindowThreadProcessId(window, Some(&mut owner));
            if window != mine && owner != me && IsWindowVisible(window).as_bool() {
                return true;
            }
            after = Some(window);
        }
        false
    }
}

fn pick_file(owner: HWND, filter: &str) -> Option<PathBuf> {
    let mut buffer = vec![0u16; 1024];
    let filter = crate::util::wide(filter);
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
        PathBuf::from(String::from_utf16_lossy(&buffer[..end]))
    })
}

unsafe extern "system" fn setup_proc(hwnd: HWND, message: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    unsafe {
        if let Some(s) = state(hwnd) {
            if message == WM_PAINT {
                let s: &State = s;
                s.style.paint(hwnd, |buffer| s.decorate(buffer));
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
                        x if x == ID_NEXT || (x == IDOK.0 && s.pass_for.is_none()) => s.next_pressed(hwnd),
                        x if x == IDOK.0 => s.open_with_passphrase(hwnd),
                        ID_BACK => s.back_pressed(hwnd),
                        x if x == IDCANCEL.0 => {
                            if !s.busy {
                                let _ = DestroyWindow(hwnd);
                            }
                        }
                        ID_PASS_OPEN => s.open_with_passphrase(hwnd),
                        ID_HOTKEY if notification == CBN_SELCHANGE => s.update_hotkey_note(hwnd),
                        ID_FILES_ON if notification == BN_CLICKED => s.post(Change::Everything(forms::is_checked(s.files_on))),
                        ID_EVERYTHING => s.everything_pressed(hwnd),
                        x if ID_TOOL_ON.contains(&x) && notification == BN_CLICKED => {
                            s.tool_switched(Tool::ALL[ID_TOOL_ON.iter().position(|id| *id == x).unwrap_or(0)]);
                        }
                        x if ID_TOOL_BUTTON.contains(&x) => s.tool_pressed(hwnd, Tool::ALL[ID_TOOL_BUTTON.iter().position(|id| *id == x).unwrap_or(0)]),
                        x if (ID_ROW..ID_ROW + 50).contains(&x) => s.row_pressed(hwnd, (x - ID_ROW) as usize),
                        _ => {}
                    }
                }
                LRESULT(0)
            }
            WM_SETUP_STEP => {
                if let Some(s) = state(hwnd) {
                    s.on_step(hwnd, step_from(wparam.0));
                }
                LRESULT(0)
            }
            WM_SETUP_FINISHED => {
                if let (Some(result), Some(s)) = (crate::util::take_posted::<Result<(), String>>(wparam.0), state(hwnd)) {
                    s.on_finished(hwnd, *result);
                }
                LRESULT(0)
            }
            WM_SETUP_READ => {
                if let (Some(read), Some(s)) = (crate::util::take_posted::<Read>(wparam.0), state(hwnd)) {
                    s.on_read(hwnd, *read);
                }
                LRESULT(0)
            }
            WM_SETUP_EVERYTHING => {
                if let Some(s) = state(hwnd) {
                    s.refresh_everything();
                }
                LRESULT(0)
            }
            WM_SETUP_TOOL => {
                if let (Some(done), Some(s)) = (crate::util::take_posted::<ToolDone>(wparam.0), state(hwnd)) {
                    s.on_tool_done(hwnd, *done);
                }
                LRESULT(0)
            }
            WM_SETUP_ENTE => {
                if let (Some(account), Some(s)) = (crate::util::take_posted::<EnteAccount>(wparam.0), state(hwnd)) {
                    s.on_ente_account(*account);
                }
                LRESULT(0)
            }
            WM_TIMER => {
                if let Some(s) = state(hwnd) {
                    match wparam.0 {
                        TIMER_ANIMATE => s.animate(hwnd),
                        TIMER_HANDOVER => s.on_handover_tick(hwnd),
                        _ => {}
                    }
                }
                LRESULT(0)
            }
            WM_CLOSE => {
                // Not halfway through copying or deleting.
                if state(hwnd).is_some_and(|s| s.busy) {
                    return LRESULT(0);
                }
                let _ = DestroyWindow(hwnd);
                LRESULT(0)
            }
            WM_DESTROY => {
                crate::editor::unregister_form(hwnd);
                let pointer = SetWindowLongPtrW(hwnd, GWLP_USERDATA, 0) as *mut State;
                if !pointer.is_null() {
                    let owned = Box::from_raw(pointer);
                    // Closed some other way than its last button: the launcher still marks the
                    // welcome as seen, and an update that closed Orbit starts it again.
                    if owned.main.is_some() {
                        owned.post(Change::Finished { open: false });
                    }
                    if owned.mode == Mode::Install && owned.installed && owned.was_running && owned.handover_since.is_none() {
                        install::launch_installed(false, false);
                    }
                }
                OPEN.with(|open| open.set(None));
                if STANDALONE.with(|standalone| standalone.get()) {
                    PostQuitMessage(0);
                }
                LRESULT(0)
            }
            _ => DefWindowProcW(hwnd, message, wparam, lparam),
        }
    }
}

/// The pages a mode goes through, and the steps its rail shows.
fn plan(mode: Mode, existing: &install::Existing) -> (Vec<Page>, Vec<Page>) {
    let welcome = [Page::Import, Page::Shortcut, Page::Extensions, Page::Done];
    match mode {
        Mode::Install => {
            let fresh = existing.version.is_none() || !onboarded();
            let steps = if fresh { vec![Page::Welcome, Page::Installing, Page::Import, Page::Shortcut, Page::Extensions, Page::Done] } else { vec![Page::Welcome, Page::Installing, Page::Done] };
            (vec![Page::Welcome, Page::Installing, Page::Done], steps)
        }
        Mode::Welcome { installed: true } => (welcome.to_vec(), vec![Page::Welcome, Page::Installing, Page::Import, Page::Shortcut, Page::Extensions, Page::Done]),
        Mode::Welcome { installed: false } => {
            let mut sequence = vec![Page::Welcome];
            sequence.extend(welcome);
            (sequence.clone(), sequence)
        }
        Mode::Import => (vec![Page::Import], vec![Page::Import]),
        Mode::Uninstall => (vec![Page::Uninstall, Page::Removing, Page::Removed], vec![Page::Uninstall]),
    }
}

/// What there is to bring over, without the shortcut, which has its own page.
fn things(haul: &Haul) -> String {
    Haul { hotkey: None, ..haul.clone() }.summary()
}

/// The text on a found launcher's row.
fn found_text(found: &Found) -> (String, &'static str) {
    if found.launcher == Launcher::Raycast {
        return match &found.export {
            Some(path) => {
                let name = path.file_name().map(|name| name.to_string_lossy().into_owned()).unwrap_or_default();
                let folder = path.parent().and_then(|folder| folder.file_name()).map(|name| name.to_string_lossy().into_owned()).unwrap_or_default();
                (format!("Found {name} in {folder}. Import it, or choose another export."), "Import")
            }
            None => ("Raycast keeps its data locked. Run Export Settings & Data in Raycast, then choose the file it saves.".to_string(), "Choose file\u{2026}"),
        };
    }
    let mut text = things(&found.haul);
    if !found.haul.left_out.is_empty() {
        text.push_str(&format!(". Orbit has no place for its {}", orbit_core::import::join_list(&found.haul.left_out)));
    }
    text.push('.');
    (text, "Import")
}

struct Parts {
    sign_in: HWND,
    desktop: HWND,
    step_text: HWND,
    rows: Vec<Row>,
    pass_controls: Vec<HWND>,
    pass_caption: HWND,
    pass_edit: HWND,
    hotkey: HWND,
    hotkey_note: HWND,
    caps_top: [i32; 2],
    files_on: HWND,
    everything_note: HWND,
    everything_button: HWND,
    tools: [ToolRow; 2],
    done_note: HWND,
    remove_data: HWND,
    removed_note: HWND,
}

/// A row on a card: a name and a line or two on the left, a button on the right.
fn action_row(b: &mut Builder, title: &str, text: &str, label: &str, id: i32, lines: i32) -> (HWND, HWND) {
    let full = b.width;
    let button = b.px(BUTTON_DIP);
    let top = b.y;
    b.width = full - button - b.px(16);
    b.y += b.px(4);
    b.heading(title);
    b.y -= b.px(10);
    let note = b.note_area(text, lines);
    b.width = full;
    let bottom = b.y - b.px(6);
    let hwnd = b.button_at(label, id, b.x + full - button, top + (bottom - top - b.px(30)) / 2, button, ButtonKind::Normal);
    (note, hwnd)
}

fn build_pages(b: &mut Builder, s: &BuildInput) -> (Vec<(Page, Vec<HWND>)>, Parts, i32) {
    let top = b.y;
    let mut pages = Vec::new();
    let mut bottom = 0;
    let mut parts = Parts {
        sign_in: HWND::default(),
        desktop: HWND::default(),
        step_text: HWND::default(),
        rows: Vec::new(),
        pass_controls: Vec::new(),
        pass_caption: HWND::default(),
        pass_edit: HWND::default(),
        hotkey: HWND::default(),
        hotkey_note: HWND::default(),
        caps_top: [0, 0],
        files_on: HWND::default(),
        everything_note: HWND::default(),
        everything_button: HWND::default(),
        tools: [ToolRow::default(); 2],
        done_note: HWND::default(),
        remove_data: HWND::default(),
        removed_note: HWND::default(),
    };
    let mut finish = |b: &mut Builder, page: Page, pages: &mut Vec<(Page, Vec<HWND>)>| {
        bottom = bottom.max(b.y);
        pages.push((page, b.take_controls()));
        b.y = top;
    };
    for page in &s.sequence {
        match page {
            Page::Welcome => {
                let update = s.mode == Mode::Install && s.existing.version.is_some();
                if update {
                    let old = s.existing.version.as_deref().unwrap_or("");
                    b.title(if install::compare_versions(old, install::VERSION).is_lt() { "Update Orbit" } else { "Reinstall Orbit" });
                    b.note(&format!("Orbit {old} is installed. This puts in Orbit {}, and keeps your settings and data as they are.", install::VERSION));
                } else {
                    b.title("Welcome to Orbit");
                    b.note("Orbit opens apps, files, settings and the web from one box, and does the rest with a few keys.");
                }
                b.begin_card();
                b.heading("Open anything");
                b.y -= b.px(8);
                b.note("Type a few letters of an app, a file, a setting or a site, then press Enter.");
                b.line();
                b.heading("Do more with Ctrl+K");
                b.y -= b.px(8);
                b.note("Every result has more you can do with it: open it as admin, copy its path, show it in Explorer.");
                b.line();
                b.heading("Kept on this PC");
                b.y -= b.px(8);
                b.note("Your notes, snippets and settings stay here. Orbit only goes online when you ask it to.");
                b.end_card();
                if s.mode == Mode::Install {
                    let choices = install::keep_choices(&s.existing);
                    b.begin_card();
                    parts.sign_in = b.switch("Start Orbit when I sign in", choices.start_at_sign_in, ID_SIGN_IN);
                    b.note("So it's ready the moment you want it.");
                    parts.desktop = b.switch("Put a shortcut on the desktop", choices.desktop_shortcut, ID_DESKTOP);
                    b.end_card();
                    b.note(&format!("Orbit installs for you alone, in {}.", install::install_dir().display()));
                }
                finish(b, Page::Welcome, &mut pages);
            }
            Page::Installing => {
                b.title(if s.existing.version.is_some() { "Updating Orbit" } else { "Installing Orbit" });
                b.note("This takes a few seconds.");
                b.gap(28);
                parts.caps_top[0] = b.y;
                b.gap(26);
                parts.step_text = b.note_area(install::Step::Closing.label(), 3);
                finish(b, Page::Installing, &mut pages);
            }
            Page::Import => {
                b.title("Bring your things over");
                b.note("Quicklinks, web searches, snippets and notes from the launcher you used before. The other launcher stays as it is.");
                let rows: Vec<usize> = (0..s.found.len()).filter(|at| s.found[*at].launcher == Launcher::Raycast || s.found[*at].has_things()).collect();
                if !rows.is_empty() {
                    b.begin_card();
                    for (place, at) in rows.iter().enumerate() {
                        if place > 0 {
                            b.line();
                        }
                        let found = &s.found[*at];
                        let (text, label) = found_text(found);
                        let id = ID_ROW + parts.rows.len() as i32;
                        let (note, button) = action_row(b, found.launcher.name(), &text, label, id, 3);
                        parts.rows.push(Row { kind: RowKind::Launcher(*at), note, button, path: found.export.clone(), label });
                    }
                    b.end_card();
                }
                b.begin_card();
                let text = if rows.is_empty() { "Orbit didn't find another launcher here. Choose an export from Raycast, or one from Orbit." } else { "An export from Raycast, Raycast's snippets or quicklinks file, or an export from Orbit." };
                let id = ID_ROW + parts.rows.len() as i32;
                let (note, button) = action_row(b, "Import a file", text, "Choose file\u{2026}", id, 2);
                parts.rows.push(Row { kind: RowKind::File, note, button, path: None, label: "Choose file\u{2026}" });
                b.end_card();
                let first = b.created.len();
                b.begin_card();
                parts.pass_caption = b.note_area("", 2);
                let full = b.width;
                let open = b.px(96);
                b.width = full - open - b.px(12);
                parts.pass_edit = b.edit("", ID_PASS);
                b.hint(parts.pass_edit, "Passphrase");
                b.width = full;
                unsafe {
                    // Shown as dots.
                    SendMessageW(parts.pass_edit, 0x00CC, Some(WPARAM(0x25CF)), None);
                }
                let mut place = RECT::default();
                unsafe {
                    let _ = GetWindowRect(parts.pass_edit, &mut place);
                    let mut origin = POINT { x: place.left, y: place.top };
                    let _ = windows::Win32::Graphics::Gdi::ScreenToClient(b.parent, &mut origin);
                    let height = place.bottom - place.top;
                    b.button_at("Open", ID_PASS_OPEN, b.x + full - open, origin.y + (height - b.px(30)) / 2, open, ButtonKind::Normal);
                }
                b.end_card();
                parts.pass_controls = b.created[first..].to_vec();
                finish(b, Page::Import, &mut pages);
            }
            Page::Shortcut => {
                b.title("Choose your shortcut");
                b.note("Press it anywhere, in any app, and Orbit opens.");
                b.gap(14);
                parts.caps_top[0] = b.y;
                b.gap(52 + 28);
                let choices = hotkey_choices(&s.active_hotkey, &s.found);
                let items: Vec<String> = choices.iter().map(|chord| chord.display("Win")).collect();
                let names: Vec<&str> = items.iter().map(String::as_str).collect();
                let selected = items.iter().position(|item| *item == s.active_hotkey).unwrap_or(0);
                b.begin_card();
                parts.hotkey = b.drop_down("Shortcut", &names, selected, ID_HOTKEY);
                parts.hotkey_note = b.note_area("", 2);
                b.end_card();
                b.note("Any other shortcut can be recorded in Settings \u{2192} Shortcuts, where Caps Lock can become a Hyper key too.");
                finish(b, Page::Shortcut, &mut pages);
            }
            Page::Extensions => {
                b.title("Add extensions");
                b.note("Orbit works with these apps. Turn on the ones you use; Settings has these and more.");
                b.begin_card();
                let (text, label, _) = crate::settings_win::everything_row();
                let (note, button) = action_row(b, "Everything", &text, label, ID_EVERYTHING, 2);
                parts.everything_note = note;
                parts.everything_button = button;
                let typed = if s.files_keyword.is_empty() { "everything" } else { &s.files_keyword };
                parts.files_on = b.switch(&format!("Search files: type \u{201C}{typed}\u{201D} in Orbit"), s.everything_on, ID_FILES_ON);
                for (slot, tool) in Tool::ALL.into_iter().enumerate() {
                    b.line();
                    let (text, label, _) = tool_row(tool, integrations::find(tool).is_some(), false, None);
                    let (note, button) = action_row(b, tool.name(), &text, label, ID_TOOL_BUTTON[slot], 2);
                    let switch = b.switch(if tool == Tool::OnePassword { "Search passwords and codes" } else { "Search two-factor codes" }, s.tools_on[slot], ID_TOOL_ON[slot]);
                    parts.tools[slot] = ToolRow { note, button, switch };
                }
                b.end_card();
                finish(b, Page::Extensions, &mut pages);
            }
            Page::Done => {
                b.title(if s.mode == Mode::Install && s.existing.version.is_some() { "Orbit is up to date" } else { "You're all set" });
                if s.mode == Mode::Install {
                    parts.done_note = b.note_area("", 3);
                } else {
                    b.gap(14);
                    parts.caps_top[1] = b.y;
                    b.gap(52 + 28);
                    parts.done_note = b.note_area("", 3);
                }
                finish(b, Page::Done, &mut pages);
            }
            Page::Uninstall => {
                b.title("Uninstall Orbit");
                b.note("This removes Orbit from this PC, with its Start menu and desktop shortcuts, and stops it starting when you sign in.");
                b.begin_card();
                parts.remove_data = b.switch("Also delete my settings and data", false, ID_REMOVE_DATA);
                b.note(&format!("Notes, snippets, clipboard history and settings, in {}. Leave this off to keep them for next time.", install::data_dir().display()));
                b.end_card();
                finish(b, Page::Uninstall, &mut pages);
            }
            Page::Removing => {
                b.title("Uninstalling Orbit");
                b.note("This takes a few seconds.");
                b.gap(28);
                parts.caps_top[0] = b.y;
                b.gap(26);
                parts.step_text = b.note_area(install::Step::Closing.label(), 3);
                finish(b, Page::Removing, &mut pages);
            }
            Page::Removed => {
                b.title("Orbit is uninstalled");
                parts.removed_note = b.note_area("", 3);
                finish(b, Page::Removed, &mut pages);
            }
        }
    }
    (pages, parts, bottom)
}

struct BuildInput {
    mode: Mode,
    sequence: Vec<Page>,
    existing: install::Existing,
    found: Vec<Found>,
    active_hotkey: String,
    everything_on: bool,
    files_keyword: String,
    tools_on: [bool; 2],
}

fn open_window(mode: Mode, theme: Theme, main: Option<SendHwnd>, app: Option<&InApp>) -> Option<HWND> {
    if let Some(hwnd) = OPEN.with(|open| open.get()) {
        if let Some(s) = state(hwnd) {
            forms::show_form(hwnd, s.next);
        }
        return None;
    }
    let stand_in = PREVIEW_EXISTING.with(|existing| existing.borrow().clone());
    let existing = match stand_in {
        Some(existing) => existing,
        None if matches!(mode, Mode::Install | Mode::Uninstall) => install::existing(),
        None => install::Existing::default(),
    };
    let (sequence, steps) = plan(mode, &existing);
    let stand_ins = PREVIEW_FOUND.with(|found| found.borrow().clone());
    let found = if sequence.contains(&Page::Import) { stand_ins.unwrap_or_else(importer::detect) } else { Vec::new() };
    let title = match mode {
        Mode::Install => w!("Orbit Setup"),
        Mode::Welcome { .. } => w!("Welcome to Orbit"),
        Mode::Import => w!("Import - Orbit"),
        Mode::Uninstall => w!("Uninstall Orbit"),
    };
    unsafe {
        let module = GetModuleHandleW(None).ok()?;
        let class = WNDCLASSEXW {
            cbSize: std::mem::size_of::<WNDCLASSEXW>() as u32,
            lpfnWndProc: Some(setup_proc),
            hInstance: module.into(),
            hCursor: LoadCursorW(None, IDC_ARROW).unwrap_or_default(),
            lpszClassName: CLASS,
            hIcon: crate::tray::load_app_icon(32, 32),
            hIconSm: crate::tray::load_app_icon(16, 16),
            ..Default::default()
        };
        RegisterClassExW(&class);
        let dpi = GetDpiForSystem();
        let px = |value: i32| forms::scale(value, dpi);
        let width = px(WIDTH_DIP);
        let hwnd = CreateWindowExW(
            forms::form_ex_style(),
            CLASS,
            title,
            WS_OVERLAPPED | WS_CAPTION | WS_SYSMENU | WS_MINIMIZEBOX | WS_CLIPCHILDREN,
            CW_USEDEFAULT,
            CW_USEDEFAULT,
            width,
            px(MIN_HEIGHT_DIP),
            None,
            None,
            None,
            None,
        )
        .ok()?;
        OPEN.with(|open| open.set(Some(hwnd)));
        let mut style = Box::new(Style::new(theme, dpi));
        style.apply_to_window(hwnd);
        let content_x = px(RAIL_DIP + SIDE_DIP);
        let content_width = width - content_x - px(SIDE_DIP);
        let input = BuildInput {
            mode,
            sequence: sequence.clone(),
            existing: existing.clone(),
            found: found.clone(),
            active_hotkey: app.map(|app| app.active_hotkey.clone()).unwrap_or_default(),
            everything_on: app.is_some_and(|app| app.everything_on),
            files_keyword: app.map(|app| app.files_keyword.clone()).unwrap_or_default(),
            tools_on: app.map(|app| app.tools_on).unwrap_or_default(),
        };
        let mut b = Builder::new(hwnd, &mut style, content_x, px(TOP_DIP), content_width);
        let (pages, parts, bottom) = build_pages(&mut b, &input);
        let height = (bottom + px(24) + px(FOOTER_DIP)).max(px(MIN_HEIGHT_DIP));
        let row = height - px(FOOTER_DIP) + (px(FOOTER_DIP) - px(30)) / 2;
        let button = px(BUTTON_DIP);
        let right = width - px(SIDE_DIP);
        let back = b.button_at("Back", ID_BACK, right - button * 2 - px(10), row, button, ButtonKind::Normal);
        let next = b.button_at("Next", ID_NEXT, right - button, row, button, ButtonKind::Primary);
        let status = b.status_at(content_x, row, content_width - button * 2 - px(26), px(34));
        b.take_controls();
        drop(b);
        forms::fit_client(hwnd, width, height);
        place_window(hwnd, app.and_then(|app| app.at));
        let logo_size = px(LOGO_DIP);
        let icon = crate::tray::load_app_icon(logo_size, logo_size);
        let logo = Backbuffer::icon_pixels(icon, logo_size);
        let _ = DestroyIcon(icon);
        let choices = hotkey_choices(&input.active_hotkey, &found);
        let s = Box::new(State {
            mode,
            main,
            style,
            dpi,
            width,
            height,
            pages,
            sequence,
            at: 0,
            steps,
            back,
            next,
            status,
            was_running: mode == Mode::Install && crate::instance::find(&install::data_dir()).is_some(),
            existing,
            sign_in: parts.sign_in,
            desktop: parts.desktop,
            installed: false,
            handover_since: None,
            step_text: parts.step_text,
            progress: 0.0,
            shown_progress: 0.0,
            busy: false,
            found,
            rows: parts.rows,
            pass_controls: parts.pass_controls,
            pass_caption: parts.pass_caption,
            pass_edit: parts.pass_edit,
            pass_for: None,
            orbit_file_row: None,
            active_hotkey: input.active_hotkey,
            choices,
            hotkey: parts.hotkey,
            hotkey_note: parts.hotkey_note,
            caps_top: parts.caps_top,
            files_on: parts.files_on,
            everything_note: parts.everything_note,
            everything_button: parts.everything_button,
            tools: parts.tools,
            tools_busy: [false; 2],
            ente_account: None,
            done_note: parts.done_note,
            remove_data: parts.remove_data,
            removed_note: parts.removed_note,
            spin_from: Instant::now(),
            logo,
        });
        let first = s.sequence[0];
        let raw = Box::into_raw(s);
        SetWindowLongPtrW(hwnd, GWLP_USERDATA, raw as isize);
        let s = &mut *raw;
        s.show_page(hwnd, first);
        if main.is_some() {
            crate::editor::register_form(hwnd);
        }
        forms::show_form(hwnd, s.next);
        s.focus_first();
        Some(hwnd)
    }
}

/// In the middle of the screen with the pointer, or where Setup's window was.
fn place_window(hwnd: HWND, at: Option<(i32, i32)>) {
    unsafe {
        if forms::is_preview() {
            let (x, y) = forms::form_origin();
            let _ = SetWindowPos(hwnd, None, x, y, 0, 0, SWP_NOSIZE | SWP_NOZORDER | SWP_NOACTIVATE);
            return;
        }
        let mut rect = RECT::default();
        let _ = GetWindowRect(hwnd, &mut rect);
        let (width, height) = (rect.right - rect.left, rect.bottom - rect.top);
        let (x, y) = match at {
            Some(at) => at,
            None => {
                let mut point = POINT::default();
                let _ = windows::Win32::UI::WindowsAndMessaging::GetCursorPos(&mut point);
                let mut info = MONITORINFO { cbSize: std::mem::size_of::<MONITORINFO>() as u32, ..Default::default() };
                if !GetMonitorInfoW(MonitorFromPoint(point, MONITOR_DEFAULTTONEAREST), &mut info).as_bool() {
                    return;
                }
                let work = info.rcWork;
                (work.left + (work.right - work.left - width) / 2, work.top + (work.bottom - work.top - height) / 2)
            }
        };
        let _ = SetWindowPos(hwnd, None, x, y, 0, 0, SWP_NOSIZE | SWP_NOZORDER | SWP_NOACTIVATE);
    }
}

/// Whether this exe is Setup: named like `Orbit-1.1.0-Setup.exe`, as the release names it. A
/// browser's `Orbit-1.1.0-Setup (1).exe` counts too.
pub fn named_as_setup() -> bool {
    std::env::current_exe().ok().and_then(|exe| exe.file_stem().map(|stem| stem.to_string_lossy().to_lowercase())).is_some_and(|stem| stem.contains("setup"))
}

/// One Setup or uninstaller at a time. A second one brings the first forward.
fn first_setup() -> bool {
    use windows::Win32::System::Threading::CreateMutexW;
    unsafe {
        let mutex = CreateMutexW(None, true, w!("Local\\OrbitSetup"));
        if GetLastError() == ERROR_ALREADY_EXISTS {
            if let Ok(window) = FindWindowW(CLASS, PCWSTR::null()) {
                let _ = ShowWindow(window, SW_RESTORE);
                let _ = SetForegroundWindow(window);
            }
            return false;
        }
        // Held until the process ends.
        std::mem::forget(mutex);
        true
    }
}

/// `Orbit-<version>-Setup.exe`. Silent keeps what was chosen before, and starts Orbit again if
/// it was running.
pub fn run_install(silent: bool) -> i32 {
    if !first_setup() {
        return 0;
    }
    if !silent {
        return run_standalone(Mode::Install);
    }
    com_for_this_thread();
    let existing = install::existing();
    let was_running = crate::instance::find(&install::data_dir()).is_some();
    // Run by Orbit's updater, or by hand over an older version: the new one says it's new.
    let newer = existing.version.as_deref().is_some_and(|old| install::compare_versions(old, install::VERSION).is_lt());
    match install::install(install::keep_choices(&existing), |_| {}) {
        Ok(()) => {
            if was_running {
                install::launch_installed(false, newer);
            }
            0
        }
        Err(_) => 1,
    }
}

/// `orbit.exe --uninstall`, as Settings → Apps runs it. The installed exe can't delete itself,
/// so it starts a copy of itself from the temp folder, which does the work and then goes too.
pub fn run_uninstall(silent: bool) -> i32 {
    if install::running_installed() {
        let Some(copy) = install::copy_to_temp() else { return 1 };
        let mut command = std::process::Command::new(&copy);
        command.arg("--uninstall").current_dir(std::env::temp_dir());
        if silent {
            command.arg("--silent");
        }
        return match command.spawn() {
            Ok(mut child) if silent => child.wait().map_or(1, |status| status.code().unwrap_or(1)),
            Ok(_) => 0,
            Err(_) => 1,
        };
    }
    if !first_setup() {
        return 0;
    }
    let code = if silent {
        com_for_this_thread();
        // Silent keeps the settings and data: deleting them takes someone saying so.
        if install::uninstall(false, |_| {}).is_ok() { 0 } else { 1 }
    } else {
        run_standalone(Mode::Uninstall)
    };
    install::delete_self_later();
    code
}

/// Setup.exe and the uninstaller: the window alone, with its own message loop. Gives the
/// process's exit code.
fn run_standalone(mode: Mode) -> i32 {
    STANDALONE.with(|standalone| standalone.set(true));
    com_for_this_thread();
    let theme = crate::ui::resolve_theme(orbit_core::settings::Appearance::System);
    let Some(hwnd) = open_window(mode, theme, None, None) else { return 1 };
    let mut message = MSG::default();
    unsafe {
        while GetMessageW(&mut message, None, 0, 0).as_bool() {
            if IsWindow(Some(hwnd)).as_bool() && IsDialogMessageW(hwnd, &message).as_bool() {
                continue;
            }
            let _ = TranslateMessage(&message);
            DispatchMessageW(&message);
        }
    }
    0
}

/// Opens the window inside Orbit: the welcome, or Import alone.
pub fn open_in_app(mode: Mode, app: InApp) {
    let (main, theme) = (app.main, app.theme);
    open_window(mode, theme, Some(main), Some(&app));
}

/// The launcher brought something over for a row on the Import page.
pub fn imported(row: usize, text: &str, ok: bool) {
    if let Some(s) = current() {
        s.finish_row(row, text, ok);
    }
}

/// The launcher read an Orbit export or settings file that the Import page chose.
pub fn orbit_file_imported(text: &str, ok: bool) {
    if let Some(s) = current() {
        if let Some(row) = s.orbit_file_row.take() {
            s.finish_row(row, text, ok);
        }
    }
}

/// The launcher's shortcut is now this, as shown, or empty if it couldn't be registered.
pub fn hotkey_changed(active: &str) {
    if let Some(s) = current() {
        s.active_hotkey = active.to_string();
        if s.page() == Page::Done {
            s.update_done_note();
            if let Some(hwnd) = OPEN.with(|open| open.get()) {
                s.invalidate(hwnd);
            }
        }
    }
}

/// Everything was installed, or couldn't be.
pub fn everything_changed() {
    if let Some(s) = current() {
        s.refresh_everything();
    }
}

/// Launchers for the developer preview, so the Import page looks as it would on a busy PC.
fn stand_in_found() -> Vec<Found> {
    use orbit_core::import::Link;
    let link = |name: &str, url: &str, alias: &str| Link { name: name.into(), url: url.into(), alias: alias.into(), open_with: String::new() };
    vec![
        Found { launcher: Launcher::Raycast, haul: Haul::default(), export: Some(PathBuf::from(r"C:\Users\You\Downloads\Raycast 2026-11-09.rayconfig")) },
        Found {
            launcher: Launcher::FlowLauncher,
            haul: Haul {
                hotkey: Chord::parse("Alt+Space").ok(),
                links: vec![link("Google", "https://www.google.com/search?q={query}", "g"), link("YouTube", "https://www.youtube.com/results?search_query={query}", "yt"), link("Work", r"C:\Work", "")],
                left_out: vec!["2 query shortcuts".into()],
                ..Haul::default()
            },
            export: None,
        },
        Found { launcher: Launcher::PowerToysRun, haul: Haul { hotkey: Chord::parse("Ctrl+Space").ok(), ..Haul::default() }, export: None },
    ]
}

/// For the developer preview: opens one page in one state, off screen and unable to take focus.
/// See `settings_preview.rs`. `what` is `welcome-install`, `installing`, `welcome`, `import`,
/// `import-passphrase`, `import-done`, `import-alone`, `shortcut`, `extensions`, `done`, `updated`,
/// `uninstall` or `removing`.
pub fn preview_open(what: &str, theme: Theme) -> bool {
    PREVIEW_FOUND.with(|found| *found.borrow_mut() = Some(stand_in_found()));
    let app = InApp { main: SendHwnd::new(HWND::default()), theme, active_hotkey: "Alt+Space".into(), everything_on: true, files_keyword: String::new(), tools_on: [false; 2], at: None };
    let welcome = Mode::Welcome { installed: true };
    if what == "updated" {
        PREVIEW_EXISTING.with(|existing| *existing.borrow_mut() = Some(install::Existing { version: Some("1.0.0".into()), starts_at_sign_in: true, desktop_shortcut: false, by_inno: true }));
    }
    let (mode, page, in_app) = match what {
        "welcome-install" => (Mode::Install, Page::Welcome, false),
        "installing" => (Mode::Install, Page::Installing, false),
        "updated" => (Mode::Install, Page::Done, false),
        "welcome" => (Mode::Welcome { installed: false }, Page::Welcome, true),
        "import" | "import-passphrase" | "import-done" => (welcome, Page::Import, true),
        "import-alone" => (Mode::Import, Page::Import, true),
        "shortcut" => (welcome, Page::Shortcut, true),
        "extensions" | "files" => (welcome, Page::Extensions, true),
        "done" => (welcome, Page::Done, true),
        "uninstall" => (Mode::Uninstall, Page::Uninstall, false),
        "removing" => (Mode::Uninstall, Page::Removing, false),
        _ => return false,
    };
    let Some(hwnd) = open_window(mode, theme, in_app.then_some(app.main), in_app.then_some(&app)) else { return false };
    let Some(s) = state(hwnd) else { return false };
    // Nothing is listening in a preview.
    s.main = None;
    if page == Page::Done && mode == Mode::Install {
        s.installed = true;
    }
    s.show_page(hwnd, page);
    match what {
        "installing" | "removing" => {
            s.busy = true;
            s.progress = 0.45;
            s.shown_progress = 0.45;
            let step = if what == "installing" { install::Step::Copying.label() } else { "Deleting its files\u{2026}" };
            forms::set_status(s.step_text, step);
            s.update_footer();
        }
        "import-passphrase" => {
            let path = PathBuf::from(r"C:\Users\You\Downloads\Raycast 2026-11-09.rayconfig");
            s.on_read(hwnd, Read { row: 0, path, result: Err(ReadError::NeedsPassphrase) });
        }
        "import-done" => {
            s.finish_row(0, "Added 24 quicklinks, 11 snippets and 3 notes. 2 were already in Orbit. Left behind: 6 extensions from Raycast's store.", true);
            s.finish_row(1, "Added 3 quicklinks. Left behind: 2 query shortcuts.", true);
        }
        _ => {}
    }
    // The logo at rest, as it is once its moon has gone round.
    s.spin_from = Instant::now() - Duration::from_secs(5);
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_shortcut_is_drawn_as_its_keys() {
        assert_eq!(keys_of("Ctrl+Alt+Space"), vec!["Ctrl", "Alt", "Space"]);
        assert!(keys_of("").is_empty());
    }

    #[test]
    fn shortcuts_on_offer_start_with_orbits_own() {
        let found = vec![Found { launcher: Launcher::Keypirinha, haul: Haul { hotkey: Chord::parse("Ctrl+Win+K").ok(), ..Haul::default() }, export: None }];
        let choices: Vec<String> = hotkey_choices("Ctrl+Shift+Space", &found).iter().map(|chord| chord.display("Win")).collect();
        assert_eq!(choices[0], "Ctrl+Shift+Space");
        assert_eq!(choices.iter().filter(|choice| *choice == "Ctrl+Shift+Space").count(), 1, "no repeats");
        assert!(choices.contains(&"Alt+Space".to_string()));
        assert_eq!(choices.last().map(String::as_str), Some("Ctrl+Win+K"));
    }

    #[test]
    fn each_mode_goes_through_its_pages() {
        let fresh = install::Existing::default();
        let (sequence, _) = plan(Mode::Welcome { installed: true }, &fresh);
        assert_eq!(sequence, vec![Page::Import, Page::Shortcut, Page::Extensions, Page::Done]);
        let (sequence, steps) = plan(Mode::Welcome { installed: false }, &fresh);
        assert_eq!(sequence[0], Page::Welcome);
        assert_eq!(steps.len(), 5);
        let (sequence, _) = plan(Mode::Import, &fresh);
        assert_eq!(sequence, vec![Page::Import]);
        let (sequence, _) = plan(Mode::Uninstall, &fresh);
        assert_eq!(sequence, vec![Page::Uninstall, Page::Removing, Page::Removed]);
    }

    #[test]
    fn tool_rows_install_then_sign_in() {
        let (text, label, can_press) = tool_row(Tool::OnePassword, false, false, None);
        assert_eq!((label, can_press), ("Install", true));
        assert!(text.contains("1Password's terms"), "{text}");
        let (text, label, can_press) = tool_row(Tool::EnteAuth, false, false, None);
        assert_eq!((label, can_press), ("Install", true));
        assert!(text.contains("Ente's command-line tool") && !text.contains("terms"), "{text}");
        assert_eq!(tool_row(Tool::EnteAuth, false, true, None).2, false, "no second install while one runs");
        assert_eq!(tool_row(Tool::OnePassword, true, false, None).1, "Installed", "1Password needs nothing more from Orbit");
        assert!(!tool_row(Tool::OnePassword, true, false, None).2);
        assert_eq!(tool_row(Tool::EnteAuth, true, false, Some(&EnteAccount::SignedOut)).1, "Sign in\u{2026}");
        let (text, label, _) = tool_row(Tool::EnteAuth, true, false, Some(&EnteAccount::SignedIn("me@example.com".into())));
        assert_eq!((text.as_str(), label), ("Signed in as me@example.com.", "Sign in again\u{2026}"));
    }

    #[test]
    fn a_raycast_row_says_how_to_export() {
        let found = Found { launcher: Launcher::Raycast, haul: Haul::default(), export: None };
        let (text, label) = found_text(&found);
        assert!(text.contains("Export Settings & Data"), "{text}");
        assert_eq!(label, "Choose file\u{2026}");
        let flow = Found {
            launcher: Launcher::FlowLauncher,
            haul: Haul { links: vec![orbit_core::import::Link { name: "G".into(), url: "https://g.example/{query}".into(), alias: "g".into(), open_with: String::new() }], left_out: vec!["1 query shortcut".into()], hotkey: Chord::parse("Alt+Space").ok(), ..Haul::default() },
            export: None,
        };
        assert_eq!(found_text(&flow).0, "1 quicklink. Orbit has no place for its 1 query shortcut.");
    }
}
