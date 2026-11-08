//! Text form of a keyboard shortcut, e.g. `Ctrl+Alt+Space`.
//!
//! Keys are stored by logical name ("C", "Space", "F5", "CapsLock"); each host maps
//! those names to its own key codes. `Cmd` and `Win` are both the OS "super" modifier
//! in storage, but hosts display the platform's own name.

use serde::{Deserialize, Deserializer, Serialize, Serializer};
use std::fmt;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct Mods(pub u8);

impl Mods {
    pub const NONE: Mods = Mods(0);
    pub const CTRL: Mods = Mods(1);
    pub const ALT: Mods = Mods(2);
    pub const SHIFT: Mods = Mods(4);
    pub const SUPER: Mods = Mods(8);
    pub const HYPER: Mods = Mods(1 | 2 | 4 | 8);

    pub const fn contains(self, other: Mods) -> bool {
        self.0 & other.0 == other.0
    }
    pub const fn union(self, other: Mods) -> Mods {
        Mods(self.0 | other.0)
    }
    pub const fn is_empty(self) -> bool {
        self.0 == 0
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct Chord {
    pub mods: Mods,
    pub key: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ChordError {
    Empty,
    NoKey,
    MultipleKeys(String, String),
    UnknownKey(String),
}

impl fmt::Display for ChordError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ChordError::Empty => write!(f, "shortcut is empty"),
            ChordError::NoKey => write!(f, "shortcut has only modifiers, it needs a key"),
            ChordError::MultipleKeys(a, b) => {
                write!(f, "shortcut has two main keys: {a} and {b}")
            }
            ChordError::UnknownKey(k) => write!(f, "unknown key name: {k}"),
        }
    }
}

impl std::error::Error for ChordError {}

/// Canonicalises a key name, or returns `None` if it is not a key Orbit knows.
pub fn canonical_key(name: &str) -> Option<String> {
    let lower = name.trim().to_ascii_lowercase();
    let mut chars = lower.chars();
    if let (Some(c), None) = (chars.next(), chars.next()) {
        if c.is_ascii_alphanumeric() {
            return Some(c.to_ascii_uppercase().to_string());
        }
        return match c {
            ',' | '.' | '/' | ';' | '\'' | '[' | ']' | '\\' | '-' | '=' | '`' => {
                Some(c.to_string())
            }
            _ => None,
        };
    }
    if let Some(n) = lower.strip_prefix('f').and_then(|n| n.parse::<u8>().ok()) {
        if (1..=24).contains(&n) {
            return Some(format!("F{n}"));
        }
    }
    let name = match lower.as_str() {
        "space" | "spacebar" => "Space",
        "enter" | "return" => "Enter",
        "tab" => "Tab",
        "esc" | "escape" => "Escape",
        "backspace" => "Backspace",
        "delete" | "del" => "Delete",
        "insert" | "ins" => "Insert",
        "home" => "Home",
        "end" => "End",
        "pageup" | "pgup" => "PageUp",
        "pagedown" | "pgdn" => "PageDown",
        "up" => "Up",
        "down" => "Down",
        "left" => "Left",
        "right" => "Right",
        "capslock" | "caps" => "CapsLock",
        "menu" | "apps" | "application" => "Menu",
        "rightalt" | "rightoption" | "ralt" => "RightAlt",
        "rightctrl" | "rightcontrol" | "rctrl" => "RightCtrl",
        "rightshift" | "rshift" => "RightShift",
        "rightwin" | "rightcmd" | "rightcommand" | "rwin" => "RightSuper",
        "rightsuper" => "RightSuper",
        "scrolllock" => "ScrollLock",
        "pause" => "Pause",
        _ => return None,
    };
    Some(name.to_string())
}

impl Chord {
    pub fn new(mods: Mods, key: &str) -> Result<Chord, ChordError> {
        let key = canonical_key(key).ok_or_else(|| ChordError::UnknownKey(key.to_string()))?;
        Ok(Chord { mods, key })
    }

    pub fn parse(text: &str) -> Result<Chord, ChordError> {
        if text.trim().is_empty() {
            return Err(ChordError::Empty);
        }
        let mut mods = Mods::NONE;
        let mut key: Option<String> = None;
        for part in text.split('+') {
            let part = part.trim();
            // "Ctrl++" is not supported: the plus key is spelled "=" with Shift.
            match part.to_ascii_lowercase().as_str() {
                "ctrl" | "control" => mods = mods.union(Mods::CTRL),
                "alt" | "option" | "opt" => mods = mods.union(Mods::ALT),
                "shift" => mods = mods.union(Mods::SHIFT),
                "win" | "windows" | "cmd" | "command" | "super" | "meta" => {
                    mods = mods.union(Mods::SUPER)
                }
                "hyper" => mods = mods.union(Mods::HYPER),
                _ => {
                    let canonical = canonical_key(part)
                        .ok_or_else(|| ChordError::UnknownKey(part.to_string()))?;
                    if let Some(existing) = key.take() {
                        return Err(ChordError::MultipleKeys(existing, canonical));
                    }
                    key = Some(canonical);
                }
            }
        }
        match key {
            Some(key) => Ok(Chord { mods, key }),
            None => Err(ChordError::NoKey),
        }
    }

    /// Text form used in config files. `super_name` is "Win" or "Cmd".
    pub fn display(&self, super_name: &str) -> String {
        let mut out = String::new();
        if self.mods == Mods::HYPER {
            out.push_str("Hyper+");
        } else {
            if self.mods.contains(Mods::CTRL) {
                out.push_str("Ctrl+");
            }
            if self.mods.contains(Mods::ALT) {
                out.push_str("Alt+");
            }
            if self.mods.contains(Mods::SHIFT) {
                out.push_str("Shift+");
            }
            if self.mods.contains(Mods::SUPER) {
                out.push_str(super_name);
                out.push('+');
            }
        }
        out.push_str(&self.key);
        out
    }
}

impl fmt::Display for Chord {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.display("Super"))
    }
}

impl Serialize for Chord {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(&self.to_string())
    }
}

impl<'de> Deserialize<'de> for Chord {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Chord, D::Error> {
        let text = String::deserialize(d)?;
        Chord::parse(&text).map_err(serde::de::Error::custom)
    }
}

/// False for a shortcut that would take an ordinary typing key away from every app, such as `A` or
/// `Shift+A`. Those are never registered, wherever they came from. Function keys are fine alone.
pub fn is_usable_global(chord: &Chord) -> bool {
    let more_than_shift = chord.mods.union(Mods::SHIFT) != Mods::SHIFT;
    let function_key = chord.key.strip_prefix('F').is_some_and(|rest| rest.parse::<u8>().is_ok_and(|n| (1..=24).contains(&n)));
    more_than_shift || function_key
}

/// True if `chord` is one the OS reserves and Orbit must not try to take.
/// This list is advisory: the registration result from the OS is the real answer.
pub fn is_reserved_on_windows(chord: &Chord) -> bool {
    let m = chord.mods;
    if m == Mods::CTRL.union(Mods::ALT) && chord.key == "Delete" {
        return true;
    }
    if m == Mods::SUPER && chord.key == "L" {
        return true;
    }
    m == Mods::ALT && matches!(chord.key.as_str(), "Tab" | "F4" | "Escape")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_plain_typing_key_is_never_usable_as_a_global_shortcut() {
        let usable = |text: &str| is_usable_global(&Chord::parse(text).unwrap());
        assert!(!usable("A"));
        assert!(!usable("Shift+A"));
        assert!(!usable("Space"));
        assert!(!usable("Shift+Space"));
        assert!(usable("Alt+Space"));
        assert!(usable("Ctrl+Shift+A"));
        assert!(usable("Win+A"));
        assert!(usable("F13"));
        assert!(usable("Shift+F1"));
    }

    #[test]
    fn parses_and_round_trips() {
        for (input, expected) in [
            ("Alt+Space", "Alt+Space"),
            ("ctrl + alt + k", "Ctrl+Alt+K"),
            ("Hyper+C", "Hyper+C"),
            ("Ctrl+Alt+Shift+Win+F13", "Hyper+F13"),
            ("Shift+F5", "Shift+F5"),
            ("Cmd+Shift+/", "Shift+Super+/"),
        ] {
            let chord = Chord::parse(input).unwrap();
            assert_eq!(chord.to_string(), expected, "from {input}");
            assert_eq!(Chord::parse(&chord.to_string()).unwrap(), chord);
        }
    }

    #[test]
    fn rejects_bad_chords() {
        assert_eq!(Chord::parse(""), Err(ChordError::Empty));
        assert_eq!(Chord::parse("Ctrl+Alt"), Err(ChordError::NoKey));
        assert!(matches!(Chord::parse("Ctrl+Nope"), Err(ChordError::UnknownKey(_))));
        assert!(matches!(Chord::parse("A+B"), Err(ChordError::MultipleKeys(_, _))));
    }

    #[test]
    fn flags_reserved_windows_chords() {
        assert!(is_reserved_on_windows(&Chord::parse("Ctrl+Alt+Delete").unwrap()));
        assert!(is_reserved_on_windows(&Chord::parse("Win+L").unwrap()));
        assert!(!is_reserved_on_windows(&Chord::parse("Alt+Space").unwrap()));
    }
}
