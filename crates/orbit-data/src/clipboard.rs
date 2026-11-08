//! Clipboard history.
//!
//! Dedupe rule: copying something already in history moves that entry to the top rather than
//! adding a second one. Rows hold a bounded preview, and queries fetch full text only when an
//! item is actually pasted.

use crate::{Result, Store, fts_query, make_preview, now_unix};
use rusqlite::{OptionalExtension, params};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ClipKind {
    Text,
    Url,
    Files,
    Image,
}

impl ClipKind {
    fn as_str(self) -> &'static str {
        match self {
            ClipKind::Text => "text",
            ClipKind::Url => "url",
            ClipKind::Files => "files",
            ClipKind::Image => "image",
        }
    }
    fn parse(text: &str) -> ClipKind {
        match text {
            "url" => ClipKind::Url,
            "files" => ClipKind::Files,
            "image" => ClipKind::Image,
            _ => ClipKind::Text,
        }
    }
}

pub struct NewClip {
    pub kind: ClipKind,
    /// The text itself, or newline-separated paths for files, or empty for images.
    pub text: String,
    pub source_app: String,
    pub blob_path: String,
    pub width: u32,
    pub height: u32,
    /// Used for images, whose identity is the pixel data rather than `text`.
    pub content_hash: Option<u64>,
    pub size: u64,
    /// The same text with its formatting, as the copying app gave it: Windows' "HTML Format" and
    /// RTF. Pasting puts them back, so the formatting comes along.
    pub formats: Formats,
}

/// A copy's formatting, kept beside its plain text.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Formats {
    pub html: Option<Vec<u8>>,
    pub rtf: Option<Vec<u8>>,
}

impl Formats {
    pub fn is_empty(&self) -> bool {
        self.html.is_none() && self.rtf.is_none()
    }
}

impl NewClip {
    pub fn text(text: String, source_app: &str) -> NewClip {
        let trimmed = text.trim();
        let is_url = (trimmed.starts_with("http://") || trimmed.starts_with("https://"))
            && !trimmed.contains(char::is_whitespace);
        let size = text.len() as u64;
        NewClip {
            kind: if is_url { ClipKind::Url } else { ClipKind::Text },
            text,
            source_app: source_app.to_string(),
            blob_path: String::new(),
            width: 0,
            height: 0,
            content_hash: None,
            size,
            formats: Formats::default(),
        }
    }

    /// Text that came with its formatting.
    pub fn with_formats(mut self, formats: Formats) -> NewClip {
        if self.kind != ClipKind::Image && self.kind != ClipKind::Files {
            self.formats = formats;
        }
        self
    }

    pub fn files(paths: &[String], source_app: &str) -> NewClip {
        let text = paths.join("\n");
        let size = text.len() as u64;
        NewClip {
            kind: ClipKind::Files,
            text,
            source_app: source_app.to_string(),
            blob_path: String::new(),
            width: 0,
            height: 0,
            content_hash: None,
            size,
            formats: Formats::default(),
        }
    }

    pub fn image(blob_path: String, width: u32, height: u32, size: u64, hash: u64, source_app: &str) -> NewClip {
        NewClip {
            kind: ClipKind::Image,
            text: String::new(),
            source_app: source_app.to_string(),
            blob_path,
            width,
            height,
            content_hash: Some(hash),
            size,
            formats: Formats::default(),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ClipItem {
    pub id: i64,
    pub kind: ClipKind,
    pub preview: String,
    pub source_app: String,
    pub created_at: i64,
    pub pinned: bool,
    pub size: i64,
    pub width: u32,
    pub height: u32,
    /// It has formatting as well as plain text.
    pub formatted: bool,
}

#[derive(Clone, Copy, Debug)]
pub struct RetentionPolicy {
    pub max_items: u32,
    /// Zero keeps items regardless of age.
    pub retention_days: u32,
}

/// FNV-1a. Not cryptographic: used only to find an identical earlier copy.
pub fn content_hash(data: &[u8]) -> u64 {
    let mut hash = 0xcbf2_9ce4_8422_2325u64;
    for &byte in data {
        hash ^= byte as u64;
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    hash
}

const PREVIEW_CHARS: usize = 160;
const COLUMNS: &str = "c.id, c.kind, c.preview, c.source_app, c.created_at, c.pinned, c.size, c.width, c.height, (c.html IS NOT NULL OR c.rtf IS NOT NULL)";

fn from_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<ClipItem> {
    Ok(ClipItem {
        id: row.get(0)?,
        kind: ClipKind::parse(&row.get::<_, String>(1)?),
        preview: row.get(2)?,
        source_app: row.get(3)?,
        created_at: row.get(4)?,
        pinned: row.get::<_, i64>(5)? != 0,
        size: row.get(6)?,
        width: row.get::<_, i64>(7)? as u32,
        height: row.get::<_, i64>(8)? as u32,
        formatted: row.get::<_, i64>(9)? != 0,
    })
}

impl Store {
    /// Records a copy and applies retention. Returns the id, or the existing id if identical.
    /// Returns removed image blob paths through `purged` so the caller can delete the files.
    pub fn add_clip(&self, clip: &NewClip, policy: &RetentionPolicy) -> Result<(i64, Vec<String>)> {
        let hash = clip.content_hash.unwrap_or_else(|| content_hash(clip.text.as_bytes())) as i64;
        let now = now_unix();
        let existing: Option<i64> = self
            .clip
            .prepare_cached("SELECT id FROM clip_items WHERE hash = ?1 AND kind = ?2 AND text = ?3 LIMIT 1")?
            .query_row(params![hash, clip.kind.as_str(), clip.text], |r| r.get(0))
            .optional()?;
        let id = if let Some(id) = existing {
            // The same text copied again takes the formatting it has this time, or none.
            self.clip
                .prepare_cached("UPDATE clip_items SET created_at = ?2, source_app = ?3, html = ?4, rtf = ?5 WHERE id = ?1")?
                .execute(params![id, now, clip.source_app, clip.formats.html, clip.formats.rtf])?;
            // created_at has one-second resolution, so re-insert ordering relies on the id
            // too: move the row to a fresh id to keep "most recent copy first" strict.
            self.bump_clip(id)?
        } else {
            let preview = match clip.kind {
                ClipKind::Image => format!("Image {}×{}", clip.width, clip.height),
                ClipKind::Files => {
                    let count = clip.text.lines().count();
                    let first = clip.text.lines().next().unwrap_or("");
                    if count > 1 {
                        make_preview(&format!("{count} files: {first}"), PREVIEW_CHARS)
                    } else {
                        make_preview(first, PREVIEW_CHARS)
                    }
                }
                _ => make_preview(&clip.text, PREVIEW_CHARS),
            };
            self.clip
                .prepare_cached(
                    "INSERT INTO clip_items(kind, text, preview, hash, size, source_app, created_at, blob_path, width, height, html, rtf)
                     VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12)",
                )?
                .execute(params![
                    clip.kind.as_str(),
                    clip.text,
                    preview,
                    hash,
                    clip.size as i64,
                    clip.source_app,
                    now,
                    clip.blob_path,
                    clip.width,
                    clip.height,
                    clip.formats.html,
                    clip.formats.rtf
                ])?;
            self.clip.last_insert_rowid()
        };
        let purged = self.apply_clip_retention(policy)?;
        Ok((id, purged))
    }

    fn bump_clip(&self, id: i64) -> Result<i64> {
        self.clip.execute(
            "INSERT INTO clip_items(kind, text, preview, hash, size, source_app, created_at, pinned, blob_path, width, height, html, rtf)
             SELECT kind, text, preview, hash, size, source_app, created_at, pinned, blob_path, width, height, html, rtf
             FROM clip_items WHERE id = ?1",
            [id],
        )?;
        let new_id = self.clip.last_insert_rowid();
        self.clip.execute("DELETE FROM clip_items WHERE id = ?1", [id])?;
        Ok(new_id)
    }

    /// Enforces age and count limits. Pinned items are exempt, since pinning means "keep".
    pub fn apply_clip_retention(&self, policy: &RetentionPolicy) -> Result<Vec<String>> {
        let mut blobs = Vec::new();
        {
            let mut stmt = self.clip.prepare_cached(
                "DELETE FROM clip_items WHERE pinned = 0 AND (
                    (?1 > 0 AND created_at < ?2)
                    OR id NOT IN (SELECT id FROM clip_items WHERE pinned = 0 ORDER BY created_at DESC, id DESC LIMIT ?3)
                 ) RETURNING blob_path",
            )?;
            let cutoff = now_unix() - policy.retention_days as i64 * 86_400;
            let rows = stmt.query_map(params![policy.retention_days, cutoff, policy.max_items], |r| r.get::<_, String>(0))?;
            for path in rows {
                let path = path?;
                if !path.is_empty() {
                    blobs.push(path);
                }
            }
        }
        Ok(blobs)
    }

    pub fn recent_clips(&self, limit: usize) -> Result<Vec<ClipItem>> {
        let sql = format!("SELECT {COLUMNS} FROM clip_items c ORDER BY c.pinned DESC, c.created_at DESC, c.id DESC LIMIT ?1");
        let mut stmt = self.clip.prepare_cached(&sql)?;
        stmt.query_map([limit as i64], from_row)?.collect()
    }

    pub fn search_clips(&self, query: &str, limit: usize) -> Result<Vec<ClipItem>> {
        let Some(fts) = fts_query(query) else {
            return Ok(Vec::new());
        };
        let sql = format!(
            "SELECT {COLUMNS} FROM clip_fts f JOIN clip_items c ON c.id = f.rowid
             WHERE clip_fts MATCH ?1 ORDER BY c.pinned DESC, c.created_at DESC, c.id DESC LIMIT ?2"
        );
        let mut stmt = self.clip.prepare_cached(&sql)?;
        stmt.query_map(params![fts, limit as i64], from_row)?.collect()
    }

    /// One item as the list shows it, or `None` once it's gone.
    pub fn clip_item(&self, id: i64) -> Result<Option<ClipItem>> {
        let sql = format!("SELECT {COLUMNS} FROM clip_items c WHERE c.id = ?1");
        self.clip.prepare_cached(&sql)?.query_row([id], from_row).optional()
    }

    /// Returns (kind, full text, blob path).
    pub fn clip_content(&self, id: i64) -> Result<Option<(ClipKind, String, String)>> {
        self.clip
            .prepare_cached("SELECT kind, text, blob_path FROM clip_items WHERE id = ?1")?
            .query_row([id], |r| Ok((ClipKind::parse(&r.get::<_, String>(0)?), r.get(1)?, r.get(2)?)))
            .optional()
    }

    /// The formatting kept with an item, if any. Read only when pasting with it.
    pub fn clip_formats(&self, id: i64) -> Result<Formats> {
        self.clip
            .prepare_cached("SELECT html, rtf FROM clip_items WHERE id = ?1")?
            .query_row([id], |r| Ok(Formats { html: r.get(0)?, rtf: r.get(1)? }))
            .optional()
            .map(Option::unwrap_or_default)
    }

    pub fn set_clip_pinned(&self, id: i64, pinned: bool) -> Result<()> {
        self.clip
            .prepare_cached("UPDATE clip_items SET pinned = ?2 WHERE id = ?1")?
            .execute(params![id, pinned as i64])?;
        Ok(())
    }

    pub fn delete_clip(&self, id: i64) -> Result<Option<String>> {
        let path: Option<String> = self
            .clip
            .prepare_cached("DELETE FROM clip_items WHERE id = ?1 RETURNING blob_path")?
            .query_row([id], |r| r.get(0))
            .optional()?;
        Ok(path.filter(|p| !p.is_empty()))
    }

    /// Deletes everything not pinned, or everything if `include_pinned`. Compacts the file so
    /// deleted content does not linger in free pages. Returns removed blob paths.
    pub fn clear_clips(&self, include_pinned: bool) -> Result<Vec<String>> {
        let mut blobs = Vec::new();
        {
            let mut stmt = self
                .clip
                .prepare("DELETE FROM clip_items WHERE pinned = 0 OR ?1 = 1 RETURNING blob_path")?;
            let rows = stmt.query_map([include_pinned as i64], |r| r.get::<_, String>(0))?;
            for path in rows {
                let path = path?;
                if !path.is_empty() {
                    blobs.push(path);
                }
            }
        }
        self.clip.execute_batch("INSERT INTO clip_fts(clip_fts) VALUES ('optimize'); VACUUM;")?;
        Ok(blobs)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const POLICY: RetentionPolicy = RetentionPolicy { max_items: 100, retention_days: 30 };

    fn add(s: &Store, text: &str) -> i64 {
        s.add_clip(&NewClip::text(text.into(), "test.exe"), &POLICY).unwrap().0
    }

    #[test]
    fn classifies_urls() {
        assert_eq!(NewClip::text("https://a.b/c".into(), "").kind, ClipKind::Url);
        assert_eq!(NewClip::text("see https://a.b".into(), "").kind, ClipKind::Text);
    }

    #[test]
    fn duplicate_moves_to_top_without_a_second_entry() {
        let s = Store::open_in_memory().unwrap();
        add(&s, "first");
        add(&s, "second");
        add(&s, "first");
        let recent = s.recent_clips(10).unwrap();
        assert_eq!(recent.len(), 2);
        assert_eq!(recent[0].preview, "first");
        assert_eq!(recent[1].preview, "second");
    }

    #[test]
    fn pins_survive_and_stay_on_top_after_duplicates() {
        let s = Store::open_in_memory().unwrap();
        let id = add(&s, "keep");
        s.set_clip_pinned(id, true).unwrap();
        add(&s, "other");
        add(&s, "keep");
        let recent = s.recent_clips(10).unwrap();
        assert_eq!(recent[0].preview, "keep");
        assert!(recent[0].pinned);
    }

    #[test]
    fn count_retention_spares_pins() {
        let s = Store::open_in_memory().unwrap();
        let policy = RetentionPolicy { max_items: 3, retention_days: 0 };
        let pinned = s.add_clip(&NewClip::text("pinned".into(), ""), &policy).unwrap().0;
        s.set_clip_pinned(pinned, true).unwrap();
        for i in 0..10 {
            s.add_clip(&NewClip::text(format!("item {i}"), ""), &policy).unwrap();
        }
        let recent = s.recent_clips(100).unwrap();
        assert_eq!(recent.len(), 4, "3 unpinned plus the pin");
        assert!(recent.iter().any(|c| c.pinned));
        assert_eq!(recent[1].preview, "item 9");
    }

    #[test]
    fn age_retention_removes_old_entries_and_reports_blobs() {
        let s = Store::open_in_memory().unwrap();
        let clip = NewClip::image("img/a.bin".into(), 10, 20, 800, 77, "");
        let id = s.add_clip(&clip, &POLICY).unwrap().0;
        s.clip.execute("UPDATE clip_items SET created_at = 1 WHERE id = ?1", [id]).unwrap();
        assert_eq!(s.apply_clip_retention(&POLICY).unwrap(), vec!["img/a.bin".to_string()]);
        assert!(s.recent_clips(10).unwrap().is_empty());
    }

    #[test]
    fn search_finds_text_and_deleted_text_leaves_the_index() {
        let s = Store::open_in_memory().unwrap();
        let id = add(&s, "the quick brown fox");
        assert_eq!(s.search_clips("quick", 5).unwrap()[0].id, id);
        s.delete_clip(id).unwrap();
        assert!(s.search_clips("quick", 5).unwrap().is_empty());
        assert_eq!(s.clip_content(id).unwrap(), None);
    }

    #[test]
    fn bumped_duplicates_stay_searchable_once() {
        let s = Store::open_in_memory().unwrap();
        add(&s, "needle text");
        add(&s, "needle text");
        add(&s, "needle text");
        assert_eq!(s.search_clips("needle", 5).unwrap().len(), 1);
    }

    #[test]
    fn clear_keeps_pins_unless_asked() {
        let s = Store::open_in_memory().unwrap();
        let id = add(&s, "keep");
        s.set_clip_pinned(id, true).unwrap();
        add(&s, "gone");
        s.clear_clips(false).unwrap();
        assert_eq!(s.recent_clips(5).unwrap().len(), 1);
        s.clear_clips(true).unwrap();
        assert!(s.recent_clips(5).unwrap().is_empty());
    }

    #[test]
    fn preview_is_bounded_but_full_text_is_kept() {
        let s = Store::open_in_memory().unwrap();
        let long = "x".repeat(100_000);
        let id = add(&s, &long);
        let item = &s.recent_clips(1).unwrap()[0];
        assert!(item.preview.chars().count() <= PREVIEW_CHARS + 1);
        assert_eq!(s.clip_content(id).unwrap().unwrap().1.len(), 100_000);
    }

    #[test]
    fn file_lists_summarise() {
        let s = Store::open_in_memory().unwrap();
        let clip = NewClip::files(&["C:\\a.txt".into(), "C:\\b.txt".into()], "");
        s.add_clip(&clip, &POLICY).unwrap();
        assert_eq!(s.recent_clips(1).unwrap()[0].preview, "2 files: C:\\a.txt");
    }

    #[test]
    fn formatting_is_kept_beside_the_text_and_follows_a_copy_again() {
        let s = Store::open_in_memory().unwrap();
        let formats = Formats { html: Some(b"<b>hi</b>".to_vec()), rtf: None };
        let id = s.add_clip(&NewClip::text("hi".into(), "").with_formats(formats.clone()), &POLICY).unwrap().0;
        assert!(s.recent_clips(1).unwrap()[0].formatted);
        assert_eq!(s.clip_formats(id).unwrap(), formats);
        let again = s.add_clip(&NewClip::text("hi".into(), ""), &POLICY).unwrap().0;
        assert!(!s.recent_clips(1).unwrap()[0].formatted, "copied again as plain text");
        assert_eq!(s.clip_formats(again).unwrap(), Formats::default());
        let pinned = s.add_clip(&NewClip::text("pin me".into(), "").with_formats(formats.clone()), &POLICY).unwrap().0;
        s.set_clip_pinned(pinned, true).unwrap();
        add(&s, "other");
        let moved = s.add_clip(&NewClip::text("pin me".into(), "").with_formats(formats.clone()), &POLICY).unwrap().0;
        assert_eq!(s.clip_formats(moved).unwrap(), formats, "moving to the top keeps it");
        assert!(NewClip::files(&["C:\\a".into()], "").with_formats(formats).formats.is_empty());
    }

    #[test]
    fn hash_is_stable() {
        assert_eq!(content_hash(b""), 0xcbf29ce484222325);
        assert_ne!(content_hash(b"a"), content_hash(b"b"));
    }
}
