//! Work Repository（Sqlite）。
//!
//! 注意：editions 表对 works 是 ON DELETE RESTRICT（防物理删除用户状态），
//! 因此存在子版本时 `delete` 返回数据库错误 —— 这是数据安全设计，不是 bug。

use std::sync::Arc;

use async_trait::async_trait;

use haven_common::{AppError, ErrorKind};
use haven_domain::contracts::{WorkOrder, WorkRepository};
use haven_domain::entities::Work;
use haven_domain::ids::WorkId;
use rusqlite::OptionalExtension;

use crate::db::Db;
use crate::db::repos::{
    artwork_from_row, artwork_to_json, enum_to_db_str, id_from_row, map_db_error,
};

pub struct SqliteWorkRepository {
    db: Arc<Db>,
}

impl SqliteWorkRepository {
    pub fn new(db: Arc<Db>) -> Self {
        Self { db }
    }
}

fn row_to_work(row: &rusqlite::Row<'_>) -> rusqlite::Result<Work> {
    let work_type: String = row.get("work_type")?;
    let status: String = row.get("status")?;
    // 019 新增列：列缺失时保底 None（migration 保证存在，此分支防异常连接）
    let director: Option<String> = row.get::<_, Option<String>>("director").unwrap_or(None);
    let actor: Option<String> = row.get::<_, Option<String>>("actor").unwrap_or(None);
    Ok(Work {
        id: id_from_row::<WorkId>(row.get("id")?)?,
        canonical_title: row.get("canonical_title")?,
        original_title: row.get("original_title")?,
        sort_title: row.get("sort_title")?,
        description: row.get("description")?,
        work_type: parse_enum(&work_type)?,
        release_year: row.get("release_year")?,
        language: row.get("language")?,
        director,
        actor,
        status: parse_enum(&status)?,
        rating_value: row.get("rating_value")?,
        rating_scale: row.get("rating_scale")?,
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

const SELECT_COLUMNS: &str = "id, canonical_title, original_title, sort_title, description, work_type, release_year, language, director, actor, status, rating_value, rating_scale, poster, cover, backdrop, thumbnail, created_at, updated_at";

/// 在指定连接上保存 Work（普通连接或事务连接复用；scanner 单文件原子写入用）。
pub(crate) fn save_on_conn(conn: &rusqlite::Connection, work: &Work) -> Result<(), AppError> {
    let [poster, cover, backdrop, thumbnail] = artwork_to_json(&work.artwork)?;
    let search_title = haven_common::tokenizer::tokenize(&work.canonical_title).join(" ");
    let search_original_title = work
        .original_title
        .as_ref()
        .map(|s| haven_common::tokenizer::tokenize(s).join(" "))
        .unwrap_or_default();
    let mut body = String::new();
    if let Some(desc) = &work.description {
        body.push_str(desc);
        body.push(' ');
    }
    if let Some(director) = &work.director {
        body.push_str(director);
        body.push(' ');
    }
    if let Some(actor) = &work.actor {
        body.push_str(actor);
    }
    let search_body = haven_common::tokenizer::tokenize(&body).join(" ");
    conn.execute(
        "INSERT INTO works
            (id, canonical_title, original_title, sort_title, description, work_type,
             release_year, language, director, actor, status, rating_value, rating_scale,
             poster, cover, backdrop, thumbnail, search_title, search_original_title, search_body,
             created_at, updated_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17, ?18, ?19, ?20, ?21, ?22)
         ON CONFLICT(id) DO UPDATE SET
             canonical_title = excluded.canonical_title,
             original_title = excluded.original_title,
             sort_title = excluded.sort_title,
             description = excluded.description,
             work_type = excluded.work_type,
             release_year = excluded.release_year,
             language = excluded.language,
             director = excluded.director,
             actor = excluded.actor,
             status = excluded.status,
             rating_value = excluded.rating_value,
             rating_scale = excluded.rating_scale,
             poster = excluded.poster,
             cover = excluded.cover,
             backdrop = excluded.backdrop,
             thumbnail = excluded.thumbnail,
             search_title = excluded.search_title,
             search_original_title = excluded.search_original_title,
             search_body = excluded.search_body,
             updated_at = excluded.updated_at",
        rusqlite::params![
            work.id.to_string(),
            work.canonical_title,
            work.original_title,
            work.sort_title,
            work.description,
            enum_to_db_str(&work.work_type)?,
            work.release_year,
            work.language,
            work.director,
            work.actor,
            enum_to_db_str(&work.status)?,
            work.rating_value,
            work.rating_scale,
            poster,
            cover,
            backdrop,
            thumbnail,
            search_title,
            search_original_title,
            search_body,
            work.created_at.0,
            work.updated_at.0,
        ],
    )
    .map_err(map_db_error("保存作品失败"))?;
    Ok(())
}

/// 在指定连接上保存来源作品去重引用（普通连接或事务连接复用）。
pub(crate) fn save_source_ref_on_conn(
    conn: &rusqlite::Connection,
    provider: &str,
    external_id: &str,
    work_id: WorkId,
) -> Result<(), AppError> {
    conn.execute(
        "INSERT INTO work_source_refs (provider, external_id, work_id) VALUES (?1, ?2, ?3)
         ON CONFLICT (provider, external_id) DO NOTHING",
        rusqlite::params![provider, external_id, work_id.to_string()],
    )
    .map_err(map_db_error("保存来源作品引用失败"))?;

    let existing: String = conn
        .query_row(
            "SELECT work_id FROM work_source_refs
             WHERE provider = ?1 AND external_id = ?2",
            rusqlite::params![provider, external_id],
            |row| row.get(0),
        )
        .map_err(map_db_error("确认来源作品引用归属失败"))?;
    if existing != work_id.to_string() {
        return Err(AppError::new(
            "SOURCE_REF_CONFLICT",
            ErrorKind::Conflict,
            "来源作品已经绑定其他 Work",
            false,
        ));
    }
    Ok(())
}

/// 构造 WHERE 子句与参数（category/media_types/query 过滤；参数顺序与 ?N 占位一致）。
/// category/media_types 匹配 media_items（category 列由 media_type 推导，同一事实源）。
/// 序列化失败显式传播（原 unwrap_or_default 会按空串过滤 → 必然空结果）。
fn build_filter(
    category: Option<haven_domain::enums::ContentCategory>,
    media_types: Option<&[haven_domain::enums::MediaType]>,
    query: Option<&str>,
) -> Result<(String, Vec<String>), AppError> {
    let mut conditions: Vec<String> = Vec::new();
    let mut params: Vec<String> = Vec::new();

    if let Some(category) = category {
        params.push(enum_to_db_str(&category)?);
        conditions.push(format!(
            "EXISTS (SELECT 1 FROM media_items m JOIN editions e ON m.edition_id = e.id
                     WHERE e.work_id = w.id AND m.category = ?{})",
            params.len()
        ));
    }

    if let Some(media_types) = media_types.filter(|t| !t.is_empty()) {
        let placeholders: Vec<String> = media_types
            .iter()
            .map(|t| {
                params.push(enum_to_db_str(t)?);
                Ok(format!("?{}", params.len()))
            })
            .collect::<Result<Vec<_>, AppError>>()?;
        conditions.push(format!(
            "EXISTS (SELECT 1 FROM media_items m2 JOIN editions e2 ON m2.edition_id = e2.id
                     WHERE e2.work_id = w.id AND m2.media_type IN ({}))",
            placeholders.join(", ")
        ));
    }

    if let Some(query) = query.map(str::trim).filter(|q| !q.is_empty()) {
        let escaped = query.replace('"', "\"\"");
        let tokens = haven_common::tokenizer::tokenize(&escaped);
        if tokens.is_empty() {
            params.push(format!(
                "%{}%",
                query.replace('%', "\\%").replace('_', "\\_")
            ));
            conditions.push(format!(
                "(w.canonical_title LIKE ?{} ESCAPE '\\' OR w.original_title LIKE ?{} ESCAPE '\\')",
                params.len(),
                params.len()
            ));
        } else {
            let fts_query = tokens
                .into_iter()
                .map(|t| format!("\"{}\"", t.replace('"', "\"\"")))
                .collect::<Vec<_>>()
                .join(" AND ");
            params.push(fts_query);
            conditions.push(format!(
                "w.rowid IN (SELECT rowid FROM work_fts WHERE work_fts MATCH ?{})",
                params.len()
            ));
        }
    }

    let where_sql = if conditions.is_empty() {
        String::new()
    } else {
        format!("WHERE {}", conditions.join(" AND "))
    };
    Ok((where_sql, params))
}

#[async_trait]
impl WorkRepository for SqliteWorkRepository {
    async fn get(&self, id: WorkId) -> Result<Option<Work>, AppError> {
        let conn = self.db.lock();
        let mut stmt = conn
            .prepare(&format!("SELECT {SELECT_COLUMNS} FROM works WHERE id = ?1"))
            .map_err(map_db_error("查询作品失败"))?;
        let mut rows = stmt
            .query_map(rusqlite::params![id.to_string()], row_to_work)
            .map_err(map_db_error("查询作品失败"))?;
        rows.next()
            .transpose()
            .map_err(map_db_error("查询作品失败"))
    }

    async fn save(&self, work: &Work) -> Result<(), AppError> {
        let conn = self.db.lock();
        save_on_conn(&conn, work)
    }

    async fn list(&self, limit: u32, offset: u32) -> Result<Vec<Work>, AppError> {
        self.list_sorted(WorkOrder::Title, limit, offset).await
    }

    async fn list_sorted(
        &self,
        order: WorkOrder,
        limit: u32,
        offset: u32,
    ) -> Result<Vec<Work>, AppError> {
        self.list_filtered(order, None, None, None, limit, offset)
            .await
    }

    async fn list_filtered(
        &self,
        order: WorkOrder,
        category: Option<haven_domain::enums::ContentCategory>,
        media_types: Option<&[haven_domain::enums::MediaType]>,
        query: Option<&str>,
        limit: u32,
        offset: u32,
    ) -> Result<Vec<Work>, AppError> {
        let has_query = query.map(|q| !q.trim().is_empty()).unwrap_or(false);
        let (where_sql, params) = build_filter(category, media_types, query)?;
        // JOIN 只在 LastActive 排序需要时引入，且用聚合子查询按 work 去重：
        // progress 唯一约束在 media_item_id 而非 work_id，一个 Work 下多个
        // MediaItem 各有进度时，裸 LEFT JOIN 会让同一作品重复出现、与
        // count_filtered（无 JOIN）的 total 错位（审查 P1-3）。
        let (join_sql, order_by) = if has_query {
            // FTS 排名：bm25 越小越相关，辅以 id 保证稳定排序
            (
                "JOIN work_fts ON work_fts.rowid = w.rowid",
                "bm25(work_fts, 10.0, 5.0, 1.0) ASC, w.id ASC",
            )
        } else {
            match order {
                WorkOrder::RecentlyAdded => ("", "w.created_at DESC, w.id DESC"),
                WorkOrder::Title => (
                    "",
                    "w.sort_title IS NULL, w.sort_title, w.canonical_title, w.id",
                ),
                WorkOrder::LastActive => (
                    "LEFT JOIN (SELECT work_id, MAX(last_active_at) AS last_active_at
                                FROM progress GROUP BY work_id) p ON p.work_id = w.id",
                    "COALESCE(p.last_active_at, 0) DESC, w.created_at DESC",
                ),
                WorkOrder::ReleaseDate => (
                    "",
                    "w.release_year IS NULL, w.release_year DESC, w.created_at DESC",
                ),
            }
        };
        // FTS 键集分页走 list_filtered_fts（(rank,id) 游标）；本方法保持 offset 分页兼容。
        let sql = format!(
            "SELECT w.id, w.canonical_title, w.original_title, w.sort_title, w.description,
                    w.work_type, w.release_year, w.language, w.director, w.actor, w.status,
                    w.rating_value, w.rating_scale,
                    w.poster, w.cover, w.backdrop, w.thumbnail,
                    w.created_at, w.updated_at
             FROM works w
             {join_sql}
             {where_sql}
             ORDER BY {order_by} LIMIT ?{} OFFSET ?{}",
            params.len() + 1,
            params.len() + 2,
        );
        let mut all_params = params;
        all_params.push(limit.to_string());
        all_params.push(offset.to_string());
        let conn = self.db.lock();
        let mut stmt = conn
            .prepare(&sql)
            .map_err(map_db_error("查询作品列表失败"))?;
        let rows = stmt
            .query_map(rusqlite::params_from_iter(all_params.iter()), row_to_work)
            .map_err(map_db_error("查询作品列表失败"))?;
        rows.collect::<rusqlite::Result<Vec<_>>>()
            .map_err(map_db_error("查询作品列表失败"))
    }

    async fn count_filtered(
        &self,
        category: Option<haven_domain::enums::ContentCategory>,
        media_types: Option<&[haven_domain::enums::MediaType]>,
        query: Option<&str>,
    ) -> Result<u64, AppError> {
        let (where_sql, params) = build_filter(category, media_types, query)?;
        let sql = format!("SELECT COUNT(*) FROM works w {where_sql}");
        let conn = self.db.lock();
        let count: i64 = conn
            .query_row(&sql, rusqlite::params_from_iter(params.iter()), |r| {
                r.get(0)
            })
            .map_err(map_db_error("统计作品失败"))?;
        Ok(count.max(0) as u64)
    }

    /// FTS 键集分页：`ORDER BY bm25 ASC, id ASC`，游标条件
    /// `rank > ?rank OR (rank = ?rank AND id > ?id)`（避免 bm25 并列时漏页/重页）。
    async fn list_filtered_fts(
        &self,
        category: Option<haven_domain::enums::ContentCategory>,
        media_types: Option<&[haven_domain::enums::MediaType]>,
        query: &str,
        after_rank: Option<f64>,
        after_id: Option<WorkId>,
        limit: u32,
    ) -> Result<Vec<(f64, Work)>, AppError> {
        let (mut where_sql, filter_params) = build_filter(category, media_types, Some(query))?;
        // 统一使用类型化参数：bm25 是 REAL，键集比较必须绑定 REAL，
        // 否则 SQLite 会把 REAL 与 TEXT 混型比较导致相等判断失败。
        let mut params: Vec<rusqlite::types::Value> = filter_params
            .iter()
            .map(|p| rusqlite::types::Value::Text(p.clone()))
            .collect();
        if let (Some(rank), Some(id)) = (after_rank, after_id) {
            let keyset = format!(
                " AND (bm25(work_fts, 10.0, 5.0, 1.0) > ?{r1}
                      OR (bm25(work_fts, 10.0, 5.0, 1.0) = ?{r2} AND w.id > ?{r3}))",
                r1 = params.len() + 1,
                r2 = params.len() + 2,
                r3 = params.len() + 3
            );
            where_sql.push_str(&keyset);
            params.push(rusqlite::types::Value::Real(rank));
            params.push(rusqlite::types::Value::Real(rank));
            params.push(rusqlite::types::Value::Text(id.to_string()));
        }
        let sql = format!(
            "SELECT w.id, w.canonical_title, w.original_title, w.sort_title, w.description,
                    w.work_type, w.release_year, w.language, w.director, w.actor, w.status,
                    w.rating_value, w.rating_scale,
                    w.poster, w.cover, w.backdrop, w.thumbnail,
                    w.created_at, w.updated_at,
                    bm25(work_fts, 10.0, 5.0, 1.0) AS fts_rank
             FROM works w
             JOIN work_fts ON work_fts.rowid = w.rowid
             {where_sql}
             ORDER BY bm25(work_fts, 10.0, 5.0, 1.0) ASC, w.id ASC
             LIMIT ?{}",
            params.len() + 1
        );
        params.push(rusqlite::types::Value::Integer(i64::from(limit)));
        let conn = self.db.lock();
        let mut stmt = conn
            .prepare(&sql)
            .map_err(map_db_error("查询作品列表失败"))?;
        let rows = stmt
            .query_map(rusqlite::params_from_iter(params.iter()), |row| {
                let rank: f64 = row.get("fts_rank")?;
                let work = row_to_work(row)?;
                Ok((rank, work))
            })
            .map_err(map_db_error("查询作品列表失败"))?;
        rows.collect::<rusqlite::Result<Vec<_>>>()
            .map_err(map_db_error("查询作品列表失败"))
    }

    async fn delete(&self, id: WorkId) -> Result<bool, AppError> {
        let conn = self.db.lock();
        let affected = conn
            .execute(
                "DELETE FROM works WHERE id = ?1",
                rusqlite::params![id.to_string()],
            )
            .map_err(map_db_error(
                "删除作品失败：可能被版本/用户状态引用（RESTRICT）",
            ))?;
        Ok(affected > 0)
    }

    async fn id_for_source_ref(
        &self,
        provider: &str,
        external_id: &str,
    ) -> Result<Option<WorkId>, AppError> {
        let conn = self.db.lock();
        let raw: Option<String> = conn
            .query_row(
                "SELECT work_id FROM work_source_refs WHERE provider = ?1 AND external_id = ?2",
                rusqlite::params![provider, external_id],
                |row| row.get(0),
            )
            .optional()
            .map_err(map_db_error("查询来源作品引用失败"))?;
        drop(conn);
        raw.map(|value| {
            id_from_row::<WorkId>(value).map_err(map_db_error("来源引用 work_id 解析失败"))
        })
        .transpose()
    }
    async fn has_any_source_ref(&self, id: WorkId) -> Result<bool, AppError> {
        let conn = self.db.lock();
        let count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM work_source_refs WHERE work_id = ?1",
                rusqlite::params![id.to_string()],
                |row| row.get(0),
            )
            .map_err(map_db_error("查询来源作品引用失败"))?;
        Ok(count > 0)
    }

    async fn save_source_ref(
        &self,
        provider: &str,
        external_id: &str,
        work_id: WorkId,
    ) -> Result<(), AppError> {
        let conn = self.db.lock();
        save_source_ref_on_conn(&conn, provider, external_id, work_id)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use haven_domain::entities::{ArtworkKind, ArtworkRef, ArtworkSet};
    use haven_domain::enums::{ContentCategory, MediaType, WorkStatus, WorkType};
    use haven_domain::ids::{EditionId, MediaItemId};

    fn sample_work() -> Work {
        Work {
            id: WorkId::new(),
            canonical_title: "三体".into(),
            original_title: Some("The Three-Body Problem".into()),
            sort_title: Some("三体".into()),
            description: Some("地球文明与三体文明的接触。".into()),
            work_type: WorkType::Fiction,
            release_year: Some(2008),
            language: Some("zh".into()),
            director: Some("刘慈欣".into()),
            actor: Some("演员甲".into()),
            status: WorkStatus::Completed,
            rating_value: None,
            rating_scale: None,
            artwork: ArtworkSet {
                poster: Some(ArtworkRef {
                    kind: ArtworkKind::Poster,
                    uri: "haven://artwork/abc".into(),
                    provider: Some("tmdb".into()),
                }),
                ..Default::default()
            },
            created_at: haven_common::UtcMillis(1_000),
            updated_at: haven_common::UtcMillis(2_000),
        }
    }

    /// 插入 work + edition + media_item（供过滤测试）。
    fn seed_with_media(db: &Db, work: &Work, media_type: MediaType) -> (EditionId, MediaItemId) {
        let edition_id = EditionId::new();
        let media_item_id = MediaItemId::new();
        let now = haven_common::UtcMillis::now().0;
        let conn = db.lock();
        let search_title = haven_common::tokenizer::tokenize(&work.canonical_title).join(" ");
        conn.execute(
            "INSERT INTO works (id, canonical_title, work_type, status, created_at, updated_at, search_title, search_original_title, search_body)
             VALUES (?1, ?2, 'fiction', 'completed', ?3, ?3, ?4, '', ?4)",
            rusqlite::params![work.id.to_string(), work.canonical_title, now, search_title],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO editions (id, work_id, title, edition_type, created_at, updated_at)
             VALUES (?1, ?2, '版本', ?3, ?4, ?4)",
            rusqlite::params![
                edition_id.to_string(),
                work.id.to_string(),
                enum_to_db_str(&media_type).unwrap(),
                now
            ],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO media_items (id, edition_id, media_type, title, category, status, created_at, updated_at)
             VALUES (?1, ?2, ?3, '条目', ?4, 'available', ?5, ?5)",
            rusqlite::params![
                media_item_id.to_string(),
                edition_id.to_string(),
                enum_to_db_str(&media_type).unwrap(),
                enum_to_db_str(&ContentCategory::from_media_type(media_type)).unwrap(),
                now
            ],
        )
        .unwrap();
        (edition_id, media_item_id)
    }

    #[tokio::test]
    async fn save_get_roundtrip_preserves_artwork() {
        let db = Arc::new(Db::open_in_memory().unwrap());
        let repo = SqliteWorkRepository::new(db);
        let work = sample_work();

        repo.save(&work).await.unwrap();
        let read = repo.get(work.id).await.unwrap().expect("存在");
        assert_eq!(read, work);
        assert_eq!(
            read.artwork.poster.expect("poster").uri,
            "haven://artwork/abc"
        );
    }

    #[tokio::test]
    async fn save_get_roundtrip_preserves_rating() {
        let db = Arc::new(Db::open_in_memory().unwrap());
        let repo = SqliteWorkRepository::new(db);
        let mut work = sample_work();
        work.rating_value = Some(8.7);
        work.rating_scale = Some(10.0);

        repo.save(&work).await.unwrap();
        let read = repo.get(work.id).await.unwrap().expect("存在");
        assert_eq!(read.rating_value, Some(8.7));
        assert_eq!(read.rating_scale, Some(10.0));

        work.rating_value = None;
        work.rating_scale = None;
        repo.save(&work).await.unwrap();
        let read = repo.get(work.id).await.unwrap().unwrap();
        assert_eq!(read.rating_value, None, "置空应清除评分");
        assert_eq!(read.rating_scale, None);
    }

    #[tokio::test]
    async fn save_updates_instead_of_duplicate() {
        let db = Arc::new(Db::open_in_memory().unwrap());
        let repo = SqliteWorkRepository::new(db.clone());
        let mut work = sample_work();
        repo.save(&work).await.unwrap();

        work.canonical_title = "三体（修订版）".into();
        work.updated_at = haven_common::UtcMillis(9_999);
        repo.save(&work).await.unwrap();

        let count: i64 = db
            .lock()
            .query_row("SELECT COUNT(*) FROM works", [], |r| r.get(0))
            .unwrap();
        assert_eq!(count, 1, "同 id 应覆盖");
        let read = repo.get(work.id).await.unwrap().unwrap();
        assert_eq!(read.canonical_title, "三体（修订版）");
    }

    #[tokio::test]
    async fn one_work_can_keep_multiple_source_refs_and_conflicts_are_explicit() {
        let db = Arc::new(Db::open_in_memory().unwrap());
        let repo = SqliteWorkRepository::new(db.clone());
        let first = sample_work();
        let mut second = sample_work();
        second.id = WorkId::new();
        repo.save(&first).await.unwrap();
        repo.save(&second).await.unwrap();

        repo.save_source_ref("mangadex", "manga-1", first.id)
            .await
            .unwrap();
        repo.save_source_ref("reader-ws", "book-1", first.id)
            .await
            .unwrap();

        let count: i64 = db
            .lock()
            .query_row(
                "SELECT COUNT(*) FROM work_source_refs WHERE work_id = ?1",
                rusqlite::params![first.id.to_string()],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(count, 2);

        let error = repo
            .save_source_ref("mangadex", "manga-1", second.id)
            .await
            .expect_err("同一来源身份绑定其他 Work 必须报告冲突");
        assert_eq!(error.code().as_str(), "SOURCE_REF_CONFLICT");
    }

    #[tokio::test]
    async fn list_is_paginated() {
        let db = Arc::new(Db::open_in_memory().unwrap());
        let repo = SqliteWorkRepository::new(db);
        for i in 0..5 {
            let mut work = sample_work();
            work.id = WorkId::new();
            work.canonical_title = format!("作品 {i}");
            work.sort_title = Some(format!("作品 {i}"));
            repo.save(&work).await.unwrap();
        }
        let page = repo.list(2, 1).await.unwrap();
        assert_eq!(page.len(), 2);
    }

    #[tokio::test]
    async fn list_filtered_by_category_and_query() {
        let db = Arc::new(Db::open_in_memory().unwrap());
        let repo = SqliteWorkRepository::new(db.clone());

        let mut movie_work = sample_work();
        movie_work.id = WorkId::new();
        movie_work.canonical_title = "沙丘".into();
        seed_with_media(&db, &movie_work, MediaType::Movie);

        let mut book_work = sample_work();
        book_work.id = WorkId::new();
        book_work.canonical_title = "三体".into();
        seed_with_media(&db, &book_work, MediaType::Book);

        // 按 category 过滤
        let movies = repo
            .list_filtered(
                WorkOrder::Title,
                Some(ContentCategory::Video),
                None,
                None,
                10,
                0,
            )
            .await
            .unwrap();
        assert_eq!(movies.len(), 1, "只有沙丘是 video");
        assert_eq!(movies[0].canonical_title, "沙丘");

        let total = repo
            .count_filtered(Some(ContentCategory::Book), None, None)
            .await
            .unwrap();
        assert_eq!(total, 1, "只有三体是 book");

        // 按标题查询
        let searched = repo
            .list_filtered(WorkOrder::Title, None, None, Some("沙丘"), 10, 0)
            .await
            .unwrap();
        assert_eq!(searched.len(), 1);
        assert_eq!(searched[0].canonical_title, "沙丘");
    }

    #[tokio::test]
    async fn fts_keyset_pagination_is_stable_and_non_overlapping() {
        let db = Arc::new(Db::open_in_memory().unwrap());
        let repo = SqliteWorkRepository::new(db.clone());

        for title in [
            "三体地球往事",
            "三体黑暗森林",
            "三体死神永生",
            "三体球状闪电",
            "三体超新星纪元",
        ] {
            let mut work = sample_work();
            work.id = WorkId::new();
            work.canonical_title = title.into();
            seed_with_media(&db, &work, MediaType::Book);
        }

        let total = repo.count_filtered(None, None, Some("三体")).await.unwrap();
        assert_eq!(total, 5, "五本书都应命中三体查询");

        let mut seen = std::collections::HashSet::new();
        let mut cursor: Option<(f64, WorkId)> = None;
        let mut pages = 0;
        loop {
            let (after_rank, after_id) = match cursor {
                Some((rank, id)) => (Some(rank), Some(id)),
                None => (None, None),
            };
            let page = repo
                .list_filtered_fts(None, None, "三体", after_rank, after_id, 2)
                .await
                .unwrap();
            assert!(!page.is_empty(), "有剩余数据时必须返回分页");
            for (_, work) in &page {
                assert!(seen.insert(work.id), "键集游标分页不得跨页重复同一作品");
            }
            pages += 1;
            if page.len() < 2 {
                break;
            }
            let last = page.last().expect("page 非空");
            cursor = Some((last.0, last.1.id));
        }
        assert_eq!(seen.len(), 5, "两页一页地翻完必须覆盖全部 5 条");
        assert!(pages >= 3, "limit=2 至少需要 3 页，实际 {pages}");
    }

    #[tokio::test]
    async fn delete_returns_false_for_missing() {
        let db = Arc::new(Db::open_in_memory().unwrap());
        let repo = SqliteWorkRepository::new(db);
        assert!(!repo.delete(WorkId::new()).await.unwrap());
    }

    #[tokio::test]
    async fn delete_blocked_when_editions_exist() {
        let db = Arc::new(Db::open_in_memory().unwrap());
        let repo = SqliteWorkRepository::new(db.clone());
        let work = sample_work();
        seed_with_media(&db, &work, MediaType::Movie);

        let err = repo.delete(work.id).await;
        assert!(
            err.is_err(),
            "存在版本时必须拒绝删除（RESTRICT 防数据丢失）"
        );
    }
}
