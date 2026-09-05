//! DownloadTask Repository（SQLite）。

use std::sync::Arc;

use async_trait::async_trait;
use haven_common::AppError;
use haven_domain::contracts::DownloadRepository;
use haven_domain::entities::DownloadTask;
use haven_domain::enums::DownloadState;
use haven_domain::ids::{
    DownloadBatchId, DownloadTaskId, EditionId, MediaItemId, ResourceId, StorageLocationId, WorkId,
};

use crate::db::Db;
use crate::db::repos::{enum_to_db_str, id_from_row, map_db_error};

pub struct SqliteDownloadRepository {
    db: Arc<Db>,
}

impl SqliteDownloadRepository {
    pub fn new(db: Arc<Db>) -> Self {
        Self { db }
    }

    /// Composition Root 在服务可见前执行；避免把上次进程的运行态伪装成仍在传输。
    pub fn recover_interrupted(&self) -> Result<u64, AppError> {
        let conn = self.db.lock();
        let affected = conn
            .execute(
                "UPDATE download_tasks
                 SET state = 'interrupted', updated_at = ?1
                 WHERE state IN ('resolving', 'downloading', 'verifying')",
                rusqlite::params![haven_common::UtcMillis::now().0],
            )
            .map_err(map_db_error("恢复下载任务失败"))?;
        Ok(affected as u64)
    }
}

const SELECT_COLUMNS: &str = "id, work_id, edition_id, media_item_id, source_resource_id, \
    target_storage_id, offline_resource_id, state, bytes_total, bytes_downloaded, speed_bps, \
    eta_seconds, created_at, updated_at, batch_id, priority, provider_key, host_key, variant_key, \
    resource_identity, retry_count, not_before, resumable";

fn parse_optional_id<T>(value: Option<String>) -> rusqlite::Result<Option<T>>
where
    T: std::str::FromStr,
{
    value.map(id_from_row).transpose()
}

fn row_to_download_task(row: &rusqlite::Row<'_>) -> rusqlite::Result<DownloadTask> {
    let state: String = row.get("state")?;
    let priority_str: Option<String> = row.get("priority").unwrap_or(None);
    let priority = priority_str
        .and_then(|s| serde_json::from_str(&format!("\"{s}\"")).ok())
        .unwrap_or(haven_domain::enums::DownloadPriority::Normal);
    Ok(DownloadTask {
        id: id_from_row::<DownloadTaskId>(row.get("id")?)?,
        work_id: parse_optional_id::<WorkId>(row.get("work_id")?)?,
        edition_id: parse_optional_id::<EditionId>(row.get("edition_id")?)?,
        media_item_id: parse_optional_id::<MediaItemId>(row.get("media_item_id")?)?,
        source_resource_id: id_from_row::<ResourceId>(row.get("source_resource_id")?)?,
        target_storage_id: id_from_row::<StorageLocationId>(row.get("target_storage_id")?)?,
        offline_resource_id: parse_optional_id::<ResourceId>(row.get("offline_resource_id")?)?,
        state: parse_state(&state)?,
        bytes_total: row
            .get::<_, Option<i64>>("bytes_total")?
            .map(|value| value as u64),
        bytes_downloaded: row.get::<_, i64>("bytes_downloaded")? as u64,
        speed_bps: row
            .get::<_, Option<i64>>("speed_bps")?
            .map(|value| value as u64),
        eta_seconds: row
            .get::<_, Option<i64>>("eta_seconds")?
            .map(|value| value as u64),
        created_at: haven_common::UtcMillis(row.get("created_at")?),
        updated_at: haven_common::UtcMillis(row.get("updated_at")?),
        batch_id: row
            .get::<_, Option<String>>("batch_id")
            .unwrap_or(None)
            .and_then(|s| s.parse().ok()),
        priority,
        provider_key: row.get::<_, Option<String>>("provider_key").unwrap_or(None),
        host_key: row.get::<_, Option<String>>("host_key").unwrap_or(None),
        variant_key: row
            .get::<_, Option<String>>("variant_key")
            .unwrap_or(None)
            .unwrap_or_default(),
        resource_identity: row
            .get::<_, Option<String>>("resource_identity")
            .unwrap_or(None),
        retry_count: row
            .get::<_, Option<i64>>("retry_count")
            .unwrap_or(Some(0))
            .unwrap_or(0) as u32,
        not_before: row
            .get::<_, Option<i64>>("not_before")
            .unwrap_or(None)
            .map(haven_common::UtcMillis),
        resumable: row
            .get::<_, Option<i64>>("resumable")
            .unwrap_or(None)
            .map(|v| v != 0),
    })
}

fn parse_state(value: &str) -> rusqlite::Result<DownloadState> {
    serde_json::from_str(&format!("\"{value}\"")).map_err(|error| {
        rusqlite::Error::FromSqlConversionFailure(0, rusqlite::types::Type::Text, Box::new(error))
    })
}

#[async_trait]
impl DownloadRepository for SqliteDownloadRepository {
    async fn get(&self, id: DownloadTaskId) -> Result<Option<DownloadTask>, AppError> {
        let conn = self.db.lock();
        let mut stmt = conn
            .prepare(&format!(
                "SELECT {SELECT_COLUMNS} FROM download_tasks WHERE id = ?1"
            ))
            .map_err(map_db_error("查询下载任务失败"))?;
        let mut rows = stmt
            .query_map(rusqlite::params![id.to_string()], row_to_download_task)
            .map_err(map_db_error("查询下载任务失败"))?;
        rows.next()
            .transpose()
            .map_err(map_db_error("查询下载任务失败"))
    }

    async fn save(&self, task: &DownloadTask) -> Result<(), AppError> {
        let conn = self.db.lock();
        conn.execute(
            "INSERT INTO download_tasks
                (id, work_id, edition_id, media_item_id, source_resource_id,
                 target_storage_id, offline_resource_id, state, bytes_total, bytes_downloaded,
                 speed_bps, eta_seconds, created_at, updated_at, batch_id, priority, provider_key,
                 host_key, variant_key, resource_identity, retry_count, not_before, resumable)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17, ?18, ?19, ?20, ?21, ?22, ?23)
             ON CONFLICT(id) DO UPDATE SET
                 state = excluded.state,
                 offline_resource_id = excluded.offline_resource_id,
                 bytes_total = excluded.bytes_total,
                 bytes_downloaded = excluded.bytes_downloaded,
                 speed_bps = excluded.speed_bps,
                 eta_seconds = excluded.eta_seconds,
                 updated_at = excluded.updated_at,
                 batch_id = excluded.batch_id,
                 priority = excluded.priority,
                 provider_key = excluded.provider_key,
                 host_key = excluded.host_key,
                 variant_key = excluded.variant_key,
                 resource_identity = excluded.resource_identity,
                 retry_count = excluded.retry_count,
                 not_before = excluded.not_before,
                 resumable = excluded.resumable",
            rusqlite::params![
                task.id.to_string(),
                task.work_id.map(|id| id.to_string()),
                task.edition_id.map(|id| id.to_string()),
                task.media_item_id.map(|id| id.to_string()),
                task.source_resource_id.to_string(),
                task.target_storage_id.to_string(),
                task.offline_resource_id.map(|id| id.to_string()),
                enum_to_db_str(&task.state)?,
                task.bytes_total.map(|value| value as i64),
                task.bytes_downloaded as i64,
                task.speed_bps.map(|value| value as i64),
                task.eta_seconds.map(|value| value as i64),
                task.created_at.0,
                task.updated_at.0,
                task.batch_id.map(|id| id.to_string()),
                enum_to_db_str(&task.priority)?,
                task.provider_key,
                task.host_key,
                task.variant_key,
                task.resource_identity,
                task.retry_count as i64,
                task.not_before.map(|v| v.0),
                task.resumable.map(|v| if v { 1 } else { 0 }),
            ],
        )
        .map_err(|e| {
            eprintln!("save download error: {:?}", e);
            map_db_error("保存下载任务失败")(e)
        })?;
        Ok(())
    }

    async fn list(&self, limit: u32) -> Result<Vec<DownloadTask>, AppError> {
        let conn = self.db.lock();
        let mut stmt = conn
            .prepare(&format!(
                "SELECT {SELECT_COLUMNS} FROM download_tasks
                 ORDER BY created_at DESC LIMIT ?1"
            ))
            .map_err(map_db_error("查询下载列表失败"))?;
        let rows = stmt
            .query_map(rusqlite::params![limit], row_to_download_task)
            .map_err(map_db_error("查询下载列表失败"))?;
        rows.collect::<rusqlite::Result<Vec<_>>>()
            .map_err(map_db_error("查询下载列表失败"))
    }

    async fn find_active(
        &self,
        source_resource_id: ResourceId,
        target_storage_id: StorageLocationId,
    ) -> Result<Option<DownloadTask>, AppError> {
        let conn = self.db.lock();
        let mut stmt = conn
            .prepare(&format!(
                "SELECT {SELECT_COLUMNS} FROM download_tasks
                 WHERE source_resource_id = ?1 AND target_storage_id = ?2
                   AND state IN ('queued', 'resolving', 'downloading', 'paused', 'verifying', 'interrupted')
                 ORDER BY created_at DESC LIMIT 1"
            ))
            .map_err(map_db_error("查询现有下载任务失败"))?;
        let mut rows = stmt
            .query_map(
                rusqlite::params![
                    source_resource_id.to_string(),
                    target_storage_id.to_string()
                ],
                row_to_download_task,
            )
            .map_err(map_db_error("查询现有下载任务失败"))?;
        rows.next()
            .transpose()
            .map_err(map_db_error("查询现有下载任务失败"))
    }

    async fn delete_terminal(&self, id: DownloadTaskId) -> Result<bool, AppError> {
        let conn = self.db.lock();
        let affected = conn
            .execute(
                "DELETE FROM download_tasks
                 WHERE id = ?1 AND state IN ('completed', 'failed', 'cancelled')",
                rusqlite::params![id.to_string()],
            )
            .map_err(map_db_error("移除下载记录失败"))?;
        Ok(affected > 0)
    }

    async fn associate_offline_resource(
        &self,
        id: DownloadTaskId,
        expected: DownloadState,
        resource_id: ResourceId,
    ) -> Result<bool, AppError> {
        let conn = self.db.lock();
        let affected = conn
            .execute(
                "UPDATE download_tasks
                 SET offline_resource_id = ?1, updated_at = ?2
                 WHERE id = ?3 AND state = ?4",
                rusqlite::params![
                    resource_id.to_string(),
                    haven_common::UtcMillis::now().0,
                    id.to_string(),
                    enum_to_db_str(&expected)?,
                ],
            )
            .map_err(map_db_error("关联离线资源失败"))?;
        Ok(affected > 0)
    }

    async fn compare_and_set_state(
        &self,
        id: DownloadTaskId,
        expected: DownloadState,
        next: DownloadState,
    ) -> Result<bool, AppError> {
        let conn = self.db.lock();
        let affected = conn
            .execute(
                "UPDATE download_tasks
                 SET state = ?1,
                     speed_bps = CASE WHEN ?1 = 'downloading' THEN speed_bps ELSE NULL END,
                     eta_seconds = CASE WHEN ?1 = 'downloading' THEN eta_seconds ELSE NULL END,
                     updated_at = ?2
                 WHERE id = ?3 AND state = ?4",
                rusqlite::params![
                    enum_to_db_str(&next)?,
                    haven_common::UtcMillis::now().0,
                    id.to_string(),
                    enum_to_db_str(&expected)?,
                ],
            )
            .map_err(map_db_error("更新下载状态失败"))?;
        Ok(affected > 0)
    }

    async fn update_progress(
        &self,
        id: DownloadTaskId,
        expected: DownloadState,
        bytes_total: Option<u64>,
        bytes_downloaded: u64,
        speed_bps: Option<u64>,
        eta_seconds: Option<u64>,
    ) -> Result<bool, AppError> {
        let conn = self.db.lock();
        let affected = conn
            .execute(
                "UPDATE download_tasks
                 SET bytes_total = ?1, bytes_downloaded = ?2, speed_bps = ?3,
                     eta_seconds = ?4, updated_at = ?5
                 WHERE id = ?6 AND state = ?7",
                rusqlite::params![
                    bytes_total.map(|value| value as i64),
                    bytes_downloaded as i64,
                    speed_bps.map(|value| value as i64),
                    eta_seconds.map(|value| value as i64),
                    haven_common::UtcMillis::now().0,
                    id.to_string(),
                    enum_to_db_str(&expected)?,
                ],
            )
            .map_err(map_db_error("更新下载进度失败"))?;
        Ok(affected > 0)
    }

    async fn mark_active_interrupted(&self) -> Result<u64, AppError> {
        self.recover_interrupted()
    }

    async fn list_by_batch(
        &self,
        batch_id: DownloadBatchId,
    ) -> Result<Vec<DownloadTask>, AppError> {
        let conn = self.db.lock();
        let mut stmt = conn
            .prepare(&format!(
                "SELECT {SELECT_COLUMNS} FROM download_tasks WHERE batch_id = ?1
                 ORDER BY created_at ASC"
            ))
            .map_err(map_db_error("查询批次任务失败"))?;
        let rows = stmt
            .query_map(
                rusqlite::params![batch_id.to_string()],
                row_to_download_task,
            )
            .map_err(map_db_error("查询批次任务失败"))?;
        rows.collect::<rusqlite::Result<Vec<_>>>()
            .map_err(map_db_error("查询批次任务失败"))
    }

    async fn list_schedulable(
        &self,
        limit: u32,
        now: haven_common::UtcMillis,
    ) -> Result<Vec<DownloadTask>, AppError> {
        let conn = self.db.lock();
        let mut stmt = conn
            .prepare(&format!(
                "SELECT {SELECT_COLUMNS} FROM download_tasks
                 WHERE state = 'queued'
                   AND (not_before IS NULL OR not_before <= ?1)
                 ORDER BY
                   CASE priority WHEN 'high' THEN 0 WHEN 'normal' THEN 1 ELSE 2 END,
                   created_at ASC
                 LIMIT ?2"
            ))
            .map_err(map_db_error("查询可调度任务失败"))?;
        let rows = stmt
            .query_map(rusqlite::params![now.0, limit], row_to_download_task)
            .map_err(map_db_error("查询可调度任务失败"))?;
        rows.collect::<rusqlite::Result<Vec<_>>>()
            .map_err(map_db_error("查询可调度任务失败"))
    }
}

/// SQLite 批次聚合仓储（026_download_batches）。
pub struct SqliteDownloadBatchRepository {
    db: Arc<Db>,
}

impl SqliteDownloadBatchRepository {
    pub fn new(db: Arc<Db>) -> Self {
        Self { db }
    }
}

fn row_to_download_batch(
    row: &rusqlite::Row<'_>,
) -> rusqlite::Result<haven_domain::entities::DownloadBatch> {
    let category: String = row.get("category")?;
    let state: String = row.get("state")?;
    let category = serde_json::from_str(&format!("\"{category}\"")).map_err(|error| {
        rusqlite::Error::FromSqlConversionFailure(0, rusqlite::types::Type::Text, Box::new(error))
    })?;
    let state = serde_json::from_str(&format!("\"{state}\"")).map_err(|error| {
        rusqlite::Error::FromSqlConversionFailure(0, rusqlite::types::Type::Text, Box::new(error))
    })?;
    Ok(haven_domain::entities::DownloadBatch {
        id: id_from_row::<DownloadBatchId>(row.get("id")?)?,
        title: row.get("title")?,
        category,
        subject_type: row.get("subject_type")?,
        subject_id: row.get("subject_id")?,
        target_storage_id: id_from_row::<haven_domain::ids::StorageLocationId>(
            row.get("target_storage_id")?,
        )?,
        state,
        total_tasks: row.get::<_, i64>("total_tasks")? as u32,
        completed_tasks: row.get::<_, i64>("completed_tasks")? as u32,
        total_bytes: row.get::<_, Option<i64>>("total_bytes")?.map(|v| v as u64),
        completed_bytes: row.get::<_, i64>("completed_bytes")? as u64,
        created_at: haven_common::UtcMillis(row.get("created_at")?),
        updated_at: haven_common::UtcMillis(row.get("updated_at")?),
    })
}

#[async_trait]
impl haven_domain::contracts::DownloadBatchRepository for SqliteDownloadBatchRepository {
    async fn get(
        &self,
        id: DownloadBatchId,
    ) -> Result<Option<haven_domain::entities::DownloadBatch>, AppError> {
        let conn = self.db.lock();
        let mut stmt = conn
            .prepare(
                "SELECT id, title, category, subject_type, subject_id, target_storage_id,
                        state, total_tasks, completed_tasks, total_bytes, completed_bytes,
                        created_at, updated_at
                 FROM download_batches WHERE id = ?1",
            )
            .map_err(map_db_error("查询下载批次失败"))?;
        let mut rows = stmt
            .query_map(rusqlite::params![id.to_string()], row_to_download_batch)
            .map_err(map_db_error("查询下载批次失败"))?;
        rows.next()
            .transpose()
            .map_err(map_db_error("查询下载批次失败"))
    }

    async fn save(&self, batch: &haven_domain::entities::DownloadBatch) -> Result<(), AppError> {
        let conn = self.db.lock();
        conn.execute(
            "INSERT INTO download_batches
                (id, title, category, subject_type, subject_id, target_storage_id, state,
                 total_tasks, completed_tasks, total_bytes, completed_bytes, created_at, updated_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13)
             ON CONFLICT(id) DO UPDATE SET
                 state = excluded.state,
                 total_tasks = excluded.total_tasks,
                 completed_tasks = excluded.completed_tasks,
                 total_bytes = excluded.total_bytes,
                 completed_bytes = excluded.completed_bytes,
                 updated_at = excluded.updated_at",
            rusqlite::params![
                batch.id.to_string(),
                batch.title,
                enum_to_db_str(&batch.category)?,
                batch.subject_type,
                batch.subject_id,
                batch.target_storage_id.to_string(),
                enum_to_db_str(&batch.state)?,
                batch.total_tasks as i64,
                batch.completed_tasks as i64,
                batch.total_bytes.map(|v| v as i64),
                batch.completed_bytes as i64,
                batch.created_at.0,
                batch.updated_at.0,
            ],
        )
        .map_err(map_db_error("保存下载批次失败"))?;
        Ok(())
    }

    async fn list(
        &self,
        limit: u32,
        offset: u32,
    ) -> Result<Vec<haven_domain::entities::DownloadBatch>, AppError> {
        let conn = self.db.lock();
        let mut stmt = conn
            .prepare(
                "SELECT id, title, category, subject_type, subject_id, target_storage_id,
                        state, total_tasks, completed_tasks, total_bytes, completed_bytes,
                        created_at, updated_at
                 FROM download_batches
                 ORDER BY created_at DESC LIMIT ?1 OFFSET ?2",
            )
            .map_err(map_db_error("查询下载批次列表失败"))?;
        let rows = stmt
            .query_map(rusqlite::params![limit, offset], row_to_download_batch)
            .map_err(map_db_error("查询下载批次列表失败"))?;
        rows.collect::<rusqlite::Result<Vec<_>>>()
            .map_err(map_db_error("查询下载批次列表失败"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use haven_domain::contracts::DownloadRepository;

    #[tokio::test]
    async fn state_transition_is_compare_and_set() {
        let db = Arc::new(Db::open_in_memory().unwrap());
        let repo = SqliteDownloadRepository::new(db.clone());
        let now = haven_common::UtcMillis::now();
        let ids = seed_dependencies(&db, now);
        let task = DownloadTask {
            id: DownloadTaskId::new(),
            work_id: Some(ids.0),
            edition_id: Some(ids.1),
            media_item_id: Some(ids.2),
            source_resource_id: ids.3,
            target_storage_id: ids.4,
            offline_resource_id: None,
            state: DownloadState::Queued,
            bytes_total: Some(10),
            bytes_downloaded: 0,
            speed_bps: None,
            eta_seconds: None,
            created_at: now,
            updated_at: now,
            batch_id: None,
            priority: haven_domain::enums::DownloadPriority::Normal,
            provider_key: None,
            host_key: None,
            variant_key: String::new(),
            resource_identity: None,
            retry_count: 0,
            not_before: None,
            resumable: None,
        };
        repo.save(&task).await.unwrap();

        assert!(
            repo.compare_and_set_state(task.id, DownloadState::Queued, DownloadState::Downloading)
                .await
                .unwrap()
        );
        assert!(
            !repo
                .compare_and_set_state(task.id, DownloadState::Queued, DownloadState::Paused)
                .await
                .unwrap()
        );
        assert_eq!(
            repo.get(task.id).await.unwrap().unwrap().state,
            DownloadState::Downloading
        );
    }

    #[tokio::test]
    async fn removing_terminal_record_preserves_offline_resource() {
        let db = Arc::new(Db::open_in_memory().unwrap());
        let repo = SqliteDownloadRepository::new(db.clone());
        let now = haven_common::UtcMillis::now();
        let ids = seed_dependencies(&db, now);
        let offline_resource_id = ResourceId::new();
        {
            let conn = db.lock();
            conn.execute(
                "INSERT INTO resources
                    (id, media_item_id, resource_type, storage_location_id, locator_kind,
                     locator_json, availability, availability_source, created_at, updated_at)
                 VALUES (?1, ?2, 'local_file', ?3, 'local_path',
                         '{\"local_path\":{\"path\":\"D:/Offline/.haven/offline/task.bin\"}}',
                         'offline_available', 'user', ?4, ?4)",
                rusqlite::params![
                    offline_resource_id.to_string(),
                    ids.2.to_string(),
                    ids.4.to_string(),
                    now.0,
                ],
            )
            .unwrap();
        }
        let task = DownloadTask {
            id: DownloadTaskId::new(),
            work_id: Some(ids.0),
            edition_id: Some(ids.1),
            media_item_id: Some(ids.2),
            source_resource_id: ids.3,
            target_storage_id: ids.4,
            offline_resource_id: Some(offline_resource_id),
            state: DownloadState::Completed,
            bytes_total: Some(10),
            bytes_downloaded: 10,
            speed_bps: None,
            eta_seconds: None,
            created_at: now,
            updated_at: now,
            batch_id: None,
            priority: haven_domain::enums::DownloadPriority::Normal,
            provider_key: None,
            host_key: None,
            variant_key: String::new(),
            resource_identity: None,
            retry_count: 0,
            not_before: None,
            resumable: None,
        };
        repo.save(&task).await.unwrap();

        assert!(repo.delete_terminal(task.id).await.unwrap());
        assert!(repo.get(task.id).await.unwrap().is_none());
        let resource_count: i64 = db
            .lock()
            .query_row(
                "SELECT COUNT(*) FROM resources WHERE id = ?1",
                rusqlite::params![offline_resource_id.to_string()],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(resource_count, 1, "移除记录不得删除 Offline Resource");
    }

    #[tokio::test]
    async fn active_record_cannot_be_removed() {
        let db = Arc::new(Db::open_in_memory().unwrap());
        let repo = SqliteDownloadRepository::new(db.clone());
        let now = haven_common::UtcMillis::now();
        let ids = seed_dependencies(&db, now);
        let task = DownloadTask {
            id: DownloadTaskId::new(),
            work_id: Some(ids.0),
            edition_id: Some(ids.1),
            media_item_id: Some(ids.2),
            source_resource_id: ids.3,
            target_storage_id: ids.4,
            offline_resource_id: None,
            state: DownloadState::Downloading,
            bytes_total: Some(10),
            bytes_downloaded: 5,
            speed_bps: None,
            eta_seconds: None,
            created_at: now,
            updated_at: now,
            batch_id: None,
            priority: haven_domain::enums::DownloadPriority::Normal,
            provider_key: None,
            host_key: None,
            variant_key: String::new(),
            resource_identity: None,
            retry_count: 0,
            not_before: None,
            resumable: None,
        };
        repo.save(&task).await.unwrap();

        assert!(!repo.delete_terminal(task.id).await.unwrap());
        assert!(repo.get(task.id).await.unwrap().is_some());
    }

    #[tokio::test]
    async fn completed_record_is_not_reused_by_active_lookup() {
        let db = Arc::new(Db::open_in_memory().unwrap());
        let repo = SqliteDownloadRepository::new(db.clone());
        let now = haven_common::UtcMillis::now();
        let ids = seed_dependencies(&db, now);
        let offline_resource_id = ResourceId::new();
        db.lock()
            .execute(
                "INSERT INTO resources
                    (id, media_item_id, resource_type, storage_location_id, locator_kind,
                     locator_json, availability, availability_source, created_at, updated_at)
                 VALUES (?1, ?2, 'local_file', ?3, 'local_path',
                         '{\"local_path\":{\"path\":\"D:/Offline/.haven/offline/task.bin\"}}',
                         'offline_available', 'user', ?4, ?4)",
                rusqlite::params![
                    offline_resource_id.to_string(),
                    ids.2.to_string(),
                    ids.4.to_string(),
                    now.0,
                ],
            )
            .unwrap();
        let task = DownloadTask {
            id: DownloadTaskId::new(),
            work_id: Some(ids.0),
            edition_id: Some(ids.1),
            media_item_id: Some(ids.2),
            source_resource_id: ids.3,
            target_storage_id: ids.4,
            offline_resource_id: Some(offline_resource_id),
            state: DownloadState::Completed,
            bytes_total: Some(10),
            bytes_downloaded: 10,
            speed_bps: None,
            eta_seconds: None,
            created_at: now,
            updated_at: now,
            batch_id: None,
            priority: haven_domain::enums::DownloadPriority::Normal,
            provider_key: None,
            host_key: None,
            variant_key: String::new(),
            resource_identity: None,
            retry_count: 0,
            not_before: None,
            resumable: None,
        };
        repo.save(&task).await.unwrap();

        let existing = repo.find_active(ids.3, ids.4).await.unwrap();
        assert!(
            existing.is_none(),
            "completed task must be checked by the application service"
        );
    }

    fn seed_dependencies(
        db: &Db,
        now: haven_common::UtcMillis,
    ) -> (
        WorkId,
        EditionId,
        MediaItemId,
        ResourceId,
        StorageLocationId,
    ) {
        let ids = (
            WorkId::new(),
            EditionId::new(),
            MediaItemId::new(),
            ResourceId::new(),
            StorageLocationId::new(),
        );
        let conn = db.lock();
        conn.execute(
            "INSERT INTO works (id, canonical_title, work_type, status, created_at, updated_at)
             VALUES (?1, '下载测试', 'standalone', 'completed', ?2, ?2)",
            rusqlite::params![ids.0.to_string(), now.0],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO editions (id, work_id, title, edition_type, created_at, updated_at)
             VALUES (?1, ?2, '版本', 'book', ?3, ?3)",
            rusqlite::params![ids.1.to_string(), ids.0.to_string(), now.0],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO media_items (id, edition_id, media_type, title, status, created_at, updated_at)
             VALUES (?1, ?2, 'book', '内容', 'available', ?3, ?3)",
            rusqlite::params![ids.2.to_string(), ids.1.to_string(), now.0],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO storage_locations
                (id, provider_type, display_name, root_ref, status, created_at, updated_at)
             VALUES (?1, 'local', '离线库', 'D:/Offline', 'connected', ?2, ?2)",
            rusqlite::params![ids.4.to_string(), now.0],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO resources
                (id, media_item_id, resource_type, locator_kind, locator_json,
                 availability, availability_source, created_at, updated_at)
             VALUES (?1, ?2, 'local_file', 'local_path',
                     '{\"local_path\":{\"path\":\"D:/source.bin\"}}',
                     'available', 'user', ?3, ?3)",
            rusqlite::params![ids.3.to_string(), ids.2.to_string(), now.0],
        )
        .unwrap();
        ids
    }
}
