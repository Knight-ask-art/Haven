//! 漫画章节目录的只读应用服务。
//!
//! Provider 负责网络、分页、身份和响应校验；这里负责校验调用意图、区分
//! 只读观察与显式刷新，并把 Domain catalog 投影为安全 Wire DTO。刷新命令
//! 只在显式调用时写入 Work/Edition/MediaItem，不会被只读查询隐式触发。

use std::sync::Arc;

use haven_common::AppError;
use haven_domain::comic_catalog::{
    ComicChapterAvailability, ComicChapterCatalog, ComicChapterCatalogState,
    ComicChapterSourceStatus,
};
use haven_domain::comic_identity::{
    ChapterSourceRef, ColorMode, EditionProfile, IdentityFacet, ScanGroupFacet,
    has_opaque_control_character,
};
use haven_domain::contracts::ChapterSourceRepository;

use super::source_import::SourceImportService;
use crate::wire::{
    ComicChapterAvailabilityDto, ComicChapterCatalogDto, ComicChapterCatalogGetRequest,
    ComicChapterCatalogItemDto, ComicChapterCatalogRefreshStateDto, ComicChapterSourceStatusDto,
    ComicColorModeDto, ComicEditionFacetKindDto, ComicEditionProfileDto,
    ComicRegisteredChapterCatalogDto, ComicRegisteredChapterCatalogItemDto, ComicScanGroupKindDto,
};

#[derive(Clone)]
pub struct ComicCatalogService {
    source_import: SourceImportService,
    registered_chapters: Arc<dyn ChapterSourceRepository>,
}

impl ComicCatalogService {
    pub fn new(
        source_import: SourceImportService,
        registered_chapters: Arc<dyn ChapterSourceRepository>,
    ) -> Self {
        Self {
            source_import,
            registered_chapters,
        }
    }

    pub async fn get(
        &self,
        request: ComicChapterCatalogGetRequest,
    ) -> Result<ComicChapterCatalogDto, AppError> {
        let catalog = self
            .source_import
            .comic_chapter_catalog(&request.source_id, &request.remote_work_id)
            .await?;
        Ok(catalog_to_dto(&catalog))
    }

    /// 读取 SQLite 中已经登记的章节，不访问 Provider，也不隐式刷新目录。
    ///
    /// 该查询故意与 Provider 观察目录分开：前者可能包含尚未入库的章节，
    /// 后者必须返回 Haven `mediaItemId`、`Missing` 状态和最近一次刷新状态，
    /// 才能成为章节列表/换源/进度迁移的真实输入。
    pub async fn get_registered(
        &self,
        request: ComicChapterCatalogGetRequest,
    ) -> Result<ComicRegisteredChapterCatalogDto, AppError> {
        let source_id = validate_registered_key(&request.source_id, "source_id")?;
        let remote_work_id = validate_registered_key(&request.remote_work_id, "remote_work_id")?;
        let chapters = ChapterSourceRepository::list_for_source_work(
            &*self.registered_chapters,
            &source_id,
            &remote_work_id,
        )
        .await?;
        let state = ChapterSourceRepository::refresh_state(
            &*self.registered_chapters,
            &source_id,
            &remote_work_id,
        )
        .await?;
        registered_catalog_to_dto(&source_id, &remote_work_id, &chapters, state.as_ref())
    }

    pub async fn refresh(
        &self,
        request: ComicChapterCatalogGetRequest,
    ) -> Result<ComicChapterCatalogDto, AppError> {
        let catalog = self
            .source_import
            .refresh_comic_chapter_catalog(&request.source_id, &request.remote_work_id)
            .await?;
        Ok(catalog_to_dto(&catalog))
    }
}

pub fn catalog_to_dto(catalog: &ComicChapterCatalog) -> ComicChapterCatalogDto {
    ComicChapterCatalogDto {
        schema_version: 1,
        source_id: catalog.source_key.clone(),
        remote_work_id: catalog.remote_work_id.clone(),
        fetched_at: crate::mapper::time::utc_millis_to_rfc3339(catalog.fetched_at),
        total: catalog.total,
        truncated: catalog.truncated,
        chapters: catalog.chapters.iter().map(chapter_to_dto).collect(),
    }
}

pub fn registered_catalog_to_dto(
    source_id: &str,
    remote_work_id: &str,
    chapters: &[ChapterSourceRef],
    state: Option<&ComicChapterCatalogState>,
) -> Result<ComicRegisteredChapterCatalogDto, AppError> {
    if chapters.iter().any(|chapter| {
        chapter.identity.source_key != source_id
            || chapter.identity.remote_work_id != remote_work_id
    }) {
        return Err(AppError::new(
            "DATABASE_ERROR",
            haven_common::ErrorKind::Database,
            "已登记漫画章节的来源身份与查询不一致",
            true,
        ));
    }
    if state.is_some_and(|state| {
        state.source_key != source_id || state.remote_work_id != remote_work_id
    }) {
        return Err(AppError::new(
            "DATABASE_ERROR",
            haven_common::ErrorKind::Database,
            "已登记漫画章节目录状态与查询不一致",
            true,
        ));
    }
    Ok(ComicRegisteredChapterCatalogDto {
        schema_version: 1,
        source_id: source_id.to_owned(),
        remote_work_id: remote_work_id.to_owned(),
        refresh_state: state.map(refresh_state_to_dto),
        chapters: chapters.iter().map(registered_chapter_to_dto).collect(),
    })
}

fn validate_registered_key(value: &str, field: &'static str) -> Result<String, AppError> {
    if has_opaque_control_character(value) {
        return Err(AppError::new(
            "INVALID_ARGUMENT",
            haven_common::ErrorKind::Validation,
            format!("漫画目录字段 {field} 非法"),
            false,
        ));
    }
    let value = value.trim();
    if value.is_empty()
        || value.len() > 4096
        || value.contains("://")
        || value.to_ascii_lowercase().starts_with("data:")
    {
        return Err(AppError::new(
            "INVALID_ARGUMENT",
            haven_common::ErrorKind::Validation,
            format!("漫画目录字段 {field} 非法"),
            false,
        ));
    }
    Ok(value.to_owned())
}

fn refresh_state_to_dto(state: &ComicChapterCatalogState) -> ComicChapterCatalogRefreshStateDto {
    ComicChapterCatalogRefreshStateDto {
        generation: state.generation,
        fetched_at: crate::mapper::time::utc_millis_to_rfc3339(state.fetched_at),
        total: state.total,
        truncated: state.truncated,
    }
}

pub(crate) fn registered_chapter_to_dto(
    chapter: &ChapterSourceRef,
) -> ComicRegisteredChapterCatalogItemDto {
    ComicRegisteredChapterCatalogItemDto {
        media_item_id: chapter.media_item_id.to_string(),
        source_id: chapter.identity.source_key.clone(),
        remote_work_id: chapter.identity.remote_work_id.clone(),
        remote_chapter_id: chapter.identity.remote_chapter_id.clone(),
        chapter_number: chapter.metadata.chapter_number,
        volume_number: chapter.metadata.volume_number,
        title: chapter.metadata.title.clone(),
        page_count: chapter.metadata.page_count,
        source_order: chapter.source_order,
        availability: source_status_to_dto(chapter.availability),
        published_at: chapter.published_at.clone(),
        source_updated_at: chapter.source_updated_at.clone(),
        last_seen_generation: chapter.last_seen_generation,
        edition_profile: profile_to_dto(&chapter.metadata.edition_profile),
    }
}

fn chapter_to_dto(
    chapter: &haven_domain::comic_catalog::ComicChapterCatalogEntry,
) -> ComicChapterCatalogItemDto {
    ComicChapterCatalogItemDto {
        remote_chapter_id: chapter.identity.remote_chapter_id.clone(),
        chapter_number: chapter.metadata.chapter_number,
        volume_number: chapter.metadata.volume_number,
        title: chapter.metadata.title.clone(),
        page_count: chapter.metadata.page_count,
        published_at: chapter.published_at.clone(),
        updated_at: chapter.updated_at.clone(),
        availability: availability_to_dto(chapter.availability),
        edition_profile: profile_to_dto(&chapter.metadata.edition_profile),
    }
}

fn availability_to_dto(value: ComicChapterAvailability) -> ComicChapterAvailabilityDto {
    match value {
        ComicChapterAvailability::Available => ComicChapterAvailabilityDto::Available,
        ComicChapterAvailability::TemporarilyUnavailable => {
            ComicChapterAvailabilityDto::TemporarilyUnavailable
        }
        ComicChapterAvailability::ExternalOnly => ComicChapterAvailabilityDto::ExternalOnly,
        ComicChapterAvailability::Unknown => ComicChapterAvailabilityDto::Unknown,
    }
}

fn source_status_to_dto(value: ComicChapterSourceStatus) -> ComicChapterSourceStatusDto {
    match value {
        ComicChapterSourceStatus::Available => ComicChapterSourceStatusDto::Available,
        ComicChapterSourceStatus::TemporarilyUnavailable => {
            ComicChapterSourceStatusDto::TemporarilyUnavailable
        }
        ComicChapterSourceStatus::ExternalOnly => ComicChapterSourceStatusDto::ExternalOnly,
        ComicChapterSourceStatus::Unknown => ComicChapterSourceStatusDto::Unknown,
        ComicChapterSourceStatus::Missing => ComicChapterSourceStatusDto::Missing,
    }
}

fn profile_to_dto(profile: &EditionProfile) -> ComicEditionProfileDto {
    let (language, language_kind) = identity_facet_to_dto(&profile.language);
    let (translation_line, translation_line_kind) =
        identity_facet_to_dto(&profile.translation_line);
    let (scan_group, scan_group_kind) = scan_group_to_dto(&profile.scan_group);
    ComicEditionProfileDto {
        language,
        language_kind,
        translation_line,
        translation_line_kind,
        scan_group,
        scan_group_kind,
        color_mode: color_mode_to_dto(profile.color_mode),
    }
}

fn identity_facet_to_dto(value: &IdentityFacet) -> (Option<String>, ComicEditionFacetKindDto) {
    match value {
        IdentityFacet::Unknown => (None, ComicEditionFacetKindDto::Unknown),
        IdentityFacet::Known(value) => (Some(value.clone()), ComicEditionFacetKindDto::Known),
        IdentityFacet::NotApplicable => (None, ComicEditionFacetKindDto::NotApplicable),
    }
}

fn scan_group_to_dto(value: &ScanGroupFacet) -> (Option<String>, ComicScanGroupKindDto) {
    match value {
        ScanGroupFacet::Unknown => (None, ComicScanGroupKindDto::Unknown),
        ScanGroupFacet::ContentLine(value) => {
            (Some(value.clone()), ComicScanGroupKindDto::ContentLine)
        }
        ScanGroupFacet::MirrorLabel(value) => {
            (Some(value.clone()), ComicScanGroupKindDto::MirrorLabel)
        }
        ScanGroupFacet::NotApplicable => (None, ComicScanGroupKindDto::NotApplicable),
    }
}

fn color_mode_to_dto(value: ColorMode) -> ComicColorModeDto {
    match value {
        ColorMode::Unknown => ComicColorModeDto::Unknown,
        ColorMode::FullColor => ComicColorModeDto::FullColor,
        ColorMode::Grayscale => ComicColorModeDto::Grayscale,
        ColorMode::Mixed => ComicColorModeDto::Mixed,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use haven_common::UtcMillis;
    use haven_domain::comic_catalog::{
        ComicChapterAvailability, ComicChapterCatalogEntry, ComicChapterSourceStatus,
    };
    use haven_domain::comic_identity::{
        ChapterSourceIdentity, ChapterSourceRef, ComicChapterMetadata,
    };
    use haven_domain::ids::MediaItemId;

    #[test]
    fn dto_keeps_facet_kind_without_exposing_internal_content_key() {
        let catalog = ComicChapterCatalog::new(
            "mangadex",
            "manga-1",
            vec![ComicChapterCatalogEntry {
                identity: ChapterSourceIdentity::new("mangadex", "manga-1", "chapter-1").unwrap(),
                metadata: ComicChapterMetadata {
                    edition_profile: EditionProfile {
                        language: IdentityFacet::known("zh-hk"),
                        translation_line: IdentityFacet::NotApplicable,
                        scan_group: ScanGroupFacet::mirror_label("mirror"),
                        color_mode: ColorMode::Grayscale,
                    },
                    chapter_number: Some(1.0),
                    volume_number: None,
                    title: Some("第一话".to_owned()),
                    page_count: Some(10),
                    authoritative_content_key: Some("internal-only".to_owned()),
                },
                availability: ComicChapterAvailability::Available,
                published_at: None,
                updated_at: None,
            }],
            UtcMillis(0),
        )
        .unwrap();
        let dto = catalog_to_dto(&catalog);
        assert_eq!(
            dto.chapters[0].edition_profile.language_kind,
            ComicEditionFacetKindDto::Known
        );
        assert_eq!(
            dto.chapters[0].edition_profile.scan_group_kind,
            ComicScanGroupKindDto::MirrorLabel
        );
        assert_eq!(
            dto.chapters[0].edition_profile.color_mode,
            ComicColorModeDto::Grayscale
        );
        let json = serde_json::to_string(&dto).unwrap();
        assert!(!json.contains("internal-only"));
        assert!(!json.contains("authoritative"));
    }

    #[test]
    fn registered_dto_keeps_media_identity_missing_state_and_refresh_generation() {
        let source_id = "mangadex";
        let remote_work_id = "manga-1";
        let reference = ChapterSourceRef {
            media_item_id: MediaItemId::new(),
            identity: ChapterSourceIdentity::new(source_id, remote_work_id, "chapter-1").unwrap(),
            metadata: ComicChapterMetadata {
                edition_profile: EditionProfile::from_language(Some("zh-cn")),
                chapter_number: Some(1.0),
                title: Some("第一话".to_owned()),
                page_count: Some(20),
                authoritative_content_key: Some("internal-only".to_owned()),
                ..ComicChapterMetadata::default()
            },
            source_order: 4,
            availability: ComicChapterSourceStatus::Missing,
            published_at: Some("2026-09-01T00:00:00Z".to_owned()),
            source_updated_at: Some("2026-09-02T00:00:00Z".to_owned()),
            last_seen_generation: Some(2),
            updated_at: UtcMillis(10),
        };
        let state = ComicChapterCatalogState {
            source_key: source_id.to_owned(),
            remote_work_id: remote_work_id.to_owned(),
            generation: 3,
            fetched_at: UtcMillis(11),
            total: Some(5),
            truncated: false,
        };

        let dto = registered_catalog_to_dto(source_id, remote_work_id, &[reference], Some(&state))
            .unwrap();
        assert_eq!(
            dto.chapters[0].availability,
            ComicChapterSourceStatusDto::Missing
        );
        assert_eq!(dto.chapters[0].source_order, 4);
        assert_eq!(dto.chapters[0].last_seen_generation, Some(2));
        assert_eq!(
            dto.refresh_state.as_ref().map(|value| value.generation),
            Some(3)
        );
        let json = serde_json::to_string(&dto).unwrap();
        assert!(!json.contains("internal-only"));
        assert!(!json.contains("authoritative"));
    }
}
