//! 报刊（期刊）层级的 SQLite Repository。
//!
//! 表结构只表达 期刊 → 卷 → 期 → 文章 的逐级归属；文章通过 `media_item_id`
//! 复用既有 MediaItem，因此本 Repository 不重复实现阅读、进度或资源语义。
//! 不保存 URL、Cookie、grant、请求头或本地路径。

use std::sync::Arc;

use async_trait::async_trait;
use rusqlite::OptionalExtension;

use haven_common::{AppError, ErrorKind, UtcMillis};
use haven_domain::contracts::PeriodicalRepository;
use haven_domain::ids::{
    MediaItemId, PeriodicalArticleId, PeriodicalId, PeriodicalIssueId, PeriodicalVolumeId, WorkId,
};
use haven_domain::periodical::{
    Doi, Issn, PageRange, Periodical, PeriodicalArticle, PeriodicalArticleAvailability,
    PeriodicalArticleSourceIdentity, PeriodicalIssue, PeriodicalPlacement, PeriodicalVolume,
};

use crate::db::Db;
use crate::db::repos::map_db_error;

pub struct SqlitePeriodicalRepository {
    db: Arc<Db>,
}

impl SqlitePeriodicalRepository {
    pub fn new(db: Arc<Db>) -> Self {
        Self { db }
    }
}

const ARTICLE_PLACEMENT_SELECT: &str = "
    a.id                AS article_id,
    a.issue_id          AS article_issue_id,
    a.media_item_id     AS article_media_item_id,
    a.ordinal           AS article_ordinal,
    a.title             AS article_title,
    a.doi               AS article_doi,
    a.page_range_start  AS article_page_range_start,
    a.page_range_end    AS article_page_range_end,
    a.source_key        AS article_source_key,
    a.remote_article_id AS article_remote_article_id,
    a.provider_content_availability AS article_provider_content_availability,
    a.created_at        AS article_created_at,
    a.updated_at        AS article_updated_at,
    i.id                AS issue_id,
    i.volume_id         AS issue_volume_id,
    i.label             AS issue_label,
    i.number            AS issue_number,
    i.publication_date  AS issue_publication_date,
    i.ordinal           AS issue_ordinal,
    i.created_at        AS issue_created_at,
    i.updated_at        AS issue_updated_at,
    v.id                AS volume_id,
    v.periodical_id     AS volume_periodical_id,
    v.label             AS volume_label,
    v.number            AS volume_number,
    v.year              AS volume_year,
    v.ordinal           AS volume_ordinal,
    v.created_at        AS volume_created_at,
    v.updated_at        AS volume_updated_at,
    p.id                AS periodical_id,
    p.work_id           AS periodical_work_id,
    p.title             AS periodical_title,
    p.issn_print        AS periodical_issn_print,
    p.issn_electronic   AS periodical_issn_electronic,
    p.publisher         AS periodical_publisher,
    p.created_at        AS periodical_created_at,
    p.updated_at        AS periodical_updated_at
";

const PLACEMENT_FROM: &str = "
    FROM periodical_articles a
    JOIN periodical_issues i ON i.id = a.issue_id
    JOIN periodical_volumes v ON v.id = i.volume_id
    JOIN periodicals p ON p.id = v.periodical_id
";

fn invalid_row(field: &'static str) -> rusqlite::Error {
    rusqlite::Error::FromSqlConversionFailure(
        0,
        rusqlite::types::Type::Text,
        Box::new(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            format!("无法解析报刊层级字段 {field}"),
        )),
    )
}

fn invalid_periodical(message: impl Into<String>) -> AppError {
    AppError::new(
        "INVALID_PERIODICAL_IDENTITY",
        ErrorKind::Validation,
        message,
        false,
    )
}

/// 期刊层级的唯一键竞争是可收敛的导入冲突；其他 SQLite 约束（CHECK、FK、
/// trigger、磁盘/数据库故障）仍保留为普通 DATABASE_ERROR，不给 Application
/// 一个会重复写入的假信号。
fn map_periodical_write_error(context: &'static str, error: rusqlite::Error) -> AppError {
    if error.sqlite_extended_error_code() == Some(rusqlite::ffi::SQLITE_CONSTRAINT_UNIQUE) {
        return AppError::new(
            "PERIODICAL_IMPORT_CONFLICT",
            ErrorKind::Conflict,
            "报刊层级唯一身份发生并发冲突，请重新读取",
            false,
        )
        .with_source(error);
    }
    map_db_error(context)(error)
}

fn parse_id<T: std::str::FromStr>(value: String, field: &'static str) -> rusqlite::Result<T> {
    value.parse().map_err(|_| invalid_row(field))
}

fn parse_ordinal(value: i64, field: &'static str) -> rusqlite::Result<u32> {
    u32::try_from(value).map_err(|_| invalid_row(field))
}

fn parse_issn(value: Option<String>, field: &'static str) -> rusqlite::Result<Option<Issn>> {
    value
        .map(|raw| Issn::parse(&raw).ok_or_else(|| invalid_row(field)))
        .transpose()
}

fn parse_doi(value: Option<String>, field: &'static str) -> rusqlite::Result<Option<Doi>> {
    value
        .map(|raw| Doi::parse(&raw).ok_or_else(|| invalid_row(field)))
        .transpose()
}

fn parse_page_range(
    start: Option<String>,
    end: Option<String>,
) -> rusqlite::Result<Option<PageRange>> {
    match start {
        None => Ok(None),
        Some(start) => PageRange::new(&start, end.as_deref())
            .map(Some)
            .ok_or_else(|| invalid_row("page_range")),
    }
}

/// 闭合枚举解析：库里出现未定义的正文观察取值时是行级错误，而不是猜一个状态。
fn parse_availability(
    value: String,
    field: &'static str,
) -> rusqlite::Result<PeriodicalArticleAvailability> {
    PeriodicalArticleAvailability::parse(&value).ok_or_else(|| invalid_row(field))
}

/// 逐列重建一条完整归属链。任何一层解析失败都返回行级错误，不返回部分归属。
fn placement_from_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<PeriodicalPlacement> {
    let periodical = Periodical {
        id: parse_id(row.get("periodical_id")?, "periodical_id")?,
        work_id: parse_id(row.get("periodical_work_id")?, "periodical_work_id")?,
        title: row.get("periodical_title")?,
        issn_print: parse_issn(row.get("periodical_issn_print")?, "periodical_issn_print")?,
        issn_electronic: parse_issn(
            row.get("periodical_issn_electronic")?,
            "periodical_issn_electronic",
        )?,
        publisher: row.get("periodical_publisher")?,
        created_at: UtcMillis(row.get("periodical_created_at")?),
        updated_at: UtcMillis(row.get("periodical_updated_at")?),
    };
    let volume = PeriodicalVolume {
        id: parse_id(row.get("volume_id")?, "volume_id")?,
        periodical_id: parse_id(row.get("volume_periodical_id")?, "volume_periodical_id")?,
        label: row.get("volume_label")?,
        number: row.get("volume_number")?,
        year: row.get("volume_year")?,
        ordinal: parse_ordinal(row.get("volume_ordinal")?, "volume_ordinal")?,
        created_at: UtcMillis(row.get("volume_created_at")?),
        updated_at: UtcMillis(row.get("volume_updated_at")?),
    };
    let issue = PeriodicalIssue {
        id: parse_id(row.get("issue_id")?, "issue_id")?,
        volume_id: parse_id(row.get("issue_volume_id")?, "issue_volume_id")?,
        label: row.get("issue_label")?,
        number: row.get("issue_number")?,
        publication_date: row.get("issue_publication_date")?,
        ordinal: parse_ordinal(row.get("issue_ordinal")?, "issue_ordinal")?,
        created_at: UtcMillis(row.get("issue_created_at")?),
        updated_at: UtcMillis(row.get("issue_updated_at")?),
    };
    let article = PeriodicalArticle {
        id: parse_id(row.get("article_id")?, "article_id")?,
        issue_id: parse_id(row.get("article_issue_id")?, "article_issue_id")?,
        media_item_id: parse_id(row.get("article_media_item_id")?, "article_media_item_id")?,
        ordinal: row
            .get::<_, Option<i64>>("article_ordinal")?
            .map(|value| parse_ordinal(value, "article_ordinal"))
            .transpose()?,
        title: row.get("article_title")?,
        doi: parse_doi(row.get("article_doi")?, "article_doi")?,
        page_range: parse_page_range(
            row.get("article_page_range_start")?,
            row.get("article_page_range_end")?,
        )?,
        source: PeriodicalArticleSourceIdentity {
            source_key: row.get("article_source_key")?,
            remote_article_id: row.get("article_remote_article_id")?,
        },
        provider_content_availability: parse_availability(
            row.get("article_provider_content_availability")?,
            "article_provider_content_availability",
        )?,
        created_at: UtcMillis(row.get("article_created_at")?),
        updated_at: UtcMillis(row.get("article_updated_at")?),
    };
    let placement = PeriodicalPlacement {
        periodical,
        volume,
        issue,
        article,
    };
    placement.validate().map_err(|_| invalid_row("placement"))?;
    Ok(placement)
}

fn periodical_from_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<Periodical> {
    let periodical = Periodical {
        id: parse_id(row.get("id")?, "id")?,
        work_id: parse_id(row.get("work_id")?, "work_id")?,
        title: row.get("title")?,
        issn_print: parse_issn(row.get("issn_print")?, "issn_print")?,
        issn_electronic: parse_issn(row.get("issn_electronic")?, "issn_electronic")?,
        publisher: row.get("publisher")?,
        created_at: UtcMillis(row.get("created_at")?),
        updated_at: UtcMillis(row.get("updated_at")?),
    };
    periodical
        .validate()
        .map_err(|_| invalid_row("periodical"))?;
    Ok(periodical)
}

const PERIODICAL_COLUMNS: &str =
    "id, work_id, title, issn_print, issn_electronic, publisher, created_at, updated_at";

const VOLUME_COLUMNS: &str =
    "id, periodical_id, label, number, year, ordinal, created_at, updated_at";

const ISSUE_COLUMNS: &str =
    "id, volume_id, label, number, publication_date, ordinal, created_at, updated_at";

const ARTICLE_COLUMNS: &str = "id, issue_id, media_item_id, ordinal, title, doi,
    page_range_start, page_range_end, source_key, remote_article_id,
    provider_content_availability, created_at, updated_at";

fn volume_from_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<PeriodicalVolume> {
    let volume = PeriodicalVolume {
        id: parse_id(row.get("id")?, "id")?,
        periodical_id: parse_id(row.get("periodical_id")?, "periodical_id")?,
        label: row.get("label")?,
        number: row.get("number")?,
        year: row.get("year")?,
        ordinal: parse_ordinal(row.get("ordinal")?, "ordinal")?,
        created_at: UtcMillis(row.get("created_at")?),
        updated_at: UtcMillis(row.get("updated_at")?),
    };
    volume.validate().map_err(|_| invalid_row("volume"))?;
    Ok(volume)
}

fn issue_from_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<PeriodicalIssue> {
    let issue = PeriodicalIssue {
        id: parse_id(row.get("id")?, "id")?,
        volume_id: parse_id(row.get("volume_id")?, "volume_id")?,
        label: row.get("label")?,
        number: row.get("number")?,
        publication_date: row.get("publication_date")?,
        ordinal: parse_ordinal(row.get("ordinal")?, "ordinal")?,
        created_at: UtcMillis(row.get("created_at")?),
        updated_at: UtcMillis(row.get("updated_at")?),
    };
    issue.validate().map_err(|_| invalid_row("issue"))?;
    Ok(issue)
}

fn article_from_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<PeriodicalArticle> {
    let article = PeriodicalArticle {
        id: parse_id(row.get("id")?, "id")?,
        issue_id: parse_id(row.get("issue_id")?, "issue_id")?,
        media_item_id: parse_id(row.get("media_item_id")?, "media_item_id")?,
        ordinal: row
            .get::<_, Option<i64>>("ordinal")?
            .map(|value| parse_ordinal(value, "ordinal"))
            .transpose()?,
        title: row.get("title")?,
        doi: parse_doi(row.get("doi")?, "doi")?,
        page_range: parse_page_range(row.get("page_range_start")?, row.get("page_range_end")?)?,
        source: PeriodicalArticleSourceIdentity {
            source_key: row.get("source_key")?,
            remote_article_id: row.get("remote_article_id")?,
        },
        provider_content_availability: parse_availability(
            row.get("provider_content_availability")?,
            "provider_content_availability",
        )?,
        created_at: UtcMillis(row.get("created_at")?),
        updated_at: UtcMillis(row.get("updated_at")?),
    };
    article.validate().map_err(|_| invalid_row("article"))?;
    Ok(article)
}

/// 在指定连接上保存期刊（普通连接或 UoW 事务连接复用）。
pub(crate) fn save_on_conn(
    conn: &rusqlite::Connection,
    periodical: &Periodical,
) -> Result<(), AppError> {
    periodical.validate()?;
    conn.execute(
        "INSERT INTO periodicals
            (id, work_id, title, issn_print, issn_electronic, publisher, created_at, updated_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)
         ON CONFLICT(id) DO UPDATE SET
             work_id = excluded.work_id,
             title = excluded.title,
             issn_print = excluded.issn_print,
             issn_electronic = excluded.issn_electronic,
             publisher = excluded.publisher,
             updated_at = excluded.updated_at",
        rusqlite::params![
            periodical.id.to_string(),
            periodical.work_id.to_string(),
            periodical.title,
            periodical.issn_print.as_ref().map(Issn::as_str),
            periodical.issn_electronic.as_ref().map(Issn::as_str),
            periodical.publisher,
            periodical.created_at.0,
            periodical.updated_at.0,
        ],
    )
    .map_err(|error| map_periodical_write_error("保存期刊失败", error))?;
    Ok(())
}

/// 在指定连接上保存卷。身份键由领域层计算，Repository 不自行拼接。
pub(crate) fn save_volume_on_conn(
    conn: &rusqlite::Connection,
    volume: &PeriodicalVolume,
) -> Result<(), AppError> {
    volume.validate()?;
    conn.execute(
        "INSERT INTO periodical_volumes
            (id, periodical_id, label, number, year, ordinal, identity_key, created_at, updated_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)
         ON CONFLICT(id) DO UPDATE SET
             periodical_id = excluded.periodical_id,
             label = excluded.label,
             number = excluded.number,
             year = excluded.year,
             ordinal = excluded.ordinal,
             identity_key = excluded.identity_key,
             updated_at = excluded.updated_at",
        rusqlite::params![
            volume.id.to_string(),
            volume.periodical_id.to_string(),
            volume.label,
            volume.number,
            volume.year,
            i64::from(volume.ordinal),
            volume.identity().key(),
            volume.created_at.0,
            volume.updated_at.0,
        ],
    )
    .map_err(|error| map_periodical_write_error("保存期刊卷失败", error))?;
    Ok(())
}

pub(crate) fn save_issue_on_conn(
    conn: &rusqlite::Connection,
    issue: &PeriodicalIssue,
) -> Result<(), AppError> {
    issue.validate()?;
    conn.execute(
        "INSERT INTO periodical_issues
            (id, volume_id, label, number, publication_date, ordinal, identity_key, created_at, updated_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)
         ON CONFLICT(id) DO UPDATE SET
             volume_id = excluded.volume_id,
             label = excluded.label,
             number = excluded.number,
             publication_date = excluded.publication_date,
             ordinal = excluded.ordinal,
             identity_key = excluded.identity_key,
             updated_at = excluded.updated_at",
        rusqlite::params![
            issue.id.to_string(),
            issue.volume_id.to_string(),
            issue.label,
            issue.number,
            issue.publication_date,
            i64::from(issue.ordinal),
            issue.identity().key(),
            issue.created_at.0,
            issue.updated_at.0,
        ],
    )
    .map_err(|error| map_periodical_write_error("保存期刊期号失败", error))?;
    Ok(())
}

pub(crate) fn save_article_on_conn(
    conn: &rusqlite::Connection,
    article: &PeriodicalArticle,
) -> Result<(), AppError> {
    article.validate()?;
    validate_article_media_item_on_conn(conn, article)?;
    conn.execute(
        "INSERT INTO periodical_articles
            (id, issue_id, media_item_id, ordinal, title, doi,
             page_range_start, page_range_end, source_key, remote_article_id,
             provider_content_availability, created_at, updated_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13)
         ON CONFLICT(id) DO UPDATE SET
             issue_id = excluded.issue_id,
             media_item_id = excluded.media_item_id,
             ordinal = excluded.ordinal,
             title = excluded.title,
             doi = excluded.doi,
             page_range_start = excluded.page_range_start,
             page_range_end = excluded.page_range_end,
             source_key = excluded.source_key,
             remote_article_id = excluded.remote_article_id,
             provider_content_availability = excluded.provider_content_availability,
             updated_at = excluded.updated_at",
        rusqlite::params![
            article.id.to_string(),
            article.issue_id.to_string(),
            article.media_item_id.to_string(),
            article.ordinal.map(i64::from),
            article.title,
            article.doi.as_ref().map(Doi::as_str),
            article
                .page_range
                .as_ref()
                .map(|range| range.start.as_str()),
            article
                .page_range
                .as_ref()
                .and_then(|range| range.end.as_deref()),
            article.source.source_key,
            article.source.remote_article_id,
            article.provider_content_availability.as_str(),
            article.created_at.0,
            article.updated_at.0,
        ],
    )
    .map_err(|error| map_periodical_write_error("保存期刊文章失败", error))?;
    Ok(())
}

/// 直接保存文章时也要验证它绑定的是同一棵期刊 Work 下的 Article MediaItem。
/// 外键只能保证三条 ID 存在，不能阻止把其他媒介类型或其他 Work 的条目挂进来。
fn validate_article_media_item_on_conn(
    conn: &rusqlite::Connection,
    article: &PeriodicalArticle,
) -> Result<(), AppError> {
    let matches: i64 = conn
        .query_row(
            "SELECT COUNT(*)
             FROM media_items mi
             JOIN editions e ON e.id = mi.edition_id
             JOIN periodical_issues i ON i.id = ?2
             JOIN periodical_volumes v ON v.id = i.volume_id
             JOIN periodicals p ON p.id = v.periodical_id
             WHERE mi.id = ?1
               AND i.id = ?2
               AND mi.media_type = 'article'
               AND e.work_id = p.work_id",
            rusqlite::params![
                article.media_item_id.to_string(),
                article.issue_id.to_string()
            ],
            |row| row.get(0),
        )
        .map_err(map_db_error("校验期刊文章媒体条目归属失败"))?;
    if matches != 1 {
        return Err(invalid_periodical(
            "期刊文章必须绑定同一 Work 下的 Article MediaItem",
        ));
    }
    Ok(())
}

/// 事务/连接级幂等检查：同一来源身份已存在时必须是同一篇文章。
pub(crate) fn existing_article_id_on_conn(
    conn: &rusqlite::Connection,
    source: &PeriodicalArticleSourceIdentity,
) -> Result<Option<PeriodicalArticleId>, AppError> {
    conn.query_row(
        "SELECT id FROM periodical_articles WHERE source_key = ?1 AND remote_article_id = ?2",
        rusqlite::params![source.source_key, source.remote_article_id],
        |row| -> rusqlite::Result<String> { row.get(0) },
    )
    .optional()
    .map_err(map_db_error("查询期刊文章来源身份失败"))?
    .map(|raw| parse_id(raw, "id").map_err(|_| invalid_periodical("期刊文章 ID 非法")))
    .transpose()
}

/// 事务/连接级幂等检查：一个 MediaItem 只能被一条文章行绑定。
///
/// 唯一索引是最终约束，这里在写入前显式检查，使它表现为可解释的冲突而不是
/// 依赖 SQLite 约束错误的文本匹配。
pub(crate) fn existing_article_id_for_media_item_on_conn(
    conn: &rusqlite::Connection,
    media_item_id: MediaItemId,
) -> Result<Option<PeriodicalArticleId>, AppError> {
    conn.query_row(
        "SELECT id FROM periodical_articles WHERE media_item_id = ?1",
        rusqlite::params![media_item_id.to_string()],
        |row| -> rusqlite::Result<String> { row.get(0) },
    )
    .optional()
    .map_err(map_db_error("查询期刊文章媒体条目失败"))?
    .map(|raw| parse_id(raw, "id").map_err(|_| invalid_periodical("期刊文章 ID 非法")))
    .transpose()
}

fn load_by_id(db: &Db, id: PeriodicalId) -> Result<Option<Periodical>, AppError> {
    let conn = db.lock();
    conn.query_row(
        &format!("SELECT {PERIODICAL_COLUMNS} FROM periodicals WHERE id = ?1"),
        rusqlite::params![id.to_string()],
        periodical_from_row,
    )
    .optional()
    .map_err(map_db_error("查询期刊失败"))
}

#[async_trait]
impl PeriodicalRepository for SqlitePeriodicalRepository {
    async fn get(&self, id: PeriodicalId) -> Result<Option<Periodical>, AppError> {
        load_by_id(&self.db, id)
    }

    async fn find_by_work(&self, work_id: WorkId) -> Result<Option<Periodical>, AppError> {
        let conn = self.db.lock();
        conn.query_row(
            &format!("SELECT {PERIODICAL_COLUMNS} FROM periodicals WHERE work_id = ?1"),
            rusqlite::params![work_id.to_string()],
            periodical_from_row,
        )
        .optional()
        .map_err(map_db_error("按作品查询期刊失败"))
    }

    async fn find_by_issn(&self, issn: &Issn) -> Result<Option<Periodical>, AppError> {
        let conn = self.db.lock();
        conn.query_row(
            &format!(
                "SELECT {PERIODICAL_COLUMNS} FROM periodicals
                 WHERE issn_print = ?1 OR issn_electronic = ?1
                 ORDER BY created_at, id LIMIT 1"
            ),
            rusqlite::params![issn.as_str()],
            periodical_from_row,
        )
        .optional()
        .map_err(map_db_error("按 ISSN 查询期刊失败"))
    }

    async fn list_volumes(
        &self,
        periodical_id: PeriodicalId,
    ) -> Result<Vec<PeriodicalVolume>, AppError> {
        let conn = self.db.lock();
        let mut stmt = conn
            .prepare(&format!(
                "SELECT {VOLUME_COLUMNS} FROM periodical_volumes
                 WHERE periodical_id = ?1 ORDER BY ordinal, id"
            ))
            .map_err(map_db_error("查询期刊卷失败"))?;
        let rows = stmt
            .query_map(
                rusqlite::params![periodical_id.to_string()],
                volume_from_row,
            )
            .map_err(map_db_error("查询期刊卷失败"))?;
        rows.collect::<rusqlite::Result<Vec<_>>>()
            .map_err(map_db_error("解析期刊卷失败"))
    }

    async fn list_issues(
        &self,
        volume_id: PeriodicalVolumeId,
    ) -> Result<Vec<PeriodicalIssue>, AppError> {
        let conn = self.db.lock();
        let mut stmt = conn
            .prepare(&format!(
                "SELECT {ISSUE_COLUMNS} FROM periodical_issues
                 WHERE volume_id = ?1 ORDER BY ordinal, id"
            ))
            .map_err(map_db_error("查询期刊期号失败"))?;
        let rows = stmt
            .query_map(rusqlite::params![volume_id.to_string()], issue_from_row)
            .map_err(map_db_error("查询期刊期号失败"))?;
        rows.collect::<rusqlite::Result<Vec<_>>>()
            .map_err(map_db_error("解析期刊期号失败"))
    }

    async fn list_articles(
        &self,
        issue_id: PeriodicalIssueId,
    ) -> Result<Vec<PeriodicalArticle>, AppError> {
        let conn = self.db.lock();
        let mut stmt = conn
            .prepare(&format!(
                "SELECT {ARTICLE_COLUMNS} FROM periodical_articles
                 WHERE issue_id = ?1 ORDER BY ordinal IS NULL, ordinal, id"
            ))
            .map_err(map_db_error("查询期刊文章失败"))?;
        let rows = stmt
            .query_map(rusqlite::params![issue_id.to_string()], article_from_row)
            .map_err(map_db_error("查询期刊文章失败"))?;
        rows.collect::<rusqlite::Result<Vec<_>>>()
            .map_err(map_db_error("解析期刊文章失败"))
    }

    async fn find_article_by_source(
        &self,
        source_key: &str,
        remote_article_id: &str,
    ) -> Result<Option<PeriodicalPlacement>, AppError> {
        let conn = self.db.lock();
        conn.query_row(
            &format!(
                "SELECT {ARTICLE_PLACEMENT_SELECT} {PLACEMENT_FROM}
                 WHERE a.source_key = ?1 AND a.remote_article_id = ?2"
            ),
            rusqlite::params![source_key, remote_article_id],
            placement_from_row,
        )
        .optional()
        .map_err(map_db_error("按来源身份查询期刊文章失败"))
    }

    async fn save(&self, periodical: &Periodical) -> Result<(), AppError> {
        let periodical = periodical.clone();
        self.db.with_tx(|tx| save_on_conn(tx, &periodical))
    }

    async fn save_volume(&self, volume: &PeriodicalVolume) -> Result<(), AppError> {
        let volume = volume.clone();
        self.db.with_tx(|tx| save_volume_on_conn(tx, &volume))
    }

    async fn save_issue(&self, issue: &PeriodicalIssue) -> Result<(), AppError> {
        let issue = issue.clone();
        self.db.with_tx(|tx| save_issue_on_conn(tx, &issue))
    }

    async fn save_article(&self, article: &PeriodicalArticle) -> Result<(), AppError> {
        let article = article.clone();
        self.db.with_tx(|tx| save_article_on_conn(tx, &article))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use haven_domain::ids::{EditionId, MediaItemId};

    fn now() -> UtcMillis {
        UtcMillis(10_000)
    }

    fn seed_article_media_item(db: &Db) -> MediaItemId {
        let work_id = WorkId::new();
        let edition_id = EditionId::new();
        let media_item_id = MediaItemId::new();
        let conn = db.lock();
        conn.execute(
            "INSERT INTO works (id, canonical_title, work_type, status, created_at, updated_at)
             VALUES (?1, '期刊作品', 'standalone', 'completed', ?2, ?2)",
            rusqlite::params![work_id.to_string(), now().0],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO editions (id, work_id, title, edition_type, created_at, updated_at)
             VALUES (?1, ?2, '期刊版本', 'article', ?3, ?3)",
            rusqlite::params![edition_id.to_string(), work_id.to_string(), now().0],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO media_items
                (id, edition_id, media_type, title, status, created_at, updated_at)
             VALUES (?1, ?2, 'article', '文章', 'available', ?3, ?3)",
            rusqlite::params![media_item_id.to_string(), edition_id.to_string(), now().0],
        )
        .unwrap();
        media_item_id
    }

    fn placement(db: &Db) -> PeriodicalPlacement {
        let media_item_id = seed_article_media_item(db);
        let work_id: WorkId = db
            .lock()
            .query_row(
                "SELECT work_id FROM editions WHERE id = (SELECT edition_id FROM media_items WHERE id = ?1)",
                rusqlite::params![media_item_id.to_string()],
                |row| row.get::<_, String>(0),
            )
            .map(|raw| raw.parse().unwrap())
            .unwrap();
        let periodical = Periodical {
            id: PeriodicalId::new(),
            work_id,
            title: "Nature Communications".into(),
            issn_print: Some(Issn::parse("2041-1723").unwrap()),
            issn_electronic: None,
            publisher: Some("Nature Portfolio".into()),
            created_at: now(),
            updated_at: now(),
        };
        let volume = PeriodicalVolume {
            id: PeriodicalVolumeId::new(),
            periodical_id: periodical.id,
            label: None,
            number: Some(15.0),
            year: Some(2024),
            ordinal: 0,
            created_at: now(),
            updated_at: now(),
        };
        let issue = PeriodicalIssue {
            id: PeriodicalIssueId::new(),
            volume_id: volume.id,
            label: Some("3-4".into()),
            number: None,
            publication_date: Some("2024-03-15".into()),
            ordinal: 0,
            created_at: now(),
            updated_at: now(),
        };
        let article = PeriodicalArticle {
            id: PeriodicalArticleId::new(),
            issue_id: issue.id,
            media_item_id,
            ordinal: Some(12),
            title: "A periodical article".into(),
            doi: Doi::parse("10.1038/s41467-024-00001-2"),
            page_range: PageRange::new("e12345", None),
            source: PeriodicalArticleSourceIdentity::new("europepmc", "PMC1234567").unwrap(),
            provider_content_availability: PeriodicalArticleAvailability::MetadataOnly,
            created_at: now(),
            updated_at: now(),
        };
        PeriodicalPlacement {
            periodical,
            volume,
            issue,
            article,
        }
    }

    async fn save_placement(
        repo: &SqlitePeriodicalRepository,
        placement: &PeriodicalPlacement,
    ) -> Result<(), AppError> {
        repo.save(&placement.periodical).await?;
        repo.save_volume(&placement.volume).await?;
        repo.save_issue(&placement.issue).await?;
        repo.save_article(&placement.article).await
    }

    #[tokio::test]
    async fn periodical_hierarchy_roundtrips_and_supports_lookups() {
        let db = Arc::new(Db::open_in_memory().unwrap());
        let expected = placement(&db);
        let repo = SqlitePeriodicalRepository::new(db.clone());
        save_placement(&repo, &expected).await.unwrap();

        assert_eq!(
            repo.get(expected.periodical.id).await.unwrap().unwrap(),
            expected.periodical
        );
        assert_eq!(
            repo.find_by_work(expected.periodical.work_id)
                .await
                .unwrap()
                .unwrap()
                .id,
            expected.periodical.id
        );
        assert_eq!(
            repo.find_by_issn(&Issn::parse("2041-1723").unwrap())
                .await
                .unwrap()
                .unwrap()
                .id,
            expected.periodical.id
        );
        assert_eq!(
            repo.list_volumes(expected.periodical.id).await.unwrap(),
            vec![expected.volume.clone()]
        );
        assert_eq!(
            repo.list_issues(expected.volume.id).await.unwrap(),
            vec![expected.issue.clone()]
        );
        assert_eq!(
            repo.list_articles(expected.issue.id).await.unwrap(),
            vec![expected.article.clone()]
        );
        let placement = repo
            .find_article_by_source("europepmc", "PMC1234567")
            .await
            .unwrap()
            .unwrap();
        assert_eq!(placement, expected);
        assert!(
            repo.find_article_by_source("europepmc", "PMC9999999")
                .await
                .unwrap()
                .is_none()
        );
    }

    #[tokio::test]
    async fn unassigned_volume_and_issue_identity_stays_unique() {
        let db = Arc::new(Db::open_in_memory().unwrap());
        let expected = placement(&db);
        let repo = SqlitePeriodicalRepository::new(db.clone());
        repo.save(&expected.periodical).await.unwrap();

        let first = PeriodicalVolume {
            id: PeriodicalVolumeId::new(),
            periodical_id: expected.periodical.id,
            label: None,
            number: None,
            year: None,
            ordinal: 0,
            created_at: now(),
            updated_at: now(),
        };
        repo.save_volume(&first).await.unwrap();

        let duplicate = PeriodicalVolume {
            id: PeriodicalVolumeId::new(),
            ordinal: 1,
            ..first.clone()
        };
        let error = repo
            .save_volume(&duplicate)
            .await
            .expect_err("同一期刊不能有两个「来源未给出」的卷");
        assert_eq!(error.code().as_str(), "PERIODICAL_IMPORT_CONFLICT");
        assert_eq!(error.kind(), ErrorKind::Conflict);
    }

    #[tokio::test]
    async fn repository_rejects_url_shaped_identity_on_save_and_read() {
        let db = Arc::new(Db::open_in_memory().unwrap());
        let expected = placement(&db);
        let repo = SqlitePeriodicalRepository::new(db.clone());
        save_placement(&repo, &expected).await.unwrap();

        let mut unsafe_article = expected.article.clone();
        unsafe_article.id = PeriodicalArticleId::new();
        unsafe_article.source =
            PeriodicalArticleSourceIdentity::new("europepmc", "PMC7654321").unwrap();
        unsafe_article.title = "https://example.invalid/article".into();
        assert!(repo.save_article(&unsafe_article).await.is_err());

        // 已落库的非法行在读取时必须失败，而不是被当作合法归属返回。
        db.lock()
            .execute(
                "UPDATE periodical_articles SET source_key = 'https://invalid' WHERE id = ?1",
                rusqlite::params![expected.article.id.to_string()],
            )
            .unwrap();
        assert!(
            repo.find_article_by_source("https://invalid", "PMC1234567")
                .await
                .is_err()
        );
    }

    #[tokio::test]
    async fn placement_save_rejects_mismatched_parent_links() {
        let db = Arc::new(Db::open_in_memory().unwrap());
        let mut expected = placement(&db);
        expected.volume.periodical_id = PeriodicalId::new();
        let repo = SqlitePeriodicalRepository::new(db);
        let saved = save_placement(&repo, &expected).await;
        assert!(saved.is_err(), "跨期刊的卷归属必须被拒绝");
    }

    #[tokio::test]
    async fn article_save_rejects_a_non_article_media_item() {
        let db = Arc::new(Db::open_in_memory().unwrap());
        let expected = placement(&db);
        let repo = SqlitePeriodicalRepository::new(db.clone());
        save_placement(&repo, &expected).await.unwrap();

        db.lock()
            .execute(
                "UPDATE media_items SET media_type = 'book' WHERE id = ?1",
                rusqlite::params![expected.article.media_item_id.to_string()],
            )
            .unwrap();
        let error = repo
            .save_article(&expected.article)
            .await
            .expect_err("文章不能绑定非 Article MediaItem");
        assert_eq!(error.code().as_str(), "INVALID_PERIODICAL_IDENTITY");
    }

    /// Provider 的正文观察是文章的持久化事实，读写都必须带上它。
    #[tokio::test]
    async fn article_provider_content_availability_roundtrips() {
        let db = Arc::new(Db::open_in_memory().unwrap());
        let mut expected = placement(&db);
        expected.article.provider_content_availability = PeriodicalArticleAvailability::FullText;
        let repo = SqlitePeriodicalRepository::new(db.clone());
        save_placement(&repo, &expected).await.unwrap();

        assert_eq!(
            repo.list_articles(expected.issue.id).await.unwrap()[0].provider_content_availability,
            PeriodicalArticleAvailability::FullText
        );
        let stored = repo
            .find_article_by_source("europepmc", "PMC1234567")
            .await
            .unwrap()
            .unwrap();
        assert_eq!(
            stored.article.provider_content_availability,
            PeriodicalArticleAvailability::FullText,
            "归属链读取必须带上正文观察"
        );
    }

    /// 040 时代写入的行没有这一列：补列后只能落成保守的「没有观察到」，
    /// 不能被追溯成任何已观察到的正文状态。
    #[tokio::test]
    async fn legacy_article_rows_without_the_observation_read_back_as_unknown() {
        let db = Arc::new(Db::open_in_memory().unwrap());
        let expected = placement(&db);
        let repo = SqlitePeriodicalRepository::new(db.clone());
        save_placement(&repo, &expected).await.unwrap();

        let legacy_media_item = seed_article_media_item(&db);
        db.lock()
            .execute(
                "INSERT INTO periodical_articles
                    (id, issue_id, media_item_id, ordinal, title, doi,
                     page_range_start, page_range_end, source_key, remote_article_id,
                     created_at, updated_at)
                 VALUES ('0196f0d2-0000-7000-8000-0000000000aa', ?1, ?2, NULL, '迁移前的文章',
                         NULL, NULL, NULL, 'europepmc', 'PMC-legacy', 1, 1)",
                rusqlite::params![expected.issue.id.to_string(), legacy_media_item.to_string()],
            )
            .unwrap();

        let articles = repo.list_articles(expected.issue.id).await.unwrap();
        let legacy = articles
            .iter()
            .find(|article| article.source.remote_article_id == "PMC-legacy")
            .expect("旧行必须仍可读取");
        assert_eq!(
            legacy.provider_content_availability,
            PeriodicalArticleAvailability::Unknown,
            "没有 Provider 观察的旧行不得被宣称有正文或只有元数据"
        );
    }
}
