//! 漫画章节换源与页面变化的进度迁移用例。
//!
//! 规则：
//! - 章节身份先经过领域比较，`OneTime` 直接执行跨 MediaItem 迁移，
//!   `Suggested` 只有显式允许最佳努力时才执行；
//! - 页面数量/顺序变化允许最佳努力定位，结果始终保留策略、置信度和快照；
//! - 目标已有进度默认不覆盖，显式允许后仍必须以目标 revision 做 CAS；
//! - 运行时 pageId、grant、URL、归档 entry 不参与输入，也不会写入快照。

use std::collections::HashMap;
use std::sync::Arc;

use haven_common::{AppError, ErrorKind, UtcMillis};
use haven_domain::comic_identity::{
    ChapterEvidence, ChapterMatch, ChapterMatchKind, ChapterSourceIdentity,
    ComicProgressMigrationSnapshot, MatchConfidence, PageIdentity, PageMappingConfidence,
    PageMappingStrategy, PageMigration, ProgressMigrationMode, ProgressMigrationState,
    compare_chapters, compare_chapters_within_media_item, has_opaque_control_character,
    migrate_page_index,
};
use haven_domain::contracts::{
    ChapterSourceRepository, ComicPageIdentityRepository, ComicProgressMigrationRepository,
    EditionRepository, MediaItemRepository, ProgressRepository,
};
use haven_domain::entities::{MediaItem, Progress};
use haven_domain::enums::MediaType;
use haven_domain::ids::{ComicProgressMigrationId, MediaItemId, ProgressId};
use haven_domain::locator::{ComicLocator, Locator};

use crate::services::ports::ComicProgressMigrationPorts;
use crate::wire::{
    ComicChapterEvidenceDto, ComicChapterEvidenceKindDto, ComicChapterMatchDto,
    ComicChapterMatchKindDto, ComicChapterSourceCandidateDto, ComicChapterSourceCandidatesDto,
    ComicChapterSourceCandidatesGetRequestDto, ComicChapterSourceIdentityDto,
    ComicMatchConfidenceDto, ComicPageMappingConfidenceDto, ComicPageMappingStrategyDto,
    ComicPageMigrationDto, ComicProgressMigrationModeDto, ComicProgressMigrationRequestDto,
    ComicProgressMigrationResultDto, ComicProgressMigrationRevertRequestDto,
    ComicProgressMigrationRevertResultDto, ComicProgressMigrationStatusDto,
};

/// 章节换源请求。来源身份必须是 provider 已校验的 opaque identity。
#[derive(Debug, Clone)]
pub struct ComicProgressMigrationRequest {
    pub source: ChapterSourceIdentity,
    pub target: ChapterSourceIdentity,
    /// 允许低置信度元数据匹配执行可撤销的最佳努力迁移。
    pub allow_best_effort: bool,
    /// 只有用户明确选择覆盖目标现有进度时才设为 true。
    pub allow_target_overwrite: bool,
}

/// 页面序列更新时的进度重定位请求。
#[derive(Debug, Clone)]
pub struct ComicPageProgressRemapRequest {
    pub media_item_id: MediaItemId,
    pub old_pages: Vec<PageIdentity>,
    pub new_pages: Vec<PageIdentity>,
    /// 若调用方已有读取时的 revision，优先传入；None 时服务使用刚读到的
    /// revision，最终仍由 Infrastructure CAS 保护。
    pub expected_revision: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ComicProgressMigrationStatus {
    Unchanged,
    NotApplicable,
    Applied,
    SharedContent,
    Suggested,
    NoSourceProgress,
    TargetProgressPreserved,
    NoTargetPage,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ComicProgressMigrationResult {
    pub status: ComicProgressMigrationStatus,
    pub match_result: Option<ChapterMatch>,
    pub page_migration: PageMigration,
    pub snapshot_id: Option<ComicProgressMigrationId>,
    pub applied_revision: Option<String>,
}

#[derive(Clone)]
pub struct ComicProgressMigrationService {
    ports: Arc<dyn ComicProgressMigrationPorts>,
}

impl ComicProgressMigrationService {
    pub fn new(ports: Arc<dyn ComicProgressMigrationPorts>) -> Self {
        Self { ports }
    }

    /// IPC 入口：把安全 Wire 请求转换为领域请求，再执行一次章节换源迁移。
    /// URL、路径、运行时授权和未经校验的内部 ID 在这里全部拒绝。
    pub async fn migrate_wire(
        &self,
        request: ComicProgressMigrationRequestDto,
    ) -> Result<ComicProgressMigrationResultDto, AppError> {
        let request = ComicProgressMigrationRequest {
            source: source_identity_to_domain(request.source)?,
            target: source_identity_to_domain(request.target)?,
            allow_best_effort: request.allow_best_effort,
            allow_target_overwrite: request.allow_target_overwrite,
        };
        migration_result_to_dto(self.migrate(request).await?)
    }

    /// IPC 入口：按当前已登记章节查询同一 Work 下的其他来源候选。
    /// 候选只由后端依据已持久化来源引用和页面身份计算，前端不能自行拼接
    /// remote ID、URL 或资源定位器。
    pub async fn source_candidates_wire(
        &self,
        request: ComicChapterSourceCandidatesGetRequestDto,
    ) -> Result<ComicChapterSourceCandidatesDto, AppError> {
        let source = source_identity_to_domain(request.source)?;
        self.source_candidates(source).await
    }

    /// 查询当前章节所属 Work 下的来源引用，并为每个候选生成后端匹配证据。
    /// 该查询是只读的，不会自动合并 Work、Edition、MediaItem 或 Progress。
    pub async fn source_candidates(
        &self,
        source: ChapterSourceIdentity,
    ) -> Result<ComicChapterSourceCandidatesDto, AppError> {
        let source_ref = ChapterSourceRepository::get(&*self.ports, &source)
            .await?
            .ok_or_else(|| source_ref_not_found("当前章节来源身份不存在"))?;
        let current_item = MediaItemRepository::get(&*self.ports, source_ref.media_item_id)
            .await?
            .ok_or_else(|| media_item_not_found("当前漫画媒体条目不存在"))?;
        ensure_comic_media_item(&current_item)?;
        let current_edition = EditionRepository::get(&*self.ports, current_item.edition_id)
            .await?
            .ok_or_else(edition_not_found)?;
        let current_pages =
            ComicPageIdentityRepository::list(&*self.ports, current_item.id).await?;
        let mut pages_by_media_item = HashMap::new();
        pages_by_media_item.insert(current_item.id, current_pages.clone());

        let editions =
            EditionRepository::list_by_work(&*self.ports, current_edition.work_id).await?;
        let mut matched_candidates = Vec::new();
        let mut truncated = false;
        'editions: for edition in editions {
            let items = MediaItemRepository::list_by_edition(&*self.ports, edition.id).await?;
            for item in items {
                if item.media_type != MediaType::Comic {
                    continue;
                }
                let references =
                    ChapterSourceRepository::list_for_media_item(&*self.ports, item.id).await?;
                for reference in references {
                    if reference.identity == source {
                        continue;
                    }
                    let pages = if let Some(pages) = pages_by_media_item.get(&item.id) {
                        pages.clone()
                    } else {
                        let pages =
                            ComicPageIdentityRepository::list(&*self.ports, item.id).await?;
                        pages_by_media_item.insert(item.id, pages.clone());
                        pages
                    };
                    let match_result = if source_ref.media_item_id == reference.media_item_id {
                        compare_chapters_within_media_item(
                            &source_ref.identity,
                            &source_ref.metadata,
                            &current_pages,
                            &reference.identity,
                            &reference.metadata,
                            &pages,
                        )
                    } else {
                        compare_chapters(
                            &source_ref.identity,
                            &source_ref.metadata,
                            &current_pages,
                            &reference.identity,
                            &reference.metadata,
                            &pages,
                        )
                    };
                    matched_candidates.push((reference, match_result));
                    if matched_candidates.len() > MAX_SOURCE_CANDIDATES {
                        truncated = true;
                        break 'editions;
                    }
                }
            }
        }

        matched_candidates.sort_by(|(left_ref, left_match), (right_ref, right_match)| {
            chapter_match_rank(left_match)
                .cmp(&chapter_match_rank(right_match))
                .then_with(|| {
                    left_ref
                        .identity
                        .source_key
                        .cmp(&right_ref.identity.source_key)
                })
                .then_with(|| {
                    left_ref
                        .identity
                        .remote_work_id
                        .cmp(&right_ref.identity.remote_work_id)
                })
                .then_with(|| {
                    left_ref
                        .identity
                        .remote_chapter_id
                        .cmp(&right_ref.identity.remote_chapter_id)
                })
        });
        matched_candidates.truncate(MAX_SOURCE_CANDIDATES);

        let candidates = matched_candidates
            .into_iter()
            .map(|(reference, match_result)| source_candidate_to_dto(&reference, match_result))
            .collect::<Result<Vec<_>, _>>()?;
        Ok(ComicChapterSourceCandidatesDto {
            schema_version: 1,
            source: source_identity_to_dto(&source),
            current_media_item_id: current_item.id.to_string(),
            candidates,
            truncated,
        })
    }

    /// IPC 入口：只接受后端之前发出的快照 ID 和应用 revision，撤销仍由基础设施
    /// 以当前目标 revision CAS 保护。
    pub async fn revert_wire(
        &self,
        request: ComicProgressMigrationRevertRequestDto,
    ) -> Result<ComicProgressMigrationRevertResultDto, AppError> {
        let migration_id = parse_canonical_migration_id(&request.migration_id)?;
        let expected_revision = opaque_value(
            request.expected_applied_revision,
            "expectedAppliedRevision",
            MAX_REVISION_LENGTH,
        )?;
        let reverted = self.revert(migration_id, &expected_revision).await?;
        Ok(ComicProgressMigrationRevertResultDto { reverted })
    }

    /// 根据两个已登记的章节来源身份进行一次换源迁移。
    pub async fn migrate(
        &self,
        request: ComicProgressMigrationRequest,
    ) -> Result<ComicProgressMigrationResult, AppError> {
        let source_ref = ChapterSourceRepository::get(&*self.ports, &request.source)
            .await?
            .ok_or_else(|| source_ref_not_found("源章节来源身份不存在"))?;
        let target_ref = ChapterSourceRepository::get(&*self.ports, &request.target)
            .await?
            .ok_or_else(|| source_ref_not_found("目标章节来源身份不存在"))?;
        let source_item = MediaItemRepository::get(&*self.ports, source_ref.media_item_id)
            .await?
            .ok_or_else(|| media_item_not_found("源漫画媒体条目不存在"))?;
        ensure_comic_media_item(&source_item)?;
        let source_edition = EditionRepository::get(&*self.ports, source_item.edition_id)
            .await?
            .ok_or_else(edition_not_found)?;
        let target_item = MediaItemRepository::get(&*self.ports, target_ref.media_item_id)
            .await?
            .ok_or_else(|| media_item_not_found("目标漫画媒体条目不存在"))?;
        ensure_comic_media_item(&target_item)?;
        let target_edition = EditionRepository::get(&*self.ports, target_item.edition_id)
            .await?
            .ok_or_else(edition_not_found)?;
        if source_edition.work_id != target_edition.work_id {
            return Err(comic_work_mismatch());
        }
        let source_page_identities =
            ComicPageIdentityRepository::list(&*self.ports, source_ref.media_item_id).await?;
        let target_page_identities =
            ComicPageIdentityRepository::list(&*self.ports, target_ref.media_item_id).await?;
        let match_result = if source_ref.media_item_id == target_ref.media_item_id {
            compare_chapters_within_media_item(
                &source_ref.identity,
                &source_ref.metadata,
                &source_page_identities,
                &target_ref.identity,
                &target_ref.metadata,
                &target_page_identities,
            )
        } else {
            compare_chapters(
                &source_ref.identity,
                &source_ref.metadata,
                &source_page_identities,
                &target_ref.identity,
                &target_ref.metadata,
                &target_page_identities,
            )
        };
        let no_page = PageMigration {
            target_page_index: None,
            confidence: PageMappingConfidence::Low,
            strategy: haven_domain::comic_identity::PageMappingStrategy::NoTarget,
            reversible: true,
        };

        match match_result.progress_migration {
            ProgressMigrationMode::Shared
                if source_ref.media_item_id == target_ref.media_item_id =>
            {
                return Ok(ComicProgressMigrationResult {
                    status: ComicProgressMigrationStatus::SharedContent,
                    match_result: Some(match_result),
                    page_migration: no_page,
                    snapshot_id: None,
                    applied_revision: None,
                });
            }
            ProgressMigrationMode::None => {
                return Ok(ComicProgressMigrationResult {
                    status: ComicProgressMigrationStatus::NotApplicable,
                    match_result: Some(match_result),
                    page_migration: no_page,
                    snapshot_id: None,
                    applied_revision: None,
                });
            }
            ProgressMigrationMode::Suggested if !request.allow_best_effort => {
                return Ok(ComicProgressMigrationResult {
                    status: ComicProgressMigrationStatus::Suggested,
                    match_result: Some(match_result),
                    page_migration: no_page,
                    snapshot_id: None,
                    applied_revision: None,
                });
            }
            // `Shared` across two MediaItems still needs a one-time bridge until
            // the caller has explicitly converged both source refs onto one
            // MediaItem. `Suggested` is allowed only when the caller has opted
            // into best-effort behavior; the snapshot/rollback and CAS make weak
            // evidence reversible without pretending that it proves exact
            // content identity.
            ProgressMigrationMode::Shared
            | ProgressMigrationMode::OneTime
            | ProgressMigrationMode::Suggested => {}
        }

        let source_progress =
            ProgressRepository::get_for_media_item(&*self.ports, source_ref.media_item_id).await?;
        let Some(source_progress) = source_progress else {
            return Ok(ComicProgressMigrationResult {
                status: ComicProgressMigrationStatus::NoSourceProgress,
                match_result: Some(match_result),
                page_migration: no_page,
                snapshot_id: None,
                applied_revision: None,
            });
        };
        ensure_comic_progress(&source_progress, &source_item, &source_edition)?;
        let old_page_index = comic_page_index(&source_progress)?;
        let source_pages = materialize_page_identities(
            source_page_identities,
            source_ref.metadata.page_count.or(source_item.page_count),
        );
        let target_pages = materialize_page_identities(
            target_page_identities,
            target_ref.metadata.page_count.or(target_item.page_count),
        );
        let page_migration = migrate_page_index(&source_pages, &target_pages, old_page_index);
        let Some(target_page_index) = page_migration.target_page_index else {
            return Ok(ComicProgressMigrationResult {
                status: ComicProgressMigrationStatus::NoTargetPage,
                match_result: Some(match_result),
                page_migration,
                snapshot_id: None,
                applied_revision: None,
            });
        };

        let target_progress =
            ProgressRepository::get_for_media_item(&*self.ports, target_ref.media_item_id).await?;
        if let Some(target_progress) = target_progress.as_ref() {
            ensure_comic_progress(target_progress, &target_item, &target_edition)?;
        }
        if target_ref.media_item_id != source_ref.media_item_id
            && target_progress.is_some()
            && !request.allow_target_overwrite
        {
            return Ok(ComicProgressMigrationResult {
                status: ComicProgressMigrationStatus::TargetProgressPreserved,
                match_result: Some(match_result),
                page_migration,
                snapshot_id: None,
                applied_revision: None,
            });
        }

        let new_progress = translated_progress(
            &source_progress,
            &target_item,
            target_edition.work_id,
            target_page_index,
            u32::try_from(target_pages.len()).map_err(|_| {
                AppError::new(
                    "INVALID_COMIC_PAGE_COUNT",
                    ErrorKind::Validation,
                    "漫画页面数量超出迁移范围",
                    false,
                )
            })?,
        )?;
        let source_revision = progress_revision(&source_progress)?.to_owned();
        let target_revision = target_progress
            .as_ref()
            .map(progress_revision)
            .transpose()?
            .map(str::to_owned);
        let migration_id = ComicProgressMigrationId::new();
        let snapshot = ComicProgressMigrationSnapshot {
            id: migration_id,
            source_media_item_id: source_ref.media_item_id,
            target_media_item_id: target_ref.media_item_id,
            source_revision: source_revision.clone(),
            target_revision_before: target_revision.clone(),
            old_progress: source_progress.clone(),
            old_target_progress: if source_ref.media_item_id == target_ref.media_item_id {
                Some(source_progress.clone())
            } else {
                target_progress.clone()
            },
            new_progress,
            mode: match_result.progress_migration,
            confidence: match_confidence(match_result.confidence),
            strategy: page_migration.strategy,
            evidence: match_result.evidence.clone(),
            created_at: UtcMillis::now(),
            applied_revision: None,
            state: ProgressMigrationState::Applied,
            reverted_at: None,
        };
        let applied_revision = ComicProgressMigrationRepository::apply(
            &*self.ports,
            &snapshot,
            &source_revision,
            target_revision.as_deref(),
        )
        .await?
        .ok_or_else(revision_conflict)?;
        Ok(ComicProgressMigrationResult {
            status: ComicProgressMigrationStatus::Applied,
            match_result: Some(match_result),
            page_migration,
            snapshot_id: Some(migration_id),
            applied_revision: Some(applied_revision),
        })
    }

    /// 对同一 MediaItem 的新页面序列重新定位当前 Progress。适用于插页、删页和
    /// 重排；不需要伪造新的章节来源身份。
    pub async fn remap_page_progress(
        &self,
        request: ComicPageProgressRemapRequest,
    ) -> Result<ComicProgressMigrationResult, AppError> {
        let item = MediaItemRepository::get(&*self.ports, request.media_item_id)
            .await?
            .ok_or_else(|| media_item_not_found("漫画媒体条目不存在"))?;
        ensure_comic_media_item(&item)?;
        let edition = EditionRepository::get(&*self.ports, item.edition_id)
            .await?
            .ok_or_else(edition_not_found)?;
        let source_progress = ProgressRepository::get_for_media_item(&*self.ports, item.id).await?;
        let Some(source_progress) = source_progress else {
            return Ok(ComicProgressMigrationResult {
                status: ComicProgressMigrationStatus::NoSourceProgress,
                match_result: None,
                page_migration: PageMigration {
                    target_page_index: None,
                    confidence: PageMappingConfidence::Low,
                    strategy: haven_domain::comic_identity::PageMappingStrategy::NoTarget,
                    reversible: true,
                },
                snapshot_id: None,
                applied_revision: None,
            });
        };
        ensure_comic_progress(&source_progress, &item, &edition)?;
        let page_migration = migrate_page_index(
            &request.old_pages,
            &request.new_pages,
            comic_page_index(&source_progress)?,
        );
        let Some(target_page_index) = page_migration.target_page_index else {
            return Ok(ComicProgressMigrationResult {
                status: ComicProgressMigrationStatus::NoTargetPage,
                match_result: None,
                page_migration,
                snapshot_id: None,
                applied_revision: None,
            });
        };
        let new_progress = translated_progress(
            &source_progress,
            &item,
            edition.work_id,
            target_page_index,
            request.new_pages.len() as u32,
        )?;
        let current_revision = progress_revision(&source_progress)?;
        let expected_revision = request
            .expected_revision
            .unwrap_or_else(|| current_revision.to_owned());
        if expected_revision != current_revision {
            return Err(revision_conflict());
        }
        let migration_id = ComicProgressMigrationId::new();
        let snapshot = ComicProgressMigrationSnapshot {
            id: migration_id,
            source_media_item_id: item.id,
            target_media_item_id: item.id,
            source_revision: expected_revision.clone(),
            target_revision_before: Some(expected_revision.clone()),
            old_progress: source_progress.clone(),
            old_target_progress: Some(source_progress.clone()),
            new_progress,
            mode: ProgressMigrationMode::OneTime,
            confidence: page_migration.confidence,
            strategy: page_migration.strategy,
            evidence: Vec::new(),
            created_at: UtcMillis::now(),
            applied_revision: None,
            state: ProgressMigrationState::Applied,
            reverted_at: None,
        };
        let applied_revision = ComicProgressMigrationRepository::apply(
            &*self.ports,
            &snapshot,
            &expected_revision,
            Some(&expected_revision),
        )
        .await?
        .ok_or_else(revision_conflict)?;
        Ok(ComicProgressMigrationResult {
            status: ComicProgressMigrationStatus::Applied,
            match_result: None,
            page_migration,
            snapshot_id: Some(migration_id),
            applied_revision: Some(applied_revision),
        })
    }

    pub async fn revert(
        &self,
        migration_id: ComicProgressMigrationId,
        expected_applied_revision: &str,
    ) -> Result<bool, AppError> {
        ComicProgressMigrationRepository::revert(
            &*self.ports,
            migration_id,
            expected_applied_revision,
        )
        .await
    }
}

fn translated_progress(
    old: &Progress,
    target_item: &MediaItem,
    target_work_id: haven_domain::ids::WorkId,
    target_page_index: u32,
    target_page_count: u32,
) -> Result<Progress, AppError> {
    let percentage = (target_page_count > 0).then(|| {
        ((target_page_index.saturating_add(1)) as f32 / target_page_count as f32).min(1.0)
    });
    Ok(Progress {
        id: ProgressId::new(),
        work_id: target_work_id,
        edition_id: target_item.edition_id,
        media_item_id: target_item.id,
        locator: Locator::Comic(ComicLocator {
            chapter_item_id: target_item.id,
            page_index: target_page_index,
            page_progression: comic_progression(old),
        }),
        completion: old.completion,
        percentage,
        last_active_at: old.last_active_at,
        updated_at: UtcMillis::now(),
        revision: None,
        // 页面发生了来源/序列变化，旧帧不再保证对应目标页。
        keyframe_uri: None,
    })
}

fn comic_progression(progress: &Progress) -> Option<f32> {
    match &progress.locator {
        Locator::Comic(locator) => locator.page_progression,
        _ => None,
    }
}

fn progress_revision(progress: &Progress) -> Result<&str, AppError> {
    progress
        .revision
        .as_deref()
        .filter(|value| !value.is_empty())
        .ok_or_else(|| {
            AppError::new(
                "PROGRESS_REVISION_MISSING",
                ErrorKind::Database,
                "Progress 缺少持久化 revision",
                false,
            )
        })
}

fn comic_page_index(progress: &Progress) -> Result<u32, AppError> {
    match &progress.locator {
        Locator::Comic(locator) if locator.chapter_item_id == progress.media_item_id => {
            Ok(locator.page_index)
        }
        _ => Err(AppError::new(
            "COMIC_PROGRESS_REQUIRED",
            ErrorKind::Validation,
            "进度不是与媒体条目一致的 Comic Locator",
            false,
        )),
    }
}

fn ensure_comic_progress(
    progress: &Progress,
    item: &MediaItem,
    edition: &haven_domain::entities::Edition,
) -> Result<(), AppError> {
    if progress.work_id != edition.work_id
        || progress.edition_id != item.edition_id
        || progress.media_item_id != item.id
    {
        return Err(AppError::new(
            "COMIC_PROGRESS_REQUIRED",
            ErrorKind::Validation,
            "漫画进度的 Work、Edition 或媒体条目不一致",
            false,
        ));
    }
    let _ = comic_page_index(progress)?;
    Ok(())
}

fn ensure_comic_media_item(item: &MediaItem) -> Result<(), AppError> {
    if item.media_type != MediaType::Comic {
        return Err(AppError::new(
            "COMIC_MEDIA_ITEM_REQUIRED",
            ErrorKind::Validation,
            "进度迁移只能处理漫画媒体条目",
            false,
        ));
    }
    Ok(())
}

fn match_confidence(value: MatchConfidence) -> PageMappingConfidence {
    match value {
        MatchConfidence::High => PageMappingConfidence::High,
        MatchConfidence::Medium => PageMappingConfidence::Medium,
        MatchConfidence::Low => PageMappingConfidence::Low,
    }
}

const MAX_SYNTHETIC_PAGE_IDENTITIES: u32 = 5_000;
const MAX_SOURCE_CANDIDATES: usize = 500;
const MAX_OPAQUE_VALUE_LENGTH: usize = 4_096;
const MAX_REVISION_LENGTH: usize = 256;

fn source_identity_to_domain(
    value: ComicChapterSourceIdentityDto,
) -> Result<ChapterSourceIdentity, AppError> {
    let source_key = opaque_value(value.source_id, "sourceId", MAX_OPAQUE_VALUE_LENGTH)?;
    let remote_work_id = opaque_value(
        value.remote_work_id,
        "remoteWorkId",
        MAX_OPAQUE_VALUE_LENGTH,
    )?;
    let remote_chapter_id = opaque_value(
        value.remote_chapter_id,
        "remoteChapterId",
        MAX_OPAQUE_VALUE_LENGTH,
    )?;
    ChapterSourceIdentity::new(source_key, remote_work_id, remote_chapter_id)
        .ok_or_else(|| invalid_wire("source"))
}

fn opaque_value(value: String, field: &'static str, max_length: usize) -> Result<String, AppError> {
    let trimmed = value.trim();
    if has_opaque_control_character(&value)
        || trimmed.is_empty()
        || trimmed.len() > max_length
        || trimmed.contains("://")
        || trimmed.to_ascii_lowercase().starts_with("data:")
    {
        return Err(invalid_wire(field));
    }
    Ok(trimmed.to_owned())
}

fn parse_canonical_migration_id(value: &str) -> Result<ComicProgressMigrationId, AppError> {
    let id: ComicProgressMigrationId = value.parse().map_err(|_| invalid_wire("migrationId"))?;
    if id.to_string() != value {
        return Err(invalid_wire("migrationId"));
    }
    Ok(id)
}

fn source_identity_to_dto(value: &ChapterSourceIdentity) -> ComicChapterSourceIdentityDto {
    ComicChapterSourceIdentityDto {
        source_id: value.source_key.clone(),
        remote_work_id: value.remote_work_id.clone(),
        remote_chapter_id: value.remote_chapter_id.clone(),
    }
}

fn chapter_match_rank(value: &ChapterMatch) -> u8 {
    match value.kind {
        ChapterMatchKind::SameRemoteChapter => 0,
        ChapterMatchKind::SameContent => 1,
        ChapterMatchKind::SameLogicalChapterVariant => 2,
        ChapterMatchKind::Candidate => 3,
        ChapterMatchKind::Unrelated => 4,
    }
}

fn source_candidate_to_dto(
    reference: &haven_domain::comic_identity::ChapterSourceRef,
    match_result: ChapterMatch,
) -> Result<ComicChapterSourceCandidateDto, AppError> {
    let registered = crate::services::comic_catalog::registered_chapter_to_dto(reference);
    Ok(ComicChapterSourceCandidateDto {
        source: source_identity_to_dto(&reference.identity),
        media_item_id: registered.media_item_id,
        chapter_number: registered.chapter_number,
        volume_number: registered.volume_number,
        title: registered.title,
        page_count: registered.page_count,
        source_order: registered.source_order,
        availability: registered.availability,
        published_at: registered.published_at,
        source_updated_at: registered.source_updated_at,
        last_seen_generation: registered.last_seen_generation,
        edition_profile: registered.edition_profile,
        match_result: chapter_match_to_dto(match_result)?,
    })
}

fn invalid_wire(field: &'static str) -> AppError {
    AppError::new(
        "INVALID_ARGUMENT",
        ErrorKind::Validation,
        format!("漫画进度迁移字段 {field} 非法"),
        false,
    )
}

pub fn migration_result_to_dto(
    result: ComicProgressMigrationResult,
) -> Result<ComicProgressMigrationResultDto, AppError> {
    Ok(ComicProgressMigrationResultDto {
        status: migration_status_to_dto(result.status),
        match_result: result.match_result.map(chapter_match_to_dto).transpose()?,
        page_migration: page_migration_to_dto(result.page_migration),
        snapshot_id: result.snapshot_id.map(|value| value.to_string()),
        applied_revision: result.applied_revision,
    })
}

fn migration_status_to_dto(value: ComicProgressMigrationStatus) -> ComicProgressMigrationStatusDto {
    match value {
        ComicProgressMigrationStatus::Unchanged => ComicProgressMigrationStatusDto::Unchanged,
        ComicProgressMigrationStatus::NotApplicable => {
            ComicProgressMigrationStatusDto::NotApplicable
        }
        ComicProgressMigrationStatus::Applied => ComicProgressMigrationStatusDto::Applied,
        ComicProgressMigrationStatus::SharedContent => {
            ComicProgressMigrationStatusDto::SharedContent
        }
        ComicProgressMigrationStatus::Suggested => ComicProgressMigrationStatusDto::Suggested,
        ComicProgressMigrationStatus::NoSourceProgress => {
            ComicProgressMigrationStatusDto::NoSourceProgress
        }
        ComicProgressMigrationStatus::TargetProgressPreserved => {
            ComicProgressMigrationStatusDto::TargetProgressPreserved
        }
        ComicProgressMigrationStatus::NoTargetPage => ComicProgressMigrationStatusDto::NoTargetPage,
    }
}

fn chapter_match_to_dto(value: ChapterMatch) -> Result<ComicChapterMatchDto, AppError> {
    Ok(ComicChapterMatchDto {
        kind: match value.kind {
            ChapterMatchKind::SameRemoteChapter => ComicChapterMatchKindDto::SameRemoteChapter,
            ChapterMatchKind::SameContent => ComicChapterMatchKindDto::SameContent,
            ChapterMatchKind::SameLogicalChapterVariant => {
                ComicChapterMatchKindDto::SameLogicalChapterVariant
            }
            ChapterMatchKind::Candidate => ComicChapterMatchKindDto::Candidate,
            ChapterMatchKind::Unrelated => ComicChapterMatchKindDto::Unrelated,
        },
        confidence: match_confidence_to_dto(value.confidence),
        progress_migration: migration_mode_to_dto(value.progress_migration),
        evidence: value
            .evidence
            .into_iter()
            .map(chapter_evidence_to_dto)
            .collect::<Result<Vec<_>, _>>()?,
    })
}

fn match_confidence_to_dto(value: MatchConfidence) -> ComicMatchConfidenceDto {
    match value {
        MatchConfidence::High => ComicMatchConfidenceDto::High,
        MatchConfidence::Medium => ComicMatchConfidenceDto::Medium,
        MatchConfidence::Low => ComicMatchConfidenceDto::Low,
    }
}

fn migration_mode_to_dto(value: ProgressMigrationMode) -> ComicProgressMigrationModeDto {
    match value {
        ProgressMigrationMode::Shared => ComicProgressMigrationModeDto::Shared,
        ProgressMigrationMode::OneTime => ComicProgressMigrationModeDto::OneTime,
        ProgressMigrationMode::Suggested => ComicProgressMigrationModeDto::Suggested,
        ProgressMigrationMode::None => ComicProgressMigrationModeDto::None,
    }
}

fn chapter_evidence_to_dto(value: ChapterEvidence) -> Result<ComicChapterEvidenceDto, AppError> {
    let (kind, matched) = match value {
        ChapterEvidence::SameRemoteIdentity => {
            (ComicChapterEvidenceKindDto::SameRemoteIdentity, None)
        }
        ChapterEvidence::AuthoritativeContentKey => {
            (ComicChapterEvidenceKindDto::AuthoritativeContentKey, None)
        }
        ChapterEvidence::ConflictingAuthoritativeContentKey => (
            ComicChapterEvidenceKindDto::ConflictingAuthoritativeContentKey,
            None,
        ),
        ChapterEvidence::EditionCompatible => {
            (ComicChapterEvidenceKindDto::EditionCompatible, None)
        }
        ChapterEvidence::EditionConflict => (ComicChapterEvidenceKindDto::EditionConflict, None),
        ChapterEvidence::ExactPageIdentity { matched } => (
            ComicChapterEvidenceKindDto::ExactPageIdentity,
            Some(u32::try_from(matched).map_err(|_| invalid_wire("evidence.matched"))?),
        ),
        ChapterEvidence::PartialPageIdentity { matched } => (
            ComicChapterEvidenceKindDto::PartialPageIdentity,
            Some(u32::try_from(matched).map_err(|_| invalid_wire("evidence.matched"))?),
        ),
        ChapterEvidence::MatchingChapterMetadata => {
            (ComicChapterEvidenceKindDto::MatchingChapterMetadata, None)
        }
        ChapterEvidence::WeakChapterMetadata => {
            (ComicChapterEvidenceKindDto::WeakChapterMetadata, None)
        }
    };
    Ok(ComicChapterEvidenceDto { kind, matched })
}

fn page_migration_to_dto(value: PageMigration) -> ComicPageMigrationDto {
    ComicPageMigrationDto {
        target_page_index: value.target_page_index,
        confidence: match value.confidence {
            PageMappingConfidence::High => ComicPageMappingConfidenceDto::High,
            PageMappingConfidence::Medium => ComicPageMappingConfidenceDto::Medium,
            PageMappingConfidence::Low => ComicPageMappingConfidenceDto::Low,
        },
        strategy: match value.strategy {
            PageMappingStrategy::StableKey => ComicPageMappingStrategyDto::StableKey,
            PageMappingStrategy::ContentFingerprint => {
                ComicPageMappingStrategyDto::ContentFingerprint
            }
            PageMappingStrategy::ReorderedAnchor => ComicPageMappingStrategyDto::ReorderedAnchor,
            PageMappingStrategy::NearestSurvivingPage => {
                ComicPageMappingStrategyDto::NearestSurvivingPage
            }
            PageMappingStrategy::ProportionalFallback => {
                ComicPageMappingStrategyDto::ProportionalFallback
            }
            PageMappingStrategy::NoTarget => ComicPageMappingStrategyDto::NoTarget,
        },
        reversible: value.reversible,
    }
}

/// 页面身份表尚未建立时，使用已验证的章节页数提供可解释的比例兜底。
/// 只在章节来源元数据或 MediaItem 已知页数且不超过本地漫画上限时生成空身份；
/// 空身份仍会被领域算法标记为 Low，不会伪装成稳定页面证据。
fn materialize_page_identities(
    pages: Vec<PageIdentity>,
    page_count: Option<u32>,
) -> Vec<PageIdentity> {
    if !pages.is_empty() {
        return pages;
    }
    let Some(page_count) =
        page_count.filter(|count| *count > 0 && *count <= MAX_SYNTHETIC_PAGE_IDENTITIES)
    else {
        return pages;
    };
    vec![PageIdentity::default(); page_count as usize]
}

fn source_ref_not_found(message: &'static str) -> AppError {
    AppError::new(
        "COMIC_SOURCE_REF_NOT_FOUND",
        ErrorKind::NotFound,
        message,
        false,
    )
}

fn media_item_not_found(message: &'static str) -> AppError {
    AppError::new("MEDIA_ITEM_NOT_FOUND", ErrorKind::NotFound, message, false)
}

fn edition_not_found() -> AppError {
    AppError::new(
        "EDITION_NOT_FOUND",
        ErrorKind::NotFound,
        "版本不存在",
        false,
    )
}

fn comic_work_mismatch() -> AppError {
    AppError::new(
        "COMIC_WORK_MISMATCH",
        ErrorKind::Validation,
        "漫画来源章节不属于同一个 Work，拒绝迁移进度",
        false,
    )
}

fn revision_conflict() -> AppError {
    AppError::new(
        "REVISION_CONFLICT",
        ErrorKind::Conflict,
        "漫画进度已被其他会话更新，请刷新后重试",
        false,
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use haven_domain::comic_identity::{
        ChapterSourceRef, ComicChapterMetadata, IdentityFacet, PageMappingConfidence,
        PageMappingStrategy,
    };
    use haven_domain::contracts::{
        ChapterSourceRepository, ComicPageIdentityRepository, EditionRepository,
        MediaItemRepository, ProgressRepository, WorkRepository,
    };
    use haven_domain::entities::{ArtworkSet, Edition, MediaIndex, MediaItem, Progress, Work};
    use haven_domain::enums::{CompletionState, MediaItemStatus, MediaType, WorkStatus, WorkType};
    use haven_domain::ids::{EditionId, WorkId};
    use haven_domain::locator::ComicLocator;
    use haven_infrastructure::db::Db;
    use haven_infrastructure::db::repos::SqliteRepositories;

    async fn seed_fixture() -> (
        Arc<SqliteRepositories>,
        ChapterSourceIdentity,
        ChapterSourceIdentity,
        WorkId,
        EditionId,
        EditionId,
        MediaItemId,
        MediaItemId,
    ) {
        let db = Arc::new(Db::open_in_memory().unwrap());
        let repositories = Arc::new(SqliteRepositories::new(db));
        let work_id = WorkId::new();
        let source_edition = EditionId::new();
        let target_edition = EditionId::new();
        repositories
            .work
            .save(&Work {
                id: work_id,
                canonical_title: "换源测试".into(),
                original_title: None,
                sort_title: None,
                description: None,
                work_type: WorkType::Fiction,
                release_year: None,
                language: Some("zh-cn".into()),
                director: None,
                actor: None,
                status: WorkStatus::Completed,
                rating_value: None,
                rating_scale: None,
                artwork: ArtworkSet::default(),
                created_at: UtcMillis(1),
                updated_at: UtcMillis(1),
            })
            .await
            .unwrap();
        for (id, title) in [(source_edition, "源版本"), (target_edition, "目标版本")] {
            repositories
                .edition
                .save(&Edition {
                    id,
                    work_id,
                    title: title.into(),
                    subtitle: None,
                    edition_type: MediaType::Comic,
                    release_date: None,
                    language: Some("zh-cn".into()),
                    region: None,
                    publisher_or_studio: None,
                    description: None,
                    artwork: ArtworkSet::default(),
                    created_at: UtcMillis(1),
                    updated_at: UtcMillis(1),
                })
                .await
                .unwrap();
        }
        let source_media = MediaItemId::new();
        let target_media = MediaItemId::new();
        for (id, edition_id) in [
            (source_media, source_edition),
            (target_media, target_edition),
        ] {
            repositories
                .media_item
                .save(&MediaItem {
                    id,
                    edition_id,
                    parent_id: None,
                    media_type: MediaType::Comic,
                    title: "第 12 话".into(),
                    index: MediaIndex::Chapter {
                        volume: None,
                        chapter: 12.0,
                    },
                    duration_ms: None,
                    page_count: Some(3),
                    chapter_count: None,
                    published_at: None,
                    status: MediaItemStatus::Available,
                    created_at: UtcMillis(1),
                    updated_at: UtcMillis(1),
                })
                .await
                .unwrap();
        }
        (
            repositories,
            ChapterSourceIdentity::new("source-a", "work-a", "chapter-a").unwrap(),
            ChapterSourceIdentity::new("source-b", "work-b", "chapter-b").unwrap(),
            work_id,
            source_edition,
            target_edition,
            source_media,
            target_media,
        )
    }

    async fn configure_fixture(
        repositories: Arc<SqliteRepositories>,
        source: ChapterSourceIdentity,
        target: ChapterSourceIdentity,
        work_id: WorkId,
        source_edition: EditionId,
        source_media: MediaItemId,
        target_media: MediaItemId,
    ) -> (ComicProgressMigrationService, Arc<SqliteRepositories>) {
        let source_ref = ChapterSourceRef {
            media_item_id: source_media,
            identity: source.clone(),
            metadata: ComicChapterMetadata {
                chapter_number: Some(12.0),
                title: Some("第 12 话".into()),
                page_count: Some(3),
                ..ComicChapterMetadata::default()
            },
            source_order: 0,
            availability: haven_domain::comic_catalog::ComicChapterSourceStatus::Available,
            published_at: None,
            source_updated_at: None,
            last_seen_generation: None,
            updated_at: UtcMillis(2),
        };
        let target_ref = ChapterSourceRef {
            media_item_id: target_media,
            identity: target.clone(),
            metadata: ComicChapterMetadata {
                chapter_number: Some(12.0),
                title: Some("第 12 话".into()),
                page_count: Some(3),
                ..ComicChapterMetadata::default()
            },
            source_order: 0,
            availability: haven_domain::comic_catalog::ComicChapterSourceStatus::Available,
            published_at: None,
            source_updated_at: None,
            last_seen_generation: None,
            updated_at: UtcMillis(2),
        };
        repositories.chapter_source.save(&source_ref).await.unwrap();
        repositories.chapter_source.save(&target_ref).await.unwrap();
        repositories
            .page_identity
            .replace(
                source_media,
                &[
                    PageIdentity::stable("a").with_fingerprint("page-a"),
                    PageIdentity::stable("removed").with_fingerprint("page-removed"),
                    PageIdentity::stable("c").with_fingerprint("page-c"),
                ],
                UtcMillis(2),
            )
            .await
            .unwrap();
        repositories
            .page_identity
            .replace(
                target_media,
                &[
                    PageIdentity::stable("a").with_fingerprint("page-a"),
                    PageIdentity::stable("c").with_fingerprint("page-c"),
                    PageIdentity::stable("d").with_fingerprint("page-d"),
                ],
                UtcMillis(2),
            )
            .await
            .unwrap();
        let source_progress = Progress {
            id: ProgressId::new(),
            work_id,
            edition_id: source_edition,
            media_item_id: source_media,
            locator: Locator::Comic(ComicLocator {
                chapter_item_id: source_media,
                page_index: 1,
                page_progression: Some(0.5),
            }),
            completion: CompletionState::InProgress,
            percentage: Some(0.5),
            last_active_at: UtcMillis(10),
            updated_at: UtcMillis(10),
            revision: None,
            keyframe_uri: None,
        };
        repositories
            .progress
            .save_if_revision(&source_progress, None)
            .await
            .unwrap();
        let service = ComicProgressMigrationService::new(repositories.clone());
        (service, repositories)
    }

    #[tokio::test]
    async fn migration_maps_deleted_page_and_can_revert() {
        let (
            repositories,
            source,
            target,
            work_id,
            source_edition,
            _target_edition,
            source_media,
            target_media,
        ) = seed_fixture().await;
        let (service, repositories) = configure_fixture(
            repositories,
            source.clone(),
            target.clone(),
            work_id,
            source_edition,
            source_media,
            target_media,
        )
        .await;
        let result = service
            .migrate(ComicProgressMigrationRequest {
                source,
                target,
                allow_best_effort: false,
                allow_target_overwrite: false,
            })
            .await
            .unwrap();
        assert_eq!(result.status, ComicProgressMigrationStatus::Applied);
        assert_eq!(result.page_migration.target_page_index, Some(1));
        assert_eq!(
            result.page_migration.strategy,
            PageMappingStrategy::NearestSurvivingPage
        );
        let snapshot_id = result.snapshot_id.unwrap();
        let target_progress = repositories
            .progress
            .get_for_media_item(target_media)
            .await
            .unwrap()
            .unwrap();
        assert!(matches!(
            target_progress.locator,
            Locator::Comic(ComicLocator { page_index: 1, .. })
        ));
        let applied_revision = result.applied_revision.clone().unwrap();
        assert!(
            service
                .revert(snapshot_id, &applied_revision)
                .await
                .unwrap()
        );
        assert!(
            repositories
                .progress
                .get_for_media_item(target_media)
                .await
                .unwrap()
                .is_none()
        );
    }

    #[tokio::test]
    async fn existing_target_progress_is_preserved_without_explicit_overwrite() {
        let (
            repositories,
            source,
            target,
            work_id,
            source_edition,
            _target_edition,
            source_media,
            target_media,
        ) = seed_fixture().await;
        let (service, repositories) = configure_fixture(
            repositories,
            source.clone(),
            target.clone(),
            work_id,
            source_edition,
            source_media,
            target_media,
        )
        .await;
        let target_item = repositories
            .media_item
            .get(target_media)
            .await
            .unwrap()
            .unwrap();
        let target_edition = repositories
            .edition
            .get(target_item.edition_id)
            .await
            .unwrap()
            .unwrap();
        repositories
            .progress
            .save_if_revision(
                &Progress {
                    id: ProgressId::new(),
                    work_id: target_edition.work_id,
                    edition_id: target_edition.id,
                    media_item_id: target_media,
                    locator: Locator::Comic(ComicLocator {
                        chapter_item_id: target_media,
                        page_index: 2,
                        page_progression: None,
                    }),
                    completion: CompletionState::InProgress,
                    percentage: Some(0.9),
                    last_active_at: UtcMillis(20),
                    updated_at: UtcMillis(20),
                    revision: None,
                    keyframe_uri: None,
                },
                None,
            )
            .await
            .unwrap();
        let result = service
            .migrate(ComicProgressMigrationRequest {
                source,
                target,
                allow_best_effort: false,
                allow_target_overwrite: false,
            })
            .await
            .unwrap();
        assert_eq!(
            result.status,
            ComicProgressMigrationStatus::TargetProgressPreserved
        );
        let stored = repositories
            .progress
            .get_for_media_item(target_media)
            .await
            .unwrap()
            .unwrap();
        assert!(matches!(
            stored.locator,
            Locator::Comic(ComicLocator { page_index: 2, .. })
        ));
    }

    #[tokio::test]
    async fn weak_chapter_match_requires_explicit_best_effort_confirmation() {
        let (
            repositories,
            source,
            target,
            work_id,
            source_edition,
            _target_edition,
            source_media,
            target_media,
        ) = seed_fixture().await;
        let (service, repositories) = configure_fixture(
            repositories,
            source.clone(),
            target.clone(),
            work_id,
            source_edition,
            source_media,
            target_media,
        )
        .await;
        repositories
            .page_identity
            .replace(source_media, &[], UtcMillis(3))
            .await
            .unwrap();
        repositories
            .page_identity
            .replace(target_media, &[], UtcMillis(3))
            .await
            .unwrap();

        let result = service
            .migrate(ComicProgressMigrationRequest {
                source,
                target,
                allow_best_effort: false,
                allow_target_overwrite: false,
            })
            .await
            .unwrap();
        assert_eq!(result.status, ComicProgressMigrationStatus::Suggested);
        assert_eq!(result.page_migration.target_page_index, None);
        assert!(result.snapshot_id.is_none());
        assert!(result.applied_revision.is_none());
        assert!(
            repositories
                .progress
                .get_for_media_item(target_media)
                .await
                .unwrap()
                .is_none()
        );
    }

    #[tokio::test]
    async fn weak_chapter_match_uses_page_count_fallback_and_keeps_snapshot_when_confirmed() {
        let (
            repositories,
            source,
            target,
            work_id,
            source_edition,
            _target_edition,
            source_media,
            target_media,
        ) = seed_fixture().await;
        let (service, repositories) = configure_fixture(
            repositories,
            source.clone(),
            target.clone(),
            work_id,
            source_edition,
            source_media,
            target_media,
        )
        .await;
        repositories
            .page_identity
            .replace(source_media, &[], UtcMillis(3))
            .await
            .unwrap();
        repositories
            .page_identity
            .replace(target_media, &[], UtcMillis(3))
            .await
            .unwrap();

        let result = service
            .migrate(ComicProgressMigrationRequest {
                source,
                target,
                allow_best_effort: true,
                allow_target_overwrite: false,
            })
            .await
            .unwrap();
        assert_eq!(result.status, ComicProgressMigrationStatus::Applied);
        assert_eq!(
            result
                .match_result
                .as_ref()
                .map(|matched| matched.progress_migration),
            Some(ProgressMigrationMode::Suggested)
        );
        assert_eq!(result.page_migration.target_page_index, Some(1));
        assert_eq!(
            result.page_migration.strategy,
            PageMappingStrategy::ProportionalFallback
        );
        assert_eq!(result.page_migration.confidence, PageMappingConfidence::Low);
        assert!(result.snapshot_id.is_some());
        assert!(
            repositories
                .progress
                .get_for_media_item(target_media)
                .await
                .unwrap()
                .is_some()
        );
    }

    #[tokio::test]
    async fn exact_content_on_separate_media_items_bridges_progress_once() {
        let (
            repositories,
            source,
            target,
            work_id,
            source_edition,
            _target_edition,
            source_media,
            target_media,
        ) = seed_fixture().await;
        let (service, repositories) = configure_fixture(
            repositories,
            source.clone(),
            target.clone(),
            work_id,
            source_edition,
            source_media,
            target_media,
        )
        .await;
        for identity in [source.clone(), target.clone()] {
            let mut reference = repositories
                .chapter_source
                .get(&identity)
                .await
                .unwrap()
                .unwrap();
            reference.metadata.authoritative_content_key = Some("same-content".into());
            repositories.chapter_source.save(&reference).await.unwrap();
        }

        let result = service
            .migrate(ComicProgressMigrationRequest {
                source,
                target,
                allow_best_effort: false,
                allow_target_overwrite: false,
            })
            .await
            .unwrap();
        assert_eq!(result.status, ComicProgressMigrationStatus::Applied);
        assert_eq!(
            result.match_result.as_ref().map(|matched| matched.kind),
            Some(haven_domain::comic_identity::ChapterMatchKind::SameContent)
        );
        assert!(result.snapshot_id.is_some());
        assert!(
            repositories
                .progress
                .get_for_media_item(target_media)
                .await
                .unwrap()
                .is_some()
        );
    }

    #[tokio::test]
    async fn unrelated_chapter_match_is_not_reported_as_suggested() {
        let (
            repositories,
            source,
            target,
            work_id,
            source_edition,
            _target_edition,
            source_media,
            target_media,
        ) = seed_fixture().await;
        let (service, repositories) = configure_fixture(
            repositories,
            source.clone(),
            target.clone(),
            work_id,
            source_edition,
            source_media,
            target_media,
        )
        .await;

        let mut source_ref = repositories
            .chapter_source
            .get(&source)
            .await
            .unwrap()
            .unwrap();
        source_ref.metadata.edition_profile.language = IdentityFacet::known("zh-CN");
        repositories.chapter_source.save(&source_ref).await.unwrap();
        let mut target_ref = repositories
            .chapter_source
            .get(&target)
            .await
            .unwrap()
            .unwrap();
        target_ref.metadata.edition_profile.language = IdentityFacet::known("ja");
        repositories.chapter_source.save(&target_ref).await.unwrap();

        let result = service
            .migrate(ComicProgressMigrationRequest {
                source,
                target,
                allow_best_effort: true,
                allow_target_overwrite: true,
            })
            .await
            .unwrap();
        assert_eq!(result.status, ComicProgressMigrationStatus::NotApplicable);
        assert_eq!(
            result.match_result.as_ref().map(|matched| matched.kind),
            Some(ChapterMatchKind::Unrelated)
        );
        assert!(
            repositories
                .progress
                .get_for_media_item(target_media)
                .await
                .unwrap()
                .is_none()
        );
    }

    #[tokio::test]
    async fn migration_rejects_chapters_from_different_local_works() {
        let (
            repositories,
            source,
            target,
            work_id,
            source_edition,
            target_edition,
            source_media,
            target_media,
        ) = seed_fixture().await;
        let (service, repositories) = configure_fixture(
            repositories,
            source.clone(),
            target.clone(),
            work_id,
            source_edition,
            source_media,
            target_media,
        )
        .await;

        let mut other_work = repositories
            .work
            .get(work_id)
            .await
            .unwrap()
            .expect("fixture Work must exist");
        other_work.id = WorkId::new();
        repositories.work.save(&other_work).await.unwrap();

        let mut other_edition = repositories
            .edition
            .get(target_edition)
            .await
            .unwrap()
            .expect("fixture target Edition must exist");
        other_edition.id = EditionId::new();
        other_edition.work_id = other_work.id;
        repositories.edition.save(&other_edition).await.unwrap();

        let mut other_item = repositories
            .media_item
            .get(target_media)
            .await
            .unwrap()
            .expect("fixture target MediaItem must exist");
        other_item.id = MediaItemId::new();
        other_item.edition_id = other_edition.id;
        repositories.media_item.save(&other_item).await.unwrap();

        let unrelated_target =
            ChapterSourceIdentity::new("source-c", "work-c", "chapter-c").unwrap();
        let mut target_ref = repositories
            .chapter_source
            .get(&target)
            .await
            .unwrap()
            .expect("fixture target source ref must exist");
        target_ref.identity = unrelated_target.clone();
        target_ref.media_item_id = other_item.id;
        repositories.chapter_source.save(&target_ref).await.unwrap();

        let error = service
            .migrate(ComicProgressMigrationRequest {
                source,
                target: unrelated_target,
                allow_best_effort: false,
                allow_target_overwrite: false,
            })
            .await
            .expect_err("跨 Work 的章节不能迁移进度");
        assert_eq!(error.code().as_str(), "COMIC_WORK_MISMATCH");
        assert!(
            repositories
                .progress
                .get_for_media_item(other_item.id)
                .await
                .unwrap()
                .is_none()
        );
    }

    #[tokio::test]
    async fn same_media_page_remap_uses_cas_and_keeps_old_page_reversible() {
        let (
            repositories,
            source,
            _target,
            work_id,
            source_edition,
            _target_edition,
            source_media,
            _target_media,
        ) = seed_fixture().await;
        let (service, repositories) = configure_fixture(
            repositories,
            source.clone(),
            source,
            work_id,
            source_edition,
            source_media,
            source_media,
        )
        .await;
        let expected_revision = repositories
            .progress
            .get_for_media_item(source_media)
            .await
            .unwrap()
            .unwrap()
            .revision
            .expect("fixture progress must have an opaque revision");
        let result = service
            .remap_page_progress(ComicPageProgressRemapRequest {
                media_item_id: source_media,
                old_pages: vec![
                    PageIdentity::stable("a"),
                    PageIdentity::stable("removed"),
                    PageIdentity::stable("c"),
                ],
                new_pages: vec![PageIdentity::stable("a"), PageIdentity::stable("c")],
                expected_revision: Some(expected_revision),
            })
            .await
            .unwrap();
        assert_eq!(result.status, ComicProgressMigrationStatus::Applied);
        assert_eq!(result.page_migration.target_page_index, Some(1));
        let revision = result.applied_revision.unwrap();
        assert!(
            service
                .revert(result.snapshot_id.unwrap(), &revision)
                .await
                .unwrap()
        );
        let restored = repositories
            .progress
            .get_for_media_item(source_media)
            .await
            .unwrap()
            .unwrap();
        assert!(matches!(
            restored.locator,
            Locator::Comic(ComicLocator { page_index: 1, .. })
        ));
    }

    #[tokio::test]
    async fn wire_migration_maps_evidence_and_revert_without_internal_fields() {
        let (
            repositories,
            source,
            target,
            work_id,
            source_edition,
            _target_edition,
            source_media,
            target_media,
        ) = seed_fixture().await;
        let (service, _) = configure_fixture(
            repositories,
            source.clone(),
            target.clone(),
            work_id,
            source_edition,
            source_media,
            target_media,
        )
        .await;

        let result = service
            .migrate_wire(ComicProgressMigrationRequestDto {
                source: ComicChapterSourceIdentityDto {
                    source_id: source.source_key,
                    remote_work_id: source.remote_work_id,
                    remote_chapter_id: source.remote_chapter_id,
                },
                target: ComicChapterSourceIdentityDto {
                    source_id: target.source_key,
                    remote_work_id: target.remote_work_id,
                    remote_chapter_id: target.remote_chapter_id,
                },
                allow_best_effort: false,
                allow_target_overwrite: false,
            })
            .await
            .unwrap();
        assert_eq!(result.status, ComicProgressMigrationStatusDto::Applied);
        assert_eq!(
            result
                .match_result
                .as_ref()
                .map(|matched| matched.progress_migration),
            Some(ComicProgressMigrationModeDto::OneTime)
        );
        assert_eq!(
            result.page_migration.strategy,
            ComicPageMappingStrategyDto::NearestSurvivingPage
        );
        let json = serde_json::to_string(&result).unwrap();
        assert!(!json.contains("authoritativeContentKey"));
        assert!(!json.contains("pageId"));
        assert!(!json.contains("grant"));
        assert!(!json.contains("url"));

        let reverted = service
            .revert_wire(ComicProgressMigrationRevertRequestDto {
                migration_id: result.snapshot_id.clone().unwrap(),
                expected_applied_revision: result.applied_revision.clone().unwrap(),
            })
            .await
            .unwrap();
        assert!(reverted.reverted);
    }

    #[tokio::test]
    async fn source_candidates_are_ranked_and_scoped_to_the_current_work() {
        let (
            repositories,
            source,
            target,
            work_id,
            source_edition,
            _target_edition,
            source_media,
            target_media,
        ) = seed_fixture().await;
        let (service, _) = configure_fixture(
            repositories,
            source.clone(),
            target.clone(),
            work_id,
            source_edition,
            source_media,
            target_media,
        )
        .await;

        let result = service
            .source_candidates_wire(ComicChapterSourceCandidatesGetRequestDto {
                source: ComicChapterSourceIdentityDto {
                    source_id: source.source_key,
                    remote_work_id: source.remote_work_id,
                    remote_chapter_id: source.remote_chapter_id,
                },
            })
            .await
            .unwrap();
        assert_eq!(result.current_media_item_id, source_media.to_string());
        assert!(!result.truncated);
        assert_eq!(result.candidates.len(), 1);
        assert_eq!(result.candidates[0].media_item_id, target_media.to_string());
        assert_eq!(
            result.candidates[0].match_result.kind,
            ComicChapterMatchKindDto::SameLogicalChapterVariant
        );
        let json = serde_json::to_string(&result).unwrap();
        assert!(!json.contains("authoritativeContentKey"));
        assert!(!json.contains("pageId"));
        assert!(!json.contains("grant"));
        assert!(!json.contains("url"));
    }
}
