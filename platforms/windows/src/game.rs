//! Is the app in front a game?
//!
//! HyperKey and the launcher's hotkey step aside for a fullscreen game, and snippets never type
//! into one. A window that
//! merely fills the screen isn't enough: a browser in F11, or an editor on a monitor with an
//! auto-hiding taskbar, covers it too and still wants its shortcuts.
//!
//! A fullscreen window counts as a game when any of these says so. None of them look inside the
//! process or touch the disk:
//! - its program is on the list Windows itself keeps of games it has recognised,
//! - its program lives in a game store's library folder, or has an Unreal shipping build's name,
//! - Windows reports exclusive Direct3D fullscreen.
//!
//! Anything missed can go on the "keeps its normal behaviour" list in Settings.

use crate::util::wide;
use windows::Win32::System::Registry::{
    HKEY, HKEY_CURRENT_USER, KEY_READ, REG_VALUE_TYPE, RRF_RT_REG_SZ, RegCloseKey, RegEnumKeyExW, RegGetValueW, RegOpenKeyExW,
};
use windows::Win32::UI::Shell::{QUNS_RUNNING_D3D_FULL_SCREEN, SHQueryUserNotificationState};
use windows::core::{PCWSTR, PWSTR, w};

/// Folders that only hold games, as lower-case path fragments.
const LIBRARY_FOLDERS: &[&str] = &[
    "\\steamapps\\common\\",
    "\\epic games\\",
    "\\gog games\\",
    "\\gog galaxy\\games\\",
    "\\riot games\\",
    "\\xboxgames\\",
    "\\ea games\\",
    "\\origin games\\",
    "\\ubisoft game launcher\\games\\",
    "\\rockstar games\\",
    "\\amazon games\\library\\",
    "\\itch\\apps\\",
];

/// Unreal Engine names its packaged games like `Name-Win64-Shipping.exe`.
const ENGINE_SUFFIXES: &[&str] = &["-win64-shipping.exe", "-wingdk-shipping.exe", "-win32-shipping.exe"];

/// `path` is lower case. `known` is sorted, as `windows_game_list` returns it.
pub fn is_game_path(path: &str, known: &[String]) -> bool {
    if path.is_empty() {
        return false;
    }
    LIBRARY_FOLDERS.iter().any(|folder| path.contains(folder))
        || ENGINE_SUFFIXES.iter().any(|suffix| path.ends_with(suffix))
        || known.binary_search_by(|entry| entry.as_str().cmp(path)).is_ok()
}

/// Exclusive fullscreen, which only games and a few video players use. About 60 microseconds, and
/// asked only when a window already fills the monitor, as the trigger goes down.
pub fn exclusive_fullscreen() -> bool {
    unsafe { SHQueryUserNotificationState() }.is_ok_and(|state| state == QUNS_RUNNING_D3D_FULL_SCREEN)
}

/// The programs Windows has recognised as games, from the settings store behind Game Bar. Lower
/// case and sorted. Takes about a millisecond, so it runs on its own short-lived thread.
pub fn windows_game_list() -> Vec<String> {
    let mut list = Vec::new();
    unsafe {
        let mut children = HKEY::default();
        if RegOpenKeyExW(HKEY_CURRENT_USER, w!("System\\GameConfigStore\\Children"), None, KEY_READ, &mut children).is_err() {
            return list;
        }
        let mut index = 0;
        loop {
            let mut name = [0u16; 128];
            let mut name_len = name.len() as u32;
            let status = RegEnumKeyExW(children, index, Some(PWSTR(name.as_mut_ptr())), &mut name_len, None, None, None, None);
            if status.is_err() {
                break;
            }
            index += 1;
            let mut path = [0u16; 1024];
            let mut bytes = std::mem::size_of_val(&path) as u32;
            let mut kind = REG_VALUE_TYPE::default();
            let read = RegGetValueW(
                children,
                PCWSTR(wide(&String::from_utf16_lossy(&name[..name_len as usize])).as_ptr()),
                w!("MatchedExeFullPath"),
                RRF_RT_REG_SZ,
                Some(&mut kind),
                Some(path.as_mut_ptr() as *mut _),
                Some(&mut bytes),
            );
            if read.is_ok() && bytes >= 2 {
                let units = (bytes as usize / 2).saturating_sub(1).min(path.len());
                let text = String::from_utf16_lossy(&path[..units]).to_lowercase();
                if !text.is_empty() {
                    list.push(text);
                }
            }
        }
        let _ = RegCloseKey(children);
    }
    list.sort_unstable();
    list.dedup();
    list
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn library_folders_and_engine_builds_count() {
        for path in [
            "d:\\steamlibrary\\steamapps\\common\\apex legends\\r5apex_dx12.exe",
            "c:\\program files\\epic games\\rocketleague\\binaries\\win64\\rocketleague.exe",
            "c:\\riot games\\valorant\\live\\shootergame\\binaries\\win64\\valorant-win64-shipping.exe",
            "c:\\games\\aimtrainer\\fpsaimtrainer-win64-shipping.exe",
            "e:\\xboxgames\\halo infinite\\content\\haloinfinite.exe",
        ] {
            assert!(is_game_path(path, &[]), "{path}");
        }
    }

    #[test]
    fn ordinary_programs_do_not() {
        for path in [
            "",
            "c:\\program files\\zen browser\\zen.exe",
            "c:\\users\\me\\appdata\\local\\programs\\microsoft vs code\\code.exe",
            "c:\\windows\\system32\\notepad.exe",
            "c:\\program files (x86)\\steam\\steam.exe",
            "c:\\code\\shipping.exe",
        ] {
            assert!(!is_game_path(path, &[]), "{path}");
        }
    }

    #[test]
    fn the_list_from_windows_counts_when_the_path_matches_exactly() {
        let known = vec!["c:\\games\\a.exe".to_string(), "c:\\games\\overwatch.exe".to_string()];
        assert!(is_game_path("c:\\games\\overwatch.exe", &known));
        assert!(!is_game_path("c:\\games\\overwatch2.exe", &known));
    }

    #[test]
    fn reading_the_list_from_windows_is_safe_and_sorted() {
        // Empty on a machine with no Game Bar history. Either way it must not fail.
        let list = windows_game_list();
        assert!(list.windows(2).all(|pair| pair[0] < pair[1]));
        assert!(list.iter().all(|entry| *entry == entry.to_lowercase()));
    }
}
