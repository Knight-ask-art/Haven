//! AI Provider Profile 的 Typed Application 入口（A2 基础切片）。
//!
//! 规范：[`docs/architecture/AI_SYSTEM.md`](../../../../docs/architecture/AI_SYSTEM.md) §3、§4、§7、§8。
//!
//! 这是 Tauri 命令层与 Provider 配置之间的**唯一**契约面：命令层只做 typed 请求解析、
//! 调用本服务、把 `AppError` 映射成 `ErrorDto`。本模块不出现 SQL、Tauri、文件系统。
//!
//! 三条边界：
//! 1. **profile 是纯配置**。`AiProviderProfile` 不含 secret，因此 list/get 可以放心
//!    投影；DTO 里唯一的凭据信息是 `credentialConfigured` 布尔事实。
//! 2. **凭据只走 CredentialStore**。统一 `CredentialRef::new_scoped("ai", profile_id)`
//!    → `haven:ai:<profile-id>`。secret 在本模块内以 `SecretString` 存在，只传给
//!    模型发现端口，并随调用栈释放；不进入任何 DTO、日志或持久化。
//! 3. **删除有确定的凭据清理语义**。先删凭据、再 CAS 删行（见 [`AiProviderProfileService::delete`]）。

use std::sync::Arc;

use async_trait::async_trait;

use haven_common::{AppError, ErrorKind};
use haven_domain::agent::AgentReadingSnapshot;
use haven_domain::ai_provider::{
    AiModelCapability, AiModelDescriptor, AiProviderKind, AiProviderProfile,
    AiProviderProfileDeleteOutcome, AiSettingsRecommendation,
};
use haven_domain::contracts::AiProviderProfileRepository;
use haven_domain::credential::{CredentialStore, SecretString};
use haven_domain::ids::{AgentRequestId, AgentSessionId, CredentialRef};

use crate::mapper::time::utc_millis_to_rfc3339;
use crate::services::agent_settings_ipc::AgentSettingsIpcService;
use crate::services::agent_trace::{AgentEventKind, AgentTracePort, record_best_effort};
use crate::wire::{
    AgentSettingsProposalCreateRequest, AgentSettingsProposalDto, AiProviderKindDto,
    AiProviderModelDto, AiProviderModelsCatalogDto, AiProviderModelsCatalogStateDto,
    AiProviderModelsListRequest, AiProviderProfileDeleteRequest, AiProviderProfileDeleteResultDto,
    AiProviderProfileDto, AiProviderProfileListResultDto, AiProviderProfileUpsertRequest,
    PreferenceReadingPatchDto,
};

/// CredentialStore scoped target 的 provider 段（ADR-001 校验规则内）。
pub const AI_CREDENTIAL_PROVIDER: &str = "ai";

/// 模型发现端口。
///
/// 实现方（Infrastructure）负责 URL 拼接、DNS 解析与固定、重定向关闭、响应大小上界
/// 与结构校验；Application 只给出 profile 的 kind/endpoint 与内存中的 secret。
/// 端口**不**提供任何写入能力，Provider 无法借此改变本机状态。
#[async_trait]
pub trait AiModelCatalogPort: Send + Sync {
    /// 列出兼容 `/models` 的模型目录。
    ///
    /// 失败必须返回稳定可行动的 `AppError`（非 2xx / 非法 JSON / 不可达），
    /// 不得吞错，也不得伪造模型。
    async fn list_models(
        &self,
        kind: AiProviderKind,
        endpoint: &str,
        secret: &SecretString,
    ) -> Result<Vec<AiModelDescriptor>, AppError>;
}

/// 设置建议端口。实现方只负责一次受限的 Structured Output 请求，不能创建 Proposal、
/// 不能批准或应用设置。
#[async_trait]
pub trait AiSettingsRecommendationPort: Send + Sync {
    async fn recommend_settings(
        &self,
        kind: AiProviderKind,
        endpoint: &str,
        secret: &SecretString,
        input: &AiSettingsRecommendationInput,
    ) -> Result<AiSettingsRecommendation, AppError>;
}

#[derive(Debug, Clone)]
pub struct AiSettingsRecommendationInput {
    pub model_id: String,
    pub reading: AgentReadingSnapshot,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AiSettingsRecommendationRequest {
    pub profile_id: String,
    pub session_id: String,
    pub request_id: String,
    pub context_id: String,
    pub context_hash: String,
    pub base_revision: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AiSettingsRecommendationResult {
    pub schema_version: u32,
    pub profile_id: String,
    pub model_id: String,
    pub explanation: Option<String>,
    pub recommended_patch: PreferenceReadingPatchDto,
    pub proposal: AgentSettingsProposalDto,
}

/// 模型发现端口不可用时的占位实现（例如尚未接线的组装层或测试替身）。
///
/// 它**明确失败**而不是返回空目录：空目录是"Provider 说没有模型"的诚实事实，
/// 用一个"没有实现"替换它会伪造一个不存在的状态。
pub struct UnavailableModelCatalog;

#[async_trait]
impl AiModelCatalogPort for UnavailableModelCatalog {
    async fn list_models(
        &self,
        _kind: AiProviderKind,
        _endpoint: &str,
        _secret: &SecretString,
    ) -> Result<Vec<AiModelDescriptor>, AppError> {
        Err(AppError::new(
            "AI_PROVIDER_MODEL_DISCOVERY_UNAVAILABLE",
            ErrorKind::Unsupported,
            "当前组装层未接入模型发现通道",
            false,
        ))
    }
}

pub struct UnavailableSettingsRecommendation;

#[async_trait]
impl AiSettingsRecommendationPort for UnavailableSettingsRecommendation {
    async fn recommend_settings(
        &self,
        _kind: AiProviderKind,
        _endpoint: &str,
        _secret: &SecretString,
        _input: &AiSettingsRecommendationInput,
    ) -> Result<AiSettingsRecommendation, AppError> {
        Err(recommendation_unavailable())
    }
}

/// AI Provider Profile 服务。
#[derive(Clone)]
pub struct AiProviderProfileService {
    profiles: Arc<dyn AiProviderProfileRepository>,
    credentials: Arc<dyn CredentialStore>,
    catalog: Arc<dyn AiModelCatalogPort>,
    recommender: Arc<dyn AiSettingsRecommendationPort>,
    agent_settings: Option<AgentSettingsIpcService>,
    trace: Option<Arc<dyn AgentTracePort>>,
}

impl AiProviderProfileService {
    pub fn new(
        profiles: Arc<dyn AiProviderProfileRepository>,
        credentials: Arc<dyn CredentialStore>,
        catalog: Arc<dyn AiModelCatalogPort>,
    ) -> Self {
        Self {
            profiles,
            credentials,
            catalog,
            recommender: Arc::new(UnavailableSettingsRecommendation),
            agent_settings: None,
            trace: None,
        }
    }

    pub fn with_settings_recommendation(
        mut self,
        recommender: Arc<dyn AiSettingsRecommendationPort>,
    ) -> Self {
        self.recommender = recommender;
        self
    }

    pub fn with_agent_settings(mut self, agent_settings: AgentSettingsIpcService) -> Self {
        self.agent_settings = Some(agent_settings);
        self
    }

    /// 注入共享轨迹 collector。Provider 轨迹只记录阶段，不记录 endpoint、凭据或模型响应。
    pub fn with_trace(mut self, trace: Arc<dyn AgentTracePort>) -> Self {
        self.trace = Some(trace);
        self
    }

    /// `ai_provider_profile_list`：列出全部 profile 并附带各自的凭据配置事实。
    pub async fn list(&self) -> Result<AiProviderProfileListResultDto, AppError> {
        let profiles = self.profiles.list().await?;
        let mut dtos = Vec::with_capacity(profiles.len());
        for profile in &profiles {
            let configured = self.credential_configured(profile.profile_id()).await?;
            dtos.push(to_dto(profile, configured)?);
        }
        Ok(AiProviderProfileListResultDto {
            schema_version: 1,
            profiles: dtos,
        })
    }

    /// `ai_provider_profile_get`。
    pub async fn get(&self, profile_id: &str) -> Result<AiProviderProfileDto, AppError> {
        let profile = self.load(profile_id).await?;
        let configured = self.credential_configured(profile.profile_id()).await?;
        to_dto(&profile, configured)
    }

    /// `ai_provider_profile_upsert`：校验 + CAS 写入，返回**持久化后**的 profile。
    ///
    /// - profile 字段（id / 显示名 / endpoint / 模型 id）在 `AiProviderProfile::new`
    ///   里全部校验，非法输入零写入。
    /// - `expected_revision` 不匹配返回 `AI_PROVIDER_PROFILE_REVISION_CONFLICT`
    ///   （`retryable = true`：UI 应重新读取再改）。
    ///
    /// 返回值取自 `cas_upsert` 回读到的行，而不是本地候选值：更新时数据库保留原
    /// `created_at`，用本地候选值构造 DTO 会让响应里的 `createdAt` 与随后 `get`
    /// 读到的权威状态不一致 —— 一个"写成功却报错值"的响应。
    pub async fn upsert(
        &self,
        request: AiProviderProfileUpsertRequest,
    ) -> Result<AiProviderProfileDto, AppError> {
        let now = haven_common::UtcMillis::now().0;
        let candidate = AiProviderProfile::new(
            request.profile_id,
            request.display_name,
            request.kind.into(),
            request.endpoint,
            request.enabled,
            request.selected_model_id,
            now,
        )?;
        let stored = self
            .profiles
            .cas_upsert(&candidate, request.expected_revision.as_deref())
            .await?
            .ok_or_else(revision_conflict)?;
        let configured = self.credential_configured(stored.profile_id()).await?;
        to_dto(&stored, configured)
    }

    /// `ai_provider_profile_delete`：先清理凭据，再 CAS 删除 profile 行。
    ///
    /// **顺序不可颠倒**。先删行再删凭据时，凭据删除失败会留下没有任何行引用的孤儿
    /// secret —— 用户再也无法从 UI 管理它。反过来，凭据已删而 CAS 因并发失败，留下的
    /// 是一个"存在但未配置密钥"的 profile：状态可见、可重新配置、可再次删除。两侧都能
    /// 出错时，选择可恢复的那一侧。
    ///
    /// 调用方携带的 `expected_revision` 在**删除凭据之前**先与刚读到的版本比对：
    /// 用过期版本请求删除是一个可预见的用户错误，不应该以"密钥已被销毁"作为代价。
    /// 只有"读之后、CAS 之前被并发写者推进版本"这个无法用单条 SQL 消除的窗口，才会
    /// 出现"凭据已删但行仍在"的中间态——这是文档化的取舍，且该状态可恢复
    /// （profile 仍在，UI 显示未配置，可重设密钥或再次删除）。
    ///
    /// 凭据删除失败必须**中止**整次删除并把错误交给 UI，不得静默跳过。
    pub async fn delete(
        &self,
        request: AiProviderProfileDeleteRequest,
    ) -> Result<AiProviderProfileDeleteResultDto, AppError> {
        let profile = self.load(&request.profile_id).await?;
        // 先做纯读比对：零副作用地拒绝可预见的过期删除请求。
        if let Some(expected) = request.expected_revision.as_deref()
            && profile.revision() != Some(expected)
        {
            return Err(revision_conflict());
        }
        let target = credential_target(profile.profile_id())?;
        let credential_deleted = self.credentials.delete(&target).await?;
        // expected_revision 为 None 时使用刚读到的版本：删除仍然是数据库层的条件删除，
        // 只是条件取自一次新鲜读取，而不是调用方手里的旧值。
        let expected = request.expected_revision.as_deref().or(profile.revision());
        match self
            .profiles
            .cas_delete(profile.profile_id(), expected)
            .await?
        {
            AiProviderProfileDeleteOutcome::Deleted => Ok(AiProviderProfileDeleteResultDto {
                profile_id: profile.profile_id().to_owned(),
                credential_deleted,
            }),
            AiProviderProfileDeleteOutcome::NotFound => Err(not_found(profile.profile_id())),
            AiProviderProfileDeleteOutcome::RevisionConflict => Err(revision_conflict()),
        }
    }

    /// `ai_provider_models_list`：按需发现模型目录。
    ///
    /// 空态是**诚实状态而不是错误**：
    /// - profile 被禁用 → `Disabled`，不发请求；
    /// - profile 没有凭据 → `NoCredential`，不发请求；
    /// - Provider 成功返回但没有模型 → `Empty`。
    ///
    /// 只有真正的失败（不可达 / 非 2xx / 非法 JSON）才返回 `Err`；不吞错、不伪造模型。
    pub async fn models_catalog(
        &self,
        request: AiProviderModelsListRequest,
    ) -> Result<AiProviderModelsCatalogDto, AppError> {
        let profile = self.load(&request.profile_id).await?;
        if !profile.enabled() {
            return Ok(empty_catalog(
                profile.profile_id(),
                AiProviderModelsCatalogStateDto::Disabled,
            ));
        }
        let target = credential_target(profile.profile_id())?;
        let Some(secret) = self.credentials.get(&target).await? else {
            return Ok(empty_catalog(
                profile.profile_id(),
                AiProviderModelsCatalogStateDto::NoCredential,
            ));
        };
        let models = self
            .catalog
            .list_models(profile.kind(), profile.endpoint(), &secret)
            .await?;
        let state = if models.is_empty() {
            AiProviderModelsCatalogStateDto::Empty
        } else {
            AiProviderModelsCatalogStateDto::Ready
        };
        Ok(AiProviderModelsCatalogDto {
            schema_version: 1,
            profile_id: profile.profile_id().to_owned(),
            state,
            models: models.iter().map(AiProviderModelDto::from_domain).collect(),
        })
    }

    /// 生成一份绑定当前全局阅读设置上下文的建议，并停在 `pending` Proposal。
    ///
    /// 此方法没有 Apply 路径：Provider 只返回 typed 值，Proposal 创建仍由
    /// `AgentSettingsIpcService` 完成，批准与 CAS 继续留在既有用户确认入口。
    pub async fn generate_settings_recommendation(
        &self,
        request: AiSettingsRecommendationRequest,
    ) -> Result<AiSettingsRecommendationResult, AppError> {
        let session_id = request
            .session_id
            .parse::<AgentSessionId>()
            .map_err(|_| invalid_argument("session_id 非法"))?;
        let request_id = request
            .request_id
            .parse::<AgentRequestId>()
            .map_err(|_| invalid_argument("request_id 非法"))?;
        record_best_effort(
            self.trace.as_ref(),
            session_id,
            request_id,
            &request.context_id,
            &request.context_hash,
            AgentEventKind::RequestStarted,
        )
        .await;
        let profile = self.load(&request.profile_id).await?;
        if !profile.enabled() {
            return Err(recommendation_disabled());
        }
        let target = credential_target(profile.profile_id())?;
        let Some(secret) = self.credentials.get(&target).await? else {
            return Err(recommendation_no_credential());
        };
        let Some(selected_model_id) = profile.selected_model_id() else {
            return Err(recommendation_no_selected_model());
        };
        let models = self
            .catalog
            .list_models(profile.kind(), profile.endpoint(), &secret)
            .await?;
        let Some(model) = models.iter().find(|model| {
            model.model_id == selected_model_id && model.chat == AiModelCapability::Supported
        }) else {
            return Err(recommendation_model_unavailable());
        };

        let agent_settings = self
            .agent_settings
            .clone()
            .ok_or_else(recommendation_unavailable)?;
        let context = agent_settings.context().await?;
        if context.context_id().to_string() != request.context_id
            || context.context_hash() != request.context_hash
            || context.revision() != request.base_revision.as_deref()
        {
            return Err(recommendation_context_stale());
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

        record_best_effort(
            self.trace.as_ref(),
            session_id,
            request_id,
            &request.context_id,
            &request.context_hash,
            AgentEventKind::ProviderRequest,
        )
        .await;
        let recommendation = self
            .recommender
            .recommend_settings(
                profile.kind(),
                profile.endpoint(),
                &secret,
                &AiSettingsRecommendationInput {
                    model_id: model.model_id.clone(),
                    reading: context.snapshot().reading().clone(),
                },
            )
            .await?;
        record_best_effort(
            self.trace.as_ref(),
            session_id,
            request_id,
            &request.context_id,
            &request.context_hash,
            AgentEventKind::ProviderResponse,
        )
        .await;
        let patch: PreferenceReadingPatchDto = recommendation.patch().clone().into();
        record_best_effort(
            self.trace.as_ref(),
            session_id,
            request_id,
            &request.context_id,
            &request.context_hash,
            AgentEventKind::StructuredOutputValidated,
        )
        .await;
        let proposal = agent_settings
            .create_proposal(
                session_id,
                request_id,
                &AgentSettingsProposalCreateRequest {
                    session_id: request.session_id,
                    request_id: request.request_id,
                    context_id: request.context_id,
                    context_hash: request.context_hash,
                    base_revision: request.base_revision,
                    patch: patch.clone(),
                },
            )
            .await?;

        Ok(AiSettingsRecommendationResult {
            schema_version: 1,
            profile_id: profile.profile_id().to_owned(),
            model_id: model.model_id.clone(),
            explanation: recommendation.explanation().map(str::to_owned),
            recommended_patch: patch,
            proposal,
        })
    }

    async fn load(&self, profile_id: &str) -> Result<AiProviderProfile, AppError> {
        self.profiles
            .get(profile_id)
            .await?
            .ok_or_else(|| not_found(profile_id))
    }

    /// 只读取凭据**存在性**；secret 本体在本方法内即被丢弃。
    async fn credential_configured(&self, profile_id: &str) -> Result<bool, AppError> {
        let target = credential_target(profile_id)?;
        Ok(self.credentials.get(&target).await?.is_some())
    }
}

/// `haven:ai:<profile-id>`。所有 AI 凭据读写都必须经过它。
pub fn credential_target(profile_id: &str) -> Result<CredentialRef, AppError> {
    CredentialRef::new_scoped(AI_CREDENTIAL_PROVIDER, profile_id)
        .map_err(|error| invalid_argument(error.user_message()))
}

fn to_dto(
    profile: &AiProviderProfile,
    credential_configured: bool,
) -> Result<AiProviderProfileDto, AppError> {
    Ok(AiProviderProfileDto {
        schema_version: 1,
        profile_id: profile.profile_id().to_owned(),
        display_name: profile.display_name().to_owned(),
        kind: AiProviderKindDto::from(profile.kind()),
        endpoint: profile.endpoint().to_owned(),
        enabled: profile.enabled(),
        selected_model_id: profile.selected_model_id().map(str::to_owned),
        credential_configured,
        revision: profile
            .revision()
            .ok_or_else(|| {
                AppError::new(
                    "AI_PROVIDER_REVISION_INVALID",
                    ErrorKind::Internal,
                    "profile 缺少持久化版本号",
                    false,
                )
            })?
            .to_owned(),
        created_at: utc_millis_to_rfc3339(haven_common::UtcMillis(profile.created_at())),
        updated_at: utc_millis_to_rfc3339(haven_common::UtcMillis(profile.updated_at())),
    })
}

fn empty_catalog(
    profile_id: &str,
    state: AiProviderModelsCatalogStateDto,
) -> AiProviderModelsCatalogDto {
    AiProviderModelsCatalogDto {
        schema_version: 1,
        profile_id: profile_id.to_owned(),
        state,
        models: Vec::new(),
    }
}

fn not_found(profile_id: &str) -> AppError {
    AppError::new(
        "AI_PROVIDER_PROFILE_NOT_FOUND",
        ErrorKind::NotFound,
        format!("未找到 AI Provider 配置：{profile_id}"),
        false,
    )
}

fn revision_conflict() -> AppError {
    AppError::new(
        "AI_PROVIDER_PROFILE_REVISION_CONFLICT",
        ErrorKind::Conflict,
        "AI Provider 配置已被其它操作修改，请重新读取后再试",
        true,
    )
}

fn invalid_argument(message: impl Into<String>) -> AppError {
    AppError::new("INVALID_ARGUMENT", ErrorKind::Validation, message, false)
}

fn recommendation_unavailable() -> AppError {
    AppError::new(
        "AI_PROVIDER_RECOMMENDATION_UNAVAILABLE",
        ErrorKind::Unsupported,
        "当前没有可用的 AI 设置建议模型",
        false,
    )
}

fn recommendation_disabled() -> AppError {
    AppError::new(
        "AI_PROVIDER_DISABLED",
        ErrorKind::Unsupported,
        "AI Provider 已禁用",
        false,
    )
}

fn recommendation_no_credential() -> AppError {
    AppError::new(
        "AI_PROVIDER_NO_CREDENTIAL",
        ErrorKind::Unauthorized,
        "AI Provider 尚未配置凭据",
        false,
    )
}

fn recommendation_no_selected_model() -> AppError {
    AppError::new(
        "AI_PROVIDER_NO_SELECTED_MODEL",
        ErrorKind::Validation,
        "AI Provider 尚未选择模型",
        false,
    )
}

fn recommendation_model_unavailable() -> AppError {
    AppError::new(
        "AI_PROVIDER_MODEL_UNAVAILABLE",
        ErrorKind::Unsupported,
        "当前选中的模型不可用于设置建议",
        false,
    )
}

fn recommendation_context_stale() -> AppError {
    AppError::new(
        "AI_PROVIDER_SETTINGS_CONTEXT_STALE",
        ErrorKind::Conflict,
        "设置上下文已变化，请重新读取后再生成建议",
        true,
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::wire::AiModelCapabilityDto;
    use haven_domain::ai_provider::AiModelCapability;
    use std::collections::HashMap;
    use std::sync::Mutex;

    #[derive(Default)]
    struct MemoryCredentialStore {
        entries: Mutex<HashMap<String, String>>,
        /// 记录每次删除，用于断言"删除 profile 一定先清理凭据"。
        deleted: Mutex<Vec<String>>,
        fail_delete: Mutex<bool>,
    }

    #[async_trait]
    impl CredentialStore for MemoryCredentialStore {
        async fn set(&self, target: &CredentialRef, secret: &SecretString) -> Result<(), AppError> {
            self.entries
                .lock()
                .unwrap()
                .insert(target.as_str().to_owned(), secret.expose().to_owned());
            Ok(())
        }

        async fn get(&self, target: &CredentialRef) -> Result<Option<SecretString>, AppError> {
            Ok(self
                .entries
                .lock()
                .unwrap()
                .get(target.as_str())
                .map(|value| SecretString::new(value.clone())))
        }

        async fn delete(&self, target: &CredentialRef) -> Result<bool, AppError> {
            if *self.fail_delete.lock().unwrap() {
                return Err(AppError::new(
                    "CREDENTIAL_ACCESS_FAILED",
                    ErrorKind::Security,
                    "凭据存储不可用",
                    true,
                ));
            }
            self.deleted
                .lock()
                .unwrap()
                .push(target.as_str().to_owned());
            Ok(self
                .entries
                .lock()
                .unwrap()
                .remove(target.as_str())
                .is_some())
        }
    }

    #[derive(Default)]
    struct MemoryProfileRepository {
        rows: Mutex<HashMap<String, (AiProviderProfile, String)>>,
        /// 下一次 cas_upsert 强制冲突（模拟并发写者抢先提交）。
        force_conflict: Mutex<bool>,
        /// 下一次 cas_delete 返回 NotFound（模拟读后被并发删除）。
        force_not_found: Mutex<bool>,
    }

    /// 内存替身给新建行钉死的 `created_at`。
    ///
    /// `utc_millis_to_rfc3339` 只到秒，用"两次 `UtcMillis::now()` 恰好落在不同秒"
    /// 来验证「响应必须回读持久化行」会变成 flaky 测试。固定值让这条断言确定。
    const MEMORY_CREATED_AT: i64 = 1_700_000_000_000;
    const MEMORY_CREATED_AT_RFC3339: &str = "2023-11-14T22:13:20Z";

    #[async_trait]
    impl AiProviderProfileRepository for MemoryProfileRepository {
        async fn list(&self) -> Result<Vec<AiProviderProfile>, AppError> {
            let rows = self.rows.lock().unwrap();
            let mut values: Vec<AiProviderProfile> =
                rows.values().map(|(profile, _)| profile.clone()).collect();
            values.sort_by(|a, b| a.profile_id().cmp(b.profile_id()));
            Ok(values)
        }

        async fn get(&self, profile_id: &str) -> Result<Option<AiProviderProfile>, AppError> {
            Ok(self
                .rows
                .lock()
                .unwrap()
                .get(profile_id)
                .map(|(profile, _)| profile.clone()))
        }

        async fn cas_upsert(
            &self,
            profile: &AiProviderProfile,
            expected_revision: Option<&str>,
        ) -> Result<Option<AiProviderProfile>, AppError> {
            if *self.force_conflict.lock().unwrap() {
                return Ok(None);
            }
            let mut rows = self.rows.lock().unwrap();
            let current = rows.get(profile.profile_id());
            let matches = match (current, expected_revision) {
                (None, None) => true,
                (Some((_, revision)), Some(expected)) => revision == expected,
                _ => false,
            };
            if !matches {
                return Ok(None);
            }
            let revision = format!("rev-{}", rows.len() + 1);
            let created_at = current
                .map(|(existing, _)| existing.created_at())
                .unwrap_or(MEMORY_CREATED_AT);
            let stored = AiProviderProfile::from_stored(
                profile.profile_id().to_owned(),
                profile.display_name().to_owned(),
                profile.kind(),
                profile.endpoint().to_owned(),
                profile.enabled(),
                profile.selected_model_id().map(str::to_owned),
                revision.clone(),
                created_at,
                profile.updated_at(),
            )?;
            rows.insert(profile.profile_id().to_owned(), (stored.clone(), revision));
            Ok(Some(stored))
        }

        async fn cas_delete(
            &self,
            profile_id: &str,
            expected_revision: Option<&str>,
        ) -> Result<AiProviderProfileDeleteOutcome, AppError> {
            if *self.force_not_found.lock().unwrap() {
                return Ok(AiProviderProfileDeleteOutcome::NotFound);
            }
            let mut rows = self.rows.lock().unwrap();
            let Some((_, revision)) = rows.get(profile_id) else {
                return Ok(AiProviderProfileDeleteOutcome::NotFound);
            };
            if Some(revision.as_str()) != expected_revision {
                return Ok(AiProviderProfileDeleteOutcome::RevisionConflict);
            }
            rows.remove(profile_id);
            Ok(AiProviderProfileDeleteOutcome::Deleted)
        }
    }

    #[derive(Default)]
    struct RecordingCatalog {
        calls: Mutex<Vec<String>>,
        models: Mutex<Vec<AiModelDescriptor>>,
        error: Mutex<Option<AppError>>,
        seen_secret: Mutex<Option<String>>,
    }

    #[async_trait]
    impl AiModelCatalogPort for RecordingCatalog {
        async fn list_models(
            &self,
            _kind: AiProviderKind,
            endpoint: &str,
            secret: &SecretString,
        ) -> Result<Vec<AiModelDescriptor>, AppError> {
            self.calls.lock().unwrap().push(endpoint.to_owned());
            *self.seen_secret.lock().unwrap() = Some(secret.expose().to_owned());
            if let Some(error) = self.error.lock().unwrap().clone() {
                return Err(error);
            }
            Ok(self.models.lock().unwrap().clone())
        }
    }

    #[derive(Default)]
    struct RecordingRecommender {
        calls: Mutex<Vec<String>>,
    }

    #[async_trait]
    impl AiSettingsRecommendationPort for RecordingRecommender {
        async fn recommend_settings(
            &self,
            _kind: AiProviderKind,
            _endpoint: &str,
            _secret: &SecretString,
            input: &AiSettingsRecommendationInput,
        ) -> Result<AiSettingsRecommendation, AppError> {
            self.calls.lock().unwrap().push(input.model_id.clone());
            AiSettingsRecommendation::new(
                haven_domain::settings::ReadingPatch {
                    font_size: Some(haven_domain::settings::ReadingFontSize::Large),
                    ..haven_domain::settings::ReadingPatch::default()
                },
                Some("根据当前阅读设置生成建议".into()),
            )
        }
    }

    struct Fixture {
        service: AiProviderProfileService,
        credentials: Arc<MemoryCredentialStore>,
        profiles: Arc<MemoryProfileRepository>,
        catalog: Arc<RecordingCatalog>,
    }

    fn fixture() -> Fixture {
        let credentials = Arc::new(MemoryCredentialStore::default());
        let profiles = Arc::new(MemoryProfileRepository::default());
        let catalog = Arc::new(RecordingCatalog::default());
        Fixture {
            service: AiProviderProfileService::new(
                profiles.clone(),
                credentials.clone(),
                catalog.clone(),
            ),
            credentials,
            profiles,
            catalog,
        }
    }

    fn upsert_request(
        profile_id: &str,
        expected_revision: Option<&str>,
    ) -> AiProviderProfileUpsertRequest {
        AiProviderProfileUpsertRequest {
            profile_id: profile_id.to_owned(),
            display_name: "自建网关".into(),
            kind: AiProviderKindDto::OpenAiCompatible,
            endpoint: "https://gateway.example.invalid/v1".into(),
            enabled: true,
            selected_model_id: None,
            expected_revision: expected_revision.map(str::to_owned),
        }
    }

    #[tokio::test]
    async fn upsert_then_list_then_get_round_trip() {
        let fixture = fixture();
        let created = fixture
            .service
            .upsert(upsert_request("gw-main", None))
            .await
            .unwrap();
        assert_eq!(created.profile_id, "gw-main");
        assert_eq!(created.kind, AiProviderKindDto::OpenAiCompatible);
        assert!(!created.credential_configured, "尚未写入凭据");
        assert!(!created.revision.is_empty());

        let listed = fixture.service.list().await.unwrap();
        assert_eq!(listed.schema_version, 1);
        assert_eq!(listed.profiles.len(), 1);
        assert_eq!(listed.profiles[0].profile_id, "gw-main");

        let fetched = fixture.service.get("gw-main").await.unwrap();
        assert_eq!(fetched, created);
    }

    #[tokio::test]
    async fn upsert_rejects_invalid_input_without_writing() {
        let fixture = fixture();
        for (request, expected_code) in [
            (
                AiProviderProfileUpsertRequest {
                    profile_id: "bad/id".into(),
                    ..upsert_request("gw", None)
                },
                "AI_PROVIDER_PROFILE_ID_INVALID",
            ),
            (
                AiProviderProfileUpsertRequest {
                    endpoint: "http://127.0.0.1:11434/v1".into(),
                    ..upsert_request("gw", None)
                },
                "AI_PROVIDER_ENDPOINT_INVALID",
            ),
            (
                AiProviderProfileUpsertRequest {
                    endpoint: "C:\\gateway".into(),
                    ..upsert_request("gw", None)
                },
                "AI_PROVIDER_ENDPOINT_INVALID",
            ),
            (
                AiProviderProfileUpsertRequest {
                    endpoint: "https://gateway.example.invalid/v1?key=secret".into(),
                    ..upsert_request("gw", None)
                },
                "AI_PROVIDER_ENDPOINT_INVALID",
            ),
            (
                AiProviderProfileUpsertRequest {
                    display_name: "a/b".into(),
                    ..upsert_request("gw", None)
                },
                "AI_PROVIDER_DISPLAY_NAME_INVALID",
            ),
            (
                AiProviderProfileUpsertRequest {
                    selected_model_id: Some("bad model".into()),
                    ..upsert_request("gw", None)
                },
                "AI_PROVIDER_MODEL_ID_INVALID",
            ),
        ] {
            let error = fixture.service.upsert(request).await.unwrap_err();
            assert_eq!(error.code().as_str(), expected_code);
            assert!(!error.retryable(), "校验错误不可重试");
        }
        assert!(
            fixture.service.list().await.unwrap().profiles.is_empty(),
            "非法输入必须零写入"
        );
    }

    #[tokio::test]
    async fn upsert_cas_conflict_is_stable_and_zero_write() {
        let fixture = fixture();
        let created = fixture
            .service
            .upsert(upsert_request("gw", None))
            .await
            .unwrap();

        // 用过期版本重复写：必须冲突，且内容不变。
        let error = fixture
            .service
            .upsert(AiProviderProfileUpsertRequest {
                display_name: "被覆盖的名字".into(),
                ..upsert_request("gw", None)
            })
            .await
            .unwrap_err();
        assert_eq!(
            error.code().as_str(),
            "AI_PROVIDER_PROFILE_REVISION_CONFLICT"
        );
        assert!(error.retryable());
        assert_eq!(fixture.service.get("gw").await.unwrap(), created);

        // 用正确版本更新成功，revision 必须变化。
        let updated = fixture
            .service
            .upsert(AiProviderProfileUpsertRequest {
                display_name: "改名后的网关".into(),
                ..upsert_request("gw", Some(&created.revision))
            })
            .await
            .unwrap();
        assert_ne!(updated.revision, created.revision);
        assert_eq!(updated.display_name, "改名后的网关");
        assert_eq!(
            updated.created_at, created.created_at,
            "更新不得改写创建时间"
        );
    }

    /// upsert 的响应必须等于随后 `get` 读到的权威状态。
    ///
    /// 曾经的实现用本地候选值（`created_at = now`）拼响应，而数据库在更新时保留原
    /// `created_at`，于是"写成功"的响应与"读回来"的状态不一致。
    #[tokio::test]
    async fn upsert_response_equals_authoritative_row() {
        let fixture = fixture();
        let created = fixture
            .service
            .upsert(upsert_request("gw", None))
            .await
            .unwrap();
        // 存储侧把新建行的 created_at 钉在固定值：若响应回传的是本地候选值，
        // 这里就会是"当前时间"而不是这个固定时刻。
        assert_eq!(created.created_at, MEMORY_CREATED_AT_RFC3339);
        assert_eq!(fixture.service.get("gw").await.unwrap(), created);

        let updated = fixture
            .service
            .upsert(AiProviderProfileUpsertRequest {
                display_name: "改名后的网关".into(),
                ..upsert_request("gw", Some(&created.revision))
            })
            .await
            .unwrap();
        assert_eq!(
            updated.created_at, MEMORY_CREATED_AT_RFC3339,
            "更新后的响应必须回读持久化行，而不是回传本地候选值"
        );
        assert_eq!(
            fixture.service.get("gw").await.unwrap(),
            updated,
            "upsert 响应必须与随后的 get 完全一致"
        );
    }

    #[tokio::test]
    async fn get_reports_not_found_for_unknown_profile() {
        let fixture = fixture();
        let error = fixture.service.get("missing").await.unwrap_err();
        assert_eq!(error.code().as_str(), "AI_PROVIDER_PROFILE_NOT_FOUND");
        assert!(!error.retryable());
    }

    #[tokio::test]
    async fn delete_clears_credential_before_removing_row() {
        let fixture = fixture();
        fixture
            .service
            .upsert(upsert_request("gw", None))
            .await
            .unwrap();
        let target = credential_target("gw").unwrap();
        assert_eq!(
            target.as_str(),
            "haven:ai:gw",
            "凭据命名空间必须是 haven:ai:<id>"
        );
        fixture
            .credentials
            .set(&target, &SecretString::new("sk-not-a-real-key"))
            .await
            .unwrap();
        assert!(
            fixture
                .service
                .get("gw")
                .await
                .unwrap()
                .credential_configured,
            "写入凭据后状态必须为已配置"
        );

        let result = fixture
            .service
            .delete(AiProviderProfileDeleteRequest {
                profile_id: "gw".into(),
                expected_revision: None,
            })
            .await
            .unwrap();
        assert!(result.credential_deleted);
        assert_eq!(result.profile_id, "gw");
        assert_eq!(
            fixture.credentials.deleted.lock().unwrap().as_slice(),
            ["haven:ai:gw"],
            "必须先对同一命名空间的凭据执行受控删除"
        );
        assert!(fixture.service.list().await.unwrap().profiles.is_empty());
        assert!(
            fixture.credentials.get(&target).await.unwrap().is_none(),
            "删除 profile 不得留下孤儿 secret"
        );
    }

    #[tokio::test]
    async fn delete_aborts_when_credential_cleanup_fails() {
        let fixture = fixture();
        fixture
            .service
            .upsert(upsert_request("gw", None))
            .await
            .unwrap();
        *fixture.credentials.fail_delete.lock().unwrap() = true;

        let error = fixture
            .service
            .delete(AiProviderProfileDeleteRequest {
                profile_id: "gw".into(),
                expected_revision: None,
            })
            .await
            .unwrap_err();
        assert_eq!(error.code().as_str(), "CREDENTIAL_ACCESS_FAILED");
        assert_eq!(
            fixture.service.list().await.unwrap().profiles.len(),
            1,
            "凭据清理失败必须中止整次删除，profile 行保留"
        );
    }

    #[tokio::test]
    async fn delete_conflict_reports_revision_conflict() {
        let fixture = fixture();
        fixture
            .service
            .upsert(upsert_request("gw", None))
            .await
            .unwrap();
        let error = fixture
            .service
            .delete(AiProviderProfileDeleteRequest {
                profile_id: "gw".into(),
                expected_revision: Some("stale-revision".into()),
            })
            .await
            .unwrap_err();
        assert_eq!(
            error.code().as_str(),
            "AI_PROVIDER_PROFILE_REVISION_CONFLICT"
        );
        assert_eq!(fixture.service.list().await.unwrap().profiles.len(), 1);
    }

    /// 被拒绝的删除必须零副作用：过期版本是**可预见的**用户错误
    /// （UI 拿着旧列表点删除），不能以"API Key 已被销毁"作为代价。
    #[tokio::test]
    async fn rejected_delete_does_not_destroy_the_api_key() {
        let fixture = fixture();
        fixture
            .service
            .upsert(upsert_request("gw", None))
            .await
            .unwrap();
        let target = credential_target("gw").unwrap();
        fixture
            .credentials
            .set(&target, &SecretString::new("sk-not-a-real-key"))
            .await
            .unwrap();

        let error = fixture
            .service
            .delete(AiProviderProfileDeleteRequest {
                profile_id: "gw".into(),
                expected_revision: Some("stale-revision".into()),
            })
            .await
            .unwrap_err();
        assert_eq!(
            error.code().as_str(),
            "AI_PROVIDER_PROFILE_REVISION_CONFLICT"
        );
        assert!(
            fixture.credentials.deleted.lock().unwrap().is_empty(),
            "CAS 前置比对失败时不得触碰凭据"
        );
        assert!(
            fixture.credentials.get(&target).await.unwrap().is_some(),
            "过期版本删除不得销毁 API Key"
        );
        assert!(
            fixture
                .service
                .get("gw")
                .await
                .unwrap()
                .credential_configured,
            "profile 与凭据都必须原样保留"
        );
    }

    /// 携带**正确**版本删除时，凭据与行都必须被清理。
    #[tokio::test]
    async fn delete_with_matching_revision_clears_credential_and_row() {
        let fixture = fixture();
        let created = fixture
            .service
            .upsert(upsert_request("gw", None))
            .await
            .unwrap();
        let target = credential_target("gw").unwrap();
        fixture
            .credentials
            .set(&target, &SecretString::new("sk-not-a-real-key"))
            .await
            .unwrap();

        let result = fixture
            .service
            .delete(AiProviderProfileDeleteRequest {
                profile_id: "gw".into(),
                expected_revision: Some(created.revision.clone()),
            })
            .await
            .unwrap();
        assert!(result.credential_deleted);
        assert!(fixture.credentials.get(&target).await.unwrap().is_none());
        assert_eq!(
            fixture.credentials.deleted.lock().unwrap().as_slice(),
            ["haven:ai:gw"]
        );
        assert!(fixture.service.list().await.unwrap().profiles.is_empty());
    }

    #[tokio::test]
    async fn delete_reports_not_found_when_row_disappears_after_read() {
        let fixture = fixture();
        fixture
            .service
            .upsert(upsert_request("gw", None))
            .await
            .unwrap();
        // 模拟"读取之后、删除之前"被并发方删掉：CAS 返回 NotFound。
        *fixture.profiles.force_not_found.lock().unwrap() = true;
        let error = fixture
            .service
            .delete(AiProviderProfileDeleteRequest {
                profile_id: "gw".into(),
                expected_revision: None,
            })
            .await
            .unwrap_err();
        assert_eq!(error.code().as_str(), "AI_PROVIDER_PROFILE_NOT_FOUND");
    }

    #[tokio::test]
    async fn models_catalog_is_honestly_empty_without_credential_or_when_disabled() {
        let fixture = fixture();
        fixture
            .service
            .upsert(upsert_request("gw", None))
            .await
            .unwrap();

        let no_credential = fixture
            .service
            .models_catalog(AiProviderModelsListRequest {
                profile_id: "gw".into(),
            })
            .await
            .unwrap();
        assert_eq!(
            no_credential.state,
            AiProviderModelsCatalogStateDto::NoCredential
        );
        assert!(no_credential.models.is_empty());
        assert!(
            fixture.catalog.calls.lock().unwrap().is_empty(),
            "没有凭据时不得发起网络请求"
        );

        let created = fixture.service.get("gw").await.unwrap();
        fixture
            .service
            .upsert(AiProviderProfileUpsertRequest {
                enabled: false,
                ..upsert_request("gw", Some(&created.revision))
            })
            .await
            .unwrap();
        let target = credential_target("gw").unwrap();
        fixture
            .credentials
            .set(&target, &SecretString::new("sk-not-a-real-key"))
            .await
            .unwrap();

        let disabled = fixture
            .service
            .models_catalog(AiProviderModelsListRequest {
                profile_id: "gw".into(),
            })
            .await
            .unwrap();
        assert_eq!(disabled.state, AiProviderModelsCatalogStateDto::Disabled);
        assert!(disabled.models.is_empty());
        assert!(
            fixture.catalog.calls.lock().unwrap().is_empty(),
            "禁用的 profile 不得发起网络请求"
        );
    }

    #[tokio::test]
    async fn models_catalog_projects_unknown_capabilities_without_guessing() {
        let fixture = fixture();
        fixture
            .service
            .upsert(upsert_request("gw", None))
            .await
            .unwrap();
        let target = credential_target("gw").unwrap();
        fixture
            .credentials
            .set(&target, &SecretString::new("sk-not-a-real-key"))
            .await
            .unwrap();
        *fixture.catalog.models.lock().unwrap() = vec![
            AiModelDescriptor::new(
                "gpt-4o-vision-preview".into(),
                None,
                Some(1_700_000_000),
                None,
                AiModelCapability::Unknown,
                AiModelCapability::Unknown,
                AiModelCapability::Unknown,
            )
            .unwrap(),
        ];

        let catalog = fixture
            .service
            .models_catalog(AiProviderModelsListRequest {
                profile_id: "gw".into(),
            })
            .await
            .unwrap();
        assert_eq!(catalog.state, AiProviderModelsCatalogStateDto::Ready);
        assert_eq!(catalog.models.len(), 1);
        assert_eq!(
            catalog.models[0].vision,
            AiModelCapabilityDto::Unknown,
            "名字里有 vision 也不构成能力声明"
        );
        assert_eq!(catalog.models[0].embedding, AiModelCapabilityDto::Unknown);
        assert_eq!(
            fixture.catalog.calls.lock().unwrap().as_slice(),
            ["https://gateway.example.invalid/v1"],
            "端口收到的是 profile 的规范化端点"
        );
    }

    #[tokio::test]
    async fn models_catalog_surfaces_discovery_failure_instead_of_faking_models() {
        let fixture = fixture();
        fixture
            .service
            .upsert(upsert_request("gw", None))
            .await
            .unwrap();
        let target = credential_target("gw").unwrap();
        fixture
            .credentials
            .set(&target, &SecretString::new("sk-not-a-real-key"))
            .await
            .unwrap();
        *fixture.catalog.error.lock().unwrap() = Some(AppError::new(
            "AI_PROVIDER_MODELS_HTTP_ERROR",
            ErrorKind::Network,
            "模型目录请求失败（HTTP 503）",
            true,
        ));

        let error = fixture
            .service
            .models_catalog(AiProviderModelsListRequest {
                profile_id: "gw".into(),
            })
            .await
            .unwrap_err();
        assert_eq!(error.code().as_str(), "AI_PROVIDER_MODELS_HTTP_ERROR");
        assert!(error.retryable());
    }

    #[tokio::test]
    async fn model_dto_never_carries_secret_material() {
        let fixture = fixture();
        fixture
            .service
            .upsert(upsert_request("gw", None))
            .await
            .unwrap();
        let target = credential_target("gw").unwrap();
        fixture
            .credentials
            .set(&target, &SecretString::new("sk-super-secret-value"))
            .await
            .unwrap();

        let profile = fixture.service.get("gw").await.unwrap();
        let encoded = serde_json::to_string(&profile).unwrap();
        for forbidden in [
            "sk-super-secret-value",
            "haven:ai:",
            "credentialRef",
            "credential_ref",
            "secret",
            "target",
        ] {
            assert!(
                !encoded.contains(forbidden),
                "profile 投影不得包含 {forbidden}: {encoded}"
            );
        }

        let catalog = fixture
            .service
            .models_catalog(AiProviderModelsListRequest {
                profile_id: "gw".into(),
            })
            .await
            .unwrap();
        let encoded = serde_json::to_string(&catalog).unwrap();
        assert!(!encoded.contains("sk-super-secret-value"));
    }

    #[tokio::test]
    async fn valid_structured_recommendation_creates_pending_proposal_without_writing_settings() {
        let fixture = fixture();
        let created = fixture
            .service
            .upsert(AiProviderProfileUpsertRequest {
                selected_model_id: Some("reader-model".into()),
                ..upsert_request("gw", None)
            })
            .await
            .unwrap();
        let target = credential_target(&created.profile_id).unwrap();
        fixture
            .credentials
            .set(&target, &SecretString::new("sk-test-only"))
            .await
            .unwrap();
        *fixture.catalog.models.lock().unwrap() = vec![
            AiModelDescriptor::new(
                "reader-model".into(),
                Some("Reader Model".into()),
                None,
                Some("fixture".into()),
                AiModelCapability::Supported,
                AiModelCapability::Unknown,
                AiModelCapability::Unknown,
            )
            .unwrap(),
        ];

        let agent_store = crate::services::agent_settings_ipc::tests::FakeStore::new();
        crate::services::agent_settings_ipc::tests::seed_reading(&agent_store, Some("rev-0001"));
        let agent_settings = crate::services::agent_settings_ipc::tests::service(&agent_store);
        let recommender = Arc::new(RecordingRecommender::default());
        let service = AiProviderProfileService::new(
            fixture.profiles.clone(),
            fixture.credentials.clone(),
            fixture.catalog.clone(),
        )
        .with_settings_recommendation(recommender.clone())
        .with_agent_settings(agent_settings.clone());

        let context = agent_settings.context().await.unwrap();
        let result = service
            .generate_settings_recommendation(AiSettingsRecommendationRequest {
                profile_id: "gw".into(),
                session_id: AgentSessionId::new().to_string(),
                request_id: AgentRequestId::new().to_string(),
                context_id: context.context_id().to_string(),
                context_hash: context.context_hash().to_owned(),
                base_revision: context.revision().map(str::to_owned),
            })
            .await
            .unwrap();

        assert_eq!(result.profile_id, "gw");
        assert_eq!(result.model_id, "reader-model");
        assert_eq!(
            result.explanation.as_deref(),
            Some("根据当前阅读设置生成建议")
        );
        assert_eq!(
            result.recommended_patch.font_size,
            Some(crate::wire::PreferenceReadingFontSizeDto::Large)
        );
        assert_eq!(
            result.proposal.status,
            crate::wire::AgentSettingsProposalStatusDto::Pending
        );
        assert_eq!(result.proposal.changes.len(), 1);
        assert_eq!(result.proposal.changes[0].key, "reading.fontSize");
        assert_eq!(result.proposal.changes[0].after, "large");
        assert_eq!(
            agent_store.target_writes(),
            0,
            "Provider 建议不得直接写设置"
        );
        assert_eq!(agent_store.revision().as_deref(), Some("rev-0001"));
        assert_eq!(
            agent_store.proposal_count(),
            1,
            "必须进入既有 Proposal store"
        );
        assert_eq!(
            recommender.calls.lock().unwrap().as_slice(),
            ["reader-model"]
        );

        let encoded = serde_json::to_string(&result).unwrap();
        for forbidden in ["sk-test-only", "haven:ai:", "credentialRef", "secret"] {
            assert!(
                !encoded.contains(forbidden),
                "建议结果不得携带敏感材料：{forbidden}"
            );
        }
    }

    #[tokio::test]
    async fn recommendation_preconditions_fail_closed_without_provider_calls() {
        let cases = [
            ("disabled", false, None, "AI_PROVIDER_DISABLED"),
            (
                "no_credential",
                true,
                Some("reader-model"),
                "AI_PROVIDER_NO_CREDENTIAL",
            ),
            (
                "no_selected_model",
                true,
                None,
                "AI_PROVIDER_NO_SELECTED_MODEL",
            ),
        ];

        for (profile_id, enabled, selected_model_id, expected_code) in cases {
            let fixture = fixture();
            fixture
                .service
                .upsert(AiProviderProfileUpsertRequest {
                    profile_id: profile_id.into(),
                    enabled,
                    selected_model_id: selected_model_id.map(str::to_owned),
                    ..upsert_request(profile_id, None)
                })
                .await
                .unwrap();
            if profile_id == "no_selected_model" {
                let target = credential_target(profile_id).unwrap();
                fixture
                    .credentials
                    .set(&target, &SecretString::new("sk-test-only"))
                    .await
                    .unwrap();
            }
            let agent_store = crate::services::agent_settings_ipc::tests::FakeStore::new();
            crate::services::agent_settings_ipc::tests::seed_reading(
                &agent_store,
                Some("rev-0001"),
            );
            let agent_settings = crate::services::agent_settings_ipc::tests::service(&agent_store);
            let context = agent_settings.context().await.unwrap();
            let recommender = Arc::new(RecordingRecommender::default());
            let service = AiProviderProfileService::new(
                fixture.profiles.clone(),
                fixture.credentials.clone(),
                fixture.catalog.clone(),
            )
            .with_settings_recommendation(recommender.clone())
            .with_agent_settings(agent_settings);

            let error = service
                .generate_settings_recommendation(AiSettingsRecommendationRequest {
                    profile_id: profile_id.into(),
                    session_id: AgentSessionId::new().to_string(),
                    request_id: AgentRequestId::new().to_string(),
                    context_id: context.context_id().to_string(),
                    context_hash: context.context_hash().to_owned(),
                    base_revision: context.revision().map(str::to_owned),
                })
                .await
                .unwrap_err();
            assert_eq!(error.code().as_str(), expected_code);
            assert!(fixture.catalog.calls.lock().unwrap().is_empty());
            assert!(recommender.calls.lock().unwrap().is_empty());
            assert_eq!(agent_store.target_writes(), 0);
            assert_eq!(agent_store.proposal_count(), 0);
        }
    }

    #[tokio::test]
    async fn unknown_or_unsupported_chat_capability_never_reaches_recommender() {
        for chat in [AiModelCapability::Unknown, AiModelCapability::Unsupported] {
            let fixture = fixture();
            fixture
                .service
                .upsert(AiProviderProfileUpsertRequest {
                    selected_model_id: Some("reader-model".into()),
                    ..upsert_request("gw", None)
                })
                .await
                .unwrap();
            let target = credential_target("gw").unwrap();
            fixture
                .credentials
                .set(&target, &SecretString::new("sk-test-only"))
                .await
                .unwrap();
            *fixture.catalog.models.lock().unwrap() = vec![
                AiModelDescriptor::new(
                    "reader-model".into(),
                    None,
                    None,
                    None,
                    chat,
                    AiModelCapability::Unknown,
                    AiModelCapability::Unknown,
                )
                .unwrap(),
            ];
            let agent_store = crate::services::agent_settings_ipc::tests::FakeStore::new();
            crate::services::agent_settings_ipc::tests::seed_reading(
                &agent_store,
                Some("rev-0001"),
            );
            let agent_settings = crate::services::agent_settings_ipc::tests::service(&agent_store);
            let context = agent_settings.context().await.unwrap();
            let recommender = Arc::new(RecordingRecommender::default());
            let service = AiProviderProfileService::new(
                fixture.profiles.clone(),
                fixture.credentials.clone(),
                fixture.catalog.clone(),
            )
            .with_settings_recommendation(recommender.clone())
            .with_agent_settings(agent_settings);

            let error = service
                .generate_settings_recommendation(AiSettingsRecommendationRequest {
                    profile_id: "gw".into(),
                    session_id: AgentSessionId::new().to_string(),
                    request_id: AgentRequestId::new().to_string(),
                    context_id: context.context_id().to_string(),
                    context_hash: context.context_hash().to_owned(),
                    base_revision: context.revision().map(str::to_owned),
                })
                .await
                .unwrap_err();
            assert_eq!(error.code().as_str(), "AI_PROVIDER_MODEL_UNAVAILABLE");
            assert_eq!(fixture.catalog.calls.lock().unwrap().len(), 1);
            assert!(recommender.calls.lock().unwrap().is_empty());
            assert_eq!(agent_store.target_writes(), 0);
            assert_eq!(agent_store.proposal_count(), 0);
        }
    }

    #[tokio::test]
    async fn stale_settings_context_is_rejected_before_provider_recommendation() {
        let fixture = fixture();
        fixture
            .service
            .upsert(AiProviderProfileUpsertRequest {
                selected_model_id: Some("reader-model".into()),
                ..upsert_request("gw", None)
            })
            .await
            .unwrap();
        let target = credential_target("gw").unwrap();
        fixture
            .credentials
            .set(&target, &SecretString::new("sk-test-only"))
            .await
            .unwrap();
        *fixture.catalog.models.lock().unwrap() = vec![
            AiModelDescriptor::new(
                "reader-model".into(),
                None,
                None,
                None,
                AiModelCapability::Supported,
                AiModelCapability::Unknown,
                AiModelCapability::Unknown,
            )
            .unwrap(),
        ];
        let agent_store = crate::services::agent_settings_ipc::tests::FakeStore::new();
        crate::services::agent_settings_ipc::tests::seed_reading(&agent_store, Some("rev-0001"));
        let agent_settings = crate::services::agent_settings_ipc::tests::service(&agent_store);
        let old_context = agent_settings.context().await.unwrap();
        let _ = agent_store.write_reading_for_test(haven_domain::settings::ReadingPatch {
            line_height: Some(haven_domain::settings::ReadingLineHeight::Airy),
            ..haven_domain::settings::ReadingPatch::default()
        });
        let recommender = Arc::new(RecordingRecommender::default());
        let service = AiProviderProfileService::new(
            fixture.profiles.clone(),
            fixture.credentials.clone(),
            fixture.catalog.clone(),
        )
        .with_settings_recommendation(recommender.clone())
        .with_agent_settings(agent_settings);

        let error = service
            .generate_settings_recommendation(AiSettingsRecommendationRequest {
                profile_id: "gw".into(),
                session_id: AgentSessionId::new().to_string(),
                request_id: AgentRequestId::new().to_string(),
                context_id: old_context.context_id().to_string(),
                context_hash: old_context.context_hash().to_owned(),
                base_revision: old_context.revision().map(str::to_owned),
            })
            .await
            .unwrap_err();
        assert_eq!(error.code().as_str(), "AI_PROVIDER_SETTINGS_CONTEXT_STALE");
        assert!(recommender.calls.lock().unwrap().is_empty());
        assert_eq!(agent_store.target_writes(), 0);
        assert_eq!(agent_store.proposal_count(), 0);
    }
}
