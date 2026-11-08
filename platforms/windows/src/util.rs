//! Small Win32 helpers shared across the host.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicI64, Ordering};
use windows::Win32::Foundation::{CloseHandle, HWND, LPARAM, MAX_PATH, WPARAM};
use windows::Win32::System::Performance::{QueryPerformanceCounter, QueryPerformanceFrequency};
use windows::Win32::System::ProcessStatus::{GetProcessMemoryInfo, PROCESS_MEMORY_COUNTERS_EX};
use windows::Win32::System::Threading::{
    GetCurrentProcess, OpenProcess, PROCESS_NAME_WIN32, PROCESS_QUERY_LIMITED_INFORMATION,
    QueryFullProcessImageNameW,
};
use windows::Win32::UI::WindowsAndMessaging::PostMessageW;
use windows::core::PWSTR;

pub fn wide(text: &str) -> Vec<u16> {
    text.encode_utf16().chain(std::iter::once(0)).collect()
}

pub fn wide_path(path: &Path) -> Vec<u16> {
    use std::os::windows::ffi::OsStrExt;
    path.as_os_str().encode_wide().chain(std::iter::once(0)).collect()
}

/// A `HWND` that can cross threads. Window handles are process-global values, and every API
/// used through this wrapper (posting messages) is documented as safe from any thread.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct SendHwnd(pub isize);
unsafe impl Send for SendHwnd {}
unsafe impl Sync for SendHwnd {}

impl SendHwnd {
    pub fn new(hwnd: HWND) -> Self {
        SendHwnd(hwnd.0 as isize)
    }
    pub fn get(self) -> HWND {
        HWND(self.0 as *mut _)
    }
    pub fn post(self, message: u32, wparam: usize, lparam: isize) {
        unsafe {
            let _ = PostMessageW(Some(self.get()), message, WPARAM(wparam), LPARAM(lparam));
        }
    }
}

static QPC_FREQ: AtomicI64 = AtomicI64::new(0);

#[inline]
pub fn qpc() -> i64 {
    let mut value = 0;
    unsafe {
        let _ = QueryPerformanceCounter(&mut value);
    }
    value
}

pub fn qpc_frequency() -> i64 {
    let cached = QPC_FREQ.load(Ordering::Relaxed);
    if cached != 0 {
        return cached;
    }
    let mut value = 0;
    unsafe {
        let _ = QueryPerformanceFrequency(&mut value);
    }
    QPC_FREQ.store(value, Ordering::Relaxed);
    value
}

#[derive(Clone, Copy, Debug, Default)]
pub struct MemoryInfo {
    pub private_bytes: usize,
    pub working_set: usize,
    pub peak_working_set: usize,
}

pub fn process_memory() -> MemoryInfo {
    let mut counters = PROCESS_MEMORY_COUNTERS_EX::default();
    counters.cb = std::mem::size_of::<PROCESS_MEMORY_COUNTERS_EX>() as u32;
    unsafe {
        let _ = GetProcessMemoryInfo(
            GetCurrentProcess(),
            &mut counters as *mut _ as *mut _,
            counters.cb,
        );
    }
    MemoryInfo {
        private_bytes: counters.PrivateUsage,
        working_set: counters.WorkingSetSize,
        peak_working_set: counters.PeakWorkingSetSize,
    }
}

/// Full image path of a process, or `None` if it is protected or gone.
pub fn process_image_path(pid: u32) -> Option<String> {
    unsafe {
        let handle = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid).ok()?;
        let mut buffer = [0u16; MAX_PATH as usize * 4];
        let mut size = buffer.len() as u32;
        let result = QueryFullProcessImageNameW(
            handle,
            PROCESS_NAME_WIN32,
            PWSTR(buffer.as_mut_ptr()),
            &mut size,
        );
        let _ = CloseHandle(handle);
        result.ok()?;
        Some(String::from_utf16_lossy(&buffer[..size as usize]))
    }
}

pub fn exe_name(path: &str) -> &str {
    path.rsplit(['\\', '/']).next().unwrap_or(path)
}

pub fn default_data_dir() -> PathBuf {
    let base = std::env::var_os("LOCALAPPDATA")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("USERPROFILE").map(|p| PathBuf::from(p).join("AppData\\Local")))
        .unwrap_or_else(std::env::temp_dir);
    base.join("Orbit")
}

/// Writes through a temporary file and renames, so a crash leaves the old or the new content
/// and never a partial file.
pub fn write_atomic(path: &Path, content: &[u8]) -> std::io::Result<()> {
    let tmp = path.with_extension("tmp");
    std::fs::write(&tmp, content)?;
    std::fs::rename(&tmp, path)
}

pub fn format_bytes(bytes: usize) -> String {
    let mib = bytes as f64 / (1024.0 * 1024.0);
    if mib >= 1.0 {
        format!("{mib:.1} MB")
    } else if bytes >= 1024 {
        format!("{:.0} KB", bytes as f64 / 1024.0)
    } else {
        format!("{bytes} bytes")
    }
}

/// A moment as the user's clock showed it.
fn local_system_time(unix: i64) -> Option<windows::Win32::Foundation::SYSTEMTIME> {
    use windows::Win32::Foundation::{FILETIME, SYSTEMTIME};
    use windows::Win32::System::Time::{FileTimeToSystemTime, SystemTimeToTzSpecificLocalTime};
    let ticks = unix.checked_add(11_644_473_600)?.checked_mul(10_000_000)?;
    let ticks = u64::try_from(ticks).ok()?;
    let file = FILETIME { dwLowDateTime: ticks as u32, dwHighDateTime: (ticks >> 32) as u32 };
    let (mut utc, mut local) = (SYSTEMTIME::default(), SYSTEMTIME::default());
    unsafe {
        FileTimeToSystemTime(&file, &mut utc).ok()?;
        SystemTimeToTzSpecificLocalTime(None, &utc, &mut local).ok()?;
    }
    Some(local)
}

/// When something happened, such as `Today at 10:31`, with the time in the user's own format.
pub fn when_label(unix: i64, now: i64) -> String {
    use windows::Win32::Globalization::{GetTimeFormatEx, TIME_NOSECONDS};
    let (Some(then), Some(today)) = (local_system_time(unix), local_system_time(now)) else { return String::new() };
    let mut buffer = [0u16; 64];
    let written = unsafe { GetTimeFormatEx(windows::core::PCWSTR::null(), TIME_NOSECONDS, Some(&then), windows::core::PCWSTR::null(), Some(&mut buffer)) };
    let time = if written > 1 { String::from_utf16_lossy(&buffer[..written as usize - 1]) } else { format!("{:02}:{:02}", then.wHour, then.wMinute) };
    let day = |t: &windows::Win32::Foundation::SYSTEMTIME| (t.wYear, t.wMonth, t.wDay);
    if day(&then) == day(&today) {
        format!("Today at {time}")
    } else if local_system_time(now - 86_400).is_some_and(|yesterday| day(&yesterday) == day(&then)) {
        format!("Yesterday at {time}")
    } else {
        format!("{:04}-{:02}-{:02} at {time}", then.wYear, then.wMonth, then.wDay)
    }
}

/// Running on battery right now. An unknown answer counts as no.
/// The user's Documents folder, wherever it really is. OneDrive often moves it.
pub fn documents_folder() -> Option<PathBuf> {
    use windows::Win32::System::Com::CoTaskMemFree;
    use windows::Win32::UI::Shell::{FOLDERID_Documents, KF_FLAG_DEFAULT, SHGetKnownFolderPath};
    unsafe {
        let path = SHGetKnownFolderPath(&FOLDERID_Documents, KF_FLAG_DEFAULT, None).ok()?;
        let text = path.to_string().ok();
        CoTaskMemFree(Some(path.as_ptr() as *const _));
        text.map(PathBuf::from)
    }
}

/// Local date and time text for snippet placeholders.
pub fn local_date_time() -> (String, String) {
    let time = unsafe { windows::Win32::System::SystemInformation::GetLocalTime() };
    (
        format!("{:04}-{:02}-{:02}", time.wYear, time.wMonth, time.wDay),
        format!("{:02}:{:02}", time.wHour, time.wMinute),
    )
}

// --- Pointers carried by posted messages -----------------------------------------------------

/// Addresses of boxes currently travelling in a posted message. Any process on the desktop can
/// post to Orbit's windows and threads, so a message's pointer is used only if it's listed here.
static POSTED: std::sync::Mutex<Vec<usize>> = std::sync::Mutex::new(Vec::new());

/// Boxes `value` for sending in a posted message. Pair with [`take_posted`].
pub fn box_for_post<T: Send + 'static>(value: T) -> usize {
    let pointer = Box::into_raw(Box::new(value)) as usize;
    if let Ok(mut posted) = POSTED.lock() {
        posted.push(pointer);
    }
    pointer
}

/// Takes back a box from [`box_for_post`]. A value that isn't one of those gives `None`.
pub fn take_posted<T: Send + 'static>(pointer: usize) -> Option<Box<T>> {
    let mut posted = POSTED.lock().ok()?;
    let index = posted.iter().position(|&known| known == pointer)?;
    posted.swap_remove(index);
    Some(unsafe { Box::from_raw(pointer as *mut T) })
}

#[cfg(test)]
mod posted_tests {
    use super::*;

    #[test]
    fn only_a_box_we_sent_can_be_taken_and_only_once() {
        let pointer = box_for_post(String::from("hello"));
        assert!(take_posted::<String>(0x1234).is_none());
        assert_eq!(*take_posted::<String>(pointer).unwrap(), "hello");
        assert!(take_posted::<String>(pointer).is_none());
    }
}
