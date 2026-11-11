//! The extensions that come with Orbit and answer with a list the way a folder extension's program
//! does: GitHub, through GitHub's own `gh` tool. Each question is answered on a thread of its own,
//! and the answer comes back as `WM_ORBIT_EXTENSION`, the same as a program's, so the launcher
//! shows both the same way.
//!
//! Nothing runs while they're off, and nothing is kept on disk: answers are kept in memory for a
//! minute, so going back into a list, typing in it or choosing a filter doesn't ask again.

use crate::extensions::{Event, EventKind};
use crate::util::{SendHwnd, box_for_post};
use orbit_core::builtin_ext::BuiltinExt;
use orbit_core::extension::{Item, Message};
use orbit_core::github::{self, Ask};
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

/// A question about a list: its request id, what's typed, and the filter chosen.
#[derive(Clone, Default)]
struct Question {
    id: u64,
    query: String,
    filter: String,
}

#[derive(Default)]
struct State {
    /// Answers, and when they came.
    kept: HashMap<Key, (Instant, Vec<Item>)>,
    /// Lists being fetched now, and the newest question to answer when each comes. Typing while
    /// one is on its way waits for it instead of asking again.
    asking: HashMap<Key, Question>,
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

    /// Answers a search of a command's list, as request `id`, narrowed by `filter`. `fresh` asks
    /// again even if an answer from the last minute is kept.
    #[allow(clippy::too_many_arguments)]
    pub fn search(&self, ext: BuiltinExt, command: &str, id: u64, query: &str, filter: &str, fresh: bool, main: SendHwnd, message: u32) {
        let key: Key = (ext, command.to_string());
        let question = Question { id, query: query.to_string(), filter: filter.to_string() };
        if !fresh {
            if let Some(items) = self.kept(&key, false) {
                post(main, message, ext, answer(ext, command, &question, items));
                return;
            }
        }
        {
            let Ok(mut state) = self.state.lock() else { return };
            if let Some(asked) = state.asking.get_mut(&key) {
                *asked = question;
                return;
            }
            state.asking.insert(key.clone(), question);
        }
        let this = self.clone();
        let started = std::thread::Builder::new().name(format!("orbit-{}", ext.id())).spawn(move || {
            let fetched = fetch(ext, &key.1);
            let question = {
                let Ok(mut state) = this.state.lock() else { return };
                if let Ok(items) = &fetched {
                    state.kept.insert(key.clone(), (Instant::now(), items.clone()));
                }
                state.asking.remove(&key).unwrap_or_default()
            };
            post(main, message, ext, match fetched {
                Ok(items) => answer(ext, &key.1, &question, items),
                Err(why) => Message::Error { id: question.id, message: why },
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
    pub fn action(&self, ext: BuiltinExt, command: &str, item: &str, action: &str, id: u64, query: &str, filter: &str, main: SendHwnd, message: u32) {
        if (ext, action) != (BuiltinExt::GitHub, "read") {
            self.search(ext, command, id, query, filter, true, main, message);
            return;
        }
        // Only a notification's own id goes into the address.
        if item.is_empty() || !item.chars().all(|c| c.is_ascii_digit()) {
            post(main, message, ext, Message::Error { id, message: "That notification can't be marked.".into() });
            return;
        }
        let this = self.clone();
        let key: Key = (ext, command.to_string());
        let item = item.to_string();
        let question = Question { id, query: query.to_string(), filter: filter.to_string() };
        let _ = std::thread::Builder::new().name("orbit-github-read".into()).spawn(move || {
            let said = match gh(&["api", "-X", "PATCH", &format!("notifications/threads/{item}")]) {
                Ok(_) => {
                    // GitHub takes a moment to stop listing it, so it's taken out here.
                    let mut items = this.kept(&key, true).unwrap_or_default();
                    items.retain(|kept| kept.id != item);
                    this.keep(key.clone(), &items);
                    answer(ext, &key.1, &question, items)
                }
                Err(why) => Message::Error { id: question.id, message: why },
            };
            post(main, message, ext, said);
        });
    }
}

fn post(main: SendHwnd, message: u32, ext: BuiltinExt, said: Message) {
    main.post(message, box_for_post(Event { ext: ext.id().to_string(), run: 0, kind: EventKind::Said(said) }), 0);
}

/// What a hidden preview shows: answers saved in the data folder, never anything asked online.
pub fn preview(ext: BuiltinExt, command: &str, id: u64, query: &str, filter: &str, dir: &Path) -> Message {
    let read = |name: &str| std::fs::read_to_string(dir.join(name)).unwrap_or_default();
    let items = match ext {
        BuiltinExt::GitHub => github::items(command, &read(&format!("github-preview-{command}.json")), now()),
        _ => Ok(Vec::new()),
    };
    let question = Question { id, query: query.to_string(), filter: filter.to_string() };
    match items {
        Ok(items) => answer(ext, command, &question, items),
        Err(message) => Message::Error { id, message },
    }
}

/// The rows for what's typed and the filter chosen, from everything fetched, and the filters on
/// offer. Those come from everything fetched, so choosing one doesn't take the others away.
fn answer(ext: BuiltinExt, command: &str, question: &Question, items: Vec<Item>) -> Message {
    let filters = github::filters(command, &items);
    // A filter for a repository that's gone from the list is let go.
    let filter = if filters.iter().any(|option| option.id == question.filter) { question.filter.as_str() } else { "" };
    let narrowed = github::apply_filter(items, filter);
    let empty = match ext {
        BuiltinExt::GitHub if !question.query.trim().is_empty() => "Nothing matches that",
        BuiltinExt::GitHub if !filter.is_empty() => github::filtered_empty_text(command, filter),
        BuiltinExt::GitHub => github::empty_text(command),
        _ => "Nothing found",
    };
    let shown = if question.query.trim().is_empty() { narrowed } else { github::filter(&narrowed, &question.query) };
    let filters = if filters.len() > 1 { filters } else { Vec::new() };
    Message::Items { id: question.id, items: shown, empty: Some(empty.to_string()), filters }
}

fn fetch(ext: BuiltinExt, command: &str) -> Result<Vec<Item>, String> {
    match ext {
        BuiltinExt::GitHub => {
            let json = match github::ask(command, now()).ok_or_else(|| "GitHub has no such list.".to_string())? {
                Ask::Rest(path) => gh(&["api", path])?,
                Ask::Graphql { query, variables } => gh_graphql(query, &variables)?,
            };
            github::items(command, &json, now())
        }
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
    let (status, stdout, stderr) = run_gh(args)?;
    if status {
        return Ok(stdout);
    }
    Err(gh_problem(&stderr, &stdout))
}

/// Asks GitHub's GraphQL API. When part of the answer couldn't be had, such as one repository the
/// sign-in can't read, gh says it failed but still prints the rest, and the rest is shown.
fn gh_graphql(query: &str, variables: &[(&str, String)]) -> Result<String, String> {
    let mut args = vec!["api".to_string(), "graphql".to_string(), "-f".to_string(), format!("query={query}")];
    for (name, value) in variables {
        args.push("-f".into());
        args.push(format!("{name}={value}"));
    }
    let args: Vec<&str> = args.iter().map(String::as_str).collect();
    let (status, stdout, stderr) = run_gh(&args)?;
    if status || stdout.contains("\"data\"") {
        return Ok(stdout);
    }
    Err(gh_problem(&stderr, &stdout))
}

/// Runs gh, giving back whether it worked and what it printed to each stream.
fn run_gh(args: &[&str]) -> Result<(bool, String, String), String> {
    let program = gh_path().ok_or_else(|| "GitHub's command-line tool isn't installed. Install it from Settings, under Extensions.".to_string())?;
    let mut command = Command::new(program);
    command.args(args).stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::piped()).creation_flags(CREATE_NO_WINDOW).env("GH_PROMPT_DISABLED", "1").env("NO_COLOR", "1");
    if gh_login().is_some() {
        command.env_remove("GITHUB_TOKEN").env_remove("GH_TOKEN");
    }
    let mut child = command.spawn().map_err(|_| "GitHub's command-line tool wouldn't start.".to_string())?;
    finish(&mut child, GH_TIMEOUT).ok_or_else(|| "GitHub took too long to answer.".to_string())
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

#[cfg(test)]
mod tests {
    use super::*;

    /// Asks GitHub for real, through the `gh` signed in on this PC, and says only how many of each
    /// kind came back. Run by hand: `cargo test -p orbit-windows real_github -- --ignored --nocapture`.
    #[test]
    #[ignore = "asks GitHub, with this PC's gh sign-in"]
    fn real_github_lists_come_back() {
        for command in ["pulls", "issues"] {
            let items = fetch(BuiltinExt::GitHub, command).unwrap_or_else(|why| panic!("{command}: {why}"));
            let closed = items.iter().filter(|item| item.section == github::CLOSED_SECTION).count();
            let mut tags: Vec<String> = items.iter().flat_map(|item| item.tags.iter().map(|tag| tag.text.clone())).collect();
            tags.sort();
            tags.dedup();
            let filters: Vec<String> = github::filters(command, &items).into_iter().map(|option| option.title).filter(|title| !title.contains('/')).collect();
            println!("{command}: {} open, {closed} recently closed; tags {tags:?}; filters {filters:?}", items.len() - closed);
        }
    }

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
        let question = Question { id: 7, query: "two".into(), filter: String::new() };
        match answer(BuiltinExt::GitHub, "starred", &question, items) {
            Message::Items { id, items, filters, .. } => {
                assert_eq!(id, 7);
                assert_eq!(items.len(), 1);
                assert!(filters.is_empty(), "stars have nothing to filter by");
            }
            _ => panic!("expected items"),
        }
    }

    #[test]
    fn a_filter_narrows_the_list_and_offers_the_rest() {
        let note = |id: u32, repo: &str| format!(r#"{{"id":"{id}","reason":"mention","updated_at":"2026-11-08T00:00:00Z","subject":{{"title":"Note {id}","url":""}},"repository":{{"full_name":"{repo}","html_url":"https://github.com/{repo}"}}}}"#);
        let json = format!("[{},{},{}]", note(1, "o/a"), note(2, "o/b"), note(3, "o/b"));
        let items = github::items("notifications", &json, 0).unwrap();
        let ask = |filter: &str, query: &str| match answer(BuiltinExt::GitHub, "notifications", &Question { id: 1, query: query.into(), filter: filter.into() }, items.clone()) {
            Message::Items { items, filters, empty, .. } => (items.len(), filters.len(), empty.unwrap_or_default()),
            _ => panic!("expected items"),
        };
        assert_eq!(ask("", ""), (3, 3, "No unread notifications".into()), "everything, then each repository");
        assert_eq!(ask("repo:o/b", ""), (2, 3, "Nothing here under this filter".into()), "choosing one keeps the others on offer");
        assert_eq!(ask("repo:o/b", "Note 1"), (0, 3, "Nothing matches that".into()));
        assert_eq!(ask("repo:o/gone", "").0, 3, "a filter that isn't offered any more is let go");
    }
}
