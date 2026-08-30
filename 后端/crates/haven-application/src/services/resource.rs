use std::sync::Arc;

use haven_common::{AppError, ErrorKind};
use haven_domain::contracts::{MediaItemRepository, ResourceRepository, StorageLocationRepository};
use haven_domain::entities::{Resource, ResourceLocator};
use haven_domain::enums::{Availability, ResourceType, StorageStatus};
use haven_domain::ids::MediaItemId;

use crate::services::ports::ResourceListPorts;
use crate::wire::{
    AvailabilityDto, ResourceListByMediaItemRequest, ResourceListDto, ResourceSummaryDto,
    ResourceTypeDto,
};

const SCHEMA_VERSION: u32 = 1;

#[derive(Clone)]
pub struct ResourceService {
    ports: Arc<dyn ResourceListPorts>,
}

impl ResourceService {
    pub fn new(ports: Arc<dyn ResourceListPorts>) -> Self {
        Self { ports }
    }

    /// 返回指定 MediaItem 的安全资源摘要。
    ///
    /// 这里先校验 MediaItem 所有权，再解析位置显示名；locator 只在后端内存中
    /// 用于派生 isLocal，绝不进入 Wire。
    pub async fn list_by_media_item(
        &self,
        request: ResourceListByMediaItemRequest,
    ) -> Result<ResourceListDto, AppError> {
        let media_item_id: MediaItemId = request.media_item_id.parse().map_err(|_| {
            AppError::new(
                "INVALID_ID",
                ErrorKind::Validation,
                "无效的媒体条目 ID",
                false,
            )
        })?;

        if MediaItemRepository::get(&*self.ports, media_item_id)
            .await?
            .is_none()
        {
            return Err(AppError::new(
                "MEDIA_ITEM_NOT_FOUND",
                ErrorKind::NotFound,
                "媒体条目不存在",
                false,
            ));
        }

        let resources = ResourceRepository::list_by_media_item(&*self.ports, media_item_id).await?;
        let mut items = Vec::with_capacity(resources.len());
        for resource in resources {
            items.push(self.to_summary(resource).await?);
        }
        Ok(ResourceListDto {
            schema_version: SCHEMA_VERSION,
            items,
        })
    }

    async fn to_summary(&self, resource: Resource) -> Result<ResourceSummaryDto, AppError> {
        let is_local = matches!(resource.locator, ResourceLocator::LocalPath { .. });
        let storage = match resource.storage_location_id {
            Some(id) => StorageLocationRepository::get(&*self.ports, id).await?,
            None => None,
        };
        let requires_reauthorization = storage
            .as_ref()
            .is_some_and(|location| location.status == StorageStatus::AuthExpired);
        Ok(ResourceSummaryDto {
            resource_id: resource.id.to_string(),
            resource_type: resource_type_dto(resource.resource_type),
            availability: availability_dto(resource.availability),
            mime_type: resource.mime_type,
            size: resource.size,
            storage_display_name: storage.map(|location| location.display_name),
            // Source entities are not yet part of the v0.1 repository surface. Keeping this
            // null is safer than leaking source IDs or remote URL hints into IPC.
            source_display_name: None,
            is_offline: resource.availability == Availability::OfflineAvailable,
            is_local,
            requires_reauthorization,
            // 契约 §36.4：streamKind 仅 remote_stream 资源非 null；
            // V2-B 引入该资源形态与受控代理 URI 前恒为 null。
            stream_kind: None,
        })
    }
}

fn resource_type_dto(value: ResourceType) -> ResourceTypeDto {
    match value {
        ResourceType::LocalFile => ResourceTypeDto::LocalFile,
        ResourceType::CloudFile => ResourceTypeDto::CloudFile,
        ResourceType::HttpFile => ResourceTypeDto::HttpFile,
        ResourceType::VideoStream => ResourceTypeDto::VideoStream,
        ResourceType::HlsStream => ResourceTypeDto::HlsStream,
        ResourceType::DashStream => ResourceTypeDto::DashStream,
        ResourceType::PublicationFile => ResourceTypeDto::PublicationFile,
        ResourceType::ComicArchive => ResourceTypeDto::ComicArchive,
        ResourceType::ImageSequence => ResourceTypeDto::ImageSequence,
        ResourceType::ArticleSnapshot => ResourceTypeDto::ArticleSnapshot,
        ResourceType::RemoteChapter => ResourceTypeDto::RemoteChapter,
        ResourceType::RemotePageSet => ResourceTypeDto::RemotePageSet,
    }
}

fn availability_dto(value: Availability) -> AvailabilityDto {
    match value {
        Availability::Available => AvailabilityDto::Available,
        Availability::OfflineAvailable => AvailabilityDto::OfflineAvailable,
        Availability::TemporarilyUnavailable => AvailabilityDto::TemporarilyUnavailable,
        Availability::SourceUnavailable => AvailabilityDto::SourceUnavailable,
        Availability::StorageUnavailable => AvailabilityDto::StorageUnavailable,
        Availability::Missing => AvailabilityDto::Missing,
        Availability::Unknown => AvailabilityDto::Unknown,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use haven_domain::contracts::{
        EditionRepository, MediaItemRepository, ResourceRepository, StorageLocationRepository,
        WorkRepository,
    };
    use haven_domain::entities::{Edition, MediaIndex, MediaItem, Resource, StorageLocation, Work};
    use haven_domain::enums::{
        AvailabilitySource, MediaItemStatus, MediaType, ResourceType, StorageProviderType,
        StorageStatus, WorkStatus, WorkType,
    };
    use haven_domain::ids::{EditionId, WorkId};
    use haven_infrastructure::Db;
    use haven_infrastructure::db::repos::SqliteRepositories;

    #[tokio::test]
    async fn list_returns_safe_summary_without_locator() {
        let db = std::sync::Arc::new(Db::open_in_memory().unwrap());
        let repos = std::sync::Arc::new(SqliteRepositories::new(db));
        let now = haven_common::UtcMillis(1);
        let work_id = WorkId::new();
        let edition_id = EditionId::new();
        let media_item_id = MediaItemId::new();
        let storage_id = haven_domain::ids::StorageLocationId::new();
        repos
            .work
            .save(&Work {
                id: work_id,
                canonical_title: "资源作品".into(),
                original_title: None,
                sort_title: None,
                description: None,
                work_type: WorkType::Standalone,
                release_year: None,
                language: None,
                director: None,
                actor: None,
                status: WorkStatus::Completed,
                rating_value: None,
                rating_scale: None,
                artwork: Default::default(),
                created_at: now,
                updated_at: now,
            })
            .await
            .unwrap();
        repos
            .edition
            .save(&Edition {
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
                created_at: now,
                updated_at: now,
            })
            .await
            .unwrap();
        repos
            .media_item
            .save(&MediaItem {
                id: media_item_id,
                edition_id,
                parent_id: None,
                media_type: MediaType::Movie,
                title: "影片".into(),
                index: MediaIndex::Movie,
                duration_ms: None,
                page_count: None,
                chapter_count: None,
                published_at: None,
                status: MediaItemStatus::Available,
                created_at: now,
                updated_at: now,
            })
            .await
            .unwrap();
        repos
            .storage_location
            .save(&StorageLocation {
                id: storage_id,
                provider_type: StorageProviderType::Local,
                display_name: "电影库".into(),
                root_ref: "C:\\private\\root".into(),
                credential_ref: None,
                status: StorageStatus::Connected,
                created_at: now,
                updated_at: now,
            })
            .await
            .unwrap();
        repos
            .resource
            .save(&Resource {
                id: haven_domain::ids::ResourceId::new(),
                media_item_id,
                resource_type: ResourceType::LocalFile,
                source_id: None,
                storage_location_id: Some(storage_id),
                locator: ResourceLocator::LocalPath {
                    path: "C:\\private\\root\\movie.mkv".into(),
                },
                mime_type: Some("video/x-matroska".into()),
                size: Some(42),
                hash: None,
                availability: Availability::Available,
                availability_source: AvailabilitySource::User,
                modified_ms: None,
                fingerprint_first: None,
                fingerprint_last: None,
                created_at: now,
                updated_at: now,
            })
            .await
            .unwrap();

        let result = ResourceService::new(repos)
            .list_by_media_item(ResourceListByMediaItemRequest {
                media_item_id: media_item_id.to_string(),
            })
            .await
            .unwrap();
        assert_eq!(result.schema_version, 1);
        assert_eq!(result.items.len(), 1);
        assert_eq!(
            result.items[0].storage_display_name.as_deref(),
            Some("电影库")
        );
        assert!(result.items[0].is_local);
        let json = serde_json::to_string(&result).unwrap();
        assert!(!json.contains("private"));
        assert!(!json.contains("locator"));
        assert!(!json.contains("movie.mkv"));
    }
}
