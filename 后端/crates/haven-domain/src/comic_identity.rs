//! 漫画来源、版本线、章节内容身份与页码迁移规则。
//!
//! 本模块只处理纯领域判断，不访问 SQLite、Tauri、网络或文件系统。
//! 运行时的 `pageId`、grant、URL 和归档 entry 不属于这里的身份输入。
//!
//! 设计原则：
//! - 来源 ID 是证据，不自动等同于 Haven 的 Work/MediaItem ID；
//! - Edition 的未知字段不是 wildcard；
//! - 不同远端章节可以在内容证据充分时汇聚；
//! - 页码变化允许最佳努力恢复，但结果必须带置信度和可回滚语义。

use serde::{Deserialize, Serialize};

use haven_common::UtcMillis;

use crate::comic_catalog::ComicChapterSourceStatus;
use crate::entities::Progress;
use crate::ids::{ComicProgressMigrationId, MediaItemId};

/// Edition 画像中可比较的字符串字段。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum IdentityFacet {
    Unknown,
    Known(String),
    NotApplicable,
}

impl IdentityFacet {
    pub fn known(value: impl AsRef<str>) -> Self {
        let value = normalize_label(value.as_ref());
        if value.is_empty() {
            Self::Unknown
        } else {
            Self::Known(value)
        }
    }

    pub fn unknown() -> Self {
        Self::Unknown
    }

    pub fn as_known(&self) -> Option<&str> {
        match self {
            Self::Known(value) => Some(value),
            Self::Unknown | Self::NotApplicable => None,
        }
    }
}

/// 扫描组标签的语义。镜像站/搬运标签不应单独制造 Edition。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ScanGroupFacet {
    Unknown,
    ContentLine(String),
    MirrorLabel(String),
    NotApplicable,
}

impl ScanGroupFacet {
    pub fn content_line(value: impl AsRef<str>) -> Self {
        let value = normalize_label(value.as_ref());
        if value.is_empty() {
            Self::Unknown
        } else {
            Self::ContentLine(value)
        }
    }

    pub fn mirror_label(value: impl AsRef<str>) -> Self {
        let value = normalize_label(value.as_ref());
        if value.is_empty() {
            Self::Unknown
        } else {
            Self::MirrorLabel(value)
        }
    }
}

/// 漫画内容本身的颜色版本，不包括阅读器的运行时灰度显示滤镜。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum ColorMode {
    #[default]
    Unknown,
    FullColor,
    Grayscale,
    Mixed,
}

/// Edition 的内容画像。
///
/// `Edition` 实体仍然保存既有通用字段；本画像承载漫画版本线新增的
/// 翻译线、扫描组和颜色语义。language 可以由既有 Edition.language 投影进来。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct EditionProfile {
    pub language: IdentityFacet,
    pub translation_line: IdentityFacet,
    pub scan_group: ScanGroupFacet,
    pub color_mode: ColorMode,
}

impl Default for EditionProfile {
    fn default() -> Self {
        Self {
            language: IdentityFacet::Unknown,
            translation_line: IdentityFacet::Unknown,
            scan_group: ScanGroupFacet::Unknown,
            color_mode: ColorMode::Unknown,
        }
    }
}

impl EditionProfile {
    pub fn from_language(language: Option<&str>) -> Self {
        Self {
            language: language
                .map(IdentityFacet::known)
                .unwrap_or_else(IdentityFacet::unknown),
            ..Self::default()
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EditionFacetKind {
    Language,
    TranslationLine,
    ScanGroup,
    ColorMode,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EditionEvidence {
    Exact(EditionFacetKind),
    Unknown(EditionFacetKind),
    MirrorLabelIgnored,
    Conflict(EditionFacetKind),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EditionMatchKind {
    Same,
    Distinct,
    Candidate,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct EditionMatch {
    pub kind: EditionMatchKind,
    pub evidence: Vec<EditionEvidence>,
}

/// 比较两个 Edition 画像。
///
/// 已知强字段冲突优先于其他证据；未知字段只会把结果降级为 Candidate，
/// 不会自动匹配到任意已知值。
pub fn compare_edition_profiles(left: &EditionProfile, right: &EditionProfile) -> EditionMatch {
    let mut evidence = Vec::new();
    let mut has_conflict = false;
    let mut has_unknown = false;

    compare_identity_facet(
        EditionFacetKind::Language,
        &left.language,
        &right.language,
        &mut evidence,
        &mut has_conflict,
        &mut has_unknown,
    );
    compare_identity_facet(
        EditionFacetKind::TranslationLine,
        &left.translation_line,
        &right.translation_line,
        &mut evidence,
        &mut has_conflict,
        &mut has_unknown,
    );
    compare_scan_group(
        &left.scan_group,
        &right.scan_group,
        &mut evidence,
        &mut has_conflict,
        &mut has_unknown,
    );
    compare_color_mode(
        left.color_mode,
        right.color_mode,
        &mut evidence,
        &mut has_conflict,
        &mut has_unknown,
    );

    let kind = if has_conflict {
        EditionMatchKind::Distinct
    } else if has_unknown {
        EditionMatchKind::Candidate
    } else {
        EditionMatchKind::Same
    };
    EditionMatch { kind, evidence }
}

/// 判断目录刷新时两个章节是否可以复用同一个 Edition 容器。
///
/// 这和 `compare_edition_profiles` 的“能否直接证明相同”不同：两个来源
/// 都明确报告 `unknown` 时，不应因为未知而给每个章节制造一个 Edition；
/// 但 `unknown` 与任意已知值仍然不匹配，避免把未知当成 wildcard。镜像
/// 标签同样只作为来源展示事实，不单独拆分 Edition。
pub fn edition_profiles_can_share_container(left: &EditionProfile, right: &EditionProfile) -> bool {
    same_identity_facet_for_container(&left.language, &right.language)
        && same_identity_facet_for_container(&left.translation_line, &right.translation_line)
        && same_scan_group_for_container(&left.scan_group, &right.scan_group)
        && left.color_mode == right.color_mode
}

fn same_identity_facet_for_container(left: &IdentityFacet, right: &IdentityFacet) -> bool {
    match (left, right) {
        (IdentityFacet::Known(left), IdentityFacet::Known(right)) => left == right,
        (IdentityFacet::Unknown, IdentityFacet::Unknown)
        | (IdentityFacet::NotApplicable, IdentityFacet::NotApplicable) => true,
        _ => false,
    }
}

fn same_scan_group_for_container(left: &ScanGroupFacet, right: &ScanGroupFacet) -> bool {
    // 镜像站/搬运标签描述的是来源展示事实，不是内容型扫描组。
    // 只要任意一侧是 MirrorLabel，它都不能单独把两个章节拆成新的 Edition；
    // 另一侧的真实 ContentLine/Unknown/NotApplicable 仍由其他事实决定。
    if matches!(left, ScanGroupFacet::MirrorLabel(_))
        || matches!(right, ScanGroupFacet::MirrorLabel(_))
    {
        return true;
    }
    match (left, right) {
        (ScanGroupFacet::ContentLine(left), ScanGroupFacet::ContentLine(right)) => left == right,
        (ScanGroupFacet::Unknown, ScanGroupFacet::Unknown)
        | (ScanGroupFacet::NotApplicable, ScanGroupFacet::NotApplicable) => true,
        _ => false,
    }
}

fn compare_identity_facet(
    facet: EditionFacetKind,
    left: &IdentityFacet,
    right: &IdentityFacet,
    evidence: &mut Vec<EditionEvidence>,
    has_conflict: &mut bool,
    has_unknown: &mut bool,
) {
    match (left, right) {
        (IdentityFacet::Known(left), IdentityFacet::Known(right)) if left == right => {
            evidence.push(EditionEvidence::Exact(facet));
        }
        (IdentityFacet::Known(_), IdentityFacet::Known(_)) => {
            *has_conflict = true;
            evidence.push(EditionEvidence::Conflict(facet));
        }
        (IdentityFacet::NotApplicable, IdentityFacet::NotApplicable) => {
            evidence.push(EditionEvidence::Exact(facet));
        }
        _ => {
            *has_unknown = true;
            evidence.push(EditionEvidence::Unknown(facet));
        }
    }
}

fn compare_scan_group(
    left: &ScanGroupFacet,
    right: &ScanGroupFacet,
    evidence: &mut Vec<EditionEvidence>,
    has_conflict: &mut bool,
    has_unknown: &mut bool,
) {
    match (left, right) {
        (ScanGroupFacet::ContentLine(left), ScanGroupFacet::ContentLine(right))
            if left == right =>
        {
            evidence.push(EditionEvidence::Exact(EditionFacetKind::ScanGroup));
        }
        (ScanGroupFacet::ContentLine(_), ScanGroupFacet::ContentLine(_)) => {
            *has_conflict = true;
            evidence.push(EditionEvidence::Conflict(EditionFacetKind::ScanGroup));
        }
        (ScanGroupFacet::MirrorLabel(_), ScanGroupFacet::MirrorLabel(_))
        | (ScanGroupFacet::MirrorLabel(_), ScanGroupFacet::NotApplicable)
        | (ScanGroupFacet::NotApplicable, ScanGroupFacet::MirrorLabel(_)) => {
            evidence.push(EditionEvidence::MirrorLabelIgnored);
        }
        (ScanGroupFacet::NotApplicable, ScanGroupFacet::NotApplicable) => {
            evidence.push(EditionEvidence::Exact(EditionFacetKind::ScanGroup));
        }
        _ => {
            *has_unknown = true;
            evidence.push(EditionEvidence::Unknown(EditionFacetKind::ScanGroup));
        }
    }
}

fn compare_color_mode(
    left: ColorMode,
    right: ColorMode,
    evidence: &mut Vec<EditionEvidence>,
    has_conflict: &mut bool,
    has_unknown: &mut bool,
) {
    match (left, right) {
        (ColorMode::Unknown, _) | (_, ColorMode::Unknown) => {
            *has_unknown = true;
            evidence.push(EditionEvidence::Unknown(EditionFacetKind::ColorMode));
        }
        (left, right) if left == right => {
            evidence.push(EditionEvidence::Exact(EditionFacetKind::ColorMode));
        }
        _ => {
            *has_conflict = true;
            evidence.push(EditionEvidence::Conflict(EditionFacetKind::ColorMode));
        }
    }
}

/// 来源侧章节身份。字段是 provider 校验后的 opaque 值，不是 URL。
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct ChapterSourceIdentity {
    pub source_key: String,
    pub remote_work_id: String,
    pub remote_chapter_id: String,
}

/// 一个远端章节引用到 Haven MediaItem 的绑定。
///
/// `identity` 是来源侧的稳定 opaque 身份；`metadata` 中的 Edition 画像
/// 优先来自来源章节最近一次观察，旧数据没有来源观察时才回退到关联 Edition
/// 的画像，不把同一事实复制成第二份。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct ChapterSourceRef {
    pub media_item_id: MediaItemId,
    pub identity: ChapterSourceIdentity,
    pub metadata: ComicChapterMetadata,
    /// 来源在最近一次成功目录观察中的顺序，只用于展示排序，不是身份。
    pub source_order: u32,
    /// 来源章节当前状态。`Missing` 只由完整目录刷新产生。
    pub availability: ComicChapterSourceStatus,
    /// provider 清洗后的发布时间和更新时间，作为展示事实保存。
    pub published_at: Option<String>,
    pub source_updated_at: Option<String>,
    /// 最近一次包含该章节的成功目录 generation；手工登记的旧数据可以为空。
    pub last_seen_generation: Option<u64>,
    pub updated_at: UtcMillis,
}

impl ChapterSourceIdentity {
    pub fn new(
        source_key: impl AsRef<str>,
        remote_work_id: impl AsRef<str>,
        remote_chapter_id: impl AsRef<str>,
    ) -> Option<Self> {
        if has_opaque_control_character(source_key.as_ref())
            || has_opaque_control_character(remote_work_id.as_ref())
            || has_opaque_control_character(remote_chapter_id.as_ref())
        {
            return None;
        }
        let source_key = source_key.as_ref().trim();
        let remote_work_id = remote_work_id.as_ref().trim();
        let remote_chapter_id = remote_chapter_id.as_ref().trim();
        if source_key.is_empty() || remote_work_id.is_empty() || remote_chapter_id.is_empty() {
            return None;
        }
        Some(Self {
            source_key: source_key.to_owned(),
            remote_work_id: remote_work_id.to_owned(),
            remote_chapter_id: remote_chapter_id.to_owned(),
        })
    }
}

/// 章节的可比较元数据。章节号和标题只参与辅助匹配，不是主键。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub struct ComicChapterMetadata {
    pub edition_profile: EditionProfile,
    pub chapter_number: Option<f64>,
    pub volume_number: Option<f64>,
    pub title: Option<String>,
    pub page_count: Option<u32>,
    pub authoritative_content_key: Option<String>,
}

/// 页面身份只保留 provider 稳定 key 或内容指纹；不包含 pageId/grant/URL。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub struct PageIdentity {
    pub stable_key: Option<String>,
    pub fingerprint: Option<String>,
}

impl PageIdentity {
    pub fn stable(key: impl AsRef<str>) -> Self {
        Self {
            stable_key: non_empty(key.as_ref()),
            fingerprint: None,
        }
    }

    pub fn fingerprint(value: impl AsRef<str>) -> Self {
        Self {
            stable_key: None,
            fingerprint: non_empty(value.as_ref()),
        }
    }

    pub fn with_fingerprint(mut self, value: impl AsRef<str>) -> Self {
        self.fingerprint = non_empty(value.as_ref());
        self
    }
}

/// 一次完整页面观察的快照。revision 保护的是页面序列整体，而不是单个
/// page index；没有任何历史观察时 revision 为 None，首次写入必须以
/// `replace_if_revision(None)` 创建 state。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub struct ComicPageIdentitySnapshot {
    pub pages: Vec<PageIdentity>,
    pub revision: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ChapterMatchKind {
    SameRemoteChapter,
    SameContent,
    SameLogicalChapterVariant,
    Candidate,
    Unrelated,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MatchConfidence {
    High,
    Medium,
    Low,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProgressMigrationMode {
    Shared,
    OneTime,
    Suggested,
    None,
}

/// 进度迁移快照的生命周期。快照在应用迁移时写入，撤销时只允许从
/// `Applied` 转为 `Reverted`，不会删除审计事实。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProgressMigrationState {
    Applied,
    Reverted,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ChapterEvidence {
    SameRemoteIdentity,
    AuthoritativeContentKey,
    ConflictingAuthoritativeContentKey,
    EditionCompatible,
    EditionConflict,
    ExactPageIdentity { matched: usize },
    PartialPageIdentity { matched: usize },
    MatchingChapterMetadata,
    WeakChapterMetadata,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct ChapterMatch {
    pub kind: ChapterMatchKind,
    pub confidence: MatchConfidence,
    pub progress_migration: ProgressMigrationMode,
    pub evidence: Vec<ChapterEvidence>,
}

/// 比较两个来源章节。
///
/// 只有同一远端身份、权威内容 key 或完整页面身份对应可以直接共享
/// MediaItem/Progress。元数据相似最多产生候选或一次性迁移建议。
pub fn compare_chapters(
    left_identity: &ChapterSourceIdentity,
    left: &ComicChapterMetadata,
    left_pages: &[PageIdentity],
    right_identity: &ChapterSourceIdentity,
    right: &ComicChapterMetadata,
    right_pages: &[PageIdentity],
) -> ChapterMatch {
    compare_chapters_with_page_scope(
        left_identity,
        left,
        left_pages,
        right_identity,
        right,
        right_pages,
        false,
    )
}

/// 比较同一个 `MediaItem` 内的两个章节来源观察。
///
/// 同一条持久化页面序列中的 provider stable key 可以用于识别插页、删页
/// 和重排；跨 `MediaItem` 时必须由调用方使用 `compare_chapters`，此时 stable
/// key 不会单独成为内容证据。
pub fn compare_chapters_within_media_item(
    left_identity: &ChapterSourceIdentity,
    left: &ComicChapterMetadata,
    left_pages: &[PageIdentity],
    right_identity: &ChapterSourceIdentity,
    right: &ComicChapterMetadata,
    right_pages: &[PageIdentity],
) -> ChapterMatch {
    compare_chapters_with_page_scope(
        left_identity,
        left,
        left_pages,
        right_identity,
        right,
        right_pages,
        true,
    )
}

fn compare_chapters_with_page_scope(
    left_identity: &ChapterSourceIdentity,
    left: &ComicChapterMetadata,
    left_pages: &[PageIdentity],
    right_identity: &ChapterSourceIdentity,
    right: &ComicChapterMetadata,
    right_pages: &[PageIdentity],
    allow_stable_page_identity: bool,
) -> ChapterMatch {
    if left_identity == right_identity {
        return ChapterMatch {
            kind: ChapterMatchKind::SameRemoteChapter,
            confidence: MatchConfidence::High,
            progress_migration: ProgressMigrationMode::Shared,
            evidence: vec![ChapterEvidence::SameRemoteIdentity],
        };
    }

    let edition_match = compare_edition_profiles(&left.edition_profile, &right.edition_profile);
    if edition_match.kind == EditionMatchKind::Distinct {
        return ChapterMatch {
            kind: ChapterMatchKind::Unrelated,
            confidence: MatchConfidence::High,
            progress_migration: ProgressMigrationMode::None,
            evidence: vec![ChapterEvidence::EditionConflict],
        };
    }

    let mut evidence = vec![ChapterEvidence::EditionCompatible];
    let authoritative_key_conflict = match (
        left.authoritative_content_key.as_deref(),
        right.authoritative_content_key.as_deref(),
    ) {
        (Some(left_key), Some(right_key))
            if !left_key.is_empty() && !right_key.is_empty() && left_key == right_key =>
        {
            evidence.push(ChapterEvidence::AuthoritativeContentKey);
            return ChapterMatch {
                kind: ChapterMatchKind::SameContent,
                confidence: MatchConfidence::High,
                progress_migration: ProgressMigrationMode::Shared,
                evidence,
            };
        }
        (Some(left_key), Some(right_key))
            if !left_key.is_empty() && !right_key.is_empty() && left_key != right_key =>
        {
            evidence.push(ChapterEvidence::ConflictingAuthoritativeContentKey);
            true
        }
        _ => false,
    };

    let page_matches =
        count_page_identity_matches(left_pages, right_pages, allow_stable_page_identity);
    if !authoritative_key_conflict
        && !left_pages.is_empty()
        && left_pages.len() == right_pages.len()
        && page_matches == left_pages.len()
    {
        evidence.push(ChapterEvidence::ExactPageIdentity {
            matched: page_matches,
        });
        return ChapterMatch {
            kind: ChapterMatchKind::SameContent,
            confidence: MatchConfidence::High,
            progress_migration: ProgressMigrationMode::Shared,
            evidence,
        };
    }

    let metadata_matches = matching_chapter_metadata(left, right);
    if page_matches > 0 {
        evidence.push(ChapterEvidence::PartialPageIdentity {
            matched: page_matches,
        });
    }
    if metadata_matches {
        evidence.push(ChapterEvidence::MatchingChapterMetadata);
    } else {
        evidence.push(ChapterEvidence::WeakChapterMetadata);
    }

    if authoritative_key_conflict {
        return ChapterMatch {
            kind: ChapterMatchKind::Candidate,
            confidence: MatchConfidence::Low,
            progress_migration: ProgressMigrationMode::Suggested,
            evidence,
        };
    }

    if page_matches > 0 && metadata_matches {
        ChapterMatch {
            kind: ChapterMatchKind::SameLogicalChapterVariant,
            confidence: MatchConfidence::Medium,
            progress_migration: ProgressMigrationMode::OneTime,
            evidence,
        }
    } else if metadata_matches {
        ChapterMatch {
            kind: ChapterMatchKind::Candidate,
            confidence: MatchConfidence::Low,
            progress_migration: ProgressMigrationMode::Suggested,
            evidence,
        }
    } else {
        ChapterMatch {
            kind: ChapterMatchKind::Candidate,
            confidence: MatchConfidence::Low,
            progress_migration: ProgressMigrationMode::None,
            evidence,
        }
    }
}

fn matching_chapter_metadata(left: &ComicChapterMetadata, right: &ComicChapterMetadata) -> bool {
    let chapter_matches = match (left.chapter_number, right.chapter_number) {
        (Some(left), Some(right)) => (left - right).abs() <= 0.0001,
        _ => false,
    };
    let volume_matches = match (left.volume_number, right.volume_number) {
        (Some(left), Some(right)) => (left - right).abs() <= 0.0001,
        (None, None) => true,
        _ => false,
    };
    let title_matches = match (left.title.as_deref(), right.title.as_deref()) {
        (Some(left), Some(right)) => normalize_label(left) == normalize_label(right),
        _ => false,
    };
    let page_count_matches = match (left.page_count, right.page_count) {
        (Some(left), Some(right)) => left == right,
        _ => false,
    };
    (chapter_matches && volume_matches) || (title_matches && page_count_matches)
}

fn count_page_identity_matches(
    left: &[PageIdentity],
    right: &[PageIdentity],
    allow_stable_page_identity: bool,
) -> usize {
    let mut used = vec![false; right.len()];
    let mut matched = 0;
    for left_page in left {
        if let Some((index, _)) = right.iter().enumerate().find(|(index, right_page)| {
            !used[*index]
                && page_identity_match(left_page, right_page, allow_stable_page_identity)
                    == PageIdentityMatch::Match
        }) {
            used[index] = true;
            matched += 1;
        }
    }
    matched
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PageIdentityMatch {
    Match,
    Conflict,
    NoMatch,
}

fn page_identity_match(
    left: &PageIdentity,
    right: &PageIdentity,
    allow_stable_page_identity: bool,
) -> PageIdentityMatch {
    let stable_equal = matches!(
        (left.stable_key.as_deref(), right.stable_key.as_deref()),
        (Some(left), Some(right)) if left == right
    );
    let fingerprint_match = matches!(
        (left.fingerprint.as_deref(), right.fingerprint.as_deref()),
        (Some(left), Some(right)) if left == right
    );
    let fingerprint_conflict = matches!(
        (left.fingerprint.as_deref(), right.fingerprint.as_deref()),
        (Some(left), Some(right)) if left != right
    );

    if stable_equal && fingerprint_conflict {
        PageIdentityMatch::Conflict
    } else if (allow_stable_page_identity && stable_equal) || fingerprint_match {
        PageIdentityMatch::Match
    } else {
        PageIdentityMatch::NoMatch
    }
}

/// 页面迁移的置信度。低置信度也可以自动恢复，但必须显示原因并保留撤销快照。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PageMappingConfidence {
    High,
    Medium,
    Low,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PageMappingStrategy {
    StableKey,
    ContentFingerprint,
    ReorderedAnchor,
    NearestSurvivingPage,
    ProportionalFallback,
    NoTarget,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct PageMigration {
    pub target_page_index: Option<u32>,
    pub confidence: PageMappingConfidence,
    pub strategy: PageMappingStrategy,
    pub reversible: bool,
}

/// 一次漫画进度迁移的可撤销快照。
///
/// 这里保存的是内部 Progress 状态和页面映射证据，不是普通 Wire。由于
/// 漫画 Locator 只含 MediaItem/page index，快照不会携带 pageId、grant、URL
/// 或归档 entry。跨 MediaItem 迁移时 `old_target_progress` 为目标原有状态；
/// 为 None 表示目标在迁移前没有进度，撤销时可以安全删除迁移产生的目标行。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct ComicProgressMigrationSnapshot {
    pub id: ComicProgressMigrationId,
    pub source_media_item_id: MediaItemId,
    pub target_media_item_id: MediaItemId,
    pub source_revision: String,
    pub target_revision_before: Option<String>,
    pub old_progress: Progress,
    pub old_target_progress: Option<Progress>,
    pub new_progress: Progress,
    pub mode: ProgressMigrationMode,
    pub confidence: PageMappingConfidence,
    pub strategy: PageMappingStrategy,
    pub evidence: Vec<ChapterEvidence>,
    pub created_at: UtcMillis,
    pub applied_revision: Option<String>,
    pub state: ProgressMigrationState,
    pub reverted_at: Option<UtcMillis>,
}

/// 将旧页码映射到新页面序列。
///
/// 优先使用唯一 stable key/fingerprint，因此可以处理插页、删页和重排。
/// 当前页消失时使用邻近的可识别页面；完全没有页面身份时才使用比例位置，
/// 结果为 Low 且 `reversible=true`。
pub fn migrate_page_index(
    old_pages: &[PageIdentity],
    new_pages: &[PageIdentity],
    old_page_index: u32,
) -> PageMigration {
    if new_pages.is_empty() {
        return PageMigration {
            target_page_index: None,
            confidence: PageMappingConfidence::Low,
            strategy: PageMappingStrategy::NoTarget,
            reversible: true,
        };
    }

    let old_index = if old_pages.is_empty() {
        old_page_index as usize
    } else {
        (old_page_index as usize).min(old_pages.len().saturating_sub(1))
    };

    if let Some(old_page) = old_pages.get(old_index) {
        if let Some((new_index, strategy)) = unique_page_match(old_page, new_pages) {
            return PageMigration {
                target_page_index: Some(new_index as u32),
                confidence: PageMappingConfidence::High,
                strategy,
                reversible: true,
            };
        }

        if let Some((new_index, strategy)) =
            nearest_surviving_match(old_pages, new_pages, old_index)
        {
            return PageMigration {
                target_page_index: Some(new_index as u32),
                confidence: PageMappingConfidence::Medium,
                strategy,
                reversible: true,
            };
        }
    }

    let target = proportional_index(old_index, old_pages.len(), new_pages.len());
    PageMigration {
        target_page_index: Some(target as u32),
        confidence: PageMappingConfidence::Low,
        strategy: PageMappingStrategy::ProportionalFallback,
        reversible: true,
    }
}

fn unique_page_match(
    page: &PageIdentity,
    new_pages: &[PageIdentity],
) -> Option<(usize, PageMappingStrategy)> {
    let stable = page.stable_key.as_deref().and_then(|key| {
        let matches: Vec<usize> = new_pages
            .iter()
            .enumerate()
            .filter_map(|(index, candidate)| {
                (candidate.stable_key.as_deref() == Some(key)
                    && page_identity_match(page, candidate, true) == PageIdentityMatch::Match)
                    .then_some(index)
            })
            .collect();
        if matches.len() == 1 {
            Some(matches[0])
        } else {
            None
        }
    });
    if let Some(index) = stable {
        return Some((index, PageMappingStrategy::StableKey));
    }

    page.fingerprint.as_deref().and_then(|fingerprint| {
        let matches: Vec<usize> = new_pages
            .iter()
            .enumerate()
            .filter_map(|(index, candidate)| {
                (candidate.fingerprint.as_deref() == Some(fingerprint)
                    && page_identity_match(page, candidate, true) == PageIdentityMatch::Match)
                    .then_some(index)
            })
            .collect();
        if matches.len() == 1 {
            Some((matches[0], PageMappingStrategy::ContentFingerprint))
        } else {
            None
        }
    })
}

fn nearest_surviving_match(
    old_pages: &[PageIdentity],
    new_pages: &[PageIdentity],
    old_index: usize,
) -> Option<(usize, PageMappingStrategy)> {
    for distance in 1..old_pages.len().max(1) {
        let next = old_index + distance;
        if let Some(page) = old_pages.get(next) {
            if let Some((index, _)) = unique_page_match(page, new_pages) {
                return Some((index, PageMappingStrategy::NearestSurvivingPage));
            }
        }
        if let Some(previous) = old_index.checked_sub(distance) {
            if let Some(page) = old_pages.get(previous) {
                if let Some((index, _)) = unique_page_match(page, new_pages) {
                    return Some((index, PageMappingStrategy::NearestSurvivingPage));
                }
            }
        }
    }
    None
}

fn proportional_index(old_index: usize, old_len: usize, new_len: usize) -> usize {
    if new_len <= 1 || old_len <= 1 {
        return old_index.min(new_len.saturating_sub(1));
    }
    let ratio = old_index as f64 / (old_len - 1) as f64;
    (ratio * (new_len - 1) as f64).round() as usize
}

fn normalize_label(value: &str) -> String {
    value
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .to_lowercase()
}

/// 身份字段允许空格修剪，但不能静默接收 ASCII 控制字符。
///
/// 统一边界只拒绝 C0 控制区和 DEL；普通 Unicode 文本中的换行等控制
/// 也不会被误当作稳定 opaque identity。
pub fn has_opaque_control_character(value: &str) -> bool {
    value.chars().any(|character| {
        let code_point = character as u32;
        code_point <= 0x1f || code_point == 0x7f
    })
}

fn non_empty(value: &str) -> Option<String> {
    let value = value.trim();
    (!value.is_empty()).then(|| value.to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn profile(
        language: &str,
        translation: &str,
        scan: ScanGroupFacet,
        color: ColorMode,
    ) -> EditionProfile {
        EditionProfile {
            language: IdentityFacet::known(language),
            translation_line: IdentityFacet::known(translation),
            scan_group: scan,
            color_mode: color,
        }
    }

    fn chapter_metadata(profile: EditionProfile) -> ComicChapterMetadata {
        ComicChapterMetadata {
            edition_profile: profile,
            chapter_number: Some(12.0),
            volume_number: Some(2.0),
            title: Some("第 12 章".into()),
            page_count: Some(3),
            authoritative_content_key: None,
        }
    }

    fn source_chapter(id: &str) -> ChapterSourceIdentity {
        ChapterSourceIdentity::new("mangadex", "work-1", id).unwrap()
    }

    #[test]
    fn language_translation_scan_and_color_conflicts_split_editions() {
        let left = profile(
            "zh-CN",
            "group-a",
            ScanGroupFacet::content_line("scan-a"),
            ColorMode::Grayscale,
        );
        for right in [
            profile(
                "ja",
                "group-a",
                ScanGroupFacet::content_line("scan-a"),
                ColorMode::Grayscale,
            ),
            profile(
                "zh-CN",
                "group-b",
                ScanGroupFacet::content_line("scan-a"),
                ColorMode::Grayscale,
            ),
            profile(
                "zh-CN",
                "group-a",
                ScanGroupFacet::content_line("scan-b"),
                ColorMode::Grayscale,
            ),
            profile(
                "zh-CN",
                "group-a",
                ScanGroupFacet::content_line("scan-a"),
                ColorMode::FullColor,
            ),
        ] {
            assert_eq!(
                compare_edition_profiles(&left, &right).kind,
                EditionMatchKind::Distinct
            );
        }
    }

    #[test]
    fn unknown_is_candidate_and_mirror_label_is_not_an_edition_conflict() {
        let known = profile(
            "zh-CN",
            "group-a",
            ScanGroupFacet::content_line("scan-a"),
            ColorMode::Grayscale,
        );
        let unknown = EditionProfile::default();
        assert_eq!(
            compare_edition_profiles(&known, &unknown).kind,
            EditionMatchKind::Candidate
        );

        let mirror_a = profile(
            "zh-CN",
            "group-a",
            ScanGroupFacet::mirror_label("mirror-a"),
            ColorMode::Grayscale,
        );
        let mirror_b = profile(
            "zh-CN",
            "group-a",
            ScanGroupFacet::mirror_label("mirror-b"),
            ColorMode::Grayscale,
        );
        assert_eq!(
            compare_edition_profiles(&mirror_a, &mirror_b).kind,
            EditionMatchKind::Same
        );
    }

    #[test]
    fn equal_unknown_facets_share_a_container_without_becoming_a_wildcard() {
        let unknown = EditionProfile::default();
        assert!(edition_profiles_can_share_container(&unknown, &unknown));
        assert!(!edition_profiles_can_share_container(
            &profile(
                "zh-CN",
                "group-a",
                ScanGroupFacet::content_line("scan-a"),
                ColorMode::Grayscale,
            ),
            &unknown,
        ));

        let mirror_a = profile(
            "zh-CN",
            "group-a",
            ScanGroupFacet::mirror_label("mirror-a"),
            ColorMode::Unknown,
        );
        let mirror_b = profile(
            "zh-CN",
            "group-a",
            ScanGroupFacet::mirror_label("mirror-b"),
            ColorMode::Unknown,
        );
        assert!(edition_profiles_can_share_container(&mirror_a, &mirror_b));
        for scan_group in [
            ScanGroupFacet::Unknown,
            ScanGroupFacet::NotApplicable,
            ScanGroupFacet::content_line("content-line"),
        ] {
            let other = profile("zh-CN", "group-a", scan_group, ColorMode::Unknown);
            assert!(
                edition_profiles_can_share_container(&mirror_a, &other),
                "镜像标签不能单独拆分 Edition"
            );
        }
    }

    #[test]
    fn different_remote_ids_with_exact_page_identity_share_content() {
        let profile = profile(
            "zh-CN",
            "group-a",
            ScanGroupFacet::content_line("scan-a"),
            ColorMode::Grayscale,
        );
        let left = chapter_metadata(profile.clone());
        let right = chapter_metadata(profile);
        let left_pages = vec![
            PageIdentity::fingerprint("p1"),
            PageIdentity::fingerprint("p2"),
        ];
        let right_pages = vec![
            PageIdentity::fingerprint("p1"),
            PageIdentity::fingerprint("p2"),
        ];

        let result = compare_chapters(
            &source_chapter("chapter-a"),
            &left,
            &left_pages,
            &source_chapter("chapter-b"),
            &right,
            &right_pages,
        );
        assert_eq!(result.kind, ChapterMatchKind::SameContent);
        assert_eq!(result.progress_migration, ProgressMigrationMode::Shared);
        assert_eq!(result.confidence, MatchConfidence::High);
    }

    #[test]
    fn stable_page_names_are_not_cross_media_content_evidence() {
        let left = chapter_metadata(EditionProfile::default());
        let right = chapter_metadata(EditionProfile::default());
        let pages = vec![PageIdentity::stable("001.jpg")];

        let result = compare_chapters(
            &source_chapter("chapter-a"),
            &left,
            &pages,
            &source_chapter("chapter-b"),
            &right,
            &pages,
        );

        assert_eq!(result.kind, ChapterMatchKind::Candidate);
        assert_eq!(result.confidence, MatchConfidence::Low);
        assert_eq!(result.progress_migration, ProgressMigrationMode::Suggested);
    }

    #[test]
    fn stable_page_names_can_match_within_one_media_item() {
        let left = chapter_metadata(EditionProfile::default());
        let right = chapter_metadata(EditionProfile::default());
        let pages = vec![PageIdentity::stable("001.jpg")];

        let result = compare_chapters_within_media_item(
            &source_chapter("chapter-a"),
            &left,
            &pages,
            &source_chapter("chapter-b"),
            &right,
            &pages,
        );

        assert_eq!(result.kind, ChapterMatchKind::SameContent);
        assert_eq!(result.confidence, MatchConfidence::High);
        assert_eq!(result.progress_migration, ProgressMigrationMode::Shared);
    }

    #[test]
    fn stable_and_fingerprint_conflicts_are_not_page_matches() {
        let old = vec![PageIdentity::stable("001.jpg").with_fingerprint("old")];
        let new = vec![PageIdentity::stable("001.jpg").with_fingerprint("new")];

        let migrated = migrate_page_index(&old, &new, 0);
        assert_eq!(migrated.strategy, PageMappingStrategy::ProportionalFallback);
        assert_eq!(migrated.confidence, PageMappingConfidence::Low);

        let result = compare_chapters_within_media_item(
            &source_chapter("chapter-a"),
            &ComicChapterMetadata::default(),
            &old,
            &source_chapter("chapter-b"),
            &ComicChapterMetadata::default(),
            &new,
        );
        assert_eq!(result.kind, ChapterMatchKind::Candidate);
        assert_ne!(result.progress_migration, ProgressMigrationMode::Shared);
    }

    #[test]
    fn conflicting_authoritative_content_keys_require_best_effort() {
        let left = ComicChapterMetadata {
            authoritative_content_key: Some("content-a".into()),
            ..Default::default()
        };
        let right = ComicChapterMetadata {
            authoritative_content_key: Some("content-b".into()),
            ..Default::default()
        };
        let pages = vec![PageIdentity::fingerprint("same-page")];

        let result = compare_chapters(
            &source_chapter("chapter-a"),
            &left,
            &pages,
            &source_chapter("chapter-b"),
            &right,
            &pages,
        );

        assert_eq!(result.kind, ChapterMatchKind::Candidate);
        assert_eq!(result.confidence, MatchConfidence::Low);
        assert_eq!(result.progress_migration, ProgressMigrationMode::Suggested);
        assert!(
            result
                .evidence
                .contains(&ChapterEvidence::ConflictingAuthoritativeContentKey)
        );
    }

    #[test]
    fn same_metadata_without_page_evidence_is_only_a_candidate() {
        let profile = profile(
            "zh-CN",
            "group-a",
            ScanGroupFacet::content_line("scan-a"),
            ColorMode::Grayscale,
        );
        let left = chapter_metadata(profile.clone());
        let right = chapter_metadata(profile);
        let result = compare_chapters(
            &source_chapter("chapter-a"),
            &left,
            &[],
            &source_chapter("chapter-b"),
            &right,
            &[],
        );
        assert_eq!(result.kind, ChapterMatchKind::Candidate);
        assert_eq!(result.progress_migration, ProgressMigrationMode::Suggested);
        assert_eq!(result.confidence, MatchConfidence::Low);
    }

    #[test]
    fn page_mapping_follows_stable_identity_after_insert_and_reorder() {
        let old = vec![
            PageIdentity::stable("a"),
            PageIdentity::stable("b"),
            PageIdentity::stable("c"),
        ];
        let inserted = vec![
            PageIdentity::stable("intro"),
            PageIdentity::stable("a"),
            PageIdentity::stable("b"),
            PageIdentity::stable("c"),
        ];
        let migrated = migrate_page_index(&old, &inserted, 1);
        assert_eq!(migrated.target_page_index, Some(2));
        assert_eq!(migrated.strategy, PageMappingStrategy::StableKey);
        assert_eq!(migrated.confidence, PageMappingConfidence::High);

        let reordered = vec![
            PageIdentity::stable("c"),
            PageIdentity::stable("a"),
            PageIdentity::stable("b"),
        ];
        let migrated = migrate_page_index(&old, &reordered, 0);
        assert_eq!(migrated.target_page_index, Some(1));
        assert_eq!(migrated.strategy, PageMappingStrategy::StableKey);
    }

    #[test]
    fn deleted_current_page_uses_nearest_surviving_page() {
        let old = vec![
            PageIdentity::stable("a"),
            PageIdentity::stable("deleted"),
            PageIdentity::stable("c"),
        ];
        let new = vec![PageIdentity::stable("a"), PageIdentity::stable("c")];
        let migrated = migrate_page_index(&old, &new, 1);
        assert_eq!(migrated.target_page_index, Some(1));
        assert_eq!(migrated.strategy, PageMappingStrategy::NearestSurvivingPage);
        assert_eq!(migrated.confidence, PageMappingConfidence::Medium);
        assert!(migrated.reversible);
    }

    #[test]
    fn missing_page_identity_uses_reversible_low_confidence_fallback() {
        let old = vec![
            PageIdentity::default(),
            PageIdentity::default(),
            PageIdentity::default(),
        ];
        let new = vec![PageIdentity::default(), PageIdentity::default()];
        let migrated = migrate_page_index(&old, &new, 2);
        assert_eq!(migrated.target_page_index, Some(1));
        assert_eq!(migrated.strategy, PageMappingStrategy::ProportionalFallback);
        assert_eq!(migrated.confidence, PageMappingConfidence::Low);
        assert!(migrated.reversible);
    }

    #[test]
    fn opaque_source_identity_trims_but_does_not_rewrite_remote_ids() {
        let identity = ChapterSourceIdentity::new(" mangadex ", " Work-1 ", " CH-01 ").unwrap();
        assert_eq!(identity.source_key, "mangadex");
        assert_eq!(identity.remote_work_id, "Work-1");
        assert_eq!(identity.remote_chapter_id, "CH-01");
        assert!(ChapterSourceIdentity::new("", "work", "chapter").is_none());
        assert!(ChapterSourceIdentity::new("mangadex\n", "work", "chapter").is_none());
        assert!(ChapterSourceIdentity::new("mangadex", "work", "chapter\u{7f}").is_none());
    }

    #[test]
    fn page_matcher_does_not_panic_on_duplicate_fingerprints() {
        let left = vec![
            PageIdentity::fingerprint("same"),
            PageIdentity::fingerprint("same"),
        ];
        let right = vec![
            PageIdentity::fingerprint("same"),
            PageIdentity::fingerprint("same"),
        ];
        let result = compare_chapters(
            &source_chapter("a"),
            &ComicChapterMetadata::default(),
            &left,
            &source_chapter("b"),
            &ComicChapterMetadata::default(),
            &right,
        );
        assert_eq!(result.kind, ChapterMatchKind::SameContent);
    }
}
