//! Feature tests on the real app: default shortcut and fallback, notes, snippets, clipboard,
//! HyperKey and settings. Run against a private data directory.
//!
//! These touch shared OS state, so `Restore` puts it back even if a check panics: modifier keys,
//! Caps Lock's toggle, and the clipboard. Clipboard contents are copied format by format and put
//! back. If any format can't be copied that way, the clipboard tests are skipped and reported.

use super::*;
use orbit_core::Settings;
use orbit_core::bindings_text::parse_hyper;
use orbit_core::settings::CustomCommand;
use orbit_data::{ClipItem, ClipKind, SnippetInput, Store};
use std::path::PathBuf;
use windows::Win32::System::DataExchange::*;
use windows::Win32::System::Memory::{GMEM_MOVEABLE, GlobalAlloc, GlobalLock, GlobalSize, GlobalUnlock};

const CF_UNICODETEXT: u32 = 13;
const EM_SETPASSWORDCHAR: u32 = 0x00CC;
/// Windows makes these from other formats that are copied, so they come back by themselves.
const SYNTHESIZED_FORMATS: &[u32] = &[1, 2, 3, 7, 8, 14, 16, 17];
/// Formats whose handle isn't a block of memory, so they can't be copied byte by byte.
fn format_is_not_memory(format: u32) -> bool {
    matches!(format, 2 | 3 | 9 | 14 | 0x80..=0x8F | 0x300..=0x3FF)
}
const VK_CAPS: u16 = 0x14;
const HELD_MODIFIERS: [u16; 8] = [0xA2, 0xA3, 0xA4, 0xA5, 0xA0, 0xA1, 0x5B, 0x5C];

fn open_clipboard() -> bool {
    open_clipboard_as(None)
}

/// Whoever opens the clipboard and empties it becomes its owner.
fn open_clipboard_as(owner: Option<HWND>) -> bool {
    for _ in 0..50 {
        if unsafe { OpenClipboard(owner) }.is_ok() {
            return true;
        }
        sleep(10);
    }
    false
}

/// A copy of everything on the clipboard.
struct ClipboardBackup(Vec<(u32, Vec<u8>)>);

fn backup_clipboard() -> Option<ClipboardBackup> {
    if !open_clipboard() {
        return None;
    }
    let mut saved = Vec::new();
    let mut formats = Vec::new();
    unsafe {
        let mut format = 0;
        loop {
            format = EnumClipboardFormats(format);
            if format == 0 {
                break;
            }
            formats.push(format);
        }
        let mut copyable = true;
        for format in &formats {
            let format = *format;
            let has_source = |wanted: &[u32]| wanted.iter().any(|f| formats.contains(f));
            if format_is_not_memory(format) {
                // A bitmap comes back from its DIB, and a metafile from its counterpart.
                let synthesized = match format {
                    2 => has_source(&[8, 17]),
                    3 => has_source(&[14]),
                    14 => has_source(&[3]),
                    _ => false,
                };
                copyable &= synthesized;
                continue;
            }
            if SYNTHESIZED_FORMATS.contains(&format) && format != 8 && has_source(&[13]) && matches!(format, 1 | 7 | 16) {
                continue;
            }
            let Ok(handle) = GetClipboardData(format) else {
                copyable = false;
                continue;
            };
            let global = HGLOBAL(handle.0);
            let size = GlobalSize(global);
            let pointer = GlobalLock(global) as *const u8;
            if pointer.is_null() {
                copyable = false;
                continue;
            }
            saved.push((format, std::slice::from_raw_parts(pointer, size).to_vec()));
            let _ = GlobalUnlock(global);
        }
        let _ = CloseClipboard();
        copyable.then_some(ClipboardBackup(saved))
    }
}

fn restore_clipboard(backup: &ClipboardBackup) -> bool {
    if !open_clipboard() {
        return false;
    }
    let mut ok = true;
    unsafe {
        let _ = EmptyClipboard();
        for (format, bytes) in &backup.0 {
            match GlobalAlloc(GMEM_MOVEABLE, bytes.len().max(1)) {
                Ok(global) => {
                    let target = GlobalLock(global) as *mut u8;
                    std::ptr::copy_nonoverlapping(bytes.as_ptr(), target, bytes.len());
                    let _ = GlobalUnlock(global);
                    ok &= SetClipboardData(*format, Some(HANDLE(global.0))).is_ok();
                }
                Err(_) => ok = false,
            }
        }
        let _ = CloseClipboard();
    }
    ok
}

fn set_clipboard(text: Option<&str>, sensitive: bool) -> bool {
    if !open_clipboard() {
        return false;
    }
    unsafe {
        let _ = EmptyClipboard();
        let mut ok = true;
        if let Some(text) = text {
            let units = wide(text);
            let bytes = units.len() * 2;
            if let Ok(global) = GlobalAlloc(GMEM_MOVEABLE, bytes) {
                let target = GlobalLock(global) as *mut u16;
                std::ptr::copy_nonoverlapping(units.as_ptr(), target, units.len());
                let _ = GlobalUnlock(global);
                ok = SetClipboardData(CF_UNICODETEXT, Some(HANDLE(global.0))).is_ok();
            }
            if sensitive {
                let format = RegisterClipboardFormatW(w!("ExcludeClipboardContentFromMonitorProcessing"));
                if let Ok(global) = GlobalAlloc(GMEM_MOVEABLE, 4) {
                    let _ = SetClipboardData(format, Some(HANDLE(global.0)));
                }
            }
        }
        let _ = CloseClipboard();
        ok
    }
}

fn held(vk: u16) -> bool {
    unsafe { GetAsyncKeyState(vk as i32) as u16 & 0x8000 != 0 }
}

fn caps_on() -> bool {
    unsafe { GetKeyState(VK_CAPS as i32) & 1 != 0 }
}

fn modifiers_clear() -> bool {
    !HELD_MODIFIERS.iter().any(|vk| held(*vk))
}

fn release_everything() {
    // A lone Win or Alt key-up would open the Start menu or a menu bar, and all four together are
    // the Office key. An unassigned key in between makes the release mean nothing.
    if !modifiers_clear() {
        send(&chord_inputs(&[0xFF]));
    }
    for vk in HELD_MODIFIERS.iter().chain([VK_CAPS, VK_CONTROL.0, VK_MENU.0, VK_SHIFT.0].iter()) {
        if held(*vk) {
            send(&[key_input(*vk, true)]);
        }
    }
}

/// Puts shared OS state back, however the run ends.
struct Restore {
    caps_was_on: bool,
    clipboard: Option<ClipboardBackup>,
}

impl Drop for Restore {
    fn drop(&mut self) {
        release_everything();
        sleep(50);
        if caps_on() != self.caps_was_on {
            send(&chord_inputs(&[VK_CAPS]));
        }
        if let Some(backup) = self.clipboard.take() {
            if !restore_clipboard(&backup) {
                eprintln!("WARNING: the clipboard could not be restored exactly");
            }
        }
    }
}

/// Foreground belongs to this benchmark or to the Orbit under test.
fn ours(pid: u32) -> bool {
    unsafe {
        let mut owner = 0u32;
        GetWindowThreadProcessId(GetForegroundWindow(), Some(&mut owner));
        owner == pid || owner == std::process::id()
    }
}

fn find_window(pid: u32, class: PCWSTR) -> Option<HWND> {
    unsafe {
        let mut after = None;
        loop {
            let hwnd = FindWindowExW(None, after, class, PCWSTR::null()).ok()?;
            let mut owner = 0u32;
            GetWindowThreadProcessId(hwnd, Some(&mut owner));
            if owner == pid {
                return Some(hwnd);
            }
            after = Some(hwnd);
        }
    }
}

/// A control on a Settings page. Pages sit two windows down, inside the part that scrolls.
fn settings_control(window: HWND, id: i32) -> Option<HWND> {
    unsafe {
        let view = FindWindowExW(Some(window), None, w!("OrbitSettingsView"), PCWSTR::null()).ok()?;
        let page = FindWindowExW(Some(view), None, w!("OrbitSettingsPage"), PCWSTR::null()).ok()?;
        GetDlgItem(Some(page), id).ok()
    }
}

/// Full text of a window in another process, of any length.
fn long_text(hwnd: HWND) -> String {
    unsafe {
        let mut length = 0usize;
        let _ = SendMessageTimeoutW(hwnd, WM_GETTEXTLENGTH, WPARAM(0), LPARAM(0), SMTO_ABORTIFHUNG, 5000, Some(&mut length));
        let mut buffer = vec![0u16; length + 2];
        let mut copied = 0usize;
        let _ = SendMessageTimeoutW(
            hwnd,
            WM_GETTEXT,
            WPARAM(buffer.len()),
            LPARAM(buffer.as_mut_ptr() as isize),
            SMTO_ABORTIFHUNG,
            5000,
            Some(&mut copied),
        );
        String::from_utf16_lossy(&buffer[..copied.min(buffer.len())])
    }
}

fn type_text(text: &str) {
    for unit in text.encode_utf16() {
        send(&char_inputs(unit));
        sleep(2);
    }
}

/// Types as real keys, which is what the hook's trigger matching needs.
fn type_keys(text: &str) {
    for c in text.chars() {
        send(&chord_inputs(&[c.to_ascii_uppercase() as u16]));
        sleep(8);
    }
}

fn press(vk: u16) {
    send(&chord_inputs(&[vk]));
}

fn launch_session(options: &E2eOptions, dir: &Path, log: &Path, hotkey: Option<&str>) -> Option<Session> {
    launch_session_with(options, dir, log, hotkey, true)
}

/// `accept_injected`: every key in these tests is injected, and Orbit normally ignores input
/// injected by other programs, so it has to be told to treat these as real.
fn launch_session_with(options: &E2eOptions, dir: &Path, log: &Path, hotkey: Option<&str>, accept_injected: bool) -> Option<Session> {
    let mut command = Command::new(&options.orbit);
    command.arg("--data-dir").arg(dir).args(["--no-reindex", "--no-trim", "--bench-log"]).arg(log);
    if accept_injected {
        command.arg("--accept-injected-input");
    }
    if let Some(hotkey) = hotkey {
        command.args(["--hotkey", hotkey]);
    }
    let child = command.spawn().ok()?;
    let pid = child.id();
    let start = Instant::now();
    let mut main = None;
    while main.is_none() && start.elapsed() < Duration::from_secs(10) {
        main = find_main_window(pid);
        sleep(2);
    }
    let main = main?;
    let mut result = 0usize;
    unsafe {
        let _ = SendMessageTimeoutW(main, WM_ORBIT_BENCH_FLUSH, WPARAM(0), LPARAM(0), SMTO_NORMAL, 5000, Some(&mut result));
    }
    let edit = unsafe { FindWindowExW(Some(main), None, w!("EDIT"), PCWSTR::null()) }.ok()?;
    Some(Session { child, pid, main, edit, log: log.to_path_buf(), log_read: 0, freq: qpc_freq() })
}

fn quit(session: &mut Session) -> bool {
    unsafe {
        let _ = PostMessageW(Some(session.main), WM_ORBIT_RUN_BUILTIN, QUIT, LPARAM(0));
    }
    let start = Instant::now();
    while start.elapsed() < Duration::from_secs(5) {
        if session.child.try_wait().ok().flatten().is_some() {
            return true;
        }
        sleep(5);
    }
    let _ = session.child.kill();
    let _ = session.child.wait();
    false
}

fn fresh_dir(root: &Path, name: &str, settings: &Settings) -> (PathBuf, PathBuf) {
    let dir = root.join(name);
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("data dir");
    if let Some(source) = root.join("apps.json").exists().then(|| root.join("apps.json")) {
        let _ = std::fs::copy(source, dir.join("apps.json"));
    }
    std::fs::write(dir.join("settings.json"), settings.to_json()).expect("settings");
    let log = dir.join("timings.csv");
    (dir, log)
}

/// No quick links: a query with no results must never turn Enter into a web search here.
fn base_settings() -> Settings {
    // No links, and no icons fetched for any a test adds: the suite never touches the network.
    Settings { onboarded: true, links: Vec::new(), fetch_link_icons: false, ..Settings::default() }
}

/// Opens the launcher, types a query, and waits for results from the data thread.
fn run_query(session: &Session, chord: &[INPUT], query: &str) -> bool {
    send(chord);
    if !wait_until(Duration::from_secs(1), || session.focused()) {
        return false;
    }
    sleep(30);
    type_text(query);
    sleep(180);
    session.focused()
}

pub fn run(report: &mut Report, options: &E2eOptions) {
    let root = options.work_dir.join("features");
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).expect("work dir");
    let indexed = Command::new(&options.orbit).args(["--index-apps", "--data-dir"]).arg(&root).status();
    if !indexed.is_ok_and(|s| s.success()) {
        report.not_tested.push("features: the app index helper failed".to_string());
    }

    let backup = backup_clipboard();
    let plain_clipboard = backup.is_some();
    let _restore = Restore { caps_was_on: caps_on(), clipboard: backup };
    let sink = Sink::start();
    if !sink.focus() {
        report.not_tested.push("features: Windows wouldn't give the benchmark window focus".to_string());
        return;
    }
    let chord = chord_inputs(&HOTKEY_VKS);
    describe_data_drive(report, &root);

    default_hotkey(report, options, &root, &sink);
    notes(report, options, &root, &sink, &chord);
    snippets(report, options, &root, &sink, &chord, plain_clipboard);
    if plain_clipboard {
        clipboard_history(report, options, &root, &sink, &chord);
    } else {
        report.not_tested.push(
            "clipboard history and {clipboard} snippets: your clipboard held a format the test couldn't copy and put back, so it was left alone".to_string(),
        );
    }
    hyper_internal(report, options, &root, &sink);
    hyper_synthetic(report, options, &root, &sink);
    settings_window(report, options, &root, &sink, &chord);
    footer_buttons(report, options, &root, &sink, &chord);
    orbit_menu(report, options, &root, &sink, &chord);
    launcher_stands_aside(report, options, &root, &sink, &chord);
    if plain_clipboard {
        emoji_list(report, options, &root, &sink, &chord);
    } else {
        report.not_tested.push("the emoji list: your clipboard held a format the test couldn't copy and put back, so it was left alone".to_string());
    }
    vim_keys(report, options, &root, &sink, &chord);
    full_vim(report, options, &root, &sink, &chord);
    if plain_clipboard {
        clipboard_rules(report, options, &root, &sink);
    } else {
        report.not_tested.push(
            "clipboard rules (excluded apps, size limit, file lists, images): your clipboard held a format the test couldn't copy and put back, so it was left alone".to_string(),
        );
    }
    hyper_runs_once(report, options, &root, &sink);
    hyper_right_modifiers(report, options, &root, &sink);
    synthetic_hard_kill(report, options, &root, &sink);
    missing_executable(report, options, &root, &sink, &chord);
    launcher_rows(report, options, &root, &sink, &chord);
    undo_trash(report, options, &root, &sink, &chord);
    hotkey_from_menu(report, options, &root, &sink, &chord);
    hotkey_toggles_app(report, options, &root, &sink);
    game_detection(report, options, &root, &sink);
    finding_things(report, options, &root, &sink, &chord);
    mac_feel(report, options, &root, &sink);
    if plain_clipboard {
        integrations(report, options, &root, &sink, &chord);
    } else {
        report.not_tested.push("integrations: your clipboard held a format the test couldn't copy and put back, so it was left alone".to_string());
    }
    reset_shortcuts(report, options, &root, &sink, &chord);
    delete_local_data(report, options, &root, &sink, &chord);
    leader_mode(report, options, &root, &sink);
    note_survives_kill(report, options, &root, &sink, &chord);
    hundred_note_opens(report, options, &root, &sink, &chord);
    footprint_by_profile(report, options, &root);
    release_everything();
}

fn default_hotkey(report: &mut Report, options: &E2eOptions, root: &Path, sink: &Sink) {
    let alt_space = chord_inputs(&[VK_MENU.0, VK_SPACE.0]);
    let ctrl_alt_space = chord_inputs(&[VK_CONTROL.0, VK_MENU.0, VK_SPACE.0]);
    let (dir, log) = fresh_dir(root, "hotkey", &base_settings());

    // The default, with nothing else holding it.
    let Some(mut session) = launch_session(options, &dir, &log, None) else {
        report.check("default shortcut: Orbit starts", 0, 1);
        return;
    };
    sleep(400);
    let mut opens = 0;
    let mut no_menu = 0;
    for _ in 0..10 {
        sink.focus();
        send(&alt_space);
        if wait_until(Duration::from_secs(1), || session.focused() && session.visible()) {
            opens += 1;
        }
        sleep(120);
        // Releasing Alt must leave focus in the input, with no menu mode swallowing typing.
        send(&char_inputs('k' as u16));
        sleep(60);
        if read_text(session.edit) == "k" {
            no_menu += 1;
        }
        session.hide();
        sleep(50);
    }
    report.check("Alt+Space opens the launcher", opens, 10);
    report.check("Alt+Space leaves the input ready to type", no_menu, 10);
    quit(&mut session);

    // Something else owns Alt+Space: Orbit must fall back to the next default and still work.
    const ID: i32 = 7001;
    let taken = unsafe { RegisterHotKey(None, ID, MOD_ALT | MOD_NOREPEAT, VK_SPACE.0 as u32) }.is_ok();
    if !taken {
        report.not_tested.push("shortcut fallback: couldn't reserve Alt+Space for the test".to_string());
        return;
    }
    let (dir, log) = fresh_dir(root, "hotkey2", &base_settings());
    if let Some(mut session) = launch_session(options, &dir, &log, None) {
        sleep(400);
        sink.focus();
        send(&ctrl_alt_space);
        let fell_back = wait_until(Duration::from_secs(1), || session.focused() && session.visible());
        report.check("when Alt+Space is taken, Ctrl+Alt+Space works instead", fell_back as u32, 1);
        session.hide();
        quit(&mut session);
    }
    unsafe {
        let _ = UnregisterHotKey(None, ID);
    }
    sink.focus();
}

fn notes(report: &mut Report, options: &E2eOptions, root: &Path, sink: &Sink, chord: &[INPUT]) {
    let (dir, log) = fresh_dir(root, "notes", &base_settings());
    let big_body = {
        let line = "The quick brown fox jumps over the lazy dog, again and again.\n";
        format!("Big bench note\n{}", line.repeat(5 * 1024 * 1024 / line.len()))
    };
    {
        let store = Store::open(&dir, true).expect("store");
        store.create_note(&big_body).expect("big note");
    }
    let Some(mut session) = launch_session(options, &dir, &log, Some(HOTKEY_TEXT)) else {
        report.check("notes: Orbit starts", 0, 1);
        return;
    };
    let process = process_handle(&session.child);
    sleep(400);
    let before = memory_of(process);
    let pid = session.pid;
    let open_note_window = || wait_until(Duration::from_secs(2), || {
        find_window(pid, w!("OrbitNote")).is_some_and(|hwnd| unsafe { GetForegroundWindow() } == hwnd)
    });

    // Create, type, autosave.
    sink.focus();
    let mut created = false;
    if run_query(&session, chord, "new note") {
        press(VK_RETURN.0);
        created = open_note_window();
    }
    report.check("New Note opens a focused editor", created as u32, 1);
    if !created {
        quit(&mut session);
        return;
    }
    let editor = find_window(pid, w!("OrbitNote")).unwrap_or_default();
    let rich = unsafe { FindWindowExW(Some(editor), None, w!("RICHEDIT50W"), PCWSTR::null()) }.unwrap_or_default();
    report.check("note editor uses RichEdit", (!rich.is_invalid()) as u32, 1);
    type_text("Bench title");
    press(VK_RETURN.0);
    sleep(20);
    type_text("second line ünï 🎉");
    sleep(800);
    let saved = {
        let store = Store::open(&dir, true).expect("store");
        let found = store.search_notes("second", 5).expect("search");
        found.first().filter(|n| n.title == "Bench title").and_then(|n| store.note_body(n.id).ok().flatten()).is_some_and(|b| b.contains("ünï 🎉"))
    };
    report.check("note autosaves while the editor is open, with accents and emoji intact", saved as u32, 1);

    // Closing straight after typing must not lose the text.
    type_text(" final words");
    press(VK_ESCAPE.0);
    let closed = wait_until(Duration::from_secs(1), || find_window(pid, w!("OrbitNote")).is_none());
    sleep(200);
    let flushed = {
        let store = Store::open(&dir, true).expect("store");
        let found = store.search_notes("final", 5).expect("search");
        found.first().and_then(|n| store.note_body(n.id).ok().flatten()).is_some_and(|b| b.contains("final words") && b.contains('🎉') && b.starts_with("Bench title"))
    };
    report.check("Esc closes the editor", closed as u32, 1);
    report.check("closing right after typing keeps every character", flushed as u32, 1);

    // Find the note from the launcher and reopen it.
    sink.focus();
    let mut reopened = false;
    if run_query(&session, chord, "bench title") {
        press(VK_RETURN.0);
        reopened = open_note_window();
    }
    let body_ok = reopened
        && find_window(pid, w!("OrbitNote"))
            .and_then(|h| unsafe { FindWindowExW(Some(h), None, w!("RICHEDIT50W"), PCWSTR::null()) }.ok())
            .is_some_and(|edit| long_text(edit).contains("final words"));
    report.check("a note found from the launcher reopens with its text", body_ok as u32, 1);
    if reopened {
        press(VK_ESCAPE.0);
        wait_until(Duration::from_secs(1), || find_window(pid, w!("OrbitNote")).is_none());
    }

    sleep(1500);
    report.add("notes: private memory after small notes closed", memory_of(process).private_kb, "KB", false);

    // Undo goes back through more than one edit. Typed runs group together, as in any editor,
    // so each edit here is separated by a caret move.
    sink.focus();
    let mut undo_ok = false;
    if run_query(&session, chord, "new note") {
        press(VK_RETURN.0);
        if open_note_window() {
            let editor = find_window(pid, w!("OrbitNote")).unwrap_or_default();
            let rich = unsafe { FindWindowExW(Some(editor), None, w!("RICHEDIT50W"), PCWSTR::null()) }.unwrap_or_default();
            type_text("one");
            sleep(40);
            press(VK_LEFT.0);
            sleep(40);
            type_text("two");
            sleep(40);
            press(VK_RIGHT.0);
            sleep(40);
            type_text("three");
            sleep(40);
            let undo = || {
                send(&chord_inputs(&[VK_CONTROL.0, 'Z' as u16]));
                sleep(60);
                long_text(rich)
            };
            let full = long_text(rich);
            let first = undo();
            let second = undo();
            undo_ok = full == "ontwoethree" && first == "ontwoe" && second == "one";
            if !undo_ok {
                report.not_tested.push(format!("note undo: got {full:?}, then {first:?}, then {second:?}"));
            }
            press(VK_ESCAPE.0);
            wait_until(Duration::from_secs(1), || find_window(pid, w!("OrbitNote")).is_none());
        }
    }
    report.check("note undo is multi-level", undo_ok as u32, 1);

    // A 5 MB note, well past the control's default limit.
    sink.focus();
    let mut big_opened = false;
    if run_query(&session, chord, "big bench note") {
        let t = Instant::now();
        press(VK_RETURN.0);
        big_opened = wait_until(Duration::from_secs(10), || {
            find_window(pid, w!("OrbitNote"))
                .and_then(|h| unsafe { FindWindowExW(Some(h), None, w!("RICHEDIT50W"), PCWSTR::null()) }.ok())
                .is_some_and(|edit| {
                    let mut length = 0usize;
                    unsafe {
                        let _ = SendMessageTimeoutW(edit, WM_GETTEXTLENGTH, WPARAM(0), LPARAM(0), SMTO_ABORTIFHUNG, 2000, Some(&mut length));
                    }
                    length >= big_body.len()
                })
        });
        if big_opened {
            report.add("notes: open a 5 MB note, Enter to text loaded", t.elapsed().as_secs_f64() * 1000.0, "ms", false);
        }
    }
    report.check("a 5 MB note opens whole", big_opened as u32, 1);
    if big_opened {
        let t = Instant::now();
        type_text("x");
        sleep(700);
        report.add("notes: 5 MB note, one edit plus autosave, wall clock including 400 ms debounce", t.elapsed().as_secs_f64() * 1000.0, "ms", false);
        press(VK_ESCAPE.0);
        wait_until(Duration::from_secs(3), || find_window(pid, w!("OrbitNote")).is_none());
    }
    sleep(1500);
    let after = memory_of(process);
    report.add("notes: private memory before opening any note", before.private_kb, "KB", false);
    report.add("notes: private memory after editors closed", after.private_kb, "KB", false);
    let clean = quit(&mut session);
    report.check("notes: clean exit", clean as u32, 1);
    sink.focus();
}

fn snippets(report: &mut Report, options: &E2eOptions, root: &Path, sink: &Sink, chord: &[INPUT], plain_clipboard: bool) {
    let mut settings = base_settings();
    settings.snippets.expansion_enabled = true;
    // Well past one typed chunk, with an emoji straddling each place a chunk could end, since a
    // surrogate pair split between chunks would arrive broken.
    let long_snippet = format!("{}end", format!("{}🎉", "x".repeat(127)).repeat(24));
    let huge_snippet = "x".repeat(20_000);
    let (dir, log) = fresh_dir(root, "snippets", &settings);
    {
        let store = Store::open(&dir, true).expect("store");
        let input = |name: &str, trigger: &str, content: &str| SnippetInput {
            name: name.into(),
            trigger: trigger.into(),
            content: content.into(),
            description: String::new(),
            category: String::new(),
            favourite: false,
            enabled: true,
        };
        store.create_snippet(&input("Bench Greeting", "", "hello from orbit")).expect("snippet");
        store.create_snippet(&input("Bench Trigger", "zzgr", "expanded text")).expect("snippet");
        store.create_snippet(&input("Bench Cursor", "zzcu", "ab{cursor}cd")).expect("snippet");
        store.create_snippet(&input("Bench Clip", "zzcb", "pasted: {clipboard}")).expect("snippet");
        store.create_snippet(&input("Bench Long", "zzlg", &long_snippet)).expect("snippet");
        store.create_snippet(&input("Bench Huge", "zzhg", &huge_snippet)).expect("snippet");
    }
    let Some(mut session) = launch_session(options, &dir, &log, Some(HOTKEY_TEXT)) else {
        report.check("snippets: Orbit starts", 0, 1);
        return;
    };
    sleep(500);

    // Insert from the launcher, back into the previous window.
    let mut inserted = 0;
    for _ in 0..5 {
        sink.focus();
        sink.take_text();
        if run_query(&session, chord, "bench greeting") {
            press(VK_RETURN.0);
            wait_until(Duration::from_secs(1), || read_text(HWND(sink.edit as *mut _)) == "hello from orbit");
            if sink.take_text() == "hello from orbit" {
                inserted += 1;
            }
        }
        sleep(80);
    }
    report.check("a snippet chosen in the launcher is typed into the previous window", inserted, 5);

    // Trigger expansion.
    let mut expanded = 0;
    for _ in 0..10 {
        sink.focus();
        sink.take_text();
        type_keys("zzgr");
        wait_until(Duration::from_millis(600), || read_text(HWND(sink.edit as *mut _)) == "expanded text");
        if sink.take_text() == "expanded text" {
            expanded += 1;
        }
    }
    report.check("typing a trigger replaces it with the snippet", expanded, 10);

    sink.focus();
    sink.take_text();
    type_keys("zzcu");
    sleep(300);
    type_text("X");
    sleep(80);
    report.check("{cursor} leaves the caret in the right place", (sink.take_text() == "abXcd") as u32, 1);

    // Near misses must stay as typed.
    sink.focus();
    sink.take_text();
    type_keys("zzg");
    sleep(100);
    press(VK_BACK.0);
    sleep(100);
    report.check("an unfinished trigger is left alone", (sink.take_text() == "zz") as u32, 1);

    // A long snippet is typed, not pasted, so the clipboard stays as the user left it.
    if plain_clipboard {
        set_clipboard(Some("orbit keeps this"), false);
        sleep(100);
        sink.focus();
        sink.take_text();
        let t = Instant::now();
        type_keys("zzlg");
        let typed = wait_until(Duration::from_secs(15), || long_text(HWND(sink.edit as *mut _)) == long_snippet);
        report.add("snippets: 3099-unit snippet, trigger to text in place", t.elapsed().as_secs_f64() * 1000.0, "ms", false);
        report.check("a long snippet arrives whole, emoji intact", typed as u32, 1);
        if !typed {
            let got = long_text(HWND(sink.edit as *mut _));
            let first_bad = got.chars().zip(long_snippet.chars()).position(|(a, b)| a != b);
            let mut flushed = 0usize;
            unsafe {
                let _ = SendMessageTimeoutW(session.main, WM_ORBIT_BENCH_FLUSH, WPARAM(0), LPARAM(0), SMTO_NORMAL, 5000, Some(&mut flushed));
            }
            let stopped: Vec<String> = std::fs::read_to_string(&log)
                .unwrap_or_default()
                .lines()
                .filter(|line| line.contains("typing-stopped"))
                .map(String::from)
                .collect();
            let tail: String = got.chars().rev().take(6).collect::<Vec<_>>().into_iter().rev().map(|c| format!("{:04x} ", c as u32)).collect();
            report.not_tested.push(format!(
                "long snippet: got {} chars, wanted {}, first difference at {first_bad:?}, tail {tail}, stops {stopped:?}, foreground is sink: {}",
                got.chars().count(),
                long_snippet.chars().count(),
                unsafe { GetForegroundWindow() } == sink.hwnd(),
            ));
        }
        report.check("typing a long snippet leaves the clipboard alone", (clipboard_text().as_deref() == Some("orbit keeps this")) as u32, 1);
        sink.take_text();

        // Past the typing limit it's handed over on the clipboard, with nothing erased first.
        sink.focus();
        type_keys("zzhg");
        // One read after a wait: polling would hold the clipboard against Orbit writing to it.
        sleep(1000);
        let handed_over = clipboard_text().as_deref() == Some(huge_snippet.as_str());
        report.check("a snippet too long to type goes on the clipboard instead", handed_over as u32, 1);
        report.check("that keeps the typed trigger, since nothing was inserted", (sink.take_text() == "zzhg") as u32, 1);
    }

    // A password field: nothing gets expanded in it, and it's UI Automation that says so.
    sink.focus();
    sink.take_text();
    let set_password = |on: bool| unsafe {
        SendMessageW(HWND(sink.edit as *mut _), EM_SETPASSWORDCHAR, Some(WPARAM(if on { '*' as usize } else { 0 })), None);
    };
    set_password(true);
    type_keys("zzgr");
    sleep(1200);
    let kept = sink.take_text() == "zzgr";
    set_password(false);
    report.check("a trigger typed in a password field isn't expanded", kept as u32, 1);
    let mut flushed = 0usize;
    unsafe {
        let _ = SendMessageTimeoutW(session.main, WM_ORBIT_BENCH_FLUSH, WPARAM(0), LPARAM(0), SMTO_NORMAL, 5000, Some(&mut flushed));
    }
    let log_text = std::fs::read_to_string(&log).unwrap_or_default();
    let reported = log_text.contains("action,password-reported");
    report.check("UI Automation is what recognised that password field", reported as u32, 1);
    let mut checks: Vec<f64> = log_text
        .lines()
        .filter_map(|line| line.strip_prefix("action,password-check,0,"))
        .filter_map(|ticks| ticks.trim().parse::<f64>().ok())
        .map(|ticks| ticks * 1000.0 / qpc_freq() as f64)
        .collect();
    if let Some(first) = checks.first().copied() {
        report.add("snippets: password check, first in the session", first, "ms", false);
    }
    if checks.len() > 1 {
        checks.remove(0);
        report.add_timings("snippets: password check, later ones", &checks, false);
    }

    // Orbit's own windows are out of bounds: a search for a trigger is only a search.
    sink.focus();
    send(chord);
    if wait_until(Duration::from_secs(1), || session.visible() && session.focused()) {
        type_keys("zzgr");
        sleep(400);
        report.check("a trigger typed into Orbit's own search box isn't expanded", (read_text(session.edit) == "zzgr") as u32, 1);
        press(VK_ESCAPE.0);
        sleep(60);
        press(VK_ESCAPE.0);
        wait_until(Duration::from_secs(1), || !session.visible());
    } else {
        report.check("a trigger typed into Orbit's own search box isn't expanded", 0, 1);
    }
    sink.focus();
    sink.take_text();

    // A pause means the caret may have moved, so the earlier characters no longer count.
    type_keys("zz");
    sleep(5300);
    type_keys("gr");
    sleep(400);
    report.check("a long pause inside a trigger stops it matching", (sink.take_text() == "zzgr") as u32, 1);

    if plain_clipboard {
        // {clipboard} content must never be dropped into a window unseen.
        set_clipboard(Some("clip content"), false);
        sleep(100);
        sink.focus();
        sink.take_text();
        type_keys("zzcb");
        let asked = wait_until(Duration::from_secs(1), || session.visible() && session.focused());
        sleep(80);
        let held_back = !sink_has(sink, "pasted");
        report.check("a {clipboard} snippet asks before inserting", (asked && held_back) as u32, 1);
        press(VK_DOWN.0);
        sleep(40);
        press(VK_RETURN.0);
        wait_until(Duration::from_secs(1), || sink_has(sink, "clip content"));
        report.check("approving it inserts the clipboard text", sink_has(sink, "pasted: clip content") as u32, 1);
        sink.take_text();

        // Content its owner marked as not to be kept doesn't end up in a snippet.
        set_clipboard(Some("orbitsecretclip"), true);
        sleep(100);
        sink.focus();
        sink.take_text();
        type_keys("zzcb");
        if wait_until(Duration::from_millis(500), || session.visible() && session.focused()) {
            press(VK_DOWN.0);
            sleep(40);
            press(VK_RETURN.0);
        }
        sleep(400);
        report.check("a {clipboard} snippet doesn't use a copy marked sensitive", (!sink_has(sink, "orbitsecretclip")) as u32, 1);
        sink.take_text();
    }
    report.add("snippets: private memory with the keyboard hook on", memory_of(process_handle(&session.child)).private_kb, "KB", false);
    quit(&mut session);
    sink.focus();

    // Without the test switch, another program's injected keys aren't typing.
    let log = dir.join("timings-physical.csv");
    if let Some(mut session) = launch_session_with(options, &dir, &log, Some(HOTKEY_TEXT), false) {
        sleep(500);
        sink.focus();
        sink.take_text();
        type_keys("zzgr");
        sleep(500);
        report.check("keys injected by another program don't trigger expansion", (sink.take_text() == "zzgr") as u32, 1);
        quit(&mut session);
        sink.focus();
    } else {
        report.check("keys injected by another program don't trigger expansion", 0, 1);
    }
}

fn clipboard_text() -> Option<String> {
    if !open_clipboard() {
        return None;
    }
    unsafe {
        let text = GetClipboardData(CF_UNICODETEXT).ok().and_then(|handle| {
            let global = HGLOBAL(handle.0);
            let pointer = GlobalLock(global) as *const u16;
            if pointer.is_null() {
                return None;
            }
            let units = std::slice::from_raw_parts(pointer, GlobalSize(global) / 2);
            let length = units.iter().position(|&c| c == 0).unwrap_or(units.len());
            let text = String::from_utf16_lossy(&units[..length]);
            let _ = GlobalUnlock(global);
            Some(text)
        });
        let _ = CloseClipboard();
        text
    }
}

fn sink_has(sink: &Sink, needle: &str) -> bool {
    read_text(HWND(sink.edit as *mut _)).contains(needle)
}

fn clipboard_history(report: &mut Report, options: &E2eOptions, root: &Path, sink: &Sink, chord: &[INPUT]) {
    let mut settings = base_settings();
    settings.clipboard.enabled = true;
    let (dir, log) = fresh_dir(root, "clipboard", &settings);
    let Some(mut session) = launch_session(options, &dir, &log, Some(HOTKEY_TEXT)) else {
        report.check("clipboard: Orbit starts", 0, 1);
        return;
    };
    sleep(400);
    let search = |text: &str| {
        Store::open(&dir, true).ok().and_then(|store| store.search_clips(text, 5).ok()).map_or(0, |found| found.len())
    };
    set_clipboard(Some("orbitcliptest ordinary"), false);
    sleep(400);
    report.check("copied text appears in history", (search("orbitcliptest") == 1) as u32, 1);

    set_clipboard(Some("orbitsecrettest hunter2"), true);
    sleep(400);
    report.check("copies marked sensitive are not recorded", (search("orbitsecrettest") == 0) as u32, 1);

    set_clipboard(Some("orbitcliptest ordinary"), false);
    sleep(400);
    report.check("copying the same text again doesn't duplicate it", (search("orbitcliptest") == 1) as u32, 1);

    set_clipboard(Some("orbitpastetest newest"), false);
    sleep(400);
    sink.focus();
    sink.take_text();
    let mut pasted = false;
    if run_query(&session, chord, "clipboard history") {
        press(VK_RETURN.0);
        sleep(200);
        if session.focused() {
            press(VK_RETURN.0);
            pasted = wait_until(Duration::from_secs(1), || sink_has(sink, "orbitpastetest newest"));
        }
    }
    report.check("Enter on a history item pastes it into the previous window", pasted as u32, 1);
    sink.take_text();

    // A body must never reach a log.
    sleep(100);
    quit(&mut session);
    let mut leaked = false;
    for entry in ["timings.csv", "crash.log"] {
        if std::fs::read_to_string(dir.join(entry)).is_ok_and(|text| text.contains("orbitcliptest") || text.contains("hunter2")) {
            leaked = true;
        }
    }
    report.check("no clipboard text in Orbit's log files", (!leaked) as u32, 1);
    let secret_on_disk = ["clipboard.db", "clipboard.db-wal", "orbit.db", "orbit.db-wal"]
        .iter()
        .any(|file| std::fs::read(dir.join(file)).is_ok_and(|bytes| bytes.windows(7).any(|w| w == b"hunter2")));
    report.check("the sensitive copy isn't anywhere in Orbit's databases", (!secret_on_disk) as u32, 1);
    sink.focus();
}

fn hyper_settings(mode: orbit_hotkeys::HyperMode) -> Settings {
    let mut settings = base_settings();
    settings.hyper.enabled = true;
    settings.hyper.mode = mode;
    settings.hyper.bindings = parse_hyper("Space = cmd:launcher\nN = cmd:new-note").expect("bindings");
    settings
}

fn hold_caps_and(vk: u16) {
    send(&[key_input(VK_CAPS, false)]);
    sleep(30);
    send(&chord_inputs(&[vk]));
    sleep(30);
    send(&[key_input(VK_CAPS, true)]);
}

fn hyper_internal(report: &mut Report, options: &E2eOptions, root: &Path, sink: &Sink) {
    let (dir, log) = fresh_dir(root, "hyper", &hyper_settings(orbit_hotkeys::HyperMode::Internal));
    let caps_before = caps_on();
    let Some(mut session) = launch_session(options, &dir, &log, Some(HOTKEY_TEXT)) else {
        report.check("HyperKey: Orbit starts", 0, 1);
        return;
    };
    sleep(500);
    sink.focus();

    // Hyper+Space opens; Hyper+Space closes; the Caps toggle never changes.
    let mut opened = 0;
    let mut times = Vec::new();
    for _ in 0..20 {
        sink.focus();
        send(&[key_input(VK_CAPS, false)]);
        sleep(25);
        let t = qpc();
        send(&chord_inputs(&[VK_SPACE.0]));
        let ok = wait_until(Duration::from_secs(1), || session.visible() && session.focused());
        let t_done = qpc();
        send(&[key_input(VK_CAPS, true)]);
        if ok {
            opened += 1;
            times.push(session.ms(t, t_done));
        }
        sleep(50);
        session.hide();
        sleep(40);
    }
    report.check("Hyper+Space opens the launcher", opened, 20);
    report.add_timings("hyper: chord key injected to launcher focused", &times, true);

    let timings = std::fs::read_to_string(&log).unwrap_or_default();
    let _ = timings;
    session.message(WM_ORBIT_BENCH_FLUSH);
    let text = std::fs::read_to_string(&log).unwrap_or_default();
    let dispatch: Vec<f64> = text
        .lines()
        .filter(|line| line.starts_with("hyper,"))
        .filter_map(|line| {
            let mut parts = line.split(',').skip(1).filter_map(|p| p.parse::<i64>().ok());
            Some(session.ms(parts.next()?, parts.next()?))
        })
        .collect();
    report.add_timings("hyper: hook callback to UI thread", &dispatch, true);

    report.check("Caps Lock's toggle never changed while used as Hyper", (caps_on() == caps_before) as u32, 1);

    // A tap sends Escape, which closes the launcher.
    sink.focus();
    send(&chord_inputs(&HOTKEY_VKS));
    wait_until(Duration::from_secs(1), || session.visible() && session.focused());
    sleep(60);
    press(VK_CAPS);
    let closed = wait_until(Duration::from_millis(600), || !session.visible());
    report.check("tapping Caps Lock sends Escape", closed as u32, 1);
    report.check("the tap doesn't toggle Caps Lock", (caps_on() == caps_before) as u32, 1);

    // Unbound chords type nothing, and ordinary typing is untouched.
    sink.focus();
    sink.take_text();
    hold_caps_and('U' as u16);
    sleep(60);
    report.check("Hyper plus an unbound key types nothing", sink.take_text().is_empty() as u32, 1);
    sink.focus();
    type_keys("abc");
    sleep(60);
    report.check("ordinary typing passes through", (sink.take_text() == "abc") as u32, 1);

    // A key-up lost, as happens when an elevated window takes the keyboard mid-hold. The hold has
    // to end by itself, or every key would stay swallowed.
    sink.focus();
    sink.take_text();
    send(&[key_input(VK_CAPS, false)]);
    sleep(6500);
    type_keys("q");
    sleep(60);
    let recovered = sink.take_text() == "q";
    send(&[key_input(VK_CAPS, true)]);
    sleep(60);
    report.check("a hold whose key-up never came ends by itself", recovered as u32, 1);
    report.check("that recovery doesn't toggle Caps Lock", (caps_on() == caps_before) as u32, 1);

    // Autorepeat, release order and rapid chords, checking for stuck keys after each.
    let mut clean = 0;
    const ROUNDS: u32 = 60;
    for i in 0..ROUNDS {
        if !ours(session.pid) && !sink.focus() {
            break;
        }
        send(&[key_input(VK_CAPS, false)]);
        match i % 4 {
            0 => send(&chord_inputs(&['U' as u16])),
            1 => {
                send(&[key_input('U' as u16, false), key_input('U' as u16, false), key_input('U' as u16, false)]);
                send(&[key_input('U' as u16, true)]);
            }
            2 => {
                // Trigger released before the other key.
                send(&[key_input('U' as u16, false)]);
                send(&[key_input(VK_CAPS, true)]);
                send(&[key_input('U' as u16, false), key_input('U' as u16, true)]);
            }
            _ => {}
        }
        send(&[key_input(VK_CAPS, true)]);
        sleep(15);
        if modifiers_clear() && !held(VK_CAPS) && !held('U' as u16) {
            clean += 1;
        }
    }
    sleep(100);
    report.check("no key stuck after 60 awkward chords", clean, ROUNDS);
    report.check("those chords typed nothing", sink.take_text().is_empty() as u32, 1);
    if session.visible() {
        session.hide();
    }

    // Quitting hands the keyboard back at once.
    sink.focus();
    let clean_exit = quit(&mut session);
    report.check("HyperKey: clean exit", clean_exit as u32, 1);
    sleep(50);
    let before = caps_on();
    press(VK_CAPS);
    sleep(60);
    let toggled = caps_on() != before;
    press(VK_CAPS);
    sleep(60);
    report.check("after quitting, Caps Lock is a normal key again", (toggled && caps_on() == before) as u32, 1);
    sink.focus();

    // The same, if Orbit is killed outright with the key held.
    let (dir, log) = fresh_dir(root, "hyper2", &hyper_settings(orbit_hotkeys::HyperMode::Internal));
    if let Some(mut session) = launch_session(options, &dir, &log, Some(HOTKEY_TEXT)) {
        sleep(400);
        sink.focus();
        send(&[key_input(VK_CAPS, false)]);
        sleep(30);
        let _ = session.child.kill();
        let _ = session.child.wait();
        sleep(50);
        send(&[key_input(VK_CAPS, true)]);
        sleep(50);
        release_everything();
        let before = caps_on();
        press(VK_CAPS);
        sleep(80);
        let toggled = caps_on() != before;
        press(VK_CAPS);
        sleep(60);
        report.check("if Orbit is killed mid-hold, Caps Lock still works afterwards", (toggled && modifiers_clear()) as u32, 1);
    }
    sink.focus();
}

fn hyper_synthetic(report: &mut Report, options: &E2eOptions, root: &Path, sink: &Sink) {
    let (dir, log) = fresh_dir(root, "hyper3", &hyper_settings(orbit_hotkeys::HyperMode::Synthetic));
    let Some(mut session) = launch_session(options, &dir, &log, Some(HOTKEY_TEXT)) else {
        report.check("Synthetic HyperKey: Orbit starts", 0, 1);
        return;
    };
    sleep(500);
    let mut clean = 0;
    const ROUNDS: u32 = 10;
    for _ in 0..ROUNDS {
        sink.focus();
        // F20 is unbound everywhere that matters, so the forwarded chord does nothing visible.
        hold_caps_and(VK_F20.0);
        sleep(40);
        if modifiers_clear() && !held(VK_CAPS) {
            clean += 1;
        }
    }
    report.check("Synthetic mode leaves no modifier held", clean, ROUNDS);
    sink.focus();
    send(&[key_input(VK_CAPS, false)]);
    sleep(30);
    send(&[key_input(VK_F20.0, false)]);
    sleep(30);
    let clean_exit = quit(&mut session);
    send(&[key_input(VK_F20.0, true), key_input(VK_CAPS, true)]);
    sleep(80);
    let left_clear = modifiers_clear();
    release_everything();
    report.check("Synthetic mode: quitting mid-chord releases every modifier", (clean_exit && left_clear) as u32, 1);
    sink.focus();
}

fn settings_window(report: &mut Report, options: &E2eOptions, root: &Path, sink: &Sink, chord: &[INPUT]) {
    let mut settings = base_settings();
    settings.hyper.bindings = parse_hyper("C = cmd:clipboard").expect("bindings");
    let (dir, log) = fresh_dir(root, "settings", &settings);
    let Some(mut session) = launch_session(options, &dir, &log, Some(HOTKEY_TEXT)) else {
        report.check("settings: Orbit starts", 0, 1);
        return;
    };
    sleep(400);
    let pid = session.pid;
    let open = |session: &Session| -> bool {
        sink.focus();
        if !run_query(session, chord, "orbit settings") {
            return false;
        }
        press(VK_RETURN.0);
        wait_until(Duration::from_secs(2), || {
            find_window(pid, w!("OrbitSettings")).is_some_and(|hwnd| unsafe { GetForegroundWindow() } == hwnd)
        })
    };
    let opened = open(&session);
    report.check("Orbit Settings opens a focused window", opened as u32, 1);
    if opened {
        press(VK_ESCAPE.0);
        let closed = wait_until(Duration::from_secs(1), || find_window(pid, w!("OrbitSettings")).is_none());
        report.check("Esc closes settings", closed as u32, 1);
    }
    let before = std::fs::read_to_string(dir.join("settings.json")).unwrap_or_default();
    if open(&session) {
        // Enter doesn't close the window. Esc does, and with nothing changed that must change nothing that matters.
        press(VK_RETURN.0);
        sleep(150);
        report.check("Enter leaves Settings open", find_window(pid, w!("OrbitSettings")).is_some() as u32, 1);
        press(VK_ESCAPE.0);
        let closed = wait_until(Duration::from_secs(2), || find_window(pid, w!("OrbitSettings")).is_none());
        sleep(300);
        let after = std::fs::read_to_string(dir.join("settings.json")).unwrap_or_default();
        let (a, _) = Settings::parse(&before);
        let (b, _) = Settings::parse(&after);
        report.check("saving settings unchanged keeps them identical", (closed && a == b) as u32, 1);
        if a != b {
            report.not_tested.push(format!("settings difference after save:\nbefore: {before}\nafter: {after}"));
        }
    }
    // The Hotkeys page, two down from General: Edit as text, the search box, then the list. Delete
    // asks, then removes a row.
    if open(&session) {
        for _ in 0..2 {
            press(VK_DOWN.0);
            sleep(60);
        }
        for _ in 0..3 {
            press(VK_TAB.0);
            sleep(40);
        }
        press(VK_DELETE.0);
        sleep(100);
        report.check("the first Delete on the Hotkeys page only asks", (!saved_settings(&dir).hyper.bindings.is_empty()) as u32, 1);
        press(VK_DELETE.0);
        let saved = wait_until(Duration::from_secs(2), || saved_settings(&dir).hyper.bindings.is_empty());
        let still_open = find_window(pid, w!("OrbitSettings")).is_some();
        report.check("Delete on the Hotkeys page removes that hotkey, and saves it with Settings still open", (saved && still_open) as u32, 1);
        press(VK_ESCAPE.0);
        let closed = wait_until(Duration::from_secs(2), || find_window(pid, w!("OrbitSettings")).is_none());
        report.check("Esc then closes Settings", closed as u32, 1);
    }
    report.add("settings: private memory after closing the window", memory_of(process_handle(&session.child)).private_kb, "KB", false);
    quit(&mut session);
    sink.focus();
    app_picker(report, options, root, sink, chord);
}

/// Opens the launcher and waits until it has the keyboard.
fn open_launcher(session: &Session, sink: &Sink, chord: &[INPUT]) -> bool {
    if !may_inject(session, sink) {
        return false;
    }
    send(chord);
    let opened = wait_until(Duration::from_secs(1), || session.focused());
    sleep(40);
    opened
}

/// Closes whatever note or snippet window a test opened, and goes back to the sink.
fn close_editors(session: &Session) {
    for class in [w!("OrbitNote"), w!("OrbitSnippet")] {
        if let Some(window) = find_window(session.pid, class) {
            unsafe {
                let _ = PostMessageW(Some(window), WM_CLOSE, WPARAM(0), LPARAM(0));
            }
            wait_until(Duration::from_secs(1), || find_window(session.pid, class).is_none());
        }
    }
}

/// Alt plus a letter, held and let go, as a person would press it.
fn alt_and(key: u16) {
    send(&chord_inputs(&[VK_MENU.0, key]));
    sleep(60);
}

fn note_opened(session: &Session) -> bool {
    wait_until(Duration::from_secs(2), || find_window(session.pid, w!("OrbitNote")).is_some())
}

/// The hints in the launcher's footer are buttons. Clicks go straight to the window, so the real
/// pointer doesn't move. Positions are from the launcher's bottom right, inside the window's shadow,
/// in the footer's order at 100%.
fn footer_buttons(report: &mut Report, options: &E2eOptions, root: &Path, sink: &Sink, chord: &[INPUT]) {
    const TEST: &str = "footer buttons";
    let (dir, log) = fresh_dir(root, "footer", &base_settings());
    let Some(mut session) = launch_session(options, &dir, &log, Some(HOTKEY_TEXT)) else {
        report.check("footer buttons: Orbit starts", 0, 1);
        return;
    };
    sleep(400);
    let click = |from_right: i32| unsafe {
        let dpi = windows::Win32::UI::HiDpi::GetDpiForWindow(session.main) as i32;
        let mut client = RECT::default();
        let _ = GetClientRect(session.main, &mut client);
        let margin = 36 * dpi / 96;
        let x = client.right - margin - from_right * dpi / 96;
        let y = client.bottom - margin - 28 * dpi / 96;
        let point = LPARAM(((y as isize) << 16) | x as isize);
        let _ = PostMessageW(Some(session.main), WM_LBUTTONDOWN, WPARAM(1), point);
        let _ = PostMessageW(Some(session.main), WM_LBUTTONUP, WPARAM(0), point);
        sleep(150);
    };
    if !open_launcher(&session, sink, chord) {
        stopped(report, TEST);
        quit(&mut session);
        return;
    }
    // The shadow around the launcher is part of the window, and it looks like outside.
    let point = LPARAM((5isize << 16) | 5);
    unsafe {
        let _ = PostMessageW(Some(session.main), WM_LBUTTONDOWN, WPARAM(1), point);
        let _ = PostMessageW(Some(session.main), WM_LBUTTONUP, WPARAM(0), point);
    }
    // It fades out, so give it that long.
    let closed = wait_until(Duration::from_millis(600), || !session.visible());
    report.check("clicking the shadow around the launcher closes it", closed as u32, 1);
    if !run_query(&session, chord, "new note") {
        stopped(report, TEST);
        quit(&mut session);
        return;
    }
    // Ctrl+K is the second button from the right. With the menu open it's the last, and closes it.
    click(150);
    let menu_open = session.visible() && long_text(session.edit).is_empty();
    click(30);
    let went_back = session.visible() && long_text(session.edit) == "new note";
    report.check("clicking Ctrl+K Actions opens the small menu, and clicking it again closes it", (menu_open && went_back) as u32, 1);
    // A click on the list behind the menu closes it, and doesn't run what's under the pointer.
    click(150);
    let point = unsafe {
        let dpi = windows::Win32::UI::HiDpi::GetDpiForWindow(session.main) as isize;
        let margin = 36 * dpi / 96;
        LPARAM(((margin + 90 * dpi / 96) << 16) | (margin + 60 * dpi / 96))
    };
    unsafe {
        let _ = PostMessageW(Some(session.main), WM_LBUTTONDOWN, WPARAM(1), point);
        let _ = PostMessageW(Some(session.main), WM_LBUTTONUP, WPARAM(0), point);
    }
    sleep(150);
    report.check("clicking outside the small menu only closes it", (session.visible() && long_text(session.edit) == "new note") as u32, 1);
    // Esc on the keyboard: what's typed goes first, and the window follows the results.
    if run_query(&session, chord, "new note") {
        let size = || unsafe {
            let mut rect = RECT::default();
            let _ = GetWindowRect(session.main, &mut rect);
            rect
        };
        let short = size();
        press(VK_ESCAPE.0);
        sleep(150);
        let tall = size();
        report.check("Esc clears what's typed and keeps the launcher open", (session.visible() && long_text(session.edit).is_empty()) as u32, 1);
        report.check("the launcher is shorter for few results, and its top stays put", (short.bottom - short.top < tall.bottom - tall.top && short.top == tall.top) as u32, 1);
        press(VK_ESCAPE.0);
        let closed = wait_until(Duration::from_secs(1), || !session.visible());
        report.check("Esc with nothing typed closes the launcher", closed as u32, 1);
    }
    if !run_query(&session, chord, "new note") {
        stopped(report, TEST);
        quit(&mut session);
        return;
    }
    click(150);
    click(30);
    // What's typed goes first.
    click(30);
    let cleared = session.visible() && long_text(session.edit).is_empty();
    report.check("clicking Esc Clear empties the search", cleared as u32, 1);
    press(VK_ESCAPE.0);
    let _ = wait_until(Duration::from_secs(1), || !session.visible());
    if !run_query(&session, chord, "new note") {
        stopped(report, TEST);
        quit(&mut session);
        return;
    }
    let keys = long_text(session.edit);
    click(265);
    let opened = note_opened(&session);
    report.check("clicking Enter in the footer runs the selected result", opened as u32, 1);
    if !opened {
        report.not_tested.push(format!("footer Enter button: query was {keys:?}"));
    }
    close_editors(&session);
    hide_launcher(&session);
    quit(&mut session);
    sink.focus();
}

/// The pill with Orbit's logo opens Orbit's menu, and its first row is Settings.
fn orbit_menu(report: &mut Report, options: &E2eOptions, root: &Path, sink: &Sink, chord: &[INPUT]) {
    let (dir, log) = fresh_dir(root, "orbit-menu", &base_settings());
    let Some(mut session) = launch_session(options, &dir, &log, Some(HOTKEY_TEXT)) else {
        report.check("Orbit menu: Orbit starts", 0, 1);
        return;
    };
    sleep(400);
    if !open_launcher(&session, sink, chord) {
        stopped(report, "Orbit menu");
        quit(&mut session);
        return;
    }
    unsafe {
        let dpi = windows::Win32::UI::HiDpi::GetDpiForWindow(session.main) as isize;
        let mut client = RECT::default();
        let _ = GetClientRect(session.main, &mut client);
        let margin = 36 * dpi / 96;
        let (x, y) = (margin + 30 * dpi / 96, client.bottom as isize - margin - 28 * dpi / 96);
        let point = LPARAM((y << 16) | x);
        let _ = PostMessageW(Some(session.main), WM_LBUTTONDOWN, WPARAM(1), point);
        let _ = PostMessageW(Some(session.main), WM_LBUTTONUP, WPARAM(0), point);
    }
    sleep(200);
    press(VK_RETURN.0);
    let opened = wait_until(Duration::from_secs(3), || find_window(session.pid, w!("OrbitSettings")).is_some());
    report.check("clicking Orbit in the footer opens its menu, and Enter opens Settings from it", opened as u32, 1);
    if let Some(window) = find_window(session.pid, w!("OrbitSettings")) {
        unsafe {
            let _ = PostMessageW(Some(window), WM_CLOSE, WPARAM(0), LPARAM(0));
        }
        let _ = wait_until(Duration::from_secs(1), || find_window(session.pid, w!("OrbitSettings")).is_none());
    }
    quit(&mut session);
    sink.focus();
}

/// The emoji list: finding one and pasting it where you were, and remembering it afterwards.
fn emoji_list(report: &mut Report, options: &E2eOptions, root: &Path, sink: &Sink, chord: &[INPUT]) {
    let (dir, log) = fresh_dir(root, "emoji", &base_settings());
    let Some(mut session) = launch_session(options, &dir, &log, Some(HOTKEY_TEXT)) else {
        report.check("emoji list: Orbit starts", 0, 1);
        return;
    };
    sleep(400);
    sink.focus();
    sleep(100);
    if !run_query(&session, chord, "search emoji") {
        stopped(report, "emoji list");
        quit(&mut session);
        return;
    }
    press(VK_RETURN.0);
    sleep(200);
    let in_list = session.focused() && long_text(session.edit).is_empty();
    report.check("searching for it opens the emoji list", in_list as u32, 1);
    type_text("rocket");
    sleep(150);
    press(VK_RETURN.0);
    let pasted = wait_until(Duration::from_secs(3), || sink_has(sink, "🚀"));
    report.check("Enter pastes the emoji where you were typing", pasted as u32, 1);
    let saved = wait_until(Duration::from_secs(2), || {
        std::fs::read_to_string(dir.join("settings.json")).is_ok_and(|text| Settings::parse(&text).0.emoji.recent.first().is_some_and(|glyph| glyph == "🚀"))
    });
    report.check("a pasted emoji is remembered under Recently Used", saved as u32, 1);
    quit(&mut session);
    sink.focus();
}

/// The launcher's hotkey goes to programs named in Settings, and comes back when they're not in front.
fn launcher_stands_aside(report: &mut Report, options: &E2eOptions, root: &Path, sink: &Sink, chord: &[INPUT]) {
    let mut settings = base_settings();
    let bench = std::env::current_exe().ok().and_then(|path| path.file_name().map(|name| name.to_string_lossy().into_owned())).unwrap_or_default();
    settings.launcher.excluded_apps = vec![bench];
    let (dir, log) = fresh_dir(root, "stands-aside", &settings);
    let Some(mut session) = launch_session(options, &dir, &log, Some(HOTKEY_TEXT)) else {
        report.check("hotkey exclusion: Orbit starts", 0, 1);
        return;
    };
    sleep(400);
    sink.focus();
    sleep(300);
    send(chord);
    sleep(500);
    report.check("the launcher's hotkey leaves an excluded app alone", (!session.visible()) as u32, 1);
    // A second start opens the launcher, whatever the hotkey is doing.
    let _ = Command::new(&options.orbit).arg("--data-dir").arg(&dir).status();
    let shown = wait_until(Duration::from_secs(2), || session.visible());
    sleep(300);
    send(chord);
    let closed = wait_until(Duration::from_secs(1), || !session.visible());
    report.check("it works again as soon as something else is in front", (shown && closed) as u32, 1);
    quit(&mut session);
    sink.focus();
}

/// Two commands for the Vim checks, each counting its runs. Searching `qzvim` lists them in this
/// order on any PC, whatever's installed or running and however often each has run: the first's
/// name is the whole query, which no amount of use can outrank.
struct VimTargets {
    first: PathBuf,
    second: PathBuf,
}

const VIM_QUERY: &str = "qzvim";

impl VimTargets {
    fn add(options: &E2eOptions, root: &Path, name: &str, settings: &mut Settings) -> VimTargets {
        let probe = probe_path(options);
        let first = root.join(format!("{name}-first.txt"));
        let second = root.join(format!("{name}-second.txt"));
        let _ = std::fs::remove_file(&first);
        let _ = std::fs::remove_file(&second);
        settings.custom_commands.push(probe_command("bench-vim-first", "Qzvim", &probe, &first));
        settings.custom_commands.push(probe_command("bench-vim-second", "Qzvim Second", &probe, &second));
        VimTargets { first, second }
    }

    fn counts(&self) -> (usize, usize) {
        (runs_recorded(&self.first), runs_recorded(&self.second))
    }

    /// Waits for something to run, then says whether it was the first result (or the second), once.
    fn ran(&self, before: (usize, usize), second: bool) -> bool {
        poll(Duration::from_secs(3), || self.counts() != before);
        sleep(300);
        let expected = if second { (before.0, before.1 + 1) } else { (before.0 + 1, before.1) };
        self.counts() == expected
    }
}

/// Holding Alt to navigate, with typing otherwise as usual.
fn vim_keys(report: &mut Report, options: &E2eOptions, root: &Path, sink: &Sink, chord: &[INPUT]) {
    use orbit_core::vim::KeyboardMode;
    const TEST: &str = "Vim keys with Alt";
    const VK_J: u16 = 0x4A;
    const VK_L: u16 = 0x4C;
    const VK_UPPER_K: u16 = 0x4B;
    const VK_H: u16 = 0x48;
    if !probe_path(options).exists() {
        report.not_tested.push(format!("{TEST}: the probe program is missing"));
        return;
    }
    for (mode, name) in [(KeyboardMode::VimKeys, "vim-keys"), (KeyboardMode::Standard, "standard-keys")] {
        let mut settings = base_settings();
        settings.launcher.keyboard = mode;
        let targets = VimTargets::add(options, root, name, &mut settings);
        let (dir, log) = fresh_dir(root, name, &settings);
        let Some(mut session) = launch_session(options, &dir, &log, Some(HOTKEY_TEXT)) else {
            report.check("Vim keys: Orbit starts", 0, 1);
            return;
        };
        sleep(400);
        sink.focus();
        if !run_query(&session, chord, VIM_QUERY) {
            stopped(report, TEST);
            quit(&mut session);
            return;
        }
        let before = targets.counts();
        alt_and(VK_J);
        press(VK_RETURN.0);
        if mode == KeyboardMode::Standard {
            report.check("with standard keys, Alt+J doesn't move the selection", targets.ran(before, false) as u32, 1);
            hide_launcher(&session);
            quit(&mut session);
            sink.focus();
            continue;
        }
        report.check("Alt+J moves down", targets.ran(before, true) as u32, 1);
        hide_launcher(&session);
        sink.focus();
        if !run_query(&session, chord, VIM_QUERY) {
            stopped(report, TEST);
            quit(&mut session);
            return;
        }
        let before = targets.counts();
        alt_and(VK_J);
        alt_and(VK_UPPER_K);
        alt_and(VK_L);
        report.check("Alt+K moves up, and Alt+L runs the result", targets.ran(before, false) as u32, 1);
        hide_launcher(&session);
        // Plain letters still type, and letting go of Alt leaves the keyboard in the box.
        if !open_launcher(&session, sink, chord) {
            stopped(report, TEST);
            quit(&mut session);
            return;
        }
        alt_and(VK_J);
        type_text("jk");
        sleep(80);
        report.check("after Alt+J, letters still type into the box", (long_text(session.edit) == "jk") as u32, 1);
        alt_and(VK_H);
        report.check("Alt+H with nothing to go back to leaves Orbit open", session.visible() as u32, 1);
        hide_launcher(&session);
        quit(&mut session);
        sink.focus();
    }
}

/// Opens ready to navigate. Characters are commands until `i`.
fn full_vim(report: &mut Report, options: &E2eOptions, root: &Path, sink: &Sink, chord: &[INPUT]) {
    use orbit_core::vim::KeyboardMode;
    const TEST: &str = "full Vim";
    if !probe_path(options).exists() {
        report.not_tested.push(format!("{TEST}: the probe program is missing"));
        return;
    }
    let mut settings = base_settings();
    settings.launcher.keyboard = KeyboardMode::Vim;
    let targets = VimTargets::add(options, root, "full-vim", &mut settings);
    let (dir, log) = fresh_dir(root, "full-vim", &settings);
    let Some(mut session) = launch_session(options, &dir, &log, Some(HOTKEY_TEXT)) else {
        report.check("full Vim: Orbit starts", 0, 1);
        return;
    };
    sleep(400);
    let text = |session: &Session| {
        sleep(80);
        long_text(session.edit)
    };
    macro_rules! open_or_stop {
        () => {
            if !open_launcher(&session, sink, chord) {
                stopped(report, TEST);
                quit(&mut session);
                return;
            }
        };
    }

    // Opens navigating, with nothing typed by letters.
    open_or_stop!();
    type_text("jkhzz");
    report.check("Vim: letters don't type when it opens", text(&session).is_empty() as u32, 1);
    report.check("Vim: h with nothing to go back to leaves Orbit open", session.visible() as u32, 1);

    // i to type, Esc to navigate, and Esc again to close.
    type_text("i");
    type_text(VIM_QUERY);
    report.check("Vim: i starts typing", (text(&session) == VIM_QUERY) as u32, 1);
    press(VK_ESCAPE.0);
    sleep(80);
    report.check("Vim: Esc while typing keeps Orbit open", session.visible() as u32, 1);
    type_text("zz");
    report.check("Vim: after Esc, letters don't type", (text(&session) == VIM_QUERY) as u32, 1);

    // j moves, l runs.
    let before = targets.counts();
    type_text("jl");
    report.check("Vim: j moves down and l runs the result", targets.ran(before, true) as u32, 1);
    hide_launcher(&session);

    // Enter while typing runs the result.
    open_or_stop!();
    type_text("i");
    type_text(VIM_QUERY);
    sleep(150);
    let before = targets.counts();
    press(VK_RETURN.0);
    report.check("Vim: Enter while typing runs the result", targets.ran(before, false) as u32, 1);
    hide_launcher(&session);

    // It always opens navigating, even after typing last time.
    open_or_stop!();
    type_text("z");
    report.check("Vim: opens navigating again next time", text(&session).is_empty() as u32, 1);
    press(VK_ESCAPE.0);
    let closed = wait_until(Duration::from_secs(1), || !session.visible());
    report.check("Vim: Esc while navigating closes Orbit", closed as u32, 1);

    // q closes too.
    open_or_stop!();
    type_text("q");
    let closed = wait_until(Duration::from_secs(1), || !session.visible());
    report.check("Vim: q closes Orbit", closed as u32, 1);

    // Editing the box.
    open_or_stop!();
    type_text("iabc def");
    press(VK_ESCAPE.0);
    sleep(60);
    type_text("x");
    report.check("Vim: x deletes under the caret", (text(&session) == "abc de") as u32, 1);
    type_text("0dw");
    report.check("Vim: 0 and dw delete the first word", (text(&session) == "de") as u32, 1);
    type_text("u");
    report.check("Vim: u undoes", (text(&session) == "abc de") as u32, 1);
    type_text("dd");
    report.check("Vim: dd clears the box", text(&session).is_empty() as u32, 1);
    type_text("ccxy");
    report.check("Vim: cc clears and starts typing", (text(&session) == "xy") as u32, 1);
    press(VK_ESCAPE.0);
    sleep(60);
    type_text("/n");
    report.check("Vim: / starts a new search", (text(&session) == "n") as u32, 1);
    press(VK_ESCAPE.0);
    sleep(60);
    type_text("A!");
    report.check("Vim: A types at the end", (text(&session) == "n!") as u32, 1);
    press(VK_ESCAPE.0);
    sleep(60);
    type_text("I?");
    report.check("Vim: I types at the start", (text(&session) == "?n!") as u32, 1);
    press(VK_ESCAPE.0);
    sleep(60);
    type_text(":");
    report.check("Vim: : starts a commands search", (text(&session) == ":") as u32, 1);
    press(VK_ESCAPE.0);
    sleep(60);

    // Menus start out navigating, and h comes back to the same search.
    type_text("dd");
    type_text("i");
    type_text(VIM_QUERY);
    sleep(150);
    press(VK_TAB.0);
    sleep(150);
    type_text("zz");
    let menu_navigating = text(&session).is_empty();
    type_text("h");
    report.check("Vim: the action menu opens navigating, and h goes back to the search", (menu_navigating && text(&session) == VIM_QUERY && session.visible()) as u32, 1);
    press(VK_ESCAPE.0);
    sleep(80);
    report.check("Vim: Esc while navigating clears the search first", (session.visible() && text(&session).is_empty()) as u32, 1);
    press(VK_ESCAPE.0);
    wait_until(Duration::from_secs(1), || !session.visible());

    // Alt keys work while typing.
    open_or_stop!();
    type_text("i");
    type_text(VIM_QUERY);
    sleep(150);
    let before = targets.counts();
    alt_and(0x4A);
    press(VK_RETURN.0);
    report.check("Vim: Alt+J moves down while typing", targets.ran(before, true) as u32, 1);
    hide_launcher(&session);
    quit(&mut session);
    sink.focus();
}

/// The list of apps on the HyperKey page: a search box over apps, with no typing of program names.
fn app_picker(report: &mut Report, options: &E2eOptions, root: &Path, sink: &Sink, chord: &[INPUT]) {
    let mut settings = base_settings();
    settings.hyper.excluded_apps.clear();
    // The list is dimmed and out of the Tab order with HyperKey off. F20 stays out of the keyboard's way.
    settings.hyper.enabled = true;
    settings.hyper.trigger = "F20".to_string();
    let (dir, log) = fresh_dir(root, "app-picker", &settings);
    let apps = serde_json::json!([{ "id": "bench-picker", "name": "Qzbench Picker", "exe": "c:\\qz\\qzpicker.exe", "aumid": "" }]);
    std::fs::write(dir.join("apps.json"), apps.to_string()).expect("apps");
    let Some(mut session) = launch_session(options, &dir, &log, Some(HOTKEY_TEXT)) else {
        report.check("app picker: Orbit starts", 0, 1);
        return;
    };
    sleep(400);
    let pid = session.pid;
    let settings_open = || find_window(pid, w!("OrbitSettings")).is_some();
    // Opens Settings on the HyperKey page, with focus in the search box.
    let to_search = |session: &Session| -> bool {
        sink.focus();
        if !run_query(session, chord, "orbit settings") {
            return false;
        }
        press(VK_RETURN.0);
        let opened = wait_until(Duration::from_secs(2), || {
            find_window(pid, w!("OrbitSettings")).is_some_and(|hwnd| unsafe { GetForegroundWindow() } == hwnd)
        });
        if !opened {
            return false;
        }
        // HyperKey is three pages down from General, then past its six settings to the search box.
        for _ in 0..3 {
            press(VK_DOWN.0);
            sleep(50);
        }
        for _ in 0..7 {
            press(VK_TAB.0);
            sleep(25);
        }
        true
    };
    let saved_and_closed = |dir: &Path| -> Option<Settings> {
        wait_until(Duration::from_secs(2), || find_window(pid, w!("OrbitSettings")).is_none()).then(|| {
            sleep(300);
            saved_settings(dir)
        })
    };
    if !to_search(&session) {
        stopped(report, "app picker");
        quit(&mut session);
        return;
    }
    // Esc clears the search and leaves the window open. Only then does it close the window.
    type_text("qz");
    sleep(100);
    press(VK_ESCAPE.0);
    sleep(150);
    report.check("app picker: Esc clears a search and keeps Settings open", settings_open() as u32, 1);
    // Search, then Enter on the match, which saves, and Esc to close.
    type_text("qzbench");
    sleep(100);
    press(VK_RETURN.0);
    sleep(100);
    report.check("app picker: Enter on a match keeps Settings open", settings_open() as u32, 1);
    press(VK_ESCAPE.0);
    let added = saved_and_closed(&dir).is_some_and(|s| s.hyper.excluded_apps == ["qzpicker.exe"]);
    report.check("app picker: searching and pressing Enter adds that app's program", added as u32, 1);

    // A program that isn't listed can be named, and a chosen one can be removed.
    if to_search(&session) {
        type_text("zzmygame");
        sleep(100);
        press(VK_RETURN.0);
        sleep(100);
        press(VK_ESCAPE.0);
        let named = saved_and_closed(&dir).is_some_and(|s| s.hyper.excluded_apps == ["qzpicker.exe", "zzmygame.exe"]);
        report.check("app picker: a program that isn't listed can be added by name", named as u32, 1);
    }
    if to_search(&session) {
        press(VK_DOWN.0);
        sleep(60);
        press(VK_DELETE.0);
        sleep(100);
        press(VK_UP.0);
        sleep(60);
        press(VK_ESCAPE.0);
        let removed = saved_and_closed(&dir).is_some_and(|s| s.hyper.excluded_apps == ["zzmygame.exe"]);
        report.check("app picker: Delete removes a chosen app", removed as u32, 1);
    }
    // The HyperKey page's first picker: ids 200 search box, 201 Browse, 202 list.
    let control = |id: i32| -> Option<HWND> {
        settings_control(find_window(pid, w!("OrbitSettings"))?, id)
    };
    // Clicks without moving the real pointer. `from_right` is from the right edge of the list.
    let click_list = |y: i32, from_right: Option<i32>| {
        let Some(list) = control(202) else { return };
        let mut client = RECT::default();
        unsafe {
            let _ = GetClientRect(list, &mut client);
            let x = from_right.map_or(client.right / 3, |gap| client.right - gap);
            let point = LPARAM(((y as isize) << 16) | x as isize);
            let _ = PostMessageW(Some(list), WM_LBUTTONDOWN, WPARAM(1), point);
            let _ = PostMessageW(Some(list), WM_LBUTTONUP, WPARAM(0), point);
        }
        sleep(150);
    };
    if to_search(&session) {
        type_text("qzbench");
        sleep(100);
        click_list(10, None);
        press(VK_ESCAPE.0);
        let clicked = saved_and_closed(&dir).is_some_and(|s| s.hyper.excluded_apps == ["zzmygame.exe", "qzpicker.exe"]);
        report.check("app picker: clicking a match adds it, and it saves", clicked as u32, 1);
    }
    // With nothing typed, a click on the name does nothing, and one on Remove removes only that app.
    if to_search(&session) {
        click_list(10, None);
        click_list(10, Some(20));
        press(VK_ESCAPE.0);
        let removed = saved_and_closed(&dir).is_some_and(|s| s.hyper.excluded_apps == ["qzpicker.exe"]);
        report.check("app picker: clicking Remove removes that app, and it saves", removed as u32, 1);
    }
    // Browse takes the program's name from the file chosen.
    if to_search(&session) {
        let exe = std::env::current_exe().unwrap_or_default();
        let window = find_window(pid, w!("OrbitSettings"));
        if let Some(button) = control(201) {
            unsafe {
                let _ = PostMessageW(Some(button), BM_CLICK, WPARAM(0), LPARAM(0));
            }
        }
        let dialog = wait_until(Duration::from_secs(5), || {
            let front = unsafe { GetForegroundWindow() };
            ours(pid) && Some(front) != window
        });
        if dialog {
            sleep(500);
            type_text(&exe.to_string_lossy());
            sleep(200);
            press(VK_RETURN.0);
            let back = wait_until(Duration::from_secs(3), || window == Some(unsafe { GetForegroundWindow() }));
            sleep(150);
            if back {
                press(VK_ESCAPE.0);
                let name = exe.file_name().map(|n| n.to_string_lossy().to_lowercase()).unwrap_or_default();
                let added = saved_and_closed(&dir).is_some_and(|s| s.hyper.excluded_apps.iter().any(|entry| *entry == name));
                report.check("app picker: Browse adds the program chosen", added as u32, 1);
            } else {
                report.check("app picker: Browse adds the program chosen", 0, 1);
            }
        } else {
            report.check("app picker: Browse opens a file dialog", 0, 1);
        }
    }
    quit(&mut session);
    sink.focus();
}

// --- Shared helpers for the tests below ---------------------------------------------------------

/// A window that isn't ours took focus, so nothing more may be injected.
fn stopped(report: &mut Report, test: &str) {
    report.not_tested.push(format!("{test}: another window took focus, so the test stopped"));
}

/// True if keys may go in now: Orbit or the sink is focused, or the sink could take focus.
fn may_inject(session: &Session, sink: &Sink) -> bool {
    ours(session.pid) || sink.focus()
}

/// Polls something slower than a window check, such as opening the database.
fn poll(limit: Duration, mut condition: impl FnMut() -> bool) -> bool {
    let start = Instant::now();
    while start.elapsed() < limit {
        if condition() {
            return true;
        }
        sleep(25);
    }
    condition()
}

fn probe_path(options: &E2eOptions) -> PathBuf {
    options.orbit.parent().map(Path::to_path_buf).unwrap_or_default().join("orbit-bench-probe.exe")
}

fn runs_recorded(counter: &Path) -> usize {
    std::fs::read_to_string(counter).map_or(0, |text| text.lines().count())
}

/// Settings where Hyper+J runs a command that adds a line to `counter` each time.
fn counting_hyper_settings(mode: orbit_hotkeys::HyperMode, probe: &Path, counter: &Path) -> Settings {
    let mut settings = hyper_settings(mode);
    settings.hyper.bindings = parse_hyper("J = custom:bench-count").expect("bindings");
    settings.custom_commands.push(CustomCommand {
        id: "bench-count".into(),
        name: "Bench Count".into(),
        program: probe.to_string_lossy().into_owned(),
        arguments: format!("\"{}\" --append", counter.display()),
        working_dir: String::new(),
        alias: String::new(),
    });
    settings
}

const VK_J: u16 = 'J' as u16;

/// Waits for one run of the command, then long enough for a second one to show if there was one.
/// Returns how many runs this chord caused.
fn runs_from_chord(counter: &Path, before: usize) -> usize {
    poll(Duration::from_secs(3), || runs_recorded(counter) > before);
    sleep(300);
    runs_recorded(counter) - before
}

// --- A bound Hyper chord runs once ----------------------------------------------------------------

fn hyper_runs_once(report: &mut Report, options: &E2eOptions, root: &Path, sink: &Sink) {
    const TEST: &str = "Hyper chord runs its action once";
    const ROUNDS: u32 = 5;
    let probe = probe_path(options);
    if !probe.exists() {
        report.not_tested.push(format!("{TEST}: {} does not exist", probe.display()));
        return;
    }
    let counter = root.join("hyper-once-count.txt");
    let _ = std::fs::remove_file(&counter);
    let settings = counting_hyper_settings(orbit_hotkeys::HyperMode::Internal, &probe, &counter);
    let (dir, log) = fresh_dir(root, "hyper-once", &settings);
    let Some(mut session) = launch_session(options, &dir, &log, Some(HOTKEY_TEXT)) else {
        report.check("Hyper chord: Orbit starts", 0, 1);
        return;
    };
    sleep(500);
    let repeat = |vk: u16, times: u32| {
        for _ in 0..times {
            send(&[key_input(vk, false)]);
            sleep(30);
        }
    };
    let (mut once, mut clean) = (0, 0);
    let mut seen = Vec::new();
    let mut finished = true;
    for round in 0..ROUNDS {
        if !may_inject(&session, sink) {
            finished = false;
            break;
        }
        sink.take_text();
        let before = runs_recorded(&counter);
        // A held key repeats: the trigger until the next key goes down, and then that key.
        repeat(VK_CAPS, 3);
        repeat(VK_J, 4);
        if round % 2 == 0 {
            send(&[key_input(VK_J, true)]);
            send(&[key_input(VK_CAPS, true)]);
        } else {
            // The trigger goes first, and the other key is still repeating.
            send(&[key_input(VK_CAPS, true)]);
            repeat(VK_J, 2);
            send(&[key_input(VK_J, true)]);
        }
        let runs = runs_from_chord(&counter, before);
        seen.push(runs);
        if runs == 1 {
            once += 1;
        }
        if modifiers_clear() && !held(VK_CAPS) && !held(VK_J) && sink.take_text().is_empty() {
            clean += 1;
        }
        release_everything();
    }
    if finished {
        report.check("a Hyper chord with autorepeat runs its bound command exactly once", once, ROUNDS);
        report.check("that chord types nothing and leaves no key held", clean, ROUNDS);
        if once != ROUNDS {
            report.not_tested.push(format!("{TEST}: runs per chord were {seen:?}"));
        }
    } else {
        stopped(report, TEST);
    }
    quit(&mut session);
    sink.focus();
}

// --- Right-hand modifiers during a chord ----------------------------------------------------------

/// A right-hand modifier as a keyboard sends it, with its scan code and extended flag.
fn right_modifier(vk: u16, up: bool) -> INPUT {
    let (scan, extended) = match vk {
        0xA1 => (0x36, false),
        0xA3 => (0x1D, true),
        _ => (0x38, true),
    };
    let mut flags = KEYBD_EVENT_FLAGS(0);
    if extended {
        flags |= KEYEVENTF_EXTENDEDKEY;
    }
    if up {
        flags |= KEYEVENTF_KEYUP;
    }
    INPUT {
        r#type: INPUT_KEYBOARD,
        Anonymous: INPUT_0 { ki: KEYBDINPUT { wVk: VIRTUAL_KEY(vk), wScan: scan, dwFlags: flags, time: 0, dwExtraInfo: 0 } },
    }
}

fn hyper_right_modifiers(report: &mut Report, options: &E2eOptions, root: &Path, sink: &Sink) {
    const TEST: &str = "right-hand modifiers during a Hyper chord";
    const MODIFIERS: [(u16, &str); 3] = [(0xA1, "right Shift"), (0xA3, "right Ctrl"), (0xA5, "right Alt")];
    let probe = probe_path(options);
    if !probe.exists() {
        report.not_tested.push(format!("{TEST}: {} does not exist", probe.display()));
        return;
    }
    for (mode, mode_name, name) in [
        (orbit_hotkeys::HyperMode::Internal, "Internal", "hyper-right"),
        (orbit_hotkeys::HyperMode::Synthetic, "Synthetic", "hyper-right2"),
    ] {
        let counter = root.join(format!("{name}-count.txt"));
        let _ = std::fs::remove_file(&counter);
        let settings = counting_hyper_settings(mode, &probe, &counter);
        let (dir, log) = fresh_dir(root, name, &settings);
        let Some(mut session) = launch_session(options, &dir, &log, Some(HOTKEY_TEXT)) else {
            report.check(format!("{mode_name} mode with right-hand modifiers: Orbit starts"), 0, 1);
            continue;
        };
        sleep(500);
        let (mut dispatched, mut clean, mut total) = (0, 0, 0);
        let mut failures = Vec::new();
        let mut finished = true;
        'rounds: for (vk, modifier) in MODIFIERS {
            // Held before the trigger, then pressed and let go inside the hold.
            for first in [true, false] {
                if !may_inject(&session, sink) {
                    finished = false;
                    break 'rounds;
                }
                total += 1;
                sink.take_text();
                let before = runs_recorded(&counter);
                if first {
                    send(&[right_modifier(vk, false)]);
                    sleep(25);
                }
                send(&[key_input(VK_CAPS, false)]);
                sleep(25);
                if !first {
                    send(&[right_modifier(vk, false)]);
                    sleep(25);
                }
                send(&chord_inputs(&[VK_J]));
                sleep(25);
                if first {
                    send(&[key_input(VK_CAPS, true)]);
                    sleep(25);
                    send(&[right_modifier(vk, true)]);
                } else {
                    send(&[right_modifier(vk, true)]);
                    sleep(25);
                    send(&[key_input(VK_CAPS, true)]);
                }
                let runs = runs_from_chord(&counter, before);
                let is_clean = modifiers_clear() && !held(VK_CAPS) && !held(VK_J) && sink.take_text().is_empty();
                if runs == 1 {
                    dispatched += 1;
                }
                if is_clean {
                    clean += 1;
                }
                if runs != 1 || !is_clean {
                    let order = if first { "held first" } else { "inside the hold" };
                    failures.push(format!("{modifier}, {order}: {runs} runs, clean {is_clean}"));
                }
                release_everything();
            }
        }
        // In Synthetic mode an unbound key is forwarded with the virtual modifiers held. Only those
        // get released, so a real right-hand modifier must survive that and then let go.
        let mut forwarded_clean = 0;
        let forwarded_total = MODIFIERS.len() as u32;
        if finished && mode == orbit_hotkeys::HyperMode::Synthetic {
            for (vk, modifier) in MODIFIERS {
                if !may_inject(&session, sink) {
                    finished = false;
                    break;
                }
                send(&[right_modifier(vk, false)]);
                sleep(25);
                hold_caps_and(VK_F20.0);
                sleep(40);
                let survived = held(vk);
                send(&[right_modifier(vk, true)]);
                sleep(40);
                let clear = modifiers_clear() && !held(VK_CAPS);
                if survived && clear {
                    forwarded_clean += 1;
                } else {
                    failures.push(format!("{modifier} with a forwarded chord: still held after it {survived}, clear after release {clear}"));
                }
                release_everything();
            }
        }
        if finished {
            report.check(format!("{mode_name} mode: a chord dispatches with a right-hand modifier held"), dispatched, total);
            report.check(format!("{mode_name} mode: a right-hand modifier during a chord doesn't stick"), clean, total);
            if mode == orbit_hotkeys::HyperMode::Synthetic {
                report.check("Synthetic mode: a forwarded chord leaves a real right-hand modifier to the user", forwarded_clean, forwarded_total);
            }
            if !failures.is_empty() {
                report.not_tested.push(format!("{TEST}, {mode_name} mode, what failed: {}", failures.join("; ")));
            }
        } else {
            stopped(report, TEST);
        }
        release_everything();
        quit(&mut session);
        sink.focus();
        if !finished {
            return;
        }
    }
}

// --- Synthetic mode killed mid-chord --------------------------------------------------------------

/// Orbit dies with its virtual modifiers down, which nothing else will ever release, so the restart
/// has to. `release_everything` always follows, so the keyboard ends up clean whatever the outcome,
/// and `Restore` does it again if anything here panics.
fn synthetic_hard_kill(report: &mut Report, options: &E2eOptions, root: &Path, sink: &Sink) {
    const TEST: &str = "Synthetic mode killed mid-chord";
    let settings = hyper_settings(orbit_hotkeys::HyperMode::Synthetic);
    let (dir, log) = fresh_dir(root, "hyper-kill", &settings);
    let Some(mut session) = launch_session(options, &dir, &log, Some(HOTKEY_TEXT)) else {
        report.check("Synthetic kill: Orbit starts", 0, 1);
        return;
    };
    sleep(500);
    if !may_inject(&session, sink) {
        stopped(report, TEST);
        quit(&mut session);
        return;
    }
    send(&[key_input(VK_CAPS, false)]);
    sleep(30);
    send(&[key_input(VK_F20.0, false)]);
    let pressed = wait_until(Duration::from_millis(500), || !modifiers_clear());

    let _ = session.child.kill();
    let _ = session.child.wait();
    sleep(30);
    let stuck_after_kill = !modifiers_clear();
    // These two are the test's own keys. The virtual modifiers stay down for the restart to find.
    send(&[key_input(VK_F20.0, true), key_input(VK_CAPS, true)]);

    let restarted = launch_session(options, &dir, &dir.join("timings-restart.csv"), Some(HOTKEY_TEXT));
    let cleared = restarted.is_some() && stuck_after_kill && wait_until(Duration::from_secs(2), modifiers_clear);
    release_everything();
    sleep(50);

    if !pressed || !stuck_after_kill {
        report.not_tested.push(format!(
            "{TEST}: modifiers held before the kill: {pressed}, after it: {stuck_after_kill}, so there was nothing for a restart to clear"
        ));
    } else if restarted.is_none() {
        report.check("Synthetic kill: Orbit restarts", 0, 1);
    } else {
        report.check("after a kill mid-chord in Synthetic mode, restarting Orbit clears the modifiers", cleared as u32, 1);
    }
    if let Some(mut session) = restarted {
        sleep(200);
        if may_inject(&session, sink) {
            hold_caps_and(VK_F20.0);
            sleep(60);
            report.check("after that restart, a Synthetic chord still lets go of everything", (modifiers_clear() && !held(VK_CAPS)) as u32, 1);
            release_everything();
        } else {
            stopped(report, TEST);
        }
        quit(&mut session);
    }
    release_everything();
    sink.focus();
}

// --- A command whose program is gone --------------------------------------------------------------

fn missing_executable(report: &mut Report, options: &E2eOptions, root: &Path, sink: &Sink, chord: &[INPUT]) {
    const TEST: &str = "command with a missing executable";
    let mut settings = base_settings();
    let (dir, log) = fresh_dir(root, "missing-exe", &settings);
    settings.custom_commands.push(CustomCommand {
        id: "bench-missing".into(),
        name: "Bench Missing Tool".into(),
        program: dir.join("no-such-folder").join("no-such-program.exe").to_string_lossy().into_owned(),
        arguments: String::new(),
        working_dir: String::new(),
        alias: String::new(),
    });
    std::fs::write(dir.join("settings.json"), settings.to_json()).expect("settings");
    let Some(mut session) = launch_session(options, &dir, &log, Some(HOTKEY_TEXT)) else {
        report.check("missing executable: Orbit starts", 0, 1);
        return;
    };
    let pid = session.pid;
    sleep(400);
    if !may_inject(&session, sink) || !run_query(&session, chord, "bench missing tool") {
        stopped(report, TEST);
        quit(&mut session);
        return;
    }
    press(VK_RETURN.0);
    // Enter hides the launcher only if it ran the command. Otherwise this would prove nothing.
    if !wait_until(Duration::from_secs(1), || !session.visible()) {
        report.not_tested.push(format!("{TEST}: Enter didn't run the command, so the launcher stayed open"));
        session.hide();
        quit(&mut session);
        return;
    }
    // Windows looks for the program on another thread, so a dialog could take a moment to show.
    let dialog = || find_window(pid, w!("#32770")).filter(|hwnd| unsafe { IsWindowVisible(*hwnd) }.as_bool());
    let appeared = poll(Duration::from_millis(1500), || dialog().is_some());
    let main_enabled = unsafe { IsWindowEnabled(session.main) }.as_bool();
    report.check("a missing executable opens no dialog that stays up", (!appeared && main_enabled) as u32, 1);
    if let Some(hwnd) = dialog() {
        // It would block the rest of the run, and the failure is already recorded.
        unsafe {
            let _ = PostMessageW(Some(hwnd), WM_CLOSE, WPARAM(0), LPARAM(0));
        }
        sleep(200);
    }
    let alive = session.child.try_wait().ok().flatten().is_none();
    report.check("a missing executable doesn't take Orbit down", alive as u32, 1);

    // The launcher still opens, takes typing, and closes.
    let mut responsive = false;
    if alive {
        if !may_inject(&session, sink) {
            stopped(report, TEST);
            quit(&mut session);
            return;
        }
        send(chord);
        if wait_until(Duration::from_secs(1), || session.focused() && session.visible()) {
            sleep(30);
            type_text("after");
            sleep(100);
            responsive = read_text(session.edit) == "after";
            session.hide();
        }
    }
    report.check("Orbit responds normally after a missing executable", responsive as u32, 1);
    session.message(WM_ORBIT_BENCH_FLUSH);
    let told = std::fs::read_to_string(&log).is_ok_and(|text| text.contains("action,launch-failed"));
    report.check("a missing executable is reported to the user", told as u32, 1);
    if alive {
        quit(&mut session);
    }
    sink.focus();
}

// --- What the launcher offers, and the commands that change everything ---------------------------

fn log_since_flush(session: &Session) -> String {
    session.message(WM_ORBIT_BENCH_FLUSH);
    std::fs::read_to_string(&session.log).unwrap_or_default()
}

/// The web row is never pressed here: these checks only read what Orbit says it offered.
fn launcher_rows(report: &mut Report, options: &E2eOptions, root: &Path, sink: &Sink, chord: &[INPUT]) {
    const TEST: &str = "launcher rows";
    let mut settings = base_settings();
    settings.links.push(orbit_core::settings::QuickLink {
        id: "bench-web".into(),
        open_with: String::new(),
        name: "Bench Web".into(),
        url: "https://example.invalid/?q={query}".into(),
        alias: String::new(),
    });
    let (dir, log) = fresh_dir(root, "rows", &settings);
    // An app whose program is this benchmark, which has a window open.
    let this_exe = std::env::current_exe().map(|p| p.to_string_lossy().to_lowercase()).unwrap_or_default();
    let apps = serde_json::json!([{ "id": "bench-running", "name": "Qzbench Running App", "exe": this_exe, "aumid": "" }]);
    std::fs::write(dir.join("apps.json"), apps.to_string()).expect("apps");
    {
        let store = Store::open(&dir, true).expect("store");
        store.create_note("Wrbench saved note").expect("note");
    }
    let Some(mut session) = launch_session(options, &dir, &log, Some(HOTKEY_TEXT)) else {
        report.check("launcher rows: Orbit starts", 0, 1);
        return;
    };
    sleep(400);
    if !may_inject(&session, sink) {
        stopped(report, TEST);
        quit(&mut session);
        return;
    }

    let count = |text: &str, line: &str| text.lines().filter(|l| l.starts_with(line)).count();
    let query = |session: &Session, q: &str| -> Option<String> {
        sink.focus();
        if !run_query(session, chord, q) {
            return None;
        }
        sleep(250);
        let text = log_since_flush(session);
        session.hide();
        sleep(60);
        Some(text)
    };

    let before = log_since_flush(&session);
    let web_before = count(&before, "action,web-row");
    let after_note = query(&session, "wrbench");
    let after_nothing = after_note.as_ref().and_then(|_| query(&session, "zzqxjunkword"));
    let after_app = after_nothing.as_ref().and_then(|_| query(&session, "qzbench"));
    match (after_note, after_nothing, after_app) {
        (Some(note), Some(nothing), Some(app)) => {
            report.check("a query that finds a saved note offers no web search", (count(&note, "action,web-row") == web_before) as u32, 1);
            report.check("a query that finds nothing offers a web search", (count(&nothing, "action,web-row") > count(&note, "action,web-row")) as u32, 1);
            report.check("an app with a window open is offered as Switch to", (count(&app, "action,switch-row") > 0) as u32, 1);
        }
        _ => stopped(report, TEST),
    }
    quit(&mut session);
    sink.focus();
}

fn undo_trash(report: &mut Report, options: &E2eOptions, root: &Path, sink: &Sink, chord: &[INPUT]) {
    const TEST: &str = "note trash and undo";
    let (dir, log) = fresh_dir(root, "undo", &base_settings());
    {
        let store = Store::open(&dir, true).expect("store");
        store.create_note("Utbench note to trash").expect("note");
    }
    let Some(mut session) = launch_session(options, &dir, &log, Some(HOTKEY_TEXT)) else {
        report.check("note undo: Orbit starts", 0, 1);
        return;
    };
    sleep(400);
    let found = || -> usize {
        Store::open(&dir, true).ok().and_then(|store| store.search_notes("utbench", 5).ok()).map_or(0, |notes| notes.len())
    };
    let ctrl_k = chord_inputs(&[VK_CONTROL.0, 'K' as u16]);
    let open_notes = |session: &Session| -> bool {
        sink.focus();
        if !run_query(session, chord, "search notes") {
            return false;
        }
        press(VK_RETURN.0);
        sleep(300);
        session.focused()
    };
    if !may_inject(&session, sink) || !open_notes(&session) {
        stopped(report, TEST);
        quit(&mut session);
        return;
    }
    // Past the "New note" row to the note. Its menu is Open, Copy contents, Set hotkey, Pin note,
    // Move to trash.
    press(VK_DOWN.0);
    sleep(40);
    send(&ctrl_k);
    sleep(120);
    press(VK_DOWN.0);
    press(VK_DOWN.0);
    press(VK_DOWN.0);
    press(VK_DOWN.0);
    sleep(40);
    press(VK_RETURN.0);
    sleep(400);
    let trashed = found() == 0;
    report.check("Move to trash takes a note out of search", trashed as u32, 1);
    // Esc steps back through the views before it hides the launcher.
    for _ in 0..3 {
        session.hide();
        sleep(80);
    }
    if trashed {
        if !may_inject(&session, sink) || !open_notes(&session) {
            stopped(report, TEST);
            quit(&mut session);
            return;
        }
        // The undo row comes first.
        press(VK_RETURN.0);
        sleep(400);
        report.check("the undo row at the top of Notes brings it back", (found() == 1) as u32, 1);
        session.hide();
    }
    quit(&mut session);
    sink.focus();
}

fn reset_shortcuts(report: &mut Report, options: &E2eOptions, root: &Path, sink: &Sink, chord: &[INPUT]) {
    const TEST: &str = "Reset All Shortcuts";
    let mut settings = base_settings();
    settings.launcher.hotkey = Some(orbit_hotkeys::Chord::parse("Ctrl+Alt+Shift+F14").expect("chord"));
    settings.shortcuts = orbit_core::bindings_text::parse_shortcuts("Ctrl+Alt+Shift+F15 = cmd:new-note").expect("shortcuts");
    settings.hyper.bindings = parse_hyper("J = cmd:new-note").expect("bindings");
    let (dir, log) = fresh_dir(root, "reset", &settings);
    let Some(mut session) = launch_session(options, &dir, &log, Some(HOTKEY_TEXT)) else {
        report.check("Reset All Shortcuts: Orbit starts", 0, 1);
        return;
    };
    sleep(400);
    let read = || Settings::parse(&std::fs::read_to_string(dir.join("settings.json")).unwrap_or_default()).0;
    if !may_inject(&session, sink) || !run_query(&session, chord, "reset all shortcuts") {
        stopped(report, TEST);
        quit(&mut session);
        return;
    }
    press(VK_RETURN.0);
    sleep(150);
    // Cancel is selected first, so a stray Enter changes nothing.
    press(VK_RETURN.0);
    sleep(200);
    let kept = !read().shortcuts.is_empty();
    report.check("Reset All Shortcuts: Enter on the first row cancels", kept as u32, 1);
    session.hide();
    sink.focus();
    if !run_query(&session, chord, "reset all shortcuts") {
        stopped(report, TEST);
        quit(&mut session);
        return;
    }
    press(VK_RETURN.0);
    sleep(150);
    press(VK_DOWN.0);
    sleep(40);
    press(VK_RETURN.0);
    let saved = poll(Duration::from_secs(2), || read().shortcuts.is_empty());
    let after = read();
    report.check(
        "Reset All Shortcuts clears the launcher choice, shortcuts and mappings",
        (saved && after.launcher.hotkey.is_none() && after.hyper.bindings.is_empty()) as u32,
        1,
    );
    session.hide();
    quit(&mut session);
    sink.focus();
}

/// The sign-in entry that starts Orbit, exactly as Windows holds it, or `None` without one.
fn sign_in_entry() -> Option<Vec<u16>> {
    use windows::Win32::System::Registry::{HKEY_CURRENT_USER, RRF_RT_REG_SZ, RegGetValueW};
    let mut buffer = vec![0u16; 2048];
    let mut size = (buffer.len() * 2) as u32;
    let status = unsafe {
        RegGetValueW(
            HKEY_CURRENT_USER,
            w!("Software\\Microsoft\\Windows\\CurrentVersion\\Run"),
            w!("Orbit"),
            RRF_RT_REG_SZ,
            None,
            Some(buffer.as_mut_ptr() as *mut _),
            Some(&mut size),
        )
    };
    status.is_ok().then(|| {
        buffer.truncate(size as usize / 2);
        buffer
    })
}

/// Writes a sign-in entry read by [`sign_in_entry`] back as it was.
fn put_back_sign_in_entry(entry: &[u16]) {
    use windows::Win32::System::Registry::{HKEY_CURRENT_USER, REG_SZ, RegSetKeyValueW};
    unsafe {
        let _ = RegSetKeyValueW(
            HKEY_CURRENT_USER,
            w!("Software\\Microsoft\\Windows\\CurrentVersion\\Run"),
            w!("Orbit"),
            REG_SZ.0,
            Some(entry.as_ptr() as *const _),
            (entry.len() * 2) as u32,
        );
    }
}

fn delete_local_data(report: &mut Report, options: &E2eOptions, root: &Path, sink: &Sink, chord: &[INPUT]) {
    const TEST: &str = "Delete All Orbit Data";
    // The copy under test only deletes what's its own. A sign-in entry that starts another copy,
    // such as the Orbit installed on this PC, isn't.
    let entry_before = sign_in_entry();
    let (dir, log) = fresh_dir(root, "delete", &base_settings());
    {
        let store = Store::open(&dir, true).expect("store");
        store.create_note("Dlbench note").expect("note");
    }
    let keep = dir.join("not-orbits-file.txt");
    std::fs::write(&keep, "keep").expect("file");
    let Some(mut session) = launch_session(options, &dir, &log, Some(HOTKEY_TEXT)) else {
        report.check("Delete All Orbit Data: Orbit starts", 0, 1);
        return;
    };
    sleep(400);
    if !may_inject(&session, sink) || !run_query(&session, chord, "delete all orbit data") {
        stopped(report, TEST);
        quit(&mut session);
        return;
    }
    press(VK_RETURN.0);
    sleep(150);
    // Cancel is selected first.
    press(VK_RETURN.0);
    sleep(300);
    let alive = session.child.try_wait().ok().flatten().is_none();
    report.check("Delete All Orbit Data: Enter on the first row cancels", (alive && dir.join("orbit.db").exists()) as u32, 1);
    if !alive {
        return;
    }
    session.hide();
    sink.focus();
    if !run_query(&session, chord, "delete all orbit data") {
        stopped(report, TEST);
        quit(&mut session);
        return;
    }
    press(VK_RETURN.0);
    sleep(150);
    press(VK_DOWN.0);
    sleep(40);
    press(VK_RETURN.0);
    let exited = poll(Duration::from_secs(5), || session.child.try_wait().ok().flatten().is_some());
    report.check("Delete All Orbit Data: Orbit quits afterwards", exited as u32, 1);
    match &entry_before {
        Some(before) => {
            let kept = sign_in_entry().as_ref() == Some(before);
            report.check("Delete All Orbit Data leaves another copy's sign-in entry alone", kept as u32, 1);
            if !kept {
                // Put back, so running the suite never changes what starts when this PC signs in.
                put_back_sign_in_entry(before);
            }
        }
        None => report.not_tested.push(format!("{TEST}: leaving another copy's sign-in entry alone, as no Orbit starts at sign-in here")),
    }
    if !exited {
        quit(&mut session);
        return;
    }
    let gone = ["orbit.db", "orbit.db-wal", "orbit.db-shm", "settings.json", "apps.json"].iter().all(|name| !dir.join(name).exists());
    report.check("Delete All Orbit Data removes the database, settings and app index", gone as u32, 1);
    report.check("Delete All Orbit Data leaves files that aren't Orbit's", keep.exists() as u32, 1);
    sink.focus();
}

fn leader_mode(report: &mut Report, options: &E2eOptions, root: &Path, sink: &Sink) {
    const TEST: &str = "HyperKey leader mode";
    let probe = probe_path(options);
    if !probe.exists() {
        report.not_tested.push(format!("{TEST}: {} does not exist", probe.display()));
        return;
    }
    let counter = root.join("leader-count.txt");
    let _ = std::fs::remove_file(&counter);
    let mut settings = counting_hyper_settings(orbit_hotkeys::HyperMode::Internal, &probe, &counter);
    settings.hyper.tap = orbit_hotkeys::TapBehavior::Leader;
    let (dir, log) = fresh_dir(root, "leader", &settings);
    let Some(mut session) = launch_session(options, &dir, &log, Some(HOTKEY_TEXT)) else {
        report.check("leader mode: Orbit starts", 0, 1);
        return;
    };
    sleep(500);
    let tap = || {
        send(&[key_input(VK_CAPS, false)]);
        sleep(30);
        send(&[key_input(VK_CAPS, true)]);
    };
    let j = || {
        send(&[key_input(VK_J, false)]);
        sleep(20);
        send(&[key_input(VK_J, true)]);
    };
    let caps_before = caps_on();
    if !may_inject(&session, sink) {
        stopped(report, TEST);
        quit(&mut session);
        return;
    }
    sink.take_text();
    let before = runs_recorded(&counter);
    tap();
    sleep(100);
    j();
    let runs = runs_from_chord(&counter, before);
    report.check("leader: a tap then a mapped key runs its command once", (runs == 1) as u32, 1);
    report.check("leader: that key isn't typed", sink.take_text().is_empty() as u32, 1);
    report.check("leader: the tap doesn't toggle Caps Lock", (caps_on() == caps_before) as u32, 1);

    if !may_inject(&session, sink) {
        stopped(report, TEST);
        quit(&mut session);
        return;
    }
    let before = runs_recorded(&counter);
    tap();
    sleep(u64::from(orbit_hotkeys::LEADER_WINDOW_MS) + 300);
    j();
    sleep(400);
    let typed = sink.take_text();
    let ran = runs_recorded(&counter) - before;
    let focused = sink.focus();
    let normal = ran == 0 && typed.to_lowercase() == "j";
    report.check("leader: the window ends, and the key types normally", normal as u32, 1);
    if !normal {
        report.not_tested.push(format!("{TEST}: after the window, {ran} runs, {} characters typed, sink could take focus: {focused}", typed.chars().count()));
    }
    release_everything();
    quit(&mut session);
    sink.focus();
}

// --- Notes: many opens, and a hard kill -----------------------------------------------------------

fn hundred_note_opens(report: &mut Report, options: &E2eOptions, root: &Path, sink: &Sink, chord: &[INPUT]) {
    const TEST: &str = "100 note opens";
    const OPENS: u32 = 100;
    let (dir, log) = fresh_dir(root, "note-opens", &base_settings());
    {
        let store = Store::open(&dir, true).expect("store");
        store.create_note("Zqbench loop note\nThe body of the note that gets opened again and again.").expect("note");
    }
    let Some(mut session) = launch_session(options, &dir, &log, Some(HOTKEY_TEXT)) else {
        report.check("100 note opens: Orbit starts", 0, 1);
        return;
    };
    let process = process_handle(&session.child);
    sleep(400);
    let before = memory_of(process);
    let pid = session.pid;
    let start = Instant::now();
    let mut completed = 0;
    let mut after_first = before;
    let mut focus_held = true;
    for _ in 0..OPENS {
        if !may_inject(&session, sink) || !run_query(&session, chord, "zqbench") {
            focus_held = false;
            break;
        }
        press(VK_RETURN.0);
        let opened = wait_until(Duration::from_secs(2), || {
            find_window(pid, w!("OrbitNote")).is_some_and(|hwnd| unsafe { GetForegroundWindow() } == hwnd)
        });
        if !opened {
            // A note on screen without focus means something else has it, so stop. With no note
            // at all, this open simply failed, which the count shows.
            if find_window(pid, w!("OrbitNote")).is_some() {
                focus_held = false;
                break;
            }
            if session.visible() && session.focused() {
                session.hide();
            }
            continue;
        }
        press(VK_ESCAPE.0);
        if !wait_until(Duration::from_secs(2), || find_window(pid, w!("OrbitNote")).is_none()) {
            break;
        }
        completed += 1;
        if completed == 1 {
            sleep(300);
            after_first = memory_of(process);
        }
    }
    // Long enough for the editor's memory to go back after the last close.
    sleep(1500);
    let after = memory_of(process);
    report.check("a note opens and closes 100 times in a row", completed, OPENS);
    if completed == OPENS {
        report.add("memory after 100 note opens: private", after.private_kb, "KB", true);
        report.add("memory after 100 note opens: working set", after.working_set_kb, "KB", true);
        report.add("memory before any note opens: private", before.private_kb, "KB", false);
        report.add("memory after the first note open: private", after_first.private_kb, "KB", false);
        report.add("100 note opens: private growth per open after the first", (after.private_kb - after_first.private_kb) / (OPENS - 1) as f64, "KB", false);
        report.add("100 note opens: duration", start.elapsed().as_secs_f64() * 1000.0, "ms", false);
    } else {
        report.add("memory after a partial note-open loop: private", after.private_kb, "KB", false);
        report.add("memory after a partial note-open loop: working set", after.working_set_kb, "KB", false);
        report.not_tested.push(format!("{TEST}: only {completed} of {OPENS} finished, so there's no figure for 100"));
    }
    if !focus_held {
        stopped(report, TEST);
    }
    quit(&mut session);
    sink.focus();
}

fn note_survives_kill(report: &mut Report, options: &E2eOptions, root: &Path, sink: &Sink, chord: &[INPUT]) {
    const TEST: &str = "note survives a hard kill";
    const TITLE: &str = "Zqkilled note";
    const REST: &str = "written just before the kill ünï 🎉";
    let (dir, log) = fresh_dir(root, "note-kill", &base_settings());
    let Some(mut session) = launch_session(options, &dir, &log, Some(HOTKEY_TEXT)) else {
        report.check("note kill: Orbit starts", 0, 1);
        return;
    };
    let pid = session.pid;
    sleep(400);
    if !may_inject(&session, sink) || !run_query(&session, chord, "new note") {
        stopped(report, TEST);
        quit(&mut session);
        return;
    }
    press(VK_RETURN.0);
    let is_note = || find_window(pid, w!("OrbitNote")).is_some_and(|hwnd| unsafe { GetForegroundWindow() } == hwnd);
    if !wait_until(Duration::from_secs(2), is_note) {
        report.check("note kill: the editor opens", 0, 1);
        quit(&mut session);
        return;
    }
    for line in [Some(TITLE), None, Some(REST)] {
        if !is_note() {
            stopped(report, TEST);
            quit(&mut session);
            return;
        }
        match line {
            Some(line) => type_text(line),
            None => press(VK_RETURN.0),
        }
        sleep(20);
    }
    // The autosave waits 400 ms after the last key, so this is past it but not by much.
    sleep(700);
    // `Child::kill` is TerminateProcess on Windows: no shutdown code runs and nothing gets flushed.
    let _ = session.child.kill();
    let _ = session.child.wait();

    let saved = Store::open(&dir, true).ok().and_then(|store| {
        let found = store.search_notes("zqkilled", 5).ok()?;
        store.note_body(found.first()?.id).ok().flatten()
    });
    let wanted = format!("{TITLE}\n{REST}");
    let intact = saved.as_deref().is_some_and(|body| body.replace("\r\n", "\n").replace('\r', "\n") == wanted);
    report.check("a note typed 700 ms before a hard kill is all there", intact as u32, 1);
    if !intact {
        report.not_tested.push(format!("{TEST}: the store held {saved:?}"));
    }

    // A quick reopen: the same data opens in a new Orbit, straight after the kill.
    let Some(mut session) = launch_session(options, &dir, &dir.join("timings-reopen.csv"), Some(HOTKEY_TEXT)) else {
        report.check("after a hard kill, Orbit starts again on the same data", 0, 1);
        return;
    };
    let pid = session.pid;
    sleep(300);
    if !may_inject(&session, sink) || !run_query(&session, chord, "zqkilled") {
        stopped(report, TEST);
    } else {
        press(VK_RETURN.0);
        let mut reopened = false;
        if wait_until(Duration::from_secs(2), || find_window(pid, w!("OrbitNote")).is_some_and(|hwnd| unsafe { GetForegroundWindow() } == hwnd)) {
            reopened = find_window(pid, w!("OrbitNote"))
                .and_then(|hwnd| unsafe { FindWindowExW(Some(hwnd), None, w!("RICHEDIT50W"), PCWSTR::null()) }.ok())
                .is_some_and(|edit| long_text(edit).contains("just before the kill"));
            press(VK_ESCAPE.0);
            wait_until(Duration::from_secs(1), || find_window(pid, w!("OrbitNote")).is_none());
        }
        report.check("after a hard kill, the note reopens in a new Orbit", reopened as u32, 1);
    }
    quit(&mut session);
    sink.focus();
}

// --- Clipboard: what's kept and what isn't --------------------------------------------------------

const CF_DIB: u32 = 8;
const CF_HDROP: u32 = 15;

/// Puts one format on the clipboard, owned by `owner`. Orbit names the source of a copy from the
/// owner, so this makes the source certain, whichever window has focus.
fn copy_as(owner: HWND, format: u32, bytes: &[u8]) -> bool {
    if !open_clipboard_as(Some(owner)) {
        return false;
    }
    unsafe {
        let mut ok = EmptyClipboard().is_ok();
        match GlobalAlloc(GMEM_MOVEABLE, bytes.len().max(1)) {
            Ok(global) => {
                let target = GlobalLock(global) as *mut u8;
                if target.is_null() {
                    ok = false;
                } else {
                    std::ptr::copy_nonoverlapping(bytes.as_ptr(), target, bytes.len());
                    let _ = GlobalUnlock(global);
                    ok &= SetClipboardData(format, Some(HANDLE(global.0))).is_ok();
                }
            }
            Err(_) => ok = false,
        }
        let _ = CloseClipboard();
        ok
    }
}

fn text_bytes(text: &str) -> Vec<u8> {
    wide(text).iter().flat_map(|unit| unit.to_le_bytes()).collect()
}

/// A DROPFILES header followed by wide paths, the list ending in an extra zero.
fn file_list_bytes(paths: &[&str]) -> Vec<u8> {
    let mut bytes = Vec::new();
    bytes.extend_from_slice(&20u32.to_le_bytes()); // offset of the list
    bytes.extend_from_slice(&[0; 12]); // drop point and non-client flag
    bytes.extend_from_slice(&1u32.to_le_bytes()); // wide characters
    for path in paths {
        bytes.extend(text_bytes(path));
    }
    bytes.extend_from_slice(&[0, 0]);
    bytes
}

/// A small 32-bit DIB: a BITMAPINFOHEADER, then the pixels.
fn dib_bytes(width: u32, height: u32) -> Vec<u8> {
    let pixels = width * height * 4;
    let mut bytes = Vec::with_capacity(40 + pixels as usize);
    bytes.extend_from_slice(&40u32.to_le_bytes());
    bytes.extend_from_slice(&width.to_le_bytes());
    bytes.extend_from_slice(&height.to_le_bytes());
    bytes.extend_from_slice(&1u16.to_le_bytes());
    bytes.extend_from_slice(&32u16.to_le_bytes());
    bytes.extend_from_slice(&0u32.to_le_bytes());
    bytes.extend_from_slice(&pixels.to_le_bytes());
    bytes.extend_from_slice(&[0; 16]);
    bytes.extend((0..pixels).map(|i| (i % 251) as u8));
    bytes
}

// These return `None` if the database can't be read, so that can't pass for "nothing recorded".
fn clips_in(dir: &Path) -> Option<Vec<ClipItem>> {
    Store::open(dir, true).ok()?.recent_clips(50).ok()
}

fn clip_matches(dir: &Path, text: &str) -> Option<usize> {
    Some(Store::open(dir, true).ok()?.search_clips(text, 5).ok()?.len())
}

fn in_databases(dir: &Path, marker: &[u8]) -> bool {
    ["clipboard.db", "clipboard.db-wal", "orbit.db", "orbit.db-wal"]
        .iter()
        .any(|file| std::fs::read(dir.join(file)).is_ok_and(|bytes| bytes.windows(marker.len()).any(|w| w == marker)))
}

/// Injects no keys, so this needs no focus. The copies come from the sink's window.
fn clipboard_rules(report: &mut Report, options: &E2eOptions, root: &Path, sink: &Sink) {
    const LIMIT: u32 = 4096;
    let Some(exe) = std::env::current_exe().ok().and_then(|path| path.file_name().map(|name| name.to_string_lossy().into_owned())) else {
        report.not_tested.push("clipboard rules: couldn't find the benchmark's own file name".to_string());
        return;
    };
    let owner = sink.hwnd();
    let recorded = |dir: &Path, text: &str| poll(Duration::from_secs(2), || clip_matches(dir, text) == Some(1));
    let padded = |marker: &str, bytes: usize| format!("{marker} {}", "x ".repeat(bytes / 2));

    // Images on, with a small size limit. Each copy that should be skipped is followed by one that
    // is kept: copies are handled in order, so once that one's there, the skipped one has had its
    // chance. It also shows the listener never stopped.
    let mut settings = base_settings();
    settings.clipboard.enabled = true;
    settings.clipboard.capture_images = true;
    settings.clipboard.max_item_bytes = LIMIT;
    let (dir, log) = fresh_dir(root, "clip-rules", &settings);
    let Some(mut session) = launch_session(options, &dir, &log, Some(HOTKEY_TEXT)) else {
        report.check("clipboard rules: Orbit starts", 0, 1);
        return;
    };
    sleep(400);
    copy_as(owner, CF_UNICODETEXT, &text_bytes(&padded("orbitundertest", 1000)));
    let under_kept = recorded(&dir, "orbitundertest");
    report.check("clipboard text under the size limit is recorded", under_kept as u32, 1);
    let named_correctly = clips_in(&dir).is_some_and(|clips| clips.iter().any(|clip| clip.source_app.eq_ignore_ascii_case(&exe)));
    report.check("a recorded copy names the app it came from", named_correctly as u32, 1);

    let over_copied = copy_as(owner, CF_UNICODETEXT, &text_bytes(&padded("orbitovertest", LIMIT as usize * 2)));
    sleep(300);
    copy_as(owner, CF_UNICODETEXT, &text_bytes("orbitaftertest kept"));
    let size_test_valid = over_copied && recorded(&dir, "orbitaftertest");
    if size_test_valid {
        report.check("clipboard text over max_item_bytes isn't recorded", (clip_matches(&dir, "orbitovertest") == Some(0)) as u32, 1);
    } else {
        report.not_tested.push("clipboard size limit: the oversized copy or the one after it didn't work, so a skip would prove nothing".to_string());
    }

    let file_text = dir.join("orbitdropfile.txt").to_string_lossy().into_owned();
    let other_text = dir.join("orbitdropother.txt").to_string_lossy().into_owned();
    copy_as(owner, CF_HDROP, &file_list_bytes(&[&file_text, &other_text]));
    let files_kept = poll(Duration::from_secs(2), || {
        let Ok(store) = Store::open(&dir, true) else { return false };
        let clips = store.recent_clips(50).unwrap_or_default();
        clips.iter().filter(|clip| clip.kind == ClipKind::Files).any(|clip| {
            store.clip_content(clip.id).ok().flatten().is_some_and(|(_, text, _)| text.contains(&file_text) && text.contains(&other_text))
        })
    });
    report.check("a copied file list is recorded with every path", files_kept as u32, 1);

    copy_as(owner, CF_DIB, &dib_bytes(16, 16));
    let image_kept = poll(Duration::from_secs(2), || {
        clips_in(&dir).is_some_and(|clips| clips.iter().any(|clip| clip.kind == ClipKind::Image && clip.width == 16 && clip.height == 16))
    });
    report.check("an image is recorded when capture_images is on", image_kept as u32, 1);
    let blob_saved = Store::open(&dir, true).ok().is_some_and(|store| {
        let clips = store.recent_clips(50).unwrap_or_default();
        clips.iter().find(|clip| clip.kind == ClipKind::Image).is_some_and(|clip| {
            store.clip_content(clip.id).ok().flatten().is_some_and(|(_, _, blob)| !blob.is_empty() && dir.join("clipboard-images").join(blob).exists())
        })
    });
    report.check("the image's pixels are saved", blob_saved as u32, 1);
    quit(&mut session);
    if size_test_valid {
        report.check("the oversized copy isn't anywhere in Orbit's databases", (!in_databases(&dir, b"orbitovertest")) as u32, 1);
    }

    // Images off.
    let mut settings = base_settings();
    settings.clipboard.enabled = true;
    settings.clipboard.capture_images = false;
    let (dir, log) = fresh_dir(root, "clip-noimage", &settings);
    if let Some(mut session) = launch_session(options, &dir, &log, Some(HOTKEY_TEXT)) {
        sleep(400);
        let image_copied = copy_as(owner, CF_DIB, &dib_bytes(16, 16));
        sleep(300);
        copy_as(owner, CF_UNICODETEXT, &text_bytes("orbitnoimagetest kept"));
        if image_copied && recorded(&dir, "orbitnoimagetest") {
            let skipped = clips_in(&dir).is_some_and(|clips| !clips.iter().any(|clip| clip.kind == ClipKind::Image));
            report.check("an image isn't recorded when capture_images is off", skipped as u32, 1);
        } else {
            report.not_tested.push("images off: the image copy or the one after it didn't work, so a skip would prove nothing".to_string());
        }
        quit(&mut session);
    } else {
        report.check("clipboard with images off: Orbit starts", 0, 1);
    }

    // This app excluded. The first session is the control: copies from the same window were recorded
    // under this name, and only the list differs. Everything is on, so each kind of copy is a chance
    // to get it wrong.
    let mut settings = base_settings();
    settings.clipboard.enabled = true;
    settings.clipboard.capture_images = true;
    settings.clipboard.excluded_apps.push(exe.clone());
    let (dir, log) = fresh_dir(root, "clip-excluded", &settings);
    if let Some(mut session) = launch_session(options, &dir, &log, Some(HOTKEY_TEXT)) {
        sleep(400);
        let mut copied = copy_as(owner, CF_UNICODETEXT, &text_bytes("orbitexcludedtest hunter3"));
        sleep(300);
        copied &= copy_as(owner, CF_HDROP, &file_list_bytes(&["C:\\orbitexcludedfile.txt"]));
        sleep(300);
        copied &= copy_as(owner, CF_DIB, &dib_bytes(16, 16));
        sleep(800);
        if copied && under_kept && named_correctly {
            let nothing = clips_in(&dir).is_some_and(|clips| clips.is_empty());
            report.check("an excluded app's copies are never recorded", nothing as u32, 1);
            quit(&mut session);
            let on_disk = in_databases(&dir, b"orbitexcludedtest") || in_databases(&dir, b"orbitexcludedfile");
            report.check("an excluded app's copy isn't anywhere in Orbit's databases", (!on_disk) as u32, 1);
        } else {
            report.not_tested.push(format!(
                "excluded apps: copies made: {copied}, and without the exclusion the control recorded: {under_kept}, under this app's name: {named_correctly}, so this would prove nothing"
            ));
            quit(&mut session);
        }
    } else {
        report.check("clipboard with an excluded app: Orbit starts", 0, 1);
    }
    sink.focus();
}

// --- What the optional features cost --------------------------------------------------------------

/// No keys go in here. The HyperKey hook is live for 3 s in the second profile, which only matters
/// if someone presses Caps Lock during it.
fn footprint_by_profile(report: &mut Report, options: &E2eOptions, root: &Path) {
    let mut all_on = hyper_settings(orbit_hotkeys::HyperMode::Internal);
    all_on.snippets.expansion_enabled = true;
    all_on.clipboard.enabled = true;
    let mut measured = Vec::new();
    for (label, name, settings) in
        [("optional features off", "footprint-off", base_settings()), ("HyperKey, expansion and clipboard on", "footprint-on", all_on)]
    {
        let (dir, log) = fresh_dir(root, name, &settings);
        let Some(mut session) = launch_session(options, &dir, &log, Some(HOTKEY_TEXT)) else {
            report.not_tested.push(format!("footprint with {label}: Orbit didn't start"));
            continue;
        };
        sleep(3000);
        let memory = memory_of(process_handle(&session.child));
        report.add(format!("footprint, {label}, 3 s idle: private"), memory.private_kb, "KB", true);
        report.add(format!("footprint, {label}, 3 s idle: working set"), memory.working_set_kb, "KB", true);
        if let Some(threads) = context_switches(session.pid).map(|counts| counts.len()) {
            report.add(format!("footprint, {label}, 3 s idle: threads"), threads as f64, "count", false);
        }
        measured.push(memory);
        quit(&mut session);
    }
    if let [off, on] = measured.as_slice() {
        report.add("footprint: what the optional features add, private", on.private_kb - off.private_kb, "KB", false);
        report.add("footprint: what the optional features add, working set", on.working_set_kb - off.working_set_kb, "KB", false);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_file_list_ends_in_two_zeros_after_its_header() {
        let bytes = file_list_bytes(&["a", "b"]);
        assert_eq!(u32::from_le_bytes(bytes[0..4].try_into().unwrap()), 20);
        assert_eq!(u32::from_le_bytes(bytes[16..20].try_into().unwrap()), 1);
        let units: Vec<u16> = bytes[20..].chunks(2).map(|c| u16::from_le_bytes([c[0], c[1]])).collect();
        assert_eq!(units, ['a' as u16, 0, 'b' as u16, 0, 0]);
    }

    #[test]
    fn a_dib_has_a_40_byte_header_and_every_pixel() {
        let bytes = dib_bytes(16, 8);
        assert_eq!(bytes.len(), 40 + 16 * 8 * 4);
        assert_eq!(u32::from_le_bytes(bytes[0..4].try_into().unwrap()), 40);
        assert_eq!(i32::from_le_bytes(bytes[4..8].try_into().unwrap()), 16);
        assert_eq!(i32::from_le_bytes(bytes[8..12].try_into().unwrap()), 8);
    }

    #[test]
    fn text_on_the_clipboard_is_terminated() {
        assert_eq!(text_bytes("hi"), [b'h', 0, b'i', 0, 0, 0]);
    }
}

// --- Setting a hotkey from Ctrl+K -----------------------------------------------------------------

const VK_K: u16 = 'K' as u16;
const VK_F10_KEY: u16 = 0x79;

fn saved_settings(dir: &Path) -> Settings {
    Settings::parse(&std::fs::read_to_string(dir.join("settings.json")).unwrap_or_default()).0
}

fn builtin(command: orbit_core::Builtin) -> orbit_core::Action {
    orbit_core::Action::Builtin { command }
}

/// Finds an item, opens its action menu and runs the action whose name is typed.
fn run_menu_action(session: &Session, sink: &Sink, chord: &[INPUT], query: &str, action: &str) -> bool {
    sink.focus();
    if !run_query(session, chord, query) {
        return false;
    }
    send(&chord_inputs(&[VK_CONTROL.0, VK_K]));
    sleep(120);
    type_text(action);
    sleep(150);
    press(VK_RETURN.0);
    sleep(150);
    session.focused()
}

fn hide_launcher(session: &Session) {
    for _ in 0..3 {
        if session.hide() {
            break;
        }
    }
}

fn hotkey_from_menu(report: &mut Report, options: &E2eOptions, root: &Path, sink: &Sink, chord: &[INPUT]) {
    use orbit_core::Builtin;
    use orbit_core::hotkeys::{Hotkey, action_of, hotkey_for};
    const TEST: &str = "setting a hotkey from Ctrl+K";
    let mut settings = hyper_settings(orbit_hotkeys::HyperMode::Internal);
    settings.hyper.bindings.clear();
    let (dir, log) = fresh_dir(root, "set-hotkey", &settings);
    let Some(mut session) = launch_session(options, &dir, &log, Some(HOTKEY_TEXT)) else {
        report.check("set hotkey: Orbit starts", 0, 1);
        return;
    };
    sleep(500);
    let shortcut = orbit_hotkeys::chord::Chord::parse("Ctrl+Alt+F10").expect("chord");
    let direct = Hotkey::Direct(shortcut);
    let press_shortcut = || send(&chord_inputs(&[VK_CONTROL.0, VK_MENU.0, VK_F10_KEY]));
    let notes = builtin(Builtin::SearchNotes);
    let clipboard = builtin(Builtin::ClipboardHistory);
    let new_note = builtin(Builtin::NewNote);

    // A shortcut, then using it.
    if !run_menu_action(&session, sink, chord, "search notes", "set hotkey") {
        stopped(report, TEST);
        quit(&mut session);
        return;
    }
    press_shortcut();
    let saved = poll(Duration::from_secs(2), || action_of(&saved_settings(&dir), &direct) == Some(&notes));
    report.check("Ctrl+K, Set hotkey saves a shortcut you press", saved as u32, 1);
    hide_launcher(&session);
    sink.focus();
    press_shortcut();
    let works = wait_until(Duration::from_secs(1), || session.visible() && session.focused());
    report.check("that shortcut works without restarting", works as u32, 1);
    hide_launcher(&session);

    // Holding the HyperKey.
    if !run_menu_action(&session, sink, chord, "clipboard history", "set hotkey") {
        stopped(report, TEST);
        quit(&mut session);
        return;
    }
    hold_caps_and(VK_K);
    let hyper = Hotkey::Hyper("K".to_string());
    let saved = poll(Duration::from_secs(2), || action_of(&saved_settings(&dir), &hyper) == Some(&clipboard));
    report.check("holding the HyperKey and pressing a key saves a Hyper hotkey", saved as u32, 1);
    hide_launcher(&session);
    sink.focus();
    sink.take_text();
    hold_caps_and(VK_K);
    let works = wait_until(Duration::from_secs(1), || session.visible() && session.focused());
    report.check("that Hyper hotkey works without restarting", works as u32, 1);
    report.check("and types nothing into the app behind", sink.take_text().is_empty() as u32, 1);
    hide_launcher(&session);

    // A plain key isn't a hotkey and Esc leaves everything as it was.
    if !run_menu_action(&session, sink, chord, "new note", "set hotkey") {
        stopped(report, TEST);
        quit(&mut session);
        return;
    }
    let before = saved_settings(&dir);
    press('J' as u16);
    sleep(150);
    let typed = !long_text(session.edit).to_lowercase().contains('j');
    press(VK_ESCAPE.0);
    sleep(150);
    let after = saved_settings(&dir);
    report.check("while recording, a plain key does nothing", typed as u32, 1);
    report.check("Esc cancels recording and changes nothing", (before == after && hotkey_for(&after, &new_note).is_none()) as u32, 1);
    hide_launcher(&session);

    // A key already in use asks first, then moves.
    if !run_menu_action(&session, sink, chord, "new note", "set hotkey") {
        stopped(report, TEST);
        quit(&mut session);
        return;
    }
    press_shortcut();
    sleep(250);
    let asked = action_of(&saved_settings(&dir), &direct) == Some(&notes);
    report.check("a key already in use isn't taken without asking", asked as u32, 1);
    if !may_inject(&session, sink) {
        stopped(report, TEST);
        quit(&mut session);
        return;
    }
    press(VK_RETURN.0);
    let moved = poll(Duration::from_secs(2), || {
        let current = saved_settings(&dir);
        action_of(&current, &direct) == Some(&new_note) && hotkey_for(&current, &notes).is_none()
    });
    report.check("confirming moves the key: one key runs one item", moved as u32, 1);
    hide_launcher(&session);

    // Removing it.
    if !run_menu_action(&session, sink, chord, "new note", "remove hotkey") {
        stopped(report, TEST);
        quit(&mut session);
        return;
    }
    let removed = poll(Duration::from_secs(2), || hotkey_for(&saved_settings(&dir), &new_note).is_none());
    report.check("Remove hotkey takes it off", removed as u32, 1);
    hide_launcher(&session);
    sink.focus();
    press_shortcut();
    sleep(300);
    report.check("the removed shortcut no longer opens anything", (!session.visible()) as u32, 1);
    hide_launcher(&session);
    quit(&mut session);
    sink.focus();
}

// --- A window from another program, for the tests below -------------------------------------------

const TEST_APP_CLASS: PCWSTR = w!("OrbitBenchTestApp");

struct TestApp {
    child: std::process::Child,
    hwnd: HWND,
}

impl TestApp {
    /// Copies the probe to this path, so the window belongs to a program with this name and place.
    fn start(options: &E2eOptions, path: &Path, fullscreen: bool) -> Option<TestApp> {
        std::fs::create_dir_all(path.parent()?).ok()?;
        std::fs::copy(probe_path(options), path).ok()?;
        let mut command = Command::new(path);
        command.arg("--window");
        if fullscreen {
            command.arg("--fullscreen");
        }
        let mut child = command.spawn().ok()?;
        let pid = child.id();
        let mut hwnd = None;
        let found = wait_until(Duration::from_secs(3), || {
            hwnd = find_window(pid, TEST_APP_CLASS);
            hwnd.is_some_and(|hwnd| unsafe { GetForegroundWindow() } == hwnd)
        });
        // The hook classifies the program when the foreground changes.
        sleep(200);
        if !found {
            let _ = child.kill();
            let _ = child.wait();
            return None;
        }
        Some(TestApp { child, hwnd: hwnd? })
    }

    fn in_front(&self) -> bool {
        unsafe { GetForegroundWindow() == self.hwnd }
    }

    fn minimized(&self) -> bool {
        unsafe { IsIconic(self.hwnd) }.as_bool()
    }
}

impl Drop for TestApp {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

// --- An app's hotkey, pressed again, minimizes it -------------------------------------------------

fn hotkey_toggles_app(report: &mut Report, options: &E2eOptions, root: &Path, sink: &Sink) {
    const TEST: &str = "an app's hotkey minimizes it";
    if !probe_path(options).exists() {
        report.not_tested.push(format!("{TEST}: the probe program is missing"));
        return;
    }
    let exe = root.join("toggle-app").join("OrbitBenchToggle.exe");
    let exe_text = exe.to_string_lossy().to_lowercase();
    let apps = serde_json::json!([{ "id": "bench-toggle", "name": "Qzbench Toggle", "exe": exe_text, "aumid": "" }]);
    for (toggles, name) in [(true, "toggle-on"), (false, "toggle-off")] {
        let mut settings = hyper_settings(orbit_hotkeys::HyperMode::Internal);
        settings.hyper.bindings = parse_hyper("J = app:Qzbench Toggle").expect("bindings");
        settings.launcher.hotkey_toggles_app = toggles;
        let (dir, log) = fresh_dir(root, name, &settings);
        std::fs::write(dir.join("apps.json"), apps.to_string()).expect("apps");
        let Some(mut session) = launch_session(options, &dir, &log, Some(HOTKEY_TEXT)) else {
            report.check("hotkey toggle: Orbit starts", 0, 1);
            return;
        };
        sleep(500);
        let Some(app) = TestApp::start(options, &exe, false) else {
            report.not_tested.push(format!("{TEST}: the test window didn't take focus"));
            quit(&mut session);
            sink.focus();
            return;
        };
        hold_caps_and(VK_J);
        if toggles {
            let minimized = wait_until(Duration::from_secs(2), || app.minimized());
            report.check("an app's Hyper hotkey, pressed while it's in front, minimizes it", minimized as u32, 1);
            // Something else now has focus, so keys go in only after the sink takes it.
            if sink.focus() {
                hold_caps_and(VK_J);
                let back = wait_until(Duration::from_secs(2), || !app.minimized() && app.in_front());
                report.check("pressed again, it comes back and has focus", back as u32, 1);
            } else {
                stopped(report, TEST);
            }
        } else {
            sleep(300);
            report.check("with the setting off, the hotkey leaves the app as it is", (!app.minimized() && app.in_front()) as u32, 1);
        }
        drop(app);
        quit(&mut session);
        sink.focus();
    }
}

// --- HyperKey over fullscreen windows and games ---------------------------------------------------

fn game_detection(report: &mut Report, options: &E2eOptions, root: &Path, sink: &Sink) {
    const TEST: &str = "HyperKey and games";
    if !probe_path(options).exists() {
        report.not_tested.push(format!("{TEST}: the probe program is missing"));
        return;
    }
    let plain = root.join("plain-app").join("OrbitBenchPlain.exe");
    let game = root.join("steamapps").join("common").join("OrbitBenchGame").join("OrbitBenchGame.exe");
    let counter = root.join("game-count.txt");
    // (window program, fills the screen, setting on, should the chord run, what's checked)
    let cases: [(&Path, bool, bool, bool, &str); 4] = [
        (&plain, true, true, true, "HyperKey keeps working over a fullscreen app that isn't a game"),
        (&game, true, true, false, "HyperKey steps aside for a fullscreen game"),
        (&game, false, true, true, "HyperKey keeps working over a game in a window"),
        (&game, true, false, true, "with the setting off, HyperKey works over a fullscreen game"),
    ];
    for (index, (program, fullscreen, setting, runs, what)) in cases.into_iter().enumerate() {
        let _ = std::fs::remove_file(&counter);
        let mut settings = counting_hyper_settings(orbit_hotkeys::HyperMode::Internal, &probe_path(options), &counter);
        settings.hyper.pass_through_in_games = setting;
        let (dir, log) = fresh_dir(root, &format!("game-{index}"), &settings);
        let Some(mut session) = launch_session(options, &dir, &log, Some(HOTKEY_TEXT)) else {
            report.check("game detection: Orbit starts", 0, 1);
            return;
        };
        sleep(500);
        let Some(app) = TestApp::start(options, program, fullscreen) else {
            report.not_tested.push(format!("{TEST}: the test window didn't take focus"));
            quit(&mut session);
            sink.focus();
            return;
        };
        let caps_before = caps_on();
        hold_caps_and(VK_J);
        let count = if runs { runs_from_chord(&counter, 0) } else { sleep(500); runs_recorded(&counter) };
        report.check(what, (count == runs as usize) as u32, 1);
        // Stepping aside means the key does what it normally does, so its toggle changes.
        let toggled = caps_on() != caps_before;
        report.check(if runs { "and the key stays Hyper, with Caps Lock unchanged" } else { "and the key acts normally in the game" }, (toggled != runs) as u32, 1);
        if toggled && app.in_front() {
            press(VK_CAPS);
        }
        drop(app);
        quit(&mut session);
        sink.focus();
    }
}

// --- Mistyped searches, favourites, and Ctrl+comma ------------------------------------------------

fn probe_command(id: &str, name: &str, probe: &Path, counter: &Path) -> CustomCommand {
    CustomCommand {
        id: id.into(),
        name: name.into(),
        program: probe.to_string_lossy().into_owned(),
        arguments: format!("\"{}\" --append", counter.display()),
        working_dir: String::new(),
        alias: String::new(),
    }
}

/// How many rows the launcher last listed, section titles included.
fn last_row_count(session: &Session) -> Option<usize> {
    log_since_flush(session).lines().rev().find(|line| line.starts_with("query,"))?.split(',').nth(2)?.parse().ok()
}

fn finding_things(report: &mut Report, options: &E2eOptions, root: &Path, sink: &Sink, chord: &[INPUT]) {
    const TEST: &str = "finding things";
    if !probe_path(options).exists() {
        report.not_tested.push(format!("{TEST}: the probe program is missing"));
        return;
    }
    let counter = root.join("finding-count.txt");
    let mut settings = base_settings();
    settings.custom_commands.push(probe_command("bench-find", "Qzbench Target", &probe_path(options), &counter));
    let (dir, log) = fresh_dir(root, "finding", &settings);
    let Some(mut session) = launch_session(options, &dir, &log, Some(HOTKEY_TEXT)) else {
        report.check("finding things: Orbit starts", 0, 1);
        return;
    };
    sleep(400);
    sink.focus();
    if !run_query(&session, chord, "qzbecnh") {
        stopped(report, TEST);
        quit(&mut session);
        return;
    }
    press(VK_RETURN.0);
    report.check("a mistyped search still finds the item, and Enter runs it", (runs_from_chord(&counter, 0) == 1) as u32, 1);
    hide_launcher(&session);

    // Ctrl+comma, from the launcher.
    sink.focus();
    if open_launcher(&session, sink, chord) {
        send(&chord_inputs(&[VK_CONTROL.0, 0xBC]));
        let opened = wait_until(Duration::from_secs(3), || find_window(session.pid, w!("OrbitSettings")).is_some());
        report.check("Ctrl+comma in the launcher opens Settings", opened as u32, 1);
        if let Some(window) = find_window(session.pid, w!("OrbitSettings")) {
            unsafe {
                let _ = PostMessageW(Some(window), WM_CLOSE, WPARAM(0), LPARAM(0));
            }
            wait_until(Duration::from_secs(1), || find_window(session.pid, w!("OrbitSettings")).is_none());
        }
    } else {
        stopped(report, TEST);
    }
    quit(&mut session);
    sink.focus();

    // A favourite comes first, under its section title, and only favourites when that's chosen.
    for (only, name) in [(false, "favourite"), (true, "favourite-only")] {
        let mut settings = settings.clone();
        settings.pinned = vec!["custom:bench-find".to_string()];
        settings.launcher.favorites_only = only;
        let (dir, log) = fresh_dir(root, name, &settings);
        let Some(mut session) = launch_session(options, &dir, &log, Some(HOTKEY_TEXT)) else {
            report.check("favourites: Orbit starts", 0, 1);
            return;
        };
        sleep(400);
        sink.focus();
        let before = runs_recorded(&counter);
        if open_launcher(&session, sink, chord) {
            sleep(150);
            let rows = last_row_count(&session);
            press(VK_RETURN.0);
            report.check(
                if only { "favourites only: Enter on opening runs the favourite" } else { "the first favourite is selected on opening, below its title" },
                (runs_from_chord(&counter, before) == 1) as u32,
                1,
            );
            if only {
                report.check("favourites only: the opening list holds the title and the favourite and nothing else", (rows == Some(2)) as u32, 1);
            } else {
                report.check("without favourites only, the opening list holds more", rows.is_some_and(|rows| rows > 2) as u32, 1);
            }
        } else {
            stopped(report, TEST);
        }
        hide_launcher(&session);
        quit(&mut session);
        sink.focus();
    }
}

// --- macOS feel -----------------------------------------------------------------------------------

fn mac_feel(report: &mut Report, options: &E2eOptions, root: &Path, sink: &Sink) {
    const TEST: &str = "macOS feel";
    const LEFT: u16 = 0x25;
    const RIGHT: u16 = 0x27;
    const BACKSPACE: u16 = 0x08;
    let mut settings = base_settings();
    settings.mac_feel.enabled = true;
    settings.mac_feel.excluded_apps = vec!["orbitbenchmackeep.exe".to_string()];
    let (dir, log) = fresh_dir(root, "mac-feel", &settings);
    let Some(mut session) = launch_session(options, &dir, &log, Some(HOTKEY_TEXT)) else {
        report.check("macOS feel: Orbit starts", 0, 1);
        return;
    };
    sleep(500);
    if !sink.focus() {
        stopped(report, TEST);
        quit(&mut session);
        return;
    }
    sink.take_text();
    let keys = |wanted: &[u16]| {
        send(&chord_inputs(wanted));
        sleep(80);
    };
    type_keys("abc");
    sleep(60);
    keys(&[VK_MENU.0, LEFT]);
    type_keys("x");
    sleep(60);
    keys(&[VK_MENU.0, RIGHT]);
    type_keys("y");
    sleep(100);
    let moved = sink.take_text();
    report.check("Alt+Left and Alt+Right go to the start and end of the line", (moved == "xabcy") as u32, 1);
    if moved != "xabcy" {
        report.not_tested.push(format!("macOS feel: the line read {moved:?}"));
    }
    type_keys("abc");
    sleep(60);
    keys(&[VK_MENU.0, BACKSPACE]);
    sleep(60);
    let deleted = sink.take_text();
    report.check("Alt+Backspace deletes back to the start of the line", deleted.is_empty() as u32, 1);
    type_keys("abc");
    sleep(60);
    keys(&[VK_MENU.0, LEFT]);
    keys(&[VK_MENU.0, VK_SHIFT.0, RIGHT]);
    type_keys("z");
    sleep(100);
    report.check("Alt+Shift+Right selects to the end of the line", (sink.take_text() == "z") as u32, 1);
    let released = wait_until(Duration::from_millis(500), modifiers_clear);
    report.check("no modifier is left held down afterwards", released as u32, 1);

    if probe_path(options).exists() {
        let exe = root.join("mac-app").join("OrbitBenchMac.exe");
        match TestApp::start(options, &exe, false) {
            Some(app) => {
                alt_and('H' as u16);
                report.check("Alt+H minimizes the app in front", wait_until(Duration::from_secs(2), || app.minimized()) as u32, 1);
                drop(app);
            }
            None => report.not_tested.push(format!("{TEST}: the test window didn't take focus for Alt+H")),
        }
        sink.focus();
        match TestApp::start(options, &exe, false) {
            Some(mut app) => {
                alt_and('Q' as u16);
                let closed = wait_until(Duration::from_secs(2), || !unsafe { IsWindow(Some(app.hwnd)) }.as_bool());
                report.check("Alt+Q closes the app in front", closed as u32, 1);
                let _ = app.child.kill();
            }
            None => report.not_tested.push(format!("{TEST}: the test window didn't take focus for Alt+Q")),
        }
        sink.focus();
        let exe = root.join("mac-app").join("OrbitBenchMacKeep.exe");
        match TestApp::start(options, &exe, false) {
            Some(app) => {
                alt_and('H' as u16);
                sleep(300);
                report.check("an app on the list keeps its own Alt keys", (!app.minimized()) as u32, 1);
                drop(app);
            }
            None => report.not_tested.push(format!("{TEST}: the test window didn't take focus for the excluded app")),
        }
    } else {
        report.not_tested.push(format!("{TEST}: the probe program is missing"));
    }
    quit(&mut session);
    sink.focus();
}

// --- 1Password and Ente Auth, through stand-ins for their tools ------------------------------------

fn clipboard_has_format(name: PCWSTR) -> bool {
    unsafe { IsClipboardFormatAvailable(RegisterClipboardFormatW(name)).is_ok() }
}

fn integrations(report: &mut Report, options: &E2eOptions, root: &Path, sink: &Sink, chord: &[INPUT]) {
    const TEST: &str = "integrations";
    const PASSWORD: &str = "fake-pass-Qz7";
    let stand_in = options.orbit.parent().map(Path::to_path_buf).unwrap_or_default().join("orbit-bench-fake-cli.exe");
    if !stand_in.exists() {
        report.not_tested.push(format!("{TEST}: the stand-in tool is missing"));
        return;
    }
    let bin = root.join("stand-in-tools");
    std::fs::create_dir_all(&bin).expect("bin");
    std::fs::copy(&stand_in, bin.join("op.exe")).expect("op");
    std::fs::copy(&stand_in, bin.join("ente.exe")).expect("ente");
    let calls = bin.join("calls.log");
    let mode = bin.join("mode.txt");
    let set_mode = |text: &str| std::fs::write(&mode, text).expect("mode");
    let count_calls = |what: &str| std::fs::read_to_string(&calls).unwrap_or_default().lines().filter(|line| line.contains(what)).count();

    let mut settings = base_settings();
    settings.integrations.one_password = true;
    settings.integrations.ente_auth = true;
    settings.clipboard.enabled = true;
    let (dir, log) = fresh_dir(root, "integrations", &settings);
    // No apps at all, so these searches can't land on the real 1Password or Ente app if it's
    // installed here, and Enter can't open it.
    std::fs::write(dir.join("apps.json"), "[]").expect("apps");
    // Only this copy of Orbit sees the stand-ins.
    let real_path = std::env::var_os("PATH").unwrap_or_default();
    let mut paths = vec![bin.clone()];
    paths.extend(std::env::split_paths(&real_path));
    unsafe { std::env::set_var("PATH", std::env::join_paths(paths).expect("path")) };
    let started = launch_session(options, &dir, &log, Some(HOTKEY_TEXT));
    unsafe { std::env::set_var("PATH", &real_path) };
    let Some(mut session) = started else {
        report.check("integrations: Orbit starts", 0, 1);
        return;
    };
    sleep(400);
    sink.focus();

    // Into 1Password, find an item, and copy its password.
    if !run_query(&session, chord, "1password") {
        stopped(report, TEST);
        quit(&mut session);
        return;
    }
    press(VK_RETURN.0);
    let listed = wait_until(Duration::from_secs(3), || count_calls("op item list --format json") == 1);
    report.check("choosing 1Password asks its tool for the list", listed as u32, 1);
    sleep(250);
    report.check("the launcher stays open, in the list", (session.visible() && session.focused()) as u32, 1);
    // Keys go nowhere but Orbit.
    if !session.focused() {
        stopped(report, TEST);
        quit(&mut session);
        return;
    }
    type_text("qzrouter");
    sleep(150);
    press(VK_RETURN.0);
    let copied = poll(Duration::from_secs(3), || clipboard_text().as_deref() == Some(PASSWORD));
    report.check("Enter on an item copies its password", copied as u32, 1);
    report.check("the right item's password was asked for", (count_calls("op item get qzitem2 --fields label=password --reveal") == 1) as u32, 1);
    report.check("the launcher closes afterwards", wait_until(Duration::from_secs(1), || !session.visible()) as u32, 1);
    let marked = clipboard_has_format(w!("ExcludeClipboardContentFromMonitorProcessing"))
        && clipboard_has_format(w!("CanIncludeInClipboardHistory"))
        && clipboard_has_format(w!("CanUploadToCloudClipboard"));
    report.check("the copy is marked to stay out of clipboard history and the cloud", marked as u32, 1);
    let copied_at = Instant::now();
    let cleared = poll(Duration::from_secs(34), || clipboard_text().as_deref() != Some(PASSWORD));
    let waited = copied_at.elapsed().as_secs();
    report.check("a copied password clears itself after about 30 seconds", (cleared && waited >= 27) as u32, 1);

    // Copying something else in the meantime keeps it. The password must really be gone first, or
    // the wait below could see the old copy and move on before the new one lands.
    set_clipboard(Some("before the second copy"), false);
    sink.focus();
    if !run_query(&session, chord, "1password") {
        stopped(report, TEST);
        quit(&mut session);
        return;
    }
    press(VK_RETURN.0);
    sleep(200);
    report.check("the list isn't fetched again within a few minutes", (count_calls("op item list") == 1) as u32, 1);
    if !session.focused() {
        stopped(report, TEST);
        quit(&mut session);
        return;
    }
    press(VK_RETURN.0);
    if poll(Duration::from_secs(3), || clipboard_text().as_deref() == Some(PASSWORD)) {
        set_clipboard(Some("copied afterwards"), false);
        sleep(31_000);
        report.check("something copied afterwards isn't cleared", (clipboard_text().as_deref() == Some("copied afterwards")) as u32, 1);
    } else {
        stopped(report, TEST);
    }

    // The action menu.
    let open_vault = |session: &Session| -> bool {
        sink.focus();
        if !run_query(session, chord, "1password") {
            return false;
        }
        press(VK_RETURN.0);
        sleep(250);
        session.focused()
    };
    let menu_choice = |choice: &str| {
        send(&chord_inputs(&[VK_CONTROL.0, VK_K]));
        sleep(120);
        type_text(choice);
        sleep(150);
        press(VK_RETURN.0);
    };
    if open_vault(&session) {
        menu_choice("copy username");
        let got = poll(Duration::from_secs(3), || clipboard_text().as_deref() == Some("fake-user-Qz7"));
        report.check("Ctrl+K offers the username", got as u32, 1);
    } else {
        stopped(report, TEST);
    }
    if open_vault(&session) {
        menu_choice("one-time");
        let got = poll(Duration::from_secs(3), || clipboard_text().as_deref() == Some("654321"));
        report.check("Ctrl+K offers the one-time code", got as u32, 1);
    } else {
        stopped(report, TEST);
    }

    // Backing out cancels a value that hasn't come yet.
    set_clipboard(Some("left alone"), false);
    if open_vault(&session) {
        set_mode("slow:1200");
        press(VK_RETURN.0);
        sleep(150);
        press(VK_ESCAPE.0);
        sleep(1600);
        report.check("Esc while waiting means nothing gets copied", (clipboard_text().as_deref() == Some("left alone")) as u32, 1);
        set_mode("");
    } else {
        stopped(report, TEST);
    }
    hide_launcher(&session);

    // A prompt takes focus while the tool is being asked. The launcher waits, then comes back.
    if open_vault(&session) {
        set_mode("slow:1500");
        let lists = count_calls("op item list");
        menu_choice("refresh");
        sleep(300);
        report.check("Refresh asks the tool again", (count_calls("op item list") == lists + 1) as u32, 1);
        sink.focus();
        sleep(300);
        report.check("the launcher stays open while something else has focus during the wait", session.visible() as u32, 1);
        let back = wait_until(Duration::from_secs(4), || session.focused());
        report.check("it takes focus back once the tool answers", back as u32, 1);
        set_mode("");
    } else {
        stopped(report, TEST);
    }
    hide_launcher(&session);

    // Signed out: says what to do, and Try again works once that's done.
    if open_vault(&session) {
        set_mode("signed-out");
        menu_choice("refresh");
        sleep(400);
        let asked = count_calls("op item list");
        set_mode("");
        press(VK_RETURN.0);
        let again = wait_until(Duration::from_secs(3), || count_calls("op item list") == asked + 1);
        report.check("signed out, Enter tries again", again as u32, 1);
        sleep(250);
        press(VK_RETURN.0);
        let works = poll(Duration::from_secs(3), || clipboard_text().as_deref() == Some(PASSWORD));
        report.check("and then it works", works as u32, 1);
    } else {
        stopped(report, TEST);
    }
    hide_launcher(&session);

    // Ente Auth. Enter types the code where the launcher was opened, and Ctrl+Enter copies it.
    sink.focus();
    let typed_before = sink_has(sink, "123456");
    if run_query(&session, chord, "ente auth") {
        press(VK_RETURN.0);
        let listed = wait_until(Duration::from_secs(3), || count_calls("ente auth code list --all --json --no-input --offline") == 1);
        report.check("choosing Ente Auth asks for what's on this PC first", listed as u32, 1);
        wait_until(Duration::from_secs(3), || count_calls("ente auth code list --all --json --no-input") == 2);
        sleep(250);
        press(VK_RETURN.0);
        let typed = !typed_before && wait_until(Duration::from_secs(3), || sink_has(sink, "123456"));
        report.check("Enter on a code types it where you were", typed as u32, 1);
        hide_launcher(&session);
        sink.focus();
        let lists = count_calls("ente auth code list");
        if run_query(&session, chord, "ente auth") {
            press(VK_RETURN.0);
            wait_until(Duration::from_secs(3), || count_calls("ente auth code list") >= lists + 2);
            sleep(250);
            send(&chord_inputs(&[VK_CONTROL.0, VK_RETURN.0]));
            let got = poll(Duration::from_secs(3), || clipboard_text().as_deref() == Some("123456"));
            report.check("Ctrl+Enter on a code copies it", got as u32, 1);
        } else {
            stopped(report, TEST);
        }
        let offline = count_calls("ente auth code view qzcode1 --json --offline --no-input");
        report.check("codes are read without touching the network", (offline == 2 && count_calls("ente auth code view") == 2) as u32, 1);
    } else {
        stopped(report, TEST);
    }
    sleep(100);

    // Nothing from a vault reaches the disk.
    set_clipboard(Some("the end"), false);
    let session_log = std::fs::read(&log).unwrap_or_default();
    quit(&mut session);
    sink.focus();
    let secrets: [&[u8]; 5] = [PASSWORD.as_bytes(), b"fake-user-Qz7", b"654321", b"123456", b"Qzrouter"];
    let mut stored = false;
    let mut files = vec![log.clone(), dir.join("settings.json")];
    for file in ["clipboard.db", "clipboard.db-wal", "orbit.db", "orbit.db-wal"] {
        files.push(dir.join(file));
    }
    for file in &files {
        let bytes = std::fs::read(file).unwrap_or_default();
        stored |= secrets.iter().any(|secret| bytes.windows(secret.len()).any(|w| w == *secret));
    }
    stored |= secrets.iter().any(|secret| session_log.windows(secret.len()).any(|w| w == *secret));
    report.check("no password, code or item name is written to Orbit's files", (!stored) as u32, 1);
    if clips_in(&dir).is_some_and(|clips| clips.iter().any(|clip| clip.preview.contains(PASSWORD))) {
        report.check("copied passwords stay out of Orbit's own history", 0, 1);
    } else {
        report.check("copied passwords stay out of Orbit's own history", 1, 1);
    }

    // No tool: Enter goes to the page with the Install button.
    let own_install = std::env::var_os("LOCALAPPDATA").map(PathBuf::from).unwrap_or_default().join("Programs").join("EnteCLI").join("ente.exe");
    let on_path = Command::new("where").arg("ente.exe").stdout(std::process::Stdio::null()).stderr(std::process::Stdio::null()).status().is_ok_and(|s| s.success());
    if own_install.exists() || on_path {
        report.not_tested.push(format!("{TEST}: the Set up row, because Ente's real tool is installed on this PC"));
    } else {
        let (dir, log) = fresh_dir(root, "integrations-missing", &settings);
        std::fs::write(dir.join("apps.json"), "[]").expect("apps");
        if let Some(mut session) = launch_session(options, &dir, &log, Some(HOTKEY_TEXT)) {
            sleep(400);
            sink.focus();
            if run_query(&session, chord, "ente auth") {
                press(VK_RETURN.0);
                sleep(400);
                press(VK_RETURN.0);
                let opened = wait_until(Duration::from_secs(3), || find_window(session.pid, w!("OrbitSettings")).is_some());
                report.check("with the tool missing, Enter opens Settings", opened as u32, 1);
                if let Some(window) = find_window(session.pid, w!("OrbitSettings")) {
                    sleep(200);
                    let install = settings_control(window, 151);
                    let shown = install.as_ref().is_some_and(|button| unsafe { IsWindowVisible(*button) }.as_bool());
                    report.check("on the Integrations page", shown as u32, 1);
                    let label = install.map(long_text).unwrap_or_default();
                    report.check("whose button offers to install it", (label == "Install") as u32, 1);
                    unsafe {
                        let _ = PostMessageW(Some(window), WM_CLOSE, WPARAM(0), LPARAM(0));
                    }
                    wait_until(Duration::from_secs(1), || find_window(session.pid, w!("OrbitSettings")).is_none());
                }
            } else {
                stopped(report, TEST);
            }
            quit(&mut session);
            sink.focus();
        }
    }
}
