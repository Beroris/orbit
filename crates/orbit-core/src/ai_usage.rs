//! AI Usage: how much of their plan's limits someone has used in Claude Code and Codex, read from
//! what those services say, and turned into launcher rows. Nothing here goes online; the host asks
//! and passes the answers in.

use crate::extension::{ActionKind, Item, ItemAction, Meter};
use serde_json::Value;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Service {
    Claude,
    Codex,
}

impl Service {
    pub const ALL: [Service; 2] = [Service::Claude, Service::Codex];

    pub const fn id(self) -> &'static str {
        match self {
            Service::Claude => "claude",
            Service::Codex => "codex",
        }
    }

    pub const fn name(self) -> &'static str {
        match self {
            Service::Claude => "Claude",
            Service::Codex => "Codex",
        }
    }

    /// The app whose sign-in is read.
    pub const fn app(self) -> &'static str {
        match self {
            Service::Claude => "Claude Code",
            Service::Codex => "Codex",
        }
    }

    /// The service's own page about the same numbers.
    pub const fn usage_page(self) -> &'static str {
        match self {
            Service::Claude => "https://claude.ai/settings/usage",
            Service::Codex => "https://chatgpt.com/codex/settings/usage",
        }
    }
}

/// One limit: a share of something that refills at a time.
#[derive(Clone, Debug, PartialEq)]
pub struct Window {
    /// Short, as on a meter: "5h", "Week", "Opus".
    pub label: String,
    /// Percent used, 0 to 100.
    pub used: f32,
    /// When it refills, in seconds since 1970.
    pub resets_at: Option<i64>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Usage {
    pub service: Service,
    /// "Max", "Plus", or empty when it isn't known.
    pub plan: String,
    pub windows: Vec<Window>,
    /// When the numbers were true, if they weren't asked for just now.
    pub as_of: Option<i64>,
}

/// What can be said about one service.
#[derive(Clone, Debug, PartialEq)]
pub enum Reading {
    Usage(Usage),
    /// It's there, but its usage couldn't be read, and why, in words to act on.
    Problem { service: Service, text: String },
}

/// "max" is "Max", "plus" is "Plus", "claude_max" is "Max".
pub fn plan_name(raw: &str) -> String {
    let raw = raw.trim().trim_start_matches("claude_").trim_start_matches("chatgpt_");
    let mut chars = raw.chars();
    match chars.next() {
        Some(first) => first.to_uppercase().chain(chars.map(|c| if c == '_' { ' ' } else { c })).collect(),
        None => String::new(),
    }
}

fn number(value: &Value) -> Option<f64> {
    value.as_f64().or_else(|| value.as_str().and_then(|text| text.parse().ok()))
}

/// A limit's length as a meter says it.
fn window_label(seconds: i64) -> String {
    match seconds {
        0 => "Limit".into(),
        s if s % 604_800 == 0 && s / 604_800 == 1 => "Week".into(),
        s if s % 86_400 == 0 && s / 86_400 == 1 => "Day".into(),
        s if s % 86_400 == 0 => format!("{}d", s / 86_400),
        s if s % 3600 == 0 => format!("{}h", s / 3600),
        s => format!("{}m", (s + 59) / 60),
    }
}

/// Claude's answer to `api/oauth/usage`. `plan` is what Claude Code's sign-in says it is.
pub fn parse_claude(json: &str, plan: &str) -> Result<Usage, String> {
    let value: Value = serde_json::from_str(json).map_err(|_| "Claude's answer wasn't readable".to_string())?;
    let mut windows = Vec::new();
    let add = |label: &str, used: Option<f64>, resets: Option<&Value>, windows: &mut Vec<Window>| {
        if let Some(used) = used.filter(|used| used.is_finite()) {
            if !windows.iter().any(|window: &Window| window.label == label) {
                let resets_at = resets.and_then(Value::as_str).and_then(rfc3339_to_unix);
                windows.push(Window { label: label.to_string(), used: used.clamp(0.0, 100.0) as f32, resets_at });
            }
        }
    };
    for (key, label) in [("five_hour", "5h"), ("seven_day", "Week"), ("seven_day_opus", "Opus"), ("seven_day_sonnet", "Sonnet")] {
        if let Some(limit) = value.get(key).filter(|limit| limit.is_object()) {
            add(label, limit.get("utilization").and_then(number), limit.get("resets_at"), &mut windows);
        }
    }
    // The newer list, which also has limits for one model. A limit that isn't in force is left out.
    for limit in value.get("limits").and_then(Value::as_array).into_iter().flatten() {
        if limit.get("is_active").and_then(Value::as_bool) == Some(false) {
            continue;
        }
        let model = limit.pointer("/scope/model/display_name").and_then(Value::as_str).filter(|name| !name.is_empty());
        let label = match (model, limit.get("group").and_then(Value::as_str)) {
            (Some(model), _) => model.to_string(),
            (None, Some("weekly")) => "Week".into(),
            (None, Some("session" | "five_hour")) => "5h".into(),
            (None, Some(group)) => plan_name(group),
            (None, None) => continue,
        };
        add(&label, limit.get("percent").and_then(number), limit.get("resets_at"), &mut windows);
    }
    if windows.is_empty() {
        return Err("Claude didn't say how much of the plan is used".into());
    }
    Ok(Usage { service: Service::Claude, plan: plan_name(plan), windows, as_of: None })
}

fn codex_window(value: &Value, seconds_key: &str, minutes_key: &str) -> Option<Window> {
    let used = value.get("used_percent").and_then(number)?;
    let seconds = value.get(seconds_key).and_then(number).map(|s| s as i64).or_else(|| value.get(minutes_key).and_then(number).map(|m| m as i64 * 60)).unwrap_or(0);
    let resets_at = value.get("reset_at").or_else(|| value.get("resets_at")).and_then(number).map(|at| at as i64);
    Some(Window { label: window_label(seconds), used: used.clamp(0.0, 100.0) as f32, resets_at })
}

/// Codex's answer to `backend-api/wham/usage`.
pub fn parse_codex(json: &str) -> Result<Usage, String> {
    let value: Value = serde_json::from_str(json).map_err(|_| "Codex's answer wasn't readable".to_string())?;
    let limits = value.get("rate_limit").ok_or_else(|| "Codex didn't say how much of the plan is used".to_string())?;
    let windows: Vec<Window> = ["primary_window", "secondary_window"]
        .iter()
        .filter_map(|key| limits.get(*key))
        .filter_map(|window| codex_window(window, "limit_window_seconds", "window_minutes"))
        .collect();
    if windows.is_empty() {
        return Err("Codex didn't say how much of the plan is used".into());
    }
    let plan = value.get("plan_type").and_then(Value::as_str).unwrap_or("");
    Ok(Usage { service: Service::Codex, plan: plan_name(plan), windows, as_of: None })
}

/// The usage Codex wrote into one line of a session's log, as it last heard it, if the line has it.
/// `written_at` is when the log was last written.
pub fn parse_codex_log_line(line: &str, written_at: i64) -> Option<Usage> {
    if !line.contains("\"rate_limits\"") {
        return None;
    }
    let value: Value = serde_json::from_str(line).ok()?;
    let limits = value.pointer("/payload/rate_limits")?;
    let windows: Vec<Window> = ["primary", "secondary"].iter().filter_map(|key| limits.get(*key)).filter_map(|window| codex_window(window, "limit_window_seconds", "window_minutes")).collect();
    if windows.is_empty() {
        return None;
    }
    let plan = limits.get("plan_type").and_then(Value::as_str).unwrap_or("");
    Some(Usage { service: Service::Codex, plan: plan_name(plan), windows, as_of: Some(written_at) })
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

/// "5h 12%".
fn meter_label(window: &Window) -> String {
    format!("{} {}%", window.label, window.used.round() as i64)
}

/// A line that sums it up, for copying: "Claude (Max): 5h 12%, Week 50%".
pub fn summary(usage: &Usage) -> String {
    let windows: Vec<String> = usage.windows.iter().map(meter_label).collect();
    let plan = if usage.plan.is_empty() { String::new() } else { format!(" ({})", usage.plan) };
    format!("{}{plan}: {}", usage.service.name(), windows.join(", "))
}

fn open_page(service: Service) -> ItemAction {
    ItemAction { title: format!("Open {} Usage Page", service.name()), kind: ActionKind::Open(service.usage_page().into()), shortcut: None }
}

fn refresh() -> ItemAction {
    ItemAction { title: "Refresh".into(), kind: ActionKind::Callback("refresh".into()), shortcut: Some("Ctrl+R".into()) }
}

/// The rows: one for each service, with a meter for each of its limits. `now` is in seconds since 1970.
pub fn items(readings: &[Reading], now: i64) -> Vec<Item> {
    readings
        .iter()
        .map(|reading| match reading {
            Reading::Usage(usage) => {
                let mut parts = Vec::new();
                if !usage.plan.is_empty() {
                    parts.push(format!("{} plan", usage.plan));
                }
                // When the fullest limit refills is what matters most.
                if let Some(fullest) = usage.windows.iter().filter(|window| window.resets_at.is_some()).max_by(|a, b| a.used.total_cmp(&b.used)) {
                    let at = fullest.resets_at.unwrap_or(now);
                    let refill = if at <= now { format!("{} has reset", fullest.label) } else { format!("{} resets in {}", fullest.label, span(at - now)) };
                    parts.push(refill);
                }
                if let Some(as_of) = usage.as_of {
                    let age = now - as_of;
                    parts.push(if age < 60 { format!("from {} just now", usage.service.app()) } else { format!("from {} {} ago", usage.service.app(), span(age)) });
                }
                Item {
                    id: usage.service.id().into(),
                    title: usage.service.name().into(),
                    subtitle: parts.join(" \u{00B7} "),
                    accessory: String::new(),
                    icon: None,
                    actions: vec![
                        open_page(usage.service),
                        refresh(),
                        ItemAction { title: "Copy Usage".into(), kind: ActionKind::Copy(summary(usage)), shortcut: Some("Ctrl+Shift+C".into()) },
                    ],
                    meters: usage.windows.iter().take(crate::extension::MAX_METERS).map(|window| Meter { label: meter_label(window), value: window.used / 100.0 }).collect(),
                }
            }
            Reading::Problem { service, text } => Item {
                id: service.id().into(),
                title: service.name().into(),
                subtitle: text.clone(),
                accessory: String::new(),
                icon: None,
                actions: vec![open_page(*service), refresh()],
                meters: Vec::new(),
            },
        })
        .collect()
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

#[cfg(test)]
mod tests {
    use super::*;

    // As Claude answered on 2026-11-09, cut down.
    const CLAUDE: &str = r#"{"five_hour":null,"seven_day":{"utilization":50.0,"resets_at":"2026-11-12T18:59:59.655247+00:00"},"seven_day_opus":null,"seven_day_sonnet":null,"extra_usage":{"is_enabled":false},"limits":[{"kind":"weekly_all","group":"weekly","percent":50,"severity":"normal","resets_at":"2026-11-12T18:59:59.655247+00:00","scope":null,"is_active":true},{"kind":"weekly_scoped","group":"weekly","percent":0,"severity":"normal","resets_at":"2026-11-12T19:00:00+00:00","scope":{"model":{"id":null,"display_name":"Fable"},"surface":null},"is_active":false}]}"#;
    // As Codex answered on 2026-11-09, without who it was.
    const CODEX: &str = r#"{"plan_type":"plus","rate_limit":{"allowed":true,"limit_reached":false,"primary_window":{"used_percent":0,"limit_window_seconds":18000,"reset_after_seconds":17326,"reset_at":1794280136},"secondary_window":{"used_percent":3,"limit_window_seconds":604800,"reset_after_seconds":436916,"reset_at":1794699727}},"credits":{"has_credits":false}}"#;
    const CODEX_LOG: &str = r#"{"timestamp":"2026-11-07T09:07:17.000Z","type":"event_msg","payload":{"type":"token_count","info":null,"rate_limits":{"limit_id":"codex","primary":{"used_percent":6.0,"window_minutes":300,"resets_at":1794060097},"secondary":{"used_percent":98.0,"window_minutes":10080,"resets_at":1794384053},"plan_type":"plus"}}}"#;

    #[test]
    fn times_are_read_in_any_zone() {
        assert_eq!(rfc3339_to_unix("1970-01-01T00:00:00Z"), Some(0));
        assert_eq!(rfc3339_to_unix("2026-11-12T18:59:59.655247+00:00"), Some(1_794_509_999));
        assert_eq!(rfc3339_to_unix("2026-11-12T19:59:59+01:00"), Some(1_794_509_999));
        assert_eq!(rfc3339_to_unix("2000-02-29T12:00:00-05:30"), Some(951_845_400));
        assert_eq!(rfc3339_to_unix("yesterday"), None);
        assert_eq!(rfc3339_to_unix("2026-13-01T00:00:00Z"), None);
    }

    #[test]
    fn claude_shows_the_limits_in_force() {
        let usage = parse_claude(CLAUDE, "max").unwrap();
        assert_eq!(usage.plan, "Max");
        assert_eq!(usage.windows, vec![Window { label: "Week".into(), used: 50.0, resets_at: Some(1_794_509_999) }], "the five-hour one is null, and Fable's isn't active");
        let with_session = CLAUDE.replace(r#""five_hour":null"#, r#""five_hour":{"utilization":12.5,"resets_at":"2026-11-09T23:00:00Z"}"#).replace(r#""is_active":false"#, r#""is_active":true"#);
        let labels: Vec<String> = parse_claude(&with_session, "pro").unwrap().windows.iter().map(|w| w.label.clone()).collect();
        assert_eq!(labels, ["5h", "Week", "Fable"]);
        assert!(parse_claude("{}", "max").is_err());
        assert!(parse_claude("<html>", "max").is_err());
    }

    #[test]
    fn codex_shows_both_windows() {
        let usage = parse_codex(CODEX).unwrap();
        assert_eq!(usage.plan, "Plus");
        assert_eq!(usage.windows.len(), 2);
        assert_eq!((usage.windows[0].label.as_str(), usage.windows[0].used), ("5h", 0.0));
        assert_eq!((usage.windows[1].label.as_str(), usage.windows[1].used, usage.windows[1].resets_at), ("Week", 3.0, Some(1_794_699_727)));
        assert!(parse_codex(r#"{"plan_type":"plus"}"#).is_err());
    }

    #[test]
    fn codex_logs_are_read_when_offline() {
        let usage = parse_codex_log_line(CODEX_LOG, 1_794_041_237).unwrap();
        assert_eq!(usage.windows.iter().map(|w| (w.label.as_str(), w.used)).collect::<Vec<_>>(), [("5h", 6.0), ("Week", 98.0)]);
        assert_eq!(usage.as_of, Some(1_794_041_237));
        assert!(parse_codex_log_line(r#"{"type":"response_item","payload":{"type":"message"}}"#, 0).is_none());
    }

    #[test]
    fn rows_have_meters_and_say_when_the_fullest_refills() {
        let now = 1_794_509_999 - (2 * 86_400 + 20 * 3600 + 120);
        let rows = items(&[Reading::Usage(parse_claude(CLAUDE, "max").unwrap())], now);
        assert_eq!(rows[0].title, "Claude");
        assert_eq!(rows[0].subtitle, "Max plan \u{00B7} Week resets in 2d 20h");
        assert_eq!(rows[0].meters, vec![Meter { label: "Week 50%".into(), value: 0.5 }]);
        assert!(matches!(&rows[0].actions[0].kind, ActionKind::Open(url) if url.contains("claude.ai")));
        assert!(matches!(&rows[0].actions[2].kind, ActionKind::Copy(text) if text == "Claude (Max): Week 50%"));
        let old = items(&[Reading::Usage(parse_codex_log_line(CODEX_LOG, now - 3 * 86_400).unwrap())], now);
        assert!(old[0].subtitle.ends_with("from Codex 3d 0h ago"), "{}", old[0].subtitle);
    }

    #[test]
    fn a_problem_is_a_row_too() {
        let items = items(&[Reading::Problem { service: Service::Codex, text: "Codex's sign-in has expired. Open Codex to sign in again.".into() }], 0);
        assert_eq!(items[0].title, "Codex");
        assert!(items[0].meters.is_empty());
        assert_eq!(items[0].actions.len(), 2);
    }

    #[test]
    fn spans_and_plans_read_well() {
        assert_eq!(span(59), "now");
        assert_eq!(span(12 * 60), "12m");
        assert_eq!(span(3 * 3600 + 5 * 60), "3h 5m");
        assert_eq!(span(6 * 86_400 + 14 * 3600), "6d 14h");
        assert_eq!(plan_name("claude_max"), "Max");
        assert_eq!(plan_name("plus"), "Plus");
        assert_eq!(plan_name(""), "");
        assert_eq!(window_label(18_000), "5h");
        assert_eq!(window_label(604_800), "Week");
    }
}
