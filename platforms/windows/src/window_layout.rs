//! Moving the window that was in front: halves, maximised, centred, or onto the next display.
//! Only ever the one window, and only when asked. Full-screen windows, such as games, are left
//! alone, and Windows itself refuses for windows running as administrator.

use windows::Win32::Foundation::{HWND, LPARAM, RECT};
use windows::Win32::Graphics::Dwm::{DWMWA_EXTENDED_FRAME_BOUNDS, DwmGetWindowAttribute};
use windows::Win32::Graphics::Gdi::{EnumDisplayMonitors, GetMonitorInfoW, HDC, HMONITOR, MONITOR_DEFAULTTONEAREST, MONITORINFO, MonitorFromWindow};
use windows::Win32::UI::WindowsAndMessaging::{
    GA_ROOT, GetAncestor, GetClassNameW, GetWindowRect, IsIconic, IsWindow, IsWindowVisible, IsZoomed, SW_MAXIMIZE, SW_RESTORE, SWP_NOACTIVATE,
    SWP_NOZORDER, SetWindowPos, ShowWindow,
};
use windows::core::BOOL;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Layout {
    Left,
    Right,
    Top,
    Bottom,
    Maximize,
    AlmostMaximize,
    Center,
    NextDisplay,
}

/// A rectangle as left, top, right, bottom.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Area {
    pub left: i32,
    pub top: i32,
    pub right: i32,
    pub bottom: i32,
}

impl Area {
    fn from(rect: RECT) -> Area {
        Area { left: rect.left, top: rect.top, right: rect.right, bottom: rect.bottom }
    }
    fn width(self) -> i32 {
        self.right - self.left
    }
    fn height(self) -> i32 {
        self.bottom - self.top
    }
}

/// Where a window goes in a display's work area. `current` is where it is now, for centring.
pub fn place(layout: Layout, work: Area, current: Area) -> Area {
    let (w, h) = (work.width(), work.height());
    match layout {
        Layout::Left => Area { right: work.left + w / 2, ..work },
        Layout::Right => Area { left: work.left + w / 2, ..work },
        Layout::Top => Area { bottom: work.top + h / 2, ..work },
        Layout::Bottom => Area { top: work.top + h / 2, ..work },
        Layout::Maximize | Layout::NextDisplay => work,
        Layout::AlmostMaximize => {
            let (dx, dy) = (w / 20, h / 20);
            Area { left: work.left + dx, top: work.top + dy, right: work.right - dx, bottom: work.bottom - dy }
        }
        Layout::Center => {
            let (cw, ch) = (current.width().min(w), current.height().min(h));
            let (left, top) = (work.left + (w - cw) / 2, work.top + (h - ch) / 2);
            Area { left, top, right: left + cw, bottom: top + ch }
        }
    }
}

/// The same place on another display, scaled to its work area.
pub fn carry(current: Area, from: Area, to: Area) -> Area {
    let x = |value: i32| to.left + ((value - from.left) as i64 * to.width() as i64 / from.width().max(1) as i64) as i32;
    let y = |value: i32| to.top + ((value - from.top) as i64 * to.height() as i64 / from.height().max(1) as i64) as i32;
    Area { left: x(current.left), top: y(current.top), right: x(current.right), bottom: y(current.bottom) }
}

fn class_name(hwnd: HWND) -> String {
    let mut buffer = [0u16; 64];
    let length = unsafe { GetClassNameW(hwnd, &mut buffer) };
    String::from_utf16_lossy(&buffer[..length.max(0) as usize])
}

fn monitor_of(hwnd: HWND) -> Option<(HMONITOR, Area, Area)> {
    unsafe {
        let monitor = MonitorFromWindow(hwnd, MONITOR_DEFAULTTONEAREST);
        let mut info = MONITORINFO { cbSize: std::mem::size_of::<MONITORINFO>() as u32, ..Default::default() };
        GetMonitorInfoW(monitor, &mut info).as_bool().then(|| (monitor, Area::from(info.rcMonitor), Area::from(info.rcWork)))
    }
}

/// Every display's work area, left to right.
fn displays() -> Vec<(HMONITOR, Area)> {
    unsafe extern "system" fn add(monitor: HMONITOR, _: HDC, _: *mut RECT, list: LPARAM) -> BOOL {
        unsafe {
            let list = &mut *(list.0 as *mut Vec<(HMONITOR, Area)>);
            let mut info = MONITORINFO { cbSize: std::mem::size_of::<MONITORINFO>() as u32, ..Default::default() };
            if GetMonitorInfoW(monitor, &mut info).as_bool() {
                list.push((monitor, Area::from(info.rcWork)));
            }
        }
        BOOL(1)
    }
    let mut list: Vec<(HMONITOR, Area)> = Vec::new();
    unsafe {
        let _ = EnumDisplayMonitors(None, None, Some(add), LPARAM(&mut list as *mut _ as isize));
    }
    list.sort_by_key(|(_, area)| (area.left, area.top));
    list
}

/// Puts the window `hwnd` where `layout` says. The message says why, when it couldn't.
pub fn apply(hwnd: HWND, layout: Layout) -> Result<(), String> {
    unsafe {
        let hwnd = if hwnd.is_invalid() { hwnd } else { GetAncestor(hwnd, GA_ROOT) };
        if hwnd.is_invalid() || !IsWindow(Some(hwnd)).as_bool() || !IsWindowVisible(hwnd).as_bool() {
            return Err("There's no window to move. Open Orbit over the window you want to move.".into());
        }
        if matches!(class_name(hwnd).as_str(), "Progman" | "WorkerW" | "Shell_TrayWnd" | "Shell_SecondaryTrayWnd") {
            return Err("There's no window to move. Open Orbit over the window you want to move.".into());
        }
        let Some((monitor, screen, work)) = monitor_of(hwnd) else { return Err("Windows didn't say which display the window is on.".into()) };
        let mut outer = RECT::default();
        GetWindowRect(hwnd, &mut outer).map_err(|_| "Windows didn't say where the window is.".to_string())?;
        // A window that covers its whole display without being maximised is full screen, such as a
        // game or a video. Those are left as they are.
        let zoomed = IsZoomed(hwnd).as_bool();
        if !zoomed && !IsIconic(hwnd).as_bool() && Area::from(outer) == screen {
            return Err("Orbit doesn't move full-screen windows.".into());
        }
        if layout == Layout::Maximize {
            let _ = ShowWindow(hwnd, SW_MAXIMIZE);
            return Ok(());
        }
        let (target_work, from_work) = if layout == Layout::NextDisplay {
            let all = displays();
            if all.len() < 2 {
                return Err("There's only one display.".into());
            }
            let at = all.iter().position(|(known, _)| *known == monitor).unwrap_or(0);
            (all[(at + 1) % all.len()].1, work)
        } else {
            (work, work)
        };
        if zoomed || IsIconic(hwnd).as_bool() {
            let _ = ShowWindow(hwnd, SW_RESTORE);
            let _ = GetWindowRect(hwnd, &mut outer);
        }
        // Windows 10 and 11 frames have invisible borders for resizing. What you see is the frame
        // DWM draws, so the borders are added back on around where it should go.
        let mut seen = outer;
        let _ = DwmGetWindowAttribute(hwnd, DWMWA_EXTENDED_FRAME_BOUNDS, &mut seen as *mut _ as *mut _, std::mem::size_of::<RECT>() as u32);
        let current = Area::from(seen);
        let goal = if layout == Layout::NextDisplay { carry(current, from_work, target_work) } else { place(layout, target_work, current) };
        let (left, top, right, bottom) = (seen.left - outer.left, seen.top - outer.top, outer.right - seen.right, outer.bottom - seen.bottom);
        let moved = SetWindowPos(
            hwnd,
            None,
            goal.left - left,
            goal.top - top,
            goal.width() + left + right,
            goal.height() + top + bottom,
            SWP_NOZORDER | SWP_NOACTIVATE,
        );
        if moved.is_err() {
            return Err("Windows wouldn't let Orbit move it. It may be running as administrator.".into());
        }
        if layout == Layout::NextDisplay && zoomed {
            let _ = ShowWindow(hwnd, SW_MAXIMIZE);
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const WORK: Area = Area { left: 0, top: 0, right: 1920, bottom: 1040 };

    #[test]
    fn halves_split_the_work_area() {
        let any = Area { left: 100, top: 100, right: 500, bottom: 400 };
        assert_eq!(place(Layout::Left, WORK, any), Area { left: 0, top: 0, right: 960, bottom: 1040 });
        assert_eq!(place(Layout::Right, WORK, any), Area { left: 960, top: 0, right: 1920, bottom: 1040 });
        assert_eq!(place(Layout::Top, WORK, any), Area { left: 0, top: 0, right: 1920, bottom: 520 });
        assert_eq!(place(Layout::Bottom, WORK, any), Area { left: 0, top: 520, right: 1920, bottom: 1040 });
    }

    #[test]
    fn centring_keeps_the_size_and_almost_maximising_leaves_a_margin() {
        let window = Area { left: 10, top: 10, right: 810, bottom: 610 };
        assert_eq!(place(Layout::Center, WORK, window), Area { left: 560, top: 220, right: 1360, bottom: 820 });
        let huge = Area { left: 0, top: 0, right: 4000, bottom: 3000 };
        assert_eq!(place(Layout::Center, WORK, huge), WORK);
        assert_eq!(place(Layout::AlmostMaximize, WORK, window), Area { left: 96, top: 52, right: 1824, bottom: 988 });
    }

    #[test]
    fn moving_to_another_display_keeps_the_same_place_on_it() {
        let second = Area { left: 1920, top: 0, right: 1920 + 2560, bottom: 1400 };
        let left_half = Area { left: 0, top: 0, right: 960, bottom: 1040 };
        assert_eq!(carry(left_half, WORK, second), Area { left: 1920, top: 0, right: 1920 + 1280, bottom: 1400 });
    }
}
