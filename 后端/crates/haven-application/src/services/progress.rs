//! ProgressService：`progress_save` / `progress_recent` / `progress_reset`（BE-PROGRESS-001）。
//!
//! 规则（契约 §22.6、§23）：
//! - workId/editionId 由后端从 MediaItem 推导，前端禁止提交。
//! - Locator kind 必须与 MediaType 兼容（domain `locator_kind_compatible`）。
//! - expectedRevision 由 Repository 原子条件写校验；不做检查后再写的 TOCTOU。
//! - revision 由持久层返回 authoritative 值，同毫秒/并发写仍单调推进。
//! - percentage 由 Locator 派生（DOMAIN_MODEL §30：percentage 是派生值）。
//! - `progress.changed` 低频事件由 Interface 层发布。

use std::sync::Arc;

use haven_common::AppError;
use haven_domain::contracts::{EditionRepository, MediaItemRepository, ProgressRepository};
use haven_domain::entities::Progress;
use haven_domain::enums::CompletionState;
use haven_domain::ids::{MediaItemId, ProgressId};
use haven_domain::locator::{Locator, locator_kind_compatible};

use crate::mapper::progress::progress_summary;
use crate::services::library::MAX_LIMIT;
use crate::wire::{ProgressSaveRequest, ProgressSaveResult, ProgressSummaryDto};

/// ProgressService 所需端口（MediaItem + Edition 推导 + Progress 存储）。
pub trait ProgressPorts:
    MediaItemRepository + EditionRepository + ProgressRepository + Send + Sync
{
}
impl<T> ProgressPorts for T where
    T: MediaItemRepository + EditionRepository + ProgressRepository + Send + Sync
{
}

#[derive(Clone)]
pub struct ProgressService {
    ports: Arc<dyn ProgressPorts>,
}

impl ProgressService {
    pub fn new(ports: Arc<dyn ProgressPorts>) -> Self {
        Self { ports }
    }

    /// 保存进度。workId/editionId 由 MediaItem 推导；校验 Locator 兼容性。
    pub async fn save(&self, request: ProgressSaveRequest) -> Result<ProgressSaveResult, AppError> {
        let media_item_id = parse_id(&request.media_item_id)?;

        // 1. MediaItem → Edition → workId 推导（禁止前端提交 ID 矛盾）。
        let media_item = MediaItemRepository::get(&*self.ports, media_item_id)
            .await?
            .ok_or_else(media_item_not_found)?;
        let edition = EditionRepository::get(&*self.ports, media_item.edition_id)
            .await?
            .ok_or_else(edition_not_found)?;

        // 2. Locator 兼容校验（未知版本在反序列化时已拒绝；这里校验 kind 与媒介）。
        let locator = wire_locator_to_domain(request.locator, media_item_id)?;
        locator.validate().map_err(|message| {
            AppError::new(
                "INVALID_ARGUMENT",
                haven_common::ErrorKind::Validation,
                message,
                false,
            )
        })?;
        if !locator_kind_compatible(media_item.media_type, &locator) {
            return Err(AppError::new(
                "LOCATOR_KIND_INCOMPATIBLE",
                haven_common::ErrorKind::Validation,
                format!(
                    "Locator 与媒介类型不兼容（media_type={:?}）",
                    media_item.media_type
                ),
                false,
            ));
        }

        // 3. 候选时间只表达本次写入发生时间；最终 revision 由持久层在原子
        //    条件写内决定并返回，避免 read/check/save 三段式 TOCTOU。
        let updated_at = haven_common::UtcMillis::now();

        // 4. percentage 由 Locator 派生（DOMAIN_MODEL §30：事实来源是 Locator；
        //    mapper 只投影已派生的 percentage）。
        let percentage = derive_ratio(&media_item, &locator);

        let completion: CompletionState = request
            .completion
            .unwrap_or(crate::wire::CompletionWire::InProgress)
            .into();
        // 关键帧：可选的 data URL，前端截取的当前帧；超大或非 data:image/ 则忽略
        let mut keyframe_uri = request
            .keyframe
            .filter(|s| s.starts_with("data:image/") && s.len() < 300_000)
            .map(|s| s.to_string());
        // 未提供新帧时沿用旧帧，避免频繁更新丢失封面
        if keyframe_uri.is_none() {
            if let Ok(Some(existing)) = self.ports.get_for_media_item(media_item_id).await {
                keyframe_uri = existing.keyframe_uri;
            }
        }
        let progress = Progress {
            id: ProgressId::new(),
            work_id: edition.work_id,
            edition_id: edition.id,
            media_item_id,
            locator,
            completion,
            percentage,
            last_active_at: updated_at,
            updated_at,
            revision: None,
            keyframe_uri,
        };
        let revision = ProgressRepository::save_if_revision(
            &*self.ports,
            &progress,
            request.expected_revision.as_deref(),
        )
        .await?
        .ok_or_else(revision_conflict)?;

        Ok(ProgressSaveResult { revision })
    }

    pub async fn get(
        &self,
        media_item_id: MediaItemId,
    ) -> Result<Option<ProgressSummaryDto>, AppError> {
        let progress = self.ports.get_for_media_item(media_item_id).await?;
        progress.as_ref().map(progress_summary).transpose()
    }

    /// 最近活跃进度（首页 Continue 数据源）。
    pub async fn recent(&self, limit: u32) -> Result<Vec<ProgressSummaryDto>, AppError> {
        let items = self.ports.recent(limit.min(MAX_LIMIT)).await?;
        items.iter().map(progress_summary).collect()
    }

    /// `progress_reset`（契约 §23.2）：业务操作，不删除任何实体。
    /// completion → NotStarted，percentage 清空；Locator 保留（恢复起点）；
    /// **last_active_at 保留原值**（reset 不是新的观看/阅读活动，不得被 recent/LastActive
    /// 重排）；updated_at 由 Repository 单调推进（不回退）。
    pub async fn reset(&self, media_item_id: MediaItemId) -> Result<(), AppError> {
        let mut progress = self
            .ports
            .get_for_media_item(media_item_id)
            .await?
            .ok_or_else(|| {
                AppError::new(
                    "PROGRESS_NOT_FOUND",
                    haven_common::ErrorKind::NotFound,
                    "进度不存在",
                    false,
                )
            })?;
        progress.completion = CompletionState::NotStarted;
        progress.percentage = None;
        progress.keyframe_uri = None;
        progress.updated_at = haven_common::UtcMillis::now();
        ProgressRepository::save(&*self.ports, &progress).await
    }
}

fn parse_id(s: &str) -> Result<MediaItemId, AppError> {
    s.parse().map_err(|_| {
        AppError::new(
            "INVALID_MEDIA_ITEM_ID",
            haven_common::ErrorKind::Validation,
            "无效的 mediaItemId",
            false,
        )
    })
}

/// 从 Locator 派生 percentage（DOMAIN_MODEL §30）。无法派生时返回 None。
fn derive_ratio(item: &haven_domain::entities::MediaItem, locator: &Locator) -> Option<f32> {
    let clamp01 = |v: f32| v.clamp(0.0, 1.0);
    match locator {
        Locator::Video(v) => item
            .duration_ms
            .filter(|d| *d > 0)
            .map(|d| clamp01(v.position_ms as f32 / d as f32)),
        Locator::Book(b) => b.progression.map(clamp01),
        Locator::Pdf(p) => item
            .page_count
            .filter(|c| *c > 0)
            .map(|c| clamp01((p.page_index as f32 + 1.0) / c as f32)),
        Locator::Comic(c) => c.page_progression.map(clamp01),
        Locator::Article(a) => a.progression.map(clamp01),
        Locator::Generic(_) => None,
    }
}

/// wire Locator → domain Locator。
/// Video/Comic 的 media_item_id 必须由请求的 media_item_id 填充（wire 不携带）。
/// 供 ProgressService / MarkerService 共用（BE-MAPPER-001 的 wire→domain 方向）。
pub(crate) fn wire_locator_to_domain(
    locator: crate::wire::LocatorDto,
    media_item_id: MediaItemId,
) -> Result<Locator, AppError> {
    let map_anchor = |t: crate::wire::TextAnchorDto| haven_domain::locator::TextAnchor {
        exact: t.exact,
        prefix: t.prefix,
        suffix: t.suffix,
    };
    Ok(match locator {
        crate::wire::LocatorDto::Video(v) => Locator::Video(haven_domain::locator::VideoLocator {
            media_item_id,
            position_ms: v.position_ms,
        }),
        crate::wire::LocatorDto::Book(b) => Locator::Book(haven_domain::locator::BookLocator {
            publication_resource: b.publication_resource,
            progression: b.progression.map(|v| v as f32),
            text_anchor: b.text_anchor.map(&map_anchor),
            format_locator: b.format_locator,
        }),
        crate::wire::LocatorDto::Pdf(p) => Locator::Pdf(haven_domain::locator::PdfLocator {
            page_index: p.page_index,
            x: p.x.map(|v| v as f32),
            y: p.y.map(|v| v as f32),
            zoom: p.zoom.map(|v| v as f32),
            text_anchor: p.text_anchor.map(&map_anchor),
        }),
        crate::wire::LocatorDto::Comic(c) => {
            if c.chapter_item_id != media_item_id.to_string() {
                return Err(AppError::new(
                    "INVALID_ARGUMENT",
                    haven_common::ErrorKind::Validation,
                    "Comic locator 的 chapterItemId 必须与 mediaItemId 一致",
                    false,
                ));
            }
            Locator::Comic(haven_domain::locator::ComicLocator {
                chapter_item_id: media_item_id,
                page_index: c.page_index,
                page_progression: c.page_progression.map(|v| v as f32),
            })
        }
        crate::wire::LocatorDto::Article(a) => {
            Locator::Article(haven_domain::locator::ArticleLocator {
                block_id: a.block_id,
                progression: a.progression.map(|v| v as f32),
                text_anchor: a.text_anchor.map(&map_anchor),
            })
        }
    })
}

fn media_item_not_found() -> AppError {
    AppError::new(
        "MEDIA_ITEM_NOT_FOUND",
        haven_common::ErrorKind::NotFound,
        "媒体条目不存在",
        false,
    )
}

fn edition_not_found() -> AppError {
    AppError::new(
        "EDITION_NOT_FOUND",
        haven_common::ErrorKind::NotFound,
        "版本不存在",
        false,
    )
}

fn revision_conflict() -> AppError {
    AppError::new(
        "REVISION_CONFLICT",
        haven_common::ErrorKind::Conflict,
        "进度已被其他会话更新，请刷新后重试",
        false,
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use haven_domain::contracts::{EditionRepository, MediaItemRepository, ProgressRepository};
    use haven_domain::entities::{Edition, MediaIndex, MediaItem};
    use haven_domain::enums::MediaItemStatus;
    use haven_domain::enums::MediaType;
    use haven_domain::ids::{EditionId, WorkId};

    /// 内存端口 + 最新保存的 Progress 记录（供断言）。
    struct MemPorts {
        items: Vec<MediaItem>,
        editions: Vec<Edition>,
        saved: std::sync::Mutex<Option<Progress>>,
        revision_counter: std::sync::Mutex<u32>,
    }

    fn mem_ports(media_type: MediaType, duration_ms: Option<u64>) -> (MemPorts, MediaItemId) {
        let work_id = WorkId::new();
        let edition_id = EditionId::new();
        let media_item_id = MediaItemId::new();
        let ports = MemPorts {
            items: vec![MediaItem {
                id: media_item_id,
                edition_id,
                parent_id: None,
                media_type,
                title: "内容".into(),
                index: MediaIndex::Movie,
                duration_ms,
                page_count: None,
                chapter_count: None,
                published_at: None,
                status: MediaItemStatus::Available,
                created_at: haven_common::UtcMillis(1),
                updated_at: haven_common::UtcMillis(1),
            }],
            editions: vec![Edition {
                id: edition_id,
                work_id,
                title: "版本".into(),
                subtitle: None,
                edition_type: media_type,
                release_date: None,
                language: None,
                region: None,
                publisher_or_studio: None,
                description: None,
                artwork: Default::default(),
                created_at: haven_common::UtcMillis(1),
                updated_at: haven_common::UtcMillis(1),
            }],
            saved: std::sync::Mutex::new(None),
            revision_counter: std::sync::Mutex::new(0),
        };
        (ports, media_item_id)
    }

    fn video_request(media_item_id: MediaItemId, position_ms: u64) -> ProgressSaveRequest {
        ProgressSaveRequest {
            media_item_id: media_item_id.to_string(),
            locator: crate::wire::LocatorDto::Video(crate::wire::VideoLocatorDto { position_ms }),
            completion: Some(crate::wire::CompletionWire::InProgress),
            expected_revision: None,
            keyframe: None,
        }
    }

    #[async_trait::async_trait]
    impl MediaItemRepository for MemPorts {
        async fn get(&self, id: MediaItemId) -> Result<Option<MediaItem>, AppError> {
            Ok(self.items.iter().find(|i| i.id == id).cloned())
        }
        async fn save(&self, _m: &MediaItem) -> Result<(), AppError> {
            Ok(())
        }
        async fn list_by_edition(&self, _e: EditionId) -> Result<Vec<MediaItem>, AppError> {
            Ok(vec![])
        }
        async fn delete(&self, _id: MediaItemId) -> Result<bool, AppError> {
            Ok(false)
        }
    }

    #[async_trait::async_trait]
    impl EditionRepository for MemPorts {
        async fn get(&self, id: EditionId) -> Result<Option<Edition>, AppError> {
            Ok(self.editions.iter().find(|e| e.id == id).cloned())
        }
        async fn save(&self, _e: &Edition) -> Result<(), AppError> {
            Ok(())
        }
        async fn list_by_work(&self, _w: WorkId) -> Result<Vec<Edition>, AppError> {
            Ok(vec![])
        }
        async fn delete(&self, _id: EditionId) -> Result<bool, AppError> {
            Ok(false)
        }
    }

    #[async_trait::async_trait]
    impl ProgressRepository for MemPorts {
        async fn get_for_media_item(&self, id: MediaItemId) -> Result<Option<Progress>, AppError> {
            let guard = self.saved.lock().unwrap();
            Ok(guard.as_ref().filter(|p| p.media_item_id == id).cloned())
        }
        async fn save(&self, progress: &Progress) -> Result<(), AppError> {
            *self.saved.lock().unwrap() = Some(progress.clone());
            Ok(())
        }
        async fn save_if_revision(
            &self,
            progress: &Progress,
            expected_revision: Option<&str>,
        ) -> Result<Option<String>, AppError> {
            let mut guard = self.saved.lock().unwrap();
            if let Some(expected) = expected_revision {
                let current = guard
                    .as_ref()
                    .and_then(|progress| progress.revision.as_deref());
                if current != Some(expected) {
                    return Ok(None);
                }
            }
            let mut stored = progress.clone();
            // 1.85 MSRV：不用 let-chains，改嵌套 if 保证同毫秒候选单调推进。
            if let Some(current) = guard.as_ref() {
                if stored.updated_at.0 <= current.updated_at.0 {
                    stored.updated_at = haven_common::UtcMillis(current.updated_at.0 + 1);
                }
            }
            let mut counter = self.revision_counter.lock().unwrap();
            *counter += 1;
            let revision = format!("mock-progress-revision-{}", *counter);
            stored.revision = Some(revision.clone());
            *guard = Some(stored);
            Ok(Some(revision))
        }
        async fn recent(&self, _limit: u32) -> Result<Vec<Progress>, AppError> {
            Ok(self.saved.lock().unwrap().clone().into_iter().collect())
        }
    }

    #[tokio::test]
    async fn save_derives_ids_and_ratio_from_locator() {
        // duration 100_000ms，position 50_000ms → ratio 0.5
        let (ports, media_item_id) = mem_ports(MediaType::Movie, Some(100_000));
        let service = ProgressService::new(Arc::new(ports));
        let result = service
            .save(video_request(media_item_id, 50_000))
            .await
            .unwrap();
        assert!(!result.revision.is_empty());

        let saved = service.get(media_item_id).await.unwrap().expect("已保存");
        assert_eq!(saved.completion, crate::wire::CompletionWire::InProgress);
        assert_eq!(saved.progress_ratio, Some(0.5), "ratio 必须从 Locator 派生");
    }

    #[tokio::test]
    async fn save_rejects_incompatible_locator_kind() {
        let (ports, media_item_id) = mem_ports(MediaType::Movie, None);
        let service = ProgressService::new(Arc::new(ports));
        let mut request = video_request(media_item_id, 60_000);
        request.locator = crate::wire::LocatorDto::Book(crate::wire::BookLocatorDto {
            publication_resource: "c.xhtml".into(),
            progression: None,
            text_anchor: None,
            format_locator: None,
        });
        let err = service.save(request).await.unwrap_err();
        assert_eq!(err.code().as_str(), "LOCATOR_KIND_INCOMPATIBLE");
        assert!(
            service.get(media_item_id).await.unwrap().is_none(),
            "非法不覆盖旧值"
        );
    }

    #[tokio::test]
    async fn save_rejects_unknown_media_item() {
        let (ports, _) = mem_ports(MediaType::Movie, None);
        let service = ProgressService::new(Arc::new(ports));
        let err = service
            .save(video_request(MediaItemId::new(), 0))
            .await
            .unwrap_err();
        assert_eq!(err.code().as_str(), "MEDIA_ITEM_NOT_FOUND");
    }

    #[tokio::test]
    async fn expected_revision_conflict_detected() {
        let (ports, media_item_id) = mem_ports(MediaType::Movie, Some(100_000));
        let service = ProgressService::new(Arc::new(ports));
        let first = service
            .save(video_request(media_item_id, 10_000))
            .await
            .unwrap();

        // 正确 revision 通过
        let mut ok = video_request(media_item_id, 20_000);
        ok.expected_revision = Some(first.revision.clone());
        assert!(service.save(ok).await.is_ok());

        // 过期 revision 冲突
        let mut stale = video_request(media_item_id, 30_000);
        stale.expected_revision = Some(first.revision);
        let err = service.save(stale).await.unwrap_err();
        assert_eq!(err.code().as_str(), "REVISION_CONFLICT");
        assert!(!err.retryable());
    }

    #[tokio::test]
    async fn reset_changes_completion_without_delete() {
        let (ports, media_item_id) = mem_ports(MediaType::Movie, Some(100_000));
        let service = ProgressService::new(Arc::new(ports));
        service
            .save(video_request(media_item_id, 50_000))
            .await
            .unwrap();
        service.reset(media_item_id).await.unwrap();
        let saved = service.get(media_item_id).await.unwrap().expect("仍存在");
        assert_eq!(saved.completion, crate::wire::CompletionWire::NotStarted);
        assert_eq!(saved.progress_ratio, None);
    }

    #[tokio::test]
    async fn reset_missing_progress_errors() {
        let (ports, media_item_id) = mem_ports(MediaType::Movie, None);
        let service = ProgressService::new(Arc::new(ports));
        let err = service.reset(media_item_id).await.unwrap_err();
        assert_eq!(err.code().as_str(), "PROGRESS_NOT_FOUND");
    }

    #[test]
    fn parse_id_rules() {
        assert!(parse_id("not-a-uuid").is_err());
        assert!(parse_id("00000000-0000-0000-0000-000000000001").is_ok());
    }
}
