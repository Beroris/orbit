//! Watches for apps being installed, changed or removed, so the app list keeps up without any
//! periodic rescan: the Start menu folders, for desktop apps, and Windows' package catalog, for
//! Store and other packaged apps, which never touch those folders. The thread sleeps inside
//! `WaitForMultipleObjects`, with no timers and no wakeups until something changes.

use crate::util::{SendHwnd, wide_path};
use std::path::PathBuf;
use windows::Win32::Foundation::{HANDLE, WAIT_OBJECT_0, WAIT_TIMEOUT};
use windows::Win32::Storage::FileSystem::{
    FILE_NOTIFY_CHANGE_DIR_NAME, FILE_NOTIFY_CHANGE_FILE_NAME, FILE_NOTIFY_CHANGE_LAST_WRITE, FindCloseChangeNotification,
    FindFirstChangeNotificationW, FindNextChangeNotification,
};
use windows::Win32::System::Threading::{CreateEventW, INFINITE, SetEvent, WaitForMultipleObjects};
use windows::core::PCWSTR;

/// Installers touch many files. Wait for the burst to finish before reindexing.
const QUIET_MS: u32 = 5000;

fn start_menu_dirs() -> Vec<PathBuf> {
    let mut dirs = Vec::new();
    if let Some(appdata) = std::env::var_os("APPDATA") {
        dirs.push(PathBuf::from(appdata).join("Microsoft\\Windows\\Start Menu\\Programs"));
    }
    if let Some(programdata) = std::env::var_os("ProgramData") {
        dirs.push(PathBuf::from(programdata).join("Microsoft\\Windows\\Start Menu\\Programs"));
    }
    dirs
}

pub fn start(main: SendHwnd, dirty_message: u32) {
    let _ = std::thread::Builder::new()
        .name("orbit-watch".into())
        .stack_size(64 * 1024)
        .spawn(move || run(main, dirty_message));
}

fn run(main: SendHwnd, dirty_message: u32) {
    let mut handles: Vec<HANDLE> = Vec::new();
    for dir in start_menu_dirs() {
        let path = wide_path(&dir);
        // Names catch shortcuts added, removed or renamed. Writes catch a shortcut saved over one
        // of the same name, as an installer does when it reinstalls or moves its program: the
        // name stays, so only the write says the shortcut now points somewhere else.
        let filter = FILE_NOTIFY_CHANGE_FILE_NAME | FILE_NOTIFY_CHANGE_DIR_NAME | FILE_NOTIFY_CHANGE_LAST_WRITE;
        if let Ok(handle) = unsafe { FindFirstChangeNotificationW(PCWSTR(path.as_ptr()), true, filter) } {
            handles.push(handle);
        }
    }
    let folders = handles.len();
    // Packaged apps: the catalog says when one has finished installing, updating or going away.
    // It calls from a thread of its own, which only wakes this one. Kept alive while this runs.
    let _ = unsafe { windows::Win32::System::Com::CoInitializeEx(None, windows::Win32::System::Com::COINIT_MULTITHREADED) };
    let mut _catalog = None;
    if let Ok(event) = unsafe { CreateEventW(None, false, false, None) } {
        if let Some(catalog) = watch_packages(event) {
            handles.push(event);
            _catalog = Some(catalog);
        } else {
            let _ = unsafe { windows::Win32::Foundation::CloseHandle(event) };
        }
    }
    if handles.is_empty() {
        return;
    }
    // A folder's handle is asked for its next change. The catalog's event resets by itself.
    let rearm = |index: usize| {
        if index < folders {
            let _ = unsafe { FindNextChangeNotification(handles[index]) };
        }
    };
    loop {
        let signalled = unsafe { WaitForMultipleObjects(&handles, false, INFINITE) };
        let index = signalled.0.wrapping_sub(WAIT_OBJECT_0.0) as usize;
        if index >= handles.len() {
            break;
        }
        rearm(index);
        // Debounce: keep absorbing changes until the folders have been quiet for a while.
        loop {
            let again = unsafe { WaitForMultipleObjects(&handles, false, QUIET_MS) };
            if again == WAIT_TIMEOUT {
                break;
            }
            let index = again.0.wrapping_sub(WAIT_OBJECT_0.0) as usize;
            if index >= handles.len() {
                break;
            }
            rearm(index);
        }
        main.post(dirty_message, 0, 0);
    }
    for (index, handle) in handles.into_iter().enumerate() {
        if index < folders {
            let _ = unsafe { FindCloseChangeNotification(handle) };
        } else {
            let _ = unsafe { windows::Win32::Foundation::CloseHandle(handle) };
        }
    }
}

/// Sets `event` whenever a packaged app finishes installing, updating or being removed. Returns
/// the catalog, which must be kept for the events to keep coming.
fn watch_packages(event: HANDLE) -> Option<windows::ApplicationModel::PackageCatalog> {
    use windows::ApplicationModel::{PackageCatalog, PackageInstallingEventArgs, PackageUninstallingEventArgs, PackageUpdatingEventArgs};
    use windows::Foundation::TypedEventHandler;
    // A handle can be set from any thread. Only its number goes across.
    let raw = event.0 as isize;
    let wake = move || {
        let _ = unsafe { SetEvent(HANDLE(raw as *mut _)) };
    };
    let catalog = PackageCatalog::OpenForCurrentUser().ok()?;
    let installed = TypedEventHandler::<PackageCatalog, PackageInstallingEventArgs>::new(move |_, args| {
        if args.as_ref().is_some_and(|args| args.IsComplete().unwrap_or(false)) {
            wake();
        }
        Ok(())
    });
    let updated = TypedEventHandler::<PackageCatalog, PackageUpdatingEventArgs>::new(move |_, args| {
        if args.as_ref().is_some_and(|args| args.IsComplete().unwrap_or(false)) {
            wake();
        }
        Ok(())
    });
    let removed = TypedEventHandler::<PackageCatalog, PackageUninstallingEventArgs>::new(move |_, args| {
        if args.as_ref().is_some_and(|args| args.IsComplete().unwrap_or(false)) {
            wake();
        }
        Ok(())
    });
    catalog.PackageInstalling(&installed).ok()?;
    catalog.PackageUpdating(&updated).ok()?;
    catalog.PackageUninstalling(&removed).ok()?;
    Some(catalog)
}
