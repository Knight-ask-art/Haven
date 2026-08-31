//! OPDS 1.x（Atom）书源适配器（V2-H1）。
//!
//! 安全边界（对齐 cms10.rs S8：防 SSRF / 出站最小化）：
//! - 仅访问注册表配置端点、其 OpenSearch 模板解析结果与条目页/获取链接；
//!   全部限定 http(s)，下载阶段拒绝重定向到非 http(s)。
//! - Feed 响应 ≤ 8 MiB；EPUB 获取 ≤ 64 MiB（Content-Length 预检 + 流式计数双保险）。
//! - 日志只允许 host，不落完整 URL / 查询串 / 书目内容。
//! - 落盘文件只写入已登记的本地存储位置（受控资源，禁止任意路径拼接）。

use std::collections::HashMap;
use std::io::{Cursor, Read};
use std::path::Path;
use std::sync::{Arc, Mutex};

use async_trait::async_trait;

use haven_application::services::SourceRegistryService;
use haven_application::services::ports::{RemoteAcquiredFile, RemoteAcquisitionPort};
use haven_application::services::search_source::SearchSourceParticipant;
use haven_application::services::source_import::{
    RemoteContentRef, SourceCatalogEntry, SourceCatalogProvider,
};
use haven_common::{AppError, ErrorKind};

use tokio::io::AsyncWriteExt;

use zip::ZipArchive;

use crate::cms10::{CMS10_SOURCE_ID, Cms10CatalogProvider};

/// OPDS 书源内置 ID（必须与 source_registry 内置目录一致）。
pub const OPDS_SOURCE_GUTENBERG: &str = "opds_gutenberg";

pub const OPDS_SOURCE_IDS: [&str; 1] = [OPDS_SOURCE_GUTENBERG];

pub fn is_opds_source_id(source_id: &str) -> bool {
    OPDS_SOURCE_IDS.contains(&source_id)
}

const FEED_CAP_BYTES: usize = 8 * 1024 * 1024;
const BOOK_CAP_BYTES: u64 = 64 * 1024 * 1024;
const EPUB_MIME: &str = "application/epub+zip";
const MAX_REDIRECTS: usize = 3;

/// 单条 OPDS 条目（解析投影）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OpdsEntry {
    pub entry_id: String,
    pub title: String,
    pub author: Option<String>,
    pub summary: Option<String>,
    /// 封面图地址（http(s) 才保留）。
    pub pic: Option<String>,
    /// 首个 EPUB 获取链接（绝对地址）。
    pub epub_href: Option<String>,
}

fn invalid_feed(detail: &'static str) -> AppError {
    AppError::new("SOURCE_UNAVAILABLE", ErrorKind::Network, detail, true)
}

fn storage_failure(detail: &'static str) -> AppError {
    AppError::new(
        "DOWNLOAD_DIRECTORY_UNAVAILABLE",
        ErrorKind::Storage,
        detail,
        true,
    )
}

// ---------- Atom 最小解析 ----------

/// 相对引用绝对化 + http→https 升级（Gutenberg 的 OpenSearch 模板与条目链接均为
/// http/相对路径，纯 http 在该站会断连）。仅接受同源相对路径，拒绝其它形态。
fn absolutize(base: &str, href: &str) -> Option<String> {
    let h = href.trim();
    if let Some(rest) = h.strip_prefix("https://") {
        return Some(format!("https://{rest}"));
    }
    if let Some(rest) = h.strip_prefix("http://") {
        return Some(format!("https://{rest}"));
    }
    if let Some(rest) = h.strip_prefix("//") {
        return Some(format!("https://{rest}"));
    }
    if !h.starts_with('/') {
        return None;
    }
    let (scheme, rest) = base.split_once("://")?;
    let host = rest.split('/').next()?;
    Some(format!("{scheme}://{host}{h}"))
}

#[derive(Default)]
struct EntryBuilder {
    id: String,
    title: String,
    author: Option<String>,
    summary: String,
    pic: Option<String>,
    epub_href: Option<String>,
}

/// 解析 Atom feed：条目级 link 只在 `<entry>` 作用域内收集；feed 级 link
/// （如 rel="search"）单独返回。命名空间前缀按 local-name 归一；链接按 base 绝对化。
fn parse_atom(xml: &str, base: &str) -> (Vec<OpdsEntry>, Vec<(String, String)>) {
    use quick_xml::events::Event;

    let mut reader = quick_xml::Reader::from_str(xml);
    let mut entries: Vec<OpdsEntry> = Vec::new();
    let mut feed_links: Vec<(String, String)> = Vec::new();
    let mut depth_entry = 0usize;
    let mut builder = EntryBuilder::default();
    // 当前正在累积文本的字段：title | summary | author_name | id
    let mut text_target: Option<&'static str> = None;

    loop {
        match reader.read_event() {
            Ok(Event::Start(e)) => {
                let name = local_name(&e);
                match name.as_str() {
                    "entry" => {
                        depth_entry += 1;
                        builder = EntryBuilder::default();
                    }
                    "title" if depth_entry > 0 => text_target = Some("title"),
                    "summary" | "content" if depth_entry > 0 => text_target = Some("summary"),
                    "id" if depth_entry > 0 => text_target = Some("id"),
                    "name" if depth_entry > 0 => text_target = Some("author"),
                    _ => {}
                }
                if name == "link" {
                    collect_link(&e, depth_entry, base, &mut builder, &mut feed_links);
                }
            }
            Ok(Event::Empty(e)) => {
                if local_name(&e) == "link" {
                    collect_link(&e, depth_entry, base, &mut builder, &mut feed_links);
                }
            }
            Ok(Event::Text(t)) => {
                if let Some(field) = text_target {
                    let text = t.unescape().map(|c| c.into_owned()).unwrap_or_default();
                    apply_text(field, &text, &mut builder);
                }
            }
            Ok(Event::CData(t)) => {
                if let Some(field) = text_target {
                    let raw = t.into_inner();
                    let text = std::str::from_utf8(raw.as_ref()).unwrap_or("");
                    apply_text(field, text, &mut builder);
                }
            }
            Ok(Event::End(e)) => {
                let name = local_name_of(e.name().as_ref());
                if name == "entry" && depth_entry > 0 {
                    depth_entry -= 1;
                    if depth_entry == 0 && !builder.id.is_empty() && !builder.title.is_empty() {
                        entries.push(OpdsEntry {
                            entry_id: std::mem::take(&mut builder.id),
                            title: std::mem::take(&mut builder.title),
                            author: builder.author.take(),
                            summary: take_summary(&mut builder.summary),
                            pic: builder.pic.take(),
                            epub_href: builder.epub_href.take(),
                        });
                    } else {
                        builder = EntryBuilder::default();
                    }
                }
                text_target = None;
            }
            Ok(Event::Eof) => break,
            Err(_)
            | Ok(Event::Comment(_))
            | Ok(Event::PI(_))
            | Ok(Event::Decl(_))
            | Ok(Event::DocType(_)) => {
                text_target = None;
            }
        }
    }
    (entries, feed_links)
}

/// 书籍候选过滤（Gutenberg 实测形态）：搜索结果的书目条目不带获取链接，
/// 其 `<id>` 即二级条目页（如 https://.../ebooks/84.opds）；Authors/Subjects
/// 聚合检索面的 id 指向 search 端点。因此按检索面签名排除，而非要求直接链接。
fn book_candidates(entries: Vec<OpdsEntry>) -> Vec<OpdsEntry> {
    entries
        .into_iter()
        .filter(|e| {
            if e.epub_href.is_some() {
                return true;
            }
            let id = e.entry_id.to_ascii_lowercase();
            !(id.contains("search.opds") || id.contains("/search?") || id.contains("?query="))
        })
        .collect()
}

fn take_summary(raw: &mut str) -> Option<String> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        None
    } else {
        let mut out = trimmed.to_owned();
        out.truncate(2000);
        Some(out)
    }
}

fn local_name_of(raw: &[u8]) -> String {
    let name = String::from_utf8_lossy(raw);
    name.rsplit(':').next().unwrap_or("").to_owned()
}

fn local_name(e: &quick_xml::events::BytesStart<'_>) -> String {
    local_name_of(e.name().as_ref())
}

fn apply_text(field: &str, text: &str, builder: &mut EntryBuilder) {
    match field {
        "title" => builder.title.push_str(text.trim()),
        "summary" => builder.summary.push_str(text.trim()),
        "id" => builder.id.push_str(text.trim()),
        "author" => builder.author = Some(text.trim().to_owned()),
        _ => {}
    }
}

fn attr(e: &quick_xml::events::BytesStart<'_>, key: &str) -> Option<String> {
    e.attributes().find_map(|a| {
        let a = a.ok()?;
        let k = String::from_utf8_lossy(a.key.as_ref());
        if k.rsplit(':').next()? == key {
            Some(String::from_utf8_lossy(&a.value).into_owned())
        } else {
            None
        }
    })
}

fn collect_link(
    e: &quick_xml::events::BytesStart<'_>,
    depth_entry: usize,
    base: &str,
    builder: &mut EntryBuilder,
    feed_links: &mut Vec<(String, String)>,
) {
    let Some(href) = attr(e, "href") else { return };
    let rel = attr(e, "rel").unwrap_or_default();
    let typ = attr(e, "type").unwrap_or_default();
    if depth_entry == 0 {
        feed_links.push((rel, href));
        return;
    }
    let absolute = absolutize(base, &href);
    if typ.contains("epub") || (rel.contains("acquisition") && href.ends_with(".epub")) {
        if builder.epub_href.is_none() {
            builder.epub_href = absolute;
        }
        return;
    }
    if typ.starts_with("image/") && rel.contains("image") && builder.pic.is_none() {
        builder.pic = absolute;
    }
}

// ---------- HTTP 客户端 ----------

/// 自定义源凭据解析器：sourceId → Basic Auth 密码（secret 只在内存，禁止落盘/日志）。
pub type CredentialResolver = dyn Fn(&str) -> std::pin::Pin<Box<dyn std::future::Future<Output = Option<String>> + Send>>
    + Send
    + Sync;

#[derive(Clone)]
pub struct OpdsClient {
    http: reqwest::Client,
    /// 每个端点的 OpenSearch atom 模板（含 `{searchTerms}`）。二次搜索跳过根目录 + 描述文件 hop。
    search_templates: Arc<Mutex<HashMap<String, String>>>,
    /// 私有源凭据提供方。
    credential_resolver: Option<Arc<CredentialResolver>>,
}

impl OpdsClient {
    pub fn new() -> Result<Self, AppError> {
        let http = reqwest::Client::builder()
            .user_agent("haven/0.1")
            .timeout(std::time::Duration::from_secs(15))
            // Redirects are followed explicitly in `get_limited_once`.  This
            // lets the built-in Gutenberg source re-validate every hop instead
            // of allowing reqwest to visit an arbitrary host first.
            .redirect(reqwest::redirect::Policy::none())
            .build()
            .map_err(|_| invalid_feed("HTTP 客户端初始化失败"))?;
        Ok(Self {
            http,
            search_templates: Arc::new(Mutex::new(HashMap::new())),
            credential_resolver: None,
        })
    }

    /// 注入自定义源凭据解析器：sourceId → Basic Auth 密码（内存即取即用）。
    pub fn with_credential_resolver(mut self, resolver: Arc<CredentialResolver>) -> Self {
        self.credential_resolver = Some(resolver);
        self
    }

    async fn get_limited(
        &self,
        source_id: Option<&str>,
        url: &str,
        cap: usize,
    ) -> Result<Vec<u8>, AppError> {
        // 网络对部分书目站存在间歇性 TLS 重置：传输层错误单次重试（响应上限不变）。
        match self.get_limited_once(source_id, url, cap).await {
            Err(e) if e.retryable() => self.get_limited_once(source_id, url, cap).await,
            other => other,
        }
    }

    async fn get_limited_once(
        &self,
        source_id: Option<&str>,
        url: &str,
        cap: usize,
    ) -> Result<Vec<u8>, AppError> {
        let mut current = validate_opds_url(url, source_id)?;
        for _ in 0..=MAX_REDIRECTS {
            let mut builder = self.http.get(current.clone());
            if let (Some(sid), Some(resolver)) = (source_id, &self.credential_resolver) {
                // Built-in sources are always anonymous.  We still pass the
                // source key here so redirect policy can be enforced.
                if !is_builtin_opds(sid) {
                    if let Some(secret) = resolver(sid).await {
                        // secret 即取即用；reqwest 内部按 header 编码，不落日志。
                        builder = builder.basic_auth::<&str, String>(sid, Some(secret));
                    }
                }
            }
            let resp = builder
                .send()
                .await
                .map_err(|_| invalid_feed("目录服务不可达"))?;
            if resp.status().is_redirection() {
                let location = resp
                    .headers()
                    .get(reqwest::header::LOCATION)
                    .and_then(|value| value.to_str().ok())
                    .ok_or_else(|| invalid_feed("目录重定向地址无效"))?;
                current = current
                    .join(location)
                    .map_err(|_| invalid_feed("目录重定向地址无效"))?;
                validate_opds_url(current.as_str(), source_id)?;
                continue;
            }
            if !resp.status().is_success() {
                return Err(invalid_feed("目录响应异常"));
            }
            if let Some(len) = resp.content_length() {
                if len > cap as u64 {
                    return Err(invalid_feed("目录响应超出大小上限"));
                }
            }
            let mut out: Vec<u8> = Vec::with_capacity(64 * 1024);
            let mut stream = resp;
            while let Some(chunk) = stream
                .chunk()
                .await
                .map_err(|_| invalid_feed("目录读取中断"))?
            {
                if out.len().saturating_add(chunk.len()) > cap {
                    return Err(invalid_feed("目录响应超出大小上限"));
                }
                out.extend_from_slice(chunk.as_ref());
            }
            return Ok(out);
        }
        Err(invalid_feed("目录重定向次数过多"))
    }

    async fn get_feed(&self, source_id: Option<&str>, url: &str) -> Result<Vec<u8>, AppError> {
        self.get_limited(source_id, url, FEED_CAP_BYTES).await
    }

    async fn get_feed_with_host_fallback(
        &self,
        source_id: Option<&str>,
        url: &str,
    ) -> Result<(String, Vec<u8>), AppError> {
        let mut last = None;
        for candidate in with_gutenberg_host_fallbacks(url) {
            match self.get_feed(source_id, &candidate).await {
                Ok(body) => return Ok((candidate, body)),
                Err(err) => last = Some(err),
            }
        }
        Err(last.unwrap_or_else(|| invalid_feed("目录服务不可达")))
    }

    async fn get_limited_with_host_fallback(
        &self,
        source_id: Option<&str>,
        url: &str,
        cap: usize,
    ) -> Result<Vec<u8>, AppError> {
        let mut last = None;
        for candidate in with_gutenberg_host_fallbacks(url) {
            match self.get_limited(source_id, &candidate, cap).await {
                Ok(body) => return Ok(body),
                Err(err) => last = Some(err),
            }
        }
        Err(last.unwrap_or_else(|| invalid_feed("目录服务不可达")))
    }

    fn cached_template(&self, endpoint: &str) -> Option<String> {
        self.search_templates
            .lock()
            .ok()
            .and_then(|g| g.get(endpoint).cloned())
    }

    fn remember_template(&self, endpoint: &str, template: String) {
        if let Ok(mut g) = self.search_templates.lock() {
            g.insert(endpoint.to_owned(), template);
        }
    }

    fn apply_template(template: &str, query: &str, base: &str) -> Option<String> {
        absolutize(base, &template.replace("{searchTerms}", &urlencode(query)))
    }

    /// 搜索地址解析：优先缓存模板 → OpenSearch atom 模板 → Gutenberg search.opds → `?query=`。
    /// 模板结果统一过 absolutize（含 http→https 升级，规避纯 http 断连站）。
    pub async fn search_url(
        &self,
        source_id: Option<&str>,
        endpoint: &str,
        query: &str,
    ) -> Result<String, AppError> {
        if let Some(tpl) = self.cached_template(endpoint) {
            if let Some(url) = Self::apply_template(&tpl, query, endpoint) {
                return Ok(url);
            }
        }
        // Gutenberg 的 m./www. 镜像在部分网络中的可用性不同；根目录解析
        // 也必须走同一组受控镜像回退，否则仅因根目录一次 TLS/504 波动就
        // 会把一个已经有真实搜索实现的内置来源误判为不可搜索。
        let (resolved_endpoint, root) = if is_gutenberg_endpoint(endpoint) {
            self.get_feed_with_host_fallback(source_id, endpoint)
                .await?
        } else {
            (
                endpoint.to_owned(),
                self.get_feed(source_id, endpoint).await?,
            )
        };
        let xml = std::str::from_utf8(&root).map_err(|_| invalid_feed("目录编码异常"))?;
        let (_, feed_links) = parse_atom(xml, &resolved_endpoint);
        if let Some((_, search_desc)) = feed_links.iter().find(|(rel, _)| rel == "search") {
            let absolute_desc = absolutize(&resolved_endpoint, search_desc)
                .ok_or_else(|| invalid_feed("搜索描述地址非法"))?;
            if let Ok(desc) = self
                .get_limited(source_id, &absolute_desc, 256 * 1024)
                .await
            {
                if let Some(tpl) = extract_atom_template(&String::from_utf8_lossy(&desc)) {
                    if let Some(url) = Self::apply_template(&tpl, query, &absolute_desc) {
                        self.remember_template(endpoint, tpl);
                        return Ok(url);
                    }
                }
            }
        }
        if let Some(url) = gutenberg_search_mirrors(query)
            .into_iter()
            .next()
            .filter(|_| is_gutenberg_endpoint(endpoint))
        {
            return Ok(url);
        }
        let sep = if endpoint.contains('?') { '&' } else { '?' };
        absolutize(
            endpoint,
            &format!("{endpoint}{sep}query={}", urlencode(query)),
        )
        .ok_or_else(|| invalid_feed("搜索地址解析失败"))
    }

    pub async fn search_entries(
        &self,
        source_id: Option<&str>,
        endpoint: &str,
        query: &str,
        limit: u32,
    ) -> Result<Vec<OpdsEntry>, AppError> {
        let mut urls: Vec<String> = Vec::new();
        // Gutenberg：www 镜像优先（m. 对 search.opds 常 504），成功则跳过 OpenSearch。
        if is_gutenberg_endpoint(endpoint) {
            urls.extend(gutenberg_search_mirrors(query));
        } else if let Ok(url) = self.search_url(source_id, endpoint, query).await {
            urls.push(url);
        }
        let mut last_err: Option<AppError> = None;
        if let Some(entries) = self
            .try_search_urls(source_id, &urls, limit, &mut last_err)
            .await
        {
            return Ok(entries);
        }
        if is_gutenberg_endpoint(endpoint) {
            if let Ok(url) = self.search_url(source_id, endpoint, query).await {
                let extras: Vec<String> = with_gutenberg_host_fallbacks(&url)
                    .into_iter()
                    .filter(|u| !urls.contains(u))
                    .collect();
                if let Some(entries) = self
                    .try_search_urls(source_id, &extras, limit, &mut last_err)
                    .await
                {
                    return Ok(entries);
                }
            }
        }
        Err(last_err.unwrap_or_else(|| invalid_feed("目录服务不可达")))
    }

    async fn try_search_urls(
        &self,
        source_id: Option<&str>,
        urls: &[String],
        limit: u32,
        last_err: &mut Option<AppError>,
    ) -> Option<Vec<OpdsEntry>> {
        for url in urls {
            match self.get_feed(source_id, url).await {
                Ok(body) => {
                    let xml = String::from_utf8_lossy(&body);
                    let mut entries = book_candidates(parse_atom(&xml, url).0);
                    if entries.is_empty() {
                        continue;
                    }
                    entries.truncate(limit as usize);
                    return Some(entries);
                }
                Err(err) => *last_err = Some(err),
            }
        }
        None
    }

    /// 条目页 → 只解析单条目元数据，不获取 EPUB 正文。
    pub async fn fetch_entry_metadata(
        &self,
        source_id: Option<&str>,
        entry_page_url: &str,
    ) -> Result<OpdsEntry, AppError> {
        let (page_url, body) = self
            .get_feed_with_host_fallback(source_id, entry_page_url)
            .await?;
        let xml = String::from_utf8_lossy(&body);
        let entries = parse_atom(&xml, &page_url).0;
        let entry = entries
            .into_iter()
            .find(|e| e.epub_href.is_some() || !e.title.is_empty())
            .ok_or_else(|| invalid_feed("该条目没有可用元数据"))?;
        Ok(entry)
    }

    /// 仅供显式 DownloadTask 使用：条目页 → 获取首个 EPUB。
    /// 搜索/导入流程不得调用此方法。
    pub async fn fetch_entry_and_epub(
        &self,
        source_id: Option<&str>,
        entry_page_url: &str,
    ) -> Result<(OpdsEntry, Vec<u8>), AppError> {
        let entry = self.fetch_entry_metadata(source_id, entry_page_url).await?;
        let href = entry
            .epub_href
            .clone()
            .ok_or_else(|| invalid_feed("该条目没有 EPUB 获取链接"))?;
        let data = self
            .get_limited_with_host_fallback(source_id, &href, BOOK_CAP_BYTES as usize)
            .await?;
        Ok((entry, data.to_vec()))
    }
}

/// OpenSearch 描述里第一个 type 含 atom 的 `<Url template="...">`（首个命中即返回；
/// Gutenberg 的描述文件中 text/html 与 suggestions 条目排在 atom 前后，不能取末尾）。
fn extract_atom_template(desc: &str) -> Option<String> {
    let mut reader = quick_xml::Reader::from_str(desc);
    loop {
        match reader.read_event() {
            Ok(quick_xml::events::Event::Start(e)) | Ok(quick_xml::events::Event::Empty(e)) => {
                if local_name(&e) == "Url" {
                    let typ = attr(&e, "type").unwrap_or_default();
                    if typ.contains("atom") {
                        if let Some(tpl) = attr(&e, "template") {
                            return Some(tpl);
                        }
                    }
                }
            }
            Ok(quick_xml::events::Event::Eof) => return None,
            Err(_) => return None,
            _ => {}
        }
    }
}

fn is_gutenberg_endpoint(endpoint: &str) -> bool {
    endpoint.to_ascii_lowercase().contains("gutenberg.org")
}

/// 校验 OPDS 请求地址。自定义目录可以访问其已登记的 http(s) 端点；内置
/// Gutenberg 只允许公开站点的 HTTPS 主机，并且每个手工跟随的重定向都会再次
/// 进入本函数。请求凭据、端口和片段不会从候选句柄进入网络层。
fn validate_opds_url(raw: &str, source_id: Option<&str>) -> Result<reqwest::Url, AppError> {
    let url = reqwest::Url::parse(raw).map_err(|_| invalid_feed("目录地址无效"))?;
    if !matches!(url.scheme(), "http" | "https")
        || !url.username().is_empty()
        || url.password().is_some()
        || url.fragment().is_some()
    {
        return Err(invalid_feed("目录地址不安全"));
    }
    let host = url
        .host_str()
        .ok_or_else(|| invalid_feed("目录地址缺少主机"))?
        .to_ascii_lowercase();
    if source_id.is_some_and(is_builtin_opds)
        && (url.scheme() != "https"
            // `url::Url::port()` normalizes an explicitly written default
            // port (`:443`) to `None`.  Inspect the authority as well so the
            // built-in allowlist never accepts a caller-supplied port.
            || url.port().is_some()
            || has_explicit_authority_port(raw)
            || !matches!(host.as_str(), "www.gutenberg.org" | "m.gutenberg.org"))
    {
        return Err(invalid_feed("古腾堡目录地址不安全"));
    }
    Ok(url)
}

/// Return whether the raw URL authority contains an explicit port.  This is
/// intentionally only used after URL parsing and user-info rejection; it
/// exists because the URL parser erases the default HTTPS port when exposing
/// `Url::port()`.  Gutenberg only needs the bare public HTTPS hosts.
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

fn validate_builtin_gutenberg_url(raw: &str) -> Result<reqwest::Url, AppError> {
    validate_opds_url(raw, Some(OPDS_SOURCE_GUTENBERG))
}

fn is_epub_payload(bytes: &[u8]) -> bool {
    // ZIP local-file, empty-archive and spanned-archive signatures are all
    // valid ZIP headers.  The EPUB `mimetype` entry check below is the stronger
    // discriminator and prevents accepting arbitrary ZIP downloads.
    bytes.starts_with(b"PK\x03\x04")
        || bytes.starts_with(b"PK\x05\x06")
        || bytes.starts_with(b"PK\x07\x08")
}

fn validate_epub_payload(bytes: &[u8]) -> Result<(), AppError> {
    if !is_epub_payload(bytes) {
        return Err(invalid_feed("目录返回的文件不是 EPUB"));
    }
    let mut archive = ZipArchive::new(Cursor::new(bytes))
        .map_err(|_| invalid_feed("目录返回的 EPUB 归档损坏"))?;
    let mut mimetype = archive
        .by_name("mimetype")
        .map_err(|_| invalid_feed("目录返回的 EPUB 缺少类型声明"))?;
    let mut value = String::new();
    mimetype
        .read_to_string(&mut value)
        .map_err(|_| invalid_feed("目录返回的 EPUB 类型声明无效"))?;
    if value != EPUB_MIME {
        return Err(invalid_feed("目录返回的文件不是 EPUB"));
    }
    Ok(())
}

/// 内置三预设（匿名访问）；仅自定义 `custom_` 前缀源注入凭据。
fn is_builtin_opds(source_id: &str) -> bool {
    OPDS_SOURCE_IDS.contains(&source_id)
}

/// Gutenberg 搜索镜像：www 优先（m. 在部分网络对 search.opds 返回 504）。
fn gutenberg_search_mirrors(query: &str) -> Vec<String> {
    let q = urlencode(query);
    vec![
        format!("https://www.gutenberg.org/ebooks/search.opds/?query={q}"),
        format!("https://m.gutenberg.org/ebooks/search.opds/?query={q}"),
    ]
}

fn with_gutenberg_host_fallbacks(url: &str) -> Vec<String> {
    let mut out = vec![url.to_owned()];
    if url.contains("://m.gutenberg.org") {
        out.push(url.replacen("://m.gutenberg.org", "://www.gutenberg.org", 1));
    } else if url.contains("://www.gutenberg.org") {
        out.push(url.replacen("://www.gutenberg.org", "://m.gutenberg.org", 1));
    }
    out
}

fn urlencode(input: &str) -> String {
    let mut out = String::with_capacity(input.len());
    for b in input.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(b as char)
            }
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}

// ---------- 目录提供方（元数据 + 远端身份） ----------

/// OPDS 目录适配器：detail 只抓取条目元数据，不下载 EPUB。
pub struct OpdsCatalogProvider {
    client: Arc<OpdsClient>,
}

impl OpdsCatalogProvider {
    pub fn new(client: Arc<OpdsClient>) -> Self {
        Self { client }
    }
}

#[async_trait]
impl SourceCatalogProvider for OpdsCatalogProvider {
    async fn detail(
        &self,
        source_id: &str,
        endpoint: &str,
        external_id: &str,
    ) -> Result<SourceCatalogEntry, AppError> {
        let is_custom = source_id
            .starts_with(haven_application::services::source_registry::CUSTOM_SOURCE_PREFIX);
        if !is_opds_source_id(source_id) && !is_custom {
            return Err(AppError::new(
                "INVALID_ARGUMENT",
                ErrorKind::Validation,
                "未知来源目录",
                false,
            ));
        }
        let _ = endpoint; // external_id 已是绝对条目页地址
        // 私有源凭据：sourceId 即 profile（`haven:opds:<sourceId>`），detail 走同源 Basic Auth。
        // Built-in source key is passed to the HTTP layer for strict host and
        // redirect validation; `get_limited_once` deliberately skips auth for
        // built-in IDs. Custom OPDS IDs retain their credential lookup path.
        let source_for_auth =
            (is_opds_source_id(source_id) || is_custom).then(|| source_id.to_owned());
        let entry = self
            .client
            .fetch_entry_metadata(source_for_auth.as_deref(), external_id)
            .await?;

        Ok(SourceCatalogEntry {
            external_id: external_id.to_owned(),
            title: entry.title,
            year: None,
            type_name: Some("图书".to_owned()),
            pic: entry.pic,
            episodes: Vec::new(),
            content: entry.summary,
            director: entry.author,
            actor: None,
            local_file: None,
            media_type: Some(haven_domain::enums::MediaType::Book),
            remote: Some(RemoteContentRef {
                source_key: source_id.to_owned(),
                remote_id: external_id.to_owned(),
                media_type: haven_domain::enums::MediaType::Book,
                mime_type: Some(EPUB_MIME.to_owned()),
            }),
        })
    }
}

/// OPDS/Gutenberg 的正文只在 DownloadTask 中获取。导入阶段登记的
/// `RemoteContentRef.remote_id` 是经过后端生成的条目页身份；这里再次校验
/// 固定主机，读取 EPUB 后先落到 provider 临时文件，成功后再原子替换 Worker
/// 的 `.part` 文件。任何失败都会清理两种临时文件。
#[async_trait]
impl RemoteAcquisitionPort for OpdsCatalogProvider {
    async fn acquire(
        &self,
        source_key: &str,
        remote_id: &str,
        destination: &Path,
    ) -> Result<RemoteAcquiredFile, AppError> {
        if source_key != OPDS_SOURCE_GUTENBERG {
            return Err(AppError::new(
                "INVALID_ARGUMENT",
                ErrorKind::Validation,
                "未知远端书籍来源",
                false,
            ));
        }
        if destination.as_os_str().is_empty() {
            return Err(storage_failure("书籍临时文件路径无效"));
        }
        let remote_url = validate_builtin_gutenberg_url(remote_id)?;
        let entry = self
            .client
            .fetch_entry_metadata(Some(source_key), remote_url.as_str())
            .await?;
        let href = entry
            .epub_href
            .as_deref()
            .ok_or_else(|| invalid_feed("该条目没有 EPUB 获取链接"))?;
        let href = validate_builtin_gutenberg_url(href)?;
        let bytes = self
            .client
            .get_limited_with_host_fallback(
                Some(source_key),
                href.as_str(),
                BOOK_CAP_BYTES as usize,
            )
            .await?;
        validate_epub_payload(&bytes)?;
        let size_bytes = write_epub_atomic(destination, &bytes).await?;
        Ok(RemoteAcquiredFile {
            size_bytes,
            mime: EPUB_MIME.to_owned(),
        })
    }
}

/// Download Worker 使用的远端来源路由。OPDS Provider 与 MangaDex/arXiv/
/// Europe PMC/Wikisource Provider 都实现同一个 Application Port；组合根只需
/// 注入此路由器即可，Worker 不需要知道具体 Infrastructure 类型。
pub struct RoutingRemoteAcquisitionPort {
    opds: Arc<OpdsCatalogProvider>,
    online: Arc<dyn RemoteAcquisitionPort>,
}

impl RoutingRemoteAcquisitionPort {
    pub fn new(opds: Arc<OpdsCatalogProvider>, online: Arc<dyn RemoteAcquisitionPort>) -> Self {
        Self { opds, online }
    }
}

#[async_trait]
impl RemoteAcquisitionPort for RoutingRemoteAcquisitionPort {
    async fn acquire(
        &self,
        source_key: &str,
        remote_id: &str,
        destination: &Path,
    ) -> Result<RemoteAcquiredFile, AppError> {
        if source_key == OPDS_SOURCE_GUTENBERG {
            self.opds.acquire(source_key, remote_id, destination).await
        } else {
            self.online
                .acquire(source_key, remote_id, destination)
                .await
        }
    }
}

async fn write_epub_atomic(destination: &Path, bytes: &[u8]) -> Result<u64, AppError> {
    let parent = destination
        .parent()
        .filter(|path| !path.as_os_str().is_empty())
        .ok_or_else(|| storage_failure("书籍临时文件路径无效"))?;
    tokio::fs::create_dir_all(parent)
        .await
        .map_err(|_| storage_failure("书籍目录创建失败"))?;
    let temporary = destination.with_extension("provider-part");
    // Provider 获取不支持从旧 `.part` 续传；覆盖前一次未完成尝试，避免
    // Windows rename 因为目标已存在而失败。
    let _ = tokio::fs::remove_file(&temporary).await;
    let _ = tokio::fs::remove_file(destination).await;
    let result = async {
        let mut file = tokio::fs::File::create(&temporary)
            .await
            .map_err(|_| storage_failure("书籍临时文件创建失败"))?;
        file.write_all(bytes)
            .await
            .map_err(|_| storage_failure("书籍写入失败"))?;
        file.sync_all()
            .await
            .map_err(|_| storage_failure("书籍写入失败"))?;
        drop(file);
        tokio::fs::rename(&temporary, destination)
            .await
            .map_err(|_| storage_failure("书籍保存失败"))?;
        Ok(bytes.len() as u64)
    }
    .await;
    if result.is_err() {
        let _ = tokio::fs::remove_file(&temporary).await;
        let _ = tokio::fs::remove_file(destination).await;
    }
    result
}

// ---------- 搜索参与者 ----------

/// 每个 OPDS 来源一个参与者实例；分类声明为 Book。
pub struct OpdsSearchParticipant {
    source_id: String,
    registry: SourceRegistryService,
    client: Arc<OpdsClient>,
}

impl OpdsSearchParticipant {
    pub fn new(
        source_id: impl Into<String>,
        registry: SourceRegistryService,
        client: Arc<OpdsClient>,
    ) -> Self {
        Self {
            source_id: source_id.into(),
            registry,
            client,
        }
    }
}

/// OPDS 自定义源家族参与者：以 `custom_` 前缀路由所有自定义源
/// （端点与启用状态每次搜索时经 SourceRegistryService 读取）。
pub const CUSTOM_OPDS_ID_PREFIX: &str =
    haven_application::services::source_registry::CUSTOM_SOURCE_PREFIX;

#[async_trait]
impl SearchSourceParticipant for OpdsSearchParticipant {
    fn source_id(&self) -> &str {
        &self.source_id
    }

    fn id_prefix(&self) -> Option<&str> {
        self.source_id
            .starts_with(CUSTOM_OPDS_ID_PREFIX)
            .then_some(CUSTOM_OPDS_ID_PREFIX)
    }

    fn supports_category(&self, category: Option<haven_application::wire::QueryCategory>) -> bool {
        matches!(
            category,
            None | Some(haven_application::wire::QueryCategory::All)
                | Some(haven_application::wire::QueryCategory::Book)
        )
    }

    async fn search(
        &self,
        query: &str,
        limit: u32,
        is_cancelled: &(dyn Fn() -> bool + Send + Sync),
    ) -> Result<Vec<haven_application::wire::WorkCardDto>, AppError> {
        self.search_for(&self.source_id, query, limit, is_cancelled)
            .await
    }

    async fn search_for(
        &self,
        dispatched_id: &str,
        query: &str,
        limit: u32,
        is_cancelled: &(dyn Fn() -> bool + Send + Sync),
    ) -> Result<Vec<haven_application::wire::WorkCardDto>, AppError> {
        let Some(endpoint) = self.registry.endpoint(dispatched_id).await? else {
            return Ok(Vec::new());
        };
        if is_cancelled() {
            return Ok(Vec::new());
        }
        // Keep the built-in key visible to the HTTP policy while still making
        // its requests anonymous (the client skips credential injection for
        // built-in IDs).
        let source_for_auth: Option<&str> = Some(dispatched_id);
        let entries = self
            .client
            .search_entries(source_for_auth, &endpoint, query, limit)
            .await?;
        Ok(entries
            .into_iter()
            .map(|entry| haven_application::wire::WorkCardDto {
                // The operation cache must never expose a directly callable remote
                // URL as a candidate handle. Keep the source key visible only for
                // server-side routing and percent-encode the entry identity; the
                // import service is the sole component that decodes it after the
                // source policy has been checked.
                work_id: format!(
                    "{}{}\u{1}{}",
                    haven_application::services::OPDS_CANDIDATE_PREFIX,
                    dispatched_id,
                    urlencode(&entry.entry_id)
                ),
                title: entry.title,
                original_title: entry.author,
                description: None,
                categories: vec![haven_application::wire::ContentCategory::Book],
                available_media_types: vec![haven_application::wire::MediaTypeDto::Book],
                poster_uri: None,
                backdrop_uri: None,
                release_year: None,
                rating_value: None,
                rating_scale: None,
                favorite: false,
                progress: None,
                primary_action: None,
                external_ids: Vec::new(),
            })
            .collect())
    }
}

/// 多来源目录路由器：按 sourceId 把 detail/search 分派给对应 Provider，
/// 让既有 SourceImportService 保持单一 catalog 注入不变。
pub struct RoutingSourceCatalogProvider {
    routes: std::collections::HashMap<String, Arc<dyn SourceCatalogProvider>>,
    /// 自定义源 detail 兜底（与内置 OPDS 同一 provider）。
    opds_fallback: Option<Arc<dyn SourceCatalogProvider>>,
}

impl RoutingSourceCatalogProvider {
    pub fn new(
        cms10: Arc<Cms10CatalogProvider>,
        opds: Arc<OpdsCatalogProvider>,
        online: Arc<dyn SourceCatalogProvider>,
    ) -> Self {
        let mut routes: std::collections::HashMap<String, Arc<dyn SourceCatalogProvider>> =
            std::collections::HashMap::new();
        routes.insert(CMS10_SOURCE_ID.to_owned(), cms10);
        for id in OPDS_SOURCE_IDS {
            routes.insert(id.to_owned(), opds.clone());
        }
        for id in ["mangadex", "arxiv", "europepmc", "wikisource"] {
            routes.insert(id.to_owned(), online.clone());
        }
        Self {
            routes,
            opds_fallback: Some(opds),
        }
    }

    /// 注册自定义源路由（V2-H 收尾批次）：detail 复用 OpdsCatalogProvider。
    pub fn register_custom(&mut self, source_id: &str, provider: Arc<dyn SourceCatalogProvider>) {
        self.routes.insert(source_id.to_owned(), provider);
    }

    fn route(&self, source_id: &str) -> Result<Arc<dyn SourceCatalogProvider>, AppError> {
        if let Some(provider) = self.routes.get(source_id) {
            return Ok(provider.clone());
        }
        // 自定义源：detail 与内置 OPDS 共用同一 provider（凭据由 client 按需解析）。
        if source_id.starts_with(haven_application::services::source_registry::CUSTOM_SOURCE_PREFIX)
        {
            if let Some(fallback) = self.opds_fallback.clone() {
                return Ok(fallback);
            }
            return Err(AppError::new(
                "INVALID_ARGUMENT",
                ErrorKind::Validation,
                "未知来源目录",
                false,
            ));
        }
        Err(AppError::new(
            "INVALID_ARGUMENT",
            ErrorKind::Validation,
            "未知来源目录",
            false,
        ))
    }
}

#[async_trait]
impl SourceCatalogProvider for RoutingSourceCatalogProvider {
    async fn detail(
        &self,
        source_id: &str,
        endpoint: &str,
        external_id: &str,
    ) -> Result<SourceCatalogEntry, AppError> {
        self.route(source_id)?
            .detail(source_id, endpoint, external_id)
            .await
    }

    async fn search(
        &self,
        source_id: &str,
        endpoint: &str,
        query: &str,
        limit: u32,
    ) -> Result<Vec<SourceCatalogEntry>, AppError> {
        self.route(source_id)?
            .search(source_id, endpoint, query, limit)
            .await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{Cursor, Write as _};
    use zip::write::SimpleFileOptions;
    use zip::{CompressionMethod, ZipWriter};

    #[test]
    fn absolutize_upgrades_http_and_resolves_relative() {
        let base = "https://m.gutenberg.org/ebooks/search.opds/?query=frankenstein";
        assert_eq!(
            absolutize(base, "/cache/epub/84/pg84.epub").unwrap(),
            "https://m.gutenberg.org/cache/epub/84/pg84.epub"
        );
        assert_eq!(
            absolutize(base, "http://m.gutenberg.org/ebooks/search.opds/?query=x").unwrap(),
            "https://m.gutenberg.org/ebooks/search.opds/?query=x"
        );
        assert_eq!(
            absolutize(base, "//cdn.example.org/a.epub").unwrap(),
            "https://cdn.example.org/a.epub"
        );
        assert_eq!(
            absolutize(base, "https://other.org/b.epub").unwrap(),
            "https://other.org/b.epub"
        );
        assert!(absolutize(base, "plain.tex").is_none());
    }

    #[test]
    fn gutenberg_mirrors_prefer_www_then_mobile() {
        let urls = gutenberg_search_mirrors("frankenstein");
        assert_eq!(
            urls[0],
            "https://www.gutenberg.org/ebooks/search.opds/?query=frankenstein"
        );
        assert_eq!(
            urls[1],
            "https://m.gutenberg.org/ebooks/search.opds/?query=frankenstein"
        );
        assert_eq!(
            with_gutenberg_host_fallbacks("https://m.gutenberg.org/ebooks/84.opds"),
            vec![
                "https://m.gutenberg.org/ebooks/84.opds".to_owned(),
                "https://www.gutenberg.org/ebooks/84.opds".to_owned(),
            ]
        );
        assert!(!is_gutenberg_endpoint(
            "https://standardebooks.org/feeds/opds"
        ));
    }

    #[test]
    fn opensearch_picks_atom_template_and_upgrades_https() {
        let desc = r#"<?xml version="1.0"?>
<OpenSearchDescription xmlns="http://a9.com/-/spec/opensearch/1.1/">
  <ShortName>Gutenberg</ShortName>
  <Url type="text/html" template="http://www.gutenberg.org/ebooks/search/?query={searchTerms}"/>
  <Url type="application/atom+xml" template="http://m.gutenberg.org/ebooks/search.opds/?query={searchTerms}"/>
</OpenSearchDescription>"#;
        let tpl = extract_atom_template(desc).unwrap();
        let url = absolutize(
            "https://www.gutenberg.org/catalog/osd-books.xml",
            &tpl.replace("{searchTerms}", &urlencode("frankenstein")),
        )
        .unwrap();
        assert_eq!(
            url,
            "https://m.gutenberg.org/ebooks/search.opds/?query=frankenstein"
        );
    }

    #[test]
    fn builtin_gutenberg_policy_rejects_untrusted_urls() {
        assert!(validate_builtin_gutenberg_url("https://www.gutenberg.org/ebooks/84.opds").is_ok());
        assert!(
            validate_builtin_gutenberg_url("https://m.gutenberg.org/cache/epub/84/pg84.epub")
                .is_ok()
        );
        assert!(validate_builtin_gutenberg_url("http://www.gutenberg.org/ebooks/84.opds").is_err());
        assert!(validate_builtin_gutenberg_url("https://evil.example/ebooks/84.opds").is_err());
        assert!(
            validate_builtin_gutenberg_url("https://www.gutenberg.org:443/ebooks/84.opds").is_err()
        );
        assert!(
            validate_builtin_gutenberg_url("https://user:secret@www.gutenberg.org/ebooks/84.opds")
                .is_err()
        );
    }

    #[test]
    fn epub_payload_requires_zip_and_mimetype() {
        assert!(validate_epub_payload(b"<html>not an epub</html>").is_err());

        let mut writer = ZipWriter::new(Cursor::new(Vec::new()));
        writer
            .start_file(
                "mimetype",
                SimpleFileOptions::default().compression_method(CompressionMethod::Stored),
            )
            .unwrap();
        writer.write_all(EPUB_MIME.as_bytes()).unwrap();
        writer
            .start_file("META-INF/container.xml", SimpleFileOptions::default())
            .unwrap();
        writer.write_all(b"<container/>").unwrap();
        let epub = writer.finish().unwrap().into_inner();
        assert!(validate_epub_payload(&epub).is_ok());

        let mut writer = ZipWriter::new(Cursor::new(Vec::new()));
        writer
            .start_file("mimetype", SimpleFileOptions::default())
            .unwrap();
        writer.write_all(b"application/zip").unwrap();
        let wrong = writer.finish().unwrap().into_inner();
        assert!(validate_epub_payload(&wrong).is_err());
    }

    #[tokio::test]
    async fn epub_atomic_write_leaves_no_provider_partial() {
        let directory = tempfile::tempdir().unwrap();
        let destination = directory.path().join("task.part");
        let bytes = b"PK\x03\x04test";
        write_epub_atomic(&destination, bytes).await.unwrap();
        assert_eq!(tokio::fs::read(&destination).await.unwrap(), bytes);
        assert!(!destination.with_extension("provider-part").exists());
    }

    #[test]
    fn parse_filters_facets_keeps_book_pages_and_absolutizes_links() {
        let base = "https://m.gutenberg.org/ebooks/search.opds/?query=frankenstein";
        let xml = r#"<?xml version="1.0" encoding="utf-8"?>
<feed xmlns="http://www.w3.org/2005/Atom" xmlns:opds="http://opds-spec.org/2010/catalog">
  <entry>
    <updated>2026-08-25T13:47:38Z</updated>
    <id>https://www.gutenberg.org/ebooks/authors/search.opds/?query=frankenstein</id>
    <title>Authors</title>
    <content type="text">One author name matches your search.</content>
    <link type="application/atom+xml;profile=opds-catalog" rel="subsection" href="/ebooks/authors/search.opds/?query=frankenstein"/>
  </entry>
  <entry>
    <updated>2026-08-25T13:47:38Z</updated>
    <id>https://www.gutenberg.org/ebooks/84.opds</id>
    <title>Frankenstein; or, the modern prometheus</title>
    <content type="text">Mary Wollstonecraft Shelley</content>
    <link type="application/atom+xml;profile=opds-catalog" rel="subsection" href="/ebooks/84.opds"/>
  </entry>
  <entry>
    <updated>2026-08-25T13:47:38Z</updated>
    <id>https://other.example.org/item/9</id>
    <title>Direct Download Book</title>
    <author><name>Anon</name></author>
    <link rel="http://opds-spec.org/acquisition" type="application/epub+zip" href="/files/9.epub"/>
    <link type="image/png" rel="http://opds-spec.org/image/thumbnail" href="/files/9.png"/>
  </entry>
</feed>"#;
        let (entries, feed_links) = parse_atom(xml, base);
        assert!(feed_links.is_empty());
        assert_eq!(entries.len(), 3);
        let books = book_candidates(entries);
        assert_eq!(books.len(), 2);
        assert_eq!(books[0].title, "Frankenstein; or, the modern prometheus");
        assert_eq!(
            books[0].entry_id,
            "https://www.gutenberg.org/ebooks/84.opds"
        );
        assert_eq!(
            books[1].epub_href.as_deref(),
            // 相对引用按 feed 地址解析（RFC 3986），与条目 id 域无关
            Some("https://m.gutenberg.org/files/9.epub")
        );
        assert_eq!(
            books[1].pic.as_deref(),
            Some("https://m.gutenberg.org/files/9.png")
        );
    }

    // ---- 自定义源凭据（V2-H 收尾批次） ----

    /// 本地 HTTP 服务器：回显 Authorization 头（仅测试内使用）。
    fn spawn_auth_echo_server() -> (String, tokio::sync::oneshot::Receiver<String>) {
        use std::io::{BufRead, BufReader, Read};
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        let (tx, rx) = tokio::sync::oneshot::channel();
        std::thread::spawn(move || {
            if let Ok((stream, _)) = listener.accept() {
                let mut reader = BufReader::new(stream);
                let mut request_line = String::new();
                if reader.read_line(&mut request_line).is_err() {
                    return;
                }
                let mut auth = String::new();
                loop {
                    let mut line = String::new();
                    match reader.read_line(&mut line) {
                        Ok(0) | Err(_) => break,
                        Ok(_) => {
                            if line.to_ascii_lowercase().starts_with("authorization:") {
                                auth = line.trim().to_owned();
                            }
                            if line == "\r\n" || line == "\n" {
                                break;
                            }
                        }
                    }
                }
                let _ = reader.read(&mut [0u8; 16]);
                let _ = tx.send(auth);
            }
        });
        (format!("http://{addr}/feed"), rx)
    }

    #[tokio::test]
    async fn basic_auth_injected_for_custom_source() {
        use base64::Engine as _;
        let (url, rx) = spawn_auth_echo_server();
        let client = OpdsClient::new()
            .unwrap()
            .with_credential_resolver(Arc::new(|source_id: &str| {
                let matched = source_id == "custom_test123456";
                Box::pin(async move { matched.then(|| "p@ss:word".to_owned()) })
            }));
        // 响应非 feed 会报错，但请求已发出；Authorization 头由服务端捕获。
        let _ = client.get_feed(Some("custom_test123456"), &url).await;
        let received = rx.await.unwrap();
        let expected =
            base64::engine::general_purpose::STANDARD.encode("custom_test123456:p@ss:word");
        assert!(
            received
                .to_ascii_lowercase()
                .ends_with(&format!("basic {expected}").to_ascii_lowercase()),
            "应注入 Basic Auth 头，实际: {received}"
        );
    }

    #[tokio::test]
    async fn builtin_source_requests_stay_anonymous() {
        let (url, rx) = spawn_auth_echo_server();
        let client = OpdsClient::new()
            .unwrap()
            .with_credential_resolver(Arc::new(|_source_id: &str| {
                Box::pin(async { Some("unused".to_owned()) })
            }));
        // 内置源路径不传 source_id → 不注入凭据。
        let _ = client.get_feed(None, &url).await;
        assert_eq!(rx.await.unwrap(), "", "内置源必须匿名访问");
    }
}
