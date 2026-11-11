//! The launcher's window, state and message handling.
//!
//! `App` lives in a thread-local `RefCell` on the UI thread. Win32 calls can re-enter the window
//! procedure while a handler is running (`SetWindowText` sends `WM_COMMAND`, hiding sends
//! `WM_ACTIVATE`). Those nested messages are queued and handled when the outer handler returns,
//! so a handler always has exclusive access to `App` and no aliasing or runtime panic is possible.

mod ext;
mod welcome;

use crate::catalog::{Catalog, Entry, app_key, glyph_for_builtin};
use crate::clipboard::{self, Capture, CaptureLimits, InsertOutcome, InsertRequest};
use crate::gfx::Backbuffer;
use crate::hook::{self, HookHandle};
use crate::icons::IconCache;
use crate::indexer::{self, icon_hash};
use crate::inline_form::{FormKind, InlineForm};
use crate::integrations::{self, Failure, Tool};
use crate::keys;
use crate::launch::{self, LaunchRequest};
use crate::metrics::{Metrics, ShowTimes};
use crate::rows::*;
use crate::tray::{self, MenuChoice, MenuState};
use crate::ui::{self, Fonts, Theme};
use windows::Win32::UI::Accessibility::{HWINEVENTHOOK, SetWinEventHook, UnhookWinEvent};
use crate::util::{self, SendHwnd, format_bytes, qpc, qpc_frequency, wide};
use crate::view::{Behind, Menu, MenuSide, SavedView, Scope, View, help_rows};
use crate::worker::{self, ClipPreview, DataRow, DataScope, Purpose, Req, Resp, Worker};
use orbit_core::action::{UrlCheck, as_openable_url, check_url, fill_url_template};
use orbit_core::links::{LinkParam, fill_link, link_params, missing_param, split_args};
use orbit_core::calc::{CalcOutcome, evaluate};
use orbit_core::hotkeys::{self, Hotkey};
use orbit_core::settings::{LoadOutcome, Settings};
use orbit_core::snippet::{TemplateContext, expand_template};
use orbit_core::{Action, Builtin, Hit, QueryScratch};
use orbit_data::{ClipKind, RetentionPolicy};
use orbit_hotkeys::chord::is_reserved_on_windows;
use orbit_hotkeys::{Chord, HyperMode, Mods};
use std::cell::RefCell;
use std::collections::{HashMap, HashSet, VecDeque};
use std::time::Instant;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicIsize, Ordering};

static CARET_PENDING: AtomicBool = AtomicBool::new(false);
/// The snippet's box in a form, which wants Enter for new lines.
static AREA_HWND: AtomicIsize = AtomicIsize::new(0);
/// A form left open when the launcher hid comes back if it's shown again this soon.
const FORM_KEPT_SECS: u64 = 300;
/// How long before links still without an icon are asked again, when the launcher opens.
const LINK_ICONS_AGAIN: std::time::Duration = std::time::Duration::from_secs(30 * 60);
use windows::Win32::Foundation::*;
use windows::Win32::Graphics::Dwm::{
    DWMWA_BORDER_COLOR, DWMWA_COLOR_NONE, DWMWA_USE_IMMERSIVE_DARK_MODE, DWMWA_WINDOW_CORNER_PREFERENCE, DWMWCP_DONOTROUND, DWMWCP_ROUND, DwmExtendFrameIntoClientArea, DwmFlush,
    DwmSetWindowAttribute,
};
use windows::Win32::Graphics::Gdi::*;
use windows::Win32::System::DataExchange::{AddClipboardFormatListener, RemoveClipboardFormatListener};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::System::Memory::SetProcessWorkingSetSizeEx;
use windows::Win32::System::Power::SetSuspendState;
use windows::Win32::System::RemoteDesktop::{NOTIFY_FOR_THIS_SESSION, WTSRegisterSessionNotification};
use windows::Win32::System::Shutdown::LockWorkStation;
use windows::Win32::System::SystemInformation::{GlobalMemoryStatusEx, MEMORYSTATUSEX};
use windows::Win32::System::Threading::{
    GetCurrentProcess, GetCurrentThread, SetThreadPriority, THREAD_PRIORITY_ABOVE_NORMAL,
};
use windows::Win32::UI::Controls::{EM_GETSEL, EM_REPLACESEL, EM_SETCUEBANNER, EM_SETSEL, EM_UNDO, MARGINS, WM_MOUSELEAVE};
use orbit_core::vim::{self, Command as VimCommand, Delete as VimDelete, InsertAt, KeyboardMode, VimKeys, VimMode};
use windows::Win32::UI::HiDpi::{GetDpiForWindow, GetDpiForSystem};
use windows::Win32::UI::Input::KeyboardAndMouse::{
    ReleaseCapture, SetCapture, TME_LEAVE, TRACKMOUSEEVENT, TrackMouseEvent, VK_DELETE, VK_MENU, VK_OEM_COMMA,
    GetKeyState, RegisterHotKey, SetFocus, UnregisterHotKey, GetFocus, MOD_ALT, MOD_CONTROL, MOD_NOREPEAT, MOD_SHIFT,
    MOD_WIN, VK_BACK, VK_CONTROL, VK_DOWN, VK_ESCAPE, VK_F4, VK_LEFT, VK_NEXT, VK_PRIOR, VK_RETURN, VK_RIGHT, VK_TAB, VK_UP,
};
use windows::Win32::UI::Shell::{SHEmptyRecycleBinW, SHERB_NOSOUND};
use windows::Win32::UI::WindowsAndMessaging::*;
use windows::core::{BOOL, PCWSTR, w};

pub const WM_ORBIT_TRAY: u32 = WM_APP + 1;
pub const WM_ORBIT_DATA: u32 = WM_APP + 2;
pub const WM_ORBIT_HYPER: u32 = WM_APP + 3;
pub const WM_ORBIT_EXPAND: u32 = WM_APP + 4;
pub const WM_ORBIT_INDEX_DONE: u32 = WM_APP + 5;
pub const WM_ORBIT_INDEX_DIRTY: u32 = WM_APP + 6;
pub const WM_ORBIT_TOGGLE: u32 = WM_APP + 7;
pub const WM_ORBIT_LAUNCH_FAILED: u32 = WM_APP + 8;
/// Something done on a thread of its own has finished. The message comes as text.
pub const WM_ORBIT_REPORT: u32 = WM_APP + 44;
pub const WM_ORBIT_INSERT_DONE: u32 = WM_APP + 9;
/// A password or code was typed, or put on the clipboard instead: wparam says which, lparam is the
/// clipboard's sequence number then.
const WM_ORBIT_SECRET_TYPED: u32 = WM_APP + 54;
pub const WM_ORBIT_SETTINGS_APPLY: u32 = WM_APP + 10;
pub const WM_ORBIT_RUN_BUILTIN: u32 = WM_APP + 11;

/// Settings is listening for keys, or has stopped, in wparam. Orbit's own shortcuts stand aside.
pub const WM_ORBIT_SETTINGS_RECORDING: u32 = WM_APP + 31;
/// Settings has moved to a screen with a different DPI, and needs making again.
pub const WM_ORBIT_SETTINGS_REOPEN: u32 = WM_APP + 32;
/// A tool has finished installing or being removed. Holds a boxed `integrations::ToolDone`. It comes
/// here because the Settings window that asked may have been rebuilt or closed since.
pub const WM_ORBIT_SETTINGS_TOOL_DONE: u32 = WM_APP + 33;
/// Ente's tool said who's signed in. Holds a boxed `integrations::EnteAccount`.
pub const WM_ORBIT_ENTE_ACCOUNT: u32 = WM_APP + 35;
/// Settings turned one item on or off in search: a boxed `(key, hidden)`.
pub const WM_ORBIT_SET_HIDDEN: u32 = WM_APP + 37;
/// Settings gave one item an alias, or took it away: a boxed `(key, alias)`.
pub const WM_ORBIT_SET_ALIAS: u32 = WM_APP + 38;
/// An editor closed: give its memory back shortly.
pub const WM_ORBIT_RECLAIM: u32 = WM_APP + 13;
/// The search box's caret or selection may have moved.
const WM_ORBIT_CARET: u32 = WM_APP + 14;
/// A command-line tool has answered. Holds a boxed `integrations::Answer`.
const WM_ORBIT_CLI: u32 = WM_APP + 16;
/// A quicklink or command form finished. Holds a boxed `link_form::LinkSaved`.
pub const WM_ORBIT_LINK_SAVED: u32 = WM_APP + 17;
/// Settings wants a link or a command added, changed or removed. Holds a boxed `link_form::LinkRequest`.
pub const WM_ORBIT_LINK_REQUEST: u32 = WM_APP + 18;
/// Another program came to the front, so the launcher's hotkey may have to step aside.
const WM_ORBIT_FOREGROUND: u32 = WM_APP + 30;
/// Quicklink icons have arrived from their sites.
const WM_ORBIT_LINK_ICONS: u32 = WM_APP + 34;
/// Exchange rates were fetched: wparam is 1 if they were saved.
const WM_ORBIT_RATES: u32 = WM_APP + 39;
/// A weather lookup finished. Its answer is waiting in `weather.rs`.
const WM_ORBIT_WEATHER: u32 = WM_APP + 41;
/// The speed test printed more, or ended.
const WM_ORBIT_SPEEDTEST: u32 = WM_APP + 42;
/// Speedtest CLI finished installing, or couldn't.
const WM_ORBIT_SPEEDTEST_INSTALLED: u32 = WM_APP + 43;
/// The colour picker has finished: wParam is 0x01RRGGBB, or 0 if nothing was picked.
const WM_ORBIT_COLOUR: u32 = WM_APP + 45;
/// The files thread answered: boxed `files::FileAnswer`.
const WM_ORBIT_FILES: u32 = WM_APP + 46;
/// An extension said something, or stopped: boxed `extensions::Event`.
const WM_ORBIT_EXTENSION: u32 = WM_APP + 47;
/// Installing Everything finished: boxed `Result<(), String>`.
pub const WM_ORBIT_EVERYTHING_INSTALLED: u32 = WM_APP + 48;
/// Settings added or removed an extension, so the folder is looked at again.
pub const WM_ORBIT_EXTENSIONS_CHANGED: u32 = WM_APP + 49;
/// The updater's worker found something: boxed `update::Event`.
pub const WM_ORBIT_UPDATE: u32 = WM_APP + 53;
/// What a new quicklink could be, read from the window that was in front.
const WM_ORBIT_LINK_SUGGESTION: u32 = WM_APP + 36;
/// For the bench driver: write out pending timing lines.
pub const WM_ORBIT_BENCH_FLUSH: u32 = WM_APP + 20;
/// For the bench driver: empty the working set now, to measure the paged-out path.
pub const WM_ORBIT_BENCH_TRIM: u32 = WM_APP + 21;

const HOTKEY_LAUNCHER: i32 = 1;
const HOTKEY_SHORTCUT_BASE: i32 = 100;
const HOTKEY_HYPER_BASE: i32 = 1000;

const TIMER_TRIM: usize = 1;
const TIMER_EXIT: usize = 2;
const TIMER_RECLAIM: usize = 3;
const TIMER_CARET: usize = 4;
const EM_SETMARGINS: u32 = 0xD3;
const TIMER_PREVIEW: usize = 5;
const TIMER_CLEAR_SECRET: usize = 6;
const TIMER_FORGET_VAULTS: usize = 7;
/// When the timer ends. Set once for the moment it's due, not ticking.
const TIMER_COUNTDOWN: usize = 8;
/// Once a second, only while a running timer or stopwatch is on screen.
const TIMER_CLOCK: usize = 9;
/// Stops extensions' programs that have been idle a while.
const TIMER_EXT_IDLE: usize = 10;
/// Looks again while Everything starts up or reads the drives.
const TIMER_FILES_RETRY: usize = 11;
/// The updater's first look, then its hourly tick. See `update.rs`.
pub const TIMER_UPDATE_CHECK: usize = 12;
/// Setup was started to update Orbit, and should have closed it by now.
pub const TIMER_UPDATE_SETUP: usize = 13;
/// How long a copied password or code stays on the clipboard.
const SECRET_CLEAR_MS: u32 = 30_000;
/// How long the names in a password manager stay in memory after being fetched.
const VAULT_KEEP_MS: u32 = 10 * 60_000;
/// If an answer takes longer than this, the user has probably moved on: don't take focus back.
const HOLD_RETURN_SECS: u64 = 20;

const ROW_LIMIT: usize = 30;

pub const MAIN_CLASS: PCWSTR = w!("OrbitLauncher");

const DEFAULT_HOTKEYS: &[&str] = &["Alt+Space", "Ctrl+Alt+Space", "Ctrl+Shift+Space", "Ctrl+Alt+O"];

/// A frame to draw to a file, for checking the look without opening anything.
pub struct Preview {
    pub out: PathBuf,
    pub query: String,
    pub actions: bool,
    /// Which list to draw, instead of the main one.
    pub scope: Option<String>,
    /// Which row to select before drawing.
    pub select: usize,
    pub step: u32,
}

pub struct Options {
    pub backdrop: Option<ui::Backdrop>,
    pub preview: Option<Preview>,
    pub data_dir: PathBuf,
    pub bench_log: Option<PathBuf>,
    pub hotkey_override: Option<Chord>,
    /// Empty the working set this long after hiding. `None` keeps pages resident.
    pub trim_after_ms: Option<u32>,
    pub no_reindex: bool,
    /// What to open once started, besides the tray icon.
    pub start: Start,
    /// Started as a new version, just installed: say so.
    pub updated: bool,
}

/// What a start opens besides the tray icon.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Start {
    #[default]
    Quietly,
    /// `--show`: the launcher, as after an update.
    Launcher,
    /// `--setup-continue x,y`: Setup.exe installed Orbit, and the welcome carries on here, where
    /// Setup's window was.
    Welcome { at: Option<(i32, i32)> },
}

/// A quicklink's blanks, being filled in from the search bar. The box being typed in is the real
/// search box, and the others wait here.
pub struct LinkArgs {
    pub url: String,
    pub params: Vec<LinkParam>,
    pub values: Vec<String>,
    /// Which box is being typed in.
    pub at: usize,
    /// The app the link opens with, if it names one.
    pub open_with: String,
}

/// Listening for the hotkey an item should get. The key the user pressed waits in `pending` if
/// it already runs something else.
struct Recording {
    action: Action,
    title: String,
    pending: Option<Hotkey>,
}

pub struct App {
    pub main: HWND,
    pub edit: HWND,
    pub send: SendHwnd,
    pub dir: PathBuf,
    pub settings: Settings,
    settings_read_only: bool,
    /// The last time settings were written, it didn't work.
    save_failed: bool,
    /// Settings is listening for keys.
    settings_recording: bool,
    pub catalog: Catalog,
    scratch: QueryScratch,
    hits: Vec<Hit>,
    pub worker: Worker,
    pub view: View,
    pub status: Option<String>,
    pub theme: Theme,
    pub dpi: u32,
    pub fonts: Option<Fonts>,
    /// How tall the window is now, which changes with the results.
    window_height: i32,
    pub backbuffer: Option<Backbuffer>,
    /// The finished picture: the launcher with rounded corners, inside its shadow.
    pub frame: Option<Backbuffer>,
    pub frame_shadow: Option<crate::gfx::Shadow>,
    /// A small window under the launcher, holding only the blur. It's kept apart because a window
    /// that draws its own soft corners and shadow can't have Windows blur what's behind it too.
    plate: Option<HWND>,
    plate_size: (i32, i32),
    /// The compositor's blur in the plate, in the launcher's exact shape. Without it, the plate
    /// falls back to Windows' own backdrop, a little inside the launcher's corners.
    frost: Option<crate::frost::Frost>,
    /// The small menu, when one is open.
    pub menu: Option<Menu>,
    /// Told when another program comes to the front. Only there if something depends on that.
    foreground_events: Option<HWINEVENTHOOK>,
    /// The launcher's hotkey is standing aside for the program in front.
    launcher_aside: bool,
    /// The launcher's hotkey as registered, to take it back after standing aside.
    launcher_chord: Option<Chord>,
    /// The programs Windows knows to be games, and when that list was last read.
    known_games: Vec<String>,
    known_games_at: Option<Instant>,
    /// Read when the emoji list opens and let go when it closes.
    pub emoji_table: Vec<orbit_core::emoji::Emoji>,
    pub emoji_cache: HashMap<u16, Vec<u8>>,
    pub icons: IconCache,
    /// What the clipboard's side panel shows, for the clip last asked for.
    pub clip_preview: Option<Box<ClipPreview>>,
    clip_preview_wanted: Option<i64>,
    /// Links whose sites have been asked for an icon since Orbit started. `None` until the first time.
    link_icons_asked: Option<HashSet<String>>,
    /// When every link without an icon was last asked for one.
    link_icons_at: Option<Instant>,
    exchange_rates: crate::rates::ExchangeRates,
    pub weather: crate::weather::Weather,
    pub speedtest: crate::speedtest::SpeedTest,
    /// The timer, once one has been set. It runs on with the launcher closed.
    pub countdown: Option<crate::clock::Countdown>,
    pub stopwatch: crate::clock::Stopwatch,
    pub link_args: Option<LinkArgs>,
    /// Where each of those boxes was last drawn.
    pub link_arg_boxes: Vec<RECT>,
    /// A second hidden box, for typing over several lines in a form.
    pub area: HWND,
    pub form: Option<InlineForm>,
    /// Where the form's boxes and placeholder chips were last drawn.
    pub form_boxes: Vec<RECT>,
    pub form_chips: Vec<RECT>,
    /// A form put aside when the launcher hid, and when.
    parked_form: Option<(InlineForm, Instant)>,
    visible: bool,
    prev_foreground: HWND,
    seq: u64,
    pub metrics: Metrics,
    hook: Option<HookHandle>,
    hook_failed: bool,
    hyper_actions: Vec<(u8, Action)>,
    shortcut_actions: Vec<(i32, Action)>,
    registered_hotkeys: Vec<i32>,
    hotkey_problems: Vec<String>,
    active_hotkey: String,
    /// A note went to the trash this session, and Notes offers to bring it back.
    can_undo_trash: bool,
    /// The query a web search row is waiting to be offered for.
    web_row_waiting: Option<String>,
    /// Apps with open windows. Forgotten each time the launcher opens.
    running_apps: Option<launch::RunningApps>,
    hotkey_override: Option<Chord>,
    suspended: bool,
    clip_listening: bool,
    clip_paused: bool,
    skip_next_copy: bool,
    index_running: bool,
    /// An app changed while memory was too tight to look. Caught up on the next show.
    index_owed: bool,
    index_dirty: bool,
    taskbar_created: u32,
    trim_after_ms: Option<u32>,
    last_mouse: (i32, i32),
    /// Where the footer's buttons were last drawn, and what each is called.
    pub footer_buttons: Vec<(RECT, &'static str)>,
    pub hovered_button: Option<&'static str>,
    pub pressed_button: Option<&'static str>,
    vim_mode: VimMode,
    vim_keys: VimKeys,
    insert_target: HWND,
    exiting: bool,
    last_counts: Option<(i64, i64, i64, i64)>,
    /// True when memory pressure has shed optional caches, until pressure ends.
    shed_active: bool,
    /// A snippet with clipboard content, waiting for the user to approve it.
    pending_expansion: Option<(orbit_core::snippet::Expanded, usize)>,
    recording: Option<Recording>,
    /// Whether the launcher has glass behind it right now.
    pub glass: bool,
    backdrop: ui::Backdrop,
    /// What was last applied, to skip repeating it on each system setting change.
    glass_applied: Option<(bool, bool, ui::Theme)>,
    /// The footer's logo at the current DPI.
    pub logo: Option<Vec<u8>>,
    pub caret_on: bool,
    /// The selection and scroll position last drawn, to tell when they've moved.
    drawn_field: (usize, usize),
    /// Where the search box's left edge is.
    edit_x: i32,
    preview: Option<Preview>,
    /// A developer preview, which never goes online. `preview` is taken while a frame is drawn.
    previewing: bool,
    /// What each tool last said, in memory only.
    vaults: HashMap<Tool, Vault>,
    /// The request whose answer still matters. Anything older is thrown away.
    tool_request: u64,
    /// A value was asked for, and pressing Esc cancels it.
    waiting_value: Option<u64>,
    /// The value asked for is to be typed where the launcher was opened, rather than copied.
    paste_value: bool,
    /// Stays open through an approval prompt, which takes focus. Set while a tool is being asked.
    hold_open: Option<Instant>,
    lost_focus_while_held: bool,
    /// A password or code Orbit copied, by its clipboard sequence number, for clearing later.
    secret_on_clipboard: Option<u32>,
    /// The files thread, once file search has been used.
    files: Option<crate::files::Files>,
    file_seq: u64,
    /// The newest search for each place files are shown. Answers to older ones are dropped.
    file_latest: [u64; 2],
    /// Where the icons of files and extensions' pictures in rows are read from, by icon key.
    icon_paths: HashMap<u64, (String, bool)>,
    everything_installing: bool,
    /// When Orbit started Everything, which takes a moment to answer.
    everything_starting: Option<Instant>,
    pub extensions: Vec<crate::extensions::Installed>,
    pub broken_extensions: Vec<crate::extensions::Broken>,
    runners: crate::extensions::Runners,
    /// GitHub, which answers from Orbit's own code.
    providers: crate::providers::Providers,
    /// The extension and request whose answer is wanted. Anything else it answers is old.
    ext_waiting: Option<(String, u64)>,
    ext_requests: u64,
    /// The filter chosen for each extension list, keyed `ext/command`, kept while Orbit runs.
    ext_filter: HashMap<String, String>,
    /// The filters the list showing now can be narrowed by, as its last answer offered them.
    pub ext_filters: Vec<orbit_core::extension::FilterOption>,
    /// Where each editable box was last drawn, so a click can put the caret where it lands.
    pub text_spots: Vec<ui::TextSpot>,
    /// A selection being made by dragging across a box: where it started, and in which box.
    text_drag: Option<(usize, ui::SpotOf)>,
}

/// What a password manager or authenticator last said. Item names and no secrets.
#[derive(Default)]
struct Vault {
    items: Vec<integrations::Item>,
    failure: Option<Failure>,
    loaded: bool,
}

thread_local! {
    pub static APP: RefCell<Option<App>> = const { RefCell::new(None) };
    static DEFERRED: RefCell<VecDeque<(u32, WPARAM, LPARAM, i64)>> = const { RefCell::new(VecDeque::new()) };
}

static EDIT_ORIGINAL_PROC: AtomicIsize = AtomicIsize::new(0);
/// Keys are being captured, so the query box must not type them. Read while `App` may be borrowed.
static RECORDING: AtomicBool = AtomicBool::new(false);
/// Full Vim, not typing: characters are commands. Read before the app state is reachable.
static VIM_NORMAL: AtomicBool = AtomicBool::new(false);
/// Alt plus a letter just did something, so its menu character and the Alt release are spent.
static ALT_CHORD_CHAR: AtomicBool = AtomicBool::new(false);
static ALT_CHORD_RELEASE: AtomicBool = AtomicBool::new(false);

fn is_deferrable(message: u32) -> bool {
    matches!(
        message,
        WM_COMMAND | WM_ACTIVATE | WM_HOTKEY | WM_DPICHANGED | WM_CLIPBOARDUPDATE | WM_SETTINGCHANGE | WM_DWMCOLORIZATIONCOLORCHANGED | WM_PAINT
            | WM_POWERBROADCAST | WM_WTSSESSION_CHANGE | WM_DISPLAYCHANGE | WM_TIMER | WM_ENDSESSION
    ) || (WM_APP..0xC000).contains(&message)
}

pub unsafe extern "system" fn main_wndproc(hwnd: HWND, message: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    unsafe {
        match message {
            WM_RBUTTONUP if in_input_band(hwnd, lparam) => {
                // The search box isn't on screen, so its menu of Cut, Copy and Paste opens from here.
                // Left clicks are the app's, which knows where the text was drawn.
                if let Ok(edit) = GetWindow(hwnd, GW_CHILD) {
                    edit_menu_at(hwnd, edit, lparam);
                    drain_deferred();
                    return LRESULT(0);
                }
            }
            WM_ERASEBKGND => return LRESULT(1),
            WM_CTLCOLOREDIT | WM_CTLCOLORSTATIC => {
                let dc = HDC(wparam.0 as *mut _);
                SetTextColor(dc, COLORREF(ui::EDIT_TEXT.load(Ordering::Relaxed)));
                SetBkColor(dc, COLORREF(ui::EDIT_BACK.load(Ordering::Relaxed)));
                return LRESULT(ui::EDIT_BRUSH.load(Ordering::Relaxed));
            }
            _ => {}
        }
        // The tray menu runs a modal loop, so it must not hold the app borrowed.
        if message == WM_ORBIT_TRAY {
            let event = (lparam.0 & 0xFFFF) as u32;
            if event == WM_RBUTTONUP || event == WM_CONTEXTMENU {
                show_tray_menu(hwnd);
                return LRESULT(0);
            }
        }
        let t0 = qpc();
        let outcome = APP.with(|cell| match cell.try_borrow_mut() {
            Ok(mut guard) => match guard.as_mut() {
                Some(app) => Ok(app.on_message(message, wparam, lparam, t0)),
                None => Ok(None),
            },
            Err(_) => Err(()),
        });
        match outcome {
            Ok(result) => {
                let value = match result {
                    Some(value) => value,
                    None => DefWindowProcW(hwnd, message, wparam, lparam),
                };
                drain_deferred();
                value
            }
            Err(()) if is_deferrable(message) => {
                DEFERRED.with(|queue| queue.borrow_mut().push_back((message, wparam, lparam, t0)));
                LRESULT(0)
            }
            Err(()) => DefWindowProcW(hwnd, message, wparam, lparam),
        }
    }
}

/// True if a mouse message's position is in the band at the top, where the search box is.
fn in_input_band(hwnd: HWND, lparam: LPARAM) -> bool {
    if ui::POPOVER_OPEN.load(Ordering::Relaxed) {
        return false;
    }
    let dpi = unsafe { GetDpiForWindow(hwnd) };
    let margin = ui::frame_margin(dpi);
    let x = (lparam.0 & 0xFFFF) as i16 as i32 - margin;
    let y = ((lparam.0 >> 16) & 0xFFFF) as i16 as i32 - margin;
    // Left of where typing starts is the back button.
    y >= 0 && y < ui::scale(ui::INPUT_HEIGHT, dpi) && x >= ui::input_left(&Scope::Root, dpi) && x < ui::scale(ui::BASE_WIDTH, dpi)
}

/// Where word of a program coming to the front goes.
static FOREGROUND_TARGET: AtomicIsize = AtomicIsize::new(0);

unsafe extern "system" fn on_foreground_event(_hook: HWINEVENTHOOK, _event: u32, _window: HWND, _object: i32, _child: i32, _thread: u32, _time: u32) {
    let target = HWND(FOREGROUND_TARGET.load(Ordering::Relaxed) as *mut _);
    if !target.is_invalid() {
        let _ = unsafe { PostMessageW(Some(target), WM_ORBIT_FOREGROUND, WPARAM(0), LPARAM(0)) };
    }
}

unsafe extern "system" fn plate_proc(hwnd: HWND, message: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    unsafe {
        match message {
            WM_ERASEBKGND => LRESULT(1),
            WM_NCHITTEST => LRESULT(HTTRANSPARENT as isize),
            _ => DefWindowProcW(hwnd, message, wparam, lparam),
        }
    }
}

/// How round Windows 11 makes a window's corners when asked to round them.
const PLATE_CORNER_DIP: i32 = 8;

/// How far inside the launcher the blur goes, so that a corner rounded by `blur_radius` sits
/// wholly inside one rounded by `launcher_radius`. The two curves' centres are then no further
/// apart than the difference in their sizes, with a pixel to spare.
fn plate_inset(launcher_radius: i32, blur_radius: i32) -> i32 {
    let gap = (launcher_radius - blur_radius).max(0) as f32;
    (gap * (1.0 - std::f32::consts::FRAC_1_SQRT_2)).ceil() as i32 + 1
}

/// The blur's own window, blurred by the compositor in exactly the launcher's shape.
fn create_frosted_plate() -> Option<(HWND, crate::frost::Frost)> {
    const WS_EX_NOREDIRECTIONBITMAP: WINDOW_EX_STYLE = WINDOW_EX_STYLE(0x0020_0000);
    unsafe {
        let module = GetModuleHandleW(None).ok()?;
        let class = WNDCLASSEXW {
            cbSize: std::mem::size_of::<WNDCLASSEXW>() as u32,
            lpfnWndProc: Some(plate_proc),
            hInstance: module.into(),
            lpszClassName: w!("OrbitPlate"),
            ..Default::default()
        };
        let _ = RegisterClassExW(&class);
        let plate = CreateWindowExW(
            WS_EX_TOOLWINDOW | WS_EX_NOACTIVATE | WS_EX_TRANSPARENT | WS_EX_TOPMOST | WS_EX_NOREDIRECTIONBITMAP,
            w!("OrbitPlate"),
            w!(""),
            WS_POPUP,
            0,
            0,
            1,
            1,
            None,
            None,
            Some(module.into()),
            None,
        )
        .ok()?;
        // The blur is shaped by its own clip, so Windows mustn't round or outline the window.
        let corner = DWMWCP_DONOTROUND;
        let _ = DwmSetWindowAttribute(plate, DWMWA_WINDOW_CORNER_PREFERENCE, &corner as *const _ as *const _, std::mem::size_of_val(&corner) as u32);
        let border = DWMWA_COLOR_NONE;
        let _ = DwmSetWindowAttribute(plate, DWMWA_BORDER_COLOR, &border as *const _ as *const _, std::mem::size_of_val(&border) as u32);
        let allow = BOOL::from(true);
        let _ = DwmSetWindowAttribute(plate, windows::Win32::Graphics::Dwm::DWMWA_USE_HOSTBACKDROPBRUSH, &allow as *const _ as *const _, std::mem::size_of::<BOOL>() as u32);
        match crate::frost::Frost::new(plate) {
            Some(frost) => Some((plate, frost)),
            None => {
                let _ = DestroyWindow(plate);
                None
            }
        }
    }
}

/// The blur's own window, from Windows' backdrop. Used when the compositor can't make one.
fn create_plate() -> Option<HWND> {
    unsafe {
        let module = GetModuleHandleW(None).ok()?;
        let class = WNDCLASSEXW {
            cbSize: std::mem::size_of::<WNDCLASSEXW>() as u32,
            lpfnWndProc: Some(plate_proc),
            hInstance: module.into(),
            lpszClassName: w!("OrbitPlate"),
            ..Default::default()
        };
        // Already registered is fine, if the window is made again.
        let _ = RegisterClassExW(&class);
        let plate = CreateWindowExW(
            WS_EX_TOOLWINDOW | WS_EX_NOACTIVATE | WS_EX_TRANSPARENT | WS_EX_TOPMOST,
            w!("OrbitPlate"),
            w!(""),
            WS_POPUP,
            0,
            0,
            1,
            1,
            None,
            None,
            Some(module.into()),
            None,
        )
        .ok()?;
        // Windows rounds it, which clips the blur too. A window region would cut the window but
        // not its blur, whose square corners then showed behind the launcher's round ones.
        let corner = DWMWCP_ROUND;
        let _ = DwmSetWindowAttribute(plate, DWMWA_WINDOW_CORNER_PREFERENCE, &corner as *const _ as *const _, std::mem::size_of_val(&corner) as u32);
        let border = DWMWA_COLOR_NONE;
        let _ = DwmSetWindowAttribute(plate, DWMWA_BORDER_COLOR, &border as *const _ as *const _, std::mem::size_of_val(&border) as u32);
        let margins = MARGINS { cxLeftWidth: -1, cxRightWidth: -1, cyTopHeight: -1, cyBottomHeight: -1 };
        let _ = DwmExtendFrameIntoClientArea(plate, &margins);
        Some(plate)
    }
}

/// Opens the search box's own menu where a right click in the launcher was.
fn edit_menu_at(hwnd: HWND, edit: HWND, lparam: LPARAM) {
    let mut point = POINT { x: (lparam.0 & 0xFFFF) as i16 as i32, y: ((lparam.0 >> 16) & 0xFFFF) as i16 as i32 };
    unsafe {
        let _ = ClientToScreen(hwnd, &mut point);
        let packed = (point.x as u16 as isize) | ((point.y as u16 as isize) << 16);
        SendMessageW(edit, WM_CONTEXTMENU, Some(WPARAM(edit.0 as usize)), Some(LPARAM(packed)));
    }
}

fn drain_deferred() {
    loop {
        let handled = APP.with(|cell| {
            // Stay queued while the app is busy. A message taken off the queue can't be retried.
            let Ok(mut guard) = cell.try_borrow_mut() else { return false };
            let Some(app) = guard.as_mut() else { return false };
            let Some((message, wparam, lparam, t0)) = DEFERRED.with(|queue| queue.borrow_mut().pop_front()) else {
                return false;
            };
            app.on_message(message, wparam, lparam, t0);
            true
        });
        if !handled {
            return;
        }
    }
}

fn show_tray_menu(hwnd: HWND) {
    let state = APP.with(|cell| {
        cell.try_borrow().ok().and_then(|guard| {
            guard.as_ref().map(|app| MenuState {
                clipboard_enabled: app.settings.clipboard.enabled,
                clipboard_paused: app.clip_paused,
                suspended: app.suspended,
                launcher_hint: app.active_hotkey.clone(),
                appearance: app.settings.appearance,
                update: crate::update::tray_item(),
            })
        })
    });
    let Some(state) = state else { return };
    let Some(choice) = tray::show_menu(hwnd, &state) else { return };
    APP.with(|cell| {
        if let Ok(mut guard) = cell.try_borrow_mut() {
            if let Some(app) = guard.as_mut() {
                app.on_tray_choice(choice);
            }
        }
    });
    drain_deferred();
}

unsafe extern "system" fn edit_wndproc(hwnd: HWND, message: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    unsafe {
        let call_original = || {
            let original: WNDPROC = std::mem::transmute(EDIT_ORIGINAL_PROC.load(Ordering::Relaxed));
            CallWindowProcW(original, hwnd, message, wparam, lparam)
        };
        // The box isn't on screen and doesn't paint, so the window redraws when anything may
        // have moved. One message at a time, however fast the keys come.
        let selecting = message == WM_MOUSEMOVE && wparam.0 & 1 != 0;
        if (selecting || matches!(message, WM_KEYDOWN | WM_KEYUP | WM_SYSKEYDOWN | WM_CHAR | WM_LBUTTONDOWN | WM_LBUTTONUP | WM_LBUTTONDBLCLK | WM_PASTE | WM_CUT | WM_UNDO | WM_CLEAR))
            && !CARET_PENDING.swap(true, Ordering::Relaxed)
        {
            if let Ok(parent) = GetParent(hwnd) {
                let _ = PostMessageW(Some(parent), WM_ORBIT_CARET, WPARAM(0), LPARAM(0));
            }
        }
        match message {
            WM_KEYDOWN | WM_SYSKEYDOWN => {
                let t_key = qpc();
                let handled = APP.with(|cell| {
                    cell.try_borrow_mut().ok().and_then(|mut guard| {
                        guard.as_mut().map(|app| app.on_key(wparam.0 as u16, t_key))
                    })
                });
                drain_deferred();
                if handled == Some(true) {
                    return LRESULT(0);
                }
            }
            WM_SYSCHAR if ALT_CHORD_CHAR.swap(false, Ordering::Relaxed) => return LRESULT(0),
            // Left to Windows, letting go of Alt after a chord would open the window's menu.
            WM_SYSKEYUP if wparam.0 == VK_MENU.0 as usize && ALT_CHORD_RELEASE.swap(false, Ordering::Relaxed) => return LRESULT(0),
            WM_CHAR if VIM_NORMAL.load(Ordering::Relaxed) => {
                let c = char::from_u32(wparam.0 as u32);
                APP.with(|cell| {
                    if let (Ok(mut guard), Some(c)) = (cell.try_borrow_mut(), c) {
                        if let Some(app) = guard.as_mut() {
                            app.on_vim_char(c);
                        }
                    }
                });
                drain_deferred();
                return LRESULT(0);
            }
            WM_LBUTTONDOWN if VIM_NORMAL.load(Ordering::Relaxed) => {
                // Clicking in the box means you want to type there.
                APP.with(|cell| {
                    if let Ok(mut guard) = cell.try_borrow_mut() {
                        if let Some(app) = guard.as_mut() {
                            app.set_vim_mode(VimMode::Insert);
                        }
                    }
                });
                drain_deferred();
            }
            WM_CHAR | WM_SYSCHAR if RECORDING.load(Ordering::Relaxed) => return LRESULT(0),
            WM_CHAR => {
                // These are handled on key-down. The control would otherwise beep or insert a box.
                let c = wparam.0 as u32;
                let new_line = c == 0x0D && hwnd.0 as isize == AREA_HWND.load(Ordering::Relaxed);
                if !new_line && (c == 0x0D || c == 0x0A || c == 0x1B || c == 0x09 || c == 0x0B || c == 0x7F || c == 0x01) {
                    return LRESULT(0);
                }
            }
            _ => {}
        }
        call_original()
    }
}

fn load_settings(dir: &std::path::Path) -> (Settings, bool, Option<String>) {
    let path = dir.join("settings.json");
    let Ok(text) = std::fs::read_to_string(&path) else {
        return (Settings::default(), false, None);
    };
    let (settings, outcome) = Settings::parse(&text);
    match outcome {
        LoadOutcome::Loaded | LoadOutcome::Missing => (settings, false, None),
        LoadOutcome::Corrupt(_) => {
            // Keep the damaged file for the user, and start from defaults.
            let _ = std::fs::rename(&path, dir.join("settings.damaged.json"));
            (settings, false, Some("Your settings file was damaged, so defaults are in use. The old file is saved as settings.damaged.json.".to_string()))
        }
        LoadOutcome::NewerVersion(version) => (
            settings,
            true,
            Some(format!("These settings are from a newer Orbit (schema {version}). Changes won't be saved.")),
        ),
    }
}

/// Creates the window and state. The window is ready to appear instantly from then on.
pub fn startup(options: Options) -> windows::core::Result<()> {
    let start = options.start;
    let updated = options.updated;
    let _ = std::fs::create_dir_all(&options.data_dir);
    let (mut settings, read_only, settings_notice) = load_settings(&options.data_dir);
    if options.preview.is_none() {
        settings.launch_at_login = launch_at_login_registered();
    }
    let first_run = !settings.onboarded;
    let apps = indexer::load_index(&options.data_dir);
    let needs_index = apps.is_none();

    unsafe {
        let _ = SetThreadPriority(GetCurrentThread(), THREAD_PRIORITY_ABOVE_NORMAL);
        let module = GetModuleHandleW(None)?;
        let class = WNDCLASSEXW {
            cbSize: std::mem::size_of::<WNDCLASSEXW>() as u32,
            lpfnWndProc: Some(main_wndproc),
            hInstance: module.into(),
            hCursor: LoadCursorW(None, IDC_ARROW)?,
            lpszClassName: MAIN_CLASS,
            style: CS_DBLCLKS,
            hIcon: tray::load_app_icon(32, 32),
            ..Default::default()
        };
        if RegisterClassExW(&class) == 0 {
            return Err(windows::core::Error::from_thread());
        }
        let dpi = GetDpiForSystem();
        let visible_rows = settings.launcher.max_visible_rows.clamp(3, 12) as i32;
        let (panel_width, height) = ui::window_size(dpi, visible_rows);
        let (width, frame_height) = ui::frame_size(dpi, (panel_width, height));
        let main = CreateWindowExW(
            // Layered: it shows a finished picture with its own soft corners and shadow.
            WS_EX_TOOLWINDOW | WS_EX_TOPMOST | WS_EX_LAYERED,
            MAIN_CLASS,
            w!("Orbit"),
            WS_POPUP | WS_CLIPCHILDREN,
            0,
            0,
            width,
            frame_height,
            None,
            None,
            Some(module.into()),
            None,
        )?;
        crate::instance::mark(main, &options.data_dir);
        let corner = DWMWCP_DONOTROUND;
        let _ = DwmSetWindowAttribute(
            main,
            DWMWA_WINDOW_CORNER_PREFERENCE,
            &corner as *const _ as *const _,
            std::mem::size_of_val(&corner) as u32,
        );
        let edit = CreateWindowExW(
            WINDOW_EX_STYLE(0),
            w!("EDIT"),
            w!(""),
            WS_CHILD | WS_VISIBLE | WINDOW_STYLE(ES_AUTOHSCROLL as u32),
            0,
            0,
            10,
            10,
            Some(main),
            None,
            Some(module.into()),
            None,
        )?;
        let original = SetWindowLongPtrW(edit, GWLP_WNDPROC, edit_wndproc as *const () as isize);
        EDIT_ORIGINAL_PROC.store(original, Ordering::Relaxed);
        // The window draws the text, because a standard control can't paint over glass. This
        // takes the box off the screen and leaves it working for typing and screen readers.
        let nothing = CreateRectRgn(0, 0, 0, 0);
        let _ = SetWindowRgn(edit, Some(nothing), false);
        let area = CreateWindowExW(
            WINDOW_EX_STYLE(0),
            w!("EDIT"),
            w!(""),
            WS_CHILD | WS_VISIBLE | WINDOW_STYLE((ES_MULTILINE | ES_WANTRETURN | ES_AUTOHSCROLL | ES_AUTOVSCROLL) as u32),
            0,
            0,
            400,
            200,
            Some(main),
            None,
            Some(module.into()),
            None,
        )?;
        SetWindowLongPtrW(area, GWLP_WNDPROC, edit_wndproc as *const () as isize);
        AREA_HWND.store(area.0 as isize, Ordering::Relaxed);
        // EM_SETLIMITTEXT: as much as it can hold, not the usual 32,767 characters.
        SendMessageW(area, 0x00C5, Some(WPARAM(0)), Some(LPARAM(0)));
        let nothing = CreateRectRgn(0, 0, 0, 0);
        let _ = SetWindowRgn(area, Some(nothing), false);

        let send = SendHwnd::new(main);
        let policy = RetentionPolicy {
            max_items: settings.clipboard.max_items,
            retention_days: settings.clipboard.retention_days,
        };
        let worker = worker::start(options.data_dir.clone(), settings.clipboard.persist, policy, send, WM_ORBIT_DATA);
        let catalog = Catalog::new(apps.unwrap_or_default(), &settings);
        let theme = ui::resolve_theme(settings.appearance);
        ui::publish_edit_colors(&theme);
        crate::rows::set_icon_style(settings.icon_style);
        let taskbar_created = RegisterWindowMessageW(w!("TaskbarCreated"));
        let icon_capacity = if settings.low_resource_mode { 16 } else { 64 };

        let extension_data_dir = options.data_dir.clone();
        let mut app = App {
            main,
            edit,
            send,
            dir: options.data_dir,
            settings,
            settings_read_only: read_only,
            save_failed: false,
            settings_recording: false,
            catalog,
            scratch: QueryScratch::default(),
            hits: Vec::with_capacity(ROW_LIMIT),
            worker,
            view: View::new(visible_rows as usize),
            status: None,
            theme,
            dpi,
            fonts: None,
            window_height: frame_height,
            backbuffer: None,
            frame: None,
            frame_shadow: None,
            plate: None,
            plate_size: (0, 0),
            frost: None,
            menu: None,
            foreground_events: None,
            launcher_aside: false,
            launcher_chord: None,
            known_games: Vec::new(),
            known_games_at: None,
            emoji_table: Vec::new(),
            emoji_cache: HashMap::new(),
            icons: IconCache::new(icon_capacity),
            clip_preview: None,
            clip_preview_wanted: None,
            link_icons_asked: None,
            link_icons_at: None,
            exchange_rates: Default::default(),
            weather: Default::default(),
            speedtest: Default::default(),
            countdown: None,
            stopwatch: Default::default(),
            link_args: None,
            link_arg_boxes: Vec::new(),
            area,
            form: None,
            form_boxes: Vec::new(),
            form_chips: Vec::new(),
            parked_form: None,
            visible: false,
            prev_foreground: HWND::default(),
            seq: 0,
            metrics: Metrics::new(options.bench_log),
            hook: None,
            hook_failed: false,
            hyper_actions: Vec::new(),
            shortcut_actions: Vec::new(),
            registered_hotkeys: Vec::new(),
            hotkey_problems: Vec::new(),
            active_hotkey: String::new(),
            can_undo_trash: false,
            web_row_waiting: None,
            running_apps: None,
            hotkey_override: options.hotkey_override,
            suspended: false,
            clip_listening: false,
            clip_paused: false,
            skip_next_copy: false,
            index_running: false,
            index_owed: false,
            index_dirty: false,
            taskbar_created,
            trim_after_ms: options.trim_after_ms,
            last_mouse: (i32::MIN, i32::MIN),
            footer_buttons: Vec::new(),
            hovered_button: None,
            pressed_button: None,
            vim_mode: VimMode::Insert,
            vim_keys: VimKeys::default(),
            insert_target: HWND::default(),
            exiting: false,
            last_counts: None,
            shed_active: false,
            pending_expansion: None,
            recording: None,
            glass: false,
            backdrop: options.backdrop.unwrap_or(ui::Backdrop::Accent),
            glass_applied: None,
            logo: None,
            caret_on: true,
            drawn_field: (0, 0),
            edit_x: 0,
            previewing: options.preview.is_some(),
            preview: options.preview,
            vaults: HashMap::new(),
            tool_request: 0,
            waiting_value: None,
            paste_value: false,
            hold_open: None,
            lost_focus_while_held: false,
            secret_on_clipboard: None,
            files: None,
            file_seq: 0,
            file_latest: [0, 0],
            icon_paths: HashMap::new(),
            everything_installing: false,
            everything_starting: None,
            extensions: Vec::new(),
            broken_extensions: Vec::new(),
            runners: crate::extensions::Runners::new(extension_data_dir),
            providers: Default::default(),
            ext_waiting: None,
            ext_requests: 0,
            ext_filter: HashMap::new(),
            ext_filters: Vec::new(),
            text_spots: Vec::new(),
            text_drag: None,
        };
        app.discover_extensions();
        app.layout_for_dpi(dpi);
        app.apply_glass();
        if app.preview.is_some() {
            let _ = SetTimer(Some(main), TIMER_PREVIEW, 1500, None);
        }
        tray::add(main, WM_ORBIT_TRAY);
        if app.preview.is_none() {
            crate::update::start(main, app.dir.clone());
            if updated {
                crate::update::announce_updated();
            }
        }
        if app.preview.is_none() {
            let s = &mut app.settings;
            let mut lists = [
                &mut s.hyper.excluded_apps,
                &mut s.mac_feel.excluded_apps,
                &mut s.launcher.excluded_apps,
                &mut s.clipboard.excluded_apps,
                &mut s.snippets.excluded_apps,
            ];
            if crate::app_picker::fix_steam_entries(&mut lists) {
                app.save_settings();
            }
        }
        app.register_hotkeys();
        app.apply_hook();
        app.apply_clipboard_listener();
        if app.preview.is_none() {
            app.sync_link_icons();
        }
        let _ = WTSRegisterSessionNotification(main, NOTIFY_FOR_THIS_SESSION);
        crate::watch::start(send, WM_ORBIT_INDEX_DIRTY);
        app.worker.send(Req::LoadUsage);

        // Apps may have come or gone while Orbit wasn't running, which no watcher saw. One short
        // run at idle priority catches up; until it's done, the saved list is used.
        if needs_index || !options.no_reindex {
            app.start_indexer(false);
        }
        if let Some(notice) = settings_notice {
            app.notify("Orbit", &notice);
        }
        app.announce_hotkey_problems();
        // The welcome marks itself as seen when it closes, however it closes.
        match start {
            Start::Welcome { at } if !app.previewing => app.open_welcome(true, at),
            _ if first_run && !app.previewing => app.open_welcome(false, None),
            Start::Launcher if !app.previewing => app.send.post(WM_ORBIT_TOGGLE, 0, 0),
            _ => {}
        }
        APP.with(|cell| *cell.borrow_mut() = Some(app));
    }
    Ok(())
}

impl App {
    fn on_message(&mut self, message: u32, wparam: WPARAM, lparam: LPARAM, t0: i64) -> Option<LRESULT> {
        match message {
            WM_HOTKEY => {
                self.on_hotkey(wparam.0 as i32, t0);
                Some(LRESULT(0))
            }
            WM_PAINT => {
                self.on_paint();
                Some(LRESULT(0))
            }
            WM_COMMAND => {
                let notification = (wparam.0 >> 16) as u32;
                if notification == EN_CHANGE && lparam.0 == self.edit.0 as isize {
                    self.on_query_changed();
                } else if notification == EN_CHANGE && lparam.0 == self.area.0 as isize {
                    self.on_area_changed();
                }
                Some(LRESULT(0))
            }
            WM_ACTIVATE => {
                if (wparam.0 & 0xFFFF) as u32 == WA_INACTIVE && self.visible {
                    if self.hold_open.is_some() {
                        // Probably an approval prompt. It must be able to come in front.
                        self.lost_focus_while_held = true;
                        unsafe {
                            let _ = SetWindowPos(self.main, Some(HWND_NOTOPMOST), 0, 0, 0, 0, SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE);
                        }
                        self.sync_plate();
                    } else {
                        self.hide(false);
                    }
                }
                Some(LRESULT(0))
            }
            WM_MOUSEMOVE => {
                let (x, y) = self.panel_point(lparam);
                if self.text_drag.is_some() {
                    // The button can be let go where this window doesn't hear it.
                    if wparam.0 & 1 != 0 {
                        self.drag_text(x, y);
                        return Some(LRESULT(0));
                    }
                    self.end_text_drag();
                }
                self.on_mouse_move(x, y);
                Some(LRESULT(0))
            }
            WM_CAPTURECHANGED => {
                self.text_drag = None;
                Some(LRESULT(0))
            }
            WM_MOUSELEAVE => {
                self.hovered_button = None;
                self.pressed_button = None;
                self.redraw();
                Some(LRESULT(0))
            }
            WM_SETCURSOR if self.pointer_in_input() => {
                if let Ok(beam) = unsafe { LoadCursorW(None, IDC_IBEAM) } {
                    unsafe {
                        SetCursor(Some(beam));
                    }
                }
                Some(LRESULT(1))
            }
            WM_ORBIT_FOREGROUND => {
                self.update_launcher_aside();
                Some(LRESULT(0))
            }
            WM_ORBIT_CLI => {
                if let Some(answer) = util::take_posted::<integrations::Answer>(wparam.0) {
                    self.on_tool_answer(*answer);
                }
                Some(LRESULT(0))
            }
            hook::WM_ORBIT_MAC => {
                launch::quit_or_hide_front(wparam.0 == 0);
                Some(LRESULT(0))
            }
            WM_ORBIT_CARET => {
                CARET_PENDING.store(false, Ordering::Relaxed);
                self.on_field_moved();
                Some(LRESULT(0))
            }
            WM_SETCURSOR if self.hovered_button.is_some() => {
                if let Ok(hand) = unsafe { LoadCursorW(None, IDC_HAND) } {
                    unsafe {
                        SetCursor(Some(hand));
                    }
                }
                Some(LRESULT(1))
            }
            WM_LBUTTONDOWN | WM_LBUTTONDBLCLK => {
                let (x, y) = self.panel_point(lparam);
                self.pressed_button = self.footer_button_at(x, y);
                if self.pressed_button.is_some() {
                    self.hovered_button = self.pressed_button;
                    self.redraw();
                } else {
                    let shift = unsafe { GetKeyState(0x10) } < 0;
                    self.press_text(x, y, message == WM_LBUTTONDBLCLK, shift);
                }
                Some(LRESULT(0))
            }
            WM_LBUTTONUP => {
                let (x, y) = self.panel_point(lparam);
                // A press in a box was handled when it went down.
                if self.text_drag.is_some() {
                    self.end_text_drag();
                    return Some(LRESULT(0));
                }
                match self.pressed_button.take() {
                    Some(pressed) => {
                        if self.footer_button_at(x, y) == Some(pressed) {
                            self.on_footer_button(pressed);
                        }
                        self.redraw();
                    }
                    None if self.footer_button_at(x, y).is_none() => self.on_click(x, y, false),
                    None => {}
                }
                Some(LRESULT(0))
            }
            WM_RBUTTONUP => {
                let (x, y) = self.panel_point(lparam);
                self.on_click(x, y, true);
                Some(LRESULT(0))
            }
            WM_MOUSEWHEEL => {
                let delta = (wparam.0 >> 16) as i16 as i32;
                let lines = if self.view.grid.is_some() { 1 } else { 2 };
                self.view.scroll_by(if delta > 0 { -lines } else { lines });
                self.redraw();
                Some(LRESULT(0))
            }
            WM_DPICHANGED => {
                self.layout_for_dpi((wparam.0 & 0xFFFF) as u32);
                Some(LRESULT(0))
            }
            WM_SETTINGCHANGE | WM_THEMECHANGED | WM_DWMCOLORIZATIONCOLORCHANGED => {
                self.refresh_theme();
                None
            }
            WM_DISPLAYCHANGE => {
                crate::displays::changed();
                if self.visible {
                    self.hide(false);
                }
                None
            }
            WM_TIMER => {
                self.on_timer(wparam.0);
                Some(LRESULT(0))
            }
            WM_CLIPBOARDUPDATE => {
                self.on_clipboard_changed();
                Some(LRESULT(0))
            }
            WM_WTSSESSION_CHANGE => {
                self.on_session_change(wparam.0 as u32);
                Some(LRESULT(0))
            }
            WM_POWERBROADCAST => {
                self.on_power(wparam.0 as u32);
                Some(LRESULT(1))
            }
            WM_QUERYENDSESSION => Some(LRESULT(1)),
            WM_ENDSESSION => {
                if wparam.0 != 0 {
                    self.begin_exit();
                    // Windows may end the process as soon as this returns, so give the data thread
                    // a moment to finish its last write. The tests don't exercise a real shutdown.
                    crate::worker::wait_for_finish(std::time::Duration::from_millis(2000));
                }
                Some(LRESULT(0))
            }
            WM_CLOSE => {
                self.hide(false);
                Some(LRESULT(0))
            }
            WM_ORBIT_TRAY => {
                let event = (lparam.0 & 0xFFFF) as u32;
                if event == WM_LBUTTONUP {
                    self.toggle(Scope::Root, "tray", t0);
                } else if event == tray::NOTIFICATION_CLICKED {
                    crate::update::notification_clicked();
                }
                Some(LRESULT(0))
            }
            WM_ORBIT_UPDATE => {
                if let Some(line) = crate::update::on_message(wparam.0) {
                    self.report(&line);
                }
                Some(LRESULT(0))
            }
            WM_ORBIT_DATA => {
                self.on_data();
                Some(LRESULT(0))
            }
            WM_ORBIT_HYPER => {
                self.metrics.record_hook_dispatch(lparam.0 as i64, t0);
                self.on_hyper(wparam.0 as u8, lparam.0 as i64);
                Some(LRESULT(0))
            }
            WM_ORBIT_EXPAND => {
                self.on_expand(wparam.0 as i64, lparam.0 as usize);
                Some(LRESULT(0))
            }
            WM_ORBIT_INDEX_DONE => {
                self.on_index_done(wparam.0 != 0);
                Some(LRESULT(0))
            }
            WM_ORBIT_INDEX_DIRTY => {
                // An app came or went, so the list catches up now, on battery too: it's one short
                // run per change. Only when memory is nearly gone does it wait, for the next show.
                if self.shed_active {
                    self.index_owed = true;
                } else {
                    self.start_indexer(false);
                }
                Some(LRESULT(0))
            }
            WM_ORBIT_TOGGLE => {
                self.toggle(Scope::Root, "instance", t0);
                Some(LRESULT(0))
            }
            WM_ORBIT_COLOUR => {
                if wparam.0 & 0x0100_0000 != 0 {
                    let colour = orbit_core::colour::Colour { r: (wparam.0 >> 16) as u8, g: (wparam.0 >> 8) as u8, b: wparam.0 as u8 };
                    self.show(Scope::Colour(colour), "command", qpc());
                }
                Some(LRESULT(0))
            }
            WM_ORBIT_REPORT => {
                if let Some(message) = util::take_posted::<String>(wparam.0) {
                    self.report(&message);
                }
                Some(LRESULT(0))
            }
            WM_ORBIT_LAUNCH_FAILED => {
                let Some(message) = util::take_posted::<String>(wparam.0) else {
                    return Some(LRESULT(0));
                };
                self.metrics.record_action("launch-failed", 0, 0);
                self.report(&message);
                Some(LRESULT(0))
            }
            WM_ORBIT_SECRET_TYPED => {
                self.on_secret_typed(wparam.0, lparam.0 as u32);
                Some(LRESULT(0))
            }
            WM_ORBIT_INSERT_DONE => {
                let ticks = clipboard::PASSWORD_CHECK_TICKS.swap(0, std::sync::atomic::Ordering::Relaxed);
                if ticks != 0 {
                    self.metrics.record_action("password-check", 0, ticks);
                }
                let stopped = clipboard::TYPING_STOPPED.swap(0, std::sync::atomic::Ordering::Relaxed);
                if stopped != 0 {
                    self.metrics.record_action("typing-stopped", 0, stopped as i64);
                }
                match wparam.0 {
                    1 => self.report("Couldn't insert the text"),
                    2 => self.report("Orbit couldn't type into that window, so the text is on your clipboard."),
                    3 => self.report("That snippet is too long to type, so it's on your clipboard. Press Ctrl+V to paste it."),
                    6 => self.report("Orbit couldn't paste into that window, so it's on your clipboard. Press Ctrl+V to paste it."),
                    4 | 5 => {
                        self.report("Orbit didn't expand that snippet because this may be a password field.");
                        let kind = if wparam.0 == 4 { "password-reported" } else { "password-unknown" };
                        self.metrics.record_action(kind, 0, 0);
                    }
                    _ => {}
                }
                Some(LRESULT(0))
            }
            WM_ORBIT_SETTINGS_APPLY => {
                let Some(new_settings) = util::take_posted::<Settings>(wparam.0) else {
                    return Some(LRESULT(0));
                };
                // These change from the launcher, and may have since the window opened.
                let mut merged = *new_settings;
                merged.pinned = self.settings.pinned.clone();
                merged.aliases = self.settings.aliases.clone();
                merged.links = self.settings.links.clone();
                merged.custom_commands = self.settings.custom_commands.clone();
                merged.emoji = self.settings.emoji.clone();
                // Only the button that brings them all back changes these from there.
                merged.hidden = if lparam.0 != 0 { Vec::new() } else { self.settings.hidden.clone() };
                // Not settings the window shows, but it holds a copy of them from when it opened.
                merged.hotkey_notice = self.settings.hotkey_notice.clone();
                merged.onboarded = self.settings.onboarded;
                if merged == self.settings {
                    // The same settings again, after a write that failed, means try that again.
                    if self.save_failed {
                        self.save_settings();
                    }
                    self.tell_settings(Some(!self.save_failed));
                    return Some(LRESULT(0));
                }
                let theme = self.theme;
                self.apply_settings(merged);
                if self.theme != theme {
                    self.reopen_settings();
                }
                self.tell_settings(Some(!self.save_failed));
                Some(LRESULT(0))
            }
            WM_ORBIT_SETTINGS_RECORDING => {
                self.set_settings_recording(wparam.0 != 0);
                Some(LRESULT(0))
            }
            WM_ORBIT_SETTINGS_REOPEN => {
                self.reopen_settings();
                Some(LRESULT(0))
            }
            WM_ORBIT_SETTINGS_TOOL_DONE => {
                if let Some(done) = util::take_posted::<integrations::ToolDone>(wparam.0) {
                    if !crate::settings_win::tool_done(done.tool, &done.message, done.succeeded) {
                        self.notify("Orbit", &done.message);
                    }
                }
                Some(LRESULT(0))
            }
            WM_ORBIT_SET_ALIAS => {
                if let Some(change) = util::take_posted::<(String, String, bool)>(wparam.0) {
                    let (key, alias, keyword) = *change;
                    let own = key.starts_with(&crate::catalog::link_key("")) || key.starts_with(&crate::catalog::custom_key(""));
                    if keyword {
                        crate::settings_win::set_item_keyword(&mut self.settings, &key, &alias);
                    } else {
                        crate::settings_win::set_item_alias(&mut self.settings, &key, &alias);
                    }
                    self.save_and_rebuild_catalog();
                    if own {
                        crate::settings_win::links_changed(&self.settings);
                    }
                    if self.visible {
                        self.refresh_results();
                    }
                }
                Some(LRESULT(0))
            }
            WM_ORBIT_SET_HIDDEN => {
                if let Some(change) = util::take_posted::<(String, bool)>(wparam.0) {
                    let (key, hide) = *change;
                    let hidden_now = self.settings.hidden.contains(&key);
                    if hide && !hidden_now {
                        self.settings.hidden.push(key);
                    } else if !hide && hidden_now {
                        self.settings.hidden.retain(|item| *item != key);
                    }
                    self.save_and_rebuild_catalog();
                    if self.visible {
                        self.refresh_results();
                    }
                }
                Some(LRESULT(0))
            }
            WM_ORBIT_ENTE_ACCOUNT => {
                if let Some(account) = util::take_posted::<integrations::EnteAccount>(wparam.0) {
                    crate::settings_win::ente_account(&account);
                }
                Some(LRESULT(0))
            }
            WM_ORBIT_LINK_SUGGESTION => {
                if let Some(found) = util::take_posted::<crate::foreground::Suggestion>(wparam.0) {
                    self.on_link_suggestion(*found);
                }
                Some(LRESULT(0))
            }
            WM_ORBIT_LINK_SAVED => {
                if let Some(saved) = util::take_posted::<crate::link_form::LinkSaved>(wparam.0) {
                    self.on_link_saved(*saved);
                }
                Some(LRESULT(0))
            }
            WM_ORBIT_LINK_REQUEST => {
                if let Some(request) = util::take_posted::<crate::link_form::LinkRequest>(wparam.0) {
                    self.on_link_request(*request);
                }
                Some(LRESULT(0))
            }
            WM_ORBIT_RUN_BUILTIN => {
                if wparam.0 == usize::MAX {
                    self.begin_exit();
                } else if let Some(entry) = Builtin::ALL.get(wparam.0) {
                    // The window that sent it has already had its second press.
                    if lparam.0 != 0 {
                        self.perform_builtin(entry.0);
                    } else {
                        self.run_builtin(entry.0);
                    }
                }
                Some(LRESULT(0))
            }
            crate::setup::WM_ORBIT_SETUP => {
                if let Some(change) = util::take_posted::<crate::setup::Change>(wparam.0) {
                    self.on_setup(*change);
                }
                Some(LRESULT(0))
            }
            WM_ORBIT_RECLAIM => {
                // Wait a moment, so the data thread has finished with what the editor sent it.
                self.worker.send(Req::Shed);
                unsafe {
                    let _ = SetTimer(Some(self.main), TIMER_RECLAIM, 800, None);
                }
                Some(LRESULT(0))
            }
            WM_ORBIT_BENCH_FLUSH => {
                self.metrics.flush();
                let memory = util::process_memory();
                Some(LRESULT((memory.private_bytes >> 10) as isize))
            }
            WM_ORBIT_BENCH_TRIM => {
                self.trim_working_set();
                Some(LRESULT(0))
            }
            WM_ORBIT_SPEEDTEST => {
                if self.speedtest.arrived() && self.visible && self.speedtest_shown() {
                    // The row says what Enter does, which changes when the test ends.
                    if !self.speedtest.test.running() && self.menu.is_none() {
                        self.refresh_results();
                    }
                    self.redraw();
                }
                Some(LRESULT(0))
            }
            WM_ORBIT_SPEEDTEST_INSTALLED => {
                self.speedtest.installed();
                if !self.speedtest.missing && self.view.scope == Scope::SpeedTest {
                    self.speedtest.start(self.send, WM_ORBIT_SPEEDTEST);
                }
                if self.visible && self.view.scope == Scope::SpeedTest {
                    self.refresh_results();
                    self.redraw();
                }
                Some(LRESULT(0))
            }
            WM_ORBIT_WEATHER => {
                if let Some(place) = self.weather.arrived() {
                    self.settings.weather_place = Some(place);
                    self.save_settings();
                }
                if self.visible && self.view.scope == Scope::Weather {
                    self.redraw();
                }
                Some(LRESULT(0))
            }
            WM_ORBIT_RATES => {
                self.exchange_rates.arrived(wparam.0 == 1);
                // A conversion still on screen gets its answer, or says why there isn't one.
                if self.visible && orbit_core::currency::parse(&self.read_query()).is_some() {
                    self.refresh_results();
                }
                Some(LRESULT(0))
            }
            WM_ORBIT_LINK_ICONS => {
                // The rows that had none ask again.
                self.icons.clear();
                self.request_visible_icons();
                self.redraw();
                crate::settings_win::link_icons_changed(self.settings.fetch_link_icons);
                Some(LRESULT(0))
            }
            WM_ORBIT_FILES => {
                if let Some(answer) = util::take_posted::<crate::files::FileAnswer>(wparam.0) {
                    self.on_files(*answer);
                }
                Some(LRESULT(0))
            }
            WM_ORBIT_EXTENSION => {
                if let Some(event) = util::take_posted::<crate::extensions::Event>(wparam.0) {
                    self.on_extension(*event);
                }
                Some(LRESULT(0))
            }
            WM_ORBIT_EVERYTHING_INSTALLED => {
                if let Some(result) = util::take_posted::<Result<(), String>>(wparam.0) {
                    self.on_everything_installed(*result);
                }
                Some(LRESULT(0))
            }
            WM_ORBIT_EXTENSIONS_CHANGED => {
                let change = util::take_posted::<crate::extensions::Change>(wparam.0);
                self.on_extensions_changed(change.map(|change| *change));
                Some(LRESULT(0))
            }
            m if m == self.taskbar_created && m != 0 => {
                tray::add(self.main, WM_ORBIT_TRAY);
                None
            }
            _ => None,
        }
    }

    // --- Window lifecycle -----------------------------------------------------------------

    fn layout_for_dpi(&mut self, dpi: u32) {
        let changed = self.fonts.is_none() || self.dpi != dpi;
        self.dpi = dpi;
        self.view.dpi = dpi;
        if changed {
            let fonts = Fonts::new(dpi);
            if let Some(fonts) = &fonts {
                unsafe {
                    SendMessageW(self.edit, WM_SETFONT, Some(WPARAM(fonts.edit.0 as usize)), Some(LPARAM(1)));
                }
            }
            self.fonts = fonts;
            self.icons.clear();
            self.backbuffer = None;
            self.frame = None;
            self.logo = ui::logo_pixels(dpi);
            self.emoji_cache.clear();
            // A picture scaled for the old size is asked for again.
            self.drop_clip_preview();
            if self.menu.is_none() {
                self.update_clip_preview();
            }
        }
        self.edit_x = i32::MIN;
        self.place_edit(ui::input_left(&self.view.scope, dpi));
        let (win_width, win_height) = ui::frame_size(dpi, ui::window_size(dpi, self.full_rows() as i32));
        self.window_height = win_height;
        unsafe {
            let _ = SetWindowPos(self.main, None, 0, 0, win_width, win_height, SWP_NOMOVE | SWP_NOZORDER | SWP_NOACTIVATE);
        }
    }

    /// Moves the search box to where its text is drawn, and sizes it to fit.
    pub fn place_edit(&mut self, x: i32) {
        if x == self.edit_x {
            return;
        }
        self.edit_x = x;
        let height = ui::scale(30, self.dpi);
        let margin = ui::frame_margin(self.dpi);
        let y = margin + (ui::scale(ui::INPUT_HEIGHT, self.dpi) - height) / 2;
        let width = ui::scale(ui::BASE_WIDTH, self.dpi) - x - ui::scale(16, self.dpi);
        unsafe {
            let _ = SetWindowPos(self.edit, None, margin + x, y, width, height, SWP_NOZORDER | SWP_NOACTIVATE);
            // Text starts exactly at the box's edge, so it lines up with what's under it.
            SendMessageW(self.edit, EM_SETMARGINS, Some(WPARAM((EC_LEFTMARGIN | EC_RIGHTMARGIN) as usize)), Some(LPARAM(0)));
        }
    }

    /// Reads what the search box holds and where it would show it.
    pub fn read_field(&self) -> ui::Field {
        let (mut start, mut end) = (0u32, 0u32);
        unsafe {
            SendMessageW(self.edit, EM_GETSEL, Some(WPARAM(&mut start as *mut u32 as usize)), Some(LPARAM(&mut end as *mut u32 as isize)));
        }
        let text = self.read_query();
        ui::Field {
            selection: (start as usize, end as usize),
            text,
        }
    }

    /// Redraws if the caret or selection has moved since the last frame, and shows the caret.
    fn on_field_moved(&mut self) {
        let field = if self.area_focused() { self.read_area_field() } else { self.read_field() };
        let now = (field.selection.0, field.selection.1);
        if now != self.drawn_field {
            self.drawn_field = now;
            self.restart_caret();
            self.redraw();
        }
    }

    /// Shows the caret for a full blink. Windows' blink setting decides whether it blinks at all.
    fn restart_caret(&mut self) {
        self.caret_on = true;
        let blink = unsafe { GetCaretBlinkTime() };
        unsafe {
            if self.visible && blink != 0 && blink != u32::MAX {
                let _ = SetTimer(Some(self.main), TIMER_CARET, blink, None);
            } else {
                let _ = KillTimer(Some(self.main), TIMER_CARET);
            }
        }
    }

    /// Whether the pointer is over text that can be typed in, where it's an I-beam.
    fn pointer_in_input(&self) -> bool {
        if !self.visible || self.hovered_button.is_some() {
            return false;
        }
        let mut point = POINT::default();
        unsafe {
            let _ = GetCursorPos(&mut point);
            let _ = ScreenToClient(self.main, &mut point);
        }
        let margin = ui::frame_margin(self.dpi);
        let (x, y) = (point.x - margin, point.y - margin);
        self.text_spots.iter().any(|spot| spot.contains(x, y))
    }

    /// A press on text that can be typed in puts the caret where it landed, and a drag from there
    /// selects, as in any text box. A double press selects the word, and Shift stretches what's
    /// selected to it. Gives back whether it was on such text.
    fn press_text(&mut self, x: i32, y: i32, double: bool, shift: bool) -> bool {
        let Some(spot) = self.text_spots.iter().find(|spot| spot.contains(x, y)).cloned() else { return false };
        let Some(at) = self.fonts.as_ref().map(|fonts| spot.index_at(fonts, x, y)) else { return false };
        // Typing moves to the box pressed, keeping what was typed in the one it leaves.
        match spot.of {
            ui::SpotOf::LinkArg(index) if self.link_args.as_ref().is_some_and(|args| args.at != index) => self.focus_link_arg(index),
            ui::SpotOf::Field(index) if self.form.as_ref().is_some_and(|form| form.at != index) => self.focus_form_field(index),
            _ => {}
        }
        if VIM_NORMAL.load(Ordering::Relaxed) {
            // Clicking in the box means you want to type there.
            self.set_vim_mode(VimMode::Insert);
        }
        let target = if self.area_focused() { self.area } else { self.edit };
        let (mut start, mut end) = (0u32, 0u32);
        unsafe {
            SendMessageW(target, EM_GETSEL, Some(WPARAM(&mut start as *mut u32 as usize)), Some(LPARAM(&mut end as *mut u32 as isize)));
        }
        let (start, end) = (start as usize, end as usize);
        let (anchor, caret) = if double {
            let units: Vec<u16> = spot.text.encode_utf16().collect();
            crate::forms::word_around(&units, at)
        } else if shift {
            // From whichever end of the selection is further from the press.
            (if at.abs_diff(start) > at.abs_diff(end) { start } else { end }, at)
        } else {
            (at, at)
        };
        unsafe {
            let _ = SetFocus(Some(target));
            SendMessageW(target, EM_SETSEL, Some(WPARAM(anchor)), Some(LPARAM(caret as isize)));
            SetCapture(self.main);
        }
        self.text_drag = Some((anchor, spot.of));
        self.drawn_field = (anchor.min(caret), anchor.max(caret));
        self.restart_caret();
        self.redraw();
        true
    }

    /// Dragging after a press in a box selects from where the press was to the pointer.
    fn drag_text(&mut self, x: i32, y: i32) {
        let Some((anchor, of)) = self.text_drag else { return };
        let Some(spot) = self.text_spots.iter().find(|spot| spot.of == of) else { return };
        let Some(at) = self.fonts.as_ref().map(|fonts| spot.index_at(fonts, x, y)) else { return };
        let target = if self.area_focused() { self.area } else { self.edit };
        unsafe {
            SendMessageW(target, EM_SETSEL, Some(WPARAM(anchor)), Some(LPARAM(at as isize)));
        }
        let now = (anchor.min(at), anchor.max(at));
        if now != self.drawn_field {
            self.drawn_field = now;
            self.restart_caret();
            self.redraw();
        }
    }

    fn end_text_drag(&mut self) {
        self.text_drag = None;
        unsafe {
            let _ = ReleaseCapture();
        }
    }

    /// A mouse message's position, measured from the launcher's corner rather than its shadow's.
    fn panel_point(&self, lparam: LPARAM) -> (i32, i32) {
        let margin = ui::frame_margin(self.dpi);
        ((lparam.0 & 0xFFFF) as i16 as i32 - margin, ((lparam.0 >> 16) & 0xFFFF) as i16 as i32 - margin)
    }

    /// The tallest the list gets, as Settings has it. A small menu shortens `view.visible_rows`
    /// while it's open, so the window's size doesn't come from there.
    fn full_rows(&self) -> usize {
        self.settings.launcher.max_visible_rows.clamp(3, 12) as usize
    }

    /// Lines the blur up under the launcher and shapes it to match. Called whenever the launcher's
    /// size or position has changed.
    fn sync_plate(&mut self) {
        let Some(plate) = self.plate else { return };
        unsafe {
            if !self.glass || !IsWindowVisible(self.main).as_bool() {
                let _ = ShowWindow(plate, SW_HIDE);
                return;
            }
            let Some(panel) = self.backbuffer.as_ref().map(|buffer| (buffer.width, buffer.height)) else { return };
            let mut rect = RECT::default();
            let _ = GetWindowRect(self.main, &mut rect);
            let margin = ui::frame_margin(self.dpi);
            // The compositor's blur takes the launcher's shape exactly. Windows' own rounds less
            // than the launcher's corners, so it's drawn a little inside them, where its corners
            // stay hidden behind the launcher's.
            let inset = if self.frost.is_some() { 0 } else { plate_inset(ui::window_radius(self.dpi), ui::scale(PLATE_CORNER_DIP, self.dpi)) };
            let (width, height) = (panel.0 - inset * 2, panel.1 - inset * 2);
            if let Some(frost) = self.frost.as_mut() {
                frost.fit(width, height, ui::window_radius(self.dpi));
            }
            if self.plate_size != (width, height) {
                self.plate_size = (width, height);
                // Earlier versions cut the window to shape, and Windows won't round one that's cut.
                let _ = SetWindowRgn(plate, None, true);
            }
            let _ = SetWindowPos(plate, Some(self.main), rect.left + margin + inset, rect.top + margin + inset, width, height, SWP_NOACTIVATE | SWP_SHOWWINDOW);
        }
    }

    /// Puts the blur behind the launcher, or takes it away, to match the settings and Windows.
    pub fn apply_glass(&mut self) {
        let glass = self.settings.launcher.glass && ui::glass_is_allowed();
        let dark = self.theme.overlay == crate::gfx::Rgb(255, 255, 255);
        let wanted = (glass, dark, self.theme);
        if self.glass_applied == Some(wanted) {
            return;
        }
        self.glass_applied = Some(wanted);
        self.glass = glass;
        unsafe {
            let dark_title = BOOL::from(dark);
            let _ = DwmSetWindowAttribute(self.main, DWMWA_USE_IMMERSIVE_DARK_MODE, &dark_title as *const _ as *const _, std::mem::size_of::<BOOL>() as u32);
        }
        if glass && self.plate.is_none() {
            match create_frosted_plate() {
                Some((plate, frost)) => {
                    self.plate = Some(plate);
                    self.frost = Some(frost);
                }
                None => self.plate = create_plate(),
            }
        }
        if let (Some(plate), None) = (self.plate, &self.frost) {
            ui::set_backdrop(plate, glass.then_some(self.backdrop), dark);
        }
        self.sync_plate();
    }

    fn refresh_theme(&mut self) {
        let theme = ui::resolve_theme(self.settings.appearance);
        let changed = theme != self.theme;
        self.theme = theme;
        self.apply_glass();
        if changed {
            ui::publish_edit_colors(&theme);
        }
        if self.visible {
            self.redraw();
        }
    }

    /// Places the window on the screen the setting says, the one the user is working on unless
    /// they chose otherwise, in the upper part of the screen.
    fn position_window(&mut self) -> (i32, i32) {
        unsafe {
            let monitor = crate::displays::launcher_monitor(&self.settings.launcher.screen, self.prev_foreground);
            let mut info = MONITORINFO { cbSize: std::mem::size_of::<MONITORINFO>() as u32, ..Default::default() };
            let _ = GetMonitorInfoW(monitor, &mut info);
            let area = info.rcWork;
            let mut dpi_x = 96;
            let mut dpi_y = 96;
            let _ = windows::Win32::UI::HiDpi::GetDpiForMonitor(
                monitor,
                windows::Win32::UI::HiDpi::MDT_EFFECTIVE_DPI,
                &mut dpi_x,
                &mut dpi_y,
            );
            if dpi_x != self.dpi {
                self.layout_for_dpi(dpi_x);
            }
            let (width, height) = ui::window_size(self.dpi, self.full_rows() as i32);
            let x = area.left + ((area.right - area.left) - width) / 2;
            let free = (area.bottom - area.top) - height;
            let y = area.top + (free / 3).max(0);
            // The launcher goes there, and the window around it reaches further by its shadow.
            let margin = ui::frame_margin(self.dpi);
            (x - margin, y - margin)
        }
    }

    fn toggle(&mut self, scope: Scope, source: &str, t0: i64) {
        if self.visible && self.view.scope == scope && self.view.stack.is_empty() {
            self.hide(true);
        } else {
            self.show(scope, source, t0);
        }
    }

    /// The hot path. See docs/PERFORMANCE_BUDGET.md for what each timestamp means.
    pub fn show(&mut self, scope: Scope, source: &str, t0: i64) {
        self.running_apps = None;
        let mut times = ShowTimes { t0, ..Default::default() };
        unsafe {
            if !self.visible {
                let foreground = GetForegroundWindow();
                self.prev_foreground = if foreground == self.main { HWND::default() } else { foreground };
            }
            let memory_tight = self.check_memory_pressure();
            if self.index_owed && !memory_tight {
                self.index_owed = false;
                self.start_indexer(false);
            }
            let (x, y) = self.position_window();
            self.status = None;
            self.view.stack.clear();
            self.menu = None;
            self.view.visible_rows = self.full_rows();
            let keep_query = self.settings.launcher.preserve_query && scope == Scope::Root && self.view.scope == Scope::Root;
            let reopen = scope == Scope::Root;
            self.set_scope(scope, keep_query);
            if let Some((form, parked_at)) = self.parked_form.take().filter(|_| reopen) {
                if parked_at.elapsed().as_secs() < FORM_KEPT_SECS && !form.saving {
                    self.enter_form(form);
                }
            }
            self.hovered_button = None;
            self.pressed_button = None;
            ui::render(self);
            times.t1 = qpc();
            let (width, full_height) = ui::frame_size(self.dpi, ui::window_size(self.dpi, self.full_rows() as i32));
            let (width, height) = self.frame.as_ref().map_or((width, full_height), |frame| (frame.width, frame.height));
            self.window_height = height;
            // Its picture is in place before it appears, so it never shows an empty window.
            if let Some(frame) = &self.frame {
                frame.present(self.main, Some((x, y)));
            }
            let _ = SetWindowPos(
                self.main,
                Some(HWND_TOPMOST),
                x,
                y,
                width,
                height,
                SWP_SHOWWINDOW | SWP_NOREDRAW,
            );
            self.visible = true;
            self.sync_plate();
            times.t2 = qpc();
            if !SetForegroundWindow(self.main).as_bool() {
                launch::force_foreground(self.main);
            }
            self.focus_input();
            if keep_query {
                SendMessageW(self.edit, EM_SETSEL, Some(WPARAM(0)), Some(LPARAM(-1)));
            }
            if self.settings.launcher.keyboard == KeyboardMode::Vim {
                self.sync_vim_to_scope();
            }
            if GetForegroundWindow() == self.main && GetFocus() == self.edit {
                times.t3 = qpc();
            }
            if self.metrics.bench_mode {
                let _ = DwmFlush();
                times.t2_composed = qpc();
            }
            let _ = KillTimer(Some(self.main), TIMER_TRIM);
        }
        self.restart_caret();
        self.metrics.record_show(source, times);
        if self.preview.is_none() && self.link_icons_at.is_some_and(|at| at.elapsed() >= LINK_ICONS_AGAIN) {
            self.ask_every_link_icon();
        }
    }

    pub fn hide(&mut self, restore_focus: bool) {
        self.end_recording();
        self.waiting_value = None;
        self.hold_open = None;
        self.lost_focus_while_held = false;
        if !self.visible {
            return;
        }
        if self.form.is_some() && self.menu.is_none() {
            self.store_form_value();
        }
        if let Some(mut form) = self.form.take() {
            form.hide_after = false;
            self.parked_form = Some((form, Instant::now()));
        }
        self.visible = false;
        let behind = self.menu.take().map(|menu| menu.behind.scope);
        self.view.visible_rows = self.full_rows();
        if matches!(self.view.scope, Scope::Vault(_) | Scope::Emoji) || matches!(behind, Some(Scope::Vault(_) | Scope::Emoji)) {
            // A list of what's in someone's vault isn't left lying around to come back with, and the
            // emoji list is big enough to be worth letting go of.
            self.view.scope = Scope::Root;
            self.view.rows.clear();
            self.view.static_rows.clear();
            self.view.grid = None;
            self.emoji_table = Vec::new();
            self.emoji_cache = HashMap::new();
            self.view.last_query = Some(String::new());
            self.set_query_text("");
        }
        unsafe {
            let _ = KillTimer(Some(self.main), TIMER_CARET);
            if let Some(plate) = self.plate {
                let _ = ShowWindow(plate, SW_HIDE);
            }
            let _ = ShowWindow(self.main, SW_HIDE);
            if restore_focus && !self.prev_foreground.is_invalid() && IsWindow(Some(self.prev_foreground)).as_bool() {
                let _ = SetForegroundWindow(self.prev_foreground);
            }
        }
        self.view.stack.clear();
        self.drop_clip_preview();
        // The weather keeps its forecast for 15 minutes, but not its pictures, which are quick to redraw.
        self.weather.icons = Vec::new();
        self.link_args = None;
        // The frame costs a few megabytes and takes well under a millisecond to recreate.
        self.backbuffer = None;
        self.frame = None;
        if let Some(delay) = self.trim_after_ms {
            unsafe {
                let _ = SetTimer(Some(self.main), TIMER_TRIM, delay, None);
            }
        }
        self.arm_extension_idle();
    }

    fn trim_working_set(&mut self) {
        unsafe {
            let _ = SetProcessWorkingSetSizeEx(
                GetCurrentProcess(),
                usize::MAX,
                usize::MAX,
                windows::Win32::System::Memory::SETPROCESSWORKINGSETSIZEEX_FLAGS(0),
            );
        }
    }

    /// Checks the system's memory load when the launcher is used. There is no background polling.
    fn check_memory_pressure(&mut self) -> bool {
        let mut status = MEMORYSTATUSEX { dwLength: std::mem::size_of::<MEMORYSTATUSEX>() as u32, ..Default::default() };
        let tight = unsafe { GlobalMemoryStatusEx(&mut status) }.is_ok() && status.dwMemoryLoad >= 92;
        if tight != self.shed_active {
            self.shed_active = tight;
            self.apply_cache_limits();
            if tight {
                self.vaults.clear();
                self.worker.send(Req::Shed);
            }
        }
        tight
    }

    fn apply_cache_limits(&mut self) {
        let small = self.settings.low_resource_mode || self.shed_active;
        self.icons.set_capacity(if small { 16 } else { 64 });
    }

    fn on_paint(&mut self) {
        unsafe {
            let mut paint = PAINTSTRUCT::default();
            let _ = BeginPaint(self.main, &mut paint);
            let _ = EndPaint(self.main, &paint);
        }
        // Windows keeps a layered window's picture itself, so this only matters if it was let go.
        if self.frame.is_none() && self.visible {
            self.redraw();
        }
    }

    pub fn redraw(&mut self) {
        if !self.visible {
            return;
        }
        ui::render(self);
        let Some(frame) = &self.frame else { return };
        // The top stays put, and the bottom follows the results.
        frame.present(self.main, None);
        if frame.height != self.window_height {
            self.window_height = frame.height;
            self.sync_plate();
        }
    }

    fn on_timer(&mut self, id: usize) {
        unsafe {
            let _ = KillTimer(Some(self.main), id);
        }
        match id {
            TIMER_TRIM => {
                if !self.visible {
                    self.trim_working_set();
                }
            }
            TIMER_EXIT => unsafe { PostQuitMessage(0) },
            TIMER_CARET => {
                if self.visible {
                    self.caret_on = !self.caret_on;
                    let blink = unsafe { GetCaretBlinkTime() };
                    unsafe {
                        let _ = SetTimer(Some(self.main), TIMER_CARET, blink, None);
                    }
                    self.redraw();
                }
            }
            TIMER_PREVIEW => self.preview_step(),
            TIMER_CLEAR_SECRET => self.clear_secret_now(),
            TIMER_COUNTDOWN => {
                let ended = self.countdown.as_mut().is_some_and(|timer| timer.check_done());
                if ended {
                    let length = self.countdown.as_ref().map_or(0, |timer| timer.length);
                    unsafe {
                        let _ = windows::Win32::System::Diagnostics::Debug::MessageBeep(windows::Win32::UI::WindowsAndMessaging::MB_ICONASTERISK);
                    }
                    self.notify("Timer done", &format!("The timer for {} has finished.", orbit_core::timer::describe(length)));
                    if self.visible {
                        self.refresh_results();
                        self.redraw();
                    }
                } else {
                    self.arm_countdown();
                }
            }
            TIMER_CLOCK => {
                if self.visible {
                    self.redraw();
                }
                self.tick_clock();
            }
            TIMER_FORGET_VAULTS => {
                if matches!(self.view.scope, Scope::Vault(_)) && self.visible {
                    unsafe {
                        let _ = SetTimer(Some(self.main), TIMER_FORGET_VAULTS, VAULT_KEEP_MS, None);
                    }
                } else {
                    self.vaults.clear();
                }
            }
            TIMER_RECLAIM => {
                crate::editor::release_rich_edit();
                release_free_heap();
            }
            TIMER_EXT_IDLE => self.on_extension_idle(),
            TIMER_FILES_RETRY => self.on_files_retry(),
            TIMER_UPDATE_CHECK => crate::update::on_check_timer(self.settings.check_updates),
            TIMER_UPDATE_SETUP => crate::update::on_setup_timer(),
            _ => {}
        }
    }

    /// Draws one frame to a file. The window stays hidden throughout, so nothing takes focus.
    fn preview_step(&mut self) {
        let Some(mut preview) = self.preview.take() else { return };
        self.visible = true;
        match preview.step {
            0 => {
                self.view.last_query = None;
                self.set_query_text(&preview.query.clone());
                self.set_scope(Scope::Root, true);
                match preview.scope.as_deref() {
                    Some("clipboard") => self.set_scope(Scope::Clipboard, true),
                    Some("link-args") => {
                        self.enter_link_args("Weather".into(), "https://weather.example/{city}/{days=3}?units={units=metric}".into(), vec!["Lisbon".into()]);
                    }
                    Some("form-link") => {
                        let link = orbit_core::settings::QuickLink { id: String::new(), name: "Weather".into(), url: "weather.example/{city}?days={days=3}".into(), alias: "w".into(), open_with: String::new() };
                        let mut form = InlineForm::link(Some(link));
                        form.at = 1;
                        self.enter_form(form);
                    }
                    Some("form-problem") => {
                        let mut form = InlineForm::link(None);
                        form.fields[0].value = "Wiki".into();
                        self.enter_form(form);
                        self.save_form();
                    }
                    Some("form-snippet") => {
                        let mut form = InlineForm::snippet(None);
                        form.fields[0].value = "Email sign-off".into();
                        form.fields[1].value = "Thanks for getting in touch. I will look at this properly and come back to you before the end of the week, with everything you asked about.\n\nBest wishes,\nSam\nSent {date}".into();
                        form.fields[2].value = ";sig".into();
                        form.at = 1;
                        self.enter_form(form);
                    }
                    Some("clipboard-sample") => {
                        self.add_sample_clips();
                        self.set_scope(Scope::Clipboard, true);
                    }
                    Some("1password") => self.set_scope(Scope::Vault(Tool::OnePassword), true),
                    Some("ente") => self.set_scope(Scope::Vault(Tool::EnteAuth), true),
                    Some("emoji") => self.set_scope(Scope::Emoji, true),
                    Some("links") => self.set_scope(Scope::Links, true),
                    Some("orbit-menu") => self.open_orbit_menu(),
                    // A saved run, `speedtest-preview.jsonl` in the data folder. Never the real program.
                    Some("speedtest") | Some("speedtest-terms") => {
                        self.settings.speedtest_accepted = preview.scope.as_deref() == Some("speedtest");
                        self.enter_scope(Scope::SpeedTest, qpc());
                        let lines = std::fs::read_to_string(self.dir.join("speedtest-preview.jsonl")).unwrap_or_default();
                        self.speedtest.test = orbit_core::speedtest::Test::starting();
                        for line in lines.lines() {
                            self.speedtest.test.read(line);
                        }
                        self.refresh_results();
                    }
                    Some("weather") => {
                        self.enter_scope(Scope::Weather, qpc());
                        self.weather_look_up("");
                    }
                    // Made-up times and a made-up colour, for drawing these views.
                    Some("timer") => {
                        self.countdown = Some(crate::clock::Countdown::with_left(300, 192));
                        self.enter_scope(Scope::Timer, qpc());
                    }
                    Some("timer-paused") => {
                        let mut timer = crate::clock::Countdown::with_left(1500, 1100);
                        timer.pause();
                        self.countdown = Some(timer);
                        self.enter_scope(Scope::Timer, qpc());
                    }
                    Some("timer-new") => self.enter_scope(Scope::Timer, qpc()),
                    Some("stopwatch") => {
                        self.stopwatch = crate::clock::Stopwatch::from_elapsed(83);
                        self.enter_scope(Scope::Stopwatch, qpc());
                    }
                    Some("colour") => self.enter_scope(Scope::Colour(orbit_core::colour::Colour { r: 56, g: 189, b: 248 }), qpc()),
                    // An extension's list, as "ext:github/issues". One that comes with Orbit reads
                    // saved answers from the data folder instead of asking online.
                    // "?filter=review" after it chooses a filter first.
                    Some(list) if list.starts_with("ext:") => {
                        let (list, options) = list.split_once('?').unwrap_or((list, ""));
                        if let Some((ext, command)) = list["ext:".len()..].split_once('/') {
                            let (ext, command) = (ext.to_string(), command.to_string());
                            if let Some(id) = options.split('&').find_map(|option| option.strip_prefix("filter=")) {
                                self.ext_filter.insert(format!("{ext}/{command}"), id.to_string());
                            }
                            self.open_extension_command(&ext, &command);
                        }
                    }
                    _ => {}
                }
                // A scope's rows arrive from the data thread, so its menu waits for them.
                if preview.actions && preview.scope.is_none() {
                    self.open_action_menu();
                }
                self.request_visible_icons();
            }
            1 => {
                // A row picked lower down, in a scope or on the home list.
                if self.menu.is_none() && (preview.scope.is_some() || preview.select > 0) {
                    self.view.select(preview.select);
                    self.after_selection_change();
                    // "&menu" after an extension's list opens its filter drop-down.
                    let filters = preview.scope.as_deref().is_some_and(|scope| scope.starts_with("ext:") && scope.split(['?', '&']).any(|option| option == "menu"));
                    if filters {
                        self.open_filter_menu();
                    } else if preview.actions {
                        self.open_action_menu();
                    }
                }
                self.request_visible_icons();
            }
            _ => {
                self.status = None;
                ui::render(self);
                if let Some(frame) = &self.frame {
                    let _ = frame.write_raw(&preview.out);
                }
                unsafe { PostQuitMessage(0) };
                return;
            }
        }
        preview.step += 1;
        self.preview = Some(preview);
        unsafe {
            let _ = SetTimer(Some(self.main), TIMER_PREVIEW, 500, None);
        }
    }

    // --- Hotkeys and input ----------------------------------------------------------------

    fn on_hotkey(&mut self, id: i32, t0: i64) {
        if self.suspended {
            return;
        }
        if id == HOTKEY_LAUNCHER {
            // A game can fill the screen long after it came to the front, which nothing announces.
            // This press is gone, but the next one reaches it.
            if !self.visible && self.launcher_should_stand_aside() {
                self.update_launcher_aside();
                return;
            }
            self.toggle(Scope::Root, "hotkey", t0);
            return;
        }
        if let Some(action) = self.shortcut_actions.iter().find(|(key, _)| *key == id).map(|(_, a)| a.clone()) {
            self.run_hotkey_action(&action);
        }
    }

    /// A hotkey for an app is a toggle: pressed while that app is in front, it minimizes it.
    fn run_hotkey_action(&mut self, action: &Action) {
        if let Action::FocusOrLaunchApp { id, name } = action {
            if self.settings.launcher.hotkey_toggles_app && !self.visible {
                let app = self.catalog.app_by_id(id).or_else(|| self.catalog.app_by_name(name));
                if app.is_some_and(launch::minimize_if_in_front) {
                    self.metrics.record_action("hotkey-minimized", 0, 0);
                    return;
                }
            }
        }
        self.run_action(action, None);
    }

    fn on_hyper(&mut self, code: u8, t0: i64) {
        if self.settings_recording {
            if let Some(name) = keys::name_from_vk(code) {
                crate::settings_win::recorded_hyper(&name);
            }
            return;
        }
        if self.recording.is_some() {
            if let Some(name) = keys::name_from_vk(code) {
                self.capture_hotkey(Hotkey::Hyper(name));
            }
            return;
        }
        if let Some(action) = self.hyper_actions.iter().find(|(key, _)| *key == code).map(|(_, a)| a.clone()) {
            // Show paths use the hook's timestamp, so hook-to-window is measured from the event.
            match action {
                Action::Builtin { command: Builtin::ToggleLauncher } => self.toggle(Scope::Root, "hyper", t0),
                Action::Builtin { command: Builtin::ClipboardHistory } => self.toggle(Scope::Clipboard, "hyper", t0),
                Action::Builtin { command: Builtin::SnippetSearch } => self.toggle(Scope::Snippets, "hyper", t0),
                Action::Builtin { command: Builtin::SearchNotes } => self.toggle(Scope::Notes, "hyper", t0),
                other => self.run_hotkey_action(&other),
            }
        }
    }

    pub fn register_hotkeys(&mut self) {
        self.register_hotkeys_now();
        // What's in front right now may be something the new hotkey should leave alone.
        self.launcher_aside = false;
        self.apply_foreground_events();
        self.update_launcher_aside();
        if self.settings_recording {
            self.release_hotkeys();
        }
    }

    /// Lets go of every shortcut for now, keeping the list of them for later.
    fn release_hotkeys(&self) {
        unsafe {
            for id in &self.registered_hotkeys {
                let _ = UnregisterHotKey(Some(self.main), *id);
            }
        }
    }

    /// Settings is listening for keys. Registered shortcuts would never reach it.
    fn set_settings_recording(&mut self, on: bool) {
        if on == self.settings_recording {
            return;
        }
        self.settings_recording = on;
        if let Some(hook) = &self.hook {
            hook.set_recording(on);
        }
        if on {
            self.release_hotkeys();
        } else if self.recording.is_none() {
            self.register_hotkeys();
        }
    }

    fn register_hotkeys_now(&mut self) {
        unsafe {
            for id in self.registered_hotkeys.drain(..) {
                let _ = UnregisterHotKey(Some(self.main), id);
            }
        }
        self.hotkey_problems.clear();
        self.launcher_chord = None;
        self.shortcut_actions.clear();
        self.active_hotkey.clear();
        self.hyper_actions.clear();
        // Registered shortcuts would keep eating their keys even with nothing listening. While
        // suspended they go back to whichever app has focus, and the tray icon is the way back.
        if self.suspended {
            return;
        }

        let chosen = self.hotkey_override.clone().or_else(|| self.settings.launcher.hotkey.clone());
        let mut candidates: Vec<Chord> = chosen.iter().cloned().collect();
        for text in DEFAULT_HOTKEYS {
            if let Ok(chord) = Chord::parse(text) {
                if !candidates.contains(&chord) {
                    candidates.push(chord);
                }
            }
        }
        let mut used_fallback = false;
        for (i, chord) in candidates.iter().enumerate() {
            if self.try_register(HOTKEY_LAUNCHER, chord).is_ok() {
                self.active_hotkey = chord.display("Win");
                self.launcher_chord = Some(chord.clone());
                if i > 0 {
                    used_fallback = true;
                    self.hotkey_problems.push(format!(
                        "{} is in use by another program, so Orbit is using {}.",
                        candidates[0].display("Win"),
                        self.active_hotkey
                    ));
                }
                break;
            }
        }
        if self.active_hotkey.is_empty() {
            self.hotkey_problems.push("No launcher shortcut could be registered. Use the tray icon or choose one in Settings.".into());
        }
        if used_fallback || self.active_hotkey.is_empty() {
            let free = self.free_alternatives();
            if !free.is_empty() {
                self.hotkey_problems.push(format!("Free right now: {}.", free.join(", ")));
            }
        }

        let shortcuts = self.settings.shortcuts.clone();
        for (i, binding) in shortcuts.iter().enumerate().filter(|(_, b)| b.enabled) {
            let id = HOTKEY_SHORTCUT_BASE + i as i32;
            match self.try_register(id, &binding.chord) {
                Ok(()) => self.shortcut_actions.push((id, binding.action.clone())),
                Err(why) => self.hotkey_problems.push(format!("{}: {why}", binding.chord.display("Win"))),
            }
        }

        let bindings = self.settings.hyper.bindings.clone();
        if self.settings.hyper.enabled {
            for (i, binding) in bindings.iter().enumerate().filter(|(_, b)| b.enabled) {
                let Some(code) = keys::vk_from_name(&binding.key) else { continue };
                self.hyper_actions.push((code, binding.action.clone()));
                if self.settings.hyper.mode == HyperMode::External {
                    let id = HOTKEY_HYPER_BASE + i as i32;
                    match Chord::new(Mods::HYPER, &binding.key).map_err(|e| e.to_string()).and_then(|c| self.try_register(id, &c)) {
                        Ok(()) => self.shortcut_actions.push((id, binding.action.clone())),
                        Err(why) => self.hotkey_problems.push(format!("Hyper+{}: {why}", binding.key)),
                    }
                }
            }
            for key in self.settings.duplicate_hyper_keys() {
                self.hotkey_problems.push(format!("Hyper+{key} is bound more than once. The first binding is used."));
            }
        }
    }

    /// Shortcuts that could be registered at this moment, offered when the chosen one couldn't.
    fn free_alternatives(&mut self) -> Vec<String> {
        const IDEAS: &[&str] = &["Ctrl+Alt+Space", "Ctrl+Shift+Space", "Alt+Shift+Space", "Ctrl+Alt+O", "Ctrl+Alt+K", "Alt+F1"];
        const PROBE_ID: i32 = 0xBFFE;
        let mut free = Vec::new();
        for text in IDEAS {
            let Ok(chord) = Chord::parse(text) else { continue };
            let display = chord.display("Win");
            if display == self.active_hotkey || is_reserved_on_windows(&chord) {
                continue;
            }
            let Some((mods, vk)) = keys::chord_to_registration(&chord) else { continue };
            unsafe {
                if RegisterHotKey(Some(self.main), PROBE_ID, mods, vk).is_ok() {
                    let _ = UnregisterHotKey(Some(self.main), PROBE_ID);
                    free.push(display);
                }
            }
            if free.len() == 3 {
                break;
            }
        }
        free
    }

    /// Tells the user about a shortcut problem once, not at every start. Settings and Diagnostics
    /// keep showing it for as long as it lasts.
    fn announce_hotkey_problems(&mut self) {
        let text = self.hotkey_problems.join(" ");
        if text == self.settings.hotkey_notice {
            return;
        }
        self.settings.hotkey_notice = text.clone();
        self.save_settings();
        if !text.is_empty() {
            self.notify("Orbit shortcuts", &text);
        }
    }

    /// Whether the launcher's hotkey should go to the program in front, because Settings names that
    /// program, or because it's a game. Only asked when the front window changes or the key's pressed.
    fn launcher_should_stand_aside(&mut self) -> bool {
        let launcher = &self.settings.launcher;
        if launcher.excluded_apps.is_empty() && !launcher.pass_through_in_games {
            return false;
        }
        let foreground = unsafe { GetForegroundWindow() };
        if foreground.is_invalid() || foreground == self.main {
            return false;
        }
        let mut pid = 0;
        unsafe { GetWindowThreadProcessId(foreground, Some(&mut pid)) };
        let path = util::process_image_path(pid).unwrap_or_default();
        if Settings::is_app_excluded(&launcher.excluded_apps, util::exe_name(&path)) {
            return true;
        }
        if !launcher.pass_through_in_games {
            return false;
        }
        // The same rule as HyperKey: filling the screen is required, because Windows' list of games
        // can hold ordinary programs, and those keep the shortcut in a normal window.
        if !hook::foreground_is_fullscreen() {
            return false;
        }
        if self.known_games_at.is_none_or(|at| at.elapsed().as_secs() >= 60) {
            self.known_games_at = Some(Instant::now());
            self.known_games = crate::game::windows_game_list();
        }
        crate::game::is_game_path(&path.to_lowercase(), &self.known_games) || crate::game::exclusive_fullscreen()
    }

    /// Gives the launcher's hotkey to the program in front, or takes it back. A registered hotkey
    /// never reaches anything else, so standing aside means letting go of it.
    fn update_launcher_aside(&mut self) {
        if self.suspended || self.recording.is_some() || self.settings_recording || self.launcher_chord.is_none() {
            return;
        }
        let aside = self.launcher_should_stand_aside();
        if aside == self.launcher_aside {
            return;
        }
        self.launcher_aside = aside;
        if aside {
            unsafe {
                let _ = UnregisterHotKey(Some(self.main), HOTKEY_LAUNCHER);
            }
            self.registered_hotkeys.retain(|id| *id != HOTKEY_LAUNCHER);
        } else if let Some(chord) = self.launcher_chord.clone() {
            // Something else may have taken it meanwhile, and then the usual fallbacks apply.
            if self.try_register(HOTKEY_LAUNCHER, &chord).is_err() {
                self.register_hotkeys();
            }
        }
    }

    /// Listens for the front window changing only while something depends on it.
    fn apply_foreground_events(&mut self) {
        let launcher = &self.settings.launcher;
        let wanted = !launcher.excluded_apps.is_empty() || launcher.pass_through_in_games;
        unsafe {
            if wanted && self.foreground_events.is_none() {
                FOREGROUND_TARGET.store(self.main.0 as isize, Ordering::Relaxed);
                let hook = SetWinEventHook(EVENT_SYSTEM_FOREGROUND, EVENT_SYSTEM_FOREGROUND, None, Some(on_foreground_event), 0, 0, WINEVENT_OUTOFCONTEXT);
                self.foreground_events = (!hook.is_invalid()).then_some(hook);
            } else if !wanted {
                if let Some(hook) = self.foreground_events.take() {
                    let _ = UnhookWinEvent(hook);
                }
            }
        }
    }

    fn try_register(&mut self, id: i32, chord: &Chord) -> Result<(), String> {
        if is_reserved_on_windows(chord) {
            return Err("reserved by Windows".to_string());
        }
        // Settings refuses these, but a file edited by hand can still hold one.
        if !orbit_hotkeys::chord::is_usable_global(chord) {
            return Err("a plain key would stop typing everywhere".to_string());
        }
        let (mods, vk) = keys::chord_to_registration(chord).ok_or("unknown key")?;
        // A preview only draws, and mustn't take keys from the Orbit that's running.
        if self.previewing {
            return Ok(());
        }
        unsafe { RegisterHotKey(Some(self.main), id, mods, vk) }
            .map_err(|_| "already taken by another program".to_string())?;
        self.registered_hotkeys.push(id);
        Ok(())
    }

    fn triggers_needed(&self) -> bool {
        self.settings.snippets.expansion_enabled
    }

    pub fn apply_hook(&mut self) {
        self.hook = None;
        self.hook_failed = false;
        // A preview only draws. Its keyboard hook would see every key typed while it runs.
        if self.suspended || self.previewing {
            return;
        }
        let config = hook::config_from_settings(&self.settings, self.send, WM_ORBIT_HYPER, WM_ORBIT_EXPAND, Vec::new());
        if let Some(config) = config {
            self.hook = HookHandle::start(config);
            if self.hook.is_none() {
                self.hook_failed = true;
                self.report("Windows refused the keyboard hook, so HyperKey and expansion are off. Direct shortcuts still work.");
            }
            if self.triggers_needed() {
                self.worker.send(Req::SnippetTriggers);
            }
        }
    }

    fn on_session_change(&mut self, event: u32) {
        const WTS_CONSOLE_CONNECT: u32 = 1;
        const WTS_CONSOLE_DISCONNECT: u32 = 2;
        const WTS_REMOTE_CONNECT: u32 = 3;
        const WTS_REMOTE_DISCONNECT: u32 = 4;
        const WTS_SESSION_LOCK: u32 = 7;
        const WTS_SESSION_UNLOCK: u32 = 8;
        match event {
            WTS_SESSION_LOCK | WTS_CONSOLE_DISCONNECT | WTS_REMOTE_DISCONNECT => {
                if let Some(hook) = &self.hook {
                    hook.reset();
                }
                self.hide(false);
                self.vaults.clear();
                self.clear_secret_now();
            }
            WTS_SESSION_UNLOCK | WTS_CONSOLE_CONNECT | WTS_REMOTE_CONNECT => {
                if let Some(hook) = &self.hook {
                    hook.reinstall();
                }
                self.register_hotkeys();
            }
            _ => {}
        }
    }

    fn on_power(&mut self, event: u32) {
        const PBT_APMSUSPEND: u32 = 4;
        const PBT_APMRESUMESUSPEND: u32 = 7;
        const PBT_APMRESUMEAUTOMATIC: u32 = 18;
        match event {
            PBT_APMSUSPEND => {
                if let Some(hook) = &self.hook {
                    hook.reset();
                }
            }
            PBT_APMRESUMESUSPEND | PBT_APMRESUMEAUTOMATIC => {
                if let Some(hook) = &self.hook {
                    hook.reinstall();
                }
                self.register_hotkeys();
            }
            _ => {}
        }
    }

    fn on_key(&mut self, vk: u16, t_key: i64) -> bool {
        let ctrl = unsafe { GetKeyState(VK_CONTROL.0 as i32) } < 0;
        self.on_key_as(vk, ctrl, t_key)
    }

    /// A key pressed, with Ctrl held or not, or a button in the footer clicked, which does just
    /// what its key does wherever it's shown.
    fn on_key_as(&mut self, vk: u16, ctrl: bool, t_key: i64) -> bool {
        if self.recording.is_some() {
            return self.on_recording_key(vk, t_key);
        }
        // Ctrl+comma, as in most apps. It means the same in every keyboard mode.
        if vk == VK_OEM_COMMA.0 && ctrl {
            self.perform_builtin(Builtin::Settings);
            return true;
        }
        if let Some(handled) = self.on_form_key(vk, ctrl) {
            return handled;
        }
        if self.settings.launcher.keyboard != KeyboardMode::Standard {
            if let Some(handled) = self.on_vim_key(vk, ctrl, t_key) {
                return handled;
            }
        }
        if let Some(handled) = self.on_link_args_key(vk, ctrl) {
            return handled;
        }
        // Ctrl+P opens a list's filter drop-down, even when the filter left nothing to choose.
        if ctrl && vk == 0x50 && self.filter_label().is_some() && unsafe { GetKeyState(0x10) } >= 0 {
            self.toggle_filter_menu();
            return true;
        }
        // The keys an action menu shows next to its entries work without opening it.
        if ctrl {
            let shift = unsafe { GetKeyState(0x10) } < 0;
            if let Some(label) = shortcut_label(vk, shift) {
                if self.run_shortcut(&label) {
                    self.metrics.record_action("enter", t_key, qpc());
                    return true;
                }
            }
        }
        match vk {
            v if v == VK_UP.0 => self.view.move_selection(-1),
            v if v == VK_DOWN.0 => self.view.move_selection(1),
            v if v == VK_PRIOR.0 => self.view.move_selection(-(self.view.visible_rows as i32)),
            v if v == VK_NEXT.0 => self.view.move_selection(self.view.visible_rows as i32),
            // Tiles have neighbours both ways, so here the arrows belong to the grid.
            v if v == VK_LEFT.0 && self.view.grid.is_some() => self.view.move_along(-1),
            v if v == VK_RIGHT.0 && self.view.grid.is_some() => self.view.move_along(1),
            v if v == VK_ESCAPE.0 => {
                self.escape();
                return true;
            }
            v if v == VK_RETURN.0 => {
                self.activate_selected(ctrl, t_key);
                return true;
            }
            v if v == VK_TAB.0 => {
                self.open_action_menu();
                return true;
            }
            0x4B if ctrl => {
                self.open_action_menu();
                return true;
            }
            0x41 if ctrl => {
                unsafe {
                    SendMessageW(self.edit, EM_SETSEL, Some(WPARAM(0)), Some(LPARAM(-1)));
                }
                return true;
            }
            v if v == VK_BACK.0 => {
                let empty = unsafe { GetWindowTextLengthW(self.edit) } == 0;
                if empty && !self.view.stack.is_empty() {
                    self.go_back_or_hide();
                    return true;
                }
                if ctrl && !self.is_vim_normal() {
                    self.delete_word_before_caret();
                    return true;
                }
                return false;
            }
            v if v == VK_F4.0 && unsafe { GetKeyState(0x12) } < 0 => {
                self.hide(true);
                return true;
            }
            _ => return false,
        }
        self.after_selection_change();
        self.redraw();
        true
    }

    // --- Vim keys --------------------------------------------------------------------------

    /// Which mode to show, if the launcher is in full Vim.
    pub fn vim_mode(&self) -> Option<VimMode> {
        (self.settings.launcher.keyboard == KeyboardMode::Vim && !matches!(self.view.scope, Scope::SetHotkey { .. })).then_some(self.vim_mode)
    }

    fn is_vim_normal(&self) -> bool {
        self.settings.launcher.keyboard == KeyboardMode::Vim && self.vim_mode == VimMode::Normal
    }

    /// Tab and Shift+Tab move between a quicklink's boxes, and Backspace in an empty one goes back
    /// to the one before. There's no menu here, so Tab and Ctrl+K aren't needed for one.
    fn on_link_args_key(&mut self, vk: u16, ctrl: bool) -> Option<bool> {
        if !matches!(self.view.scope, Scope::LinkArgs { .. }) || self.menu.is_some() {
            return None;
        }
        let (at, count) = self.link_args.as_ref().map(|args| (args.at, args.params.len()))?;
        let shift = unsafe { GetKeyState(0x10) } < 0;
        if vk == VK_TAB.0 {
            let to = if shift { at.checked_sub(1) } else { (at + 1 < count).then_some(at + 1) };
            if let Some(to) = to {
                self.focus_link_arg(to);
            }
            return Some(true);
        }
        if vk == 0x4B && ctrl {
            return Some(true);
        }
        if vk == VK_BACK.0 && at > 0 && !ctrl && unsafe { GetWindowTextLengthW(self.edit) } == 0 {
            self.focus_link_arg(at - 1);
            return Some(true);
        }
        None
    }

    /// Moves typing to another of the quicklink's boxes, keeping what was in this one.
    fn focus_link_arg(&mut self, to: usize) {
        let typed = self.read_query();
        let Some(args) = self.link_args.as_mut() else { return };
        if to >= args.values.len() {
            return;
        }
        args.values[args.at] = typed;
        args.at = to;
        let text = args.values[to].clone();
        self.view.last_query = Some(text.clone());
        self.set_query_text(&text);
        self.refresh_results();
        self.restart_caret();
        self.redraw();
    }

    /// Shows a quicklink's blanks as boxes in the search bar, with what's known already filled in.
    fn enter_link_args(&mut self, name: String, url: String, mut values: Vec<String>) {
        let params = link_params(&url);
        if params.is_empty() {
            return;
        }
        values.resize(params.len(), String::new());
        let at = missing_param(&url, &values).unwrap_or(0);
        let text = values[at].clone();
        self.leave_menu();
        self.push_view();
        self.view.scope = Scope::LinkArgs { of: name, fields: params.len() };
        let open_with = self.open_with_for(&url);
        self.link_args = Some(LinkArgs { url, params, values, at, open_with });
        self.view.static_rows.clear();
        self.view.grid = None;
        self.view.reset_position();
        self.status = None;
        self.sync_vim_to_scope();
        self.set_placeholder();
        self.view.last_query = Some(text.clone());
        self.set_query_text(&text);
        self.refresh_results();
        self.restart_caret();
        self.redraw();
    }

    /// The one row under the boxes: the address as it will open, or which box still needs filling.
    fn build_link_args_rows(&mut self, typed: &str) {
        self.view.rows.clear();
        let Scope::LinkArgs { of, .. } = &self.view.scope else { return };
        let name = of.clone();
        let Some(args) = self.link_args.as_mut() else { return };
        if let Some(value) = args.values.get_mut(args.at) {
            *value = typed.to_string();
        }
        let mut row = match missing_param(&args.url, &args.values) {
            Some(at) => Row::op(format!("Open {name}"), format!("Fill in {}", args.params[at].label), GLYPH_LINK, Op::LinkArgsNext),
            None => {
                let url = fill_link(&args.url, &args.values);
                Row::op(format!("Open {name}"), url.clone(), GLYPH_WEB, Op::OpenLink { target: url, open_with: args.open_with.clone() })
            }
        };
        row.kind = "Quicklink";
        self.view.rows.push(row);
    }

    // --- Forms inside the launcher -----------------------------------------------------------

    /// Shows a form for making or changing a quicklink or snippet, in the launcher. If the launcher
    /// was hidden, it appears for the form and hides again when the form is done.
    pub fn open_form(&mut self, mut form: InlineForm) {
        self.leave_menu();
        self.parked_form = None;
        if !self.visible {
            form.hide_after = true;
            self.show(Scope::Root, "form", qpc());
        }
        self.enter_form(form);
        self.focus_input();
        self.restart_caret();
        self.redraw();
    }

    fn enter_form(&mut self, form: InlineForm) {
        self.push_view();
        self.view.scope = Scope::Form { title: form.title.clone(), link: form.is_link(), multiline: false, choice: false };
        self.view.rows.clear();
        self.view.static_rows.clear();
        self.view.grid = None;
        self.view.reset_position();
        self.status = None;
        self.link_args = None;
        let at = form.at;
        self.form = Some(form);
        self.sync_vim_to_scope();
        self.set_placeholder();
        self.load_form_field(at);
    }

    /// Whether the field being filled in is picked from a list.
    fn choice_focused(&self) -> bool {
        matches!(self.view.scope, Scope::Form { choice: true, .. }) && self.menu.is_none()
    }

    fn set_edit_read_only(&self, read_only: bool) {
        unsafe {
            SendMessageW(self.edit, windows::Win32::UI::Controls::EM_SETREADONLY, Some(WPARAM(read_only as usize)), None);
        }
    }

    /// The apps a quicklink can open with, as a small menu to search, with the usual app first.
    fn open_app_picker(&mut self) {
        let current = self.form.as_ref().and_then(|form| form.fields.get(crate::inline_form::LINK_OPEN_WITH)).map(|field| field.value.clone()).unwrap_or_default();
        let mut apps: Vec<&indexer::AppEntry> = self.catalog.apps.iter().collect();
        apps.sort_by_key(|app| app.name.to_lowercase());
        let mut rows = vec![Row::op("The usual app", "Whatever Windows uses for it", GLYPH_LINK, Op::FormOpenWith(String::new()))];
        rows.extend(apps.into_iter().map(|app| {
            let mut row = Row::op(app.name.clone(), "", GLYPH_LINK, Op::FormOpenWith(app.id.clone()));
            row.icon = RowIcon::App(icon_hash(&app.id));
            row
        }));
        let at = rows.iter().position(|row| row.primary == Some(Op::FormOpenWith(current.clone()))).unwrap_or(0);
        self.set_edit_read_only(false);
        self.open_menu(MenuSide::Right, ui::APP_PICKER_TITLE.to_string(), rows);
        self.view.select(at);
        self.after_selection_change();
        self.restart_caret();
        self.redraw();
    }

    /// Whether typing is going into the snippet's box rather than the search box.
    fn area_focused(&self) -> bool {
        matches!(self.view.scope, Scope::Form { multiline: true, .. }) && self.menu.is_none()
    }

    /// Puts the keyboard in whichever hidden box is being typed in.
    fn focus_input(&self) {
        let target = if self.area_focused() { self.area } else { self.edit };
        unsafe {
            let _ = SetFocus(Some(target));
        }
    }

    pub fn read_area_field(&self) -> ui::Field {
        if !self.area_focused() {
            return ui::Field { text: String::new(), selection: (0, 0) };
        }
        let (mut start, mut end) = (0u32, 0u32);
        unsafe {
            SendMessageW(self.area, EM_GETSEL, Some(WPARAM(&mut start as *mut u32 as usize)), Some(LPARAM(&mut end as *mut u32 as isize)));
        }
        ui::Field { text: crate::forms::get_text(self.area), selection: (start as usize, end as usize) }
    }

    /// Keeps what's in the box being typed in as its field's value.
    fn store_form_value(&mut self) {
        if self.choice_focused() {
            return;
        }
        let multiline = self.area_focused();
        let typed = if multiline { crate::forms::get_text(self.area).replace("\r\n", "\n") } else { self.read_query() };
        let Some(form) = self.form.as_mut() else { return };
        let at = form.at;
        let Some(field) = form.fields.get_mut(at) else { return };
        if field.value != typed {
            field.value = typed;
            // Changing what was wrong takes the complaint away, until the next Save.
            if form.problem.as_ref().is_some_and(|(wrong, _)| *wrong == at) {
                form.problem = None;
            }
        }
    }

    /// Puts field `to`'s value in the box it's typed in, and the keyboard there.
    fn load_form_field(&mut self, to: usize) {
        let Some(form) = self.form.as_mut() else { return };
        let Some(field) = form.fields.get(to) else { return };
        let (value, multiline, choice) = (field.value.clone(), field.multiline, field.choice);
        form.at = to;
        if let Scope::Form { multiline: shown, choice: picked, .. } = &mut self.view.scope {
            *shown = multiline;
            *picked = choice;
        }
        // A field picked from a list takes no typing.
        self.set_edit_read_only(choice);
        if choice {
            self.view.last_query = Some(String::new());
            self.set_query_text("");
        } else if multiline {
            let lines = value.replace("\r\n", "\n").replace('\n', "\r\n");
            let (text, end) = (wide(&lines), lines.encode_utf16().count());
            unsafe {
                let _ = SetWindowTextW(self.area, PCWSTR(text.as_ptr()));
                SendMessageW(self.area, EM_SETSEL, Some(WPARAM(end)), Some(LPARAM(end as isize)));
            }
            self.view.last_query = Some(String::new());
            self.set_query_text("");
        } else {
            self.view.last_query = Some(value.clone());
            self.set_query_text(&value);
        }
        if self.visible {
            self.focus_input();
        }
    }

    fn focus_form_field(&mut self, to: usize) {
        self.store_form_value();
        self.load_form_field(to);
        self.restart_caret();
        self.redraw();
    }

    /// Tab and Shift+Tab go round the fields, and so do Up and Down where a field has one line.
    /// Enter saves, except in the snippet's own box, where it starts a new line and Ctrl+Enter saves.
    fn on_form_key(&mut self, vk: u16, ctrl: bool) -> Option<bool> {
        if !matches!(self.view.scope, Scope::Form { .. }) || self.menu.is_some() {
            return None;
        }
        let (at, count) = self.form.as_ref().map(|form| (form.at, form.fields.len()))?;
        let shift = unsafe { GetKeyState(0x10) } < 0;
        let multiline = self.area_focused();
        let alt = unsafe { GetKeyState(0x12) } < 0;
        if self.choice_focused() {
            let open = vk == 0x20 || (vk == VK_F4.0 && !alt) || (vk == VK_DOWN.0 && alt);
            if open {
                self.open_app_picker();
                return Some(true);
            }
            if vk == VK_BACK.0 || vk == VK_DELETE.0 {
                if let Some(form) = self.form.as_mut() {
                    form.set_open_with("", "");
                }
                self.redraw();
                return Some(true);
            }
        }
        let handled = match vk {
            v if v == VK_TAB.0 => {
                let to = if shift { (at + count - 1) % count } else { (at + 1) % count };
                self.focus_form_field(to);
                true
            }
            v if v == VK_RETURN.0 && (ctrl || !multiline) => {
                self.save_form();
                true
            }
            v if v == VK_ESCAPE.0 => {
                self.close_form();
                true
            }
            v if (v == VK_UP.0 || v == VK_DOWN.0) && !multiline => {
                let to = if v == VK_UP.0 { at.checked_sub(1) } else { (at + 1 < count).then_some(at + 1) };
                if let Some(to) = to {
                    self.focus_form_field(to);
                }
                true
            }
            0x41 if ctrl => {
                let target = if multiline { self.area } else { self.edit };
                unsafe {
                    SendMessageW(target, EM_SETSEL, Some(WPARAM(0)), Some(LPARAM(-1)));
                }
                true
            }
            v if v == VK_BACK.0 && ctrl && !multiline => {
                self.delete_word_before_caret();
                true
            }
            // There's no menu of actions here. Alt+F4 still closes.
            0x4B if ctrl => true,
            v if v == VK_F4.0 => return None,
            _ => false,
        };
        Some(handled)
    }

    fn save_form(&mut self) {
        self.store_form_value();
        let Some(form) = self.form.as_ref() else { return };
        if form.saving {
            return;
        }
        if form.is_link() {
            let editing = matches!(&form.kind, FormKind::Link { id } if !id.is_empty());
            match form.check_link(&self.settings.links, &self.settings.custom_commands) {
                Ok(link) => {
                    let message = if editing { format!("Saved “{}”", link.name) } else { format!("Created “{}”", link.name) };
                    self.on_link_saved(crate::link_form::LinkSaved::Link(link));
                    self.finish_form(message);
                }
                Err(problem) => self.show_form_problem(problem),
            }
            return;
        }
        match form.check_snippet() {
            Ok((id, input)) => {
                if let Some(form) = self.form.as_mut() {
                    form.saving = true;
                }
                self.worker.send(Req::SnippetSave { id, input, editor: self.send });
            }
            Err(problem) => self.show_form_problem(problem),
        }
    }

    fn show_form_problem(&mut self, (at, message): (usize, String)) {
        if let Some(form) = self.form.as_mut() {
            form.problem = Some((at, message));
            form.saving = false;
        }
        self.focus_form_field(at);
        // Ready to type over.
        let target = if self.area_focused() { self.area } else { self.edit };
        unsafe {
            SendMessageW(target, EM_SETSEL, Some(WPARAM(0)), Some(LPARAM(-1)));
        }
        self.redraw();
    }

    /// The form was saved: back to where it was opened from, saying so.
    fn finish_form(&mut self, message: String) {
        let Some(form) = self.form.take() else { return };
        if form.hide_after {
            self.hide(true);
            return;
        }
        self.go_back_or_hide();
        self.status = Some(message);
        self.redraw();
    }

    fn close_form(&mut self) {
        let Some(form) = self.form.take() else { return };
        if form.hide_after {
            self.hide(true);
        } else {
            self.go_back_or_hide();
        }
    }

    /// A click on a field types in it, and a placeholder chip goes in at the snippet's caret.
    fn on_form_click(&mut self, x: i32, y: i32) {
        let inside = |rect: &RECT| x >= rect.left && x < rect.right && y >= rect.top && y < rect.bottom;
        if let Some(index) = self.form_boxes.iter().position(inside) {
            if self.form.as_ref().is_some_and(|form| form.at != index) {
                self.focus_form_field(index);
            }
            if self.choice_focused() {
                self.open_app_picker();
                return;
            }
        } else if let Some(chip) = self.form_chips.iter().position(inside) {
            let Some(multiline_at) = self.form.as_ref().and_then(|form| form.fields.iter().position(|field| field.multiline)) else { return };
            if !self.area_focused() {
                self.focus_form_field(multiline_at);
            }
            let text = wide(crate::inline_form::PLACEHOLDERS[chip].1);
            unsafe {
                SendMessageW(self.area, EM_REPLACESEL, Some(WPARAM(1)), Some(LPARAM(text.as_ptr() as isize)));
            }
            self.store_form_value();
        }
        self.focus_input();
        self.restart_caret();
        self.redraw();
    }

    fn on_area_changed(&mut self) {
        if !self.area_focused() {
            return;
        }
        self.store_form_value();
        self.redraw();
    }

    /// Prompts need typing. Everything else starts out navigating.
    fn sync_vim_to_scope(&mut self) {
        let needs_typing = matches!(self.view.scope, Scope::SetAlias { .. } | Scope::SetHotkey { .. } | Scope::LinkArgs { .. } | Scope::Form { .. });
        let mode = if needs_typing || self.settings.launcher.keyboard != KeyboardMode::Vim { VimMode::Insert } else { VimMode::Normal };
        self.set_vim_mode(mode);
    }

    pub fn set_vim_mode(&mut self, mode: VimMode) {
        let was_normal = VIM_NORMAL.load(Ordering::Relaxed);
        let is_vim = self.settings.launcher.keyboard == KeyboardMode::Vim;
        if !is_vim && !was_normal {
            return; // Nothing has touched the caret, and nothing needs to.
        }
        self.vim_keys.reset();
        self.vim_mode = mode;
        let normal = is_vim && mode == VimMode::Normal;
        VIM_NORMAL.store(normal, Ordering::Relaxed);
        if normal {
            let units: Vec<u16> = self.read_query().encode_utf16().collect();
            self.set_caret(vim::normal_caret(&units, self.caret().min(units.len())));
        }
        self.caret_on = true;
        self.set_placeholder();
        self.redraw();
    }

    fn caret(&self) -> usize {
        let mut end: u32 = 0;
        unsafe {
            SendMessageW(self.edit, EM_GETSEL, None, Some(LPARAM(&mut end as *mut u32 as isize)));
        }
        end as usize
    }

    fn set_caret(&self, at: usize) {
        unsafe {
            SendMessageW(self.edit, EM_SETSEL, Some(WPARAM(at)), Some(LPARAM(at as isize)));
        }
    }

    fn delete_range(&mut self, start: usize, end: usize) {
        unsafe {
            SendMessageW(self.edit, EM_SETSEL, Some(WPARAM(start)), Some(LPARAM(end as isize)));
            let empty = wide("");
            SendMessageW(self.edit, EM_REPLACESEL, Some(WPARAM(1)), Some(LPARAM(empty.as_ptr() as isize)));
        }
    }

    /// Keys that aren't typed characters. `None` leaves the key to the usual handling.
    fn on_vim_key(&mut self, vk: u16, ctrl: bool, t_key: i64) -> Option<bool> {
        let held = |key: u16| unsafe { GetKeyState(key as i32) } < 0;
        if held(VK_MENU.0) && !ctrl {
            let letter = u8::try_from(vk).ok().filter(u8::is_ascii_uppercase)? as char;
            let command = vim::alt_command(letter, held(0x10))?;
            ALT_CHORD_CHAR.store(true, Ordering::Relaxed);
            ALT_CHORD_RELEASE.store(true, Ordering::Relaxed);
            self.run_vim_command(command, t_key);
            return Some(true);
        }
        if self.settings.launcher.keyboard != KeyboardMode::Vim {
            return None;
        }
        let is_escape = vk == VK_ESCAPE.0 || (ctrl && vk == 0xDB); // Ctrl+[ is Esc, as in Vim
        match self.vim_mode {
            VimMode::Insert => {
                if !is_escape {
                    return None;
                }
                if matches!(self.view.scope, Scope::SetAlias { .. } | Scope::LinkArgs { .. }) {
                    self.go_back_or_hide();
                } else {
                    self.set_vim_mode(VimMode::Normal);
                }
                Some(true)
            }
            VimMode::Normal => {
                if is_escape {
                    self.escape();
                    return Some(true);
                }
                if ctrl {
                    let letter = u8::try_from(vk).ok().filter(u8::is_ascii_uppercase)? as char;
                    let command = vim::ctrl_command(letter)?;
                    self.run_vim_command(command, t_key);
                    return Some(true);
                }
                if vk == VK_DELETE.0 {
                    self.run_vim_command(VimCommand::Delete { what: VimDelete::Chars(1), then_insert: false }, t_key);
                    return Some(true);
                }
                if vk == VK_BACK.0 {
                    let empty = self.read_query().is_empty();
                    if empty && !self.view.stack.is_empty() {
                        self.go_back_or_hide();
                    } else {
                        let units: Vec<u16> = self.read_query().encode_utf16().collect();
                        self.set_caret(vim::char_before(&units, self.caret()));
                    }
                    return Some(true);
                }
                None
            }
        }
    }

    pub fn on_vim_char(&mut self, c: char) {
        if let Some(command) = self.vim_keys.on_char(c) {
            self.run_vim_command(command, qpc());
        }
    }

    fn run_vim_command(&mut self, command: VimCommand, t_key: i64) {
        let units: Vec<u16> = self.read_query().encode_utf16().collect();
        let caret = self.caret().min(units.len());
        let last = self.view.rows.len().saturating_sub(1);
        let half = (self.view.visible_rows as i32 / 2).max(2);
        let page = (self.view.visible_rows as i32).max(2);
        match command {
            VimCommand::Move(by) => self.move_rows(by),
            VimCommand::HalfPage(direction) => self.move_rows(direction * half),
            VimCommand::Page(direction) => self.move_rows(direction * page),
            VimCommand::First => self.select_row(0),
            VimCommand::Last => self.select_row(last),
            VimCommand::GoTo(row) => self.select_row((row as usize).min(last)),
            VimCommand::Back => {
                // Going back never closes Orbit by accident. Esc and q do that.
                if !self.view.stack.is_empty() {
                    self.go_back_or_hide();
                }
            }
            VimCommand::Open => self.activate_selected(false, t_key),
            VimCommand::Close => self.hide(true),
            VimCommand::Insert(at) => {
                self.set_vim_mode(VimMode::Insert);
                self.set_caret(match at {
                    InsertAt::Caret => caret,
                    InsertAt::AfterCaret => vim::char_after(&units, caret),
                    InsertAt::Start => 0,
                    InsertAt::End => units.len(),
                });
            }
            VimCommand::InsertText(c) => {
                self.set_vim_mode(VimMode::Insert);
                self.set_query_text(&c.to_string());
            }
            VimCommand::Delete { what, then_insert } => {
                let (start, end) = match what {
                    VimDelete::All => (0, units.len()),
                    VimDelete::ToEnd => (caret, units.len()),
                    VimDelete::Word => (caret, vim::word_forward(&units, caret)),
                    VimDelete::Chars(times) => {
                        let mut end = caret;
                        for _ in 0..times {
                            end = vim::char_after(&units, end);
                        }
                        (caret, end)
                    }
                };
                if start < end {
                    self.delete_range(start, end);
                }
                let mode = if then_insert { VimMode::Insert } else { self.vim_mode };
                self.set_vim_mode(mode);
            }
            VimCommand::CaretStart => self.set_caret(0),
            VimCommand::CaretEnd => self.set_caret(vim::normal_caret(&units, units.len())),
            VimCommand::WordForward => self.set_caret(vim::normal_caret(&units, vim::word_forward(&units, caret))),
            VimCommand::WordBack => self.set_caret(vim::word_back(&units, caret)),
            VimCommand::Undo => unsafe {
                SendMessageW(self.edit, EM_UNDO, None, None);
            },
            VimCommand::PasteBefore | VimCommand::PasteAfter => {
                if command == VimCommand::PasteAfter {
                    self.set_caret(vim::char_after(&units, caret));
                }
                unsafe {
                    SendMessageW(self.edit, WM_PASTE, None, None);
                }
            }
        }
    }

    fn move_rows(&mut self, by: i32) {
        self.view.move_selection(by);
        self.after_selection_change();
        self.redraw();
    }

    fn select_row(&mut self, row: usize) {
        self.view.select(row);
        self.after_selection_change();
        self.redraw();
    }

    // --- Footer buttons --------------------------------------------------------------------

    fn footer_button_at(&self, x: i32, y: i32) -> Option<&'static str> {
        self.footer_buttons
            .iter()
            .find(|(area, _)| x >= area.left && x < area.right && y >= area.top && y < area.bottom)
            .map(|(_, name)| *name)
    }

    /// A click does what the key shown on the button does, as if it had been pressed: Enter saves
    /// a form, Space picks its app, and so on.
    fn on_footer_button(&mut self, name: &'static str) {
        if let Some((vk, ctrl)) = footer_key(name) {
            self.on_key_as(vk, ctrl, qpc());
        }
        match name {
            "i" => self.set_vim_mode(VimMode::Insert),
            ui::BUTTON_ORBIT_MENU => self.open_orbit_menu(),
            ui::BUTTON_BACK => self.go_back_or_hide(),
            ui::BUTTON_FILTER => self.toggle_filter_menu(),
            ui::BUTTON_VIM_MODE => {
                let next = if self.vim_mode == VimMode::Normal { VimMode::Insert } else { VimMode::Normal };
                self.set_vim_mode(next);
            }
            _ => {}
        }
        // Typing carries on in whichever box it was in, such as a snippet's.
        if self.visible {
            self.focus_input();
        }
    }

    // --- Setting a hotkey from the action menu ---------------------------------------------

    fn begin_recording(&mut self, action: Action, title: String) {
        self.recording = Some(Recording { action, title: title.clone(), pending: None });
        RECORDING.store(true, Ordering::Relaxed);
        // Registered shortcuts never reach the query box, so they stand aside while it listens.
        unsafe {
            for id in &self.registered_hotkeys {
                let _ = UnregisterHotKey(Some(self.main), *id);
            }
        }
        if let Some(hook) = &self.hook {
            hook.set_recording(true);
        }
        self.view.scope = Scope::SetHotkey { of: title };
        self.view.reset_position();
        self.sync_vim_to_scope();
        self.set_placeholder();
        self.view.last_query = Some(String::new());
        self.set_query_text("");
        self.view.static_rows = self.recording_rows();
        self.refresh_results();
    }

    fn end_recording(&mut self) {
        if self.recording.take().is_some() {
            RECORDING.store(false, Ordering::Relaxed);
            if let Some(hook) = &self.hook {
                hook.set_recording(false);
            }
            self.register_hotkeys();
        }
    }

    fn hyper_key_name(&self) -> String {
        match self.settings.hyper.trigger.as_str() {
            "CapsLock" => "Caps Lock".to_string(),
            other => other.to_string(),
        }
    }

    fn recording_rows(&self) -> Vec<Row> {
        let Some(recording) = &self.recording else { return Vec::new() };
        if let Some(hotkey) = &recording.pending {
            let label = hotkey.label("Win");
            let current = hotkeys::action_of(&self.settings, hotkey).map(|action| hotkeys::describe(&self.settings, action)).unwrap_or_default();
            return vec![
                Row::op(format!("Replace {current} with {}", recording.title), "", GLYPH_WARNING, Op::ConfirmHotkey).with_keys(label),
                Row::op("Choose a different key", "", GLYPH_BACK, Op::RecordAgain),
            ];
        }
        let hyper_on = self.settings.hyper.enabled && self.hook.is_some();
        let mut rows = if hyper_on {
            vec![Row::info("Press the keys you want", format!("Any shortcut, such as Ctrl+Alt+K, or {} with a key", self.hyper_key_name()), GLYPH_KEYBOARD)]
        } else {
            vec![Row::info("Press the keys you want", "Any shortcut, such as Ctrl+Alt+K or Ctrl+Shift+Space", GLYPH_KEYBOARD)]
        };
        if let Some(current) = hotkeys::hotkey_for(&self.settings, &recording.action) {
            rows.push(Row::op("Remove hotkey", "", GLYPH_DELETE, Op::ClearHotkey(recording.action.clone())).with_keys(current.label("Win")));
        }
        rows
    }

    fn show_recording_rows(&mut self) {
        self.view.static_rows = self.recording_rows();
        self.view.reset_position();
        self.status = None;
        self.refresh_results();
    }

    fn on_recording_key(&mut self, vk: u16, t_key: i64) -> bool {
        let held = |key: u16| unsafe { GetKeyState(key as i32) } < 0;
        let mut mods = Mods::default();
        if held(VK_CONTROL.0) {
            mods = mods.union(Mods::CTRL);
        }
        if held(0x12) {
            mods = mods.union(Mods::ALT);
        }
        if held(0x10) {
            mods = mods.union(Mods::SHIFT);
        }
        if held(0x5B) || held(0x5C) {
            mods = mods.union(Mods::SUPER);
        }
        if mods.is_empty() {
            match vk {
                v if v == VK_ESCAPE.0 => {
                    if self.recording.as_ref().is_some_and(|r| r.pending.is_some()) {
                        self.execute(Op::RecordAgain);
                    } else {
                        self.go_back_or_hide();
                    }
                    return true;
                }
                v if v == VK_RETURN.0 => {
                    self.activate_selected(false, t_key);
                    return true;
                }
                v if v == VK_UP.0 || v == VK_DOWN.0 => {
                    self.view.move_selection(if v == VK_UP.0 { -1 } else { 1 });
                    self.after_selection_change();
                    self.redraw();
                    return true;
                }
                _ => {}
            }
        }
        // A modifier on its own, or Caps Lock itself, is still on the way to being a chord.
        if matches!(vk, 0x10..=0x12 | 0x14 | 0x5B | 0x5C | 0xA0..=0xA5) {
            return true;
        }
        if self.recording.as_ref().is_some_and(|r| r.pending.is_some()) {
            return true; // answer the question first
        }
        let Some(name) = u8::try_from(vk).ok().and_then(keys::name_from_vk) else { return true };
        let function_key = (0x70..=0x87).contains(&vk);
        if !function_key && mods.union(Mods::SHIFT) == Mods::SHIFT {
            let hint = if self.settings.hyper.enabled && self.hook.is_some() {
                format!("A hotkey needs Ctrl, Alt or Win held, or {}.", self.hyper_key_name())
            } else {
                "A hotkey needs Ctrl, Alt or Win held.".to_string()
            };
            self.status = Some(hint);
            self.redraw();
            return true;
        }
        let Ok(chord) = Chord::new(mods, &name) else { return true };
        // Another remapper sends the whole chord, and nothing here sees the key behind it.
        let hotkey = if mods == Mods::HYPER && self.settings.hyper.mode == HyperMode::External && self.settings.hyper.enabled {
            Hotkey::Hyper(name)
        } else {
            Hotkey::Direct(chord)
        };
        self.capture_hotkey(hotkey);
        true
    }

    /// A key was pressed. Either it's unusable, or it needs a question, or it's done.
    fn capture_hotkey(&mut self, hotkey: Hotkey) {
        let Some(recording) = &self.recording else { return };
        if recording.pending.is_some() {
            return;
        }
        let action = recording.action.clone();
        if let Hotkey::Direct(chord) = &hotkey {
            let label = chord.display("Win");
            let problem = if is_reserved_on_windows(chord) {
                Some(format!("{label} is reserved by Windows. Choose another."))
            } else if label == self.active_hotkey {
                Some(format!("{label} opens Orbit. Choose another."))
            } else if !self.shortcut_is_free(chord) {
                Some(format!("{label} is taken by another program. Choose another."))
            } else {
                None
            };
            if let Some(problem) = problem {
                self.status = Some(problem);
                self.redraw();
                return;
            }
        }
        let conflict = hotkeys::action_of(&self.settings, &hotkey).is_some_and(|other| !hotkeys::same_target(other, &action));
        if conflict {
            if let Some(recording) = self.recording.as_mut() {
                recording.pending = Some(hotkey);
            }
            self.show_recording_rows();
            return;
        }
        self.finish_hotkey(hotkey);
    }

    fn shortcut_is_free(&self, chord: &Chord) -> bool {
        const PROBE_ID: i32 = 0xBFFD;
        let Some((mods, vk)) = keys::chord_to_registration(chord) else { return false };
        unsafe {
            let free = RegisterHotKey(Some(self.main), PROBE_ID, mods, vk).is_ok();
            if free {
                let _ = UnregisterHotKey(Some(self.main), PROBE_ID);
            }
            free
        }
    }

    fn finish_hotkey(&mut self, hotkey: Hotkey) {
        let Some(recording) = self.recording.take() else { return };
        RECORDING.store(false, Ordering::Relaxed);
        if let Some(hook) = &self.hook {
            hook.set_recording(false);
        }
        let label = hotkey.label("Win");
        let mut settings = self.settings.clone();
        hotkeys::assign(&mut settings, hotkey, recording.action);
        self.apply_settings(settings);
        crate::settings_win::hotkeys_changed(&self.settings);
        self.metrics.record_action("hotkey-set", 0, 0);
        self.go_back_or_hide();
        self.status = Some(format!("{label} now opens {}", recording.title));
        self.redraw();
    }

    fn after_selection_change(&mut self) {
        let secondary = self.view.selected_row().and_then(|row| secondary_for(row, self.app_id_of(row).as_deref()));
        self.view.secondary_label = secondary.map(|(label, _)| label);
        self.request_visible_icons();
        // A menu over the clipboard leaves its preview as it was.
        if self.menu.is_none() {
            self.update_clip_preview();
        }
    }

    /// Asks for the selected clip's preview, once per selection. Anywhere else, it's let go.
    fn update_clip_preview(&mut self) {
        let wanted = match (&self.view.scope, self.view.selected_row()) {
            (Scope::Clipboard, Some(Row { source: RowSource::Clip { id, .. }, .. })) => *id,
            _ => {
                self.drop_clip_preview();
                return;
            }
        };
        if self.clip_preview_wanted == Some(wanted) {
            return;
        }
        // The last one stays up until this arrives, so moving down the list doesn't flicker.
        self.clip_preview_wanted = Some(wanted);
        let (max_width, max_height) = ui::preview_room(self.dpi, self.full_rows() as i32);
        self.worker.clip_preview(wanted, max_width, max_height);
    }

    fn drop_clip_preview(&mut self) {
        self.clip_preview = None;
        self.clip_preview_wanted = None;
    }

    /// A program's name as people know it: the app's own, if it's in the index.
    fn source_name(&self, program: &str) -> String {
        if program.is_empty() {
            return String::new();
        }
        let lower = program.to_lowercase();
        // Shortcuts to folders run Explorer too, so its own name comes first.
        let known = match lower.as_str() {
            "explorer.exe" => "File Explorer",
            "msedge.exe" => "Microsoft Edge",
            "snippingtool.exe" | "screenclippinghost.exe" => "Snipping Tool",
            _ => "",
        };
        if !known.is_empty() {
            return known.into();
        }
        match self.catalog.apps.iter().find(|app| !app.exe.is_empty() && util::exe_name(&app.exe) == lower) {
            Some(app) => app.name.clone(),
            None => crate::app_picker::name_from_program(&lower),
        }
    }

    /// Stand-in clipboard items, for drawing the clipboard in a private data folder.
    fn add_sample_clips(&self) {
        use orbit_data::NewClip;
        let (width, height) = (480usize, 300usize);
        let mut dib = vec![0u8; 40];
        dib[0] = 40;
        dib[4..8].copy_from_slice(&(width as i32).to_le_bytes());
        dib[8..12].copy_from_slice(&(height as i32).to_le_bytes());
        dib[12] = 1;
        dib[14] = 32;
        for y in 0..height {
            for x in 0..width {
                // A dusk sky over hills, bottom row first.
                let (fx, fy) = (x as f32 / width as f32, 1.0 - y as f32 / height as f32);
                let hill = 0.72 + 0.08 * (fx * 7.0).sin() + 0.05 * (fx * 13.0 + 1.0).cos();
                let sun = ((fx - 0.68).powi(2) + (fy - 0.42).powi(2)).sqrt() < 0.09;
                let (r, g, b) = if fy > hill {
                    (30.0 + 20.0 * fx, 34.0 + 30.0 * fx, 58.0)
                } else if sun {
                    (255.0, 214.0, 150.0)
                } else {
                    (70.0 + 170.0 * fy, 80.0 + 60.0 * fy, 170.0 - 40.0 * fy)
                };
                dib.extend_from_slice(&[b as u8, g as u8, r as u8, 0]);
            }
        }
        let size = dib.len() as u64;
        let samples = [
            NewClip::files(&["C:\\Users\\Public\\Documents\\Quarterly report.docx".into(), "C:\\Users\\Public\\Documents\\Budget 2027.xlsx".into()], "explorer.exe"),
            NewClip::text("fn main() {\n    let launcher = Orbit::new();\n    launcher.show();\n}".into(), "Code.exe"),
            NewClip::text("https://github.com/rust-lang/rust/issues/12345".into(), "msedge.exe"),
            NewClip::text("Meeting moved to Thursday at 3pm. Bring the draft of the launch plan and the numbers from last week, and we'll go through the open questions together. The room is booked for an hour.".into(), "msedge.exe")
                .with_formats(orbit_data::Formats { html: Some(b"Version:0.9\r\n<b>Meeting moved</b>".to_vec()), rtf: None }),
        ];
        for clip in samples {
            self.worker.send(Req::ClipAdd { clip, image: None });
        }
        let image = NewClip::image(String::new(), width as u32, height as u32, size, 0x5A11_D05E, "SnippingTool.exe");
        self.worker.send(Req::ClipAdd { clip: image, image: Some(dib) });
    }

    fn delete_word_before_caret(&mut self) {
        let text = self.read_query();
        let mut end: u32 = 0;
        unsafe {
            SendMessageW(self.edit, EM_GETSEL, None, Some(LPARAM(&mut end as *mut u32 as isize)));
        }
        let units: Vec<u16> = text.encode_utf16().collect();
        let end = (end as usize).min(units.len());
        let start = crate::forms::word_start(&units, end);
        unsafe {
            SendMessageW(self.edit, EM_SETSEL, Some(WPARAM(start)), Some(LPARAM(end as isize)));
            let empty = wide("");
            SendMessageW(self.edit, EM_REPLACESEL, Some(WPARAM(1)), Some(LPARAM(empty.as_ptr() as isize)));
        }
    }

    fn on_mouse_move(&mut self, x: i32, y: i32) {
        let hovered = self.footer_button_at(x, y);
        if hovered != self.hovered_button {
            self.hovered_button = hovered;
            if hovered.is_some() {
                let mut track = TRACKMOUSEEVENT {
                    cbSize: std::mem::size_of::<TRACKMOUSEEVENT>() as u32,
                    dwFlags: TME_LEAVE,
                    hwndTrack: self.main,
                    dwHoverTime: 0,
                };
                unsafe {
                    let _ = TrackMouseEvent(&mut track);
                }
            }
            self.redraw();
        }
        // A window appearing under a still pointer must not steal the keyboard selection.
        if self.last_mouse == (x, y) || self.last_mouse == (i32::MIN, i32::MIN) {
            self.last_mouse = (x, y);
            return;
        }
        self.last_mouse = (x, y);
        if let Some(index) = self.row_under(x, y) {
            if index != self.view.selected {
                // What's under the pointer is on screen already, and moving over it mustn't scroll
                // the list out from under the pointer.
                let scroll = self.view.scroll;
                self.view.select(index);
                self.view.scroll = scroll;
                self.after_selection_change();
                self.redraw();
            }
        }
    }

    /// The launcher's size as last drawn.
    fn panel_size(&self) -> (i32, i32) {
        let margin = ui::frame_margin(self.dpi);
        let width = ui::scale(ui::BASE_WIDTH, self.dpi);
        (width, self.window_height - margin * 2)
    }

    /// Which row a point in the launcher is on, in whichever way the rows are laid out right now.
    fn row_under(&self, x: i32, y: i32) -> Option<usize> {
        let (width, height) = self.panel_size();
        if x < 0 || y < 0 || x >= width || y >= height {
            return None;
        }
        if let Some(menu) = &self.menu {
            let popover = ui::popover_layout(width, height, self.dpi, menu.side, menu.shown);
            return popover.row_at(x, y, self.view.rows.len(), self.view.scroll, menu.shown);
        }
        match &self.view.grid {
            Some(grid) => ui::grid_row_at(grid, self.view.scroll, x, y, width, self.dpi),
            // The clipboard's preview is beside its list, not part of a row.
            None if ui::is_split(&self.view.scope, &self.view.rows) && x >= ui::split_x(self.dpi) => None,
            None => ui::row_at(y, self.dpi, &self.view.rows, self.view.scroll, self.view.visible_rows),
        }
    }

    fn on_click(&mut self, x: i32, y: i32, right: bool) {
        let (width, height) = self.panel_size();
        // The shadow looks like outside, so clicking it is clicking away.
        if x < 0 || y < 0 || x >= width || y >= height {
            self.hide(true);
            return;
        }
        if let Some(menu) = &self.menu {
            let popover = ui::popover_layout(width, height, self.dpi, menu.side, menu.shown);
            let inside = popover.contains(x, y);
            if !inside {
                self.close_menu();
                return;
            }
        }
        if matches!(self.view.scope, Scope::Form { .. }) && self.menu.is_none() {
            self.on_form_click(x, y);
            return;
        }
        // A click on one of a quicklink's boxes types in that one.
        if matches!(self.view.scope, Scope::LinkArgs { .. }) && self.menu.is_none() {
            if let Some(index) = self.link_arg_boxes.iter().position(|b| x >= b.left && x < b.right && y >= b.top && y < b.bottom) {
                self.focus_link_arg(index);
                unsafe {
                    let _ = SetFocus(Some(self.edit));
                }
                return;
            }
        }
        if let Some(index) = self.row_under(x, y) {
            let scroll = self.view.scroll;
            self.view.select(index);
            if self.view.grid.is_some() {
                self.view.scroll = scroll;
            }
            self.after_selection_change();
            if right {
                self.open_action_menu();
            } else {
                self.activate_selected(false, qpc());
            }
        } else {
            unsafe {
                let _ = SetFocus(Some(self.edit));
            }
        }
    }

    // --- Queries --------------------------------------------------------------------------

    pub fn read_query(&self) -> String {
        unsafe {
            let len = GetWindowTextLengthW(self.edit);
            if len <= 0 {
                return String::new();
            }
            let mut buffer = vec![0u16; len as usize + 1];
            let copied = GetWindowTextW(self.edit, &mut buffer);
            String::from_utf16_lossy(&buffer[..copied as usize])
        }
    }

    fn set_query_text(&mut self, text: &str) {
        let wide_text = wide(text);
        unsafe {
            let _ = SetWindowTextW(self.edit, PCWSTR(wide_text.as_ptr()));
            let end = text.encode_utf16().count();
            SendMessageW(self.edit, EM_SETSEL, Some(WPARAM(end)), Some(LPARAM(end as isize)));
        }
    }

    pub fn placeholder_text(&self) -> String {
        let mut text = self.view.scope.placeholder();
        if self.is_vim_normal() && self.view.scope.can_filter() {
            text.push_str(" Press i to type");
        }
        text
    }

    fn set_placeholder(&mut self) {
        let text = wide(&self.placeholder_text());
        unsafe {
            SendMessageW(self.edit, EM_SETCUEBANNER, Some(WPARAM(1)), Some(LPARAM(text.as_ptr() as isize)));
        }
    }

    /// Switches what the launcher shows. `keep_query` retains the current text.
    pub fn set_scope(&mut self, scope: Scope, keep_query: bool) {
        // Lists that arrive later start empty, not showing the last list's rows.
        if matches!(scope, Scope::Files | Scope::Extension { .. }) && scope != self.view.scope {
            self.view.rows.clear();
            // Another list's filters aren't this one's. Its own come with its first answer.
            self.ext_filters.clear();
        }
        self.view.scope = scope;
        self.view.reset_position();
        self.view.static_rows.clear();
        self.view.grid = None;
        self.sync_vim_to_scope();
        self.set_placeholder();
        if !keep_query {
            self.view.last_query = Some(String::new());
            self.set_query_text("");
        }
        self.build_static_rows();
        self.refresh_results();
        if self.menu.is_none() {
            self.update_clip_preview();
        }
    }

    fn build_static_rows(&mut self) {
        match self.view.scope.clone() {
            Scope::Help => {
                self.view.static_rows = help_rows(&self.active_hotkey, self.settings.launcher.keyboard);
            }
            Scope::Diagnostics => {
                self.view.static_rows = self.diagnostics_rows();
                self.worker.send(Req::Counts);
            }
            Scope::Vault(tool) => {
                if !self.vaults.get(&tool).is_some_and(|vault| vault.loaded) {
                    self.ask_tool_for_items(tool);
                }
                self.view.static_rows = self.vault_rows(tool);
            }
            Scope::Links => {
                let mut rows: Vec<Row> = self
                    .catalog
                    .entries
                    .iter()
                    .enumerate()
                    .filter(|(_, entry)| matches!(entry, Entry::Link(_)))
                    .map(|(item, _)| self.row_for_item(item as u32))
                    .collect();
                rows.push(Row::op("Create Quicklink", "", GLYPH_ADD, Op::RunBuiltin(Builtin::NewQuicklink)));
                self.view.static_rows = rows;
            }
            Scope::Confirm(command) => {
                let title = Builtin::ALL.iter().find(|e| e.0 == command).map_or("this", |e| e.2);
                self.view.static_rows = vec![
                    Row::op("Cancel", "Enter", GLYPH_BACK, Op::Back),
                    Row::op(format!("Yes, {title}"), "Press ↓ then Enter", GLYPH_WARNING, Op::Confirmed(command)),
                ];
            }
            _ => {}
        }
    }

    fn on_query_changed(&mut self) {
        let query = self.read_query();
        if self.view.last_query.as_deref() == Some(query.as_str()) {
            return; // our own SetWindowText, already handled
        }
        self.view.reset_position();
        self.refresh_results();
    }

    fn refresh_results(&mut self) {
        let t4 = qpc();
        let query = self.read_query();
        self.view.last_query = Some(query.clone());
        let scope = self.view.scope.clone();
        if !matches!(scope, Scope::LinkArgs { .. } | Scope::Actions { .. }) {
            self.link_args = None;
        }
        // Only the emoji list is laid out as a grid. Going back from it to the list it was opened
        // from used to keep its grid, which then pointed past the end of the shorter list.
        if scope != Scope::Emoji {
            self.view.grid = None;
        }
        if !matches!(scope, Scope::Form { .. }) {
            self.set_edit_read_only(false);
            // A menu over a form, such as the app picker, leaves the form where it is.
            if !matches!(scope, Scope::Actions { .. }) {
                self.form = None;
            }
        }
        match &scope {
            Scope::Root => self.build_root_rows(&query),
            Scope::LinkArgs { .. } => self.build_link_args_rows(&query),
            Scope::Form { .. } => {
                self.view.rows.clear();
                self.store_form_value();
            }
            Scope::Weather => {
                // Nothing is listed: the weather is drawn instead. This row is what Enter does.
                let name = query.trim().to_string();
                let title = if name.is_empty() { "Refresh".to_string() } else { format!("Look up \u{201C}{name}\u{201D}") };
                self.view.rows = vec![Row::op(title, "", GLYPH_WEATHER, Op::WeatherLookup(name))];
            }
            Scope::SpeedTest => {
                // Nothing is listed: the test is drawn instead. This row is what Enter does.
                let speed = &self.speedtest;
                let test = &speed.test;
                let mut row = if !self.settings.speedtest_accepted {
                    Row::op("Speed Test", "", GLYPH_SPEED, Op::SpeedTestAccept)
                } else if speed.installing {
                    Row::info("Installing Speedtest CLI\u{2026}", "", GLYPH_SPEED)
                } else if speed.missing {
                    Row::op("Speed Test", "", GLYPH_SPEED, Op::SpeedTestInstall)
                } else if test.running() {
                    Row::op("Speed Test", "", GLYPH_SPEED, Op::SpeedTestStop)
                } else {
                    Row::op("Speed Test", "", GLYPH_SPEED, Op::SpeedTestStart)
                };
                let finished = test.phase == orbit_core::speedtest::Phase::Done;
                row.source = RowSource::SpeedTest { summary: finished.then(|| test.summary()), url: test.url.clone() };
                self.view.rows = vec![row];
            }
            Scope::Timer => self.view.rows = vec![self.timer_row(&query)],
            Scope::Stopwatch => self.view.rows = vec![self.stopwatch_row()],
            Scope::Colour(colour) => {
                let mut row = Row::op(format!("Copy {}", colour.hex()), "", GLYPH_COPY, Op::Copy(colour.hex()));
                row.source = RowSource::Colour(*colour);
                self.view.rows = vec![row];
            }
            Scope::Emoji => self.build_emoji_rows(&query),
            Scope::Notes | Scope::Snippets | Scope::Clipboard => self.request_scope_rows(&scope, &query),
            Scope::Files => self.search_files(crate::files::Place::List, &query),
            Scope::Extension { ext, command, .. } => self.search_extension(ext, command, &query),
            Scope::SetAlias { key, .. } => {
                let alias = query.trim();
                self.view.rows.clear();
                if !alias.is_empty() && !alias.contains(char::is_whitespace) {
                    let row = if self.opens_with_keyword(key) {
                        Row::op(format!("Use keyword “{alias}”"), format!("Type “{alias}” and a space to search it"), GLYPH_TAG, Op::SaveAlias { key: key.clone() })
                    } else {
                        Row::op(format!("Use alias “{alias}”"), "", GLYPH_TAG, Op::SaveAlias { key: key.clone() })
                    };
                    self.view.rows.push(row);
                }
            }
            _ => self.view.filter_static(&query),
        }
        // A list on its way replaces these rows, so they don't get to move the selection.
        if !scope.is_data() {
            self.view.select(self.view.selected);
        }
        self.after_selection_change();
        let t5 = qpc();
        let hits = self.view.rows.len();
        self.tick_clock();
        if self.visible {
            self.redraw();
        }
        self.metrics.record_query(query.len(), hits, t4, t5, qpc());
    }

    /// The emoji list: what you've pinned and used, then everything by group. Searching shows just
    /// what matches, best first.
    fn build_emoji_rows(&mut self, query: &str) {
        use orbit_core::emoji;
        if self.emoji_table.is_empty() {
            self.emoji_table = emoji::table();
        }
        let table = std::mem::take(&mut self.emoji_table);
        let pinned = &self.settings.emoji.pinned;
        let tile = |at: u16| {
            let entry = &table[at as usize];
            Row {
                title: emoji::display_name(entry.name),
                subtitle: String::new(),
                kind: "",
                icon: RowIcon::Emoji(at),
                accent: false,
                source: RowSource::Emoji { at, pinned: pinned.iter().any(|glyph| glyph == entry.glyph) },
                primary: Some(Op::EmojiPaste(at)),
                keys: None,
                accessory: None,
            }
        };
        let place_of = |glyph: &str| table.iter().position(|entry| entry.glyph == glyph).map(|at| at as u16);
        let mut rows = Vec::with_capacity(table.len() + 16);
        if query.trim().is_empty() {
            let mut section = |title: &str, glyphs: &[String]| {
                let places: Vec<u16> = glyphs.iter().filter_map(|glyph| place_of(glyph)).collect();
                if !places.is_empty() {
                    rows.push(Row::header(title));
                    rows.extend(places.into_iter().map(tile));
                }
            };
            section("Pinned", &self.settings.emoji.pinned);
            section("Recently Used", &self.settings.emoji.recent);
            let groups = emoji::groups();
            let mut group = None;
            for (at, entry) in table.iter().enumerate() {
                if group != Some(entry.group) {
                    group = Some(entry.group);
                    rows.push(Row::header(groups.get(entry.group as usize).copied().unwrap_or("")));
                }
                rows.push(tile(at as u16));
            }
        } else {
            rows.extend(emoji::search(&table, query).into_iter().map(tile));
        }
        self.emoji_table = table;
        let room = ui::ROW_HEIGHT * self.full_rows() as i32;
        self.view.grid = Some(crate::grid::Grid::new(&rows, room));
        self.view.rows = rows;
        self.view.async_insert_at = self.view.rows.len();
    }

    fn build_root_rows(&mut self, query: &str) {
        // "f report": a keyword and a space opens the file search, or an extension's list.
        if self.menu.is_none() {
            if let Some((to, rest)) = self.keyword_jump(query) {
                self.jump(to, rest);
                return;
            }
        }
        // The files found for the last query stay until the next ones arrive, so they don't flicker.
        let files_at = self.root_files_start();
        let kept_files = match self.view.rows.get(files_at) {
            Some(row) if row.is_header() && row.title == "Files" => self.view.rows.split_off(files_at),
            _ => Vec::new(),
        };
        self.view.rows.clear();
        let q = query.trim();
        self.seq += 1;
        if q.is_empty() {
            self.push_home_rows();
            self.view.async_insert_at = self.view.rows.len();
            return;
        }
        if q == "?" {
            self.view.rows = help_rows(&self.active_hotkey, self.settings.launcher.keyboard);
            self.view.async_insert_at = self.view.rows.len();
            return;
        }

        // Explicit command mode.
        if let Some(rest) = q.strip_prefix('>').or_else(|| q.strip_prefix(':')) {
            let rest = rest.trim();
            self.hits.clear();
            if rest.is_empty() {
                for (item, entry) in self.catalog.entries.iter().enumerate() {
                    if matches!(entry, Entry::Builtin(_)) {
                        self.hits.push(Hit { item: item as u32, score: 0, class: orbit_core::MatchClass::Metadata });
                    }
                }
            } else {
                self.catalog.index.search(rest, ROW_LIMIT * 4, &mut self.scratch, &mut self.hits);
                let entries = &self.catalog.entries;
                self.hits.retain(|hit| matches!(entries[hit.item as usize], Entry::Builtin(_)));
            }
            self.hits.truncate(ROW_LIMIT);
            self.push_hits();
            self.hits.clear();
            self.view.async_insert_at = self.view.rows.len();
            return;
        }

        // "weather London". The word alone finds the Weather command, which has the place from last time.
        if let Some(place) = orbit_core::weather::parse_query(q).filter(|place| !place.is_empty() && self.settings.fetch_weather) {
            let mut row = Row::op(format!("Weather in {}", orbit_core::weather::capitalized(place)), "Looks it up with Open-Meteo", GLYPH_WEATHER, Op::Weather(place.to_string()));
            row.kind = "Weather";
            row.accent = true;
            self.view.rows.push(row);
        }

        // "timer 5m", or "timer 1h 30m".
        let lower = q.to_lowercase();
        if let Some(seconds) = lower.strip_prefix("timer ").and_then(orbit_core::timer::parse_duration) {
            let mut row = Row::op(format!("Start a timer for {}", orbit_core::timer::describe(seconds)), "", GLYPH_TIMER, Op::TimerStart(seconds));
            row.kind = "Timer";
            row.accent = true;
            self.view.rows.push(row);
        }

        // "123 euro in pounds" is money. Otherwise, anything that reads as maths is worked out.
        let conversion = if self.settings.fetch_exchange_rates { orbit_core::currency::parse(q) } else { None };
        let outcome = if conversion.is_some() { CalcOutcome::NotMath } else { evaluate(q) };
        if let Some(conversion) = conversion {
            let row = self.currency_row(&conversion);
            self.view.rows.push(row);
        }
        match outcome {
            CalcOutcome::Value { display, copy, note, reverse, units } => {
                // A unit keeps its symbol when copied with formatting, "1,234.5 m".
                let formatted = match display.strip_prefix(copy.as_str()) {
                    Some(unit) => format!("{}{unit}", orbit_core::calc::group_thousands(&copy)),
                    None => display.clone(),
                };
                use orbit_core::calc::{group_thousands, pretty_sum, rounded_for_show, unit_name, unit_symbol};
                let card = match units {
                    Some((amount, from, to)) => AnswerCard {
                        question: format!("{} {}", group_thousands(&amount), unit_symbol(from)),
                        question_label: unit_name(from).to_string(),
                        answer: format!("{} {}", group_thousands(&rounded_for_show(&copy)), unit_symbol(to)),
                        answer_label: unit_name(to).to_string(),
                        note: String::new(),
                    },
                    None => AnswerCard { question: pretty_sum(q), question_label: "Calculation".into(), answer: group_thousands(&rounded_for_show(&copy)), answer_label: "Result".into(), note: String::new() },
                };
                let mut row = Row::op(display, note.unwrap_or_else(|| q.to_string()), GLYPH_CALC, Op::Copy(String::new()));
                self.answer_copies(&mut row, copy, formatted, reverse, card);
                row.kind = "Calculator";
                row.accent = true;
                self.view.rows.push(row);
            }
            CalcOutcome::Error(message) => {
                let mut row = Row::info(message, q.to_string(), GLYPH_CALC);
                row.kind = "Calculator";
                self.view.rows.push(row);
            }
            CalcOutcome::NotMath => {}
        }

        // "g rust async": a quicklink's alias, then what goes in its blanks, a word each with the
        // last taking the rest. Anything still missing is asked for in boxes.
        if let Some((first, rest)) = q.split_once(char::is_whitespace) {
            let rest = rest.trim();
            if !rest.is_empty() {
                let template = self.settings.links.iter().find(|link| !link.alias.is_empty() && link.alias.eq_ignore_ascii_case(first) && !link_params(&link.url).is_empty());
                if let Some(link) = template {
                    let params = link_params(&link.url);
                    let values = split_args(rest, params.len());
                    let mut row = match missing_param(&link.url, &values) {
                        None => {
                            let url = fill_link(&link.url, &values);
                            let filled: Vec<&str> = values.iter().map(String::as_str).filter(|value| !value.is_empty()).collect();
                            Row::op(format!("{} for “{}”", link.name, filled.join(" · ")), "", GLYPH_WEB, Op::OpenLink { target: url, open_with: link.open_with.clone() })
                        }
                        Some(at) => Row::op(link.name.clone(), format!("Fill in {}", params[at].label), GLYPH_LINK, Op::LinkArgs { id: link.id.clone(), values }),
                    };
                    row.kind = "Web";
                    row.accent = true;
                    if let RowIcon::Favicon(key) = self.link_icon(&link.url) {
                        row.icon = RowIcon::Favicon(key);
                    }
                    self.view.rows.insert(0, row);
                }
            }
        }

        if let Some(url) = as_openable_url(q) {
            let mut row = Row::op(format!("Open {q}"), "", GLYPH_WEB, Op::OpenUrl(url.clone()));
            row.kind = "Web";
            self.view.rows.push(row);
            self.view.rows.push(Row::op("Save as Quicklink", q.to_string(), GLYPH_ADD, Op::NewLinkFor(url)));
        }

        if self.running_apps.is_none() && self.settings.launcher.focus_running_app {
            self.running_apps = Some(launch::RunningApps::gather());
        }
        self.catalog.index.search(q, ROW_LIMIT, &mut self.scratch, &mut self.hits);
        self.push_hits();
        self.view.async_insert_at = self.view.rows.len();

        // Web search stays explicit: an offered row, shown only when nothing local matched. Notes
        // and snippets answer a moment later, so the row waits for them. Otherwise it would be
        // what's selected, and it would move, or Enter would search the web for a saved note.
        let searches_data = q.chars().count() >= 2;
        if self.view.rows.is_empty() {
            if searches_data {
                self.web_row_waiting = Some(q.to_string());
            } else {
                self.push_web_row(q);
            }
        }

        if searches_data {
            self.worker.search(self.seq, DataScope::Root, q.to_string(), 6);
        }
        // A few files, under everything else, from Everything.
        let everything = &self.settings.extensions.everything;
        if everything.enabled && everything.in_search && q.chars().count() >= ext::ROOT_MIN_CHARS && crate::everything::window().is_some() {
            self.search_files(crate::files::Place::Root, q);
            self.view.rows.extend(kept_files);
        }
    }

    fn push_web_row(&mut self, q: &str) {
        if let Some(link) = self.settings.links.iter().find(|link| link.url.contains("{query}")) {
            let url = fill_url_template(&link.url, q);
            let mut row = Row::op(format!("{} for “{q}”", link.name), "", GLYPH_WEB, Op::OpenUrl(url));
            row.kind = "Web";
            // Above any files, which always come last.
            let at = self.root_files_start();
            self.view.rows.insert(at, row);
            self.metrics.record_action("web-row", 0, 0);
        }
    }

    /// What an empty search shows: favourites in the order you put them, what you use often,
    /// then a few commands. Sections with nothing in them leave no title behind.
    fn push_home_rows(&mut self) {
        let favourites: Vec<u32> = self.settings.pinned.iter().filter_map(|key| self.catalog.index.find_key(key)).collect();
        if !favourites.is_empty() {
            self.view.rows.push(Row::header("Favourites"));
            self.hits.clear();
            self.hits.extend(favourites.iter().map(|&item| Hit { item, score: 0, class: orbit_core::MatchClass::Metadata }));
            self.push_hits();
        }
        if self.settings.launcher.favorites_only {
            if favourites.is_empty() {
                self.view.rows.push(Row::info(
                    "No favourites yet",
                    "Search for something, press Ctrl+K, then Add to favourites",
                    GLYPH_STAR,
                ));
            }
            self.hits.clear();
            return;
        }
        self.catalog.index.suggestions(ROW_LIMIT + favourites.len(), &mut self.hits);
        self.hits.retain(|hit| !favourites.contains(&hit.item));
        self.hits.truncate(ROW_LIMIT);
        let suggested: Vec<u32> = self.hits.iter().map(|hit| hit.item).collect();
        if !self.hits.is_empty() {
            self.view.rows.push(Row::header("Suggestions"));
            self.push_hits();
        }
        self.hits.clear();
        for key in ["cmd:new-note", "cmd:new-link", "cmd:new-snippet", "cmd:emoji", "cmd:clipboard", "cmd:settings", "cmd:help"] {
            if let Some(item) = self.catalog.index.find_key(key) {
                if !favourites.contains(&item) && !suggested.contains(&item) {
                    self.hits.push(Hit { item, score: 0, class: orbit_core::MatchClass::Metadata });
                }
            }
        }
        if !self.hits.is_empty() {
            self.view.rows.push(Row::header("Commands"));
            self.push_hits();
        }
        self.hits.clear();
        if self.catalog.apps.is_empty() {
            let message = if self.index_running { "Finding your apps…" } else { "No apps indexed yet" };
            self.view.rows.push(Row::info(message, "This takes a few seconds the first time", GLYPH_INFO));
        }
    }

    fn push_hits(&mut self) {
        for i in 0..self.hits.len() {
            let hit = self.hits[i];
            let row = self.row_for_item(hit.item);
            if row.kind == "Switch to" {
                self.metrics.record_action("switch-row", 0, 0);
            }
            self.view.rows.push(row);
        }
    }

    fn row_for_item(&self, item: u32) -> Row {
        let index = &self.catalog.index;
        let key = index.key(item).to_string();
        let mut running = false;
        let (icon, kind) = match &self.catalog.entries[item as usize] {
            Entry::App(i) => {
                let app = &self.catalog.apps[*i];
                running = self.running_apps.as_ref().is_some_and(|apps| apps.has(app));
                (RowIcon::App(icon_hash(&app.id)), if running { "Switch to" } else { "App" })
            }
            Entry::Builtin(command) => (RowIcon::Glyph(glyph_for_builtin(*command)), "Command"),
            Entry::Link(i) => (self.catalog.link(*i).map_or(RowIcon::Glyph(GLYPH_LINK), |link| self.link_icon(&link.url)), "Link"),
            Entry::Custom(_) => (RowIcon::Glyph(GLYPH_COMMAND), "Script"),
            Entry::Integration(tool) => (RowIcon::Glyph(tool.glyph()), "Extension"),
            Entry::Extension(i) => match self.catalog.extension_commands.get(*i) {
                Some(command) => (self.extension_icon(&command.ext, command.icon.as_deref()), "Extension"),
                None => (RowIcon::Glyph(GLYPH_EXTENSION), "Extension"),
            },
        };
        // Weather says which place it opens on.
        let known_place = match (&self.catalog.entries[item as usize], &self.settings.weather_place) {
            (Entry::Builtin(Builtin::Weather), Some(place)) => Some(if place.region.is_empty() { place.name.clone() } else { format!("{}, {}", place.name, place.region) }),
            _ => None,
        };
        let primary = match &self.catalog.entries[item as usize] {
            Entry::Integration(tool) => Op::OpenVault(*tool),
            _ => Op::RunItem { item, key: key.clone() },
        };
        Row {
            title: index.title(item).to_string(),
            subtitle: if running { "Running".to_string() } else { known_place.unwrap_or_else(|| index.subtitle(item).to_string()) },
            kind,
            icon,
            accent: false,
            keys: self.catalog.hotkey_label(item).map(str::to_string),
            accessory: None,
            primary: Some(primary),
            source: RowSource::Catalog { item, key },
        }
    }

    fn request_scope_rows(&mut self, scope: &Scope, query: &str) {
        self.seq += 1;
        let data_scope = match scope {
            Scope::Notes => DataScope::Notes,
            Scope::Snippets => DataScope::Snippets,
            _ => DataScope::Clipboard,
        };
        if data_scope == DataScope::Clipboard && !self.settings.clipboard.enabled {
            self.view.rows.clear();
            self.view.rows.push(Row::op(
                "Turn on clipboard history",
                "Stored only on this PC. Password managers and excluded apps are skipped.",
                GLYPH_CLIPBOARD,
                Op::EnableClipboard,
            ));
            self.view.rows.push(Row::info("Clipboard history is off", "Nothing you copy is recorded", GLYPH_INFO));
            return;
        }
        self.worker.search(self.seq, data_scope, query.to_string(), 200);
    }

    fn on_data(&mut self) {
        while let Ok(response) = self.worker.rx.try_recv() {
            self.on_response(response);
        }
    }

    fn on_response(&mut self, response: Resp) {
        match response {
            Resp::Results { seq, scope, query, rows } => {
                if seq == self.seq {
                    self.merge_results(scope, &query, rows);
                }
            }
            Resp::ClipPreview(mut preview) => {
                if self.visible && self.clip_preview_wanted == Some(preview.id) {
                    preview.item.source_app = self.source_name(&preview.item.source_app);
                    self.clip_preview = Some(preview);
                    self.redraw();
                }
            }
            Resp::Icon { hash, size, pixels } => {
                let has_pixels = pixels.is_some();
                self.icons.insert(hash, size, pixels);
                if has_pixels && self.visible {
                    self.redraw();
                }
            }
            Resp::Usage(records) => {
                self.catalog.apply_usage(&records, now_unix());
            }
            Resp::NoteBody { id, body } => {
                crate::editor::open_note(self.editor_context(), Some(id), &body);
            }
            Resp::NoteSaved { token, id, editor } => {
                editor.post(crate::editor::WM_EDITOR_SAVED, token as usize, id as isize);
            }
            Resp::NoteText { text } => {
                if clipboard::write_text(&text) {
                    self.report("Note copied");
                }
            }
            Resp::NoteRestored => self.report("Note restored"),
            Resp::Snippet { snippet, purpose } => self.on_snippet(snippet, purpose),
            Resp::SnippetSaved { id, conflict, editor } => {
                if editor.0 == self.main.0 as isize {
                    if self.form.as_ref().is_some_and(|form| form.saving) {
                        let message = self.form.as_ref().map_or_else(String::new, |form| format!("Saved “{}”", form.fields[0].value.trim()));
                        self.finish_form(message);
                    }
                    if self.parked_form.as_ref().is_some_and(|(form, _)| form.saving) {
                        self.parked_form = None;
                    }
                } else {
                    editor.post(crate::editor::WM_EDITOR_SAVED, 0, id as isize);
                }
                if let Some(other) = conflict {
                    self.report(&format!("Keyword also used by “{other}”. The older snippet wins."));
                }
                self.refresh_if_data_scope();
                crate::settings_win::snippets_changed();
            }
            Resp::SnippetList(rows) => crate::settings_win::snippets_listed(rows),
            Resp::Triggers(list) => {
                if let Some(hook) = &self.hook {
                    hook.set_triggers(list);
                }
            }
            Resp::Clip { kind, text, blob, purpose, formats } => self.on_clip_content(kind, text, blob, purpose, formats),
            Resp::Changed => {
                self.refresh_if_data_scope();
                crate::settings_win::snippets_changed();
            }
            Resp::Exported(result) => match result {
                Ok(path) => {
                    self.report(&format!("Exported to {}", path.display()));
                    launch::spawn(LaunchRequest::open(&path.to_string_lossy()), self.send, WM_ORBIT_LAUNCH_FAILED);
                }
                Err(why) => self.report(&format!("Export failed: {why}")),
            },
            Resp::HaulImported { row, links, left_out, result } => self.on_haul_imported(row, links, left_out, result),
            Resp::Imported(result) => match result {
                Ok(summary) => {
                    if crate::setup::is_open() {
                        crate::setup::orbit_file_imported(&summary, true);
                    } else {
                        self.report(&summary);
                    }
                    // Imported snippets may bring triggers the running hook hasn't heard of.
                    if self.triggers_needed() {
                        self.worker.send(Req::SnippetTriggers);
                    }
                    crate::settings_win::snippets_changed();
                }
                Err(why) => {
                    if crate::setup::is_open() {
                        crate::setup::orbit_file_imported(&format!("Couldn't import it: {why}."), false);
                    } else {
                        self.report(&format!("Import failed: {why}"));
                    }
                }
            },
            Resp::Counts { notes, snippets, clips, sqlite_bytes } => {
                self.last_counts = Some((notes, snippets, clips, sqlite_bytes));
                if self.view.scope == Scope::Diagnostics {
                    self.view.static_rows = self.diagnostics_rows();
                    let query = self.read_query();
                    self.view.filter_static(&query);
                    self.redraw();
                }
            }
            Resp::Failed(message) => match self.form.as_ref().filter(|form| form.saving).map(|form| form.at) {
                Some(at) => self.show_form_problem((at, message)),
                None => self.report(&message),
            },
            Resp::Finished => unsafe { PostQuitMessage(0) },
        }
    }

    fn refresh_if_data_scope(&mut self) {
        if self.visible && self.view.scope.is_data() {
            let selected = self.view.selected;
            self.refresh_results();
            self.view.select(selected);
        }
    }

    /// Adds results from the data thread. In the root list they go below what is already shown,
    /// so a selection cannot jump under the user's fingers.
    fn merge_results(&mut self, scope: DataScope, query: &str, rows: Vec<DataRow>) {
        let now = now_unix();
        let converted = rows.into_iter().map(|row| data_row(row, now));
        match scope {
            DataScope::Root => {
                // A menu may have opened since this search started.
                if self.view.scope != Scope::Root {
                    return;
                }
                let at = self.view.async_insert_at.min(self.view.rows.len());
                self.view.rows.splice(at..at, converted);
                if let Some(q) = self.web_row_waiting.take() {
                    if self.root_files_start() == 0 {
                        self.push_web_row(&q);
                    }
                }
            }
            DataScope::Notes => {
                self.view.rows = converted.collect();
                let q = query.trim();
                let title = if q.is_empty() { "New note".to_string() } else { format!("New note “{q}”") };
                self.view.rows.insert(0, Row::op(title, "", GLYPH_ADD, Op::NewNote));
                if self.view.rows.len() > 1 && !q.is_empty() {
                    let new_row = self.view.rows.remove(0);
                    self.view.rows.push(new_row);
                }
                if self.can_undo_trash && q.is_empty() {
                    self.view.rows.insert(0, Row::op("Undo: restore the note you just trashed", "", GLYPH_DELETE, Op::NoteUndoTrash));
                }
            }
            DataScope::Snippets => {
                self.view.rows = converted.collect();
                self.view.rows.push(Row::op("Create Snippet", "", GLYPH_ADD, Op::NewSnippet));
            }
            DataScope::Clipboard => {
                self.view.rows = converted.collect();
                if self.clip_paused {
                    self.status = Some("Clipboard history is paused".into());
                }
            }
        }
        self.view.select(self.view.selected);
        self.after_selection_change();
        self.redraw();
    }

    fn request_visible_icons(&mut self) {
        // A menu's rows are the view's while it's open, and its icons are smaller.
        let size = ui::scale(if self.menu.is_some() { ui::POPOVER_ICON_DIP } else { ui::ICON_DIP }, self.dpi) as usize;
        let first = self.view.scroll;
        // Two more than fit, for the rows that fade out under the footer.
        let last = (first + self.view.visible_rows + 2).min(self.view.rows.len());
        for index in first..last {
            match self.view.rows[index].icon {
                RowIcon::App(hash) => {
                    if self.icons.touch(hash, size) {
                        self.worker.send(Req::LoadIcon { hash, size });
                    }
                }
                RowIcon::Image(id) => {
                    let hash = thumb_hash(id);
                    let size = ui::scale(ui::THUMB_DIP, self.dpi) as usize;
                    if self.icons.touch(hash, size) {
                        self.worker.send(Req::LoadClipThumb { id, hash, size });
                    }
                }
                RowIcon::Favicon(key) => {
                    let hash = crate::favicon::row_hash(key);
                    if self.icons.touch(hash, size) {
                        self.worker.send(Req::LoadFavicon { key, hash, size });
                    }
                }
                RowIcon::File(hash, _) => self.request_file_icon(hash, size),
                RowIcon::Glyph(_) | RowIcon::Tinted(..) | RowIcon::Emoji(_) => {}
            }
        }
    }

    // --- Running things -------------------------------------------------------------------

    fn app_id_of(&self, row: &Row) -> Option<String> {
        if let RowSource::Catalog { item, .. } = &row.source {
            if let Some(Entry::App(i)) = self.catalog.entries.get(*item as usize) {
                return self.catalog.apps.get(*i).map(|app| app.id.clone());
            }
        }
        None
    }

    fn activate_selected(&mut self, secondary: bool, t_enter: i64) {
        let Some(row) = self.view.selected_row().cloned() else { return };
        let op = if secondary {
            secondary_for(&row, self.app_id_of(&row).as_deref()).map(|(_, op)| op)
        } else {
            row.primary.clone()
        };
        if let Some(op) = op {
            self.execute(op);
            self.metrics.record_action("enter", t_enter, qpc());
        }
    }

    /// What Enter copies from an answer, by the setting: the bare number or the formatted one. Ctrl+Shift+Enter
    /// copies the other, where they differ.
    fn answer_copies(&self, row: &mut Row, plain: String, formatted: String, reverse: Option<String>, card: AnswerCard) {
        let formatted_first = self.settings.launcher.copy_formatted;
        let (first, second) = if formatted_first { (formatted, plain) } else { (plain, formatted) };
        let other = (second != first).then_some(second);
        row.primary = Some(Op::Copy(first));
        row.source = RowSource::Answer { other, other_formatted: !formatted_first, reverse, card: Box::new(card) };
    }

    /// The speed test is on screen, on its own or behind its menu.
    fn speedtest_shown(&self) -> bool {
        self.menu.as_ref().map_or(&self.view.scope, |menu| &menu.behind.scope) == &Scope::SpeedTest
    }

    /// What a hotkey on this row would run. A template link has nothing to open without a query.
    fn hotkey_target(&self, row: &Row) -> Option<Action> {
        match &row.source {
            RowSource::Catalog { item, .. } => self.catalog.entry_action(*item, true).filter(|action| {
                !matches!(action, Action::Url { url } if !link_params(url).is_empty())
            }),
            RowSource::Note { id, .. } => Some(Action::OpenNote { id: *id }),
            RowSource::Snippet { id } => Some(Action::InsertSnippet { id: *id }),
            RowSource::Plain | RowSource::Header | RowSource::Clip { .. } | RowSource::Secret { .. } | RowSource::Emoji { .. } | RowSource::Answer { .. } | RowSource::SpeedTest { .. } | RowSource::Clock { .. } | RowSource::Colour(_) | RowSource::File { .. } | RowSource::Extension { .. } => None,
        }
    }

    fn open_action_menu(&mut self) {
        // The same key closes it, whichever menu it is.
        if self.menu.is_some() {
            self.close_menu();
            return;
        }
        let Some((row, menu)) = self.actions_for_selected() else { return };
        self.open_menu(MenuSide::Right, row.title.clone(), menu);
    }

    /// Runs the entry of the selected row's action menu that shows these keys, as if it had
    /// been picked there. With the menu open, its own entries are looked at instead.
    fn run_shortcut(&mut self, label: &str) -> bool {
        let wanted = |row: &Row| row.keys.as_deref() == Some(label) && !matches!(row.primary, Some(Op::RecordHotkey { .. }) | None);
        if self.menu.is_some() {
            let Some(op) = self.view.rows.iter().find(|row| wanted(row)).and_then(|row| row.primary.clone()) else { return false };
            self.execute(op);
            return true;
        }
        let Some((row, menu)) = self.actions_for_selected() else { return false };
        let Some(op) = menu.iter().find(|entry| wanted(entry)).and_then(|entry| entry.primary.clone()) else { return false };
        // Picked from the menu, so that what it does afterwards, such as going back to the list,
        // is the same either way. The menu is never drawn.
        self.open_menu(MenuSide::Right, row.title.clone(), menu);
        self.execute(op);
        if matches!(&self.view.scope, Scope::Actions { of } if *of == row.title) {
            self.leave_menu();
        }
        self.redraw();
        true
    }

    /// The selected row and the entries of its action menu, where there is one.
    fn actions_for_selected(&self) -> Option<(Row, Vec<Row>)> {
        if matches!(self.view.scope, Scope::Actions { .. } | Scope::Confirm(_) | Scope::SetAlias { .. } | Scope::SetHotkey { .. } | Scope::LinkArgs { .. } | Scope::Form { .. }) {
            return None;
        }
        let row = self.view.selected_row().cloned()?;
        if row.source == RowSource::Plain {
            return None;
        }
        let pinned = match &row.source {
            RowSource::Catalog { key, .. } => self.settings.pinned.iter().any(|p| p == key),
            _ => false,
        };
        let hotkey = self.hotkey_target(&row).map(|action| HotkeyChoice {
            current: hotkeys::hotkey_for(&self.settings, &action).map(|hotkey| hotkey.label("Win")),
            action,
        });
        let mut menu = menu_for(&row, self.app_id_of(&row).as_deref(), pinned, hotkey);
        // Things you made yourself can be changed from here too.
        if let RowSource::Catalog { item, .. } = &row.source {
            let own = match self.catalog.entries.get(*item as usize) {
                Some(Entry::Link(i)) => self.catalog.link(*i).map(|link| {
                    [
                        Row::op("Edit quicklink", "", GLYPH_EDIT, Op::EditLink(link.id.clone())).with_keys("Ctrl+E"),
                        Row::op("Delete quicklink", "", GLYPH_DELETE, Op::DeleteLink(link.id.clone())).with_keys("Ctrl+Shift+Del"),
                    ]
                }),
                Some(Entry::Custom(i)) => self.settings.custom_commands.get(*i).map(|command| {
                    [
                        Row::op("Edit command", "", GLYPH_EDIT, Op::EditCommand(command.id.clone())).with_keys("Ctrl+E"),
                        Row::op("Delete command", "", GLYPH_DELETE, Op::DeleteCommand(command.id.clone())).with_keys("Ctrl+Shift+Del"),
                    ]
                }),
                _ => None,
            };
            menu.extend(own.into_iter().flatten());
        }
        // A list that can be narrowed says so in every row's menu.
        if matches!(row.source, RowSource::Extension { .. }) && self.filter_label().is_some() {
            menu.push(Row::op("Filter\u{2026}", "", crate::rows::GLYPH_FILTER, Op::OpenFilterMenu).with_keys("Ctrl+P"));
        }
        Some((row, menu))
    }

    /// Orbit's own menu, from the pill with its logo: settings and the like, from wherever you are.
    pub fn open_orbit_menu(&mut self) {
        let was_orbit = self.menu.as_ref().is_some_and(|menu| menu.side == MenuSide::Left);
        self.leave_menu();
        if was_orbit {
            self.redraw();
            return;
        }
        if matches!(self.view.scope, Scope::Confirm(_) | Scope::SetAlias { .. } | Scope::SetHotkey { .. } | Scope::LinkArgs { .. } | Scope::Form { .. }) {
            return;
        }
        let command = |title: &str, glyph: char, command: Builtin| Row::op(title, "", glyph, Op::RunBuiltin(command));
        let suspended = if self.suspended {
            command("Resume Shortcuts", GLYPH_KEYBOARD, Builtin::ResumeShortcuts)
        } else {
            command("Suspend Shortcuts", GLYPH_KEYBOARD, Builtin::SuspendShortcuts)
        };
        let version = concat!("Orbit ", env!("CARGO_PKG_VERSION"));
        // Only what's about Orbit itself. Making a quicklink or a snippet, the emoji and the
        // diagnostics are all found by typing their names.
        let rows = vec![
            command("Orbit Settings", GLYPH_SETTINGS, Builtin::Settings).with_keys("Ctrl+,"),
            {
                let (title, subtitle) = crate::update::menu_row();
                Row::op(&title, &subtitle, crate::rows::GLYPH_UPDATE, Op::RunBuiltin(Builtin::CheckForUpdates))
            },
            command("Orbit Help", GLYPH_HELP, Builtin::ShowHelp),
            suspended,
            command("Restart Orbit", GLYPH_REFRESH, Builtin::RestartOrbit),
            command("Quit Orbit", GLYPH_POWER, Builtin::QuitOrbit),
        ];
        self.open_menu(MenuSide::Left, version.to_string(), rows);
    }

    /// Opens a small menu over whatever's showing, which waits behind it just as it was.
    fn open_menu(&mut self, side: MenuSide, of: String, rows: Vec<Row>) {
        // The menu fits the launcher as it is, rather than the launcher growing for the menu.
        let height = ui::launcher_height(self.dpi, &self.view.scope, &self.view.rows, self.view.grid.is_some(), self.settings.launcher.fit_results, self.full_rows() as i32);
        // Orbit's own menu, on the left, is short and always the same, so it shows whole. A
        // drop-down hangs from the top and has all the room down to the footer.
        let most = match side {
            MenuSide::Left => ui::popover_rows_most(height, self.dpi),
            MenuSide::Right => ui::popover_rows_for(height, self.dpi),
            MenuSide::Drop => ui::drop_rows_for(height, self.dpi),
        };
        let shown = rows.len().clamp(1, most);
        self.push_view();
        let behind = Behind {
            scope: self.view.scope.clone(),
            query: self.read_query(),
            rows: std::mem::take(&mut self.view.rows),
            static_rows: std::mem::take(&mut self.view.static_rows),
            scroll: self.view.scroll,
            selected: self.view.selected,
            grid: self.view.grid.take(),
        };
        self.menu = Some(Menu { side, shown, behind });
        self.view.scope = Scope::Actions { of };
        self.view.static_rows = rows;
        self.view.visible_rows = shown;
        self.view.reset_position();
        self.sync_vim_to_scope();
        self.set_placeholder();
        self.view.last_query = Some(String::new());
        self.set_query_text("");
        self.refresh_results();
    }

    /// Takes a small menu away and puts back what it covered, without asking anything again.
    fn leave_menu(&mut self) {
        let Some(menu) = self.menu.take() else { return };
        self.view.stack.pop();
        self.view.visible_rows = self.full_rows();
        let behind = menu.behind;
        self.view.scope = behind.scope;
        self.view.rows = behind.rows;
        self.view.static_rows = behind.static_rows;
        self.view.scroll = behind.scroll;
        self.view.selected = behind.selected;
        self.view.grid = behind.grid;
        self.view.last_query = Some(behind.query.clone());
        self.set_query_text(&behind.query);
        self.sync_vim_to_scope();
        self.set_placeholder();
        // A tool may have answered while the menu was up.
        if let Scope::Vault(tool) = self.view.scope {
            self.view.static_rows = self.vault_rows(tool);
            self.view.filter_static(&behind.query);
        }
        self.refresh_if_data_scope();
        self.after_selection_change();
        if let (Scope::Form { .. }, Some(at)) = (&self.view.scope, self.form.as_ref().map(|form| form.at)) {
            self.load_form_field(at);
        }
    }

    fn close_menu(&mut self) {
        self.leave_menu();
        self.status = None;
        self.restart_caret();
        self.redraw();
    }

    fn push_view(&mut self) {
        self.seq += 1; // results still on their way belong to the view being left
        self.web_row_waiting = None;
        let saved = SavedView {
            scope: self.view.scope.clone(),
            query: self.read_query(),
            selected: self.view.selected,
        };
        self.view.stack.push(saved);
    }

    /// Esc: anything typed goes first, and only then does it go back or close.
    fn escape(&mut self) {
        // Waiting for a password or code to arrive, Esc means stop that, whatever's typed.
        if self.waiting_value.is_none() && self.menu.is_some() && self.read_query().is_empty() {
            self.close_menu();
            return;
        }
        if self.waiting_value.is_some() || self.read_query().is_empty() {
            self.go_back_or_hide();
            return;
        }
        // Clearing the box tells on_query_changed, which redoes the list from the top.
        self.set_query_text("");
        self.restart_caret();
        self.redraw();
    }

    fn go_back_or_hide(&mut self) {
        self.end_recording();
        // Leaving the speed test stops it. Its own menu doesn't.
        if self.menu.is_none() && self.view.scope == Scope::SpeedTest {
            self.speedtest.stop();
        }
        // Backing out cancels a value that's on its way.
        self.waiting_value = None;
        self.hold_open = None;
        self.lost_focus_while_held = false;
        match self.view.stack.pop() {
            Some(saved) => {
                // What a menu covered goes straight back, while anything newer is on its way.
                let scroll = self.menu.take().map(|menu| {
                    self.view.visible_rows = self.full_rows();
                    self.view.rows = menu.behind.rows;
                    self.view.grid = menu.behind.grid;
                    menu.behind.scroll
                });
                if saved.scope != Scope::Emoji {
                    self.view.grid = None;
                }
                self.view.scope = saved.scope;
                self.view.static_rows.clear();
                self.build_static_rows();
                self.view.last_query = Some(saved.query.clone());
                self.set_query_text(&saved.query);
                self.sync_vim_to_scope();
                self.set_placeholder();
                self.status = None;
                self.refresh_results();
                if let Some(scroll) = scroll {
                    self.view.scroll = scroll;
                }
                self.view.select(saved.selected);
                self.after_selection_change();
                self.redraw();
            }
            None => self.hide(true),
        }
    }

    fn execute(&mut self, op: Op) {
        match op {
            op @ (Op::OpenFile(_) | Op::RevealFile(_) | Op::CopyPath(_) | Op::CopyFile(_) | Op::OpenFileWith(_) | Op::FileAsAdmin(_) | Op::TerminalHere(_) | Op::EverythingInstall | Op::EverythingStart | Op::EverythingRetry | Op::ExtAction { .. } | Op::OpenExtensionsSettings | Op::OpenFilterMenu | Op::ExtFilter(_)) => {
                self.execute_ext(op);
            }
            Op::Weather(name) => {
                self.enter_scope(Scope::Weather, qpc());
                self.weather_look_up(&name);
            }
            Op::SpeedTestAccept => {
                self.settings.speedtest_accepted = true;
                self.save_settings();
                self.speedtest.check();
                if self.speedtest.missing {
                    self.speedtest.install(self.send, WM_ORBIT_SPEEDTEST_INSTALLED);
                } else {
                    self.speedtest.start(self.send, WM_ORBIT_SPEEDTEST);
                }
                self.refresh_results();
                self.redraw();
            }
            Op::SpeedTestInstall => {
                self.speedtest.install(self.send, WM_ORBIT_SPEEDTEST_INSTALLED);
                self.refresh_results();
                self.redraw();
            }
            Op::SpeedTestStart => {
                self.speedtest.start(self.send, WM_ORBIT_SPEEDTEST);
                self.refresh_results();
                self.redraw();
            }
            Op::SpeedTestStop => {
                self.speedtest.stop();
                self.refresh_results();
                self.redraw();
            }
            Op::TimerStart(seconds) => {
                self.leave_menu();
                self.countdown = Some(crate::clock::Countdown::start(seconds));
                if self.view.scope != Scope::Timer {
                    self.enter_scope(Scope::Timer, qpc());
                }
                self.after_clock_change(true);
            }
            Op::TimerPause | Op::TimerResume | Op::TimerAdd(_) | Op::TimerCancel => {
                self.leave_menu();
                match op {
                    Op::TimerPause => self.countdown.iter_mut().for_each(|timer| timer.pause()),
                    Op::TimerResume => self.countdown.iter_mut().for_each(|timer| timer.resume()),
                    Op::TimerAdd(seconds) => self.countdown.iter_mut().for_each(|timer| timer.add(seconds)),
                    _ => self.countdown = None,
                }
                self.after_clock_change(false);
            }
            Op::StopwatchStart | Op::StopwatchPause | Op::StopwatchReset => {
                self.leave_menu();
                match op {
                    Op::StopwatchStart => self.stopwatch.start(),
                    Op::StopwatchPause => self.stopwatch.pause(),
                    _ => self.stopwatch.reset(),
                }
                self.after_clock_change(false);
            }
            Op::PickColour => {
                self.leave_menu();
                self.pick_colour();
            }
            Op::PasteText(text) => {
                self.insert_target = self.prev_foreground;
                self.hide(true);
                self.start_insert(text, 0, None, false);
            }
            Op::SetQuery(text) => {
                self.leave_menu();
                self.set_query_text(&text);
                self.refresh_results();
                self.redraw();
            }
            Op::WeatherLookup(name) => {
                self.weather_look_up(&name);
                self.set_query_text("");
                self.refresh_results();
                self.redraw();
            }
            Op::OpenVault(tool) => {
                self.worker.send(Req::RecordUsage(tool.key().to_string()));
                self.enter_scope(Scope::Vault(tool), qpc());
            }
            Op::RefreshVault(tool) => {
                self.vaults.remove(&tool);
                // Back in the list, when this came from its action menu. Going back asks the tool
                // by itself, and asking twice could mean a second approval prompt.
                let mut went_back = false;
                while matches!(self.view.scope, Scope::Actions { .. }) && !self.view.stack.is_empty() {
                    self.go_back_or_hide();
                    went_back = true;
                }
                if !went_back && self.view.scope == Scope::Vault(tool) {
                    self.build_static_rows();
                    self.refresh_results();
                }
            }
            Op::CopySecret { tool, id, field } => self.ask_tool_for_value(tool, id, field, false),
            Op::PasteSecret { tool, id, field } => self.ask_tool_for_value(tool, id, field, true),
            Op::RunItem { item, key } => {
                let Some(action) = self.catalog.entry_action(item, self.settings.launcher.focus_running_app) else { return };
                self.worker.send(Req::RecordUsage(key));
                // A link with blanks asks for them rather than opening a broken address.
                if let Action::Url { url } = &action {
                    if !link_params(url).is_empty() {
                        let name = match self.catalog.entries.get(item as usize) {
                            Some(Entry::Link(i)) => self.catalog.link(*i).map(|l| l.name.clone()).unwrap_or_default(),
                            _ => String::new(),
                        };
                        self.enter_link_args(name, url.clone(), Vec::new());
                        return;
                    }
                }
                self.run_action(&action, Some(item));
            }
            Op::AskConfirm(command) => {
                self.leave_menu();
                self.push_view();
                self.view.scope = Scope::Confirm(command);
                self.view.reset_position();
                self.sync_vim_to_scope();
                self.set_placeholder();
                self.view.last_query = Some(String::new());
                self.set_query_text("");
                self.build_static_rows();
                self.refresh_results();
            }
            Op::Confirmed(command) => {
                self.hide(true);
                self.perform_builtin(command);
            }
            Op::Back => self.go_back_or_hide(),
            Op::Copy(text) => {
                self.hide(true);
                if !clipboard::write_text(&text) {
                    self.report("Couldn't reach the clipboard");
                }
            }
            Op::OpenUrl(url) => {
                self.hide(false);
                self.open_url(&url);
            }
            Op::OpenLink { target, open_with } => {
                self.hide(false);
                self.open_link(&target, &open_with);
            }
            Op::FormOpenWith(id) => {
                self.leave_menu();
                let name = self.catalog.app_by_id(&id).map(|app| app.name.clone()).unwrap_or_default();
                if let Some(form) = self.form.as_mut() {
                    form.set_open_with(if name.is_empty() { "" } else { &id }, &name);
                }
                if !name.is_empty() {
                    self.request_app_icon(&id);
                }
                if let Some(at) = self.form.as_ref().map(|form| form.at) {
                    self.load_form_field(at);
                }
                self.restart_caret();
                self.redraw();
            }
            Op::AppNewWindow(id) => {
                self.hide(false);
                launch::spawn(LaunchRequest::app(&id, false), self.send, WM_ORBIT_LAUNCH_FAILED);
            }
            Op::AppAsAdmin(id) => {
                self.hide(false);
                launch::spawn(LaunchRequest::app(&id, true), self.send, WM_ORBIT_LAUNCH_FAILED);
            }
            Op::AppFolder(id) => {
                self.hide(false);
                let target = self.catalog.app_by_id(&id).map(|app| app.exe.clone()).unwrap_or_default();
                if target.is_empty() {
                    self.report("This app has no file location. It's a packaged app.");
                } else if let Some(folder) = std::path::Path::new(&target).parent() {
                    launch::spawn(LaunchRequest::open(&folder.to_string_lossy()), self.send, WM_ORBIT_LAUNCH_FAILED);
                }
            }
            Op::AppEndTask(id) => {
                let Some(app) = self.catalog.app_by_id(&id).cloned() else { return };
                self.leave_menu();
                let message = match launch::end_task(&app) {
                    Ok(_) => format!("Ended {}", app.name),
                    Err(why) => format!("Couldn't end {}. {why}", app.name),
                };
                // It's no longer running, so its row stops saying so.
                self.running_apps = None;
                self.refresh_results();
                self.report(&message);
            }
            Op::TogglePin(key) => {
                if let Some(at) = self.settings.pinned.iter().position(|p| *p == key) {
                    self.settings.pinned.remove(at);
                } else {
                    self.settings.pinned.push(key);
                }
                self.save_and_rebuild_catalog();
                self.go_back_or_hide();
            }
            Op::MoveFavorite(key, by) => {
                if let Some(at) = self.settings.pinned.iter().position(|p| *p == key) {
                    let to = (at as i32 + by).clamp(0, self.settings.pinned.len() as i32 - 1) as usize;
                    self.settings.pinned.swap(at, to);
                }
                self.save_and_rebuild_catalog();
                self.go_back_or_hide();
            }
            Op::HideItem(key) => {
                if !self.settings.hidden.contains(&key) {
                    self.settings.hidden.push(key);
                }
                self.save_and_rebuild_catalog();
                self.go_back_or_hide();
                self.status = Some("Hidden. Settings can bring it back.".into());
                self.redraw();
            }
            Op::RunBuiltin(command) => {
                self.leave_menu();
                self.run_builtin(command);
            }
            Op::EmojiPaste(at) => {
                let Some(glyph) = self.emoji_table.get(at as usize).map(|entry| entry.glyph) else { return };
                self.settings.emoji.note_used(glyph);
                self.save_settings();
                self.insert_target = self.prev_foreground;
                self.hide(true);
                self.start_insert(glyph.to_string(), 0, None, false);
            }
            Op::EmojiCopy(at) => {
                let Some(glyph) = self.emoji_table.get(at as usize).map(|entry| entry.glyph) else { return };
                self.settings.emoji.note_used(glyph);
                self.save_settings();
                self.hide(true);
                if !clipboard::write_text(glyph) {
                    self.report("Couldn't reach the clipboard");
                }
            }
            Op::EmojiPin(at, pinned) => {
                let Some(glyph) = self.emoji_table.get(at as usize).map(|entry| entry.glyph) else { return };
                self.settings.emoji.pinned.retain(|other| other != glyph);
                if pinned {
                    self.settings.emoji.pinned.push(glyph.to_string());
                }
                self.save_settings();
                self.go_back_or_hide();
                // It may have moved, and it's still the one you were on.
                let found = self.view.rows.iter().position(|row| row.icon == RowIcon::Emoji(at));
                if let Some(found) = found {
                    self.view.select(found);
                    self.after_selection_change();
                    self.redraw();
                }
            }
            Op::NewLinkFor(url) => {
                self.leave_menu();
                let link = orbit_core::settings::QuickLink { id: String::new(), name: String::new(), url, alias: String::new(), open_with: String::new() };
                self.open_link_form(Some(link));
            }
            Op::EditLink(id) => {
                let link = self.settings.links.iter().find(|link| link.id == id).cloned();
                self.open_link_form(link);
            }
            Op::DeleteLink(id) => {
                self.delete_link(&id);
                self.go_back_or_hide();
                self.status = Some("Quicklink deleted".into());
                self.redraw();
            }
            Op::EditCommand(id) => {
                let command = self.settings.custom_commands.iter().find(|command| command.id == id).cloned();
                self.hide(false);
                self.open_command_form(command);
            }
            Op::DeleteCommand(id) => {
                self.delete_command(&id);
                self.go_back_or_hide();
                self.status = Some("Command deleted".into());
                self.redraw();
            }
            Op::AliasPrompt { key, title } => {
                // It takes the menu's place. Going back from here goes to the list behind it.
                if let Some(menu) = self.menu.take() {
                    self.view.visible_rows = self.full_rows();
                    self.view.grid = None;
                    drop(menu);
                }
                self.view.scope = Scope::SetAlias { key, of: title };
                self.view.reset_position();
                self.view.last_query = Some(String::new());
                self.set_query_text("");
                self.sync_vim_to_scope();
                self.set_placeholder();
                self.refresh_results();
            }
            Op::LinkArgs { id, values } => {
                if let Some(link) = self.settings.links.iter().find(|link| link.id == id).cloned() {
                    self.enter_link_args(link.name, link.url, values);
                }
            }
            Op::LinkArgsNext => {
                let typed = self.read_query();
                let next = self.link_args.as_mut().and_then(|args| {
                    args.values[args.at] = typed;
                    missing_param(&args.url, &args.values).map(|at| (at, args.params[at].label.clone()))
                });
                if let Some((at, label)) = next {
                    self.focus_link_arg(at);
                    self.status = Some(format!("Fill in {label} first"));
                    self.redraw();
                }
            }
            Op::SaveAlias { key } => {
                let alias = self.read_query().trim().to_string();
                if !alias.is_empty() {
                    // A list's alias is its keyword, as on Settings' Search page.
                    if self.opens_with_keyword(&key) {
                        crate::settings_win::set_item_keyword(&mut self.settings, &key, &alias);
                    } else {
                        self.settings.aliases.insert(key, alias);
                    }
                    self.save_and_rebuild_catalog();
                    crate::settings_win::aliases_changed(&self.settings);
                }
                self.go_back_or_hide();
            }
            Op::NoteOpen(id) => {
                self.hide(false);
                self.worker.send(Req::NoteOpen { id });
            }
            Op::NoteCopy(id) => {
                self.hide(true);
                self.worker.send(Req::NoteCopy { id });
            }
            Op::NotePin(id, pinned) => {
                self.worker.send(Req::NoteSetPinned { id, pinned });
                self.go_back_or_hide();
            }
            Op::NoteTrash(id) => {
                self.worker.send(Req::NoteTrash { id });
                self.can_undo_trash = true;
                self.go_back_or_hide();
                self.status = Some("Note moved to trash. Undo is at the top of Notes.".into());
                self.redraw();
            }
            Op::NoteUndoTrash => {
                self.can_undo_trash = false;
                self.worker.send(Req::NoteRestoreLast);
            }
            Op::NewNote => {
                let seed = if self.view.scope == Scope::Notes { self.read_query() } else { String::new() };
                self.hide(false);
                crate::editor::open_note(self.editor_context(), None, seed.trim());
            }
            Op::SnippetInsert(id) => {
                self.insert_target = self.prev_foreground;
                self.hide(true);
                self.worker.send(Req::SnippetGet { id, purpose: Purpose::Insert });
            }
            Op::SnippetCopy(id) => {
                self.hide(true);
                self.worker.send(Req::SnippetGet { id, purpose: Purpose::Copy });
            }
            Op::SnippetEdit(id) => {
                self.hide(false);
                self.worker.send(Req::SnippetGet { id, purpose: Purpose::Edit });
            }
            Op::SnippetDelete(id) => {
                self.worker.send(Req::SnippetDelete { id });
                self.go_back_or_hide();
            }
            Op::NewSnippet => {
                self.open_form(InlineForm::snippet(None));
            }
            Op::ClipPaste(id) => {
                self.insert_target = self.prev_foreground;
                self.hide(true);
                self.worker.send(Req::ClipGet { id, purpose: Purpose::Insert });
            }
            Op::ClipPastePlain(id) => {
                self.insert_target = self.prev_foreground;
                self.hide(true);
                self.worker.send(Req::ClipGet { id, purpose: Purpose::InsertPlain });
            }
            Op::ClipOpen(id) => {
                self.hide(false);
                self.worker.send(Req::ClipGet { id, purpose: Purpose::Open });
            }
            Op::ClipCopy(id) => {
                self.hide(true);
                self.worker.send(Req::ClipGet { id, purpose: Purpose::Copy });
            }
            Op::ClipPin(id, pinned) => {
                self.worker.send(Req::ClipSetPinned { id, pinned });
                self.go_back_or_hide();
            }
            Op::ClipDelete(id) => {
                self.worker.send(Req::ClipDelete { id });
                self.go_back_or_hide();
            }
            Op::EnableClipboard => {
                let mut settings = self.settings.clone();
                settings.clipboard.enabled = true;
                self.apply_settings(settings);
                self.reopen_settings();
                self.refresh_results();
            }
            Op::RecordHotkey { action, title } => {
                // Listening for keys takes the menu's place, as setting an alias does.
                if self.menu.take().is_some() {
                    self.view.visible_rows = self.full_rows();
                }
                self.begin_recording(action, title);
            }
            Op::ClearHotkey(action) => {
                self.end_recording();
                let mut settings = self.settings.clone();
                hotkeys::clear(&mut settings, &action);
                self.apply_settings(settings);
                crate::settings_win::hotkeys_changed(&self.settings);
                self.go_back_or_hide();
                self.status = Some("Hotkey removed".to_string());
                self.redraw();
            }
            Op::ConfirmHotkey => {
                if let Some(hotkey) = self.recording.as_mut().and_then(|recording| recording.pending.take()) {
                    self.finish_hotkey(hotkey);
                }
            }
            Op::RecordAgain => {
                if let Some(recording) = self.recording.as_mut() {
                    recording.pending = None;
                }
                self.show_recording_rows();
            }
            Op::InsertPending => {
                let pending = self.pending_expansion.take();
                self.hide(true);
                if let Some((expanded, erase)) = pending {
                    self.start_insert(expanded.text, erase, expanded.cursor_back, false);
                }
            }
        }
    }

    pub fn run_action(&mut self, action: &Action, _item: Option<u32>) {
        match action {
            Action::FocusOrLaunchApp { id, name } => {
                let found = self.catalog.app_by_id(id).or_else(|| self.catalog.app_by_name(name)).cloned();
                let Some(app) = found else {
                    self.hide(false);
                    self.report(&format!("{name} isn't installed, or the app index is out of date. Try Rebuild App Index."));
                    return;
                };
                self.hide(false);
                match launch::find_running_window(&app) {
                    Some(window) => {
                        if !launch::force_foreground(window) {
                            self.report(&format!("Windows didn't let Orbit bring {} to the front.", app.name));
                        }
                    }
                    None => launch::spawn(LaunchRequest::app(&app.id, false), self.send, WM_ORBIT_LAUNCH_FAILED),
                }
            }
            Action::LaunchApp { id, name } => {
                let found = self.catalog.app_by_id(id).or_else(|| self.catalog.app_by_name(name)).cloned();
                self.hide(false);
                match found {
                    Some(app) => launch::spawn(LaunchRequest::app(&app.id, false), self.send, WM_ORBIT_LAUNCH_FAILED),
                    None => self.report(&format!("{name} isn't installed, or the app index is out of date.")),
                }
            }
            Action::OpenPath { path } => {
                self.hide(false);
                launch::spawn(LaunchRequest::open(path), self.send, WM_ORBIT_LAUNCH_FAILED);
            }
            Action::Url { url } => {
                // From a hotkey: a link with blanks opens the launcher to ask for them.
                if !link_params(url).is_empty() {
                    let name = self.settings.links.iter().find(|link| link.url == *url).map_or_else(|| "Quicklink".to_string(), |link| link.name.clone());
                    if !self.visible {
                        self.show(Scope::Root, "link", qpc());
                    }
                    self.enter_link_args(name, url.clone(), Vec::new());
                    return;
                }
                self.hide(false);
                let open_with = self.open_with_for(url);
                self.open_link(url, &open_with);
            }
            Action::Builtin { command } => self.run_builtin(*command),
            Action::Custom { id } => {
                let command = self.settings.custom_commands.iter().find(|c| c.id == *id).cloned();
                self.hide(false);
                match command {
                    Some(command) => launch::spawn(
                        LaunchRequest {
                            file: command.program,
                            parameters: command.arguments,
                            working_dir: command.working_dir,
                            run_as_admin: false,
                        },
                        self.send,
                        WM_ORBIT_LAUNCH_FAILED,
                    ),
                    None => self.report("That command no longer exists."),
                }
            }
            Action::OpenNote { id } => {
                self.hide(false);
                self.worker.send(Req::NoteOpen { id: *id });
            }
            Action::InsertSnippet { id } => {
                self.insert_target = if self.visible { self.prev_foreground } else { unsafe { GetForegroundWindow() } };
                self.hide(true);
                self.worker.send(Req::SnippetGet { id: *id, purpose: Purpose::Insert });
            }
            Action::Extension { ext, command } => self.open_extension_command(ext, command),
        }
    }

    fn ask_tool_for_items(&mut self, tool: Tool) {
        self.tool_request += 1;
        self.hold_open = Some(Instant::now());
        self.lost_focus_while_held = false;
        self.vaults.insert(tool, Vault::default());
        integrations::request_items(tool, self.tool_request, self.send, WM_ORBIT_CLI);
    }

    fn ask_tool_for_value(&mut self, tool: Tool, id: String, field: integrations::Field, paste: bool) {
        self.tool_request += 1;
        self.waiting_value = Some(self.tool_request);
        self.paste_value = paste;
        self.hold_open = Some(Instant::now());
        self.lost_focus_while_held = false;
        self.status = Some(format!("Asking {}…", tool.name()));
        self.redraw();
        integrations::request_value(tool, id, field, now_unix(), self.tool_request, self.send, WM_ORBIT_CLI);
    }

    /// The tool has answered, so nothing is keeping the launcher open any more.
    fn end_hold(&mut self) {
        let Some(asked_at) = self.hold_open.take() else { return };
        if !std::mem::take(&mut self.lost_focus_while_held) || !self.visible {
            return;
        }
        if asked_at.elapsed().as_secs() >= HOLD_RETURN_SECS {
            self.hide(false);
            return;
        }
        unsafe {
            let _ = SetWindowPos(self.main, Some(HWND_TOPMOST), 0, 0, 0, 0, SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE);
        }
        self.sync_plate();
        launch::force_foreground(self.main);
        unsafe {
            let _ = SetFocus(Some(self.edit));
        }
    }

    fn on_tool_answer(&mut self, answer: integrations::Answer) {
        match answer {
            integrations::Answer::Items { tool, seq, result } => {
                if seq == self.tool_request {
                    self.end_hold();
                }
                let Some(vault) = self.vaults.get_mut(&tool) else { return };
                match result {
                    Ok(items) => {
                        vault.items = items;
                        vault.failure = None;
                    }
                    // A fresh list that couldn't be fetched doesn't take away one that's on screen.
                    Err(_) if vault.loaded && !vault.items.is_empty() => {}
                    Err(failure) => vault.failure = Some(failure),
                }
                vault.loaded = true;
                unsafe {
                    let _ = SetTimer(Some(self.main), TIMER_FORGET_VAULTS, VAULT_KEEP_MS, None);
                }
                if self.visible && self.view.scope == Scope::Vault(tool) {
                    self.view.static_rows = self.vault_rows(tool);
                    self.refresh_results();
                }
            }
            integrations::Answer::Value { tool, field, seq, result } => {
                // Esc was pressed, or something newer was asked for. The value is wiped as it goes.
                if self.waiting_value != Some(seq) {
                    return;
                }
                self.waiting_value = None;
                match result {
                    Ok(secret) if self.paste_value => self.paste_secret(secret),
                    Ok(secret) => self.copy_secret(field, &secret),
                    Err(failure) => {
                        self.end_hold();
                        self.report(&failure.message(tool));
                    }
                }
            }
        }
    }

    fn copy_secret(&mut self, field: integrations::Field, secret: &integrations::Secret) {
        let Some(sequence) = clipboard::write_secret(&secret.text) else {
            self.end_hold();
            self.report("Couldn't reach the clipboard");
            return;
        };
        // An approval prompt took focus, so going back to the app needs more than asking nicely.
        let prompted = self.lost_focus_while_held;
        self.hide(true);
        if prompted && !self.prev_foreground.is_invalid() {
            launch::force_foreground(self.prev_foreground);
        }
        // A username isn't a secret, so it can stay, and it replaces anything waiting to clear.
        self.secret_on_clipboard = None;
        unsafe {
            let _ = KillTimer(Some(self.main), TIMER_CLEAR_SECRET);
        }
        if field != integrations::Field::Username {
            self.secret_on_clipboard = Some(sequence);
            unsafe {
                let _ = SetTimer(Some(self.main), TIMER_CLEAR_SECRET, SECRET_CLEAR_MS, None);
            }
        }
        if let Some(left) = secret.seconds_left.filter(|&left| left < 8) {
            self.notify("Orbit", &format!("Code copied. It expires in {left} seconds, so the next one may be safer."));
        }
    }

    /// Types a code into the window the launcher was opened over, on a thread of its own, so the
    /// clipboard isn't used unless it can't be typed there.
    fn paste_secret(&mut self, secret: integrations::Secret) {
        // An approval prompt took focus, so going back to the app needs more than asking nicely.
        let prompted = self.lost_focus_while_held;
        let target = self.prev_foreground;
        self.hide(true);
        if prompted && !target.is_invalid() {
            launch::force_foreground(target);
        }
        let target = SendHwnd::new(if target.is_invalid() { unsafe { GetForegroundWindow() } } else { target });
        if let Some(left) = secret.seconds_left.filter(|&left| left < 8) {
            self.notify("Orbit", &format!("Code pasted. It expires in {left} seconds, so if it's refused, try the next one."));
        }
        let main = self.send;
        let _ = std::thread::Builder::new().name("orbit-type-secret".into()).stack_size(256 * 1024).spawn(move || {
            let (code, sequence) = match clipboard::type_secret(target.get(), &secret.text) {
                clipboard::SecretTyped::Typed => (0, 0),
                clipboard::SecretTyped::OnClipboard(sequence) => (1, sequence),
                clipboard::SecretTyped::Failed => (2, 0),
            };
            // The code is wiped from memory here, as it goes.
            drop(secret);
            main.post(WM_ORBIT_SECRET_TYPED, code, sequence as isize);
        });
    }

    /// A code that couldn't be typed went on the clipboard instead, so it clears as a copied one does.
    fn on_secret_typed(&mut self, outcome: usize, sequence: u32) {
        match outcome {
            1 => {
                self.secret_on_clipboard = Some(sequence);
                unsafe {
                    let _ = SetTimer(Some(self.main), TIMER_CLEAR_SECRET, SECRET_CLEAR_MS, None);
                }
                self.report("Orbit couldn't type into that window, so the code is on your clipboard for 30 seconds. Press Ctrl+V to paste it.");
            }
            2 => self.report("Couldn't paste the code, or put it on the clipboard"),
            _ => {}
        }
    }

    fn clear_secret_now(&mut self) {
        if let Some(sequence) = self.secret_on_clipboard.take() {
            clipboard::clear_if_unchanged(sequence);
        }
    }

    fn vault_rows(&self, tool: Tool) -> Vec<Row> {
        let vault = self.vaults.get(&tool);
        if !vault.is_some_and(|vault| vault.loaded) {
            let hint = match tool {
                Tool::OnePassword => "Approve it if 1Password asks",
                Tool::EnteAuth => "This takes a moment",
            };
            return vec![Row::info(format!("Asking {}…", tool.name()), hint, tool.glyph())];
        }
        let vault = vault.expect("checked above");
        if let Some(failure) = &vault.failure {
            let again = Row::op("Try again", failure.message(tool), GLYPH_REFRESH, Op::RefreshVault(tool));
            return match failure {
                Failure::NotInstalled => vec![Row::op(
                    format!("Set up {}", tool.name()),
                    "Its command-line tool isn't installed. Settings can install it.",
                    tool.glyph(),
                    Op::OpenExtensionsSettings,
                )],
                Failure::NeedsSignIn => vec![again, Row::op(format!("Open {} in Settings", tool.name()), "Under Extensions", GLYPH_KEY, Op::OpenExtensionsSettings)],
                _ => vec![again],
            };
        }
        if vault.items.is_empty() {
            return vec![Row::info(format!("Nothing in {}", tool.name()), "", tool.glyph())];
        }
        vault
            .items
            .iter()
            .map(|item| {
                // A password is copied, for whichever box it's wanted in; an Ente code is typed
                // where the launcher was opened, with Ctrl+Enter to copy it instead.
                let primary = match tool {
                    Tool::OnePassword => Op::CopySecret { tool, id: item.id.clone(), field: integrations::Field::Password },
                    Tool::EnteAuth => Op::PasteSecret { tool, id: item.id.clone(), field: integrations::Field::OneTimeCode },
                };
                Row {
                    title: item.title.clone(),
                    subtitle: item.subtitle.clone(),
                    kind: item.kind,
                    icon: RowIcon::Glyph(tool.glyph()),
                    accent: false,
                    keys: None,
                    accessory: None,
                    primary: Some(primary),
                    source: RowSource::Secret { tool, id: item.id.clone(), url: item.url.clone() },
                }
            })
            .collect()
    }

    /// The app the quicklink with this address opens with, if it names one.
    fn open_with_for(&self, url: &str) -> String {
        self.settings.links.iter().find(|link| link.url == url).map(|link| link.open_with.clone()).unwrap_or_default()
    }

    /// Opens a quicklink's address or path, in the app it names, or else the one Windows uses.
    fn open_link(&mut self, target: &str, open_with: &str) {
        let app = (!open_with.is_empty()).then(|| self.catalog.app_by_id(open_with).cloned()).flatten();
        if app.is_none() && !open_with.is_empty() {
            self.report("The app this quicklink opens with isn't installed any more, so it opens as usual.");
        }
        // An address the app would be handed gets the same checks as one Windows opens.
        let openable = orbit_core::links::is_path(target) || matches!(check_url(target), UrlCheck::Safe | UrlCheck::Unusual(_));
        if let Some(app) = app.filter(|_| openable) {
            let quoted = format!("\"{}\"", target.trim().trim_matches('"'));
            let request = if app.exe.starts_with("::") {
                // File Explorer, which the app list knows by a shell folder rather than a program.
                LaunchRequest { file: "explorer.exe".into(), parameters: quoted, working_dir: String::new(), run_as_admin: false }
            } else if app.exe.is_empty() || app.exe.contains("://") {
                // A packaged app has no program of its own to start. Windows passes the target on.
                LaunchRequest { parameters: quoted, ..LaunchRequest::app(&app.id, false) }
            } else {
                LaunchRequest { file: app.exe.clone(), parameters: quoted, working_dir: String::new(), run_as_admin: false }
            };
            launch::spawn(request, self.send, WM_ORBIT_LAUNCH_FAILED);
            return;
        }
        if orbit_core::links::is_path(target) {
            launch::spawn(LaunchRequest::open(target.trim().trim_matches('"')), self.send, WM_ORBIT_LAUNCH_FAILED);
            return;
        }
        self.open_url(target);
    }

    fn open_url(&mut self, url: &str) {
        match check_url(url) {
            UrlCheck::Safe => {
                launch::spawn(LaunchRequest::open(url), self.send, WM_ORBIT_LAUNCH_FAILED);
            }
            UrlCheck::Unusual(scheme) => {
                self.report(&format!("Opening a {scheme}: link. Windows chooses the app that handles it."));
                launch::spawn(LaunchRequest::open(url), self.send, WM_ORBIT_LAUNCH_FAILED);
            }
            UrlCheck::Blocked(scheme) => self.report(&format!("Orbit won't open {scheme}: links, because they can run code.")),
            UrlCheck::Invalid => self.report("That doesn't look like a valid address."),
        }
    }

    /// Moves the window that was in front when the launcher opened, or, from a hotkey, the one in
    /// front now. The launcher goes first, giving that window back its focus.
    /// Covers the screens to pick a colour. The launcher goes first, so it isn't in the picture.
    fn pick_colour(&mut self) {
        if self.visible {
            self.hide(false);
            // Waits for the screen to be drawn without it.
            unsafe {
                let _ = windows::Win32::Graphics::Dwm::DwmFlush();
                let _ = windows::Win32::Graphics::Dwm::DwmFlush();
            }
        }
        if !crate::colour_pick::start(self.main, WM_ORBIT_COLOUR) {
            self.report("Orbit couldn't see the screen to pick a colour from.");
        }
    }

    /// The row Enter runs in the timer: start what's typed, or pause, resume or restart.
    fn timer_row(&self, query: &str) -> Row {
        use orbit_core::timer::{describe, parse_duration};
        let timer = self.countdown.as_ref();
        let typed = parse_duration(query);
        let (title, op) = match (typed, timer) {
            (Some(seconds), _) => (format!("Start a timer for {}", describe(seconds)), Some(Op::TimerStart(seconds))),
            (None, Some(timer)) if timer.running() => ("Pause the timer".to_string(), Some(Op::TimerPause)),
            (None, Some(timer)) if timer.done => (format!("Start another for {}", describe(timer.length)), Some(Op::TimerStart(timer.length))),
            (None, Some(_)) => ("Resume the timer".to_string(), Some(Op::TimerResume)),
            (None, None) if query.trim().is_empty() => ("Type how long, such as 5m, 1h 30m or 1:30".to_string(), None),
            (None, None) => ("That isn\u{2019}t a time Orbit understands".to_string(), None),
        };
        let mut row = match op {
            Some(op) => Row::op(title, "", GLYPH_TIMER, op),
            None => Row::info(title, "", GLYPH_TIMER),
        };
        row.source = RowSource::Clock { stopwatch: false, running: timer.is_some_and(|timer| timer.running()), started: timer.is_some_and(|timer| !timer.done) };
        row
    }

    fn stopwatch_row(&self) -> Row {
        let watch = &self.stopwatch;
        let (title, op) = if watch.running() {
            ("Pause", Op::StopwatchPause)
        } else if watch.is_reset() {
            ("Start", Op::StopwatchStart)
        } else {
            ("Resume", Op::StopwatchStart)
        };
        let mut row = Row::op(title, "", GLYPH_STOPWATCH, op);
        row.source = RowSource::Clock { stopwatch: true, running: watch.running(), started: !watch.is_reset() };
        row
    }

    /// Sets the one Windows timer for when the countdown ends, or clears it.
    fn arm_countdown(&mut self) {
        unsafe {
            let _ = KillTimer(Some(self.main), TIMER_COUNTDOWN);
        }
        if let Some(timer) = self.countdown.as_ref().filter(|timer| timer.running()) {
            let wait = (timer.remaining().as_millis() as u64 + 20).clamp(10, 0x7fff_0000) as u32;
            unsafe {
                let _ = SetTimer(Some(self.main), TIMER_COUNTDOWN, wait, None);
            }
        }
    }

    /// Redraws on the next second while a running timer or stopwatch is on screen, and not otherwise.
    fn tick_clock(&mut self) {
        unsafe {
            let _ = KillTimer(Some(self.main), TIMER_CLOCK);
        }
        if !self.visible {
            return;
        }
        let scope = self.menu.as_ref().map_or(&self.view.scope, |menu| &menu.behind.scope);
        let wait = match scope {
            Scope::Timer => self.countdown.as_ref().filter(|timer| timer.running()).map(|timer| timer.remaining().subsec_millis()),
            Scope::Stopwatch => self.stopwatch.running().then(|| 1000 - self.stopwatch.elapsed().subsec_millis()),
            _ => None,
        };
        if let Some(wait) = wait {
            unsafe {
                let _ = SetTimer(Some(self.main), TIMER_CLOCK, wait.max(15) + 5, None);
            }
        }
    }

    /// After the timer or stopwatch changes: its row, its Windows timers and the picture.
    fn after_clock_change(&mut self, clear_query: bool) {
        self.arm_countdown();
        if clear_query {
            self.view.last_query = Some(String::new());
            self.set_query_text("");
        }
        self.refresh_results();
        self.redraw();
    }

    fn arrange(&mut self, layout: crate::window_layout::Layout) {
        let target = if self.visible { self.prev_foreground } else { unsafe { GetForegroundWindow() } };
        if self.visible {
            self.hide(true);
        }
        if let Err(why) = crate::window_layout::apply(target, layout) {
            self.report(&why);
        }
    }

    pub fn run_builtin(&mut self, command: Builtin) {
        if command.needs_confirmation() {
            if !self.visible {
                self.show(Scope::Root, "command", qpc());
            }
            self.execute(Op::AskConfirm(command));
            return;
        }
        self.perform_builtin(command);
    }

    fn perform_builtin(&mut self, command: Builtin) {
        let t0 = qpc();
        // A hotkey can outlast its extension being turned off.
        if let Some(ext) = orbit_core::builtin_ext::BuiltinExt::of_builtin(command).filter(|ext| !ext.is_on(&self.settings)) {
            self.report(&format!("{} is turned off. Turn it on in Settings, under Extensions.", ext.name()));
            return;
        }
        match command {
            Builtin::ToggleLauncher => self.toggle(Scope::Root, "command", t0),
            Builtin::NewNote => {
                self.hide(false);
                crate::editor::open_note(self.editor_context(), None, "");
            }
            Builtin::SearchNotes => self.enter_scope(Scope::Notes, t0),
            Builtin::ClipboardHistory => self.enter_scope(Scope::Clipboard, t0),
            Builtin::SnippetSearch => self.enter_scope(Scope::Snippets, t0),
            Builtin::SearchQuicklinks => self.enter_scope(Scope::Links, t0),
            Builtin::SearchFiles => self.enter_scope(Scope::Files, t0),
            Builtin::ManageExtensions => self.execute_ext(Op::OpenExtensionsSettings),
            Builtin::ImportData => self.open_import(),
            Builtin::EmojiPicker => self.enter_scope(Scope::Emoji, t0),
            Builtin::Weather => {
                self.enter_scope(Scope::Weather, t0);
                self.weather_look_up("");
            }
            Builtin::SpeedTest => {
                self.enter_scope(Scope::SpeedTest, t0);
                // Opening it starts a test, once Ookla's terms have been accepted.
                self.speedtest.check();
                if self.settings.speedtest_accepted && !self.speedtest.missing {
                    self.speedtest.start(self.send, WM_ORBIT_SPEEDTEST);
                }
                self.refresh_results();
                self.redraw();
            }
            Builtin::NewSnippet => {
                self.open_form(InlineForm::snippet(None));
            }
            Builtin::WindowLeft => self.arrange(crate::window_layout::Layout::Left),
            Builtin::WindowRight => self.arrange(crate::window_layout::Layout::Right),
            Builtin::WindowTop => self.arrange(crate::window_layout::Layout::Top),
            Builtin::WindowBottom => self.arrange(crate::window_layout::Layout::Bottom),
            Builtin::WindowMaximize => self.arrange(crate::window_layout::Layout::Maximize),
            Builtin::WindowAlmostMaximize => self.arrange(crate::window_layout::Layout::AlmostMaximize),
            Builtin::WindowCenter => self.arrange(crate::window_layout::Layout::Center),
            Builtin::WindowNextDisplay => self.arrange(crate::window_layout::Layout::NextDisplay),
            Builtin::ToggleDarkMode => {
                let outcome = crate::toggles::toggle_dark_mode();
                self.report(&outcome.unwrap_or_else(|why| why));
            }
            Builtin::ToggleMute => {
                let outcome = crate::toggles::toggle_mute();
                self.report(&outcome.unwrap_or_else(|why| why));
            }
            Builtin::ToggleBluetooth => crate::toggles::toggle_radio(crate::toggles::Radio::Bluetooth, self.send, WM_ORBIT_REPORT),
            Builtin::ToggleWifi => crate::toggles::toggle_radio(crate::toggles::Radio::WiFi, self.send, WM_ORBIT_REPORT),
            Builtin::Timer => self.enter_scope(Scope::Timer, t0),
            Builtin::Stopwatch => {
                // Opening a stopwatch that hasn't been used starts it.
                if self.stopwatch.is_reset() {
                    self.stopwatch.start();
                }
                self.enter_scope(Scope::Stopwatch, t0);
            }
            Builtin::PickColour => self.pick_colour(),
            Builtin::NewQuicklink => {
                self.open_link_form(None);
            }
            Builtin::PauseClipboard => {
                self.clip_paused = true;
                self.apply_clipboard_listener();
                self.hide(true);
                self.report("Clipboard history paused");
            }
            Builtin::ResumeClipboard => {
                self.clip_paused = false;
                self.apply_clipboard_listener();
                self.hide(true);
                self.report("Clipboard history resumed");
            }
            Builtin::SkipNextCopy => {
                self.skip_next_copy = true;
                self.hide(true);
                self.report("Your next copy won't be remembered");
            }
            Builtin::ClearClipboard => self.worker.send(Req::ClipClear),
            Builtin::ResetShortcuts => {
                let mut settings = self.settings.clone();
                settings.launcher.hotkey = None;
                settings.shortcuts.clear();
                settings.hyper.bindings.clear();
                self.apply_settings(settings);
                self.reopen_settings();
                self.report("Shortcuts are back to the defaults.");
            }
            Builtin::DeleteLocalData => self.delete_local_data(),
            Builtin::Settings => {
                self.hide(false);
                crate::settings_win::open(self.editor_context(), self.settings.clone(), self.dir.clone(), &self.catalog.apps);
                self.tell_settings(None);
            }
            Builtin::Diagnostics => self.enter_scope(Scope::Diagnostics, t0),
            Builtin::ShowHelp => self.enter_scope(Scope::Help, t0),
            Builtin::RebuildAppIndex => {
                self.hide(true);
                self.start_indexer(true);
                self.report("Rebuilding the app index…");
            }
            Builtin::SuspendShortcuts => self.set_suspended(true),
            Builtin::ResumeShortcuts => self.set_suspended(false),
            Builtin::ToggleLowResource => {
                let mut settings = self.settings.clone();
                settings.low_resource_mode = !settings.low_resource_mode;
                let on = settings.low_resource_mode;
                self.apply_settings(settings);
                self.reopen_settings();
                self.hide(true);
                self.report(if on { "Low Resource Mode is on" } else { "Low Resource Mode is off" });
            }
            Builtin::ExportData => {
                self.hide(true);
                self.worker.send(Req::Export { settings_json: self.settings.to_json() });
            }
            Builtin::RestartOrbit => self.restart(),
            Builtin::CheckForUpdates => {
                use crate::update::{Ask, Phase};
                // Installing closes Orbit, and the release page opens in the browser.
                if matches!(crate::update::phase(), Phase::Ready(_) | Phase::Available(_)) {
                    self.hide(false);
                }
                if let Some(line) = crate::update::press(Ask::FromLauncher) {
                    self.report(&line);
                }
            }
            Builtin::QuitOrbit => self.begin_exit(),
            Builtin::LockScreen => {
                self.hide(false);
                unsafe {
                    let _ = LockWorkStation();
                }
            }
            Builtin::Sleep => unsafe {
                let _ = SetSuspendState(false, false, false);
            },
            Builtin::Shutdown => {
                self.spawn_shutdown("/s");
            }
            Builtin::Restart => {
                self.spawn_shutdown("/r");
            }
            Builtin::EmptyTrash => unsafe {
                let _ = SHEmptyRecycleBinW(None, PCWSTR::null(), SHERB_NOSOUND);
            },
            Builtin::OpenDownloads => {
                self.hide(false);
                let path = std::env::var_os("USERPROFILE").map(PathBuf::from).unwrap_or_default().join("Downloads");
                launch::spawn(LaunchRequest::open(&path.to_string_lossy()), self.send, WM_ORBIT_LAUNCH_FAILED);
            }
            Builtin::OpenTerminal => {
                self.hide(false);
                launch::spawn(LaunchRequest::open("wt.exe"), self.send, WM_ORBIT_LAUNCH_FAILED);
            }
        }
    }

    fn spawn_shutdown(&mut self, flag: &str) {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        let _ = std::process::Command::new("shutdown.exe")
            .args([flag, "/t", "0"])
            .creation_flags(CREATE_NO_WINDOW)
            .spawn();
    }

    fn enter_scope(&mut self, scope: Scope, t0: i64) {
        self.leave_menu();
        if self.visible {
            self.push_view();
            self.status = None;
            self.set_scope(scope, false);
            self.redraw();
        } else {
            self.show(scope, "command", t0);
        }
    }

    fn set_suspended(&mut self, suspended: bool) {
        self.suspended = suspended;
        self.hide(true);
        if suspended {
            // The hook is removed entirely, not merely told to pass events along.
            self.hook = None;
            self.register_hotkeys();
            self.report("Orbit shortcuts are suspended. Resume from the tray icon.");
        } else {
            self.register_hotkeys();
            self.apply_hook();
            self.report("Orbit shortcuts are back on.");
        }
    }

    pub fn on_tray_choice(&mut self, choice: MenuChoice) {
        match choice {
            MenuChoice::Open => self.show(Scope::Root, "tray", qpc()),
            MenuChoice::Settings => self.perform_builtin(Builtin::Settings),
            MenuChoice::ToggleClipboardPause => {
                self.clip_paused = !self.clip_paused;
                self.apply_clipboard_listener();
            }
            MenuChoice::ToggleSuspend => self.set_suspended(!self.suspended),
            MenuChoice::Quit => self.begin_exit(),
            MenuChoice::Update => self.perform_builtin(Builtin::CheckForUpdates),
        }
    }

    // --- Snippets, clipboard and text insertion -------------------------------------------

    fn on_snippet(&mut self, snippet: orbit_data::Snippet, purpose: Purpose) {
        match purpose {
            Purpose::Edit => self.open_form(InlineForm::snippet(Some(snippet))),
            Purpose::Copy => {
                let expanded = self.expand(&snippet.content);
                if !clipboard::write_text(&expanded.text) {
                    self.report("Couldn't reach the clipboard");
                }
            }
            Purpose::Insert => {
                let expanded = self.expand(&snippet.content);
                if expanded.used_clipboard {
                    self.show_clipboard_preview(snippet.name, expanded, 0);
                } else {
                    self.start_insert(expanded.text, 0, expanded.cursor_back, false);
                }
            }
            Purpose::Expand { trigger_chars } => {
                let expanded = self.expand(&snippet.content);
                if expanded.used_clipboard {
                    // Never drop clipboard content into a window without the user seeing it.
                    self.show_clipboard_preview(snippet.name, expanded, trigger_chars);
                } else {
                    self.start_insert(expanded.text, trigger_chars, expanded.cursor_back, false);
                }
            }
            // Only clips are asked for these.
            Purpose::InsertPlain | Purpose::Open => {}
        }
    }

    fn show_clipboard_preview(&mut self, name: String, expanded: orbit_core::snippet::Expanded, erase: usize) {
        self.insert_target = unsafe { GetForegroundWindow() };
        let preview: String = expanded.text.chars().take(120).map(|c| if c.is_control() { ' ' } else { c }).collect();
        self.show(Scope::Root, "preview", qpc());
        self.prev_foreground = self.insert_target;
        self.push_view();
        self.view.scope = Scope::Actions { of: name };
        self.view.static_rows = vec![
            Row::op("Cancel", "", GLYPH_BACK, Op::Back),
            Row::info(preview, "This snippet includes your clipboard", GLYPH_WARNING),
        ];
        self.pending_expansion = Some((expanded, erase));
        self.view.static_rows.insert(1, Row::op("Insert with clipboard", "Press ↓ then Enter", GLYPH_CHECK, Op::InsertPending));
        self.view.reset_position();
        self.sync_vim_to_scope();
        self.set_placeholder();
        self.refresh_results();
    }

    fn expand(&self, template: &str) -> orbit_core::snippet::Expanded {
        let (date, time) = util::local_date_time();
        let needs_clipboard = orbit_core::snippet::uses_clipboard(template);
        let clipboard_text = if needs_clipboard { clipboard::read_text() } else { None };
        expand_template(template, &TemplateContext { date: &date, time: &time, clipboard: clipboard_text.as_deref() })
    }

    fn on_clip_content(&mut self, kind: ClipKind, text: String, blob: String, purpose: Purpose, formats: orbit_data::Formats) {
        if purpose == Purpose::Open {
            self.open_clip(kind, &text, &blob);
            return;
        }
        let insert = matches!(purpose, Purpose::Insert | Purpose::InsertPlain);
        match kind {
            ClipKind::Text | ClipKind::Url => {
                if insert {
                    self.start_insert_formatted(text, 0, None, true, formats);
                } else if !clipboard::write_formatted(&text, &formats) {
                    self.report("Couldn't reach the clipboard");
                }
            }
            ClipKind::Files => {
                let paths: Vec<&str> = text.lines().collect();
                if !clipboard::write_files(&paths) {
                    self.report("Couldn't reach the clipboard");
                } else if insert {
                    self.start_paste();
                }
            }
            ClipKind::Image => {
                let path = self.dir.join("clipboard-images").join(&blob);
                // The worker made a PNG beside it for pasting. A bitmap alone still pastes.
                let png = std::fs::read(path.with_extension("png")).ok();
                match std::fs::read(&path) {
                    Ok(bytes) if clipboard::write_dib(&bytes, png.as_deref()) => {
                        if insert {
                            self.start_paste();
                        }
                    }
                    _ => self.report("That image is no longer available."),
                }
            }
        }
    }

    /// A link from history opens in the browser, a picture in the app Windows uses for pictures,
    /// and a file in its own app. A program, script or shortcut is never run from here: its folder
    /// opens instead, so running it stays a choice made in Explorer.
    fn open_clip(&mut self, kind: ClipKind, text: &str, blob: &str) {
        match kind {
            ClipKind::Url => self.open_url(text.trim()),
            ClipKind::Image if !blob.is_empty() => {
                let path = self.dir.join("clipboard-images").join(blob);
                launch::spawn(LaunchRequest::open(&path.to_string_lossy()), self.send, WM_ORBIT_LAUNCH_FAILED);
            }
            ClipKind::Image => self.report("That picture is no longer available."),
            ClipKind::Files => {
                let paths: Vec<&str> = text.lines().filter(|line| !line.trim().is_empty()).collect();
                let Some(first) = paths.first() else { return };
                let path = std::path::Path::new(first);
                if !path.exists() {
                    self.report("That file is no longer there.");
                } else if paths.len() == 1 && !path.is_dir() && !crate::launch::is_runnable(path) {
                    launch::spawn(LaunchRequest::open(first), self.send, WM_ORBIT_LAUNCH_FAILED);
                } else if paths.len() == 1 && path.is_dir() {
                    launch::spawn(LaunchRequest::open(first), self.send, WM_ORBIT_LAUNCH_FAILED);
                } else if let Some(folder) = path.parent() {
                    launch::spawn(LaunchRequest::open(&folder.to_string_lossy()), self.send, WM_ORBIT_LAUNCH_FAILED);
                }
            }
            ClipKind::Text => {}
        }
    }

    /// Pastes what was just put on the clipboard into the window the launcher was opened over.
    fn start_paste(&mut self) {
        let target = SendHwnd::new(std::mem::take(&mut self.insert_target));
        let main = self.send;
        let _ = std::thread::Builder::new().name("orbit-paste".into()).stack_size(256 * 1024).spawn(move || {
            let code = if clipboard::paste_into(target.get()) { 0 } else { 6 };
            main.post(WM_ORBIT_INSERT_DONE, code, 0);
        });
    }

    fn start_insert(&mut self, text: String, erase_chars: usize, cursor_back: Option<usize>, keep_on_clipboard: bool) {
        self.start_insert_formatted(text, erase_chars, cursor_back, keep_on_clipboard, orbit_data::Formats::default());
    }

    fn start_insert_formatted(&mut self, text: String, erase_chars: usize, cursor_back: Option<usize>, keep_on_clipboard: bool, formats: orbit_data::Formats) {
        if text.is_empty() {
            return;
        }
        let target = SendHwnd::new(if self.insert_target.is_invalid() {
            unsafe { GetForegroundWindow() }
        } else {
            self.insert_target
        });
        self.insert_target = HWND::default();
        let main = self.send;
        let _ = std::thread::Builder::new().name("orbit-insert".into()).stack_size(1024 * 1024).spawn(move || {
            let request = InsertRequest { target: target.get(), text, erase_chars, cursor_back, keep_on_clipboard, formats };
            let code = match clipboard::insert(request) {
                InsertOutcome::Inserted => 0,
                InsertOutcome::Failed => 1,
                InsertOutcome::CopiedInstead => 2,
                InsertOutcome::TooLongToType => 3,
                InsertOutcome::PasswordField { reported: true } => 4,
                InsertOutcome::PasswordField { reported: false } => 5,
            };
            main.post(WM_ORBIT_INSERT_DONE, code, 0);
        });
    }

    fn on_expand(&mut self, snippet_id: i64, trigger_chars: usize) {
        if self.suspended {
            return;
        }
        self.insert_target = unsafe { GetForegroundWindow() };
        self.worker.send(Req::SnippetGet { id: snippet_id, purpose: Purpose::Expand { trigger_chars } });
    }

    pub fn apply_clipboard_listener(&mut self) {
        // A preview only draws, so it never keeps what's copied while it runs.
        let want = self.settings.clipboard.enabled && !self.clip_paused && !self.previewing;
        if want == self.clip_listening {
            return;
        }
        unsafe {
            if want {
                self.clip_listening = AddClipboardFormatListener(self.main).is_ok();
            } else {
                let _ = RemoveClipboardFormatListener(self.main);
                self.clip_listening = false;
            }
        }
    }

    fn on_clipboard_changed(&mut self) {
        // Orbit's own write mustn't use up the promise to skip the user's next copy.
        if clipboard::is_own_write() {
            return;
        }
        if self.skip_next_copy {
            self.skip_next_copy = false;
            return;
        }
        let tx = self.worker.tx.clone();
        let max_item_bytes = self.settings.clipboard.max_item_bytes;
        // Images are files, which "only in memory" rules out.
        let capture_images = self.settings.clipboard.capture_images
            && self.settings.clipboard.persist
            && !self.settings.low_resource_mode
            && !self.shed_active;
        let excluded = self.settings.clipboard.excluded_apps.clone();
        let _ = std::thread::Builder::new().name("orbit-clip".into()).stack_size(256 * 1024).spawn(move || {
            let limits = CaptureLimits { max_item_bytes, capture_images, excluded_apps: &excluded };
            if let Some(Capture { clip, image }) = clipboard::capture(&limits) {
                let _ = tx.send(Req::ClipAdd { clip, image });
            }
        });
    }

    // --- Settings, indexing, and everything else ------------------------------------------

    pub fn editor_context(&self) -> crate::editor::EditorContext {
        crate::editor::EditorContext {
            tx: self.worker.tx.clone(),
            main: self.send,
            theme: self.theme,
        }
    }

    /// Settings changed in a way the open Settings window can't just take in.
    fn reopen_settings(&self) {
        if crate::settings_win::is_open() {
            crate::settings_win::reopen(self.editor_context(), self.settings.clone(), self.dir.clone(), &self.catalog.apps);
            self.tell_settings(None);
        }
    }

    /// Tells an open Settings window what's true now, and whether what it sent reached the disk.
    fn tell_settings(&self, saved: Option<bool>) {
        crate::settings_win::applied(saved, &self.hotkey_problems, &self.active_hotkey, &self.settings.hidden);
    }

    pub fn save_settings(&mut self) {
        // A settings file Orbit couldn't read is left alone, so nothing reaches the disk.
        self.save_failed = self.settings_read_only;
        if self.settings_read_only {
            return;
        }
        let json = self.settings.to_json();
        if util::write_atomic(&self.dir.join("settings.json"), json.as_bytes()).is_err() {
            self.save_failed = true;
            self.report("Couldn't save your settings.");
        }
    }

    fn save_and_rebuild_catalog(&mut self) {
        self.save_settings();
        self.catalog.rebuild(&self.settings);
        self.tell_settings(None);
    }

    pub fn apply_settings(&mut self, new: Settings) {
        if self.exiting {
            return; // it would start the hook and the shortcuts up again
        }
        let old = std::mem::replace(&mut self.settings, new);
        // The place is the launcher's to keep: Settings may hold an older copy of it.
        self.settings.weather_place = if self.settings.fetch_weather { old.weather_place.clone() } else { None };
        if old.fetch_weather && !self.settings.fetch_weather {
            self.weather.forget();
        }
        self.save_settings();
        self.catalog.rebuild(&self.settings);
        self.register_hotkeys();
        self.announce_hotkey_problems();
        self.stop_disabled_extensions();
        // Mappings alone don't replace the hook, because a key may be held right now.
        let without_bindings = |hyper: &orbit_core::settings::HyperSettings| orbit_core::settings::HyperSettings { bindings: Vec::new(), ..hyper.clone() };
        let hook_inputs_changed = without_bindings(&old.hyper) != without_bindings(&self.settings.hyper)
            || old.snippets.expansion_enabled != self.settings.snippets.expansion_enabled
            || old.snippets.excluded_apps != self.settings.snippets.excluded_apps
            || old.mac_feel != self.settings.mac_feel;
        if hook_inputs_changed {
            self.apply_hook();
        } else if old.hyper.bindings != self.settings.hyper.bindings {
            if let Some(hook) = &self.hook {
                hook.set_bound_keys(hook::bound_keys(&self.settings));
            }
        }
        self.apply_clipboard_listener();
        if old.clipboard.persist != self.settings.clipboard.persist {
            self.worker.send(Req::SetClipPersist(self.settings.clipboard.persist));
        }
        if old.clipboard.max_items != self.settings.clipboard.max_items
            || old.clipboard.retention_days != self.settings.clipboard.retention_days
        {
            self.worker.send(Req::SetClipPolicy(RetentionPolicy {
                max_items: self.settings.clipboard.max_items,
                retention_days: self.settings.clipboard.retention_days,
            }));
        }
        if old.launch_at_login != self.settings.launch_at_login {
            set_launch_at_login(self.settings.launch_at_login);
        }
        if old.fetch_link_icons && !self.settings.fetch_link_icons {
            crate::favicon::forget(&self.dir);
            self.link_icons_asked = None;
            self.link_icons_at = None;
            self.icons.clear();
            crate::settings_win::link_icons_changed(false);
        }
        if old.fetch_exchange_rates && !self.settings.fetch_exchange_rates {
            self.exchange_rates.forget(&self.dir);
        }
        if self.settings.fetch_link_icons && (!old.fetch_link_icons || old.links != self.settings.links) {
            self.sync_link_icons();
        }
        if old.low_resource_mode != self.settings.low_resource_mode {
            self.apply_cache_limits();
            if self.settings.low_resource_mode {
                self.worker.send(Req::Shed);
                self.trim_working_set();
            }
        }
        crate::rows::set_icon_style(self.settings.icon_style);
        self.refresh_theme();
    }

    /// Automatic reindexing is optional work. Low Resource Mode, tight memory and running on
    /// battery all put it off. Asking for it with "Rebuild App Index" still works.
    fn import_settings(&mut self, path: &std::path::Path) {
        const MAX_SETTINGS_BYTES: u64 = 4 * 1024 * 1024;
        if std::fs::metadata(path).map_or(true, |meta| meta.len() > MAX_SETTINGS_BYTES) {
            self.orbit_file_report("That settings file is too large.", false);
            return;
        }
        let Ok(text) = std::fs::read_to_string(path) else {
            self.orbit_file_report("Couldn't read that settings file.", false);
            return;
        };
        let (settings, outcome) = Settings::parse(&text);
        if !matches!(outcome, orbit_core::settings::LoadOutcome::Loaded) {
            self.orbit_file_report("That file isn't a settings file Orbit can read, so nothing changed.", false);
            return;
        }
        self.apply_settings(settings);
        self.reopen_settings();
        self.orbit_file_report("Settings imported. Orbit now has the settings from that file.", true);
    }

    /// Removes what Orbit stored, by name, then quits. It never empties the folder as a whole, since
    /// `--data-dir` can point anywhere.
    fn delete_local_data(&mut self) {
        set_launch_at_login(false);
        self.settings_read_only = true;
        crate::editor::close_all();
        self.worker.send(Req::DeleteLocalData);
        self.begin_exit();
    }

    pub fn start_indexer(&mut self, refresh_icons: bool) {
        if self.index_running {
            self.index_dirty = true;
            return;
        }
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        let Ok(exe) = std::env::current_exe() else { return };
        let mut command = std::process::Command::new(exe);
        command.arg("--index-apps").arg("--data-dir").arg(&self.dir).creation_flags(CREATE_NO_WINDOW);
        if refresh_icons {
            command.arg("--refresh-icons");
        }
        let Ok(mut child) = command.spawn() else {
            self.report("Couldn't start the app indexer.");
            return;
        };
        self.index_running = true;
        let main = self.send;
        let _ = std::thread::Builder::new().name("orbit-index-wait".into()).stack_size(64 * 1024).spawn(move || {
            let ok = child.wait().map(|status| status.success()).unwrap_or(false);
            main.post(WM_ORBIT_INDEX_DONE, ok as usize, 0);
        });
    }

    fn on_index_done(&mut self, ok: bool) {
        self.index_running = false;
        if ok {
            if let Some(apps) = indexer::load_index(&self.dir) {
                if apps != self.catalog.apps {
                    self.catalog.set_apps(apps, &self.settings);
                    self.worker.send(Req::LoadUsage);
                }
                if self.visible {
                    self.refresh_results();
                }
            }
        } else if self.catalog.apps.is_empty() {
            self.report("Couldn't list your apps. Try Rebuild App Index from the launcher.");
        }
        if self.index_dirty {
            self.index_dirty = false;
            self.start_indexer(false);
        }
    }

    /// Shows a message in the launcher footer, or as a notification when the launcher is closed.
    pub fn report(&mut self, message: &str) {
        if self.visible {
            self.status = Some(message.to_string());
            self.redraw();
        } else if !crate::settings_win::show_message(message) {
            self.notify("Orbit", message);
        }
    }

    pub fn notify(&self, title: &str, message: &str) {
        tray::notify(self.main, WM_ORBIT_TRAY, title, message);
    }

    fn diagnostics_rows(&self) -> Vec<Row> {
        let memory = util::process_memory();
        let (show_p50, show_p95) = self.metrics.show_summary();
        let (query_p50, query_p95) = self.metrics.query_summary();
        let hook_state = if self.hook.is_some() && hook::HOOK_INSTALLED.load(Ordering::Relaxed) {
            let max_us = hook::HOOK_MAX_TICKS.load(Ordering::Relaxed) as f64 * 1e6 / qpc_frequency() as f64;
            format!("Active. Slowest callback {max_us:.0} µs, {} reinstalls", hook::HOOK_REINSTALLS.load(Ordering::Relaxed))
        } else if self.hook_failed {
            "Refused by Windows".to_string()
        } else {
            "Not running: no feature needs it".to_string()
        };
        let clipboard_state = match (self.settings.clipboard.enabled, self.clip_paused) {
            (false, _) => "Off",
            (true, true) => "Paused",
            (true, false) => "Recording",
        };
        let mut rows = vec![
            Row::info("Private memory", format!("{} (committed by Orbit, Task Manager \"Memory\")", format_bytes(memory.private_bytes)), GLYPH_INFO),
            Row::info("Working set", format!("{} now, {} peak (physical RAM in use, includes shared DLLs)", format_bytes(memory.working_set), format_bytes(memory.peak_working_set)), GLYPH_INFO),
            Row::info("Hotkey to focused input", if self.metrics.shows_total == 0 { "No samples yet".to_string() } else { format!("p50 {show_p50:.1} ms, p95 {show_p95:.1} ms (last {} opens)", self.metrics.shows_total.min(64)) }, GLYPH_INFO),
            Row::info("Keystroke to results drawn", if self.metrics.queries_total == 0 { "No samples yet".to_string() } else { format!("p50 {query_p50:.2} ms, p95 {query_p95:.2} ms") }, GLYPH_INFO),
            Row::info("Search index", format!("{} items, {}", self.catalog.index.len(), format_bytes(self.catalog.index.heap_bytes())), GLYPH_INFO),
            Row::info("Icon cache", format!("{} icons, {}", self.icons.len(), format_bytes(self.icons.bytes())), GLYPH_INFO),
            Row::info("Keyboard hook", hook_state, GLYPH_INFO),
            Row::info("Launcher shortcut", self.active_hotkey.clone(), GLYPH_INFO),
            Row::info("Clipboard history", clipboard_state, GLYPH_INFO),
            Row::info(
                "App index",
                if self.index_running {
                    "Updating now".to_string()
                } else {
                    match std::fs::metadata(self.dir.join(indexer::INDEX_FILE)).and_then(|meta| meta.modified()).ok().and_then(|t| t.elapsed().ok()) {
                        Some(age) if age.as_secs() < 3600 => format!("Idle, updated {} min ago", age.as_secs() / 60),
                        Some(age) => format!("Idle, updated {} h ago", age.as_secs() / 3600),
                        None => "Idle, not built yet".to_string(),
                    }
                },
                GLYPH_INFO,
            ),
            Row::info("Low Resource Mode", if self.settings.low_resource_mode { "On" } else if self.shed_active { "Off, caches shed: system memory is tight" } else { "Off" }, GLYPH_INFO),
        ];
        if let Some((notes, snippets, clips, sqlite)) = self.last_counts {
            rows.push(Row::info("Stored", format!("{notes} notes, {snippets} snippets, {clips} clips. SQLite is holding {}", format_bytes(sqlite as usize)), GLYPH_INFO));
        }
        for problem in &self.hotkey_problems {
            rows.push(Row::info("Shortcut problem", problem.clone(), GLYPH_WARNING));
        }
        rows
    }

    fn restart(&mut self) {
        if let Ok(exe) = std::env::current_exe() {
            let _ = std::process::Command::new(exe)
                .arg("--wait-pid")
                .arg(std::process::id().to_string())
                .arg("--data-dir")
                .arg(&self.dir)
                .spawn();
        }
        self.begin_exit();
    }

    /// Clean exit. Editors flush first, then the data thread drains and checkpoints, and the
    /// keyboard is returned to normal. A timer guarantees exit if anything stalls.
    pub fn begin_exit(&mut self) {
        if self.exiting {
            return;
        }
        self.exiting = true;
        self.speedtest.stop();
        self.runners.stop_all();
        self.hide(false);
        self.clear_secret_now();
        crate::editor::close_all();
        self.hook = None;
        hook::release_synthetic_modifiers();
        unsafe {
            for id in self.registered_hotkeys.drain(..) {
                let _ = UnregisterHotKey(Some(self.main), id);
            }
            if self.clip_listening {
                let _ = RemoveClipboardFormatListener(self.main);
                self.clip_listening = false;
            }
            let _ = SetTimer(Some(self.main), TIMER_EXIT, 1500, None);
        }
        tray::remove(self.main, WM_ORBIT_TRAY);
        self.metrics.flush();
        self.worker.send(Req::Shutdown);
    }
}

/// Ctrl with `vk`, written as menus write it, such as `Ctrl+Shift+E`. Only keys a menu shows.
/// The key a button in the footer stands for, and whether Ctrl goes with it.
fn footer_key(name: &str) -> Option<(u16, bool)> {
    Some(match name {
        "Enter" => (VK_RETURN.0, false),
        "Ctrl+Enter" => (VK_RETURN.0, true),
        "Ctrl+K" => (0x4B, true),
        "Tab" => (VK_TAB.0, false),
        "Esc" => (VK_ESCAPE.0, false),
        "Space" => (0x20, false),
        _ => return None,
    })
}

fn shortcut_label(vk: u16, shift: bool) -> Option<String> {
    let key = match vk {
        0x0D => "Enter".to_string(),
        0x26 => "Up".to_string(),
        0x28 => "Down".to_string(),
        0x2E => "Del".to_string(),
        0x41..=0x5A => char::from(vk as u8).to_string(),
        _ => return None,
    };
    Some(if shift { format!("Ctrl+Shift+{key}") } else { format!("Ctrl+{key}") })
}

fn data_row(row: DataRow, now: i64) -> Row {
    match row {
        DataRow::Note(note) => {
            let subtitle = if note.preview.is_empty() { time_ago(note.updated_at, now) } else { note.preview };
            Row {
                title: note.title,
                subtitle,
                kind: if note.pinned { "Pinned note" } else { "Note" },
                icon: RowIcon::Glyph(GLYPH_NOTE),
                accent: note.pinned,
                keys: None,
                accessory: None,
                primary: Some(Op::NoteOpen(note.id)),
                source: RowSource::Note { id: note.id, pinned: note.pinned },
            }
        }
        DataRow::Snippet { id, name, trigger, preview } => {
            let subtitle = if trigger.is_empty() { preview } else { format!("{trigger}   {preview}") };
            Row {
                title: name,
                subtitle,
                kind: "Snippet",
                icon: RowIcon::Glyph(GLYPH_SNIPPET),
                accent: false,
                keys: None,
                accessory: None,
                primary: Some(Op::SnippetInsert(id)),
                source: RowSource::Snippet { id },
            }
        }
        DataRow::Clip(clip) => {
            let (glyph, kind) = match clip.kind {
                ClipKind::Text => (GLYPH_CLIPBOARD, "Text"),
                ClipKind::Url => (GLYPH_LINK, "Link"),
                ClipKind::Files => (GLYPH_FOLDER, "Files"),
                ClipKind::Image => (GLYPH_IMAGE, "Image"),
            };
            let when = time_ago(clip.created_at, now);
            let subtitle = if clip.source_app.is_empty() { when } else { format!("{}, {when}", clip.source_app) };
            Row {
                title: clip.preview,
                subtitle,
                kind,
                icon: if clip.kind == ClipKind::Image {
                    RowIcon::Image(clip.id)
                } else {
                    RowIcon::Glyph(if clip.pinned { GLYPH_PIN } else { glyph })
                },
                accent: clip.pinned,
                keys: None,
                accessory: None,
                primary: Some(Op::ClipPaste(clip.id)),
                source: RowSource::Clip { id: clip.id, kind: clip.kind, pinned: clip.pinned, formatted: clip.formatted },
            }
        }
    }
}

/// Returns freed heap memory to Windows. Used after an unusual peak such as a very large note,
/// not on any routine path, because the heap would otherwise keep it for reuse.
fn release_free_heap() {
    use windows::Win32::System::Memory::{GetProcessHeap, HeapOptimizeResources, HeapSetInformation};
    #[repr(C)]
    struct OptimizeInfo {
        version: u32,
        flags: u32,
    }
    let info = OptimizeInfo { version: 1, flags: 0 };
    unsafe {
        if let Ok(heap) = GetProcessHeap() {
            let _ = HeapSetInformation(
                Some(heap),
                HeapOptimizeResources,
                Some(&info as *const _ as *const _),
                std::mem::size_of_val(&info),
            );
        }
    }
}

pub(crate) fn now_unix() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

/// Whether Windows starts this copy of Orbit at sign-in. The installer can set it too, so the
/// switch in Settings reads it from Windows rather than from settings.json. An entry for a copy
/// somewhere else, such as one that has since moved, counts as off.
pub fn launch_at_login_registered() -> bool {
    use windows::Win32::System::Registry::{HKEY_CURRENT_USER, RRF_RT_REG_SZ, RegGetValueW};
    let Ok(exe) = std::env::current_exe() else { return false };
    let mut buffer = vec![0u16; 1024];
    let mut size = (buffer.len() * 2) as u32;
    let status = unsafe {
        RegGetValueW(
            HKEY_CURRENT_USER,
            w!("Software\\Microsoft\\Windows\\CurrentVersion\\Run"),
            w!("Orbit"),
            RRF_RT_REG_SZ,
            None,
            Some(buffer.as_mut_ptr() as *mut _),
            Some(&mut size),
        )
    };
    if status.is_err() {
        return false;
    }
    let len = buffer.iter().position(|&unit| unit == 0).unwrap_or(0);
    let command = String::from_utf16_lossy(&buffer[..len]);
    command.trim().trim_matches('"').eq_ignore_ascii_case(&exe.display().to_string())
}

pub fn set_launch_at_login(enabled: bool) {
    use windows::Win32::System::Registry::*;
    let value_name = w!("Orbit");
    unsafe {
        let mut key = HKEY::default();
        let status = RegOpenKeyExW(
            HKEY_CURRENT_USER,
            w!("Software\\Microsoft\\Windows\\CurrentVersion\\Run"),
            None,
            KEY_SET_VALUE,
            &mut key,
        );
        if status.is_err() {
            return;
        }
        if enabled {
            if let Ok(exe) = std::env::current_exe() {
                let text = wide(&format!("\"{}\"", exe.display()));
                let bytes = std::slice::from_raw_parts(text.as_ptr() as *const u8, text.len() * 2);
                let _ = RegSetValueExW(key, value_name, None, REG_SZ, Some(bytes));
            }
        } else {
            let _ = RegDeleteValueW(key, value_name);
        }
        let _ = RegCloseKey(key);
    }
}

/// Avoids an unused import warning while keeping the Mods flags available to registration code.
#[allow(dead_code)]
const _: fn() = || {
    let _ = (MOD_ALT, MOD_CONTROL, MOD_NOREPEAT, MOD_SHIFT, MOD_WIN, GetDpiForWindow, BOOL(0), app_key);
};

// --- Quicklink and command forms ---------------------------------------------------------

impl App {
    /// Opens the form to add a quicklink, or to change this one.
    pub fn open_link_form(&mut self, existing: Option<orbit_core::settings::QuickLink>) {
        let blank = existing.is_none();
        let mut form = InlineForm::link(existing);
        let app = form.fields[crate::inline_form::LINK_OPEN_WITH].value.clone();
        if !app.is_empty() {
            let name = self.catalog.app_by_id(&app).map(|app| app.name.clone()).unwrap_or_default();
            // An app that's gone is shown as the usual app, which is what it will open with.
            form.set_open_with(if name.is_empty() { "" } else { &app }, &name);
            self.request_app_icon(&app);
        }
        self.open_form(form);
        // A new quicklink is offered what was in front: the page, folder or file it showed.
        if blank && !self.prev_foreground.is_invalid() {
            crate::foreground::suggest_async(self.prev_foreground, self.send, WM_ORBIT_LINK_SUGGESTION);
        }
    }

    /// Loads an app's icon at the size the form and its picker show it.
    fn request_app_icon(&mut self, id: &str) {
        let hash = icon_hash(id);
        let size = ui::scale(ui::POPOVER_ICON_DIP, self.dpi) as usize;
        if self.icons.touch(hash, size) {
            self.worker.send(Req::LoadIcon { hash, size });
        }
    }

    /// Fills in a new quicklink from the window that was in front, if nothing has been typed yet.
    fn on_link_suggestion(&mut self, found: crate::foreground::Suggestion) {
        use crate::inline_form::{FormKind, LINK_OPEN_WITH, LINK_TARGET};
        self.store_form_value();
        let app = self
            .catalog
            .apps
            .iter()
            .find(|app| (!found.program.is_empty() && app.exe == found.program) || (!found.aumid.is_empty() && app.aumid.eq_ignore_ascii_case(&found.aumid)))
            .map(|app| (app.id.clone(), app.name.clone()));
        let Some(form) = self.form.as_mut() else { return };
        let FormKind::Link { id } = &form.kind else { return };
        if !id.is_empty() || form.saving || !form.fields[0].value.is_empty() || !form.fields[LINK_TARGET].value.is_empty() {
            return;
        }
        if found.target.is_empty() && app.is_none() {
            return;
        }
        form.fields[0].value = found.name.clone();
        form.fields[LINK_TARGET].value = found.target.clone();
        let mut icon = None;
        if let Some((id, name)) = app.filter(|_| form.fields[LINK_OPEN_WITH].value.is_empty()) {
            form.set_open_with(&id, &name);
            icon = Some(id);
        }
        let at = form.at;
        if let Some(id) = icon {
            self.request_app_icon(&id);
        }
        self.load_form_field(at);
        let from = if found.from.is_empty() { "the window you were in".to_string() } else { found.from.clone() };
        self.status = Some(if found.target.is_empty() { format!("Set to open with {from}") } else { format!("Filled in from {from}") });
        self.restart_caret();
        self.redraw();
    }

    /// Opens the form to add a command, or to change this one.
    pub fn open_command_form(&mut self, existing: Option<orbit_core::settings::CustomCommand>) {
        crate::link_form::open_command(self.editor_context(), existing, self.settings.links.clone(), self.settings.custom_commands.clone());
    }

    pub fn delete_link(&mut self, id: &str) {
        self.settings.links.retain(|link| link.id != id);
        self.links_saved();
    }

    pub fn delete_command(&mut self, id: &str) {
        self.settings.custom_commands.retain(|command| command.id != id);
        self.links_saved();
    }

    /// A form finished: replaces the item with the same id, or adds it at the end.
    fn on_link_saved(&mut self, saved: crate::link_form::LinkSaved) {
        match saved {
            crate::link_form::LinkSaved::Link(link) => {
                // Saving a link asks its site again, even if it gave an icon before, so a site
                // that changed its icon or couldn't be reached can be tried by hand.
                if self.settings.fetch_link_icons && !self.previewing {
                    crate::favicon::fetch(self.dir.clone(), std::slice::from_ref(&link.url), true, self.send, WM_ORBIT_LINK_ICONS);
                }
                match self.settings.links.iter_mut().find(|other| other.id == link.id) {
                    Some(other) => *other = link,
                    None => self.settings.links.push(link),
                }
            }
            crate::link_form::LinkSaved::Command(command) => match self.settings.custom_commands.iter_mut().find(|other| other.id == command.id) {
                Some(other) => *other = command,
                None => self.settings.custom_commands.push(command),
            },
        }
        self.links_saved();
    }

    fn on_link_request(&mut self, request: crate::link_form::LinkRequest) {
        use crate::link_form::LinkRequest;
        match request {
            LinkRequest::NewLink => self.open_link_form(None),
            LinkRequest::EditLink(id) => {
                let existing = self.settings.links.iter().find(|link| link.id == id).cloned();
                if existing.is_some() {
                    self.open_link_form(existing);
                }
            }
            LinkRequest::DeleteLink(id) => self.delete_link(&id),
            LinkRequest::NewCommand => self.open_command_form(None),
            LinkRequest::NewSnippet => self.open_form(InlineForm::snippet(None)),
            LinkRequest::EditCommand(id) => {
                let existing = self.settings.custom_commands.iter().find(|command| command.id == id).cloned();
                if existing.is_some() {
                    self.open_command_form(existing);
                }
            }
            LinkRequest::DeleteCommand(id) => self.delete_command(&id),
        }
    }

    /// Shows `name`'s weather, or with nothing typed the place from last time. Previews never go
    /// online: they read a saved answer from `weather-preview.json` in the data folder, if there is one.
    fn weather_look_up(&mut self, name: &str) {
        if !self.settings.fetch_weather {
            self.report("Weather is turned off in Settings.");
            return;
        }
        if self.previewing {
            let json = std::fs::read_to_string(self.dir.join("weather-preview.json")).unwrap_or_default();
            self.weather.place = Some(orbit_core::weather::Place { name: "London".into(), region: "England, United Kingdom".into(), latitude: 51.51, longitude: -0.13 });
            self.weather.forecast = orbit_core::weather::parse_forecast(&json, false);
            return;
        }
        let fahrenheit = crate::weather::uses_fahrenheit();
        let known = self.settings.weather_place.clone();
        self.weather.look_up(name, known.as_ref(), fahrenheit, self.send, WM_ORBIT_WEATHER);
        self.redraw();
    }

    /// "123 euro in pounds": the answer at the bank's last rate, or why there isn't one yet. Previews
    /// never go online.
    fn currency_row(&mut self, conversion: &orbit_core::currency::Query) -> Row {
        use crate::rates::Status;
        use orbit_core::currency::format_amount;
        let fetch = !self.previewing;
        let (dir, send) = (self.dir.clone(), self.send);
        let asked = format!("{} {} in {}", orbit_core::calc::format_number(conversion.amount), conversion.from, conversion.to);
        let mut row = match self.exchange_rates.get(&dir, fetch, send, WM_ORBIT_RATES) {
            Status::Ready(rates) => match rates.convert(conversion) {
                Some(value) => {
                    let shown = format_amount(value, conversion.to);
                    let plain = shown.replace(',', "");
                    let mut row = Row::op(format!("{shown} {}", conversion.to), format!("{asked}, at the rate of {}", rates.day()), GLYPH_CURRENCY, Op::Copy(String::new()));
                    let formatted = orbit_core::currency::with_symbol(value, conversion.to);
                    let given = orbit_core::currency::with_symbol(conversion.amount, conversion.from);
                    let reverse = Some(format!("{plain} {} in {}", conversion.to, conversion.from));
                    let card = AnswerCard {
                        question: given.clone(),
                        question_label: orbit_core::currency::name(conversion.from).to_string(),
                        answer: formatted.clone(),
                        answer_label: orbit_core::currency::name(conversion.to).to_string(),
                        note: format!("At the European Central Bank\u{2019}s rate of {}", rates.day()),
                    };
                    self.answer_copies(&mut row, plain, formatted, reverse, card);
                    row.accent = true;
                    row
                }
                None => Row::info("No rate for that currency yet", asked, GLYPH_CURRENCY),
            },
            Status::Waiting => Row::info("Getting exchange rates\u{2026}", format!("{asked}, from the European Central Bank"), GLYPH_CURRENCY),
            Status::Unavailable => Row::info("Exchange rates aren't available", "Orbit couldn't reach the European Central Bank. It tries again in a few minutes.", GLYPH_CURRENCY),
        };
        row.kind = "Currency";
        row
    }

    /// How a quicklink's row looks: its site's icon, when Orbit may ask for one.
    fn link_icon(&self, url: &str) -> RowIcon {
        match crate::favicon::link_key(url) {
            Some(key) if self.settings.fetch_link_icons => RowIcon::Favicon(key),
            _ => RowIcon::Glyph(GLYPH_LINK),
        }
    }

    /// Asks for the icons of links not asked about since Orbit started: at start every link, and
    /// after that links just added or changed. Sites whose icons are kept aren't asked.
    fn sync_link_icons(&mut self) {
        if !self.settings.fetch_link_icons {
            return;
        }
        if self.link_icons_asked.is_none() {
            self.ask_every_link_icon();
            return;
        }
        let asked = self.link_icons_asked.get_or_insert_with(HashSet::new);
        let fresh: Vec<String> = self.settings.links.iter().map(|link| link.url.clone()).filter(|url| asked.insert(url.clone())).collect();
        if !fresh.is_empty() {
            crate::favicon::fetch(self.dir.clone(), &fresh, false, self.send, WM_ORBIT_LINK_ICONS);
        }
    }

    /// Asks for the icon of every link that has none, as at start and when the launcher opens a
    /// while later: a site that couldn't be reached, or had nothing, may have one now.
    fn ask_every_link_icon(&mut self) {
        if !self.settings.fetch_link_icons {
            return;
        }
        let urls: Vec<String> = self.settings.links.iter().map(|link| link.url.clone()).collect();
        self.link_icons_asked = Some(urls.iter().cloned().collect());
        self.link_icons_at = Some(Instant::now());
        crate::favicon::fetch(self.dir.clone(), &urls, false, self.send, WM_ORBIT_LINK_ICONS);
    }

    fn links_saved(&mut self) {
        self.save_and_rebuild_catalog();
        self.sync_link_icons();
        crate::settings_win::links_changed(&self.settings);
        crate::link_form::lists_changed(&self.settings);
    }
}

#[cfg(test)]
mod tests {
    use super::{footer_key, plate_inset, shortcut_label};

    /// Every key the footer shows is a button that presses it, so a click saves a form, picks its
    /// app or opens the menu just as the key would.
    #[test]
    fn every_key_in_the_footer_can_be_clicked() {
        // A Windows checkout may have CRLF line ends.
        let source = include_str!("view.rs").replace("\r\n", "\n");
        let body = &source[source.find("pub fn footer_hints").unwrap()..];
        let body = &body[..body.find("
    }
").unwrap()];
        // Each hint is pushed as (keys, what they do): the keys are the quoted words before the last comma.
        let keys: Vec<&str> = body
            .split("out.push((")
            .skip(1)
            .filter_map(|push| Some(push[..push.find("));")?].rsplit_once(", ")?.0))
            .flat_map(|keys| keys.split('"').skip(1).step_by(2))
            .collect();
        assert!(keys.contains(&"Space") && keys.contains(&"Ctrl+Enter"), "{keys:?}");
        for key in keys {
            assert!(footer_key(key).is_some() || key == "i", "{key} is shown but does nothing when clicked");
        }
        assert_eq!(footer_key("Ctrl+Enter"), Some((0x0D, true)));
        assert_eq!(footer_key("Space"), Some((0x20, false)));
    }

    /// Keys are read back in the words the menus show them in.
    #[test]
    fn pressed_keys_are_named_as_menus_name_them() {
        assert_eq!(shortcut_label(0x0D, true).as_deref(), Some("Ctrl+Shift+Enter"));
        assert_eq!(shortcut_label(0x45, true).as_deref(), Some("Ctrl+Shift+E"));
        assert_eq!(shortcut_label(0x26, true).as_deref(), Some("Ctrl+Shift+Up"));
        assert_eq!(shortcut_label(0x2E, true).as_deref(), Some("Ctrl+Shift+Del"));
        assert_eq!(shortcut_label(0x4F, false).as_deref(), Some("Ctrl+O"));
        assert_eq!(shortcut_label(0x70, false), None, "F1 is in no menu");
    }

    /// The blur's corner, at its inset, must stay inside the launcher's corner.
    #[test]
    fn the_blur_hides_inside_the_launchers_corners() {
        for (launcher, blur) in [(22, 8), (28, 10), (33, 12), (44, 16)] {
            let inset = plate_inset(launcher, blur) as f32;
            let (big, small) = (launcher as f32, blur as f32);
            // Centres of the two corner curves, measured from the launcher's corner.
            let apart = ((big - (inset + small)) * std::f32::consts::SQRT_2).abs();
            assert!(apart + small <= big, "{launcher}/{blur}: inset {inset}");
            assert!(inset <= (big - small) * 0.3 + 2.0, "and no further in than it needs to be");
        }
    }
}

#[cfg(test)]
mod frost_tests {
    /// The compositor's blur can be made on this PC. The window is never shown.
    #[test]
    #[ignore = "makes a hidden window on this PC"]
    fn the_shaped_blur_can_be_made() {
        let _ = unsafe { windows::Win32::System::Com::CoInitializeEx(None, windows::Win32::System::Com::COINIT_APARTMENTTHREADED) };
        let (plate, mut frost) = super::create_frosted_plate().expect("the compositor made no blur");
        frost.fit(700, 480, 22);
        unsafe {
            let _ = windows::Win32::UI::WindowsAndMessaging::DestroyWindow(plate);
        }
    }
}
