//! Comic Runtime PageSet boundary. Paths and archive entry keys remain server-only.

use std::sync::Arc;

use async_trait::async_trait;
use haven_common::{AppError, ErrorKind};
use haven_domain::enums::{MediaType, ResourceType};

use super::session::PreparedSession;
use crate::wire::SessionEngineDto;

/// Provider-private page locator. Never serialize this type into IPC.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PreparedComicPageSource {
    ArchiveEntry {
        entry_index: u32,
        normalized_name: String,
        crc32: u32,
        compressed_size: u64,
        uncompressed_size: u64,
        source_size: u64,
        source_sha256: [u8; 32],
    },
    DirectoryFile {
        relative_name: String,
        expected_size: u64,
        sha256: [u8; 32],
    },
    /// A page belonging to a remote provider.  The page name is an opaque,
    /// provider-validated fact; it is never serialized to the frontend.  The
    /// provider must derive the actual request from the session's source
    /// identity and re-check its allowlist on every read.
    RemotePage { page_name: String },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PreparedComicPage {
    pub availability: PreparedComicPageAvailability,
    pub source: PreparedComicPageSource,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PreparedComicPageAvailability {
    Ready,
    Unavailable,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ComicImageMime {
    Jpeg,
    Png,
    Webp,
}

impl ComicImageMime {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Jpeg => "image/jpeg",
            Self::Png => "image/png",
            Self::Webp => "image/webp",
        }
    }
}

pub struct ComicPageBody {
    pub mime_type: ComicImageMime,
    pub bytes: Vec<u8>,
}

/// Runtime IO port implemented by Infrastructure. It accepts only server-side prepared facts.
pub trait ComicPageProvider: Send + Sync {
    fn inspect(&self, session: &PreparedSession) -> Result<Vec<PreparedComicPage>, AppError>;
    fn read_page(
        &self,
        session: &PreparedSession,
        page: &PreparedComicPage,
    ) -> Result<ComicPageBody, AppError>;
}

/// Asynchronous provider port for remote comic pages.  It is kept separate
/// from the local synchronous provider so local archive/directory behavior
/// remains unchanged while the controlled protocol can perform network IO.
#[async_trait]
pub trait RemoteComicPageProvider: Send + Sync {
    async fn inspect(&self, session: &PreparedSession) -> Result<Vec<PreparedComicPage>, AppError>;

    async fn read_page(
        &self,
        session: &PreparedSession,
        page: &PreparedComicPage,
    ) -> Result<ComicPageBody, AppError>;
}

#[derive(Clone)]
pub struct ComicPageService {
    provider: Arc<dyn ComicPageProvider>,
    remote_provider: Option<Arc<dyn RemoteComicPageProvider>>,
}

impl ComicPageService {
    pub fn new(provider: Arc<dyn ComicPageProvider>) -> Self {
        Self {
            provider,
            remote_provider: None,
        }
    }

    /// Attach the single controlled remote comic provider at the composition
    /// root.  A missing provider fails closed for remote sessions; local
    /// archives and image directories continue to use `provider`.
    pub fn with_remote_provider(mut self, provider: Arc<dyn RemoteComicPageProvider>) -> Self {
        self.remote_provider = Some(provider);
        self
    }

    pub async fn inspect(
        &self,
        session: &PreparedSession,
    ) -> Result<Vec<PreparedComicPage>, AppError> {
        self.validate_session(session)?;
        if matches!(
            &session.source,
            crate::services::session::PreparedSessionSource::Remote { .. }
        ) {
            let provider = self
                .remote_provider
                .as_ref()
                .ok_or_else(remote_unavailable)?;
            provider.inspect(session).await
        } else {
            self.provider.inspect(session)
        }
    }

    pub async fn read_page(
        &self,
        session: &PreparedSession,
        page: &PreparedComicPage,
    ) -> Result<ComicPageBody, AppError> {
        self.validate_session(session)?;
        if matches!(
            &session.source,
            crate::services::session::PreparedSessionSource::Remote { .. }
        ) {
            let provider = self
                .remote_provider
                .as_ref()
                .ok_or_else(remote_unavailable)?;
            provider.read_page(session, page).await
        } else {
            self.provider.read_page(session, page)
        }
    }

    fn validate_session(&self, session: &PreparedSession) -> Result<(), AppError> {
        if session.engine != SessionEngineDto::Comic
            || session.media_type != MediaType::Comic
            || !matches!(
                session.resource_type,
                ResourceType::ComicArchive | ResourceType::ImageSequence
            )
        {
            return Err(AppError::new(
                "FORMAT_UNSUPPORTED",
                ErrorKind::Unsupported,
                "当前 Session 不是受支持的漫画资源",
                false,
            ));
        }
        Ok(())
    }
}

fn remote_unavailable() -> AppError {
    AppError::new(
        "SOURCE_UNAVAILABLE",
        ErrorKind::Network,
        "远端漫画来源当前不可用，请重试或下载后阅读",
        true,
    )
}
