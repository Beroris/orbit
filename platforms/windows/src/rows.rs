//! What the launcher list holds, and what can be done with each row.

use crate::gfx::Rgb;
use crate::integrations::{Field, Tool};
use crate::util::format_bytes;
use crate::worker::{ClipPreview, PreviewBody};
use orbit_core::extension::{ActionKind, ItemAction};
use orbit_core::{Action, Builtin};
use orbit_data::ClipKind;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RowIcon {
    App(u64),
    Glyph(char),
    /// A clipboard image, by its clip's id.
    Image(i64),
    /// An emoji, by its place in the table.
    Emoji(u16),
    /// A quicklink's website icon, by its key in `favicon`. Drawn as a link until it's there.
    Favicon(u64),
    /// A file's own icon or an extension's picture, by its key in `files::icon_hash`, with the
    /// symbol to draw until it's there.
    File(u64, char),
}

/// Where a clipboard image's thumbnail sits in the icon cache, away from any app's hash.
pub fn thumb_hash(clip: i64) -> u64 {
    0xC11B_0000_0000_0000 | (clip as u64 & 0xFFFF_FFFF_FFFF)
}

pub const GLYPH_SEARCH: char = '\u{E721}';
pub const GLYPH_NOTE: char = '\u{E70B}';
pub const GLYPH_SNIPPET: char = '\u{E943}';
pub const GLYPH_CLIPBOARD: char = '\u{E77F}';
pub const GLYPH_CALC: char = '\u{E8EF}';
pub const GLYPH_CURRENCY: char = '\u{E825}';
pub const GLYPH_WEATHER: char = '\u{E706}';
pub const GLYPH_SPEED: char = '\u{EC4A}';
pub const GLYPH_LINK: char = '\u{E71B}';
pub const GLYPH_WEB: char = '\u{E774}';
pub const GLYPH_COPY: char = '\u{E8C8}';
pub const GLYPH_PIN: char = '\u{E718}';
pub const GLYPH_DELETE: char = '\u{E74D}';
pub const GLYPH_EDIT: char = '\u{E70F}';
pub const GLYPH_FOLDER: char = '\u{E8B7}';
pub const GLYPH_HIDE: char = '\u{ED1A}';
pub const GLYPH_SHIELD: char = '\u{E83D}';
pub const GLYPH_TAG: char = '\u{E8EC}';
pub const GLYPH_PLAY: char = '\u{E768}';
pub const GLYPH_ADD: char = '\u{E710}';
pub const GLYPH_INFO: char = '\u{E946}';
pub const GLYPH_WARNING: char = '\u{E7BA}';
pub const GLYPH_BACK: char = '\u{E72B}';
pub const GLYPH_CHECK: char = '\u{E73E}';
pub const GLYPH_IMAGE: char = '\u{EB9F}';
pub const GLYPH_COMMAND: char = '\u{E756}';
pub const GLYPH_KEYBOARD: char = '\u{E765}';
pub const GLYPH_STAR: char = '\u{E734}';
pub const GLYPH_STAR_FILLED: char = '\u{E735}';
pub const GLYPH_UP: char = '\u{E70E}';
pub const GLYPH_DOWN: char = '\u{E70D}';
pub const GLYPH_KEY: char = '\u{E8D7}';
pub const GLYPH_REFRESH: char = '\u{E72C}';
pub const GLYPH_EMOJI: char = '\u{E76E}';
pub const GLYPH_SETTINGS: char = '\u{E713}';
pub const GLYPH_POWER: char = '\u{E7E8}';
pub const GLYPH_DIAGNOSTICS: char = '\u{E9D9}';
pub const GLYPH_HELP: char = '\u{E897}';
pub const GLYPH_END_TASK: char = '\u{E711}';
pub const GLYPH_FILE: char = '\u{E8A5}';
pub const GLYPH_EXTENSION: char = '\u{EA86}';
pub const GLYPH_IMPORT: char = '\u{E8B5}';
/// A download arrow, for a new version of Orbit.
pub const GLYPH_UPDATE: char = '\u{E896}';
// Symbols the icon font has no good match for. Orbit draws these itself (see `symbols.rs`), so the
// code points, from the font's private range, are only names.
pub const GLYPH_WIN_LEFT: char = '\u{F101}';
pub const GLYPH_WIN_RIGHT: char = '\u{F102}';
pub const GLYPH_WIN_TOP: char = '\u{F103}';
pub const GLYPH_WIN_BOTTOM: char = '\u{F104}';
pub const GLYPH_WIN_MAX: char = '\u{F105}';
pub const GLYPH_WIN_ALMOST: char = '\u{F106}';
pub const GLYPH_WIN_CENTER: char = '\u{F107}';
pub const GLYPH_WIN_DISPLAY: char = '\u{F108}';
pub const GLYPH_THEME: char = '\u{F109}';
pub const GLYPH_BLUETOOTH: char = '\u{F10A}';
pub const GLYPH_WIFI: char = '\u{F10B}';
pub const GLYPH_SOUND: char = '\u{F10C}';
pub const GLYPH_TIMER: char = '\u{F10D}';
pub const GLYPH_STOPWATCH: char = '\u{F10E}';
pub const GLYPH_PICKER: char = '\u{F10F}';
/// A branch, for GitHub.
pub const GLYPH_GITHUB: char = '\u{F110}';
/// A sparkle, for AI Usage.
pub const GLYPH_AI: char = '\u{F111}';

/// The symbol of an extension that comes with Orbit.
pub fn extension_glyph(ext: orbit_core::builtin_ext::BuiltinExt) -> char {
    use orbit_core::builtin_ext::BuiltinExt;
    match ext {
        BuiltinExt::Files => GLYPH_FILE,
        BuiltinExt::OnePassword => GLYPH_KEY,
        BuiltinExt::EnteAuth => GLYPH_SHIELD,
        BuiltinExt::Weather => GLYPH_WEATHER,
        BuiltinExt::SpeedTest => GLYPH_SPEED,
        BuiltinExt::GitHub => GLYPH_GITHUB,
        BuiltinExt::AiUsage => GLYPH_AI,
    }
}

/// The two colours of a command's tile, light at the top left and deep at the bottom right. Colour
/// tells them apart at a glance, as app icons do, and the second shifts a little in hue so the tile
/// has some life in it rather than one colour getting darker.
pub fn tile_colors(glyph: char) -> (Rgb, Rgb) {
    const BLUE: (Rgb, Rgb) = (Rgb(96, 165, 250), Rgb(37, 84, 220));
    const SKY: (Rgb, Rgb) = (Rgb(56, 196, 248), Rgb(14, 104, 206));
    const WEATHER: (Rgb, Rgb) = (Rgb(125, 211, 252), Rgb(37, 99, 235));
    const YELLOW: (Rgb, Rgb) = (Rgb(253, 214, 64), Rgb(212, 150, 8));
    const VIOLET: (Rgb, Rgb) = (Rgb(176, 148, 252), Rgb(118, 50, 228));
    const PURPLE: (Rgb, Rgb) = (Rgb(192, 132, 252), Rgb(126, 34, 206));
    const INDIGO: (Rgb, Rgb) = (Rgb(139, 150, 250), Rgb(72, 60, 214));
    const TEAL: (Rgb, Rgb) = (Rgb(52, 218, 196), Rgb(13, 130, 140));
    const GREEN: (Rgb, Rgb) = (Rgb(86, 222, 136), Rgb(22, 140, 78));
    const EMERALD: (Rgb, Rgb) = (Rgb(64, 216, 160), Rgb(5, 122, 100));
    const ROSE: (Rgb, Rgb) = (Rgb(252, 122, 150), Rgb(208, 28, 84));
    const RED: (Rgb, Rgb) = (Rgb(250, 120, 120), Rgb(196, 30, 46));
    const FUCHSIA: (Rgb, Rgb) = (Rgb(240, 134, 230), Rgb(178, 36, 190));
    // Grey, as settings usually are, though a cool grey rather than a dull one.
    const SLATE: (Rgb, Rgb) = (Rgb(166, 178, 198), Rgb(68, 82, 106));
    const GRAPHITE: (Rgb, Rgb) = (Rgb(122, 130, 146), Rgb(28, 33, 42));
    const ORANGE: (Rgb, Rgb) = (Rgb(253, 170, 96), Rgb(222, 82, 24));
    match glyph {
        GLYPH_NOTE => YELLOW,
        GLYPH_SNIPPET | GLYPH_IMAGE => VIOLET,
        GLYPH_SPEED => PURPLE,
        GLYPH_CLIPBOARD => TEAL,
        GLYPH_CALC | GLYPH_COMMAND | GLYPH_KEYBOARD => INDIGO,
        GLYPH_EMOJI | GLYPH_PICKER => FUCHSIA,
        GLYPH_WIN_LEFT | GLYPH_WIN_RIGHT | GLYPH_WIN_TOP | GLYPH_WIN_BOTTOM | GLYPH_WIN_MAX | GLYPH_WIN_ALMOST | GLYPH_WIN_CENTER | GLYPH_WIN_DISPLAY => BLUE,
        GLYPH_THEME => INDIGO,
        GLYPH_BLUETOOTH => BLUE,
        GLYPH_WIFI => SKY,
        GLYPH_SOUND | GLYPH_TIMER => ROSE,
        GLYPH_STOPWATCH => TEAL,
        GLYPH_DIAGNOSTICS => TEAL,
        GLYPH_HELP => BLUE,
        GLYPH_CURRENCY => EMERALD,
        GLYPH_WEATHER => WEATHER,
        GLYPH_GITHUB => GRAPHITE,
        GLYPH_AI => ORANGE,
        GLYPH_LINK | GLYPH_FOLDER | GLYPH_EDIT | GLYPH_REFRESH => BLUE,
        GLYPH_WEB | GLYPH_SEARCH | GLYPH_KEY => SKY,
        GLYPH_PIN | GLYPH_TAG => ROSE,
        GLYPH_DELETE => RED,
        GLYPH_SHIELD | GLYPH_PLAY | GLYPH_ADD | GLYPH_CHECK | GLYPH_UPDATE => GREEN,
        GLYPH_WARNING | GLYPH_STAR | GLYPH_STAR_FILLED => YELLOW,
        '\u{E7E8}' => RED, // power
        '\u{E72E}' | '\u{E708}' => INDIGO, // lock, sleep
        _ => SLATE,
    }
}


/// How an answer is shown when it's the only result: the question and the answer side by side,
/// with what each is in, and a note under them.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct AnswerCard {
    pub question: String,
    pub question_label: String,
    pub answer: String,
    pub answer_label: String,
    pub note: String,
}

/// Row identity, which decides the available actions.
#[derive(Clone, Debug, PartialEq)]
pub enum RowSource {
    Plain,
    Catalog { item: u32, key: String },
    /// A section title. Never selected.
    Header,
    Note { id: i64, pinned: bool },
    Snippet { id: i64 },
    Clip { id: i64, kind: ClipKind, pinned: bool, formatted: bool },
    /// An item in a password manager or authenticator. Holds no secret.
    Secret { tool: Tool, id: String, url: Option<String> },
    Emoji { at: u16, pinned: bool },
    /// A worked-out answer: a sum, a unit or a currency. Enter copies the bare number, and these
    /// are the other ways to take it.
    /// `other` is the answer the way Enter doesn't copy it: formatted, as "£1,234.50", when Enter
    /// copies the bare number, and the other way round. It's `None` when they're the same.
    Answer { other: Option<String>, other_formatted: bool, reverse: Option<String>, card: Box<AnswerCard> },
    /// The speed test's one row. `summary` and `url` are there once a test has finished.
    SpeedTest { summary: Option<String>, url: Option<String> },
    /// The timer or the stopwatch, and whether it has been started and is running, for its menu.
    Clock { stopwatch: bool, running: bool, started: bool },
    /// A colour picked from the screen.
    Colour(orbit_core::colour::Colour),
    /// A file or folder Everything found.
    File { path: String, is_folder: bool },
    /// An item in an extension's list, with everything it offers to do, and the meters it shows.
    Extension { ext: String, command: String, item: String, actions: Vec<ItemAction>, meters: Vec<orbit_core::extension::Meter> },
}

#[derive(Clone, Debug, PartialEq)]
pub struct Row {
    pub title: String,
    pub subtitle: String,
    pub kind: &'static str,
    pub icon: RowIcon,
    pub accent: bool,
    pub source: RowSource,
    /// What Enter does. `None` makes the row informational.
    pub primary: Option<Op>,
    /// A key combination to show as key caps on the right, such as `Hyper+C`.
    pub keys: Option<String>,
    /// Text on the right in place of `kind`, for rows whose kind isn't known in advance.
    pub accessory: Option<String>,
}

impl Row {
    pub fn info(title: impl Into<String>, subtitle: impl Into<String>, glyph: char) -> Row {
        Row {
            title: title.into(),
            subtitle: subtitle.into(),
            kind: "",
            icon: RowIcon::Glyph(glyph),
            accent: false,
            source: RowSource::Plain,
            primary: None,
            keys: None,
            accessory: None,
        }
    }

    pub fn op(title: impl Into<String>, subtitle: impl Into<String>, glyph: char, op: Op) -> Row {
        Row { primary: Some(op), ..Row::info(title, subtitle, glyph) }
    }

    pub fn header(title: impl Into<String>) -> Row {
        Row { source: RowSource::Header, ..Row::info(title, "", GLYPH_INFO) }
    }

    pub fn is_header(&self) -> bool {
        self.source == RowSource::Header
    }

    pub fn with_keys(mut self, keys: impl Into<String>) -> Row {
        self.keys = Some(keys.into());
        self
    }
}

/// Everything a row can do. Rows are rebuilt per keystroke, so these stay cheap to clone.
#[derive(Clone, Debug, PartialEq)]
pub enum Op {
    /// Run a catalog item: records usage, then its action.
    RunItem { item: u32, key: String },
    AskConfirm(Builtin),
    Confirmed(Builtin),
    Back,
    Copy(String),
    OpenUrl(String),
    /// A quicklink's address or path, filled in, opened with the app it names, if any.
    OpenLink { target: String, open_with: String },
    /// The app a quicklink form opens its link with. Empty for the usual app.
    FormOpenWith(String),
    AppAsAdmin(String),
    /// Start another copy, even if the app is already running.
    AppNewWindow(String),
    AppFolder(String),
    /// Ends every process of an app, as Task Manager does.
    AppEndTask(String),
    TogglePin(String),
    /// Move a favourite up (negative) or down its list.
    MoveFavorite(String, i32),
    HideItem(String),
    AliasPrompt { key: String, title: String },
    NoteOpen(i64),
    NoteCopy(i64),
    NotePin(i64, bool),
    NoteTrash(i64),
    NoteUndoTrash,
    NewNote,
    SnippetInsert(i64),
    SnippetCopy(i64),
    SnippetEdit(i64),
    SnippetDelete(i64),
    NewSnippet,
    ClipPaste(i64),
    /// Paste the text alone, without the formatting it was copied with.
    ClipPastePlain(i64),
    /// Open a link, picture or file from history in the app Windows uses for it.
    ClipOpen(i64),
    ClipCopy(i64),
    ClipPin(i64, bool),
    ClipDelete(i64),
    EnableClipboard,
    SaveAlias { key: String },
    /// Fill in a quicklink's blanks, starting from what was typed after its alias.
    LinkArgs { id: String, values: Vec<String> },
    /// Go to the box that still needs filling in.
    LinkArgsNext,
    /// Insert the snippet with clipboard content the user has just reviewed.
    InsertPending,
    /// Start listening for a new hotkey for this item.
    RecordHotkey { action: Action, title: String },
    ClearHotkey(Action),
    /// Take a key from whatever it ran before, after the user has been told.
    ConfirmHotkey,
    /// Don't take that key after all. Listen again.
    RecordAgain,
    /// Show what a password manager or authenticator holds.
    OpenVault(Tool),
    /// Ask it again, instead of using the list from a few minutes ago.
    RefreshVault(Tool),
    /// Fetch one value and put it on the clipboard.
    CopySecret { tool: Tool, id: String, field: Field },
    /// A command from a menu, as if it had been searched for and run.
    RunBuiltin(Builtin),
    EmojiPaste(u16),
    EmojiCopy(u16),
    EmojiPin(u16, bool),
    /// A new quicklink, with this address filled in.
    NewLinkFor(String),
    EditLink(String),
    DeleteLink(String),
    EditCommand(String),
    DeleteCommand(String),
    /// Open the weather, for this place, or with nothing typed the one from last time.
    Weather(String),
    /// In the weather: look up what's typed, or with nothing typed show the same place again.
    WeatherLookup(String),
    /// Type this text into the window the launcher was opened over.
    PasteText(String),
    /// Search for this instead, as if it had been typed.
    SetQuery(String),
    /// Accept Ookla's terms for Speedtest CLI, then install it if need be and test.
    SpeedTestAccept,
    SpeedTestInstall,
    SpeedTestStart,
    SpeedTestStop,
    TimerStart(u64),
    TimerPause,
    TimerResume,
    TimerAdd(u64),
    TimerCancel,
    StopwatchStart,
    StopwatchPause,
    StopwatchReset,
    PickColour,
    /// A file or folder, in the app Windows uses for it.
    OpenFile(String),
    /// A file or folder, selected in File Explorer.
    RevealFile(String),
    CopyPath(String),
    /// The file itself, to paste into a folder or an email.
    CopyFile(String),
    /// Windows' "Open with" box for a file.
    OpenFileWith(String),
    FileAsAdmin(String),
    /// A terminal in this folder.
    TerminalHere(String),
    EverythingInstall,
    EverythingStart,
    /// Search again, after Everything had a problem.
    EverythingRetry,
    /// Open an extension's command: its list, or run it.
    /// Do one of an extension item's actions.
    ExtAction { ext: String, command: String, item: String, action: ItemAction },
    /// Settings, on the Extensions page.
    OpenExtensionsSettings,
}

/// Short labels for the footer: what Enter and Ctrl+Enter do on this row.
pub fn primary_label(op: &Op) -> &'static str {
    match op {
        Op::RunItem { .. } => "Open",
        Op::AskConfirm(_) => "Continue",
        Op::Confirmed(_) => "Confirm",
        Op::Back => "Back",
        Op::Copy(_) => "Copy",
        Op::OpenUrl(_) | Op::OpenLink { .. } => "Open",
        Op::FormOpenWith(_) => "Choose",
        Op::NoteOpen(_) => "Open note",
        Op::SnippetInsert(_) => "Insert",
        Op::ClipPaste(_) => "Paste",
        Op::ClipPastePlain(_) => "Paste as plain text",
        Op::ClipOpen(_) => "Open",
        Op::SaveAlias { .. } => "Save",
        Op::InsertPending => "Insert",
        Op::ConfirmHotkey => "Replace",
        Op::RecordAgain => "Choose another",
        Op::ClearHotkey(_) => "Remove",
        Op::OpenVault(_) => "Open",
        Op::RefreshVault(_) => "Try again",
        Op::CopySecret { field: Field::Password, .. } => "Copy password",
        Op::CopySecret { field: Field::Username, .. } => "Copy username",
        Op::CopySecret { field: Field::OneTimeCode, .. } => "Copy code",
        Op::EmojiPaste(_) => "Paste",
        Op::EmojiCopy(_) => "Copy",
        Op::NewLinkFor(_) => "Save",
        Op::LinkArgs { .. } | Op::LinkArgsNext => "Fill in",
        Op::Weather(_) => "Show weather",
        Op::WeatherLookup(name) if name.trim().is_empty() => "Refresh",
        Op::WeatherLookup(_) => "Look up",
        Op::PasteText(_) => "Paste",
        Op::SetQuery(_) => "Convert",
        Op::SpeedTestAccept => "Accept and start",
        Op::SpeedTestInstall => "Install",
        Op::SpeedTestStart => "Start test",
        Op::SpeedTestStop => "Stop",
        Op::TimerStart(_) | Op::StopwatchStart => "Start",
        Op::TimerPause | Op::StopwatchPause => "Pause",
        Op::TimerResume => "Resume",
        Op::TimerAdd(_) => "Add time",
        Op::TimerCancel => "Cancel",
        Op::StopwatchReset => "Reset",
        Op::PickColour => "Pick",
        Op::OpenFile(_) => "Open",
        Op::RevealFile(_) => "Show in folder",
        Op::CopyPath(_) => "Copy path",
        Op::CopyFile(_) => "Copy file",
        Op::EverythingInstall => "Install",
        Op::EverythingStart => "Start",
        Op::EverythingRetry => "Try again",
        Op::OpenExtensionsSettings => "Open Settings",
        Op::ExtAction { action, .. } => match action.kind {
            ActionKind::Open(_) => "Open",
            ActionKind::Reveal(_) => "Show in folder",
            ActionKind::Copy(_) => "Copy",
            ActionKind::Paste(_) => "Paste",
            ActionKind::Callback(_) => "Run",
            ActionKind::SetQuery(_) => "Search",
        },
        _ => "Run",
    }
}

/// Whether a file is a program, so it can be offered as administrator.
pub fn is_program(path: &str) -> bool {
    let lower = path.to_ascii_lowercase();
    [".exe", ".lnk", ".bat", ".cmd", ".msi", ".ps1"].iter().any(|ext| lower.ends_with(ext))
}

/// The Ctrl+Enter action, if the row has one.
pub fn secondary_for(row: &Row, app_item_id: Option<&str>) -> Option<(&'static str, Op)> {
    match &row.source {
        RowSource::Catalog { .. } => app_item_id.map(|id| ("New window", Op::AppNewWindow(id.to_string()))),
        RowSource::Note { id, .. } => Some(("Copy", Op::NoteCopy(*id))),
        RowSource::Snippet { id } => Some(("Copy", Op::SnippetCopy(*id))),
        RowSource::Clip { id, .. } => Some(("Copy", Op::ClipCopy(*id))),
        RowSource::Emoji { at, .. } => Some(("Copy", Op::EmojiCopy(*at))),
        RowSource::Answer { .. } => match &row.primary {
            Some(Op::Copy(answer)) => Some(("Paste", Op::PasteText(answer.clone()))),
            _ => None,
        },
        RowSource::SpeedTest { summary: Some(summary), .. } => Some(("Copy results", Op::Copy(summary.clone()))),
        RowSource::SpeedTest { .. } => None,
        RowSource::Clock { .. } => None,
        RowSource::Colour(colour) => Some(("Copy RGB", Op::Copy(colour.rgb()))),
        RowSource::File { path, .. } => Some(("Show in folder", Op::RevealFile(path.clone()))),
        RowSource::Extension { ext, command, item, actions, .. } => actions.get(1).map(|action| {
            let op = Op::ExtAction { ext: ext.clone(), command: command.clone(), item: item.clone(), action: action.clone() };
            (primary_label(&op), op)
        }),
        RowSource::Plain | RowSource::Header | RowSource::Secret { .. } => None,
    }
}

/// What the menu needs to know to offer a hotkey: what it would run, and what it has now.
pub struct HotkeyChoice {
    pub action: Action,
    pub current: Option<String>,
}

fn hotkey_rows(out: &mut Vec<Row>, row: &Row, choice: Option<HotkeyChoice>) {
    let Some(HotkeyChoice { action, current }) = choice else { return };
    let record = Op::RecordHotkey { action: action.clone(), title: row.title.clone() };
    match current {
        Some(label) => {
            out.push(Row::op("Change hotkey…", "", GLYPH_KEYBOARD, record).with_keys(label));
            out.push(Row::op("Remove hotkey", "", GLYPH_DELETE, Op::ClearHotkey(action)));
        }
        None => out.push(Row::op("Set hotkey…", "", GLYPH_KEYBOARD, record)),
    }
}

/// Entries for the Ctrl+K action menu.
pub fn menu_for(row: &Row, app_item_id: Option<&str>, is_pinned: bool, hotkey: Option<HotkeyChoice>) -> Vec<Row> {
    let mut out = Vec::with_capacity(8);
    if let Some(primary) = &row.primary {
        out.push(Row::op(primary_label(primary), "", GLYPH_PLAY, primary.clone()).with_keys("Enter"));
    }
    match &row.source {
        RowSource::Plain | RowSource::Header => {}
        RowSource::Catalog { key, .. } => {
            if let Some(id) = app_item_id {
                out.push(Row::op("Open a new window", "", GLYPH_ADD, Op::AppNewWindow(id.to_string())).with_keys("Ctrl+Enter"));
                out.push(Row::op("Run as administrator", "", GLYPH_SHIELD, Op::AppAsAdmin(id.to_string())).with_keys("Ctrl+Shift+Enter"));
                out.push(Row::op("Open file location", "", GLYPH_FOLDER, Op::AppFolder(id.to_string())).with_keys("Ctrl+Shift+E"));
                // No key for this one, so it can only be chosen on purpose.
                if row.kind == "Switch to" {
                    out.push(Row::op("End task", "Closes it without asking about unsaved work", GLYPH_END_TASK, Op::AppEndTask(id.to_string())));
                }
            }
            hotkey_rows(&mut out, row, hotkey);
            if is_pinned {
                out.push(Row::op("Remove from favourites", "", GLYPH_STAR_FILLED, Op::TogglePin(key.clone())).with_keys("Ctrl+Shift+F"));
                out.push(Row::op("Move up in favourites", "", GLYPH_UP, Op::MoveFavorite(key.clone(), -1)).with_keys("Ctrl+Shift+Up"));
                out.push(Row::op("Move down in favourites", "", GLYPH_DOWN, Op::MoveFavorite(key.clone(), 1)).with_keys("Ctrl+Shift+Down"));
            } else {
                out.push(Row::op("Add to favourites", "", GLYPH_STAR, Op::TogglePin(key.clone())).with_keys("Ctrl+Shift+F"));
            }
            out.push(Row::op("Set alias…", "", GLYPH_TAG, Op::AliasPrompt { key: key.clone(), title: row.title.clone() }).with_keys("Ctrl+Shift+A"));
            out.push(Row::op("Hide from search", "", GLYPH_HIDE, Op::HideItem(key.clone())).with_keys("Ctrl+Shift+H"));
        }
        RowSource::Note { id, pinned } => {
            out.push(Row::op("Copy contents", "", GLYPH_COPY, Op::NoteCopy(*id)).with_keys("Ctrl+Enter"));
            hotkey_rows(&mut out, row, hotkey);
            let label = if *pinned { "Unpin note" } else { "Pin note" };
            out.push(Row::op(label, "", GLYPH_PIN, Op::NotePin(*id, !*pinned)).with_keys("Ctrl+Shift+P"));
            out.push(Row::op("Move to trash", "Kept for 30 days", GLYPH_DELETE, Op::NoteTrash(*id)).with_keys("Ctrl+Shift+Del"));
        }
        RowSource::Snippet { id } => {
            out.push(Row::op("Copy text", "", GLYPH_COPY, Op::SnippetCopy(*id)).with_keys("Ctrl+Enter"));
            hotkey_rows(&mut out, row, hotkey);
            out.push(Row::op("Edit snippet", "", GLYPH_EDIT, Op::SnippetEdit(*id)).with_keys("Ctrl+E"));
            out.push(Row::op("Delete snippet", "", GLYPH_DELETE, Op::SnippetDelete(*id)).with_keys("Ctrl+Shift+Del"));
        }
        RowSource::Clip { id, pinned, kind, formatted } => {
            match kind {
                ClipKind::Url => out.push(Row::op("Open in browser", "", GLYPH_WEB, Op::ClipOpen(*id)).with_keys("Ctrl+O")),
                ClipKind::Image => out.push(Row::op("Open picture", "", GLYPH_IMAGE, Op::ClipOpen(*id)).with_keys("Ctrl+O")),
                ClipKind::Files => out.push(Row::op("Open", "", GLYPH_FOLDER, Op::ClipOpen(*id)).with_keys("Ctrl+O")),
                ClipKind::Text => {}
            }
            if *formatted {
                out.push(Row::op("Paste as plain text", "", GLYPH_PLAY, Op::ClipPastePlain(*id)).with_keys("Ctrl+Shift+Enter"));
            }
            out.push(Row::op("Copy", "", GLYPH_COPY, Op::ClipCopy(*id)).with_keys("Ctrl+Enter"));
            let label = if *pinned { "Unpin" } else { "Pin" };
            out.push(Row::op(label, "", GLYPH_PIN, Op::ClipPin(*id, !*pinned)).with_keys("Ctrl+Shift+P"));
            out.push(Row::op("Delete from history", "", GLYPH_DELETE, Op::ClipDelete(*id)).with_keys("Ctrl+Shift+Del"));
        }
        RowSource::Secret { tool, id, url } => {
            if *tool == Tool::OnePassword {
                let copy = |field| Op::CopySecret { tool: *tool, id: id.clone(), field };
                out.push(Row::op("Copy username", "", GLYPH_COPY, copy(Field::Username)));
                out.push(Row::op("Copy one-time code", "", GLYPH_COPY, copy(Field::OneTimeCode)));
                if let Some(url) = url {
                    out.push(Row::op("Open website", url.clone(), GLYPH_WEB, Op::OpenUrl(url.clone())));
                }
            }
            out.push(Row::op("Refresh the list", "", GLYPH_REFRESH, Op::RefreshVault(*tool)).with_keys("Ctrl+R"));
        }
        RowSource::Emoji { at, pinned } => {
            out.push(Row::op("Copy", "", GLYPH_COPY, Op::EmojiCopy(*at)).with_keys("Ctrl+Enter"));
            let label = if *pinned { "Unpin" } else { "Pin to the top" };
            out.push(Row::op(label, "", GLYPH_PIN, Op::EmojiPin(*at, !*pinned)).with_keys("Ctrl+Shift+P"));
        }
        RowSource::Answer { other, other_formatted, reverse, .. } => {
            if let Some(Op::Copy(answer)) = &row.primary {
                if let Some(first) = out.first_mut() {
                    first.title = "Copy answer".into();
                    first.subtitle = answer.clone();
                }
                out.push(Row::op("Paste answer", "", GLYPH_CLIPBOARD, Op::PasteText(answer.clone())).with_keys("Ctrl+Enter"));
            }
            if let Some(other) = other {
                let label = if *other_formatted { "Copy with formatting" } else { "Copy without formatting" };
                out.push(Row::op(label, other.clone(), GLYPH_COPY, Op::Copy(other.clone())).with_keys("Ctrl+Shift+Enter"));
            }
            if let Some(reverse) = reverse {
                out.push(Row::op("Convert the other way", "", GLYPH_REFRESH, Op::SetQuery(reverse.clone())).with_keys("Ctrl+S"));
            }
        }
        RowSource::SpeedTest { summary, url } => {
            if let Some(summary) = summary {
                out.push(Row::op("Copy results", "", GLYPH_COPY, Op::Copy(summary.clone())).with_keys("Ctrl+Enter"));
            }
            if let Some(url) = url {
                out.push(Row::op("Open result on Speedtest.net", "", GLYPH_WEB, Op::OpenUrl(url.clone())).with_keys("Ctrl+O"));
            }
            out.push(Row::op("Ookla\u{2019}s End User License Agreement", "", GLYPH_WEB, Op::OpenUrl(crate::speedtest::EULA.into())));
            out.push(Row::op("Ookla\u{2019}s Terms of Use", "", GLYPH_WEB, Op::OpenUrl(crate::speedtest::TERMS.into())));
            out.push(Row::op("Ookla\u{2019}s Privacy Policy", "", GLYPH_WEB, Op::OpenUrl(crate::speedtest::PRIVACY.into())));
        }
        RowSource::Clock { stopwatch: false, started, .. } => {
            if *started {
                out.push(Row::op("Add 1 minute", "", GLYPH_ADD, Op::TimerAdd(60)));
                out.push(Row::op("Add 5 minutes", "", GLYPH_ADD, Op::TimerAdd(300)));
                out.push(Row::op("Cancel timer", "", GLYPH_DELETE, Op::TimerCancel).with_keys("Ctrl+Shift+Del"));
            }
        }
        RowSource::Clock { stopwatch: true, started, .. } => {
            if *started {
                out.push(Row::op("Reset", "", GLYPH_REFRESH, Op::StopwatchReset).with_keys("Ctrl+Shift+Del"));
            }
        }
        RowSource::Colour(colour) => {
            out.push(Row::op("Copy RGB", colour.rgb(), GLYPH_COPY, Op::Copy(colour.rgb())).with_keys("Ctrl+Enter"));
            out.push(Row::op("Copy HSL", colour.hsl(), GLYPH_COPY, Op::Copy(colour.hsl())).with_keys("Ctrl+Shift+Enter"));
            out.push(Row::op("Pick another colour", "", GLYPH_PICKER, Op::PickColour));
        }
        RowSource::File { path, is_folder } => {
            out.push(Row::op("Show in folder", "", GLYPH_FOLDER, Op::RevealFile(path.clone())).with_keys("Ctrl+Enter"));
            if is_program(path) {
                out.push(Row::op("Run as administrator", "", GLYPH_SHIELD, Op::FileAsAdmin(path.clone())).with_keys("Ctrl+Shift+Enter"));
            }
            if !*is_folder {
                out.push(Row::op("Open with\u{2026}", "", GLYPH_PLAY, Op::OpenFileWith(path.clone())).with_keys("Ctrl+O"));
            }
            out.push(Row::op("Copy path", path.clone(), GLYPH_COPY, Op::CopyPath(path.clone())).with_keys("Ctrl+Shift+C"));
            out.push(Row::op(if *is_folder { "Copy folder" } else { "Copy file" }, "To paste into a folder or an email", GLYPH_COPY, Op::CopyFile(path.clone())).with_keys("Ctrl+Shift+F"));
            if *is_folder {
                out.push(Row::op("Open in Terminal", "", GLYPH_COMMAND, Op::TerminalHere(path.clone())).with_keys("Ctrl+Shift+T"));
            }
        }
        RowSource::Extension { ext, command, item, actions, .. } => {
            // The extension names its own actions, so the plain label goes.
            out.clear();
            for (at, action) in actions.iter().enumerate() {
                let op = Op::ExtAction { ext: ext.clone(), command: command.clone(), item: item.clone(), action: action.clone() };
                let glyph = match action.kind {
                    ActionKind::Open(_) => GLYPH_PLAY,
                    ActionKind::Reveal(_) => GLYPH_FOLDER,
                    ActionKind::Copy(_) => GLYPH_COPY,
                    ActionKind::Paste(_) => GLYPH_CLIPBOARD,
                    ActionKind::Callback(_) => GLYPH_PLAY,
                    ActionKind::SetQuery(_) => GLYPH_SEARCH,
                };
                let keys = match at {
                    0 => Some("Enter".to_string()),
                    1 => Some("Ctrl+Enter".to_string()),
                    _ => action.shortcut.clone(),
                };
                let mut row = Row::op(action.title.clone(), "", glyph, op);
                row.keys = keys;
                out.push(row);
            }
        }
    }
    out
}

pub fn time_ago(unix: i64, now: i64) -> String {
    let age = (now - unix).max(0);
    match age {
        0..=59 => "just now".into(),
        60..=3599 => format!("{}m ago", age / 60),
        3600..=86_399 => format!("{}h ago", age / 3600),
        _ => format!("{}d ago", age / 86_400),
    }
}

/// `12345` as `12,345`.
fn grouped(number: usize) -> String {
    let digits = number.to_string();
    let mut out = String::with_capacity(digits.len() + digits.len() / 3);
    for (index, digit) in digits.chars().enumerate() {
        if index > 0 && (digits.len() - index) % 3 == 0 {
            out.push(',');
        }
        out.push(digit);
    }
    out
}

/// The folder every copied file is in, or a word for there being several.
fn shared_folder(paths: &[String]) -> Option<String> {
    let parent = |path: &str| std::path::Path::new(path).parent().map(|folder| folder.to_string_lossy().to_string()).filter(|folder| !folder.is_empty());
    let first = parent(paths.first()?)?;
    if paths.iter().all(|path| parent(path).as_deref() == Some(first.as_str())) { Some(first) } else { Some("Several folders".into()) }
}

/// The facts under a clipboard preview, as (label, value). `copied` is when, already worded.
pub fn clip_details(preview: &ClipPreview, copied: String) -> Vec<(&'static str, String)> {
    let item = &preview.item;
    let mut out = Vec::with_capacity(6);
    if !item.source_app.is_empty() {
        out.push(("Source", item.source_app.clone()));
    }
    match &preview.body {
        PreviewBody::Text { characters, words, lines, .. } => {
            let link = item.kind == ClipKind::Url;
            let kind = if link { "Link" } else if item.formatted { "Formatted text" } else { "Text" };
            out.push(("Type", kind.into()));
            out.push(("Characters", grouped(*characters)));
            if !link {
                out.push(("Words", grouped(*words)));
            }
            if *lines > 1 {
                out.push(("Lines", grouped(*lines)));
            }
        }
        PreviewBody::Files { paths, bytes } => {
            out.push(("Type", if paths.len() == 1 { "File" } else { "Files" }.into()));
            if paths.len() > 1 {
                out.push(("Items", grouped(paths.len())));
            }
            if let Some(folder) = shared_folder(paths) {
                out.push(("Location", folder));
            }
            if let Some(bytes) = bytes {
                out.push(("Size", format_bytes(*bytes as usize)));
            }
        }
        PreviewBody::Image { .. } | PreviewBody::Nothing => {
            out.push(("Type", "Image".into()));
            if item.width > 0 && item.height > 0 {
                out.push(("Dimensions", format!("{} × {}", item.width, item.height)));
            }
            if item.size > 0 {
                out.push(("Size", format_bytes(item.size as usize)));
            }
        }
    }
    out.push(("Copied", copied));
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn preview(kind: ClipKind, source: &str, body: PreviewBody) -> ClipPreview {
        let item = orbit_data::ClipItem { id: 1, kind, preview: String::new(), source_app: source.into(), created_at: 0, pinned: false, size: 8_294_400, width: 1920, height: 1080, formatted: false };
        ClipPreview { id: 1, item, body }
    }

    #[test]
    fn a_clip_menu_offers_what_suits_it() {
        let menu = |kind, formatted| {
            let row = Row { source: RowSource::Clip { id: 5, kind, pinned: false, formatted }, ..Row::info("x", "", GLYPH_CLIPBOARD) };
            menu_for(&row, None, false, None).into_iter().map(|row| row.title).collect::<Vec<_>>()
        };
        assert!(menu(ClipKind::Url, false).contains(&"Open in browser".to_string()));
        assert!(menu(ClipKind::Image, false).contains(&"Open picture".to_string()));
        let formatted = menu(ClipKind::Text, true);
        assert!(formatted.contains(&"Paste as plain text".to_string()));
        assert!(!formatted.iter().any(|title| title.starts_with("Open")), "plain text has nothing to open");
        assert!(!menu(ClipKind::Text, false).contains(&"Paste as plain text".to_string()), "plain already");
    }

    #[test]
    fn programs_from_history_are_not_opened() {
        for name in ["setup.EXE", "run.bat", "x.ps1", "a.lnk", "b.msi", "c.url"] {
            assert!(crate::launch::is_runnable(std::path::Path::new(name)), "{name}");
        }
        for name in ["photo.png", "notes.txt", "report.pdf", "folder"] {
            assert!(!crate::launch::is_runnable(std::path::Path::new(name)), "{name}");
        }
    }

    #[test]
    fn numbers_are_grouped_in_threes() {
        assert_eq!(grouped(0), "0");
        assert_eq!(grouped(999), "999");
        assert_eq!(grouped(1000), "1,000");
        assert_eq!(grouped(1_234_567), "1,234,567");
    }

    #[test]
    fn details_say_what_each_kind_of_clip_is() {
        let text = preview(ClipKind::Text, "Notepad", PreviewBody::Text { shown: String::new(), characters: 1200, words: 210, lines: 3 });
        let labels: Vec<_> = clip_details(&text, "Today at 10:31".into()).into_iter().collect();
        assert_eq!(labels[0], ("Source", "Notepad".to_string()));
        assert!(labels.contains(&("Characters", "1,200".to_string())));
        assert!(labels.contains(&("Lines", "3".to_string())));
        assert_eq!(labels.last().unwrap(), &("Copied", "Today at 10:31".to_string()));

        let image = preview(ClipKind::Image, "", PreviewBody::Nothing);
        let details = clip_details(&image, String::new());
        assert!(!details.iter().any(|(label, _)| *label == "Source"), "no source, no row");
        assert!(details.contains(&("Dimensions", "1920 × 1080".to_string())));
        assert!(details.contains(&("Size", "7.9 MB".to_string())));

        let files = preview(ClipKind::Files, "File Explorer", PreviewBody::Files { paths: vec!["C:\\Work\\a.txt".into(), "C:\\Work\\b.txt".into()], bytes: Some(10) });
        let details = clip_details(&files, String::new());
        assert!(details.contains(&("Items", "2".to_string())));
        assert!(details.contains(&("Location", "C:\\Work".to_string())));
        assert!(details.contains(&("Size", "10 bytes".to_string())));
    }

    #[test]
    fn files_from_different_folders_say_so() {
        assert_eq!(shared_folder(&["C:\\A\\x".into(), "D:\\B\\y".into()]).as_deref(), Some("Several folders"));
        assert_eq!(shared_folder(&[]), None);
    }

    #[test]
    fn time_labels() {
        assert_eq!(time_ago(100, 100), "just now");
        assert_eq!(time_ago(100, 40), "just now", "clock skew");
        assert_eq!(time_ago(0, 120), "2m ago");
        assert_eq!(time_ago(0, 7200), "2h ago");
        assert_eq!(time_ago(0, 172_800), "2d ago");
    }

    #[test]
    fn menus_offer_the_right_actions() {
        let note = Row {
            source: RowSource::Note { id: 3, pinned: true },
            primary: Some(Op::NoteOpen(3)),
            ..Row::info("n", "", GLYPH_NOTE)
        };
        let menu = menu_for(&note, None, false, None);
        assert_eq!(menu[0].primary, Some(Op::NoteOpen(3)));
        assert!(menu.iter().any(|r| r.primary == Some(Op::NotePin(3, false))), "pinned note offers unpin");
        assert!(menu.iter().any(|r| r.primary == Some(Op::NoteTrash(3))));

        let app = Row {
            source: RowSource::Catalog { item: 0, key: "app:x".into() },
            primary: Some(Op::RunItem { item: 0, key: "app:x".into() }),
            ..Row::info("x", "", GLYPH_NOTE)
        };
        let menu = menu_for(&app, Some("x"), false, None);
        assert!(menu.iter().any(|r| r.primary == Some(Op::AppAsAdmin("x".into()))));
        let command_menu = menu_for(&app, None, false, None);
        assert!(!command_menu.iter().any(|r| matches!(r.primary, Some(Op::AppAsAdmin(_)))));
    }

    #[test]
    fn an_apps_menu_shows_each_shortcut_once() {
        let app = Row {
            source: RowSource::Catalog { item: 0, key: "app:x".into() },
            primary: Some(Op::RunItem { item: 0, key: "app:x".into() }),
            ..Row::info("x", "", GLYPH_NOTE)
        };
        for pinned in [false, true] {
            let menu = menu_for(&app, Some("x"), pinned, None);
            let keys: Vec<&str> = menu.iter().filter_map(|row| row.keys.as_deref()).collect();
            let mut unique = keys.clone();
            unique.sort_unstable();
            unique.dedup();
            assert_eq!(keys.len(), unique.len(), "{keys:?}");
        }
        let menu = menu_for(&app, Some("x"), false, None);
        let keys_of = |op: Op| menu.iter().find(|row| row.primary == Some(op.clone())).and_then(|row| row.keys.clone());
        assert_eq!(keys_of(Op::AppNewWindow("x".into())).as_deref(), Some("Ctrl+Enter"));
        assert_eq!(keys_of(Op::AppAsAdmin("x".into())).as_deref(), Some("Ctrl+Shift+Enter"));
        assert_eq!(secondary_for(&app, Some("x")).map(|(_, op)| op), Some(Op::AppNewWindow("x".into())));
    }

    #[test]
    fn menus_offer_a_hotkey_and_show_the_current_one() {
        let app = Row {
            source: RowSource::Catalog { item: 0, key: "app:x".into() },
            primary: Some(Op::RunItem { item: 0, key: "app:x".into() }),
            ..Row::info("x", "", GLYPH_NOTE)
        };
        let action = Action::FocusOrLaunchApp { id: "x".into(), name: "x".into() };
        let fresh = menu_for(&app, Some("x"), false, Some(HotkeyChoice { action: action.clone(), current: None }));
        assert!(fresh.iter().any(|r| r.title == "Set hotkey…"));
        assert!(!fresh.iter().any(|r| r.title == "Remove hotkey"));

        let bound = menu_for(&app, Some("x"), false, Some(HotkeyChoice { action, current: Some("Hyper+C".into()) }));
        let change = bound.iter().find(|r| r.title == "Change hotkey…").unwrap();
        assert_eq!(change.keys.as_deref(), Some("Hyper+C"));
        assert!(bound.iter().any(|r| r.title == "Remove hotkey"));
    }

    #[test]
    fn secret_rows_offer_only_what_each_tool_can_do() {
        let login = Row {
            source: RowSource::Secret { tool: Tool::OnePassword, id: "a".into(), url: Some("https://x.example".into()) },
            primary: Some(Op::CopySecret { tool: Tool::OnePassword, id: "a".into(), field: Field::Password }),
            ..Row::info("GitHub", "", GLYPH_KEY)
        };
        let menu = menu_for(&login, None, false, None);
        let titles: Vec<&str> = menu.iter().map(|row| row.title.as_str()).collect();
        assert_eq!(titles, ["Copy password", "Copy username", "Copy one-time code", "Open website", "Refresh the list"]);

        let code = Row {
            source: RowSource::Secret { tool: Tool::EnteAuth, id: "b".into(), url: None },
            primary: Some(Op::CopySecret { tool: Tool::EnteAuth, id: "b".into(), field: Field::OneTimeCode }),
            ..Row::info("GitHub", "", GLYPH_KEY)
        };
        let titles: Vec<String> = menu_for(&code, None, false, None).into_iter().map(|row| row.title).collect();
        assert_eq!(titles, ["Copy code", "Refresh the list"]);
    }

    #[test]
    fn info_rows_have_no_action() {
        assert_eq!(Row::info("t", "", GLYPH_INFO).primary, None);
        assert_eq!(secondary_for(&Row::info("t", "", GLYPH_INFO), None), None);
    }
}
