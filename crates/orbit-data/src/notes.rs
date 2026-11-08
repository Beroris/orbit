//! Notes. There is no count limit anywhere in this module. The first line of the body is the
//! title, so "type, dismiss" is all a quick note needs.

use crate::{Result, Store, fts_query, make_preview, now_unix};
use rusqlite::{OptionalExtension, params};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NoteSummary {
    pub id: i64,
    pub title: String,
    pub preview: String,
    pub updated_at: i64,
    pub pinned: bool,
}

const PREVIEW_CHARS: usize = 80;
const TITLE_CHARS: usize = 120;

pub fn title_from_body(body: &str) -> String {
    let first = body.lines().map(str::trim).find(|line| !line.is_empty()).unwrap_or("");
    if first.is_empty() { "Untitled note".to_string() } else { make_preview(first, TITLE_CHARS) }
}

/// Preview text: whatever follows the title line.
fn after_title(body: &str) -> String {
    let body = body.trim_start();
    let rest = body.split_once('\n').map_or("", |(_, rest)| rest);
    make_preview(rest, PREVIEW_CHARS)
}

fn summary(row: &rusqlite::Row<'_>) -> rusqlite::Result<NoteSummary> {
    let body_head: String = row.get(2)?;
    Ok(NoteSummary {
        id: row.get(0)?,
        title: row.get(1)?,
        preview: after_title(&body_head),
        updated_at: row.get(3)?,
        pinned: row.get::<_, i64>(4)? != 0,
    })
}

// Only the first few KiB of the body are read for the preview, however long the note is.
const SUMMARY_COLUMNS: &str = "n.id, n.title, substr(n.body, 1, 2048), n.updated_at, n.pinned";

impl Store {
    pub fn create_note(&self, body: &str) -> Result<i64> {
        let now = now_unix();
        self.main.prepare_cached(
            "INSERT INTO notes(title, body, created_at, updated_at) VALUES (?1, ?2, ?3, ?3)",
        )?
        .execute(params![title_from_body(body), body, now])?;
        Ok(self.main.last_insert_rowid())
    }

    /// Saves in a single transaction, so a crash leaves either the old text or the new text.
    pub fn save_note(&self, id: i64, body: &str) -> Result<()> {
        self.main
            .prepare_cached("UPDATE notes SET title = ?2, body = ?3, updated_at = ?4 WHERE id = ?1")?
            .execute(params![id, title_from_body(body), body, now_unix()])?;
        Ok(())
    }

    pub fn note_body(&self, id: i64) -> Result<Option<String>> {
        self.main
            .prepare_cached("SELECT body FROM notes WHERE id = ?1 AND deleted_at IS NULL")?
            .query_row([id], |r| r.get(0))
            .optional()
    }

    pub fn recent_notes(&self, limit: usize) -> Result<Vec<NoteSummary>> {
        let sql = format!(
            "SELECT {SUMMARY_COLUMNS} FROM notes n WHERE n.deleted_at IS NULL AND n.archived = 0
             ORDER BY n.pinned DESC, n.updated_at DESC, n.id DESC LIMIT ?1"
        );
        let mut stmt = self.main.prepare_cached(&sql)?;
        stmt.query_map([limit as i64], summary)?.collect()
    }

    pub fn search_notes(&self, query: &str, limit: usize) -> Result<Vec<NoteSummary>> {
        let Some(fts) = fts_query(query) else {
            return Ok(Vec::new());
        };
        let sql = format!(
            "SELECT {SUMMARY_COLUMNS} FROM notes_fts f JOIN notes n ON n.id = f.rowid
             WHERE notes_fts MATCH ?1 AND n.deleted_at IS NULL AND n.archived = 0
             ORDER BY bm25(notes_fts, 10.0, 1.0), n.id DESC LIMIT ?2"
        );
        let mut stmt = self.main.prepare_cached(&sql)?;
        stmt.query_map(params![fts, limit as i64], summary)?.collect()
    }

    pub fn set_note_pinned(&self, id: i64, pinned: bool) -> Result<()> {
        self.main
            .prepare_cached("UPDATE notes SET pinned = ?2 WHERE id = ?1")?
            .execute(params![id, pinned as i64])?;
        Ok(())
    }

    pub fn trash_note(&self, id: i64) -> Result<()> {
        self.main
            .prepare_cached("UPDATE notes SET deleted_at = ?2 WHERE id = ?1")?
            .execute(params![id, now_unix()])?;
        Ok(())
    }

    pub fn restore_note(&self, id: i64) -> Result<()> {
        self.main.prepare_cached("UPDATE notes SET deleted_at = NULL WHERE id = ?1")?.execute([id])?;
        Ok(())
    }

    pub fn recently_trashed_note(&self) -> Result<Option<i64>> {
        self.main
            .prepare_cached("SELECT id FROM notes WHERE deleted_at IS NOT NULL ORDER BY deleted_at DESC LIMIT 1")?
            .query_row([], |r| r.get(0))
            .optional()
    }

    /// Removes notes trashed more than `days` ago.
    pub fn purge_trash(&self, days: u32) -> Result<usize> {
        let cutoff = now_unix() - days as i64 * 86_400;
        self.main
            .prepare_cached("DELETE FROM notes WHERE deleted_at IS NOT NULL AND deleted_at < ?1")?
            .execute([cutoff])
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn store() -> Store {
        Store::open_in_memory().unwrap()
    }

    #[test]
    fn title_comes_from_first_nonempty_line() {
        assert_eq!(title_from_body("\n\n  Ask about turbo  \nmore"), "Ask about turbo");
        assert_eq!(title_from_body("   \n "), "Untitled note");
        assert_eq!(after_title("Title\nsecond line\nthird"), "second line third");
    }

    #[test]
    fn create_search_edit_cycle() {
        let s = store();
        let id = s.create_note("Ask about turbo fitment and compare costs").unwrap();
        assert_eq!(s.search_notes("turbo", 5).unwrap()[0].id, id);
        assert_eq!(s.search_notes("TUR", 5).unwrap()[0].id, id, "prefix and case-insensitive");
        s.save_note(id, "Changed entirely").unwrap();
        assert!(s.search_notes("turbo", 5).unwrap().is_empty(), "old text leaves the index");
        assert_eq!(s.search_notes("changed", 5).unwrap()[0].title, "Changed entirely");
        assert_eq!(s.note_body(id).unwrap().unwrap(), "Changed entirely");
    }

    #[test]
    fn there_is_no_note_limit() {
        let s = store();
        for i in 0..500 {
            s.create_note(&format!("note number {i}")).unwrap();
        }
        assert_eq!(s.counts().unwrap().0, 500);
    }

    #[test]
    fn body_matches_and_title_matches_outrank() {
        let s = store();
        let body_hit = s.create_note("Shopping\nbuy a turbo").unwrap();
        let title_hit = s.create_note("Turbo plans\nsomething").unwrap();
        let hits = s.search_notes("turbo", 5).unwrap();
        assert_eq!(hits[0].id, title_hit);
        assert_eq!(hits[1].id, body_hit);
    }

    #[test]
    fn hostile_search_text_cannot_break_queries() {
        let s = store();
        s.create_note("plain").unwrap();
        for q in ["\"", "'", "*", "AND", "OR NOT", "NEAR(", "a:b", "col:x", "---", "()", "\0", "); DROP TABLE notes;--"] {
            assert!(s.search_notes(q, 5).is_ok(), "{q:?}");
        }
        assert_eq!(s.counts().unwrap().0, 1);
    }

    #[test]
    fn diacritics_match_both_ways() {
        let s = store();
        s.create_note("Résumé notes").unwrap();
        assert_eq!(s.search_notes("resume", 5).unwrap().len(), 1);
        assert_eq!(s.search_notes("RÉSUMÉ", 5).unwrap().len(), 1);
    }

    #[test]
    fn pinned_first_and_trash_hides_then_restores() {
        let s = store();
        let a = s.create_note("a").unwrap();
        let b = s.create_note("b").unwrap();
        s.set_note_pinned(a, true).unwrap();
        assert_eq!(s.recent_notes(5).unwrap()[0].id, a);
        s.trash_note(b).unwrap();
        assert_eq!(s.recent_notes(5).unwrap().len(), 1);
        assert_eq!(s.note_body(b).unwrap(), None);
        assert_eq!(s.recently_trashed_note().unwrap(), Some(b));
        s.restore_note(b).unwrap();
        assert_eq!(s.recent_notes(5).unwrap().len(), 2);
    }

    #[test]
    fn purge_only_removes_old_trash() {
        let s = store();
        let id = s.create_note("x").unwrap();
        s.trash_note(id).unwrap();
        assert_eq!(s.purge_trash(30).unwrap(), 0);
        s.main.execute("UPDATE notes SET deleted_at = 1 WHERE id = ?1", [id]).unwrap();
        assert_eq!(s.purge_trash(30).unwrap(), 1);
    }

    #[test]
    fn preview_reads_only_the_head_of_huge_notes() {
        let s = store();
        let huge = format!("Big\n{}", "word ".repeat(1_000_000));
        let id = s.create_note(&huge).unwrap();
        let summary = &s.recent_notes(1).unwrap()[0];
        assert_eq!(summary.id, id);
        assert!(summary.preview.chars().count() <= PREVIEW_CHARS + 1);
    }
}
