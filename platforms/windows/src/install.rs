//! Installing and removing Orbit, for Orbit Setup (`setup.rs`).
//!
//! Orbit installs itself: `Orbit-<version>-Setup.exe` is orbit.exe under another name, and copies
//! itself into `%LOCALAPPDATA%\Programs\Orbit`. It installs for the current user only, so Windows
//! never asks for an administrator. It writes the same entries a setup program would: a Start menu
//! shortcut, an optional desktop shortcut, the sign-in entry Orbit's own switch writes, and an entry
//! in Settings → Apps whose Uninstall runs `orbit.exe --uninstall`.
//!
//! Version 1.0.0 was installed by Inno Setup, into the same folder. Installing over it takes over
//! its entry and removes its uninstaller, so there's one Orbit in Settings → Apps.

use crate::util::wide;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};
use windows::Win32::System::Registry::{
    HKEY, HKEY_CURRENT_USER, KEY_SET_VALUE, REG_DWORD, REG_OPTION_NON_VOLATILE, REG_SZ, RRF_RT_REG_EXPAND_SZ, RRF_RT_REG_SZ, RegCloseKey,
    RegCreateKeyExW, RegDeleteTreeW, RegDeleteValueW, RegGetValueW, RegOpenKeyExW, RegSetValueExW,
};
use windows::core::PCWSTR;

/// Never change this: Windows knows the installed app by it. It's the AppId 1.0.0's installer used.
pub const APP_ID: &str = "{2124CE11-6877-42C6-B33E-D4C37E050DE4}";
pub const VERSION: &str = env!("CARGO_PKG_VERSION");
const UNINSTALL_ROOT: &str = r"Software\Microsoft\Windows\CurrentVersion\Uninstall";
pub const RUN_KEY: &str = r"Software\Microsoft\Windows\CurrentVersion\Run";
pub const RUN_VALUE: &str = "Orbit";
const EXE: &str = "orbit.exe";
const PUBLISHER: &str = "Beroris";
const WEBSITE: &str = "https://github.com/Beroris/orbit";

/// Files that go beside orbit.exe. They're built into it, so Setup is one file.
const DOCUMENTS: &[(&str, &str)] = &[
    ("PRIVACY.md", include_str!("../../../docs/PRIVACY.md")),
    ("THIRD_PARTY_NOTICES.md", include_str!("../../../docs/THIRD_PARTY_NOTICES.md")),
    ("EXTENSIONS.md", include_str!("../../../docs/EXTENSIONS.md")),
    ("Geist-LICENSE.txt", include_str!("../../../assets/fonts/Geist-LICENSE.txt")),
];

fn uninstall_key() -> String {
    format!(r"{UNINSTALL_ROOT}\{APP_ID}")
}

/// 1.0.0's entry, written by Inno Setup.
fn inno_key() -> String {
    format!(r"{UNINSTALL_ROOT}\{APP_ID}_is1")
}

pub fn read_string(root: HKEY, path: &str, name: &str) -> Option<String> {
    let path = wide(path);
    let name = wide(name);
    let mut buffer = vec![0u16; 2048];
    let mut size = (buffer.len() * 2) as u32;
    let status = unsafe {
        RegGetValueW(
            root,
            PCWSTR(path.as_ptr()),
            if name.len() > 1 { PCWSTR(name.as_ptr()) } else { PCWSTR::null() },
            RRF_RT_REG_SZ | RRF_RT_REG_EXPAND_SZ,
            None,
            Some(buffer.as_mut_ptr() as *mut _),
            Some(&mut size),
        )
    };
    if status.is_err() {
        return None;
    }
    let length = buffer.iter().position(|&unit| unit == 0).unwrap_or(0);
    Some(String::from_utf16_lossy(&buffer[..length]))
}

/// Opens or makes a key under HKEY_CURRENT_USER for writing.
fn writable(path: &str) -> Option<HKEY> {
    let path = wide(path);
    let mut key = HKEY::default();
    let status = unsafe { RegCreateKeyExW(HKEY_CURRENT_USER, PCWSTR(path.as_ptr()), None, PCWSTR::null(), REG_OPTION_NON_VOLATILE, KEY_SET_VALUE, None, &mut key, None) };
    status.is_ok().then_some(key)
}

fn set_string(key: HKEY, name: &str, value: &str) -> bool {
    let name = wide(name);
    let text = wide(value);
    let bytes = unsafe { std::slice::from_raw_parts(text.as_ptr() as *const u8, text.len() * 2) };
    unsafe { RegSetValueExW(key, PCWSTR(name.as_ptr()), None, REG_SZ, Some(bytes)) }.is_ok()
}

fn set_dword(key: HKEY, name: &str, value: u32) -> bool {
    let name = wide(name);
    unsafe { RegSetValueExW(key, PCWSTR(name.as_ptr()), None, REG_DWORD, Some(&value.to_le_bytes())) }.is_ok()
}

fn delete_value(path: &str, name: &str) {
    let path = wide(path);
    let name = wide(name);
    let mut key = HKEY::default();
    unsafe {
        if RegOpenKeyExW(HKEY_CURRENT_USER, PCWSTR(path.as_ptr()), None, KEY_SET_VALUE, &mut key).is_ok() {
            let _ = RegDeleteValueW(key, PCWSTR(name.as_ptr()));
            let _ = RegCloseKey(key);
        }
    }
}

fn delete_key(path: &str) {
    let path = wide(path);
    unsafe {
        let _ = RegDeleteTreeW(HKEY_CURRENT_USER, PCWSTR(path.as_ptr()));
    }
}

/// Where Orbit installs: `%LOCALAPPDATA%\Programs\Orbit`.
pub fn install_dir() -> PathBuf {
    let local = std::env::var_os("LOCALAPPDATA").map(PathBuf::from).unwrap_or_else(|| crate::util::default_data_dir().parent().map(Path::to_path_buf).unwrap_or_default());
    local.join("Programs").join("Orbit")
}

pub fn installed_exe() -> PathBuf {
    install_dir().join(EXE)
}

/// The data folder an installed Orbit uses.
pub fn data_dir() -> PathBuf {
    crate::util::default_data_dir()
}

fn start_menu_shortcut() -> Option<PathBuf> {
    Some(PathBuf::from(std::env::var_os("APPDATA")?).join(r"Microsoft\Windows\Start Menu\Programs\Orbit.lnk"))
}

fn desktop_shortcut() -> Option<PathBuf> {
    crate::util::known_folder(&windows::Win32::UI::Shell::FOLDERID_Desktop).map(|desktop| desktop.join("Orbit.lnk"))
}

/// What's on this PC now.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Existing {
    /// The version installed, if any.
    pub version: Option<String>,
    /// Windows starts the installed copy at sign-in.
    pub starts_at_sign_in: bool,
    pub desktop_shortcut: bool,
    /// Installed by 1.0.0's Inno Setup installer.
    pub by_inno: bool,
}

/// Whether the sign-in entry starts this exe.
pub fn run_entry_points_at(exe: &Path) -> bool {
    read_string(HKEY_CURRENT_USER, RUN_KEY, RUN_VALUE).is_some_and(|command| same_path(command.trim().trim_matches('"'), exe))
}

fn same_path(text: &str, path: &Path) -> bool {
    text.eq_ignore_ascii_case(&path.to_string_lossy())
}

pub fn existing() -> Existing {
    let exe = installed_exe();
    let ours = read_string(HKEY_CURRENT_USER, &uninstall_key(), "DisplayVersion");
    let inno = read_string(HKEY_CURRENT_USER, &inno_key(), "DisplayVersion");
    let by_inno = ours.is_none() && inno.is_some();
    let version = ours.or(inno).filter(|_| exe.is_file());
    Existing {
        starts_at_sign_in: run_entry_points_at(&exe),
        desktop_shortcut: desktop_shortcut().is_some_and(|link| link.is_file()),
        by_inno,
        version,
    }
}

/// Compares "1.2.10" with "1.10.0" as versions, not as text.
pub fn compare_versions(a: &str, b: &str) -> std::cmp::Ordering {
    let parts = |text: &str| -> Vec<u64> { text.split(['.', '-', '+']).map(|part| part.parse().unwrap_or(0)).collect() };
    let (a, b) = (parts(a), parts(b));
    for at in 0..a.len().max(b.len()) {
        let order = a.get(at).copied().unwrap_or(0).cmp(&b.get(at).copied().unwrap_or(0));
        if order.is_ne() {
            return order;
        }
    }
    std::cmp::Ordering::Equal
}

/// What the person chose in Setup.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Choices {
    pub start_at_sign_in: bool,
    pub desktop_shortcut: bool,
}

/// Where an install has got to, for the progress bar.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Step {
    Closing,
    Copying,
    Shortcuts,
    Registering,
    Done,
}

impl Step {
    pub fn label(self) -> &'static str {
        match self {
            Step::Closing => "Closing Orbit\u{2026}",
            Step::Copying => "Copying Orbit\u{2026}",
            Step::Shortcuts => "Adding it to Start\u{2026}",
            Step::Registering => "Adding it to Settings \u{2192} Apps\u{2026}",
            Step::Done => "Done",
        }
    }

    pub fn fraction(self) -> f32 {
        match self {
            Step::Closing => 0.1,
            Step::Copying => 0.45,
            Step::Shortcuts => 0.75,
            Step::Registering => 0.9,
            Step::Done => 1.0,
        }
    }
}

/// Asks the installed Orbit to quit, as its own Quit does, so its keyboard hook and hotkeys let go
/// and its file can be replaced. True if none is left running.
pub fn quit_running() -> bool {
    crate::instance::quit(&data_dir())
}

/// Copies `from` over `to`. A running exe can't be overwritten, but it can be renamed, so if Orbit
/// is somehow still running, the old file is moved aside and goes at the next install.
fn replace_file(from: &Path, to: &Path) -> std::io::Result<()> {
    let fresh = to.with_extension("exe.new");
    std::fs::copy(from, &fresh)?;
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        match std::fs::rename(&fresh, to) {
            Ok(()) => return Ok(()),
            Err(error) if Instant::now() > deadline => {
                let aside = to.with_extension("exe.old");
                let _ = std::fs::remove_file(&aside);
                if std::fs::rename(to, &aside).is_ok() && std::fs::rename(&fresh, to).is_ok() {
                    return Ok(());
                }
                let _ = std::fs::remove_file(&fresh);
                return Err(error);
            }
            Err(_) => std::thread::sleep(Duration::from_millis(150)),
        }
    }
}

fn folder_size_kb(dir: &Path) -> u32 {
    let bytes: u64 = std::fs::read_dir(dir).map(|entries| entries.flatten().filter_map(|entry| entry.metadata().ok()).map(|meta| meta.len()).sum()).unwrap_or(0);
    (bytes / 1024) as u32
}

fn today() -> String {
    let time = unsafe { windows::Win32::System::SystemInformation::GetLocalTime() };
    format!("{:04}{:02}{:02}", time.wYear, time.wMonth, time.wDay)
}

/// Installs or updates Orbit from the running exe. `step` hears where it's got to.
pub fn install(choices: Choices, mut step: impl FnMut(Step)) -> Result<(), String> {
    let source = std::env::current_exe().map_err(|_| "Setup couldn't find its own file.".to_string())?;
    let dir = install_dir();
    let exe = dir.join(EXE);

    step(Step::Closing);
    quit_running();

    step(Step::Copying);
    std::fs::create_dir_all(&dir).map_err(|e| format!("Couldn't make {}: {e}", dir.display()))?;
    if !same_path(&source.to_string_lossy(), &exe) {
        replace_file(&source, &exe).map_err(|e| format!("Couldn't copy Orbit into {}: {e}", dir.display()))?;
    }
    for (name, text) in DOCUMENTS {
        let _ = crate::util::write_atomic(&dir.join(name), text.as_bytes());
    }
    // 1.0.0's uninstaller, and an exe a previous update had to move aside.
    for leftover in ["unins000.exe", "unins000.dat", "unins000.msg", "orbit.exe.old"] {
        let _ = std::fs::remove_file(dir.join(leftover));
    }
    delete_key(&inno_key());

    step(Step::Shortcuts);
    if let Some(link) = start_menu_shortcut() {
        let _ = make_shortcut(&link, &exe, "Open the launcher");
    }
    if let Some(link) = desktop_shortcut() {
        if choices.desktop_shortcut {
            let _ = make_shortcut(&link, &exe, "Open the launcher");
        } else if shortcut_points_at(&link, &exe) {
            let _ = std::fs::remove_file(link);
        }
    }

    step(Step::Registering);
    // The same entry Orbit's own "Start Orbit when I sign in" switch writes, so the two agree.
    if choices.start_at_sign_in {
        if let Some(key) = writable(RUN_KEY) {
            set_string(key, RUN_VALUE, &format!("\"{}\"", exe.display()));
            unsafe {
                let _ = RegCloseKey(key);
            }
        }
    } else if run_entry_points_at(&exe) {
        delete_value(RUN_KEY, RUN_VALUE);
    }
    let key = writable(&uninstall_key()).ok_or("Couldn't add Orbit to Settings \u{2192} Apps.")?;
    let quoted = format!("\"{}\"", exe.display());
    set_string(key, "DisplayName", "Orbit");
    set_string(key, "DisplayVersion", VERSION);
    set_string(key, "DisplayIcon", &format!("{},0", exe.display()));
    set_string(key, "Publisher", PUBLISHER);
    set_string(key, "Comments", "A fast, keyboard-first launcher.");
    set_string(key, "URLInfoAbout", WEBSITE);
    set_string(key, "HelpLink", &format!("{WEBSITE}/issues"));
    set_string(key, "InstallLocation", &dir.display().to_string());
    set_string(key, "InstallDate", &today());
    set_string(key, "UninstallString", &format!("{quoted} --uninstall"));
    set_string(key, "QuietUninstallString", &format!("{quoted} --uninstall --silent"));
    set_dword(key, "NoModify", 1);
    set_dword(key, "NoRepair", 1);
    set_dword(key, "EstimatedSize", folder_size_kb(&dir));
    unsafe {
        let _ = RegCloseKey(key);
    }
    step(Step::Done);
    Ok(())
}

/// Starts the installed Orbit. `show` opens the launcher once it's up; `updated` has it say
/// it's a new version.
pub fn launch_installed(show: bool, updated: bool) -> bool {
    let exe = installed_exe();
    let mut command = std::process::Command::new(&exe);
    if show {
        command.arg("--show");
    }
    if updated {
        command.arg(crate::update::UPDATED_ARG);
    }
    command.current_dir(install_dir()).spawn().is_ok()
}

/// Removes the installed Orbit: closes it, takes away its sign-in entry, shortcuts and Settings →
/// Apps entry, and deletes its folder. The data folder goes too only if `remove_data`.
/// The running exe can't delete itself, so this runs from a copy in the temp folder.
pub fn uninstall(remove_data: bool, mut step: impl FnMut(Step)) -> Result<(), String> {
    let dir = install_dir();
    let exe = dir.join(EXE);
    step(Step::Closing);
    quit_running();
    // Only Orbit's own entry. A copy of Orbit kept elsewhere keeps its own.
    if run_entry_points_at(&exe) {
        delete_value(RUN_KEY, RUN_VALUE);
    }
    step(Step::Shortcuts);
    // Only shortcuts to the installed copy, even if it's already gone: one someone made to a copy
    // of Orbit elsewhere stays.
    for link in [start_menu_shortcut(), desktop_shortcut()].into_iter().flatten() {
        if shortcut_points_at(&link, &exe) {
            let _ = std::fs::remove_file(&link);
        }
    }
    step(Step::Registering);
    delete_key(&uninstall_key());
    delete_key(&inno_key());
    step(Step::Copying);
    let deadline = Instant::now() + Duration::from_secs(10);
    for (name, _) in DOCUMENTS {
        let _ = std::fs::remove_file(dir.join(name));
    }
    for name in [EXE, "orbit.exe.old", "orbit.exe.new", "unins000.exe", "unins000.dat", "unins000.msg"] {
        let file = dir.join(name);
        if !file.exists() || std::fs::remove_file(&file).is_ok() {
            continue;
        }
        // A running exe can't be deleted, but it can be moved. The installed exe is running when
        // it started this uninstall and is waiting to hear how it went, so it goes to the temp
        // folder and is deleted there once it has exited.
        let aside = std::env::temp_dir().join(format!("Orbit-removed-{}-{name}", std::process::id()));
        if std::fs::rename(&file, &aside).is_ok() {
            delete_later(&aside);
            continue;
        }
        while file.exists() && std::fs::remove_file(&file).is_err() && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(200));
        }
    }
    // Only if that left it empty: anything else in it isn't Orbit's to delete.
    let _ = std::fs::remove_dir(&dir);
    if remove_data {
        let data = data_dir();
        if data.is_dir() {
            std::fs::remove_dir_all(&data).map_err(|e| format!("Orbit is uninstalled, but some of its data couldn't be deleted: {e}"))?;
        }
    }
    step(Step::Done);
    if exe.exists() {
        return Err(format!("Orbit is uninstalled, but {} couldn't be deleted. Delete it once Orbit has closed.", exe.display()));
    }
    Ok(())
}

/// An uninstall started from the installed exe copies itself to the temp folder and runs from
/// there, so the installed exe can be deleted. Returns the copy's path.
pub fn copy_to_temp() -> Option<PathBuf> {
    let source = std::env::current_exe().ok()?;
    let target = std::env::temp_dir().join(format!("{UNINSTALL_COPY}{}.exe", std::process::id()));
    std::fs::copy(&source, &target).ok()?;
    Some(target)
}

/// How `copy_to_temp` names its copy, so that copy alone deletes itself afterwards.
const UNINSTALL_COPY: &str = "Orbit-uninstall-";

fn is_uninstall_copy(exe: &Path, temp: &Path) -> bool {
    let folder = |path: &Path| path.to_string_lossy().trim_end_matches('\\').to_lowercase();
    let named = exe.file_name().is_some_and(|name| name.to_string_lossy().to_lowercase().starts_with(&UNINSTALL_COPY.to_lowercase()));
    named && exe.parent().is_some_and(|dir| folder(dir) == folder(temp))
}

/// Whether this exe is the installed one.
pub fn running_installed() -> bool {
    std::env::current_exe().is_ok_and(|exe| same_path(&exe.to_string_lossy(), &installed_exe()))
}

/// The copy `copy_to_temp` made deletes itself once it has exited. Any other exe is left alone.
pub fn delete_self_later() {
    let Ok(exe) = std::env::current_exe() else { return };
    if is_uninstall_copy(&exe, &std::env::temp_dir()) {
        delete_later(&exe);
    }
}

/// Deletes a file that's in use, once it isn't: `cmd` tries every second or so, for up to a
/// minute, so the program running from it has time to exit.
fn delete_later(file: &Path) {
    use std::os::windows::process::CommandExt;
    let file = file.display();
    let command = format!("for /l %i in (1,1,60) do @(del /f /q \"{file}\" >nul 2>&1 & if exist \"{file}\" (ping -n 2 127.0.0.1 >nul) else (exit /b))");
    let _ = std::process::Command::new("cmd.exe").args(["/d", "/c"]).raw_arg(command).creation_flags(0x0800_0000).spawn();
}

fn make_shortcut(link: &Path, target: &Path, description: &str) -> windows::core::Result<()> {
    use windows::Win32::System::Com::{CLSCTX_INPROC_SERVER, CoCreateInstance, IPersistFile};
    use windows::Win32::UI::Shell::{IShellLinkW, ShellLink};
    use windows::core::Interface;
    unsafe {
        let shell_link: IShellLinkW = CoCreateInstance(&ShellLink, None, CLSCTX_INPROC_SERVER)?;
        let target_text = wide(&target.to_string_lossy());
        shell_link.SetPath(PCWSTR(target_text.as_ptr()))?;
        if let Some(folder) = target.parent() {
            let folder = wide(&folder.to_string_lossy());
            shell_link.SetWorkingDirectory(PCWSTR(folder.as_ptr()))?;
        }
        let description = wide(description);
        shell_link.SetDescription(PCWSTR(description.as_ptr()))?;
        shell_link.SetIconLocation(PCWSTR(target_text.as_ptr()), 0)?;
        if let Some(parent) = link.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        let file: IPersistFile = shell_link.cast()?;
        let link_text = wide(&link.to_string_lossy());
        file.Save(PCWSTR(link_text.as_ptr()), true)
    }
}

/// Whether a shortcut starts this exe.
fn shortcut_points_at(link: &Path, exe: &Path) -> bool {
    link.is_file() && crate::foreground::shortcut_target(link).is_some_and(|target| same_path(&target.to_string_lossy(), exe))
}

/// Silent: `/S`, `/SILENT`, `/VERYSILENT` (what Inno Setup took) or `--silent`.
pub fn is_silent_flag(arg: &str) -> bool {
    let lower = arg.to_ascii_lowercase();
    matches!(lower.as_str(), "/s" | "/silent" | "/verysilent" | "--silent" | "-s")
}

/// Whether the 1.0.0 installer's sign-in setting is there to keep on a silent update.
pub fn keep_choices(existing: &Existing) -> Choices {
    Choices { start_at_sign_in: existing.version.is_none() || existing.starts_at_sign_in, desktop_shortcut: existing.desktop_shortcut }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cmp::Ordering;

    #[test]
    fn versions_compare_as_numbers() {
        assert_eq!(compare_versions("1.10.0", "1.2.10"), Ordering::Greater);
        assert_eq!(compare_versions("1.0.0", "1.0"), Ordering::Equal);
        assert_eq!(compare_versions("1.0.0", "1.1.0"), Ordering::Less);
        assert_eq!(compare_versions("2.0.0-beta", "1.9.9"), Ordering::Greater);
    }

    #[test]
    fn silent_flags_are_the_usual_ones() {
        for flag in ["/S", "/SILENT", "/VERYSILENT", "/verysilent", "--silent"] {
            assert!(is_silent_flag(flag), "{flag}");
        }
        assert!(!is_silent_flag("/SUPPRESSMSGBOXES"));
    }

    #[test]
    fn a_silent_update_keeps_what_was_chosen() {
        let fresh = keep_choices(&Existing::default());
        assert!(fresh.start_at_sign_in, "a new install starts at sign-in, as Setup's switch defaults to");
        assert!(!fresh.desktop_shortcut);
        let installed = Existing { version: Some("1.0.0".into()), starts_at_sign_in: false, desktop_shortcut: true, by_inno: true };
        assert_eq!(keep_choices(&installed), Choices { start_at_sign_in: false, desktop_shortcut: true });
    }

    #[test]
    fn the_exe_says_its_own_version() {
        let rc = include_str!("../res/orbit.rc");
        let numbers: Vec<&str> = VERSION.split(['.', '-']).take(3).collect();
        let commas = format!("{},0", numbers.join(","));
        assert!(rc.contains(&format!("\"FileVersion\", \"{VERSION}\"")), "res/orbit.rc should say {VERSION}");
        assert!(rc.contains(&format!("FILEVERSION {commas}")), "res/orbit.rc should say {commas}");
    }

    #[test]
    fn the_install_folder_is_per_user() {
        let dir = install_dir();
        assert!(dir.ends_with("Programs\\Orbit"), "{}", dir.display());
    }

    #[test]
    fn only_the_uninstall_copy_deletes_itself() {
        let temp = Path::new(r"C:\Users\You\AppData\Local\Temp\");
        assert!(is_uninstall_copy(Path::new(r"C:\Users\You\AppData\Local\Temp\Orbit-uninstall-4120.exe"), temp));
        assert!(is_uninstall_copy(Path::new(r"c:\users\you\appdata\local\temp\orbit-uninstall-4120.exe"), temp));
        assert!(!is_uninstall_copy(Path::new(r"C:\Users\You\AppData\Local\Temp\orbit.exe"), temp), "an Orbit someone ran from the temp folder");
        assert!(!is_uninstall_copy(Path::new(r"C:\Users\You\AppData\Local\Temp\Downloads\Orbit-uninstall-1.exe"), temp));
        assert!(!is_uninstall_copy(Path::new(r"C:\Users\You\AppData\Local\Programs\Orbit\orbit.exe"), temp));
    }
}
