//! 漫画进度 Subject/Member 的 SQLite Repository。
//!
//! Subject 只保存跨 MediaItem 的连续性身份映射；所有页面、完成度、比例、
//! keyframe 和 Progress revision 都留在既有 Progress Repository。成员 evidence
//! 由领域类型序列化，且本层拒绝 URL/控制字符等运行时通道内容。

use std::sync::Arc;

use rusqlite::OptionalExtension;

use haven_common::{AppError, ErrorKind, UtcMillis};
use haven_domain::comic_progress_subject::{ComicProgressSubject, ComicProgressSubjectMember};
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
    serde_json::from_value(value)
        .map(Some)
        .map_err(|error| invalid_subject(error.to_string()))
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
            let evidence =
                serde_json::from_str(&evidence_json).map_err(|_| invalid_row("evidence_json"))?;
            Ok(ComicProgressSubjectMember {
                subject_id: parse_id(row.get(0)?, "subject_id")?,
                media_item_id: parse_id(row.get(1)?, "media_item_id")?,
                relationship: parse_db_enum(row.get(2)?, "relationship")?,
                confidence: parse_db_enum(row.get(3)?, "confidence")?,
                evidence,
                state: parse_db_enum(row.get(5)?, "state")?,
                algorithm_version: row.get(6)?,
                created_at: UtcMillis(row.get(7)?),
                updated_at: UtcMillis(row.get(8)?),
            })
        })
        .map_err(map_db_error("查询漫画进度主体成员失败"))?;
    rows.collect::<rusqlite::Result<Vec<_>>>()
        .map_err(map_db_error("查询漫画进度主体成员失败"))
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
    use haven_domain::comic_identity::ChapterEvidence;
    use haven_domain::comic_progress_subject::ComicProgressSubject;

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

    fn count_rows(db: &Db, table: &str) -> i64 {
        db.lock()
            .query_row(&format!("SELECT COUNT(*) FROM {table}"), [], |row| {
                row.get(0)
            })
            .unwrap()
    }
}
