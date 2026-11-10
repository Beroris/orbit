//! File search with Everything, and the commands and lists extensions add to the launcher.
//!
//! Files are searched on the files thread (`files.rs`), which answers with `WM_ORBIT_FILES`. An
//! extension's program runs on threads of its own (`extensions.rs`), and what it says comes back as
//! `WM_ORBIT_EXTENSION`. Only the answer to the newest question is shown; anything older is dropped.

use super::*;
use crate::everything::{self, FileHit, Problem};
use crate::extensions::{Event, EventKind, Installed};
use crate::files::{self, FileAnswer, FileReq, Files, Place};
use orbit_core::builtin_ext::{self, BuiltinExt};
use orbit_core::extension::{ActionKind, Item, ItemAction, Message, Mode, Request};
use orbit_core::settings::FileSort;

/// Left out of file results under the launcher's other results, and of the recent files: places
/// full of files nobody opens by hand. The file search itself searches everywhere.
const QUIET_PLACES: &str = r" !\appdata\ !\node_modules\ !\.git\ !c:\windows\ !\$recycle.bin\ !c:\programdata\ !\.cache\ !\temp\ !\.vs\";
/// How many files show under the launcher's other results.
const ROOT_FILES: u32 = 5;
/// The most the file search lists. Everything finds more, but nobody scrolls that far.
const LIST_FILES: u32 = 80;
/// Typing fewer characters than this finds too many files to be useful under other results.
pub(super) const ROOT_MIN_CHARS: usize = 3;
/// How often to look again while Everything is starting or still reading the drives.
const RETRY_MS: u32 = 1000;

/// What a keyword typed in the launcher opens.
pub(super) enum Jump {
    Files,
    Command { ext: String, command: String },
}

impl App {
    fn files(&mut self) -> &Files {
        let send = self.send;
        self.files.get_or_insert_with(|| Files::start(send, WM_ORBIT_FILES))
    }

    // --- Files ----------------------------------------------------------------------------

    /// Asks Everything for files matching the query, for the file search or under other results.
    pub(super) fn search_files(&mut self, place: Place, query: &str) {
        let everything = &self.settings.extensions.everything;
        if !everything.enabled {
            return;
        }
        let q = query.trim();
        let (text, max, sort) = match place {
            Place::Root => (format!("{q}{QUIET_PLACES}"), ROOT_FILES, everything.sort),
            // Nothing typed lists what changed last, which is usually what's wanted next.
            Place::List if q.is_empty() => (format!("file:{QUIET_PLACES}"), LIST_FILES, FileSort::Recent),
            Place::List => (q.to_string(), LIST_FILES, everything.sort),
        };
        self.file_seq += 1;
        let seq = self.file_seq;
        self.file_latest[place as usize] = seq;
        self.files().search(seq, place, text, max, sort);
    }

    pub(super) fn on_files(&mut self, answer: FileAnswer) {
        match answer {
            FileAnswer::Icon { hash, size, pixels } => {
                let has_pixels = pixels.is_some();
                self.icons.insert(hash, size, pixels);
                if has_pixels && self.visible {
                    self.redraw();
                }
            }
            FileAnswer::Results { seq, place, result, .. } => {
                if seq != self.file_latest[place as usize] {
                    return;
                }
                match place {
                    // A problem is the file search's to explain. Under other results, files just don't show.
                    Place::Root => self.merge_root_files(result.unwrap_or_default()),
                    Place::List => self.show_file_list(result),
                }
            }
        }
    }

    fn file_row(&mut self, hit: &FileHit) -> Row {
        let path = hit.full_path();
        let hash = files::icon_hash(&path, false);
        self.remember_icon_path(hash, &path, false);
        let symbol = if hit.is_folder { GLYPH_FOLDER } else { GLYPH_FILE };
        let title = if hit.folder.is_empty() { format!("{} drive", hit.name) } else { hit.name.clone() };
        Row {
            title,
            subtitle: files::shown_folder(&hit.folder),
            kind: "File",
            icon: RowIcon::File(hash, symbol),
            accent: false,
            primary: Some(Op::OpenFile(path.clone())),
            keys: None,
            accessory: Some(files::kind_of(hit)),
            source: RowSource::File { path, is_folder: hit.is_folder },
        }
    }

    /// Where an icon that isn't loaded yet is to be read from, when its row comes into view.
    fn remember_icon_path(&mut self, hash: u64, path: &str, picture: bool) {
        // Thousands of files a session would add up. Rows still showing put theirs back.
        if self.icon_paths.len() > 4000 {
            self.icon_paths.clear();
        }
        self.icon_paths.entry(hash).or_insert_with(|| (path.to_string(), picture));
    }

    /// Asks for a file's icon, or an extension's picture, if the row showing it has one.
    pub(super) fn request_file_icon(&mut self, hash: u64, size: usize) {
        let Some((path, picture)) = self.icon_paths.get(&hash).cloned() else { return };
        if self.icons.touch(hash, size) {
            self.files().send(FileReq::Icon { hash, path, size, picture });
        }
    }

    /// Where the files under the launcher's other results start: their title, or the end.
    pub(super) fn root_files_start(&self) -> usize {
        let rows = &self.view.rows;
        match rows.iter().position(|row| matches!(row.source, RowSource::File { .. })) {
            Some(at) if at > 0 && rows[at - 1].is_header() => at - 1,
            Some(at) => at,
            None => rows.len(),
        }
    }

    /// Files go last, under a title of their own, so nothing above them moves when they arrive.
    fn merge_root_files(&mut self, hits: Vec<FileHit>) {
        if self.view.scope != Scope::Root || self.menu.is_some() {
            return;
        }
        let start = self.root_files_start();
        self.view.rows.truncate(start);
        if !hits.is_empty() {
            self.view.rows.push(Row::header("Files"));
            for hit in &hits {
                let row = self.file_row(hit);
                self.view.rows.push(row);
            }
        }
        self.view.select(self.view.selected);
        self.after_selection_change();
        self.redraw();
    }

    fn show_file_list(&mut self, result: Result<Vec<FileHit>, Problem>) {
        if self.view.scope != Scope::Files {
            return;
        }
        self.view.rows.clear();
        match result {
            Ok(hits) => {
                if self.read_query().trim().is_empty() && !hits.is_empty() {
                    self.view.rows.push(Row::header("Recently changed"));
                }
                for hit in &hits {
                    let row = self.file_row(hit);
                    self.view.rows.push(row);
                }
            }
            Err(problem) => {
                if matches!(problem, Problem::Loading) || self.everything_starting.is_some_and(|at| at.elapsed().as_secs() < 15) {
                    unsafe {
                        let _ = SetTimer(Some(self.main), TIMER_FILES_RETRY, RETRY_MS, None);
                    }
                }
                self.view.rows = self.everything_problem_rows(&problem);
            }
        }
        self.view.select(self.view.selected);
        self.after_selection_change();
        self.redraw();
    }

    /// What the file search shows when Everything can't answer, with what can be done about it.
    fn everything_problem_rows(&self, problem: &Problem) -> Vec<Row> {
        let mut rows = Vec::new();
        match problem {
            Problem::NotInstalled if self.everything_installing => {
                rows.push(Row::info("Installing Everything\u{2026}", "Windows asks for permission once. This takes a minute.", GLYPH_FILE));
            }
            Problem::NotInstalled => {
                rows.push(Row::op("Install Everything", problem.detail(), GLYPH_ADD, Op::EverythingInstall));
                rows.push(Row::op("Visit voidtools.com", "Everything's own website", GLYPH_WEB, Op::OpenUrl(everything::WEBSITE.into())));
            }
            Problem::NotRunning if self.everything_starting.is_some() => {
                rows.push(Row::info("Starting Everything\u{2026}", "", GLYPH_FILE));
            }
            Problem::NotRunning => rows.push(Row::op("Start Everything", problem.detail(), GLYPH_PLAY, Op::EverythingStart)),
            Problem::Loading => rows.push(Row::info(problem.title(), problem.detail(), GLYPH_FILE)),
            Problem::NoAnswer => rows.push(Row::op("Try again", format!("{}. {}", problem.title(), problem.detail()), GLYPH_REFRESH, Op::EverythingRetry)),
        }
        rows.push(Row::op("File search settings", "Turn it off, change its keyword, or which files come first", GLYPH_SETTINGS, Op::OpenExtensionsSettings));
        rows
    }

    pub(super) fn on_everything_installed(&mut self, result: Result<(), String>) {
        self.everything_installing = false;
        crate::settings_win::everything_changed();
        crate::setup::everything_changed();
        match result {
            Ok(()) => {
                self.everything_starting = Some(Instant::now());
                self.notify("Everything is installed", "Orbit can search your files now. Type f and a space in the launcher.");
            }
            Err(why) => self.report(&why),
        }
        if self.visible && self.view.scope == Scope::Files {
            self.refresh_results();
        }
    }

    pub(super) fn on_files_retry(&mut self) {
        if self.visible && self.view.scope == Scope::Files && self.menu.is_none() {
            self.refresh_results();
        }
    }

    // --- Running what a row offers --------------------------------------------------------

    /// The file and extension rows' actions.
    pub(super) fn execute_ext(&mut self, op: Op) {
        match op {
            Op::OpenFile(path) => {
                self.hide(false);
                launch::spawn(LaunchRequest::open(&path), self.send, WM_ORBIT_LAUNCH_FAILED);
                // Everything counts what's opened, for "most opened first".
                if let Some(files) = &self.files {
                    files.send(FileReq::Opened(path));
                }
            }
            Op::RevealFile(path) => {
                self.hide(false);
                reveal(&path, self.send);
            }
            Op::CopyPath(path) => {
                self.hide(true);
                if !clipboard::write_text(&path) {
                    self.report("Couldn't reach the clipboard");
                }
            }
            Op::CopyFile(path) => {
                self.hide(true);
                if !clipboard::write_files(&[path.as_str()]) {
                    self.report("Couldn't reach the clipboard");
                }
            }
            Op::OpenFileWith(path) => {
                self.hide(false);
                let request = LaunchRequest { file: "rundll32.exe".into(), parameters: format!("shell32.dll,OpenAs_RunDLL {path}"), working_dir: String::new(), run_as_admin: false };
                launch::spawn(request, self.send, WM_ORBIT_LAUNCH_FAILED);
            }
            Op::FileAsAdmin(path) => {
                self.hide(false);
                launch::spawn(LaunchRequest { run_as_admin: true, ..LaunchRequest::open(&path) }, self.send, WM_ORBIT_LAUNCH_FAILED);
            }
            Op::TerminalHere(path) => {
                self.hide(false);
                // A quoted path can't end in a backslash, or it escapes the quote: "C:\" becomes "C:\.".
                let folder = if path.ends_with('\\') { format!("{path}.") } else { path };
                let request = LaunchRequest { file: "wt.exe".into(), parameters: format!("-d \"{folder}\""), working_dir: String::new(), run_as_admin: false };
                launch::spawn(request, self.send, WM_ORBIT_LAUNCH_FAILED);
            }
            Op::EverythingInstall => {
                if self.everything_installing {
                    return;
                }
                self.everything_installing = true;
                let main = self.send;
                let _ = std::thread::Builder::new().name("orbit-everything-install".into()).spawn(move || {
                    let result = everything::install();
                    main.post(WM_ORBIT_EVERYTHING_INSTALLED, util::box_for_post(result), 0);
                });
                self.refresh_results();
            }
            Op::EverythingStart => {
                if everything::start() {
                    self.everything_starting = Some(Instant::now());
                    self.refresh_results();
                    unsafe {
                        let _ = SetTimer(Some(self.main), TIMER_FILES_RETRY, RETRY_MS, None);
                    }
                } else {
                    self.report("Couldn't start Everything. Start it from the Start menu.");
                }
            }
            Op::EverythingRetry => self.refresh_results(),
            Op::ExtAction { ext, command, item, action } => self.run_extension_action(ext, command, item, action),
            Op::OpenExtensionsSettings => {
                self.hide(false);
                crate::settings_win::open_on_extensions(self.editor_context(), self.settings.clone(), self.dir.clone(), &self.catalog.apps);
                self.tell_settings(None);
            }
            _ => {}
        }
    }

    // --- Keywords -------------------------------------------------------------------------

    /// A keyword and a space at the start of the launcher's search, such as "ghi bug", opens what
    /// it belongs to. A quicklink with the same alias wins, as it was there first.
    pub(super) fn keyword_jump(&self, query: &str) -> Option<(Jump, String)> {
        let (word, rest) = query.split_once(' ')?;
        if word.is_empty() || self.settings.links.iter().any(|link| link.alias.eq_ignore_ascii_case(word)) {
            return None;
        }
        let everything = &self.settings.extensions.everything;
        if everything.enabled && !everything.alias.is_empty() && everything.alias.eq_ignore_ascii_case(word) {
            return Some((Jump::Files, rest.to_string()));
        }
        let matches = |keyword: &str| !keyword.is_empty() && keyword.eq_ignore_ascii_case(word);
        for ext in BuiltinExt::ALL.into_iter().filter(|ext| ext.is_provider() && ext.is_on(&self.settings)) {
            for command in ext.list_commands() {
                if matches(builtin_ext::command_keyword(&self.settings, ext.id(), command.id, command.keyword)) {
                    return Some((Jump::Command { ext: ext.id().to_string(), command: command.id.to_string() }, rest.to_string()));
                }
            }
        }
        for installed in &self.extensions {
            if !self.is_enabled(installed.id()) {
                continue;
            }
            for spec in &installed.manifest.commands {
                if spec.mode == Mode::List && matches(builtin_ext::command_keyword(&self.settings, installed.id(), &spec.id, &spec.keyword)) {
                    return Some((Jump::Command { ext: installed.id().to_string(), command: spec.id.clone() }, rest.to_string()));
                }
            }
        }
        None
    }

    /// Opens what a keyword belongs to, with what was typed after it. Going back returns to an
    /// empty search, not to the keyword, which would only open it again.
    pub(super) fn jump(&mut self, to: Jump, rest: String) {
        let scope = match to {
            Jump::Files => Scope::Files,
            Jump::Command { ext, command } => match self.extension_scope(&ext, &command) {
                Some(scope) => scope,
                None => return,
            },
        };
        self.push_view();
        if let Some(saved) = self.view.stack.last_mut() {
            saved.query.clear();
            saved.selected = 0;
        }
        self.status = None;
        self.set_query_text(&rest);
        self.set_scope(scope, true);
    }

    // --- Extensions -----------------------------------------------------------------------

    /// Whether an item answers to a keyword and a space, rather than an alias: Everything, once
    /// it has one, and every command that opens a list.
    pub(super) fn opens_with_keyword(&self, key: &str) -> bool {
        key == crate::settings_win::FILES_KEY
            || self.catalog.extension_commands.iter().any(|command| command.keyword.is_some() && crate::catalog::extension_key(&command.ext, &command.command) == key)
    }

    /// Finds the extensions in the extensions folder again, and gives the catalog their commands.
    pub fn discover_extensions(&mut self) {
        let (found, broken) = crate::extensions::discover(&self.dir);
        // A program from a folder that's gone or changed stops. It starts again when next used.
        for old in &self.extensions {
            if !found.contains(old) {
                self.runners.stop(old.id());
            }
        }
        let mut commands = Vec::new();
        for installed in &found {
            let icon = installed.icon_path().map(|path| path.to_string_lossy().into_owned());
            if let Some(path) = &icon {
                let hash = files::icon_hash(path, true);
                self.remember_icon_path(hash, path, true);
            }
            for spec in &installed.manifest.commands {
                commands.push(crate::catalog::ExtCommand {
                    ext: installed.id().to_string(),
                    command: spec.id.clone(),
                    title: spec.title.clone(),
                    subtitle: if spec.subtitle.is_empty() { installed.manifest.name.clone() } else { spec.subtitle.clone() },
                    keywords: format!("{} {}", spec.keywords, installed.manifest.name),
                    icon: icon.clone(),
                    keyword: (spec.mode == Mode::List).then(|| spec.keyword.clone()),
                });
            }
        }
        // The lists that come with Orbit. The catalog leaves out those turned off.
        for ext in BuiltinExt::ALL.into_iter().filter(|ext| ext.is_provider()) {
            for command in ext.list_commands() {
                commands.push(crate::catalog::ExtCommand {
                    ext: ext.id().to_string(),
                    command: command.id.to_string(),
                    title: command.title.to_string(),
                    subtitle: command.subtitle.to_string(),
                    keywords: format!("{} {}", command.keywords, ext.name()),
                    keyword: Some(command.keyword.to_string()),
                    icon: None,
                });
            }
        }
        self.extensions = found;
        self.broken_extensions = broken;
        self.catalog.extension_commands = commands;
        self.catalog.rebuild(&self.settings);
    }

    /// Stops the programs of extensions that were turned off.
    pub(super) fn stop_disabled_extensions(&mut self) {
        let off: Vec<String> = self.extensions.iter().map(|e| e.id().to_string()).filter(|id| !self.settings.extensions.enabled.contains(id)).collect();
        for id in off {
            self.runners.stop(&id);
        }
        // What GitHub or AI Usage said goes with them.
        if BuiltinExt::ALL.into_iter().any(|ext| ext.is_provider() && !ext.is_on(&self.settings)) {
            self.providers.forget();
        }
    }

    /// An extension command's row icon: the symbol of one that comes with Orbit, or a folder
    /// extension's picture, or the extensions symbol.
    pub(super) fn extension_icon(&self, ext: &str, icon: Option<&str>) -> RowIcon {
        if let Some(builtin) = BuiltinExt::from_id(ext) {
            return RowIcon::Glyph(crate::rows::extension_glyph(builtin));
        }
        match icon {
            Some(path) => RowIcon::File(files::icon_hash(path, true), GLYPH_EXTENSION),
            None => RowIcon::Glyph(GLYPH_EXTENSION),
        }
    }

    fn installed(&self, ext: &str) -> Option<Installed> {
        self.extensions.iter().find(|installed| installed.id() == ext).cloned()
    }

    fn is_enabled(&self, ext: &str) -> bool {
        builtin_ext::extension_on(&self.settings, ext)
    }

    /// The display name of an extension, built in or from a folder.
    fn extension_name(&self, ext: &str) -> String {
        match BuiltinExt::from_id(ext) {
            Some(builtin) => builtin.name().to_string(),
            None => self.installed(ext).map_or_else(|| ext.to_string(), |installed| installed.manifest.name),
        }
    }

    /// The list scope for a command, or `None` after saying why it can't open.
    fn extension_scope(&mut self, ext: &str, command: &str) -> Option<Scope> {
        if let Some(builtin) = BuiltinExt::from_id(ext) {
            if !builtin.is_on(&self.settings) {
                self.report(&format!("{} is turned off. Turn it on in Settings, under Extensions.", builtin.name()));
                return None;
            }
            let Some(spec) = builtin.list_command(command) else {
                self.report(&format!("{} has no such command.", builtin.name()));
                return None;
            };
            return Some(Scope::Extension { ext: ext.to_string(), command: command.to_string(), title: spec.title.to_string(), placeholder: spec.placeholder.to_string() });
        }
        let Some(installed) = self.installed(ext) else {
            self.report("That extension isn't installed any more.");
            return None;
        };
        if !self.is_enabled(ext) {
            self.report(&format!("{} is turned off. Turn it on in Settings, under Extensions.", installed.manifest.name));
            return None;
        }
        let Some(spec) = installed.manifest.command(command) else {
            self.report(&format!("{} no longer has that command.", installed.manifest.name));
            return None;
        };
        Some(Scope::Extension { ext: ext.to_string(), command: command.to_string(), title: spec.title.clone(), placeholder: spec.placeholder.clone() })
    }

    /// Runs a command: opens its list, or runs it once.
    pub(super) fn open_extension_command(&mut self, ext: &str, command: &str) {
        if let Some(builtin) = BuiltinExt::from_id(ext) {
            if !builtin.is_on(&self.settings) {
                self.report(&format!("{} is turned off. Turn it on in Settings, under Extensions.", builtin.name()));
                return;
            }
            if let Some(tool) = crate::integrations::Tool::of_extension(builtin) {
                self.execute(Op::OpenVault(tool));
            } else if let Some(&first) = builtin.builtins().first() {
                self.run_builtin(first);
            } else if let Some(scope) = self.extension_scope(ext, command) {
                self.enter_scope(scope, qpc());
            }
            return;
        }
        let Some(installed) = self.installed(ext) else {
            self.report("That extension isn't installed any more.");
            return;
        };
        let Some(spec) = installed.manifest.command(command).cloned() else {
            self.report(&format!("{} no longer has that command.", installed.manifest.name));
            return;
        };
        if spec.mode == Mode::List {
            if let Some(scope) = self.extension_scope(ext, command) {
                self.enter_scope(scope, qpc());
            }
            return;
        }
        if !self.is_enabled(ext) {
            self.report(&format!("{} is turned off. Turn it on in Settings, under Extensions.", installed.manifest.name));
            return;
        }
        self.ext_requests += 1;
        let id = self.ext_requests;
        match self.runners.send(&installed, &Request::Run { id, command: command.to_string() }, self.send, WM_ORBIT_EXTENSION) {
            Ok(_) => {
                self.ext_waiting = Some((ext.to_string(), id));
                if self.visible {
                    self.status = Some(format!("{}\u{2026}", spec.title));
                    self.redraw();
                }
            }
            Err(why) => self.report(&format!("{} couldn't start. {why}", installed.manifest.name)),
        }
        self.arm_extension_idle();
    }

    /// Asks the extension whose list is open for what matches the query.
    pub(super) fn search_extension(&mut self, ext: &str, command: &str, query: &str) {
        if let Some(builtin) = BuiltinExt::from_id(ext).filter(|builtin| builtin.is_provider()) {
            if !builtin.is_on(&self.settings) {
                self.view.rows = vec![Row::info(format!("{} is turned off", builtin.name()), "Turn it on in Settings, under Extensions", GLYPH_WARNING)];
                return;
            }
            self.ext_requests += 1;
            let id = self.ext_requests;
            self.ext_waiting = Some((ext.to_string(), id));
            if self.previewing {
                let said = crate::providers::preview(builtin, command, id, query, &self.dir);
                self.on_extension(Event { ext: ext.to_string(), run: 0, kind: EventKind::Said(said) });
                return;
            }
            self.providers.search(builtin, command, id, query, false, self.send, WM_ORBIT_EXTENSION);
            if self.view.rows.is_empty() {
                self.view.rows.push(Row::info("Loading\u{2026}", "", crate::rows::extension_glyph(builtin)));
            }
            return;
        }
        let Some(installed) = self.installed(ext).filter(|_| self.is_enabled(ext)) else {
            self.view.rows = vec![Row::info("This extension isn't available", "It was removed or turned off in Settings", GLYPH_WARNING)];
            return;
        };
        self.ext_requests += 1;
        let id = self.ext_requests;
        let request = Request::Search { id, command: command.to_string(), query: query.to_string() };
        match self.runners.send(&installed, &request, self.send, WM_ORBIT_EXTENSION) {
            Ok(_) => {
                self.ext_waiting = Some((ext.to_string(), id));
                if self.view.rows.is_empty() {
                    self.view.rows.push(Row::info("Loading\u{2026}", "", GLYPH_EXTENSION));
                }
            }
            Err(why) => self.view.rows = vec![Row::info(format!("{} couldn't start", installed.manifest.name), why, GLYPH_WARNING)],
        }
        self.arm_extension_idle();
    }

    /// A row for an item in a list. `installed` is the folder extension it came from, or `None` for
    /// one that comes with Orbit.
    fn extension_row(&mut self, ext: &str, installed: Option<&Installed>, command: &str, item: Item) -> Row {
        let picture = installed.zip(item.icon.as_deref()).and_then(|(installed, icon)| installed.item_icon(icon));
        let icon = match picture {
            Some((path, picture)) => {
                let hash = files::icon_hash(&path, picture);
                self.remember_icon_path(hash, &path, picture);
                RowIcon::File(hash, GLYPH_EXTENSION)
            }
            None => self.extension_icon(ext, installed.and_then(|installed| installed.icon_path()).map(|path| path.to_string_lossy().into_owned()).as_deref()),
        };
        let ext = ext.to_string();
        let primary = item.actions.first().map(|action| Op::ExtAction { ext: ext.clone(), command: command.to_string(), item: item.id.clone(), action: action.clone() });
        Row {
            title: item.title,
            subtitle: item.subtitle,
            kind: "",
            icon,
            accent: false,
            primary,
            keys: None,
            accessory: (!item.accessory.is_empty()).then_some(item.accessory),
            source: RowSource::Extension { ext, command: command.to_string(), item: item.id, actions: item.actions, meters: item.meters },
        }
    }

    /// The extension whose list is showing, behind a menu or not, and the text typed in it.
    fn open_extension_list(&self) -> Option<(String, String, String)> {
        let (scope, query) = match &self.menu {
            Some(menu) => (&menu.behind.scope, menu.behind.query.clone()),
            None => (&self.view.scope, self.read_query()),
        };
        match scope {
            Scope::Extension { ext, command, .. } => Some((ext.clone(), command.clone(), query)),
            _ => None,
        }
    }

    pub(super) fn on_extension(&mut self, event: Event) {
        let builtin = BuiltinExt::from_id(&event.ext);
        // News from a program that has since been stopped, or started again, is old news. The
        // lists that come with Orbit have no program; only the newest question's answer counts.
        if builtin.is_none() && self.runners.current_run(&event.ext) != Some(event.run) {
            return;
        }
        let name = self.extension_name(&event.ext);
        let waited_on = |app: &App, id: u64| app.ext_waiting.as_ref().is_some_and(|(ext, waiting)| *ext == event.ext && *waiting == id);
        let showing = self.menu.is_none() && matches!(&self.view.scope, Scope::Extension { ext, .. } if *ext == event.ext);
        match event.kind {
            EventKind::Ended(why) => {
                self.runners.ended(&event.ext, event.run);
                let was_waiting = self.ext_waiting.as_ref().is_some_and(|(ext, _)| *ext == event.ext);
                self.ext_waiting = None;
                if showing {
                    self.view.rows = vec![Row::info(format!("{name} stopped"), why, GLYPH_WARNING)];
                    self.view.select(0);
                    self.redraw();
                } else if was_waiting {
                    let message = if why.is_empty() { format!("{name} stopped.") } else { format!("{name} stopped: {why}") };
                    self.report(&message);
                }
            }
            EventKind::Said(Message::Items { id, items, empty }) => {
                if !waited_on(self, id) || !showing {
                    return;
                }
                self.ext_waiting = None;
                if builtin.is_some() {
                    // "Refreshing…" or "Marking as read…" is done.
                    self.status = None;
                }
                let installed = self.installed(&event.ext);
                if builtin.is_none() && installed.is_none() {
                    return;
                }
                let Scope::Extension { command, .. } = self.view.scope.clone() else { return };
                let mut rows = Vec::with_capacity(items.len());
                for item in items {
                    rows.push(self.extension_row(&event.ext, installed.as_ref(), &command, item));
                }
                if rows.is_empty() {
                    rows.push(Row::info(empty.unwrap_or_else(|| "Nothing found".into()), "", GLYPH_INFO));
                }
                self.view.rows = rows;
                self.view.select(self.view.selected);
                self.after_selection_change();
                self.redraw();
            }
            EventKind::Said(Message::Done { id, message, close }) => {
                if waited_on(self, id) {
                    self.ext_waiting = None;
                }
                if close {
                    self.hide(true);
                }
                if let Some(message) = message {
                    self.report(&message);
                } else if self.visible {
                    self.status = None;
                    self.redraw();
                }
            }
            EventKind::Said(Message::Error { id, message }) => {
                if !waited_on(self, id) {
                    return;
                }
                self.ext_waiting = None;
                if builtin.is_some() {
                    self.status = None;
                }
                if showing {
                    self.view.rows = vec![Row::info(message, name, GLYPH_WARNING)];
                    self.view.select(0);
                    self.redraw();
                } else {
                    self.report(&format!("{name}: {message}"));
                }
            }
            EventKind::Said(Message::Notify { message }) => self.report(&message),
        }
    }

    fn run_extension_action(&mut self, ext: String, command: String, item: String, action: ItemAction) {
        match action.kind {
            ActionKind::Open(target) => {
                self.hide(false);
                if orbit_core::extension::is_full_path(&target) {
                    launch::spawn(LaunchRequest::open(&target), self.send, WM_ORBIT_LAUNCH_FAILED);
                } else {
                    self.open_url(&target);
                }
            }
            ActionKind::Reveal(path) => {
                self.hide(false);
                reveal(&path, self.send);
            }
            ActionKind::Copy(text) => self.execute(Op::Copy(text)),
            ActionKind::Paste(text) => self.execute(Op::PasteText(text)),
            ActionKind::SetQuery(text) => self.execute(Op::SetQuery(text)),
            ActionKind::Callback(action_id) => {
                let query = self.open_extension_list().map(|(_, _, query)| query).unwrap_or_default();
                self.leave_menu();
                if let Some(builtin) = BuiltinExt::from_id(&ext).filter(|builtin| builtin.is_provider()) {
                    self.ext_requests += 1;
                    let id = self.ext_requests;
                    self.ext_waiting = Some((ext, id));
                    if self.previewing {
                        let said = crate::providers::preview(builtin, &command, id, &query, &self.dir);
                        self.on_extension(Event { ext: builtin.id().to_string(), run: 0, kind: EventKind::Said(said) });
                    } else {
                        self.providers.action(builtin, &command, &item, &action_id, id, &query, self.send, WM_ORBIT_EXTENSION);
                        self.status = Some(if action_id == "read" { "Marking as read\u{2026}".into() } else { "Refreshing\u{2026}".into() });
                    }
                    self.redraw();
                    return;
                }
                let Some(installed) = self.installed(&ext) else { return };
                self.ext_requests += 1;
                let id = self.ext_requests;
                let request = Request::Action { id, command, item, action: action_id, query };
                match self.runners.send(&installed, &request, self.send, WM_ORBIT_EXTENSION) {
                    Ok(_) => self.ext_waiting = Some((ext, id)),
                    Err(why) => self.report(&format!("{} couldn't start. {why}", installed.manifest.name)),
                }
                self.redraw();
            }
        }
    }

    /// Programs left idle are stopped a while after the launcher was last used.
    pub(super) fn arm_extension_idle(&mut self) {
        if self.runners.any_running() {
            let after = crate::extensions::IDLE_STOP.as_millis() as u32 + 5_000;
            unsafe {
                let _ = SetTimer(Some(self.main), TIMER_EXT_IDLE, after, None);
            }
        }
    }

    pub(super) fn on_extension_idle(&mut self) {
        self.runners.stop_idle();
        self.arm_extension_idle();
    }
}

/// Shows a file or folder selected in File Explorer.
fn reveal(path: &str, send: SendHwnd) {
    let request = LaunchRequest { file: "explorer.exe".into(), parameters: format!("/select,\"{path}\""), working_dir: String::new(), run_as_admin: false };
    launch::spawn(request, send, WM_ORBIT_LAUNCH_FAILED);
}

impl App {
    /// Settings added an extension, or wants one taken away. The folder is looked at again either
    /// way, and Settings is made again to show what's there now.
    pub(super) fn on_extensions_changed(&mut self, change: Option<crate::extensions::Change>) {
        let mut message = None;
        match change {
            Some(crate::extensions::Change::Remove(dir)) => {
                if let Some(installed) = self.extensions.iter().find(|installed| installed.dir == dir).cloned() {
                    // Its program may be running from that folder.
                    self.runners.stop(installed.id());
                    match crate::extensions::remove(&installed, &self.dir) {
                        Ok(()) => {
                            let id = installed.id().to_string();
                            self.settings.extensions.enabled.retain(|on| *on != id);
                            let prefix = format!("{id}/");
                            self.settings.extensions.keywords.retain(|key, _| !key.starts_with(&prefix));
                            self.save_settings();
                            message = Some(format!("Removed {}", installed.manifest.name));
                        }
                        Err(why) => message = Some(why),
                    }
                }
            }
            Some(crate::extensions::Change::Added(name)) => message = Some(format!("Added {name}. Turn it on to use it.")),
            None => {}
        }
        self.discover_extensions();
        self.reopen_settings();
        if let Some(message) = message {
            self.report(&message);
        }
        if self.visible {
            self.refresh_results();
        }
    }
}
