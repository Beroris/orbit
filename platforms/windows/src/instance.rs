//! Which Orbit is which. One Orbit runs per data folder, so a test copy can run beside the real one.
//! Each marks its window with its folder's key, which is how a second launch finds the right one to
//! open, and how `orbit --quit`, which the installer runs, finds the right one to close.

use std::path::{Path, PathBuf};
use windows::Win32::Foundation::{CloseHandle, HANDLE, HWND, LPARAM, WAIT_OBJECT_0};
use windows::Win32::System::Threading::{OpenProcess, PROCESS_SYNCHRONIZE, WaitForSingleObject};
use windows::Win32::UI::WindowsAndMessaging::{
    EnumWindows, GetClassNameW, GetPropW, GetWindowThreadProcessId, PostMessageW, SetPropW,
};
use windows::core::{BOOL, w};

const PROP: windows::core::PCWSTR = w!("OrbitInstance");

/// A short stable key per data folder.
pub fn key(dir: &Path) -> u64 {
    crate::indexer::icon_hash(&dir.to_string_lossy().to_lowercase())
}

pub fn name(dir: &Path) -> String {
    format!("{:016x}", key(dir))
}

/// A copy of orbit.exe with "portable" in its name keeps its data in an "Orbit settings" folder
/// beside itself, so it can run from a USB stick and leave nothing behind on the PC.
pub fn portable_dir() -> Option<PathBuf> {
    let exe = std::env::current_exe().ok()?;
    portable_dir_for(&exe)
}

fn portable_dir_for(exe: &Path) -> Option<PathBuf> {
    let stem = exe.file_stem()?.to_string_lossy().to_lowercase();
    stem.contains("portable").then(|| exe.parent().unwrap_or(Path::new(".")).join("Orbit settings"))
}

/// Marks the launcher window as the one for this data folder.
pub fn mark(window: HWND, dir: &Path) {
    // Only the low half fits in a pointer-sized value on every Windows Orbit runs on, which is
    // plenty to tell a handful of folders apart.
    let value = key(dir) as usize;
    unsafe {
        let _ = SetPropW(window, PROP, Some(HANDLE(value as *mut _)));
    }
}

/// The launcher window of the Orbit using this data folder, if one is running.
pub fn find(dir: &Path) -> Option<HWND> {
    struct Search {
        want: usize,
        found: Option<HWND>,
    }
    unsafe extern "system" fn visit(window: HWND, lparam: LPARAM) -> BOOL {
        let search = unsafe { &mut *(lparam.0 as *mut Search) };
        let mut class = [0u16; 32];
        let length = unsafe { GetClassNameW(window, &mut class) } as usize;
        if String::from_utf16_lossy(&class[..length]) == "OrbitLauncher" {
            let value = unsafe { GetPropW(window, PROP) };
            if value.0 as usize == search.want {
                search.found = Some(window);
                return BOOL(0);
            }
        }
        BOOL(1)
    }
    let mut search = Search { want: key(dir) as usize, found: None };
    unsafe {
        let _ = EnumWindows(Some(visit), LPARAM(&mut search as *mut Search as isize));
    }
    search.found
}

/// Whether an Orbit holds this data folder.
fn running(dir: &Path) -> bool {
    use windows::Win32::System::Threading::{OpenMutexW, SYNCHRONIZATION_SYNCHRONIZE};
    let mutex = crate::util::wide(&format!("Local\\OrbitLauncher-{}", name(dir)));
    match unsafe { OpenMutexW(SYNCHRONIZATION_SYNCHRONIZE, false, windows::core::PCWSTR(mutex.as_ptr())) } {
        Ok(handle) => {
            unsafe {
                let _ = CloseHandle(handle);
            }
            true
        }
        Err(_) => false,
    }
}

/// Any launcher window, for an Orbit from before windows were marked.
pub fn unmarked() -> Option<HWND> {
    unsafe { windows::Win32::UI::WindowsAndMessaging::FindWindowW(w!("OrbitLauncher"), w!("Orbit")).ok() }
}

/// Asks the Orbit using this data folder to quit, and waits up to ten seconds for it to go. True if
/// none is left running.
pub fn quit(dir: &Path) -> bool {
    let Some(window) = find(dir).or_else(|| if running(dir) { unmarked() } else { None }) else { return true };
    unsafe {
        let mut pid = 0u32;
        GetWindowThreadProcessId(window, Some(&mut pid));
        let process = OpenProcess(PROCESS_SYNCHRONIZE, false, pid).ok();
        let posted = PostMessageW(
            Some(window),
            crate::app::WM_ORBIT_RUN_BUILTIN,
            windows::Win32::Foundation::WPARAM(usize::MAX),
            LPARAM(0),
        );
        let Some(process) = process else { return posted.is_ok() };
        let gone = posted.is_ok() && WaitForSingleObject(process, 10_000) == WAIT_OBJECT_0;
        let _ = CloseHandle(process);
        gone
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_a_copy_named_portable_keeps_its_settings_beside_itself() {
        let dir = portable_dir_for(Path::new(r"D:\Tools\Orbit-1.0.0-Portable.exe"));
        assert_eq!(dir, Some(PathBuf::from(r"D:\Tools\Orbit settings")));
        assert_eq!(portable_dir_for(Path::new(r"C:\Users\a\AppData\Local\Programs\Orbit\orbit.exe")), None);
    }

    #[test]
    fn a_data_folder_has_the_same_key_however_it_is_written() {
        assert_eq!(key(Path::new(r"C:\Users\A\AppData\Local\Orbit")), key(Path::new(r"c:\users\a\appdata\local\orbit")));
    }
}
