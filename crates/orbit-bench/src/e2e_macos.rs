//! macOS end-to-end driver. Runs the real Orbit binary with a private data directory and measures
//! it from outside. Timestamps are `mach_absolute_time` ticks, comparable across processes, so
//! Orbit's own CSV lines up with the moment the driver acted.
//!
//! Keys are only injected when all of these hold: the session isn't locked, the system allows this
//! process to post events, and the driver's own sink window or Orbit holds the keyboard. Anything
//! else means the owner did something, and the run stops rather than type into their window. A
//! locked screen matters most: its password field would receive the keys.
//!
//! With no injection, Orbit is driven through Darwin notifications, which it listens for only when
//! given `--bench-log`. Those numbers exclude the OS key path, and are labelled.

use crate::E2eOptions;
use crate::report::Report;
use std::collections::HashMap;
use std::ffi::{CString, c_char, c_int, c_void};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

const HOTKEY_TEXT: &str = "Ctrl+Alt+Shift+F13";
const KEY_F13: u16 = 105;
const KEY_F19: u16 = 80;
const KEY_K: u16 = 40;
const KEY_J: u16 = 38;
const KEY_ESCAPE: u16 = 53;
const KEY_RETURN: u16 = 36;
const KEY_CONTROL: u16 = 59;
const KEY_OPTION: u16 = 58;
const KEY_SHIFT: u16 = 56;
const FLAG_SHIFT: u64 = 0x20000;
const FLAG_CONTROL: u64 = 0x40000;
const FLAG_OPTION: u64 = 0x80000;
/// Function keys carry this on real keyboards. Hotkey matching ignores an F-key without it.
const FLAG_SECONDARY_FN: u64 = 0x800000;

#[repr(C)]
#[derive(Default, Clone, Copy)]
struct Timebase {
    numer: u32,
    denom: u32,
}

/// `rusage_info_v4` from <sys/resource.h>.
#[repr(C)]
#[derive(Clone, Copy)]
struct RusageV4 {
    uuid: [u8; 16],
    user_time: u64,
    system_time: u64,
    pkg_idle_wkups: u64,
    interrupt_wkups: u64,
    pageins: u64,
    wired_size: u64,
    resident_size: u64,
    phys_footprint: u64,
    proc_start_abstime: u64,
    proc_exit_abstime: u64,
    child_user_time: u64,
    child_system_time: u64,
    child_pkg_idle_wkups: u64,
    child_interrupt_wkups: u64,
    child_pageins: u64,
    child_elapsed_abstime: u64,
    diskio_bytesread: u64,
    diskio_byteswritten: u64,
    cpu_time_qos: [u64; 7],
    billed_system_time: u64,
    serviced_system_time: u64,
    logical_writes: u64,
    lifetime_max_phys_footprint: u64,
    instructions: u64,
    cycles: u64,
    billed_energy: u64,
    serviced_energy: u64,
    interval_max_phys_footprint: u64,
    runnable_time: u64,
}

const RUSAGE_INFO_V4: c_int = 4;
const PROC_PIDTASKINFO: c_int = 4;

/// `proc_taskinfo` from <sys/proc_info.h>.
#[repr(C)]
#[derive(Default, Clone, Copy)]
struct TaskInfo {
    virtual_size: u64,
    resident_size: u64,
    total_user: u64,
    total_system: u64,
    threads_user: u64,
    threads_system: u64,
    policy: i32,
    faults: i32,
    pageins: i32,
    cow_faults: i32,
    messages_sent: i32,
    messages_received: i32,
    syscalls_mach: i32,
    syscalls_unix: i32,
    csw: i32,
    threadnum: i32,
    numrunning: i32,
    priority: i32,
}

/// `rusage` from <sys/resource.h>, enough to read peak RSS from `wait4`.
#[repr(C)]
#[derive(Default)]
struct Rusage {
    utime: [i64; 2],
    stime: [i64; 2],
    maxrss: i64,
    rest: [i64; 13],
}

unsafe extern "C" {
    fn mach_absolute_time() -> u64;
    fn mach_timebase_info(info: *mut Timebase) -> c_int;
    fn proc_pid_rusage(pid: c_int, flavor: c_int, buffer: *mut c_void) -> c_int;
    fn proc_pidinfo(pid: c_int, flavor: c_int, arg: u64, buffer: *mut c_void, size: c_int) -> c_int;
    fn wait4(pid: c_int, status: *mut c_int, options: c_int, usage: *mut Rusage) -> c_int;
    fn notify_post(name: *const c_char) -> u32;
    fn notify_register_check(name: *const c_char, token: *mut c_int) -> u32;
    fn notify_set_state(token: c_int, state: u64) -> u32;
    fn sysctlbyname(name: *const c_char, old: *mut c_void, oldlen: *mut usize, new: *const c_void, newlen: usize) -> c_int;
}

#[link(name = "CoreFoundation", kind = "framework")]
unsafe extern "C" {
    fn CFRelease(value: *const c_void);
    fn CFStringCreateWithCString(alloc: *const c_void, text: *const c_char, encoding: u32) -> *const c_void;
    fn CFDictionaryGetValue(dict: *const c_void, key: *const c_void) -> *const c_void;
    fn CFBooleanGetValue(value: *const c_void) -> bool;
    fn CFNumberGetValue(number: *const c_void, kind: i64, out: *mut c_void) -> bool;
    fn CFArrayGetCount(array: *const c_void) -> i64;
    fn CFArrayGetValueAtIndex(array: *const c_void, index: i64) -> *const c_void;
}

#[link(name = "CoreGraphics", kind = "framework")]
unsafe extern "C" {
    fn CGPreflightPostEventAccess() -> bool;
    fn CGPreflightListenEventAccess() -> bool;
    fn CGEventCreateKeyboardEvent(source: *const c_void, key: u16, down: bool) -> *mut c_void;
    fn CGEventSetFlags(event: *mut c_void, flags: u64);
    fn CGEventKeyboardSetUnicodeString(event: *mut c_void, length: usize, text: *const u16);
    fn CGEventPost(tap: u32, event: *mut c_void);
    fn CGEventSourceCreate(state: i32) -> *const c_void;
    fn CGWindowListCopyWindowInfo(option: u32, relative: u32) -> *const c_void;
    fn CGSessionCopyCurrentDictionary() -> *const c_void;
}

const CG_HID_EVENT_TAP: u32 = 0;
const CG_SESSION_EVENT_TAP: u32 = 1;
const CG_HID_SYSTEM_STATE: i32 = 1;
const WINDOW_LIST_ON_SCREEN: u32 = 1;
const UTF8: u32 = 0x0800_0100;
const NUMBER_SINT64: i64 = 4;

fn ticks() -> i64 {
    unsafe { mach_absolute_time() as i64 }
}

fn tick_ms() -> f64 {
    let mut info = Timebase::default();
    unsafe { mach_timebase_info(&mut info) };
    info.numer as f64 / info.denom.max(1) as f64 / 1e6
}

fn sleep(ms: u64) {
    std::thread::sleep(Duration::from_millis(ms));
}

fn wait_until(limit: Duration, mut condition: impl FnMut() -> bool) -> bool {
    let start = Instant::now();
    while start.elapsed() < limit {
        if condition() {
            return true;
        }
        std::thread::sleep(Duration::from_micros(200));
    }
    condition()
}

fn cf_string(text: &str) -> *const c_void {
    let c = CString::new(text).unwrap_or_default();
    unsafe { CFStringCreateWithCString(std::ptr::null(), c.as_ptr(), UTF8) }
}

/// True if the login session's screen is locked. Unknown counts as locked: injection must not run.
pub fn screen_locked() -> bool {
    unsafe {
        let dict = CGSessionCopyCurrentDictionary();
        if dict.is_null() {
            return true;
        }
        let key = cf_string("CGSSessionScreenIsLocked");
        let value = CFDictionaryGetValue(dict, key);
        let locked = !value.is_null() && CFBooleanGetValue(value);
        CFRelease(key);
        CFRelease(dict);
        locked
    }
}

// --- Observing Orbit from outside ---------------------------------------------------------

/// Owner PID of on-screen windows, front to back. Needs no grant, since it reads no window titles.
fn on_screen_owners() -> Vec<(i64, i64)> {
    let mut out = Vec::new();
    unsafe {
        let list = CGWindowListCopyWindowInfo(WINDOW_LIST_ON_SCREEN, 0);
        if list.is_null() {
            return out;
        }
        let pid_key = cf_string("kCGWindowOwnerPID");
        let layer_key = cf_string("kCGWindowLayer");
        for i in 0..CFArrayGetCount(list) {
            let info = CFArrayGetValueAtIndex(list, i);
            let (mut pid, mut layer) = (0i64, 0i64);
            let p = CFDictionaryGetValue(info, pid_key);
            let l = CFDictionaryGetValue(info, layer_key);
            if !p.is_null() {
                CFNumberGetValue(p, NUMBER_SINT64, &mut pid as *mut i64 as *mut c_void);
            }
            if !l.is_null() {
                CFNumberGetValue(l, NUMBER_SINT64, &mut layer as *mut i64 as *mut c_void);
            }
            out.push((pid, layer));
        }
        CFRelease(pid_key);
        CFRelease(layer_key);
        CFRelease(list);
    }
    out
}

fn panel_on_screen(pid: u32) -> bool {
    // The panel floats above layer 0. Orbit's menu bar icon has its own high layer, so this checks
    // for the panel's level specifically.
    on_screen_owners().iter().any(|(owner, layer)| *owner == pid as i64 && *layer == 8)
}

fn frontmost_normal_owner() -> Option<i64> {
    on_screen_owners().iter().find(|(_, layer)| *layer == 0).map(|(owner, _)| *owner)
}

#[derive(Clone, Copy)]
struct Usage {
    cpu_ms: f64,
    idle_wakeups: u64,
    interrupt_wakeups: u64,
    footprint: u64,
    resident: u64,
    peak_footprint: u64,
    disk_written: u64,
    logical_writes: u64,
}

fn usage(pid: u32) -> Option<Usage> {
    unsafe {
        let mut info = std::mem::zeroed::<RusageV4>();
        if proc_pid_rusage(pid as c_int, RUSAGE_INFO_V4, &mut info as *mut _ as *mut c_void) != 0 {
            return None;
        }
        // On Apple Silicon these two fields are in mach ticks, not nanoseconds.
        let cpu_ms = (info.user_time + info.system_time) as f64 * tick_ms();
        Some(Usage {
            cpu_ms,
            idle_wakeups: info.pkg_idle_wkups,
            interrupt_wakeups: info.interrupt_wkups,
            footprint: info.phys_footprint,
            resident: info.resident_size,
            peak_footprint: info.lifetime_max_phys_footprint,
            disk_written: info.diskio_byteswritten,
            logical_writes: info.logical_writes,
        })
    }
}

fn task_info(pid: u32) -> Option<TaskInfo> {
    unsafe {
        let mut info = TaskInfo::default();
        let size = std::mem::size_of::<TaskInfo>() as c_int;
        (proc_pidinfo(pid as c_int, PROC_PIDTASKINFO, 0, &mut info as *mut _ as *mut c_void, size) == size).then_some(info)
    }
}

fn sysctl_string(name: &str) -> String {
    let c = CString::new(name).unwrap_or_default();
    let mut buffer = vec![0u8; 256];
    let mut size = buffer.len();
    let ok = unsafe { sysctlbyname(c.as_ptr(), buffer.as_mut_ptr() as *mut c_void, &mut size, std::ptr::null(), 0) };
    if ok != 0 {
        return "unknown".into();
    }
    buffer.truncate(size.saturating_sub(1));
    String::from_utf8_lossy(&buffer).into_owned()
}

fn sysctl_u64(name: &str) -> u64 {
    let c = CString::new(name).unwrap_or_default();
    let mut value = 0u64;
    let mut size = 8usize;
    unsafe { sysctlbyname(c.as_ptr(), &mut value as *mut u64 as *mut c_void, &mut size, std::ptr::null(), 0) };
    value
}

fn command_text(program: &str, args: &[&str]) -> String {
    Command::new(program).args(args).output().map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string()).unwrap_or_default()
}

pub fn describe_environment(report: &mut Report) {
    let e = &mut report.environment;
    e.insert("cpu".into(), sysctl_string("machdep.cpu.brand_string"));
    e.insert("model".into(), sysctl_string("hw.model"));
    e.insert("logical processors".into(), std::thread::available_parallelism().map_or(0, |n| n.get()).to_string());
    e.insert("ram".into(), format!("{:.0} GB", sysctl_u64("hw.memsize") as f64 / 1_073_741_824.0));
    e.insert("macos".into(), format!("{} ({})", command_text("/usr/bin/sw_vers", &["-productVersion"]), command_text("/usr/bin/sw_vers", &["-buildVersion"])));
    e.insert("memory free at start".into(), format!("{}%", sysctl_u64("kern.memorystatus_level") & 0xffff_ffff));
    let power = command_text("/usr/bin/pmset", &["-g", "batt"]);
    e.insert("power".into(), power.lines().next().unwrap_or("unknown").replace("Now drawing from ", "").replace('\'', ""));
    let load = command_text("/usr/sbin/sysctl", &["-n", "vm.loadavg"]);
    e.insert("load average at start".into(), load.trim_matches(|c| c == '{' || c == '}' || c == ' ').to_string());
    e.insert("screen locked at start".into(), if screen_locked() { "yes" } else { "no" }.into());
    e.insert(
        "driver's event permissions".into(),
        format!(
            "post {}, listen {}. When run from a terminal these are that terminal's grants, not Orbit's",
            unsafe { CGPreflightPostEventAccess() },
            unsafe { CGPreflightListenEventAccess() }
        ),
    );
}

// --- Talking to processes -----------------------------------------------------------------

#[derive(Default)]
struct Notifier {
    tokens: HashMap<String, c_int>,
}

impl Notifier {
    fn post(&self, name: &str) {
        let c = CString::new(name).unwrap_or_default();
        unsafe { notify_post(c.as_ptr()) };
    }

    fn post_value(&mut self, name: &str, value: u64) {
        let token = *self.tokens.entry(name.to_string()).or_insert_with(|| {
            let c = CString::new(name).unwrap_or_default();
            let mut token = 0;
            unsafe { notify_register_check(c.as_ptr(), &mut token) };
            token
        });
        unsafe { notify_set_state(token, value) };
        self.post(name);
    }
}

struct Sink {
    child: Child,
    report: PathBuf,
    sequence: u64,
    notifier: Notifier,
}

#[derive(Debug, Default, Clone)]
struct SinkState {
    active: bool,
    key: bool,
    text: String,
}

impl Sink {
    fn start(binary: &Path, dir: &Path) -> Option<Sink> {
        let report = dir.join("sink.txt");
        let _ = std::fs::remove_file(&report);
        let child = Command::new(binary).arg(&report).stdout(Stdio::null()).stderr(Stdio::null()).spawn().ok()?;
        let mut sink = Sink { child, report, sequence: 0, notifier: Notifier::default() };
        if sink.wait_for_report(Duration::from_secs(3)).is_none() {
            sink.stop();
            return None;
        }
        Some(sink)
    }

    fn name(&self, command: &str) -> String {
        format!("com.orbit.benchsink.{}.{command}", self.child.id())
    }

    fn wait_for_report(&mut self, limit: Duration) -> Option<SinkState> {
        let start = Instant::now();
        while start.elapsed() < limit {
            if let Ok(text) = std::fs::read_to_string(&self.report) {
                let parts: Vec<&str> = text.trim_end_matches('\n').splitn(4, ',').collect();
                if parts.len() == 4 {
                    let sequence: u64 = parts[0].parse().unwrap_or(0);
                    if sequence > self.sequence {
                        self.sequence = sequence;
                        return Some(SinkState { active: parts[1] == "true", key: parts[2] == "true", text: parts[3].to_string() });
                    }
                }
            }
            sleep(2);
        }
        None
    }

    fn state(&mut self) -> Option<SinkState> {
        self.notifier.post(&self.name("report"));
        self.wait_for_report(Duration::from_millis(800))
    }

    fn focus(&mut self) -> bool {
        self.notifier.post(&self.name("focus"));
        sleep(60);
        self.state().is_some_and(|s| s.active && s.key)
    }

    fn clear(&mut self) {
        self.notifier.post(&self.name("clear"));
        sleep(20);
    }

    fn stop(&mut self) {
        self.notifier.post(&self.name("quit"));
        sleep(100);
        if self.child.try_wait().ok().flatten().is_none() {
            let _ = self.child.kill();
        }
        let _ = self.child.wait();
    }
}

// --- Injection ----------------------------------------------------------------------------

/// Real modifier presses, not just flags: hotkey matching reads the hardware modifier state.
fn post_key(source: *const c_void, tap: u32, key: u16, down: bool, flags: u64) {
    unsafe {
        let event = CGEventCreateKeyboardEvent(source, key, down);
        if event.is_null() {
            return;
        }
        CGEventSetFlags(event, flags);
        CGEventPost(tap, event);
        CFRelease(event);
    }
}

fn post_chord(source: *const c_void) {
    let all = FLAG_CONTROL | FLAG_OPTION | FLAG_SHIFT;
    post_key(source, CG_HID_EVENT_TAP, KEY_CONTROL, true, FLAG_CONTROL);
    post_key(source, CG_HID_EVENT_TAP, KEY_OPTION, true, FLAG_CONTROL | FLAG_OPTION);
    post_key(source, CG_HID_EVENT_TAP, KEY_SHIFT, true, all);
    post_key(source, CG_HID_EVENT_TAP, KEY_F13, true, all | FLAG_SECONDARY_FN);
    post_key(source, CG_HID_EVENT_TAP, KEY_F13, false, all | FLAG_SECONDARY_FN);
    post_key(source, CG_HID_EVENT_TAP, KEY_SHIFT, false, FLAG_CONTROL | FLAG_OPTION);
    post_key(source, CG_HID_EVENT_TAP, KEY_OPTION, false, FLAG_CONTROL);
    post_key(source, CG_HID_EVENT_TAP, KEY_CONTROL, false, 0);
}

fn post_text(source: *const c_void, text: &str) {
    for unit in text.encode_utf16() {
        for down in [true, false] {
            unsafe {
                let event = CGEventCreateKeyboardEvent(source, 0, down);
                if event.is_null() {
                    continue;
                }
                CGEventKeyboardSetUnicodeString(event, 1, &unit);
                CGEventPost(CG_SESSION_EVENT_TAP, event);
                CFRelease(event);
            }
        }
    }
}

// --- One running Orbit --------------------------------------------------------------------

struct Session {
    child: Child,
    pid: u32,
    log: PathBuf,
    log_read: usize,
    notifier: Notifier,
    tick_ms: f64,
    held: Timings,
}

#[derive(Debug, Default)]
struct Timings {
    shows: Vec<[i64; 5]>,
    queries: Vec<(usize, usize, i64, i64, i64)>,
    actions: Vec<(i64, i64)>,
    hyper: Vec<(i64, i64)>,
    hyper_dispatches: usize,
    memory: Option<(f64, f64, f64)>,
    state: Option<HashMap<String, String>>,
    reports: Vec<String>,
    clip_decisions: Vec<String>,
    counts: Option<(i64, i64, i64)>,
    ready: Option<i64>,
    index_done: usize,
}

impl Session {
    fn start(orbit: &Path, dir: &Path, log: &Path, extra: &[&str]) -> Option<Session> {
        let _ = std::fs::remove_file(log);
        let child = Command::new(orbit)
            .arg("--data-dir")
            .arg(dir)
            .args(["--hotkey", HOTKEY_TEXT, "--no-open", "--bench-log"])
            .arg(log)
            .args(extra)
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .ok()?;
        let pid = child.id();
        Some(Session { child, pid, log: log.to_path_buf(), log_read: 0, notifier: Notifier::default(), tick_ms: tick_ms(), held: Timings::default() })
    }

    fn post(&self, command: &str) {
        self.notifier.post(&format!("com.orbit.bench.{}.{command}", self.pid));
    }

    fn type_char(&mut self, c: char) {
        let name = format!("com.orbit.bench.{}.type", self.pid);
        self.notifier.post_value(&name, c as u64);
        // Darwin notifications carry one state value per name, and Orbit reads it when the handler
        // runs. The gap must outlast one drawn frame, or characters repeat and get lost.
        sleep(12);
    }

    fn ms(&self, from: i64, to: i64) -> f64 {
        (to - from) as f64 * self.tick_ms
    }

    /// Timing lines since the last call, including any that `state` has already read.
    fn take_timings(&mut self) -> Timings {
        let mut out = self.read_log();
        let held = std::mem::take(&mut self.held);
        out.shows.splice(0..0, held.shows);
        out.queries.splice(0..0, held.queries);
        out.actions.splice(0..0, held.actions);
        out.hyper.splice(0..0, held.hyper);
        out.hyper_dispatches += held.hyper_dispatches;
        out
    }

    fn read_log(&mut self) -> Timings {
        self.post("flush");
        sleep(40);
        let text = std::fs::read_to_string(&self.log).unwrap_or_default();
        let fresh = &text[self.log_read.min(text.len())..];
        let mut out = Timings::default();
        for line in fresh.lines() {
            let parts: Vec<&str> = line.split(',').collect();
            let int = |i: usize| parts.get(i).and_then(|p| p.parse::<i64>().ok()).unwrap_or(0);
            let float = |i: usize| parts.get(i).and_then(|p| p.parse::<f64>().ok()).unwrap_or(0.0);
            match parts.first().copied() {
                Some("show") => out.shows.push([int(2), int(3), int(4), int(5), int(6)]),
                Some("query") => out.queries.push((int(1) as usize, int(2) as usize, int(3), int(4), int(5))),
                Some("action") => out.actions.push((int(2), int(3))),
                Some("hyper") => out.hyper.push((int(1), int(2))),
                Some("hyper-dispatch") => out.hyper_dispatches += 1,
                Some("memory") => out.memory = Some((float(1), float(2), float(3))),
                Some("ready") => out.ready = Some(int(1)),
                Some("index-done") => out.index_done += 1,
                Some("counts") => out.counts = Some((int(1), int(2), int(3))),
                Some("clip-decision") => out.clip_decisions.push(parts.get(1).unwrap_or(&"").to_string()),
                Some("report") => out.reports.push(line["report,".len()..].to_string()),
                Some("state") => {
                    let mut map = HashMap::new();
                    for part in &parts[1..] {
                        if let Some((k, v)) = part.split_once('=') {
                            map.insert(k.to_string(), v.to_string());
                        }
                    }
                    out.state = Some(map);
                }
                _ => {}
            }
        }
        self.log_read = text.len();
        out
    }

    fn wait_ready(&mut self, limit: Duration) -> Option<i64> {
        let start = Instant::now();
        while start.elapsed() < limit {
            if let Ok(text) = std::fs::read_to_string(&self.log) {
                if let Some(line) = text.lines().find(|l| l.starts_with("ready,")) {
                    return line[6..].parse().ok();
                }
            }
            sleep(1);
        }
        None
    }

    fn state(&mut self) -> HashMap<String, String> {
        let timings = self.read_log();
        self.held.shows.extend(timings.shows);
        self.held.queries.extend(timings.queries);
        self.held.actions.extend(timings.actions);
        self.held.hyper.extend(timings.hyper);
        self.held.hyper_dispatches += timings.hyper_dispatches;
        timings.state.unwrap_or_default()
    }

    fn is(&mut self, key: &str) -> bool {
        self.state().get(key).is_some_and(|v| v == "true")
    }

    fn quit(mut self, report: Option<&mut Report>) {
        let start = Instant::now();
        self.post("quit");
        let mut clean = false;
        while start.elapsed() < Duration::from_secs(5) {
            if let Ok(Some(status)) = self.child.try_wait() {
                clean = status.success();
                break;
            }
            sleep(2);
        }
        if self.child.try_wait().ok().flatten().is_none() {
            let _ = self.child.kill();
            let _ = self.child.wait();
        }
        if let Some(report) = report {
            report.check("quit exits cleanly within 5 s", clean as u32, 1);
            report.add("shutdown: quit to process gone", start.elapsed().as_secs_f64() * 1000.0, "ms", false);
        }
    }
}

fn snapshot(report: &mut Report, label: &str, session: &mut Session, gated: bool) {
    let timings = session.take_timings();
    if let Some((footprint, resident, peak)) = timings.memory {
        report.add(format!("memory {label}: footprint (task_info phys_footprint)"), footprint / 1024.0, "KB", gated);
        report.add(format!("memory {label}: resident (task_info resident_size, includes shared frameworks)"), resident / 1024.0, "KB", gated);
        report.add(format!("memory {label}: peak footprint so far"), peak / 1024.0, "KB", false);
    } else {
        report.not_tested.push(format!("memory {label}: Orbit didn't report"));
    }
}

fn write_settings(dir: &Path, probe: &Path, probe_file: &Path, extra: serde_json::Value) {
    let mut settings = serde_json::json!({
        "schema_version": 1,
        "onboarded": true,
        "custom_commands": [{
            "id": "bench-probe",
            "name": "Orbit Bench Probe",
            "program": probe.to_string_lossy(),
            "arguments": format!("\"{}\"", probe_file.display()),
            "working_dir": "",
            "alias": "obp",
        }],
    });
    if let (Some(base), Some(more)) = (settings.as_object_mut(), extra.as_object()) {
        for (key, value) in more {
            base.insert(key.clone(), value.clone());
        }
    }
    std::fs::write(dir.join("settings.json"), settings.to_string()).expect("write settings");
}

pub fn run(report: &mut Report, options: &E2eOptions) {
    let dir = options.work_dir.clone();
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("bench data dir");
    let log = dir.join("timings.csv");
    let probe_file = dir.join("probe.txt");
    let exe_dir = std::env::current_exe().ok().and_then(|p| p.parent().map(Path::to_path_buf)).unwrap_or_default();
    let probe = exe_dir.join("orbit-bench-probe");
    write_settings(&dir, &probe, &probe_file, serde_json::json!({}));

    if !options.orbit.exists() {
        report.not_tested.push(format!("e2e: {} does not exist; build it first", options.orbit.display()));
        return;
    }
    report.add("binary size: Orbit", std::fs::metadata(&options.orbit).map_or(0.0, |m| m.len() as f64) / 1024.0, "KB", true);

    if options.records > 0 {
        eprintln!("seeding {} notes and clips...", options.records);
        let store = orbit_data::Store::open(&dir, true).expect("store");
        crate::data_bench::seed(&store, options.records, options.records);
        drop(store);
        report.environment.insert("e2e data set".into(), format!("{} notes, {} clips", options.records, options.records));
    } else {
        report.environment.insert("e2e data set".into(), "empty".into());
    }

    // App scan, in a short-lived process so its peak can be read cleanly.
    let start = Instant::now();
    if let Ok(child) = Command::new(&options.orbit).args(["--index-apps", "--data-dir"]).arg(&dir).spawn() {
        let mut status = 0;
        let mut rusage = Rusage::default();
        unsafe { wait4(child.id() as c_int, &mut status, 0, &mut rusage) };
        report.add("app index: scan in its own process", start.elapsed().as_secs_f64() * 1000.0, "ms", false);
        report.add("app index: that process's peak RSS (wait4 ru_maxrss)", rusage.maxrss as f64 / 1024.0, "KB", false);
    }
    let apps = std::fs::read_to_string(dir.join("apps.json")).ok().and_then(|t| serde_json::from_str::<serde_json::Value>(&t).ok()).and_then(|v| v.as_array().map(Vec::len)).unwrap_or(0);
    report.add("app index: apps found", apps as f64, "count", false);

    // Process start. The app's files are already in the disk cache, so this isn't first boot.
    let mut starts = Vec::new();
    for _ in 0..5 {
        let t = ticks();
        let Some(mut session) = Session::start(&options.orbit, &dir, &log, &["--no-reindex"]) else { continue };
        if let Some(ready) = session.wait_ready(Duration::from_secs(10)) {
            starts.push(session.ms(t, ready));
        }
        sleep(150);
        session.quit(None);
        sleep(120);
    }
    report.add_timings("startup: process spawned to shortcut registered and ready (warm disk cache)", &starts, true);

    let Some(mut session) = Session::start(&options.orbit, &dir, &log, &["--no-reindex", "--no-trim", "--export-dir", dir.join("export").to_str().unwrap_or("")]) else {
        report.not_tested.push("e2e: Orbit wouldn't start".into());
        return;
    };
    if session.wait_ready(Duration::from_secs(10)).is_none() {
        report.not_tested.push("e2e: Orbit never reported ready".into());
        session.quit(None);
        return;
    }
    let state = session.state();
    let registered = state.get("hotkey").is_some_and(|h| h == HOTKEY_TEXT);
    report.check("the test chord registers with RegisterEventHotKey", registered as u32, 1);
    sleep(1500);
    snapshot(report, "at idle after start", &mut session, true);
    if let Some(info) = task_info(session.pid) {
        report.add("threads at idle after start", info.threadnum as f64, "count", true);
    }

    notification_suite(report, options, &mut session);

    let locked = screen_locked();
    let can_post = unsafe { CGPreflightPostEventAccess() };
    if locked {
        for item in KEY_TESTS {
            report.not_tested.push(format!("{item}: needs an unlocked screen. The driver refuses to inject keys into a locked session"));
        }
    } else if !can_post {
        for item in KEY_TESTS {
            report.not_tested.push(format!("{item}: needs the Accessibility grant, for the program running orbit-bench"));
        }
    } else {
        match Sink::start(&exe_dir_for_sink(options), &dir) {
            Some(mut sink) => {
                if !sink.focus() {
                    report.not_tested.push("key tests: macOS wouldn't give the benchmark window focus".into());
                } else if !key_suite(report, options, &mut session, &mut sink) {
                    report.not_tested.push("key tests stopped early because another window took focus, or the screen locked. Results above are partial.".into());
                }
                sink.stop();
            }
            None => report.not_tested.push("key tests: OrbitBenchSink didn't start. Pass --orbit with a build from scripts/macos/build_app.sh".into()),
        }
    }

    idle_suite(report, options, &mut session);
    session.quit(Some(report));

    hyper_suite(report, options, &dir, &probe, &probe_file);
    report.not_tested.push("true reopening under memory pressure: off by default because it slows the whole machine. Use --pressure-mb N. Not run on the owner's machine.".into());
    if options.pressure_mb > 0 {
        report.not_tested.push("--pressure-mb was given but the macOS driver doesn't implement it yet".into());
    }
    report.not_tested.push("after 100 note opens: each opens and activates a window, too disruptive for a shared machine".into());
}

const KEY_TESTS: &[&str] = &[
    "real hotkey to visible and to key window",
    "typing right after the hotkey: no lost or leaked keys",
    "typing latency through real key events",
    "focus restoration: keys reach the previous app after Esc",
    "Enter launches through a real key event",
    "launch of a real installed app",
    "rapid hotkey presses",
    "HyperKey chord through the event tap",
];

fn exe_dir_for_sink(options: &E2eOptions) -> PathBuf {
    // <out>/Orbit.app/Contents/MacOS/Orbit, with OrbitBenchSink beside the bundle.
    options.orbit.ancestors().nth(4).unwrap_or(Path::new(".")).join("OrbitBenchSink")
}

/// Everything that works with no key events. All numbers here exclude the OS key path.
fn notification_suite(report: &mut Report, options: &E2eOptions, session: &mut Session) {
    let _ = session.take_timings();
    let pid = session.pid;
    let mut to_visible = Vec::new();
    let mut opened = 0;
    let opens = options.opens.min(60);
    for i in 0..opens {
        let t = ticks();
        session.post("toggle");
        if wait_until(Duration::from_secs(2), || panel_on_screen(pid)) {
            to_visible.push(session.ms(t, ticks()));
            opened += 1;
        }
        sleep(60 + (i as u64 * 37) % 90);
        session.post("escape");
        wait_until(Duration::from_secs(1), || !panel_on_screen(pid));
        sleep(40 + (i as u64 * 53) % 120);
    }
    report.check("notification opens the panel (on screen per CGWindowList)", opened, opens as u32);
    report.add_timings("no key path: notification to on screen (CGWindowList polling)", &to_visible, false);
    let timings = session.take_timings();
    let shows: Vec<[i64; 5]> = timings.shows.into_iter().filter(|s| s[0] != 0).collect();
    let span = |a: usize, b: usize, session: &Session| -> Vec<f64> {
        shows.iter().filter(|s| s[a] != 0 && s[b] != 0).map(|s| session.ms(s[a], s[b])).collect()
    };
    report.add_timings("no key path: T0 callback to T1 frame prepared", &span(0, 1, session), false);
    report.add_timings("no key path: T0 to T2 ordered front and committed", &span(0, 2, session), false);
    report.add_timings("no key path: T0 to T2b window server reports visible", &span(0, 3, session), false);
    report.add_timings("no key path: T0 to T3 panel key and editing", &span(0, 4, session), false);
    snapshot(report, "after opens", session, true);

    // Search as you type, going through the field editor but not through a key event.
    let queries = ["calc", "settings", "note", "clip", "zzqq", "2*21", "help", "orbit bench", "chrome", "code"];
    for query in queries {
        session.post("toggle");
        wait_until(Duration::from_secs(1), || panel_on_screen(pid));
        sleep(30);
        for c in query.chars() {
            session.type_char(c);
            sleep(35);
        }
        sleep(60);
        session.post("escape");
        sleep(80);
    }
    let timings = session.take_timings();
    let typed: Vec<_> = timings.queries.iter().filter(|q| q.0 > 0).copied().collect();
    let expected: usize = queries.iter().map(|q| q.chars().count()).sum();
    report.check("every character reached the search box", typed.len().min(expected) as u32, expected as u32);
    report.add_timings("no key path: T4 query seen to T5 results built", &typed.iter().map(|q| session.ms(q.2, q.3)).collect::<Vec<_>>(), true);
    report.add_timings("no key path: T5 to T6 row view drawn", &typed.iter().map(|q| session.ms(q.3, q.4)).collect::<Vec<_>>(), true);
    report.add_timings("no key path: T4 to T6 total", &typed.iter().map(|q| session.ms(q.2, q.4)).collect::<Vec<_>>(), true);
    snapshot(report, "after typing", session, true);

    // The brief's memory point: after 500 searches.
    session.post("toggle");
    wait_until(Duration::from_secs(1), || panel_on_screen(pid));
    let words = "abcdefghijklmnopqrstuvwxyz";
    for (i, c) in words.chars().cycle().take(500).enumerate() {
        session.type_char(c);
        sleep(4);
        if i % 8 == 7 {
            session.post("escape");
            sleep(3);
            session.post("toggle");
            sleep(3);
        }
    }
    sleep(100);
    session.post("escape");
    sleep(100);
    snapshot(report, "after 500 searches", session, true);

    // Launching a harmless probe through the whole action path.
    let probe_file = options.work_dir.join("probe.txt");
    let mut launched = 0;
    let mut launch_ms = Vec::new();
    const LAUNCHES: u32 = 10;
    let _ = session.take_timings();
    for _ in 0..LAUNCHES {
        let _ = std::fs::remove_file(&probe_file);
        session.post("toggle");
        wait_until(Duration::from_secs(1), || panel_on_screen(pid));
        sleep(20);
        for c in "orbit bench probe".chars() {
            session.type_char(c);
            sleep(3);
        }
        sleep(60);
        let t = ticks();
        session.post("enter");
        let limit = Instant::now();
        while limit.elapsed() < Duration::from_secs(4) {
            if let Some(stamp) = std::fs::read_to_string(&probe_file).ok().and_then(|s| s.trim().parse::<i64>().ok()) {
                launch_ms.push(session.ms(t, stamp));
                launched += 1;
                break;
            }
            sleep(2);
        }
        sleep(100);
    }
    report.check("the selected custom command runs, without a shell", launched, LAUNCHES);
    report.add_timings("no key path: Enter to new process running", &launch_ms, false);
    let timings = session.take_timings();
    let dispatch: Vec<f64> = timings.actions.iter().map(|a| session.ms(a.0, a.1)).collect();
    report.add_timings("no key path: Enter received to action dispatched", &dispatch, true);
    snapshot(report, "after 10 launches", session, true);
}

/// Returns false if another window took focus or the screen locked, and the run had to stop.
fn key_suite(report: &mut Report, options: &E2eOptions, session: &mut Session, sink: &mut Sink) -> bool {
    let source = unsafe { CGEventSourceCreate(CG_HID_SYSTEM_STATE) };
    let result = key_suite_inner(report, options, session, sink, source);
    if !source.is_null() {
        unsafe { CFRelease(source) };
    }
    result
}

fn key_suite_inner(report: &mut Report, options: &E2eOptions, session: &mut Session, sink: &mut Sink, source: *const c_void) -> bool {
    let pid = session.pid;
    let sink_pid = sink.child.id() as i64;
    let safe = |sink_pid: i64| !screen_locked() && frontmost_normal_owner() == Some(sink_pid);
    let _ = session.take_timings();

    // --- Warm opens, through the real key path ------------------------------------------
    let mut to_visible = Vec::new();
    let mut injects = Vec::new();
    let mut opens_ok = 0;
    let mut key_ok = 0;
    let mut restored = 0;
    let mut hidden_ok = 0;
    let opens = options.opens as u32;
    for i in 0..opens {
        if !safe(sink_pid) {
            return false;
        }
        let t = ticks();
        post_chord(source);
        if wait_until(Duration::from_secs(2), || panel_on_screen(pid)) {
            to_visible.push(session.ms(t, ticks()));
            opens_ok += 1;
            injects.push(t);
        }
        sleep(60 + (i as u64 * 37) % 90);
        let is_key = session.is("key");
        if is_key {
            key_ok += 1;
        }
        // Esc only when Orbit holds the keyboard. Otherwise it would go to the window in front.
        if is_key {
            post_key(source, CG_HID_EVENT_TAP, KEY_ESCAPE, true, 0);
            post_key(source, CG_HID_EVENT_TAP, KEY_ESCAPE, false, 0);
        } else {
            session.post("escape");
        }
        if wait_until(Duration::from_secs(1), || !panel_on_screen(pid)) {
            hidden_ok += 1;
        }
        sleep(40);
        if sink.state().is_some_and(|s| s.active && s.key) {
            restored += 1;
        }
        sleep(40 + (i as u64 * 53) % 120);
    }
    report.check("hotkey opens the panel", opens_ok, opens);
    report.check("the panel holds the keyboard after the hotkey", key_ok, opens);
    report.check("Esc hides the panel", hidden_ok, opens);
    report.check("the previous app is active and its window key after Esc", restored, opens);
    report.add_timings("warm open: key injected to on screen (external, CGWindowList polling)", &to_visible, true);
    let timings = session.take_timings();
    let shows: Vec<[i64; 5]> = timings.shows.iter().copied().filter(|s| s[0] != 0).collect();
    let span = |a: usize, b: usize| -> Vec<f64> {
        shows.iter().filter(|s| s[a] != 0 && s[b] != 0).map(|s| session.ms(s[a], s[b])).collect()
    };
    report.add_timings("warm open: T0 hotkey callback to T1 frame prepared", &span(0, 1), true);
    report.add_timings("warm open: T0 to T2 ordered front and committed", &span(0, 2), true);
    report.add_timings("warm open: T0 to T2b window server reports visible", &span(0, 3), false);
    report.add_timings("warm open: T0 to T3 panel key and editing", &span(0, 4), true);
    if shows.len() == injects.len() {
        let delivery: Vec<f64> = shows.iter().zip(&injects).map(|(s, t)| session.ms(*t, s[0])).collect();
        report.add_timings("warm open: key injected to T0 (OS delivery)", &delivery, false);
    }
    snapshot(report, "after warm opens", session, true);

    // --- Typing immediately after the shortcut ------------------------------------------
    let typed = "immediate";
    const ROUNDS: u32 = 10;
    for gap_ms in [0u64, 15, 30] {
        let mut none_lost = 0;
        let mut none_leaked = 0;
        for _ in 0..ROUNDS {
            if !safe(sink_pid) {
                return false;
            }
            sink.clear();
            post_chord(source);
            if gap_ms > 0 {
                sleep(gap_ms);
            }
            post_text(source, typed);
            sleep(150);
            let state = session.state();
            if state.get("query").is_some_and(|q| q == &typed.len().to_string()) {
                none_lost += 1;
            }
            session.post("escape");
            wait_until(Duration::from_secs(1), || !panel_on_screen(pid));
            sleep(40);
            if sink.state().is_some_and(|s| s.text.is_empty()) {
                none_leaked += 1;
            }
            sleep(40);
        }
        if gap_ms >= 15 {
            report.check(format!("typing {gap_ms} ms after the shortcut: no keys lost"), none_lost, ROUNDS);
            report.check(format!("typing {gap_ms} ms after the shortcut: no keys reach the old window"), none_leaked, ROUNDS);
        } else {
            report.add(format!("typing {gap_ms} ms after the shortcut: rounds with all keys captured, of {ROUNDS}"), none_lost as f64, "count", false);
            report.add(format!("typing {gap_ms} ms after the shortcut: rounds with no leaked keys, of {ROUNDS}"), none_leaked as f64, "count", false);
        }
    }

    // --- Search as you type, real key events ---------------------------------------------
    let _ = session.take_timings();
    let mut inject_times = Vec::new();
    for query in ["calc", "settings", "note", "clip", "zzqq", "2*21", "help", "orbit bench", "chrome", "code"] {
        if !safe(sink_pid) {
            return false;
        }
        post_chord(source);
        if !wait_until(Duration::from_secs(1), || panel_on_screen(pid)) {
            continue;
        }
        sleep(40);
        for c in query.chars() {
            inject_times.push(ticks());
            post_text(source, &c.to_string());
            sleep(35);
        }
        sleep(60);
        session.post("escape");
        wait_until(Duration::from_secs(1), || !panel_on_screen(pid));
        sleep(60);
    }
    let timings = session.take_timings();
    let typed_queries: Vec<_> = timings.queries.iter().filter(|q| q.0 > 0).copied().collect();
    report.add_timings("typing: T4 query seen to T5 results built", &typed_queries.iter().map(|q| session.ms(q.2, q.3)).collect::<Vec<_>>(), true);
    report.add_timings("typing: T5 to T6 row view drawn", &typed_queries.iter().map(|q| session.ms(q.3, q.4)).collect::<Vec<_>>(), true);
    report.add_timings("typing: T4 to T6 total", &typed_queries.iter().map(|q| session.ms(q.2, q.4)).collect::<Vec<_>>(), true);
    if typed_queries.len() == inject_times.len() {
        let external: Vec<f64> = typed_queries.iter().zip(&inject_times).map(|(q, t)| session.ms(*t, q.4)).collect();
        report.add_timings("typing: key injected to T6 row view drawn", &external, true);
    } else {
        report.not_tested.push(format!("typing: key injected to drawn: {} keys but {} query events, so they can't be paired", inject_times.len(), typed_queries.len()));
    }

    // --- Focus restoration, proved by typing ---------------------------------------------
    let mut typed_after = 0;
    const FOCUS_ROUNDS: u32 = 10;
    for _ in 0..FOCUS_ROUNDS {
        if !safe(sink_pid) {
            return false;
        }
        sink.clear();
        post_chord(source);
        wait_until(Duration::from_secs(1), || panel_on_screen(pid));
        sleep(50);
        post_key(source, CG_HID_EVENT_TAP, KEY_ESCAPE, true, 0);
        post_key(source, CG_HID_EVENT_TAP, KEY_ESCAPE, false, 0);
        wait_until(Duration::from_secs(1), || !panel_on_screen(pid));
        sleep(50);
        if !safe(sink_pid) {
            return false;
        }
        post_text(source, "x");
        sleep(60);
        if sink.state().is_some_and(|s| s.text == "x") {
            typed_after += 1;
        }
    }
    report.check("a key typed after Esc reaches the previous app", typed_after, FOCUS_ROUNDS);

    // --- Launching: real Enter ------------------------------------------------------------
    let probe_file = options.work_dir.join("probe.txt");
    let mut launched = 0;
    let mut launch_ms = Vec::new();
    const LAUNCHES: u32 = 10;
    let _ = session.take_timings();
    for _ in 0..LAUNCHES {
        if !safe(sink_pid) {
            return false;
        }
        let _ = std::fs::remove_file(&probe_file);
        post_chord(source);
        if !wait_until(Duration::from_secs(1), || panel_on_screen(pid)) {
            continue;
        }
        sleep(30);
        post_text(source, "orbit bench probe");
        sleep(80);
        if !session.is("key") {
            session.post("escape");
            continue;
        }
        let t = ticks();
        post_key(source, CG_HID_EVENT_TAP, KEY_RETURN, true, 0);
        post_key(source, CG_HID_EVENT_TAP, KEY_RETURN, false, 0);
        let limit = Instant::now();
        while limit.elapsed() < Duration::from_secs(4) {
            if let Some(stamp) = std::fs::read_to_string(&probe_file).ok().and_then(|s| s.trim().parse::<i64>().ok()) {
                launch_ms.push(session.ms(t, stamp));
                launched += 1;
                break;
            }
            sleep(2);
        }
        sleep(100);
    }
    report.check("real Enter launches the selected command", launched, LAUNCHES);
    report.add_timings("launch: Enter injected to new process running", &launch_ms, false);
    let timings = session.take_timings();
    report.add_timings("launch: Enter received to launch dispatched", &timings.actions.iter().map(|a| session.ms(a.0, a.1)).collect::<Vec<_>>(), true);

    // --- A real installed app, once. Benign, and only quit if this run opened it ----------
    if let Some((app, process)) = &options.real_app {
        real_app(report, session, sink, source, app, process);
        sink.focus();
    } else {
        report.not_tested.push("launch of a real installed app: run with --real-app NAME:PROCESS".into());
    }

    // --- Rapid toggling -------------------------------------------------------------------
    if !safe(sink_pid) {
        return false;
    }
    for _ in 0..40 {
        post_chord(source);
        sleep(12);
    }
    sleep(400);
    let alive = session.child.try_wait().ok().flatten().is_none();
    report.check("survives 40 rapid shortcut presses", alive as u32, 1);
    session.post("escape");
    sleep(100);
    if panel_on_screen(pid) {
        session.post("escape");
    }
    sink.focus();
    true
}

fn real_app(report: &mut Report, session: &mut Session, sink: &mut Sink, source: *const c_void, app: &str, process: &str) {
    let running = || !command_text("/usr/bin/pgrep", &["-x", process]).is_empty();
    if running() {
        report.not_tested.push(format!("launch of {app}: it was already running, and the benchmark won't touch your copy"));
        return;
    }
    if screen_locked() {
        return;
    }
    post_chord(source);
    if !wait_until(Duration::from_secs(1), || panel_on_screen(session.pid)) {
        report.check(format!("launch real app {app}"), 0, 1);
        return;
    }
    sleep(30);
    post_text(source, app);
    sleep(120);
    let t = Instant::now();
    post_key(source, CG_HID_EVENT_TAP, KEY_RETURN, true, 0);
    post_key(source, CG_HID_EVENT_TAP, KEY_RETURN, false, 0);
    let found = wait_until(Duration::from_secs(8), || running());
    report.check(format!("launch real app {app}"), found as u32, 1);
    if found {
        report.add(format!("launch: {app}, Enter to process present (pgrep polling, coarse)"), t.elapsed().as_secs_f64() * 1000.0, "ms", false);
        sleep(800);
        // Only what this run opened: it wasn't running a moment ago.
        let _ = Command::new("/usr/bin/pkill").args(["-x", process]).status();
        sleep(300);
    }
    sink.focus();
}

fn idle_suite(report: &mut Report, options: &E2eOptions, session: &mut Session) {
    session.post("trim");
    sleep(1000);
    let pid = session.pid;
    let (Some(before), Some(task_before)) = (usage(pid), task_info(pid)) else {
        report.not_tested.push("idle: couldn't read process counters".into());
        return;
    };
    let started = Instant::now();
    std::thread::sleep(Duration::from_secs(options.idle_secs));
    let seconds = started.elapsed().as_secs_f64();
    let per_minute = 60.0 / seconds;
    let (Some(after), Some(task_after)) = (usage(pid), task_info(pid)) else {
        report.not_tested.push("idle: couldn't read process counters".into());
        return;
    };
    report.add("idle: measured for", seconds, "s", false);
    report.add("idle: CPU time per minute (proc_pid_rusage)", (after.cpu_ms - before.cpu_ms) * per_minute, "ms", false);
    report.add("idle: CPU percent of one core", (after.cpu_ms - before.cpu_ms) / (seconds * 10.0), "%", false);
    report.add("idle: package idle wakeups per minute", (after.idle_wakeups - before.idle_wakeups) as f64 * per_minute, "count", false);
    report.add("idle: interrupt wakeups per minute", (after.interrupt_wakeups - before.interrupt_wakeups) as f64 * per_minute, "count", false);
    report.add("idle: context switches per minute", (task_after.csw - task_before.csw) as f64 * per_minute, "count", false);
    report.add("idle: disk bytes written", (after.disk_written - before.disk_written) as f64, "count", false);
    report.add("idle: logical bytes written", (after.logical_writes - before.logical_writes) as f64, "count", false);
    report.add("idle: threads", task_after.threadnum as f64, "count", true);
    report.add("memory after idle: footprint (proc_pid_rusage phys_footprint)", after.footprint as f64 / 1024.0, "KB", true);
    report.add("memory after idle: resident (proc_pid_rusage, includes shared frameworks)", after.resident as f64 / 1024.0, "KB", true);
    report.add("memory peak: footprint this run", after.peak_footprint as f64 / 1024.0, "KB", true);
    snapshot(report, "after idle, self-reported", session, false);
}

/// HyperKey through a real event tap. F19 stands in for the trigger, because this Mac has no F19 key,
/// so nothing about the owner's real keyboard changes while it runs.
fn hyper_suite(report: &mut Report, options: &E2eOptions, dir: &Path, probe: &Path, probe_file: &Path) {
    let locked = screen_locked();
    let allowed = unsafe { CGPreflightPostEventAccess() && CGPreflightListenEventAccess() };
    if locked || !allowed {
        return;
    }
    write_settings(dir, probe, probe_file, serde_json::json!({
        "hyper": { "enabled": true, "trigger": "F19", "mode": "internal", "tap": "none", "tap_timeout_ms": 250,
                   "bindings": [{ "key": "K", "action": { "kind": "builtin", "command": "show_help" } }] },
    }));
    let log = dir.join("hyper.csv");
    let Some(mut session) = Session::start(&options.orbit, dir, &log, &["--no-reindex", "--no-trim"]) else { return };
    if session.wait_ready(Duration::from_secs(10)).is_none() {
        session.quit(None);
        return;
    }
    let state = session.state();
    let tap_on = state.get("tap").is_some_and(|t| t == "true");
    report.check("the event tap starts", tap_on as u32, 1);
    let source = unsafe { CGEventSourceCreate(CG_HID_SYSTEM_STATE) };
    let Some(mut sink) = Sink::start(&exe_dir_for_sink(options), dir) else {
        session.quit(None);
        return;
    };
    if tap_on && sink.focus() {
        let sink_pid = sink.child.id() as i64;
        let mut dispatched = 0;
        let mut no_stray = 0;
        const ROUNDS: u32 = 10;
        for _ in 0..ROUNDS {
            if screen_locked() || frontmost_normal_owner() != Some(sink_pid) {
                break;
            }
            sink.clear();
            let _ = session.take_timings();
            post_key(source, CG_HID_EVENT_TAP, KEY_F19, true, FLAG_SECONDARY_FN);
            post_key(source, CG_HID_EVENT_TAP, KEY_K, true, 0);
            post_key(source, CG_HID_EVENT_TAP, KEY_K, false, 0);
            post_key(source, CG_HID_EVENT_TAP, KEY_F19, false, FLAG_SECONDARY_FN);
            sleep(150);
            let timings = session.take_timings();
            if timings.hyper_dispatches == 1 {
                dispatched += 1;
            }
            session.post("escape");
            session.post("escape");
            wait_until(Duration::from_secs(1), || !panel_on_screen(session.pid));
            sleep(60);
            if sink.state().is_some_and(|s| s.text.is_empty()) {
                no_stray += 1;
            }
            sink.focus();
        }
        report.check("Hyper+K dispatches exactly once", dispatched, ROUNDS);
        report.check("Hyper+K leaves no stray k in the focused app", no_stray, ROUNDS);
        // And normal typing must pass through untouched.
        sink.clear();
        if !screen_locked() && frontmost_normal_owner() == Some(sink_pid) {
            post_key(source, CG_HID_EVENT_TAP, KEY_K, true, 0);
            post_key(source, CG_HID_EVENT_TAP, KEY_K, false, 0);
            sleep(100);
            report.check("a plain k still types with the tap on", sink.state().is_some_and(|s| s.text == "k") as u32, 1);
        }
    }
    sink.stop();
    if !source.is_null() {
        unsafe { CFRelease(source) };
    }
    session.quit(None);
    report.environment.insert("hyper test".into(), "F19 as the trigger, under the grants the driver inherits, not Orbit.app's own".into());
    leader_suite(report, options, dir, probe, probe_file);
}

/// Leader mode: tap the trigger, then press a mapped key with nothing held. Same F19 stand-in.
fn leader_suite(report: &mut Report, options: &E2eOptions, dir: &Path, probe: &Path, probe_file: &Path) {
    write_settings(dir, probe, probe_file, serde_json::json!({
        "hyper": { "enabled": true, "trigger": "F19", "mode": "internal", "tap": "leader", "tap_timeout_ms": 250,
                   "bindings": [{ "key": "K", "action": { "kind": "builtin", "command": "show_help" } }] },
    }));
    let log = dir.join("leader.csv");
    let Some(mut session) = Session::start(&options.orbit, dir, &log, &["--no-reindex", "--no-trim"]) else { return };
    if session.wait_ready(Duration::from_secs(10)).is_none() {
        session.quit(None);
        return;
    }
    let tap_on = session.state().get("tap").is_some_and(|t| t == "true");
    let source = unsafe { CGEventSourceCreate(CG_HID_SYSTEM_STATE) };
    let Some(mut sink) = Sink::start(&exe_dir_for_sink(options), dir) else {
        session.quit(None);
        return;
    };
    if tap_on && sink.focus() {
        let sink_pid = sink.child.id() as i64;
        let usable = |sink_owner: i64| !screen_locked() && frontmost_normal_owner() == Some(sink_owner);
        let tap_trigger = || {
            post_key(source, CG_HID_EVENT_TAP, KEY_F19, true, FLAG_SECONDARY_FN);
            sleep(30);
            post_key(source, CG_HID_EVENT_TAP, KEY_F19, false, FLAG_SECONDARY_FN);
            sleep(30);
        };
        let press = |key: u16| {
            post_key(source, CG_HID_EVENT_TAP, key, true, 0);
            post_key(source, CG_HID_EVENT_TAP, key, false, 0);
        };
        const ROUNDS: u32 = 5;
        let (mut dispatched, mut no_stray) = (0, 0);
        for _ in 0..ROUNDS {
            if !usable(sink_pid) {
                break;
            }
            sink.clear();
            let _ = session.take_timings();
            tap_trigger();
            press(KEY_K);
            sleep(150);
            if session.take_timings().hyper_dispatches == 1 {
                dispatched += 1;
            }
            session.post("escape");
            session.post("escape");
            wait_until(Duration::from_secs(1), || !panel_on_screen(session.pid));
            sleep(60);
            if sink.state().is_some_and(|s| s.text.is_empty()) {
                no_stray += 1;
            }
            sink.focus();
        }
        report.check("Leader: tap then K dispatches exactly once", dispatched, ROUNDS);
        report.check("Leader: the trigger and K leave nothing in the focused app", no_stray, ROUNDS);
        if usable(sink_pid) {
            sink.clear();
            let _ = session.take_timings();
            tap_trigger();
            press(KEY_J);
            sleep(100);
            press(KEY_K);
            sleep(150);
            report.check("Leader: an unmapped key types and ends the wait", (session.take_timings().hyper_dispatches == 0 && sink.state().is_some_and(|s| s.text == "jk")) as u32, 1);
        }
        if usable(sink_pid) {
            sink.clear();
            let _ = session.take_timings();
            tap_trigger();
            sleep(2200);
            press(KEY_K);
            sleep(150);
            report.check("Leader: a key after 2 s types normally", (session.take_timings().hyper_dispatches == 0 && sink.state().is_some_and(|s| s.text == "k")) as u32, 1);
        }
    } else {
        report.not_tested.push("Leader through the tap: the tap didn't start or the sink didn't get focus".into());
    }
    sink.stop();
    if !source.is_null() {
        unsafe { CFRelease(source) };
    }
    session.quit(None);
}
