//! Search History Repository（V02-SETTINGS-PRIVACY-DATA-007）。
//!
//! 该表只保存可删除的搜索词偏好，不与 `history_entries`（播放/阅读历史）
//! 或任何媒体业务事实共用清理路径。

use std::sync::Arc;

use async_trait::async_trait;

use haven_common::{AppError, UtcMillis};
use haven_domain::contracts::{SearchHistoryEntry, SearchHistoryRepository};

use crate::db::Db;
use crate::db::repos::map_db_error;

pub struct SqliteSearchHistoryRepository {
    db: Arc<Db>,
}

impl SqliteSearchHistoryRepository {
    pub fn new(db: Arc<Db>) -> Self {
        Self { db }
    }
}

#[async_trait]
impl SearchHistoryRepository for SqliteSearchHistoryRepository {
    async fn list(&self, limit: u32) -> Result<Vec<SearchHistoryEntry>, AppError> {
        let conn = self.db.lock();
        let mut statement = conn
            .prepare(
                "SELECT term, last_used_at
                 FROM search_history
                 ORDER BY last_used_at DESC, term ASC
                 LIMIT ?1",
            )
            .map_err(map_db_error("查询搜索历史失败"))?;
        let rows = statement
            .query_map(rusqlite::params![limit], |row| {
                Ok(SearchHistoryEntry {
                    term: row.get("term")?,
                    last_used_at: UtcMillis(row.get("last_used_at")?),
                })
            })
            .map_err(map_db_error("查询搜索历史失败"))?;
        rows.collect::<rusqlite::Result<Vec<_>>>()
            .map_err(map_db_error("查询搜索历史失败"))
    }

    async fn record(&self, term: &str, at: UtcMillis) -> Result<(), AppError> {
        let conn = self.db.lock();
        conn.execute(
            "INSERT INTO search_history (term, last_used_at)
             VALUES (?1, ?2)
             ON CONFLICT(term) DO UPDATE SET last_used_at = excluded.last_used_at",
            rusqlite::params![term, at.0],
        )
        .map_err(map_db_error("保存搜索历史失败"))?;
        Ok(())
    }

    async fn delete(&self, term: &str) -> Result<bool, AppError> {
        let conn = self.db.lock();
        let affected = conn
            .execute(
                "DELETE FROM search_history WHERE term = ?1",
                rusqlite::params![term],
            )
            .map_err(map_db_error("删除搜索历史失败"))?;
        Ok(affected > 0)
    }

    async fn clear_all(&self) -> Result<u64, AppError> {
        let conn = self.db.lock();
        let affected = conn
            .execute("DELETE FROM search_history", [])
            .map_err(map_db_error("清空搜索历史失败"))?;
        Ok(affected as u64)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn record_lists_deduplicated_terms_in_recent_order() {
        let db = Arc::new(Db::open_in_memory().unwrap());
        let repo = SqliteSearchHistoryRepository::new(db);
        repo.record("旧词", UtcMillis(1)).await.unwrap();
        repo.record("新词", UtcMillis(2)).await.unwrap();
        repo.record("旧词", UtcMillis(3)).await.unwrap();

        let entries = repo.list(10).await.unwrap();
        assert_eq!(
            entries
                .iter()
                .map(|entry| entry.term.as_str())
                .collect::<Vec<_>>(),
            ["旧词", "新词"]
        );
        assert_eq!(entries[0].last_used_at, UtcMillis(3));
    }

    #[tokio::test]
    async fn delete_and_clear_are_idempotent_and_scoped() {
        let db = Arc::new(Db::open_in_memory().unwrap());
        let repo = SqliteSearchHistoryRepository::new(db);
        repo.record("词", UtcMillis(1)).await.unwrap();
        assert!(repo.delete("词").await.unwrap());
        assert!(!repo.delete("词").await.unwrap());
        assert_eq!(repo.clear_all().await.unwrap(), 0);
    }
}
