//! Agent 只读上下文与资源偏好提案 Application service。
//!
//! 这一层把 MCP/内置 Agent 需要的有限事实投影成 typed DTO，并把资源偏好写入
//! 接回既有 `SettingProposalService`。它不提供任何直接写入、文件访问、SQL 或
//! secret 读取能力；资源偏好提案仍然必须经过 Proposal → UI approval → CAS。

use std::collections::HashMap;
use std::sync::Arc;

use haven_common::{AppError, ErrorKind};
use haven_domain::agent::{
    AgentBoundaryMode, AgentCapability, AgentContentKind, AgentContextPayload, AgentContextSegment,
    AgentContextSegmentId, AgentContextSnapshot, AgentContextSourceKind, AgentLocatorConfidence,
    AgentSubject, contains_sensitive_text, derive_settings_context_id,
    redact_preference_data_for_agent, redact_setting_value_for_agent,
};
use haven_domain::contracts::{
    EditionRepository, FavoriteRepository, MediaItemRepository, ProgressRepository,
    ResourcePreferenceRepository, ResourceRepository, StorageLocationRepository, WorkRepository,
};
use haven_domain::entities::FavoriteTarget;
use haven_domain::enums::{Availability, CompletionState, ContentCategory, MediaType};
use haven_domain::ids::{AgentRequestId, AgentSessionId, EditionId, MediaItemId};
use haven_domain::setting_proposal::{
    SettingProposal, SettingProposalChange, SettingTarget, canonical_digest, canonical_json_of,
};
use haven_domain::settings::PreferenceData;

use crate::services::agent::{AgentProposalService, AgentSettingsActionRequest};
use crate::services::agent_trace::{AgentEventKind, AgentTracePort, record_best_effort};
use crate::services::ai_provider::AiProviderProfileService;
use crate::services::settings::SettingsService;
use crate::wire::{
    AgentLibraryCategoryCountDto, AgentLibraryRecentItemDto, AgentLibrarySummaryCountsDto,
    AgentLibrarySummaryDto, AgentMediaAvailabilityDto, AgentMediaCapabilitiesDto,
    AgentMediaCapabilityDto, AgentMediaCapabilityFlagDto, AgentOnboardingStateDto,
    AgentResourcePreferencePatchDto, AgentResourcePreferenceProposalApproveRequest,
    AgentResourcePreferenceProposalApproveResultDto, AgentResourcePreferenceProposalCreateRequest,
    AgentResourcePreferenceProposalDto, AgentResourcePreferenceProposalGetRequest,
    AgentResourcePreferenceProposalGetResultDto, AgentResourcePreferenceScopeDto,
    AgentResourcePreferenceSnapshotDto, AgentSettingChangeDto, AgentSettingChangeReceiptDto,
    AgentSettingSourceDto, AgentSettingSourceLayerDto, AgentSettingSourcesDto,
    AgentSettingsProposalStatusDto, ContentCategory as ContentCategoryDto, MediaTypeDto,
};

const MAX_LIBRARY_ROWS: u32 = 10_000;
const MAX_CAPABILITY_ROWS: u32 = 5_000;

#[derive(Clone)]
pub struct AgentContextQueryService {
    works: Arc<dyn WorkRepository + Send + Sync>,
    editions: Arc<dyn EditionRepository + Send + Sync>,
    media_items: Arc<dyn MediaItemRepository + Send + Sync>,
    resources: Arc<dyn ResourceRepository + Send + Sync>,
    progress: Arc<dyn ProgressRepository + Send + Sync>,
    favorites: Arc<dyn FavoriteRepository + Send + Sync>,
    storage_locations: Arc<dyn StorageLocationRepository + Send + Sync>,
    preferences: Arc<dyn ResourcePreferenceRepository + Send + Sync>,
    settings: SettingsService,
    proposals: AgentProposalService,
    ai_provider: AiProviderProfileService,
    trace: Option<Arc<dyn AgentTracePort>>,
}

impl AgentContextQueryService {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        works: Arc<dyn WorkRepository + Send + Sync>,
        editions: Arc<dyn EditionRepository + Send + Sync>,
        media_items: Arc<dyn MediaItemRepository + Send + Sync>,
        resources: Arc<dyn ResourceRepository + Send + Sync>,
        progress: Arc<dyn ProgressRepository + Send + Sync>,
        favorites: Arc<dyn FavoriteRepository + Send + Sync>,
        storage_locations: Arc<dyn StorageLocationRepository + Send + Sync>,
        preferences: Arc<dyn ResourcePreferenceRepository + Send + Sync>,
        settings: SettingsService,
        proposals: AgentProposalService,
        ai_provider: AiProviderProfileService,
    ) -> Self {
        Self {
            works,
            editions,
            media_items,
            resources,
            progress,
            favorites,
            storage_locations,
            preferences,
            settings,
            proposals,
            ai_provider,
            trace: None,
        }
    }

    /// 注入共享轨迹 collector。轨迹故障不能影响资源提案或 CAS 结果。
    pub fn with_trace(mut self, trace: Arc<dyn AgentTracePort>) -> Self {
        self.trace = Some(trace);
        self
    }

    /// 返回全局阅读分区的来源层事实。没有目标版本/条目参数时，作用域层明确返回
    /// `present=false`，不把某一本资源的来源误报成全局结论。
    pub async fn setting_sources(&self) -> Result<AgentSettingSourcesDto, AppError> {
        let snapshot = self
            .settings
            .get(haven_domain::settings::SettingsSection::Reading)
            .await?;
        let revision = snapshot.revision;
        let global_present = revision.is_some();
        Ok(AgentSettingSourcesDto {
            schema_version: 1,
            section: crate::wire::AgentSettingsSectionDto::Reading,
            revision: revision.clone(),
            layers: vec![
                AgentSettingSourceDto {
                    layer: AgentSettingSourceLayerDto::Default,
                    present: true,
                    revision: None,
                },
                AgentSettingSourceDto {
                    layer: AgentSettingSourceLayerDto::Global,
                    present: global_present,
                    revision,
                },
                AgentSettingSourceDto {
                    layer: AgentSettingSourceLayerDto::Edition,
                    present: false,
                    revision: None,
                },
                AgentSettingSourceDto {
                    layer: AgentSettingSourceLayerDto::MediaItem,
                    present: false,
                    revision: None,
                },
            ],
        })
    }

    pub async fn resource_preference_snapshot(
        &self,
        scope: AgentResourcePreferenceScopeDto,
        edition_id: EditionId,
        media_item_id: Option<MediaItemId>,
    ) -> Result<AgentResourcePreferenceSnapshotDto, AppError> {
        let target = self.load_target(scope, edition_id, media_item_id).await?;
        let context = build_resource_context(&target)?;
        let visible = redact_preference_data_for_agent(&target.data);
        Ok(AgentResourcePreferenceSnapshotDto {
            schema_version: 1,
            context_id: context.id().to_string(),
            context_hash: context.context_hash().to_owned(),
            target_scope: scope,
            edition_id: edition_id.to_string(),
            media_item_id: target.media_item_id.map(|id| id.to_string()),
            revision: target.revision,
            reading: visible.reading.map(Into::into),
            comic: visible.comic.map(Into::into),
        })
    }

    pub async fn create_resource_preference_proposal(
        &self,
        session_id: AgentSessionId,
        request_id: AgentRequestId,
        request: &AgentResourcePreferenceProposalCreateRequest,
    ) -> Result<AgentResourcePreferenceProposalDto, AppError> {
        record_best_effort(
            self.trace.as_ref(),
            session_id,
            request_id,
            &request.context_id,
            &request.context_hash,
            AgentEventKind::RequestStarted,
        )
        .await;
        let edition_id = parse_edition_id(&request.edition_id)?;
        let media_item_id = match request.target_scope {
            AgentResourcePreferenceScopeDto::Edition => {
                if request.media_item_id.is_some() {
                    return Err(invalid_argument("edition 作用域不得携带 media_item_id"));
                }
                None
            }
            AgentResourcePreferenceScopeDto::MediaItem => Some(parse_media_item_id(
                request
                    .media_item_id
                    .as_deref()
                    .ok_or_else(|| invalid_argument("media_item 作用域必须携带 media_item_id"))?,
            )?),
        };
        let target = self
            .load_target(request.target_scope, edition_id, media_item_id)
            .await?;
        let context = build_resource_context(&target)?;
        if request.context_id != context.id().to_string() {
            return Err(context_id_mismatch());
        }
        if request.context_hash != context.context_hash() {
            return Err(context_stale());
        }
        if request.base_revision.as_deref() != target.revision.as_deref() {
            return Err(base_revision_mismatch());
        }
        record_best_effort(
            self.trace.as_ref(),
            session_id,
            request_id,
            &request.context_id,
            &request.context_hash,
            AgentEventKind::ContextLoaded,
        )
        .await;

        let patch = domain_preference_patch(&request.patch);
        let next = target.data.apply_patch(&patch);
        if next == target.data {
            return Err(invalid_argument("提案没有产生任何资源偏好改动"));
        }

        let setting_target = match request.target_scope {
            AgentResourcePreferenceScopeDto::Edition => SettingTarget::edition(edition_id),
            AgentResourcePreferenceScopeDto::MediaItem => SettingTarget::media_item(
                edition_id,
                media_item_id.expect("media_item scope validated above"),
            ),
        };
        let action = self
            .proposals
            .create_setting_proposal(
                AgentSettingsActionRequest::new(
                    session_id,
                    request_id,
                    &context,
                    setting_target,
                    // Agent 资源提案只保存 patch；应用时在同一 CAS 事务里重读并合并，
                    // 避免把 authoritative 中原本存在的敏感自由文本复制进提案。
                    SettingProposalChange::AgentResourcePreferencePatch(patch),
                )
                .with_required_capability(AgentCapability::ResourcePreferenceProposal)
                .with_base_revision(target.revision.clone()),
            )
            .await?;
        let proposal = self
            .proposals
            .setting_proposal()
            .get(action.setting_proposal_id())
            .await?
            .ok_or_else(|| not_found("SETTING_PROPOSAL_NOT_FOUND", "资源偏好提案不存在"))?;
        let projected = project_resource_proposal(&proposal, &target)?;
        record_best_effort(
            self.trace.as_ref(),
            session_id,
            request_id,
            &request.context_id,
            &request.context_hash,
            AgentEventKind::ProposalCreated,
        )
        .await;
        record_best_effort(
            self.trace.as_ref(),
            session_id,
            request_id,
            &request.context_id,
            &request.context_hash,
            AgentEventKind::WaitingForApproval,
        )
        .await;
        Ok(projected)
    }

    /// 按 proposalId 回读资源级 Agent 提案与（可能存在的）回执。
    ///
    /// 外部 Agent 只会得到 proposalId/digest；未来 UI 可用这个 typed 入口从同一条
    /// authoritative Proposal 事实打开 Diff，不需要自己拼第二套资源快照。
    pub async fn get_resource_preference_proposal(
        &self,
        request: &AgentResourcePreferenceProposalGetRequest,
    ) -> Result<AgentResourcePreferenceProposalGetResultDto, AppError> {
        let proposal_id = parse_proposal_id(&request.proposal_id)?;
        let proposal = self
            .proposals
            .setting_proposal()
            .get(proposal_id)
            .await?
            .ok_or_else(|| not_found("SETTING_PROPOSAL_NOT_FOUND", "资源偏好提案不存在"))?;
        let target = self.load_resource_target_from_proposal(&proposal).await?;
        let receipt = self
            .proposals
            .setting_proposal()
            .get_receipt(proposal_id)
            .await?;
        let proposal_target = if let Some(receipt) = receipt.as_ref() {
            ResourcePreferenceTarget {
                data: resource_preference_receipt(receipt, &proposal)?,
                ..target.clone()
            }
        } else {
            target.clone()
        };
        let projected_receipt = receipt
            .as_ref()
            .map(|receipt| project_resource_receipt(receipt, &proposal))
            .transpose()?;
        Ok(AgentResourcePreferenceProposalGetResultDto {
            // 已应用提案的 authoritative 当前值就是 after；回读 Diff 必须从同一
            // UoW 产生的 receipt before 投影，否则 UI 重开后会看到空 Diff。
            proposal: project_resource_proposal(&proposal, &proposal_target)?,
            receipt: projected_receipt,
        })
    }

    /// 用户批准资源级 Agent 提案；目标 scope 从持久化 Proposal 重建，并继续复用
    /// Agent Binding + Approval Token + CAS + Receipt 的单一 UoW 内核。
    pub async fn approve_resource_preference_proposal(
        &self,
        request: &AgentResourcePreferenceProposalApproveRequest,
    ) -> Result<AgentResourcePreferenceProposalApproveResultDto, AppError> {
        let proposal_id = parse_proposal_id(&request.proposal_id)?;
        let proposal = self
            .proposals
            .setting_proposal()
            .get(proposal_id)
            .await?
            .ok_or_else(|| not_found("SETTING_PROPOSAL_NOT_FOUND", "资源偏好提案不存在"))?;
        let target = self.load_resource_target_from_proposal(&proposal).await?;
        let expected_target = proposal.target();
        let binding = self
            .proposals
            .load_binding(proposal_id)
            .await
            .ok()
            .flatten();
        if let Some(binding) = binding.as_ref() {
            let context_id = binding.context_snapshot_id().to_string();
            record_best_effort(
                self.trace.as_ref(),
                binding.session_id(),
                binding.request_id(),
                &context_id,
                binding.context_hash(),
                AgentEventKind::CasStarted,
            )
            .await;
        }
        let receipt = self
            .proposals
            .approve_agent_resource_proposal_in_one_uow(
                proposal_id,
                &request.expected_digest,
                expected_target,
            )
            .await?;
        let applied_proposal = self
            .proposals
            .setting_proposal()
            .get(proposal_id)
            .await?
            .ok_or_else(|| not_found("SETTING_PROPOSAL_NOT_FOUND", "资源偏好提案不存在"))?;
        if let Some(binding) = binding {
            let context_id = binding.context_snapshot_id().to_string();
            record_best_effort(
                self.trace.as_ref(),
                binding.session_id(),
                binding.request_id(),
                &context_id,
                binding.context_hash(),
                AgentEventKind::Applied,
            )
            .await;
            record_best_effort(
                self.trace.as_ref(),
                binding.session_id(),
                binding.request_id(),
                &context_id,
                binding.context_hash(),
                AgentEventKind::ReceiptCreated,
            )
            .await;
        }
        Ok(AgentResourcePreferenceProposalApproveResultDto {
            // 用批准前的 target 投影 changes，避免批准后重读 authoritative 值把 Diff
            // 错误地投影成空列表；receipt 自己则来自同一 UoW 的审计事实。
            proposal: project_resource_proposal(&applied_proposal, &target)?,
            receipt: project_resource_receipt(&receipt, &applied_proposal)?,
        })
    }

    pub async fn library_summary(&self, limit: u32) -> Result<AgentLibrarySummaryDto, AppError> {
        let limit = limit.clamp(1, 50);
        let work_total = self.works.count_filtered(None, None, None).await?;
        let mut works = self.works.list(MAX_LIBRARY_ROWS, 0).await?;
        let library_truncated = (works.len() as u64) < work_total;
        if library_truncated {
            works.truncate(MAX_LIBRARY_ROWS as usize);
        }
        let work_ids: Vec<_> = works.iter().map(|work| work.id).collect();
        let editions = self.editions.list_by_works(&work_ids).await?;
        let edition_ids: Vec<_> = editions.iter().map(|edition| edition.id).collect();
        let media_items = self.media_items.list_by_editions(&edition_ids).await?;
        let media_ids: Vec<_> = media_items.iter().map(|item| item.id).collect();
        let progress = self.progress.get_for_media_items(&media_ids).await?;
        let favorites = self
            .favorites
            .is_favorite_many(
                &work_ids
                    .iter()
                    .copied()
                    .map(FavoriteTarget::Work)
                    .collect::<Vec<_>>(),
            )
            .await?;

        let mut categories: HashMap<ContentCategory, u64> = HashMap::new();
        let mut editions_by_work: HashMap<_, Vec<_>> = HashMap::new();
        for edition in &editions {
            editions_by_work
                .entry(edition.work_id)
                .or_default()
                .push(edition);
        }
        let mut media_by_edition: HashMap<_, Vec<_>> = HashMap::new();
        for item in &media_items {
            media_by_edition
                .entry(item.edition_id)
                .or_default()
                .push(item);
        }
        for work in &works {
            let category = editions_by_work
                .get(&work.id)
                .and_then(|editions| {
                    editions
                        .iter()
                        .map(|edition| ContentCategory::from_media_type(edition.edition_type))
                        .next()
                })
                .unwrap_or(ContentCategory::All);
            if category != ContentCategory::All {
                *categories.entry(category).or_default() += 1;
            }
        }

        let mut recent = Vec::new();
        for work in works.iter().take(limit as usize) {
            let media = editions_by_work
                .get(&work.id)
                .into_iter()
                .flatten()
                .flat_map(|edition| media_by_edition.get(&edition.id).into_iter().flatten())
                .collect::<Vec<_>>();
            let category = media
                .first()
                .map(|item| ContentCategory::from_media_type(item.media_type))
                .or_else(|| {
                    editions_by_work.get(&work.id).and_then(|items| {
                        items
                            .first()
                            .map(|item| ContentCategory::from_media_type(item.edition_type))
                    })
                })
                .unwrap_or(ContentCategory::All);
            let progress_ratio = media
                .iter()
                .filter_map(|item| progress.get(&item.id))
                .max_by_key(|item| (item.last_active_at, item.id))
                .and_then(|item| item.percentage.map(f64::from));
            recent.push(AgentLibraryRecentItemDto {
                work_id: work.id.to_string(),
                title: bounded_label(&work.canonical_title, 256),
                category: content_category_dto(category),
                progress_ratio,
            });
        }

        let category_order = [
            ContentCategory::Video,
            ContentCategory::Book,
            ContentCategory::Comic,
            ContentCategory::Periodical,
        ];
        let categories = category_order
            .into_iter()
            .filter_map(|category| {
                let work_count = *categories.get(&category)?;
                Some(AgentLibraryCategoryCountDto {
                    category: content_category_dto(category)?,
                    work_count,
                })
            })
            .collect();
        Ok(AgentLibrarySummaryDto {
            schema_version: 1,
            counts: AgentLibrarySummaryCountsDto {
                works: work_total,
                editions: editions.len() as u64,
                media_items: media_items.len() as u64,
                favorites: favorites.len() as u64,
                in_progress: progress
                    .values()
                    .filter(|item| item.completion == CompletionState::InProgress)
                    .count() as u64,
            },
            categories,
            recent,
            truncated: library_truncated || works.len() > limit as usize,
        })
    }

    pub async fn media_capabilities(
        &self,
        media_item_id: Option<MediaItemId>,
        limit: u32,
    ) -> Result<AgentMediaCapabilitiesDto, AppError> {
        let limit = limit.clamp(1, 50) as usize;
        let (items, truncated) = if let Some(id) = media_item_id {
            let item = self
                .media_items
                .get(id)
                .await?
                .ok_or_else(|| not_found("MEDIA_ITEM_NOT_FOUND", "媒体条目不存在"))?;
            (vec![item], false)
        } else {
            let work_total = self.works.count_filtered(None, None, None).await?;
            let works = self.works.list(MAX_CAPABILITY_ROWS, 0).await?;
            let work_ids: Vec<_> = works.iter().map(|work| work.id).collect();
            let editions = self.editions.list_by_works(&work_ids).await?;
            let edition_ids: Vec<_> = editions.iter().map(|edition| edition.id).collect();
            let mut items = self.media_items.list_by_editions(&edition_ids).await?;
            let truncated = (works.len() as u64) < work_total || items.len() > limit;
            items.truncate(limit);
            (items, truncated)
        };

        let mut projected = Vec::with_capacity(items.len());
        for item in items {
            let resources = self.resources.list_by_media_item(item.id).await?;
            let availability = project_availability(&resources);
            let available = availability == AgentMediaAvailabilityDto::Available;
            let can_open_session = available;
            let can_extract_text = available
                && matches!(
                    item.media_type,
                    MediaType::Book | MediaType::Document | MediaType::Article
                );
            let can_render_pages = available
                && matches!(
                    item.media_type,
                    MediaType::Comic | MediaType::Document | MediaType::Article
                );
            let mut declared_capabilities = Vec::new();
            if can_open_session {
                declared_capabilities.push(AgentMediaCapabilityFlagDto::OpenSession);
            }
            if can_extract_text {
                declared_capabilities.push(AgentMediaCapabilityFlagDto::ExtractText);
            }
            if can_render_pages {
                declared_capabilities.push(AgentMediaCapabilityFlagDto::RenderPages);
            }
            if available
                && matches!(
                    item.media_type,
                    MediaType::Movie | MediaType::Series | MediaType::Episode | MediaType::Audio
                )
            {
                declared_capabilities.push(AgentMediaCapabilityFlagDto::Stream);
            }
            projected.push(AgentMediaCapabilityDto {
                media_item_id: item.id.to_string(),
                media_type: media_type_dto(item.media_type),
                availability,
                can_open_session,
                can_extract_text,
                can_render_pages,
                declared_capabilities,
            });
        }
        Ok(AgentMediaCapabilitiesDto {
            schema_version: 1,
            items: projected,
            truncated,
        })
    }

    pub async fn onboarding_state(&self) -> Result<AgentOnboardingStateDto, AppError> {
        let has_storage_location = !self.storage_locations.list().await?.is_empty();
        let has_library_content = self.works.count_filtered(None, None, None).await? > 0;
        let provider_profiles = self.ai_provider.list().await?.profiles;
        let has_ai_provider = provider_profiles.iter().any(|profile| {
            profile.enabled && profile.credential_configured && profile.selected_model_id.is_some()
        });
        let mut completed_steps = Vec::new();
        if has_storage_location {
            completed_steps.push("storage_location".to_owned());
        }
        if has_library_content {
            completed_steps.push("library_content".to_owned());
        }
        if has_ai_provider {
            completed_steps.push("ai_provider".to_owned());
        }
        let next_step = if !has_storage_location {
            Some("storage_location".to_owned())
        } else if !has_library_content {
            Some("library_content".to_owned())
        } else if !has_ai_provider {
            Some("ai_provider".to_owned())
        } else {
            None
        };
        Ok(AgentOnboardingStateDto {
            schema_version: 1,
            completed_steps,
            next_step,
            has_storage_location,
            has_library_content,
            has_ai_provider,
        })
    }

    async fn load_target(
        &self,
        scope: AgentResourcePreferenceScopeDto,
        edition_id: EditionId,
        media_item_id: Option<MediaItemId>,
    ) -> Result<ResourcePreferenceTarget, AppError> {
        let edition = self
            .editions
            .get(edition_id)
            .await?
            .ok_or_else(|| not_found("EDITION_NOT_FOUND", "版本不存在"))?;
        match scope {
            AgentResourcePreferenceScopeDto::Edition => {
                let row = self.preferences.get_edition(edition_id).await?;
                Ok(ResourcePreferenceTarget {
                    scope,
                    work_id: edition.work_id,
                    edition_id,
                    media_item_id: None,
                    content_kind: content_kind(edition.edition_type)?,
                    label: bounded_label(&edition.title, 256),
                    data: row.as_ref().map(|row| row.data.clone()).unwrap_or_default(),
                    revision: row.map(|row| row.revision),
                })
            }
            AgentResourcePreferenceScopeDto::MediaItem => {
                let media_item_id = media_item_id
                    .ok_or_else(|| invalid_argument("media_item 作用域必须携带 media_item_id"))?;
                let item = self
                    .media_items
                    .get(media_item_id)
                    .await?
                    .ok_or_else(|| not_found("MEDIA_ITEM_NOT_FOUND", "媒体条目不存在"))?;
                if item.edition_id != edition_id {
                    return Err(invalid_argument("媒体条目不属于指定版本"));
                }
                let row = self.preferences.get_media_item(media_item_id).await?;
                Ok(ResourcePreferenceTarget {
                    scope,
                    work_id: edition.work_id,
                    edition_id,
                    media_item_id: Some(media_item_id),
                    content_kind: content_kind(item.media_type)?,
                    label: bounded_label(&item.title, 256),
                    data: row.as_ref().map(|row| row.data.clone()).unwrap_or_default(),
                    revision: row.map(|row| row.revision),
                })
            }
        }
    }

    async fn load_resource_target_from_proposal(
        &self,
        proposal: &SettingProposal,
    ) -> Result<ResourcePreferenceTarget, AppError> {
        match proposal.target() {
            SettingTarget::Edition(target) => {
                if !matches!(
                    proposal.change(),
                    SettingProposalChange::AgentResourcePreferencePatch(_)
                ) {
                    return Err(invalid_argument("提案不是资源级 Agent patch"));
                }
                self.load_target(
                    AgentResourcePreferenceScopeDto::Edition,
                    target.edition_id,
                    None,
                )
                .await
            }
            SettingTarget::MediaItem(target) => {
                if !matches!(
                    proposal.change(),
                    SettingProposalChange::AgentResourcePreferencePatch(_)
                ) {
                    return Err(invalid_argument("提案不是资源级 Agent patch"));
                }
                self.load_target(
                    AgentResourcePreferenceScopeDto::MediaItem,
                    target.edition_id,
                    Some(target.media_item_id),
                )
                .await
            }
            SettingTarget::Global(_) => Err(invalid_argument("提案不是资源级 Agent patch")),
        }
    }
}

#[derive(Debug, Clone)]
struct ResourcePreferenceTarget {
    scope: AgentResourcePreferenceScopeDto,
    work_id: haven_domain::ids::WorkId,
    edition_id: EditionId,
    media_item_id: Option<MediaItemId>,
    content_kind: AgentContentKind,
    label: String,
    data: PreferenceData,
    revision: Option<String>,
}

fn build_resource_context(
    target: &ResourcePreferenceTarget,
) -> Result<AgentContextSnapshot, AppError> {
    let subject = match target.media_item_id {
        Some(media_item_id) => AgentSubject::media_item(
            target.work_id,
            target.edition_id,
            media_item_id,
            target.content_kind,
        ),
        None => AgentSubject::edition(target.work_id, target.edition_id, target.content_kind),
    };
    let preference_json = canonical_json_of(&target.data)?;
    let preference_hash = canonical_digest(&preference_json);
    let segment_id = AgentContextSegmentId::new("resource-preference")?;
    let mut payload = AgentContextPayload::new(
        subject,
        None,
        AgentLocatorConfidence::Unresolved,
        AgentBoundaryMode::UserProvided,
        target.revision.as_deref().unwrap_or("unpersisted"),
    );
    payload.segments.push(AgentContextSegment {
        segment_id,
        source_kind: AgentContextSourceKind::Metadata,
        locator_range: None,
        // Only the digest of the current preference data enters the Agent context. This binds
        // the proposal to the complete value without exporting a custom font/path-like string
        // through a free-form context segment.
        text: format!(
            "scope={};edition_id={};media_item_id={};revision={};preference_hash={}",
            scope_name(target.scope),
            target.edition_id,
            target
                .media_item_id
                .map(|id| id.to_string())
                .unwrap_or_else(|| "none".to_owned()),
            target.revision.as_deref().unwrap_or("none"),
            preference_hash,
        ),
    });
    // 上下文身份必须能从上下文事实**重算**，而不是每次读取随机生成：读取快照与随后
    // 创建提案会各自重建一次上下文，`context_id` 正是用来证明"你读到的就是这一版"。
    // 随机 id 会让这条比对永远失败（`AGENT_CONTEXT_ID_MISMATCH`），把资源偏好提案
    // 变成不可用入口。这里复用领域已有的「上下文哈希 → 确定性 UUID」派生规则
    // （与全局设置上下文同一条规则）：偏好或 revision 没变 → 同一个 id，变了 → 另一个 id，
    // 因此不需要为上下文维护"这个 id 我发过"的登记表。
    let canonical_json = canonical_json_of(&payload)?;
    let context_id = derive_settings_context_id(&canonical_digest(&canonical_json));
    AgentContextSnapshot::new(context_id, payload)
}

fn domain_preference_patch(patch: &AgentResourcePreferencePatchDto) -> PreferenceData {
    PreferenceData {
        reading: patch.reading.clone().map(Into::into),
        comic: patch.comic.clone().map(Into::into),
    }
}

fn project_resource_proposal(
    proposal: &SettingProposal,
    target: &ResourcePreferenceTarget,
) -> Result<AgentResourcePreferenceProposalDto, AppError> {
    let SettingProposalChange::AgentResourcePreferencePatch(patch) = proposal.change() else {
        return Err(AppError::new(
            "AGENT_RESOURCE_PROPOSAL_INVALID",
            ErrorKind::Internal,
            "资源提案载荷类型不一致",
            false,
        ));
    };
    let next = target.data.apply_patch(patch);
    Ok(AgentResourcePreferenceProposalDto {
        schema_version: 1,
        proposal_id: proposal.id().to_string(),
        status: AgentSettingsProposalStatusDto::from(proposal.status()),
        target_scope: target.scope,
        // 展示标签取自 authoritative 目标本身（版本/条目标题，已按字符数截断），
        // 审批界面要能看清"这条提案改的是哪一份资源"，而不是只看得到一个作用域名。
        target_label: target.label.clone(),
        edition_id: target.edition_id.to_string(),
        media_item_id: target.media_item_id.map(|id| id.to_string()),
        base_revision: proposal.base_revision().map(str::to_owned),
        digest: proposal.digest().to_owned(),
        created_at: format_millis(proposal.created_at()),
        expires_at: format_millis(proposal.expires_at()),
        changes: describe_preference_changes(&target.data, &next)?,
    })
}

fn describe_preference_changes(
    before: &PreferenceData,
    after: &PreferenceData,
) -> Result<Vec<AgentSettingChangeDto>, AppError> {
    let mut changes = Vec::new();
    let before_reading = before.reading.clone().unwrap_or_default();
    let after_reading = after.reading.clone().unwrap_or_default();
    push_option_change(
        &mut changes,
        "reading.fontFamily",
        before_reading.font_family,
        after_reading.font_family,
    )?;
    push_text_change(
        &mut changes,
        "reading.customFontFamily",
        before_reading.custom_font_family.as_deref(),
        after_reading.custom_font_family.as_deref(),
    );
    push_option_change(
        &mut changes,
        "reading.fontSize",
        before_reading.font_size,
        after_reading.font_size,
    )?;
    push_option_change(
        &mut changes,
        "reading.lineHeight",
        before_reading.line_height,
        after_reading.line_height,
    )?;
    push_option_change(
        &mut changes,
        "reading.contentWidth",
        before_reading.content_width,
        after_reading.content_width,
    )?;
    push_option_change(
        &mut changes,
        "reading.theme",
        before_reading.theme,
        after_reading.theme,
    )?;
    push_text_change(
        &mut changes,
        "reading.customBackground",
        before_reading.custom_background.as_deref(),
        after_reading.custom_background.as_deref(),
    );
    push_text_change(
        &mut changes,
        "reading.customText",
        before_reading.custom_text.as_deref(),
        after_reading.custom_text.as_deref(),
    );
    push_option_change(
        &mut changes,
        "reading.fontWeight",
        before_reading.font_weight,
        after_reading.font_weight,
    )?;
    push_option_change(
        &mut changes,
        "reading.letterSpacing",
        before_reading.letter_spacing,
        after_reading.letter_spacing,
    )?;
    push_option_change(
        &mut changes,
        "reading.systemAuto",
        before_reading.system_auto,
        after_reading.system_auto,
    )?;
    push_option_change(
        &mut changes,
        "reading.pagination",
        before_reading.pagination,
        after_reading.pagination,
    )?;

    let before_comic = before.comic.clone().unwrap_or_default();
    let after_comic = after.comic.clone().unwrap_or_default();
    push_option_change(
        &mut changes,
        "comic.viewMode",
        before_comic.view_mode,
        after_comic.view_mode,
    )?;
    push_option_change(
        &mut changes,
        "comic.direction",
        before_comic.direction,
        after_comic.direction,
    )?;
    push_option_change(
        &mut changes,
        "comic.pageGap",
        before_comic.page_gap,
        after_comic.page_gap,
    )?;
    push_option_change(
        &mut changes,
        "comic.preloadPages",
        before_comic.preload_pages,
        after_comic.preload_pages,
    )?;
    Ok(changes)
}

fn project_resource_receipt(
    receipt: &haven_domain::setting_proposal::SettingChangeReceipt,
    proposal: &SettingProposal,
) -> Result<AgentSettingChangeReceiptDto, AppError> {
    let before = resource_preference_receipt(receipt, proposal)?;
    let after = resource_preference_receipt_after(receipt, proposal)?;
    Ok(AgentSettingChangeReceiptDto {
        schema_version: 1,
        receipt_id: receipt.id.to_string(),
        proposal_id: receipt.proposal_id.to_string(),
        proposal_digest: receipt.proposal_digest.clone(),
        status: proposal.status().into(),
        applied_revision: receipt.applied_revision.clone(),
        changed: receipt.changed,
        changes: describe_preference_changes(&before, &after)?,
        applied_at: format_millis(receipt.applied_at),
    })
}

fn resource_preference_receipt(
    receipt: &haven_domain::setting_proposal::SettingChangeReceipt,
    _proposal: &SettingProposal,
) -> Result<PreferenceData, AppError> {
    serde_json::from_str(&receipt.before_canonical_json)
        .map_err(|_| invalid_argument("资源偏好回执改前值无法解析"))
}

fn resource_preference_receipt_after(
    receipt: &haven_domain::setting_proposal::SettingChangeReceipt,
    _proposal: &SettingProposal,
) -> Result<PreferenceData, AppError> {
    serde_json::from_str(&receipt.after_canonical_json)
        .map_err(|_| invalid_argument("资源偏好回执改后值无法解析"))
}

fn push_text_change(
    changes: &mut Vec<AgentSettingChangeDto>,
    key: &str,
    before: Option<&str>,
    after: Option<&str>,
) {
    let before = before
        .map(redact_setting_value_for_agent)
        .unwrap_or_default();
    let after = after
        .map(redact_setting_value_for_agent)
        .unwrap_or_default();
    if before != after {
        changes.push(AgentSettingChangeDto {
            key: key.to_owned(),
            before: before.to_owned(),
            after: after.to_owned(),
        });
    }
}

fn push_option_change<T: serde::Serialize + PartialEq>(
    changes: &mut Vec<AgentSettingChangeDto>,
    key: &str,
    before: Option<T>,
    after: Option<T>,
) -> Result<(), AppError> {
    if before == after {
        return Ok(());
    }
    changes.push(AgentSettingChangeDto {
        key: key.to_owned(),
        before: option_token(before.as_ref())?,
        after: option_token(after.as_ref())?,
    });
    Ok(())
}

fn option_token<T: serde::Serialize + ?Sized>(value: Option<&T>) -> Result<String, AppError> {
    match value {
        Some(value) => match serde_json::to_value(value)
            .map_err(|_| invalid_argument("资源偏好枚举无法序列化"))?
        {
            serde_json::Value::String(value) => Ok(value),
            _ => Err(invalid_argument("资源偏好枚举不是字符串表示")),
        },
        None => Ok(String::new()),
    }
}

fn project_availability(
    resources: &[haven_domain::entities::Resource],
) -> AgentMediaAvailabilityDto {
    if resources.is_empty() {
        return AgentMediaAvailabilityDto::Unknown;
    }
    if resources.iter().any(|resource| {
        matches!(
            resource.availability,
            Availability::Available | Availability::OfflineAvailable
        )
    }) {
        AgentMediaAvailabilityDto::Available
    } else {
        AgentMediaAvailabilityDto::Unavailable
    }
}

fn content_kind(media_type: MediaType) -> Result<AgentContentKind, AppError> {
    Ok(match media_type {
        MediaType::Book => AgentContentKind::Book,
        MediaType::Comic => AgentContentKind::Comic,
        MediaType::Movie | MediaType::Series | MediaType::Episode | MediaType::Audio => {
            AgentContentKind::Video
        }
        MediaType::Document | MediaType::Article => AgentContentKind::Periodical,
        MediaType::Unknown => return Err(invalid_argument("未知媒体类型不能建立 Agent 上下文")),
    })
}

fn media_type_dto(value: MediaType) -> MediaTypeDto {
    match value {
        MediaType::Movie => MediaTypeDto::Movie,
        MediaType::Series => MediaTypeDto::Series,
        MediaType::Episode => MediaTypeDto::Episode,
        MediaType::Book => MediaTypeDto::Book,
        MediaType::Document => MediaTypeDto::Document,
        MediaType::Comic => MediaTypeDto::Comic,
        MediaType::Article => MediaTypeDto::Article,
        MediaType::Audio => MediaTypeDto::Audio,
        MediaType::Unknown => MediaTypeDto::Unknown,
    }
}

/// domain 的 `ContentCategory::All` 既是查询哨兵、也是"按媒体类型无法归类"的落点；
/// wire 的 canonical 分类里没有对应值（契约明确 `all` 只作为查询 sentinel），
/// 因此这里投影成 `None`，而不是借用某个具体分类去猜。
fn content_category_dto(category: ContentCategory) -> Option<ContentCategoryDto> {
    match category {
        ContentCategory::All => None,
        ContentCategory::Video => Some(ContentCategoryDto::Video),
        ContentCategory::Book => Some(ContentCategoryDto::Book),
        ContentCategory::Comic => Some(ContentCategoryDto::Comic),
        ContentCategory::Periodical => Some(ContentCategoryDto::Periodical),
    }
}

fn scope_name(scope: AgentResourcePreferenceScopeDto) -> &'static str {
    match scope {
        AgentResourcePreferenceScopeDto::Edition => "edition",
        AgentResourcePreferenceScopeDto::MediaItem => "media_item",
    }
}

fn bounded_label(value: &str, max_chars: usize) -> String {
    let bounded: String = value.chars().take(max_chars).collect();
    if contains_sensitive_text(&bounded) {
        "[redacted]".to_owned()
    } else {
        bounded
    }
}

fn parse_edition_id(value: &str) -> Result<EditionId, AppError> {
    value
        .parse()
        .map_err(|_| invalid_argument("edition_id 非法"))
}

fn parse_media_item_id(value: &str) -> Result<MediaItemId, AppError> {
    value
        .parse()
        .map_err(|_| invalid_argument("media_item_id 非法"))
}

fn parse_proposal_id(value: &str) -> Result<haven_domain::ids::SettingProposalId, AppError> {
    value
        .parse()
        .map_err(|_| invalid_argument("proposal_id 非法"))
}

fn format_millis(value: haven_common::UtcMillis) -> String {
    chrono::DateTime::<chrono::Utc>::from_timestamp_millis(value.0)
        .map(|at| at.to_rfc3339())
        .unwrap_or_else(|| value.0.to_string())
}

fn invalid_argument(message: impl Into<String>) -> AppError {
    AppError::new("INVALID_ARGUMENT", ErrorKind::Validation, message, false)
}

fn not_found(code: &'static str, message: &'static str) -> AppError {
    AppError::new(code, ErrorKind::NotFound, message, false)
}

fn context_id_mismatch() -> AppError {
    AppError::new(
        "AGENT_CONTEXT_ID_MISMATCH",
        ErrorKind::Conflict,
        "资源偏好上下文已变化，请重新读取",
        true,
    )
}

fn context_stale() -> AppError {
    AppError::new(
        "AGENT_CONTEXT_STALE",
        ErrorKind::Conflict,
        "资源偏好上下文已变化，请重新读取",
        true,
    )
}

fn base_revision_mismatch() -> AppError {
    AppError::new(
        "REVISION_CONFLICT",
        ErrorKind::Conflict,
        "资源偏好版本已变化，请重新读取",
        true,
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use haven_domain::ids::WorkId;
    use haven_domain::settings::{ReadingFontSize, ReadingPatch};

    /// 资源偏好上下文的身份只由 authoritative 事实决定，因此 `context_id` 可以在
    /// 读取路径与提案路径各自重建后逐字节比对——这正是 create 入口那两处比对的
    /// 前提，也是这一组测试要钉住的不变量。
    fn reading(font_size: ReadingFontSize) -> PreferenceData {
        PreferenceData {
            reading: Some(ReadingPatch {
                font_size: Some(font_size),
                ..ReadingPatch::default()
            }),
            comic: None,
        }
    }

    fn target(revision: Option<&str>, data: PreferenceData) -> ResourcePreferenceTarget {
        ResourcePreferenceTarget {
            scope: AgentResourcePreferenceScopeDto::Edition,
            work_id: WorkId::new(),
            edition_id: EditionId::new(),
            media_item_id: None,
            content_kind: AgentContentKind::Book,
            label: "样书".to_owned(),
            data,
            revision: revision.map(str::to_owned),
        }
    }

    /// 同一个 authoritative 目标的两个变体：只允许 revision / 偏好数据不同，
    /// 作品、版本与作用域必须逐字相同，否则"上下文身份稳定"这件事根本没有被测到
    /// ——不同的目标本来就该得到不同的上下文 id。
    fn variant(
        base: &ResourcePreferenceTarget,
        revision: Option<&str>,
        data: PreferenceData,
    ) -> ResourcePreferenceTarget {
        ResourcePreferenceTarget {
            data,
            revision: revision.map(str::to_owned),
            ..base.clone()
        }
    }

    #[test]
    fn resource_context_id_is_stable_across_reads_of_the_same_snapshot() {
        let base = target(Some("rev-0001"), reading(ReadingFontSize::Large));
        let first = build_resource_context(&base).unwrap();
        let second = build_resource_context(&variant(
            &base,
            Some("rev-0001"),
            reading(ReadingFontSize::Large),
        ))
        .unwrap();
        assert_eq!(first.id(), second.id());
        assert_eq!(first.context_hash(), second.context_hash());

        // 尚未持久化的作用域同样稳定（`revision=None` 不是"随机 id"的借口）。
        let unpersisted = variant(&base, None, reading(ReadingFontSize::Large));
        let unpersisted_id = build_resource_context(&unpersisted).unwrap().id();
        assert_eq!(
            unpersisted_id,
            build_resource_context(&unpersisted).unwrap().id()
        );

        // 同一个版本与内容下，持久化与否必须是两个不同的上下文身份。
        assert_ne!(first.id(), unpersisted_id);

        // 换一个目标（不同版本 id）必须得到另一个上下文身份。
        let other_target = ResourcePreferenceTarget {
            edition_id: EditionId::new(),
            ..base.clone()
        };
        assert_ne!(
            first.id(),
            build_resource_context(&other_target).unwrap().id()
        );
    }

    #[test]
    fn resource_context_id_follows_the_authoritative_revision_and_preference_data() {
        let base = target(Some("rev-0001"), reading(ReadingFontSize::Large));
        let reference = build_resource_context(&base).unwrap();

        // 同一个目标、只改 revision：上下文身份必须随之改变，否则"读到的是这一版"
        // 就退化成"读到的是某个版本"。
        let other_revision = build_resource_context(&variant(
            &base,
            Some("rev-0002"),
            reading(ReadingFontSize::Large),
        ))
        .unwrap();
        assert_ne!(reference.id(), other_revision.id());

        // 同一个 revision、只改偏好数据：同样必须换一个上下文身份。
        let other_preference = build_resource_context(&variant(
            &base,
            Some("rev-0001"),
            reading(ReadingFontSize::Small),
        ))
        .unwrap();
        assert_ne!(reference.id(), other_preference.id());

        // 上下文身份必须落在 agent 上下文的合法范围内（非 nil），否则提案绑定会在
        // 领域边界被拒绝，而这与"id 稳定"是两件必须同时成立的事。
        assert!(!reference.id().as_uuid().is_nil());
        assert!(haven_domain::setting_proposal::is_canonical_digest(
            reference.context_hash()
        ));
    }

    #[test]
    fn content_category_projection_never_invents_a_category() {
        // domain 的 `All` 是"按媒体类型无法归类"，wire 的 canonical 分类里没有它：
        // 投影必须给出 `None`，而不是借用别的分类去猜。
        assert_eq!(content_category_dto(ContentCategory::All), None);
        assert_eq!(
            content_category_dto(ContentCategory::Video),
            Some(ContentCategoryDto::Video)
        );
        assert_eq!(
            content_category_dto(ContentCategory::Periodical),
            Some(ContentCategoryDto::Periodical)
        );
    }
}
