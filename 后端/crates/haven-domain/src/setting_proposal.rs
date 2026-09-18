//! SettingProposal 领域模型（V02-SETTING-PROPOSAL-001）。
//!
//! 把"要改哪一项设置"从一次直接写入，变成一条**可版本化、可校验、可审计**的
//! 领域事实。Agent / UI 只能创建、读取、拒绝提案；真正生效走显式的
//! `apply_confirmed` 路径（Application 层）。
//!
//! 不变量：
//! - Target 与 Change 强类型且闭合：`Global` 只接受同名 section 的
//!   `SettingsPatch`；`Edition`/`MediaItem` 只接受资源级 `PreferenceData`。
//!   任意 JSON Map 不能成为写入模型（沿用 `deny_unknown_fields` 的
//!   newtype 变体包裹写法，见 `settings.rs` 的 `SettingsPatch`）。
//! - 执行载荷（target / operation / patch 或 data / base_revision / provenance /
//!   expires_at）经 **canonical JSON**（递归按对象键的 UTF-8 字节序升序、紧凑
//!   UTF-8）序列化；digest 为该字节序列的 SHA-256 小写十六进制。
//!   proposal id、status、created_at **不进入**载荷：它们不改变"要执行什么"。
//! - Provenance 只承载安全短字符串与闭合 reason code；禁止 secret、SQL 与绝对路径。
//! - digest 的形状是闭合的：恰好 64 个 ASCII 小写十六进制字符。
//! - 回执（Receipt）必须自身可重放：`changed = true` 必然带 `applied_revision`，
//!   `changed` 与"before/after 是否相同"一致，且 before/after 落在 **target 对应的
//!   类型**上（Global 是整段 `SettingsValue`，资源级是 `PreferenceData`）。
//! - 从存储恢复时重算 canonical JSON 与 digest，与存储值不一致即报稳定的
//!   `SETTING_PROPOSAL_INTEGRITY_MISMATCH`，不静默接受被篡改的行。

use haven_common::{AppError, ErrorKind, UtcMillis};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::ids::{EditionId, MediaItemId, SettingChangeReceiptId, SettingProposalId};
use crate::settings::{PreferenceData, SettingsPatch, SettingsSection, SettingsValue};

/// canonical JSON 执行载荷的 schema 版本（写入存储并参与 digest）。
pub const SETTING_PROPOSAL_PAYLOAD_VERSION: u32 = 1;

/// SHA-256 十六进制摘要长度。
pub const SETTING_PROPOSAL_DIGEST_HEX_LEN: usize = 64;

/// provenance 来源类别的闭合集合。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProvenanceSourceKind {
    /// 用户在本机界面直接发起。
    User,
    /// Agent（本地或远端推理）建议。
    Agent,
    /// 应用自身的确定性逻辑（迁移、修复、默认值补全）。
    System,
}

/// provenance 行为主体的闭合集合。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProvenanceActorKind {
    User,
    Agent,
    /// 后台服务/例程（无交互会话）。
    Service,
}

/// provenance 理由的闭合 reason code（不接受自由文本理由作为分类）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProvenanceReason {
    /// 用户主动请求该项改动。
    UserRequest,
    /// Agent 给出的改进建议已被接受。
    AgentSuggestion,
    /// 系统默认值/首次初始化。
    SystemDefault,
    /// 版本升级带来的数据迁移。
    Migration,
}

/// 提案生命周期状态。
///
/// status 是存储事实，不参与 digest：同一条提案从 pending 到 applied 的执行
/// 载荷完全不变，因此外部展示的 digest 在状态迁移前后保持一致。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SettingProposalStatus {
    Pending,
    Applied,
    Rejected,
    Expired,
}

impl SettingProposalStatus {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Pending => "pending",
            Self::Applied => "applied",
            Self::Rejected => "rejected",
            Self::Expired => "expired",
        }
    }

    /// 从存储字符串解析（未知值拒绝，闭合集合）。
    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "pending" => Some(Self::Pending),
            "applied" => Some(Self::Applied),
            "rejected" => Some(Self::Rejected),
            "expired" => Some(Self::Expired),
            _ => None,
        }
    }

    /// 终态：不再接受任何状态迁移。
    pub const fn is_terminal(self) -> bool {
        !matches!(self, Self::Pending)
    }
}

/// 全局设置分区目标：必须自带 section，且与 patch 的 section 一致。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct GlobalSettingTarget {
    pub section: SettingsSection,
}

/// 版本级资源偏好目标。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct EditionSettingTarget {
    pub edition_id: EditionId,
}

/// 媒体条目级资源偏好目标：必须携带所属 edition，应用时与
/// `media_items.edition_id` 交叉校验，防止把偏好写到别的版本上。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct MediaItemSettingTarget {
    pub edition_id: EditionId,
    pub media_item_id: MediaItemId,
}

/// 写入作用域（强类型闭合联合；`scope` 为判别标签）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(tag = "scope", rename_all = "snake_case")]
pub enum SettingTarget {
    Global(GlobalSettingTarget),
    Edition(EditionSettingTarget),
    MediaItem(MediaItemSettingTarget),
}

impl SettingTarget {
    pub const fn global(section: SettingsSection) -> Self {
        Self::Global(GlobalSettingTarget { section })
    }

    pub const fn edition(edition_id: EditionId) -> Self {
        Self::Edition(EditionSettingTarget { edition_id })
    }

    pub const fn media_item(edition_id: EditionId, media_item_id: MediaItemId) -> Self {
        Self::MediaItem(MediaItemSettingTarget {
            edition_id,
            media_item_id,
        })
    }

    /// 存储/约束使用的 scope 字面量。
    pub const fn scope_str(self) -> &'static str {
        match self {
            Self::Global(_) => "global",
            Self::Edition(_) => "edition",
            Self::MediaItem(_) => "media_item",
        }
    }

    pub const fn section(self) -> Option<SettingsSection> {
        match self {
            Self::Global(target) => Some(target.section),
            Self::Edition(_) | Self::MediaItem(_) => None,
        }
    }

    pub const fn edition_id(self) -> Option<EditionId> {
        match self {
            Self::Global(_) => None,
            Self::Edition(target) => Some(target.edition_id),
            Self::MediaItem(target) => Some(target.edition_id),
        }
    }

    pub const fn media_item_id(self) -> Option<MediaItemId> {
        match self {
            Self::Global(_) | Self::Edition(_) => None,
            Self::MediaItem(target) => Some(target.media_item_id),
        }
    }

    /// 与 change 的结构兼容性（构造、从存储恢复与 apply 三处复用同一规则）。
    pub fn accepts(self, change: &SettingProposalChange) -> bool {
        match (self, change) {
            (Self::Global(target), SettingProposalChange::SettingsPatch(patch)) => {
                patch.section() == target.section
            }
            (
                Self::Edition(_) | Self::MediaItem(_),
                SettingProposalChange::ResourcePreference(_),
            ) => true,
            _ => false,
        }
    }
}

/// 提案要执行的操作（强类型闭合联合；`kind` 为判别标签）。
///
/// 只有两种操作可以进入设置事实源：全局分区部分更新，或资源级偏好数据。
/// 两者的载荷都是 `deny_unknown_fields` 的 Typed DTO，未知字段与任意 JSON Map
/// 都在反序列化边界被拒绝。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum SettingProposalChange {
    SettingsPatch(SettingsPatch),
    ResourcePreference(PreferenceData),
}

impl SettingProposalChange {
    pub const fn kind_str(&self) -> &'static str {
        match self {
            Self::SettingsPatch(_) => "settings_patch",
            Self::ResourcePreference(_) => "resource_preference",
        }
    }
}

/// 提案来源（闭合 source/actor 枚举 + 安全短 source_ref + 闭合 reason code）。
///
/// `source_ref` 指向发起方（界面标识、Agent 会话标识等），`summary` 则是给人读的
/// 一句说明。两者都必须通过安全短文本校验：禁止换行/控制字符、绝对路径、SQL
/// 片段与凭据关键词，避免 provenance 变成绕过设置事实源的旁路信道。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct SettingProvenance {
    pub source_kind: ProvenanceSourceKind,
    pub actor_kind: ProvenanceActorKind,
    pub reason: ProvenanceReason,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source_ref: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub summary: Option<String>,
}

impl SettingProvenance {
    pub fn new(
        source_kind: ProvenanceSourceKind,
        actor_kind: ProvenanceActorKind,
        reason: ProvenanceReason,
    ) -> Self {
        Self {
            source_kind,
            actor_kind,
            reason,
            source_ref: None,
            summary: None,
        }
    }

    /// 用户交互发起的默认来源。
    pub fn user(reason: ProvenanceReason) -> Self {
        Self::new(
            ProvenanceSourceKind::User,
            ProvenanceActorKind::User,
            reason,
        )
    }

    /// Agent 建议的默认来源。
    pub fn agent(reason: ProvenanceReason) -> Self {
        Self::new(
            ProvenanceSourceKind::Agent,
            ProvenanceActorKind::Agent,
            reason,
        )
    }

    /// 附加发起方引用（构造时校验）。
    pub fn with_source_ref(mut self, source_ref: impl Into<String>) -> Result<Self, AppError> {
        let source_ref = source_ref.into();
        validate_safe_short_text(
            &source_ref,
            PROVENANCE_SOURCE_REF_MAX_CHARS,
            "provenance source_ref",
        )?;
        self.source_ref = Some(source_ref);
        Ok(self)
    }

    /// 附加展示说明（构造时校验）。
    pub fn with_summary(mut self, summary: impl Into<String>) -> Result<Self, AppError> {
        let summary = summary.into();
        validate_safe_short_text(&summary, PROVENANCE_SUMMARY_MAX_CHARS, "provenance summary")?;
        self.summary = Some(summary);
        Ok(self)
    }

    /// 完整校验（构造与从存储恢复都要跑一遍）。
    pub fn validate(&self) -> Result<(), AppError> {
        if let Some(source_ref) = &self.source_ref {
            validate_safe_short_text(
                source_ref,
                PROVENANCE_SOURCE_REF_MAX_CHARS,
                "provenance source_ref",
            )?;
        }
        if let Some(summary) = &self.summary {
            validate_safe_short_text(summary, PROVENANCE_SUMMARY_MAX_CHARS, "provenance summary")?;
        }
        Ok(())
    }
}

const PROVENANCE_SOURCE_REF_MAX_CHARS: usize = 64;
const PROVENANCE_SUMMARY_MAX_CHARS: usize = 200;
const BASE_REVISION_MAX_CHARS: usize = 128;

/// 明确禁止出现在 provenance 短文本里的片段：绝对路径、SQL、凭据。
///
/// 这是**自由文本字段**的护栏，不是通用内容过滤：命中即拒绝，让调用方改用
/// 闭合 reason code，而不是把细节塞进 source_ref。
const FORBIDDEN_PROVENANCE_FRAGMENTS: &[&str] = &[
    "/",
    "\\",
    "..",
    ";",
    "--",
    "/*",
    "*/",
    "select ",
    "insert ",
    "update ",
    "delete ",
    "drop ",
    "pragma",
    "attach ",
    "sqlite_",
    "password",
    "passwd",
    "secret",
    "token",
    "api_key",
    "apikey",
    "bearer ",
    "authorization",
];

fn validate_safe_short_text(
    value: &str,
    max_chars: usize,
    field: &'static str,
) -> Result<(), AppError> {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        return Err(invalid_provenance(format!("{field} 不能为空")));
    }
    if trimmed.chars().count() > max_chars {
        return Err(invalid_provenance(format!("{field} 超过长度上限")));
    }
    if trimmed.chars().any(char::is_control) {
        return Err(invalid_provenance(format!("{field} 不允许控制字符")));
    }
    let lowered = trimmed.to_ascii_lowercase();
    if FORBIDDEN_PROVENANCE_FRAGMENTS
        .iter()
        .any(|fragment| lowered.contains(fragment))
    {
        return Err(invalid_provenance(format!(
            "{field} 不允许包含路径、SQL 或凭据片段"
        )));
    }
    Ok(())
}

/// base_revision 是数据库生成的不透明版本 token，只接受安全字符集。
fn validate_base_revision(revision: &str) -> Result<(), AppError> {
    if revision.is_empty() || revision.chars().count() > BASE_REVISION_MAX_CHARS {
        return Err(invalid_proposal(
            "SETTING_PROPOSAL_INVALID_BASE_REVISION",
            "base_revision 长度非法",
        ));
    }
    if !revision
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
    {
        return Err(invalid_proposal(
            "SETTING_PROPOSAL_INVALID_BASE_REVISION",
            "base_revision 只允许不透明 token 字符",
        ));
    }
    Ok(())
}

/// digest 唯一覆盖的执行载荷。
///
/// 只包含"真正会执行的东西"：目标、操作、数据、基线版本、来源、过期时间。
/// proposal id / status / created_at 刻意排除在外。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct SettingProposalPayload {
    pub version: u32,
    pub target: SettingTarget,
    pub operation: SettingProposalChange,
    /// 目标作用域当前 authoritative revision；`None` 表示目标行尚未持久化
    /// （Global 走分区默认值，资源偏好走空 `PreferenceData`）。
    pub base_revision: Option<String>,
    pub provenance: SettingProvenance,
    /// 过期时间（UTC 毫秒）。参与 digest：延长/缩短有效期会改变执行语义。
    pub expires_at: i64,
}

/// 一条设置变更提案。
///
/// 字段私有：canonical_json 与 digest 只能由构造/恢复路径产生，外部无法在
/// 构造后篡改其中一个而让另一个保持自洽。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SettingProposal {
    id: SettingProposalId,
    payload: SettingProposalPayload,
    created_at: UtcMillis,
    status: SettingProposalStatus,
    canonical_json: String,
    digest: String,
}

impl SettingProposal {
    /// 构造新提案（status 固定为 `Pending`）。
    ///
    /// 时间由调用方给出：Application 决定"现在"和 TTL，领域只校验
    /// `expires_at > created_at` 并固化 canonical JSON / digest。
    pub fn new(
        id: SettingProposalId,
        target: SettingTarget,
        change: SettingProposalChange,
        base_revision: Option<String>,
        provenance: SettingProvenance,
        created_at: UtcMillis,
        expires_at: UtcMillis,
    ) -> Result<Self, AppError> {
        let payload = SettingProposalPayload {
            version: SETTING_PROPOSAL_PAYLOAD_VERSION,
            target,
            operation: change,
            base_revision,
            provenance,
            expires_at: expires_at.0,
        };
        Self::from_payload(id, payload, created_at, SettingProposalStatus::Pending)
    }

    fn from_payload(
        id: SettingProposalId,
        payload: SettingProposalPayload,
        created_at: UtcMillis,
        status: SettingProposalStatus,
    ) -> Result<Self, AppError> {
        if payload.version != SETTING_PROPOSAL_PAYLOAD_VERSION {
            return Err(AppError::new(
                "SETTING_PROPOSAL_UNSUPPORTED_VERSION",
                ErrorKind::Unsupported,
                "提案载荷版本不受支持",
                false,
            ));
        }
        if !payload.target.accepts(&payload.operation) {
            return Err(setting_proposal_invalid_target_error("目标与操作不匹配"));
        }
        if let Some(revision) = &payload.base_revision {
            validate_base_revision(revision)?;
        }
        payload.provenance.validate()?;
        if payload.expires_at <= created_at.0 {
            return Err(invalid_proposal(
                "SETTING_PROPOSAL_INVALID_EXPIRY",
                "提案过期时间必须晚于创建时间",
            ));
        }

        let canonical_json = canonical_json_of(&payload)?;
        let digest = canonical_digest(&canonical_json);
        Ok(Self {
            id,
            payload,
            created_at,
            status,
            canonical_json,
            digest,
        })
    }

    /// 只做完整性校验：载荷必须是 canonical 形式，digest 必须与载荷自洽。
    ///
    /// 供存储恢复与审计扫描共用。本方法不构造提案，也不做语义校验
    /// （版本、target/operation 匹配、provenance 安全性、有效期由 `from_stored` 负责）。
    pub fn verify_integrity(payload_json: &str, digest: &str) -> Result<(), AppError> {
        parse_canonical_payload(payload_json, digest).map(|_| ())
    }

    /// 从存储恢复并做完整性校验。
    ///
    /// - `payload_json` 必须是 canonical 形式：重新序列化后与存储字节一致，
    ///   否则视为被篡改（含键序、空白等一切重写）。
    /// - `digest` 必须等于 `payload_json` 的 SHA-256 小写十六进制。
    /// - 载荷本身仍要过一遍完整语义校验（版本、target/operation、provenance、有效期）。
    pub fn from_stored(
        id: SettingProposalId,
        payload_json: String,
        digest: String,
        status: SettingProposalStatus,
        created_at: UtcMillis,
    ) -> Result<Self, AppError> {
        let payload = parse_canonical_payload(&payload_json, &digest)?;
        Self::from_payload(id, payload, created_at, status)
    }

    pub fn id(&self) -> SettingProposalId {
        self.id
    }

    pub fn payload(&self) -> &SettingProposalPayload {
        &self.payload
    }

    pub fn version(&self) -> u32 {
        self.payload.version
    }

    pub fn target(&self) -> SettingTarget {
        self.payload.target
    }

    pub fn change(&self) -> &SettingProposalChange {
        &self.payload.operation
    }

    pub fn base_revision(&self) -> Option<&str> {
        self.payload.base_revision.as_deref()
    }

    pub fn provenance(&self) -> &SettingProvenance {
        &self.payload.provenance
    }

    pub fn created_at(&self) -> UtcMillis {
        self.created_at
    }

    pub fn expires_at(&self) -> UtcMillis {
        UtcMillis(self.payload.expires_at)
    }

    pub fn status(&self) -> SettingProposalStatus {
        self.status
    }

    /// canonical JSON 执行载荷（展示给用户的 digest 就是它的哈希）。
    pub fn canonical_json(&self) -> &str {
        &self.canonical_json
    }

    /// canonical JSON 的 SHA-256 小写十六进制。
    pub fn digest(&self) -> &str {
        &self.digest
    }

    /// 是否已过期（在给定时刻）。
    pub fn is_expired_at(&self, now: UtcMillis) -> bool {
        now.0 >= self.payload.expires_at
    }
}

/// 解析并校验存储中的执行载荷：必须是 canonical 形式，且 digest 与载荷自洽。
fn parse_canonical_payload(
    payload_json: &str,
    digest: &str,
) -> Result<SettingProposalPayload, AppError> {
    // 摘要形状先收敛：长度相等但含大写/非十六进制字符的摘要不是"格式不同"，
    // 而是伪造或损坏，必须和内容不符一样被拒绝。
    if !is_canonical_digest(digest) {
        return Err(setting_proposal_integrity_error(
            "提案摘要不是 SHA-256 小写十六进制",
        ));
    }
    let payload: SettingProposalPayload = serde_json::from_str(payload_json)
        .map_err(|_| setting_proposal_integrity_error("提案载荷无法解析"))?;
    if canonical_json_of(&payload)? != payload_json {
        return Err(setting_proposal_integrity_error(
            "提案载荷不是 canonical 形式",
        ));
    }
    if canonical_digest(payload_json) != digest {
        return Err(setting_proposal_integrity_error("提案摘要与载荷不一致"));
    }
    Ok(payload)
}

/// 一次成功应用设置的审计回执（append-only）。
///
/// before/after 保存的是**值**的 canonical JSON（Global 为整个 `SettingsValue`，
/// 资源偏好为 `PreferenceData`），因此回执可以跨进程稳定比较与重放。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SettingChangeReceipt {
    pub id: SettingChangeReceiptId,
    pub proposal_id: SettingProposalId,
    pub proposal_digest: String,
    pub target: SettingTarget,
    pub before_canonical_json: String,
    pub after_canonical_json: String,
    /// 应用后的 authoritative revision；`changed=false` 时为未改动的当前版本。
    pub applied_revision: Option<String>,
    pub changed: bool,
    pub provenance: SettingProvenance,
    pub applied_at: UtcMillis,
}

impl SettingChangeReceipt {
    /// 回执自身的一致性校验（供存储恢复使用）。
    ///
    /// 回执是**审计事实**：它必须能被独立重放，因此这里把"它自己声称的东西"全部
    /// 对齐，任何一项不自洽都视为行被改写：
    /// - digest 是 64 个 ASCII 小写十六进制字符；
    /// - provenance 通过安全短文本校验；
    /// - `changed` 与 `applied_revision` 互相印证：真正改动过就必然写出了新版本；
    /// - `before`/`after` 都落在 **target 对应的类型**上且是 canonical 形式
    ///   （Global 是整段 `SettingsValue` 且分区一致，资源级是 `PreferenceData`）；
    /// - `changed` 与"before/after 是否相同"一致：没写过库就不可能改变值，
    ///   写过库就不可能没有变化。
    pub fn validate(&self) -> Result<(), AppError> {
        if !is_canonical_digest(&self.proposal_digest) {
            return Err(setting_proposal_integrity_error(
                "回执摘要不是 SHA-256 小写十六进制",
            ));
        }
        self.provenance.validate()?;
        if let Some(revision) = &self.applied_revision {
            validate_base_revision(revision)?;
        }
        if self.changed && self.applied_revision.is_none() {
            return Err(setting_proposal_integrity_error(
                "回执声称已改动却没有应用后的版本号",
            ));
        }
        let before = canonical_receipt_value(self.target, &self.before_canonical_json)?;
        let after = canonical_receipt_value(self.target, &self.after_canonical_json)?;
        if self.changed != (before != after) {
            return Err(setting_proposal_integrity_error(
                "回执 changed 与 before/after 值不一致",
            ));
        }
        Ok(())
    }
}

/// 回执的 before/after 必须是**目标类型**的 canonical JSON。
///
/// Global 目标承载整段 `SettingsValue`（分区还要与 target 一致），Edition/MediaItem
/// 目标承载 `PreferenceData`。回执若落在别的类型上，就不再是一条可跨进程稳定比较
/// 与重放的重放记录——它描述的"改前/改后"根本不是该作用域的值。
fn canonical_receipt_value(target: SettingTarget, raw: &str) -> Result<String, AppError> {
    let canonical = match target {
        SettingTarget::Global(global) => {
            let value: SettingsValue = serde_json::from_str(raw)
                .map_err(|_| setting_proposal_integrity_error("回执设置值无法解析"))?;
            if value.section() != global.section {
                return Err(setting_proposal_integrity_error("回执设置分区与目标不一致"));
            }
            canonical_json_of(&value)?
        }
        SettingTarget::Edition(_) | SettingTarget::MediaItem(_) => {
            let value: PreferenceData = serde_json::from_str(raw)
                .map_err(|_| setting_proposal_integrity_error("回执资源内设无法解析"))?;
            canonical_json_of(&value)?
        }
    };
    if canonical != raw {
        return Err(setting_proposal_integrity_error(
            "回执数据不是 canonical 形式",
        ));
    }
    Ok(canonical)
}

// ---------- canonical JSON ----------

/// 递归按对象键的 UTF-8 字节序升序、紧凑输出的 canonical JSON。
///
/// 规则（稳定契约，改动即破坏所有历史 digest）：
/// 1. 对象键按字节序升序，不依赖语言/插入顺序；
/// 2. 无多余空白，UTF-8 直出（非 ASCII 不转义）；
/// 3. 字符串转义集合固定（`"`、`\`、控制字符）；
/// 4. 数组保持原顺序；
/// 5. 只承载整数、布尔、字符串、null、数组与对象（载荷中不含浮点）。
pub fn canonical_json_of<T: Serialize + ?Sized>(value: &T) -> Result<String, AppError> {
    let value = serde_json::to_value(value).map_err(|e| {
        AppError::new(
            "SETTING_PROPOSAL_SERIALIZE_FAILED",
            ErrorKind::Internal,
            "提案载荷序列化失败",
            false,
        )
        .with_source(e)
    })?;
    Ok(canonical_string(&value))
}

/// canonical JSON 的 SHA-256 小写十六进制摘要。
pub fn canonical_digest(canonical_json: &str) -> String {
    format!("{:x}", Sha256::digest(canonical_json.as_bytes()))
}

/// 摘要的严格形状：恰好 64 个 ASCII **小写**十六进制字符。
///
/// 只检查长度会放过 `"A".repeat(64)` 这类"长度对、内容不是摘要"的行；digest 是
/// 完整性判据本身，形状必须是闭合集合，任何偏离都按篡改处理。
pub fn is_canonical_digest(value: &str) -> bool {
    value.len() == SETTING_PROPOSAL_DIGEST_HEX_LEN
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

fn canonical_string(value: &serde_json::Value) -> String {
    let mut out = String::new();
    write_canonical(value, &mut out);
    out
}

fn write_canonical(value: &serde_json::Value, out: &mut String) {
    match value {
        serde_json::Value::Null => out.push_str("null"),
        serde_json::Value::Bool(true) => out.push_str("true"),
        serde_json::Value::Bool(false) => out.push_str("false"),
        serde_json::Value::Number(number) => out.push_str(&number.to_string()),
        serde_json::Value::String(text) => write_canonical_string(text, out),
        serde_json::Value::Array(items) => {
            out.push('[');
            for (index, item) in items.iter().enumerate() {
                if index > 0 {
                    out.push(',');
                }
                write_canonical(item, out);
            }
            out.push(']');
        }
        serde_json::Value::Object(map) => {
            let mut keys: Vec<&String> = map.keys().collect();
            keys.sort_unstable();
            out.push('{');
            for (index, key) in keys.iter().enumerate() {
                if index > 0 {
                    out.push(',');
                }
                write_canonical_string(key, out);
                out.push(':');
                write_canonical(&map[key.as_str()], out);
            }
            out.push('}');
        }
    }
}

fn write_canonical_string(text: &str, out: &mut String) {
    out.push('"');
    for ch in text.chars() {
        match ch {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            '\u{08}' => out.push_str("\\b"),
            '\u{0c}' => out.push_str("\\f"),
            c if (c as u32) < 0x20 => {
                out.push_str(&format!("\\u{:04x}", c as u32));
            }
            c => out.push(c),
        }
    }
    out.push('"');
}

// ---------- 稳定错误 ----------

fn invalid_proposal(code: &'static str, message: &'static str) -> AppError {
    AppError::new(code, ErrorKind::Validation, message, false)
}

fn invalid_provenance(message: impl Into<String>) -> AppError {
    AppError::new(
        "SETTING_PROPOSAL_INVALID_PROVENANCE",
        ErrorKind::Validation,
        message,
        false,
    )
}

/// 目标与操作不匹配（Global 非 SettingsPatch、Edition/MediaItem 非资源偏好、
/// section 不一致等）。
pub fn setting_proposal_invalid_target_error(detail: &'static str) -> AppError {
    AppError::new(
        "SETTING_PROPOSAL_INVALID_TARGET",
        ErrorKind::Validation,
        format!("提案目标非法：{detail}"),
        false,
    )
}

/// 存储中的提案行被篡改或损坏。
pub fn setting_proposal_integrity_error(detail: &'static str) -> AppError {
    AppError::new(
        "SETTING_PROPOSAL_INTEGRITY_MISMATCH",
        ErrorKind::Internal,
        format!("提案数据完整性校验失败：{detail}"),
        false,
    )
}

pub fn setting_proposal_not_found_error() -> AppError {
    AppError::new(
        "SETTING_PROPOSAL_NOT_FOUND",
        ErrorKind::NotFound,
        "设置提案不存在",
        false,
    )
}

/// 调用方展示给用户的 digest 与存储不一致：拒绝执行。
pub fn setting_proposal_digest_mismatch_error() -> AppError {
    AppError::new(
        "SETTING_PROPOSAL_DIGEST_MISMATCH",
        ErrorKind::Conflict,
        "提案摘要与确认内容不一致，已拒绝执行",
        false,
    )
}

pub fn setting_proposal_expired_error() -> AppError {
    AppError::new(
        "SETTING_PROPOSAL_EXPIRED",
        ErrorKind::Conflict,
        "提案已过期，未执行任何修改",
        false,
    )
}

pub fn setting_proposal_rejected_error() -> AppError {
    AppError::new(
        "SETTING_PROPOSAL_REJECTED",
        ErrorKind::Conflict,
        "提案已被拒绝，未执行任何修改",
        false,
    )
}

/// 提案不处于 pending（例如已被拒绝或过期）。与 `REVISION_CONFLICT` 区分：
/// 这是提案自身状态的问题，不是目标数据被并发修改。
pub fn setting_proposal_not_pending_error(status: SettingProposalStatus) -> AppError {
    AppError::new(
        "SETTING_PROPOSAL_NOT_PENDING",
        ErrorKind::Conflict,
        format!("提案状态为 {}，不能执行", status.as_str()),
        false,
    )
}

/// 目标数据已被其他会话修改（base_revision 过期或 CAS 未命中）。
pub fn setting_proposal_revision_conflict_error() -> AppError {
    AppError::new(
        "REVISION_CONFLICT",
        ErrorKind::Conflict,
        "目标设置已被其他窗口/请求更新，请重新发起提案",
        false,
    )
}

/// 目标实体不存在（例如媒体条目已被删除）。
pub fn setting_proposal_target_not_found_error(detail: &'static str) -> AppError {
    AppError::new(
        "SETTING_PROPOSAL_TARGET_NOT_FOUND",
        ErrorKind::NotFound,
        format!("提案目标不存在：{detail}"),
        false,
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::settings::{ComicPatch, ReadingFontSize, ReadingPatch};

    fn provenance() -> SettingProvenance {
        SettingProvenance::agent(ProvenanceReason::AgentSuggestion)
            .with_source_ref("settings-page")
            .unwrap()
    }

    fn global_change() -> SettingProposalChange {
        let patch: SettingsPatch =
            serde_json::from_str(r#"{"section":"reading","fontSize":"large"}"#).unwrap();
        SettingProposalChange::SettingsPatch(patch)
    }

    fn global_proposal() -> SettingProposal {
        SettingProposal::new(
            SettingProposalId::new(),
            SettingTarget::global(SettingsSection::Reading),
            global_change(),
            Some("set-0000000000000001-2a".into()),
            provenance(),
            UtcMillis(1_000),
            UtcMillis(2_000),
        )
        .unwrap()
    }

    fn preference_change() -> SettingProposalChange {
        SettingProposalChange::ResourcePreference(PreferenceData {
            reading: Some(ReadingPatch {
                font_size: Some(ReadingFontSize::Large),
                ..ReadingPatch::default()
            }),
            comic: Some(ComicPatch::default()),
        })
    }

    #[test]
    fn canonical_json_sorts_keys_and_is_compact() {
        let proposal = global_proposal();
        let json = proposal.canonical_json();
        assert!(!json.contains(' '), "canonical JSON 必须紧凑：{json}");
        assert!(!json.contains('\n'), "canonical JSON 不能有换行：{json}");
        // 顶层键按字节序：baseRevision < expiresAt < operation < provenance < target < version
        let positions: Vec<usize> = [
            "\"baseRevision\"",
            "\"expiresAt\"",
            "\"operation\"",
            "\"provenance\"",
            "\"target\"",
            "\"version\"",
        ]
        .iter()
        .map(|key| {
            json.find(key)
                .unwrap_or_else(|| panic!("缺少 {key}：{json}"))
        })
        .collect();
        assert!(
            positions.windows(2).all(|pair| pair[0] < pair[1]),
            "顶层键必须按键序升序：{json}"
        );
        // 嵌套对象同样排序（settings_patch 被展平到 operation 内）。
        let font_size = json.find("\"fontSize\"").unwrap();
        let kind = json.find("\"kind\"").unwrap();
        let section = json.find("\"section\"").unwrap();
        assert!(
            font_size < kind && kind < section,
            "嵌套键也必须排序：{json}"
        );
    }

    #[test]
    fn canonical_json_is_independent_of_input_key_order() {
        // 同一语义载荷的不同书写顺序，重算后得到同一 canonical 字节串。
        let ordered = concat!(
            r#"{"version":1,"target":{"scope":"global","section":"reading"},"#,
            r#""operation":{"kind":"resource_preference","reading":null,"comic":null},"#,
            r#""baseRevision":null,"#,
            r#""provenance":{"sourceKind":"user","actorKind":"user","reason":"user_request"},"#,
            r#""expiresAt":7}"#
        );
        let shuffled = concat!(
            r#"{"expiresAt":7,"#,
            r#""provenance":{"reason":"user_request","actorKind":"user","sourceKind":"user"},"#,
            r#""baseRevision":null,"operation":{"comic":null,"reading":null,"kind":"resource_preference"},"#,
            r#""target":{"section":"reading","scope":"global"},"version":1}"#
        );
        let left: SettingProposalPayload = serde_json::from_str(ordered).unwrap();
        let right: SettingProposalPayload = serde_json::from_str(shuffled).unwrap();
        assert_eq!(
            canonical_json_of(&left).unwrap(),
            canonical_json_of(&right).unwrap()
        );
    }

    #[test]
    fn canonical_json_preserves_array_order_and_utf8() {
        let value = serde_json::json!({
            "b": ["二", "一", 3],
            "a": "中文直接输出",
        });
        assert_eq!(
            canonical_string(&value),
            r#"{"a":"中文直接输出","b":["二","一",3]}"#
        );
    }

    #[test]
    fn digest_covers_target_operation_data_base_revision_provenance_and_expiry() {
        let baseline = global_proposal();
        let digest = baseline.digest().to_owned();
        assert_eq!(digest.len(), SETTING_PROPOSAL_DIGEST_HEX_LEN);
        assert!(
            digest
                .chars()
                .all(|c| c.is_ascii_hexdigit() && !c.is_uppercase())
        );

        // id / created_at / status 不进载荷：同载荷换 id 与创建时间，digest 不变。
        let same_payload = SettingProposal::new(
            SettingProposalId::new(),
            SettingTarget::global(SettingsSection::Reading),
            global_change(),
            Some("set-0000000000000001-2a".into()),
            provenance(),
            UtcMillis(1_500),
            UtcMillis(2_000),
        )
        .unwrap();
        assert_eq!(
            same_payload.digest(),
            digest,
            "id/created_at 不得进入 digest"
        );

        // 过期时间、base_revision、目标、数据、provenance 任何一项变化都改 digest。
        let later = SettingProposal::new(
            SettingProposalId::new(),
            SettingTarget::global(SettingsSection::Reading),
            global_change(),
            Some("set-0000000000000001-2a".into()),
            provenance(),
            UtcMillis(1_000),
            UtcMillis(3_000),
        )
        .unwrap();
        assert_ne!(later.digest(), digest, "expiry 必须进入 digest");

        let other_base = SettingProposal::new(
            SettingProposalId::new(),
            SettingTarget::global(SettingsSection::Reading),
            global_change(),
            None,
            provenance(),
            UtcMillis(1_000),
            UtcMillis(2_000),
        )
        .unwrap();
        assert_ne!(other_base.digest(), digest, "base_revision 必须进入 digest");

        let other_provenance = SettingProposal::new(
            SettingProposalId::new(),
            SettingTarget::global(SettingsSection::Reading),
            global_change(),
            Some("set-0000000000000001-2a".into()),
            SettingProvenance::user(ProvenanceReason::UserRequest),
            UtcMillis(1_000),
            UtcMillis(2_000),
        )
        .unwrap();
        assert_ne!(
            other_provenance.digest(),
            digest,
            "provenance 必须进入 digest"
        );

        let other_patch: SettingsPatch =
            serde_json::from_str(r#"{"section":"reading","fontSize":"small"}"#).unwrap();
        let other_change = SettingProposal::new(
            SettingProposalId::new(),
            SettingTarget::global(SettingsSection::Reading),
            SettingProposalChange::SettingsPatch(other_patch),
            Some("set-0000000000000001-2a".into()),
            provenance(),
            UtcMillis(1_000),
            UtcMillis(2_000),
        )
        .unwrap();
        assert_ne!(other_change.digest(), digest, "patch 数据必须进入 digest");

        let other_target = SettingProposal::new(
            SettingProposalId::new(),
            SettingTarget::edition(EditionId::new()),
            preference_change(),
            Some("set-0000000000000001-2a".into()),
            provenance(),
            UtcMillis(1_000),
            UtcMillis(2_000),
        )
        .unwrap();
        assert_ne!(other_target.digest(), digest, "target 必须进入 digest");
    }

    #[test]
    fn from_stored_roundtrips_and_verifies_integrity() {
        let proposal = global_proposal();
        let restored = SettingProposal::from_stored(
            proposal.id(),
            proposal.canonical_json().to_owned(),
            proposal.digest().to_owned(),
            SettingProposalStatus::Pending,
            proposal.created_at(),
        )
        .unwrap();
        assert_eq!(restored, proposal);
        assert_eq!(restored.target(), proposal.target());
        assert_eq!(restored.change(), proposal.change());
        assert_eq!(restored.base_revision(), proposal.base_revision());
        assert_eq!(restored.expires_at(), proposal.expires_at());
        assert_eq!(restored.version(), SETTING_PROPOSAL_PAYLOAD_VERSION);

        // applied 状态不改变载荷与 digest。
        let applied = SettingProposal::from_stored(
            proposal.id(),
            proposal.canonical_json().to_owned(),
            proposal.digest().to_owned(),
            SettingProposalStatus::Applied,
            proposal.created_at(),
        )
        .unwrap();
        assert_eq!(applied.digest(), proposal.digest());
        assert_eq!(applied.status(), SettingProposalStatus::Applied);
    }

    #[test]
    fn verify_integrity_checks_a_stored_row_without_constructing_a_proposal() {
        let proposal = global_proposal();
        SettingProposal::verify_integrity(proposal.canonical_json(), proposal.digest()).unwrap();

        let mut digest = proposal.digest().to_owned();
        digest.replace_range(0..1, if digest.starts_with('0') { "1" } else { "0" });
        assert_eq!(
            SettingProposal::verify_integrity(proposal.canonical_json(), &digest)
                .unwrap_err()
                .code()
                .as_str(),
            "SETTING_PROPOSAL_INTEGRITY_MISMATCH"
        );

        // 载荷被改写但仍是 canonical 形式：只有摘要对不上。
        let tampered = proposal
            .canonical_json()
            .replace("\"fontSize\":\"large\"", "\"fontSize\":\"small\"");
        assert_eq!(
            SettingProposal::verify_integrity(&tampered, proposal.digest())
                .unwrap_err()
                .code()
                .as_str(),
            "SETTING_PROPOSAL_INTEGRITY_MISMATCH"
        );

        // 非法 JSON 与非法形式都返回稳定错误，不 panic。
        assert!(SettingProposal::verify_integrity("{", "x").is_err());
        let padded = format!(" {}", proposal.canonical_json());
        assert_eq!(
            SettingProposal::verify_integrity(&padded, &canonical_digest(&padded))
                .unwrap_err()
                .code()
                .as_str(),
            "SETTING_PROPOSAL_INTEGRITY_MISMATCH"
        );

        // 校验层不做语义判断：self-consistent 但 target/operation 不匹配的行
        // 由 `from_stored` 拒绝。
        let mismatched = concat!(
            r#"{"baseRevision":null,"expiresAt":2000,"#,
            r#""operation":{"kind":"settings_patch","section":"reading"},"#,
            r#""provenance":{"actorKind":"agent","reason":"agent_suggestion","sourceKind":"agent"},"#,
            r#""target":{"scope":"edition","editionId":"0196f0d2-0000-7000-8000-000000000001"},"version":1}"#
        );
        let payload: SettingProposalPayload = serde_json::from_str(mismatched).unwrap();
        let canonical = canonical_json_of(&payload).unwrap();
        let canonical_hash = canonical_digest(&canonical);
        SettingProposal::verify_integrity(&canonical, &canonical_hash).unwrap();
        assert_eq!(
            SettingProposal::from_stored(
                SettingProposalId::new(),
                canonical,
                canonical_hash,
                SettingProposalStatus::Pending,
                UtcMillis(1_000),
            )
            .unwrap_err()
            .code()
            .as_str(),
            "SETTING_PROPOSAL_INVALID_TARGET"
        );
    }

    #[test]
    fn tampered_canonical_json_is_rejected() {
        let proposal = global_proposal();
        let tampered = proposal
            .canonical_json()
            .replace("\"fontSize\":\"large\"", "\"fontSize\":\"small\"");
        assert_ne!(tampered, proposal.canonical_json());
        let error = SettingProposal::from_stored(
            proposal.id(),
            tampered,
            proposal.digest().to_owned(),
            SettingProposalStatus::Pending,
            proposal.created_at(),
        )
        .unwrap_err();
        assert_eq!(error.code().as_str(), "SETTING_PROPOSAL_INTEGRITY_MISMATCH");
    }

    #[test]
    fn tampered_digest_is_rejected() {
        let proposal = global_proposal();
        let mut digest = proposal.digest().to_owned();
        digest.replace_range(0..1, if digest.starts_with('0') { "1" } else { "0" });
        let error = SettingProposal::from_stored(
            proposal.id(),
            proposal.canonical_json().to_owned(),
            digest,
            SettingProposalStatus::Pending,
            proposal.created_at(),
        )
        .unwrap_err();
        assert_eq!(error.code().as_str(), "SETTING_PROPOSAL_INTEGRITY_MISMATCH");
    }

    /// digest 的形状是闭合集合：长度对但不是 64 位 ASCII 小写十六进制，一律按篡改处理。
    ///
    /// 只校验长度会放过 `"A".repeat(64)` 这类"看起来像摘要"的行；digest 本身就是
    /// 完整性判据，因此恢复路径与只读校验路径都必须给出同一个稳定错误。
    #[test]
    fn proposal_digest_shape_is_closed_lowercase_hex() {
        let proposal = global_proposal();
        for bad in [
            // 大写十六进制：长度对、字符也是十六进制，但不是本系统写出的摘要。
            "A".repeat(SETTING_PROPOSAL_DIGEST_HEX_LEN),
            // 非十六进制字符。
            "g".repeat(SETTING_PROPOSAL_DIGEST_HEX_LEN),
            // 长度差一位。
            "0".repeat(SETTING_PROPOSAL_DIGEST_HEX_LEN - 1),
        ] {
            assert!(!is_canonical_digest(&bad), "非法摘要形状：{bad:?}");
            let restored_error = SettingProposal::from_stored(
                proposal.id(),
                proposal.canonical_json().to_owned(),
                bad.clone(),
                SettingProposalStatus::Pending,
                proposal.created_at(),
            )
            .unwrap_err();
            let verified_error =
                SettingProposal::verify_integrity(proposal.canonical_json(), &bad).unwrap_err();
            for error in [restored_error, verified_error] {
                assert_eq!(
                    error.code().as_str(),
                    "SETTING_PROPOSAL_INTEGRITY_MISMATCH",
                    "非法摘要形状必须按篡改处理：{bad:?}"
                );
            }
        }
        assert!(is_canonical_digest(proposal.digest()));
    }

    #[test]
    fn non_canonical_payload_json_is_rejected() {
        let proposal = global_proposal();

        // 合法 JSON 但含多余空白：即使 digest 正确也必须拒绝。
        let padded = format!(" {}", proposal.canonical_json());
        let error = SettingProposal::from_stored(
            proposal.id(),
            padded.clone(),
            canonical_digest(&padded),
            SettingProposalStatus::Pending,
            proposal.created_at(),
        )
        .unwrap_err();
        assert_eq!(error.code().as_str(), "SETTING_PROPOSAL_INTEGRITY_MISMATCH");

        // 键序未排序：digest 与内容自洽也仍然拒绝（存储必须是 canonical）。
        let unsorted = concat!(
            r#"{"version":1,"target":{"scope":"global","section":"reading"},"#,
            r#""operation":{"kind":"settings_patch","section":"reading","fontSize":"large"},"#,
            r#""baseRevision":"set-0000000000000001-2a","#,
            r#""provenance":{"sourceKind":"agent","actorKind":"agent","reason":"agent_suggestion","sourceRef":"settings-page"},"#,
            r#""expiresAt":2000}"#
        );
        assert_ne!(unsorted, proposal.canonical_json());
        let error = SettingProposal::from_stored(
            proposal.id(),
            unsorted.to_owned(),
            canonical_digest(unsorted),
            SettingProposalStatus::Pending,
            proposal.created_at(),
        )
        .unwrap_err();
        assert_eq!(error.code().as_str(), "SETTING_PROPOSAL_INTEGRITY_MISMATCH");
    }

    #[test]
    fn unsupported_payload_version_is_rejected() {
        let json = global_proposal()
            .canonical_json()
            .replace("\"version\":1", "\"version\":2");
        let error = SettingProposal::from_stored(
            SettingProposalId::new(),
            json.clone(),
            canonical_digest(&json),
            SettingProposalStatus::Pending,
            UtcMillis(1_000),
        )
        .unwrap_err();
        assert_eq!(
            error.code().as_str(),
            "SETTING_PROPOSAL_UNSUPPORTED_VERSION"
        );
    }

    #[test]
    fn global_target_requires_matching_section() {
        let patch: SettingsPatch =
            serde_json::from_str(r#"{"section":"comic","viewMode":"double"}"#).unwrap();
        let error = SettingProposal::new(
            SettingProposalId::new(),
            SettingTarget::global(SettingsSection::Reading),
            SettingProposalChange::SettingsPatch(patch),
            None,
            provenance(),
            UtcMillis(1_000),
            UtcMillis(2_000),
        )
        .unwrap_err();
        assert_eq!(error.code().as_str(), "SETTING_PROPOSAL_INVALID_TARGET");
    }

    #[test]
    fn target_and_change_must_match_structurally() {
        let edition = EditionId::new();
        let media_item = MediaItemId::new();

        // Global 拒绝资源偏好数据。
        let error = SettingProposal::new(
            SettingProposalId::new(),
            SettingTarget::global(SettingsSection::Reading),
            preference_change(),
            None,
            provenance(),
            UtcMillis(1_000),
            UtcMillis(2_000),
        )
        .unwrap_err();
        assert_eq!(error.code().as_str(), "SETTING_PROPOSAL_INVALID_TARGET");

        // Edition / MediaItem 拒绝全局 SettingsPatch。
        for target in [
            SettingTarget::edition(edition),
            SettingTarget::media_item(edition, media_item),
        ] {
            let error = SettingProposal::new(
                SettingProposalId::new(),
                target,
                global_change(),
                None,
                provenance(),
                UtcMillis(1_000),
                UtcMillis(2_000),
            )
            .unwrap_err();
            assert_eq!(error.code().as_str(), "SETTING_PROPOSAL_INVALID_TARGET");
            assert!(!target.accepts(&global_change()));
        }

        // 资源级目标接受 PreferenceData。
        assert!(SettingTarget::edition(edition).accepts(&preference_change()));
        assert!(SettingTarget::media_item(edition, media_item).accepts(&preference_change()));
        assert!(SettingTarget::global(SettingsSection::Reading).accepts(&global_change()));

        assert_eq!(SettingTarget::edition(edition).edition_id(), Some(edition));
        assert_eq!(SettingTarget::edition(edition).media_item_id(), None);
        assert_eq!(
            SettingTarget::media_item(edition, media_item).media_item_id(),
            Some(media_item)
        );
        assert_eq!(
            SettingTarget::media_item(edition, media_item).edition_id(),
            Some(edition)
        );
        assert_eq!(SettingTarget::edition(edition).scope_str(), "edition");
        assert_eq!(
            SettingTarget::media_item(edition, media_item).scope_str(),
            "media_item"
        );
        assert_eq!(
            SettingTarget::global(SettingsSection::Comic).section(),
            Some(SettingsSection::Comic)
        );
        assert_eq!(SettingTarget::edition(edition).section(), None);
    }

    #[test]
    fn target_serializes_with_scope_tag_and_camel_case_fields() {
        let json = serde_json::to_string(&SettingTarget::global(SettingsSection::Reading)).unwrap();
        assert_eq!(json, r#"{"scope":"global","section":"reading"}"#);
        let json = serde_json::to_string(&SettingTarget::media_item(
            EditionId::new(),
            MediaItemId::new(),
        ))
        .unwrap();
        assert!(json.contains(r#""scope":"media_item""#), "{json}");
        assert!(json.contains(r#""editionId":"#), "{json}");
        assert!(json.contains(r#""mediaItemId":"#), "{json}");
        assert_eq!(
            serde_json::from_str::<SettingTarget>(&json).unwrap(),
            SettingTarget::media_item(
                serde_json::from_str::<SettingTarget>(&json)
                    .unwrap()
                    .edition_id()
                    .unwrap(),
                serde_json::from_str::<SettingTarget>(&json)
                    .unwrap()
                    .media_item_id()
                    .unwrap(),
            )
        );
    }

    #[test]
    fn unknown_fields_are_rejected_in_target_change_and_provenance() {
        let error = serde_json::from_str::<SettingTarget>(
            r#"{"scope":"global","section":"reading","bogus":1}"#,
        );
        assert!(error.is_err(), "target 未知字段必须拒绝");

        let error = serde_json::from_str::<SettingTarget>(r#"{"scope":"bogus"}"#);
        assert!(error.is_err(), "未知 scope 必须拒绝");

        let error = serde_json::from_str::<SettingTarget>(
            r#"{"scope":"edition","editionId":"0196f0d2-0000-7000-8000-000000000001","section":"reading"}"#,
        );
        assert!(error.is_err(), "Edition 目标不允许携带 section");

        let error = serde_json::from_str::<SettingProposalChange>(
            r#"{"kind":"settings_patch","section":"reading","fontSize":"large","extra":true}"#,
        );
        assert!(error.is_err(), "change 未知字段必须拒绝");

        let error = serde_json::from_str::<SettingProposalChange>(
            r#"{"kind":"settings_patch","section":"reading","expectedRevision":"x"}"#,
        );
        assert!(error.is_err(), "patch 内部未知字段必须拒绝");

        let error =
            serde_json::from_str::<SettingProposalChange>(r#"{"kind":"raw_map","data":{"a":1}}"#);
        assert!(error.is_err(), "任意 JSON Map 写入模型必须被拒绝");

        let error = serde_json::from_str::<SettingProposalChange>(r#"{"kind":"bogus"}"#);
        assert!(error.is_err(), "未知 kind 必须拒绝");

        let error = serde_json::from_str::<SettingProvenance>(
            r#"{"sourceKind":"user","actorKind":"user","reason":"user_request","path":"D:/x"}"#,
        );
        assert!(error.is_err(), "provenance 未知字段必须拒绝");

        let error = serde_json::from_str::<SettingProvenance>(
            r#"{"sourceKind":"user","actorKind":"user","reason":"because_i_said_so"}"#,
        );
        assert!(error.is_err(), "未知 reason code 必须拒绝");

        let error = serde_json::from_str::<SettingProposalPayload>(
            r#"{"version":1,"target":{"scope":"global","section":"reading"},"operation":{"kind":"resource_preference"},"baseRevision":null,"provenance":{"sourceKind":"user","actorKind":"user","reason":"user_request"},"expiresAt":1,"extra":1}"#,
        );
        assert!(error.is_err(), "载荷未知字段必须拒绝");
    }

    #[test]
    fn provenance_rejects_secrets_paths_and_sql() {
        let base = || SettingProvenance::agent(ProvenanceReason::AgentSuggestion);
        for bad in [
            "D:/Users/me/secret.txt",
            "/etc/passwd",
            "C:\\Users\\me",
            "SELECT * FROM settings",
            "drop table settings",
            "apiKey=abcd",
            "Authorization: Bearer x",
            "user; delete",
            "../../escape",
            "line\nbreak",
        ] {
            assert!(
                base().with_source_ref(bad).is_err(),
                "provenance source_ref 必须拒绝：{bad}"
            );
        }
        assert!(base().with_source_ref("").is_err());
        assert!(base().with_source_ref("   ").is_err());
        assert!(base().with_source_ref("x".repeat(65)).is_err());
        assert!(base().with_summary("x".repeat(201)).is_err());
        // 安全短文本（含中文）可用。
        assert!(base().with_source_ref("设置页").is_ok());
        assert!(
            base()
                .with_summary("根据你最近调大的阅读字号给出建议")
                .is_ok()
        );

        // 直接构造出的非法 provenance 也必须在校验时被拦截。
        let unsafe_provenance = SettingProvenance {
            source_ref: Some("D:/secret".into()),
            ..base()
        };
        assert_eq!(
            unsafe_provenance.validate().unwrap_err().code().as_str(),
            "SETTING_PROPOSAL_INVALID_PROVENANCE"
        );
    }

    #[test]
    fn provenance_safety_is_rechecked_on_from_stored() {
        // 构造一段 provenance 非法、但自身自洽的 canonical 载荷（模拟被写坏的存储行）。
        let json = concat!(
            r#"{"baseRevision":null,"expiresAt":2000,"#,
            r#""operation":{"kind":"settings_patch","section":"reading"},"#,
            r#""provenance":{"actorKind":"agent","reason":"agent_suggestion","sourceKind":"agent","sourceRef":"D:/secret"},"#,
            r#""target":{"scope":"global","section":"reading"},"version":1}"#
        );
        let payload: SettingProposalPayload = serde_json::from_str(json).unwrap();
        let canonical = canonical_json_of(&payload).unwrap();
        let error = SettingProposal::from_stored(
            SettingProposalId::new(),
            canonical.clone(),
            canonical_digest(&canonical),
            SettingProposalStatus::Pending,
            UtcMillis(1_000),
        )
        .unwrap_err();
        assert_eq!(error.code().as_str(), "SETTING_PROPOSAL_INVALID_PROVENANCE");

        // 恢复路径同样拒绝 target/operation 不匹配的行。
        let mismatched = concat!(
            r#"{"baseRevision":null,"expiresAt":2000,"#,
            r#""operation":{"kind":"settings_patch","section":"reading"},"#,
            r#""provenance":{"actorKind":"agent","reason":"agent_suggestion","sourceKind":"agent"},"#,
            r#""target":{"scope":"edition","editionId":"0196f0d2-0000-7000-8000-000000000001"},"version":1}"#
        );
        let payload: SettingProposalPayload = serde_json::from_str(mismatched).unwrap();
        let canonical = canonical_json_of(&payload).unwrap();
        let error = SettingProposal::from_stored(
            SettingProposalId::new(),
            canonical.clone(),
            canonical_digest(&canonical),
            SettingProposalStatus::Pending,
            UtcMillis(1_000),
        )
        .unwrap_err();
        assert_eq!(error.code().as_str(), "SETTING_PROPOSAL_INVALID_TARGET");
    }

    #[test]
    fn base_revision_must_be_an_opaque_token() {
        for bad in ["has space", "semi;colon", "D:/path", "quote'"] {
            let error = SettingProposal::new(
                SettingProposalId::new(),
                SettingTarget::edition(EditionId::new()),
                preference_change(),
                Some(bad.to_owned()),
                provenance(),
                UtcMillis(1_000),
                UtcMillis(2_000),
            )
            .unwrap_err();
            assert_eq!(
                error.code().as_str(),
                "SETTING_PROPOSAL_INVALID_BASE_REVISION"
            );
        }
        assert!(
            SettingProposal::new(
                SettingProposalId::new(),
                SettingTarget::edition(EditionId::new()),
                preference_change(),
                Some("pref-edition-0000000000000001-2a".into()),
                provenance(),
                UtcMillis(1_000),
                UtcMillis(2_000),
            )
            .is_ok()
        );
    }

    #[test]
    fn expiry_must_follow_creation() {
        let error = SettingProposal::new(
            SettingProposalId::new(),
            SettingTarget::global(SettingsSection::Reading),
            global_change(),
            None,
            provenance(),
            UtcMillis(2_000),
            UtcMillis(2_000),
        )
        .unwrap_err();
        assert_eq!(error.code().as_str(), "SETTING_PROPOSAL_INVALID_EXPIRY");
    }

    #[test]
    fn status_parsing_is_closed() {
        for status in [
            SettingProposalStatus::Pending,
            SettingProposalStatus::Applied,
            SettingProposalStatus::Rejected,
            SettingProposalStatus::Expired,
        ] {
            assert_eq!(SettingProposalStatus::parse(status.as_str()), Some(status));
        }
        assert_eq!(SettingProposalStatus::parse("bogus"), None);
        assert_eq!(SettingProposalStatus::parse(""), None);
        assert!(!SettingProposalStatus::Pending.is_terminal());
        assert!(SettingProposalStatus::Applied.is_terminal());
        assert!(SettingProposalStatus::Rejected.is_terminal());
        assert!(SettingProposalStatus::Expired.is_terminal());
    }

    /// Global 目标的回执承载整段 `SettingsValue`（不是资源级 `PreferenceData`）。
    fn reading_value_json() -> String {
        canonical_json_of(&SettingsValue::default_for(SettingsSection::Reading)).unwrap()
    }

    fn receipt_with_changed(changed: bool) -> SettingChangeReceipt {
        let before = reading_value_json();
        let after = if changed {
            let patch: SettingsPatch =
                serde_json::from_str(r#"{"section":"reading","fontSize":"large"}"#).unwrap();
            canonical_json_of(
                &patch.apply_to(&serde_json::from_str::<SettingsValue>(&before).unwrap()),
            )
            .unwrap()
        } else {
            before.clone()
        };
        SettingChangeReceipt {
            id: SettingChangeReceiptId::new(),
            proposal_id: SettingProposalId::new(),
            proposal_digest: global_proposal().digest().to_owned(),
            target: SettingTarget::global(SettingsSection::Reading),
            before_canonical_json: before,
            after_canonical_json: after,
            applied_revision: Some("set-0000000000000001-2a".into()),
            changed,
            provenance: provenance(),
            applied_at: UtcMillis(1_500),
        }
    }

    #[test]
    fn receipt_validation_rejects_non_canonical_and_unsafe_rows() {
        let receipt = receipt_with_changed(false);
        receipt.validate().unwrap();

        let tampered = SettingChangeReceipt {
            before_canonical_json: format!(" {}", receipt.before_canonical_json),
            ..receipt.clone()
        };
        assert_eq!(
            tampered.validate().unwrap_err().code().as_str(),
            "SETTING_PROPOSAL_INTEGRITY_MISMATCH"
        );

        let short_digest = SettingChangeReceipt {
            proposal_digest: "abc".into(),
            ..receipt.clone()
        };
        assert_eq!(
            short_digest.validate().unwrap_err().code().as_str(),
            "SETTING_PROPOSAL_INTEGRITY_MISMATCH"
        );

        // applied_revision 也必须是不透明 token：回执要能跨进程重放，带路径或
        // 空格的"版本号"本身就说明这一行不是本系统写出来的。
        let bad_revision = SettingChangeReceipt {
            applied_revision: Some("D:/path".into()),
            ..receipt.clone()
        };
        assert_eq!(
            bad_revision.validate().unwrap_err().code().as_str(),
            "SETTING_PROPOSAL_INVALID_BASE_REVISION"
        );

        let unsafe_provenance = SettingChangeReceipt {
            provenance: SettingProvenance {
                source_ref: Some("D:/secret".into()),
                ..provenance()
            },
            ..receipt
        };
        assert_eq!(
            unsafe_provenance.validate().unwrap_err().code().as_str(),
            "SETTING_PROPOSAL_INVALID_PROVENANCE"
        );
    }

    #[test]
    fn receipt_digest_shape_is_closed_lowercase_hex() {
        let receipt = receipt_with_changed(false);
        // 长度对、但不是十六进制/不是小写：必须和"内容不符"一样被拒绝，
        // 而不是被当成"另一种写法"放过。
        for bad in [
            "A".repeat(SETTING_PROPOSAL_DIGEST_HEX_LEN),
            "g".repeat(SETTING_PROPOSAL_DIGEST_HEX_LEN),
            "0".repeat(SETTING_PROPOSAL_DIGEST_HEX_LEN - 1),
            "0".repeat(SETTING_PROPOSAL_DIGEST_HEX_LEN + 1),
            format!("{} ", "0".repeat(SETTING_PROPOSAL_DIGEST_HEX_LEN - 1)),
        ] {
            let tampered = SettingChangeReceipt {
                proposal_digest: bad.clone(),
                ..receipt.clone()
            };
            assert_eq!(
                tampered.validate().unwrap_err().code().as_str(),
                "SETTING_PROPOSAL_INTEGRITY_MISMATCH",
                "非法摘要形状必须拒绝：{bad:?}"
            );
        }
        for good in ["0".repeat(64), global_proposal().digest().to_owned()] {
            assert!(
                SettingChangeReceipt {
                    proposal_digest: good.clone(),
                    ..receipt.clone()
                }
                .validate()
                .is_ok(),
                "合法摘要必须接受：{good}"
            );
        }
        assert!(is_canonical_digest(&canonical_digest("{}")));
        assert!(!is_canonical_digest(""));
        assert!(!is_canonical_digest(&"F".repeat(64)));
    }

    #[test]
    fn receipt_changed_must_agree_with_revision_and_values() {
        // 声称改动过却没有新版本号：不可能——写库必然写出新 revision。
        let no_revision = SettingChangeReceipt {
            applied_revision: None,
            ..receipt_with_changed(true)
        };
        assert_eq!(
            no_revision.validate().unwrap_err().code().as_str(),
            "SETTING_PROPOSAL_INTEGRITY_MISMATCH"
        );

        // changed 与 before/after 必须一致：没写过库就不可能改变值。
        let lying_changed = SettingChangeReceipt {
            changed: true,
            after_canonical_json: reading_value_json(),
            ..receipt_with_changed(false)
        };
        assert_eq!(
            lying_changed.validate().unwrap_err().code().as_str(),
            "SETTING_PROPOSAL_INTEGRITY_MISMATCH"
        );

        let lying_unchanged = SettingChangeReceipt {
            changed: false,
            ..receipt_with_changed(true)
        };
        assert_eq!(
            lying_unchanged.validate().unwrap_err().code().as_str(),
            "SETTING_PROPOSAL_INTEGRITY_MISMATCH"
        );

        // 从未持久化的目标 + 空 patch：before/after 相同且没有版本号，合法。
        assert!(
            SettingChangeReceipt {
                applied_revision: None,
                ..receipt_with_changed(false)
            }
            .validate()
            .is_ok()
        );
    }

    #[test]
    fn receipt_values_must_match_the_target_scope() {
        let preference = canonical_json_of(&PreferenceData::default()).unwrap();

        // 资源级数据放进 Global 目标的回执：类型不符。
        let wrong_type = SettingChangeReceipt {
            before_canonical_json: preference.clone(),
            after_canonical_json: preference.clone(),
            ..receipt_with_changed(false)
        };
        assert_eq!(
            wrong_type.validate().unwrap_err().code().as_str(),
            "SETTING_PROPOSAL_INTEGRITY_MISMATCH"
        );

        // 分区与目标不一致的 SettingsValue 同样是错行。
        let other_section =
            canonical_json_of(&SettingsValue::default_for(SettingsSection::Comic)).unwrap();
        let wrong_section = SettingChangeReceipt {
            before_canonical_json: other_section.clone(),
            after_canonical_json: other_section,
            ..receipt_with_changed(false)
        };
        assert_eq!(
            wrong_section.validate().unwrap_err().code().as_str(),
            "SETTING_PROPOSAL_INTEGRITY_MISMATCH"
        );

        // Global 的 SettingsValue 放进资源级目标：同样拒绝。
        let edition = EditionId::new();
        let wrong_scope = SettingChangeReceipt {
            target: SettingTarget::edition(edition),
            before_canonical_json: reading_value_json(),
            after_canonical_json: reading_value_json(),
            ..receipt_with_changed(false)
        };
        assert_eq!(
            wrong_scope.validate().unwrap_err().code().as_str(),
            "SETTING_PROPOSAL_INTEGRITY_MISMATCH"
        );

        // 资源级目标的回执用 PreferenceData：合法（改前空覆盖、改后有值）。
        let after = canonical_json_of(&PreferenceData {
            reading: Some(ReadingPatch {
                font_size: Some(ReadingFontSize::Large),
                ..ReadingPatch::default()
            }),
            comic: None,
        })
        .unwrap();
        let resource_receipt = SettingChangeReceipt {
            target: SettingTarget::media_item(edition, MediaItemId::new()),
            before_canonical_json: preference,
            after_canonical_json: after,
            changed: true,
            ..receipt_with_changed(false)
        };
        resource_receipt.validate().unwrap();

        // 资源级目标同样要求 canonical 形式：键序未排序的等价 JSON 也是错行，
        // 否则 before/after 会因书写方式不同而无法跨进程稳定比较。
        let unsorted = SettingChangeReceipt {
            target: SettingTarget::edition(edition),
            before_canonical_json: r#"{"reading":null,"comic":null}"#.to_owned(),
            after_canonical_json: r#"{"reading":null,"comic":null}"#.to_owned(),
            ..receipt_with_changed(false)
        };
        assert_ne!(
            unsorted.before_canonical_json,
            canonical_json_of(&PreferenceData::default()).unwrap(),
            "测试前提：这段 JSON 与同一值的 canonical 形式不同"
        );
        assert_eq!(
            unsorted.validate().unwrap_err().code().as_str(),
            "SETTING_PROPOSAL_INTEGRITY_MISMATCH"
        );
    }

    #[test]
    fn expiry_is_evaluated_against_the_payload_timestamp() {
        let proposal = global_proposal();
        assert!(!proposal.is_expired_at(UtcMillis(1_999)));
        assert!(proposal.is_expired_at(UtcMillis(2_000)));
        assert!(proposal.is_expired_at(UtcMillis(9_999)));
    }
}
