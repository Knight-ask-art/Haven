use std::sync::Arc;

use haven_common::{AppError, ErrorKind};
use haven_domain::contracts::{MediaItemRepository, ResourceRepository, StorageLocationRepository};
use haven_domain::entities::{Resource, ResourceLocator};
use haven_domain::enums::{Availability, ResourceType, StorageStatus};
use haven_domain::ids::MediaItemId;

use crate::services::ports::ResourceListPorts;
use crate::services::source_import::{source_key_for_id, validate_remote_source_object};
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
        let is_available = matches!(
            resource.availability,
            Availability::Available | Availability::OfflineAvailable
        );
        let is_offline = resource.availability == Availability::OfflineAvailable;
        let can_download = !is_offline
            && is_available
            && !requires_reauthorization
            && downloadable_locator(&resource, storage.as_ref());
        let can_online_read = is_available && online_readable_locator(&resource, storage.as_ref());
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
            is_offline,
            is_local,
            requires_reauthorization,
            can_download,
            can_online_read,
            // 契约 §36.4：streamKind 仅 remote_stream 资源非 null；
            // V2-B 引入该资源形态与受控代理 URI 前恒为 null。
            stream_kind: None,
        })
    }
}

/// 计算下载能力的唯一后端规则。只有登记过的本地存储对象或固定来源
/// `SourceObject` 可以进入 DownloadTask；Http/播放流永远不能被当作正文下载。
fn downloadable_locator(
    resource: &Resource,
    storage: Option<&haven_domain::entities::StorageLocation>,
) -> bool {
    match &resource.locator {
        ResourceLocator::SourceObject { source_id, .. } => {
            let Some(source_key) = source_key_for_id(*source_id) else {
                return false;
            };
            let ResourceLocator::SourceObject { remote_id, .. } = &resource.locator else {
                return false;
            };
            if resource.source_id != Some(*source_id)
                || validate_remote_source_object(source_key, resource.resource_type, remote_id)
                    .is_err()
            {
                return false;
            }
            matches!(
                (source_key, resource.resource_type),
                ("mangadex", ResourceType::ComicArchive)
                    | ("arxiv", ResourceType::PublicationFile)
                    | ("europepmc", ResourceType::ArticleSnapshot)
                    | ("wikisource", ResourceType::ArticleSnapshot)
                    | ("opds_gutenberg", ResourceType::PublicationFile)
            )
        }
        ResourceLocator::LocalPath { .. } | ResourceLocator::StorageObject { .. } => {
            storage.is_some_and(|location| {
                location.provider_type == haven_domain::enums::StorageProviderType::Local
                    && matches!(
                        location.status,
                        StorageStatus::Connected | StorageStatus::ReadOnly
                    )
            }) && !is_stream_resource(resource.resource_type)
        }
        ResourceLocator::Http { .. } => false,
    }
}

/// 计算在线打开能力。远端正文只开放已经接入 Remote Session 的固定来源；
/// OPDS/Gutenberg 的 EPUB 首轮仍明确要求下载后阅读。
fn online_readable_locator(
    resource: &Resource,
    storage: Option<&haven_domain::entities::StorageLocation>,
) -> bool {
    match &resource.locator {
        ResourceLocator::SourceObject { source_id, .. } => {
            let Some(source_key) = source_key_for_id(*source_id) else {
                return false;
            };
            let ResourceLocator::SourceObject { remote_id, .. } = &resource.locator else {
                return false;
            };
            if resource.source_id != Some(*source_id)
                || validate_remote_source_object(source_key, resource.resource_type, remote_id)
                    .is_err()
            {
                return false;
            }
            matches!(
                (source_key, resource.resource_type),
                ("mangadex", ResourceType::ComicArchive)
                    | ("arxiv", ResourceType::PublicationFile)
                    | ("europepmc", ResourceType::ArticleSnapshot)
                    | ("wikisource", ResourceType::ArticleSnapshot)
            )
        }
        ResourceLocator::LocalPath { .. } | ResourceLocator::StorageObject { .. } => {
            storage.is_some_and(|location| {
                location.provider_type == haven_domain::enums::StorageProviderType::Local
                    && matches!(
                        location.status,
                        StorageStatus::Connected | StorageStatus::ReadOnly
                    )
            }) && !is_stream_resource(resource.resource_type)
        }
        ResourceLocator::Http { .. } => false,
    }
}

fn is_stream_resource(resource_type: ResourceType) -> bool {
    matches!(
        resource_type,
        ResourceType::VideoStream
            | ResourceType::HlsStream
            | ResourceType::DashStream
            | ResourceType::RemoteChapter
            | ResourceType::RemotePageSet
    )
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
        assert!(result.items[0].can_download);
        assert!(result.items[0].can_online_read);
        let json = serde_json::to_string(&result).unwrap();
        assert!(!json.contains("private"));
        assert!(!json.contains("locator"));
        assert!(!json.contains("movie.mkv"));
    }

    #[tokio::test]
    async fn remote_mangadex_resource_projects_download_and_online_capabilities() {
        let db = std::sync::Arc::new(Db::open_in_memory().unwrap());
        let repos = std::sync::Arc::new(SqliteRepositories::new(db));
        let now = haven_common::UtcMillis(1);
        let work_id = WorkId::new();
        let edition_id = EditionId::new();
        let media_item_id = MediaItemId::new();
        repos
            .work
            .save(&Work {
                id: work_id,
                canonical_title: "火影忍者".into(),
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
                title: "火影忍者".into(),
                subtitle: None,
                edition_type: MediaType::Comic,
                release_date: None,
                language: Some("zh".into()),
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
                media_type: MediaType::Comic,
                title: "第 1 章".into(),
                index: MediaIndex::Chapter {
                    volume: None,
                    chapter: 1.0,
                },
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

        let source_id = crate::services::source_import::stable_source_id("mangadex").unwrap();
        repos
            .resource
            .save(&Resource {
                id: haven_domain::ids::ResourceId::new(),
                media_item_id,
                resource_type: ResourceType::ComicArchive,
                source_id: Some(source_id),
                storage_location_id: None,
                locator: ResourceLocator::SourceObject {
                    source_id,
                    remote_id:
                        "aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa:bbbbbbbb-bbbb-4bbb-8bbb-bbbbbbbbbbbb"
                            .into(),
                },
                mime_type: Some("application/vnd.comicbook+zip".into()),
                size: None,
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
        assert_eq!(result.items.len(), 1);
        let summary = &result.items[0];
        assert!(!summary.is_local);
        assert!(summary.can_download, "远端 MangaDex 资源必须可下载");
        assert!(summary.can_online_read, "远端 MangaDex 资源必须可在线阅读");
        let json = serde_json::to_string(summary).unwrap();
        assert!(!json.contains("mangadex"));
        assert!(!json.contains("aaaaaaaa"));
    }

    #[test]
    fn local_online_read_requires_a_usable_registered_storage() {
        let storage_id = haven_domain::ids::StorageLocationId::new();
        let resource = Resource {
            id: haven_domain::ids::ResourceId::new(),
            media_item_id: MediaItemId::new(),
            resource_type: ResourceType::LocalFile,
            source_id: None,
            storage_location_id: Some(storage_id),
            locator: ResourceLocator::LocalPath {
                path: "books/example.epub".into(),
            },
            mime_type: Some("application/epub+zip".into()),
            size: Some(1),
            hash: None,
            availability: Availability::Available,
            availability_source: AvailabilitySource::User,
            modified_ms: None,
            fingerprint_first: None,
            fingerprint_last: None,
            created_at: haven_common::UtcMillis(1),
            updated_at: haven_common::UtcMillis(1),
        };
        let mut storage = StorageLocation {
            id: storage_id,
            provider_type: StorageProviderType::Local,
            display_name: "本地库".into(),
            root_ref: "C:\\library".into(),
            credential_ref: None,
            status: StorageStatus::Disconnected,
            created_at: haven_common::UtcMillis(1),
            updated_at: haven_common::UtcMillis(1),
        };

        assert!(!online_readable_locator(&resource, None));
        assert!(!online_readable_locator(&resource, Some(&storage)));

        storage.status = StorageStatus::ReadOnly;
        assert!(online_readable_locator(&resource, Some(&storage)));
    }
}
