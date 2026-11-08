//! Choosing apps from a list instead of typing program names.
//!
//! A search box and a list. With nothing typed, the list shows the apps already chosen. While
//! typing, it shows matching apps on this PC. What gets saved is still a program name such as
//! `game.exe`, because that's what the hook and the clipboard listener compare against.

use crate::indexer::{AppEntry, icon_hash};
use crate::util::exe_name;
use std::path::{Path, PathBuf};

/// Enough for any search. More would only be rows nobody scrolls to.
const MAX_MATCHES: usize = 100;
/// Windows that say nothing about an app: Orbit's own, and the host of every Microsoft Store app.
const NEVER_OFFERED: [&str; 2] = ["orbit.exe", "applicationframehost.exe"];

/// An app that can be chosen.
#[derive(Clone, Debug, PartialEq)]
pub struct Candidate {
    pub name: String,
    /// Lower-case file name, such as `notepad.exe`.
    pub program: String,
    pub icon: Option<u64>,
}

#[derive(Clone, Debug, PartialEq)]
pub enum Row {
    /// An app, chosen or not.
    App { name: String, program: String, icon: Option<u64>, chosen: bool },
    /// A line of text with nothing to act on.
    Message(String),
    /// Stands in for the rest of a long list, and says how long it is.
    ShowAll(usize),
}

/// How many rows a long list shows until asked for the rest.
const COLLAPSED_ROWS: usize = 5;

/// A long list of what's chosen shows its start, and a row that shows the rest.
pub fn collapsed(mut rows: Vec<Row>) -> Vec<Row> {
    let count = rows.len();
    if count > COLLAPSED_ROWS + 1 {
        rows.truncate(COLLAPSED_ROWS);
        rows.push(Row::ShowAll(count));
    }
    rows
}

/// The name says everything the program's file name would, so showing both is only noise.
pub fn name_says_program(name: &str, program: &str) -> bool {
    let plain: String = name.chars().filter(|c| !c.is_whitespace()).collect::<String>().to_lowercase();
    plain == program.strip_suffix(".exe").unwrap_or(program)
}

/// Apps in the index that name a program, then programs with a window open now. An app with no
/// program path can't be matched against the app in front, so it can't be offered until it's
/// running and its program is known. Programs are unique: choosing one covers every app using it.
pub fn candidates(apps: &[AppEntry], running: &[String]) -> Vec<Candidate> {
    let mut out: Vec<Candidate> = Vec::with_capacity(apps.len() + running.len());
    let mut steam: Option<Vec<PathBuf>> = None;
    for app in apps {
        if app.exe.is_empty() {
            continue;
        }
        // A Steam game's shortcut is an address, which names no program. Its folder does.
        let program = if let Some(id) = steam_game_id(&app.exe) {
            match steam_program(id, steam.get_or_insert_with(steam_libraries)) {
                Some(program) => program,
                None => continue,
            }
        } else if app.exe.contains("://") {
            continue;
        } else {
            exe_name(&app.exe).to_lowercase()
        };
        if !out.iter().any(|c| c.program == program) {
            out.push(Candidate { name: app.name.clone(), program, icon: Some(icon_hash(&app.id)) });
        }
    }
    for path in running {
        let program = exe_name(path).to_lowercase();
        if !NEVER_OFFERED.contains(&program.as_str()) && !out.iter().any(|c| c.program == program) {
            out.push(Candidate { name: name_from_program(&program), program, icon: None });
        }
    }
    out.sort_by_key(|c| c.name.to_lowercase());
    out
}

/// The game in a Steam shortcut such as `steam://rungameid/1172470`.
pub fn steam_game_id(address: &str) -> Option<&str> {
    let id = address.strip_prefix("steam://rungameid/")?.trim_end_matches('/');
    (!id.is_empty() && id.bytes().all(|b| b.is_ascii_digit())).then_some(id)
}

/// Where Steam keeps games: its own folder, and any other library it lists.
pub fn steam_libraries() -> Vec<PathBuf> {
    let Some(steam) = steam_folder() else { return Vec::new() };
    let mut out = vec![steam.clone()];
    if let Ok(text) = std::fs::read_to_string(steam.join("steamapps").join("libraryfolders.vdf")) {
        for path in vdf_values(&text, "path") {
            let path = PathBuf::from(path);
            if !out.iter().any(|known| known.as_os_str().eq_ignore_ascii_case(path.as_os_str())) {
                out.push(path);
            }
        }
    }
    out
}

fn steam_folder() -> Option<PathBuf> {
    use windows::Win32::System::Registry::{HKEY_CURRENT_USER, RRF_RT_REG_SZ, RegGetValueW};
    use windows::core::w;
    let mut buffer = vec![0u16; 1024];
    let mut size = (buffer.len() * 2) as u32;
    let status = unsafe {
        RegGetValueW(HKEY_CURRENT_USER, w!("Software\\Valve\\Steam"), w!("SteamPath"), RRF_RT_REG_SZ, None, Some(buffer.as_mut_ptr() as *mut _), Some(&mut size))
    };
    if status.is_err() {
        return None;
    }
    let len = buffer.iter().position(|&unit| unit == 0).unwrap_or(0);
    let path = String::from_utf16_lossy(&buffer[..len]).replace('/', "\\");
    (!path.is_empty()).then(|| PathBuf::from(path))
}

/// Every value of `key` in Steam's text format, where `\\` stands for one backslash.
fn vdf_values(text: &str, key: &str) -> Vec<String> {
    let quoted = format!("\"{key}\"");
    text.lines()
        .filter_map(|line| {
            let rest = line.trim().strip_prefix(&quoted)?.trim();
            let value = rest.strip_prefix('"')?.strip_suffix('"')?;
            Some(value.replace("\\\\", "\\"))
        })
        .collect()
}

/// Programs in a game's folder that only help it: installers, anti-cheat, crash reporters.
fn is_helper(lower: &str) -> bool {
    const HELPERS: [&str; 14] = [
        "anticheat", "battleye", "beservice", "crash", "report", "unins", "setup", "install", "redist", "prereq", "dxsetup",
        "helper", "launcherpatcher", "dotnet",
    ];
    HELPERS.iter().any(|helper| lower.contains(helper))
}

/// The program a Steam game runs: the largest one in its folder that isn't a helper, looking a few
/// folders down, where engines like Unreal put it. `None` if the game isn't installed.
pub fn steam_program(id: &str, libraries: &[PathBuf]) -> Option<String> {
    let folder = libraries.iter().find_map(|library| {
        let manifest = std::fs::read_to_string(library.join("steamapps").join(format!("appmanifest_{id}.acf"))).ok()?;
        let dir = vdf_values(&manifest, "installdir").into_iter().next()?;
        let folder = library.join("steamapps").join("common").join(dir);
        folder.is_dir().then_some(folder)
    })?;
    largest_program(&folder)
}

fn largest_program(folder: &Path) -> Option<String> {
    const DEEPEST: usize = 4;
    const MOST_FOLDERS: usize = 300;
    let mut best: Option<(u64, String)> = None;
    let mut queue = vec![(folder.to_path_buf(), 0usize)];
    let mut seen = 0;
    while let Some((dir, depth)) = queue.pop() {
        seen += 1;
        if seen > MOST_FOLDERS {
            break;
        }
        let Ok(entries) = std::fs::read_dir(&dir) else { continue };
        for entry in entries.flatten() {
            let name = entry.file_name().to_string_lossy().to_lowercase();
            let Ok(kind) = entry.file_type() else { continue };
            if kind.is_dir() {
                if depth < DEEPEST && !is_helper(&name) && !name.starts_with('_') {
                    queue.push((entry.path(), depth + 1));
                }
            } else if name.ends_with(".exe") && !is_helper(&name) {
                let size = entry.metadata().map_or(0, |meta| meta.len());
                if best.as_ref().is_none_or(|(most, _)| size > *most) {
                    best = Some((size, name));
                }
            }
        }
    }
    best.map(|(_, name)| name)
}

/// Lists saved before Steam games were understood hold the game's number, which matches nothing.
/// Each becomes the game's program. Returns true if anything changed.
pub fn fix_steam_entries(lists: &mut [&mut Vec<String>]) -> bool {
    let wanted = lists.iter().any(|list| list.iter().any(|entry| !entry.is_empty() && entry.bytes().all(|b| b.is_ascii_digit())));
    if !wanted {
        return false;
    }
    let libraries = steam_libraries();
    let mut changed = false;
    for list in lists.iter_mut() {
        let mut fixed: Vec<String> = Vec::with_capacity(list.len());
        for entry in list.iter() {
            let program = (!entry.is_empty() && entry.bytes().all(|b| b.is_ascii_digit())).then(|| steam_program(entry, &libraries)).flatten();
            let entry = program.unwrap_or_else(|| entry.clone());
            if !fixed.iter().any(|known| known.eq_ignore_ascii_case(&entry)) {
                fixed.push(entry);
            }
        }
        if fixed != **list {
            **list = fixed;
            changed = true;
        }
    }
    changed
}

/// How the apps on the default list write their own names.
const KNOWN_NAMES: [(&str, &str); 10] = [
    ("1password", "1Password"),
    ("bitwarden", "Bitwarden"),
    ("keepass", "KeePass"),
    ("keepassxc", "KeePassXC"),
    ("lastpass", "LastPass"),
    ("dashlane", "Dashlane"),
    ("enpass", "Enpass"),
    ("nordpass", "NordPass"),
    ("protonpass", "Proton Pass"),
    ("authy", "Authy"),
];

/// `notepad.exe` reads as `Notepad`.
pub fn name_from_program(program: &str) -> String {
    let stem = program.strip_suffix(".exe").or_else(|| program.strip_suffix(".EXE")).unwrap_or(program);
    if let Some((_, name)) = KNOWN_NAMES.iter().find(|(known, _)| *known == stem) {
        return (*name).to_string();
    }
    let mut chars = stem.chars();
    match chars.next() {
        Some(first) => first.to_uppercase().chain(chars).collect(),
        None => String::new(),
    }
}

/// Lower is better. `None` means no match.
fn score(candidate: &Candidate, query: &str) -> Option<u8> {
    let name = candidate.name.to_lowercase();
    if name.starts_with(query) {
        Some(0)
    } else if name.split(|c: char| !c.is_alphanumeric()).any(|word| word.starts_with(query)) {
        Some(1)
    } else if name.contains(query) {
        Some(2)
    } else if candidate.program.contains(query) {
        Some(3)
    } else {
        None
    }
}

fn is_chosen(chosen: &[String], program: &str) -> bool {
    chosen.iter().any(|entry| entry.eq_ignore_ascii_case(program))
}

/// What typing this would add by hand, for a program that isn't listed.
fn typed_program(query: &str) -> Option<String> {
    let text = query.trim();
    if text.is_empty() || text.contains(['\\', '/', ':', '*', '?', '"', '<', '>', '|']) {
        return None;
    }
    let lower = text.to_lowercase();
    Some(if lower.ends_with(".exe") { lower } else { format!("{lower}.exe") })
}

/// The list to show for what's been typed.
pub fn rows(chosen: &[String], candidates: &[Candidate], query: &str) -> Vec<Row> {
    let query = query.trim().to_lowercase();
    if query.is_empty() {
        let out: Vec<Row> = chosen
            .iter()
            // Names without `.exe` are for macOS, where the same settings file can be used.
            .filter(|entry| entry.to_lowercase().ends_with(".exe"))
            .map(|entry| {
                let program = entry.to_lowercase();
                let known = candidates.iter().find(|c| c.program == program);
                Row::App {
                    name: known.map_or_else(|| name_from_program(&program), |c| c.name.clone()),
                    icon: known.and_then(|c| c.icon),
                    program,
                    chosen: true,
                }
            })
            .collect();
        if out.is_empty() {
            return vec![Row::Message("No apps yet. Search above to add one.".to_string())];
        }
        return out;
    }
    let mut found: Vec<(u8, &Candidate)> = candidates.iter().filter_map(|c| score(c, &query).map(|s| (s, c))).collect();
    found.sort_by_key(|(s, _)| *s);
    found.truncate(MAX_MATCHES);
    let mut out: Vec<Row> = found
        .iter()
        .map(|(_, c)| Row::App { name: c.name.clone(), program: c.program.clone(), icon: c.icon, chosen: is_chosen(chosen, &c.program) })
        .collect();
    if let Some(program) = typed_program(&query) {
        // Offer it by hand only if nothing listed is that program.
        let listed = out.iter().any(|row| matches!(row, Row::App { program: p, .. } if *p == program));
        if !listed && (out.is_empty() || query.ends_with(".exe")) {
            out.push(Row::App { name: format!("Add \u{201C}{program}\u{201D}"), chosen: is_chosen(chosen, &program), program, icon: None });
        }
    }
    if out.is_empty() {
        out.push(Row::Message("No app matches. Use Browse to pick its program.".to_string()));
    }
    out
}

/// Adds a program, or takes it off if it's there. Returns true if it's now chosen.
pub fn toggle(chosen: &mut Vec<String>, program: &str) -> bool {
    if is_chosen(chosen, program) {
        chosen.retain(|entry| !entry.eq_ignore_ascii_case(program));
        false
    } else {
        chosen.push(program.to_string());
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(name: &str, exe: &str) -> AppEntry {
        AppEntry { id: format!("id-{name}"), name: name.to_string(), exe: exe.to_string(), aumid: String::new() }
    }

    fn sample() -> Vec<Candidate> {
        candidates(
            &[
                entry("Notepad", "c:\\windows\\notepad.exe"),
                entry("Visual Studio Code", "c:\\code\\code.exe"),
                entry("Calculator", ""),
                entry("Note Taker", "c:\\t\\taker.exe"),
            ],
            &[
                "C:\\games\\Quake.exe".to_string(),
                "C:\\Windows\\notepad.exe".to_string(),
                "C:\\Windows\\System32\\ApplicationFrameHost.exe".to_string(),
                "C:\\x\\orbit.exe".to_string(),
            ],
        )
    }

    fn names(rows: &[Row]) -> Vec<&str> {
        rows.iter()
            .map(|row| match row {
                Row::App { name, .. } => name.as_str(),
                Row::Message(text) => text.as_str(),
                Row::ShowAll(_) => "show all",
            })
            .collect()
    }

    #[test]
    fn a_long_list_shows_its_start_and_how_many_there_are() {
        let list = |count: usize| -> Vec<Row> { (0..count).map(|at| Row::Message(at.to_string())).collect() };
        assert_eq!(collapsed(list(6)).len(), 6);
        let long = collapsed(list(10));
        assert_eq!(long.len(), 6);
        assert_eq!(long[5], Row::ShowAll(10));
    }

    #[test]
    fn the_default_apps_have_their_own_names() {
        assert_eq!(name_from_program("1password.exe"), "1Password");
        assert_eq!(name_from_program("protonpass.exe"), "Proton Pass");
        assert_eq!(name_from_program("notepad.exe"), "Notepad");
        assert!(name_says_program("Proton Pass", "protonpass.exe"));
        assert!(name_says_program("Notepad", "notepad.exe"));
        assert!(!name_says_program("Apex Legends", "r5apex.exe"));
    }

    #[test]
    fn an_app_with_no_program_is_not_offered_and_running_ones_are() {
        let list = sample();
        assert!(!list.iter().any(|c| c.name == "Calculator"));
        let quake = list.iter().find(|c| c.program == "quake.exe").expect("running app");
        assert_eq!(quake.name, "Quake");
        assert_eq!(list.iter().filter(|c| c.program == "notepad.exe").count(), 1);
        assert!(!list.iter().any(|c| c.program == "orbit.exe" || c.program == "applicationframehost.exe"));
    }

    #[test]
    fn nothing_typed_shows_what_is_chosen() {
        let chosen = vec!["Notepad.exe".to_string(), "unknown.exe".to_string()];
        let rows = rows(&chosen, &sample(), "");
        assert_eq!(names(&rows), ["Notepad", "Unknown"]);
        assert!(matches!(&rows[0], Row::App { chosen: true, icon: Some(_), .. }));
    }

    #[test]
    fn names_for_macos_stay_saved_but_are_not_shown() {
        let mut chosen = vec!["1Password".to_string(), "keepass.exe".to_string()];
        assert_eq!(names(&rows(&chosen, &sample(), "")), ["KeePass"]);
        toggle(&mut chosen, "keepass.exe");
        assert_eq!(chosen, ["1Password"]);
        assert!(matches!(rows(&chosen, &sample(), "")[0], Row::Message(_)));
    }

    #[test]
    fn search_ranks_the_start_of_a_name_first_and_marks_what_is_chosen() {
        let chosen = vec!["taker.exe".to_string()];
        let rows = rows(&chosen, &sample(), "not");
        assert_eq!(names(&rows), ["Note Taker", "Notepad"]);
        assert!(matches!(&rows[0], Row::App { chosen: true, .. }));
        assert!(matches!(&rows[1], Row::App { chosen: false, .. }));
        assert_eq!(names(&super::rows(&[], &sample(), "studio")), ["Visual Studio Code"]);
        assert_eq!(names(&super::rows(&[], &sample(), "code.exe")), ["Visual Studio Code"]);
    }

    #[test]
    fn a_program_that_is_not_listed_can_be_added_by_name() {
        let rows = rows(&[], &sample(), "mygame");
        assert!(matches!(&rows[0], Row::App { program, .. } if program == "mygame.exe"));
        let exact = super::rows(&[], &sample(), "Other.EXE");
        assert!(matches!(&exact[0], Row::App { program, .. } if program == "other.exe"));
    }

    #[test]
    fn something_that_cannot_be_a_program_name_says_so() {
        assert!(matches!(rows(&[], &sample(), "c:\\nope")[0], Row::Message(_)));
    }

    #[test]
    fn a_steam_game_is_found_by_its_biggest_program_that_is_not_a_helper() {
        assert_eq!(steam_game_id("steam://rungameid/1172470"), Some("1172470"));
        assert_eq!(steam_game_id("steam://open/games"), None);
        let root = std::env::temp_dir().join(format!("orbit-steam-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let apps = root.join("steamapps");
        let game = apps.join("common").join("Some Game");
        std::fs::create_dir_all(game.join("Game").join("Binaries").join("Win64")).unwrap();
        std::fs::write(apps.join("appmanifest_42.acf"), "\"AppState\"\n{\n\t\"appid\"\t\t\"42\"\n\t\"installdir\"\t\t\"Some Game\"\n}\n").unwrap();
        std::fs::write(game.join("Game.exe"), vec![0u8; 10]).unwrap();
        std::fs::write(game.join("EAAntiCheat.Installer.exe"), vec![0u8; 500]).unwrap();
        std::fs::write(game.join("Game").join("Binaries").join("Win64").join("Game-Win64-Shipping.exe"), vec![0u8; 100]).unwrap();
        let libraries = [root.clone()];
        assert_eq!(steam_program("42", &libraries).as_deref(), Some("game-win64-shipping.exe"));
        assert_eq!(steam_program("43", &libraries), None);
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn steam_library_paths_are_read_with_single_backslashes() {
        let text = "\"libraryfolders\"\n{\n\t\"0\"\n\t{\n\t\t\"path\"\t\t\"C:\\\\Program Files (x86)\\\\Steam\"\n\t}\n\t\"1\"\n\t{\n\t\t\"path\"\t\t\"I:\\\\SteamLibrary\"\n\t}\n}\n";
        assert_eq!(vdf_values(text, "path"), ["C:\\Program Files (x86)\\Steam", "I:\\SteamLibrary"]);
    }

    #[test]
    fn toggle_ignores_case() {
        let mut chosen = vec!["Game.exe".to_string()];
        assert!(!toggle(&mut chosen, "game.exe"));
        assert!(chosen.is_empty());
        assert!(toggle(&mut chosen, "game.exe"));
        assert_eq!(chosen, ["game.exe"]);
    }
}
