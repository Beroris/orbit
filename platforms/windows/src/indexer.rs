//! App discovery, run as a short-lived low-priority child process: `orbit --index-apps`.
//!
//! Enumerating `shell:AppsFolder` covers Win32 and packaged apps through one supported API, the
//! same list the Start menu shows. It also pulls large shell DLLs and possibly third-party shell
//! extensions into whichever process calls it. Doing the work in a child keeps all of that out
//! of the resident launcher, and a crash in a shell extension cannot take the launcher down.

use crate::util::write_atomic;
use serde::{Deserialize, Serialize};
use std::path::Path;
use windows::Win32::Foundation::{HWND, SIZE};
use windows::Win32::Graphics::Gdi::{
    BI_RGB, BITMAPINFO, BITMAPINFOHEADER, DIB_RGB_COLORS, DeleteObject, GetDC, GetDIBits, HBITMAP,
    HGDIOBJ, ReleaseDC,
};
use windows::Win32::System::Com::{
    COINIT_APARTMENTTHREADED, COINIT_DISABLE_OLE1DDE, CoInitializeEx,
    CoTaskMemFree, CoUninitialize,
};
use windows::Win32::System::Threading::{
    GetCurrentProcess, IDLE_PRIORITY_CLASS, ProcessPowerThrottling,
    PROCESS_POWER_THROTTLING_CURRENT_VERSION, PROCESS_POWER_THROTTLING_EXECUTION_SPEED,
    PROCESS_POWER_THROTTLING_STATE, SetPriorityClass, SetProcessInformation,
};
use windows::Win32::Foundation::PROPERTYKEY;
use windows::Win32::Storage::EnhancedStorage::PKEY_AppUserModel_ID;
use windows::Win32::UI::Shell::{
    BHID_EnumItems, IEnumShellItems, IShellItem, IShellItem2, IShellItemImageFactory,
    SHCreateItemFromParsingName, SIGDN_DESKTOPABSOLUTEPARSING, SIGDN_NORMALDISPLAY,
    SIIGBF_ICONONLY, SIIGBF_BIGGERSIZEOK,
};
use windows::core::{GUID, Interface, PWSTR, w};

pub const ICON_SIZE: usize = 64;
pub const ICON_BYTES: usize = ICON_SIZE * ICON_SIZE * 4;
pub const INDEX_FILE: &str = "apps.json";

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct AppEntry {
    /// `shell:AppsFolder` parsing name. Launching goes through this.
    pub id: String,
    pub name: String,
    /// Target executable, lowercase, if known. Used to find a running window.
    #[serde(default)]
    pub exe: String,
    /// AppUserModelID if known. Used to find a running window of a packaged app.
    #[serde(default)]
    pub aumid: String,
}

pub fn load_index(dir: &Path) -> Option<Vec<AppEntry>> {
    let text = std::fs::read(dir.join(INDEX_FILE)).ok()?;
    serde_json::from_slice(&text).ok()
}

/// Stable icon identity. FNV-1a of the app id.
pub fn icon_hash(id: &str) -> u64 {
    let mut hash = 0xcbf2_9ce4_8422_2325u64;
    for byte in id.bytes() {
        hash ^= byte as u64;
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    hash
}

pub fn icon_path(dir: &Path, hash: u64) -> std::path::PathBuf {
    dir.join("icons").join(format!("{hash:016x}.bgra"))
}

/// Entries that are not apps a person launches. Conservative: when unsure, keep it.
fn is_noise(name: &str, id: &str) -> bool {
    let lower = name.to_ascii_lowercase();
    let id_lower = id.to_ascii_lowercase();
    // "Uninstall Foo" or "Foo - Uninstall", but not an app that is itself an uninstaller, such as
    // Revo Uninstaller. An uninstaller's own program is named like unins000.exe or uninstall.exe.
    let words = lower.split(|c: char| !c.is_alphanumeric()).collect::<Vec<_>>();
    let program = id_lower.rsplit(['\\', '/']).next().unwrap_or("");
    if words.contains(&"uninstall") || (program.starts_with("unins") && program.ends_with(".exe")) {
        return true;
    }
    if id_lower.starts_with("http://") || id_lower.starts_with("https://") {
        return true; // documentation links dropped into the Start menu
    }
    [".chm", ".html", ".htm", ".txt", ".pdf", ".url", ".rtf", ".md"]
        .iter()
        .any(|ext| id_lower.ends_with(ext))
}

fn low_priority() {
    unsafe {
        let _ = SetPriorityClass(GetCurrentProcess(), IDLE_PRIORITY_CLASS);
        // EcoQoS: run on efficient cores at low clocks where the hardware supports it.
        let state = PROCESS_POWER_THROTTLING_STATE {
            Version: PROCESS_POWER_THROTTLING_CURRENT_VERSION,
            ControlMask: PROCESS_POWER_THROTTLING_EXECUTION_SPEED,
            StateMask: PROCESS_POWER_THROTTLING_EXECUTION_SPEED,
        };
        let _ = SetProcessInformation(
            GetCurrentProcess(),
            ProcessPowerThrottling,
            &state as *const _ as *const _,
            std::mem::size_of::<PROCESS_POWER_THROTTLING_STATE>() as u32,
        );
    }
}

unsafe fn display_name(item: &IShellItem, kind: windows::Win32::UI::Shell::SIGDN) -> Option<String> {
    unsafe {
        let raw: PWSTR = item.GetDisplayName(kind).ok()?;
        let text = raw.to_string().ok();
        CoTaskMemFree(Some(raw.0 as *const _));
        text
    }
}

unsafe fn string_property(item: &IShellItem2, key: &PROPERTYKEY) -> Option<String> {
    unsafe {
        let raw = item.GetString(key).ok()?;
        let text = raw.to_string().ok();
        CoTaskMemFree(Some(raw.0 as *const _));
        text.filter(|t| !t.is_empty())
    }
}

/// System.Link.TargetParsingPath
const PKEY_LINK_TARGET: PROPERTYKEY = PROPERTYKEY {
    fmtid: GUID::from_u128(0xb9b4b3fc_2b51_4a42_b5d8_324146afcf25),
    pid: 2,
};

/// Reads the icon as 64×64 premultiplied BGRA, top-down.
unsafe fn read_icon(item: &IShellItem) -> Option<Vec<u8>> {
    unsafe {
        let factory: IShellItemImageFactory = item.cast().ok()?;
        let size = SIZE { cx: ICON_SIZE as i32, cy: ICON_SIZE as i32 };
        let bitmap: HBITMAP = factory.GetImage(size, SIIGBF_ICONONLY | SIIGBF_BIGGERSIZEOK).ok()?;
        let mut info = BITMAPINFO::default();
        info.bmiHeader = BITMAPINFOHEADER {
            biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
            biWidth: ICON_SIZE as i32,
            biHeight: -(ICON_SIZE as i32),
            biPlanes: 1,
            biBitCount: 32,
            biCompression: BI_RGB.0,
            ..Default::default()
        };
        let mut pixels = vec![0u8; ICON_BYTES];
        let dc = GetDC(Some(HWND::default()));
        let lines = GetDIBits(
            dc,
            bitmap,
            0,
            ICON_SIZE as u32,
            Some(pixels.as_mut_ptr() as *mut _),
            &mut info,
            DIB_RGB_COLORS,
        );
        ReleaseDC(Some(HWND::default()), dc);
        let _ = DeleteObject(HGDIOBJ(bitmap.0));
        if lines != ICON_SIZE as i32 {
            return None;
        }
        // Old-style icons come back with no alpha at all. Treat those as opaque.
        if pixels.chunks_exact(4).all(|p| p[3] == 0) {
            if pixels.chunks_exact(4).all(|p| p[..3] == [0, 0, 0]) {
                return None;
            }
            for p in pixels.chunks_exact_mut(4) {
                p[3] = 255;
            }
        }
        Some(pixels)
    }
}

/// Entry point of the child process. Exit code 0 means the index file was replaced.
pub fn run(dir: &Path, refresh_icons: bool) -> i32 {
    low_priority();
    unsafe {
        if CoInitializeEx(None, COINIT_APARTMENTTHREADED | COINIT_DISABLE_OLE1DDE).is_err() {
            return 2;
        }
    }
    let result = enumerate(dir, refresh_icons);
    unsafe { CoUninitialize() };
    match result {
        Some(entries) if !entries.is_empty() => {
            let json = serde_json::to_vec(&entries).unwrap_or_default();
            if write_atomic(&dir.join(INDEX_FILE), &json).is_ok() { 0 } else { 3 }
        }
        _ => 1,
    }
}

fn enumerate(dir: &Path, refresh_icons: bool) -> Option<Vec<AppEntry>> {
    let icons_dir = dir.join("icons");
    let _ = std::fs::create_dir_all(&icons_dir);
    let mut entries: Vec<AppEntry> = Vec::with_capacity(256);
    unsafe {
        let folder: IShellItem =
            SHCreateItemFromParsingName(w!("shell:AppsFolder"), None).ok()?;
        let items: IEnumShellItems = folder.BindToHandler(None, &BHID_EnumItems).ok()?;
        loop {
            let mut batch = [None];
            let mut fetched = 0;
            if items.Next(&mut batch, Some(&mut fetched)).is_err() || fetched == 0 {
                break;
            }
            let Some(item) = batch[0].take() else { break };
            let Some(name) = display_name(&item, SIGDN_NORMALDISPLAY) else { continue };
            let Some(id) = display_name(&item, SIGDN_DESKTOPABSOLUTEPARSING) else { continue };
            if name.trim().is_empty() || is_noise(&name, &id) {
                continue;
            }
            let (mut exe, mut aumid) = (String::new(), String::new());
            if let Ok(item2) = item.cast::<IShellItem2>() {
                exe = string_property(&item2, &PKEY_LINK_TARGET).unwrap_or_default();
                aumid = string_property(&item2, &PKEY_AppUserModel_ID).unwrap_or_default();
            }
            let hash = icon_hash(&id);
            let path = icon_path(dir, hash);
            if refresh_icons || !path.exists() {
                if let Some(pixels) = read_icon(&item) {
                    let _ = write_atomic(&path, &pixels);
                }
            }
            entries.push(AppEntry {
                id,
                name,
                exe: exe.to_lowercase(),
                aumid,
            });
        }
    }
    // Stable order, and one entry per id, so an unchanged system writes an identical file.
    entries.sort_by(|a, b| a.name.to_lowercase().cmp(&b.name.to_lowercase()).then(a.id.cmp(&b.id)));
    entries.dedup_by(|a, b| a.id == b.id);
    Some(entries)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn filters_noise_but_keeps_apps() {
        assert!(is_noise("Uninstall Foo", "x"));
        assert!(is_noise("Foo - Uninstall", "x"));
        assert!(is_noise("Foo Uninstaller", r"C:\Program Files\Foo\unins000.exe"));
        assert!(!is_noise("Revo Uninstaller", r"{6D809377-6AF0-444B-8957-A3773F02200E}\VS Revo Group\Revo Uninstaller\RevoUnin.exe"));
        assert!(is_noise("Foo Docs", "C:\\foo\\readme.txt"));
        assert!(is_noise("Website", "https://example.com"));
        assert!(!is_noise("Notepad", "Microsoft.WindowsNotepad_8wekyb3d8bbwe!App"));
        assert!(!is_noise("Visual Studio Code", "Microsoft.VisualStudioCode"));
    }

    #[test]
    fn icon_hash_is_stable() {
        assert_eq!(icon_hash(""), 0xcbf29ce484222325);
        assert_ne!(icon_hash("a"), icon_hash("b"));
    }
}
