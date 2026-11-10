//! The extensions that come with Orbit and answer with a list the way a folder extension's program
//! does: GitHub, through GitHub's own `gh` tool, and AI Usage, from Claude Code's and Codex's
//! sign-ins. Each question is answered on a thread of its own, and the answer comes back as
//! `WM_ORBIT_EXTENSION`, the same as a program's, so the launcher shows both the same way.
//!
//! Nothing runs while they're off, and nothing is kept on disk: answers are kept in memory for a
//! minute, so going back into a list doesn't ask again.

use crate::extensions::{Event, EventKind};
use crate::util::{SendHwnd, box_for_post};
use orbit_core::ai_usage::{self, Reading, Service};
use orbit_core::builtin_ext::BuiltinExt;
use orbit_core::extension::{Item, Message};
use orbit_core::github;
use std::collections::HashMap;
use std::io::Read;
use std::os::windows::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

const CREATE_NO_WINDOW: u32 = 0x0800_0000;
const CREATE_NEW_CONSOLE: u32 = 0x0000_0010;
/// How long an answer is shown again without asking.
const FRESH_FOR: Duration = Duration::from_secs(60);
const GH_TIMEOUT: Duration = Duration::from_secs(20);
const INSTALL_TIMEOUT: Duration = Duration::from_secs(300);
/// The most of an answer that's read.
const MOST_BYTES: usize = 4 * 1024 * 1024;
pub const GH_WINGET_ID: &str = "GitHub.cli";

fn now() -> i64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |since| since.as_secs() as i64)
}

type Key = (BuiltinExt, String);

#[derive(Default)]
struct State {
    /// Answers, and when they came.
    kept: HashMap<Key, (Instant, Vec<Item>)>,
    /// Lists being fetched now, and the newest question to answer when each comes. Typing while
    /// one is on its way waits for it instead of asking again.
    asking: HashMap<Key, (u64, String)>,
}

/// Answers kept for a minute, by extension and command.
#[derive(Clone, Default)]
pub struct Providers {
    state: Arc<Mutex<State>>,
}

impl Providers {
    /// The kept answer, if it's from the last minute or `any_age`.
    fn kept(&self, key: &Key, any_age: bool) -> Option<Vec<Item>> {
        let state = self.state.lock().ok()?;
        state.kept.get(key).filter(|(at, _)| any_age || at.elapsed() < FRESH_FOR).map(|(_, items)| items.clone())
    }

    fn keep(&self, key: Key, items: &[Item]) {
        if let Ok(mut state) = self.state.lock() {
            state.kept.insert(key, (Instant::now(), items.to_vec()));
        }
    }

    /// Forgets every answer, when an extension is turned off.
    pub fn forget(&self) {
        if let Ok(mut state) = self.state.lock() {
            state.kept.clear();
        }
    }

    /// Answers a search of a command's list, as request `id`. `fresh` asks again even if an answer
    /// from the last minute is kept.
    #[allow(clippy::too_many_arguments)]
    pub fn search(&self, ext: BuiltinExt, command: &str, id: u64, query: &str, fresh: bool, main: SendHwnd, message: u32) {
        let key: Key = (ext, command.to_string());
        if !fresh {
            if let Some(items) = self.kept(&key, false) {
                post(main, message, ext, answer(ext, command, id, query, items));
                return;
            }
        }
        {
            let Ok(mut state) = self.state.lock() else { return };
            if let Some(asked) = state.asking.get_mut(&key) {
                *asked = (id, query.to_string());
                return;
            }
            state.asking.insert(key.clone(), (id, query.to_string()));
        }
        let this = self.clone();
        let started = std::thread::Builder::new().name(format!("orbit-{}", ext.id())).spawn(move || {
            let fetched = fetch(ext, &key.1);
            let (id, query) = {
                let Ok(mut state) = this.state.lock() else { return };
                if let Ok(items) = &fetched {
                    state.kept.insert(key.clone(), (Instant::now(), items.clone()));
                }
                state.asking.remove(&key).unwrap_or_default()
            };
            post(main, message, ext, match fetched {
                Ok(items) => answer(ext, &key.1, id, &query, items),
                Err(why) => Message::Error { id, message: why },
            });
        });
        if started.is_err() {
            if let Ok(mut state) = self.state.lock() {
                state.asking.remove(&(ext, command.to_string()));
            }
            post(main, message, ext, Message::Error { id, message: "Couldn't start asking.".into() });
        }
    }

    /// An item's `callback` action: refresh, or mark a GitHub notification read.
    #[allow(clippy::too_many_arguments)]
    pub fn action(&self, ext: BuiltinExt, command: &str, item: &str, action: &str, id: u64, query: &str, main: SendHwnd, message: u32) {
        if (ext, action) != (BuiltinExt::GitHub, "read") {
            self.search(ext, command, id, query, true, main, message);
            return;
        }
        // Only a notification's own id goes into the address.
        if item.is_empty() || !item.chars().all(|c| c.is_ascii_digit()) {
            post(main, message, ext, Message::Error { id, message: "That notification can't be marked.".into() });
            return;
        }
        let this = self.clone();
        let key: Key = (ext, command.to_string());
        let (item, query) = (item.to_string(), query.to_string());
        let _ = std::thread::Builder::new().name("orbit-github-read".into()).spawn(move || {
            let said = match gh(&["api", "-X", "PATCH", &format!("notifications/threads/{item}")]) {
                Ok(_) => {
                    // GitHub takes a moment to stop listing it, so it's taken out here.
                    let mut items = this.kept(&key, true).unwrap_or_default();
                    items.retain(|kept| kept.id != item);
                    this.keep(key.clone(), &items);
                    answer(ext, &key.1, id, &query, items)
                }
                Err(why) => Message::Error { id, message: why },
            };
            post(main, message, ext, said);
        });
    }
}

fn post(main: SendHwnd, message: u32, ext: BuiltinExt, said: Message) {
    main.post(message, box_for_post(Event { ext: ext.id().to_string(), run: 0, kind: EventKind::Said(said) }), 0);
}

/// What a hidden preview shows: answers saved in the data folder, never anything asked online.
pub fn preview(ext: BuiltinExt, command: &str, id: u64, query: &str, dir: &Path) -> Message {
    let read = |name: &str| std::fs::read_to_string(dir.join(name)).unwrap_or_default();
    let items = match ext {
        BuiltinExt::GitHub => github::items(command, &read(&format!("github-preview-{command}.json")), now()),
        BuiltinExt::AiUsage => {
            let mut readings = Vec::new();
            if let Ok(usage) = ai_usage::parse_claude(&read("ai-usage-preview-claude.json"), "max") {
                readings.push(Reading::Usage(usage));
            }
            if let Ok(usage) = ai_usage::parse_codex(&read("ai-usage-preview-codex.json")) {
                readings.push(Reading::Usage(usage));
            }
            Ok(ai_usage::items(&readings, now()))
        }
        _ => Ok(Vec::new()),
    };
    match items {
        Ok(items) => answer(ext, command, id, query, items),
        Err(message) => Message::Error { id, message },
    }
}

/// The rows for what's typed, from everything fetched.
fn answer(ext: BuiltinExt, command: &str, id: u64, query: &str, items: Vec<Item>) -> Message {
    let (items, empty) = match ext {
        BuiltinExt::GitHub if query.trim().is_empty() => (items, github::empty_text(command).to_string()),
        BuiltinExt::GitHub => (github::filter(&items, query), "Nothing matches that".to_string()),
        _ => (github::filter(&items, query), "Claude Code and Codex aren't signed in on this PC".to_string()),
    };
    Message::Items { id, items, empty: Some(empty) }
}

fn fetch(ext: BuiltinExt, command: &str) -> Result<Vec<Item>, String> {
    match ext {
        BuiltinExt::GitHub => {
            let path = github::api_path(command).ok_or_else(|| "GitHub has no such list.".to_string())?;
            let json = gh(&["api", path])?;
            github::items(command, &json, now())
        }
        BuiltinExt::AiUsage => Ok(ai_usage::items(&ai_readings(), now())),
        _ => Err("That isn't a list.".into()),
    }
}

// --- GitHub ------------------------------------------------------------------------------------

/// Where `gh` is, if it's installed. Orbit may have started before it was, so the saved PATHs and
/// its usual folder are looked in too.
pub fn gh_path() -> Option<PathBuf> {
    crate::integrations::find_on_path("gh.exe").or_else(|| {
        let usual = PathBuf::from(std::env::var_os("ProgramFiles").unwrap_or_else(|| "C:\\Program Files".into())).join("GitHub CLI").join("gh.exe");
        usual.is_file().then_some(usual)
    })
}

fn gh_config_dir() -> Option<PathBuf> {
    if let Some(dir) = std::env::var_os("GH_CONFIG_DIR").filter(|dir| !dir.is_empty()) {
        return Some(PathBuf::from(dir));
    }
    std::env::var_os("APPDATA").map(|appdata| PathBuf::from(appdata).join("GitHub CLI"))
}

/// Who `gh` is signed in as on github.com, from its own settings file. Its token isn't there, and
/// isn't read.
pub fn gh_login() -> Option<String> {
    let text = std::fs::read_to_string(gh_config_dir()?.join("hosts.yml")).ok()?;
    signed_in_user(&text)
}

/// The `user:` under `github.com:` in gh's hosts.yml.
fn signed_in_user(hosts: &str) -> Option<String> {
    let mut in_github = false;
    for line in hosts.lines() {
        if !line.starts_with([' ', '\t']) {
            in_github = line.trim_end() == "github.com:";
            continue;
        }
        if in_github {
            if let Some(user) = line.trim().strip_prefix("user:") {
                let user = user.trim().trim_matches(['"', '\'']);
                if !user.is_empty() {
                    return Some(user.to_string());
                }
            }
        }
    }
    None
}

/// Runs `gh` with no window and gives back what it printed. When gh has a sign-in of its own, a
/// GITHUB_TOKEN or GH_TOKEN left in the environment is set aside: gh would use it first, and an old
/// one makes every request fail with "Bad credentials".
fn gh(args: &[&str]) -> Result<String, String> {
    let program = gh_path().ok_or_else(|| "GitHub's command-line tool isn't installed. Install it from Settings, under Extensions.".to_string())?;
    let mut command = Command::new(program);
    command.args(args).stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::piped()).creation_flags(CREATE_NO_WINDOW).env("GH_PROMPT_DISABLED", "1").env("NO_COLOR", "1");
    if gh_login().is_some() {
        command.env_remove("GITHUB_TOKEN").env_remove("GH_TOKEN");
    }
    let mut child = command.spawn().map_err(|_| "GitHub's command-line tool wouldn't start.".to_string())?;
    let (status, stdout, stderr) = finish(&mut child, GH_TIMEOUT).ok_or_else(|| "GitHub took too long to answer.".to_string())?;
    if status {
        return Ok(stdout);
    }
    Err(gh_problem(&stderr, &stdout))
}

/// What went wrong, in words to act on.
fn gh_problem(stderr: &str, stdout: &str) -> String {
    let lower = format!("{stderr} {stdout}").to_lowercase();
    if lower.contains("gh auth login") || lower.contains("not logged in") || lower.contains("authentication required") {
        "Sign in to GitHub first, from Settings, under Extensions.".into()
    } else if lower.contains("bad credentials") || lower.contains("http 401") {
        "GitHub didn't accept the sign-in. Sign in again from Settings, under Extensions.".into()
    } else if lower.contains("http 403") && lower.contains("rate limit") {
        "GitHub asked Orbit to wait a while before asking again.".into()
    } else if lower.contains("http 403") || lower.contains("http 404") {
        "GitHub's sign-in doesn't allow this. Sign in again, allowing notifications and repositories.".into()
    } else if lower.contains("dial tcp") || lower.contains("no such host") || lower.contains("connect") || lower.contains("timeout") {
        "Couldn't reach GitHub. Check your connection.".into()
    } else {
        let first = stderr.lines().map(str::trim).find(|line| !line.is_empty()).unwrap_or("GitHub's tool stopped without saying why.");
        first.chars().take(140).collect()
    }
}

/// Waits for a program, reading what it prints as it goes so it can't fill a pipe and stall.
/// `None` if it ran out of time, and then it's ended.
fn finish(child: &mut std::process::Child, timeout: Duration) -> Option<(bool, String, String)> {
    let mut out = child.stdout.take();
    let mut err = child.stderr.take();
    let read_out = std::thread::spawn(move || {
        let mut bytes = Vec::new();
        if let Some(pipe) = out.as_mut() {
            let _ = pipe.take(MOST_BYTES as u64).read_to_end(&mut bytes);
        }
        bytes
    });
    let read_err = std::thread::spawn(move || {
        let mut bytes = Vec::new();
        if let Some(pipe) = err.as_mut() {
            let _ = pipe.take(64 * 1024).read_to_end(&mut bytes);
        }
        bytes
    });
    let started = Instant::now();
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break Some(status.success()),
            Ok(None) if started.elapsed() >= timeout => {
                let _ = child.kill();
                let _ = child.wait();
                break None;
            }
            Ok(None) => std::thread::sleep(Duration::from_millis(20)),
            Err(_) => break None,
        }
    };
    let stdout = String::from_utf8_lossy(&read_out.join().unwrap_or_default()).into_owned();
    let stderr = String::from_utf8_lossy(&read_err.join().unwrap_or_default()).into_owned();
    status.map(|ok| (ok, stdout, stderr))
}

/// What Settings shows about GitHub's tool.
#[derive(Clone, Debug, PartialEq)]
pub enum GitHubState {
    NotInstalled,
    SignedOut,
    SignedIn(String),
}

pub fn github_state() -> GitHubState {
    if gh_path().is_none() {
        GitHubState::NotInstalled
    } else {
        gh_login().map_or(GitHubState::SignedOut, GitHubState::SignedIn)
    }
}

/// Installs GitHub's tool with Windows Package Manager. Gives back what to say, and whether it worked.
pub fn install_gh() -> (String, bool) {
    let Some(winget) = crate::integrations::winget() else {
        return ("Windows Package Manager isn't available. Get GitHub's tool from cli.github.com.".into(), false);
    };
    let args = ["install", "--id", GH_WINGET_ID, "-e", "--silent", "--accept-source-agreements", "--accept-package-agreements", "--disable-interactivity"];
    let worked = Command::new(winget).args(args).stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::piped()).creation_flags(CREATE_NO_WINDOW).spawn().ok().and_then(|mut child| finish(&mut child, INSTALL_TIMEOUT)).is_some_and(|(ok, _, _)| ok);
    if worked || gh_path().is_some() {
        ("GitHub's tool is installed. Now sign in.".into(), true)
    } else {
        ("Windows Package Manager couldn't install it. Check your connection and try again.".into(), false)
    }
}

/// Opens a console where GitHub's own sign-in runs, in the browser. Orbit never sees the
/// password or the token. When the console closes, `done` is called.
pub fn open_gh_sign_in(done: impl FnOnce() + Send + 'static) -> Result<(), String> {
    let program = gh_path().ok_or_else(|| "Install GitHub's tool first.".to_string())?;
    let line = format!("/c \"\"{}\" auth login --hostname github.com --git-protocol https --web --scopes notifications & echo. & pause\"", program.display());
    let mut command = Command::new("cmd.exe");
    command.raw_arg(line).creation_flags(CREATE_NEW_CONSOLE);
    // An old token in the environment would stop gh from signing in at all.
    command.env_remove("GITHUB_TOKEN").env_remove("GH_TOKEN");
    let mut child = command.spawn().map_err(|_| "Couldn't open a window to sign in.".to_string())?;
    let _ = std::thread::Builder::new().name("orbit-github-sign-in".into()).spawn(move || {
        let _ = child.wait();
        done();
    });
    Ok(())
}

// --- AI Usage ----------------------------------------------------------------------------------

fn home() -> Option<PathBuf> {
    std::env::var_os("USERPROFILE").map(PathBuf::from)
}

fn claude_dir() -> Option<PathBuf> {
    std::env::var_os("CLAUDE_CONFIG_DIR").filter(|dir| !dir.is_empty()).map(PathBuf::from).or_else(|| Some(home()?.join(".claude")))
}

fn codex_dir() -> Option<PathBuf> {
    std::env::var_os("CODEX_HOME").filter(|dir| !dir.is_empty()).map(PathBuf::from).or_else(|| Some(home()?.join(".codex")))
}

/// Which of the services are signed in on this PC, for Settings to say.
pub fn ai_services_found() -> Vec<Service> {
    let mut found = Vec::new();
    if claude_dir().is_some_and(|dir| dir.join(".credentials.json").is_file()) {
        found.push(Service::Claude);
    }
    if codex_dir().is_some_and(|dir| dir.join("auth.json").is_file()) {
        found.push(Service::Codex);
    }
    found
}

/// Each service that's signed in on this PC, asked now. A service with no sign-in here is left out.
pub fn ai_readings() -> Vec<Reading> {
    let (claude, codex) = std::thread::scope(|scope| {
        let claude = scope.spawn(claude_reading);
        let codex = scope.spawn(codex_reading);
        (claude.join().ok().flatten(), codex.join().ok().flatten())
    });
    claude.into_iter().chain(codex).collect()
}

fn problem(service: Service, text: impl Into<String>) -> Reading {
    Reading::Problem { service, text: text.into() }
}

/// Says why a signed-in request was turned down, for one service.
fn refused(service: Service, failure: crate::favicon::Failure) -> String {
    use crate::favicon::Failure;
    match failure {
        Failure::Status(401 | 403) => format!("It didn't accept {}'s sign-in. Use {} once to renew it.", service.app(), service.app()),
        Failure::Status(429) => format!("{} asked Orbit to wait. Try again in a minute.", service.name()),
        Failure::Status(status) => format!("{} answered with an error ({status}). Try again later.", service.name()),
        Failure::Unreachable => format!("Couldn't reach {}. Check your connection.", service.name()),
    }
}

/// Claude: Claude Code's sign-in, sent only to Anthropic's own server to ask for its usage. The
/// sign-in is never renewed here, since renewing it would sign Claude Code out.
fn claude_reading() -> Option<Reading> {
    let file = claude_dir()?.join(".credentials.json");
    let text = std::fs::read_to_string(file).ok()?;
    let value: serde_json::Value = serde_json::from_str(&text).ok()?;
    let oauth = value.get("claudeAiOauth")?;
    let token = oauth.get("accessToken").and_then(serde_json::Value::as_str).filter(|token| !token.is_empty())?;
    let plan = oauth.get("subscriptionType").and_then(serde_json::Value::as_str).unwrap_or("");
    if oauth.get("expiresAt").and_then(serde_json::Value::as_i64).is_some_and(|ms| ms / 1000 < now()) {
        return Some(problem(Service::Claude, "Claude Code's sign-in has expired. Use Claude Code once to renew it."));
    }
    let bearer = format!("Bearer {token}");
    let headers = [("Authorization", bearer.as_str()), ("anthropic-beta", "oauth-2025-04-20"), ("Accept", "application/json")];
    Some(match crate::favicon::get_signed_in("https://api.anthropic.com/api/oauth/usage", &headers, 512 * 1024) {
        Ok(body) => match ai_usage::parse_claude(&String::from_utf8_lossy(&body), plan) {
            Ok(usage) => Reading::Usage(usage),
            Err(why) => problem(Service::Claude, why),
        },
        Err(failure) => problem(Service::Claude, refused(Service::Claude, failure)),
    })
}

/// Codex: its sign-in, sent only to OpenAI's own server. When that can't answer, what Codex last
/// wrote in a session's log is shown instead, saying how old it is.
fn codex_reading() -> Option<Reading> {
    let dir = codex_dir()?;
    let text = std::fs::read_to_string(dir.join("auth.json")).ok()?;
    let value: serde_json::Value = serde_json::from_str(&text).ok()?;
    let token = value.pointer("/tokens/access_token").and_then(serde_json::Value::as_str).filter(|token| !token.is_empty());
    let Some(token) = token else {
        return Some(problem(Service::Codex, "Codex is signed in with an API key, which has no plan limits to show."));
    };
    let account = value.pointer("/tokens/account_id").and_then(serde_json::Value::as_str).unwrap_or("");
    let bearer = format!("Bearer {token}");
    let mut headers = vec![("Authorization", bearer.as_str()), ("Accept", "application/json")];
    if !account.is_empty() {
        headers.push(("ChatGPT-Account-Id", account));
    }
    let failure = match crate::favicon::get_signed_in("https://chatgpt.com/backend-api/wham/usage", &headers, 512 * 1024) {
        Ok(body) => match ai_usage::parse_codex(&String::from_utf8_lossy(&body)) {
            Ok(usage) => return Some(Reading::Usage(usage)),
            Err(why) => why,
        },
        Err(failure) => refused(Service::Codex, failure),
    };
    Some(codex_from_log(&dir).map_or_else(|| problem(Service::Codex, failure), Reading::Usage))
}

/// The newest session log under `sessions\YYYY\MM\DD`.
fn newest_log(dir: &Path) -> Option<PathBuf> {
    let mut at = dir.join("sessions");
    // Year, month, then day folders, newest name first.
    for _ in 0..3 {
        let mut names: Vec<PathBuf> = std::fs::read_dir(&at).ok()?.flatten().map(|entry| entry.path()).filter(|path| path.is_dir()).collect();
        names.sort();
        at = names.pop()?;
    }
    std::fs::read_dir(&at)
        .ok()?
        .flatten()
        .filter(|entry| entry.path().extension().is_some_and(|ext| ext == "jsonl"))
        .max_by_key(|entry| entry.metadata().and_then(|meta| meta.modified()).ok())
        .map(|entry| entry.path())
}

fn codex_from_log(dir: &Path) -> Option<ai_usage::Usage> {
    let file = newest_log(dir)?;
    let written = std::fs::metadata(&file).and_then(|meta| meta.modified()).ok()?.duration_since(std::time::UNIX_EPOCH).ok()?.as_secs() as i64;
    // Logs can be large; the usage is near the end.
    let bytes = std::fs::read(&file).ok()?;
    let tail = &bytes[bytes.len().saturating_sub(MOST_BYTES)..];
    String::from_utf8_lossy(tail).lines().rev().find_map(|line| ai_usage::parse_codex_log_line(line, written))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_signed_in_user_is_read_from_hosts() {
        let hosts = "github.com:\n    users:\n        Beroris:\n    git_protocol: https\n    user: Beroris\n";
        assert_eq!(signed_in_user(hosts).as_deref(), Some("Beroris"));
        assert_eq!(signed_in_user("ghe.example.com:\n    user: someone\n"), None, "only github.com");
        assert_eq!(signed_in_user(""), None);
    }

    #[test]
    fn gh_problems_say_what_to_do() {
        assert!(gh_problem("To get started with GitHub CLI, please run:  gh auth login", "").contains("Sign in"));
        assert!(gh_problem("gh: Bad credentials (HTTP 401)", "").contains("didn't accept"));
        assert!(gh_problem("error connecting to api.github.com: dial tcp: lookup api.github.com: no such host", "").contains("Couldn't reach"));
        assert_eq!(gh_problem("something odd\nmore", ""), "something odd");
    }

    #[test]
    fn searching_kept_answers_narrows_them() {
        let items = github::items("starred", r#"[{"full_name":"a/one","html_url":"https://github.com/a/one"},{"full_name":"b/two","html_url":"https://github.com/b/two"}]"#, 0).unwrap();
        match answer(BuiltinExt::GitHub, "starred", 7, "two", items) {
            Message::Items { id, items, .. } => {
                assert_eq!(id, 7);
                assert_eq!(items.len(), 1);
            }
            _ => panic!("expected items"),
        }
    }
}
