//! Notification-area icon. Purely event driven: no timers keep it alive.

use crate::util::wide;
use windows::Win32::Foundation::{HWND, POINT};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::Shell::{
    NIF_ICON, NIF_INFO, NIF_MESSAGE, NIF_TIP, NIIF_NONE, NIM_ADD, NIM_DELETE, NIM_MODIFY, NOTIFYICONDATAW,
    Shell_NotifyIconW,
};
use windows::Win32::UI::WindowsAndMessaging::{
    AppendMenuW, CreatePopupMenu, DestroyMenu, GetCursorPos, GetSystemMetrics, HICON, IMAGE_ICON, LR_DEFAULTCOLOR,
    LoadImageW, MF_CHECKED, MF_SEPARATOR, MF_STRING, MF_UNCHECKED, SM_CXSMICON, SM_CYSMICON, SetForegroundWindow,
    TPM_NONOTIFY, TPM_RETURNCMD, TrackPopupMenu,
};
use windows::core::PCWSTR;

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

pub fn notify(hwnd: HWND, message: u32, title: &str, body: &str) {
    let mut data = base_data(hwnd, message);
    data.uFlags |= NIF_INFO;
    data.dwInfoFlags = NIIF_NONE;
    copy_truncated(&mut data.szInfoTitle, title);
    copy_truncated(&mut data.szInfo, body);
    unsafe {
        let _ = Shell_NotifyIconW(NIM_MODIFY, &data);
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MenuChoice {
    Open = 1,
    Settings,
    ToggleClipboardPause,
    ToggleSuspend,
    Quit,
}

pub struct MenuState {
    pub clipboard_enabled: bool,
    pub clipboard_paused: bool,
    pub suspended: bool,
    pub launcher_hint: String,
}

/// Runs a modal menu loop. The caller must not hold the app state across this call.
pub fn show_menu(hwnd: HWND, state: &MenuState) -> Option<MenuChoice> {
    unsafe {
        let menu = CreatePopupMenu().ok()?;
        let add = |id: usize, text: &str, checked: bool| {
            let label = wide(text);
            let check = if checked { MF_CHECKED } else { MF_UNCHECKED };
            let _ = AppendMenuW(menu, MF_STRING | check, id, PCWSTR(label.as_ptr()));
        };
        add(MenuChoice::Open as usize, &format!("Open Orbit\t{}", state.launcher_hint), false);
        add(MenuChoice::Settings as usize, "Settings…", false);
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
            _ => None,
        }
    }
}
