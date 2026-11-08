//! Result rows as the host receives them: plain data with no pointers into anything that moves.

use orbit_data::{ClipItem, ClipKind, NoteSummary};
use std::ffi::{CString, c_char};
use std::ptr;

pub const GLYPH_SEARCH: u16 = 0;
pub const GLYPH_NOTE: u16 = 1;
pub const GLYPH_SNIPPET: u16 = 2;
pub const GLYPH_CLIPBOARD: u16 = 3;
pub const GLYPH_CALC: u16 = 4;
pub const GLYPH_LINK: u16 = 5;
pub const GLYPH_WEB: u16 = 6;
pub const GLYPH_PIN: u16 = 7;
pub const GLYPH_FOLDER: u16 = 8;
pub const GLYPH_COMMAND: u16 = 9;
pub const GLYPH_INFO: u16 = 10;
pub const GLYPH_IMAGE: u16 = 11;
pub const GLYPH_SETTINGS: u16 = 12;
pub const GLYPH_DIAGNOSTICS: u16 = 13;
pub const GLYPH_RELOAD: u16 = 14;
pub const GLYPH_PAUSE: u16 = 15;
pub const GLYPH_POWER: u16 = 16;
pub const GLYPH_LOCK: u16 = 17;
pub const GLYPH_MOON: u16 = 18;
pub const GLYPH_DELETE: u16 = 19;
pub const GLYPH_HELP: u16 = 20;
pub const GLYPH_TERMINAL: u16 = 21;

pub const OP_NONE: u16 = 0;
pub const OP_RUN_ITEM: u16 = 1;
pub const OP_COPY: u16 = 2;
pub const OP_OPEN_URL: u16 = 3;
pub const OP_NOTE_OPEN: u16 = 4;
pub const OP_SNIPPET_INSERT: u16 = 5;
pub const OP_CLIP_PASTE: u16 = 6;

pub const SOURCE_PLAIN: u16 = 0;
pub const SOURCE_CATALOG: u16 = 1;
pub const SOURCE_NOTE: u16 = 2;
pub const SOURCE_SNIPPET: u16 = 3;
pub const SOURCE_CLIP: u16 = 4;

pub const CLIP_TEXT: u16 = 0;
pub const CLIP_URL: u16 = 1;
pub const CLIP_FILES: u16 = 2;
pub const CLIP_IMAGE: u16 = 3;

pub fn clip_kind_code(kind: ClipKind) -> u16 {
    match kind {
        ClipKind::Text => CLIP_TEXT,
        ClipKind::Url => CLIP_URL,
        ClipKind::Files => CLIP_FILES,
        ClipKind::Image => CLIP_IMAGE,
    }
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Row {
    pub title: String,
    pub subtitle: String,
    pub kind: String,
    /// Path of the bundle whose icon this row shows, if any.
    pub icon_path: String,
    pub payload: String,
    pub key: String,
    pub id: i64,
    pub item: u32,
    pub glyph: u16,
    pub op: u16,
    pub source: u16,
    pub clip_kind: u16,
    pub accent: bool,
    pub pinned: bool,
}

#[repr(C)]
#[derive(Clone, Copy)]
pub struct OrbitRow {
    pub title: *const c_char,
    pub subtitle: *const c_char,
    pub kind: *const c_char,
    pub icon_path: *const c_char,
    pub payload: *const c_char,
    pub key: *const c_char,
    pub id: i64,
    pub item: u32,
    pub glyph: u16,
    pub op: u16,
    pub source: u16,
    pub clip_kind: u16,
    pub accent: bool,
    pub pinned: bool,
}

/// Interior NULs cannot cross a C string, so they are dropped. Nothing Orbit shows needs them.
pub fn cstring(text: &str) -> CString {
    CString::new(text).unwrap_or_else(|_| CString::new(text.replace('\0', "")).unwrap_or_default())
}

/// Owns the strings behind a slice of `OrbitRow`.
#[derive(Default)]
pub struct RowList {
    strings: Vec<CString>,
    rows: Vec<OrbitRow>,
}

impl RowList {
    pub fn set(&mut self, rows: &[Row]) {
        self.strings.clear();
        self.rows.clear();
        self.strings.reserve(rows.len() * 6);
        self.rows.reserve(rows.len());
        for row in rows {
            let mut hold = |text: &str| -> *const c_char {
                if text.is_empty() {
                    return c"".as_ptr();
                }
                let owned = cstring(text);
                // The heap buffer of a CString does not move when the CString itself is moved.
                let pointer = owned.as_ptr();
                self.strings.push(owned);
                pointer
            };
            let c = OrbitRow {
                title: hold(&row.title),
                subtitle: hold(&row.subtitle),
                kind: hold(&row.kind),
                icon_path: hold(&row.icon_path),
                payload: hold(&row.payload),
                key: hold(&row.key),
                id: row.id,
                item: row.item,
                glyph: row.glyph,
                op: row.op,
                source: row.source,
                clip_kind: row.clip_kind,
                accent: row.accent,
                pinned: row.pinned,
            };
            self.rows.push(c);
        }
    }

    pub fn as_ptr(&self) -> *const OrbitRow {
        if self.rows.is_empty() { ptr::null() } else { self.rows.as_ptr() }
    }

    pub fn len(&self) -> usize {
        self.rows.len()
    }
}

pub fn time_ago(unix: i64, now: i64) -> String {
    let age = (now - unix).max(0);
    match age {
        0..=59 => "just now".into(),
        60..=3599 => format!("{}m ago", age / 60),
        3600..=86_399 => format!("{}h ago", age / 3600),
        _ => format!("{}d ago", age / 86_400),
    }
}

pub fn preview(text: &str) -> String {
    let mut out = String::with_capacity(64);
    for (i, c) in text.chars().enumerate() {
        if i == 64 {
            out.push('…');
            break;
        }
        out.push(if c.is_control() { ' ' } else { c });
    }
    out
}

pub fn note_row(note: NoteSummary, now: i64) -> Row {
    let subtitle = if note.preview.is_empty() { time_ago(note.updated_at, now) } else { note.preview };
    Row {
        title: note.title,
        subtitle,
        kind: if note.pinned { "Pinned note" } else { "Note" }.into(),
        glyph: GLYPH_NOTE,
        accent: note.pinned,
        pinned: note.pinned,
        op: OP_NOTE_OPEN,
        source: SOURCE_NOTE,
        id: note.id,
        ..Row::default()
    }
}

pub fn snippet_row(id: i64, name: String, trigger: &str, content: &str) -> Row {
    let text = preview(content);
    let subtitle = if trigger.is_empty() { text } else { format!("{trigger}   {text}") };
    Row {
        title: name,
        subtitle,
        kind: "Snippet".into(),
        glyph: GLYPH_SNIPPET,
        op: OP_SNIPPET_INSERT,
        source: SOURCE_SNIPPET,
        id,
        ..Row::default()
    }
}

pub fn clip_row(clip: ClipItem, now: i64) -> Row {
    let (glyph, kind) = match clip.kind {
        ClipKind::Text => (GLYPH_CLIPBOARD, "Text"),
        ClipKind::Url => (GLYPH_LINK, "Link"),
        ClipKind::Files => (GLYPH_FOLDER, "Files"),
        ClipKind::Image => (GLYPH_IMAGE, "Image"),
    };
    let when = time_ago(clip.created_at, now);
    let subtitle = if clip.source_app.is_empty() { when } else { format!("{}, {when}", clip.source_app) };
    Row {
        title: clip.preview,
        subtitle,
        kind: kind.into(),
        glyph: if clip.pinned { GLYPH_PIN } else { glyph },
        accent: clip.pinned,
        pinned: clip.pinned,
        op: OP_CLIP_PASTE,
        source: SOURCE_CLIP,
        clip_kind: clip_kind_code(clip.kind),
        id: clip.id,
        ..Row::default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::ffi::CStr;

    #[test]
    fn row_pointers_survive_the_owning_vec_growing() {
        let mut list = RowList::default();
        let rows: Vec<Row> = (0..200).map(|i| Row { title: format!("row {i}"), ..Row::default() }).collect();
        list.set(&rows);
        assert_eq!(list.len(), 200);
        for i in 0..200 {
            let title = unsafe { CStr::from_ptr((*list.as_ptr().add(i)).title) };
            assert_eq!(title.to_str().unwrap(), format!("row {i}"));
        }
    }

    #[test]
    fn interior_nul_is_dropped_not_fatal() {
        assert_eq!(cstring("a\0b").to_str().unwrap(), "ab");
    }

    #[test]
    fn time_labels() {
        assert_eq!(time_ago(100, 40), "just now", "clock skew");
        assert_eq!(time_ago(0, 120), "2m ago");
        assert_eq!(time_ago(0, 172_800), "2d ago");
    }
}
