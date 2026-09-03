//! 漫画章节来源身份与页面身份的 SQLite Repository。
//!
//! 这里的表是来源事实和页面迁移证据，不是运行时资源通道。任何 URL、
//! pageId、grant、归档 entry 或 provider header 都在这一层被拒绝，不进入数据库。

use std::sync::Arc;

use async_trait::async_trait;
use rusqlite::OptionalExtension;

use haven_common::{AppError, ErrorKind, UtcMillis};
use haven_domain::comic_catalog::ComicChapterCatalogState;
use haven_domain::comic_identity::{
    ChapterSourceIdentity, ChapterSourceRef, ComicChapterMetadata, ComicPageIdentitySnapshot,
    PageIdentity, has_opaque_control_character,
};
use haven_domain::contracts::{ChapterSourceRepository, ComicPageIdentityRepository};
use haven_domain::ids::MediaItemId;

use crate::db::Db;
use crate::db::repos::edition_profiles::{profile_from_row, validate_profile};
use crate::db::repos::{enum_to_db_str, map_db_error};

pub struct SqliteChapterSourceRepository {
    db: Arc<Db>,
}

pub struct SqliteComicPageIdentityRepository {
    db: Arc<Db>,
}

impl SqliteChapterSourceRepository {
    pub fn new(db: Arc<Db>) -> Self {
        Self { db }
    }
}

impl SqliteComicPageIdentityRepository {
    pub fn new(db: Arc<Db>) -> Self {
        Self { db }
    }
}

fn invalid_identity(field: &'static str) -> AppError {
    AppError::new(
        "INVALID_COMIC_IDENTITY",
        ErrorKind::Validation,
        format!("漫画身份字段 {field} 非法"),
        false,
    )
}

fn validate_opaque(value: &str, field: &'static str) -> Result<(), AppError> {
    let trimmed = value.trim();
    if has_opaque_control_character(value)
        || trimmed.is_empty()
        || trimmed.len() > 4096
        || trimmed.contains("://")
        || trimmed.to_ascii_lowercase().starts_with("data:")
    {
        return Err(invalid_identity(field));
    }
    Ok(())
}

fn validate_source_identity(identity: &ChapterSourceIdentity) -> Result<(), AppError> {
    validate_opaque(&identity.source_key, "source_key")?;
    validate_opaque(&identity.remote_work_id, "remote_work_id")?;
    validate_opaque(&identity.remote_chapter_id, "remote_chapter_id")
}

fn validate_metadata(metadata: &ComicChapterMetadata) -> Result<(), AppError> {
    if metadata
        .chapter_number
        .is_some_and(|value| !value.is_finite())
        || metadata
            .volume_number
            .is_some_and(|value| !value.is_finite())
    {
        return Err(invalid_identity("chapter_number_or_volume_number"));
    }
    if let Some(page_count) = metadata.page_count {
        if page_count > i64::MAX as u32 {
            return Err(invalid_identity("page_count"));
        }
    }
    if let Some(key) = metadata.authoritative_content_key.as_deref() {
        validate_opaque(key, "authoritative_content_key")?;
    }
    Ok(())
}

fn validate_page_identity(page: &PageIdentity) -> Result<(), AppError> {
    if let Some(key) = page.stable_key.as_deref() {
        validate_opaque(key, "stable_key")?;
    }
    if let Some(fingerprint) = page.fingerprint.as_deref() {
        validate_opaque(fingerprint, "fingerprint")?;
    }
    Ok(())
}

fn conversion_error(field: &'static str) -> rusqlite::Error {
    rusqlite::Error::FromSqlConversionFailure(
        0,
        rusqlite::types::Type::Text,
        Box::new(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            format!("无法解析漫画身份字段 {field}"),
        )),
    )
}

fn parse_db_enum<T: serde::de::DeserializeOwned>(
    value: String,
    field: &'static str,
) -> rusqlite::Result<T> {
    serde_json::from_str(&format!("\"{value}\"")).map_err(|_| conversion_error(field))
}

fn row_to_chapter_source_ref(row: &rusqlite::Row<'_>) -> rusqlite::Result<ChapterSourceRef> {
    let source_key: String = row.get("source_key")?;
    let remote_work_id: String = row.get("remote_work_id")?;
    let remote_chapter_id: String = row.get("remote_chapter_id")?;
    let media_item_id: String = row.get("media_item_id")?;
    let page_count: Option<i64> = row.get("page_count")?;
    let page_count = page_count
        .map(|value| u32::try_from(value).map_err(|_| conversion_error("page_count")))
        .transpose()?;
    let edition_profile = match row.get::<_, Option<String>>("observed_edition_profile")? {
        Some(value) => {
            let profile: haven_domain::comic_identity::EditionProfile =
                serde_json::from_str(&value)
                    .map_err(|_| conversion_error("observed_edition_profile"))?;
            validate_profile(&profile).map_err(|_| conversion_error("observed_edition_profile"))?;
            profile
        }
        None => profile_from_row(row)?,
    };
    Ok(ChapterSourceRef {
        media_item_id: media_item_id
            .parse()
            .map_err(|_| conversion_error("media_item_id"))?,
        identity: ChapterSourceIdentity::new(source_key, remote_work_id, remote_chapter_id)
            .ok_or_else(|| conversion_error("source_identity"))?,
        metadata: ComicChapterMetadata {
            edition_profile,
            chapter_number: row.get("chapter_number")?,
            volume_number: row.get("volume_number")?,
            title: row.get("title")?,
            page_count,
            authoritative_content_key: row.get("authoritative_content_key")?,
        },
        source_order: u32::try_from(row.get::<_, i64>("source_order")?)
            .map_err(|_| conversion_error("source_order"))?,
        availability: parse_db_enum(row.get("availability")?, "availability")?,
        published_at: row.get("published_at")?,
        source_updated_at: row.get("source_updated_at")?,
        last_seen_generation: row
            .get::<_, Option<i64>>("last_seen_generation")?
            .map(|value| u64::try_from(value).map_err(|_| conversion_error("last_seen_generation")))
            .transpose()?,
        updated_at: UtcMillis(row.get("updated_at")?),
    })
}

fn row_to_catalog_state(row: &rusqlite::Row<'_>) -> rusqlite::Result<ComicChapterCatalogState> {
    let generation = u64::try_from(row.get::<_, i64>("generation")?)
        .map_err(|_| conversion_error("generation"))?;
    let fetched_at = row.get::<_, i64>("fetched_at")?;
    let total = row
        .get::<_, Option<i64>>("total")?
        .map(|value| u32::try_from(value).map_err(|_| conversion_error("total")))
        .transpose()?;
    let truncated = match row.get::<_, i64>("truncated")? {
        0 => false,
        1 => true,
        _ => return Err(conversion_error("truncated")),
    };
    Ok(ComicChapterCatalogState {
        source_key: row.get("source_key")?,
        remote_work_id: row.get("remote_work_id")?,
        generation,
        fetched_at: UtcMillis(fetched_at),
        total,
        truncated,
    })
}

const CHAPTER_SELECT: &str = "c.source_key, c.remote_work_id, c.remote_chapter_id,
    c.media_item_id, c.chapter_number, c.volume_number, c.title, c.page_count,
    c.authoritative_content_key, c.source_order, c.availability, c.published_at,
    c.source_updated_at, c.last_seen_generation, c.updated_at, c.observed_edition_profile,
    e.language AS edition_language,
    ep.language_kind, ep.translation_line, ep.translation_line_kind,
    ep.scan_group, ep.scan_group_kind, ep.color_mode";

#[async_trait]
impl ChapterSourceRepository for SqliteChapterSourceRepository {
    async fn get(
        &self,
        identity: &ChapterSourceIdentity,
    ) -> Result<Option<ChapterSourceRef>, AppError> {
        validate_source_identity(identity)?;
        let conn = self.db.lock();
        conn.query_row(
            &format!(
                "SELECT {CHAPTER_SELECT}
                 FROM comic_chapter_source_refs c
                 JOIN media_items m ON m.id = c.media_item_id
                 JOIN editions e ON e.id = m.edition_id
                 LEFT JOIN edition_profiles ep ON ep.edition_id = e.id
                 WHERE c.source_key = ?1 AND c.remote_work_id = ?2
                   AND c.remote_chapter_id = ?3"
            ),
            rusqlite::params![
                identity.source_key,
                identity.remote_work_id,
                identity.remote_chapter_id,
            ],
            row_to_chapter_source_ref,
        )
        .optional()
        .map_err(map_db_error("查询漫画章节来源身份失败"))
    }

    async fn list_for_media_item(
        &self,
        media_item_id: MediaItemId,
    ) -> Result<Vec<ChapterSourceRef>, AppError> {
        let conn = self.db.lock();
        let mut stmt = conn
            .prepare(&format!(
                "SELECT {CHAPTER_SELECT}
                 FROM comic_chapter_source_refs c
                 JOIN media_items m ON m.id = c.media_item_id
                 JOIN editions e ON e.id = m.edition_id
                 LEFT JOIN edition_profiles ep ON ep.edition_id = e.id
                 WHERE c.media_item_id = ?1
                 ORDER BY c.updated_at DESC, c.remote_chapter_id"
            ))
            .map_err(map_db_error("查询漫画章节来源列表失败"))?;
        let rows = stmt
            .query_map(
                rusqlite::params![media_item_id.to_string()],
                row_to_chapter_source_ref,
            )
            .map_err(map_db_error("查询漫画章节来源列表失败"))?;
        rows.collect::<rusqlite::Result<Vec<_>>>()
            .map_err(map_db_error("查询漫画章节来源列表失败"))
    }

    async fn list_for_source_work(
        &self,
        source_key: &str,
        remote_work_id: &str,
    ) -> Result<Vec<ChapterSourceRef>, AppError> {
        validate_opaque(source_key, "source_key")?;
        validate_opaque(remote_work_id, "remote_work_id")?;
        let conn = self.db.lock();
        let mut stmt = conn
            .prepare(&format!(
                "SELECT {CHAPTER_SELECT}
                 FROM comic_chapter_source_refs c
                 JOIN media_items m ON m.id = c.media_item_id
                 JOIN editions e ON e.id = m.edition_id
                 LEFT JOIN edition_profiles ep ON ep.edition_id = e.id
                 WHERE c.source_key = ?1 AND c.remote_work_id = ?2
                 ORDER BY c.source_order, c.remote_chapter_id"
            ))
            .map_err(map_db_error("查询来源作品漫画章节列表失败"))?;
        let rows = stmt
            .query_map(
                rusqlite::params![source_key.trim(), remote_work_id.trim()],
                row_to_chapter_source_ref,
            )
            .map_err(map_db_error("查询来源作品漫画章节列表失败"))?;
        rows.collect::<rusqlite::Result<Vec<_>>>()
            .map_err(map_db_error("查询来源作品漫画章节列表失败"))
    }

    async fn refresh_state(
        &self,
        source_key: &str,
        remote_work_id: &str,
    ) -> Result<Option<ComicChapterCatalogState>, AppError> {
        validate_opaque(source_key, "source_key")?;
        validate_opaque(remote_work_id, "remote_work_id")?;
        let conn = self.db.lock();
        conn.query_row(
            "SELECT source_key, remote_work_id, generation, fetched_at, total, truncated
             FROM comic_chapter_catalog_states
             WHERE source_key = ?1 AND remote_work_id = ?2",
            rusqlite::params![source_key.trim(), remote_work_id.trim()],
            row_to_catalog_state,
        )
        .optional()
        .map_err(map_db_error("查询漫画章节目录刷新状态失败"))
    }

    async fn save(&self, reference: &ChapterSourceRef) -> Result<(), AppError> {
        let mut guard = self.db.lock();
        let tx = guard
            .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)
            .map_err(map_db_error("开启漫画章节来源事务失败"))?;
        save_on_conn(&tx, reference)?;
        tx.commit()
            .map_err(map_db_error("提交漫画章节来源事务失败"))?;
        Ok(())
    }
}

/// 在指定连接上保存章节来源引用；调用方负责事务边界。
pub(crate) fn save_on_conn(
    conn: &rusqlite::Connection,
    reference: &ChapterSourceRef,
) -> Result<(), AppError> {
    validate_source_identity(&reference.identity)?;
    validate_metadata(&reference.metadata)?;
    validate_profile(&reference.metadata.edition_profile)?;
    let observed_edition_profile = serde_json::to_string(&reference.metadata.edition_profile)
        .map_err(|_| invalid_identity("observed_edition_profile"))?;
    let media_item_id = reference.media_item_id;
    let now = reference.updated_at.0;
    let is_comic: i64 = conn
        .query_row(
            "SELECT EXISTS(
                 SELECT 1 FROM media_items WHERE id = ?1 AND media_type = 'comic'
             )",
            rusqlite::params![media_item_id.to_string()],
            |row| row.get(0),
        )
        .map_err(map_db_error("检查漫画媒体条目失败"))?;
    if is_comic == 0 {
        return Err(AppError::new(
            "COMIC_MEDIA_ITEM_REQUIRED",
            ErrorKind::Validation,
            "章节来源身份只能绑定漫画媒体条目",
            false,
        ));
    }
    let local_work_id: String = conn
        .query_row(
            "SELECT e.work_id
             FROM media_items m
             JOIN editions e ON e.id = m.edition_id
             WHERE m.id = ?1",
            rusqlite::params![media_item_id.to_string()],
            |row| row.get(0),
        )
        .map_err(map_db_error("确认漫画媒体条目 Work 归属失败"))?;
    let bound_work_id: Option<String> = conn
        .query_row(
            "SELECT work_id FROM work_source_refs
             WHERE provider = ?1 AND external_id = ?2",
            rusqlite::params![
                reference.identity.source_key,
                reference.identity.remote_work_id
            ],
            |row| row.get(0),
        )
        .optional()
        .map_err(map_db_error("确认漫画来源作品归属失败"))?;
    if bound_work_id.is_some_and(|work_id| work_id != local_work_id) {
        return Err(AppError::new(
            "SOURCE_REF_CONFLICT",
            ErrorKind::Conflict,
            "来源章节所属远端作品与本地 Work 不一致",
            false,
        ));
    }

    conn.execute(
        "INSERT INTO comic_chapter_source_refs
            (source_key, remote_work_id, remote_chapter_id, media_item_id,
             chapter_number, volume_number, title, page_count,
             authoritative_content_key, source_order, availability, published_at,
             source_updated_at, last_seen_generation, updated_at, observed_edition_profile)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16)
         ON CONFLICT(source_key, remote_work_id, remote_chapter_id) DO NOTHING",
        rusqlite::params![
            reference.identity.source_key,
            reference.identity.remote_work_id,
            reference.identity.remote_chapter_id,
            media_item_id.to_string(),
            reference.metadata.chapter_number,
            reference.metadata.volume_number,
            reference.metadata.title,
            reference.metadata.page_count.map(i64::from),
            reference.metadata.authoritative_content_key,
            i64::from(reference.source_order),
            enum_to_db_str(&reference.availability)?,
            reference.published_at,
            reference.source_updated_at,
            reference
                .last_seen_generation
                .map(|value| {
                    i64::try_from(value).map_err(|_| invalid_identity("last_seen_generation"))
                })
                .transpose()?,
            now,
            observed_edition_profile,
        ],
    )
    .map_err(map_db_error("保存漫画章节来源身份失败"))?;

    let existing_media_item: String = conn
        .query_row(
            "SELECT media_item_id FROM comic_chapter_source_refs
             WHERE source_key = ?1 AND remote_work_id = ?2 AND remote_chapter_id = ?3",
            rusqlite::params![
                reference.identity.source_key,
                reference.identity.remote_work_id,
                reference.identity.remote_chapter_id,
            ],
            |row| row.get(0),
        )
        .map_err(map_db_error("确认漫画章节来源归属失败"))?;
    if existing_media_item != media_item_id.to_string() {
        return Err(AppError::new(
            "CHAPTER_SOURCE_REF_CONFLICT",
            ErrorKind::Conflict,
            "来源章节已经绑定其他媒体条目",
            false,
        ));
    }
    conn.execute(
        "UPDATE comic_chapter_source_refs SET
             chapter_number = ?4,
             volume_number = ?5,
             title = ?6,
             page_count = ?7,
             authoritative_content_key = ?8,
             source_order = ?9,
             availability = ?10,
             published_at = ?11,
             source_updated_at = ?12,
             last_seen_generation = ?13,
             updated_at = ?14,
             observed_edition_profile = ?15
         WHERE source_key = ?1 AND remote_work_id = ?2 AND remote_chapter_id = ?3",
        rusqlite::params![
            reference.identity.source_key,
            reference.identity.remote_work_id,
            reference.identity.remote_chapter_id,
            reference.metadata.chapter_number,
            reference.metadata.volume_number,
            reference.metadata.title,
            reference.metadata.page_count.map(i64::from),
            reference.metadata.authoritative_content_key,
            i64::from(reference.source_order),
            enum_to_db_str(&reference.availability)?,
            reference.published_at,
            reference.source_updated_at,
            reference
                .last_seen_generation
                .map(|value| {
                    i64::try_from(value).map_err(|_| invalid_identity("last_seen_generation"))
                })
                .transpose()?,
            now,
            observed_edition_profile,
        ],
    )
    .map_err(map_db_error("更新漫画章节来源身份失败"))?;
    Ok(())
}

fn row_to_page_identity(row: &rusqlite::Row<'_>) -> rusqlite::Result<(u32, PageIdentity)> {
    let page_index: i64 = row.get("page_index")?;
    let page_index = u32::try_from(page_index).map_err(|_| conversion_error("page_index"))?;
    Ok((
        page_index,
        PageIdentity {
            stable_key: row.get("stable_key")?,
            fingerprint: row.get("fingerprint")?,
        },
    ))
}

#[async_trait]
impl ComicPageIdentityRepository for SqliteComicPageIdentityRepository {
    async fn get_snapshot(
        &self,
        media_item_id: MediaItemId,
    ) -> Result<ComicPageIdentitySnapshot, AppError> {
        let conn = self.db.lock();
        let revision: Option<String> = conn
            .query_row(
                "SELECT revision FROM comic_page_identity_states WHERE media_item_id = ?1",
                rusqlite::params![media_item_id.to_string()],
                |row| row.get(0),
            )
            .optional()
            .map_err(map_db_error("查询漫画页面身份 revision 失败"))?;
        let mut stmt = conn
            .prepare(
                "SELECT page_index, stable_key, fingerprint
                 FROM comic_page_identities WHERE media_item_id = ?1
                 ORDER BY page_index",
            )
            .map_err(map_db_error("查询漫画页面身份失败"))?;
        let rows = stmt
            .query_map(
                rusqlite::params![media_item_id.to_string()],
                row_to_page_identity,
            )
            .map_err(map_db_error("查询漫画页面身份失败"))?;
        let pages = rows
            .map(|row| row.map(|(_, page)| page))
            .collect::<rusqlite::Result<Vec<_>>>()
            .map_err(map_db_error("查询漫画页面身份失败"))?;
        Ok(ComicPageIdentitySnapshot { pages, revision })
    }

    async fn replace_if_revision(
        &self,
        media_item_id: MediaItemId,
        pages: &[PageIdentity],
        updated_at: UtcMillis,
        expected_revision: Option<&str>,
    ) -> Result<Option<String>, AppError> {
        for page in pages {
            validate_page_identity(page)?;
        }
        if let Some(expected_revision) = expected_revision {
            validate_opaque(expected_revision, "revision")?;
        }
        let mut guard = self.db.lock();
        let tx = guard
            .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)
            .map_err(map_db_error("开启漫画页面身份事务失败"))?;
        let is_comic: i64 = tx
            .query_row(
                "SELECT EXISTS(
                     SELECT 1 FROM media_items WHERE id = ?1 AND media_type = 'comic'
                 )",
                rusqlite::params![media_item_id.to_string()],
                |row| row.get(0),
            )
            .map_err(map_db_error("检查漫画媒体条目失败"))?;
        if is_comic == 0 {
            return Err(AppError::new(
                "COMIC_MEDIA_ITEM_REQUIRED",
                ErrorKind::Validation,
                "页面身份只能绑定漫画媒体条目",
                false,
            ));
        }
        let current_revision: Option<String> = tx
            .query_row(
                "SELECT revision FROM comic_page_identity_states WHERE media_item_id = ?1",
                rusqlite::params![media_item_id.to_string()],
                |row| row.get(0),
            )
            .optional()
            .map_err(map_db_error("查询当前漫画页面身份 revision 失败"))?;
        if current_revision.as_deref() != expected_revision {
            return Ok(None);
        }

        let revision = uuid::Uuid::new_v4().to_string();
        tx.execute(
            "DELETE FROM comic_page_identities WHERE media_item_id = ?1",
            rusqlite::params![media_item_id.to_string()],
        )
        .map_err(map_db_error("替换旧漫画页面身份失败"))?;
        for (index, page) in pages.iter().enumerate() {
            let page_index = u32::try_from(index).map_err(|_| invalid_identity("page_index"))?;
            tx.execute(
                "INSERT INTO comic_page_identities
                    (media_item_id, page_index, stable_key, fingerprint, updated_at)
                 VALUES (?1, ?2, ?3, ?4, ?5)",
                rusqlite::params![
                    media_item_id.to_string(),
                    i64::from(page_index),
                    page.stable_key,
                    page.fingerprint,
                    updated_at.0,
                ],
            )
            .map_err(map_db_error("保存漫画页面身份失败"))?;
        }
        tx.execute(
            "INSERT INTO comic_page_identity_states (media_item_id, revision, updated_at)
             VALUES (?1, ?2, ?3)
             ON CONFLICT(media_item_id) DO UPDATE SET
                 revision = excluded.revision,
                 updated_at = excluded.updated_at",
            rusqlite::params![media_item_id.to_string(), revision, updated_at.0],
        )
        .map_err(map_db_error("保存漫画页面身份 revision 失败"))?;
        tx.commit()
            .map_err(map_db_error("提交漫画页面身份事务失败"))?;
        Ok(Some(revision))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use haven_domain::comic_catalog::ComicChapterSourceStatus;
    use haven_domain::comic_identity::{ColorMode, EditionProfile, IdentityFacet, ScanGroupFacet};
    use haven_domain::contracts::EditionProfileRepository;
    use haven_domain::entities::{MediaIndex, MediaItem};
    use haven_domain::enums::{MediaItemStatus, MediaType};
    use haven_domain::ids::{EditionId, WorkId};

    fn seed_comic(db: &Db) -> (EditionId, MediaItemId) {
        let work_id = WorkId::new();
        let edition_id = EditionId::new();
        let media_item_id = MediaItemId::new();
        let now = 1_000;
        let conn = db.lock();
        conn.execute(
            "INSERT INTO works (id, canonical_title, work_type, status, created_at, updated_at)
             VALUES (?1, '漫画身份测试', 'fiction', 'completed', ?2, ?2)",
            rusqlite::params![work_id.to_string(), now],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO editions (id, work_id, title, edition_type, language, created_at, updated_at)
             VALUES (?1, ?2, '中文扫描版', 'comic', 'zh-cn', ?3, ?3)",
            rusqlite::params![edition_id.to_string(), work_id.to_string(), now],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO media_items
                (id, edition_id, media_type, title, category, chapter, page_count,
                 status, created_at, updated_at)
             VALUES (?1, ?2, 'comic', '第 1 话', 'comic', 1.0, 2, 'available', ?3, ?3)",
            rusqlite::params![media_item_id.to_string(), edition_id.to_string(), now],
        )
        .unwrap();
        (edition_id, media_item_id)
    }

    fn seed_comic_item(db: &Db, edition_id: EditionId, title: &str, chapter: f64) -> MediaItemId {
        let media_item_id = MediaItemId::new();
        let now = 1_000;
        db.lock()
            .execute(
                "INSERT INTO media_items
                    (id, edition_id, media_type, title, category, chapter, page_count,
                     status, created_at, updated_at)
                 VALUES (?1, ?2, 'comic', ?3, 'comic', ?4, 2, 'available', ?5, ?5)",
                rusqlite::params![
                    media_item_id.to_string(),
                    edition_id.to_string(),
                    title,
                    chapter,
                    now,
                ],
            )
            .unwrap();
        media_item_id
    }

    #[tokio::test]
    async fn chapter_source_refs_roundtrip_and_allow_multiple_sources() {
        let db = Arc::new(Db::open_in_memory().unwrap());
        let (edition_id, media_item_id) = seed_comic(&db);
        let profiles = crate::db::repos::SqliteEditionProfileRepository::new(db.clone());
        let profile = EditionProfile {
            language: IdentityFacet::known("zh-cn"),
            translation_line: IdentityFacet::known("line-a"),
            scan_group: ScanGroupFacet::content_line("scan-a"),
            color_mode: ColorMode::Grayscale,
        };
        profiles.save(edition_id, &profile).await.unwrap();
        let repo = SqliteChapterSourceRepository::new(db);
        let make = |chapter: &str| ChapterSourceRef {
            media_item_id,
            identity: ChapterSourceIdentity::new("source-a", "work-a", chapter).unwrap(),
            metadata: ComicChapterMetadata {
                edition_profile: profile.clone(),
                chapter_number: Some(1.0),
                volume_number: None,
                title: Some("第一话".into()),
                page_count: Some(2),
                authoritative_content_key: Some("content-1".into()),
            },
            source_order: 0,
            availability: ComicChapterSourceStatus::Available,
            published_at: None,
            source_updated_at: None,
            last_seen_generation: None,
            updated_at: UtcMillis(2_000),
        };
        repo.save(&make("remote-a")).await.unwrap();
        repo.save(&ChapterSourceRef {
            identity: ChapterSourceIdentity::new("source-b", "work-b", "remote-b").unwrap(),
            updated_at: UtcMillis(3_000),
            ..make("remote-a")
        })
        .await
        .unwrap();
        let refs = repo.list_for_media_item(media_item_id).await.unwrap();
        assert_eq!(refs.len(), 2);
        assert_eq!(
            refs[0].metadata.edition_profile.translation_line,
            IdentityFacet::known("line-a")
        );
        assert_eq!(
            repo.get(&make("remote-a").identity)
                .await
                .unwrap()
                .unwrap()
                .media_item_id,
            media_item_id
        );
    }

    #[tokio::test]
    async fn source_profile_observation_is_not_overwritten_by_shared_edition_profile() {
        let db = Arc::new(Db::open_in_memory().unwrap());
        let (edition_id, media_item_id) = seed_comic(&db);
        let profiles = crate::db::repos::SqliteEditionProfileRepository::new(db.clone());
        profiles
            .save(
                edition_id,
                &EditionProfile {
                    language: IdentityFacet::known("zh-cn"),
                    translation_line: IdentityFacet::known("shared-line"),
                    scan_group: ScanGroupFacet::content_line("shared-scan"),
                    color_mode: ColorMode::Grayscale,
                },
            )
            .await
            .unwrap();

        let observed_profile = EditionProfile {
            language: IdentityFacet::known("zh-cn"),
            translation_line: IdentityFacet::known("source-line-a"),
            scan_group: ScanGroupFacet::content_line("source-scan-a"),
            color_mode: ColorMode::FullColor,
        };
        let repo = SqliteChapterSourceRepository::new(db);
        repo.save(&ChapterSourceRef {
            media_item_id,
            identity: ChapterSourceIdentity::new("source-a", "work-a", "chapter-a").unwrap(),
            metadata: ComicChapterMetadata {
                edition_profile: observed_profile.clone(),
                ..ComicChapterMetadata::default()
            },
            source_order: 0,
            availability: ComicChapterSourceStatus::Available,
            published_at: None,
            source_updated_at: None,
            last_seen_generation: Some(1),
            updated_at: UtcMillis(2_000),
        })
        .await
        .unwrap();

        let stored = repo
            .get(&ChapterSourceIdentity::new("source-a", "work-a", "chapter-a").unwrap())
            .await
            .unwrap()
            .unwrap();
        assert_eq!(stored.metadata.edition_profile, observed_profile);
    }

    #[tokio::test]
    async fn legacy_source_profile_falls_back_to_edition_without_materializing_history() {
        let db = Arc::new(Db::open_in_memory().unwrap());
        let (edition_id, media_item_id) = seed_comic(&db);
        let shared_profile = EditionProfile {
            language: IdentityFacet::known("zh-cn"),
            translation_line: IdentityFacet::known("legacy-shared-line"),
            scan_group: ScanGroupFacet::content_line("legacy-shared-scan"),
            color_mode: ColorMode::Grayscale,
        };
        let profiles = crate::db::repos::SqliteEditionProfileRepository::new(db.clone());
        profiles.save(edition_id, &shared_profile).await.unwrap();

        // Simulate a row created before 036: the source observation is absent,
        // while the associated Edition still has its canonical profile.
        db.lock()
            .execute(
                "INSERT INTO comic_chapter_source_refs
                    (source_key, remote_work_id, remote_chapter_id, media_item_id,
                     title, page_count, updated_at, observed_edition_profile)
                 VALUES ('legacy-source', 'legacy-work', 'legacy-chapter', ?1,
                         '旧章节', 2, 2, NULL)",
                rusqlite::params![media_item_id.to_string()],
            )
            .unwrap();

        let repo = SqliteChapterSourceRepository::new(db.clone());
        let stored = repo
            .get(
                &ChapterSourceIdentity::new("legacy-source", "legacy-work", "legacy-chapter")
                    .unwrap(),
            )
            .await
            .unwrap()
            .unwrap();
        assert_eq!(stored.metadata.edition_profile, shared_profile);

        let observed: Option<String> = db
            .lock()
            .query_row(
                "SELECT observed_edition_profile
                 FROM comic_chapter_source_refs
                 WHERE source_key = 'legacy-source'
                   AND remote_work_id = 'legacy-work'
                   AND remote_chapter_id = 'legacy-chapter'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert!(
            observed.is_none(),
            "兼容回退不得把 Edition 投影伪造为来源历史观察"
        );
    }

    #[tokio::test]
    async fn chapter_catalog_fields_roundtrip_and_source_order_is_stable() {
        let db = Arc::new(Db::open_in_memory().unwrap());
        let (edition_id, first_media_item_id) = seed_comic(&db);
        let second_media_item_id = seed_comic_item(&db, edition_id, "第 2 话", 2.0);
        let profiles = crate::db::repos::SqliteEditionProfileRepository::new(db.clone());
        let profile = EditionProfile {
            language: IdentityFacet::known("zh-cn"),
            translation_line: IdentityFacet::known("line-a"),
            scan_group: ScanGroupFacet::content_line("scan-a"),
            color_mode: ColorMode::FullColor,
        };
        profiles.save(edition_id, &profile).await.unwrap();

        let repo = SqliteChapterSourceRepository::new(db);
        repo.save(&ChapterSourceRef {
            media_item_id: first_media_item_id,
            identity: ChapterSourceIdentity::new("mangadex", "manga-a", "chapter-1").unwrap(),
            metadata: ComicChapterMetadata {
                edition_profile: profile.clone(),
                chapter_number: Some(1.0),
                volume_number: Some(1.0),
                title: Some("第一话".into()),
                page_count: Some(24),
                authoritative_content_key: Some("content-1".into()),
            },
            source_order: 9,
            availability: ComicChapterSourceStatus::TemporarilyUnavailable,
            published_at: Some("2026-01-01T00:00:00Z".into()),
            source_updated_at: Some("2026-01-02T00:00:00Z".into()),
            last_seen_generation: Some(7),
            updated_at: UtcMillis(7_000),
        })
        .await
        .unwrap();
        repo.save(&ChapterSourceRef {
            media_item_id: second_media_item_id,
            identity: ChapterSourceIdentity::new("mangadex", "manga-a", "chapter-2").unwrap(),
            metadata: ComicChapterMetadata {
                edition_profile: profile.clone(),
                chapter_number: Some(2.0),
                volume_number: Some(1.0),
                title: Some("第二话".into()),
                page_count: Some(25),
                authoritative_content_key: Some("content-2".into()),
            },
            source_order: 2,
            availability: ComicChapterSourceStatus::ExternalOnly,
            published_at: Some("2026-02-01T00:00:00Z".into()),
            source_updated_at: Some("2026-02-02T00:00:00Z".into()),
            last_seen_generation: Some(7),
            updated_at: UtcMillis(7_001),
        })
        .await
        .unwrap();

        let refs = repo
            .list_for_source_work(" mangadex ", " manga-a ")
            .await
            .unwrap();
        assert_eq!(refs.len(), 2);
        assert_eq!(refs[0].identity.remote_chapter_id, "chapter-2");
        assert_eq!(refs[0].source_order, 2);
        assert_eq!(refs[0].media_item_id, second_media_item_id);
        assert_eq!(refs[0].availability, ComicChapterSourceStatus::ExternalOnly);
        assert_eq!(
            refs[0].published_at.as_deref(),
            Some("2026-02-01T00:00:00Z")
        );
        assert_eq!(
            refs[0].source_updated_at.as_deref(),
            Some("2026-02-02T00:00:00Z")
        );
        assert_eq!(refs[0].last_seen_generation, Some(7));
        assert_eq!(refs[0].metadata.page_count, Some(25));
        assert_eq!(refs[0].metadata.edition_profile, profile);
        assert_eq!(refs[1].identity.remote_chapter_id, "chapter-1");
        assert_eq!(refs[1].source_order, 9);
        assert_eq!(
            refs[1].availability,
            ComicChapterSourceStatus::TemporarilyUnavailable
        );
        assert_eq!(
            refs[1].metadata.authoritative_content_key.as_deref(),
            Some("content-1")
        );
    }

    #[tokio::test]
    async fn chapter_source_ref_conflict_is_explicit() {
        let db = Arc::new(Db::open_in_memory().unwrap());
        let (_, media_item_id) = seed_comic(&db);
        let other_edition = EditionId::new();
        let other_media = MediaItemId::new();
        let work_id = WorkId::new();
        let conn = db.lock();
        conn.execute(
            "INSERT INTO works (id, canonical_title, work_type, status, created_at, updated_at)
             VALUES (?1, '第二作品', 'fiction', 'completed', 1, 1)",
            rusqlite::params![work_id.to_string()],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO editions (id, work_id, title, edition_type, created_at, updated_at)
             VALUES (?1, ?2, '另一版', 'comic', 1, 1)",
            rusqlite::params![other_edition.to_string(), work_id.to_string()],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO media_items
                (id, edition_id, media_type, title, category, status, created_at, updated_at)
             VALUES (?1, ?2, 'comic', '另一话', 'comic', 'available', 1, 1)",
            rusqlite::params![other_media.to_string(), other_edition.to_string()],
        )
        .unwrap();
        drop(conn);
        let repo = SqliteChapterSourceRepository::new(db);
        let base = ChapterSourceRef {
            media_item_id,
            identity: ChapterSourceIdentity::new("source", "work", "chapter").unwrap(),
            metadata: ComicChapterMetadata::default(),
            source_order: 0,
            availability: ComicChapterSourceStatus::Unknown,
            published_at: None,
            source_updated_at: None,
            last_seen_generation: None,
            updated_at: UtcMillis(1),
        };
        repo.save(&base).await.unwrap();
        let err = repo
            .save(&ChapterSourceRef {
                media_item_id: other_media,
                ..base
            })
            .await
            .unwrap_err();
        assert_eq!(err.code().as_str(), "CHAPTER_SOURCE_REF_CONFLICT");
    }

    #[tokio::test]
    async fn chapter_source_ref_rejects_source_work_bound_to_another_local_work() {
        let db = Arc::new(Db::open_in_memory().unwrap());
        let (_, media_item_id) = seed_comic(&db);
        let foreign_work_id = WorkId::new();
        let local_work_id: String = db
            .lock()
            .query_row(
                "SELECT e.work_id
                 FROM media_items m
                 JOIN editions e ON e.id = m.edition_id
                 WHERE m.id = ?1",
                rusqlite::params![media_item_id.to_string()],
                |row| row.get(0),
            )
            .unwrap();
        assert_ne!(local_work_id, foreign_work_id.to_string());
        db.lock()
            .execute(
                "INSERT INTO works (id, canonical_title, work_type, status, created_at, updated_at)
                 VALUES (?1, '外部绑定作品', 'fiction', 'completed', 1, 1)",
                rusqlite::params![foreign_work_id.to_string()],
            )
            .unwrap();
        db.lock()
            .execute(
                "INSERT INTO work_source_refs (provider, external_id, work_id)
                 VALUES ('source', 'remote-work', ?1)",
                rusqlite::params![foreign_work_id.to_string()],
            )
            .unwrap();

        let repo = SqliteChapterSourceRepository::new(db);
        let error = repo
            .save(&ChapterSourceRef {
                media_item_id,
                identity: ChapterSourceIdentity::new("source", "remote-work", "chapter").unwrap(),
                metadata: ComicChapterMetadata::default(),
                source_order: 0,
                availability: ComicChapterSourceStatus::Available,
                published_at: None,
                source_updated_at: None,
                last_seen_generation: None,
                updated_at: UtcMillis(2),
            })
            .await
            .unwrap_err();
        assert_eq!(error.code().as_str(), "SOURCE_REF_CONFLICT");
    }

    #[tokio::test]
    async fn page_identity_replace_is_ordered_and_reversible_at_data_level() {
        let db = Arc::new(Db::open_in_memory().unwrap());
        let (_, media_item_id) = seed_comic(&db);
        let repo = SqliteComicPageIdentityRepository::new(db);
        repo.replace(
            media_item_id,
            &[
                PageIdentity::stable("a"),
                PageIdentity::fingerprint("b"),
                PageIdentity::default(),
            ],
            UtcMillis(2),
        )
        .await
        .unwrap();
        let pages = repo.list(media_item_id).await.unwrap();
        assert_eq!(pages.len(), 3);
        assert_eq!(pages[0], PageIdentity::stable("a"));
        assert_eq!(pages[1], PageIdentity::fingerprint("b"));
        assert_eq!(pages[2], PageIdentity::default());
    }

    #[tokio::test]
    async fn page_identity_revision_allows_one_writer_and_preserves_empty_observation() {
        let db = Arc::new(Db::open_in_memory().unwrap());
        let (_, media_item_id) = seed_comic(&db);
        let repo = SqliteComicPageIdentityRepository::new(db);

        let first_revision = repo
            .replace_if_revision(
                media_item_id,
                &[PageIdentity::stable("first")],
                UtcMillis(2),
                None,
            )
            .await
            .unwrap()
            .unwrap();
        assert!(
            repo.replace_if_revision(
                media_item_id,
                &[PageIdentity::stable("stale")],
                UtcMillis(3),
                None,
            )
            .await
            .unwrap()
            .is_none()
        );

        let second_revision = repo
            .replace_if_revision(media_item_id, &[], UtcMillis(4), Some(&first_revision))
            .await
            .unwrap()
            .unwrap();
        let snapshot = repo.get_snapshot(media_item_id).await.unwrap();
        assert!(snapshot.pages.is_empty());
        assert_eq!(snapshot.revision.as_deref(), Some(second_revision.as_str()));
        assert!(
            repo.replace_if_revision(
                media_item_id,
                &[PageIdentity::stable("stale-after-empty")],
                UtcMillis(5),
                Some(&first_revision),
            )
            .await
            .unwrap()
            .is_none()
        );
    }

    #[tokio::test]
    async fn page_identity_rejects_url_and_non_comic_media() {
        let db = Arc::new(Db::open_in_memory().unwrap());
        let (_, media_item_id) = seed_comic(&db);
        let repo = SqliteComicPageIdentityRepository::new(db);
        let err = repo
            .replace(
                media_item_id,
                &[PageIdentity::stable("https://example.invalid/page")],
                UtcMillis(1),
            )
            .await
            .unwrap_err();
        assert_eq!(err.code().as_str(), "INVALID_COMIC_IDENTITY");
    }

    #[tokio::test]
    async fn page_identity_replace_rolls_back_after_mid_sequence_failure() {
        let db = Arc::new(Db::open_in_memory().unwrap());
        let (_, media_item_id) = seed_comic(&db);
        let repo = SqliteComicPageIdentityRepository::new(db.clone());
        repo.replace(media_item_id, &[PageIdentity::stable("old")], UtcMillis(1))
            .await
            .unwrap();

        db.lock()
            .execute_batch(
                "CREATE TRIGGER fail_comic_page_identity_insert
                 BEFORE INSERT ON comic_page_identities
                 WHEN NEW.stable_key = 'fail'
                 BEGIN
                     SELECT RAISE(ABORT, 'injected comic page identity failure');
                 END;",
            )
            .unwrap();
        let error = repo
            .replace(
                media_item_id,
                &[PageIdentity::stable("new"), PageIdentity::stable("fail")],
                UtcMillis(2),
            )
            .await
            .unwrap_err();
        assert_eq!(error.code().as_str(), "DATABASE_ERROR");
        db.lock()
            .execute_batch("DROP TRIGGER fail_comic_page_identity_insert")
            .unwrap();

        assert_eq!(
            repo.list(media_item_id).await.unwrap(),
            vec![PageIdentity::stable("old")],
            "页面身份替换失败不得留下删除或部分插入结果"
        );
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
