//! The settings window's data, flat and text-heavy. The port of `collect()` from
//! `platforms/windows/src/settings_win.rs` lives here so its validation can be unit tested and
//! the Swift window holds no rules. The `bindings_text` tables are reused unchanged, except that
//! chords print with "Cmd" because the shared formatter is hard-wired to print "Win".

use orbit_core::bindings_text::*;
use orbit_core::settings::{Appearance, ShortcutBinding};
use orbit_core::Settings;
use orbit_hotkeys::{Chord, HyperMode, Mods, TapBehavior};
use serde::{Deserialize, Serialize};

pub const TRIGGERS: &[&str] = &["CapsLock", "F13", "F14", "F15", "F16", "F17", "F18", "F19", "F20", "RightCtrl", "RightAlt"];

/// Chords macOS keeps for itself. Advisory: `RegisterEventHotKey` and the system's symbolic
/// hotkey table are the real answer, and the host checks both.
pub fn is_reserved_on_macos(chord: &Chord) -> Option<&'static str> {
    let m = chord.mods;
    let key = chord.key.as_str();
    if m == Mods::SUPER {
        return match key {
            "Space" => Some("Spotlight or the input source switcher"),
            "Tab" => Some("the app switcher"),
            "Q" => Some("Quit in every app"),
            "H" | "M" | "W" => Some("standard window commands"),
            "C" | "V" | "X" | "Z" | "A" | "S" => Some("standard editing commands"),
            _ => None,
        };
    }
    if m == Mods::SUPER.union(Mods::SHIFT) && matches!(key, "3" | "4" | "5") {
        return Some("screenshots");
    }
    if m == Mods::SUPER.union(Mods::CTRL) && key == "Q" {
        return Some("Lock Screen");
    }
    None
}

pub fn format_shortcuts_mac(bindings: &[ShortcutBinding]) -> String {
    let mut out = String::new();
    for binding in bindings {
        let prefix = if binding.enabled { "" } else { "# " };
        out.push_str(&format!("{prefix}{} = {}\n", binding.chord.display("Cmd"), format_action(&binding.action)));
    }
    out
}

#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct SettingsForm {
    pub hotkey: String,
    pub launch_at_login: bool,
    pub focus_running_app: bool,
    pub preserve_query: bool,
    pub low_resource_mode: bool,
    pub appearance: String,
    pub shortcuts: String,
    pub hyper_enabled: bool,
    pub hyper_trigger: String,
    pub hyper_mode: String,
    pub hyper_tap: String,
    pub hyper_timeout: String,
    pub hyper_pass_games: bool,
    pub hyper_table: String,
    pub hyper_excluded: String,
    pub clip_enabled: bool,
    pub clip_persist: bool,
    pub clip_images: bool,
    pub clip_max: String,
    pub clip_days: String,
    pub clip_excluded: String,
    pub expansion: bool,
    pub snippet_excluded: String,
    pub links: String,
    pub commands: String,
}

fn mode_name(mode: HyperMode) -> &'static str {
    match mode {
        HyperMode::Internal => "internal",
        HyperMode::Synthetic => "synthetic",
        HyperMode::External => "external",
    }
}

fn tap_name(tap: TapBehavior) -> &'static str {
    match tap {
        TapBehavior::Escape => "escape",
        TapBehavior::Original => "original",
        TapBehavior::None => "none",
        TapBehavior::Leader => "leader",
    }
}

fn appearance_name(appearance: Appearance) -> &'static str {
    match appearance {
        Appearance::System => "system",
        Appearance::Light => "light",
        Appearance::Dark => "dark",
    }
}

impl SettingsForm {
    pub fn from_settings(s: &Settings) -> SettingsForm {
        SettingsForm {
            hotkey: s.launcher.hotkey.as_ref().map(|c| c.display("Cmd")).unwrap_or_default(),
            launch_at_login: s.launch_at_login,
            focus_running_app: s.launcher.focus_running_app,
            preserve_query: s.launcher.preserve_query,
            low_resource_mode: s.low_resource_mode,
            appearance: appearance_name(s.appearance).into(),
            shortcuts: format_shortcuts_mac(&s.shortcuts),
            hyper_enabled: s.hyper.enabled,
            hyper_trigger: s.hyper.trigger.clone(),
            hyper_mode: mode_name(s.hyper.mode).into(),
            hyper_tap: tap_name(s.hyper.tap).into(),
            hyper_timeout: s.hyper.tap_timeout_ms.to_string(),
            hyper_pass_games: s.hyper.pass_through_in_games,
            hyper_table: format_hyper(&s.hyper.bindings),
            hyper_excluded: format_list(&s.hyper.excluded_apps),
            clip_enabled: s.clipboard.enabled,
            clip_persist: s.clipboard.persist,
            clip_images: s.clipboard.capture_images,
            clip_max: s.clipboard.max_items.to_string(),
            clip_days: s.clipboard.retention_days.to_string(),
            clip_excluded: format_list(&s.clipboard.excluded_apps),
            expansion: s.snippets.expansion_enabled,
            snippet_excluded: format_list(&s.snippets.excluded_apps),
            links: format_links(&s.links),
            commands: format_commands(&s.custom_commands),
        }
    }

    /// Builds a new `Settings` on top of `base`. Returns the page and message of the first problem.
    pub fn collect(&self, base: &Settings) -> Result<Settings, (i32, String)> {
        let mut s = base.clone();
        let hotkey = self.hotkey.trim();
        s.launcher.hotkey = if hotkey.is_empty() {
            None
        } else {
            let chord = Chord::parse(hotkey).map_err(|e| (0, format!("Launcher shortcut: {e}")))?;
            Some(chord)
        };
        s.launch_at_login = self.launch_at_login;
        s.launcher.focus_running_app = self.focus_running_app;
        s.launcher.preserve_query = self.preserve_query;
        s.low_resource_mode = self.low_resource_mode;
        s.appearance = match self.appearance.as_str() {
            "light" => Appearance::Light,
            "dark" => Appearance::Dark,
            _ => Appearance::System,
        };

        s.shortcuts = parse_shortcuts(&self.shortcuts).map_err(|e| (1, format!("Shortcuts. {}", e.replace("Win+", "Cmd+"))))?;
        for binding in &s.shortcuts {
            let text = binding.chord.display("Cmd");
            if let Some(owner) = is_reserved_on_macos(&binding.chord) {
                return Err((1, format!("{text} is reserved by macOS for {owner}.")));
            }
            if s.launcher.hotkey.as_ref() == Some(&binding.chord) {
                return Err((1, format!("{text} is already the launcher shortcut.")));
            }
        }

        s.hyper.enabled = self.hyper_enabled;
        if !TRIGGERS.iter().any(|t| t.eq_ignore_ascii_case(&self.hyper_trigger)) {
            return Err((2, format!("“{}” can't be the HyperKey on macOS.", self.hyper_trigger)));
        }
        s.hyper.trigger = self.hyper_trigger.clone();
        s.hyper.mode = match self.hyper_mode.as_str() {
            "synthetic" => HyperMode::Synthetic,
            "external" => HyperMode::External,
            _ => HyperMode::Internal,
        };
        s.hyper.tap = match self.hyper_tap.as_str() {
            "original" => TapBehavior::Original,
            "none" => TapBehavior::None,
            "leader" => TapBehavior::Leader,
            _ => TapBehavior::Escape,
        };
        s.hyper.tap_timeout_ms = self
            .hyper_timeout
            .trim()
            .parse::<u32>()
            .ok()
            .filter(|ms| (50..=2000).contains(ms))
            .ok_or_else(|| (2, "Tap timeout must be between 50 and 2000 milliseconds.".to_string()))?;
        s.hyper.pass_through_in_games = self.hyper_pass_games;
        s.hyper.bindings = parse_hyper(&self.hyper_table).map_err(|e| (2, format!("HyperKey. {e}")))?;
        s.hyper.excluded_apps = parse_list(&self.hyper_excluded);

        s.clipboard.enabled = self.clip_enabled;
        s.clipboard.persist = self.clip_persist;
        s.clipboard.capture_images = self.clip_images;
        s.clipboard.max_items = self
            .clip_max
            .trim()
            .parse::<u32>()
            .ok()
            .filter(|n| (1..=100_000).contains(n))
            .ok_or_else(|| (3, "Keep between 1 and 100000 clipboard items.".to_string()))?;
        s.clipboard.retention_days = self
            .clip_days
            .trim()
            .parse::<u32>()
            .map_err(|_| (3, "Retention must be a number of days. Use 0 to keep items regardless of age.".to_string()))?;
        s.clipboard.excluded_apps = parse_list(&self.clip_excluded);

        s.snippets.expansion_enabled = self.expansion;
        s.snippets.excluded_apps = parse_list(&self.snippet_excluded);

        s.links = parse_links(&self.links, &base.links).map_err(|e| (5, format!("Links. {e}")))?;
        s.custom_commands = parse_commands(&self.commands, &base.custom_commands).map_err(|e| (5, format!("Commands. {e}")))?;
        Ok(s)
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct ChordView {
    pub mods: u8,
    pub key: String,
    pub display: String,
}

impl ChordView {
    pub fn new(chord: &Chord) -> ChordView {
        ChordView { mods: chord.mods.0, key: chord.key.clone(), display: chord.display("Cmd") }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct BindingView {
    pub key: String,
    pub action: String,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct ShortcutView {
    pub chord: ChordView,
    pub action: String,
}

/// What the running host needs to read. Actions travel in their `bindings_text` form.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct HostView {
    pub hotkey: Option<ChordView>,
    pub reserved_hotkey_owner: Option<String>,
    pub preserve_query: bool,
    pub max_visible_rows: u8,
    pub focus_running_app: bool,
    pub hyper_enabled: bool,
    pub hyper_trigger: String,
    pub hyper_mode: String,
    pub hyper_tap: String,
    pub hyper_timeout_ms: u32,
    pub hyper_bindings: Vec<BindingView>,
    pub hyper_excluded: Vec<String>,
    pub hyper_pass_games: bool,
    pub hyper_duplicates: Vec<String>,
    pub shortcuts: Vec<ShortcutView>,
    pub clip_enabled: bool,
    pub clip_persist: bool,
    pub clip_images: bool,
    pub clip_max_items: u32,
    pub clip_retention_days: u32,
    pub clip_max_item_bytes: u32,
    pub clip_excluded: Vec<String>,
    pub expansion_enabled: bool,
    pub expansion_excluded: Vec<String>,
    pub appearance: String,
    pub low_resource_mode: bool,
    pub launch_at_login: bool,
    pub onboarded: bool,
    pub read_only: bool,
    pub form: SettingsForm,
}

impl HostView {
    pub fn new(s: &Settings, read_only: bool) -> HostView {
        HostView {
            hotkey: s.launcher.hotkey.as_ref().map(ChordView::new),
            reserved_hotkey_owner: s.launcher.hotkey.as_ref().and_then(is_reserved_on_macos).map(str::to_string),
            preserve_query: s.launcher.preserve_query,
            max_visible_rows: s.launcher.max_visible_rows.clamp(3, 12),
            focus_running_app: s.launcher.focus_running_app,
            hyper_enabled: s.hyper.enabled,
            hyper_trigger: s.hyper.trigger.clone(),
            hyper_mode: mode_name(s.hyper.mode).into(),
            hyper_tap: tap_name(s.hyper.tap).into(),
            hyper_timeout_ms: s.hyper.tap_timeout_ms,
            hyper_bindings: s
                .hyper
                .bindings
                .iter()
                .filter(|b| b.enabled)
                .map(|b| BindingView { key: b.key.clone(), action: format_action(&b.action) })
                .collect(),
            hyper_excluded: s.hyper.excluded_apps.clone(),
            hyper_pass_games: s.hyper.pass_through_in_games,
            hyper_duplicates: s.duplicate_hyper_keys(),
            shortcuts: s
                .shortcuts
                .iter()
                .filter(|b| b.enabled)
                .map(|b| ShortcutView { chord: ChordView::new(&b.chord), action: format_action(&b.action) })
                .collect(),
            clip_enabled: s.clipboard.enabled,
            clip_persist: s.clipboard.persist,
            clip_images: s.clipboard.capture_images,
            clip_max_items: s.clipboard.max_items,
            clip_retention_days: s.clipboard.retention_days,
            clip_max_item_bytes: s.clipboard.max_item_bytes,
            clip_excluded: s.clipboard.excluded_apps.clone(),
            expansion_enabled: s.snippets.expansion_enabled,
            expansion_excluded: s.snippets.excluded_apps.clone(),
            appearance: appearance_name(s.appearance).into(),
            low_resource_mode: s.low_resource_mode,
            launch_at_login: s.launch_at_login,
            onboarded: s.onboarded,
            read_only,
            form: SettingsForm::from_settings(s),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn untouched_form_round_trips_to_the_same_settings() {
        let mut s = Settings::default();
        s.launcher.hotkey = Some(Chord::parse("Ctrl+Option+Space").unwrap());
        s.shortcuts = parse_shortcuts("Cmd+Shift+K = cmd:clipboard\nCtrl+Alt+F13 = url:https://example.com").unwrap();
        s.hyper.bindings = parse_hyper("C = cmd:clipboard\nB = app:Safari").unwrap();
        let form = SettingsForm::from_settings(&s);
        assert!(form.shortcuts.contains("Shift+Cmd+K"), "prints Cmd, not Win: {}", form.shortcuts);
        assert_eq!(form.collect(&s).unwrap(), s);
    }

    #[test]
    fn errors_name_the_page() {
        let s = Settings::default();
        let good = SettingsForm::from_settings(&s);
        let check = |change: &dyn Fn(&mut SettingsForm), page: i32, needle: &str| {
            let mut form = good.clone();
            change(&mut form);
            let (got, message) = form.collect(&s).unwrap_err();
            assert_eq!(got, page, "{message}");
            assert!(message.contains(needle), "{message}");
        };
        check(&|f| f.hotkey = "Ctrl+Nope".into(), 0, "unknown key");
        check(&|f| f.shortcuts = "Cmd+Space = cmd:help".into(), 1, "reserved by macOS");
        check(&|f| f.shortcuts = "Ctrl+K = cmd:help\nctrl+k = cmd:notes".into(), 1, "already bound");
        check(&|f| f.hyper_timeout = "1".into(), 2, "Tap timeout");
        check(&|f| f.hyper_trigger = "Nope".into(), 2, "can't be the HyperKey");
        check(&|f| f.hyper_table = "C = cmd:bogus".into(), 2, "HyperKey.");
        check(&|f| f.clip_max = "0".into(), 3, "between 1 and");
        check(&|f| f.clip_days = "x".into(), 3, "Retention");
        check(&|f| f.links = "Bad | b | javascript:alert(1)".into(), 5, "blocked");
    }

    #[test]
    fn launcher_chord_cannot_double_as_a_shortcut() {
        let s = Settings::default();
        let mut form = SettingsForm::from_settings(&s);
        form.hotkey = "Ctrl+Alt+K".into();
        form.shortcuts = "Ctrl+Alt+K = cmd:help".into();
        assert!(form.collect(&s).unwrap_err().1.contains("already the launcher"));
    }

    #[test]
    fn cmd_space_is_reserved_and_host_view_says_so() {
        let mut s = Settings::default();
        s.launcher.hotkey = Some(Chord::parse("Cmd+Space").unwrap());
        let view = HostView::new(&s, false);
        assert!(view.reserved_hotkey_owner.is_some());
        assert!(is_reserved_on_macos(&Chord::parse("Alt+Space").unwrap()).is_none());
    }

    #[test]
    fn host_view_is_opt_in_by_default() {
        let view = HostView::new(&Settings::default(), false);
        assert!(!view.hyper_enabled && !view.clip_enabled && !view.expansion_enabled && !view.launch_at_login);
        serde_json::to_string(&view).unwrap();
    }
}
