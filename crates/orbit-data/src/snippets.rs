//! Snippets. Unlimited, stored locally, searchable by name, trigger and content.

use crate::{Result, Store, fts_query, now_unix};
use rusqlite::{OptionalExtension, params};

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct SnippetInput {
    pub name: String,
    pub trigger: String,
    pub content: String,
    pub description: String,
    pub category: String,
    pub favourite: bool,
    pub enabled: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Snippet {
    pub id: i64,
    pub name: String,
    pub trigger: String,
    pub content: String,
    pub description: String,
    pub category: String,
    pub favourite: bool,
    pub enabled: bool,
}

const COLUMNS: &str = "s.id, s.name, s.trigger, s.content, s.description, s.category, s.favourite, s.enabled";

fn from_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<Snippet> {
    Ok(Snippet {
        id: row.get(0)?,
        name: row.get(1)?,
        trigger: row.get(2)?,
        content: row.get(3)?,
        description: row.get(4)?,
        category: row.get(5)?,
        favourite: row.get::<_, i64>(6)? != 0,
        enabled: row.get::<_, i64>(7)? != 0,
    })
}

impl Store {
    pub fn create_snippet(&self, input: &SnippetInput) -> Result<i64> {
        let now = now_unix();
        self.main
            .prepare_cached(
                "INSERT INTO snippets(name, trigger, content, description, category, favourite, enabled, created_at, updated_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, 1, ?7, ?7)",
            )?
            .execute(params![input.name, input.trigger, input.content, input.description, input.category, input.favourite as i64, now])?;
        Ok(self.main.last_insert_rowid())
    }

    pub fn update_snippet(&self, id: i64, input: &SnippetInput) -> Result<()> {
        self.main
            .prepare_cached(
                "UPDATE snippets SET name = ?2, trigger = ?3, content = ?4, description = ?5,
                 category = ?6, favourite = ?7, enabled = ?8, updated_at = ?9 WHERE id = ?1",
            )?
            .execute(params![id, input.name, input.trigger, input.content, input.description, input.category, input.favourite as i64, input.enabled as i64, now_unix()])?;
        Ok(())
    }

    pub fn delete_snippet(&self, id: i64) -> Result<()> {
        self.main.prepare_cached("DELETE FROM snippets WHERE id = ?1")?.execute([id])?;
        Ok(())
    }

    pub fn snippet(&self, id: i64) -> Result<Option<Snippet>> {
        let sql = format!("SELECT {COLUMNS} FROM snippets s WHERE s.id = ?1");
        self.main.prepare_cached(&sql)?.query_row([id], from_row).optional()
    }

    pub fn recent_snippets(&self, limit: usize) -> Result<Vec<Snippet>> {
        let sql = format!("SELECT {COLUMNS} FROM snippets s ORDER BY s.favourite DESC, s.updated_at DESC, s.id DESC LIMIT ?1");
        let mut stmt = self.main.prepare_cached(&sql)?;
        stmt.query_map([limit as i64], from_row)?.collect()
    }

    /// An exact trigger match comes first, since typing `;sig` means that snippet.
    pub fn search_snippets(&self, query: &str, limit: usize) -> Result<Vec<Snippet>> {
        let mut out = Vec::new();
        let sql = format!("SELECT {COLUMNS} FROM snippets s WHERE s.trigger = ?1 AND s.trigger <> '' ORDER BY s.id LIMIT ?2");
        {
            let mut stmt = self.main.prepare_cached(&sql)?;
            for row in stmt.query_map(params![query.trim(), limit as i64], from_row)? {
                out.push(row?);
            }
        }
        let Some(fts) = fts_query(query) else {
            return Ok(out);
        };
        let sql = format!(
            "SELECT {COLUMNS} FROM snippets_fts f JOIN snippets s ON s.id = f.rowid
             WHERE snippets_fts MATCH ?1 ORDER BY bm25(snippets_fts, 10.0, 8.0, 1.0, 2.0, 2.0), s.id DESC LIMIT ?2"
        );
        let mut stmt = self.main.prepare_cached(&sql)?;
        for row in stmt.query_map(params![fts, limit as i64], from_row)? {
            let snippet = row?;
            if !out.iter().any(|existing| existing.id == snippet.id) {
                out.push(snippet);
            }
        }
        out.truncate(limit);
        Ok(out)
    }

    /// Triggers for the expansion engine. Only called when expansion is on.
    pub fn snippet_triggers(&self) -> Result<Vec<(String, i64)>> {
        let mut stmt = self.main.prepare_cached(
            "SELECT trigger, id FROM snippets WHERE trigger <> '' AND enabled = 1 ORDER BY id",
        )?;
        stmt.query_map([], |r| Ok((r.get(0)?, r.get(1)?)))?.collect()
    }

    /// Another snippet already using this trigger, shown when saving. It does not block the
    /// save: the matcher has a deterministic rule, and the UI tells the user who wins.
    pub fn trigger_conflict(&self, trigger: &str, excluding: i64) -> Result<Option<String>> {
        if trigger.is_empty() {
            return Ok(None);
        }
        self.main
            .prepare_cached("SELECT name FROM snippets WHERE trigger = ?1 AND id <> ?2 LIMIT 1")?
            .query_row(params![trigger, excluding], |r| r.get(0))
            .optional()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn input(name: &str, trigger: &str, content: &str) -> SnippetInput {
        SnippetInput {
            name: name.into(),
            trigger: trigger.into(),
            content: content.into(),
            enabled: true,
            ..Default::default()
        }
    }

    #[test]
    fn crud_and_search() {
        let s = Store::open_in_memory().unwrap();
        let id = s.create_snippet(&input("Email signature", ";sig", "Regards,\nSean")).unwrap();
        assert_eq!(s.search_snippets("signature", 5).unwrap()[0].id, id);
        assert_eq!(s.search_snippets("regards", 5).unwrap()[0].id, id);
        assert_eq!(s.search_snippets(";sig", 5).unwrap()[0].id, id);
        let mut updated = input("Signature", ";s", "Bye");
        updated.enabled = false;
        s.update_snippet(id, &updated).unwrap();
        assert!(s.search_snippets("regards", 5).unwrap().is_empty());
        let loaded = s.snippet(id).unwrap().unwrap();
        assert_eq!(loaded.content, "Bye");
        assert!(!loaded.enabled);
        assert!(s.snippet_triggers().unwrap().is_empty(), "disabled snippets don't expand");
        s.delete_snippet(id).unwrap();
        assert_eq!(s.snippet(id).unwrap(), None);
        assert!(s.search_snippets("signature", 5).unwrap().is_empty());
    }

    #[test]
    fn exact_trigger_comes_first() {
        let s = Store::open_in_memory().unwrap();
        s.create_snippet(&input("addr mentions ;addr in text", "", "about ;addr")).unwrap();
        let exact = s.create_snippet(&input("Home", ";addr", "1 Road")).unwrap();
        assert_eq!(s.search_snippets(";addr", 5).unwrap()[0].id, exact);
    }

    #[test]
    fn detects_trigger_conflicts() {
        let s = Store::open_in_memory().unwrap();
        let a = s.create_snippet(&input("One", ";x", "1")).unwrap();
        assert_eq!(s.trigger_conflict(";x", 0).unwrap().as_deref(), Some("One"));
        assert_eq!(s.trigger_conflict(";x", a).unwrap(), None);
        assert_eq!(s.trigger_conflict("", 0).unwrap(), None);
    }

    #[test]
    fn lists_only_enabled_triggers() {
        let s = Store::open_in_memory().unwrap();
        let a = s.create_snippet(&input("A", ";a", "1")).unwrap();
        s.create_snippet(&input("NoTrigger", "", "2")).unwrap();
        assert_eq!(s.snippet_triggers().unwrap(), vec![(";a".to_string(), a)]);
    }
}
