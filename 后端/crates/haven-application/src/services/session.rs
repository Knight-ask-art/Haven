//! Session open preparation (Phase A).
//!
//! This service resolves a media item to a checked local file or an opaque,
//! allowlisted remote source identity. It deliberately does not create
//! History and does not know about the Tauri session registry.

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
use haven_domain::ids::{MediaItemId, ResourceId, SourceId, StorageLocationId};

use crate::mapper::progress::progress_summary;
use crate::services::comic::{ComicPageService, PreparedComicPage};
use crate::services::ports::{
    RemoteByteRange, RemoteSessionBody, RemoteSessionPort, SessionOpenPorts,
};
use crate::services::source_import::{source_key_for_id, validate_remote_source_object};
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
    /// `Some` only for a local session. Remote sessions deliberately carry no
    /// path or storage identity, preventing a remote locator from masquerading
    /// as a local file.
    pub storage_location_id: Option<StorageLocationId>,
    pub canonical_root: Option<PathBuf>,
    pub canonical_file: Option<PathBuf>,
    pub source: PreparedSessionSource,
    pub mime_type: Option<String>,
    pub media_type: MediaType,
    pub resource_type: ResourceType,
    pub comic_pages: Option<Vec<PreparedComicPage>>,
    pub progress: Option<ProgressSummaryDto>,
}

/// Server-only origin of the prepared session. The remote identity is opaque
/// to the frontend and is revalidated against the Resource row before every
/// protocol read.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PreparedSessionSource {
    Local,
    Remote {
        source_id: SourceId,
        source_key: String,
        remote_id: String,
    },
}

#[derive(Clone)]
pub struct SessionService {
    ports: Arc<dyn SessionOpenPorts>,
    comic_pages: ComicPageService,
    remote: Option<Arc<dyn RemoteSessionPort>>,
}

impl SessionService {
    pub fn new(ports: Arc<dyn SessionOpenPorts>, comic_pages: ComicPageService) -> Self {
        Self {
            ports,
            comic_pages,
            remote: None,
        }
    }

    /// Composition-root constructor for sessions that can read fixed,
    /// allowlisted remote providers. Keeping the provider behind an
    /// application port prevents Tauri and domain layers from learning URLs.
    pub fn new_with_remote(
        ports: Arc<dyn SessionOpenPorts>,
        comic_pages: ComicPageService,
        remote: Arc<dyn RemoteSessionPort>,
    ) -> Self {
        Self {
            ports,
            comic_pages,
            remote: Some(remote),
        }
    }

    /// Fetch one bounded body for a prepared remote Article/PDF session. The
    /// caller must already have checked the session owner in the registry;
    /// this method only accepts the immutable remote facts captured at open.
    pub async fn read_remote(
        &self,
        prepared: &PreparedSession,
        range: Option<RemoteByteRange>,
    ) -> Result<RemoteSessionBody, AppError> {
        let PreparedSessionSource::Remote {
            source_key,
            remote_id,
            ..
        } = &prepared.source
        else {
            return Err(remote_session_unavailable());
        };
        let provider = self
            .remote
            .as_ref()
            .ok_or_else(remote_session_unavailable)?;
        provider.read(source_key, remote_id, range).await
    }

    /// Validate the hierarchy, engine and resource policy, then return
    /// server-only facts for registry registration. Local resources are
    /// canonicalized here; remote resources retain only their source identity.
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
                ) && resource_type_compatible(request.engine, resource)
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
                    resolved.push(ResolvedCandidate {
                        resource,
                        resolution: ResolvedResource::Local {
                            storage_location_id,
                            canonical_root,
                            canonical_file,
                        },
                    });
                }
                CandidateResolution::Remote {
                    source_id,
                    source_key,
                    remote_id,
                } => resolved.push(ResolvedCandidate {
                    resource,
                    resolution: ResolvedResource::Remote {
                        source_id,
                        source_key,
                        remote_id,
                    },
                }),
                CandidateResolution::Skipped => {
                    // 远端流等非本地候选：静默跳过，不污染 rejected 语义。
                }
                CandidateResolution::Rejected(error) => {
                    rejected.get_or_insert(error);
                }
            }
        }
        resolved.sort_by_key(|candidate| {
            (
                if candidate.resource.availability == Availability::OfflineAvailable {
                    0u8
                } else {
                    1u8
                },
                candidate.resource.id.to_string(),
            )
        });
        let Some(ResolvedCandidate {
            resource,
            resolution,
        }) = resolved.into_iter().next()
        else {
            // 全部 Skipped = 没有可由该引擎打开的候选（例如传统远端流）
            // → RESOURCE_NOT_FOUND，由上层决定是否回退受控流会话。
            return Err(rejected.unwrap_or_else(resource_not_found));
        };
        let progress = ProgressRepository::get_for_media_item(&*self.ports, media_item_id)
            .await?
            .as_ref()
            .map(progress_summary)
            .transpose()?;

        let (storage_location_id, canonical_root, canonical_file, source) = match resolution {
            ResolvedResource::Local {
                storage_location_id,
                canonical_root,
                canonical_file,
            } => (
                Some(storage_location_id),
                Some(canonical_root),
                Some(canonical_file),
                PreparedSessionSource::Local,
            ),
            ResolvedResource::Remote {
                source_id,
                source_key,
                remote_id,
            } => (
                None,
                None,
                None,
                PreparedSessionSource::Remote {
                    source_id,
                    source_key,
                    remote_id,
                },
            ),
        };
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
            source,
            comic_pages: None,
            progress,
        };
        if prepared.engine == SessionEngineDto::Comic {
            prepared.comic_pages = Some(self.comic_pages.inspect(&prepared).await?);
        }
        Ok(prepared)
    }

    async fn resolve_local_resource(
        &self,
        resource: &Resource,
    ) -> Result<CandidateResolution, AppError> {
        // SourceObject 只能通过固定来源映射进入 Remote session；它不会被
        // 解释为路径，也不会被转换为 URL。Provider 在真正读取时再次校验
        // source_key/remote_id。
        if let ResourceLocator::SourceObject {
            source_id,
            remote_id,
        } = &resource.locator
        {
            let Some(source_key) = source_key_for_id(*source_id) else {
                return Ok(CandidateResolution::Rejected(security_denied(
                    "远端来源未被授权",
                )));
            };
            if resource.source_id != Some(*source_id)
                || validate_remote_source_object(source_key, resource.resource_type, remote_id)
                    .is_err()
            {
                return Ok(CandidateResolution::Rejected(security_denied(
                    "远端资源身份校验失败",
                )));
            }
            // A persisted SourceObject is not, by itself, proof that the
            // requested engine has an online reader.  Keep this allowlist in
            // lockstep with the provider router so a capability projection
            // can never issue a session URI that no provider can consume.
            if !remote_session_compatible(
                resource.resource_type,
                resource.mime_type.as_deref(),
                source_key,
            ) {
                return Ok(CandidateResolution::Skipped);
            }
            return Ok(CandidateResolution::Remote {
                source_id: *source_id,
                source_key: source_key.to_owned(),
                remote_id: remote_id.to_owned(),
            });
        }
        let path = match &resource.locator {
            ResourceLocator::LocalPath { path } => path.clone(),
            ResourceLocator::StorageObject {
                object_id,
                path_hint,
                ..
            } => path_hint.clone().unwrap_or_else(|| object_id.clone()),
            _ => return Ok(CandidateResolution::Skipped),
        };
        if let ResourceLocator::StorageObject { provider_id, .. } = &resource.locator {
            if resource.storage_location_id != Some(*provider_id) {
                return Ok(CandidateResolution::Rejected(security_denied(
                    "资源存储位置身份校验失败",
                )));
            }
        }
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
    Remote {
        source_id: SourceId,
        source_key: String,
        remote_id: String,
    },
    /// 非本地候选（如远端流 Http 资源）：不属于本地 Session 的管辖，
    /// 也不是策略违规；全部 Skip 时按"本地无候选"返回 RESOURCE_NOT_FOUND，
    /// 由前端回退受控流会话（stream_open，契约 §36.4）。
    Skipped,
    Rejected(AppError),
}

struct ResolvedCandidate {
    resource: Resource,
    resolution: ResolvedResource,
}

enum ResolvedResource {
    Local {
        storage_location_id: StorageLocationId,
        canonical_root: PathBuf,
        canonical_file: PathBuf,
    },
    Remote {
        source_id: SourceId,
        source_key: String,
        remote_id: String,
    },
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

/// Check the persisted resource kind *and* its format hint before a session is
/// handed to an engine.  `ResourceType` is intentionally coarse for local
/// files (TXT/Markdown/HTML/video are all `LocalFile`), so MIME and the
/// controlled locator extension are part of the gate as well.  This keeps a
/// stale or incorrectly classified row from reaching the wrong reader.
pub(crate) fn resource_type_compatible(engine: SessionEngineDto, resource: &Resource) -> bool {
    match &resource.locator {
        // Remote identities are checked again by `remote_session_compatible`
        // after the fixed source mapping has been resolved.  This first gate
        // only rejects impossible engine/resource combinations.
        ResourceLocator::SourceObject { .. } => match engine {
            SessionEngineDto::Playback => false,
            SessionEngineDto::Reader => {
                resource.resource_type == ResourceType::PublicationFile
                    && is_epub_mime(resource.mime_type.as_deref())
            }
            SessionEngineDto::Comic => resource.resource_type == ResourceType::ComicArchive,
            SessionEngineDto::Article => {
                (resource.resource_type == ResourceType::ArticleSnapshot
                    && is_html_mime(resource.mime_type.as_deref()))
                    || (resource.resource_type == ResourceType::PublicationFile
                        && is_pdf_mime(resource.mime_type.as_deref()))
            }
        },
        // HTTP locators are consumed by StreamService, not the file/session
        // protocol.  Returning false here deliberately preserves the
        // RESOURCE_NOT_FOUND -> stream_open fallback for Playback only.
        ResourceLocator::Http { .. } => false,
        ResourceLocator::LocalPath { path } => {
            local_resource_type_compatible(engine, resource, Some(path.as_str()))
        }
        ResourceLocator::StorageObject { path_hint, .. } => {
            local_resource_type_compatible(engine, resource, path_hint.as_deref())
        }
    }
}

fn local_resource_type_compatible(
    engine: SessionEngineDto,
    resource: &Resource,
    path_hint: Option<&str>,
) -> bool {
    match engine {
        SessionEngineDto::Playback => {
            matches!(
                resource.resource_type,
                ResourceType::LocalFile | ResourceType::CloudFile | ResourceType::HttpFile
            ) && is_video_or_audio(resource.mime_type.as_deref(), path_hint)
        }
        SessionEngineDto::Reader => {
            (matches!(
                resource.resource_type,
                ResourceType::LocalFile | ResourceType::CloudFile
            ) && is_text_book(resource.mime_type.as_deref(), path_hint))
                || (resource.resource_type == ResourceType::PublicationFile
                    && (is_epub_mime(resource.mime_type.as_deref())
                        || is_pdf_mime(resource.mime_type.as_deref())
                        || extension_is(path_hint, "epub")
                        || extension_is(path_hint, "pdf")))
        }
        SessionEngineDto::Comic => matches!(
            resource.resource_type,
            ResourceType::ComicArchive | ResourceType::ImageSequence
        ),
        SessionEngineDto::Article => {
            (matches!(
                resource.resource_type,
                ResourceType::LocalFile | ResourceType::CloudFile
            ) && is_article_text(resource.mime_type.as_deref(), path_hint))
                || (resource.resource_type == ResourceType::PublicationFile
                    && (is_pdf_mime(resource.mime_type.as_deref())
                        || extension_is(path_hint, "pdf")))
                || (resource.resource_type == ResourceType::ArticleSnapshot
                    && is_html_mime(resource.mime_type.as_deref()))
        }
    }
}

fn normalized_mime(mime: Option<&str>) -> Option<&str> {
    mime.and_then(|value| {
        let value = value.split(';').next()?.trim();
        (!value.is_empty()).then_some(value)
    })
}

fn is_epub_mime(mime: Option<&str>) -> bool {
    normalized_mime(mime).is_some_and(|value| value.eq_ignore_ascii_case("application/epub+zip"))
}

fn is_pdf_mime(mime: Option<&str>) -> bool {
    normalized_mime(mime).is_some_and(|value| value.eq_ignore_ascii_case("application/pdf"))
}

fn is_html_mime(mime: Option<&str>) -> bool {
    normalized_mime(mime).is_some_and(|value| {
        value.eq_ignore_ascii_case("text/html")
            || value.eq_ignore_ascii_case("application/xhtml+xml")
    })
}

fn is_text_book(mime: Option<&str>, path_hint: Option<&str>) -> bool {
    normalized_mime(mime).is_some_and(|value| {
        value.eq_ignore_ascii_case("text/plain") || value.eq_ignore_ascii_case("text/markdown")
    }) || extension_is(path_hint, "txt")
        || extension_is(path_hint, "md")
        || extension_is(path_hint, "markdown")
}

fn is_article_text(mime: Option<&str>, path_hint: Option<&str>) -> bool {
    is_text_book(mime, path_hint)
        || is_html_mime(mime)
        || extension_is(path_hint, "html")
        || extension_is(path_hint, "htm")
}

fn is_video_or_audio(mime: Option<&str>, path_hint: Option<&str>) -> bool {
    normalized_mime(mime).is_some_and(|value| {
        value.to_ascii_lowercase().starts_with("video/")
            || value.to_ascii_lowercase().starts_with("audio/")
    }) || [
        "mp4", "m4v", "webm", "mkv", "avi", "mov", "wmv", "flv", "mp3", "m4a", "flac", "wav", "ogg",
    ]
    .iter()
    .any(|extension| extension_is(path_hint, extension))
}

fn extension_is(path_hint: Option<&str>, expected: &str) -> bool {
    path_hint
        .and_then(|value| Path::new(value).extension())
        .and_then(|value| value.to_str())
        .is_some_and(|value| value.eq_ignore_ascii_case(expected))
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

fn remote_session_unavailable() -> AppError {
    AppError::new(
        "SOURCE_UNAVAILABLE",
        ErrorKind::Network,
        "远端正文当前不可用，请重试或下载后阅读",
        true,
    )
}

pub(crate) fn remote_session_compatible(
    resource_type: ResourceType,
    mime_type: Option<&str>,
    source_key: &str,
) -> bool {
    match source_key {
        "mangadex" => {
            resource_type == ResourceType::ComicArchive && is_comic_archive_mime(mime_type)
        }
        "arxiv" => resource_type == ResourceType::PublicationFile && is_pdf_mime(mime_type),
        "europepmc" | "wikisource" => {
            resource_type == ResourceType::ArticleSnapshot && is_html_mime(mime_type)
        }
        "opds_gutenberg" => {
            resource_type == ResourceType::PublicationFile && is_epub_mime(mime_type)
        }
        _ => false,
    }
}

fn is_comic_archive_mime(mime: Option<&str>) -> bool {
    normalized_mime(mime).is_some_and(|value| {
        value.eq_ignore_ascii_case("application/vnd.comicbook+zip")
            || value.eq_ignore_ascii_case("application/zip")
            || value.eq_ignore_ascii_case("application/x-cbz")
    })
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
            stream_kind: None,
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
        assert_eq!(
            prepared
                .canonical_file
                .as_ref()
                .and_then(|path| path.file_name())
                .unwrap(),
            "movie.mkv"
        );
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
        assert_eq!(
            prepared
                .canonical_file
                .as_ref()
                .and_then(|path| path.file_name())
                .unwrap(),
            "movie.mkv"
        );
    }

    #[tokio::test]
    async fn source_object_prepares_remote_session_without_local_path() {
        let f = fixture(None).await;
        let mut item = MediaItemRepository::get(&*f.repos, f.item_id)
            .await
            .unwrap()
            .unwrap();
        item.media_type = MediaType::Article;
        f.repos.media_item.save(&item).await.unwrap();
        for resource in f
            .repos
            .resource
            .list_by_media_item(f.item_id)
            .await
            .unwrap()
        {
            f.repos.resource.delete(resource.id).await.unwrap();
        }

        let source_id = crate::services::source_import::stable_source_id("europepmc").unwrap();
        let now = haven_common::UtcMillis(3);
        f.repos
            .resource
            .save(&Resource {
                id: ResourceId::new(),
                media_item_id: f.item_id,
                resource_type: ResourceType::ArticleSnapshot,
                source_id: Some(source_id),
                storage_location_id: None,
                locator: ResourceLocator::SourceObject {
                    source_id,
                    remote_id: "PMC123456".into(),
                },
                mime_type: Some("text/html; charset=utf-8".into()),
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

        let prepared = f
            .service
            .prepare(SessionOpenRequest {
                media_item_id: f.item_id.to_string(),
                engine: SessionEngineDto::Article,
            })
            .await
            .unwrap();
        assert!(prepared.storage_location_id.is_none());
        assert!(prepared.canonical_root.is_none());
        assert!(prepared.canonical_file.is_none());
        assert!(matches!(
            prepared.source,
            PreparedSessionSource::Remote {
                source_key,
                remote_id,
                ..
            } if source_key == "europepmc" && remote_id == "PMC123456"
        ));
    }

    #[test]
    fn remote_session_requires_a_provider_reader_for_each_resource_kind() {
        assert!(remote_session_compatible(
            ResourceType::ComicArchive,
            Some("application/vnd.comicbook+zip"),
            "mangadex"
        ));
        assert!(remote_session_compatible(
            ResourceType::PublicationFile,
            Some("application/pdf"),
            "arxiv"
        ));
        assert!(remote_session_compatible(
            ResourceType::ArticleSnapshot,
            Some("text/html"),
            "wikisource"
        ));
        assert!(remote_session_compatible(
            ResourceType::PublicationFile,
            Some("application/epub+zip"),
            "opds_gutenberg"
        ));
        assert!(!remote_session_compatible(
            ResourceType::PublicationFile,
            Some("application/pdf"),
            "opds_gutenberg"
        ));
        assert!(!remote_session_compatible(
            ResourceType::PublicationFile,
            Some("application/epub+zip"),
            "unknown"
        ));
        assert!(!remote_session_compatible(
            ResourceType::PublicationFile,
            Some("application/pdf"),
            "unknown"
        ));
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
            Some(std::fs::canonicalize(chapter).unwrap())
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

    fn matrix_resource(
        resource_type: ResourceType,
        mime_type: Option<&str>,
        locator: ResourceLocator,
    ) -> Resource {
        Resource {
            id: ResourceId::new(),
            media_item_id: MediaItemId::new(),
            resource_type,
            source_id: None,
            storage_location_id: None,
            locator,
            mime_type: mime_type.map(str::to_owned),
            size: None,
            hash: None,
            availability: Availability::Available,
            availability_source: AvailabilitySource::User,
            modified_ms: None,
            fingerprint_first: None,
            fingerprint_last: None,
            created_at: haven_common::UtcMillis(1),
            updated_at: haven_common::UtcMillis(1),
        }
    }

    #[test]
    fn engine_resource_matrix_rejects_cross_format_sessions() {
        let video = matrix_resource(
            ResourceType::LocalFile,
            Some("video/x-matroska"),
            ResourceLocator::LocalPath {
                path: "movie.mkv".into(),
            },
        );
        assert!(resource_type_compatible(SessionEngineDto::Playback, &video));
        assert!(!resource_type_compatible(SessionEngineDto::Reader, &video));
        assert!(!resource_type_compatible(SessionEngineDto::Article, &video));
        assert!(!resource_type_compatible(SessionEngineDto::Comic, &video));

        let epub = matrix_resource(
            ResourceType::PublicationFile,
            Some("application/epub+zip; charset=binary"),
            ResourceLocator::LocalPath {
                path: "book.epub".into(),
            },
        );
        assert!(resource_type_compatible(SessionEngineDto::Reader, &epub));
        assert!(!resource_type_compatible(SessionEngineDto::Playback, &epub));
        assert!(!resource_type_compatible(SessionEngineDto::Comic, &epub));

        let pdf = matrix_resource(
            ResourceType::PublicationFile,
            Some("application/pdf"),
            ResourceLocator::LocalPath {
                path: "paper.pdf".into(),
            },
        );
        assert!(resource_type_compatible(SessionEngineDto::Reader, &pdf));
        assert!(resource_type_compatible(SessionEngineDto::Article, &pdf));
        assert!(!resource_type_compatible(SessionEngineDto::Playback, &pdf));

        let html = matrix_resource(
            ResourceType::LocalFile,
            Some("text/html; charset=utf-8"),
            ResourceLocator::LocalPath {
                path: "article.html".into(),
            },
        );
        assert!(resource_type_compatible(SessionEngineDto::Article, &html));
        assert!(!resource_type_compatible(SessionEngineDto::Reader, &html));

        let comic = matrix_resource(
            ResourceType::ComicArchive,
            Some("application/vnd.comicbook+zip"),
            ResourceLocator::LocalPath {
                path: "chapter.cbz".into(),
            },
        );
        assert!(resource_type_compatible(SessionEngineDto::Comic, &comic));
        assert!(!resource_type_compatible(SessionEngineDto::Reader, &comic));
        assert!(!resource_type_compatible(SessionEngineDto::Article, &comic));

        let dash = matrix_resource(
            ResourceType::DashStream,
            Some("application/dash+xml"),
            ResourceLocator::Http {
                url: "https://stream.example.test/manifest.mpd".into(),
            },
        );
        assert!(!resource_type_compatible(SessionEngineDto::Playback, &dash));
    }
}
