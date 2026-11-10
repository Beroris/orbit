//! Plain-text tables for bindings, links and commands, as edited in settings.
//!
//! A keyboard-first table you can type, paste and diff needs no custom list control, and any
//! host can reuse it. Errors name the line, so settings can point straight at the problem.
//!
//! ```text
//! C = cmd:clipboard
//! B = app:Firefox
//! G = url:https://github.com
//! ```

use crate::action::{Action, Builtin};
use crate::settings::{CustomCommand, HyperBinding, QuickLink, ShortcutBinding};
use orbit_hotkeys::Chord;
use orbit_hotkeys::chord::canonical_key;

pub const LAUNCHER_KEY: &str = "cmd:launcher";

/// Mappings offered by the "add suggested mappings" button. None of them run anything risky.
pub const SUGGESTED_HYPER: &str = "Space = cmd:launcher\nV = cmd:clipboard\nN = cmd:new-note\nS = cmd:snippets\nF = cmd:notes\nT = cmd:terminal\n";

pub fn format_action(action: &Action) -> String {
    match action {
        Action::FocusOrLaunchApp { name, .. } => format!("app:{name}"),
        Action::LaunchApp { name, .. } => format!("start:{name}"),
        Action::OpenPath { path } => format!("path:{path}"),
        Action::Url { url } => format!("url:{url}"),
        Action::Builtin { command: Builtin::ToggleLauncher } => LAUNCHER_KEY.to_string(),
        Action::Builtin { command } => Builtin::ALL
            .iter()
            .find(|entry| entry.0 == *command)
            .map_or_else(|| LAUNCHER_KEY.to_string(), |entry| entry.1.to_string()),
        Action::Custom { id } => format!("custom:{id}"),
        Action::OpenNote { id } => format!("note:{id}"),
        Action::InsertSnippet { id } => format!("snippet:{id}"),
        Action::Extension { ext, command } => format!("ext:{ext}/{command}"),
    }
}

pub fn parse_action(text: &str) -> Result<Action, String> {
    let text = text.trim();
    let Some((kind, value)) = text.split_once(':') else {
        return Err(format!("â{text}â needs a type, such as app:Firefox or cmd:clipboard"));
    };
    let value = value.trim();
    if value.is_empty() {
        return Err(format!("â{text}â is missing what to open"));
    }
    let number = |value: &str| value.parse::<i64>().map_err(|_| format!("â{value}â isn't a number"));
    match kind.trim().to_ascii_lowercase().as_str() {
        "app" => Ok(Action::FocusOrLaunchApp { id: String::new(), name: value.to_string() }),
        "start" => Ok(Action::LaunchApp { id: String::new(), name: value.to_string() }),
        "path" => Ok(Action::OpenPath { path: value.to_string() }),
        // A URL keeps its own colon, so take everything after the first one.
        "url" => Ok(Action::Url { url: value.to_string() }),
        "cmd" => {
            let key = format!("cmd:{value}");
            if key == LAUNCHER_KEY {
                return Ok(Action::Builtin { command: Builtin::ToggleLauncher });
            }
            Builtin::from_key(&key)
                .map(|command| Action::Builtin { command })
                .ok_or_else(|| format!("There's no command called â{value}â"))
        }
        "custom" => Ok(Action::Custom { id: value.to_string() }),
        "note" => Ok(Action::OpenNote { id: number(value)? }),
        "snippet" => Ok(Action::InsertSnippet { id: number(value)? }),
        "ext" => match value.split_once('/') {
            Some((ext, command)) if !ext.is_empty() && !command.is_empty() => Ok(Action::Extension { ext: ext.to_string(), command: command.to_string() }),
            _ => Err(format!("“{value}” should be an extension and its command, such as ext:windows-settings/search")),
        },
        other => Err(format!("â{other}:â isn't a type. Use app, start, path, url, cmd, custom, note or snippet")),
    }
}

fn lines(text: &str) -> impl Iterator<Item = (usize, &str)> {
    text.lines()
        .enumerate()
        .map(|(i, line)| (i + 1, line.trim()))
        .filter(|(_, line)| !line.is_empty() && !line.starts_with('#'))
}

/// A binding line, or one turned off with `#`. A `#` line that isn't a binding is just a comment.
fn binding_lines(text: &str) -> impl Iterator<Item = (usize, &str, bool)> {
    text.lines().enumerate().filter_map(|(i, line)| {
        let line = line.trim();
        if line.is_empty() {
            return None;
        }
        match line.strip_prefix('#') {
            Some(rest) if rest.contains('=') => Some((i + 1, rest.trim(), false)),
            Some(_) => None,
            None => Some((i + 1, line, true)),
        }
    })
}

pub fn format_hyper(bindings: &[HyperBinding]) -> String {
    let mut out = String::new();
    for binding in bindings {
        let prefix = if binding.enabled { "" } else { "# " };
        out.push_str(&format!("{prefix}{} = {}\n", binding.key, format_action(&binding.action)));
    }
    out
}

/// Parses `key = action` lines. A key bound twice is an error here, not a silent overwrite.
pub fn parse_hyper(text: &str) -> Result<Vec<HyperBinding>, String> {
    let mut out: Vec<HyperBinding> = Vec::new();
    for (number, line, enabled) in binding_lines(text) {
        let (key, action) = line.split_once('=').ok_or_else(|| format!("Line {number}: use key = action"))?;
        let Some(key) = canonical_key(key.trim()) else {
            if !enabled {
                continue; // a comment that happens to contain "="
            }
            return Err(format!("Line {number}: â{}â isn't a key", key.trim()));
        };
        if out.iter().any(|binding| binding.key == key) {
            return Err(format!("Line {number}: {key} is already bound above"));
        }
        let action = match parse_action(action) {
            Ok(action) => action,
            Err(_) if !enabled => continue,
            Err(e) => return Err(format!("Line {number}: {e}")),
        };
        out.push(HyperBinding { key, action, enabled });
    }
    Ok(out)
}

pub fn format_shortcuts(bindings: &[ShortcutBinding]) -> String {
    let mut out = String::new();
    for binding in bindings {
        let prefix = if binding.enabled { "" } else { "# " };
        out.push_str(&format!("{prefix}{} = {}\n", binding.chord.display("Win"), format_action(&binding.action)));
    }
    out
}

pub fn parse_shortcuts(text: &str) -> Result<Vec<ShortcutBinding>, String> {
    let mut out: Vec<ShortcutBinding> = Vec::new();
    for (number, line, enabled) in binding_lines(text) {
        let (chord, action) = line.split_once('=').ok_or_else(|| format!("Line {number}: use shortcut = action"))?;
        let chord = match Chord::parse(chord.trim()) {
            Ok(chord) => chord,
            Err(_) if !enabled => continue,
            Err(e) => return Err(format!("Line {number}: {e}")),
        };
        if out.iter().any(|binding| binding.chord == chord) {
            return Err(format!("Line {number}: {} is already bound above", chord.display("Win")));
        }
        let action = match parse_action(action) {
            Ok(action) => action,
            Err(_) if !enabled => continue,
            Err(e) => return Err(format!("Line {number}: {e}")),
        };
        out.push(ShortcutBinding { chord, action, enabled });
    }
    Ok(out)
}

/// A stable id from a display name, so aliases and usage survive a settings edit.
fn slug(prefix: &str, name: &str) -> String {
    let mut out = String::from(prefix);
    for c in name.chars() {
        if c.is_alphanumeric() {
            out.extend(c.to_lowercase());
        } else if !out.ends_with('-') {
            out.push('-');
        }
    }
    out
}

pub fn format_links(links: &[QuickLink]) -> String {
    let mut out = String::new();
    for link in links {
        out.push_str(&format!("{} | {} | {}\n", link.name, link.alias, link.url));
    }
    out
}

/// `name | alias | url`. Blocked URL schemes are refused when saving, not when clicked.
/// An existing link with the same name or URL keeps its id, so pins, aliases and usage carry over,
/// and the app that opens it, which this text doesn't show.
pub fn parse_links(text: &str, existing: &[QuickLink]) -> Result<Vec<QuickLink>, String> {
    let mut out: Vec<QuickLink> = Vec::new();
    for (number, line) in lines(text) {
        let parts: Vec<&str> = line.splitn(3, '|').map(str::trim).collect();
        let [name, alias, url] = parts[..] else {
            return Err(format!("Line {number}: use name | alias | url"));
        };
        if name.is_empty() || url.is_empty() {
            return Err(format!("Line {number}: a link needs a name and a URL"));
        }
        let probe = crate::links::link_probe(url);
        let check = if crate::links::is_path(url) { crate::action::UrlCheck::Safe } else { crate::action::check_url(&probe) };
        match check {
            crate::action::UrlCheck::Blocked(scheme) => {
                return Err(format!("Line {number}: {scheme}: links are blocked because they can run code"));
            }
            crate::action::UrlCheck::Invalid => return Err(format!("Line {number}: â{url}â isn't a valid URL")),
            _ => {}
        }
        let same = existing.iter().find(|link| link.name == name).or_else(|| existing.iter().find(|link| link.url == url));
        let id = same.map_or_else(|| slug("link-", name), |link| link.id.clone());
        let open_with = same.map(|link| link.open_with.clone()).unwrap_or_default();
        if out.iter().any(|link| link.id == id) {
            return Err(format!("Line {number}: â{name}â is already listed"));
        }
        out.push(QuickLink { id, name: name.to_string(), url: url.to_string(), alias: alias.to_string(), open_with });
    }
    Ok(out)
}

pub fn format_commands(commands: &[CustomCommand]) -> String {
    let mut out = String::new();
    for command in commands {
        out.push_str(&format!("{} | {} | {} | {}", command.name, command.alias, command.program, command.arguments));
        if !command.working_dir.is_empty() {
            out.push_str(&format!("{WORKING_DIR_MARK}{}", command.working_dir));
        }
        out.push('\n');
    }
    out
}

/// Ends a command line with its working folder. Arguments may hold plain `|` characters, so a
/// fifth column wouldn't do.
const WORKING_DIR_MARK: &str = " | in: ";

/// `name | alias | program | arguments | in: folder`. Commands run only when chosen, with no shell
/// in between. An existing command with the same name keeps its id.
pub fn parse_commands(text: &str, existing: &[CustomCommand]) -> Result<Vec<CustomCommand>, String> {
    let mut out: Vec<CustomCommand> = Vec::new();
    for (number, line) in lines(text) {
        let (line, working_dir) = line.rsplit_once(WORKING_DIR_MARK).unwrap_or((line, ""));
        let parts: Vec<&str> = line.splitn(4, '|').map(str::trim).collect();
        if parts.len() < 3 {
            return Err(format!("Line {number}: use name | alias | program | arguments"));
        }
        let (name, alias, program) = (parts[0], parts[1], parts[2]);
        if name.is_empty() || program.is_empty() {
            return Err(format!("Line {number}: a command needs a name and a program"));
        }
        let previous = existing.iter().find(|command| command.name == name);
        let id = previous.map_or_else(|| slug("cmd-", name), |command| command.id.clone());
        if out.iter().any(|command: &CustomCommand| command.id == id) {
            return Err(format!("Line {number}: â{name}â is already listed"));
        }
        out.push(CustomCommand {
            id,
            name: name.to_string(),
            program: program.to_string(),
            arguments: parts.get(3).unwrap_or(&"").to_string(),
            working_dir: working_dir.trim().to_string(),
            alias: alias.to_string(),
        });
    }
    Ok(out)
}

/// One entry per line. Blanks are dropped.
pub fn parse_list(text: &str) -> Vec<String> {
    lines(text).map(|(_, line)| line.to_string()).collect()
}

pub fn format_list(items: &[String]) -> String {
    let mut out = items.join("\n");
    if !out.is_empty() {
        out.push('\n');
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_command_keeps_its_working_folder_and_arguments_keep_their_bars() {
        let parsed = parse_commands("Build | b | cargo | build | head -1 | in: C:\\code\\orbit\n", &[]).unwrap();
        assert_eq!(parsed[0].arguments, "build | head -1");
        assert_eq!(parsed[0].working_dir, "C:\\code\\orbit");
        let again = parse_commands(&format_commands(&parsed), &parsed).unwrap();
        assert_eq!(again, parsed);
        // Deleting the folder from the line really removes it.
        let cleared = parse_commands("Build | b | cargo | build\n", &parsed).unwrap();
        assert!(cleared[0].working_dir.is_empty());
    }

    #[test]
    fn actions_round_trip() {
        for text in [
            "app:Firefox",
            "start:Notepad",
            "path:C:\\Users\\me\\file.txt",
            "url:https://example.com:8080/a?b=c",
            "cmd:clipboard",
            "cmd:launcher",
            "custom:deploy",
            "note:12",
            "snippet:7",
        ] {
            let action = parse_action(text).unwrap_or_else(|e| panic!("{text}: {e}"));
            assert_eq!(format_action(&action), text);
        }
    }

    #[test]
    fn bad_actions_explain_themselves() {
        assert!(parse_action("firefox").unwrap_err().contains("needs a type"));
        assert!(parse_action("app:").unwrap_err().contains("missing"));
        assert!(parse_action("cmd:nope").unwrap_err().contains("no command"));
        assert!(parse_action("zzz:1").unwrap_err().contains("isn't a type"));
        assert!(parse_action("note:abc").unwrap_err().contains("number"));
    }

    #[test]
    fn hyper_table_round_trips_and_ignores_comments() {
        let parsed = parse_hyper("# my keys\n\nC = cmd:clipboard\n b = app:Firefox\nspace = cmd:launcher\n").unwrap();
        assert_eq!(parsed.len(), 3);
        assert_eq!(parsed[1].key, "B");
        assert_eq!(parsed[2].key, "Space");
        assert_eq!(parse_hyper(&format_hyper(&parsed)).unwrap(), parsed);
    }

    #[test]
    fn hyper_errors_name_the_line() {
        assert_eq!(parse_hyper("C = cmd:clipboard\nno equals").unwrap_err(), "Line 2: use key = action");
        assert!(parse_hyper("C = cmd:clipboard\n\nc = cmd:notes").unwrap_err().starts_with("Line 3: C is already bound"));
        assert!(parse_hyper("NotAKey = cmd:notes").unwrap_err().contains("isn't a key"));
        assert!(parse_hyper("C = cmd:bogus").unwrap_err().starts_with("Line 1:"));
    }

    #[test]
    fn suggested_mappings_are_valid() {
        let parsed = parse_hyper(SUGGESTED_HYPER).unwrap();
        assert!(parsed.len() >= 4);
        assert!(!parsed.iter().any(|b| matches!(b.action, Action::Url { .. } | Action::Custom { .. })));
    }

    #[test]
    fn shortcuts_round_trip_and_reject_duplicates() {
        let parsed = parse_shortcuts("ctrl+alt+k = cmd:clipboard\nCtrl+Shift+F13 = url:https://example.com").unwrap();
        assert_eq!(parsed[0].chord.display("Win"), "Ctrl+Alt+K");
        assert_eq!(parse_shortcuts(&format_shortcuts(&parsed)).unwrap(), parsed);
        assert!(parse_shortcuts("Ctrl+Alt+K = cmd:notes\nctrl+alt+k = cmd:help").unwrap_err().contains("already bound"));
    }

    #[test]
    fn links_refuse_dangerous_schemes() {
        let parsed = parse_links("Search Google | g | https://www.google.com/search?q={query}", &[]).unwrap();
        assert_eq!(parsed[0].alias, "g");
        assert_eq!(parse_links(&format_links(&parsed), &parsed).unwrap(), parsed);
        assert!(parse_links("Bad | b | javascript:alert(1)", &[]).unwrap_err().contains("blocked"));
        assert!(parse_links("Bad | b | file:///c:/x", &[]).unwrap_err().contains("blocked"));
        assert!(parse_links("missing pipes", &[]).unwrap_err().starts_with("Line 1"));
        assert!(parse_links(" | a | https://example.com", &[]).is_err());
    }

    #[test]
    fn commands_parse_with_or_without_arguments() {
        let parsed = parse_commands("Deploy | dp | C:\\tools\\deploy.exe | --prod --fast\nBuild | | make", &[]).unwrap();
        assert_eq!(parsed[0].arguments, "--prod --fast");
        assert_eq!(parsed[0].id, "cmd-deploy");
        assert_eq!(parsed[1].arguments, "");
        assert_eq!(parse_commands(&format_commands(&parsed), &parsed).unwrap(), parsed);
        assert!(parse_commands("only | two", &[]).is_err());
        assert!(parse_commands("A | a | x\nA | b | y", &[]).unwrap_err().contains("already listed"));
    }

    #[test]
    fn saving_keeps_identity_and_fields_the_table_does_not_show() {
        let links = vec![QuickLink { id: "default-search".into(), name: "Search Google".into(), url: "https://g.example/?q={query}".into(), alias: "g".into(), open_with: "zen".into() }];
        assert_eq!(parse_links(&format_links(&links), &links).unwrap(), links);
        let renamed = parse_links("Web | g | https://g.example/?q={query}", &links).unwrap();
        assert_eq!(renamed[0].id, "default-search", "same URL, new name: still the same link");

        let commands = vec![CustomCommand { id: "x1".into(), name: "Build".into(), program: "make".into(), arguments: "-j8".into(), working_dir: "C:\\src".into(), alias: "b".into() }];
        assert_eq!(parse_commands(&format_commands(&commands), &commands).unwrap(), commands);
    }

    #[test]
    fn disabled_bindings_survive_a_save() {
        let parsed = parse_hyper("C = cmd:clipboard\n# N = cmd:new-note\n# just a comment\n# a = b = c\n").unwrap();
        assert_eq!(parsed.len(), 2);
        assert!(parsed[0].enabled);
        assert_eq!(parsed[1].key, "N");
        assert!(!parsed[1].enabled);
        assert_eq!(parse_hyper(&format_hyper(&parsed)).unwrap(), parsed);

        let shortcuts = parse_shortcuts("# Ctrl+Alt+K = cmd:notes\n# not a binding\n").unwrap();
        assert_eq!(shortcuts.len(), 1);
        assert!(!shortcuts[0].enabled);
        assert_eq!(parse_shortcuts(&format_shortcuts(&shortcuts)).unwrap(), shortcuts);
    }

    #[test]
    fn lists_drop_blanks() {
        assert_eq!(parse_list(" a.exe \n\n# note\nb.exe\n"), ["a.exe", "b.exe"]);
        assert_eq!(format_list(&["a".to_string()]), "a\n");
        assert_eq!(format_list(&[]), "");
    }
}
