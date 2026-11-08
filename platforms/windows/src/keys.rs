//! Logical key names to Windows virtual-key codes, and chord registration values.

use orbit_hotkeys::{Chord, Mods};
use windows::Win32::UI::Input::KeyboardAndMouse::{
    HOT_KEY_MODIFIERS, MOD_ALT, MOD_CONTROL, MOD_NOREPEAT, MOD_SHIFT, MOD_WIN,
};

pub const VK_CAPITAL: u8 = 0x14;
pub const VK_LSHIFT: u8 = 0xA0;
pub const VK_RSHIFT: u8 = 0xA1;
pub const VK_LCONTROL: u8 = 0xA2;
pub const VK_RCONTROL: u8 = 0xA3;
pub const VK_LMENU: u8 = 0xA4;
pub const VK_RMENU: u8 = 0xA5;
pub const VK_LWIN: u8 = 0x5B;
pub const VK_RWIN: u8 = 0x5C;
pub const VK_ESCAPE: u8 = 0x1B;
pub const VK_BACK: u8 = 0x08;

/// All modifier codes that the Hyper machine must let pass.
pub const MODIFIER_CODES: &[u8] = &[
    VK_LSHIFT, VK_RSHIFT, VK_LCONTROL, VK_RCONTROL, VK_LMENU, VK_RMENU, VK_LWIN, VK_RWIN,
    0x10, 0x11, 0x12, // generic Shift, Ctrl, Alt, as reported by some injected input
];

pub fn vk_from_name(name: &str) -> Option<u8> {
    let bytes = name.as_bytes();
    if bytes.len() == 1 {
        let c = bytes[0].to_ascii_uppercase();
        if c.is_ascii_alphanumeric() {
            return Some(c);
        }
        return Some(match c {
            b';' => 0xBA,
            b'=' => 0xBB,
            b',' => 0xBC,
            b'-' => 0xBD,
            b'.' => 0xBE,
            b'/' => 0xBF,
            b'`' => 0xC0,
            b'[' => 0xDB,
            b'\\' => 0xDC,
            b']' => 0xDD,
            b'\'' => 0xDE,
            _ => return None,
        });
    }
    if let Some(n) = name.strip_prefix('F').and_then(|n| n.parse::<u8>().ok()) {
        if (1..=24).contains(&n) {
            return Some(0x70 + n - 1);
        }
    }
    Some(match name {
        "Space" => 0x20,
        "Enter" => 0x0D,
        "Tab" => 0x09,
        "Escape" => VK_ESCAPE,
        "Backspace" => VK_BACK,
        "Delete" => 0x2E,
        "Insert" => 0x2D,
        "Home" => 0x24,
        "End" => 0x23,
        "PageUp" => 0x21,
        "PageDown" => 0x22,
        "Up" => 0x26,
        "Down" => 0x28,
        "Left" => 0x25,
        "Right" => 0x27,
        "CapsLock" => VK_CAPITAL,
        "Menu" => 0x5D,
        "RightAlt" => VK_RMENU,
        "RightCtrl" => VK_RCONTROL,
        "RightShift" => VK_RSHIFT,
        "RightSuper" => VK_RWIN,
        "ScrollLock" => 0x91,
        "Pause" => 0x13,
        _ => return None,
    })
}

/// The reverse of `vk_from_name`, for the shortcut recorder.
pub fn name_from_vk(vk: u8) -> Option<String> {
    let c = vk as char;
    if c.is_ascii_uppercase() || c.is_ascii_digit() {
        return Some(c.to_string());
    }
    if (0x70..=0x87).contains(&vk) {
        return Some(format!("F{}", vk - 0x70 + 1));
    }
    Some(
        match vk {
            0x20 => "Space",
            0x0D => "Enter",
            0x09 => "Tab",
            0x1B => "Escape",
            0x08 => "Backspace",
            0x2E => "Delete",
            0x2D => "Insert",
            0x24 => "Home",
            0x23 => "End",
            0x21 => "PageUp",
            0x22 => "PageDown",
            0x26 => "Up",
            0x28 => "Down",
            0x25 => "Left",
            0x27 => "Right",
            0xBA => ";",
            0xBB => "=",
            0xBC => ",",
            0xBD => "-",
            0xBE => ".",
            0xBF => "/",
            0xC0 => "`",
            0xDB => "[",
            0xDC => "\\",
            0xDD => "]",
            0xDE => "'",
            _ => return None,
        }
        .to_string(),
    )
}

pub fn chord_to_registration(chord: &Chord) -> Option<(HOT_KEY_MODIFIERS, u32)> {
    let vk = vk_from_name(&chord.key)?;
    let mut mods = MOD_NOREPEAT;
    if chord.mods.contains(Mods::CTRL) {
        mods |= MOD_CONTROL;
    }
    if chord.mods.contains(Mods::ALT) {
        mods |= MOD_ALT;
    }
    if chord.mods.contains(Mods::SHIFT) {
        mods |= MOD_SHIFT;
    }
    if chord.mods.contains(Mods::SUPER) {
        mods |= MOD_WIN;
    }
    Some((mods, vk as u32))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn name_and_vk_round_trip() {
        for name in ["A", "Z", "0", "9", "F1", "F13", "F24", "Space", "Enter", "PageUp", ";", "/", "]"] {
            let vk = vk_from_name(name).unwrap();
            assert_eq!(name_from_vk(vk).as_deref(), Some(name));
        }
        assert_eq!(vk_from_name("CapsLock"), Some(VK_CAPITAL));
        assert_eq!(vk_from_name("Nope"), None);
    }

    #[test]
    fn every_canonical_hyper_trigger_maps() {
        for name in ["CapsLock", "Menu", "RightAlt", "RightCtrl", "RightShift", "RightSuper", "ScrollLock", "Pause", "F13"] {
            let canonical = orbit_hotkeys::chord::canonical_key(name).unwrap();
            assert!(vk_from_name(&canonical).is_some(), "{name}");
        }
    }

    #[test]
    fn registration_never_autorepeats() {
        let (mods, vk) = chord_to_registration(&Chord::parse("Ctrl+Alt+Space").unwrap()).unwrap();
        assert_eq!(vk, 0x20);
        assert_eq!(mods, MOD_NOREPEAT | MOD_CONTROL | MOD_ALT);
    }
}
