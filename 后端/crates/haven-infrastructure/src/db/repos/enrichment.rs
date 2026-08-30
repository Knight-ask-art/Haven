//! Enrichment Repository（Sqlite）：流水线状态持久化（migration 017）。
//!
//! - 每 Work 至多一条（PRIMARY KEY upsert）。
//! - status 为闭合枚举字符串，非法值读取时显式报错（不静默兜底）。

use std::sync::Arc;

use async_trait::async_trait;

use haven_common::{AppError, UtcMillis};
use haven_domain::contracts::{EnrichmentRepository, EnrichmentState};
use haven_domain::ids::WorkId;
use rusqlite::OptionalExtension;

use crate::db::Db;
use crate::db::repos::{id_from_row, map_db_error};

pub struct SqliteEnrichmentRepository {
    db: Arc<Db>,
}

impl SqliteEnrichmentRepository {
    pub fn new(db: Arc<Db>) -> Self {
        Self { db }
    }
}

const VALID_STATUSES: [&str; 3] = ["pending", "enriched", "failed"];

fn validate_status(status: &str) -> Result<(), AppError> {
    if VALID_STATUSES.contains(&status) {
        Ok(())
    } else {
        Err(AppError::new(
            "INVALID_ARGUMENT",
            haven_common::ErrorKind::Validation,
            "enrichment 状态非法",
            false,
        ))
    }
}

#[async_trait]
impl EnrichmentRepository for SqliteEnrichmentRepository {
    async fn get(&self, work_id: WorkId) -> Result<Option<EnrichmentState>, AppError> {
        let conn = self.db.lock();
        let row = conn
            .query_row(
                "SELECT work_id, status, source_id, error, updated_at
                 FROM enrichment_state WHERE work_id = ?1",
                rusqlite::params![work_id.to_string()],
                |row| {
                    Ok(EnrichmentState {
                        work_id: id_from_row(row.get::<_, String>("work_id")?)?,
                        status: row.get("status")?,
                        source_id: row.get("source_id")?,
                        error: row.get("error")?,
                        updated_at: UtcMillis(row.get("updated_at")?),
                    })
                },
            )
            .optional()
            .map_err(map_db_error("查询 enrichment 状态失败"))?;
        Ok(row)
    }

    async fn list(&self, work_id: Option<WorkId>) -> Result<Vec<EnrichmentState>, AppError> {
        let (sql, param) = match work_id {
            Some(id) => (
                "SELECT work_id, status, source_id, error, updated_at
                 FROM enrichment_state WHERE work_id = ?1 ORDER BY updated_at DESC",
                Some(id.to_string()),
            ),
            None => (
                "SELECT work_id, status, source_id, error, updated_at
                 FROM enrichment_state ORDER BY updated_at DESC",
                None,
            ),
        };
        let conn = self.db.lock();
        let mut stmt = conn
            .prepare(sql)
            .map_err(map_db_error("查询 enrichment 列表失败"))?;
        let rows = stmt
            .query_map(rusqlite::params_from_iter(param.iter()), |row| {
                Ok(EnrichmentState {
                    work_id: id_from_row(row.get::<_, String>("work_id")?)?,
                    status: row.get("status")?,
                    source_id: row.get("source_id")?,
                    error: row.get("error")?,
                    updated_at: UtcMillis(row.get("updated_at")?),
                })
            })
            .map_err(map_db_error("查询 enrichment 列表失败"))?;
        rows.collect::<rusqlite::Result<Vec<_>>>()
            .map_err(map_db_error("查询 enrichment 列表失败"))
    }

    async fn upsert(&self, state: &EnrichmentState) -> Result<(), AppError> {
        validate_status(&state.status)?;
        let conn = self.db.lock();
        conn.execute(
            "INSERT INTO enrichment_state (work_id, status, source_id, error, updated_at)
             VALUES (?1, ?2, ?3, ?4, ?5)
             ON CONFLICT(work_id) DO UPDATE SET
                 status = excluded.status,
                 source_id = excluded.source_id,
                 error = excluded.error,
                 updated_at = excluded.updated_at",
            rusqlite::params![
                state.work_id.to_string(),
                state.status,
                state.source_id,
                state.error,
                state.updated_at.0,
            ],
        )
        .map_err(map_db_error("保存 enrichment 状态失败"))?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use haven_domain::ids::WorkId;

    fn state(work_id: WorkId, status: &str) -> EnrichmentState {
        EnrichmentState {
            work_id,
            status: status.into(),
            source_id: Some("cms10".into()),
            error: None,
            updated_at: UtcMillis::now(),
        }
    }

    #[tokio::test]
    async fn upsert_get_roundtrip_and_overwrite() {
        let db = Arc::new(Db::open_in_memory().unwrap());
        let repo = SqliteEnrichmentRepository::new(db);
        let work = WorkId::new();

        repo.upsert(&state(work, "pending")).await.unwrap();
        let read = repo.get(work).await.unwrap().expect("存在");
        assert_eq!(read.status, "pending");

        repo.upsert(&state(work, "enriched")).await.unwrap();
        let read = repo.get(work).await.unwrap().unwrap();
        assert_eq!(read.status, "enriched", "upsert 应覆盖");
        assert_eq!(repo.list(None).await.unwrap().len(), 1);
    }

    #[tokio::test]
    async fn invalid_status_is_rejected_and_list_filters() {
        let db = Arc::new(Db::open_in_memory().unwrap());
        let repo = SqliteEnrichmentRepository::new(db);
        assert!(repo.upsert(&state(WorkId::new(), "bogus")).await.is_err());

        let a = WorkId::new();
        let b = WorkId::new();
        repo.upsert(&state(a, "failed")).await.unwrap();
        repo.upsert(&state(b, "enriched")).await.unwrap();
        assert_eq!(repo.list(Some(a)).await.unwrap().len(), 1);
        assert_eq!(repo.list(Some(a)).await.unwrap()[0].work_id, a);
        assert_eq!(repo.list(None).await.unwrap().len(), 2);
        assert_eq!(repo.get(WorkId::new()).await.unwrap(), None);
    }
}
