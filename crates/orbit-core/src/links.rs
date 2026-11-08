//! Checking a quicklink or command before it's saved, with messages a person can act on.

use crate::action::{UrlCheck, as_openable_url, check_url};
use crate::settings::{CustomCommand, QuickLink};

fn slug(prefix: &str, name: &str) -> String {
    let mut out = String::from(prefix);
    for c in name.chars() {
        if c.is_alphanumeric() {
            out.extend(c.to_lowercase());
        } else if !out.ends_with('-') {
            out.push('-');
        }
    }
    out.trim_end_matches('-').to_string()
}

/// An id no other item has, so pins, aliases and usage stay with the right one.
pub fn fresh_id(prefix: &str, name: &str, taken: impl Fn(&str) -> bool) -> String {
    let base = slug(prefix, name);
    if !taken(&base) {
        return base;
    }
    (2..).map(|n| format!("{base}-{n}")).find(|id| !taken(id)).unwrap_or(base)
}

/// Which box a problem is in, so the form can put the cursor there.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Field {
    Name,
    Target,
    Alias,
}

pub type Problem = (Field, String);

fn check_alias(alias: &str, own_id: &str, links: &[QuickLink], commands: &[CustomCommand]) -> Result<(), Problem> {
    if alias.chars().any(char::is_whitespace) {
        return Err((Field::Alias, "An alias is one word, with no spaces.".to_string()));
    }
    if alias.is_empty() {
        return Ok(());
    }
    let same = |other: &str| other.eq_ignore_ascii_case(alias);
    let other = links
        .iter()
        .find(|link| link.id != own_id && same(&link.alias))
        .map(|link| link.name.as_str())
        .or_else(|| commands.iter().find(|command| command.id != own_id && same(&command.alias)).map(|command| command.name.as_str()));
    match other {
        Some(name) => Err((Field::Alias, format!("“{name}” already uses that alias."))),
        None => Ok(()),
    }
}

/// One blank in a quicklink's address: `{query}`, `{city}`, or `{city=London}` with a default.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LinkParam {
    /// As written between the braces, before any `=`.
    pub name: String,
    /// What its box says: `Search` for `{query}`, and otherwise its name.
    pub label: String,
    /// Used when it's left empty. `None` means it has to be filled in.
    pub default: Option<String>,
}

impl LinkParam {
    pub fn required(&self) -> bool {
        self.default.is_none()
    }
}

/// The longest name a blank can have. Anything longer in braces is part of the address.
const PARAM_NAME_MAX: usize = 40;

/// Every `{…}` in `url` that names a blank, in order: where it is, its name and its default.
/// Braces around anything else, such as `{}` or JSON, stay part of the address.
fn placeholders(url: &str) -> Vec<(std::ops::Range<usize>, &str, Option<&str>)> {
    let mut out = Vec::new();
    let mut from = 0;
    while let Some(open) = url[from..].find('{').map(|at| at + from) {
        let Some(close) = url[open + 1..].find(['{', '}']).map(|at| at + open + 1) else { break };
        if url.as_bytes()[close] == b'{' {
            from = close;
            continue;
        }
        let inner = &url[open + 1..close];
        let (name, default) = match inner.split_once('=') {
            Some((name, default)) => (name.trim(), Some(default)),
            None => (inner.trim(), None),
        };
        let fits = !name.is_empty()
            && name.chars().count() <= PARAM_NAME_MAX
            && name.chars().all(|c| c.is_alphanumeric() || matches!(c, ' ' | '_' | '-' | '.'));
        if fits {
            out.push((open..close + 1, name, default));
        }
        from = close + 1;
    }
    out
}

/// The blanks in a link's address, each once, in the order they first appear.
pub fn link_params(url: &str) -> Vec<LinkParam> {
    let mut out: Vec<LinkParam> = Vec::new();
    for (_, name, default) in placeholders(url) {
        if out.iter().any(|param| param.name.eq_ignore_ascii_case(name)) {
            continue;
        }
        let label = if name.eq_ignore_ascii_case("query") { "Search".to_string() } else { name.to_string() };
        out.push(LinkParam { name: name.to_string(), label, default: default.map(str::to_string) });
    }
    out
}

/// The first blank that has to be filled in and isn't, by its place in [`link_params`].
pub fn missing_param(url: &str, values: &[String]) -> Option<usize> {
    link_params(url)
        .iter()
        .enumerate()
        .find(|(at, param)| param.required() && values.get(*at).is_none_or(|value| value.trim().is_empty()))
        .map(|(at, _)| at)
}

/// The address with every blank filled in: `values` in the order of [`link_params`], or a blank's
/// default where its value is empty. Each value is percent-encoded, so it can't change the shape
/// of the address. A space is `+` in the part after `?`, and `%20` before it.
/// In a file or folder's path, blanks are filled with what was typed, as it is.
pub fn fill_link(url: &str, values: &[String]) -> String {
    if is_path(url) {
        return fill_path(url, values);
    }
    let params = link_params(url);
    let spots = placeholders(url);
    let in_blank = |at: usize| spots.iter().any(|(range, _, _)| range.contains(&at));
    let query_start = url.char_indices().find(|&(at, c)| c == '?' && !in_blank(at)).map(|(at, _)| at);
    let fragment_start = url.char_indices().find(|&(at, c)| c == '#' && !in_blank(at)).map_or(usize::MAX, |(at, _)| at);
    let mut out = String::with_capacity(url.len() + 16);
    let mut last = 0;
    for (range, name, default) in &spots {
        out.push_str(&url[last..range.start]);
        let index = params.iter().position(|param| param.name.eq_ignore_ascii_case(name));
        let typed = index.and_then(|at| values.get(at)).map(|value| value.trim()).filter(|value| !value.is_empty());
        let value = typed.or(*default).unwrap_or("");
        let plus = query_start.is_some_and(|start| range.start > start) && range.start < fragment_start;
        out.push_str(&encode(value, plus));
        last = range.end;
    }
    out.push_str(&url[last..]);
    out
}

/// Whether a link's target is a path on this computer or a network share, not a web address.
pub fn is_path(target: &str) -> bool {
    let target = target.trim().trim_matches('"');
    let bytes = target.as_bytes();
    let drive = bytes.len() >= 3 && bytes[0].is_ascii_alphabetic() && bytes[1] == b':' && matches!(bytes[2], b'\\' | b'/');
    drive || (target.starts_with("\\\\") && target.len() > 2)
}

fn fill_path(path: &str, values: &[String]) -> String {
    let params = link_params(path);
    let mut out = String::with_capacity(path.len() + 16);
    let mut last = 0;
    for (range, name, default) in placeholders(path) {
        out.push_str(&path[last..range.start]);
        let index = params.iter().position(|param| param.name.eq_ignore_ascii_case(name));
        let typed = index.and_then(|at| values.get(at)).map(|value| value.trim()).filter(|value| !value.is_empty());
        out.push_str(typed.or(default).unwrap_or(""));
        last = range.end;
    }
    out.push_str(&path[last..]);
    out
}

fn encode(value: &str, space_as_plus: bool) -> String {
    let mut out = String::with_capacity(value.len() + 8);
    for byte in value.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => out.push(byte as char),
            b' ' if space_as_plus => out.push('+'),
            _ => out.push_str(&format!("%{byte:02X}")),
        }
    }
    out
}

/// The address with a stand-in in every blank, for checking its shape.
pub fn link_probe(url: &str) -> String {
    let mut out = String::with_capacity(url.len());
    let mut last = 0;
    for (range, _, _) in placeholders(url) {
        out.push_str(&url[last..range.start]);
        out.push('x');
        last = range.end;
    }
    out.push_str(&url[last..]);
    out
}

/// What was typed after a link's alias, as one value per blank: a word each, with the last one
/// taking the rest, so `w london next week` fills a place and then a time.
pub fn split_args(rest: &str, count: usize) -> Vec<String> {
    let rest = rest.trim();
    if count <= 1 {
        return vec![rest.to_string()];
    }
    let mut out = Vec::with_capacity(count);
    let mut remaining = rest;
    while out.len() + 1 < count {
        let Some((word, after)) = remaining.split_once(char::is_whitespace) else { break };
        out.push(word.to_string());
        remaining = after.trim_start();
    }
    if !remaining.is_empty() {
        out.push(remaining.to_string());
    }
    out
}

/// What a link's address should be, with `https://` added to a bare one such as `github.com`.
fn normal_address(typed: &str) -> Result<String, Problem> {
    // A path may hold spaces, and may be pasted in quotes, as Explorer's "Copy as path" gives it.
    if is_path(typed) {
        let path = typed.trim().trim_matches('"').to_string();
        if path.chars().any(char::is_control) {
            return Err((Field::Target, format!("“{typed}” isn't a path Orbit can open.")));
        }
        return Ok(path);
    }
    let probe = link_probe(typed);
    let refused = || (Field::Target, format!("“{typed}” isn't an address Orbit can open."));
    let bare = |probe: &str| as_openable_url(probe).map(|_| format!("https://{typed}"));
    match check_url(&probe) {
        UrlCheck::Blocked(scheme) => Err((Field::Target, format!("Orbit won't open {scheme}: links, because they can run code."))),
        UrlCheck::Invalid => bare(&probe).ok_or_else(refused),
        // example.com:8080 looks like a scheme called "example.com" until the dot gives it away.
        UrlCheck::Unusual(scheme) if scheme.contains('.') || scheme == "localhost" => {
            if typed.contains(char::is_whitespace) { Err(refused()) } else { Ok(format!("https://{typed}")) }
        }
        UrlCheck::Safe | UrlCheck::Unusual(_) => {
            if probe.contains(char::is_whitespace) { Err(refused()) } else { Ok(typed.to_string()) }
        }
    }
}

/// Cleans up what was typed into a quicklink, or says what's wrong with it.
pub fn check_link(link: QuickLink, links: &[QuickLink], commands: &[CustomCommand]) -> Result<QuickLink, Problem> {
    let name = link.name.trim().to_string();
    let alias = link.alias.trim().to_string();
    let typed = link.url.trim();
    if name.is_empty() {
        return Err((Field::Name, "Give it a name.".to_string()));
    }
    if typed.is_empty() {
        return Err((Field::Target, "Add the address it opens.".to_string()));
    }
    let url = normal_address(typed)?;
    check_alias(&alias, &link.id, links, commands)?;
    let id = if link.id.is_empty() { fresh_id("link-", &name, |id| links.iter().any(|other| other.id == id)) } else { link.id };
    Ok(QuickLink { id, name, url, alias, open_with: link.open_with.trim().to_string() })
}

/// The same for a command. It runs only when chosen, with no shell in between.
pub fn check_command(command: CustomCommand, links: &[QuickLink], commands: &[CustomCommand]) -> Result<CustomCommand, Problem> {
    let name = command.name.trim().to_string();
    let program = command.program.trim().trim_matches('"').to_string();
    let alias = command.alias.trim().to_string();
    if name.is_empty() {
        return Err((Field::Name, "Give it a name.".to_string()));
    }
    if program.is_empty() {
        return Err((Field::Target, "Choose the program it runs.".to_string()));
    }
    check_alias(&alias, &command.id, links, commands)?;
    let id = if command.id.is_empty() { fresh_id("cmd-", &name, |id| commands.iter().any(|other| other.id == id)) } else { command.id.clone() };
    Ok(CustomCommand {
        id,
        name,
        program,
        alias,
        arguments: command.arguments.trim().to_string(),
        working_dir: command.working_dir.trim().to_string(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn link(name: &str, alias: &str, url: &str) -> QuickLink {
        QuickLink { id: String::new(), name: name.into(), alias: alias.into(), url: url.into(), open_with: String::new() }
    }

    #[test]
    fn a_bare_address_gets_https() {
        let saved = check_link(link(" GitHub ", "gh", "github.com/a"), &[], &[]).unwrap();
        assert_eq!(saved.url, "https://github.com/a");
        assert_eq!(saved.name, "GitHub");
        assert_eq!(saved.id, "link-github");
        assert_eq!(check_link(link("A", "a", "example.com/{query}"), &[], &[]).unwrap().url, "https://example.com/{query}");
        assert_eq!(check_link(link("A", "", "example.com:8080/x"), &[], &[]).unwrap().url, "https://example.com:8080/x");
    }

    #[test]
    fn files_and_folders_can_be_linked() {
        assert_eq!(check_link(link("Projects", "", "C:\\Users\\me\\My Projects"), &[], &[]).unwrap().url, "C:\\Users\\me\\My Projects", "spaces are fine in a path");
        assert_eq!(check_link(link("Share", "", "\"\\\\nas\\photos\\2026\""), &[], &[]).unwrap().url, "\\\\nas\\photos\\2026", "quotes from Copy as path go");
        assert!(is_path("D:/notes/todo.md") && !is_path("https://x.com") && !is_path("spotify:track:1") && !is_path("\\\\"));
        assert_eq!(fill_link("C:\\Notes\\{day}.md", &["2026 11 07".to_string()]), "C:\\Notes\\2026 11 07.md", "typed as it is, not encoded");
        let mut with_app = link("Doc", "", "C:\\a.txt");
        with_app.open_with = " app-id ".into();
        assert_eq!(check_link(with_app, &[], &[]).unwrap().open_with, "app-id");
    }

    #[test]
    fn other_kinds_of_link_stay_as_typed() {
        assert_eq!(check_link(link("Mail", "", "mailto:a@b.c"), &[], &[]).unwrap().url, "mailto:a@b.c");
        assert_eq!(check_link(link("Song", "", "spotify:track:1"), &[], &[]).unwrap().url, "spotify:track:1");
    }

    #[test]
    fn problems_name_the_box_to_fix() {
        assert_eq!(check_link(link("", "", "x.com"), &[], &[]).unwrap_err().0, Field::Name);
        assert_eq!(check_link(link("A", "", ""), &[], &[]).unwrap_err().0, Field::Target);
        assert_eq!(check_link(link("A", "", "not a link"), &[], &[]).unwrap_err().0, Field::Target);
        assert_eq!(check_link(link("A", "", "javascript:alert(1)"), &[], &[]).unwrap_err().0, Field::Target);
        assert_eq!(check_link(link("A", "two words", "x.com"), &[], &[]).unwrap_err().0, Field::Alias);
        assert!(check_link(link("A", "", "x.com/s?q={query}"), &[], &[]).is_ok(), "choosing it asks for the search");
        assert_eq!(check_link(link("A", "", "x.com/{a b c}/{query} now"), &[], &[]).unwrap_err().0, Field::Target, "only blanks may hold spaces");
    }

    #[test]
    fn blanks_are_found_once_each_in_order() {
        let params = link_params("https://x.example/{city=London}/{query}?d={days}&again={CITY}");
        let names: Vec<_> = params.iter().map(|param| (param.name.as_str(), param.label.as_str(), param.default.as_deref())).collect();
        assert_eq!(names, [("city", "city", Some("London")), ("query", "Search", None), ("days", "days", None)]);
        assert!(!params[0].required() && params[1].required());
        assert!(link_params("https://x.example/{}/{\"a\":1}/{{x}}").iter().all(|param| param.name == "x"), "only names count");
        assert!(link_params("https://plain.example").is_empty());
    }

    #[test]
    fn filling_encodes_each_value_for_where_it_goes() {
        let url = "https://x.example/{place}/s?q={query}#{query}";
        let filled = fill_link(url, &["New York".into(), "a&b c".into()]);
        assert_eq!(filled, "https://x.example/New%20York/s?q=a%26b+c#a%26b%20c");
        assert_eq!(fill_link("https://s?q={query}", &["rust async".into()]), "https://s?q=rust+async", "as searches always were");
        assert_eq!(fill_link("https://x.example/{city=London}", &[String::new()]), "https://x.example/London");
        assert_eq!(fill_link("https://x.example/{a}/{A}", &["é".into()]), "https://x.example/%C3%A9/%C3%A9", "the same blank twice");
    }

    #[test]
    fn missing_values_are_found_and_defaults_count() {
        let url = "https://x.example/{city=London}/{query}";
        assert_eq!(missing_param(url, &[]), Some(1));
        assert_eq!(missing_param(url, &[String::new(), "  ".into()]), Some(1));
        assert_eq!(missing_param(url, &[String::new(), "rain".into()]), None);
    }

    #[test]
    fn typed_words_fill_blanks_with_the_last_taking_the_rest() {
        assert_eq!(split_args("  rust async  ", 1), ["rust async"]);
        assert_eq!(split_args("london next week", 2), ["london", "next week"]);
        assert_eq!(split_args("london", 3), ["london"]);
        assert!(split_args("", 2).is_empty());
        assert_eq!(link_probe("https://x/{a}/{b=1}?q={query}"), "https://x/x/x?q=x");
    }

    #[test]
    fn an_alias_belongs_to_one_item() {
        let mut first = link("First", "g", "https://a.example");
        first.id = "one".into();
        let existing = [first.clone()];
        assert_eq!(check_link(link("Second", "G", "https://b.example"), &existing, &[]).unwrap_err().0, Field::Alias);
        assert!(check_link(first, &existing, &[]).is_ok(), "keeping its own is fine");
    }

    #[test]
    fn ids_stay_unique_and_stay_put() {
        let mut taken = link("Docs", "", "https://a.example");
        taken.id = "link-docs".into();
        let existing = [taken.clone()];
        assert_eq!(check_link(link("Docs", "", "https://b.example"), &existing, &[]).unwrap().id, "link-docs-2");
        assert_eq!(check_link(taken, &existing, &[]).unwrap().id, "link-docs");
    }

    #[test]
    fn commands_need_a_name_and_a_program() {
        let command = |name: &str, program: &str| CustomCommand {
            id: String::new(),
            name: name.into(),
            program: program.into(),
            arguments: " --x ".into(),
            working_dir: String::new(),
            alias: String::new(),
        };
        assert_eq!(check_command(command("", "a.exe"), &[], &[]).unwrap_err().0, Field::Name);
        assert_eq!(check_command(command("A", " "), &[], &[]).unwrap_err().0, Field::Target);
        let saved = check_command(command("Build It", "\"C:\\a b\\c.exe\""), &[], &[]).unwrap();
        assert_eq!(saved.program, "C:\\a b\\c.exe");
        assert_eq!(saved.arguments, "--x");
        assert_eq!(saved.id, "cmd-build-it");
    }
}
