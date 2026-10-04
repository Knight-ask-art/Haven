//! 用户登记的自托管漫画库 Provider（Komga / Kavita）。
//!
//! Intent lock（不扩边）：
//! - 只请求用户在来源设置中登记的漫画库地址；所有请求都基于**已登记的 endpoint**
//!   与固定 API 路径 + 服务端返回、形状受校验的 opaque 标识。响应体里的 `url`、
//!   文件路径或页面地址**从不**被当成下一次请求的目标。
//! - API key 只经系统凭据库解析，并且**只作为请求头**注入（Komga `X-API-Key`、
//!   Kavita `x-api-key`）。它绝不进入 URL、query、path、wire、日志或错误文案。
//! - 出站请求沿用仓库现有的 HTTPS/Host/DNS/端口/重定向/响应大小策略，不新增
//!   内网、回环或单标签主机例外（自托管 NAS 属于后续单独的 ADR）。
//!
//! 网络访问收在 [`ComicLibraryGateway`] 端口后面：生产实现是
//! [`HttpComicLibraryGateway`]，测试注入进程内 fake，因此端点映射、分页、
//! 字段解析、认证头与上限语义无需真实服务器即可验证。

use std::pin::Pin;
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use serde_json::Value;

use haven_application::services::comic::{
    ComicImageMime, ComicPageBody, PreparedComicPage, PreparedComicPageAvailability,
    PreparedComicPageSource, RemoteComicPageProvider,
};
use haven_application::services::ports::{RemoteAcquiredFile, RemoteAcquisitionPort};
use haven_application::services::search_source::SearchSourceParticipant;
use haven_application::services::session::{PreparedSession, PreparedSessionSource};
use haven_application::services::source_import::{
    KAVITA_SOURCE_KEY, KOMGA_SOURCE_KEY, RemoteContentRef, SourceCatalogEntry,
    SourceCatalogProvider, comic_library_candidate_handle, comic_library_source_key,
    split_comic_library_chapter_remote_id, split_comic_library_work_ref,
};
use haven_application::services::source_registry::{CustomSourceKind, SourceRegistryService};
use haven_application::wire::{ContentCategory, MediaTypeDto, QueryCategory, WorkCardDto};
use haven_common::network::HttpUrlPolicy;
use haven_common::{AppError, ErrorKind, UtcMillis};
use haven_domain::comic_catalog::{
    ComicChapterAvailability, ComicChapterCatalog, ComicChapterCatalogEntry,
};
use haven_domain::comic_identity::{
    ChapterSourceIdentity, ComicChapterMetadata, EditionProfile, IdentityFacet, ScanGroupFacet,
};
use haven_domain::enums::MediaType;

use crate::comic::page_identity_for_provider;
use crate::http_security::{pin_client_builder, resolve_public_http_target};
use crate::online_sources::{image_mime, write_bytes_to, write_cbz_to};

/// JSON 响应上限（搜索、目录、章节信息）。
const MAX_JSON_BYTES: usize = 4 * 1024 * 1024;
/// 单页图片上限。
const MAX_PAGE_BYTES: usize = 32 * 1024 * 1024;
/// 单章分页读取总字节上限。
const MAX_CHAPTER_TOTAL_BYTES: usize = 512 * 1024 * 1024;
/// 单章归档上限（服务端提供的 CBZ 流）。
const MAX_ARCHIVE_BYTES: usize = 1024 * 1024 * 1024;
/// 单章页数上限。
const MAX_CHAPTER_PAGES: usize = 2_000;
/// 单次目录读取的章节数上限；对 Komga 的分页循环同时是「服务端原始条目」上限，
/// 因此即使整页条目都无法解析，循环也一定有界。
const MAX_CHAPTERS: usize = 2_000;
/// 目录分页大小（同时是分页循环的步长）。
const CATALOG_PAGE_SIZE: usize = 100;
/// 搜索分页大小上限。
const MAX_SEARCH_PAGE_SIZE: u32 = 50;
const MAX_REDIRECTS: usize = 3;
const REQUEST_TIMEOUT: Duration = Duration::from_secs(20);
const CONNECT_TIMEOUT: Duration = Duration::from_secs(3);
const USER_AGENT: &str = "Haven/0.1.0 (offline content importer)";

/// Komga API key 请求头（官方 API key auth scheme）。
const KOMGA_API_KEY_HEADER: &str = "X-API-Key";
/// Kavita API key 请求头（官方 API key auth scheme）。
const KAVITA_API_KEY_HEADER: &str = "x-api-key";

/// 从系统凭据库解析某漫画库的 API key（内存即取即用，禁止落盘/日志）。
pub type ApiKeyResolver =
    dyn Fn(&str) -> Pin<Box<dyn std::future::Future<Output = Option<String>> + Send>> + Send + Sync;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LibraryMethod {
    Get,
    Post,
}

/// 一次有界的漫画库请求描述。
///
/// `endpoint` 是用户登记且已通过 `HttpUrlPolicy::SourceEndpoint` 校验的基地址；
/// `path` 只由本模块按固定 API 路径构造（可含本模块编码的查询串）。
#[derive(Clone)]
pub struct LibraryRequest {
    pub endpoint: String,
    pub method: LibraryMethod,
    pub path: String,
    /// API key 只在此处出现，并由实现写入请求头；绝不拼接进 URL。
    pub api_key: Option<String>,
    pub api_key_header: &'static str,
    pub json_body: Option<Value>,
    pub accept: &'static str,
}

#[derive(Debug, Clone)]
pub struct LibraryResponse {
    pub bytes: Vec<u8>,
}

/// 漫画库网络端口。实现方负责 URL 策略、DNS 解析与固定、逐跳重定向校验、
/// 超时、请求头上限与响应大小上限。
#[async_trait]
pub trait ComicLibraryGateway: Send + Sync {
    async fn send(
        &self,
        request: LibraryRequest,
        max_bytes: usize,
    ) -> Result<LibraryResponse, AppError>;
}

/// 生产实现：按登记的 endpoint 逐跳校验的受控 HTTP 客户端。
#[derive(Clone)]
pub struct HttpComicLibraryGateway;

impl HttpComicLibraryGateway {
    pub fn new() -> Result<Self, AppError> {
        Ok(Self)
    }
}

#[async_trait]
impl ComicLibraryGateway for HttpComicLibraryGateway {
    async fn send(
        &self,
        request: LibraryRequest,
        max_bytes: usize,
    ) -> Result<LibraryResponse, AppError> {
        // endpoint 已在登记时校验；这里仍然重新走一次策略，避免持久化数据被
        // 手工改写后绕过策略。
        haven_common::network::parse_http_url(&request.endpoint, HttpUrlPolicy::SourceEndpoint)
            .map_err(|_| security_denied("漫画库地址不安全"))?;
        let mut current = reqwest::Url::parse(&format!(
            "{}{}",
            request.endpoint.trim_end_matches('/'),
            request.path
        ))
        .map_err(|_| security_denied("漫画库请求地址无效"))?;
        let origin_host = current
            .host_str()
            .ok_or_else(|| security_denied("漫画库地址缺少主机"))?
            .to_ascii_lowercase();
        let origin_port = current.port_or_known_default();

        for _ in 0..=MAX_REDIRECTS {
            if current.host_str().map(str::to_ascii_lowercase).as_deref()
                != Some(origin_host.as_str())
                || current.port_or_known_default() != origin_port
                || current.scheme() != "https"
            {
                return Err(security_denied("漫画库重定向目标与登记地址不一致"));
            }
            let target =
                resolve_public_http_target(current.as_str(), HttpUrlPolicy::SourceEndpoint)
                    .await
                    .map_err(|_| security_denied("漫画库地址解析不安全"))?;
            let builder = reqwest::Client::builder()
                .connect_timeout(CONNECT_TIMEOUT)
                .timeout(REQUEST_TIMEOUT)
                .user_agent(USER_AGENT)
                .http1_only()
                .redirect(reqwest::redirect::Policy::none());
            let client = pin_client_builder(builder, &target)
                .build()
                .map_err(|_| internal_error("漫画库客户端初始化失败"))?;
            let mut call = match request.method {
                LibraryMethod::Get => client.get(target.url.clone()),
                LibraryMethod::Post => client.post(target.url.clone()),
            }
            .header(reqwest::header::ACCEPT, request.accept);
            if let Some(body) = &request.json_body {
                call = call.json(body);
            }
            if let Some(secret) = &request.api_key {
                // secret 只在此处进入请求头；reqwest 内部按 header 编码，不落日志。
                call = call.header(request.api_key_header, secret.as_str());
            }
            let response = call
                .send()
                .await
                .map_err(|_| source_unavailable("漫画库暂时不可达"))?;
            if response.status().is_redirection() {
                let location = response
                    .headers()
                    .get(reqwest::header::LOCATION)
                    .and_then(|value| value.to_str().ok())
                    .ok_or_else(|| source_unavailable("漫画库重定向地址无效"))?;
                current = current
                    .join(location)
                    .map_err(|_| security_denied("漫画库重定向地址不安全"))?;
                continue;
            }
            if !response.status().is_success() {
                // 401/403 明确表示为认证失败，但不回显响应体或凭据。
                if matches!(
                    response.status(),
                    reqwest::StatusCode::UNAUTHORIZED | reqwest::StatusCode::FORBIDDEN
                ) {
                    return Err(AppError::new(
                        "SOURCE_AUTH_FAILED",
                        ErrorKind::Security,
                        "漫画库拒绝了这次请求，请检查 API key 是否有效",
                        false,
                    ));
                }
                return Err(source_unavailable("漫画库返回异常状态"));
            }
            if response
                .content_length()
                .is_some_and(|length| length > max_bytes as u64)
            {
                return Err(source_unavailable("漫画库响应超出大小上限"));
            }
            let mut body = Vec::with_capacity(64 * 1024);
            let mut response = response;
            while let Some(chunk) = response
                .chunk()
                .await
                .map_err(|_| source_unavailable("漫画库响应读取中断"))?
            {
                if body.len().saturating_add(chunk.len()) > max_bytes {
                    return Err(source_unavailable("漫画库响应超出大小上限"));
                }
                body.extend_from_slice(&chunk);
            }
            return Ok(LibraryResponse { bytes: body });
        }
        Err(source_unavailable("漫画库重定向次数过多"))
    }
}

/// 一次搜索结果。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SeriesHit {
    pub series_id: String,
    pub title: String,
    pub summary: Option<String>,
    pub year: Option<i32>,
}

/// 目录中的一个章节。
#[derive(Debug, Clone, PartialEq)]
pub struct ChapterHit {
    pub chapter_id: String,
    pub number: Option<f64>,
    pub volume: Option<f64>,
    pub title: Option<String>,
    pub page_count: Option<u32>,
    pub available: bool,
}

/// 自托管漫画库 Provider：目录详情、章节列表、在线逐页读取与章节归档下载。
pub struct ComicLibraryProvider {
    registry: SourceRegistryService,
    gateway: Arc<dyn ComicLibraryGateway>,
    api_key: Arc<ApiKeyResolver>,
}

impl ComicLibraryProvider {
    pub fn new(
        registry: SourceRegistryService,
        gateway: Arc<dyn ComicLibraryGateway>,
        api_key: Arc<ApiKeyResolver>,
    ) -> Self {
        Self {
            registry,
            gateway,
            api_key,
        }
    }

    fn kind_of(source_id: &str) -> Result<CustomSourceKind, AppError> {
        SourceRegistryService::comic_library_kind(source_id)
            .ok_or_else(|| invalid_argument("未知漫画库来源"))
    }

    async fn endpoint(&self, source_id: &str) -> Result<String, AppError> {
        self.registry
            .endpoint(source_id)
            .await?
            // 库被删除或地址被清空时，已落库的远端身份仍然存在：这是「来源当前
            // 不可用」，不是调用方参数错误。
            .ok_or_else(|| source_unavailable("该漫画库当前不可用"))
    }

    async fn api_key(&self, source_id: &str) -> Option<String> {
        (self.api_key)(source_id).await
    }

    fn header_for(kind: CustomSourceKind) -> Result<&'static str, AppError> {
        match kind {
            CustomSourceKind::Komga => Ok(KOMGA_API_KEY_HEADER),
            CustomSourceKind::Kavita => Ok(KAVITA_API_KEY_HEADER),
            _ => Err(invalid_argument("未知漫画库来源")),
        }
    }

    /// 构造一次请求：API key 只在 `api_key` 字段里，绝不进入 `path`。
    async fn request(
        &self,
        source_id: &str,
        kind: CustomSourceKind,
        method: LibraryMethod,
        path: String,
        json_body: Option<Value>,
        accept: &'static str,
    ) -> Result<LibraryRequest, AppError> {
        Ok(LibraryRequest {
            endpoint: self.endpoint(source_id).await?,
            method,
            path,
            api_key: self.api_key(source_id).await,
            api_key_header: Self::header_for(kind)?,
            json_body,
            accept,
        })
    }

    async fn json(&self, request: LibraryRequest, max_bytes: usize) -> Result<Value, AppError> {
        let response = self.gateway.send(request, max_bytes).await?;
        serde_json::from_slice(&response.bytes)
            .map_err(|_| source_unavailable("漫画库返回了无效 JSON"))
    }

    /// 搜索作品。Komga 用 `POST /api/v1/series/list`（`SeriesSearch.fullTextSearch`），
    /// Kavita 用 `GET /api/Search/search`（只投影 `SearchResultGroupDto.series`）。
    pub async fn search_series(
        &self,
        source_id: &str,
        query: &str,
        limit: u32,
    ) -> Result<Vec<SeriesHit>, AppError> {
        let kind = Self::kind_of(source_id)?;
        let size = limit.clamp(1, MAX_SEARCH_PAGE_SIZE);
        match kind {
            CustomSourceKind::Komga => {
                let request = self
                    .request(
                        source_id,
                        kind,
                        LibraryMethod::Post,
                        format!("/api/v1/series/list?page=0&size={size}"),
                        Some(serde_json::json!({ "fullTextSearch": query })),
                        "application/json",
                    )
                    .await?;
                let value = self.json(request, MAX_JSON_BYTES).await?;
                Ok(parse_komga_series_list(&value))
            }
            CustomSourceKind::Kavita => {
                let request = self
                    .request(
                        source_id,
                        kind,
                        LibraryMethod::Get,
                        format!(
                            "/api/Search/search?queryString={}&includeChapterAndFiles=false",
                            encode_query(query)
                        ),
                        None,
                        "application/json",
                    )
                    .await?;
                let value = self.json(request, MAX_JSON_BYTES).await?;
                Ok(parse_kavita_search(&value))
            }
            _ => Err(invalid_argument("未知漫画库来源")),
        }
    }

    /// 读取作品详情（标题、简介、年份）。
    pub async fn series_detail(
        &self,
        source_id: &str,
        series_id: &str,
    ) -> Result<SeriesHit, AppError> {
        let kind = Self::kind_of(source_id)?;
        if !haven_application::services::source_import::is_comic_library_opaque_id(series_id) {
            return Err(invalid_argument("漫画库作品标识非法"));
        }
        match kind {
            CustomSourceKind::Komga => {
                let request = self
                    .request(
                        source_id,
                        kind,
                        LibraryMethod::Get,
                        format!("/api/v1/series/{}", encode_query(series_id)),
                        None,
                        "application/json",
                    )
                    .await?;
                let value = self.json(request, MAX_JSON_BYTES).await?;
                parse_komga_series(&value).ok_or_else(|| source_unavailable("Komga 作品不存在"))
            }
            CustomSourceKind::Kavita => {
                let request = self
                    .request(
                        source_id,
                        kind,
                        LibraryMethod::Get,
                        format!("/api/Series/{}", encode_query(series_id)),
                        None,
                        "application/json",
                    )
                    .await?;
                let value = self.json(request, MAX_JSON_BYTES).await?;
                parse_kavita_series(&value).ok_or_else(|| source_unavailable("Kavita 作品不存在"))
            }
            _ => Err(invalid_argument("未知漫画库来源")),
        }
    }

    /// 读取有界章节目录。分页循环受 [`MAX_CHAPTERS`] 约束；达到上限时把目录标记为
    /// truncated，而不是静默返回一个前缀让人以为结果完整。
    pub async fn chapter_catalog(
        &self,
        source_id: &str,
        series_id: &str,
    ) -> Result<(Vec<ChapterHit>, Option<u32>, bool), AppError> {
        let kind = Self::kind_of(source_id)?;
        if !haven_application::services::source_import::is_comic_library_opaque_id(series_id) {
            return Err(invalid_argument("漫画库作品标识非法"));
        }
        match kind {
            CustomSourceKind::Komga => self.komga_chapters(source_id, series_id).await,
            CustomSourceKind::Kavita => self.kavita_chapters(source_id, series_id).await,
            _ => Err(invalid_argument("未知漫画库来源")),
        }
    }

    async fn komga_chapters(
        &self,
        source_id: &str,
        series_id: &str,
    ) -> Result<(Vec<ChapterHit>, Option<u32>, bool), AppError> {
        let kind = CustomSourceKind::Komga;
        let mut chapters = Vec::new();
        let mut page = 0usize;
        let mut total = None;
        let mut raw_seen = 0usize;
        let mut truncated = false;
        loop {
            let request = self
                .request(
                    source_id,
                    kind,
                    LibraryMethod::Post,
                    format!("/api/v1/books/list?page={page}&size={CATALOG_PAGE_SIZE}"),
                    Some(serde_json::json!({
                        "condition": { "type": "SeriesId", "seriesId": series_id }
                    })),
                    "application/json",
                )
                .await?;
            let value = self.json(request, MAX_JSON_BYTES).await?;
            let page_total = value
                .get("totalElements")
                .and_then(Value::as_u64)
                .and_then(|count| u32::try_from(count).ok());
            if page_total.is_some() {
                total = page_total;
            }
            let content = value
                .get("content")
                .and_then(Value::as_array)
                .ok_or_else(|| source_unavailable("Komga 章节列表结构异常"))?;
            let raw_len = content.len();
            // 上限必须按服务端返回的**原始条目**计，而不是解析成功的章节数：一整页
            // 都是畸形条目时 `chapters` 不增长，只看 `chapters.len()` 会让分页循环
            // 永远无法终止。原始条目数只增不减，因此循环最多
            // `ceil(MAX_CHAPTERS / CATALOG_PAGE_SIZE)` 次请求。
            raw_seen = raw_seen.saturating_add(raw_len);
            chapters.extend(content.iter().filter_map(parse_komga_book));
            page += 1;
            if raw_len < CATALOG_PAGE_SIZE {
                break;
            }
            if raw_seen >= MAX_CHAPTERS {
                truncated = true;
                break;
            }
        }
        if chapters.len() > MAX_CHAPTERS {
            chapters.truncate(MAX_CHAPTERS);
            truncated = true;
        }
        Ok((chapters, total, truncated))
    }

    async fn kavita_chapters(
        &self,
        source_id: &str,
        series_id: &str,
    ) -> Result<(Vec<ChapterHit>, Option<u32>, bool), AppError> {
        let kind = CustomSourceKind::Kavita;
        let request = self
            .request(
                source_id,
                kind,
                LibraryMethod::Get,
                format!(
                    "/api/Search/chapters-by-series?seriesId={}",
                    encode_query(series_id)
                ),
                None,
                "application/json",
            )
            .await?;
        let value = self.json(request, MAX_JSON_BYTES).await?;
        let chapters: Vec<ChapterHit> = value
            .as_array()
            .ok_or_else(|| source_unavailable("Kavita 章节列表结构异常"))?
            .iter()
            .filter_map(parse_kavita_chapter)
            .collect();
        let truncated = chapters.len() > MAX_CHAPTERS;
        let mut chapters = chapters;
        if truncated {
            chapters.truncate(MAX_CHAPTERS);
        }
        let total = u32::try_from(chapters.len()).ok();
        Ok((chapters, total, truncated))
    }

    /// 读取章节页清单（页标识由本模块生成，不接受服务端 URL）。
    pub async fn chapter_pages(
        &self,
        source_id: &str,
        chapter_id: &str,
    ) -> Result<Vec<ChapterPage>, AppError> {
        let kind = Self::kind_of(source_id)?;
        if !haven_application::services::source_import::is_comic_library_opaque_id(chapter_id) {
            return Err(invalid_argument("漫画库章节标识非法"));
        }
        match kind {
            CustomSourceKind::Komga => {
                let request = self
                    .request(
                        source_id,
                        kind,
                        LibraryMethod::Get,
                        format!("/api/v1/books/{}/pages", encode_query(chapter_id)),
                        None,
                        "application/json",
                    )
                    .await?;
                let value = self.json(request, MAX_JSON_BYTES).await?;
                let pages = value
                    .as_array()
                    .ok_or_else(|| source_unavailable("Komga 页面列表结构异常"))?;
                if pages.is_empty() || pages.len() > MAX_CHAPTER_PAGES {
                    return Err(source_unavailable("Komga 页面数量超出限制"));
                }
                Ok((0..pages.len())
                    .map(|index| ChapterPage {
                        number: index as u32 + 1,
                        name: format!("page-{:04}", index + 1),
                    })
                    .collect())
            }
            CustomSourceKind::Kavita => {
                let request = self
                    .request(
                        source_id,
                        kind,
                        LibraryMethod::Get,
                        format!(
                            "/api/Reader/chapter-info?chapterId={}",
                            encode_query(chapter_id)
                        ),
                        None,
                        "application/json",
                    )
                    .await?;
                let value = self.json(request, MAX_JSON_BYTES).await?;
                let count = json_u32(&value, &["pageNumber", "pages", "pageCount"])
                    .ok_or_else(|| source_unavailable("Kavita 章节页数不可用"))?;
                if count == 0 || count as usize > MAX_CHAPTER_PAGES {
                    return Err(source_unavailable("Kavita 页面数量超出限制"));
                }
                Ok((0..count)
                    .map(|index| ChapterPage {
                        number: index,
                        name: format!("page-{:04}", index + 1),
                    })
                    .collect())
            }
            _ => Err(invalid_argument("未知漫画库来源")),
        }
    }

    async fn page_bytes(
        &self,
        source_id: &str,
        kind: CustomSourceKind,
        chapter_id: &str,
        page: &ChapterPage,
    ) -> Result<Vec<u8>, AppError> {
        let path = match kind {
            CustomSourceKind::Komga => format!(
                "/api/v1/books/{}/pages/{}/raw",
                encode_query(chapter_id),
                page.number
            ),
            CustomSourceKind::Kavita => format!(
                "/api/Reader/image?chapterId={}&page={}",
                encode_query(chapter_id),
                page.number
            ),
            _ => return Err(invalid_argument("未知漫画库来源")),
        };
        let request = self
            .request(source_id, kind, LibraryMethod::Get, path, None, "image/*")
            .await?;
        let response = self.gateway.send(request, MAX_PAGE_BYTES).await?;
        Ok(response.bytes)
    }

    /// 读取整章归档。服务端响应只通过 ZIP 本地文件头做快速识别，并不验证完整归档结构；
    /// 没有该文件头或归档端点不可用时回退到逐页组装。带有文件头但内容损坏的归档仍可能
    /// 被原样保存，阅读器届时可能无法打开。
    async fn chapter_archive(
        &self,
        source_id: &str,
        chapter_id: &str,
        destination: &std::path::Path,
    ) -> Result<RemoteAcquiredFile, AppError> {
        let kind = Self::kind_of(source_id)?;
        let archive_path = match kind {
            CustomSourceKind::Komga => {
                format!("/api/v1/books/{}/file", encode_query(chapter_id))
            }
            CustomSourceKind::Kavita => {
                format!(
                    "/api/Download/chapter?chapterId={}",
                    encode_query(chapter_id)
                )
            }
            _ => return Err(invalid_argument("未知漫画库来源")),
        };
        let request = self
            .request(
                source_id,
                kind,
                LibraryMethod::Get,
                archive_path,
                None,
                "application/zip,application/octet-stream,*/*;q=0.1",
            )
            .await?;
        match self.gateway.send(request, MAX_ARCHIVE_BYTES).await {
            Ok(response) if response.bytes.starts_with(b"PK\x03\x04") => {
                let size = write_bytes_to(destination.to_path_buf(), response.bytes).await?;
                Ok(RemoteAcquiredFile {
                    size_bytes: size,
                    mime: "application/vnd.comicbook+zip".to_owned(),
                })
            }
            // 无 ZIP 本地文件头（例如 CBR）或归档端点不可用时，改用逐页组装，
            // 由本地 ZIP writer 生成 CBZ。
            Ok(_) | Err(_) => {
                self.assemble_chapter_cbz(source_id, chapter_id, destination)
                    .await
            }
        }
    }

    async fn assemble_chapter_cbz(
        &self,
        source_id: &str,
        chapter_id: &str,
        destination: &std::path::Path,
    ) -> Result<RemoteAcquiredFile, AppError> {
        let kind = Self::kind_of(source_id)?;
        let pages = self.chapter_pages(source_id, chapter_id).await?;
        let mut collected: Vec<(String, Vec<u8>)> = Vec::with_capacity(pages.len());
        let mut total = 0usize;
        for page in &pages {
            let bytes = self.page_bytes(source_id, kind, chapter_id, page).await?;
            let extension = image_extension(&bytes)
                .ok_or_else(|| source_unavailable("漫画库返回了非图片页面"))?;
            total = total.saturating_add(bytes.len());
            if total > MAX_CHAPTER_TOTAL_BYTES {
                return Err(source_unavailable("漫画库章节总大小超出限制"));
            }
            collected.push((format!("{}.{}", page.name, extension), bytes));
        }
        let size = write_cbz_to(destination.to_path_buf(), collected).await?;
        Ok(RemoteAcquiredFile {
            size_bytes: size,
            mime: "application/vnd.comicbook+zip".to_owned(),
        })
    }

    fn catalog_for(
        &self,
        source_key: &'static str,
        work_ref: &str,
        chapters: Vec<ChapterHit>,
        total: Option<u32>,
        truncated: bool,
    ) -> Result<ComicChapterCatalog, AppError> {
        let entries: Vec<ComicChapterCatalogEntry> = chapters
            .iter()
            .filter_map(|chapter| catalog_entry(source_key, work_ref, chapter))
            .collect();
        ComicChapterCatalog::new_with_coverage(
            source_key,
            work_ref,
            entries,
            UtcMillis::now(),
            total,
            truncated,
        )
        .ok_or_else(|| source_unavailable("漫画库章节目录身份无效"))
    }
}

/// 一个页面：`number` 是 provider 侧页码（Komga 1-based / Kavita 0-based），
/// `name` 是本地生成的稳定页名（不含任何服务端 URL 或路径）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChapterPage {
    pub number: u32,
    pub name: String,
}

#[async_trait]
impl SourceCatalogProvider for ComicLibraryProvider {
    async fn detail(
        &self,
        source_id: &str,
        _endpoint: &str,
        external_id: &str,
    ) -> Result<SourceCatalogEntry, AppError> {
        let (embedded, series_id) = split_comic_library_work_ref(external_id)?;
        if embedded != source_id {
            return Err(invalid_argument("漫画库作品身份与来源不一致"));
        }
        let source_key = comic_library_source_key(source_id)?;
        let series = self.series_detail(source_id, &series_id).await?;
        let (chapters, total, truncated) = self.chapter_catalog(source_id, &series_id).await?;
        let catalog = self.catalog_for(source_key, external_id, chapters, total, truncated)?;
        let first_chapter_id = catalog
            .readable_chapters()
            .next()
            .map(|chapter| chapter.identity.remote_chapter_id.clone())
            .ok_or_else(|| source_unavailable("该漫画库作品暂时没有可阅读章节"))?;
        Ok(SourceCatalogEntry {
            external_id: external_id.to_owned(),
            title: series.title,
            year: series.year,
            type_name: Some("漫画".to_owned()),
            // Komga/Kavita 的封面需要 API key 才能取，而图片代理不带凭据；
            // 这里不投影一个必然失败的封面地址，交给默认封面处理。
            pic: None,
            episodes: Vec::new(),
            content: series.summary,
            director: None,
            actor: None,
            local_file: None,
            media_type: Some(MediaType::Comic),
            remote: Some(RemoteContentRef {
                source_key: source_key.to_owned(),
                remote_id: format!("{external_id}:{first_chapter_id}"),
                media_type: MediaType::Comic,
                mime_type: Some("application/vnd.comicbook+zip".to_owned()),
            }),
            comic_catalog: Some(catalog),
        })
    }

    async fn comic_chapter_catalog(
        &self,
        source_id: &str,
        _endpoint: &str,
        external_id: &str,
    ) -> Result<Option<ComicChapterCatalog>, AppError> {
        let (embedded, series_id) = split_comic_library_work_ref(external_id)?;
        if embedded != source_id {
            return Err(invalid_argument("漫画库作品身份与来源不一致"));
        }
        let source_key = comic_library_source_key(source_id)?;
        let (chapters, total, truncated) = self.chapter_catalog(source_id, &series_id).await?;
        Ok(Some(self.catalog_for(
            source_key,
            external_id,
            chapters,
            total,
            truncated,
        )?))
    }
}

#[async_trait]
impl RemoteComicPageProvider for ComicLibraryProvider {
    async fn inspect(&self, session: &PreparedSession) -> Result<Vec<PreparedComicPage>, AppError> {
        let (source_id, source_key, chapter_id) = session_remote_chapter(session)?;
        let pages = self.chapter_pages(&source_id, &chapter_id).await?;
        Ok(pages
            .into_iter()
            .map(|page| PreparedComicPage {
                availability: PreparedComicPageAvailability::Ready,
                identity: page_identity_for_provider(source_key, &page.name, None),
                source: PreparedComicPageSource::RemotePage {
                    page_name: page.name,
                },
            })
            .collect())
    }

    async fn read_page(
        &self,
        session: &PreparedSession,
        page: &PreparedComicPage,
    ) -> Result<ComicPageBody, AppError> {
        let (source_id, _source_key, chapter_id) = session_remote_chapter(session)?;
        let PreparedComicPageSource::RemotePage { page_name } = &page.source else {
            return Err(invalid_argument("漫画页面身份无效"));
        };
        let kind = Self::kind_of(&source_id)?;
        // 页标识必须仍然属于当前章节的页清单：不接受前端提交的任意页名。
        let pages = self.chapter_pages(&source_id, &chapter_id).await?;
        let resolved = pages
            .into_iter()
            .find(|candidate| &candidate.name == page_name)
            .ok_or_else(|| source_unavailable("漫画页面已不存在"))?;
        let bytes = self
            .page_bytes(&source_id, kind, &chapter_id, &resolved)
            .await?;
        let mime_type =
            image_mime(&bytes).ok_or_else(|| source_unavailable("漫画库返回了非图片页面"))?;
        Ok(ComicPageBody { mime_type, bytes })
    }
}

#[async_trait]
impl RemoteAcquisitionPort for ComicLibraryProvider {
    async fn acquire(
        &self,
        source_key: &str,
        remote_id: &str,
        destination: &std::path::Path,
    ) -> Result<RemoteAcquiredFile, AppError> {
        if !matches!(source_key, KOMGA_SOURCE_KEY | KAVITA_SOURCE_KEY) {
            return Err(invalid_argument("该来源不支持漫画库离线获取"));
        }
        if destination.as_os_str().is_empty() {
            return Err(storage_error("漫画临时文件路径无效"));
        }
        let (source_id, _series_id, chapter_id) = split_comic_library_chapter_remote_id(remote_id)?;
        // 章节来源必须与来源键一致：komga 的 remote_id 不能借 kavita 的键。
        if comic_library_source_key(&source_id)? != source_key {
            return Err(invalid_argument("漫画库章节身份与来源不一致"));
        }
        self.chapter_archive(&source_id, &chapter_id, destination)
            .await
    }
}

/// 从会话里取出漫画库来源身份；非漫画库远端会话明确失败。
fn session_remote_chapter(
    session: &PreparedSession,
) -> Result<(String, &'static str, String), AppError> {
    let PreparedSessionSource::Remote {
        source_key,
        remote_id,
        ..
    } = &session.source
    else {
        return Err(invalid_argument("漫画会话不是远端来源"));
    };
    let key = match source_key.as_str() {
        KOMGA_SOURCE_KEY => KOMGA_SOURCE_KEY,
        KAVITA_SOURCE_KEY => KAVITA_SOURCE_KEY,
        _ => return Err(invalid_argument("该来源不支持在线漫画")),
    };
    let (source_id, _series_id, chapter_id) = split_comic_library_chapter_remote_id(remote_id)?;
    if comic_library_source_key(&source_id)? != key {
        return Err(invalid_argument("漫画库章节身份与来源不一致"));
    }
    Ok((source_id, key, chapter_id))
}

// ---------- 搜索参与者 ----------

/// 一个前缀一个实例：`custom_komga_` 与 `custom_kavita_` 各自由自己的参与者承接。
pub struct ComicLibrarySearchParticipant {
    registry: SourceRegistryService,
    gateway: Arc<dyn ComicLibraryGateway>,
    api_key: Arc<ApiKeyResolver>,
    prefix: &'static str,
}

impl ComicLibrarySearchParticipant {
    pub fn new(
        registry: SourceRegistryService,
        gateway: Arc<dyn ComicLibraryGateway>,
        api_key: Arc<ApiKeyResolver>,
        prefix: &'static str,
    ) -> Self {
        Self {
            registry,
            gateway,
            api_key,
            prefix,
        }
    }

    fn provider(&self) -> ComicLibraryProvider {
        ComicLibraryProvider::new(
            self.registry.clone(),
            self.gateway.clone(),
            self.api_key.clone(),
        )
    }
}

#[async_trait]
impl SearchSourceParticipant for ComicLibrarySearchParticipant {
    fn source_id(&self) -> &str {
        self.prefix
    }

    fn id_prefix(&self) -> Option<&str> {
        Some(self.prefix)
    }

    fn supports_category(&self, category: Option<QueryCategory>) -> bool {
        matches!(
            category,
            None | Some(QueryCategory::All) | Some(QueryCategory::Comic)
        )
    }

    async fn search(
        &self,
        query: &str,
        limit: u32,
        is_cancelled: &(dyn Fn() -> bool + Send + Sync),
    ) -> Result<Vec<WorkCardDto>, AppError> {
        let _ = (query, limit, is_cancelled);
        Ok(Vec::new())
    }

    async fn search_for(
        &self,
        dispatched_id: &str,
        query: &str,
        limit: u32,
        is_cancelled: &(dyn Fn() -> bool + Send + Sync),
    ) -> Result<Vec<WorkCardDto>, AppError> {
        if !dispatched_id.starts_with(self.prefix)
            || !SourceRegistryService::is_comic_library_source_id(dispatched_id)
        {
            return Ok(Vec::new());
        }
        if self.registry.endpoint(dispatched_id).await?.is_none() {
            return Ok(Vec::new());
        }
        if is_cancelled() {
            return Ok(Vec::new());
        }
        let hits = self
            .provider()
            .search_series(dispatched_id, query, limit)
            .await?;
        if is_cancelled() {
            return Ok(Vec::new());
        }
        Ok(hits
            .into_iter()
            .take(limit as usize)
            .map(|hit| card(dispatched_id, &hit))
            .collect())
    }
}

/// 搜索卡片只携带 opaque 候选句柄：句柄里是来源 sourceId 与经校验的 seriesId，
/// 没有漫画库地址、API key 或任何 URL。
fn card(source_id: &str, hit: &SeriesHit) -> WorkCardDto {
    WorkCardDto {
        work_id: comic_library_candidate_handle(source_id, &hit.series_id),
        title: hit.title.clone(),
        original_title: None,
        description: hit.summary.clone(),
        categories: vec![ContentCategory::Comic],
        available_media_types: vec![MediaTypeDto::Comic],
        poster_uri: None,
        backdrop_uri: None,
        release_year: hit.year,
        rating_value: None,
        rating_scale: None,
        favorite: false,
        progress: None,
        primary_action: None,
        external_ids: Vec::new(),
    }
}

// ---------- 响应解析（字段来自官方 OpenAPI 投影，全部 fail closed） ----------

/// Komga `PageSeriesDto.content[]` → 搜索结果。
fn parse_komga_series_list(value: &Value) -> Vec<SeriesHit> {
    value
        .get("content")
        .and_then(Value::as_array)
        .map(|items| items.iter().filter_map(parse_komga_series).collect())
        .unwrap_or_default()
}

fn parse_komga_series(value: &Value) -> Option<SeriesHit> {
    let series_id = json_string(value, &["id"])?;
    if !haven_application::services::source_import::is_comic_library_opaque_id(&series_id) {
        return None;
    }
    let title = json_string(value, &["metadata.title", "metadata.titleSort", "name"])
        .filter(|title| !title.trim().is_empty())?;
    Some(SeriesHit {
        series_id,
        title: bounded_text(&title),
        summary: json_string(value, &["metadata.summary"]).map(|text| bounded_text(&text)),
        year: json_year(value, &["metadata.releaseDate", "metadata.year"]),
    })
}

/// Komga `PageBookDto.content[]` → 章节。
fn parse_komga_book(value: &Value) -> Option<ChapterHit> {
    let chapter_id = json_string(value, &["id"])?;
    if !haven_application::services::source_import::is_comic_library_opaque_id(&chapter_id) {
        return None;
    }
    let page_count = json_u32(value, &["media.pagesCount"]).or_else(|| json_u32(value, &["pages"]));
    let number = json_f64(value, &["metadata.numberSort"]).or_else(|| {
        json_string(value, &["metadata.number"]).and_then(|raw| raw.trim().parse().ok())
    });
    Some(ChapterHit {
        chapter_id,
        number,
        volume: json_f64(value, &["metadata.volumeSort", "metadata.volume"]),
        title: json_string(value, &["metadata.title"]).map(|text| bounded_text(&text)),
        page_count,
        available: page_count.is_none_or(|count| count > 0),
    })
}

/// Kavita `SearchResultGroupDto.series[]` → 搜索结果。
fn parse_kavita_search(value: &Value) -> Vec<SeriesHit> {
    value
        .get("series")
        .and_then(Value::as_array)
        .map(|items| items.iter().filter_map(parse_kavita_series).collect())
        .unwrap_or_default()
}

fn parse_kavita_series(value: &Value) -> Option<SeriesHit> {
    let series_id = json_scalar_id(value, &["seriesId", "id"])?;
    if !haven_application::services::source_import::is_comic_library_opaque_id(&series_id) {
        return None;
    }
    let title = json_string(value, &["name", "localizedName", "originalName"])
        .filter(|title| !title.trim().is_empty())?;
    Some(SeriesHit {
        series_id,
        title: bounded_text(&title),
        summary: json_string(value, &["summary"]).map(|text| bounded_text(&text)),
        year: json_year(value, &["releaseDate", "year"]),
    })
}

/// Kavita `ChapterDto` → 章节。
fn parse_kavita_chapter(value: &Value) -> Option<ChapterHit> {
    let chapter_id = json_scalar_id(value, &["id", "chapterId"])?;
    if !haven_application::services::source_import::is_comic_library_opaque_id(&chapter_id) {
        return None;
    }
    let page_count = json_u32(value, &["pages", "pagesCount", "pageCount"]);
    let number = json_f64(value, &["number", "chapterNumber"])
        .or_else(|| json_string(value, &["number"]).and_then(|raw| raw.trim().parse().ok()));
    Some(ChapterHit {
        chapter_id,
        number,
        volume: json_f64(value, &["volumeNumber"]),
        title: json_string(value, &["title", "range"]).map(|text| bounded_text(&text)),
        page_count,
        available: page_count.is_none_or(|count| count > 0),
    })
}

fn catalog_entry(
    source_key: &str,
    work_ref: &str,
    chapter: &ChapterHit,
) -> Option<ComicChapterCatalogEntry> {
    Some(ComicChapterCatalogEntry {
        identity: ChapterSourceIdentity::new(source_key, work_ref, &chapter.chapter_id)?,
        metadata: ComicChapterMetadata {
            edition_profile: EditionProfile {
                language: IdentityFacet::unknown(),
                translation_line: IdentityFacet::unknown(),
                scan_group: ScanGroupFacet::NotApplicable,
                color_mode: Default::default(),
            },
            chapter_number: chapter.number,
            volume_number: chapter.volume,
            title: chapter.title.clone(),
            page_count: chapter.page_count,
            authoritative_content_key: None,
        },
        availability: if chapter.available {
            ComicChapterAvailability::Available
        } else {
            ComicChapterAvailability::TemporarilyUnavailable
        },
        published_at: None,
        updated_at: None,
    })
}

// ---------- JSON 辅助 ----------

/// 取嵌套字符串（点号路径，例如 `metadata.title`）。
fn json_string(value: &Value, paths: &[&str]) -> Option<String> {
    paths
        .iter()
        .find_map(|path| value_at(value, path).and_then(Value::as_str))
        .map(str::to_owned)
}

/// 取数值或整数字符串形式 ID（Kavita 的 `seriesId` 是整数，Komga 的是字符串）。
fn json_scalar_id(value: &Value, paths: &[&str]) -> Option<String> {
    for path in paths {
        match value_at(value, path) {
            Some(Value::String(text)) => return Some(text.clone()),
            Some(Value::Number(number)) => return Some(number.to_string()),
            _ => {}
        }
    }
    None
}

fn json_u32(value: &Value, paths: &[&str]) -> Option<u32> {
    paths.iter().find_map(|path| {
        let value = value_at(value, path)?;
        value
            .as_u64()
            .and_then(|raw| u32::try_from(raw).ok())
            .or_else(|| value.as_str().and_then(|raw| raw.trim().parse().ok()))
    })
}

fn json_f64(value: &Value, paths: &[&str]) -> Option<f64> {
    paths.iter().find_map(|path| {
        let value = value_at(value, path)?;
        value
            .as_f64()
            .filter(|number| number.is_finite())
            .or_else(|| {
                value
                    .as_str()
                    .and_then(|raw| raw.trim().parse::<f64>().ok())
                    .filter(|number| number.is_finite())
            })
    })
}

/// 从日期文本或年份数字里取四位年份。
fn json_year(value: &Value, paths: &[&str]) -> Option<i32> {
    for path in paths {
        let Some(value) = value_at(value, path) else {
            continue;
        };
        if let Some(number) = value.as_i64() {
            if (1..=9999).contains(&number) {
                return Some(number as i32);
            }
        }
        if let Some(text) = value.as_str() {
            if let Some(year) = text
                .split(|ch: char| !ch.is_ascii_digit())
                .find(|token| token.len() == 4)
                .and_then(|token| token.parse::<i32>().ok())
                .filter(|year| *year > 0)
            {
                return Some(year);
            }
        }
    }
    None
}

fn value_at<'a>(value: &'a Value, path: &str) -> Option<&'a Value> {
    let mut current = value;
    for segment in path.split('.') {
        current = current.get(segment)?;
    }
    Some(current)
}

fn bounded_text(value: &str) -> String {
    value.trim().chars().take(2_000).collect()
}

fn image_extension(bytes: &[u8]) -> Option<&'static str> {
    match image_mime(bytes)? {
        ComicImageMime::Jpeg => Some("jpg"),
        ComicImageMime::Png => Some("png"),
        ComicImageMime::Webp => Some("webp"),
    }
}

fn encode_query(value: &str) -> String {
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

// ---------- 错误 ----------

fn invalid_argument(message: &'static str) -> AppError {
    AppError::new("INVALID_ARGUMENT", ErrorKind::Validation, message, false)
}

fn security_denied(message: &'static str) -> AppError {
    AppError::new(
        "SECURITY_POLICY_DENIED",
        ErrorKind::Security,
        message,
        false,
    )
}

/// 固定安全文案：绝不回显漫画库地址、API key 或远端响应片段。
fn source_unavailable(message: &'static str) -> AppError {
    AppError::new("SOURCE_UNAVAILABLE", ErrorKind::Network, message, true)
}

fn storage_error(message: &'static str) -> AppError {
    AppError::new("STORAGE_ERROR", ErrorKind::Storage, message, true)
}

fn internal_error(message: &'static str) -> AppError {
    AppError::new("INTERNAL_ERROR", ErrorKind::Internal, message, false)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::repos::SqliteRepositories;
    use haven_application::services::source_import::CONTENT_CANDIDATE_PREFIX;
    use std::collections::HashMap;
    use std::sync::Mutex;

    const KOMGA_SERIES_LIST: &str = r#"{
        "content": [
            {"id": "series-a", "name": "文件夹名", "metadata": {"title": "钢之炼金术师", "summary": "简介"}, "booksCount": 2},
            {"id": "series-b", "name": "B", "metadata": {"title": "B 作品"}}
        ],
        "totalElements": 2, "totalPages": 1, "number": 0, "size": 20
    }"#;

    const KOMGA_BOOKS: &str = r#"{
        "content": [
            {"id": "book-1", "seriesId": "series-a", "name": "第 1 卷", "metadata": {"number": "1", "numberSort": 1.0, "title": "第一卷"}, "media": {"pagesCount": 3}},
            {"id": "book-2", "seriesId": "series-a", "name": "第 2 卷", "metadata": {"number": "2", "numberSort": 2.0}, "media": {"pagesCount": 0}}
        ],
        "totalElements": 2, "totalPages": 1, "number": 0, "size": 100
    }"#;

    const KOMGA_SERIES_DETAIL: &str = r#"{"id": "series-a", "name": "文件夹名", "metadata": {"title": "钢之炼金术师", "summary": "简介"}}"#;

    const KAVITA_SEARCH: &str = r#"{
        "series": [{"seriesId": 42, "libraryId": 1, "libraryName": "漫画", "name": "进击的巨人"}],
        "collections": [], "readingLists": [], "bookmarks": [], "persons": [], "genres": [], "tags": []
    }"#;

    const KAVITA_CHAPTERS: &str = r#"[
        {"id": 7, "number": "1", "title": "第一话", "pages": 4},
        {"id": 8, "number": "2", "pages": 0}
    ]"#;

    const KAVITA_CHAPTER_INFO: &str = r#"{"chapterId": 7, "chapterNumber": "1", "pageNumber": 4}"#;

    /// 记录每次请求的 fake gateway；按 path 前缀返回预置响应。
    struct FakeGateway {
        responses: Mutex<HashMap<String, Vec<u8>>>,
        requests: Mutex<Vec<LibraryRequest>>,
    }

    impl FakeGateway {
        fn new(entries: &[(&str, &str)]) -> Arc<Self> {
            let responses = entries
                .iter()
                .map(|(prefix, body)| ((*prefix).to_owned(), body.as_bytes().to_vec()))
                .collect();
            Arc::new(Self {
                responses: Mutex::new(responses),
                requests: Mutex::new(Vec::new()),
            })
        }

        fn requests(&self) -> Vec<LibraryRequest> {
            self.requests
                .lock()
                .unwrap_or_else(|error| error.into_inner())
                .clone()
        }
    }

    #[async_trait]
    impl ComicLibraryGateway for FakeGateway {
        async fn send(
            &self,
            request: LibraryRequest,
            _max_bytes: usize,
        ) -> Result<LibraryResponse, AppError> {
            let body = {
                let responses = self
                    .responses
                    .lock()
                    .unwrap_or_else(|error| error.into_inner());
                responses
                    .iter()
                    .find(|(prefix, _)| request.path.starts_with(prefix.as_str()))
                    .map(|(_, body)| body.clone())
            };
            self.requests
                .lock()
                .unwrap_or_else(|error| error.into_inner())
                .push(request);
            body.map(|bytes| LibraryResponse { bytes })
                .ok_or_else(|| source_unavailable("测试 gateway 未配置该路径"))
        }
    }

    fn registry() -> SourceRegistryService {
        let db = Arc::new(crate::Db::open_in_memory().unwrap());
        let repos = Arc::new(SqliteRepositories::new(db));
        SourceRegistryService::new(repos)
    }

    async fn komga_source_id(registry: &SourceRegistryService) -> String {
        registry
            .add_comic_library_source(
                "我的 Komga",
                "https://komga.example.invalid",
                CustomSourceKind::Komga,
            )
            .await
            .unwrap()
            .source_id
    }

    async fn kavita_source_id(registry: &SourceRegistryService) -> String {
        registry
            .add_comic_library_source(
                "我的 Kavita",
                "https://kavita.example.invalid",
                CustomSourceKind::Kavita,
            )
            .await
            .unwrap()
            .source_id
    }

    fn provider(
        registry: SourceRegistryService,
        gateway: Arc<FakeGateway>,
        key: Option<&'static str>,
    ) -> ComicLibraryProvider {
        let gateway: Arc<dyn ComicLibraryGateway> = gateway;
        let api_key: Arc<ApiKeyResolver> =
            Arc::new(move |_source_id: &str| Box::pin(async move { key.map(str::to_owned) }));
        ComicLibraryProvider::new(registry, gateway, api_key)
    }

    /// 一整页都是畸形条目（缺少 opaque id）的 Komga `books/list` 响应。
    fn komga_page_of_malformed_books(total_elements: u64) -> String {
        let content: Vec<Value> = (0..CATALOG_PAGE_SIZE)
            .map(|index| serde_json::json!({ "name": format!("缺 id 的第 {index} 条") }))
            .collect();
        serde_json::json!({
            "content": content,
            "totalElements": total_elements,
            "totalPages": 1_000,
            "number": 0,
            "size": CATALOG_PAGE_SIZE
        })
        .to_string()
    }

    #[tokio::test]
    async fn komga_search_maps_series_list_and_never_puts_the_key_in_the_url() {
        let registry = registry();
        let source_id = komga_source_id(&registry).await;
        let gateway = FakeGateway::new(&[("/api/v1/series/list", KOMGA_SERIES_LIST)]);
        let provider = provider(registry, gateway.clone(), Some("komga-secret"));

        let hits = provider.search_series(&source_id, "钢", 10).await.unwrap();
        assert_eq!(hits.len(), 2);
        assert_eq!(hits[0].series_id, "series-a");
        assert_eq!(hits[0].title, "钢之炼金术师");
        assert_eq!(hits[0].summary.as_deref(), Some("简介"));

        let requests = gateway.requests();
        assert_eq!(requests.len(), 1);
        assert_eq!(requests[0].method, LibraryMethod::Post);
        assert_eq!(requests[0].path, "/api/v1/series/list?page=0&size=10");
        assert_eq!(
            requests[0].json_body.as_ref().unwrap()["fullTextSearch"],
            Value::String("钢".to_owned())
        );
        assert_eq!(requests[0].api_key_header, "X-API-Key");
        assert_eq!(requests[0].api_key.as_deref(), Some("komga-secret"));
        assert!(
            !requests[0].path.contains("komga-secret"),
            "API key 绝不能出现在请求路径或查询串里"
        );
        assert!(!requests[0].endpoint.contains("komga-secret"));
    }

    #[tokio::test]
    async fn komga_chapter_catalog_is_bounded_and_keeps_unavailable_chapters() {
        let registry = registry();
        let source_id = komga_source_id(&registry).await;
        let gateway = FakeGateway::new(&[("/api/v1/books/list", KOMGA_BOOKS)]);
        let provider = provider(registry, gateway.clone(), None);
        let work_ref = format!("{source_id}:series-a");

        let (chapters, total, truncated) = provider
            .chapter_catalog(&source_id, "series-a")
            .await
            .unwrap();
        assert_eq!(chapters.len(), 2);
        assert_eq!(total, Some(2));
        assert!(!truncated);
        assert_eq!(chapters[0].number, Some(1.0));
        assert_eq!(chapters[0].page_count, Some(3));
        assert!(chapters[0].available);
        assert!(!chapters[1].available, "0 页章节必须标记为不可用");

        let catalog = provider
            .catalog_for(KOMGA_SOURCE_KEY, &work_ref, chapters, total, truncated)
            .unwrap();
        assert_eq!(catalog.source_key, KOMGA_SOURCE_KEY);
        assert_eq!(catalog.remote_work_id, work_ref);
        assert_eq!(catalog.readable_chapters().count(), 1);
    }

    /// 每一页都是满页畸形条目时，分页循环必须按「服务端返回的原始条目数」封顶而
    /// 终止，而不是因为解析成功数为 0 就无限请求；同时必须如实标记 truncated，
    /// 并保持 `totalElements` 语义不变。
    #[tokio::test]
    async fn komga_chapter_catalog_terminates_on_repeated_full_pages_of_invalid_entries() {
        let registry = registry();
        let source_id = komga_source_id(&registry).await;
        let body = komga_page_of_malformed_books(100_000);
        let gateway = FakeGateway::new(&[("/api/v1/books/list", body.as_str())]);
        let provider = provider(registry, gateway.clone(), None);

        let (chapters, total, truncated) = provider
            .chapter_catalog(&source_id, "series-a")
            .await
            .unwrap();

        assert!(chapters.is_empty(), "畸形条目不应产出任何章节");
        assert!(truncated, "达到原始条目上限时必须如实报告截断");
        assert_eq!(total, Some(100_000), "totalElements 语义保持不变");

        let requests = gateway.requests();
        let page_bound = MAX_CHAPTERS / CATALOG_PAGE_SIZE;
        assert_eq!(
            requests.len(),
            page_bound,
            "必须在原始条目上限处停止分页，而不是继续请求"
        );
        assert_eq!(
            requests[page_bound - 1].path,
            format!(
                "/api/v1/books/list?page={}&size={CATALOG_PAGE_SIZE}",
                page_bound - 1
            )
        );
    }

    #[tokio::test]
    async fn kavita_search_projects_only_the_series_group() {
        let registry = registry();
        let source_id = kavita_source_id(&registry).await;
        let gateway = FakeGateway::new(&[("/api/Search/search", KAVITA_SEARCH)]);
        let provider = provider(registry, gateway.clone(), Some("kavita-secret"));

        let hits = provider.search_series(&source_id, "巨人", 5).await.unwrap();
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].series_id, "42");
        assert_eq!(hits[0].title, "进击的巨人");

        let requests = gateway.requests();
        assert_eq!(requests[0].method, LibraryMethod::Get);
        assert_eq!(
            requests[0].path,
            "/api/Search/search?queryString=%E5%B7%A8%E4%BA%BA&includeChapterAndFiles=false"
        );
        assert_eq!(requests[0].api_key_header, "x-api-key");
        assert!(!requests[0].path.contains("kavita-secret"));
    }

    #[tokio::test]
    async fn kavita_chapter_pages_come_from_chapter_info_not_from_server_urls() {
        let registry = registry();
        let source_id = kavita_source_id(&registry).await;
        let gateway = FakeGateway::new(&[
            ("/api/Search/chapters-by-series", KAVITA_CHAPTERS),
            ("/api/Reader/chapter-info", KAVITA_CHAPTER_INFO),
        ]);
        let provider = provider(registry, gateway.clone(), None);

        let (chapters, _total, truncated) =
            provider.chapter_catalog(&source_id, "42").await.unwrap();
        assert_eq!(chapters.len(), 2);
        assert!(!truncated);

        let pages = provider.chapter_pages(&source_id, "7").await.unwrap();
        assert_eq!(pages.len(), 4);
        assert_eq!(pages[0].number, 0, "Kavita 页码是 0-based");
        assert_eq!(pages[0].name, "page-0001");
    }

    #[tokio::test]
    async fn komga_detail_builds_a_chapter_level_remote_identity() {
        let registry = registry();
        let source_id = komga_source_id(&registry).await;
        let gateway = FakeGateway::new(&[
            ("/api/v1/series/series-a", KOMGA_SERIES_DETAIL),
            ("/api/v1/books/list", KOMGA_BOOKS),
        ]);
        let provider = provider(registry, gateway.clone(), None);
        let work_ref = format!("{source_id}:series-a");

        let entry = provider.detail(&source_id, "", &work_ref).await.unwrap();
        assert_eq!(entry.title, "钢之炼金术师");
        assert_eq!(entry.media_type, Some(MediaType::Comic));
        let remote = entry.remote.expect("必须是可导入的远端身份");
        assert_eq!(remote.source_key, KOMGA_SOURCE_KEY);
        assert_eq!(remote.remote_id, format!("{work_ref}:book-1"));
        assert!(entry.comic_catalog.is_some());
        assert!(
            !remote.remote_id.contains("komga.example.invalid"),
            "远端身份不得携带漫画库地址"
        );
    }

    #[tokio::test]
    async fn candidate_handles_are_opaque_and_carry_no_endpoint_or_key() {
        let registry = registry();
        let source_id = komga_source_id(&registry).await;
        let gateway = FakeGateway::new(&[("/api/v1/series/list", KOMGA_SERIES_LIST)]);
        let provider = provider(registry, gateway, Some("komga-secret"));
        let hits = provider.search_series(&source_id, "", 10).await.unwrap();
        let handle = card(&source_id, &hits[0]).work_id;
        assert!(handle.starts_with(CONTENT_CANDIDATE_PREFIX));
        for leaked in ["komga-secret", "komga.example.invalid", "https://"] {
            assert!(
                !handle.contains(leaked),
                "候选句柄泄漏了 {leaked}: {handle}"
            );
        }
    }

    #[tokio::test]
    async fn page_reads_reject_page_names_outside_the_current_chapter() {
        let registry = registry();
        let source_id = komga_source_id(&registry).await;
        let gateway = FakeGateway::new(&[("/api/v1/books/book-1/pages/1/raw", "not-an-image")]);
        let provider = provider(registry, gateway, None);
        let work_ref = format!("{source_id}:series-a");
        let session = PreparedSession {
            work_id: "work".into(),
            edition_id: "edition".into(),
            media_item_id: uuid::Uuid::now_v7().to_string(),
            engine: haven_application::wire::SessionEngineDto::Comic,
            resource_id: haven_domain::ids::ResourceId::new(),
            storage_location_id: None,
            canonical_root: None,
            canonical_file: None,
            subtitle_tracks: Vec::new(),
            source: PreparedSessionSource::Remote {
                source_id: haven_application::services::source_import::stable_source_id(
                    KOMGA_SOURCE_KEY,
                )
                .unwrap(),
                source_key: KOMGA_SOURCE_KEY.to_owned(),
                remote_id: format!("{work_ref}:book-1"),
            },
            mime_type: Some("application/vnd.comicbook+zip".to_owned()),
            media_type: MediaType::Comic,
            resource_type: haven_domain::enums::ResourceType::ComicArchive,
            comic_pages: None,
            progress: None,
        };
        let page = PreparedComicPage {
            availability: PreparedComicPageAvailability::Ready,
            identity: page_identity_for_provider(KOMGA_SOURCE_KEY, "page-0001", None),
            source: PreparedComicPageSource::RemotePage {
                page_name: "page-0001".to_owned(),
            },
        };
        // page-0001 不在服务端返回的页清单里（测试未配置 pages 端点）→ fail closed。
        assert!(provider.read_page(&session, &page).await.is_err());
    }

    #[tokio::test]
    async fn acquire_rejects_a_chapter_identity_from_the_other_library_kind() {
        let registry = registry();
        let gateway = FakeGateway::new(&[]);
        let provider = provider(registry, gateway, None);
        let directory = tempfile::tempdir().unwrap();
        // komga 的来源键不能接受 kavita 的远端身份。
        let remote_id = format!("{}:series-a:book-1", "custom_kavita_0123456789ab");
        let error = provider
            .acquire(
                KOMGA_SOURCE_KEY,
                &remote_id,
                &directory.path().join("chapter.cbz"),
            )
            .await
            .unwrap_err();
        assert_eq!(error.code().as_str(), "INVALID_ARGUMENT");
    }

    #[tokio::test]
    async fn missing_endpoint_reports_a_retryable_source_error_without_the_id() {
        let registry = registry();
        let gateway = FakeGateway::new(&[("/api/v1/series/list", KOMGA_SERIES_LIST)]);
        let provider = provider(registry, gateway.clone(), None);
        let error = provider
            .search_series("custom_komga_ffffffffffff", "", 10)
            .await
            .unwrap_err();
        assert_eq!(error.code().as_str(), "SOURCE_UNAVAILABLE");
        assert!(error.retryable());
        assert!(!error.user_message().contains("custom_komga_"));
        assert!(gateway.requests().is_empty(), "未登记端点时不得发起请求");
    }
}
