//! 漫画章节目录的领域只读模型。
//!
//! 目录是来源在某个时间点观察到的章节集合，不是来源 URL、请求授权或
//! 阅读器页面会话。每个条目的来源身份仍然使用
//! `(source_key, remote_work_id, remote_chapter_id)`，这样目录刷新可以更新
//! 同一个来源章节，而不会把排序位置误当成章节主键。

use std::collections::HashSet;

use serde::{Deserialize, Serialize};

use haven_common::UtcMillis;

use crate::comic_identity::{ChapterSourceIdentity, ComicChapterMetadata};

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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::comic_identity::EditionProfile;

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
}
