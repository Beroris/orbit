//! Extensions from folders: finding them, and running their programs. The messages are read and
//! checked in `orbit_core::extension`; docs/EXTENSIONS.md describes them.
//!
//! Each running extension has a reader thread for its output and a writer thread for its input, so
//! a program that stops reading or writing never holds up the UI thread. Every program is put in a
//! job object that ends it when Orbit ends, however Orbit ends.

use crate::util::{SendHwnd, box_for_post};
use orbit_core::extension::{MANIFEST_FILE, MAX_LINE_BYTES, Manifest, Message, Request};
use std::collections::HashMap;
use std::io::{BufRead, BufReader, Write};
use std::os::windows::io::AsRawHandle;
use std::os::windows::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::mpsc::{Sender, channel};
use std::time::{Duration, Instant};
use windows::Win32::Foundation::HANDLE;
use windows::Win32::System::JobObjects::{
    AssignProcessToJobObject, CreateJobObjectW, JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE, JOBOBJECT_EXTENDED_LIMIT_INFORMATION, JobObjectExtendedLimitInformation,
    SetInformationJobObject,
};

const CREATE_NO_WINDOW: u32 = 0x0800_0000;
/// An extension unused for this long is stopped, and started again when it's next needed.
pub const IDLE_STOP: Duration = Duration::from_secs(5 * 60);

/// An extension found in the extensions folder.
#[derive(Clone, Debug, PartialEq)]
pub struct Installed {
    pub dir: PathBuf,
    pub manifest: Manifest,
}

impl Installed {
    pub fn id(&self) -> &str {
        &self.manifest.id
    }

    /// The extension's picture, if it has one that's there.
    pub fn icon_path(&self) -> Option<PathBuf> {
        let relative = orbit_core::extension::inside_folder(&self.manifest.icon)?;
        let path = self.dir.join(relative);
        path.is_file().then_some(path)
    }

    /// An item's picture: one in the extension's folder, or any file by its full path.
    pub fn item_icon(&self, icon: &str) -> Option<(String, bool)> {
        if orbit_core::extension::is_full_path(icon) {
            let picture = is_picture(icon) && !icon.starts_with("\\\\");
            return Some((icon.to_string(), picture));
        }
        let relative = orbit_core::extension::inside_folder(icon)?;
        Some((self.dir.join(relative).to_string_lossy().into_owned(), true))
    }

    /// The command line, as Settings shows it before the extension is turned on.
    pub fn command_line(&self) -> String {
        self.manifest.run.iter().map(|part| if part.contains(' ') { format!("\"{part}\"") } else { part.clone() }).collect::<Vec<_>>().join(" ")
    }
}

fn is_picture(path: &str) -> bool {
    let lower = path.to_ascii_lowercase();
    [".png", ".ico", ".svg", ".jpg", ".jpeg", ".bmp", ".gif", ".webp"].iter().any(|ext| lower.ends_with(ext))
}

/// A folder that has a manifest but couldn't be used, and why.
#[derive(Clone, Debug, PartialEq)]
pub struct Broken {
    pub dir: PathBuf,
    pub why: String,
}

pub fn folder(data_dir: &Path) -> PathBuf {
    data_dir.join("extensions")
}

/// Where an extension may keep things of its own, given to it as `ORBIT_EXTENSION_DATA`.
fn data_folder(data_dir: &Path, id: &str) -> PathBuf {
    data_dir.join("extension-data").join(id)
}

pub fn read(dir: &Path) -> Result<Installed, String> {
    let text = std::fs::read_to_string(dir.join(MANIFEST_FILE)).map_err(|_| format!("There's no {MANIFEST_FILE} in it."))?;
    let manifest = Manifest::parse(&text)?;
    Ok(Installed { dir: dir.to_path_buf(), manifest })
}

/// Every extension in the extensions folder, by name, and folders that couldn't be read. The first
/// of two with the same id wins.
pub fn discover(data_dir: &Path) -> (Vec<Installed>, Vec<Broken>) {
    let mut found: Vec<Installed> = Vec::new();
    let mut broken = Vec::new();
    let Ok(entries) = std::fs::read_dir(folder(data_dir)) else { return (found, broken) };
    // A folder starting with a dot is one being added, or one the user hid.
    let mut dirs: Vec<PathBuf> = entries
        .flatten()
        .filter(|entry| !entry.file_name().to_string_lossy().starts_with('.'))
        .map(|entry| entry.path())
        .filter(|path| path.is_dir())
        .collect();
    dirs.sort();
    for dir in dirs {
        if !dir.join(MANIFEST_FILE).is_file() {
            continue;
        }
        match read(&dir) {
            Ok(installed) if orbit_core::builtin_ext::BuiltinExt::from_id(installed.id()).is_some() => {
                broken.push(Broken { dir, why: format!("\"{}\" is the id of an extension that comes with Orbit.", installed.id()) });
            }
            Ok(installed) if found.iter().any(|other| other.id() == installed.id()) => {
                broken.push(Broken { dir, why: format!("Another extension already uses the id \"{}\".", installed.id()) });
            }
            Ok(installed) => found.push(installed),
            Err(why) => broken.push(Broken { dir, why }),
        }
    }
    found.sort_by_key(|installed| installed.manifest.name.to_lowercase());
    (found, broken)
}

fn copy_folder(from: &Path, to: &Path, depth: usize) -> std::io::Result<u64> {
    if depth > 8 {
        return Ok(0);
    }
    std::fs::create_dir_all(to)?;
    let mut bytes = 0;
    for entry in std::fs::read_dir(from)?.flatten() {
        let kind = entry.file_type()?;
        let target = to.join(entry.file_name());
        if kind.is_symlink() {
            continue;
        } else if kind.is_dir() {
            if entry.file_name() == ".git" || entry.file_name() == "node_modules" && depth > 0 {
                continue;
            }
            bytes += copy_folder(&entry.path(), &target, depth + 1)?;
        } else {
            bytes += std::fs::copy(entry.path(), target)?;
        }
        if bytes > 200 * 1024 * 1024 {
            return Err(std::io::Error::other("It's larger than 200 MB."));
        }
    }
    Ok(bytes)
}

/// Copies an extension's folder into the extensions folder. One already there with the same id is
/// replaced, which is how an extension is updated.
pub fn add(source: &Path, data_dir: &Path) -> Result<Installed, String> {
    let source = if source.is_file() { source.parent().unwrap_or(source).to_path_buf() } else { source.to_path_buf() };
    let incoming = read(&source)?;
    let root = folder(data_dir);
    let target = root.join(incoming.id());
    if source.starts_with(&root) {
        return Ok(incoming);
    }
    let (existing, _) = discover(data_dir);
    let staging = root.join(format!(".adding-{}", incoming.id()));
    let _ = std::fs::remove_dir_all(&staging);
    copy_folder(&source, &staging, 0).map_err(|e| format!("Couldn't copy it: {e}"))?;
    if let Some(old) = existing.iter().find(|other| other.id() == incoming.id()) {
        let _ = std::fs::remove_dir_all(&old.dir);
    }
    let _ = std::fs::remove_dir_all(&target);
    std::fs::rename(&staging, &target).map_err(|e| format!("Couldn't put it in place: {e}"))?;
    read(&target)
}

/// Deletes an extension's folder, and what it kept in its data folder.
pub fn remove(installed: &Installed, data_dir: &Path) -> Result<(), String> {
    if !installed.dir.starts_with(folder(data_dir)) {
        return Err("It isn't in Orbit's extensions folder.".into());
    }
    std::fs::remove_dir_all(&installed.dir).map_err(|e| format!("Couldn't delete it: {e}"))?;
    let _ = std::fs::remove_dir_all(data_folder(data_dir, installed.id()));
    Ok(())
}

/// The program to start and the arguments before the manifest's own. A script named first runs
/// in the program made for it.
fn resolve(installed: &Installed) -> Result<(PathBuf, Vec<String>), String> {
    let first = installed.manifest.run[0].trim();
    let lower = first.to_ascii_lowercase();
    let in_folder = |name: &str| -> Option<PathBuf> {
        let relative = orbit_core::extension::inside_folder(name)?;
        let path = installed.dir.join(relative);
        path.is_file().then_some(path)
    };
    let script = |interpreter: &str, before: &[&str], file: PathBuf| -> Result<(PathBuf, Vec<String>), String> {
        let program = find_program(interpreter).ok_or_else(|| format!("It needs {interpreter}, which isn't installed."))?;
        let mut args: Vec<String> = before.iter().map(|arg| arg.to_string()).collect();
        args.push(file.to_string_lossy().into_owned());
        Ok((program, args))
    };
    if let Some(file) = in_folder(first) {
        if lower.ends_with(".ps1") {
            return script("powershell", &["-NoProfile", "-NonInteractive", "-ExecutionPolicy", "Bypass", "-File"], file);
        }
        if lower.ends_with(".py") {
            return script("python", &["-u"], file);
        }
        if lower.ends_with(".js") || lower.ends_with(".mjs") {
            return script("node", &[], file);
        }
        if lower.ends_with(".cmd") || lower.ends_with(".bat") {
            return script("cmd", &["/d", "/c"], file);
        }
        return Ok((file, Vec::new()));
    }
    if orbit_core::extension::is_full_path(first) {
        let path = PathBuf::from(first);
        return if path.is_file() { Ok((path, Vec::new())) } else { Err(format!("{first} isn't there.")) };
    }
    if let Some(file) = in_folder(&format!("{first}.exe")) {
        return Ok((file, Vec::new()));
    }
    find_program(first).map(|program| (program, Vec::new())).ok_or_else(|| format!("\"{first}\" isn't installed, or isn't on the PATH."))
}

fn find_program(name: &str) -> Option<PathBuf> {
    let exe = if name.to_ascii_lowercase().ends_with(".exe") { name.to_string() } else { format!("{name}.exe") };
    if name.eq_ignore_ascii_case("powershell") || name.eq_ignore_ascii_case("cmd") {
        let system = std::env::var_os("SystemRoot").map(PathBuf::from).unwrap_or_else(|| PathBuf::from("C:\\Windows")).join("System32");
        let path = if name.eq_ignore_ascii_case("cmd") { system.join("cmd.exe") } else { system.join("WindowsPowerShell\\v1.0\\powershell.exe") };
        if path.is_file() {
            return Some(path);
        }
    }
    crate::integrations::find_on_path(&exe)
}

/// What Settings asks the launcher to do with the extensions folder.
pub enum Change {
    /// One was copied in, by its name.
    Added(String),
    /// Take this one away. The launcher does it, as it may have to stop its program first.
    Remove(PathBuf),
}

/// What an extension's threads tell the UI thread.
pub struct Event {
    pub ext: String,
    /// Which time it was started, so news from a program since replaced is ignored.
    pub run: u64,
    pub kind: EventKind,
}

pub enum EventKind {
    Said(Message),
    /// The program ended. The last lines it wrote to its error output, if any.
    Ended(String),
}

struct Running {
    child: Child,
    input: Sender<String>,
    run: u64,
    last_used: Instant,
}

/// The extensions' programs that are running.
pub struct Runners {
    running: HashMap<String, Running>,
    job: Option<HANDLE>,
    runs: u64,
    pub data_dir: PathBuf,
}

impl Runners {
    pub fn new(data_dir: PathBuf) -> Runners {
        Runners { running: HashMap::new(), job: None, runs: 0, data_dir }
    }

    fn job(&mut self) -> Option<HANDLE> {
        if self.job.is_none() {
            unsafe {
                let job = CreateJobObjectW(None, windows::core::PCWSTR::null()).ok()?;
                let mut limits = JOBOBJECT_EXTENDED_LIMIT_INFORMATION::default();
                limits.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
                let _ = SetInformationJobObject(job, JobObjectExtendedLimitInformation, &limits as *const _ as *const _, std::mem::size_of_val(&limits) as u32);
                self.job = Some(job);
            }
        }
        self.job
    }

    pub fn any_running(&self) -> bool {
        !self.running.is_empty()
    }

    /// Sends a request, starting the program first if it isn't running. Returns which run of it
    /// the answer will come from.
    pub fn send(&mut self, installed: &Installed, request: &Request, main: SendHwnd, message: u32) -> Result<u64, String> {
        let id = installed.id().to_string();
        let alive = self.running.get_mut(&id).is_some_and(|running| matches!(running.child.try_wait(), Ok(None)));
        if !alive {
            self.stop(&id);
            self.start(installed, main, message)?;
        }
        let running = self.running.get_mut(&id).expect("started above");
        running.last_used = Instant::now();
        running.input.send(request.to_line()).map_err(|_| "It stopped.".to_string())?;
        Ok(running.run)
    }

    fn start(&mut self, installed: &Installed, main: SendHwnd, message: u32) -> Result<(), String> {
        let (program, mut args) = resolve(installed)?;
        args.extend(installed.manifest.run.iter().skip(1).cloned());
        let data = data_folder(&self.data_dir, installed.id());
        let _ = std::fs::create_dir_all(&data);
        let mut child = Command::new(&program)
            .args(&args)
            .current_dir(&installed.dir)
            .env("ORBIT_VERSION", env!("CARGO_PKG_VERSION"))
            .env("ORBIT_EXTENSION_DIR", &installed.dir)
            .env("ORBIT_EXTENSION_DATA", &data)
            .env("PYTHONIOENCODING", "utf-8")
            .env("PYTHONUNBUFFERED", "1")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .creation_flags(CREATE_NO_WINDOW)
            .spawn()
            .map_err(|e| format!("Couldn't start {}: {e}", program.display()))?;
        if let Some(job) = self.job() {
            unsafe {
                let _ = AssignProcessToJobObject(job, HANDLE(child.as_raw_handle()));
            }
        }
        self.runs += 1;
        let run = self.runs;
        let ext = installed.id().to_string();

        let (input, lines) = channel::<String>();
        let mut stdin = child.stdin.take().expect("piped");
        let _ = std::thread::Builder::new().name(format!("orbit-ext-in-{ext}")).spawn(move || {
            while let Ok(line) = lines.recv() {
                if stdin.write_all(line.as_bytes()).and_then(|_| stdin.flush()).is_err() {
                    break;
                }
            }
        });

        // Its error output is kept short, for saying why it stopped.
        let stderr = child.stderr.take().expect("piped");
        let last_error = std::sync::Arc::new(std::sync::Mutex::new(String::new()));
        let errors = last_error.clone();
        let _ = std::thread::Builder::new().name(format!("orbit-ext-err-{ext}")).spawn(move || {
            for line in BufReader::new(stderr).lines().map_while(Result::ok) {
                if let Ok(mut kept) = errors.lock() {
                    *kept = line.chars().take(300).collect();
                }
            }
        });

        let stdout = child.stdout.take().expect("piped");
        let reader_ext = ext.clone();
        let _ = std::thread::Builder::new().name(format!("orbit-ext-out-{ext}")).spawn(move || {
            let mut reader = BufReader::new(stdout);
            let mut line = Vec::new();
            loop {
                line.clear();
                match read_line_bounded(&mut reader, &mut line) {
                    Ok(0) | Err(_) => break,
                    Ok(_) => {
                        let text = String::from_utf8_lossy(&line);
                        if let Some(said) = orbit_core::extension::parse_message(&text) {
                            main.post(message, box_for_post(Event { ext: reader_ext.clone(), run, kind: EventKind::Said(said) }), 0);
                        }
                    }
                }
            }
            std::thread::sleep(Duration::from_millis(50));
            let why = last_error.lock().map(|text| text.clone()).unwrap_or_default();
            main.post(message, box_for_post(Event { ext: reader_ext, run, kind: EventKind::Ended(why) }), 0);
        });

        let hello = Request::Hello { orbit: env!("CARGO_PKG_VERSION").into(), protocol: orbit_core::extension::PROTOCOL };
        let _ = input.send(hello.to_line());
        self.running.insert(ext, Running { child, input, run, last_used: Instant::now() });
        Ok(())
    }

    pub fn stop(&mut self, id: &str) {
        if let Some(mut running) = self.running.remove(id) {
            let _ = running.child.kill();
            let _ = running.child.wait();
        }
    }

    /// Ends programs that haven't been used for a while.
    pub fn stop_idle(&mut self) {
        let idle: Vec<String> = self.running.iter().filter(|(_, running)| running.last_used.elapsed() >= IDLE_STOP).map(|(id, _)| id.clone()).collect();
        for id in idle {
            self.stop(&id);
        }
    }

    pub fn stop_all(&mut self) {
        let ids: Vec<String> = self.running.keys().cloned().collect();
        for id in ids {
            self.stop(&id);
        }
    }

    /// Forgets a program that ended by itself.
    pub fn ended(&mut self, id: &str, run: u64) {
        if self.running.get(id).is_some_and(|running| running.run == run) {
            self.stop(id);
        }
    }

    pub fn current_run(&self, id: &str) -> Option<u64> {
        self.running.get(id).map(|running| running.run)
    }
}

/// One line, up to the limit. A longer line is read to its end and thrown away.
fn read_line_bounded(reader: &mut impl BufRead, line: &mut Vec<u8>) -> std::io::Result<usize> {
    let mut total = 0;
    loop {
        let buffer = reader.fill_buf()?;
        if buffer.is_empty() {
            return Ok(total);
        }
        let (chunk, done) = match buffer.iter().position(|&b| b == b'\n') {
            Some(at) => (&buffer[..=at], true),
            None => (buffer, false),
        };
        let used = chunk.len();
        if line.len() + used <= MAX_LINE_BYTES {
            line.extend_from_slice(chunk);
        } else {
            line.clear();
            line.extend_from_slice(b"{}");
        }
        total += used;
        reader.consume(used);
        if done {
            return Ok(total);
        }
    }
}

impl Drop for Runners {
    fn drop(&mut self) {
        self.stop_all();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("orbit-ext-test-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn manifest(id: &str, run: &str) -> String {
        format!(r#"{{"id":"{id}","name":"{id} name","run":[{run}],"commands":[{{"id":"search","title":"Search {id}"}}]}}"#)
    }

    #[test]
    fn extensions_are_found_and_bad_ones_say_why() {
        let data = temp("discover");
        let root = folder(&data);
        for (name, text) in [("b", manifest("beta", "\"b.exe\"")), ("a", manifest("alpha", "\"a.exe\"")), ("bad", "{".to_string()), ("twin", manifest("alpha", "\"x\""))] {
            std::fs::create_dir_all(root.join(name)).unwrap();
            std::fs::write(root.join(name).join(MANIFEST_FILE), text).unwrap();
        }
        std::fs::create_dir_all(root.join("not-an-extension")).unwrap();
        let (found, broken) = discover(&data);
        assert_eq!(found.iter().map(Installed::id).collect::<Vec<_>>(), ["alpha", "beta"]);
        assert_eq!(broken.len(), 2);
        assert!(broken.iter().any(|b| b.why.contains("already uses")));
        let _ = std::fs::remove_dir_all(data);
    }

    #[test]
    fn adding_copies_and_updates_in_place() {
        let data = temp("add");
        let source = temp("add-source");
        std::fs::write(source.join(MANIFEST_FILE), manifest("tool", "\"tool.exe\"")).unwrap();
        std::fs::write(source.join("tool.exe"), b"MZ").unwrap();
        let added = add(&source, &data).unwrap();
        assert_eq!(added.dir, folder(&data).join("tool"));
        assert!(added.dir.join("tool.exe").is_file());
        std::fs::write(source.join("new.txt"), b"x").unwrap();
        let again = add(&source.join(MANIFEST_FILE), &data).unwrap();
        assert!(again.dir.join("new.txt").is_file(), "adding again updates it");
        assert_eq!(discover(&data).0.len(), 1);
        remove(&again, &data).unwrap();
        assert!(discover(&data).0.is_empty());
        let _ = std::fs::remove_dir_all(data);
        let _ = std::fs::remove_dir_all(source);
    }

    #[test]
    fn programs_are_found_in_the_folder_then_on_the_path() {
        let dir = temp("resolve");
        std::fs::write(dir.join("main.ps1"), "").unwrap();
        std::fs::write(dir.join("tool.exe"), b"MZ").unwrap();
        let with = |run: &str| Installed { dir: dir.clone(), manifest: Manifest::parse(&manifest("x", run)).unwrap() };
        let (program, args) = resolve(&with("\"main.ps1\"")).unwrap();
        assert!(program.to_string_lossy().to_lowercase().ends_with("powershell.exe"));
        assert_eq!(args.last().map(PathBuf::from), Some(dir.join("main.ps1")));
        assert_eq!(resolve(&with("\"tool\"")).unwrap().0, dir.join("tool.exe"));
        assert_eq!(resolve(&with("\"tool.exe\"")).unwrap().0, dir.join("tool.exe"));
        assert!(resolve(&with("\"powershell\", \"-File\", \"main.ps1\"")).unwrap().0.is_file());
        assert!(resolve(&with("\"no-such-program-anywhere\"")).unwrap_err().contains("isn't installed"));
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn item_icons_stay_in_the_folder() {
        let installed = Installed { dir: PathBuf::from("C:\\ext"), manifest: Manifest::parse(&manifest("x", "\"a\"")).unwrap() };
        assert_eq!(installed.item_icon("icons/a.png"), Some(("C:\\ext\\icons\\a.png".into(), true)));
        assert_eq!(installed.item_icon("..\\..\\Windows\\a.png"), None);
        assert_eq!(installed.item_icon("C:\\Users\\a\\report.pdf"), Some(("C:\\Users\\a\\report.pdf".into(), false)), "a file's own icon");
        assert_eq!(installed.item_icon("C:\\Users\\a\\photo.png"), Some(("C:\\Users\\a\\photo.png".into(), true)));
    }

    #[test]
    fn long_lines_are_dropped_not_kept() {
        let long = "x".repeat(MAX_LINE_BYTES + 10);
        let input = format!("{long}\n{{\"type\":\"notify\",\"message\":\"hi\"}}\n");
        let mut reader = BufReader::with_capacity(64, input.as_bytes());
        let mut line = Vec::new();
        read_line_bounded(&mut reader, &mut line).unwrap();
        assert_eq!(line, b"{}");
        line.clear();
        read_line_bounded(&mut reader, &mut line).unwrap();
        assert!(String::from_utf8_lossy(&line).contains("notify"));
    }

    /// Runs the example extension in the repository, in PowerShell, the way the launcher does: a
    /// search goes in, and its answer comes back as a message to a window. The window is a
    /// message-only one, which is never on screen.
    #[test]
    fn the_example_extension_answers_searches() {
        use orbit_core::extension::ActionKind;
        use windows::Win32::UI::WindowsAndMessaging::{CreateWindowExW, DestroyWindow, HWND_MESSAGE, MSG, PM_REMOVE, PeekMessageW, WINDOW_EX_STYLE, WINDOW_STYLE, WM_APP};
        use windows::core::w;

        let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join(r"..\..\examples\extensions\windows-settings");
        let installed = read(&dir).expect("the example's manifest reads");
        if find_program("powershell").is_none() {
            eprintln!("No PowerShell here, so the example can't run.");
            return;
        }
        let window = unsafe { CreateWindowExW(WINDOW_EX_STYLE(0), w!("STATIC"), w!(""), WINDOW_STYLE(0), 0, 0, 0, 0, Some(HWND_MESSAGE), None, None, None) }.unwrap();
        let data = temp("example");
        let mut runners = Runners::new(data.clone());
        let message = WM_APP + 99;
        let mut answer = |id: u64, query: &str| -> (Vec<orbit_core::extension::Item>, Option<String>) {
            let request = Request::Search { id, command: "search".into(), query: query.into(), filter: String::new() };
            runners.send(&installed, &request, SendHwnd::new(window), message).expect("the example starts");
            let deadline = Instant::now() + Duration::from_secs(30);
            while Instant::now() < deadline {
                let mut msg = MSG::default();
                if unsafe { PeekMessageW(&mut msg, Some(window), message, message, PM_REMOVE) }.as_bool() {
                    let event = crate::util::take_posted::<Event>(msg.wParam.0).expect("an event");
                    match event.kind {
                        EventKind::Said(Message::Items { id: answered, items, empty, .. }) if answered == id => return (items, empty),
                        EventKind::Said(_) => {}
                        EventKind::Ended(why) => panic!("the example stopped: {why}"),
                    }
                } else {
                    std::thread::sleep(Duration::from_millis(20));
                }
            }
            panic!("no answer to \"{query}\" in 30 seconds");
        };

        let (items, _) = answer(1, "blue");
        assert_eq!(items[0].id, "ms-settings:bluetooth");
        assert_eq!(items[0].title, "Bluetooth & devices");
        assert_eq!(items[0].actions[0].kind, ActionKind::Open("ms-settings:bluetooth".into()));
        assert_eq!(items[0].actions[1].kind, ActionKind::Copy("ms-settings:bluetooth".into()));
        // The same program answers the next search.
        assert_eq!(answer(2, "dark mode").0[0].id, "ms-settings:colors");
        // Text outside ASCII goes there and back intact: it says what it didn't find.
        let (items, empty) = answer(3, "caf\u{e9} \u{1F600}");
        assert!(items.is_empty());
        assert_eq!(empty.as_deref(), Some("No Settings page matches \"caf\u{e9} \u{1F600}\""));
        assert!(answer(4, "").0.len() > 50, "an empty search lists every page");
        assert_eq!(runners.current_run(installed.id()), Some(1), "one program, started once");

        runners.stop_all();
        unsafe {
            let _ = DestroyWindow(window);
        }
        let _ = std::fs::remove_dir_all(data);
    }
}
