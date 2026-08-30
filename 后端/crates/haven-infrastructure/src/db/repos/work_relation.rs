use haven_common::AppError;
use haven_domain::contracts::WorkRelationRepository;
use haven_domain::entities::WorkRelation;
use haven_domain::ids::WorkId;

use crate::db::Db;

fn db_error(context: &'static str) -> impl FnOnce(rusqlite::Error) -> AppError {
    move |e| {
        AppError::new(
            "DATABASE_ERROR",
            haven_common::ErrorKind::Database,
            context,
            true,
        )
        .with_source(e)
    }
}

fn row_to_relation(row: &rusqlite::Row<'_>) -> rusqlite::Result<WorkRelation> {
    let from: String = row.get(1)?;
    let to: String = row.get(2)?;
    let relation_type_raw: String = row.get(3)?;
    let from_work_id = from.parse().map_err(row_parse_error)?;
    let to_work_id = to.parse().map_err(row_parse_error)?;
    let relation_type =
        serde_json::from_str(&format!("\"{relation_type_raw}\"")).map_err(row_parse_error)?;
    Ok(WorkRelation {
        id: row.get(0)?,
        from_work_id,
        to_work_id,
        relation_type,
        evidence: row.get(4)?,
        created_at: haven_common::UtcMillis(row.get(5)?),
    })
}

fn row_parse_error(_e: impl std::fmt::Display) -> rusqlite::Error {
    rusqlite::Error::FromSqlConversionFailure(
        0,
        rusqlite::types::Type::Text,
        Box::new(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            "无法解析关联记录",
        )),
    )
}

pub struct SqliteWorkRelationRepository {
    db: std::sync::Arc<Db>,
}

impl SqliteWorkRelationRepository {
    pub fn new(db: std::sync::Arc<Db>) -> Self {
        Self { db }
    }
}

#[async_trait::async_trait]
impl WorkRelationRepository for SqliteWorkRelationRepository {
    async fn list_relations_by_work(&self, work_id: WorkId) -> Result<Vec<WorkRelation>, AppError> {
        let conn = self.db.lock();
        let mut stmt = conn
            .prepare(
                "SELECT id, from_work_id, to_work_id, relation_type, evidence, created_at
                 FROM work_relations WHERE from_work_id = ?1 OR to_work_id = ?1",
            )
            .map_err(db_error("查询关联失败"))?;
        let rows = stmt
            .query_map(rusqlite::params![work_id.to_string()], row_to_relation)
            .map_err(db_error("查询关联失败"))?;
        rows.collect::<Result<Vec<_>, _>>()
            .map_err(db_error("查询关联失败"))
    }

    async fn save_relation(&self, relation: &WorkRelation) -> Result<(), AppError> {
        let conn = self.db.lock();
        let relation_type = serde_json::to_string(&relation.relation_type)
            .map_err(|e| {
                AppError::new(
                    "DATABASE_ERROR",
                    haven_common::ErrorKind::Database,
                    "保存关联失败",
                    true,
                )
                .with_source(e)
            })?
            .trim_matches('"')
            .to_string();
        conn.execute(
            "INSERT OR IGNORE INTO work_relations (id, from_work_id, to_work_id, relation_type, evidence, created_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            rusqlite::params![
                relation.id,
                relation.from_work_id.to_string(),
                relation.to_work_id.to_string(),
                relation_type,
                relation.evidence,
                relation.created_at.0
            ],
        )
        .map_err(db_error("保存关联失败"))?;
        Ok(())
    }

    async fn delete_relation(&self, id: String) -> Result<bool, AppError> {
        let conn = self.db.lock();
        let affected = conn
            .execute(
                "DELETE FROM work_relations WHERE id = ?1",
                rusqlite::params![id],
            )
            .map_err(db_error("删除关联失败"))?;
        Ok(affected > 0)
    }
}
