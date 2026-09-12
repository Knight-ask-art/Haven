//! 漫画章节目录的只读应用服务。
//!
//! Provider 负责网络、分页、身份和响应校验；这里负责校验调用意图、区分
//! 只读观察与显式刷新，并把 Domain catalog 投影为安全 Wire DTO。刷新命令
//! 只在显式调用时写入 Work/Edition/MediaItem，不会被只读查询隐式触发。
//!
//! Work 级聚合（[`ComicCatalogService::work_catalog_get`]）同样只读：它读取
//! Work 来源引用、漫画 Edition/画像、MediaItem、ChapterSourceRef、Resource、
//! Subject/Progress 和刷新 Receipt，然后把事实交给纯 Domain 聚合。顺序、上一章/
//! 下一章和当前项全部由后端确定，前端不得自行推导。

use std::sync::Arc;

use haven_common::{AppError, ErrorKind, UtcMillis};
use haven_domain::comic_catalog::{
    ComicCatalogRefreshOutcomeStatus, ComicChapterAggregateSource, ComicChapterAvailability,
    ComicChapterCatalog, ComicChapterCatalogState, ComicChapterSourceStatus,
    ComicResourceAvailabilityFact, ComicWorkCatalogStatus, ComicWorkChapterAggregateInput,
    ComicWorkChapterCatalog, ComicWorkChapterObservation, aggregate_comic_work_chapters,
    comic_work_catalog_status_from_receipts,
};
use haven_domain::comic_identity::{
    ChapterSourceRef, ColorMode, EditionProfile, IdentityFacet, ScanGroupFacet,
    has_opaque_control_character,
};
use haven_domain::contracts::{
    ChapterSourceRepository, ComicProgressSubjectRepository, EditionProfileRepository,
    EditionRepository, MediaItemRepository, ProgressRepository, ResourceRepository, WorkRepository,
    WorkSourceRef,
};
use haven_domain::enums::MediaType;
use haven_domain::ids::{MediaItemId, WorkId};

use super::ports::{ComicCatalogRefreshReceiptPort, ComicCatalogWorkPorts};
use super::source_import::SourceImportService;
use crate::wire::{
    ComicChapterAvailabilityDto, ComicChapterCatalogDto, ComicChapterCatalogGetRequest,
    ComicChapterCatalogItemDto, ComicChapterCatalogRefreshStateDto, ComicChapterSourceStatusDto,
    ComicColorModeDto, ComicEditionFacetKindDto, ComicEditionProfileDto,
    ComicRegisteredChapterCatalogDto, ComicRegisteredChapterCatalogItemDto, ComicScanGroupKindDto,
};

/// Work 级漫画章节只读聚合的请求。
///
/// `work_id` 与 `media_item_id` 必须严格二选一：同时为空或同时有值都返回
/// 稳定的 `INVALID_ARGUMENT`，后端绝不替调用方挑选一个。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ComicWorkChapterCatalogRequest {
    pub work_id: Option<WorkId>,
    pub media_item_id: Option<MediaItemId>,
}

#[derive(Clone)]
pub struct ComicCatalogService {
    source_import: SourceImportService,
    registered_chapters: Arc<dyn ChapterSourceRepository>,
    work_ports: Option<Arc<dyn ComicCatalogWorkPorts>>,
    refresh_receipts: Option<Arc<dyn ComicCatalogRefreshReceiptPort>>,
}

impl ComicCatalogService {
    pub fn new(
        source_import: SourceImportService,
        registered_chapters: Arc<dyn ChapterSourceRepository>,
    ) -> Self {
        Self {
            source_import,
            registered_chapters,
            work_ports: None,
            refresh_receipts: None,
        }
    }

    /// 注入 Work 级聚合所需的只读端口。
    ///
    /// 未注入时 [`Self::work_catalog_get`] 返回明确的不支持错误，而不是 panic，
    /// 也不会返回伪造的空目录。
    pub fn with_work_catalog_ports(mut self, work_ports: Arc<dyn ComicCatalogWorkPorts>) -> Self {
        self.work_ports = Some(work_ports);
        self
    }

    /// 注入可选的刷新 Receipt 读取端口。没有该端口时聚合根按来源刷新状态
    /// 推导，或回落到 `NeverSynced`，不会伪造 Receipt。
    pub fn with_refresh_receipt_port(
        mut self,
        refresh_receipts: Arc<dyn ComicCatalogRefreshReceiptPort>,
    ) -> Self {
        self.refresh_receipts = Some(refresh_receipts);
        self
    }

    /// 读取 Work 级漫画章节只读聚合。
    ///
    /// 顺序、`backend_order`、当前项和上一章/下一章都由后端在聚合完成后写入；
    /// 跨 Work 或非漫画 Edition/MediaItem 一律过滤或拒绝，不泄漏到返回值。
    pub async fn work_catalog_get(
        &self,
        request: ComicWorkChapterCatalogRequest,
    ) -> Result<ComicWorkChapterCatalog, AppError> {
        let ports = self
            .work_ports
            .as_ref()
            .ok_or_else(work_catalog_ports_unavailable)?;

        let (work_id, current_media_item_id) = match (request.work_id, request.media_item_id) {
            (Some(work_id), None) => (work_id, None),
            (None, Some(media_item_id)) => (
                resolve_work_for_comic_media_item(&**ports, media_item_id).await?,
                Some(media_item_id),
            ),
            _ => {
                return Err(AppError::new(
                    "INVALID_ARGUMENT",
                    ErrorKind::Validation,
                    "work_id 与 media_item_id 必须严格二选一",
                    false,
                ));
            }
        };

        if WorkRepository::get(ports.as_work(), work_id)
            .await?
            .is_none()
        {
            return Err(comic_not_found("COMIC_WORK_NOT_FOUND", "漫画作品不存在"));
        }
        // 来源引用必须读取成功：读不到 Work 的来源范围时不能伪造目录，
        // `list_source_refs` 的不支持错误必须原样传播。
        let source_refs = WorkRepository::list_source_refs(ports.as_work(), work_id).await?;

        let receipts = match &self.refresh_receipts {
            Some(port) => port.list_by_work(work_id).await?,
            None => Vec::new(),
        };
        let (refresh_status, last_observed_at, truncated) = if self.refresh_receipts.is_some() {
            (
                comic_work_catalog_status_from_receipts(&receipts),
                receipts.iter().map(|receipt| receipt.observed_at).max(),
                receipts.iter().any(|receipt| {
                    receipt.truncated
                        || receipt.status == ComicCatalogRefreshOutcomeStatus::Truncated
                }),
            )
        } else {
            refresh_status_from_source_states(&**ports, &source_refs).await?
        };

        let mut observations: Vec<ComicWorkChapterObservation> = Vec::new();
        for edition in EditionRepository::list_by_work(ports.as_edition(), work_id).await? {
            if edition.work_id != work_id || edition.edition_type != MediaType::Comic {
                continue;
            }
            let profile = EditionProfileRepository::get(ports.as_edition_profile(), edition.id)
                .await?
                .unwrap_or_else(|| EditionProfile::from_language(edition.language.as_deref()));
            for item in
                MediaItemRepository::list_by_edition(ports.as_media_item(), edition.id).await?
            {
                if item.edition_id != edition.id || item.media_type != MediaType::Comic {
                    continue;
                }
                let sources = ChapterSourceRepository::list_for_media_item(
                    ports.as_chapter_source(),
                    item.id,
                )
                .await?;
                let resources =
                    ResourceRepository::list_by_media_item(ports.as_resource(), item.id).await?;
                let progress =
                    ProgressRepository::get_for_media_item(ports.as_progress(), item.id).await?;
                // Subject 只是投影：Candidate 成员不是 active 归属，必须为 None。
                let subject_id = ComicProgressSubjectRepository::get_for_media_item(
                    ports.as_comic_progress_subject(),
                    item.id,
                )
                .await?
                .filter(|member| member.participates_in_active_progress())
                .map(|member| member.subject_id);

                observations.push(ComicWorkChapterObservation {
                    media_item: item,
                    edition_id: edition.id,
                    edition_profile: profile.clone(),
                    sources: sources
                        .into_iter()
                        .map(|reference| ComicChapterAggregateSource {
                            reference,
                            // provider_rank 尚未持久化；读取路径不发明序号，
                            // 排序回落到 source_order 与本地 MediaItem key。
                            provider_rank: 0,
                            // 本用例没有页面身份端口：元数据候选不得升级为
                            // SameContent，跨 MediaItem 归并只依赖同一来源身份
                            // 或权威内容 key。
                            pages: Vec::new(),
                        })
                        .collect(),
                    resource_facts: resources
                        .iter()
                        .map(ComicResourceAvailabilityFact::from_resource)
                        .collect(),
                    progress,
                    subject_id,
                });
            }
        }

        Ok(aggregate_comic_work_chapters(
            ComicWorkChapterAggregateInput {
                work_id,
                observations,
                current_media_item_id,
                refresh_status,
                last_observed_at,
                truncated,
                refresh_receipts: receipts,
            },
        ))
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

fn work_catalog_ports_unavailable() -> AppError {
    AppError::new(
        "COMIC_WORK_CATALOG_UNAVAILABLE",
        ErrorKind::Unsupported,
        "当前组装层未注入 Work 级漫画聚合端口",
        false,
    )
}

fn comic_not_found(code: &'static str, message: &'static str) -> AppError {
    AppError::new(code, ErrorKind::NotFound, message, false)
}

fn invalid_comic_target(message: &'static str) -> AppError {
    AppError::new("INVALID_ARGUMENT", ErrorKind::Validation, message, false)
}

/// 由本地 MediaItem 解析它所属的 Work。
///
/// MediaItem 必须存在且属于漫画，Edition 必须存在且属于对应 Work；任何跨
/// Work 或非漫画数据都在这里被拒绝。
async fn resolve_work_for_comic_media_item(
    ports: &dyn ComicCatalogWorkPorts,
    media_item_id: MediaItemId,
) -> Result<WorkId, AppError> {
    let item = MediaItemRepository::get(ports.as_media_item(), media_item_id)
        .await?
        .ok_or_else(|| comic_not_found("COMIC_MEDIA_ITEM_NOT_FOUND", "漫画章节条目不存在"))?;
    if item.media_type != MediaType::Comic {
        return Err(invalid_comic_target("指定的条目不是漫画章节"));
    }
    let edition = EditionRepository::get(ports.as_edition(), item.edition_id)
        .await?
        .ok_or_else(|| comic_not_found("COMIC_EDITION_NOT_FOUND", "漫画版本不存在"))?;
    if edition.id != item.edition_id || edition.edition_type != MediaType::Comic {
        return Err(invalid_comic_target("指定条目的版本不是漫画版本"));
    }
    Ok(edition.work_id)
}

/// 没有刷新 Receipt 端口时，用每个来源作品的目录状态推导聚合根刷新状态。
///
/// 只能区分从未同步/同步/截断；刷新失败只能由 Receipt 表达。
async fn refresh_status_from_source_states(
    ports: &dyn ComicCatalogWorkPorts,
    source_refs: &[WorkSourceRef],
) -> Result<(ComicWorkCatalogStatus, Option<UtcMillis>, bool), AppError> {
    let mut states: Vec<ComicChapterCatalogState> = Vec::new();
    for reference in source_refs {
        if let Some(state) = ChapterSourceRepository::refresh_state(
            ports.as_chapter_source(),
            &reference.provider,
            &reference.external_id,
        )
        .await?
        {
            states.push(state);
        }
    }
    if states.is_empty() {
        return Ok((ComicWorkCatalogStatus::NeverSynced, None, false));
    }
    let truncated = states.iter().any(|state| state.truncated);
    let status = if truncated {
        ComicWorkCatalogStatus::Truncated
    } else {
        ComicWorkCatalogStatus::Synced
    };
    Ok((
        status,
        states.iter().map(|state| state.fetched_at).max(),
        truncated,
    ))
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

    // ---- Work 级聚合（真实 SQLite + 内存 Receipt 端口） ----------------------
    //
    // 这里用真实 `SqliteRepositories` 写入 Work/Edition/MediaItem/来源身份/
    // Resource/Progress/Subject，只把没有 Infrastructure 实现的刷新 Receipt
    // 端口替换成内存替身。这样聚合读取的每个事实都来自真实持久化形状。

    use haven_domain::comic_catalog::{
        ComicCatalogRefreshOutcomeStatus, ComicCatalogRefreshReceipt, ComicChapterAggregateStatus,
    };
    use haven_domain::comic_progress_subject::{
        ComicProgressSubject, ComicProgressSubjectMember, ComicProgressSubjectRelationship,
    };
    use haven_domain::contracts::ComicCatalogRefreshOutcomeRepository;
    use haven_domain::entities::{
        ArtworkSet, Edition, MediaIndex, MediaItem, Progress, Resource, ResourceLocator, Work,
    };
    use haven_domain::enums::{
        Availability, AvailabilitySource, CompletionState, MediaItemStatus, MediaType,
        ResourceType, WorkStatus, WorkType,
    };
    use haven_domain::ids::{ComicCatalogRefreshId, EditionId, ProgressId, ResourceId, WorkId};
    use haven_domain::locator::{ComicLocator, Locator};
    use haven_infrastructure::Db;
    use haven_infrastructure::db::repos::SqliteRepositories;
    use std::sync::Arc;

    struct UnusedCatalogProvider;

    #[async_trait::async_trait]
    impl crate::services::source_import::SourceCatalogProvider for UnusedCatalogProvider {
        async fn detail(
            &self,
            _source_id: &str,
            _endpoint: &str,
            _external_id: &str,
        ) -> Result<crate::services::SourceCatalogEntry, AppError> {
            Err(unused_collaborator())
        }

        async fn search(
            &self,
            _source_id: &str,
            _endpoint: &str,
            _query: &str,
            _limit: u32,
        ) -> Result<Vec<crate::services::SourceCatalogEntry>, AppError> {
            Err(unused_collaborator())
        }
    }

    struct UnusedUnitOfWork;

    impl crate::services::ports::UnitOfWork for UnusedUnitOfWork {
        fn run_favorite(
            &self,
            _f: &dyn Fn(&dyn crate::services::ports::FavoriteTxPorts) -> Result<(), AppError>,
        ) -> Result<(), AppError> {
            Err(unused_collaborator())
        }

        fn run_source_import(
            &self,
            _provider: &str,
            _external_id: &str,
            _work: &Work,
            _edition: &Edition,
            _items: &[MediaItem],
            _resources: &[Resource],
        ) -> Result<(), AppError> {
            Err(unused_collaborator())
        }
    }

    fn unused_collaborator() -> AppError {
        AppError::new(
            "UNUSED_TEST_COLLABORATOR",
            ErrorKind::Internal,
            "测试替身不执行该操作",
            false,
        )
    }

    /// 内存刷新 Receipt 端口：只读回放预置 Receipt。
    struct FakeRefreshReceipts {
        receipts: Vec<ComicCatalogRefreshReceipt>,
    }

    #[async_trait::async_trait]
    impl ComicCatalogRefreshOutcomeRepository for FakeRefreshReceipts {
        async fn list_by_work(
            &self,
            _work_id: WorkId,
        ) -> Result<Vec<ComicCatalogRefreshReceipt>, AppError> {
            Ok(self.receipts.clone())
        }
    }

    fn work_catalog_service(
        repos: Arc<SqliteRepositories>,
        receipts: Option<Vec<ComicCatalogRefreshReceipt>>,
    ) -> ComicCatalogService {
        let registered_chapters: Arc<dyn ChapterSourceRepository> = repos.clone();
        let import_ports: Arc<dyn crate::services::ports::SourceImportPorts> = repos.clone();
        let registry_ports: Arc<dyn crate::services::ports::SourceRegistryPorts> = repos.clone();
        let registry = crate::services::source_registry::SourceRegistryService::new(registry_ports);
        let source_import = SourceImportService::new(
            import_ports,
            Arc::new(UnusedUnitOfWork),
            registry,
            Arc::new(UnusedCatalogProvider),
        );
        let work_ports: Arc<dyn ComicCatalogWorkPorts> = repos;
        let service = ComicCatalogService::new(source_import, registered_chapters)
            .with_work_catalog_ports(work_ports);
        match receipts {
            Some(receipts) => {
                service.with_refresh_receipt_port(Arc::new(FakeRefreshReceipts { receipts }))
            }
            None => service,
        }
    }

    async fn seed_work(repos: &SqliteRepositories, title: &str, id: WorkId) {
        let now = UtcMillis(1);
        WorkRepository::save(
            repos,
            &Work {
                id,
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
                artwork: ArtworkSet::default(),
                created_at: now,
                updated_at: now,
            },
        )
        .await
        .unwrap();
    }

    async fn seed_edition(
        repos: &SqliteRepositories,
        work_id: WorkId,
        media_type: MediaType,
        language: Option<&str>,
        id: EditionId,
    ) {
        let now = UtcMillis(1);
        EditionRepository::save(
            repos,
            &Edition {
                id,
                work_id,
                title: "单行本".to_owned(),
                subtitle: None,
                edition_type: media_type,
                release_date: None,
                language: language.map(str::to_owned),
                region: None,
                publisher_or_studio: None,
                description: None,
                artwork: ArtworkSet::default(),
                created_at: now,
                updated_at: now,
            },
        )
        .await
        .unwrap();
    }

    async fn seed_media_item(
        repos: &SqliteRepositories,
        edition_id: EditionId,
        media_type: MediaType,
        id: MediaItemId,
        chapter: Option<f64>,
    ) {
        let now = UtcMillis(1);
        let index = match chapter {
            Some(chapter) => MediaIndex::Chapter {
                volume: Some(1.0),
                chapter: chapter as f32,
            },
            None => MediaIndex::Movie,
        };
        MediaItemRepository::save(
            repos,
            &MediaItem {
                id,
                edition_id,
                parent_id: None,
                media_type,
                title: "第 1 话".to_owned(),
                index,
                duration_ms: None,
                page_count: Some(20),
                chapter_count: None,
                published_at: None,
                status: MediaItemStatus::Available,
                created_at: now,
                updated_at: now,
            },
        )
        .await
        .unwrap();
    }

    async fn seed_source_ref(
        repos: &SqliteRepositories,
        media_item_id: MediaItemId,
        remote_chapter_id: &str,
        source_order: u32,
        availability: ComicChapterSourceStatus,
        authoritative_content_key: Option<&str>,
    ) {
        let reference = ChapterSourceRef {
            media_item_id,
            identity: ChapterSourceIdentity::new("mangadex", "manga-1", remote_chapter_id).unwrap(),
            metadata: ComicChapterMetadata {
                edition_profile: EditionProfile::from_language(Some("zh-cn")),
                chapter_number: Some(1.0),
                volume_number: Some(1.0),
                title: Some("第一话".to_owned()),
                page_count: Some(20),
                authoritative_content_key: authoritative_content_key.map(str::to_owned),
            },
            source_order,
            availability,
            published_at: None,
            source_updated_at: None,
            last_seen_generation: Some(1),
            updated_at: UtcMillis(5),
        };
        ChapterSourceRepository::save(repos, &reference)
            .await
            .unwrap();
    }

    async fn seed_local_resource(
        repos: &SqliteRepositories,
        media_item_id: MediaItemId,
        availability: Availability,
    ) {
        let now = UtcMillis(1);
        ResourceRepository::save(
            repos,
            &Resource {
                id: ResourceId::new(),
                media_item_id,
                resource_type: ResourceType::ComicArchive,
                source_id: None,
                storage_location_id: None,
                locator: ResourceLocator::LocalPath {
                    path: "C:/comics/chapter.cbz".to_owned(),
                },
                mime_type: Some("application/vnd.comicbook+zip".to_owned()),
                size: None,
                hash: None,
                availability,
                availability_source: AvailabilitySource::User,
                modified_ms: None,
                fingerprint_first: None,
                fingerprint_last: None,
                created_at: now,
                updated_at: now,
            },
        )
        .await
        .unwrap();
    }

    fn receipt(
        work_id: WorkId,
        status: ComicCatalogRefreshOutcomeStatus,
        truncated: bool,
        observed_at: i64,
    ) -> ComicCatalogRefreshReceipt {
        ComicCatalogRefreshReceipt {
            id: ComicCatalogRefreshId::new(),
            work_id,
            source_key: "mangadex".to_owned(),
            remote_work_id: "manga-1".to_owned(),
            status,
            generation_before: 1,
            generation_after: Some(2),
            observed_from: None,
            observed_to: None,
            truncated,
            retained_previous_catalog: true,
            error_code: None,
            observed_at: UtcMillis(observed_at),
        }
    }

    #[tokio::test]
    async fn comic_catalog_work_level_rejects_ambiguous_and_missing_targets() {
        let db = Arc::new(Db::open_in_memory().unwrap());
        let repos = Arc::new(SqliteRepositories::new(db));
        let service = work_catalog_service(repos.clone(), None);

        let error = service
            .work_catalog_get(ComicWorkChapterCatalogRequest {
                work_id: None,
                media_item_id: None,
            })
            .await
            .expect_err("同时为空必须拒绝");
        assert_eq!(error.code().as_str(), "INVALID_ARGUMENT");
        assert_eq!(error.kind(), ErrorKind::Validation);

        let error = service
            .work_catalog_get(ComicWorkChapterCatalogRequest {
                work_id: Some(WorkId::new()),
                media_item_id: Some(MediaItemId::new()),
            })
            .await
            .expect_err("同时有值必须拒绝，不得静默挑选一个");
        assert_eq!(error.code().as_str(), "INVALID_ARGUMENT");
        assert_eq!(error.kind(), ErrorKind::Validation);

        let error = service
            .work_catalog_get(ComicWorkChapterCatalogRequest {
                work_id: Some(WorkId::new()),
                media_item_id: None,
            })
            .await
            .expect_err("不存在的 Work 必须返回稳定错误");
        assert_eq!(error.code().as_str(), "COMIC_WORK_NOT_FOUND");
        assert_eq!(error.kind(), ErrorKind::NotFound);

        let error = service
            .work_catalog_get(ComicWorkChapterCatalogRequest {
                work_id: None,
                media_item_id: Some(MediaItemId::new()),
            })
            .await
            .expect_err("不存在的 MediaItem 必须返回稳定错误");
        assert_eq!(error.code().as_str(), "COMIC_MEDIA_ITEM_NOT_FOUND");
        assert_eq!(error.kind(), ErrorKind::NotFound);

        // 非漫画条目必须被拒绝，不得跨类型泄漏到聚合结果。
        let work_id = WorkId::new();
        let edition_id = EditionId::new();
        seed_work(&repos, "电影", work_id).await;
        seed_edition(&repos, work_id, MediaType::Movie, None, edition_id).await;
        let movie_item = MediaItemId::new();
        seed_media_item(&repos, edition_id, MediaType::Movie, movie_item, None).await;

        let error = service
            .work_catalog_get(ComicWorkChapterCatalogRequest {
                work_id: None,
                media_item_id: Some(movie_item),
            })
            .await
            .expect_err("非漫画条目必须被拒绝");
        assert_eq!(error.code().as_str(), "INVALID_ARGUMENT");
        assert_eq!(error.kind(), ErrorKind::Validation);
    }

    #[tokio::test]
    async fn comic_catalog_work_level_aggregates_editions_sources_and_navigation() {
        let db = Arc::new(Db::open_in_memory().unwrap());
        let repos = Arc::new(SqliteRepositories::new(db));
        let service = work_catalog_service(repos.clone(), None);

        let work_id = WorkId::new();
        seed_work(&repos, "漫画 A", work_id).await;
        WorkRepository::save_source_ref(&*repos, "mangadex", "manga-1", work_id)
            .await
            .unwrap();
        let zh_edition = EditionId::new();
        let ja_edition = EditionId::new();
        seed_edition(&repos, work_id, MediaType::Comic, Some("zh-cn"), zh_edition).await;
        seed_edition(&repos, work_id, MediaType::Comic, Some("ja"), ja_edition).await;

        let first = MediaItemId::new();
        let second = MediaItemId::new();
        let foreign = MediaItemId::new();
        seed_media_item(&repos, zh_edition, MediaType::Comic, first, Some(1.0)).await;
        seed_media_item(&repos, zh_edition, MediaType::Comic, second, Some(2.0)).await;
        seed_media_item(&repos, ja_edition, MediaType::Comic, foreign, Some(1.0)).await;

        seed_source_ref(
            &repos,
            first,
            "chapter-1",
            0,
            ComicChapterSourceStatus::Available,
            None,
        )
        .await;
        seed_source_ref(
            &repos,
            first,
            "chapter-1-alternate",
            1,
            ComicChapterSourceStatus::Available,
            None,
        )
        .await;
        seed_source_ref(
            &repos,
            second,
            "chapter-2",
            2,
            ComicChapterSourceStatus::Available,
            None,
        )
        .await;
        seed_source_ref(
            &repos,
            foreign,
            "jp-chapter-1",
            0,
            ComicChapterSourceStatus::Available,
            None,
        )
        .await;
        seed_local_resource(&repos, first, Availability::Available).await;

        let catalog = service
            .work_catalog_get(ComicWorkChapterCatalogRequest {
                work_id: Some(work_id),
                media_item_id: None,
            })
            .await
            .unwrap();

        assert_eq!(catalog.work_id, work_id);
        assert_eq!(
            catalog.current_media_item_id, None,
            "work_id 请求的当前项必须为 None"
        );
        assert_eq!(catalog.editions.len(), 2);
        assert_eq!(catalog.chapters.len(), 3);
        for (position, chapter) in catalog.chapters.iter().enumerate() {
            assert_eq!(
                chapter.backend_order, position as u32,
                "backend_order 必须是整个返回数组的零基位置"
            );
        }

        let first_chapter = catalog
            .chapters
            .iter()
            .find(|chapter| chapter.media_item_id == first)
            .unwrap();
        assert_eq!(
            first_chapter.sources.len(),
            2,
            "同一 MediaItem 的多条来源身份聚合成一个章节"
        );
        assert_eq!(first_chapter.status, ComicChapterAggregateStatus::Available);
        assert!(first_chapter.can_open);
        assert_eq!(
            first_chapter.progress, None,
            "没有 Progress 原行时不得伪造进度"
        );

        let first_block: Vec<MediaItemId> = catalog
            .chapters
            .iter()
            .filter(|chapter| chapter.edition_id == zh_edition)
            .map(|chapter| chapter.media_item_id)
            .collect();
        assert_eq!(first_block, vec![first, second]);
        let foreign_chapter = catalog
            .chapters
            .iter()
            .find(|chapter| chapter.media_item_id == foreign)
            .unwrap();
        assert_eq!(foreign_chapter.edition_id, ja_edition);
        assert_eq!(
            foreign_chapter.previous_media_item_id, None,
            "不同 Edition 之间不得互相链接"
        );
        assert_eq!(foreign_chapter.next_media_item_id, None);

        // media_item_id 路径：解析当前项，并返回它所属的 Work。
        let by_media = service
            .work_catalog_get(ComicWorkChapterCatalogRequest {
                work_id: None,
                media_item_id: Some(second),
            })
            .await
            .unwrap();
        assert_eq!(by_media.work_id, work_id);
        assert_eq!(by_media.current_media_item_id, Some(second));
        assert_eq!(by_media.chapters.len(), 3);
    }

    #[tokio::test]
    async fn comic_catalog_work_level_projects_progress_and_only_active_subjects() {
        let db = Arc::new(Db::open_in_memory().unwrap());
        let repos = Arc::new(SqliteRepositories::new(db));
        let service = work_catalog_service(repos.clone(), None);

        let work_id = WorkId::new();
        seed_work(&repos, "漫画 B", work_id).await;
        let edition_id = EditionId::new();
        seed_edition(&repos, work_id, MediaType::Comic, Some("zh-cn"), edition_id).await;
        let active_item = MediaItemId::new();
        let candidate_item = MediaItemId::new();
        seed_media_item(&repos, edition_id, MediaType::Comic, active_item, Some(1.0)).await;
        seed_media_item(
            &repos,
            edition_id,
            MediaType::Comic,
            candidate_item,
            Some(2.0),
        )
        .await;
        seed_source_ref(
            &repos,
            active_item,
            "chapter-1",
            0,
            ComicChapterSourceStatus::Available,
            None,
        )
        .await;
        seed_source_ref(
            &repos,
            candidate_item,
            "chapter-2",
            1,
            ComicChapterSourceStatus::Available,
            None,
        )
        .await;

        let progress = Progress {
            id: ProgressId::new(),
            work_id,
            edition_id,
            media_item_id: active_item,
            locator: Locator::Comic(ComicLocator {
                chapter_item_id: active_item,
                page_index: 7,
                page_progression: Some(0.4),
            }),
            completion: CompletionState::InProgress,
            percentage: Some(0.4),
            last_active_at: UtcMillis(500),
            updated_at: UtcMillis(500),
            revision: None,
            keyframe_uri: Some("data:image/png;base64,fixture".to_owned()),
        };
        let revision = ProgressRepository::save_if_revision(&*repos, &progress, None)
            .await
            .unwrap()
            .expect("fixture Progress 必须取得 revision");
        let mut stored_progress = progress.clone();
        stored_progress.revision = Some(revision.clone());

        let mut subject = ComicProgressSubject::new(work_id, edition_id, active_item, UtcMillis(1));
        let mut active_member = ComicProgressSubjectMember::active(
            subject.id,
            active_item,
            vec![haven_domain::comic_identity::ChapterEvidence::SameRemoteIdentity],
        );
        active_member.relationship = ComicProgressSubjectRelationship::Canonical;
        subject.attach_member(active_member).unwrap();
        subject
            .attach_member(ComicProgressSubjectMember::candidate(
                subject.id,
                candidate_item,
                vec![haven_domain::comic_identity::ChapterEvidence::WeakChapterMetadata],
            ))
            .unwrap();
        let expected_subject_id = subject.id;
        ComicProgressSubjectRepository::save_subject(&*repos, &subject)
            .await
            .unwrap();

        let catalog = service
            .work_catalog_get(ComicWorkChapterCatalogRequest {
                work_id: Some(work_id),
                media_item_id: None,
            })
            .await
            .unwrap();

        let active_chapter = catalog
            .chapters
            .iter()
            .find(|chapter| chapter.media_item_id == active_item)
            .unwrap();
        let projected = active_chapter
            .progress
            .as_ref()
            .expect("Progress 必须被投影");
        assert_eq!(projected, &stored_progress);
        assert_eq!(projected.revision.as_deref(), Some(revision.as_str()));
        assert_eq!(projected.last_active_at, UtcMillis(500));
        assert_eq!(
            projected.locator,
            Locator::Comic(ComicLocator {
                chapter_item_id: active_item,
                page_index: 7,
                page_progression: Some(0.4),
            })
        );
        assert_eq!(active_chapter.subject_id, Some(expected_subject_id));

        let candidate_chapter = catalog
            .chapters
            .iter()
            .find(|chapter| chapter.media_item_id == candidate_item)
            .unwrap();
        assert_eq!(
            candidate_chapter.subject_id, None,
            "Candidate 成员不得成为 active Subject 归属"
        );
        assert_eq!(candidate_chapter.progress, None);
    }

    #[tokio::test]
    async fn comic_catalog_work_level_uses_refresh_receipts_without_marking_missing() {
        let db = Arc::new(Db::open_in_memory().unwrap());
        let repos = Arc::new(SqliteRepositories::new(db));

        let work_id = WorkId::new();
        seed_work(&repos, "漫画 C", work_id).await;
        WorkRepository::save_source_ref(&*repos, "mangadex", "manga-1", work_id)
            .await
            .unwrap();
        let edition_id = EditionId::new();
        seed_edition(&repos, work_id, MediaType::Comic, Some("zh-cn"), edition_id).await;
        let kept = MediaItemId::new();
        seed_media_item(&repos, edition_id, MediaType::Comic, kept, Some(1.0)).await;
        seed_source_ref(
            &repos,
            kept,
            "chapter-1",
            0,
            ComicChapterSourceStatus::Available,
            None,
        )
        .await;
        seed_local_resource(&repos, kept, Availability::OfflineAvailable).await;

        let failed = receipt(
            work_id,
            ComicCatalogRefreshOutcomeStatus::RefreshFailed,
            false,
            900,
        );
        let truncated = receipt(
            work_id,
            ComicCatalogRefreshOutcomeStatus::Succeeded,
            true,
            1000,
        );
        let service = work_catalog_service(repos.clone(), Some(vec![failed, truncated]));
        let catalog = service
            .work_catalog_get(ComicWorkChapterCatalogRequest {
                work_id: Some(work_id),
                media_item_id: None,
            })
            .await
            .unwrap();

        assert_eq!(
            catalog.refresh_status,
            ComicWorkCatalogStatus::RefreshFailed
        );
        assert!(catalog.truncated);
        assert_eq!(catalog.last_observed_at, Some(UtcMillis(1000)));
        assert_eq!(catalog.refresh_receipts.len(), 2);
        let chapter = &catalog.chapters[0];
        assert_eq!(
            chapter.status,
            ComicChapterAggregateStatus::Available,
            "离线可用资源仍然可打开，刷新失败不得改写有效章节"
        );
        assert!(chapter.can_open);
        assert_ne!(chapter.status, ComicChapterAggregateStatus::Missing);
        assert_eq!(
            chapter.sources[0].observed_at,
            Some(UtcMillis(5)),
            "来源摘要保留自身观察时间"
        );
    }

    #[tokio::test]
    async fn comic_catalog_work_level_without_ports_reports_unsupported_instead_of_panicking() {
        let db = Arc::new(Db::open_in_memory().unwrap());
        let repos = Arc::new(SqliteRepositories::new(db));
        let registered_chapters: Arc<dyn ChapterSourceRepository> = repos.clone();
        let import_ports: Arc<dyn crate::services::ports::SourceImportPorts> = repos.clone();
        let registry_ports: Arc<dyn crate::services::ports::SourceRegistryPorts> = repos.clone();
        let registry = crate::services::source_registry::SourceRegistryService::new(registry_ports);
        let service = ComicCatalogService::new(
            SourceImportService::new(
                import_ports,
                Arc::new(UnusedUnitOfWork),
                registry,
                Arc::new(UnusedCatalogProvider),
            ),
            registered_chapters,
        );

        let error = service
            .work_catalog_get(ComicWorkChapterCatalogRequest {
                work_id: Some(WorkId::new()),
                media_item_id: None,
            })
            .await
            .expect_err("未注入 work ports 必须返回明确错误");
        assert_eq!(error.code().as_str(), "COMIC_WORK_CATALOG_UNAVAILABLE");
        assert_eq!(error.kind(), ErrorKind::Unsupported);
    }

    #[tokio::test]
    async fn comic_catalog_work_level_filters_other_works_and_non_comic_editions() {
        let db = Arc::new(Db::open_in_memory().unwrap());
        let repos = Arc::new(SqliteRepositories::new(db));
        let service = work_catalog_service(repos.clone(), None);

        let target_work = WorkId::new();
        let other_work = WorkId::new();
        seed_work(&repos, "目标漫画", target_work).await;
        seed_work(&repos, "其他作品", other_work).await;
        let target_edition = EditionId::new();
        seed_edition(
            &repos,
            target_work,
            MediaType::Comic,
            Some("zh-cn"),
            target_edition,
        )
        .await;
        let target_item = MediaItemId::new();
        seed_media_item(
            &repos,
            target_edition,
            MediaType::Comic,
            target_item,
            Some(1.0),
        )
        .await;
        seed_source_ref(
            &repos,
            target_item,
            "chapter-1",
            0,
            ComicChapterSourceStatus::Available,
            None,
        )
        .await;

        // 同一 Work 下的非漫画 Edition/条目必须被过滤。
        let book_edition = EditionId::new();
        seed_edition(
            &repos,
            target_work,
            MediaType::Book,
            Some("zh-cn"),
            book_edition,
        )
        .await;
        let book_item = MediaItemId::new();
        seed_media_item(&repos, book_edition, MediaType::Book, book_item, None).await;

        // 另一个 Work 的漫画 Edition/条目必须完全不可见。
        let other_edition = EditionId::new();
        seed_edition(
            &repos,
            other_work,
            MediaType::Comic,
            Some("zh-cn"),
            other_edition,
        )
        .await;
        let other_item = MediaItemId::new();
        seed_media_item(
            &repos,
            other_edition,
            MediaType::Comic,
            other_item,
            Some(1.0),
        )
        .await;
        seed_source_ref(
            &repos,
            other_item,
            "other-chapter-1",
            0,
            ComicChapterSourceStatus::Available,
            None,
        )
        .await;

        let catalog = service
            .work_catalog_get(ComicWorkChapterCatalogRequest {
                work_id: Some(target_work),
                media_item_id: None,
            })
            .await
            .unwrap();

        assert_eq!(catalog.work_id, target_work);
        assert_eq!(catalog.editions.len(), 1);
        assert_eq!(catalog.chapters.len(), 1);
        assert_eq!(catalog.chapters[0].media_item_id, target_item);
        assert!(
            catalog
                .chapters
                .iter()
                .all(|chapter| chapter.media_item_id != book_item
                    && chapter.media_item_id != other_item),
            "非漫画或跨 Work 数据不得泄漏到聚合结果"
        );
    }
}
