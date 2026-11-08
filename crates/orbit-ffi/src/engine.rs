//! Settings, the searchable catalog and the synchronous part of the root query. This is the
//! same pipeline as `platforms/windows/src/app.rs::build_root_rows`, kept here so the macOS host
//! only draws rows and it can be tested without a window.

use crate::form::{HostView, SettingsForm};
use crate::rows::*;
use orbit_core::action::{as_openable_url, fill_url_template};
use orbit_core::calc::{CalcOutcome, evaluate};
use orbit_core::search::usage_bonus;
use orbit_core::settings::{CustomCommand, LoadOutcome, QuickLink};
use orbit_core::{Action, Builtin, Hit, MatchClass, QueryScratch, SearchIndex, SearchItem, Settings};
use orbit_data::UsageRecord;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::{Path, PathBuf};

pub const INDEX_FILE: &str = "apps.json";
pub const ROW_LIMIT: usize = 30;

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct AppEntry {
    /// Bundle identifier when the app has one, otherwise its path. Survives moving the app.
    pub id: String,
    pub name: String,
    pub path: String,
}

#[derive(Clone, Debug, PartialEq)]
pub enum Entry {
    App(usize),
    Builtin(Builtin),
    Link(usize),
    Custom(usize),
}

/// macOS wording for the few shared command titles that name a Windows concept.
fn mac_title(command: Builtin, title: &'static str) -> &'static str {
    match command {
        Builtin::EmptyTrash => "Empty Trash",
        _ => title,
    }
}

pub fn glyph_for_builtin(command: Builtin) -> u16 {
    match command {
        Builtin::NewNote | Builtin::SearchNotes => GLYPH_NOTE,
        Builtin::ClipboardHistory | Builtin::SkipNextCopy => GLYPH_CLIPBOARD,
        Builtin::PauseClipboard | Builtin::ResumeClipboard | Builtin::SuspendShortcuts | Builtin::ResumeShortcuts => GLYPH_PAUSE,
        Builtin::ClearClipboard | Builtin::EmptyTrash | Builtin::DeleteLocalData => GLYPH_DELETE,
        Builtin::SnippetSearch | Builtin::NewSnippet | Builtin::NewQuicklink | Builtin::SearchQuicklinks | Builtin::EmojiPicker => GLYPH_SNIPPET,
        Builtin::Settings | Builtin::ToggleLauncher => GLYPH_SETTINGS,
        Builtin::Diagnostics | Builtin::ToggleLowResource => GLYPH_DIAGNOSTICS,
        Builtin::RebuildAppIndex | Builtin::RestartOrbit | Builtin::Restart | Builtin::ResetShortcuts => GLYPH_RELOAD,
        Builtin::QuitOrbit | Builtin::Shutdown => GLYPH_POWER,
        Builtin::LockScreen => GLYPH_LOCK,
        Builtin::Sleep => GLYPH_MOON,
        Builtin::OpenDownloads | Builtin::ExportData => GLYPH_FOLDER,
        Builtin::OpenTerminal => GLYPH_TERMINAL,
        Builtin::ShowHelp => GLYPH_HELP,
        // Only built for Windows so far, and left out of the Mac's catalog.
        _ => GLYPH_DIAGNOSTICS,
    }
}

pub fn app_key(id: &str) -> String {
    format!("app:{id}")
}

pub struct Catalog {
    pub index: SearchIndex,
    pub entries: Vec<Entry>,
    pub apps: Vec<AppEntry>,
    usage: HashMap<String, u16>,
    links: Vec<QuickLink>,
    custom: Vec<CustomCommand>,
}

impl Catalog {
    pub fn new(apps: Vec<AppEntry>, settings: &Settings) -> Self {
        let mut catalog = Catalog {
            index: SearchIndex::new(),
            entries: Vec::new(),
            apps,
            usage: HashMap::new(),
            links: Vec::new(),
            custom: Vec::new(),
        };
        catalog.rebuild(settings);
        catalog
    }

    pub fn rebuild(&mut self, settings: &Settings) {
        self.links = settings.links.clone();
        self.custom = settings.custom_commands.clone();
        let capacity = self.apps.len() + Builtin::ALL.len() + self.links.len() + self.custom.len();
        let mut index = SearchIndex::with_capacity(capacity);
        let mut entries = Vec::with_capacity(capacity);
        let is_pinned = |key: &str| settings.pinned.iter().any(|p| p == key);
        let is_hidden = |key: &str| settings.hidden.iter().any(|h| h == key);
        let alias = |key: &str| settings.aliases.get(key).cloned().unwrap_or_default();
        let usage = &self.usage;
        let used = |key: &str| usage.get(key).copied().unwrap_or(0);

        for (i, app) in self.apps.iter().enumerate() {
            let key = app_key(&app.id);
            if is_hidden(&key) {
                continue;
            }
            index.add(&SearchItem {
                title: app.name.clone(),
                subtitle: String::new(),
                alias: alias(&key),
                keywords: bundle_keyword(&app.id),
                pinned: is_pinned(&key),
                usage: used(&key),
                key,
            });
            entries.push(Entry::App(i));
        }
        for &(command, key, title, subtitle, keywords) in Builtin::ALL {
            // Not built for macOS yet.
            if is_hidden(key) || command.windows_only() {
                continue;
            }
            index.add(&SearchItem {
                key: key.to_string(),
                title: mac_title(command, title).to_string(),
                subtitle: subtitle.to_string(),
                alias: alias(key),
                keywords: keywords.to_string(),
                pinned: is_pinned(key),
                usage: used(key),
            });
            entries.push(Entry::Builtin(command));
        }
        for (i, link) in self.links.iter().enumerate() {
            let key = format!("link:{}", link.id);
            index.add(&SearchItem {
                title: link.name.clone(),
                subtitle: link.url.clone(),
                alias: link.alias.clone(),
                keywords: String::new(),
                pinned: is_pinned(&key),
                usage: used(&key),
                key,
            });
            entries.push(Entry::Link(i));
        }
        for (i, command) in self.custom.iter().enumerate() {
            let key = format!("custom:{}", command.id);
            index.add(&SearchItem {
                title: command.name.clone(),
                subtitle: command.program.clone(),
                alias: command.alias.clone(),
                keywords: String::new(),
                pinned: is_pinned(&key),
                usage: used(&key),
                key,
            });
            entries.push(Entry::Custom(i));
        }
        self.index = index;
        self.entries = entries;
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
            Entry::Link(i) => Action::Url { url: self.links.get(*i)?.url.clone() },
            Entry::Custom(i) => Action::Custom { id: self.custom.get(*i)?.id.clone() },
        })
    }

    pub fn find_app(&self, id: &str, name: &str) -> Option<&AppEntry> {
        if !id.is_empty() {
            if let Some(app) = self.apps.iter().find(|app| app.id == id) {
                return Some(app);
            }
        }
        self.apps.iter().find(|app| app.name.eq_ignore_ascii_case(name))
    }

    pub fn link_alias(&self, item: u32) -> Option<String> {
        match self.entries.get(item as usize)? {
            Entry::Link(i) => self.links.get(*i).map(|link| link.alias.clone()),
            _ => None,
        }
    }

    pub fn custom(&self, id: &str) -> Option<&CustomCommand> {
        self.custom.iter().find(|command| command.id == id)
    }
}

/// `com.microsoft.VSCode` lets people find an app by the last part of its identifier.
fn bundle_keyword(id: &str) -> String {
    if id.starts_with('/') {
        return String::new();
    }
    id.rsplit('.').next().unwrap_or("").to_string()
}

pub struct RootQuery {
    pub rows: Vec<Row>,
    /// Where results from the data thread go, so they never shift rows above them.
    pub async_insert_at: usize,
    pub wants_data: bool,
}

pub struct Engine {
    pub dir: PathBuf,
    pub settings: Settings,
    pub read_only: bool,
    pub notice: Option<String>,
    pub had_cached_apps: bool,
    pub catalog: Catalog,
    pub indexing: bool,
    scratch: QueryScratch,
    hits: Vec<Hit>,
    pub rows: RowList,
    /// Backs strings returned by accessors. Replaced on each such call.
    pub held: Vec<std::ffi::CString>,
}

pub fn load_settings(dir: &Path) -> (Settings, bool, Option<String>) {
    let path = dir.join("settings.json");
    let Ok(text) = std::fs::read_to_string(&path) else {
        return (Settings::default(), false, None);
    };
    let (settings, outcome) = Settings::parse(&text);
    match outcome {
        LoadOutcome::Loaded | LoadOutcome::Missing => (settings, false, None),
        LoadOutcome::Corrupt(_) => {
            let _ = std::fs::rename(&path, dir.join("settings.damaged.json"));
            (settings, false, Some("Your settings file was damaged, so defaults are in use. The old file is saved as settings.damaged.json.".to_string()))
        }
        LoadOutcome::NewerVersion(version) => {
            (settings, true, Some(format!("These settings are from a newer Orbit (schema {version}). Changes won't be saved.")))
        }
    }
}

pub fn load_apps(dir: &Path) -> Option<Vec<AppEntry>> {
    serde_json::from_slice(&std::fs::read(dir.join(INDEX_FILE)).ok()?).ok()
}

/// Temp file then rename, so a crash leaves the old file or the new one and never half of one.
pub fn write_atomic(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    let temp = path.with_extension("tmp");
    std::fs::write(&temp, bytes)?;
    std::fs::rename(&temp, path)
}

impl Engine {
    pub fn new(dir: PathBuf) -> Engine {
        let _ = std::fs::create_dir_all(&dir);
        let (settings, read_only, notice) = load_settings(&dir);
        let apps = load_apps(&dir);
        let had_cached_apps = apps.is_some();
        let catalog = Catalog::new(apps.unwrap_or_default(), &settings);
        Engine {
            dir,
            settings,
            read_only,
            notice,
            had_cached_apps,
            catalog,
            indexing: false,
            scratch: QueryScratch::default(),
            hits: Vec::with_capacity(ROW_LIMIT),
            rows: RowList::default(),
            held: Vec::new(),
        }
    }

    pub fn save(&self) -> std::io::Result<()> {
        if self.read_only {
            return Ok(());
        }
        write_atomic(&self.dir.join("settings.json"), self.settings.to_json().as_bytes())
    }

    pub fn save_and_rebuild(&mut self) -> bool {
        let saved = self.save().is_ok();
        self.catalog.rebuild(&self.settings);
        saved
    }

    pub fn set_apps(&mut self, apps: Vec<AppEntry>) -> bool {
        if apps == self.catalog.apps {
            return false;
        }
        self.catalog.apps = apps;
        self.catalog.rebuild(&self.settings);
        true
    }

    pub fn host_view(&self) -> HostView {
        HostView::new(&self.settings, self.read_only)
    }

    /// Validates a settings form and, if it is fine, makes it current and saves it.
    pub fn apply_form(&mut self, form: &SettingsForm) -> Result<(), (i32, String)> {
        let next = form.collect(&self.settings)?;
        self.settings = next;
        if self.save_and_rebuild() {
            Ok(())
        } else {
            Err((-2, "Couldn't save your settings.".to_string()))
        }
    }

    fn row_for_item(&self, item: u32) -> Row {
        let index = &self.catalog.index;
        let key = index.key(item).to_string();
        let (icon_path, glyph, kind) = match &self.catalog.entries[item as usize] {
            Entry::App(i) => (self.catalog.apps[*i].path.clone(), GLYPH_COMMAND, "App"),
            Entry::Builtin(command) => (String::new(), glyph_for_builtin(*command), "Command"),
            Entry::Link(_) => (String::new(), GLYPH_LINK, "Link"),
            Entry::Custom(_) => (String::new(), GLYPH_COMMAND, "Script"),
        };
        Row {
            title: index.title(item).to_string(),
            subtitle: index.subtitle(item).to_string(),
            kind: kind.to_string(),
            icon_path,
            glyph,
            op: OP_RUN_ITEM,
            source: SOURCE_CATALOG,
            item,
            pinned: index.is_pinned(item),
            key,
            ..Row::default()
        }
    }

    fn push_hits(&self, rows: &mut Vec<Row>, hits: &[Hit]) {
        rows.extend(hits.iter().map(|hit| self.row_for_item(hit.item)));
    }

    pub fn query_root(&mut self, query: &str) -> RootQuery {
        let q = query.trim();
        let mut rows: Vec<Row> = Vec::with_capacity(ROW_LIMIT);
        let mut hits = std::mem::take(&mut self.hits);
        hits.clear();

        if q.is_empty() {
            self.catalog.index.suggestions(ROW_LIMIT, &mut hits);
            if hits.is_empty() {
                for key in ["cmd:new-note", "cmd:clipboard", "cmd:settings", "cmd:help"] {
                    if let Some(item) = self.catalog.index.find_key(key) {
                        hits.push(Hit { item, score: 0, class: MatchClass::Metadata });
                    }
                }
                self.push_hits(&mut rows, &hits);
                if self.catalog.apps.is_empty() {
                    let title = if self.indexing { "Finding your apps…" } else { "No apps indexed yet" };
                    rows.push(Row {
                        title: title.into(),
                        subtitle: "This takes a moment the first time".into(),
                        glyph: GLYPH_INFO,
                        ..Row::default()
                    });
                }
            } else {
                self.push_hits(&mut rows, &hits);
            }
            self.hits = hits;
            let at = rows.len();
            return RootQuery { rows, async_insert_at: at, wants_data: false };
        }

        // Explicit command mode.
        if let Some(rest) = q.strip_prefix('>').or_else(|| q.strip_prefix(':')) {
            let rest = rest.trim();
            if rest.is_empty() {
                for (item, entry) in self.catalog.entries.iter().enumerate() {
                    if matches!(entry, Entry::Builtin(_)) {
                        hits.push(Hit { item: item as u32, score: 0, class: MatchClass::Metadata });
                    }
                }
            } else {
                self.catalog.index.search(rest, ROW_LIMIT * 4, &mut self.scratch, &mut hits);
                let entries = &self.catalog.entries;
                hits.retain(|hit| matches!(entries[hit.item as usize], Entry::Builtin(_)));
            }
            hits.truncate(ROW_LIMIT);
            self.push_hits(&mut rows, &hits);
            self.hits = hits;
            let at = rows.len();
            return RootQuery { rows, async_insert_at: at, wants_data: false };
        }

        match evaluate(q) {
            CalcOutcome::Value { display, copy, note, .. } => rows.push(Row {
                title: display,
                subtitle: note.unwrap_or_else(|| q.to_string()),
                kind: "Calculator".into(),
                glyph: GLYPH_CALC,
                accent: true,
                op: OP_COPY,
                payload: copy,
                ..Row::default()
            }),
            CalcOutcome::Error(message) => rows.push(Row {
                title: message,
                subtitle: q.to_string(),
                kind: "Calculator".into(),
                glyph: GLYPH_CALC,
                ..Row::default()
            }),
            CalcOutcome::NotMath => {}
        }

        // "g rust async": a quick link template followed by its query.
        if let Some((first, rest)) = q.split_once(char::is_whitespace) {
            let rest = rest.trim();
            if !rest.is_empty() {
                let template = self.settings.links.iter().find(|link| link.url.contains("{query}") && link.alias.eq_ignore_ascii_case(first));
                if let Some(link) = template {
                    rows.insert(0, web_row(format!("{} for “{rest}”", link.name), fill_url_template(&link.url, rest), true));
                }
            }
        }

        if let Some(url) = as_openable_url(q) {
            rows.push(web_row(format!("Open {q}"), url, false));
        }

        self.catalog.index.search(q, ROW_LIMIT, &mut self.scratch, &mut hits);
        self.push_hits(&mut rows, &hits);
        let async_insert_at = rows.len();

        // Web search stays explicit: an offered row, shown only when nothing local matched.
        if rows.is_empty() {
            if let Some(link) = self.settings.links.iter().find(|link| link.url.contains("{query}")) {
                rows.push(web_row(format!("{} for “{q}”", link.name), fill_url_template(&link.url, q), false));
            }
        }
        self.hits = hits;
        RootQuery { rows, async_insert_at, wants_data: q.chars().count() >= 2 }
    }
}

fn web_row(title: String, url: String, accent: bool) -> Row {
    Row { title, kind: "Web".into(), glyph: GLYPH_WEB, accent, op: OP_OPEN_URL, payload: url, ..Row::default() }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn apps() -> Vec<AppEntry> {
        vec![
            AppEntry { id: "com.microsoft.VSCode".into(), name: "Visual Studio Code".into(), path: "/Applications/Visual Studio Code.app".into() },
            AppEntry { id: "com.apple.calculator".into(), name: "Calculator".into(), path: "/System/Applications/Calculator.app".into() },
        ]
    }

    fn engine() -> Engine {
        let dir = std::env::temp_dir().join(format!("orbit-ffi-engine-{}-{:?}", std::process::id(), std::thread::current().id()));
        let _ = std::fs::remove_dir_all(&dir);
        let mut engine = Engine::new(dir);
        engine.set_apps(apps());
        engine
    }

    fn titles(engine: &mut Engine, query: &str) -> Vec<String> {
        engine.query_root(query).rows.into_iter().map(|row| row.title).collect()
    }

    #[test]
    fn matches_the_windows_pipeline() {
        let mut e = engine();
        assert_eq!(titles(&mut e, "vsc")[0], "Visual Studio Code");
        assert_eq!(titles(&mut e, "2*21")[0], "42");
        assert_eq!(titles(&mut e, "g rust async")[0], "Search Google for “rust async”");
        assert_eq!(titles(&mut e, "github.com")[0], "Open github.com");
        assert_eq!(titles(&mut e, "zzzqqqxxx")[0], "Search Google for “zzzqqqxxx”");
        let commands = titles(&mut e, ">");
        assert!(commands.contains(&"Empty Trash".to_string()), "macOS wording");
        assert!(!commands.contains(&"Visual Studio Code".to_string()));
    }

    #[test]
    fn async_results_go_below_local_ones() {
        let mut e = engine();
        let q = e.query_root("calc");
        assert_eq!(q.async_insert_at, q.rows.len());
        assert!(q.wants_data);
        assert!(!e.query_root("c").wants_data, "one character skips the database");
        let web = e.query_root("zzzqqqxxx");
        assert_eq!(web.async_insert_at, 0, "database hits sit above the web fallback");
    }

    #[test]
    fn app_rows_carry_the_icon_path_and_action() {
        let mut e = engine();
        let row = e.query_root("calculator").rows.into_iter().next().unwrap();
        assert_eq!(row.icon_path, "/System/Applications/Calculator.app");
        assert_eq!(row.key, "app:com.apple.calculator");
        assert!(matches!(e.catalog.entry_action(row.item, true), Some(Action::FocusOrLaunchApp { .. })));
        assert!(matches!(e.catalog.entry_action(row.item, false), Some(Action::LaunchApp { .. })));
    }

    #[test]
    fn hidden_aliases_pins_and_usage_apply() {
        let mut e = engine();
        e.settings.hidden.push(app_key("com.apple.calculator"));
        e.settings.aliases.insert(app_key("com.microsoft.VSCode"), "ed".into());
        e.settings.pinned.push(app_key("com.microsoft.VSCode"));
        e.catalog.rebuild(&e.settings);
        assert!(!titles(&mut e, "calculator").contains(&"Calculator".to_string()));
        assert_eq!(titles(&mut e, "ed")[0], "Visual Studio Code");
        assert_eq!(titles(&mut e, "")[0], "Visual Studio Code", "pinned shows in suggestions");
        e.catalog.apply_usage(&[UsageRecord { key: "cmd:help".into(), count: 1000, last_used_at: 100 }], 100);
        e.catalog.rebuild(&e.settings);
        assert!(titles(&mut e, "").contains(&"Orbit Help".to_string()), "usage survives a rebuild");
    }

    #[test]
    fn starter_rows_explain_an_empty_index() {
        let dir = std::env::temp_dir().join(format!("orbit-ffi-empty-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let mut e = Engine::new(dir);
        assert!(!e.had_cached_apps);
        e.indexing = true;
        assert!(titles(&mut e, "").contains(&"Finding your apps…".to_string()));
        e.indexing = false;
        assert!(titles(&mut e, "").contains(&"No apps indexed yet".to_string()));
    }

    #[test]
    fn apps_cache_round_trips() {
        let dir = std::env::temp_dir().join(format!("orbit-ffi-apps-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        write_atomic(&dir.join(INDEX_FILE), serde_json::to_string(&apps()).unwrap().as_bytes()).unwrap();
        assert_eq!(load_apps(&dir), Some(apps()));
        assert!(Engine::new(dir).had_cached_apps);
    }

    #[test]
    fn corrupt_settings_are_kept_aside() {
        let dir = std::env::temp_dir().join(format!("orbit-ffi-corrupt-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("settings.json"), "{ nope").unwrap();
        let e = Engine::new(dir.clone());
        assert!(e.notice.is_some());
        assert!(dir.join("settings.damaged.json").exists());
    }
}
