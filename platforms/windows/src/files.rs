//! The files thread: searches with Everything, and the icons of files and extensions' pictures.
//!
//! Started the first time it's needed. It owns a hidden message-only window, because Everything
//! answers a query with a message to a window. Searches are answered newest first: one that has
//! been overtaken by another while it waited is skipped. Answers go to the UI thread as
//! `WM_ORBIT_FILES` with a boxed `FileAnswer`.

use crate::everything::{self, FileHit, Problem};
use crate::util::{SendHwnd, box_for_post, wide};
use orbit_core::settings::FileSort;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{Receiver, Sender, channel};
use windows::Win32::Foundation::{HWND, LPARAM, LRESULT, WPARAM};
use windows::Win32::System::Com::{COINIT_APARTMENTTHREADED, COINIT_DISABLE_OLE1DDE, CoInitializeEx};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::WindowsAndMessaging::{ChangeWindowMessageFilterEx, MSGFLT_ALLOW, CreateWindowExW, DefWindowProcW, HWND_MESSAGE, RegisterClassExW, WINDOW_EX_STYLE, WINDOW_STYLE, WM_COPYDATA, WNDCLASSEXW};
use windows::core::{PCWSTR, w};

/// Where a search's answer goes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Place {
    /// A few files under the launcher's other results.
    Root,
    /// The file search's own list.
    List,
}

pub enum FileReq {
    Search { seq: u64, place: Place, query: String, max: u32, sort: FileSort },
    /// A file's icon, or an extension's picture (`picture`), for `hash` in the icon cache.
    Icon { hash: u64, path: String, size: usize, picture: bool },
    /// A file was opened from Orbit.
    Opened(String),
}

pub enum FileAnswer {
    Results { seq: u64, place: Place, result: Result<Vec<FileHit>, Problem> },
    Icon { hash: u64, size: usize, pixels: Option<Vec<u8>> },
}

pub struct Files {
    tx: Sender<FileReq>,
    /// The newest search for each place. Older ones are skipped.
    latest: Arc<[AtomicU64; 2]>,
}

impl Files {
    pub fn start(main: SendHwnd, message: u32) -> Files {
        let (tx, rx) = channel();
        let latest: Arc<[AtomicU64; 2]> = Arc::new([AtomicU64::new(0), AtomicU64::new(0)]);
        let seen = latest.clone();
        let _ = std::thread::Builder::new().name("orbit-files".into()).spawn(move || run(rx, seen, main, message));
        Files { tx, latest }
    }

    pub fn search(&self, seq: u64, place: Place, query: String, max: u32, sort: FileSort) {
        self.latest[place as usize].store(seq, Ordering::Relaxed);
        let _ = self.tx.send(FileReq::Search { seq, place, query, max, sort });
    }

    pub fn send(&self, request: FileReq) {
        let _ = self.tx.send(request);
    }
}

unsafe extern "system" fn reply_proc(hwnd: HWND, message: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    if message == WM_COPYDATA && lparam.0 != 0 && everything::take_reply(lparam) {
        return LRESULT(1);
    }
    unsafe { DefWindowProcW(hwnd, message, wparam, lparam) }
}

fn reply_window() -> Option<HWND> {
    unsafe {
        let module = GetModuleHandleW(None).ok()?;
        let class = WNDCLASSEXW {
            cbSize: std::mem::size_of::<WNDCLASSEXW>() as u32,
            lpfnWndProc: Some(reply_proc),
            hInstance: module.into(),
            lpszClassName: w!("OrbitFiles"),
            ..Default::default()
        };
        RegisterClassExW(&class);
        let window = CreateWindowExW(WINDOW_EX_STYLE(0), w!("OrbitFiles"), PCWSTR::null(), WINDOW_STYLE(0), 0, 0, 0, 0, Some(HWND_MESSAGE), None, Some(module.into()), None).ok()?;
        // Everything may run elevated, and Windows drops messages from higher to lower windows
        // unless they are let through.
        let _ = ChangeWindowMessageFilterEx(window, WM_COPYDATA, MSGFLT_ALLOW, None);
        Some(window)
    }
}

fn run(rx: Receiver<FileReq>, latest: Arc<[AtomicU64; 2]>, main: SendHwnd, message: u32) {
    unsafe {
        let _ = CoInitializeEx(None, COINIT_APARTMENTTHREADED | COINIT_DISABLE_OLE1DDE);
    }
    let Some(window) = reply_window() else { return };
    let post = |answer: FileAnswer| main.post(message, box_for_post(answer), 0);
    while let Ok(request) = rx.recv() {
        match request {
            FileReq::Search { seq, place, query, max, sort } => {
                if latest[place as usize].load(Ordering::Relaxed) != seq {
                    continue;
                }
                let result = everything::search(window, &query, max, sort);
                post(FileAnswer::Results { seq, place, result });
            }
            FileReq::Icon { hash, path, size, picture } => {
                let pixels = if picture { picture_pixels(&path, size) } else { file_icon(&path, size) };
                post(FileAnswer::Icon { hash, size, pixels });
            }
            FileReq::Opened(path) => everything::note_opened(window, &path),
        }
    }
}

/// The icon Windows shows for a file or folder, at `size`.
pub fn file_icon(path: &str, size: usize) -> Option<Vec<u8>> {
    use windows::Win32::UI::Shell::{IShellItem, SHCreateItemFromParsingName};
    let text = wide(path);
    let item: IShellItem = unsafe { SHCreateItemFromParsingName(PCWSTR(text.as_ptr()), None) }.ok()?;
    let pixels = unsafe { crate::indexer::read_icon(&item) }?;
    Some(crate::gfx::downscale_premultiplied(&pixels, crate::indexer::ICON_SIZE, size))
}

/// A picture file drawn as an icon: PNG, ICO, SVG and the rest Windows can read.
pub fn picture_pixels(path: &str, size: usize) -> Option<Vec<u8>> {
    let bytes = std::fs::read(path).ok().filter(|bytes| bytes.len() < 4 * 1024 * 1024)?;
    let pixels = crate::favicon::decode(&bytes)?;
    Some(crate::gfx::downscale_premultiplied(&pixels, crate::indexer::ICON_SIZE, size))
}

/// The key of a path's icon in the icon cache, away from apps' and favicons' keys.
pub fn icon_hash(path: &str, picture: bool) -> u64 {
    let base = crate::indexer::icon_hash(&path.to_lowercase());
    (if picture { 0xE7E0_0000_0000_0000 } else { 0xF11E_0000_0000_0000 }) | (base & 0x0000_FFFF_FFFF_FFFF)
}

/// What a file's row says under its name: the folder it's in, shortened from the front if long.
pub fn shown_folder(folder: &str) -> String {
    let home = std::env::var("USERPROFILE").unwrap_or_default();
    if !home.is_empty() && folder.len() >= home.len() && folder[..home.len()].eq_ignore_ascii_case(&home) {
        return format!("~{}", &folder[home.len()..]);
    }
    folder.to_string()
}

/// The kind of file, for the right of its row: "PDF", "Folder", "Application".
pub fn kind_of(hit: &FileHit) -> String {
    if hit.is_folder {
        return if hit.folder.is_empty() { "Drive".into() } else { "Folder".into() };
    }
    match hit.name.rsplit_once('.').map(|(_, ext)| ext.to_ascii_lowercase()) {
        Some(ext) if ext == "exe" => "Application".into(),
        Some(ext) if ext == "lnk" => "Shortcut".into(),
        Some(ext) if !ext.is_empty() && ext.len() <= 5 => ext.to_ascii_uppercase(),
        _ => "File".into(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hit(name: &str, folder: &str, is_folder: bool) -> FileHit {
        FileHit { name: name.into(), folder: folder.into(), is_folder, size: None, modified: None }
    }

    #[test]
    fn kinds_read_like_explorers() {
        assert_eq!(kind_of(&hit("report.pdf", "C:\\", false)), "PDF");
        assert_eq!(kind_of(&hit("orbit.exe", "C:\\", false)), "Application");
        assert_eq!(kind_of(&hit("Projects", "C:\\", true)), "Folder");
        assert_eq!(kind_of(&hit("C:", "", true)), "Drive");
        assert_eq!(kind_of(&hit("Makefile", "C:\\", false)), "File");
        assert_eq!(kind_of(&hit("archive.backup-old", "C:\\", false)), "File");
    }

    #[test]
    fn icon_keys_differ_by_kind_and_ignore_case() {
        assert_eq!(icon_hash("C:\\A.txt", false), icon_hash("c:\\a.TXT", false));
        assert_ne!(icon_hash("C:\\a.png", false), icon_hash("C:\\a.png", true));
        assert_ne!(icon_hash("C:\\a.png", false) >> 48, crate::rows::thumb_hash(1) >> 48);
    }
}
