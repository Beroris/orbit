//! Planning for `hidutil` key remapping. Pure text in, commands out, so it is tested against
//! real `hidutil property --get UserKeyMapping` output without touching any keyboard.
//!
//! `UserKeyMapping` replaces a service's whole list, and other tools keep entries there too, so
//! every write keeps what is already present. `--matching` can't address one service: it has no
//! RegistryID filter, so a write reaches every keyboard at once. Orbit therefore only writes when
//! all keyboards hold the same list, refuses to remap a key another tool already remapped, and on
//! removal takes out only its own pair.

use serde::Serialize;

pub const USAGE_CAPS_LOCK: u64 = 0x7_0000_0039;
pub const USAGE_F18: u64 = 0x7_0000_006D;
/// Keyboards only. Without a matcher hidutil would write to every HID service in the machine.
pub const KEYBOARD_MATCHING: &str = r#"{"PrimaryUsagePage":1,"PrimaryUsage":6}"#;

pub type Mapping = (u64, u64);

/// Reads `hidutil property --matching <keyboards> --get UserKeyMapping` output: one list per
/// service, in order. `(null)` and an empty list both mean no entries.
pub fn parse_services(output: &str) -> Vec<Vec<Mapping>> {
    let mut services: Vec<Vec<Mapping>> = Vec::new();
    let (mut src, mut dst) = (None, None);
    for line in output.lines() {
        let trimmed = line.trim();
        if let Some(rest) = trimmed.strip_prefix("HIDKeyboardModifierMappingSrc") {
            src = number(rest);
        } else if let Some(rest) = trimmed.strip_prefix("HIDKeyboardModifierMappingDst") {
            dst = number(rest);
        } else if trimmed.starts_with('}') {
            if let (Some(s), Some(d), Some(list)) = (src, dst, services.last_mut()) {
                list.push((s, d));
            }
            (src, dst) = (None, None);
        } else if trimmed.contains("UserKeyMapping") && !line.starts_with(char::is_whitespace) {
            services.push(Vec::new());
        }
    }
    services
}

fn number(rest: &str) -> Option<u64> {
    let text = rest.trim().trim_start_matches('=').trim().trim_end_matches(';').trim();
    match text.strip_prefix("0x") {
        Some(hex) => u64::from_str_radix(hex, 16).ok(),
        None => text.parse().ok(),
    }
}

fn mapping_json(entries: &[Mapping]) -> String {
    let parts: Vec<String> = entries
        .iter()
        .map(|(s, d)| format!(r#"{{"HIDKeyboardModifierMappingSrc":{s:#x},"HIDKeyboardModifierMappingDst":{d:#x}}}"#))
        .collect();
    format!(r#"{{"UserKeyMapping":[{}]}}"#, parts.join(","))
}

#[derive(Clone, Debug, Serialize, PartialEq, Eq)]
pub struct Command {
    pub matching: String,
    pub set: String,
}

#[derive(Clone, Debug, Serialize, PartialEq, Eq)]
pub struct Plan {
    /// Why nothing should be done, in words for the user. When set, `commands` is empty.
    pub conflict: Option<String>,
    pub commands: Vec<Command>,
}

fn refused(message: String) -> Plan {
    Plan { conflict: Some(message), commands: Vec::new() }
}

/// A name for a mapped-to key, for messages.
fn usage_name(usage: u64) -> String {
    match usage {
        0x7_0000_0068..=0x7_0000_0073 => format!("F{}", usage - 0x7_0000_0068 + 13),
        0x7_0000_0029 => "Escape".into(),
        _ => format!("key {usage:#x}"),
    }
}

/// The one list all services share, ignoring Orbit's pair. `None` if they disagree.
fn shared_list(services: &[Vec<Mapping>], own: Mapping) -> Option<Vec<Mapping>> {
    let mut lists = services.iter().map(|list| list.iter().copied().filter(|e| *e != own).collect::<Vec<_>>());
    let first = lists.next().unwrap_or_default();
    lists.all(|list| list == first).then_some(first)
}

/// `current` must be read with the same `matching`: normally `KEYBOARD_MATCHING`, or a matcher for
/// no device when the self-test exercises the path without touching a keyboard.
pub fn plan_apply(current: &str, src: u64, dst: u64, matching: &str) -> Plan {
    let services = parse_services(current);
    let own = (src, dst);
    for list in &services {
        if let Some((_, other)) = list.iter().find(|(s, d)| *s == src && *d != dst) {
            let name = usage_name(*other);
            return refused(format!(
                "Caps Lock is already remapped to {name} by another tool, so Orbit left it alone. Choose {name} as the HyperKey, or remove the other mapping first."
            ));
        }
    }
    let Some(mut list) = shared_list(&services, own) else {
        return refused("Your keyboards have different key mappings from another tool, and hidutil can only set them all together, so Orbit left them alone. Choose an F-key as the HyperKey instead.".to_string());
    };
    list.push(own);
    Plan { conflict: None, commands: vec![Command { matching: matching.to_string(), set: mapping_json(&list) }] }
}

/// Removes exactly Orbit's pair, and writes nothing if it isn't there. Safe to run repeatedly,
/// which is what crash recovery does at the next start.
pub fn plan_remove(current: &str, src: u64, dst: u64, matching: &str) -> Plan {
    let services = parse_services(current);
    let own = (src, dst);
    if !services.iter().any(|list| list.contains(&own)) {
        return Plan { conflict: None, commands: Vec::new() };
    }
    let Some(list) = shared_list(&services, own) else {
        return refused("Orbit's Caps Lock mapping is on a keyboard whose other mappings differ, so it wasn't removed automatically. Restarting the Mac clears it.".to_string());
    };
    Plan { conflict: None, commands: vec![Command { matching: matching.to_string(), set: mapping_json(&list) }] }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Real output from the development machine, where another tool maps Caps Lock to F20.
    const REAL: &str = "RegistryID  Key                   Value\n100000ca5   UserKeyMapping   (\n        {\n        HIDKeyboardModifierMappingDst = 30064771183;\n        HIDKeyboardModifierMappingSrc = 30064771129;\n    }\n)\n";
    /// Real output from the same machine after something wrote an empty list.
    const EMPTY: &str = "RegistryID  Key                   Value\n100000ca5   UserKeyMapping   (\n)\n";
    const OWN_AND_OTHER: &str = "RegistryID  Key                   Value\n1000000aa   UserKeyMapping   (\n        {\n        HIDKeyboardModifierMappingDst = 7;\n        HIDKeyboardModifierMappingSrc = 8;\n    },\n        {\n        HIDKeyboardModifierMappingDst = 30064771181;\n        HIDKeyboardModifierMappingSrc = 30064771129;\n    }\n)\n";
    const DIFFERING: &str = "1000000aa   UserKeyMapping   (\n        {\n        HIDKeyboardModifierMappingDst = 7;\n        HIDKeyboardModifierMappingSrc = 8;\n    }\n)\n1000000bb   UserKeyMapping   (null)\n";
    const M: &str = KEYBOARD_MATCHING;

    #[test]
    fn reads_real_hidutil_output() {
        assert_eq!(parse_services(REAL), vec![vec![(USAGE_CAPS_LOCK, 0x7_0000_006F)]]);
        assert_eq!(parse_services(EMPTY), vec![Vec::<Mapping>::new()]);
        assert_eq!(parse_services(DIFFERING), vec![vec![(8, 7)], vec![]]);
        assert_eq!(parse_services(""), Vec::<Vec<Mapping>>::new());
    }

    #[test]
    fn refuses_to_fight_another_remapper() {
        let plan = plan_apply(REAL, USAGE_CAPS_LOCK, USAGE_F18, M);
        assert!(plan.commands.is_empty());
        assert!(plan.conflict.unwrap().contains("F20"));
    }

    #[test]
    fn apply_targets_keyboards_and_keeps_other_entries() {
        let current = "1000000aa   UserKeyMapping   (\n        {\n        HIDKeyboardModifierMappingDst = 1;\n        HIDKeyboardModifierMappingSrc = 2;\n    }\n)\n";
        let plan = plan_apply(current, USAGE_CAPS_LOCK, USAGE_F18, M);
        assert_eq!(plan.conflict, None);
        assert_eq!(plan.commands.len(), 1);
        assert_eq!(plan.commands[0].matching, M);
        let set = &plan.commands[0].set;
        assert!(set.contains("0x700000039") && set.contains("0x70000006d") && set.contains(":0x2,"), "{set}");
    }

    #[test]
    fn apply_to_an_empty_list_writes_just_the_pair() {
        let plan = plan_apply(EMPTY, USAGE_CAPS_LOCK, USAGE_F18, M);
        assert_eq!(plan.commands[0].set, r#"{"UserKeyMapping":[{"HIDKeyboardModifierMappingSrc":0x700000039,"HIDKeyboardModifierMappingDst":0x70000006d}]}"#);
    }

    #[test]
    fn apply_is_idempotent_after_a_crash() {
        let plan = plan_apply(OWN_AND_OTHER, USAGE_CAPS_LOCK, USAGE_F18, M);
        assert_eq!(plan.conflict, None);
        assert_eq!(plan.commands[0].set.matches("0x700000039").count(), 1);
    }

    #[test]
    fn differing_keyboards_are_left_alone() {
        assert!(plan_apply(DIFFERING, USAGE_CAPS_LOCK, USAGE_F18, M).conflict.is_some());
    }

    #[test]
    fn removal_takes_only_orbits_pair() {
        let plan = plan_remove(OWN_AND_OTHER, USAGE_CAPS_LOCK, USAGE_F18, M);
        assert_eq!(plan.commands.len(), 1);
        assert!(plan.commands[0].set.contains(":0x8,"), "other entry kept");
        assert!(!plan.commands[0].set.contains("0x700000039"), "Orbit's pair removed");
    }

    #[test]
    fn removal_leaves_someone_elses_caps_mapping_alone() {
        assert!(plan_remove(REAL, USAGE_CAPS_LOCK, USAGE_F18, M).commands.is_empty());
        assert!(plan_remove(EMPTY, USAGE_CAPS_LOCK, USAGE_F18, M).commands.is_empty());
    }
}
