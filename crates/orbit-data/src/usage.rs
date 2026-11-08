//! Per-action usage counts for the ranking signal. Stores item keys only, never query text.

use crate::{Result, Store, now_unix};
use rusqlite::params;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct UsageRecord {
    pub key: String,
    pub count: u32,
    pub last_used_at: i64,
}

impl Store {
    pub fn record_usage(&self, key: &str) -> Result<()> {
        self.main
            .prepare_cached(
                "INSERT INTO command_usage(key, count, last_used_at) VALUES (?1, 1, ?2)
                 ON CONFLICT(key) DO UPDATE SET count = count + 1, last_used_at = excluded.last_used_at",
            )?
            .execute(params![key, now_unix()])?;
        Ok(())
    }

    pub fn usage_records(&self, limit: usize) -> Result<Vec<UsageRecord>> {
        let mut stmt = self.main.prepare_cached(
            "SELECT key, count, last_used_at FROM command_usage ORDER BY last_used_at DESC LIMIT ?1",
        )?;
        stmt.query_map([limit as i64], |r| {
            Ok(UsageRecord { key: r.get(0)?, count: r.get::<_, i64>(1)? as u32, last_used_at: r.get(2)? })
        })?
        .collect()
    }

    pub fn clear_usage(&self) -> Result<()> {
        self.main.execute("DELETE FROM command_usage", [])?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn counts_up_and_lists_recent() {
        let s = Store::open_in_memory().unwrap();
        s.record_usage("app:a").unwrap();
        s.record_usage("app:a").unwrap();
        s.record_usage("app:b").unwrap();
        let records = s.usage_records(10).unwrap();
        let a = records.iter().find(|r| r.key == "app:a").unwrap();
        assert_eq!(a.count, 2);
        s.clear_usage().unwrap();
        assert!(s.usage_records(10).unwrap().is_empty());
    }
}
