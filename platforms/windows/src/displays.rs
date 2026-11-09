//! The screens Windows has: what Settings draws in its picture of them, and which one the launcher
//! opens on.
//!
//! A specific screen picked in Settings is kept by Windows' path for the monitor on its
//! connection, which stays the same from one start to the next, unlike the handles and
//! `\\.\DISPLAYn` names. The list is read once and kept until Windows says the screens changed, so
//! opening the launcher on a specific screen costs a lookup, not a query.

use orbit_core::settings::LauncherScreen;
use std::sync::Mutex;
use windows::Win32::Devices::Display::{
    DISPLAYCONFIG_DEVICE_INFO_GET_SOURCE_NAME, DISPLAYCONFIG_DEVICE_INFO_GET_TARGET_NAME, DISPLAYCONFIG_DEVICE_INFO_HEADER, DISPLAYCONFIG_MODE_INFO, DISPLAYCONFIG_OUTPUT_TECHNOLOGY_DISPLAYPORT_EMBEDDED,
    DISPLAYCONFIG_OUTPUT_TECHNOLOGY_INTERNAL, DISPLAYCONFIG_OUTPUT_TECHNOLOGY_UDI_EMBEDDED, DISPLAYCONFIG_PATH_INFO, DISPLAYCONFIG_SOURCE_DEVICE_NAME, DISPLAYCONFIG_TARGET_DEVICE_NAME, DisplayConfigGetDeviceInfo,
    GetDisplayConfigBufferSizes, QDC_ONLY_ACTIVE_PATHS, QueryDisplayConfig,
};
use windows::Win32::Foundation::{ERROR_INSUFFICIENT_BUFFER, ERROR_SUCCESS, HWND, LPARAM, POINT, RECT};
use windows::Win32::Graphics::Gdi::{EnumDisplayMonitors, GetMonitorInfoW, HDC, HMONITOR, MONITOR_DEFAULTTONEAREST, MONITOR_DEFAULTTOPRIMARY, MONITORINFO, MONITORINFOEXW, MonitorFromPoint, MonitorFromWindow};
use windows::Win32::UI::WindowsAndMessaging::{GetCursorPos, MONITORINFOF_PRIMARY};
use windows::core::BOOL;

/// One screen.
#[derive(Clone, Debug, PartialEq)]
pub struct Display {
    /// Windows' path for the monitor on its connection, `\\?\DISPLAY#…`, or the `\\.\DISPLAYn` name
    /// when Windows doesn't give one.
    pub id: String,
    /// The monitor's own name, such as "VG259", "Built-in screen" for a laptop's, or "Screen 2".
    pub name: String,
    /// The number Windows puts on it in its own display settings, from `\\.\DISPLAYn`.
    pub number: u32,
    /// The whole screen, in physical pixels on the desktop the screens make together.
    pub bounds: RECT,
    pub primary: bool,
    /// `\\.\DISPLAYn`, which matches a monitor handle to the screen.
    gdi: String,
    /// The handle while the screens stay as they are. Kept as a number so the list can be shared.
    monitor: isize,
}

impl Display {
    /// A stand-in screen, for previews and tests.
    pub fn stand_in(number: u32, name: &str, bounds: RECT, primary: bool) -> Display {
        Display { id: format!(r"\\?\DISPLAY#STANDIN{number}"), name: name.into(), number, bounds, primary, gdi: format!(r"\\.\DISPLAY{number}"), monitor: 0 }
    }

    pub fn width(&self) -> i32 {
        self.bounds.right - self.bounds.left
    }

    pub fn height(&self) -> i32 {
        self.bounds.bottom - self.bounds.top
    }

    /// "Screen 2, VG259, 1920 × 1080", with "main screen" for Windows' main one.
    pub fn describe(&self) -> String {
        let main = if self.primary { ", main screen" } else { "" };
        if self.name == default_name(self.number) {
            format!("Screen {}, {} \u{00D7} {}{main}", self.number, self.width(), self.height())
        } else {
            format!("Screen {}, {}, {} \u{00D7} {}{main}", self.number, self.name, self.width(), self.height())
        }
    }
}

fn default_name(number: u32) -> String {
    format!("Screen {number}")
}

static CACHE: Mutex<Option<Vec<Display>>> = Mutex::new(None);
/// Screens to show instead of the real ones, in a preview.
static STAND_INS: Mutex<Option<Vec<Display>>> = Mutex::new(None);

/// Forgets the screens, because Windows says they changed.
pub fn changed() {
    if let Ok(mut cache) = CACHE.lock() {
        *cache = None;
    }
}

/// Shows these screens instead of the real ones, for a preview.
pub fn use_stand_ins(displays: Vec<Display>) {
    if let Ok(mut stand_ins) = STAND_INS.lock() {
        *stand_ins = Some(displays);
    }
}

/// Every screen, left to right, read again from Windows.
pub fn list() -> Vec<Display> {
    if let Some(stand_ins) = STAND_INS.lock().ok().and_then(|stand_ins| stand_ins.clone()) {
        return stand_ins;
    }
    let displays = read();
    if let Ok(mut cache) = CACHE.lock() {
        *cache = Some(displays.clone());
    }
    displays
}

/// Every screen, as last read.
fn known() -> Vec<Display> {
    match CACHE.lock().ok().and_then(|cache| cache.clone()) {
        Some(displays) => displays,
        None => list(),
    }
}

/// The screen kept as `id`, or else the only one connected that's called `name`, as when a monitor
/// moves to another port or cable.
pub fn find<'a>(displays: &'a [Display], id: &str, name: &str) -> Option<&'a Display> {
    if let Some(display) = displays.iter().find(|display| display.id.eq_ignore_ascii_case(id)) {
        return Some(display);
    }
    let mut named = displays.iter().filter(|display| !name.is_empty() && display.name == name && display.name != default_name(display.number));
    match (named.next(), named.next()) {
        (Some(display), None) => Some(display),
        _ => None,
    }
}

/// The screen with the window `foreground`, or the pointer's when there's no window.
fn active_monitor(foreground: HWND) -> HMONITOR {
    unsafe {
        if foreground.is_invalid() {
            pointer_monitor()
        } else {
            MonitorFromWindow(foreground, MONITOR_DEFAULTTONEAREST)
        }
    }
}

fn pointer_monitor() -> HMONITOR {
    unsafe {
        let mut point = POINT::default();
        let _ = GetCursorPos(&mut point);
        MonitorFromPoint(point, MONITOR_DEFAULTTOPRIMARY)
    }
}

/// The screen Windows marks as main. Its top left corner is always the desktop's (0, 0).
fn main_monitor() -> HMONITOR {
    unsafe { MonitorFromPoint(POINT { x: 0, y: 0 }, MONITOR_DEFAULTTOPRIMARY) }
}

/// Whether `monitor` is still the screen called `gdi`, so a handle kept from before can be used.
fn still_is(monitor: HMONITOR, gdi: &str) -> bool {
    monitor_info(monitor).is_some_and(|(info, _)| info == gdi)
}

/// The screen the launcher opens on, by the setting. `foreground` is the window in front of it.
pub fn launcher_monitor(choice: &LauncherScreen, foreground: HWND) -> HMONITOR {
    match choice {
        LauncherScreen::Active => active_monitor(foreground),
        LauncherScreen::Pointer => pointer_monitor(),
        LauncherScreen::Main => main_monitor(),
        LauncherScreen::Specific { id, name } => {
            for fresh in [false, true] {
                let displays = if fresh { list() } else { known() };
                let Some(display) = find(&displays, id, name) else { break };
                let monitor = HMONITOR(display.monitor as *mut _);
                if still_is(monitor, &display.gdi) {
                    return monitor;
                }
                // The screens changed without a word, so the list is read again, once.
                changed();
            }
            active_monitor(foreground)
        }
    }
}

/// The screen a window is on, as one of `displays`.
pub fn of_window(displays: &[Display], hwnd: HWND) -> Option<usize> {
    let monitor = unsafe { MonitorFromWindow(hwnd, MONITOR_DEFAULTTOPRIMARY) };
    let (gdi, _) = monitor_info(monitor)?;
    displays.iter().position(|display| display.gdi == gdi)
}

/// A screen's `\\.\DISPLAYn` name and its info.
fn monitor_info(monitor: HMONITOR) -> Option<(String, MONITORINFO)> {
    if monitor.is_invalid() {
        return None;
    }
    let mut info = MONITORINFOEXW { monitorInfo: MONITORINFO { cbSize: std::mem::size_of::<MONITORINFOEXW>() as u32, ..Default::default() }, ..Default::default() };
    unsafe { GetMonitorInfoW(monitor, &mut info.monitorInfo as *mut MONITORINFO).as_bool() }.then(|| (from_wide(&info.szDevice), info.monitorInfo))
}

fn from_wide(text: &[u16]) -> String {
    let end = text.iter().position(|unit| *unit == 0).unwrap_or(text.len());
    String::from_utf16_lossy(&text[..end])
}

/// The number at the end of `\\.\DISPLAY2`.
fn number_of(gdi: &str) -> Option<u32> {
    let digits = gdi.trim_end_matches(|c: char| !c.is_ascii_digit());
    let start = digits.rfind(|c: char| !c.is_ascii_digit()).map_or(0, |at| at + 1);
    digits[start..].parse().ok().filter(|number| *number > 0)
}

/// What Windows' display settings know about each screen that's on: its `\\.\DISPLAYn` name, the
/// monitor's path and the monitor's own name.
struct Named {
    gdi: String,
    path: String,
    name: String,
}

fn names() -> Vec<Named> {
    let mut paths: Vec<DISPLAYCONFIG_PATH_INFO> = Vec::new();
    let mut found = 0;
    unsafe {
        // The screens can change between asking how many there are and asking for them.
        for _ in 0..3 {
            let (mut path_count, mut mode_count) = (0u32, 0u32);
            if GetDisplayConfigBufferSizes(QDC_ONLY_ACTIVE_PATHS, &mut path_count, &mut mode_count) != ERROR_SUCCESS {
                return Vec::new();
            }
            paths = vec![DISPLAYCONFIG_PATH_INFO::default(); path_count as usize];
            let mut modes = vec![DISPLAYCONFIG_MODE_INFO::default(); mode_count as usize];
            let answer = QueryDisplayConfig(QDC_ONLY_ACTIVE_PATHS, &mut path_count, paths.as_mut_ptr(), &mut mode_count, modes.as_mut_ptr(), None);
            if answer == ERROR_SUCCESS {
                found = path_count as usize;
                break;
            }
            if answer != ERROR_INSUFFICIENT_BUFFER {
                return Vec::new();
            }
        }
    }
    let mut named: Vec<Named> = Vec::new();
    for path in paths.iter().take(found) {
        let mut source = DISPLAYCONFIG_SOURCE_DEVICE_NAME {
            header: DISPLAYCONFIG_DEVICE_INFO_HEADER {
                r#type: DISPLAYCONFIG_DEVICE_INFO_GET_SOURCE_NAME,
                size: std::mem::size_of::<DISPLAYCONFIG_SOURCE_DEVICE_NAME>() as u32,
                adapterId: path.sourceInfo.adapterId,
                id: path.sourceInfo.id,
            },
            ..Default::default()
        };
        let mut target = DISPLAYCONFIG_TARGET_DEVICE_NAME {
            header: DISPLAYCONFIG_DEVICE_INFO_HEADER {
                r#type: DISPLAYCONFIG_DEVICE_INFO_GET_TARGET_NAME,
                size: std::mem::size_of::<DISPLAYCONFIG_TARGET_DEVICE_NAME>() as u32,
                adapterId: path.targetInfo.adapterId,
                id: path.targetInfo.id,
            },
            ..Default::default()
        };
        unsafe {
            if DisplayConfigGetDeviceInfo(&mut source.header) != 0 || DisplayConfigGetDeviceInfo(&mut target.header) != 0 {
                continue;
            }
        }
        let gdi = from_wide(&source.viewGdiDeviceName);
        // Two monitors showing the same picture are one screen. The first one names it.
        if named.iter().any(|known| known.gdi == gdi) {
            continue;
        }
        let built_in = [DISPLAYCONFIG_OUTPUT_TECHNOLOGY_INTERNAL, DISPLAYCONFIG_OUTPUT_TECHNOLOGY_DISPLAYPORT_EMBEDDED, DISPLAYCONFIG_OUTPUT_TECHNOLOGY_UDI_EMBEDDED].contains(&target.outputTechnology);
        let own_name = from_wide(&target.monitorFriendlyDeviceName).trim().to_string();
        let name = if !own_name.is_empty() {
            own_name
        } else if built_in {
            "Built-in screen".to_string()
        } else {
            String::new()
        };
        named.push(Named { gdi, path: from_wide(&target.monitorDevicePath), name });
    }
    named
}

fn read() -> Vec<Display> {
    unsafe extern "system" fn add(monitor: HMONITOR, _: HDC, _: *mut RECT, list: LPARAM) -> BOOL {
        unsafe {
            let list = &mut *(list.0 as *mut Vec<(HMONITOR, String, MONITORINFO)>);
            if let Some((gdi, info)) = monitor_info(monitor) {
                list.push((monitor, gdi, info));
            }
        }
        BOOL(1)
    }
    let mut monitors: Vec<(HMONITOR, String, MONITORINFO)> = Vec::new();
    unsafe {
        let _ = EnumDisplayMonitors(None, None, Some(add), LPARAM(&mut monitors as *mut _ as isize));
    }
    let named = names();
    let mut displays: Vec<Display> = monitors
        .into_iter()
        .enumerate()
        .map(|(at, (monitor, gdi, info))| {
            let number = number_of(&gdi).unwrap_or(at as u32 + 1);
            let known = named.iter().find(|named| named.gdi.eq_ignore_ascii_case(&gdi));
            let id = known.map(|named| named.path.clone()).filter(|path| !path.is_empty()).unwrap_or_else(|| gdi.clone());
            let name = known.map(|named| named.name.clone()).filter(|name| !name.is_empty()).unwrap_or_else(|| default_name(number));
            Display { id, name, number, bounds: info.rcMonitor, primary: info.dwFlags & MONITORINFOF_PRIMARY != 0, gdi, monitor: monitor.0 as isize }
        })
        .collect();
    displays.sort_by_key(|display| (display.bounds.left, display.bounds.top));
    displays
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rect(left: i32, top: i32, width: i32, height: i32) -> RECT {
        RECT { left, top, right: left + width, bottom: top + height }
    }

    #[test]
    fn the_number_comes_from_windows_name_for_the_screen() {
        assert_eq!(number_of(r"\\.\DISPLAY1"), Some(1));
        assert_eq!(number_of(r"\\.\DISPLAY12"), Some(12));
        assert_eq!(number_of(r"\\.\DISPLAY"), None);
        assert_eq!(number_of(""), None);
    }

    #[test]
    fn a_specific_screen_is_found_by_its_path_and_then_by_a_name_only_it_has() {
        let mut moved = Display::stand_in(2, "VG259", rect(2560, 0, 1920, 1080), false);
        moved.id = r"\\?\DISPLAY#AUS2591#OTHERPORT".into();
        let displays = vec![Display::stand_in(1, "H27E6", rect(0, 0, 2560, 1440), true), moved, Display::stand_in(3, "Screen 3", rect(-1080, 0, 1080, 1920), false)];
        assert_eq!(find(&displays, r"\\?\DISPLAY#STANDIN1", "anything").map(|d| d.number), Some(1));
        assert_eq!(find(&displays, r"\\?\display#standin1", "").map(|d| d.number), Some(1), "Windows' paths ignore case");
        assert_eq!(find(&displays, r"\\?\DISPLAY#AUS2591#OLDPORT", "VG259").map(|d| d.number), Some(2), "moved to another port");
        assert_eq!(find(&displays, r"\\?\DISPLAY#GONE", "Screen 3"), None, "a made-up name never matches");
        assert_eq!(find(&displays, r"\\?\DISPLAY#GONE", ""), None);
    }

    #[test]
    fn two_monitors_with_the_same_name_are_never_guessed_between() {
        let displays = vec![Display::stand_in(1, "VG259", rect(0, 0, 1920, 1080), true), Display::stand_in(2, "VG259", rect(1920, 0, 1920, 1080), false)];
        assert_eq!(find(&displays, r"\\?\DISPLAY#GONE", "VG259"), None);
    }

    #[test]
    fn a_screen_says_its_number_name_size_and_whether_it_is_the_main_one() {
        assert_eq!(Display::stand_in(2, "VG259", rect(2560, 0, 1920, 1080), false).describe(), "Screen 2, VG259, 1920 \u{00D7} 1080");
        assert_eq!(Display::stand_in(1, "Screen 1", rect(0, 0, 2560, 1440), true).describe(), "Screen 1, 2560 \u{00D7} 1440, main screen");
    }

    #[test]
    fn the_real_screens_can_be_read() {
        let displays = read();
        assert!(!displays.is_empty(), "a test machine has a screen");
        assert_eq!(displays.iter().filter(|display| display.primary).count(), 1);
        for display in &displays {
            assert!(display.width() > 0 && display.height() > 0);
            let monitor = HMONITOR(display.monitor as *mut _);
            assert!(still_is(monitor, &display.gdi));
        }
        let first = &displays[0];
        let specific = LauncherScreen::Specific { id: first.id.clone(), name: first.name.clone() };
        assert_eq!(launcher_monitor(&specific, HWND::default()).0 as isize, first.monitor);
        let main = displays.iter().find(|display| display.primary).map(|display| display.monitor);
        assert_eq!(Some(launcher_monitor(&LauncherScreen::Main, HWND::default()).0 as isize), main, "the main screen is the one Windows marks");
    }
}
