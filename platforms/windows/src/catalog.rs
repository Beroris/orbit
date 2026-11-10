//! Everything searchable synchronously: apps, built-in commands, links, custom commands.
//! Built from settings plus the app index, and rebuilt only when one of those changes.

use crate::indexer::{AppEntry};
use crate::integrations::Tool;
use orbit_core::search::usage_bonus;
use orbit_core::{Action, Builtin, SearchIndex, SearchItem, Settings};
use orbit_data::UsageRecord;
use std::collections::HashMap;

#[derive(Clone, Debug, PartialEq)]
pub enum Entry {
    App(usize),
    Builtin(Builtin),
    Link(usize),
    Custom(usize),
    Integration(Tool),
    /// A command of an extension that's turned on, by its place in `extension_commands`.
    Extension(usize),
}

/// A command an extension adds, as the catalog lists it.
#[derive(Clone, Debug, PartialEq)]
pub struct ExtCommand {
    pub ext: String,
    pub command: String,
    pub title: String,
    pub subtitle: String,
    pub keywords: String,
    /// The extension's picture, if it has one.
    pub icon: Option<String>,
    /// For a command that opens a list, its own keyword. The one set in Settings wins, and typed
    /// alone it finds the command, as an alias does.
    pub keyword: Option<String>,
}

pub fn extension_key(ext: &str, command: &str) -> String {
    format!("ext:{ext}/{command}")
}

pub struct Catalog {
    pub index: SearchIndex,
    pub entries: Vec<Entry>,
    pub apps: Vec<AppEntry>,
    /// Row identity (key) to usage bonus, kept so a rebuild does not lose ranking.
    usage: HashMap<String, u16>,
    settings_links: Vec<orbit_core::settings::QuickLink>,
    settings_custom: Vec<orbit_core::settings::CustomCommand>,
    /// Items with a hotkey, and its label. Few enough that rows just scan it.
    pub hotkeys: Vec<(u32, String)>,
    /// The commands of every extension that's turned on.
    pub extension_commands: Vec<ExtCommand>,
}

pub fn app_key(id: &str) -> String {
    format!("app:{id}")
}

pub fn link_key(id: &str) -> String {
    format!("link:{id}")
}

pub fn custom_key(id: &str) -> String {
    format!("custom:{id}")
}

impl Catalog {
    pub fn new(apps: Vec<AppEntry>, settings: &Settings) -> Self {
        let mut catalog = Catalog {
            index: SearchIndex::new(),
            entries: Vec::new(),
            apps,
            usage: HashMap::new(),
            settings_links: Vec::new(),
            settings_custom: Vec::new(),
            hotkeys: Vec::new(),
            extension_commands: Vec::new(),
        };
        catalog.rebuild(settings);
        catalog
    }

    pub fn set_apps(&mut self, apps: Vec<AppEntry>, settings: &Settings) {
        self.apps = apps;
        self.rebuild(settings);
    }

    pub fn rebuild(&mut self, settings: &Settings) {
        self.settings_links = settings.links.clone();
        self.settings_custom = settings.custom_commands.clone();
        let capacity = self.apps.len() + Builtin::ALL.len() + settings.links.len() + settings.custom_commands.len();
        let mut index = SearchIndex::with_capacity(capacity);
        let mut entries = Vec::with_capacity(capacity);
        let is_pinned = |key: &str| settings.pinned.iter().any(|p| p == key);
        let is_hidden = |key: &str| settings.hidden.iter().any(|h| h == key);
        let alias = |key: &str| settings.aliases.get(key).cloned().unwrap_or_default();

        for (i, app) in self.apps.iter().enumerate() {
            let key = app_key(&app.id);
            if is_hidden(&key) {
                continue;
            }
            index.add(&SearchItem {
                title: app.name.clone(),
                subtitle: String::new(),
                alias: alias(&key),
                keywords: exe_keyword(&app.exe),
                pinned: is_pinned(&key),
                usage: self.usage.get(&key).copied().unwrap_or(0),
                key,
            });
            entries.push(Entry::App(i));
        }
        for &(command, key, title, subtitle, keywords) in Builtin::ALL {
            // A command of an extension that's turned off isn't there, such as File search or Weather.
            if is_hidden(key) || !orbit_core::builtin_ext::builtin_available(settings, command) {
                continue;
            }
            index.add(&SearchItem {
                key: key.to_string(),
                title: title.to_string(),
                subtitle: subtitle.to_string(),
                // Everything answers to the alias kept with its settings, unless it has one here.
                alias: match alias(key) {
                    own if own.is_empty() && command == Builtin::SearchFiles => settings.extensions.everything.alias.clone(),
                    own => own,
                },
                keywords: keywords.to_string(),
                pinned: is_pinned(key),
                usage: self.usage.get(key).copied().unwrap_or(0),
            });
            entries.push(Entry::Builtin(command));
        }
        for (i, link) in settings.links.iter().enumerate() {
            let key = link_key(&link.id);
            if is_hidden(&key) {
                continue;
            }
            index.add(&SearchItem {
                title: link.name.clone(),
                subtitle: shown_address(&link.url),
                alias: if link.alias.is_empty() { alias(&key) } else { link.alias.clone() },
                keywords: String::new(),
                pinned: is_pinned(&key),
                usage: self.usage.get(&key).copied().unwrap_or(0),
                key,
            });
            entries.push(Entry::Link(i));
        }
        for (i, command) in settings.custom_commands.iter().enumerate() {
            let key = custom_key(&command.id);
            if is_hidden(&key) {
                continue;
            }
            index.add(&SearchItem {
                title: command.name.clone(),
                subtitle: command_summary(command),
                alias: if command.alias.is_empty() { alias(&key) } else { command.alias.clone() },
                keywords: String::new(),
                pinned: is_pinned(&key),
                usage: self.usage.get(&key).copied().unwrap_or(0),
                key,
            });
            entries.push(Entry::Custom(i));
        }
        for tool in Tool::ALL {
            let on = match tool {
                Tool::OnePassword => settings.integrations.one_password,
                Tool::EnteAuth => settings.integrations.ente_auth,
            };
            let key = tool.key();
            if !on || is_hidden(key) {
                continue;
            }
            index.add(&SearchItem {
                key: key.to_string(),
                title: tool.name().to_string(),
                subtitle: tool.subtitle().to_string(),
                alias: alias(key),
                keywords: tool.keywords().to_string(),
                pinned: is_pinned(key),
                usage: self.usage.get(key).copied().unwrap_or(0),
            });
            entries.push(Entry::Integration(tool));
        }
        for (i, command) in self.extension_commands.iter().enumerate() {
            let key = extension_key(&command.ext, &command.command);
            if is_hidden(&key) || !orbit_core::builtin_ext::extension_on(settings, &command.ext) {
                continue;
            }
            index.add(&SearchItem {
                title: command.title.clone(),
                subtitle: command.subtitle.clone(),
                alias: match (alias(&key), &command.keyword) {
                    (own, Some(keyword)) if own.is_empty() => orbit_core::builtin_ext::command_keyword(settings, &command.ext, &command.command, keyword).to_string(),
                    (own, _) => own,
                },
                keywords: command.keywords.clone(),
                pinned: is_pinned(&key),
                usage: self.usage.get(&key).copied().unwrap_or(0),
                key,
            });
            entries.push(Entry::Extension(i));
        }
        self.index = index;
        self.entries = entries;
        self.find_hotkeys(settings);
    }

    fn find_hotkeys(&mut self, settings: &Settings) {
        self.hotkeys.clear();
        for item in 0..self.entries.len() as u32 {
            let Some(action) = self.entry_action(item, true) else { continue };
            if let Some(hotkey) = orbit_core::hotkeys::hotkey_for(settings, &action) {
                self.hotkeys.push((item, hotkey.label("Win")));
            }
        }
    }

    pub fn hotkey_label(&self, item: u32) -> Option<&str> {
        self.hotkeys.iter().find(|(at, _)| *at == item).map(|(_, label)| label.as_str())
    }

    pub fn link(&self, i: usize) -> Option<&orbit_core::settings::QuickLink> {
        self.settings_links.get(i)
    }

    pub fn apply_usage(&mut self, records: &[UsageRecord], now: i64) {
        self.usage.clear();
        for record in records {
            self.usage.insert(record.key.clone(), usage_bonus(record.count, now - record.last_used_at));
        }
        for item in 0..self.index.len() as u32 {
            let bonus = self.usage.get(self.index.key(item)).copied().unwrap_or(0);
            self.index.set_usage(item, bonus);
        }
    }

    pub fn app_by_id(&self, id: &str) -> Option<&AppEntry> {
        self.apps.iter().find(|app| app.id == id)
    }

    /// Case-insensitive name lookup used when an action stored only a name.
    pub fn app_by_name(&self, name: &str) -> Option<&AppEntry> {
        self.apps.iter().find(|app| app.name.eq_ignore_ascii_case(name))
    }

    pub fn entry_action(&self, item: u32, focus_running: bool) -> Option<Action> {
        Some(match self.entries.get(item as usize)? {
            Entry::App(i) => {
                let app = self.apps.get(*i)?;
                if focus_running {
                    Action::FocusOrLaunchApp { id: app.id.clone(), name: app.name.clone() }
                } else {
                    Action::LaunchApp { id: app.id.clone(), name: app.name.clone() }
                }
            }
            Entry::Builtin(command) => Action::Builtin { command: *command },
            Entry::Link(i) => Action::Url { url: self.settings_links.get(*i)?.url.clone() },
            Entry::Custom(i) => Action::Custom { id: self.settings_custom.get(*i)?.id.clone() },
            Entry::Integration(tool) => tool.action(),
            Entry::Extension(i) => {
                let command = self.extension_commands.get(*i)?;
                Action::Extension { ext: command.ext.clone(), command: command.command.clone() }
            }
        })
    }
}

/// Exactly what will run, shown on the row before it's chosen.
fn command_summary(command: &orbit_core::settings::CustomCommand) -> String {
    let mut text = format!("Runs {}", command.program);
    if !command.arguments.is_empty() {
        text.push(' ');
        text.push_str(&command.arguments);
    }
    if !command.working_dir.is_empty() {
        text.push_str(&format!(", in {}", command.working_dir));
    }
    text
}

/// `C:\...\code.exe` lets people find an app by its executable's name.
fn exe_keyword(exe: &str) -> String {
    let name = crate::util::exe_name(exe);
    name.strip_suffix(".exe").unwrap_or(name).to_string()
}

pub fn glyph_for_builtin(command: Builtin) -> char {
    match command {
        Builtin::NewNote | Builtin::SearchNotes => '\u{E70B}',
        Builtin::ClipboardHistory | Builtin::PauseClipboard | Builtin::ResumeClipboard | Builtin::SkipNextCopy => '\u{E77F}',
        Builtin::ClearClipboard | Builtin::EmptyTrash | Builtin::DeleteLocalData => '\u{E74D}',
        Builtin::SnippetSearch | Builtin::NewSnippet => '\u{E943}',
        Builtin::NewQuicklink | Builtin::SearchQuicklinks => '\u{E71B}',
        Builtin::EmojiPicker => '\u{E76E}',
        Builtin::Settings | Builtin::ToggleLauncher => '\u{E713}',
        Builtin::Diagnostics | Builtin::ToggleLowResource => '\u{E9D9}',
        Builtin::RebuildAppIndex | Builtin::RestartOrbit | Builtin::Restart | Builtin::ResetShortcuts => '\u{E72C}',
        Builtin::SuspendShortcuts | Builtin::ResumeShortcuts => '\u{E765}',
        Builtin::QuitOrbit | Builtin::Shutdown => '\u{E7E8}',
        Builtin::LockScreen => '\u{E72E}',
        Builtin::Sleep => '\u{E708}',
        Builtin::OpenDownloads | Builtin::ExportData => '\u{E8B7}',
        Builtin::OpenTerminal => '\u{E756}',
        Builtin::ShowHelp => '\u{E897}',
        Builtin::SpeedTest => crate::rows::GLYPH_SPEED,
        Builtin::Weather => crate::rows::GLYPH_WEATHER,
        Builtin::WindowLeft => crate::rows::GLYPH_WIN_LEFT,
        Builtin::WindowRight => crate::rows::GLYPH_WIN_RIGHT,
        Builtin::WindowTop => crate::rows::GLYPH_WIN_TOP,
        Builtin::WindowBottom => crate::rows::GLYPH_WIN_BOTTOM,
        Builtin::WindowMaximize => crate::rows::GLYPH_WIN_MAX,
        Builtin::WindowAlmostMaximize => crate::rows::GLYPH_WIN_ALMOST,
        Builtin::WindowCenter => crate::rows::GLYPH_WIN_CENTER,
        Builtin::WindowNextDisplay => crate::rows::GLYPH_WIN_DISPLAY,
        Builtin::ToggleDarkMode => crate::rows::GLYPH_THEME,
        Builtin::ToggleBluetooth => crate::rows::GLYPH_BLUETOOTH,
        Builtin::ToggleWifi => crate::rows::GLYPH_WIFI,
        Builtin::ToggleMute => crate::rows::GLYPH_SOUND,
        Builtin::Timer => crate::rows::GLYPH_TIMER,
        Builtin::Stopwatch => crate::rows::GLYPH_STOPWATCH,
        Builtin::PickColour => crate::rows::GLYPH_PICKER,
        Builtin::SearchFiles => crate::rows::GLYPH_FILE,
        Builtin::ImportData => crate::rows::GLYPH_IMPORT,
        Builtin::ManageExtensions => crate::rows::GLYPH_EXTENSION,
        Builtin::CheckForUpdates => crate::rows::GLYPH_UPDATE,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use orbit_core::QueryScratch;

    fn apps() -> Vec<AppEntry> {
        vec![
            AppEntry { id: "code".into(), name: "Visual Studio Code".into(), exe: "c:\\x\\code.exe".into(), aumid: String::new() },
            AppEntry { id: "np".into(), name: "Notepad".into(), exe: String::new(), aumid: "np!App".into() },
        ]
    }

    fn titles(catalog: &Catalog, query: &str) -> Vec<String> {
        let mut out = Vec::new();
        catalog.index.search(query, 10, &mut QueryScratch::default(), &mut out);
        out.iter().map(|h| catalog.index.title(h.item).to_string()).collect()
    }

    #[test]
    fn finds_apps_commands_and_links() {
        let catalog = Catalog::new(apps(), &Settings::default());
        assert_eq!(titles(&catalog, "vsc")[0], "Visual Studio Code");
        assert_eq!(titles(&catalog, "notepad")[0], "Notepad");
        assert_eq!(titles(&catalog, "lock")[0], "Lock Screen");
        assert_eq!(titles(&catalog, "g")[0], "Search Google", "alias is exact");
    }

    #[test]
    fn set_alias_works_on_links_too() {
        let mut settings = Settings { links: Vec::new(), ..Settings::default() };
        settings.links.push(orbit_core::settings::QuickLink { id: "l".into(), name: "Docs".into(), url: "https://example.com".into(), alias: String::new(), open_with: String::new() });
        settings.aliases.insert(link_key("l"), "dd".into());
        let catalog = Catalog::new(apps(), &settings);
        assert!(titles(&catalog, "dd").contains(&"Docs".to_string()));
    }

    #[test]
    fn hidden_items_are_gone_and_aliases_apply() {
        let mut settings = Settings::default();
        settings.hidden.push(app_key("np"));
        settings.aliases.insert(app_key("code"), "ed".into());
        let catalog = Catalog::new(apps(), &settings);
        assert!(titles(&catalog, "notepad").is_empty() || titles(&catalog, "notepad")[0] != "Notepad");
        assert_eq!(titles(&catalog, "ed")[0], "Visual Studio Code");
    }

    #[test]
    fn exe_name_is_a_keyword() {
        let catalog = Catalog::new(apps(), &Settings::default());
        assert_eq!(titles(&catalog, "code.exe").first().map(String::as_str), None);
        assert!(titles(&catalog, "code").contains(&"Visual Studio Code".to_string()));
    }

    #[test]
    fn integrations_show_only_when_turned_on() {
        let mut settings = Settings::default();
        let catalog = Catalog::new(apps(), &settings);
        assert!(!titles(&catalog, "1password").contains(&"1Password".to_string()));
        settings.integrations.one_password = true;
        settings.integrations.ente_auth = true;
        let catalog = Catalog::new(apps(), &settings);
        assert_eq!(catalog.entries.len(), catalog.index.len());
        assert_eq!(titles(&catalog, "1password")[0], "1Password");
        assert_eq!(titles(&catalog, "ente")[0], "Ente Auth");
        assert!(titles(&catalog, "2fa").contains(&"Ente Auth".to_string()));
        settings.hidden.push(Tool::EnteAuth.key().to_string());
        assert!(!titles(&Catalog::new(apps(), &settings), "ente").contains(&"Ente Auth".to_string()));
    }

    #[test]
    fn extension_commands_show_only_when_turned_on() {
        let mut settings = Settings::default();
        let mut catalog = Catalog::new(apps(), &settings);
        catalog.extension_commands = vec![ExtCommand {
            ext: "windows-settings".into(),
            command: "search".into(),
            title: "Search Windows Settings".into(),
            subtitle: "Bluetooth, display, sound and the rest".into(),
            keywords: "control panel preferences".into(),
            icon: None,
            keyword: Some("ws".into()),
        }];
        catalog.rebuild(&settings);
        assert!(!titles(&catalog, "windows settings").contains(&"Search Windows Settings".to_string()), "added extensions start switched off");
        settings.extensions.enabled.push("windows-settings".into());
        catalog.rebuild(&settings);
        assert_eq!(catalog.entries.len(), catalog.index.len());
        assert_eq!(titles(&catalog, "windows settings")[0], "Search Windows Settings");
        assert!(titles(&catalog, "control panel").contains(&"Search Windows Settings".to_string()), "found by its keywords");
        let item = (0..catalog.index.len() as u32).find(|&item| catalog.index.title(item) == "Search Windows Settings").unwrap();
        assert_eq!(catalog.entry_action(item, true), Some(Action::Extension { ext: "windows-settings".into(), command: "search".into() }));
        assert_eq!(titles(&catalog, "ws")[0], "Search Windows Settings", "its keyword, typed alone, finds it");
        settings.extensions.keywords.insert("windows-settings/search".into(), "win".into());
        catalog.rebuild(&settings);
        assert_eq!(titles(&catalog, "win")[0], "Search Windows Settings", "and so does one set in Settings");
        settings.hidden.push(extension_key("windows-settings", "search"));
        catalog.rebuild(&settings);
        assert!(!titles(&catalog, "windows settings").contains(&"Search Windows Settings".to_string()), "hidden from search");
    }

    #[test]
    fn an_extension_that_is_off_takes_its_commands_with_it() {
        let mut settings = Settings::default();
        let catalog = Catalog::new(apps(), &settings);
        assert!(titles(&catalog, "speed test").contains(&"Speed Test".to_string()), "on by default");
        assert_eq!(titles(&catalog, "weather").first().map(String::as_str), Some("Weather"));
        assert_eq!(titles(&catalog, "everything").first().map(String::as_str), Some("Everything"), "file search is found by its name");
        settings.extensions.speed_test = false;
        settings.fetch_weather = false;
        let catalog = Catalog::new(apps(), &settings);
        assert!(!titles(&catalog, "speed test").contains(&"Speed Test".to_string()));
        assert!(!titles(&catalog, "weather").contains(&"Weather".to_string()));
        let ente = (0..catalog.index.len() as u32).find(|&item| catalog.index.title(item) == "Ente Auth");
        assert!(ente.is_none(), "Ente Auth starts off");
        settings.integrations.ente_auth = true;
        let catalog = Catalog::new(apps(), &settings);
        let ente = (0..catalog.index.len() as u32).find(|&item| catalog.index.title(item) == "Ente Auth").unwrap();
        assert_eq!(catalog.entry_action(ente, true), Some(Action::Extension { ext: "ente-auth".into(), command: "search".into() }), "so it can have a hotkey");
    }

    #[test]
    fn entries_stay_parallel_to_the_index() {
        let mut settings = Settings::default();
        settings.hidden.push(app_key("code"));
        let catalog = Catalog::new(apps(), &settings);
        assert_eq!(catalog.entries.len(), catalog.index.len());
        for item in 0..catalog.index.len() as u32 {
            assert!(catalog.entry_action(item, true).is_some());
        }
    }

    #[test]
    fn usage_changes_rank_and_survives_rebuild() {
        let mut settings = Settings::default();
        settings.links.push(orbit_core::settings::QuickLink {
            id: "n".into(),
            open_with: String::new(),
            name: "Notes Site".into(),
            url: "https://n".into(),
            alias: String::new(),
        });
        let mut catalog = Catalog::new(apps(), &settings);
        catalog.apply_usage(
            &[UsageRecord { key: link_key("n"), count: 1000, last_used_at: 100 }],
            100,
        );
        catalog.rebuild(&settings);
        let item = catalog.index.find_key(&link_key("n")).unwrap();
        let mut out = Vec::new();
        catalog.index.suggestions(5, &mut out);
        assert_eq!(out[0].item, item);
    }
}

/// A link's address as a row shows it. `https://` says nothing most links don't, so it's left
/// off. Anything else, such as `http://`, stays, since it's worth noticing.
pub fn shown_address(url: &str) -> String {
    let url = url.trim();
    match url.get(..8) {
        Some(scheme) if scheme.eq_ignore_ascii_case("https://") => url[8..].to_string(),
        _ => url.to_string(),
    }
}

#[cfg(test)]
mod shown_address_tests {
    use super::shown_address;

    #[test]
    fn a_links_https_is_left_off() {
        assert_eq!(shown_address("https://mail.google.com"), "mail.google.com");
        assert_eq!(shown_address("HTTPS://Example.com/a"), "Example.com/a");
        assert_eq!(shown_address("http://router.local"), "http://router.local");
        assert_eq!(shown_address(r"C:\Projects"), r"C:\Projects");
    }
}
