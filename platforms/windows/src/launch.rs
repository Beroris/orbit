//! Starting and focusing other programs.
//!
//! `ShellExecuteExW` can take hundreds of milliseconds and may load shell extensions, so it
//! runs on a short-lived thread and the launcher's thread never waits on it.

use crate::indexer::AppEntry;
use crate::util::{SendHwnd, exe_name, process_image_path, wide};
use windows::Win32::Foundation::{CloseHandle, HWND, LPARAM, RECT};
use windows::Win32::System::Com::{COINIT_APARTMENTTHREADED, COINIT_DISABLE_OLE1DDE, CoInitializeEx, CoUninitialize};
use windows::Win32::Storage::Packaging::Appx::GetApplicationUserModelId;
use windows::Win32::System::Threading::{OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION};
use windows::Win32::UI::Input::KeyboardAndMouse::{
    INPUT, INPUT_0, INPUT_MOUSE, MOUSE_EVENT_FLAGS, MOUSEINPUT, SendInput,
};
use windows::Win32::UI::Shell::{
    SEE_MASK_FLAG_NO_UI, SEE_MASK_NOASYNC, SHELLEXECUTEINFOW, ShellExecuteExW,
};
use windows::Win32::UI::WindowsAndMessaging::{
    ASFW_ANY, AllowSetForegroundWindow, EnumChildWindows, EnumWindows, GA_ROOTOWNER, GW_OWNER, GWL_EXSTYLE,
    GetAncestor, GetClassNameW, GetForegroundWindow, GetWindow, GetWindowLongW, GetWindowRect, GetWindowTextLengthW,
    GetWindowThreadProcessId, IsIconic, IsWindowVisible, SW_MINIMIZE, SW_RESTORE, SW_SHOWNORMAL,
    SetForegroundWindow, ShowWindow, ShowWindowAsync, WS_EX_TOOLWINDOW,
};
use windows::core::{BOOL, PCWSTR, PWSTR};

/// Posted back to the launcher thread if a launch fails.
pub struct LaunchRequest {
    pub file: String,
    pub parameters: String,
    pub working_dir: String,
    pub run_as_admin: bool,
}

impl LaunchRequest {
    pub fn app(id: &str, run_as_admin: bool) -> Self {
        LaunchRequest {
            file: format!("shell:AppsFolder\\{id}"),
            parameters: String::new(),
            working_dir: String::new(),
            run_as_admin,
        }
    }

    pub fn open(target: &str) -> Self {
        LaunchRequest {
            file: target.to_string(),
            parameters: String::new(),
            working_dir: String::new(),
            run_as_admin: false,
        }
    }
}

/// Runs the request off-thread. `report` receives the failure text, if any.
/// A file that opening would run, rather than show: programs, scripts, installers, shortcuts and
/// the like. Orbit opens these only from the app list, never from clipboard history.
pub fn is_runnable(path: &std::path::Path) -> bool {
    const RUNNABLE: [&str; 30] = [
        "exe", "com", "bat", "cmd", "ps1", "psm1", "vbs", "vbe", "js", "jse", "wsf", "wsh", "msi", "msp", "mst", "scr", "pif", "lnk", "url", "hta",
        "cpl", "reg", "jar", "msix", "msixbundle", "appx", "appxbundle", "appinstaller", "application", "appref-ms",
    ];
    path.extension().map(|ext| ext.to_string_lossy().to_ascii_lowercase()).is_some_and(|ext| RUNNABLE.contains(&ext.as_str()) || ext == "settingcontent-ms")
}

pub fn spawn(request: LaunchRequest, main: SendHwnd, on_error_message: u32) {
    unsafe {
        // Hand our foreground right to whatever starts, so the launched app appears in front.
        let _ = AllowSetForegroundWindow(ASFW_ANY);
    }
    let _ = std::thread::Builder::new()
        .name("orbit-launch".into())
        .stack_size(512 * 1024)
        .spawn(move || {
            if let Err(message) = execute(&request) {
                main.post(on_error_message, crate::util::box_for_post(message), 0);
            }
        });
}

fn execute(request: &LaunchRequest) -> Result<(), String> {
    unsafe {
        let com = CoInitializeEx(None, COINIT_APARTMENTTHREADED | COINIT_DISABLE_OLE1DDE);
        let file = wide(&request.file);
        let parameters = wide(&request.parameters);
        let directory = wide(&request.working_dir);
        let verb = wide(if request.run_as_admin { "runas" } else { "open" });
        let mut info = SHELLEXECUTEINFOW {
            cbSize: std::mem::size_of::<SHELLEXECUTEINFOW>() as u32,
            fMask: SEE_MASK_NOASYNC | SEE_MASK_FLAG_NO_UI,
            lpVerb: PCWSTR(verb.as_ptr()),
            lpFile: PCWSTR(file.as_ptr()),
            nShow: SW_SHOWNORMAL.0,
            ..Default::default()
        };
        if !request.parameters.is_empty() {
            info.lpParameters = PCWSTR(parameters.as_ptr());
        }
        if !request.working_dir.is_empty() {
            info.lpDirectory = PCWSTR(directory.as_ptr());
        }
        let result = ShellExecuteExW(&mut info);
        if com.is_ok() {
            CoUninitialize();
        }
        result.map_err(|error| {
            // Don't echo the path into logs elsewhere; it is only shown to the user.
            format!("Could not open {}: {}", request.file, error.message())
        })
    }
}

pub(crate) fn window_aumid(pid: u32) -> Option<String> {
    unsafe {
        let handle = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid).ok()?;
        let mut buffer = [0u16; 256];
        let mut len = buffer.len() as u32;
        let status = GetApplicationUserModelId(handle, &mut len, Some(PWSTR(buffer.as_mut_ptr())));
        let _ = CloseHandle(handle);
        if status.0 != 0 || len == 0 {
            return None;
        }
        Some(String::from_utf16_lossy(&buffer[..len as usize - 1]))
    }
}

struct FindContext<'a> {
    app: &'a AppEntry,
    found: Option<HWND>,
}

fn class_name(hwnd: HWND) -> String {
    let mut buffer = [0u16; 64];
    let len = unsafe { GetClassNameW(hwnd, &mut buffer) };
    String::from_utf16_lossy(&buffer[..len.max(0) as usize])
}

fn matches_process(app: &AppEntry, pid: u32) -> bool {
    if !app.aumid.is_empty() && window_aumid(pid).is_some_and(|id| id.eq_ignore_ascii_case(&app.aumid)) {
        return true;
    }
    if app.exe.is_empty() {
        return false;
    }
    let Some(path) = process_image_path(pid) else {
        return false;
    };
    let path = path.to_lowercase();
    path == app.exe || exe_name(&path) == exe_name(&app.exe)
}

unsafe extern "system" fn find_child(child: HWND, lparam: LPARAM) -> BOOL {
    // A UWP frame's content lives in a child owned by the real app process.
    unsafe {
        let (app_ptr, frame_pid, hit): (*const AppEntry, u32, *mut bool) =
            *(lparam.0 as *const (*const AppEntry, u32, *mut bool));
        let mut pid = 0;
        GetWindowThreadProcessId(child, Some(&mut pid));
        if pid != frame_pid && matches_process(&*app_ptr, pid) {
            *hit = true;
            return BOOL(0);
        }
        BOOL(1)
    }
}

/// A window someone would call "the app": visible, titled, top-level, and not a tool palette.
fn is_app_window(hwnd: HWND) -> bool {
    unsafe {
        if !IsWindowVisible(hwnd).as_bool()
            || GetWindowTextLengthW(hwnd) == 0
            || GetWindow(hwnd, GW_OWNER).is_ok_and(|owner| !owner.is_invalid())
            || GetWindowLongW(hwnd, GWL_EXSTYLE) as u32 & WS_EX_TOOLWINDOW.0 != 0
        {
            return false;
        }
        let mut rect = RECT::default();
        let _ = GetWindowRect(hwnd, &mut rect);
        rect.right - rect.left > 1 && rect.bottom - rect.top > 1
    }
}

/// Which apps have a window open right now, gathered once per launcher opening on the first
/// keystroke, so results can say "Switch to" before Enter is pressed.
#[derive(Default)]
pub struct RunningApps {
    exe_names: Vec<String>,
    aumids: Vec<String>,
}

impl RunningApps {
    pub fn gather() -> RunningApps {
        let mut running = RunningApps::default();
        unsafe {
            let _ = EnumWindows(Some(gather_window), LPARAM(&mut running as *mut _ as isize));
        }
        running
    }

    fn add_process(&mut self, pid: u32) {
        if let Some(path) = process_image_path(pid) {
            let name = exe_name(&path.to_lowercase()).to_string();
            if !self.exe_names.contains(&name) {
                self.exe_names.push(name);
            }
        }
        if let Some(id) = window_aumid(pid) {
            if !self.aumids.iter().any(|known| known.eq_ignore_ascii_case(&id)) {
                self.aumids.push(id);
            }
        }
    }

    pub fn has(&self, app: &AppEntry) -> bool {
        if !app.aumid.is_empty() && self.aumids.iter().any(|id| id.eq_ignore_ascii_case(&app.aumid)) {
            return true;
        }
        !app.exe.is_empty() && self.exe_names.iter().any(|name| name == exe_name(&app.exe))
    }
}

unsafe extern "system" fn gather_child(child: HWND, lparam: LPARAM) -> BOOL {
    unsafe {
        let (running, frame_pid) = &mut *(lparam.0 as *mut (&mut RunningApps, u32));
        let mut pid = 0;
        GetWindowThreadProcessId(child, Some(&mut pid));
        if pid != *frame_pid {
            running.add_process(pid);
        }
        BOOL(1)
    }
}

unsafe extern "system" fn gather_window(hwnd: HWND, lparam: LPARAM) -> BOOL {
    unsafe {
        if !is_app_window(hwnd) {
            return BOOL(1);
        }
        let running = &mut *(lparam.0 as *mut RunningApps);
        let mut pid = 0;
        GetWindowThreadProcessId(hwnd, Some(&mut pid));
        if class_name(hwnd) == "ApplicationFrameWindow" {
            let mut args: (&mut RunningApps, u32) = (running, pid);
            let _ = EnumChildWindows(Some(hwnd), Some(gather_child), LPARAM(&mut args as *mut _ as isize));
        } else {
            running.add_process(pid);
        }
        BOOL(1)
    }
}

/// Does this window belong to the app? A UWP frame is hosted by another process, so its content
/// is what counts.
fn window_is_app(hwnd: HWND, app: &AppEntry) -> bool {
    unsafe {
        let mut pid = 0;
        GetWindowThreadProcessId(hwnd, Some(&mut pid));
        if class_name(hwnd) == "ApplicationFrameWindow" {
            let mut found = false;
            let args: (*const AppEntry, u32, *mut bool) = (app, pid, &mut found);
            let _ = EnumChildWindows(Some(hwnd), Some(find_child), LPARAM(&args as *const _ as isize));
            found
        } else {
            matches_process(app, pid)
        }
    }
}

unsafe extern "system" fn find_window(hwnd: HWND, lparam: LPARAM) -> BOOL {
    unsafe {
        let context = &mut *(lparam.0 as *mut FindContext);
        if !is_app_window(hwnd) {
            return BOOL(1);
        }
        if window_is_app(hwnd, &*context.app) {
            context.found = Some(hwnd);
            return BOOL(0);
        }
        BOOL(1)
    }
}

/// Program paths of the windows open now. Asked only when Settings opens, to offer apps the index
/// can't name a program for, such as ones from the Microsoft Store.
pub fn running_programs() -> Vec<String> {
    unsafe extern "system" fn collect(hwnd: HWND, lparam: LPARAM) -> BOOL {
        unsafe {
            if is_app_window(hwnd) {
                let found = &mut *(lparam.0 as *mut Vec<String>);
                let mut pid = 0;
                GetWindowThreadProcessId(hwnd, Some(&mut pid));
                if let Some(path) = process_image_path(pid) {
                    if !found.contains(&path) {
                        found.push(path);
                    }
                }
            }
            BOOL(1)
        }
    }
    let mut found: Vec<String> = Vec::new();
    unsafe {
        let _ = EnumWindows(Some(collect), LPARAM(&mut found as *mut _ as isize));
    }
    found
}

/// Finds a top-level window of this app. Runs only when an action asks, never in the background.
pub fn find_running_window(app: &AppEntry) -> Option<HWND> {
    if app.exe.is_empty() && app.aumid.is_empty() {
        return None;
    }
    let mut context = FindContext { app, found: None };
    unsafe {
        let _ = EnumWindows(Some(find_window), LPARAM(&mut context as *mut _ as isize));
    }
    context.found
}

/// If this app is what's in front, minimizes that window and returns true. Windows then activates
/// whatever was behind it. A dialog minimizes along with the window that owns it. The request is
/// asynchronous, so an app that has stopped responding can't hold up the launcher.
pub fn minimize_if_in_front(app: &AppEntry) -> bool {
    if app.exe.is_empty() && app.aumid.is_empty() {
        return false;
    }
    unsafe {
        let foreground = GetForegroundWindow();
        if foreground.is_invalid() {
            return false;
        }
        let root = GetAncestor(foreground, GA_ROOTOWNER);
        let window = if root.is_invalid() { foreground } else { root };
        if !window_is_app(window, app) {
            return false;
        }
        ShowWindowAsync(window, SW_MINIMIZE).as_bool()
    }
}

/// Asks the app in front to close, or minimizes it: all of its windows, as a Mac does. Closing is
/// the same request as the X on a window, so an app can still ask about unsaved work. Explorer and
/// the host of Store apps run many unrelated windows, so with those it's just the one in front.
pub fn quit_or_hide_front(quit: bool) {
    unsafe {
        let foreground = GetForegroundWindow();
        if foreground.is_invalid() {
            return;
        }
        let root = GetAncestor(foreground, GA_ROOTOWNER);
        let window = if root.is_invalid() { foreground } else { root };
        let mut class = [0u16; 32];
        let len = GetClassNameW(window, &mut class).max(0) as usize;
        let class = String::from_utf16_lossy(&class[..len]);
        // The desktop and the taskbar aren't apps.
        if matches!(class.as_str(), "Progman" | "WorkerW" | "Shell_TrayWnd" | "Shell_SecondaryTrayWnd") {
            return;
        }
        let act = |hwnd: HWND| {
            if quit {
                let _ = windows::Win32::UI::WindowsAndMessaging::PostMessageW(Some(hwnd), windows::Win32::UI::WindowsAndMessaging::WM_CLOSE, windows::Win32::Foundation::WPARAM(0), LPARAM(0));
            } else {
                let _ = ShowWindowAsync(hwnd, SW_MINIMIZE);
            }
        };
        let mut pid = 0;
        GetWindowThreadProcessId(window, Some(&mut pid));
        let path = crate::util::process_image_path(pid).unwrap_or_default();
        let name = crate::util::exe_name(&path);
        if name.eq_ignore_ascii_case("explorer.exe") || name.eq_ignore_ascii_case("applicationframehost.exe") {
            act(window);
            return;
        }
        let mut windows = SameProcess { pid, found: vec![window] };
        let _ = EnumWindows(Some(gather_same_process), LPARAM(&mut windows as *mut _ as isize));
        for hwnd in windows.found {
            act(hwnd);
        }
    }
}

struct SameProcess {
    pid: u32,
    found: Vec<HWND>,
}

unsafe extern "system" fn gather_same_process(hwnd: HWND, data: LPARAM) -> BOOL {
    unsafe {
        let windows = &mut *(data.0 as *mut SameProcess);
        let mut pid = 0;
        GetWindowThreadProcessId(hwnd, Some(&mut pid));
        if pid == windows.pid && is_app_window(hwnd) && !windows.found.contains(&hwnd) {
            windows.found.push(hwnd);
        }
    }
    BOOL(1)
}

/// Brings a window to the front. Windows only lets the process with recent input change the
/// foreground, so a no-op mouse event is injected first to qualify: this does not move the
/// pointer or reach any app, and it is the method PowerToys uses for the same purpose.
pub fn force_foreground(hwnd: HWND) -> bool {
    unsafe {
        let input = INPUT {
            r#type: INPUT_MOUSE,
            Anonymous: INPUT_0 {
                mi: MOUSEINPUT { dwFlags: MOUSE_EVENT_FLAGS(0), ..Default::default() },
            },
        };
        SendInput(&[input], std::mem::size_of::<INPUT>() as i32);
        if IsIconic(hwnd).as_bool() {
            let _ = ShowWindow(hwnd, SW_RESTORE);
        }
        SetForegroundWindow(hwnd).as_bool()
    }
}

/// Ends an app as Task Manager's End task does: every process of its program, straight away, with no
/// chance to ask about unsaved work. Never File Explorer, which is also the taskbar and the desktop,
/// and never Orbit. Returns how many processes ended, or why none did.
pub fn end_task(app: &AppEntry) -> Result<usize, String> {
    use windows::Win32::System::ProcessStatus::EnumProcesses;
    use windows::Win32::System::Threading::{GetCurrentProcessId, PROCESS_TERMINATE, TerminateProcess};
    if app.exe.is_empty() && app.aumid.is_empty() {
        return Err("Orbit can't tell which program this app is.".into());
    }
    if exe_name(&app.exe) == "explorer.exe" {
        return Err("File Explorer also runs the taskbar and desktop, so Orbit leaves it alone.".into());
    }
    let mut ids = vec![0u32; 4096];
    let mut used = 0u32;
    unsafe { EnumProcesses(ids.as_mut_ptr(), (ids.len() * 4) as u32, &mut used) }.map_err(|_| "Windows didn't list the running programs.".to_string())?;
    ids.truncate(used as usize / 4);
    let own = unsafe { GetCurrentProcessId() };
    let (mut ended, mut refused) = (0, 0);
    for pid in ids.into_iter().filter(|&pid| pid != 0 && pid != own && matches_process(app, pid)) {
        match unsafe { OpenProcess(PROCESS_TERMINATE, false, pid) } {
            Ok(handle) => {
                if unsafe { TerminateProcess(handle, 1) }.is_ok() {
                    ended += 1;
                } else {
                    refused += 1;
                }
                let _ = unsafe { CloseHandle(handle) };
            }
            Err(_) => refused += 1,
        }
    }
    match (ended, refused) {
        (0, 0) => Err("It isn't running.".into()),
        (0, _) => Err("Windows wouldn't let Orbit end it. It may be running as administrator.".into()),
        _ => Ok(ended),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Starts a hidden `ping` and ends it as End task would. Ignored, because it starts a program;
    /// it ends any other `ping` running at the time too.
    #[test]
    #[ignore]
    fn end_task_ends_every_process_of_the_program() {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        let mut child = std::process::Command::new(r"C:\Windows\System32\PING.EXE")
            .args(["-n", "60", "127.0.0.1"])
            .stdout(std::process::Stdio::null())
            .creation_flags(CREATE_NO_WINDOW)
            .spawn()
            .unwrap();
        std::thread::sleep(std::time::Duration::from_millis(300));
        let app = AppEntry { id: "ping".into(), name: "Ping".into(), exe: r"c:\windows\system32\ping.exe".into(), aumid: String::new() };
        assert_eq!(end_task(&app), Ok(1));
        let status = child.wait().unwrap();
        assert!(!status.success(), "it was ended, not left to finish");
        assert!(end_task(&app).is_err(), "nothing left to end");
        let explorer = AppEntry { id: "e".into(), name: "File Explorer".into(), exe: r"c:\windows\explorer.exe".into(), aumid: String::new() };
        assert!(end_task(&explorer).unwrap_err().contains("taskbar"));
    }
}
