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
        self.db.with_tx(|tx| save_member_on_conn(tx, &member))
    }
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

fn load_subject(
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

fn load_members(
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
    use haven_domain::comic_identity::ChapterEvidence;
    use haven_domain::comic_progress_subject::{
        ComicProgressSubject, ComicProgressSubjectMember, ComicProgressSubjectRelationship,
    };
    use haven_domain::contracts::{
        ComicProgressSubjectRepository, EditionRepository, HistoryRepository, MarkerRepository,
        MediaItemRepository, ProgressRepository, WorkRepository,
    };
    use haven_domain::entities::{
        Edition, HistoryEntry, Marker, MediaIndex, MediaItem, Progress, Work,
    };
    use haven_domain::enums::{
        CompletionState, MarkerType, MediaItemStatus, MediaType, WorkStatus, WorkType,
    };
    use haven_domain::ids::{HistoryEntryId, MarkerId, ProgressId};
    use haven_domain::locator::{ComicLocator, Locator};

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

    fn count_rows(db: &Db, table: &str) -> i64 {
        db.lock()
            .query_row(&format!("SELECT COUNT(*) FROM {table}"), [], |row| {
                row.get(0)
            })
            .unwrap()
    }
}
