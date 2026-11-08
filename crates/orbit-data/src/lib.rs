//! Orbit's local storage.
//!
//! `Store` is owned by one worker thread and never touched from a UI or input thread. Records
//! stay on disk: queries return small summaries with bounded previews, and the page cache is
//! capped, so RAM does not grow with the number of notes, snippets or clipboard entries.

mod clipboard;
mod export;
mod notes;
mod snippets;
mod usage;

pub use clipboard::{ClipItem, ClipKind, Formats, NewClip, RetentionPolicy};
pub use export::{ExportBundle, ExportedNote, ExportedSnippet, ImportSummary, MAX_IMPORT_BYTES};
pub use notes::{NoteSummary, title_from_body};
pub use snippets::{Snippet, SnippetInput};
pub use usage::UsageRecord;

use rusqlite::Connection;
use std::path::{Path, PathBuf};

pub type Result<T> = std::result::Result<T, rusqlite::Error>;

const MAIN_MIGRATIONS: &[&str] = &[r#"
    CREATE TABLE notes (
        id INTEGER PRIMARY KEY,
        title TEXT NOT NULL,
        body TEXT NOT NULL,
        created_at INTEGER NOT NULL,
        updated_at INTEGER NOT NULL,
        pinned INTEGER NOT NULL DEFAULT 0,
        archived INTEGER NOT NULL DEFAULT 0,
        deleted_at INTEGER
    );
    CREATE INDEX notes_recent ON notes(deleted_at, archived, pinned DESC, updated_at DESC, id DESC);
    CREATE VIRTUAL TABLE notes_fts USING fts5(
        title, body, content='notes', content_rowid='id',
        tokenize='unicode61 remove_diacritics 2'
    );
    CREATE TRIGGER notes_ai AFTER INSERT ON notes BEGIN
        INSERT INTO notes_fts(rowid, title, body) VALUES (new.id, new.title, new.body);
    END;
    CREATE TRIGGER notes_ad AFTER DELETE ON notes BEGIN
        INSERT INTO notes_fts(notes_fts, rowid, title, body) VALUES ('delete', old.id, old.title, old.body);
    END;
    CREATE TRIGGER notes_au AFTER UPDATE OF title, body ON notes BEGIN
        INSERT INTO notes_fts(notes_fts, rowid, title, body) VALUES ('delete', old.id, old.title, old.body);
        INSERT INTO notes_fts(rowid, title, body) VALUES (new.id, new.title, new.body);
    END;

    CREATE TABLE snippets (
        id INTEGER PRIMARY KEY,
        name TEXT NOT NULL,
        trigger TEXT NOT NULL DEFAULT '',
        content TEXT NOT NULL,
        description TEXT NOT NULL DEFAULT '',
        category TEXT NOT NULL DEFAULT '',
        favourite INTEGER NOT NULL DEFAULT 0,
        enabled INTEGER NOT NULL DEFAULT 1,
        created_at INTEGER NOT NULL,
        updated_at INTEGER NOT NULL
    );
    CREATE INDEX snippets_recent ON snippets(favourite DESC, updated_at DESC);
    CREATE VIRTUAL TABLE snippets_fts USING fts5(
        name, trigger, content, description, category, content='snippets', content_rowid='id',
        tokenize='unicode61 remove_diacritics 2'
    );
    CREATE TRIGGER snippets_ai AFTER INSERT ON snippets BEGIN
        INSERT INTO snippets_fts(rowid, name, trigger, content, description, category)
        VALUES (new.id, new.name, new.trigger, new.content, new.description, new.category);
    END;
    CREATE TRIGGER snippets_ad AFTER DELETE ON snippets BEGIN
        INSERT INTO snippets_fts(snippets_fts, rowid, name, trigger, content, description, category)
        VALUES ('delete', old.id, old.name, old.trigger, old.content, old.description, old.category);
    END;
    CREATE TRIGGER snippets_au AFTER UPDATE ON snippets BEGIN
        INSERT INTO snippets_fts(snippets_fts, rowid, name, trigger, content, description, category)
        VALUES ('delete', old.id, old.name, old.trigger, old.content, old.description, old.category);
        INSERT INTO snippets_fts(rowid, name, trigger, content, description, category)
        VALUES (new.id, new.name, new.trigger, new.content, new.description, new.category);
    END;

    CREATE TABLE command_usage (
        key TEXT PRIMARY KEY,
        count INTEGER NOT NULL,
        last_used_at INTEGER NOT NULL
    ) WITHOUT ROWID;
"#];

const CLIP_MIGRATIONS: &[&str] = &[r#"
    CREATE TABLE clip_items (
        id INTEGER PRIMARY KEY,
        kind TEXT NOT NULL,
        text TEXT NOT NULL,
        preview TEXT NOT NULL,
        hash INTEGER NOT NULL,
        size INTEGER NOT NULL,
        source_app TEXT NOT NULL DEFAULT '',
        created_at INTEGER NOT NULL,
        pinned INTEGER NOT NULL DEFAULT 0,
        blob_path TEXT NOT NULL DEFAULT '',
        width INTEGER NOT NULL DEFAULT 0,
        height INTEGER NOT NULL DEFAULT 0
    );
    CREATE INDEX clip_recent ON clip_items(pinned DESC, created_at DESC, id DESC);
    CREATE INDEX clip_hash ON clip_items(hash);
    CREATE VIRTUAL TABLE clip_fts USING fts5(
        text, content='clip_items', content_rowid='id',
        tokenize='unicode61 remove_diacritics 2'
    );
    CREATE TRIGGER clip_ai AFTER INSERT ON clip_items BEGIN
        INSERT INTO clip_fts(rowid, text) VALUES (new.id, new.text);
    END;
    CREATE TRIGGER clip_ad AFTER DELETE ON clip_items BEGIN
        INSERT INTO clip_fts(clip_fts, rowid, text) VALUES ('delete', old.id, old.text);
    END;
"#, r#"
    ALTER TABLE clip_items ADD COLUMN html BLOB;
    ALTER TABLE clip_items ADD COLUMN rtf BLOB;
"#];

fn tune(conn: &Connection, on_disk: bool) -> Result<()> {
    // A hard 1 MiB page cache per connection and no memory mapping, so resident memory
    // does not scale with database size.
    conn.execute_batch("PRAGMA cache_size = -1024; PRAGMA mmap_size = 0; PRAGMA temp_store = MEMORY; PRAGMA foreign_keys = ON;")?;
    if on_disk {
        conn.execute_batch("PRAGMA journal_mode = WAL; PRAGMA synchronous = NORMAL; PRAGMA wal_autocheckpoint = 256; PRAGMA journal_size_limit = 1048576;")?;
    }
    Ok(())
}

/// Applies each migration once, in one transaction per step, tracked in `user_version`.
fn migrate(conn: &mut Connection, steps: &[&str]) -> Result<()> {
    let current: u32 = conn.query_row("PRAGMA user_version", [], |r| r.get(0))?;
    if current as usize > steps.len() {
        // Written by a newer Orbit. Leave it alone rather than guessing at its shape.
        return Err(rusqlite::Error::InvalidQuery);
    }
    // An upgrade of existing data gets a copy first. Restoring it is the rollback: replace the
    // database with the copy and run the older Orbit again.
    if current > 0 && (current as usize) < steps.len() {
        if let Some(path) = conn.path().filter(|path| !path.is_empty()).map(String::from) {
            let backup = format!("{path}.before-v{}.bak", steps.len());
            let _ = std::fs::remove_file(&backup);
            conn.execute("VACUUM INTO ?1", [&backup])?;
        }
    }
    for (index, step) in steps.iter().enumerate().skip(current as usize) {
        let tx = conn.transaction()?;
        tx.execute_batch(step)?;
        tx.execute_batch(&format!("PRAGMA user_version = {}", index + 1))?;
        tx.commit()?;
    }
    Ok(())
}

pub struct Store {
    pub(crate) main: Connection,
    pub(crate) clip: Connection,
    clip_persistent: bool,
    dir: PathBuf,
}

impl Store {
    /// Opens or creates the databases in `dir`. With `persist_clipboard` false, history is
    /// kept in an in-memory database and vanishes when the process exits.
    pub fn open(dir: &Path, persist_clipboard: bool) -> Result<Store> {
        let mut main = Connection::open(dir.join("orbit.db"))?;
        tune(&main, true)?;
        migrate(&mut main, MAIN_MIGRATIONS)?;
        let clip = open_clip(dir, persist_clipboard)?;
        Ok(Store { main, clip, clip_persistent: persist_clipboard, dir: dir.to_path_buf() })
    }

    /// In-memory store for tests and benchmarks.
    pub fn open_in_memory() -> Result<Store> {
        let mut main = Connection::open_in_memory()?;
        tune(&main, false)?;
        migrate(&mut main, MAIN_MIGRATIONS)?;
        let mut clip = Connection::open_in_memory()?;
        tune(&clip, false)?;
        migrate(&mut clip, CLIP_MIGRATIONS)?;
        Ok(Store { main, clip, clip_persistent: false, dir: PathBuf::new() })
    }

    /// Switches clipboard history between disk and memory. Turning persistence off deletes
    /// the on-disk history: the user asked for it not to be kept.
    pub fn set_clipboard_persistence(&mut self, persist: bool) -> Result<()> {
        if persist == self.clip_persistent || self.dir.as_os_str().is_empty() {
            return Ok(());
        }
        self.clip = open_clip(&self.dir, persist)?;
        self.clip_persistent = persist;
        if !persist {
            for suffix in ["", "-wal", "-shm"] {
                let _ = std::fs::remove_file(self.dir.join(format!("clipboard.db{suffix}")));
            }
            let _ = std::fs::remove_dir_all(self.dir.join("clipboard-images"));
        }
        Ok(())
    }

    /// Releases SQLite's cached pages under memory pressure.
    pub fn shed_memory(&self) {
        let _ = self.main.execute_batch("PRAGMA shrink_memory;");
        let _ = self.clip.execute_batch("PRAGMA shrink_memory;");
    }

    /// Writes pending WAL pages and lets SQLite update its statistics. For clean shutdown.
    pub fn close_maintenance(&self) {
        let _ = self.main.execute_batch("PRAGMA optimize; PRAGMA wal_checkpoint(TRUNCATE);");
        let _ = self.clip.execute_batch("PRAGMA optimize;");
    }

    pub fn counts(&self) -> Result<(i64, i64, i64)> {
        let notes = self.main.query_row("SELECT count(*) FROM notes WHERE deleted_at IS NULL", [], |r| r.get(0))?;
        let snippets = self.main.query_row("SELECT count(*) FROM snippets", [], |r| r.get(0))?;
        let clips = self.clip.query_row("SELECT count(*) FROM clip_items", [], |r| r.get(0))?;
        Ok((notes, snippets, clips))
    }

    /// Bytes SQLite currently holds in RAM for this process.
    pub fn sqlite_memory_bytes() -> i64 {
        unsafe { rusqlite::ffi::sqlite3_memory_used() }
    }
}

fn open_clip(dir: &Path, persist: bool) -> Result<Connection> {
    let mut clip = if persist {
        Connection::open(dir.join("clipboard.db"))?
    } else {
        Connection::open_in_memory()?
    };
    tune(&clip, persist)?;
    migrate(&mut clip, CLIP_MIGRATIONS)?;
    Ok(clip)
}

/// Builds a safe FTS5 prefix query from user text: every token is quoted, so operators, quotes
/// and stray punctuation in the input cannot produce a syntax error or change the query's meaning.
pub(crate) fn fts_query(input: &str) -> Option<String> {
    let mut out = String::new();
    for token in input.split(|c: char| !c.is_alphanumeric()) {
        if token.is_empty() {
            continue;
        }
        if !out.is_empty() {
            out.push(' ');
        }
        out.push('"');
        out.push_str(token);
        out.push_str("\"*");
    }
    if out.is_empty() { None } else { Some(out) }
}

pub(crate) fn now_unix() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

/// Short single-line preview. Cuts on a character boundary, since a byte cut could split UTF-8.
pub(crate) fn make_preview(text: &str, max_chars: usize) -> String {
    let mut out = String::with_capacity(max_chars.min(text.len()) + 1);
    let mut count = 0;
    let mut last_was_space = true;
    for c in text.chars() {
        if count == max_chars {
            out.push('…');
            break;
        }
        let c = if c.is_whitespace() || c.is_control() { ' ' } else { c };
        if c == ' ' && last_was_space {
            continue;
        }
        last_was_space = c == ' ';
        out.push(c);
        count += 1;
    }
    let trimmed = out.trim_end().len();
    out.truncate(trimmed);
    out
}

/// Removes the database files, their journals and the copies made before upgrades. The store must
/// already be closed. Only these names are touched, since the data folder can be set to anywhere.
pub fn remove_database_files(dir: &Path) {
    for base in ["orbit.db", "clipboard.db"] {
        for suffix in ["", "-wal", "-shm"] {
            let _ = std::fs::remove_file(dir.join(format!("{base}{suffix}")));
        }
    }
    let Ok(entries) = std::fs::read_dir(dir) else { return };
    for entry in entries.flatten() {
        let name = entry.file_name();
        let name = name.to_string_lossy();
        let is_upgrade_copy = name.ends_with(".bak")
            && (name.starts_with("orbit.db.before-v") || name.starts_with("clipboard.db.before-v"));
        if is_upgrade_copy {
            let _ = std::fs::remove_file(entry.path());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn removing_database_files_leaves_everything_else() {
        let dir = std::env::temp_dir().join(format!("orbit-remove-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        for name in ["orbit.db", "orbit.db-wal", "clipboard.db-shm", "orbit.db.before-v3.bak", "clipboard.db.before-v2.bak", "keep.bak", "keep.txt"] {
            std::fs::write(dir.join(name), b"x").unwrap();
        }
        remove_database_files(&dir);
        let mut left: Vec<String> = std::fs::read_dir(&dir).unwrap().map(|e| e.unwrap().file_name().to_string_lossy().into_owned()).collect();
        left.sort();
        assert_eq!(left, ["keep.bak", "keep.txt"]);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn upgrading_existing_data_keeps_a_copy_of_the_old_database() {
        let dir = std::env::temp_dir().join(format!("orbit-migrate-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("test.db");
        const V1: &str = "CREATE TABLE t (a TEXT); INSERT INTO t VALUES ('old');";
        const V2: &str = "ALTER TABLE t ADD COLUMN b TEXT;";
        {
            let mut conn = Connection::open(&path).unwrap();
            migrate(&mut conn, &[V1]).unwrap();
            assert!(!dir.join("test.db.before-v1.bak").exists(), "a new database has nothing to keep");
            migrate(&mut conn, &[V1, V2]).unwrap();
            conn.query_row("SELECT b FROM t", [], |_| Ok(())).unwrap();
        }
        let backup = Connection::open(dir.join("test.db.before-v2.bak")).unwrap();
        let version: u32 = backup.query_row("PRAGMA user_version", [], |r| r.get(0)).unwrap();
        assert_eq!(version, 1);
        let old: String = backup.query_row("SELECT a FROM t", [], |r| r.get(0)).unwrap();
        assert_eq!(old, "old");
        assert!(backup.query_row("SELECT b FROM t", [], |_| Ok(())).is_err());
        drop(backup);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn fts_query_quotes_everything() {
        assert_eq!(fts_query("turbo cost").as_deref(), Some("\"turbo\"* \"cost\"*"));
        assert_eq!(fts_query("a\"b OR c*").as_deref(), Some("\"a\"* \"b\"* \"OR\"* \"c\"*"));
        assert_eq!(fts_query("NEAR(x y)").as_deref(), Some("\"NEAR\"* \"x\"* \"y\"*"));
        assert_eq!(fts_query(" \" * ( ) "), None);
        assert_eq!(fts_query(""), None);
    }

    #[test]
    fn preview_is_bounded_and_single_line() {
        assert_eq!(make_preview("  a\n\n b\tc  ", 50), "a b c");
        assert_eq!(make_preview("abcdef", 3), "abc…");
        assert_eq!(make_preview("日本語テキスト", 3), "日本語…");
    }

    #[test]
    fn migrations_are_idempotent_and_reopen_keeps_data() {
        let dir = std::env::temp_dir().join(format!("orbit-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        {
            let store = Store::open(&dir, true).unwrap();
            store.create_note("survives reopen").unwrap();
        }
        {
            let store = Store::open(&dir, true).unwrap();
            assert_eq!(store.search_notes("survives", 5).unwrap().len(), 1);
        }
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn newer_database_is_refused_not_damaged() {
        let dir = std::env::temp_dir().join(format!("orbit-test-newer-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        drop(Store::open(&dir, true).unwrap());
        {
            let conn = Connection::open(dir.join("orbit.db")).unwrap();
            conn.execute_batch("PRAGMA user_version = 999").unwrap();
        }
        assert!(Store::open(&dir, true).is_err());
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn turning_persistence_off_removes_history_from_disk() {
        let dir = std::env::temp_dir().join(format!("orbit-test-clip-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let mut store = Store::open(&dir, true).unwrap();
        assert!(dir.join("clipboard.db").exists());
        store.set_clipboard_persistence(false).unwrap();
        assert!(!dir.join("clipboard.db").exists());
        drop(store);
        std::fs::remove_dir_all(&dir).ok();
    }

    /// The Phase 3 gate. With 10,000 records of each kind, a store that was just opened holds
    /// only its capped page caches in RAM, and queries answer quickly.
    #[test]
    fn ten_thousand_records_stay_on_disk() {
        let dir = std::env::temp_dir().join(format!("orbit-test-10k-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        {
            let store = Store::open(&dir, true).unwrap();
            let body = "lorem ipsum dolor sit amet ".repeat(40);
            store.main.execute_batch("BEGIN").unwrap();
            for i in 0..10_000 {
                store.create_note(&format!("Note {i}\n{body} marker{i}")).unwrap();
                store
                    .create_snippet(&SnippetInput {
                        name: format!("Snippet {i}"),
                        trigger: format!(";s{i}"),
                        content: body.clone(),
                        ..Default::default()
                    })
                    .unwrap();
            }
            store.main.execute_batch("COMMIT").unwrap();
            store.clip.execute_batch("BEGIN").unwrap();
            let policy = RetentionPolicy { max_items: 20_000, retention_days: 0 };
            for i in 0..10_000 {
                store
                    .add_clip(&NewClip::text(format!("clip {i} {body}"), "test.exe"), &policy)
                    .unwrap();
            }
            store.clip.execute_batch("COMMIT").unwrap();
            store.close_maintenance();
        }
        let before = Store::sqlite_memory_bytes();
        let store = Store::open(&dir, true).unwrap();
        assert_eq!(store.counts().unwrap(), (10_000, 10_000, 10_000));
        assert_eq!(store.search_notes("marker7777", 5).unwrap().len(), 1);
        assert_eq!(store.search_snippets("Snippet 7777", 5).unwrap().len() >= 1, true);
        assert!(!store.search_clips("clip", 8).unwrap().is_empty());
        let grown = Store::sqlite_memory_bytes() - before;
        // Tests run in parallel and the counter is global, so this bound is generous. Loading
        // the records would take over 30 MiB: 30,000 records of about 1 KiB each.
        assert!(grown < 16 * 1024 * 1024, "SQLite grew by {grown} bytes");
        drop(store);
        std::fs::remove_dir_all(&dir).ok();
    }
}
