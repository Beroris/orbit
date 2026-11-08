//! A developer's view of the forms, as image files, that nobody sees happen.
//!
//! `orbit --data-dir <folder> --preview-settings <what> <out.png>` opens one form far off-screen,
//! in a window that can't take focus, captures it and exits. This runs before anything global:
//! no single-instance check, no hotkeys, hooks, tray icon, data thread or database.
//!
//! `<what>` is a Settings page, a page in a state (`hotkeys-filled`, `hotkeys-text`,
//! `hotkeys-recording`, `hotkeys-search`, `hotkeys-bad-text`, `general-listening`,
//! `general-problem`, `general-saved`, `general-failed`, `data-armed`), or `link-form`,
//! `command-form` or `snippet-form`. Add `-new` to a form for a blank one, or `-error` to a link or
//! command form to see it complain.

use crate::editor::EditorContext;
use crate::forms;
use crate::util::SendHwnd;
use crate::worker::{DataRow, Req};
use orbit_core::Settings;
use orbit_core::settings::{Appearance, LoadOutcome};
use std::path::{Path, PathBuf};
use windows::Win32::Foundation::{HWND, RECT};
use windows::Win32::Graphics::Gdi::*;
use windows::Win32::UI::WindowsAndMessaging::*;

/// `PW_RENDERFULLCONTENT`: ask for what's really on the window, not just what `WM_PRINT` draws.
const RENDER_FULL_CONTENT: u32 = 2;

#[link(name = "user32")]
unsafe extern "system" {
    // Declared here so the whole `Storage_Xps` feature isn't pulled in for one call.
    fn PrintWindow(hwnd: HWND, dc: HDC, flags: u32) -> windows::core::BOOL;
}

#[derive(Default)]
pub struct SettingsPreview {
    pub what: String,
    pub out: PathBuf,
    pub scroll: i32,
    pub theme: Option<Appearance>,
    pub empty_snippets: bool,
    pub arm_delete: bool,
    pub selfcheck: Option<PathBuf>,
}

fn stand_in_snippets() -> Vec<DataRow> {
    vec![
        DataRow::Snippet { id: 1, name: "Email sign-off".into(), trigger: "sig".into(), preview: "Thanks for your help, and speak soon.".into() },
        DataRow::Snippet { id: 2, name: "Home address".into(), trigger: "addr".into(), preview: "1 Example Road, Sampletown".into() },
        DataRow::Snippet { id: 3, name: "Today's date, with a long name that needs to be cut short at some point".into(), trigger: "today-with-a-long-keyword".into(), preview: "{date}".into() },
        DataRow::Snippet { id: 4, name: "No keyword".into(), trigger: String::new(), preview: "Only inserted from the launcher.".into() },
    ]
}

fn stand_in_snippet() -> orbit_data::Snippet {
    orbit_data::Snippet {
        id: 1,
        name: "Email sign-off".into(),
        trigger: "sig".into(),
        content: "Thanks for your help.\nSent on {date}.\n{cursor}".into(),
        description: "How I end most emails".into(),
        category: "Work".into(),
        favourite: true,
        enabled: true,
    }
}

/// Handles waiting messages for a moment, so everything has painted.
fn settle(milliseconds: u64) {
    let until = std::time::Instant::now() + std::time::Duration::from_millis(milliseconds);
    unsafe {
        let mut message = MSG::default();
        while std::time::Instant::now() < until {
            while PeekMessageW(&mut message, None, 0, 0, PM_REMOVE).as_bool() {
                let _ = TranslateMessage(&message);
                DispatchMessageW(&message);
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        }    }
}

/// The whole window, title bar included, as top-down BGRA rows.
fn capture(hwnd: HWND) -> Option<(i32, i32, Vec<u8>)> {
    unsafe {
        let mut rect = RECT::default();
        GetWindowRect(hwnd, &mut rect).ok()?;
        let (width, height) = (rect.right - rect.left, rect.bottom - rect.top);
        let screen = GetDC(None);
        let dc = CreateCompatibleDC(Some(screen));
        let bitmap = CreateCompatibleBitmap(screen, width, height);
        let previous = SelectObject(dc, bitmap.into());
        let ok = PrintWindow(hwnd, dc, RENDER_FULL_CONTENT).as_bool();
        SelectObject(dc, previous);
        let mut info = BITMAPINFO {
            bmiHeader: BITMAPINFOHEADER {
                biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
                biWidth: width,
                biHeight: -height,
                biPlanes: 1,
                biBitCount: 32,
                biCompression: BI_RGB.0,
                ..Default::default()
            },
            ..Default::default()
        };
        let mut pixels = vec![0u8; (width * height * 4) as usize];
        GetDIBits(dc, bitmap, 0, height as u32, Some(pixels.as_mut_ptr() as *mut _), &mut info, DIB_RGB_COLORS);
        let _ = DeleteObject(bitmap.into());
        let _ = DeleteDC(dc);
        ReleaseDC(None, screen);
        ok.then_some((width, height, pixels))
    }
}

/// Cuts a window's picture down to its client area.
fn inside_frame(hwnd: HWND, width: i32, height: i32, pixels: Vec<u8>) -> (i32, i32, Vec<u8>) {
    unsafe {
        let mut window = RECT::default();
        let mut client = RECT::default();
        if GetWindowRect(hwnd, &mut window).is_err() || windows::Win32::UI::WindowsAndMessaging::GetClientRect(hwnd, &mut client).is_err() {
            return (width, height, pixels);
        }
        let mut origin = windows::Win32::Foundation::POINT::default();
        let _ = windows::Win32::Graphics::Gdi::ClientToScreen(hwnd, &mut origin);
        let (left, top) = ((origin.x - window.left).clamp(0, width), (origin.y - window.top).clamp(0, height));
        let (inner_width, inner_height) = (client.right.min(width - left), client.bottom.min(height - top));
        let mut out = Vec::with_capacity((inner_width * inner_height * 4) as usize);
        for row in top..top + inner_height {
            let start = ((row * width + left) * 4) as usize;
            out.extend_from_slice(&pixels[start..start + (inner_width * 4) as usize]);
        }
        (inner_width, inner_height, out)
    }
}

fn crc32(bytes: &[u8]) -> u32 {
    let mut crc = !0u32;
    for byte in bytes {
        crc ^= *byte as u32;
        for _ in 0..8 {
            crc = if crc & 1 != 0 { (crc >> 1) ^ 0xEDB8_8320 } else { crc >> 1 };
        }
    }
    !crc
}

fn adler32(bytes: &[u8]) -> u32 {
    let (mut a, mut b) = (1u32, 0u32);
    for byte in bytes {
        a = (a + *byte as u32) % 65521;
        b = (b + a) % 65521;
    }
    (b << 16) | a
}

/// A PNG with no compression: simple, and these files don't stay around.
pub fn encode_png(width: u32, height: u32, bgra: &[u8]) -> Vec<u8> {
    let mut raw = Vec::with_capacity((width * height * 3 + height) as usize);
    for row in bgra.chunks_exact(width as usize * 4) {
        raw.push(0);
        for pixel in row.chunks_exact(4) {
            raw.extend_from_slice(&[pixel[2], pixel[1], pixel[0]]);
        }
    }
    let mut zlib = vec![0x78, 0x01];
    let mut blocks = raw.chunks(65535).peekable();
    while let Some(block) = blocks.next() {
        zlib.push(u8::from(blocks.peek().is_none()));
        zlib.extend_from_slice(&(block.len() as u16).to_le_bytes());
        zlib.extend_from_slice(&(!(block.len() as u16)).to_le_bytes());
        zlib.extend_from_slice(block);
    }
    zlib.extend_from_slice(&adler32(&raw).to_be_bytes());
    let mut out = vec![0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A];
    let mut chunk = |name: &[u8; 4], data: &[u8]| {
        out.extend_from_slice(&(data.len() as u32).to_be_bytes());
        let mut body = name.to_vec();
        body.extend_from_slice(data);
        out.extend_from_slice(&body);
        out.extend_from_slice(&crc32(&body).to_be_bytes());
    };
    let mut header = Vec::with_capacity(13);
    header.extend_from_slice(&width.to_be_bytes());
    header.extend_from_slice(&height.to_be_bytes());
    header.extend_from_slice(&[8, 2, 0, 0, 0]);
    chunk(b"IHDR", &header);
    chunk(b"IDAT", &zlib);
    chunk(b"IEND", &[]);
    out
}

/// `None` for a settings file that doesn't parse, so a stand-in with a typo can't pass unnoticed.
fn load(dir: &Path) -> Option<Settings> {
    let text = std::fs::read_to_string(dir.join("settings.json")).ok()?;
    let (settings, outcome) = Settings::parse(&text);
    if let LoadOutcome::Corrupt(reason) = outcome {
        let _ = std::fs::write(dir.join("settings-problem.txt"), reason);
        return None;
    }
    Some(settings)
}

/// Opens the form asked for, captures it and gives the exit code.
pub fn run(preview: SettingsPreview, dir: Option<PathBuf>) -> i32 {
    // Never the real data folder. It must be named, and it's only read.
    let Some(dir) = dir else { return 2 };
    forms::set_preview(true);
    let Some(mut settings) = load(&dir) else { return 7 };
    if let Some(theme) = preview.theme {
        settings.appearance = theme;
    }
    let theme = crate::ui::resolve_theme(settings.appearance);
    // Nothing answers: the receiver just stays open while the form is up.
    let (tx, _rx) = std::sync::mpsc::channel::<Req>();
    let context = EditorContext { tx, main: SendHwnd::new(HWND::default()), theme };
    let (kind, blank, complain) = match preview.what.as_str() {
        what if what.ends_with("-new") => (what.trim_end_matches("-new"), true, false),
        what if what.ends_with("-error") => (what.trim_end_matches("-error"), true, true),
        what => (what, false, false),
    };
    let (links, commands) = (settings.links.clone(), settings.custom_commands.clone());
    match kind {
        "link-form" => {
            let existing = if blank { None } else { links.first().cloned() };
            crate::link_form::open_link(context, existing, links, commands);
        }
        "command-form" => {
            let existing = if blank { None } else { commands.first().cloned() };
            crate::link_form::open_command(context, existing, links, commands);
        }
        "snippet-form" => crate::editor::open_snippet(context, if blank { None } else { Some(stand_in_snippet()) }),
        page => {
            let (page, filled, as_text) = match page {
                "hotkeys-filled" => ("hotkeys", true, false),
                "hotkeys-text" => ("hotkeys", true, true),
                "hotkeys-recording" | "hotkeys-held" | "hotkeys-replace" | "hotkeys-hover" | "hotkeys-narrow" | "hotkeys-replace-narrow" | "hotkeys-search" | "hotkeys-bad-text" => ("hotkeys", true, false),
                "general-listening" | "general-problem" | "general-saved" | "general-failed" | "general-narrow" => ("general", false, false),
                "data-armed" | "data-narrow" => ("data", false, false),
                "search-filled" | "search-alias" | "search-record" => ("search", false, false),
                other => (other, false, false),
            };
            let state = match kind {
                "hotkeys-recording" => "recording",
                "hotkeys-held" => "held",
                "hotkeys-hover" => "hover",
                "hotkeys-replace" | "hotkeys-replace-narrow" => "replace",
                "hotkeys-search" => "search",
                "hotkeys-bad-text" => "text-error",
                "search-filled" => "search-filled",
                "search-alias" => "search-alias",
                "search-record" => "search-record",
                "general-listening" => "listening",
                "general-problem" => "problem",
                "general-saved" => "saved",
                "general-failed" => "failed",
                "data-armed" => "armed",
                _ => "",
            };
            let Some(index) = crate::settings_win::preview_page_index(page) else { return 3 };
            if kind.ends_with("-narrow") {
                crate::settings_win::preview_limit_width(640);
            }
            crate::settings_win::preview_open(context, settings, dir, index);
            if !preview.empty_snippets {
                crate::settings_win::snippets_listed(stand_in_snippets());
            }
            if filled {
                crate::settings_win::preview_fill_hotkeys();
            }
            if as_text {
                crate::settings_win::preview_hotkeys_as_text();
            }
            if preview.arm_delete {
                crate::settings_win::preview_arm_first_link();
            }
            crate::settings_win::preview_state(state);
            crate::settings_win::preview_scroll(preview.scroll);
        }
    }
    if complain {
        crate::link_form::preview_submit();
    }
    settle(250);
    #[cfg(debug_assertions)]
    if let Some(path) = &preview.selfcheck {
        let lines = crate::settings_win::preview_selfcheck();
        let _ = std::fs::write(path, lines.join("\n") + "\n");
        return 0;
    }
    let Some(hwnd) = forms::last_form() else { return 4 };
    let Some((width, height, pixels)) = capture(hwnd) else { return 5 };
    // Settings has no frame of its own on screen, so its picture is only what's inside the frame.
    let (width, height, pixels) = if crate::settings_win::is_settings(hwnd) { inside_frame(hwnd, width, height, pixels) } else { (width, height, pixels) };
    match std::fs::write(&preview.out, encode_png(width as u32, height as u32, &pixels)) {
        Ok(()) => 0,
        Err(_) => 6,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn checksums_match_known_values() {
        assert_eq!(crc32(b"IEND"), 0xAE42_6082);
        assert_eq!(adler32(b"Wikipedia"), 0x11E6_0398);
    }

    #[test]
    fn a_png_has_the_parts_a_reader_expects() {
        let png = encode_png(2, 1, &[1, 2, 3, 255, 4, 5, 6, 255]);
        assert_eq!(&png[..8], &[0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A]);
        assert_eq!(&png[12..16], b"IHDR");
        assert_eq!(&png[16..20], &2u32.to_be_bytes());
        assert_eq!(&png[png.len() - 8..png.len() - 4], b"IEND");
    }
}
