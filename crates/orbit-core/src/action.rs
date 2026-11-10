//! Actions that shortcuts and results can run, plus the safety rules around URLs.

use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Action {
    /// Switch to the app if running, otherwise start it.
    FocusOrLaunchApp { id: String, name: String },
    LaunchApp { id: String, name: String },
    OpenPath { path: String },
    Url { url: String },
    Builtin { command: Builtin },
    /// A command the user configured under Settings. Runs only from an explicit trigger.
    Custom { id: String },
    OpenNote { id: i64 },
    InsertSnippet { id: i64 },
    /// A command from an extension, by the extension's id and the command's.
    Extension { ext: String, command: String },
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq, Hash)]
#[serde(rename_all = "snake_case")]
pub enum Builtin {
    ToggleLauncher,
    NewNote,
    SearchNotes,
    ClipboardHistory,
    SnippetSearch,
    NewSnippet,
    NewQuicklink,
    SearchQuicklinks,
    EmojiPicker,
    PauseClipboard,
    ResumeClipboard,
    SkipNextCopy,
    ClearClipboard,
    Settings,
    Diagnostics,
    RebuildAppIndex,
    SuspendShortcuts,
    ResumeShortcuts,
    ToggleLowResource,
    RestartOrbit,
    CheckForUpdates,
    QuitOrbit,
    LockScreen,
    Sleep,
    Shutdown,
    Restart,
    EmptyTrash,
    OpenDownloads,
    OpenTerminal,
    ShowHelp,
    ExportData,
    ResetShortcuts,
    DeleteLocalData,
    SpeedTest,
    WindowLeft,
    WindowRight,
    WindowTop,
    WindowBottom,
    WindowMaximize,
    WindowAlmostMaximize,
    WindowCenter,
    WindowNextDisplay,
    ToggleDarkMode,
    ToggleBluetooth,
    ToggleWifi,
    ToggleMute,
    Timer,
    Stopwatch,
    PickColour,
    SearchFiles,
    ImportData,
    ManageExtensions,
    Weather,
}

impl Builtin {
    /// Commands only built for Windows so far, which the macOS app leaves out.
    pub const fn windows_only(self) -> bool {
        matches!(
            self,
            Builtin::SpeedTest
                | Builtin::WindowLeft
                | Builtin::WindowRight
                | Builtin::WindowTop
                | Builtin::WindowBottom
                | Builtin::WindowMaximize
                | Builtin::WindowAlmostMaximize
                | Builtin::WindowCenter
                | Builtin::WindowNextDisplay
                | Builtin::ToggleDarkMode
                | Builtin::ToggleBluetooth
                | Builtin::ToggleWifi
                | Builtin::ToggleMute
                | Builtin::Timer
                | Builtin::Stopwatch
                | Builtin::PickColour
                | Builtin::SearchFiles
                | Builtin::ImportData
                | Builtin::ManageExtensions
                | Builtin::Weather
        )
    }

    /// Destructive commands always ask for confirmation in the launcher first.
    pub const fn needs_confirmation(self) -> bool {
        matches!(
            self,
            Builtin::Shutdown
                | Builtin::Restart
                | Builtin::Sleep
                | Builtin::EmptyTrash
                | Builtin::ClearClipboard
                | Builtin::ResetShortcuts
                | Builtin::DeleteLocalData
        )
    }

    /// (key, title, subtitle, search keywords) for each command shown in the launcher.
    pub const ALL: &'static [(Builtin, &'static str, &'static str, &'static str, &'static str)] = &[
        (Builtin::NewNote, "cmd:new-note", "New Note", "Create a note", "write quick text jot"),
        (Builtin::SearchNotes, "cmd:notes", "Search Notes", "All notes", "notes list browse"),
        (Builtin::ClipboardHistory, "cmd:clipboard", "Clipboard History", "Search what you copied", "paste copy history"),
        (Builtin::SnippetSearch, "cmd:snippets", "Search Snippets", "Insert saved text", "templates text"),
        (Builtin::NewSnippet, "cmd:new-snippet", "Create Snippet", "Save text to reuse", "new add template"),
        (Builtin::NewQuicklink, "cmd:new-link", "Create Quicklink", "Save a link or a web search", "new add bookmark url website"),
        (Builtin::SearchQuicklinks, "cmd:links", "Search Quicklinks", "Your saved links", "bookmarks urls websites"),
        (Builtin::EmojiPicker, "cmd:emoji", "Search Emoji & Symbols", "Paste any emoji", "emoticon smiley picker"),
        (Builtin::PauseClipboard, "cmd:clipboard-pause", "Pause Clipboard History", "Stop recording copies", "private stop"),
        (Builtin::ResumeClipboard, "cmd:clipboard-resume", "Resume Clipboard History", "Record copies again", "start"),
        (Builtin::SkipNextCopy, "cmd:clipboard-skip", "Do Not Remember Next Copy", "Skip one clipboard entry", "private password"),
        (Builtin::ClearClipboard, "cmd:clipboard-clear", "Clear Clipboard History", "Delete unpinned entries", "delete wipe"),
        (Builtin::Settings, "cmd:settings", "Orbit Settings", "Shortcuts, HyperKey, privacy", "preferences options config hotkey"),
        (Builtin::Diagnostics, "cmd:diagnostics", "Orbit Diagnostics", "Memory, index and timing", "performance memory stats"),
        (Builtin::RebuildAppIndex, "cmd:reindex", "Rebuild App Index", "Find newly installed apps", "refresh rescan"),
        (Builtin::SuspendShortcuts, "cmd:suspend", "Suspend Orbit Shortcuts", "HyperKey and global shortcuts off", "game pause disable"),
        (Builtin::ResumeShortcuts, "cmd:resume", "Resume Orbit Shortcuts", "HyperKey and global shortcuts on", "enable"),
        (Builtin::ToggleLowResource, "cmd:low-resource", "Toggle Low Resource Mode", "Shed caches and optional work", "memory performance"),
        (Builtin::ExportData, "cmd:export", "Export Orbit Data", "Notes, snippets and settings", "backup portability"),
        (Builtin::ResetShortcuts, "cmd:reset-shortcuts", "Reset All Shortcuts", "Back to the default launcher key, with no other shortcuts or mappings", "hotkey hyper mappings defaults"),
        (Builtin::DeleteLocalData, "cmd:delete-data", "Delete All Orbit Data", "Notes, snippets, clipboard history and settings, for good. Orbit then quits", "erase wipe remove uninstall"),
        (Builtin::ShowHelp, "cmd:help", "Orbit Help", "Keys and shortcuts", "keyboard shortcuts ?"),
        (Builtin::RestartOrbit, "cmd:restart-orbit", "Restart Orbit", "Quit and start again", "reload"),
        (Builtin::CheckForUpdates, "cmd:update", "Check for Updates", "Get the newest version of Orbit", "update upgrade new version release latest download install"),
        (Builtin::QuitOrbit, "cmd:quit", "Quit Orbit", "Stop Orbit, keyboard returns to normal", "exit close"),
        (Builtin::LockScreen, "cmd:lock", "Lock Screen", "System", "security away"),
        (Builtin::Sleep, "cmd:sleep", "Sleep", "System, asks first", "suspend"),
        (Builtin::Shutdown, "cmd:shutdown", "Shut Down", "System, asks first", "power off turn off"),
        (Builtin::Restart, "cmd:restart", "Restart Computer", "System, asks first", "reboot"),
        (Builtin::EmptyTrash, "cmd:empty-trash", "Empty Recycle Bin", "System, asks first", "trash delete"),
        (Builtin::OpenDownloads, "cmd:downloads", "Open Downloads", "Folder", "files"),
        (Builtin::OpenTerminal, "cmd:terminal", "Open Terminal", "System", "console shell cmd"),
        (Builtin::SpeedTest, "cmd:speedtest", "Speed Test", "Internet speed, by Speedtest.net", "internet bandwidth download upload ping ookla speedtest connection wifi broadband"),
        (Builtin::WindowLeft, "cmd:window-left", "Left Half", "Window layout", "snap window move side tile arrange split"),
        (Builtin::WindowRight, "cmd:window-right", "Right Half", "Window layout", "snap window move side tile arrange split"),
        (Builtin::WindowTop, "cmd:window-top", "Top Half", "Window layout", "snap window move upper tile arrange split"),
        (Builtin::WindowBottom, "cmd:window-bottom", "Bottom Half", "Window layout", "snap window move lower tile arrange split"),
        (Builtin::WindowMaximize, "cmd:window-maximize", "Maximise Window", "Window layout", "maximize full fill window big"),
        (Builtin::WindowAlmostMaximize, "cmd:window-almost-maximize", "Almost Maximise", "Window layout, with a margin around it", "maximize window big margin"),
        (Builtin::WindowCenter, "cmd:window-center", "Centre Window", "Window layout", "center middle window move"),
        (Builtin::WindowNextDisplay, "cmd:window-next-display", "Move to Next Display", "Window layout", "monitor screen display window move other"),
        (Builtin::ToggleDarkMode, "cmd:dark-mode", "Toggle Dark Mode", "Switch Windows between dark and light", "theme light appearance night colour color mode"),
        (Builtin::ToggleBluetooth, "cmd:bluetooth", "Toggle Bluetooth", "Turn Bluetooth on or off", "bluetooth radio wireless headphones"),
        (Builtin::ToggleWifi, "cmd:wifi", "Toggle Wi-Fi", "Turn Wi-Fi on or off", "wifi wi-fi wireless internet network radio"),
        (Builtin::ToggleMute, "cmd:mute", "Toggle Mute", "Mute or unmute the sound", "sound audio volume speaker silence quiet"),
        (Builtin::Timer, "cmd:timer", "Timer", "Count down from a time you type", "countdown alarm minutes clock egg"),
        (Builtin::Stopwatch, "cmd:stopwatch", "Stopwatch", "Count up from zero", "timer clock time lap"),
        (Builtin::PickColour, "cmd:pick-colour", "Pick Colour", "Copy the colour of anything on screen", "color colour picker eyedropper hex rgb pixel"),
        (Builtin::SearchFiles, "cmd:files", "Everything", "Search every file and folder on your PC", "search files find file folder document locate explorer voidtools disk"),
        (Builtin::ImportData, "cmd:import", "Import from Another Launcher", "Raycast, Flow Launcher, PowerToys Run, Wox or an Orbit export", "import migrate move bring settings quicklinks snippets raycast flow powertoys wox"),
        (Builtin::ManageExtensions, "cmd:extensions", "Manage Extensions", "Add extensions and turn them on or off", "extensions plugins add-ons addons install store"),
        (Builtin::Weather, "cmd:weather", "Weather", "The forecast for a place you type", "forecast temperature rain sun wind outside"),
    ];

    pub fn from_key(key: &str) -> Option<Builtin> {
        Builtin::ALL.iter().find(|entry| entry.1 == key).map(|entry| entry.0)
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum UrlCheck {
    Safe,
    /// A scheme that works but starts something other than a browser. Ask before saving.
    Unusual(String),
    /// A scheme that can run code or read local files. Refused.
    Blocked(String),
    Invalid,
}

const SAFE_SCHEMES: &[&str] = &["http", "https", "mailto", "tel", "ftp", "ftps", "ms-settings", "x-apple.systempreferences"];
const BLOCKED_SCHEMES: &[&str] = &["javascript", "vbscript", "data", "file", "ms-msdt", "search-ms", "ms-officecmd", "shell"];

fn scheme_of(url: &str) -> Option<&str> {
    let (scheme, _) = url.split_once(':')?;
    let mut chars = scheme.chars();
    let first = chars.next()?;
    if first.is_ascii_alphabetic() && chars.all(|c| c.is_ascii_alphanumeric() || matches!(c, '+' | '-' | '.')) {
        Some(scheme)
    } else {
        None
    }
}

pub fn check_url(url: &str) -> UrlCheck {
    let url = url.trim();
    if url.is_empty() || url.chars().any(char::is_control) {
        return UrlCheck::Invalid;
    }
    let Some(scheme) = scheme_of(url) else {
        return UrlCheck::Invalid;
    };
    let lower = scheme.to_ascii_lowercase();
    if BLOCKED_SCHEMES.contains(&lower.as_str()) {
        UrlCheck::Blocked(lower)
    } else if SAFE_SCHEMES.contains(&lower.as_str()) {
        UrlCheck::Safe
    } else {
        UrlCheck::Unusual(lower)
    }
}

/// Fills a `{query}` template with percent-encoding, so the query cannot alter the URL's shape.
pub fn fill_url_template(template: &str, query: &str) -> String {
    let mut encoded = String::with_capacity(query.len() + 8);
    for byte in query.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => encoded.push(byte as char),
            b' ' => encoded.push('+'),
            _ => encoded.push_str(&format!("%{byte:02X}")),
        }
    }
    template.replace("{query}", &encoded)
}

/// If the text is a bare address or URL, returns what to open. Used for "paste a URL, press Enter".
pub fn as_openable_url(text: &str) -> Option<String> {
    let t = text.trim();
    if t.contains(char::is_whitespace) || t.len() < 4 {
        return None;
    }
    if t.starts_with("http://") || t.starts_with("https://") {
        return Some(t.to_string());
    }
    let host = t.split('/').next()?;
    let tld = host.rsplit('.').next()?;
    let looks_like_domain = host.contains('.')
        && tld.len() >= 2
        && tld.chars().all(|c| c.is_ascii_alphabetic())
        && host.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '-'));
    if looks_like_domain {
        // Common filename extensions look like TLDs. Don't turn "notes.txt" into a website.
        const NOT_TLDS: &[&str] = &["txt", "md", "exe", "pdf", "png", "jpg", "zip", "rs", "py", "js", "json", "doc", "docx", "xlsx", "csv", "log", "ini", "bat"];
        if NOT_TLDS.contains(&tld.to_ascii_lowercase().as_str()) {
            return None;
        }
        Some(format!("https://{t}"))
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classifies_url_schemes() {
        assert_eq!(check_url("https://example.com"), UrlCheck::Safe);
        assert_eq!(check_url("HTTP://EXAMPLE.COM"), UrlCheck::Safe);
        assert_eq!(check_url("mailto:a@b.c"), UrlCheck::Safe);
        assert_eq!(check_url("spotify:track:1"), UrlCheck::Unusual("spotify".into()));
        assert_eq!(check_url("vscode://file/x"), UrlCheck::Unusual("vscode".into()));
        assert_eq!(check_url("javascript:alert(1)"), UrlCheck::Blocked("javascript".into()));
        assert_eq!(check_url("file:///C:/Windows/System32/calc.exe"), UrlCheck::Blocked("file".into()));
        assert_eq!(check_url("ms-msdt:/id"), UrlCheck::Blocked("ms-msdt".into()));
        assert_eq!(check_url("not a url"), UrlCheck::Invalid);
        assert_eq!(check_url("https://a\u{0}b"), UrlCheck::Invalid);
        assert_eq!(check_url(""), UrlCheck::Invalid);
    }

    #[test]
    fn template_encoding_cannot_break_out() {
        assert_eq!(fill_url_template("https://s?q={query}", "rust async"), "https://s?q=rust+async");
        assert_eq!(fill_url_template("https://s?q={query}", "a&b=c#d"), "https://s?q=a%26b%3Dc%23d");
        assert_eq!(fill_url_template("https://s?q={query}", "é"), "https://s?q=%C3%A9");
    }

    #[test]
    fn detects_openable_urls() {
        assert_eq!(as_openable_url("github.com"), Some("https://github.com".into()));
        assert_eq!(as_openable_url("github.com/a/b"), Some("https://github.com/a/b".into()));
        assert_eq!(as_openable_url("http://x.io"), Some("http://x.io".into()));
        assert_eq!(as_openable_url("notes.txt"), None);
        assert_eq!(as_openable_url("hello world.com"), None);
        assert_eq!(as_openable_url("code"), None);
        assert_eq!(as_openable_url("3.14"), None);
    }

    #[test]
    fn every_builtin_key_round_trips_and_is_unique() {
        let mut keys = std::collections::HashSet::new();
        for entry in Builtin::ALL {
            assert_eq!(Builtin::from_key(entry.1), Some(entry.0));
            assert!(keys.insert(entry.1), "duplicate key {}", entry.1);
        }
    }

    #[test]
    fn destructive_commands_confirm() {
        assert!(Builtin::Shutdown.needs_confirmation());
        assert!(Builtin::EmptyTrash.needs_confirmation());
        assert!(!Builtin::LockScreen.needs_confirmation());
    }
}
