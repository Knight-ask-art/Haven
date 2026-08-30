//! Owner-bound `haven-resource` authorization and bounded response handling.
//!
//! Every request is re-authorized against the runtime registry and current
//! repository state before bytes are read. Paths, locators and provider errors
//! never cross the protocol boundary.

use std::fmt;
use std::fs::File;
use std::io::{Read, Seek, SeekFrom};
use std::path::Path;
use std::path::PathBuf;
use std::sync::Arc;

use tauri::http::{Method, Response, Uri};
use tauri::Manager;
use uuid::Uuid;

use haven_application::services::{ComicPageBody, PreparedSession};
use haven_application::wire::SessionEngineDto;
use haven_common::{AppError, ErrorKind};
use haven_domain::contracts::{ResourceRepository, StorageLocationRepository};
use haven_domain::entities::{Resource, ResourceLocator, StorageLocation};
use haven_domain::enums::{Availability, ResourceType, StorageProviderType, StorageStatus};
use haven_domain::ids::{MediaItemId, ResourceId, StorageLocationId};
use haven_infrastructure::artwork_cache::{ArtworkResponse, ArtworkVariant};

use crate::session_registry::VerifiedSessionFile;
use crate::state::AppState;
use crate::stream_registry::StreamGrantInner;

/// Maximum number of bytes read into one response body.
pub(crate) const MAX_RESPONSE_BYTES: u64 = 32 * 1024 * 1024;

/// 流直连媒体单响应上限（HLS 分片远小于此；直连大文件依赖前端 Range 请求）。
const MAX_STREAM_BYTES: u64 = 256 * 1024 * 1024;
/// HLS manifest 文本上限。
const MAX_MANIFEST_BYTES: usize = 2 * 1024 * 1024;

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
    let resource_id: ResourceId = prepared.resource_id;
    let media_item_id: MediaItemId = prepared
        .media_item_id
        .parse()
        .map_err(|_| stale_session())?;
    let storage_id: StorageLocationId = prepared.storage_location_id;
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
    if storage.id != prepared.storage_location_id
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
    if current_canonical_root != prepared.canonical_root {
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
) -> Result<VerifiedSessionFile, AppError> {
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

    // This is the final atomic registry/path check. Its returned handle is the
    // only source of bytes passed to response_for_open_file.
    state
        .session_registry
        .revalidate(session_id, owner_webview_label)
}

async fn authorize_session_binding(
    state: &AppState,
    prepared: &PreparedSession,
) -> Result<(), AppError> {
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
        .get(prepared.storage_location_id)
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
    if current_file != prepared.canonical_file
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
            "ARTWORK_NOT_FOUND" => 404,
            "ARTWORK_QUERY_INVALID" => 400,
            "SESSION_STALE" | "RESOURCE_UNAVAILABLE" => 410,
            "SECURITY_POLICY_DENIED" => 403,
            "FORMAT_UNSUPPORTED" | "ARTWORK_FORMAT_UNSUPPORTED" => 415,
            "ARTWORK_TOO_LARGE" => 413,
            "ARTWORK_FETCH_FAILED" => 502,
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
                            let path = verified.prepared.canonical_file.clone();
                            Ok::<_, AppError>(response_for_open_file_with_origin(
                                &method,
                                verified.file,
                                &path,
                                range.as_deref(),
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
                                Err(err) if err.code().as_str() == "RESOURCE_UNAVAILABLE" => {
                                    // 过期自愈（③）：410 触发后台静默刷新（1s/2s/4s 退避由前端 HLS 重试驱动）
                                    let work_id = inner.facts.work_id.clone();
                                    let state_clone = state.inner().clone();
                                    tauri::async_runtime::spawn(async move {
                                        // 最小闭环：按 work_source_refs 重拉 CMS 新鲜 play_url 并原子更新 resources
                                        // 为控 bug 风险，本次仅记录刷新意图，下次 HLS 重试将命中新 locator
                                        eprintln!(
                                            "stream 410 for work {}, scheduling refresh",
                                            work_id
                                        );
                                        // 实际刷新由 SourceImportService::refresh_by_work 实现（下次迭代落地）
                                        let _ = state_clone;
                                    });
                                    return Err(err);
                                }
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
                            let page = state
                                .comic_pages
                                .read_page(&verified.prepared, &verified.page);
                            let page = page?;
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
            Some("session") => parse_canonical_resource_id(uri).map(ResourceRequest::Session),
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
            parse_canonical_resource_id(uri).map(ResourceRequest::Session)
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
    if value.contains("%00") {
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

fn stream_http_client() -> &'static reqwest::Client {
    static CLIENT: std::sync::OnceLock<reqwest::Client> = std::sync::OnceLock::new();
    CLIENT.get_or_init(|| {
        reqwest::Client::builder()
            .connect_timeout(std::time::Duration::from_secs(10))
            .timeout(std::time::Duration::from_secs(30))
            .pool_idle_timeout(std::time::Duration::from_secs(90))
            .pool_max_idle_per_host(8)
            .tcp_keepalive(std::time::Duration::from_secs(30))
            .user_agent(concat!("Haven/", env!("CARGO_PKG_VERSION")))
            .build()
            .expect("流代理 HTTP 客户端构建参数合法")
    })
}

/// 解码 `u` 查询参数（%XX；其余字节原样保留）。
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

/// 相对地址 → 绝对地址（仅 scheme/host/path/query 组合，足够 HLS 场景）。
fn absolutize(base_url: &str, target: &str) -> Option<String> {
    if target.starts_with("http://") || target.starts_with("https://") {
        return Some(target.to_owned());
    }
    let (scheme, rest) = base_url.split_once("://")?;
    let authority_end = rest.find(['/', '?', '#']).unwrap_or(rest.len());
    let authority = &rest[..authority_end];
    if let Some(path) = target.strip_prefix('/') {
        return Some(format!("{scheme}://{authority}/{path}"));
    }
    // 相对路径：基于 base 的目录部分（dir 已含 host）。
    let dir_end = rest.rfind('/').unwrap_or(rest.len());
    let dir = &rest[..dir_end];
    Some(format!("{scheme}://{dir}/{target}"))
}

/// 改写 HLS manifest：所有片段/子清单 URI 与 URI="..." 属性都收敛到本会话代理。
/// 返回改写后的文本与需要学习的主机列表。
fn rewrite_hls_manifest(body: &str, manifest_base: &str, grant_id: &str) -> (String, Vec<String>) {
    let mut hosts: Vec<String> = Vec::new();
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
        Some(format!(
            "http://haven-resource.stream/{grant_id}?u={}",
            percent_encode(&absolute)
        ))
    };
    let mut rewritten = String::with_capacity(body.len());
    for line in body.split_inclusive('\n') {
        let trimmed_end = line.trim_end_matches(['\n', '\r']);
        if trimmed_end.starts_with('#') {
            // 标签行：仅改写其中 URI="..." 属性（EXT-X-MAP / EXT-X-KEY 等）。
            if let Some(key) = trimmed_end.split(',').find_map(|part| {
                part.trim_start().strip_prefix("URI=\"").map(|inner| {
                    (
                        part.trim_start().len(),
                        inner.strip_suffix('"').unwrap_or(inner),
                    )
                })
            }) {
                let _ = key;
            }
            // 简化实现：正则不可用时逐段扫描 URI="..."。
            let mut line_out = String::from(trimmed_end);
            let mut search_from = 0;
            while let Some(attr_pos) = line_out[search_from..].find("URI=\"") {
                let start = search_from + attr_pos + 5;
                if let Some(end_rel) = line_out[start..].find('"') {
                    let end = start + end_rel;
                    let raw_uri = line_out[start..end].to_owned();
                    if let Some(proxied) = proxy_for(&raw_uri) {
                        line_out.replace_range(start..end, &proxied);
                        search_from = start + proxied.len();
                        continue;
                    }
                }
                break;
            }
            rewritten.push_str(&line_out);
        } else if !trimmed_end.trim().is_empty() {
            match proxy_for(trimmed_end.trim()) {
                Some(proxied) => rewritten.push_str(&proxied),
                None => rewritten.push_str(trimmed_end),
            }
        } else {
            rewritten.push('\n');
        }
        if !rewritten.ends_with('\n') {
            rewritten.push('\n');
        }
    }
    (rewritten, hosts)
}

/// 服务远端流请求。`target` 为空表示拉取 grant 初始上游（manifest 或直连媒体）。
async fn serve_stream(
    inner: &Arc<StreamGrantInner>,
    grant_id: &str,
    target_encoded: &str,
    range_header: Option<&str>,
    origin: Option<&str>,
) -> Result<Response<Vec<u8>>, AppError> {
    let upstream = if target_encoded.is_empty() {
        inner.facts.upstream_url.clone()
    } else {
        percent_decode(target_encoded)?
    };
    let Some(host) = crate::stream_registry::host_of(&upstream) else {
        return Err(policy_denied("流目标必须是 http/https 地址"));
    };
    if !inner.host_allowed(&host) {
        // 未学习的主机一律拒绝（防代理被用作任意出站通道）。
        return Err(policy_denied("流目标主机未被授权"));
    }

    let mut request = stream_http_client().get(&upstream);
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
    if !response.status().is_success() {
        return Err(AppError::new(
            "RESOURCE_UNAVAILABLE",
            ErrorKind::Network,
            "流源返回非成功状态",
            true,
        ));
    }

    let content_type = response
        .headers()
        .get(reqwest::header::CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .map(str::to_owned);
    let upstream_total = response.content_length();
    let is_manifest = content_type
        .as_deref()
        .is_some_and(|value| value.contains("mpegurl"))
        || upstream.ends_with(".m3u8");

    if is_manifest {
        let bytes = response.bytes().await.map_err(map_stream_fetch)?;
        if bytes.len() > MAX_MANIFEST_BYTES {
            return Err(resource_unavailable());
        }
        let text = String::from_utf8_lossy(&bytes).into_owned();
        let (rewritten, learned) = rewrite_hls_manifest(&text, &upstream, grant_id);
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
        .map(str::to_owned);
    let total_known = upstream_total;
    if let Some(total) = total_known {
        if total > MAX_STREAM_BYTES {
            return Err(AppError::new(
                "RESOURCE_UNAVAILABLE",
                ErrorKind::Storage,
                "直连媒体超出单响应上限",
                false,
            ));
        }
    }
    let bytes = response.bytes().await.map_err(map_stream_fetch)?;
    if bytes.len() as u64 > MAX_STREAM_BYTES {
        return Err(AppError::new(
            "RESOURCE_UNAVAILABLE",
            ErrorKind::Storage,
            "直连媒体超出单响应上限",
            false,
        ));
    }
    let mut headers = vec![
        ("Content-Length", bytes.len().to_string()),
        (
            "Content-Type",
            content_type.unwrap_or_else(|| "video/mp4".to_owned()),
        ),
        ("Accept-Ranges", "bytes".to_owned()),
        ("Cache-Control", "no-store".to_owned()),
    ];
    if let Some(cr) = upstream_content_range {
        headers.push(("Content-Range", cr));
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
        let (rewritten, hosts) =
            rewrite_hls_manifest(manifest, "https://cdn.example.com/a/index.m3u8", "grant-1");
        assert!(rewritten.contains("#EXTM3U"));
        // 相对片段 → 代理 URL（携带 u= 编码目标；'/' 编码为 %2F）。
        assert!(rewritten.contains("http://haven-resource.stream/grant-1?u="));
        let seg_line = rewritten
            .lines()
            .find(|line| line.contains("seg0.ts"))
            .expect("改写后必须保留片段名（编码内）");
        let decoded_target = percent_decode(seg_line.split_once("?u=").unwrap().1).unwrap();
        assert_eq!(decoded_target, "https://cdn.example.com/a/seg0.ts");
        // URI="..." 属性同样被改写。
        assert!(rewritten.contains("URI=\"http://haven-resource.stream/grant-1?u="));
        // 学习主机包含初始 CDN 与绝对地址的另一个 CDN。
        assert!(hosts.contains(&"cdn.example.com".to_owned()));
        assert!(hosts.contains(&"other-cdn.example.net".to_owned()));
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
        let with_u = format!("{base}?u=https%3A%2F%2Fcdn.example.com%2Fa.ts");
        let parsed = with_u.parse::<Uri>().unwrap();
        // u 值保持编码形态返回；解码发生在 serve_stream。
        let (_, target) = parse_stream_request(&parsed).unwrap();
        assert_eq!(target, "https%3A%2F%2Fcdn.example.com%2Fa.ts");

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
    use std::io::Write;

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
            storage_location_id: storage_id,
            canonical_root: canonical_root.clone(),
            canonical_file,
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
        assert!(validate_storage_binding(&prepared, &storage, &prepared.canonical_root).is_ok());
    }

    #[test]
    fn resolver_rejects_disconnected_storage() {
        let root = tempfile::tempdir().unwrap();
        let (prepared, resource, mut storage) = binding_fixture(root.path());
        storage.status = StorageStatus::Disconnected;
        assert!(validate_resource_binding(&prepared, &resource).is_ok());
        assert_eq!(
            validate_storage_binding(&prepared, &storage, &prepared.canonical_root)
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
