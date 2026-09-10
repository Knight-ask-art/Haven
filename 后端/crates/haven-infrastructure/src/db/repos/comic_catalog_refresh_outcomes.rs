//! 漫画目录刷新结果 Receipt 的 SQLite Repository。
//!
//! Receipt 是 append-only 的来源观察事实，不包含 URL、Cookie、grant、请求头或
//! 本地路径；每次刷新结果由 UUID v7 ID 独立标识。

use std::sync::Arc;

use rusqlite::OptionalExtension;

use haven_common::{AppError, ErrorKind};
use haven_domain::comic_catalog::ComicCatalogRefreshReceipt;
use haven_domain::ids::{ComicCatalogRefreshId, WorkId};

use crate::db::Db;
use crate::db::repos::{enum_to_db_str, map_db_error};

pub struct SqliteComicCatalogRefreshOutcomeRepository {
    db: Arc<Db>,
}

impl SqliteComicCatalogRefreshOutcomeRepository {
    pub fn new(db: Arc<Db>) -> Self {
        Self { db }
    }

    pub async fn save(&self, receipt: &ComicCatalogRefreshReceipt) -> Result<(), AppError> {
        let receipt = receipt.clone();
        self.db.with_tx(|tx| save_on_conn(tx, &receipt))
    }

    pub async fn get(
        &self,
        id: ComicCatalogRefreshId,
    ) -> Result<Option<ComicCatalogRefreshReceipt>, AppError> {
        let conn = self.db.lock();
        load_on_conn(&conn, id)
    }

    pub async fn list_by_work(
        &self,
        work_id: WorkId,
    ) -> Result<Vec<ComicCatalogRefreshReceipt>, AppError> {
        let conn = self.db.lock();
        let mut stmt = conn
            .prepare(
                "SELECT id FROM comic_catalog_refresh_outcomes
                 WHERE work_id = ?1 ORDER BY observed_at DESC, id DESC",
            )
            .map_err(map_db_error("查询漫画目录刷新结果列表失败"))?;
        let ids = stmt
            .query_map(rusqlite::params![work_id.to_string()], |row| {
                let id: String = row.get(0)?;
                id.parse().map_err(|_| invalid_row("id"))
            })
            .map_err(map_db_error("查询漫画目录刷新结果列表失败"))?
            .collect::<rusqlite::Result<Vec<ComicCatalogRefreshId>>>()
            .map_err(map_db_error("查询漫画目录刷新结果列表失败"))?;
        ids.into_iter()
            .map(|id| load_on_conn(&conn, id))
            .collect::<Result<Vec<_>, _>>()
            .map(|receipts| receipts.into_iter().flatten().collect())
    }
}

pub(crate) fn save_on_conn(
    conn: &rusqlite::Connection,
    receipt: &ComicCatalogRefreshReceipt,
) -> Result<(), AppError> {
    validate_text(&receipt.source_key, "source_key")?;
    validate_text(&receipt.remote_work_id, "remote_work_id")?;
    if let Some(value) = receipt.observed_from.as_deref() {
        validate_text(value, "observed_from")?;
    }
    if let Some(value) = receipt.observed_to.as_deref() {
        validate_text(value, "observed_to")?;
    }
    if let Some(value) = receipt.error_code.as_deref() {
        validate_text(value, "error_code")?;
    }
    let generation_before = i64::try_from(receipt.generation_before)
        .map_err(|_| invalid_receipt("generation_before 超出数据库范围"))?;
    let generation_after = receipt
        .generation_after
        .map(i64::try_from)
        .transpose()
        .map_err(|_| invalid_receipt("generation_after 超出数据库范围"))?;
    conn.execute(
        "INSERT INTO comic_catalog_refresh_outcomes
            (id, work_id, source_key, remote_work_id, status,
             generation_before, generation_after, observed_from, observed_to,
             truncated, retained_previous_catalog, error_code, observed_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13)",
        rusqlite::params![
            receipt.id.to_string(),
            receipt.work_id.to_string(),
            receipt.source_key,
            receipt.remote_work_id,
            enum_to_db_str(&receipt.status)?,
            generation_before,
            generation_after,
            receipt.observed_from,
            receipt.observed_to,
            i64::from(receipt.truncated),
            i64::from(receipt.retained_previous_catalog),
            receipt.error_code,
            receipt.observed_at.0,
        ],
    )
    .map_err(map_db_error("保存漫画目录刷新结果失败"))?;
    Ok(())
}

fn load_on_conn(
    conn: &rusqlite::Connection,
    id: ComicCatalogRefreshId,
) -> Result<Option<ComicCatalogRefreshReceipt>, AppError> {
    conn.query_row(
        "SELECT id, work_id, source_key, remote_work_id, status,
                generation_before, generation_after, observed_from, observed_to,
                truncated, retained_previous_catalog, error_code, observed_at
         FROM comic_catalog_refresh_outcomes WHERE id = ?1",
        rusqlite::params![id.to_string()],
        |row| {
            let source_key: String = row.get(2)?;
            validate_text(&source_key, "source_key").map_err(|_| invalid_row("source_key"))?;
            let remote_work_id: String = row.get(3)?;
            validate_text(&remote_work_id, "remote_work_id")
                .map_err(|_| invalid_row("remote_work_id"))?;
            let observed_from: Option<String> = row.get(7)?;
            if let Some(value) = observed_from.as_deref() {
                validate_text(value, "observed_from").map_err(|_| invalid_row("observed_from"))?;
            }
            let observed_to: Option<String> = row.get(8)?;
            if let Some(value) = observed_to.as_deref() {
                validate_text(value, "observed_to").map_err(|_| invalid_row("observed_to"))?;
            }
            let error_code: Option<String> = row.get(11)?;
            if let Some(value) = error_code.as_deref() {
                validate_text(value, "error_code").map_err(|_| invalid_row("error_code"))?;
            }
            Ok(ComicCatalogRefreshReceipt {
                id: parse_id(row.get(0)?, "id")?,
                work_id: parse_id(row.get(1)?, "work_id")?,
                source_key,
                remote_work_id,
                status: parse_db_enum(row.get(4)?, "status")?,
                generation_before: parse_generation(row.get(5)?, "generation_before")?,
                generation_after: row
                    .get::<_, Option<i64>>(6)?
                    .map(|value| parse_generation(value, "generation_after"))
                    .transpose()?,
                observed_from,
                observed_to,
                truncated: parse_bool(row.get(9)?, "truncated")?,
                retained_previous_catalog: parse_bool(row.get(10)?, "retained_previous_catalog")?,
                error_code,
                observed_at: haven_common::UtcMillis(row.get(12)?),
            })
        },
    )
    .optional()
    .map_err(map_db_error("查询漫画目录刷新结果失败"))
}

fn validate_text(value: &str, field: &'static str) -> Result<(), AppError> {
    if value.trim().is_empty()
        || value.len() > 4096
        || value
            .chars()
            .any(|ch| (ch as u32) <= 0x1f || ch == '\u{7f}')
        || value.contains("://")
        || value.to_ascii_lowercase().starts_with("data:")
    {
        return Err(invalid_receipt(format!("刷新结果字段 {field} 非法")));
    }
    Ok(())
}

fn parse_db_enum<T: serde::de::DeserializeOwned>(
    value: String,
    field: &'static str,
) -> rusqlite::Result<T> {
    serde_json::from_str(&format!("\"{value}\"")).map_err(|_| invalid_row(field))
}

fn parse_id<T: std::str::FromStr>(value: String, field: &'static str) -> rusqlite::Result<T> {
    value.parse().map_err(|_| invalid_row(field))
}

fn parse_generation(value: i64, field: &'static str) -> rusqlite::Result<u64> {
    u64::try_from(value).map_err(|_| invalid_row(field))
}

fn parse_bool(value: i64, field: &'static str) -> rusqlite::Result<bool> {
    match value {
        0 => Ok(false),
        1 => Ok(true),
        _ => Err(invalid_row(field)),
    }
}

fn invalid_row(field: &'static str) -> rusqlite::Error {
    rusqlite::Error::FromSqlConversionFailure(
        0,
        rusqlite::types::Type::Text,
        Box::new(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            format!("无法解析漫画目录刷新结果字段 {field}"),
        )),
    )
}

fn invalid_receipt(message: impl Into<String>) -> AppError {
    AppError::new(
        "INVALID_COMIC_CATALOG_REFRESH_RECEIPT",
        ErrorKind::Validation,
        message,
        false,
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use haven_common::UtcMillis;
    use haven_domain::comic_catalog::ComicCatalogRefreshOutcomeStatus;

    fn seed(db: &Db) -> WorkId {
        let id = WorkId::new();
        db.lock()
            .execute(
                "INSERT INTO works (id, canonical_title, work_type, status, created_at, updated_at)
                 VALUES (?1, '刷新作品', 'fiction', 'completed', 1, 1)",
                rusqlite::params![id.to_string()],
            )
            .unwrap();
        id
    }

    #[tokio::test]
    async fn comic_catalog_refresh_outcomes_roundtrip_and_remain_append_only() {
        let db = Arc::new(Db::open_in_memory().unwrap());
        let work_id = seed(&db);
        let repo = SqliteComicCatalogRefreshOutcomeRepository::new(db.clone());
        let receipt = ComicCatalogRefreshReceipt {
            id: ComicCatalogRefreshId::new(),
            work_id,
            source_key: "source".to_owned(),
            remote_work_id: "remote-work".to_owned(),
            status: ComicCatalogRefreshOutcomeStatus::Succeeded,
            generation_before: 2,
            generation_after: Some(3),
            observed_from: Some("chapter-1".to_owned()),
            observed_to: Some("chapter-3".to_owned()),
            truncated: false,
            retained_previous_catalog: false,
            error_code: None,
            observed_at: UtcMillis(10),
        };
        repo.save(&receipt).await.unwrap();
        assert_eq!(repo.get(receipt.id).await.unwrap().unwrap(), receipt);
        assert_eq!(repo.list_by_work(work_id).await.unwrap(), vec![receipt]);
        let count: i64 = db
            .lock()
            .query_row(
                "SELECT COUNT(*) FROM comic_catalog_refresh_outcomes",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(count, 1);
    }

    #[test]
    fn comic_catalog_refresh_outcomes_uow_rolls_back_subject_member_receipt_and_progress() {
        use crate::db::uow::SqliteUnitOfWork;
        use haven_application::services::ports::{
            ComicProgressSubjectWritePlan, ComicProgressWriteCandidate, UnitOfWork,
        };
        use haven_domain::comic_identity::ChapterEvidence;
        use haven_domain::comic_progress_subject::{
            ComicProgressSubject, ComicProgressSubjectMember,
        };
        use haven_domain::entities::Progress;
        use haven_domain::enums::CompletionState;
        use haven_domain::ids::{EditionId, MediaItemId, ProgressId};
        use haven_domain::locator::{ComicLocator, Locator};

        let db = Arc::new(Db::open_in_memory().unwrap());
        let work_id = seed(&db);
        let edition_id = EditionId::new();
        let media_item_id = MediaItemId::new();
        let now = haven_common::UtcMillis(1);
        {
            let conn = db.lock();
            conn.execute(
                "INSERT INTO editions (id, work_id, title, edition_type, created_at, updated_at)
                 VALUES (?1, ?2, '原子漫画版', 'comic', ?3, ?3)",
                rusqlite::params![edition_id.to_string(), work_id.to_string(), now.0],
            )
            .unwrap();
            conn.execute(
                "INSERT INTO media_items
                    (id, edition_id, media_type, title, category, chapter, page_count,
                     status, created_at, updated_at)
                 VALUES (?1, ?2, 'comic', '第 1 话', 'comic', 1, 1, 'available', ?3, ?3)",
                rusqlite::params![media_item_id.to_string(), edition_id.to_string(), now.0],
            )
            .unwrap();
            conn.execute_batch(
                "CREATE TRIGGER fail_subject_progress_write
                 BEFORE INSERT ON progress
                 BEGIN
                     SELECT RAISE(ABORT, 'injected subject progress failure');
                 END;",
            )
            .unwrap();
        }

        let mut subject = ComicProgressSubject::new(work_id, edition_id, media_item_id, now);
        subject
            .attach_member(ComicProgressSubjectMember::active(
                subject.id,
                media_item_id,
                vec![ChapterEvidence::SameRemoteIdentity],
            ))
            .unwrap();
        let members = subject.members().to_vec();
        let receipt = ComicCatalogRefreshReceipt {
            id: ComicCatalogRefreshId::new(),
            work_id,
            source_key: "source".to_owned(),
            remote_work_id: "remote-work".to_owned(),
            status: ComicCatalogRefreshOutcomeStatus::Succeeded,
            generation_before: 0,
            generation_after: Some(1),
            observed_from: Some("chapter-1".to_owned()),
            observed_to: Some("chapter-1".to_owned()),
            truncated: false,
            retained_previous_catalog: false,
            error_code: None,
            observed_at: now,
        };
        let progress = Progress {
            id: ProgressId::new(),
            work_id,
            edition_id,
            media_item_id,
            locator: Locator::Comic(ComicLocator {
                chapter_item_id: media_item_id,
                page_index: 0,
                page_progression: None,
            }),
            completion: CompletionState::InProgress,
            percentage: Some(0.25),
            last_active_at: now,
            updated_at: now,
            revision: None,
            keyframe_uri: None,
        };
        let error = SqliteUnitOfWork::new(db.clone())
            .run_comic_progress_subject_write(&ComicProgressSubjectWritePlan {
                subject,
                members,
                page_identity_write: None,
                progress_writes: vec![ComicProgressWriteCandidate {
                    progress,
                    expected_revision: None,
                }],
                migration_snapshot: None,
                refresh_receipt: Some(receipt),
            })
            .unwrap_err();
        assert_eq!(error.code().as_str(), "DATABASE_ERROR");

        for table in [
            "comic_progress_subjects",
            "comic_progress_subject_members",
            "comic_catalog_refresh_outcomes",
            "progress",
        ] {
            let count: i64 = db
                .lock()
                .query_row(&format!("SELECT COUNT(*) FROM {table}"), [], |row| {
                    row.get(0)
                })
                .unwrap();
            assert_eq!(count, 0, "Immediate 事务失败后 {table} 不得留下写入");
        }
    }

    #[test]
    fn comic_catalog_refresh_outcomes_uow_rejects_invalid_migration_snapshot_atomically() {
        use crate::db::uow::SqliteUnitOfWork;
        use haven_application::services::ports::{
            ComicProgressSubjectWritePlan, ComicProgressWriteCandidate, UnitOfWork,
        };
        use haven_domain::comic_identity::{
            ChapterEvidence, ComicProgressMigrationSnapshot, PageMappingConfidence,
            PageMappingStrategy, ProgressMigrationMode, ProgressMigrationState,
        };
        use haven_domain::comic_progress_subject::ComicProgressSubject;
        use haven_domain::entities::Progress;
        use haven_domain::enums::CompletionState;
        use haven_domain::ids::{EditionId, MediaItemId, ProgressId};
        use haven_domain::locator::{ComicLocator, Locator};

        let db = Arc::new(Db::open_in_memory().unwrap());
        let work_id = seed(&db);
        let edition_id = EditionId::new();
        let media_item_id = MediaItemId::new();
        let now = haven_common::UtcMillis(1);
        {
            let conn = db.lock();
            conn.execute(
                "INSERT INTO editions (id, work_id, title, edition_type, created_at, updated_at)
                 VALUES (?1, ?2, '原子漫画版', 'comic', ?3, ?3)",
                rusqlite::params![edition_id.to_string(), work_id.to_string(), now.0],
            )
            .unwrap();
            conn.execute(
                "INSERT INTO media_items
                    (id, edition_id, media_type, title, category, chapter, page_count,
                     status, created_at, updated_at)
                 VALUES (?1, ?2, 'comic', '第 1 话', 'comic', 1, 2, 'available', ?3, ?3)",
                rusqlite::params![media_item_id.to_string(), edition_id.to_string(), now.0],
            )
            .unwrap();
        }

        let subject = ComicProgressSubject::new(work_id, edition_id, media_item_id, now);
        let old_progress = Progress {
            id: ProgressId::new(),
            work_id,
            edition_id,
            media_item_id,
            locator: Locator::Comic(ComicLocator {
                chapter_item_id: media_item_id,
                page_index: 0,
                page_progression: None,
            }),
            completion: CompletionState::InProgress,
            percentage: Some(0.5),
            last_active_at: now,
            updated_at: now,
            revision: Some("source-revision".to_owned()),
            keyframe_uri: None,
        };
        let mut new_progress = old_progress.clone();
        new_progress.id = ProgressId::new();
        new_progress.locator = Locator::Comic(ComicLocator {
            chapter_item_id: media_item_id,
            page_index: 1,
            page_progression: None,
        });
        new_progress.percentage = Some(1.0);
        new_progress.revision = None;
        let snapshot = ComicProgressMigrationSnapshot {
            id: haven_domain::ids::ComicProgressMigrationId::new(),
            source_media_item_id: media_item_id,
            target_media_item_id: media_item_id,
            source_revision: "source-revision".to_owned(),
            target_revision_before: Some("source-revision".to_owned()),
            old_progress: old_progress.clone(),
            old_target_progress: Some(old_progress),
            new_progress: new_progress.clone(),
            mode: ProgressMigrationMode::OneTime,
            confidence: PageMappingConfidence::High,
            strategy: PageMappingStrategy::StableKey,
            evidence: vec![ChapterEvidence::ExactPageIdentity { matched: 1 }],
            created_at: now,
            applied_revision: Some("already-applied".to_owned()),
            state: ProgressMigrationState::Reverted,
            reverted_at: Some(haven_common::UtcMillis(2)),
        };

        let error = SqliteUnitOfWork::new(db.clone())
            .run_comic_progress_subject_write(&ComicProgressSubjectWritePlan {
                subject,
                members: vec![],
                page_identity_write: None,
                progress_writes: vec![ComicProgressWriteCandidate {
                    progress: new_progress,
                    expected_revision: Some("source-revision".to_owned()),
                }],
                migration_snapshot: Some(snapshot),
                refresh_receipt: None,
            })
            .unwrap_err();
        assert_eq!(error.kind(), haven_common::ErrorKind::Validation);

        for table in [
            "comic_progress_subjects",
            "comic_progress_subject_members",
            "comic_progress_migration_snapshots",
            "progress",
        ] {
            let count: i64 = db
                .lock()
                .query_row(&format!("SELECT COUNT(*) FROM {table}"), [], |row| {
                    row.get(0)
                })
                .unwrap();
            assert_eq!(count, 0, "无效迁移快照失败后 {table} 不得有残留");
        }
    }

    #[tokio::test]
    async fn comic_catalog_refresh_outcomes_reject_unsafe_source_identity() {
        let db = Arc::new(Db::open_in_memory().unwrap());
        let work_id = seed(&db);
        let repo = SqliteComicCatalogRefreshOutcomeRepository::new(db);
        let receipt = ComicCatalogRefreshReceipt {
            id: ComicCatalogRefreshId::new(),
            work_id,
            source_key: "https://provider.invalid".to_owned(),
            remote_work_id: "remote-work".to_owned(),
            status: ComicCatalogRefreshOutcomeStatus::Unknown,
            generation_before: 0,
            generation_after: None,
            observed_from: None,
            observed_to: None,
            truncated: false,
            retained_previous_catalog: true,
            error_code: None,
            observed_at: UtcMillis(1),
        };
        assert!(repo.save(&receipt).await.is_err());
    }

    #[tokio::test]
    async fn comic_catalog_refresh_outcomes_reject_unsafe_persisted_text_on_read() {
        let db = Arc::new(Db::open_in_memory().unwrap());
        let work_id = seed(&db);
        let receipt_id = ComicCatalogRefreshId::new();
        db.lock()
            .execute(
                "INSERT INTO comic_catalog_refresh_outcomes
                    (id, work_id, source_key, remote_work_id, status,
                     generation_before, generation_after, observed_from, observed_to,
                     truncated, retained_previous_catalog, error_code, observed_at)
                 VALUES (?1, ?2, 'https://invalid', 'remote-work', 'unknown',
                         0, NULL, NULL, NULL, 0, 0, NULL, 1)",
                rusqlite::params![receipt_id.to_string(), work_id.to_string()],
            )
            .unwrap();

        let repo = SqliteComicCatalogRefreshOutcomeRepository::new(db);
        assert!(repo.get(receipt_id).await.is_err());
    }
}
