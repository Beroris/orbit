//! GitHub: your issues, pull requests, notifications and stars, from what GitHub's API answers,
//! turned into launcher rows. The host asks through GitHub's own `gh` tool, which holds the
//! sign-in, and passes the answers in. Nothing here goes online.

use crate::ai_usage::{rfc3339_to_unix, span};
use crate::extension::{ActionKind, Item, ItemAction};
use serde_json::Value;

/// What each command asks `gh api` for.
pub fn api_path(command: &str) -> Option<&'static str> {
    Some(match command {
        "issues" => "search/issues?q=is:open+is:issue+involves:@me+archived:false&sort=updated&order=desc&per_page=100",
        "pulls" => "search/issues?q=is:open+is:pr+involves:@me+archived:false&sort=updated&order=desc&per_page=100",
        "notifications" => "notifications?per_page=50",
        "starred" => "user/starred?per_page=100",
        _ => return None,
    })
}

/// What the list says when there's nothing in it.
pub fn empty_text(command: &str) -> &'static str {
    match command {
        "issues" => "No open issues involve you",
        "pulls" => "No open pull requests involve you",
        "notifications" => "No unread notifications",
        "starred" => "You haven't starred anything",
        _ => "Nothing found",
    }
}

fn text<'a>(value: &'a Value, pointer: &str) -> &'a str {
    value.pointer(pointer).and_then(Value::as_str).unwrap_or("")
}

/// "3h ago", "2d ago", "just now".
fn ago(when: &str, now: i64) -> String {
    match rfc3339_to_unix(when) {
        Some(at) if now - at < 60 => "just now".into(),
        Some(at) => {
            let full = span(now - at);
            // The largest part is enough on a row.
            format!("{} ago", full.split(' ').next().unwrap_or(&full))
        }
        None => String::new(),
    }
}

fn open(title: &str, url: &str) -> ItemAction {
    ItemAction { title: title.into(), kind: ActionKind::Open(url.into()), shortcut: None }
}

fn copy(title: &str, text: String, keys: &str) -> ItemAction {
    ItemAction { title: title.into(), kind: ActionKind::Copy(text), shortcut: Some(keys.into()) }
}

fn refresh() -> ItemAction {
    ItemAction { title: "Refresh".into(), kind: ActionKind::Callback("refresh".into()), shortcut: Some("Ctrl+R".into()) }
}

/// "owner/repo" from an API address such as `https://api.github.com/repos/owner/repo`.
fn repo_of(api_url: &str) -> String {
    api_url.split("/repos/").nth(1).unwrap_or("").to_string()
}

/// The web page for an API address: issues and pulls, commits and releases have their own.
pub fn web_url(api_url: &str, fallback: &str) -> String {
    let Some(rest) = api_url.strip_prefix("https://api.github.com/repos/") else { return fallback.to_string() };
    let parts: Vec<&str> = rest.split('/').collect();
    match parts.as_slice() {
        [owner, repo, "pulls", number, ..] => format!("https://github.com/{owner}/{repo}/pull/{number}"),
        [owner, repo, "issues", number, ..] => format!("https://github.com/{owner}/{repo}/issues/{number}"),
        [owner, repo, "commits", sha, ..] => format!("https://github.com/{owner}/{repo}/commit/{sha}"),
        [owner, repo, "releases", ..] => format!("https://github.com/{owner}/{repo}/releases"),
        [owner, repo, "discussions", number, ..] => format!("https://github.com/{owner}/{repo}/discussions/{number}"),
        [owner, repo, ..] => format!("https://github.com/{owner}/{repo}"),
        _ => fallback.to_string(),
    }
}

/// Why a notification came, as GitHub's `reason` is said on a row.
fn reason(reason: &str) -> &str {
    match reason {
        "assign" => "Assigned to you",
        "author" => "You opened it",
        "comment" => "You commented",
        "ci_activity" => "Workflow run",
        "invitation" => "Invitation",
        "manual" => "Subscribed",
        "mention" => "Mentioned you",
        "team_mention" => "Mentioned your team",
        "review_requested" => "Review requested",
        "security_alert" => "Security alert",
        "state_change" => "State changed",
        "subscribed" => "Watching",
        "approval_requested" => "Approval requested",
        _ => "",
    }
}

fn issue_items(value: &Value, now: i64) -> Vec<Item> {
    let list = value.get("items").and_then(Value::as_array).cloned().unwrap_or_default();
    list.iter()
        .filter_map(|issue| {
            let url = text(issue, "/html_url");
            let title = text(issue, "/title");
            if url.is_empty() || title.is_empty() {
                return None;
            }
            let repo = repo_of(text(issue, "/repository_url"));
            let number = issue.get("number").and_then(Value::as_u64).unwrap_or(0);
            let reference = format!("{repo}#{number}");
            let draft = issue.get("draft").and_then(Value::as_bool) == Some(true);
            let author = text(issue, "/user/login");
            let mut subtitle = reference.clone();
            if draft {
                subtitle.push_str(" \u{00B7} Draft");
            }
            if !author.is_empty() {
                subtitle.push_str(&format!(" \u{00B7} by {author}"));
            }
            Some(Item {
                id: reference.clone(),
                title: title.into(),
                subtitle,
                accessory: ago(text(issue, "/updated_at"), now),
                icon: None,
                actions: vec![open("Open in Browser", url), copy("Copy Link", url.into(), "Ctrl+Shift+C"), copy("Copy Reference", reference, "Ctrl+Shift+."), refresh()],
                meters: Vec::new(),
            })
        })
        .collect()
}

fn notification_items(value: &Value, now: i64) -> Vec<Item> {
    let list = value.as_array().cloned().unwrap_or_default();
    list.iter()
        .filter_map(|note| {
            let id = text(note, "/id");
            let title = text(note, "/subject/title");
            if id.is_empty() || title.is_empty() {
                return None;
            }
            let repo = text(note, "/repository/full_name");
            let repo_page = text(note, "/repository/html_url");
            let url = web_url(text(note, "/subject/url"), repo_page);
            let why = reason(text(note, "/reason"));
            let subtitle = if why.is_empty() { repo.to_string() } else { format!("{repo} \u{00B7} {why}") };
            Some(Item {
                id: id.into(),
                title: title.into(),
                subtitle,
                accessory: ago(text(note, "/updated_at"), now),
                icon: None,
                actions: vec![
                    open("Open in Browser", &url),
                    ItemAction { title: "Mark as Read".into(), kind: ActionKind::Callback("read".into()), shortcut: None },
                    copy("Copy Link", url.clone(), "Ctrl+Shift+C"),
                    refresh(),
                ],
                meters: Vec::new(),
            })
        })
        .collect()
}

/// 1234 is "1.2k", 56789 is "57k".
fn stars(count: u64) -> String {
    match count {
        0..=999 => count.to_string(),
        1000..=9_999 => format!("{:.1}k", count as f64 / 1000.0).replace(".0k", "k"),
        10_000..=999_999 => format!("{}k", (count + 500) / 1000),
        _ => format!("{:.1}m", count as f64 / 1_000_000.0).replace(".0m", "m"),
    }
}

fn starred_items(value: &Value) -> Vec<Item> {
    let list = value.as_array().cloned().unwrap_or_default();
    list.iter()
        .filter_map(|repo| {
            let name = text(repo, "/full_name");
            let url = text(repo, "/html_url");
            if name.is_empty() || url.is_empty() {
                return None;
            }
            let count = repo.get("stargazers_count").and_then(Value::as_u64).unwrap_or(0);
            let language = text(repo, "/language");
            let description = text(repo, "/description");
            let subtitle = match (description.is_empty(), language.is_empty()) {
                (false, false) => format!("{language} \u{00B7} {description}"),
                (false, true) => description.to_string(),
                (true, false) => language.to_string(),
                (true, true) => String::new(),
            };
            Some(Item {
                id: name.into(),
                title: name.into(),
                subtitle,
                accessory: format!("\u{2605} {}", stars(count)),
                icon: None,
                actions: vec![
                    open("Open in Browser", url),
                    copy("Copy Link", url.into(), "Ctrl+Shift+C"),
                    copy("Copy Clone Command", format!("git clone {url}.git"), "Ctrl+Shift+."),
                    refresh(),
                ],
                meters: Vec::new(),
            })
        })
        .collect()
}

/// The rows for what `gh api` answered for a command, newest first, as GitHub gives them.
pub fn items(command: &str, json: &str, now: i64) -> Result<Vec<Item>, String> {
    let value: Value = serde_json::from_str(json).map_err(|_| "GitHub's answer wasn't readable".to_string())?;
    if let Some(message) = value.get("message").and_then(Value::as_str) {
        return Err(format!("GitHub said: {message}"));
    }
    Ok(match command {
        "issues" | "pulls" => issue_items(&value, now),
        "notifications" => notification_items(&value, now),
        "starred" => starred_items(&value),
        _ => Vec::new(),
    })
}

/// The rows that have every word typed, in any order, in their title or line under it.
pub fn filter(items: &[Item], query: &str) -> Vec<Item> {
    let words: Vec<String> = query.split_whitespace().map(str::to_lowercase).collect();
    items
        .iter()
        .filter(|item| {
            let haystack = format!("{} {} {}", item.title, item.subtitle, item.id).to_lowercase();
            words.iter().all(|word| haystack.contains(word))
        })
        .cloned()
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    const NOW: i64 = 1_794_268_800; // 2026-11-10T00:00:00Z

    #[test]
    fn issues_and_pulls_show_where_they_are() {
        let json = r#"{"total_count":1,"items":[{"number":42,"title":"Crash on start","html_url":"https://github.com/o/r/pull/42","repository_url":"https://api.github.com/repos/o/r","updated_at":"2026-11-09T21:00:00Z","draft":true,"user":{"login":"sam"},"pull_request":{}}]}"#;
        let items = items("pulls", json, NOW).unwrap();
        assert_eq!(items[0].title, "Crash on start");
        assert_eq!(items[0].subtitle, "o/r#42 \u{00B7} Draft \u{00B7} by sam");
        assert_eq!(items[0].accessory, "3h ago");
        assert!(matches!(&items[0].actions[0].kind, ActionKind::Open(url) if url == "https://github.com/o/r/pull/42"));
        assert!(matches!(&items[0].actions[2].kind, ActionKind::Copy(text) if text == "o/r#42"));
    }

    #[test]
    fn notifications_open_the_page_not_the_api() {
        let json = r#"[{"id":"77","unread":true,"reason":"review_requested","updated_at":"2026-11-08T00:00:00Z","subject":{"title":"Add dark mode","url":"https://api.github.com/repos/o/r/pulls/9","type":"PullRequest"},"repository":{"full_name":"o/r","html_url":"https://github.com/o/r"}}]"#;
        let items = items("notifications", json, NOW).unwrap();
        assert_eq!(items[0].id, "77");
        assert_eq!(items[0].subtitle, "o/r \u{00B7} Review requested");
        assert_eq!(items[0].accessory, "2d ago");
        assert!(matches!(&items[0].actions[0].kind, ActionKind::Open(url) if url == "https://github.com/o/r/pull/9"));
        assert!(matches!(&items[0].actions[1].kind, ActionKind::Callback(id) if id == "read"));
    }

    #[test]
    fn web_pages_for_api_addresses() {
        assert_eq!(web_url("https://api.github.com/repos/o/r/issues/3", ""), "https://github.com/o/r/issues/3");
        assert_eq!(web_url("https://api.github.com/repos/o/r/commits/abc", ""), "https://github.com/o/r/commit/abc");
        assert_eq!(web_url("https://api.github.com/repos/o/r/releases/123", ""), "https://github.com/o/r/releases");
        assert_eq!(web_url("", "https://github.com/o/r"), "https://github.com/o/r", "a check run has no address of its own");
    }

    #[test]
    fn stars_are_short() {
        let json = r#"[{"full_name":"octocat/hello-world","html_url":"https://github.com/octocat/hello-world","stargazers_count":12345,"language":"TypeScript","description":"A GUI"}]"#;
        let items = items("starred", json, NOW).unwrap();
        assert_eq!(items[0].accessory, "\u{2605} 12k");
        assert_eq!(items[0].subtitle, "TypeScript \u{00B7} A GUI");
        assert_eq!(stars(999), "999");
        assert_eq!(stars(1000), "1k");
        assert_eq!(stars(1250), "1.2k");
        assert_eq!(stars(2_500_000), "2.5m");
    }

    #[test]
    fn errors_say_what_github_said() {
        assert_eq!(items("issues", r#"{"message":"Bad credentials"}"#, NOW).unwrap_err(), "GitHub said: Bad credentials");
        assert!(items("issues", "gh: not logged in", NOW).is_err());
    }

    #[test]
    fn typing_narrows_the_list() {
        let json = r#"[{"full_name":"a/rust-thing","html_url":"https://github.com/a/rust-thing","stargazers_count":1},{"full_name":"b/web","html_url":"https://github.com/b/web","stargazers_count":1,"description":"Rust on the web"}]"#;
        let all = items("starred", json, NOW).unwrap();
        assert_eq!(filter(&all, "").len(), 2);
        assert_eq!(filter(&all, "RUST").len(), 2);
        assert_eq!(filter(&all, "rust web").len(), 1);
        assert_eq!(filter(&all, "python").len(), 0);
    }

    #[test]
    fn every_command_asks_for_something() {
        for command in ["issues", "pulls", "notifications", "starred"] {
            assert!(api_path(command).is_some());
            assert!(crate::builtin_ext::BuiltinExt::GitHub.list_command(command).is_some());
        }
    }
}
