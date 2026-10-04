//! 用户登记的 RSS 2.0 / Atom 订阅源 Provider。
//!
//! Intent lock（不扩边）：
//! - 只请求用户在来源设置中登记的订阅源地址；**绝不**请求条目 `link` 指向的
//!   网页，也不做任意网页爬取。条目的 `guid`/`id`/`link` 只用于推导稳定条目
//!   身份，不参与任何网络请求。
//! - 只消费订阅源自带且经过清洗的标题、发布时间、摘要/正文。脚本、iframe、
//!   外链图片和其它任意 HTML 行为在落库前统一剥离为纯文本段落。
//! - 私有订阅源地址可能携带 query token：这类地址在登记时就被拒绝（当前没有
//!   受控的 typed credential 路径可以承载 Feed query 凭据），因此订阅源地址
//!   不会因为凭据而变成敏感值；错误信息一律使用固定安全文案。
//! - 出站请求遵守仓库现有的 HTTPS、Host、DNS、端口、重定向与响应大小策略，
//!   不新增内网/回环/单标签主机例外。
//!
//! 网络访问收在 [`FeedSource`] 端口后面：生产实现是 [`FeedClient`]，测试可以
//! 注入进程内 fake，因此解析、候选与消费语义无需真实网络即可验证。

use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use quick_xml::events::{BytesStart, Event};
use sha2::{Digest, Sha256};

use haven_application::services::ports::{
    RemoteAcquiredFile, RemoteAcquisitionPort, RemoteByteRange, RemoteSessionBody,
    RemoteSessionPort,
};
use haven_application::services::search_source::SearchSourceParticipant;
use haven_application::services::source_import::{
    FEED_SOURCE_KEY, MAX_FEED_ENTRY_KEY_BYTES, RemoteContentRef, SourceCatalogEntry,
    SourceCatalogProvider, feed_candidate_handle, feed_entry_digest, feed_remote_id,
    is_feed_entry_digest, split_feed_remote_id,
};
use haven_application::services::source_registry::{
    CUSTOM_FEED_SOURCE_PREFIX, SourceRegistryService,
};
use haven_application::wire::{ContentCategory, MediaTypeDto, QueryCategory, WorkCardDto};
use haven_common::network::{HttpUrlPolicy, parse_http_url};
use haven_common::{AppError, ErrorKind};

use crate::http_security::{pin_client_builder, resolve_public_http_target};
use crate::online_sources::{
    bounded_session_body, html_to_plain_paragraphs, safe_article_html, write_bytes_to,
};

/// 订阅文章的受控 MIME。在线会话与离线快照都使用同一个清洗后的 HTML。
pub const FEED_ARTICLE_MIME: &str = "text/html; charset=utf-8";

/// 重定向跟随上限（与仓库其它固定来源一致）。
const MAX_REDIRECTS: usize = 3;
/// 单次订阅源响应大小上限。
const MAX_FEED_BYTES: usize = 4 * 1024 * 1024;
/// 清洗后文章 HTML 的大小上限。
///
/// 由渲染生产者 [`render_entry_html`] 统一保证，因此搜索候选、目录详情、在线
/// 会话与离线快照不会各有一套口径：超限条目在「能否阅读」的判定里就是不可读，
/// 而不是先被投影成可读、到在线阅读或下载时才失败。
const MAX_FEED_ARTICLE_BYTES: usize = 4 * 1024 * 1024;
/// 单次解析的条目数上限。超过上限时明确失败，而不是静默返回一个前缀。
const MAX_FEED_ENTRIES: usize = 500;
const MAX_ENTRY_TITLE_CHARS: usize = 300;
const REQUEST_TIMEOUT: Duration = Duration::from_secs(10);
const CONNECT_TIMEOUT: Duration = Duration::from_secs(3);
const USER_AGENT: &str = "Haven/0.1.0 (offline content importer)";

/// 有界获取一个用户登记的订阅源正文。
///
/// 实现方负责 URL 策略、DNS 解析与固定、重定向校验、超时和响应大小上限；
/// 调用方只看到一段已经限制过大小的文本。
#[async_trait]
pub trait FeedSource: Send + Sync {
    async fn fetch(&self, url: &str) -> Result<String, AppError>;
}

/// 生产实现：单个有界 HTTP 客户端，逐跳重新校验地址与 DNS。
#[derive(Clone)]
pub struct FeedClient;

impl FeedClient {
    pub fn new() -> Result<Self, AppError> {
        Ok(Self)
    }
}

#[async_trait]
impl FeedSource for FeedClient {
    async fn fetch(&self, url: &str) -> Result<String, AppError> {
        let mut current = validate_feed_request_url(url)?.into_url();
        for _ in 0..=MAX_REDIRECTS {
            let target =
                resolve_public_http_target(current.as_str(), HttpUrlPolicy::SourceEndpoint)
                    .await
                    .map_err(|_| security_denied("订阅源地址解析不安全"))?;
            let builder = reqwest::Client::builder()
                .connect_timeout(CONNECT_TIMEOUT)
                .timeout(REQUEST_TIMEOUT)
                .user_agent(USER_AGENT)
                // Keep the client predictable on Windows while preserving
                // HTTPS certificate validation. Redirects are followed
                // explicitly so every hop is revalidated.
                .http1_only()
                .redirect(reqwest::redirect::Policy::none());
            let client = pin_client_builder(builder, &target)
                .build()
                .map_err(|_| internal_error("订阅源客户端初始化失败"))?;
            let response = client
                .get(target.url.clone())
                .header(
                    reqwest::header::ACCEPT,
                    "application/rss+xml,application/atom+xml,application/xml,text/xml;q=0.9,*/*;q=0.1",
                )
                .send()
                .await
                .map_err(|_| feed_unavailable("订阅源暂时不可达"))?;
            if response.status().is_redirection() {
                let location = response
                    .headers()
                    .get(reqwest::header::LOCATION)
                    .and_then(|value| value.to_str().ok())
                    .ok_or_else(|| feed_unavailable("订阅源重定向地址无效"))?;
                current = next_feed_redirect(&current, location)?;
                continue;
            }
            if !response.status().is_success() {
                return Err(feed_unavailable("订阅源返回异常状态"));
            }
            if response
                .content_length()
                .is_some_and(|length| length > MAX_FEED_BYTES as u64)
            {
                return Err(feed_unavailable("订阅源响应超出大小上限"));
            }
            let mut body = Vec::with_capacity(64 * 1024);
            let mut response = response;
            while let Some(chunk) = response
                .chunk()
                .await
                .map_err(|_| feed_unavailable("订阅源响应读取中断"))?
            {
                if body.len().saturating_add(chunk.len()) > MAX_FEED_BYTES {
                    return Err(feed_unavailable("订阅源响应超出大小上限"));
                }
                body.extend_from_slice(&chunk);
            }
            return String::from_utf8(body).map_err(|_| feed_unavailable("订阅源编码无法读取"));
        }
        Err(feed_unavailable("订阅源重定向次数过多"))
    }
}

/// 订阅源地址策略：在共享的 `SourceEndpoint` 策略之上强制 HTTPS，并**拒绝
/// query 串**。
///
/// 共享策略已经拒绝 userinfo、fragment、私网/回环/单标签主机与非白名单端口。
/// 这里额外做的两条都来自同一个现实：私有 Feed 常把访问令牌放在 URL query 里，
/// 而当前没有受控的 typed credential 路径可以安全承载它。与其把凭据以 URL 形式
/// 留在设置里、再让它渗进日志/错误/身份，不如 fail closed 地拒绝这类端点。
fn validate_feed_request_url(raw: &str) -> Result<haven_common::network::SafeHttpUrl, AppError> {
    let parsed = parse_http_url(raw, HttpUrlPolicy::SourceEndpoint)
        .map_err(|_| security_denied("订阅源地址不安全"))?;
    if !parsed.as_url().scheme().eq_ignore_ascii_case("https") {
        return Err(security_denied("订阅源地址必须使用 HTTPS"));
    }
    if parsed.as_url().query().is_some() {
        return Err(security_denied("订阅源地址不能包含查询串"));
    }
    Ok(parsed)
}

/// 解析一次重定向目标：必须停留在登记地址的同一 host/port 上并重新过策略。
fn next_feed_redirect(current: &reqwest::Url, location: &str) -> Result<reqwest::Url, AppError> {
    let next = current
        .join(location)
        .map_err(|_| security_denied("订阅源重定向地址不安全"))?;
    if next.host_str() != current.host_str()
        || next.port_or_known_default() != current.port_or_known_default()
    {
        return Err(security_denied("订阅源重定向目标与登记地址不一致"));
    }
    validate_feed_request_url(next.as_str())?;
    Ok(next)
}

// ---------- 订阅源解析（RSS 2.0 / Atom，有界） ----------

/// 一条订阅条目。只保留 Feed 自身给出的字段。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FeedEntry {
    /// 稳定条目身份原文：`guid`/`id` 优先，其次 `link`，最后标题+发布时间派生。
    ///
    /// 它**只在后端内存中**存在；进入候选句柄与持久化远端身份前一定会被
    /// [`feed_entry_digest`] 压缩成固定长度摘要。
    pub key: String,
    pub title: String,
    /// 发布时间原文（Feed 给出的形式；只用于展示与年份推导）。
    pub published: Option<String>,
    /// Feed 自带正文（原始标记，消费前仍需清洗）。
    pub content: Option<String>,
    /// Feed 自带摘要（已转为纯文本）。
    pub summary: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum FeedFormat {
    Unknown,
    Rss,
    Atom,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Field {
    Key,
    Link,
    Title,
    Published,
    Summary,
    Content,
}

#[derive(Default)]
struct EntryBuilder {
    key: Option<String>,
    link: Option<String>,
    title: Option<String>,
    published: Option<String>,
    summary: Option<String>,
    content: Option<String>,
    /// 当前正在收集文本的字段及其元素名。
    field: Option<(Field, String)>,
    text: String,
}

impl EntryBuilder {
    fn assign(&mut self, field: Field, value: String) {
        let value = value.trim().to_owned();
        if value.is_empty() {
            return;
        }
        let slot = match field {
            Field::Key => &mut self.key,
            Field::Link => &mut self.link,
            Field::Title => &mut self.title,
            Field::Published => &mut self.published,
            Field::Summary => &mut self.summary,
            Field::Content => &mut self.content,
        };
        // 同一个元素重复出现时保留第一次观察到的值，保持身份稳定。
        if slot.is_none() {
            *slot = Some(value);
        }
    }

    fn finish(mut self) -> Option<FeedEntry> {
        if let Some((field, _)) = self.field.take() {
            let text = std::mem::take(&mut self.text);
            self.assign(field, text);
        }
        let title = self
            .title
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(|value| truncate_chars(value, MAX_ENTRY_TITLE_CHARS));
        let content = self.content;
        let summary = self
            .summary
            .as_deref()
            .map(plain_text)
            .filter(|value| !value.is_empty());
        // 既没有标题也没有正文的条目无法被用户识别或阅读，直接跳过。
        if title.is_none() && content.is_none() && summary.is_none() {
            return None;
        }
        let key = self
            .key
            .as_deref()
            .map(str::trim)
            .filter(|value| acceptable_entry_key(value))
            .map(str::to_owned)
            .or_else(|| {
                self.link
                    .as_deref()
                    .map(str::trim)
                    .filter(|value| acceptable_entry_key(value))
                    .map(str::to_owned)
            })
            .unwrap_or_else(|| derived_key(title.as_deref(), self.published.as_deref()));
        Some(FeedEntry {
            key,
            title: title.unwrap_or_else(|| "（无标题）".to_owned()),
            published: self.published,
            content,
            summary,
        })
    }
}

/// 有界解析 RSS 2.0 或 Atom 文档。
///
/// 明确失败而不是静默截断：无法识别的格式、超过条目上限、畸形 XML，以及在元素
/// 仍处于未闭合状态时就结束的截断响应，都返回稳定错误；调用方不会把「只看到
/// 前缀」当成完整结果。
pub fn parse_feed(xml: &str) -> Result<Vec<FeedEntry>, AppError> {
    let mut reader = quick_xml::Reader::from_str(xml);
    let mut format = FeedFormat::Unknown;
    let mut entries: Vec<FeedEntry> = Vec::new();
    let mut builder: Option<EntryBuilder> = None;
    let mut raw_entry_count = 0usize;
    // 仍未闭合的元素个数。响应会在任意元素中途结束（连接中断、代理截流），
    // 此时读取器只会把已经看完的片段交出来；这份计数让「解析成功」只可能
    // 发生在结构完整的文档上，而不是把前缀当成完整结果。
    let mut open_depth = 0usize;

    loop {
        match reader.read_event() {
            Err(_) => return Err(feed_unavailable("订阅源内容无法解析")),
            Ok(Event::Eof) => {
                if open_depth != 0 {
                    return Err(feed_unavailable("订阅源内容不完整"));
                }
                break;
            }
            Ok(Event::Start(event)) => {
                open_depth += 1;
                let name = element_name(&event);
                match name.as_str() {
                    "rss" if builder.is_none() && format == FeedFormat::Unknown => {
                        format = FeedFormat::Rss;
                    }
                    "feed" if builder.is_none() && format == FeedFormat::Unknown => {
                        format = FeedFormat::Atom;
                    }
                    "item" | "entry" if format != FeedFormat::Unknown => {
                        if let Some(previous) = builder.take()
                            && let Some(entry) = previous.finish()
                        {
                            push_unique(&mut entries, entry);
                        }
                        raw_entry_count += 1;
                        if raw_entry_count > MAX_FEED_ENTRIES {
                            return Err(feed_unavailable("订阅源条目数超出安全上限"));
                        }
                        builder = Some(EntryBuilder::default());
                    }
                    _ => {
                        if let Some(entry) = builder.as_mut() {
                            if entry.field.is_none()
                                && let Some(field) = classify_field(&name, format)
                            {
                                entry.field = Some((field, name.clone()));
                                entry.text.clear();
                            }
                            if name == "link"
                                && let Some(href) = attribute(&event, "href")
                            {
                                entry.assign(Field::Link, href);
                            }
                        }
                    }
                }
            }
            Ok(Event::Empty(event)) => {
                if let Some(entry) = builder.as_mut()
                    && element_name(&event) == "link"
                    && let Some(href) = attribute(&event, "href")
                {
                    entry.assign(Field::Link, href);
                }
            }
            Ok(Event::Text(event)) => {
                if let Some(entry) = builder.as_mut()
                    && entry.field.is_some()
                    && let Some(decoded) = crate::unescape_xml_text(&event)
                {
                    entry.text.push_str(&decoded);
                }
            }
            Ok(Event::GeneralRef(reference)) => {
                if let Some(entry) = builder.as_mut()
                    && entry.field.is_some()
                    && let Some(decoded) = crate::unescape_xml_reference(&reference)
                {
                    entry.text.push_str(&decoded);
                }
            }
            Ok(Event::CData(event)) => {
                let raw = event.into_inner();
                if let Some(entry) = builder.as_mut()
                    && entry.field.is_some()
                {
                    entry.text.push_str(&String::from_utf8_lossy(raw.as_ref()));
                }
            }
            Ok(Event::End(event)) => {
                open_depth = open_depth.saturating_sub(1);
                let name = local_name(event.name().as_ref());
                if (name == "item" || name == "entry")
                    && let Some(previous) = builder.take()
                    && let Some(parsed) = previous.finish()
                {
                    push_unique(&mut entries, parsed);
                    continue;
                }
                if let Some(entry) = builder.as_mut()
                    && let Some((field, field_name)) = entry.field.clone()
                    && name == field_name
                {
                    let text = std::mem::take(&mut entry.text);
                    entry.field = None;
                    entry.assign(field, text);
                }
            }
            Ok(_) => {}
        }
    }

    if format == FeedFormat::Unknown {
        return Err(feed_unavailable("订阅源格式无法识别"));
    }
    Ok(entries)
}

/// 追加条目并保持身份唯一。
///
/// 同一个 `guid`/`id` 在订阅源里重复出现时只保留第一次观察到的条目：这样搜索
/// 不会给出两张指向同一身份的卡片，导入也始终命中同一作品。
fn push_unique(entries: &mut Vec<FeedEntry>, entry: FeedEntry) {
    if !entries.iter().any(|existing| existing.key == entry.key) {
        entries.push(entry);
    }
}

/// 订阅源格式 → 字段分类。`format` 决定 RSS 与 Atom 的同名元素归属。
fn classify_field(name: &str, format: FeedFormat) -> Option<Field> {
    match name {
        "title" => Some(Field::Title),
        "guid" | "id" => Some(Field::Key),
        "pubDate" | "pubdate" | "published" | "updated" | "date" | "dc:date" => {
            Some(Field::Published)
        }
        "description" | "summary" => Some(Field::Summary),
        "encoded" | "content" => Some(Field::Content),
        "link" if format == FeedFormat::Rss => Some(Field::Link),
        _ => None,
    }
}

fn element_name(event: &BytesStart<'_>) -> String {
    local_name(event.name().as_ref())
}

fn local_name(raw: &[u8]) -> String {
    String::from_utf8_lossy(raw)
        .rsplit(':')
        .next()
        .unwrap_or("")
        .to_owned()
}

fn attribute(event: &BytesStart<'_>, key: &str) -> Option<String> {
    event
        .attributes()
        .flatten()
        .find(|attribute| local_name(attribute.key.as_ref()).eq_ignore_ascii_case(key))
        .and_then(|attribute| {
            attribute
                .normalized_value(quick_xml::XmlVersion::Implicit1_0)
                .ok()
        })
        .map(|value| value.into_owned())
        .filter(|value| !value.trim().is_empty())
}

/// Feed 身份原文是否可以参与身份推导。
///
/// `guid`/`link` 可能是很长的 URL：这里只拒绝明显异常的文本（空、超长、控制
/// 字符）。真正保证"URL/query token 不出现在候选句柄或持久化身份"的是随后的
/// [`feed_entry_digest`]，而不是这段长度限制。
fn acceptable_entry_key(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= MAX_FEED_ENTRY_KEY_BYTES
        && !value.chars().any(|ch| ch == '\0' || ch.is_control())
}

/// 缺少 `guid`/`id`/`link` 时的派生身份：标题 + 发布时间的 SHA-256 前缀。
/// 同一 Feed 内重复标题、缺少时间的条目会得到相同身份，这是有意的——它们对
/// 用户而言也不可区分，而且比随机 ID 更稳定。
fn derived_key(title: Option<&str>, published: Option<&str>) -> String {
    let mut hasher = Sha256::new();
    hasher.update(title.unwrap_or("").as_bytes());
    hasher.update([0x1f]);
    hasher.update(published.unwrap_or("").as_bytes());
    let digest = hasher.finalize();
    let mut key = String::with_capacity(34);
    key.push_str("derived:");
    for byte in digest.iter().take(16) {
        key.push_str(&crate::lower_hex(&[*byte]));
    }
    key
}

fn truncate_chars(value: &str, max: usize) -> String {
    if value.chars().count() <= max {
        return value.to_owned();
    }
    value.chars().take(max).collect()
}

/// 把一段 Feed 文本转成单行纯文本（用于卡片与作品描述）。
fn plain_text(raw: &str) -> String {
    html_to_plain_paragraphs(raw)
        .join(" ")
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

/// 从发布时间中取年份。
///
/// RSS 2.0 常用 RFC 822 形式（`Wed, 01 Oct 2026 08:00:00 GMT`），年份不在开头；
/// Atom 用 ISO 8601（`2026-10-03T10:00:00Z`），年份在最前。两者都取「第一个恰好
/// 四位数字的片段」，因此 `01`（日）、`08:00:00`（时间）与 `+0000`（时区偏移）
/// 都不会被误当成日期。
fn published_year(published: &str) -> Option<i32> {
    published
        .split(|ch: char| !ch.is_ascii_digit())
        .find(|token| token.len() == 4)
        .and_then(|token| token.parse::<i32>().ok())
        .filter(|year| *year > 0)
}

/// 条目清洗后真正可读的段落。
///
/// 这是「条目有哪些可读内容」的唯一判定口径：它只喂给渲染生产者
/// [`render_entry_html`]，由后者再向搜索、详情与正文消费给出统一的结论，因此
/// 不会出现三套不一致的可读性标准。正文（`content`）清洗后为空时才回退到摘要；
/// 两者都清洗为空即不可读——例如标题存在、但 `content:encoded` 只有
/// 脚本/iframe/外链图片。
///
/// 两个字段的信任级别不同，因此处理方式也不同：`content` 是 Feed 自带的原始
/// 标记，必须（且只）经过一次 [`html_to_plain_paragraphs`]；`summary` 在解析
/// 阶段已经由 [`plain_text`] 转成纯文本，再当 HTML 洗一遍会把摘要里字面量书写
/// 的 `<i>`/`<script>` 当成标签剥掉，用户看到的是被吞掉一部分的摘要。摘要因此
/// 直接作为文本段落进入 [`safe_article_html`]，由它统一转义。
fn entry_paragraphs(entry: &FeedEntry) -> Vec<String> {
    let mut paragraphs = entry
        .content
        .as_deref()
        .map(html_to_plain_paragraphs)
        .unwrap_or_default();
    if paragraphs.is_empty()
        && let Some(summary) = entry.summary.as_deref()
    {
        paragraphs.push(summary.to_owned());
    }
    paragraphs
}

/// 条目是否带有清洗后可读的正文。
///
/// 判定的唯一口径是渲染生产者本身：能渲染出不超过上限的清洗后 HTML 才算可读。
/// 搜索与详情都用它把「有标题但读不到内容」以及「清洗后超过大小上限」的条目
/// 挡在可导入作品之外，避免先给出候选/远端身份、到导入或阅读时才失败。
fn entry_has_readable_body(entry: &FeedEntry) -> bool {
    render_entry_html(entry).is_ok()
}

/// 渲染条目正文：只使用 Feed 自带内容，先清洗再包进受控 HTML 骨架。
///
/// 这是清洗后 HTML 的唯一生产者，同时负责大小上限：返回值一定不超过
/// [`MAX_FEED_ARTICLE_BYTES`]，超限条目在这里 fail closed，而不是把上限留给
/// 每个消费者各自判定。
fn render_entry_html(entry: &FeedEntry) -> Result<String, AppError> {
    let mut paragraphs = entry_paragraphs(entry);
    if paragraphs.is_empty() {
        return Err(feed_unavailable("订阅条目没有可读的正文内容"));
    }
    if let Some(published) = entry
        .published
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
    {
        paragraphs.insert(0, format!("发布时间：{published}"));
    }
    let html = safe_article_html(&entry.title, &paragraphs);
    if html.len() > MAX_FEED_ARTICLE_BYTES {
        return Err(feed_unavailable("订阅正文超出大小上限"));
    }
    Ok(html)
}

// ---------- Provider ----------

/// 订阅源 Provider：目录详情、在线正文会话与离线快照。
///
/// 三个能力都由同一个受限对象提供，避免出现两套互不一致的订阅源读取路径。
pub struct FeedProvider {
    registry: SourceRegistryService,
    source: Arc<dyn FeedSource>,
}

impl FeedProvider {
    pub fn new(registry: SourceRegistryService, source: Arc<dyn FeedSource>) -> Self {
        Self { registry, source }
    }

    /// 读取登记端点、解析订阅源，然后按条目摘要定位条目。
    ///
    /// 摘要不可逆，因此唯一的解析方式就是把这次有界抓取到的条目逐个摘要化再比对；
    /// 原始 `guid`/`link` 始终只在本次调用栈内存在。
    async fn load_entry(&self, source_id: &str, entry_digest: &str) -> Result<FeedEntry, AppError> {
        if !SourceRegistryService::is_feed_source_id(source_id) {
            return Err(invalid_argument("未知订阅来源"));
        }
        if !is_feed_entry_digest(entry_digest) {
            return Err(invalid_argument("订阅条目身份非法"));
        }
        // 订阅源已被删除或端点被清空时，已落库的远端身份仍然存在；这是「来源
        // 当前不可用」，不是调用方参数错误，因此返回可重试的固定来源错误。
        let endpoint = self
            .registry
            .endpoint(source_id)
            .await?
            .ok_or_else(|| feed_unavailable("该订阅源当前不可用"))?;
        let xml = self.source.fetch(&endpoint).await?;
        let entries = parse_feed(&xml)?;
        entries
            .into_iter()
            .find(|entry| feed_entry_digest(&entry.key) == entry_digest)
            .ok_or_else(|| feed_unavailable("订阅源中已找不到该条目"))
    }
}

#[async_trait]
impl SourceCatalogProvider for FeedProvider {
    async fn detail(
        &self,
        source_id: &str,
        _endpoint: &str,
        external_id: &str,
    ) -> Result<SourceCatalogEntry, AppError> {
        let entry = self.load_entry(source_id, external_id).await?;
        // 已落库的候选句柄在订阅源改版后可能指向一条「有标题但读不到正文」的
        // 条目：详情必须和在线会话/离线快照一样 fail closed，不能先返回可导入
        // 的远端身份、把失败推迟到导入阶段。
        if !entry_has_readable_body(&entry) {
            return Err(feed_unavailable("订阅条目没有可读的正文内容"));
        }
        Ok(SourceCatalogEntry {
            external_id: external_id.to_owned(),
            title: entry.title.clone(),
            year: entry.published.as_deref().and_then(published_year),
            type_name: Some("订阅文章".to_owned()),
            pic: None,
            episodes: Vec::new(),
            content: entry.summary.clone(),
            director: None,
            actor: None,
            local_file: None,
            media_type: Some(haven_domain::enums::MediaType::Article),
            remote: Some(RemoteContentRef {
                source_key: FEED_SOURCE_KEY.to_owned(),
                remote_id: feed_remote_id(source_id, external_id)?,
                media_type: haven_domain::enums::MediaType::Article,
                mime_type: Some(FEED_ARTICLE_MIME.to_owned()),
            }),
            comic_catalog: None,
        })
    }
}

#[async_trait]
impl RemoteSessionPort for FeedProvider {
    async fn read(
        &self,
        source_key: &str,
        remote_id: &str,
        range: Option<RemoteByteRange>,
    ) -> Result<RemoteSessionBody, AppError> {
        if source_key != FEED_SOURCE_KEY {
            return Err(invalid_argument("该来源不支持订阅正文会话"));
        }
        let (source_id, entry_digest) = split_feed_remote_id(remote_id)?;
        let entry = self.load_entry(&source_id, &entry_digest).await?;
        let html = render_entry_html(&entry)?;
        bounded_session_body(html.into_bytes(), range, FEED_ARTICLE_MIME)
    }
}

#[async_trait]
impl RemoteAcquisitionPort for FeedProvider {
    async fn acquire(
        &self,
        source_key: &str,
        remote_id: &str,
        destination: &std::path::Path,
    ) -> Result<RemoteAcquiredFile, AppError> {
        if source_key != FEED_SOURCE_KEY {
            return Err(invalid_argument("该来源不支持订阅离线获取"));
        }
        if destination.as_os_str().is_empty() {
            return Err(storage_error("订阅正文临时文件路径无效"));
        }
        let (source_id, entry_digest) = split_feed_remote_id(remote_id)?;
        let entry = self.load_entry(&source_id, &entry_digest).await?;
        // 大小上限由渲染生产者保证；这里不再重复判定，避免出现两份口径。
        let html = render_entry_html(&entry)?;
        let size = write_bytes_to(destination.to_path_buf(), html.into_bytes()).await?;
        Ok(RemoteAcquiredFile {
            size_bytes: size,
            mime: FEED_ARTICLE_MIME.to_owned(),
        })
    }
}

// ---------- 搜索参与者 ----------

/// 订阅源家族的搜索参与者：以 `custom_feed_` 前缀承接全部用户登记的订阅源。
///
/// 前缀比自定义 OPDS 的 `custom_` 更长，因此 `SearchSourceService` 的最长前缀
/// 路由会把订阅源交给本参与者，而不会落到 OPDS 参与者上。
pub struct FeedSearchParticipant {
    registry: SourceRegistryService,
    source: Arc<dyn FeedSource>,
}

impl FeedSearchParticipant {
    pub fn new(registry: SourceRegistryService, source: Arc<dyn FeedSource>) -> Self {
        Self { registry, source }
    }
}

#[async_trait]
impl SearchSourceParticipant for FeedSearchParticipant {
    fn source_id(&self) -> &str {
        // 家族前缀本身不是任何真实 sourceId（真实 ID 会带 12 位十六进制后缀），
        // 因此精确匹配分支永远不会命中，实际分发始终走前缀路由。
        CUSTOM_FEED_SOURCE_PREFIX
    }

    fn id_prefix(&self) -> Option<&str> {
        Some(CUSTOM_FEED_SOURCE_PREFIX)
    }

    fn supports_category(&self, category: Option<QueryCategory>) -> bool {
        matches!(
            category,
            None | Some(QueryCategory::All) | Some(QueryCategory::Periodical)
        )
    }

    async fn search(
        &self,
        query: &str,
        limit: u32,
        is_cancelled: &(dyn Fn() -> bool + Send + Sync),
    ) -> Result<Vec<WorkCardDto>, AppError> {
        self.search_for(CUSTOM_FEED_SOURCE_PREFIX, query, limit, is_cancelled)
            .await
    }

    async fn search_for(
        &self,
        dispatched_id: &str,
        query: &str,
        limit: u32,
        is_cancelled: &(dyn Fn() -> bool + Send + Sync),
    ) -> Result<Vec<WorkCardDto>, AppError> {
        if !SourceRegistryService::is_feed_source_id(dispatched_id) {
            return Ok(Vec::new());
        }
        let Some(endpoint) = self.registry.endpoint(dispatched_id).await? else {
            return Ok(Vec::new());
        };
        if is_cancelled() {
            return Ok(Vec::new());
        }
        let xml = self.source.fetch(&endpoint).await?;
        let entries = parse_feed(&xml)?;
        if is_cancelled() {
            return Ok(Vec::new());
        }
        let needle = query.trim().to_lowercase();
        Ok(entries
            .into_iter()
            // 有标题但清洗后没有可读正文的条目（例如正文只有脚本/外链资源）不
            // 是用户能阅读或导入的作品，先于匹配剔除，也不占用 limit 名额。
            .filter(entry_has_readable_body)
            .filter(|entry| entry_matches(entry, &needle))
            .take(limit as usize)
            .map(|entry| card(dispatched_id, &entry))
            .collect())
    }
}

fn entry_matches(entry: &FeedEntry, needle: &str) -> bool {
    if needle.is_empty() {
        return true;
    }
    entry.title.to_lowercase().contains(needle)
        || entry
            .summary
            .as_deref()
            .is_some_and(|summary| summary.to_lowercase().contains(needle))
}

/// 搜索卡片只携带 opaque 候选句柄：句柄里的条目身份是单向摘要，订阅源地址、
/// 条目 link、query token 与正文都不进 Wire。
fn card(source_id: &str, entry: &FeedEntry) -> WorkCardDto {
    WorkCardDto {
        work_id: feed_candidate_handle(source_id, &feed_entry_digest(&entry.key)),
        title: entry.title.clone(),
        original_title: None,
        description: entry.summary.clone(),
        categories: vec![ContentCategory::Periodical],
        available_media_types: vec![MediaTypeDto::Article],
        poster_uri: None,
        backdrop_uri: None,
        release_year: entry.published.as_deref().and_then(published_year),
        rating_value: None,
        rating_scale: None,
        favorite: false,
        progress: None,
        primary_action: None,
        external_ids: Vec::new(),
    }
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

/// 固定安全文案：绝不回显订阅源地址、query token 或远端响应片段。
fn feed_unavailable(message: &'static str) -> AppError {
    AppError::new("SOURCE_UNAVAILABLE", ErrorKind::Network, message, true)
}

fn storage_error(message: &'static str) -> AppError {
    AppError::new("SOURCE_UNAVAILABLE", ErrorKind::Storage, message, true)
}

fn internal_error(message: &'static str) -> AppError {
    AppError::new("INTERNAL_ERROR", ErrorKind::Internal, message, false)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::repos::SqliteRepositories;
    use haven_application::services::source_import::CONTENT_CANDIDATE_PREFIX;
    use std::sync::Mutex;

    const RSS_FIXTURE: &str = r#"<?xml version="1.0" encoding="UTF-8"?>
<rss version="2.0" xmlns:content="http://purl.org/rss/1.0/modules/content/">
  <channel>
    <title>示例订阅源</title>
    <link>https://feed.example.invalid/</link>
    <item>
      <title>第一篇 &amp; 摘要</title>
      <link>https://feed.example.invalid/posts/1</link>
      <guid isPermaLink="false">post-1</guid>
      <pubDate>Wed, 01 Oct 2026 08:00:00 GMT</pubDate>
      <description>&lt;p&gt;第一段&lt;/p&gt;&lt;p&gt;第二段&lt;/p&gt;</description>
      <content:encoded>&lt;p&gt;正文第一段&lt;/p&gt;&lt;script&gt;alert(1)&lt;/script&gt;&lt;img src="https://cdn.example.invalid/a.png"&gt;&lt;p&gt;正文第二段&lt;/p&gt;</content:encoded>
    </item>
    <item>
      <title>第二篇</title>
      <link>https://feed.example.invalid/posts/2</link>
      <pubDate>Thu, 02 Oct 2026 08:00:00 GMT</pubDate>
      <description>只有摘要</description>
    </item>
  </channel>
</rss>"#;

    const ATOM_FIXTURE: &str = r#"<?xml version="1.0" encoding="UTF-8"?>
<feed xmlns="http://www.w3.org/2005/Atom">
  <title>Atom 示例</title>
  <entry>
    <id>tag:feed.example.invalid,2026:1</id>
    <title>Atom 第一篇</title>
    <link rel="alternate" href="https://feed.example.invalid/atom/1"/>
    <published>2026-10-03T10:00:00Z</published>
    <summary>Atom 摘要</summary>
    <content type="html">&lt;p&gt;Atom 正文&lt;/p&gt;</content>
  </entry>
</feed>"#;

    /// 标题存在、但清洗后没有任何可读正文的条目：第一条只有一个标题，第二条
    /// 的正文只有脚本、iframe 与外链图片。第三条是纯摘要条目，用于证明可读的
    /// 摘要条目不会被误伤。
    const UNREADABLE_FEED_FIXTURE: &str = r#"<?xml version="1.0" encoding="UTF-8"?>
<rss version="2.0" xmlns:content="http://purl.org/rss/1.0/modules/content/">
  <channel>
    <title>含不可读条目的订阅源</title>
    <link>https://feed.example.invalid/</link>
    <item>
      <title>只有标题</title>
      <link>https://feed.example.invalid/posts/only-title</link>
      <guid isPermaLink="false">only-title</guid>
      <pubDate>Wed, 01 Oct 2026 08:00:00 GMT</pubDate>
    </item>
    <item>
      <title>只有脚本与外链</title>
      <link>https://feed.example.invalid/posts/scripts</link>
      <guid isPermaLink="false">scripts-only</guid>
      <content:encoded>&lt;script&gt;alert(1)&lt;/script&gt;&lt;iframe src="https://cdn.example.invalid/frame"&gt;&lt;/iframe&gt;&lt;img src="https://cdn.example.invalid/a.png"&gt;</content:encoded>
    </item>
    <item>
      <title>可读摘要条目</title>
      <link>https://feed.example.invalid/posts/summary</link>
      <guid isPermaLink="false">readable-summary</guid>
      <description>&lt;p&gt;只有摘要但可读&lt;/p&gt;</description>
    </item>
  </channel>
</rss>"#;

    /// 摘要与正文里**字面量**书写的尖括号：XML 里再转义一层
    /// （`&amp;lt;i&amp;gt;`），解析后得到的是文本 `<i>`，而不是待清洗的标记。
    ///
    /// 第一条只有摘要，第二条的 `content:encoded` 清洗后为空、必须回退到同一份
    /// 纯文本摘要。两条都必须让字面量标签以文本形式显示。
    const LITERAL_ANGLE_BRACKET_FEED_FIXTURE: &str = r#"<?xml version="1.0" encoding="UTF-8"?>
<rss version="2.0" xmlns:content="http://purl.org/rss/1.0/modules/content/">
  <channel>
    <title>含字面量尖括号的订阅源</title>
    <link>https://feed.example.invalid/</link>
    <item>
      <title>摘要里有字面量标签</title>
      <link>https://feed.example.invalid/posts/literal-summary</link>
      <guid isPermaLink="false">literal-summary</guid>
      <description>&amp;lt;i&amp;gt;强调文本&amp;lt;/i&amp;gt; 与 &amp;lt;script&amp;gt;alert(1)&amp;lt;/script&amp;gt;</description>
    </item>
    <item>
      <title>正文清洗为空后回退摘要</title>
      <link>https://feed.example.invalid/posts/literal-fallback</link>
      <guid isPermaLink="false">literal-fallback</guid>
      <description>&amp;lt;b&amp;gt;字面量加粗&amp;lt;/b&amp;gt;</description>
      <content:encoded>&lt;script&gt;alert(1)&lt;/script&gt;&lt;img src="https://cdn.example.invalid/a.png"&gt;</content:encoded>
    </item>
  </channel>
</rss>"#;

    struct StaticFeedSource {
        body: String,
        requested: Mutex<Vec<String>>,
    }

    impl StaticFeedSource {
        fn new(body: &str) -> Arc<Self> {
            Arc::new(Self {
                body: body.to_owned(),
                requested: Mutex::new(Vec::new()),
            })
        }

        fn requested(&self) -> Vec<String> {
            self.requested
                .lock()
                .unwrap_or_else(|error| error.into_inner())
                .clone()
        }
    }

    #[async_trait]
    impl FeedSource for StaticFeedSource {
        async fn fetch(&self, url: &str) -> Result<String, AppError> {
            self.requested
                .lock()
                .unwrap_or_else(|error| error.into_inner())
                .push(url.to_owned());
            Ok(self.body.clone())
        }
    }

    fn registry() -> SourceRegistryService {
        let db = Arc::new(crate::Db::open_in_memory().unwrap());
        let repos = Arc::new(SqliteRepositories::new(db));
        SourceRegistryService::new(repos)
    }

    async fn feed_source_id(registry: &SourceRegistryService) -> String {
        registry
            .add_feed_source("示例订阅", "https://feed.example.invalid/rss.xml")
            .await
            .unwrap()
            .source_id
    }

    fn entry(entries: &[FeedEntry], key: &str) -> FeedEntry {
        entries
            .iter()
            .find(|entry| entry.key == key)
            .cloned()
            .unwrap_or_else(|| panic!("缺少条目 {key}"))
    }

    #[test]
    fn rss_entries_carry_stable_identity_title_date_and_body() {
        let entries = parse_feed(RSS_FIXTURE).unwrap();
        assert_eq!(entries.len(), 2);

        let first = entry(&entries, "post-1");
        assert_eq!(first.title, "第一篇 & 摘要");
        assert_eq!(
            first.published.as_deref(),
            Some("Wed, 01 Oct 2026 08:00:00 GMT")
        );
        assert!(first.content.is_some(), "content:encoded 必须被读取");
        assert_eq!(first.summary.as_deref(), Some("第一段 第二段"));

        // 缺少 guid 的条目退回 link 作为稳定身份。
        let second = entry(&entries, "https://feed.example.invalid/posts/2");
        assert_eq!(second.title, "第二篇");
        assert_eq!(second.summary.as_deref(), Some("只有摘要"));
    }

    #[test]
    fn atom_entries_use_id_published_and_content() {
        let entries = parse_feed(ATOM_FIXTURE).unwrap();
        assert_eq!(entries.len(), 1);
        let only = entry(&entries, "tag:feed.example.invalid,2026:1");
        assert_eq!(only.title, "Atom 第一篇");
        assert_eq!(only.published.as_deref(), Some("2026-10-03T10:00:00Z"));
        assert_eq!(only.summary.as_deref(), Some("Atom 摘要"));
        assert!(
            only.content
                .as_deref()
                .is_some_and(|body| body.contains("Atom 正文"))
        );
        assert_eq!(published_year(&only.published.clone().unwrap()), Some(2026));
    }

    #[test]
    fn derived_identity_is_stable_when_the_feed_has_no_identifier() {
        let xml = r#"<rss version="2.0"><channel>
            <item><title>无标识条目</title><pubDate>2026-10-04</pubDate><description>正文</description></item>
            <item><title>无标识条目</title><pubDate>2026-10-04</pubDate><description>正文</description></item>
        </channel></rss>"#;
        let entries = parse_feed(xml).unwrap();
        assert_eq!(entries.len(), 1, "派生身份相同的条目应合并为一条");
        assert!(entries[0].key.starts_with("derived:"));

        // 同一份文档重复解析必须得到同一个身份（导入幂等的前提）。
        let again = parse_feed(xml).unwrap();
        assert_eq!(entries[0].key, again[0].key);
    }

    #[test]
    fn malformed_or_foreign_documents_fail_closed() {
        let broken =
            parse_feed("<rss><channel><item><title>标签不匹配</wrong></item></channel></rss>")
                .unwrap_err();
        assert_eq!(broken.code().as_str(), "SOURCE_UNAVAILABLE");

        let foreign = parse_feed("<html><body>不是订阅源</body></html>").unwrap_err();
        assert_eq!(foreign.code().as_str(), "SOURCE_UNAVAILABLE");
    }

    #[test]
    fn oversized_feeds_error_instead_of_reporting_a_truncated_prefix() {
        let mut xml = String::from("<rss version=\"2.0\"><channel>");
        for index in 0..=MAX_FEED_ENTRIES {
            xml.push_str(&format!(
                "<item><guid>k{index}</guid><title>t{index}</title><description>d</description></item>"
            ));
        }
        xml.push_str("</channel></rss>");
        let error = parse_feed(&xml).unwrap_err();
        assert_eq!(error.code().as_str(), "SOURCE_UNAVAILABLE");
        assert!(error.user_message().contains("条目数"));
    }

    /// 截断的响应：最后一条 `<item>` 还没闭合就到了 EOF。读取器此时已经把第一条
    /// 完整条目交了出来；如果不检查结构完整性，调用方会拿到一个「成功但少一条」
    /// 的前缀结果。
    #[test]
    fn truncated_item_at_eof_fails_instead_of_returning_the_complete_prefix() {
        // 同一份内容在正常闭合时确实能解析出条目——这恰恰是必须失败的原因：
        // 截断版本与它几乎一模一样，只是少了一条。
        let complete = r#"<rss version="2.0"><channel>
            <title>被截断的订阅源</title>
            <item>
              <guid>complete-1</guid>
              <title>完整条目</title>
              <description>完整正文</description>
            </item>
        </channel></rss>"#;
        let entries = parse_feed(complete).unwrap();
        assert_eq!(entries.len(), 1);
        assert_eq!(entry(&entries, "complete-1").title, "完整条目");

        let truncated = r#"<rss version="2.0"><channel>
            <title>被截断的订阅源</title>
            <item>
              <guid>complete-1</guid>
              <title>完整条目</title>
              <description>完整正文</description>
            </item>
            <item>
              <title>半截条目</title>
              <description>正文还没写完"#;
        let error = parse_feed(truncated).unwrap_err();
        assert_eq!(error.code().as_str(), "SOURCE_UNAVAILABLE");
        assert!(error.retryable(), "截断属于可重试的来源错误");
    }

    /// 条目自身已经闭合，但 `channel`/`rss`（或 Atom 的 `feed`）在 EOF 时仍未
    /// 闭合：这是被截断的响应，而不是「恰好没有更多条目」的完整订阅源。
    #[test]
    fn feed_whose_root_is_unclosed_at_eof_fails_instead_of_returning_a_prefix() {
        let rss_prefix = r#"<rss version="2.0"><channel>
            <title>根元素未闭合</title>
            <item><guid>rss-1</guid><title>唯一条目</title><description>正文</description></item>"#;
        let atom_prefix = r#"<feed xmlns="http://www.w3.org/2005/Atom">
            <title>根元素未闭合</title>
            <entry><id>tag:feed.example.invalid,2026:1</id><title>唯一条目</title><summary>摘要</summary></entry>"#;

        // 两份内容补齐根元素后都能解析出唯一那条条目；截断版本若被当成完整结果，
        // 调用方会拿到一个「成功但只剩前缀」的列表。
        assert_eq!(
            parse_feed(&format!("{rss_prefix}</channel></rss>"))
                .unwrap()
                .len(),
            1
        );
        assert_eq!(
            parse_feed(&format!("{atom_prefix}</feed>")).unwrap().len(),
            1
        );

        for truncated in [rss_prefix, atom_prefix] {
            let error = parse_feed(truncated).unwrap_err();
            assert_eq!(
                error.code().as_str(),
                "SOURCE_UNAVAILABLE",
                "根元素未闭合的文档不得被当成完整订阅源"
            );
            assert!(error.retryable());
        }
    }

    /// 结构完整、只是没有任何条目的订阅源是合法的，不能被「EOF 时仍有未闭合
    /// 元素」的判定误伤。
    #[test]
    fn well_formed_feeds_without_entries_are_still_accepted() {
        for xml in [
            r#"<rss version="2.0"><channel><title>空订阅源</title></channel></rss>"#,
            r#"<feed xmlns="http://www.w3.org/2005/Atom"><title>空订阅源</title></feed>"#,
            // 自闭合的 `channel` 同样算已闭合。
            r#"<rss version="2.0"><channel/></rss>"#,
        ] {
            assert!(
                parse_feed(xml).unwrap().is_empty(),
                "合法的空订阅源必须仍然被接受: {xml}"
            );
        }
    }

    #[test]
    fn rendered_article_never_keeps_scripts_or_external_resources() {
        let entries = parse_feed(RSS_FIXTURE).unwrap();
        let first = entry(&entries, "post-1");
        let html = render_entry_html(&first).unwrap();

        assert!(html.contains("正文第一段"));
        assert!(html.contains("正文第二段"));
        assert!(!html.contains("<script"), "脚本标签与脚本内容都必须被剥离");
        assert!(!html.contains("alert(1)"));
        assert!(!html.contains("<img"));
        assert!(!html.contains("cdn.example.invalid"));
        assert!(!html.contains("href="), "外链必须被剥离");
        // 段落结构保留：两段正文各自成为独立的 <p>。
        assert_eq!(html.matches("<p>正文第一段</p>").count(), 1);
        assert_eq!(html.matches("<p>正文第二段</p>").count(), 1);
    }

    #[test]
    fn entries_without_readable_text_are_flagged_unreadable() {
        let entries = parse_feed(UNREADABLE_FEED_FIXTURE).unwrap();
        // 解析层仍然保留条目（身份推导不因此改变）；可读性只在搜索/详情边界判定。
        assert_eq!(entries.len(), 3);

        let title_only = entry(&entries, "only-title");
        assert_eq!(title_only.title, "只有标题");
        assert!(
            !entry_has_readable_body(&title_only),
            "只有标题的条目没有可读正文"
        );
        assert!(render_entry_html(&title_only).is_err());

        let scripts_only = entry(&entries, "scripts-only");
        assert!(
            scripts_only.content.is_some(),
            "脚本正文在解析层仍是原始标记"
        );
        assert!(
            !entry_has_readable_body(&scripts_only),
            "正文清洗后只剩脚本/外链时不算可读"
        );
        assert!(render_entry_html(&scripts_only).is_err());

        // 纯摘要条目必须保持可读：这是 summary-only 条目的正向保护。
        let summary_only = entry(&entries, "readable-summary");
        assert!(entry_has_readable_body(&summary_only));
        assert!(
            render_entry_html(&summary_only)
                .unwrap()
                .contains("只有摘要但可读")
        );
    }

    #[test]
    fn literal_angle_brackets_in_summaries_render_as_escaped_text() {
        let entries = parse_feed(LITERAL_ANGLE_BRACKET_FEED_FIXTURE).unwrap();

        // 第一条：只有摘要。摘要已经是纯文本——解析阶段就把 `&lt;i&gt;` 解成了
        // 字面量 `<i>`——把它再当 HTML 洗一遍就会当成标签剥掉。
        let summary_only = entry(&entries, "literal-summary");
        assert!(summary_only.content.is_none(), "该条目只有摘要");
        assert_eq!(
            summary_only.summary.as_deref(),
            Some("<i>强调文本</i> 与 <script>alert(1)</script>")
        );
        assert!(entry_has_readable_body(&summary_only));

        let html = render_entry_html(&summary_only).unwrap();
        assert!(
            html.contains("&lt;i&gt;强调文本&lt;/i&gt;"),
            "摘要里的字面量 <i> 必须转义后原样展示，而不是被当成标记剥掉: {html}"
        );
        assert!(
            html.contains("&lt;script&gt;alert(1)&lt;/script&gt;"),
            "摘要里的字面量 <script> 同样只是文本: {html}"
        );
        assert_eq!(html.matches("<p>").count(), 1, "纯文本摘要渲染为一段");
        assert!(!html.contains("<i>"), "字面量 <i> 不得变成真实标记");
        assert!(
            !html.contains("<script"),
            "字面量 <script> 不得变成可执行标记"
        );

        // 第二条：正文清洗后为空，必须回退到同一份纯文本摘要，而不是退化成
        // 「不可读」。这条同时证明正文仍然（且只）清洗一次。
        let fallback = entry(&entries, "literal-fallback");
        assert!(fallback.content.is_some(), "正文在解析层仍是原始标记");
        assert_eq!(fallback.summary.as_deref(), Some("<b>字面量加粗</b>"));
        assert!(entry_has_readable_body(&fallback));

        let html = render_entry_html(&fallback).unwrap();
        assert!(
            html.contains("&lt;b&gt;字面量加粗&lt;/b&gt;"),
            "回退到摘要时同样必须按文本转义: {html}"
        );
        assert!(!html.contains("<b>"), "字面量 <b> 不得变成真实标记");
        assert!(!html.contains("<script"), "被剥离的正文脚本不得回到正文里");
        assert!(!html.contains("cdn.example.invalid"), "外链必须被剥离");
    }

    #[test]
    fn feed_url_policy_rejects_credentials_fragments_plaintext_private_hosts_and_queries() {
        for rejected in [
            "https://user:secret@feed.example.invalid/rss",
            "https://feed.example.invalid/rss#fragment",
            "http://feed.example.invalid/rss",
            "https://127.0.0.1/rss",
            "https://10.0.0.5/rss",
            "https://192.168.1.10/rss",
            "https://[::1]/rss",
            "https://localhost/rss",
            "https://feed/rss",
            "https://feed.internal/rss",
            "https://feed.example.invalid:12345/rss",
            // 当前没有受控的 typed credential 路径可以承载 Feed query 凭据，
            // 因此所有 query 串一律 fail closed。
            "https://feed.example.invalid/rss?a=b",
            "https://feed.example.invalid/rss?token=secret-token",
            "https://feed.example.invalid/rss?",
        ] {
            assert!(
                validate_feed_request_url(rejected).is_err(),
                "不安全的订阅源地址被接受: {rejected}"
            );
        }
        assert!(validate_feed_request_url("https://feed.example.invalid/rss").is_ok());
        assert!(validate_feed_request_url("https://feed.example.invalid/nested/rss.xml").is_ok());
    }

    #[test]
    fn feed_identity_is_a_fixed_size_digest_that_never_carries_the_source_url() {
        // 私有 Feed 常用带 token 的 guid/link URL；它们绝不能出现在候选句柄、
        // Wire 或持久化身份里。
        let token_url = "https://reader.example.invalid/article/1?token=super-secret-token";
        let xml = format!(
            r#"<rss version="2.0"><channel>
            <item><title>私密条目</title><guid isPermaLink="false">{token_url}</guid><description>正文</description></item>
            </channel></rss>"#
        );
        let entries = parse_feed(&xml).unwrap();
        assert_eq!(entries[0].key, token_url, "原始 guid 仍只保留在后端内存");

        let digest = feed_entry_digest(&entries[0].key);
        assert!(!digest.contains("token"));
        assert_eq!(
            digest.len(),
            haven_application::services::source_import::FEED_ENTRY_DIGEST_HEX_LEN
        );

        let source_id = "custom_feed_0123456789ab";
        let handle = feed_candidate_handle(source_id, &digest);
        let remote_id = feed_remote_id(source_id, &digest).unwrap();
        for value in [handle.as_str(), remote_id.as_str(), digest.as_str()] {
            assert!(
                !value.contains("token"),
                "身份不得携带 query token: {value}"
            );
            assert!(!value.contains("super-secret-token"));
            assert!(!value.contains("reader.example.invalid"));
            assert!(!value.contains("https://"));
            assert!(!value.contains('/'));
        }
        // 同一输入的摘要恒定：去重与幂等语义因此保持不变。
        assert_eq!(digest, feed_entry_digest(&entries[0].key));
        assert_ne!(digest, feed_entry_digest("post-1"));
    }

    #[test]
    fn published_year_reads_both_rfc822_and_iso_forms() {
        assert_eq!(published_year("Wed, 01 Oct 2026 08:00:00 GMT"), Some(2026));
        assert_eq!(
            published_year("Thu, 02 Oct 2026 08:00:00 +0800"),
            Some(2026)
        );
        assert_eq!(published_year("2026"), Some(2026));
        assert_eq!(published_year("2026-10-03T10:00:00Z"), Some(2026));
        assert_eq!(published_year("2026-10-03"), Some(2026));
        assert_eq!(published_year("01 Oct 1999 23:59:59 GMT"), Some(1999));
        assert_eq!(published_year("最近更新"), None);
        assert_eq!(published_year(""), None);
    }

    #[test]
    fn rss_pub_date_year_reaches_the_search_card_and_detail() {
        let entries = parse_feed(RSS_FIXTURE).unwrap();
        let first = entry(&entries, "post-1");
        assert_eq!(
            published_year(first.published.as_deref().unwrap()),
            Some(2026),
            "RSS pubDate 是 RFC 822，年份不在开头"
        );
        let search_card = card("custom_feed_0123456789ab", &first);
        assert_eq!(search_card.release_year, Some(2026));
    }

    #[test]
    fn redirects_must_stay_on_the_registered_authority() {
        let origin = reqwest::Url::parse("https://feed.example.invalid/rss").unwrap();
        assert!(next_feed_redirect(&origin, "/rss-v2").is_ok());
        assert!(next_feed_redirect(&origin, "https://feed.example.invalid/other").is_ok());
        for rejected in [
            "https://evil.example.invalid/rss",
            "http://feed.example.invalid/rss",
            "https://user:secret@feed.example.invalid/rss",
            // 重定向也不能把 query 串带回来。
            "/rss-v2?token=secret",
        ] {
            assert!(
                next_feed_redirect(&origin, rejected).is_err(),
                "越界的重定向被接受: {rejected}"
            );
        }
    }

    #[test]
    fn remote_id_round_trips_and_rejects_foreign_shapes() {
        let source_id = "custom_feed_0123456789ab";
        let digest = feed_entry_digest("post-1");
        let remote_id = feed_remote_id(source_id, &digest).unwrap();
        let (parsed_source, parsed_digest) = split_feed_remote_id(&remote_id).unwrap();
        assert_eq!(parsed_source, source_id);
        assert_eq!(parsed_digest, digest);
        assert_eq!(
            feed_candidate_handle(source_id, &digest),
            format!("content-candidate-{source_id}-{digest}")
        );

        for rejected in [
            "mangadex:post-1",
            "custom_feed_0123456789ab",
            "custom_feed_ZZZZZZZZZZZZ:post-1",
            "feed:post-1",
            // 摘要形状必须严格：原始 guid/URL 不再被接受。
            "custom_feed_0123456789ab:post-1",
            "custom_feed_0123456789ab:https%3A%2F%2Ffeed.example.invalid%2F1",
            "custom_feed_0123456789ab:0123456789ABCDEF0123456789abcdef",
        ] {
            assert!(
                split_feed_remote_id(rejected).is_err(),
                "非法远端身份被接受: {rejected}"
            );
        }
        assert!(feed_remote_id("custom_0123456789ab", &digest).is_err());
        assert!(feed_remote_id(source_id, "post-1").is_err());
    }

    #[tokio::test]
    async fn search_participant_returns_opaque_cards_only_for_the_feed_family() {
        let source = StaticFeedSource::new(RSS_FIXTURE);
        let registry = registry();
        let source_id = feed_source_id(&registry).await;
        let participant = FeedSearchParticipant::new(registry, source.clone());

        let not_cancelled = || false;
        let cards = participant
            .search_for(&source_id, "第一篇", 10, &not_cancelled)
            .await
            .unwrap();
        assert_eq!(cards.len(), 1);
        assert_eq!(cards[0].title, "第一篇 & 摘要");
        assert_eq!(cards[0].categories, vec![ContentCategory::Periodical]);
        assert_eq!(cards[0].available_media_types, vec![MediaTypeDto::Article]);
        assert!(cards[0].work_id.starts_with(CONTENT_CANDIDATE_PREFIX));
        assert!(
            !cards[0].work_id.contains("feed.example.invalid"),
            "候选句柄不得携带订阅源地址"
        );

        // 其它来源前缀不会被订阅源参与者接管。
        let foreign = participant
            .search_for("custom_0123456789ab", "第一篇", 10, &not_cancelled)
            .await
            .unwrap();
        assert!(foreign.is_empty());
        assert_eq!(
            source.requested(),
            vec!["https://feed.example.invalid/rss.xml".to_owned()],
            "只允许请求登记端点，且每次搜索只取一次"
        );
    }

    #[tokio::test]
    async fn search_never_offers_entries_without_readable_text() {
        let source = StaticFeedSource::new(UNREADABLE_FEED_FIXTURE);
        let registry = registry();
        let source_id = feed_source_id(&registry).await;
        let participant = FeedSearchParticipant::new(registry, source.clone());

        let not_cancelled = || false;
        let cards = participant
            .search_for(&source_id, "", 10, &not_cancelled)
            .await
            .unwrap();
        assert_eq!(cards.len(), 1, "只有可读的摘要条目应成为候选");
        assert_eq!(cards[0].title, "可读摘要条目");
        assert!(cards[0].work_id.starts_with(CONTENT_CANDIDATE_PREFIX));

        // 不可读条目的标题也不能命中搜索：它拿不到候选句柄。
        let title_only = participant
            .search_for(&source_id, "只有标题", 10, &not_cancelled)
            .await
            .unwrap();
        assert!(title_only.is_empty());
    }

    #[tokio::test]
    async fn provider_reads_and_acquires_only_from_the_registered_feed() {
        let source = StaticFeedSource::new(ATOM_FIXTURE);
        let registry = registry();
        let source_id = feed_source_id(&registry).await;
        let provider = FeedProvider::new(registry, source.clone());
        let entry_digest = feed_entry_digest("tag:feed.example.invalid,2026:1");
        let remote_id = feed_remote_id(&source_id, &entry_digest).unwrap();

        let detail = provider
            .detail(&source_id, "", &entry_digest)
            .await
            .unwrap();
        assert_eq!(
            detail.media_type,
            Some(haven_domain::enums::MediaType::Article)
        );
        assert_eq!(detail.year, Some(2026));
        assert_eq!(
            detail
                .remote
                .as_ref()
                .map(|remote| remote.remote_id.as_str()),
            Some(remote_id.as_str())
        );

        let body = provider
            .read(FEED_SOURCE_KEY, &remote_id, None)
            .await
            .unwrap();
        assert_eq!(body.mime_type, FEED_ARTICLE_MIME);
        let html = String::from_utf8(body.bytes).unwrap();
        assert!(html.contains("Atom 正文"));
        assert!(
            !html.contains("feed.example.invalid/atom/1"),
            "不得消费条目链接内容"
        );

        let directory = tempfile::tempdir().unwrap();
        let destination = directory.path().join("article.html");
        let acquired = provider
            .acquire(FEED_SOURCE_KEY, &remote_id, &destination)
            .await
            .unwrap();
        assert_eq!(acquired.mime, FEED_ARTICLE_MIME);
        assert_eq!(acquired.size_bytes, html.len() as u64);
        assert_eq!(
            source.requested(),
            vec!["https://feed.example.invalid/rss.xml".to_owned(); 3],
            "详情、在线会话与离线快照都只请求登记端点"
        );

        // 未登记的来源键不会命中订阅源 Provider。
        assert!(provider.read("mangadex", &remote_id, None).await.is_err());
        assert!(
            provider
                .acquire("mangadex", &remote_id, &destination)
                .await
                .is_err()
        );
    }

    #[tokio::test]
    async fn detail_rejects_a_stale_candidate_whose_entry_is_no_longer_readable() {
        let source = StaticFeedSource::new(UNREADABLE_FEED_FIXTURE);
        let registry = registry();
        let source_id = feed_source_id(&registry).await;
        let provider = FeedProvider::new(registry, source.clone());

        // 已落库的候选句柄（条目此前可读）在订阅源改版后指向不可读条目：详情必须
        // fail closed，不能返回可导入的远端身份、把失败推迟到导入阶段。
        for key in ["only-title", "scripts-only"] {
            let digest = feed_entry_digest(key);
            let error = provider.detail(&source_id, "", &digest).await.unwrap_err();
            assert_eq!(error.code().as_str(), "SOURCE_UNAVAILABLE");
            assert!(error.retryable());
            let message = error.user_message().to_owned();
            assert!(
                !message.contains(key)
                    && !message.contains("feed.example.invalid")
                    && !message.contains("cdn.example.invalid"),
                "错误文案不得回显条目身份、订阅源地址或被剥离的外链: {message}"
            );
        }

        // 可读条目仍然走得通 详情 → 在线会话 的导入路径。
        let digest = feed_entry_digest("readable-summary");
        let detail = provider.detail(&source_id, "", &digest).await.unwrap();
        let remote_id = detail
            .remote
            .as_ref()
            .expect("可读条目必须给出可导入远端身份")
            .remote_id
            .clone();
        let body = provider
            .read(FEED_SOURCE_KEY, &remote_id, None)
            .await
            .unwrap();
        assert!(
            String::from_utf8(body.bytes)
                .unwrap()
                .contains("只有摘要但可读")
        );
    }

    #[tokio::test]
    async fn removed_or_unregistered_feed_reports_a_stable_unavailable_source_error() {
        let source = StaticFeedSource::new(ATOM_FIXTURE);
        let registry = registry();
        let provider = FeedProvider::new(registry, source.clone());
        let entry_digest = feed_entry_digest("tag:feed.example.invalid,2026:1");

        // 订阅源被删除后，已落库的远端身份仍然存在，但注册表里已经查不到端点。
        // 这属于「来源当前不可用」，必须是可重试的固定来源错误，而不是让调用方
        // 以为是自己传错了参数。
        let error = provider
            .detail("custom_feed_ffffffffffff", "", &entry_digest)
            .await
            .unwrap_err();
        assert_eq!(error.code().as_str(), "SOURCE_UNAVAILABLE");
        assert!(error.retryable());
        assert!(
            !error.user_message().contains("custom_feed_"),
            "错误文案不得回显来源身份"
        );
        assert!(
            source.requested().is_empty(),
            "没有登记端点时不得发起任何请求"
        );
    }

    /// 结构完整、但没有任何条目的订阅源：搜索必须返回空结果——不是错误，也
    /// 不是别的来源的结果——并且仍然只请求一次登记端点。
    #[tokio::test]
    async fn empty_feed_search_returns_no_cards_without_error() {
        let empty = r#"<rss version="2.0"><channel>
            <title>空订阅源</title>
            <link>https://feed.example.invalid/</link>
        </channel></rss>"#;
        let source = StaticFeedSource::new(empty);
        let registry = registry();
        let source_id = feed_source_id(&registry).await;
        let participant = FeedSearchParticipant::new(registry, source.clone());

        let not_cancelled = || false;
        let cards = participant
            .search_for(&source_id, "任意关键词", 10, &not_cancelled)
            .await
            .unwrap();
        assert!(cards.is_empty(), "空订阅源不得产出候选");
        assert_eq!(
            source.requested(),
            vec!["https://feed.example.invalid/rss.xml".to_owned()],
            "空订阅源同样只请求一次登记端点"
        );

        // 家族之外的 sourceId 不会被订阅源参与者接管，也不会触发额外请求。
        assert!(
            participant
                .search_for("mangadex", "任意关键词", 10, &not_cancelled)
                .await
                .unwrap()
                .is_empty()
        );
        assert_eq!(source.requested().len(), 1);
    }

    /// 清洗后 HTML 超过上限的条目，在搜索与详情里都必须表现为「不可读」，
    /// 而不是先给出候选/远端身份、到在线阅读或离线下载时才失败。
    #[tokio::test]
    async fn oversized_entry_is_never_projected_as_readable() {
        // 单段 64 KiB、共 70 段：清洗后正文约 4.4 MiB，确定超过 4 MiB 上限。
        let chunk = "x".repeat(64 * 1024);
        let mut xml = String::from(
            r#"<rss version="2.0" xmlns:content="http://purl.org/rss/1.0/modules/content/"><channel><title>超限订阅源</title><item><guid>huge</guid><title>超限条目</title><content:encoded><![CDATA["#,
        );
        for _ in 0..70 {
            xml.push_str("<p>");
            xml.push_str(&chunk);
            xml.push_str("</p>");
        }
        xml.push_str("]]></content:encoded></item>");
        xml.push_str(
            "<item><guid>small</guid><title>正常条目</title><description>正常正文</description></item>",
        );
        xml.push_str("</channel></rss>");

        let entries = parse_feed(&xml).unwrap();
        let huge = entry(&entries, "huge");
        let small = entry(&entries, "small");
        assert!(
            !entry_has_readable_body(&huge),
            "清洗后超过大小上限的条目不是可读正文"
        );
        let error = render_entry_html(&huge).unwrap_err();
        assert_eq!(error.code().as_str(), "SOURCE_UNAVAILABLE");
        assert!(error.user_message().contains("大小上限"));
        assert!(entry_has_readable_body(&small));

        let source = StaticFeedSource::new(&xml);
        let registry = registry();
        let source_id = feed_source_id(&registry).await;

        let participant = FeedSearchParticipant::new(registry.clone(), source.clone());
        let not_cancelled = || false;
        let cards = participant
            .search_for(&source_id, "", 10, &not_cancelled)
            .await
            .unwrap();
        assert_eq!(cards.len(), 1, "超限条目不得成为搜索候选");
        assert_eq!(cards[0].title, "正常条目");

        // 详情同样 fail closed：已落库的候选句柄不能返回一个可导入的远端身份。
        let provider = FeedProvider::new(registry, source.clone());
        let error = provider
            .detail(&source_id, "", &feed_entry_digest("huge"))
            .await
            .unwrap_err();
        assert_eq!(error.code().as_str(), "SOURCE_UNAVAILABLE");
        assert!(error.retryable());
        assert!(
            provider
                .detail(&source_id, "", &feed_entry_digest("small"))
                .await
                .is_ok()
        );

        // 在线会话与离线获取同样是 fail closed，不会写出一份超限快照。
        let remote_id = feed_remote_id(&source_id, &feed_entry_digest("huge")).unwrap();
        assert!(
            provider
                .read(FEED_SOURCE_KEY, &remote_id, None)
                .await
                .is_err()
        );
        let directory = tempfile::tempdir().unwrap();
        assert!(
            provider
                .acquire(
                    FEED_SOURCE_KEY,
                    &remote_id,
                    &directory.path().join("article.html")
                )
                .await
                .is_err()
        );
    }

    /// 生产组合根使用的远端路由器（`RoutingRemoteSessionPort` /
    /// `RoutingRemoteAcquisitionPort`）必须能把订阅源会话与订阅源获取交给
    /// `OnlineCatalogProvider` 里的 `FeedProvider`：只测 Provider 本身无法证明
    /// 这条真实链路是通的。
    #[tokio::test]
    async fn production_routers_serve_feed_sessions_and_acquisitions() {
        use crate::online_sources::{OnlineCatalogProvider, OnlineContentClient};
        use crate::opds::{
            OpdsCatalogProvider, OpdsClient, RoutingRemoteAcquisitionPort, RoutingRemoteSessionPort,
        };

        let source = StaticFeedSource::new(ATOM_FIXTURE);
        let registry = registry();
        let source_id = feed_source_id(&registry).await;
        let feed = Arc::new(FeedProvider::new(registry, source.clone()));
        let online = Arc::new(
            OnlineCatalogProvider::new(Arc::new(OnlineContentClient::new().unwrap()))
                .with_feed_provider(feed),
        );
        let opds = Arc::new(OpdsCatalogProvider::new(Arc::new(
            OpdsClient::new().unwrap(),
        )));
        let session = RoutingRemoteSessionPort::new(opds.clone(), online.clone());
        let acquisition = RoutingRemoteAcquisitionPort::new(opds, online);

        let remote_id = feed_remote_id(
            &source_id,
            &feed_entry_digest("tag:feed.example.invalid,2026:1"),
        )
        .unwrap();

        let body = session
            .read(FEED_SOURCE_KEY, &remote_id, None)
            .await
            .unwrap();
        assert_eq!(body.mime_type, FEED_ARTICLE_MIME);
        assert!(String::from_utf8(body.bytes).unwrap().contains("Atom 正文"));

        let directory = tempfile::tempdir().unwrap();
        let destination = directory.path().join("article.html");
        let acquired = acquisition
            .acquire(FEED_SOURCE_KEY, &remote_id, &destination)
            .await
            .unwrap();
        assert_eq!(acquired.mime, FEED_ARTICLE_MIME);
        assert!(destination.exists());

        // 路由器不会把订阅源身份发给别的 Provider：未知来源键仍然失败。
        assert!(session.read("mangadex", &remote_id, None).await.is_err());
        assert_eq!(
            source.requested(),
            vec!["https://feed.example.invalid/rss.xml".to_owned(); 2],
            "在线会话与离线获取各只请求一次登记端点"
        );
    }
}
