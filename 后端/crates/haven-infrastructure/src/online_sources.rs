//! 固定公开正文来源的受控 Provider。
//!
//! 本模块故意不把远端 URL 或正文数据暴露给前端。搜索卡片只携带由
//! `source_import` 解码的 opaque candidate；导入阶段只登记远端身份。
//! 显式下载和 Remote Session 才分别调用 `RemoteAcquisitionPort` 与
//! `RemoteSessionPort`，后者由 `haven-resource` 协议按需消费。

use std::fs::{self, File};
use std::io::Write;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use haven_application::services::comic::{
    ComicImageMime, ComicPageBody, PreparedComicPage, PreparedComicPageAvailability,
    PreparedComicPageSource, RemoteComicPageProvider,
};
use haven_application::services::ports::{RemoteAcquiredFile, RemoteAcquisitionPort};
use haven_application::services::ports::{
    RemoteByteRange, RemoteContentRange, RemoteSessionBody, RemoteSessionPort,
};
use haven_application::services::session::{PreparedSession, PreparedSessionSource};
use haven_application::services::source_import::{
    RemoteContentRef, SourceCatalogEntry, SourceCatalogProvider,
};
use haven_common::{AppError, ErrorKind};
use haven_domain::enums::MediaType;
use quick_xml::events::Event;
use serde_json::Value;
use tokio::io::AsyncWriteExt;
use uuid::Uuid;
use zip::write::SimpleFileOptions;
use zip::{CompressionMethod, ZipWriter};

const MANGADEX_API: &str = "https://api.mangadex.org";
const ARXIV_API: &str = "https://export.arxiv.org";
const EUROPE_PMC_API: &str = "https://www.ebi.ac.uk/europepmc/webservices/rest";
const WIKISOURCE_API: &str = "https://zh.wikisource.org/w/api.php";

const MAX_REDIRECTS: usize = 3;
const MAX_API_BYTES: usize = 8 * 1024 * 1024;
const MAX_PDF_BYTES: usize = 64 * 1024 * 1024;
const MAX_HTML_BYTES: usize = 8 * 1024 * 1024;
const MAX_MANGA_PAGES: usize = 500;
const MAX_MANGA_PAGE_BYTES: usize = 32 * 1024 * 1024;
const MAX_MANGA_TOTAL_BYTES: usize = 256 * 1024 * 1024;
const REQUEST_TIMEOUT: Duration = Duration::from_secs(20);
const CONNECT_TIMEOUT: Duration = Duration::from_secs(3);
const USER_AGENT: &str = "Haven/0.1.0 (offline content importer)";

#[derive(Debug, Clone, Copy)]
enum HostPolicy {
    MangadexApi,
    MangadexCdn,
    Arxiv,
    EuropePmc,
    Wikisource,
}

/// Fixed-host HTTP client for import operations. Redirects are disabled in the
/// reqwest client and followed manually so every target is revalidated.
#[derive(Clone)]
pub struct OnlineContentClient {
    http: reqwest::Client,
}

impl OnlineContentClient {
    pub fn new() -> Result<Self, AppError> {
        let http = reqwest::Client::builder()
            .connect_timeout(CONNECT_TIMEOUT)
            .timeout(REQUEST_TIMEOUT)
            .user_agent(USER_AGENT)
            .redirect(reqwest::redirect::Policy::none())
            .build()
            .map_err(|_| internal_error("正文来源客户端初始化失败"))?;
        Ok(Self { http })
    }

    async fn json(&self, url: &str, policy: HostPolicy) -> Result<Value, AppError> {
        let bytes = self.bytes(url, policy, MAX_API_BYTES).await?;
        serde_json::from_slice(&bytes).map_err(|_| source_unavailable("正文来源返回了无效 JSON"))
    }

    async fn text(
        &self,
        url: &str,
        policy: HostPolicy,
        max_bytes: usize,
    ) -> Result<String, AppError> {
        let bytes = self.bytes(url, policy, max_bytes).await?;
        String::from_utf8(bytes).map_err(|_| source_unavailable("正文来源返回了无法读取的文本"))
    }

    async fn bytes(
        &self,
        url: &str,
        policy: HostPolicy,
        max_bytes: usize,
    ) -> Result<Vec<u8>, AppError> {
        Ok(self.fetch(url, policy, max_bytes, None).await?.bytes)
    }

    async fn fetch(
        &self,
        url: &str,
        policy: HostPolicy,
        max_bytes: usize,
        range: Option<RemoteByteRange>,
    ) -> Result<RemoteHttpResponse, AppError> {
        let mut current = validate_url(url, policy)?;
        for _ in 0..=MAX_REDIRECTS {
            let mut request = self.http.get(current.clone()).header(
                reqwest::header::ACCEPT,
                "application/json,text/plain,application/pdf,image/*,*/*;q=0.1",
            );
            if let Some(range) = range {
                let value = match range.end {
                    Some(end) => format!("bytes={}-{}", range.start, end),
                    None => format!("bytes={}-", range.start),
                };
                request = request.header(reqwest::header::RANGE, value);
            }
            let response = request
                .send()
                .await
                .map_err(|_| source_unavailable("正文来源暂时不可达"))?;
            if response.status().is_redirection() {
                let location = response
                    .headers()
                    .get(reqwest::header::LOCATION)
                    .and_then(|value| value.to_str().ok())
                    .ok_or_else(|| source_unavailable("正文来源重定向无效"))?;
                current = current
                    .join(location)
                    .map_err(|_| security_denied("正文来源重定向地址不安全"))?;
                validate_url(current.as_str(), policy)?;
                continue;
            }
            if !response.status().is_success() {
                return Err(source_unavailable("正文来源返回异常状态"));
            }
            if response
                .content_length()
                .is_some_and(|length| length > max_bytes as u64)
            {
                return Err(source_unavailable("正文来源响应超出大小上限"));
            }
            let status = response.status();
            let content_type = response
                .headers()
                .get(reqwest::header::CONTENT_TYPE)
                .and_then(|value| value.to_str().ok())
                .map(str::to_owned);
            let accept_ranges = response
                .headers()
                .get(reqwest::header::ACCEPT_RANGES)
                .and_then(|value| value.to_str().ok())
                .is_some_and(|value| value.split(',').any(|part| part.trim() == "bytes"));
            let content_range = response
                .headers()
                .get(reqwest::header::CONTENT_RANGE)
                .and_then(|value| value.to_str().ok())
                .and_then(parse_content_range);
            if content_range.is_some_and(|range| range.total > max_bytes as u64) {
                return Err(source_unavailable("正文来源总大小超过安全上限"));
            }
            let mut body = Vec::with_capacity(64 * 1024);
            let mut response = response;
            while let Some(chunk) = response
                .chunk()
                .await
                .map_err(|_| source_unavailable("正文来源响应读取中断"))?
            {
                if body.len().saturating_add(chunk.len()) > max_bytes {
                    return Err(source_unavailable("正文来源响应超出大小上限"));
                }
                body.extend_from_slice(&chunk);
            }
            let total_size = content_range
                .map(|value| value.total)
                .or_else(|| response_content_length(status, &body))
                .unwrap_or(body.len() as u64);
            return Ok(RemoteHttpResponse {
                bytes: body,
                content_type,
                total_size,
                content_range,
                accept_ranges,
                partial: status == reqwest::StatusCode::PARTIAL_CONTENT,
            });
        }
        Err(source_unavailable("正文来源重定向次数过多"))
    }
}

#[derive(Debug, Clone)]
struct RemoteHttpResponse {
    bytes: Vec<u8>,
    content_type: Option<String>,
    total_size: u64,
    content_range: Option<RemoteContentRange>,
    accept_ranges: bool,
    partial: bool,
}

fn parse_content_range(value: &str) -> Option<RemoteContentRange> {
    let value = value.strip_prefix("bytes ")?;
    let (range, total) = value.split_once('/')?;
    let (start, end) = range.split_once('-')?;
    let start = start.parse().ok()?;
    let end = end.parse().ok()?;
    let total = total.parse().ok()?;
    (start <= end && end < total).then_some(RemoteContentRange { start, end, total })
}

fn response_content_length(_status: reqwest::StatusCode, body: &[u8]) -> Option<u64> {
    Some(body.len() as u64)
}

/// Online catalog provider. `detail` 只准备元数据和远端身份；显式下载流程
/// 通过 `acquire_offline` 获取正文。搜索仍由 `metadata_sources.rs` 负责，
/// 因此候选缓存和前端操作句柄保持不变。
pub struct OnlineCatalogProvider {
    client: Arc<OnlineContentClient>,
}

impl OnlineCatalogProvider {
    pub fn new(client: Arc<OnlineContentClient>) -> Self {
        Self { client }
    }

    async fn prepare_mangadex(&self, manga_id: &str) -> Result<SourceCatalogEntry, AppError> {
        validate_mangadex_id(manga_id)?;
        let detail_url = format!("{MANGADEX_API}/manga/{manga_id}?includes[]=cover_art");
        let detail = self
            .client
            .json(&detail_url, HostPolicy::MangadexApi)
            .await?;
        let data = detail
            .get("data")
            .ok_or_else(|| source_unavailable("MangaDex 条目不存在"))?;
        let attrs = data
            .get("attributes")
            .ok_or_else(|| source_unavailable("MangaDex 条目结构异常"))?;
        let title = localized_value(attrs.get("title"))
            .ok_or_else(|| source_unavailable("MangaDex 条目缺少标题"))?;
        let year = attrs
            .get("year")
            .and_then(Value::as_i64)
            .and_then(|v| i32::try_from(v).ok());
        let description = localized_value(attrs.get("description")).map(|v| clean_text(&v));
        let pic = manga_cover_url(manga_id, data);

        // 选择最近的可读章节。官方/已删除章节常见 pages=0 或 externalUrl；
        // 这些条目跳过后继续尝试有限数量的其它章节。
        let feed_url = format!(
            "{MANGADEX_API}/manga/{manga_id}/feed?translatedLanguage[]=zh&translatedLanguage[]=zh-hans&translatedLanguage[]=en&order[chapter]=desc&limit=20"
        );
        let feed = self.client.json(&feed_url, HostPolicy::MangadexApi).await?;
        let chapters = feed
            .get("data")
            .and_then(Value::as_array)
            .ok_or_else(|| source_unavailable("MangaDex 章节列表结构异常"))?;
        for chapter in chapters.iter().take(20) {
            let Some(chapter_id) = chapter.get("id").and_then(Value::as_str) else {
                continue;
            };
            let Some(chapter_attrs) = chapter.get("attributes") else {
                continue;
            };
            if chapter_attrs
                .get("isUnavailable")
                .and_then(Value::as_bool)
                .unwrap_or(false)
                || chapter_attrs
                    .get("pages")
                    .and_then(Value::as_u64)
                    .unwrap_or(0)
                    == 0
                || chapter_attrs
                    .get("externalUrl")
                    .and_then(Value::as_str)
                    .is_some()
            {
                continue;
            }
            if validate_uuid_like(chapter_id, "MangaDex 章节标识").is_ok() {
                let chapter_label = chapter_attrs
                    .get("chapter")
                    .and_then(Value::as_str)
                    .unwrap_or("正文");
                return Ok(SourceCatalogEntry {
                    external_id: manga_id.to_owned(),
                    title,
                    year,
                    type_name: Some("漫画".to_owned()),
                    pic,
                    episodes: Vec::new(),
                    content: description
                        .clone()
                        .or_else(|| Some(format!("MangaDex 第 {chapter_label} 章"))),
                    director: None,
                    actor: None,
                    local_file: None,
                    media_type: Some(MediaType::Comic),
                    remote: Some(RemoteContentRef {
                        source_key: "mangadex".to_owned(),
                        remote_id: format!("{manga_id}:{chapter_id}"),
                        media_type: MediaType::Comic,
                        mime_type: Some("application/vnd.comicbook+zip".to_owned()),
                    }),
                });
            }
        }
        Err(source_unavailable("MangaDex 暂时没有可阅读章节"))
    }

    async fn download_mangadex_chapter(
        &self,
        chapter_id: &str,
        destination: &std::path::Path,
    ) -> Result<RemoteAcquiredFile, AppError> {
        let (base_url, hash, names) = self.mangadex_page_manifest(chapter_id).await?;

        let mut pages: Vec<(String, Vec<u8>)> = Vec::with_capacity(names.len());
        let mut total = 0usize;
        for (index, name) in names.iter().enumerate() {
            let url = format!("{}/data/{hash}/{name}", base_url.trim_end_matches('/'));
            let bytes = self
                .client
                .bytes(&url, HostPolicy::MangadexCdn, MAX_MANGA_PAGE_BYTES)
                .await?;
            let extension = image_extension(&bytes)
                .ok_or_else(|| source_unavailable("MangaDex 返回了非图片页面"))?;
            total = total.saturating_add(bytes.len());
            if total > MAX_MANGA_TOTAL_BYTES {
                return Err(source_unavailable("MangaDex 漫画总大小超出限制"));
            }
            pages.push((format!("page-{:04}.{}", index + 1, extension), bytes));
        }

        let final_size = write_cbz_to(destination.to_path_buf(), pages).await?;
        Ok(RemoteAcquiredFile {
            size_bytes: final_size,
            mime: "application/vnd.comicbook+zip".to_owned(),
        })
    }

    /// Fetch only the MangaDex page manifest. No page bytes are downloaded;
    /// this method is used by online comic sessions before a grant is issued.
    async fn mangadex_page_manifest(
        &self,
        chapter_id: &str,
    ) -> Result<(String, String, Vec<String>), AppError> {
        validate_uuid_like(chapter_id, "MangaDex 章节标识")?;
        let at_home_url = format!("{MANGADEX_API}/at-home/server/{chapter_id}");
        let value = self
            .client
            .json(&at_home_url, HostPolicy::MangadexApi)
            .await?;
        let base_url = value
            .get("baseUrl")
            .and_then(Value::as_str)
            .ok_or_else(|| source_unavailable("MangaDex 页面服务响应缺少地址"))?;
        validate_url(base_url, HostPolicy::MangadexCdn)?;
        let chapter = value
            .get("chapter")
            .ok_or_else(|| source_unavailable("MangaDex 页面服务响应结构异常"))?;
        let hash = chapter
            .get("hash")
            .and_then(Value::as_str)
            .filter(|value| {
                !value.is_empty()
                    && value.len() <= 128
                    && value.chars().all(|c| c.is_ascii_hexdigit())
            })
            .ok_or_else(|| source_unavailable("MangaDex 页面哈希无效"))?;
        let raw_names = chapter
            .get("data")
            .and_then(Value::as_array)
            .ok_or_else(|| source_unavailable("MangaDex 页面列表结构异常"))?;
        if raw_names.is_empty() || raw_names.len() > MAX_MANGA_PAGES {
            return Err(source_unavailable("MangaDex 页面数量超出限制"));
        }
        let mut names = Vec::with_capacity(raw_names.len());
        for raw_name in raw_names {
            let name = raw_name
                .as_str()
                .ok_or_else(|| source_unavailable("MangaDex 页面名称无效"))?;
            validate_page_name(name)?;
            names.push(name.to_owned());
        }
        Ok((base_url.to_owned(), hash.to_owned(), names))
    }

    async fn prepare_arxiv(&self, arxiv_id: &str) -> Result<SourceCatalogEntry, AppError> {
        let arxiv_id = validate_arxiv_id(arxiv_id)?;
        let metadata_url = format!("{ARXIV_API}/api/query?id_list={}", encode_query(&arxiv_id));
        let xml = self
            .client
            .text(&metadata_url, HostPolicy::Arxiv, MAX_API_BYTES)
            .await?;
        let (title, summary, year) = parse_arxiv_metadata(&xml)
            .ok_or_else(|| source_unavailable("arXiv 条目元数据不可用"))?;
        Ok(SourceCatalogEntry {
            external_id: arxiv_id.clone(),
            title,
            year,
            type_name: Some("文章".to_owned()),
            pic: None,
            episodes: Vec::new(),
            content: Some(summary),
            director: None,
            actor: None,
            local_file: None,
            media_type: Some(MediaType::Article),
            remote: Some(RemoteContentRef {
                source_key: "arxiv".to_owned(),
                remote_id: arxiv_id,
                media_type: MediaType::Article,
                mime_type: Some("application/pdf".to_owned()),
            }),
        })
    }

    async fn prepare_europe_pmc(&self, pmcid: &str) -> Result<SourceCatalogEntry, AppError> {
        if !is_pmcid(pmcid) {
            return Err(invalid_argument("Europe PMC 标识非法"));
        }
        Ok(SourceCatalogEntry {
            external_id: pmcid.to_owned(),
            title: pmcid.to_owned(),
            year: None,
            type_name: Some("报刊文章".to_owned()),
            pic: None,
            episodes: Vec::new(),
            content: Some("开放获取全文，可在线阅读或保存为文章快照".to_owned()),
            director: None,
            actor: None,
            local_file: None,
            media_type: Some(MediaType::Article),
            remote: Some(RemoteContentRef {
                source_key: "europepmc".to_owned(),
                remote_id: pmcid.to_owned(),
                media_type: MediaType::Article,
                mime_type: Some("text/html; charset=utf-8".to_owned()),
            }),
        })
    }

    async fn prepare_wikisource(&self, title: &str) -> Result<SourceCatalogEntry, AppError> {
        validate_wikisource_title(title)?;
        Ok(SourceCatalogEntry {
            external_id: title.to_owned(),
            title: title.to_owned(),
            year: None,
            type_name: Some("公版文章".to_owned()),
            pic: None,
            episodes: Vec::new(),
            content: Some("Wikisource 公版正文，可在线阅读或保存为文章快照".to_owned()),
            director: None,
            actor: None,
            local_file: None,
            media_type: Some(MediaType::Article),
            remote: Some(RemoteContentRef {
                source_key: "wikisource".to_owned(),
                remote_id: title.to_owned(),
                media_type: MediaType::Article,
                mime_type: Some("text/html; charset=utf-8".to_owned()),
            }),
        })
    }

    async fn acquire_arxiv_to(
        &self,
        arxiv_id: &str,
        destination: &std::path::Path,
    ) -> Result<RemoteAcquiredFile, AppError> {
        let arxiv_id = validate_arxiv_id(arxiv_id)?;
        let pdf_url = format!("{ARXIV_API}/pdf/{arxiv_id}.pdf");
        let pdf = self
            .client
            .bytes(&pdf_url, HostPolicy::Arxiv, MAX_PDF_BYTES)
            .await?;
        if !pdf.starts_with(b"%PDF-") {
            return Err(source_unavailable("arXiv 返回的文件不是 PDF"));
        }
        let size = write_bytes_to(destination.to_path_buf(), pdf).await?;
        Ok(RemoteAcquiredFile {
            size_bytes: size,
            mime: "application/pdf".to_owned(),
        })
    }

    async fn acquire_europe_pmc_to(
        &self,
        pmcid: &str,
        destination: &std::path::Path,
    ) -> Result<RemoteAcquiredFile, AppError> {
        if !is_pmcid(pmcid) {
            return Err(invalid_argument("Europe PMC 标识非法"));
        }
        let url = format!("{EUROPE_PMC_API}/{pmcid}/fullTextXML");
        let xml = self
            .client
            .text(&url, HostPolicy::EuropePmc, MAX_HTML_BYTES)
            .await?;
        let document = parse_europe_pmc(&xml);
        if document.title.is_empty() && document.paragraphs.is_empty() {
            return Err(source_unavailable("Europe PMC 全文为空"));
        }
        let title = if document.title.is_empty() {
            pmcid
        } else {
            &document.title
        };
        let html = safe_article_html(title, &document.paragraphs);
        if html.len() > MAX_HTML_BYTES {
            return Err(source_unavailable("Europe PMC 正文超出大小上限"));
        }
        let size = write_bytes_to(destination.to_path_buf(), html.into_bytes()).await?;
        Ok(RemoteAcquiredFile {
            size_bytes: size,
            mime: "text/html; charset=utf-8".to_owned(),
        })
    }

    async fn acquire_wikisource_to(
        &self,
        title: &str,
        destination: &std::path::Path,
    ) -> Result<RemoteAcquiredFile, AppError> {
        validate_wikisource_title(title)?;
        let url = format!(
            "{WIKISOURCE_API}?action=parse&page={}&prop=text&format=json&formatversion=2",
            encode_query(title)
        );
        let value = self.client.json(&url, HostPolicy::Wikisource).await?;
        let parsed = value
            .get("parse")
            .ok_or_else(|| source_unavailable("Wikisource 页面不存在"))?;
        let page_title = parsed
            .get("title")
            .and_then(Value::as_str)
            .unwrap_or(title)
            .trim();
        let raw_html = parsed
            .get("text")
            .and_then(Value::as_str)
            .or_else(|| {
                parsed
                    .get("text")
                    .and_then(|v| v.get("*"))
                    .and_then(Value::as_str)
            })
            .ok_or_else(|| source_unavailable("Wikisource 页面正文为空"))?;
        let paragraphs = html_to_plain_paragraphs(raw_html);
        if paragraphs.is_empty() {
            return Err(source_unavailable("Wikisource 页面正文为空"));
        }
        let html = safe_article_html(page_title, &paragraphs);
        if html.len() > MAX_HTML_BYTES {
            return Err(source_unavailable("Wikisource 正文超出大小上限"));
        }
        let size = write_bytes_to(destination.to_path_buf(), html.into_bytes()).await?;
        Ok(RemoteAcquiredFile {
            size_bytes: size,
            mime: "text/html; charset=utf-8".to_owned(),
        })
    }
}

#[async_trait]
impl SourceCatalogProvider for OnlineCatalogProvider {
    async fn detail(
        &self,
        source_id: &str,
        _endpoint: &str,
        external_id: &str,
    ) -> Result<SourceCatalogEntry, AppError> {
        match source_id {
            "mangadex" => self.prepare_mangadex(external_id).await,
            "arxiv" => self.prepare_arxiv(external_id).await,
            "europepmc" => self.prepare_europe_pmc(external_id).await,
            "wikisource" => self.prepare_wikisource(external_id).await,
            _ => Err(invalid_argument("未知在线正文来源")),
        }
    }
}

#[async_trait]
impl RemoteAcquisitionPort for OnlineCatalogProvider {
    async fn acquire(
        &self,
        source_key: &str,
        remote_id: &str,
        destination: &std::path::Path,
    ) -> Result<RemoteAcquiredFile, AppError> {
        if destination.as_os_str().is_empty() {
            return Err(storage_error("正文临时文件路径无效"));
        }
        match source_key {
            "mangadex" => {
                let (manga_id, chapter_id) = remote_id
                    .split_once(':')
                    .ok_or_else(|| invalid_argument("MangaDex 远端身份无效"))?;
                validate_mangadex_id(manga_id)?;
                self.download_mangadex_chapter(chapter_id, destination)
                    .await
            }
            "arxiv" => self.acquire_arxiv_to(remote_id, destination).await,
            "europepmc" => self.acquire_europe_pmc_to(remote_id, destination).await,
            "wikisource" => self.acquire_wikisource_to(remote_id, destination).await,
            _ => Err(invalid_argument("未知远端正文来源")),
        }
    }
}

#[async_trait]
impl RemoteComicPageProvider for OnlineCatalogProvider {
    async fn inspect(&self, session: &PreparedSession) -> Result<Vec<PreparedComicPage>, AppError> {
        let PreparedSessionSource::Remote {
            source_key,
            remote_id,
            ..
        } = &session.source
        else {
            return Err(invalid_argument("漫画会话不是远端来源"));
        };
        if source_key != "mangadex" {
            return Err(invalid_argument("该来源不支持在线漫画"));
        }
        let (_, chapter_id) = remote_id
            .split_once(':')
            .ok_or_else(|| invalid_argument("MangaDex 远端章节身份无效"))?;
        let (_, _, names) = self.mangadex_page_manifest(chapter_id).await?;
        Ok(names
            .into_iter()
            .map(|page_name| PreparedComicPage {
                availability: PreparedComicPageAvailability::Ready,
                source: PreparedComicPageSource::RemotePage { page_name },
            })
            .collect())
    }

    async fn read_page(
        &self,
        session: &PreparedSession,
        page: &PreparedComicPage,
    ) -> Result<ComicPageBody, AppError> {
        let PreparedSessionSource::Remote {
            source_key,
            remote_id,
            ..
        } = &session.source
        else {
            return Err(invalid_argument("漫画会话不是远端来源"));
        };
        if source_key != "mangadex" {
            return Err(invalid_argument("该来源不支持在线漫画"));
        }
        let PreparedComicPageSource::RemotePage { page_name } = &page.source else {
            return Err(invalid_argument("漫画页面身份无效"));
        };
        validate_page_name(page_name)?;
        let (_, chapter_id) = remote_id
            .split_once(':')
            .ok_or_else(|| invalid_argument("MangaDex 远端章节身份无效"))?;
        let (base_url, hash, names) = self.mangadex_page_manifest(chapter_id).await?;
        if !names.iter().any(|name| name == page_name) {
            return Err(source_unavailable("MangaDex 页面已不存在"));
        }
        let url = format!("{}/data/{hash}/{page_name}", base_url.trim_end_matches('/'));
        let bytes = self
            .client
            .bytes(&url, HostPolicy::MangadexCdn, MAX_MANGA_PAGE_BYTES)
            .await?;
        let mime_type =
            image_mime(&bytes).ok_or_else(|| source_unavailable("MangaDex 返回了非图片页面"))?;
        Ok(ComicPageBody { mime_type, bytes })
    }
}

#[async_trait]
impl RemoteSessionPort for OnlineCatalogProvider {
    async fn read(
        &self,
        source_key: &str,
        remote_id: &str,
        range: Option<RemoteByteRange>,
    ) -> Result<RemoteSessionBody, AppError> {
        match source_key {
            "arxiv" => self.read_arxiv_session(remote_id, range).await,
            "europepmc" => self.read_europe_pmc_session(remote_id, range).await,
            "wikisource" => self.read_wikisource_session(remote_id, range).await,
            _ => Err(invalid_argument("该来源不支持在线正文会话")),
        }
    }
}

impl OnlineCatalogProvider {
    async fn read_arxiv_session(
        &self,
        arxiv_id: &str,
        range: Option<RemoteByteRange>,
    ) -> Result<RemoteSessionBody, AppError> {
        let arxiv_id = validate_arxiv_id(arxiv_id)?;
        let pdf_url = format!("{ARXIV_API}/pdf/{arxiv_id}.pdf");
        let response = self
            .client
            .fetch(&pdf_url, HostPolicy::Arxiv, MAX_PDF_BYTES, range)
            .await?;
        if let Some(requested) = range {
            let Some(actual) = response.content_range else {
                // A 206 response without Content-Range is not a safe range
                // response: the reader cannot know which bytes it received.
                return Err(source_range_unsupported());
            };
            validate_range_response(&response, requested, actual)?;
        } else if response.partial {
            // Initial PDF probes expect the complete object. Do not expose a
            // partial body as a successful 200 response.
            return Err(source_range_unsupported());
        }
        if range.is_none()
            && response.content_type.as_deref().is_some_and(|value| {
                let mime = value.split(';').next().unwrap_or("").trim();
                !matches!(mime, "application/pdf" | "application/octet-stream")
            })
        {
            return Err(source_unavailable("arXiv 返回的类型不是 PDF"));
        }
        if !response.bytes.starts_with(b"%PDF-") && range.is_none() {
            return Err(source_unavailable("arXiv 返回的文件不是 PDF"));
        }
        Ok(RemoteSessionBody {
            mime_type: "application/pdf".to_owned(),
            bytes: response.bytes,
            total_size: response.total_size,
            content_range: response.content_range,
            accept_ranges: range.is_some() || response.accept_ranges,
        })
    }

    async fn read_europe_pmc_session(
        &self,
        pmcid: &str,
        range: Option<RemoteByteRange>,
    ) -> Result<RemoteSessionBody, AppError> {
        if !is_pmcid(pmcid) {
            return Err(invalid_argument("Europe PMC 标识非法"));
        }
        let url = format!("{EUROPE_PMC_API}/{pmcid}/fullTextXML");
        let xml = self
            .client
            .text(&url, HostPolicy::EuropePmc, MAX_HTML_BYTES)
            .await?;
        let document = parse_europe_pmc(&xml);
        if document.title.is_empty() && document.paragraphs.is_empty() {
            return Err(source_unavailable("Europe PMC 全文为空"));
        }
        let title = if document.title.is_empty() {
            pmcid
        } else {
            &document.title
        };
        let html = safe_article_html(title, &document.paragraphs).into_bytes();
        bounded_session_body(html, range, "text/html; charset=utf-8")
    }

    async fn read_wikisource_session(
        &self,
        title: &str,
        range: Option<RemoteByteRange>,
    ) -> Result<RemoteSessionBody, AppError> {
        validate_wikisource_title(title)?;
        let url = format!(
            "{WIKISOURCE_API}?action=parse&page={}&prop=text&format=json&formatversion=2",
            encode_query(title)
        );
        let value = self.client.json(&url, HostPolicy::Wikisource).await?;
        let parsed = value
            .get("parse")
            .ok_or_else(|| source_unavailable("Wikisource 页面不存在"))?;
        let page_title = parsed
            .get("title")
            .and_then(Value::as_str)
            .unwrap_or(title)
            .trim();
        let raw_html = parsed
            .get("text")
            .and_then(Value::as_str)
            .or_else(|| {
                parsed
                    .get("text")
                    .and_then(|v| v.get("*"))
                    .and_then(Value::as_str)
            })
            .ok_or_else(|| source_unavailable("Wikisource 页面正文为空"))?;
        let paragraphs = html_to_plain_paragraphs(raw_html);
        if paragraphs.is_empty() {
            return Err(source_unavailable("Wikisource 页面正文为空"));
        }
        let html = safe_article_html(page_title, &paragraphs).into_bytes();
        bounded_session_body(html, range, "text/html; charset=utf-8")
    }
}

fn validate_range_response(
    response: &RemoteHttpResponse,
    requested: RemoteByteRange,
    actual: RemoteContentRange,
) -> Result<(), AppError> {
    if !response.partial
        || actual.start != requested.start
        || actual.end < actual.start
        || requested.end.is_some_and(|end| actual.end > end)
        || actual.end >= actual.total
    {
        return Err(source_unavailable("远端范围响应无效"));
    }
    let expected_len = actual
        .end
        .checked_sub(actual.start)
        .and_then(|length| length.checked_add(1))
        .ok_or_else(|| source_unavailable("远端范围响应无效"))?;
    if expected_len != response.bytes.len() as u64 || expected_len > MAX_PDF_BYTES as u64 {
        return Err(source_unavailable("远端范围响应长度无效"));
    }
    Ok(())
}

/// Build a bounded Article response for both the initial probe and later
/// range reads. The upstream XML/HTML is still fetched and sanitised inside
/// Infrastructure; only the requested slice crosses the resource protocol.
fn bounded_session_body(
    bytes: Vec<u8>,
    range: Option<RemoteByteRange>,
    mime_type: &str,
) -> Result<RemoteSessionBody, AppError> {
    let total = u64::try_from(bytes.len()).map_err(|_| source_unavailable("正文大小无效"))?;
    if total == 0 {
        return Err(source_unavailable("正文为空"));
    }
    let Some(range) = range else {
        return Ok(RemoteSessionBody {
            total_size: total,
            bytes,
            mime_type: mime_type.to_owned(),
            content_range: None,
            accept_ranges: true,
        });
    };
    if range.start >= total {
        return Err(remote_range_invalid());
    }
    let requested_end = range.end.unwrap_or(total - 1);
    if requested_end < range.start {
        return Err(remote_range_invalid());
    }
    let end = requested_end.min(total - 1);
    let start = usize::try_from(range.start).map_err(|_| remote_range_invalid())?;
    let end_exclusive = usize::try_from(end + 1).map_err(|_| remote_range_invalid())?;
    Ok(RemoteSessionBody {
        total_size: total,
        bytes: bytes[start..end_exclusive].to_vec(),
        mime_type: mime_type.to_owned(),
        content_range: Some(RemoteContentRange {
            start: range.start,
            end,
            total,
        }),
        accept_ranges: true,
    })
}

fn validate_url(raw: &str, policy: HostPolicy) -> Result<reqwest::Url, AppError> {
    let url = reqwest::Url::parse(raw).map_err(|_| security_denied("正文来源地址无效"))?;
    if url.scheme() != "https"
        || url.username() != ""
        || url.password().is_some()
        || url.fragment().is_some()
        || url.port().is_some()
        || has_explicit_authority_port(raw)
    {
        return Err(security_denied("正文来源地址不安全"));
    }
    let host = url
        .host_str()
        .ok_or_else(|| security_denied("正文来源地址缺少主机"))?
        .to_ascii_lowercase();
    let allowed = match policy {
        HostPolicy::MangadexApi => host == "api.mangadex.org",
        HostPolicy::MangadexCdn => {
            // At-Home currently returns both command hosts under
            // `mangadex.network` and the long-standing `uploads.mangadex.org`
            // host.  Keep both exact suffix policies narrow; never accept the
            // bare parent domain or an arbitrary Mangadex subdomain.
            (host.ends_with(".mangadex.network") && host != "mangadex.network")
                || host == "uploads.mangadex.org"
        }
        HostPolicy::Arxiv => host == "export.arxiv.org" || host == "arxiv.org",
        HostPolicy::EuropePmc => host == "www.ebi.ac.uk",
        HostPolicy::Wikisource => host == "zh.wikisource.org",
    };
    if !allowed {
        return Err(security_denied("正文来源主机不在允许范围"));
    }
    Ok(url)
}

/// `url::Url` normalizes an explicit default port such as `:443` away from
/// `Url::port()`.  Keep the fixed-host policy strict by checking the raw
/// authority as well.  User-info is rejected by `validate_url` before this
/// helper affects the allowlist decision.
fn has_explicit_authority_port(raw: &str) -> bool {
    let Some((_, after_scheme)) = raw.split_once("://") else {
        return false;
    };
    let authority = after_scheme
        .split(['/', '?', '#'])
        .next()
        .unwrap_or_default();
    let host = authority
        .rsplit_once('@')
        .map_or(authority, |(_, host)| host);
    if host.starts_with('[') {
        return host.find(']').is_some_and(|end| {
            host.get(end + 1..)
                .is_some_and(|tail| tail.starts_with(':'))
        });
    }
    host.contains(':')
}

fn validate_uuid_like(value: &str, label: &'static str) -> Result<(), AppError> {
    if Uuid::parse_str(value)
        .ok()
        .is_none_or(|parsed| parsed.to_string() != value)
    {
        return Err(invalid_argument(label));
    }
    Ok(())
}

fn validate_mangadex_id(value: &str) -> Result<(), AppError> {
    validate_uuid_like(value, "MangaDex 漫画标识")
}

fn validate_arxiv_id(value: &str) -> Result<String, AppError> {
    let value = value.trim().trim_end_matches(".pdf");
    if value.is_empty()
        || value.len() > 128
        || value
            .chars()
            .any(|ch| !(ch.is_ascii_alphanumeric() || matches!(ch, '.' | '-' | '_' | '/')))
    {
        return Err(invalid_argument("arXiv 标识非法"));
    }
    Ok(value.to_owned())
}

fn is_pmcid(value: &str) -> bool {
    let Some(rest) = value.strip_prefix("PMC") else {
        return false;
    };
    !rest.is_empty() && rest.len() <= 12 && rest.bytes().all(|byte| byte.is_ascii_digit())
}

fn validate_wikisource_title(value: &str) -> Result<(), AppError> {
    let value = value.trim();
    if value.is_empty()
        || value.len() > 300
        || value.chars().any(|ch| ch == '\0' || ch.is_control())
    {
        return Err(invalid_argument("Wikisource 页面标题非法"));
    }
    Ok(())
}

fn validate_page_name(value: &str) -> Result<(), AppError> {
    if value.is_empty()
        || value.len() > 512
        || value.contains(['/', '\\', ':'])
        || value == "."
        || value == ".."
        || value.chars().any(|ch| ch == '\0' || ch.is_control())
    {
        return Err(security_denied("漫画页面名称不安全"));
    }
    Ok(())
}

fn localized_value(value: Option<&Value>) -> Option<String> {
    let value = value?;
    if let Some(text) = value.as_str() {
        return (!text.trim().is_empty()).then(|| text.trim().to_owned());
    }
    let object = value.as_object()?;
    ["zh", "zh-hans", "zh-hk", "en", "ja", "ja-ro", "*", "und"]
        .iter()
        .find_map(|key| object.get(*key).and_then(Value::as_str))
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_owned)
        .or_else(|| {
            object
                .values()
                .find_map(Value::as_str)
                .map(str::trim)
                .filter(|v| !v.is_empty())
                .map(str::to_owned)
        })
}

fn manga_cover_url(manga_id: &str, data: &Value) -> Option<String> {
    let file_name = data
        .get("relationships")
        .and_then(Value::as_array)?
        .iter()
        .find(|relationship| relationship.get("type").and_then(Value::as_str) == Some("cover_art"))
        .and_then(|relationship| relationship.get("attributes"))
        .and_then(|attributes| attributes.get("fileName"))
        .and_then(Value::as_str)?;
    validate_page_name(file_name).ok()?;
    Some(format!(
        "https://uploads.mangadex.org/covers/{manga_id}/{file_name}.256.jpg"
    ))
}

fn image_extension(bytes: &[u8]) -> Option<&'static str> {
    if bytes.starts_with(&[0xff, 0xd8, 0xff]) {
        Some("jpg")
    } else if bytes.starts_with(b"\x89PNG\r\n\x1a\n") {
        Some("png")
    } else if bytes.len() >= 12 && &bytes[..4] == b"RIFF" && &bytes[8..12] == b"WEBP" {
        Some("webp")
    } else {
        None
    }
}

fn image_mime(bytes: &[u8]) -> Option<ComicImageMime> {
    match image_extension(bytes)? {
        "jpg" => Some(ComicImageMime::Jpeg),
        "png" => Some(ComicImageMime::Png),
        "webp" => Some(ComicImageMime::Webp),
        _ => None,
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

async fn write_bytes_to(destination: PathBuf, bytes: Vec<u8>) -> Result<u64, AppError> {
    let directory = destination
        .parent()
        .filter(|path| !path.as_os_str().is_empty())
        .ok_or_else(|| storage_error("正文临时文件路径无效"))?;
    tokio::fs::create_dir_all(directory)
        .await
        .map_err(|error| storage_io_error(&error, "正文目录创建失败"))?;
    let temp = destination.with_extension("provider-part");
    let _ = tokio::fs::remove_file(&temp).await;
    let _ = tokio::fs::remove_file(&destination).await;
    let mut cleanup = TempPathGuard::new(temp.clone());
    let length = bytes.len() as u64;
    let result = async {
        let mut file = tokio::fs::File::create(&temp)
            .await
            .map_err(|error| storage_io_error(&error, "正文临时文件创建失败"))?;
        file.write_all(&bytes)
            .await
            .map_err(|error| storage_io_error(&error, "正文写入失败"))?;
        file.sync_all()
            .await
            .map_err(|error| storage_io_error(&error, "正文写入失败"))?;
        drop(file);
        tokio::fs::rename(&temp, &destination)
            .await
            .map_err(|error| storage_io_error(&error, "正文保存失败"))?;
        cleanup.committed = true;
        Ok(length)
    }
    .await;
    if result.is_err() {
        let _ = tokio::fs::remove_file(&temp).await;
        let _ = tokio::fs::remove_file(&destination).await;
    }
    result
}

async fn write_cbz_to(
    destination: PathBuf,
    pages: Vec<(String, Vec<u8>)>,
) -> Result<u64, AppError> {
    tokio::task::spawn_blocking(move || {
        let directory = destination
            .parent()
            .ok_or_else(|| storage_error("漫画临时文件路径无效"))?;
        fs::create_dir_all(directory)
            .map_err(|error| storage_io_error(&error, "漫画目录创建失败"))?;
        let temp = destination.with_extension("provider-part");
        let _ = fs::remove_file(&temp);
        let _ = fs::remove_file(&destination);
        let mut cleanup = TempPathGuard::new(temp.clone());
        let result = (|| {
            let file = File::create(&temp)
                .map_err(|error| storage_io_error(&error, "漫画临时文件创建失败"))?;
            let mut writer = ZipWriter::new(file);
            let options =
                SimpleFileOptions::default().compression_method(CompressionMethod::Stored);
            for (name, bytes) in pages {
                writer
                    .start_file(name, options)
                    .map_err(|_| storage_error("漫画归档写入失败"))?;
                writer
                    .write_all(&bytes)
                    .map_err(|error| storage_io_error(&error, "漫画归档写入失败"))?;
            }
            let file = writer
                .finish()
                .map_err(|_| storage_error("漫画归档写入失败"))?;
            file.sync_all()
                .map_err(|error| storage_io_error(&error, "漫画归档写入失败"))?;
            fs::rename(&temp, &destination)
                .map_err(|error| storage_io_error(&error, "漫画归档保存失败"))?;
            cleanup.committed = true;
            Ok(fs::metadata(&destination)
                .map_err(|error| storage_io_error(&error, "漫画归档状态读取失败"))?
                .len())
        })();
        if result.is_err() {
            let _ = fs::remove_file(&temp);
            let _ = fs::remove_file(&destination);
        }
        result
    })
    .await
    .map_err(|_| storage_error("漫画写入任务失败"))?
}

/// Provider 写入可能被取消或任务 future 被丢弃。Drop 兜底清理仅包含
/// provider 临时文件，不接触用户已有资源；正常原子 rename 后标记已提交。
struct TempPathGuard {
    path: PathBuf,
    committed: bool,
}

impl TempPathGuard {
    fn new(path: PathBuf) -> Self {
        Self {
            path,
            committed: false,
        }
    }
}

impl Drop for TempPathGuard {
    fn drop(&mut self) {
        if !self.committed {
            let _ = fs::remove_file(&self.path);
        }
    }
}

#[derive(Default)]
struct EuropeDocument {
    title: String,
    paragraphs: Vec<String>,
}

fn parse_europe_pmc(xml: &str) -> EuropeDocument {
    let mut reader = quick_xml::Reader::from_str(xml);
    let mut document = EuropeDocument::default();
    let mut current: Option<String> = None;
    let mut buffer = String::new();
    loop {
        match reader.read_event() {
            Ok(Event::Start(event)) => {
                let name = local_name(event.name().as_ref());
                if name == "article-title" {
                    current = Some("title".to_owned());
                    buffer.clear();
                } else if name == "p" {
                    current = Some("paragraph".to_owned());
                    buffer.clear();
                }
            }
            Ok(Event::Text(text)) => {
                if current.is_some() {
                    let value = text.unescape().map(|v| v.into_owned()).unwrap_or_default();
                    buffer.push_str(&value);
                    buffer.push(' ');
                }
            }
            Ok(Event::CData(text)) => {
                if current.is_some() {
                    buffer.push_str(&String::from_utf8_lossy(text.as_ref()));
                    buffer.push(' ');
                }
            }
            Ok(Event::End(event)) => {
                let name = local_name(event.name().as_ref());
                if (name == "article-title" && current.as_deref() == Some("title"))
                    || (name == "p" && current.as_deref() == Some("paragraph"))
                {
                    let value = clean_text(&buffer);
                    if !value.is_empty() {
                        if name == "article-title" {
                            document.title = value;
                        } else {
                            document.paragraphs.push(value);
                        }
                    }
                    current = None;
                    buffer.clear();
                }
            }
            Ok(Event::Eof) | Err(_) => break,
            _ => {}
        }
    }
    document
}

fn parse_arxiv_metadata(xml: &str) -> Option<(String, String, Option<i32>)> {
    let mut reader = quick_xml::Reader::from_str(xml);
    let mut in_entry = false;
    let mut field: Option<&'static str> = None;
    let mut title = String::new();
    let mut summary = String::new();
    let mut year = None;
    loop {
        match reader.read_event() {
            Ok(Event::Start(event)) => {
                let name = local_name(event.name().as_ref());
                if name == "entry" {
                    in_entry = true;
                } else if in_entry {
                    field = match name.as_str() {
                        "title" => Some("title"),
                        "summary" => Some("summary"),
                        "published" => Some("published"),
                        _ => None,
                    };
                }
            }
            Ok(Event::Text(text)) if in_entry => {
                let value = text.unescape().map(|v| v.into_owned()).unwrap_or_default();
                match field {
                    Some("title") => title.push_str(value.trim()),
                    Some("summary") => summary.push_str(value.trim()),
                    Some("published") => year = value.get(0..4).and_then(|v| v.parse().ok()),
                    _ => {}
                }
            }
            Ok(Event::End(event)) => {
                let name = local_name(event.name().as_ref());
                if name == "entry" && in_entry {
                    break;
                }
                field = None;
            }
            Ok(Event::Eof) | Err(_) => break,
            _ => {}
        }
    }
    let title = clean_text(&title);
    if title.is_empty() {
        None
    } else {
        Some((title, clean_text(&summary), year))
    }
}

fn local_name(raw: &[u8]) -> String {
    String::from_utf8_lossy(raw)
        .rsplit(':')
        .next()
        .unwrap_or("")
        .to_owned()
}

fn clean_text(value: &str) -> String {
    value.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn html_to_plain_paragraphs(input: &str) -> Vec<String> {
    let mut paragraphs = Vec::new();
    let mut text = String::new();
    let mut in_tag = false;
    let mut tag = String::new();
    let mut skip_depth = 0usize;
    let chars = input.chars();
    for ch in chars {
        if ch == '<' {
            in_tag = true;
            tag.clear();
            continue;
        }
        if in_tag {
            if ch == '>' {
                in_tag = false;
                let raw = tag.trim();
                let closing = raw.starts_with('/');
                let name = raw
                    .trim_start_matches('/')
                    .split_whitespace()
                    .next()
                    .unwrap_or("")
                    .trim_end_matches('/');
                let lower = name.to_ascii_lowercase();
                if [
                    "script", "style", "iframe", "form", "svg", "img", "video", "audio", "object",
                    "embed", "link", "meta",
                ]
                .contains(&lower.as_str())
                {
                    if closing {
                        skip_depth = skip_depth.saturating_sub(1);
                    } else if !raw.ends_with('/')
                        && !["img", "meta", "link", "embed"].contains(&lower.as_str())
                    {
                        skip_depth = skip_depth.saturating_add(1);
                    }
                } else if skip_depth == 0
                    && [
                        "p", "div", "br", "li", "h1", "h2", "h3", "h4", "h5", "h6", "tr",
                    ]
                    .contains(&lower.as_str())
                {
                    push_paragraph(&mut paragraphs, &mut text);
                }
            } else if tag.len() < 256 {
                tag.push(ch);
            }
            continue;
        }
        if skip_depth == 0 {
            text.push(ch);
        }
    }
    push_paragraph(&mut paragraphs, &mut text);
    paragraphs
}

fn push_paragraph(paragraphs: &mut Vec<String>, text: &mut String) {
    let value = decode_basic_entities(&clean_text(text));
    if !value.is_empty() {
        paragraphs.push(value);
    }
    text.clear();
}

fn decode_basic_entities(value: &str) -> String {
    value
        .replace("&nbsp;", " ")
        .replace("&amp;", "&")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&#39;", "'")
}

fn safe_article_html(title: &str, paragraphs: &[String]) -> String {
    let mut html =
        String::from("<!doctype html><html><head><meta charset=\"utf-8\"></head><body><article>");
    html.push_str("<h1>");
    html.push_str(&escape_html(title));
    html.push_str("</h1>");
    for paragraph in paragraphs.iter().take(20_000) {
        html.push_str("<p>");
        html.push_str(&escape_html(paragraph));
        html.push_str("</p>");
    }
    html.push_str("</article></body></html>");
    html
}

fn escape_html(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&#39;")
}

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

fn source_unavailable(message: &'static str) -> AppError {
    AppError::new("SOURCE_UNAVAILABLE", ErrorKind::Network, message, true)
}

fn source_range_unsupported() -> AppError {
    AppError::new(
        "SOURCE_RANGE_UNSUPPORTED",
        ErrorKind::Unsupported,
        "该远端正文不支持按范围在线读取，请先下载到本地",
        false,
    )
}

fn remote_range_invalid() -> AppError {
    AppError::new(
        "RANGE_INVALID",
        ErrorKind::Validation,
        "远端正文的范围请求无效",
        false,
    )
}

fn storage_error(message: &'static str) -> AppError {
    AppError::new("STORAGE_ERROR", ErrorKind::Storage, message, true)
}

fn storage_io_error(error: &std::io::Error, message: &'static str) -> AppError {
    match error.kind() {
        std::io::ErrorKind::StorageFull => AppError::new(
            "DOWNLOAD_DISK_SPACE_LOW",
            ErrorKind::Storage,
            "磁盘空间不足",
            false,
        )
        .with_source(safe_io_source(error)),
        std::io::ErrorKind::PermissionDenied => AppError::new(
            "DOWNLOAD_PERMISSION_DENIED",
            ErrorKind::Io,
            "没有权限写入下载目录",
            false,
        )
        .with_source(safe_io_source(error)),
        _ => storage_error(message).with_source(safe_io_source(error)),
    }
}

fn safe_io_source(error: &std::io::Error) -> std::io::Error {
    // Preserve only the OS error kind. Path-bearing platform messages must not
    // become part of an AppError source that a diagnostic logger might render.
    std::io::Error::from(error.kind())
}

fn internal_error(message: &'static str) -> AppError {
    AppError::new("INTERNAL_ERROR", ErrorKind::Internal, message, false)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fixed_host_policy_rejects_untrusted_redirects() {
        assert!(validate_url("https://api.mangadex.org/manga/x", HostPolicy::MangadexApi).is_ok());
        assert!(validate_url("https://evil.example/manga/x", HostPolicy::MangadexApi).is_err());
        assert!(
            validate_url(
                "https://cmd.mangadex.network/data/x",
                HostPolicy::MangadexCdn
            )
            .is_ok()
        );
        assert!(
            validate_url(
                "https://uploads.mangadex.org/data/hash/page.jpg",
                HostPolicy::MangadexCdn
            )
            .is_ok()
        );
        assert!(validate_url("https://mangadex.network/data/x", HostPolicy::MangadexCdn).is_err());
        assert!(validate_url("http://api.mangadex.org/manga/x", HostPolicy::MangadexApi).is_err());
        assert!(
            validate_url(
                "https://api.mangadex.org:443/manga/x",
                HostPolicy::MangadexApi
            )
            .is_err()
        );
        assert!(
            validate_url(
                "https://api.mangadex.org/manga/x#fragment",
                HostPolicy::MangadexApi
            )
            .is_err()
        );
    }

    #[test]
    fn validates_external_identifiers() {
        assert_eq!(
            validate_arxiv_id("hep-th/9901001.pdf").unwrap(),
            "hep-th/9901001"
        );
        assert!(validate_arxiv_id("https://evil.example/a").is_err());
        assert!(is_pmcid("PMC123456"));
        assert!(!is_pmcid("PMCX"));
        assert!(validate_mangadex_id("aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa").is_ok());
        assert!(validate_mangadex_id("AAAAAAAA-AAAA-4AAA-8AAA-AAAAAAAAAAAA").is_err());
        assert!(validate_mangadex_id("aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaa").is_err());
        assert!(validate_mangadex_id("aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa\n").is_err());
        assert!(validate_page_name("page-0001.png").is_ok());
        assert!(validate_page_name("../page.png").is_err());
    }

    #[test]
    fn sanitizes_wikisource_html_to_escaped_article() {
        let paragraphs = html_to_plain_paragraphs(
            "<p>Hello <b>world</b></p><script>alert(1)</script><img src='x'><p>&amp; safe</p>",
        );
        assert_eq!(paragraphs, vec!["Hello world", "& safe"]);
        let html = safe_article_html("<unsafe>", &paragraphs);
        assert!(html.contains("&lt;unsafe&gt;"));
        assert!(!html.contains("<script"));
        assert!(!html.contains("<img"));
    }

    #[test]
    fn parses_europe_pmc_title_and_paragraphs() {
        let document = parse_europe_pmc(
            "<article><front><article-title>A title</article-title></front><body><p>First <italic>paragraph</italic>.</p><p>Second.</p></body></article>",
        );
        assert_eq!(document.title, "A title");
        assert_eq!(document.paragraphs, vec!["First paragraph .", "Second."]);
    }

    #[test]
    fn image_magic_is_strict() {
        assert_eq!(image_extension(b"\xff\xd8\xffdata"), Some("jpg"));
        assert_eq!(image_extension(b"\x89PNG\r\n\x1a\ndata"), Some("png"));
        assert_eq!(image_extension(b"<html>"), None);
    }

    #[test]
    fn bounded_article_ranges_are_clamped_and_keep_total_length() {
        let full = bounded_session_body(
            b"0123456789".to_vec(),
            Some(RemoteByteRange {
                start: 2,
                end: Some(99),
            }),
            "text/html; charset=utf-8",
        )
        .unwrap();
        assert_eq!(full.bytes, b"23456789");
        assert_eq!(full.total_size, 10);
        assert_eq!(
            full.content_range,
            Some(RemoteContentRange {
                start: 2,
                end: 9,
                total: 10,
            })
        );
        assert!(full.accept_ranges);
    }

    #[test]
    fn bounded_article_ranges_reject_out_of_bounds_requests() {
        let error = bounded_session_body(
            b"body".to_vec(),
            Some(RemoteByteRange {
                start: 4,
                end: Some(5),
            }),
            "text/html",
        )
        .unwrap_err();
        assert_eq!(error.code().as_str(), "RANGE_INVALID");
    }

    #[test]
    fn remote_pdf_range_validation_requires_exact_206_metadata_and_length() {
        let response = RemoteHttpResponse {
            bytes: b"page".to_vec(),
            content_type: Some("application/pdf".to_owned()),
            total_size: 100,
            content_range: Some(RemoteContentRange {
                start: 10,
                end: 13,
                total: 100,
            }),
            accept_ranges: true,
            partial: true,
        };
        assert!(
            validate_range_response(
                &response,
                RemoteByteRange {
                    start: 10,
                    end: Some(20),
                },
                response.content_range.unwrap(),
            )
            .is_ok()
        );

        let mut malformed = response.clone();
        malformed.partial = false;
        assert!(
            validate_range_response(
                &malformed,
                RemoteByteRange {
                    start: 10,
                    end: Some(20),
                },
                RemoteContentRange {
                    start: 10,
                    end: 13,
                    total: 100,
                },
            )
            .is_err()
        );

        let mut wrong_length = response;
        wrong_length.bytes = b"too-long".to_vec();
        assert!(
            validate_range_response(
                &wrong_length,
                RemoteByteRange {
                    start: 10,
                    end: Some(20),
                },
                RemoteContentRange {
                    start: 10,
                    end: 13,
                    total: 100,
                },
            )
            .is_err()
        );
    }
}
