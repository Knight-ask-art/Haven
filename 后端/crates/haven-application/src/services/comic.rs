//! Comic Runtime PageSet boundary. Paths and archive entry keys remain server-only.

use std::sync::Arc;

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

#[derive(Clone)]
pub struct ComicPageService {
    provider: Arc<dyn ComicPageProvider>,
}

impl ComicPageService {
    pub fn new(provider: Arc<dyn ComicPageProvider>) -> Self {
        Self { provider }
    }

    pub fn inspect(&self, session: &PreparedSession) -> Result<Vec<PreparedComicPage>, AppError> {
        self.validate_session(session)?;
        self.provider.inspect(session)
    }

    pub fn read_page(
        &self,
        session: &PreparedSession,
        page: &PreparedComicPage,
    ) -> Result<ComicPageBody, AppError> {
        self.validate_session(session)?;
        self.provider.read_page(session, page)
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
