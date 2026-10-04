//! SourceRegistryService：来源注册表（契约 §36.2 / CONTRACT-V02-SOURCE-REGISTRY-001）。
//!
//! 规则：
//! - 内置目录由 `resources/builtin-sources.json` 静态定义
//!   （sourceId/displayName/categories/mode/kinds/notes）；前端不得自造或猜测 sourceId。
//! - 清单必须为四个内容分类各包含至少 3 个已注册搜索 Provider；每个条目都必须
//!   在 Composition Root 绑定真实搜索参与者。需要用户端点的流/下载来源只能显示
//!   “待配置”并在配置后搜索，绝不能以“目录登记/Provider 待接入”混入内置清单。
//! - `enabled` 持久化于 SQLite settings 存储（section=`sources`），重启后保留；
//!   禁止写入 localStorage 或其他前端状态。
//! - `health` 与 `endpointConfigured` 的运行时事实属 V2-B（探测与端点配置）；
//!   在此之前诚实返回 `unknown` / `false`，不伪造健康状态。
//! - 未知 `sourceId` → `INVALID_ARGUMENT`；重复设置同值幂等，不产生新 revision。

use std::collections::HashSet;
use std::sync::{Arc, OnceLock};

use haven_common::network::{HttpUrlPolicy, parse_http_url};
use haven_common::{AppError, ErrorKind, UtcMillis};
use haven_domain::contracts::{SettingsRepository, SettingsRow};

use crate::services::ports::SourceRegistryPorts;
use crate::services::source_config_cache::SourceConfigCache;
use crate::services::tvbox_config_preview::MAX_CONFIG_URL_CHARS;
use crate::wire::{
    SourceCategoryDto, SourceDescriptorDto, SourceHealthDto, SourceKindDto, SourceModeDto,
    SourceRegistryDto, SourceRegistrySetRequest, SourceRegistrySetResult,
};

/// settings 存储中的 section 名（复用 007_settings KV 表；来源管理属于设置域）。
pub const SOURCES_SETTINGS_SECTION: &str = "sources";
/// schemaVersion 2：`customSources[].kind`（RSS/Atom 订阅源）为向前兼容的新增字段。
const PAYLOAD_SCHEMA_VERSION: u32 = 2;

/// 自定义源 sourceId 前缀。
pub const CUSTOM_SOURCE_PREFIX: &str = "custom_";
/// 用户登记的 RSS/Atom 订阅源 sourceId 前缀（`custom_` 家族的子前缀）。
///
/// 订阅源与自定义 OPDS 书源共用同一份持久化与启用语义，但身份前缀更长，
/// 这样前缀路由（`resolve_participant` 取最长前缀）能把 Feed 家族交给
/// Feed 参与者，而不会落到 OPDS 参与者上。
pub const CUSTOM_FEED_SOURCE_PREFIX: &str = "custom_feed_";
/// 用户登记的自托管 Komga 漫画库 sourceId 前缀（`custom_` 家族的子前缀）。
pub const CUSTOM_KOMGA_SOURCE_PREFIX: &str = "custom_komga_";
/// 用户登记的自托管 Kavita 漫画库 sourceId 前缀（`custom_` 家族的子前缀）。
pub const CUSTOM_KAVITA_SOURCE_PREFIX: &str = "custom_kavita_";
/// 用户登记的 TVBox / FongMi 配置 sourceId 前缀（`custom_` 家族的子前缀）。
///
/// 与 `custom_komga_` / `custom_kavita_` 同理，前缀比 `custom_` 更长，最长前缀路由
/// 不会把它交给自定义 OPDS 参与者。本切片**没有**为它注册任何搜索/读取参与者：
/// TVBox 来源只能登记，不能启用（见 `set_custom_source_enabled`），因此不存在
/// 「启用后被错误路由到 OPDS 参与者」的路径。
pub const CUSTOM_TVBOX_SOURCE_PREFIX: &str = "custom_tvbox_";
/// 自定义源凭据 provider 段（target：`haven:opds:<sourceId>`）。
pub const OPDS_CREDENTIAL_PROVIDER: &str = "opds";
/// Komga API key 凭据 provider 段（target：`haven:komga:<sourceId>`）。
pub const KOMGA_CREDENTIAL_PROVIDER: &str = "komga";
/// Kavita API key 凭据 provider 段（target：`haven:kavita:<sourceId>`）。
pub const KAVITA_CREDENTIAL_PROVIDER: &str = "kavita";

/// 自定义源的能力种类。旧数据缺字段时按 `Opds` 处理，保持既有行为不变。
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum CustomSourceKind {
    /// 自定义 OPDS 书库（图书；搜索 + 受控 EPUB 正文）。
    #[default]
    Opds,
    /// 用户登记的 RSS 2.0 / Atom 订阅源（报刊文章）。
    Feed,
    /// 用户登记的自托管 Komga 漫画库（漫画；搜索 + 章节在线逐页 + CBZ 下载）。
    Komga,
    /// 用户登记的自托管 Kavita 漫画库（漫画；搜索 + 章节在线逐页 + CBZ 下载）。
    Kavita,
    /// 用户登记的 TVBox / FongMi 配置（影视；本切片只登记身份与端点并缓存原文，
    /// 没有任何已实现的搜索、播放或刷新路径）。
    Tvbox,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
struct SourcesPayload {
    enabled_sources: Vec<String>,
    /// sourceId → 用户配置端点（V2-B 增量；旧数据缺字段时按空表处理）。
    #[serde(default)]
    endpoints: std::collections::BTreeMap<String, String>,
    /// 用户自定义来源（V2-H 收尾批次；旧数据缺字段时按空表处理）。
    #[serde(default)]
    custom_sources: Vec<CustomSourceRecord>,
}

/// 自定义源持久化记录。endpoint 属后端事实，禁止出 IPC；
/// credential_ref 只保存 target 字符串（secret 在系统 keyring）。
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CustomSourceRecord {
    pub source_id: String,
    pub display_name: String,
    pub endpoint: String,
    pub enabled: bool,
    /// `haven:opds:<sourceId>`；无凭据为 None。
    pub credential_ref: Option<String>,
    /// 来源种类；旧记录缺字段时按 OPDS 处理。
    #[serde(default)]
    pub kind: CustomSourceKind,
}

/// 内置来源清单的编译期资源。JSON 只保存受信任的展示元数据，
/// endpoint 与 credential 仍由后端设置/凭据存储管理，绝不进入 Wire。
const BUILTIN_SOURCES_JSON: &str = include_str!("../../resources/builtin-sources.json");
const BUILTIN_MANIFEST_SCHEMA_VERSION: u32 = 1;
const MIN_SOURCES_PER_CATEGORY: usize = 3;

#[derive(Debug, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
#[serde(deny_unknown_fields)]
struct BuiltinSourcesManifest {
    schema_version: u32,
    categories: Vec<BuiltinSourceCategory>,
    sources: Vec<BuiltinSource>,
}

#[derive(Debug, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
#[serde(deny_unknown_fields)]
struct BuiltinSourceCategory {
    id: SourceCategoryDto,
    label: String,
    description: String,
}

#[derive(Debug, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
#[serde(deny_unknown_fields)]
struct BuiltinSource {
    source_id: String,
    display_name: String,
    categories: Vec<SourceCategoryDto>,
    mode: SourceModeDto,
    kinds: Vec<SourceKindDto>,
    notes: String,
}

static BUILTIN_CATALOG: OnceLock<Result<Vec<SourceDescriptorDto>, String>> = OnceLock::new();

/// 从受信任 JSON 清单生成来源描述。解析/校验失败时 fail closed，
/// 不回退到第二份硬编码目录，避免出现两套来源事实源。
fn builtin_catalog() -> Result<Vec<SourceDescriptorDto>, AppError> {
    BUILTIN_CATALOG
        .get_or_init(|| {
            let manifest: BuiltinSourcesManifest =
                serde_json::from_str(BUILTIN_SOURCES_JSON).map_err(|err| err.to_string())?;
            validate_builtin_manifest(&manifest)?;
            Ok(manifest
                .sources
                .into_iter()
                .map(|source| SourceDescriptorDto {
                    source_id: source.source_id,
                    display_name: source.display_name,
                    kinds: source.kinds,
                    categories: source.categories,
                    mode: source.mode,
                    notes: source.notes,
                    enabled: false,
                    health: SourceHealthDto::Unknown,
                    endpoint_configured: false,
                    credential_configured: false,
                    last_checked: None,
                    latency_ms: None,
                    success_rate: None,
                })
                .collect())
        })
        .clone()
        .map_err(|_| {
            AppError::new(
                "INTERNAL_ERROR",
                ErrorKind::Internal,
                "内置来源目录暂时不可用",
                false,
            )
        })
}

fn validate_builtin_manifest(manifest: &BuiltinSourcesManifest) -> Result<(), String> {
    if manifest.schema_version != BUILTIN_MANIFEST_SCHEMA_VERSION {
        return Err("内置来源清单版本不受支持".to_owned());
    }
    let expected_categories = [
        SourceCategoryDto::Video,
        SourceCategoryDto::Book,
        SourceCategoryDto::Comic,
        SourceCategoryDto::Periodical,
    ];
    if manifest.categories.len() != expected_categories.len()
        || expected_categories.iter().any(|expected| {
            !manifest
                .categories
                .iter()
                .any(|category| category.id == *expected)
        })
    {
        return Err("内置来源清单必须包含四个内容分类".to_owned());
    }
    if manifest
        .categories
        .iter()
        .any(|category| category.label.trim().is_empty() || category.description.trim().is_empty())
    {
        return Err("内置来源分类缺少显示说明".to_owned());
    }
    if manifest.sources.is_empty() {
        return Err("内置来源清单不能为空".to_owned());
    }
    let mut source_ids = HashSet::new();
    for source in &manifest.sources {
        if source.source_id.trim().is_empty()
            || !source_ids.insert(source.source_id.as_str())
            || source.display_name.trim().is_empty()
            || source.notes.trim().is_empty()
            || source.categories.is_empty()
            || source.kinds.is_empty()
        {
            return Err("内置来源清单包含重复或不完整条目".to_owned());
        }
        if source
            .categories
            .iter()
            .any(|category| !expected_categories.contains(category))
        {
            return Err("内置来源包含未知内容分类".to_owned());
        }
        if source.notes.contains("目录登记") || source.notes.contains("待接入") {
            return Err("内置来源不能以待接入或仅目录登记状态发布".to_owned());
        }
    }
    for category in expected_categories {
        let count = manifest
            .sources
            .iter()
            .filter(|source| source.categories.contains(&category))
            .count();
        if count < MIN_SOURCES_PER_CATEGORY {
            return Err(format!(
                "内置来源分类 {category:?} 至少需要 {MIN_SOURCES_PER_CATEGORY} 个来源"
            ));
        }
    }
    Ok(())
}

/// 来源注册表服务。
#[derive(Clone)]
pub struct SourceRegistryService {
    settings: Arc<dyn SourceRegistryPorts>,
}

/// OPDS 书源出厂预设端点（开箱即用；用户可在设置中覆盖）。
/// 仅保留无需额外账户且已验证可搜索的公开目录；Internet Archive 与
/// Open Library 的旧 OPDS/JSON 端点在当前网络经常 TLS 失败；不再把
/// 不稳定端点或仅能间歇访问的 Provider 放进内置目录。
pub fn default_opds_endpoints() -> Vec<(&'static str, &'static str)> {
    vec![("opds_gutenberg", "https://m.gutenberg.org/ebooks.opds/")]
}

impl SourceRegistryService {
    pub fn new(settings: Arc<dyn SourceRegistryPorts>) -> Self {
        Self { settings }
    }

    /// 健康探测尚未接入真实 Provider。
    ///
    /// 保留这个兼容入口是为了让后续独立的 SourceHealthProbe Foundation 可以
    /// 在不改变调用面语义的前提下接入；当前绝不写入“成功”指标，也不把未知
    /// 状态伪装成正常、0ms 或 100%。
    pub async fn probe_health(&self, source_id: &str) -> SourceHealthDto {
        let _ = source_id;
        SourceHealthDto::Unknown
    }

    /// `source_registry_list`：JSON 内置目录 + 持久化 enabled/endpoint 叠加。
    /// 首次调用时播种已有的内置来源；已有设置行中的启用/停用选择永不被覆盖。
    pub async fn list(&self) -> Result<SourceRegistryDto, AppError> {
        self.ensure_default_sources().await?;
        let catalog = builtin_catalog()?;
        let payload = self.payload().await?;
        let enabled = self.enabled_set().await?;
        let endpoints = self.endpoints_map().await?;
        let sources = catalog
            .into_iter()
            .map(|mut source| {
                source.enabled = enabled.contains(&source.source_id);
                source.endpoint_configured = endpoints.contains_key(&source.source_id);
                source
            })
            .chain(
                payload
                    .custom_sources
                    .iter()
                    .map(|custom| SourceDescriptorDto {
                        source_id: custom.source_id.clone(),
                        display_name: custom.display_name.clone(),
                        kinds: custom_kind_kinds(custom.kind).to_vec(),
                        categories: custom_kind_categories(custom.kind).to_vec(),
                        mode: SourceModeDto::Single,
                        notes: custom_kind_notes(custom.kind).to_owned(),
                        enabled: custom.enabled,
                        health: SourceHealthDto::Unknown,
                        endpoint_configured: !custom.endpoint.is_empty(),
                        credential_configured: custom.credential_ref.is_some(),
                        last_checked: None,
                        latency_ms: None,
                        success_rate: None,
                    }),
            )
            .collect();
        Ok(SourceRegistryDto {
            schema_version: 2,
            sources,
        })
    }

    /// 读取某来源已配置端点（仅后端内存使用；禁止出 IPC）。
    pub async fn endpoint(&self, source_id: &str) -> Result<Option<String>, AppError> {
        if let Some(url) = self.endpoints_map().await?.get(source_id).cloned() {
            return Ok(Some(url));
        }
        Ok(self.custom_source(source_id).await?.map(|c| c.endpoint))
    }

    /// `source_registry_set_endpoint`：校验 http/https 绝对 URL 后持久化。
    /// 幂等覆盖；响应只回布尔投影，端点本身不出 IPC。
    pub async fn set_endpoint(&self, source_id: &str, endpoint: &str) -> Result<bool, AppError> {
        if !builtin_catalog()?
            .iter()
            .any(|source| source.source_id == source_id)
        {
            return Err(AppError::new(
                "INVALID_ARGUMENT",
                ErrorKind::Validation,
                "来源不存在或未注册",
                false,
            ));
        }
        let normalized = validate_endpoint(endpoint)?;
        let mut payload = self.payload().await?;
        match normalized {
            Some(url) => {
                payload.endpoints.insert(source_id.to_owned(), url);
            }
            None => {
                payload.endpoints.remove(source_id);
            }
        }
        self.persist_payload(&payload).await?;
        Ok(payload.endpoints.contains_key(source_id))
    }

    /// `source_registry_set`：幂等启用/停用；未知 sourceId → INVALID_ARGUMENT。
    /// 自定义源走独立存储（V2-H 收尾批次），与内置目录共享同一开关语义。
    pub async fn set(
        &self,
        request: SourceRegistrySetRequest,
    ) -> Result<SourceRegistrySetResult, AppError> {
        if Self::is_custom_source_id(&request.source_id) {
            self.set_custom_source_enabled(&request.source_id, request.enabled)
                .await?;
            return Ok(SourceRegistrySetResult {
                source_id: request.source_id,
                enabled: request.enabled,
            });
        }
        if !builtin_catalog()?
            .iter()
            .any(|source| source.source_id == request.source_id)
        {
            return Err(AppError::new(
                "INVALID_ARGUMENT",
                ErrorKind::Validation,
                "来源不存在或未注册",
                false,
            ));
        }
        let mut enabled = self.enabled_set().await?;
        let changed = if request.enabled {
            enabled.insert(request.source_id.clone())
        } else {
            enabled.remove(&request.source_id)
        };
        if changed {
            self.persist_enabled(&enabled).await?;
        }
        Ok(SourceRegistrySetResult {
            source_id: request.source_id,
            enabled: request.enabled,
        })
    }

    /// 当前已启用集合（无记录 → 空集；全部默认停用，fail closed）。
    async fn enabled_set(&self) -> Result<HashSet<String>, AppError> {
        let payload = self.payload().await?;
        let mut set: HashSet<String> = payload.enabled_sources.into_iter().collect();
        for custom in &payload.custom_sources {
            if custom.enabled {
                set.insert(custom.source_id.clone());
            } else {
                set.remove(&custom.source_id);
            }
        }
        Ok(set)
    }

    // ---- 自定义源管理（V2-H 收尾批次） ----

    fn is_custom_source_id(source_id: &str) -> bool {
        source_id.starts_with(CUSTOM_SOURCE_PREFIX)
    }

    /// 用户登记的 RSS/Atom 订阅源身份判定（`custom_` 家族的 `custom_feed_` 子前缀）。
    ///
    /// 前缀比 `custom_` 更长，因此 `SearchSourceService` 的最长前缀路由会把
    /// 订阅源交给 Feed 参与者，而不是自定义 OPDS 参与者。
    pub fn is_feed_source_id(source_id: &str) -> bool {
        source_id.starts_with(CUSTOM_FEED_SOURCE_PREFIX)
    }

    /// 用户登记的自托管漫画库（Komga/Kavita）身份判定。
    pub fn is_comic_library_source_id(source_id: &str) -> bool {
        source_id.starts_with(CUSTOM_KOMGA_SOURCE_PREFIX)
            || source_id.starts_with(CUSTOM_KAVITA_SOURCE_PREFIX)
    }

    /// 用户登记的自托管漫画库种类（非漫画库来源返回 None）。
    pub fn comic_library_kind(source_id: &str) -> Option<CustomSourceKind> {
        if source_id.starts_with(CUSTOM_KOMGA_SOURCE_PREFIX) {
            Some(CustomSourceKind::Komga)
        } else if source_id.starts_with(CUSTOM_KAVITA_SOURCE_PREFIX) {
            Some(CustomSourceKind::Kavita)
        } else {
            None
        }
    }

    /// 用户登记的 TVBox / FongMi 配置来源身份判定。
    pub fn is_tvbox_source_id(source_id: &str) -> bool {
        source_id.starts_with(CUSTOM_TVBOX_SOURCE_PREFIX)
    }

    /// 读取某个自定义来源的种类；未知 `sourceId` 返回 `None`。
    ///
    /// 供测试与后续调用方按种类分支使用；不改变身份归属——`sourceId` 仍然只由
    /// 注册表生成与解释。
    pub async fn custom_source_kind(
        &self,
        source_id: &str,
    ) -> Result<Option<CustomSourceKind>, AppError> {
        Ok(self
            .custom_source(source_id)
            .await?
            .map(|record| record.kind))
    }

    /// 自定义源凭据 target（`haven:opds:<sourceId>`）。非法 ID 返回 INVALID_ARGUMENT。
    pub fn custom_credential_target(
        source_id: &str,
    ) -> Result<haven_domain::ids::CredentialRef, AppError> {
        Self::credential_target_for_kind(source_id, CustomSourceKind::Opds)
    }

    /// 按来源种类选择凭据 provider 段：OPDS 书源沿用 `opds`（历史兼容），
    /// Komga/Kavita 各自使用独立 provider，避免把 API key 写进 OPDS 命名空间。
    pub fn credential_target_for_kind(
        source_id: &str,
        kind: CustomSourceKind,
    ) -> Result<haven_domain::ids::CredentialRef, AppError> {
        let provider = match kind {
            CustomSourceKind::Opds => OPDS_CREDENTIAL_PROVIDER,
            CustomSourceKind::Komga => KOMGA_CREDENTIAL_PROVIDER,
            CustomSourceKind::Kavita => KAVITA_CREDENTIAL_PROVIDER,
            // 订阅源没有受控的 HTTP 认证路径；调用方必须先拒绝，而不是拿到
            // 一个永远不会被任何请求消费的凭据 target。
            CustomSourceKind::Feed => return Err(invalid_argument("订阅源不使用单独的访问凭据")),
            // TVBox 配置来源同理：本切片没有读取该凭据的消费者。
            CustomSourceKind::Tvbox => {
                return Err(invalid_argument("TVBox 配置来源不使用单独的访问凭据"));
            }
        };
        haven_domain::ids::CredentialRef::new_scoped(provider, source_id)
            .map_err(|err| invalid_argument(err.user_message()))
    }

    async fn custom_source(&self, source_id: &str) -> Result<Option<CustomSourceRecord>, AppError> {
        Ok(self
            .payload()
            .await?
            .custom_sources
            .into_iter()
            .find(|c| c.source_id == source_id))
    }

    /// `source_add`：新增自定义 OPDS 书源，生成稳定 `custom_` 前缀 sourceId。
    /// 默认停用（fail closed），端点经校验后持久化；端点本身不出 IPC。
    pub async fn add_custom_source(
        &self,
        display_name: &str,
        endpoint: &str,
    ) -> Result<crate::wire::SourceAddResult, AppError> {
        self.add_custom_source_with_kind(display_name, endpoint, CustomSourceKind::Opds)
            .await
    }

    /// 新增用户登记的 RSS 2.0 / Atom 订阅源，生成稳定 `custom_feed_` 前缀 sourceId。
    ///
    /// 与自定义 OPDS 书源共用同一份持久化、启用与删除语义，但：
    /// - 端点必须是 HTTPS，且**不接受 query 串**（当前没有受控的 typed credential
    ///   路径承载 Feed query 凭据，带令牌的地址一律 fail closed）；
    /// - 仍然拒绝 userinfo、fragment、私网/回环/单标签主机与非白名单端口
    ///   （由 `HttpUrlPolicy::SourceEndpoint` 统一保证）；
    /// - 端点只留在后端，绝不出 IPC、日志或错误文案。
    ///
    /// 默认停用（fail closed）。
    pub async fn add_feed_source(
        &self,
        display_name: &str,
        endpoint: &str,
    ) -> Result<crate::wire::SourceAddResult, AppError> {
        self.add_custom_source_with_kind(display_name, endpoint, CustomSourceKind::Feed)
            .await
    }

    /// 新增用户登记的自托管漫画库（Komga `kind = Komga` / Kavita `kind = Kavita`），
    /// 生成稳定 `custom_komga_` / `custom_kavita_` 前缀 sourceId。
    ///
    /// 与 OPDS/订阅源共用同一份持久化、启用与删除语义，但：
    /// - 端点是漫画库的 API 根地址，必须是 HTTPS 且**不接受 query 串**：API key
    ///   只经系统凭据库注入请求头，任何形式的地址内 token 都 fail closed；
    /// - 仍然拒绝 userinfo、fragment、私网/回环/单标签主机与非白名单端口
    ///   （由 `HttpUrlPolicy::SourceEndpoint` 统一保证）；
    /// - 端点只留在后端，绝不出 IPC、日志或错误文案。
    ///
    /// 默认停用（fail closed）。
    pub async fn add_comic_library_source(
        &self,
        display_name: &str,
        endpoint: &str,
        kind: CustomSourceKind,
    ) -> Result<crate::wire::SourceAddResult, AppError> {
        if !matches!(kind, CustomSourceKind::Komga | CustomSourceKind::Kavita) {
            return Err(invalid_argument("该种类不是自托管漫画库"));
        }
        self.add_custom_source_with_kind(display_name, endpoint, kind)
            .await
    }

    /// `tvbox_config_save` 的来源登记入口：新增用户登记的 TVBox / FongMi 配置来源，
    /// 生成稳定 `custom_tvbox_` 前缀 sourceId。
    ///
    /// 与其它自定义源共用同一份持久化、启用与删除语义，但：
    /// - 端点是**配置地址**，可以带 query（TVBox 配置常把访问令牌放在 query 里），
    ///   长度上限与预览用例一致，避免「预览能过、保存被拒」的落差不被解释；
    /// - 仍然拒绝 userinfo、fragment、私网/回环/单标签主机与非白名单端口
    ///   （由 `HttpUrlPolicy::SourceEndpoint` 统一保证）；
    /// - 端点只留在后端，绝不出 IPC、日志或错误文案。
    ///
    /// 默认停用（fail closed）。本切片没有为 `custom_tvbox_` 注册任何搜索/读取
    /// 参与者，因此启用入口也被显式拒绝（见 [`Self::set_custom_source_enabled`]）：
    /// 否则最长前缀路由会把它交给自定义 OPDS 参与者，对着配置文件地址发 OPDS 请求。
    pub async fn add_tvbox_source(
        &self,
        display_name: &str,
        endpoint: &str,
    ) -> Result<crate::wire::SourceAddResult, AppError> {
        self.add_custom_source_with_kind(display_name, endpoint, CustomSourceKind::Tvbox)
            .await
    }

    async fn add_custom_source_with_kind(
        &self,
        display_name: &str,
        endpoint: &str,
        kind: CustomSourceKind,
    ) -> Result<crate::wire::SourceAddResult, AppError> {
        let name = display_name.trim();
        if name.is_empty() || name.len() > 100 {
            return Err(invalid_argument("显示名不能为空且不超过 100 字符"));
        }
        let normalized = match kind {
            CustomSourceKind::Opds => {
                validate_endpoint(endpoint)?.ok_or_else(|| invalid_argument("端点地址不能为空"))?
            }
            CustomSourceKind::Feed => validate_feed_endpoint(endpoint)?,
            CustomSourceKind::Komga | CustomSourceKind::Kavita => {
                validate_comic_library_endpoint(endpoint)?
            }
            CustomSourceKind::Tvbox => validate_tvbox_endpoint(endpoint)?,
        };
        let mut payload = self.payload().await?;
        if payload.custom_sources.len() >= 20 {
            return Err(AppError::new(
                "INVALID_ARGUMENT",
                ErrorKind::Validation,
                "自定义来源数量已达上限",
                false,
            ));
        }
        if payload
            .custom_sources
            .iter()
            .any(|c| c.endpoint == normalized)
        {
            return Err(AppError::new(
                "INVALID_ARGUMENT",
                ErrorKind::Validation,
                "该端点的自定义来源已存在",
                false,
            ));
        }
        let prefix = match kind {
            CustomSourceKind::Opds => CUSTOM_SOURCE_PREFIX,
            CustomSourceKind::Feed => CUSTOM_FEED_SOURCE_PREFIX,
            CustomSourceKind::Komga => CUSTOM_KOMGA_SOURCE_PREFIX,
            CustomSourceKind::Kavita => CUSTOM_KAVITA_SOURCE_PREFIX,
            CustomSourceKind::Tvbox => CUSTOM_TVBOX_SOURCE_PREFIX,
        };
        let source_id = loop {
            let candidate = format!(
                "{prefix}{}",
                &uuid::Uuid::new_v4().simple().to_string()[..12]
            );
            if !payload
                .custom_sources
                .iter()
                .any(|c| c.source_id == candidate)
            {
                break candidate;
            }
        };
        payload.custom_sources.push(CustomSourceRecord {
            source_id: source_id.clone(),
            display_name: name.to_owned(),
            endpoint: normalized,
            enabled: false,
            credential_ref: None,
            kind,
        });
        self.persist_payload(&payload).await?;
        Ok(crate::wire::SourceAddResult {
            schema_version: 1,
            source_id,
        })
    }

    /// `source_update`：修改自定义源显示名/端点；内置源 → INVALID_ARGUMENT。
    pub async fn update_custom_source(
        &self,
        request: crate::wire::SourceUpdateRequest,
    ) -> Result<crate::wire::SourceUpdateResult, AppError> {
        if !Self::is_custom_source_id(&request.source_id) {
            return Err(invalid_argument("仅自定义来源可修改"));
        }
        if let Some(name) = &request.display_name {
            let trimmed = name.trim();
            if trimmed.is_empty() || trimmed.len() > 100 {
                return Err(invalid_argument("显示名不能为空且不超过 100 字符"));
            }
        }
        let mut payload = self.payload().await?;
        let record = payload
            .custom_sources
            .iter_mut()
            .find(|c| c.source_id == request.source_id)
            .ok_or_else(|| not_found("自定义来源不存在"))?;
        // 端点校验按记录自身的种类进行：订阅源必须保持 HTTPS，不能借更新
        // 把已登记的 Feed 端点降级成明文地址。
        if let Some(endpoint) = &request.endpoint {
            // TVBox 配置来源的端点与它的 last-known-good 原文是一对。本切片没有刷新
            // 路径，改地址而不重新取回会让缓存指向一个已经不再使用的地址——那是
            // 一条「看起来有配置、实际对不上」的假事实。因此显式拒绝改地址。
            if record.kind == CustomSourceKind::Tvbox {
                return Err(invalid_argument(
                    "TVBox 配置来源不能修改地址；请删除后重新导入",
                ));
            }
            record.endpoint = match record.kind {
                CustomSourceKind::Opds => validate_endpoint(endpoint)?
                    .ok_or_else(|| invalid_argument("端点地址不能为空"))?,
                CustomSourceKind::Feed => validate_feed_endpoint(endpoint)?,
                CustomSourceKind::Komga | CustomSourceKind::Kavita => {
                    validate_comic_library_endpoint(endpoint)?
                }
                // 上面已经拦下 Tvbox；这里保持穷尽匹配，避免将来新增种类时被静默放行。
                CustomSourceKind::Tvbox => validate_tvbox_endpoint(endpoint)?,
            };
        }
        if let Some(name) = &request.display_name {
            record.display_name = name.trim().to_owned();
        }
        self.persist_payload(&payload).await?;
        Ok(crate::wire::SourceUpdateResult {
            schema_version: 1,
            source_id: request.source_id,
        })
    }

    /// 读取自定义源当前 credential_ref（供凭据写入与 Basic Auth 使用；target 字符串禁止出 IPC）。
    pub async fn custom_credential_ref(
        &self,
        source_id: &str,
    ) -> Result<Option<haven_domain::ids::CredentialRef>, AppError> {
        let record = self
            .custom_source(source_id)
            .await?
            .ok_or_else(|| not_found("自定义来源不存在"))?;
        record
            .credential_ref
            .as_deref()
            .map(|r| r.parse())
            .transpose()
    }

    /// `source_remove`：ADR-001 删除顺序——先删系统凭据，再清持久化引用。
    /// 凭据删除失败时保持 DB 记录不变（可重试）；不存在视为幂等成功。
    ///
    /// `cache` 用来清掉该来源的 last-known-good 原始配置。它排在**凭据之后、持久化
    /// 记录之前**：
    /// - 凭据仍然第一个删（ADR-001），失败就整体不改动，可以重试；
    /// - 缓存清理失败时不写回记录，于是来源仍然完整可重试，而不会留下一个「记录没了、
    ///   原始配置还躺在库里」的孤儿；
    /// - 没有缓存的来源（其它自定义源种类）删除返回 `false`，不是错误。
    pub async fn remove_custom_source(
        &self,
        source_id: &str,
        store: &dyn haven_domain::credential::CredentialStore,
        cache: &dyn SourceConfigCache,
    ) -> Result<crate::wire::SourceRemoveResult, AppError> {
        let record = self
            .custom_source(source_id)
            .await?
            .ok_or_else(|| not_found("自定义来源不存在"))?;
        let mut credential_deleted = false;
        if let Some(ref_str) = &record.credential_ref {
            let target: haven_domain::ids::CredentialRef = ref_str
                .parse()
                .map_err(|_| invalid_argument("凭据引用非法"))?;
            credential_deleted = store.delete(&target).await?;
        }
        let _cache_removed = cache.delete(source_id).await?;
        let mut payload = self.payload().await?;
        if strip_custom_source(&mut payload, source_id) {
            self.persist_payload(&payload).await?;
        }
        Ok(crate::wire::SourceRemoveResult {
            schema_version: 1,
            source_id: source_id.to_owned(),
            credential_deleted,
        })
    }

    /// 回滚一次**刚完成、尚未写入任何凭据**的注册（保存失败时撤销 `add_*`）。
    ///
    /// 记录的 `credential_ref` 必为 `None`——`add_custom_source_with_kind` 只会写
    /// `None`——因此这里刻意不接触系统凭据库：导入 TVBox 配置不该因为凭据库在这台
    /// 机器上不可用而失败。任何**可能**带凭据的来源都必须走
    /// [`Self::remove_custom_source`]（ADR-001 删除顺序在那里）。
    ///
    /// 缓存写入失败时只撤销来源记录：[`SourceConfigCache::put`] 必须是原子写入，失败
    /// 不得留下新缓存（也不得破坏已有值）。这里不再调用缓存删除，避免缓存不可用时
    /// 连带阻断来源回滚；正常删除仍走 [`Self::remove_custom_source`] 清理两者。
    pub(crate) async fn discard_fresh_registration(&self, source_id: &str) -> Result<(), AppError> {
        let mut payload = self.payload().await?;
        if strip_custom_source(&mut payload, source_id) {
            self.persist_payload(&payload).await?;
        }
        Ok(())
    }

    /// `source_set_credential`：写/删系统 keyring 凭据并同步持久化 credential_ref。
    /// secret 在本调用栈内以可清零类型存在；清除走 ADR-001 删除顺序。
    pub async fn set_custom_source_credential(
        &self,
        request: &crate::wire::SourceSetCredentialRequest,
        store: &dyn haven_domain::credential::CredentialStore,
    ) -> Result<(), AppError> {
        if !Self::is_custom_source_id(&request.source_id) {
            return Err(invalid_argument("仅自定义来源可配置凭据"));
        }
        let mut payload = self.payload().await?;
        let record = payload
            .custom_sources
            .iter_mut()
            .find(|c| c.source_id == request.source_id)
            .ok_or_else(|| not_found("自定义来源不存在"))?;
        if matches!(
            record.kind,
            CustomSourceKind::Feed | CustomSourceKind::Tvbox
        ) {
            // 订阅源没有受控的 HTTP 认证路径：私有 Feed 的访问凭据只能作为
            // URL 的一部分由后端持有。TVBox 配置地址同理——本切片没有任何消费者会
            // 读取这个 keyring 条目，写进去只会变成一条不被消费的假能力。
            // 这里明确拒绝，而不是把凭据写进一个不会被任何请求消费的 keyring 条目。
            return Err(invalid_argument("该来源不使用单独的访问凭据"));
        }
        // 凭据 provider 段由来源种类决定：Komga/Kavita 的 API key 不能与 OPDS
        // 书库密码共用同一个命名空间。
        let target = Self::credential_target_for_kind(&request.source_id, record.kind)?;
        match &request.secret {
            None => {
                let _deleted = store.delete(&target).await?;
                record.credential_ref = None;
            }
            Some(secret) if secret.is_empty() => {
                return Err(invalid_argument("凭据内容不能为空"));
            }
            Some(secret) => {
                let wrapped = haven_domain::credential::SecretString::new(secret.clone());
                store.set(&target, &wrapped).await?;
                record.credential_ref = Some(target.as_str().to_owned());
            }
        }
        self.persist_payload(&payload).await?;
        Ok(())
    }

    /// 设置自定义源启用状态（设置页开关复用 `set` 的幂等语义）。
    ///
    /// TVBox 配置来源在本切片**只能登记，不能启用**：`custom_tvbox_` 前缀比 `custom_`
    /// 长，但没有任何参与者注册它，启用后最长前缀路由会把它交给自定义 OPDS 参与者，
    /// 对着配置文件地址发一次 OPDS 请求——那既不是用户要的，也不是任何已实现的能力。
    /// 停用仍然是幂等的成功，方便将来的参与者落地时不需要改调用方。
    pub async fn set_custom_source_enabled(
        &self,
        source_id: &str,
        enabled: bool,
    ) -> Result<(), AppError> {
        if !Self::is_custom_source_id(source_id) {
            return Err(invalid_argument("仅自定义来源可切换"));
        }
        let mut payload = self.payload().await?;
        let record = payload
            .custom_sources
            .iter_mut()
            .find(|c| c.source_id == source_id)
            .ok_or_else(|| invalid_argument("来源不存在或未注册"))?;
        if enabled && record.kind == CustomSourceKind::Tvbox {
            return Err(invalid_argument("TVBox 配置来源尚不支持启用"));
        }
        record.enabled = enabled;
        self.persist_payload(&payload).await
    }

    /// 当前端点映射。
    async fn endpoints_map(&self) -> Result<std::collections::BTreeMap<String, String>, AppError> {
        Ok(self.payload().await?.endpoints)
    }

    async fn ensure_default_sources(&self) -> Result<(), AppError> {
        // 只有 Sources 设置行不存在时才播种默认启用来源。这样用户在设置页
        // 明确停用来源后，后续 list()/重启不会再次把它自动打开。
        let first_install =
            SettingsRepository::get(self.settings.as_settings(), SOURCES_SETTINGS_SECTION)
                .await?
                .is_none();
        let mut payload = self.payload().await?;
        let mut changed = false;
        // 仅为仍然需要出厂端点的内置 OPDS 目录播种地址。CMS10 必须由用户
        // 明确填写端点，避免仓库默认指向具体的第三方采集站。
        for (source_id, factory) in default_opds_endpoints() {
            if !payload.endpoints.contains_key(source_id) {
                payload
                    .endpoints
                    .insert(source_id.to_owned(), factory.to_owned());
                changed = true;
            }
        }
        if first_install {
            // 首次安装继续启用已有的固定来源；CMS10 需要用户先配置端点，
            // 因此不加入默认启用集合。之后的显式停用选择由已存在的 settings
            // 行保留，不会被再次自动打开。
            for source_id in default_opds_endpoints()
                .into_iter()
                .map(|(id, _)| id)
                .chain(["mangadex", "arxiv", "europepmc", "wikisource"])
            {
                if !payload.enabled_sources.iter().any(|id| id == source_id) {
                    payload.enabled_sources.push(source_id.to_owned());
                    changed = true;
                }
            }
            if changed {
                payload.enabled_sources.sort();
            }
        }
        if changed {
            self.persist_payload(&payload).await?;
        }
        Ok(())
    }

    /// 读取完整持久化负载（无记录 → 空负载）。
    async fn payload(&self) -> Result<SourcesPayload, AppError> {
        let Some(row) =
            SettingsRepository::get(self.settings.as_settings(), SOURCES_SETTINGS_SECTION).await?
        else {
            return Ok(SourcesPayload {
                enabled_sources: Vec::new(),
                endpoints: Default::default(),
                custom_sources: Vec::new(),
            });
        };
        let payload: SourcesPayload = serde_json::from_str(&row.data_json).map_err(|e| {
            AppError::new(
                "DATABASE_ERROR",
                ErrorKind::Database,
                "来源启用状态数据损坏",
                true,
            )
            .with_source(e)
        })?;
        Ok(payload)
    }

    async fn persist_payload(&self, payload: &SourcesPayload) -> Result<(), AppError> {
        let row = SettingsRow {
            section: SOURCES_SETTINGS_SECTION.to_owned(),
            schema_version: PAYLOAD_SCHEMA_VERSION,
            revision: new_revision(),
            data_json: serde_json::to_string(payload)
                .map_err(|e| haven_common::validation(format!("来源状态序列化失败: {e}")))?,
            updated_at: UtcMillis::now(),
        };
        SettingsRepository::upsert(self.settings.as_settings(), &row).await
    }

    async fn persist_enabled(&self, enabled: &HashSet<String>) -> Result<(), AppError> {
        let mut payload = self.payload().await?;
        payload.enabled_sources = {
            let mut sorted: Vec<String> = enabled.iter().cloned().collect();
            sorted.sort();
            sorted
        };
        self.persist_payload(&payload).await
    }
}

/// 端点校验与规范化：仅 http/https 绝对 URL，host 非空，长度 ≤ 500；
/// 去尾部 `/`。空串表示清除配置（返回 None）。
fn validate_endpoint(raw: &str) -> Result<Option<String>, AppError> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return Ok(None);
    }
    if trimmed.len() > 500 {
        return Err(AppError::new(
            "INVALID_ARGUMENT",
            ErrorKind::Validation,
            "端点地址超长",
            false,
        ));
    }
    parse_http_url(trimmed, HttpUrlPolicy::SourceEndpoint).map_err(|_| invalid_endpoint())?;
    Ok(Some(trimmed.trim_end_matches('/').to_owned()))
}

/// TVBox / FongMi 配置地址：在通用端点策略之上只放宽**长度**，不改任何安全判定。
///
/// 与 [`validate_endpoint`] 的两点差异都是有意的：
/// - 允许带 query。TVBox 配置地址常把访问令牌放在 query 里，而本仓库没有承载它的
///   typed credential 路径；地址本身只留在后端（`list()` 不回传端点，命令层
///   `Debug` 脱敏），这与预览切片对同一类地址的处理一致。
/// - 长度上限与预览用例共用 [`MAX_CONFIG_URL_CHARS`]（按字符数），而不是通用端点的
///   500 字节，避免出现「预览能过、保存被拒」而文案又解释不清的落差。
///
/// 其余约束（拒绝非 http(s) scheme、userinfo、fragment、回环/私网/单标签主机、
/// 非白名单端口）仍由 `HttpUrlPolicy::SourceEndpoint` 统一提供。这里**不做**
/// 去掉末尾 `/` 的规范化：配置地址是一个具体资源，改写路径会让缓存对不上原文。
fn validate_tvbox_endpoint(raw: &str) -> Result<String, AppError> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return Err(invalid_argument("TVBox 配置地址不能为空"));
    }
    if trimmed.chars().count() > MAX_CONFIG_URL_CHARS {
        return Err(invalid_argument("TVBox 配置地址过长"));
    }
    parse_http_url(trimmed, HttpUrlPolicy::SourceEndpoint).map_err(|_| invalid_endpoint())?;
    Ok(trimmed.to_owned())
}

fn invalid_endpoint() -> AppError {
    AppError::new(
        "INVALID_ARGUMENT",
        ErrorKind::Validation,
        "端点格式非法",
        false,
    )
}

/// 订阅源端点：在通用端点策略之上强制 HTTPS，并拒绝 query 串。
///
/// 私有 Feed 常把访问令牌放在 URL query 中，而当前没有受控的 typed credential
/// 路径可以安全承载 Feed query 凭据：把它留在设置里会渗进日志、错误与来源身份。
/// 因此这里 fail closed 地拒绝带 query 的端点，并在用户文档中说明这一限制。
/// 其余约束仍由 `HttpUrlPolicy::SourceEndpoint` 统一提供（拒绝
/// userinfo/fragment/私网/回环/单标签主机/非白名单端口）。
fn validate_feed_endpoint(raw: &str) -> Result<String, AppError> {
    let normalized = validate_endpoint(raw)?.ok_or_else(|| invalid_argument("端点地址不能为空"))?;
    if !normalized.starts_with("https://") {
        return Err(AppError::new(
            "INVALID_ARGUMENT",
            ErrorKind::Validation,
            "订阅源地址必须使用 HTTPS",
            false,
        ));
    }
    if normalized.contains('?') {
        return Err(AppError::new(
            "INVALID_ARGUMENT",
            ErrorKind::Validation,
            "订阅源地址不能包含查询串；带访问令牌的 Feed 请在来源服务端改用不带 query 的地址",
            false,
        ));
    }
    Ok(normalized)
}

/// 自托管漫画库端点：与订阅源同样的 HTTPS + 无 query 约束。
///
/// Komga 的 API key 走 `X-API-Key` 请求头、Kavita 走 `x-api-key`，两者都不需要
/// 把凭据放进 URL。因此这里可以 fail closed 地拒绝任何带 query 的端点，避免
/// 访问令牌以地址形式留在设置、日志或来源身份里。
fn validate_comic_library_endpoint(raw: &str) -> Result<String, AppError> {
    let normalized = validate_endpoint(raw)?.ok_or_else(|| invalid_argument("端点地址不能为空"))?;
    if !normalized.starts_with("https://") {
        return Err(AppError::new(
            "INVALID_ARGUMENT",
            ErrorKind::Validation,
            "漫画库地址必须使用 HTTPS",
            false,
        ));
    }
    if normalized.contains('?') {
        return Err(AppError::new(
            "INVALID_ARGUMENT",
            ErrorKind::Validation,
            "漫画库地址不能包含查询串；API key 会通过请求头注入，不需要写进地址",
            false,
        ));
    }
    Ok(normalized)
}

/// 自定义源种类 → 能力投影（契约 §36.2 的 `kinds`）。
fn custom_kind_kinds(kind: CustomSourceKind) -> &'static [SourceKindDto] {
    match kind {
        CustomSourceKind::Opds => &[SourceKindDto::Search, SourceKindDto::OfflineDownload],
        CustomSourceKind::Feed => &[
            SourceKindDto::Search,
            SourceKindDto::OnlineRead,
            SourceKindDto::OfflineDownload,
        ],
        CustomSourceKind::Komga | CustomSourceKind::Kavita => &[
            SourceKindDto::Search,
            SourceKindDto::OnlineRead,
            SourceKindDto::OfflineDownload,
        ],
        // 本切片没有任何已实现的搜索 / 在线播放 / 离线下载路径，因此这里诚实地留空，
        // 而不是复制一份「看起来像影视源」的能力清单。能力落地时再逐项补上。
        CustomSourceKind::Tvbox => &[],
    }
}

/// 自定义源种类 → 内容分类投影。分类只用于设置页分组与来源筛选。
fn custom_kind_categories(kind: CustomSourceKind) -> &'static [SourceCategoryDto] {
    match kind {
        CustomSourceKind::Opds => &[SourceCategoryDto::Book],
        CustomSourceKind::Feed => &[SourceCategoryDto::Periodical],
        CustomSourceKind::Komga | CustomSourceKind::Kavita => &[SourceCategoryDto::Comic],
        CustomSourceKind::Tvbox => &[SourceCategoryDto::Video],
    }
}

/// 自定义源种类 → 设置页固定安全文案（不得包含端点或凭据）。
fn custom_kind_notes(kind: CustomSourceKind) -> &'static str {
    match kind {
        CustomSourceKind::Opds => "这是你添加的自定义 OPDS 书库；可在下方编辑地址或配置访问凭据。",
        CustomSourceKind::Feed => {
            "这是你添加的 RSS/Atom 订阅源；Haven 只读取该订阅源自身提供的条目内容，不会打开条目链接指向的网页。地址必须使用 HTTPS，且不能带查询串。"
        }
        CustomSourceKind::Komga => {
            "这是你添加的 Komga 漫画库；搜索、章节列表、在线逐页阅读与 CBZ 下载都只访问该地址。API key 只保存在系统凭据管理器并作为请求头发送，不会出现在地址、日志或来源身份里。"
        }
        CustomSourceKind::Kavita => {
            "这是你添加的 Kavita 漫画库；搜索、章节列表、在线逐页阅读与 CBZ 下载都只访问该地址。API key 只保存在系统凭据管理器并作为请求头发送，不会出现在地址、日志或来源身份里。"
        }
        CustomSourceKind::Tvbox => {
            "这是你导入的 TVBox / FongMi 配置。Haven 只保存这份配置的原文并在本机解析，不会下载或执行配置里声明的第三方脚本，也不会自动重新获取。该来源当前处于停用状态：本版本尚未提供影视搜索与播放能力。"
        }
    }
}

fn invalid_argument(message: impl Into<String>) -> AppError {
    AppError::new("INVALID_ARGUMENT", ErrorKind::Validation, message, false)
}

fn not_found(message: &'static str) -> AppError {
    AppError::new("RESOURCE_NOT_FOUND", ErrorKind::NotFound, message, false)
}

/// 从一个已加载的 payload 中移除某来源的全部痕迹（记录、启用位、端点）。
///
/// 返回是否真的移除了一条记录；调用方据此决定要不要写回。删除与回滚共用它，避免
/// 两条路径对「移除干净」的定义悄悄分叉。
fn strip_custom_source(payload: &mut SourcesPayload, source_id: &str) -> bool {
    let before = payload.custom_sources.len();
    payload.custom_sources.retain(|c| c.source_id != source_id);
    payload.enabled_sources.retain(|id| id != source_id);
    payload.endpoints.remove(source_id);
    payload.custom_sources.len() != before
}

/// 生成 opaque revision token（时间戳 + 纳秒，保证单调唯一性）。
fn new_revision() -> String {
    format!(
        "src-rev-{:016x}-{:x}",
        UtcMillis::now().0 as u64,
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.subsec_nanos() as u64)
            .unwrap_or(0)
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use haven_infrastructure::Db;
    use haven_infrastructure::db::repos::SqliteRepositories;

    fn service_from_memory() -> SourceRegistryService {
        let db = Arc::new(Db::open_in_memory().unwrap());
        let repos = Arc::new(SqliteRepositories::new(db));
        SourceRegistryService::new(repos)
    }

    /// 删除路径需要一个缓存端口；这些用例关心的是来源记录，缓存用一个空的内存实现。
    fn memory_cache() -> crate::services::source_config_cache::InMemorySourceConfigCache {
        crate::services::source_config_cache::InMemorySourceConfigCache::new()
    }

    #[test]
    fn builtin_manifest_has_four_categories_and_safe_descriptions() {
        let catalog = builtin_catalog().expect("编译期内置来源清单必须可加载");
        assert!(catalog.iter().all(|source| {
            !source.notes.trim().is_empty()
                && !source.categories.is_empty()
                && source.categories.iter().all(|category| {
                    matches!(
                        category,
                        SourceCategoryDto::Video
                            | SourceCategoryDto::Book
                            | SourceCategoryDto::Comic
                            | SourceCategoryDto::Periodical
                    )
                })
        }));
        let cms10 = catalog
            .iter()
            .find(|source| source.source_id == "cms10")
            .expect("内置清单必须包含 cms10");
        assert_eq!(cms10.mode, SourceModeDto::Collection);
        assert!(
            catalog
                .iter()
                .any(|source| source.mode == SourceModeDto::Single)
        );
        for category in [
            SourceCategoryDto::Video,
            SourceCategoryDto::Book,
            SourceCategoryDto::Comic,
            SourceCategoryDto::Periodical,
        ] {
            assert!(
                catalog
                    .iter()
                    .filter(|source| source.categories.contains(&category))
                    .count()
                    >= MIN_SOURCES_PER_CATEGORY,
                "每个内容分类至少需要 {MIN_SOURCES_PER_CATEGORY} 个内置来源"
            );
        }
    }

    #[test]
    fn builtin_manifest_rejects_category_with_fewer_than_three_sources() {
        let categories = vec![
            BuiltinSourceCategory {
                id: SourceCategoryDto::Video,
                label: "影视".to_owned(),
                description: "影视来源".to_owned(),
            },
            BuiltinSourceCategory {
                id: SourceCategoryDto::Book,
                label: "图书".to_owned(),
                description: "图书来源".to_owned(),
            },
            BuiltinSourceCategory {
                id: SourceCategoryDto::Comic,
                label: "漫画".to_owned(),
                description: "漫画来源".to_owned(),
            },
            BuiltinSourceCategory {
                id: SourceCategoryDto::Periodical,
                label: "报刊文章".to_owned(),
                description: "报刊文章来源".to_owned(),
            },
        ];
        let sources = [
            SourceCategoryDto::Video,
            SourceCategoryDto::Book,
            SourceCategoryDto::Comic,
            SourceCategoryDto::Periodical,
        ]
        .into_iter()
        .enumerate()
        .map(|(index, category)| BuiltinSource {
            source_id: format!("source-{index}"),
            display_name: format!("来源 {index}"),
            categories: vec![category],
            mode: SourceModeDto::Single,
            kinds: vec![SourceKindDto::Search],
            notes: "测试来源".to_owned(),
        })
        .collect();
        let manifest = BuiltinSourcesManifest {
            schema_version: BUILTIN_MANIFEST_SCHEMA_VERSION,
            categories,
            sources,
        };

        let error = validate_builtin_manifest(&manifest).expect_err("分类不足时必须 fail closed");
        assert!(error.contains("至少需要 3 个来源"));
    }

    #[test]
    fn builtin_manifest_rejects_placeholder_provider_status() {
        let manifest = BuiltinSourcesManifest {
            schema_version: BUILTIN_MANIFEST_SCHEMA_VERSION,
            categories: [
                (SourceCategoryDto::Video, "影视"),
                (SourceCategoryDto::Book, "图书"),
                (SourceCategoryDto::Comic, "漫画"),
                (SourceCategoryDto::Periodical, "报刊文章"),
            ]
            .into_iter()
            .map(|(id, label)| BuiltinSourceCategory {
                id,
                label: label.to_owned(),
                description: "来源分类".to_owned(),
            })
            .collect(),
            sources: vec![BuiltinSource {
                source_id: "placeholder".to_owned(),
                display_name: "占位来源".to_owned(),
                categories: vec![SourceCategoryDto::Video],
                mode: SourceModeDto::Single,
                kinds: vec![SourceKindDto::Search],
                notes: "目录登记、Provider 待接入".to_owned(),
            }],
        };
        let error = validate_builtin_manifest(&manifest).expect_err("占位来源必须 fail closed");
        assert!(error.contains("待接入"));
    }

    #[tokio::test]
    async fn list_defaults_to_disabled_and_unknown_health() {
        let service = service_from_memory();
        let registry = service.list().await.unwrap();
        assert_eq!(registry.schema_version, 2);
        assert!(registry.sources.len() >= 8, "内置目录必须完整");
        // 首次安装出厂默认：固定来源继续启用，CMS10 必须由用户配置端点后
        // 再主动启用；它不应被仓库指向任何具体采集站。
        let cms10 = registry
            .sources
            .iter()
            .find(|s| s.source_id == "cms10")
            .unwrap();
        assert!(!cms10.enabled, "cms10 出厂应停用");
        assert!(!cms10.endpoint_configured, "cms10 出厂不应配置默认端点");
        assert_eq!(cms10.health, SourceHealthDto::Unknown);
        let gutenberg = registry
            .sources
            .iter()
            .find(|s| s.source_id == "opds_gutenberg")
            .unwrap();
        assert!(gutenberg.enabled, "Gutenberg 出厂应已启用");
        assert!(
            gutenberg.endpoint_configured,
            "Gutenberg 出厂应已配置默认端点"
        );
        assert!(
            registry
                .sources
                .iter()
                .filter(|s| {
                    !matches!(
                        s.source_id.as_str(),
                        "cms10"
                            | "opds_gutenberg"
                            | "mangadex"
                            | "arxiv"
                            | "europepmc"
                            | "wikisource"
                    )
                })
                .all(|s| !s.enabled),
            "除固定来源外其余来源出厂停用"
        );
        for source_id in ["mangadex", "arxiv", "europepmc", "wikisource"] {
            let source = registry
                .sources
                .iter()
                .find(|s| s.source_id == source_id)
                .unwrap();
            assert!(source.enabled, "{source_id} 出厂应已启用");
            assert!(!source.endpoint_configured, "{source_id} 不应要求用户端点");
        }
    }

    #[tokio::test]
    async fn explicit_disable_is_preserved_after_relisting() {
        let service = service_from_memory();
        let _ = service.list().await.unwrap();

        for source_id in ["cms10", "opds_gutenberg"] {
            service
                .set(SourceRegistrySetRequest {
                    source_id: source_id.to_owned(),
                    enabled: false,
                })
                .await
                .unwrap();
        }

        let registry = service.list().await.unwrap();
        for source_id in ["cms10", "opds_gutenberg"] {
            let source = registry
                .sources
                .iter()
                .find(|item| item.source_id == source_id)
                .unwrap();
            assert!(
                !source.enabled,
                "用户停用 {source_id} 后不应被 list 重新打开"
            );
        }
    }

    #[tokio::test]
    async fn set_enables_then_persists_across_instances() {
        let db = Arc::new(Db::open_in_memory().unwrap());
        let repos = Arc::new(SqliteRepositories::new(db));
        let first = SourceRegistryService::new(repos.clone());
        let result = first
            .set(SourceRegistrySetRequest {
                source_id: "cms10".into(),
                enabled: true,
            })
            .await
            .unwrap();
        assert_eq!(result.source_id, "cms10");
        assert!(result.enabled);

        // 重启等价：新实例读取同一持久化状态。
        let second = SourceRegistryService::new(repos);
        let registry = second.list().await.unwrap();
        let cms10 = registry
            .sources
            .iter()
            .find(|s| s.source_id == "cms10")
            .unwrap();
        assert!(cms10.enabled);
        assert!(!cms10.endpoint_configured, "仅启用 CMS10 不应自动配置端点");

        // 幂等：重复设置同值不报错且结果同值。
        let repeat = second
            .set(SourceRegistrySetRequest {
                source_id: "cms10".into(),
                enabled: true,
            })
            .await
            .unwrap();
        assert_eq!(repeat.enabled, result.enabled);
    }

    #[tokio::test]
    async fn cms10_endpoint_is_user_configured_and_persists_across_instances() {
        let db = Arc::new(Db::open_in_memory().unwrap());
        let repos = Arc::new(SqliteRepositories::new(db));
        let first = SourceRegistryService::new(repos.clone());

        assert!(
            first
                .set_endpoint("cms10", "https://example.invalid/api.php/provide/vod")
                .await
                .unwrap()
        );
        first
            .set(SourceRegistrySetRequest {
                source_id: "cms10".into(),
                enabled: true,
            })
            .await
            .unwrap();

        let second = SourceRegistryService::new(repos);
        let registry = second.list().await.unwrap();
        let cms10 = registry
            .sources
            .iter()
            .find(|source| source.source_id == "cms10")
            .unwrap();
        assert!(cms10.enabled);
        assert!(cms10.endpoint_configured);
        assert_eq!(
            second.endpoint("cms10").await.unwrap().as_deref(),
            Some("https://example.invalid/api.php/provide/vod")
        );
    }

    #[tokio::test]
    async fn unknown_source_returns_invalid_argument() {
        let service = service_from_memory();
        let err = service
            .set(SourceRegistrySetRequest {
                source_id: "not-a-source".into(),
                enabled: true,
            })
            .await
            .unwrap_err();
        assert_eq!(err.code().as_str(), "INVALID_ARGUMENT");
    }

    // ---- 自定义源（V2-H 收尾批次） ----

    use haven_domain::credential::CredentialStore as _;
    use std::collections::HashMap;

    /// 内存 mock CredentialStore：记录 set/delete 调用，供生命周期与删除顺序断言。
    struct MemoryStore {
        entries: std::sync::Mutex<HashMap<String, String>>,
        deleted: std::sync::Mutex<Vec<String>>,
        fail_set: bool,
    }

    impl MemoryStore {
        fn new() -> Self {
            Self {
                entries: std::sync::Mutex::new(HashMap::new()),
                deleted: std::sync::Mutex::new(Vec::new()),
                fail_set: false,
            }
        }
    }

    #[async_trait::async_trait]
    impl haven_domain::credential::CredentialStore for MemoryStore {
        async fn set(
            &self,
            target: &haven_domain::ids::CredentialRef,
            secret: &haven_domain::credential::SecretString,
        ) -> Result<(), AppError> {
            if self.fail_set {
                return Err(AppError::new(
                    "CREDENTIAL_ACCESS_FAILED",
                    ErrorKind::Security,
                    "模拟平台错误",
                    true,
                ));
            }
            self.entries
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .insert(target.as_str().to_owned(), secret.expose().to_owned());
            Ok(())
        }

        async fn get(
            &self,
            target: &haven_domain::ids::CredentialRef,
        ) -> Result<Option<haven_domain::credential::SecretString>, AppError> {
            Ok(self
                .entries
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .get(target.as_str())
                .map(|v| haven_domain::credential::SecretString::new(v.clone())))
        }

        async fn delete(
            &self,
            target: &haven_domain::ids::CredentialRef,
        ) -> Result<bool, AppError> {
            let deleted = self
                .entries
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .remove(target.as_str())
                .is_some();
            self.deleted
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .push(target.as_str().to_owned());
            Ok(deleted)
        }
    }

    async fn seeded_custom_source() -> (SourceRegistryService, String) {
        let service = service_from_memory();
        let result = service
            .add_custom_source("我的书源", "https://example.invalid/opds/")
            .await
            .unwrap();
        (service, result.source_id)
    }

    #[tokio::test]
    async fn custom_source_lifecycle_and_registry_merge() {
        let (service, source_id) = seeded_custom_source().await;
        assert!(source_id.starts_with(CUSTOM_SOURCE_PREFIX));

        // 注册表合并投影：默认停用、端点已配置、kinds 为 search+offline_download。
        let registry = service.list().await.unwrap();
        let custom = registry
            .sources
            .iter()
            .find(|s| s.source_id == source_id)
            .expect("自定义源应出现在注册表");
        assert!(!custom.enabled, "自定义源出厂停用（fail closed）");
        assert!(custom.endpoint_configured);
        assert_eq!(custom.display_name, "我的书源");

        // 启用 → enabled_sources 投影包含该源。
        service
            .set_custom_source_enabled(&source_id, true)
            .await
            .unwrap();
        let registry = service.list().await.unwrap();
        assert!(
            registry
                .sources
                .iter()
                .find(|s| s.source_id == source_id)
                .unwrap()
                .enabled
        );

        // 更新显示名/端点。
        service
            .update_custom_source(crate::wire::SourceUpdateRequest {
                source_id: source_id.clone(),
                display_name: Some("新名字".into()),
                endpoint: Some("https://other.example.org/feed".into()),
            })
            .await
            .unwrap();
        let registry = service.list().await.unwrap();
        let custom = registry
            .sources
            .iter()
            .find(|s| s.source_id == source_id)
            .unwrap();
        assert_eq!(custom.display_name, "新名字");
        // 端点读取仅后端内存使用。
        assert_eq!(
            service.endpoint(&source_id).await.unwrap(),
            Some("https://other.example.org/feed".into())
        );
    }

    #[tokio::test]
    async fn custom_source_credential_write_read_delete() {
        let (service, source_id) = seeded_custom_source().await;
        let store = MemoryStore::new();

        // 写凭据：keyring 有条目 + DB 引用存在。
        service
            .set_custom_source_credential(
                &crate::wire::SourceSetCredentialRequest {
                    source_id: source_id.clone(),
                    secret: Some("s3cret".into()),
                },
                &store,
            )
            .await
            .unwrap();
        let target = SourceRegistryService::custom_credential_target(&source_id).unwrap();
        let stored = store.get(&target).await.unwrap().unwrap();
        assert_eq!(stored.expose(), "s3cret");
        assert_eq!(
            service
                .custom_credential_ref(&source_id)
                .await
                .unwrap()
                .map(|r| r.as_str().to_owned()),
            Some(target.as_str().to_owned())
        );

        // 清除凭据：先删 keyring 再清引用（ADR-001 顺序由实现保证；这里断言终态）。
        service
            .set_custom_source_credential(
                &crate::wire::SourceSetCredentialRequest {
                    source_id: source_id.clone(),
                    secret: None,
                },
                &store,
            )
            .await
            .unwrap();
        assert!(store.get(&target).await.unwrap().is_none());
        assert!(
            service
                .custom_credential_ref(&source_id)
                .await
                .unwrap()
                .is_none()
        );
    }

    #[tokio::test]
    async fn credential_failure_leaves_persisted_ref_unchanged() {
        let (service, source_id) = seeded_custom_source().await;
        let mut store = MemoryStore::new();
        store.fail_set = true;
        let err = service
            .set_custom_source_credential(
                &crate::wire::SourceSetCredentialRequest {
                    source_id: source_id.clone(),
                    secret: Some("x".into()),
                },
                &store,
            )
            .await
            .unwrap_err();
        assert_eq!(err.code().as_str(), "CREDENTIAL_ACCESS_FAILED");
        assert!(
            service
                .custom_credential_ref(&source_id)
                .await
                .unwrap()
                .is_none(),
            "keyring 写入失败时不得写入引用"
        );
    }

    #[tokio::test]
    async fn remove_deletes_credential_before_clearing_record() {
        let (service, source_id) = seeded_custom_source().await;
        let store = MemoryStore::new();
        service
            .set_custom_source_credential(
                &crate::wire::SourceSetCredentialRequest {
                    source_id: source_id.clone(),
                    secret: Some("pw".into()),
                },
                &store,
            )
            .await
            .unwrap();

        let result = service
            .remove_custom_source(&source_id, &store, &memory_cache())
            .await
            .unwrap();
        assert!(result.credential_deleted, "系统凭据实际删除");
        assert_eq!(
            store
                .deleted
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .as_slice(),
            [format!("haven:{OPDS_CREDENTIAL_PROVIDER}:{source_id}")],
            "先删系统凭据"
        );
        // 记录已从注册表消失。
        let registry = service.list().await.unwrap();
        assert!(!registry.sources.iter().any(|s| s.source_id == source_id));
    }

    #[tokio::test]
    async fn remove_unknown_returns_not_found() {
        let service = service_from_memory();
        let store = MemoryStore::new();
        let err = service
            .remove_custom_source("custom_missing0000", &store, &memory_cache())
            .await
            .unwrap_err();
        assert_eq!(err.code().as_str(), "RESOURCE_NOT_FOUND");
    }

    #[tokio::test]
    async fn builtin_sources_reject_custom_mutations() {
        let service = service_from_memory();
        let store = MemoryStore::new();
        assert!(
            service
                .set_custom_source_enabled("cms10", false)
                .await
                .is_err()
        );
        assert!(
            service
                .set_custom_source_credential(
                    &crate::wire::SourceSetCredentialRequest {
                        source_id: "opds_gutenberg".into(),
                        secret: None
                    },
                    &store
                )
                .await
                .is_err()
        );
        assert!(
            service
                .update_custom_source(crate::wire::SourceUpdateRequest {
                    source_id: "cms10".into(),
                    display_name: None,
                    endpoint: None,
                })
                .await
                .is_err()
        );
    }

    #[tokio::test]
    async fn duplicate_endpoint_rejected() {
        let service = service_from_memory();
        service
            .add_custom_source("A", "https://dup.example.org/opds")
            .await
            .unwrap();
        let err = service
            .add_custom_source("B", "https://dup.example.org/opds")
            .await
            .unwrap_err();
        assert_eq!(err.code().as_str(), "INVALID_ARGUMENT");
    }

    // ---- 用户登记的 RSS/Atom 订阅源（Task 1） ----

    #[tokio::test]
    async fn feed_source_registers_with_periodical_projection_and_stays_disabled() {
        let service = service_from_memory();
        let added = service
            .add_feed_source("我的订阅", "https://feeds.example.invalid/rss.xml")
            .await
            .unwrap();
        assert!(added.source_id.starts_with(CUSTOM_FEED_SOURCE_PREFIX));
        assert!(SourceRegistryService::is_feed_source_id(&added.source_id));

        let registry = service.list().await.unwrap();
        let feed = registry
            .sources
            .iter()
            .find(|source| source.source_id == added.source_id)
            .expect("订阅源应出现在注册表");
        assert!(!feed.enabled, "订阅源出厂停用（fail closed）");
        assert!(feed.endpoint_configured);
        assert_eq!(feed.categories, vec![SourceCategoryDto::Periodical]);
        assert_eq!(
            feed.kinds,
            vec![
                SourceKindDto::Search,
                SourceKindDto::OnlineRead,
                SourceKindDto::OfflineDownload
            ]
        );
        assert!(
            !feed.notes.contains("feeds.example.invalid"),
            "来源说明不得回显端点"
        );

        // 启用、端点读取与删除复用自定义源同一套语义。
        service
            .set_custom_source_enabled(&added.source_id, true)
            .await
            .unwrap();
        assert!(
            service
                .list()
                .await
                .unwrap()
                .sources
                .iter()
                .find(|source| source.source_id == added.source_id)
                .unwrap()
                .enabled
        );
        assert_eq!(
            service.endpoint(&added.source_id).await.unwrap().as_deref(),
            Some("https://feeds.example.invalid/rss.xml")
        );

        let store = MemoryStore::new();
        service
            .remove_custom_source(&added.source_id, &store, &memory_cache())
            .await
            .unwrap();
        assert!(
            !service
                .list()
                .await
                .unwrap()
                .sources
                .iter()
                .any(|source| source.source_id == added.source_id)
        );
    }

    #[tokio::test]
    async fn feed_source_rejects_unsafe_or_plaintext_endpoints() {
        let service = service_from_memory();
        for endpoint in [
            "http://feeds.example.invalid/rss.xml",
            "https://user:secret@feeds.example.invalid/rss.xml",
            "https://feeds.example.invalid/rss.xml#fragment",
            "https://127.0.0.1/rss.xml",
            "https://192.168.1.4/rss.xml",
            "https://feeds/rss.xml",
            "https://feeds.internal/rss.xml",
            "https://feeds.example.invalid:12345/rss.xml",
        ] {
            let error = service
                .add_feed_source("不安全", endpoint)
                .await
                .unwrap_err();
            assert_eq!(
                error.code().as_str(),
                "INVALID_ARGUMENT",
                "订阅源端点应被拒绝: {endpoint}"
            );
            assert!(
                !error.user_message().contains(endpoint),
                "错误文案不得回显端点: {endpoint}"
            );
        }
    }

    #[tokio::test]
    async fn feed_source_rejects_query_string_endpoints() {
        // 当前没有受控的 typed credential 路径承载 Feed query 凭据：带令牌的
        // 地址一律 fail closed，而不是把 token 留在设置、日志或来源身份里。
        let service = service_from_memory();
        for endpoint in [
            "https://feeds.example.invalid/rss.xml?token=secret-token",
            "https://feeds.example.invalid/rss.xml?",
            "https://feeds.example.invalid/rss.xml?a=b&c=d",
        ] {
            let error = service
                .add_feed_source("带查询串", endpoint)
                .await
                .unwrap_err();
            assert_eq!(
                error.code().as_str(),
                "INVALID_ARGUMENT",
                "带 query 的订阅源端点应被拒绝: {endpoint}"
            );
            for leaked in [endpoint, "secret-token"] {
                assert!(
                    !error.user_message().contains(leaked),
                    "错误文案不得回显端点或 query 内容: {endpoint}"
                );
            }
        }
        // 不带 query 的路径仍然可用。
        assert!(
            service
                .add_feed_source("正常订阅", "https://feeds.example.invalid/nested/rss.xml")
                .await
                .is_ok()
        );
    }

    #[tokio::test]
    async fn feed_endpoint_cannot_be_downgraded_by_update() {
        let service = service_from_memory();
        let added = service
            .add_feed_source("我的订阅", "https://feeds.example.invalid/rss.xml")
            .await
            .unwrap();
        let error = service
            .update_custom_source(crate::wire::SourceUpdateRequest {
                source_id: added.source_id.clone(),
                display_name: None,
                endpoint: Some("http://feeds.example.invalid/rss.xml".into()),
            })
            .await
            .unwrap_err();
        assert_eq!(error.code().as_str(), "INVALID_ARGUMENT");
        assert_eq!(
            service.endpoint(&added.source_id).await.unwrap().as_deref(),
            Some("https://feeds.example.invalid/rss.xml"),
            "失败的更新不得改变已登记端点"
        );
    }

    #[tokio::test]
    async fn feed_source_credential_configuration_is_rejected() {
        let service = service_from_memory();
        let store = MemoryStore::new();
        let added = service
            .add_feed_source("我的订阅", "https://feeds.example.invalid/rss.xml")
            .await
            .unwrap();
        let error = service
            .set_custom_source_credential(
                &crate::wire::SourceSetCredentialRequest {
                    source_id: added.source_id.clone(),
                    secret: Some("s3cret".into()),
                },
                &store,
            )
            .await
            .unwrap_err();
        assert_eq!(error.code().as_str(), "INVALID_ARGUMENT");
        assert!(
            store
                .entries
                .lock()
                .unwrap_or_else(|error| error.into_inner())
                .is_empty(),
            "订阅源不得写入一条不会被任何请求消费的凭据"
        );
    }

    #[tokio::test]
    async fn custom_opds_sources_keep_their_existing_behaviour() {
        let service = service_from_memory();
        let opds = service
            .add_custom_source("书源", "http://example.invalid/opds/")
            .await
            .unwrap();
        assert!(opds.source_id.starts_with(CUSTOM_SOURCE_PREFIX));
        assert!(!SourceRegistryService::is_feed_source_id(&opds.source_id));
        let registry = service.list().await.unwrap();
        let descriptor = registry
            .sources
            .iter()
            .find(|source| source.source_id == opds.source_id)
            .unwrap();
        assert_eq!(descriptor.categories, vec![SourceCategoryDto::Book]);
        assert_eq!(
            descriptor.kinds,
            vec![SourceKindDto::Search, SourceKindDto::OfflineDownload]
        );
    }

    #[test]
    fn legacy_custom_source_payload_without_kind_loads_as_opds() {
        let payload: SourcesPayload = serde_json::from_str(
            r#"{"enabledSources":["custom_legacy000001"],"endpoints":{},"customSources":[{"sourceId":"custom_legacy000001","displayName":"旧书源","endpoint":"https://old.example.invalid/opds","enabled":true,"credentialRef":null}]}"#,
        )
        .unwrap();
        assert_eq!(payload.custom_sources.len(), 1);
        assert_eq!(payload.custom_sources[0].kind, CustomSourceKind::Opds);
    }

    // ---- 自托管漫画库（Task 2） ----

    #[tokio::test]
    async fn comic_library_registers_as_comic_with_api_key_support() {
        let service = service_from_memory();
        for (kind, prefix, endpoint) in [
            (
                CustomSourceKind::Komga,
                CUSTOM_KOMGA_SOURCE_PREFIX,
                "https://komga.example.invalid",
            ),
            (
                CustomSourceKind::Kavita,
                CUSTOM_KAVITA_SOURCE_PREFIX,
                "https://kavita.example.invalid",
            ),
        ] {
            let added = service
                .add_comic_library_source("我的漫画库", endpoint, kind)
                .await
                .unwrap();
            assert!(added.source_id.starts_with(prefix));
            assert!(SourceRegistryService::is_comic_library_source_id(
                &added.source_id
            ));
            assert_eq!(
                SourceRegistryService::comic_library_kind(&added.source_id),
                Some(kind)
            );
            // 漫画库不是订阅源，也不是 `custom_` OPDS 家族里的书源。
            assert!(!SourceRegistryService::is_feed_source_id(&added.source_id));

            let registry = service.list().await.unwrap();
            let descriptor = registry
                .sources
                .iter()
                .find(|source| source.source_id == added.source_id)
                .expect("漫画库应出现在注册表");
            assert!(!descriptor.enabled, "漫画库出厂停用（fail closed）");
            assert!(descriptor.endpoint_configured);
            assert!(!descriptor.credential_configured, "尚未配置 API key");
            assert_eq!(descriptor.categories, vec![SourceCategoryDto::Comic]);
            assert_eq!(
                descriptor.kinds,
                vec![
                    SourceKindDto::Search,
                    SourceKindDto::OnlineRead,
                    SourceKindDto::OfflineDownload
                ]
            );
            assert!(!descriptor.notes.contains(endpoint), "来源说明不得回显端点");
        }
    }

    #[tokio::test]
    async fn comic_library_credential_projects_configured_without_exposing_the_secret() {
        let service = service_from_memory();
        let store = MemoryStore::new();
        let added = service
            .add_comic_library_source(
                "我的 Komga",
                "https://comics.example.invalid",
                CustomSourceKind::Komga,
            )
            .await
            .unwrap();

        service
            .set_custom_source_credential(
                &crate::wire::SourceSetCredentialRequest {
                    source_id: added.source_id.clone(),
                    secret: Some("komga-api-key".into()),
                },
                &store,
            )
            .await
            .unwrap();

        // API key 使用独立的 provider 命名空间，不与 OPDS 密码共用。
        let target = SourceRegistryService::credential_target_for_kind(
            &added.source_id,
            CustomSourceKind::Komga,
        )
        .unwrap();
        assert_eq!(
            target.as_str(),
            format!("haven:{KOMGA_CREDENTIAL_PROVIDER}:{}", added.source_id)
        );
        assert_eq!(
            store.get(&target).await.unwrap().unwrap().expose(),
            "komga-api-key"
        );

        // 注册表只投影「是否已配置」，不回显 secret 或 target。
        let registry = service.list().await.unwrap();
        let descriptor = registry
            .sources
            .iter()
            .find(|source| source.source_id == added.source_id)
            .unwrap();
        assert!(descriptor.credential_configured);
        let encoded = serde_json::to_string(descriptor).unwrap();
        for forbidden in ["komga-api-key", "haven:komga:", "credentialRef"] {
            assert!(
                !encoded.contains(forbidden),
                "来源描述符不得包含 {forbidden}"
            );
        }

        // 删除顺序仍是先删系统凭据、再清持久化引用。
        let removed = service
            .remove_custom_source(&added.source_id, &store, &memory_cache())
            .await
            .unwrap();
        assert!(removed.credential_deleted);
    }

    #[tokio::test]
    async fn comic_library_endpoints_must_be_https_without_query_strings() {
        let service = service_from_memory();
        for endpoint in [
            "http://comics.example.invalid",
            "https://comics.example.invalid?token=secret-token",
            "https://user:secret@comics.example.invalid",
            "https://comics.example.invalid#fragment",
            "https://127.0.0.1",
            "https://192.168.1.4",
            "https://nas",
            "https://comics.internal",
            "https://comics.example.invalid:12345",
        ] {
            let error = service
                .add_comic_library_source("不安全", endpoint, CustomSourceKind::Komga)
                .await
                .unwrap_err();
            assert_eq!(
                error.code().as_str(),
                "INVALID_ARGUMENT",
                "漫画库端点应被拒绝: {endpoint}"
            );
            for leaked in [endpoint, "secret-token"] {
                assert!(
                    !error.user_message().contains(leaked),
                    "错误文案不得回显端点或 query 内容: {endpoint}"
                );
            }
        }
    }

    #[tokio::test]
    async fn comic_library_endpoint_cannot_be_downgraded_by_update() {
        let service = service_from_memory();
        let added = service
            .add_comic_library_source(
                "我的 Kavita",
                "https://comics.example.invalid",
                CustomSourceKind::Kavita,
            )
            .await
            .unwrap();
        let error = service
            .update_custom_source(crate::wire::SourceUpdateRequest {
                source_id: added.source_id.clone(),
                display_name: None,
                endpoint: Some("http://comics.example.invalid".into()),
            })
            .await
            .unwrap_err();
        assert_eq!(error.code().as_str(), "INVALID_ARGUMENT");
        assert_eq!(
            service.endpoint(&added.source_id).await.unwrap().as_deref(),
            Some("https://comics.example.invalid")
        );
    }

    #[test]
    fn comic_library_credential_target_rejects_feed_kind() {
        assert!(
            SourceRegistryService::credential_target_for_kind(
                "custom_feed_0123456789ab",
                CustomSourceKind::Feed
            )
            .is_err(),
            "订阅源不得拿到一个不会被消费的凭据 target"
        );
        // 旧数据（无 kind）仍然按 OPDS 命名空间解析凭据 target。
        assert_eq!(
            SourceRegistryService::custom_credential_target("custom_0123456789ab")
                .unwrap()
                .as_str(),
            "haven:opds:custom_0123456789ab"
        );
    }

    // ---------- TVBox / FongMi 配置来源（Film/TV Provider 基础切片） ----------

    /// 登记一份 TVBox 配置来源并返回它的 sourceId。
    async fn seeded_tvbox_source(service: &SourceRegistryService) -> String {
        service
            .add_tvbox_source("我的电视源", "https://config.example.invalid/tvbox.json")
            .await
            .expect("TVBox 来源登记应成功")
            .source_id
    }

    #[tokio::test]
    async fn tvbox_source_registers_disabled_with_video_projection_and_no_capability() {
        let service = service_from_memory();
        let source_id = seeded_tvbox_source(&service).await;
        assert!(source_id.starts_with(CUSTOM_TVBOX_SOURCE_PREFIX));

        let descriptor = service
            .list()
            .await
            .unwrap()
            .sources
            .into_iter()
            .find(|source| source.source_id == source_id)
            .expect("TVBox 来源必须出现在注册表里");
        assert!(!descriptor.enabled, "默认必须停用（fail closed）");
        assert!(descriptor.endpoint_configured);
        assert_eq!(descriptor.categories, vec![SourceCategoryDto::Video]);
        assert!(
            descriptor.kinds.is_empty(),
            "本切片没有任何已实现能力，kinds 必须留空而不是伪造"
        );
        assert!(
            !descriptor.notes.contains("config.example.invalid"),
            "来源说明不得回显端点"
        );
        // 端点只在后端可见。
        assert_eq!(
            service.endpoint(&source_id).await.unwrap().as_deref(),
            Some("https://config.example.invalid/tvbox.json")
        );
        assert_eq!(
            service.custom_source_kind(&source_id).await.unwrap(),
            Some(CustomSourceKind::Tvbox)
        );
    }

    #[tokio::test]
    async fn tvbox_source_cannot_be_enabled() {
        let service = service_from_memory();
        let source_id = seeded_tvbox_source(&service).await;

        // 启用被拒：没有任何参与者注册 `custom_tvbox_`，启用后最长前缀路由会把它
        // 交给自定义 OPDS 参与者，对着配置文件地址发 OPDS 请求。
        let error = service
            .set_custom_source_enabled(&source_id, true)
            .await
            .unwrap_err();
        assert_eq!(error.code().as_str(), "INVALID_ARGUMENT");

        // `source_registry_set` 是设置页开关的入口，必须同样被拒。
        let error = service
            .set(crate::wire::SourceRegistrySetRequest {
                source_id: source_id.clone(),
                enabled: true,
            })
            .await
            .unwrap_err();
        assert_eq!(error.code().as_str(), "INVALID_ARGUMENT");

        // 停用保持幂等成功（将来接入参与者时调用方无需改动）。
        service
            .set_custom_source_enabled(&source_id, false)
            .await
            .expect("停用应幂等成功");
        let descriptor = service
            .list()
            .await
            .unwrap()
            .sources
            .into_iter()
            .find(|source| source.source_id == source_id)
            .unwrap();
        assert!(!descriptor.enabled);
    }

    #[tokio::test]
    async fn tvbox_source_rejects_credentials_and_endpoint_changes() {
        let service = service_from_memory();
        let store = MemoryStore::new();
        let source_id = seeded_tvbox_source(&service).await;

        // 没有消费者会读取这个凭据，写进去只会变成假能力。
        let error = service
            .set_custom_source_credential(
                &crate::wire::SourceSetCredentialRequest {
                    source_id: source_id.clone(),
                    secret: Some("token".into()),
                },
                &store,
            )
            .await
            .unwrap_err();
        assert_eq!(error.code().as_str(), "INVALID_ARGUMENT");
        assert!(
            SourceRegistryService::credential_target_for_kind(&source_id, CustomSourceKind::Tvbox)
                .is_err()
        );

        // 改地址而不重新取回会让缓存指向一个不再使用的地址。
        let error = service
            .update_custom_source(crate::wire::SourceUpdateRequest {
                source_id: source_id.clone(),
                display_name: None,
                endpoint: Some("https://other.example.invalid/tvbox.json".into()),
            })
            .await
            .unwrap_err();
        assert_eq!(error.code().as_str(), "INVALID_ARGUMENT");
        for leaked in ["config.example.invalid", "other.example.invalid"] {
            assert!(
                !error.user_message().contains(leaked),
                "错误文案不得回显端点"
            );
        }
        assert_eq!(
            service.endpoint(&source_id).await.unwrap().as_deref(),
            Some("https://config.example.invalid/tvbox.json")
        );

        // 改显示名仍然允许。
        service
            .update_custom_source(crate::wire::SourceUpdateRequest {
                source_id: source_id.clone(),
                display_name: Some("改名后的电视源".into()),
                endpoint: None,
            })
            .await
            .expect("改显示名不应被拒");
    }

    #[tokio::test]
    async fn removing_a_tvbox_source_cleans_its_cached_config() {
        let service = service_from_memory();
        let store = MemoryStore::new();
        let cache = memory_cache();
        let source_id = seeded_tvbox_source(&service).await;
        cache
            .put(&source_id, b"{\"sites\":[]}", UtcMillis(1_000))
            .await
            .unwrap();

        let removed = service
            .remove_custom_source(&source_id, &store, &cache)
            .await
            .expect("删除应成功");
        assert_eq!(removed.source_id, source_id);
        assert!(
            cache.get(&source_id).await.unwrap().is_none(),
            "来源删除后不得留下原始配置"
        );
        assert!(
            !service
                .list()
                .await
                .unwrap()
                .sources
                .iter()
                .any(|source| source.source_id == source_id)
        );
    }

    /// 持久化里的 `kind` 值是一份长期契约：新增种类必须能原样写回、原样读出，
    /// 而缺字段的旧记录仍然按 OPDS 处理（见 `legacy_custom_source_payload_without_kind_loads_as_opds`）。
    #[test]
    fn tvbox_kind_round_trips_through_the_persisted_payload() {
        let record = CustomSourceRecord {
            source_id: "custom_tvbox_0123456789ab".to_owned(),
            display_name: "电视源".to_owned(),
            endpoint: "https://config.example.invalid/tvbox.json".to_owned(),
            enabled: false,
            credential_ref: None,
            kind: CustomSourceKind::Tvbox,
        };
        let encoded = serde_json::to_string(&record).unwrap();
        assert!(
            encoded.contains("\"kind\":\"tvbox\""),
            "落盘值必须是 tvbox: {encoded}"
        );
        let decoded: CustomSourceRecord = serde_json::from_str(&encoded).unwrap();
        assert_eq!(decoded, record);
    }
}
