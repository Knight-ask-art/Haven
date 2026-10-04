//! Google Drive / OAuth 2.0 固定端点 HTTP 传输（基础设施切片）。
//!
//! 只做一件事：把冻结的 [`GoogleRequest`] 送到 Google 的两个固定端点之一，并把响应有界
//! 取回；不解析 Drive JSON、不管理 token、不落库、不生成 IPC DTO。
//!
//! 端点策略在**任何 DNS 解析与 I/O 之前**执行，且刻意比通用策略更严：先叠加
//! `HttpUrlPolicy::SourceEndpoint`（字面内网 / 回环地址、userinfo、fragment、单标签主机与
//! 非白名单端口已在那一层被拒），再要求 `https` + 443 + 主机**恰好**是
//! `www.googleapis.com` 或 `oauth2.googleapis.com`。Drive 只允许三个精确路径：
//! `GET /drive/v3/files`、`GET /drive/v3/files/{id}`（`id` 复用 application 的
//! `validate_drive_object_id`）与 `GET /drive/v3/about`；**没有** `/drive/v3/…` 前缀通配。
//! 查询参数名取固定白名单、取值有界、重复参数直接拒绝。token 端点只允许无查询串的
//! `POST /token`，字段名取固定白名单（含 `redirect_uri`），且不接受 Bearer。
//!
//! `Range` 只允许出现在 `GET /drive/v3/files/{id}?alt=media` 上：`start` / `end` 必须自洽，
//! 请求窗口被夹到 [`MEDIA_LIMIT`] 以内（开口范围也会被封闭成有界窗口），并固定
//! `Accept-Encoding: identity`，免得压缩改变字节偏移；其它请求带 `range` 一律拒绝。
//!
//! 传输：不用代理、**完全不跟随重定向**（3xx 直接失败）、`http1_only`，连接 3 秒、整体
//! 20 秒（同一个上界覆盖 DNS / 连接 / 发送 / 正文读取），目的地按
//! `resolve_public_http_target` 解析并固定成公网地址。正文按块累积、逐块校验上限；本 crate
//! 的 reqwest 打开了 gzip/brotli，因此上限落在**解压后**字节上。
//!
//! 状态语义：2xx 必须是预期内容类型（HTML 一律拒绝）；4xx（401/403/404 与 OAuth
//! `400 invalid_grant`）原样保留给适配器映射，JSON 错误正文按 [`JSON_LIMIT`] 有界取回，
//! 非 JSON 错误正文不读、留空；3xx 与 5xx 是传输失败。错误与 `Debug` 里不出现 URL、查询串、
//! Bearer、表单值或正文；[`GoogleResponse`] 刻意不实现 `Debug`。
//!

use std::time::Duration;

use async_trait::async_trait;
use haven_application::services::cloud_storage::ports::validate_drive_object_id;
use haven_application::services::ports::RemoteByteRange;
use haven_common::network::{HttpUrlPolicy, parse_http_url};
use haven_common::{AppError, ErrorKind};
use haven_domain::credential::SecretString;
use reqwest::Url;
use reqwest::header::{ACCEPT, ACCEPT_ENCODING, ACCEPT_RANGES, CONTENT_RANGE, CONTENT_TYPE, RANGE};
use reqwest::redirect::Policy;

use crate::http_security::{pin_client_builder, resolve_public_http_target};

/// JSON 类响应的正文上限（解压后字节）。
pub(crate) const JSON_LIMIT: usize = 4 * 1024 * 1024;
/// 媒体下载的正文上限（解压后字节）。
pub(crate) const MEDIA_LIMIT: usize = 32 * 1024 * 1024;

const REQUEST_TIMEOUT: Duration = Duration::from_secs(20);
const CONNECT_TIMEOUT: Duration = Duration::from_secs(3);
const USER_AGENT: &str = concat!("Haven/", env!("CARGO_PKG_VERSION"), " (google drive)");
/// 两个固定端点都只接受默认 HTTPS 端口。
const TLS_PORT: u16 = 443;
const API_HOST: &str = "www.googleapis.com";
const OAUTH_HOST: &str = "oauth2.googleapis.com";
const DRIVE_FILES: &str = "/drive/v3/files";
const DRIVE_ABOUT: &str = "/drive/v3/about";
const TOKEN_PATH: &str = "/token";
/// Bearer 与表单值共用的长度上限；OAuth 凭据远小于此。
const MAX_SECRET_BYTES: usize = 8 * 1024;
/// 响应头取值上限；超过即视为畸形响应。
const MAX_HEADER_BYTES: usize = 256;
const JSON_ACCEPT: &str = "application/json";
const MEDIA_ACCEPT: &str = "application/pdf, application/octet-stream";
/// token 端点表单字段名的固定白名单；授权码流程必须能带 `redirect_uri`。
const TOKEN_FORM_FIELDS: &[&str] = &[
    "client_id",
    "client_secret",
    "code",
    "code_verifier",
    "grant_type",
    "redirect_uri",
    "refresh_token",
];

/// 固定端点允许的两种方法；其它方法在这条路径上没有合法用途。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum GoogleMethod {
    Get,
    PostForm,
}

/// 一次固定端点请求。刻意不实现 `Clone`；`Debug` 不打印 URL、查询串、Bearer 与表单值。
pub(crate) struct GoogleRequest {
    pub(crate) url: Url,
    pub(crate) method: GoogleMethod,
    pub(crate) bearer: Option<SecretString>,
    pub(crate) form: Vec<(String, SecretString)>,
    /// 仅 `files/{id}?alt=media` 允许的字节区间；其它请求必须为 `None`。
    pub(crate) range: Option<RemoteByteRange>,
    /// 调用方声明的正文上限；必须 ≥ 1，且只会被夹到端点天花板以内。
    pub(crate) max_bytes: usize,
}

impl std::fmt::Debug for GoogleRequest {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("GoogleRequest")
            .field("method", &self.method)
            .field("host", &self.url.host_str())
            .field("has_bearer", &self.bearer.is_some())
            .field("form_fields", &self.form.len())
            .field("has_range", &self.range.is_some())
            .field("max_bytes", &self.max_bytes)
            .finish()
    }
}

/// 一次固定端点响应。`bytes` 是远端数据，故本类型没有 `Debug`。
pub(crate) struct GoogleResponse {
    pub(crate) status: u16,
    pub(crate) bytes: Vec<u8>,
    pub(crate) content_type: Option<String>,
    pub(crate) content_range: Option<String>,
    pub(crate) accept_ranges: bool,
}

/// 单次请求的传输端口；OAuth / Drive 切片注入 fake 即可离线覆盖整条策略与收集逻辑。
#[async_trait]
pub(crate) trait GoogleTransport: Send + Sync {
    async fn send(&self, request: GoogleRequest) -> Result<GoogleResponse, AppError>;
}

/// 生产实现：每次调用重新解析并固定地址，不用代理，不跟随重定向。
pub(crate) struct ReqwestGoogleTransport;

impl ReqwestGoogleTransport {
    pub(crate) fn new() -> Self {
        Self
    }
}

#[async_trait]
impl GoogleTransport for ReqwestGoogleTransport {
    async fn send(&self, request: GoogleRequest) -> Result<GoogleResponse, AppError> {
        let validated = validate_request(&request)?;
        // 总超时包住 DNS、连接、发送与正文读取；reqwest 自身的 timeout 也覆盖整条请求，
        // 两层指向同一个上界，任一层先到都会以 SOURCE_TIMEOUT 失败。
        with_deadline(exchange(request, validated), REQUEST_TIMEOUT).await
    }
}

/// 校验通过后仍需携带的事实：`media` 决定内容类型与大小上限，`range` 是成形后的 `Range` 头。
struct ValidatedRequest {
    media: bool,
    range: Option<String>,
}

/// 校验一个请求的全部策略；所有检查都在 DNS 与 I/O 之前完成，错误不回显 URL 或凭据。
fn validate_request(request: &GoogleRequest) -> Result<ValidatedRequest, AppError> {
    // 叠加而非替代通用出站策略：字面地址、控制字符与可疑主机先在这一层被统一拒绝。
    parse_http_url(request.url.as_str(), HttpUrlPolicy::SourceEndpoint)
        .map_err(|_| denied("Google 端点未通过通用出站 URL 策略"))?;
    let url = &request.url;
    // 通用策略仍允许 http 与 80/8080/8443；固定端点只接受默认 HTTPS 端口。
    if url.scheme() != "https" || url.port_or_known_default() != Some(TLS_PORT) {
        return Err(denied("Google 端点只允许 443 端口的 HTTPS"));
    }
    if request.max_bytes == 0 {
        return Err(invalid("Google 请求的正文上限必须大于 0"));
    }
    match url.host_str() {
        Some(OAUTH_HOST) => validate_token(request),
        Some(API_HOST) => validate_drive(request),
        _ => Err(denied("Google 端点主机不在固定白名单内")),
    }
}

/// Drive：只允许不带表单的 GET，路径与查询参数都取固定白名单。
fn validate_drive(request: &GoogleRequest) -> Result<ValidatedRequest, AppError> {
    const DRIVE_QUERY_PARAMS: &[&str] = &[
        "alt",
        "fields",
        "orderBy",
        "pageSize",
        "pageToken",
        "q",
        "spaces",
    ];
    if request.method != GoogleMethod::Get || !request.form.is_empty() {
        return Err(denied("Drive 端点只允许不带表单的 GET"));
    }
    let has_file_id = classify_drive_path(&request.url)?;
    let mut alt: Option<String> = None;
    match request.url.query() {
        None => {}
        Some("") => return Err(denied("Drive 端点查询串为空")),
        Some(_) => {
            // query_pairs 先做百分号解码，因此编码变体绕不过名字白名单。
            let mut seen: Vec<String> = Vec::new();
            for (name, value) in request.url.query_pairs() {
                if !DRIVE_QUERY_PARAMS.contains(&name.as_ref())
                    || seen.iter().any(|prev| prev == name.as_ref())
                {
                    return Err(denied("Drive 端点查询参数不在固定白名单内"));
                }
                validate_query_value(&name, &value)?;
                if name == "alt" {
                    alt = Some(value.into_owned());
                }
                seen.push(name.into_owned());
            }
        }
    }
    let media = has_file_id && alt.as_deref() == Some("media");
    if alt.as_deref() == Some("media") && !has_file_id {
        return Err(denied("只有 files/{id} 才允许 alt=media"));
    }
    let range = match request.range.as_ref() {
        None => None,
        Some(range) if media => Some(range_header(range)?),
        Some(_) => return Err(denied("Range 只允许出现在 files/{id}?alt=media 上")),
    };
    require_secret(request.bearer.as_ref(), "Drive 请求缺少合法凭据")?;
    Ok(ValidatedRequest { media, range })
}

/// 路径必须恰好是 `files` / `files/{id}` / `about`；返回是否带（已校验的）文件 ID。
fn classify_drive_path(url: &Url) -> Result<bool, AppError> {
    let path = url.path();
    if path == DRIVE_FILES || path == DRIVE_ABOUT {
        return Ok(false);
    }
    let id = path
        .strip_prefix(DRIVE_FILES)
        .and_then(|rest| rest.strip_prefix('/'))
        .ok_or_else(|| denied("Drive 端点路径不在固定白名单内"))?;
    validate_drive_object_id(id).map_err(|_| denied("Drive 对象 ID 不符合固定白名单"))?;
    Ok(true)
}

/// 查询取值必须有界、无控制字符；只有 `alt` 与 `pageSize` 有固定取值集合。
fn validate_query_value(name: &str, value: &str) -> Result<(), AppError> {
    const MAX_VALUE_BYTES: usize = 2048;
    const MAX_PAGE_SIZE: u32 = 1000;
    if value.is_empty() || value.len() > MAX_VALUE_BYTES || value.chars().any(char::is_control) {
        return Err(denied("Drive 端点查询取值非法"));
    }
    match name {
        "alt" if !matches!(value, "json" | "media") => Err(denied("Drive 端点 alt 取值非法")),
        "pageSize" => match value.parse::<u32>() {
            Ok(size) if (1..=MAX_PAGE_SIZE).contains(&size) => Ok(()),
            _ => Err(denied("Drive 端点 pageSize 取值非法")),
        },
        _ => Ok(()),
    }
}

/// OAuth token：只允许无查询串的 POST /token，字段名固定、不接受 Bearer 与字节区间。
fn validate_token(request: &GoogleRequest) -> Result<ValidatedRequest, AppError> {
    let url = &request.url;
    if request.method != GoogleMethod::PostForm || url.path() != TOKEN_PATH || url.query().is_some()
    {
        return Err(denied("OAuth token 端点只允许无查询串的 POST /token"));
    }
    if request.bearer.is_some() {
        return Err(denied("OAuth token 端点不接受 Bearer 凭据"));
    }
    if request.range.is_some() {
        return Err(denied("OAuth token 端点不接受字节区间"));
    }
    if request.form.is_empty() {
        return Err(denied("OAuth token 端点缺少表单字段"));
    }
    let mut seen: Vec<&str> = Vec::new();
    for (name, value) in &request.form {
        if !TOKEN_FORM_FIELDS.contains(&name.as_str()) || seen.contains(&name.as_str()) {
            return Err(denied("OAuth token 表单字段不在固定白名单内"));
        }
        seen.push(name);
        require_secret(Some(value), "OAuth token 表单值非法")?;
    }
    Ok(ValidatedRequest {
        media: false,
        range: None,
    })
}

/// 秘密值在进入 header / 表单之前必须非空、有界，且只含可见 ASCII（CR/LF 与空白在此被拒）。
fn require_secret(secret: Option<&SecretString>, message: &'static str) -> Result<(), AppError> {
    let value = secret.map(SecretString::expose).unwrap_or_default();
    if value.is_empty()
        || value.len() > MAX_SECRET_BYTES
        || !value.bytes().all(|byte| byte.is_ascii_graphic())
    {
        return Err(denied(message));
    }
    Ok(())
}

/// `Range` 头只由整数拼装；窗口一律封闭且有界，开口范围封顶到 [`MEDIA_LIMIT`]。
fn range_header(range: &RemoteByteRange) -> Result<String, AppError> {
    let start = range.start;
    let end = match range.end {
        Some(end) if end < start => return Err(denied("字节区间起止非法")),
        Some(end) => end,
        None => start
            .checked_add(MEDIA_LIMIT as u64 - 1)
            .ok_or_else(|| denied("字节区间起点超出上限"))?,
    };
    if end
        .checked_sub(start)
        .and_then(|n| n.checked_add(1))
        .is_none_or(|n| n > MEDIA_LIMIT as u64)
    {
        return Err(denied("字节区间超出媒体上限"));
    }
    Ok(format!("bytes={start}-{end}"))
}

/// 一次完整交换：解析并固定地址 → 发请求 → 有界读取。
async fn exchange(
    request: GoogleRequest,
    validated: ValidatedRequest,
) -> Result<GoogleResponse, AppError> {
    let target = resolve_public_http_target(request.url.as_str(), HttpUrlPolicy::SourceEndpoint)
        .await
        .map_err(|_| denied("Google 端点未通过公网地址策略"))?;
    let builder = reqwest::Client::builder()
        .connect_timeout(CONNECT_TIMEOUT)
        .timeout(REQUEST_TIMEOUT)
        .http1_only()
        .redirect(Policy::none())
        .user_agent(USER_AGENT);
    // pin_client_builder 同时设置 no_proxy()，并把解析结果固定为唯一目的地。
    let client = pin_client_builder(builder, &target)
        .build()
        .map_err(|_| failed("Google 客户端初始化失败"))?;
    let mut pending = client.request(method_of(request.method), target.url.clone());
    if let Some(bearer) = request.bearer.as_ref() {
        pending = pending.bearer_auth(bearer.expose());
    }
    if request.method == GoogleMethod::PostForm {
        let fields: Vec<(&str, &str)> = request
            .form
            .iter()
            .map(|(name, value)| (name.as_str(), value.expose()))
            .collect();
        // 表单由 reqwest 做 URL 编码并自行设置 Content-Type；取值只活在这一个请求里。
        pending = pending.form(&fields);
    }
    if let Some(range) = validated.range.as_deref() {
        // 固定身份编码：压缩会改变字节偏移，让 Range 语义失真。
        pending = pending
            .header(RANGE, range)
            .header(ACCEPT_ENCODING, "identity");
    }
    let response = pending
        .header(
            ACCEPT,
            if validated.media {
                MEDIA_ACCEPT
            } else {
                JSON_ACCEPT
            },
        )
        .send()
        .await
        .map_err(|_| failed("Google 端点暂时不可达"))?;
    let head = ResponseHead {
        status: response.status().as_u16(),
        content_type: bounded_header(response.headers(), CONTENT_TYPE)?,
        content_range: bounded_header(response.headers(), CONTENT_RANGE)?,
        content_length: response.content_length(),
        accept_ranges: response
            .headers()
            .get(ACCEPT_RANGES)
            .and_then(|value| value.to_str().ok())
            .is_some_and(|value| value.trim().eq_ignore_ascii_case("bytes")),
    };
    let mut chunks = ReqwestChunks { response };
    collect_bounded(request.max_bytes, validated.media, head, &mut chunks).await
}

fn method_of(method: GoogleMethod) -> reqwest::Method {
    match method {
        GoogleMethod::Get => reqwest::Method::GET,
        GoogleMethod::PostForm => reqwest::Method::POST,
    }
}

/// 一次响应的头部快照（已完成唯一性、可解析与有界校验）。
struct ResponseHead {
    status: u16,
    content_type: Option<String>,
    content_range: Option<String>,
    content_length: Option<u64>,
    accept_ranges: bool,
}

/// 逐块读取的正文端口；让上限在流式过程中执行，并能用 fake 离线覆盖。
#[async_trait]
trait ChunkStream: Send {
    async fn next_chunk(&mut self) -> Result<Option<Vec<u8>>, AppError>;
}

/// `reqwest::Response` 的逐块适配。
struct ReqwestChunks {
    response: reqwest::Response,
}

#[async_trait]
impl ChunkStream for ReqwestChunks {
    async fn next_chunk(&mut self) -> Result<Option<Vec<u8>>, AppError> {
        self.response
            .chunk()
            .await
            .map(|chunk| chunk.map(|bytes| bytes.to_vec()))
            .map_err(|_| failed("Google 响应读取中断"))
    }
}

/// 状态、内容类型、上限与流式读取的唯一实现；纯逻辑，可用 fake 离线覆盖。
async fn collect_bounded(
    max_bytes: usize,
    media: bool,
    head: ResponseHead,
    stream: &mut dyn ChunkStream,
) -> Result<GoogleResponse, AppError> {
    if (300..400).contains(&head.status) {
        return Err(failed("Google 端点返回了重定向"));
    }
    if !(200..500).contains(&head.status) {
        return Err(failed("Google 端点返回异常状态"));
    }
    let ok = (200..300).contains(&head.status);
    let essence = essence_of(head.content_type.as_deref());
    let limit = if ok {
        let accepted = if media {
            matches!(head.status, 200 | 206)
                && matches!(essence, "application/pdf" | "application/octet-stream")
        } else {
            head.status == 200 && essence == "application/json"
        };
        if !accepted {
            return Err(failed("Google 端点返回了非预期内容类型"));
        }
        body_limit(max_bytes, media)
    } else if essence == "application/json" {
        // 4xx 原样保留给适配器；错误正文只在它是 JSON 时按 JSON 上限取回。
        JSON_LIMIT
    } else {
        // 非 JSON 的错误正文不读、留空，也绝不回显。
        0
    };
    let bytes = if limit == 0 {
        Vec::new()
    } else if head
        .content_length
        .is_some_and(|length| length > limit as u64)
    {
        if ok {
            return Err(failed("Google 响应超出大小上限"));
        }
        Vec::new()
    } else {
        match read_bounded(limit, stream).await? {
            Some(bytes) => bytes,
            None if ok => return Err(failed("Google 响应超出大小上限")),
            // 错误状态的正文超限：状态照旧保留，正文留空。
            None => Vec::new(),
        }
    };
    Ok(GoogleResponse {
        status: head.status,
        bytes,
        content_type: head.content_type,
        content_range: head.content_range,
        accept_ranges: head.accept_ranges,
    })
}

/// 逐块读取正文；超过 `limit` 返回 `None`，由调用方决定是失败还是「正文留空」。
async fn read_bounded(
    limit: usize,
    stream: &mut dyn ChunkStream,
) -> Result<Option<Vec<u8>>, AppError> {
    let mut bytes = zeroize::Zeroizing::new(Vec::with_capacity(64 * 1024));
    while let Some(chunk) = stream.next_chunk().await? {
        if bytes.len().saturating_add(chunk.len()) > limit {
            return Ok(None);
        }
        bytes.extend_from_slice(&chunk);
    }
    Ok(Some(std::mem::take(&mut *bytes)))
}

/// `Content-Type` 的 essence：去掉参数并去空白。
fn essence_of(value: Option<&str>) -> &str {
    value
        .unwrap_or_default()
        .split(';')
        .next()
        .unwrap_or_default()
        .trim()
}

/// 头部取值必须唯一、可解析且有界；重复的 Content-Type / Content-Range 直接拒绝。
fn bounded_header(
    headers: &reqwest::header::HeaderMap,
    name: reqwest::header::HeaderName,
) -> Result<Option<String>, AppError> {
    let mut values = headers.get_all(name).iter();
    let Some(value) = values.next() else {
        return Ok(None);
    };
    if values.next().is_some() {
        return Err(failed("Google 响应头部重复"));
    }
    let text = value
        .to_str()
        .map_err(|_| failed("Google 响应头部不可解析"))?;
    if text.len() > MAX_HEADER_BYTES {
        return Err(failed("Google 响应头部超长"));
    }
    Ok(Some(text.to_owned()))
}

/// 调用方声明的上限只会被夹到该类别天花板以内，永远不会被放大。
fn body_limit(requested: usize, media: bool) -> usize {
    requested.min(if media { MEDIA_LIMIT } else { JSON_LIMIT })
}

/// 总超时的唯一实现；拆出来是为了让「传输整体有界」能在没有网络的情况下被覆盖。
async fn with_deadline<F, T>(future: F, limit: Duration) -> Result<T, AppError>
where
    F: std::future::Future<Output = Result<T, AppError>>,
{
    tokio::time::timeout(limit, future)
        .await
        .map_err(|_| timeout_error())?
}

fn denied(message: &'static str) -> AppError {
    AppError::new(
        "SECURITY_POLICY_DENIED",
        ErrorKind::Security,
        message,
        false,
    )
}

fn invalid(message: &'static str) -> AppError {
    AppError::new("VALIDATION", ErrorKind::Validation, message, false)
}

fn failed(message: &'static str) -> AppError {
    AppError::new("SOURCE_UNAVAILABLE", ErrorKind::Network, message, true)
}

fn timeout_error() -> AppError {
    AppError::new(
        "SOURCE_TIMEOUT",
        ErrorKind::Timeout,
        "Google 端点请求超时",
        true,
    )
}

#[cfg(test)]
#[path = "http_tests.rs"]
mod tests;
