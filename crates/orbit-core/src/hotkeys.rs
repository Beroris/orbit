//! Giving one thing one hotkey, the way the launcher's action menu does it.
//!
//! Settings hold two lists, HyperKey mappings and direct shortcuts. Here they read as a single
//! idea: a hotkey runs an action. An item has at most one hotkey, and a hotkey runs one item, so
//! assigning replaces rather than piling up duplicates.

use crate::action::{Action, Builtin};
use crate::settings::{HyperBinding, Settings, ShortcutBinding};
use orbit_hotkeys::chord::{is_reserved_on_windows, is_usable_global};
use orbit_hotkeys::{Chord, Mods};

/// What keys pressed in a recorder mean. With HyperKey on, a key alone means that key with Hyper.
/// `Err` says what to do instead, for showing right there.
pub fn from_recorded(mods: Mods, key: &str, hyper_on: bool) -> Result<Hotkey, String> {
    let chord = Chord::new(mods, key).map_err(|_| "Orbit can't use that key. Try another.".to_string())?;
    if hyper_on && mods == Mods::NONE && !is_usable_global(&chord) {
        return Ok(Hotkey::Hyper(chord.key));
    }
    if !is_usable_global(&chord) {
        return Err("A shortcut needs Ctrl, Alt or Win, so that typing still works.".to_string());
    }
    if is_reserved_on_windows(&chord) {
        return Err(format!("{} belongs to Windows. Try another.", chord.display("Win")));
    }
    Ok(Hotkey::Direct(chord))
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Hotkey {
    /// The HyperKey held, plus this key.
    Hyper(String),
    /// An ordinary shortcut, registered with the OS.
    Direct(Chord),
}

impl Hotkey {
    /// `super_name` is "Win" or "Cmd".
    pub fn label(&self, super_name: &str) -> String {
        match self {
            Hotkey::Hyper(key) => format!("Hyper+{key}"),
            Hotkey::Direct(chord) => chord.display(super_name),
        }
    }
}

/// Do these run the same thing? An app counts as the same whichever way it's started, and an
/// entry typed by hand has a name but no id.
pub fn same_target(a: &Action, b: &Action) -> bool {
    fn app(action: &Action) -> Option<(&str, &str)> {
        match action {
            Action::FocusOrLaunchApp { id, name } | Action::LaunchApp { id, name } => Some((id, name)),
            _ => None,
        }
    }
    match (app(a), app(b)) {
        (Some((id_a, name_a)), Some((id_b, name_b))) => {
            (!id_a.is_empty() && id_a == id_b) || name_a.eq_ignore_ascii_case(name_b)
        }
        (None, None) => a == b,
        _ => false,
    }
}

/// The hotkey that currently runs this action, if any.
pub fn hotkey_for(settings: &Settings, action: &Action) -> Option<Hotkey> {
    if settings.hyper.enabled {
        if let Some(binding) = settings.hyper.bindings.iter().find(|b| b.enabled && same_target(&b.action, action)) {
            return Some(Hotkey::Hyper(binding.key.clone()));
        }
    }
    settings
        .shortcuts
        .iter()
        .find(|b| b.enabled && same_target(&b.action, action))
        .map(|b| Hotkey::Direct(b.chord.clone()))
}

/// What this hotkey runs at the moment. Turned-off bindings count, because assigning replaces them.
pub fn action_of<'a>(settings: &'a Settings, hotkey: &Hotkey) -> Option<&'a Action> {
    match hotkey {
        Hotkey::Hyper(key) => settings.hyper.bindings.iter().find(|b| b.key.eq_ignore_ascii_case(key)).map(|b| &b.action),
        Hotkey::Direct(chord) => settings.shortcuts.iter().find(|b| b.chord == *chord).map(|b| &b.action),
    }
}

/// Removes every hotkey that runs this action.
pub fn clear(settings: &mut Settings, action: &Action) {
    settings.hyper.bindings.retain(|b| !same_target(&b.action, action));
    settings.shortcuts.retain(|b| !same_target(&b.action, action));
}

/// Whatever this hotkey already runs, for saying so before replacing it.
pub fn in_use_by(settings: &Settings, hotkey: &Hotkey) -> Option<Action> {
    match hotkey {
        Hotkey::Hyper(key) => settings.hyper.bindings.iter().find(|b| b.key.eq_ignore_ascii_case(key)).map(|b| b.action.clone()),
        Hotkey::Direct(chord) => settings.shortcuts.iter().find(|b| b.chord == *chord).map(|b| b.action.clone()),
    }
}

/// Makes `hotkey` run `action`: the item's old hotkey goes, and so does whatever the key ran before.
///
/// An item that already has a hotkey of the same kind keeps its place in the list, so that changing
/// its keys doesn't move it. Anything else goes at the end.
pub fn assign(settings: &mut Settings, hotkey: Hotkey, action: Action) {
    let hyper_at = settings.hyper.bindings.iter().position(|b| same_target(&b.action, &action));
    let direct_at = settings.shortcuts.iter().position(|b| same_target(&b.action, &action));
    clear(settings, &action);
    match hotkey {
        Hotkey::Hyper(key) => {
            let same_key = |b: &HyperBinding| b.key.eq_ignore_ascii_case(&key);
            let at = hyper_at.map(|at| {
                let at = at.min(settings.hyper.bindings.len());
                at - settings.hyper.bindings[..at].iter().filter(|b| same_key(b)).count()
            });
            settings.hyper.bindings.retain(|b| !same_key(b));
            let at = at.unwrap_or(settings.hyper.bindings.len());
            settings.hyper.bindings.insert(at, HyperBinding { key, action, enabled: true });
        }
        Hotkey::Direct(chord) => {
            let at = direct_at.map(|at| {
                let at = at.min(settings.shortcuts.len());
                at - settings.shortcuts[..at].iter().filter(|b| b.chord == chord).count()
            });
            settings.shortcuts.retain(|b| b.chord != chord);
            let at = at.unwrap_or(settings.shortcuts.len());
            settings.shortcuts.insert(at, ShortcutBinding { chord, action, enabled: true });
        }
    }
}

/// Words for an action, for a person reading a list or a question.
pub fn describe(settings: &Settings, action: &Action) -> String {
    match action {
        Action::FocusOrLaunchApp { name, .. } | Action::LaunchApp { name, .. } => name.clone(),
        Action::OpenPath { path } => path.clone(),
        Action::Url { url } => settings
            .links
            .iter()
            .find(|link| link.url == *url)
            .map_or_else(|| url.clone(), |link| link.name.clone()),
        Action::Builtin { command: Builtin::ToggleLauncher } => "Open Orbit".to_string(),
        Action::Builtin { command } => Builtin::ALL
            .iter()
            .find(|entry| entry.0 == *command)
            .map_or_else(|| "Orbit command".to_string(), |entry| entry.2.to_string()),
        Action::Custom { id } => settings
            .custom_commands
            .iter()
            .find(|command| command.id == *id)
            .map_or_else(|| format!("Command {id}"), |command| command.name.clone()),
        Action::OpenNote { id } => format!("Note {id}"),
        Action::InsertSnippet { id } => format!("Snippet {id}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn app(name: &str) -> Action {
        Action::FocusOrLaunchApp { id: format!("id-{name}"), name: name.to_string() }
    }

    fn hyper_on() -> Settings {
        let mut settings = Settings::default();
        settings.hyper.enabled = true;
        settings
    }

    #[test]
    fn an_entry_typed_by_hand_is_the_same_app() {
        let typed = Action::FocusOrLaunchApp { id: String::new(), name: "zen".to_string() };
        assert!(same_target(&typed, &app("Zen")));
        assert!(same_target(&Action::LaunchApp { id: "id-Zen".into(), name: "Other name".into() }, &app("Zen")));
        assert!(!same_target(&typed, &app("Firefox")));
        assert!(!same_target(&typed, &Action::Url { url: "zen".into() }));
    }

    #[test]
    fn empty_ids_never_match_each_other() {
        let a = Action::FocusOrLaunchApp { id: String::new(), name: "A".into() };
        let b = Action::FocusOrLaunchApp { id: String::new(), name: "B".into() };
        assert!(!same_target(&a, &b));
    }

    #[test]
    fn assigning_moves_an_items_hotkey() {
        let mut s = hyper_on();
        assign(&mut s, Hotkey::Hyper("C".into()), app("Zen"));
        assign(&mut s, Hotkey::Hyper("B".into()), app("Zen"));
        assert_eq!(s.hyper.bindings.len(), 1);
        assert_eq!(hotkey_for(&s, &app("Zen")), Some(Hotkey::Hyper("B".into())));

        let chord = Chord::parse("Ctrl+Alt+Z").unwrap();
        assign(&mut s, Hotkey::Direct(chord.clone()), app("Zen"));
        assert!(s.hyper.bindings.is_empty(), "one hotkey per item, across both lists");
        assert_eq!(hotkey_for(&s, &app("Zen")), Some(Hotkey::Direct(chord)));
    }

    #[test]
    fn changing_an_items_keys_keeps_its_place() {
        let mut s = Settings::default();
        assign(&mut s, Hotkey::Hyper("A".into()), app("Zen"));
        assign(&mut s, Hotkey::Hyper("B".into()), app("T3 Code"));
        assign(&mut s, Hotkey::Hyper("C".into()), app("Notes"));
        assign(&mut s, Hotkey::Hyper("D".into()), app("T3 Code"));
        let keys: Vec<&str> = s.hyper.bindings.iter().map(|b| b.key.as_str()).collect();
        assert_eq!(keys, ["A", "D", "C"]);
        // Taking a key from a row above it still lands in the right place.
        assign(&mut s, Hotkey::Hyper("A".into()), app("Notes"));
        let keys: Vec<&str> = s.hyper.bindings.iter().map(|b| b.key.as_str()).collect();
        assert_eq!(keys, ["D", "A"]);
    }

    #[test]
    fn assigning_replaces_what_the_key_ran() {
        let mut s = hyper_on();
        assign(&mut s, Hotkey::Hyper("C".into()), app("Zen"));
        assert_eq!(action_of(&s, &Hotkey::Hyper("c".into())), Some(&app("Zen")));
        assign(&mut s, Hotkey::Hyper("C".into()), app("T3 Code"));
        assert_eq!(s.hyper.bindings.len(), 1);
        assert_eq!(hotkey_for(&s, &app("Zen")), None);
        assert_eq!(hotkey_for(&s, &app("T3 Code")), Some(Hotkey::Hyper("C".into())));
    }

    #[test]
    fn a_turned_off_key_can_be_taken_but_runs_nothing() {
        let mut s = hyper_on();
        s.hyper.bindings.push(HyperBinding { key: "C".into(), action: app("Zen"), enabled: false });
        assert_eq!(hotkey_for(&s, &app("Zen")), None);
        assert_eq!(action_of(&s, &Hotkey::Hyper("C".into())), Some(&app("Zen")));
        assign(&mut s, Hotkey::Hyper("C".into()), app("T3 Code"));
        assert_eq!(s.hyper.bindings.len(), 1);
    }

    #[test]
    fn with_hyperkey_off_its_mappings_are_not_offered() {
        let mut s = hyper_on();
        assign(&mut s, Hotkey::Hyper("C".into()), app("Zen"));
        s.hyper.enabled = false;
        assert_eq!(hotkey_for(&s, &app("Zen")), None);
    }

    #[test]
    fn clearing_removes_it_from_both_lists() {
        let mut s = hyper_on();
        s.hyper.bindings.push(HyperBinding { key: "C".into(), action: app("Zen"), enabled: true });
        s.shortcuts.push(ShortcutBinding { chord: Chord::parse("Ctrl+Alt+Z").unwrap(), action: app("Zen"), enabled: true });
        clear(&mut s, &app("Zen"));
        assert!(s.hyper.bindings.is_empty() && s.shortcuts.is_empty());
    }

    #[test]
    fn descriptions_read_like_names() {
        let s = Settings::default();
        assert_eq!(describe(&s, &app("Zen")), "Zen");
        assert_eq!(describe(&s, &Action::Builtin { command: Builtin::ClipboardHistory }), "Clipboard History");
        assert_eq!(describe(&s, &Action::Builtin { command: Builtin::ToggleLauncher }), "Open Orbit");
        assert_eq!(describe(&s, &Action::Url { url: s.links[0].url.clone() }), "Search Google");
        assert_eq!(describe(&s, &Action::Url { url: "https://example.com".into() }), "https://example.com");
    }

    #[test]
    fn recorded_keys_become_the_right_kind_of_hotkey() {
        let ctrl_alt = Mods::CTRL.union(Mods::ALT);
        assert_eq!(from_recorded(ctrl_alt, "J", false), Ok(Hotkey::Direct(Chord::parse("Ctrl+Alt+J").unwrap())));
        assert_eq!(from_recorded(Mods::NONE, "J", true), Ok(Hotkey::Hyper("J".into())));
        assert!(from_recorded(Mods::NONE, "J", false).is_err(), "a plain key would stop typing everywhere");
        assert!(from_recorded(Mods::SHIFT, "J", true).is_err(), "and so would Shift with one");
        assert!(matches!(from_recorded(Mods::NONE, "F9", false), Ok(Hotkey::Direct(_))), "a function key is fine alone");
        assert!(from_recorded(Mods::SUPER, "L", false).is_err(), "Win+L belongs to Windows");
    }

    #[test]
    fn labels_use_the_platforms_word_for_super() {
        let chord = Hotkey::Direct(Chord::parse("Cmd+Shift+K").unwrap());
        assert_eq!(chord.label("Cmd"), "Shift+Cmd+K");
        assert_eq!(Hotkey::Hyper("Space".into()).label("Win"), "Hyper+Space");
    }
}
