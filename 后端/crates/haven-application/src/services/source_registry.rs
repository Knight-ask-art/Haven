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
//! - **所有** `sources` 段的写入都在单一设置事务内「读 → 改 → 条件写」
//!   （见 `mutations` 子模块）：并发配置变更不会互相覆盖，条件写失败返回
//!   `REVISION_CONFLICT` 而不是乐观覆盖。首次写入（设置行尚不存在）会先播种出厂
//!   默认，因此 section 的初始状态不取决于哪一次写入先到：在第一次 `list` 之前就
//!   显式停用某个出厂启用来源，这个选择同样会落地，不会被随后的 `list` 重新打开。
//! - 自定义源凭据的生命周期（来源作用域版本槽、持久化清理队列、跨重启重试、外部 I/O
//!   独占租约）在 `credentials` / `lease` 子模块：keystore 之类的外部 IO 留在事务之外，
//!   写入前先把槽位登记进持久化队列，外部写入之后再在同一事务里复核来源身份、配置版本
//!   与自己的暂存记录；复核失败只删除**自己刚写的那一个槽位**，不存在任何「还原旧秘密」
//!   的分支。写入、清除、来源移除与清理扫描都在整个外部 I/O 区间持有同一份
//!   [`SourceCredentialLease`]，因此扫描永远不会回收一次仍然活着的写入；没有注入租约的
//!   装配面对这四条路径 **fail closed**（只读注册表照常可用）。
//! - 清理义务的推进是**有界、容量感知、公平**的：一轮扫描最多尝试一个批量，清理队列
//!   写满时过期暂存槽只退休装得下的那几条（其余继续留在暂存队列里当自己的所有者），
//!   一轮没结项的条目被轮转到队尾——任何一条义务都不会被丢掉或遗忘，队列也不会卡在
//!   一个永远重复的失败上（详见 `credentials`）。
//! - `health` / `lastChecked` / `latencyMs` / `successRate` 是**真实搜索观测**的投影
//!   （见 `health` 子模块）：空成功响应算成功，错误与超时算失败；被跳过的来源与取消
//!   的查询不写健康事实，出厂无观测时诚实返回 `unknown`。健康结果永远不改写 `enabled`。
//! - 未知 `sourceId` → `INVALID_ARGUMENT`；重复设置同值幂等，不产生新 revision。

use std::collections::HashSet;
use std::sync::{Arc, OnceLock};

use haven_common::network::{HttpUrlPolicy, origin_of, parse_http_url};
use haven_common::{AppError, ErrorKind, UtcMillis};
use haven_domain::contracts::SettingsRepository;

use crate::services::ports::SourceRegistryPorts;
use crate::services::settings::SettingsUoW;
use crate::services::tvbox_config_preview::MAX_CONFIG_URL_CHARS;
use crate::wire::{
    SourceCategoryDto, SourceDescriptorDto, SourceHealthDto, SourceKindDto, SourceModeDto,
    SourceRegistryDto, SourceRegistrySetRequest, SourceRegistrySetResult,
};

mod credentials;
mod health;
mod lease;
mod mutations;

pub use credentials::{
    MAX_CREDENTIAL_CLEANUP, MAX_STAGED_CREDENTIALS, SOURCE_CREDENTIAL_CLEANUP_BATCH,
    SOURCE_CREDENTIAL_STAGE_TTL_MS, SourceCredentialCleanupReport,
};
use credentials::{PendingCredentialCleanup, StagedSourceCredential, ref_belongs_to_source};
pub use health::{SourceHealthRecorder, SourceSearchObservation};
pub use lease::{
    InMemorySourceCredentialLease, SourceCredentialLease, SourceCredentialLeaseGuard,
    source_credential_busy, source_credential_lease_io_failed, source_credential_lease_unavailable,
};
use mutations::{SourcesMutation, decode_payload, payload_row};

/// settings 存储中的 section 名（复用 007_settings KV 表；来源管理属于设置域）。
pub const SOURCES_SETTINGS_SECTION: &str = "sources";
/// schemaVersion 4：`stagedCredentials` / `credentialCleanup` / `cacheCleanup`
/// （凭据版本槽的暂存与持久化清理义务）为向前兼容的新增字段；3 为
/// `sourceRevisions`（每个来源的配置版本，健康观测的新鲜度依据）；
/// 2 为 `customSources[].kind`（RSS/Atom 订阅源）。
const PAYLOAD_SCHEMA_VERSION: u32 = 4;

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
/// 自定义源凭据 provider 段的**种类部分**（`haven:<provider>:<profile>` 的第二段）。
///
/// 新写入的槽位把种类与来源身份拼在一起（`opds_<sha256(sourceId) 前 24 字节>`）；这个
/// 常量同时是历史稳定引用 `haven:opds:<sourceId>` 的种类段，两条形状都要能被读取与轮换。
pub const OPDS_CREDENTIAL_PROVIDER: &str = "opds";
/// Komga API key 凭据 provider 段的种类部分（历史稳定引用：`haven:komga:<sourceId>`）。
pub const KOMGA_CREDENTIAL_PROVIDER: &str = "komga";
/// Kavita API key 凭据 provider 段的种类部分（历史稳定引用：`haven:kavita:<sourceId>`）。
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

/// 来源持久化负载（enabled / endpoints / customSources / sourceRevisions /
/// stagedCredentials / credentialCleanup / cacheCleanup）。
/// 端点、凭据引用与清理队列都属后端事实，禁止出 IPC；负载里没有任何 secret。
#[derive(Clone, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
struct SourcesPayload {
    enabled_sources: Vec<String>,
    /// sourceId → 用户配置端点（V2-B 增量；旧数据缺字段时按空表处理）。
    #[serde(default)]
    endpoints: std::collections::BTreeMap<String, String>,
    /// 用户自定义来源（V2-H 收尾批次；旧数据缺字段时按空表处理）。
    #[serde(default)]
    custom_sources: Vec<CustomSourceRecord>,
    /// sourceId → 配置版本 token（健康事实的新鲜度依据）。
    ///
    /// 任何会改变该来源出站行为的配置变更（端点写入/清除、凭据写入/清除、来源新增）
    /// 都会生成新 token。搜索观测携带分发时读到的 token，写入健康时必须在同一事务里
    /// 重新比对：端点 A→B、A→B→A、删除重建都会让旧观测失效，而**其它**来源的 token
    /// 不受影响（编辑一个来源不会丢掉别人的健康历史）。
    #[serde(default)]
    source_revisions: std::collections::BTreeMap<String, String>,
    /// 已落库、尚未被采纳的凭据版本槽（外部写入**之前**登记）。
    ///
    /// 它是「一次被中断的凭据写入仍然有清理所有者」的全部依据：进程在外部写入与采纳
    /// 之间退出时，这条记录让清理扫描仍能删掉那个槽位。只存不透明引用，不存 secret。
    #[serde(default)]
    staged_credentials: Vec<StagedSourceCredential>,
    /// 待从系统凭据库删除的引用（采纳时退休的旧槽、撤销的引用、被中断的暂存槽）。
    ///
    /// 外部删除失败时条目**保留**在这里等待重试；队列有界，写满时新的变更 fail closed
    /// 而不是丢掉旧的清理义务。
    #[serde(default)]
    credential_cleanup: Vec<PendingCredentialCleanup>,
    /// 待清理 last-known-good 原始配置的来源（按其 sourceId 寻址）。
    ///
    /// 来源记录被移除后这条义务仍然存在：删除不会因为缓存端口暂时不可用而丢掉
    /// 「库里已经没有身份指向的原始配置」这一事实。
    #[serde(default)]
    cache_cleanup: Vec<String>,
}

impl std::fmt::Debug for SourcesPayload {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SourcesPayload")
            .field("enabled_source_count", &self.enabled_sources.len())
            .field("endpoint_count", &self.endpoints.len())
            .field("custom_source_count", &self.custom_sources.len())
            .field("source_revision_count", &self.source_revisions.len())
            .field("staged_credential_count", &self.staged_credentials.len())
            .field("credential_cleanup_count", &self.credential_cleanup.len())
            .field("cache_cleanup_count", &self.cache_cleanup.len())
            .finish()
    }
}

/// 自定义源持久化记录。endpoint 属后端事实，禁止出 IPC；
/// credential_ref 只保存 target 字符串（secret 在系统 keyring）。
#[derive(Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CustomSourceRecord {
    pub source_id: String,
    pub display_name: String,
    pub endpoint: String,
    pub enabled: bool,
    /// 当前凭据的 target（`haven:<provider>:<profile>`）；无凭据为 None。
    ///
    /// 新写入的 target 是每次分配一个的随机版本槽，并且**同时绑定来源种类与来源身份**
    /// （provider 段为 `<opds|komga|kavita>_<sha256(sourceId) 前 24 字节>`）；历史记录里的
    /// 稳定引用（`haven:opds:<sourceId>` 等）照常被读取，下一次写入时轮换并退休。
    pub credential_ref: Option<String>,
    /// 来源种类；旧记录缺字段时按 OPDS 处理。
    #[serde(default)]
    pub kind: CustomSourceKind,
}

impl std::fmt::Debug for CustomSourceRecord {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CustomSourceRecord")
            .field("source_id_present", &!self.source_id.is_empty())
            .field("display_name_present", &!self.display_name.is_empty())
            .field("endpoint_configured", &!self.endpoint.is_empty())
            .field("credential_configured", &self.credential_ref.is_some())
            .field("enabled", &self.enabled)
            .field("kind", &self.kind)
            .finish()
    }
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
///
/// `settings_uow` 是配置变更与健康观测**唯一**的原子写入通道：
/// - 注入时，`sources` 段的每次写入都在 BEGIN IMMEDIATE 事务内读 → 改 → 条件写，
///   健康观测也在同一类事务内先复核来源与版本；
/// - 未注入时（只读组装面、测试替身）配置变更退回既有的「读 → 覆盖写」行为，
///   健康诚实保持 `unknown` 且不做任何事——没有任何写入通道时，既不伪造成功也不伪造失败。
///
/// `credential_lease` 是外部凭据 I/O 的独占通道（见 [`SourceCredentialLease`]）：
/// 它**不**替代 `settings_uow`（一个管数据库事务，一个管 keystore 这类外部副作用），
/// 但缺少它时凭据写入/清除、来源移除与清理扫描一律 fail closed——只读入口不受影响。
#[derive(Clone)]
pub struct SourceRegistryService {
    settings: Arc<dyn SourceRegistryPorts>,
    settings_uow: Option<Arc<dyn SettingsUoW>>,
    credential_lease: Option<Arc<dyn SourceCredentialLease>>,
}

/// `search_source_start` 的分发快照：启用来源描述符 + 每个来源当前的配置版本。
///
/// 版本号随观测一起进入健康写入事务：分发之后发生的端点/凭据变更（含 A→B→A）会让
/// 这次观测作废，而不是把新端点标成健康。整个结构只留在后端，不出 IPC。
#[derive(Debug, Clone)]
pub struct EnabledSourcesSnapshot {
    pub sources: Vec<SourceDescriptorDto>,
    /// sourceId → 配置版本 token（没有条目 = 该来源从未发生过影响出站的配置变更）。
    pub revisions: std::collections::BTreeMap<String, String>,
}

/// 一次出站请求使用的**只读**已登记来源上下文快照。
///
/// 出站请求（OPDS/CMS10/订阅源/漫画库/TVBox 配置）需要同时知道四件互相牵连的事实：
/// 这个 `sourceId` 在注册表里到底是什么种类、登记端点是什么、配置版本是多少、有没有本
/// 来源作用域的凭据引用。分四次读取会让它们来自四个不同的负载快照，于是在一次异步凭据
/// 解析期间发生的端点/版本变化可能被拼成「新凭据 + 旧端点」。这里用**一次**读取把四件事
/// 固定下来（实现见 [`SourceRegistryService::registered_source_request_snapshot`]），
/// 调用方在异步区间前后各取一份并比对即可。
///
/// 边界：
/// - 这是后端内部的只读投影，**不出 IPC、不进 Wire、不是控制面**，也不改变任何来源生命
///   周期（它只读持久化负载，不写、不播种、不清理）；
/// - `endpoint_origin` 是语法层的 `scheme://host[:port]`，**不是**网络授权，也不代表端点
///   已通过当前出站策略（历史记录可能是在更早的策略下写入的，出站前仍要逐跳过策略）；
/// - `credential_configured` 只回答「本来源当前有一个归属自己的凭据引用」，**不读出**也
///   不携带任何 secret；
/// - 手写 `Debug`：端点、配置版本与来源身份都不出现在诊断输出里。
#[derive(Clone, PartialEq, Eq)]
pub struct RegisteredSourceRequestSnapshot {
    kind: CustomSourceKind,
    endpoint_origin: Option<String>,
    revision: Option<String>,
    credential_configured: bool,
}

impl RegisteredSourceRequestSnapshot {
    /// 注册表里记录的种类（唯一权威，不由 `sourceId` 拼写推断）。
    pub fn kind(&self) -> CustomSourceKind {
        self.kind
    }

    /// 登记端点的归一化 origin；没有登记端点时为 `None`。
    pub fn endpoint_origin(&self) -> Option<&str> {
        self.endpoint_origin.as_deref()
    }

    /// 该来源当前的配置版本 token（端点/凭据变更都会换新；无记录为 `None`）。
    pub fn revision(&self) -> Option<&str> {
        self.revision.as_deref()
    }

    /// 该来源当前是否配置了归属自己的凭据引用（不表示 secret 可读）。
    pub fn credential_configured(&self) -> bool {
        self.credential_configured
    }

    /// 这次请求在异步区间之后是否仍然由**同一份**权威上下文授权。
    ///
    /// 三个字段都必须一致：种类、登记端点 origin、配置版本。凭据引用被轮换/清除也会改变
    /// 配置版本，因此「解析期间凭据被换掉」同样会让旧上下文失效。调用方据此 fail closed，
    /// 绝不把新解析出来的 secret 发往旧上下文选定的端点。
    pub fn is_current_with(&self, other: &Self) -> bool {
        self.kind == other.kind
            && self.endpoint_origin == other.endpoint_origin
            && self.revision == other.revision
            && self.credential_configured == other.credential_configured
    }
}

impl std::fmt::Debug for RegisteredSourceRequestSnapshot {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // 只打印形状：端点、版本与来源身份都可能携带用户信息，不进日志。
        f.debug_struct("RegisteredSourceRequestSnapshot")
            .field("kind", &self.kind)
            .field("endpoint_configured", &self.endpoint_origin.is_some())
            .field("credential_configured", &self.credential_configured)
            .finish_non_exhaustive()
    }
}

/// OPDS 书源出厂预设端点（开箱即用；用户可在设置中覆盖）。
/// 仅保留无需额外账户且已验证可搜索的公开目录；Internet Archive 与
/// Open Library 的旧 OPDS/JSON 端点在当前网络经常 TLS 失败；不再把
/// 不稳定端点或仅能间歇访问的 Provider 放进内置目录。
pub fn default_opds_endpoints() -> Vec<(&'static str, &'static str)> {
    vec![("opds_gutenberg", "https://m.gutenberg.org/ebooks.opds/")]
}

/// 出厂默认启用的固定来源：需要用户先配置端点才能工作的 CMS10 不在其列。
fn factory_enabled_sources() -> impl Iterator<Item = &'static str> {
    default_opds_endpoints()
        .into_iter()
        .map(|(source_id, _)| source_id)
        .chain(["mangadex", "arxiv", "europepmc", "wikisource"])
}

/// 出厂端点播种（幂等）：只为仍然需要出厂端点的内置 OPDS 目录补上地址。
///
/// CMS10 必须由用户明确填写端点，避免仓库默认指向具体的第三方采集站。出厂端点同样是
/// 该来源的出站绑定：写入即产生新版本，旧观测随之作废。返回是否有实际变化。
fn seed_factory_endpoints(payload: &mut SourcesPayload) -> bool {
    let mut seeded = false;
    for (source_id, factory) in default_opds_endpoints() {
        if !payload.endpoints.contains_key(source_id) {
            payload
                .endpoints
                .insert(source_id.to_owned(), factory.to_owned());
            bump_source_revision(payload, source_id);
            seeded = true;
        }
    }
    seeded
}

/// 出厂启用播种（幂等）：补齐缺失的出厂启用来源。
///
/// 只补缺失项、绝不清空：用户显式停用过的其它来源不会因为这一份默认集合而被关掉，
/// 而已经在集合里的来源被停用后也不会被重新打开（播种只在设置行尚不存在时发生）。
fn seed_factory_enabled(payload: &mut SourcesPayload) -> bool {
    let mut seeded = false;
    for source_id in factory_enabled_sources() {
        if !payload.enabled_sources.iter().any(|id| id == source_id) {
            payload.enabled_sources.push(source_id.to_owned());
            seeded = true;
        }
    }
    if seeded {
        payload.enabled_sources.sort();
    }
    seeded
}

/// 出厂初始化：内置端点 + 首次安装的默认启用集合。幂等，返回是否有实际变化。
///
/// 这是 `sources` 段**唯一**的初始化定义，由 `mutate_sources` 在设置行尚不存在时对
/// 每一次首次写入统一施加：用户在第一次 `list` 之前就显式停用某个出厂启用来源时，
/// 这次写入同样先播种出厂默认、再应用用户的选择，因此紧随其后的 `list` 不会把它重新
/// 打开。反过来，已有设置行时永不播种，用户的选择不会被覆盖。
fn initialize_factory_defaults(payload: &mut SourcesPayload) -> bool {
    let endpoints = seed_factory_endpoints(payload);
    let enabled = seed_factory_enabled(payload);
    endpoints || enabled
}

impl SourceRegistryService {
    pub fn new(settings: Arc<dyn SourceRegistryPorts>) -> Self {
        Self {
            settings,
            settings_uow: None,
            credential_lease: None,
        }
    }

    /// 注入设置事务：配置变更（读 → 改 → 条件写）与真实搜索观测的健康事实都经它原子落地。
    ///
    /// 生产组合根必须走这个构造路径，否则来源配置变更是「读一次 → 整包覆盖写」，
    /// 并发写入会互相覆盖、来源配置版本栅栏也会被陈旧写入绕过。
    pub fn with_settings_uow(mut self, uow: Arc<dyn SettingsUoW>) -> Self {
        self.settings_uow = Some(uow);
        self
    }

    /// 注入外部凭据 I/O 的独占租约。
    ///
    /// 生产组合根**必须**注入：文件数据库装配用
    /// `haven_infrastructure::FileSourceCredentialLease`（跨进程文件锁），无盘装配
    /// 显式用 [`InMemorySourceCredentialLease`]。没有注入时凭据写入/清除、来源移除与
    /// 清理扫描 fail closed，而不是退回「本进程内大概没问题」的假保证。
    pub fn with_credential_lease(mut self, lease: Arc<dyn SourceCredentialLease>) -> Self {
        self.credential_lease = Some(lease);
        self
    }

    /// 历史命名（健康切片先落地时的方法名）：与 [`Self::with_settings_uow`] 等价。
    pub fn with_health_uow(self, uow: Arc<dyn SettingsUoW>) -> Self {
        self.with_settings_uow(uow)
    }

    /// 健康探测入口（历史兼容）。
    ///
    /// 真实健康事实来自真实搜索观测：它们由 `SearchSourceService` 写入，并投影到
    /// [`Self::list`] 的 `health`/`lastChecked`/`latencyMs`/`successRate`。这里没有来源
    /// 配置上下文，因此仍然诚实返回 `unknown`，绝不把未知伪装成正常、0ms 或 100%。
    pub async fn probe_health(&self, source_id: &str) -> SourceHealthDto {
        let _ = source_id;
        SourceHealthDto::Unknown
    }

    /// `source_registry_list`：JSON 内置目录 + 持久化 enabled/endpoint/健康叠加。
    /// 首次调用时播种已有的内置来源；已有设置行中的启用/停用选择永不被覆盖。
    /// 只读取持久化事实，不做任何探测或网络访问。
    pub async fn list(&self) -> Result<SourceRegistryDto, AppError> {
        let (mut sources, payload) = self.descriptors().await?;
        self.project_health(&mut sources, &payload).await?;
        Ok(SourceRegistryDto {
            schema_version: 2,
            sources,
        })
    }

    /// `search_source_start` 的分发快照：启用来源描述符 + 每个来源的配置版本。
    ///
    /// 与 [`Self::list`] 使用同一次读取（启用位、端点与版本号不会来自三次不同的快照），
    /// 同样不做探测。已落库的健康事实随描述符一起返回，作为调度排序的真实依据。
    pub async fn enabled_sources_snapshot(&self) -> Result<EnabledSourcesSnapshot, AppError> {
        let (mut sources, payload) = self.descriptors().await?;
        sources.retain(|source| source.enabled);
        self.project_health(&mut sources, &payload).await?;
        Ok(EnabledSourcesSnapshot {
            sources,
            revisions: payload.source_revisions,
        })
    }

    /// 内置目录 + 自定义来源的完整描述符（已合并持久化 enabled/endpoint，未叠加健康）。
    async fn descriptors(&self) -> Result<(Vec<SourceDescriptorDto>, SourcesPayload), AppError> {
        self.ensure_default_sources().await?;
        let catalog = builtin_catalog()?;
        let payload = self.payload().await?;
        let enabled = enabled_from_payload(&payload);
        let sources = catalog
            .into_iter()
            .map(|mut source| {
                source.enabled = enabled.contains(&source.source_id);
                source.endpoint_configured = payload.endpoints.contains_key(&source.source_id);
                source
            })
            .chain(payload.custom_sources.iter().map(custom_descriptor))
            .collect();
        Ok((sources, payload))
    }

    /// 读取某来源已配置端点（仅后端内存使用；禁止出 IPC）。
    pub async fn endpoint(&self, source_id: &str) -> Result<Option<String>, AppError> {
        if let Some(url) = self.endpoints_map().await?.get(source_id).cloned() {
            return Ok(Some(url));
        }
        Ok(self.custom_source(source_id).await?.map(|c| c.endpoint))
    }

    /// 由 `sourceId` 前缀恢复的**自定义**来源种类（非 `custom_` 家族返回 `None`）。
    ///
    /// 前缀是注册表生成 `sourceId` 时唯一决定种类的地方，因此这个判定与持久化记录里的
    /// `kind` 在合法数据上永远一致；调用方仍应同时比对
    /// [`RegisteredSourceRequestSnapshot::kind`]，让被伪造或手工改写的记录 fail closed。
    pub fn custom_kind_from_source_id(source_id: &str) -> Option<CustomSourceKind> {
        credentials::kind_from_source_id(source_id)
    }

    /// 一次读取的已登记来源请求上下文快照（见 [`RegisteredSourceRequestSnapshot`]）。
    ///
    /// 种类、登记端点 origin、配置版本与「是否配置了本来源凭据引用」来自**同一份**持久化
    /// 负载，因此调用方不需要（也不应该）分多次读取再拼装。未登记该 `sourceId` 时返回
    /// `None`——调用方必须 fail closed，绝不能因为「prefix 看起来像」就授予用户来源策略或
    /// 去解析凭据。内置来源没有自定义记录，同样返回 `None`。
    pub async fn registered_source_request_snapshot(
        &self,
        source_id: &str,
    ) -> Result<Option<RegisteredSourceRequestSnapshot>, AppError> {
        let payload = self.payload().await?;
        let Some(record) = payload
            .custom_sources
            .iter()
            .find(|record| record.source_id == source_id)
        else {
            return Ok(None);
        };
        // 引用归属用与读写凭据同一份判定：格式非法、串源或跨 provider 的引用不算「已配置
        // 凭据」，因此不会被当成可注入凭据的来源。
        let credential_configured = record
            .credential_ref
            .as_deref()
            .is_some_and(|reference| ref_belongs_to_source(record.kind, source_id, reference));
        Ok(Some(RegisteredSourceRequestSnapshot {
            kind: record.kind,
            endpoint_origin: origin_of(&record.endpoint),
            revision: payload.source_revisions.get(source_id).cloned(),
            credential_configured,
        }))
    }

    /// `source_registry_set_endpoint`：校验 http/https 绝对 URL 后持久化。
    /// 幂等覆盖；响应只回布尔投影，端点本身不出 IPC。
    ///
    /// 端口策略按来源选择（见 [`USER_PROVIDED_ENDPOINT_SOURCE_IDS`]）：用户提供端点的
    /// 来源保存时按任意非零端口接受，出站也读同一个策略；固定来源保持原策略。保存与
    /// 消费共用同一个判定，因此不存在「能存下来却永远连不上」的端口。
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
        let normalized = endpoint_validator(source_id)(endpoint)?;
        self.mutate_sources(|payload, _persisted| {
            let previous = payload.endpoints.get(source_id).cloned();
            match &normalized {
                Some(url) => {
                    payload.endpoints.insert(source_id.to_owned(), url.clone());
                }
                None => {
                    payload.endpoints.remove(source_id);
                }
            }
            let configured = payload.endpoints.contains_key(source_id);
            // 同值设置是幂等的，不产生新版本：健康历史只在该来源的出站地址真的变化时作废。
            if payload.endpoints.get(source_id) == previous.as_ref() {
                return Ok(SourcesMutation::Unchanged(configured));
            }
            bump_source_revision(payload, source_id);
            Ok(SourcesMutation::Changed(configured))
        })
        .await
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
        let source_id = request.source_id.clone();
        let enabling = request.enabled;
        self.mutate_sources(|payload, _persisted| {
            // 启用集合 = `enabledSources` + 自定义记录的 `enabled`（后者是权威）。
            let mut current = enabled_from_payload(payload);
            let changed = if enabling {
                current.insert(source_id.clone())
            } else {
                current.remove(&source_id)
            };
            if !changed {
                return Ok(SourcesMutation::Unchanged(()));
            }
            let mut sorted: Vec<String> = current.into_iter().collect();
            sorted.sort();
            payload.enabled_sources = sorted;
            Ok(SourcesMutation::Changed(()))
        })
        .await?;
        Ok(SourceRegistrySetResult {
            source_id: request.source_id,
            enabled: request.enabled,
        })
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

    // ---- 凭据命名空间 ----
    //
    // 这里**刻意不提供**「由 sourceId 拼出 target」的构造函数：凭据 target 现在是每次
    // 写入分配的随机版本槽（见 `credentials`），唯一权威来源是自定义源记录里的
    // `credential_ref`（读：[`Self::custom_credential_ref`]）。继续暴露一个能把 sourceId
    // 直接变成 target 的入口，只会诱使调用方去读一个**没有写入者**的地址，并让来源 ID 的
    // 长度重新变成 target 合法性的隐变量。provider 段（`haven:<provider>:<profile>` 的
    // 第二段）由 [`Self::credential_provider`] 给出种类部分，再由 `credentials` 拼上
    // 来源作用域；引用是否归属某个来源由 `credentials::ref_belongs_to_source` 统一判定。

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
    /// - 允许用户选择局域网、自托管与本机端点；仍拒绝 userinfo、fragment、链路本地
    ///   和保留地址（由 `HttpUrlPolicy::UserProvidedSource` 统一保证）；
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
    /// - 允许用户选择局域网、自托管与本机端点；仍拒绝 userinfo、fragment、链路本地
    ///   和保留地址（由 `HttpUrlPolicy::UserProvidedSource` 统一保证）；
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
    /// - 允许用户选择局域网、自托管与本机端点；仍拒绝 userinfo、fragment、链路本地
    ///   和保留地址（由 `HttpUrlPolicy::UserProvidedSource` 统一保证）；
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
        if name.is_empty() || name.chars().count() > 100 {
            return Err(invalid_argument("显示名不能为空且不超过 100 字符"));
        }
        let normalized = match kind {
            CustomSourceKind::Opds => validate_user_provided_endpoint(endpoint)?
                .ok_or_else(|| invalid_argument("端点地址不能为空"))?,
            CustomSourceKind::Feed => validate_feed_endpoint(endpoint)?,
            CustomSourceKind::Komga | CustomSourceKind::Kavita => {
                validate_comic_library_endpoint(endpoint)?
            }
            CustomSourceKind::Tvbox => validate_tvbox_endpoint(endpoint)?,
        };
        let prefix = match kind {
            CustomSourceKind::Opds => CUSTOM_SOURCE_PREFIX,
            CustomSourceKind::Feed => CUSTOM_FEED_SOURCE_PREFIX,
            CustomSourceKind::Komga => CUSTOM_KOMGA_SOURCE_PREFIX,
            CustomSourceKind::Kavita => CUSTOM_KAVITA_SOURCE_PREFIX,
            CustomSourceKind::Tvbox => CUSTOM_TVBOX_SOURCE_PREFIX,
        };
        // 数量上限、端点去重与 sourceId 唯一性都必须在事务内相对**当前**负载判定：
        // 事务外的读会把两个并发新增都放进来，或让两个新增撞上同一个 id。
        self.mutate_sources(move |payload, _persisted| {
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
            // 新来源从第 1 个配置版本开始：任何携带「无版本」的迟到观测都不会被采纳。
            bump_source_revision(payload, &source_id);
            Ok(SourcesMutation::Changed(source_id))
        })
        .await
        .map(|source_id| crate::wire::SourceAddResult {
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
            if trimmed.is_empty() || trimmed.chars().count() > 100 {
                return Err(invalid_argument("显示名不能为空且不超过 100 字符"));
            }
        }
        let source_id = request.source_id.clone();
        self.mutate_sources(|payload, _persisted| {
            // 显示名等展示字段不改变健康事实，因此不为它们作废已落库的搜索历史；
            // 端点（出站绑定）改变才作废。
            let (binding_changed, changed) = {
                let record = payload
                    .custom_sources
                    .iter_mut()
                    .find(|c| c.source_id == source_id)
                    .ok_or_else(|| not_found("自定义来源不存在"))?;
                let mut binding_changed = false;
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
                    let next = match record.kind {
                        CustomSourceKind::Opds => validate_user_provided_endpoint(endpoint)?
                            .ok_or_else(|| invalid_argument("端点地址不能为空"))?,
                        CustomSourceKind::Feed => validate_feed_endpoint(endpoint)?,
                        CustomSourceKind::Komga | CustomSourceKind::Kavita => {
                            validate_comic_library_endpoint(endpoint)?
                        }
                        // 上面已经拦下 Tvbox；这里保持穷尽匹配，避免将来新增种类时被静默放行。
                        CustomSourceKind::Tvbox => validate_tvbox_endpoint(endpoint)?,
                    };
                    binding_changed = record.endpoint != next;
                    record.endpoint = next;
                }
                let mut changed = binding_changed;
                if let Some(name) = &request.display_name {
                    let trimmed = name.trim();
                    if record.display_name != trimmed {
                        record.display_name = trimmed.to_owned();
                        changed = true;
                    }
                }
                (binding_changed, changed)
            };
            if !changed {
                // 同值更新不写库：不制造新的 revision，也不惊动任何读取方。
                return Ok(SourcesMutation::Unchanged(()));
            }
            if binding_changed {
                // 地址 A→B→A 也会各产生一个新版本：改地址后重新分发前完成的旧观测一律作废。
                bump_source_revision(payload, &source_id);
            }
            Ok(SourcesMutation::Changed(()))
        })
        .await?;
        Ok(crate::wire::SourceUpdateResult {
            schema_version: 1,
            source_id,
        })
    }

    /// 读取自定义源当前 credential_ref（供凭据写入与 Basic Auth 使用；target 字符串禁止出 IPC）。
    ///
    /// 读取前校验引用的**归属**：格式非法、串源（引用的来源作用域不是本来源）或 provider
    /// 段与来源种类不匹配（伪造、跨 provider）的引用一律以 `INVALID_ARGUMENT` 拒绝，而不是
    /// 把一个不属于这里的 target 交给出站请求路径。
    pub async fn custom_credential_ref(
        &self,
        source_id: &str,
    ) -> Result<Option<haven_domain::ids::CredentialRef>, AppError> {
        let record = self
            .custom_source(source_id)
            .await?
            .ok_or_else(|| not_found("自定义来源不存在"))?;
        let Some(reference) = record.credential_ref.as_deref() else {
            return Ok(None);
        };
        if !ref_belongs_to_source(record.kind, source_id, reference) {
            return Err(invalid_argument("来源凭据引用与来源身份或种类不匹配"));
        }
        reference
            .parse()
            .map(Some)
            .map_err(|_| invalid_argument("凭据引用非法"))
    }

    /// 回滚一次**刚完成、尚未写入任何凭据**的注册（保存失败时撤销 `add_*`）。
    ///
    /// `add_custom_source_with_kind` 只会写 `credential_ref = None`，因此这里刻意不接触
    /// 系统凭据库：导入 TVBox 配置不该因为凭据库在这台机器上不可用而失败。任何**带凭据**
    /// 的记录都必须走 [`Self::remove_custom_source`]（凭据版本槽与持久化清理义务在那里）。
    /// 这条纪律由代码强制，而不只是一句注释：记录一旦带引用，这里直接 fail closed，
    /// 绝不让一次回滚把「库里还有一条身份指向的秘密」这件事悄悄删掉。
    ///
    /// 缓存写入失败时只撤销来源记录：`SourceConfigCache::put` 必须是原子写入，失败
    /// 不得留下新缓存（也不得破坏已有值）。这里不再调用缓存删除，避免缓存不可用时
    /// 连带阻断来源回滚；正常删除仍走 [`Self::remove_custom_source`] 清理两者。
    pub(crate) async fn discard_fresh_registration(&self, source_id: &str) -> Result<(), AppError> {
        self.mutate_sources(|payload, _persisted| {
            let Some(record) = payload
                .custom_sources
                .iter()
                .find(|c| c.source_id == source_id)
            else {
                return Ok(SourcesMutation::Unchanged(()));
            };
            if record.credential_ref.is_some() {
                return Err(AppError::new(
                    "SOURCE_CREDENTIAL_CLEANUP_REQUIRED",
                    ErrorKind::Conflict,
                    "该来源已经带凭据引用，必须走来源移除路径清理",
                    false,
                ));
            }
            strip_custom_source(payload, source_id);
            Ok(SourcesMutation::Changed(()))
        })
        .await
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
        self.mutate_sources(|payload, _persisted| {
            let record = payload
                .custom_sources
                .iter_mut()
                .find(|c| c.source_id == source_id)
                .ok_or_else(|| invalid_argument("来源不存在或未注册"))?;
            if enabled && record.kind == CustomSourceKind::Tvbox {
                return Err(invalid_argument("TVBox 配置来源尚不支持启用"));
            }
            if record.enabled == enabled {
                // 同值设置幂等：不写库，也不产生新 revision。
                return Ok(SourcesMutation::Unchanged(()));
            }
            record.enabled = enabled;
            Ok(SourcesMutation::Changed(()))
        })
        .await
    }

    /// 当前端点映射。
    async fn endpoints_map(&self) -> Result<std::collections::BTreeMap<String, String>, AppError> {
        Ok(self.payload().await?.endpoints)
    }

    /// `list` 的读取前置：确保出厂默认已经落地。
    ///
    /// 出厂**端点**总是幂等补齐（只补缺失的内置端点）；出厂**启用集合**只在设置行尚不
    /// 存在时播种，而这件事由 `mutate_sources` 对所有首次写入统一完成——因此这里只剩
    /// 「补齐可能缺失的出厂端点」这一件事，`list` 与其它首次写入不会各自留下一份初始化。
    async fn ensure_default_sources(&self) -> Result<(), AppError> {
        self.mutate_sources(|payload, _persisted| {
            Ok(if seed_factory_endpoints(payload) {
                SourcesMutation::Changed(())
            } else {
                SourcesMutation::Unchanged(())
            })
        })
        .await
    }

    /// 读取完整持久化负载（无记录 → 空负载）。
    async fn payload(&self) -> Result<SourcesPayload, AppError> {
        let Some(row) =
            SettingsRepository::get(self.settings.as_settings(), SOURCES_SETTINGS_SECTION).await?
        else {
            return Ok(SourcesPayload::default());
        };
        decode_payload(&row.data_json)
    }

    /// 整包覆盖写。只有**未注入设置事务**的组装面才会走到这里（见 `mutations`）。
    async fn persist_payload(&self, payload: &SourcesPayload) -> Result<(), AppError> {
        SettingsRepository::upsert(self.settings.as_settings(), &payload_row(payload)?).await
    }
}

/// 内置来源中端点**由用户提供**、因此按 [`HttpUrlPolicy::UserProvidedSource`]
/// 端口策略保存与消费的 ID。
///
/// CMS10 与 M3U 的端点都由用户明确提供：采集站 API 和播放列表可以位于本机、局域网
/// 或使用非标准端口。其它内置来源的端点即使能被用户覆盖（例如 Gutenberg 的出厂端点），
/// 出站仍只认源码内的固定主机白名单与端口策略，因此这里不能把它们一起放进来。
pub const USER_PROVIDED_ENDPOINT_SOURCE_IDS: [&str; 2] = ["cms10", "m3u"];

/// 端点校验与规范化：仅 http/https 绝对 URL，host 非空，长度 ≤ 500；
/// 去尾部 `/`。空串表示清除配置（返回 None）。
///
/// 这里保持 [`HttpUrlPolicy::SourceEndpoint`]：内置固定来源的端点即使可被用户覆盖，
/// 出站也由各 Provider 自己的主机白名单与端口策略把关，不能借保存路径放宽端口。
fn validate_endpoint(raw: &str) -> Result<Option<String>, AppError> {
    validate_source_endpoint_with(raw, HttpUrlPolicy::SourceEndpoint)
}

/// 用户提供的来源端点（自定义 OPDS/订阅源/漫画库/TVBox 配置、CMS10、M3U）。
///
/// 用户明确配置的自托管来源可使用本机、局域网、overlay VPN 地址或本地 DNS 名称，
/// 并可使用任意非零端口。仍拒绝非 http(s) scheme、userinfo、fragment、链路本地、
/// 未指定、组播、云元数据与其它保留地址；固定来源和媒体资源仍使用更严格策略。
fn validate_user_provided_endpoint(raw: &str) -> Result<Option<String>, AppError> {
    validate_source_endpoint_with(raw, HttpUrlPolicy::UserProvidedSource)
}

fn validate_source_endpoint_with(
    raw: &str,
    policy: HttpUrlPolicy,
) -> Result<Option<String>, AppError> {
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
    parse_http_url(trimmed, policy).map_err(|_| invalid_endpoint())?;
    Ok(Some(trimmed.trim_end_matches('/').to_owned()))
}

/// 内置来源端点保存时使用的策略选择：用户提供端点的来源走用户端口策略，其余走固定
/// 策略。保存与出站必须选同一个策略，否则会出现「能存不能读」或「存的是别的端口」。
fn endpoint_validator(source_id: &str) -> fn(&str) -> Result<Option<String>, AppError> {
    if source_id == "m3u" {
        validate_m3u_playlist_endpoint
    } else if USER_PROVIDED_ENDPOINT_SOURCE_IDS.contains(&source_id) {
        validate_user_provided_endpoint
    } else {
        validate_endpoint
    }
}

/// M3U 地址由用户提供，因而可指向本机/局域网及任意非零端口。
/// 带 query 的播放列表 URL 可能包含访问令牌，因此只允许 HTTPS 承载 query。
fn validate_m3u_playlist_endpoint(raw: &str) -> Result<Option<String>, AppError> {
    let endpoint = validate_user_provided_endpoint(raw)?;
    if let Some(endpoint) = endpoint.as_deref() {
        let parsed = parse_http_url(endpoint, HttpUrlPolicy::UserProvidedSource)
            .map_err(|_| invalid_endpoint())?;
        if parsed.as_url().query().is_some() && parsed.as_url().scheme() != "https" {
            return Err(invalid_argument("带查询参数的 M3U 地址必须使用 HTTPS"));
        }
    }
    Ok(endpoint)
}

/// TVBox / FongMi 配置地址：在**用户提供来源**策略之上只放宽**长度**，不改任何安全判定。
///
/// 与通用端点校验的两点差异都是有意的：
/// - 允许带 query。TVBox 配置地址常把访问令牌放在 query 里，而本仓库没有承载它的
///   typed credential 路径；地址本身只留在后端（`list()` 不回传端点，命令层
///   `Debug` 脱敏），这与预览切片对同一类地址的处理一致。
/// - 长度上限与预览用例共用 [`MAX_CONFIG_URL_CHARS`]（按字符数），而不是通用端点的
///   500 字节，避免出现「预览能过、保存被拒」而文案又解释不清的落差。
///
/// 其余约束（拒绝非 http(s) scheme、userinfo、fragment、链路本地与保留地址）仍由
/// 共享策略统一提供；用户可配置本机/局域网来源，并使用任意非零端口——预览与保存、
/// 保存与出站因此是同一个策略。这里**不做**去掉末尾 `/` 的规范化：配置地址是一个具体资源，
/// 改写路径会让缓存对不上原文。
fn validate_tvbox_endpoint(raw: &str) -> Result<String, AppError> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return Err(invalid_argument("TVBox 配置地址不能为空"));
    }
    if trimmed.chars().count() > MAX_CONFIG_URL_CHARS {
        return Err(invalid_argument("TVBox 配置地址过长"));
    }
    parse_http_url(trimmed, HttpUrlPolicy::UserProvidedSource).map_err(|_| invalid_endpoint())?;
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

/// 订阅源端点：在用户提供来源策略之上强制 HTTPS，并拒绝 query 串。
///
/// 私有 Feed 常把访问令牌放在 URL query 中，而当前没有受控的 typed credential
/// 路径可以安全承载 Feed query 凭据：把它留在设置里会渗进日志、错误与来源身份。
/// 因此这里 fail closed 地拒绝带 query 的端点，并在用户文档中说明这一限制。
/// 其余约束仍由共享策略统一提供（拒绝 userinfo/fragment/链路本地与保留地址），
/// 端口与出站每一跳都使用同一个用户提供来源策略。
fn validate_feed_endpoint(raw: &str) -> Result<String, AppError> {
    validate_scoped_user_endpoint(
        raw,
        "订阅源地址必须使用 HTTPS",
        "订阅源地址不能包含查询串；带访问令牌的 Feed 请在来源服务端改用不带 query 的地址",
    )
}

/// 自托管漫画库端点：与订阅源同样的 HTTPS + 无 query 约束。
///
/// Komga 的 API key 走 `X-API-Key` 请求头、Kavita 走 `x-api-key`，两者都不需要
/// 把凭据放进 URL。因此这里可以 fail closed 地拒绝任何带 query 的端点，避免
/// 访问令牌以地址形式留在设置、日志或来源身份里。
fn validate_comic_library_endpoint(raw: &str) -> Result<String, AppError> {
    validate_scoped_user_endpoint(
        raw,
        "漫画库地址必须使用 HTTPS",
        "漫画库地址不能包含查询串；API key 会通过请求头注入，不需要写进地址",
    )
}

/// 用户提供端点 + HTTPS + 无 query 的共用校验（订阅源与自托管漫画库同一份口径）。
fn validate_scoped_user_endpoint(
    raw: &str,
    https_message: &'static str,
    query_message: &'static str,
) -> Result<String, AppError> {
    let normalized = validate_user_provided_endpoint(raw)?
        .ok_or_else(|| invalid_argument("端点地址不能为空"))?;
    if !normalized.starts_with("https://") {
        return Err(AppError::new(
            "INVALID_ARGUMENT",
            ErrorKind::Validation,
            https_message,
            false,
        ));
    }
    if normalized.contains('?') {
        return Err(AppError::new(
            "INVALID_ARGUMENT",
            ErrorKind::Validation,
            query_message,
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

/// keystore IO 之后的复核失败：来源已在 IO 期间被删除或被改写，本次写入作废
/// （调用方负责把 keystore 补偿回写入之前的状态）。
fn source_config_stale() -> AppError {
    AppError::new(
        "SOURCE_CONFIG_STALE",
        ErrorKind::Conflict,
        "来源配置已变化，请重新加载后再试",
        true,
    )
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
    // 版本 token 随来源一起消失：迟到的观测既找不到来源记录，也匹配不上下一次分配。
    payload.source_revisions.remove(source_id);
    payload.custom_sources.len() != before
}

/// 启用集合投影：自定义记录的 `enabled` 是权威，`enabledSources` 只提供其余来源的顺序。
fn enabled_from_payload(payload: &SourcesPayload) -> HashSet<String> {
    let mut set: HashSet<String> = payload.enabled_sources.iter().cloned().collect();
    for custom in &payload.custom_sources {
        if custom.enabled {
            set.insert(custom.source_id.clone());
        } else {
            set.remove(&custom.source_id);
        }
    }
    set
}

/// 自定义来源记录 → 注册表描述符（尚未叠加健康事实）。
fn custom_descriptor(custom: &CustomSourceRecord) -> SourceDescriptorDto {
    SourceDescriptorDto {
        source_id: custom.source_id.clone(),
        display_name: custom.display_name.clone(),
        kinds: custom_kind_kinds(custom.kind).to_vec(),
        categories: custom_kind_categories(custom.kind).to_vec(),
        mode: SourceModeDto::Single,
        notes: custom_kind_notes(custom.kind).to_owned(),
        enabled: custom.enabled,
        health: SourceHealthDto::Unknown,
        endpoint_configured: !custom.endpoint.is_empty(),
        // 只有确实归属该来源（种类 + 身份）的引用才算「已配置凭据」：伪造、串源或跨
        // provider 的引用不会在设置页上被投影成一项真实能力。
        credential_configured: custom.credential_ref.as_deref().is_some_and(|reference| {
            ref_belongs_to_source(custom.kind, &custom.source_id, reference)
        }),
        last_checked: None,
        latency_ms: None,
        success_rate: None,
    }
}

/// 推进一个来源的配置版本。
///
/// 版本是**状态版本**语义：每一次影响出站的配置变更都产生一个新 token，因此
/// A→B→A 也会与分发时的版本不同，携带旧版本的观测永远不会被采纳。
fn bump_source_revision(payload: &mut SourcesPayload, source_id: &str) {
    let previous = payload.source_revisions.get(source_id);
    let mut token = new_revision();
    // 同一毫秒内连续两次变更也必须产生不同版本（否则旧观测会被误判为仍然新鲜）。
    while previous.is_some_and(|previous| previous == &token) {
        token = new_revision();
    }
    payload.source_revisions.insert(source_id.to_owned(), token);
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
    // 缓存端口的方法来自这个 trait：`InMemorySourceConfigCache` 的具体类型在这里可见，
    // 但 `put` / `get` / `delete` 是 trait 方法，必须在**本模块**把它引入作用域。
    use crate::services::source_config_cache::SourceConfigCache;
    use haven_infrastructure::Db;
    use haven_infrastructure::db::repos::SqliteRepositories;

    fn service_from_memory() -> SourceRegistryService {
        let db = Arc::new(Db::open_in_memory().unwrap());
        let repos = Arc::new(SqliteRepositories::new(db));
        SourceRegistryService::new(repos)
            .with_credential_lease(Arc::new(InMemorySourceCredentialLease::new()))
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

    /// 第一次 `list` **之前**的显式停用必须落地：这次写入要先把出厂默认播种下来，
    /// 再应用用户的选择，否则紧随其后的 `list` 会把出厂启用的来源重新打开。
    #[tokio::test]
    async fn explicit_disable_before_the_first_list_is_persisted() {
        let service = service_from_memory();
        service
            .set(SourceRegistrySetRequest {
                source_id: "opds_gutenberg".to_owned(),
                enabled: false,
            })
            .await
            .unwrap();

        let registry = service.list().await.unwrap();
        let gutenberg = registry
            .sources
            .iter()
            .find(|source| source.source_id == "opds_gutenberg")
            .unwrap();
        assert!(
            !gutenberg.enabled,
            "首次 list 之前的停用选择不得被出厂播种覆盖"
        );
        // 其余出厂启用来源不受影响。
        for source_id in ["mangadex", "arxiv", "europepmc", "wikisource"] {
            assert!(
                registry
                    .sources
                    .iter()
                    .find(|source| source.source_id == source_id)
                    .unwrap()
                    .enabled,
                "{source_id} 仍应按出厂默认启用"
            );
        }
        // 生产装配面（内存仓储）也走同一条播种路径：出厂端点一并落地。
        assert!(
            gutenberg.endpoint_configured,
            "出厂端点与出厂启用来自同一份初始化"
        );
    }

    /// 端点写入是另一种「首次写入」：它必须与应用启用位那条路径共享同一份初始化，
    /// 否则先写端点的用户会得到一个没有出厂启用来源的 section。
    #[tokio::test]
    async fn endpoint_first_write_shares_the_same_factory_initialization() {
        let service = service_from_memory();
        service
            .set_endpoint("cms10", "https://example.invalid/api.php/provide/vod")
            .await
            .unwrap();

        let registry = service.list().await.unwrap();
        for source_id in [
            "opds_gutenberg",
            "mangadex",
            "arxiv",
            "europepmc",
            "wikisource",
        ] {
            assert!(
                registry
                    .sources
                    .iter()
                    .find(|source| source.source_id == source_id)
                    .unwrap()
                    .enabled,
                "{source_id} 的出厂启用不取决于哪一次写入先到"
            );
        }
        let cms10 = registry
            .sources
            .iter()
            .find(|source| source.source_id == "cms10")
            .unwrap();
        assert!(!cms10.enabled, "仅写入端点不得自动启用 CMS10");
        assert!(cms10.endpoint_configured);
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

    /// 已落库的来源负载（暂存槽与清理队列的事实只在这里可见）。
    async fn stored_payload(service: &SourceRegistryService) -> SourcesPayload {
        service.payload().await.expect("来源负载必须可读")
    }

    #[test]
    fn persisted_source_debug_redacts_endpoints_and_credential_references() {
        let endpoint_token = "endpoint-token-sentinel";
        let credential_ref = "credential-reference-sentinel";
        let source = CustomSourceRecord {
            source_id: "custom_opds_0123456789ab".to_owned(),
            display_name: "自托管书库".to_owned(),
            endpoint: format!("https://catalog.example.invalid/opds?token={endpoint_token}"),
            enabled: true,
            credential_ref: Some(credential_ref.to_owned()),
            kind: CustomSourceKind::Opds,
        };
        let source_debug = format!("{source:?}");
        assert!(!source_debug.contains(endpoint_token), "{source_debug}");
        assert!(!source_debug.contains(credential_ref), "{source_debug}");

        let payload = SourcesPayload {
            endpoints: [(
                "m3u".to_owned(),
                format!("https://playlist.example/list?token={endpoint_token}"),
            )]
            .into_iter()
            .collect(),
            custom_sources: vec![source],
            ..SourcesPayload::default()
        };
        let payload_debug = format!("{payload:?}");
        assert!(!payload_debug.contains(endpoint_token), "{payload_debug}");
        assert!(!payload_debug.contains(credential_ref), "{payload_debug}");
        assert!(
            !payload_debug.contains("catalog.example.invalid"),
            "{payload_debug}"
        );
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
    async fn custom_source_display_names_use_a_character_limit() {
        let service = service_from_memory();
        let allowed_name = "书".repeat(100);
        let added = service
            .add_custom_source(&allowed_name, "https://example.invalid/opds")
            .await
            .unwrap();

        let updated_name = "馆".repeat(100);
        service
            .update_custom_source(crate::wire::SourceUpdateRequest {
                source_id: added.source_id.clone(),
                display_name: Some(updated_name.clone()),
                endpoint: None,
            })
            .await
            .unwrap();

        let registry = service.list().await.unwrap();
        let source = registry
            .sources
            .iter()
            .find(|source| source.source_id == added.source_id)
            .unwrap();
        assert_eq!(source.display_name, updated_name);

        let too_long_name = "书".repeat(101);
        assert!(
            service
                .add_custom_source(&too_long_name, "https://another.example.invalid/opds")
                .await
                .is_err()
        );
    }

    #[tokio::test]
    async fn custom_source_credential_write_read_delete() {
        let (service, source_id) = seeded_custom_source().await;
        let store = MemoryStore::new();

        // 写凭据：keyring 里出现一个**新的随机版本槽**，持久化记录持有它的引用。
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
        let target = service
            .custom_credential_ref(&source_id)
            .await
            .unwrap()
            .expect("写入后必须存在凭据引用");
        assert!(
            target
                .as_str()
                .starts_with(&format!("haven:{OPDS_CREDENTIAL_PROVIDER}_")),
            "OPDS 书源的槽位必须落在 opds 作用域命名空间: {}",
            target.as_str()
        );
        assert!(
            !target.as_str().ends_with(&source_id),
            "版本槽不得由 sourceId 拼接而成"
        );
        let stored = store.get(&target).await.unwrap().unwrap();
        assert_eq!(stored.expose(), "s3cret");
        // 写入完成后没有遗留的暂存槽或清理义务。
        let payload = stored_payload(&service).await;
        assert!(payload.staged_credentials.is_empty());
        assert!(payload.credential_cleanup.is_empty());

        // 清除凭据：引用被撤销，旧槽位从 keyring 删除，队列清空（终态断言）。
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
        let payload = stored_payload(&service).await;
        assert!(payload.credential_cleanup.is_empty(), "清理义务已履行完");
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
        // 失败的一次写入只留下**自己的**槽位痕迹，并且已经被补偿清理：暂存队列与清理
        // 队列都不残留（也没有任何旧凭据被还原或删除的分支）。
        let payload = stored_payload(&service).await;
        assert!(payload.staged_credentials.is_empty());
        assert!(payload.credential_cleanup.is_empty());
    }

    #[tokio::test]
    async fn remove_queues_the_credential_then_deletes_it() {
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
        let credential = service
            .custom_credential_ref(&source_id)
            .await
            .unwrap()
            .expect("写入后必须存在凭据引用");

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
            [credential.as_str().to_owned()],
            "只删除该来源当前引用的版本槽"
        );
        // 记录已从注册表消失。
        let registry = service.list().await.unwrap();
        assert!(!registry.sources.iter().any(|s| s.source_id == source_id));
        // 记录移除与清理义务同事务登记，义务在外部删除成功后出队。
        let payload = stored_payload(&service).await;
        assert!(payload.credential_cleanup.is_empty());
        assert!(payload.cache_cleanup.is_empty());
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
    async fn feed_source_rejects_unsafe_or_plaintext_but_accepts_self_hosted_endpoints() {
        let service = service_from_memory();
        for endpoint in [
            "http://feeds.example.invalid/rss.xml",
            "https://user:secret@feeds.example.invalid/rss.xml",
            "https://feeds.example.invalid/rss.xml#fragment",
            // 用户提供来源策略接受任意非零端口，因此这里只保留真正非法的端口形态
            // （正例见 `user_provided_endpoints_accept_self_hosted_ports`）。
            "https://feeds.example.invalid:0/rss.xml",
            "https://feeds.example.invalid:65536/rss.xml",
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
        for endpoint in [
            "https://127.0.0.1:8443/rss.xml",
            "https://192.168.1.4:5001/rss.xml",
            "https://feeds/rss.xml",
            "https://feeds.internal/rss.xml",
        ] {
            assert!(
                service
                    .add_feed_source("自托管订阅", endpoint)
                    .await
                    .is_ok(),
                "用户明确配置的本机/局域网订阅地址应可用: {endpoint}"
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

    /// 用户提供端点的来源（自定义 OPDS/订阅源/漫画库/TVBox 配置、CMS10）保存时接受任意
    /// 非零端口；固定来源保持原来的端口集合。
    #[tokio::test]
    async fn user_provided_endpoints_accept_self_hosted_ports() {
        let service = service_from_memory();
        let cases: Vec<(String, String)> = vec![
            (
                service
                    .add_custom_source("自建书库", "http://media.example.invalid:8096/opds")
                    .await
                    .unwrap()
                    .source_id,
                "http://media.example.invalid:8096/opds".to_owned(),
            ),
            (
                service
                    .add_feed_source("自建订阅", "https://feed.example.invalid:5001/rss.xml")
                    .await
                    .unwrap()
                    .source_id,
                "https://feed.example.invalid:5001/rss.xml".to_owned(),
            ),
            (
                service
                    .add_comic_library_source(
                        "自建 Komga",
                        "https://komga.example.invalid:32400",
                        CustomSourceKind::Komga,
                    )
                    .await
                    .unwrap()
                    .source_id,
                "https://komga.example.invalid:32400".to_owned(),
            ),
            (
                service
                    .add_comic_library_source(
                        "自建 Kavita",
                        "https://kavita.example.invalid:9117",
                        CustomSourceKind::Kavita,
                    )
                    .await
                    .unwrap()
                    .source_id,
                "https://kavita.example.invalid:9117".to_owned(),
            ),
            (
                service
                    .add_tvbox_source("自建配置", "https://tvbox.example.invalid:9973/config.json")
                    .await
                    .unwrap()
                    .source_id,
                "https://tvbox.example.invalid:9973/config.json".to_owned(),
            ),
        ];
        for (source_id, endpoint) in &cases {
            assert_eq!(
                service.endpoint(source_id).await.unwrap().as_deref(),
                Some(endpoint.as_str()),
                "用户提供的端口必须原样保存"
            );
        }

        // CMS10 同样由用户填端点：8096 之类的自建端口必须能存下来。
        assert!(
            service
                .set_endpoint("cms10", "http://cms.example.invalid:8096/api.php")
                .await
                .unwrap()
        );
        assert_eq!(
            service.endpoint("cms10").await.unwrap().as_deref(),
            Some("http://cms.example.invalid:8096/api.php")
        );
    }

    /// 用户提供端点可指向私有/本机网络；链路本地/保留地址、端口 0、userinfo 与
    /// fragment 仍须在每一条保存路径 fail closed。
    #[tokio::test]
    async fn user_provided_endpoints_accept_local_servers_but_reject_reserved_and_malformed_targets()
     {
        let service = service_from_memory();
        for endpoint in [
            "http://169.254.169.254:8096/latest/meta-data",
            "http://198.18.0.1:8096/opds",
            "http://192.0.0.9:8096/opds",
            "https://[2001:db8::1]:8096/opds",
            "https://metadata.google.internal:8096/opds",
            "https://catalog.example.invalid:0/opds",
            "https://catalog.example.invalid:/opds",
            "https://user:secret@catalog.example.invalid:8096/opds",
            "https://catalog.example.invalid:8096/opds#fragment",
            "ftp://catalog.example.invalid:8096/opds",
        ] {
            assert!(
                !service
                    .set_endpoint("cms10", endpoint)
                    .await
                    .unwrap_or(false),
                "CMS10 保存不得接受: {endpoint}"
            );
            assert!(
                service.add_custom_source("书源", endpoint).await.is_err(),
                "自定义书源保存不得接受: {endpoint}"
            );
            assert!(
                service.add_tvbox_source("配置", endpoint).await.is_err(),
                "TVBox 配置保存不得接受: {endpoint}"
            );
            assert!(
                service
                    .add_comic_library_source("漫画库", endpoint, CustomSourceKind::Komga)
                    .await
                    .is_err(),
                "漫画库保存不得接受: {endpoint}"
            );
        }

        assert!(
            service
                .set_endpoint("cms10", "http://127.0.0.1:8096/api.php")
                .await
                .expect("CMS10 应接受用户配置的本机端点")
        );
        assert!(
            service
                .add_custom_source("局域网书源", "http://192.168.1.10:8096/opds")
                .await
                .is_ok()
        );
        assert!(
            service
                .add_feed_source("本地订阅", "https://feeds.local:8443/rss.xml")
                .await
                .is_ok()
        );
        assert!(
            service
                .add_tvbox_source("本地配置", "http://tvbox:9978/config.json")
                .await
                .is_ok()
        );
        assert!(
            service
                .add_comic_library_source(
                    "局域网漫画库",
                    "https://komga.local:5001/komga",
                    CustomSourceKind::Komga
                )
                .await
                .is_ok()
        );
    }

    /// 固定来源（端点由源码内白名单决定）保存时保持原端口策略；M3U 是用户明确提供的
    /// 播放列表地址，应允许本机/局域网与自选端口。
    #[tokio::test]
    async fn fixed_sources_keep_the_previous_port_policy_and_m3u_accepts_user_endpoints() {
        let service = service_from_memory();
        for (source_id, endpoint) in [(
            "opds_gutenberg",
            "https://www.gutenberg.org:8096/ebooks.opds/",
        )] {
            assert!(
                service.set_endpoint(source_id, endpoint).await.is_err(),
                "{source_id} 的固定端口策略不得被用户端口策略取代"
            );
        }
        // M3U 的地址由用户填写：自建服务常用 HTTP 与任意非零端口。
        assert!(
            service
                .set_endpoint("m3u", "http://127.0.0.1:9978/list.m3u")
                .await
                .unwrap()
        );
        assert!(
            service
                .set_endpoint(
                    "m3u",
                    "http://playlist.example.invalid:8096/list.m3u?token=x"
                )
                .await
                .is_err(),
            "明文 HTTP 不得承载 M3U query token"
        );
        assert!(
            service
                .set_endpoint(
                    "m3u",
                    "https://playlist.example.invalid:8096/list.m3u?token=x"
                )
                .await
                .unwrap()
        );
    }

    /// 出站请求快照：种类、端点 origin、配置版本与凭据归属来自**同一次**读取，
    /// 且 Debug 不泄漏端点/版本/来源身份。
    #[tokio::test]
    async fn registered_source_request_snapshot_is_single_read_and_redacted() {
        let service = service_from_memory();
        let payload: SourcesPayload = serde_json::from_str(
            r#"{"enabledSources":["custom_snapshot0001"],"endpoints":{},
                "customSources":[{"sourceId":"custom_snapshot0001","displayName":"快照书源","endpoint":"https://catalog.example.invalid:8096/opds","enabled":true,"credentialRef":"haven:opds:custom_snapshot0001","kind":"opds"}],
                "sourceRevisions":{"custom_snapshot0001":"rev-1"}}"#,
        )
        .unwrap();
        service.persist_payload(&payload).await.unwrap();

        let snapshot = service
            .registered_source_request_snapshot("custom_snapshot0001")
            .await
            .unwrap()
            .expect("已登记来源必须有快照");
        assert_eq!(snapshot.kind(), CustomSourceKind::Opds);
        assert_eq!(
            snapshot.endpoint_origin(),
            Some("https://catalog.example.invalid:8096")
        );
        assert_eq!(snapshot.revision(), Some("rev-1"));
        assert!(snapshot.credential_configured());
        assert!(snapshot.is_current_with(&snapshot.clone()));

        let rendered = format!("{snapshot:?}");
        for leaked in [
            "catalog.example.invalid",
            "8096",
            "custom_snapshot0001",
            "rev-1",
            "haven:opds",
        ] {
            assert!(
                !rendered.contains(leaked),
                "快照 Debug 不得泄漏 {leaked}: {rendered}"
            );
        }

        // 端点或配置版本变了，旧上下文就不再权威（异步凭据解析后的复核依据）。
        let mut moved = payload.clone();
        moved.custom_sources[0].endpoint = "https://catalog.example.invalid:8097/opds".to_owned();
        service.persist_payload(&moved).await.unwrap();
        let after = service
            .registered_source_request_snapshot("custom_snapshot0001")
            .await
            .unwrap()
            .unwrap();
        assert!(
            !snapshot.is_current_with(&after),
            "端点变化必须让旧上下文失效"
        );

        let mut bumped = payload.clone();
        bumped
            .source_revisions
            .insert("custom_snapshot0001".to_owned(), "rev-2".to_owned());
        service.persist_payload(&bumped).await.unwrap();
        let after = service
            .registered_source_request_snapshot("custom_snapshot0001")
            .await
            .unwrap()
            .unwrap();
        assert!(
            !snapshot.is_current_with(&after),
            "版本变化必须让旧上下文失效"
        );

        // 未登记 / 内置 / 家族之外一律 None：调用方据此 fail closed，而不是凭名字授权。
        for unknown in ["custom_ffffffffffff", "cms10", "", "mangadex"] {
            assert!(
                service
                    .registered_source_request_snapshot(unknown)
                    .await
                    .unwrap()
                    .is_none(),
                "{unknown} 不应出现快照"
            );
        }
    }

    /// `sourceId` 前缀 → 种类的恢复只认注册表自己生成的形状；其它来源一律 None。
    #[test]
    fn custom_kind_from_source_id_only_recognizes_registry_shapes() {
        assert_eq!(
            SourceRegistryService::custom_kind_from_source_id("custom_0123456789ab"),
            Some(CustomSourceKind::Opds)
        );
        assert_eq!(
            SourceRegistryService::custom_kind_from_source_id("custom_feed_0123456789ab"),
            Some(CustomSourceKind::Feed)
        );
        assert_eq!(
            SourceRegistryService::custom_kind_from_source_id("custom_komga_0123456789ab"),
            Some(CustomSourceKind::Komga)
        );
        assert_eq!(
            SourceRegistryService::custom_kind_from_source_id("custom_kavita_0123456789ab"),
            Some(CustomSourceKind::Kavita)
        );
        assert_eq!(
            SourceRegistryService::custom_kind_from_source_id("custom_tvbox_0123456789ab"),
            Some(CustomSourceKind::Tvbox)
        );
        for foreign in [
            "cms10",
            "opds_gutenberg",
            "mangadex",
            "custom",
            "Custom_x",
            "",
        ] {
            assert_eq!(
                SourceRegistryService::custom_kind_from_source_id(foreign),
                None,
                "{foreign} 不是自定义来源身份"
            );
        }
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

        // API key 使用独立的 provider 命名空间，不与 OPDS 密码共用；槽位本身由注册表
        // 分配并持久化在来源记录里（这里是唯一权威来源）。
        let target = service
            .custom_credential_ref(&added.source_id)
            .await
            .unwrap()
            .expect("写入后必须存在凭据引用");
        assert!(
            target
                .as_str()
                .starts_with(&format!("haven:{KOMGA_CREDENTIAL_PROVIDER}_")),
            "Komga 的 API key 必须落在 komga 作用域命名空间: {}",
            target.as_str()
        );
        assert_eq!(
            SourceRegistryService::credential_provider(CustomSourceKind::Komga),
            Some(KOMGA_CREDENTIAL_PROVIDER)
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
        for forbidden in ["komga-api-key", "haven:komga", "credentialRef"] {
            assert!(
                !encoded.contains(forbidden),
                "来源描述符不得包含 {forbidden}"
            );
        }

        // 删除顺序是 ADR-0002 的撤销优先：先在同一事务里撤销记录并登记清理义务，
        // 之后才做外部删除（`credential_deleted` 报告的是本次是否真的删掉了引用）。
        let removed = service
            .remove_custom_source(&added.source_id, &store, &memory_cache())
            .await
            .unwrap();
        assert!(removed.credential_deleted);
    }

    #[tokio::test]
    async fn comic_library_endpoints_require_https_without_query_and_accept_self_hosted_hosts() {
        let service = service_from_memory();
        for endpoint in [
            "http://comics.example.invalid",
            "https://comics.example.invalid?token=secret-token",
            "https://user:secret@comics.example.invalid",
            "https://comics.example.invalid#fragment",
            // 用户提供来源策略接受任意非零端口，因此这里只保留真正非法的端口形态
            // （正例见 `user_provided_endpoints_accept_self_hosted_ports`）。
            "https://comics.example.invalid:0",
            "https://comics.example.invalid:65536",
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
        for endpoint in [
            "https://127.0.0.1:5001/komga",
            "https://192.168.1.4:5001/komga",
            "https://nas:5001/komga",
            "https://comics.internal:5001/komga",
        ] {
            assert!(
                service
                    .add_comic_library_source("自托管漫画库", endpoint, CustomSourceKind::Komga)
                    .await
                    .is_ok(),
                "用户明确配置的自托管漫画库应可用: {endpoint}"
            );
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
    fn credential_provider_is_absent_for_kinds_without_a_credential_consumer() {
        // 订阅源与 TVBox 配置来源没有受控的凭据消费路径：它们不得拿到一个永远不会被
        // 任何请求消费的命名空间。
        assert_eq!(
            SourceRegistryService::credential_provider(CustomSourceKind::Feed),
            None
        );
        assert_eq!(
            SourceRegistryService::credential_provider(CustomSourceKind::Tvbox),
            None
        );
        // 三种有凭据的来源各自使用独立命名空间（旧数据的 provider 段保持不变）。
        assert_eq!(
            SourceRegistryService::credential_provider(CustomSourceKind::Opds),
            Some(OPDS_CREDENTIAL_PROVIDER)
        );
        assert_eq!(
            SourceRegistryService::credential_provider(CustomSourceKind::Kavita),
            Some("kavita")
        );
    }

    /// 历史记录的稳定引用必须继续可读：引用按记录持久化，读取只看记录本身。
    #[tokio::test]
    async fn legacy_stable_reference_is_readable() {
        let (service, source_id) = seeded_custom_source().await;
        let legacy = format!("haven:{OPDS_CREDENTIAL_PROVIDER}:{source_id}");
        let mut payload = stored_payload(&service).await;
        let record = payload
            .custom_sources
            .iter_mut()
            .find(|record| record.source_id == source_id)
            .expect("已登记的来源记录");
        record.credential_ref = Some(legacy.clone());
        service.persist_payload(&payload).await.unwrap();

        assert_eq!(
            service
                .custom_credential_ref(&source_id)
                .await
                .unwrap()
                .map(|r| r.as_str().to_owned()),
            Some(legacy),
            "旧记录里的稳定引用照常被读出"
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
            SourceRegistryService::credential_provider(CustomSourceKind::Tvbox).is_none(),
            "TVBox 配置来源没有凭据消费者"
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

    // ---------- 外部凭据 I/O 租约（写者仲裁） ----------

    /// 没有注入租约的装配面：只读入口照常可用，四条会碰外部凭据库的入口一律 fail closed，
    /// 且**一个字节都不写**外部存储。
    ///
    /// 这里刻意用**没有设置事务**的本地夹具（`Db::open_in_memory` + `SqliteRepositories`）：
    /// 断言的是「缺租约就在任何外部 I/O 之前 fail closed」，而这件事发生在取得租约那一步
    /// ——四条入口都在任何事务与任何写入之前返回，因此事务通道是不是真实事务对这几条断言
    /// 没有影响。「真实 `SqliteSettingsUoW` + 缺租约」的同款断言在
    /// `tests/source_credential/{lifecycle,cleanup}.rs` 里跑：`haven-infrastructure` 的
    /// `SqliteSettingsUoW` 实现的是 **normal** crate 实例的 `SettingsUoW`，lib-test 实例里
    /// `Arc::new(SqliteSettingsUoW)` 无法满足 `Arc<dyn SettingsUoW>`（E0277），所以真实事务
    /// 只能在那边的集成装配面上注入。
    #[tokio::test]
    async fn a_missing_credential_lease_fails_closed_without_touching_the_store() {
        let db = Arc::new(Db::open_in_memory().unwrap());
        let repos = Arc::new(SqliteRepositories::new(db));
        let service = SourceRegistryService::new(repos);
        let store = MemoryStore::new();
        let source_id = service
            .add_custom_source("无租约装配", "https://plain.example.invalid/opds")
            .await
            .unwrap()
            .source_id;

        // 只读入口不受影响。
        assert!(
            service
                .custom_credential_ref(&source_id)
                .await
                .unwrap()
                .is_none()
        );

        let mut errors = Vec::new();
        errors.push(
            service
                .set_custom_source_credential(
                    &crate::wire::SourceSetCredentialRequest {
                        source_id: source_id.clone(),
                        secret: Some("x".into()),
                    },
                    &store,
                )
                .await
                .unwrap_err(),
        );
        errors.push(
            service
                .set_custom_source_credential(
                    &crate::wire::SourceSetCredentialRequest {
                        source_id: source_id.clone(),
                        secret: None,
                    },
                    &store,
                )
                .await
                .unwrap_err(),
        );
        errors.push(
            service
                .remove_custom_source(&source_id, &store, &memory_cache())
                .await
                .unwrap_err(),
        );
        errors.push(
            service
                .retry_pending_cleanup(&store, &memory_cache(), UtcMillis::now())
                .await
                .unwrap_err(),
        );
        for error in &errors {
            assert_eq!(
                error.code().as_str(),
                "SOURCE_CREDENTIAL_LEASE_UNAVAILABLE",
                "没有租约时必须 fail closed: {error:?}"
            );
            assert!(!error.retryable(), "装配错误不是「稍后重试就好」");
        }
        assert!(
            store
                .entries
                .lock()
                .unwrap_or_else(|error| error.into_inner())
                .is_empty(),
            "被拒绝的操作不得触达系统凭据库"
        );
        assert!(
            store
                .deleted
                .lock()
                .unwrap_or_else(|error| error.into_inner())
                .is_empty()
        );
        // 来源记录没有被移除：拒绝发生在任何副作用之前。
        assert!(
            service
                .list()
                .await
                .unwrap()
                .sources
                .iter()
                .any(|source| source.source_id == source_id)
        );
    }

    /// 回滚一次刚登记的来源时，带凭据引用的记录必须被拒绝：那只能用
    /// `remove_custom_source`（凭据清理义务在那里），绝不能被一次回滚悄悄丢掉。
    #[tokio::test]
    async fn discarding_a_registration_never_drops_a_credential_bearing_record() {
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

        let error = service
            .discard_fresh_registration(&source_id)
            .await
            .expect_err("带凭据的记录不得被回滚路径丢弃");
        assert_eq!(error.code().as_str(), "SOURCE_CREDENTIAL_CLEANUP_REQUIRED");
        assert!(
            service
                .list()
                .await
                .unwrap()
                .sources
                .iter()
                .any(|source| source.source_id == source_id),
            "被拒绝的回滚不得移除记录"
        );
        assert!(
            service
                .custom_credential_ref(&source_id)
                .await
                .unwrap()
                .is_some(),
            "被拒绝的回滚不得撤销凭据引用"
        );
        assert_eq!(
            store
                .entries
                .lock()
                .unwrap_or_else(|error| error.into_inner())
                .len(),
            1,
            "系统凭据库里的那一条必须完好"
        );

        // 不带凭据的登记仍然可以正常撤销；未知 id 是幂等 no-op。
        let fresh = service
            .add_custom_source("待回滚", "https://fresh.example.invalid/opds")
            .await
            .unwrap()
            .source_id;
        service.discard_fresh_registration(&fresh).await.unwrap();
        assert!(
            !service
                .list()
                .await
                .unwrap()
                .sources
                .iter()
                .any(|source| source.source_id == fresh)
        );
        service
            .discard_fresh_registration("custom_missing0000")
            .await
            .expect("未知 id 的撤销必须幂等成功");
    }
}
