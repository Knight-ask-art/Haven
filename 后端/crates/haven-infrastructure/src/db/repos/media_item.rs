//! MediaItem Repository（Sqlite）。
//!
//! 规范：DOMAIN_MODEL §7–§8；ADR-002 §分类映射。
//! 不变量（ADR-002）：`category` 列只能由 Repository 从 `media_type` 推导写入，
//! 前端/调用方不允许直接提交 category；在新增 DB CHECK 前不宣称 SQLite 已强制该不变量。

use std::sync::Arc;

use async_trait::async_trait;

use haven_common::AppError;
use haven_domain::contracts::MediaItemRepository;
use haven_domain::entities::{MediaIndex, MediaItem};
use haven_domain::enums::{ContentCategory, MediaType};
use haven_domain::ids::{EditionId, MediaItemId};

use crate::db::Db;
use crate::db::repos::{enum_to_db_str, id_from_row, map_db_error};

pub struct SqliteMediaItemRepository {
    db: Arc<Db>,
}

impl SqliteMediaItemRepository {
    pub fn new(db: Arc<Db>) -> Self {
        Self { db }
    }
}

fn row_to_media_item(row: &rusqlite::Row<'_>) -> rusqlite::Result<MediaItem> {
    let media_type: String = row.get("media_type")?;
    let media_type: MediaType = parse_enum(&media_type)?;
    let status: String = row.get("status")?;

    // MediaIndex 按实际落库列重建：
    // season/episode → Episode；volume/chapter → Chapter；custom_label → Custom；
    // ordinal → Article；全空 → Movie。
    let season: Option<i32> = row.get("season")?;
    let episode: Option<i32> = row.get("episode")?;
    let volume: Option<f64> = row.get("volume")?;
    let chapter: Option<f64> = row.get("chapter")?;
    let ordinal: Option<f64> = row.get("ordinal")?;
    let custom_label: Option<String> = row.get("custom_label")?;

    let index = match (episode, chapter, custom_label, ordinal) {
        (Some(episode), _, _, _) => MediaIndex::Episode {
            season: season.map(|value| value as u32),
            episode: episode as u32,
        },
        (_, Some(chapter), _, _) => MediaIndex::Chapter {
            volume: volume.map(|value| value as f32),
            chapter: chapter as f32,
        },
        (_, _, Some(label), ordinal) => MediaIndex::Custom { label, ordinal },
        (_, _, None, ordinal) if media_type == MediaType::Article => MediaIndex::Article {
            ordinal: ordinal.map(|value| value as u32),
        },
        _ => MediaIndex::Movie,
    };

    Ok(MediaItem {
        id: id_from_row::<MediaItemId>(row.get("id")?)?,
        edition_id: id_from_row::<EditionId>(row.get("edition_id")?)?,
        parent_id: row
            .get::<_, Option<String>>("parent_id")?
            .map(id_from_row::<MediaItemId>)
            .transpose()?,
        media_type,
        title: row.get("title")?,
        index,
        duration_ms: row.get::<_, Option<i64>>("duration_ms")?.map(|v| v as u64),
        page_count: row.get("page_count")?,
        chapter_count: row.get("chapter_count")?,
        published_at: row.get("published_at")?,
        status: parse_enum(&status)?,
        created_at: haven_common::UtcMillis(row.get("created_at")?),
        updated_at: haven_common::UtcMillis(row.get("updated_at")?),
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

/// MediaIndex 扁平列（season/episode/volume/chapter/ordinal/custom_label）。
type IndexColumns = (
    Option<i32>,
    Option<i32>,
    Option<f64>,
    Option<f64>,
    Option<f64>,
    Option<String>,
);

/// MediaIndex → 扁平列（season/episode/volume/chapter/ordinal/custom_label）。
fn index_to_columns(index: &MediaIndex) -> IndexColumns {
    match index {
        MediaIndex::Movie => (None, None, None, None, None, None),
        MediaIndex::Episode { season, episode } => (
            season.as_ref().map(|v| *v as i32),
            Some(*episode as i32),
            None,
            None,
            None,
            None,
        ),
        MediaIndex::Chapter { volume, chapter } => (
            None,
            None,
            volume.as_ref().map(|v| *v as f64),
            Some(*chapter as f64),
            None,
            None,
        ),
        MediaIndex::Article { ordinal } => (
            None,
            None,
            None,
            None,
            ordinal.as_ref().map(|v| *v as f64),
            None,
        ),
        MediaIndex::Custom { label, ordinal } => (
            None,
            None,
            None,
            None,
            ordinal.as_ref().map(|v| *v),
            Some(label.clone()),
        ),
    }
}

const SELECT_COLUMNS: &str = "id, edition_id, parent_id, media_type, title, season, episode, volume, chapter, ordinal, custom_label, duration_ms, page_count, chapter_count, published_at, status, created_at, updated_at";

/// 在指定连接上保存 MediaItem（普通连接或事务连接复用；scanner 原子写入用）。
/// ADR-002：category 只从 media_type 推导，不接受调用方传入。
pub(crate) fn save_on_conn(conn: &rusqlite::Connection, item: &MediaItem) -> Result<(), AppError> {
    let category = ContentCategory::from_media_type(item.media_type);
    let (season, episode, volume, chapter, ordinal, custom_label) = index_to_columns(&item.index);
    conn.execute(
        "INSERT INTO media_items
            (id, edition_id, parent_id, media_type, title, category,
             season, episode, volume, chapter, ordinal, custom_label,
             duration_ms, page_count, chapter_count, published_at, status,
             created_at, updated_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12,
                 ?13, ?14, ?15, ?16, ?17, ?18, ?19)
         ON CONFLICT(id) DO UPDATE SET
             edition_id = excluded.edition_id,
             parent_id = excluded.parent_id,
             media_type = excluded.media_type,
             title = excluded.title,
             category = excluded.category,
             season = excluded.season,
             episode = excluded.episode,
             volume = excluded.volume,
             chapter = excluded.chapter,
             ordinal = excluded.ordinal,
             custom_label = excluded.custom_label,
             duration_ms = excluded.duration_ms,
             page_count = excluded.page_count,
             chapter_count = excluded.chapter_count,
             published_at = excluded.published_at,
             status = excluded.status,
             updated_at = excluded.updated_at",
        rusqlite::params![
            item.id.to_string(),
            item.edition_id.to_string(),
            item.parent_id.map(|id| id.to_string()),
            enum_to_db_str(&item.media_type)?,
            item.title,
            enum_to_db_str(&category)?,
            season,
            episode,
            volume,
            chapter,
            ordinal,
            custom_label,
            item.duration_ms.map(|v| v as i64),
            item.page_count,
            item.chapter_count,
            item.published_at,
            enum_to_db_str(&item.status)?,
            item.created_at.0,
            item.updated_at.0,
        ],
    )
    .map_err(map_db_error("保存媒体条目失败"))?;
    Ok(())
}

#[async_trait]
impl MediaItemRepository for SqliteMediaItemRepository {
    async fn get(&self, id: MediaItemId) -> Result<Option<MediaItem>, AppError> {
        let conn = self.db.lock();
        let mut stmt = conn
            .prepare(&format!(
                "SELECT {SELECT_COLUMNS} FROM media_items WHERE id = ?1"
            ))
            .map_err(map_db_error("查询媒体条目失败"))?;
        let mut rows = stmt
            .query_map(rusqlite::params![id.to_string()], row_to_media_item)
            .map_err(map_db_error("查询媒体条目失败"))?;
        rows.next()
            .transpose()
            .map_err(map_db_error("查询媒体条目失败"))
    }

    async fn save(&self, item: &MediaItem) -> Result<(), AppError> {
        let conn = self.db.lock();
        save_on_conn(&conn, item)
    }

    async fn list_by_edition(&self, edition_id: EditionId) -> Result<Vec<MediaItem>, AppError> {
        let conn = self.db.lock();
        let mut stmt = conn
            .prepare(&format!(
                "SELECT {SELECT_COLUMNS} FROM media_items WHERE edition_id = ?1 ORDER BY created_at"
            ))
            .map_err(map_db_error("查询媒体条目列表失败"))?;
        let rows = stmt
            .query_map(rusqlite::params![edition_id.to_string()], row_to_media_item)
            .map_err(map_db_error("查询媒体条目列表失败"))?;
        rows.collect::<rusqlite::Result<Vec<_>>>()
            .map_err(map_db_error("查询媒体条目列表失败"))
    }

    async fn list_by_editions(
        &self,
        edition_ids: &[EditionId],
    ) -> Result<Vec<MediaItem>, AppError> {
        if edition_ids.is_empty() {
            return Ok(Vec::new());
        }
        let conn = self.db.lock();
        let placeholders: Vec<String> = (1..=edition_ids.len()).map(|i| format!("?{i}")).collect();
        let sql = format!(
            "SELECT {SELECT_COLUMNS} FROM media_items WHERE edition_id IN ({}) ORDER BY created_at",
            placeholders.join(",")
        );
        let mut stmt = conn
            .prepare(&sql)
            .map_err(map_db_error("批量查询媒体条目失败"))?;
        let params: Vec<String> = edition_ids.iter().map(|id| id.to_string()).collect();
        let params: Vec<&dyn rusqlite::ToSql> =
            params.iter().map(|s| s as &dyn rusqlite::ToSql).collect();
        let rows = stmt
            .query_map(params.as_slice(), row_to_media_item)
            .map_err(map_db_error("批量查询媒体条目失败"))?;
        rows.collect::<rusqlite::Result<Vec<_>>>()
            .map_err(map_db_error("批量查询媒体条目失败"))
    }

    async fn delete(&self, id: MediaItemId) -> Result<bool, AppError> {
        let conn = self.db.lock();
        let affected = conn
            .execute(
                "DELETE FROM media_items WHERE id = ?1",
                rusqlite::params![id.to_string()],
            )
            .map_err(map_db_error(
                "删除媒体条目失败（可能被资源/用户状态引用，RESTRICT）",
            ))?;
        Ok(affected > 0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::Db;
    use haven_domain::enums::{MediaItemStatus, MediaType};

    fn seed_edition(db: &Db) -> EditionId {
        let work_id = haven_domain::ids::WorkId::new();
        let edition_id = EditionId::new();
        let now = haven_common::UtcMillis::now().0;
        let conn = db.lock();
        conn.execute(
            "INSERT INTO works (id, canonical_title, work_type, status, created_at, updated_at)
             VALUES (?1, '测试作品', 'fiction', 'completed', ?2, ?2)",
            rusqlite::params![work_id.to_string(), now],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO editions (id, work_id, title, edition_type, created_at, updated_at)
             VALUES (?1, ?2, '测试版本', 'series', ?3, ?3)",
            rusqlite::params![edition_id.to_string(), work_id.to_string(), now],
        )
        .unwrap();
        edition_id
    }

    fn sample_item(edition_id: EditionId, index: MediaIndex, media_type: MediaType) -> MediaItem {
        MediaItem {
            id: MediaItemId::new(),
            edition_id,
            parent_id: None,
            media_type,
            title: "S01E01".into(),
            index,
            duration_ms: Some(2_700_000),
            page_count: None,
            chapter_count: None,
            published_at: None,
            status: MediaItemStatus::Available,
            created_at: haven_common::UtcMillis(1_000),
            updated_at: haven_common::UtcMillis(1_000),
        }
    }

    #[tokio::test]
    async fn episode_roundtrip_and_category_derived() {
        let db = Arc::new(Db::open_in_memory().unwrap());
        let edition_id = seed_edition(&db);
        let repo = SqliteMediaItemRepository::new(db.clone());
        let item = sample_item(
            edition_id,
            MediaIndex::Episode {
                season: Some(1),
                episode: 3,
            },
            MediaType::Episode,
        );

        repo.save(&item).await.unwrap();
        let read = repo.get(item.id).await.unwrap().expect("存在");
        assert_eq!(read, item);
        assert_eq!(
            read.index,
            MediaIndex::Episode {
                season: Some(1),
                episode: 3
            }
        );

        let category: String = db
            .lock()
            .query_row(
                "SELECT category FROM media_items WHERE id = ?1",
                rusqlite::params![item.id.to_string()],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(category, "video", "episode 必须推导为 video（ADR-002）");
    }

    #[tokio::test]
    async fn chapter_index_supports_fractional_values() {
        let db = Arc::new(Db::open_in_memory().unwrap());
        let edition_id = seed_edition(&db);
        let repo = SqliteMediaItemRepository::new(db);
        let item = sample_item(
            edition_id,
            MediaIndex::Chapter {
                volume: Some(2.0),
                chapter: 12.5,
            },
            MediaType::Comic,
        );

        repo.save(&item).await.unwrap();
        let read = repo.get(item.id).await.unwrap().unwrap();
        assert_eq!(
            read.index,
            MediaIndex::Chapter {
                volume: Some(2.0),
                chapter: 12.5
            }
        );
    }

    #[tokio::test]
    async fn episode_without_season_roundtrips() {
        let db = Arc::new(Db::open_in_memory().unwrap());
        let edition_id = seed_edition(&db);
        let repo = SqliteMediaItemRepository::new(db);
        let item = sample_item(
            edition_id,
            MediaIndex::Episode {
                season: None,
                episode: 7,
            },
            MediaType::Episode,
        );

        repo.save(&item).await.unwrap();
        let read = repo.get(item.id).await.unwrap().unwrap();
        assert_eq!(read.index, item.index);
    }

    #[tokio::test]
    async fn chapter_without_volume_roundtrips() {
        let db = Arc::new(Db::open_in_memory().unwrap());
        let edition_id = seed_edition(&db);
        let repo = SqliteMediaItemRepository::new(db);
        let item = sample_item(
            edition_id,
            MediaIndex::Chapter {
                volume: None,
                chapter: 3.5,
            },
            MediaType::Comic,
        );

        repo.save(&item).await.unwrap();
        let read = repo.get(item.id).await.unwrap().unwrap();
        assert_eq!(read.index, item.index);
    }

    #[tokio::test]
    async fn article_without_ordinal_roundtrips() {
        let db = Arc::new(Db::open_in_memory().unwrap());
        let edition_id = seed_edition(&db);
        let repo = SqliteMediaItemRepository::new(db);
        let item = sample_item(
            edition_id,
            MediaIndex::Article { ordinal: None },
            MediaType::Article,
        );

        repo.save(&item).await.unwrap();
        let read = repo.get(item.id).await.unwrap().unwrap();
        assert_eq!(read.index, item.index);
    }

    #[tokio::test]
    async fn list_by_edition_orders_by_creation() {
        let db = Arc::new(Db::open_in_memory().unwrap());
        let edition_id = seed_edition(&db);
        let repo = SqliteMediaItemRepository::new(db);

        let mut a = sample_item(
            edition_id,
            MediaIndex::Episode {
                season: Some(1),
                episode: 1,
            },
            MediaType::Episode,
        );
        a.created_at = haven_common::UtcMillis(1_000);
        a.title = "S01E01".into();
        let mut b = sample_item(
            edition_id,
            MediaIndex::Episode {
                season: Some(1),
                episode: 2,
            },
            MediaType::Episode,
        );
        b.created_at = haven_common::UtcMillis(2_000);
        b.title = "S01E02".into();
        repo.save(&a).await.unwrap();
        repo.save(&b).await.unwrap();

        let listed = repo.list_by_edition(edition_id).await.unwrap();
        assert_eq!(listed.len(), 2);
        assert_eq!(listed[0].title, "S01E01");
    }
}
