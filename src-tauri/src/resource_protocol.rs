//! Owner-bound `haven-resource` authorization and bounded response handling.
//!
//! Every request is re-authorized against the runtime registry and current
//! repository state before bytes are read. Paths, locators and provider errors
//! never cross the protocol boundary.

use std::fmt;
use std::fs::File;
use std::io::{Read, Seek, SeekFrom};
use std::net::{IpAddr, SocketAddr, ToSocketAddrs};
use std::path::Path;
use std::path::PathBuf;
use std::sync::Arc;

use tauri::http::{Method, Response, Uri};
use tauri::Manager;
use uuid::Uuid;

use haven_application::services::ports::{RemoteByteRange, RemoteSessionBody};
use haven_application::services::{ComicPageBody, PreparedSession, PreparedSessionSource};
use haven_application::wire::SessionEngineDto;
#[cfg(test)]
use haven_common::network::validate_host;
use haven_common::network::{is_publicly_routable, parse_http_url, HttpUrlPolicy};
use haven_common::{AppError, ErrorKind};
use haven_domain::contracts::{ResourceRepository, StorageLocationRepository};
use haven_domain::entities::{Resource, ResourceLocator, StorageLocation};
use haven_domain::enums::{Availability, ResourceType, StorageProviderType, StorageStatus};
use haven_domain::ids::{MediaItemId, ResourceId, StorageLocationId};
use haven_infrastructure::artwork_cache::{ArtworkResponse, ArtworkVariant};

use crate::session_registry::VerifiedSession;
use crate::state::AppState;
use crate::stream_registry::StreamGrantInner;

/// Maximum number of bytes read into one response body.
pub(crate) const MAX_RESPONSE_BYTES: u64 = 32 * 1024 * 1024;

/// 流直连媒体单响应上限（HLS 分片远小于此；直连大文件依赖前端 Range 请求）。
const MAX_STREAM_BYTES: u64 = 256 * 1024 * 1024;
/// HLS manifest 文本上限。
const MAX_MANIFEST_BYTES: usize = 2 * 1024 * 1024;
/// 字幕是一次性受控读取，不提供 Range，也不允许无限大正文。
const MAX_SUBTITLE_BYTES: u64 = 8 * 1024 * 1024;
/// Do not follow an unbounded redirect chain while proxying a stream.
const MAX_STREAM_REDIRECTS: usize = 3;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct ByteRange {
    /// Inclusive first byte.
    pub(crate) start: u64,
    /// Inclusive last byte.
    pub(crate) end: u64,
}

impl ByteRange {
    pub(crate) fn len(self) -> u64 {
        self.end - self.start + 1
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum RangeError {
    Invalid,
    EmptyResource,
    Unsatisfiable,
    Overflow,
}

impl fmt::Display for RangeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Invalid => "invalid byte range",
            Self::EmptyResource => "empty resource has no byte ranges",
            Self::Unsatisfiable => "byte range is not satisfiable",
            Self::Overflow => "byte range overflows",
        })
    }
}

/// Parse the single-range form of an HTTP `Range` header.
///
/// `None` means no Range header.  Multi-range requests and anything other
/// than the exact `bytes=` unit are rejected.
pub(crate) fn parse_byte_range(
    header: Option<&str>,
    total: u64,
) -> Result<Option<ByteRange>, RangeError> {
    let Some(header) = header else {
        return Ok(None);
    };
    if total == 0 {
        return Err(RangeError::EmptyResource);
    }
    let Some(spec) = header.strip_prefix("bytes=") else {
        return Err(RangeError::Invalid);
    };
    if spec.is_empty() || spec.contains(',') {
        return Err(RangeError::Invalid);
    }
    let mut parts = spec.split('-');
    let first = parts.next().ok_or(RangeError::Invalid)?;
    let second = parts.next().ok_or(RangeError::Invalid)?;
    if parts.next().is_some() {
        return Err(RangeError::Invalid);
    }

    if first.is_empty() {
        let suffix = second.parse::<u64>().map_err(|_| RangeError::Overflow)?;
        if suffix == 0 {
            return Err(RangeError::Unsatisfiable);
        }
        let start = total.saturating_sub(suffix);
        return Ok(Some(ByteRange {
            start,
            end: total - 1,
        }));
    }

    let start = first.parse::<u64>().map_err(|_| RangeError::Overflow)?;
    if start >= total {
        return Err(RangeError::Unsatisfiable);
    }
    let end = if second.is_empty() {
        total - 1
    } else {
        let requested = second.parse::<u64>().map_err(|_| RangeError::Overflow)?;
        if requested < start {
            return Err(RangeError::Unsatisfiable);
        }
        requested.min(total - 1)
    };
    Ok(Some(ByteRange { start, end }))
}

/// Parse the single-range form used by remote PDF sessions. The total size is
/// intentionally unknown until the provider responds, so suffix ranges are
/// rejected and open-ended ranges are represented as `end = None`.
fn parse_remote_byte_range(header: Option<&str>) -> Result<Option<RemoteByteRange>, RangeError> {
    let Some(header) = header else {
        return Ok(None);
    };
    let Some(spec) = header.strip_prefix("bytes=") else {
        return Err(RangeError::Invalid);
    };
    if spec.is_empty() || spec.contains(',') {
        return Err(RangeError::Invalid);
    }
    let (first, second) = spec.split_once('-').ok_or(RangeError::Invalid)?;
    if first.is_empty() {
        // We cannot safely resolve a suffix range before knowing the remote
        // total, and must not fetch the entire body to discover it.
        return Err(RangeError::Unsatisfiable);
    }
    let start = first.parse::<u64>().map_err(|_| RangeError::Overflow)?;
    let end = if second.is_empty() {
        None
    } else {
        let end = second.parse::<u64>().map_err(|_| RangeError::Overflow)?;
        if end < start {
            return Err(RangeError::Unsatisfiable);
        }
        Some(end)
    };
    Ok(Some(RemoteByteRange { start, end }))
}

fn invalid_remote_range() -> AppError {
    AppError::new(
        "RANGE_INVALID",
        ErrorKind::Validation,
        "远端正文的范围请求无效",
        false,
    )
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[allow(clippy::enum_variant_names)] // 所有变体共享 `Invalid` 前缀是刻意为之的安全错误分类。
pub(crate) enum ResourceUriError {
    #[cfg(test)]
    InvalidUri,
    InvalidScheme,
    InvalidAuthority,
    InvalidPath,
    InvalidSessionId,
    /// Artwork query parameters outside the fixed `w=200`/`w=400` allowlist.
    InvalidArtworkQuery,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum ResourceRequest {
    Session(Uuid),
    Subtitle(Uuid, Uuid),
    ComicPage(Uuid),
    /// 远端流代理（V2-B 实战批次）：grant UUID + 上游目标（空 = 初始 manifest）。
    Stream(Uuid, String),
    /// 受控图片资源：image_proxy 注册的 opaque id + 只允许的列表变体。
    Artwork {
        id: String,
        variant: Option<u32>,
    },
}

impl fmt::Display for ResourceUriError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            #[cfg(test)]
            Self::InvalidUri => "invalid resource URI",
            Self::InvalidScheme => "invalid resource URI scheme",
            Self::InvalidAuthority => "invalid resource URI authority",
            Self::InvalidPath => "invalid resource URI path",
            Self::InvalidSessionId => "invalid resource session id",
            Self::InvalidArtworkQuery => "invalid artwork resource query",
        })
    }
}

/// Parse and validate a `haven-resource://session/<uuid>` URI.
///
/// 生产协议处理经 `parse_resource_uri_uri` 路径消费（Tauri 已解析为 `Uri`）；
/// 本字符串入口仅测试使用，保留 `#[cfg(test)]` 以免 dead_code 误报。
#[cfg(test)]
pub(crate) fn parse_resource_uri(raw: &str) -> Result<Uuid, ResourceUriError> {
    // `http::Uri` discards fragments during parsing; reject them from the raw
    // input first so a secret-bearing suffix can never become an accepted URI.
    if raw.contains('#') {
        return Err(ResourceUriError::InvalidUri);
    }
    let uri = raw
        .parse::<Uri>()
        .map_err(|_| ResourceUriError::InvalidUri)?;
    parse_resource_uri_uri(&uri)
}

/// URI form of [`parse_resource_uri`], useful when handling a Tauri request.
#[cfg(test)]
pub(crate) fn parse_resource_uri_uri(uri: &Uri) -> Result<Uuid, ResourceUriError> {
    parse_native_resource(uri, "session")
}

#[cfg(test)]
fn parse_comic_page_uri(raw: &str) -> Result<Uuid, ResourceUriError> {
    if raw.contains('#') {
        return Err(ResourceUriError::InvalidUri);
    }
    let uri = raw
        .parse::<Uri>()
        .map_err(|_| ResourceUriError::InvalidUri)?;
    parse_native_resource(&uri, "comic-page")
}

#[cfg(test)]
fn parse_native_resource(uri: &Uri, authority: &str) -> Result<Uuid, ResourceUriError> {
    if uri.scheme_str() != Some("haven-resource") {
        return Err(ResourceUriError::InvalidScheme);
    }
    if uri.authority().map(|value| value.as_str()) != Some(authority) {
        return Err(ResourceUriError::InvalidAuthority);
    }
    parse_canonical_resource_id(uri)
}

fn parse_subtitle_resource_uri(uri: &Uri) -> Result<(Uuid, Uuid), ResourceUriError> {
    if uri.query().is_some() {
        return Err(ResourceUriError::InvalidPath);
    }
    let mut segments = uri.path().split('/');
    if segments.next() != Some("") {
        return Err(ResourceUriError::InvalidPath);
    }
    let session = segments.next().ok_or(ResourceUriError::InvalidPath)?;
    if segments.next() != Some("subtitle") {
        return Err(ResourceUriError::InvalidPath);
    }
    let track = segments.next().ok_or(ResourceUriError::InvalidPath)?;
    if segments.next().is_some() {
        return Err(ResourceUriError::InvalidPath);
    }
    Ok((
        parse_canonical_uuid_segment(session)?,
        parse_canonical_uuid_segment(track)?,
    ))
}

fn parse_canonical_uuid_segment(segment: &str) -> Result<Uuid, ResourceUriError> {
    if segment.is_empty()
        || segment.contains('\\')
        || segment.contains('%')
        || segment == "."
        || segment == ".."
    {
        return Err(ResourceUriError::InvalidPath);
    }
    let id = Uuid::parse_str(segment).map_err(|_| ResourceUriError::InvalidSessionId)?;
    if id.to_string() != segment {
        return Err(ResourceUriError::InvalidSessionId);
    }
    Ok(id)
}

fn parse_canonical_resource_id(uri: &Uri) -> Result<Uuid, ResourceUriError> {
    if uri.query().is_some() {
        return Err(ResourceUriError::InvalidPath);
    }
    let Some(segment) = uri.path().strip_prefix('/') else {
        return Err(ResourceUriError::InvalidPath);
    };
    if segment.is_empty()
        || segment.contains('/')
        || segment.contains('\\')
        || segment == "."
        || segment == ".."
        || segment.contains('%')
    {
        return Err(ResourceUriError::InvalidPath);
    }
    let id = Uuid::parse_str(segment).map_err(|_| ResourceUriError::InvalidSessionId)?;
    if id.to_string() != segment {
        return Err(ResourceUriError::InvalidSessionId);
    }
    Ok(id)
}

/// 解析 artwork 请求：路径为 image_proxy 注册的 opaque id
/// （字母数字/连字符/下划线，与 mapper 的 controlled_artwork_uri 校验一致）。
fn parse_artwork_id(uri: &Uri) -> Result<(String, Option<u32>), ResourceUriError> {
    let variant = match uri.query() {
        None => None,
        Some("w=200") => Some(200),
        Some("w=400") => Some(400),
        Some(_) => return Err(ResourceUriError::InvalidArtworkQuery),
    };
    let Some(segment) = uri.path().strip_prefix('/') else {
        return Err(ResourceUriError::InvalidPath);
    };
    if segment.is_empty()
        || !segment
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
    {
        return Err(ResourceUriError::InvalidPath);
    }
    Ok((segment.to_owned(), variant))
}

/// Return the opaque registry key (without any URI/path component).
#[cfg(test)]
pub(crate) fn parse_resource_session_id(raw: &str) -> Result<String, ResourceUriError> {
    parse_resource_uri(raw).map(|id| id.to_string())
}

/// MIME allowlist used for resource responses.  Matching is case-insensitive.
pub(crate) fn mime_for_extension(extension: &str) -> &'static str {
    match extension
        .trim_start_matches('.')
        .to_ascii_lowercase()
        .as_str()
    {
        "mp4" => "video/mp4",
        "webm" => "video/webm",
        "mkv" => "video/x-matroska",
        "mp3" => "audio/mpeg",
        "m4a" => "audio/mp4",
        "flac" => "audio/flac",
        "pdf" => "application/pdf",
        "epub" => "application/epub+zip",
        "cbz" => "application/vnd.comicbook+zip",
        "md" | "markdown" => "text/markdown; charset=utf-8",
        "html" | "htm" => "text/html; charset=utf-8",
        "vtt" | "webvtt" => "text/vtt; charset=utf-8",
        "srt" | "sbv" | "ass" | "ssa" | "ttml" | "dfxp" | "sub" | "lrc" => {
            "text/plain; charset=utf-8"
        }
        "jpg" | "jpeg" => "image/jpeg",
        "png" => "image/png",
        "webp" => "image/webp",
        "txt" => "text/plain; charset=utf-8",
        _ => "application/octet-stream",
    }
}

pub(crate) fn mime_for_path(path: &Path) -> &'static str {
    path.extension()
        .and_then(|extension| extension.to_str())
        .map(mime_for_extension)
        .unwrap_or("application/octet-stream")
}

fn stale_session() -> AppError {
    AppError::new(
        "SESSION_STALE",
        ErrorKind::NotFound,
        "资源会话已失效",
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

/// Check the database resource against the immutable session snapshot.
///
/// This helper intentionally compares only opaque IDs and the local locator;
/// no entity is ever serialized into the protocol response.
pub(crate) fn validate_resource_binding(
    prepared: &PreparedSession,
    resource: &Resource,
) -> Result<PathBuf, AppError> {
    if !matches!(&prepared.source, PreparedSessionSource::Local) {
        return Err(policy_denied("资源类型不允许由本地 Session 打开"));
    }
    let resource_id: ResourceId = prepared.resource_id;
    let media_item_id: MediaItemId = prepared
        .media_item_id
        .parse()
        .map_err(|_| stale_session())?;
    let storage_id: StorageLocationId = prepared.storage_location_id.ok_or_else(stale_session)?;
    if resource.id != resource_id
        || resource.media_item_id != media_item_id
        || resource.storage_location_id != Some(storage_id)
        || resource.resource_type != prepared.resource_type
        || resource.mime_type != prepared.mime_type
    {
        return Err(stale_session());
    }
    if !matches!(
        resource.availability,
        Availability::Available | Availability::OfflineAvailable
    ) {
        return Err(resource_unavailable());
    }
    // 本地受控存储两种定位形态与 SessionService 同策略：LocalPath 直用；
    // StorageObject 取相对路径（path_hint 优先，退化对象名）。远端定位拒绝。
    let path = match &resource.locator {
        ResourceLocator::LocalPath { path } => path.clone(),
        ResourceLocator::StorageObject {
            object_id,
            path_hint,
            ..
        } => path_hint.clone().unwrap_or_else(|| object_id.clone()),
        _ => return Err(policy_denied("资源类型不允许由本地 Session 打开")),
    };
    Ok(PathBuf::from(path))
}

/// Check the storage policy and root identity against the session snapshot.
pub(crate) fn validate_storage_binding(
    prepared: &PreparedSession,
    storage: &StorageLocation,
    current_canonical_root: &Path,
) -> Result<(), AppError> {
    if storage.id != prepared.storage_location_id.ok_or_else(stale_session)?
        || storage.provider_type != StorageProviderType::Local
    {
        return Err(policy_denied("资源存储策略不允许由本地 Session 打开"));
    }
    if !matches!(
        storage.status,
        StorageStatus::Connected | StorageStatus::ReadOnly
    ) {
        return Err(resource_unavailable());
    }
    if Some(current_canonical_root) != prepared.canonical_root.as_deref() {
        return Err(stale_session());
    }
    Ok(())
}

/// Re-authorize a session against current repository state and return the
/// registry's already-open file handle.  The path is never reopened here.
pub(crate) async fn authorize_and_open(
    state: &AppState,
    session_id: &str,
    owner_webview_label: &str,
) -> Result<VerifiedSession, AppError> {
    let prepared = state
        .session_registry
        .lookup_for_owner(session_id, owner_webview_label)?;

    if prepared.engine == SessionEngineDto::Comic {
        return Err(AppError::new(
            "RESOURCE_NOT_FOUND",
            ErrorKind::NotFound,
            "资源会话不存在或已撤销",
            false,
        ));
    }
    authorize_session_binding(state, &prepared).await?;

    // This is the final atomic registry/source check. Local sessions return an
    // already-open handle; remote sessions return only the immutable opaque
    // source facts for the provider read below.
    state
        .session_registry
        .revalidate_any(session_id, owner_webview_label)
}

async fn authorize_session_binding(
    state: &AppState,
    prepared: &PreparedSession,
) -> Result<(), AppError> {
    if let PreparedSessionSource::Remote {
        source_id,
        source_key,
        remote_id,
    } = &prepared.source
    {
        let resource = state
            .repos
            .resource
            .get(prepared.resource_id)
            .await?
            .ok_or_else(stale_session)?;
        if resource.id != prepared.resource_id
            || resource.media_item_id
                != prepared
                    .media_item_id
                    .parse::<MediaItemId>()
                    .map_err(|_| stale_session())?
            || resource.source_id != Some(*source_id)
            || resource.storage_location_id.is_some()
            || resource.resource_type != prepared.resource_type
            || resource.mime_type != prepared.mime_type
        {
            return Err(stale_session());
        }
        let ResourceLocator::SourceObject {
            source_id: current_source_id,
            remote_id: current_remote_id,
        } = &resource.locator
        else {
            return Err(stale_session());
        };
        if current_source_id != source_id || current_remote_id != remote_id {
            return Err(stale_session());
        }
        if haven_application::services::source_import::source_key_for_id(*source_id)
            != Some(source_key.as_str())
        {
            return Err(policy_denied("远端来源未被授权"));
        }
        // Re-run the Application-level SourceObject validator on every
        // protocol read.  Session preparation already validates the identity,
        // but a persisted row (or a future provider) may be changed between
        // open and the first/next resource request.  A length/control-byte
        // check alone would allow a malformed remote id to reach the
        // provider, so keep the protocol boundary fail-closed and aligned
        // with Download/Resource capability projection.
        if haven_application::services::source_import::validate_remote_source_object(
            source_key,
            resource.resource_type,
            remote_id,
        )
        .is_err()
        {
            return Err(policy_denied("远端资源身份校验失败"));
        }
        if !matches!(
            resource.availability,
            Availability::Available | Availability::OfflineAvailable
        ) {
            return Err(resource_unavailable());
        }
        return Ok(());
    }
    let resource = state
        .repos
        .resource
        .get(prepared.resource_id)
        .await?
        .ok_or_else(stale_session)?;
    let locator_path = validate_resource_binding(prepared, &resource)?;

    let storage = state
        .repos
        .storage_location
        .get(prepared.storage_location_id.ok_or_else(stale_session)?)
        .await?
        .ok_or_else(stale_session)?;
    let current_root =
        std::fs::canonicalize(&storage.root_ref).map_err(|_| resource_unavailable())?;
    if !current_root.is_dir() {
        return Err(resource_unavailable());
    }
    validate_storage_binding(prepared, &storage, &current_root)?;

    // Resolve the current locator against the current canonical root and bind
    // it to the exact source captured during session preparation.
    let raw_file = if locator_path.is_absolute() {
        locator_path
    } else {
        current_root.join(locator_path)
    };
    let current_file = std::fs::canonicalize(&raw_file).map_err(|_| resource_unavailable())?;
    let expects_directory = prepared.resource_type == ResourceType::ImageSequence;
    if Some(current_file.as_path()) != prepared.canonical_file.as_deref()
        || current_file.strip_prefix(&current_root).is_err()
        || (expects_directory && !current_file.is_dir())
        || (!expects_directory && !current_file.is_file())
    {
        return Err(stale_session());
    }
    Ok(())
}

pub(crate) async fn authorize_comic_page(
    state: &AppState,
    grant_id: &str,
    owner_webview_label: &str,
) -> Result<crate::session_registry::ComicPageReadPermit, AppError> {
    let snapshot = state
        .session_registry
        .lookup_comic_page(grant_id, owner_webview_label)?;
    authorize_session_binding(state, &snapshot.prepared).await?;
    state
        .session_registry
        .begin_comic_page_read(grant_id, owner_webview_label)
}

fn resolver_error_response(error: &AppError, origin: Option<&str>) -> Response<Vec<u8>> {
    let status = if error.kind() == ErrorKind::Timeout {
        503
    } else {
        match error.code().as_str() {
            "SESSION_NOT_FOUND" | "RESOURCE_NOT_FOUND" => 404,
            "RANGE_INVALID" => 416,
            "SOURCE_RANGE_UNSUPPORTED" => 501,
            "ARTWORK_NOT_FOUND" => 404,
            "ARTWORK_QUERY_INVALID" => 400,
            "SESSION_STALE" | "RESOURCE_UNAVAILABLE" => 410,
            "SECURITY_POLICY_DENIED" => 403,
            "FORMAT_UNSUPPORTED" | "ARTWORK_FORMAT_UNSUPPORTED" => 415,
            "ARTWORK_TOO_LARGE" => 413,
            "ARTWORK_FETCH_FAILED" | "SOURCE_UNAVAILABLE" => 502,
            "ARTWORK_CACHE_IO_FAILED" => 503,
            "DATABASE_ERROR" => 503,
            _ if error.kind() == ErrorKind::Database => 503,
            _ if error.kind() == ErrorKind::Security || error.kind() == ErrorKind::Forbidden => 403,
            _ if error.kind() == ErrorKind::Conflict => 409,
            _ => 500,
        }
    };
    let mut headers = vec![
        ("Content-Length", "0".to_owned()),
        ("Cache-Control", "no-store".to_owned()),
        ("Vary", "Origin".to_owned()),
    ];
    push_allowed_origin(&mut headers, origin);
    response(status, &headers, Vec::new())
}

/// Register the opaque, read-only resource protocol on a Tauri builder.
pub(crate) fn register_resource_protocol<R: tauri::Runtime>(
    builder: tauri::Builder<R>,
) -> tauri::Builder<R> {
    builder.register_asynchronous_uri_scheme_protocol(
        "haven-resource",
        |ctx, request, responder| {
            let method = request.method().clone();
            let origin = request
                .headers()
                .get("origin")
                .and_then(|value| value.to_str().ok())
                .map(str::to_owned);
            if method != Method::GET && method != Method::HEAD {
                responder.respond(method_not_allowed_response(origin.as_deref()));
                return;
            }
            let range = request
                .headers()
                .get("range")
                .and_then(|value| value.to_str().ok())
                .map(str::to_owned);
            let uri = request.uri().clone();
            let app_handle = ctx.app_handle().clone();
            let owner_webview_label = ctx.webview_label().to_owned();
            tauri::async_runtime::spawn_blocking(move || {
                let response = (|| {
                    let resource = parse_request_resource(&uri).map_err(|error| {
                        if matches!(error, ResourceUriError::InvalidArtworkQuery) {
                            AppError::new(
                                "ARTWORK_QUERY_INVALID",
                                ErrorKind::Validation,
                                "图片资源参数无效",
                                false,
                            )
                        } else {
                            AppError::new(
                                "RESOURCE_NOT_FOUND",
                                ErrorKind::NotFound,
                                "资源会话不存在或已撤销",
                                false,
                            )
                        }
                    })?;
                    let state = app_handle.try_state::<AppState>().ok_or_else(|| {
                        AppError::new("APP_NOT_READY", ErrorKind::Internal, "应用尚未就绪", true)
                    })?;
                    match resource {
                        ResourceRequest::Session(session_id) => {
                            let verified = tauri::async_runtime::block_on(authorize_and_open(
                                state.inner(),
                                &session_id.to_string(),
                                &owner_webview_label,
                            ))?;
                            match verified {
                                VerifiedSession::Local(verified) => {
                                    let Some(path) = verified.prepared.canonical_file.as_deref()
                                    else {
                                        return Err(resource_unavailable());
                                    };
                                    Ok::<_, AppError>(response_for_open_file_with_origin(
                                        &method,
                                        verified.file,
                                        path,
                                        range.as_deref(),
                                        origin.as_deref(),
                                    ))
                                }
                                VerifiedSession::Remote(prepared) => {
                                    let remote_range = parse_remote_byte_range(range.as_deref())
                                        .map_err(|_| invalid_remote_range())?;
                                    let body = tauri::async_runtime::block_on(
                                        state.session.read_remote(&prepared, remote_range),
                                    )?;
                                    Ok::<_, AppError>(response_for_remote_session(
                                        &method,
                                        body,
                                        origin.as_deref(),
                                    ))
                                }
                            }
                        }
                        ResourceRequest::Subtitle(session_id, track_id) => {
                            if range.is_some() {
                                return Ok::<_, AppError>(error_response(
                                    416,
                                    false,
                                    None,
                                    origin.as_deref(),
                                ));
                            }
                            let snapshot = state
                                .session_registry
                                .lookup_for_owner(&session_id.to_string(), &owner_webview_label)?;
                            tauri::async_runtime::block_on(authorize_session_binding(
                                state.inner(),
                                &snapshot,
                            ))?;
                            let verified = state.session_registry.revalidate_subtitle(
                                &session_id.to_string(),
                                &track_id.to_string(),
                                &owner_webview_label,
                            )?;
                            Ok::<_, AppError>(response_for_subtitle_file(
                                &method,
                                verified.file,
                                &verified.track.canonical_file,
                                None,
                                origin.as_deref(),
                            ))
                        }
                        ResourceRequest::Stream(grant_id, target) => {
                            if method == Method::HEAD {
                                return Ok::<_, AppError>(error_response(
                                    405,
                                    false,
                                    None,
                                    origin.as_deref(),
                                ));
                            }
                            let inner = state
                                .stream_registry
                                .lookup(&grant_id.to_string(), &owner_webview_label)
                                .ok_or_else(|| {
                                    AppError::new(
                                        "RESOURCE_NOT_FOUND",
                                        ErrorKind::NotFound,
                                        "流会话不存在或已撤销",
                                        false,
                                    )
                                })?;
                            let response = match tauri::async_runtime::block_on(serve_stream(
                                &inner,
                                &grant_id.to_string(),
                                target.as_str(),
                                range.as_deref(),
                                origin.as_deref(),
                            )) {
                                Ok(resp) => resp,
                                Err(err) => return Err(err),
                            };
                            Ok(response)
                        }
                        ResourceRequest::ComicPage(grant_id) => {
                            if method == Method::HEAD {
                                return Ok::<_, AppError>(comic_method_not_allowed_response(
                                    origin.as_deref(),
                                ));
                            }
                            if range.is_some() {
                                return Ok::<_, AppError>(error_response(
                                    416,
                                    false,
                                    None,
                                    origin.as_deref(),
                                ));
                            }
                            let verified = tauri::async_runtime::block_on(authorize_comic_page(
                                state.inner(),
                                &grant_id.to_string(),
                                &owner_webview_label,
                            ))?;
                            let page = tauri::async_runtime::block_on(
                                state
                                    .comic_pages
                                    .read_page(&verified.prepared, &verified.page),
                            )?;
                            Ok::<_, AppError>(response_for_comic_page(
                                &method,
                                page,
                                range.as_deref(),
                                origin.as_deref(),
                            ))
                        }
                        ResourceRequest::Artwork { id, variant } => {
                            let artwork_variant = match variant {
                                None => ArtworkVariant::Original,
                                Some(200) => ArtworkVariant::Width200,
                                Some(400) => ArtworkVariant::Width400,
                                Some(_) => {
                                    return Ok::<_, AppError>(error_response(
                                        400,
                                        false,
                                        None,
                                        origin.as_deref(),
                                    ));
                                }
                            };
                            let response = tauri::async_runtime::block_on(
                                state.artwork_cache.load(&id, artwork_variant),
                            )?;
                            Ok::<_, AppError>(artwork_response(
                                &method,
                                response,
                                origin.as_deref(),
                            ))
                        }
                    }
                })()
                .unwrap_or_else(|error| resolver_error_response(&error, origin.as_deref()));
                responder.respond(response);
            });
        },
    )
}

/// Tauri's Windows webview presents custom-scheme requests through a fixed
/// `http://haven-resource.session/...` origin.  Keep that compatibility form
/// exact and isolated from the strict core parser.
fn parse_request_resource(uri: &Uri) -> Result<ResourceRequest, ResourceUriError> {
    if uri.scheme_str() == Some("haven-resource") {
        return match uri.authority().map(|authority| authority.as_str()) {
            Some("session") => {
                if uri.path().contains("/subtitle/") {
                    parse_subtitle_resource_uri(uri)
                        .map(|(session, track)| ResourceRequest::Subtitle(session, track))
                } else {
                    parse_canonical_resource_id(uri).map(ResourceRequest::Session)
                }
            }
            Some("comic-page") => parse_canonical_resource_id(uri).map(ResourceRequest::ComicPage),
            Some("artwork") => {
                parse_artwork_id(uri).map(|(id, variant)| ResourceRequest::Artwork { id, variant })
            }
            Some("stream") => {
                parse_stream_request(uri).map(|(id, target)| ResourceRequest::Stream(id, target))
            }
            _ => Err(ResourceUriError::InvalidAuthority),
        };
    }
    if uri.scheme_str() != Some("http") {
        return Err(ResourceUriError::InvalidScheme);
    }
    match uri.authority().map(|authority| authority.as_str()) {
        Some("haven-resource.session") => {
            if uri.path().contains("/subtitle/") {
                parse_subtitle_resource_uri(uri)
                    .map(|(session, track)| ResourceRequest::Subtitle(session, track))
            } else {
                parse_canonical_resource_id(uri).map(ResourceRequest::Session)
            }
        }
        Some("haven-resource.comic-page") => {
            parse_canonical_resource_id(uri).map(ResourceRequest::ComicPage)
        }
        Some("haven-resource.artwork") => {
            parse_artwork_id(uri).map(|(id, variant)| ResourceRequest::Artwork { id, variant })
        }
        Some("haven-resource.stream") => {
            parse_stream_request(uri).map(|(id, target)| ResourceRequest::Stream(id, target))
        }
        _ => Err(ResourceUriError::InvalidAuthority),
    }
}

/// 解析流代理请求：路径为 canonical grant UUID；query 必须恰好携带一个 `u` 参数
/// （上游目标，由 manifest 改写阶段生成），禁止 fragment。
fn parse_stream_request(uri: &Uri) -> Result<(Uuid, String), ResourceUriError> {
    if uri.query().is_none() {
        // 无 u 参数 = 拉取初始 manifest（upstream URL 在 grant 事实中）。
        let id = parse_stream_grant_path(uri)?;
        return Ok((id, String::new()));
    }
    let query = uri.query().unwrap_or_default();
    let mut pairs = query.split('&');
    let (key, value) = pairs
        .next()
        .and_then(|pair| pair.split_once('='))
        .ok_or(ResourceUriError::InvalidPath)?;
    if key != "u" || pairs.next().is_some() || value.is_empty() {
        return Err(ResourceUriError::InvalidPath);
    }
    // Manifest rewrites carry only UUID tokens. Reject percent-encoded values
    // (in particular a URL) and require the canonical UUID representation.
    if value.contains('%')
        || Uuid::parse_str(value).is_err()
        || Uuid::parse_str(value)
            .ok()
            .is_some_and(|id| id.to_string() != value)
    {
        return Err(ResourceUriError::InvalidPath);
    }
    let id = parse_stream_grant_path_without_query(uri)?;
    Ok((id, value.to_owned()))
}

fn parse_stream_grant_path(uri: &Uri) -> Result<Uuid, ResourceUriError> {
    let segment = uri
        .path()
        .strip_prefix('/')
        .ok_or(ResourceUriError::InvalidPath)?;
    let id = Uuid::parse_str(segment).map_err(|_| ResourceUriError::InvalidSessionId)?;
    if id.to_string() != segment {
        return Err(ResourceUriError::InvalidSessionId);
    }
    Ok(id)
}

/// 带 query 的 URI：只校验 path 部分的 grant 形状。
fn parse_stream_grant_path_without_query(uri: &Uri) -> Result<Uuid, ResourceUriError> {
    parse_stream_grant_path(uri)
}

fn response(status: u16, headers: &[(&str, String)], body: Vec<u8>) -> Response<Vec<u8>> {
    let mut builder = Response::builder().status(status);
    for (name, value) in headers {
        builder = builder.header(*name, value.as_str());
    }
    builder
        .body(body)
        .expect("static response headers are valid")
}

fn error_response(
    status: u16,
    accept_ranges: bool,
    content_range: Option<String>,
    origin: Option<&str>,
) -> Response<Vec<u8>> {
    let mut headers = vec![
        ("Content-Length", "0".to_owned()),
        ("Cache-Control", "no-store".to_owned()),
        ("Vary", "Origin".to_owned()),
    ];
    if accept_ranges {
        headers.push(("Accept-Ranges", "bytes".to_owned()));
    }
    if let Some(value) = content_range {
        headers.push(("Content-Range", value));
    }
    push_allowed_origin(&mut headers, origin);
    response(status, &headers, Vec::new())
}

fn method_not_allowed_response(origin: Option<&str>) -> Response<Vec<u8>> {
    let mut headers = vec![
        ("Allow", "GET, HEAD".to_owned()),
        ("Content-Length", "0".to_owned()),
        ("Cache-Control", "no-store".to_owned()),
        ("Vary", "Origin".to_owned()),
    ];
    push_allowed_origin(&mut headers, origin);
    response(405, &headers, Vec::new())
}

fn comic_method_not_allowed_response(origin: Option<&str>) -> Response<Vec<u8>> {
    let mut headers = vec![
        ("Allow", "GET".to_owned()),
        ("Content-Length", "0".to_owned()),
        ("Cache-Control", "no-store".to_owned()),
        ("Vary", "Origin".to_owned()),
    ];
    push_allowed_origin(&mut headers, origin);
    response(405, &headers, Vec::new())
}

/// Build a bounded response from an already-open file.
///
/// The path is used only to choose an allowlisted MIME type; it is never
/// included in the response.  The file itself is the source of truth for all
/// bytes and metadata.
fn response_for_open_file_with_origin(
    method: &Method,
    mut file: File,
    path: &Path,
    range_header: Option<&str>,
    origin: Option<&str>,
) -> Response<Vec<u8>> {
    if method != Method::GET && method != Method::HEAD {
        return method_not_allowed_response(origin);
    }

    let total = match file.metadata().map(|metadata| metadata.len()) {
        Ok(total) => total,
        Err(_) => return error_response(500, false, None, origin),
    };
    let range = match parse_byte_range(range_header, total) {
        Ok(range) => range,
        Err(_) => return error_response(416, true, Some(format!("bytes */{total}")), origin),
    };
    let (status, start, content_length) = match range {
        Some(range) => (206, range.start, range.len()),
        None => (200, 0, total),
    };

    // HEAD has no body, so the response body cap applies to actual reads only.
    if method == Method::GET && content_length > MAX_RESPONSE_BYTES {
        return error_response(413, true, None, origin);
    }

    let body = if method == Method::HEAD {
        Vec::new()
    } else {
        if file.seek(SeekFrom::Start(start)).is_err() {
            return error_response(500, false, None, origin);
        }
        let mut body = Vec::with_capacity(content_length as usize);
        let read_result = file.take(content_length).read_to_end(&mut body);
        if read_result.is_err() || body.len() as u64 != content_length {
            return error_response(500, false, None, origin);
        }
        body
    };

    let mut headers = vec![
        ("Accept-Ranges", "bytes".to_owned()),
        ("Content-Length", content_length.to_string()),
        ("Content-Type", mime_for_path(path).to_owned()),
        ("X-Content-Type-Options", "nosniff".to_owned()),
        ("Cache-Control", "no-store".to_owned()),
    ];
    if let Some(range) = range {
        headers.push((
            "Content-Range",
            format!("bytes {}-{}/{}", range.start, range.end, total),
        ));
    }
    headers.push(("Vary", "Origin".to_owned()));
    push_allowed_origin(&mut headers, origin);
    response(status, &headers, body)
}

fn response_for_subtitle_file(
    method: &Method,
    mut file: File,
    path: &Path,
    range_header: Option<&str>,
    origin: Option<&str>,
) -> Response<Vec<u8>> {
    if method != Method::GET && method != Method::HEAD {
        return method_not_allowed_response(origin);
    }
    if range_header.is_some() {
        return error_response(416, false, None, origin);
    }
    let total = match file.metadata().map(|metadata| metadata.len()) {
        Ok(total) => total,
        Err(_) => return error_response(500, false, None, origin),
    };
    if total > MAX_SUBTITLE_BYTES {
        return error_response(413, false, None, origin);
    }
    let body = if *method == Method::HEAD {
        Vec::new()
    } else {
        let mut body = Vec::with_capacity(total as usize);
        if file.read_to_end(&mut body).is_err() || body.len() as u64 != total {
            return error_response(500, false, None, origin);
        }
        body
    };
    let mut headers = vec![
        ("Content-Length", total.to_string()),
        ("Content-Type", mime_for_path(path).to_owned()),
        ("X-Content-Type-Options", "nosniff".to_owned()),
        ("Cache-Control", "no-store".to_owned()),
        ("Vary", "Origin".to_owned()),
    ];
    push_allowed_origin(&mut headers, origin);
    response(200, &headers, body)
}

/// Build a bounded response for an application-provided remote body. URLs and
/// provider headers are deliberately discarded; only the validated MIME,
/// length and (for arXiv) content range cross the resource protocol.
fn response_for_remote_session(
    method: &Method,
    body: RemoteSessionBody,
    origin: Option<&str>,
) -> Response<Vec<u8>> {
    let content_length = body.bytes.len() as u64;
    if *method != Method::GET && *method != Method::HEAD {
        return method_not_allowed_response(origin);
    }
    if content_length > MAX_RESPONSE_BYTES {
        return error_response(413, body.accept_ranges, None, origin);
    }
    if !remote_session_body_is_valid(&body, content_length) {
        // The provider/application boundary should already reject malformed
        // metadata. Keep the protocol fail-closed as a second line of defence
        // in case a future provider returns an inconsistent body.
        return error_response(502, false, None, origin);
    }
    let (status, content_range) = match body.content_range {
        Some(range) => (
            206,
            Some(format!(
                "bytes {}-{}/{}",
                range.start, range.end, range.total
            )),
        ),
        None => (200, None),
    };
    let mut headers = vec![
        ("Content-Length", content_length.to_string()),
        ("Content-Type", body.mime_type),
        ("X-Content-Type-Options", "nosniff".to_owned()),
        ("Cache-Control", "no-store".to_owned()),
        ("Vary", "Origin".to_owned()),
    ];
    if body.accept_ranges {
        headers.push(("Accept-Ranges", "bytes".to_owned()));
    }
    if let Some(content_range) = content_range {
        headers.push(("Content-Range", content_range));
    }
    push_allowed_origin(&mut headers, origin);
    let payload = if *method == Method::HEAD {
        Vec::new()
    } else {
        body.bytes
    };
    response(status, &headers, payload)
}

fn remote_session_body_is_valid(body: &RemoteSessionBody, content_length: u64) -> bool {
    if body.total_size == 0 || content_length == 0 {
        return false;
    }
    match body.content_range {
        Some(range) => {
            let Some(expected_length) = range
                .end
                .checked_sub(range.start)
                .and_then(|length| length.checked_add(1))
            else {
                return false;
            };
            range.start <= range.end
                && range.end < range.total
                && range.total == body.total_size
                && expected_length == content_length
        }
        None => content_length == body.total_size,
    }
}

fn response_for_comic_page(
    method: &Method,
    page: ComicPageBody,
    range_header: Option<&str>,
    origin: Option<&str>,
) -> Response<Vec<u8>> {
    if method != Method::GET {
        return comic_method_not_allowed_response(origin);
    }
    if range_header.is_some() {
        return error_response(416, false, None, origin);
    }
    let content_length = page.bytes.len() as u64;
    if content_length > MAX_RESPONSE_BYTES {
        return error_response(413, false, None, origin);
    }
    let mut headers = vec![
        ("Content-Length", content_length.to_string()),
        ("Content-Type", page.mime_type.as_str().to_owned()),
        ("X-Content-Type-Options", "nosniff".to_owned()),
        ("Cache-Control", "no-store".to_owned()),
        ("Vary", "Origin".to_owned()),
    ];
    push_allowed_origin(&mut headers, origin);
    response(200, &headers, page.bytes)
}

fn push_allowed_origin(headers: &mut Vec<(&'static str, String)>, origin: Option<&str>) {
    if let Some(origin) =
        origin.filter(|origin| matches!(*origin, "tauri://localhost" | "http://tauri.localhost"))
    {
        headers.push(("Access-Control-Allow-Origin", origin.to_owned()));
    }
}

// ---------- 受控图片资源（契约 §36 C1 / V02-ARTWORK-CACHE-001） ----------

/// Artwork 字节只能来自 Infrastructure 的本地缓存。资源协议不接触
/// target URL，也不复用允许 LAN 的流媒体 HTTP Client。
fn artwork_response(
    method: &Method,
    artwork: ArtworkResponse,
    origin: Option<&str>,
) -> Response<Vec<u8>> {
    let mut headers = vec![
        ("Content-Length", artwork.bytes.len().to_string()),
        ("Content-Type", artwork.mime),
        ("X-Content-Type-Options", "nosniff".to_owned()),
        ("Cache-Control", "private, max-age=2592000".to_owned()),
        ("Vary", "Origin".to_owned()),
    ];
    push_allowed_origin(&mut headers, origin);
    let body = if *method == Method::HEAD {
        Vec::new()
    } else {
        artwork.bytes
    };
    response(200, &headers, body)
}

// ---------- 远端流代理（V2-B 实战批次；契约 §36.4） ----------

/// Resolver choice is production-only by default.  The test fixture variant
/// is a transport injection: it keeps the production URL policy intact while
/// allowing a local listener to exercise response/range handling without
/// making loopback an accepted media destination in release code.
#[derive(Debug, Clone, Copy)]
enum StreamResolution {
    System,
    #[cfg(test)]
    Fixture(SocketAddr),
}

#[derive(Debug, Clone)]
struct ResolvedStreamTarget {
    url: String,
    host: String,
    dns_name: Option<String>,
    addresses: Vec<SocketAddr>,
}

fn stream_http_client(
    dns_name: Option<&str>,
    addresses: &[SocketAddr],
) -> Result<reqwest::Client, AppError> {
    let mut builder = reqwest::Client::builder()
        .connect_timeout(std::time::Duration::from_secs(10))
        .timeout(std::time::Duration::from_secs(30))
        // Redirects are followed explicitly in `fetch_stream_response` after
        // checking the grant's host allowlist and DNS policy at every hop.
        .redirect(reqwest::redirect::Policy::none())
        .pool_idle_timeout(std::time::Duration::from_secs(90))
        .pool_max_idle_per_host(8)
        .tcp_keepalive(std::time::Duration::from_secs(30))
        // Do not let ambient HTTP(S)_PROXY settings replace the address set
        // that was validated and pinned for this request. An explicit proxy
        // integration needs its own destination policy instead of inheriting
        // process environment state.
        .no_proxy();
    if let Some(dns_name) = dns_name {
        builder = builder.resolve_to_addrs(dns_name, addresses);
    }
    builder
        .user_agent(concat!("Haven/", env!("CARGO_PKG_VERSION")))
        .build()
        .map_err(|_| {
            AppError::new(
                "RESOURCE_UNAVAILABLE",
                ErrorKind::Network,
                "流源客户端初始化失败",
                true,
            )
        })
}

/// 解码测试中的百分号编码（生产 manifest 只使用不编码的 UUID token）。
#[cfg(test)]
fn percent_decode(value: &str) -> Result<String, AppError> {
    let bytes = value.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] == b'%' {
            let hex = bytes
                .get(index + 1..index + 3)
                .ok_or_else(|| policy_denied("流目标编码非法"))?;
            let hex_str = std::str::from_utf8(hex).map_err(|_| policy_denied("流目标编码非法"))?;
            let byte =
                u8::from_str_radix(hex_str, 16).map_err(|_| policy_denied("流目标编码非法"))?;
            out.push(byte);
            index += 3;
        } else {
            out.push(bytes[index]);
            index += 1;
        }
    }
    String::from_utf8(out).map_err(|_| policy_denied("流目标不是合法 UTF-8"))
}

/// 百分号编码（查询值用）。
fn percent_encode(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    for byte in value.as_bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(*byte as char)
            }
            other => out.push_str(&format!("%{other:02X}")),
        }
    }
    out
}

/// 相对地址 → 绝对地址。只允许 HTTP(S)，并使用 URL 解析器处理
/// `../`、查询和 IPv6 authority，避免手工拼接产生跨主机或路径歧义。
fn absolutize(base_url: &str, target: &str) -> Option<String> {
    let base = parse_http_url(base_url, HttpUrlPolicy::MediaResource)
        .ok()?
        .into_url();
    let joined = base.join(target).ok()?;
    parse_http_url(joined.as_str(), HttpUrlPolicy::MediaResource)
        .ok()
        .map(|safe| safe.as_str().to_owned())
}

/// 改写 HLS manifest：所有片段/子清单 URI 与 URI="..." 属性都收敛到本会话代理。
/// 返回改写后的文本与需要学习的主机列表。
fn rewrite_hls_manifest(
    body: &str,
    manifest_base: &str,
    grant_id: &str,
    register_target: &mut dyn FnMut(&str) -> String,
) -> Option<(String, Vec<String>)> {
    let mut hosts: Vec<String> = Vec::new();
    let mut over_budget = false;
    let mut proxy_for = |raw: &str| -> Option<String> {
        let raw = raw.trim();
        if raw.is_empty() {
            return None;
        }
        let absolute = absolutize(manifest_base, raw)?;
        let host = crate::stream_registry::host_of(&absolute)?;
        if !hosts.contains(&host) {
            hosts.push(host);
        }
        let token = register_target(&absolute);
        if token.is_empty() {
            over_budget = true;
            return None;
        }
        Some(format!(
            "http://haven-resource.stream/{grant_id}?u={}",
            percent_encode(&token)
        ))
    };
    let mut rewritten = String::with_capacity(body.len());
    for line in body.split_inclusive('\n') {
        let trimmed_end = line.trim_end_matches(['\n', '\r']);
        if trimmed_end.starts_with('#') {
            // 标签行：仅改写其中 URI="..." 属性（EXT-X-MAP / EXT-X-KEY 等）。
            // 任一属性缺少结束引号或无法通过受控 URL 校验时丢弃整行，绝不能
            // 把原始第三方 URI 留给 WebView。
            if let Some(line_out) = rewrite_hls_uri_attributes(trimmed_end, &mut proxy_for) {
                rewritten.push_str(&line_out);
            }
        } else if !trimmed_end.trim().is_empty() {
            if let Some(proxied) = proxy_for(trimmed_end.trim()) {
                rewritten.push_str(&proxied);
            }
            // An unsupported/data URI cannot be safely proxied. Drop the
            // segment line instead of leaking it to the WebView.
        } else {
            rewritten.push('\n');
        }
        if !rewritten.ends_with('\n') {
            rewritten.push('\n');
        }
    }
    (!over_budget).then_some((rewritten, hosts))
}

fn rewrite_hls_uri_attributes(
    line: &str,
    proxy_for: &mut dyn FnMut(&str) -> Option<String>,
) -> Option<String> {
    let mut output = String::with_capacity(line.len());
    let mut cursor = 0usize;
    while let Some(relative) = line[cursor..].find("URI=\"") {
        let attribute_start = cursor + relative;
        let value_start = attribute_start + "URI=\"".len();
        let value_end = value_start + line[value_start..].find('"')?;
        let raw_uri = &line[value_start..value_end];
        let proxied = proxy_for(raw_uri)?;
        output.push_str(&line[cursor..value_start]);
        output.push_str(&proxied);
        cursor = value_end;
    }
    output.push_str(&line[cursor..]);
    Some(output)
}

/// 服务远端流请求。`target` 为空表示拉取 grant 初始上游（manifest 或直连媒体）。
async fn serve_stream(
    inner: &Arc<StreamGrantInner>,
    grant_id: &str,
    target_encoded: &str,
    range_header: Option<&str>,
    origin: Option<&str>,
) -> Result<Response<Vec<u8>>, AppError> {
    serve_stream_with_resolution(
        inner,
        grant_id,
        target_encoded,
        range_header,
        origin,
        StreamResolution::System,
    )
    .await
}

async fn serve_stream_with_resolution(
    inner: &Arc<StreamGrantInner>,
    grant_id: &str,
    target_encoded: &str,
    range_header: Option<&str>,
    origin: Option<&str>,
    resolution: StreamResolution,
) -> Result<Response<Vec<u8>>, AppError> {
    let upstream = if target_encoded.is_empty() {
        inner.facts.upstream_url.clone()
    } else {
        inner
            .resolve_target(target_encoded)
            .ok_or_else(|| policy_denied("流目标令牌已失效"))?
    };
    // Reject malformed/multi-range requests before contacting the upstream.
    // A remote stream response is never allowed to silently turn an invalid
    // browser range into a full-object fetch.
    if range_header.is_some() && parse_remote_byte_range(range_header).is_err() {
        return Err(invalid_remote_range());
    }
    let (response, upstream) =
        fetch_stream_response(inner, upstream, range_header, resolution).await?;

    let content_type = response
        .headers()
        .get(reqwest::header::CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .map(str::to_owned);
    let is_manifest = (target_encoded.is_empty() && inner.facts.is_hls)
        || is_hls_manifest_mime(content_type.as_deref())
        || is_hls_manifest_url(&upstream);

    if is_manifest {
        let bytes = read_stream_body_bounded(response, MAX_MANIFEST_BYTES as u64).await?;
        if !looks_like_hls_manifest(&bytes) {
            return Err(resource_unavailable());
        }
        let text = String::from_utf8_lossy(&bytes).into_owned();
        let mut register_target = |target: &str| inner.register_target(target);
        let Some((rewritten, learned)) =
            rewrite_hls_manifest(&text, &upstream, grant_id, &mut register_target)
        else {
            return Err(resource_unavailable());
        };
        inner.learn_hosts(learned);
        let mut headers = vec![
            ("Content-Length", rewritten.len().to_string()),
            ("Content-Type", "application/vnd.apple.mpegurl".to_owned()),
            ("X-Content-Type-Options", "nosniff".to_owned()),
            ("Cache-Control", "no-store".to_owned()),
        ];
        push_allowed_origin(&mut headers, origin);
        return Ok(response_ok(headers, rewritten.into_bytes()));
    }

    // 直连媒体：转发 Range 语义与关键响应头（206 透传）。
    let upstream_status = response.status().as_u16();
    let is_partial = upstream_status == 206;
    let upstream_content_range = response
        .headers()
        .get(reqwest::header::CONTENT_RANGE)
        .and_then(|v| v.to_str().ok())
        .and_then(parse_stream_content_range);
    let upstream_accept_ranges = response
        .headers()
        .get(reqwest::header::ACCEPT_RANGES)
        .and_then(|value| value.to_str().ok())
        .is_some_and(|value| {
            value
                .split(',')
                .any(|part| part.trim().eq_ignore_ascii_case("bytes"))
        });
    let requested_range =
        range_header.and_then(|header| parse_remote_byte_range(Some(header)).ok().flatten());
    validate_direct_stream_headers(
        upstream_status,
        content_type.as_deref(),
        requested_range,
        upstream_content_range,
    )?;
    let bytes = read_stream_body_bounded(response, MAX_STREAM_BYTES).await?;
    if bytes.is_empty() {
        return Err(resource_unavailable());
    }
    if is_partial {
        let content_range = upstream_content_range.expect("partial response range checked above");
        validate_direct_stream_body_len(content_range, bytes.len() as u64)?;
    }
    let accept_ranges = is_partial || upstream_accept_ranges;
    let mut headers = vec![
        ("Content-Length", bytes.len().to_string()),
        (
            "Content-Type",
            canonical_direct_stream_mime(content_type.as_deref()),
        ),
        (
            "Accept-Ranges",
            if accept_ranges { "bytes" } else { "none" }.to_owned(),
        ),
        ("Cache-Control", "no-store".to_owned()),
    ];
    if let Some(cr) = upstream_content_range {
        headers.push((
            "Content-Range",
            format!("bytes {}-{}/{}", cr.start, cr.end, cr.total),
        ));
    }
    headers.push(("Vary", "Origin".into()));
    push_allowed_origin(&mut headers, origin);
    let status = if is_partial { 206 } else { 200 };
    let mut builder = Response::builder().status(status);
    for (name, value) in &headers {
        builder = builder.header(*name, value.as_str());
    }
    Ok(builder.body(bytes.to_vec()).expect("静态响应头合法"))
}

fn is_hls_manifest_url(raw_url: &str) -> bool {
    reqwest::Url::parse(raw_url)
        .ok()
        .map(|url| url.path().to_ascii_lowercase())
        .is_some_and(|path| path.ends_with(".m3u8") || path.ends_with(".m3u"))
}

fn is_hls_manifest_mime(value: Option<&str>) -> bool {
    let Some(value) = value else {
        return false;
    };
    let base = value.split(';').next().unwrap_or_default().trim();
    matches!(
        base.to_ascii_lowercase().as_str(),
        "application/vnd.apple.mpegurl"
            | "application/x-mpegurl"
            | "application/mpegurl"
            | "audio/mpegurl"
            | "audio/x-mpegurl"
    )
}

fn looks_like_hls_manifest(bytes: &[u8]) -> bool {
    let Ok(text) = std::str::from_utf8(bytes) else {
        return false;
    };
    text.trim_start_matches('\u{feff}')
        .trim_start()
        .starts_with("#EXTM3U")
}

fn canonical_direct_stream_mime(value: Option<&str>) -> String {
    let base = value
        .and_then(|value| value.split(';').next())
        .map(str::trim)
        .filter(|value| !value.is_empty());
    let Some(base) = base else {
        return "video/mp4".to_owned();
    };
    if base.eq_ignore_ascii_case("application/octet-stream")
        || is_audio_mime_base(base)
        || base.eq_ignore_ascii_case("text/vtt")
    {
        return "application/octet-stream".to_owned();
    }
    if is_video_mime_base(base) {
        return base.to_ascii_lowercase();
    }
    "video/mp4".to_owned()
}

fn is_video_mime_base(value: &str) -> bool {
    let lower = value.to_ascii_lowercase();
    let Some(subtype) = lower.strip_prefix("video/") else {
        return false;
    };
    !subtype.is_empty()
        && subtype
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || b"!#$&^_.+-".contains(&byte))
}

fn validate_direct_stream_headers(
    status: u16,
    content_type: Option<&str>,
    requested_range: Option<RemoteByteRange>,
    content_range: Option<StreamContentRange>,
) -> Result<(), AppError> {
    let mime_is_empty = content_type
        .map(|value| {
            value
                .split(';')
                .next()
                .unwrap_or_default()
                .trim()
                .is_empty()
        })
        .unwrap_or(true);
    let mime_is_video = content_type
        .and_then(|value| value.split(';').next())
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .is_some_and(|value| {
            is_video_mime_base(value)
                || is_audio_mime_base(value)
                || value.eq_ignore_ascii_case("text/vtt")
                || value.eq_ignore_ascii_case("application/octet-stream")
        });
    if !mime_is_empty && !mime_is_video {
        return Err(resource_unavailable());
    }

    match (status, requested_range, content_range) {
        (200, None, None) => Ok(()),
        (206, Some(requested), Some(actual)) => {
            if actual.start != requested.start
                || requested.end.is_some_and(|end| actual.end != end)
            {
                return Err(resource_unavailable());
            }
            Ok(())
        }
        // A request with Range must not silently become a full-object 200; a
        // partial response without a requested range is equally ambiguous.
        _ => Err(resource_unavailable()),
    }
}

fn is_audio_mime_base(value: &str) -> bool {
    let lower = value.to_ascii_lowercase();
    let Some(subtype) = lower.strip_prefix("audio/") else {
        return false;
    };
    !subtype.is_empty()
        && subtype
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || b"!#$&^_.+-".contains(&byte))
}

fn validate_direct_stream_body_len(
    content_range: StreamContentRange,
    body_len: u64,
) -> Result<(), AppError> {
    let expected = content_range
        .end
        .checked_sub(content_range.start)
        .and_then(|length| length.checked_add(1))
        .ok_or_else(resource_unavailable)?;
    (expected == body_len)
        .then_some(())
        .ok_or_else(resource_unavailable)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct StreamContentRange {
    start: u64,
    end: u64,
    total: u64,
}

fn parse_stream_content_range(value: &str) -> Option<StreamContentRange> {
    let value = value.strip_prefix("bytes ")?;
    let (range, total) = value.split_once('/')?;
    let (start, end) = range.split_once('-')?;
    let start = start.parse::<u64>().ok()?;
    let end = end.parse::<u64>().ok()?;
    let total = total.parse::<u64>().ok()?;
    if start > end || end >= total {
        return None;
    }
    Some(StreamContentRange { start, end, total })
}

async fn read_stream_body_bounded(
    mut response: reqwest::Response,
    max_bytes: u64,
) -> Result<Vec<u8>, AppError> {
    if response
        .content_length()
        .is_some_and(|length| length > max_bytes)
    {
        return Err(resource_unavailable());
    }
    let mut body = Vec::with_capacity(
        response
            .content_length()
            .unwrap_or(64 * 1024)
            .min(max_bytes) as usize,
    );
    while let Some(chunk) = response.chunk().await.map_err(map_stream_fetch)? {
        let next_len = body
            .len()
            .checked_add(chunk.len())
            .ok_or_else(resource_unavailable)?;
        if next_len as u64 > max_bytes {
            return Err(resource_unavailable());
        }
        body.extend_from_slice(&chunk);
    }
    Ok(body)
}

/// Fetch one stream response with an explicit, owner-bound redirect policy.
/// reqwest's automatic redirect support is disabled for the shared client so a
/// redirect cannot silently escape the grant's learned host allowlist.
async fn fetch_stream_response(
    inner: &Arc<StreamGrantInner>,
    mut upstream: String,
    range_header: Option<&str>,
    resolution: StreamResolution,
) -> Result<(reqwest::Response, String), AppError> {
    for redirect_count in 0..=MAX_STREAM_REDIRECTS {
        let target = resolve_stream_target(&upstream, resolution).await?;
        if !inner.host_allowed(&target.host) {
            return Err(policy_denied("流目标主机未被授权"));
        }

        let client = stream_http_client(target.dns_name.as_deref(), &target.addresses)?;
        let mut request = client.get(&target.url);
        if let Some(range) = range_header {
            request = request.header("range", range);
        }
        let response = request.send().await.map_err(|err| {
            let message = if err.is_timeout() {
                "流源请求超时"
            } else {
                "流源连接失败"
            };
            AppError::new("RESOURCE_UNAVAILABLE", ErrorKind::Network, message, true)
        })?;
        if response.status().is_redirection() {
            if redirect_count == MAX_STREAM_REDIRECTS {
                return Err(AppError::new(
                    "RESOURCE_UNAVAILABLE",
                    ErrorKind::Network,
                    "流源跳转次数过多",
                    true,
                ));
            }
            let Some(location) = response
                .headers()
                .get(reqwest::header::LOCATION)
                .and_then(|value| value.to_str().ok())
            else {
                return Err(AppError::new(
                    "RESOURCE_UNAVAILABLE",
                    ErrorKind::Network,
                    "流源跳转地址无效",
                    true,
                ));
            };
            let Some(next) = absolutize(&target.url, location) else {
                return Err(policy_denied("流源跳转地址不受支持"));
            };
            upstream = next;
            continue;
        }
        if !response.status().is_success() {
            return Err(AppError::new(
                "RESOURCE_UNAVAILABLE",
                ErrorKind::Network,
                "流源返回非成功状态",
                true,
            ));
        }
        return Ok((response, upstream));
    }
    Err(AppError::new(
        "RESOURCE_UNAVAILABLE",
        ErrorKind::Network,
        "流源跳转次数过多",
        true,
    ))
}

/// Validate one target, resolve its host, reject unsafe addresses, and return
/// the exact address set that will be pinned into this request's reqwest
/// client. Redirects call this function again, so every hop gets a fresh DNS
/// decision instead of inheriting the previous response's resolution.
async fn resolve_stream_target(
    raw_url: &str,
    resolution: StreamResolution,
) -> Result<ResolvedStreamTarget, AppError> {
    let (url, host, dns_name, port) = match resolution {
        StreamResolution::System => {
            let safe = parse_http_url(raw_url, HttpUrlPolicy::MediaResource)
                .map_err(|_| policy_denied("流目标地址不受安全策略允许"))?;
            let url = safe.as_str().to_owned();
            let host = safe.host().to_owned();
            let dns_name = safe
                .host()
                .parse::<IpAddr>()
                .is_err()
                .then(|| safe.as_url().host_str().unwrap_or(safe.host()).to_owned());
            let port = safe
                .as_url()
                .port_or_known_default()
                .ok_or_else(|| policy_denied("流目标端口不受安全策略允许"))?;
            (url, host, dns_name, port)
        }
        #[cfg(test)]
        StreamResolution::Fixture(_) => {
            if raw_url
                .chars()
                .any(|ch| ch.is_control() || ch.is_whitespace())
            {
                return Err(policy_denied("测试流目标地址不受安全策略允许"));
            }
            let parsed = raw_url
                .parse::<reqwest::Url>()
                .map_err(|_| policy_denied("测试流目标地址不受安全策略允许"))?;
            if !matches!(parsed.scheme(), "http" | "https")
                || !parsed.username().is_empty()
                || parsed.password().is_some()
                || parsed.fragment().is_some()
            {
                return Err(policy_denied("测试流目标地址不受安全策略允许"));
            }
            let host = validate_host(
                parsed
                    .host_str()
                    .ok_or_else(|| policy_denied("测试流目标地址不受安全策略允许"))?,
            )
            .map_err(|_| policy_denied("测试流目标地址不受安全策略允许"))?;
            let dns_name = parsed.host_str().unwrap_or(&host).to_owned();
            let port = parsed
                .port_or_known_default()
                .ok_or_else(|| policy_denied("测试流目标端口不受安全策略允许"))?;
            (parsed.to_string(), host, Some(dns_name), port)
        }
    };
    let addresses = match resolution {
        StreamResolution::System => match host.parse::<IpAddr>() {
            Ok(ip) => vec![SocketAddr::new(ip, port)],
            Err(_) => {
                resolve_public_addresses(dns_name.as_deref().unwrap_or(host.as_str()), port).await?
            }
        },
        #[cfg(test)]
        StreamResolution::Fixture(address) => vec![address],
    };
    Ok(ResolvedStreamTarget {
        url,
        host,
        dns_name,
        addresses,
    })
}

/// Resolve synchronously in a blocking worker, then fail closed if DNS
/// returns even one non-public address. The resulting list is pinned with
/// `resolve_to_addrs` before the HTTP request is sent.
async fn resolve_public_addresses(host: &str, port: u16) -> Result<Vec<SocketAddr>, AppError> {
    let host = host.to_owned();
    let addresses = tauri::async_runtime::spawn_blocking(move || {
        (host.as_str(), port)
            .to_socket_addrs()
            .map(|items| items.collect::<Vec<_>>())
    })
    .await
    .map_err(|_| {
        AppError::new(
            "RESOURCE_UNAVAILABLE",
            ErrorKind::Network,
            "流源地址解析失败",
            true,
        )
    })?
    .map_err(|_| {
        AppError::new(
            "RESOURCE_UNAVAILABLE",
            ErrorKind::Network,
            "流源地址解析失败",
            true,
        )
    })?;
    if addresses.is_empty()
        || addresses
            .iter()
            .any(|address| !is_publicly_routable(address.ip()))
    {
        return Err(policy_denied("流源解析到不受支持的地址"));
    }
    Ok(addresses)
}

fn map_stream_fetch(err: reqwest::Error) -> AppError {
    AppError::new(
        "RESOURCE_UNAVAILABLE",
        ErrorKind::Network,
        if err.is_timeout() {
            "流源读取超时"
        } else {
            "流源读取失败"
        },
        true,
    )
}

fn resource_unavailable() -> AppError {
    AppError::new(
        "RESOURCE_UNAVAILABLE",
        ErrorKind::Storage,
        "本地资源当前不可用",
        true,
    )
}

fn response_ok(headers: Vec<(&'static str, String)>, body: Vec<u8>) -> Response<Vec<u8>> {
    let mut builder = Response::builder().status(200);
    for (name, value) in &headers {
        builder = builder.header(*name, value.as_str());
    }
    builder.body(body).expect("静态响应头合法")
}

#[cfg(test)]
pub(crate) fn response_for_open_file(
    method: &Method,
    file: File,
    path: &Path,
    range_header: Option<&str>,
) -> Response<Vec<u8>> {
    response_for_open_file_with_origin(method, file, path, range_header, None)
}

#[cfg(test)]
mod tests {
    use super::*;
    use haven_application::services::{ComicImageMime, ComicPageBody};

    #[test]
    fn hls_manifest_rewrites_segments_and_uri_attrs_to_proxy() {
        let manifest = "#EXTM3U\n#EXT-X-VERSION:3\n#EXT-X-MAP:URI=\"init.mp4\"\nseg0.ts\nhttps://other-cdn.example.net/abs.ts\n";
        let mut targets = std::collections::HashMap::new();
        let mut register_target = |target: &str| {
            let token = Uuid::new_v4().to_string();
            targets.insert(token.clone(), target.to_owned());
            token
        };
        let (rewritten, hosts) = rewrite_hls_manifest(
            manifest,
            "https://cdn.example.com/a/index.m3u8",
            "00000000-0000-0000-0000-000000000001",
            &mut register_target,
        )
        .expect("fixture manifest must fit target budget");
        assert!(rewritten.contains("#EXTM3U"));
        // 相对片段 → 代理 URL（u 仅携带 opaque token，不是远端 URL）。
        assert!(rewritten
            .contains("http://haven-resource.stream/00000000-0000-0000-0000-000000000001?u="));
        let seg_line = rewritten
            .lines()
            .find(|line| line.starts_with("http://haven-resource.stream/"))
            .expect("改写后必须保留片段代理行");
        let target_token = percent_decode(seg_line.split_once("?u=").unwrap().1).unwrap();
        assert!(!target_token.contains("://"));
        assert_eq!(
            targets.get(&target_token).map(String::as_str),
            Some("https://cdn.example.com/a/seg0.ts")
        );
        // URI="..." 属性同样被改写。
        assert!(rewritten.contains(
            "URI=\"http://haven-resource.stream/00000000-0000-0000-0000-000000000001?u="
        ));
        // 学习主机包含初始 CDN 与绝对地址的另一个 CDN。
        assert!(hosts.contains(&"cdn.example.com".to_owned()));
        assert!(hosts.contains(&"other-cdn.example.net".to_owned()));
    }

    #[test]
    fn hls_uri_attributes_drop_unclosed_or_unsupported_values() {
        let mut proxy = |raw: &str| {
            raw.starts_with("https://")
                .then(|| format!("proxy://{}", raw.len()))
        };
        assert_eq!(
            rewrite_hls_uri_attributes(
                "#EXT-X-KEY:METHOD=AES-128,URI=\"https://key.bin\"",
                &mut proxy,
            )
            .as_deref(),
            Some("#EXT-X-KEY:METHOD=AES-128,URI=\"proxy://15\"")
        );
        assert!(rewrite_hls_uri_attributes("#EXT-X-MAP:URI=\"", &mut proxy).is_none());
        assert!(rewrite_hls_uri_attributes(
            "#EXT-X-KEY:METHOD=AES-128,URI=\"data:text/plain,x\"",
            &mut proxy
        )
        .is_none());
    }

    #[test]
    fn hls_manifest_validation_rejects_non_manifest_body() {
        assert!(looks_like_hls_manifest(
            b"\xEF\xBB\xBF #EXTM3U\n#EXTINF:1\nsegment.ts\n"
        ));
        assert!(!looks_like_hls_manifest(b"<html>upstream error</html>"));
        assert!(!looks_like_hls_manifest(b""));
        assert!(!looks_like_hls_manifest(b"\xFF\xFE#EXTM3U"));
    }

    #[test]
    fn percent_roundtrip_preserves_cjk_and_specials() {
        let raw = "https://cdn.example.com/x/第01集/index.m3u8?a=1&b=2";
        let encoded = percent_encode(raw);
        assert!(!encoded.contains(' '));
        assert_eq!(percent_decode(&encoded).unwrap(), raw);
    }

    #[test]
    fn stream_query_requires_single_u_param() {
        let base = format!("http://haven-resource.stream/{}", Uuid::new_v4());
        let target_token = Uuid::new_v4().to_string();
        let with_u = format!("{base}?u={target_token}");
        let parsed = with_u.parse::<Uri>().unwrap();
        // u 值是服务端注册的 opaque token；真实 URL 不进入 URI。
        let (_, target) = parse_stream_request(&parsed).unwrap();
        assert_eq!(target, target_token);

        let raw_url = format!("{base}?u=https%3A%2F%2Fcdn.example.com%2Fa.ts");
        let parsed = raw_url.parse::<Uri>().unwrap();
        assert!(parse_stream_request(&parsed).is_err());

        let no_query = base.parse::<Uri>().unwrap();
        let (id, empty) = parse_stream_request(&no_query).unwrap();
        assert!(empty.is_empty());
        assert_eq!(
            parse_request_resource(&no_query).unwrap(),
            ResourceRequest::Stream(id, String::new())
        );

        let extra = format!("{base}?u=a&b=c").parse::<Uri>().unwrap();
        assert!(parse_stream_request(&extra).is_err());
    }
    use haven_application::wire::SessionEngineDto;
    use haven_domain::entities::Resource;
    use haven_domain::enums::{
        AvailabilitySource, MediaType, ResourceType, StorageProviderType, StorageStatus,
    };
    use haven_domain::ids::{MediaItemId, ResourceId, StorageLocationId};
    use std::io::{Read, Write};

    fn file_with(data: &[u8], extension: &str) -> (tempfile::TempDir, std::path::PathBuf) {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(format!("asset.{extension}"));
        let mut file = File::create(&path).unwrap();
        file.write_all(data).unwrap();
        (dir, path)
    }

    #[test]
    fn uri_accepts_only_exact_session_uuid_shape() {
        let id = Uuid::new_v4();
        let valid = format!("haven-resource://session/{id}");
        assert_eq!(parse_resource_uri(&valid).unwrap(), id);
        assert_eq!(parse_resource_session_id(&valid).unwrap(), id.to_string());
        for invalid in [
            format!("haven-resource://other/{id}"),
            format!("http://session/{id}"),
            format!("haven-resource://user@session/{id}"),
            format!("haven-resource://session:443/{id}"),
            format!("haven-resource://session/{id}/extra"),
            format!("haven-resource://session/./{id}"),
            format!("haven-resource://session/%2F{id}"),
            format!("haven-resource://session/%5C{id}"),
            "haven-resource://session/not-a-uuid".to_owned(),
            format!("haven-resource://session/{id}?x=1"),
            format!("haven-resource://session/{id}#secret"),
            format!("haven-resource://session/{}", id.to_string().to_uppercase()),
        ] {
            assert!(parse_resource_uri(&invalid).is_err(), "accepted {invalid}");
        }
    }

    #[test]
    fn comic_uri_and_windows_compatibility_forms_are_exact() {
        let id = Uuid::new_v4();
        let native = format!("haven-resource://comic-page/{id}");
        assert_eq!(parse_comic_page_uri(&native).unwrap(), id);

        let windows_session = format!("http://haven-resource.session/{id}")
            .parse::<Uri>()
            .unwrap();
        let windows_comic = format!("http://haven-resource.comic-page/{id}")
            .parse::<Uri>()
            .unwrap();
        assert_eq!(
            parse_request_resource(&windows_session).unwrap(),
            ResourceRequest::Session(id)
        );
        assert_eq!(
            parse_request_resource(&windows_comic).unwrap(),
            ResourceRequest::ComicPage(id)
        );

        for invalid in [
            format!("haven-resource://comic-page/{id}/extra"),
            format!("haven-resource://comic-page/{id}?x=1"),
            format!("haven-resource://comic-page/%2F{id}"),
            format!(
                "haven-resource://comic-page/{}",
                id.to_string().to_uppercase()
            ),
        ] {
            assert!(
                parse_comic_page_uri(&invalid).is_err(),
                "accepted {invalid}"
            );
        }
    }

    #[test]
    fn subtitle_uri_requires_two_canonical_uuids_and_no_query() {
        let session_id = Uuid::new_v4();
        let track_id = Uuid::new_v4();
        let native = format!("haven-resource://session/{session_id}/subtitle/{track_id}")
            .parse::<Uri>()
            .unwrap();
        assert_eq!(
            parse_request_resource(&native).unwrap(),
            ResourceRequest::Subtitle(session_id, track_id)
        );
        let windows = format!("http://haven-resource.session/{session_id}/subtitle/{track_id}")
            .parse::<Uri>()
            .unwrap();
        assert_eq!(
            parse_request_resource(&windows).unwrap(),
            ResourceRequest::Subtitle(session_id, track_id)
        );

        for (case, invalid) in [
            format!("haven-resource://session/{session_id}/subtitle/{track_id}?x=1"),
            format!(
                "haven-resource://session/{session_id}/subtitle/{}",
                track_id.to_string().to_uppercase()
            ),
            format!("haven-resource://session/{session_id}/subtitle/{track_id}/extra"),
            format!("haven-resource://session/{session_id}/subtitle/%2F{track_id}"),
            format!("haven-resource://other/{session_id}/subtitle/{track_id}"),
        ]
        .into_iter()
        .enumerate()
        {
            let uri = invalid.parse::<Uri>().unwrap();
            assert!(
                parse_request_resource(&uri).is_err(),
                "accepted invalid subtitle URI case {case}"
            );
        }
    }

    #[test]
    fn artwork_uri_accepts_only_supported_width_variants() {
        let native = "haven-resource://artwork/opaque-id".parse::<Uri>().unwrap();
        assert_eq!(
            parse_request_resource(&native).unwrap(),
            ResourceRequest::Artwork {
                id: "opaque-id".into(),
                variant: None,
            }
        );
        let width = "haven-resource://artwork/opaque-id?w=200"
            .parse::<Uri>()
            .unwrap();
        assert_eq!(
            parse_request_resource(&width).unwrap(),
            ResourceRequest::Artwork {
                id: "opaque-id".into(),
                variant: Some(200),
            }
        );
        for invalid in [
            "haven-resource://artwork/opaque-id?w=600",
            "haven-resource://artwork/opaque-id?w=200&x=1",
            "haven-resource://artwork/opaque-id?width=200",
            "haven-resource://artwork/opaque-id/extra",
            "haven-resource://artwork/../secret",
        ] {
            let uri = invalid.parse::<Uri>().unwrap();
            assert!(parse_request_resource(&uri).is_err(), "accepted {invalid}");
        }
        let invalid_query = "haven-resource://artwork/opaque-id?w=600"
            .parse::<Uri>()
            .unwrap();
        assert_eq!(
            parse_request_resource(&invalid_query).unwrap_err(),
            ResourceUriError::InvalidArtworkQuery
        );
    }

    #[test]
    fn byte_ranges_cover_three_forms_and_rejections() {
        assert_eq!(parse_byte_range(None, 10), Ok(None));
        assert_eq!(
            parse_byte_range(Some("bytes=2-5"), 10),
            Ok(Some(ByteRange { start: 2, end: 5 }))
        );
        assert_eq!(
            parse_byte_range(Some("bytes=2-"), 10),
            Ok(Some(ByteRange { start: 2, end: 9 }))
        );
        assert_eq!(
            parse_byte_range(Some("bytes=-3"), 10),
            Ok(Some(ByteRange { start: 7, end: 9 }))
        );
        assert_eq!(
            parse_byte_range(Some("bytes=7-99"), 10),
            Ok(Some(ByteRange { start: 7, end: 9 }))
        );
        for header in [
            "Bytes=0-1",
            "items=0-1",
            "bytes=0-1,2-3",
            "bytes=",
            "bytes=-0",
        ] {
            assert!(parse_byte_range(Some(header), 10).is_err());
        }
        assert!(parse_byte_range(Some("bytes=10-"), 10).is_err());
        assert!(parse_byte_range(Some("bytes=0-1"), 0).is_err());
    }

    #[test]
    fn remote_byte_ranges_are_single_forward_ranges_only() {
        assert_eq!(parse_remote_byte_range(None), Ok(None));
        assert_eq!(
            parse_remote_byte_range(Some("bytes=2-5")),
            Ok(Some(RemoteByteRange {
                start: 2,
                end: Some(5),
            }))
        );
        assert_eq!(
            parse_remote_byte_range(Some("bytes=2-")),
            Ok(Some(RemoteByteRange {
                start: 2,
                end: None,
            }))
        );
        for header in [
            "bytes=-500",
            "bytes=5-2",
            "bytes=0-1,2-3",
            "items=0-1",
            "bytes=",
        ] {
            assert!(
                parse_remote_byte_range(Some(header)).is_err(),
                "accepted {header}"
            );
        }
    }

    #[test]
    fn remote_session_response_exposes_only_bounded_body_metadata() {
        let body = RemoteSessionBody {
            mime_type: "application/pdf".into(),
            bytes: b"pdf-page".to_vec(),
            total_size: 100,
            content_range: Some(haven_application::services::ports::RemoteContentRange {
                start: 10,
                end: 17,
                total: 100,
            }),
            accept_ranges: true,
        };
        let response = response_for_remote_session(&Method::GET, body.clone(), None);
        assert_eq!(response.status().as_u16(), 206);
        assert_eq!(response.body(), b"pdf-page");
        assert_eq!(response.headers().get("content-length").unwrap(), "8");
        assert_eq!(
            response.headers().get("content-type").unwrap(),
            "application/pdf"
        );
        assert_eq!(response.headers().get("accept-ranges").unwrap(), "bytes");
        assert_eq!(
            response.headers().get("content-range").unwrap(),
            "bytes 10-17/100"
        );
        assert!(response.headers().get("x-haven-source-url").is_none());

        let response = response_for_remote_session(&Method::HEAD, body, Some("tauri://localhost"));
        assert_eq!(response.status().as_u16(), 206);
        assert!(response.body().is_empty());
        assert_eq!(response.headers().get("content-length").unwrap(), "8");
        assert_eq!(
            response
                .headers()
                .get("access-control-allow-origin")
                .unwrap(),
            "tauri://localhost"
        );
    }

    #[test]
    fn remote_session_response_rejects_inconsistent_range_metadata() {
        let response = response_for_remote_session(
            &Method::GET,
            RemoteSessionBody {
                mime_type: "application/pdf".into(),
                bytes: b"short".to_vec(),
                total_size: 100,
                content_range: Some(haven_application::services::ports::RemoteContentRange {
                    start: 10,
                    end: 20,
                    total: 100,
                }),
                accept_ranges: true,
            },
            None,
        );
        assert_eq!(response.status().as_u16(), 502);
        assert!(response.body().is_empty());
        assert!(response.headers().get("content-range").is_none());
    }

    #[test]
    fn direct_stream_validation_is_fail_closed_for_mime_and_range_semantics() {
        let requested = RemoteByteRange {
            start: 10,
            end: Some(19),
        };
        let content_range = StreamContentRange {
            start: 10,
            end: 19,
            total: 100,
        };
        assert!(validate_direct_stream_headers(
            206,
            Some("video/mp4; codecs=avc1"),
            Some(requested),
            Some(content_range),
        )
        .is_ok());
        assert!(validate_direct_stream_headers(200, Some("video/mp4"), None, None,).is_ok());
        assert!(
            validate_direct_stream_headers(200, Some("Video/WebM; codecs=vp9"), None, None,)
                .is_ok()
        );
        assert!(validate_direct_stream_headers(200, None, None, None,).is_ok());

        for mime in [
            Some("text/html"),
            Some("application/json; charset=utf-8"),
            Some("text/plain"),
            Some("audio/mpeg"),
            Some("video/mp4, text/html"),
        ] {
            assert!(
                validate_direct_stream_headers(200, mime, None, None).is_err(),
                "non-video MIME must be rejected: {mime:?}"
            );
        }
        assert!(validate_direct_stream_headers(
            200,
            Some("video/mp4"),
            Some(requested),
            Some(content_range),
        )
        .is_err());
        assert!(
            validate_direct_stream_headers(206, Some("video/mp4"), None, Some(content_range),)
                .is_err()
        );
        assert!(validate_direct_stream_headers(
            206,
            Some("video/mp4"),
            Some(requested),
            Some(StreamContentRange {
                start: 10,
                end: 18,
                total: 100,
            }),
        )
        .is_err());
    }

    #[test]
    fn direct_stream_body_must_match_content_range_exactly() {
        let range = StreamContentRange {
            start: 10,
            end: 19,
            total: 100,
        };
        assert!(validate_direct_stream_body_len(range, 10).is_ok());
        assert!(validate_direct_stream_body_len(range, 9).is_err());
        assert!(validate_direct_stream_body_len(range, 11).is_err());
    }

    fn spawn_stream_fixture(
        status: &str,
        headers: &str,
        body: &[u8],
    ) -> (String, std::net::SocketAddr) {
        let listener = std::net::TcpListener::bind(("127.0.0.1", 0)).unwrap();
        let address = listener.local_addr().unwrap();
        let response = format!(
            "HTTP/1.1 {status}\r\n{headers}Content-Length: {}\r\nConnection: close\r\n\r\n",
            body.len()
        );
        let body = body.to_vec();
        std::thread::spawn(move || {
            if let Ok((mut stream, _)) = listener.accept() {
                let mut request = [0_u8; 2048];
                let _ = stream.read(&mut request);
                let _ = stream.write_all(response.as_bytes());
                let _ = stream.write_all(&body);
            }
        });
        (
            format!("http://stream.example.test:{}/video.mp4", address.port()),
            address,
        )
    }

    fn direct_stream_facts(url: &str) -> crate::stream_registry::StreamGrantFacts {
        crate::stream_registry::StreamGrantFacts {
            work_id: "w".into(),
            edition_id: "e".into(),
            media_item_id: "m".into(),
            mime_type: Some("video/mp4".into()),
            is_hls: false,
            progress: None,
            upstream_url: url.to_owned(),
        }
    }

    #[tokio::test]
    async fn direct_stream_rejects_full_response_to_a_range_request() {
        let (url, address) = spawn_stream_fixture("200 OK", "Content-Type: video/mp4\r\n", b"abc");
        let registry = crate::stream_registry::StreamRegistry::new();
        let grant = registry
            .register_fixture(direct_stream_facts(&url), &url, "main")
            .unwrap();
        let inner = registry.lookup(&grant.to_string(), "main").unwrap();
        let error = serve_stream_with_resolution(
            &inner,
            &grant.to_string(),
            "",
            Some("bytes=0-2"),
            None,
            StreamResolution::Fixture(address),
        )
        .await
        .unwrap_err();
        assert_eq!(error.code().as_str(), "RESOURCE_UNAVAILABLE");
    }

    #[tokio::test]
    async fn direct_stream_rejects_html_even_when_status_is_success() {
        let (url, address) =
            spawn_stream_fixture("200 OK", "Content-Type: text/html\r\n", b"<html>");
        let registry = crate::stream_registry::StreamRegistry::new();
        let grant = registry
            .register_fixture(direct_stream_facts(&url), &url, "main")
            .unwrap();
        let inner = registry.lookup(&grant.to_string(), "main").unwrap();
        let error = serve_stream_with_resolution(
            &inner,
            &grant.to_string(),
            "",
            None,
            None,
            StreamResolution::Fixture(address),
        )
        .await
        .unwrap_err();
        assert_eq!(error.code().as_str(), "RESOURCE_UNAVAILABLE");
    }

    #[tokio::test]
    async fn direct_stream_accepts_matching_partial_response() {
        let (url, address) = spawn_stream_fixture(
            "206 Partial Content",
            "Content-Type: video/mp4\r\nContent-Range: bytes 0-2/3\r\n",
            b"abc",
        );
        let registry = crate::stream_registry::StreamRegistry::new();
        let grant = registry
            .register_fixture(direct_stream_facts(&url), &url, "main")
            .unwrap();
        let inner = registry.lookup(&grant.to_string(), "main").unwrap();
        let response = serve_stream_with_resolution(
            &inner,
            &grant.to_string(),
            "",
            Some("bytes=0-2"),
            None,
            StreamResolution::Fixture(address),
        )
        .await
        .unwrap();
        assert_eq!(response.status().as_u16(), 206);
        assert_eq!(response.body(), b"abc");
        assert_eq!(
            response.headers().get("content-range").unwrap(),
            "bytes 0-2/3"
        );
    }

    #[test]
    fn responses_cover_statuses_headers_and_body_cap() {
        let (_dir, path) = file_with(b"0123456789", "mp4");
        let response =
            response_for_open_file(&Method::GET, File::open(&path).unwrap(), &path, None);
        assert_eq!(response.status().as_u16(), 200);
        assert_eq!(response.body(), b"0123456789");
        assert_eq!(response.headers().get("content-length").unwrap(), "10");
        assert_eq!(
            response.headers().get("x-content-type-options").unwrap(),
            "nosniff"
        );

        let response = response_for_open_file(
            &Method::GET,
            File::open(&path).unwrap(),
            &path,
            Some("bytes=2-5"),
        );
        assert_eq!(response.status().as_u16(), 206);
        assert_eq!(response.body(), b"2345");
        assert_eq!(
            response.headers().get("content-range").unwrap(),
            "bytes 2-5/10"
        );
        assert_eq!(response.headers().get("accept-ranges").unwrap(), "bytes");

        let response = response_for_open_file(
            &Method::GET,
            File::open(&path).unwrap(),
            &path,
            Some("bytes=10-"),
        );
        assert_eq!(response.status().as_u16(), 416);
        assert!(response.body().is_empty());
        assert_eq!(
            response.headers().get("content-range").unwrap(),
            "bytes */10"
        );
        assert_eq!(response.headers().get("content-length").unwrap(), "0");

        let response =
            response_for_open_file(&Method::HEAD, File::open(&path).unwrap(), &path, None);
        assert_eq!(response.status().as_u16(), 200);
        assert!(response.body().is_empty());
        assert_eq!(response.headers().get("content-length").unwrap(), "10");

        let response =
            response_for_open_file(&Method::POST, File::open(&path).unwrap(), &path, None);
        assert_eq!(response.status().as_u16(), 405);
        assert!(response.body().is_empty());

        let response = response_for_open_file(
            &Method::GET,
            File::open(&path).unwrap(),
            &path,
            Some("bytes=0-"),
        );
        // Small test files are served normally; this also exercises the exact
        // cap boundary through the dedicated oversized test below.
        assert_eq!(response.status().as_u16(), 206);
    }

    #[test]
    fn oversized_single_read_is_rejected() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("large.mp4");
        let file = File::create(&path).unwrap();
        file.set_len(MAX_RESPONSE_BYTES + 1).unwrap();
        let response =
            response_for_open_file(&Method::GET, File::open(&path).unwrap(), &path, None);
        assert_eq!(response.status().as_u16(), 413);
        assert!(response.body().is_empty());
        assert_eq!(response.headers().get("accept-ranges").unwrap(), "bytes");
    }

    #[test]
    fn subtitle_response_is_bounded_and_does_not_offer_ranges() {
        let (_dir, path) = file_with(b"1\n00:00:00,000 --> 00:00:01,000\nhello\n", "srt");
        let response =
            response_for_subtitle_file(&Method::GET, File::open(&path).unwrap(), &path, None, None);
        assert_eq!(response.status().as_u16(), 200);
        assert_eq!(
            response.body(),
            b"1\n00:00:00,000 --> 00:00:01,000\nhello\n"
        );
        assert_eq!(
            response.headers().get("content-type").unwrap(),
            "text/plain; charset=utf-8"
        );
        assert!(response.headers().get("accept-ranges").is_none());

        let response = response_for_subtitle_file(
            &Method::GET,
            File::open(&path).unwrap(),
            &path,
            Some("bytes=0-1"),
            None,
        );
        assert_eq!(response.status().as_u16(), 416);
        assert!(response.body().is_empty());
    }

    #[test]
    fn mime_allowlist_is_narrow() {
        assert_eq!(mime_for_extension(".MP4"), "video/mp4");
        assert_eq!(mime_for_extension("webp"), "image/webp");
        assert_eq!(mime_for_extension("TXT"), "text/plain; charset=utf-8");
        assert_eq!(mime_for_extension("exe"), "application/octet-stream");
        assert_eq!(mime_for_path(Path::new("cover.JPEG")), "image/jpeg");
    }

    #[test]
    fn cors_allows_exact_tauri_origins_only() {
        let (_dir, path) = file_with(b"hello", "txt");
        let response = response_for_open_file_with_origin(
            &Method::GET,
            File::open(&path).unwrap(),
            &path,
            None,
            Some("tauri://localhost"),
        );
        assert_eq!(
            response.headers().get("content-type").unwrap(),
            "text/plain; charset=utf-8"
        );
        assert_eq!(
            response
                .headers()
                .get("access-control-allow-origin")
                .unwrap(),
            "tauri://localhost"
        );
        assert_eq!(response.headers().get("vary").unwrap(), "Origin");

        let response = response_for_open_file_with_origin(
            &Method::GET,
            File::open(&path).unwrap(),
            &path,
            None,
            Some("https://evil.example"),
        );
        assert!(response
            .headers()
            .get("access-control-allow-origin")
            .is_none());
        assert_eq!(response.headers().get("vary").unwrap(), "Origin");
    }

    #[test]
    fn comic_page_response_uses_magic_mime_and_never_caches() {
        let response = response_for_comic_page(
            &Method::GET,
            ComicPageBody {
                mime_type: ComicImageMime::Png,
                bytes: b"png-bytes".to_vec(),
            },
            None,
            Some("http://tauri.localhost"),
        );
        assert_eq!(response.status().as_u16(), 200);
        assert_eq!(response.body(), b"png-bytes");
        assert_eq!(response.headers().get("content-type").unwrap(), "image/png");
        assert_eq!(response.headers().get("cache-control").unwrap(), "no-store");
        assert_eq!(
            response.headers().get("x-content-type-options").unwrap(),
            "nosniff"
        );
        assert_eq!(
            response
                .headers()
                .get("access-control-allow-origin")
                .unwrap(),
            "http://tauri.localhost"
        );

        let response = response_for_comic_page(
            &Method::HEAD,
            ComicPageBody {
                mime_type: ComicImageMime::Webp,
                bytes: b"webp-bytes".to_vec(),
            },
            None,
            None,
        );
        assert_eq!(response.status().as_u16(), 405);
        assert!(response.body().is_empty());
        assert_eq!(response.headers().get("content-length").unwrap(), "0");
        assert_eq!(response.headers().get("allow").unwrap(), "GET");

        let response = response_for_comic_page(
            &Method::GET,
            ComicPageBody {
                mime_type: ComicImageMime::Jpeg,
                bytes: b"jpeg".to_vec(),
            },
            Some("bytes=0-1"),
            None,
        );
        assert_eq!(response.status().as_u16(), 416);
        assert!(response.body().is_empty());
        assert_eq!(response.headers().get("cache-control").unwrap(), "no-store");
        assert_eq!(response.headers().get("vary").unwrap(), "Origin");

        let response = response_for_comic_page(
            &Method::GET,
            ComicPageBody {
                mime_type: ComicImageMime::Jpeg,
                bytes: b"jpeg".to_vec(),
            },
            Some("bytes=0-1"),
            Some("tauri://localhost"),
        );
        assert_eq!(
            response
                .headers()
                .get("access-control-allow-origin")
                .unwrap(),
            "tauri://localhost"
        );
    }

    fn binding_fixture(root: &Path) -> (PreparedSession, Resource, StorageLocation) {
        let media_item_id = MediaItemId::new();
        let resource_id = ResourceId::new();
        let storage_id = StorageLocationId::new();
        let file = root.join("movie.mkv");
        std::fs::write(&file, b"movie").unwrap();
        let canonical_root = std::fs::canonicalize(root).unwrap();
        let canonical_file = std::fs::canonicalize(file).unwrap();
        let prepared = PreparedSession {
            work_id: "w".into(),
            edition_id: "e".into(),
            media_item_id: media_item_id.to_string(),
            engine: SessionEngineDto::Playback,
            resource_id,
            storage_location_id: Some(storage_id),
            canonical_root: Some(canonical_root.clone()),
            canonical_file: Some(canonical_file),
            subtitle_tracks: Vec::new(),
            source: PreparedSessionSource::Local,
            mime_type: Some("video/x-matroska".into()),
            media_type: MediaType::Movie,
            resource_type: ResourceType::LocalFile,
            comic_pages: None,
            progress: None,
        };
        let resource = Resource {
            id: resource_id,
            media_item_id,
            resource_type: ResourceType::LocalFile,
            source_id: None,
            storage_location_id: Some(storage_id),
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
            created_at: haven_common::UtcMillis(1),
            updated_at: haven_common::UtcMillis(1),
        };
        let storage = StorageLocation {
            id: storage_id,
            provider_type: StorageProviderType::Local,
            display_name: "local".into(),
            root_ref: canonical_root.to_string_lossy().into_owned(),
            credential_ref: None,
            status: StorageStatus::Connected,
            created_at: haven_common::UtcMillis(1),
            updated_at: haven_common::UtcMillis(1),
        };
        (prepared, resource, storage)
    }

    #[test]
    fn resolver_rejects_stale_resource_binding() {
        let root = tempfile::tempdir().unwrap();
        let (prepared, mut resource, storage) = binding_fixture(root.path());
        resource.media_item_id = MediaItemId::new();
        assert_eq!(
            validate_resource_binding(&prepared, &resource)
                .unwrap_err()
                .code()
                .as_str(),
            "SESSION_STALE"
        );
        assert!(validate_storage_binding(
            &prepared,
            &storage,
            prepared.canonical_root.as_deref().unwrap(),
        )
        .is_ok());
    }

    #[test]
    fn resolver_rejects_disconnected_storage() {
        let root = tempfile::tempdir().unwrap();
        let (prepared, resource, mut storage) = binding_fixture(root.path());
        storage.status = StorageStatus::Disconnected;
        assert!(validate_resource_binding(&prepared, &resource).is_ok());
        assert_eq!(
            validate_storage_binding(
                &prepared,
                &storage,
                prepared.canonical_root.as_deref().unwrap(),
            )
            .unwrap_err()
            .code()
            .as_str(),
            "RESOURCE_UNAVAILABLE"
        );
    }

    #[test]
    fn resolver_accepts_storage_object_locator_and_keeps_remote_denied() {
        let root = tempfile::tempdir().unwrap();
        let (prepared, mut resource, storage) = binding_fixture(root.path());
        resource.locator = ResourceLocator::StorageObject {
            provider_id: storage.id,
            object_id: "d59b17d8.epub".into(),
            path_hint: Some("books/d59b17d8.epub".into()),
        };
        assert_eq!(
            validate_resource_binding(&prepared, &resource).unwrap(),
            PathBuf::from("books/d59b17d8.epub")
        );
        // 无 path_hint 时退化用对象名（导入侧保证两者一致）。
        resource.locator = ResourceLocator::StorageObject {
            provider_id: storage.id,
            object_id: "book.epub".into(),
            path_hint: None,
        };
        assert_eq!(
            validate_resource_binding(&prepared, &resource).unwrap(),
            PathBuf::from("book.epub")
        );
        // 远端定位仍拒绝。
        resource.locator = ResourceLocator::Http {
            url: "https://example.invalid/a.epub".into(),
        };
        assert_eq!(
            validate_resource_binding(&prepared, &resource)
                .unwrap_err()
                .code()
                .as_str(),
            "SECURITY_POLICY_DENIED"
        );
    }

    #[test]
    fn resolver_rejects_rebound_storage_root() {
        let root = tempfile::tempdir().unwrap();
        let (prepared, _resource, mut storage) = binding_fixture(root.path());
        let rebound = tempfile::tempdir().unwrap();
        storage.root_ref = rebound.path().to_string_lossy().into_owned();
        let rebound_root = std::fs::canonicalize(rebound.path()).unwrap();
        assert_eq!(
            validate_storage_binding(&prepared, &storage, &rebound_root)
                .unwrap_err()
                .code()
                .as_str(),
            "SESSION_STALE"
        );
    }
}
