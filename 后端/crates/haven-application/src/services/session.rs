//! Session open preparation (Phase A).
//!
//! This service resolves a media item to a checked, local file.  It deliberately
//! does not create History and does not know about the Tauri session registry.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use haven_common::{AppError, ErrorKind};
use haven_domain::contracts::{
    EditionRepository, MediaItemRepository, ProgressRepository, ResourceRepository,
    StorageLocationRepository, WorkRepository,
};
use haven_domain::entities::{Resource, ResourceLocator};
use haven_domain::enums::{
    Availability, MediaType, ResourceType, StorageProviderType, StorageStatus,
};
use haven_domain::ids::{MediaItemId, ResourceId, StorageLocationId};

use crate::mapper::progress::progress_summary;
use crate::services::comic::{ComicPageService, PreparedComicPage};
use crate::services::ports::SessionOpenPorts;
use crate::wire::{ProgressSummaryDto, SessionEngineDto, SessionOpenRequest};

/// A prepared session is server-only state.  In particular, its paths and
/// resource/storage IDs must never be serialized as IPC fields.
#[derive(Debug, Clone)]
pub struct PreparedSession {
    pub work_id: String,
    pub edition_id: String,
    pub media_item_id: String,
    pub engine: SessionEngineDto,
    pub resource_id: ResourceId,
    pub storage_location_id: StorageLocationId,
    pub canonical_root: PathBuf,
    pub canonical_file: PathBuf,
    pub mime_type: Option<String>,
    pub media_type: MediaType,
    pub resource_type: ResourceType,
    pub comic_pages: Option<Vec<PreparedComicPage>>,
    pub progress: Option<ProgressSummaryDto>,
}

#[derive(Clone)]
pub struct SessionService {
    ports: Arc<dyn SessionOpenPorts>,
    comic_pages: ComicPageService,
}

impl SessionService {
    pub fn new(ports: Arc<dyn SessionOpenPorts>, comic_pages: ComicPageService) -> Self {
        Self { ports, comic_pages }
    }

    /// Validate the hierarchy, engine, storage policy and file containment,
    /// then return server-only facts for registry registration.
    pub async fn prepare(&self, request: SessionOpenRequest) -> Result<PreparedSession, AppError> {
        let media_item_id: MediaItemId = request.media_item_id.parse().map_err(|_| {
            AppError::new(
                "INVALID_ID",
                ErrorKind::Validation,
                "无效的媒体条目 ID",
                false,
            )
        })?;

        let media_item = MediaItemRepository::get(&*self.ports, media_item_id)
            .await?
            .ok_or_else(media_item_not_found)?;
        if !engine_compatible(request.engine, media_item.media_type) {
            return Err(AppError::new(
                "FORMAT_UNSUPPORTED",
                ErrorKind::Unsupported,
                "当前媒介格式不支持该引擎",
                false,
            ));
        }

        let edition = EditionRepository::get(&*self.ports, media_item.edition_id)
            .await?
            .ok_or_else(edition_not_found)?;
        let work = WorkRepository::get(&*self.ports, edition.work_id)
            .await?
            .ok_or_else(work_not_found)?;

        let resources = ResourceRepository::list_by_media_item(&*self.ports, media_item_id).await?;
        let mut candidates: Vec<Resource> = resources
            .into_iter()
            .filter(|resource| {
                matches!(
                    resource.availability,
                    Availability::Available | Availability::OfflineAvailable
                ) && resource_type_compatible(request.engine, resource.resource_type)
            })
            .collect();
        candidates.sort_by_key(|resource| {
            (
                if resource.availability == Availability::OfflineAvailable {
                    0u8
                } else {
                    1u8
                },
                resource.id.to_string(),
            )
        });
        if candidates.is_empty() {
            return Err(resource_not_found());
        }
        let mut rejected = None;
        let mut resolved = Vec::new();
        for resource in candidates {
            match self.resolve_local_resource(&resource).await? {
                CandidateResolution::Eligible {
                    storage_location_id,
                    canonical_root,
                    canonical_file,
                } => {
                    resolved.push((
                        resource,
                        storage_location_id,
                        canonical_root,
                        canonical_file,
                    ));
                }
                CandidateResolution::Skipped => {
                    // 远端流等非本地候选：静默跳过，不污染 rejected 语义。
                }
                CandidateResolution::Rejected(error) => {
                    rejected.get_or_insert(error);
                }
            }
        }
        resolved.sort_by_key(|(resource, ..)| {
            (
                if resource.availability == Availability::OfflineAvailable {
                    0u8
                } else {
                    1u8
                },
                resource.id.to_string(),
            )
        });
        let Some((resource, storage_location_id, canonical_root, canonical_file)) =
            resolved.into_iter().next()
        else {
            // 全部 Skipped = 本地无候选（远端流条目）→ RESOURCE_NOT_FOUND，
            // 前端据此回退受控流会话；有 rejected 才保留原错误语义。
            return Err(rejected.unwrap_or_else(resource_not_found));
        };
        let progress = ProgressRepository::get_for_media_item(&*self.ports, media_item_id)
            .await?
            .as_ref()
            .map(progress_summary)
            .transpose()?;

        let mut prepared = PreparedSession {
            work_id: work.id.to_string(),
            edition_id: edition.id.to_string(),
            media_item_id: media_item.id.to_string(),
            engine: request.engine,
            resource_id: resource.id,
            storage_location_id,
            canonical_root,
            canonical_file,
            mime_type: resource.mime_type,
            media_type: media_item.media_type,
            resource_type: resource.resource_type,
            comic_pages: None,
            progress,
        };
        if prepared.engine == SessionEngineDto::Comic {
            prepared.comic_pages = Some(self.comic_pages.inspect(&prepared)?);
        }
        Ok(prepared)
    }

    async fn resolve_local_resource(
        &self,
        resource: &Resource,
    ) -> Result<CandidateResolution, AppError> {
        // 本地受控存储有两种定位形态：扫描入库的 LocalPath 与来源下载入库的
        // StorageObject（相对路径取 path_hint，退化用对象名）。两者同策略：
        // 规范化后必须仍位于存储根内。Http 等远端定位不属于本地 Session，
        // 交给受控流通道处理。
        let path = match &resource.locator {
            ResourceLocator::LocalPath { path } => path.clone(),
            ResourceLocator::StorageObject {
                object_id,
                path_hint,
                ..
            } => path_hint.clone().unwrap_or_else(|| object_id.clone()),
            _ => return Ok(CandidateResolution::Skipped),
        };
        let Some(storage_location_id) = resource.storage_location_id else {
            return Ok(CandidateResolution::Rejected(resource_unavailable()));
        };
        let Some(storage) =
            StorageLocationRepository::get(&*self.ports, storage_location_id).await?
        else {
            return Ok(CandidateResolution::Rejected(resource_unavailable()));
        };
        if storage.provider_type != StorageProviderType::Local {
            return Ok(CandidateResolution::Rejected(security_denied(
                "资源存储策略不允许由本地 Session 打开",
            )));
        }
        if !matches!(
            storage.status,
            StorageStatus::Connected | StorageStatus::ReadOnly
        ) {
            return Ok(CandidateResolution::Rejected(resource_unavailable()));
        }

        // Canonicalization is intentionally done before containment checking.
        // This makes `..` and symlink escapes fail the same policy check.
        let canonical_root = match std::fs::canonicalize(&storage.root_ref) {
            Ok(root) => root,
            Err(_) => return Ok(CandidateResolution::Rejected(resource_unavailable())),
        };
        if !canonical_root.is_dir() {
            return Ok(CandidateResolution::Rejected(resource_unavailable()));
        }
        let raw_file = if Path::new(&path).is_absolute() {
            PathBuf::from(path)
        } else {
            canonical_root.join(path)
        };
        let canonical_file = match std::fs::canonicalize(&raw_file) {
            Ok(file) => file,
            Err(_) => return Ok(CandidateResolution::Rejected(resource_unavailable())),
        };
        if canonical_file.strip_prefix(&canonical_root).is_err() {
            return Ok(CandidateResolution::Rejected(security_denied(
                "资源路径超出存储根目录",
            )));
        }
        let expects_directory = resource.resource_type == ResourceType::ImageSequence;
        if (expects_directory && !canonical_file.is_dir())
            || (!expects_directory && !canonical_file.is_file())
        {
            return Ok(CandidateResolution::Rejected(resource_unavailable()));
        }
        let readable = if expects_directory {
            std::fs::read_dir(&canonical_file).is_ok()
        } else {
            std::fs::File::open(&canonical_file).is_ok()
        };
        if !readable {
            return Ok(CandidateResolution::Rejected(resource_unavailable()));
        }
        Ok(CandidateResolution::Eligible {
            storage_location_id,
            canonical_root,
            canonical_file,
        })
    }
}

enum CandidateResolution {
    Eligible {
        storage_location_id: StorageLocationId,
        canonical_root: PathBuf,
        canonical_file: PathBuf,
    },
    /// 非本地候选（如远端流 Http 资源）：不属于本地 Session 的管辖，
    /// 也不是策略违规；全部 Skip 时按"本地无候选"返回 RESOURCE_NOT_FOUND，
    /// 由前端回退受控流会话（stream_open，契约 §36.4）。
    Skipped,
    Rejected(AppError),
}

pub(crate) fn engine_compatible(engine: SessionEngineDto, media_type: MediaType) -> bool {
    match engine {
        SessionEngineDto::Playback => matches!(
            media_type,
            MediaType::Movie | MediaType::Series | MediaType::Episode | MediaType::Audio
        ),
        SessionEngineDto::Reader => matches!(media_type, MediaType::Book | MediaType::Document),
        SessionEngineDto::Comic => media_type == MediaType::Comic,
        SessionEngineDto::Article => media_type == MediaType::Article,
    }
}

fn resource_type_compatible(engine: SessionEngineDto, resource_type: ResourceType) -> bool {
    engine != SessionEngineDto::Comic
        || matches!(
            resource_type,
            ResourceType::ComicArchive | ResourceType::ImageSequence
        )
}

fn media_item_not_found() -> AppError {
    AppError::new(
        "MEDIA_ITEM_NOT_FOUND",
        ErrorKind::NotFound,
        "媒体条目不存在",
        false,
    )
}

fn edition_not_found() -> AppError {
    AppError::new(
        "EDITION_NOT_FOUND",
        ErrorKind::NotFound,
        "版本不存在",
        false,
    )
}

fn work_not_found() -> AppError {
    AppError::new("WORK_NOT_FOUND", ErrorKind::NotFound, "作品不存在", false)
}

fn resource_not_found() -> AppError {
    AppError::new(
        "RESOURCE_NOT_FOUND",
        ErrorKind::NotFound,
        "没有可用的本地资源",
        false,
    )
}

fn resource_unavailable() -> AppError {
    AppError::new(
        "RESOURCE_UNAVAILABLE",
        ErrorKind::Storage,
        "本地资源当前不可用",
        false,
    )
}

fn security_denied(message: &'static str) -> AppError {
    AppError::new(
        "SECURITY_POLICY_DENIED",
        ErrorKind::Security,
        message,
        false,
    )
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
        AvailabilitySource, MediaItemStatus, ResourceType, StorageProviderType, WorkStatus,
        WorkType,
    };
    use haven_domain::ids::{EditionId, WorkId};
    use haven_infrastructure::Db;
    use haven_infrastructure::db::repos::SqliteRepositories;
    use tempfile::TempDir;

    #[derive(Debug)]
    struct TestComicPageProvider;

    impl crate::services::comic::ComicPageProvider for TestComicPageProvider {
        fn inspect(
            &self,
            _session: &PreparedSession,
        ) -> Result<Vec<crate::services::comic::PreparedComicPage>, AppError> {
            Ok(Vec::new())
        }

        fn read_page(
            &self,
            _session: &PreparedSession,
            _page: &crate::services::comic::PreparedComicPage,
        ) -> Result<crate::services::comic::ComicPageBody, AppError> {
            Err(AppError::new(
                "FORMAT_UNSUPPORTED",
                ErrorKind::Unsupported,
                "测试桩不提供漫画页面字节",
                false,
            ))
        }
    }

    struct Fixture {
        _root: TempDir,
        service: SessionService,
        repos: Arc<SqliteRepositories>,
        item_id: MediaItemId,
    }

    async fn fixture(path: Option<&Path>) -> Fixture {
        let root = TempDir::new().unwrap();
        let media_root = path
            .map(Path::to_path_buf)
            .unwrap_or_else(|| root.path().to_path_buf());
        let file = media_root.join("movie.mkv");
        std::fs::write(&file, b"video").unwrap();
        let db = Arc::new(Db::open_in_memory().unwrap());
        let repos = Arc::new(SqliteRepositories::new(db));
        let now = haven_common::UtcMillis(1);
        let work = Work {
            id: WorkId::new(),
            canonical_title: "w".into(),
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
        };
        let edition = Edition {
            id: EditionId::new(),
            work_id: work.id,
            title: "e".into(),
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
        };
        let item = MediaItem {
            id: MediaItemId::new(),
            edition_id: edition.id,
            parent_id: None,
            media_type: MediaType::Movie,
            title: "m".into(),
            index: MediaIndex::Movie,
            duration_ms: None,
            page_count: None,
            chapter_count: None,
            published_at: None,
            status: MediaItemStatus::Available,
            created_at: now,
            updated_at: now,
        };
        let storage = StorageLocation {
            id: StorageLocationId::new(),
            provider_type: StorageProviderType::Local,
            display_name: "local".into(),
            root_ref: media_root.to_string_lossy().into_owned(),
            credential_ref: None,
            status: StorageStatus::Connected,
            created_at: now,
            updated_at: now,
        };
        repos.work.save(&work).await.unwrap();
        repos.edition.save(&edition).await.unwrap();
        repos.media_item.save(&item).await.unwrap();
        repos.storage_location.save(&storage).await.unwrap();
        repos
            .resource
            .save(&Resource {
                id: ResourceId::new(),
                media_item_id: item.id,
                resource_type: ResourceType::LocalFile,
                source_id: None,
                storage_location_id: Some(storage.id),
                locator: ResourceLocator::LocalPath {
                    path: "movie.mkv".into(),
                },
                mime_type: Some("video/x-matroska".into()),
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
        Fixture {
            _root: root,
            service: SessionService::new(
                repos.clone(),
                ComicPageService::new(Arc::new(TestComicPageProvider)),
            ),
            repos,
            item_id: item.id,
        }
    }

    #[tokio::test]
    async fn prepares_local_file_without_path_in_wire_json() {
        let f = fixture(None).await;
        let prepared = f
            .service
            .prepare(SessionOpenRequest {
                media_item_id: f.item_id.to_string(),
                engine: SessionEngineDto::Playback,
            })
            .await
            .unwrap();
        let json = serde_json::to_string(&crate::wire::SessionOpenResultDto {
            schema_version: 1,
            session_id: "opaque".into(),
            content_uri: Some("haven-resource://session/opaque".into()),
            work_id: prepared.work_id,
            edition_id: prepared.edition_id,
            media_item_id: prepared.media_item_id,
            engine: prepared.engine,
            progress: prepared.progress,
        })
        .unwrap();
        assert!(!json.contains("movie.mkv"));
        assert!(!json.contains("resourceId"));
    }

    /// 回归（实战验收）：CMS10 导入的条目只有 Http 资源时，session.open 必须返回
    /// RESOURCE_NOT_FOUND（前端据此回退 stream_open），而不是 SECURITY_POLICY_DENIED。
    #[tokio::test]
    async fn http_only_media_item_maps_to_resource_not_found() {
        let f = fixture(None).await;
        // 移除本地资源，只留 Http 流资源。
        let local = f
            .repos
            .resource
            .list_by_media_item(f.item_id)
            .await
            .unwrap();
        for r in local {
            if matches!(r.locator, ResourceLocator::LocalPath { .. }) {
                f.repos.resource.delete(r.id).await.unwrap();
            }
        }
        f.repos
            .resource
            .save(&Resource {
                id: ResourceId::new(),
                media_item_id: f.item_id,
                resource_type: ResourceType::HlsStream,
                source_id: None,
                storage_location_id: None,
                locator: ResourceLocator::Http {
                    url: "https://example.invalid/ep1.m3u8".into(),
                },
                mime_type: Some("application/vnd.apple.mpegurl".into()),
                size: None,
                hash: None,
                availability: Availability::Available,
                availability_source: AvailabilitySource::User,
                modified_ms: None,
                fingerprint_first: None,
                fingerprint_last: None,
                created_at: haven_common::UtcMillis(2),
                updated_at: haven_common::UtcMillis(2),
            })
            .await
            .unwrap();

        let err = f
            .service
            .prepare(SessionOpenRequest {
                media_item_id: f.item_id.to_string(),
                engine: SessionEngineDto::Playback,
            })
            .await
            .unwrap_err();
        assert_eq!(err.code().as_str(), "RESOURCE_NOT_FOUND");
    }

    /// 回归（V2-H1 实战）：OPDS 导入的书以 StorageObject 定位落库（books/<uuid>.epub），
    /// session.open 必须与 LocalPath 同样解析成功，而不是回退流会话报
    /// 「流会话仅支持播放引擎」。
    #[tokio::test]
    async fn storage_object_locator_resolves_like_local_path() {
        let f = fixture(None).await;
        let mut resources = f
            .repos
            .resource
            .list_by_media_item(f.item_id)
            .await
            .unwrap();
        let mut resource = resources.remove(0);
        let storage_id = resource.storage_location_id.expect("fixture 已绑定存储");
        resource.locator = ResourceLocator::StorageObject {
            provider_id: storage_id,
            object_id: "movie.mkv".into(),
            path_hint: Some("movie.mkv".into()),
        };
        f.repos.resource.save(&resource).await.unwrap();

        let prepared = f
            .service
            .prepare(SessionOpenRequest {
                media_item_id: f.item_id.to_string(),
                engine: SessionEngineDto::Playback,
            })
            .await
            .unwrap();
        assert_eq!(prepared.canonical_file.file_name().unwrap(), "movie.mkv");
    }

    /// StorageObject 指向不存在的对象 → 不可用拒绝；不得降级为
    /// RESOURCE_NOT_FOUND（那会诱导前端误走受控流会话）。
    #[tokio::test]
    async fn storage_object_missing_file_is_resource_unavailable() {
        let f = fixture(None).await;
        let mut resources = f
            .repos
            .resource
            .list_by_media_item(f.item_id)
            .await
            .unwrap();
        let mut resource = resources.remove(0);
        let storage_id = resource.storage_location_id.expect("fixture 已绑定存储");
        resource.locator = ResourceLocator::StorageObject {
            provider_id: storage_id,
            object_id: "..\\escape.mkv".into(),
            path_hint: Some("books/gone.epub".into()),
        };
        f.repos.resource.save(&resource).await.unwrap();

        let err = f
            .service
            .prepare(SessionOpenRequest {
                media_item_id: f.item_id.to_string(),
                engine: SessionEngineDto::Playback,
            })
            .await
            .unwrap_err();
        assert_eq!(err.code().as_str(), "RESOURCE_UNAVAILABLE");
    }

    #[tokio::test]
    async fn skips_preferred_non_local_candidate_and_uses_valid_local_file() {
        let f = fixture(None).await;
        f.repos
            .resource
            .save(&Resource {
                id: ResourceId::new(),
                media_item_id: f.item_id,
                resource_type: ResourceType::HttpFile,
                source_id: None,
                storage_location_id: None,
                locator: ResourceLocator::Http {
                    url: "https://example.invalid/video".into(),
                },
                mime_type: Some("video/mp4".into()),
                size: None,
                hash: None,
                availability: Availability::OfflineAvailable,
                availability_source: AvailabilitySource::User,
                modified_ms: None,
                fingerprint_first: None,
                fingerprint_last: None,
                created_at: haven_common::UtcMillis(2),
                updated_at: haven_common::UtcMillis(2),
            })
            .await
            .unwrap();

        let prepared = f
            .service
            .prepare(SessionOpenRequest {
                media_item_id: f.item_id.to_string(),
                engine: SessionEngineDto::Playback,
            })
            .await
            .unwrap();
        assert_eq!(prepared.canonical_file.file_name().unwrap(), "movie.mkv");
    }

    #[tokio::test]
    async fn comic_skips_preferred_wrong_resource_type_and_uses_image_sequence() {
        let f = fixture(None).await;
        let mut item = MediaItemRepository::get(&*f.repos, f.item_id)
            .await
            .unwrap()
            .unwrap();
        item.media_type = MediaType::Comic;
        f.repos.media_item.save(&item).await.unwrap();

        let mut resources = f
            .repos
            .resource
            .list_by_media_item(f.item_id)
            .await
            .unwrap();
        let mut wrong = resources.pop().unwrap();
        wrong.availability = Availability::OfflineAvailable;
        f.repos.resource.save(&wrong).await.unwrap();

        let chapter = f._root.path().join("chapter");
        std::fs::create_dir(&chapter).unwrap();
        std::fs::write(chapter.join("page1.png"), b"image").unwrap();
        f.repos
            .resource
            .save(&Resource {
                id: ResourceId::new(),
                media_item_id: f.item_id,
                resource_type: ResourceType::ImageSequence,
                source_id: None,
                storage_location_id: wrong.storage_location_id,
                locator: ResourceLocator::LocalPath {
                    path: "chapter".into(),
                },
                mime_type: None,
                size: None,
                hash: None,
                availability: Availability::Available,
                availability_source: AvailabilitySource::User,
                modified_ms: None,
                fingerprint_first: None,
                fingerprint_last: None,
                created_at: haven_common::UtcMillis(2),
                updated_at: haven_common::UtcMillis(2),
            })
            .await
            .unwrap();

        let prepared = f
            .service
            .prepare(SessionOpenRequest {
                media_item_id: f.item_id.to_string(),
                engine: SessionEngineDto::Comic,
            })
            .await
            .unwrap();
        assert_eq!(prepared.resource_type, ResourceType::ImageSequence);
        assert_eq!(
            prepared.canonical_file,
            std::fs::canonicalize(chapter).unwrap()
        );
    }

    #[tokio::test]
    async fn wrong_engine_is_format_unsupported() {
        let f = fixture(None).await;
        let err = f
            .service
            .prepare(SessionOpenRequest {
                media_item_id: f.item_id.to_string(),
                engine: SessionEngineDto::Reader,
            })
            .await
            .unwrap_err();
        assert_eq!(err.code().as_str(), "FORMAT_UNSUPPORTED");
    }

    #[tokio::test]
    async fn outside_file_is_security_denied() {
        let outside = TempDir::new().unwrap();
        let outside_file = outside.path().join("outside.mkv");
        std::fs::write(&outside_file, b"outside").unwrap();
        let f = fixture(None).await;
        let mut resource = f
            .repos
            .resource
            .list_by_media_item(f.item_id)
            .await
            .unwrap()
            .pop()
            .unwrap();
        resource.locator = ResourceLocator::LocalPath {
            path: outside_file.to_string_lossy().into_owned(),
        };
        f.repos.resource.save(&resource).await.unwrap();
        let err = f
            .service
            .prepare(SessionOpenRequest {
                media_item_id: f.item_id.to_string(),
                engine: SessionEngineDto::Playback,
            })
            .await
            .unwrap_err();
        assert_eq!(err.code().as_str(), "SECURITY_POLICY_DENIED");
    }

    #[tokio::test]
    async fn missing_item_is_not_found() {
        let f = fixture(None).await;
        let err = f
            .service
            .prepare(SessionOpenRequest {
                media_item_id: MediaItemId::new().to_string(),
                engine: SessionEngineDto::Playback,
            })
            .await
            .unwrap_err();
        assert_eq!(err.code().as_str(), "MEDIA_ITEM_NOT_FOUND");
    }

    #[test]
    fn hierarchy_missing_error_codes_are_stable() {
        assert_eq!(
            media_item_not_found().code().as_str(),
            "MEDIA_ITEM_NOT_FOUND"
        );
        assert_eq!(edition_not_found().code().as_str(), "EDITION_NOT_FOUND");
        assert_eq!(work_not_found().code().as_str(), "WORK_NOT_FOUND");
    }
}
