//! 漫画进度迁移的 SQLite 原子写入与撤销。
//!
//! 应用层负责比较章节/页面身份并生成快照，本模块负责把 revision 校验、
//! Progress 写入和迁移快照放进同一事务。这样页面变化允许最佳努力恢复，
//! 但不会出现“位置已经覆盖、撤销证据没有保存”的半成功状态。

use std::sync::Arc;

use async_trait::async_trait;
use rusqlite::OptionalExtension;

use haven_common::{AppError, ErrorKind, UtcMillis};
use haven_domain::comic_identity::{
    ComicProgressMigrationSnapshot, ProgressMigrationMode, ProgressMigrationState,
};
use haven_domain::contracts::ComicProgressMigrationRepository;
use haven_domain::entities::Progress;
use haven_domain::ids::{ComicProgressMigrationId, MediaItemId};
use haven_domain::locator::{ComicLocator, Locator};

use crate::db::Db;
use crate::db::repos::progress::{
    SELECT_COLUMNS, new_revision, row_to_progress, validate_progress,
};
use crate::db::repos::{enum_to_db_str, locator_to_json, map_db_error};

pub struct SqliteComicProgressMigrationRepository {
    db: Arc<Db>,
}

impl SqliteComicProgressMigrationRepository {
    pub fn new(db: Arc<Db>) -> Self {
        Self { db }
    }
}

fn conversion_error(field: &'static str) -> rusqlite::Error {
    rusqlite::Error::FromSqlConversionFailure(
        0,
        rusqlite::types::Type::Text,
        Box::new(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            format!("无法解析漫画进度迁移字段 {field}"),
        )),
    )
}

fn serialize_json<T: serde::Serialize>(value: &T, field: &'static str) -> Result<String, AppError> {
    serde_json::to_string(value).map_err(|error| {
        AppError::new(
            "SERIALIZE_FAILED",
            ErrorKind::Parse,
            format!("序列化漫画进度迁移字段 {field} 失败"),
            false,
        )
        .with_source(error)
    })
}

fn parse_json<T: serde::de::DeserializeOwned>(
    value: String,
    field: &'static str,
) -> rusqlite::Result<T> {
    serde_json::from_str(&value).map_err(|_| conversion_error(field))
}

fn parse_id<T: std::str::FromStr>(value: String, field: &'static str) -> rusqlite::Result<T> {
    value.parse().map_err(|_| conversion_error(field))
}

fn parse_db_enum<T: serde::de::DeserializeOwned>(
    value: String,
    field: &'static str,
) -> rusqlite::Result<T> {
    serde_json::from_str(&format!("\"{value}\"")).map_err(|_| conversion_error(field))
}

fn snapshot_state(value: &ProgressMigrationState) -> Result<String, AppError> {
    enum_to_db_str(value)
}

fn current_progress(
    conn: &rusqlite::Connection,
    media_item_id: MediaItemId,
) -> Result<Option<Progress>, AppError> {
    let mut stmt = conn
        .prepare(&format!(
            "SELECT {SELECT_COLUMNS} FROM progress WHERE media_item_id = ?1"
        ))
        .map_err(map_db_error("查询漫画迁移进度失败"))?;
    let mut rows = stmt
        .query_map(
            rusqlite::params![media_item_id.to_string()],
            row_to_progress,
        )
        .map_err(map_db_error("查询漫画迁移进度失败"))?;
    rows.next()
        .transpose()
        .map_err(map_db_error("查询漫画迁移进度失败"))
}

fn ensure_comic_progress(conn: &rusqlite::Connection, progress: &Progress) -> Result<(), AppError> {
    let is_comic: i64 = conn
        .query_row(
            "SELECT EXISTS(
                 SELECT 1 FROM media_items WHERE id = ?1 AND media_type = 'comic'
             )",
            rusqlite::params![progress.media_item_id.to_string()],
            |row| row.get(0),
        )
        .map_err(map_db_error("检查漫画进度归属失败"))?;
    let locator_matches = matches!(
        &progress.locator,
        Locator::Comic(ComicLocator { chapter_item_id, .. })
            if *chapter_item_id == progress.media_item_id
    );
    if is_comic == 0 || !locator_matches {
        return Err(AppError::new(
            "COMIC_PROGRESS_REQUIRED",
            ErrorKind::Validation,
            "漫画进度迁移只能处理与媒体条目一致的 Comic Locator",
            false,
        ));
    }
    Ok(())
}

fn write_progress_if_revision(
    conn: &rusqlite::Connection,
    progress: &Progress,
    expected_revision: Option<&str>,
) -> Result<Option<String>, AppError> {
    validate_progress(conn, progress)?;
    ensure_comic_progress(conn, progress)?;
    let locator_json = locator_to_json(&progress.locator)?;
    let completion = enum_to_db_str(&progress.completion)?;
    let percentage = progress.percentage.map(f64::from);
    match expected_revision {
        Some(expected) => {
            let revision = new_revision();
            Ok(conn
                .query_row(
                    "UPDATE progress SET
                     work_id = ?1,
                     edition_id = ?2,
                     locator_json = ?4,
                     locator_version = ?5,
                     completion = ?6,
                     percentage = ?7,
                     keyframe_uri = ?8,
                     revision = ?9,
                     last_active_at = CASE
                         WHEN ?10 <= last_active_at THEN last_active_at
                         ELSE ?10
                     END,
                     updated_at = CASE
                         WHEN ?10 <= updated_at THEN updated_at + 1
                         ELSE ?10
                     END
                 WHERE media_item_id = ?3 AND revision = ?11
                 RETURNING revision",
                    rusqlite::params![
                        progress.work_id.to_string(),
                        progress.edition_id.to_string(),
                        progress.media_item_id.to_string(),
                        locator_json,
                        progress.locator.version(),
                        completion,
                        percentage,
                        progress.keyframe_uri,
                        revision,
                        progress.updated_at.0,
                        expected,
                    ],
                    |row| row.get::<_, String>(0),
                )
                .optional()
                .map_err(map_db_error("条件写入漫画迁移进度失败"))?)
        }
        None => {
            let revision = new_revision();
            Ok(conn
                .query_row(
                    "INSERT INTO progress
                    (id, work_id, edition_id, media_item_id, locator_json, locator_version,
                     completion, percentage, last_active_at, updated_at, revision, keyframe_uri)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12)
                 ON CONFLICT(media_item_id) DO NOTHING
                 RETURNING revision",
                    rusqlite::params![
                        progress.id.to_string(),
                        progress.work_id.to_string(),
                        progress.edition_id.to_string(),
                        progress.media_item_id.to_string(),
                        locator_json,
                        progress.locator.version(),
                        completion,
                        percentage,
                        progress.last_active_at.0,
                        progress.updated_at.0,
                        revision,
                        progress.keyframe_uri,
                    ],
                    |row| row.get::<_, String>(0),
                )
                .optional()
                .map_err(map_db_error("创建漫画迁移进度失败"))?)
        }
    }
}

fn row_to_snapshot(row: &rusqlite::Row<'_>) -> rusqlite::Result<ComicProgressMigrationSnapshot> {
    let id: String = row.get("id")?;
    let old_progress_json: String = row.get("old_progress_json")?;
    let old_target_progress_json: Option<String> = row.get("old_target_progress_json")?;
    let new_progress_json: String = row.get("new_progress_json")?;
    let evidence_json: String = row.get("evidence_json")?;
    let state: String = row.get("state")?;
    Ok(ComicProgressMigrationSnapshot {
        id: parse_id(id, "id")?,
        source_media_item_id: parse_id(row.get("source_media_item_id")?, "source_media_item_id")?,
        target_media_item_id: parse_id(row.get("target_media_item_id")?, "target_media_item_id")?,
        source_revision: row.get("source_revision")?,
        target_revision_before: row.get("target_revision_before")?,
        old_progress: parse_json(old_progress_json, "old_progress_json")?,
        old_target_progress: old_target_progress_json
            .map(|json| parse_json(json, "old_target_progress_json"))
            .transpose()?,
        new_progress: parse_json(new_progress_json, "new_progress_json")?,
        mode: parse_db_enum(row.get("mode")?, "mode")?,
        confidence: parse_db_enum(row.get("confidence")?, "confidence")?,
        strategy: parse_db_enum(row.get("strategy")?, "strategy")?,
        evidence: parse_json(evidence_json, "evidence_json")?,
        created_at: UtcMillis(row.get("created_at")?),
        applied_revision: row.get("applied_revision")?,
        state: parse_db_enum(state, "state")?,
        reverted_at: row.get::<_, Option<i64>>("reverted_at")?.map(UtcMillis),
    })
}

const SNAPSHOT_COLUMNS: &str = "id, source_media_item_id, target_media_item_id, source_revision,
    target_revision_before, old_progress_json, old_target_progress_json, new_progress_json,
    mode, confidence, strategy, evidence_json, created_at, applied_revision, state, reverted_at";

fn load_snapshot(
    conn: &rusqlite::Connection,
    id: ComicProgressMigrationId,
) -> Result<Option<ComicProgressMigrationSnapshot>, AppError> {
    conn.query_row(
        &format!(
            "SELECT {SNAPSHOT_COLUMNS}
             FROM comic_progress_migration_snapshots WHERE id = ?1"
        ),
        rusqlite::params![id.to_string()],
        row_to_snapshot,
    )
    .optional()
    .map_err(map_db_error("查询漫画进度迁移快照失败"))
}

fn write_snapshot(
    conn: &rusqlite::Connection,
    snapshot: &ComicProgressMigrationSnapshot,
) -> Result<(), AppError> {
    let mode = enum_to_db_str(&snapshot.mode)?;
    let confidence = enum_to_db_str(&snapshot.confidence)?;
    let strategy = enum_to_db_str(&snapshot.strategy)?;
    let state = snapshot_state(&snapshot.state)?;
    let old_progress_json = serialize_json(&snapshot.old_progress, "old_progress_json")?;
    let old_target_progress_json = snapshot
        .old_target_progress
        .as_ref()
        .map(|progress| serialize_json(progress, "old_target_progress_json"))
        .transpose()?;
    let new_progress_json = serialize_json(&snapshot.new_progress, "new_progress_json")?;
    let evidence_json = serialize_json(&snapshot.evidence, "evidence_json")?;
    conn.execute(
        "INSERT INTO comic_progress_migration_snapshots
            (id, source_media_item_id, target_media_item_id, source_revision,
             target_revision_before, old_progress_json, old_target_progress_json,
             new_progress_json, mode, confidence, strategy, evidence_json,
             created_at, applied_revision, state, reverted_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16)",
        rusqlite::params![
            snapshot.id.to_string(),
            snapshot.source_media_item_id.to_string(),
            snapshot.target_media_item_id.to_string(),
            snapshot.source_revision,
            snapshot.target_revision_before,
            old_progress_json,
            old_target_progress_json,
            new_progress_json,
            mode,
            confidence,
            strategy,
            evidence_json,
            snapshot.created_at.0,
            snapshot.applied_revision,
            state,
            snapshot.reverted_at.map(|value| value.0),
        ],
    )
    .map_err(map_db_error("保存漫画进度迁移快照失败"))?;
    Ok(())
}

#[async_trait]
impl ComicProgressMigrationRepository for SqliteComicProgressMigrationRepository {
    async fn apply(
        &self,
        snapshot: &ComicProgressMigrationSnapshot,
        expected_source_revision: &str,
        expected_target_revision: Option<&str>,
    ) -> Result<Option<String>, AppError> {
        if snapshot.state != ProgressMigrationState::Applied
            || snapshot.mode == ProgressMigrationMode::None
        {
            return Err(AppError::new(
                "INVALID_COMIC_MIGRATION",
                ErrorKind::Validation,
                "只能应用有效的漫画进度迁移快照",
                false,
            ));
        }
        if snapshot.source_media_item_id != snapshot.old_progress.media_item_id
            || snapshot.target_media_item_id != snapshot.new_progress.media_item_id
            || snapshot.source_revision != expected_source_revision
            || snapshot.target_revision_before.as_deref() != expected_target_revision
            || snapshot.old_progress.revision.as_deref() != Some(expected_source_revision)
            || (snapshot.source_media_item_id != snapshot.target_media_item_id
                && snapshot
                    .old_target_progress
                    .as_ref()
                    .and_then(|progress| progress.revision.as_deref())
                    != expected_target_revision)
        {
            return Err(AppError::new(
                "INVALID_COMIC_MIGRATION",
                ErrorKind::Validation,
                "漫画进度迁移快照与 revision 或媒体条目不一致",
                false,
            ));
        }

        let mut guard = self.db.lock();
        let tx = guard
            .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)
            .map_err(map_db_error("开启漫画进度迁移事务失败"))?;
        let source = current_progress(&tx, snapshot.source_media_item_id)?;
        let Some(source) = source else {
            return Ok(None);
        };
        if source.revision.as_deref() != Some(expected_source_revision) {
            return Ok(None);
        }

        let target_before = current_progress(&tx, snapshot.target_media_item_id)?;
        if snapshot.source_media_item_id != snapshot.target_media_item_id {
            match (target_before.as_ref(), expected_target_revision) {
                (None, None) => {}
                (Some(target), Some(expected)) if target.revision.as_deref() == Some(expected) => {}
                _ => return Ok(None),
            }
        }

        let expected = if snapshot.source_media_item_id == snapshot.target_media_item_id {
            Some(expected_source_revision)
        } else {
            expected_target_revision
        };
        let Some(applied_revision) =
            write_progress_if_revision(&tx, &snapshot.new_progress, expected)?
        else {
            return Ok(None);
        };
        let authoritative = current_progress(&tx, snapshot.target_media_item_id)?
            .ok_or_else(|| internal_error("迁移进度写入后无法读取权威状态"))?;
        let mut stored = snapshot.clone();
        stored.old_progress = source;
        stored.old_target_progress =
            if snapshot.source_media_item_id == snapshot.target_media_item_id {
                target_before
            } else {
                target_before
            };
        stored.new_progress = authoritative;
        stored.applied_revision = Some(applied_revision.clone());
        stored.state = ProgressMigrationState::Applied;
        write_snapshot(&tx, &stored)?;
        tx.commit()
            .map_err(map_db_error("提交漫画进度迁移事务失败"))?;
        Ok(Some(applied_revision))
    }

    async fn get_snapshot(
        &self,
        id: ComicProgressMigrationId,
    ) -> Result<Option<ComicProgressMigrationSnapshot>, AppError> {
        let conn = self.db.lock();
        load_snapshot(&conn, id)
    }

    async fn revert(
        &self,
        id: ComicProgressMigrationId,
        expected_applied_revision: &str,
    ) -> Result<bool, AppError> {
        let mut guard = self.db.lock();
        let tx = guard
            .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)
            .map_err(map_db_error("开启撤销漫画进度迁移事务失败"))?;
        let Some(snapshot) = load_snapshot(&tx, id)? else {
            return Err(AppError::new(
                "COMIC_MIGRATION_NOT_FOUND",
                ErrorKind::NotFound,
                "漫画进度迁移快照不存在",
                false,
            ));
        };
        if snapshot.state == ProgressMigrationState::Reverted {
            return Err(AppError::new(
                "COMIC_MIGRATION_ALREADY_REVERTED",
                ErrorKind::Conflict,
                "漫画进度迁移已经撤销",
                false,
            ));
        }
        if snapshot.applied_revision.as_deref() != Some(expected_applied_revision) {
            return Ok(false);
        }

        if snapshot.source_media_item_id == snapshot.target_media_item_id {
            let current = current_progress(&tx, snapshot.target_media_item_id)?;
            if current
                .as_ref()
                .and_then(|progress| progress.revision.as_deref())
                != Some(expected_applied_revision)
            {
                return Ok(false);
            }
            if write_progress_if_revision(
                &tx,
                &snapshot.old_progress,
                Some(expected_applied_revision),
            )?
            .is_none()
            {
                return Ok(false);
            }
        } else {
            let current = current_progress(&tx, snapshot.target_media_item_id)?;
            if current
                .as_ref()
                .and_then(|progress| progress.revision.as_deref())
                != Some(expected_applied_revision)
            {
                return Ok(false);
            }
            if let Some(old_target) = snapshot.old_target_progress.as_ref() {
                if write_progress_if_revision(&tx, old_target, Some(expected_applied_revision))?
                    .is_none()
                {
                    return Ok(false);
                }
            } else {
                let deleted = tx
                    .execute(
                        "DELETE FROM progress
                         WHERE media_item_id = ?1 AND revision = ?2",
                        rusqlite::params![
                            snapshot.target_media_item_id.to_string(),
                            expected_applied_revision,
                        ],
                    )
                    .map_err(map_db_error("删除迁移产生的目标进度失败"))?;
                if deleted != 1 {
                    return Ok(false);
                }
            }
        }

        let now = UtcMillis::now().0;
        let changed = tx
            .execute(
                "UPDATE comic_progress_migration_snapshots
                 SET state = 'reverted', reverted_at = ?1
                 WHERE id = ?2 AND state = 'applied'",
                rusqlite::params![now, id.to_string()],
            )
            .map_err(map_db_error("标记漫画进度迁移撤销失败"))?;
        if changed != 1 {
            return Ok(false);
        }
        tx.commit()
            .map_err(map_db_error("提交撤销漫画进度迁移事务失败"))?;
        Ok(true)
    }
}

fn internal_error(message: &'static str) -> AppError {
    AppError::new("DATABASE_ERROR", ErrorKind::Database, message, true)
}

#[cfg(test)]
mod tests {
    use super::*;
    use haven_domain::comic_identity::{
        ChapterEvidence, PageMappingConfidence, PageMappingStrategy,
    };
    use haven_domain::contracts::ProgressRepository;
    use haven_domain::entities::{MediaIndex, MediaItem};
    use haven_domain::enums::{CompletionState, MediaItemStatus, MediaType};
    use haven_domain::ids::{EditionId, ProgressId, WorkId};
    use haven_domain::locator::ComicLocator;

    fn seed_comic(db: &Db, second: bool) -> (WorkId, EditionId, MediaItemId) {
        let work_id = WorkId::new();
        let edition_id = EditionId::new();
        let media_item_id = MediaItemId::new();
        let now = 1_000;
        let conn = db.lock();
        conn.execute(
            "INSERT INTO works (id, canonical_title, work_type, status, created_at, updated_at)
             VALUES (?1, '迁移作品', 'fiction', 'completed', ?2, ?2)",
            rusqlite::params![work_id.to_string(), now],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO editions (id, work_id, title, edition_type, created_at, updated_at)
             VALUES (?1, ?2, '漫画版', 'comic', ?3, ?3)",
            rusqlite::params![edition_id.to_string(), work_id.to_string(), now],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO media_items
                (id, edition_id, media_type, title, category, chapter, page_count,
                 status, created_at, updated_at)
             VALUES (?1, ?2, 'comic', ?3, 'comic', ?4, 4, 'available', ?5, ?5)",
            rusqlite::params![
                media_item_id.to_string(),
                edition_id.to_string(),
                if second { "第 2 话" } else { "第 1 话" },
                if second { 2.0 } else { 1.0 },
                now,
            ],
        )
        .unwrap();
        (work_id, edition_id, media_item_id)
    }

    fn progress(
        work_id: WorkId,
        edition_id: EditionId,
        media_item_id: MediaItemId,
        page_index: u32,
        updated_at: i64,
    ) -> Progress {
        Progress {
            id: ProgressId::new(),
            work_id,
            edition_id,
            media_item_id,
            locator: Locator::Comic(ComicLocator {
                chapter_item_id: media_item_id,
                page_index,
                page_progression: Some(0.25),
            }),
            completion: CompletionState::InProgress,
            percentage: Some(0.25),
            last_active_at: UtcMillis(updated_at),
            updated_at: UtcMillis(updated_at),
            revision: None,
            keyframe_uri: None,
        }
    }

    fn snapshot(
        old: Progress,
        new: Progress,
        target_old: Option<Progress>,
    ) -> ComicProgressMigrationSnapshot {
        let source_revision = old
            .revision
            .clone()
            .expect("snapshot 的源进度必须来自持久层");
        let target_revision_before = if old.media_item_id == new.media_item_id {
            Some(source_revision.clone())
        } else {
            target_old.as_ref().map(|progress| {
                progress
                    .revision
                    .clone()
                    .expect("snapshot 的目标进度必须来自持久层")
            })
        };
        ComicProgressMigrationSnapshot {
            id: ComicProgressMigrationId::new(),
            source_media_item_id: old.media_item_id,
            target_media_item_id: new.media_item_id,
            source_revision,
            target_revision_before,
            old_progress: old,
            old_target_progress: target_old,
            new_progress: new,
            mode: ProgressMigrationMode::OneTime,
            confidence: PageMappingConfidence::Medium,
            strategy: PageMappingStrategy::NearestSurvivingPage,
            evidence: vec![ChapterEvidence::PartialPageIdentity { matched: 1 }],
            created_at: UtcMillis(3_000),
            applied_revision: None,
            state: ProgressMigrationState::Applied,
            reverted_at: None,
        }
    }

    #[tokio::test]
    async fn same_media_migration_is_atomic_and_reversible() {
        let db = Arc::new(Db::open_in_memory().unwrap());
        let (work, edition, media) = seed_comic(&db, false);
        let repo = SqliteComicProgressMigrationRepository::new(db.clone());
        let old = progress(work, edition, media, 3, 1_000);
        let progress_repo = crate::db::repos::SqliteProgressRepository::new(db.clone());
        progress_repo.save_if_revision(&old, None).await.unwrap();
        let old = progress_repo
            .get_for_media_item(media)
            .await
            .unwrap()
            .unwrap();
        let new = progress(work, edition, media, 2, 2_000);
        let migration = snapshot(old.clone(), new, None);
        let old_revision = old.revision.clone().unwrap();
        let applied = repo
            .apply(&migration, &old_revision, Some(&old_revision))
            .await
            .unwrap()
            .unwrap();
        assert_eq!(
            repo.get_snapshot(migration.id)
                .await
                .unwrap()
                .unwrap()
                .applied_revision
                .as_deref(),
            Some(applied.as_str())
        );
        let stored = progress_repo
            .get_for_media_item(media)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(stored.revision.as_deref(), Some(applied.as_str()));
        assert!(matches!(
            stored.locator,
            Locator::Comic(ComicLocator { page_index: 2, .. })
        ));
        assert!(repo.revert(migration.id, &applied).await.unwrap());
        let restored = progress_repo
            .get_for_media_item(media)
            .await
            .unwrap()
            .unwrap();
        assert!(matches!(
            restored.locator,
            Locator::Comic(ComicLocator { page_index: 3, .. })
        ));
        assert_eq!(
            repo.get_snapshot(migration.id)
                .await
                .unwrap()
                .unwrap()
                .state,
            ProgressMigrationState::Reverted
        );
    }

    #[tokio::test]
    async fn cross_media_migration_requires_absent_target_and_revert_deletes_created_row() {
        let db = Arc::new(Db::open_in_memory().unwrap());
        let (work, edition, source_media) = seed_comic(&db, false);
        let (_, _, target_media) = seed_comic(&db, true);
        let repo = SqliteComicProgressMigrationRepository::new(db.clone());
        let progress_repo = crate::db::repos::SqliteProgressRepository::new(db.clone());
        let old = progress(work, edition, source_media, 1, 1_000);
        progress_repo.save_if_revision(&old, None).await.unwrap();
        let old = progress_repo
            .get_for_media_item(source_media)
            .await
            .unwrap()
            .unwrap();
        let (target_work, target_edition) = {
            let conn = db.lock();
            let values: (String, String) = conn
                .query_row(
                    "SELECT e.work_id, e.id FROM editions e
                     JOIN media_items m ON m.edition_id = e.id WHERE m.id = ?1",
                    rusqlite::params![target_media.to_string()],
                    |row| Ok((row.get(0)?, row.get(1)?)),
                )
                .unwrap();
            (values.0.parse().unwrap(), values.1.parse().unwrap())
        };
        let new = progress(target_work, target_edition, target_media, 0, 2_000);
        let migration = snapshot(old.clone(), new, None);
        let old_revision = old.revision.clone().unwrap();
        let applied = repo
            .apply(&migration, &old_revision, None)
            .await
            .unwrap()
            .unwrap();
        assert!(
            progress_repo
                .get_for_media_item(target_media)
                .await
                .unwrap()
                .is_some()
        );
        assert!(repo.revert(migration.id, &applied).await.unwrap());
        assert!(
            progress_repo
                .get_for_media_item(target_media)
                .await
                .unwrap()
                .is_none()
        );
        assert!(
            progress_repo
                .get_for_media_item(source_media)
                .await
                .unwrap()
                .is_some()
        );
    }

    #[tokio::test]
    async fn stale_source_revision_never_overwrites_newer_progress() {
        let db = Arc::new(Db::open_in_memory().unwrap());
        let (work, edition, media) = seed_comic(&db, false);
        let repo = SqliteComicProgressMigrationRepository::new(db.clone());
        let progress_repo = crate::db::repos::SqliteProgressRepository::new(db.clone());
        let old = progress(work, edition, media, 1, 1_000);
        progress_repo.save_if_revision(&old, None).await.unwrap();
        let old = progress_repo
            .get_for_media_item(media)
            .await
            .unwrap()
            .unwrap();
        let newer = progress(work, edition, media, 2, 2_000);
        let old_revision = old.revision.clone().unwrap();
        progress_repo
            .save_if_revision(&newer, Some(&old_revision))
            .await
            .unwrap();
        let migration = snapshot(old, progress(work, edition, media, 0, 3_000), None);
        assert_eq!(
            repo.apply(&migration, &old_revision, Some(&old_revision))
                .await
                .unwrap(),
            None
        );
        let stored = progress_repo
            .get_for_media_item(media)
            .await
            .unwrap()
            .unwrap();
        assert!(matches!(
            stored.locator,
            Locator::Comic(ComicLocator { page_index: 2, .. })
        ));
    }

    #[tokio::test]
    async fn snapshot_write_failure_rolls_back_progress_and_snapshot() {
        let db = Arc::new(Db::open_in_memory().unwrap());
        let (work, edition, media) = seed_comic(&db, false);
        let repo = SqliteComicProgressMigrationRepository::new(db.clone());
        let progress_repo = crate::db::repos::SqliteProgressRepository::new(db.clone());
        let old = progress(work, edition, media, 3, 1_000);
        progress_repo.save_if_revision(&old, None).await.unwrap();
        let old = progress_repo
            .get_for_media_item(media)
            .await
            .unwrap()
            .unwrap();
        let migration = snapshot(old, progress(work, edition, media, 2, 2_000), None);
        let old_revision = migration.source_revision.clone();

        db.lock()
            .execute_batch(
                "CREATE TRIGGER fail_comic_migration_snapshot_insert
                 BEFORE INSERT ON comic_progress_migration_snapshots
                 BEGIN
                     SELECT RAISE(ABORT, 'injected comic migration snapshot failure');
                 END;",
            )
            .unwrap();
        let error = repo
            .apply(&migration, &old_revision, Some(&old_revision))
            .await
            .unwrap_err();
        assert_eq!(error.code().as_str(), "DATABASE_ERROR");
        db.lock()
            .execute_batch("DROP TRIGGER fail_comic_migration_snapshot_insert")
            .unwrap();

        let stored = progress_repo
            .get_for_media_item(media)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(stored.updated_at.0, 1_000);
        assert!(matches!(
            stored.locator,
            Locator::Comic(ComicLocator { page_index: 3, .. })
        ));
        assert!(repo.get_snapshot(migration.id).await.unwrap().is_none());
    }

    #[tokio::test]
    async fn revert_revision_conflict_preserves_new_user_progress_and_snapshot() {
        let db = Arc::new(Db::open_in_memory().unwrap());
        let (work, edition, media) = seed_comic(&db, false);
        let repo = SqliteComicProgressMigrationRepository::new(db.clone());
        let progress_repo = crate::db::repos::SqliteProgressRepository::new(db.clone());
        let old = progress(work, edition, media, 3, 1_000);
        progress_repo.save_if_revision(&old, None).await.unwrap();
        let old = progress_repo
            .get_for_media_item(media)
            .await
            .unwrap()
            .unwrap();
        let migration = snapshot(old.clone(), progress(work, edition, media, 2, 2_000), None);
        let old_revision = old.revision.clone().unwrap();
        let applied = repo
            .apply(&migration, &old_revision, Some(&old_revision))
            .await
            .unwrap()
            .unwrap();

        let user_update = progress(work, edition, media, 0, 4_000);
        let user_revision = progress_repo
            .save_if_revision(&user_update, Some(&applied))
            .await
            .unwrap()
            .unwrap();
        assert_ne!(user_revision, applied);

        assert!(!repo.revert(migration.id, &applied).await.unwrap());
        let stored = progress_repo
            .get_for_media_item(media)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(stored.revision.as_deref(), Some(user_revision.as_str()));
        assert!(matches!(
            stored.locator,
            Locator::Comic(ComicLocator { page_index: 0, .. })
        ));
        let snapshot = repo.get_snapshot(migration.id).await.unwrap().unwrap();
        assert_eq!(snapshot.state, ProgressMigrationState::Applied);
    }

    #[allow(dead_code)]
    fn _future_media_fixture(edition_id: EditionId) -> MediaItem {
        MediaItem {
            id: MediaItemId::new(),
            edition_id,
            parent_id: None,
            media_type: MediaType::Comic,
            title: String::new(),
            index: MediaIndex::Movie,
            duration_ms: None,
            page_count: None,
            chapter_count: None,
            published_at: None,
            status: MediaItemStatus::Available,
            created_at: UtcMillis(1),
            updated_at: UtcMillis(1),
        }
    }
}
