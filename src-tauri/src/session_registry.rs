//! Opaque, owner-bound runtime session and comic page grant registry.
//!
//! Paths, archive entry keys and page bytes stay server-side. Session records
//! and comic grants deliberately share one lock so close/window cleanup can
//! revoke the complete capability set at one linearization point.

use std::collections::{HashMap, HashSet};
use std::fmt;
use std::fs::File;
use std::ops::Deref;
use std::sync::{Arc, RwLock, RwLockReadGuard, RwLockWriteGuard};

use haven_application::services::{
    PreparedComicPage, PreparedComicPageAvailability, PreparedSession,
};
use haven_application::wire::{
    ComicPageAvailabilityDto, ComicPageDto, ComicPageManifestDto, SessionEngineDto,
};
use haven_common::{AppError, ErrorKind};

pub(crate) const MAX_CONCURRENT_COMIC_PAGE_READS: usize = 4;

pub(crate) struct SessionRegistry {
    state: RwLock<RegistryState>,
}

#[derive(Default)]
struct RegistryState {
    sessions: HashMap<String, SessionRecord>,
    grants: HashMap<String, ComicGrant>,
}

struct SessionRecord {
    prepared: PreparedSession,
    owner_webview_label: String,
    owner_window_label: String,
    comic_manifest: Option<RegisteredComicManifest>,
    active_comic_reads: usize,
}

struct RegisteredComicManifest {
    dto: ComicPageManifestDto,
    grant_ids: Vec<String>,
}

#[derive(Clone)]
struct ComicGrant {
    session_id: String,
    page_index: usize,
}

/// A file handle whose canonical path was validated while the registry read
/// lock was held. The protocol consumes this handle instead of reopening a
/// changed path after authorization.
#[derive(Debug)]
pub(crate) struct VerifiedSessionFile {
    pub(crate) prepared: PreparedSession,
    pub(crate) file: File,
}

#[derive(Debug, Clone)]
pub(crate) struct VerifiedComicPage {
    pub(crate) session_id: String,
    pub(crate) prepared: PreparedSession,
    pub(crate) page: PreparedComicPage,
}

pub(crate) struct ComicPageReadPermit {
    registry: Arc<SessionRegistry>,
    verified: VerifiedComicPage,
}

impl Deref for ComicPageReadPermit {
    type Target = VerifiedComicPage;

    fn deref(&self) -> &Self::Target {
        &self.verified
    }
}

impl fmt::Debug for ComicPageReadPermit {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ComicPageReadPermit")
            .field("verified", &self.verified)
            .finish_non_exhaustive()
    }
}

impl Drop for ComicPageReadPermit {
    fn drop(&mut self) {
        self.registry
            .finish_comic_page_read(&self.verified.session_id);
    }
}

impl SessionRegistry {
    pub(crate) fn new() -> Self {
        Self {
            state: RwLock::new(RegistryState::default()),
        }
    }

    pub(crate) fn register(
        &self,
        prepared: PreparedSession,
        owner_webview_label: String,
        owner_window_label: String,
    ) -> Result<String, AppError> {
        let mut state = self.write_state()?;
        let id = unique_uuid(|candidate| state.sessions.contains_key(candidate));
        state.sessions.insert(
            id.clone(),
            SessionRecord {
                prepared,
                owner_webview_label,
                owner_window_label,
                comic_manifest: None,
                active_comic_reads: 0,
            },
        );
        Ok(id)
    }

    pub(crate) fn uri(id: &str) -> String {
        format!("haven-resource://session/{id}")
    }

    pub(crate) fn comic_page_uri(grant_id: &str) -> String {
        format!("haven-resource://comic-page/{grant_id}")
    }

    pub(crate) fn lookup_for_owner(
        &self,
        id: &str,
        owner_webview_label: &str,
    ) -> Result<PreparedSession, AppError> {
        let state = self.read_state()?;
        let record = state
            .sessions
            .get(id)
            .filter(|record| record.owner_webview_label == owner_webview_label)
            .ok_or_else(resource_not_found)?;
        Ok(record.prepared.clone())
    }

    pub(crate) fn comic_manifest(
        &self,
        id: &str,
        owner_webview_label: &str,
    ) -> Result<ComicPageManifestDto, AppError> {
        let mut state = self.write_state()?;
        let (media_item_id, prepared_pages) = {
            let record = state
                .sessions
                .get(id)
                .filter(|record| record.owner_webview_label == owner_webview_label)
                .ok_or_else(resource_not_found)?;
            if record.prepared.engine != SessionEngineDto::Comic {
                return Err(format_unsupported());
            }
            if let Some(manifest) = &record.comic_manifest {
                return Ok(manifest.dto.clone());
            }
            let pages = record
                .prepared
                .comic_pages
                .clone()
                .ok_or_else(format_unsupported)?;
            (record.prepared.media_item_id.clone(), pages)
        };

        let page_count = u32::try_from(prepared_pages.len()).map_err(|_| format_unsupported())?;
        let mut reserved_ids = HashSet::with_capacity(prepared_pages.len().saturating_mul(2));
        let mut grants = Vec::new();
        let mut pages = Vec::with_capacity(prepared_pages.len());
        for (page_index, page) in prepared_pages.iter().enumerate() {
            let page_id = unique_uuid(|candidate| {
                reserved_ids.contains(candidate)
                    || state.sessions.contains_key(candidate)
                    || state.grants.contains_key(candidate)
            });
            reserved_ids.insert(page_id.clone());
            let (availability, content_uri) = match page.availability {
                PreparedComicPageAvailability::Ready => {
                    let grant_id = unique_uuid(|candidate| {
                        reserved_ids.contains(candidate)
                            || state.sessions.contains_key(candidate)
                            || state.grants.contains_key(candidate)
                    });
                    reserved_ids.insert(grant_id.clone());
                    grants.push((grant_id.clone(), page_index));
                    (
                        ComicPageAvailabilityDto::Ready,
                        Some(Self::comic_page_uri(&grant_id)),
                    )
                }
                PreparedComicPageAvailability::Unavailable => {
                    (ComicPageAvailabilityDto::Unavailable, None)
                }
            };
            pages.push(ComicPageDto {
                page_id,
                page_index: u32::try_from(page_index).map_err(|_| format_unsupported())?,
                availability,
                content_uri,
            });
        }

        let dto = ComicPageManifestDto {
            schema_version: 1,
            session_id: id.to_owned(),
            media_item_id,
            page_count,
            pages,
        };
        for (grant_id, page_index) in &grants {
            state.grants.insert(
                grant_id.clone(),
                ComicGrant {
                    session_id: id.to_owned(),
                    page_index: *page_index,
                },
            );
        }
        let record = state.sessions.get_mut(id).ok_or_else(resource_not_found)?;
        record.comic_manifest = Some(RegisteredComicManifest {
            dto: dto.clone(),
            grant_ids: grants.into_iter().map(|(grant_id, _)| grant_id).collect(),
        });
        Ok(dto)
    }

    pub(crate) fn lookup_comic_page(
        &self,
        grant_id: &str,
        owner_webview_label: &str,
    ) -> Result<VerifiedComicPage, AppError> {
        let state = self.read_state()?;
        verified_comic_page(&state, grant_id, owner_webview_label)
    }

    pub(crate) fn begin_comic_page_read(
        self: &Arc<Self>,
        grant_id: &str,
        owner_webview_label: &str,
    ) -> Result<ComicPageReadPermit, AppError> {
        let mut state = self.write_state()?;
        let grant = state
            .grants
            .get(grant_id)
            .cloned()
            .ok_or_else(resource_not_found)?;
        let record = state
            .sessions
            .get_mut(&grant.session_id)
            .filter(|record| record.owner_webview_label == owner_webview_label)
            .ok_or_else(resource_not_found)?;
        if record.active_comic_reads >= MAX_CONCURRENT_COMIC_PAGE_READS {
            return Err(resource_busy());
        }
        let page = record
            .prepared
            .comic_pages
            .as_ref()
            .and_then(|pages| pages.get(grant.page_index))
            .cloned()
            .ok_or_else(resource_not_found)?;
        record.active_comic_reads += 1;
        Ok(ComicPageReadPermit {
            registry: self.clone(),
            verified: VerifiedComicPage {
                session_id: grant.session_id,
                prepared: comic_session_snapshot(&record.prepared),
                page,
            },
        })
    }

    fn finish_comic_page_read(&self, session_id: &str) {
        let mut state = self
            .state
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if let Some(record) = state.sessions.get_mut(session_id) {
            record.active_comic_reads = record.active_comic_reads.saturating_sub(1);
        }
    }

    /// Owner-aware idempotent close. A different WebView cannot use close as an
    /// existence oracle and cannot revoke another owner's session.
    pub(crate) fn remove_for_owner(
        &self,
        id: &str,
        owner_webview_label: &str,
    ) -> Result<Option<PreparedSession>, AppError> {
        let mut state = self.write_state()?;
        let is_owner = state
            .sessions
            .get(id)
            .is_some_and(|record| record.owner_webview_label == owner_webview_label);
        if !is_owner {
            return Ok(None);
        }
        Ok(remove_session(&mut state, id).map(|record| record.prepared))
    }

    pub(crate) fn remove_window(&self, owner_window_label: &str) -> Result<usize, AppError> {
        let mut state = self.write_state()?;
        let session_ids: Vec<String> = state
            .sessions
            .iter()
            .filter(|(_, record)| record.owner_window_label == owner_window_label)
            .map(|(id, _)| id.clone())
            .collect();
        let removed = session_ids.len();
        for session_id in session_ids {
            remove_session(&mut state, &session_id);
        }
        Ok(removed)
    }

    /// Atomically validate and open a non-Comic session file while the record
    /// read lock is held. Comic archive bytes must never use this root channel.
    pub(crate) fn revalidate(
        &self,
        id: &str,
        owner_webview_label: &str,
    ) -> Result<VerifiedSessionFile, AppError> {
        let state = self.read_state()?;
        let record = state
            .sessions
            .get(id)
            .filter(|record| record.owner_webview_label == owner_webview_label)
            .ok_or_else(resource_not_found)?;
        let prepared = &record.prepared;
        if prepared.engine == SessionEngineDto::Comic {
            return Err(resource_not_found());
        }
        let root =
            std::fs::canonicalize(&prepared.canonical_root).map_err(|_| resource_unavailable())?;
        let file =
            std::fs::canonicalize(&prepared.canonical_file).map_err(|_| resource_unavailable())?;
        if root != prepared.canonical_root
            || file != prepared.canonical_file
            || file.strip_prefix(&root).is_err()
            || !file.is_file()
        {
            return Err(policy_denied("资源路径校验失败"));
        }
        let handle = File::open(&file).map_err(|_| resource_unavailable())?;
        Ok(VerifiedSessionFile {
            prepared: prepared.clone(),
            file: handle,
        })
    }

    fn read_state(&self) -> Result<RwLockReadGuard<'_, RegistryState>, AppError> {
        self.state.read().map_err(|_| registry_unavailable())
    }

    fn write_state(&self) -> Result<RwLockWriteGuard<'_, RegistryState>, AppError> {
        self.state.write().map_err(|_| registry_unavailable())
    }
}

fn verified_comic_page(
    state: &RegistryState,
    grant_id: &str,
    owner_webview_label: &str,
) -> Result<VerifiedComicPage, AppError> {
    let grant = state.grants.get(grant_id).ok_or_else(resource_not_found)?;
    let record = state
        .sessions
        .get(&grant.session_id)
        .filter(|record| record.owner_webview_label == owner_webview_label)
        .ok_or_else(resource_not_found)?;
    let page = record
        .prepared
        .comic_pages
        .as_ref()
        .and_then(|pages| pages.get(grant.page_index))
        .cloned()
        .ok_or_else(resource_not_found)?;
    Ok(VerifiedComicPage {
        session_id: grant.session_id.clone(),
        prepared: comic_session_snapshot(&record.prepared),
        page,
    })
}

fn comic_session_snapshot(prepared: &PreparedSession) -> PreparedSession {
    PreparedSession {
        work_id: prepared.work_id.clone(),
        edition_id: prepared.edition_id.clone(),
        media_item_id: prepared.media_item_id.clone(),
        engine: prepared.engine,
        resource_id: prepared.resource_id,
        storage_location_id: prepared.storage_location_id,
        canonical_root: prepared.canonical_root.clone(),
        canonical_file: prepared.canonical_file.clone(),
        mime_type: prepared.mime_type.clone(),
        media_type: prepared.media_type,
        resource_type: prepared.resource_type,
        comic_pages: None,
        progress: prepared.progress.clone(),
    }
}

fn remove_session(state: &mut RegistryState, id: &str) -> Option<SessionRecord> {
    let record = state.sessions.remove(id)?;
    if let Some(manifest) = &record.comic_manifest {
        for grant_id in &manifest.grant_ids {
            state.grants.remove(grant_id);
        }
    }
    Some(record)
}

fn unique_uuid(mut exists: impl FnMut(&str) -> bool) -> String {
    loop {
        let candidate = uuid::Uuid::new_v4().to_string();
        if !exists(&candidate) {
            return candidate;
        }
    }
}

fn resource_not_found() -> AppError {
    AppError::new(
        "RESOURCE_NOT_FOUND",
        ErrorKind::NotFound,
        "资源会话不存在或已撤销",
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

fn resource_busy() -> AppError {
    AppError::new(
        "RESOURCE_UNAVAILABLE",
        ErrorKind::Timeout,
        "漫画页面读取繁忙，请稍后重试",
        true,
    )
}

fn format_unsupported() -> AppError {
    AppError::new(
        "FORMAT_UNSUPPORTED",
        ErrorKind::Unsupported,
        "当前 Session 不是受支持的漫画资源",
        false,
    )
}

fn policy_denied(message: &'static str) -> AppError {
    AppError::new(
        "SECURITY_POLICY_DENIED",
        ErrorKind::Security,
        message,
        false,
    )
}

fn registry_unavailable() -> AppError {
    AppError::new(
        "INTERNAL_ERROR",
        ErrorKind::Internal,
        "资源会话暂时不可用",
        false,
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use haven_application::services::{
        PreparedComicPage, PreparedComicPageAvailability, PreparedComicPageSource,
    };
    use haven_domain::enums::{MediaType, ResourceType};
    use haven_domain::ids::{ResourceId, StorageLocationId};
    use std::io::Read;
    use std::path::PathBuf;

    fn prepared(root: PathBuf, file: PathBuf) -> PreparedSession {
        PreparedSession {
            work_id: "w".into(),
            edition_id: "e".into(),
            media_item_id: uuid::Uuid::new_v4().to_string(),
            engine: SessionEngineDto::Playback,
            resource_id: ResourceId::new(),
            storage_location_id: StorageLocationId::new(),
            canonical_root: root,
            canonical_file: file,
            mime_type: None,
            media_type: MediaType::Movie,
            resource_type: ResourceType::LocalFile,
            comic_pages: None,
            progress: None,
        }
    }

    fn comic_prepared(root: PathBuf, file: PathBuf) -> PreparedSession {
        let mut prepared = prepared(root, file);
        prepared.engine = SessionEngineDto::Comic;
        prepared.media_type = MediaType::Comic;
        prepared.resource_type = ResourceType::ComicArchive;
        prepared.comic_pages = Some(vec![
            PreparedComicPage {
                availability: PreparedComicPageAvailability::Ready,
                source: PreparedComicPageSource::ArchiveEntry {
                    entry_index: 0,
                    normalized_name: "page1.jpg".into(),
                    crc32: 1,
                    compressed_size: 3,
                    uncompressed_size: 3,
                    source_size: 3,
                    source_sha256: [1; 32],
                },
            },
            PreparedComicPage {
                availability: PreparedComicPageAvailability::Unavailable,
                source: PreparedComicPageSource::ArchiveEntry {
                    entry_index: 1,
                    normalized_name: "page2.jpg".into(),
                    crc32: 2,
                    compressed_size: 0,
                    uncompressed_size: 0,
                    source_size: 3,
                    source_sha256: [1; 32],
                },
            },
            PreparedComicPage {
                availability: PreparedComicPageAvailability::Ready,
                source: PreparedComicPageSource::ArchiveEntry {
                    entry_index: 2,
                    normalized_name: "page3.jpg".into(),
                    crc32: 3,
                    compressed_size: 3,
                    uncompressed_size: 3,
                    source_size: 3,
                    source_sha256: [1; 32],
                },
            },
            PreparedComicPage {
                availability: PreparedComicPageAvailability::Ready,
                source: PreparedComicPageSource::ArchiveEntry {
                    entry_index: 3,
                    normalized_name: "page4.jpg".into(),
                    crc32: 4,
                    compressed_size: 3,
                    uncompressed_size: 3,
                    source_size: 3,
                    source_sha256: [1; 32],
                },
            },
        ]);
        prepared
    }

    fn grant_id(uri: &str) -> String {
        uri.strip_prefix("haven-resource://comic-page/")
            .expect("comic grant URI")
            .to_owned()
    }

    #[test]
    fn session_close_is_owner_aware_idempotent_and_open_handle_can_finish() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("video.mkv");
        std::fs::write(&file, b"video").unwrap();
        let root = std::fs::canonicalize(dir.path()).unwrap();
        let file = std::fs::canonicalize(file).unwrap();
        let registry = SessionRegistry::new();
        let id = registry
            .register(prepared(root, file), "main".into(), "main".into())
            .unwrap();
        assert_eq!(
            SessionRegistry::uri(&id),
            format!("haven-resource://session/{id}")
        );
        let mut verified = registry.revalidate(&id, "main").unwrap();
        assert!(registry.remove_for_owner(&id, "main2").unwrap().is_none());
        assert!(registry.lookup_for_owner(&id, "main").is_ok());
        assert!(registry.remove_for_owner(&id, "main").unwrap().is_some());
        assert!(registry.remove_for_owner(&id, "main").unwrap().is_none());
        assert_eq!(
            registry
                .revalidate(&id, "main")
                .unwrap_err()
                .code()
                .as_str(),
            "RESOURCE_NOT_FOUND"
        );
        let mut content = Vec::new();
        verified.file.read_to_end(&mut content).unwrap();
        assert_eq!(content, b"video");
    }

    #[test]
    fn comic_manifest_is_idempotent_and_close_revokes_every_grant() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("chapter.cbz");
        std::fs::write(&file, b"zip").unwrap();
        let root = std::fs::canonicalize(dir.path()).unwrap();
        let file = std::fs::canonicalize(file).unwrap();
        let registry = SessionRegistry::new();
        let id = registry
            .register(comic_prepared(root, file), "main".into(), "main".into())
            .unwrap();

        let first = registry.comic_manifest(&id, "main").unwrap();
        let second = registry.comic_manifest(&id, "main").unwrap();
        assert_eq!(first, second);
        assert_eq!(first.page_count, 4);
        assert_eq!(first.pages[0].page_index, 0);
        assert_eq!(first.pages[1].page_index, 1);
        assert_eq!(
            first.pages[1].availability,
            ComicPageAvailabilityDto::Unavailable
        );
        assert!(first.pages[1].content_uri.is_none());
        let page_ids: HashSet<&str> = first
            .pages
            .iter()
            .map(|page| page.page_id.as_str())
            .collect();
        let grant_ids: HashSet<String> = first
            .pages
            .iter()
            .filter_map(|page| page.content_uri.as_deref().map(grant_id))
            .collect();
        assert_eq!(page_ids.len(), first.pages.len());
        assert_eq!(grant_ids.len(), 3);
        assert!(page_ids.iter().all(|page_id| !grant_ids.contains(*page_id)));
        let grant = grant_id(first.pages[0].content_uri.as_deref().unwrap());
        assert_ne!(first.pages[0].page_id, grant);
        assert!(registry.lookup_comic_page(&grant, "main").is_ok());
        assert_eq!(
            registry
                .lookup_comic_page(&grant, "main2")
                .unwrap_err()
                .code()
                .as_str(),
            "RESOURCE_NOT_FOUND"
        );

        registry.remove_for_owner(&id, "main").unwrap();
        assert_eq!(
            registry
                .lookup_comic_page(&grant, "main")
                .unwrap_err()
                .code()
                .as_str(),
            "RESOURCE_NOT_FOUND"
        );
    }

    #[test]
    fn window_cleanup_is_scoped_and_reopen_rotates_runtime_identities() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("chapter.cbz");
        std::fs::write(&file, b"zip").unwrap();
        let root = std::fs::canonicalize(dir.path()).unwrap();
        let file = std::fs::canonicalize(file).unwrap();
        let registry = SessionRegistry::new();
        let first_id = registry
            .register(
                comic_prepared(root.clone(), file.clone()),
                "main".into(),
                "window-a".into(),
            )
            .unwrap();
        let second_id = registry
            .register(
                comic_prepared(root.clone(), file.clone()),
                "side".into(),
                "window-b".into(),
            )
            .unwrap();
        let first_manifest = registry.comic_manifest(&first_id, "main").unwrap();
        assert_eq!(registry.remove_window("window-a").unwrap(), 1);
        assert!(registry.lookup_for_owner(&first_id, "main").is_err());
        assert!(registry.lookup_for_owner(&second_id, "side").is_ok());

        let reopened_id = registry
            .register(comic_prepared(root, file), "main".into(), "window-a".into())
            .unwrap();
        let reopened = registry.comic_manifest(&reopened_id, "main").unwrap();
        assert_ne!(first_id, reopened_id);
        assert_ne!(first_manifest.pages[0].page_id, reopened.pages[0].page_id);
        assert_ne!(
            first_manifest.pages[0].content_uri,
            reopened.pages[0].content_uri
        );
    }

    #[test]
    fn comic_page_reads_are_bounded_per_session() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("chapter.cbz");
        std::fs::write(&file, b"zip").unwrap();
        let root = std::fs::canonicalize(dir.path()).unwrap();
        let file = std::fs::canonicalize(file).unwrap();
        let registry = std::sync::Arc::new(SessionRegistry::new());
        let id = registry
            .register(comic_prepared(root, file), "main".into(), "main".into())
            .unwrap();
        let manifest = registry.comic_manifest(&id, "main").unwrap();
        let grant = grant_id(manifest.pages[0].content_uri.as_deref().unwrap());

        let mut leases = Vec::new();
        for _ in 0..MAX_CONCURRENT_COMIC_PAGE_READS {
            leases.push(registry.begin_comic_page_read(&grant, "main").unwrap());
        }
        assert!(leases[0].prepared.comic_pages.is_none());
        let busy = registry.begin_comic_page_read(&grant, "main").unwrap_err();
        assert_eq!(busy.code().as_str(), "RESOURCE_UNAVAILABLE");
        assert!(busy.retryable());
        drop(leases);
        let lease = registry.begin_comic_page_read(&grant, "main").unwrap();
        drop(lease);
        assert!(registry.begin_comic_page_read(&grant, "main").is_ok());
    }
}
