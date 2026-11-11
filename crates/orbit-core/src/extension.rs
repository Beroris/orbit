//! Extensions: programs that add commands to Orbit's launcher.
//!
//! An extension is a folder holding `orbit-extension.json` and whatever program it runs. Orbit
//! starts that program when one of its commands is first used and talks to it over standard input
//! and output, one JSON object per line. The program can be written in anything that reads and
//! writes lines: PowerShell, Python, Node, a compiled exe. docs/EXTENSIONS.md is the reference.
//!
//! This module is the shared part: reading and checking manifests, and the messages both ways. The
//! host owns the processes. Nothing here runs anything.

use crate::action::{UrlCheck, check_url};
use serde::{Deserialize, Serialize};
use serde_json::Value;

/// The manifest's file name, inside the extension's folder.
pub const MANIFEST_FILE: &str = "orbit-extension.json";
/// The protocol this Orbit speaks. A manifest may ask for this or anything older.
pub const PROTOCOL: u32 = 1;
/// A line from an extension longer than this is dropped, so a runaway program can't eat memory.
pub const MAX_LINE_BYTES: usize = 1024 * 1024;
/// The most items one answer may hold. More are cut off.
pub const MAX_ITEMS: usize = 200;
/// The most actions one item may offer.
pub const MAX_ACTIONS: usize = 12;
const MAX_COMMANDS: usize = 32;
const MAX_TITLE: usize = 200;
const MAX_SUBTITLE: usize = 400;
const MAX_KEYWORD: usize = 16;

/// What a command does when it's chosen.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Mode {
    /// Opens a list that the extension fills as you type.
    #[default]
    List,
    /// Runs once, with no list. The extension answers with `done`.
    Run,
}

/// One command an extension adds to the launcher.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct CommandSpec {
    pub id: String,
    pub title: String,
    #[serde(default)]
    pub subtitle: String,
    /// Typing this and a space in the launcher goes straight into the command.
    #[serde(default)]
    pub keyword: String,
    /// More words to find the command by.
    #[serde(default)]
    pub keywords: String,
    #[serde(default)]
    pub mode: Mode,
    /// What the search box says while the command's list is open.
    #[serde(default)]
    pub placeholder: String,
}

/// `orbit-extension.json`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Manifest {
    /// Lower case letters, digits, dots, dashes and underscores. Settings refer to it.
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub author: String,
    #[serde(default)]
    pub version: String,
    /// A picture in the folder, such as `icon.png`.
    #[serde(default)]
    pub icon: String,
    /// The program and its arguments. A bare first word is looked for in the folder first, then
    /// on the PATH, so `["powershell", "-File", "main.ps1"]` and `["main.exe"]` both work.
    pub run: Vec<String>,
    pub commands: Vec<CommandSpec>,
    /// The protocol it was written for.
    #[serde(default = "first_protocol")]
    pub protocol: u32,
}

fn first_protocol() -> u32 {
    1
}

fn valid_id(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= 64
        && id.chars().next().is_some_and(|c| c.is_ascii_lowercase() || c.is_ascii_digit())
        && id.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || matches!(c, '.' | '-' | '_'))
}

/// A keyword is one word, typed before a space.
pub fn valid_keyword(keyword: &str) -> bool {
    keyword.is_empty() || (keyword.chars().count() <= MAX_KEYWORD && !keyword.chars().any(|c| c.is_whitespace() || c.is_control()))
}

impl Manifest {
    /// Reads and checks a manifest. The message says what's wrong in words a person can act on.
    pub fn parse(text: &str) -> Result<Manifest, String> {
        let text = text.trim_start_matches('\u{FEFF}');
        let manifest: Manifest = serde_json::from_str(text).map_err(|e| format!("{MANIFEST_FILE} isn't valid: {e}"))?;
        manifest.check()?;
        Ok(manifest)
    }

    fn check(&self) -> Result<(), String> {
        if !valid_id(&self.id) {
            return Err(format!("\"{}\" can't be an id. Use lower case letters, digits, dots and dashes.", self.id));
        }
        if self.name.trim().is_empty() || self.name.chars().count() > 64 {
            return Err("The name must be 1 to 64 characters.".into());
        }
        if self.protocol > PROTOCOL {
            return Err(format!("It needs a newer Orbit (protocol {}).", self.protocol));
        }
        if self.run.first().is_none_or(|program| program.trim().is_empty()) {
            return Err("\"run\" must name the program to start.".into());
        }
        if self.commands.is_empty() || self.commands.len() > MAX_COMMANDS {
            return Err(format!("It must have 1 to {MAX_COMMANDS} commands."));
        }
        let mut seen = std::collections::HashSet::new();
        for command in &self.commands {
            if !valid_id(&command.id) {
                return Err(format!("\"{}\" can't be a command id.", command.id));
            }
            if !seen.insert(command.id.as_str()) {
                return Err(format!("Two commands are called \"{}\".", command.id));
            }
            if command.title.trim().is_empty() {
                return Err(format!("Command \"{}\" needs a title.", command.id));
            }
            if !valid_keyword(&command.keyword) {
                return Err(format!("Command \"{}\" has a keyword with a space in it, or longer than {MAX_KEYWORD}.", command.id));
            }
        }
        if !self.icon.is_empty() && inside_folder(&self.icon).is_none() {
            return Err("The icon must be a file in the extension's folder.".into());
        }
        Ok(())
    }

    pub fn command(&self, id: &str) -> Option<&CommandSpec> {
        self.commands.iter().find(|command| command.id == id)
    }
}

/// A relative path that stays inside the extension's folder, with `/` turned into `\`. `None` for
/// absolute paths and anything that climbs out with `..`.
pub fn inside_folder(path: &str) -> Option<String> {
    let path = path.trim();
    if path.is_empty() || path.starts_with(['/', '\\']) || path.contains(':') {
        return None;
    }
    let mut parts = Vec::new();
    for part in path.split(['/', '\\']) {
        match part {
            "" | "." => {}
            ".." => return None,
            other => parts.push(other),
        }
    }
    (!parts.is_empty()).then(|| parts.join("\\"))
}

/// What Orbit says to an extension.
#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Request {
    /// Sent once, when the program starts.
    Hello { orbit: String, protocol: u32 },
    /// The text in the search box changed, or the command was opened (with an empty query).
    /// `filter` is the id of the filter chosen from those the last answer offered, if one was.
    Search {
        id: u64,
        command: String,
        query: String,
        #[serde(skip_serializing_if = "String::is_empty")]
        filter: String,
    },
    /// A `run` command was chosen.
    Run { id: u64, command: String },
    /// A `callback` action was chosen on an item. `query` and `filter` are the list's, so that an
    /// answer with items can be for the same search.
    Action {
        id: u64,
        command: String,
        item: String,
        action: String,
        query: String,
        #[serde(skip_serializing_if = "String::is_empty")]
        filter: String,
    },
}

impl Request {
    pub fn to_line(&self) -> String {
        let mut line = serde_json::to_string(self).expect("requests always serialise");
        line.push('\n');
        line
    }
}

/// Something an item can do, as Orbit understands it after checking.
#[derive(Clone, Debug, PartialEq)]
pub enum ActionKind {
    /// A web address or an app's link, or a file or folder by its full path.
    Open(String),
    /// Show a file or folder in File Explorer.
    Reveal(String),
    Copy(String),
    /// Type the text into the window Orbit was opened over.
    Paste(String),
    /// Ask the extension to do it, by the action's id.
    Callback(String),
    /// Put this in the search box.
    SetQuery(String),
}

#[derive(Clone, Debug, PartialEq)]
pub struct ItemAction {
    pub title: String,
    pub kind: ActionKind,
    /// A key combination shown beside it in the menu, as the extension asked.
    pub shortcut: Option<String>,
    /// Its symbol in the menu, by name: one of [`ACTION_ICONS`]. Without one, or with a name Orbit
    /// doesn't know, its kind chooses.
    pub icon: Option<String>,
}

/// The symbols an action can name for itself in the menu.
pub const ACTION_ICONS: [&str; 30] = [
    "open", "globe", "link", "hash", "branch", "terminal", "copy", "paste", "text", "folder", "file", "image", "note", "edit", "trash",
    "check", "refresh", "search", "star", "pin", "tag", "person", "key", "clock", "download", "play", "add", "eye-off", "settings", "swap",
];

/// One row in an extension's list.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Item {
    pub id: String,
    pub title: String,
    pub subtitle: String,
    /// Short text on the right of the row.
    pub accessory: String,
    /// A picture: a path relative to the extension's folder, or any file's full path, whose own
    /// icon is shown.
    pub icon: Option<String>,
    /// The first runs on Enter, the second on Ctrl+Enter. All of them are in Ctrl+K.
    pub actions: Vec<ItemAction>,
    /// Small bars on the right of the row, such as how much of a limit is used. They take the
    /// accessory's place.
    pub meters: Vec<Meter>,
    /// Words in coloured pills on the right, before the accessory, such as "Conflicts".
    pub tags: Vec<Tag>,
    /// A colour for the row's symbol, when it has no picture of its own, such as green for a pull
    /// request that's open.
    pub tint: Option<Tone>,
    /// The heading it's listed under. A heading is shown where it changes from the row before.
    pub section: String,
    /// The filters it's shown under, for a list Orbit makes itself, which filters for itself. An
    /// extension from a folder is sent the filter and answers with what it covers.
    pub facets: Vec<String>,
}

/// One of a few colours a tag or symbol can take, each drawn to read on light and dark.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum Tone {
    #[default]
    Grey,
    Green,
    Red,
    Orange,
    Purple,
    Blue,
}

impl Tone {
    /// "green", "red" and so on. Anything else is grey.
    pub fn parse(name: &str) -> Tone {
        match name.trim().to_ascii_lowercase().as_str() {
            "green" => Tone::Green,
            "red" => Tone::Red,
            "orange" | "amber" | "yellow" => Tone::Orange,
            "purple" | "violet" => Tone::Purple,
            "blue" => Tone::Blue,
            _ => Tone::Grey,
        }
    }
}

/// A word or two in a coloured pill, such as "Draft" or "Checks failing".
#[derive(Clone, Debug, PartialEq)]
pub struct Tag {
    pub text: String,
    pub tone: Tone,
}

impl Tag {
    pub fn new(text: &str, tone: Tone) -> Tag {
        Tag { text: text.into(), tone }
    }
}

/// The most tags one item may show.
pub const MAX_TAGS: usize = 3;
/// The most filters one answer may offer.
pub const MAX_FILTERS: usize = 60;

/// One way to narrow a list, offered in the drop-down at the right of the search box.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct FilterOption {
    /// Sent back in the next search when it's chosen. The first one offered is "everything", and
    /// its id is usually empty.
    pub id: String,
    pub title: String,
    /// A little more, such as how many rows it covers.
    pub subtitle: String,
}

/// A bar that is filled part of the way, with a word or two on it, such as "Week 85%".
#[derive(Clone, Debug, PartialEq)]
pub struct Meter {
    pub label: String,
    /// From 0 to 1.
    pub value: f32,
}

/// The most meters one item may show.
pub const MAX_METERS: usize = 3;

/// What an extension says to Orbit.
#[derive(Clone, Debug, PartialEq)]
pub enum Message {
    /// The list for a search, replacing what's shown, and the filters it can be narrowed by.
    Items { id: u64, items: Vec<Item>, empty: Option<String>, filters: Vec<FilterOption> },
    /// A `run` or `callback` finished. `close` hides the launcher.
    Done { id: u64, message: Option<String>, close: bool },
    Error { id: u64, message: String },
    /// A message to show, whatever's going on.
    Notify { message: String },
}

fn text(value: &Value, key: &str, limit: usize) -> String {
    let text = value.get(key).and_then(Value::as_str).unwrap_or("");
    let text: String = text.chars().map(|c| if c.is_control() { ' ' } else { c }).collect();
    if text.chars().count() > limit {
        let mut cut: String = text.chars().take(limit.saturating_sub(1)).collect();
        cut.push('\u{2026}');
        cut
    } else {
        text
    }
}

/// Text that is passed on whole, such as what to copy. Only NUL is taken out.
fn raw(value: &Value, key: &str) -> Option<String> {
    value.get(key).and_then(Value::as_str).map(|text| text.replace('\0', ""))
}

fn id_of(value: &Value) -> u64 {
    value.get("id").and_then(Value::as_u64).unwrap_or(0)
}

/// An address an extension may ask Orbit to open: a web or app link with a scheme that can't run
/// code, or a full path on this PC.
fn openable(target: &str) -> bool {
    let target = target.trim();
    if is_full_path(target) {
        return true;
    }
    matches!(check_url(target), UrlCheck::Safe | UrlCheck::Unusual(_))
}

/// `C:\…` or `\\server\…`.
pub fn is_full_path(text: &str) -> bool {
    let bytes = text.as_bytes();
    (bytes.len() >= 3 && bytes[0].is_ascii_alphabetic() && bytes[1] == b':' && matches!(bytes[2], b'\\' | b'/')) || text.starts_with("\\\\")
}

fn parse_action(value: &Value) -> Option<ItemAction> {
    let title = text(value, "title", 80);
    let kind = match value.get("type").and_then(Value::as_str)? {
        "open" => {
            let target = raw(value, "target")?;
            openable(&target).then(|| ActionKind::Open(target.trim().to_string()))?
        }
        "reveal" => {
            let path = raw(value, "path")?;
            is_full_path(path.trim()).then(|| ActionKind::Reveal(path.trim().to_string()))?
        }
        "copy" => ActionKind::Copy(raw(value, "text")?),
        "paste" => ActionKind::Paste(raw(value, "text")?),
        "callback" => ActionKind::Callback(raw(value, "id").filter(|id| !id.is_empty())?),
        "set_query" => ActionKind::SetQuery(text(value, "query", 500)),
        _ => return None,
    };
    let title = if title.trim().is_empty() {
        match &kind {
            ActionKind::Open(_) => "Open",
            ActionKind::Reveal(_) => "Show in folder",
            ActionKind::Copy(_) => "Copy",
            ActionKind::Paste(_) => "Paste",
            ActionKind::Callback(_) => "Run",
            ActionKind::SetQuery(_) => "Search",
        }
        .to_string()
    } else {
        title
    };
    let shortcut = value.get("shortcut").and_then(Value::as_str).map(|keys| keys.chars().take(24).collect());
    let icon = value.get("icon").and_then(Value::as_str).map(|name| name.trim().to_ascii_lowercase()).filter(|name| ACTION_ICONS.contains(&name.as_str()));
    Some(ItemAction { title, kind, shortcut, icon })
}

/// A meter: `{"label": "Week", "value": 0.85}`. A value outside 0 to 1 is brought inside.
fn parse_meter(value: &Value) -> Option<Meter> {
    let fill = value.get("value").and_then(Value::as_f64)?;
    if !fill.is_finite() {
        return None;
    }
    Some(Meter { label: text(value, "label", 24), value: fill.clamp(0.0, 1.0) as f32 })
}

fn parse_item(value: &Value, at: usize) -> Option<Item> {
    let title = text(value, "title", MAX_TITLE);
    if title.trim().is_empty() {
        return None;
    }
    let actions = value
        .get("actions")
        .and_then(Value::as_array)
        .map(|list| list.iter().filter_map(parse_action).take(MAX_ACTIONS).collect())
        .unwrap_or_default();
    let id = match value.get("id") {
        Some(Value::String(id)) => id.clone(),
        Some(Value::Number(n)) => n.to_string(),
        _ => at.to_string(),
    };
    Some(Item {
        id,
        title,
        subtitle: text(value, "subtitle", MAX_SUBTITLE),
        accessory: text(value, "accessory", 40),
        icon: raw(value, "icon").map(|icon| icon.trim().to_string()).filter(|icon| !icon.is_empty()),
        actions,
        meters: value.get("meters").and_then(Value::as_array).map(|list| list.iter().filter_map(parse_meter).take(MAX_METERS).collect()).unwrap_or_default(),
        tags: value.get("tags").and_then(Value::as_array).map(|list| list.iter().filter_map(parse_tag).take(MAX_TAGS).collect()).unwrap_or_default(),
        tint: tone_of(value),
        section: text(value, "section", 60),
        facets: Vec::new(),
    })
}

/// `"color": "green"`, or the British spelling.
fn tone_of(value: &Value) -> Option<Tone> {
    value.get("color").or_else(|| value.get("colour")).and_then(Value::as_str).map(Tone::parse)
}

/// A tag: `{"text": "Draft", "color": "grey"}`. One with no words is left out.
fn parse_tag(value: &Value) -> Option<Tag> {
    let words = text(value, "text", 24);
    (!words.trim().is_empty()).then(|| Tag { text: words, tone: tone_of(value).unwrap_or_default() })
}

/// A filter: `{"id": "repo:o/r", "title": "o/r", "subtitle": "3"}`.
fn parse_filter(value: &Value) -> Option<FilterOption> {
    let title = text(value, "title", 60);
    if title.trim().is_empty() {
        return None;
    }
    Some(FilterOption { id: text(value, "id", 200), title, subtitle: text(value, "subtitle", 60) })
}

/// Reads one line from an extension. Lines that aren't a message Orbit knows are ignored, which
/// lets an extension print other things without breaking, and newer messages pass older Orbits.
pub fn parse_message(line: &str) -> Option<Message> {
    if line.len() > MAX_LINE_BYTES {
        return None;
    }
    let value: Value = serde_json::from_str(line.trim().trim_start_matches('\u{FEFF}')).ok()?;
    match value.get("type").and_then(Value::as_str)? {
        "items" => {
            let items = value
                .get("items")
                .and_then(Value::as_array)
                .map(|list| list.iter().take(MAX_ITEMS).enumerate().filter_map(|(at, item)| parse_item(item, at)).collect())
                .unwrap_or_default();
            let empty = Some(text(&value, "empty", 120)).filter(|empty| !empty.trim().is_empty());
            let filters = value.get("filters").and_then(Value::as_array).map(|list| list.iter().filter_map(parse_filter).take(MAX_FILTERS).collect()).unwrap_or_default();
            Some(Message::Items { id: id_of(&value), items, empty, filters })
        }
        "done" => Some(Message::Done {
            id: id_of(&value),
            message: Some(text(&value, "message", 300)).filter(|m| !m.trim().is_empty()),
            close: value.get("close").and_then(Value::as_bool).unwrap_or(false),
        }),
        "error" => Some(Message::Error { id: id_of(&value), message: text(&value, "message", 300) }),
        "notify" => Some(Message::Notify { message: text(&value, "message", 300) }).filter(|m| matches!(m, Message::Notify { message } if !message.trim().is_empty())),
        _ => None,
    }
}

/// Where a command's keyword is typed: "ghi bug" is `("ghi", "bug")`. Only a keyword followed by
/// a space counts, so typing "ghi" alone still searches as usual.
pub fn split_keyword(query: &str) -> Option<(&str, &str)> {
    let query = query.trim_start();
    let (first, rest) = query.split_once(' ')?;
    (!first.is_empty()).then_some((first, rest))
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = r#"{
        "id": "windows-settings",
        "name": "Windows Settings",
        "description": "Open any page of Settings",
        "author": "Beroris",
        "version": "1.0.0",
        "icon": "icon.png",
        "run": ["powershell", "-NoProfile", "-File", "main.ps1"],
        "commands": [
            { "id": "search", "title": "Search Windows Settings", "keyword": "ws", "placeholder": "Search Settings pages…" },
            { "id": "night", "title": "Toggle Night Light", "mode": "run" }
        ]
    }"#;

    #[test]
    fn a_good_manifest_is_read() {
        let manifest = Manifest::parse(SAMPLE).unwrap();
        assert_eq!(manifest.id, "windows-settings");
        assert_eq!(manifest.commands.len(), 2);
        assert_eq!(manifest.commands[0].mode, Mode::List);
        assert_eq!(manifest.commands[1].mode, Mode::Run);
        assert_eq!(manifest.protocol, 1);
        assert_eq!(manifest.command("search").unwrap().keyword, "ws");
    }

    #[test]
    fn a_byte_order_mark_is_fine() {
        assert!(Manifest::parse(&format!("\u{FEFF}{SAMPLE}")).is_ok(), "Notepad and PowerShell add one");
    }

    #[test]
    fn bad_manifests_say_why() {
        let with = |from: &str, to: &str| Manifest::parse(&SAMPLE.replace(from, to));
        assert!(with("\"windows-settings\"", "\"Windows Settings\"").unwrap_err().contains("id"));
        assert!(with("[\"powershell\", \"-NoProfile\", \"-File\", \"main.ps1\"]", "[]").unwrap_err().contains("run"));
        assert!(with("\"ws\"", "\"w s\"").unwrap_err().contains("keyword"));
        assert!(with("\"night\"", "\"search\"").unwrap_err().contains("Two commands"));
        assert!(with("\"icon.png\"", "\"..\\\\..\\\\secret.png\"").unwrap_err().contains("icon"));
        assert!(with("\"version\"", "\"protocol\": 9, \"version\"").unwrap_err().contains("newer Orbit"));
        assert!(Manifest::parse("not json").unwrap_err().contains(MANIFEST_FILE));
    }

    #[test]
    fn paths_stay_inside_the_folder() {
        assert_eq!(inside_folder("icons/a.png").as_deref(), Some("icons\\a.png"));
        assert_eq!(inside_folder("./icon.png").as_deref(), Some("icon.png"));
        assert_eq!(inside_folder("../x.png"), None);
        assert_eq!(inside_folder("a/../../x.png"), None);
        assert_eq!(inside_folder("C:\\x.png"), None);
        assert_eq!(inside_folder("\\\\server\\x.png"), None);
        assert_eq!(inside_folder(""), None);
    }

    #[test]
    fn requests_are_single_lines() {
        let line = Request::Search { id: 3, command: "search".into(), query: "blue\nlight \"x\"".into(), filter: String::new() }.to_line();
        assert_eq!(line.matches('\n').count(), 1, "{line}");
        assert!(line.ends_with('\n'));
        let back: Value = serde_json::from_str(&line).unwrap();
        assert_eq!(back["type"], "search");
        assert_eq!(back["query"], "blue\nlight \"x\"");
        assert!(back.get("filter").is_none(), "no filter chosen sends none, as before");
        let filtered = Request::Search { id: 4, command: "search".into(), query: String::new(), filter: "repo:o/r".into() }.to_line();
        assert_eq!(serde_json::from_str::<Value>(&filtered).unwrap()["filter"], "repo:o/r");
        let hello = Request::Hello { orbit: "1.1.0".into(), protocol: PROTOCOL }.to_line();
        assert!(hello.contains("\"type\":\"hello\""));
    }

    #[test]
    fn items_and_their_actions_are_read() {
        let line = r#"{"type":"items","id":7,"items":[
            {"id":"bt","title":"Bluetooth","subtitle":"Devices","accessory":"System","icon":"icons/bt.png",
             "actions":[{"title":"Open","type":"open","target":"ms-settings:bluetooth"},
                        {"type":"copy","text":"ms-settings:bluetooth","icon":" Link "},
                        {"title":"Run it","type":"callback","id":"go","shortcut":"Ctrl+R","icon":"rocket"},
                        {"type":"open","target":"javascript:alert(1)"},
                        {"type":"open","target":"file:///C:/x"},
                        {"type":"reveal","path":"relative\\path"},
                        {"type":"teleport"}]},
            {"title":""},
            {"title":"Second\nline", "id": 5}
        ]}"#;
        let Some(Message::Items { id, items, empty, .. }) = parse_message(&line.replace('\n', " ")) else { panic!() };
        assert_eq!(id, 7);
        assert_eq!(empty, None);
        assert_eq!(items.len(), 2, "an item with no title is dropped");
        let first = &items[0];
        assert_eq!(first.icon.as_deref(), Some("icons/bt.png"));
        let kinds: Vec<&ActionKind> = first.actions.iter().map(|a| &a.kind).collect();
        assert_eq!(
            kinds,
            [&ActionKind::Open("ms-settings:bluetooth".into()), &ActionKind::Copy("ms-settings:bluetooth".into()), &ActionKind::Callback("go".into())],
            "blocked schemes, relative paths and unknown types are refused"
        );
        assert_eq!(first.actions[1].title, "Copy", "a missing title gets a plain one");
        assert_eq!(first.actions[2].shortcut.as_deref(), Some("Ctrl+R"));
        assert_eq!(first.actions[1].icon.as_deref(), Some("link"), "a symbol is named in any case");
        assert_eq!(first.actions[2].icon, None, "one Orbit doesn't draw is left to the kind");
        assert_eq!(first.actions[0].icon, None);
        assert_eq!(items[1].title, "Second line", "control characters can't break a row");
        assert_eq!(items[1].id, "5");
    }

    #[test]
    fn meters_are_read_and_kept_in_range() {
        let line = r#"{"type":"items","id":1,"items":[{"title":"Codex","meters":[
            {"label":"5h 32%","value":0.32},{"label":"Week","value":1.7},{"value":-2},
            {"label":"no value"},{"label":"Fourth","value":0.5}]}]}"#;
        let Some(Message::Items { items, .. }) = parse_message(&line.replace('\n', " ")) else { panic!() };
        let meters = &items[0].meters;
        assert_eq!(meters.len(), MAX_METERS, "a meter with no value is dropped, and only the first few are kept");
        assert_eq!(meters[0], Meter { label: "5h 32%".into(), value: 0.32 });
        assert_eq!(meters[1].value, 1.0, "full is as far as a meter goes");
        assert_eq!(meters[2], Meter { label: String::new(), value: 0.0 });
        let Some(Message::Items { items, .. }) = parse_message(r#"{"type":"items","id":2,"items":[{"title":"Plain"}]}"#) else { panic!() };
        assert!(items[0].meters.is_empty(), "most items have none");
    }

    #[test]
    fn tags_sections_and_filters_are_read() {
        let line = r#"{"type":"items","id":3,"items":[
            {"title":"Fix the build","section":"Open","color":"green","tags":[
                {"text":"Conflicts","color":"red"},{"text":"Draft"},{"text":""},{"text":"Approved","colour":"GREEN"},{"text":"Fourth","color":"blue"}]},
            {"title":"Old one","section":"Recently closed","tags":[{"text":"Merged","color":"purple"}]}],
            "filters":[{"id":"","title":"All pull requests"},{"id":"repo:o/r","title":"o/r","subtitle":"2"},{"title":""}]}"#;
        let Some(Message::Items { items, filters, .. }) = parse_message(&line.replace('\n', " ")) else { panic!() };
        let tags = &items[0].tags;
        assert_eq!(tags.len(), MAX_TAGS, "an empty tag is left out, and only the first few are kept");
        assert_eq!(tags[0], Tag::new("Conflicts", Tone::Red));
        assert_eq!(tags[1], Tag::new("Draft", Tone::Grey), "no colour is grey");
        assert_eq!(tags[2].tone, Tone::Green, "either spelling, any case");
        assert_eq!(items[0].tint, Some(Tone::Green));
        assert_eq!(items[1].tint, None);
        assert_eq!((items[0].section.as_str(), items[1].section.as_str()), ("Open", "Recently closed"));
        assert_eq!(filters.len(), 2, "a filter with no title is left out");
        assert_eq!(filters[1], FilterOption { id: "repo:o/r".into(), title: "o/r".into(), subtitle: "2".into() });
        let Some(Message::Items { items, filters, .. }) = parse_message(r#"{"type":"items","id":2,"items":[{"title":"Plain"}]}"#) else { panic!() };
        assert!(items[0].tags.is_empty() && items[0].section.is_empty() && filters.is_empty(), "all of it is optional");
    }

    #[test]
    fn files_can_be_opened_and_shown() {
        let line = r#"{"type":"items","id":1,"items":[{"title":"Report","actions":[{"type":"open","target":"C:\\Users\\a\\report.pdf"},{"type":"reveal","path":"C:\\Users\\a\\report.pdf"},{"type":"open","target":"\\\\nas\\share\\x.txt"}]}]}"#;
        let Some(Message::Items { items, .. }) = parse_message(line) else { panic!() };
        assert_eq!(items[0].actions.len(), 3);
        assert!(matches!(items[0].actions[1].kind, ActionKind::Reveal(_)));
    }

    #[test]
    fn long_and_unknown_lines_are_handled() {
        assert_eq!(parse_message("hello from a debug print"), None);
        assert_eq!(parse_message(r#"{"type":"from-the-future"}"#), None);
        let many: Vec<String> = (0..500).map(|n| format!(r#"{{"title":"{n}"}}"#)).collect();
        let line = format!(r#"{{"type":"items","id":1,"items":[{}]}}"#, many.join(","));
        let Some(Message::Items { items, .. }) = parse_message(&line) else { panic!() };
        assert_eq!(items.len(), MAX_ITEMS);
        let title = "x".repeat(1000);
        let Some(Message::Items { items, .. }) = parse_message(&format!(r#"{{"type":"items","items":[{{"title":"{title}"}}]}}"#)) else { panic!() };
        assert_eq!(items[0].title.chars().count(), MAX_TITLE);
        assert!(items[0].title.ends_with('\u{2026}'));
    }

    #[test]
    fn other_messages_are_read() {
        assert_eq!(parse_message(r#"{"type":"done","id":4,"close":true}"#), Some(Message::Done { id: 4, message: None, close: true }));
        assert_eq!(parse_message(r#"{"type":"error","id":4,"message":"No network"}"#), Some(Message::Error { id: 4, message: "No network".into() }));
        assert_eq!(parse_message(r#"{"type":"notify","message":"Copied"}"#), Some(Message::Notify { message: "Copied".into() }));
        assert_eq!(parse_message(r#"{"type":"notify","message":""}"#), None);
    }

    #[test]
    fn keywords_need_a_space_after_them() {
        assert_eq!(split_keyword("f report"), Some(("f", "report")));
        assert_eq!(split_keyword("f "), Some(("f", "")));
        assert_eq!(split_keyword("f"), None);
        assert_eq!(split_keyword(" ws  blue"), Some(("ws", " blue")));
        assert!(valid_keyword("f"));
        assert!(valid_keyword(""));
        assert!(!valid_keyword("a b"));
    }
}
