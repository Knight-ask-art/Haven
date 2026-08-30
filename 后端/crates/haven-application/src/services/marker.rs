//! MarkerService：`marker_create` / `marker_list` / `marker_delete`（BE-MARKER-001）。
//!
//! 规则（契约 §23.2、DOMAIN_MODEL §41–§44）：
//! - workId/editionId 由后端从 MediaItem 推导。
//! - Locator kind 与 MediaType 兼容校验（复用 locator_kind_compatible）。
//! - delete 为软删除（墓碑语义，同步场景需要）。
//! - 标记列表不返回已软删除项。

use std::sync::Arc;

use haven_common::AppError;
use haven_domain::contracts::{EditionRepository, MarkerRepository, MediaItemRepository};
use haven_domain::entities::Marker;
use haven_domain::ids::{MarkerId, MediaItemId};
use haven_domain::locator::locator_kind_compatible;

use crate::mapper::time::utc_millis_to_rfc3339;
use crate::services::progress::wire_locator_to_domain;
use crate::wire::{MarkerCreateRequest, MarkerDto};

/// MarkerService 所需端口。
/// 访问方法由 blanket impl 提供（具体类型 → 子契约 coercion），
/// 避免 dyn→dyn trait upcasting（MSRV 1.85 不支持，E0658）。
pub trait MarkerPorts:
    MediaItemRepository + EditionRepository + MarkerRepository + Send + Sync
{
    fn as_media_item(&self) -> &dyn MediaItemRepository;
    fn as_edition(&self) -> &dyn EditionRepository;
    fn as_marker(&self) -> &dyn MarkerRepository;
}
impl<T> MarkerPorts for T
where
    T: MediaItemRepository + EditionRepository + MarkerRepository + Send + Sync,
{
    fn as_media_item(&self) -> &dyn MediaItemRepository {
        self
    }
    fn as_edition(&self) -> &dyn EditionRepository {
        self
    }
    fn as_marker(&self) -> &dyn MarkerRepository {
        self
    }
}

#[derive(Clone)]
pub struct MarkerService {
    ports: Arc<dyn MarkerPorts>,
}

impl MarkerService {
    pub fn new(ports: Arc<dyn MarkerPorts>) -> Self {
        Self { ports }
    }

    pub async fn create(&self, request: MarkerCreateRequest) -> Result<MarkerDto, AppError> {
        let media_item_id = request.media_item_id.parse().map_err(|_| invalid_id())?;

        // 推导 workId/editionId（与 Progress 同规则）。
        let item = self
            .ports
            .as_media_item()
            .get(media_item_id)
            .await?
            .ok_or_else(media_item_not_found)?;
        let edition = self
            .ports
            .as_edition()
            .get(item.edition_id)
            .await?
            .ok_or_else(edition_not_found)?;

        // Locator 兼容校验。
        let locator = wire_locator_to_domain(request.locator, media_item_id)?;
        if !locator_kind_compatible(item.media_type, &locator) {
            return Err(AppError::new(
                "LOCATOR_KIND_INCOMPATIBLE",
                haven_common::ErrorKind::Validation,
                "Locator 与媒介类型不兼容",
                false,
            ));
        }

        let now = haven_common::UtcMillis::now();
        let marker = Marker {
            id: MarkerId::new(),
            work_id: edition.work_id,
            edition_id: edition.id,
            media_item_id,
            locator,
            marker_type: request.marker_type.into(),
            title: request.title,
            excerpt: request.excerpt,
            note: request.note,
            preview: None,
            created_at: now,
            updated_at: now,
            deleted_at: None,
        };
        self.ports.as_marker().save(&marker).await?;
        to_dto(&marker)
    }

    pub async fn list(&self, media_item_id: MediaItemId) -> Result<Vec<MarkerDto>, AppError> {
        let markers = self
            .ports
            .as_marker()
            .list_for_media_item(media_item_id)
            .await?;
        // 任何一条无法投影（如 DB 中存在 Generic/损坏/未来版本 Locator）→ 显式失败，
        // 绝不伪造位置（审查修复：原实现静默替换为 Video(0)）。
        markers.iter().map(to_dto).collect()
    }

    /// 列出所有未软删除标记（足迹页聚合 Query，契约 §23.1）。
    /// limit 由 Repository 钳制到 MAX_MARKER_LIST_LIMIT；任何投影失败显式报错，不伪造。
    pub async fn list_all(&self, limit: u32) -> Result<Vec<MarkerDto>, AppError> {
        let markers = self.ports.as_marker().list_all(limit).await?;
        markers.iter().map(to_dto).collect()
    }

    /// 软删除（墓碑语义）；返回是否实际删除。
    pub async fn delete(&self, marker_id: MarkerId) -> Result<bool, AppError> {
        self.ports.as_marker().soft_delete(marker_id).await
    }
}

/// 投影失败时返回显式错误（不伪造 Locator）。
/// Generic Locator 无 wire 形态 → `LOCATOR_MAP_UNSUPPORTED`（与 mapper 规则一致）。
fn to_dto(marker: &Marker) -> Result<MarkerDto, AppError> {
    let locator = crate::mapper::locator::locator_to_dto(&marker.locator)?;
    Ok(MarkerDto {
        marker_id: marker.id.to_string(),
        media_item_id: marker.media_item_id.to_string(),
        work_id: marker.work_id.to_string(),
        edition_id: marker.edition_id.to_string(),
        locator,
        marker_type: marker.marker_type.into(),
        title: marker.title.clone(),
        excerpt: marker.excerpt.clone(),
        note: marker.note.clone(),
        created_at: utc_millis_to_rfc3339(marker.created_at),
        updated_at: utc_millis_to_rfc3339(marker.updated_at),
    })
}

fn invalid_id() -> AppError {
    AppError::new(
        "INVALID_MEDIA_ITEM_ID",
        haven_common::ErrorKind::Validation,
        "无效的 mediaItemId",
        false,
    )
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::wire::MarkerTypeDto;
    use haven_domain::contracts::{EditionRepository, MarkerRepository, MediaItemRepository};
    use haven_domain::entities::{Edition, MediaIndex, MediaItem};
    use haven_domain::enums::{MediaItemStatus, MediaType};
    use haven_domain::ids::{EditionId, WorkId};

    struct MemPorts {
        items: Vec<MediaItem>,
        editions: Vec<Edition>,
        markers: std::sync::Mutex<Vec<Marker>>,
    }

    fn mem_ports() -> (MemPorts, MediaItemId) {
        let work_id = WorkId::new();
        let edition_id = EditionId::new();
        let media_item_id = MediaItemId::new();
        let ports = MemPorts {
            items: vec![MediaItem {
                id: media_item_id,
                edition_id,
                parent_id: None,
                media_type: MediaType::Movie,
                title: "电影".into(),
                index: MediaIndex::Movie,
                duration_ms: None,
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
                edition_type: MediaType::Movie,
                release_date: None,
                language: None,
                region: None,
                publisher_or_studio: None,
                description: None,
                artwork: Default::default(),
                created_at: haven_common::UtcMillis(1),
                updated_at: haven_common::UtcMillis(1),
            }],
            markers: std::sync::Mutex::new(vec![]),
        };
        (ports, media_item_id)
    }

    fn create_request(
        media_item_id: MediaItemId,
        marker_type: MarkerTypeDto,
    ) -> MarkerCreateRequest {
        MarkerCreateRequest {
            media_item_id: media_item_id.to_string(),
            locator: crate::wire::LocatorDto::Video(crate::wire::VideoLocatorDto {
                position_ms: 30_000,
            }),
            marker_type,
            title: Some("名场面".into()),
            excerpt: None,
            note: Some("笔记".into()),
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
    impl MarkerRepository for MemPorts {
        async fn list_for_media_item(
            &self,
            media_item_id: MediaItemId,
        ) -> Result<Vec<Marker>, AppError> {
            Ok(self
                .markers
                .lock()
                .unwrap()
                .iter()
                .filter(|m| m.media_item_id == media_item_id && m.deleted_at.is_none())
                .cloned()
                .collect())
        }
        async fn list_all(&self, limit: u32) -> Result<Vec<Marker>, AppError> {
            Ok(self
                .markers
                .lock()
                .unwrap()
                .iter()
                .filter(|m| m.deleted_at.is_none())
                .take(limit as usize)
                .cloned()
                .collect())
        }
        async fn save(&self, marker: &Marker) -> Result<(), AppError> {
            let mut markers = self.markers.lock().unwrap();
            if let Some(existing) = markers.iter_mut().find(|m| m.id == marker.id) {
                *existing = marker.clone();
            } else {
                markers.push(marker.clone());
            }
            Ok(())
        }
        async fn soft_delete(&self, id: MarkerId) -> Result<bool, AppError> {
            let mut markers = self.markers.lock().unwrap();
            let marker = markers.iter_mut().find(|m| m.id == id);
            match marker {
                Some(m) if m.deleted_at.is_none() => {
                    m.deleted_at = Some(haven_common::UtcMillis::now());
                    Ok(true)
                }
                _ => Ok(false),
            }
        }
    }

    #[tokio::test]
    async fn create_derives_ids_and_returns_dto() {
        let (ports, media_item_id) = mem_ports();
        let service = MarkerService::new(Arc::new(ports));
        let dto = service
            .create(create_request(media_item_id, MarkerTypeDto::Scene))
            .await
            .unwrap();
        assert_eq!(dto.media_item_id, media_item_id.to_string());
        assert_eq!(dto.marker_type, MarkerTypeDto::Scene);
        assert_eq!(dto.note.as_deref(), Some("笔记"));
        let json = serde_json::to_string(&dto).unwrap();
        assert!(json.contains("\"markerId\""), "{json}");
        assert!(json.contains("\"markerType\":\"scene\""), "{json}");
    }

    #[tokio::test]
    async fn create_rejects_incompatible_locator() {
        let (ports, media_item_id) = mem_ports();
        let service = MarkerService::new(Arc::new(ports));
        let mut request = create_request(media_item_id, MarkerTypeDto::Bookmark);
        request.locator = crate::wire::LocatorDto::Book(crate::wire::BookLocatorDto {
            publication_resource: "c.xhtml".into(),
            progression: None,
            text_anchor: None,
            format_locator: None,
        });
        let err = service.create(request).await.unwrap_err();
        assert_eq!(err.code().as_str(), "LOCATOR_KIND_INCOMPATIBLE");
    }

    #[tokio::test]
    async fn delete_is_soft_and_idempotent() {
        let (ports, media_item_id) = mem_ports();
        let service = MarkerService::new(Arc::new(ports));
        let dto = service
            .create(create_request(media_item_id, MarkerTypeDto::Note))
            .await
            .unwrap();
        let marker_id: MarkerId = dto.marker_id.parse().unwrap();

        assert!(service.delete(marker_id).await.unwrap());
        assert!(!service.delete(marker_id).await.unwrap(), "重复删除 false");
        assert!(
            service.list(media_item_id).await.unwrap().is_empty(),
            "列表隐藏已删"
        );
    }
}
