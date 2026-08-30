//! 领域实体与值对象。
//!
//! 规范：`plan/DOMAIN_MODEL.md`。本文件只含数据形态，不含业务规则。

use serde::{Deserialize, Serialize};

use haven_common::UtcMillis;

use crate::enums::*;
use crate::ids::*;
use crate::locator::Locator;

// ---------- 值对象 ----------

/// 媒体次序。避免所有类型硬塞 season/episode/chapter/page（§8）。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MediaIndex {
    Movie,
    Episode { season: Option<u32>, episode: u32 },
    Chapter { volume: Option<f32>, chapter: f32 },
    Article { ordinal: Option<u32> },
    Custom { label: String, ordinal: Option<f64> },
}

impl Eq for MediaIndex {}

impl Ord for MediaIndex {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        use std::cmp::Ordering;
        match (self, other) {
            (MediaIndex::Movie, MediaIndex::Movie) => Ordering::Equal,
            (MediaIndex::Movie, _) => Ordering::Less,
            (_, MediaIndex::Movie) => Ordering::Greater,
            (
                MediaIndex::Episode {
                    season: s1,
                    episode: e1,
                },
                MediaIndex::Episode {
                    season: s2,
                    episode: e2,
                },
            ) => s1.cmp(s2).then_with(|| e1.cmp(e2)),
            (MediaIndex::Episode { .. }, _) => Ordering::Less,
            (_, MediaIndex::Episode { .. }) => Ordering::Greater,
            (
                MediaIndex::Chapter {
                    volume: v1,
                    chapter: c1,
                },
                MediaIndex::Chapter {
                    volume: v2,
                    chapter: c2,
                },
            ) => {
                let v1_key = v1.map(|v| (v * 1000.0) as i32);
                let v2_key = v2.map(|v| (v * 1000.0) as i32);
                v1_key.cmp(&v2_key).then_with(|| {
                    let c1_key = (*c1 * 1000.0) as i32;
                    let c2_key = (*c2 * 1000.0) as i32;
                    c1_key.cmp(&c2_key)
                })
            }
            (MediaIndex::Chapter { .. }, _) => Ordering::Less,
            (_, MediaIndex::Chapter { .. }) => Ordering::Greater,
            (MediaIndex::Article { ordinal: o1 }, MediaIndex::Article { ordinal: o2 }) => {
                o1.cmp(o2)
            }
            (MediaIndex::Article { .. }, _) => Ordering::Less,
            (_, MediaIndex::Article { .. }) => Ordering::Greater,
            (MediaIndex::Custom { ordinal: o1, .. }, MediaIndex::Custom { ordinal: o2, .. }) => {
                o1.partial_cmp(o2).unwrap_or(Ordering::Equal)
            }
        }
    }
}

impl PartialOrd for MediaIndex {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

/// 内容哈希（§18）。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct ContentHash {
    pub algorithm: HashAlgorithm,
    pub digest: String,
}

/// 封面/背景图引用。不把 Base64 塞进主表（§19.1）。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct ArtworkRef {
    pub kind: ArtworkKind,
    pub uri: String,
    pub provider: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ArtworkKind {
    Poster,
    Cover,
    Backdrop,
    Thumbnail,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct ArtworkSet {
    pub poster: Option<ArtworkRef>,
    pub cover: Option<ArtworkRef>,
    pub backdrop: Option<ArtworkRef>,
    pub thumbnail: Option<ArtworkRef>,
}

/// 资源定位（资源在哪里）≠ Universal Locator（用户在资源内哪里）。
/// 来源：DOMAIN_MODEL §11
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ResourceLocator {
    LocalPath {
        path: String,
    },
    StorageObject {
        provider_id: StorageLocationId,
        object_id: String,
        path_hint: Option<String>,
    },
    Http {
        url: String,
    },
    SourceObject {
        source_id: SourceId,
        remote_id: String,
    },
}

// ---------- 实体 ----------

/// 作品（最上层内容实体，§4）。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct Work {
    pub id: WorkId,
    pub canonical_title: String,
    pub original_title: Option<String>,
    pub sort_title: Option<String>,
    pub description: Option<String>,
    pub work_type: WorkType,
    pub release_year: Option<i32>,
    pub language: Option<String>,
    /// 导演（019 正列；外部源清洗后 / 分隔）。
    pub director: Option<String>,
    /// 主演（019 正列；外部源清洗后 / 分隔）。
    pub actor: Option<String>,
    pub status: WorkStatus,
    /// 评分数值（契约 §11.2：value + 明确量表；无来源时为 None）。
    pub rating_value: Option<f64>,
    /// 评分量表（如豆瓣五星为 5.0）。
    pub rating_scale: Option<f64>,
    pub artwork: ArtworkSet,
    pub created_at: UtcMillis,
    pub updated_at: UtcMillis,
}

/// 版本（§5）。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct Edition {
    pub id: EditionId,
    pub work_id: WorkId,
    pub title: String,
    pub subtitle: Option<String>,
    pub edition_type: MediaType,
    pub release_date: Option<String>,
    pub language: Option<String>,
    pub region: Option<String>,
    pub publisher_or_studio: Option<String>,
    pub description: Option<String>,
    pub artwork: ArtworkSet,
    pub created_at: UtcMillis,
    pub updated_at: UtcMillis,
}

/// 媒体条目（实际消费单元，§7）。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct MediaItem {
    pub id: MediaItemId,
    pub edition_id: EditionId,
    pub parent_id: Option<MediaItemId>,
    pub media_type: MediaType,
    pub title: String,
    pub index: MediaIndex,
    pub duration_ms: Option<u64>,
    pub page_count: Option<u32>,
    pub chapter_count: Option<u32>,
    pub published_at: Option<String>,
    pub status: MediaItemStatus,
    pub created_at: UtcMillis,
    pub updated_at: UtcMillis,
}

/// 资源（§9）。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct Resource {
    pub id: ResourceId,
    pub media_item_id: MediaItemId,
    pub resource_type: ResourceType,
    pub source_id: Option<SourceId>,
    pub storage_location_id: Option<StorageLocationId>,
    pub locator: ResourceLocator,
    pub mime_type: Option<String>,
    pub size: Option<u64>,
    pub hash: Option<ContentHash>,
    pub availability: Availability,
    /// 可用性状态来源（迁移 009；位置级操作只作用于非 User 来源，见 enums::AvailabilitySource）。
    pub availability_source: AvailabilitySource,
    /// 文件修改时间（毫秒）——扫描变化检测（§39 Local File Identity）。
    pub modified_ms: Option<u64>,
    /// FastFingerprint 首块 SHA-256（§40）。
    pub fingerprint_first: Option<String>,
    /// FastFingerprint 末块 SHA-256（§40）。
    pub fingerprint_last: Option<String>,
    pub created_at: UtcMillis,
    pub updated_at: UtcMillis,
}

/// 存储位置（§13）。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct StorageLocation {
    pub id: StorageLocationId,
    pub provider_type: StorageProviderType,
    pub display_name: String,
    pub root_ref: String,
    pub credential_ref: Option<CredentialRef>,
    pub status: StorageStatus,
    pub created_at: UtcMillis,
    pub updated_at: UtcMillis,
}

/// 进度（§28）。Locator 是事实来源。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct Progress {
    pub id: ProgressId,
    pub work_id: WorkId,
    pub edition_id: EditionId,
    pub media_item_id: MediaItemId,
    pub locator: Locator,
    pub completion: CompletionState,
    pub percentage: Option<f32>,
    pub last_active_at: UtcMillis,
    pub updated_at: UtcMillis,
    /// 关键帧（data URL 或本地路径），可选，足迹页优先展示。
    pub keyframe_uri: Option<String>,
}

/// 标记（§41）。Highlight 是 Marker 的一种，不另建系统（§43）。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct Marker {
    pub id: MarkerId,
    pub work_id: WorkId,
    pub edition_id: EditionId,
    pub media_item_id: MediaItemId,
    pub locator: Locator,
    pub marker_type: MarkerType,
    pub title: Option<String>,
    pub excerpt: Option<String>,
    pub note: Option<String>,
    pub preview: Option<ArtworkRef>,
    pub created_at: UtcMillis,
    pub updated_at: UtcMillis,
    pub deleted_at: Option<UtcMillis>,
}

/// 收藏（§26）。默认优先收藏 Work。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct Favorite {
    pub target: FavoriteTarget,
    pub created_at: UtcMillis,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FavoriteTarget {
    Work(WorkId),
    Edition(EditionId),
    MediaItem(MediaItemId),
}

/// 历史（§27）。历史 ≠ 进度。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct HistoryEntry {
    pub id: HistoryEntryId,
    pub media_item_id: MediaItemId,
    pub work_id: WorkId,
    pub edition_id: EditionId,
    pub locator: Option<Locator>,
    pub started_at: UtcMillis,
    pub last_active_at: UtcMillis,
    pub completed_at: Option<UtcMillis>,
}

/// 下载任务（§45）。完成产生新 Offline Resource，不是修改原 Resource。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct DownloadTask {
    pub id: DownloadTaskId,
    pub work_id: Option<WorkId>,
    pub edition_id: Option<EditionId>,
    pub media_item_id: Option<MediaItemId>,
    pub source_resource_id: ResourceId,
    pub target_storage_id: StorageLocationId,
    /// 下载完成后生成的离线 Resource。删除任务记录不得级联删除该 Resource。
    pub offline_resource_id: Option<ResourceId>,
    pub state: DownloadState,
    pub bytes_total: Option<u64>,
    pub bytes_downloaded: u64,
    pub speed_bps: Option<u64>,
    pub eta_seconds: Option<u64>,
    pub created_at: UtcMillis,
    pub updated_at: UtcMillis,
    /// 批次聚合（整本/整季）。None 表示单任务。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub batch_id: Option<DownloadBatchId>,
    #[serde(default)]
    pub priority: DownloadPriority,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provider_key: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub host_key: Option<String>,
    #[serde(default)]
    pub variant_key: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub resource_identity: Option<String>,
    #[serde(default)]
    pub retry_count: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub not_before: Option<UtcMillis>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub resumable: Option<bool>,
}

/// 下载批次（整本/整季聚合，不直接拥有网络执行权）。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct DownloadBatch {
    pub id: DownloadBatchId,
    pub title: String,
    pub category: ContentCategory,
    pub subject_type: String,
    pub subject_id: String,
    pub target_storage_id: StorageLocationId,
    pub state: BatchState,
    pub total_tasks: u32,
    pub completed_tasks: u32,
    pub total_bytes: Option<u64>,
    pub completed_bytes: u64,
    pub created_at: UtcMillis,
    pub updated_at: UtcMillis,
}

/// 作品间关系（有向，10 种类型，`work_relations` 表）。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct WorkRelation {
    pub id: String,
    pub from_work_id: WorkId,
    pub to_work_id: WorkId,
    pub relation_type: RelationType,
    pub evidence: Option<String>,
    pub created_at: UtcMillis,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_work() -> Work {
        Work {
            id: WorkId::new(),
            canonical_title: "三体".into(),
            original_title: None,
            sort_title: None,
            description: None,
            work_type: WorkType::Fiction,
            release_year: Some(2008),
            language: Some("zh".into()),
            director: None,
            actor: None,
            status: WorkStatus::Completed,
            rating_value: Some(9.3),
            rating_scale: Some(10.0),
            artwork: ArtworkSet::default(),
            created_at: UtcMillis::now(),
            updated_at: UtcMillis::now(),
        }
    }

    #[test]
    fn work_entity_roundtrip() {
        let work = sample_work();
        let json = serde_json::to_string(&work).unwrap();
        let back: Work = serde_json::from_str(&json).unwrap();
        assert_eq!(work, back);
        assert_eq!(back.canonical_title, "三体");
    }

    #[test]
    fn media_index_supports_fractional_chapters() {
        let idx = MediaIndex::Chapter {
            volume: Some(2.0),
            chapter: 12.5,
        };
        let json = serde_json::to_string(&idx).unwrap();
        assert!(json.contains("12.5"));
    }

    #[test]
    fn download_task_tracks_byte_counts() {
        let task = DownloadTask {
            id: DownloadTaskId::new(),
            work_id: None,
            edition_id: None,
            media_item_id: None,
            source_resource_id: ResourceId::new(),
            target_storage_id: StorageLocationId::new(),
            offline_resource_id: None,
            state: DownloadState::Downloading,
            bytes_total: Some(1_000_000),
            bytes_downloaded: 250_000,
            speed_bps: Some(125_000),
            eta_seconds: Some(6),
            created_at: UtcMillis::now(),
            updated_at: UtcMillis::now(),
            batch_id: None,
            priority: DownloadPriority::Normal,
            provider_key: None,
            host_key: None,
            variant_key: String::new(),
            resource_identity: None,
            retry_count: 0,
            not_before: None,
            resumable: None,
        };
        assert_eq!(task.bytes_downloaded, 250_000);
        assert_eq!(task.state, DownloadState::Downloading);
    }
}
