//! 内容层级一致性校验（共享）。
//!
//! 不变量：`(work_id, edition_id, media_item_id)` 必须构成
//! work → edition → media_item 的合法层级链。
//!
//! - HistoryEntry（TASK-DB-005）写入前必须通过本校验。
//! - Progress / Marker 等同样携带三层 ID 的实体后续复用（追加任务）。
//! - DB 级兜底：`002_history_consistency.sql` 触发器（REPOSITORY 层是快速失败友好路径）。

use rusqlite::Connection;

use haven_common::AppError;
use haven_domain::ids::{EditionId, MediaItemId, WorkId};

/// 校验内容层级链是否合法。非法返回 `CONTENT_CHAIN_INVALID`（不写入）。
pub(crate) fn validate_content_chain(
    conn: &Connection,
    work_id: WorkId,
    edition_id: EditionId,
    media_item_id: MediaItemId,
) -> Result<(), AppError> {
    let edition_ok: bool = conn
        .query_row(
            "SELECT EXISTS(
                SELECT 1 FROM editions WHERE id = ?1 AND work_id = ?2
             )",
            rusqlite::params![edition_id.to_string(), work_id.to_string()],
            |row| row.get(0),
        )
        .map_err(db_err("校验内容层级失败"))?;

    let media_item_ok: bool = conn
        .query_row(
            "SELECT EXISTS(
                SELECT 1 FROM media_items WHERE id = ?1 AND edition_id = ?2
             )",
            rusqlite::params![media_item_id.to_string(), edition_id.to_string()],
            |row| row.get(0),
        )
        .map_err(db_err("校验内容层级失败"))?;

    if edition_ok && media_item_ok {
        Ok(())
    } else {
        Err(chain_invalid())
    }
}

pub(crate) fn chain_invalid() -> AppError {
    AppError::new(
        "CONTENT_CHAIN_INVALID",
        haven_common::ErrorKind::Validation,
        "work/edition/media_item 必须构成合法层级链",
        false,
    )
}

fn db_err(msg: &'static str) -> impl Fn(rusqlite::Error) -> AppError {
    move |e| {
        AppError::new(
            "DATABASE_ERROR",
            haven_common::ErrorKind::Database,
            msg,
            true,
        )
        .with_source(e)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::Db;

    fn seed_chain(db: &Db) -> (WorkId, EditionId, MediaItemId) {
        let work_id = WorkId::new();
        let edition_id = EditionId::new();
        let media_item_id = MediaItemId::new();
        let now = haven_common::UtcMillis::now().0;
        let conn = db.lock();
        conn.execute(
            "INSERT INTO works (id, canonical_title, work_type, status, created_at, updated_at)
             VALUES (?1, '层级测试作品', 'fiction', 'completed', ?2, ?2)",
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

    #[test]
    fn valid_chain_passes() {
        let db = Db::open_in_memory().unwrap();
        let (w, e, m) = seed_chain(&db);
        let conn = db.lock();
        assert!(validate_content_chain(&conn, w, e, m).is_ok());
    }

    #[test]
    fn mismatched_work_rejected() {
        let db = Db::open_in_memory().unwrap();
        let (w, e, m) = seed_chain(&db);
        let other_work = WorkId::new();
        let now = haven_common::UtcMillis::now().0;
        db.lock()
            .execute(
                "INSERT INTO works (id, canonical_title, work_type, status, created_at, updated_at)
                 VALUES (?1, '另一个作品', 'fiction', 'completed', ?2, ?2)",
                rusqlite::params![other_work.to_string(), now],
            )
            .unwrap();
        let conn = db.lock();
        let err = validate_content_chain(&conn, other_work, e, m).unwrap_err();
        assert_eq!(err.code().as_str(), "CONTENT_CHAIN_INVALID");
        // 合法链不受影响
        assert!(validate_content_chain(&conn, w, e, m).is_ok());
    }

    #[test]
    fn mismatched_edition_rejected() {
        let db = Db::open_in_memory().unwrap();
        let (w, _e, m) = seed_chain(&db);
        let other_edition = EditionId::new();
        let now = haven_common::UtcMillis::now().0;
        db.lock()
            .execute(
                "INSERT INTO editions (id, work_id, title, edition_type, created_at, updated_at)
                 VALUES (?1, ?2, '另一版本', 'movie', ?3, ?3)",
                rusqlite::params![other_edition.to_string(), w.to_string(), now],
            )
            .unwrap();
        let conn = db.lock();
        let err = validate_content_chain(&conn, w, other_edition, m).unwrap_err();
        assert_eq!(err.code().as_str(), "CONTENT_CHAIN_INVALID");
    }

    #[test]
    fn missing_media_item_rejected() {
        let db = Db::open_in_memory().unwrap();
        let (w, e, _m) = seed_chain(&db);
        let ghost = MediaItemId::new();
        let conn = db.lock();
        let err = validate_content_chain(&conn, w, e, ghost).unwrap_err();
        assert_eq!(err.code().as_str(), "CONTENT_CHAIN_INVALID");
    }
}
