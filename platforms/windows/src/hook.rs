//! The keyboard hook thread. Exists only while HyperKey or text expansion is on.
//!
//! The `WH_KEYBOARD_LL` callback runs on this dedicated thread. Windows drops a hook that
//! responds too slowly, and a slow hook lags every keypress on the system, so the callback only
//! updates small in-memory state machines and posts recognised events to the launcher thread.
//! It never touches a database or runs a command. The only typed text it keeps is the short
//! in-memory buffer trigger matching needs, cleared on any sign the caret may have moved.
//!
//! A key-up can go missing: an elevated window or a UAC prompt takes the keyboard mid-hold, a
//! session disconnects, or Windows silently drops the hook. So a hold is never trusted forever.
//! While one is active a slow watchdog timer runs here, and it exists only during that time.

use crate::keys::{MODIFIER_CODES, VK_BACK, VK_CAPITAL, VK_ESCAPE, VK_LCONTROL, VK_LMENU, VK_LSHIFT, VK_LWIN};
use crate::util::{SendHwnd, exe_name, process_image_path, qpc};
use orbit_core::Settings;
use orbit_core::snippet::TriggerMatcher;
use orbit_hotkeys::{Effect, HyperConfig, HyperMachine, HyperMode, TapBehavior};
use std::cell::RefCell;
use std::sync::atomic::{AtomicBool, AtomicIsize, AtomicU32, Ordering};
use windows::Win32::Foundation::{HINSTANCE, HWND, LPARAM, LRESULT, RECT, WPARAM};
use windows::Win32::Graphics::Gdi::{GetMonitorInfoW, MONITOR_DEFAULTTONEAREST, MONITORINFO, MonitorFromWindow};
use windows::Win32::System::SystemInformation::GetTickCount;
use windows::Win32::System::Threading::{
    GetCurrentProcessId, GetCurrentThread, GetCurrentThreadId, SetThreadPriority, THREAD_PRIORITY_HIGHEST,
};
use windows::Win32::UI::Accessibility::{HWINEVENTHOOK, SetWinEventHook, UnhookWinEvent};
use windows::Win32::UI::Input::KeyboardAndMouse::{
    GetAsyncKeyState, GetKeyState, GetKeyboardLayout, HKL, INPUT, INPUT_0, INPUT_KEYBOARD, KEYBD_EVENT_FLAGS,
    KEYBDINPUT, KEYEVENTF_EXTENDEDKEY, KEYEVENTF_KEYUP, SendInput, ToUnicodeEx, VIRTUAL_KEY,
};
use windows::Win32::UI::WindowsAndMessaging::{
    CallNextHookEx, DispatchMessageW, EVENT_OBJECT_FOCUS, EVENT_SYSTEM_CAPTURESTART, EVENT_SYSTEM_DESKTOPSWITCH,
    EVENT_SYSTEM_FOREGROUND, GetClassNameW, GetForegroundWindow, GetMessageW, GetWindowRect,
    GetWindowThreadProcessId, HHOOK, KBDLLHOOKSTRUCT, KillTimer, LLKHF_EXTENDED, LLKHF_INJECTED, MSG,
    PeekMessageW, PM_NOREMOVE, PostThreadMessageW, SetTimer, SetWindowsHookExW, TranslateMessage,
    UnhookWindowsHookEx, WH_KEYBOARD_LL, WINEVENT_OUTOFCONTEXT, WINEVENT_SKIPOWNPROCESS, WM_KEYDOWN,
    WM_SYSKEYDOWN, WM_TIMER, WM_USER,
};

/// Marks events Orbit injected, so the hook passes them through instead of acting on them.
pub const INJECTED_TAG: usize = 0x4F52_4249; // "ORBI"

const WM_HOOK_RESET: u32 = WM_USER + 1;
const WM_HOOK_REINSTALL: u32 = WM_USER + 2;
const WM_HOOK_TRIGGERS: u32 = WM_USER + 4;
const WM_HOOK_QUIT: u32 = WM_USER + 5;
const WM_HOOK_RECORD: u32 = WM_USER + 6;
const WM_HOOK_BOUND: u32 = WM_USER + 7;
const WM_HOOK_GAMES: u32 = WM_USER + 8;

/// Tells the launcher thread to quit (`wparam` 0) or hide (1) the app in front.
pub const WM_ORBIT_MAC: u32 = windows::Win32::UI::WindowsAndMessaging::WM_APP + 15;

/// Sends the mouse and keyboard to a remote computer, whose own shortcuts these chords may be.
const REMOTE_SESSION_APPS: &[&str] = &["mstsc.exe", "msrdc.exe", "vmconnect.exe", "vncviewer.exe", "parsecd.exe", "moonlight.exe"];

/// A key Windows has no use for. Pressing it between Alt's press and release tells Windows that
/// Alt was part of a chord, so letting go of Alt doesn't open the window's menu.
const MENU_MASK_VK: u8 = 0xE8;

/// Reinstalling costs microseconds and repairs the case where Windows silently removed the hook
/// after a timeout. The interval is long enough not to matter for idle wakeups.
const REINSTALL_MS: u32 = 5 * 60 * 1000;

const WATCHDOG_TICK_MS: u32 = 1000;
/// A hold with no key event at all for this long means the hook has probably been dropped. A held
/// trigger normally autorepeats, and if it doesn't, the cost is only one bare keypress.
const SILENT_HOLD_MS: u32 = 5000;
/// Nobody holds Hyper this long. A real hold simply restarts on the trigger's next autorepeat.
const LONGEST_HOLD_MS: u32 = 30_000;
/// A trigger's characters are typed together. A pause this long means the caret may have moved,
/// by a mouse click for example, so erasing "the trigger" could delete something else.
const TRIGGER_PAUSE_MS: u32 = 5000;
/// Windows adds a game to its list the first time it runs, so a fullscreen program that isn't on
/// the list is a reason to read it again, but not more often than this.
const GAME_LIST_REFRESH_MS: u32 = 60_000;

/// Virtual modifiers are currently down in the OS. Read by the crash path.
static SYNTHETIC_ACTIVE: AtomicBool = AtomicBool::new(false);
pub static HOOK_INSTALLED: AtomicBool = AtomicBool::new(false);
/// Slowest callback seen, in QPC ticks, for the diagnostics page.
pub static HOOK_MAX_TICKS: AtomicIsize = AtomicIsize::new(0);
pub static HOOK_REINSTALLS: AtomicU32 = AtomicU32::new(0);
/// Other programs' injected input is normally ignored, because the machine and trigger matching
/// are defined over physical keys. The end-to-end tests can only inject, so they opt in.
pub static ACCEPT_INJECTED: AtomicBool = AtomicBool::new(false);

#[derive(Clone)]
pub struct HookConfig {
    pub main: SendHwnd,
    pub hyper_message: u32,
    pub expand_message: u32,
    pub hyper: Option<HyperConfig>,
    pub bound_keys: Vec<u8>,
    pub expansion: bool,
    pub triggers: Vec<(String, i64)>,
    pub hyper_excluded: Vec<String>,
    pub expansion_excluded: Vec<String>,
    pub pass_through_in_games: bool,
    pub mac: MacKeys,
    pub mac_excluded: Vec<String>,
}

/// Which macOS-style shortcuts are on.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct MacKeys {
    pub text: bool,
    pub apps: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MacChord {
    LineStart,
    LineEnd,
    DeleteToLineStart,
    Quit,
    Hide,
}

/// What a key press means with the modifiers held. Windows' AltGr is Ctrl+Alt and types text, so
/// Ctrl rules a chord out. Orbit's own window keeps Alt+H and Alt+Q for itself.
pub fn mac_chord(vk: u8, alt: bool, ctrl: bool, shift: bool, win: bool, keys: MacKeys, own_window: bool) -> Option<MacChord> {
    if !alt || ctrl || win {
        return None;
    }
    match vk {
        0x25 if keys.text => Some(MacChord::LineStart),
        0x27 if keys.text => Some(MacChord::LineEnd),
        0x08 if keys.text && !shift => Some(MacChord::DeleteToLineStart),
        0x51 if keys.apps && !shift && !own_window => Some(MacChord::Quit),
        0x48 if keys.apps && !shift && !own_window => Some(MacChord::Hide),
        _ => None,
    }
}

/// Sends the keys that do what a text chord means. Alt is being held, and an app would read it as
/// part of them, so it's let go for just these keys and pressed again after. A held Shift stays
/// held, so Alt+Shift+Left selects.
fn send_text_chord(chord: MacChord, alt: u8) {
    const HOME: u8 = 0x24;
    const END: u8 = 0x23;
    const SHIFT: u8 = VK_LSHIFT;
    let plain = KEYBD_EVENT_FLAGS(0);
    let up = KEYEVENTF_KEYUP;
    let extended = KEYEVENTF_EXTENDEDKEY;
    let alt_flags = if alt == 0xA5 { extended } else { plain };
    let key = |vk: u8, flags: KEYBD_EVENT_FLAGS| key_input(vk, 0, flags);
    let mut inputs = vec![key(MENU_MASK_VK, plain), key(MENU_MASK_VK, up), key(alt, alt_flags | up)];
    match chord {
        MacChord::LineStart => inputs.extend([key(HOME, extended), key(HOME, extended | up)]),
        MacChord::LineEnd => inputs.extend([key(END, extended), key(END, extended | up)]),
        MacChord::DeleteToLineStart => inputs.extend([
            key(SHIFT, plain),
            key(HOME, extended),
            key(HOME, extended | up),
            key(SHIFT, up),
            key(VK_BACK, plain),
            key(VK_BACK, up),
        ]),
        MacChord::Quit | MacChord::Hide => return,
    }
    inputs.push(key(alt, alt_flags));
    let _ = send(&inputs);
}

struct State {
    config: HookConfig,
    machine: Option<HyperMachine>,
    matcher: TriggerMatcher,
    hook: HHOOK,
    /// The foreground app is on the Hyper exclusion list.
    hyper_excluded_now: bool,
    /// The foreground app is on the expansion exclusion list, or the user suspended shortcuts.
    expansion_excluded_now: bool,
    /// One of Orbit's own windows is in front. Expansion never runs inside them.
    own_foreground: bool,
    /// The program in front is one `game.rs` recognises. It still has to fill the screen to count.
    game_program_now: bool,
    known_games: Vec<String>,
    games_read_at: u32,
    layout: HKL,
    /// Time of the last key event, on the same clock as `KBDLLHOOKSTRUCT::time`.
    last_event: u32,
    last_typed: u32,
    busy_since: u32,
    /// The watchdog timer, while one is running.
    watchdog: usize,
    /// The app in front keeps its own macOS-style keys: the user listed it, or it's a remote session.
    mac_excluded_now: bool,
    /// A chord has used Alt since it went down, so its release needs the menu kept shut.
    mac_alt_used: bool,
    /// Alt+Q or Alt+H is down. Holding one must act once, not at the key repeat rate.
    mac_app_key_down: Option<u8>,
    /// Settings is listening for keys, so macOS-style chords mustn't turn them into others.
    recording: bool,
}

thread_local! {
    static STATE: RefCell<Option<State>> = const { RefCell::new(None) };
}

fn key_input(vk: u8, scan: u16, flags: KEYBD_EVENT_FLAGS) -> INPUT {
    INPUT {
        r#type: INPUT_KEYBOARD,
        Anonymous: INPUT_0 {
            ki: KEYBDINPUT {
                wVk: VIRTUAL_KEY(vk as u16),
                wScan: scan,
                dwFlags: flags,
                time: 0,
                dwExtraInfo: INJECTED_TAG,
            },
        },
    }
}

fn send(inputs: &[INPUT]) -> bool {
    unsafe { SendInput(inputs, std::mem::size_of::<INPUT>() as i32) as usize == inputs.len() }
}

fn press_and_release(vk: u8) {
    send(&[key_input(vk, 0, KEYBD_EVENT_FLAGS(0)), key_input(vk, 0, KEYEVENTF_KEYUP)]);
}

const SYNTHETIC_MODIFIERS: [u8; 4] = [VK_LCONTROL, VK_LMENU, VK_LSHIFT, VK_LWIN];

/// Releases the virtual modifiers. Safe to call from any thread, including the panic handler.
/// If Windows refuses, on the secure desktop for example, they still count as down, and the
/// watchdog and the next foreground change try again.
pub fn release_synthetic_modifiers() {
    if !SYNTHETIC_ACTIVE.load(Ordering::SeqCst) {
        return;
    }
    // The mask key stops a lone Win key-up from opening the Start menu.
    const MASK_VK: u8 = 0xFF;
    let up = KEYEVENTF_KEYUP;
    let sent = send(&[
        key_input(MASK_VK, 0, KEYBD_EVENT_FLAGS(0)),
        key_input(MASK_VK, 0, up),
        key_input(VK_LWIN, 0, up),
        key_input(VK_LSHIFT, 0, up),
        key_input(VK_LMENU, 0, up),
        key_input(VK_LCONTROL, 0, up),
    ]);
    if sent {
        SYNTHETIC_ACTIVE.store(false, Ordering::SeqCst);
    }
}

/// If an earlier Orbit was killed mid-chord, nothing could release its virtual modifiers. So at
/// the first start of the hook, any that read as down are let go. Only at process start: later,
/// a settings save with Ctrl+S would get its real Ctrl released under the user's finger.
fn clear_leftover_modifiers() {
    static DONE: AtomicBool = AtomicBool::new(false);
    if DONE.swap(true, Ordering::SeqCst) {
        return;
    }
    let any_down = SYNTHETIC_MODIFIERS
        .iter()
        .any(|&vk| unsafe { GetAsyncKeyState(vk as i32) } as u16 & 0x8000 != 0);
    if any_down {
        SYNTHETIC_ACTIVE.store(true, Ordering::SeqCst);
        release_synthetic_modifiers();
    }
}

/// For use outside a key event, where the foreground may be a window that takes no injected
/// input. `SendInput` reports success in that case, so it's checked first.
fn release_unless_blocked() {
    if SYNTHETIC_ACTIVE.load(Ordering::SeqCst)
        && !crate::clipboard::target_is_elevated(unsafe { GetForegroundWindow() })
    {
        release_synthetic_modifiers();
    }
}

#[derive(Clone, Copy)]
struct PostTarget {
    main: SendHwnd,
    hyper_message: u32,
    expand_message: u32,
    trigger: u8,
}

fn perform(effect: Effect, config: PostTarget, scan: u16, extended: bool, t_event: i64) {
    match effect {
        Effect::Dispatch { code } => config.main.post(config.hyper_message, code as usize, t_event as isize),
        Effect::Tap(TapBehavior::Escape) => press_and_release(VK_ESCAPE),
        Effect::Tap(TapBehavior::Original) => press_and_release(config.trigger),
        Effect::Tap(TapBehavior::None | TapBehavior::Leader) => {}
        Effect::SyntheticChordDown { code } => {
            SYNTHETIC_ACTIVE.store(true, Ordering::SeqCst);
            let down = KEYBD_EVENT_FLAGS(0);
            let flags = if extended { KEYEVENTF_EXTENDEDKEY } else { down };
            let _ = send(&[
                key_input(SYNTHETIC_MODIFIERS[0], 0, down),
                key_input(SYNTHETIC_MODIFIERS[1], 0, down),
                key_input(SYNTHETIC_MODIFIERS[2], 0, down),
                key_input(SYNTHETIC_MODIFIERS[3], 0, down),
                key_input(code, scan, flags),
            ]);
        }
        Effect::SyntheticRelease => release_synthetic_modifiers(),
    }
}

/// Is the foreground window covering a whole monitor, and not the desktop itself?
pub fn foreground_is_fullscreen() -> bool {
    unsafe {
        let hwnd = GetForegroundWindow();
        if hwnd.is_invalid() {
            return false;
        }
        let mut class = [0u16; 16];
        let len = GetClassNameW(hwnd, &mut class).max(0) as usize;
        let is = |name: &str| name.encode_utf16().eq(class[..len].iter().copied());
        if is("Progman") || is("WorkerW") {
            return false;
        }
        let mut rect = RECT::default();
        if GetWindowRect(hwnd, &mut rect).is_err() {
            return false;
        }
        let monitor = MonitorFromWindow(hwnd, MONITOR_DEFAULTTONEAREST);
        let mut info = MONITORINFO { cbSize: std::mem::size_of::<MONITORINFO>() as u32, ..Default::default() };
        if !GetMonitorInfoW(monitor, &mut info).as_bool() {
            return false;
        }
        let screen = info.rcMonitor;
        rect.left <= screen.left && rect.top <= screen.top && rect.right >= screen.right && rect.bottom >= screen.bottom
    }
}

/// A game filling the screen, as opposed to any other program doing that.
fn game_in_front(game_program: bool) -> bool {
    foreground_is_fullscreen() && (game_program || crate::game::exclusive_fullscreen())
}

/// Text produced by a key, without disturbing dead-key state in the focused app.
/// Flag bit 2 of `ToUnicodeEx` (Windows 10 1607+) means "do not change keyboard state".
fn translate_key(vk: u8, scan: u32, layout: HKL) -> Option<[u16; 4]> {
    let mut keys = [0u8; 256];
    unsafe {
        for (generic, left, right) in [(0x10usize, 0xA0usize, 0xA1usize), (0x11, 0xA2, 0xA3), (0x12, 0xA4, 0xA5)] {
            let left_down = GetAsyncKeyState(left as i32) as u16 & 0x8000 != 0;
            let right_down = GetAsyncKeyState(right as i32) as u16 & 0x8000 != 0;
            if left_down {
                keys[left] = 0x80;
            }
            if right_down {
                keys[right] = 0x80;
            }
            if left_down || right_down {
                keys[generic] = 0x80;
            }
        }
        if GetKeyState(VK_CAPITAL as i32) & 1 != 0 {
            keys[VK_CAPITAL as usize] = 1;
        }
        let mut buffer = [0u16; 4];
        let written = ToUnicodeEx(vk as u32, scan, &keys, &mut buffer, 1 << 2, Some(layout));
        (written > 0).then_some(buffer)
    }
}

fn feed_expansion(state: &mut State, vk: u8, scan: u32, time: u32) -> Option<(i64, usize)> {
    if time.wrapping_sub(state.last_typed) >= TRIGGER_PAUSE_MS {
        state.matcher.clear();
    }
    state.last_typed = time;
    // Navigation and editing keys make "the text before the caret" unknowable.
    const CLEARS: &[u8] = &[0x09, 0x0D, 0x1B, 0x21, 0x22, 0x23, 0x24, 0x25, 0x26, 0x27, 0x28, 0x2D, 0x2E];
    if vk == VK_BACK {
        state.matcher.backspace();
        return None;
    }
    if CLEARS.contains(&vk) {
        state.matcher.clear();
        return None;
    }
    if MODIFIER_CODES.contains(&vk) {
        return None;
    }
    unsafe {
        let ctrl = GetAsyncKeyState(0x11) as u16 & 0x8000 != 0;
        let alt = GetAsyncKeyState(0x12) as u16 & 0x8000 != 0;
        let win = (GetAsyncKeyState(0x5B) as u16 | GetAsyncKeyState(0x5C) as u16) & 0x8000 != 0;
        // Ctrl+Alt is AltGr and types text. Ctrl alone or Alt alone are commands.
        if win || (ctrl != alt) {
            state.matcher.clear();
            return None;
        }
    }
    let Some(text) = translate_key(vk, scan, state.layout) else {
        state.matcher.clear();
        return None;
    };
    let mut hit = None;
    for decoded in char::decode_utf16(text.iter().copied().take_while(|&c| c != 0)) {
        match decoded {
            Ok(c) => hit = state.matcher.push(c).or(hit),
            Err(_) => state.matcher.clear(),
        }
    }
    hit.map(|h| (h.snippet_id, h.trigger_chars))
}

unsafe extern "system" fn keyboard_proc(code: i32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    unsafe {
        if code < 0 {
            return CallNextHookEx(None, code, wparam, lparam);
        }
        let event = &*(lparam.0 as *const KBDLLHOOKSTRUCT);
        if event.dwExtraInfo == INJECTED_TAG {
            return CallNextHookEx(None, code, wparam, lparam);
        }
        // A password manager's auto-type must not reach trigger matching, and another program's
        // input isn't a key the user pressed.
        if event.flags.contains(LLKHF_INJECTED) && !ACCEPT_INJECTED.load(Ordering::Relaxed) {
            return CallNextHookEx(None, code, wparam, lparam);
        }
        let start = qpc();
        let is_down = wparam.0 as u32 == WM_KEYDOWN || wparam.0 as u32 == WM_SYSKEYDOWN;
        let vk = event.vkCode as u8;
        let scan = event.scanCode as u16;
        let extended = event.flags.contains(LLKHF_EXTENDED);

        // Decide with the state borrowed, then act with it released: `SendInput` and
        // `PostMessage` must not run while the state is locked.
        let mut decision: Option<orbit_hotkeys::Decision> = None;
        let mut expanded = None;
        let mut target = None;
        let mut mac_hit: Option<(MacChord, u8)> = None;
        let mut mac_swallow = false;
        let mut mask_menu = false;
        STATE.with(|cell| {
            let Ok(mut guard) = cell.try_borrow_mut() else { return };
            let Some(state) = guard.as_mut() else { return };
            let excluded = state.hyper_excluded_now;
            let pass_in_games = state.config.pass_through_in_games;
            let game_program = state.game_program_now;
            state.last_event = event.time;
            if let Some(machine) = state.machine.as_mut() {
                let was_idle = machine.is_idle();
                let is_trigger = state.config.hyper.as_ref().is_some_and(|h| h.trigger == vk);
                if is_trigger && is_down && !machine.is_holding() {
                    let suspend = excluded || (pass_in_games && game_in_front(game_program));
                    // Nothing is held at this point, so changing suspension has no effects to run.
                    let _ = machine.set_suspended(suspend);
                }
                decision = Some(machine.on_key(vk, is_down, event.time));
                if was_idle && !machine.is_idle() {
                    state.busy_since = event.time;
                    arm_watchdog(state);
                }
            }
            if decision.is_some_and(|d| d.swallow) {
                // A chord isn't text, and what it does may move the caret.
                state.matcher.clear();
            } else if is_down && state.config.expansion && !state.expansion_excluded_now && !state.own_foreground {
                expanded = feed_expansion(state, vk, event.scanCode, event.time);
                // Erasing the trigger means sending Backspaces, which a game would act on.
                if expanded.is_some() && game_in_front(game_program) {
                    expanded = None;
                }
            }
            // macOS-style chords. Decided here, because the key must not reach the app.
            let mac = state.config.mac;
            if mac != MacKeys::default() {
                if !is_down {
                    if matches!(vk, 0x12 | 0xA4 | 0xA5) && state.mac_alt_used {
                        state.mac_alt_used = false;
                        mask_menu = true;
                    }
                    if state.mac_app_key_down == Some(vk) {
                        state.mac_app_key_down = None;
                    }
                } else if !decision.is_some_and(|d| d.swallow) && !state.mac_excluded_now && !state.recording {
                    let held = |key: i32| GetAsyncKeyState(key) as u16 & 0x8000 != 0;
                    let chord = mac_chord(vk, held(0x12), held(0x11), held(0x10), held(0x5B) || held(0x5C), mac, state.own_foreground);
                    if let Some(chord) = chord {
                        if !game_in_front(game_program) {
                            mac_swallow = true;
                            state.mac_alt_used = true;
                            state.matcher.clear();
                            expanded = None;
                            let is_app_key = matches!(chord, MacChord::Quit | MacChord::Hide);
                            if !is_app_key || state.mac_app_key_down != Some(vk) {
                                let alt = if held(0xA5) { 0xA5 } else { VK_LMENU };
                                mac_hit = Some((chord, alt));
                            }
                            if is_app_key {
                                state.mac_app_key_down = Some(vk);
                            }
                        }
                    }
                }
            }
            target = Some(PostTarget {
                main: state.config.main,
                hyper_message: state.config.hyper_message,
                expand_message: state.config.expand_message,
                trigger: state.config.hyper.as_ref().map_or(VK_CAPITAL, |h| h.trigger),
            });
        });

        let swallow = decision.is_some_and(|d| d.swallow) || mac_swallow;
        if mask_menu {
            press_and_release(MENU_MASK_VK);
        }
        if let (Some((chord, alt)), Some(target)) = (mac_hit, target) {
            match chord {
                MacChord::Quit => target.main.post(WM_ORBIT_MAC, 0, 0),
                MacChord::Hide => target.main.post(WM_ORBIT_MAC, 1, 0),
                _ => send_text_chord(chord, alt),
            }
        }
        if let Some(target) = target {
            if let Some(decision) = decision {
                for effect in decision.effects() {
                    perform(effect, target, scan, extended, start);
                }
            }
            if let Some((snippet_id, trigger_chars)) = expanded {
                target.main.post(target.expand_message, snippet_id as usize, trigger_chars as isize);
            }
        }

        let ticks = qpc() - start;
        if ticks as isize > HOOK_MAX_TICKS.load(Ordering::Relaxed) {
            HOOK_MAX_TICKS.store(ticks as isize, Ordering::Relaxed);
        }

        if swallow {
            return LRESULT(1);
        }
        CallNextHookEx(None, code, wparam, lparam)
    }
}

unsafe extern "system" fn foreground_changed(
    _hook: HWINEVENTHOOK,
    event: u32,
    hwnd: HWND,
    _object: i32,
    _child: i32,
    _thread: u32,
    _time: u32,
) {
    STATE.with(|cell| {
        let Ok(mut guard) = cell.try_borrow_mut() else { return };
        let Some(state) = guard.as_mut() else { return };
        match event {
            EVENT_SYSTEM_FOREGROUND => update_foreground(state, hwnd),
            // The secure desktop, as UAC uses. Nothing typed there reaches this hook.
            EVENT_SYSTEM_DESKTOPSWITCH => apply_reset(state),
            // Focus moved inside a window, or a click landed: the caret may have moved.
            _ => state.matcher.clear(),
        }
    });
}

fn is_busy(state: &State) -> bool {
    state.machine.as_ref().is_some_and(|machine| !machine.is_idle()) || SYNTHETIC_ACTIVE.load(Ordering::SeqCst)
}

fn arm_watchdog(state: &mut State) {
    if state.watchdog == 0 {
        state.watchdog = unsafe { SetTimer(None, 0, WATCHDOG_TICK_MS, None) };
    }
}

fn watchdog_tick(state: &mut State) {
    if !is_busy(state) {
        unsafe {
            let _ = KillTimer(None, state.watchdog);
        }
        state.watchdog = 0;
        return;
    }
    let holding = state.machine.as_ref().is_some_and(|machine| !machine.is_idle());
    if holding {
        let now = unsafe { GetTickCount() };
        let silent = now.wrapping_sub(state.last_event) >= SILENT_HOLD_MS;
        let too_long = now.wrapping_sub(state.busy_since) >= LONGEST_HOLD_MS;
        if silent || too_long {
            reinstall(state, true);
        }
    } else {
        release_unless_blocked();
    }
}

fn update_foreground(state: &mut State, hwnd: HWND) {
    state.matcher.clear();
    unsafe {
        let mut pid = 0;
        let thread = GetWindowThreadProcessId(hwnd, Some(&mut pid));
        state.layout = GetKeyboardLayout(thread);
        state.own_foreground = pid == GetCurrentProcessId();
        if is_busy(state) {
            // Keys pressed in an elevated window never reach this hook, so the key-up that would
            // end this hold won't come. Otherwise the hold carries on across the focus change,
            // because a chord often opens the app the next chord is meant for.
            if crate::clipboard::target_is_elevated(hwnd) {
                apply_reset(state);
            } else if state.machine.as_ref().is_none_or(HyperMachine::is_idle) {
                release_synthetic_modifiers();
            }
        }
        let path = process_image_path(pid).unwrap_or_default();
        state.game_program_now = crate::game::is_game_path(&path.to_lowercase(), &state.known_games);
        if !state.game_program_now && foreground_is_fullscreen() {
            let now = GetTickCount();
            if now.wrapping_sub(state.games_read_at) >= GAME_LIST_REFRESH_MS {
                state.games_read_at = now;
                read_game_list(GetCurrentThreadId());
            }
        }
        let name = exe_name(&path);
        state.hyper_excluded_now = Settings::is_app_excluded(&state.config.hyper_excluded, name);
        state.expansion_excluded_now = Settings::is_app_excluded(&state.config.expansion_excluded, name);
        state.mac_excluded_now = Settings::is_app_excluded(&state.config.mac_excluded, name)
            || REMOTE_SESSION_APPS.iter().any(|app| app.eq_ignore_ascii_case(name));
    }
}

/// Reads the registry off this thread, so a slow read can't hold up keypresses.
fn read_game_list(hook_thread: u32) {
    let _ = std::thread::Builder::new().name("orbit-games".into()).stack_size(128 * 1024).spawn(move || {
        let posted = crate::util::box_for_post(crate::game::windows_game_list());
        unsafe {
            if PostThreadMessageW(hook_thread, WM_HOOK_GAMES, WPARAM(0), LPARAM(posted as isize)).is_err() {
                drop(crate::util::take_posted::<Vec<String>>(posted));
            }
        }
    });
}

fn install_hook() -> Option<HHOOK> {
    unsafe {
        SetWindowsHookExW(WH_KEYBOARD_LL, Some(keyboard_proc), Some(HINSTANCE::default()), 0).ok()
    }
}

fn apply_reset(state: &mut State) {
    state.matcher.clear();
    state.mac_alt_used = false;
    state.mac_app_key_down = None;
    if let Some(machine) = state.machine.as_mut() {
        for effect in machine.reset().effects() {
            if effect == Effect::SyntheticRelease {
                release_unless_blocked();
            }
        }
    }
    if SYNTHETIC_ACTIVE.load(Ordering::SeqCst) {
        arm_watchdog(state); // keep trying until Windows accepts the release
    }
}

fn thread_main(config: HookConfig, ready: std::sync::mpsc::Sender<Option<u32>>) {
    unsafe {
        let _ = SetThreadPriority(GetCurrentThread(), THREAD_PRIORITY_HIGHEST);
        // Forces creation of this thread's message queue before the id is handed out.
        let mut msg = MSG::default();
        let _ = PeekMessageW(&mut msg, None, 0, 0, PM_NOREMOVE);

        if config.hyper.is_some() {
            clear_leftover_modifiers();
        }
        let machine = config.hyper.clone().map(|hyper| {
            let mut modifiers: Vec<u8> = MODIFIER_CODES.iter().copied().filter(|&c| c != hyper.trigger).collect();
            modifiers.sort_unstable();
            let mut machine = HyperMachine::new(hyper, &modifiers);
            machine.set_bound_keys(config.bound_keys.iter().copied());
            machine
        });
        let matcher = TriggerMatcher::new(config.triggers.iter().map(|(t, id)| (t.as_str(), *id)));

        let Some(hook) = install_hook() else {
            let _ = ready.send(None);
            return;
        };
        HOOK_INSTALLED.store(true, Ordering::SeqCst);

        // Orbit's own windows are included here: expansion has to know when they're in front.
        let event_hook = SetWinEventHook(
            EVENT_SYSTEM_FOREGROUND,
            EVENT_SYSTEM_FOREGROUND,
            None,
            Some(foreground_changed),
            0,
            0,
            WINEVENT_OUTOFCONTEXT,
        );
        let mut event_hooks = vec![event_hook];
        let watched: &[u32] = if config.expansion {
            &[EVENT_SYSTEM_DESKTOPSWITCH, EVENT_OBJECT_FOCUS, EVENT_SYSTEM_CAPTURESTART]
        } else {
            &[EVENT_SYSTEM_DESKTOPSWITCH]
        };
        for &watched_event in watched {
            event_hooks.push(SetWinEventHook(
                watched_event,
                watched_event,
                None,
                Some(foreground_changed),
                0,
                0,
                WINEVENT_OUTOFCONTEXT | WINEVENT_SKIPOWNPROCESS,
            ));
        }

        STATE.with(|cell| {
            let mut state = State {
                config,
                machine,
                matcher,
                hook,
                hyper_excluded_now: false,
                expansion_excluded_now: false,
                own_foreground: false,
                game_program_now: false,
                known_games: Vec::new(),
                games_read_at: GetTickCount(),
                layout: GetKeyboardLayout(0),
                last_event: 0,
                last_typed: 0,
                busy_since: 0,
                watchdog: 0,
                mac_excluded_now: false,
                mac_alt_used: false,
                mac_app_key_down: None,
                recording: false,
            };
            update_foreground(&mut state, GetForegroundWindow());
            *cell.borrow_mut() = Some(state);
        });
        let timer = SetTimer(None, 0, REINSTALL_MS, None);
        read_game_list(GetCurrentThreadId());
        let _ = ready.send(Some(GetCurrentThreadId()));

        while GetMessageW(&mut msg, None, 0, 0).as_bool() {
            match msg.message {
                WM_HOOK_QUIT => break,
                WM_HOOK_RESET => with_state(apply_reset),
                WM_HOOK_REINSTALL => with_state(|state| reinstall(state, true)),
                WM_HOOK_RECORD => with_state(|state| {
                    state.recording = msg.wParam.0 != 0;
                    if let Some(machine) = state.machine.as_mut() {
                        machine.set_recording(msg.wParam.0 != 0);
                    }
                }),
                WM_HOOK_BOUND => {
                    if let Some(keys) = crate::util::take_posted::<Vec<u8>>(msg.lParam.0 as usize) {
                        with_state(|state| {
                            if let Some(machine) = state.machine.as_mut() {
                                machine.set_bound_keys(keys.iter().copied());
                            }
                        });
                    }
                }
                WM_HOOK_GAMES => {
                    if let Some(list) = crate::util::take_posted::<Vec<String>>(msg.lParam.0 as usize) {
                        with_state(|state| {
                            state.known_games = *list;
                            let mut pid = 0;
                            GetWindowThreadProcessId(GetForegroundWindow(), Some(&mut pid));
                            let path = process_image_path(pid).unwrap_or_default().to_lowercase();
                            state.game_program_now = crate::game::is_game_path(&path, &state.known_games);
                        });
                    }
                }
                WM_TIMER if msg.wParam.0 == timer => with_state(|state| reinstall(state, false)),
                WM_TIMER => with_state(|state| {
                    if msg.wParam.0 == state.watchdog {
                        watchdog_tick(state);
                    }
                }),
                WM_HOOK_TRIGGERS => {
                    if let Some(list) = crate::util::take_posted::<Vec<(String, i64)>>(msg.lParam.0 as usize) {
                        with_state(|state| {
                            state.matcher = TriggerMatcher::new(list.iter().map(|(t, id)| (t.as_str(), *id)));
                        });
                    }
                }
                _ => {
                    let _ = TranslateMessage(&msg);
                    DispatchMessageW(&msg);
                }
            }
        }

        // Shut down in an order that cannot leave the keyboard in a modified state.
        with_state(apply_reset);
        let hook = STATE.with(|cell| cell.borrow_mut().take()).map(|state| state.hook);
        if let Some(hook) = hook {
            let _ = UnhookWindowsHookEx(hook);
        }
        for event_hook in event_hooks {
            let _ = UnhookWinEvent(event_hook);
        }
        HOOK_INSTALLED.store(false, Ordering::SeqCst);
    }
}

fn with_state(f: impl FnOnce(&mut State)) {
    STATE.with(|cell| {
        if let Ok(mut guard) = cell.try_borrow_mut() {
            if let Some(state) = guard.as_mut() {
                f(state);
            }
        }
    });
}

fn reinstall(state: &mut State, force: bool) {
    let idle = state.machine.as_ref().is_none_or(HyperMachine::is_idle);
    if !idle && !force {
        return; // mid-chord: try again at the next interval
    }
    if force {
        apply_reset(state);
    }
    if let Some(new_hook) = install_hook() {
        unsafe {
            let _ = UnhookWindowsHookEx(state.hook);
        }
        state.hook = new_hook;
        HOOK_REINSTALLS.fetch_add(1, Ordering::Relaxed);
    }
}

pub struct HookHandle {
    thread_id: u32,
    join: Option<std::thread::JoinHandle<()>>,
}

impl HookHandle {
    /// Starts the thread and waits for the install result. `None` means Windows refused the hook.
    pub fn start(config: HookConfig) -> Option<HookHandle> {
        let (tx, rx) = std::sync::mpsc::channel();
        let join = std::thread::Builder::new()
            .name("orbit-hook".into())
            .stack_size(128 * 1024)
            .spawn(move || thread_main(config, tx))
            .ok()?;
        let thread_id = rx.recv().ok()??;
        Some(HookHandle { thread_id, join: Some(join) })
    }

    fn post(&self, message: u32, wparam: usize, lparam: isize) -> bool {
        unsafe { PostThreadMessageW(self.thread_id, message, WPARAM(wparam), LPARAM(lparam)).is_ok() }
    }

    /// Forget transient key state and release anything virtual. Call on lock and sleep.
    pub fn reset(&self) {
        self.post(WM_HOOK_RESET, 0, 0);
    }

    /// Replace the hook. Call after resume, in case Windows dropped it.
    pub fn reinstall(&self) {
        self.post(WM_HOOK_REINSTALL, 0, 0);
    }

    /// While on, Hyper plus any key is reported, so the launcher can bind it.
    pub fn set_recording(&self, recording: bool) {
        self.post(WM_HOOK_RECORD, recording as usize, 0);
    }

    /// New mappings take effect without replacing the hook, which may be mid-chord right now.
    pub fn set_bound_keys(&self, keys: Vec<u8>) {
        let posted = crate::util::box_for_post(keys);
        if !self.post(WM_HOOK_BOUND, 0, posted as isize) {
            drop(crate::util::take_posted::<Vec<u8>>(posted));
        }
    }

    pub fn set_triggers(&self, triggers: Vec<(String, i64)>) {
        let posted = crate::util::box_for_post(triggers);
        if !self.post(WM_HOOK_TRIGGERS, 0, posted as isize) {
            drop(crate::util::take_posted::<Vec<(String, i64)>>(posted));
        }
    }
}

impl Drop for HookHandle {
    fn drop(&mut self) {
        self.post(WM_HOOK_QUIT, 0, 0);
        if let Some(join) = self.join.take() {
            let _ = join.join();
        }
    }
}

/// Builds the hook configuration from settings. `None` means no hook is needed at all, which
/// is the default state: no input hook runs unless a feature that needs one is on.
pub fn config_from_settings(
    settings: &Settings,
    main: SendHwnd,
    hyper_message: u32,
    expand_message: u32,
    triggers: Vec<(String, i64)>,
) -> Option<HookConfig> {
    let hyper_on = settings.hyper.enabled && settings.hyper.mode != HyperMode::External;
    let expansion = settings.snippets.expansion_enabled;
    let mac = if settings.mac_feel.enabled {
        MacKeys { text: settings.mac_feel.text_keys, apps: settings.mac_feel.app_keys }
    } else {
        MacKeys::default()
    };
    if !hyper_on && !expansion && mac == MacKeys::default() {
        return None;
    }
    let hyper = if hyper_on {
        crate::keys::vk_from_name(&settings.hyper.trigger).map(|trigger| HyperConfig {
            trigger,
            mode: settings.hyper.mode,
            tap: settings.hyper.tap,
            tap_timeout_ms: settings.hyper.tap_timeout_ms,
        })
    } else {
        None
    };
    Some(HookConfig {
        main,
        hyper_message,
        expand_message,
        hyper,
        bound_keys: bound_keys(settings),
        expansion,
        triggers,
        hyper_excluded: settings.hyper.excluded_apps.clone(),
        expansion_excluded: settings.snippets.excluded_apps.clone(),
        pass_through_in_games: settings.hyper.pass_through_in_games,
        mac,
        mac_excluded: settings.mac_feel.excluded_apps.clone(),
    })
}

pub fn bound_keys(settings: &Settings) -> Vec<u8> {
    settings
        .hyper
        .bindings
        .iter()
        .filter(|binding| binding.enabled)
        .filter_map(|binding| crate::keys::vk_from_name(&binding.key))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn main_hwnd() -> SendHwnd {
        SendHwnd(0)
    }

    const ALL_KEYS: MacKeys = MacKeys { text: true, apps: true };

    #[test]
    fn mac_chords_need_alt_and_nothing_that_means_something_else() {
        let chord = |vk, alt, ctrl, shift, win| mac_chord(vk, alt, ctrl, shift, win, ALL_KEYS, false);
        assert_eq!(chord(0x25, true, false, false, false), Some(MacChord::LineStart));
        assert_eq!(chord(0x27, true, false, false, false), Some(MacChord::LineEnd));
        assert_eq!(chord(0x08, true, false, false, false), Some(MacChord::DeleteToLineStart));
        assert_eq!(chord(0x51, true, false, false, false), Some(MacChord::Quit));
        assert_eq!(chord(0x48, true, false, false, false), Some(MacChord::Hide));
        assert_eq!(chord(0x25, true, false, true, false), Some(MacChord::LineStart), "Shift selects");
        assert_eq!(chord(0x25, false, false, false, false), None, "a plain arrow");
        assert_eq!(chord(0x51, true, true, false, false), None, "Ctrl+Alt is AltGr, which types");
        assert_eq!(chord(0x51, true, false, true, false), None);
        assert_eq!(chord(0x48, true, false, false, true), None);
        assert_eq!(chord(0x41, true, false, false, false), None, "other letters are left alone");
    }

    #[test]
    fn mac_chords_follow_the_settings_and_stay_out_of_orbit() {
        let text = MacKeys { text: true, apps: false };
        let apps = MacKeys { text: false, apps: true };
        assert_eq!(mac_chord(0x51, true, false, false, false, text, false), None);
        assert_eq!(mac_chord(0x25, true, false, false, false, apps, false), None);
        assert_eq!(mac_chord(0x48, true, false, false, false, ALL_KEYS, true), None, "Alt+H is Vim keys in the launcher");
        assert_eq!(mac_chord(0x25, true, false, false, false, ALL_KEYS, true), Some(MacChord::LineStart));
    }

    #[test]
    fn mac_feel_alone_asks_for_a_hook_and_off_asks_for_none() {
        let mut settings = Settings::default();
        settings.mac_feel = orbit_core::settings::MacFeelSettings { enabled: false, text_keys: true, app_keys: true, excluded_apps: Vec::new() };
        assert!(config_from_settings(&settings, main_hwnd(), 1, 2, Vec::new()).is_none());
        settings.mac_feel.enabled = true;
        let config = config_from_settings(&settings, main_hwnd(), 1, 2, Vec::new()).unwrap();
        assert_eq!(config.mac, ALL_KEYS);
        settings.mac_feel.text_keys = false;
        settings.mac_feel.app_keys = false;
        assert!(config_from_settings(&settings, main_hwnd(), 1, 2, Vec::new()).is_none(), "on with nothing chosen");
    }

    #[test]
    fn no_hook_by_default() {
        let settings = Settings::default();
        assert!(config_from_settings(&settings, main_hwnd(), 0, 0, vec![]).is_none());
    }

    #[test]
    fn external_mode_needs_no_hook() {
        let mut settings = Settings::default();
        settings.hyper.enabled = true;
        settings.hyper.mode = HyperMode::External;
        assert!(config_from_settings(&settings, main_hwnd(), 0, 0, vec![]).is_none());
    }

    #[test]
    fn hyper_or_expansion_requests_a_hook() {
        let mut settings = Settings::default();
        settings.hyper.enabled = true;
        let config = config_from_settings(&settings, main_hwnd(), 0, 0, vec![]).unwrap();
        assert_eq!(config.hyper.unwrap().trigger, VK_CAPITAL);
        assert!(!config.expansion);

        let mut settings = Settings::default();
        settings.snippets.expansion_enabled = true;
        let config = config_from_settings(&settings, main_hwnd(), 0, 0, vec![]).unwrap();
        assert!(config.hyper.is_none());
        assert!(config.expansion);
    }

    #[test]
    fn release_without_activation_sends_nothing() {
        SYNTHETIC_ACTIVE.store(false, Ordering::SeqCst);
        release_synthetic_modifiers();
        assert!(!SYNTHETIC_ACTIVE.load(Ordering::SeqCst));
    }

    #[test]
    fn a_hook_is_only_kept_alive_by_a_hold() {
        assert!(SILENT_HOLD_MS < LONGEST_HOLD_MS);
        assert!(WATCHDOG_TICK_MS <= SILENT_HOLD_MS / 2);
    }
}
