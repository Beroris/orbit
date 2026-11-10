//! What a new quicklink could be, from the window that was in front when it was asked for: the
//! page a browser shows, the folder File Explorer shows, or the file another app has open.
//!
//! This runs only when Create Quicklink is chosen, on a thread of its own, and reads only that one
//! window: a browser's address bar through UI Automation, Explorer's folder through the shell, and
//! a window's title, matched against Windows' own list of recent files. Nothing is kept unless the
//! quicklink is saved.

use crate::util::{SendHwnd, box_for_post, exe_name, process_image_path};
use std::path::{Path, PathBuf};
use windows::Win32::Foundation::HWND;
use windows::Win32::System::Com::{CLSCTX_ALL, CLSCTX_INPROC_SERVER, COINIT_APARTMENTTHREADED, CoCreateInstance, CoInitializeEx, CoUninitialize};
use windows::Win32::UI::WindowsAndMessaging::{GetClassNameW, GetWindowTextW, GetWindowThreadProcessId, IsWindow};
use windows::core::Interface;

/// What the form can be filled in with.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Suggestion {
    pub name: String,
    /// An address or a path. Empty when only the app is known.
    pub target: String,
    /// The program the file was open in, lowercase, to open it with again.
    pub program: String,
    /// The packaged app it was open in, which has no program of its own.
    pub aumid: String,
    /// Where it came from, to say so: "Zen", "File Explorer" or the like.
    pub from: String,
}

/// Browsers built on Chromium, whose address bar is the window's first text box.
const CHROMIUM: &[&str] = &["chrome.exe", "msedge.exe", "brave.exe", "vivaldi.exe", "opera.exe", "opera_gx.exe", "arc.exe", "thorium.exe", "chromium.exe", "yandex.exe"];
/// Browsers built on Firefox, whose address bar is named `urlbar-input`.
const GECKO: &[&str] = &["firefox.exe", "zen.exe", "librewolf.exe", "floorp.exe", "waterfox.exe", "mullvadbrowser.exe", "tor.exe"];

/// Works out a suggestion on its own thread, and posts a boxed [`Suggestion`] to `main` as
/// `message` if there is one.
pub fn suggest_async(window: HWND, main: SendHwnd, message: u32) {
    let window = SendHwnd::new(window);
    let _ = std::thread::Builder::new().name("orbit-link-suggestion".into()).spawn(move || {
        let _ = unsafe { CoInitializeEx(None, COINIT_APARTMENTTHREADED) };
        let found = suggest(window.get());
        unsafe { CoUninitialize() };
        if let Some(found) = found {
            main.post(message, box_for_post(found), 0);
        }
    });
}

pub fn suggest(window: HWND) -> Option<Suggestion> {
    if window.is_invalid() || !unsafe { IsWindow(Some(window)) }.as_bool() {
        return None;
    }
    let mut pid = 0u32;
    unsafe { GetWindowThreadProcessId(window, Some(&mut pid)) };
    // Orbit's own windows, such as Settings, have nothing to link to.
    if pid == 0 || pid == std::process::id() {
        return None;
    }
    let program = process_image_path(pid).unwrap_or_default();
    let exe = exe_name(&program).to_lowercase();
    let title = window_text(window);
    if CHROMIUM.contains(&exe.as_str()) || GECKO.contains(&exe.as_str()) {
        let url = address_bar(window, GECKO.contains(&exe.as_str()))?;
        let url = url.trim();
        // A browser's own pages, such as settings or a new tab, aren't worth a link.
        if url.is_empty() || url.contains(char::is_whitespace) || is_internal(url) {
            return None;
        }
        let target = if url.contains("://") { url.to_string() } else { format!("https://{url}") };
        return Some(Suggestion { name: page_title(&title), target, from: browser_name(&exe), ..Suggestion::default() });
    }
    if exe == "explorer.exe" && class_name(window) == "CabinetWClass" {
        let folder = explorer_folder(window)?;
        let name = folder.file_name().map_or_else(|| folder.to_string_lossy().into_owned(), |name| name.to_string_lossy().into_owned());
        return Some(Suggestion { name, target: folder.to_string_lossy().into_owned(), from: "File Explorer".into(), ..Suggestion::default() });
    }
    let aumid = crate::launch::window_aumid(pid).unwrap_or_default();
    let from = title.rsplit(" - ").next().unwrap_or_default().trim().to_string();
    let base = Suggestion { program: program.to_lowercase(), aumid, from, ..Suggestion::default() };
    match recent_file(&title) {
        Some(path) => {
            let name = path.file_stem().map_or_else(String::new, |stem| stem.to_string_lossy().into_owned());
            Some(Suggestion { name, target: path.to_string_lossy().into_owned(), ..base })
        }
        None if !base.program.is_empty() || !base.aumid.is_empty() => Some(base),
        None => None,
    }
}

fn window_text(window: HWND) -> String {
    let mut buffer = [0u16; 512];
    let len = unsafe { GetWindowTextW(window, &mut buffer) };
    String::from_utf16_lossy(&buffer[..len.max(0) as usize])
}

fn class_name(window: HWND) -> String {
    let mut buffer = [0u16; 64];
    let len = unsafe { GetClassNameW(window, &mut buffer) };
    String::from_utf16_lossy(&buffer[..len.max(0) as usize])
}

fn is_internal(url: &str) -> bool {
    let lower = url.to_ascii_lowercase();
    ["about:", "chrome:", "edge:", "brave:", "vivaldi:", "opera:", "moz-extension:", "chrome-extension:", "view-source:", "file:"].iter().any(|scheme| lower.starts_with(scheme))
}

fn browser_name(exe: &str) -> String {
    match exe {
        "chrome.exe" => "Chrome",
        "msedge.exe" => "Edge",
        "brave.exe" => "Brave",
        "vivaldi.exe" => "Vivaldi",
        "opera.exe" | "opera_gx.exe" => "Opera",
        "arc.exe" => "Arc",
        "firefox.exe" => "Firefox",
        "zen.exe" => "Zen",
        "librewolf.exe" => "LibreWolf",
        "floorp.exe" => "Floorp",
        "waterfox.exe" => "Waterfox",
        _ => "your browser",
    }
    .to_string()
}

/// A page's title from its window's: browsers add their own name at the end.
pub fn page_title(title: &str) -> String {
    let title = title.trim();
    for separator in [" — ", " - "] {
        if let Some((page, _browser)) = title.rsplit_once(separator) {
            if !page.trim().is_empty() {
                return page.trim().to_string();
            }
        }
    }
    title.to_string()
}

/// The address a browser window shows, read through UI Automation.
fn address_bar(window: HWND, gecko: bool) -> Option<String> {
    use windows::Win32::UI::Accessibility::*;
    use windows::Win32::System::Variant::VARIANT;
    use windows::core::BSTR;
    unsafe {
        let automation: IUIAutomation = CoCreateInstance(&CUIAutomation, None, CLSCTX_INPROC_SERVER).ok()?;
        let root = automation.ElementFromHandle(window).ok()?;
        let condition = if gecko {
            automation.CreatePropertyCondition(UIA_AutomationIdPropertyId, &VARIANT::from(BSTR::from("urlbar-input"))).ok()?
        } else {
            automation.CreatePropertyCondition(UIA_ControlTypePropertyId, &VARIANT::from(UIA_EditControlTypeId.0)).ok()?
        };
        let field = root.FindFirst(TreeScope_Descendants, &condition).ok()?;
        let value = field.GetCurrentPropertyValue(UIA_ValueValuePropertyId).ok()?;
        let text = BSTR::try_from(&value).ok()?.to_string();
        (!text.is_empty()).then_some(text)
    }
}

/// The folder an Explorer window shows, in the tab that's showing.
fn explorer_folder(window: HWND) -> Option<PathBuf> {
    use windows::Win32::System::Com::IServiceProvider;
    use windows::Win32::UI::Shell::*;
    use windows::Win32::UI::WindowsAndMessaging::IsWindowVisible;
    use windows::Win32::System::Variant::VARIANT;
    unsafe {
        let shell_windows: IShellWindows = CoCreateInstance(&ShellWindows, None, CLSCTX_ALL).ok()?;
        for index in 0..shell_windows.Count().ok()? {
            let Ok(item) = shell_windows.Item(&VARIANT::from(index)) else { continue };
            let Ok(browser) = item.cast::<IWebBrowserApp>() else { continue };
            if browser.HWND().ok().map(|handle| handle.0) != Some(window.0 as isize) {
                continue;
            }
            let Ok(provider) = browser.cast::<IServiceProvider>() else { continue };
            let Ok(shell_browser) = provider.QueryService::<IShellBrowser>(&SID_STopLevelBrowser) else { continue };
            let Ok(view) = shell_browser.QueryActiveShellView() else { continue };
            // Windows 11 gives each tab its own entry. Only the one in front is showing.
            if !view.GetWindow().is_ok_and(|view_window| IsWindowVisible(view_window).as_bool()) {
                continue;
            }
            let Ok(folder_view) = view.cast::<IFolderView>() else { continue };
            let Ok(folder) = folder_view.GetFolder::<IPersistFolder2>() else { continue };
            let Ok(pidl) = folder.GetCurFolder() else { continue };
            let mut buffer = [0u16; 1024];
            let found = SHGetPathFromIDListEx(pidl, &mut buffer, GPFIDL_DEFAULT).as_bool();
            windows::Win32::System::Com::CoTaskMemFree(Some(pidl as *const _));
            if found {
                let len = buffer.iter().position(|&unit| unit == 0).unwrap_or(buffer.len());
                return Some(PathBuf::from(String::from_utf16_lossy(&buffer[..len])));
            }
        }
        None
    }
}

/// The file a window's title names, if Windows' list of recent files has it. Titles are like
/// "Report.docx - Word" or "notes - Notepad", so each part is tried, with or without its extension.
fn recent_file(title: &str) -> Option<PathBuf> {
    let recent = std::env::var_os("APPDATA").map(PathBuf::from)?.join("Microsoft").join("Windows").join("Recent");
    let candidates = title_parts(title);
    if candidates.is_empty() {
        return None;
    }
    // A shortcut is named for its file, with or without the extension, as Explorer shows names.
    let mut matches = Vec::new();
    for entry in std::fs::read_dir(&recent).ok()?.flatten() {
        let name = entry.file_name().to_string_lossy().to_lowercase();
        let Some(shown) = name.strip_suffix(".lnk") else { continue };
        if !candidates.iter().any(|candidate| names_match(candidate, shown)) {
            continue;
        }
        let modified = entry.metadata().and_then(|meta| meta.modified()).unwrap_or(std::time::UNIX_EPOCH);
        matches.push((modified, entry.path()));
    }
    // Newest first. The file itself must have the name the title gave, so that "report.docx"
    // in a title can't be taken for report.pdf, whose shortcut is also "report".
    matches.sort_by(|a, b| b.0.cmp(&a.0));
    matches.into_iter().take(4).find_map(|(_, shortcut)| {
        let target = shortcut_target(&shortcut)?;
        let name = target.file_name()?.to_string_lossy().to_lowercase();
        let fits = candidates.iter().any(|candidate| *candidate == name || (!candidate.contains('.') && names_match(candidate, &name)));
        (fits && target.is_file()).then_some(target)
    })
}

/// Whether a name from a title and a file's name are the same, one perhaps without its extension.
fn names_match(title: &str, file: &str) -> bool {
    let stem = |name: &str| name.rsplit_once('.').map_or(name.to_string(), |(stem, _)| stem.to_string());
    title == file || stem(title) == file || title == stem(file)
}

/// The parts of a title that could be a file's name, lowercase, without the marks editors add
/// for unsaved changes.
pub fn title_parts(title: &str) -> Vec<String> {
    title
        .split(" - ")
        .map(|part| part.trim().trim_start_matches(['*', '●', '•']).trim_end_matches(['*', '●', '•']).trim().to_lowercase())
        .filter(|part| !part.is_empty() && part.len() <= 200)
        .collect()
}

pub fn shortcut_target(shortcut: &Path) -> Option<PathBuf> {
    use windows::Win32::System::Com::{IPersistFile, STGM_READ};
    use windows::Win32::UI::Shell::{IShellLinkW, ShellLink};
    use windows::core::PCWSTR;
    unsafe {
        let link: IShellLinkW = CoCreateInstance(&ShellLink, None, CLSCTX_INPROC_SERVER).ok()?;
        let file: IPersistFile = link.cast().ok()?;
        let wide = crate::util::wide_path(shortcut);
        file.Load(PCWSTR(wide.as_ptr()), STGM_READ).ok()?;
        let mut buffer = [0u16; 1024];
        link.GetPath(&mut buffer, std::ptr::null_mut(), 0).ok()?;
        let len = buffer.iter().position(|&unit| unit == 0).unwrap_or(buffer.len());
        (len > 0).then(|| PathBuf::from(String::from_utf16_lossy(&buffer[..len])))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_browser_title_loses_the_browsers_name() {
        assert_eq!(page_title("Inbox (3) - me@example.com - Gmail — Zen Browser"), "Inbox (3) - me@example.com - Gmail");
        assert_eq!(page_title("GitHub - Google Chrome"), "GitHub");
        assert_eq!(page_title("Plain"), "Plain");
    }

    #[test]
    fn a_title_names_its_file_in_parts() {
        assert_eq!(title_parts("*notes.txt - Notepad"), ["notes.txt", "notepad"]);
        assert_eq!(title_parts("● main.rs - orbit - Visual Studio Code"), ["main.rs", "orbit", "visual studio code"]);
        assert!(title_parts("").is_empty());
    }

    /// Reads every visible window as Create Quicklink would, without focusing any. Prints only
    /// what kind of suggestion each gave, never the address or path itself.
    #[test]
    #[ignore = "reads the windows open on this PC"]
    fn live_windows() {
        use windows::Win32::Foundation::{LPARAM, TRUE};
        use windows::Win32::UI::WindowsAndMessaging::{EnumWindows, IsWindowVisible};
        unsafe extern "system" fn each(window: HWND, list: LPARAM) -> windows::core::BOOL {
            let list = unsafe { &mut *(list.0 as *mut Vec<isize>) };
            if unsafe { IsWindowVisible(window) }.as_bool() && !window_text(window).is_empty() {
                list.push(window.0 as isize);
            }
            TRUE
        }
        let _ = unsafe { CoInitializeEx(None, COINIT_APARTMENTTHREADED) };
        let mut windows = Vec::<isize>::new();
        let _ = unsafe { EnumWindows(Some(each), LPARAM(&mut windows as *mut _ as isize)) };
        for window in windows {
            let started = std::time::Instant::now();
            let found = suggest(HWND(window as *mut _));
            let kind = match &found {
                None => "nothing".to_string(),
                Some(found) if found.target.starts_with("http") => format!("web page, named: {}", !found.name.is_empty()),
                Some(found) if Path::new(&found.target).is_dir() => "folder".to_string(),
                Some(found) if !found.target.is_empty() => format!("file, opens with a known program: {}", !found.program.is_empty()),
                Some(_) => "app only".to_string(),
            };
            let from = found.as_ref().map_or("", |found| found.from.as_str()).to_string();
            println!("{:>5} ms  {kind}  (from {from})", started.elapsed().as_millis());
        }
    }

    /// Titles made from this PC's recent files, as an editor would show them, find those files.
    #[test]
    #[ignore = "reads this PC's recent files"]
    fn live_recent_files() {
        let _ = unsafe { CoInitializeEx(None, COINIT_APARTMENTTHREADED) };
        let recent = PathBuf::from(std::env::var_os("APPDATA").unwrap()).join(r"Microsoft\Windows\Recent");
        let (mut tried, mut found) = (0, 0);
        for entry in std::fs::read_dir(recent).unwrap().flatten().take(40) {
            let Some(target) = shortcut_target(&entry.path()).filter(|target| target.is_file()) else { continue };
            let title = format!("{} - Some Editor", target.file_name().unwrap().to_string_lossy());
            tried += 1;
            if recent_file(&title).is_some() {
                found += 1;
            }
        }
        println!("recent files tried: {tried}, found from their titles: {found}");
        assert_eq!(tried, found);
    }

    #[test]
    fn a_files_name_matches_with_or_without_its_extension() {
        assert!(names_match("report.docx", "report"));
        assert!(names_match("notes", "notes.txt"));
        assert!(names_match("notes.txt", "notes.txt"));
        assert!(!names_match("notes", "other.txt"));
    }

    #[test]
    fn a_browsers_own_pages_are_not_suggested() {
        assert!(is_internal("chrome://settings") && is_internal("about:newtab") && !is_internal("https://example.com"));
    }
}
