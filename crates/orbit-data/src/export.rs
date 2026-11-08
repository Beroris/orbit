//! Export and import in a documented, versioned JSON format. See docs/DATA_PORTABILITY.md.
//! Clipboard history is deliberately not part of this bundle.

use crate::{Result, SnippetInput, Store};
use serde::{Deserialize, Serialize};

pub const BUNDLE_VERSION: u32 = 1;
/// Imports larger than this are refused before parsing.
pub const MAX_IMPORT_BYTES: u64 = 256 * 1024 * 1024;

#[derive(Debug, Serialize, Deserialize)]
pub struct ExportBundle {
    pub format: String,
    pub version: u32,
    pub exported_at: i64,
    pub notes: Vec<ExportedNote>,
    pub snippets: Vec<ExportedSnippet>,
}

#[derive(Debug, Serialize, Deserialize, PartialEq)]
pub struct ExportedNote {
    pub body: String,
    pub created_at: i64,
    pub updated_at: i64,
    #[serde(default)]
    pub pinned: bool,
}

#[derive(Debug, Serialize, Deserialize, PartialEq)]
pub struct ExportedSnippet {
    pub name: String,
    #[serde(default)]
    pub trigger: String,
    pub content: String,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub category: String,
    #[serde(default)]
    pub favourite: bool,
}

#[derive(Debug, Default, PartialEq, Eq)]
pub struct ImportSummary {
    pub notes_added: usize,
    pub notes_skipped: usize,
    pub snippets_added: usize,
    pub snippets_skipped: usize,
}

#[derive(Debug)]
pub enum ImportError {
    TooLarge,
    Malformed(String),
    NewerFormat(u32),
    Storage(rusqlite::Error),
}

impl std::fmt::Display for ImportError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ImportError::TooLarge => write!(f, "file is larger than the import limit"),
            ImportError::Malformed(why) => write!(f, "not a valid Orbit export: {why}"),
            ImportError::NewerFormat(v) => write!(f, "export format {v} is newer than this Orbit"),
            ImportError::Storage(error) => write!(f, "storage error: {error}"),
        }
    }
}

impl Store {
    pub fn export_bundle(&self) -> Result<ExportBundle> {
        let notes = {
            let mut stmt = self.main.prepare(
                "SELECT body, created_at, updated_at, pinned FROM notes WHERE deleted_at IS NULL ORDER BY id",
            )?;
            stmt.query_map([], |r| {
                Ok(ExportedNote {
                    body: r.get(0)?,
                    created_at: r.get(1)?,
                    updated_at: r.get(2)?,
                    pinned: r.get::<_, i64>(3)? != 0,
                })
            })?
            .collect::<Result<Vec<_>>>()?
        };
        let snippets = {
            let mut stmt = self.main.prepare(
                "SELECT name, trigger, content, description, category, favourite FROM snippets ORDER BY id",
            )?;
            stmt.query_map([], |r| {
                Ok(ExportedSnippet {
                    name: r.get(0)?,
                    trigger: r.get(1)?,
                    content: r.get(2)?,
                    description: r.get(3)?,
                    category: r.get(4)?,
                    favourite: r.get::<_, i64>(5)? != 0,
                })
            })?
            .collect::<Result<Vec<_>>>()?
        };
        Ok(ExportBundle {
            format: "orbit-export".into(),
            version: BUNDLE_VERSION,
            exported_at: crate::now_unix(),
            notes,
            snippets,
        })
    }

    /// Merges a bundle into the store in one transaction: it applies fully or not at all.
    /// Records identical to existing ones are skipped, so importing twice is harmless. Imported
    /// text is stored as plain data and nothing in a bundle is ever executed or used as a path.
    pub fn import_bundle(&self, json: &str) -> std::result::Result<ImportSummary, ImportError> {
        if json.len() as u64 > MAX_IMPORT_BYTES {
            return Err(ImportError::TooLarge);
        }
        let bundle: ExportBundle =
            serde_json::from_str(json).map_err(|e| ImportError::Malformed(e.to_string()))?;
        if bundle.format != "orbit-export" {
            return Err(ImportError::Malformed(format!("format is {:?}", bundle.format)));
        }
        if bundle.version > BUNDLE_VERSION {
            return Err(ImportError::NewerFormat(bundle.version));
        }
        let mut summary = ImportSummary::default();
        let mut run = || -> Result<()> {
            self.main.execute_batch("BEGIN IMMEDIATE")?;
            for note in &bundle.notes {
                let exists: bool = self.main.query_row(
                    "SELECT EXISTS(SELECT 1 FROM notes WHERE body = ?1 AND deleted_at IS NULL)",
                    [&note.body],
                    |r| r.get(0),
                )?;
                if exists {
                    summary.notes_skipped += 1;
                    continue;
                }
                self.main.execute(
                    "INSERT INTO notes(title, body, created_at, updated_at, pinned) VALUES (?1, ?2, ?3, ?4, ?5)",
                    rusqlite::params![
                        crate::title_from_body(&note.body),
                        note.body,
                        note.created_at,
                        note.updated_at,
                        note.pinned as i64
                    ],
                )?;
                summary.notes_added += 1;
            }
            for snippet in &bundle.snippets {
                let exists: bool = self.main.query_row(
                    "SELECT EXISTS(SELECT 1 FROM snippets WHERE name = ?1 AND content = ?2)",
                    [&snippet.name, &snippet.content],
                    |r| r.get(0),
                )?;
                if exists {
                    summary.snippets_skipped += 1;
                    continue;
                }
                self.create_snippet(&SnippetInput {
                    name: snippet.name.clone(),
                    trigger: snippet.trigger.clone(),
                    content: snippet.content.clone(),
                    description: snippet.description.clone(),
                    category: snippet.category.clone(),
                    favourite: snippet.favourite,
                    enabled: true,
                })?;
                summary.snippets_added += 1;
            }
            self.main.execute_batch("COMMIT")
        };
        if let Err(error) = run() {
            let _ = self.main.execute_batch("ROLLBACK");
            return Err(ImportError::Storage(error));
        }
        Ok(summary)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn seeded() -> Store {
        let s = Store::open_in_memory().unwrap();
        let id = s.create_note("Pinned one\nbody").unwrap();
        s.set_note_pinned(id, true).unwrap();
        let trashed = s.create_note("in the trash").unwrap();
        s.trash_note(trashed).unwrap();
        s.create_snippet(&SnippetInput {
            name: "Sig".into(),
            trigger: ";sig".into(),
            content: "Regards".into(),
            ..Default::default()
        })
        .unwrap();
        s
    }

    #[test]
    fn round_trips_and_is_idempotent() {
        let source = seeded();
        let bundle = source.export_bundle().unwrap();
        assert_eq!(bundle.notes.len(), 1, "trash is not exported");
        let json = serde_json::to_string(&bundle).unwrap();

        let target = Store::open_in_memory().unwrap();
        let first = target.import_bundle(&json).unwrap();
        assert_eq!((first.notes_added, first.snippets_added), (1, 1));
        assert!(target.recent_notes(1).unwrap()[0].pinned);
        assert_eq!(target.search_notes("pinned", 5).unwrap().len(), 1);

        let second = target.import_bundle(&json).unwrap();
        assert_eq!((second.notes_added, second.notes_skipped), (0, 1));
        assert_eq!((second.snippets_added, second.snippets_skipped), (0, 1));
    }

    #[test]
    fn export_never_contains_clipboard_data() {
        let s = seeded();
        let policy = crate::RetentionPolicy { max_items: 10, retention_days: 0 };
        s.add_clip(&crate::NewClip::text("hunter2".into(), ""), &policy).unwrap();
        let json = serde_json::to_string(&s.export_bundle().unwrap()).unwrap();
        assert!(!json.contains("hunter2"));
    }

    #[test]
    fn rejects_bad_input_without_changes() {
        let s = Store::open_in_memory().unwrap();
        assert!(matches!(s.import_bundle("not json"), Err(ImportError::Malformed(_))));
        assert!(matches!(
            s.import_bundle(r#"{"format":"other","version":1,"exported_at":0,"notes":[],"snippets":[]}"#),
            Err(ImportError::Malformed(_))
        ));
        assert!(matches!(
            s.import_bundle(r#"{"format":"orbit-export","version":99,"exported_at":0,"notes":[],"snippets":[]}"#),
            Err(ImportError::NewerFormat(99))
        ));
        assert_eq!(s.counts().unwrap(), (0, 0, 0));
    }
}
