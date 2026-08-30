//! HistoryEntry Repository（Sqlite）。
//!
//! 规范：DOMAIN_MODEL §27（历史 ≠ 进度；记录"何时打开过"）。
//! 不变量：`(work_id, edition_id, media_item_id)` 必须构成合法层级链——
//! Repository 写入前调用 `validate_content_chain` 快速失败（友好错误码），
//! DB 层由 `002_history_consistency` 触发器兜底（跨表 CHECK 无法用 SQLite CHECK 表达）。

use std::sync::Arc;

use async_trait::async_trait;

use haven_common::AppError;
use haven_domain::contracts::HistoryRepository;
use haven_domain::entities::HistoryEntry;
use haven_domain::ids::{EditionId, HistoryEntryId, MediaItemId, WorkId};

use crate::db::Db;
use crate::db::repos::hierarchy::validate_content_chain;
use crate::db::repos::{id_from_row, locator_from_json, locator_to_json, map_db_error};

pub struct SqliteHistoryRepository {
    db: Arc<Db>,
}

impl SqliteHistoryRepository {
    pub fn new(db: Arc<Db>) -> Self {
        Self { db }
    }
}

fn row_to_history_entry(row: &rusqlite::Row<'_>) -> rusqlite::Result<HistoryEntry> {
    let locator_json: Option<String> = row.get("locator_json")?;
    Ok(HistoryEntry {
        id: id_from_row::<HistoryEntryId>(row.get("id")?)?,
        media_item_id: id_from_row::<MediaItemId>(row.get("media_item_id")?)?,
        work_id: id_from_row::<WorkId>(row.get("work_id")?)?,
        edition_id: id_from_row::<EditionId>(row.get("edition_id")?)?,
        locator: locator_json
            .map(|json| locator_from_json(&json))
            .transpose()
            .map_err(locator_db_err)?,
        started_at: haven_common::UtcMillis(row.get("started_at")?),
        last_active_at: haven_common::UtcMillis(row.get("last_active_at")?),
        completed_at: row
            .get::<_, Option<i64>>("completed_at")?
            .map(haven_common::UtcMillis),
    })
}

fn locator_db_err(e: AppError) -> rusqlite::Error {
    rusqlite::Error::FromSqlConversionFailure(
        0,
        rusqlite::types::Type::Text,
        Box::new(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            e.user_message(),
        )),
    )
}

const SELECT_COLUMNS: &str = "id, media_item_id, work_id, edition_id, locator_json, started_at, last_active_at, completed_at";

#[async_trait]
impl HistoryRepository for SqliteHistoryRepository {
    async fn get(&self, id: HistoryEntryId) -> Result<Option<HistoryEntry>, AppError> {
        let conn = self.db.lock();
        let mut stmt = conn
            .prepare(&format!(
                "SELECT {SELECT_COLUMNS} FROM history_entries WHERE id = ?1"
            ))
            .map_err(map_db_error("查询历史失败"))?;
        let mut rows = stmt
            .query_map(rusqlite::params![id.to_string()], row_to_history_entry)
            .map_err(map_db_error("查询历史失败"))?;
        rows.next()
            .transpose()
            .map_err(map_db_error("查询历史失败"))
    }

    async fn save(&self, entry: &HistoryEntry) -> Result<(), AppError> {
        let conn = self.db.lock();
        // 内容层级校验：禁止跨作品错配元组（快速失败，友好错误码）。
        validate_content_chain(&conn, entry.work_id, entry.edition_id, entry.media_item_id)?;
        conn.execute(
            "INSERT INTO history_entries
                (id, media_item_id, work_id, edition_id, locator_json,
                 started_at, last_active_at, completed_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)
             ON CONFLICT(media_item_id) DO UPDATE SET
                 id = excluded.id,
                 work_id = excluded.work_id,
                 edition_id = excluded.edition_id,
                 locator_json = excluded.locator_json,
                 started_at = excluded.started_at,
                 last_active_at = excluded.last_active_at,
                 completed_at = excluded.completed_at",
            rusqlite::params![
                entry.id.to_string(),
                entry.media_item_id.to_string(),
                entry.work_id.to_string(),
                entry.edition_id.to_string(),
                entry.locator.as_ref().map(locator_to_json).transpose()?,
                entry.started_at.0,
                entry.last_active_at.0,
                entry.completed_at.map(|t| t.0),
            ],
        )
        .map_err(map_db_error("保存历史失败"))?;
        Ok(())
    }

    async fn list_for_media_item(
        &self,
        media_item_id: MediaItemId,
    ) -> Result<Vec<HistoryEntry>, AppError> {
        let conn = self.db.lock();
        let mut stmt = conn
            .prepare(&format!(
                "SELECT {SELECT_COLUMNS} FROM history_entries
                 WHERE media_item_id = ?1 ORDER BY last_active_at DESC"
            ))
            .map_err(map_db_error("查询历史列表失败"))?;
        let rows = stmt
            .query_map(
                rusqlite::params![media_item_id.to_string()],
                row_to_history_entry,
            )
            .map_err(map_db_error("查询历史列表失败"))?;
        rows.collect::<rusqlite::Result<Vec<_>>>()
            .map_err(map_db_error("查询历史列表失败"))
    }

    async fn recent(&self, limit: u32) -> Result<Vec<HistoryEntry>, AppError> {
        let conn = self.db.lock();
        let mut stmt = conn
            .prepare(&format!(
                "SELECT {SELECT_COLUMNS} FROM history_entries
                 ORDER BY last_active_at DESC LIMIT ?1"
            ))
            .map_err(map_db_error("查询最近历史失败"))?;
        let rows = stmt
            .query_map(rusqlite::params![limit], row_to_history_entry)
            .map_err(map_db_error("查询最近历史失败"))?;
        rows.collect::<rusqlite::Result<Vec<_>>>()
            .map_err(map_db_error("查询最近历史失败"))
    }

    async fn clear_all(&self) -> Result<(), AppError> {
        let conn = self.db.lock();
        conn.execute("DELETE FROM history_entries", [])
            .map_err(map_db_error("清空历史失败"))?;
        Ok(())
    }

    async fn delete(&self, id: HistoryEntryId) -> Result<bool, AppError> {
        let conn = self.db.lock();
        let affected = conn
            .execute(
                "DELETE FROM history_entries WHERE id = ?1",
                rusqlite::params![id.to_string()],
            )
            .map_err(map_db_error("删除历史失败"))?;
        Ok(affected > 0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::Db;
    use haven_domain::ids::EditionId;
    use haven_domain::locator::{Locator, VideoLocator};

    fn seed_chain(db: &Db) -> (WorkId, EditionId, MediaItemId) {
        let work_id = WorkId::new();
        let edition_id = EditionId::new();
        let media_item_id = MediaItemId::new();
        let now = haven_common::UtcMillis::now().0;
        let conn = db.lock();
        conn.execute(
            "INSERT INTO works (id, canonical_title, work_type, status, created_at, updated_at)
             VALUES (?1, '历史测试作品', 'fiction', 'completed', ?2, ?2)",
            rusqlite::params![work_id.to_string(), now],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO editions (id, work_id, title, edition_type, created_at, updated_at)
             VALUES (?1, ?2, '测试版本', 'series', ?3, ?3)",
            rusqlite::params![edition_id.to_string(), work_id.to_string(), now],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO media_items (id, edition_id, media_type, title, status, created_at, updated_at)
             VALUES (?1, ?2, 'episode', 'S01E01', 'available', ?3, ?3)",
            rusqlite::params![media_item_id.to_string(), edition_id.to_string(), now],
        )
        .unwrap();
        (work_id, edition_id, media_item_id)
    }

    fn sample_entry(
        work_id: WorkId,
        edition_id: EditionId,
        media_item_id: MediaItemId,
    ) -> HistoryEntry {
        HistoryEntry {
            id: HistoryEntryId::new(),
            media_item_id,
            work_id,
            edition_id,
            locator: Some(Locator::Video(VideoLocator {
                media_item_id,
                position_ms: 1_200_000,
            })),
            started_at: haven_common::UtcMillis(1_000),
            last_active_at: haven_common::UtcMillis(1_000),
            completed_at: None,
        }
    }

    #[tokio::test]
    async fn save_get_roundtrip_preserves_locator() {
        let db = Arc::new(Db::open_in_memory().unwrap());
        let (w, e, m) = seed_chain(&db);
        let repo = SqliteHistoryRepository::new(db);
        let entry = sample_entry(w, e, m);

        repo.save(&entry).await.unwrap();
        let read = repo.get(entry.id).await.unwrap().expect("存在");
        assert_eq!(read, entry);
        assert_eq!(read.locator, entry.locator);
    }

    #[tokio::test]
    async fn mismatched_chain_rejected_at_repository() {
        let db = Arc::new(Db::open_in_memory().unwrap());
        let (_w, e, m) = seed_chain(&db);
        let other_work = WorkId::new();
        let now = haven_common::UtcMillis::now().0;
        db.lock()
            .execute(
                "INSERT INTO works (id, canonical_title, work_type, status, created_at, updated_at)
                 VALUES (?1, '错配作品', 'fiction', 'completed', ?2, ?2)",
                rusqlite::params![other_work.to_string(), now],
            )
            .unwrap();
        let repo = SqliteHistoryRepository::new(db);

        let entry = sample_entry(other_work, e, m);
        let err = repo.save(&entry).await.unwrap_err();
        assert_eq!(err.code().as_str(), "CONTENT_CHAIN_INVALID");
    }

    #[tokio::test]
    async fn mismatched_chain_rejected_by_db_trigger() {
        let db = Arc::new(Db::open_in_memory().unwrap());
        let (_w, e, m) = seed_chain(&db);
        let other_work = WorkId::new();
        let now = haven_common::UtcMillis::now().0;
        let conn = db.lock();
        conn.execute(
            "INSERT INTO works (id, canonical_title, work_type, status, created_at, updated_at)
             VALUES (?1, '错配作品', 'fiction', 'completed', ?2, ?2)",
            rusqlite::params![other_work.to_string(), now],
        )
        .unwrap();
        // 绕过 Repository 直接 SQL 插入：触发器必须兜底拒绝。
        let result = conn.execute(
            "INSERT INTO history_entries
                (id, media_item_id, work_id, edition_id, locator_json, started_at, last_active_at)
             VALUES ('00000000-0000-0000-0000-000000000001', ?1, ?2, ?3, '{}', 1, 1)",
            rusqlite::params![m.to_string(), other_work.to_string(), e.to_string()],
        );
        assert!(result.is_err(), "触发器必须拒绝错配元组");
        let count: i64 = conn
            .query_row("SELECT COUNT(*) FROM history_entries", [], |r| r.get(0))
            .unwrap();
        assert_eq!(count, 0, "错配元组不得落库");
    }

    #[tokio::test]
    async fn recent_orders_by_last_active_desc() {
        let db = Arc::new(Db::open_in_memory().unwrap());
        let (w, e, m1) = seed_chain(&db);
        let (w2, e2, m2) = seed_chain(&db);
        let repo = SqliteHistoryRepository::new(db);

        let mut a = sample_entry(w, e, m1);
        a.last_active_at = haven_common::UtcMillis(1_000);
        let mut b = sample_entry(w2, e2, m2);
        b.last_active_at = haven_common::UtcMillis(2_000);
        repo.save(&a).await.unwrap();
        repo.save(&b).await.unwrap();

        let recent = repo.recent(10).await.unwrap();
        assert_eq!(recent.len(), 2);
        assert_eq!(recent[0].media_item_id, m2, "最近活跃在前");
    }

    #[tokio::test]
    async fn delete_removes_entry() {
        let db = Arc::new(Db::open_in_memory().unwrap());
        let (w, e, m) = seed_chain(&db);
        let repo = SqliteHistoryRepository::new(db);
        let entry = sample_entry(w, e, m);
        repo.save(&entry).await.unwrap();

        assert!(repo.delete(entry.id).await.unwrap());
        assert!(repo.get(entry.id).await.unwrap().is_none());
        assert!(!repo.delete(entry.id).await.unwrap(), "重复删除 false");
    }
}
