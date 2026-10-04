//! Google Drive v3 只读适配器（Infrastructure）。
//!
//! 只读：list / stat / alt=media / about。URL 用固定端点拼接，只接受经过
//! validate_drive_object_id 的 opaque ID，不接受调用方给的 URL、header 或字段列表。
//! 令牌每次请求重新构造成一次性 SecretString，不 Clone、不缓存明文；过期凭据在任何
//! 网络 IO 之前被拒绝。响应大小、分页、条目数、字段长度与 Range 形状全部有界，无法
//! 验证的形状返回结构化错误；上游字节、Drive ID 与令牌不进入错误文案或 Debug。
//! 网络收在 GoogleTransport 端口后面，测试注入进程内 fake，无需真实网络即可验证。
use std::sync::Arc;

use async_trait::async_trait;
use haven_application::services::cloud_storage::ports::{
    CLOUD_DRIVE_ROOT_ID, CloudDriveAccount, CloudDriveCredential, CloudDriveFileMetadata,
    CloudDriveFolderPage, CloudDrivePort, validate_drive_object_id, validate_drive_page_token,
};
use haven_application::services::ports::{RemoteByteRange, RemoteContentRange, RemoteSessionBody};
use haven_common::{AppError, ErrorKind, UtcMillis, internal};
use haven_domain::credential::SecretString;

use crate::cloud_drive::http::{
    GoogleMethod, GoogleRequest, GoogleResponse, GoogleTransport, JSON_LIMIT, MEDIA_LIMIT,
};

/// Drive 固定端点；适配器不接受任何调用方提供的地址。
const FILES_URL: &str = "https://www.googleapis.com/drive/v3/files";
const ABOUT_URL: &str = "https://www.googleapis.com/drive/v3/about";
const FOLDER_MIME: &str = "application/vnd.google-apps.folder";
const PDF_MIME: &str = "application/pdf";
const PDF_MAGIC: &[u8] = b"%PDF-";
/// list 与 stat 的条目字段必须一致，否则元数据与列表会漂移。
const FILE_FIELDS: &str = "id,name,mimeType,size,parents,trashed";
const LIST_FIELDS: &str = "nextPageToken,files(id,name,mimeType,size,parents,trashed)";
const ABOUT_FIELDS: &str = "user(permissionId,displayName)";
/// pageSize 与响应条目上限必须同步。
const PAGE_SIZE: &str = "100";
const MAX_PAGE_ENTRIES: usize = 100;
const MAX_PDF_TOTAL_BYTES: u64 = 128 * 1024 * 1024;
const MAX_TEXT_BYTES: usize = 1024;
const MAX_PARENTS: usize = 16;

// 稳定错误码 → 固定 kind 与文案；上游状态码、正文、ID 与令牌都不进入错误。
fn err(code: &'static str) -> AppError {
    let (kind, message, retryable) = match code {
        "CLOUD_DRIVE_UNAUTHORIZED" => {
            (ErrorKind::Unauthorized, "云盘授权已失效，请重新连接", false)
        }
        "CLOUD_DRIVE_FORBIDDEN" => (ErrorKind::Forbidden, "云盘拒绝了该读取请求", false),
        "CLOUD_DRIVE_NOT_FOUND" => (ErrorKind::NotFound, "云盘对象不存在或不可见", false),
        "CLOUD_DRIVE_UNAVAILABLE" => (ErrorKind::Network, "云盘暂时不可用", true),
        "CLOUD_DRIVE_RESPONSE_INVALID" => (ErrorKind::Parse, "云盘返回的数据无法识别", false),
        "CLOUD_DRIVE_UNSUPPORTED_CONTENT" => (ErrorKind::Unsupported, "该文件无法在线读取", false),
        "CLOUD_DRIVE_PDF_TOO_LARGE" => (ErrorKind::Unsupported, "该 PDF 超出在线读取上限", false),
        "RANGE_INVALID" => (ErrorKind::Validation, "远端正文的范围请求无效", false),
        _ => (ErrorKind::Unsupported, "该正文不支持按范围读取", false),
    };
    AppError::new(code, kind, message, retryable)
}

/// CloudDrivePort 的生产实现；传输通过端口注入，便于离线覆盖状态、范围与上限。
pub struct GoogleDriveClient {
    transport: Arc<dyn GoogleTransport>,
}

impl GoogleDriveClient {
    /// 生产组合（`factory`）与 crate 内测试共用；对外只暴露 [`CloudDrivePort`] 行为。
    pub(crate) fn new(transport: Arc<dyn GoogleTransport>) -> Self {
        Self { transport }
    }

    /// 只读请求的唯一入口：凭据检查早于任何网络 IO，且每次重建一次性 bearer。
    async fn call(
        &self,
        credential: &CloudDriveCredential,
        url: reqwest::Url,
        range: Option<RemoteByteRange>,
        max_bytes: usize,
    ) -> Result<GoogleResponse, AppError> {
        if credential.expires_at_ms <= UtcMillis::now().0
            || credential.access_token.expose().is_empty()
        {
            return Err(err("CLOUD_DRIVE_UNAUTHORIZED"));
        }
        let request = GoogleRequest {
            url,
            method: GoogleMethod::Get,
            bearer: Some(SecretString::new(credential.access_token.expose())),
            form: Vec::new(),
            range,
            max_bytes,
        };
        self.transport.send(request).await
    }
}

#[async_trait]
impl CloudDrivePort for GoogleDriveClient {
    async fn list_folder(
        &self,
        credential: &CloudDriveCredential,
        folder_id: &str,
        page_token: Option<&str>,
    ) -> Result<CloudDriveFolderPage, AppError> {
        let folder_id = validate_drive_object_id(folder_id)?;
        let page_token = page_token.map(validate_drive_page_token).transpose()?;
        let mut url = drive_url(FILES_URL)?;
        {
            let mut query = url.query_pairs_mut();
            query.append_pair("q", &format!("'{folder_id}' in parents and trashed=false"));
            query.append_pair("pageSize", PAGE_SIZE);
            query.append_pair("fields", LIST_FIELDS);
            if let Some(token) = page_token {
                query.append_pair("pageToken", token);
            }
        }
        let response = self.call(credential, url, None, JSON_LIMIT).await?;
        ensure_success(&response)?;
        folder_page(&response.bytes)
    }

    async fn stat_file(
        &self,
        credential: &CloudDriveCredential,
        file_id: &str,
    ) -> Result<CloudDriveFileMetadata, AppError> {
        let requested = validate_drive_object_id(file_id)?;
        // 只有 root 别名可以解析成真实 folder ID；其余必须原样匹配。
        let expected = (requested != CLOUD_DRIVE_ROOT_ID).then_some(requested);
        let mut url = drive_url(&format!("{FILES_URL}/{requested}"))?;
        url.query_pairs_mut().append_pair("fields", FILE_FIELDS);
        let response = self.call(credential, url, None, JSON_LIMIT).await?;
        ensure_success(&response)?;
        let raw: RawFile = serde_json::from_slice(&response.bytes).map_err(|_| malformed())?;
        let metadata = file_metadata(raw, expected)?;
        if expected.is_none() && !metadata.is_folder {
            return Err(malformed());
        }
        Ok(metadata)
    }

    async fn read_pdf(
        &self,
        credential: &CloudDriveCredential,
        file_id: &str,
        range: Option<RemoteByteRange>,
    ) -> Result<RemoteSessionBody, AppError> {
        let file_id = validate_drive_object_id(file_id)?;
        if file_id == CLOUD_DRIVE_ROOT_ID {
            return Err(err("CLOUD_DRIVE_UNSUPPORTED_CONTENT"));
        }
        // 元数据只取一次：PDF 判定、大小上限与 total 校验都基于同一份事实。
        let metadata = self.stat_file(credential, file_id).await?;
        if metadata.trashed || metadata.is_folder || metadata.mime_type != PDF_MIME {
            return Err(err("CLOUD_DRIVE_UNSUPPORTED_CONTENT"));
        }
        if metadata.size_bytes.is_none_or(|size| size == 0) {
            return Err(malformed());
        }
        if metadata
            .size_bytes
            .is_some_and(|size| size > MAX_PDF_TOTAL_BYTES)
            || (range.is_none()
                && metadata
                    .size_bytes
                    .is_some_and(|size| size > MEDIA_LIMIT as u64))
        {
            return Err(err("CLOUD_DRIVE_PDF_TOO_LARGE"));
        }
        if range.is_some_and(|range| {
            range.end.is_some_and(|end| end < range.start)
                || metadata
                    .size_bytes
                    .is_some_and(|total| range.start >= total)
        }) {
            return Err(err("RANGE_INVALID"));
        }
        let mut url = drive_url(&format!("{FILES_URL}/{file_id}"))?;
        url.query_pairs_mut().append_pair("alt", "media");
        let response = self.call(credential, url, range, MEDIA_LIMIT).await?;
        pdf_body(response, range, metadata.size_bytes)
    }

    async fn about(
        &self,
        credential: &CloudDriveCredential,
    ) -> Result<CloudDriveAccount, AppError> {
        let mut url = drive_url(ABOUT_URL)?;
        url.query_pairs_mut().append_pair("fields", ABOUT_FIELDS);
        let response = self.call(credential, url, None, JSON_LIMIT).await?;
        ensure_success(&response)?;
        let raw: RawAbout = serde_json::from_slice(&response.bytes).map_err(|_| malformed())?;
        let user = raw.user.ok_or_else(malformed)?;
        Ok(CloudDriveAccount {
            // permissionId 就是 Drive 账户标识；不编造 email 或第二个账户 ID。
            provider_account_id: validate_drive_object_id(&bounded_text(
                user.permission_id,
                MAX_TEXT_BYTES,
            )?)
            .map_err(|_| malformed())?
            .to_owned(),
            display_name: bounded_text(user.display_name, MAX_TEXT_BYTES)?,
        })
    }
}

fn malformed() -> AppError {
    err("CLOUD_DRIVE_RESPONSE_INVALID")
}

fn drive_url(base: &str) -> Result<reqwest::Url, AppError> {
    reqwest::Url::parse(base).map_err(|_| internal("云盘端点无效"))
}

/// 只有 200 是 JSON 端点的成功形状；其余状态映射成不含上游数据的安全错误。
fn ensure_success(response: &GoogleResponse) -> Result<(), AppError> {
    match response.status {
        200 => Ok(()),
        401 => Err(err("CLOUD_DRIVE_UNAUTHORIZED")),
        403 => Err(err("CLOUD_DRIVE_FORBIDDEN")),
        404 => Err(err("CLOUD_DRIVE_NOT_FOUND")),
        _ => Err(err("CLOUD_DRIVE_UNAVAILABLE")),
    }
}

fn bounded_text(value: Option<String>, max_bytes: usize) -> Result<String, AppError> {
    let value = value.ok_or_else(malformed)?;
    if value.is_empty() || value.len() > max_bytes || value.chars().any(char::is_control) {
        return Err(malformed());
    }
    Ok(value)
}

fn folder_page(bytes: &[u8]) -> Result<CloudDriveFolderPage, AppError> {
    let raw: RawFileList = serde_json::from_slice(bytes).map_err(|_| malformed())?;
    let files = raw.files.unwrap_or_default();
    if files.len() > MAX_PAGE_ENTRIES {
        return Err(malformed());
    }
    let mut entries = Vec::with_capacity(files.len());
    for file in files {
        entries.push(file_metadata(file, None)?);
    }
    let next_page_token = match raw.next_page_token {
        Some(token) => Some(
            validate_drive_page_token(&token)
                .map_err(|_| malformed())?
                .to_owned(),
        ),
        None => None,
    };
    Ok(CloudDriveFolderPage {
        entries,
        next_page_token,
    })
}

fn file_metadata(raw: RawFile, expected: Option<&str>) -> Result<CloudDriveFileMetadata, AppError> {
    let id = validate_drive_object_id(&raw.id)
        .map_err(|_| malformed())?
        .to_owned();
    if expected.is_some_and(|expected| id != expected) {
        return Err(malformed());
    }
    let size_bytes = raw
        .size
        .map(|size| size.parse::<u64>())
        .transpose()
        .map_err(|_| malformed())?;
    // parents 只作为事实上报，本切片不会拿它拼 URL；只约束条数。
    let parents = raw.parents.unwrap_or_default();
    if parents.len() > MAX_PARENTS
        || parents
            .iter()
            .any(|id| validate_drive_object_id(id).is_err())
    {
        return Err(malformed());
    }
    let mime_type = bounded_text(raw.mime_type, MAX_TEXT_BYTES)?;
    Ok(CloudDriveFileMetadata {
        id,
        name: bounded_text(raw.name, MAX_TEXT_BYTES)?,
        is_folder: mime_type == FOLDER_MIME,
        mime_type,
        size_bytes,
        parents,
        trashed: raw.trashed.unwrap_or(false),
    })
}

/// 只接受 bytes start-end/total；bytes */total（不可满足）不是可用分片。
fn parse_content_range(raw: &str) -> Result<RemoteContentRange, AppError> {
    let rest = raw
        .trim()
        .strip_prefix("bytes")
        .ok_or_else(malformed)?
        .trim();
    let (span, total) = rest.split_once('/').ok_or_else(malformed)?;
    let (start, end) = span.split_once('-').ok_or_else(malformed)?;
    let start = start.trim().parse().map_err(|_| malformed())?;
    let end = end.trim().parse().map_err(|_| malformed())?;
    let total = total.trim().parse().map_err(|_| malformed())?;
    if start > end || end >= total {
        return Err(malformed());
    }
    Ok(RemoteContentRange { start, end, total })
}

fn mime_of(response: &GoogleResponse) -> &str {
    response
        .content_type
        .as_deref()
        .unwrap_or_default()
        .split(';')
        .next()
        .unwrap_or_default()
        .trim()
}

fn session(
    bytes: Vec<u8>,
    total_size: u64,
    content_range: Option<RemoteContentRange>,
    accept_ranges: bool,
) -> RemoteSessionBody {
    RemoteSessionBody {
        mime_type: PDF_MIME.to_owned(),
        bytes,
        total_size,
        content_range,
        accept_ranges,
    }
}

/// 只接受真正的 PDF 字节；Range 请求必须得到 206 与一致的分片元数据。
fn pdf_body(
    response: GoogleResponse,
    range: Option<RemoteByteRange>,
    size: Option<u64>,
) -> Result<RemoteSessionBody, AppError> {
    if !matches!(response.status, 200 | 206) {
        ensure_success(&response)?;
    }
    if !matches!(mime_of(&response), PDF_MIME | "application/octet-stream") {
        return Err(err("CLOUD_DRIVE_UNSUPPORTED_CONTENT"));
    }
    match (range, response.status) {
        (None, 200) => {
            if response.content_range.is_some() {
                return Err(err("SOURCE_RANGE_UNSUPPORTED"));
            }
            if !response.bytes.starts_with(PDF_MAGIC) {
                return Err(err("CLOUD_DRIVE_UNSUPPORTED_CONTENT"));
            }
            let total = response.bytes.len() as u64;
            if size.is_some_and(|size| size != total) {
                return Err(malformed());
            }
            Ok(session(response.bytes, total, None, response.accept_ranges))
        }
        (Some(requested), 206) => {
            let actual =
                parse_content_range(response.content_range.as_deref().ok_or_else(malformed)?)?;
            let length = actual
                .end
                .checked_sub(actual.start)
                .and_then(|length| length.checked_add(1))
                .ok_or_else(malformed)?;
            // 请求的 end 可以越过 EOF（服务端会夹紧），但不得错起点、被超出或换 total。
            if actual.start != requested.start
                || actual.total == 0
                || actual.total > MAX_PDF_TOTAL_BYTES
                || actual.end
                    != requested
                        .end
                        .unwrap_or(actual.total - 1)
                        .min(actual.total - 1)
                || size.is_some_and(|size| size != actual.total)
                || length != response.bytes.len() as u64
            {
                return Err(malformed());
            }
            if actual.start == 0
                && !response
                    .bytes
                    .starts_with(&PDF_MAGIC[..PDF_MAGIC.len().min(response.bytes.len())])
            {
                return Err(err("CLOUD_DRIVE_UNSUPPORTED_CONTENT"));
            }
            Ok(session(response.bytes, actual.total, Some(actual), true))
        }
        // 401/403/404 仍是结构化错误；用 200 回应 Range（或反之）不可接受。
        _ => match response.status {
            401 => Err(err("CLOUD_DRIVE_UNAUTHORIZED")),
            403 => Err(err("CLOUD_DRIVE_FORBIDDEN")),
            404 => Err(err("CLOUD_DRIVE_NOT_FOUND")),
            200 | 206 => Err(err("SOURCE_RANGE_UNSUPPORTED")),
            _ => Err(err("CLOUD_DRIVE_UNAVAILABLE")),
        },
    }
}

#[derive(serde::Deserialize)]
struct RawFileList {
    #[serde(rename = "nextPageToken")]
    next_page_token: Option<String>,
    files: Option<Vec<RawFile>>,
}

#[derive(serde::Deserialize)]
struct RawFile {
    id: String,
    name: Option<String>,
    #[serde(rename = "mimeType")]
    mime_type: Option<String>,
    size: Option<String>,
    parents: Option<Vec<String>>,
    trashed: Option<bool>,
}

#[derive(serde::Deserialize)]
struct RawAbout {
    user: Option<RawUser>,
}

#[derive(serde::Deserialize)]
struct RawUser {
    #[serde(rename = "permissionId")]
    permission_id: Option<String>,
    #[serde(rename = "displayName")]
    display_name: Option<String>,
}

#[cfg(test)]
#[path = "drive_tests.rs"]
mod tests;
