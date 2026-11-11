//! 1Password and Ente Auth, through their own command-line tools.
//!
//! Orbit keeps no secrets. Each tool keeps its own login and vault, and is asked for one value when
//! the user picks one. Item names are held in memory for a short time and never written to disk.
//! Nothing here logs what the tools return. Everything runs off the UI thread, because a tool can
//! wait a long time on an approval prompt.

use crate::rows::{GLYPH_KEY, GLYPH_SHIELD};
use crate::util::{SendHwnd, box_for_post};
use serde_json::Value;
use std::io::Read;
use std::os::windows::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};
use windows::Win32::System::Registry::{HKEY, HKEY_CURRENT_USER, HKEY_LOCAL_MACHINE, RRF_RT_REG_EXPAND_SZ, RRF_RT_REG_SZ, RegGetValueW};
use windows::Win32::UI::Shell::ShellExecuteW;
use windows::Win32::UI::WindowsAndMessaging::SW_SHOWNORMAL;
use windows::core::{PCWSTR, w};

const CREATE_NO_WINDOW: u32 = 0x0800_0000;
const CREATE_NEW_CONSOLE: u32 = 0x0000_0010;

/// Long enough to read an approval prompt and answer it.
const ASK_TIMEOUT: Duration = Duration::from_secs(90);
const INSTALL_TIMEOUT: Duration = Duration::from_secs(300);

const ONE_PASSWORD_WINGET_ID: &str = "AgileBits.1Password.CLI";
const ONE_PASSWORD_PAGE: PCWSTR = w!("https://www.1password.dev/cli/get-started/");
const ENTE_SCRIPT: &str = include_str!("install_ente_cli.ps1");
/// For an item with no name, which both apps allow.
const UNTITLED: &str = "Untitled";
const ENTE_REMOVE_SCRIPT: &str = include_str!("remove_ente_cli.ps1");

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Tool {
    OnePassword,
    EnteAuth,
}

impl Tool {
    pub const ALL: [Tool; 2] = [Tool::OnePassword, Tool::EnteAuth];

    pub fn name(self) -> &'static str {
        match self {
            Tool::OnePassword => "1Password",
            Tool::EnteAuth => "Ente Auth",
        }
    }

    pub fn tool_name(self) -> &'static str {
        match self {
            Tool::OnePassword => "1Password command-line tool",
            Tool::EnteAuth => "Ente command-line tool",
        }
    }

    fn exe(self) -> &'static str {
        match self {
            Tool::OnePassword => "op.exe",
            Tool::EnteAuth => "ente.exe",
        }
    }

    /// Its key in the search index, also used for favourites and aliases.
    pub fn key(self) -> &'static str {
        match self {
            Tool::OnePassword => "int:1password",
            Tool::EnteAuth => "int:ente-auth",
        }
    }

    pub fn glyph(self) -> char {
        match self {
            Tool::OnePassword => GLYPH_KEY,
            Tool::EnteAuth => GLYPH_SHIELD,
        }
    }

    /// Other words people search for it by.
    pub fn keywords(self) -> &'static str {
        match self {
            Tool::OnePassword => "1p op password vault login otp",
            Tool::EnteAuth => "2fa totp otp authenticator code",
        }
    }

    pub fn subtitle(self) -> &'static str {
        match self {
            Tool::OnePassword => "Copy a password or one-time code",
            Tool::EnteAuth => "Paste a one-time code",
        }
    }

    /// The extension it is in Settings, which turns it on and off.
    pub fn extension(self) -> orbit_core::builtin_ext::BuiltinExt {
        match self {
            Tool::OnePassword => orbit_core::builtin_ext::BuiltinExt::OnePassword,
            Tool::EnteAuth => orbit_core::builtin_ext::BuiltinExt::EnteAuth,
        }
    }

    pub fn of_extension(ext: orbit_core::builtin_ext::BuiltinExt) -> Option<Tool> {
        Tool::ALL.into_iter().find(|tool| tool.extension() == ext)
    }

    /// What a hotkey for it runs: its search, as an extension's command.
    pub fn action(self) -> orbit_core::Action {
        orbit_core::Action::Extension { ext: self.extension().id().to_string(), command: VAULT_COMMAND.to_string() }
    }
}

/// The one command 1Password and Ente Auth have, as extensions: searching what they keep.
pub const VAULT_COMMAND: &str = "search";

/// What can be asked for about one item.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Field {
    Password,
    Username,
    OneTimeCode,
}

impl Field {
    pub fn label(self) -> &'static str {
        match self {
            Field::Password => "password",
            Field::Username => "username",
            Field::OneTimeCode => "one-time code",
        }
    }
}

/// One entry in a vault: everything here is safe to show, and none of it is a secret.
#[derive(Clone, Debug, PartialEq)]
pub struct Item {
    pub id: String,
    pub title: String,
    pub subtitle: String,
    pub kind: &'static str,
    pub url: Option<String>,
}

/// Why a tool didn't answer, in terms of what the user can do about it.
#[derive(Clone, Debug, PartialEq)]
pub enum Failure {
    NotInstalled,
    NeedsSignIn,
    TimedOut,
    Other(String),
}

impl Failure {
    /// What to tell the user.
    pub fn message(&self, tool: Tool) -> String {
        match self {
            Failure::NotInstalled => format!("The {} isn't installed. Settings, Integrations can install it.", tool.tool_name()),
            Failure::NeedsSignIn => match tool {
                Tool::OnePassword => "In the 1Password app, turn on Settings, Developer, Integrate with 1Password CLI, then try again.".to_string(),
                Tool::EnteAuth => "Sign in to Ente first, in Settings, Integrations.".to_string(),
            },
            Failure::TimedOut => format!("{} took too long to answer.", tool.name()),
            Failure::Other(text) => text.clone(),
        }
    }
}

pub enum Answer {
    Items { tool: Tool, seq: u64, result: Result<Vec<Item>, Failure> },
    Value { tool: Tool, field: Field, seq: u64, result: Result<Secret, Failure> },
}

/// A value from a tool. The memory is wiped when it goes away.
pub struct Secret {
    pub text: String,
    /// How long a one-time code has left, when the tool says.
    pub seconds_left: Option<i64>,
}

impl Drop for Secret {
    fn drop(&mut self) {
        // Volatile, so the optimizer can't decide nobody will read these bytes again.
        for byte in unsafe { self.text.as_bytes_mut() } {
            unsafe { std::ptr::write_volatile(byte, 0) };
        }
    }
}

fn own_install_dir() -> PathBuf {
    std::env::var_os("LOCALAPPDATA").map(PathBuf::from).unwrap_or_default().join("Programs").join("EnteCLI")
}

fn registry_path(root: HKEY, key: PCWSTR) -> String {
    let mut buffer = vec![0u16; 8192];
    let mut size = (buffer.len() * 2) as u32;
    let status = unsafe {
        RegGetValueW(root, key, w!("Path"), RRF_RT_REG_SZ | RRF_RT_REG_EXPAND_SZ, None, Some(buffer.as_mut_ptr() as *mut _), Some(&mut size))
    };
    if status.is_err() {
        return String::new();
    }
    let len = buffer.iter().position(|&unit| unit == 0).unwrap_or(0);
    String::from_utf16_lossy(&buffer[..len])
}

fn search_path(exe: &str, path: &str) -> Option<PathBuf> {
    path.split(';').filter(|dir| !dir.trim().is_empty()).map(|dir| Path::new(dir.trim()).join(exe)).find(|file| file.is_file())
}

/// A program on the PATH, Orbit's own or the one saved for new programs, which may be newer.
pub(crate) fn find_on_path(exe: &str) -> Option<PathBuf> {
    if let Some(found) = std::env::var("PATH").ok().and_then(|path| search_path(exe, &path)) {
        return Some(found);
    }
    let user = registry_path(HKEY_CURRENT_USER, w!("Environment"));
    let machine = registry_path(HKEY_LOCAL_MACHINE, w!("SYSTEM\\CurrentControlSet\\Control\\Session Manager\\Environment"));
    search_path(exe, &user).or_else(|| search_path(exe, &machine))
}

/// Finds a tool. Orbit may have started before it was installed, so its own PATH isn't enough:
/// the saved ones are read as well, along with where winget and this module put things.
pub fn find(tool: Tool) -> Option<PathBuf> {
    let exe = tool.exe();
    if let Some(found) = std::env::var("PATH").ok().and_then(|path| search_path(exe, &path)) {
        return Some(found);
    }
    let user = registry_path(HKEY_CURRENT_USER, w!("Environment"));
    let machine = registry_path(HKEY_LOCAL_MACHINE, w!("SYSTEM\\CurrentControlSet\\Control\\Session Manager\\Environment"));
    if let Some(found) = search_path(exe, &user).or_else(|| search_path(exe, &machine)) {
        return Some(found);
    }
    let local = PathBuf::from(std::env::var_os("LOCALAPPDATA")?);
    let links = local.join("Microsoft").join("WinGet").join("Links").join(exe);
    if links.is_file() {
        return Some(links);
    }
    if tool == Tool::EnteAuth {
        let own = own_install_dir().join(exe);
        return own.is_file().then_some(own);
    }
    let packages = local.join("Microsoft").join("WinGet").join("Packages");
    std::fs::read_dir(packages).ok()?.flatten().find_map(|entry| {
        let is_it = entry.file_name().to_string_lossy().starts_with(ONE_PASSWORD_WINGET_ID);
        let file = entry.path().join(exe);
        (is_it && file.is_file()).then_some(file)
    })
}

/// Runs a program with no window. Gives back standard output, or the first error line.
fn run(program: &Path, args: &[&str], timeout: Duration) -> Result<Vec<u8>, Failure> {
    let mut child = Command::new(program)
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .creation_flags(CREATE_NO_WINDOW)
        .spawn()
        .map_err(|_| Failure::Other("It wouldn't start.".into()))?;
    finish(&mut child, timeout)
}

fn finish(child: &mut std::process::Child, timeout: Duration) -> Result<Vec<u8>, Failure> {
    let mut out = child.stdout.take();
    let mut err = child.stderr.take();
    let read_out = std::thread::spawn(move || {
        let mut bytes = Vec::new();
        if let Some(pipe) = out.as_mut() {
            let _ = pipe.read_to_end(&mut bytes);
        }
        bytes
    });
    let read_err = std::thread::spawn(move || {
        let mut bytes = Vec::new();
        if let Some(pipe) = err.as_mut() {
            let _ = pipe.read_to_end(&mut bytes);
        }
        bytes
    });
    let started = Instant::now();
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break Some(status),
            Ok(None) if started.elapsed() >= timeout => {
                let _ = child.kill();
                let _ = child.wait();
                break None;
            }
            Ok(None) => std::thread::sleep(Duration::from_millis(25)),
            Err(_) => break None,
        }
    };
    let stdout = read_out.join().unwrap_or_default();
    let stderr = read_err.join().unwrap_or_default();
    match status {
        None => Err(Failure::TimedOut),
        Some(status) if status.success() => Ok(stdout),
        Some(_) => Err(classify_error(&String::from_utf8_lossy(&stderr))),
    }
}

/// Turns what a tool printed into what to do next. Only ever shown on screen.
fn classify_error(stderr: &str) -> Failure {
    let lower = stderr.to_lowercase();
    const SIGN_IN: &[&str] = &[
        "no accounts",
        "not logged in",
        "account login",
        "not currently signed in",
        "no account found",
        "app integration",
        "sign in",
        "signin",
        "authorization",
    ];
    if SIGN_IN.iter().any(|text| lower.contains(text)) {
        return Failure::NeedsSignIn;
    }
    // PowerShell can add its own markup around what a script wrote.
    let first = stderr.lines().map(str::trim).find(|line| !line.is_empty() && !line.starts_with("#<") && !line.starts_with("<Objs")).unwrap_or("It stopped without saying why.");
    Failure::Other(without_log_prefix(first).chars().take(120).collect())
}

/// `op` starts its errors with `[ERROR] 2026/11/06 19:30:00 `, which means nothing to the user.
fn without_log_prefix(line: &str) -> &str {
    let Some(rest) = line.strip_prefix("[ERROR] ") else { return line };
    rest.splitn(3, ' ').nth(2).unwrap_or(rest)
}

fn category_label(category: &str) -> &'static str {
    match category {
        "LOGIN" => "Login",
        "PASSWORD" => "Password",
        "API_CREDENTIAL" => "API key",
        "SECURE_NOTE" => "Note",
        "CREDIT_CARD" => "Card",
        "IDENTITY" => "Identity",
        "SSH_KEY" => "SSH key",
        "SERVER" => "Server",
        "DATABASE" => "Database",
        _ => "Item",
    }
}

/// The items `op item list --format json` describes.
pub fn parse_op_items(bytes: &[u8]) -> Option<Vec<Item>> {
    let rows: Vec<Value> = serde_json::from_slice(bytes).ok()?;
    Some(
        rows.iter()
            .filter_map(|row| {
                let text = |value: &Value| value.as_str().map(str::to_string);
                let id = text(&row["id"])?;
                let title = text(&row["title"]).filter(|title| !title.trim().is_empty()).unwrap_or_else(|| UNTITLED.to_string());
                let info = text(&row["additional_information"]).unwrap_or_default();
                let vault = text(&row["vault"]["name"]).unwrap_or_default();
                let urls = row["urls"].as_array();
                let primary = urls.and_then(|urls| urls.iter().find(|url| url["primary"] == true).or(urls.first()));
                Some(Item {
                    id,
                    title,
                    subtitle: if info.is_empty() { vault } else { info },
                    kind: category_label(row["category"].as_str().unwrap_or("")),
                    url: primary.and_then(|url| text(&url["href"])),
                })
            })
            .collect(),
    )
}

/// The codes `ente auth code list --json` describes. HOTP codes are left out, because Ente's tool
/// doesn't show them.
pub fn parse_ente_codes(bytes: &[u8]) -> Option<Vec<Item>> {
    let rows: Vec<Value> = serde_json::from_slice(bytes).ok()?;
    Some(
        rows.iter()
            .filter(|row| row["type"].as_str() != Some("hotp"))
            .filter_map(|row| {
                let text = |value: &Value| value.as_str().unwrap_or("").to_string();
                let id = row["id"].as_str()?.to_string();
                let (issuer, account, name) = (text(&row["issuer"]), text(&row["accountName"]), text(&row["name"]));
                let (title, subtitle) = if !issuer.is_empty() {
                    (issuer, account)
                } else if !name.is_empty() {
                    (name, String::new())
                } else {
                    (UNTITLED.to_string(), account)
                };
                Some(Item { id, title, subtitle, kind: "Code", url: None })
            })
            .collect(),
    )
}

/// Seconds since 1970 from a time like `2026-11-06T19:30:00Z`.
fn parse_utc(text: &str) -> Option<i64> {
    let bytes = text.as_bytes();
    if bytes.len() != 20 || bytes[4] != b'-' || bytes[7] != b'-' || bytes[10] != b'T' || bytes[13] != b':' || bytes[16] != b':' || bytes[19] != b'Z' {
        return None;
    }
    let number = |range: std::ops::Range<usize>| text.get(range)?.parse::<i64>().ok();
    let (year, month, day) = (number(0..4)?, number(5..7)?, number(8..10)?);
    let (hour, minute, second) = (number(11..13)?, number(14..16)?, number(17..19)?);
    // Days from the civil calendar, by Howard Hinnant's algorithm.
    let y = if month <= 2 { year - 1 } else { year };
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let doy = (153 * (month + if month > 2 { -3 } else { 9 }) + 2) / 5 + day - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    let days = era * 146_097 + doe - 719_468;
    Some(days * 86_400 + hour * 3600 + minute * 60 + second)
}

/// The value in `ente auth code view --json`, and how long it has left at `now`.
pub fn parse_ente_view(bytes: &[u8], now: i64) -> Option<Secret> {
    let row: Value = serde_json::from_slice(bytes).ok()?;
    let text = row["otp"].as_str()?.to_string();
    let seconds_left = row["validUntil"].as_str().and_then(parse_utc).map(|until| (until - now).max(0));
    Some(Secret { text, seconds_left })
}

fn bad_output() -> Failure {
    Failure::Other("It answered in a way Orbit doesn't understand. It may need updating.".to_string())
}

fn list_op(program: &Path) -> Result<Vec<Item>, Failure> {
    let bytes = run(program, &["item", "list", "--format", "json"], ASK_TIMEOUT)?;
    parse_op_items(&bytes).ok_or_else(bad_output)
}

fn list_ente(program: &Path, offline: bool) -> Result<Vec<Item>, Failure> {
    let mut args = vec!["auth", "code", "list", "--all", "--json", "--no-input"];
    if offline {
        args.push("--offline");
    }
    let bytes = run(program, &args, ASK_TIMEOUT)?;
    parse_ente_codes(&bytes).ok_or_else(bad_output)
}

fn get_value(tool: Tool, program: &Path, id: &str, field: Field, now: i64) -> Result<Secret, Failure> {
    match tool {
        Tool::OnePassword => {
            let args: &[&str] = match field {
                Field::Password => &["item", "get", id, "--fields", "label=password", "--reveal"],
                Field::Username => &["item", "get", id, "--fields", "label=username"],
                Field::OneTimeCode => &["item", "get", id, "--otp"],
            };
            let bytes = run(program, args, ASK_TIMEOUT)?;
            let text = String::from_utf8_lossy(&bytes).trim_end_matches(['\r', '\n']).to_string();
            let mut bytes = bytes;
            bytes.iter_mut().for_each(|byte| unsafe { std::ptr::write_volatile(byte, 0) });
            if text.is_empty() {
                return Err(Failure::Other(format!("This item has no {}.", field.label())));
            }
            Ok(Secret { text, seconds_left: None })
        }
        Tool::EnteAuth => {
            let bytes = run(program, &["auth", "code", "view", id, "--json", "--offline", "--no-input"], ASK_TIMEOUT)?;
            parse_ente_view(&bytes, now).ok_or_else(bad_output)
        }
    }
}

fn post_answer(main: SendHwnd, message: u32, answer: Answer) {
    main.post(message, box_for_post(answer), 0);
}

/// Asks a tool for its list, and posts the answer to `main` as `message`. Ente's answers twice when
/// it has anything saved: what it has on this PC straight away, then what's new after syncing.
pub fn request_items(tool: Tool, seq: u64, main: SendHwnd, message: u32) {
    let _ = std::thread::Builder::new().name("orbit-integration".into()).spawn(move || {
        let Some(program) = find(tool) else {
            post_answer(main, message, Answer::Items { tool, seq, result: Err(Failure::NotInstalled) });
            return;
        };
        let result = match tool {
            Tool::OnePassword => list_op(&program),
            Tool::EnteAuth => {
                let saved = list_ente(&program, true);
                let has_some = saved.as_ref().is_ok_and(|items| !items.is_empty());
                if has_some {
                    post_answer(main, message, Answer::Items { tool, seq, result: saved });
                }
                list_ente(&program, false)
            }
        };
        post_answer(main, message, Answer::Items { tool, seq, result });
    });
}

/// Asks a tool for one value, and posts the answer to `main` as `message`.
pub fn request_value(tool: Tool, id: String, field: Field, now: i64, seq: u64, main: SendHwnd, message: u32) {
    let _ = std::thread::Builder::new().name("orbit-integration".into()).spawn(move || {
        let result = match find(tool) {
            Some(program) => get_value(tool, &program, &id, field, now),
            None => Err(Failure::NotInstalled),
        };
        post_answer(main, message, Answer::Value { tool, field, seq, result });
    });
}

/// How the tool got here, which decides whether Orbit can take it away again.
pub fn can_remove(tool: Tool, path: &Path) -> bool {
    match tool {
        Tool::OnePassword => {
            let text = path.to_string_lossy().to_lowercase();
            text.contains("\\winget\\")
        }
        Tool::EnteAuth => path.parent().is_some_and(|dir| dir.eq(own_install_dir().as_path())),
    }
}

pub struct ToolDone {
    pub tool: Tool,
    /// What to tell the user.
    pub message: String,
    /// It did what was asked. A file left behind by a failed install doesn't count.
    pub succeeded: bool,
}

pub(crate) fn winget() -> Option<PathBuf> {
    // It's an app alias, which can't be followed to a file, so `is_file` may say no.
    let local = PathBuf::from(std::env::var_os("LOCALAPPDATA")?);
    let file = local.join("Microsoft").join("WindowsApps").join("winget.exe");
    if std::fs::symlink_metadata(&file).is_ok() {
        return Some(file);
    }
    std::env::var("PATH").ok().and_then(|path| search_path("winget.exe", &path))
}

/// Base64 of the text as UTF-16, which is what `powershell -EncodedCommand` takes.
fn encoded_command(script: &str) -> String {
    const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let bytes: Vec<u8> = script.encode_utf16().flat_map(u16::to_le_bytes).collect();
    let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for chunk in bytes.chunks(3) {
        let group = (chunk[0] as u32) << 16 | (*chunk.get(1).unwrap_or(&0) as u32) << 8 | *chunk.get(2).unwrap_or(&0) as u32;
        for at in 0..4 {
            if at <= chunk.len() {
                out.push(ALPHABET[(group >> (18 - at * 6) & 63) as usize] as char);
            } else {
                out.push('=');
            }
        }
    }
    out
}

/// Runs a script and gives back what it printed. Not through standard input: PowerShell can read
/// nothing from there and still report success, and a script that fails exits with 0 anyway.
fn run_powershell(script: &str, env: &[(&str, String)]) -> Result<String, String> {
    // Otherwise a failure comes back as a block of XML, with this form of the command.
    let script = format!("$ProgressPreference = 'SilentlyContinue'\ntry {{\n{script}\n}} catch {{\n    [Console]::Error.WriteLine($_.Exception.Message)\n    exit 1\n}}\n");
    let mut command = Command::new("powershell.exe");
    command
        .args(["-NoProfile", "-NonInteractive", "-OutputFormat", "Text", "-EncodedCommand"])
        .arg(encoded_command(&script))
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .creation_flags(CREATE_NO_WINDOW);
    for (name, value) in env {
        command.env(name, value);
    }
    let mut child = command.spawn().map_err(|_| "PowerShell wouldn't start.".to_string())?;
    match finish(&mut child, INSTALL_TIMEOUT) {
        Ok(out) => Ok(String::from_utf8_lossy(&out).trim().to_string()),
        Err(Failure::TimedOut) => Err("It took too long, so it was stopped.".to_string()),
        Err(Failure::Other(text)) => Err(text),
        Err(_) => Err("It stopped without saying why.".to_string()),
    }
}

/// Installs Ente's tool into `dir`, and only says it worked if the program is there and runs.
fn install_ente(dir: &Path, add_to_path: bool) -> Result<String, String> {
    let env = [("ORBIT_INSTALL_DIR", dir.to_string_lossy().into_owned()), ("ORBIT_ADD_TO_PATH", if add_to_path { "1" } else { "0" }.to_string())];
    let tag = run_powershell(ENTE_SCRIPT, &env)?;
    let program = dir.join("ente.exe");
    if !program.is_file() {
        return Err("The download finished, but the program isn't there.".to_string());
    }
    run(&program, &["--version"], Duration::from_secs(20)).map_err(|_| "It installed, but it won't start.".to_string())?;
    Ok(tag.trim_start_matches("ente-cli-").to_string())
}

fn install_now(tool: Tool) -> (String, bool) {
    match tool {
        Tool::OnePassword => {
            let Some(winget) = winget() else {
                unsafe {
                    ShellExecuteW(None, w!("open"), ONE_PASSWORD_PAGE, None, None, SW_SHOWNORMAL);
                }
                return ("Windows Package Manager isn't available, so 1Password's download page is open instead.".to_string(), false);
            };
            let args = ["install", "--id", ONE_PASSWORD_WINGET_ID, "-e", "--silent", "--accept-source-agreements", "--accept-package-agreements", "--disable-interactivity"];
            match run(&winget, &args, INSTALL_TIMEOUT) {
                Ok(_) => ("1Password's tool is installed.".to_string(), true),
                Err(_) if find(tool).is_some() => ("1Password's tool is installed.".to_string(), true),
                Err(_) => ("Windows Package Manager couldn't install it. Check your connection and try again.".to_string(), false),
            }
        }
        Tool::EnteAuth => {
            match install_ente(&own_install_dir(), true) {
                Ok(version) => (format!("Ente's tool is installed ({version}). Now sign in."), true),
                Err(reason) => (format!("Couldn't install Ente's tool. {reason}"), false),
            }
        }
    }
}

fn remove_now(tool: Tool) -> (String, bool) {
    match tool {
        Tool::OnePassword => {
            let Some(winget) = winget() else {
                return ("Windows Package Manager isn't available, so Orbit can't remove it.".to_string(), false);
            };
            let args = ["uninstall", "--id", ONE_PASSWORD_WINGET_ID, "-e", "--silent", "--accept-source-agreements", "--disable-interactivity"];
            match run(&winget, &args, INSTALL_TIMEOUT) {
                Ok(_) => ("1Password's tool is removed.".to_string(), true),
                Err(_) => ("Windows Package Manager couldn't remove it.".to_string(), false),
            }
        }
        Tool::EnteAuth => {
            let dir = own_install_dir();
            let _ = run_powershell(ENTE_REMOVE_SCRIPT, &[("ORBIT_INSTALL_DIR", dir.to_string_lossy().into_owned())]);
            let removed = std::fs::remove_file(dir.join("ente.exe")).is_ok();
            let _ = std::fs::remove_dir(&dir);
            if removed {
                ("Ente's tool is removed. Your sign-in stays in its own data folder.".to_string(), true)
            } else {
                ("Couldn't remove Ente's tool. It may be in use.".to_string(), false)
            }
        }
    }
}

/// Installs or removes a tool, and posts a [`ToolDone`] to `window` as `message`.
pub fn change_tool(tool: Tool, install: bool, window: SendHwnd, message: u32) {
    let _ = std::thread::Builder::new().name("orbit-tool-install".into()).spawn(move || {
        let (text, succeeded) = if install { install_now(tool) } else { remove_now(tool) };
        window.post(message, box_for_post(ToolDone { tool, message: text, succeeded }), 0);
    });
}

/// Opens a console where Ente's own login asks for the email, password and any second factor.
/// Orbit never sees them. It stays open at the end, so a failure can be read. When it closes,
/// who's signed in is checked again and posted to `main` as `message`.
pub fn open_sign_in(tool: Tool, main: SendHwnd, message: u32) -> Result<(), String> {
    let Some(program) = find(tool) else { return Err("Install the tool first.".to_string()) };
    let line = format!("/c \"\"{}\" account login --app auth & echo. & pause\"", program.display());
    let mut child = Command::new("cmd.exe")
        .raw_arg(line)
        .creation_flags(CREATE_NEW_CONSOLE)
        .spawn()
        .map_err(|_| "Couldn't open a window to sign in.".to_string())?;
    let _ = std::thread::Builder::new().name("orbit-ente-sign-in".into()).spawn(move || {
        let _ = child.wait();
        post_ente_account(main, message);
    });
    Ok(())
}

/// Who is signed in to Ente's tool, for Settings to show.
#[derive(Clone, Debug, PartialEq)]
pub enum EnteAccount {
    /// The tool isn't there, or didn't answer.
    Unknown,
    SignedOut,
    /// The email of the first account signed in for Ente Auth.
    SignedIn(String),
}

/// The Ente Auth accounts in what `ente account list --json` printed, the selected one first.
/// Accounts signed in only to Ente Photos or Locker don't count, since their codes aren't there.
/// Ente's tool before 0.4 printed a list of `Email:` and `App:` lines instead, which is read too.
pub fn parse_ente_accounts(text: &str) -> Vec<String> {
    #[derive(serde::Deserialize)]
    struct Account {
        #[serde(default)]
        name: String,
        #[serde(default)]
        email: String,
        #[serde(default)]
        apps: Vec<String>,
        #[serde(default)]
        selected: bool,
    }
    if let Ok(mut accounts) = serde_json::from_str::<Vec<Account>>(text.trim()) {
        accounts.retain(|account| account.apps.iter().any(|app| app.eq_ignore_ascii_case("auth")));
        accounts.sort_by_key(|account| !account.selected);
        return accounts.into_iter().map(|account| if account.email.trim().is_empty() { account.name } else { account.email }).filter(|who| !who.trim().is_empty()).collect();
    }
    let mut out = Vec::new();
    let mut email = None;
    for line in text.lines() {
        let line = line.trim();
        if line.starts_with("====") {
            email = None;
        } else if let Some(value) = line.strip_prefix("Email:") {
            email = Some(value.trim().to_string()).filter(|value| !value.is_empty());
        } else if let Some(value) = line.strip_prefix("App:") {
            if value.trim().eq_ignore_ascii_case("auth") {
                if let Some(email) = email.take() {
                    out.push(email);
                }
            }
        }
    }
    out
}

fn ente_account() -> EnteAccount {
    let Some(program) = find(Tool::EnteAuth) else { return EnteAccount::Unknown };
    match run(&program, &["account", "list", "--json", "--offline", "--no-input"], Duration::from_secs(20)) {
        Ok(bytes) => match parse_ente_accounts(&String::from_utf8_lossy(&bytes)).into_iter().next() {
            Some(email) => EnteAccount::SignedIn(email),
            None => EnteAccount::SignedOut,
        },
        Err(Failure::NeedsSignIn) => EnteAccount::SignedOut,
        Err(_) => EnteAccount::Unknown,
    }
}

fn post_ente_account(main: SendHwnd, message: u32) {
    main.post(message, box_for_post(ente_account()), 0);
}

/// Asks Ente's tool who's signed in, off this thread, and posts an [`EnteAccount`] to `main`.
pub fn check_ente_account(main: SendHwnd, message: u32) {
    let _ = std::thread::Builder::new().name("orbit-ente-account".into()).spawn(move || post_ente_account(main, message));
}


#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_1password_items_without_any_secret_fields() {
        let json = br#"[
            {"id":"abc","title":"GitHub","category":"LOGIN","additional_information":"me@example.com",
             "vault":{"id":"v","name":"Personal"},
             "urls":[{"href":"https://other.example"},{"primary":true,"href":"https://github.com"}]},
            {"id":"def","title":"Router","category":"PASSWORD","vault":{"name":"Home"}},
            {"title":"no id, so it can't be used"}
        ]"#;
        let items = parse_op_items(json).unwrap();
        assert_eq!(items.len(), 2);
        assert_eq!(items[0], Item {
            id: "abc".into(),
            title: "GitHub".into(),
            subtitle: "me@example.com".into(),
            kind: "Login",
            url: Some("https://github.com".into()),
        });
        assert_eq!(items[1].subtitle, "Home", "falls back to the vault");
        assert_eq!(items[1].url, None);
        assert_eq!(parse_op_items(b"not json"), None);
    }

    #[test]
    fn ente_accounts_count_only_for_auth() {
        let text = "Configured accounts: 2\n====================================\nEmail:     photos@example.com\nID:        1\nApp:       photos\nExportDir: \n====================================\n====================================\nEmail:     me@example.com\nID:        2\nApp:       auth\nExportDir: \n====================================\n";
        assert_eq!(parse_ente_accounts(text), ["me@example.com"]);
        assert!(parse_ente_accounts("Configured accounts: 0\n").is_empty());
    }

    /// The shape `ente account list --help` describes in ente-cli 0.4.3.
    #[test]
    fn ente_accounts_are_read_from_json_selected_first() {
        let json = r#"[
            {"id":"1","name":"photos","email":"photos@example.com","host":"https://api.ente.com","apps":["photos"],"selected":false},
            {"id":"2","name":"work","email":"work@example.com","host":"https://api.ente.com","apps":["auth","locker"],"selected":false},
            {"id":"3","name":"me","email":"me@example.com","host":"https://api.ente.com","apps":["photos","auth"],"selected":true}
        ]"#;
        assert_eq!(parse_ente_accounts(json), ["me@example.com", "work@example.com"]);
        assert!(parse_ente_accounts("[]").is_empty());
        assert!(parse_ente_accounts(r#"[{"id":"1","name":"p","email":"p@example.com","apps":["photos"],"selected":true}]"#).is_empty());
    }

    #[test]
    fn items_with_no_name_still_get_one() {
        let items = parse_op_items(br#"[{"id":"a","title":"","vault":{"name":"Home"}},{"id":"b","title":"  "},{"id":"c"}]"#).unwrap();
        assert!(items.iter().all(|item| item.title == UNTITLED), "{items:?}");
        let codes = parse_ente_codes(br#"[{"id":"a","name":"","issuer":"","accountName":"me","type":"totp"}]"#).unwrap();
        assert_eq!((codes[0].title.as_str(), codes[0].subtitle.as_str()), (UNTITLED, "me"));
    }

    /// The shape comes from the tests in Ente's own repository, at ente-cli-v0.4.3.
    #[test]
    fn reads_ente_codes_and_leaves_out_ones_it_cannot_show() {
        let json = br#"[
            {"id":"1","name":"GitHub (alice@example.com)","issuer":"GitHub","accountName":"alice@example.com","type":"totp"},
            {"id":"2","name":"Steam","issuer":"","accountName":"","type":"steam"},
            {"id":"3","name":"Work","issuer":"Work","accountName":"bob","type":"hotp"}
        ]"#;
        let items = parse_ente_codes(json).unwrap();
        assert_eq!(items.len(), 2);
        assert_eq!((items[0].title.as_str(), items[0].subtitle.as_str()), ("GitHub", "alice@example.com"));
        assert_eq!(items[1].title, "Steam");
    }

    #[test]
    fn reads_an_ente_code_and_its_time_left() {
        let json = br#"{"id":"1","name":"GitHub","issuer":"GitHub","accountName":"a","type":"totp","otp":"123456","validUntil":"2026-11-06T19:30:30Z"}"#;
        let now = parse_utc("2026-11-06T19:30:13Z").unwrap();
        let secret = parse_ente_view(json, now).unwrap();
        assert_eq!(secret.text, "123456");
        assert_eq!(secret.seconds_left, Some(17));
        assert!(parse_ente_view(br#"{"id":"1"}"#, now).is_none());
    }

    #[test]
    fn dates_match_known_values() {
        assert_eq!(parse_utc("1970-01-01T00:00:00Z"), Some(0));
        assert_eq!(parse_utc("2000-03-01T00:00:00Z"), Some(951_868_800));
        assert_eq!(parse_utc("2026-11-06T19:30:00Z"), Some(1_793_993_400));
        assert_eq!(parse_utc("2026-11-06 19:30:00"), None);
        assert_eq!(parse_utc("soon"), None);
    }

    #[test]
    fn errors_say_what_to_do() {
        assert_eq!(classify_error("Error: no accounts; use account login --app <APP>"), Failure::NeedsSignIn);
        assert_eq!(classify_error("[ERROR] 2026/11/06 account is not signed in. You are not currently signed in."), Failure::NeedsSignIn);
        assert_eq!(classify_error("\n  network unreachable\nmore"), Failure::Other("network unreachable".to_string()));
        assert_eq!(classify_error("#< CLIXML\n<Objs Version=\"1\"></Objs>\nno route\n"), Failure::Other("no route".to_string()));
        assert_eq!(classify_error("[ERROR] 2026/11/06 19:30:00 \"x\" isn't a field"), Failure::Other("\"x\" isn't a field".to_string()));
        assert!(matches!(classify_error(""), Failure::Other(_)));
    }


    #[test]
    fn only_what_orbit_installed_can_be_removed() {
        assert!(can_remove(Tool::OnePassword, Path::new(r"C:\Users\a\AppData\Local\Microsoft\WinGet\Links\op.exe")));
        assert!(!can_remove(Tool::OnePassword, Path::new(r"C:\Tools\op.exe")));
        assert!(!can_remove(Tool::EnteAuth, Path::new(r"C:\Tools\ente.exe")));
        assert!(can_remove(Tool::EnteAuth, &own_install_dir().join("ente.exe")));
    }

    #[test]
    fn encoding_matches_what_powershell_expects() {
        // [Convert]::ToBase64String([Text.Encoding]::Unicode.GetBytes('dir'))
        assert_eq!(encoded_command("dir"), "ZABpAHIA");
        assert_eq!(encoded_command("ab"), "YQBiAA==");
        assert_eq!(encoded_command(""), "");
    }

    /// The bug this guards against: a script with LF line endings, sent over standard input, ran
    /// nothing and exited with 0, so Orbit said it had installed something it hadn't.
    #[test]
    fn scripts_really_run_and_their_failures_are_noticed() {
        let script = "$ErrorActionPreference = 'Stop'\n# a comment\ntry {\n    Write-Output $env:ORBIT_TEST_VALUE\n} finally {\n    Write-Output 'after'\n}\n";
        let out = run_powershell(script, &[("ORBIT_TEST_VALUE", "it ran".to_string())]).unwrap();
        assert_eq!(out.lines().map(str::trim).collect::<Vec<_>>(), ["it ran", "after"]);
        let failed = run_powershell("$ErrorActionPreference = 'Stop'\nthrow 'nope, it failed'\nWrite-Output 'not here'\n", &[]);
        let reason = failed.unwrap_err();
        assert!(reason.contains("nope, it failed"), "{reason}");
    }

    /// The real download, into a scratch folder. `cargo test -- --ignored`, as it needs the network.
    #[test]
    #[ignore]
    fn the_real_install_works() {
        let dir = std::env::temp_dir().join(format!("orbit-ente-test-{}", std::process::id()));
        let version = install_ente(&dir, false).unwrap();
        assert!(version.starts_with('v'), "{version}");
        assert!(dir.join("ente.exe").is_file());
        let _ = std::fs::remove_dir_all(&dir);
    }

    fn path_key(name: &str) -> windows::Win32::System::Registry::HKEY {
        use windows::Win32::System::Registry::{KEY_ALL_ACCESS, REG_EXPAND_SZ, REG_OPTION_NON_VOLATILE, RegCreateKeyExW, RegSetValueExW};
        let mut key = HKEY::default();
        let wide: Vec<u16> = format!("Software\\{name}").encode_utf16().chain([0]).collect();
        let start: Vec<u8> = r"%USERPROFILE%\bin;C:\Tools".encode_utf16().chain([0]).flat_map(u16::to_le_bytes).collect();
        unsafe {
            let _ = RegCreateKeyExW(HKEY_CURRENT_USER, PCWSTR(wide.as_ptr()), None, None, REG_OPTION_NON_VOLATILE, KEY_ALL_ACCESS, None, &mut key, None);
            let _ = RegSetValueExW(key, w!("Path"), None, REG_EXPAND_SZ, Some(&start));
        }
        key
    }

    /// Both PATH edits, on a scratch registry key instead of the real one.
    #[test]
    fn path_edits_keep_other_entries_exactly_as_written() {
        use windows::Win32::System::Registry::{RRF_NOEXPAND, RegDeleteTreeW, RegCloseKey};
        let name = format!("OrbitPathTest{}", std::process::id());
        let key = path_key(&name);
        let read = || {
            let mut buffer = vec![0u16; 2048];
            let mut size = (buffer.len() * 2) as u32;
            let _ = unsafe { RegGetValueW(key, PCWSTR::null(), w!("Path"), RRF_RT_REG_EXPAND_SZ | RRF_NOEXPAND, None, Some(buffer.as_mut_ptr() as *mut _), Some(&mut size)) };
            let len = buffer.iter().position(|&unit| unit == 0).unwrap_or(0);
            String::from_utf16_lossy(&buffer[..len])
        };
        let scratch = format!("'Software\\{name}'");
        let dir = r"C:\Orbit Test\EnteCLI".to_string();
        // Just the PATH step of the installer, cut out of the real script.
        let start = ENTE_SCRIPT.find("# Read and written unexpanded").unwrap();
        let end = ENTE_SCRIPT.find("Write-Output $tag").unwrap();
        let body = ENTE_SCRIPT[start..end].trim_end().trim_end_matches('}').replace("'Environment'", &scratch);
        let add = format!("$ErrorActionPreference = 'Stop'\n$dir = $env:ORBIT_INSTALL_DIR\n{body}");
        let remove = ENTE_REMOVE_SCRIPT.replace("'Environment'", &scratch);
        let env = [("ORBIT_INSTALL_DIR", dir.clone())];
        run_powershell(&add, &env).unwrap();
        assert_eq!(read(), r"C:\Orbit Test\EnteCLI;%USERPROFILE%\bin;C:\Tools");
        run_powershell(&add, &env).unwrap();
        assert_eq!(read(), r"C:\Orbit Test\EnteCLI;%USERPROFILE%\bin;C:\Tools", "only added once");
        run_powershell(&remove, &env).unwrap();
        assert_eq!(read(), r"%USERPROFILE%\bin;C:\Tools");
        run_powershell(&remove, &env).unwrap();
        assert_eq!(read(), r"%USERPROFILE%\bin;C:\Tools", "nothing else goes");
        let wide: Vec<u16> = format!("Software\\{name}").encode_utf16().chain([0]).collect();
        unsafe {
            let _ = RegCloseKey(key);
            let _ = RegDeleteTreeW(HKEY_CURRENT_USER, PCWSTR(wide.as_ptr()));
        }
    }

    #[test]
    fn the_install_script_checks_the_download() {
        assert!(ENTE_SCRIPT.contains("SHA256SUMS"));
        assert!(ENTE_SCRIPT.contains("Get-FileHash"));
        assert!(ENTE_SCRIPT.contains("https://github.com/ente/ente/releases/download/"));
    }
}
