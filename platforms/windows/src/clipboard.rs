//! Reading and writing the Windows clipboard.
//!
//! Change detection is `AddClipboardFormatListener`, with no polling, so a pause or disabled
//! history removes the listener and costs nothing. Before any content is read, the clipboard is
//! checked for the formats password managers use to opt out of history.

use crate::hook::INJECTED_TAG;
use crate::util::{exe_name, process_image_path, wide};
use orbit_core::Settings;
use orbit_data::{Formats, NewClip};
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Mutex, MutexGuard, TryLockError};
use windows::Win32::Foundation::{GlobalFree, HANDLE, HGLOBAL, HWND};
use windows::Win32::System::DataExchange::{
    CloseClipboard, EmptyClipboard, GetClipboardData, GetClipboardOwner, GetClipboardSequenceNumber,
    IsClipboardFormatAvailable, OpenClipboard, RegisterClipboardFormatW, SetClipboardData,
};
use windows::Win32::System::Memory::{GMEM_MOVEABLE, GlobalAlloc, GlobalLock, GlobalSize, GlobalUnlock};
use windows::Win32::System::Ole::{CF_DIB, CF_HDROP, CF_UNICODETEXT};
use windows::Win32::UI::Input::KeyboardAndMouse::{
    GetAsyncKeyState, INPUT, INPUT_0, INPUT_KEYBOARD, KEYBD_EVENT_FLAGS, KEYBDINPUT, KEYEVENTF_KEYUP,
    KEYEVENTF_UNICODE, SendInput, VIRTUAL_KEY,
};
use windows::Win32::UI::Shell::{DragQueryFileW, HDROP};
use windows::Win32::UI::WindowsAndMessaging::{GetForegroundWindow, GetWindowThreadProcessId};
use windows::core::PCWSTR;

/// Sequence number of Orbit's most recent clipboard write. Its own writes are not history.
static OWN_WRITE_SEQ: AtomicU32 = AtomicU32::new(0);

/// Sequence number of the content the last capture read, so a burst of change notices reads it once.
static LAST_CAPTURE_SEQ: AtomicU32 = AtomicU32::new(0);

/// Held for as long as Orbit has the clipboard open. Windows keeps other processes out, but not
/// Orbit's own threads: while one has it open, another's `OpenClipboard(NULL)` succeeds too, and two
/// threads reading at once corrupt the heap. Each clipboard change starts a capture on its own
/// thread, so two changes close together did exactly that and crashed Orbit (0xc0000374).
static OPEN: Mutex<()> = Mutex::new(());

fn registered_format(name: &str) -> u32 {
    let text = wide(name);
    unsafe { RegisterClipboardFormatW(PCWSTR(text.as_ptr())) }
}

struct ClipboardGuard {
    wrote: std::cell::Cell<bool>,
    // Dropped after `drop` has closed the clipboard, so the next thread opens it only then.
    _open: MutexGuard<'static, ()>,
}

impl ClipboardGuard {
    fn open() -> Option<ClipboardGuard> {
        // Another process may hold the clipboard for a moment, and a clipboard tool can hold it for
        // longer. Retries beat a lost write or a missed entry, and this only runs on a real change,
        // off the launcher thread, or for a single user action. Orbit's own threads are waited for
        // the same way, with the same few tries, so a stuck one can't hang the launcher.
        for attempt in 0..7 {
            let open = match OPEN.try_lock() {
                Ok(open) => Some(open),
                // Nothing it guards can be left half done: the clipboard closes as a panic unwinds.
                Err(TryLockError::Poisoned(poisoned)) => Some(poisoned.into_inner()),
                Err(TryLockError::WouldBlock) => None,
            };
            if let Some(open) = open
                && unsafe { OpenClipboard(None) }.is_ok()
            {
                return Some(ClipboardGuard { wrote: std::cell::Cell::new(false), _open: open });
            }
            // A lock that came without the clipboard is let go here, before the wait.
            std::thread::sleep(std::time::Duration::from_millis(2 << attempt));
        }
        None
    }
}

impl ClipboardGuard {
    /// Notes that this write is Orbit's own. The number to remember only exists after closing,
    /// because closing a changed clipboard moves it again, so `drop` reads it.
    fn wrote(&self) {
        self.wrote.set(true);
    }
}

impl Drop for ClipboardGuard {
    fn drop(&mut self) {
        unsafe {
            let _ = CloseClipboard();
            if self.wrote.get() {
                OWN_WRITE_SEQ.store(GetClipboardSequenceNumber(), Ordering::Relaxed);
            }
        }
    }
}

fn has_format(format: u32) -> bool {
    unsafe { IsClipboardFormatAvailable(format).is_ok() }
}

/// Formats apps set to say "do not keep this". They are honoured without reading the content.
/// If the marker is there but can't be read, the answer is yes.
fn app_opted_out() -> bool {
    if has_format(registered_format("ExcludeClipboardContentFromMonitorProcessing"))
        || has_format(registered_format("Clipboard Viewer Ignore"))
    {
        return true;
    }
    // A DWORD of zero means "not allowed in history".
    let format = registered_format("CanIncludeInClipboardHistory");
    if has_format(format) {
        unsafe {
            let Ok(handle) = GetClipboardData(format) else {
                return true;
            };
            let global = HGLOBAL(handle.0);
            let pointer = GlobalLock(global) as *const u32;
            if pointer.is_null() {
                return true;
            }
            let allowed = *pointer != 0;
            let _ = GlobalUnlock(global);
            return !allowed;
        }
    }
    false
}

fn source_process() -> String {
    unsafe {
        let mut pid = 0;
        if let Ok(owner) = GetClipboardOwner() {
            GetWindowThreadProcessId(owner, Some(&mut pid));
        }
        if pid == 0 {
            GetWindowThreadProcessId(GetForegroundWindow(), Some(&mut pid));
        }
        process_image_path(pid).map(|path| exe_name(&path).to_string()).unwrap_or_default()
    }
}

/// Captured clipboard content, ready for the data thread.
pub struct Capture {
    pub clip: NewClip,
    pub image: Option<Vec<u8>>,
}

pub struct CaptureLimits<'a> {
    pub max_item_bytes: u32,
    pub capture_images: bool,
    pub excluded_apps: &'a [String],
}

const MAX_IMAGE_BYTES: usize = 32 * 1024 * 1024;
/// Formatting bigger than this is left behind, and the text is kept plain.
const MAX_FORMAT_BYTES: usize = 4 * 1024 * 1024;
/// The formats that carry formatting, in Windows' own names.
const HTML_FORMAT: &str = "HTML Format";
const RTF_FORMAT: &str = "Rich Text Format";

/// One registered format's bytes, as given, if it's there and not too big. The clipboard must be
/// open.
unsafe fn read_format(format: u32) -> Option<Vec<u8>> {
    unsafe {
        if !has_format(format) {
            return None;
        }
        let handle = GetClipboardData(format).ok()?;
        let global = HGLOBAL(handle.0);
        let bytes = GlobalSize(global);
        if bytes == 0 || bytes > MAX_FORMAT_BYTES {
            return None;
        }
        let pointer = GlobalLock(global) as *const u8;
        if pointer.is_null() {
            return None;
        }
        let data = std::slice::from_raw_parts(pointer, bytes).to_vec();
        let _ = GlobalUnlock(global);
        Some(data)
    }
}

/// The formatting that came with copied text. The clipboard must be open.
unsafe fn read_formats() -> Formats {
    unsafe { Formats { html: read_format(registered_format(HTML_FORMAT)), rtf: read_format(registered_format(RTF_FORMAT)) } }
}

/// Reads the current clipboard if it qualifies. Called after `WM_CLIPBOARDUPDATE`. Content goes
/// straight to the data thread, and nothing here is ever logged.
pub fn capture(limits: &CaptureLimits<'_>) -> Option<Capture> {
    // Holding the clipboard first means the owner and the content can't change between the checks
    // and the read, so a copy can't be saved under another app's name.
    let _guard = ClipboardGuard::open()?;
    if is_own_write() {
        return None;
    }
    // An app can change the clipboard several times for one copy, and each change starts a capture.
    // The ones that open it after the first find the same content, already read.
    let sequence = unsafe { GetClipboardSequenceNumber() };
    if sequence != 0 && LAST_CAPTURE_SEQ.swap(sequence, Ordering::Relaxed) == sequence {
        return None;
    }
    let source = source_process();
    if Settings::is_app_excluded(limits.excluded_apps, &source) {
        return None;
    }
    if app_opted_out() {
        return None;
    }
    unsafe {
        if has_format(CF_HDROP.0 as u32) {
            let handle = GetClipboardData(CF_HDROP.0 as u32).ok()?;
            let drop = HDROP(handle.0);
            let count = DragQueryFileW(drop, 0xFFFF_FFFF, None);
            if count == 0 {
                return None;
            }
            let mut paths = Vec::with_capacity(count.min(64) as usize);
            for i in 0..count.min(64) {
                let mut buffer = vec![0u16; DragQueryFileW(drop, i, None) as usize + 1];
                let len = DragQueryFileW(drop, i, Some(&mut buffer));
                paths.push(String::from_utf16_lossy(&buffer[..len as usize]));
            }
            return Some(Capture { clip: NewClip::files(&paths, &source), image: None });
        }
        if has_format(CF_UNICODETEXT.0 as u32) {
            let handle = GetClipboardData(CF_UNICODETEXT.0 as u32).ok()?;
            let global = HGLOBAL(handle.0);
            let bytes = GlobalSize(global);
            // The size includes the terminator and can be rounded up, so this only rules out what
            // can't fit. The exact check on the text follows.
            if bytes == 0 || bytes > (limits.max_item_bytes as usize + 8) * 2 {
                return None; // too large to keep, and it was never copied out
            }
            let pointer = GlobalLock(global) as *const u16;
            if pointer.is_null() {
                return None;
            }
            let units = std::slice::from_raw_parts(pointer, bytes / 2);
            let len = units.iter().position(|&c| c == 0).unwrap_or(units.len());
            let text = String::from_utf16_lossy(&units[..len]);
            let _ = GlobalUnlock(global);
            if text.trim().is_empty() || text.len() > limits.max_item_bytes as usize {
                return None;
            }
            let clip = NewClip::text(text, &source).with_formats(read_formats());
            return Some(Capture { clip, image: None });
        }
        if limits.capture_images && has_format(CF_DIB.0 as u32) {
            let handle = GetClipboardData(CF_DIB.0 as u32).ok()?;
            let global = HGLOBAL(handle.0);
            let bytes = GlobalSize(global);
            if !(40..=MAX_IMAGE_BYTES).contains(&bytes) {
                return None;
            }
            let pointer = GlobalLock(global) as *const u8;
            if pointer.is_null() {
                return None;
            }
            let data = std::slice::from_raw_parts(pointer, bytes).to_vec();
            let _ = GlobalUnlock(global);
            let width = i32::from_le_bytes(data[4..8].try_into().ok()?).unsigned_abs();
            let height = i32::from_le_bytes(data[8..12].try_into().ok()?).unsigned_abs();
            let hash = fnv(&data);
            let clip = NewClip::image(String::new(), width, height, bytes as u64, hash, &source);
            return Some(Capture { clip, image: Some(data) });
        }
    }
    None
}

/// The clipboard's current content is something Orbit put there.
pub fn is_own_write() -> bool {
    unsafe { GetClipboardSequenceNumber() == OWN_WRITE_SEQ.load(Ordering::Relaxed) }
}

fn fnv(data: &[u8]) -> u64 {
    let mut hash = 0xcbf2_9ce4_8422_2325u64;
    for &byte in data {
        hash ^= byte as u64;
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    hash
}

unsafe fn set_global(format: u32, bytes: &[u8]) -> bool {
    unsafe {
        let Ok(global) = GlobalAlloc(GMEM_MOVEABLE, bytes.len()) else {
            return false;
        };
        let pointer = GlobalLock(global) as *mut u8;
        if pointer.is_null() {
            let _ = GlobalFree(Some(global));
            return false;
        }
        std::ptr::copy_nonoverlapping(bytes.as_ptr(), pointer, bytes.len());
        let _ = GlobalUnlock(global);
        if SetClipboardData(format, Some(HANDLE(global.0))).is_err() {
            let _ = GlobalFree(Some(global));
            return false;
        }
        true
    }
}

pub fn write_text(text: &str) -> bool {
    write_formatted(text, &Formats::default())
}

/// Text, with its formatting if it has any, so an app that reads formatting gets it.
pub fn write_formatted(text: &str, formats: &Formats) -> bool {
    let Some(guard) = ClipboardGuard::open() else {
        return false;
    };
    let mut bytes = Vec::with_capacity((text.len() + 1) * 2);
    for unit in text.encode_utf16() {
        bytes.extend_from_slice(&unit.to_le_bytes());
    }
    bytes.extend_from_slice(&[0, 0]);
    unsafe {
        if EmptyClipboard().is_err() {
            return false;
        }
        let ok = set_global(CF_UNICODETEXT.0 as u32, &bytes);
        // Plain text is what matters. Formatting that won't go on is left off.
        if ok {
            if let Some(html) = &formats.html {
                let _ = set_global(registered_format(HTML_FORMAT), html);
            }
            if let Some(rtf) = &formats.rtf {
                let _ = set_global(registered_format(RTF_FORMAT), rtf);
            }
        }
        guard.wrote();
        ok
    }
}

/// Puts a password or code on the clipboard, marked so that clipboard history, cloud clipboard and
/// tools like Orbit's own history leave it out. Gives back the sequence number it landed at, which
/// `clear_if_unchanged` needs.
pub fn write_secret(text: &str) -> Option<u32> {
    let guard = ClipboardGuard::open()?;
    let mut bytes = Vec::with_capacity((text.len() + 1) * 2);
    for unit in text.encode_utf16() {
        bytes.extend_from_slice(&unit.to_le_bytes());
    }
    bytes.extend_from_slice(&[0, 0]);
    let written = unsafe {
        if EmptyClipboard().is_err() {
            false
        } else {
            let ok = set_global(CF_UNICODETEXT.0 as u32, &bytes);
            // Windows documents these three formats for this purpose. A DWORD of 0 means "don't".
            let no = 0u32.to_le_bytes();
            let marked = set_global(registered_format("ExcludeClipboardContentFromMonitorProcessing"), &no)
                && set_global(registered_format("CanIncludeInClipboardHistory"), &no)
                && set_global(registered_format("CanUploadToCloudClipboard"), &no);
            guard.wrote();
            if ok && !marked {
                // Better no copy than one that history could keep.
                let _ = EmptyClipboard();
                false
            } else {
                ok
            }
        }
    };
    for byte in bytes.iter_mut() {
        unsafe { std::ptr::write_volatile(byte, 0) };
    }
    drop(guard);
    written.then(|| OWN_WRITE_SEQ.load(Ordering::Relaxed))
}

/// Empties the clipboard, unless something has been copied since `sequence`.
pub fn clear_if_unchanged(sequence: u32) -> bool {
    if unsafe { GetClipboardSequenceNumber() } != sequence {
        return false;
    }
    let Some(guard) = ClipboardGuard::open() else {
        return false;
    };
    unsafe {
        if GetClipboardSequenceNumber() != sequence || EmptyClipboard().is_err() {
            return false;
        }
        guard.wrote();
    }
    true
}

/// Puts a picture on the clipboard as a bitmap, and as a PNG too when there is one. Browsers and
/// apps built on them, such as Electron apps, paste the PNG and keep its transparency.
pub fn write_dib(data: &[u8], png: Option<&[u8]>) -> bool {
    let Some(guard) = ClipboardGuard::open() else {
        return false;
    };
    unsafe {
        if EmptyClipboard().is_err() {
            return false;
        }
        let ok = set_global(CF_DIB.0 as u32, data);
        if ok {
            if let Some(png) = png {
                let _ = set_global(registered_format("PNG"), png);
            }
        }
        guard.wrote();
        ok
    }
}

/// Pastes what's on the clipboard into `target` with Ctrl+V, once it's in front again and the
/// keys that chose the item are up. Runs on a short-lived thread. False means nothing was sent,
/// and the item is still on the clipboard to paste by hand.
pub fn paste_into(target: HWND) -> bool {
    if target.is_invalid() || !wait_for_foreground(target) || target_is_elevated(target) {
        return false;
    }
    if !wait_for_modifiers_released() || !still_focused(target) {
        return false;
    }
    press_ctrl_v()
}

/// Files go on the clipboard as a list, using the documented DROPFILES layout.
pub fn write_files(paths: &[&str]) -> bool {
    let Some(guard) = ClipboardGuard::open() else {
        return false;
    };
    let mut bytes = vec![0u8; 20];
    bytes[0] = 20; // pFiles: offset of the list
    bytes[16] = 1; // fWide: wide characters
    for path in paths {
        for unit in path.encode_utf16() {
            bytes.extend_from_slice(&unit.to_le_bytes());
        }
        bytes.extend_from_slice(&[0, 0]);
    }
    bytes.extend_from_slice(&[0, 0]);
    unsafe {
        if EmptyClipboard().is_err() {
            return false;
        }
        let ok = set_global(CF_HDROP.0 as u32, &bytes);
        guard.wrote();
        ok
    }
}

/// The clipboard's text, for use in a snippet. Content its owner marked as not to be kept, such
/// as a password manager's copy, reads as nothing.
pub fn read_text() -> Option<String> {
    let _guard = ClipboardGuard::open()?;
    if app_opted_out() {
        return None;
    }
    unsafe {
        if !has_format(CF_UNICODETEXT.0 as u32) {
            return None;
        }
        let handle = GetClipboardData(CF_UNICODETEXT.0 as u32).ok()?;
        let global = HGLOBAL(handle.0);
        let bytes = GlobalSize(global);
        let pointer = GlobalLock(global) as *const u16;
        if pointer.is_null() {
            return None;
        }
        let units = std::slice::from_raw_parts(pointer, bytes / 2);
        let len = units.iter().position(|&c| c == 0).unwrap_or(units.len());
        let text = String::from_utf16_lossy(&units[..len]);
        let _ = GlobalUnlock(global);
        Some(text)
    }
}

// --- Inserting text into the focused app ---------------------------------------------------

fn key(vk: u16, scan: u16, flags: KEYBD_EVENT_FLAGS) -> INPUT {
    INPUT {
        r#type: INPUT_KEYBOARD,
        Anonymous: INPUT_0 {
            ki: KEYBDINPUT { wVk: VIRTUAL_KEY(vk), wScan: scan, dwFlags: flags, time: 0, dwExtraInfo: INJECTED_TAG },
        },
    }
}

fn send(inputs: &[INPUT]) -> bool {
    unsafe { SendInput(inputs, std::mem::size_of::<INPUT>() as i32) as usize == inputs.len() }
}

/// Inserted text is typed, in paced chunks, so the clipboard stays untouched. Restoring a clipboard
/// can't be done safely: rich content and a password manager's opt-out don't survive it, and a busy
/// app can paste the old content after the restore. Longer text goes on the clipboard for the user
/// to paste, since typing it would take too long and hold the app's input queue.
const MAX_TYPED_UTF16: usize = 4 * 1024;
const TYPED_CHUNK_UTF16: usize = 128;
const CHUNK_PAUSE_MS: u64 = 10;

/// False if a modifier is still down after the wait: sending keys now would run shortcuts, and
/// a held Ctrl would make each Backspace delete a whole word.
fn wait_for_modifiers_released() -> bool {
    for _ in 0..80 {
        let held = [0x10, 0x11, 0x12, 0x5B, 0x5C]
            .iter()
            .any(|&vk| unsafe { GetAsyncKeyState(vk) } as u16 & 0x8000 != 0);
        if !held {
            return true;
        }
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    false
}

fn still_focused(target: HWND) -> bool {
    unsafe { GetForegroundWindow() == target }
}

fn wait_for_foreground(target: HWND) -> bool {
    for _ in 0..60 {
        if unsafe { GetForegroundWindow() } == target {
            return true;
        }
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    false
}

/// Types `text`. Between chunks it checks the keystrokes are still going to the right place, so
/// a focus change can stop them part way, and it returns false if that happens.
fn type_text(text: &str, target: HWND) -> bool {
    let mut inputs = Vec::with_capacity(TYPED_CHUNK_UTF16 * 2);
    let mut last_was_cr = false;
    let mut in_chunk = 0;
    for unit in text.encode_utf16() {
        // A surrogate pair must stay together, so a chunk never ends on a high surrogate.
        if in_chunk >= TYPED_CHUNK_UTF16 && !(0xDC00..0xE000).contains(&unit) {
            if !send(&inputs) {
                TYPING_STOPPED.store(1, Ordering::Relaxed);
                return false;
            }
            inputs.clear();
            in_chunk = 0;
            std::thread::sleep(std::time::Duration::from_millis(CHUNK_PAUSE_MS));
            if !still_focused(target) {
                TYPING_STOPPED.store(2, Ordering::Relaxed);
                return false;
            }
            if !wait_for_modifiers_released() {
                TYPING_STOPPED.store(3, Ordering::Relaxed);
                return false;
            }
        }
        in_chunk += 1;
        match unit {
            0x0A if last_was_cr => {}
            0x0A | 0x0D => {
                inputs.push(key(0x0D, 0, KEYBD_EVENT_FLAGS(0)));
                inputs.push(key(0x0D, 0, KEYEVENTF_KEYUP));
            }
            0x09 => {
                inputs.push(key(0x09, 0, KEYBD_EVENT_FLAGS(0)));
                inputs.push(key(0x09, 0, KEYEVENTF_KEYUP));
            }
            _ => {
                inputs.push(key(0, unit, KEYEVENTF_UNICODE));
                inputs.push(key(0, unit, KEYEVENTF_UNICODE | KEYEVENTF_KEYUP));
            }
        }
        last_was_cr = unit == 0x0D;
    }
    send(&inputs)
}

fn press(vk: u16, times: usize) -> bool {
    let mut inputs = Vec::with_capacity(times * 2);
    for _ in 0..times {
        inputs.push(key(vk, 0, KEYBD_EVENT_FLAGS(0)));
        inputs.push(key(vk, 0, KEYEVENTF_KEYUP));
    }
    send(&inputs)
}

fn press_ctrl_v() -> bool {
    send(&[
        key(0x11, 0, KEYBD_EVENT_FLAGS(0)),
        key(0x56, 0, KEYBD_EVENT_FLAGS(0)),
        key(0x56, 0, KEYEVENTF_KEYUP),
        key(0x11, 0, KEYEVENTF_KEYUP),
    ])
}

/// A process at a higher integrity level silently ignores injected input, and `SendInput`
/// gives no error for that, so this is checked first. Failure to inspect the target is
/// treated as "elevated", since that is the usual reason inspecting fails.
pub fn target_is_elevated(hwnd: HWND) -> bool {
    use windows::Win32::Foundation::CloseHandle;
    use windows::Win32::Security::{GetTokenInformation, TOKEN_ELEVATION, TOKEN_QUERY, TokenElevation};
    use windows::Win32::System::Threading::{GetCurrentProcess, OpenProcess, OpenProcessToken, PROCESS_QUERY_LIMITED_INFORMATION};
    unsafe {
        fn token_elevated(process: HANDLE) -> Option<bool> {
            unsafe {
                let mut token = HANDLE::default();
                OpenProcessToken(process, TOKEN_QUERY, &mut token).ok()?;
                let mut info = TOKEN_ELEVATION::default();
                let mut size = 0;
                let result = GetTokenInformation(
                    token,
                    TokenElevation,
                    Some(&mut info as *mut _ as *mut _),
                    std::mem::size_of::<TOKEN_ELEVATION>() as u32,
                    &mut size,
                );
                let _ = CloseHandle(token);
                result.ok()?;
                Some(info.TokenIsElevated != 0)
            }
        }
        if token_elevated(GetCurrentProcess()) == Some(true) {
            return false;
        }
        let mut pid = 0;
        GetWindowThreadProcessId(hwnd, Some(&mut pid));
        if pid == 0 {
            return false;
        }
        let Ok(process) = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid) else {
            return true;
        };
        let elevated = token_elevated(process).unwrap_or(true);
        let _ = CloseHandle(process);
        elevated
    }
}

pub struct InsertRequest {
    pub target: HWND,
    pub text: String,
    pub erase_chars: usize,
    pub cursor_back: Option<usize>,
    /// Always go through the clipboard and leave the text there. For clipboard history paste.
    pub keep_on_clipboard: bool,
    /// Formatting to paste with the text. Only used through the clipboard.
    pub formats: Formats,
}

/// Result reported back to the launcher thread, so the user always learns what happened.
pub enum InsertOutcome {
    Inserted,
    /// Could not reach the target. The text is on the clipboard for a manual paste.
    CopiedInstead,
    /// Too long to type. The text is on the clipboard for a manual paste.
    TooLongToType,
    /// Nothing was sent, because the focused field is or might be a password field.
    /// `reported` says UI Automation answered yes, rather than failing to answer.
    PasswordField { reported: bool },
    Failed,
}

/// How long the last password check took, for the benchmark log.
pub static PASSWORD_CHECK_TICKS: std::sync::atomic::AtomicI64 = std::sync::atomic::AtomicI64::new(0);

/// Why typing last stopped part way, for the benchmark log: 1 Windows refused the keys, 2 focus
/// moved, 3 a modifier stayed down.
pub static TYPING_STOPPED: AtomicU32 = AtomicU32::new(0);

fn copy_instead(request: &InsertRequest) -> InsertOutcome {
    if write_formatted(&request.text, &request.formats) { InsertOutcome::CopiedInstead } else { InsertOutcome::Failed }
}

/// The focused control's own style, which only means this on a real edit control.
fn focused_edit_is_password(foreground: HWND) -> bool {
    use windows::Win32::UI::WindowsAndMessaging::{
        ES_PASSWORD, GUITHREADINFO, GWL_STYLE, GetClassNameW, GetGUIThreadInfo, GetWindowLongW,
    };
    unsafe {
        let thread = GetWindowThreadProcessId(foreground, None);
        let mut info = GUITHREADINFO { cbSize: std::mem::size_of::<GUITHREADINFO>() as u32, ..Default::default() };
        if GetGUIThreadInfo(thread, &mut info).is_err() || info.hwndFocus.is_invalid() {
            return false;
        }
        let mut class = [0u16; 64];
        let length = GetClassNameW(info.hwndFocus, &mut class).max(0) as usize;
        let is_edit = String::from_utf16_lossy(&class[..length]).to_ascii_lowercase().contains("edit");
        is_edit && GetWindowLongW(info.hwndFocus, GWL_STYLE) as u32 & ES_PASSWORD as u32 != 0
    }
}

/// Asks UI Automation, which also knows about fields inside browsers and other toolkits that have
/// no window of their own. `None` means it couldn't say. This loads a large system library, which
/// is why it runs only at the moment a trigger has been typed, on this short-lived thread.
fn automation_says_password() -> Option<bool> {
    use windows::Win32::System::Com::{
        CLSCTX_INPROC_SERVER, COINIT_MULTITHREADED, CoCreateInstance, CoInitializeEx, CoUninitialize,
    };
    use windows::Win32::UI::Accessibility::{CUIAutomation, IUIAutomation, IUIAutomation2};
    use windows::core::Interface;
    const ANSWER_TIMEOUT_MS: u32 = 700;
    unsafe {
        let initialized = CoInitializeEx(None, COINIT_MULTITHREADED).is_ok();
        let answer = (|| {
            let automation: IUIAutomation = CoCreateInstance(&CUIAutomation, None, CLSCTX_INPROC_SERVER).ok()?;
            if let Ok(with_timeouts) = automation.cast::<IUIAutomation2>() {
                let _ = with_timeouts.SetConnectionTimeout(ANSWER_TIMEOUT_MS);
                let _ = with_timeouts.SetTransactionTimeout(ANSWER_TIMEOUT_MS);
            }
            let focused = automation.GetFocusedElement().ok()?;
            focused.CurrentIsPassword().ok().map(|value| value.as_bool())
        })();
        if initialized {
            CoUninitialize();
        }
        answer
    }
}

/// Runs on a short-lived thread, since the waits here must not block the launcher.
pub fn insert(request: InsertRequest) -> InsertOutcome {
    let target = request.target;
    if !wait_for_foreground(target) || target_is_elevated(target) {
        return copy_instead(&request);
    }
    // Erasing a trigger means Orbit is acting on what was typed, rather than on a choice the user
    // made in the launcher, so it needs to be sure this isn't a password. If it can't tell, it stops.
    if request.erase_chars > 0 {
        let t = crate::util::qpc();
        let answer = automation_says_password();
        PASSWORD_CHECK_TICKS.store(crate::util::qpc() - t, Ordering::Relaxed);
        match answer {
            Some(true) => return InsertOutcome::PasswordField { reported: true },
            None => return InsertOutcome::PasswordField { reported: false },
            Some(false) if focused_edit_is_password(target) => return InsertOutcome::PasswordField { reported: false },
            Some(false) => {}
        }
    }
    let units = request.text.encode_utf16().count();
    if !request.keep_on_clipboard && units > MAX_TYPED_UTF16 {
        // Nothing has been erased yet, so nothing is lost if the user doesn't paste.
        return if write_text(&request.text) { InsertOutcome::TooLongToType } else { InsertOutcome::Failed };
    }
    if !wait_for_modifiers_released() || !still_focused(target) {
        return copy_instead(&request);
    }
    if request.erase_chars > 0 && !press(0x08, request.erase_chars) {
        return copy_instead(&request);
    }
    if !still_focused(target) {
        return copy_instead(&request);
    }
    if request.keep_on_clipboard {
        if !write_formatted(&request.text, &request.formats) {
            return InsertOutcome::Failed;
        }
        if !still_focused(target) || !press_ctrl_v() {
            return InsertOutcome::CopiedInstead;
        }
    } else if !type_text(&request.text, target) {
        return copy_instead(&request);
    }
    if let Some(back) = request.cursor_back {
        if back > 0 && still_focused(target) {
            press(0x25, back);
        }
    }
    InsertOutcome::Inserted
}

#[cfg(test)]
mod tests {
    use super::*;
    use windows::Win32::System::StationsAndDesktops::{
        CreateDesktopW, CreateWindowStationW, DESKTOP_CONTROL_FLAGS, HDESK, SetProcessWindowStation, SetThreadDesktop,
    };

    /// Windows lets a thread open the clipboard while another thread of the same process still has it
    /// open, and two threads reading its text at once corrupt the heap. Orbit crashed that way
    /// (0xc0000374, with two `orbit-clip` threads inside GetClipboardData), because each clipboard
    /// change starts a capture on a thread of its own. This reads from four threads for a few seconds,
    /// which crashed every run without the lock. It runs in a private window station, which has a
    /// clipboard of its own, so the real clipboard is never read or written. Moving there changes the
    /// whole test process, so it's ignored and run alone:
    /// `cargo test -p orbit-windows reads_from_many_threads -- --ignored`
    #[test]
    #[ignore]
    fn reads_from_many_threads_at_once_dont_corrupt_the_heap() {
        const TEXT: &str = "Some text someone copied at work, long enough to look like a sentence.";
        const WINSTA_ALL_ACCESS: u32 = 0x37F;
        const GENERIC_ALL: u32 = 0x1000_0000;
        let desktop = unsafe {
            let station = CreateWindowStationW(PCWSTR::null(), 0, WINSTA_ALL_ACCESS, None).expect("a private window station");
            SetProcessWindowStation(station).expect("the process moved to it");
            let name = wide("orbit-clipboard-test");
            CreateDesktopW(PCWSTR(name.as_ptr()), PCWSTR::null(), None, DESKTOP_CONTROL_FLAGS(0), GENERIC_ALL, None)
                .expect("a desktop on it")
        };
        // A thread joins the test desktop before it touches anything of the window station's.
        let address = desktop.0 as usize;
        let join_desktop = move || unsafe { SetThreadDesktop(HDESK(address as *mut _)).expect("thread on the test desktop") };
        std::thread::spawn(move || {
            join_desktop();
            assert!(write_text(TEXT));
        })
        .join()
        .unwrap();
        let until = std::time::Instant::now() + std::time::Duration::from_secs(4);
        let readers: Vec<_> = (0..4)
            .map(|_| {
                std::thread::spawn(move || {
                    join_desktop();
                    let mut reads = 0u32;
                    while std::time::Instant::now() < until {
                        if let Some(text) = read_text() {
                            assert_eq!(text, TEXT);
                            reads += 1;
                        }
                    }
                    reads
                })
            })
            .collect();
        let reads: u32 = readers.into_iter().map(|reader| reader.join().unwrap()).sum();
        assert!(reads > 1000, "only {reads} reads in 4 s");
    }
}
