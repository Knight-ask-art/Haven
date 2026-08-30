//! Marker Repository（Sqlite）。
//!
//! 规范：DOMAIN_MODEL §41–§44。
//! - 删除为软删除（tombstone：deleted_at），同步场景需要。
//! - 常规列表过滤掉已删除；save 会完整写入（含 deleted_at，保留墓碑语义）。

use std::sync::Arc;

use async_trait::async_trait;

use haven_common::AppError;
use haven_domain::contracts::MarkerRepository;
use haven_domain::entities::Marker;
use haven_domain::ids::{EditionId, MarkerId, MediaItemId, WorkId};

use crate::db::Db;
use crate::db::repos::hierarchy::validate_content_chain;
use crate::db::repos::{
    enum_to_db_str, id_from_row, locator_from_json, locator_to_json, map_db_error,
};

pub struct SqliteMarkerRepository {
    db: Arc<Db>,
}

impl SqliteMarkerRepository {
    pub fn new(db: Arc<Db>) -> Self {
        Self { db }
    }
}

fn row_to_marker(row: &rusqlite::Row<'_>) -> rusqlite::Result<Marker> {
    let marker_type: String = row.get("marker_type")?;
    let preview: Option<String> = row.get("preview")?;
    let deleted_at: Option<i64> = row.get("deleted_at")?;
    let locator_json: String = row.get("locator_json")?;

    Ok(Marker {
        id: id_from_row::<MarkerId>(row.get("id")?)?,
        work_id: id_from_row::<WorkId>(row.get("work_id")?)?,
        edition_id: id_from_row::<EditionId>(row.get("edition_id")?)?,
        media_item_id: id_from_row::<MediaItemId>(row.get("media_item_id")?)?,
        locator: locator_from_json(&locator_json).map_err(locator_db_err)?,
        marker_type: parse_enum(&marker_type)?,
        title: row.get("title")?,
        excerpt: row.get("excerpt")?,
        note: row.get("note")?,
        preview: preview
            .map(|json| serde_json::from_str(&json))
            .transpose()
            .map_err(|e| {
                rusqlite::Error::FromSqlConversionFailure(
                    0,
                    rusqlite::types::Type::Text,
                    Box::new(e),
                )
            })?,
        created_at: haven_common::UtcMillis(row.get("created_at")?),
        updated_at: haven_common::UtcMillis(row.get("updated_at")?),
        deleted_at: deleted_at.map(haven_common::UtcMillis),
    })
}

fn parse_enum<T: serde::de::DeserializeOwned>(s: &str) -> rusqlite::Result<T> {
    serde_json::from_str(&format!("\"{s}\"")).map_err(|_| {
        rusqlite::Error::FromSqlConversionFailure(
            0,
            rusqlite::types::Type::Text,
            Box::new(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "无法解析枚举",
            )),
        )
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

const SELECT_COLUMNS: &str = "id, work_id, edition_id, media_item_id, locator_json, marker_type, title, excerpt, note, preview, created_at, updated_at, deleted_at";

/// 足迹页 marker_list_all 的硬上限（防过大结果集；与 LibraryService::MAX_LIMIT 对齐）。
const MAX_MARKER_LIST_LIMIT: u32 = 200;

#[async_trait]
impl MarkerRepository for SqliteMarkerRepository {
    async fn list_for_media_item(
        &self,
        media_item_id: MediaItemId,
    ) -> Result<Vec<Marker>, AppError> {
        let conn = self.db.lock();
        let mut stmt = conn
            .prepare(&format!(
                "SELECT {SELECT_COLUMNS} FROM markers
                 WHERE media_item_id = ?1 AND deleted_at IS NULL
                 ORDER BY created_at"
            ))
            .map_err(map_db_error("查询标记失败"))?;
        let rows = stmt
            .query_map(rusqlite::params![media_item_id.to_string()], row_to_marker)
            .map_err(map_db_error("查询标记失败"))?;
        rows.collect::<rusqlite::Result<Vec<_>>>()
            .map_err(map_db_error("查询标记失败"))
    }

    async fn list_all(&self, limit: u32) -> Result<Vec<Marker>, AppError> {
        let conn = self.db.lock();
        let mut stmt = conn
            .prepare(&format!(
                "SELECT {SELECT_COLUMNS} FROM markers
                 WHERE deleted_at IS NULL
                 ORDER BY created_at DESC
                 LIMIT ?1"
            ))
            .map_err(map_db_error("查询标记失败"))?;
        let rows = stmt
            .query_map(
                rusqlite::params![i64::from(limit.min(MAX_MARKER_LIST_LIMIT))],
                row_to_marker,
            )
            .map_err(map_db_error("查询标记失败"))?;
        rows.collect::<rusqlite::Result<Vec<_>>>()
            .map_err(map_db_error("查询标记失败"))
    }

    async fn save(&self, marker: &Marker) -> Result<(), AppError> {
        let preview_json = match &marker.preview {
            Some(artwork) => Some(serde_json::to_string(artwork).map_err(|e| {
                AppError::new(
                    "SERIALIZE_FAILED",
                    haven_common::ErrorKind::Parse,
                    "序列化失败",
                    false,
                )
                .with_source(e)
            })?),
            None => None,
        };
        let conn = self.db.lock();
        validate_content_chain(
            &conn,
            marker.work_id,
            marker.edition_id,
            marker.media_item_id,
        )?;
        conn.execute(
            "INSERT INTO markers
                (id, work_id, edition_id, media_item_id, locator_json, marker_type,
                 title, excerpt, note, preview, created_at, updated_at, deleted_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13)
             ON CONFLICT(id) DO UPDATE SET
                 work_id = excluded.work_id,
                 edition_id = excluded.edition_id,
                 media_item_id = excluded.media_item_id,
                 locator_json = excluded.locator_json,
                 marker_type = excluded.marker_type,
                 title = excluded.title,
                 excerpt = excluded.excerpt,
                 note = excluded.note,
                 preview = excluded.preview,
                 updated_at = excluded.updated_at,
                 deleted_at = COALESCE(markers.deleted_at, excluded.deleted_at)",
            rusqlite::params![
                marker.id.to_string(),
                marker.work_id.to_string(),
                marker.edition_id.to_string(),
                marker.media_item_id.to_string(),
                locator_to_json(&marker.locator)?,
                enum_to_db_str(&marker.marker_type)?,
                marker.title,
                marker.excerpt,
                marker.note,
                preview_json,
                marker.created_at.0,
                marker.updated_at.0,
                marker.deleted_at.map(|t| t.0),
            ],
        )
        .map_err(map_db_error("保存标记失败"))?;
        Ok(())
    }

    async fn soft_delete(&self, id: MarkerId) -> Result<bool, AppError> {
        let conn = self.db.lock();
        let affected = conn
            .execute(
                "UPDATE markers SET deleted_at = ?2, updated_at = ?2
                 WHERE id = ?1 AND deleted_at IS NULL",
                rusqlite::params![id.to_string(), haven_common::UtcMillis::now().0],
            )
            .map_err(map_db_error("软删除标记失败"))?;
        Ok(affected > 0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::Db;
    use haven_domain::enums::MarkerType;
    use haven_domain::locator::{Locator, VideoLocator};

    fn seed_content(db: &Db) -> (WorkId, EditionId, MediaItemId) {
        let work_id = WorkId::new();
        let edition_id = EditionId::new();
        let media_item_id = MediaItemId::new();
        let now = haven_common::UtcMillis::now().0;
        let conn = db.lock();
        conn.execute(
            "INSERT INTO works (id, canonical_title, work_type, status, created_at, updated_at)
             VALUES (?1, '标记测试作品', 'fiction', 'completed', ?2, ?2)",
            rusqlite::params![work_id.to_string(), now],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO editions (id, work_id, title, edition_type, created_at, updated_at)
             VALUES (?1, ?2, '测试版本', 'movie', ?3, ?3)",
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

    fn sample_marker(work_id: WorkId, edition_id: EditionId, media_item_id: MediaItemId) -> Marker {
        Marker {
            id: MarkerId::new(),
            work_id,
            edition_id,
            media_item_id,
            locator: Locator::Video(VideoLocator {
                media_item_id,
                position_ms: 1_234_000,
            }),
            marker_type: MarkerType::Scene,
            title: Some("名场面".into()),
            excerpt: None,
            note: Some("这里的构图呼应开场".into()),
            preview: None,
            created_at: haven_common::UtcMillis::now(),
            updated_at: haven_common::UtcMillis::now(),
            deleted_at: None,
        }
    }

    #[tokio::test]
    async fn save_and_list_roundtrip() {
        let db = Arc::new(Db::open_in_memory().unwrap());
        let (w, e, m) = seed_content(&db);
        let repo = SqliteMarkerRepository::new(db);
        let marker = sample_marker(w, e, m);

        repo.save(&marker).await.unwrap();
        let listed = repo.list_for_media_item(m).await.unwrap();
        assert_eq!(listed.len(), 1);
        assert_eq!(listed[0].id, marker.id);
        assert_eq!(listed[0].note.as_deref(), Some("这里的构图呼应开场"));
        assert_eq!(listed[0].locator, marker.locator);
    }

    #[tokio::test]
    async fn soft_delete_hides_from_list_but_keeps_tombstone() {
        let db = Arc::new(Db::open_in_memory().unwrap());
        let (w, e, m) = seed_content(&db);
        let repo = SqliteMarkerRepository::new(db.clone());
        let marker = sample_marker(w, e, m);
        repo.save(&marker).await.unwrap();

        assert!(repo.soft_delete(marker.id).await.unwrap(), "首次删除 true");
        assert!(
            !repo.soft_delete(marker.id).await.unwrap(),
            "重复删除 false"
        );

        assert!(
            repo.list_for_media_item(m).await.unwrap().is_empty(),
            "列表应隐藏"
        );

        let deleted_at: Option<i64> = db
            .lock()
            .query_row(
                "SELECT deleted_at FROM markers WHERE id = ?1",
                rusqlite::params![marker.id.to_string()],
                |r| r.get(0),
            )
            .unwrap();
        assert!(deleted_at.is_some(), "墓碑必须保留（同步 tombstone）");
    }

    #[tokio::test]
    async fn save_updates_existing_marker() {
        let db = Arc::new(Db::open_in_memory().unwrap());
        let (w, e, m) = seed_content(&db);
        let repo = SqliteMarkerRepository::new(db);
        let mut marker = sample_marker(w, e, m);
        repo.save(&marker).await.unwrap();

        marker.note = Some("更新后的笔记".into());
        repo.save(&marker).await.unwrap();

        let listed = repo.list_for_media_item(m).await.unwrap();
        assert_eq!(listed.len(), 1, "同 id 应覆盖而非新增");
        assert_eq!(listed[0].note.as_deref(), Some("更新后的笔记"));
    }

    #[tokio::test]
    async fn ordinary_save_cannot_resurrect_tombstone() {
        let db = Arc::new(Db::open_in_memory().unwrap());
        let (w, e, m) = seed_content(&db);
        let repo = SqliteMarkerRepository::new(db.clone());
        let marker = sample_marker(w, e, m);
        repo.save(&marker).await.unwrap();
        assert!(repo.soft_delete(marker.id).await.unwrap());

        repo.save(&marker).await.unwrap();
        assert!(repo.list_for_media_item(m).await.unwrap().is_empty());
        let deleted_at: Option<i64> = db
            .lock()
            .query_row(
                "SELECT deleted_at FROM markers WHERE id = ?1",
                rusqlite::params![marker.id.to_string()],
                |row| row.get(0),
            )
            .unwrap();
        assert!(deleted_at.is_some());
    }

    #[tokio::test]
    async fn save_rejects_mismatched_content_chain() {
        let db = Arc::new(Db::open_in_memory().unwrap());
        let (w1, e1, _) = seed_content(&db);
        let (_, _, m2) = seed_content(&db);
        let repo = SqliteMarkerRepository::new(db);
        let marker = sample_marker(w1, e1, m2);

        let error = repo.save(&marker).await.unwrap_err();
        assert_eq!(error.code().as_str(), "CONTENT_CHAIN_INVALID");
    }
}
