//! 漫画 Edition 画像的 SQLite 持久化。
//!
//! `editions.language` 仍是语言事实源；本 Repository 只把语言投影到
//! `EditionProfile`，并在独立表保存翻译线、扫描组语义和彩色模式。

use std::sync::Arc;

use async_trait::async_trait;
use rusqlite::OptionalExtension;

use haven_common::{AppError, ErrorKind, UtcMillis};
use haven_domain::comic_identity::{
    ColorMode, EditionProfile, IdentityFacet, ScanGroupFacet, has_opaque_control_character,
};
use haven_domain::contracts::EditionProfileRepository;
use haven_domain::ids::EditionId;

use crate::db::Db;
use crate::db::repos::map_db_error;

pub struct SqliteEditionProfileRepository {
    db: Arc<Db>,
}

impl SqliteEditionProfileRepository {
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

/// provider 的 opaque 标签不能借身份表持久化远端地址或运行时资源句柄。
fn validate_identity_value(value: &str, field: &'static str) -> Result<(), AppError> {
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

fn facet_columns<'a>(
    value: &'a IdentityFacet,
    field: &'static str,
) -> Result<(Option<&'a str>, &'static str), AppError> {
    match value {
        IdentityFacet::Unknown => Ok((None, "unknown")),
        IdentityFacet::Known(value) => {
            validate_identity_value(value, field)?;
            Ok((Some(value.as_str()), "known"))
        }
        IdentityFacet::NotApplicable => Ok((None, "not_applicable")),
    }
}

fn scan_group_columns(value: &ScanGroupFacet) -> Result<(Option<&str>, &'static str), AppError> {
    match value {
        ScanGroupFacet::Unknown => Ok((None, "unknown")),
        ScanGroupFacet::ContentLine(value) => {
            validate_identity_value(value, "scan_group")?;
            Ok((Some(value.as_str()), "content_line"))
        }
        ScanGroupFacet::MirrorLabel(value) => {
            validate_identity_value(value, "scan_group")?;
            Ok((Some(value.as_str()), "mirror_label"))
        }
        ScanGroupFacet::NotApplicable => Ok((None, "not_applicable")),
    }
}

/// Validate a profile before storing it either as an Edition profile or as a
/// source-level chapter observation. The latter must obey the same opaque
/// identity boundary as the canonical Edition row.
pub(crate) fn validate_profile(profile: &EditionProfile) -> Result<(), AppError> {
    facet_columns(&profile.language, "language")?;
    facet_columns(&profile.translation_line, "translation_line")?;
    scan_group_columns(&profile.scan_group)?;
    Ok(())
}

fn parse_identity_facet(
    value: Option<String>,
    kind: Option<String>,
    field: &'static str,
) -> rusqlite::Result<IdentityFacet> {
    match kind.as_deref().unwrap_or("unknown") {
        "unknown" => Ok(IdentityFacet::Unknown),
        "known" => value
            .as_deref()
            .map(IdentityFacet::known)
            .ok_or_else(|| conversion_error(field)),
        "not_applicable" => Ok(IdentityFacet::NotApplicable),
        _ => Err(conversion_error(field)),
    }
}

fn parse_language_facet(
    value: Option<String>,
    kind: Option<String>,
) -> rusqlite::Result<IdentityFacet> {
    match kind.as_deref() {
        // No profile row is the legacy projection: editions.language remains the
        // source of truth and a NULL value means Unknown.
        None => Ok(value
            .as_deref()
            .map(IdentityFacet::known)
            .unwrap_or(IdentityFacet::Unknown)),
        Some("unknown") => {
            if value.is_some() {
                Err(conversion_error("language"))
            } else {
                Ok(IdentityFacet::Unknown)
            }
        }
        Some("known") => value
            .as_deref()
            .map(IdentityFacet::known)
            .ok_or_else(|| conversion_error("language")),
        Some("not_applicable") => {
            if value.is_some() {
                Err(conversion_error("language"))
            } else {
                Ok(IdentityFacet::NotApplicable)
            }
        }
        Some(_) => Err(conversion_error("language")),
    }
}

fn parse_scan_group(
    value: Option<String>,
    kind: Option<String>,
) -> rusqlite::Result<ScanGroupFacet> {
    match kind.as_deref().unwrap_or("unknown") {
        "unknown" => Ok(ScanGroupFacet::Unknown),
        "content_line" => value
            .as_deref()
            .map(ScanGroupFacet::content_line)
            .ok_or_else(|| conversion_error("scan_group")),
        "mirror_label" => value
            .as_deref()
            .map(ScanGroupFacet::mirror_label)
            .ok_or_else(|| conversion_error("scan_group")),
        "not_applicable" => Ok(ScanGroupFacet::NotApplicable),
        _ => Err(conversion_error("scan_group")),
    }
}

fn parse_color_mode(value: Option<String>) -> rusqlite::Result<ColorMode> {
    match value.as_deref().unwrap_or("unknown") {
        "unknown" => Ok(ColorMode::Unknown),
        "full_color" => Ok(ColorMode::FullColor),
        "grayscale" => Ok(ColorMode::Grayscale),
        "mixed" => Ok(ColorMode::Mixed),
        _ => Err(conversion_error("color_mode")),
    }
}

fn conversion_error(field: &'static str) -> rusqlite::Error {
    rusqlite::Error::FromSqlConversionFailure(
        0,
        rusqlite::types::Type::Text,
        Box::new(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            format!("无法解析漫画 Edition 画像字段 {field}"),
        )),
    )
}

pub(crate) fn profile_from_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<EditionProfile> {
    let language: Option<String> = row.get("edition_language")?;
    let language_kind: Option<String> = row.get("language_kind")?;
    let translation_line: Option<String> = row.get("translation_line")?;
    let translation_line_kind: Option<String> = row.get("translation_line_kind")?;
    let scan_group: Option<String> = row.get("scan_group")?;
    let scan_group_kind: Option<String> = row.get("scan_group_kind")?;
    let color_mode: Option<String> = row.get("color_mode")?;
    Ok(EditionProfile {
        language: parse_language_facet(language, language_kind)?,
        translation_line: parse_identity_facet(
            translation_line,
            translation_line_kind,
            "translation_line",
        )?,
        scan_group: parse_scan_group(scan_group, scan_group_kind)?,
        color_mode: parse_color_mode(color_mode)?,
    })
}

pub(crate) const PROFILE_SELECT: &str = "e.language AS edition_language, p.language_kind,
    p.translation_line, p.translation_line_kind, p.scan_group, p.scan_group_kind,
    p.color_mode";

/// 在指定连接上保存 Edition 语言投影和漫画画像。连接可以是普通 SQLite
/// Connection，也可以是 UnitOfWork 持有的事务连接；这里不自行开启事务。
pub(crate) fn save_on_conn(
    conn: &rusqlite::Connection,
    edition_id: EditionId,
    profile: &EditionProfile,
) -> Result<(), AppError> {
    validate_profile(profile)?;
    let (language, language_kind) = facet_columns(&profile.language, "language")?;
    let (translation_line, translation_line_kind) =
        facet_columns(&profile.translation_line, "translation_line")?;
    let (scan_group, scan_group_kind) = scan_group_columns(&profile.scan_group)?;
    let color_mode = match profile.color_mode {
        ColorMode::Unknown => "unknown",
        ColorMode::FullColor => "full_color",
        ColorMode::Grayscale => "grayscale",
        ColorMode::Mixed => "mixed",
    };
    let now = UtcMillis::now().0;

    let affected = conn
        .execute(
            "UPDATE editions SET language = ?1, updated_at = ?2 WHERE id = ?3",
            rusqlite::params![language, now, edition_id.to_string()],
        )
        .map_err(map_db_error("更新漫画 Edition 语言失败"))?;
    if affected == 0 {
        return Err(AppError::new(
            "EDITION_NOT_FOUND",
            ErrorKind::NotFound,
            "版本不存在",
            false,
        ));
    }
    conn.execute(
        "INSERT INTO edition_profiles
            (edition_id, language_kind, translation_line, translation_line_kind,
             scan_group, scan_group_kind, color_mode, created_at, updated_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?8)
         ON CONFLICT(edition_id) DO UPDATE SET
             language_kind = excluded.language_kind,
             translation_line = excluded.translation_line,
             translation_line_kind = excluded.translation_line_kind,
             scan_group = excluded.scan_group,
             scan_group_kind = excluded.scan_group_kind,
             color_mode = excluded.color_mode,
             updated_at = excluded.updated_at",
        rusqlite::params![
            edition_id.to_string(),
            language_kind,
            translation_line,
            translation_line_kind,
            scan_group,
            scan_group_kind,
            color_mode,
            now,
        ],
    )
    .map_err(map_db_error("保存漫画 Edition 画像失败"))?;
    Ok(())
}

#[async_trait]
impl EditionProfileRepository for SqliteEditionProfileRepository {
    async fn get(&self, edition_id: EditionId) -> Result<Option<EditionProfile>, AppError> {
        let conn = self.db.lock();
        conn.query_row(
            &format!(
                "SELECT {PROFILE_SELECT}
                 FROM editions e
                 LEFT JOIN edition_profiles p ON p.edition_id = e.id
                 WHERE e.id = ?1"
            ),
            rusqlite::params![edition_id.to_string()],
            profile_from_row,
        )
        .optional()
        .map_err(map_db_error("查询漫画 Edition 画像失败"))
    }

    async fn save(&self, edition_id: EditionId, profile: &EditionProfile) -> Result<(), AppError> {
        let mut guard = self.db.lock();
        let tx = guard
            .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)
            .map_err(map_db_error("开启漫画 Edition 画像事务失败"))?;
        save_on_conn(&tx, edition_id, profile)?;
        tx.commit()
            .map_err(map_db_error("提交漫画 Edition 画像事务失败"))?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use haven_domain::comic_identity::{EditionProfile, IdentityFacet, ScanGroupFacet};
    use haven_domain::entities::MediaItem;
    use haven_domain::enums::{MediaItemStatus, MediaType};
    use haven_domain::ids::{MediaItemId, WorkId};

    fn seed_edition(db: &Db) -> EditionId {
        let work_id = WorkId::new();
        let edition_id = EditionId::new();
        let now = 1_000;
        let conn = db.lock();
        conn.execute(
            "INSERT INTO works (id, canonical_title, work_type, status, created_at, updated_at)
             VALUES (?1, '漫画画像测试', 'fiction', 'completed', ?2, ?2)",
            rusqlite::params![work_id.to_string(), now],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO editions (id, work_id, title, edition_type, language, created_at, updated_at)
             VALUES (?1, ?2, '中文漫画', 'comic', 'zh-cn', ?3, ?3)",
            rusqlite::params![edition_id.to_string(), work_id.to_string(), now],
        )
        .unwrap();
        edition_id
    }

    #[tokio::test]
    async fn profile_roundtrip_keeps_language_and_all_identity_facets() {
        let db = Arc::new(Db::open_in_memory().unwrap());
        let edition_id = seed_edition(&db);
        let repo = SqliteEditionProfileRepository::new(db);
        let profile = EditionProfile {
            language: IdentityFacet::known("zh-CN"),
            translation_line: IdentityFacet::known("Team A"),
            scan_group: ScanGroupFacet::content_line("Scan A"),
            color_mode: ColorMode::Grayscale,
        };

        repo.save(edition_id, &profile).await.unwrap();
        let stored = repo.get(edition_id).await.unwrap().unwrap();
        assert_eq!(stored, profile);
    }

    #[tokio::test]
    async fn missing_profile_projects_legacy_edition_language_and_defaults_unknowns() {
        let db = Arc::new(Db::open_in_memory().unwrap());
        let edition_id = seed_edition(&db);
        let repo = SqliteEditionProfileRepository::new(db);
        let stored = repo.get(edition_id).await.unwrap().unwrap();
        assert_eq!(stored.language, IdentityFacet::known("zh-cn"));
        assert_eq!(stored.translation_line, IdentityFacet::Unknown);
        assert_eq!(stored.scan_group, ScanGroupFacet::Unknown);
        assert_eq!(stored.color_mode, ColorMode::Unknown);
    }

    #[tokio::test]
    async fn language_not_applicable_roundtrips_without_value() {
        let db = Arc::new(Db::open_in_memory().unwrap());
        let edition_id = seed_edition(&db);
        let repo = SqliteEditionProfileRepository::new(db.clone());
        let profile = EditionProfile {
            language: IdentityFacet::NotApplicable,
            ..EditionProfile::default()
        };

        repo.save(edition_id, &profile).await.unwrap();
        let stored = repo.get(edition_id).await.unwrap().unwrap();
        assert_eq!(stored, profile);
        let language: Option<String> = db
            .lock()
            .query_row(
                "SELECT language FROM editions WHERE id = ?1",
                rusqlite::params![edition_id.to_string()],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(language, None);
    }

    #[tokio::test]
    async fn profile_save_rejects_url_shaped_identity() {
        let db = Arc::new(Db::open_in_memory().unwrap());
        let edition_id = seed_edition(&db);
        let repo = SqliteEditionProfileRepository::new(db);
        let profile = EditionProfile {
            translation_line: IdentityFacet::known("https://example.invalid/group"),
            ..EditionProfile::default()
        };
        let err = repo.save(edition_id, &profile).await.unwrap_err();
        assert_eq!(err.code().as_str(), "INVALID_COMIC_IDENTITY");
    }

    #[allow(dead_code)]
    fn _media_item_type_is_available_for_future_identity_fixtures() -> MediaItem {
        MediaItem {
            id: MediaItemId::new(),
            edition_id: EditionId::new(),
            parent_id: None,
            media_type: MediaType::Comic,
            title: String::new(),
            index: haven_domain::entities::MediaIndex::Movie,
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
