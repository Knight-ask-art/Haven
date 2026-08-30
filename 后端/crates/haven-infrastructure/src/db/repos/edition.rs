//! Edition Repository（Sqlite）。
//!
//! 注意：media_items 对 editions 是 ON DELETE RESTRICT，
//! 存在子媒体条目时 `delete` 返回数据库错误 —— 数据安全设计。

use std::sync::Arc;

use async_trait::async_trait;

use haven_common::AppError;
use haven_domain::contracts::EditionRepository;
use haven_domain::entities::Edition;
use haven_domain::ids::{EditionId, WorkId};

use crate::db::Db;
use crate::db::repos::{
    artwork_from_row, artwork_to_json, enum_to_db_str, id_from_row, map_db_error,
};

pub struct SqliteEditionRepository {
    db: Arc<Db>,
}

impl SqliteEditionRepository {
    pub fn new(db: Arc<Db>) -> Self {
        Self { db }
    }
}

fn row_to_edition(row: &rusqlite::Row<'_>) -> rusqlite::Result<Edition> {
    let edition_type: String = row.get("edition_type")?;
    Ok(Edition {
        id: id_from_row::<EditionId>(row.get("id")?)?,
        work_id: id_from_row::<WorkId>(row.get("work_id")?)?,
        title: row.get("title")?,
        subtitle: row.get("subtitle")?,
        edition_type: parse_enum(&edition_type)?,
        release_date: row.get("release_date")?,
        language: row.get("language")?,
        region: row.get("region")?,
        publisher_or_studio: row.get("publisher_or_studio")?,
        description: row.get("description")?,
        artwork: artwork_from_row(row)?,
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

const SELECT_COLUMNS: &str = "id, work_id, title, subtitle, edition_type, release_date, language, region, publisher_or_studio, description, poster, cover, backdrop, thumbnail, created_at, updated_at";

#[async_trait]
impl EditionRepository for SqliteEditionRepository {
    async fn get(&self, id: EditionId) -> Result<Option<Edition>, AppError> {
        let conn = self.db.lock();
        let mut stmt = conn
            .prepare(&format!(
                "SELECT {SELECT_COLUMNS} FROM editions WHERE id = ?1"
            ))
            .map_err(map_db_error("查询版本失败"))?;
        let mut rows = stmt
            .query_map(rusqlite::params![id.to_string()], row_to_edition)
            .map_err(map_db_error("查询版本失败"))?;
        rows.next()
            .transpose()
            .map_err(map_db_error("查询版本失败"))
    }

    async fn save(&self, edition: &Edition) -> Result<(), AppError> {
        let conn = self.db.lock();
        save_on_conn(&conn, edition)
    }

    async fn list_by_work(&self, work_id: WorkId) -> Result<Vec<Edition>, AppError> {
        let conn = self.db.lock();
        let mut stmt = conn
            .prepare(&format!(
                "SELECT {SELECT_COLUMNS} FROM editions WHERE work_id = ?1 ORDER BY created_at"
            ))
            .map_err(map_db_error("查询版本列表失败"))?;
        let rows = stmt
            .query_map(rusqlite::params![work_id.to_string()], row_to_edition)
            .map_err(map_db_error("查询版本列表失败"))?;
        rows.collect::<rusqlite::Result<Vec<_>>>()
            .map_err(map_db_error("查询版本列表失败"))
    }

    async fn list_by_works(&self, work_ids: &[WorkId]) -> Result<Vec<Edition>, AppError> {
        if work_ids.is_empty() {
            return Ok(Vec::new());
        }
        let conn = self.db.lock();
        let placeholders: Vec<String> = (1..=work_ids.len()).map(|i| format!("?{i}")).collect();
        let sql = format!(
            "SELECT {SELECT_COLUMNS} FROM editions WHERE work_id IN ({}) ORDER BY created_at",
            placeholders.join(",")
        );
        let mut stmt = conn
            .prepare(&sql)
            .map_err(map_db_error("批量查询版本失败"))?;
        let params: Vec<String> = work_ids.iter().map(|id| id.to_string()).collect();
        let params: Vec<&dyn rusqlite::ToSql> =
            params.iter().map(|s| s as &dyn rusqlite::ToSql).collect();
        let rows = stmt
            .query_map(params.as_slice(), row_to_edition)
            .map_err(map_db_error("批量查询版本失败"))?;
        rows.collect::<rusqlite::Result<Vec<_>>>()
            .map_err(map_db_error("批量查询版本失败"))
    }

    async fn delete(&self, id: EditionId) -> Result<bool, AppError> {
        let conn = self.db.lock();
        let affected = conn
            .execute(
                "DELETE FROM editions WHERE id = ?1",
                rusqlite::params![id.to_string()],
            )
            .map_err(map_db_error("删除版本失败（可能被子条目引用，RESTRICT）"))?;
        Ok(affected > 0)
    }
}

pub(crate) fn save_on_conn(conn: &rusqlite::Connection, edition: &Edition) -> Result<(), AppError> {
    let [poster, cover, backdrop, thumbnail] = artwork_to_json(&edition.artwork)?;
    conn.execute(
        "INSERT INTO editions
                (id, work_id, title, subtitle, edition_type, release_date, language, region,
                 publisher_or_studio, description, poster, cover, backdrop, thumbnail,
                 created_at, updated_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16)
             ON CONFLICT(id) DO UPDATE SET
                 work_id = excluded.work_id,
                 title = excluded.title,
                 subtitle = excluded.subtitle,
                 edition_type = excluded.edition_type,
                 release_date = excluded.release_date,
                 language = excluded.language,
                 region = excluded.region,
                 publisher_or_studio = excluded.publisher_or_studio,
                 description = excluded.description,
                 poster = excluded.poster,
                 cover = excluded.cover,
                 backdrop = excluded.backdrop,
                 thumbnail = excluded.thumbnail,
                 updated_at = excluded.updated_at",
        rusqlite::params![
            edition.id.to_string(),
            edition.work_id.to_string(),
            edition.title,
            edition.subtitle,
            enum_to_db_str(&edition.edition_type)?,
            edition.release_date,
            edition.language,
            edition.region,
            edition.publisher_or_studio,
            edition.description,
            poster,
            cover,
            backdrop,
            thumbnail,
            edition.created_at.0,
            edition.updated_at.0,
        ],
    )
    .map_err(map_db_error("保存版本失败"))?;
    Ok(())
}

#[cfg(test)]
#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::Db;
    use haven_domain::entities::ArtworkSet;
    use haven_domain::enums::MediaType;

    fn sample_edition(work_id: WorkId) -> Edition {
        Edition {
            id: EditionId::new(),
            work_id,
            title: "三体（2023 电视剧）".into(),
            subtitle: None,
            edition_type: MediaType::Series,
            release_date: Some("2023-01-15".into()),
            language: Some("zh".into()),
            region: Some("CN".into()),
            publisher_or_studio: Some("腾讯视频".into()),
            description: None,
            artwork: ArtworkSet::default(),
            created_at: haven_common::UtcMillis(1_000),
            updated_at: haven_common::UtcMillis(1_000),
        }
    }

    fn seed_work(db: &Db) -> WorkId {
        let work_id = WorkId::new();
        let now = haven_common::UtcMillis::now().0;
        db.lock()
            .execute(
                "INSERT INTO works (id, canonical_title, work_type, status, created_at, updated_at)
                 VALUES (?1, '三体', 'fiction', 'completed', ?2, ?2)",
                rusqlite::params![work_id.to_string(), now],
            )
            .unwrap();
        work_id
    }

    #[tokio::test]
    async fn save_get_roundtrip() {
        let db = Arc::new(Db::open_in_memory().unwrap());
        let work_id = seed_work(&db);
        let repo = SqliteEditionRepository::new(db);
        let edition = sample_edition(work_id);

        repo.save(&edition).await.unwrap();
        let read = repo.get(edition.id).await.unwrap().expect("存在");
        assert_eq!(read, edition);
        assert_eq!(read.edition_type, MediaType::Series);
    }

    #[tokio::test]
    async fn list_by_work_returns_all_editions() {
        let db = Arc::new(Db::open_in_memory().unwrap());
        let work_id = seed_work(&db);
        let repo = SqliteEditionRepository::new(db);

        let mut e1 = sample_edition(work_id);
        e1.title = "原著小说".into();
        let mut e2 = sample_edition(work_id);
        e2.title = "漫画改编".into();
        repo.save(&e1).await.unwrap();
        repo.save(&e2).await.unwrap();

        let listed = repo.list_by_work(work_id).await.unwrap();
        assert_eq!(listed.len(), 2);
        assert_eq!(listed[0].title, "原著小说");
        assert_eq!(listed[1].title, "漫画改编");
    }

    #[tokio::test]
    async fn delete_returns_false_for_missing() {
        let db = Arc::new(Db::open_in_memory().unwrap());
        let repo = SqliteEditionRepository::new(db);
        assert!(!repo.delete(EditionId::new()).await.unwrap());
    }
}
