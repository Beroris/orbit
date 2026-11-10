//! Finding the other launchers on this PC and reading what they keep, for the Import page in
//! Orbit Setup. The reading itself is `orbit_core::import`; this knows where the files are.
//!
//! Nothing here writes anywhere, and the other launchers' files are only read, never changed.

use orbit_core::import::{self, Haul, ReadError};
use std::path::{Path, PathBuf};

/// No settings file a launcher keeps is anywhere near this big.
const MAX_FILE_BYTES: u64 = 16 * 1024 * 1024;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Launcher {
    Raycast,
    FlowLauncher,
    Wox,
    PowerToysRun,
    CommandPalette,
    Keypirinha,
}

impl Launcher {
    pub fn name(self) -> &'static str {
        match self {
            Launcher::Raycast => "Raycast",
            Launcher::FlowLauncher => "Flow Launcher",
            Launcher::Wox => "Wox",
            Launcher::PowerToysRun => "PowerToys Run",
            Launcher::CommandPalette => "Command Palette",
            Launcher::Keypirinha => "Keypirinha",
        }
    }

    /// The program's file name, to tell whether it's running.
    pub fn process(self) -> &'static [&'static str] {
        match self {
            Launcher::Raycast => &["raycast.exe"],
            Launcher::FlowLauncher => &["flow.launcher.exe"],
            Launcher::Wox => &["wox.exe"],
            Launcher::PowerToysRun => &["powertoys.powerlauncher.exe"],
            Launcher::CommandPalette => &["microsoft.cmdpal.ui.exe"],
            Launcher::Keypirinha => &["keypirinha.exe", "keypirinha-x64.exe"],
        }
    }
}

/// A launcher found on this PC.
#[derive(Clone, Debug, PartialEq)]
pub struct Found {
    pub launcher: Launcher,
    /// What it has. Raycast keeps its data encrypted, so it has nothing until an export is chosen.
    pub haul: Haul,
    /// Raycast only: the newest export file in Downloads, Documents or on the desktop.
    pub export: Option<PathBuf>,
}

impl Found {
    /// Whether there's anything to bring over now, or a file to choose first.
    pub fn has_things(&self) -> bool {
        !self.haul.links.is_empty() || !self.haul.snippets.is_empty() || !self.haul.notes.is_empty()
    }
}

fn read_text(path: &Path) -> Option<String> {
    if std::fs::metadata(path).ok()?.len() > MAX_FILE_BYTES {
        return None;
    }
    let bytes = std::fs::read(path).ok()?;
    Some(String::from_utf8_lossy(&bytes).into_owned())
}

fn env_dir(name: &str) -> Option<PathBuf> {
    std::env::var_os(name).map(PathBuf::from)
}

/// Every launcher Orbit knows that's on this PC, in the order the Import page lists them.
pub fn detect() -> Vec<Found> {
    let roaming = env_dir("APPDATA");
    let local = env_dir("LOCALAPPDATA");
    let home = env_dir("USERPROFILE");
    let mut found = Vec::new();
    if let Some(raycast) = raycast(local.as_deref()) {
        found.push(raycast);
    }
    if let Some(flow) = flow_launcher(roaming.as_deref(), local.as_deref()) {
        found.push(flow);
    }
    if let Some(wox) = wox(roaming.as_deref(), home.as_deref()) {
        found.push(wox);
    }
    if let Some(local) = local.as_deref() {
        let run = local.join(r"Microsoft\PowerToys\PowerToys Run\settings.json");
        if let Some(text) = read_text(&run) {
            found.push(Found { launcher: Launcher::PowerToysRun, haul: import::powertoys_run(&text), export: None });
        }
        let palette = local.join(r"Packages\Microsoft.CommandPalette_8wekyb3d8bbwe\LocalState");
        if palette.is_dir() {
            let mut haul = read_text(&palette.join("settings.json")).map(|text| import::command_palette_settings(&text)).unwrap_or_default();
            if let Some(text) = read_text(&palette.join("bookmarks.json")) {
                haul.take(import::command_palette_bookmarks(&text));
            }
            found.push(Found { launcher: Launcher::CommandPalette, haul, export: None });
        }
    }
    if let Some(roaming) = roaming.as_deref() {
        let user = roaming.join(r"Keypirinha\User");
        if user.is_dir() {
            let main = read_text(&user.join("Keypirinha.ini")).unwrap_or_default();
            let sites = read_text(&user.join("WebSearch.ini")).unwrap_or_default();
            found.push(Found { launcher: Launcher::Keypirinha, haul: import::keypirinha(&main, &sites), export: None });
        }
    }
    found
}

fn raycast(local: Option<&Path>) -> Option<Found> {
    let local = local?;
    let installed = local.join("Raycast").is_dir()
        || std::fs::read_dir(local.join("Packages")).ok()?.flatten().any(|entry| entry.file_name().to_string_lossy().starts_with("Raycast.Raycast_"));
    if !installed {
        return None;
    }
    Some(Found { launcher: Launcher::Raycast, haul: Haul::default(), export: newest_raycast_export() })
}

/// The newest `.rayconfig` file where Raycast's Export puts one: Downloads, Documents, the desktop.
pub fn newest_raycast_export() -> Option<PathBuf> {
    use windows::Win32::UI::Shell::{FOLDERID_Desktop, FOLDERID_Documents, FOLDERID_Downloads};
    let mut best: Option<(std::time::SystemTime, PathBuf)> = None;
    for folder in [FOLDERID_Downloads, FOLDERID_Documents, FOLDERID_Desktop].iter().filter_map(crate::util::known_folder) {
        let Ok(entries) = std::fs::read_dir(&folder) else { continue };
        for entry in entries.flatten() {
            let path = entry.path();
            if !path.extension().is_some_and(|ext| ext.eq_ignore_ascii_case("rayconfig")) {
                continue;
            }
            let Some(modified) = entry.metadata().ok().and_then(|meta| meta.modified().ok()) else { continue };
            if best.as_ref().is_none_or(|(time, _)| modified > *time) {
                best = Some((modified, path));
            }
        }
    }
    best.map(|(_, path)| path)
}

/// Flow Launcher keeps its settings in AppData, or beside itself in a portable copy.
fn flow_launcher(roaming: Option<&Path>, local: Option<&Path>) -> Option<Found> {
    let mut folders: Vec<PathBuf> = roaming.map(|roaming| roaming.join("FlowLauncher")).into_iter().collect();
    if let Some(local) = local {
        if let Ok(entries) = std::fs::read_dir(local.join("FlowLauncher")) {
            let mut apps: Vec<PathBuf> = entries.flatten().map(|entry| entry.path()).filter(|path| path.file_name().is_some_and(|name| name.to_string_lossy().starts_with("app-"))).collect();
            apps.sort();
            folders.extend(apps.into_iter().rev().map(|app| app.join("UserData")));
        }
    }
    let folder = folders.into_iter().find(|folder| folder.join(r"Settings\Settings.json").is_file())?;
    let settings = folder.join("Settings");
    let mut haul = read_text(&settings.join("Settings.json")).map(|text| import::flow_settings(&text)).unwrap_or_default();
    let plugins = settings.join("Plugins");
    if let Some(text) = read_text(&plugins.join(r"Flow.Launcher.Plugin.WebSearch\Settings.json")) {
        haul.take(import::web_searches(&text));
    }
    if let Some(text) = read_text(&plugins.join(r"Flow.Launcher.Plugin.Explorer\Settings.json")) {
        haul.take(import::folder_links(&text));
    }
    Some(Found { launcher: Launcher::FlowLauncher, haul, export: None })
}

/// Wox 2 keeps its settings in a database in the home folder. Wox 1 kept JSON files in AppData.
fn wox(roaming: Option<&Path>, home: Option<&Path>) -> Option<Found> {
    if let Some(home) = home {
        let wox = home.join(".wox");
        // Wox 2 can be told to keep its data elsewhere, and says where in this file.
        let user = std::fs::read_to_string(wox.join(".userdata.location")).ok().map(|text| PathBuf::from(text.trim())).filter(|path| path.is_dir()).unwrap_or_else(|| wox.join("wox-user"));
        let database = user.join("wox.db");
        if database.is_file() {
            return Some(Found { launcher: Launcher::Wox, haul: wox2(&database).unwrap_or_default(), export: None });
        }
    }
    let settings = roaming?.join(r"Wox\Settings");
    let main = read_text(&settings.join("Settings.json"))?;
    let mut haul = import::flow_settings(&main);
    if let Some(text) = read_text(&settings.join(r"Plugins\Wox.Plugin.WebSearch\Settings.json")) {
        haul.take(import::web_searches(&text));
    }
    if let Some(text) = read_text(&settings.join(r"Plugins\Wox.Plugin.Folder\Settings.json")) {
        haul.take(import::folder_links(&text));
    }
    Some(Found { launcher: Launcher::Wox, haul, export: None })
}

/// Reads Wox 2's settings without changing its database, even while Wox has it open.
fn wox2(database: &Path) -> Option<Haul> {
    use rusqlite::OpenFlags;
    let connection = rusqlite::Connection::open_with_flags(database, OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX).ok()?;
    let _ = connection.busy_timeout(std::time::Duration::from_millis(500));
    let settings: Vec<(String, String)> = connection
        .prepare("SELECT key, value FROM wox_settings")
        .and_then(|mut query| query.query_map([], |row| Ok((row.get(0)?, row.get(1)?)))?.collect())
        .unwrap_or_default();
    let plugins: Vec<(String, String, String)> = connection
        .prepare("SELECT plugin_id, key, value FROM plugin_settings")
        .and_then(|mut query| query.query_map([], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)))?.collect())
        .unwrap_or_default();
    Some(import::wox2(&settings, &plugins))
}

/// A file someone chose to import.
pub enum Chosen {
    /// Something to bring over, and who it's from.
    Haul(Haul, &'static str),
    /// An Orbit export, or Orbit's own settings.json, which Orbit reads as it always has.
    Orbit,
}

/// Reads a chosen file: an export from Raycast, a settings file from another launcher, or Orbit's
/// own export or settings.
pub fn read_file(path: &Path, passphrase: Option<&str>) -> Result<Chosen, ReadError> {
    let size = std::fs::metadata(path).map_err(|e| ReadError::Damaged(e.to_string()))?.len();
    if size > orbit_data::MAX_IMPORT_BYTES {
        return Err(ReadError::Damaged("it's larger than any export".into()));
    }
    let bytes = std::fs::read(path).map_err(|e| ReadError::Damaged(e.to_string()))?;
    if let Ok(value) = serde_json::from_slice::<serde_json::Value>(strip_bom(&bytes)) {
        if value.get("format").and_then(|f| f.as_str()) == Some("orbit-export") {
            return Ok(Chosen::Orbit);
        }
        let named = |name: &str| path.file_name().is_some_and(|file| file.eq_ignore_ascii_case(name));
        if named("settings.json") && value.get("launcher").is_some() {
            return Ok(Chosen::Orbit);
        }
        let text = String::from_utf8_lossy(strip_bom(&bytes));
        if value.get("SearchSources").is_some() {
            return Ok(Chosen::Haul(import::web_searches(&text), "Web Searches"));
        }
        if value.get("QuickAccessLinks").is_some() || value.get("FolderLinks").is_some() {
            return Ok(Chosen::Haul(import::folder_links(&text), "Explorer"));
        }
        if value.get("Hotkey").is_some_and(|hotkey| hotkey.is_string()) {
            return Ok(Chosen::Haul(import::flow_settings(&text), "Flow Launcher"));
        }
        if value.get("Data").is_some_and(|data| data.as_array().is_some_and(|items| items.iter().any(|item| item.get("Bookmark").is_some()))) {
            return Ok(Chosen::Haul(import::command_palette_bookmarks(&text), "Command Palette"));
        }
    }
    import::raycast::read(&bytes, passphrase).map(|haul| Chosen::Haul(haul, "Raycast"))
}

fn strip_bom(bytes: &[u8]) -> &[u8] {
    bytes.strip_prefix(&[0xEF, 0xBB, 0xBF]).unwrap_or(bytes)
}

/// The snippets and notes as an Orbit export, which the data thread knows how to bring in.
/// Snippets go in a category named after where they came from, to be easy to find.
pub fn bundle(haul: &Haul, from: &str, now: i64) -> String {
    let bundle = orbit_data::ExportBundle {
        format: "orbit-export".into(),
        version: 1,
        exported_at: now,
        notes: haul.notes.iter().map(|note| orbit_data::ExportedNote { body: note.body.clone(), created_at: now, updated_at: now, pinned: false }).collect(),
        snippets: haul
            .snippets
            .iter()
            .map(|snippet| orbit_data::ExportedSnippet {
                name: snippet.name.clone(),
                trigger: snippet.keyword.clone(),
                content: snippet.text.clone(),
                description: String::new(),
                category: from.to_string(),
                favourite: false,
            })
            .collect(),
    };
    serde_json::to_string(&bundle).unwrap_or_default()
}

/// The app with this name in Orbit's app list, for a link that opened with it. A Mac app's path
/// counts by its name: `/Applications/Google Chrome.app` is Google Chrome.
pub fn app_named<'a>(apps: &'a [crate::indexer::AppEntry], name: &str) -> Option<&'a crate::indexer::AppEntry> {
    let name = name.trim().trim_end_matches('/');
    let name = name.rsplit(['/', '\\']).next().unwrap_or(name);
    let name = name.strip_suffix(".app").or_else(|| name.strip_suffix(".exe")).unwrap_or(name);
    if name.is_empty() {
        return None;
    }
    apps.iter().find(|app| app.name.eq_ignore_ascii_case(name)).or_else(|| apps.iter().find(|app| Path::new(&app.exe).file_stem().is_some_and(|stem| stem.to_string_lossy().eq_ignore_ascii_case(name))))
}

/// Whether one of the launcher's programs is running.
pub fn is_running(launcher: Launcher) -> bool {
    use windows::Win32::Foundation::CloseHandle;
    use windows::Win32::System::Diagnostics::ToolHelp::{CreateToolhelp32Snapshot, PROCESSENTRY32W, Process32FirstW, Process32NextW, TH32CS_SNAPPROCESS};
    let names = launcher.process();
    unsafe {
        let Ok(snapshot) = CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0) else { return false };
        let mut entry = PROCESSENTRY32W { dwSize: std::mem::size_of::<PROCESSENTRY32W>() as u32, ..Default::default() };
        let mut running = false;
        let mut more = Process32FirstW(snapshot, &mut entry).is_ok();
        while more {
            let end = entry.szExeFile.iter().position(|unit| *unit == 0).unwrap_or(entry.szExeFile.len());
            let exe = String::from_utf16_lossy(&entry.szExeFile[..end]).to_ascii_lowercase();
            if names.contains(&exe.as_str()) {
                running = true;
                break;
            }
            more = Process32NextW(snapshot, &mut entry).is_ok();
        }
        let _ = CloseHandle(snapshot);
        running
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn app(name: &str, exe: &str) -> crate::indexer::AppEntry {
        crate::indexer::AppEntry { id: format!("id-{name}"), name: name.into(), exe: exe.into(), aumid: String::new() }
    }

    #[test]
    fn apps_are_found_by_name_or_mac_path() {
        let apps = vec![app("Google Chrome", r"c:\program files\google\chrome\application\chrome.exe"), app("Zen", r"c:\program files\zen browser\zen.exe")];
        assert_eq!(app_named(&apps, "google chrome").map(|a| a.name.as_str()), Some("Google Chrome"));
        assert_eq!(app_named(&apps, "/Applications/Google Chrome.app").map(|a| a.name.as_str()), Some("Google Chrome"));
        assert_eq!(app_named(&apps, "zen.exe").map(|a| a.name.as_str()), Some("Zen"));
        assert!(app_named(&apps, "Finder").is_none());
        assert!(app_named(&apps, "").is_none());
    }

    #[test]
    fn a_bundle_is_an_orbit_export_the_store_takes() {
        let haul = Haul {
            snippets: vec![import::Snippet { name: "Sig".into(), text: "Thanks".into(), keyword: ";sig".into() }],
            notes: vec![import::Note { body: "# Groceries\n\n- milk".into() }],
            ..Haul::default()
        };
        let json = bundle(&haul, "Raycast", 1_700_000_000);
        let parsed: orbit_data::ExportBundle = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.format, "orbit-export");
        assert_eq!(parsed.snippets[0].trigger, ";sig");
        assert_eq!(parsed.snippets[0].category, "Raycast");
        assert_eq!(parsed.notes[0].body, "# Groceries\n\n- milk");
    }

    #[test]
    fn chosen_files_are_recognised() {
        let dir = std::env::temp_dir().join(format!("orbit-import-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let write = |name: &str, text: &str| {
            let path = dir.join(name);
            std::fs::write(&path, text).unwrap();
            path
        };
        let orbit = write("orbit-export.json", r#"{"format":"orbit-export","version":1,"exported_at":0,"notes":[],"snippets":[]}"#);
        assert!(matches!(read_file(&orbit, None), Ok(Chosen::Orbit)));
        let searches = write("Settings.json", r#"{"SearchSources":[{"Title":"Google","Url":"https://g.example/{q}","Enabled":true}]}"#);
        assert!(matches!(read_file(&searches, None), Ok(Chosen::Haul(haul, "Web Searches")) if haul.links.len() == 1));
        let links = write("quicklinks.json", "\u{feff}[{\"name\":\"Mail\",\"link\":\"https://mail.example\"}]");
        assert!(matches!(read_file(&links, None), Ok(Chosen::Haul(haul, "Raycast")) if haul.links.len() == 1));
        let other = write("notes.txt", "hello");
        assert!(matches!(read_file(&other, None), Err(ReadError::Unknown)));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
