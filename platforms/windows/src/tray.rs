//! Notification-area icon. Purely event driven: no timers keep it alive.

use crate::util::wide;
use orbit_core::settings::Appearance;
use windows::Win32::Foundation::{HWND, POINT};
use windows::Win32::System::LibraryLoader::{GetModuleHandleW, GetProcAddress, LoadLibraryW};
use windows::Win32::UI::Shell::{
    NIF_ICON, NIF_INFO, NIF_MESSAGE, NIF_TIP, NIIF_NONE, NIM_ADD, NIM_DELETE, NIM_MODIFY, NOTIFYICONDATAW,
    Shell_NotifyIconW,
};
use windows::Win32::UI::WindowsAndMessaging::{
    AppendMenuW, CreatePopupMenu, DestroyMenu, GetCursorPos, GetSystemMetrics, HICON, IMAGE_ICON, LR_DEFAULTCOLOR,
    LoadImageW, MF_CHECKED, MF_SEPARATOR, MF_STRING, MF_UNCHECKED, SM_CXSMICON, SM_CYSMICON, SetForegroundWindow,
    TPM_NONOTIFY, TPM_RETURNCMD, TrackPopupMenu,
};
use windows::core::{PCSTR, PCWSTR, w};

const TRAY_ID: u32 = 1;

fn copy_truncated(destination: &mut [u16], text: &str) {
    let units: Vec<u16> = text.encode_utf16().take(destination.len() - 1).collect();
    destination[..units.len()].copy_from_slice(&units);
    destination[units.len()] = 0;
}

pub fn load_app_icon(cx: i32, cy: i32) -> HICON {
    unsafe {
        let module = GetModuleHandleW(None).unwrap_or_default();
        LoadImageW(Some(module.into()), PCWSTR(1 as *const u16), IMAGE_ICON, cx, cy, LR_DEFAULTCOLOR)
            .map(|handle| HICON(handle.0))
            .unwrap_or_default()
    }
}

fn base_data(hwnd: HWND, message: u32) -> NOTIFYICONDATAW {
    let mut data = NOTIFYICONDATAW {
        cbSize: std::mem::size_of::<NOTIFYICONDATAW>() as u32,
        hWnd: hwnd,
        uID: TRAY_ID,
        uFlags: NIF_MESSAGE | NIF_ICON | NIF_TIP,
        uCallbackMessage: message,
        ..Default::default()
    };
    data.hIcon = load_app_icon(unsafe { GetSystemMetrics(SM_CXSMICON) }, unsafe { GetSystemMetrics(SM_CYSMICON) });
    copy_truncated(&mut data.szTip, "Orbit");
    data
}

pub fn add(hwnd: HWND, message: u32) {
    let data = base_data(hwnd, message);
    unsafe {
        let _ = Shell_NotifyIconW(NIM_ADD, &data);
    }
}

pub fn remove(hwnd: HWND, message: u32) {
    let data = base_data(hwnd, message);
    unsafe {
        let _ = Shell_NotifyIconW(NIM_DELETE, &data);
    }
}

/// Counts the notifications shown, so a click on one can be told from a click on another.
static NOTES: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);

/// Windows says a notification was clicked with this, in the tray message's lParam.
pub const NOTIFICATION_CLICKED: u32 = 0x0405; // NIN_BALLOONUSERCLICK

/// The last notification shown, which is the one a click is on: Windows shows one at a time.
pub fn last_note() -> u32 {
    NOTES.load(std::sync::atomic::Ordering::Relaxed)
}

/// Shows a notification, and gives its number for [`last_note`].
pub fn notify(hwnd: HWND, message: u32, title: &str, body: &str) -> u32 {
    let note = NOTES.fetch_add(1, std::sync::atomic::Ordering::Relaxed) + 1;
    let mut data = base_data(hwnd, message);
    data.uFlags |= NIF_INFO;
    data.dwInfoFlags = NIIF_NONE;
    copy_truncated(&mut data.szInfoTitle, title);
    copy_truncated(&mut data.szInfo, body);
    unsafe {
        let _ = Shell_NotifyIconW(NIM_MODIFY, &data);
    }
    note
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MenuChoice {
    Open = 1,
    Settings,
    ToggleClipboardPause,
    ToggleSuspend,
    Quit,
    Update,
}

pub struct MenuState {
    pub clipboard_enabled: bool,
    pub clipboard_paused: bool,
    pub suspended: bool,
    pub launcher_hint: String,
    /// Orbit's Appearance, which the menu follows: Windows' own colours unless it's set to one.
    pub appearance: Appearance,
    /// A new version to install or get, such as "Install Orbit 1.2.0".
    pub update: Option<String>,
}

/// Runs a modal menu loop. The caller must not hold the app state across this call.
pub fn show_menu(hwnd: HWND, state: &MenuState) -> Option<MenuChoice> {
    follow_theme(hwnd, state.appearance);
    unsafe {
        let menu = CreatePopupMenu().ok()?;
        let add = |id: usize, text: &str, checked: bool| {
            let label = wide(text);
            let check = if checked { MF_CHECKED } else { MF_UNCHECKED };
            let _ = AppendMenuW(menu, MF_STRING | check, id, PCWSTR(label.as_ptr()));
        };
        add(MenuChoice::Open as usize, &format!("Open Orbit\t{}", state.launcher_hint), false);
        add(MenuChoice::Settings as usize, "Settings…", false);
        if let Some(update) = &state.update {
            add(MenuChoice::Update as usize, update, false);
        }
        let _ = AppendMenuW(menu, MF_SEPARATOR, 0, PCWSTR::null());
        if state.clipboard_enabled {
            add(MenuChoice::ToggleClipboardPause as usize, "Pause clipboard history", state.clipboard_paused);
        }
        add(MenuChoice::ToggleSuspend as usize, "Suspend shortcuts", state.suspended);
        let _ = AppendMenuW(menu, MF_SEPARATOR, 0, PCWSTR::null());
        add(MenuChoice::Quit as usize, "Quit Orbit", false);

        let mut point = POINT::default();
        let _ = GetCursorPos(&mut point);
        // Required so the menu closes when the user clicks elsewhere.
        let _ = SetForegroundWindow(hwnd);
        let chosen = TrackPopupMenu(menu, TPM_RETURNCMD | TPM_NONOTIFY, point.x, point.y, None, hwnd, None);
        let _ = DestroyMenu(menu);
        match chosen.0 {
            1 => Some(MenuChoice::Open),
            2 => Some(MenuChoice::Settings),
            3 => Some(MenuChoice::ToggleClipboardPause),
            4 => Some(MenuChoice::ToggleSuspend),
            5 => Some(MenuChoice::Quit),
            6 => Some(MenuChoice::Update),
            _ => None,
        }
    }
}

/// Windows' own menus are light unless an app says it can be dark. The switch is in uxtheme.dll
/// with a number and no name: 135 sets the app's mode (1 follows Windows, 2 is always dark, 3
/// always light), 133 lets a window go dark, and 136 makes menus pick up the change, so changing
/// Windows' colours or Orbit's Appearance while Orbit runs shows at the next opening. Where those
/// aren't there, menus stay light, as before.
fn follow_theme(hwnd: HWND, appearance: Appearance) {
    type SetPreferredAppMode = unsafe extern "system" fn(i32) -> i32;
    type AllowDarkModeForWindow = unsafe extern "system" fn(HWND, i32) -> i32;
    type FlushMenuThemes = unsafe extern "system" fn();
    let mode = match appearance {
        Appearance::System => 1,
        Appearance::Dark => 2,
        Appearance::Light => 3,
    };
    unsafe {
        let Ok(uxtheme) = LoadLibraryW(w!("uxtheme.dll")) else { return };
        let by_number = |number: usize| GetProcAddress(uxtheme, PCSTR(number as *const u8));
        if let Some(function) = by_number(135) {
            std::mem::transmute::<unsafe extern "system" fn() -> isize, SetPreferredAppMode>(function)(mode);
        }
        if let Some(function) = by_number(133) {
            std::mem::transmute::<unsafe extern "system" fn() -> isize, AllowDarkModeForWindow>(function)(hwnd, (mode != 3) as i32);
        }
        if let Some(function) = by_number(136) {
            std::mem::transmute::<unsafe extern "system" fn() -> isize, FlushMenuThemes>(function)();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The numbered functions the menu's colours rely on are still in this Windows' uxtheme.dll.
    #[test]
    fn the_menu_colour_switches_are_there() {
        unsafe {
            let uxtheme = LoadLibraryW(w!("uxtheme.dll")).expect("uxtheme.dll");
            for number in [133usize, 135, 136] {
                assert!(GetProcAddress(uxtheme, PCSTR(number as *const u8)).is_some(), "uxtheme.dll has no function {number}");
            }
        }
    }
}
