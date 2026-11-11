//! GitHub: your issues, pull requests, notifications and stars, from what GitHub's API answers,
//! turned into launcher rows. The host asks through GitHub's own `gh` tool, which holds the
//! sign-in, and passes the answers in. Nothing here goes online.
//!
//! Issues and pull requests are each one GraphQL question. It says whether a pull request has
//! conflicts, how its checks went and what its reviewers decided, and it brings what was closed in
//! the last two weeks, which is listed under the open ones. Each row is tagged with the filters it
//! comes under (its repository, whether you made it, and so on) so the list can be narrowed without
//! asking again.

use crate::extension::{ActionKind, FilterOption, Item, ItemAction, Tag, Tone};
use serde_json::Value;

/// How far back "Recently closed" goes, in days.
pub const CLOSED_DAYS: i64 = 14;
/// The headings issues and pull requests are listed under.
pub const OPEN_SECTION: &str = "Open";
pub const CLOSED_SECTION: &str = "Recently closed";
/// The filter that leaves out what was merged or closed.
pub const OPEN_FILTER: &str = "open";

/// How a list is asked for.
#[derive(Debug, PartialEq)]
pub enum Ask {
    /// `gh api <path>`.
    Rest(&'static str),
    /// `gh api graphql -f query=<query>`, with each variable as `-f name=value`.
    Graphql { query: &'static str, variables: Vec<(&'static str, String)> },
}

/// Each issue or pull request: what the rows show, and what the filters and tags are worked out from.
const PULLS_QUERY: &str = "query($open: String!, $closed: String!) { viewer { login } \
open: search(query: $open, type: ISSUE, first: 50) { nodes { ...pull } } \
closed: search(query: $closed, type: ISSUE, first: 20) { nodes { ...pull } } } \
fragment pull on PullRequest { number title url isDraft state updatedAt closedAt headRefName mergeable reviewDecision \
author { login } repository { nameWithOwner } \
reviewRequests(first: 20) { nodes { requestedReviewer { ... on User { login } } } } \
commits(last: 1) { nodes { commit { statusCheckRollup { state } } } } }";

const ISSUES_QUERY: &str = "query($open: String!, $closed: String!) { viewer { login } \
open: search(query: $open, type: ISSUE, first: 50) { nodes { ...issue } } \
closed: search(query: $closed, type: ISSUE, first: 20) { nodes { ...issue } } } \
fragment issue on Issue { number title url state stateReason updatedAt closedAt \
author { login } repository { nameWithOwner } assignees(first: 10) { nodes { login } } }";

/// What a command asks GitHub for, at `now`, which decides how far back "recently closed" goes.
pub fn ask(command: &str, now: i64) -> Option<Ask> {
    let since = date(now - CLOSED_DAYS * 86_400);
    let searches = |kind: &str| {
        vec![
            ("open", format!("is:open is:{kind} involves:@me archived:false sort:updated-desc")),
            ("closed", format!("is:closed is:{kind} involves:@me archived:false closed:>={since} sort:updated-desc")),
        ]
    };
    Some(match command {
        "issues" => Ask::Graphql { query: ISSUES_QUERY, variables: searches("issue") },
        "pulls" => Ask::Graphql { query: PULLS_QUERY, variables: searches("pr") },
        "notifications" => Ask::Rest("notifications?per_page=50"),
        "starred" => Ask::Rest("user/starred?per_page=100"),
        _ => return None,
    })
}

/// What the list says when there's nothing in it.
/// What an empty list says under a filter.
pub fn filtered_empty_text(command: &str, filter: &str) -> &'static str {
    match (command, filter) {
        ("pulls", OPEN_FILTER) => "No open pull requests involve you",
        ("issues", OPEN_FILTER) => "No open issues involve you",
        _ => "Nothing here under this filter",
    }
}

pub fn empty_text(command: &str) -> &'static str {
    match command {
        "issues" => "No issues involve you",
        "pulls" => "No pull requests involve you",
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

/// "2d 20h", "3h 5m", "12m", "now".
pub fn span(seconds: i64) -> String {
    let seconds = seconds.max(0);
    let (days, hours, minutes) = (seconds / 86_400, seconds % 86_400 / 3600, seconds % 3600 / 60);
    if days > 0 {
        format!("{days}d {hours}h")
    } else if hours > 0 {
        format!("{hours}h {minutes}m")
    } else if minutes > 0 {
        format!("{minutes}m")
    } else {
        "now".into()
    }
}

/// Seconds since 1970 for an RFC 3339 time, such as "2026-11-12T18:59:59.655247+00:00".
pub fn rfc3339_to_unix(text: &str) -> Option<i64> {
    let text = text.trim();
    let (date, rest) = text.split_once(['T', 't', ' '])?;
    let mut date_parts = date.splitn(3, '-').map(|part| part.parse::<i64>().ok());
    let (year, month, day) = (date_parts.next()??, date_parts.next()??, date_parts.next()??);
    let zone_at = rest.find(['Z', 'z', '+', '-']).unwrap_or(rest.len());
    let (clock, zone) = rest.split_at(zone_at);
    let mut clock_parts = clock.splitn(3, ':');
    let hour: i64 = clock_parts.next()?.parse().ok()?;
    let minute: i64 = clock_parts.next()?.parse().ok()?;
    let second: i64 = clock_parts.next().map(|s| s.split('.').next().unwrap_or("0")).unwrap_or("0").parse().ok()?;
    if !(1..=12).contains(&month) || !(1..=31).contains(&day) || hour > 23 || minute > 59 || second > 60 {
        return None;
    }
    let offset = match zone.chars().next() {
        None | Some('Z' | 'z') => 0,
        Some(sign) => {
            let (h, m) = zone[1..].split_once(':').unwrap_or((&zone[1..], "0"));
            let minutes = h.parse::<i64>().ok()? * 60 + m.parse::<i64>().ok()?;
            if sign == '-' { -minutes * 60 } else { minutes * 60 }
        }
    };
    // Days since 1970 from a civil date (Howard Hinnant's algorithm).
    let y = if month <= 2 { year - 1 } else { year };
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let mp = (month + 9) % 12;
    let doy = (153 * mp + 2) / 5 + day - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    let days = era * 146_097 + doe - 719_468;
    Some(days * 86_400 + hour * 3600 + minute * 60 + second - offset)
}

/// The date in UTC, as GitHub's search wants it: "2026-10-28".
pub fn date(unix: i64) -> String {
    // A civil date from days since 1970 (Howard Hinnant's algorithm).
    let z = unix.div_euclid(86_400) + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = yoe + era * 400 + i64::from(month <= 2);
    format!("{year:04}-{month:02}-{day:02}")
}

fn open(title: &str, url: &str) -> ItemAction {
    ItemAction { title: title.into(), kind: ActionKind::Open(url.into()), shortcut: None, icon: None }
}

/// A Copy action, with keys for it and the symbol that says what it copies.
fn copy(title: &str, text: String, keys: &str, icon: &str) -> ItemAction {
    ItemAction { title: title.into(), kind: ActionKind::Copy(text), shortcut: Some(keys.into()), icon: Some(icon.into()) }
}

fn refresh() -> ItemAction {
    ItemAction { title: "Refresh".into(), kind: ActionKind::Callback("refresh".into()), shortcut: Some("Ctrl+R".into()), icon: Some("refresh".into()) }
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

/// The filter for one repository's rows.
fn repo_facet(repo: &str) -> String {
    format!("repo:{repo}")
}

/// Where a pull request stands, from what GitHub said about it.
#[derive(Debug, Default, PartialEq)]
struct PullState {
    open: bool,
    merged: bool,
    draft: bool,
    conflicts: bool,
    checks_failed: bool,
    checks_running: bool,
    changes_requested: bool,
    approved: bool,
    /// Waiting for a review from whoever is signed in.
    review_wanted: bool,
}

impl PullState {
    fn of(node: &Value) -> PullState {
        let state = text(node, "/state");
        let checks = text(node, "/commits/nodes/0/commit/statusCheckRollup/state");
        let review = text(node, "/reviewDecision");
        PullState {
            open: state == "OPEN",
            merged: state == "MERGED",
            draft: node.get("isDraft").and_then(Value::as_bool) == Some(true),
            conflicts: text(node, "/mergeable") == "CONFLICTING",
            checks_failed: matches!(checks, "FAILURE" | "ERROR"),
            checks_running: matches!(checks, "PENDING" | "EXPECTED"),
            changes_requested: review == "CHANGES_REQUESTED",
            approved: review == "APPROVED",
            review_wanted: false,
        }
    }

    /// Something the author has to sort out before it can go in.
    fn needs_attention(&self) -> bool {
        self.open && (self.conflicts || self.checks_failed || self.changes_requested)
    }

    /// At most two pills, the most pressing first. An open one with nothing to say is just "Open".
    fn tags(&self) -> Vec<Tag> {
        if !self.open {
            return vec![if self.merged { Tag::new("Merged", Tone::Purple) } else { Tag::new("Closed", Tone::Red) }];
        }
        let said = [
            (self.review_wanted, "Review requested", Tone::Blue),
            (self.conflicts, "Conflicts", Tone::Red),
            (self.checks_failed, "Checks failing", Tone::Red),
            (self.changes_requested, "Changes requested", Tone::Orange),
            (self.draft, "Draft", Tone::Grey),
            (self.approved, "Approved", Tone::Green),
            (self.checks_running, "Checks running", Tone::Orange),
        ];
        let mut tags: Vec<Tag> = said.iter().filter(|(on, _, _)| *on).take(2).map(|(_, words, tone)| Tag::new(words, *tone)).collect();
        if tags.is_empty() {
            tags.push(Tag::new("Open", Tone::Green));
        }
        tags
    }

    /// The symbol's colour, as GitHub colours a pull request's own.
    fn tint(&self) -> Tone {
        match (self.open, self.merged, self.draft) {
            (true, _, true) => Tone::Grey,
            (true, _, false) => Tone::Green,
            (false, true, _) => Tone::Purple,
            (false, false, _) => Tone::Red,
        }
    }
}

/// An issue's or a pull request's row. `closed` puts it under "Recently closed".
fn search_item(node: &Value, pull: bool, me: &str, now: i64) -> Option<Item> {
    let url = text(node, "/url");
    let title = text(node, "/title");
    if url.is_empty() || title.is_empty() {
        return None;
    }
    let repo = text(node, "/repository/nameWithOwner");
    let number = node.get("number").and_then(Value::as_u64).unwrap_or(0);
    let reference = format!("{repo}#{number}");
    let author = text(node, "/author/login");
    let mine = !me.is_empty() && author.eq_ignore_ascii_case(me);
    let open = text(node, "/state") == "OPEN";
    let mut subtitle = reference.clone();
    if !author.is_empty() && !mine {
        subtitle.push_str(&format!(" \u{00B7} by {author}"));
    }
    let when = if open { text(node, "/updatedAt") } else { text(node, "/closedAt") };
    let mut facets = vec![repo_facet(repo)];
    if mine {
        facets.push("mine".into());
    }
    let mut actions = vec![open_action(pull, url), copy("Copy Link", url.into(), "Ctrl+Shift+C", "link"), copy("Copy Reference", reference.clone(), "Ctrl+Shift+.", "hash")];
    let (tags, tint) = if pull {
        let mut state = PullState::of(node);
        let asked = node.pointer("/reviewRequests/nodes").and_then(Value::as_array).is_some_and(|list| list.iter().any(|ask| !me.is_empty() && text(ask, "/requestedReviewer/login").eq_ignore_ascii_case(me)));
        state.review_wanted = state.open && asked;
        if state.review_wanted {
            facets.push("review".into());
        }
        if state.needs_attention() {
            facets.push("attention".into());
        }
        if state.open && state.draft {
            facets.push("draft".into());
        }
        let branch = text(node, "/headRefName");
        if !branch.is_empty() {
            actions.push(copy("Copy Branch Name", branch.into(), "Ctrl+Shift+B", "branch"));
        }
        (state.tags(), state.tint())
    } else {
        let assigned = node.pointer("/assignees/nodes").and_then(Value::as_array).is_some_and(|list| list.iter().any(|who| !me.is_empty() && text(who, "/login").eq_ignore_ascii_case(me)));
        if open && assigned {
            facets.push("assigned".into());
        }
        issue_look(open, text(node, "/stateReason"))
    };
    if open {
        facets.push(OPEN_FILTER.into());
    }
    actions.push(refresh());
    Some(Item {
        id: reference,
        title: title.into(),
        subtitle,
        accessory: ago(when, now),
        actions,
        tags,
        tint: Some(tint),
        section: if open { OPEN_SECTION } else { CLOSED_SECTION }.into(),
        facets,
        ..Item::default()
    })
}

fn open_action(pull: bool, url: &str) -> ItemAction {
    open(if pull { "Open Pull Request" } else { "Open Issue" }, url)
}

/// An issue's pill and symbol colour, as GitHub shows it: green while open, purple once done, grey
/// when it was closed without being done.
fn issue_look(open: bool, reason: &str) -> (Vec<Tag>, Tone) {
    if open {
        return (vec![Tag::new("Open", Tone::Green)], Tone::Green);
    }
    match reason {
        "NOT_PLANNED" => (vec![Tag::new("Not planned", Tone::Grey)], Tone::Grey),
        "DUPLICATE" => (vec![Tag::new("Duplicate", Tone::Grey)], Tone::Grey),
        "COMPLETED" => (vec![Tag::new("Completed", Tone::Purple)], Tone::Purple),
        _ => (vec![Tag::new("Closed", Tone::Purple)], Tone::Purple),
    }
}

/// The open ones as GitHub ordered them, newest change first, then what was closed lately, last
/// closed first.
fn search_items(value: &Value, pull: bool, now: i64) -> Result<Vec<Item>, String> {
    let Some(data) = value.get("data").filter(|data| !data.is_null()) else {
        let message = value.pointer("/errors/0/message").and_then(Value::as_str).unwrap_or("nothing came back");
        return Err(format!("GitHub said: {message}"));
    };
    let me = text(data, "/viewer/login");
    let list = |pointer: &str| data.pointer(pointer).and_then(Value::as_array).cloned().unwrap_or_default();
    let mut closed = list("/closed/nodes");
    closed.sort_by_key(|node| std::cmp::Reverse(rfc3339_to_unix(text(node, "/closedAt")).unwrap_or(0)));
    let open = list("/open/nodes");
    // A search can be a moment behind: something just closed may still be in the open one.
    let mut seen = std::collections::HashSet::new();
    Ok(open
        .iter()
        .chain(closed.iter())
        .filter_map(|node| search_item(node, pull, me, now))
        .filter(|item| seen.insert(item.id.clone()))
        .collect())
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
                actions: vec![
                    open("Open in Browser", &url),
                    ItemAction { title: "Mark as Read".into(), kind: ActionKind::Callback("read".into()), shortcut: None, icon: Some("check".into()) },
                    copy("Copy Link", url.clone(), "Ctrl+Shift+C", "link"),
                    refresh(),
                ],
                facets: if repo.is_empty() { Vec::new() } else { vec![repo_facet(repo)] },
                ..Item::default()
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
                actions: vec![
                    open("Open in Browser", url),
                    copy("Copy Link", url.into(), "Ctrl+Shift+C", "link"),
                    copy("Copy Clone Command", format!("git clone {url}.git"), "Ctrl+Shift+.", "terminal"),
                    refresh(),
                ],
                ..Item::default()
            })
        })
        .collect()
}

/// The rows for what `gh api` answered for a command, as GitHub ordered them.
pub fn items(command: &str, json: &str, now: i64) -> Result<Vec<Item>, String> {
    let value: Value = serde_json::from_str(json).map_err(|_| "GitHub's answer wasn't readable".to_string())?;
    if let Some(message) = value.get("message").and_then(Value::as_str) {
        return Err(format!("GitHub said: {message}"));
    }
    match command {
        "issues" => search_items(&value, false, now),
        "pulls" => search_items(&value, true, now),
        "notifications" => Ok(notification_items(&value, now)),
        "starred" => Ok(starred_items(&value)),
        _ => Ok(Vec::new()),
    }
}

/// What the filter drop-down offers for a list: everything, then the ways to narrow it that have
/// something in them, then each repository, those with the most rows first.
pub fn filters(command: &str, items: &[Item]) -> Vec<FilterOption> {
    let count = |facet: &str| items.iter().filter(|item| item.facets.iter().any(|has| has == facet)).count();
    let everything = match command {
        "issues" => "All issues",
        "pulls" => "All pull requests",
        "notifications" => "All notifications",
        _ => "Everything",
    };
    let mut options = vec![FilterOption { id: String::new(), title: everything.into(), subtitle: String::new() }];
    let kinds: &[(&str, &str)] = match command {
        "pulls" => &[(OPEN_FILTER, "Open pull requests"), ("mine", "Created by you"), ("review", "Waiting for your review"), ("attention", "Needs attention"), ("draft", "Drafts")],
        "issues" => &[(OPEN_FILTER, "Open issues"), ("mine", "Created by you"), ("assigned", "Assigned to you")],
        _ => &[],
    };
    for (id, title) in kinds {
        let found = count(id);
        // Open ones only is worth offering whenever it would hide something, even if it leaves
        // nothing; the others when they have something in them.
        let worth = if *id == OPEN_FILTER { found < items.len() } else { found > 0 };
        if worth {
            options.push(FilterOption { id: (*id).into(), title: (*title).into(), subtitle: found.to_string() });
        }
    }
    let mut repos: Vec<(&str, usize)> = Vec::new();
    for item in items {
        for repo in item.facets.iter().filter_map(|facet| facet.strip_prefix("repo:")) {
            match repos.iter_mut().find(|(name, _)| *name == repo) {
                Some((_, found)) => *found += 1,
                None => repos.push((repo, 1)),
            }
        }
    }
    // One repository is nothing to choose between.
    if repos.len() > 1 {
        repos.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.to_lowercase().cmp(&b.0.to_lowercase())));
        options.extend(repos.into_iter().map(|(repo, found)| FilterOption { id: repo_facet(repo), title: repo.into(), subtitle: found.to_string() }));
    }
    options
}

/// The rows a filter keeps. No filter, or one that isn't offered any more, keeps them all.
pub fn apply_filter(items: Vec<Item>, filter: &str) -> Vec<Item> {
    if filter.is_empty() {
        return items;
    }
    items.into_iter().filter(|item| item.facets.iter().any(|facet| facet == filter)).collect()
}

/// The rows that have every word typed, in any order, in their title, the line under it or a tag.
pub fn filter(items: &[Item], query: &str) -> Vec<Item> {
    let words: Vec<String> = query.split_whitespace().map(str::to_lowercase).collect();
    items
        .iter()
        .filter(|item| {
            let tags: Vec<&str> = item.tags.iter().map(|tag| tag.text.as_str()).collect();
            let haystack = format!("{} {} {} {}", item.title, item.subtitle, item.id, tags.join(" ")).to_lowercase();
            words.iter().all(|word| haystack.contains(word))
        })
        .cloned()
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    const NOW: i64 = 1_794_268_800; // 2026-11-10T00:00:00Z

    /// A pull request as GitHub's GraphQL answers, with the parts that change passed in.
    fn pull(number: u64, repo: &str, author: &str, more: &str) -> String {
        format!(
            r#"{{"number":{number},"title":"Change {number}","url":"https://github.com/{repo}/pull/{number}","isDraft":false,"state":"OPEN","updatedAt":"2026-11-09T21:00:00Z","closedAt":null,"headRefName":"fix-{number}","mergeable":"MERGEABLE","reviewDecision":null,"author":{{"login":"{author}"}},"repository":{{"nameWithOwner":"{repo}"}},"reviewRequests":{{"nodes":[]}},"commits":{{"nodes":[{{"commit":{{"statusCheckRollup":{{"state":"SUCCESS"}}}}}}]}}{more}}}"#
        )
    }

    fn answer(open: &[String], closed: &[String]) -> String {
        format!(r#"{{"data":{{"viewer":{{"login":"me"}},"open":{{"nodes":[{}]}},"closed":{{"nodes":[{}]}}}}}}"#, open.join(","), closed.join(","))
    }

    fn tags(item: &Item) -> Vec<&str> {
        item.tags.iter().map(|tag| tag.text.as_str()).collect()
    }

    #[test]
    fn pull_requests_say_how_they_stand() {
        let json = answer(
            &[
                pull(1, "o/r", "me", ""),
                pull(2, "o/r", "sam", r#","mergeable":"CONFLICTING","isDraft":true"#),
                pull(3, "o/web", "me", r#","commits":{"nodes":[{"commit":{"statusCheckRollup":{"state":"FAILURE"}}}]},"reviewDecision":"CHANGES_REQUESTED","mergeable":"CONFLICTING""#),
                pull(4, "o/r", "sam", r#","reviewDecision":"APPROVED","reviewRequests":{"nodes":[{"requestedReviewer":{"login":"ME"}}]}"#),
            ],
            &[],
        );
        let items = items("pulls", &json, NOW).unwrap();
        assert_eq!(tags(&items[0]), ["Open"], "nothing to say is just open");
        assert_eq!(items[0].tint, Some(Tone::Green));
        assert_eq!(items[0].subtitle, "o/r#1", "your own doesn't say who made it");
        assert_eq!(tags(&items[1]), ["Conflicts", "Draft"]);
        assert_eq!(items[1].tint, Some(Tone::Grey), "a draft is grey, as on GitHub");
        assert_eq!(items[1].subtitle, "o/r#2 \u{00B7} by sam");
        assert_eq!(tags(&items[2]), ["Conflicts", "Checks failing"], "two at most, the most pressing first");
        assert_eq!(items[2].tags[0].tone, Tone::Red);
        assert_eq!(tags(&items[3]), ["Review requested", "Approved"], "a review you were asked for comes first");
        assert_eq!(items[3].accessory, "3h ago");
        assert!(items.iter().all(|item| item.section == OPEN_SECTION));
        assert!(matches!(&items[0].actions[0].kind, ActionKind::Open(url) if url == "https://github.com/o/r/pull/1"));
        assert!(matches!(&items[0].actions[2].kind, ActionKind::Copy(text) if text == "o/r#1"));
        assert!(items[0].actions.iter().any(|action| action.kind == ActionKind::Copy("fix-1".into())), "the branch can be copied");
        assert_eq!(items[3].facets, ["repo:o/r", "review", "open"], "asked for your review, whatever the case of your name");
        assert_eq!(items[2].facets, ["repo:o/web", "mine", "attention", "open"]);
        assert_eq!(items[1].facets, ["repo:o/r", "attention", "draft", "open"]);
    }

    #[test]
    fn recently_closed_comes_after_the_open_ones() {
        let merged = pull(7, "o/r", "me", r#","state":"MERGED","closedAt":"2026-11-08T00:00:00Z""#);
        let closed = pull(8, "o/r", "me", r#","state":"CLOSED","closedAt":"2026-11-09T12:00:00Z","mergeable":"CONFLICTING""#);
        let items = items("pulls", &answer(&[pull(1, "o/r", "me", "")], &[merged, closed]), NOW).unwrap();
        let ids: Vec<&str> = items.iter().map(|item| item.id.as_str()).collect();
        assert_eq!(ids, ["o/r#1", "o/r#8", "o/r#7"], "the last closed first");
        assert_eq!(items[1].section, CLOSED_SECTION);
        assert_eq!(tags(&items[1]), ["Closed"], "a closed one's conflicts don't matter any more");
        assert_eq!(items[1].tint, Some(Tone::Red));
        assert_eq!(tags(&items[2]), ["Merged"]);
        assert_eq!(items[2].tint, Some(Tone::Purple));
        assert_eq!(items[2].accessory, "2d ago", "when it was closed, not last changed");
        assert!(!items[1].facets.contains(&"attention".to_string()));
        assert!(!items[1].facets.contains(&OPEN_FILTER.to_string()) && !items[2].facets.contains(&OPEN_FILTER.to_string()));
    }

    #[test]
    fn merged_and_closed_ones_can_be_left_out() {
        let merged = pull(7, "o/r", "me", r#","state":"MERGED","closedAt":"2026-11-08T00:00:00Z""#);
        let closed = pull(8, "o/web", "me", r#","state":"CLOSED","closedAt":"2026-11-09T12:00:00Z""#);
        let all = items("pulls", &answer(&[pull(1, "o/r", "me", "")], &[merged.clone(), closed.clone()]), NOW).unwrap();
        let open = filters("pulls", &all).into_iter().find(|option| option.id == OPEN_FILTER).expect("offered when something is closed");
        assert_eq!((open.title.as_str(), open.subtitle.as_str()), ("Open pull requests", "1"));
        let kept: Vec<String> = apply_filter(all, OPEN_FILTER).into_iter().map(|item| item.id).collect();
        assert_eq!(kept, ["o/r#1"]);
        // Nothing open yet: still offered, since it hides what was merged, and says so when empty.
        let only_closed = items("pulls", &answer(&[], &[merged, closed]), NOW).unwrap();
        assert!(filters("pulls", &only_closed).iter().any(|option| option.id == OPEN_FILTER && option.subtitle == "0"));
        assert_eq!(filtered_empty_text("pulls", OPEN_FILTER), "No open pull requests involve you");
        // All of them open: it would change nothing, so it isn't offered.
        let all_open = items("pulls", &answer(&[pull(1, "o/r", "me", ""), pull(2, "o/web", "sam", "")], &[]), NOW).unwrap();
        assert!(!filters("pulls", &all_open).iter().any(|option| option.id == OPEN_FILTER));
    }

    #[test]
    fn one_still_listed_as_open_after_closing_shows_once() {
        let open = pull(5, "o/r", "me", "");
        let closed = pull(5, "o/r", "me", r#","state":"MERGED","closedAt":"2026-11-09T23:00:00Z""#);
        let items = items("pulls", &answer(&[open], &[closed]), NOW).unwrap();
        assert_eq!(items.len(), 1);
    }

    #[test]
    fn issues_show_why_they_closed() {
        let issue = |number: u64, more: &str| {
            format!(r#"{{"number":{number},"title":"Bug {number}","url":"https://github.com/o/r/issues/{number}","state":"OPEN","stateReason":null,"updatedAt":"2026-11-09T00:00:00Z","closedAt":"2026-11-09T00:00:00Z","author":{{"login":"sam"}},"repository":{{"nameWithOwner":"o/r"}},"assignees":{{"nodes":[{{"login":"me"}}]}}{more}}}"#)
        };
        let json = answer(&[issue(1, "")], &[issue(2, r#","state":"CLOSED","stateReason":"COMPLETED""#), issue(3, r#","state":"CLOSED","stateReason":"NOT_PLANNED""#)]);
        let items = items("issues", &json, NOW).unwrap();
        assert_eq!(tags(&items[0]), ["Open"]);
        assert_eq!(items[0].facets, ["repo:o/r", "assigned", "open"]);
        assert_eq!(tags(&items[1]), ["Completed"]);
        assert_eq!(items[1].tint, Some(Tone::Purple));
        assert_eq!(tags(&items[2]), ["Not planned"]);
        assert_eq!(items[2].tint, Some(Tone::Grey));
        assert!(!items[1].facets.contains(&"assigned".to_string()), "assigned to you means there's still something to do");
        assert!(matches!(&items[0].actions[0].kind, ActionKind::Open(url) if url.ends_with("/issues/1")));
    }

    #[test]
    fn filters_offer_what_there_is() {
        let json = answer(&[pull(1, "o/r", "me", ""), pull(2, "o/web", "sam", r#","isDraft":true"#), pull(3, "o/web", "sam", "")], &[]);
        let all = items("pulls", &json, NOW).unwrap();
        let options = filters("pulls", &all);
        let offered: Vec<(&str, &str)> = options.iter().map(|option| (option.id.as_str(), option.subtitle.as_str())).collect();
        assert_eq!(offered, [("", ""), ("mine", "1"), ("draft", "1"), ("repo:o/web", "2"), ("repo:o/r", "1")], "nothing waits for your review, so that isn't offered");
        assert_eq!(options[0].title, "All pull requests");
        assert_eq!(apply_filter(all.clone(), "repo:o/web").len(), 2);
        assert_eq!(apply_filter(all.clone(), "mine").len(), 1);
        assert_eq!(apply_filter(all.clone(), "").len(), 3);
        let one_repo = items("pulls", &answer(&[pull(1, "o/r", "me", "")], &[]), NOW).unwrap();
        assert!(!filters("pulls", &one_repo).iter().any(|option| option.id.starts_with("repo:")), "one repository is no choice");
    }

    #[test]
    fn graphql_errors_say_what_github_said() {
        let refused = r#"{"data":null,"errors":[{"message":"Your token has not been granted the required scopes"}]}"#;
        assert_eq!(items("pulls", refused, NOW).unwrap_err(), "GitHub said: Your token has not been granted the required scopes");
        let partly = format!(r#"{{"data":{{"viewer":{{"login":"me"}},"open":{{"nodes":[{},null]}},"closed":{{"nodes":[]}}}},"errors":[{{"message":"one repository can't be read"}}]}}"#, pull(1, "o/r", "me", ""));
        assert_eq!(items("pulls", &partly, NOW).unwrap().len(), 1, "what could be read is shown");
    }

    #[test]
    fn closed_means_the_last_two_weeks() {
        let Some(Ask::Graphql { query, variables }) = ask("pulls", NOW) else { panic!() };
        assert!(query.contains("mergeable") && query.contains("statusCheckRollup"));
        assert_eq!(variables[0].1, "is:open is:pr involves:@me archived:false sort:updated-desc");
        assert_eq!(variables[1].1, "is:closed is:pr involves:@me archived:false closed:>=2026-10-27 sort:updated-desc");
        assert!(matches!(ask("issues", NOW), Some(Ask::Graphql { variables, .. }) if variables[0].1.contains("is:issue")));
        assert_eq!(ask("starred", NOW), Some(Ask::Rest("user/starred?per_page=100")));
    }

    #[test]
    fn dates_and_times_go_both_ways() {
        assert_eq!(date(0), "1970-01-01");
        assert_eq!(date(NOW), "2026-11-10");
        assert_eq!(date(951_782_400), "2000-02-29");
        for at in [0, NOW, 951_782_400, 1_709_164_800] {
            assert_eq!(rfc3339_to_unix(&format!("{}T00:00:00Z", date(at))), Some(at - at.rem_euclid(86_400)));
        }
        assert_eq!(rfc3339_to_unix("2026-11-12T18:59:59.655247+00:00"), Some(1_794_509_999));
        assert_eq!(rfc3339_to_unix("2026-11-12T19:59:59+01:00"), Some(1_794_509_999));
        assert_eq!(rfc3339_to_unix("2000-02-29T12:00:00-05:30"), Some(951_845_400));
        assert_eq!(rfc3339_to_unix("yesterday"), None);
        assert_eq!(rfc3339_to_unix("2026-13-01T00:00:00Z"), None);
        assert_eq!(span(59), "now");
        assert_eq!(span(3 * 3600 + 5 * 60), "3h 5m");
        assert_eq!(span(6 * 86_400 + 14 * 3600), "6d 14h");
    }

    #[test]
    fn notifications_open_the_page_not_the_api() {
        let json = r#"[{"id":"77","unread":true,"reason":"review_requested","updated_at":"2026-11-08T00:00:00Z","subject":{"title":"Add dark mode","url":"https://api.github.com/repos/o/r/pulls/9","type":"PullRequest"},"repository":{"full_name":"o/r","html_url":"https://github.com/o/r"}}]"#;
        let items = items("notifications", json, NOW).unwrap();
        assert_eq!(items[0].id, "77");
        assert_eq!(items[0].subtitle, "o/r \u{00B7} Review requested");
        assert_eq!(items[0].accessory, "2d ago");
        assert_eq!(items[0].facets, ["repo:o/r"]);
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
        assert!(items[0].tags.is_empty() && items[0].section.is_empty());
        assert_eq!(stars(999), "999");
        assert_eq!(stars(1000), "1k");
        assert_eq!(stars(1250), "1.2k");
        assert_eq!(stars(2_500_000), "2.5m");
    }

    #[test]
    fn errors_say_what_github_said() {
        assert_eq!(items("starred", r#"{"message":"Bad credentials"}"#, NOW).unwrap_err(), "GitHub said: Bad credentials");
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
        let pulls = items("pulls", &answer(&[pull(1, "o/r", "me", r#","mergeable":"CONFLICTING""#), pull(2, "o/r", "me", "")], &[]), NOW).unwrap();
        assert_eq!(filter(&pulls, "conflicts").len(), 1, "a tag's words find it too");
    }

    #[test]
    fn every_command_asks_for_something() {
        for command in ["issues", "pulls", "notifications", "starred"] {
            assert!(ask(command, NOW).is_some());
            assert!(crate::builtin_ext::BuiltinExt::GitHub.list_command(command).is_some());
        }
    }
}
