use std::sync::Arc;

use haven_common::network::{HttpUrlPolicy, parse_http_url};
use haven_common::{AppError, ErrorKind};
use haven_domain::contracts::{MediaItemRepository, ResourceRepository, StorageLocationRepository};
use haven_domain::entities::{Resource, ResourceLocator};
use haven_domain::enums::{Availability, MediaType, ResourceType, StorageStatus};
use haven_domain::ids::MediaItemId;

use crate::services::ports::ResourceListPorts;
use crate::services::source_import::{
    remote_source_mime_compatible, source_key_for_id, validate_remote_source_object,
};
use crate::wire::{
    AvailabilityDto, ResourceListByMediaItemRequest, ResourceListDto, ResourceSummaryDto,
    ResourceTypeDto, StreamKindDto,
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

        let media_item = if let Some(media_item) =
            MediaItemRepository::get(&*self.ports, media_item_id).await?
        {
            media_item
        } else {
            return Err(AppError::new(
                "MEDIA_ITEM_NOT_FOUND",
                ErrorKind::NotFound,
                "媒体条目不存在",
                false,
            ));
        };

        let resources = ResourceRepository::list_by_media_item(&*self.ports, media_item_id).await?;
        let mut items = Vec::with_capacity(resources.len());
        for resource in resources {
            items.push(self.to_summary(resource, media_item.media_type).await?);
        }
        Ok(ResourceListDto {
            schema_version: SCHEMA_VERSION,
            items,
        })
    }

    async fn to_summary(
        &self,
        resource: Resource,
        media_type: MediaType,
    ) -> Result<ResourceSummaryDto, AppError> {
        // `StorageObject` is the canonical locator written by the scanner and
        // by the download worker.  Treat it as local only when its embedded
        // provider identity agrees with the Resource row; otherwise a stale
        // or tampered locator must not project local capabilities.
        let is_local = match &resource.locator {
            ResourceLocator::LocalPath { .. } => true,
            ResourceLocator::StorageObject { provider_id, .. } => {
                resource.storage_location_id == Some(*provider_id)
            }
            ResourceLocator::Http { .. } | ResourceLocator::SourceObject { .. } => false,
        };
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
        let persisted_offline = resource.availability == Availability::OfflineAvailable;
        // OfflineAvailable is a persisted claim, not proof that the file is
        // still present.  A missing local object is projected as a cache miss
        // so the detail page can offer a real retry instead of a dead
        // "already downloaded" state.  Remote SourceObjects keep their remote
        // capability projection; an OfflineAvailable row is only considered
        // offline when it is backed by a bound local locator.
        let offline_file_missing =
            persisted_offline && is_local && !local_locator_exists(&resource, storage.as_ref());
        // Only a local/storage-backed locator can prove that an offline copy
        // exists.  A tampered or stale remote row marked OfflineAvailable must
        // remain downloadable/online-readable according to its remote source,
        // but must not be projected as an already downloaded local item.
        let is_offline = persisted_offline && is_local && !offline_file_missing;
        let can_download = !is_offline
            && is_available
            && !requires_reauthorization
            && downloadable_locator(&resource, storage.as_ref());
        let can_online_read = is_available
            && !offline_file_missing
            && online_readable_locator(&resource, media_type, storage.as_ref());
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
            stream_kind: stream_kind_dto(resource.resource_type),
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
                || !remote_source_mime_compatible(
                    source_key,
                    resource.resource_type,
                    resource.mime_type.as_deref(),
                )
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
            let provider_matches = match &resource.locator {
                ResourceLocator::StorageObject { provider_id, .. } => {
                    resource.storage_location_id == Some(*provider_id)
                }
                ResourceLocator::LocalPath { .. } => true,
                _ => false,
            };
            provider_matches
                && storage.is_some_and(|location| {
                    location.provider_type == haven_domain::enums::StorageProviderType::Local
                        && matches!(
                            location.status,
                            StorageStatus::Connected | StorageStatus::ReadOnly
                        )
                })
                && !is_stream_resource(resource.resource_type)
        }
        // HTTP resources are intentionally not downloadable through the generic
        // resource capability.  Stream resources are only playable through the
        // existing controlled stream/session path; allowing them here must not
        // accidentally turn an arbitrary HTTP file into a download target.
        ResourceLocator::Http { .. } => false,
    }
}

/// 计算在线打开能力。远端正文只开放已经接入 Remote Session 的固定来源；
/// Gutenberg EPUB 与其它远端正文一样，只有 MIME 和资源类型同时匹配时才开放。
fn online_readable_locator(
    resource: &Resource,
    media_type: MediaType,
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
            let Some(engine) = engine_for_media_type(media_type) else {
                return false;
            };
            crate::services::session::resource_type_compatible(engine, resource)
                && crate::services::session::remote_session_compatible(
                    resource.resource_type,
                    resource.mime_type.as_deref(),
                    source_key,
                )
        }
        ResourceLocator::LocalPath { .. } | ResourceLocator::StorageObject { .. } => {
            let Some(engine) = engine_for_media_type(media_type) else {
                return false;
            };
            let provider_matches = match &resource.locator {
                ResourceLocator::StorageObject { provider_id, .. } => {
                    resource.storage_location_id == Some(*provider_id)
                }
                ResourceLocator::LocalPath { .. } => true,
                _ => false,
            };
            provider_matches
                && storage.is_some_and(|location| {
                    location.provider_type == haven_domain::enums::StorageProviderType::Local
                        && matches!(
                            location.status,
                            StorageStatus::Connected | StorageStatus::ReadOnly
                        )
                })
                && crate::services::session::resource_type_compatible(engine, resource)
        }
        ResourceLocator::Http { url } => {
            media_type_is_playback(media_type)
                && http_stream_online_readable(resource.resource_type, url)
        }
    }
}

/// HTTP-backed video streams are playable through the controlled stream
/// session, even though they are not ordinary downloadable files.  Keep this
/// capability projection deliberately narrow: only the implemented HTTP video
/// and HLS resource kinds may opt in. DASH remains an explicit domain enum but
/// is unavailable until a complete player path is implemented. The URL must
/// also pass the shared outbound HTTP URL policy.
/// The URL itself never crosses the resource-summary wire boundary.
pub(crate) fn http_stream_online_readable(resource_type: ResourceType, raw_url: &str) -> bool {
    matches!(
        resource_type,
        ResourceType::VideoStream | ResourceType::HlsStream
    ) && parse_http_url(raw_url, HttpUrlPolicy::MediaResource).is_ok()
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

/// Check whether a persisted offline locator still resolves to a file (or an
/// image-sequence directory) inside its registered local storage root.  This
/// is intentionally a local boolean projection; it never returns the path to
/// the frontend and does not change the persisted availability row.
fn local_locator_exists(
    resource: &Resource,
    storage: Option<&haven_domain::entities::StorageLocation>,
) -> bool {
    let Some(storage) = storage else {
        return false;
    };
    if storage.provider_type != haven_domain::enums::StorageProviderType::Local
        || !matches!(
            storage.status,
            StorageStatus::Connected | StorageStatus::ReadOnly
        )
    {
        return false;
    }
    let Ok(root) = std::fs::canonicalize(&storage.root_ref) else {
        return false;
    };
    if !root.is_dir() {
        return false;
    }
    let raw = match &resource.locator {
        ResourceLocator::LocalPath { path } => {
            if std::path::Path::new(path).is_absolute() {
                std::path::PathBuf::from(path)
            } else {
                root.join(path)
            }
        }
        ResourceLocator::StorageObject {
            provider_id,
            object_id,
            path_hint,
        } if resource.storage_location_id == Some(*provider_id) => {
            let relative = path_hint.as_deref().unwrap_or(object_id.as_str());
            let path = std::path::Path::new(relative);
            if path.is_absolute()
                || relative.contains(['\\', '\0'])
                || relative
                    .split('/')
                    .any(|part| part.is_empty() || part == "." || part == "..")
            {
                return false;
            }
            root.join(path)
        }
        _ => return false,
    };
    let Ok(canonical) = std::fs::canonicalize(raw) else {
        return false;
    };
    if canonical == root || canonical.strip_prefix(&root).is_err() {
        return false;
    }
    if resource.resource_type == ResourceType::ImageSequence {
        canonical.is_dir()
    } else {
        canonical.is_file()
    }
}

fn engine_for_media_type(media_type: MediaType) -> Option<crate::wire::SessionEngineDto> {
    match media_type {
        MediaType::Movie | MediaType::Series | MediaType::Episode | MediaType::Audio => {
            Some(crate::wire::SessionEngineDto::Playback)
        }
        MediaType::Book | MediaType::Document => Some(crate::wire::SessionEngineDto::Reader),
        MediaType::Comic => Some(crate::wire::SessionEngineDto::Comic),
        MediaType::Article => Some(crate::wire::SessionEngineDto::Article),
        MediaType::Unknown => None,
    }
}

fn media_type_is_playback(media_type: MediaType) -> bool {
    matches!(
        media_type,
        MediaType::Movie | MediaType::Series | MediaType::Episode | MediaType::Audio
    )
}

/// Project the only stream distinction that the player needs.  Keep this
/// derived from the persisted resource type rather than from a MIME string or
/// URL so the frontend cannot accidentally select a transport based on an
/// untrusted locator.  DASH remains unsupported until it has a complete
/// controlled playback path.
fn stream_kind_dto(resource_type: ResourceType) -> Option<StreamKindDto> {
    match resource_type {
        ResourceType::HlsStream => Some(StreamKindDto::Hls),
        ResourceType::VideoStream => Some(StreamKindDto::Direct),
        _ => None,
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
                // A remote locator cannot prove that an offline copy exists;
                // the projection must not treat this tampered/stale state as
                // a local download.
                availability: Availability::OfflineAvailable,
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
        assert!(!summary.is_offline);
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
            resource_type: ResourceType::PublicationFile,
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

        assert!(!online_readable_locator(&resource, MediaType::Book, None));
        assert!(!online_readable_locator(
            &resource,
            MediaType::Book,
            Some(&storage)
        ));

        storage.status = StorageStatus::ReadOnly;
        assert!(online_readable_locator(
            &resource,
            MediaType::Book,
            Some(&storage)
        ));
    }

    #[test]
    fn http_video_streams_are_online_readable_but_not_generic_files() {
        for resource_type in [ResourceType::VideoStream, ResourceType::HlsStream] {
            assert!(http_stream_online_readable(
                resource_type,
                "https://stream.example.test/video/index.m3u8?token=opaque"
            ));
            assert!(http_stream_online_readable(
                resource_type,
                "http://stream.example.test/video/file.mp4"
            ));
            assert!(http_stream_online_readable(
                resource_type,
                "https://stream.example.test:8443/video/index.m3u8"
            ));
            assert!(http_stream_online_readable(
                resource_type,
                "https://[2001:4860:4860::8888]:8443/video/index.m3u8"
            ));
        }

        assert!(!http_stream_online_readable(
            ResourceType::DashStream,
            "https://stream.example.test/video/manifest.mpd"
        ));

        assert!(!http_stream_online_readable(
            ResourceType::HttpFile,
            "https://stream.example.test/video/file.mp4"
        ));
        assert!(!http_stream_online_readable(
            ResourceType::PublicationFile,
            "https://stream.example.test/video/file.mp4"
        ));
    }

    #[test]
    fn http_stream_capability_rejects_malformed_or_ambiguous_urls() {
        for raw_url in [
            "",
            "ftp://stream.example.test/video.m3u8",
            "file:///tmp/video.m3u8",
            "https:///video.m3u8",
            "https://",
            "https://stream.example.test: /video.m3u8",
            "https://stream.example.test:99999/video.m3u8",
            "https://stream.example.test:/video.m3u8",
            "https://[2001:db8::10/video.m3u8",
            "https://2001:db8::10/video.m3u8",
            "https://user:secret@stream.example.test/video.m3u8",
            "https://stream.example.test/video\n.m3u8",
            "https://stream.example.test/video.m3u8 extra",
        ] {
            assert!(
                !http_stream_online_readable(ResourceType::HlsStream, raw_url),
                "malformed URL must not receive online stream capability: {raw_url:?}"
            );
        }
    }

    #[test]
    fn stream_kind_projection_matches_the_real_stream_resource_type() {
        assert_eq!(
            stream_kind_dto(ResourceType::HlsStream),
            Some(StreamKindDto::Hls)
        );
        assert_eq!(
            stream_kind_dto(ResourceType::VideoStream),
            Some(StreamKindDto::Direct)
        );
        for resource_type in [
            ResourceType::LocalFile,
            ResourceType::PublicationFile,
            ResourceType::ComicArchive,
            ResourceType::ArticleSnapshot,
            ResourceType::DashStream,
        ] {
            assert_eq!(stream_kind_dto(resource_type), None);
        }
    }

    #[test]
    fn remote_gutenberg_epub_projects_download_and_online_capabilities() {
        let source_id = crate::services::source_import::stable_source_id("opds_gutenberg").unwrap();
        let resource = Resource {
            id: haven_domain::ids::ResourceId::new(),
            media_item_id: MediaItemId::new(),
            resource_type: ResourceType::PublicationFile,
            source_id: Some(source_id),
            storage_location_id: None,
            locator: ResourceLocator::SourceObject {
                source_id,
                remote_id: "https://www.gutenberg.org/ebooks/84.opds".into(),
            },
            mime_type: Some("application/epub+zip".into()),
            size: None,
            hash: None,
            availability: Availability::Available,
            availability_source: AvailabilitySource::User,
            modified_ms: None,
            fingerprint_first: None,
            fingerprint_last: None,
            created_at: haven_common::UtcMillis(1),
            updated_at: haven_common::UtcMillis(1),
        };
        assert!(downloadable_locator(&resource, None));
        assert!(online_readable_locator(&resource, MediaType::Book, None));

        let mut wrong_mime = resource.clone();
        wrong_mime.mime_type = Some("application/pdf".into());
        assert!(!downloadable_locator(&wrong_mime, None));
        assert!(!online_readable_locator(&wrong_mime, MediaType::Book, None));
    }

    #[test]
    fn remote_download_capability_rejects_missing_or_cross_provider_mime() {
        assert!(remote_source_mime_compatible(
            "mangadex",
            ResourceType::ComicArchive,
            Some("application/vnd.comicbook+zip; charset=binary"),
        ));
        assert!(remote_source_mime_compatible(
            "europepmc",
            ResourceType::ArticleSnapshot,
            Some("text/html; charset=utf-8"),
        ));
        assert!(!remote_source_mime_compatible(
            "mangadex",
            ResourceType::ComicArchive,
            Some("application/pdf"),
        ));
        assert!(!remote_source_mime_compatible(
            "arxiv",
            ResourceType::PublicationFile,
            None,
        ));
        assert!(!remote_source_mime_compatible(
            "opds_gutenberg",
            ResourceType::PublicationFile,
            Some("application/pdf"),
        ));
    }

    #[test]
    fn offline_projection_requires_a_file_inside_the_registered_root() {
        let root = tempfile::tempdir().unwrap();
        let storage = StorageLocation {
            id: haven_domain::ids::StorageLocationId::new(),
            provider_type: StorageProviderType::Local,
            display_name: "测试本地库".into(),
            root_ref: root.path().to_string_lossy().into_owned(),
            credential_ref: None,
            status: StorageStatus::Connected,
            created_at: haven_common::UtcMillis(1),
            updated_at: haven_common::UtcMillis(1),
        };
        let resource = Resource {
            id: haven_domain::ids::ResourceId::new(),
            media_item_id: MediaItemId::new(),
            resource_type: ResourceType::PublicationFile,
            source_id: None,
            storage_location_id: Some(storage.id),
            locator: ResourceLocator::LocalPath {
                path: "missing.epub".into(),
            },
            mime_type: Some("application/epub+zip".into()),
            size: None,
            hash: None,
            availability: Availability::OfflineAvailable,
            availability_source: AvailabilitySource::User,
            modified_ms: None,
            fingerprint_first: None,
            fingerprint_last: None,
            created_at: haven_common::UtcMillis(1),
            updated_at: haven_common::UtcMillis(1),
        };
        assert!(!local_locator_exists(&resource, Some(&storage)));

        std::fs::write(root.path().join("missing.epub"), b"not a real epub").unwrap();
        assert!(local_locator_exists(&resource, Some(&storage)));
    }
}
