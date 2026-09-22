//! Agent 运行时领域模型（Haven Copilot Agent Runtime 垂直切片 1）。
//!
//! 本模块只定义**闭合、可序列化、可校验**的领域值对象，不含任何 Provider 调用、
//! 工具执行、凭据读取或权限提升路径：
//! - `AgentSubject` 只表达"这次推理针对哪个作品范围"，**不授予任何权限**；
//! - `AgentContextSnapshot` 是"Agent 看到了什么"的可哈希事实：它由**不含自身
//!   context_hash** 的 canonical 载荷算出摘要，任何字段被改写都无法再自洽；
//! - `AgentCapabilityManifest` 是**关闭优先**的：默认只声明本切片已实现的能力，
//!   未实现的写能力在构造/校验时被拒绝，不能通过 manifest 变相授权。
//!
//! 与既有模型的边界：
//! - Locator 复用 `crate::locator::Locator`，不新增第二套定位结构；
//! - canonical JSON 与摘要复用 `crate::setting_proposal` 的 canonicalizer；
//! - Agent 产生的设置改动必须变成 `SettingProposal`（Application 层桥接），
//!   本模块不提供任何直接写设置事实的入口。

use std::collections::HashSet;
use std::fmt;
use std::str::FromStr;

use haven_common::{AppError, ErrorKind, UtcMillis};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use zeroize::Zeroize;

use crate::ids::{
    AgentContextSnapshotId, AgentRequestId, AgentSessionId, EditionId, MediaItemId,
    SettingProposalId, WorkId,
};
use crate::locator::Locator;
use crate::setting_proposal::{
    SettingProposalChange, SettingTarget, canonical_digest, canonical_json_of, is_canonical_digest,
};
use crate::settings::{
    PreferenceData, ReadingPatch, ReadingSettings, SettingsPatch, SettingsSection,
};

/// canonical 上下文载荷的 schema 版本（参与 `context_hash`）。
pub const AGENT_CONTEXT_PAYLOAD_VERSION: u32 = 1;

/// Agent 协议/能力清单的当前版本。
pub const AGENT_API_VERSION: u32 = 1;

/// 单个快照允许的最大 segment 数。
pub const AGENT_CONTEXT_MAX_SEGMENTS: usize = 32;
/// 单个 segment 文本的字符数上限。
pub const AGENT_CONTEXT_MAX_SEGMENT_CHARS: usize = 2_000;
/// 整个快照文本的字符数上限（与 segment 数上限一起挡掉"整部作品"）。
pub const AGENT_CONTEXT_MAX_TOTAL_CHARS: usize = 16_000;
/// 单个快照允许的最大 citation 数。
pub const AGENT_CONTEXT_MAX_CITATIONS: usize = 64;
/// segment id 的字符数上限。
pub const AGENT_SEGMENT_ID_MAX_CHARS: usize = 64;
/// content_revision 的字符数上限。
pub const AGENT_CONTENT_REVISION_MAX_CHARS: usize = 128;

// ---------- Subject ----------

/// 内容形态（闭合集合；IPC 序列化为 `snake_case`）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AgentContentKind {
    Book,
    Comic,
    Video,
    Periodical,
}

/// Agent 任务类型（闭合集合）。
///
/// 任务只描述"要做什么"，不是权限：任何任务都只能通过 Application 层的
/// 提案桥接产生副作用。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AgentTask {
    /// 回顾/前情提要。
    Recap,
    /// 上下文问答。
    QuestionAnswer,
    /// 解释选中内容。
    ExplainSelection,
    /// 翻译选中内容。
    TranslateSelection,
    /// 期刊期号摘要。
    PeriodicalSummary,
    /// 人物关系梳理。
    CharacterRelations,
}

impl AgentTask {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Recap => "recap",
            Self::QuestionAnswer => "question_answer",
            Self::ExplainSelection => "explain_selection",
            Self::TranslateSelection => "translate_selection",
            Self::PeriodicalSummary => "periodical_summary",
            Self::CharacterRelations => "character_relations",
        }
    }

    /// 该任务是否必须建立在**已解析**的 locator 上。
    ///
    /// "解释/翻译选中内容""前情回顾"都隐含"用户当前所在的这一段"，
    /// 没有可解析位置的上下文不足以支撑这些任务。
    pub const fn requires_locator(self) -> bool {
        matches!(
            self,
            Self::Recap | Self::ExplainSelection | Self::TranslateSelection
        )
    }
}

/// 上下文边界模式（闭合集合）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AgentBoundaryMode {
    /// 严格限制在 locator 指向的位置/范围内。
    StrictToLocator,
    /// 当前阅读范围（由当前进度推导）。
    CurrentRange,
    /// 用户显式提供的范围（可没有可解析 locator）。
    UserProvided,
}

/// locator 的解析置信度（闭合集合）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AgentLocatorConfidence {
    /// 精确定位到一点。
    Exact,
    /// 定位到一个范围（起止已知，但不是一个点）。
    Range,
    /// 无法解析：只能表达"没有可用位置"。
    Unresolved,
}

/// 推理范围（**只表达范围，不授予权限**）。
///
/// 不变量：
/// - `media_item_id` 必须与 `edition_id` 同时出现（媒体条目永远归属某个版本）；
/// - `content_kind` 关闭集合，未知形态无法构造；
/// - 这里**没有**任何权限位：能不能读设置、能不能写设置由 capability manifest 决定，
///   subject 只回答"针对哪一块内容"。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct AgentSubject {
    pub work_id: WorkId,
    pub edition_id: Option<EditionId>,
    pub media_item_id: Option<MediaItemId>,
    pub content_kind: AgentContentKind,
}

impl AgentSubject {
    /// 构造并校验归属约束（MediaItem 必须带 Edition）。
    pub fn new(
        work_id: WorkId,
        edition_id: Option<EditionId>,
        media_item_id: Option<MediaItemId>,
        content_kind: AgentContentKind,
    ) -> Result<Self, AppError> {
        let subject = Self {
            work_id,
            edition_id,
            media_item_id,
            content_kind,
        };
        subject.validate()?;
        Ok(subject)
    }

    /// 作品级范围（不指定版本/条目）。
    pub fn work(work_id: WorkId, content_kind: AgentContentKind) -> Self {
        Self {
            work_id,
            edition_id: None,
            media_item_id: None,
            content_kind,
        }
    }

    /// 版本级范围。
    pub fn edition(work_id: WorkId, edition_id: EditionId, content_kind: AgentContentKind) -> Self {
        Self {
            work_id,
            edition_id: Some(edition_id),
            media_item_id: None,
            content_kind,
        }
    }

    /// 媒体条目级范围（必须同时给出所属版本）。
    pub fn media_item(
        work_id: WorkId,
        edition_id: EditionId,
        media_item_id: MediaItemId,
        content_kind: AgentContentKind,
    ) -> Self {
        Self {
            work_id,
            edition_id: Some(edition_id),
            media_item_id: Some(media_item_id),
            content_kind,
        }
    }

    /// 校验归属约束与强类型身份：
    /// - `work_id` 必须是非 nil 的强类型 ID；
    /// - 出现 `edition_id` / `media_item_id` 时同样必须非 nil；
    /// - `media_item_id` 必须与 `edition_id` 同时出现。
    ///
    /// 便捷构造（`work` / `edition` / `media_item`）为了签名稳定仍返回 `Self`，
    /// 但它们产出的任何非法身份都会在 `AgentContextPayload::validate`（快照边界）
    /// 与 `AgentActionProposal` 构造边界被拒绝，不会静默流入推理。
    pub fn validate(&self) -> Result<(), AppError> {
        if self.work_id.as_uuid().is_nil() {
            return Err(invalid_subject("作品 ID 不能是 nil UUID"));
        }
        if let Some(edition_id) = self.edition_id {
            if edition_id.as_uuid().is_nil() {
                return Err(invalid_subject("版本 ID 不能是 nil UUID"));
            }
        }
        if let Some(media_item_id) = self.media_item_id {
            if media_item_id.as_uuid().is_nil() {
                return Err(invalid_subject("媒体条目 ID 不能是 nil UUID"));
            }
        }
        if self.media_item_id.is_some() && self.edition_id.is_none() {
            return Err(invalid_subject("媒体条目范围必须同时指定所属版本"));
        }
        Ok(())
    }

    /// Subject 是否覆盖某个设置目标作用域。
    ///
    /// - 全局设置不属于任何作品范围：任何已解析的 subject 都可以提出全局建议；
    /// - 版本级目标必须与 subject 的 `edition_id` 一致；
    /// - 媒体条目级目标必须与 subject 的 `edition_id` 和 `media_item_id` 都一致。
    ///
    /// 这是"跨 Subject/Context 绑定"的判据：Agent 不能借 A 作品的上下文
    /// 去改 B 作品（或 B 版本）的设置。
    pub fn covers(self, target: SettingTarget) -> bool {
        match target {
            SettingTarget::Global(_) => true,
            SettingTarget::Edition(target) => self.edition_id == Some(target.edition_id),
            SettingTarget::MediaItem(target) => {
                self.edition_id == Some(target.edition_id)
                    && self.media_item_id == Some(target.media_item_id)
            }
        }
    }
}

// ---------- Agent Settings Subject ----------

/// 全局设置范围的 Section（闭合集合；第一版只有 `reading`）。
///
/// 它是**全局设置作用域**自己的枚举，不复用作品内容形态
/// （[`AgentContentKind`]），也不借用 `SettingsSection` 的全量分区：
/// 本阶段只开放 `reading`，后续分区必须逐个显式加入契约。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AgentSettingsSection {
    Reading,
}

impl AgentSettingsSection {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Reading => "reading",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "reading" => Some(Self::Reading),
            _ => None,
        }
    }

    /// 对应的设置分区（唯一写目标）。
    pub const fn settings_section(self) -> SettingsSection {
        match self {
            Self::Reading => SettingsSection::Reading,
        }
    }
}

/// Agent **全局设置**范围（Agent Settings Subject）。
///
/// 它表达"这次推理面向全局设置"，因此**没有** work/edition/mediaItem 身份：
/// 既不需要、也不允许为了让结构看起来像作品上下文而伪造一个 Work ID。
/// 字段里没有权限位：能不能读设置、能不能提提案仍由服务端固定能力清单决定。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct AgentSettingsSubject {
    pub section: AgentSettingsSection,
}

impl AgentSettingsSubject {
    pub const fn reading() -> Self {
        Self {
            section: AgentSettingsSection::Reading,
        }
    }

    pub const fn new(section: AgentSettingsSection) -> Self {
        Self { section }
    }

    pub const fn validate(self) -> Result<(), AppError> {
        Ok(())
    }

    /// 设置范围只覆盖**全局**且分区一致的设置目标。
    ///
    /// 版本级/媒体条目级目标一律不覆盖：全局设置上下文没有资格改某个版本的覆盖值。
    pub fn covers(self, target: SettingTarget) -> bool {
        match target {
            SettingTarget::Global(global) => global.section == self.section.settings_section(),
            SettingTarget::Edition(_) | SettingTarget::MediaItem(_) => false,
        }
    }
}

/// Agent 动作的完整作用域：作品内容范围 **或** 全局设置范围。
///
/// 序列化形状刻意保持"内容变体与旧 [`AgentSubject`] 完全一致"（扁平对象）：
/// 因为已落库的 `subject_json` 就是旧形状，且恢复路径要求
/// "载荷 == 它的 canonical 重写"；任何嵌套包装都会让旧绑定变成
/// 完整性错误。设置变体是另一个字段集合（没有 work/edition/mediaItem），
/// 因此在两个变体之间不存在有歧义的 JSON。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(untagged)]
pub enum AgentScopeSubject {
    Content(AgentSubject),
    Settings(AgentSettingsSubject),
}

impl AgentScopeSubject {
    pub const fn settings(subject: AgentSettingsSubject) -> Self {
        Self::Settings(subject)
    }

    pub const fn content(subject: AgentSubject) -> Self {
        Self::Content(subject)
    }

    pub const fn as_content(self) -> Option<AgentSubject> {
        match self {
            Self::Content(subject) => Some(subject),
            Self::Settings(_) => None,
        }
    }

    pub const fn as_settings(self) -> Option<AgentSettingsSubject> {
        match self {
            Self::Settings(subject) => Some(subject),
            Self::Content(_) => None,
        }
    }

    pub fn validate(&self) -> Result<(), AppError> {
        match self {
            Self::Content(subject) => subject.validate(),
            Self::Settings(subject) => subject.validate(),
        }
    }

    /// 该作用域是否覆盖某个设置目标（内容范围沿用原规则；设置范围只覆盖全局同分区）。
    pub fn covers(self, target: SettingTarget) -> bool {
        match self {
            Self::Content(subject) => subject.covers(target),
            Self::Settings(subject) => subject.covers(target),
        }
    }
}

impl From<AgentSubject> for AgentScopeSubject {
    fn from(subject: AgentSubject) -> Self {
        Self::Content(subject)
    }
}

impl From<AgentSettingsSubject> for AgentScopeSubject {
    fn from(subject: AgentSettingsSubject) -> Self {
        Self::Settings(subject)
    }
}

// ---------- 全局设置上下文 ----------

/// 全局阅读设置上下文的 schema 版本（参与 `context_hash`）。
pub const AGENT_SETTINGS_CONTEXT_PAYLOAD_VERSION: u32 = 1;

/// 被脱敏的字段名（闭合集合；Wire 上是 `snake_case` 字符串）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AgentSettingsRedactedField {
    CustomFontFamily,
    CustomBackground,
    CustomText,
}

impl AgentSettingsRedactedField {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::CustomFontFamily => "custom_font_family",
            Self::CustomBackground => "custom_background",
            Self::CustomText => "custom_text",
        }
    }
}

/// 哪些字段因为命中"疑似路径/凭据/endpoint"结构而被脱敏。
///
/// 脱敏是**事实的一部分**并进入 `context_hash`：调用方无法在不知情的情况下
/// 让一份"被裁剪过的快照"冒充完整快照。
#[derive(Debug, Clone, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct AgentReadingRedaction {
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub redacted_fields: Vec<AgentSettingsRedactedField>,
}

impl AgentReadingRedaction {
    pub fn is_empty(&self) -> bool {
        self.redacted_fields.is_empty()
    }

    pub fn contains(&self, field: AgentSettingsRedactedField) -> bool {
        self.redacted_fields.contains(&field)
    }
}

/// 进入全局设置上下文的**脱敏**阅读设置快照。
///
/// 只承载阅读排版的档位与布尔值；三个自由文本字段（自定义字体族、背景色、文字色）
/// 先过既有的敏感结构扫描，命中即以 `None` 落地并登记在 `redacted` 里。
/// 这里没有 API Key、凭据、绝对路径、正文或任意 JSON 的入口。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct AgentReadingSnapshot {
    /// 字段名与 Wire 投影（`PreferenceReadingSettingsDto`）逐字一致。
    pub font_family: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub custom_font_family: Option<String>,
    pub font_size: String,
    pub line_height: String,
    pub content_width: String,
    pub theme: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub custom_background: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub custom_text: Option<String>,
    pub font_weight: String,
    pub letter_spacing: String,
    pub system_auto: bool,
    pub pagination: String,
    pub redacted: AgentReadingRedaction,
}

impl AgentReadingSnapshot {
    /// 从 authoritative 阅读设置构造脱敏快照。
    ///
    /// 三个自由文本字段的字符集是开放的（用户可以粘贴任意字符串），因此在进入
    /// Agent 上下文之前必须走敏感结构扫描；命中即整体丢弃该字段并登记，
    /// 不做部分截断——半截的路径同样是路径。
    ///
    /// 扫描用 [`contains_sensitive_setting_value`] 而不是快照/正文用的
    /// [`contains_sensitive_text`]：后者的凭据关键词表（`custom_background` 这类
    /// 字段名本身就含 `secret`/`custom_*` 结构）是为**正文片段**设计的，
    /// 直接套在单值上会把正常字段值判成敏感。
    pub fn from_settings(settings: &ReadingSettings) -> Self {
        let mut redacted_fields = Vec::new();
        let mut keep = |value: &Option<String>, field: AgentSettingsRedactedField| match value {
            Some(text) if contains_sensitive_setting_value(text) => {
                redacted_fields.push(field);
                None
            }
            Some(text) => Some(text.clone()),
            None => None,
        };
        let custom_font_family = keep(
            &settings.custom_font_family,
            AgentSettingsRedactedField::CustomFontFamily,
        );
        let custom_background = keep(
            &settings.custom_background,
            AgentSettingsRedactedField::CustomBackground,
        );
        let custom_text = keep(
            &settings.custom_text,
            AgentSettingsRedactedField::CustomText,
        );
        redacted_fields.sort_unstable();
        redacted_fields.dedup();
        Self {
            font_family: preference_token(&serde_json::to_value(settings.font_family).ok()),
            custom_font_family,
            font_size: preference_token(&serde_json::to_value(settings.font_size).ok()),
            line_height: preference_token(&serde_json::to_value(settings.line_height).ok()),
            content_width: preference_token(&serde_json::to_value(settings.content_width).ok()),
            theme: preference_token(&serde_json::to_value(settings.theme).ok()),
            custom_background,
            custom_text,
            font_weight: preference_token(&serde_json::to_value(settings.font_weight).ok()),
            letter_spacing: preference_token(&serde_json::to_value(settings.letter_spacing).ok()),
            system_auto: settings.system_auto,
            pagination: preference_token(&serde_json::to_value(settings.pagination).ok()),
            redacted: AgentReadingRedaction { redacted_fields },
        }
    }
}

/// 自由文本设置值在 Agent 投影里的固定占位符。
///
/// 它是**唯一**允许替代敏感原值的字符串：任何人看到它都知道"这里原本有一个值，
/// 但按安全规则没有投影给 Agent"，而不是把它当成用户真的这么设置过。
pub const AGENT_REDACTED_VALUE: &str = "[redacted]";

/// 单个自由文本设置值的 Agent 投影：命中敏感结构 → 占位符。
///
/// 与 [`AgentReadingSnapshot::from_settings`] 的"清空 + 登记"不同，这里服务的是
/// **形状固定、只有字符串槽位**的改动列表（`key`/`before`/`after`）：那里没有
/// "这个字段被脱敏过"的登记位，只能用固定占位符替换。
pub fn redact_setting_value_for_agent(value: &str) -> &str {
    if contains_sensitive_setting_value(value) {
        AGENT_REDACTED_VALUE
    } else {
        value
    }
}

/// 整个阅读 Patch 的自由文本字段投影：敏感字段清空并登记被清空的字段名。
///
/// 这是"资源偏好快照 → Agent"的唯一脱敏规则，与全局设置上下文的快照共用同一个
/// 单值判据（[`contains_sensitive_setting_value`]），避免两处各自实现、各自漂移。
pub fn redact_reading_patch_for_agent(
    patch: &ReadingPatch,
) -> (ReadingPatch, Vec<AgentSettingsRedactedField>) {
    let mut redacted_fields = Vec::new();
    let mut keep = |value: &Option<String>, field: AgentSettingsRedactedField| match value {
        Some(text) if contains_sensitive_setting_value(text) => {
            redacted_fields.push(field);
            None
        }
        Some(text) => Some(text.clone()),
        None => None,
    };
    let custom_font_family = keep(
        &patch.custom_font_family,
        AgentSettingsRedactedField::CustomFontFamily,
    );
    let custom_background = keep(
        &patch.custom_background,
        AgentSettingsRedactedField::CustomBackground,
    );
    let custom_text = keep(&patch.custom_text, AgentSettingsRedactedField::CustomText);
    redacted_fields.sort_unstable();
    redacted_fields.dedup();
    (
        ReadingPatch {
            custom_font_family,
            custom_background,
            custom_text,
            ..patch.clone()
        },
        redacted_fields,
    )
}

/// 将 authoritative 阅读设置投影为可进入 Agent 回执/差异的值。
///
/// 与上下文快照一样，命中敏感结构的自由文本不会被替换成用户可误认的普通值；
/// 统一使用固定占位符。这个函数只用于 Agent 的审计投影，不改变设置事实源。
pub fn redact_reading_settings_for_agent(settings: &ReadingSettings) -> ReadingSettings {
    ReadingSettings {
        custom_font_family: settings
            .custom_font_family
            .as_deref()
            .map(redact_setting_value_for_agent)
            .map(str::to_owned),
        custom_background: settings
            .custom_background
            .as_deref()
            .map(redact_setting_value_for_agent)
            .map(str::to_owned),
        custom_text: settings
            .custom_text
            .as_deref()
            .map(redact_setting_value_for_agent)
            .map(str::to_owned),
        ..settings.clone()
    }
}

/// 将资源偏好值投影为 Agent 可见的值。
///
/// 资源偏好在存储层是窄 Patch，而不是完整阅读设置；这里只复制结构并对三个开放
/// 字符串槽位做同一套脱敏。资源快照/资源 Receipt 没有全局快照那样的字段登记，
/// 因此使用固定 `[redacted]` 占位符而不是清空字段，避免 Agent 把“被隐藏”误判成
/// “用户没有设置”。真正的 Agent 资源写入保存的是 patch 变体，不能依赖这个投影
/// 来恢复被隐藏的 authoritative 值。
pub fn redact_preference_data_for_agent(data: &PreferenceData) -> PreferenceData {
    PreferenceData {
        reading: data.reading.as_ref().map(|patch| ReadingPatch {
            custom_font_family: patch
                .custom_font_family
                .as_deref()
                .map(redact_setting_value_for_agent)
                .map(str::to_owned),
            custom_background: patch
                .custom_background
                .as_deref()
                .map(redact_setting_value_for_agent)
                .map(str::to_owned),
            custom_text: patch
                .custom_text
                .as_deref()
                .map(redact_setting_value_for_agent)
                .map(str::to_owned),
            ..patch.clone()
        }),
        comic: data.comic.clone(),
    }
}

/// Agent 来源提案的自由文本安全校验。
///
/// 用户自己的设置允许自由文本（本机字体族名、自定义取色），因此**这条规则只加在
/// Agent 来源的写入上**：Agent 提案不得把"疑似绝对路径 / endpoint / 凭据赋值"的
/// 自由文本写进设置事实源。
///
/// 命中即拒绝，且错误文案是**固定短语**：被拒绝的原值不会出现在错误消息、wire
/// 或日志里——否则"拒绝"本身就成了把敏感值带出边界的信道。
pub fn validate_agent_proposal_free_text(change: &SettingProposalChange) -> Result<(), AppError> {
    let reading = match change {
        SettingProposalChange::SettingsPatch(SettingsPatch::Reading(patch)) => patch,
        SettingProposalChange::ResourcePreference(data)
        | SettingProposalChange::AgentResourcePreferencePatch(data) => match &data.reading {
            Some(patch) => patch,
            None => return Ok(()),
        },
        _ => return Ok(()),
    };
    for (value, field) in [
        (
            &reading.custom_font_family,
            AgentSettingsRedactedField::CustomFontFamily,
        ),
        (
            &reading.custom_background,
            AgentSettingsRedactedField::CustomBackground,
        ),
        (&reading.custom_text, AgentSettingsRedactedField::CustomText),
    ] {
        let Some(value) = value else { continue };
        if contains_sensitive_setting_value(value) {
            return Err(agent_free_text_error(field));
        }
    }
    Ok(())
}

fn agent_free_text_error(_field: AgentSettingsRedactedField) -> AppError {
    AppError::new(
        "AGENT_PROPOSAL_SENSITIVE_FREE_TEXT",
        ErrorKind::Validation,
        "Agent 提案包含不允许的敏感自由文本，已拒绝（原值不回显）",
        false,
    )
}

/// 枚举值在快照里只作为**字符串 token** 保存：领域层不复制一份 Wire 字符串字面量，
/// 而是直接采用该枚举自身的 serde 表示（与 Wire DTO 共用同一个 `snake_case` 规则）。
fn preference_token(value: &Option<serde_json::Value>) -> String {
    match value {
        Some(serde_json::Value::String(text)) => text.clone(),
        _ => "unknown".to_owned(),
    }
}

/// `context_hash` 唯一覆盖的全局设置上下文载荷（**不含** snapshot id 与哈希自身）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct AgentSettingsContextPayload {
    pub version: u32,
    pub subject: AgentSettingsSubject,
    /// 构造这份快照时读到的 authoritative revision；从未保存过该分区时为 `None`。
    ///
    /// 它参与哈希，因此"设置被改过"必然带来另一个上下文 id/hash。
    pub revision: Option<String>,
    pub reading: AgentReadingSnapshot,
    /// 生成这份上下文的服务端固定能力清单（调用方不能自带清单）。
    pub capabilities: AgentCapabilityManifest,
    pub privacy: AgentPrivacyDescriptor,
}

impl AgentSettingsContextPayload {
    /// 构造骨架（reading 与 capabilities 由调用方填入；调用方通常是 Application 服务）。
    pub fn new(subject: AgentSettingsSubject, revision: Option<String>) -> Self {
        Self {
            version: AGENT_SETTINGS_CONTEXT_PAYLOAD_VERSION,
            subject,
            revision,
            reading: AgentReadingSnapshot::from_settings(&ReadingSettings::default()),
            capabilities: AgentCapabilityManifest::for_current_slice(),
            privacy: AgentPrivacyDescriptor::default(),
        }
    }

    pub fn validate(&self) -> Result<(), AppError> {
        if self.version != AGENT_SETTINGS_CONTEXT_PAYLOAD_VERSION {
            return Err(AppError::new(
                "AGENT_SETTINGS_CONTEXT_UNSUPPORTED_VERSION",
                ErrorKind::Unsupported,
                "Agent 设置上下文载荷版本不受支持",
                false,
            ));
        }
        self.subject.validate()?;
        if let Some(revision) = &self.revision {
            validate_settings_revision(revision)?;
        }
        self.capabilities.validate()?;
        // 能力清单是"本切片已实现能力"的声明，设置上下文只允许声明设置读取/提案；
        // 别的能力即使将来实现，也不得从设置上下文这里变相出现。
        if !self.capabilities.grants(AgentCapability::SettingsRead)
            || !self.capabilities.grants(AgentCapability::SettingsProposal)
        {
            return Err(invalid_settings_context(
                "设置上下文的能力清单必须声明设置读取与设置提案",
            ));
        }
        // 脱敏登记与字段现状必须自洽：登记了就得真空，没有登记就不能凭空缺失。
        for field in &self.reading.redacted.redacted_fields {
            let cleared = match field {
                AgentSettingsRedactedField::CustomFontFamily => {
                    self.reading.custom_font_family.is_none()
                }
                AgentSettingsRedactedField::CustomBackground => {
                    self.reading.custom_background.is_none()
                }
                AgentSettingsRedactedField::CustomText => self.reading.custom_text.is_none(),
            };
            if !cleared {
                return Err(invalid_settings_context("脱敏登记的字段必须已被清空"));
            }
        }
        self.privacy.validate()
    }
}

/// 一次"读取全局设置"所看到的上下文事实。
///
/// 与 [`AgentContextSnapshot`] 一样，字段私有：canonical JSON 与
/// `context_hash` 只能由构造/恢复路径产生。`id` 是**从 context_hash 派生**的
/// 确定性身份（见 [`derive_settings_context_id`]），因此同一个设置版本永远得到
/// 同一个 id，不需要额外持久化"这个 id 曾经存在过"。
#[derive(Debug, Clone, PartialEq)]
pub struct AgentSettingsContextSnapshot {
    id: AgentContextSnapshotId,
    payload: AgentSettingsContextPayload,
    canonical_json: String,
    context_hash: String,
}

impl AgentSettingsContextSnapshot {
    pub fn new(
        id: AgentContextSnapshotId,
        payload: AgentSettingsContextPayload,
    ) -> Result<Self, AppError> {
        payload.validate()?;
        let canonical_json = canonical_json_of(&payload)?;
        let context_hash = canonical_digest(&canonical_json);
        let snapshot = Self {
            id,
            payload,
            canonical_json,
            context_hash,
        };
        snapshot.validate()?;
        Ok(snapshot)
    }

    /// 用**从载荷派生**的确定性 id 构造快照（生产路径）。
    pub fn derive(payload: AgentSettingsContextPayload) -> Result<Self, AppError> {
        payload.validate()?;
        let canonical_json = canonical_json_of(&payload)?;
        let context_hash = canonical_digest(&canonical_json);
        let id = derive_settings_context_id(&context_hash);
        Self::new(id, payload)
    }

    /// 重算 canonical 载荷与 `context_hash`，并核对 id 就是它的派生身份。
    pub fn validate(&self) -> Result<(), AppError> {
        if self.id.as_uuid().is_nil() {
            return Err(invalid_settings_context("上下文 ID 不能是 nil UUID"));
        }
        self.payload.validate()?;
        let canonical_json = canonical_json_of(&self.payload)?;
        if canonical_json != self.canonical_json {
            return Err(settings_context_integrity_error(
                "canonical 载荷与内存中的载荷不一致",
            ));
        }
        if canonical_digest(&canonical_json) != self.context_hash {
            return Err(settings_context_integrity_error(
                "context_hash 与载荷不一致",
            ));
        }
        if derive_settings_context_id(&self.context_hash) != self.id {
            return Err(settings_context_integrity_error(
                "上下文 ID 不是 context_hash 的派生身份",
            ));
        }
        Ok(())
    }

    /// 从存储恢复：载荷必须是 canonical 形式、哈希自洽，且 id 必须是派生身份。
    pub fn from_canonical_payload(
        id: AgentContextSnapshotId,
        payload_json: &str,
        context_hash: &str,
    ) -> Result<Self, AppError> {
        let payload = parse_canonical_settings_payload(payload_json, context_hash)?;
        Self::new(id, payload)
    }

    /// 只做完整性校验：不构造快照，也不做语义判断。
    pub fn verify_integrity(payload_json: &str, context_hash: &str) -> Result<(), AppError> {
        parse_canonical_settings_payload(payload_json, context_hash).map(|_| ())
    }

    pub fn id(&self) -> AgentContextSnapshotId {
        self.id
    }

    pub fn payload(&self) -> &AgentSettingsContextPayload {
        &self.payload
    }

    pub fn subject(&self) -> AgentSettingsSubject {
        self.payload.subject
    }

    pub fn revision(&self) -> Option<&str> {
        self.payload.revision.as_deref()
    }

    pub fn reading(&self) -> &AgentReadingSnapshot {
        &self.payload.reading
    }

    pub fn capabilities(&self) -> AgentCapabilityManifest {
        self.payload.capabilities
    }

    pub fn canonical_json(&self) -> &str {
        &self.canonical_json
    }

    pub fn context_hash(&self) -> &str {
        &self.context_hash
    }
}

/// 从 `context_hash` 派生确定性上下文 ID（RFC 4122 v5 风格：SHA-256/16 字节 + 版本/变体位）。
///
/// 目的不是"加密身份"，而是让"这个设置上下文"有一个**可从内容重算**的
/// 不透明 ID：设置没变 → 同一个 ID；设置变了 → 另一个 ID。
/// 因此 `agent_settings_context_get` 不需要把上下文落库，也不需要维护
/// "这个 id 我发过"的登记表；重建时对不上即 fail-closed。
pub fn derive_settings_context_id(context_hash: &str) -> AgentContextSnapshotId {
    let digest = Sha256::digest(context_hash.as_bytes());
    let mut bytes = [0u8; 16];
    bytes.copy_from_slice(&digest[..16]);
    // version 5（名称型/派生型）+ RFC 4122 变体，与 UUID 语义保持一致。
    bytes[6] = (bytes[6] & 0x0f) | 0x50;
    bytes[8] = (bytes[8] & 0x3f) | 0x80;
    AgentContextSnapshotId::from_uuid(uuid::Uuid::from_bytes(bytes))
}

fn parse_canonical_settings_payload(
    payload_json: &str,
    context_hash: &str,
) -> Result<AgentSettingsContextPayload, AppError> {
    if !is_canonical_digest(context_hash) {
        return Err(settings_context_integrity_error(
            "context_hash 不是 SHA-256 小写十六进制",
        ));
    }
    let payload: AgentSettingsContextPayload = serde_json::from_str(payload_json)
        .map_err(|_| settings_context_integrity_error("设置上下文载荷无法解析"))?;
    if canonical_json_of(&payload)? != payload_json {
        return Err(settings_context_integrity_error(
            "设置上下文载荷不是 canonical 形式",
        ));
    }
    if canonical_digest(payload_json) != context_hash {
        return Err(settings_context_integrity_error(
            "context_hash 与载荷不一致",
        ));
    }
    Ok(payload)
}

/// authoritative 设置 revision 的形状（数据库生成的不透明 token）。
fn validate_settings_revision(revision: &str) -> Result<(), AppError> {
    if revision.is_empty() || revision.chars().count() > AGENT_CONTENT_REVISION_MAX_CHARS {
        return Err(invalid_settings_context("设置 revision 长度非法"));
    }
    if !revision
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
    {
        return Err(invalid_settings_context(
            "设置 revision 只允许不透明 token 字符",
        ));
    }
    Ok(())
}

// ---------- Agent Action Binding ----------
/// Agent 动作类型。它是持久化绑定的一部分，不是权限声明。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AgentActionKind {
    SettingsProposal,
}

impl AgentActionKind {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::SettingsProposal => "settings_proposal",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "settings_proposal" => Some(Self::SettingsProposal),
            _ => None,
        }
    }
}

/// Agent 动作绑定的审批生命周期。
///
/// 它与 SettingProposal 的生命周期一一对应，但作为 Agent 绑定自己的完整性事实
/// 单独持久化；Application/Infrastructure 必须在同一事务中推进两者，不能只改其中一条。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AgentApprovalState {
    Pending,
    Applied,
    Rejected,
    Expired,
}

impl AgentApprovalState {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Pending => "pending",
            Self::Applied => "applied",
            Self::Rejected => "rejected",
            Self::Expired => "expired",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "pending" => Some(Self::Pending),
            "applied" => Some(Self::Applied),
            "rejected" => Some(Self::Rejected),
            "expired" => Some(Self::Expired),
            _ => None,
        }
    }

    /// 是否为终态：进入终态后不得再携带令牌，也不能再签发。
    pub const fn is_terminal(self) -> bool {
        !matches!(self, Self::Pending)
    }

    pub fn validate_transition(self, next: Self) -> Result<(), AppError> {
        if self == next || (self == Self::Pending && next != Self::Pending) {
            return Ok(());
        }
        Err(invalid_agent_binding("审批状态不能从终态回退或横向迁移"))
    }
}

// ---------- 一次性审批令牌 ----------

/// 原始令牌的总字符数：32 字节随机载荷 → 64 位小写十六进制，无前缀、无分隔符。
///
/// 长度本身就封闭：不存在"带前缀/带分隔符"的第二种写法，因此任何偏离
/// 64 位小写十六进制的输入都不是"另一种令牌"，而是伪造或损坏。
pub const AGENT_APPROVAL_TOKEN_LEN: usize = 64;
/// 令牌载荷的字节数（32 字节 → 64 位小写十六进制）。
const AGENT_APPROVAL_TOKEN_BYTES: usize = AGENT_APPROVAL_TOKEN_LEN / 2;
/// 令牌摘要的 SHA-256 绑定域前缀（参与 preimage，避免与其它摘要用途混淆）。
const AGENT_APPROVAL_TOKEN_HASH_DOMAIN: &str = "haven:agent-approval-token:v1";

/// 小写十六进制字母表（生成与校验共用一份，避免两处写法漂移）。
const HEX_DIGITS: &[u8; 16] = b"0123456789abcdef";

/// 一次性审批令牌的**原始明文**。
///
/// 它只在签发结果中出现一次；持久化只保存 [`AgentApprovalTokenHash`]，因此
/// 令牌被盗库的代价远高于它在内存中的可见窗口。四道防线：
/// - `Debug`/`Display` 一律脱敏为 `[REDACTED]`，无法用 `{:?}` / `{token}` 打印；
/// - **不实现** `Clone` / `Serialize` / `Deserialize`，无法被复制残留或写进
///   任何 wire/DTO 载荷；
/// - `Drop` 清零底层缓冲区，被拒绝的输入同样清零；
/// - 唯一读取入口是 [`AgentApprovalToken::expose`] / [`AgentApprovalToken::as_str`]，
///   调用点因此可以被审查（两者都是显式调用，不存在隐式泄漏路径）。
///
/// **刻意不实现 `PartialEq`**：令牌比较必须走
/// [`AgentActionBinding::verify_approval_token`] 这条与提案绑定、带状态与
/// 时效校验的路径，而不是裸字符串比较。
pub struct AgentApprovalToken {
    raw: String,
}

impl AgentApprovalToken {
    /// 签发一个新令牌：32 字节随机载荷（64 位小写十六进制）。
    ///
    /// 随机性来自已有依赖 `uuid` 的 v7 生成器（不新增依赖）：每次取一个 v7 UUID
    /// 的 16 字节原始表示，两次拼成 32 字节。载荷因此包含生成时刻（v7 毫秒时间戳）
    /// 与 UUID v7 的 74 位随机位——**这不是密码学强度的秘密**，它保证的是"不可
    /// 预测、不可枚举、不可离线爆破"，并把令牌与签发时刻绑在一起。
    pub fn issue() -> Self {
        let mut bytes = [0u8; AGENT_APPROVAL_TOKEN_BYTES];
        for chunk in bytes.chunks_mut(16) {
            let seed = uuid::Uuid::new_v7(uuid::Timestamp::now(uuid::NoContext));
            chunk.copy_from_slice(seed.as_bytes());
        }
        let mut raw = String::with_capacity(AGENT_APPROVAL_TOKEN_LEN);
        for byte in bytes {
            raw.push(HEX_DIGITS[(byte >> 4) as usize] as char);
            raw.push(HEX_DIGITS[(byte & 0x0f) as usize] as char);
        }
        bytes.zeroize();
        Self { raw }
    }

    /// 从外部输入恢复令牌：形状必须严格闭合（恰好 64 位小写十六进制）。
    ///
    /// 形状之外的写法（大写、带前缀、长度不符、非十六进制字符）一律拒绝，且被
    /// 拒绝的输入同样会被清零——它可能是一条"接近正确"的令牌。
    pub fn from_text(text: impl Into<String>) -> Result<Self, AppError> {
        let mut text = text.into();
        if let Err(error) = validate_approval_token_shape(&text) {
            text.zeroize();
            return Err(error);
        }
        Ok(Self { raw: text })
    }

    /// 暴露原始令牌：只允许在"把它交回签发调用方"的边界使用。
    ///
    /// 禁止写进日志、错误消息、provenance、回执或任何 wire 载荷。
    pub fn expose(&self) -> &str {
        &self.raw
    }

    /// [`AgentApprovalToken::expose`] 的同义入口（显式调用，语义相同）。
    pub fn as_str(&self) -> &str {
        &self.raw
    }

    /// 计算与 `proposal_id` + 提案 digest 绑定的 SHA-256 摘要。
    ///
    /// preimage 由**域前缀 + 提案 ID + 提案 digest + 原始令牌**组成，因此同一枚
    /// 令牌换一条提案、或提案载荷被改写（digest 变化）都会让摘要不再匹配：
    /// 存储的摘要本身就是"这枚令牌被批准用来改这一份内容"的绑定证据。
    pub fn bind(
        &self,
        proposal_id: SettingProposalId,
        proposal_digest: &str,
    ) -> Result<AgentApprovalTokenHash, AppError> {
        if proposal_id.as_uuid().is_nil() {
            return Err(agent_approval_token_invalid("提案 ID 不能是 nil"));
        }
        if !is_canonical_digest(proposal_digest) {
            return Err(agent_approval_token_invalid("提案摘要不是闭合摘要"));
        }
        let mut hasher = Sha256::new();
        hasher.update(AGENT_APPROVAL_TOKEN_HASH_DOMAIN.as_bytes());
        hasher.update(b"\n");
        hasher.update(proposal_id.to_string().as_bytes());
        hasher.update(b"\n");
        hasher.update(proposal_digest.as_bytes());
        hasher.update(b"\n");
        hasher.update(self.raw.as_bytes());
        let digest: String = hasher
            .finalize()
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect();
        AgentApprovalTokenHash::parse(digest)
    }
}

impl Drop for AgentApprovalToken {
    fn drop(&mut self) {
        self.raw.zeroize();
    }
}

impl fmt::Debug for AgentApprovalToken {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("[REDACTED]")
    }
}

impl fmt::Display for AgentApprovalToken {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("[REDACTED]")
    }
}

/// 一次性审批令牌的 SHA-256 摘要（绑定提案 ID 与提案 digest）。
///
/// 它是**可持久化**的令牌材料：绑定表只存这一个值，明文永不落库。它不是秘密
/// （单向摘要 + 256 位随机明文不可反推），但序列化入口统一收敛到
/// [`AgentApprovalTokenHash::parse`]，任何与存储不一致的形状（大写、长度不符、
/// 非十六进制）在反序列化阶段就被拒绝，而不是被当作"另一种写法"接受。
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct AgentApprovalTokenHash(String);

impl AgentApprovalTokenHash {
    /// 校验摘要形状（64 位 ASCII 小写十六进制）。
    pub fn parse(value: impl Into<String>) -> Result<Self, AppError> {
        let value = value.into();
        if !is_canonical_digest(&value) {
            return Err(agent_approval_token_invalid("令牌摘要不是闭合摘要"));
        }
        Ok(Self(value))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl Serialize for AgentApprovalTokenHash {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(&self.0)
    }
}

impl<'de> Deserialize<'de> for AgentApprovalTokenHash {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let value = String::deserialize(deserializer)?;
        Self::parse(value)
            .map_err(|error| serde::de::Error::custom(error.user_message().to_owned()))
    }
}

fn validate_approval_token_shape(raw: &str) -> Result<(), AppError> {
    if raw.len() != AGENT_APPROVAL_TOKEN_LEN {
        return Err(agent_approval_token_invalid("令牌长度非法"));
    }
    if !raw
        .bytes()
        .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    {
        return Err(agent_approval_token_invalid("令牌不是 64 位小写十六进制"));
    }
    Ok(())
}

/// 创建 Agent 动作绑定时的输入。
///
/// 不包含提案 ID、时间和状态：这些值必须由 SettingProposalService 在同一创建事务中
/// 从真实提案派生，调用方不能分别构造两套生命周期事实。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AgentActionBindingInput {
    session_id: AgentSessionId,
    request_id: AgentRequestId,
    context_snapshot_id: AgentContextSnapshotId,
    context_hash: String,
    subject: AgentScopeSubject,
    action_kind: AgentActionKind,
}

impl AgentActionBindingInput {
    pub fn new(
        session_id: AgentSessionId,
        request_id: AgentRequestId,
        context_snapshot_id: AgentContextSnapshotId,
        context_hash: impl Into<String>,
        subject: impl Into<AgentScopeSubject>,
        action_kind: AgentActionKind,
    ) -> Result<Self, AppError> {
        let input = Self {
            session_id,
            request_id,
            context_snapshot_id,
            context_hash: context_hash.into(),
            subject: subject.into(),
            action_kind,
        };
        input.validate()?;
        Ok(input)
    }

    pub fn validate(&self) -> Result<(), AppError> {
        validate_agent_binding_context_ids(
            self.session_id,
            self.request_id,
            self.context_snapshot_id,
        )?;
        validate_agent_binding_context_hash(&self.context_hash)?;
        self.subject.validate()
    }

    pub const fn subject(&self) -> AgentScopeSubject {
        self.subject
    }
}

/// 持久化的 Agent 动作绑定。
///
/// 只保存“这次动作与哪条提案/上下文事实绑定”的最小元数据，不保存上下文正文、模型
/// 参数、Provider endpoint、凭据、SQL 或本地路径。提案本身仍是设置变更的唯一事实源。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct AgentActionBinding {
    proposal_id: SettingProposalId,
    session_id: AgentSessionId,
    request_id: AgentRequestId,
    context_snapshot_id: AgentContextSnapshotId,
    context_hash: String,
    subject: AgentScopeSubject,
    action_kind: AgentActionKind,
    created_at: UtcMillis,
    expires_at: UtcMillis,
    approval_state: AgentApprovalState,
    /// 当前有效令牌的摘要；`None` 表示尚未签发（或已被终态清除）。
    approval_token_hash: Option<AgentApprovalTokenHash>,
    /// 当前有效令牌的签发时刻；与 `approval_token_hash` 同生同灭。
    issued_at: Option<UtcMillis>,
}

impl AgentActionBinding {
    /// 唯一的组装入口：所有构造/恢复路径共用同一条校验与字段放置规则。
    #[allow(clippy::too_many_arguments)]
    fn build(
        proposal_id: SettingProposalId,
        session_id: AgentSessionId,
        request_id: AgentRequestId,
        context_snapshot_id: AgentContextSnapshotId,
        context_hash: String,
        subject: AgentScopeSubject,
        action_kind: AgentActionKind,
        created_at: UtcMillis,
        expires_at: UtcMillis,
        approval_state: AgentApprovalState,
        approval_token_hash: Option<AgentApprovalTokenHash>,
        issued_at: Option<UtcMillis>,
    ) -> Result<Self, AppError> {
        let binding = Self {
            proposal_id,
            session_id,
            request_id,
            context_snapshot_id,
            context_hash,
            subject,
            action_kind,
            created_at,
            expires_at,
            approval_state,
            approval_token_hash,
            issued_at,
        };
        binding.validate()?;
        Ok(binding)
    }

    /// 新建 pending 绑定：**默认不带令牌**，签发是之后的一次显式动作。
    pub fn pending(
        proposal_id: SettingProposalId,
        input: AgentActionBindingInput,
        created_at: UtcMillis,
        expires_at: UtcMillis,
    ) -> Result<Self, AppError> {
        input.validate()?;
        Self::build(
            proposal_id,
            input.session_id,
            input.request_id,
            input.context_snapshot_id,
            input.context_hash,
            input.subject,
            input.action_kind,
            created_at,
            expires_at,
            AgentApprovalState::Pending,
            None,
            None,
        )
    }

    /// 从存储恢复（无令牌字段的旧路径，保持签名兼容）。
    ///
    /// 等价于"恢复一条没有令牌的绑定"，因此终态与 pending 都适用；带令牌的
    /// 绑定必须走 [`AgentActionBinding::from_stored_with_token`]。
    #[allow(clippy::too_many_arguments)]
    pub fn from_stored(
        proposal_id: SettingProposalId,
        session_id: AgentSessionId,
        request_id: AgentRequestId,
        context_snapshot_id: AgentContextSnapshotId,
        context_hash: String,
        subject: AgentScopeSubject,
        action_kind: AgentActionKind,
        created_at: UtcMillis,
        expires_at: UtcMillis,
        approval_state: AgentApprovalState,
    ) -> Result<Self, AppError> {
        Self::build(
            proposal_id,
            session_id,
            request_id,
            context_snapshot_id,
            context_hash,
            subject,
            action_kind,
            created_at,
            expires_at,
            approval_state,
            None,
            None,
        )
    }

    /// 从存储恢复**带令牌**的绑定（持久化路径）。
    ///
    /// `approval_token_hash` / `issued_at` 必须同时给出或同时缺省；摘要形状由
    /// [`AgentApprovalTokenHash::parse`] 严格校验，与存储不一致的写法在这里就
    /// 失败，而不是被当成另一条合法事实。
    #[allow(clippy::too_many_arguments)]
    pub fn from_stored_with_token(
        proposal_id: SettingProposalId,
        session_id: AgentSessionId,
        request_id: AgentRequestId,
        context_snapshot_id: AgentContextSnapshotId,
        context_hash: String,
        subject: AgentScopeSubject,
        action_kind: AgentActionKind,
        created_at: UtcMillis,
        expires_at: UtcMillis,
        approval_state: AgentApprovalState,
        approval_token_hash: Option<String>,
        issued_at: Option<UtcMillis>,
    ) -> Result<Self, AppError> {
        let approval_token_hash = approval_token_hash
            .map(AgentApprovalTokenHash::parse)
            .transpose()?;
        Self::build(
            proposal_id,
            session_id,
            request_id,
            context_snapshot_id,
            context_hash,
            subject,
            action_kind,
            created_at,
            expires_at,
            approval_state,
            approval_token_hash,
            issued_at,
        )
    }

    /// 签发（或重新签发）审批令牌：写入令牌摘要与签发时刻。
    ///
    /// 只有 pending 绑定可以签发；`issued_at` 必须落在 `created_at..expires_at`
    /// 这个半开区间内。重新签发是**覆盖**语义：旧摘要被新摘要替换，旧令牌因此
    /// 立即失效（校验时摘要不再匹配）。
    pub fn issue_approval_token(
        &self,
        approval_token_hash: AgentApprovalTokenHash,
        issued_at: UtcMillis,
    ) -> Result<Self, AppError> {
        if self.approval_state != AgentApprovalState::Pending {
            return Err(agent_approval_token_invalid(
                "只有 pending 绑定可以签发令牌",
            ));
        }
        if issued_at.0 < self.created_at.0 || issued_at.0 >= self.expires_at.0 {
            return Err(agent_approval_token_invalid("签发时间不在绑定有效期内"));
        }
        let mut next = self.clone();
        next.approval_token_hash = Some(approval_token_hash);
        next.issued_at = Some(issued_at);
        next.validate()?;
        Ok(next)
    }

    /// 校验一枚令牌是否就是当前绑定已签发的令牌。
    ///
    /// 四道门都必须通过，否则 fail-closed：
    /// - 绑定必须仍是 pending（终态手上的令牌一律无效）；
    /// - 绑定必须带着未过期的签发记录；
    /// - 令牌必须与 `proposal_id` + `proposal_digest` 重新绑定出相同摘要——
    ///   换一条提案、改动提案载荷、或换一枚令牌都会让摘要不再匹配。
    ///
    /// `proposal_digest` 的形状由 [`AgentApprovalToken::bind`] 校验；任何失败
    /// 都不把令牌原文放进错误消息。
    pub fn verify_approval_token(
        &self,
        proposal_id: SettingProposalId,
        proposal_digest: &str,
        token: &AgentApprovalToken,
    ) -> Result<(), AppError> {
        if self.approval_state != AgentApprovalState::Pending {
            return Err(agent_approval_token_invalid("绑定已进入终态，令牌不再有效"));
        }
        if proposal_id != self.proposal_id {
            return Err(agent_approval_token_invalid("令牌与绑定不是同一条提案"));
        }
        let expected = self
            .approval_token_hash
            .as_ref()
            .ok_or_else(|| agent_approval_token_invalid("绑定没有已签发的令牌"))?;
        let issued_at = self
            .issued_at
            .ok_or_else(|| agent_approval_token_invalid("绑定缺少令牌签发时间"))?;
        if issued_at.0 < self.created_at.0 || issued_at.0 >= self.expires_at.0 {
            return Err(agent_approval_token_invalid("令牌签发时间已超出有效期"));
        }
        let presented = token.bind(proposal_id, proposal_digest)?;
        if !constant_time_eq(expected.as_str().as_bytes(), presented.as_str().as_bytes()) {
            return Err(agent_approval_token_invalid("令牌与已签发的令牌不一致"));
        }
        Ok(())
    }

    pub fn with_approval_state(
        &self,
        approval_state: AgentApprovalState,
    ) -> Result<Self, AppError> {
        self.approval_state.validate_transition(approval_state)?;
        let mut next = self.clone();
        next.approval_state = approval_state;
        if approval_state.is_terminal() {
            // 终态不得携带令牌：应用/拒绝/过期后，旧令牌必须在同一份事实里被清除。
            next.approval_token_hash = None;
            next.issued_at = None;
        }
        next.validate()?;
        Ok(next)
    }

    pub fn validate(&self) -> Result<(), AppError> {
        validate_agent_binding_ids(
            self.session_id,
            self.request_id,
            self.context_snapshot_id,
            self.proposal_id,
        )?;
        validate_agent_binding_context_hash(&self.context_hash)?;
        self.subject.validate()?;
        if self.created_at.0 < 0 {
            return Err(invalid_agent_binding("创建时间不能是负数"));
        }
        if self.expires_at.0 <= self.created_at.0 {
            return Err(invalid_agent_binding("过期时间必须晚于创建时间"));
        }
        // 令牌摘要与签发时间同生同灭：只有一半的记录是损坏的事实。
        if self.approval_token_hash.is_some() != self.issued_at.is_some() {
            return Err(invalid_agent_binding("令牌摘要与签发时间必须同时存在"));
        }
        if let Some(issued_at) = self.issued_at {
            if self.approval_state != AgentApprovalState::Pending {
                return Err(invalid_agent_binding("终态绑定不得携带令牌"));
            }
            if issued_at.0 < self.created_at.0 || issued_at.0 >= self.expires_at.0 {
                return Err(invalid_agent_binding("令牌签发时间不在绑定有效期内"));
            }
        }
        Ok(())
    }

    pub const fn proposal_id(&self) -> SettingProposalId {
        self.proposal_id
    }

    pub const fn session_id(&self) -> AgentSessionId {
        self.session_id
    }

    pub const fn request_id(&self) -> AgentRequestId {
        self.request_id
    }

    pub const fn context_snapshot_id(&self) -> AgentContextSnapshotId {
        self.context_snapshot_id
    }

    pub fn context_hash(&self) -> &str {
        &self.context_hash
    }

    pub const fn subject(&self) -> AgentScopeSubject {
        self.subject
    }

    pub const fn action_kind(&self) -> AgentActionKind {
        self.action_kind
    }

    pub const fn created_at(&self) -> UtcMillis {
        self.created_at
    }

    pub const fn expires_at(&self) -> UtcMillis {
        self.expires_at
    }

    pub const fn approval_state(&self) -> AgentApprovalState {
        self.approval_state
    }

    /// 当前有效令牌的摘要；终态与未签发状态都是 `None`。
    pub fn approval_token_hash(&self) -> Option<&AgentApprovalTokenHash> {
        self.approval_token_hash.as_ref()
    }

    /// 当前有效令牌的签发时刻；与 [`AgentActionBinding::approval_token_hash`] 同生同灭。
    pub const fn issued_at(&self) -> Option<UtcMillis> {
        self.issued_at
    }

    /// 是否带着一枚可校验的令牌（等价于"摘要与签发时间都存在"）。
    pub fn has_approval_token(&self) -> bool {
        self.approval_token_hash.is_some()
    }
}

fn validate_agent_binding_ids(
    session_id: AgentSessionId,
    request_id: AgentRequestId,
    context_snapshot_id: AgentContextSnapshotId,
    proposal_id: SettingProposalId,
) -> Result<(), AppError> {
    validate_agent_binding_context_ids(session_id, request_id, context_snapshot_id)?;
    if proposal_id.as_uuid().is_nil() {
        return Err(invalid_agent_binding("动作绑定 ID 不能是 nil UUID"));
    }
    Ok(())
}

fn validate_agent_binding_context_ids(
    session_id: AgentSessionId,
    request_id: AgentRequestId,
    context_snapshot_id: AgentContextSnapshotId,
) -> Result<(), AppError> {
    if session_id.as_uuid().is_nil()
        || request_id.as_uuid().is_nil()
        || context_snapshot_id.as_uuid().is_nil()
    {
        return Err(invalid_agent_binding("动作绑定 ID 不能是 nil UUID"));
    }
    Ok(())
}

fn validate_agent_binding_context_hash(value: &str) -> Result<(), AppError> {
    if !is_canonical_digest(value) {
        return Err(invalid_agent_binding("动作绑定 context_hash 非法"));
    }
    Ok(())
}

/// Agent 动作绑定事实非法（字段缺失/自相矛盾）。`detail` 只允许是静态字面量，
/// 不得拼接任何令牌材料。
fn invalid_agent_binding(detail: &'static str) -> AppError {
    AppError::new(
        "AGENT_ACTION_BINDING_INVALID",
        ErrorKind::Validation,
        format!("Agent 动作绑定非法：{detail}"),
        false,
    )
}

/// 一次性审批令牌的稳定错误：形状非法、被拒绝的签发、绑定不匹配、终态失效。
///
/// `detail` 是 `&'static str` 而不是 `String`：令牌原文（或被拒绝的"接近正确"
/// 输入）因此**在类型层面**无法被拼进错误消息，也无法随错误冒泡到日志/回执。
fn agent_approval_token_invalid(detail: &'static str) -> AppError {
    AppError::new(
        "AGENT_APPROVAL_TOKEN_INVALID",
        ErrorKind::Validation,
        format!("Agent 审批令牌非法：{detail}"),
        false,
    )
}

/// 定长字节比较：不因首个不同字节提前返回。
///
/// 比较对象是"存储的令牌摘要"与"提交令牌重算出的摘要"，两者都不是秘密，
/// 因此这里防的不是摘要泄漏，而是**校验路径本身**不应随输入逐字节变化
/// ——为将来把同一形状用于更敏感的比较留出正确实现。
fn constant_time_eq(left: &[u8], right: &[u8]) -> bool {
    if left.len() != right.len() {
        return false;
    }
    let mut diff = 0u8;
    for (a, b) in left.iter().zip(right.iter()) {
        diff |= a ^ b;
    }
    diff == 0
}

// ---------- 上下文片段与引用 ----------

/// Segment id：**闭合校验**的不透明短 token（非空、字符集受限、长度受限）。
///
/// 只有通过校验的字符串能成为 segment id，避免空白/控制字符/超长字符串
/// 变成"看起来像 ID 的自由文本"。
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct AgentContextSegmentId(String);

impl AgentContextSegmentId {
    pub fn new(value: impl Into<String>) -> Result<Self, AppError> {
        let value = value.into();
        validate_segment_id(&value)?;
        Ok(Self(value))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for AgentContextSegmentId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl FromStr for AgentContextSegmentId {
    type Err = AppError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Self::new(s)
    }
}

impl Serialize for AgentContextSegmentId {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(&self.0)
    }
}

impl<'de> Deserialize<'de> for AgentContextSegmentId {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let value = String::deserialize(deserializer)?;
        Self::new(value).map_err(|error| serde::de::Error::custom(error.user_message().to_owned()))
    }
}

/// 上下文片段的来源类别（闭合集合）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AgentContextSourceKind {
    /// 电子书正文。
    BookText,
    /// 漫画页文本（OCR/译文）。
    ComicPageText,
    /// 视频字幕。
    VideoSubtitle,
    /// 期刊文章正文。
    PeriodicalArticle,
    /// 用户当前的选中内容。
    UserSelection,
    /// 作品/版本元数据（标题、作者、简介等）。
    Metadata,
}

/// 一段进入推理的上下文文本。
///
/// `locator_range` 复用既有 [`Locator`]：它描述"这段文本来自哪里"，
/// 而不是另一套定位体系。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct AgentContextSegment {
    pub segment_id: AgentContextSegmentId,
    pub source_kind: AgentContextSourceKind,
    pub locator_range: Option<Locator>,
    pub text: String,
}

/// 对某个 segment 文本范围的引用（审计"这句话是从哪来的"）。
///
/// 区间按 Unicode 标量计数，`char_start < char_end`，且必须落在
/// 被引用 segment 的文本长度内。
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct AgentCitation {
    pub segment_id: AgentContextSegmentId,
    pub char_start: u32,
    pub char_end: u32,
}

impl AgentCitation {
    pub fn new(
        segment_id: AgentContextSegmentId,
        char_start: u32,
        char_end: u32,
    ) -> Result<Self, AppError> {
        let citation = Self {
            segment_id,
            char_start,
            char_end,
        };
        if citation.char_start >= citation.char_end {
            return Err(invalid_citation("引用区间必须满足 start < end"));
        }
        Ok(citation)
    }
}

/// 隐私描述：**必须自述否含凭据/绝对路径/整部作品**。
///
/// 这是快照的自我声明位，`validate` 拒绝任何"承认自己含凭据/路径/整部作品"的
/// 快照——把它们挡在上下文构造边界，而不是等到写日志或发给远端时才发现。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct AgentPrivacyDescriptor {
    /// 是否含个人数据（进度、历史、笔记等）。
    pub contains_personal_data: bool,
    /// 是否含凭据/API key（必须为 false）。
    pub contains_credentials: bool,
    /// 是否含绝对路径（必须为 false）。
    pub contains_absolute_paths: bool,
    /// 是否含整部作品内容（必须为 false：上下文只能是片段）。
    pub contains_full_work: bool,
    pub redaction: AgentRedactionMode,
}

/// 脱敏模式（闭合集合）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AgentRedactionMode {
    /// 未做额外脱敏（只允许在 `contains_personal_data = false` 时使用）。
    None,
    /// 结构性脱敏：只保留范围与结构，剥掉可识别个人的内容。
    Structural,
}

impl Default for AgentPrivacyDescriptor {
    fn default() -> Self {
        Self {
            contains_personal_data: false,
            contains_credentials: false,
            contains_absolute_paths: false,
            contains_full_work: false,
            redaction: AgentRedactionMode::None,
        }
    }
}

impl AgentPrivacyDescriptor {
    pub fn validate(&self) -> Result<(), AppError> {
        if self.contains_credentials {
            return Err(invalid_privacy("上下文不允许包含凭据"));
        }
        if self.contains_absolute_paths {
            return Err(invalid_privacy("上下文不允许包含绝对路径"));
        }
        if self.contains_full_work {
            return Err(invalid_privacy("上下文不允许包含整部作品"));
        }
        if self.contains_personal_data && self.redaction == AgentRedactionMode::None {
            return Err(invalid_privacy("含个人数据的上下文必须经过脱敏"));
        }
        Ok(())
    }
}

// ---------- 快照 ----------

/// `context_hash` 唯一覆盖的 canonical 载荷（**不含** snapshot id 与哈希自身）。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct AgentContextPayload {
    pub version: u32,
    pub subject: AgentSubject,
    pub locator: Option<Locator>,
    pub locator_confidence: AgentLocatorConfidence,
    pub boundary_mode: AgentBoundaryMode,
    /// 内容版本 token（不透明短字符串，标识"这段内容取自哪一版"）。
    pub content_revision: String,
    /// 源内容摘要（若调用方已知）；形状与提案摘要一致：64 位 ASCII 小写十六进制。
    pub content_hash: Option<String>,
    pub segments: Vec<AgentContextSegment>,
    pub citations: Vec<AgentCitation>,
    pub privacy: AgentPrivacyDescriptor,
}

impl AgentContextPayload {
    /// 构造一个空载荷骨架（segments/citations 由调用方填充）。
    pub fn new(
        subject: AgentSubject,
        locator: Option<Locator>,
        locator_confidence: AgentLocatorConfidence,
        boundary_mode: AgentBoundaryMode,
        content_revision: impl Into<String>,
    ) -> Self {
        Self {
            version: AGENT_CONTEXT_PAYLOAD_VERSION,
            subject,
            locator,
            locator_confidence,
            boundary_mode,
            content_revision: content_revision.into(),
            content_hash: None,
            segments: Vec::new(),
            citations: Vec::new(),
            privacy: AgentPrivacyDescriptor::default(),
        }
    }

    /// 完整语义校验（构造与从存储恢复共用同一条规则）。
    pub fn validate(&self) -> Result<(), AppError> {
        if self.version != AGENT_CONTEXT_PAYLOAD_VERSION {
            return Err(AppError::new(
                "AGENT_CONTEXT_UNSUPPORTED_VERSION",
                ErrorKind::Unsupported,
                "Agent 上下文载荷版本不受支持",
                false,
            ));
        }
        self.subject.validate()?;
        if let Some(locator) = &self.locator {
            validate_agent_locator(locator)?;
        }
        validate_content_revision(&self.content_revision)?;
        if let Some(content_hash) = &self.content_hash {
            if !is_canonical_digest(content_hash) {
                return Err(invalid_context(
                    "content_hash 必须是 64 位 ASCII 小写十六进制",
                ));
            }
        }
        self.validate_boundary()?;
        self.validate_segments()?;
        self.validate_citations()?;
        self.privacy.validate()
    }

    /// 边界模式与 locator/置信度必须自洽。
    fn validate_boundary(&self) -> Result<(), AppError> {
        match self.boundary_mode {
            AgentBoundaryMode::UserProvided => Ok(()),
            AgentBoundaryMode::StrictToLocator | AgentBoundaryMode::CurrentRange => {
                if self.locator.is_none() {
                    return Err(invalid_boundary("严格/当前范围边界必须携带 locator"));
                }
                if self.locator_confidence == AgentLocatorConfidence::Unresolved {
                    return Err(invalid_boundary(
                        "未解析的 locator 不能作为严格/当前范围边界",
                    ));
                }
                Ok(())
            }
        }
    }

    fn validate_segments(&self) -> Result<(), AppError> {
        if self.segments.len() > AGENT_CONTEXT_MAX_SEGMENTS {
            return Err(context_too_large("segment 数量超过上限"));
        }
        let mut seen: HashSet<&str> = HashSet::with_capacity(self.segments.len());
        let mut total_chars = 0usize;
        for segment in &self.segments {
            if !seen.insert(segment.segment_id.as_str()) {
                return Err(invalid_segment("segment id 重复"));
            }
            total_chars = total_chars.saturating_add(validate_segment_text(&segment.text)?);
            // locator_range 与 payload.locator 走同一条 Agent 边界校验：
            // 其字符串事实同样不得携带 endpoint/绝对路径。
            if let Some(locator_range) = &segment.locator_range {
                validate_agent_locator(locator_range)?;
            }
        }
        if total_chars > AGENT_CONTEXT_MAX_TOTAL_CHARS {
            return Err(context_too_large("上下文文本总量超过上限"));
        }
        Ok(())
    }

    fn validate_citations(&self) -> Result<(), AppError> {
        if self.citations.len() > AGENT_CONTEXT_MAX_CITATIONS {
            return Err(context_too_large("citation 数量超过上限"));
        }
        let mut seen: HashSet<(&str, u32, u32)> = HashSet::with_capacity(self.citations.len());
        for citation in &self.citations {
            let segment = self
                .segments
                .iter()
                .find(|segment| segment.segment_id == citation.segment_id)
                .ok_or_else(|| unknown_segment("citation 引用了不存在的 segment"))?;
            if citation.char_start >= citation.char_end {
                return Err(invalid_citation("引用区间必须满足 start < end"));
            }
            let text_chars = segment.text.chars().count();
            if citation.char_end as usize > text_chars {
                return Err(invalid_citation("引用区间超出 segment 文本长度"));
            }
            if !seen.insert((
                citation.segment_id.as_str(),
                citation.char_start,
                citation.char_end,
            )) {
                return Err(AppError::new(
                    "AGENT_CONTEXT_DUPLICATE_CITATION",
                    ErrorKind::Validation,
                    "同一引用区间不能重复出现",
                    false,
                ));
            }
        }
        Ok(())
    }
}

/// 一次推理所看到的上下文事实。
///
/// 字段私有：`canonical_json` 与 `context_hash` 只能由构造/恢复路径产生，
/// 外部无法在构造后改写其中一个而让另一个保持自洽。
#[derive(Debug, Clone, PartialEq)]
pub struct AgentContextSnapshot {
    id: AgentContextSnapshotId,
    payload: AgentContextPayload,
    canonical_json: String,
    context_hash: String,
}

impl AgentContextSnapshot {
    /// 构造快照并固化 canonical 载荷与 `context_hash`。
    ///
    /// 顺序：先做语义校验（非法 Locator 等在这里就返回稳定错误码，
    /// 而不是在序列化阶段退化成 `SETTING_PROPOSAL_SERIALIZE_FAILED`），
    /// 再固化 canonical 载荷与摘要，最后用 [`AgentContextSnapshot::validate`]
    /// 做一致性校验。`context_hash` 是**不含自身**的 canonical 载荷的
    /// SHA-256 小写十六进制。
    pub fn new(id: AgentContextSnapshotId, payload: AgentContextPayload) -> Result<Self, AppError> {
        payload.validate()?;
        let canonical_json = canonical_json_of(&payload)?;
        let context_hash = canonical_digest(&canonical_json);
        let snapshot = Self {
            id,
            payload,
            canonical_json,
            context_hash,
        };
        snapshot.validate()?;
        Ok(snapshot)
    }

    /// 重新校验快照自身的 id / payload，并**重算** canonical 载荷与 `context_hash`。
    ///
    /// 任何"存储/内存里的 canonical 载荷或摘要与重算结果不一致"都按完整性
    /// 篡改处理（`AGENT_CONTEXT_INTEGRITY_MISMATCH`）；id nil 或 payload 语义非法
    /// 沿用各自的稳定错误码。`new` / `from_canonical_payload` 共用这条逻辑。
    pub fn validate(&self) -> Result<(), AppError> {
        if self.id.as_uuid().is_nil() {
            return Err(invalid_context("快照 ID 不能是 nil UUID"));
        }
        self.payload.validate()?;
        let canonical_json = canonical_json_of(&self.payload)?;
        if canonical_json != self.canonical_json {
            return Err(context_integrity_error(
                "canonical 载荷与内存中的载荷不一致",
            ));
        }
        if canonical_digest(&canonical_json) != self.context_hash {
            return Err(context_integrity_error("context_hash 与载荷不一致"));
        }
        Ok(())
    }

    /// 从存储恢复：载荷必须是 canonical 形式，且 `context_hash` 必须自洽。
    ///
    /// 任何键序/空白/字段改动（即使摘要跟着改）都会在这里被拒绝，
    /// 因为恢复路径要求"载荷 == 它的 canonical 重写"。
    pub fn from_canonical_payload(
        id: AgentContextSnapshotId,
        payload_json: &str,
        context_hash: &str,
    ) -> Result<Self, AppError> {
        let payload = parse_canonical_payload(payload_json, context_hash)?;
        Self::new(id, payload)
    }

    /// 只做完整性校验：不构造快照，也不做语义判断（语义由 `from_canonical_payload` 负责）。
    pub fn verify_integrity(payload_json: &str, context_hash: &str) -> Result<(), AppError> {
        parse_canonical_payload(payload_json, context_hash).map(|_| ())
    }

    pub fn id(&self) -> AgentContextSnapshotId {
        self.id
    }

    pub fn payload(&self) -> &AgentContextPayload {
        &self.payload
    }

    pub fn subject(&self) -> AgentSubject {
        self.payload.subject
    }

    pub fn locator(&self) -> Option<&Locator> {
        self.payload.locator.as_ref()
    }

    pub fn locator_confidence(&self) -> AgentLocatorConfidence {
        self.payload.locator_confidence
    }

    pub fn boundary_mode(&self) -> AgentBoundaryMode {
        self.payload.boundary_mode
    }

    pub fn content_revision(&self) -> &str {
        &self.payload.content_revision
    }

    pub fn content_hash(&self) -> Option<&str> {
        self.payload.content_hash.as_deref()
    }

    pub fn segments(&self) -> &[AgentContextSegment] {
        &self.payload.segments
    }

    pub fn citations(&self) -> &[AgentCitation] {
        &self.payload.citations
    }

    pub fn privacy(&self) -> AgentPrivacyDescriptor {
        self.payload.privacy
    }

    /// canonical 载荷 JSON（`context_hash` 就是它的 SHA-256）。
    pub fn canonical_json(&self) -> &str {
        &self.canonical_json
    }

    /// canonical 载荷的 SHA-256 小写十六进制。
    pub fn context_hash(&self) -> &str {
        &self.context_hash
    }
}

fn parse_canonical_payload(
    payload_json: &str,
    context_hash: &str,
) -> Result<AgentContextPayload, AppError> {
    // 摘要形状先收敛：长度对但含大写/非十六进制字符的不是"另一种写法"，是伪造或损坏。
    if !is_canonical_digest(context_hash) {
        return Err(context_integrity_error(
            "context_hash 不是 SHA-256 小写十六进制",
        ));
    }
    let payload: AgentContextPayload = serde_json::from_str(payload_json)
        .map_err(|_| context_integrity_error("上下文载荷无法解析"))?;
    if canonical_json_of(&payload)? != payload_json {
        return Err(context_integrity_error("上下文载荷不是 canonical 形式"));
    }
    if canonical_digest(payload_json) != context_hash {
        return Err(context_integrity_error("context_hash 与载荷不一致"));
    }
    Ok(payload)
}

// ---------- 能力清单 ----------

/// Agent 能力（闭合集合）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AgentCapability {
    /// 读取设置。
    SettingsRead,
    /// 创建设置变更提案（不直接生效）。
    SettingsProposal,
    /// 读取书库摘要。
    LibrarySummaryRead,
    /// 读取设置来源分层。
    SettingSourcesRead,
    /// 读取版本/媒体条目的资源偏好。
    ResourcePreferenceRead,
    /// 创建资源偏好变更提案（不直接生效）。
    ResourcePreferenceProposal,
    /// 读取媒体条目的声明式能力。
    MediaCapabilitiesRead,
    /// 读取本地引导状态。
    OnboardingRead,
    /// 创建元数据变更提案。
    MetadataProposal,
    /// 创建重命名提案。
    RenameProposal,
    /// 读取凭据。
    SecretRead,
    /// 写文件系统。
    FilesystemWrite,
}

impl AgentCapability {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::SettingsRead => "settings_read",
            Self::SettingsProposal => "settings_proposal",
            Self::LibrarySummaryRead => "library_summary_read",
            Self::SettingSourcesRead => "setting_sources_read",
            Self::ResourcePreferenceRead => "resource_preference_read",
            Self::ResourcePreferenceProposal => "resource_preference_proposal",
            Self::MediaCapabilitiesRead => "media_capabilities_read",
            Self::OnboardingRead => "onboarding_read",
            Self::MetadataProposal => "metadata_proposal",
            Self::RenameProposal => "rename_proposal",
            Self::SecretRead => "secret_read",
            Self::FilesystemWrite => "filesystem_write",
        }
    }
}

/// 能力开关集合（manifest 里 `capabilities` 对象的值对象）。
///
/// 关闭优先的落点：每个字段都带 `#[serde(default)]`，缺省一律 **false**，
/// 因此没有任何 JSON 写法能在不显式写 `true` 的情况下拿到能力；
/// `deny_unknown_fields` 让"没写进契约的能力名"（例如 `shellExec`）直接解析失败，
/// 而不是被静默忽略。`Default` 即全关。
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct AgentCapabilitySet {
    #[serde(default)]
    pub settings_read: bool,
    #[serde(default)]
    pub settings_proposal: bool,
    #[serde(default)]
    pub library_summary_read: bool,
    #[serde(default)]
    pub setting_sources_read: bool,
    #[serde(default)]
    pub resource_preference_read: bool,
    #[serde(default)]
    pub resource_preference_proposal: bool,
    #[serde(default)]
    pub media_capabilities_read: bool,
    #[serde(default)]
    pub onboarding_read: bool,
    #[serde(default)]
    pub metadata_proposal: bool,
    #[serde(default)]
    pub rename_proposal: bool,
    #[serde(default)]
    pub secret_read: bool,
    #[serde(default)]
    pub filesystem_write: bool,
}

impl AgentCapabilitySet {
    /// 是否声明了某项能力（不判断该能力是否已实现）。
    pub const fn grants(&self, capability: AgentCapability) -> bool {
        match capability {
            AgentCapability::SettingsRead => self.settings_read,
            AgentCapability::SettingsProposal => self.settings_proposal,
            AgentCapability::LibrarySummaryRead => self.library_summary_read,
            AgentCapability::SettingSourcesRead => self.setting_sources_read,
            AgentCapability::ResourcePreferenceRead => self.resource_preference_read,
            AgentCapability::ResourcePreferenceProposal => self.resource_preference_proposal,
            AgentCapability::MediaCapabilitiesRead => self.media_capabilities_read,
            AgentCapability::OnboardingRead => self.onboarding_read,
            AgentCapability::MetadataProposal => self.metadata_proposal,
            AgentCapability::RenameProposal => self.rename_proposal,
            AgentCapability::SecretRead => self.secret_read,
            AgentCapability::FilesystemWrite => self.filesystem_write,
        }
    }
}

/// Agent 能力清单。
///
/// 契约形状（`AI_AGENT_RUNTIME.md` 批准的嵌套形式，不再有扁平字段）：
/// ```json
/// {"agentApiVersion":1,"capabilities":{"settingsRead":true,...}}
/// ```
/// 关闭优先：默认值与 [`AgentCapabilityManifest::for_current_slice`] 只声明
/// **本切片已实现**的读取/提案能力，其余高风险能力一律 false；
/// `validate` 拒绝任何声明了未实现能力的清单——manifest 是"声明"，不是"授权"。
/// 未知字段由 `deny_unknown_fields` 在顶层与 `capabilities` 两层分别拒绝。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct AgentCapabilityManifest {
    /// 协议版本，必须等于 [`AGENT_API_VERSION`]。
    pub agent_api_version: u32,
    /// 能力开关集合；整个对象缺省时退化为全关（fail-closed），不会退化为全开。
    #[serde(default)]
    pub capabilities: AgentCapabilitySet,
}

impl Default for AgentCapabilityManifest {
    fn default() -> Self {
        Self::for_current_slice()
    }
}

impl AgentCapabilityManifest {
    /// 本切片的能力清单：冻结的 7 个只读能力 + 2 个设置/资源偏好提案能力。
    pub const fn for_current_slice() -> Self {
        Self {
            agent_api_version: AGENT_API_VERSION,
            capabilities: AgentCapabilitySet {
                settings_read: true,
                settings_proposal: true,
                library_summary_read: true,
                setting_sources_read: true,
                resource_preference_read: true,
                resource_preference_proposal: true,
                media_capabilities_read: true,
                onboarding_read: true,
                metadata_proposal: false,
                rename_proposal: false,
                secret_read: false,
                filesystem_write: false,
            },
        }
    }

    /// 校验清单自身：版本必须匹配，且不得声明本切片未实现的能力。
    ///
    /// 未知/不受支持的字段不在这里判断——它们在反序列化阶段就被
    /// `deny_unknown_fields` 拒绝，根本进不到校验。
    pub fn validate(&self) -> Result<(), AppError> {
        if self.agent_api_version != AGENT_API_VERSION {
            return Err(AppError::new(
                "AGENT_API_VERSION_UNSUPPORTED",
                ErrorKind::Unsupported,
                format!("Agent 协议版本不受支持：{}", self.agent_api_version),
                false,
            ));
        }
        for capability in [
            AgentCapability::MetadataProposal,
            AgentCapability::RenameProposal,
            AgentCapability::SecretRead,
            AgentCapability::FilesystemWrite,
        ] {
            if self.grants(capability) {
                return Err(AppError::new(
                    "AGENT_CAPABILITY_NOT_IMPLEMENTED",
                    ErrorKind::Unsupported,
                    format!("本版本尚未实现能力：{}", capability.as_str()),
                    false,
                ));
            }
        }
        Ok(())
    }

    /// 是否声明了某项能力（不判断该能力是否已实现）。
    pub const fn grants(&self, capability: AgentCapability) -> bool {
        self.capabilities.grants(capability)
    }

    /// 要求某项能力：清单先自校验，再检查是否声明。
    pub fn require(&self, capability: AgentCapability) -> Result<(), AppError> {
        self.validate()?;
        if !self.grants(capability) {
            return Err(AppError::new(
                "AGENT_CAPABILITY_NOT_GRANTED",
                ErrorKind::Forbidden,
                format!("能力未被授予：{}", capability.as_str()),
                false,
            ));
        }
        Ok(())
    }
}

// ---------- 校验辅助 ----------

fn validate_segment_id(value: &str) -> Result<(), AppError> {
    if value.is_empty() || value.chars().count() > AGENT_SEGMENT_ID_MAX_CHARS {
        return Err(invalid_segment("segment id 长度非法"));
    }
    if !value
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
    {
        return Err(invalid_segment("segment id 只允许不透明 token 字符"));
    }
    Ok(())
}

fn validate_content_revision(value: &str) -> Result<(), AppError> {
    if value.is_empty() || value.chars().count() > AGENT_CONTENT_REVISION_MAX_CHARS {
        return Err(invalid_context("content_revision 长度非法"));
    }
    if !value
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
    {
        return Err(invalid_context("content_revision 只允许不透明 token 字符"));
    }
    Ok(())
}

/// 校验 segment 文本，返回其字符数。
fn validate_segment_text(text: &str) -> Result<usize, AppError> {
    if text.trim().is_empty() {
        return Err(invalid_segment("segment 文本不能为空"));
    }
    let chars = text.chars().count();
    if chars > AGENT_CONTEXT_MAX_SEGMENT_CHARS {
        return Err(context_too_large("segment 文本超过长度上限"));
    }
    // 换行/制表符是正文的正常组成；其余控制字符不是。
    if text
        .chars()
        .any(|c| c.is_control() && c != '\n' && c != '\r' && c != '\t')
    {
        return Err(invalid_segment("segment 文本不允许控制字符"));
    }
    // 隐私不能只信 boolean 声明：在边界上对明显敏感文本 fail-closed。
    if contains_sensitive_text(text) {
        return Err(sensitive_text_error());
    }
    Ok(chars)
}

/// Agent 专用边界的 locator 校验（**不改动通用 [`Locator`]**）。
///
/// 两步走，语义与通用 Locator 保持隔离：
/// 1. 先复用 [`Locator::validate`] 的数值规则（progression 必须有限且在 0..=1）；
/// 2. 再把 locator 的 canonical 序列化送入既有的 [`contains_sensitive_text`]。
///
/// 这样 `BookLocator.publication_resource` / `format_locator`、`ArticleLocator`、
/// `BookLocator`、`PdfLocator` 的 `text_anchor`、`GenericLocator` 的 key/value
/// 等**任意字符串字段**一旦携带 `http(s)://`/`file://`/`ftp://` endpoint 或
/// Windows/Unix/UNC 绝对路径，就会在快照边界 fail-closed，堵住"正文被扫描、
/// 但 locator 里的 endpoint/绝对路径绕过"的隐私缺口。
///
/// 扫描对象是序列化后的字符串事实，因此未来新增的字符串字段会自动纳入；
/// 普通资源 token（`chapter-03.xhtml`、`OEBPS/text/chapter01.xhtml`）、
/// 章节名与中文正文不会命中。错误码复用 segment 文本扫描的同一个
/// `AGENT_CONTEXT_SENSITIVE_TEXT`。
fn validate_agent_locator(locator: &Locator) -> Result<(), AppError> {
    locator
        .validate()
        .map_err(|detail| invalid_locator(detail.to_owned()))?;
    let serialized = serde_json::to_string(locator)
        .map_err(|_| invalid_locator("locator 无法序列化".to_owned()))?;
    if contains_sensitive_text(&serialized) {
        return Err(sensitive_text_error());
    }
    Ok(())
}

/// 凭据赋值关键词（ASCII 小写；命中后还需满足 `=`/`:` + 足够长的值才算）。
const CREDENTIAL_KEYWORDS: &[&str] = &[
    "api_key",
    "apikey",
    "api-key",
    "api_token",
    "apitoken",
    "api-token",
    "access_key",
    "accesskey",
    "access-key",
    "access_token",
    "accesstoken",
    "access-token",
    "secret_key",
    "secretkey",
    "secret-key",
    "client_secret",
    "clientsecret",
    "client-secret",
    "private_key",
    "privatekey",
    "private-key",
    "auth_token",
    "authtoken",
    "auth-token",
    "refresh_token",
    "refreshtoken",
    "refresh-token",
    "bearer_token",
    "bearertoken",
    "bearer-token",
    "password",
    "passwd",
    "passphrase",
    "authorization",
    "credential",
    "credentials",
];

/// 凭据赋值的值最少字符数（真实 key/token 远长于此）。
const CREDENTIAL_VALUE_MIN_CHARS: usize = 6;
/// `bearer` 之后 token 的最少字符数。
const BEARER_TOKEN_MIN_CHARS: usize = 8;

/// 单个**自由文本设置值**的敏感结构扫描（无正则、无新依赖）。
///
/// 与 [`contains_sensitive_text`] 的区别：这里扫描的是一个独立的设置值，
/// 不是正文片段，因此
/// - 不做凭据**关键词**扫描（`custom_background` 这类合法字段名/值本身就可能含
///   `secret` 前缀，误判会把正常设置判成敏感），只保留"关键词 + `=`/`:` + 足够长
///   的值"这种几乎不可能出现在合法设置值里的赋值结构；
/// - 其余三类（`bearer` 片段、endpoint、绝对路径）与正文扫描共用同一实现。
///
/// 允许 `#rrggbb` 颜色、字体族名与普通中文，不误伤。
///
/// 这是**单值**层面的唯一判据：`AgentReadingSnapshot::from_settings`、
/// [`redact_reading_patch_for_agent`]、[`redact_setting_value_for_agent`] 与
/// [`validate_agent_proposal_free_text`] 全部走它，避免"读路径脱敏、写路径放行"
/// 这类只有一处实现的漂移。
pub fn contains_sensitive_setting_value(text: &str) -> bool {
    let lowered = text.to_ascii_lowercase();
    contains_credential_assignment(&lowered)
        || contains_bearer_fragment(&lowered)
        || contains_endpoint(&lowered)
        || contains_absolute_path(&lowered)
}

/// 明显敏感文本的 fail-closed 扫描（无正则、无新依赖）。
///
/// 只匹配**几乎不可能出现在正常正文里**的结构：凭据赋值、`bearer` token 片段、
/// `http(s)://`/`file://` endpoint、Windows/Unix/UNC 绝对路径。命中即拒绝，
/// 从而不把 raw arguments、API key 或完整 endpoint 保留进快照。
///
/// 普通中文正文、换行与制表符不受影响；扫描只针对 ASCII 结构，不做宽泛的
/// 文本禁止。
/// 该函数只判断"是否命中明显敏感结构"，不返回任何命中内容。
pub fn contains_sensitive_text(text: &str) -> bool {
    let lowered = text.to_ascii_lowercase();
    contains_credential_assignment(&lowered)
        || contains_bearer_fragment(&lowered)
        || contains_endpoint(&lowered)
        || contains_absolute_path(&lowered)
}

/// 关键词是否以独立 token 出现（前一个字符不能是字母数字/`_`/`-`）。
fn at_token_boundary(text: &str, start: usize) -> bool {
    text[..start]
        .chars()
        .next_back()
        .is_none_or(|c| !(c.is_ascii_alphanumeric() || c == '_' || c == '-'))
}

fn contains_credential_assignment(text: &str) -> bool {
    for keyword in CREDENTIAL_KEYWORDS {
        let mut from = 0;
        while let Some(offset) = text[from..].find(keyword) {
            let start = from + offset;
            let end = start + keyword.len();
            if at_token_boundary(text, start) {
                let rest = text[end..].trim_start_matches([' ', '\t']);
                let after_sep = rest.strip_prefix('=').or_else(|| rest.strip_prefix(':'));
                if let Some(value) = after_sep {
                    let value_len = value
                        .trim_start_matches([' ', '\t'])
                        .chars()
                        .take_while(|c| !c.is_whitespace())
                        .count();
                    if value_len >= CREDENTIAL_VALUE_MIN_CHARS {
                        return true;
                    }
                }
            }
            from = end;
        }
    }
    false
}

fn contains_bearer_fragment(text: &str) -> bool {
    let mut from = 0;
    while let Some(offset) = text[from..].find("bearer") {
        let start = from + offset;
        let end = start + "bearer".len();
        if at_token_boundary(text, start) {
            let rest = &text[end..];
            let after_ws = rest.trim_start_matches([' ', '\t']);
            if after_ws.len() != rest.len() {
                let token_len = after_ws.chars().take_while(|c| !c.is_whitespace()).count();
                if token_len >= BEARER_TOKEN_MIN_CHARS {
                    return true;
                }
            }
        }
        from = end;
    }
    false
}

fn contains_endpoint(text: &str) -> bool {
    ["http://", "https://", "file://", "ftp://"]
        .iter()
        .any(|scheme| text.contains(scheme))
}

fn contains_absolute_path(text: &str) -> bool {
    contains_windows_drive_path(text) || text.contains("\\\\") || contains_unix_absolute_path(text)
}

/// Windows 盘符绝对路径：`X:\...` 或 `X:/...`。
fn contains_windows_drive_path(text: &str) -> bool {
    let bytes = text.as_bytes();
    let mut index = 0;
    while index + 2 < bytes.len() {
        if bytes[index].is_ascii_alphabetic()
            && bytes[index + 1] == b':'
            && (bytes[index + 2] == b'\\' || bytes[index + 2] == b'/')
            && (index == 0 || !bytes[index - 1].is_ascii_alphanumeric())
        {
            return true;
        }
        index += 1;
    }
    false
}

/// Unix 绝对路径：位于文本/空白/开括号等边界之后、以字母开头的 `/...`。
///
/// 不会误伤 `和/或`、`TCP/IP`、`2024/09/17` 这类"斜杠两侧是普通字符"的正文。
fn contains_unix_absolute_path(text: &str) -> bool {
    let mut prev: Option<char> = None;
    for (index, c) in text.char_indices() {
        if c == '/' {
            let boundary = match prev {
                None => true,
                Some(p) => {
                    p.is_whitespace()
                        || matches!(
                            p,
                            '"' | '\'' | '(' | '[' | '{' | '=' | ':' | '>' | ',' | ';' | '~'
                        )
                }
            };
            if boundary {
                if let Some(next) = text[index + 1..].chars().next() {
                    if next.is_ascii_alphabetic() {
                        return true;
                    }
                }
            }
        }
        prev = Some(c);
    }
    false
}

// ---------- 稳定错误 ----------

fn invalid_subject(detail: &'static str) -> AppError {
    AppError::new(
        "AGENT_CONTEXT_INVALID_SUBJECT",
        ErrorKind::Validation,
        format!("Agent 上下文范围非法：{detail}"),
        false,
    )
}

fn invalid_context(detail: impl Into<String>) -> AppError {
    AppError::new(
        "AGENT_CONTEXT_INVALID",
        ErrorKind::Validation,
        detail.into(),
        false,
    )
}

fn invalid_locator(detail: String) -> AppError {
    AppError::new(
        "AGENT_CONTEXT_INVALID_LOCATOR",
        ErrorKind::Validation,
        format!("上下文 locator 非法：{detail}"),
        false,
    )
}

fn invalid_segment(detail: &'static str) -> AppError {
    AppError::new(
        "AGENT_CONTEXT_INVALID_SEGMENT",
        ErrorKind::Validation,
        format!("上下文片段非法：{detail}"),
        false,
    )
}

fn invalid_citation(detail: &'static str) -> AppError {
    AppError::new(
        "AGENT_CONTEXT_INVALID_CITATION",
        ErrorKind::Validation,
        format!("上下文引用非法：{detail}"),
        false,
    )
}

fn unknown_segment(detail: &'static str) -> AppError {
    AppError::new(
        "AGENT_CONTEXT_UNKNOWN_SEGMENT",
        ErrorKind::Validation,
        format!("上下文引用非法：{detail}"),
        false,
    )
}

fn invalid_boundary(detail: &'static str) -> AppError {
    AppError::new(
        "AGENT_CONTEXT_INVALID_BOUNDARY",
        ErrorKind::Validation,
        format!("上下文边界非法：{detail}"),
        false,
    )
}

fn context_too_large(detail: &'static str) -> AppError {
    AppError::new(
        "AGENT_CONTEXT_TOO_LARGE",
        ErrorKind::Validation,
        format!("上下文超出上限：{detail}"),
        false,
    )
}

fn invalid_privacy(detail: &'static str) -> AppError {
    AppError::new(
        "AGENT_CONTEXT_INVALID_PRIVACY",
        ErrorKind::Validation,
        format!("上下文隐私声明非法：{detail}"),
        false,
    )
}

/// segment 文本命中明显敏感结构（凭据/endpoint/绝对路径），fail-closed 拒绝。
fn sensitive_text_error() -> AppError {
    AppError::new(
        "AGENT_CONTEXT_SENSITIVE_TEXT",
        ErrorKind::Validation,
        "上下文片段疑似包含凭据、endpoint 或绝对路径，已拒绝",
        false,
    )
}

/// 存储中的上下文快照被篡改或损坏。
pub fn context_integrity_error(detail: &'static str) -> AppError {
    AppError::new(
        "AGENT_CONTEXT_INTEGRITY_MISMATCH",
        ErrorKind::Internal,
        format!("Agent 上下文完整性校验失败：{detail}"),
        false,
    )
}

fn invalid_settings_context(detail: &'static str) -> AppError {
    AppError::new(
        "AGENT_SETTINGS_CONTEXT_INVALID",
        ErrorKind::Validation,
        format!("Agent 设置上下文非法：{detail}"),
        false,
    )
}

/// 存储中的设置上下文快照被篡改或损坏。
fn settings_context_integrity_error(detail: &'static str) -> AppError {
    AppError::new(
        "AGENT_SETTINGS_CONTEXT_INTEGRITY_MISMATCH",
        ErrorKind::Internal,
        format!("Agent 设置上下文完整性校验失败：{detail}"),
        false,
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ids::{AgentRequestId, AgentSessionId};
    use crate::locator::{ArticleLocator, BookLocator, GenericLocator, TextAnchor};

    fn segment_id(value: &str) -> AgentContextSegmentId {
        AgentContextSegmentId::new(value).unwrap()
    }

    fn segment(id: &str, text: &str) -> AgentContextSegment {
        AgentContextSegment {
            segment_id: segment_id(id),
            source_kind: AgentContextSourceKind::BookText,
            locator_range: None,
            text: text.to_owned(),
        }
    }

    fn article_locator(progression: Option<f32>) -> Locator {
        Locator::Article(ArticleLocator {
            block_id: Some("block-1".into()),
            progression,
            text_anchor: None,
        })
    }

    fn book_locator(resource: &str, format_locator: Option<&str>) -> Locator {
        Locator::Book(BookLocator {
            publication_resource: resource.to_owned(),
            progression: Some(0.5),
            text_anchor: None,
            format_locator: format_locator.map(str::to_owned),
        })
    }

    fn generic_locator(key: &str, value: &str) -> Locator {
        Locator::Generic(GenericLocator {
            key: key.to_owned(),
            value: value.to_owned(),
        })
    }

    fn article_locator_with_anchor(anchor: TextAnchor) -> Locator {
        Locator::Article(ArticleLocator {
            block_id: Some("block-1".into()),
            progression: Some(0.5),
            text_anchor: Some(anchor),
        })
    }

    fn subject() -> AgentSubject {
        AgentSubject::work(WorkId::new(), AgentContentKind::Book)
    }

    fn context_payload() -> AgentContextPayload {
        let mut payload = AgentContextPayload::new(
            subject(),
            Some(article_locator(Some(0.25))),
            AgentLocatorConfidence::Exact,
            AgentBoundaryMode::StrictToLocator,
            "rev-0001",
        );
        payload.segments = vec![segment("seg-1", "第一章：风从北面来。")];
        payload
            .segments
            .push(segment("seg-2", "第二章：雨落在旧城。"));
        payload.citations = vec![
            AgentCitation::new(segment_id("seg-1"), 0, 3).unwrap(),
            AgentCitation::new(segment_id("seg-2"), 4, 8).unwrap(),
        ];
        payload
    }

    fn snapshot() -> AgentContextSnapshot {
        AgentContextSnapshot::new(AgentContextSnapshotId::new(), context_payload()).unwrap()
    }

    /// 固定的 64 位小写十六进制样例：大小写/长度对比因此与随机内容无关。
    fn hex64() -> String {
        "0123456789abcdef".repeat(4)
    }

    /// 一条 pending 绑定，有效期 `[1000, 5000)`。
    fn action_binding() -> AgentActionBinding {
        AgentActionBinding::pending(
            SettingProposalId::new(),
            AgentActionBindingInput::new(
                AgentSessionId::new(),
                AgentRequestId::new(),
                AgentContextSnapshotId::new(),
                canonical_digest("agent-context"),
                subject(),
                AgentActionKind::SettingsProposal,
            )
            .unwrap(),
            UtcMillis::from_millis(1_000),
            UtcMillis::from_millis(5_000),
        )
        .unwrap()
    }

    /// 一枚绑定到该绑定所属提案与摘要的令牌，及其摘要。
    fn bound_token(
        binding: &AgentActionBinding,
        digest: &str,
    ) -> (AgentApprovalToken, AgentApprovalTokenHash) {
        let token = AgentApprovalToken::issue();
        let hash = token.bind(binding.proposal_id(), digest).unwrap();
        (token, hash)
    }

    /// 用该绑定的元数据走"带令牌"的持久化恢复路径。
    fn stored_with_token(
        binding: &AgentActionBinding,
        approval_state: AgentApprovalState,
        approval_token_hash: Option<String>,
        issued_at: Option<UtcMillis>,
    ) -> Result<AgentActionBinding, AppError> {
        AgentActionBinding::from_stored_with_token(
            binding.proposal_id(),
            binding.session_id(),
            binding.request_id(),
            binding.context_snapshot_id(),
            binding.context_hash().to_owned(),
            binding.subject(),
            binding.action_kind(),
            binding.created_at(),
            binding.expires_at(),
            approval_state,
            approval_token_hash,
            issued_at,
        )
    }

    #[test]
    fn agent_ids_roundtrip_through_json_and_strings() {
        let session = AgentSessionId::new();
        let request = AgentRequestId::new();
        let snapshot_id = AgentContextSnapshotId::new();
        assert_ne!(session.to_string(), request.to_string());
        assert_ne!(request.to_string(), snapshot_id.to_string());

        // 每个 ID 都必须能从字符串与 JSON 精确往返。
        assert_eq!(
            session,
            session.to_string().parse::<AgentSessionId>().unwrap()
        );
        assert_eq!(
            request,
            request.to_string().parse::<AgentRequestId>().unwrap()
        );
        assert_eq!(
            snapshot_id,
            snapshot_id
                .to_string()
                .parse::<AgentContextSnapshotId>()
                .unwrap()
        );

        let session_json = serde_json::to_string(&session).unwrap();
        let request_json = serde_json::to_string(&request).unwrap();
        let snapshot_json = serde_json::to_string(&snapshot_id).unwrap();
        for json in [&session_json, &request_json, &snapshot_json] {
            assert!(json.starts_with('"') && json.ends_with('"'), "{json}");
        }
        assert_eq!(session_json.trim_matches('"'), session.to_string());
        assert_eq!(
            serde_json::from_str::<AgentSessionId>(&session_json).unwrap(),
            session
        );
        assert_eq!(
            serde_json::from_str::<AgentRequestId>(&request_json).unwrap(),
            request
        );
        assert_eq!(
            serde_json::from_str::<AgentContextSnapshotId>(&snapshot_json).unwrap(),
            snapshot_id
        );

        assert!(serde_json::from_str::<AgentSessionId>("\"not-a-uuid\"").is_err());
        // nil UUID 是"未设置"的常见误写，不能当作合法快照身份。
        assert_eq!(
            AgentContextSnapshot::new(
                AgentContextSnapshotId::from_uuid(uuid::Uuid::nil()),
                context_payload()
            )
            .unwrap_err()
            .code()
            .as_str(),
            "AGENT_CONTEXT_INVALID"
        );
    }

    #[test]
    fn subject_requires_edition_for_media_items() {
        let work = WorkId::new();
        let edition = EditionId::new();
        let media_item = MediaItemId::new();

        assert_eq!(
            AgentSubject::new(work, None, Some(media_item), AgentContentKind::Book)
                .unwrap_err()
                .code()
                .as_str(),
            "AGENT_CONTEXT_INVALID_SUBJECT"
        );

        let ok = AgentSubject::media_item(work, edition, media_item, AgentContentKind::Book);
        ok.validate().unwrap();
        assert_eq!(ok.edition_id, Some(edition));
        assert_eq!(ok.media_item_id, Some(media_item));
        assert_eq!(
            AgentSubject::work(work, AgentContentKind::Comic).edition_id,
            None
        );
    }

    #[test]
    fn subject_covers_only_its_own_scope() {
        let work = WorkId::new();
        let edition = EditionId::new();
        let other_edition = EditionId::new();
        let media_item = MediaItemId::new();

        let work_scope = AgentSubject::work(work, AgentContentKind::Book);
        let edition_scope = AgentSubject::edition(work, edition, AgentContentKind::Book);
        let item_scope =
            AgentSubject::media_item(work, edition, media_item, AgentContentKind::Book);

        // 全局设置不属于任何作品范围。
        for scope in [work_scope, edition_scope, item_scope] {
            assert!(scope.covers(SettingTarget::global(
                crate::settings::SettingsSection::Reading
            )));
        }

        use crate::setting_proposal::SettingTarget;
        let edition_target = SettingTarget::edition(edition);
        let other_target = SettingTarget::edition(other_edition);
        let item_target = SettingTarget::media_item(edition, media_item);

        assert!(edition_scope.covers(edition_target));
        assert!(item_scope.covers(edition_target));
        assert!(
            !work_scope.covers(edition_target),
            "作品级范围不覆盖具体版本"
        );
        assert!(!edition_scope.covers(other_target));
        assert!(!work_scope.covers(item_target));
        assert!(
            !edition_scope.covers(item_target),
            "版本级范围不覆盖具体条目"
        );
        assert!(item_scope.covers(item_target));
    }

    #[test]
    fn unknown_enums_and_fields_are_rejected() {
        assert!(serde_json::from_str::<AgentContentKind>("\"podcast\"").is_err());
        assert!(serde_json::from_str::<AgentTask>("\"summarize_everything\"").is_err());
        assert!(serde_json::from_str::<AgentBoundaryMode>("\"whatever\"").is_err());
        assert!(serde_json::from_str::<AgentLocatorConfidence>("\"maybe\"").is_err());
        assert!(serde_json::from_str::<AgentContextSourceKind>("\"guess\"").is_err());
        assert!(serde_json::from_str::<AgentRedactionMode>("\"partial\"").is_err());

        let subject_json = serde_json::json!({
            "workId": WorkId::new(),
            "editionId": null,
            "mediaItemId": null,
            "contentKind": "book",
            "capabilities": ["filesystem_write"],
        });
        assert!(
            serde_json::from_value::<AgentSubject>(subject_json).is_err(),
            "未知字段（变相授权字段）必须拒绝"
        );

        let segment_json = serde_json::json!({
            "segmentId": "seg-1",
            "sourceKind": "book_text",
            "locatorRange": null,
            "text": "正文",
            "role": "system",
        });
        assert!(serde_json::from_value::<AgentContextSegment>(segment_json).is_err());

        let payload_json = serde_json::json!({
            "version": 1,
            "subject": {
                "workId": WorkId::new(),
                "editionId": null,
                "mediaItemId": null,
                "contentKind": "book",
            },
            "locator": null,
            "locatorConfidence": "unresolved",
            "boundaryMode": "user_provided",
            "contentRevision": "rev-1",
            "contentHash": null,
            "segments": [],
            "citations": [],
            "privacy": {
                "containsPersonalData": false,
                "containsCredentials": false,
                "containsAbsolutePaths": false,
                "containsFullWork": false,
                "redaction": "none",
            },
            "apiKey": "sk-whatever",
        });
        assert!(
            serde_json::from_value::<AgentContextPayload>(payload_json).is_err(),
            "载荷未知字段必须拒绝"
        );
    }

    #[test]
    fn context_hash_is_derived_from_the_canonical_payload() {
        let snapshot = snapshot();
        assert_eq!(snapshot.context_hash().len(), 64);
        assert!(is_canonical_digest(snapshot.context_hash()));
        assert_eq!(
            snapshot.context_hash(),
            canonical_digest(snapshot.canonical_json())
        );
        assert!(
            !snapshot.canonical_json().contains(snapshot.context_hash()),
            "context_hash 不得进入自身载荷"
        );
        assert!(!snapshot.canonical_json().contains("contextHash"));
        assert!(!snapshot.canonical_json().contains(' '));

        // 同 id、同载荷 → 同摘要（含 id 也不影响：id 不进载荷）。
        let again =
            AgentContextSnapshot::new(AgentContextSnapshotId::new(), snapshot.payload().clone())
                .unwrap();
        assert_eq!(again.context_hash(), snapshot.context_hash());
        assert_ne!(again.id(), snapshot.id());
    }

    #[test]
    fn canonical_payload_is_independent_of_key_order() {
        let snapshot = snapshot();
        let canonical = snapshot.canonical_json();

        // 同一语义载荷的不同书写顺序，重算后得到同一 canonical 字节串。
        let shuffled = concat!(
            r#"{"segments":[{"text":"第一章：风从北面来。","sourceKind":"book_text","#,
            r#""segmentId":"seg-1","locatorRange":null},{"text":"第二章：雨落在旧城。","#,
            r#""sourceKind":"book_text","segmentId":"seg-2","locatorRange":null}],"#,
            r#""privacy":{"redaction":"none","containsFullWork":false,"#,
            r#""containsAbsolutePaths":false,"containsCredentials":false,"#,
            r#""containsPersonalData":false},"contentRevision":"rev-0001","#,
            r#""boundaryMode":"strict_to_locator","locatorConfidence":"exact","#,
            r#""locator":{"kind":"article","data":{"text_anchor":null,"progression":0.25,"#,
            r#""block_id":"block-1"},"version":1},"contentHash":null,"citations":["#,
            r#"{"charEnd":3,"charStart":0,"segmentId":"seg-1"},"#,
            r#"{"segmentId":"seg-2","charStart":4,"charEnd":8}],"subject":{"contentKind":"book","#,
            r#""mediaItemId":null,"editionId":null,"workId":"#
        )
        .to_owned();
        let shuffled = format!(
            "{shuffled}\"{}\"}},\"version\":1}}",
            snapshot.subject().work_id
        );

        let parsed: AgentContextPayload = serde_json::from_str(&shuffled).unwrap();
        assert_eq!(parsed, *snapshot.payload());
        let recomputed = canonical_json_of(&parsed).unwrap();
        assert_eq!(recomputed, canonical);
        assert_eq!(canonical_digest(&recomputed), snapshot.context_hash());
    }

    #[test]
    fn restored_payload_roundtrips_and_rejects_tampering() {
        let snapshot = snapshot();
        let restored = AgentContextSnapshot::from_canonical_payload(
            snapshot.id(),
            snapshot.canonical_json(),
            snapshot.context_hash(),
        )
        .unwrap();
        assert_eq!(restored, snapshot);
        AgentContextSnapshot::verify_integrity(snapshot.canonical_json(), snapshot.context_hash())
            .unwrap();

        // 内容被改写（摘要不变）。
        let tampered_text = snapshot
            .canonical_json()
            .replace("第一章：风从北面来。", "第一章：风从南面来。");
        assert_ne!(tampered_text, snapshot.canonical_json());
        assert_eq!(
            AgentContextSnapshot::verify_integrity(&tampered_text, snapshot.context_hash())
                .unwrap_err()
                .code()
                .as_str(),
            "AGENT_CONTEXT_INTEGRITY_MISMATCH"
        );

        // 内容与摘要一起被改写：语义仍合法，但恢复路径要求"摘要自洽"；
        // 仅当攻击者重算摘要时才自洽——此时它是一条**合法的新事实**，
        // 因此恢复成功，但它是另一个 hash，不再是原来那条快照。
        let rehashed = canonical_digest(&tampered_text);
        let other =
            AgentContextSnapshot::from_canonical_payload(snapshot.id(), &tampered_text, &rehashed);
        assert!(other.is_ok());
        assert_ne!(other.unwrap().context_hash(), snapshot.context_hash());

        // 摘要形状非法（大写/长度差一位/非十六进制）。
        for bad in [
            "A".repeat(64),
            "g".repeat(64),
            "0".repeat(63),
            String::new(),
        ] {
            assert_eq!(
                AgentContextSnapshot::verify_integrity(snapshot.canonical_json(), &bad)
                    .unwrap_err()
                    .code()
                    .as_str(),
                "AGENT_CONTEXT_INTEGRITY_MISMATCH",
                "非法摘要形状必须按篡改处理"
            );
        }

        // 非 canonical 形式（多余空白）。
        let padded = format!(" {}", snapshot.canonical_json());
        assert_eq!(
            AgentContextSnapshot::verify_integrity(&padded, &canonical_digest(&padded))
                .unwrap_err()
                .code()
                .as_str(),
            "AGENT_CONTEXT_INTEGRITY_MISMATCH"
        );

        // 载荷版本不受支持。
        let mut payload = snapshot.payload().clone();
        payload.version = 2;
        assert_eq!(
            AgentContextSnapshot::new(AgentContextSnapshotId::new(), payload)
                .unwrap_err()
                .code()
                .as_str(),
            "AGENT_CONTEXT_UNSUPPORTED_VERSION"
        );
    }

    #[test]
    fn unresolved_locator_cannot_be_a_strict_boundary() {
        let make = |confidence, boundary| {
            AgentContextSnapshot::new(
                AgentContextSnapshotId::new(),
                AgentContextPayload::new(
                    subject(),
                    Some(article_locator(None)),
                    confidence,
                    boundary,
                    "rev-0001",
                ),
            )
        };

        assert_eq!(
            make(
                AgentLocatorConfidence::Unresolved,
                AgentBoundaryMode::StrictToLocator
            )
            .unwrap_err()
            .code()
            .as_str(),
            "AGENT_CONTEXT_INVALID_BOUNDARY"
        );
        assert_eq!(
            make(
                AgentLocatorConfidence::Unresolved,
                AgentBoundaryMode::CurrentRange
            )
            .unwrap_err()
            .code()
            .as_str(),
            "AGENT_CONTEXT_INVALID_BOUNDARY"
        );
        // 严格边界没有 locator：同样拒绝。
        assert_eq!(
            AgentContextSnapshot::new(
                AgentContextSnapshotId::new(),
                AgentContextPayload::new(
                    subject(),
                    None,
                    AgentLocatorConfidence::Exact,
                    AgentBoundaryMode::StrictToLocator,
                    "rev-0001",
                ),
            )
            .unwrap_err()
            .code()
            .as_str(),
            "AGENT_CONTEXT_INVALID_BOUNDARY"
        );

        // 用户显式范围允许没有可解析 locator。
        assert!(
            make(
                AgentLocatorConfidence::Unresolved,
                AgentBoundaryMode::UserProvided
            )
            .is_ok()
        );
        assert!(
            make(
                AgentLocatorConfidence::Range,
                AgentBoundaryMode::CurrentRange
            )
            .is_ok()
        );

        // 非法 locator 数值（progression 越界）在快照边界被拒绝，不 panic。
        assert_eq!(
            AgentContextSnapshot::new(
                AgentContextSnapshotId::new(),
                AgentContextPayload::new(
                    subject(),
                    Some(article_locator(Some(1.5))),
                    AgentLocatorConfidence::Exact,
                    AgentBoundaryMode::StrictToLocator,
                    "rev-0001",
                ),
            )
            .unwrap_err()
            .code()
            .as_str(),
            "AGENT_CONTEXT_INVALID_LOCATOR"
        );
    }

    #[test]
    fn citations_must_reference_existing_segments_and_be_unique() {
        let mut payload = context_payload();
        payload.citations = vec![AgentCitation::new(segment_id("seg-404"), 0, 1).unwrap()];
        assert_eq!(
            AgentContextSnapshot::new(AgentContextSnapshotId::new(), payload)
                .unwrap_err()
                .code()
                .as_str(),
            "AGENT_CONTEXT_UNKNOWN_SEGMENT"
        );

        // 重复引用同一区间。
        let mut payload = context_payload();
        let citation = AgentCitation::new(segment_id("seg-1"), 0, 3).unwrap();
        payload.citations = vec![citation.clone(), citation];
        assert_eq!(
            AgentContextSnapshot::new(AgentContextSnapshotId::new(), payload)
                .unwrap_err()
                .code()
                .as_str(),
            "AGENT_CONTEXT_DUPLICATE_CITATION"
        );

        // 区间越界 / 反向区间。
        let mut payload = context_payload();
        payload.citations = vec![AgentCitation::new(segment_id("seg-1"), 0, 999).unwrap()];
        assert_eq!(
            AgentContextSnapshot::new(AgentContextSnapshotId::new(), payload)
                .unwrap_err()
                .code()
                .as_str(),
            "AGENT_CONTEXT_INVALID_CITATION"
        );
        assert_eq!(
            AgentCitation::new(segment_id("seg-1"), 5, 5)
                .unwrap_err()
                .code()
                .as_str(),
            "AGENT_CONTEXT_INVALID_CITATION"
        );

        // 合法引用通过。
        AgentContextSnapshot::new(AgentContextSnapshotId::new(), payload_of_valid_citations())
            .unwrap();
    }

    fn payload_of_valid_citations() -> AgentContextPayload {
        let mut payload = context_payload();
        payload.citations = vec![AgentCitation::new(segment_id("seg-1"), 0, 3).unwrap()];
        payload
    }

    #[test]
    fn control_characters_counts_and_lengths_are_bounded() {
        let with_segment = |segment: AgentContextSegment| {
            let mut payload = context_payload();
            payload.citations = Vec::new();
            payload.segments = vec![segment];
            AgentContextSnapshot::new(AgentContextSnapshotId::new(), payload)
        };

        // 控制字符（换行/制表符除外）。
        for bad in ["正文\u{0}", "正文\u{7}", "正文\u{1b}[31m"] {
            assert_eq!(
                with_segment(segment("seg-1", bad))
                    .unwrap_err()
                    .code()
                    .as_str(),
                "AGENT_CONTEXT_INVALID_SEGMENT",
                "控制字符必须拒绝：{bad:?}"
            );
        }
        // 换行与制表符是正文的正常组成。
        assert!(with_segment(segment("seg-1", "第一段\n\t第二段")).is_ok());
        // 空文本。
        assert_eq!(
            with_segment(segment("seg-1", "   "))
                .unwrap_err()
                .code()
                .as_str(),
            "AGENT_CONTEXT_INVALID_SEGMENT"
        );

        // 单段文本超长。
        assert_eq!(
            with_segment(segment(
                "seg-1",
                &"字".repeat(AGENT_CONTEXT_MAX_SEGMENT_CHARS + 1)
            ))
            .unwrap_err()
            .code()
            .as_str(),
            "AGENT_CONTEXT_TOO_LARGE"
        );

        // segment 数量超限。
        let mut payload = context_payload();
        payload.citations = Vec::new();
        payload.segments = (0..=AGENT_CONTEXT_MAX_SEGMENTS)
            .map(|index| segment(&format!("seg-{index}"), "正文"))
            .collect();
        assert_eq!(
            AgentContextSnapshot::new(AgentContextSnapshotId::new(), payload)
                .unwrap_err()
                .code()
                .as_str(),
            "AGENT_CONTEXT_TOO_LARGE"
        );

        // 文本总量超限（每段都在单段上限内）。
        let mut payload = context_payload();
        payload.citations = Vec::new();
        payload.segments = (0..AGENT_CONTEXT_MAX_SEGMENTS)
            .map(|index| {
                segment(
                    &format!("seg-{index}"),
                    &"字".repeat(AGENT_CONTEXT_MAX_SEGMENT_CHARS),
                )
            })
            .collect();
        assert_eq!(
            AgentContextSnapshot::new(AgentContextSnapshotId::new(), payload)
                .unwrap_err()
                .code()
                .as_str(),
            "AGENT_CONTEXT_TOO_LARGE"
        );

        // citation 数量超限。
        let mut payload = context_payload();
        let mut citations = Vec::new();
        for index in 0..=AGENT_CONTEXT_MAX_CITATIONS {
            let start = index as u32 % 2;
            citations.push(AgentCitation::new(segment_id("seg-1"), start, start + 1).unwrap());
        }
        payload.citations = citations;
        assert_eq!(
            AgentContextSnapshot::new(AgentContextSnapshotId::new(), payload)
                .unwrap_err()
                .code()
                .as_str(),
            "AGENT_CONTEXT_TOO_LARGE"
        );

        // 重复 segment id 会让 citation 变得有歧义。
        let mut payload = context_payload();
        payload.citations = Vec::new();
        payload.segments = vec![segment("seg-1", "甲"), segment("seg-1", "乙")];
        assert_eq!(
            AgentContextSnapshot::new(AgentContextSnapshotId::new(), payload)
                .unwrap_err()
                .code()
                .as_str(),
            "AGENT_CONTEXT_INVALID_SEGMENT"
        );
    }

    #[test]
    fn segment_ids_and_content_revision_are_closed_tokens() {
        for bad in [
            "",
            " ",
            "seg 1",
            "seg/1",
            "seg\n1",
            "会话",
            &"x".repeat(AGENT_SEGMENT_ID_MAX_CHARS + 1),
        ] {
            assert!(
                AgentContextSegmentId::new(bad).is_err(),
                "segment id 必须拒绝：{bad:?}"
            );
        }
        assert!(AgentContextSegmentId::new("seg-1_2").is_ok());
        assert_eq!(segment_id("seg-1").to_string(), "seg-1");
        assert!(serde_json::from_str::<AgentContextSegmentId>("\"\"").is_err());
        assert!(serde_json::from_str::<AgentContextSegmentId>("\"seg 1\"").is_err());

        let with_revision = |revision: &str| {
            let mut payload = context_payload();
            payload.content_revision = revision.to_owned();
            AgentContextSnapshot::new(AgentContextSnapshotId::new(), payload)
        };
        for bad in [
            "",
            "D:/path",
            "rev 1",
            &"r".repeat(AGENT_CONTENT_REVISION_MAX_CHARS + 1),
        ] {
            assert_eq!(
                with_revision(bad).unwrap_err().code().as_str(),
                "AGENT_CONTEXT_INVALID",
                "content_revision 必须拒绝：{bad:?}"
            );
        }
        // content_hash 形状必须与提案摘要一致。
        let mut payload = context_payload();
        payload.content_hash = Some("ABC".into());
        assert_eq!(
            AgentContextSnapshot::new(AgentContextSnapshotId::new(), payload)
                .unwrap_err()
                .code()
                .as_str(),
            "AGENT_CONTEXT_INVALID"
        );
        let mut payload = context_payload();
        payload.content_hash = Some(canonical_digest("content"));
        assert!(AgentContextSnapshot::new(AgentContextSnapshotId::new(), payload).is_ok());
    }

    #[test]
    fn privacy_descriptor_refuses_secrets_paths_and_full_works() {
        let with_privacy = |privacy: AgentPrivacyDescriptor| {
            let mut payload = context_payload();
            payload.privacy = privacy;
            AgentContextSnapshot::new(AgentContextSnapshotId::new(), payload)
        };
        let defaults = AgentPrivacyDescriptor::default();
        assert!(with_privacy(defaults).is_ok());

        for (privacy, expected) in [
            (
                AgentPrivacyDescriptor {
                    contains_credentials: true,
                    ..defaults
                },
                "AGENT_CONTEXT_INVALID_PRIVACY",
            ),
            (
                AgentPrivacyDescriptor {
                    contains_absolute_paths: true,
                    ..defaults
                },
                "AGENT_CONTEXT_INVALID_PRIVACY",
            ),
            (
                AgentPrivacyDescriptor {
                    contains_full_work: true,
                    ..defaults
                },
                "AGENT_CONTEXT_INVALID_PRIVACY",
            ),
            (
                AgentPrivacyDescriptor {
                    contains_personal_data: true,
                    redaction: AgentRedactionMode::None,
                    ..defaults
                },
                "AGENT_CONTEXT_INVALID_PRIVACY",
            ),
        ] {
            assert_eq!(with_privacy(privacy).unwrap_err().code().as_str(), expected);
        }

        // 含个人数据但已做结构性脱敏：允许。
        assert!(
            with_privacy(AgentPrivacyDescriptor {
                contains_personal_data: true,
                redaction: AgentRedactionMode::Structural,
                ..defaults
            })
            .is_ok()
        );
    }

    #[test]
    fn subject_rejects_nil_uuids_at_every_boundary() {
        let nil = uuid::Uuid::nil();

        // 便捷构造返回 Self，但其 validate 必须拒绝 nil 身份。
        let nil_work = AgentSubject::work(WorkId::from_uuid(nil), AgentContentKind::Book);
        assert_eq!(
            nil_work.validate().unwrap_err().code().as_str(),
            "AGENT_CONTEXT_INVALID_SUBJECT"
        );
        assert_eq!(
            AgentSubject::new(WorkId::from_uuid(nil), None, None, AgentContentKind::Book)
                .unwrap_err()
                .code()
                .as_str(),
            "AGENT_CONTEXT_INVALID_SUBJECT"
        );

        assert_eq!(
            AgentSubject::edition(
                WorkId::new(),
                EditionId::from_uuid(nil),
                AgentContentKind::Book
            )
            .validate()
            .unwrap_err()
            .code()
            .as_str(),
            "AGENT_CONTEXT_INVALID_SUBJECT"
        );
        assert_eq!(
            AgentSubject::media_item(
                WorkId::new(),
                EditionId::new(),
                MediaItemId::from_uuid(nil),
                AgentContentKind::Book
            )
            .validate()
            .unwrap_err()
            .code()
            .as_str(),
            "AGENT_CONTEXT_INVALID_SUBJECT"
        );

        // 快照边界同样拒绝：便捷构造产出的非法 subject 无法流入推理。
        let with_subject = |subject: AgentSubject| {
            AgentContextSnapshot::new(
                AgentContextSnapshotId::new(),
                AgentContextPayload::new(
                    subject,
                    None,
                    AgentLocatorConfidence::Unresolved,
                    AgentBoundaryMode::UserProvided,
                    "rev-0001",
                ),
            )
        };
        for subject in [
            AgentSubject::work(WorkId::from_uuid(nil), AgentContentKind::Book),
            AgentSubject::edition(
                WorkId::new(),
                EditionId::from_uuid(nil),
                AgentContentKind::Book,
            ),
            AgentSubject::media_item(
                WorkId::new(),
                EditionId::new(),
                MediaItemId::from_uuid(nil),
                AgentContentKind::Book,
            ),
        ] {
            assert_eq!(
                with_subject(subject).unwrap_err().code().as_str(),
                "AGENT_CONTEXT_INVALID_SUBJECT"
            );
        }
    }

    #[test]
    fn snapshot_validate_recomputes_payload_and_hash() {
        let snapshot = snapshot();
        snapshot.validate().unwrap();

        // 内存中的 canonical 载荷被改写（摘要不变）：完整性失败。
        let mut tampered = snapshot.clone();
        tampered.canonical_json.push(' ');
        assert_eq!(
            tampered.validate().unwrap_err().code().as_str(),
            "AGENT_CONTEXT_INTEGRITY_MISMATCH"
        );

        // 摘要被改写：完整性失败。
        let mut rehashed = snapshot.clone();
        rehashed.context_hash = "0".repeat(64);
        assert_eq!(
            rehashed.validate().unwrap_err().code().as_str(),
            "AGENT_CONTEXT_INTEGRITY_MISMATCH"
        );

        // 从 canonical 载荷恢复出的快照走同一条校验逻辑。
        let restored = AgentContextSnapshot::from_canonical_payload(
            snapshot.id(),
            snapshot.canonical_json(),
            snapshot.context_hash(),
        )
        .unwrap();
        restored.validate().unwrap();
        assert_eq!(restored, snapshot);

        // nil id 沿用 INVALID（不是完整性错误），new 与 validate 一致。
        let nil_id = AgentContextSnapshot {
            id: AgentContextSnapshotId::from_uuid(uuid::Uuid::nil()),
            payload: snapshot.payload().clone(),
            canonical_json: snapshot.canonical_json().to_owned(),
            context_hash: snapshot.context_hash().to_owned(),
        };
        assert_eq!(
            nil_id.validate().unwrap_err().code().as_str(),
            "AGENT_CONTEXT_INVALID"
        );
    }

    #[test]
    fn segment_text_rejects_obvious_secrets_endpoints_and_paths() {
        let with_text = |text: &str| {
            let mut payload = context_payload();
            payload.citations = Vec::new();
            payload.segments = vec![segment("seg-1", text)];
            AgentContextSnapshot::new(AgentContextSnapshotId::new(), payload)
        };

        // 明显凭据赋值 / bearer 片段（不得把 API key 保留进快照）。
        for bad in [
            "api_key: sk-live-0123456789abcdef",
            "API_KEY = 42c0ffee1234",
            "Authorization: Bearer eyJhbGciOiJIUzI1NiIsInR5cCI6IkpXVCJ9",
            "access_key=local-dev-placeholder-0001",
            "password: hunter2secret",
            "client_secret: 0123456789abcdef",
            "bearer\tdeadbeefcafebabe",
        ] {
            assert_eq!(
                with_text(bad).unwrap_err().code().as_str(),
                "AGENT_CONTEXT_SENSITIVE_TEXT",
                "敏感凭据必须拒绝：{bad:?}"
            );
        }

        // endpoint 形式（不得保留完整 endpoint）。
        for bad in [
            "见 https://api.example.com/v1/models",
            "http://localhost:8080/health",
            "file:///C:/Users/me/secret.txt",
            "ftp://files.example.com/dump",
        ] {
            assert_eq!(
                with_text(bad).unwrap_err().code().as_str(),
                "AGENT_CONTEXT_SENSITIVE_TEXT",
                "endpoint 必须拒绝：{bad:?}"
            );
        }

        // Windows / Unix / UNC 绝对路径。
        for bad in [
            r"导出到 D:\Media\book.epub",
            "打开 C:/Users/me/Documents",
            "位于 /home/reader/library",
            r"挂载在 \\server\share\books",
            "~/Documents/novel.epub",
        ] {
            assert_eq!(
                with_text(bad).unwrap_err().code().as_str(),
                "AGENT_CONTEXT_SENSITIVE_TEXT",
                "绝对路径必须拒绝：{bad:?}"
            );
        }
    }

    #[test]
    fn segment_text_keeps_normal_prose_with_slashes_and_newlines() {
        let with_text = |text: &str| {
            let mut payload = context_payload();
            payload.citations = Vec::new();
            payload.segments = vec![segment("seg-1", text)];
            AgentContextSnapshot::new(AgentContextSnapshotId::new(), payload)
        };

        for good in [
            "第一段\n\t第二段：风从北面来。",
            "选择是/否都可以，TCP/IP 协议没问题。",
            "进度 2024/09/17 已完成 3/5。",
            "他说：“并列/递进都可以。”",
        ] {
            assert!(with_text(good).is_ok(), "正常正文必须通过：{good:?}");
        }
    }

    #[test]
    fn manifest_defaults_grant_only_the_implemented_slice() {
        let manifest = AgentCapabilityManifest::default();
        manifest.validate().unwrap();
        assert_eq!(manifest.agent_api_version, AGENT_API_VERSION);
        for capability in [
            AgentCapability::SettingsRead,
            AgentCapability::SettingsProposal,
            AgentCapability::LibrarySummaryRead,
            AgentCapability::SettingSourcesRead,
            AgentCapability::ResourcePreferenceRead,
            AgentCapability::ResourcePreferenceProposal,
            AgentCapability::MediaCapabilitiesRead,
            AgentCapability::OnboardingRead,
        ] {
            assert!(manifest.grants(capability), "{capability:?} 必须已实现");
            manifest.require(capability).unwrap();
        }
        for capability in [
            AgentCapability::MetadataProposal,
            AgentCapability::RenameProposal,
            AgentCapability::SecretRead,
            AgentCapability::FilesystemWrite,
        ] {
            assert!(!manifest.grants(capability), "{capability:?} 必须默认关闭");
            assert_eq!(
                manifest.require(capability).unwrap_err().code().as_str(),
                "AGENT_CAPABILITY_NOT_GRANTED"
            );
        }
        manifest.require(AgentCapability::SettingsProposal).unwrap();

        // manifest 的默认值就是本切片清单本身：不存在"默认多开一项"的宽松默认。
        assert_eq!(
            AgentCapabilityManifest::default().capabilities,
            AgentCapabilityManifest::for_current_slice().capabilities
        );

        // 值对象自身的默认值才是全关：不存在"默认放行"的中间态。
        for capability in [
            AgentCapability::MetadataProposal,
            AgentCapability::RenameProposal,
            AgentCapability::SecretRead,
            AgentCapability::FilesystemWrite,
        ] {
            assert!(!AgentCapabilitySet::default().grants(capability));
        }
        assert!(!AgentCapabilitySet::default().grants(AgentCapability::SettingsRead));
        assert!(!AgentCapabilitySet::default().grants(AgentCapability::SettingsProposal));
    }

    #[test]
    fn manifest_json_is_the_nested_capability_contract() {
        // 序列化形状必须逐字段等于批准的嵌套契约（不是扁平字段）。
        assert_eq!(
            serde_json::to_value(AgentCapabilityManifest::for_current_slice()).unwrap(),
            serde_json::json!({
                "agentApiVersion": 1,
                "capabilities": {
                    "settingsRead": true,
                    "settingsProposal": true,
                    "librarySummaryRead": true,
                    "settingSourcesRead": true,
                    "resourcePreferenceRead": true,
                    "resourcePreferenceProposal": true,
                    "mediaCapabilitiesRead": true,
                    "onboardingRead": true,
                    "metadataProposal": false,
                    "renameProposal": false,
                    "secretRead": false,
                    "filesystemWrite": false,
                },
            })
        );

        // 反序列化同一份 JSON 必须回到同一个值。
        let approved = r#"{"agentApiVersion":1,"capabilities":{
            "settingsRead":true,"settingsProposal":true,"librarySummaryRead":true,
            "settingSourcesRead":true,"resourcePreferenceRead":true,
            "resourcePreferenceProposal":true,"mediaCapabilitiesRead":true,"onboardingRead":true,
            "metadataProposal":false,"renameProposal":false,"secretRead":false,
            "filesystemWrite":false}}"#;
        let parsed: AgentCapabilityManifest = serde_json::from_str(approved).unwrap();
        assert_eq!(parsed, AgentCapabilityManifest::for_current_slice());
        parsed.validate().unwrap();
        assert_eq!(parsed, AgentCapabilityManifest::default());

        // 值对象自身也能独立往返。
        let set: AgentCapabilitySet =
            serde_json::from_str(r#"{"secretRead":false,"settingsRead":true}"#).unwrap();
        assert!(set.settings_read);
        assert!(!set.secret_read);
        assert!(!set.settings_proposal);
        assert_eq!(
            serde_json::to_value(set).unwrap(),
            serde_json::json!({
                "settingsRead": true,
                "settingsProposal": false,
                "librarySummaryRead": false,
                "settingSourcesRead": false,
                "resourcePreferenceRead": false,
                "resourcePreferenceProposal": false,
                "mediaCapabilitiesRead": false,
                "onboardingRead": false,
                "metadataProposal": false,
                "renameProposal": false,
                "secretRead": false,
                "filesystemWrite": false,
            })
        );
    }

    #[test]
    fn manifest_capability_fields_default_to_closed() {
        // 缺省的能力字段一律 false（fail-closed），不会"缺省即放行"。
        let partial: AgentCapabilityManifest =
            serde_json::from_str(r#"{"agentApiVersion":1,"capabilities":{"settingsRead":true}}"#)
                .unwrap();
        assert!(partial.grants(AgentCapability::SettingsRead));
        assert!(!partial.grants(AgentCapability::SettingsProposal));
        assert_eq!(
            partial,
            AgentCapabilityManifest {
                capabilities: AgentCapabilitySet {
                    settings_read: true,
                    ..AgentCapabilitySet::default()
                },
                ..AgentCapabilityManifest::for_current_slice()
            }
        );
        partial.validate().unwrap();
        assert_eq!(
            partial
                .require(AgentCapability::SettingsProposal)
                .unwrap_err()
                .code()
                .as_str(),
            "AGENT_CAPABILITY_NOT_GRANTED"
        );

        // 空 capabilities 对象与整体缺省都退化为全关，且版本合法时仍可校验通过。
        let empty_object: AgentCapabilityManifest =
            serde_json::from_str(r#"{"agentApiVersion":1,"capabilities":{}}"#).unwrap();
        assert_eq!(empty_object.capabilities, AgentCapabilitySet::default());
        empty_object.validate().unwrap();
        assert_eq!(
            empty_object
                .require(AgentCapability::SettingsRead)
                .unwrap_err()
                .code()
                .as_str(),
            "AGENT_CAPABILITY_NOT_GRANTED"
        );

        let no_capabilities: AgentCapabilityManifest =
            serde_json::from_str(r#"{"agentApiVersion":1}"#).unwrap();
        assert_eq!(no_capabilities.capabilities, AgentCapabilitySet::default());
        no_capabilities.validate().unwrap();
        for capability in [
            AgentCapability::SettingsRead,
            AgentCapability::SettingsProposal,
            AgentCapability::LibrarySummaryRead,
            AgentCapability::MetadataProposal,
            AgentCapability::RenameProposal,
            AgentCapability::SecretRead,
            AgentCapability::FilesystemWrite,
        ] {
            assert!(
                !no_capabilities.grants(capability),
                "{capability:?} 缺省即关闭"
            );
        }
    }

    #[test]
    fn manifest_rejects_unknown_fields_at_both_levels() {
        // 嵌套对象里的未知能力名：解析即失败，不会被静默忽略成"没声明"。
        for bad in [
            r#"{"agentApiVersion":1,"capabilities":{"shellExec":true}}"#,
            r#"{"agentApiVersion":1,"capabilities":{"settingsRead":true,"shellExec":true}}"#,
            r#"{"agentApiVersion":1,"capabilities":{"filesystem_write":true}}"#,
        ] {
            assert!(
                serde_json::from_str::<AgentCapabilityManifest>(bad).is_err(),
                "capabilities 未知字段必须拒绝：{bad}"
            );
        }

        // 顶层未知字段：包括旧扁平写法——字段搬到 capabilities 里之后，
        // 扁平写法必须整体失效，而不是被当作未知字段静默丢弃。
        for bad in [
            r#"{"agentApiVersion":1,"settingsRead":true}"#,
            r#"{"agentApiVersion":1,"capabilities":{"settingsRead":true},"shellExec":true}"#,
            r#"{"agentApiVersion":1,"capabilities":{"settingsRead":true},"grants":{}}"#,
        ] {
            assert!(
                serde_json::from_str::<AgentCapabilityManifest>(bad).is_err(),
                "manifest 未知顶层字段必须拒绝：{bad}"
            );
        }

        // 值对象本身同样拒绝未知字段与非布尔值。
        for bad in [
            r#"{"shellExec":true}"#,
            r#"{"settingsRead":"true"}"#,
            r#"{"settingsRead":1}"#,
        ] {
            assert!(
                serde_json::from_str::<AgentCapabilitySet>(bad).is_err(),
                "能力开关必须是布尔字段：{bad}"
            );
        }
    }

    #[test]
    fn manifest_rejects_unimplemented_capabilities_and_unknown_versions() {
        // 合法可解析，但声明了本切片未实现的能力：解析通过、校验拒绝。
        let declared: AgentCapabilityManifest = serde_json::from_str(
            r#"{"agentApiVersion":1,"capabilities":{
                "settingsRead":true,"settingsProposal":true,"filesystemWrite":true}}"#,
        )
        .unwrap();
        assert!(declared.grants(AgentCapability::FilesystemWrite));
        assert_eq!(
            declared.validate().unwrap_err().code().as_str(),
            "AGENT_CAPABILITY_NOT_IMPLEMENTED"
        );
        assert_eq!(
            declared
                .require(AgentCapability::SettingsRead)
                .unwrap_err()
                .code()
                .as_str(),
            "AGENT_CAPABILITY_NOT_IMPLEMENTED",
            "清单自身非法时，任何 require 都必须先失败"
        );

        // 每一项高风险未实现能力单独声明都会被拒绝。
        for (capability, json) in [
            (
                AgentCapability::MetadataProposal,
                r#"{"agentApiVersion":1,"capabilities":{"metadataProposal":true}}"#,
            ),
            (
                AgentCapability::RenameProposal,
                r#"{"agentApiVersion":1,"capabilities":{"renameProposal":true}}"#,
            ),
            (
                AgentCapability::SecretRead,
                r#"{"agentApiVersion":1,"capabilities":{"secretRead":true}}"#,
            ),
            (
                AgentCapability::FilesystemWrite,
                r#"{"agentApiVersion":1,"capabilities":{"filesystemWrite":true}}"#,
            ),
        ] {
            let manifest: AgentCapabilityManifest = serde_json::from_str(json).unwrap();
            assert!(manifest.grants(capability));
            assert_eq!(
                manifest.validate().unwrap_err().code().as_str(),
                "AGENT_CAPABILITY_NOT_IMPLEMENTED",
                "{capability:?} 未实现，必须拒绝"
            );
        }

        // 程序化构造同样被拒绝（不能绕过 JSON 校验）。
        for capability in [
            AgentCapability::MetadataProposal,
            AgentCapability::RenameProposal,
            AgentCapability::SecretRead,
            AgentCapability::FilesystemWrite,
        ] {
            let mut manifest = AgentCapabilityManifest::for_current_slice();
            match capability {
                AgentCapability::MetadataProposal => manifest.capabilities.metadata_proposal = true,
                AgentCapability::RenameProposal => manifest.capabilities.rename_proposal = true,
                AgentCapability::SecretRead => manifest.capabilities.secret_read = true,
                AgentCapability::FilesystemWrite => manifest.capabilities.filesystem_write = true,
                _ => unreachable!(),
            }
            assert_eq!(
                manifest.validate().unwrap_err().code().as_str(),
                "AGENT_CAPABILITY_NOT_IMPLEMENTED"
            );
        }

        // 版本不匹配：未知版本（更高或更低）一律拒绝。
        for version in [0, 2, u32::MAX] {
            let manifest = AgentCapabilityManifest {
                agent_api_version: version,
                ..AgentCapabilityManifest::for_current_slice()
            };
            assert_eq!(
                manifest.validate().unwrap_err().code().as_str(),
                "AGENT_API_VERSION_UNSUPPORTED",
                "版本 {version} 必须拒绝"
            );
        }
        assert_eq!(
            serde_json::from_str::<AgentCapabilityManifest>(
                r#"{"agentApiVersion":2,"capabilities":{"settingsRead":true}}"#
            )
            .unwrap()
            .validate()
            .unwrap_err()
            .code()
            .as_str(),
            "AGENT_API_VERSION_UNSUPPORTED"
        );
        // 版本不受支持时不吞掉错误：require 同样失败。
        assert_eq!(
            AgentCapabilityManifest {
                agent_api_version: 2,
                ..AgentCapabilityManifest::for_current_slice()
            }
            .require(AgentCapability::SettingsRead)
            .unwrap_err()
            .code()
            .as_str(),
            "AGENT_API_VERSION_UNSUPPORTED"
        );
    }

    #[test]
    fn tasks_declare_whether_they_need_a_locator() {
        assert!(AgentTask::Recap.requires_locator());
        assert!(AgentTask::ExplainSelection.requires_locator());
        assert!(AgentTask::TranslateSelection.requires_locator());
        assert!(!AgentTask::QuestionAnswer.requires_locator());
        assert!(!AgentTask::PeriodicalSummary.requires_locator());
        assert!(!AgentTask::CharacterRelations.requires_locator());
        assert_eq!(AgentTask::QuestionAnswer.as_str(), "question_answer");
        assert_eq!(
            serde_json::to_string(&AgentTask::ExplainSelection).unwrap(),
            "\"explain_selection\""
        );
    }

    #[test]
    fn payload_and_segment_locators_reject_endpoints_and_absolute_paths() {
        let with_payload_locator = |locator: Locator| {
            AgentContextSnapshot::new(
                AgentContextSnapshotId::new(),
                AgentContextPayload::new(
                    subject(),
                    Some(locator),
                    AgentLocatorConfidence::Exact,
                    AgentBoundaryMode::StrictToLocator,
                    "rev-0001",
                ),
            )
        };
        let with_segment_locator = |locator: Locator| {
            let mut payload = context_payload();
            payload.citations = Vec::new();
            let mut only = segment("seg-1", "第一章：风从北面来。");
            only.locator_range = Some(locator);
            payload.segments = vec![only];
            AgentContextSnapshot::new(AgentContextSnapshotId::new(), payload)
        };

        // locator 里的 endpoint/绝对路径必须与 segment 正文走同一条 fail-closed 扫描。
        let rejected = [
            // Book.publication_resource：endpoint 与 Windows 绝对路径。
            book_locator("https://cdn.example.com/book/chapter-01.xhtml", None),
            book_locator(r"D:\Library\book\chapter-01.xhtml", None),
            // Book.format_locator：endpoint 与 Unix 绝对路径。
            book_locator("chapter-01.xhtml", Some("https://cdn.example.com/book.cfi")),
            book_locator(
                "chapter-01.xhtml",
                Some("/home/reader/library/chapter-01.xhtml"),
            ),
            // Article.text_anchor：endpoint 与绝对路径。
            article_locator_with_anchor(TextAnchor {
                exact: Some("见 http://localhost:8080/health".into()),
                prefix: None,
                suffix: None,
            }),
            article_locator_with_anchor(TextAnchor {
                exact: None,
                prefix: Some("取自 file:///C:/Users/me/secret.txt".into()),
                suffix: None,
            }),
            // GenericLocator：endpoint 与绝对路径。
            generic_locator("mirror", "https://files.example.com/dump"),
            generic_locator("root", "/srv/library/dump"),
        ];
        for locator in rejected {
            assert_eq!(
                with_payload_locator(locator.clone())
                    .unwrap_err()
                    .code()
                    .as_str(),
                "AGENT_CONTEXT_SENSITIVE_TEXT",
                "payload.locator 必须拒绝：{locator:?}"
            );
            assert_eq!(
                with_segment_locator(locator.clone())
                    .unwrap_err()
                    .code()
                    .as_str(),
                "AGENT_CONTEXT_SENSITIVE_TEXT",
                "segment.locator_range 必须拒绝：{locator:?}"
            );
        }

        // 正常 resource token（含多级相对路径）不得误伤。
        let normal = book_locator("OEBPS/text/chapter01.xhtml", Some("chapter-03.xhtml"));
        with_payload_locator(normal.clone()).unwrap();
        with_segment_locator(normal).unwrap();
    }

    #[test]
    fn approval_tokens_are_opaque_redacted_and_shape_closed() {
        let token = AgentApprovalToken::issue();
        let raw = token.as_str().to_owned();

        // 形状封闭：恰好 64 位小写十六进制，无前缀、无分隔符。
        assert_eq!(AGENT_APPROVAL_TOKEN_LEN, 64);
        assert_eq!(raw.len(), 64);
        assert!(
            raw.bytes()
                .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
        );
        assert_eq!(raw.as_str(), token.expose());
        // 两枚令牌不可预测地不同（v7 随机位）。
        assert_ne!(raw, AgentApprovalToken::issue().as_str());

        // 明文只能显式取出，Debug/Display 一律脱敏。
        assert_eq!(format!("{token}"), "[REDACTED]");
        assert_eq!(format!("{token:?}"), "[REDACTED]");
        assert!(!format!("{token:?}").contains(&raw));

        // from_text 接受签发出的形状，并按同一个闭合形状校验。
        assert_eq!(
            AgentApprovalToken::from_text(raw.clone()).unwrap().as_str(),
            raw.as_str()
        );
        let sample = hex64();
        for bad in [
            sample.to_ascii_uppercase(),
            format!("{sample}0"),
            sample[..63].to_owned(),
            format!("-{sample}"),
            format!("0x{}", &sample[..62]),
            "g".repeat(AGENT_APPROVAL_TOKEN_LEN),
            String::new(),
        ] {
            assert_eq!(
                AgentApprovalToken::from_text(bad.clone())
                    .unwrap_err()
                    .code()
                    .as_str(),
                "AGENT_APPROVAL_TOKEN_INVALID",
                "形状之外的写法必须拒绝：{bad:?}"
            );
        }
    }

    #[test]
    fn approval_token_hash_is_a_strict_sha256_digest() {
        let digest = canonical_digest("token-material");
        let hash = AgentApprovalTokenHash::parse(digest.clone()).unwrap();
        assert_eq!(hash.as_str(), digest.as_str());
        assert!(is_canonical_digest(hash.as_str()));
        assert_eq!(
            serde_json::to_string(&hash).unwrap(),
            format!("\"{digest}\"")
        );
        assert_eq!(
            serde_json::from_str::<AgentApprovalTokenHash>(&format!("\"{digest}\"")).unwrap(),
            hash
        );

        let sample = hex64();
        for bad in [
            sample.to_ascii_uppercase(),
            sample[..63].to_owned(),
            format!("{sample}0"),
            "g".repeat(64),
            String::new(),
        ] {
            assert_eq!(
                AgentApprovalTokenHash::parse(bad.clone())
                    .unwrap_err()
                    .code()
                    .as_str(),
                "AGENT_APPROVAL_TOKEN_INVALID",
                "非闭合摘要必须拒绝：{bad:?}"
            );
            // 反序列化入口与构造入口共用同一形状校验。
            assert!(
                serde_json::from_str::<AgentApprovalTokenHash>(
                    &serde_json::to_string(&bad).unwrap()
                )
                .is_err()
            );
        }
    }

    #[test]
    fn pending_binding_issues_and_reissues_only_inside_its_window() {
        let binding = action_binding();
        let digest = canonical_digest("proposal-payload");

        // 新建 pending 绑定默认不带令牌。
        assert_eq!(binding.approval_state(), AgentApprovalState::Pending);
        assert!(!binding.has_approval_token());
        assert!(binding.approval_token_hash().is_none());
        assert!(binding.issued_at().is_none());

        let (token, hash) = bound_token(&binding, &digest);
        // 签发时刻必须落在 [created_at, expires_at) 内。
        for bad_at in [
            UtcMillis::from_millis(0),
            UtcMillis::from_millis(999),
            UtcMillis::from_millis(5_000),
            UtcMillis::from_millis(9_999),
        ] {
            assert_eq!(
                binding
                    .issue_approval_token(hash.clone(), bad_at)
                    .unwrap_err()
                    .code()
                    .as_str(),
                "AGENT_APPROVAL_TOKEN_INVALID",
                "签发时刻 {bad_at:?} 必须拒绝"
            );
        }

        let issued = binding
            .issue_approval_token(hash.clone(), UtcMillis::from_millis(2_000))
            .unwrap();
        assert!(issued.has_approval_token());
        assert_eq!(issued.approval_token_hash(), Some(&hash));
        assert_eq!(issued.issued_at(), Some(UtcMillis::from_millis(2_000)));
        assert_eq!(issued.approval_state(), AgentApprovalState::Pending);
        // 签发不就地改写原绑定。
        assert!(!binding.has_approval_token());

        // 重新签发是覆盖语义：旧摘要被替换，旧令牌立即失效。
        let (other_token, other_hash) = bound_token(&binding, &digest);
        assert_ne!(other_hash, hash);
        let reissued = issued
            .issue_approval_token(other_hash.clone(), UtcMillis::from_millis(3_000))
            .unwrap();
        assert_eq!(reissued.approval_token_hash(), Some(&other_hash));
        assert_eq!(reissued.issued_at(), Some(UtcMillis::from_millis(3_000)));
        reissued
            .verify_approval_token(binding.proposal_id(), &digest, &other_token)
            .unwrap();
        assert_eq!(
            reissued
                .verify_approval_token(binding.proposal_id(), &digest, &token)
                .unwrap_err()
                .code()
                .as_str(),
            "AGENT_APPROVAL_TOKEN_INVALID"
        );
    }

    #[test]
    fn terminal_states_clear_the_token_and_refuse_issuing() {
        let binding = action_binding();
        let digest = canonical_digest("proposal-payload");
        let (token, hash) = bound_token(&binding, &digest);
        let issued = binding
            .issue_approval_token(hash.clone(), UtcMillis::from_millis(2_000))
            .unwrap();

        for terminal in [
            AgentApprovalState::Applied,
            AgentApprovalState::Rejected,
            AgentApprovalState::Expired,
        ] {
            let next = issued.with_approval_state(terminal).unwrap();
            assert_eq!(next.approval_state(), terminal);
            assert!(next.approval_token_hash().is_none());
            assert!(next.issued_at().is_none());
            assert!(!next.has_approval_token());
            assert_eq!(
                next.issue_approval_token(hash.clone(), UtcMillis::from_millis(3_000))
                    .unwrap_err()
                    .code()
                    .as_str(),
                "AGENT_APPROVAL_TOKEN_INVALID",
                "终态不得再签发"
            );
            assert_eq!(
                next.verify_approval_token(binding.proposal_id(), &digest, &token)
                    .unwrap_err()
                    .code()
                    .as_str(),
                "AGENT_APPROVAL_TOKEN_INVALID",
                "终态手上的令牌失效"
            );
        }
    }

    #[test]
    fn verify_approval_token_binds_token_to_one_proposal() {
        let binding = action_binding();
        let digest = canonical_digest("proposal-payload");
        let (token, hash) = bound_token(&binding, &digest);

        // 尚未签发：手上的令牌无效（不存在"默认有效"的令牌）。
        assert_eq!(
            binding
                .verify_approval_token(binding.proposal_id(), &digest, &token)
                .unwrap_err()
                .code()
                .as_str(),
            "AGENT_APPROVAL_TOKEN_INVALID"
        );

        let issued = binding
            .issue_approval_token(hash, UtcMillis::from_millis(2_000))
            .unwrap();
        issued
            .verify_approval_token(binding.proposal_id(), &digest, &token)
            .unwrap();

        // 换提案 / 换载荷 / 换令牌 / 摘要形状非法 全部 fail-closed。
        let (stranger, _) = bound_token(&binding, &digest);
        let other_digest = canonical_digest("tampered-payload");
        assert_ne!(other_digest, digest);
        let rejected = [
            (SettingProposalId::new(), digest.clone(), &token),
            (binding.proposal_id(), other_digest, &token),
            (binding.proposal_id(), digest.clone(), &stranger),
        ];
        for (proposal_id, presented_digest, presented_token) in rejected {
            assert_eq!(
                issued
                    .verify_approval_token(proposal_id, &presented_digest, presented_token)
                    .unwrap_err()
                    .code()
                    .as_str(),
                "AGENT_APPROVAL_TOKEN_INVALID"
            );
        }
        assert_eq!(
            issued
                .verify_approval_token(binding.proposal_id(), "not-a-digest", &token)
                .unwrap_err()
                .code()
                .as_str(),
            "AGENT_APPROVAL_TOKEN_INVALID"
        );
    }

    #[test]
    fn stored_token_fields_must_be_present_together_and_absent_in_terminal_states() {
        let binding = action_binding();
        let digest = canonical_digest("proposal-payload");
        let (token, hash) = bound_token(&binding, &digest);
        let issued = binding
            .issue_approval_token(hash.clone(), UtcMillis::from_millis(2_000))
            .unwrap();

        // 旧的 from_stored 路径继续可用：恢复出的是一条"无令牌"绑定。
        let restored = AgentActionBinding::from_stored(
            binding.proposal_id(),
            binding.session_id(),
            binding.request_id(),
            binding.context_snapshot_id(),
            binding.context_hash().to_owned(),
            binding.subject(),
            binding.action_kind(),
            binding.created_at(),
            binding.expires_at(),
            AgentApprovalState::Pending,
        )
        .unwrap();
        assert!(!restored.has_approval_token());
        assert_eq!(restored, binding);

        // 带令牌的持久化路径：摘要 + 签发时刻同时存在才可恢复。
        let with_token = stored_with_token(
            &binding,
            AgentApprovalState::Pending,
            Some(hash.as_str().to_owned()),
            Some(UtcMillis::from_millis(2_000)),
        )
        .unwrap();
        assert_eq!(with_token.approval_token_hash(), Some(&hash));
        assert_eq!(with_token.issued_at(), Some(UtcMillis::from_millis(2_000)));
        with_token
            .verify_approval_token(binding.proposal_id(), &digest, &token)
            .unwrap();
        assert_eq!(with_token, issued);

        // 序列化形状保留两字段，且能精确往返。
        let json = serde_json::to_string(&with_token).unwrap();
        assert!(json.contains("approvalTokenHash"));
        assert!(json.contains("issuedAt"));
        assert_eq!(
            serde_json::from_str::<AgentActionBinding>(&json).unwrap(),
            with_token
        );

        // 只有一半、越过有效期、终态携带令牌：都是损坏的事实。
        for (approval_state, token_hash, issued_at) in [
            (
                AgentApprovalState::Pending,
                Some(hash.as_str().to_owned()),
                None,
            ),
            (
                AgentApprovalState::Pending,
                None,
                Some(UtcMillis::from_millis(2_000)),
            ),
            (
                AgentApprovalState::Pending,
                Some(hash.as_str().to_owned()),
                Some(UtcMillis::from_millis(5_000)),
            ),
            (
                AgentApprovalState::Applied,
                Some(hash.as_str().to_owned()),
                Some(UtcMillis::from_millis(2_000)),
            ),
            (
                AgentApprovalState::Rejected,
                Some(hash.as_str().to_owned()),
                Some(UtcMillis::from_millis(2_000)),
            ),
            (
                AgentApprovalState::Expired,
                Some(hash.as_str().to_owned()),
                Some(UtcMillis::from_millis(2_000)),
            ),
        ] {
            assert_eq!(
                stored_with_token(&binding, approval_state, token_hash, issued_at)
                    .unwrap_err()
                    .code()
                    .as_str(),
                "AGENT_ACTION_BINDING_INVALID",
                "半截/终态令牌事实必须拒绝"
            );
        }

        // 摘要形状非法在恢复入口就被拒绝（不是"另一种写法"）。
        assert_eq!(
            stored_with_token(
                &binding,
                AgentApprovalState::Pending,
                Some(hex64().to_ascii_uppercase()),
                Some(UtcMillis::from_millis(2_000)),
            )
            .unwrap_err()
            .code()
            .as_str(),
            "AGENT_APPROVAL_TOKEN_INVALID"
        );
    }

    #[test]
    fn approval_token_errors_never_carry_the_token_text() {
        let binding = action_binding();
        let digest = canonical_digest("proposal-payload");
        let (token, hash) = bound_token(&binding, &digest);
        let raw = token.as_str().to_owned();
        let issued = binding
            .issue_approval_token(hash, UtcMillis::from_millis(2_000))
            .unwrap();

        // 校验失败：错误消息与 Debug 都不得回显被提交的令牌原文。
        let mismatch = issued
            .verify_approval_token(binding.proposal_id(), &digest, &AgentApprovalToken::issue())
            .unwrap_err();
        assert_eq!(mismatch.code().as_str(), "AGENT_APPROVAL_TOKEN_INVALID");
        assert!(!mismatch.user_message().contains(&raw));
        assert!(!format!("{mismatch:?}").contains(&raw));

        // 被拒绝的"接近正确"输入同样不得随错误冒泡。
        let near_miss = format!("0{raw}");
        let rejected = AgentApprovalToken::from_text(near_miss.clone()).unwrap_err();
        assert_eq!(rejected.code().as_str(), "AGENT_APPROVAL_TOKEN_INVALID");
        assert!(!rejected.user_message().contains(&near_miss));
        assert!(!format!("{rejected:?}").contains(&near_miss));
    }
    // ---------- 全局设置 Subject / 设置上下文 ----------

    /// 旧版（内容 Subject）的**已存 canonical JSON**：恢复后 canonical 形状必须逐字节不变。
    ///
    /// 这段字面量是存储里真实存在的形状（扁平四字段），不是"现在的写法重排一遍"，
    /// 因此它能挡住"给 AgentSubject 套一层 tag/flatten 包装"这类静默漂移。
    const LEGACY_SUBJECT_JSON: &str = concat!(
        r#"{"contentKind":"book","editionId":null,"mediaItemId":null,"#,
        r#""workId":"0196f0d2-0000-7000-8000-00000000c100"}"#
    );

    #[test]
    fn legacy_stored_content_subject_keeps_its_canonical_shape() {
        // 旧绑定读出来的就是这条 subject_json。
        let legacy: AgentScopeSubject = serde_json::from_str(LEGACY_SUBJECT_JSON).unwrap();
        let content = legacy.as_content().expect("旧载荷必须仍是内容范围");
        assert_eq!(content.content_kind, AgentContentKind::Book);
        assert_eq!(content.edition_id, None);
        assert_eq!(content.media_item_id, None);
        assert_eq!(
            content.work_id.to_string(),
            "0196f0d2-0000-7000-8000-00000000c100"
        );

        // 恢复路径要求"载荷 == 它的 canonical 重写"：任何形状漂移都会让旧绑定变成完整性错误。
        assert_eq!(
            canonical_json_of(&legacy).unwrap(),
            LEGACY_SUBJECT_JSON,
            "内容 Subject 的 canonical 形状必须零漂移"
        );
        // 恢复路径只要求 canonical 形状零漂移；裸 serde 输出的键序由结构体字段序决定，
        // 存储从不依赖它（canonical 会把键按字节序排好）。
        assert_eq!(
            serde_json::from_str::<AgentScopeSubject>(&serde_json::to_string(&legacy).unwrap())
                .unwrap(),
            legacy
        );
    }

    /// 两个变体之间不能有歧义：内容 JSON 只能解析成内容，设置 JSON 只能解析成设置。
    #[test]
    fn scope_variants_do_not_overlap() {
        let settings = AgentScopeSubject::Settings(AgentSettingsSubject::reading());
        let content =
            AgentScopeSubject::Content(AgentSubject::work(WorkId::new(), AgentContentKind::Comic));
        let settings_json = canonical_json_of(&settings).unwrap();
        let content_json = canonical_json_of(&content).unwrap();
        assert_ne!(settings_json, content_json);

        assert!(
            serde_json::from_str::<AgentScopeSubject>(&settings_json)
                .unwrap()
                .as_settings()
                .is_some()
        );
        assert!(
            serde_json::from_str::<AgentScopeSubject>(&content_json)
                .unwrap()
                .as_content()
                .is_some()
        );
        // 空对象与半成品都不属于任何一个变体。
        assert!(serde_json::from_str::<AgentScopeSubject>("{}").is_err());
        assert!(
            serde_json::from_str::<AgentScopeSubject>(
                r#"{"section":"reading","workId":"0196f0d2-0000-7000-8000-00000000c100"}"#
            )
            .is_err()
        );
        assert!(serde_json::from_str::<AgentScopeSubject>(r#"{"section":"comic"}"#).is_err());
    }

    #[test]
    fn settings_subject_is_a_global_scope_without_fake_work_id() {
        let subject = AgentSettingsSubject::reading();
        let json = canonical_json_of(&subject).unwrap();
        assert_eq!(json, r#"{"section":"reading"}"#);
        for forbidden in ["workId", "editionId", "mediaItemId", "contentKind"] {
            assert!(
                !json.contains(forbidden),
                "设置范围不得包含作品身份字段：{forbidden}"
            );
        }

        // 全局同分区覆盖；其它分区与任何资源级目标都不覆盖。
        assert!(subject.covers(SettingTarget::global(SettingsSection::Reading)));
        assert!(!subject.covers(SettingTarget::global(SettingsSection::Comic)));
        assert!(!subject.covers(SettingTarget::edition(EditionId::new())));
        assert!(!subject.covers(SettingTarget::media_item(
            EditionId::new(),
            MediaItemId::new()
        )));

        // 内容范围对全局目标的原有语义不变（任何作品范围都可以提全局建议）。
        let content =
            AgentScopeSubject::content(AgentSubject::work(WorkId::new(), AgentContentKind::Book));
        assert!(content.covers(SettingTarget::global(SettingsSection::Reading)));
        assert!(content.as_settings().is_none());
        assert!(AgentScopeSubject::settings(subject).as_settings().is_some());

        // 闭合集合：未知分区与未知字段都必须在反序列化边界被拒绝。
        assert!(serde_json::from_str::<AgentSettingsSection>("\"general\"").is_err());
        assert!(
            serde_json::from_str::<AgentSettingsSubject>(r#"{"section":"reading","workId":null}"#)
                .is_err()
        );
        assert!(
            serde_json::from_str::<AgentSettingsSubject>(r#"{"section":"reading","extra":1}"#)
                .is_err()
        );
    }

    fn settings_payload(revision: Option<&str>) -> AgentSettingsContextPayload {
        let mut payload = AgentSettingsContextPayload::new(
            AgentSettingsSubject::reading(),
            revision.map(str::to_owned),
        );
        payload.reading = AgentReadingSnapshot::from_settings(&ReadingSettings {
            custom_font_family: Some("Source Han Serif".into()),
            custom_background: Some("#f7f1e3".into()),
            custom_text: Some("#1d1d1f".into()),
            ..ReadingSettings::default()
        });
        payload
    }

    #[test]
    fn settings_context_hash_and_id_are_derived_from_the_payload() {
        let snapshot =
            AgentSettingsContextSnapshot::derive(settings_payload(Some("rev-0001"))).unwrap();
        assert!(is_canonical_digest(snapshot.context_hash()));
        assert_eq!(
            snapshot.context_hash(),
            canonical_digest(snapshot.canonical_json())
        );
        assert!(!snapshot.canonical_json().contains(snapshot.context_hash()));
        // 载荷是 canonical 形式（键按字节序、无多余空白），因此恢复路径的
        // "载荷 == 它的 canonical 重写"总能成立。字段值里的空格是数据，不是空白。
        assert_eq!(
            canonical_json_of(snapshot.payload()).unwrap(),
            snapshot.canonical_json()
        );
        assert_eq!(
            snapshot.id(),
            derive_settings_context_id(snapshot.context_hash()),
            "上下文 ID 必须是从 context_hash 派生的身份"
        );
        assert!(!snapshot.id().as_uuid().is_nil());

        // 同载荷 → 同 id、同 hash；revision 一变 → 另一个 id/hash。
        let again =
            AgentSettingsContextSnapshot::derive(settings_payload(Some("rev-0001"))).unwrap();
        assert_eq!(again.id(), snapshot.id());
        assert_eq!(again.context_hash(), snapshot.context_hash());
        let moved =
            AgentSettingsContextSnapshot::derive(settings_payload(Some("rev-0002"))).unwrap();
        assert_ne!(moved.id(), snapshot.id());
        assert_ne!(moved.context_hash(), snapshot.context_hash());
        // 从未保存过的分区（revision=None）与"有版本"是两种不同事实。
        let unsaved = AgentSettingsContextSnapshot::derive(settings_payload(None)).unwrap();
        assert_ne!(unsaved.context_hash(), snapshot.context_hash());
        assert!(unsaved.revision().is_none());

        // 恢复与篡改：id 必须仍是派生身份。
        let restored = AgentSettingsContextSnapshot::from_canonical_payload(
            snapshot.id(),
            snapshot.canonical_json(),
            snapshot.context_hash(),
        )
        .unwrap();
        assert_eq!(restored, snapshot);
        assert_eq!(
            AgentSettingsContextSnapshot::from_canonical_payload(
                AgentContextSnapshotId::new(),
                snapshot.canonical_json(),
                snapshot.context_hash(),
            )
            .unwrap_err()
            .code()
            .as_str(),
            "AGENT_SETTINGS_CONTEXT_INTEGRITY_MISMATCH",
            "非派生 id 不得冒充同一份上下文"
        );
        let tampered = snapshot.canonical_json().replace("rev-0001", "rev-0009");
        assert_ne!(tampered, snapshot.canonical_json());
        assert_eq!(
            AgentSettingsContextSnapshot::verify_integrity(&tampered, snapshot.context_hash())
                .unwrap_err()
                .code()
                .as_str(),
            "AGENT_SETTINGS_CONTEXT_INTEGRITY_MISMATCH"
        );
    }

    #[test]
    fn settings_snapshot_redacts_free_text_fields_and_never_carries_secrets() {
        let snapshot =
            AgentSettingsContextSnapshot::derive(settings_payload(Some("rev-0001"))).unwrap();
        let reading = snapshot.reading();
        assert_eq!(
            reading.custom_font_family.as_deref(),
            Some("Source Han Serif")
        );
        assert!(reading.redacted.is_empty());
        assert!(
            !snapshot
                .canonical_json()
                .contains(&["api", "_key"].concat())
        );

        // 命中敏感结构的自由文本字段必须整体丢弃并登记。
        let mut tainted = settings_payload(Some("rev-0001"));
        tainted.reading = AgentReadingSnapshot::from_settings(&ReadingSettings {
            custom_font_family: Some("C:/Windows/Fonts/msyh.ttc".into()),
            custom_background: Some("api_key=sk-live-1234567890".into()),
            custom_text: Some("https://example.invalid/theme".into()),
            ..ReadingSettings::default()
        });
        assert!(tainted.reading.custom_font_family.is_none());
        assert!(tainted.reading.custom_background.is_none());
        assert!(tainted.reading.custom_text.is_none());
        assert_eq!(
            tainted.reading.redacted.redacted_fields,
            vec![
                AgentSettingsRedactedField::CustomFontFamily,
                AgentSettingsRedactedField::CustomBackground,
                AgentSettingsRedactedField::CustomText,
            ]
        );
        let tainted = AgentSettingsContextSnapshot::derive(tainted).unwrap();
        for needle in ["C:/Windows", "sk-live-1234567890", "example.invalid"] {
            assert!(
                !tainted.canonical_json().contains(needle),
                "设置上下文不得携带敏感原文：{needle}"
            );
        }

        // 登记与字段现状必须自洽：登记了却还有值 = 损坏的事实。
        let mut inconsistent = settings_payload(Some("rev-0001"));
        inconsistent.reading.redacted.redacted_fields =
            vec![AgentSettingsRedactedField::CustomText];
        assert_eq!(
            AgentSettingsContextSnapshot::derive(inconsistent)
                .unwrap_err()
                .code()
                .as_str(),
            "AGENT_SETTINGS_CONTEXT_INVALID"
        );
    }

    /// 单值层面的敏感结构判据：**读路径**（快照/投影）与**写路径**（Agent 提案）
    /// 共用它，因此只写一条用例钉住"什么算敏感、什么不算"。
    #[test]
    fn setting_value_sensitivity_scan_is_shared_by_reads_and_writes() {
        // 合法的设置值：取色、字体族名、中文说明都不算敏感。
        for allowed in [
            "#f7f1e3",
            "Source Han Serif",
            "Noto Serif CJK SC",
            "霞鹜文楷",
            "system-ui",
        ] {
            assert!(
                !contains_sensitive_setting_value(allowed),
                "合法设置值不得被误判：{allowed}"
            );
            assert_eq!(redact_setting_value_for_agent(allowed), allowed);
        }
        // 绝对路径、endpoint、`Bearer` 片段与凭据赋值一律命中。
        for sensitive in [
            "C:/Windows/Fonts/msyh.ttc",
            "D:\\fonts\\my.ttf",
            "/etc/fonts/local.conf",
            "\\\\server\\share\\font.ttf",
            "https://example.invalid/font.css",
            "file:///usr/share/fonts",
            "Authorization: Bearer sk-live-1234567890",
            "api_key=sk-live-1234567890",
        ] {
            assert!(
                contains_sensitive_setting_value(sensitive),
                "敏感结构必须命中：{sensitive}"
            );
            assert_eq!(
                redact_setting_value_for_agent(sensitive),
                AGENT_REDACTED_VALUE,
                "敏感值的 Agent 投影只能是占位符：{sensitive}"
            );
        }
    }

    /// 资源偏好（版本/条目级覆盖）只在**三个自由文本字段**上做脱敏，其余字段原样保留。
    #[test]
    fn reading_patch_redaction_clears_only_sensitive_free_text_fields() {
        let patch = ReadingPatch {
            font_size: Some(crate::settings::ReadingFontSize::Large),
            custom_font_family: Some("C:/Windows/Fonts/msyh.ttc".into()),
            custom_background: Some("#101418".into()),
            custom_text: Some("password=hunter2xyz".into()),
            ..ReadingPatch::default()
        };
        let (redacted, fields) = redact_reading_patch_for_agent(&patch);
        assert!(redacted.custom_font_family.is_none());
        assert!(redacted.custom_text.is_none());
        // 合规的自由文本与档位原样保留：脱敏不是"抹掉整段偏好"。
        assert_eq!(redacted.custom_background.as_deref(), Some("#101418"));
        assert_eq!(redacted.font_size, patch.font_size);
        assert_eq!(
            fields,
            vec![
                AgentSettingsRedactedField::CustomFontFamily,
                AgentSettingsRedactedField::CustomText,
            ]
        );
        // 序列化投影不得携带被清空的原值。
        let json = canonical_json_of(&redacted).unwrap();
        for needle in ["C:/Windows", "hunter2xyz"] {
            assert!(!json.contains(needle), "脱敏后的 patch 不得携带 {needle}");
        }
    }

    #[test]
    fn preference_data_redaction_uses_a_placeholder_for_sensitive_resource_text() {
        let data = PreferenceData {
            reading: Some(ReadingPatch {
                custom_font_family: Some("D:/private/font.ttf".into()),
                custom_background: Some("#101418".into()),
                ..ReadingPatch::default()
            }),
            comic: None,
        };
        let redacted = redact_preference_data_for_agent(&data);
        let reading = redacted
            .reading
            .as_ref()
            .expect("reading section remains present");
        assert_eq!(
            reading.custom_font_family.as_deref(),
            Some(AGENT_REDACTED_VALUE)
        );
        assert_eq!(reading.custom_background.as_deref(), Some("#101418"));
        let json = serde_json::to_string(&redacted).unwrap();
        assert!(!json.contains("D:/private/font.ttf"));
        assert!(json.contains(AGENT_REDACTED_VALUE));
    }

    /// Agent 提案的自由文本校验：命中即拒绝，且**错误里没有原值**。
    #[test]
    fn agent_proposal_free_text_is_rejected_without_echoing_the_value() {
        let sensitive = "C:/Windows/Fonts/msyh.ttc";
        let reading_patch = ReadingPatch {
            custom_font_family: Some(sensitive.into()),
            ..ReadingPatch::default()
        };
        for change in [
            SettingProposalChange::SettingsPatch(SettingsPatch::Reading(reading_patch.clone())),
            SettingProposalChange::ResourcePreference(crate::settings::PreferenceData {
                reading: Some(reading_patch.clone()),
                comic: None,
            }),
            SettingProposalChange::AgentResourcePreferencePatch(crate::settings::PreferenceData {
                reading: Some(reading_patch.clone()),
                comic: None,
            }),
        ] {
            let error = validate_agent_proposal_free_text(&change).unwrap_err();
            assert_eq!(error.code().as_str(), "AGENT_PROPOSAL_SENSITIVE_FREE_TEXT");
            assert!(
                !error.user_message().contains(sensitive)
                    && !error.user_message().contains("Windows"),
                "拒绝文案不得回显原值：{}",
                error.user_message()
            );
            assert_eq!(
                error.user_message(),
                "Agent 提案包含不允许的敏感自由文本，已拒绝（原值不回显）"
            );
        }

        // 合规的自由文本（字体族名、取色、中文）必须放行，否则 Agent 连正常建议都提不了。
        for change in [
            SettingProposalChange::SettingsPatch(SettingsPatch::Reading(ReadingPatch {
                custom_font_family: Some("Source Han Serif".into()),
                custom_background: Some("#f7f1e3".into()),
                custom_text: Some("#2b2b2b".into()),
                ..ReadingPatch::default()
            })),
            SettingProposalChange::ResourcePreference(crate::settings::PreferenceData {
                reading: Some(ReadingPatch {
                    custom_text: Some("霞鹜文楷".into()),
                    ..ReadingPatch::default()
                }),
                comic: None,
            }),
            SettingProposalChange::AgentResourcePreferencePatch(crate::settings::PreferenceData {
                reading: Some(ReadingPatch {
                    custom_text: Some("霞鹜文楷".into()),
                    ..ReadingPatch::default()
                }),
                comic: None,
            }),
        ] {
            validate_agent_proposal_free_text(&change).unwrap();
        }

        // 与阅读自由文本无关的分区/操作不受这条规则影响。
        assert!(
            validate_agent_proposal_free_text(&SettingProposalChange::SettingsPatch(
                SettingsPatch::Comic(Default::default())
            ))
            .is_ok()
        );
        assert!(
            validate_agent_proposal_free_text(&SettingProposalChange::ResourcePreference(
                crate::settings::PreferenceData {
                    reading: None,
                    comic: Some(Default::default()),
                }
            ))
            .is_ok()
        );
    }

    #[test]
    fn settings_context_capability_manifest_is_closed() {
        let payload = settings_payload(Some("rev-0001"));
        assert!(payload.capabilities.grants(AgentCapability::SettingsRead));
        assert!(
            payload
                .capabilities
                .grants(AgentCapability::SettingsProposal)
        );
        assert!(!payload.capabilities.grants(AgentCapability::SecretRead));
        assert!(
            !payload
                .capabilities
                .grants(AgentCapability::FilesystemWrite)
        );

        let raw = canonical_json_of(&payload).unwrap();
        // 打开未实现能力：反序列化能过，但构造/校验必须拒绝。
        let opened = serde_json::from_str::<AgentSettingsContextPayload>(
            &raw.replace(r#""secretRead":false"#, r#""secretRead":true"#),
        )
        .unwrap();
        assert!(AgentSettingsContextSnapshot::derive(opened).is_err());
        // 未知能力字段（例如未来才有的 shell 能力）在反序列化边界就被拒绝。
        let unknown = raw.replace(
            r#""capabilities":{"#,
            r#""capabilities":{"shellExec":true,"#,
        );
        assert!(serde_json::from_str::<AgentSettingsContextPayload>(&unknown).is_err());
        // 载荷未知字段（变相塞入任意 JSON）同样拒绝。canonical 键序里
        // "revision" 紧跟在顶层 "version" 之后，因此锚点不会撞上嵌套的同名字段。
        let anchor = r#""revision":"#;
        assert!(raw.contains(anchor));
        let with_extra = raw.replacen(
            anchor,
            r#""rawArguments":{"sql":"select 1"},"revision":"#,
            1,
        );
        assert!(serde_json::from_str::<AgentSettingsContextPayload>(&with_extra).is_err());
    }

    #[test]
    fn settings_binding_uses_the_settings_scope_end_to_end() {
        let snapshot =
            AgentSettingsContextSnapshot::derive(settings_payload(Some("rev-0001"))).unwrap();
        let binding = AgentActionBinding::pending(
            SettingProposalId::new(),
            AgentActionBindingInput::new(
                AgentSessionId::new(),
                AgentRequestId::new(),
                snapshot.id(),
                snapshot.context_hash(),
                snapshot.subject(),
                AgentActionKind::SettingsProposal,
            )
            .unwrap(),
            UtcMillis::from_millis(1_000),
            UtcMillis::from_millis(5_000),
        )
        .unwrap();
        assert_eq!(
            binding.subject().as_settings(),
            Some(AgentSettingsSubject::reading())
        );
        assert!(binding.subject().as_content().is_none());
        assert_eq!(
            canonical_json_of(&binding.subject()).unwrap(),
            r#"{"section":"reading"}"#
        );
    }
}
