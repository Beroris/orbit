//! Windows end-to-end driver. Runs the real orbit.exe with a private data directory and measures
//! it from outside: keys go in through `SendInput`, and windows are observed by polling.
//!
//! QPC is comparable across processes, so Orbit's own timestamps line up with injection time.

pub mod features;

use crate::E2eOptions;
use crate::report::{Report, UNIT_BYTES_PER_MINUTE, UNIT_CPU_PER_MINUTE, UNIT_WRITES_PER_MINUTE};
use std::os::windows::io::AsRawHandle;
use std::collections::HashMap;
use std::path::Path;
use std::process::{Child, Command};
use std::sync::mpsc;
use std::time::{Duration, Instant};
use windows::Win32::Foundation::*;
use windows::Win32::Graphics::Gdi::{DEVMODEW, ENUM_CURRENT_SETTINGS, EnumDisplaySettingsW, HBRUSH};
use windows::Win32::Storage::FileSystem::{GetDiskFreeSpaceExW, GetDriveTypeW, GetVolumeInformationW};
use windows::Win32::System::Diagnostics::ToolHelp::{
    CreateToolhelp32Snapshot, PROCESSENTRY32W, Process32FirstW, Process32NextW, TH32CS_SNAPPROCESS,
};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::System::Performance::{QueryPerformanceCounter, QueryPerformanceFrequency};
use windows::Win32::System::Power::{GetSystemPowerStatus, SYSTEM_POWER_STATUS};
use windows::Win32::System::ProcessStatus::{GetProcessMemoryInfo, PROCESS_MEMORY_COUNTERS, PROCESS_MEMORY_COUNTERS_EX};
use windows::Win32::System::SystemInformation::{GlobalMemoryStatusEx, MEMORYSTATUSEX};
use windows::Win32::System::Threading::{
    GetProcessIoCounters, GetProcessTimes, IO_COUNTERS, OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION,
};
use windows::Win32::System::WindowsProgramming::QueryProcessCycleTime;
use windows::Win32::UI::Input::KeyboardAndMouse::*;
use windows::Win32::UI::WindowsAndMessaging::*;
use windows::core::{PCWSTR, w};

const WM_ORBIT_RUN_BUILTIN: u32 = WM_APP + 11;
const WM_ORBIT_BENCH_FLUSH: u32 = WM_APP + 20;
const WM_ORBIT_BENCH_TRIM: u32 = WM_APP + 21;
const WM_SINK_FOCUS: u32 = WM_APP + 1;
const EM_GETSEL: u32 = 0x00B0;
/// `WM_ORBIT_RUN_BUILTIN` with this value asks for a clean quit.
const QUIT: WPARAM = WPARAM(usize::MAX);

/// F13 is on almost no keyboard, so nothing else wants this chord.
const HOTKEY_TEXT: &str = "Ctrl+Alt+Shift+F13";
const HOTKEY_VKS: [u16; 4] = [VK_CONTROL.0, VK_MENU.0, VK_SHIFT.0, VK_F13.0];

#[link(name = "ntdll")]
unsafe extern "system" {
    fn NtQuerySystemInformation(class: u32, buffer: *mut u8, length: u32, returned: *mut u32) -> i32;
}

fn qpc() -> i64 {
    let mut t = 0;
    unsafe {
        let _ = QueryPerformanceCounter(&mut t);
    }
    t
}

fn qpc_freq() -> f64 {
    let mut f = 0;
    unsafe {
        let _ = QueryPerformanceFrequency(&mut f);
    }
    f as f64
}

fn wide(text: &str) -> Vec<u16> {
    text.encode_utf16().chain(std::iter::once(0)).collect()
}

fn sleep(ms: u64) {
    std::thread::sleep(Duration::from_millis(ms));
}

pub fn describe_environment(report: &mut Report) {
    let cpu = Command::new("reg")
        .args(["query", r"HKLM\HARDWARE\DESCRIPTION\System\CentralProcessor\0", "/v", "ProcessorNameString"])
        .output()
        .ok()
        .and_then(|o| {
            let text = String::from_utf8_lossy(&o.stdout).to_string();
            text.split("REG_SZ").nth(1).map(|s| s.trim().to_string())
        })
        .unwrap_or_else(|| "unknown".to_string());
    report.environment.insert("cpu".into(), cpu);
    let threads = std::thread::available_parallelism().map(|n| n.get()).unwrap_or(0);
    report.environment.insert("logical processors".into(), threads.to_string());
    let status = memory_status();
    report.environment.insert(
        "ram".into(),
        format!(
            "{:.1} GB total, {:.1} GB free at start, {}% load",
            status.ullTotalPhys as f64 / 1_073_741_824.0,
            status.ullAvailPhys as f64 / 1_073_741_824.0,
            status.dwMemoryLoad
        ),
    );
    report.environment.insert("power at start".into(), power_state());
    if let Ok(output) = Command::new("cmd").args(["/c", "ver"]).output() {
        report.environment.insert("windows".into(), String::from_utf8_lossy(&output.stdout).trim().to_string());
    }
    let mut mode = DEVMODEW { dmSize: std::mem::size_of::<DEVMODEW>() as u16, ..Default::default() };
    if unsafe { EnumDisplaySettingsW(PCWSTR::null(), ENUM_CURRENT_SETTINGS, &mut mode) }.as_bool() {
        report.environment.insert(
            "display".into(),
            format!("{}x{} at {} Hz (primary)", mode.dmPelsWidth, mode.dmPelsHeight, mode.dmDisplayFrequency),
        );
    }
}

/// The machine as the run left it. A load that climbed during the run explains odd numbers.
pub fn describe_environment_at_end(report: &mut Report) {
    let status = memory_status();
    report.environment.insert(
        "ram at end".into(),
        format!("{:.1} GB free, {}% load", status.ullAvailPhys as f64 / 1_073_741_824.0, status.dwMemoryLoad),
    );
    report.environment.insert("power at end".into(), power_state());
}

fn memory_status() -> MEMORYSTATUSEX {
    let mut status = MEMORYSTATUSEX { dwLength: std::mem::size_of::<MEMORYSTATUSEX>() as u32, ..Default::default() };
    unsafe {
        let _ = GlobalMemoryStatusEx(&mut status);
    }
    status
}

/// A laptop on battery may be running slower, so its numbers don't compare with mains power.
fn power_state() -> String {
    let mut status = SYSTEM_POWER_STATUS::default();
    if unsafe { GetSystemPowerStatus(&mut status) }.is_err() {
        return "unknown".to_string();
    }
    let source = match status.ACLineStatus {
        0 => "battery",
        1 => "AC",
        _ => "unknown source",
    };
    let saver = if status.SystemStatusFlag & 1 != 0 { ", battery saver on" } else { "" };
    // Flag 128 means no battery. 255 is what Windows reports when it doesn't know the charge.
    if status.BatteryFlag & 128 != 0 || status.BatteryLifePercent == 255 {
        format!("{source}, no battery reading{saver}")
    } else {
        format!("{source}, battery at {}%{saver}", status.BatteryLifePercent)
    }
}

/// The root of a path, such as `C:\`. A network path has no drive letter to report.
fn drive_root(path: &Path) -> Option<String> {
    let text = path.to_string_lossy();
    let mut chars = text.chars();
    let letter = chars.next().filter(char::is_ascii_alphabetic)?;
    (chars.next() == Some(':')).then(|| format!("{letter}:\\"))
}

fn describe_data_drive(report: &mut Report, dir: &Path) {
    let Some(root) = drive_root(dir) else {
        report.environment.insert("data drive".into(), format!("no drive letter: {}", dir.display()));
        return;
    };
    let wide_root = wide(&root);
    let path = PCWSTR(wide_root.as_ptr());
    let kind = match unsafe { GetDriveTypeW(path) } {
        2 => "removable",
        3 => "fixed",
        4 => "network",
        5 => "optical",
        6 => "RAM disk",
        _ => "unknown type",
    };
    let mut file_system = [0u16; 32];
    let named = unsafe { GetVolumeInformationW(path, None, None, None, None, Some(&mut file_system)) }.is_ok();
    let length = file_system.iter().position(|&c| c == 0).unwrap_or(file_system.len());
    let file_system = if named { String::from_utf16_lossy(&file_system[..length]) } else { "unknown file system".to_string() };
    let (mut free, mut total) = (0u64, 0u64);
    let measured = unsafe { GetDiskFreeSpaceExW(path, Some(&mut free), Some(&mut total), None) }.is_ok();
    let space = if measured {
        format!(", {:.0} GB free of {:.0} GB", free as f64 / 1_073_741_824.0, total as f64 / 1_073_741_824.0)
    } else {
        String::new()
    };
    report.environment.insert("data drive".into(), format!("{root} {kind}, {file_system}{space}"));
}

// --- Injection ----------------------------------------------------------------------------

fn key_input(vk: u16, up: bool) -> INPUT {
    let mut flags = KEYBD_EVENT_FLAGS(0);
    if up {
        flags |= KEYEVENTF_KEYUP;
    }
    INPUT {
        r#type: INPUT_KEYBOARD,
        Anonymous: INPUT_0 { ki: KEYBDINPUT { wVk: VIRTUAL_KEY(vk), wScan: 0, dwFlags: flags, time: 0, dwExtraInfo: 0 } },
    }
}

fn char_inputs(c: u16) -> [INPUT; 2] {
    let make = |flags: KEYBD_EVENT_FLAGS| INPUT {
        r#type: INPUT_KEYBOARD,
        Anonymous: INPUT_0 { ki: KEYBDINPUT { wVk: VIRTUAL_KEY(0), wScan: c, dwFlags: flags, time: 0, dwExtraInfo: 0 } },
    };
    [make(KEYEVENTF_UNICODE), make(KEYEVENTF_UNICODE | KEYEVENTF_KEYUP)]
}

fn send(inputs: &[INPUT]) {
    unsafe {
        SendInput(inputs, std::mem::size_of::<INPUT>() as i32);
    }
}

fn chord_inputs(vks: &[u16]) -> Vec<INPUT> {
    let mut inputs: Vec<INPUT> = vks.iter().map(|vk| key_input(*vk, false)).collect();
    inputs.extend(vks.iter().rev().map(|vk| key_input(*vk, true)));
    inputs
}

// --- The sink: our own focused window, to catch keys that would have leaked ---------------

unsafe extern "system" fn sink_proc(hwnd: HWND, message: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    unsafe {
        match message {
            WM_SINK_FOCUS => {
                let _ = SetForegroundWindow(hwnd);
                if let Ok(edit) = FindWindowExW(Some(hwnd), None, w!("EDIT"), PCWSTR::null()) {
                    let _ = SetFocus(Some(edit));
                }
                LRESULT(0)
            }
            WM_SETFOCUS => {
                if let Ok(edit) = FindWindowExW(Some(hwnd), None, w!("EDIT"), PCWSTR::null()) {
                    let _ = SetFocus(Some(edit));
                }
                LRESULT(0)
            }
            _ => DefWindowProcW(hwnd, message, wparam, lparam),
        }
    }
}

struct Sink {
    hwnd: isize,
    edit: isize,
}

impl Sink {
    fn start() -> Sink {
        let (tx, rx) = mpsc::channel();
        std::thread::spawn(move || unsafe {
            let module = GetModuleHandleW(None).expect("module");
            let class = WNDCLASSW {
                lpfnWndProc: Some(sink_proc),
                hInstance: module.into(),
                lpszClassName: w!("OrbitBenchSink"),
                hbrBackground: HBRUSH((5 + 1) as *mut _),
                ..Default::default()
            };
            RegisterClassW(&class);
            let hwnd = CreateWindowExW(
                WS_EX_TOPMOST,
                w!("OrbitBenchSink"),
                w!("Orbit benchmark running. Please don't type or click."),
                WS_OVERLAPPED | WS_CAPTION | WS_VISIBLE,
                40,
                40,
                520,
                110,
                None,
                None,
                Some(module.into()),
                None,
            )
            .expect("sink window");
            let edit = CreateWindowExW(
                WINDOW_EX_STYLE(0),
                w!("EDIT"),
                w!(""),
                WS_CHILD | WS_VISIBLE | WS_BORDER | WINDOW_STYLE(0x80), // ES_AUTOHSCROLL: no limit at the visible width
                10,
                10,
                480,
                26,
                Some(hwnd),
                None,
                Some(module.into()),
                None,
            )
            .expect("sink edit");
            tx.send((hwnd.0 as isize, edit.0 as isize)).expect("send");
            let mut message = MSG::default();
            while GetMessageW(&mut message, None, 0, 0).as_bool() {
                let _ = TranslateMessage(&message);
                DispatchMessageW(&message);
            }
        });
        let (hwnd, edit) = rx.recv().expect("sink ready");
        Sink { hwnd, edit }
    }

    fn hwnd(&self) -> HWND {
        HWND(self.hwnd as *mut _)
    }

    fn focus(&self) -> bool {
        unsafe {
            // Windows lets the process that sent the last input take the foreground.
            send(&[key_input(VK_F24.0, false), key_input(VK_F24.0, true)]);
            let _ = PostMessageW(Some(self.hwnd()), WM_SINK_FOCUS, WPARAM(0), LPARAM(0));
        }
        wait_until(Duration::from_millis(800), || unsafe { GetForegroundWindow() } == self.hwnd())
    }

    fn take_text(&self) -> String {
        let edit = HWND(self.edit as *mut _);
        let text = read_text(edit);
        unsafe {
            let empty = wide("");
            SendMessageW(edit, WM_SETTEXT, None, Some(LPARAM(empty.as_ptr() as isize)));
        }
        text
    }
}

fn read_text(hwnd: HWND) -> String {
    unsafe {
        let mut buffer = [0u16; 256];
        let mut result = 0usize;
        let ok = SendMessageTimeoutW(
            hwnd,
            WM_GETTEXT,
            WPARAM(buffer.len()),
            LPARAM(buffer.as_mut_ptr() as isize),
            SMTO_ABORTIFHUNG,
            500,
            Some(&mut result),
        );
        if ok.0 == 0 {
            return String::from("<no reply>");
        }
        String::from_utf16_lossy(&buffer[..result.min(buffer.len())])
    }
}

/// A precise short wait. `sleep` can overshoot by several milliseconds.
fn spin_for(ms: u64) {
    let start = Instant::now();
    while start.elapsed() < Duration::from_millis(ms) {
        std::hint::spin_loop();
    }
}

fn wait_until(limit: Duration, mut condition: impl FnMut() -> bool) -> bool {
    let start = Instant::now();
    while start.elapsed() < limit {
        if condition() {
            return true;
        }
        std::thread::yield_now();
    }
    condition()
}

// --- Observing the process ----------------------------------------------------------------

#[derive(Clone, Copy, Debug, Default)]
struct Memory {
    private_kb: f64,
    working_set_kb: f64,
    peak_working_set_kb: f64,
    page_faults: u32,
}

fn process_handle(child: &Child) -> HANDLE {
    HANDLE(child.as_raw_handle() as *mut _)
}

fn memory_of(process: HANDLE) -> Memory {
    let mut counters = PROCESS_MEMORY_COUNTERS_EX::default();
    counters.cb = std::mem::size_of::<PROCESS_MEMORY_COUNTERS_EX>() as u32;
    unsafe {
        let _ = GetProcessMemoryInfo(process, &mut counters as *mut _ as *mut PROCESS_MEMORY_COUNTERS, counters.cb);
    }
    Memory {
        private_kb: counters.PrivateUsage as f64 / 1024.0,
        working_set_kb: counters.WorkingSetSize as f64 / 1024.0,
        peak_working_set_kb: counters.PeakWorkingSetSize as f64 / 1024.0,
        page_faults: counters.PageFaultCount,
    }
}

fn cpu_ms(process: HANDLE) -> f64 {
    let (mut create, mut exit, mut kernel, mut user) = Default::default();
    unsafe {
        let _ = GetProcessTimes(process, &mut create, &mut exit, &mut kernel, &mut user);
    }
    let ticks = |t: FILETIME| ((t.dwHighDateTime as u64) << 32 | t.dwLowDateTime as u64) as f64;
    (ticks(kernel) + ticks(user)) / 10_000.0
}

fn cycles(process: HANDLE) -> u64 {
    let mut value = 0u64;
    unsafe {
        let _ = QueryProcessCycleTime(process, &mut value);
    }
    value
}

/// Context switches per thread of the process, by thread ID. A plain sum wouldn't do: a pool thread
/// that exits during the window takes its count with it, which can hide real activity.
fn context_switches(pid: u32) -> Option<HashMap<u64, u64>> {
    const SYSTEM_PROCESS_INFORMATION: u32 = 5;
    const PROCESS_HEADER: usize = 0x100;
    const THREAD_SIZE: usize = 0x50;
    const THREAD_ID_OFFSET: usize = 48;
    const THREAD_SWITCHES_OFFSET: usize = 64;
    const PID_OFFSET: usize = 0x50;
    let mut buffer = vec![0u8; 1 << 22];
    let mut returned = 0u32;
    let status = unsafe {
        NtQuerySystemInformation(SYSTEM_PROCESS_INFORMATION, buffer.as_mut_ptr(), buffer.len() as u32, &mut returned)
    };
    if status != 0 {
        return None;
    }
    let mut offset = 0usize;
    loop {
        let entry = &buffer[offset..];
        let next = u32::from_ne_bytes(entry[0..4].try_into().ok()?) as usize;
        let threads = u32::from_ne_bytes(entry[4..8].try_into().ok()?);
        let process = usize::from_ne_bytes(entry[PID_OFFSET..PID_OFFSET + 8].try_into().ok()?);
        if process == pid as usize {
            let mut counts = HashMap::with_capacity(threads as usize);
            for i in 0..threads as usize {
                let thread = PROCESS_HEADER + i * THREAD_SIZE;
                let id = usize::from_ne_bytes(entry[thread + THREAD_ID_OFFSET..thread + THREAD_ID_OFFSET + 8].try_into().ok()?) as u64;
                let at = thread + THREAD_SWITCHES_OFFSET;
                counts.insert(id, u32::from_ne_bytes(entry[at..at + 4].try_into().ok()?) as u64);
            }
            return Some(counts);
        }
        if next == 0 {
            return None;
        }
        offset += next;
    }
}

/// Switches during a window, counting threads that started in it from zero.
fn switches_since(before: &HashMap<u64, u64>, after: &HashMap<u64, u64>) -> u64 {
    after.iter().map(|(id, count)| count.saturating_sub(before.get(id).copied().unwrap_or(0))).sum()
}

/// Bytes and operations written. `GetProcessIoCounters` counts all of the process's file and device
/// writes, not only those that reach a disk, so this can overstate disk use but never hide it.
fn writes(process: HANDLE) -> Option<(u64, u64)> {
    let mut counters = IO_COUNTERS::default();
    unsafe { GetProcessIoCounters(process, &mut counters) }.ok()?;
    Some((counters.WriteTransferCount, counters.WriteOperationCount))
}

/// When a process started, in FILETIME ticks.
fn started_at(pid: u32) -> Option<u64> {
    unsafe {
        let process = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid).ok()?;
        let (mut create, mut exit, mut kernel, mut user) = Default::default();
        let result = GetProcessTimes(process, &mut create, &mut exit, &mut kernel, &mut user);
        let _ = CloseHandle(process);
        result.ok()?;
        Some((create.dwHighDateTime as u64) << 32 | create.dwLowDateTime as u64)
    }
}

/// How many processes are `root` or descend from it. A listed parent id can be stale, because it
/// isn't cleared when the parent exits and the id may have been given to `root` since.
/// `started_after(child, parent)` rules those out.
fn count_tree(processes: &[(u32, u32)], root: u32, started_after: impl Fn(u32, u32) -> bool) -> u32 {
    let mut found = vec![root];
    let mut next = 0;
    while next < found.len() {
        let parent = found[next];
        next += 1;
        for &(pid, listed_parent) in processes {
            if listed_parent == parent && !found.contains(&pid) && started_after(pid, parent) {
                found.push(pid);
            }
        }
    }
    found.len() as u32
}

/// Orbit plus any child processes alive now, or `None` if the process list can't be read.
fn process_count(root: u32) -> Option<u32> {
    let mut processes = Vec::new();
    unsafe {
        let snapshot = CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0).ok()?;
        let mut entry = PROCESSENTRY32W { dwSize: std::mem::size_of::<PROCESSENTRY32W>() as u32, ..Default::default() };
        let mut more = Process32FirstW(snapshot, &mut entry).is_ok();
        while more {
            processes.push((entry.th32ProcessID, entry.th32ParentProcessID));
            more = Process32NextW(snapshot, &mut entry).is_ok();
        }
        let _ = CloseHandle(snapshot);
    }
    if !processes.iter().any(|(pid, _)| *pid == root) {
        return None;
    }
    Some(count_tree(&processes, root, |child, parent| match (started_at(child), started_at(parent)) {
        (Some(child), Some(parent)) => child >= parent,
        // One that can't be inspected counts: better to look heavier than to hide a process.
        _ => true,
    }))
}

fn find_main_window(pid: u32) -> Option<HWND> {
    unsafe {
        let mut after: Option<HWND> = None;
        loop {
            let hwnd = FindWindowExW(None, after, w!("OrbitLauncher"), PCWSTR::null()).ok()?;
            if hwnd.is_invalid() {
                return None;
            }
            let mut owner = 0u32;
            GetWindowThreadProcessId(hwnd, Some(&mut owner));
            if owner == pid {
                return Some(hwnd);
            }
            after = Some(hwnd);
        }
    }
}

// --- The run ------------------------------------------------------------------------------

struct Session {
    child: Child,
    pid: u32,
    main: HWND,
    edit: HWND,
    log: std::path::PathBuf,
    log_read: usize,
    freq: f64,
}

#[derive(Debug, Default)]
struct Timings {
    shows: Vec<[i64; 5]>,
    queries: Vec<(usize, usize, i64, i64, i64)>,
    actions: Vec<(i64, i64)>,
}

impl Session {
    fn message(&self, message: u32) -> usize {
        let mut result = 0usize;
        unsafe {
            let _ = SendMessageTimeoutW(self.main, message, WPARAM(0), LPARAM(0), SMTO_ABORTIFHUNG, 2000, Some(&mut result));
        }
        result
    }

    /// Timing lines written since the last call.
    fn take_timings(&mut self) -> Timings {
        self.message(WM_ORBIT_BENCH_FLUSH);
        let text = std::fs::read_to_string(&self.log).unwrap_or_default();
        let fresh = &text[self.log_read.min(text.len())..];
        let mut out = Timings::default();
        for line in fresh.lines() {
            let parts: Vec<&str> = line.split(',').collect();
            let number = |i: usize| parts.get(i).and_then(|p| p.parse::<i64>().ok()).unwrap_or(0);
            match parts.first().copied() {
                Some("show") => out.shows.push([number(2), number(3), number(4), number(5), number(6)]),
                Some("query") => out.queries.push((number(1) as usize, number(2) as usize, number(3), number(4), number(5))),
                Some("action") => out.actions.push((number(2), number(3))),
                _ => {}
            }
        }
        self.log_read = text.len();
        out
    }

    fn ms(&self, from: i64, to: i64) -> f64 {
        (to - from) as f64 * 1000.0 / self.freq
    }

    fn visible(&self) -> bool {
        unsafe { IsWindowVisible(self.main) }.as_bool()
    }

    fn focused(&self) -> bool {
        unsafe { GetForegroundWindow() == self.main }
    }

    fn hide(&self) -> bool {
        // The first Esc clears anything typed, and the next closes.
        for _ in 0..2 {
            if self.visible() && self.focused() {
                send(&chord_inputs(&[VK_ESCAPE.0]));
            }
            if wait_until(Duration::from_millis(400), || !self.visible()) {
                return true;
            }
        }
        !self.visible()
    }
}

/// Only the sink or Orbit may hold focus when keys are injected. Anything else means the user
/// did something, and the run must stop rather than type into their window.
struct Guard<'a> {
    sink: &'a Sink,
    main: HWND,
}

impl Guard<'_> {
    fn safe(&self) -> bool {
        let foreground = unsafe { GetForegroundWindow() };
        foreground == self.sink.hwnd() || foreground == self.main
    }

    fn ready(&self) -> bool {
        self.safe() || self.sink.focus()
    }
}

fn snapshot(report: &mut Report, label: &str, process: HANDLE, gated: bool) -> Memory {
    let memory = memory_of(process);
    report.add(format!("memory {label}: private"), memory.private_kb, "KB", gated);
    report.add(format!("memory {label}: working set"), memory.working_set_kb, "KB", gated);
    memory
}

fn write_settings(dir: &Path, probe: &Path, probe_file: &Path) {
    let settings = serde_json::json!({
        "schema_version": 1,
        "onboarded": true,
        // The default Google link would otherwise have its icon fetched during the run.
        "fetch_link_icons": false,
        "custom_commands": [{
            "id": "bench-probe",
            "name": "Orbit Bench Probe",
            "program": probe.to_string_lossy(),
            "arguments": format!("\"{}\"", probe_file.display()),
            "working_dir": "",
            "alias": "obp",
        }],
    });
    std::fs::write(dir.join("settings.json"), settings.to_string()).expect("write settings");
}

/// Sends the chord and waits for the window to appear and take focus.
fn timed_open(session: &Session, chord: &[INPUT], limit: Duration) -> (i64, i64, i64) {
    let t = qpc();
    send(chord);
    let (mut t_visible, mut t_focused) = (0, 0);
    wait_until(limit, || {
        if t_visible == 0 && session.visible() {
            t_visible = qpc();
        }
        if t_focused == 0 && session.focused() {
            t_focused = qpc();
        }
        t_visible != 0 && t_focused != 0
    });
    (t, t_visible, t_focused)
}

pub fn run(report: &mut Report, options: &E2eOptions) {
    let freq = qpc_freq();
    let dir = options.work_dir.clone();
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("bench data dir");
    describe_data_drive(report, &dir);
    let log = dir.join("timings.csv");
    let probe_file = dir.join("probe.txt");
    let exe_dir = options.orbit.parent().map(Path::to_path_buf).unwrap_or_default();
    write_settings(&dir, &exe_dir.join("orbit-bench-probe.exe"), &probe_file);

    if !options.orbit.exists() {
        report.not_tested.push(format!("e2e: {} does not exist; build it first", options.orbit.display()));
        return;
    }
    let size = std::fs::metadata(&options.orbit).map(|m| m.len()).unwrap_or(0);
    report.add("binary size: orbit.exe", size as f64 / 1024.0, "KB", true);

    if options.records > 0 {
        eprintln!("seeding {} notes and clips...", options.records);
        let store = orbit_data::Store::open(&dir, true).expect("store");
        crate::data_bench::seed(&store, options.records, options.records);
        drop(store);
        report.environment.insert("e2e data set".into(), format!("{} notes, {} clips", options.records, options.records));
    } else {
        report.environment.insert("e2e data set".into(), "empty".into());
    }

    // App indexing, in the short-lived helper process.
    let start = Instant::now();
    let mut indexer = Command::new(&options.orbit).args(["--index-apps", "--data-dir"]).arg(&dir).spawn().expect("indexer");
    let handle = process_handle(&indexer);
    let mut peak = Memory::default();
    while indexer.try_wait().ok().flatten().is_none() {
        let now = memory_of(handle);
        if now.peak_working_set_kb > 0.0 {
            peak = now;
        }
        sleep(5);
    }
    report.add("app index: helper duration", start.elapsed().as_secs_f64() * 1000.0, "ms", false);
    report.add("app index: helper peak working set (sampled)", peak.peak_working_set_kb, "KB", false);
    let app_count = std::fs::read_to_string(dir.join("apps.json"))
        .ok()
        .and_then(|text| serde_json::from_str::<serde_json::Value>(&text).ok())
        .and_then(|v| v.as_array().map(Vec::len))
        .unwrap_or(0);
    report.add("app index: apps found", app_count as f64, "count", false);

    // Start: launch to window existing and shortcut registered.
    let sink = Sink::start();
    let t_start = Instant::now();
    let child = Command::new(&options.orbit)
        .arg("--data-dir")
        .arg(&dir)
        .args(["--hotkey", HOTKEY_TEXT, "--no-reindex", "--no-trim", "--bench-log"])
        .arg(&log)
        .spawn()
        .expect("start orbit");
    let pid = child.id();
    let mut main = None;
    while main.is_none() && t_start.elapsed() < Duration::from_secs(10) {
        main = find_main_window(pid);
        std::thread::yield_now();
    }
    let Some(main) = main else {
        report.not_tested.push("e2e: Orbit's window never appeared".to_string());
        return;
    };
    // The window exists before startup has finished; a reply means the message loop is running.
    let mut result = 0usize;
    unsafe {
        let _ = SendMessageTimeoutW(main, WM_ORBIT_BENCH_FLUSH, WPARAM(0), LPARAM(0), SMTO_NORMAL, 5000, Some(&mut result));
    }
    report.add("startup: launch to message loop", t_start.elapsed().as_secs_f64() * 1000.0, "ms", true);
    let edit = unsafe { FindWindowExW(Some(main), None, w!("EDIT"), PCWSTR::null()) }.expect("edit control");

    let mut session = Session { child, pid, main, edit, log, log_read: 0, freq };
    let process = process_handle(&session.child);
    let guard = Guard { sink: &sink, main };

    sleep(1500);
    snapshot(report, "at idle after start", process, true);
    if !sink.focus() {
        report.not_tested.push(
            "e2e: Windows wouldn't give the benchmark window focus, so keyboard tests were skipped. Run it from an active terminal.".to_string(),
        );
        finish(report, session);
        return;
    }

    let finished = keyboard_suite(report, options, &mut session, &guard, process, &probe_file);
    if !finished {
        report.not_tested.push("e2e stopped early because another window took focus. Results above are partial.".to_string());
    }
    finish(report, session);
}

/// Returns false if another window took focus and the run had to stop.
fn keyboard_suite(
    report: &mut Report,
    options: &E2eOptions,
    session: &mut Session,
    guard: &Guard,
    process: HANDLE,
    probe_file: &Path,
) -> bool {
    let sink = guard.sink;
    let chord = chord_inputs(&HOTKEY_VKS);
    let _ = session.take_timings();

    // --- Warm opens -------------------------------------------------------------------
    let mut to_visible = Vec::new();
    let mut to_focused = Vec::new();
    let mut injects = Vec::new();
    let mut opens_ok = 0;
    let mut focus_restored = 0;
    for i in 0..options.opens {
        if !guard.ready() {
            return false;
        }
        let (t, t_visible, t_focused) = timed_open(session, &chord, Duration::from_secs(2));
        if t_visible != 0 && t_focused != 0 {
            opens_ok += 1;
            injects.push(t);
            to_visible.push(session.ms(t, t_visible));
            to_focused.push(session.ms(t, t_focused));
        }
        sleep(60 + (i as u64 * 37) % 90);
        if !guard.safe() {
            return false;
        }
        session.hide();
        if wait_until(Duration::from_millis(500), || unsafe { GetForegroundWindow() } == sink.hwnd()) {
            focus_restored += 1;
        }
        sleep(40 + (i as u64 * 53) % 120);
    }
    report.check("hotkey opens the launcher and focuses it", opens_ok, options.opens as u32);
    report.check("Esc returns focus to the previous window", focus_restored, options.opens as u32);
    report.add_timings("warm open: key injected to window visible (external)", &to_visible, true);
    report.add_timings("warm open: key injected to foreground (external)", &to_focused, true);
    let timings = session.take_timings();
    let shows: Vec<[i64; 5]> = timings.shows.into_iter().filter(|s| s[0] != 0).collect();
    let span = |a: usize, b: usize| -> Vec<f64> {
        shows.iter().filter(|s| s[a] != 0 && s[b] != 0).map(|s| session.ms(s[a], s[b])).collect()
    };
    report.add_timings("warm open: T0 hotkey message to T1 frame ready", &span(0, 1), true);
    report.add_timings("warm open: T0 to T2 window shown and painted", &span(0, 2), true);
    report.add_timings("warm open: T0 to T3 input focused", &span(0, 4), true);
    report.add_timings("warm open: T0 to T2b DwmFlush returned (compositor ack)", &span(0, 3), false);
    if shows.len() == injects.len() {
        let delivery: Vec<f64> = shows.iter().zip(&injects).map(|(s, t)| session.ms(*t, s[0])).collect();
        report.add_timings("warm open: key injected to T0 (OS delivery)", &delivery, false);
    }
    snapshot(report, "after warm opens", process, true);

    // --- Typing immediately after the shortcut ------------------------------------------
    // Keys reach whichever window is focused when Windows processes them, so this is a race the
    // launcher can only win by being fast. Each gap is measured separately, and the gated check
    // uses a gap far shorter than the quickest human could manage.
    let typed = "immediate";
    const ROUNDS: u32 = 20;
    for gap_ms in [0u64, 5, 15, 30] {
        let mut none_lost = 0;
        let mut none_leaked = 0;
        for _ in 0..ROUNDS {
            if !guard.ready() {
                return false;
            }
            sink.take_text();
            send(&chord);
            if gap_ms > 0 {
                spin_for(gap_ms);
            }
            for unit in typed.encode_utf16() {
                send(&char_inputs(unit));
                if gap_ms > 0 {
                    spin_for(gap_ms.min(5));
                }
            }
            sleep(150);
            if read_text(session.edit) == typed {
                none_lost += 1;
            }
            if !guard.safe() {
                return false;
            }
            session.hide();
            wait_until(Duration::from_millis(500), || unsafe { GetForegroundWindow() } == sink.hwnd());
            sleep(40);
            if sink.take_text().is_empty() {
                none_leaked += 1;
            }
            sleep(60);
        }
        if gap_ms >= 15 {
            report.check(format!("typing {gap_ms} ms after the shortcut: no keys lost"), none_lost, ROUNDS);
            report.check(format!("typing {gap_ms} ms after the shortcut: no keys reach the old window"), none_leaked, ROUNDS);
        } else {
            report.add(format!("typing {gap_ms} ms after the shortcut: rounds with all keys captured, of {ROUNDS}"), none_lost as f64, "count", false);
        }
    }

    // --- Search as you type -------------------------------------------------------------
    let _ = session.take_timings();
    let mut inject_times = Vec::new();
    let queries = ["calc", "settings", "note", "clip", "zzqq", "2*21", "help", "orbit bench", "chrome", "code"];
    for query in queries {
        if !guard.ready() {
            return false;
        }
        send(&chord);
        if !wait_until(Duration::from_secs(1), || session.focused()) {
            continue;
        }
        sleep(40);
        for unit in query.encode_utf16() {
            if !session.focused() {
                return false;
            }
            inject_times.push(qpc());
            send(&char_inputs(unit));
            sleep(35);
        }
        sleep(60);
        session.hide();
        sleep(60);
    }
    let timings = session.take_timings();
    let typed_queries: Vec<_> = timings.queries.iter().filter(|q| q.0 > 0).copied().collect();
    let rank: Vec<f64> = typed_queries.iter().map(|q| session.ms(q.2, q.3)).collect();
    let draw: Vec<f64> = typed_queries.iter().map(|q| session.ms(q.3, q.4)).collect();
    let total: Vec<f64> = typed_queries.iter().map(|q| session.ms(q.2, q.4)).collect();
    report.add_timings("typing: T4 query seen to T5 results built", &rank, true);
    report.add_timings("typing: T5 to T6 frame drawn", &draw, true);
    report.add_timings("typing: T4 to T6 total", &total, true);
    if typed_queries.len() == inject_times.len() {
        let external: Vec<f64> = typed_queries.iter().zip(&inject_times).map(|(q, t)| session.ms(*t, q.4)).collect();
        report.add_timings("typing: key injected to T6 frame drawn", &external, true);
    } else {
        report.not_tested.push(format!(
            "typing: key injected to frame drawn: {} keys but {} query events, so they can't be paired",
            inject_times.len(),
            typed_queries.len()
        ));
    }
    snapshot(report, "after typing", process, true);

    if !open_with_results(report, session, guard, process, &chord) {
        return false;
    }
    if !many_searches(report, session, guard, process, &chord) {
        return false;
    }
    if !typing_during_updates(report, session, guard, &chord) {
        return false;
    }

    // --- Launching: Enter to a new process existing --------------------------------------
    let mut launch_ok = 0;
    let mut launch_ms = Vec::new();
    const LAUNCHES: u32 = 10;
    let _ = session.take_timings();
    for _ in 0..LAUNCHES {
        if !guard.ready() {
            return false;
        }
        let _ = std::fs::remove_file(probe_file);
        send(&chord);
        if !wait_until(Duration::from_secs(1), || session.focused()) {
            continue;
        }
        sleep(30);
        for unit in "orbit bench probe".encode_utf16() {
            send(&char_inputs(unit));
        }
        sleep(80);
        if !session.focused() {
            return false;
        }
        let t = qpc();
        send(&chord_inputs(&[VK_RETURN.0]));
        let limit = Instant::now();
        while limit.elapsed() < Duration::from_secs(4) {
            if let Some(ticks) = std::fs::read_to_string(probe_file).ok().and_then(|s| s.trim().parse::<i64>().ok()) {
                launch_ms.push(session.ms(t, ticks));
                launch_ok += 1;
                break;
            }
            sleep(2);
        }
        sleep(100);
        sink.focus();
        sleep(50);
    }
    report.check("Enter launches the selected command", launch_ok, LAUNCHES);
    report.add_timings("launch: Enter injected to new process running", &launch_ms, false);
    let timings = session.take_timings();
    let dispatch: Vec<f64> = timings.actions.iter().map(|a| session.ms(a.0, a.1)).collect();
    report.add_timings("launch: Enter received to launch dispatched", &dispatch, true);
    snapshot(report, "after 10 launches", process, true);

    // --- A real installed app, once ------------------------------------------------------
    if let Some((app, process_name)) = &options.real_app {
        real_app(report, session, guard, app, process_name, &chord);
        snapshot(report, "after real app launch", process, false);
    } else {
        report.not_tested.push("launch of a real installed app: run with --real-app NAME:PROCESS".to_string());
    }

    // --- Rapid toggling ---------------------------------------------------------------------
    if !guard.ready() {
        return false;
    }
    for _ in 0..40 {
        send(&chord);
        sleep(12);
    }
    sleep(400);
    let alive = session.child.try_wait().ok().flatten().is_none();
    report.check("survives 40 rapid shortcut presses", alive as u32, 1);
    if session.visible() {
        session.hide();
    }

    // --- Working-set trim ---------------------------------------------------------------
    sink.focus();
    sleep(300);
    let before = memory_of(process);
    session.message(WM_ORBIT_BENCH_TRIM);
    sleep(500);
    let after = memory_of(process);
    report.add("memory trim: working set before", before.working_set_kb, "KB", false);
    report.add("memory trim: working set after", after.working_set_kb, "KB", true);
    report.add("memory trim: private after", after.private_kb, "KB", true);

    // --- Reopening after pages were trimmed ----------------------------------------------
    let mut cold_visible = Vec::new();
    let mut cold_focused = Vec::new();
    let mut faults = Vec::new();
    let _ = session.take_timings();
    for _ in 0..15 {
        if !guard.ready() {
            return false;
        }
        session.message(WM_ORBIT_BENCH_TRIM);
        sleep(400);
        let faults_before = memory_of(process).page_faults;
        let (t, t_visible, t_focused) = timed_open(session, &chord, Duration::from_secs(3));
        if t_visible != 0 {
            cold_visible.push(session.ms(t, t_visible));
        }
        if t_focused != 0 {
            cold_focused.push(session.ms(t, t_focused));
        }
        sleep(60);
        faults.push((memory_of(process).page_faults - faults_before) as f64);
        session.hide();
        sleep(150);
    }
    report.add_timings("after trim: key injected to window visible (soft faults)", &cold_visible, true);
    report.add_timings("after trim: key injected to foreground (soft faults)", &cold_focused, true);
    report.add("after trim: page faults per open (median)", crate::report::percentile(&faults, 0.5), "count", false);
    let timings = session.take_timings();
    let t0_t3: Vec<f64> = timings.shows.iter().filter(|s| s[4] != 0).map(|s| session.ms(s[0], s[4])).collect();
    report.add_timings("after trim: T0 to T3 input focused", &t0_t3, false);

    // --- Real memory pressure, only when asked for ------------------------------------------
    if options.pressure_mb > 0 {
        pressure(report, session, guard, process, options.pressure_mb, &chord);
    } else {
        report.not_tested.push(
            "true hard-fault reopening under memory pressure: off by default because it slows the whole machine. Use --pressure-mb N. The trim test only exercises soft faults from the standby list.".to_string(),
        );
    }

    // --- Idle --------------------------------------------------------------------------------
    // The end of the previous step, such as the launcher hiding, isn't idle time.
    sleep(2000);
    sink.focus();
    sleep(300);
    session.message(WM_ORBIT_BENCH_TRIM);
    sleep(1000);
    let (cpu_before, cycles_before, memory_before) = (cpu_ms(process), cycles(process), memory_of(process));
    let switches_before = context_switches(session.pid);
    let writes_before = writes(process);
    let started = Instant::now();
    std::thread::sleep(Duration::from_secs(options.idle_secs));
    let seconds = started.elapsed().as_secs_f64();
    let per_minute = 60.0 / seconds;
    report.add("idle: measured for", seconds, "s", false);
    report.add("idle: CPU time per minute", (cpu_ms(process) - cpu_before) * per_minute, UNIT_CPU_PER_MINUTE, true);
    match (writes_before, writes(process)) {
        (Some((bytes_before, operations_before)), Some((bytes, operations))) => {
            report.add("idle: bytes written per minute", bytes.saturating_sub(bytes_before) as f64 * per_minute, UNIT_BYTES_PER_MINUTE, true);
            report.add(
                "idle: write operations per minute",
                operations.saturating_sub(operations_before) as f64 * per_minute,
                UNIT_WRITES_PER_MINUTE,
                true,
            );
        }
        _ => report.not_tested.push("idle writes: Windows wouldn't give the process's I/O counters".to_string()),
    }
    report.add("idle: CPU cycles per minute", (cycles(process) - cycles_before) as f64 * per_minute, "count", false);
    report.add(
        "idle: page faults per minute",
        (memory_of(process).page_faults - memory_before.page_faults) as f64 * per_minute,
        "count",
        false,
    );
    if let (Some(before), Some(after)) = (switches_before, context_switches(session.pid)) {
        let switches = switches_since(&before, &after);
        report.add("idle: context switches per minute (threads still alive at the end)", switches as f64 * per_minute, "count", false);
        report.add("idle: threads", after.len() as f64, "count", true);
    }
    match process_count(session.pid) {
        Some(count) => report.add("processes: Orbit and its children at idle", count as f64, "count", true),
        None => report.not_tested.push("process count at idle: couldn't read the process list".to_string()),
    }
    snapshot(report, "after idle", process, true);
    report.add("memory peak: working set this run", memory_of(process).peak_working_set_kb, "KB", true);
    processes_while_indexing(report, session);
    true
}

/// The launcher open with real results on screen, so the frame and row caches are all in use.
/// Returns false if another window took focus.
fn open_with_results(report: &mut Report, session: &mut Session, guard: &Guard, process: HANDLE, chord: &[INPUT]) -> bool {
    if !guard.ready() {
        return false;
    }
    let _ = session.take_timings();
    send(chord);
    if !wait_until(Duration::from_secs(1), || session.focused()) {
        report.not_tested.push("memory with the launcher open: it didn't take focus".to_string());
        return true;
    }
    sleep(40);
    for unit in "settings".encode_utf16() {
        if !session.focused() {
            return false;
        }
        send(&char_inputs(unit));
        sleep(20);
    }
    sleep(200);
    let results_shown = session.take_timings().queries.iter().any(|q| q.0 > 0 && q.1 > 0);
    if results_shown {
        snapshot(report, "launcher open with results", process, true);
    } else {
        report.not_tested.push("memory with the launcher open: the query showed no results".to_string());
    }
    session.hide();
    sleep(60);
    true
}

/// The query for search number `index`. Different prefixes of different words, so the results
/// keep changing, and some match nothing, as real typing does.
fn search_query(index: usize) -> String {
    const WORDS: [&str; 20] = [
        "calculator", "settings", "note", "clipboard", "help", "orbit", "chrome", "code", "terminal", "lock",
        "sleep", "zzqq", "word", "mail", "photos", "music", "paint", "edge", "store", "files",
    ];
    let word = WORDS[index % WORDS.len()];
    let length = 2 + (index / WORDS.len()) % (word.len() - 1);
    word[..length].to_string()
}

/// Memory after 500 complete queries. Each replaces the last, with enough time for results to come
/// back, and the launcher is reopened now and then. Returns false if another window took focus.
fn many_searches(report: &mut Report, session: &mut Session, guard: &Guard, process: HANDLE, chord: &[INPUT]) -> bool {
    const SEARCHES: usize = 500;
    const PER_OPEN: usize = 25;
    let _ = session.take_timings();
    let started = Instant::now();
    let mut events = 0;
    let mut mistyped = 0;
    let mut first_wrong = None;
    let mut slowest_text = 0f64;
    for index in 0..SEARCHES {
        if index % PER_OPEN == 0 {
            if session.visible() {
                session.hide();
            }
            // Orbit buffers its timing lines only in bench mode. Flushing often keeps that buffer
            // small, so it doesn't count against Orbit in the snapshot below.
            events += session.take_timings().queries.len();
            sleep(40);
            if !guard.ready() {
                return false;
            }
            send(chord);
            if !wait_until(Duration::from_secs(1), || session.focused()) {
                report.not_tested.push(format!("memory after {SEARCHES} searches: the launcher didn't take focus at search {index}"));
                return true;
            }
            sleep(30);
        }
        if !session.focused() {
            return false;
        }
        let query = search_query(index);
        let mut inputs = chord_inputs(&[VK_CONTROL.0, 'A' as u16]);
        for unit in query.encode_utf16() {
            inputs.extend(char_inputs(unit));
        }
        send(&inputs);
        sleep(20);
        if (index + 1) % PER_OPEN == 0 {
            // The keys are queued input, so a read can arrive before the last of them is handled.
            let waited = Instant::now();
            let settled = wait_until(Duration::from_millis(500), || read_text(session.edit) == query);
            slowest_text = slowest_text.max(waited.elapsed().as_secs_f64() * 1000.0);
            if !settled {
                mistyped += 1;
                first_wrong.get_or_insert((index, query.clone(), read_text(session.edit)));
            }
        }
    }
    session.hide();
    events += session.take_timings().queries.len();
    sleep(300);
    if events < SEARCHES || mistyped > 0 {
        report.not_tested.push(format!(
            "memory after {SEARCHES} searches: Orbit logged {events} query events and {mistyped} sampled queries were wrong in the search box, so this wasn't {SEARCHES} real searches. First wrong one (index, typed, in the box): {first_wrong:?}"
        ));
        return true;
    }
    report.add(format!("{SEARCHES} searches: query events seen by Orbit"), events as f64, "count", false);
    report.add(format!("{SEARCHES} searches: longest wait for a sampled query to reach the box, after 20 ms"), slowest_text, "ms", false);
    report.add(format!("{SEARCHES} searches: duration"), started.elapsed().as_secs_f64() * 1000.0, "ms", false);
    snapshot(report, &format!("after {SEARCHES} searches"), process, true);
    true
}

/// Typing while results arrive must lose nothing and leave the caret at the end. Keys start after
/// the launcher has focus, because the race on the first key after the hotkey is measured elsewhere.
/// Returns false if another window took focus.
fn typing_during_updates(report: &mut Report, session: &mut Session, guard: &Guard, chord: &[INPUT]) -> bool {
    const ROUNDS: usize = 50;
    const QUERIES: [&str; 10] = [
        "orbit settings", "new note", "clipboard history", "rebuild app index", "orbit help",
        "search notes", "lock screen", "open terminal", "orbit diagnostics", "open downloads",
    ];
    let _ = session.take_timings();
    let mut text_intact = 0;
    let mut caret_at_end = 0;
    let mut nothing_leaked = 0;
    let mut first_problem = None;
    for round in 0..ROUNDS {
        if !guard.ready() {
            return false;
        }
        guard.sink.take_text();
        let query = QUERIES[round % QUERIES.len()];
        send(chord);
        if wait_until(Duration::from_secs(1), || session.focused()) {
            spin_for(15);
            for unit in query.encode_utf16() {
                if !session.focused() {
                    return false;
                }
                send(&char_inputs(unit));
                spin_for(15);
            }
            sleep(80);
            let got = read_text(session.edit);
            let units = got.encode_utf16().count();
            let mut selection = 0usize;
            unsafe {
                let _ = SendMessageTimeoutW(session.edit, EM_GETSEL, WPARAM(0), LPARAM(0), SMTO_ABORTIFHUNG, 500, Some(&mut selection));
            }
            let (start, end) = (selection & 0xFFFF, (selection >> 16) & 0xFFFF);
            if got == query {
                text_intact += 1;
            } else if first_problem.is_none() {
                first_problem = Some(format!("typed {query:?}, the box held {got:?}"));
            }
            if start == units && end == units {
                caret_at_end += 1;
            } else if first_problem.is_none() {
                first_problem = Some(format!("after {query:?} the selection was {start}..{end}, not the end at {units}"));
            }
            session.hide();
        } else if first_problem.is_none() {
            first_problem = Some(format!("the launcher didn't take focus in round {round}"));
        }
        wait_until(Duration::from_millis(500), || unsafe { GetForegroundWindow() } == guard.sink.hwnd());
        sleep(30);
        if guard.sink.take_text().is_empty() {
            nothing_leaked += 1;
        }
    }
    let total = ROUNDS as u32;
    let updates = session.take_timings().queries.iter().filter(|q| q.0 > 0 && q.1 > 0).count();
    if updates == 0 {
        report.not_tested.push("typing during live result updates: no results ever arrived, so this proved nothing".to_string());
        return true;
    }
    report.add("typing during live results: result updates drawn", updates as f64, "count", false);
    report.check("typing during live results: no character dropped", text_intact, total);
    report.check("typing during live results: the caret stays at the end", caret_at_end, total);
    report.check("typing during live results: no key reaches the old window", nothing_leaked, total);
    if let Some(problem) = first_problem {
        report.not_tested.push(format!("typing during live results, first problem: {problem}"));
    }
    true
}

/// Asks Orbit to rebuild its app index, then watches the helper process come and go. No keys are
/// involved, so this is safe at any focus.
fn processes_while_indexing(report: &mut Report, session: &Session) {
    let Some(command) = orbit_core::Builtin::ALL.iter().position(|entry| entry.1 == "cmd:reindex") else {
        report.not_tested.push("process count while indexing: Orbit has no rebuild command".to_string());
        return;
    };
    let Some(idle) = process_count(session.pid) else {
        report.not_tested.push("process count while indexing: couldn't read the process list".to_string());
        return;
    };
    unsafe {
        let _ = PostMessageW(Some(session.main), WM_ORBIT_RUN_BUILTIN, WPARAM(command), LPARAM(0));
    }
    let started = Instant::now();
    let mut most = idle;
    let mut settled = false;
    while started.elapsed() < Duration::from_secs(30) {
        let count = process_count(session.pid).unwrap_or(idle);
        most = most.max(count);
        if most > idle && count <= idle {
            settled = true;
            break;
        }
        // Long enough to see a helper that takes time, and to give up on one that never starts.
        if most == idle && started.elapsed() > Duration::from_secs(5) {
            break;
        }
        sleep(10);
    }
    if most == idle {
        report.not_tested.push("process count while indexing: no helper process was seen within 5 s of asking for a rebuild".to_string());
        return;
    }
    report.add("processes: Orbit and its children while indexing (most seen)", most as f64, "count", false);
    if !settled {
        report.not_tested.push("index helper exit: it was still running after 30 s, so the run moved on".to_string());
    }
}

fn real_app(report: &mut Report, session: &Session, guard: &Guard, app: &str, process_name: &str, chord: &[INPUT]) {
    let running = |name: &str| -> Vec<u32> {
        let output = Command::new("tasklist").args(["/FI", &format!("IMAGENAME eq {name}"), "/FO", "CSV", "/NH"]).output();
        let text = output.map(|o| String::from_utf8_lossy(&o.stdout).to_string()).unwrap_or_default();
        text.lines().filter_map(|line| line.split("\",\"").nth(1).and_then(|p| p.parse().ok())).collect()
    };
    if !running(process_name).is_empty() {
        report.not_tested.push(format!("launch of {app}: it was already running, and the benchmark won't touch your copy"));
        return;
    }
    if !guard.ready() {
        return;
    }
    send(chord);
    if !wait_until(Duration::from_secs(1), || session.focused()) {
        report.check(format!("launch real app {app}"), 0, 1);
        return;
    }
    sleep(30);
    for unit in app.encode_utf16() {
        send(&char_inputs(unit));
    }
    sleep(120);
    if !session.focused() {
        return;
    }
    let t = Instant::now();
    send(&chord_inputs(&[VK_RETURN.0]));
    let mut found = Vec::new();
    while t.elapsed() < Duration::from_secs(8) && found.is_empty() {
        found = running(process_name);
        sleep(25);
    }
    report.check(format!("launch real app {app}"), !found.is_empty() as u32, 1);
    if !found.is_empty() {
        report.add(
            format!("launch: {app}, Enter to process present (tasklist polling, coarse)"),
            t.elapsed().as_secs_f64() * 1000.0,
            "ms",
            false,
        );
        sleep(800);
        let _ = Command::new("taskkill").args(["/IM", process_name]).output();
        // A store app keeps a suspended process after its window closes. This run started it (it refuses
        // to run if the app was open already), so it's this run's to remove.
        sleep(1500);
        let _ = Command::new("taskkill").args(["/F", "/IM", process_name]).output();
        sleep(300);
    }
    guard.sink.focus();
}

fn pressure(report: &mut Report, session: &Session, guard: &Guard, process: HANDLE, megabytes: usize, chord: &[INPUT]) {
    eprintln!("allocating {megabytes} MB to create memory pressure...");
    let mut block = vec![0u8; megabytes * 1024 * 1024];
    let touch = |block: &mut Vec<u8>| {
        for page in block.chunks_mut(4096) {
            page[0] = page[0].wrapping_add(1);
        }
    };
    touch(&mut block);
    let mut samples = Vec::new();
    for _ in 0..8 {
        if !guard.ready() {
            break;
        }
        touch(&mut block);
        let (t, _, t_focused) = timed_open(session, chord, Duration::from_secs(10));
        if t_focused != 0 {
            samples.push(session.ms(t, t_focused));
        }
        sleep(60);
        session.hide();
        sleep(200);
    }
    report.add("pressure: megabytes held and retouched by the benchmark", megabytes as f64, "MB", false);
    report.add_timings("pressure: key injected to foreground", &samples, false);
    touch(&mut block);
    let under = memory_of(process);
    report.add("pressure: system memory load while held", memory_status().dwMemoryLoad as f64, "%", false);
    report.add("memory under pressure: private", under.private_kb, "KB", false);
    report.add("memory under pressure: working set", under.working_set_kb, "KB", false);
    drop(block);
    sleep(1000);
    let after = memory_of(process);
    report.add("memory after pressure ended: private", after.private_kb, "KB", false);
    report.add("memory after pressure ended: working set", after.working_set_kb, "KB", false);
}

fn finish(report: &mut Report, mut session: Session) {
    // A clean exit, so the shutdown path is exercised as well.
    let start = Instant::now();
    unsafe {
        let _ = PostMessageW(Some(session.main), WM_ORBIT_RUN_BUILTIN, QUIT, LPARAM(0));
    }
    let mut clean = false;
    while start.elapsed() < Duration::from_secs(5) {
        if let Ok(Some(status)) = session.child.try_wait() {
            clean = status.success();
            break;
        }
        sleep(5);
    }
    if session.child.try_wait().ok().flatten().is_none() {
        let _ = session.child.kill();
    }
    report.check("quit command exits cleanly within 5 s", clean as u32, 1);
    report.add("shutdown: quit to process gone", start.elapsed().as_secs_f64() * 1000.0, "ms", false);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn process_tree_counts_children_and_their_children() {
        let processes = [(10, 1), (20, 10), (30, 20), (40, 1), (50, 40)];
        assert_eq!(count_tree(&processes, 10, |_, _| true), 3);
        assert_eq!(count_tree(&processes, 99, |_, _| true), 1);
    }

    #[test]
    fn process_tree_ignores_stale_parents_and_loops() {
        // 20 lists 10 as its parent but started before it, so that was an earlier owner of the id.
        let processes = [(10, 1), (20, 10), (30, 10)];
        assert_eq!(count_tree(&processes, 10, |child, _| child != 20), 2);
        let looped = [(10, 20), (20, 10)];
        assert_eq!(count_tree(&looped, 10, |_, _| true), 2);
    }

    #[test]
    fn search_queries_vary_and_are_never_empty() {
        let queries: Vec<String> = (0..500).map(search_query).collect();
        assert!(queries.iter().all(|query| query.len() >= 2));
        let distinct: std::collections::HashSet<&String> = queries.iter().collect();
        assert!(distinct.len() > 50, "only {} distinct queries", distinct.len());
    }

    #[test]
    fn drive_root_needs_a_drive_letter() {
        assert_eq!(drive_root(Path::new(r"C:\Temp\x")).as_deref(), Some(r"C:\"));
        assert_eq!(drive_root(Path::new(r"\\server\share")), None);
    }
}
