//! 漫画进度 Subject/Member 的 SQLite Repository。
//!
//! Subject 只保存跨 MediaItem 的连续性身份映射；所有页面、完成度、比例、
//! keyframe 和 Progress revision 都留在既有 Progress Repository。成员 evidence
//! 由领域类型序列化，且本层拒绝 URL/控制字符等运行时通道内容。

use std::sync::Arc;

use rusqlite::OptionalExtension;

use haven_common::{AppError, ErrorKind, UtcMillis};
use haven_domain::comic_progress_subject::{ComicProgressSubject, ComicProgressSubjectMember};
use haven_domain::contracts::ComicProgressSubjectRepository;
use haven_domain::ids::{ComicProgressSubjectId, EditionId, MediaItemId, WorkId};

use crate::db::Db;
use crate::db::repos::{enum_to_db_str, map_db_error};

pub struct SqliteComicProgressSubjectRepository {
    db: Arc<Db>,
}

impl SqliteComicProgressSubjectRepository {
    pub fn new(db: Arc<Db>) -> Self {
        Self { db }
    }

    pub async fn save(&self, subject: &ComicProgressSubject) -> Result<(), AppError> {
        let subject = subject.clone();
        self.db
            .with_tx(|tx| save_on_conn(tx, &subject, subject.members()))
    }

    pub async fn get(
        &self,
        id: ComicProgressSubjectId,
    ) -> Result<Option<ComicProgressSubject>, AppError> {
        let conn = self.db.lock();
        load_subject(&conn, id)
    }

    pub async fn list_by_work(
        &self,
        work_id: WorkId,
    ) -> Result<Vec<ComicProgressSubject>, AppError> {
        let conn = self.db.lock();
        let ids = conn
            .prepare(
                "SELECT id FROM comic_progress_subjects
                 WHERE work_id = ?1 ORDER BY created_at, id",
            )
            .map_err(map_db_error("查询漫画进度主体列表失败"))?
            .query_map(rusqlite::params![work_id.to_string()], |row| {
                let id: String = row.get(0)?;
                id.parse().map_err(|_| invalid_row("id"))
            })
            .map_err(map_db_error("查询漫画进度主体列表失败"))?
            .collect::<rusqlite::Result<Vec<_>>>()
            .map_err(map_db_error("查询漫画进度主体列表失败"))?;
        ids.into_iter()
            .map(|id| load_subject(&conn, id))
            .collect::<Result<Vec<_>, _>>()
            .map(|subjects| subjects.into_iter().flatten().collect())
    }
}

#[async_trait::async_trait]
impl ComicProgressSubjectRepository for SqliteComicProgressSubjectRepository {
    async fn get(
        &self,
        id: ComicProgressSubjectId,
    ) -> Result<Option<ComicProgressSubject>, AppError> {
        SqliteComicProgressSubjectRepository::get(self, id).await
    }

    async fn get_for_media_item(
        &self,
        media_item_id: MediaItemId,
    ) -> Result<Option<ComicProgressSubjectMember>, AppError> {
        let conn = self.db.lock();
        load_member_for_media_item(&conn, media_item_id)
    }

    async fn list_members(
        &self,
        subject_id: ComicProgressSubjectId,
    ) -> Result<Vec<ComicProgressSubjectMember>, AppError> {
        let conn = self.db.lock();
        load_members(&conn, &subject_id.to_string())
    }

    async fn save_subject(&self, subject: &ComicProgressSubject) -> Result<(), AppError> {
        self.save(subject).await
    }

    async fn save_member(&self, member: &ComicProgressSubjectMember) -> Result<(), AppError> {
        let member = member.clone();
        // BEGIN IMMEDIATE：先取写锁再读整个聚合。Deferred 事务会先按旧快照读
        // Subject/成员，等到写阶段才发现并发变更；这里把"读 Subject + 全部成员 →
        // 合并待写成员 → 完整校验 → 替换写入"放进同一个 Immediate 事务，校验
        // 失败整笔回滚，不留下部分写入，旧聚合保持可读。
        let mut guard = self.db.lock();
        let tx = guard
            .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)
            .map_err(map_db_error("开启漫画进度主体成员 Immediate 事务失败"))?;
        match save_member_in_immediate_tx(&tx, &member) {
            Ok(()) => tx
                .commit()
                .map_err(map_db_error("提交漫画进度主体成员事务失败")),
            Err(error) => Err(error),
        }
    }
}

/// 在调用方给定的连接上原子合并一个成员：连接必须已处于 Immediate 事务。
///
/// 聚合必须整体重写（`save_on_conn` 会先 DELETE 再插入全部成员），因此写入前
/// 先加载 Subject 与现有全部成员，再按 `(subject_id, media_item_id, state)`
/// 合并待写行，最后交给 `save_on_conn` 做完整校验与写入。
fn save_member_in_immediate_tx(
    conn: &rusqlite::Connection,
    member: &ComicProgressSubjectMember,
) -> Result<(), AppError> {
    let Some(subject) = load_subject(conn, member.subject_id)? else {
        return Err(invalid_subject("漫画进度主体不存在"));
    };
    let mut members = load_members(conn, &member.subject_id.to_string())?;
    match members.iter_mut().find(|existing| {
        existing.media_item_id == member.media_item_id && existing.state == member.state
    }) {
        Some(existing) => *existing = member.clone(),
        None => members.push(member.clone()),
    }
    save_on_conn(conn, &subject, &members)
}

pub(crate) fn save_on_conn(
    conn: &rusqlite::Connection,
    subject: &ComicProgressSubject,
    members: &[ComicProgressSubjectMember],
) -> Result<(), AppError> {
    let mut validated = subject.clone();
    validated
        .load_members(members.to_vec())
        .map_err(|error| invalid_subject(error.to_string()))?;
    validated
        .validate()
        .map_err(|error| invalid_subject(error.to_string()))?;
    ensure_subject_hierarchy(conn, &validated, members)?;
    validate_subject_text(&validated.algorithm_version, "algorithm_version")?;

    conn.execute(
        "INSERT INTO comic_progress_subjects
            (id, work_id, edition_id, canonical_media_item_id,
             authoritative_progress_media_item_id, state, created_at, updated_at,
             algorithm_version, redirect_subject_id)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)
         ON CONFLICT(id) DO UPDATE SET
             work_id = excluded.work_id,
             edition_id = excluded.edition_id,
             canonical_media_item_id = excluded.canonical_media_item_id,
             authoritative_progress_media_item_id = excluded.authoritative_progress_media_item_id,
             state = excluded.state,
             created_at = excluded.created_at,
             updated_at = excluded.updated_at,
             algorithm_version = excluded.algorithm_version,
             redirect_subject_id = excluded.redirect_subject_id",
        rusqlite::params![
            validated.id.to_string(),
            validated.work_id.to_string(),
            validated.edition_id.to_string(),
            validated.canonical_media_item_id.to_string(),
            validated
                .authoritative_progress_media_item_id
                .map(|id| id.to_string()),
            enum_to_db_str(&validated.state)?,
            validated.created_at.0,
            validated.updated_at.0,
            validated.algorithm_version,
            validated.redirect_subject_id.map(|id| id.to_string()),
        ],
    )
    .map_err(map_db_error("保存漫画进度主体失败"))?;

    conn.execute(
        "DELETE FROM comic_progress_subject_members WHERE subject_id = ?1",
        rusqlite::params![validated.id.to_string()],
    )
    .map_err(map_db_error("替换漫画进度主体成员失败"))?;
    for member in members {
        save_member_on_conn(conn, member)?;
    }
    Ok(())
}

fn ensure_subject_hierarchy(
    conn: &rusqlite::Connection,
    subject: &ComicProgressSubject,
    members: &[ComicProgressSubjectMember],
) -> Result<(), AppError> {
    let edition_matches_work: i64 = conn
        .query_row(
            "SELECT EXISTS(
                 SELECT 1 FROM editions
                 WHERE id = ?1 AND work_id = ?2
            )",
            rusqlite::params![subject.edition_id.to_string(), subject.work_id.to_string()],
            |row| row.get(0),
        )
        .map_err(map_db_error("校验漫画进度主体版本归属失败"))?;
    if edition_matches_work == 0 {
        return Err(invalid_subject("漫画进度主体的 Edition 不属于指定 Work"));
    }

    let canonical_matches_edition: i64 = conn
        .query_row(
            "SELECT EXISTS(
                 SELECT 1 FROM media_items
                 WHERE id = ?1 AND edition_id = ?2 AND media_type = 'comic'
             )",
            rusqlite::params![
                subject.canonical_media_item_id.to_string(),
                subject.edition_id.to_string()
            ],
            |row| row.get(0),
        )
        .map_err(map_db_error("校验漫画进度主体 canonical MediaItem 失败"))?;
    if canonical_matches_edition == 0 {
        return Err(invalid_subject(
            "漫画进度主体的 canonical MediaItem 不属于指定 Edition 或不是 Comic",
        ));
    }

    if let Some(authoritative) = subject.authoritative_progress_media_item_id {
        let authoritative_matches_work: i64 = conn
            .query_row(
                "SELECT EXISTS(
                     SELECT 1
                     FROM media_items m
                     JOIN editions e ON e.id = m.edition_id
                     WHERE m.id = ?1 AND m.media_type = 'comic' AND e.work_id = ?2
                 )",
                rusqlite::params![authoritative.to_string(), subject.work_id.to_string()],
                |row| row.get(0),
            )
            .map_err(map_db_error("校验漫画进度主体权威 MediaItem 失败"))?;
        if authoritative_matches_work == 0 {
            return Err(invalid_subject(
                "漫画进度主体的 authoritative MediaItem 不属于指定 Work 或不是 Comic",
            ));
        }
    }

    for member in members {
        let member_matches_work: i64 = conn
            .query_row(
                "SELECT EXISTS(
                     SELECT 1
                     FROM media_items m
                     JOIN editions e ON e.id = m.edition_id
                     WHERE m.id = ?1 AND m.media_type = 'comic' AND e.work_id = ?2
                 )",
                rusqlite::params![
                    member.media_item_id.to_string(),
                    subject.work_id.to_string()
                ],
                |row| row.get(0),
            )
            .map_err(map_db_error("校验漫画进度主体成员归属失败"))?;
        if member_matches_work == 0 {
            return Err(invalid_subject(
                "漫画进度主体成员必须属于相同 Work 的 Comic MediaItem",
            ));
        }
    }
    Ok(())
}

pub(crate) fn save_member_on_conn(
    conn: &rusqlite::Connection,
    member: &ComicProgressSubjectMember,
) -> Result<(), AppError> {
    member
        .validate()
        .map_err(|error| invalid_subject(error.to_string()))?;
    validate_subject_text(&member.algorithm_version, "algorithm_version")?;
    let evidence_json = serde_json::to_string(&member.evidence).map_err(|error| {
        AppError::new(
            "SERIALIZE_FAILED",
            ErrorKind::Validation,
            "漫画进度主体成员证据序列化失败",
            false,
        )
        .with_source(error)
    })?;
    validate_subject_text(&evidence_json, "evidence_json")?;
    conn.execute(
        "INSERT INTO comic_progress_subject_members
            (subject_id, media_item_id, relationship, confidence, evidence_json,
             state, algorithm_version, created_at, updated_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)
         ON CONFLICT(subject_id, media_item_id, state) DO UPDATE SET
             relationship = excluded.relationship,
             confidence = excluded.confidence,
             evidence_json = excluded.evidence_json,
             algorithm_version = excluded.algorithm_version,
             created_at = excluded.created_at,
             updated_at = excluded.updated_at",
        rusqlite::params![
            member.subject_id.to_string(),
            member.media_item_id.to_string(),
            enum_to_db_str(&member.relationship)?,
            enum_to_db_str(&member.confidence)?,
            evidence_json,
            enum_to_db_str(&member.state)?,
            member.algorithm_version,
            member.created_at.0,
            member.updated_at.0,
        ],
    )
    .map_err(map_db_error("保存漫画进度主体成员失败"))?;
    Ok(())
}

pub(crate) fn load_subject(
    conn: &rusqlite::Connection,
    id: ComicProgressSubjectId,
) -> Result<Option<ComicProgressSubject>, AppError> {
    let row = conn
        .query_row(
            "SELECT id, work_id, edition_id, canonical_media_item_id,
                    authoritative_progress_media_item_id, state, created_at, updated_at,
                    algorithm_version, redirect_subject_id
             FROM comic_progress_subjects WHERE id = ?1",
            rusqlite::params![id.to_string()],
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, String>(3)?,
                    row.get::<_, Option<String>>(4)?,
                    row.get::<_, String>(5)?,
                    row.get::<_, i64>(6)?,
                    row.get::<_, i64>(7)?,
                    row.get::<_, String>(8)?,
                    row.get::<_, Option<String>>(9)?,
                ))
            },
        )
        .optional()
        .map_err(map_db_error("查询漫画进度主体失败"))?;
    let Some((
        id,
        work_id,
        edition_id,
        canonical,
        authoritative,
        state,
        created,
        updated,
        version,
        redirect,
    )) = row
    else {
        return Ok(None);
    };
    let parsed_id = parse_id::<ComicProgressSubjectId>(id, "id")
        .map_err(|error| invalid_subject(error.to_string()))?;
    let parsed_work_id = parse_id::<WorkId>(work_id, "work_id")
        .map_err(|error| invalid_subject(error.to_string()))?;
    let parsed_edition_id = parse_id::<EditionId>(edition_id, "edition_id")
        .map_err(|error| invalid_subject(error.to_string()))?;
    let parsed_canonical = parse_id::<MediaItemId>(canonical, "canonical_media_item_id")
        .map_err(|error| invalid_subject(error.to_string()))?;
    let parsed_authoritative = authoritative
        .map(|value| parse_id::<MediaItemId>(value, "authoritative_progress_media_item_id"))
        .transpose()
        .map_err(|error| invalid_subject(error.to_string()))?;
    let parsed_redirect = redirect
        .map(|value| parse_id::<ComicProgressSubjectId>(value, "redirect_subject_id"))
        .transpose()
        .map_err(|error| invalid_subject(error.to_string()))?;
    validate_subject_text(&version, "algorithm_version")?;
    let members = load_members(conn, &parsed_id.to_string())?;
    let value = serde_json::json!({
        "id": parsed_id,
        "work_id": parsed_work_id,
        "edition_id": parsed_edition_id,
        "canonical_media_item_id": parsed_canonical,
        "authoritative_progress_media_item_id": parsed_authoritative,
        "state": state,
        "created_at": UtcMillis(created),
        "updated_at": UtcMillis(updated),
        "algorithm_version": version,
        "redirect_subject_id": parsed_redirect,
        "members": members,
    });
    let subject: ComicProgressSubject =
        serde_json::from_value(value).map_err(|error| invalid_subject(error.to_string()))?;
    ensure_subject_hierarchy(conn, &subject, subject.members())?;
    Ok(Some(subject))
}

pub(crate) fn load_members(
    conn: &rusqlite::Connection,
    subject_id: &str,
) -> Result<Vec<ComicProgressSubjectMember>, AppError> {
    let mut stmt = conn
        .prepare(
            "SELECT subject_id, media_item_id, relationship, confidence, evidence_json,
                    state, algorithm_version, created_at, updated_at
             FROM comic_progress_subject_members
             WHERE subject_id = ?1 ORDER BY created_at, media_item_id, state",
        )
        .map_err(map_db_error("查询漫画进度主体成员失败"))?;
    let rows = stmt
        .query_map(rusqlite::params![subject_id], |row| {
            let evidence_json: String = row.get(4)?;
            validate_subject_text(&evidence_json, "evidence_json")
                .map_err(|_| invalid_row("evidence_json"))?;
            let evidence =
                serde_json::from_str(&evidence_json).map_err(|_| invalid_row("evidence_json"))?;
            let algorithm_version: String = row.get(6)?;
            validate_subject_text(&algorithm_version, "algorithm_version")
                .map_err(|_| invalid_row("algorithm_version"))?;
            Ok(ComicProgressSubjectMember {
                subject_id: parse_id(row.get(0)?, "subject_id")?,
                media_item_id: parse_id(row.get(1)?, "media_item_id")?,
                relationship: parse_db_enum(row.get(2)?, "relationship")?,
                confidence: parse_db_enum(row.get(3)?, "confidence")?,
                evidence,
                state: parse_db_enum(row.get(5)?, "state")?,
                algorithm_version,
                created_at: UtcMillis(row.get(7)?),
                updated_at: UtcMillis(row.get(8)?),
            })
        })
        .map_err(map_db_error("查询漫画进度主体成员失败"))?;
    rows.collect::<rusqlite::Result<Vec<_>>>()
        .map_err(map_db_error("查询漫画进度主体成员失败"))
}

fn load_member_for_media_item(
    conn: &rusqlite::Connection,
    media_item_id: MediaItemId,
) -> Result<Option<ComicProgressSubjectMember>, AppError> {
    let subject_id = conn
        .query_row(
            "SELECT subject_id FROM comic_progress_subject_members
             WHERE media_item_id = ?1 AND state = 'active'",
            rusqlite::params![media_item_id.to_string()],
            |row| row.get::<_, String>(0),
        )
        .optional()
        .map_err(map_db_error("查询漫画进度主体成员失败"))?;
    let Some(subject_id) = subject_id else {
        return Ok(None);
    };
    Ok(load_members(conn, &subject_id)?.into_iter().find(|member| {
        member.media_item_id == media_item_id
            && member.state
                == haven_domain::comic_progress_subject::ComicProgressSubjectMemberState::Active
    }))
}

fn validate_subject_text(value: &str, field: &'static str) -> Result<(), AppError> {
    if value.trim().is_empty()
        || value.len() > 4096
        || value
            .chars()
            .any(|ch| (ch as u32) <= 0x1f || ch == '\u{7f}')
        || value.contains("://")
        || value.to_ascii_lowercase().starts_with("data:")
    {
        return Err(AppError::new(
            "INVALID_COMIC_PROGRESS_SUBJECT",
            ErrorKind::Validation,
            format!("漫画进度主体字段 {field} 非法"),
            false,
        ));
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

fn invalid_row(field: &'static str) -> rusqlite::Error {
    rusqlite::Error::FromSqlConversionFailure(
        0,
        rusqlite::types::Type::Text,
        Box::new(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            format!("无法解析漫画进度主体字段 {field}"),
        )),
    )
}

fn invalid_subject(message: impl Into<String>) -> AppError {
    AppError::new(
        "INVALID_COMIC_PROGRESS_SUBJECT",
        ErrorKind::Validation,
        message,
        false,
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::repos::SqliteRepositories;
    use crate::db::uow::SqliteUnitOfWork;
    use haven_application::services::ports::{
        ComicProgressSubjectWritePlan, ComicProgressSubjectWritePrecondition,
        ComicProgressSubjectWriteResult, FavoriteTxPorts, UnitOfWork,
    };
    use haven_application::services::progress::comic_progress_subject::ComicProgressSubjectService;
    use haven_domain::comic_identity::ChapterEvidence;
    use haven_domain::comic_progress_subject::{
        ComicProgressSubject, ComicProgressSubjectMember, ComicProgressSubjectRelationship,
        ComicProgressSubjectState,
    };
    use haven_domain::contracts::{
        ComicProgressSubjectRepository, EditionRepository, HistoryRepository, MarkerRepository,
        MediaItemRepository, ProgressRepository, WorkRepository,
    };
    use haven_domain::entities::{
        Edition, HistoryEntry, Marker, MediaIndex, MediaItem, Progress, Resource, Work,
    };
    use haven_domain::enums::{
        CompletionState, MarkerType, MediaItemStatus, MediaType, WorkStatus, WorkType,
    };
    use haven_domain::ids::{HistoryEntryId, MarkerId, ProgressId};
    use haven_domain::locator::{ComicLocator, Locator};
    use std::sync::Mutex;
    use std::sync::atomic::{AtomicUsize, Ordering};

    fn seed(db: &Db) -> (WorkId, EditionId, MediaItemId) {
        let work_id = WorkId::new();
        let edition_id = EditionId::new();
        let media_item_id = MediaItemId::new();
        let conn = db.lock();
        conn.execute(
            "INSERT INTO works (id, canonical_title, work_type, status, created_at, updated_at)
             VALUES (?1, '主体作品', 'fiction', 'completed', 1, 1)",
            rusqlite::params![work_id.to_string()],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO editions (id, work_id, title, edition_type, created_at, updated_at)
             VALUES (?1, ?2, '漫画版', 'comic', 1, 1)",
            rusqlite::params![edition_id.to_string(), work_id.to_string()],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO media_items
                (id, edition_id, media_type, title, category, status, created_at, updated_at)
             VALUES (?1, ?2, 'comic', '第 1 话', 'comic', 'available', 1, 1)",
            rusqlite::params![media_item_id.to_string(), edition_id.to_string()],
        )
        .unwrap();
        (work_id, edition_id, media_item_id)
    }

    async fn seed_real_comic_content(
        repos: &SqliteRepositories,
        title: &str,
        media_item_id: MediaItemId,
    ) -> (WorkId, EditionId) {
        let now = UtcMillis(1);
        let work_id = WorkId::new();
        let edition_id = EditionId::new();
        WorkRepository::save(
            repos,
            &Work {
                id: work_id,
                canonical_title: title.to_owned(),
                original_title: None,
                sort_title: None,
                description: None,
                work_type: WorkType::Fiction,
                release_year: None,
                language: None,
                director: None,
                actor: None,
                status: WorkStatus::Completed,
                rating_value: None,
                rating_scale: None,
                artwork: Default::default(),
                created_at: now,
                updated_at: now,
            },
        )
        .await
        .unwrap();
        EditionRepository::save(
            repos,
            &Edition {
                id: edition_id,
                work_id,
                title: format!("{title} 漫画版"),
                subtitle: None,
                edition_type: MediaType::Comic,
                release_date: None,
                language: None,
                region: None,
                publisher_or_studio: None,
                description: None,
                artwork: Default::default(),
                created_at: now,
                updated_at: now,
            },
        )
        .await
        .unwrap();
        MediaItemRepository::save(
            repos,
            &MediaItem {
                id: media_item_id,
                edition_id,
                parent_id: None,
                media_type: MediaType::Comic,
                title: "第 1 话".to_owned(),
                index: MediaIndex::Chapter {
                    volume: None,
                    chapter: 1.0,
                },
                duration_ms: None,
                page_count: Some(10),
                chapter_count: None,
                published_at: None,
                status: MediaItemStatus::Available,
                created_at: now,
                updated_at: now,
            },
        )
        .await
        .unwrap();
        (work_id, edition_id)
    }

    fn comic_progress(
        work_id: WorkId,
        edition_id: EditionId,
        media_item_id: MediaItemId,
        at: i64,
    ) -> Progress {
        Progress {
            id: ProgressId::new(),
            work_id,
            edition_id,
            media_item_id,
            locator: Locator::Comic(ComicLocator {
                chapter_item_id: media_item_id,
                page_index: 3,
                page_progression: Some(0.4),
            }),
            completion: CompletionState::InProgress,
            percentage: Some(0.4),
            last_active_at: UtcMillis(at),
            updated_at: UtcMillis(at),
            revision: None,
            keyframe_uri: Some("data:image/png;base64,fixture".to_owned()),
        }
    }

    /// 在既有 Edition 下追加一个 Comic MediaItem（CAS / 多成员测试复用）。
    async fn add_comic_media_item(
        repos: &SqliteRepositories,
        edition_id: EditionId,
        chapter: f32,
    ) -> MediaItemId {
        let media_item_id = MediaItemId::new();
        MediaItemRepository::save(
            repos,
            &MediaItem {
                id: media_item_id,
                edition_id,
                parent_id: None,
                media_type: MediaType::Comic,
                title: format!("第 {chapter} 话"),
                index: MediaIndex::Chapter {
                    volume: None,
                    chapter,
                },
                duration_ms: None,
                page_count: Some(10),
                chapter_count: None,
                published_at: None,
                status: MediaItemStatus::Available,
                created_at: UtcMillis(1),
                updated_at: UtcMillis(1),
            },
        )
        .await
        .unwrap();
        media_item_id
    }

    /// 构造一条合法 active 成员（relationship 非 candidate、confidence=high、
    /// 证据支持高置信度），`at` 同时作为 created_at/updated_at 保证读取顺序确定。
    fn active_subject_member(
        subject_id: ComicProgressSubjectId,
        media_item_id: MediaItemId,
        relationship: ComicProgressSubjectRelationship,
        at: i64,
    ) -> ComicProgressSubjectMember {
        let mut member = ComicProgressSubjectMember::active(
            subject_id,
            media_item_id,
            vec![ChapterEvidence::SameRemoteIdentity],
        );
        member.relationship = relationship;
        member.created_at = UtcMillis(at);
        member.updated_at = UtcMillis(at);
        member
    }

    fn subject_write_plan(
        subject: ComicProgressSubject,
        members: Vec<ComicProgressSubjectMember>,
    ) -> haven_application::services::ports::ComicProgressSubjectWritePlan {
        haven_application::services::ports::ComicProgressSubjectWritePlan {
            subject,
            members,
            page_identity_write: None,
            progress_writes: vec![],
            migration_snapshot: None,
            refresh_receipt: None,
        }
    }

    fn table_exists(conn: &rusqlite::Connection, name: &str) -> bool {
        conn.query_row(
            "SELECT EXISTS(
                 SELECT 1 FROM sqlite_master WHERE type = 'table' AND name = ?1
             )",
            rusqlite::params![name],
            |row| row.get::<_, i64>(0),
        )
        .unwrap()
            != 0
    }

    fn index_exists(conn: &rusqlite::Connection, name: &str) -> bool {
        conn.query_row(
            "SELECT EXISTS(
                 SELECT 1 FROM sqlite_master WHERE type = 'index' AND name = ?1
             )",
            rusqlite::params![name],
            |row| row.get::<_, i64>(0),
        )
        .unwrap()
            != 0
    }

    #[tokio::test]
    async fn progress_subject_backfill_existing_comic_progress_is_backfilled_without_copying_progress_fields()
     {
        use crate::db::uow::SqliteUnitOfWork;
        use haven_application::services::progress::ProgressService;

        let db = Arc::new(Db::open_in_memory().unwrap());
        let repos = Arc::new(SqliteRepositories::new(db.clone()));
        let media_item_id = MediaItemId::new();
        let (work_id, edition_id) =
            seed_real_comic_content(&repos, "回填漫画", media_item_id).await;
        let persisted_revision = ProgressRepository::save_if_revision(
            &*repos,
            &comic_progress(work_id, edition_id, media_item_id, 100),
            None,
        )
        .await
        .unwrap()
        .expect("Progress 应取得 revision");
        let marker = Marker {
            id: MarkerId::new(),
            work_id,
            edition_id,
            media_item_id,
            locator: Locator::Comic(ComicLocator {
                chapter_item_id: media_item_id,
                page_index: 3,
                page_progression: Some(0.4),
            }),
            marker_type: MarkerType::Bookmark,
            title: Some("保留标记".to_owned()),
            excerpt: None,
            note: None,
            preview: None,
            created_at: UtcMillis(101),
            updated_at: UtcMillis(101),
            deleted_at: None,
        };
        MarkerRepository::save(&*repos, &marker).await.unwrap();
        let history = HistoryEntry {
            id: HistoryEntryId::new(),
            media_item_id,
            work_id,
            edition_id,
            locator: Some(marker.locator.clone()),
            started_at: UtcMillis(99),
            last_active_at: UtcMillis(100),
            completed_at: None,
        };
        HistoryRepository::save(&*repos, &history).await.unwrap();

        let service = ProgressService::with_comic_progress_subjects(
            repos.clone(),
            Arc::new(SqliteUnitOfWork::new(db)),
        );
        let resolved = service
            .read_for_media_item(media_item_id)
            .await
            .unwrap()
            .expect("既有 Progress 必须仍可读");
        assert_eq!(
            resolved.revision.as_deref(),
            Some(persisted_revision.as_str())
        );
        assert_eq!(resolved.locator, marker.locator);
        let member = ComicProgressSubjectRepository::get_for_media_item(&*repos, media_item_id)
            .await
            .unwrap()
            .expect("首次读取必须创建 canonical member");
        let subject = ComicProgressSubjectRepository::get(&*repos, member.subject_id)
            .await
            .unwrap()
            .expect("首次读取必须创建 Subject");
        assert_eq!(
            subject.authoritative_progress_media_item_id,
            Some(media_item_id)
        );
        assert_eq!(
            ProgressRepository::get_for_media_item(&*repos, media_item_id)
                .await
                .unwrap()
                .unwrap()
                .revision
                .as_deref(),
            Some(persisted_revision.as_str()),
            "Subject 回填不得改变 Progress revision"
        );
        assert_eq!(
            HistoryRepository::list_for_media_item(&*repos, media_item_id)
                .await
                .unwrap()
                .len(),
            1
        );
        assert_eq!(
            MarkerRepository::list_for_media_item(&*repos, media_item_id)
                .await
                .unwrap()
                .len(),
            1
        );
    }

    #[tokio::test]
    async fn progress_subject_backfill_unread_comic_creates_one_unpointed_subject_and_remains_readable()
     {
        use crate::db::uow::SqliteUnitOfWork;
        use haven_application::services::progress::ProgressService;

        let db = Arc::new(Db::open_in_memory().unwrap());
        let repos = Arc::new(SqliteRepositories::new(db.clone()));
        let media_item_id = MediaItemId::new();
        seed_real_comic_content(&repos, "未开始漫画", media_item_id).await;
        let service = ProgressService::with_comic_progress_subjects(
            repos.clone(),
            Arc::new(SqliteUnitOfWork::new(db.clone())),
        );

        assert!(
            service
                .read_for_media_item(media_item_id)
                .await
                .unwrap()
                .is_none(),
            "首次未开始读取必须是空进度，不得因悬空 pointer 失败"
        );
        assert!(
            service
                .read_for_media_item(media_item_id)
                .await
                .unwrap()
                .is_none(),
            "重复读取必须继续返回空进度"
        );
        let member = ComicProgressSubjectRepository::get_for_media_item(&*repos, media_item_id)
            .await
            .unwrap()
            .expect("首次读取必须创建 active canonical member");
        let subject = ComicProgressSubjectRepository::get(&*repos, member.subject_id)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(subject.authoritative_progress_media_item_id, None);
        let conn = db.lock();
        let subject_count: i64 = conn
            .query_row("SELECT COUNT(*) FROM comic_progress_subjects", [], |row| {
                row.get(0)
            })
            .unwrap();
        let active_member_count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM comic_progress_subject_members WHERE state = 'active'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        let progress_count: i64 = conn
            .query_row("SELECT COUNT(*) FROM progress", [], |row| row.get(0))
            .unwrap();
        assert_eq!(subject_count, 1);
        assert_eq!(active_member_count, 1);
        assert_eq!(progress_count, 0);
    }

    #[tokio::test]
    async fn progress_subject_backfill_multiple_active_member_progress_uses_last_active_then_media_id_and_keeps_other_rows()
     {
        use crate::db::uow::SqliteUnitOfWork;
        use haven_application::services::ports::{ComicProgressSubjectWritePlan, UnitOfWork};
        use haven_application::services::progress::ProgressService;

        let db = Arc::new(Db::open_in_memory().unwrap());
        let repos = Arc::new(SqliteRepositories::new(db.clone()));
        let first_media_item_id = MediaItemId::new();
        let second_media_item_id = MediaItemId::new();
        let (work_id, edition_id) =
            seed_real_comic_content(&repos, "权威选择漫画", first_media_item_id).await;
        MediaItemRepository::save(
            &*repos,
            &MediaItem {
                id: second_media_item_id,
                edition_id,
                parent_id: None,
                media_type: MediaType::Comic,
                title: "第 2 话".to_owned(),
                index: MediaIndex::Chapter {
                    volume: None,
                    chapter: 2.0,
                },
                duration_ms: None,
                page_count: Some(10),
                chapter_count: None,
                published_at: None,
                status: MediaItemStatus::Available,
                created_at: UtcMillis(1),
                updated_at: UtcMillis(1),
            },
        )
        .await
        .unwrap();
        for (media_item_id, at) in [(first_media_item_id, 200), (second_media_item_id, 100)] {
            ProgressRepository::save_if_revision(
                &*repos,
                &comic_progress(work_id, edition_id, media_item_id, at),
                None,
            )
            .await
            .unwrap()
            .expect("fixture Progress");
        }
        let mut subject =
            ComicProgressSubject::new(work_id, edition_id, first_media_item_id, UtcMillis(1));
        subject.authoritative_progress_media_item_id = None;
        let mut first_member = ComicProgressSubjectMember::active(
            subject.id,
            first_media_item_id,
            vec![ChapterEvidence::SameRemoteIdentity],
        );
        first_member.relationship = ComicProgressSubjectRelationship::Canonical;
        let second_member = ComicProgressSubjectMember::active(
            subject.id,
            second_media_item_id,
            vec![ChapterEvidence::SameRemoteIdentity],
        );
        SqliteUnitOfWork::new(db.clone())
            .run_comic_progress_subject_write(&ComicProgressSubjectWritePlan {
                subject,
                members: vec![first_member, second_member],
                page_identity_write: None,
                progress_writes: vec![],
                migration_snapshot: None,
                refresh_receipt: None,
            })
            .unwrap();
        let service = ProgressService::with_comic_progress_subjects(
            repos.clone(),
            Arc::new(SqliteUnitOfWork::new(db.clone())),
        );
        service
            .read_for_media_item(second_media_item_id)
            .await
            .unwrap();
        let member =
            ComicProgressSubjectRepository::get_for_media_item(&*repos, first_media_item_id)
                .await
                .unwrap()
                .unwrap();
        let subject = ComicProgressSubjectRepository::get(&*repos, member.subject_id)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(
            subject.authoritative_progress_media_item_id,
            Some(first_media_item_id)
        );
        assert!(
            ProgressRepository::get_for_media_item(&*repos, first_media_item_id)
                .await
                .unwrap()
                .is_some()
        );
        assert!(
            ProgressRepository::get_for_media_item(&*repos, second_media_item_id)
                .await
                .unwrap()
                .is_some()
        );

        // A persisted pointer remains authoritative even if another active
        // member has a newer Progress and the requested member has none.
        let third_media_item_id = MediaItemId::new();
        MediaItemRepository::save(
            &*repos,
            &MediaItem {
                id: third_media_item_id,
                edition_id,
                parent_id: None,
                media_type: MediaType::Comic,
                title: "第 3 话".to_owned(),
                index: MediaIndex::Chapter {
                    volume: None,
                    chapter: 3.0,
                },
                duration_ms: None,
                page_count: Some(10),
                chapter_count: None,
                published_at: None,
                status: MediaItemStatus::Available,
                created_at: UtcMillis(1),
                updated_at: UtcMillis(1),
            },
        )
        .await
        .unwrap();
        let third_member = ComicProgressSubjectMember::active(
            subject.id,
            third_media_item_id,
            vec![ChapterEvidence::SameRemoteIdentity],
        );
        let mut pointer_subject = subject.clone();
        pointer_subject.authoritative_progress_media_item_id = Some(second_media_item_id);
        let mut pointer_members =
            ComicProgressSubjectRepository::list_members(&*repos, member.subject_id)
                .await
                .unwrap();
        pointer_members.push(third_member);
        SqliteUnitOfWork::new(db.clone())
            .run_comic_progress_subject_write(&ComicProgressSubjectWritePlan {
                subject: pointer_subject,
                members: pointer_members,
                page_identity_write: None,
                progress_writes: vec![],
                migration_snapshot: None,
                refresh_receipt: None,
            })
            .unwrap();
        let pointer_resolution = service
            .read_for_media_item(third_media_item_id)
            .await
            .unwrap()
            .expect("目标无 Progress 时必须返回已有权威指针行");
        assert_eq!(
            pointer_resolution.media_item_id, second_media_item_id,
            "不得用较新的非 pointer Progress 替换既有权威映射"
        );
        let target_resolution = service
            .read_for_media_item(first_media_item_id)
            .await
            .unwrap()
            .expect("目标已有 Progress 时必须保留目标当前视角");
        assert_eq!(
            target_resolution.media_item_id, first_media_item_id,
            "目标自身 Progress 不得被 Subject pointer 覆盖"
        );
        let mut dangling_pointer_subject = subject.clone();
        dangling_pointer_subject.authoritative_progress_media_item_id = Some(third_media_item_id);
        SqliteUnitOfWork::new(db.clone())
            .run_comic_progress_subject_write(&ComicProgressSubjectWritePlan {
                subject: dangling_pointer_subject,
                members: ComicProgressSubjectRepository::list_members(&*repos, member.subject_id)
                    .await
                    .unwrap(),
                page_identity_write: None,
                progress_writes: vec![],
                migration_snapshot: None,
                refresh_receipt: None,
            })
            .unwrap();
        let err = service
            .read_for_media_item(first_media_item_id)
            .await
            .expect_err("目标自身 Progress 不能掩盖悬空 authoritative pointer");
        assert_eq!(
            err.code().as_str(),
            "COMIC_PROGRESS_SUBJECT_AUTHORITATIVE_PROGRESS_MISSING"
        );

        // Equal timestamps use only MediaItem UUID text as the deterministic tie-breaker.
        let tied_media_item_id =
            if first_media_item_id.to_string() < second_media_item_id.to_string() {
                first_media_item_id
            } else {
                second_media_item_id
            };
        let mut tied_subject = subject.clone();
        tied_subject.authoritative_progress_media_item_id = None;
        let mut left = ProgressRepository::get_for_media_item(&*repos, first_media_item_id)
            .await
            .unwrap()
            .unwrap();
        let mut right = ProgressRepository::get_for_media_item(&*repos, second_media_item_id)
            .await
            .unwrap()
            .unwrap();
        left.last_active_at = UtcMillis(300);
        right.last_active_at = UtcMillis(300);
        left.updated_at = UtcMillis(301);
        right.updated_at = UtcMillis(301);
        ProgressRepository::save(&*repos, &left).await.unwrap();
        ProgressRepository::save(&*repos, &right).await.unwrap();
        SqliteUnitOfWork::new(db.clone())
            .run_comic_progress_subject_write(&ComicProgressSubjectWritePlan {
                subject: tied_subject,
                members: ComicProgressSubjectRepository::list_members(&*repos, member.subject_id)
                    .await
                    .unwrap(),
                page_identity_write: None,
                progress_writes: vec![],
                migration_snapshot: None,
                refresh_receipt: None,
            })
            .unwrap();
        service
            .read_for_media_item(second_media_item_id)
            .await
            .unwrap();
        let tied = ComicProgressSubjectRepository::get(&*repos, member.subject_id)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(
            tied.authoritative_progress_media_item_id,
            Some(tied_media_item_id)
        );
    }

    #[tokio::test]
    async fn progress_subject_backfill_get_for_media_item_prefers_active_member_over_older_candidate()
     {
        use crate::db::uow::SqliteUnitOfWork;
        use haven_application::services::ports::{ComicProgressSubjectWritePlan, UnitOfWork};

        let db = Arc::new(Db::open_in_memory().unwrap());
        let repos = Arc::new(SqliteRepositories::new(db.clone()));
        let media_item_id = MediaItemId::new();
        let (work_id, edition_id) =
            seed_real_comic_content(&repos, "成员状态漫画", media_item_id).await;
        let subject = ComicProgressSubject::new(work_id, edition_id, media_item_id, UtcMillis(1));
        let mut candidate = ComicProgressSubjectMember::candidate(
            subject.id,
            media_item_id,
            vec![ChapterEvidence::WeakChapterMetadata],
        );
        candidate.created_at = UtcMillis(1);
        candidate.updated_at = UtcMillis(1);
        let mut active = ComicProgressSubjectMember::active(
            subject.id,
            media_item_id,
            vec![ChapterEvidence::SameRemoteIdentity],
        );
        active.relationship = ComicProgressSubjectRelationship::Canonical;
        active.created_at = UtcMillis(2);
        active.updated_at = UtcMillis(2);
        SqliteUnitOfWork::new(db)
            .run_comic_progress_subject_write(&ComicProgressSubjectWritePlan {
                subject,
                members: vec![candidate, active.clone()],
                page_identity_write: None,
                progress_writes: vec![],
                migration_snapshot: None,
                refresh_receipt: None,
            })
            .unwrap();
        let resolved = ComicProgressSubjectRepository::get_for_media_item(&*repos, media_item_id)
            .await
            .unwrap()
            .expect("存在 active 行时必须返回 active member");
        assert_eq!(resolved, active);
    }

    #[tokio::test]
    async fn progress_subject_backfill_non_comic_progress_never_creates_subject_or_changes_revision()
     {
        use crate::db::uow::SqliteUnitOfWork;
        use haven_application::services::progress::ProgressService;
        use haven_domain::locator::{Locator, VideoLocator};

        let db = Arc::new(Db::open_in_memory().unwrap());
        let repos = Arc::new(SqliteRepositories::new(db.clone()));
        let now = UtcMillis(1);
        let work_id = WorkId::new();
        let edition_id = EditionId::new();
        let media_item_id = MediaItemId::new();
        WorkRepository::save(
            &*repos,
            &Work {
                id: work_id,
                canonical_title: "非漫画影片".to_owned(),
                original_title: None,
                sort_title: None,
                description: None,
                work_type: WorkType::Standalone,
                release_year: None,
                language: None,
                director: None,
                actor: None,
                status: WorkStatus::Completed,
                rating_value: None,
                rating_scale: None,
                artwork: Default::default(),
                created_at: now,
                updated_at: now,
            },
        )
        .await
        .unwrap();
        EditionRepository::save(
            &*repos,
            &Edition {
                id: edition_id,
                work_id,
                title: "影片版".to_owned(),
                subtitle: None,
                edition_type: MediaType::Movie,
                release_date: None,
                language: None,
                region: None,
                publisher_or_studio: None,
                description: None,
                artwork: Default::default(),
                created_at: now,
                updated_at: now,
            },
        )
        .await
        .unwrap();
        MediaItemRepository::save(
            &*repos,
            &MediaItem {
                id: media_item_id,
                edition_id,
                parent_id: None,
                media_type: MediaType::Movie,
                title: "正片".to_owned(),
                index: MediaIndex::Movie,
                duration_ms: Some(1_000),
                page_count: None,
                chapter_count: None,
                published_at: None,
                status: MediaItemStatus::Available,
                created_at: now,
                updated_at: now,
            },
        )
        .await
        .unwrap();
        let movie_progress = Progress {
            id: ProgressId::new(),
            work_id,
            edition_id,
            media_item_id,
            locator: Locator::Video(VideoLocator {
                media_item_id,
                position_ms: 500,
            }),
            completion: CompletionState::InProgress,
            percentage: Some(0.5),
            last_active_at: now,
            updated_at: now,
            revision: None,
            keyframe_uri: None,
        };
        let revision = ProgressRepository::save_if_revision(&*repos, &movie_progress, None)
            .await
            .unwrap()
            .unwrap();
        let service = ProgressService::with_comic_progress_subjects(
            repos.clone(),
            Arc::new(SqliteUnitOfWork::new(db.clone())),
        );
        assert_eq!(
            service
                .read_for_media_item(media_item_id)
                .await
                .unwrap()
                .unwrap()
                .revision
                .as_deref(),
            Some(revision.as_str())
        );
        let subject_count: i64 = db
            .lock()
            .query_row("SELECT COUNT(*) FROM comic_progress_subjects", [], |row| {
                row.get(0)
            })
            .unwrap();
        assert_eq!(subject_count, 0);
    }

    #[test]
    fn migration_comic_progress_subjects_create_schema_and_active_member_index() {
        let db = Db::open_in_memory().unwrap();
        let (work, edition, media) = seed(&db);
        let conn = db.lock();

        assert!(table_exists(&conn, "comic_progress_subjects"));
        assert!(table_exists(&conn, "comic_progress_subject_members"));
        assert!(table_exists(&conn, "comic_catalog_refresh_outcomes"));
        assert!(index_exists(
            &conn,
            "uq_comic_progress_subject_members_active_media_item"
        ));

        let versions: Vec<String> = conn
            .prepare(
                "SELECT version FROM schema_migrations
                 WHERE version IN (
                     '037_comic_progress_subjects',
                     '038_comic_progress_subject_members',
                     '039_comic_catalog_refresh_outcomes'
                 )
                 ORDER BY rowid",
            )
            .unwrap()
            .query_map([], |row| row.get(0))
            .unwrap()
            .collect::<rusqlite::Result<_>>()
            .unwrap();
        assert_eq!(
            versions,
            vec![
                "037_comic_progress_subjects",
                "038_comic_progress_subject_members",
                "039_comic_catalog_refresh_outcomes",
            ]
        );

        let subject_a = ComicProgressSubjectId::new();
        let subject_b = ComicProgressSubjectId::new();
        for subject_id in [subject_a, subject_b] {
            conn.execute(
                "INSERT INTO comic_progress_subjects
                    (id, work_id, edition_id, canonical_media_item_id,
                     authoritative_progress_media_item_id, state, created_at, updated_at,
                     algorithm_version, redirect_subject_id)
                 VALUES (?1, ?2, ?3, ?4, NULL, 'active', 1, 1, 'test-v1', NULL)",
                rusqlite::params![
                    subject_id.to_string(),
                    work.to_string(),
                    edition.to_string(),
                    media.to_string()
                ],
            )
            .unwrap();
        }
        conn.execute(
            "INSERT INTO comic_progress_subject_members
                (subject_id, media_item_id, relationship, confidence, evidence_json,
                 state, algorithm_version, created_at, updated_at)
             VALUES (?1, ?2, 'canonical', 'high', '[]', 'active', 'test-v1', 1, 1)",
            rusqlite::params![subject_a.to_string(), media.to_string()],
        )
        .unwrap();
        let duplicate_active = conn.execute(
            "INSERT INTO comic_progress_subject_members
                (subject_id, media_item_id, relationship, confidence, evidence_json,
                 state, algorithm_version, created_at, updated_at)
             VALUES (?1, ?2, 'equivalent', 'medium', '[]', 'active', 'test-v1', 1, 1)",
            rusqlite::params![subject_b.to_string(), media.to_string()],
        );
        assert!(duplicate_active.is_err(), "active media_item 必须保持唯一");
    }

    #[tokio::test]
    async fn comic_progress_subjects_roundtrip_without_progress_fields() {
        let db = Arc::new(Db::open_in_memory().unwrap());
        let (work, edition, media) = seed(&db);
        let repo = SqliteComicProgressSubjectRepository::new(db.clone());
        let subject = ComicProgressSubject::new(work, edition, media, UtcMillis(1));
        repo.save(&subject).await.unwrap();
        let restored = repo.get(subject.id).await.unwrap().unwrap();
        assert_eq!(restored.id, subject.id);
        assert_eq!(restored.members(), &[]);
        let columns: Vec<String> = db
            .lock()
            .prepare("PRAGMA table_info(comic_progress_subjects)")
            .unwrap()
            .query_map([], |row| row.get(1))
            .unwrap()
            .collect::<rusqlite::Result<_>>()
            .unwrap();
        for forbidden in [
            "page_index",
            "page_progression",
            "completion",
            "percentage",
            "keyframe_uri",
            "revision",
        ] {
            assert!(!columns.iter().any(|column| column == forbidden));
        }
    }

    #[tokio::test]
    async fn comic_progress_subjects_reject_cross_work_edition_chain_without_partial_write() {
        let db = Arc::new(Db::open_in_memory().unwrap());
        let (work_a, _, _) = seed(&db);
        let (work_b, edition_b, media_b) = seed(&db);
        let repo = SqliteComicProgressSubjectRepository::new(db.clone());
        let subject = ComicProgressSubject::new(work_a, edition_b, media_b, UtcMillis(1));

        let error = repo.save(&subject).await.unwrap_err();
        assert_eq!(error.kind(), ErrorKind::Validation);
        let conn = db.lock();
        let subjects: i64 = conn
            .query_row("SELECT COUNT(*) FROM comic_progress_subjects", [], |row| {
                row.get(0)
            })
            .unwrap();
        assert_eq!(subjects, 0);
        assert_ne!(work_a, work_b);
    }

    #[tokio::test]
    async fn comic_progress_subjects_reject_cross_work_member_without_partial_write() {
        let db = Arc::new(Db::open_in_memory().unwrap());
        let (work_a, edition_a, media_a) = seed(&db);
        let (work_b, _, media_b) = seed(&db);
        let repo = SqliteComicProgressSubjectRepository::new(db.clone());
        let mut subject = ComicProgressSubject::new(work_a, edition_a, media_a, UtcMillis(1));
        subject
            .attach_member(ComicProgressSubjectMember::candidate(
                subject.id,
                media_b,
                vec![ChapterEvidence::EditionCompatible],
            ))
            .unwrap();

        let error = repo.save(&subject).await.unwrap_err();
        assert_eq!(error.kind(), ErrorKind::Validation);
        let conn = db.lock();
        for table in ["comic_progress_subjects", "comic_progress_subject_members"] {
            let count: i64 = conn
                .query_row(&format!("SELECT COUNT(*) FROM {table}"), [], |row| {
                    row.get(0)
                })
                .unwrap();
            assert_eq!(count, 0, "跨 Work 成员失败后 {table} 不得有部分写入");
        }
        assert_ne!(work_a, work_b);
    }

    /// `save_member` 必须在同一 Immediate 事务里加载 Subject + 全部成员、合并
    /// 待写成员、整体校验并写入。跨 Work 成员在 `ensure_subject_hierarchy` 被
    /// 拒绝：整笔回滚，原 Subject 仍可读，且不留下任何成员行。
    #[tokio::test]
    async fn comic_progress_subject_save_member_rejects_cross_work_member_without_partial_write() {
        let db = Arc::new(Db::open_in_memory().unwrap());
        let (work_a, edition_a, media_a) = seed(&db);
        let (work_b, _, media_b) = seed(&db);
        let repos = Arc::new(SqliteRepositories::new(db.clone()));
        let repo = SqliteComicProgressSubjectRepository::new(db.clone());
        let mut subject = ComicProgressSubject::new(work_a, edition_a, media_a, UtcMillis(1));
        subject.authoritative_progress_media_item_id = None;
        repo.save(&subject).await.unwrap();

        let error = repo
            .save_member(&active_subject_member(
                subject.id,
                media_b,
                ComicProgressSubjectRelationship::Equivalent,
                1,
            ))
            .await
            .unwrap_err();
        assert_eq!(error.kind(), ErrorKind::Validation);
        assert_ne!(work_a, work_b);
        assert_eq!(
            repo.get(subject.id).await.unwrap().unwrap(),
            subject,
            "校验失败后旧聚合必须仍可读且未被改写"
        );
        assert!(
            ComicProgressSubjectRepository::list_members(&*repos, subject.id)
                .await
                .unwrap()
                .is_empty(),
            "校验失败不得留下部分成员写入"
        );
    }

    /// 同 Work/Edition 但非 Comic 的 MediaItem 成员同样必须被拒绝且无部分写入。
    #[tokio::test]
    async fn comic_progress_subject_save_member_rejects_non_comic_member_without_partial_write() {
        let db = Arc::new(Db::open_in_memory().unwrap());
        let (work, edition, media) = seed(&db);
        let repos = Arc::new(SqliteRepositories::new(db.clone()));
        let movie_media_item_id = MediaItemId::new();
        MediaItemRepository::save(
            &*repos,
            &MediaItem {
                id: movie_media_item_id,
                edition_id: edition,
                parent_id: None,
                media_type: MediaType::Movie,
                title: "正片".to_owned(),
                index: MediaIndex::Movie,
                duration_ms: Some(1_000),
                page_count: None,
                chapter_count: None,
                published_at: None,
                status: MediaItemStatus::Available,
                created_at: UtcMillis(1),
                updated_at: UtcMillis(1),
            },
        )
        .await
        .unwrap();
        let repo = SqliteComicProgressSubjectRepository::new(db.clone());
        let mut subject = ComicProgressSubject::new(work, edition, media, UtcMillis(1));
        subject.authoritative_progress_media_item_id = None;
        repo.save(&subject).await.unwrap();

        let error = repo
            .save_member(&active_subject_member(
                subject.id,
                movie_media_item_id,
                ComicProgressSubjectRelationship::Equivalent,
                1,
            ))
            .await
            .unwrap_err();
        assert_eq!(error.kind(), ErrorKind::Validation);
        assert_eq!(repo.get(subject.id).await.unwrap().unwrap(), subject);
        assert!(
            ComicProgressSubjectRepository::list_members(&*repos, subject.id)
                .await
                .unwrap()
                .is_empty()
        );
    }

    /// Redirected Subject 不接受 active 成员：成员写入必须在写入前被拒绝，
    /// Subject 保持 redirected 且成员表保持为空。
    #[tokio::test]
    async fn comic_progress_subject_save_member_rejects_redirected_subject_active_member() {
        let db = Arc::new(Db::open_in_memory().unwrap());
        let (work, edition, media) = seed(&db);
        let repos = Arc::new(SqliteRepositories::new(db.clone()));
        let repo = SqliteComicProgressSubjectRepository::new(db.clone());
        let target = ComicProgressSubject::new(work, edition, media, UtcMillis(1));
        repo.save(&target).await.unwrap();
        let redirected_id = ComicProgressSubjectId::new();
        db.lock()
            .execute(
                "INSERT INTO comic_progress_subjects
                    (id, work_id, edition_id, canonical_media_item_id,
                     authoritative_progress_media_item_id, state, created_at, updated_at,
                     algorithm_version, redirect_subject_id)
                 VALUES (?1, ?2, ?3, ?4, ?4, 'redirected', 1, 1,
                         'comic-progress-subject-v1', ?5)",
                rusqlite::params![
                    redirected_id.to_string(),
                    work.to_string(),
                    edition.to_string(),
                    media.to_string(),
                    target.id.to_string()
                ],
            )
            .unwrap();

        let error = repo
            .save_member(&active_subject_member(
                redirected_id,
                media,
                ComicProgressSubjectRelationship::Equivalent,
                1,
            ))
            .await
            .unwrap_err();
        assert_eq!(error.kind(), ErrorKind::Validation);

        let restored = repo
            .get(redirected_id)
            .await
            .unwrap()
            .expect("被拒绝的成员写入不得破坏既有 Subject");
        assert_eq!(restored.state, ComicProgressSubjectState::Redirected);
        assert_eq!(restored.redirect_subject_id, Some(target.id));
        assert!(restored.members().is_empty());
        assert!(
            ComicProgressSubjectRepository::list_members(&*repos, redirected_id)
                .await
                .unwrap()
                .is_empty()
        );
    }

    /// 同一 `(subject_id, media_item_id, state)` 的重复 `save_member` 是 upsert：
    /// 覆盖该行而不是新增，成员行数保持 1，且改动确实落库。
    #[tokio::test]
    async fn comic_progress_subject_save_member_upserts_same_subject_media_state() {
        let db = Arc::new(Db::open_in_memory().unwrap());
        let (work, edition, media) = seed(&db);
        let repos = Arc::new(SqliteRepositories::new(db.clone()));
        let repo = SqliteComicProgressSubjectRepository::new(db.clone());
        let mut subject = ComicProgressSubject::new(work, edition, media, UtcMillis(1));
        subject.authoritative_progress_media_item_id = None;
        repo.save(&subject).await.unwrap();

        let mut member = active_subject_member(
            subject.id,
            media,
            ComicProgressSubjectRelationship::Canonical,
            1,
        );
        repo.save_member(&member).await.unwrap();
        assert_eq!(count_rows(&db, "comic_progress_subject_members"), 1);
        let first_member = member.clone();
        let after_first_insert = repo.get(subject.id).await.unwrap().unwrap();
        assert_eq!(
            after_first_insert.members(),
            std::slice::from_ref(&first_member)
        );

        member.evidence = vec![ChapterEvidence::AuthoritativeContentKey];
        member.updated_at = UtcMillis(5);
        repo.save_member(&member).await.unwrap();

        assert_eq!(
            count_rows(&db, "comic_progress_subject_members"),
            1,
            "同 subject/media/state 的重复写入必须是 upsert"
        );
        let members = ComicProgressSubjectRepository::list_members(&*repos, subject.id)
            .await
            .unwrap();
        assert_eq!(members, vec![member.clone()]);
        assert_eq!(
            members[0].evidence,
            vec![ChapterEvidence::AuthoritativeContentKey]
        );
        assert_eq!(members[0].updated_at, UtcMillis(5));
        assert_eq!(
            repo.get(subject.id).await.unwrap().unwrap().members(),
            std::slice::from_ref(&member)
        );
    }

    #[tokio::test]
    async fn comic_progress_subjects_reject_unsafe_evidence_boundary() {
        let db = Arc::new(Db::open_in_memory().unwrap());
        let (work, edition, media) = seed(&db);
        let repo = SqliteComicProgressSubjectRepository::new(db);
        let mut subject = ComicProgressSubject::new(work, edition, media, UtcMillis(1));
        let mut member = ComicProgressSubjectMember::active(
            subject.id,
            media,
            vec![ChapterEvidence::SameRemoteIdentity],
        );
        member.algorithm_version = "https://secret.invalid".to_owned();
        subject.attach_member(member).unwrap();
        assert!(repo.save(&subject).await.is_err());
    }

    #[tokio::test]
    async fn comic_progress_subjects_reject_unsafe_persisted_text_on_read() {
        let db = Arc::new(Db::open_in_memory().unwrap());
        let (work, edition, media) = seed(&db);
        let subject_id = ComicProgressSubjectId::new();
        db.lock()
            .execute(
                "INSERT INTO comic_progress_subjects
                    (id, work_id, edition_id, canonical_media_item_id,
                     authoritative_progress_media_item_id, state, created_at, updated_at,
                     algorithm_version, redirect_subject_id)
                 VALUES (?1, ?2, ?3, ?4, ?4, 'active', 1, 1, 'https://invalid', NULL)",
                rusqlite::params![
                    subject_id.to_string(),
                    work.to_string(),
                    edition.to_string(),
                    media.to_string()
                ],
            )
            .unwrap();

        let repo = SqliteComicProgressSubjectRepository::new(db);
        assert!(repo.get(subject_id).await.is_err());
    }

    #[tokio::test]
    async fn comic_progress_subjects_reject_corrupt_cross_work_rows_on_get_and_list() {
        let db = Arc::new(Db::open_in_memory().unwrap());
        let (work_a, edition_a, media_a) = seed(&db);
        let (work_b, edition_b, media_b) = seed(&db);
        let corrupt_subject_id = ComicProgressSubjectId::new();
        let member_corrupt_subject_id = ComicProgressSubjectId::new();
        {
            let conn = db.lock();
            conn.execute(
                "INSERT INTO comic_progress_subjects
                    (id, work_id, edition_id, canonical_media_item_id,
                     authoritative_progress_media_item_id, state, created_at, updated_at,
                     algorithm_version, redirect_subject_id)
                 VALUES (?1, ?2, ?3, ?4, ?4, 'active', 1, 1, 'test-v1', NULL)",
                rusqlite::params![
                    corrupt_subject_id.to_string(),
                    work_a.to_string(),
                    edition_b.to_string(),
                    media_b.to_string()
                ],
            )
            .unwrap();
            conn.execute(
                "INSERT INTO comic_progress_subjects
                    (id, work_id, edition_id, canonical_media_item_id,
                     authoritative_progress_media_item_id, state, created_at, updated_at,
                     algorithm_version, redirect_subject_id)
                 VALUES (?1, ?2, ?3, ?4, ?4, 'active', 1, 1, 'test-v1', NULL)",
                rusqlite::params![
                    member_corrupt_subject_id.to_string(),
                    work_a.to_string(),
                    edition_a.to_string(),
                    media_a.to_string()
                ],
            )
            .unwrap();
            conn.execute(
                "INSERT INTO comic_progress_subject_members
                    (subject_id, media_item_id, relationship, confidence, evidence_json,
                     state, algorithm_version, created_at, updated_at)
                 VALUES (?1, ?2, 'candidate', 'medium', '[]', 'candidate', 'test-v1', 1, 1)",
                rusqlite::params![member_corrupt_subject_id.to_string(), media_b.to_string()],
            )
            .unwrap();
        }

        let repo = SqliteComicProgressSubjectRepository::new(db);
        assert!(repo.get(corrupt_subject_id).await.is_err());
        assert!(repo.get(member_corrupt_subject_id).await.is_err());
        assert!(repo.list_by_work(work_a).await.is_err());
        assert_ne!(work_a, work_b);
    }

    #[test]
    fn comic_progress_subjects_uow_commits_page_subject_receipt_and_new_progress() {
        use crate::db::uow::SqliteUnitOfWork;
        use haven_application::services::ports::{
            ComicPageIdentityWriteCandidate, ComicProgressWriteCandidate, UnitOfWork,
        };
        use haven_domain::comic_catalog::ComicCatalogRefreshReceipt;
        use haven_domain::comic_identity::PageIdentity;
        use haven_domain::entities::Progress;
        use haven_domain::enums::CompletionState;
        use haven_domain::ids::{ComicCatalogRefreshId, ProgressId};
        use haven_domain::locator::{ComicLocator, Locator};

        let db = Arc::new(Db::open_in_memory().unwrap());
        let (work_id, edition_id, media_item_id) = seed(&db);
        let now = UtcMillis(1);
        let subject = ComicProgressSubject::new(work_id, edition_id, media_item_id, now);
        let receipt = ComicCatalogRefreshReceipt {
            id: ComicCatalogRefreshId::new(),
            work_id,
            source_key: "source".to_owned(),
            remote_work_id: "remote-work".to_owned(),
            status: haven_domain::comic_catalog::ComicCatalogRefreshOutcomeStatus::NeverSynced,
            generation_before: 0,
            generation_after: None,
            observed_from: None,
            observed_to: None,
            truncated: false,
            retained_previous_catalog: true,
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
        let result = SqliteUnitOfWork::new(db.clone())
            .run_comic_progress_subject_write(
                &haven_application::services::ports::ComicProgressSubjectWritePlan {
                    subject,
                    members: vec![],
                    page_identity_write: Some(ComicPageIdentityWriteCandidate {
                        media_item_id,
                        pages: vec![PageIdentity::stable("page-1")],
                        expected_revision: None,
                    }),
                    progress_writes: vec![ComicProgressWriteCandidate {
                        progress,
                        expected_revision: None,
                    }],
                    migration_snapshot: None,
                    refresh_receipt: Some(receipt.clone()),
                },
            )
            .unwrap();
        assert_eq!(result.applied_progress_revisions.len(), 1);
        assert_eq!(result.refresh_id, Some(receipt.id));
        assert_eq!(count_rows(&db, "comic_progress_subjects"), 1);
        assert_eq!(count_rows(&db, "comic_catalog_refresh_outcomes"), 1);
        assert_eq!(count_rows(&db, "comic_page_identities"), 1);
        assert_eq!(count_rows(&db, "progress"), 1);
    }

    #[test]
    fn comic_progress_subjects_uow_rejects_cross_work_page_identity_without_overwrite() {
        use crate::db::uow::SqliteUnitOfWork;
        use haven_application::services::ports::{
            ComicPageIdentityWriteCandidate, ComicProgressSubjectWritePlan, UnitOfWork,
        };
        use haven_domain::comic_identity::PageIdentity;

        let db = Arc::new(Db::open_in_memory().unwrap());
        let (work_a, edition_a, media_a) = seed(&db);
        let (_, _edition_b, media_b) = seed(&db);
        {
            let conn = db.lock();
            conn.execute(
                "INSERT INTO comic_page_identities
                    (media_item_id, page_index, stable_key, fingerprint, updated_at)
                 VALUES (?1, 0, 'work-b-original', NULL, 11)",
                rusqlite::params![media_b.to_string()],
            )
            .unwrap();
            conn.execute(
                "INSERT INTO comic_page_identity_states (media_item_id, revision, updated_at)
                 VALUES (?1, 'work-b-revision', 11)",
                rusqlite::params![media_b.to_string()],
            )
            .unwrap();
        }

        let subject = ComicProgressSubject::new(work_a, edition_a, media_a, UtcMillis(1));
        let error = SqliteUnitOfWork::new(db.clone())
            .run_comic_progress_subject_write(&ComicProgressSubjectWritePlan {
                subject,
                members: vec![],
                page_identity_write: Some(ComicPageIdentityWriteCandidate {
                    media_item_id: media_b,
                    pages: vec![PageIdentity::stable("work-a-forbidden")],
                    expected_revision: Some("work-b-revision".to_owned()),
                }),
                progress_writes: vec![],
                migration_snapshot: None,
                refresh_receipt: None,
            })
            .unwrap_err();
        assert_eq!(error.kind(), ErrorKind::Validation);

        let conn = db.lock();
        let page: (Option<String>, Option<String>) = conn
            .query_row(
                "SELECT stable_key, fingerprint
                 FROM comic_page_identities WHERE media_item_id = ?1 AND page_index = 0",
                rusqlite::params![media_b.to_string()],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .unwrap();
        assert_eq!(page, (Some("work-b-original".to_owned()), None));
        let revision: String = conn
            .query_row(
                "SELECT revision FROM comic_page_identity_states WHERE media_item_id = ?1",
                rusqlite::params![media_b.to_string()],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(revision, "work-b-revision");
    }

    /// ExactSnapshot CAS：受检写入必须与**同一 Immediate 事务内重新读取**的
    /// Subject/全部成员比较。这里先落库两个 active 成员并取旧快照，再并发提交
    /// 新 pointer / 新 member，最后用旧快照调用受检写入：
    /// - 必须返回 COMIC_PROGRESS_SUBJECT_CONFLICT；
    /// - 并发写入的 pointer 与新成员必须仍然存在——即冲突发生在
    ///   `save_on_conn` 的 DELETE members **之前**（否则旧计划的 2 条成员会
    ///   覆盖掉并发写入的第 3 条）。
    #[tokio::test]
    async fn comic_progress_subject_checked_write_rejects_stale_snapshot_before_destructive_delete()
    {
        use crate::db::uow::SqliteUnitOfWork;
        use haven_application::services::ports::{
            ComicProgressSubjectWritePrecondition, UnitOfWork,
        };

        let db = Arc::new(Db::open_in_memory().unwrap());
        let repos = Arc::new(SqliteRepositories::new(db.clone()));
        let first_media_item_id = MediaItemId::new();
        let (work_id, edition_id) =
            seed_real_comic_content(&repos, "CAS 漫画", first_media_item_id).await;
        let second_media_item_id = add_comic_media_item(&repos, edition_id, 2.0).await;
        let third_media_item_id = add_comic_media_item(&repos, edition_id, 3.0).await;

        let mut subject =
            ComicProgressSubject::new(work_id, edition_id, first_media_item_id, UtcMillis(1));
        subject.authoritative_progress_media_item_id = None;
        let first_member = active_subject_member(
            subject.id,
            first_media_item_id,
            ComicProgressSubjectRelationship::Canonical,
            1,
        );
        let second_member = active_subject_member(
            subject.id,
            second_media_item_id,
            ComicProgressSubjectRelationship::Equivalent,
            2,
        );
        let unit_of_work = SqliteUnitOfWork::new(db.clone());
        unit_of_work
            .run_comic_progress_subject_write(&subject_write_plan(
                subject.clone(),
                vec![first_member.clone(), second_member.clone()],
            ))
            .unwrap();
        assert_eq!(count_rows(&db, "comic_progress_subject_members"), 2);

        // 旧快照：并发写入之前读到的 Subject + 全部成员（pointer 仍为 None）。
        let stale_subject = ComicProgressSubjectRepository::get(&*repos, subject.id)
            .await
            .unwrap()
            .unwrap();
        let stale_members = ComicProgressSubjectRepository::list_members(&*repos, subject.id)
            .await
            .unwrap();
        assert_eq!(stale_subject.authoritative_progress_media_item_id, None);
        assert_eq!(
            stale_members,
            vec![first_member.clone(), second_member.clone()]
        );

        // 并发 winner 1：另一处写入把权威 pointer 指向第二个 active 成员。
        let mut pointer_subject = stale_subject.clone();
        pointer_subject.authoritative_progress_media_item_id = Some(second_media_item_id);
        pointer_subject.updated_at = UtcMillis(2);
        unit_of_work
            .run_comic_progress_subject_write(&subject_write_plan(
                pointer_subject.clone(),
                stale_members.clone(),
            ))
            .unwrap();

        // 用旧快照（pointer=None）做受检写入：必须冲突且不得破坏并发 pointer。
        let mut stale_plan_subject = stale_subject.clone();
        stale_plan_subject.authoritative_progress_media_item_id = Some(first_media_item_id);
        let error = unit_of_work
            .run_checked_comic_progress_subject_write(
                &subject_write_plan(stale_plan_subject.clone(), stale_members.clone()),
                &ComicProgressSubjectWritePrecondition::ExactSnapshot {
                    subject: stale_subject.clone(),
                    members: stale_members.clone(),
                    require_authoritative_progress_none: true,
                },
            )
            .unwrap_err();
        assert_eq!(error.code().as_str(), "COMIC_PROGRESS_SUBJECT_CONFLICT");
        assert_eq!(error.kind(), ErrorKind::Conflict);
        assert_eq!(
            ComicProgressSubjectRepository::get(&*repos, subject.id)
                .await
                .unwrap()
                .unwrap(),
            pointer_subject,
            "并发提交的 pointer 必须在冲突后原样保留"
        );

        // 并发 winner 2：另一处写入新增第三个 active 成员。
        let third_member = active_subject_member(
            subject.id,
            third_media_item_id,
            ComicProgressSubjectRelationship::Equivalent,
            3,
        );
        let mut grown_members = stale_members.clone();
        grown_members.push(third_member.clone());
        let mut grown_subject = pointer_subject.clone();
        grown_subject.updated_at = UtcMillis(3);
        grown_subject.load_members(grown_members.clone()).unwrap();
        unit_of_work
            .run_comic_progress_subject_write(&subject_write_plan(
                grown_subject.clone(),
                grown_members.clone(),
            ))
            .unwrap();
        assert_eq!(count_rows(&db, "comic_progress_subject_members"), 3);

        // 旧成员快照（2 条）再次受检写入：冲突必须发生在 DELETE members 之前，
        // 否则第 3 个成员会被旧计划的 2 条成员覆盖删除。
        let error = unit_of_work
            .run_checked_comic_progress_subject_write(
                &subject_write_plan(stale_plan_subject, stale_members.clone()),
                &ComicProgressSubjectWritePrecondition::ExactSnapshot {
                    subject: stale_subject,
                    members: stale_members,
                    require_authoritative_progress_none: false,
                },
            )
            .unwrap_err();
        assert_eq!(error.code().as_str(), "COMIC_PROGRESS_SUBJECT_CONFLICT");
        assert_eq!(count_rows(&db, "comic_progress_subject_members"), 3);
        assert_eq!(
            ComicProgressSubjectRepository::list_members(&*repos, subject.id)
                .await
                .unwrap(),
            grown_members,
            "并发新增的成员必须在冲突后仍可读"
        );
        assert_eq!(
            ComicProgressSubjectRepository::get(&*repos, subject.id)
                .await
                .unwrap()
                .unwrap(),
            grown_subject
        );
    }

    // ---- 真实 Immediate 事务上的并发竞争 ---------------------------------------

    /// 为 fixture MediaItem 保存一条 Comic Progress，返回持久化 revision。
    async fn seed_comic_progress(
        repos: &SqliteRepositories,
        work_id: WorkId,
        edition_id: EditionId,
        media_item_id: MediaItemId,
        at: i64,
    ) -> String {
        ProgressRepository::save_if_revision(
            repos,
            &comic_progress(work_id, edition_id, media_item_id, at),
            None,
        )
        .await
        .unwrap()
        .expect("fixture Progress 必须取得 revision")
    }

    /// 受检写入的哪一类前置条件需要被"插队"。
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    enum RaceTrigger {
        /// 首次创建：`AbsentActiveMember`。
        FirstCreate,
        /// 权威 pointer 回填：`ExactSnapshot` + pointer-none。
        PointerBackfill,
    }

    fn is_first_create(precondition: &ComicProgressSubjectWritePrecondition) -> bool {
        matches!(
            precondition,
            ComicProgressSubjectWritePrecondition::AbsentActiveMember { .. }
        )
    }

    fn is_pointer_backfill(precondition: &ComicProgressSubjectWritePrecondition) -> bool {
        matches!(
            precondition,
            ComicProgressSubjectWritePrecondition::ExactSnapshot {
                require_authoritative_progress_none: true,
                ..
            }
        )
    }

    /// 只在测试中使用的确定性竞争注入器。
    ///
    /// 它在真实 `SqliteUnitOfWork` 外面包一层：在第一次命中 `trigger` 的受检写入
    /// **之前**，先用真实 Immediate 事务提交一份并发 winner 计划，再原样转发
    /// Application 的旧计划。于是该次受检写入会在自己的事务内读到 winner 并返回
    /// `COMIC_PROGRESS_SUBJECT_CONFLICT`——这是"读取快照"与"写事务"之间被真实并发
    /// 插队的确定性等价物：被拒绝的旧计划确实走到了生产代码的 Immediate 事务与
    /// 事务内前置条件判定，winner 也确实由真实 `save_on_conn` 落库。
    struct RaceUow {
        inner: Arc<SqliteUnitOfWork>,
        trigger: RaceTrigger,
        winner: Mutex<Option<ComicProgressSubjectWritePlan>>,
        conflicts_injected: AtomicUsize,
    }

    impl RaceUow {
        fn injecting_once(
            inner: Arc<SqliteUnitOfWork>,
            trigger: RaceTrigger,
            winner: ComicProgressSubjectWritePlan,
        ) -> Self {
            Self {
                inner,
                trigger,
                winner: Mutex::new(Some(winner)),
                conflicts_injected: AtomicUsize::new(0),
            }
        }

        fn conflicts_injected(&self) -> usize {
            self.conflicts_injected.load(Ordering::SeqCst)
        }

        fn should_inject(&self, precondition: &ComicProgressSubjectWritePrecondition) -> bool {
            match self.trigger {
                RaceTrigger::FirstCreate => is_first_create(precondition),
                RaceTrigger::PointerBackfill => is_pointer_backfill(precondition),
            }
        }
    }

    impl UnitOfWork for RaceUow {
        fn run_favorite(
            &self,
            f: &dyn Fn(&dyn FavoriteTxPorts) -> Result<(), AppError>,
        ) -> Result<(), AppError> {
            self.inner.run_favorite(f)
        }

        fn run_source_import(
            &self,
            provider: &str,
            external_id: &str,
            work: &Work,
            edition: &Edition,
            items: &[MediaItem],
            resources: &[Resource],
        ) -> Result<(), AppError> {
            self.inner
                .run_source_import(provider, external_id, work, edition, items, resources)
        }

        fn run_comic_progress_subject_write(
            &self,
            plan: &ComicProgressSubjectWritePlan,
        ) -> Result<ComicProgressSubjectWriteResult, AppError> {
            self.inner.run_comic_progress_subject_write(plan)
        }

        fn run_checked_comic_progress_subject_write(
            &self,
            plan: &ComicProgressSubjectWritePlan,
            precondition: &ComicProgressSubjectWritePrecondition,
        ) -> Result<ComicProgressSubjectWriteResult, AppError> {
            if self.should_inject(precondition) {
                if let Some(winner) = self.winner.lock().unwrap().take() {
                    self.inner
                        .run_comic_progress_subject_write(&winner)
                        .expect("并发 winner 必须能在真实 SQLite 上提交");
                    self.conflicts_injected.fetch_add(1, Ordering::SeqCst);
                }
            }
            self.inner
                .run_checked_comic_progress_subject_write(plan, precondition)
        }
    }

    /// pointer 回填遇到一次事务内冲突后，必须在有界预算内重新读取并采用并发
    /// winner，而不是重放旧计划或无限自递归。
    ///
    /// 竞争只对第一次 `ExactSnapshot`（pointer-none）受检写入注入；注入本身走真实
    /// `SqliteUnitOfWork` 的 Immediate 事务，被拒绝的旧计划也由真实 SQLite 事务内的
    /// 前置条件判定。winner 选择的 pointer 与本次回填按 `last_active_at` 会选出的
    /// 成员**不同**，所以落库结果能区分"重算"与"重放旧计划"。
    #[tokio::test]
    async fn comic_progress_subject_backfill_pointer_conflict_rereads_winner_within_budget() {
        let db = Arc::new(Db::open_in_memory().unwrap());
        let repos = Arc::new(SqliteRepositories::new(db.clone()));
        let first_media = MediaItemId::new();
        let (work, edition) = seed_real_comic_content(&repos, "cas", first_media).await;
        let second_media = add_comic_media_item(&repos, edition, 2.0).await;
        // 第二个成员有更新的 last_active_at：若重放旧计划，pointer 会指向它。
        seed_comic_progress(&repos, work, edition, first_media, 100).await;
        seed_comic_progress(&repos, work, edition, second_media, 200).await;

        // 已存在但缺少权威 pointer 的 Subject（两个 active 成员）。
        let mut subject = ComicProgressSubject::new(work, edition, first_media, UtcMillis(1));
        subject.authoritative_progress_media_item_id = None;
        let members = vec![
            active_subject_member(
                subject.id,
                first_media,
                ComicProgressSubjectRelationship::Canonical,
                1,
            ),
            active_subject_member(
                subject.id,
                second_media,
                ComicProgressSubjectRelationship::Equivalent,
                2,
            ),
        ];
        let uow = Arc::new(SqliteUnitOfWork::new(db.clone()));
        uow.run_comic_progress_subject_write(&subject_write_plan(subject.clone(), members.clone()))
            .unwrap();
        assert_eq!(
            ComicProgressSubjectRepository::get(&*repos, subject.id)
                .await
                .unwrap()
                .unwrap()
                .authoritative_progress_media_item_id,
            None
        );

        // 并发 winner：另一个 reader 已经把 pointer 指向**较旧**的第一个成员，
        // 与旧计划按 `last_active_at` 会选出的第二个成员不同。
        let mut winner = subject.clone();
        winner.authoritative_progress_media_item_id = Some(first_media);
        winner.updated_at = UtcMillis(2);
        winner.load_members(members.clone()).unwrap();

        let race = Arc::new(RaceUow::injecting_once(
            uow,
            RaceTrigger::PointerBackfill,
            subject_write_plan(winner.clone(), members.clone()),
        ));
        let service = ComicProgressSubjectService::new(repos.clone(), race.clone());

        let resolution = service
            .ensure_for_media_item(first_media)
            .await
            .expect("pointer 冲突后必须在有界预算内重新解析成功");

        assert_eq!(race.conflicts_injected(), 1);
        assert_eq!(resolution.subject, winner);
        assert_eq!(
            resolution.subject.authoritative_progress_media_item_id,
            Some(first_media),
            "必须采用并发 winner 的 pointer，而不是重放旧计划按 last_active_at 选出的较新成员"
        );
        assert_eq!(
            resolution
                .authoritative_progress
                .as_ref()
                .unwrap()
                .media_item_id,
            first_media,
            "目标自身已有 Progress 时必须保留目标视角"
        );
        let stored = ComicProgressSubjectRepository::get(&*repos, subject.id)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(stored, winner);
        assert_eq!(stored.members(), members.as_slice());
    }

    /// 首次创建的确定性竞争：第二个 service 的创建计划在真实 Immediate 事务内发现
    /// 同一 MediaItem 已有 active 成员，必须重读 winner 并成功返回。
    ///
    /// 注入器在 `AbsentActiveMember` 受检写入之前先提交一份 winner 创建（不同的
    /// Subject ID、同一 canonical MediaItem），再转发旧计划；
    /// `validate_subject_write_precondition` 在同一事务内看到已提交的 active 成员
    /// → `COMIC_PROGRESS_SUBJECT_CONFLICT` → `ensure_for_media_item` 重新读取
    /// `get_for_media_item`/`get`/`list_members` 并返回 winner，全程只写入一次。
    #[tokio::test]
    async fn comic_progress_subject_backfill_first_create_conflict_recovers_winner() {
        let db = Arc::new(Db::open_in_memory().unwrap());
        let repos = Arc::new(SqliteRepositories::new(db.clone()));
        let media = MediaItemId::new();
        let (work, edition) = seed_real_comic_content(&repos, "race", media).await;

        // winner：另一个 service 先提交的 Subject（未开始阅读 → 无 pointer）。
        let mut winner = ComicProgressSubject::new(work, edition, media, UtcMillis(1));
        winner.authoritative_progress_media_item_id = None;
        let member = active_subject_member(
            winner.id,
            media,
            ComicProgressSubjectRelationship::Canonical,
            1,
        );
        winner.load_members(vec![member.clone()]).unwrap();

        let uow = Arc::new(SqliteUnitOfWork::new(db.clone()));
        let race = Arc::new(RaceUow::injecting_once(
            uow.clone(),
            RaceTrigger::FirstCreate,
            subject_write_plan(winner.clone(), vec![member.clone()]),
        ));
        let racing = ComicProgressSubjectService::new(repos.clone(), race.clone());
        let sequential = ComicProgressSubjectService::new(repos.clone(), uow);

        let raced = racing
            .ensure_for_media_item(media)
            .await
            .expect("创建冲突后必须恢复 winner 并成功返回");
        let repeated = sequential
            .ensure_for_media_item(media)
            .await
            .expect("winner 已存在时必须继续成功读取");

        assert_eq!(race.conflicts_injected(), 1);
        assert_eq!(raced.subject.id, winner.id);
        assert_eq!(repeated.subject.id, winner.id);
        assert_eq!(raced.member, member);
        assert_eq!(raced.authoritative_progress, None);
        assert_eq!(
            count_rows(&db, "comic_progress_subjects"),
            1,
            "竞争创建只允许留下一个 Subject"
        );
        assert_eq!(
            count_rows(&db, "comic_progress_subject_members"),
            1,
            "竞争创建只允许留下一个 active 成员"
        );
        let owner = ComicProgressSubjectRepository::get_for_media_item(&*repos, media)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(owner.subject_id, winner.id);
    }

    /// 真实并发：两个 `Db` 连接（同一临时库文件）上的两个 service 同时读取同一
    /// Comic，两个调用都必须成功，且最终只有一个 Subject、一个 active 成员。
    ///
    /// 两个连接都会先读到"没有成员"再各自 `BEGIN IMMEDIATE`：后到的事务在
    /// busy_timeout 内等待先到者提交，然后在自己的事务内看到已提交成员并返回
    /// `COMIC_PROGRESS_SUBJECT_CONFLICT`，service 据此重读 winner。即使调度让两次
    /// 调用完全串行完成，不变量也必须成立。
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn comic_progress_subject_backfill_concurrent_first_reads_keep_one_subject_and_member() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("comic-subject-race.db");
        let first_db = Arc::new(Db::open(&path).unwrap());
        let second_db = Arc::new(Db::open(&path).unwrap());
        let first_repos = Arc::new(SqliteRepositories::new(first_db.clone()));
        let media = MediaItemId::new();
        seed_real_comic_content(&first_repos, "shared", media).await;

        let first_service = ComicProgressSubjectService::new(
            first_repos.clone(),
            Arc::new(SqliteUnitOfWork::new(first_db.clone())),
        );
        let second_service = ComicProgressSubjectService::new(
            Arc::new(SqliteRepositories::new(second_db.clone())),
            Arc::new(SqliteUnitOfWork::new(second_db.clone())),
        );

        let spawned_first = {
            let service = first_service.clone();
            tokio::spawn(async move { service.ensure_for_media_item(media).await })
        };
        let spawned_second = {
            let service = second_service.clone();
            tokio::spawn(async move { service.ensure_for_media_item(media).await })
        };
        let first = spawned_first.await.unwrap().unwrap();
        let second = spawned_second.await.unwrap().unwrap();

        assert_eq!(first.subject.id, second.subject.id);
        assert_eq!(first.authoritative_progress, None);
        assert_eq!(second.authoritative_progress, None);
        assert_eq!(count_rows(&first_db, "comic_progress_subjects"), 1);
        assert_eq!(count_rows(&first_db, "comic_progress_subject_members"), 1);
        let owner = ComicProgressSubjectRepository::get_for_media_item(&*first_repos, media)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(owner.subject_id, first.subject.id);
    }

    fn count_rows(db: &Db, table: &str) -> i64 {
        db.lock()
            .query_row(&format!("SELECT COUNT(*) FROM {table}"), [], |row| {
                row.get(0)
            })
            .unwrap()
    }
}
