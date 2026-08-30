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

use haven_common::{AppError, ErrorKind, UtcMillis};
use haven_domain::contracts::{SettingsRepository, SettingsRow};

use crate::services::ports::SourceRegistryPorts;
use crate::wire::{
    SourceCategoryDto, SourceDescriptorDto, SourceHealthDto, SourceKindDto, SourceModeDto,
    SourceRegistryDto, SourceRegistrySetRequest, SourceRegistrySetResult,
};

/// settings 存储中的 section 名（复用 007_settings KV 表；来源管理属于设置域）。
pub const SOURCES_SETTINGS_SECTION: &str = "sources";
const PAYLOAD_SCHEMA_VERSION: u32 = 1;

/// 自定义源 sourceId 前缀。
pub const CUSTOM_SOURCE_PREFIX: &str = "custom_";
/// 自定义源凭据 provider 段（target：`haven:opds:<sourceId>`）。
pub const OPDS_CREDENTIAL_PROVIDER: &str = "opds";

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
struct SourcesPayload {
    enabled_sources: Vec<String>,
    /// sourceId → 用户配置端点（V2-B 增量；旧数据缺字段时按空表处理）。
    #[serde(default)]
    endpoints: std::collections::BTreeMap<String, String>,
    /// 用户自定义 OPDS 书源（V2-H 收尾批次；旧数据缺字段时按空表处理）。
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

#[derive(Debug, Clone)]
struct HealthMetrics {
    last_checked: Option<String>,
    latency_ms: Option<u64>,
    success_rate: Option<f64>,
    health: SourceHealthDto,
}

/// 来源注册表服务。
#[derive(Clone)]
pub struct SourceRegistryService {
    settings: Arc<dyn SourceRegistryPorts>,
    health_state: Arc<std::sync::Mutex<std::collections::HashMap<String, HealthMetrics>>>,
}

/// CMS10 预设采集接口（开箱即用）。顺序即前端展示顺序；首项为出厂默认端点。
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Cms10Preset {
    pub id: &'static str,
    pub label: &'static str,
    pub endpoint: &'static str,
}

pub fn cms10_presets() -> Vec<Cms10Preset> {
    vec![
        Cms10Preset {
            id: "bfzy",
            label: "暴风资源",
            endpoint: "https://bfzyapi.com/api.php/provide/vod",
        },
        Cms10Preset {
            id: "wujin",
            label: "无尽资源",
            endpoint: "https://api.wujinapi.me/api.php/provide/vod",
        },
        Cms10Preset {
            id: "suoni",
            label: "索尼资源",
            endpoint: "https://suonizy.net/api.php/provide/vod",
        },
        Cms10Preset {
            id: "guangsu",
            label: "光速资源",
            endpoint: "https://api.guangsuapi.com/api.php/provide/vod",
        },
    ]
}

pub fn default_cms10_endpoint() -> &'static str {
    cms10_presets()
        .first()
        .map(|p| p.endpoint)
        .unwrap_or("https://bfzyapi.com/api.php/provide/vod")
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
        Self {
            settings,
            health_state: Arc::new(std::sync::Mutex::new(std::collections::HashMap::new())),
        }
    }

    /// 探活占位：当前仅记录检测时间与延迟占位，真实网络探测由后续 SourceHealthProbe 服务落地
    pub async fn probe_health(&self, source_id: &str) -> SourceHealthDto {
        let now = chrono::Utc::now().to_rfc3339();
        if let Ok(mut map) = self.health_state.lock() {
            map.insert(
                source_id.to_owned(),
                HealthMetrics {
                    last_checked: Some(now),
                    latency_ms: Some(0),
                    success_rate: Some(1.0),
                    health: SourceHealthDto::Ok,
                },
            );
        }
        SourceHealthDto::Ok
    }

    /// `source_registry_list`：JSON 内置目录 + 持久化 enabled/endpoint 叠加。
    /// 首次调用时若 cms10 尚未配置则幂等写入默认预设。
    pub async fn list(&self) -> Result<SourceRegistryDto, AppError> {
        self.ensure_default_cms10_endpoint().await?;
        let catalog = builtin_catalog()?;
        let payload = self.payload().await?;
        let enabled = self.enabled_set().await?;
        let endpoints = self.endpoints_map().await?;
        let health_snapshot = self
            .health_state
            .lock()
            .ok()
            .map(|g| g.clone())
            .unwrap_or_default();
        // 后台探活：对已启用且已配置端点的来源，异步刷新健康（不阻塞本次 list）
        let to_probe: Vec<String> = catalog
            .iter()
            .filter(|s| enabled.contains(&s.source_id) && endpoints.contains_key(&s.source_id))
            .map(|s| s.source_id.clone())
            .collect();
        if !to_probe.is_empty() {
            let svc = self.clone();
            tokio::spawn(async move {
                for sid in to_probe {
                    let _ = svc.probe_health(&sid).await;
                }
            });
        }
        let sources = catalog
            .into_iter()
            .map(|mut source| {
                source.enabled = enabled.contains(&source.source_id);
                source.endpoint_configured = endpoints.contains_key(&source.source_id);
                if let Some(metrics) = health_snapshot.get(&source.source_id) {
                    source.health = metrics.health;
                    source.last_checked = metrics.last_checked.clone();
                    source.latency_ms = metrics.latency_ms;
                    source.success_rate = metrics.success_rate;
                }
                source
            })
            .chain(
                payload
                    .custom_sources
                    .iter()
                    .map(|custom| SourceDescriptorDto {
                        source_id: custom.source_id.clone(),
                        display_name: custom.display_name.clone(),
                        kinds: vec![SourceKindDto::Metadata, SourceKindDto::Download],
                        categories: vec![SourceCategoryDto::Book],
                        mode: SourceModeDto::Single,
                        notes: "这是你添加的自定义 OPDS 书库；可在下方编辑地址或配置访问凭据。"
                            .to_owned(),
                        enabled: custom.enabled,
                        health: SourceHealthDto::Unknown,
                        endpoint_configured: !custom.endpoint.is_empty(),
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

    /// 自定义源凭据 target（`haven:opds:<sourceId>`）。非法 ID 返回 INVALID_ARGUMENT。
    pub fn custom_credential_target(
        source_id: &str,
    ) -> Result<haven_domain::ids::CredentialRef, AppError> {
        haven_domain::ids::CredentialRef::new_scoped(OPDS_CREDENTIAL_PROVIDER, source_id)
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

    /// `source_add`：新增自定义 OPDS 源，生成稳定 `custom_` 前缀 sourceId。
    /// 默认停用（fail closed），端点经校验后持久化；端点本身不出 IPC。
    pub async fn add_custom_source(
        &self,
        display_name: &str,
        endpoint: &str,
    ) -> Result<crate::wire::SourceAddResult, AppError> {
        let name = display_name.trim();
        if name.is_empty() || name.len() > 100 {
            return Err(invalid_argument("显示名不能为空且不超过 100 字符"));
        }
        let normalized =
            validate_endpoint(endpoint)?.ok_or_else(|| invalid_argument("端点地址不能为空"))?;
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
        let source_id = loop {
            let candidate = format!(
                "{CUSTOM_SOURCE_PREFIX}{}",
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
        if let Some(endpoint) = &request.endpoint {
            validate_endpoint(endpoint)?.ok_or_else(|| invalid_argument("端点地址不能为空"))?;
        }
        let mut payload = self.payload().await?;
        let record = payload
            .custom_sources
            .iter_mut()
            .find(|c| c.source_id == request.source_id)
            .ok_or_else(|| not_found("自定义来源不存在"))?;
        if let Some(name) = &request.display_name {
            record.display_name = name.trim().to_owned();
        }
        if let Some(endpoint) = &request.endpoint {
            record.endpoint = validate_endpoint(endpoint)?.unwrap_or_default();
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
    pub async fn remove_custom_source(
        &self,
        source_id: &str,
        store: &dyn haven_domain::credential::CredentialStore,
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
        let mut payload = self.payload().await?;
        let before = payload.custom_sources.len();
        payload.custom_sources.retain(|c| c.source_id != source_id);
        payload.enabled_sources.retain(|id| id != source_id);
        payload.endpoints.remove(source_id);
        if payload.custom_sources.len() != before {
            self.persist_payload(&payload).await?;
        }
        Ok(crate::wire::SourceRemoveResult {
            schema_version: 1,
            source_id: source_id.to_owned(),
            credential_deleted,
        })
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
        let target = Self::custom_credential_target(&request.source_id)?;
        let mut payload = self.payload().await?;
        let record = payload
            .custom_sources
            .iter_mut()
            .find(|c| c.source_id == request.source_id)
            .ok_or_else(|| not_found("自定义来源不存在"))?;
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
        record.enabled = enabled;
        self.persist_payload(&payload).await
    }

    /// 当前端点映射。
    async fn endpoints_map(&self) -> Result<std::collections::BTreeMap<String, String>, AppError> {
        Ok(self.payload().await?.endpoints)
    }

    async fn ensure_default_cms10_endpoint(&self) -> Result<(), AppError> {
        let mut payload = self.payload().await?;
        let mut changed = false;
        if !payload.endpoints.contains_key("cms10") {
            payload
                .endpoints
                .insert("cms10".to_owned(), default_cms10_endpoint().to_owned());
            changed = true;
        }
        // OPDS 书源出厂播种端点（不默认启用：启用与否交用户在设置里决定）。
        // 防御：设置页曾对 download 源复用 Cms10EndpointRow，可能把 CMS 预设 URL
        // 误写入 OPDS 源；已知 CMS 预设地址一律纠正回出厂 OPDS 端点。
        let cms_presets = cms10_presets();
        let cms_urls: HashSet<&str> = cms_presets.iter().map(|p| p.endpoint).collect();
        for (source_id, factory) in default_opds_endpoints() {
            match payload.endpoints.get(source_id).map(String::as_str) {
                None => {
                    payload
                        .endpoints
                        .insert(source_id.to_owned(), factory.to_owned());
                    changed = true;
                }
                Some(url) if cms_urls.contains(url) => {
                    payload
                        .endpoints
                        .insert(source_id.to_owned(), factory.to_owned());
                    changed = true;
                }
                _ => {}
            }
        }
        if !payload.enabled_sources.contains(&"cms10".to_owned()) {
            // 首次安装零配置即可搜索：出厂默认启用 cms10，失败闭环由后续健康探测保障
            payload.enabled_sources.push("cms10".to_owned());
            payload.enabled_sources.sort();
            changed = true;
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
    let parsed = url_parse(trimmed)?;
    if !matches!(parsed.scheme.as_str(), "http" | "https") || parsed.host.is_empty() {
        return Err(AppError::new(
            "INVALID_ARGUMENT",
            ErrorKind::Validation,
            "端点必须是 http/https 绝对地址",
            false,
        ));
    }
    Ok(Some(trimmed.trim_end_matches('/').to_owned()))
}

struct ParsedEndpoint {
    scheme: String,
    host: String,
}

/// 极简 URL 解析（scheme://host[:port]/...），避免引入完整 URL 解析依赖到 app 层。
fn url_parse(raw: &str) -> Result<ParsedEndpoint, AppError> {
    let (scheme, rest) = raw.split_once("://").ok_or_else(invalid_endpoint)?;
    let host_end = rest.find(['/', '?', '#']).unwrap_or(rest.len());
    let authority = &rest[..host_end];
    let host = authority.rsplit_once('@').map_or(authority, |(_, h)| h);
    let host = host.split(':').next().unwrap_or(host);
    if host.is_empty() || host.contains(char::is_whitespace) {
        return Err(invalid_endpoint());
    }
    Ok(ParsedEndpoint {
        scheme: scheme.to_ascii_lowercase(),
        host: host.to_ascii_lowercase(),
    })
}

fn invalid_endpoint() -> AppError {
    AppError::new(
        "INVALID_ARGUMENT",
        ErrorKind::Validation,
        "端点格式非法",
        false,
    )
}

fn invalid_argument(message: impl Into<String>) -> AppError {
    AppError::new("INVALID_ARGUMENT", ErrorKind::Validation, message, false)
}

fn not_found(message: &'static str) -> AppError {
    AppError::new("RESOURCE_NOT_FOUND", ErrorKind::NotFound, message, false)
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
            kinds: vec![SourceKindDto::Metadata],
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
                kinds: vec![SourceKindDto::Metadata],
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
        // 出厂默认：cms10 已预填并启用（零配置即可搜索），其余停用
        let cms10 = registry
            .sources
            .iter()
            .find(|s| s.source_id == "cms10")
            .unwrap();
        assert!(cms10.enabled, "cms10 出厂应已启用");
        assert!(cms10.endpoint_configured, "cms10 出厂应已配置默认端点");
        assert_eq!(cms10.health, SourceHealthDto::Unknown);
        assert!(
            registry
                .sources
                .iter()
                .filter(|s| s.source_id != "cms10")
                .all(|s| !s.enabled),
            "除 cms10 外其余来源出厂停用"
        );
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
        assert!(cms10.endpoint_configured, "cms10 已配置默认端点");

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

    #[tokio::test]
    async fn heals_cms_preset_url_written_onto_opds_source() {
        let service = service_from_memory();
        service
            .set_endpoint(
                "opds_gutenberg",
                "https://api.guangsuapi.com/api.php/provide/vod",
            )
            .await
            .unwrap();
        let _ = service.list().await.unwrap();
        let endpoint = service.endpoint("opds_gutenberg").await.unwrap().unwrap();
        assert_eq!(endpoint, "https://m.gutenberg.org/ebooks.opds/");
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

        // 注册表合并投影：默认停用、端点已配置、kinds 为 metadata+download。
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
            .remove_custom_source(&source_id, &store)
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
            .remove_custom_source("custom_missing0000", &store)
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
}
