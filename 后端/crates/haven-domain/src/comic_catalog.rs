//! 漫画章节目录的领域只读模型。
//!
//! 目录是来源在某个时间点观察到的章节集合，不是来源 URL、请求授权或
//! 阅读器页面会话。每个条目的来源身份仍然使用
//! `(source_key, remote_work_id, remote_chapter_id)`，这样目录刷新可以更新
//! 同一个来源章节，而不会把排序位置误当成章节主键。
//!
//! 本模块同时承载 Work 级只读聚合（[`ComicWorkChapterCatalog`]）：它把多个
//! 来源作品、多个 Edition 和多个 MediaItem 折叠成后端唯一确定的章节数组。
//! 前端只消费最终数组顺序和 `backend_order`；`sort_key` 只是 Domain 内部的
//! 排序辅助值，不是 Wire 字段承诺。

use std::cmp::Ordering;
use std::collections::{BTreeMap, HashSet};

use serde::{Deserialize, Serialize};

use haven_common::UtcMillis;

use crate::comic_identity::{
    ChapterMatch, ChapterMatchKind, ChapterSourceIdentity, ChapterSourceRef, ComicChapterMetadata,
    EditionProfile, PageIdentity, ScanGroupFacet, compare_chapters,
    edition_profiles_can_share_container,
};
use crate::entities::{MediaIndex, MediaItem, Progress, Resource, ResourceLocator};
use crate::enums::{Availability, ResourceType};
use crate::ids::{ComicCatalogRefreshId, ComicProgressSubjectId, EditionId, MediaItemId, WorkId};

/// 漫画目录刷新的一次持久化结果。
///
/// 这里只保存来源的 opaque identity、刷新代际和有界观察范围；URL、Cookie、
/// grant、请求头、本地路径等运行时授权/通道信息永不进入领域模型。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ComicCatalogRefreshOutcomeStatus {
    NeverSynced,
    Succeeded,
    TemporarilyUnavailable,
    ExternalOnly,
    Unknown,
    RefreshFailed,
    Truncated,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct ComicCatalogRefreshReceipt {
    pub id: ComicCatalogRefreshId,
    pub work_id: WorkId,
    pub source_key: String,
    pub remote_work_id: String,
    pub status: ComicCatalogRefreshOutcomeStatus,
    pub generation_before: u64,
    pub generation_after: Option<u64>,
    pub observed_from: Option<String>,
    pub observed_to: Option<String>,
    pub truncated: bool,
    pub retained_previous_catalog: bool,
    pub error_code: Option<String>,
    pub observed_at: UtcMillis,
}

/// Provider 对章节可消费性的观察结果。
///
/// `TemporarilyUnavailable` 和 `ExternalOnly` 都保留在目录中，不能在解析层
/// 直接丢弃；刷新层需要据此把已有章节标记为暂不可用，同时保留其来源身份
/// 和用户进度。`Unknown` 表示来源没有给出足够的页数/可读性信息，最终读取
/// 时仍需由 provider 再次确认。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ComicChapterAvailability {
    Available,
    TemporarilyUnavailable,
    ExternalOnly,
    Unknown,
}

/// 持久化的章节来源状态。
///
/// `Missing` 只表示一次完整目录观察中没有再次出现该章节；它不表示
/// MediaItem、Progress、Marker 或 History 应被删除。目录不完整或刷新失败时，
/// 刷新用例不会把已有记录改成 `Missing`。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ComicChapterSourceStatus {
    Available,
    TemporarilyUnavailable,
    ExternalOnly,
    Unknown,
    Missing,
}

impl From<ComicChapterAvailability> for ComicChapterSourceStatus {
    fn from(value: ComicChapterAvailability) -> Self {
        match value {
            ComicChapterAvailability::Available => Self::Available,
            ComicChapterAvailability::TemporarilyUnavailable => Self::TemporarilyUnavailable,
            ComicChapterAvailability::ExternalOnly => Self::ExternalOnly,
            ComicChapterAvailability::Unknown => Self::Unknown,
        }
    }
}

/// 某来源作品的一次成功目录刷新状态。
///
/// `generation` 是不透明给前端的乐观并发版本。刷新请求在网络读取期间可以并发，
/// 但落库时必须以读取到的 generation 做 CAS，旧响应不能覆盖新目录。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct ComicChapterCatalogState {
    pub source_key: String,
    pub remote_work_id: String,
    pub generation: u64,
    pub fetched_at: UtcMillis,
    pub total: Option<u32>,
    pub truncated: bool,
}

impl ComicChapterAvailability {
    /// 已由来源明确报告页数并可进入已确认可读列表。
    pub fn is_confirmed_available(self) -> bool {
        matches!(self, Self::Available)
    }

    /// 可以交给后端页面 manifest 探测的章节。Unknown 只表示待探测，
    /// 不能直接投影成 UI 的“已确认可读”。
    pub fn is_probeable(self) -> bool {
        matches!(self, Self::Available | Self::Unknown)
    }

    /// 兼容旧调用方；“readable”现在严格表示已确认可读。
    pub fn is_readable(self) -> bool {
        self.is_confirmed_available()
    }
}

/// 来源目录中的一条章节观察。
///
/// `published_at`/`updated_at` 是 provider 清洗后的展示元数据，不参与主键
/// 和身份合并。页面 URL、pageId、grant、请求头和本地路径不属于该模型。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct ComicChapterCatalogEntry {
    pub identity: ChapterSourceIdentity,
    pub metadata: ComicChapterMetadata,
    pub availability: ComicChapterAvailability,
    pub published_at: Option<String>,
    pub updated_at: Option<String>,
}

/// 某来源对某一远端作品的一次有界目录观察。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct ComicChapterCatalog {
    pub source_key: String,
    pub remote_work_id: String,
    pub chapters: Vec<ComicChapterCatalogEntry>,
    pub fetched_at: UtcMillis,
    /// 来源报告的章节总数；缺失时为 `None`。
    pub total: Option<u32>,
    /// 因本地安全上限或来源分页信息不足而不能证明拿到全量目录。
    pub truncated: bool,
}

impl ComicChapterCatalog {
    /// 构造目录并检查每条章节确实属于这个来源作品。
    pub fn new(
        source_key: impl AsRef<str>,
        remote_work_id: impl AsRef<str>,
        chapters: Vec<ComicChapterCatalogEntry>,
        fetched_at: UtcMillis,
    ) -> Option<Self> {
        let total = u32::try_from(chapters.len()).ok();
        Self::new_with_coverage(
            source_key,
            remote_work_id,
            chapters,
            fetched_at,
            total,
            false,
        )
    }

    pub fn new_with_coverage(
        source_key: impl AsRef<str>,
        remote_work_id: impl AsRef<str>,
        chapters: Vec<ComicChapterCatalogEntry>,
        fetched_at: UtcMillis,
        total: Option<u32>,
        truncated: bool,
    ) -> Option<Self> {
        let source_key = source_key.as_ref().trim();
        let remote_work_id = remote_work_id.as_ref().trim();
        if source_key.is_empty() || remote_work_id.is_empty() {
            return None;
        }
        if chapters.iter().any(|chapter| {
            chapter.identity.source_key != source_key
                || chapter.identity.remote_work_id != remote_work_id
        }) {
            return None;
        }
        let mut identities = HashSet::with_capacity(chapters.len());
        if chapters
            .iter()
            .any(|chapter| !identities.insert(&chapter.identity))
        {
            return None;
        }
        Some(Self {
            source_key: source_key.to_owned(),
            remote_work_id: remote_work_id.to_owned(),
            chapters,
            fetched_at,
            total,
            truncated,
        })
    }

    pub fn readable_chapters(&self) -> impl Iterator<Item = &ComicChapterCatalogEntry> {
        self.chapters
            .iter()
            .filter(|chapter| chapter.availability.is_readable())
    }

    /// 返回可由后端进一步探测的章节；调用方在获得真实 manifest 后，
    /// 才能把 Unknown 更新为 Available。
    pub fn probeable_chapters(&self) -> impl Iterator<Item = &ComicChapterCatalogEntry> {
        self.chapters
            .iter()
            .filter(|chapter| chapter.availability.is_probeable())
    }
}

// ---------------------------------------------------------------------------
// Work 级只读聚合
// ---------------------------------------------------------------------------

/// Work 级漫画章节目录的刷新状态（聚合根）。
///
/// `RefreshFailed`/`Truncated` 只描述聚合根最近一次观察的覆盖度，不能把仍然
/// 有效的章节改写成 `Missing`。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ComicWorkCatalogStatus {
    NeverSynced,
    Synced,
    RefreshFailed,
    Truncated,
}

/// 聚合后单个漫画章节的可消费状态。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ComicChapterAggregateStatus {
    Available,
    TemporarilyUnavailable,
    ExternalOnly,
    Unknown,
    Missing,
}

/// 聚合章节的一条来源摘要。
///
/// 同一 MediaItem 上的多条 `ChapterSourceRef` 天然聚合为同一个章节，因此一个
/// 章节可以有多条来源摘要。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct ComicChapterSourceSummary {
    pub identity: ChapterSourceIdentity,
    pub status: ComicChapterSourceStatus,
    /// 只保留来源展示用的镜像标签；镜像标签不参与 Edition 划分。
    pub mirror_label: Option<String>,
    /// 最近一次观察到该来源章节的时间。
    pub observed_at: Option<UtcMillis>,
    pub source_order: u32,
}

/// 章节排序辅助键。
///
/// 它只是 Domain 内部的确定性排序输入，不是 Wire 字段承诺；前端不得据此自己
/// 推导顺序或上一章/下一章。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct ComicChapterOrderKey {
    pub volume_number: Option<f64>,
    pub chapter_number: Option<f64>,
    pub provider_rank: u16,
    pub source_order: u32,
    pub published_at: Option<String>,
    pub local_media_item_key: String,
}

impl ComicChapterOrderKey {
    /// 依次比较卷号、章节号、provider 序号、来源序号、发布时间和本地稳定 key。
    ///
    /// 缺失卷号/章节号的章节排在已编号项之后，因此番外落在末尾，并继续由来源
    /// 顺序和本地 MediaItem key 保持稳定。
    pub fn compare_for_catalog(&self, other: &Self) -> Ordering {
        option_f64_ascending(self.volume_number, other.volume_number)
            .then_with(|| option_f64_ascending(self.chapter_number, other.chapter_number))
            .then_with(|| self.provider_rank.cmp(&other.provider_rank))
            .then_with(|| self.source_order.cmp(&other.source_order))
            .then_with(|| {
                option_str_ascending(self.published_at.as_deref(), other.published_at.as_deref())
            })
            .then_with(|| self.local_media_item_key.cmp(&other.local_media_item_key))
    }
}

fn option_f64_ascending(left: Option<f64>, right: Option<f64>) -> Ordering {
    match (left, right) {
        (Some(left), Some(right)) => left.partial_cmp(&right).unwrap_or(Ordering::Equal),
        (Some(_), None) => Ordering::Less,
        (None, Some(_)) => Ordering::Greater,
        (None, None) => Ordering::Equal,
    }
}

fn option_str_ascending(left: Option<&str>, right: Option<&str>) -> Ordering {
    match (left, right) {
        (Some(left), Some(right)) => left.cmp(right),
        (Some(_), None) => Ordering::Less,
        (None, Some(_)) => Ordering::Greater,
        (None, None) => Ordering::Equal,
    }
}

/// 一个聚合后的漫画章节。
///
/// `media_item_id` 始终指向真实存在的本地 MediaItem；跨 MediaItem 只有同一来源
/// 身份或内容证据（`SameContent`/`SameLogicalChapterVariant`）成立时才会归并到
/// 同一个章节。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct ComicWorkChapter {
    pub media_item_id: MediaItemId,
    pub edition_id: EditionId,
    pub subject_id: Option<ComicProgressSubjectId>,
    pub chapter_number: Option<f64>,
    pub volume_number: Option<f64>,
    pub title: Option<String>,
    pub published_at: Option<String>,
    pub page_count: Option<u32>,
    pub status: ComicChapterAggregateStatus,
    pub can_open: bool,
    pub sources: Vec<ComicChapterSourceSummary>,
    pub match_result: Option<ChapterMatch>,
    pub progress: Option<Progress>,
    pub sort_key: ComicChapterOrderKey,
    pub backend_order: u32,
    pub previous_media_item_id: Option<MediaItemId>,
    pub next_media_item_id: Option<MediaItemId>,
}

/// Work 级漫画章节只读聚合。
///
/// 这是前端唯一可信的顺序事实：`chapters` 的数组顺序和每项的 `backend_order`
/// 由后端确定，上一章/下一章只在同一个 Edition 的相邻项之间链接。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct ComicWorkChapterCatalog {
    pub work_id: WorkId,
    pub current_media_item_id: Option<MediaItemId>,
    pub editions: Vec<(EditionId, EditionProfile)>,
    pub chapters: Vec<ComicWorkChapter>,
    pub refresh_status: ComicWorkCatalogStatus,
    pub last_observed_at: Option<UtcMillis>,
    pub truncated: bool,
    /// 每个来源作品的最新 Receipt（见 [`latest_receipts_by_source`]）；历史
    /// Receipt 仍留在存储中，但既不回放也不参与状态推导。
    pub refresh_receipts: Vec<ComicCatalogRefreshReceipt>,
}

/// 判定“本地是否可打开”所需的最小资源事实。
///
/// 它是聚合输入而不是聚合结果的一部分：`Resource` 的路径、URL、指纹和时间戳
/// 不会复制进章节模型。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct ComicResourceAvailabilityFact {
    pub resource_type: ResourceType,
    pub availability: Availability,
    /// locator 是否指向本地文件；远端/存储对象不算本地可打开。
    pub locator_is_local: bool,
}

impl ComicResourceAvailabilityFact {
    pub fn from_resource(resource: &Resource) -> Self {
        Self {
            resource_type: resource.resource_type,
            availability: resource.availability,
            locator_is_local: matches!(resource.locator, ResourceLocator::LocalPath { .. }),
        }
    }
}

/// 只有“可用或离线可用的本地内容型资源”才代表章节真的可以打开。
///
/// 远端类型（HttpFile/RemoteChapter/…）即使状态是 `Available` 也不能算本地
/// 可打开；`TemporarilyUnavailable`/`Missing`/`Unknown` 一律不算。
pub fn resource_fact_allows_local_open(fact: &ComicResourceAvailabilityFact) -> bool {
    matches!(
        fact.availability,
        Availability::Available | Availability::OfflineAvailable
    ) && fact.locator_is_local
        && matches!(
            fact.resource_type,
            ResourceType::LocalFile
                | ResourceType::PublicationFile
                | ResourceType::ComicArchive
                | ResourceType::ImageSequence
        )
}

/// 聚合输入中的一条来源章节观察。
#[derive(Debug, Clone, PartialEq)]
pub struct ComicChapterAggregateSource {
    pub reference: ChapterSourceRef,
    /// 来源声明的展示序号；Haven 尚未持久化该字段时为 0。
    pub provider_rank: u16,
    /// 该来源章节的页面身份；为空时元数据候选不得升级为 `SameContent`。
    pub pages: Vec<PageIdentity>,
}

/// 一个 MediaItem 在一次 Work 级聚合中的只读观察。
#[derive(Debug, Clone, PartialEq)]
pub struct ComicWorkChapterObservation {
    pub media_item: MediaItem,
    pub edition_id: EditionId,
    pub edition_profile: EditionProfile,
    pub sources: Vec<ComicChapterAggregateSource>,
    pub resource_facts: Vec<ComicResourceAvailabilityFact>,
    /// 既有的 Progress 原行；聚合只投影，不改写。
    pub progress: Option<Progress>,
    /// 当前 active Subject 归属；候选成员不是 active 归属，必须为 `None`。
    pub subject_id: Option<ComicProgressSubjectId>,
}

/// Work 级聚合的纯输入。
#[derive(Debug, Clone, PartialEq)]
pub struct ComicWorkChapterAggregateInput {
    pub work_id: WorkId,
    pub observations: Vec<ComicWorkChapterObservation>,
    pub current_media_item_id: Option<MediaItemId>,
    pub refresh_status: ComicWorkCatalogStatus,
    pub last_observed_at: Option<UtcMillis>,
    pub truncated: bool,
    pub refresh_receipts: Vec<ComicCatalogRefreshReceipt>,
}

/// Work 级刷新 Receipt 的聚合摘要。
///
/// `latest_receipts` 是每个来源作品（`source_key` + `remote_work_id`）的最新一条
/// Receipt；`status`/`truncated`/`last_observed_at` 全部只用这份最新观察推导，
/// 因此历史旧失败或旧截断不会永久遮蔽该来源后来的成功。
#[derive(Debug, Clone, PartialEq)]
pub struct ComicWorkRefreshSummary {
    pub status: ComicWorkCatalogStatus,
    pub last_observed_at: Option<UtcMillis>,
    pub truncated: bool,
    pub latest_receipts: Vec<ComicCatalogRefreshReceipt>,
}

/// 每个来源作品只保留最新 Receipt：先比 `observed_at`，再比 `id` 作稳定 tie-breaker。
///
/// 返回顺序按来源键升序，保证同一批 Receipt 的摘要结果可复现。
pub fn latest_receipts_by_source(
    receipts: &[ComicCatalogRefreshReceipt],
) -> Vec<ComicCatalogRefreshReceipt> {
    let mut latest: BTreeMap<(&str, &str), &ComicCatalogRefreshReceipt> = BTreeMap::new();
    for receipt in receipts {
        let key = (receipt.source_key.as_str(), receipt.remote_work_id.as_str());
        let is_newer = latest.get(&key).is_none_or(|current| {
            (receipt.observed_at, receipt.id) > (current.observed_at, current.id)
        });
        if is_newer {
            latest.insert(key, receipt);
        }
    }
    latest.into_values().cloned().collect()
}

/// 由每个来源的最新 Receipt 推导聚合根状态：失败优先于截断，截断优先于成功。
pub fn comic_work_catalog_status_from_receipts(
    receipts: &[ComicCatalogRefreshReceipt],
) -> ComicWorkCatalogStatus {
    catalog_status_from_latest(&latest_receipts_by_source(receipts))
}

/// 统一计算聚合根刷新状态、覆盖度和最近观察时间。
pub fn summarize_comic_work_refresh(
    receipts: &[ComicCatalogRefreshReceipt],
) -> ComicWorkRefreshSummary {
    let latest_receipts = latest_receipts_by_source(receipts);
    ComicWorkRefreshSummary {
        status: catalog_status_from_latest(&latest_receipts),
        last_observed_at: latest_receipts
            .iter()
            .map(|receipt| receipt.observed_at)
            .max(),
        truncated: latest_receipts.iter().any(|receipt| {
            receipt.truncated || receipt.status == ComicCatalogRefreshOutcomeStatus::Truncated
        }),
        latest_receipts,
    }
}

fn catalog_status_from_latest(
    latest_receipts: &[ComicCatalogRefreshReceipt],
) -> ComicWorkCatalogStatus {
    if latest_receipts.is_empty() {
        return ComicWorkCatalogStatus::NeverSynced;
    }
    if latest_receipts
        .iter()
        .any(|receipt| receipt.status == ComicCatalogRefreshOutcomeStatus::RefreshFailed)
    {
        return ComicWorkCatalogStatus::RefreshFailed;
    }
    if latest_receipts.iter().any(|receipt| {
        receipt.truncated || receipt.status == ComicCatalogRefreshOutcomeStatus::Truncated
    }) {
        return ComicWorkCatalogStatus::Truncated;
    }
    ComicWorkCatalogStatus::Synced
}

struct EditionBucket {
    edition_id: EditionId,
    profile: EditionProfile,
    observations: Vec<ComicWorkChapterObservation>,
}

struct RawChapter {
    media_item: MediaItem,
    sources: Vec<ComicChapterAggregateSource>,
    resource_facts: Vec<ComicResourceAvailabilityFact>,
    progress: Option<Progress>,
    subject_id: Option<ComicProgressSubjectId>,
}

impl RawChapter {
    fn from_observation(observation: ComicWorkChapterObservation) -> Self {
        let mut sources = observation.sources;
        sources.sort_by(|left, right| source_order_key(left).cmp(&source_order_key(right)));
        Self {
            media_item: observation.media_item,
            sources,
            resource_facts: observation.resource_facts,
            progress: observation.progress,
            subject_id: observation.subject_id,
        }
    }
}

fn source_order_key(source: &ComicChapterAggregateSource) -> (u16, u32, &str, &str, &str) {
    (
        source.provider_rank,
        source.reference.source_order,
        source.reference.identity.source_key.as_str(),
        source.reference.identity.remote_work_id.as_str(),
        source.reference.identity.remote_chapter_id.as_str(),
    )
}

/// 把 Work 下的多个来源、多个 Edition 和多个 MediaItem 聚合为唯一确定的章节数组。
///
/// 分组顺序是先按 Edition（同一 `EditionId` 内再按画像容器边界拆分），再在
/// 每个 Edition 内按 [`ComicChapterOrderKey`] 排序；整个数组的零基位置写入
/// `backend_order`，上一章/下一章只在同一 Edition 块内相邻项之间链接。
pub fn aggregate_comic_work_chapters(
    input: ComicWorkChapterAggregateInput,
) -> ComicWorkChapterCatalog {
    let ComicWorkChapterAggregateInput {
        work_id,
        observations,
        current_media_item_id,
        refresh_status,
        last_observed_at,
        truncated,
        refresh_receipts,
    } = input;

    let mut buckets: Vec<EditionBucket> = Vec::new();
    for observation in observations {
        // 新观察只有在与桶内**所有**已有观察的画像都能共享容器时才入桶。
        // MirrorLabel 的单次比较是放宽规则，只对一次比较成立；若只和桶内首个
        // 画像比较，"MirrorLabel -> ContentLine(A) -> ContentLine(B)" 会把两个
        // 真实冲突的内容线并进同一个 Edition 容器。
        let compatible = buckets.iter_mut().find(|bucket| {
            bucket.edition_id == observation.edition_id
                && bucket.observations.iter().all(|existing| {
                    edition_profiles_can_share_container(
                        &existing.edition_profile,
                        &observation.edition_profile,
                    )
                })
        });
        match compatible {
            Some(bucket) => bucket.observations.push(observation),
            None => buckets.push(EditionBucket {
                edition_id: observation.edition_id,
                profile: observation.edition_profile.clone(),
                observations: vec![observation],
            }),
        }
    }

    let mut editions: Vec<(EditionId, EditionProfile)> = Vec::with_capacity(buckets.len());
    let mut chapters: Vec<ComicWorkChapter> = Vec::new();
    let mut blocks: Vec<(usize, usize)> = Vec::with_capacity(buckets.len());
    for bucket in buckets {
        editions.push((bucket.edition_id, bucket.profile.clone()));
        let start = chapters.len();
        chapters.extend(aggregate_edition_chapters(
            bucket.observations,
            bucket.edition_id,
            refresh_status,
            current_media_item_id,
            &refresh_receipts,
        ));
        blocks.push((start, chapters.len()));
    }

    let mut backend_order: u32 = 0;
    for (start, end) in blocks {
        let media_item_ids: Vec<MediaItemId> = chapters[start..end]
            .iter()
            .map(|chapter| chapter.media_item_id)
            .collect();
        for offset in 0..media_item_ids.len() {
            chapters[start + offset].backend_order = backend_order;
            backend_order += 1;
            chapters[start + offset].previous_media_item_id = offset
                .checked_sub(1)
                .map(|previous| media_item_ids[previous]);
            chapters[start + offset].next_media_item_id = media_item_ids.get(offset + 1).copied();
        }
    }

    ComicWorkChapterCatalog {
        work_id,
        current_media_item_id,
        editions,
        chapters,
        refresh_status,
        last_observed_at,
        truncated,
        refresh_receipts,
    }
}

fn aggregate_edition_chapters(
    observations: Vec<ComicWorkChapterObservation>,
    edition_id: EditionId,
    refresh_status: ComicWorkCatalogStatus,
    current_media_item_id: Option<MediaItemId>,
    refresh_receipts: &[ComicCatalogRefreshReceipt],
) -> Vec<ComicWorkChapter> {
    let mut raw: Vec<RawChapter> = observations
        .into_iter()
        .map(RawChapter::from_observation)
        .collect();
    raw.sort_by_key(|chapter| chapter.media_item.id);

    let groups = merge_groups(&raw);
    let mut chapters: Vec<ComicWorkChapter> = groups
        .iter()
        .map(|group| {
            build_merged_chapter(
                group,
                &raw,
                edition_id,
                refresh_status,
                current_media_item_id,
                refresh_receipts,
            )
        })
        .collect();

    // 不能归并的章节仍然保留 Candidate 证据：Candidate 只描述匹配证据，
    // 不改变 Edition、MediaItem 或 Subject 的 active 归属。
    for left in 0..groups.len() {
        for right in (left + 1)..groups.len() {
            let matched = strongest_group_match(&groups[left], &groups[right], &raw);
            if let Some(matched) =
                matched.filter(|matched| matched.kind == ChapterMatchKind::Candidate)
            {
                if chapters[left].match_result.is_none() {
                    chapters[left].match_result = Some(matched.clone());
                }
                if chapters[right].match_result.is_none() {
                    chapters[right].match_result = Some(matched);
                }
            }
        }
    }

    chapters.sort_by(|left, right| left.sort_key.compare_for_catalog(&right.sort_key));
    chapters
}

fn merge_groups(raw: &[RawChapter]) -> Vec<Vec<usize>> {
    let mut parent: Vec<usize> = (0..raw.len()).collect();
    for left in 0..raw.len() {
        for right in (left + 1)..raw.len() {
            if chapter_merge_match(&raw[left], &raw[right]).is_some() {
                union(&mut parent, left, right);
            }
        }
    }

    let mut roots: Vec<usize> = Vec::new();
    let mut groups: Vec<Vec<usize>> = Vec::new();
    for index in 0..raw.len() {
        let root = find(&mut parent, index);
        match roots.iter().position(|candidate| *candidate == root) {
            Some(position) => groups[position].push(index),
            None => {
                roots.push(root);
                groups.push(vec![index]);
            }
        }
    }
    groups
}

fn find(parent: &mut [usize], index: usize) -> usize {
    let mut current = index;
    while parent[current] != current {
        parent[current] = parent[parent[current]];
        current = parent[current];
    }
    current
}

fn union(parent: &mut [usize], left: usize, right: usize) {
    let left_root = find(parent, left);
    let right_root = find(parent, right);
    if left_root != right_root {
        parent[right_root] = left_root;
    }
}

fn chapter_merge_match(left: &RawChapter, right: &RawChapter) -> Option<ChapterMatch> {
    let best = strongest_chapter_match(left, right)?;
    is_mergeable_match_kind(best.kind).then_some(best)
}

fn is_mergeable_match_kind(kind: ChapterMatchKind) -> bool {
    matches!(
        kind,
        ChapterMatchKind::SameRemoteChapter
            | ChapterMatchKind::SameContent
            | ChapterMatchKind::SameLogicalChapterVariant
    )
}

/// 跨 MediaItem 的最强匹配证据。
///
/// 只有同一来源身份或内容证据（SameContent/同一 Edition 内的
/// SameLogicalChapterVariant）才允许归并来源；元数据相似最多产生 Candidate。
fn strongest_chapter_match(left: &RawChapter, right: &RawChapter) -> Option<ChapterMatch> {
    let mut best: Option<ChapterMatch> = None;
    for left_source in &left.sources {
        for right_source in &right.sources {
            let matched = compare_chapters(
                &left_source.reference.identity,
                &left_source.reference.metadata,
                &left_source.pages,
                &right_source.reference.identity,
                &right_source.reference.metadata,
                &right_source.pages,
            );
            if is_stronger(&best, &matched) {
                best = Some(matched);
            }
        }
    }
    best
}

fn strongest_group_match(
    left: &[usize],
    right: &[usize],
    raw: &[RawChapter],
) -> Option<ChapterMatch> {
    let mut best: Option<ChapterMatch> = None;
    for left_index in left {
        for right_index in right {
            let matched = strongest_chapter_match(&raw[*left_index], &raw[*right_index]);
            if let Some(candidate) = matched {
                if is_stronger(&best, &candidate) {
                    best = Some(candidate);
                }
            }
        }
    }
    best
}

fn is_stronger(current: &Option<ChapterMatch>, candidate: &ChapterMatch) -> bool {
    current
        .as_ref()
        .map(|existing| match_strength(candidate.kind) > match_strength(existing.kind))
        .unwrap_or(true)
}

fn match_strength(kind: ChapterMatchKind) -> u8 {
    match kind {
        ChapterMatchKind::SameRemoteChapter => 5,
        ChapterMatchKind::SameContent => 4,
        ChapterMatchKind::SameLogicalChapterVariant => 3,
        ChapterMatchKind::Candidate => 2,
        ChapterMatchKind::Unrelated => 1,
    }
}

fn chapter_has_projection(chapter: &RawChapter) -> bool {
    chapter.progress.is_some() || chapter.subject_id.is_some()
}

/// 代表成员优先级：本地可打开且有投影 → 本地可打开 → 有投影 → 兜底。
fn representative_priority(chapter: &RawChapter) -> u8 {
    match (
        has_local_readable_resource(&chapter.resource_facts),
        chapter_has_projection(chapter),
    ) {
        (true, true) => 0,
        (true, false) => 1,
        (false, true) => 2,
        (false, false) => 3,
    }
}

/// 归并组内的代表成员，决定 `ComicWorkChapter::media_item_id`。
///
/// 规则按优先级：
/// 1. `current_media_item_id` 命中的成员——它是当前读取上下文，也必然是本地真实
///    MediaItem，因此优先让它代表这一章，前端才能用返回的 `media_item_id` 定位
///    当前项；
/// 2. 有本地可打开资源且有 Progress/active Subject 投影的成员；
/// 3. 有本地可打开资源的成员；
/// 4. 有 Progress/active Subject 投影的成员；
/// 5. 其余成员按稳定 MediaItem ID 取最小。
fn select_representative<'a>(
    ordered: &[&'a RawChapter],
    current_media_item_id: Option<MediaItemId>,
) -> &'a RawChapter {
    if let Some(current) = current_media_item_id {
        if let Some(representative) = ordered
            .iter()
            .find(|chapter| chapter.media_item.id == current)
        {
            return representative;
        }
    }
    ordered
        .iter()
        .min_by_key(|chapter| (representative_priority(chapter), chapter.media_item.id))
        .copied()
        .expect("归并组至少有一个成员")
}

/// Progress/Subject 的唯一投影成员。
///
/// 代表成员自身有投影时直接取它的原值；否则（例如当前读取上下文命中了没有本行
/// 进度的一侧）回退到同一个投影成员，Progress 与 Subject 必须同源，不能让两个
/// 独立标准各自挑一个 MediaItem。回退只选择投影成员，返回的 Progress 始终是原
/// 行，locator/revision/last_active_at 一律不改写。
fn select_projection_member<'a>(
    ordered: &[&'a RawChapter],
    representative: &'a RawChapter,
) -> Option<&'a RawChapter> {
    if chapter_has_projection(representative) {
        return Some(representative);
    }
    ordered
        .iter()
        .copied()
        .filter(|chapter| chapter_has_projection(chapter))
        .min_by_key(|chapter| {
            (
                !has_local_readable_resource(&chapter.resource_facts),
                chapter.media_item.id,
            )
        })
}

fn build_merged_chapter(
    members: &[usize],
    raw: &[RawChapter],
    edition_id: EditionId,
    refresh_status: ComicWorkCatalogStatus,
    current_media_item_id: Option<MediaItemId>,
    refresh_receipts: &[ComicCatalogRefreshReceipt],
) -> ComicWorkChapter {
    let mut ordered: Vec<&RawChapter> = members.iter().map(|index| &raw[*index]).collect();
    ordered.sort_by_key(|chapter| chapter.media_item.id);
    let representative = select_representative(&ordered, current_media_item_id);

    let mut sources: Vec<ComicChapterAggregateSource> = Vec::new();
    for chapter in &ordered {
        for source in &chapter.sources {
            if !sources
                .iter()
                .any(|existing| existing.reference.identity == source.reference.identity)
            {
                sources.push(source.clone());
            }
        }
    }
    sources.sort_by(|left, right| source_order_key(left).cmp(&source_order_key(right)));

    let resource_facts: Vec<ComicResourceAvailabilityFact> = ordered
        .iter()
        .flat_map(|chapter| chapter.resource_facts.iter().copied())
        .collect();
    let projection_member = select_projection_member(&ordered, representative);
    let progress = projection_member.and_then(|chapter| chapter.progress.clone());
    let subject_id = projection_member.and_then(|chapter| chapter.subject_id);

    let mut match_result: Option<ChapterMatch> = None;
    for left in 0..ordered.len() {
        for right in (left + 1)..ordered.len() {
            if let Some(candidate) = strongest_chapter_match(ordered[left], ordered[right]) {
                if is_stronger(&match_result, &candidate) {
                    match_result = Some(candidate);
                }
            }
        }
    }

    let primary_source = sources.first();
    let (index_volume, index_chapter) = match representative.media_item.index {
        MediaIndex::Chapter { volume, chapter } => {
            (volume.map(f64::from), Some(f64::from(chapter)))
        }
        _ => (None, None),
    };
    let chapter_number = index_chapter
        .or_else(|| primary_source.and_then(|source| source.reference.metadata.chapter_number));
    let volume_number = index_volume
        .or_else(|| primary_source.and_then(|source| source.reference.metadata.volume_number));
    let published_at = primary_source
        .and_then(|source| source.reference.published_at.clone())
        .or_else(|| representative.media_item.published_at.clone());
    let page_count = primary_source
        .and_then(|source| source.reference.metadata.page_count)
        .or(representative.media_item.page_count);
    let title = primary_source
        .and_then(|source| source.reference.metadata.title.clone())
        .or_else(|| {
            (!representative.media_item.title.trim().is_empty())
                .then(|| representative.media_item.title.clone())
        });

    let (status, can_open) =
        chapter_aggregate_status(&sources, &resource_facts, refresh_status, refresh_receipts);
    let sort_key = ComicChapterOrderKey {
        volume_number,
        chapter_number,
        provider_rank: primary_source
            .map(|source| source.provider_rank)
            .unwrap_or(0),
        source_order: sources
            .iter()
            .map(|source| source.reference.source_order)
            .min()
            .unwrap_or(0),
        published_at: published_at.clone(),
        local_media_item_key: representative.media_item.id.to_string(),
    };

    ComicWorkChapter {
        media_item_id: representative.media_item.id,
        edition_id,
        subject_id,
        chapter_number,
        volume_number,
        title,
        published_at,
        page_count,
        status,
        can_open,
        sources: sources.iter().map(source_summary).collect(),
        match_result,
        progress,
        sort_key,
        backend_order: 0,
        previous_media_item_id: None,
        next_media_item_id: None,
    }
}

fn source_summary(source: &ComicChapterAggregateSource) -> ComicChapterSourceSummary {
    ComicChapterSourceSummary {
        identity: source.reference.identity.clone(),
        status: source.reference.availability,
        mirror_label: match &source.reference.metadata.edition_profile.scan_group {
            ScanGroupFacet::MirrorLabel(value) => Some(value.clone()),
            _ => None,
        },
        observed_at: Some(source.reference.updated_at),
        source_order: source.reference.source_order,
    }
}

fn has_local_readable_resource(facts: &[ComicResourceAvailabilityFact]) -> bool {
    facts.iter().any(resource_fact_allows_local_open)
}

fn chapter_aggregate_status(
    sources: &[ComicChapterAggregateSource],
    resource_facts: &[ComicResourceAvailabilityFact],
    refresh_status: ComicWorkCatalogStatus,
    refresh_receipts: &[ComicCatalogRefreshReceipt],
) -> (ComicChapterAggregateStatus, bool) {
    if has_local_readable_resource(resource_facts) {
        return (ComicChapterAggregateStatus::Available, true);
    }
    if sources.is_empty() {
        return (ComicChapterAggregateStatus::Unknown, false);
    }
    let all = |status: ComicChapterSourceStatus| {
        sources
            .iter()
            .all(|source| source.reference.availability == status)
    };
    if all(ComicChapterSourceStatus::ExternalOnly) {
        return (ComicChapterAggregateStatus::ExternalOnly, false);
    }
    if all(ComicChapterSourceStatus::TemporarilyUnavailable) {
        return (ComicChapterAggregateStatus::TemporarilyUnavailable, false);
    }
    // 刷新失败或被截断时不得把章节解释成 Missing：覆盖度不足只能停在 Unknown。
    if all(ComicChapterSourceStatus::Missing)
        && sources
            .iter()
            .all(|source| source_missing_is_confirmed(source, refresh_status, refresh_receipts))
    {
        return (ComicChapterAggregateStatus::Missing, false);
    }
    (ComicChapterAggregateStatus::Unknown, false)
}

/// A Work-level failure must not invalidate a complete observation made by a
/// different source.  Match Missing eligibility to the source/work identity
/// that produced the chapter instead of using the aggregate Work status as a
/// global gate.  Empty receipt input retains the legacy state-only fallback
/// used by readers assembled without the optional Receipt repository.
fn source_missing_is_confirmed(
    source: &ComicChapterAggregateSource,
    refresh_status: ComicWorkCatalogStatus,
    refresh_receipts: &[ComicCatalogRefreshReceipt],
) -> bool {
    let identity = &source.reference.identity;
    let latest_receipt = refresh_receipts
        .iter()
        .filter(|receipt| {
            receipt.source_key == identity.source_key
                && receipt.remote_work_id == identity.remote_work_id
        })
        .max_by_key(|receipt| (receipt.observed_at, receipt.id));

    match latest_receipt {
        Some(receipt) => {
            !receipt.truncated
                && !matches!(
                    receipt.status,
                    ComicCatalogRefreshOutcomeStatus::NeverSynced
                        | ComicCatalogRefreshOutcomeStatus::RefreshFailed
                        | ComicCatalogRefreshOutcomeStatus::Truncated
                )
        }
        None => matches!(
            refresh_status,
            ComicWorkCatalogStatus::NeverSynced | ComicWorkCatalogStatus::Synced
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::comic_identity::{
        ChapterMatchKind, ChapterSourceIdentity, ChapterSourceRef, ColorMode, ComicChapterMetadata,
        EditionProfile, IdentityFacet, PageIdentity, ScanGroupFacet,
    };
    use crate::entities::{MediaIndex, MediaItem, Progress};
    use crate::enums::{Availability, CompletionState, MediaItemStatus, MediaType, ResourceType};
    use crate::ids::{ComicProgressSubjectId, EditionId, MediaItemId, ProgressId, WorkId};
    use crate::locator::{ComicLocator, Locator};

    fn entry(chapter_id: &str) -> ComicChapterCatalogEntry {
        ComicChapterCatalogEntry {
            identity: ChapterSourceIdentity::new("mangadex", "manga-1", chapter_id).unwrap(),
            metadata: ComicChapterMetadata {
                edition_profile: EditionProfile::default(),
                chapter_number: Some(1.0),
                volume_number: None,
                title: Some("第一话".to_owned()),
                page_count: Some(12),
                authoritative_content_key: None,
            },
            availability: ComicChapterAvailability::Available,
            published_at: None,
            updated_at: None,
        }
    }

    #[test]
    fn catalog_requires_consistent_source_identity() {
        let wrong = ComicChapterCatalogEntry {
            identity: ChapterSourceIdentity::new("other", "manga-1", "chapter-2").unwrap(),
            ..entry("chapter-2")
        };
        assert!(
            ComicChapterCatalog::new("mangadex", "manga-1", vec![wrong], UtcMillis(1),).is_none()
        );
    }

    #[test]
    fn catalog_rejects_duplicate_source_identities_and_preserves_coverage() {
        let duplicate = vec![entry("chapter-1"), entry("chapter-1")];
        assert!(
            ComicChapterCatalog::new("mangadex", "manga-1", duplicate, UtcMillis(1),).is_none()
        );

        let catalog = ComicChapterCatalog::new_with_coverage(
            "mangadex",
            "manga-1",
            vec![entry("chapter-1")],
            UtcMillis(1),
            Some(12),
            true,
        )
        .unwrap();
        assert_eq!(catalog.total, Some(12));
        assert!(catalog.truncated);
    }

    #[test]
    fn unavailable_entries_stay_in_catalog_but_readable_view_filters_them() {
        let mut unavailable = entry("chapter-2");
        unavailable.availability = ComicChapterAvailability::TemporarilyUnavailable;
        let catalog = ComicChapterCatalog::new(
            "mangadex",
            "manga-1",
            vec![entry("chapter-1"), unavailable],
            UtcMillis(1),
        )
        .unwrap();
        assert_eq!(catalog.chapters.len(), 2);
        assert_eq!(catalog.readable_chapters().count(), 1);
    }

    #[test]
    fn unknown_entries_are_probeable_but_not_confirmed_readable() {
        let mut unknown = entry("chapter-2");
        unknown.availability = ComicChapterAvailability::Unknown;
        let catalog = ComicChapterCatalog::new(
            "mangadex",
            "manga-1",
            vec![entry("chapter-1"), unknown],
            UtcMillis(1),
        )
        .unwrap();
        assert_eq!(catalog.readable_chapters().count(), 1);
        assert_eq!(catalog.probeable_chapters().count(), 2);
        assert!(!ComicChapterAvailability::Unknown.is_readable());
        assert!(ComicChapterAvailability::Unknown.is_probeable());
    }

    #[test]
    fn refresh_receipt_roundtrips_snake_case_without_runtime_channels() {
        let receipt = ComicCatalogRefreshReceipt {
            id: ComicCatalogRefreshId::new(),
            work_id: WorkId::new(),
            source_key: "mangadex".to_owned(),
            remote_work_id: "remote-work-42".to_owned(),
            status: ComicCatalogRefreshOutcomeStatus::RefreshFailed,
            generation_before: 7,
            generation_after: Some(8),
            observed_from: Some("chapter-1".to_owned()),
            observed_to: Some("chapter-12".to_owned()),
            truncated: true,
            retained_previous_catalog: true,
            error_code: Some("temporarily_unavailable".to_owned()),
            observed_at: UtcMillis(1_234),
        };

        let value = serde_json::to_value(&receipt).unwrap();
        assert_eq!(value["status"], "refresh_failed");
        let restored: ComicCatalogRefreshReceipt = serde_json::from_value(value.clone()).unwrap();
        assert_eq!(restored, receipt);

        let object = value.as_object().unwrap();
        for forbidden_field in [
            "url",
            "cookie",
            "grant",
            "request_headers",
            "headers",
            "local_path",
            "path",
        ] {
            assert!(
                !object.contains_key(forbidden_field),
                "Receipt 不得序列化运行时通道字段 {forbidden_field}"
            );
        }
        let serialized = serde_json::to_string(&value).unwrap();
        assert!(!serialized.contains("://"));
        assert!(!serialized.to_ascii_lowercase().contains("cookie"));
        assert!(!serialized.to_ascii_lowercase().contains("grant"));
    }

    // ---- Work 级聚合 fixture -------------------------------------------------

    fn catalog_profile(
        language: &str,
        translation_line: &str,
        scan_group: ScanGroupFacet,
        color_mode: ColorMode,
    ) -> EditionProfile {
        EditionProfile {
            language: IdentityFacet::known(language),
            translation_line: IdentityFacet::known(translation_line),
            scan_group,
            color_mode,
        }
    }

    fn work_media_item(edition_id: EditionId, id: MediaItemId, chapter: Option<f64>) -> MediaItem {
        MediaItem {
            id,
            edition_id,
            parent_id: None,
            media_type: MediaType::Comic,
            title: chapter
                .map(|chapter| format!("第 {chapter} 话"))
                .unwrap_or_else(|| "番外".to_owned()),
            index: match chapter {
                Some(chapter) => MediaIndex::Chapter {
                    volume: Some(1.0),
                    chapter: chapter as f32,
                },
                None => MediaIndex::Custom {
                    label: "番外".to_owned(),
                    ordinal: None,
                },
            },
            duration_ms: None,
            page_count: Some(20),
            chapter_count: None,
            published_at: None,
            status: MediaItemStatus::Available,
            created_at: UtcMillis(1),
            updated_at: UtcMillis(1),
        }
    }

    fn aggregate_source(
        media_item_id: MediaItemId,
        remote_chapter_id: &str,
        source_order: u32,
        availability: ComicChapterSourceStatus,
        profile: EditionProfile,
    ) -> ComicChapterAggregateSource {
        ComicChapterAggregateSource {
            reference: ChapterSourceRef {
                media_item_id,
                identity: ChapterSourceIdentity::new("mangadex", "manga-1", remote_chapter_id)
                    .unwrap(),
                metadata: ComicChapterMetadata {
                    edition_profile: profile,
                    chapter_number: Some(1.0),
                    volume_number: Some(1.0),
                    title: Some(format!("第 {source_order} 话")),
                    page_count: Some(20),
                    authoritative_content_key: None,
                },
                source_order,
                availability,
                published_at: None,
                source_updated_at: None,
                last_seen_generation: Some(1),
                updated_at: UtcMillis(10),
            },
            provider_rank: 0,
            pages: Vec::new(),
        }
    }

    fn observation(
        edition_id: EditionId,
        media_item: MediaItem,
        edition_profile: EditionProfile,
        sources: Vec<ComicChapterAggregateSource>,
    ) -> ComicWorkChapterObservation {
        ComicWorkChapterObservation {
            media_item,
            edition_id,
            edition_profile,
            sources,
            resource_facts: Vec::new(),
            progress: None,
            subject_id: None,
        }
    }

    fn resource_fact(
        resource_type: ResourceType,
        availability: Availability,
        locator_is_local: bool,
    ) -> ComicResourceAvailabilityFact {
        ComicResourceAvailabilityFact {
            resource_type,
            availability,
            locator_is_local,
        }
    }

    fn aggregate_for_test(
        observations: Vec<ComicWorkChapterObservation>,
        refresh_status: ComicWorkCatalogStatus,
    ) -> ComicWorkChapterCatalog {
        aggregate_comic_work_chapters(ComicWorkChapterAggregateInput {
            work_id: WorkId::new(),
            observations,
            current_media_item_id: None,
            refresh_status,
            last_observed_at: None,
            truncated: matches!(refresh_status, ComicWorkCatalogStatus::Truncated),
            refresh_receipts: Vec::new(),
        })
    }

    fn refresh_receipt(
        work_id: WorkId,
        remote_work_id: &str,
        status: ComicCatalogRefreshOutcomeStatus,
        observed_at: i64,
    ) -> ComicCatalogRefreshReceipt {
        ComicCatalogRefreshReceipt {
            id: ComicCatalogRefreshId::new(),
            work_id,
            source_key: "mangadex".to_owned(),
            remote_work_id: remote_work_id.to_owned(),
            status,
            generation_before: 1,
            generation_after: Some(2),
            observed_from: None,
            observed_to: None,
            truncated: status == ComicCatalogRefreshOutcomeStatus::Truncated,
            retained_previous_catalog: status == ComicCatalogRefreshOutcomeStatus::RefreshFailed,
            error_code: (status == ComicCatalogRefreshOutcomeStatus::RefreshFailed)
                .then(|| "SOURCE_RATE_LIMITED".to_owned()),
            observed_at: UtcMillis(observed_at),
        }
    }

    // ---- Work 级聚合测试 -----------------------------------------------------

    #[test]
    fn comic_work_catalog_aggregates_source_refs_within_one_media_item_and_keeps_candidates_apart()
    {
        let edition = EditionId::new();
        let profile = EditionProfile::from_language(Some("zh-cn"));
        let first = MediaItemId::new();
        let second = MediaItemId::new();

        let catalog = aggregate_for_test(
            vec![
                observation(
                    edition,
                    work_media_item(edition, first, Some(1.0)),
                    profile.clone(),
                    vec![
                        aggregate_source(
                            first,
                            "chapter-1",
                            0,
                            ComicChapterSourceStatus::Available,
                            profile.clone(),
                        ),
                        aggregate_source(
                            first,
                            "chapter-1-alternate",
                            1,
                            ComicChapterSourceStatus::Available,
                            profile.clone(),
                        ),
                    ],
                ),
                observation(
                    edition,
                    work_media_item(edition, second, Some(1.0)),
                    profile.clone(),
                    vec![aggregate_source(
                        second,
                        "chapter-1-mirror",
                        2,
                        ComicChapterSourceStatus::Unknown,
                        profile,
                    )],
                ),
            ],
            ComicWorkCatalogStatus::Synced,
        );

        assert_eq!(
            catalog.chapters.len(),
            2,
            "没有页面或内容证据的两个 MediaItem 不得静默合并"
        );
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
        let second_chapter = catalog
            .chapters
            .iter()
            .find(|chapter| chapter.media_item_id == second)
            .unwrap();
        assert_eq!(
            first_chapter.match_result.as_ref().map(|m| m.kind),
            Some(ChapterMatchKind::Candidate),
            "Candidate 必须保留匹配证据"
        );
        assert_eq!(
            second_chapter.match_result.as_ref().map(|m| m.kind),
            Some(ChapterMatchKind::Candidate)
        );
        assert_eq!(first_chapter.status, ComicChapterAggregateStatus::Unknown);
        assert!(!first_chapter.can_open);
        assert!(!second_chapter.can_open);
    }

    #[test]
    fn comic_work_catalog_merges_cross_media_items_only_with_content_evidence() {
        let edition = EditionId::new();
        let profile = EditionProfile::default();
        let content_left = MediaItemId::new();
        let content_right = MediaItemId::new();
        let variant_left = MediaItemId::new();
        let variant_right = MediaItemId::new();

        let mut content_a = aggregate_source(
            content_left,
            "content-a",
            0,
            ComicChapterSourceStatus::Available,
            profile.clone(),
        );
        content_a.reference.metadata.authoritative_content_key = Some("content-key".to_owned());
        let mut content_b = aggregate_source(
            content_right,
            "content-b",
            1,
            ComicChapterSourceStatus::Available,
            profile.clone(),
        );
        content_b.reference.metadata.authoritative_content_key = Some("content-key".to_owned());

        let mut variant_a = aggregate_source(
            variant_left,
            "variant-a",
            2,
            ComicChapterSourceStatus::Available,
            profile.clone(),
        );
        variant_a.pages = vec![
            PageIdentity::fingerprint("p1"),
            PageIdentity::fingerprint("p2"),
        ];
        let mut variant_b = aggregate_source(
            variant_right,
            "variant-b",
            3,
            ComicChapterSourceStatus::Available,
            profile.clone(),
        );
        variant_b.pages = vec![PageIdentity::fingerprint("p1")];

        let catalog = aggregate_for_test(
            vec![
                observation(
                    edition,
                    work_media_item(edition, content_left, Some(1.0)),
                    profile.clone(),
                    vec![content_a],
                ),
                observation(
                    edition,
                    work_media_item(edition, content_right, Some(1.0)),
                    profile.clone(),
                    vec![content_b],
                ),
                observation(
                    edition,
                    work_media_item(edition, variant_left, Some(2.0)),
                    profile.clone(),
                    vec![variant_a],
                ),
                observation(
                    edition,
                    work_media_item(edition, variant_right, Some(2.0)),
                    profile.clone(),
                    vec![variant_b],
                ),
            ],
            ComicWorkCatalogStatus::Synced,
        );

        assert_eq!(catalog.chapters.len(), 2);
        let content_chapter = catalog
            .chapters
            .iter()
            .find(|chapter| {
                chapter.match_result.as_ref().map(|m| m.kind) == Some(ChapterMatchKind::SameContent)
            })
            .expect("权威内容 key 相同必须归并");
        assert_eq!(content_chapter.sources.len(), 2);
        assert!(
            [content_left, content_right].contains(&content_chapter.media_item_id),
            "归并后的章节必须指向其中一个真实 MediaItem"
        );

        let variant_chapter = catalog
            .chapters
            .iter()
            .find(|chapter| {
                chapter.match_result.as_ref().map(|m| m.kind)
                    == Some(ChapterMatchKind::SameLogicalChapterVariant)
            })
            .expect("同一 Edition 内的逻辑变体必须保留 match_result 并归并");
        assert_eq!(variant_chapter.sources.len(), 2);
        assert!([variant_left, variant_right].contains(&variant_chapter.media_item_id));
    }

    /// 归并测试需要可控的 MediaItem 顺序，因此用固定 UUID 而不是随机 v7。
    fn fixed_media_item_id(value: u128) -> MediaItemId {
        MediaItemId::from_uuid(uuid::Uuid::from_u128(value))
    }

    fn content_keyed_source(
        media_item_id: MediaItemId,
        remote_chapter_id: &str,
        source_order: u32,
        profile: EditionProfile,
    ) -> ComicChapterAggregateSource {
        let mut source = aggregate_source(
            media_item_id,
            remote_chapter_id,
            source_order,
            ComicChapterSourceStatus::Available,
            profile,
        );
        source.reference.metadata.authoritative_content_key = Some("content-key".to_owned());
        source
    }

    fn comic_progress(media_item_id: MediaItemId, edition_id: EditionId) -> Progress {
        Progress {
            id: ProgressId::new(),
            work_id: WorkId::new(),
            edition_id,
            media_item_id,
            locator: Locator::Comic(ComicLocator {
                chapter_item_id: media_item_id,
                page_index: 3,
                page_progression: Some(0.25),
            }),
            completion: CompletionState::InProgress,
            percentage: Some(0.25),
            last_active_at: UtcMillis(777),
            updated_at: UtcMillis(777),
            revision: Some("revision-3".to_owned()),
            keyframe_uri: None,
        }
    }

    #[test]
    fn comic_work_catalog_keeps_the_current_media_item_as_representative_after_merge() {
        let edition = EditionId::new();
        let profile = EditionProfile::default();
        let current = fixed_media_item_id(1);
        let sibling = fixed_media_item_id(2);
        let subject_id = ComicProgressSubjectId::new();
        let sibling_progress = comic_progress(sibling, edition);

        // 两条 MediaItem 通过同一权威内容 key 真正归并（SameContent）。
        let current_observation = observation(
            edition,
            work_media_item(edition, current, Some(1.0)),
            profile.clone(),
            vec![content_keyed_source(
                current,
                "content-a",
                0,
                profile.clone(),
            )],
        );
        let mut sibling_observation = observation(
            edition,
            work_media_item(edition, sibling, Some(1.0)),
            profile.clone(),
            vec![content_keyed_source(sibling, "content-b", 1, profile)],
        );
        sibling_observation.resource_facts = vec![resource_fact(
            ResourceType::ComicArchive,
            Availability::Available,
            true,
        )];
        sibling_observation.progress = Some(sibling_progress.clone());
        sibling_observation.subject_id = Some(subject_id);

        let catalog = aggregate_comic_work_chapters(ComicWorkChapterAggregateInput {
            work_id: WorkId::new(),
            observations: vec![current_observation, sibling_observation],
            current_media_item_id: Some(current),
            refresh_status: ComicWorkCatalogStatus::Synced,
            last_observed_at: None,
            truncated: false,
            refresh_receipts: Vec::new(),
        });

        assert_eq!(catalog.chapters.len(), 1, "内容证据必须归并两条 MediaItem");
        let chapter = &catalog.chapters[0];
        assert_eq!(
            chapter.media_item_id, current,
            "当前读取上下文必须代表该章，即使另一个成员才有本地资源"
        );
        assert_eq!(catalog.current_media_item_id, Some(current));
        assert!(
            catalog
                .chapters
                .iter()
                .any(|chapter| chapter.media_item_id == current),
            "media_item 请求必须能在返回数组里定位当前项"
        );
        // 当前成员没有投影：Progress/Subject 回退到同一个投影成员，仍是原行。
        assert_eq!(chapter.progress.as_ref(), Some(&sibling_progress));
        assert_eq!(chapter.subject_id, Some(subject_id));
        assert_eq!(chapter.progress.as_ref().unwrap().media_item_id, sibling);
        assert_eq!(
            chapter.progress.as_ref().unwrap().locator,
            Locator::Comic(ComicLocator {
                chapter_item_id: sibling,
                page_index: 3,
                page_progression: Some(0.25),
            })
        );
        assert_eq!(
            chapter.progress.as_ref().unwrap().revision.as_deref(),
            Some("revision-3")
        );
        assert_eq!(
            chapter.progress.as_ref().unwrap().last_active_at,
            UtcMillis(777)
        );
    }

    #[test]
    fn comic_work_catalog_prefers_the_readable_projection_member_as_representative() {
        let edition = EditionId::new();
        let profile = EditionProfile::default();
        let plain = fixed_media_item_id(1);
        let readable = fixed_media_item_id(2);
        let readable_with_progress = fixed_media_item_id(3);
        let progress = comic_progress(readable_with_progress, edition);

        let with_resource = |mut observed: ComicWorkChapterObservation| {
            observed.resource_facts = vec![resource_fact(
                ResourceType::ComicArchive,
                Availability::Available,
                true,
            )];
            observed
        };

        let catalog = aggregate_for_test(
            vec![
                observation(
                    edition,
                    work_media_item(edition, plain, Some(1.0)),
                    profile.clone(),
                    vec![content_keyed_source(plain, "content-a", 0, profile.clone())],
                ),
                with_resource(observation(
                    edition,
                    work_media_item(edition, readable, Some(1.0)),
                    profile.clone(),
                    vec![content_keyed_source(
                        readable,
                        "content-b",
                        1,
                        profile.clone(),
                    )],
                )),
                with_resource({
                    let mut observed = observation(
                        edition,
                        work_media_item(edition, readable_with_progress, Some(1.0)),
                        profile.clone(),
                        vec![content_keyed_source(
                            readable_with_progress,
                            "content-c",
                            2,
                            profile,
                        )],
                    );
                    observed.progress = Some(progress.clone());
                    observed
                }),
            ],
            ComicWorkCatalogStatus::Synced,
        );

        assert_eq!(catalog.chapters.len(), 1);
        let chapter = &catalog.chapters[0];
        assert_eq!(
            chapter.media_item_id, readable_with_progress,
            "本地可打开且有投影的成员优先代表该章"
        );
        assert_eq!(
            chapter.progress.as_ref(),
            Some(&progress),
            "代表成员自身的 Progress 原值优先"
        );
        assert_eq!(
            chapter.progress.as_ref().unwrap().media_item_id,
            chapter.media_item_id,
            "代表成员自身有投影时，Reading 入口与 locator 必须指向同一 MediaItem"
        );
    }

    #[test]
    fn comic_work_catalog_takes_progress_and_subject_from_one_projection_member() {
        let edition = EditionId::new();
        let profile = EditionProfile::default();
        let readable = fixed_media_item_id(1);
        let progress_only = fixed_media_item_id(2);
        let subject_only = fixed_media_item_id(3);
        let progress = comic_progress(progress_only, edition);
        let subject_id = ComicProgressSubjectId::new();

        let mut progress_observation = observation(
            edition,
            work_media_item(edition, progress_only, Some(1.0)),
            profile.clone(),
            vec![content_keyed_source(
                progress_only,
                "content-b",
                1,
                profile.clone(),
            )],
        );
        progress_observation.progress = Some(progress.clone());

        let mut subject_observation = observation(
            edition,
            work_media_item(edition, subject_only, Some(1.0)),
            profile.clone(),
            vec![content_keyed_source(
                subject_only,
                "content-c",
                2,
                profile.clone(),
            )],
        );
        subject_observation.subject_id = Some(subject_id);

        let mut readable_observation = observation(
            edition,
            work_media_item(edition, readable, Some(1.0)),
            profile.clone(),
            vec![content_keyed_source(readable, "content-a", 0, profile)],
        );
        readable_observation.resource_facts = vec![resource_fact(
            ResourceType::ComicArchive,
            Availability::Available,
            true,
        )];

        let catalog = aggregate_for_test(
            vec![
                readable_observation,
                progress_observation,
                subject_observation,
            ],
            ComicWorkCatalogStatus::Synced,
        );

        assert_eq!(catalog.chapters.len(), 1);
        let chapter = &catalog.chapters[0];
        assert_eq!(
            chapter.media_item_id, readable,
            "没有 current 时本地可打开成员优先"
        );
        assert_eq!(chapter.progress.as_ref(), Some(&progress));
        assert_eq!(
            chapter.progress.as_ref().unwrap().media_item_id,
            progress_only
        );
        assert_eq!(
            chapter.subject_id, None,
            "Subject 必须与 Progress 取自同一个投影成员，不能各自挑一个 MediaItem"
        );
    }

    #[test]
    fn comic_work_catalog_splits_conflicting_content_lines_behind_mirror_label() {
        let edition = EditionId::new();
        let mirror = catalog_profile(
            "zh-cn",
            "line-a",
            ScanGroupFacet::mirror_label("mirror-a"),
            ColorMode::Grayscale,
        );
        let line_a = catalog_profile(
            "zh-cn",
            "line-a",
            ScanGroupFacet::content_line("scan-a"),
            ColorMode::Grayscale,
        );
        let line_b = catalog_profile(
            "zh-cn",
            "line-a",
            ScanGroupFacet::content_line("scan-b"),
            ColorMode::Grayscale,
        );
        let mirror_item = fixed_media_item_id(1);
        let line_a_item = fixed_media_item_id(2);
        let line_b_item = fixed_media_item_id(3);

        let catalog = aggregate_for_test(
            vec![
                observation(
                    edition,
                    work_media_item(edition, mirror_item, Some(1.0)),
                    mirror.clone(),
                    vec![aggregate_source(
                        mirror_item,
                        "m-1",
                        0,
                        ComicChapterSourceStatus::Available,
                        mirror,
                    )],
                ),
                observation(
                    edition,
                    work_media_item(edition, line_a_item, Some(2.0)),
                    line_a.clone(),
                    vec![aggregate_source(
                        line_a_item,
                        "a-1",
                        1,
                        ComicChapterSourceStatus::Available,
                        line_a,
                    )],
                ),
                observation(
                    edition,
                    work_media_item(edition, line_b_item, Some(3.0)),
                    line_b.clone(),
                    vec![aggregate_source(
                        line_b_item,
                        "b-1",
                        2,
                        ComicChapterSourceStatus::Available,
                        line_b,
                    )],
                ),
            ],
            ComicWorkCatalogStatus::Synced,
        );

        assert_eq!(
            catalog.editions.len(),
            2,
            "MirrorLabel 只对单次比较放宽，不能把两个真实冲突的内容线并进同一容器"
        );
        let chapter = |id: MediaItemId| {
            catalog
                .chapters
                .iter()
                .find(|chapter| chapter.media_item_id == id)
                .unwrap()
        };
        assert_eq!(chapter(mirror_item).next_media_item_id, Some(line_a_item));
        assert_eq!(
            chapter(line_a_item).previous_media_item_id,
            Some(mirror_item)
        );
        assert_eq!(chapter(line_a_item).next_media_item_id, None);
        assert_eq!(
            chapter(line_b_item).previous_media_item_id,
            None,
            "冲突的内容线必须单独成块，不得跨错误桶互连"
        );
        assert_eq!(chapter(line_b_item).next_media_item_id, None);
    }

    #[test]
    fn comic_work_catalog_respects_edition_profile_boundaries() {
        let shared_edition = EditionId::new();
        let unknown_edition = EditionId::new();
        let mirror_edition = EditionId::new();

        let zh = catalog_profile(
            "zh-cn",
            "line-a",
            ScanGroupFacet::content_line("scan-a"),
            ColorMode::Grayscale,
        );
        let ja = catalog_profile(
            "ja",
            "line-a",
            ScanGroupFacet::content_line("scan-a"),
            ColorMode::Grayscale,
        );
        let zh_item = MediaItemId::new();
        let ja_item = MediaItemId::new();
        let unknown_item_a = MediaItemId::new();
        let unknown_item_b = MediaItemId::new();
        let mirror_item_a = MediaItemId::new();
        let mirror_item_b = MediaItemId::new();

        let catalog = aggregate_for_test(
            vec![
                observation(
                    shared_edition,
                    work_media_item(shared_edition, zh_item, Some(1.0)),
                    zh.clone(),
                    vec![aggregate_source(
                        zh_item,
                        "zh-1",
                        0,
                        ComicChapterSourceStatus::Available,
                        zh,
                    )],
                ),
                observation(
                    shared_edition,
                    work_media_item(shared_edition, ja_item, Some(2.0)),
                    ja.clone(),
                    vec![aggregate_source(
                        ja_item,
                        "ja-1",
                        1,
                        ComicChapterSourceStatus::Available,
                        ja,
                    )],
                ),
                observation(
                    unknown_edition,
                    work_media_item(unknown_edition, unknown_item_a, Some(3.0)),
                    EditionProfile::default(),
                    vec![aggregate_source(
                        unknown_item_a,
                        "u-1",
                        2,
                        ComicChapterSourceStatus::Available,
                        EditionProfile::default(),
                    )],
                ),
                observation(
                    unknown_edition,
                    work_media_item(unknown_edition, unknown_item_b, Some(4.0)),
                    EditionProfile::default(),
                    vec![aggregate_source(
                        unknown_item_b,
                        "u-2",
                        3,
                        ComicChapterSourceStatus::Available,
                        EditionProfile::default(),
                    )],
                ),
                observation(
                    mirror_edition,
                    work_media_item(mirror_edition, mirror_item_a, Some(5.0)),
                    catalog_profile(
                        "zh-cn",
                        "line-a",
                        ScanGroupFacet::mirror_label("mirror-a"),
                        ColorMode::Grayscale,
                    ),
                    vec![aggregate_source(
                        mirror_item_a,
                        "m-1",
                        4,
                        ComicChapterSourceStatus::Available,
                        catalog_profile(
                            "zh-cn",
                            "line-a",
                            ScanGroupFacet::mirror_label("mirror-a"),
                            ColorMode::Grayscale,
                        ),
                    )],
                ),
                observation(
                    mirror_edition,
                    work_media_item(mirror_edition, mirror_item_b, Some(6.0)),
                    catalog_profile(
                        "zh-cn",
                        "line-a",
                        ScanGroupFacet::mirror_label("mirror-b"),
                        ColorMode::Grayscale,
                    ),
                    vec![aggregate_source(
                        mirror_item_b,
                        "m-2",
                        5,
                        ComicChapterSourceStatus::Available,
                        catalog_profile(
                            "zh-cn",
                            "line-a",
                            ScanGroupFacet::mirror_label("mirror-b"),
                            ColorMode::Grayscale,
                        ),
                    )],
                ),
            ],
            ComicWorkCatalogStatus::Synced,
        );

        assert_eq!(
            catalog.editions.len(),
            4,
            "已知语言冲突必须分开；Unknown/Unknown 与镜像标签差异共享一个容器"
        );
        let chapter = |id: MediaItemId| {
            catalog
                .chapters
                .iter()
                .find(|chapter| chapter.media_item_id == id)
                .unwrap()
        };
        // 同一个 EditionId 被画像冲突拆开后是两个互不链接的块。
        assert_eq!(chapter(zh_item).previous_media_item_id, None);
        assert_eq!(chapter(zh_item).next_media_item_id, None);
        assert_eq!(chapter(ja_item).previous_media_item_id, None);
        assert_eq!(chapter(ja_item).next_media_item_id, None);
        assert_ne!(
            chapter(zh_item).backend_order,
            chapter(ja_item).backend_order
        );
        // Unknown/Unknown 与镜像标签共享容器，因此同一块内相邻。
        assert_eq!(
            chapter(unknown_item_a).next_media_item_id,
            Some(unknown_item_b)
        );
        assert_eq!(
            chapter(mirror_item_a).next_media_item_id,
            Some(mirror_item_b)
        );
    }

    #[test]
    fn comic_work_catalog_derives_status_and_can_open_from_local_resources() {
        let edition = EditionId::new();
        let profile = EditionProfile::default();
        let local_available = MediaItemId::new();
        let offline_available = MediaItemId::new();
        let remote_only = MediaItemId::new();
        let temporarily = MediaItemId::new();
        let unknown = MediaItemId::new();
        let missing = MediaItemId::new();

        let with_fact = |id: MediaItemId,
                         number: f64,
                         order: u32,
                         chapter_id: &str,
                         source_status: ComicChapterSourceStatus,
                         fact: ComicResourceAvailabilityFact| {
            let mut observation = observation(
                edition,
                work_media_item(edition, id, Some(number)),
                profile.clone(),
                vec![aggregate_source(
                    id,
                    chapter_id,
                    order,
                    source_status,
                    profile.clone(),
                )],
            );
            observation.resource_facts = vec![fact];
            observation
        };

        let catalog = aggregate_for_test(
            vec![
                with_fact(
                    local_available,
                    1.0,
                    0,
                    "local-1",
                    ComicChapterSourceStatus::Available,
                    resource_fact(ResourceType::LocalFile, Availability::Available, true),
                ),
                with_fact(
                    offline_available,
                    2.0,
                    1,
                    "local-2",
                    ComicChapterSourceStatus::Missing,
                    resource_fact(
                        ResourceType::ComicArchive,
                        Availability::OfflineAvailable,
                        true,
                    ),
                ),
                with_fact(
                    remote_only,
                    3.0,
                    2,
                    "remote-1",
                    ComicChapterSourceStatus::ExternalOnly,
                    resource_fact(ResourceType::HttpFile, Availability::Available, false),
                ),
                with_fact(
                    temporarily,
                    4.0,
                    3,
                    "temp-1",
                    ComicChapterSourceStatus::TemporarilyUnavailable,
                    resource_fact(
                        ResourceType::ComicArchive,
                        Availability::TemporarilyUnavailable,
                        true,
                    ),
                ),
                with_fact(
                    unknown,
                    5.0,
                    4,
                    "unknown-1",
                    ComicChapterSourceStatus::Unknown,
                    resource_fact(ResourceType::HttpFile, Availability::Unknown, false),
                ),
                observation(
                    edition,
                    work_media_item(edition, missing, Some(6.0)),
                    profile.clone(),
                    vec![aggregate_source(
                        missing,
                        "missing-1",
                        5,
                        ComicChapterSourceStatus::Missing,
                        profile,
                    )],
                ),
            ],
            ComicWorkCatalogStatus::Synced,
        );

        let chapter = |id: MediaItemId| {
            catalog
                .chapters
                .iter()
                .find(|chapter| chapter.media_item_id == id)
                .unwrap()
        };

        assert_eq!(
            chapter(local_available).status,
            ComicChapterAggregateStatus::Available
        );
        assert!(chapter(local_available).can_open);

        assert_eq!(
            chapter(offline_available).status,
            ComicChapterAggregateStatus::Available,
            "Missing 来源仍有本地资源时必须按可打开处理"
        );
        assert!(chapter(offline_available).can_open);

        assert_eq!(
            chapter(remote_only).status,
            ComicChapterAggregateStatus::ExternalOnly
        );
        assert!(!chapter(remote_only).can_open);

        assert_eq!(
            chapter(temporarily).status,
            ComicChapterAggregateStatus::TemporarilyUnavailable
        );
        assert!(!chapter(temporarily).can_open);

        assert_eq!(
            chapter(unknown).status,
            ComicChapterAggregateStatus::Unknown
        );
        assert!(!chapter(unknown).can_open);

        assert_eq!(
            chapter(missing).status,
            ComicChapterAggregateStatus::Missing
        );
        assert!(!chapter(missing).can_open);
    }

    #[test]
    fn comic_work_catalog_never_turns_failed_or_truncated_refresh_into_missing() {
        let edition = EditionId::new();
        let profile = EditionProfile::default();
        let item = MediaItemId::new();

        for refresh_status in [
            ComicWorkCatalogStatus::RefreshFailed,
            ComicWorkCatalogStatus::Truncated,
        ] {
            let catalog = aggregate_for_test(
                vec![observation(
                    edition,
                    work_media_item(edition, item, Some(1.0)),
                    profile.clone(),
                    vec![aggregate_source(
                        item,
                        "missing-1",
                        0,
                        ComicChapterSourceStatus::Missing,
                        profile.clone(),
                    )],
                )],
                refresh_status,
            );
            assert_eq!(
                catalog.chapters[0].status,
                ComicChapterAggregateStatus::Unknown,
                "刷新失败/截断只写聚合根，不得把章节解释成 Missing"
            );
            assert!(!catalog.chapters[0].can_open);
            assert_eq!(catalog.refresh_status, refresh_status);
        }
    }

    #[test]
    fn comic_work_catalog_missing_is_scoped_to_each_source_receipt() {
        let work_id = WorkId::new();
        let edition = EditionId::new();
        let profile = EditionProfile::default();
        let successful_source_item = MediaItemId::new();
        let failed_source_item = MediaItemId::new();

        let successful_source = aggregate_source(
            successful_source_item,
            "chapter-a",
            0,
            ComicChapterSourceStatus::Missing,
            profile.clone(),
        );
        let mut failed_source = aggregate_source(
            failed_source_item,
            "chapter-b",
            1,
            ComicChapterSourceStatus::Missing,
            profile.clone(),
        );
        failed_source.reference.identity.remote_work_id = "manga-2".to_owned();

        let catalog = aggregate_comic_work_chapters(ComicWorkChapterAggregateInput {
            work_id,
            observations: vec![
                observation(
                    edition,
                    work_media_item(edition, successful_source_item, Some(1.0)),
                    profile.clone(),
                    vec![successful_source],
                ),
                observation(
                    edition,
                    work_media_item(edition, failed_source_item, Some(2.0)),
                    profile,
                    vec![failed_source],
                ),
            ],
            current_media_item_id: None,
            refresh_status: ComicWorkCatalogStatus::RefreshFailed,
            last_observed_at: Some(UtcMillis(200)),
            truncated: false,
            refresh_receipts: vec![
                refresh_receipt(
                    work_id,
                    "manga-1",
                    ComicCatalogRefreshOutcomeStatus::Succeeded,
                    100,
                ),
                refresh_receipt(
                    work_id,
                    "manga-2",
                    ComicCatalogRefreshOutcomeStatus::RefreshFailed,
                    200,
                ),
            ],
        });

        let successful = catalog
            .chapters
            .iter()
            .find(|chapter| {
                chapter
                    .sources
                    .iter()
                    .any(|source| source.identity.remote_work_id == "manga-1")
            })
            .expect("successful source chapter should remain in the catalog");
        assert_eq!(successful.status, ComicChapterAggregateStatus::Missing);

        let failed = catalog
            .chapters
            .iter()
            .find(|chapter| {
                chapter
                    .sources
                    .iter()
                    .any(|source| source.identity.remote_work_id == "manga-2")
            })
            .expect("failed source chapter should remain in the catalog");
        assert_eq!(failed.status, ComicChapterAggregateStatus::Unknown);
    }

    #[test]
    fn comic_work_catalog_status_uses_only_the_latest_receipt_per_source() {
        let work_id = WorkId::new();
        let receipt = |source_key: &str,
                       remote_work_id: &str,
                       status: ComicCatalogRefreshOutcomeStatus,
                       truncated: bool,
                       observed_at: i64| {
            ComicCatalogRefreshReceipt {
                id: ComicCatalogRefreshId::new(),
                work_id,
                source_key: source_key.to_owned(),
                remote_work_id: remote_work_id.to_owned(),
                status,
                generation_before: 1,
                generation_after: Some(2),
                observed_from: None,
                observed_to: None,
                truncated,
                retained_previous_catalog: false,
                error_code: None,
                observed_at: UtcMillis(observed_at),
            }
        };

        assert_eq!(
            comic_work_catalog_status_from_receipts(&[]),
            ComicWorkCatalogStatus::NeverSynced
        );
        assert_eq!(
            comic_work_catalog_status_from_receipts(&[receipt(
                "mangadex",
                "manga-1",
                ComicCatalogRefreshOutcomeStatus::Succeeded,
                false,
                10,
            )]),
            ComicWorkCatalogStatus::Synced
        );
        assert_eq!(
            comic_work_catalog_status_from_receipts(&[receipt(
                "mangadex",
                "manga-1",
                ComicCatalogRefreshOutcomeStatus::Succeeded,
                true,
                10,
            )]),
            ComicWorkCatalogStatus::Truncated
        );

        // 同一来源：历史失败不再遮蔽后来的成功。
        let old_failure = receipt(
            "mangadex",
            "manga-1",
            ComicCatalogRefreshOutcomeStatus::RefreshFailed,
            false,
            100,
        );
        let newer_success = receipt(
            "mangadex",
            "manga-1",
            ComicCatalogRefreshOutcomeStatus::Succeeded,
            false,
            200,
        );
        assert_eq!(
            comic_work_catalog_status_from_receipts(&[old_failure.clone(), newer_success.clone()]),
            ComicWorkCatalogStatus::Synced
        );

        // 同一来源：最新一次失败仍然优先于更早的截断成功。
        let older_truncated = receipt(
            "mangadex",
            "manga-1",
            ComicCatalogRefreshOutcomeStatus::Succeeded,
            true,
            100,
        );
        let newer_failure = receipt(
            "mangadex",
            "manga-1",
            ComicCatalogRefreshOutcomeStatus::RefreshFailed,
            false,
            200,
        );
        assert_eq!(
            comic_work_catalog_status_from_receipts(&[
                older_truncated.clone(),
                newer_failure.clone(),
            ]),
            ComicWorkCatalogStatus::RefreshFailed
        );

        // 不同来源：任一来源的最新观察是失败，聚合根就是失败。
        let failed_source = receipt(
            "other-source",
            "manga-9",
            ComicCatalogRefreshOutcomeStatus::RefreshFailed,
            false,
            300,
        );
        assert_eq!(
            comic_work_catalog_status_from_receipts(&[
                newer_success.clone(),
                failed_source.clone()
            ]),
            ComicWorkCatalogStatus::RefreshFailed
        );

        let summary = summarize_comic_work_refresh(&[
            receipt(
                "mangadex",
                "manga-1",
                ComicCatalogRefreshOutcomeStatus::RefreshFailed,
                false,
                100,
            ),
            receipt(
                "mangadex",
                "manga-1",
                ComicCatalogRefreshOutcomeStatus::Succeeded,
                false,
                300,
            ),
            receipt(
                "other-source",
                "manga-9",
                ComicCatalogRefreshOutcomeStatus::Succeeded,
                true,
                50,
            ),
            receipt(
                "other-source",
                "manga-9",
                ComicCatalogRefreshOutcomeStatus::Succeeded,
                false,
                200,
            ),
        ]);
        assert_eq!(
            summary.latest_receipts.len(),
            2,
            "两个来源各只保留最新一条 Receipt"
        );
        assert_eq!(
            summary.status,
            ComicWorkCatalogStatus::Synced,
            "mangadex 的历史失败被它自己的新成功取代"
        );
        assert!(
            !summary.truncated,
            "旧截断不能进入摘要：只有每个来源的最新观察决定覆盖度"
        );
        assert_eq!(summary.last_observed_at, Some(UtcMillis(300)));
        assert_eq!(summary.latest_receipts[0].source_key, "mangadex");
        assert_eq!(summary.latest_receipts[0].observed_at, UtcMillis(300));
        assert_eq!(summary.latest_receipts[1].source_key, "other-source");
        assert_eq!(summary.latest_receipts[1].observed_at, UtcMillis(200));
    }

    #[test]
    fn comic_work_catalog_latest_receipt_tie_breaks_on_stable_id() {
        let work_id = WorkId::new();
        let mut older_id = ComicCatalogRefreshId::new();
        let mut newer_id = ComicCatalogRefreshId::new();
        if older_id > newer_id {
            std::mem::swap(&mut older_id, &mut newer_id);
        }
        let receipt = |id: ComicCatalogRefreshId, status: ComicCatalogRefreshOutcomeStatus| {
            ComicCatalogRefreshReceipt {
                id,
                work_id,
                source_key: "mangadex".to_owned(),
                remote_work_id: "manga-1".to_owned(),
                status,
                generation_before: 1,
                generation_after: Some(2),
                observed_from: None,
                observed_to: None,
                truncated: false,
                retained_previous_catalog: false,
                error_code: None,
                // 同一时间戳：只有 id 能决定谁是最新观察。
                observed_at: UtcMillis(500),
            }
        };

        let latest = latest_receipts_by_source(&[
            receipt(older_id, ComicCatalogRefreshOutcomeStatus::RefreshFailed),
            receipt(newer_id, ComicCatalogRefreshOutcomeStatus::Succeeded),
        ]);
        assert_eq!(latest.len(), 1);
        assert_eq!(latest[0].id, newer_id);
        assert_eq!(
            comic_work_catalog_status_from_receipts(&[
                receipt(older_id, ComicCatalogRefreshOutcomeStatus::RefreshFailed),
                receipt(newer_id, ComicCatalogRefreshOutcomeStatus::Succeeded),
            ]),
            ComicWorkCatalogStatus::Synced
        );
    }

    #[test]
    fn comic_work_catalog_orders_chapters_and_links_only_within_one_edition() {
        let edition = EditionId::new();
        let other_edition = EditionId::new();
        let profile = EditionProfile::default();
        let third = MediaItemId::new();
        let first = MediaItemId::new();
        let special = MediaItemId::new();
        let second = MediaItemId::new();
        let foreign = MediaItemId::new();

        let numbered = |id: MediaItemId, number: f64, order: u32, chapter_id: &str| {
            observation(
                edition,
                work_media_item(edition, id, Some(number)),
                profile.clone(),
                vec![aggregate_source(
                    id,
                    chapter_id,
                    order,
                    ComicChapterSourceStatus::Available,
                    profile.clone(),
                )],
            )
        };

        let mut special_source = aggregate_source(
            special,
            "chapter-extra",
            9,
            ComicChapterSourceStatus::Available,
            profile.clone(),
        );
        special_source.reference.metadata.chapter_number = None;
        special_source.reference.metadata.volume_number = None;

        let catalog = aggregate_for_test(
            vec![
                numbered(third, 3.0, 2, "chapter-3"),
                numbered(first, 1.0, 0, "chapter-1"),
                observation(
                    edition,
                    work_media_item(edition, special, None),
                    profile.clone(),
                    vec![special_source],
                ),
                numbered(second, 2.0, 1, "chapter-2"),
                observation(
                    other_edition,
                    work_media_item(other_edition, foreign, Some(0.5)),
                    profile.clone(),
                    vec![aggregate_source(
                        foreign,
                        "chapter-0",
                        5,
                        ComicChapterSourceStatus::Available,
                        profile,
                    )],
                ),
            ],
            ComicWorkCatalogStatus::Synced,
        );

        let order: Vec<MediaItemId> = catalog
            .chapters
            .iter()
            .map(|chapter| chapter.media_item_id)
            .collect();
        assert_eq!(
            order,
            vec![first, second, third, special, foreign],
            "缺失章节号的番外排在已编号项之后，Edition 分组保持稳定"
        );
        for (position, chapter) in catalog.chapters.iter().enumerate() {
            assert_eq!(
                chapter.backend_order, position as u32,
                "backend_order 必须是整个返回数组的零基位置"
            );
        }

        let chapter = |id: MediaItemId| {
            catalog
                .chapters
                .iter()
                .find(|chapter| chapter.media_item_id == id)
                .unwrap()
        };
        assert_eq!(chapter(first).previous_media_item_id, None);
        assert_eq!(chapter(first).next_media_item_id, Some(second));
        assert_eq!(chapter(second).previous_media_item_id, Some(first));
        assert_eq!(chapter(second).next_media_item_id, Some(third));
        assert_eq!(chapter(third).next_media_item_id, Some(special));
        assert_eq!(chapter(special).previous_media_item_id, Some(third));
        assert_eq!(chapter(special).next_media_item_id, None);
        assert_eq!(
            chapter(foreign).previous_media_item_id,
            None,
            "不同 Edition 之间不得互相链接"
        );
        assert_eq!(chapter(foreign).next_media_item_id, None);
    }

    #[test]
    fn comic_work_catalog_projects_progress_and_subject_without_rewriting_them() {
        let edition = EditionId::new();
        let profile = EditionProfile::default();
        let item = MediaItemId::new();
        let subject_id = ComicProgressSubjectId::new();
        let progress = Progress {
            id: ProgressId::new(),
            work_id: WorkId::new(),
            edition_id: edition,
            media_item_id: item,
            locator: Locator::Comic(ComicLocator {
                chapter_item_id: item,
                page_index: 7,
                page_progression: Some(0.5),
            }),
            completion: CompletionState::InProgress,
            percentage: Some(0.5),
            last_active_at: UtcMillis(1_234),
            updated_at: UtcMillis(1_234),
            revision: Some("revision-9".to_owned()),
            keyframe_uri: None,
        };

        let mut observed = observation(
            edition,
            work_media_item(edition, item, Some(1.0)),
            profile.clone(),
            vec![aggregate_source(
                item,
                "chapter-1",
                0,
                ComicChapterSourceStatus::Available,
                profile,
            )],
        );
        observed.progress = Some(progress.clone());
        observed.subject_id = Some(subject_id);

        let catalog = aggregate_for_test(vec![observed], ComicWorkCatalogStatus::Synced);
        let chapter = &catalog.chapters[0];
        assert_eq!(chapter.progress.as_ref(), Some(&progress));
        assert_eq!(
            chapter.progress.as_ref().unwrap().locator,
            Locator::Comic(ComicLocator {
                chapter_item_id: item,
                page_index: 7,
                page_progression: Some(0.5),
            })
        );
        assert_eq!(
            chapter.progress.as_ref().unwrap().revision.as_deref(),
            Some("revision-9")
        );
        assert_eq!(
            chapter.progress.as_ref().unwrap().last_active_at,
            UtcMillis(1_234)
        );
        assert_eq!(chapter.subject_id, Some(subject_id));
    }

    #[test]
    fn comic_work_catalog_serializes_final_order_without_runtime_channels() {
        let edition = EditionId::new();
        let profile = EditionProfile::default();
        let item = MediaItemId::new();
        let mut observed = observation(
            edition,
            work_media_item(edition, item, Some(1.0)),
            profile.clone(),
            vec![aggregate_source(
                item,
                "chapter-1",
                0,
                ComicChapterSourceStatus::Available,
                profile,
            )],
        );
        observed.resource_facts = vec![resource_fact(
            ResourceType::LocalFile,
            Availability::Available,
            true,
        )];
        let catalog = aggregate_for_test(vec![observed], ComicWorkCatalogStatus::Synced);

        let value = serde_json::to_value(&catalog.chapters[0]).unwrap();
        let object = value.as_object().unwrap();
        for forbidden_field in ["url", "cookie", "grant", "request_headers", "local_path"] {
            assert!(
                !object.contains_key(forbidden_field),
                "章节聚合不得序列化运行时通道字段 {forbidden_field}"
            );
        }
        assert_eq!(value["backend_order"], 0);
        assert_eq!(value["status"], "available");
        assert_eq!(value["can_open"], true);
        assert_eq!(value["media_item_id"], item.to_string());
        // sort_key 只是 Domain 排序辅助值；它的存在不影响前端按数组顺序消费。
        assert!(value.get("sort_key").is_some());
        assert_eq!(value["sort_key"]["local_media_item_key"], item.to_string());
    }
}
