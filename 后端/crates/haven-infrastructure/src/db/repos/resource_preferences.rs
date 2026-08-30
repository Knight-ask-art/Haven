//! 资源级阅读/漫画偏好 SQLite Repository（ADR-RESOURCE-PREF-001）。
//!
//! 该模块只实现持久化和数据库层 CAS；effective 合并、媒体归属校验与 DTO
//! 由 Application 层负责。数据库中只保存经过 Domain `deny_unknown_fields`
//! 校验的 `PreferenceData` JSON。

use std::sync::Arc;

use async_trait::async_trait;
use haven_common::{AppError, UtcMillis};
use haven_domain::contracts::{
    EditionPreference, MediaItemPreference, ResourcePreferenceRepository,
};
use haven_domain::ids::{EditionId, MediaItemId};
use haven_domain::settings::PreferenceData;
use rusqlite::OptionalExtension;

use crate::db::Db;
use crate::db::repos::map_db_error;

pub struct SqliteResourcePreferenceRepository {
    db: Arc<Db>,
}

impl SqliteResourcePreferenceRepository {
    pub fn new(db: Arc<Db>) -> Self {
        Self { db }
    }

    fn decode_data(raw: String) -> Result<PreferenceData, AppError> {
        serde_json::from_str(&raw).map_err(|e| {
            AppError::new(
                "SETTINGS_DATA_CORRUPT",
                haven_common::ErrorKind::Internal,
                "资源内设数据损坏，已回退到上一级设置",
                false,
            )
            .with_source(e)
        })
    }
}

#[async_trait]
impl ResourcePreferenceRepository for SqliteResourcePreferenceRepository {
    async fn get_edition(
        &self,
        edition_id: EditionId,
    ) -> Result<Option<EditionPreference>, AppError> {
        let conn = self.db.lock();
        let row = conn
            .query_row(
                "SELECT edition_id, data_json, revision, updated_at
                 FROM edition_preferences WHERE edition_id = ?1",
                rusqlite::params![edition_id.to_string()],
                |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, String>(1)?,
                        row.get::<_, String>(2)?,
                        row.get::<_, i64>(3)?,
                    ))
                },
            )
            .optional()
            .map_err(map_db_error("查询版本内设失败"))?;
        row.map(|(id, data, revision, updated_at)| {
            let edition_id = id.parse().map_err(|e| {
                AppError::new(
                    "DATABASE_ERROR",
                    haven_common::ErrorKind::Database,
                    "版本内设 ID 损坏",
                    false,
                )
                .with_source(e)
            })?;
            Ok(EditionPreference {
                edition_id,
                data: Self::decode_data(data)?,
                revision,
                updated_at: UtcMillis(updated_at),
            })
        })
        .transpose()
    }

    async fn get_media_item(
        &self,
        media_item_id: MediaItemId,
    ) -> Result<Option<MediaItemPreference>, AppError> {
        let conn = self.db.lock();
        let row = conn
            .query_row(
                "SELECT media_item_id, edition_id, data_json, revision, updated_at
                 FROM media_item_preferences WHERE media_item_id = ?1",
                rusqlite::params![media_item_id.to_string()],
                |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, String>(1)?,
                        row.get::<_, String>(2)?,
                        row.get::<_, String>(3)?,
                        row.get::<_, i64>(4)?,
                    ))
                },
            )
            .optional()
            .map_err(map_db_error("查询媒体资源内设失败"))?;
        row.map(|(media_id, edition_id, data, revision, updated_at)| {
            let media_item_id = media_id.parse().map_err(|e| {
                AppError::new(
                    "DATABASE_ERROR",
                    haven_common::ErrorKind::Database,
                    "媒体资源内设 ID 损坏",
                    false,
                )
                .with_source(e)
            })?;
            let edition_id = edition_id.parse().map_err(|e| {
                AppError::new(
                    "DATABASE_ERROR",
                    haven_common::ErrorKind::Database,
                    "媒体资源内设版本 ID 损坏",
                    false,
                )
                .with_source(e)
            })?;
            Ok(MediaItemPreference {
                media_item_id,
                edition_id,
                data: Self::decode_data(data)?,
                revision,
                updated_at: UtcMillis(updated_at),
            })
        })
        .transpose()
    }

    async fn cas_upsert_edition(
        &self,
        preference: &EditionPreference,
        expected_revision: Option<&str>,
    ) -> Result<bool, AppError> {
        let data_json = serde_json::to_string(&preference.data).map_err(|e| {
            AppError::new(
                "SETTINGS_DATA_INVALID",
                haven_common::ErrorKind::Validation,
                "资源内设数据非法",
                false,
            )
            .with_source(e)
        })?;
        let mut guard = self.db.lock();
        let tx = guard
            .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)
            .map_err(map_db_error("开启版本内设事务失败"))?;
        let current: Option<String> = tx
            .query_row(
                "SELECT revision FROM edition_preferences WHERE edition_id = ?1",
                rusqlite::params![preference.edition_id.to_string()],
                |row| row.get(0),
            )
            .optional()
            .map_err(map_db_error("读取版本内设版本失败"))?;
        let matches = match (current.as_deref(), expected_revision) {
            (None, None) => true,
            (Some(current), Some(expected)) => current == expected,
            _ => false,
        };
        if !matches {
            tx.commit().map_err(map_db_error("提交版本内设事务失败"))?;
            return Ok(false);
        }
        tx.execute(
            "INSERT INTO edition_preferences (edition_id, data_json, revision, updated_at)
             VALUES (?1, ?2, ?3, ?4)
             ON CONFLICT(edition_id) DO UPDATE SET
               data_json = excluded.data_json,
               revision = excluded.revision,
               updated_at = excluded.updated_at",
            rusqlite::params![
                preference.edition_id.to_string(),
                data_json,
                preference.revision,
                preference.updated_at.0
            ],
        )
        .map_err(map_db_error("保存版本内设失败"))?;
        tx.commit().map_err(map_db_error("提交版本内设事务失败"))?;
        Ok(true)
    }

    async fn cas_upsert_media_item(
        &self,
        preference: &MediaItemPreference,
        expected_revision: Option<&str>,
    ) -> Result<bool, AppError> {
        let data_json = serde_json::to_string(&preference.data).map_err(|e| {
            AppError::new(
                "SETTINGS_DATA_INVALID",
                haven_common::ErrorKind::Validation,
                "资源内设数据非法",
                false,
            )
            .with_source(e)
        })?;
        let mut guard = self.db.lock();
        let tx = guard
            .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)
            .map_err(map_db_error("开启媒体资源内设事务失败"))?;
        let current: Option<String> = tx
            .query_row(
                "SELECT revision FROM media_item_preferences WHERE media_item_id = ?1",
                rusqlite::params![preference.media_item_id.to_string()],
                |row| row.get(0),
            )
            .optional()
            .map_err(map_db_error("读取媒体资源内设版本失败"))?;
        let matches = match (current.as_deref(), expected_revision) {
            (None, None) => true,
            (Some(current), Some(expected)) => current == expected,
            _ => false,
        };
        if !matches {
            tx.commit()
                .map_err(map_db_error("提交媒体资源内设事务失败"))?;
            return Ok(false);
        }
        tx.execute(
            "INSERT INTO media_item_preferences
               (media_item_id, edition_id, data_json, revision, updated_at)
             VALUES (?1, ?2, ?3, ?4, ?5)
             ON CONFLICT(media_item_id) DO UPDATE SET
               edition_id = excluded.edition_id,
               data_json = excluded.data_json,
               revision = excluded.revision,
               updated_at = excluded.updated_at",
            rusqlite::params![
                preference.media_item_id.to_string(),
                preference.edition_id.to_string(),
                data_json,
                preference.revision,
                preference.updated_at.0
            ],
        )
        .map_err(map_db_error("保存媒体资源内设失败"))?;
        tx.commit()
            .map_err(map_db_error("提交媒体资源内设事务失败"))?;
        Ok(true)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use haven_domain::settings::{ComicPatch, PreferenceData, ReadingPatch};

    #[tokio::test]
    async fn edition_and_media_item_cas_are_scoped_and_persistent() {
        let db = Arc::new(Db::open_in_memory().unwrap());
        let repo = SqliteResourcePreferenceRepository::new(db.clone());
        let edition_id = EditionId::new();
        let media_item_id = MediaItemId::new();
        let work_id = haven_domain::ids::WorkId::new();
        let now = UtcMillis::now().0;
        {
            let conn = db.lock();
            conn.execute(
                "INSERT INTO works (id, canonical_title, work_type, status, created_at, updated_at)
                 VALUES (?1, '偏好测试', 'fiction', 'completed', ?2, ?2)",
                rusqlite::params![work_id.to_string(), now],
            )
            .unwrap();
            conn.execute(
                "INSERT INTO editions (id, work_id, title, edition_type, created_at, updated_at)
                 VALUES (?1, ?2, '测试版本', 'book', ?3, ?3)",
                rusqlite::params![edition_id.to_string(), work_id.to_string(), now],
            )
            .unwrap();
            conn.execute(
                "INSERT INTO media_items (id, edition_id, media_type, title, status, created_at, updated_at)
                 VALUES (?1, ?2, 'book', '测试资源', 'available', ?3, ?3)",
                rusqlite::params![media_item_id.to_string(), edition_id.to_string(), now],
            )
            .unwrap();
        }
        let data = PreferenceData {
            reading: Some(ReadingPatch {
                font_size: Some(haven_domain::settings::ReadingFontSize::Large),
                ..ReadingPatch::default()
            }),
            comic: Some(ComicPatch::default()),
        };
        let edition = EditionPreference {
            edition_id,
            data: data.clone(),
            revision: "rev-edition-1".into(),
            updated_at: UtcMillis(1),
        };
        assert!(repo.cas_upsert_edition(&edition, None).await.unwrap());
        assert!(!repo.cas_upsert_edition(&edition, None).await.unwrap());
        assert!(
            repo.cas_upsert_edition(
                &EditionPreference {
                    revision: "rev-edition-2".into(),
                    ..edition.clone()
                },
                Some("rev-edition-1")
            )
            .await
            .unwrap()
        );
        assert_eq!(
            repo.get_edition(edition_id)
                .await
                .unwrap()
                .unwrap()
                .revision,
            "rev-edition-2"
        );

        let media = MediaItemPreference {
            media_item_id,
            edition_id,
            data,
            revision: "rev-media-1".into(),
            updated_at: UtcMillis(2),
        };
        assert!(repo.cas_upsert_media_item(&media, None).await.unwrap());
        assert!(
            !repo
                .cas_upsert_media_item(&media, Some("stale"))
                .await
                .unwrap()
        );
        assert_eq!(
            repo.get_media_item(media_item_id)
                .await
                .unwrap()
                .unwrap()
                .edition_id,
            edition_id
        );

        // Preference rows are owned by their media/edition and must disappear
        // with the corresponding business entity, without a second cleanup job.
        {
            let conn = db.lock();
            conn.execute(
                "DELETE FROM media_items WHERE id = ?1",
                rusqlite::params![media_item_id.to_string()],
            )
            .unwrap();
        }
        assert!(repo.get_media_item(media_item_id).await.unwrap().is_none());
        assert!(repo.get_edition(edition_id).await.unwrap().is_some());

        {
            let conn = db.lock();
            conn.execute(
                "DELETE FROM editions WHERE id = ?1",
                rusqlite::params![edition_id.to_string()],
            )
            .unwrap();
        }
        assert!(repo.get_edition(edition_id).await.unwrap().is_none());
    }
}
