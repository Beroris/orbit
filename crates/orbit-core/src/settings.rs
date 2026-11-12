//! User settings, stored as one small JSON file so startup needs no database and the file
//! is readable and portable. Every section has defaults and unknown fields are ignored, so
//! older and newer versions can read each other's files.

use crate::action::Action;
use orbit_hotkeys::{Chord, HyperMode, TapBehavior};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

pub const SCHEMA_VERSION: u32 = 1;

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct Settings {
    pub schema_version: u32,
    pub launcher: LauncherSettings,
    pub hyper: HyperSettings,
    pub mac_feel: MacFeelSettings,
    pub integrations: IntegrationSettings,
    /// Direct global shortcuts, registered with the OS without any input hook.
    pub shortcuts: Vec<ShortcutBinding>,
    pub clipboard: ClipboardSettings,
    pub snippets: SnippetSettings,
    pub links: Vec<QuickLink>,
    pub custom_commands: Vec<CustomCommand>,
    pub emoji: EmojiSettings,
    /// Item key to alias, for any searchable item.
    pub aliases: BTreeMap<String, String>,
    /// The shortcut problem last announced, so the same one isn't repeated at every start.
    pub hotkey_notice: String,
    pub pinned: Vec<String>,
    pub hidden: Vec<String>,
    pub appearance: Appearance,
    /// How Orbit's own commands' symbols are drawn. Apps keep their own icons.
    #[serde(deserialize_with = "or_default")]
    pub icon_style: IconStyle,
    pub low_resource_mode: bool,
    /// Ask each quicklink's own site for its icon, once.
    pub fetch_link_icons: bool,
    /// Fetch the European Central Bank's daily exchange rates, when a currency is converted.
    pub fetch_exchange_rates: bool,
    /// Look up places and forecasts with Open-Meteo, when weather is asked for.
    pub fetch_weather: bool,
    /// The last place weather was looked up for, which plain "weather" shows.
    pub weather_place: Option<crate::weather::Place>,
    /// Ookla's terms for Speedtest CLI were accepted in the launcher, so a speed test can run.
    pub speedtest_accepted: bool,
    pub launch_at_login: bool,
    /// Look for a new version on GitHub, once a day.
    pub check_updates: bool,
    /// True once first-run consent has been shown.
    pub onboarded: bool,
    /// File search through Everything, which is built in, and extensions added from folders.
    pub extensions: ExtensionSettings,
}

/// Extensions. Everything's file search comes with Orbit; others are folders in the data folder's
/// `extensions` folder, each off until the user turns it on.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct ExtensionSettings {
    pub everything: EverythingSettings,
    /// Speed Test, which comes with Orbit. On unless turned off.
    pub speed_test: bool,
    /// GitHub, through GitHub's `gh` command-line tool. Off until turned on.
    pub github: bool,
    /// Extensions from folders that the user turned on, by id.
    pub enabled: Vec<String>,
    /// A command's keyword where the user changed it from the extension's own, keyed `id/command`.
    /// An empty one means no keyword.
    pub keywords: BTreeMap<String, String>,
}

impl Default for ExtensionSettings {
    fn default() -> Self {
        ExtensionSettings {
            everything: EverythingSettings::default(),
            speed_test: true,
            github: false,
            enabled: Vec::new(),
            keywords: BTreeMap::new(),
        }
    }
}

/// File search with voidtools' Everything, which keeps its own index of every file name.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct EverythingSettings {
    pub enabled: bool,
    /// What the Everything command answers to when typed, as any alias does. Followed by a space
    /// and more, it searches files straight away. None until one is set: the command is found by
    /// its name, as everything else is.
    pub alias: String,
    /// A few matching files under the other results when searching in the launcher.
    pub in_search: bool,
    pub sort: FileSort,
}

impl Default for EverythingSettings {
    fn default() -> Self {
        EverythingSettings { enabled: true, alias: String::new(), in_search: true, sort: FileSort::Recent }
    }
}

/// Which files come first.
#[derive(Clone, Copy, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum FileSort {
    /// Most recently changed first.
    #[default]
    Recent,
    /// Most often opened first, by Everything's own count.
    MostOpened,
    Name,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct LauncherSettings {
    pub hotkey: Option<Chord>,
    /// Keep the last query when the launcher reopens.
    pub preserve_query: bool,
    pub max_visible_rows: u8,
    /// Enter on an app switches to its running window instead of starting another instance.
    pub focus_running_app: bool,
    /// An app's hotkey, pressed while that app is in front, minimizes it.
    pub hotkey_toggles_app: bool,
    /// Standard keys, Vim keys with Alt, or full Vim.
    pub keyboard: crate::vim::KeyboardMode,
    /// With an empty search, show favourites and nothing else.
    pub favorites_only: bool,
    /// See-through behind the launcher, where Windows allows it.
    pub glass: bool,
    /// The launcher is only as tall as its results, rather than always its full height.
    pub fit_results: bool,
    /// The launcher fades in and out, and its selection, menus and height move rather than jump.
    /// Windows' own "Animation effects" being off turns this off too.
    pub animate: bool,
    /// Programs where the launcher's hotkey goes to the program instead, by file name.
    pub excluded_apps: Vec<String>,
    /// The same whenever a game is in front.
    pub pass_through_in_games: bool,
    /// Enter copies a sum's or conversion's answer formatted, "£1,234.50", rather than "1234.50".
    pub copy_formatted: bool,
    /// Which screen the launcher opens on.
    #[serde(deserialize_with = "or_default")]
    pub screen: LauncherScreen,
}

/// Which screen the launcher opens on.
#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum LauncherScreen {
    /// The screen with the window being worked in, or the pointer's when there's none.
    #[default]
    Active,
    /// The screen the pointer is on.
    Pointer,
    /// The screen Windows marks as the main one, whichever monitor that is.
    Main,
    /// One screen picked in Settings, every time while it's connected, and otherwise as `Active`.
    Specific {
        /// Windows' path for the monitor on its connection, which stays the same between starts.
        id: String,
        /// The monitor's own name, such as "DELL U2720Q", to find it on another connection and to
        /// say which one it was while it isn't connected.
        name: String,
    },
}

/// A value this version doesn't know, such as a choice added later, gives the default rather than
/// making the whole file unreadable.
fn or_default<'de, D, T>(deserializer: D) -> Result<T, D::Error>
where
    D: serde::Deserializer<'de>,
    T: Deserialize<'de> + Default,
{
    let value = serde_json::Value::deserialize(deserializer)?;
    Ok(T::deserialize(value).unwrap_or_default())
}

/// Other apps that show up in the launcher, through their own command-line tools.
#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct IntegrationSettings {
    pub one_password: bool,
    pub ente_auth: bool,
}

/// macOS-style shortcuts in every app, using Alt where a Mac uses Cmd.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct MacFeelSettings {
    pub enabled: bool,
    /// Alt+Left and Alt+Right for the ends of the line, Alt+Backspace to delete back to the start.
    pub text_keys: bool,
    /// Alt+Q to quit the app in front, Alt+H to hide it.
    pub app_keys: bool,
    pub excluded_apps: Vec<String>,
}

impl Default for MacFeelSettings {
    fn default() -> Self {
        MacFeelSettings { enabled: false, text_keys: true, app_keys: true, excluded_apps: Vec::new() }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct HyperSettings {
    pub enabled: bool,
    pub trigger: String,
    pub mode: HyperMode,
    pub tap: TapBehavior,
    pub tap_timeout_ms: u32,
    pub bindings: Vec<HyperBinding>,
    /// Executable names, matched case-insensitively, where Hyper is passed through.
    pub excluded_apps: Vec<String>,
    /// Pass Hyper through while a game fills the screen. Other fullscreen apps keep it.
    pub pass_through_in_games: bool,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct HyperBinding {
    pub key: String,
    pub action: Action,
    #[serde(default = "yes")]
    pub enabled: bool,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct ShortcutBinding {
    pub chord: Chord,
    pub action: Action,
    #[serde(default = "yes")]
    pub enabled: bool,
}

fn yes() -> bool {
    true
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct ClipboardSettings {
    /// Off until the user opts in.
    pub enabled: bool,
    /// When false, history lives in memory only and is gone when Orbit exits.
    pub persist: bool,
    pub capture_images: bool,
    pub max_items: u32,
    pub retention_days: u32,
    pub max_item_bytes: u32,
    pub excluded_apps: Vec<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct SnippetSettings {
    /// Automatic expansion needs an input hook, so it is off until the user opts in.
    pub expansion_enabled: bool,
    pub excluded_apps: Vec<String>,
}

/// The emoji list's own sections. Stored on this PC only, like everything else here.
#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct EmojiSettings {
    pub pinned: Vec<String>,
    /// Most recent first, and never more than `EMOJI_RECENT_MAX`.
    pub recent: Vec<String>,
}

pub const EMOJI_RECENT_MAX: usize = 20;

impl EmojiSettings {
    pub fn note_used(&mut self, glyph: &str) {
        self.recent.retain(|other| other != glyph);
        self.recent.insert(0, glyph.to_string());
        self.recent.truncate(EMOJI_RECENT_MAX);
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct QuickLink {
    pub id: String,
    pub name: String,
    /// May contain `{query}`, which makes it a search template.
    pub url: String,
    #[serde(default)]
    pub alias: String,
    /// The app that opens it, by its id in the app index. Empty for whatever Windows picks.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub open_with: String,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct CustomCommand {
    pub id: String,
    pub name: String,
    pub program: String,
    #[serde(default)]
    pub arguments: String,
    #[serde(default)]
    pub working_dir: String,
    #[serde(default)]
    pub alias: String,
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "snake_case")]
pub enum Appearance {
    #[default]
    System,
    Light,
    Dark,
}

/// How the symbols of Orbit's own commands are drawn, in the launcher and in Settings.
#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "snake_case")]
pub enum IconStyle {
    /// The symbol alone, in grey.
    #[default]
    Plain,
    /// The symbol alone, in its own colour.
    Coloured,
    /// The symbol on a grey square.
    GreyTiles,
    /// The symbol in its colour, on a faint square of that colour.
    TintedTiles,
}

/// Apps that commonly hold secrets. A best effort, not a guarantee: see docs/PRIVACY.md.
pub const DEFAULT_SECRET_APPS: &[&str] = &[
    "1password.exe",
    "bitwarden.exe",
    "keepass.exe",
    "keepassxc.exe",
    "lastpass.exe",
    "dashlane.exe",
    "enpass.exe",
    "nordpass.exe",
    "protonpass.exe",
    "authy.exe",
    "1Password",
    "Bitwarden",
    "KeePassXC",
    "Keychain Access",
];

impl Default for Settings {
    fn default() -> Self {
        Settings {
            schema_version: SCHEMA_VERSION,
            launcher: LauncherSettings::default(),
            hyper: HyperSettings::default(),
            mac_feel: MacFeelSettings::default(),
            integrations: IntegrationSettings::default(),
            shortcuts: Vec::new(),
            clipboard: ClipboardSettings::default(),
            snippets: SnippetSettings::default(),
            links: vec![QuickLink {
                id: "default-search".into(),
                name: "Search Google".into(),
                url: "https://www.google.com/search?q={query}".into(),
                alias: "g".into(),
                open_with: String::new(),
            }],
            custom_commands: Vec::new(),
            emoji: EmojiSettings::default(),
            aliases: BTreeMap::new(),
            hotkey_notice: String::new(),
            pinned: Vec::new(),
            hidden: Vec::new(),
            appearance: Appearance::System,
            icon_style: IconStyle::Plain,
            low_resource_mode: false,
            fetch_link_icons: true,
            fetch_exchange_rates: true,
            fetch_weather: true,
            weather_place: None,
            speedtest_accepted: false,
            launch_at_login: false,
            check_updates: true,
            onboarded: false,
            extensions: ExtensionSettings::default(),
        }
    }
}

impl Default for LauncherSettings {
    fn default() -> Self {
        LauncherSettings {
            hotkey: None,
            preserve_query: false,
            max_visible_rows: 8,
            focus_running_app: true,
            hotkey_toggles_app: true,
            keyboard: crate::vim::KeyboardMode::Standard,
            favorites_only: false,
            copy_formatted: false,
            glass: true,
            fit_results: true,
            animate: true,
            excluded_apps: Vec::new(),
            pass_through_in_games: true,
            screen: LauncherScreen::Active,
        }
    }
}

impl Default for HyperSettings {
    fn default() -> Self {
        HyperSettings {
            enabled: false,
            trigger: "CapsLock".into(),
            mode: HyperMode::Internal,
            tap: TapBehavior::Escape,
            tap_timeout_ms: 250,
            bindings: Vec::new(),
            excluded_apps: Vec::new(),
            pass_through_in_games: true,
        }
    }
}

impl Default for ClipboardSettings {
    fn default() -> Self {
        ClipboardSettings {
            enabled: false,
            persist: true,
            capture_images: true,
            max_items: 500,
            retention_days: 30,
            max_item_bytes: 256 * 1024,
            excluded_apps: DEFAULT_SECRET_APPS.iter().map(|s| s.to_string()).collect(),
        }
    }
}

impl Default for SnippetSettings {
    fn default() -> Self {
        SnippetSettings {
            expansion_enabled: false,
            excluded_apps: DEFAULT_SECRET_APPS.iter().map(|s| s.to_string()).collect(),
        }
    }
}

#[derive(Debug)]
pub enum LoadOutcome {
    /// File parsed. It may have been migrated.
    Loaded,
    /// No file: first run.
    Missing,
    /// File is damaged. Defaults are used and the bad file is left for the host to set aside.
    Corrupt(String),
    /// File is from a newer version. Read best-effort, and the host should not overwrite it.
    NewerVersion(u32),
}

impl Settings {
    pub fn parse(text: &str) -> (Settings, LoadOutcome) {
        match serde_json::from_str::<Settings>(text) {
            Ok(mut settings) => {
                let version = settings.schema_version;
                if version > SCHEMA_VERSION {
                    return (settings, LoadOutcome::NewerVersion(version));
                }
                settings.migrate();
                (settings, LoadOutcome::Loaded)
            }
            Err(error) => (Settings::default(), LoadOutcome::Corrupt(error.to_string())),
        }
    }

    fn migrate(&mut self) {
        // Version-to-version steps go here. Version 1 is the first schema.
        self.schema_version = SCHEMA_VERSION;
    }

    pub fn to_json(&self) -> String {
        serde_json::to_string_pretty(self).expect("settings always serialise")
    }

    /// Reports every key bound twice, so settings can show conflicts rather than dropping one.
    pub fn duplicate_hyper_keys(&self) -> Vec<String> {
        let mut seen = std::collections::BTreeSet::new();
        let mut duplicates = Vec::new();
        for binding in self.hyper.bindings.iter().filter(|b| b.enabled) {
            let key = binding.key.to_ascii_uppercase();
            if !seen.insert(key.clone()) && !duplicates.contains(&key) {
                duplicates.push(key);
            }
        }
        duplicates
    }

    pub fn is_app_excluded(list: &[String], exe_name: &str) -> bool {
        list.iter().any(|entry| entry.eq_ignore_ascii_case(exe_name))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A settings file as version 1 wrote it, kept unchanged in `fixtures`. If this breaks, a
    /// change needs a migration step instead.
    #[test]
    fn a_version_1_settings_file_still_loads() {
        let (settings, outcome) = Settings::parse(include_str!("../../../fixtures/settings-v1.json"));
        assert!(matches!(outcome, LoadOutcome::Loaded));
        assert_eq!(settings.schema_version, SCHEMA_VERSION);
        assert!(settings.onboarded);
        assert!(settings.hyper.enabled);
        assert_eq!(settings.hyper.trigger, "CapsLock");
        assert_eq!(settings.hyper.bindings.len(), 1);
        assert_eq!(settings.shortcuts.len(), 1);
        assert_eq!(settings.clipboard.max_items, 250);
        assert_eq!(settings.links[0].alias, "g");
    }

    #[test]
    fn defaults_are_conservative() {
        let s = Settings::default();
        assert!(!s.hyper.enabled, "Hyper must be opt-in");
        assert!(!s.clipboard.enabled, "clipboard capture must be opt-in");
        assert!(!s.snippets.expansion_enabled, "expansion must be opt-in");
        assert!(!s.launch_at_login);
        assert!(s.hyper.pass_through_in_games, "games keep their keys unless asked otherwise");
    }

    #[test]
    fn round_trips() {
        let mut s = Settings::default();
        s.launcher.hotkey = Some(Chord::parse("Ctrl+Alt+Space").unwrap());
        s.hyper.bindings.push(HyperBinding {
            key: "C".into(),
            action: Action::Url { url: "https://example.com".into() },
            enabled: true,
        });
        let (parsed, outcome) = Settings::parse(&s.to_json());
        assert!(matches!(outcome, LoadOutcome::Loaded));
        assert_eq!(parsed, s);
    }

    #[test]
    fn missing_sections_take_defaults_and_unknown_fields_are_ignored() {
        let (s, outcome) = Settings::parse(r#"{"schema_version":1,"hyper":{"enabled":true},"from_the_future":42}"#);
        assert!(matches!(outcome, LoadOutcome::Loaded));
        assert!(s.hyper.enabled);
        assert_eq!(s.hyper.trigger, "CapsLock");
        assert_eq!(s.clipboard.max_items, 500);
    }

    #[test]
    fn settings_from_when_ai_usage_was_an_extension_still_load() {
        let (s, outcome) = Settings::parse(r#"{"schema_version":1,"extensions":{"github":true,"ai_usage":true,"keywords":{"ai-usage/usage":"quota","github/pulls":"prs"}}}"#);
        assert!(matches!(outcome, LoadOutcome::Loaded));
        assert!(s.extensions.github, "the rest of the section is kept");
        assert_eq!(s.extensions.keywords.get("github/pulls").map(String::as_str), Some("prs"));
    }

    #[test]
    fn icons_are_plain_unless_chosen_and_an_unknown_style_is_plain() {
        assert_eq!(Settings::default().icon_style, IconStyle::Plain);
        let mut s = Settings::default();
        s.icon_style = IconStyle::TintedTiles;
        assert!(s.to_json().contains(r#""icon_style": "tinted_tiles""#), "{}", s.to_json());
        assert_eq!(Settings::parse(&s.to_json()).0.icon_style, IconStyle::TintedTiles);
        // A style from a later version loses only itself.
        let (parsed, outcome) = Settings::parse(r#"{"icon_style":"neon","low_resource_mode":true}"#);
        assert!(matches!(outcome, LoadOutcome::Loaded));
        assert_eq!(parsed.icon_style, IconStyle::Plain);
        assert!(parsed.low_resource_mode);
    }

    #[test]
    fn the_launchers_screen_round_trips_and_an_unknown_one_is_the_default() {
        let mut s = Settings::default();
        assert_eq!(s.launcher.screen, LauncherScreen::Active);
        s.launcher.screen = LauncherScreen::Specific { id: r"\\?\DISPLAY#GSM5B7F#5&1&UID4352#{e6f07b5f}".into(), name: "VG259".into() };
        let (parsed, _) = Settings::parse(&s.to_json());
        assert_eq!(parsed.launcher.screen, s.launcher.screen);
        assert!(s.to_json().contains(r#""specific": {"#), "{}", s.to_json());

        let (parsed, outcome) = Settings::parse(r#"{"launcher":{"screen":"pointer"}}"#);
        assert!(matches!(outcome, LoadOutcome::Loaded));
        assert_eq!(parsed.launcher.screen, LauncherScreen::Pointer);
        let (parsed, _) = Settings::parse(r#"{"launcher":{"screen":"main"}}"#);
        assert_eq!(parsed.launcher.screen, LauncherScreen::Main);
        // A choice from a later version loses only itself, never the rest of the file.
        let (parsed, outcome) = Settings::parse(r#"{"launcher":{"screen":"largest","preserve_query":true}}"#);
        assert!(matches!(outcome, LoadOutcome::Loaded));
        assert_eq!(parsed.launcher.screen, LauncherScreen::Active);
        assert!(parsed.launcher.preserve_query);
    }

    #[test]
    fn corrupt_files_fall_back_to_defaults() {
        let (s, outcome) = Settings::parse("{ not json");
        assert!(matches!(outcome, LoadOutcome::Corrupt(_)));
        assert_eq!(s, Settings::default());
    }

    #[test]
    fn newer_files_are_flagged() {
        let (_, outcome) = Settings::parse(r#"{"schema_version":99}"#);
        assert!(matches!(outcome, LoadOutcome::NewerVersion(99)));
    }

    #[test]
    fn reports_duplicate_hyper_keys() {
        let mut s = Settings::default();
        let action = Action::Url { url: "https://a".into() };
        for key in ["c", "C", "V"] {
            s.hyper.bindings.push(HyperBinding { key: key.into(), action: action.clone(), enabled: true });
        }
        assert_eq!(s.duplicate_hyper_keys(), vec!["C".to_string()]);
    }

    #[test]
    fn excluded_app_match_ignores_case() {
        let list = vec!["KeePass.exe".to_string()];
        assert!(Settings::is_app_excluded(&list, "keepass.EXE"));
        assert!(!Settings::is_app_excluded(&list, "notepad.exe"));
    }
}
