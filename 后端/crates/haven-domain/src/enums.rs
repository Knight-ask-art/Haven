//! 领域枚举。
//!
//! 规范：`plan/DOMAIN_MODEL.md`。IPC 序列化统一 `snake_case`。

use serde::{Deserialize, Serialize};

/// 作品的抽象形态（不是消费媒介）。
/// 来源：DOMAIN_MODEL §4.4
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WorkType {
    Fiction,
    NonFiction,
    Franchise,
    Series,
    Article,
    Standalone,
    Unknown,
}

/// 作品当前更新/发布状态。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WorkStatus {
    Ongoing,
    Completed,
    Hiatus,
    Unknown,
}

/// 媒介形态（Edition / MediaItem 层）。
/// 来源：DOMAIN_MODEL §6
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MediaType {
    Movie,
    Series,
    Episode,
    Book,
    Document,
    Comic,
    Article,
    Audio,
    Unknown,
}

/// 一级内容分类（产品锁定：全部 / 影视 / 图书 / 漫画 / 报刊资料）。
/// 来源：DEVELOPMENT_ROADMAP §279
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ContentCategory {
    All,
    Video,
    Book,
    Comic,
    Periodical,
}

impl ContentCategory {
    /// 由 MediaType 推导默认一级分类（canonical 值在 DB 层与 media_type 保持一致性）。
    pub fn from_media_type(media_type: MediaType) -> Self {
        match media_type {
            MediaType::Movie | MediaType::Series | MediaType::Episode | MediaType::Audio => {
                ContentCategory::Video
            }
            MediaType::Book => ContentCategory::Book,
            MediaType::Comic => ContentCategory::Comic,
            MediaType::Document | MediaType::Article => ContentCategory::Periodical,
            MediaType::Unknown => ContentCategory::All,
        }
    }
}

/// 媒体条目状态。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MediaItemStatus {
    Available,
    Unavailable,
    Unknown,
}

/// 消费完成状态。
/// 来源：DOMAIN_MODEL §29
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CompletionState {
    NotStarted,
    InProgress,
    Completed,
    Abandoned,
}

/// 标记类型（UI 统一叫“标记”，底层区分类型）。
/// 来源：DOMAIN_MODEL §42
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MarkerType {
    Bookmark,
    Highlight,
    Note,
    Scene,
    Quote,
    Image,
}

/// 资源可用性。
/// 来源：DOMAIN_MODEL §48
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Availability {
    Available,
    OfflineAvailable,
    TemporarilyUnavailable,
    SourceUnavailable,
    StorageUnavailable,
    Missing,
    Unknown,
}

/// 可用性状态来源（R-MAIN-02 复审修复：位置级操作不得覆盖用户/扫描器显式标记）。
///
/// - `User`：扫描器/未来用户操作显式设置（SourceUnavailable、TemporarilyUnavailable、
///   Unknown、资源自身 Missing 等）——**位置级操作绝不触碰**。
/// - `Storage`：位置级自动标记（disconnect/path-missing/rebind 无效化/恢复）。
/// - `Unknown`：迁移前数据（无来源记录）；位置级操作可迁移归位（发布前无持久库）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AvailabilitySource {
    User,
    Storage,
    Unknown,
}

/// 资源类型。
/// 来源：DOMAIN_MODEL §10
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ResourceType {
    LocalFile,
    CloudFile,
    HttpFile,
    VideoStream,
    HlsStream,
    DashStream,
    PublicationFile,
    ComicArchive,
    ImageSequence,
    ArticleSnapshot,
    RemoteChapter,
    RemotePageSet,
}

/// 存储提供方类型。
/// 来源：LIBRARY_AND_STORAGE §13
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StorageProviderType {
    Local,
    WebDav,
    OneDrive,
    GoogleDrive,
}

/// 存储位置状态。
/// 来源：LIBRARY_AND_STORAGE §17
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StorageStatus {
    Connected,
    Disconnected,
    AuthExpired,
    Unavailable,
    ReadOnly,
    Error,
    Disabled,
    /// 位置存在但根目录当前不可达（目录被移动/删除；BE-STORAGE-001 状态迁移）。
    Missing,
}

/// 下载状态（含重启恢复的 Interrupted）。
/// 来源：DOMAIN_MODEL §46
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DownloadState {
    Queued,
    Resolving,
    Downloading,
    Paused,
    Verifying,
    Completed,
    Failed,
    Cancelled,
    Interrupted,
}

/// 下载优先级（Batch 调度用）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum DownloadPriority {
    Low,
    #[default]
    Normal,
    High,
}

/// 批次聚合状态（派生自子任务，非持久化直接写入由 BatchService 维护）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum BatchState {
    #[default]
    Queued,
    Downloading,
    Verifying,
    Completed,
    Failed,
    Cancelled,
    PartialCompleted,
}

/// 来源类型。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SourceType {
    Rss,
    Opds,
    Metadata,
    Video,
    Novel,
    Comic,
    Article,
    Unknown,
}

/// 作品间关系类型（有方向）。
/// 来源：DOMAIN_MODEL §14.1
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RelationType {
    OriginalOf,
    AdaptationOf,
    SequelOf,
    PrequelOf,
    SpinOffOf,
    SideStoryOf,
    RemakeOf,
    SameFranchise,
    InspiredBy,
    Related,
}

/// 别名类型。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AliasType {
    Canonical,
    Original,
    Short,
    Romanized,
    Translation,
    Other,
}

/// 哈希算法。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum HashAlgorithm {
    Sha256,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn category_derivation_from_media_type() {
        assert_eq!(
            ContentCategory::from_media_type(MediaType::Movie),
            ContentCategory::Video
        );
        assert_eq!(
            ContentCategory::from_media_type(MediaType::Episode),
            ContentCategory::Video
        );
        assert_eq!(
            ContentCategory::from_media_type(MediaType::Book),
            ContentCategory::Book
        );
        assert_eq!(
            ContentCategory::from_media_type(MediaType::Comic),
            ContentCategory::Comic
        );
        assert_eq!(
            ContentCategory::from_media_type(MediaType::Article),
            ContentCategory::Periodical
        );
        assert_eq!(
            ContentCategory::from_media_type(MediaType::Document),
            ContentCategory::Periodical
        );
    }

    #[test]
    fn enums_serialize_snake_case() {
        assert_eq!(
            serde_json::to_string(&MarkerType::Scene).unwrap(),
            "\"scene\""
        );
        assert_eq!(
            serde_json::to_string(&DownloadState::Interrupted).unwrap(),
            "\"interrupted\""
        );
        assert_eq!(
            serde_json::to_string(&StorageProviderType::WebDav).unwrap(),
            "\"web_dav\""
        );
    }
}
