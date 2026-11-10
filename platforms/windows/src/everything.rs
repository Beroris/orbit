//! File search through voidtools' Everything, which keeps an index of every file name on the PC.
//!
//! Orbit asks the running Everything directly, the way voidtools' own SDK does: a `WM_COPYDATA`
//! query to Everything's hidden window, answered with another `WM_COPYDATA` to a window of ours.
//! No DLL is shipped or loaded. Everything 1.4 and 1.5 both answer this. The layouts are from
//! voidtools' `everything_ipc.h`.
//!
//! Queries run on the files thread (`files.rs`), never the UI thread, because the answer comes back
//! as a message that has to be waited for.

use crate::util::wide;
use std::os::windows::process::CommandExt;
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};
use windows::Win32::Foundation::{HWND, LPARAM, WPARAM};
use windows::Win32::System::DataExchange::COPYDATASTRUCT;
use windows::Win32::UI::WindowsAndMessaging::{
    DispatchMessageW, FindWindowW, MSG, MsgWaitForMultipleObjects, PM_REMOVE, PeekMessageW, QS_ALLINPUT, SMTO_ABORTIFHUNG, SMTO_BLOCK,
    SendMessageTimeoutW, WM_COPYDATA, WM_USER,
};
use windows::core::PCWSTR;

const CREATE_NO_WINDOW: u32 = 0x0800_0000;
pub const WINGET_ID: &str = "voidtools.Everything";
pub const WEBSITE: &str = "https://www.voidtools.com/";

/// Everything's window. 1.5 adds its instance name in brackets.
const WINDOW_CLASSES: &[&str] = &["EVERYTHING_TASKBAR_NOTIFICATION", "EVERYTHING_TASKBAR_NOTIFICATION_(1.5a)"];
const EVERYTHING_WM_IPC: u32 = WM_USER;
const IPC_IS_DB_LOADED: usize = 401;
const COPYDATA_QUERY2W: usize = 18;
/// Tells Everything a file was opened, so its run count goes up.
const COPYDATA_INC_RUN_COUNTW: usize = 24;

const REQUEST_FILE_NAME: u32 = 0x0000_0001;
const REQUEST_PATH: u32 = 0x0000_0002;
const REQUEST_SIZE: u32 = 0x0000_0010;
const REQUEST_DATE_MODIFIED: u32 = 0x0000_0040;
const ITEM_FOLDER: u32 = 0x1;
const ITEM_DRIVE: u32 = 0x2;

const SORT_NAME_ASCENDING: u32 = 1;
const SORT_DATE_MODIFIED_DESCENDING: u32 = 14;
const SORT_RUN_COUNT_DESCENDING: u32 = 20;

/// Long enough for a big index on a slow disk, short enough that a hung Everything isn't waited on.
const ANSWER_TIMEOUT: Duration = Duration::from_millis(2500);
/// The message number our window expects the answer under. Any number works; this one is ours.
pub const REPLY_ID: usize = 0x4F52_4246;

/// One file or folder Everything found.
#[derive(Clone, Debug, PartialEq)]
pub struct FileHit {
    pub name: String,
    /// The folder it's in.
    pub folder: String,
    pub is_folder: bool,
    pub size: Option<u64>,
    /// Unix seconds.
    pub modified: Option<i64>,
}

impl FileHit {
    pub fn full_path(&self) -> String {
        if self.folder.is_empty() {
            self.name.clone()
        } else if self.folder.ends_with('\\') {
            format!("{}{}", self.folder, self.name)
        } else {
            format!("{}\\{}", self.folder, self.name)
        }
    }
}

/// Why there are no files to show, in terms of what the user can do about it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Problem {
    NotInstalled,
    /// Installed, but not running.
    NotRunning,
    /// Running, but still reading the disks for the first time.
    Loading,
    /// It didn't answer in time.
    NoAnswer,
}

impl Problem {
    pub fn title(&self) -> &'static str {
        match self {
            Problem::NotInstalled => "Everything isn't installed",
            Problem::NotRunning => "Everything isn't running",
            Problem::Loading => "Everything is still indexing",
            Problem::NoAnswer => "Everything didn't answer",
        }
    }

    pub fn detail(&self) -> &'static str {
        match self {
            Problem::NotInstalled => "Orbit finds files with Everything, free from voidtools. Install it to search every file by name.",
            Problem::NotRunning => "Start it, and Orbit can search your files.",
            Problem::Loading => "It's reading your drives for the first time. This takes a moment.",
            Problem::NoAnswer => "It may be busy. Try again in a moment.",
        }
    }
}

pub fn window() -> Option<HWND> {
    for class in WINDOW_CLASSES {
        let name = wide(class);
        if let Ok(window) = unsafe { FindWindowW(PCWSTR(name.as_ptr()), PCWSTR::null()) } {
            if !window.is_invalid() {
                return Some(window);
            }
        }
    }
    None
}

fn ask(window: HWND, what: usize) -> Option<usize> {
    let mut result = 0usize;
    let sent = unsafe { SendMessageTimeoutW(window, EVERYTHING_WM_IPC, WPARAM(what), LPARAM(0), SMTO_ABORTIFHUNG | SMTO_BLOCK, 1000, Some(&mut result)) };
    (sent.0 != 0).then_some(result)
}

/// Whether Everything is there and ready. Cheap: a window lookup and, if found, one message.
pub fn check() -> Result<HWND, Problem> {
    let Some(window) = window() else {
        return Err(if find_exe().is_some() { Problem::NotRunning } else { Problem::NotInstalled });
    };
    match ask(window, IPC_IS_DB_LOADED) {
        Some(0) => Err(Problem::Loading),
        Some(_) => Ok(window),
        None => Err(Problem::NoAnswer),
    }
}

fn sort_number(sort: orbit_core::settings::FileSort) -> u32 {
    use orbit_core::settings::FileSort;
    match sort {
        FileSort::Recent => SORT_DATE_MODIFIED_DESCENDING,
        FileSort::MostOpened => SORT_RUN_COUNT_DESCENDING,
        FileSort::Name => SORT_NAME_ASCENDING,
    }
}

/// The query as Everything takes it: seven numbers, then the search text.
fn query_bytes(reply_to: HWND, text: &str, max: u32, sort: u32) -> Vec<u8> {
    let fields: [u32; 7] = [
        reply_to.0 as usize as u32,
        REPLY_ID as u32,
        0, // search flags: no case, no whole word, no path, no regex
        0, // offset
        max,
        REQUEST_FILE_NAME | REQUEST_PATH | REQUEST_SIZE | REQUEST_DATE_MODIFIED,
        sort,
    ];
    let mut bytes = Vec::with_capacity(28 + (text.len() + 1) * 2);
    for field in fields {
        bytes.extend_from_slice(&field.to_le_bytes());
    }
    for unit in text.encode_utf16().chain(std::iter::once(0)) {
        bytes.extend_from_slice(&unit.to_le_bytes());
    }
    bytes
}

/// Windows file time (100 ns since 1601) as Unix seconds.
fn unix_from_filetime(value: u64) -> Option<i64> {
    const SECONDS_TO_1970: i64 = 11_644_473_600;
    (value != 0 && value != u64::MAX).then(|| (value / 10_000_000) as i64 - SECONDS_TO_1970)
}

/// Reads Everything's answer: five numbers, a list of (flags, offset) pairs, and each item's fields
/// at its offset from the start, in the order of the request bits. Anything out of bounds stops the
/// reading there rather than trusting it.
pub fn parse_list(data: &[u8]) -> Option<Vec<FileHit>> {
    let number = |at: usize| -> Option<u32> { data.get(at..at + 4).map(|b| u32::from_le_bytes([b[0], b[1], b[2], b[3]])) };
    let count = number(4)? as usize;
    let requested = number(12)?;
    let mut hits = Vec::with_capacity(count.min(1000));
    for index in 0..count {
        let item = 20 + index * 8;
        let flags = number(item)?;
        let mut at = number(item + 4)? as usize;
        let text = |at: &mut usize| -> Option<String> {
            let length = number(*at)? as usize;
            let start = *at + 4;
            let end = start.checked_add(length.checked_mul(2)?)?;
            let units: Vec<u16> = data.get(start..end)?.chunks_exact(2).map(|b| u16::from_le_bytes([b[0], b[1]])).collect();
            *at = end + 2; // and its terminating zero
            Some(String::from_utf16_lossy(&units))
        };
        let wide_number = |at: &mut usize| -> Option<u64> {
            let bytes = data.get(*at..*at + 8)?;
            *at += 8;
            Some(u64::from_le_bytes(bytes.try_into().ok()?))
        };
        let name = if requested & REQUEST_FILE_NAME != 0 { text(&mut at)? } else { String::new() };
        let folder = if requested & REQUEST_PATH != 0 { text(&mut at)? } else { String::new() };
        let size = if requested & REQUEST_SIZE != 0 { Some(wide_number(&mut at)?) } else { None };
        let modified = if requested & REQUEST_DATE_MODIFIED != 0 { unix_from_filetime(wide_number(&mut at)?) } else { None };
        let is_folder = flags & (ITEM_FOLDER | ITEM_DRIVE) != 0;
        hits.push(FileHit { name, folder, is_folder, size: if is_folder { None } else { size.filter(|&s| s != u64::MAX) }, modified });
    }
    Some(hits)
}

thread_local! {
    /// The answer, copied out of the message it came in, which is only valid while it's handled.
    static ANSWER: std::cell::RefCell<Option<Vec<u8>>> = const { std::cell::RefCell::new(None) };
}

/// For the reply window's procedure: keeps Everything's answer. True if it was one.
pub fn take_reply(lparam: LPARAM) -> bool {
    let copy = unsafe { &*(lparam.0 as *const COPYDATASTRUCT) };
    if copy.dwData != REPLY_ID || copy.lpData.is_null() {
        return false;
    }
    let bytes = unsafe { std::slice::from_raw_parts(copy.lpData as *const u8, copy.cbData as usize) }.to_vec();
    ANSWER.with(|answer| *answer.borrow_mut() = Some(bytes));
    true
}

/// Searches. Must run on the thread that owns `reply_to`, which must hand `WM_COPYDATA` to
/// `take_reply`. Everything answers during the send or soon after, so messages are pumped until it
/// has, or until the wait is over.
pub fn search(reply_to: HWND, text: &str, max: u32, sort: orbit_core::settings::FileSort) -> Result<Vec<FileHit>, Problem> {
    let window = check()?;
    ANSWER.with(|answer| answer.borrow_mut().take());
    let bytes = query_bytes(reply_to, text, max, sort_number(sort));
    let copy = COPYDATASTRUCT { dwData: COPYDATA_QUERY2W, cbData: bytes.len() as u32, lpData: bytes.as_ptr() as *mut _ };
    let mut accepted = 0usize;
    let sent = unsafe {
        SendMessageTimeoutW(window, WM_COPYDATA, WPARAM(reply_to.0 as usize), LPARAM(&copy as *const _ as isize), SMTO_ABORTIFHUNG | SMTO_BLOCK, 1500, Some(&mut accepted))
    };
    if sent.0 == 0 || accepted == 0 {
        return Err(Problem::NoAnswer);
    }
    let deadline = Instant::now() + ANSWER_TIMEOUT;
    loop {
        if let Some(answer) = ANSWER.with(|answer| answer.borrow_mut().take()) {
            return parse_list(&answer).ok_or(Problem::NoAnswer);
        }
        let left = deadline.saturating_duration_since(Instant::now());
        if left.is_zero() {
            return Err(Problem::NoAnswer);
        }
        unsafe {
            MsgWaitForMultipleObjects(None, false, left.as_millis() as u32, QS_ALLINPUT);
            let mut message = MSG::default();
            while PeekMessageW(&mut message, None, 0, 0, PM_REMOVE).as_bool() {
                DispatchMessageW(&message);
            }
        }
    }
}

/// Tells Everything a file was opened from Orbit, so "most opened" learns from Orbit too.
pub fn note_opened(reply_to: HWND, path: &str) {
    let Some(window) = window() else { return };
    let text: Vec<u8> = path.encode_utf16().chain(std::iter::once(0)).flat_map(u16::to_le_bytes).collect();
    let copy = COPYDATASTRUCT { dwData: COPYDATA_INC_RUN_COUNTW, cbData: text.len() as u32, lpData: text.as_ptr() as *mut _ };
    let mut result = 0usize;
    unsafe {
        let _ = SendMessageTimeoutW(window, WM_COPYDATA, WPARAM(reply_to.0 as usize), LPARAM(&copy as *const _ as isize), SMTO_ABORTIFHUNG | SMTO_BLOCK, 500, Some(&mut result));
    }
}

/// Everything.exe, from its uninstall entry or the usual places.
pub fn find_exe() -> Option<PathBuf> {
    let mut places: Vec<PathBuf> = Vec::new();
    for root in [windows::Win32::System::Registry::HKEY_LOCAL_MACHINE, windows::Win32::System::Registry::HKEY_CURRENT_USER] {
        if let Some(folder) = crate::install::read_string(root, r"Software\Microsoft\Windows\CurrentVersion\Uninstall\Everything", "InstallLocation") {
            places.push(PathBuf::from(folder.trim_matches('"')).join("Everything.exe"));
        }
        if let Some(icon) = crate::install::read_string(root, r"Software\Microsoft\Windows\CurrentVersion\App Paths\Everything.exe", "") {
            places.push(PathBuf::from(icon.trim_matches('"')));
        }
    }
    for variable in ["ProgramFiles", "ProgramFiles(x86)", "ProgramW6432"] {
        if let Some(folder) = std::env::var_os(variable) {
            places.push(PathBuf::from(folder).join("Everything").join("Everything.exe"));
        }
    }
    for variable in ["ProgramFiles", "ProgramW6432"] {
        if let Some(folder) = std::env::var_os(variable) {
            places.push(PathBuf::from(&folder).join("Everything 1.5a").join("Everything.exe"));
            places.push(PathBuf::from(folder).join("Everything 1.5a").join("Everything64.exe"));
        }
    }
    if let Some(local) = std::env::var_os("LOCALAPPDATA") {
        places.push(PathBuf::from(&local).join("Programs").join("Everything").join("Everything.exe"));
        places.push(PathBuf::from(local).join("Everything").join("Everything.exe"));
    }
    places.into_iter().find(|place| place.is_file())
}

/// Starts Everything in the background, in the notification area, as it starts at sign-in.
pub fn start() -> bool {
    let Some(exe) = find_exe() else { return false };
    Command::new(exe).arg("-startup").stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null()).spawn().is_ok()
}

/// Installs Everything with Windows Package Manager. Its installer asks Windows for permission,
/// because Everything reads the drives' file tables, which needs an administrator once.
pub fn install() -> Result<(), String> {
    let Some(winget) = crate::integrations::winget() else {
        return Err("Windows Package Manager isn't available. Everything can be downloaded from voidtools.com.".into());
    };
    let args = ["install", "--id", WINGET_ID, "-e", "--accept-source-agreements", "--accept-package-agreements", "--disable-interactivity"];
    let ran = Command::new(&winget).args(args).stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null()).creation_flags(CREATE_NO_WINDOW).status();
    match (ran, find_exe()) {
        (_, Some(_)) => {
            if window().is_none() {
                start();
            }
            Ok(())
        }
        _ => Err("Windows Package Manager couldn't install Everything. Check the connection and try again.".into()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn string(out: &mut Vec<u8>, text: &str) {
        let units: Vec<u16> = text.encode_utf16().collect();
        out.extend_from_slice(&(units.len() as u32).to_le_bytes());
        for unit in units.iter().chain(std::iter::once(&0)) {
            out.extend_from_slice(&unit.to_le_bytes());
        }
    }

    /// An answer laid out as Everything lays it out.
    fn answer(items: &[(u32, &str, &str, u64, u64)]) -> Vec<u8> {
        let requested = REQUEST_FILE_NAME | REQUEST_PATH | REQUEST_SIZE | REQUEST_DATE_MODIFIED;
        let mut head = Vec::new();
        for number in [items.len() as u32 * 10, items.len() as u32, 0, requested, SORT_DATE_MODIFIED_DESCENDING] {
            head.extend_from_slice(&number.to_le_bytes());
        }
        let mut bodies = Vec::new();
        let first = 20 + items.len() * 8;
        for (flags, name, folder, size, modified) in items {
            let offset = first + bodies.len();
            head.extend_from_slice(&flags.to_le_bytes());
            head.extend_from_slice(&(offset as u32).to_le_bytes());
            string(&mut bodies, name);
            string(&mut bodies, folder);
            bodies.extend_from_slice(&size.to_le_bytes());
            bodies.extend_from_slice(&modified.to_le_bytes());
        }
        head.extend(bodies);
        head
    }

    #[test]
    fn an_answer_is_read() {
        // 2026-11-09 00:00:00 UTC as a Windows file time.
        let modified = (1_794_182_400u64 + 11_644_473_600) * 10_000_000;
        let data = answer(&[(0, "report.pdf", "C:\\Users\\a\\Documents", 2048, modified), (ITEM_FOLDER, "Projects", "D:\\", u64::MAX, 0), (ITEM_DRIVE, "C:", "", 0, 0)]);
        let hits = parse_list(&data).unwrap();
        assert_eq!(hits.len(), 3);
        assert_eq!(hits[0].name, "report.pdf");
        assert_eq!(hits[0].full_path(), "C:\\Users\\a\\Documents\\report.pdf");
        assert_eq!(hits[0].size, Some(2048));
        assert_eq!(hits[0].modified, Some(1_794_182_400));
        assert!(!hits[0].is_folder);
        assert!(hits[1].is_folder);
        assert_eq!(hits[1].size, None, "folders have no size");
        assert_eq!(hits[1].full_path(), "D:\\Projects", "no doubled backslash after a drive");
        assert_eq!(hits[2].full_path(), "C:", "a drive is its own path");
    }

    #[test]
    fn a_short_or_lying_answer_is_not_trusted() {
        let data = answer(&[(0, "a.txt", "C:\\", 1, 0)]);
        for cut in [0, 10, 24, data.len() - 9] {
            assert!(parse_list(&data[..cut]).is_none_or(|hits| hits.is_empty()), "cut at {cut}");
        }
        let mut lying = data.clone();
        lying[4..8].copy_from_slice(&1000u32.to_le_bytes());
        assert_eq!(parse_list(&lying), None, "more items than are there");
        let mut huge = data;
        let at = 20 + 8;
        huge[at..at + 4].copy_from_slice(&u32::MAX.to_le_bytes());
        assert_eq!(parse_list(&huge), None, "a name longer than the answer");
    }

    #[test]
    fn a_query_is_laid_out_as_everything_reads_it() {
        let bytes = query_bytes(HWND(0x1234 as *mut _), "ext:pdf", 50, SORT_DATE_MODIFIED_DESCENDING);
        assert_eq!(&bytes[0..4], &0x1234u32.to_le_bytes());
        assert_eq!(&bytes[4..8], &(REPLY_ID as u32).to_le_bytes());
        assert_eq!(&bytes[16..20], &50u32.to_le_bytes());
        assert_eq!(&bytes[24..28], &SORT_DATE_MODIFIED_DESCENDING.to_le_bytes());
        assert_eq!(bytes.len(), 28 + ("ext:pdf".len() + 1) * 2);
        assert_eq!(&bytes[bytes.len() - 2..], &[0, 0], "the text ends with a zero");
    }
}
