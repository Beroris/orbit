//! Launched by Orbit during the benchmark. Records the moment it started, then exits.
//!
//! `probe PATH` replaces the file with the time. `probe PATH --append` adds a line instead, so a
//! test can count how many times it ran. `probe --window`, with `--fullscreen` if it should fill
//! the screen, shows a plain window for a test to act on, and closes by itself after a minute.
#![cfg_attr(windows, windows_subsystem = "windows")]

fn main() {
    let Some(path) = std::env::args().nth(1) else { return };
    #[cfg(windows)]
    if path == "--window" {
        test_window::run(std::env::args().nth(2).as_deref() == Some("--fullscreen"));
        return;
    }
    #[cfg(windows)]
    let now = {
        let mut ticks = 0i64;
        unsafe {
            let _ = windows::Win32::System::Performance::QueryPerformanceCounter(&mut ticks);
        }
        ticks
    };
    #[cfg(target_os = "macos")]
    let now = {
        unsafe extern "C" {
            fn mach_absolute_time() -> u64;
        }
        unsafe { mach_absolute_time() as i64 }
    };
    #[cfg(not(any(windows, target_os = "macos")))]
    let now = 0i64;
    if std::env::args().nth(2).as_deref() == Some("--append") {
        use std::io::Write;
        if let Ok(mut file) = std::fs::OpenOptions::new().create(true).append(true).open(path) {
            let _ = writeln!(file, "{now}");
        }
    } else {
        let _ = std::fs::write(path, now.to_string());
    }
}

#[cfg(windows)]
mod test_window {
    use windows::Win32::Foundation::{HWND, LPARAM, LRESULT, POINT, WPARAM};
    use windows::Win32::Graphics::Gdi::{CreateSolidBrush, GetMonitorInfoW, MONITOR_DEFAULTTOPRIMARY, MONITORINFO, MonitorFromPoint};
    use windows::Win32::System::LibraryLoader::GetModuleHandleW;
    use windows::Win32::UI::WindowsAndMessaging::*;
    use windows::core::w;

    const CLOSE_AFTER_MS: u32 = 60_000;

    unsafe extern "system" fn proc(hwnd: HWND, message: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
        unsafe {
            if message == WM_DESTROY || message == WM_TIMER {
                PostQuitMessage(0);
                return LRESULT(0);
            }
            DefWindowProcW(hwnd, message, wparam, lparam)
        }
    }

    pub fn run(fullscreen: bool) {
        unsafe {
            let instance = GetModuleHandleW(None).unwrap_or_default();
            let class = WNDCLASSW {
                lpfnWndProc: Some(proc),
                hInstance: instance.into(),
                lpszClassName: w!("OrbitBenchTestApp"),
                hbrBackground: CreateSolidBrush(windows::Win32::Foundation::COLORREF(0x201C1C)),
                ..Default::default()
            };
            RegisterClassW(&class);
            let monitor = MonitorFromPoint(POINT { x: 0, y: 0 }, MONITOR_DEFAULTTOPRIMARY);
            let mut info = MONITORINFO { cbSize: std::mem::size_of::<MONITORINFO>() as u32, ..Default::default() };
            let _ = GetMonitorInfoW(monitor, &mut info);
            let screen = info.rcMonitor;
            let (style, x, y, width, height) = if fullscreen {
                (WS_POPUP, screen.left, screen.top, screen.right - screen.left, screen.bottom - screen.top)
            } else {
                (WS_OVERLAPPEDWINDOW, screen.left + 120, screen.top + 120, 420, 300)
            };
            let Ok(hwnd) = CreateWindowExW(WINDOW_EX_STYLE(0), w!("OrbitBenchTestApp"), w!("Orbit benchmark window"), style, x, y, width, height, None, None, Some(instance.into()), None) else {
                return;
            };
            let _ = ShowWindow(hwnd, SW_SHOW);
            let _ = SetForegroundWindow(hwnd);
            SetTimer(Some(hwnd), 1, CLOSE_AFTER_MS, None);
            let mut message = MSG::default();
            while GetMessageW(&mut message, None, 0, 0).as_bool() {
                let _ = TranslateMessage(&message);
                DispatchMessageW(&message);
            }
        }
    }
}
