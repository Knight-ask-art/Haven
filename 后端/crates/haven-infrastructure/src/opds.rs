//! OPDS 1.x（Atom）书源适配器（V2-H1）。
//!
//! 安全边界（对齐 cms10.rs S8：防 SSRF / 出站最小化）：
//! - 仅访问注册表配置端点、其 OpenSearch 模板解析结果与条目页/获取链接；
//!   全部限定 http(s)，下载阶段拒绝重定向到非 http(s)。
//! - 请求策略按**正向确认的来源身份**选择：内置 Gutenberg 只允许固定的公开 HTTPS
//!   主机白名单且不允许任何显式端口；只有注册表里确实登记为自定义 OPDS 书源的
//!   `sourceId` 才使用用户来源端口策略（任意非零端口）。其它 `custom_` 家族
//!   （订阅源 / 漫画库 / TVBox 配置）、未登记与家族外 ID 一律 fail closed。
//! - 每个手工跟随的重定向都重新过一遍策略与 DNS；自定义书源还必须逐跳留在登记端点
//!   origin 上。
//! - 相对引用按 RFC 3986 与目录地址做标准 join，并**保留目录自己的 scheme 与端口**
//!   （`url` 子模块）：匿名 HTTP 目录能保存、能取根目录，也必须能解析搜索地址与条目
//!   链接。只有 Gutenberg 官方镜像保留「明文结果按 https 取」那条窄行为。
//! - Basic Auth 只在**登记端点 origin** 上注入，明文 HTTP 上一律 fail closed；异步凭据
//!   解析前后各取一次权威上下文，解析期间发生的端点/版本变化会让本次请求作废。
//! - Feed 响应 ≤ 8 MiB；EPUB 获取 ≤ 64 MiB（Content-Length 预检 + 流式计数双保险）。
//! - 日志只允许 host，不落完整 URL / 查询串 / 书目内容；请求上下文与注册表快照的
//!   `Debug` 也只打印形状。
//! - 落盘文件只写入已登记的本地存储位置（受控资源，禁止任意路径拼接）。
//! - 搜索结果里的图片只**登记**（`ImageProxyRepository`）并投影成
//!   `haven://artwork/<opaque>`；搜索阶段不为每条结果发图片请求，登记失败只让卡片没有海报。

use std::collections::HashMap;
use std::io::{Cursor, Read};
use std::path::Path;
use std::sync::{Arc, Mutex};

use async_trait::async_trait;

use haven_application::services::SourceRegistryService;
use haven_application::services::ports::{
    RemoteAcquiredFile, RemoteAcquisitionPort, RemoteByteRange, RemoteContentRange,
    RemoteSessionBody, RemoteSessionPort, RemoteTransferContext,
};
use haven_application::services::search_source::SearchSourceParticipant;
use haven_application::services::source_import::{
    RemoteContentRef, SourceCatalogEntry, SourceCatalogProvider,
};
use haven_application::services::source_registry::{
    CustomSourceKind, OpdsCredentialRead, read_opds_credential,
};
use haven_common::network::parse_http_url;
use haven_common::{AppError, ErrorKind};
use haven_domain::comic_catalog::ComicChapterCatalog;
use haven_domain::contracts::ImageProxyRepository;

use tokio::io::AsyncWriteExt;

use zip::ZipArchive;

use crate::cms10::{CMS10_SOURCE_ID, Cms10CatalogProvider};
use crate::http_security::{ResolvedHttpTarget, pin_client_builder, resolve_public_http_target};

/// 地址构造（`url`）与请求权威上下文（`request`）各有自己的所有者模块；离线夹具与
/// 目录级测试只在 `cfg(test)` 下存在。
mod request;
mod url;

#[cfg(test)]
mod catalog_tests;
#[cfg(test)]
pub(crate) mod fixture;
#[cfg(test)]
mod request_tests;

#[cfg(test)]
use self::fixture::{is_loopback_http_url, loopback_fixture_target, parse_loopback_fixture_url};

use self::request::{
    HopCredential, OpdsRequestContext, OpdsSourceIdentity, hop_credential,
    opds_request_context_changed, opds_source_denied,
};
use self::url::{
    absolutize, gutenberg_search_mirrors, has_explicit_authority_port, is_gutenberg_endpoint,
    urlencode, with_gutenberg_host_fallbacks,
};

/// OPDS 书源内置 ID（必须与 source_registry 内置目录一致）。
pub const OPDS_SOURCE_GUTENBERG: &str = "opds_gutenberg";

pub const OPDS_SOURCE_IDS: [&str; 1] = [OPDS_SOURCE_GUTENBERG];

pub fn is_opds_source_id(source_id: &str) -> bool {
    OPDS_SOURCE_IDS.contains(&source_id)
}

const FEED_CAP_BYTES: usize = 8 * 1024 * 1024;
const BOOK_CAP_BYTES: u64 = 64 * 1024 * 1024;
/// The resource protocol and the browser EPUB parser both cap one response at
/// 32 MiB. Keep online EPUB sessions below that boundary even though the
/// explicit download path permits larger provider payloads.
const ONLINE_EPUB_CAP_BYTES: usize = 32 * 1024 * 1024;
const EPUB_MIME: &str = "application/epub+zip";
const MAX_REDIRECTS: usize = 3;
/// 单个网络等待（连接、响应头、每一块正文读取）的上界，与既有整段请求超时相同。
///
/// 远端获取带有限速时，限速睡眠发生在两次读取之间，**不计入**任何网络等待超时；
/// 否则一个大文件在有限速率下会因为「等待配额」被误判为网络超时。
const NETWORK_WAIT_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(15);

/// Atom 解析允许的元素嵌套深度。作用域栈每层保存一份有效 XML Base；8 MiB 输入
/// 足以放下十万层嵌套，若没有上限，逐层 join 出来的祖先 URL 会累积成二次方内存。
const MAX_XML_NESTING_DEPTH: usize = 256;
/// 单个 `xml:base` 原文与解析后的作用域基准长度上限。超长值在 URL join 之前、
/// 结果进入作用域栈之前一律退回父作用域，避免一个巨大基准被复制进每一层。
const MAX_SCOPED_BASE_BYTES: usize = 16 * 1024;

/// 单条 OPDS 条目（解析投影）。
///
/// `entry_id` 是 Atom 的**身份**（`<id>` 原文），不是「一个地址」：它可以是
/// `urn:uuid:...`、`tag:example.org,2026:...` 这类**永远不可请求**的标识，也可以是条目页
/// URL。身份与可抓取地址是两件事，因此候选过滤只对**真正会被请求的地址**做校验，绝不拿
/// 身份去套 URL 策略——否则一个完全合法的 URN 身份会让整条条目（连同它合法的获取链接）
/// 被静默丢弃。
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

impl OpdsEntry {
    /// 目录获取链接；封面由独立图片代理校验，不能使书目被丢弃。
    ///
    /// 只包含已经过 `absolutize` 的绝对 http(s) 值；身份不在其中——它是标识符，不是目标。
    fn fetchable_urls(&self) -> impl Iterator<Item = &str> {
        self.epub_href.as_deref().into_iter()
    }

    /// 身份是否**看起来**是一个可以被当作条目页请求的 http(s) 地址。
    ///
    /// URN / tag 身份在搜索阶段只是一个标识符，不参与出站校验；只有 http(s) 身份才可能
    /// 被后续的条目页抓取（`detail` 路径）当成目标。
    fn identity_is_http_url(&self) -> bool {
        reqwest::Url::parse(&self.entry_id)
            .is_ok_and(|url| matches!(url.scheme(), "http" | "https"))
    }
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

#[derive(Default)]
struct EntryBuilder {
    id: String,
    title: String,
    author: Option<String>,
    summary: String,
    pic: Option<String>,
    epub_href: Option<String>,
}

/// 正在累积文本的字段及其元素在作用域栈中的深度。
///
/// Atom 允许 `title`/`summary`/`content` 使用 xhtml 内联标记；只有字段元素**自己的**
/// 结束标签才能结束捕获，嵌套的 `<em>` 等内联元素不得清空目标，否则 `前<em>中</em>后`
/// 会丢掉 `后`。
#[derive(Clone, Copy)]
struct TextCapture {
    field: &'static str,
    depth: usize,
}

/// 解析 Atom feed：条目级 link 只在 `<entry>` 作用域内收集；feed 级 link
/// （如 rel="search"）单独返回。命名空间前缀按 local-name 归一；链接按**元素作用域**的
/// XML Base 绝对化——`xml:base` 从 feed/entry/link 逐层继承，相对值按父作用域解析。
/// feed 级链接也已是绝对地址；调用方仍会逐条做来源授权校验。
fn parse_atom(xml: &str, base: &str) -> (Vec<OpdsEntry>, Vec<(String, String)>) {
    use quick_xml::events::Event;

    let mut reader = quick_xml::Reader::from_str(xml);
    let mut entries: Vec<OpdsEntry> = Vec::new();
    let mut feed_links: Vec<(String, String)> = Vec::new();
    let mut depth_entry = 0usize;
    let mut builder = EntryBuilder::default();
    // 元素作用域栈：`base_stack[0]` 是文档/响应 base，之后每一项是某个已打开元素的
    // 有效 XML Base（父作用域 + 自身 `xml:base`）。空元素不入栈。
    let mut base_stack: Vec<String> = vec![base.to_owned()];
    let mut text_target: Option<TextCapture> = None;

    loop {
        match reader.read_event() {
            Ok(Event::Start(e)) => {
                // 深度上限必须在计算/压入下一个作用域基准**之前**判定：一旦超限，
                // 继续解析只会把祖先 URL 再复制一份并 join 出更长的值。
                if base_stack.len() > MAX_XML_NESTING_DEPTH {
                    break;
                }
                let name = local_name(&e);
                let element_base = scoped_base(
                    base_stack.last().map(String::as_str).unwrap_or(base),
                    &e,
                );
                base_stack.push(element_base.clone());
                match name.as_str() {
                    "entry" => {
                        depth_entry += 1;
                        builder = EntryBuilder::default();
                        // 条目边界是硬边界：上一条目的字段状态不得流入下一条。
                        text_target = None;
                    }
                    // 字段只在当前没有捕获时开始：字段内部嵌套的元素（包括同名字段）
                    // 属于内联内容，不能把文本改投到另一个 builder 字段。
                    "title" if depth_entry > 0 && text_target.is_none() => {
                        text_target = Some(TextCapture {
                            field: "title",
                            depth: base_stack.len(),
                        });
                    }
                    "summary" | "content" if depth_entry > 0 && text_target.is_none() => {
                        text_target = Some(TextCapture {
                            field: "summary",
                            depth: base_stack.len(),
                        });
                    }
                    "id" if depth_entry > 0 && text_target.is_none() => {
                        text_target = Some(TextCapture {
                            field: "id",
                            depth: base_stack.len(),
                        });
                    }
                    "name" if depth_entry > 0 && text_target.is_none() => {
                        text_target = Some(TextCapture {
                            field: "author",
                            depth: base_stack.len(),
                        });
                    }
                    _ => {}
                }
                if name == "link" {
                    collect_link(
                        &e,
                        depth_entry,
                        &element_base,
                        &mut builder,
                        &mut feed_links,
                    );
                }
            }
            Ok(Event::Empty(e)) => {
                if local_name(&e) == "link" {
                    let element_base = scoped_base(
                        base_stack.last().map(String::as_str).unwrap_or(base),
                        &e,
                    );
                    collect_link(
                        &e,
                        depth_entry,
                        &element_base,
                        &mut builder,
                        &mut feed_links,
                    );
                }
            }
            Ok(Event::Text(t)) => {
                if let Some(capture) = text_target {
                    let text = crate::unescape_xml_text(&t).unwrap_or_default();
                    apply_text(capture.field, &text, &mut builder);
                }
            }
            Ok(Event::CData(t)) => {
                if let Some(capture) = text_target {
                    let raw = t.into_inner();
                    let text = std::str::from_utf8(raw.as_ref()).unwrap_or("");
                    apply_text(capture.field, text, &mut builder);
                }
            }
            Ok(Event::GeneralRef(reference)) => {
                if let Some(capture) = text_target {
                    let text = crate::unescape_xml_reference(&reference).unwrap_or_default();
                    apply_text(capture.field, &text, &mut builder);
                }
            }
            Ok(Event::End(e)) => {
                let name = local_name_of(e.name().as_ref());
                if name == "entry" && depth_entry > 0 {
                    depth_entry -= 1;
                    if depth_entry == 0 && !builder.id.is_empty() && !builder.title.is_empty() {
                        entries.push(OpdsEntry {
                            entry_id: std::mem::take(&mut builder.id).trim().to_owned(),
                            title: clean_text(&builder.title),
                            author: builder.author.take().map(|value| clean_text(&value)),
                            summary: take_summary(&mut builder.summary),
                            pic: builder.pic.take(),
                            epub_href: builder.epub_href.take(),
                        });
                    } else {
                        builder = EntryBuilder::default();
                    }
                    // 条目结束是硬边界：无论字段是否正常闭合，都不把捕获带进下一条。
                    text_target = None;
                } else if text_target.is_some_and(|capture| capture.depth == base_stack.len()) {
                    // 只有字段元素自己的结束标签才收尾；嵌套内联元素的 End 保留捕获。
                    text_target = None;
                }
                if base_stack.len() > 1 {
                    base_stack.pop();
                }
            }
            Ok(Event::Eof) => break,
            // 注释 / 处理指令 / 声明 / DOCTYPE 不是元素边界：字段内的注释不能清空捕获。
            Ok(Event::Comment(_))
            | Ok(Event::PI(_))
            | Ok(Event::Decl(_))
            | Ok(Event::DocType(_)) => {}
            Err(_) => text_target = None,
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

fn valid_search_feed(xml: &str) -> bool {
    use quick_xml::events::Event;
    let mut reader = quick_xml::Reader::from_str(xml);
    let mut depth = 0usize;
    let mut root_seen = false;
    loop {
        match reader.read_event() {
            Ok(Event::Start(element)) => {
                if element.attributes().any(|value| value.is_err()) {
                    return false;
                }
                if depth == 0 {
                    if root_seen || local_name(&element) != "feed" {
                        return false;
                    }
                    root_seen = true;
                }
                depth += 1;
            }
            Ok(Event::Empty(element)) => {
                if element.attributes().any(|value| value.is_err()) {
                    return false;
                }
                if depth == 0 {
                    if root_seen || local_name(&element) != "feed" {
                        return false;
                    }
                    root_seen = true;
                }
            }
            Ok(Event::End(_)) => {
                let Some(next) = depth.checked_sub(1) else {
                    return false;
                };
                depth = next;
            }
            Ok(Event::Text(text)) if depth == 0 => {
                let bytes: &[u8] = text.as_ref();
                if !bytes.iter().all(u8::is_ascii_whitespace) {
                    return false;
                }
            }
            Ok(Event::Eof) => return root_seen && depth == 0,
            Err(_) => return false,
            _ => {}
        }
    }
}

fn apply_opds_auth(
    builder: reqwest::RequestBuilder,
    secret: &str,
) -> Result<reqwest::RequestBuilder, AppError> {
    match read_opds_credential(secret) {
        OpdsCredentialRead::Basic { username, password } => {
            Ok(builder.basic_auth(username, Some(password)))
        }
        OpdsCredentialRead::LegacyPassword | OpdsCredentialRead::Malformed => {
            Err(opds_credential_reconfiguration_required())
        }
    }
}

fn take_summary(raw: &mut str) -> Option<String> {
    let cleaned = clean_text(raw);
    if cleaned.is_empty() {
        None
    } else {
        let mut out = cleaned;
        if out.len() > 2000 {
            let mut end = 2000;
            while !out.is_char_boundary(end) {
                end -= 1;
            }
            out.truncate(end);
        }
        Some(out)
    }
}

fn clean_text(value: &str) -> String {
    value.split_whitespace().collect::<Vec<_>>().join(" ")
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
        "title" => builder.title.push_str(text),
        "summary" => builder.summary.push_str(text),
        "id" => builder.id.push_str(text),
        "author" => builder
            .author
            .get_or_insert_with(String::new)
            .push_str(text),
        _ => {}
    }
}

fn attr(e: &quick_xml::events::BytesStart<'_>, key: &str) -> Option<String> {
    e.attributes().find_map(|a| {
        let a = a.ok()?;
        let k = String::from_utf8_lossy(a.key.as_ref());
        if k.rsplit(':').next()? == key {
            a.normalized_value(quick_xml::XmlVersion::Implicit1_0)
                .ok()
                .map(|value| value.into_owned())
        } else {
            None
        }
    })
}

/// 元素自身 XML Base 的**原文**（`xml:base`），只接受 XML 保留前缀 `xml`：
/// 普通同名属性 `base` 不是 XML Base，不能改变解析基准。
fn element_xml_base(e: &quick_xml::events::BytesStart<'_>) -> Option<String> {
    e.attributes().find_map(|a| {
        let a = a.ok()?;
        let key = String::from_utf8_lossy(a.key.as_ref());
        let (prefix, local) = key.rsplit_once(':')?;
        if prefix != "xml" || local != "base" {
            return None;
        }
        a.normalized_value(quick_xml::XmlVersion::Implicit1_0)
            .ok()
            .map(|value| value.into_owned())
    })
}

/// 计算一个元素的作用域基准：父作用域 base 与本元素 `xml:base` 按 RFC 3986 解析。
///
/// 畸形引用、无法与父 base join 的值，以及非 http(s) 的基准都被忽略，退回父作用域：
/// 绝不凭一个解析不出（或不能用于目录请求）的值去构造目标地址。退回的父 base 仍然
/// 是文档/响应的真实地址，链接随后还要经过来源授权校验。
fn scoped_base(parent: &str, e: &quick_xml::events::BytesStart<'_>) -> String {
    let Some(raw) = element_xml_base(e) else {
        return parent.to_owned();
    };
    // 超长原文在 join 之前拒绝：否则一个巨大 base 会先被解析、再原样进入作用域栈。
    if raw.len() > MAX_SCOPED_BASE_BYTES {
        return parent.to_owned();
    }
    match absolutize(parent, &raw) {
        Some(base)
            if base.len() <= MAX_SCOPED_BASE_BYTES
                && reqwest::Url::parse(&base)
                    .is_ok_and(|url| matches!(url.scheme(), "http" | "https")) =>
        {
            base
        }
        _ => parent.to_owned(),
    }
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
        // feed 级 link（如 rel="search"）同样按元素作用域的 XML Base 绝对化；解析
        // 失败时保留原文，交给既有调用方的错误路径，不在解析层猜测地址。
        feed_links.push((rel, absolutize(base, &href).unwrap_or(href)));
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

/// 凭据解析阶段的失败：**只有** `Reconfigure` 会走到用户面前。
///
/// 历史明文密码（本切片之前的 `haven:opds:<sourceId>` 槽里只有一段密码，确定不了用户名）
/// 不能靠猜来继续：按 `:` 切一刀会把 `p@ss:word` 静默变成一个**不同的**账号，用错误的
/// 身份去登录，用户只会看到一次说不清原因的 401。这里如实报成可操作的重新配置状态。
pub(super) fn opds_credential_reconfiguration_required() -> AppError {
    AppError::new(
        "CREDENTIAL_RECONFIGURATION_REQUIRED",
        ErrorKind::Validation,
        "该来源的访问凭据是旧格式（没有用户名），无法用于 Basic 认证；请在设置 → 来源中重新填写用户名与密码",
        false,
    )
}

/// 自定义源凭据解析器：sourceId → Basic 凭据（用户名 + 密码，只在内存，禁止落盘/日志）。
///
/// 返回 `Ok(None)` = 该来源没有可解析的凭据（未配置 / 已清除）；返回
/// `Err(CREDENTIAL_RECONFIGURATION_REQUIRED)` = 有凭据但形状不可用。两者必须可区分：
/// 前者是「匿名请求照常发」，后者是「停下来让用户重新配置」，都不能降级成盲发。
pub type CredentialResolver = dyn Fn(&str) -> std::pin::Pin<Box<dyn std::future::Future<Output = Option<String>> + Send>>
    + Send
    + Sync;

#[derive(Clone)]
pub struct OpdsClient {
    /// 每个端点的 OpenSearch atom 模板（含 `{searchTerms}`）及**发现它的描述文件地址**。
    /// 模板可能是相对 `results.opds?...`：二次搜索必须仍以描述文件为解析基准，
    /// 不能改用登记端点（否则会落到端点所在的目录下）。二次搜索跳过根目录 + 描述文件 hop。
    search_templates: Arc<Mutex<HashMap<String, (String, String)>>>,
    /// 私有源凭据提供方。
    credential_resolver: Option<Arc<CredentialResolver>>,
    /// 注册表：用来**正向确认**某个 `sourceId` 确实是已登记的自定义 OPDS 书源，并给出它
    /// 当前的登记端点 origin 与配置版本。
    ///
    /// 未注入时自定义 OPDS 请求一律 fail closed（既不解析凭据，也不继承用户来源策略）；
    /// 内置 Gutenberg 与匿名请求照常可用。
    source_registry: Option<SourceRegistryService>,
    #[cfg(test)]
    allow_loopback_fixture: bool,
}

impl OpdsClient {
    pub fn new() -> Result<Self, AppError> {
        Ok(Self {
            search_templates: Arc::new(Mutex::new(HashMap::new())),
            credential_resolver: None,
            source_registry: None,
            #[cfg(test)]
            allow_loopback_fixture: false,
        })
    }

    /// 注入自定义源凭据解析器：sourceId → Basic Auth 密码（内存即取即用）。
    pub fn with_credential_resolver(mut self, resolver: Arc<CredentialResolver>) -> Self {
        self.credential_resolver = Some(resolver);
        self
    }

    /// 注入来源注册表。生产组合根**必须**注入：没有它时任何 `custom_` 来源都拿不到用户
    /// 来源策略与凭据，请求会以 `INVALID_ARGUMENT` fail closed。
    pub fn with_source_registry(mut self, registry: SourceRegistryService) -> Self {
        self.source_registry = Some(registry);
        self
    }

    #[cfg(test)]
    fn with_loopback_fixture(mut self) -> Self {
        self.allow_loopback_fixture = true;
        self
    }

    /// 解析一次请求的权威上下文。
    ///
    /// `None` = 匿名请求（没有来源身份）；内置 Gutenberg 走固定白名单；其余一律要求
    /// **注册表里确实登记为自定义 OPDS 书源**——订阅源 / 漫画库 / TVBox 配置家族、其它
    /// `custom_` 家族、未登记 ID 与家族外 ID 都 fail closed，绝不解析凭据，也绝不继承
    /// 用户来源网络策略。
    async fn request_context(
        &self,
        source_id: Option<&str>,
    ) -> Result<OpdsRequestContext, AppError> {
        let Some(source_id) = source_id else {
            return Ok(OpdsRequestContext::anonymous());
        };
        if is_builtin_opds(source_id) {
            return Ok(OpdsRequestContext::builtin_gutenberg(source_id));
        }
        let Some(registry) = self.source_registry.as_ref() else {
            return Err(opds_source_denied());
        };
        if !matches!(
            SourceRegistryService::custom_kind_from_source_id(source_id),
            Some(CustomSourceKind::Opds)
        ) {
            return Err(opds_source_denied());
        }
        let Some(snapshot) = registry
            .registered_source_request_snapshot(source_id)
            .await?
        else {
            return Err(opds_source_denied());
        };
        // 记录里的种类必须与来源身份形状一致；被伪造/手工改写的记录 fail closed。
        if snapshot.kind() != CustomSourceKind::Opds {
            return Err(opds_source_denied());
        }
        Ok(OpdsRequestContext::registered_custom_opds(
            source_id, snapshot,
        ))
    }

    fn validate_request_url(
        &self,
        raw: &str,
        context: &OpdsRequestContext,
    ) -> Result<reqwest::Url, AppError> {
        #[cfg(test)]
        if self.allow_loopback_fixture && is_loopback_http_url(raw) {
            return parse_loopback_fixture_url(raw);
        }
        validate_opds_url(raw, context)
    }

    async fn resolve_request_target(
        &self,
        context: &OpdsRequestContext,
        current: &reqwest::Url,
    ) -> Result<ResolvedHttpTarget, AppError> {
        #[cfg(test)]
        if self.allow_loopback_fixture {
            if let Some(target) = loopback_fixture_target(current) {
                return target;
            }
        }
        resolve_public_http_target(current.as_str(), context.policy())
            .await
            .map_err(|_| invalid_feed("目录地址解析不安全"))
    }

    /// 解析凭据并复核上下文。
    ///
    /// 解析器在异步区间之前读到的来源身份/端点/版本，与解析之后重读的那一份必须完全一致；
    /// 任何变化（端点 A→B、A→B→A、凭据轮换、来源被删除）都让本次请求 fail closed，绝不把
    /// 新解析出来的 secret 发往旧上下文选定的端点。**解析结果为空也必须复核**：解析器返回
    /// `Ok(None)` 只说明本次拿不到 secret，不说明来源上下文没变；漏掉这一步会让一个已经失效的
    /// 上下文照常发请求。
    ///
    /// 凭据形状本身由解析器判定（它才读得到凭据槽）：历史明文密码在那里变成
    /// `CREDENTIAL_RECONFIGURATION_REQUIRED`，**绝不**由本函数拿 `sourceId` 顶替用户名。
    async fn resolve_credential(
        &self,
        context: &OpdsRequestContext,
        target: &reqwest::Url,
    ) -> Result<Option<String>, AppError> {
        let (Some(source_id), Some(resolver)) =
            (context.source_id(), self.credential_resolver.as_ref())
        else {
            return Ok(None);
        };
        // 权威 origin 的一跳才允许解析 secret；跨 origin 的一跳连解析都不做。
        if hop_credential(context, target)? != HopCredential::Authorized {
            return Ok(None);
        }
        let secret = resolver(source_id).await;
        let current = self.request_context(Some(source_id)).await?;
        if !context.is_current_with(&current) {
            return Err(opds_request_context_changed());
        }
        Ok(secret)
    }

    async fn get_limited(
        &self,
        context: &OpdsRequestContext,
        url: &str,
        cap: usize,
    ) -> Result<(String, Vec<u8>), AppError> {
        self.get_limited_with(context, url, cap, None, false).await
    }

    /// 受控读取：`transfer` 携带本次获取共享的有限速率与进度观察者。
    /// `total_from_response` 只在「响应体就是最终产物」时置真；目录/元数据响应
    /// 必须保持未知总大小。
    async fn get_limited_with(
        &self,
        context: &OpdsRequestContext,
        url: &str,
        cap: usize,
        transfer: Option<&RemoteTransferContext<'_>>,
        total_from_response: bool,
    ) -> Result<(String, Vec<u8>), AppError> {
        // 网络对部分书目站存在间歇性 TLS 重置：传输层错误单次重试（响应上限不变）。
        match self
            .get_limited_once_with(context, url, cap, transfer, total_from_response)
            .await
        {
            Err(e) if e.retryable() => {
                self.get_limited_once_with(context, url, cap, transfer, total_from_response)
                    .await
            }
            other => other,
        }
    }

    async fn get_limited_once_with(
        &self,
        context: &OpdsRequestContext,
        url: &str,
        cap: usize,
        transfer: Option<&RemoteTransferContext<'_>>,
        total_from_response: bool,
    ) -> Result<(String, Vec<u8>), AppError> {
        let origin = self.validate_request_url(url, context)?;
        let mut current = origin.clone();
        for _ in 0..=MAX_REDIRECTS {
            // 凭据判定（含「已配置凭据的来源不得走明文 HTTP」）在任何 DNS 或网络动作
            // 之前完成：fail closed 不依赖解析是否成功。
            let credential = hop_credential(context, &current)?;
            let target = self.resolve_request_target(context, &current).await?;
            // 已登记的自定义书源必须逐跳留在**登记端点 origin** 上（重定向到别的
            // origin 一律拒绝）。内置 Gutenberg 与匿名请求的主机判定已经由
            // `validate_request_url`（含 `has_explicit_authority_port`）逐跳施加，
            // 这里不再重复一遍。
            if context.is_registered_custom_opds() && !context.authorizes_url(&target.url) {
                return Err(invalid_feed("目录重定向来源不一致"));
            }
            // 连接与响应头各有独立超时；不再设置整段请求超时，否则有限速时的
            // 限速睡眠会被计入网络超时，大文件必然失败。
            let builder = reqwest::Client::builder()
                .user_agent("haven/0.1")
                .connect_timeout(NETWORK_WAIT_TIMEOUT)
                // Redirects are followed explicitly below so every hop gets
                // a fresh URL policy and DNS decision.
                .redirect(reqwest::redirect::Policy::none());
            let client = pin_client_builder(builder, &target)
                .build()
                .map_err(|_| invalid_feed("HTTP 客户端初始化失败"))?;
            let mut builder = client.get(target.url.clone());
            if credential == HopCredential::Authorized {
                let resolved = self.resolve_credential(context, &current).await?;
                if let Some(secret) = resolved {
                    builder = apply_opds_auth(builder, &secret)?;
                }
            }
            let resp = tokio::time::timeout(NETWORK_WAIT_TIMEOUT, builder.send())
                .await
                .map_err(|_| invalid_feed("目录服务不可达"))?
                .map_err(|_| invalid_feed("目录服务不可达"))?;
            if resp.status().is_redirection() {
                let location = resp
                    .headers()
                    .get(reqwest::header::LOCATION)
                    .and_then(|value| value.to_str().ok())
                    .ok_or_else(|| invalid_feed("目录重定向地址无效"))?;
                current = target
                    .url
                    .join(location)
                    .map_err(|_| invalid_feed("目录重定向地址无效"))?;
                self.validate_request_url(current.as_str(), context)?;
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
            if let (Some(transfer), true) = (transfer, total_from_response) {
                // 一次新的产物尝试（首次读取、传输层重试或主机回退）：产物进度归零、
                // 总大小回到未知，累计网络字节保持不变。只有一个跳转后的最终响应且
                // 响应体就是产物时，Content-Length 才是这次尝试的整体总大小；
                // 重定向前的 hop 不会走到这里。
                transfer.begin_artifact_attempt();
                transfer.set_total_bytes(resp.content_length());
            }
            let mut out: Vec<u8> = Vec::with_capacity(64 * 1024);
            let mut stream = resp;
            loop {
                let next = tokio::time::timeout(NETWORK_WAIT_TIMEOUT, stream.chunk())
                    .await
                    .map_err(|_| invalid_feed("目录读取中断"))?
                    .map_err(|_| invalid_feed("目录读取中断"))?;
                let Some(chunk) = next else { break };
                if out.len().saturating_add(chunk.len()) > cap {
                    return Err(invalid_feed("目录响应超出大小上限"));
                }
                out.extend_from_slice(chunk.as_ref());
                if let Some(transfer) = transfer {
                    // 真实网络字节：限速睡眠发生在这次读取之后，不在网络超时窗口内。
                    // 只有响应体就是最终产物时才同时计入产物进度；目录/条目元数据
                    // 只计网络字节，绝不能冒充 EPUB 的完成度。
                    if total_from_response {
                        transfer.record_artifact(chunk.len() as u64).await;
                    } else {
                        transfer.record(chunk.len() as u64).await;
                    }
                }
            }
            return Ok((current.to_string(), out));
        }
        Err(invalid_feed("目录重定向次数过多"))
    }

    #[cfg(test)]
    async fn get_feed(
        &self,
        context: &OpdsRequestContext,
        url: &str,
    ) -> Result<(String, Vec<u8>), AppError> {
        self.get_limited(context, url, FEED_CAP_BYTES).await
    }

    /// 受控目录读取：把同一次获取的 `transfer` 带到目录/条目元数据的正文读取循环。
    /// 目录响应不是产物，`total_from_response` 保持 false（总大小未知，且不计入产物进度）。
    async fn get_feed_with(
        &self,
        context: &OpdsRequestContext,
        url: &str,
        transfer: Option<&RemoteTransferContext<'_>>,
    ) -> Result<(String, Vec<u8>), AppError> {
        self.get_limited_with(context, url, FEED_CAP_BYTES, transfer, false)
            .await
    }

    async fn get_feed_with_host_fallback(
        &self,
        context: &OpdsRequestContext,
        url: &str,
        transfer: Option<&RemoteTransferContext<'_>>,
    ) -> Result<(String, Vec<u8>), AppError> {
        let mut last = None;
        for candidate in with_gutenberg_host_fallbacks(url) {
            match self.get_feed_with(context, &candidate, transfer).await {
                Ok(response) => return Ok(response),
                Err(err) => last = Some(err),
            }
        }
        Err(last.unwrap_or_else(|| invalid_feed("目录服务不可达")))
    }

    async fn get_limited_with_host_fallback(
        &self,
        context: &OpdsRequestContext,
        url: &str,
        cap: usize,
    ) -> Result<Vec<u8>, AppError> {
        self.get_limited_with_host_fallback_with(context, url, cap, None, false)
            .await
    }

    /// 主机回退的受控版本：所有候选主机共享同一个 `transfer`（累计网络字节与
    /// 限速不随一次失败的主机重置）。
    async fn get_limited_with_host_fallback_with(
        &self,
        context: &OpdsRequestContext,
        url: &str,
        cap: usize,
        transfer: Option<&RemoteTransferContext<'_>>,
        total_from_response: bool,
    ) -> Result<Vec<u8>, AppError> {
        let mut last = None;
        for candidate in with_gutenberg_host_fallbacks(url) {
            match self
                .get_limited_with(context, &candidate, cap, transfer, total_from_response)
                .await
            {
                Ok((_, body)) => return Ok(body),
                Err(err) => last = Some(err),
            }
        }
        Err(last.unwrap_or_else(|| invalid_feed("目录服务不可达")))
    }

    /// 返回缓存的 `(模板, 描述文件地址)`。描述文件地址是相对模板的唯一解析基准。
    fn cached_template(&self, endpoint: &str) -> Option<(String, String)> {
        self.search_templates
            .lock()
            .ok()
            .and_then(|g| g.get(endpoint).cloned())
    }

    /// 连同发现模板的描述文件地址一起缓存：模板里的 `{searchTerms}` 原样保留，
    /// 二次搜索才能在不重新访问根目录/描述文件的前提下解析出同一个结果地址。
    fn remember_template(&self, endpoint: &str, template: String, base: String) {
        if let Ok(mut g) = self.search_templates.lock() {
            g.insert(endpoint.to_owned(), (template, base));
        }
    }

    /// 模板展开的唯一所有者：缓存模板与刚发现的模板都走这里。
    ///
    /// `None` 表示模板不可用；已发现的搜索描述不能静默退回另一个查询协议。
    fn apply_template(template: &str, query: &str, base: &str) -> Option<String> {
        expand_search_template(template, query).and_then(|expanded| absolutize(base, &expanded))
    }

    /// 搜索地址解析：优先缓存模板 → OpenSearch atom 模板 → Gutenberg search.opds → `?query=`。
    /// 模板结果统一过 `absolutize`：保留目录自己的 scheme 与端口，只有 Gutenberg 官方
    /// 镜像保留那条「明文结果按 https 取」的窄行为。
    pub async fn search_url(
        &self,
        source_id: Option<&str>,
        endpoint: &str,
        query: &str,
    ) -> Result<String, AppError> {
        let context = self.request_context(source_id).await?;
        self.search_url_with(&context, endpoint, query).await
    }

    async fn search_url_with(
        &self,
        context: &OpdsRequestContext,
        endpoint: &str,
        query: &str,
    ) -> Result<String, AppError> {
        if let Some((tpl, base)) = self.cached_template(endpoint) {
            let url = Self::apply_template(&tpl, query, &base)
                .ok_or_else(|| invalid_feed("搜索描述模板含不支持的必填变量或格式无效"))?;
            if self.is_source_url_allowed(context, endpoint, &url) {
                return Ok(url);
            }
            return Err(invalid_feed("搜索地址解析失败"));
        }
        // Gutenberg 的 m./www. 镜像在部分网络中的可用性不同；根目录解析
        // 也必须走同一组受控镜像回退，否则仅因根目录一次 TLS/504 波动就
        // 会把一个已经有真实搜索实现的内置来源误判为不可搜索。
        let (resolved_endpoint, root) = if is_gutenberg_endpoint(endpoint) {
            self.get_feed_with_host_fallback(context, endpoint, None)
                .await?
        } else {
            self.get_feed_with(context, endpoint, None).await?
        };
        let xml = std::str::from_utf8(&root).map_err(|_| invalid_feed("目录编码异常"))?;
        let (_, feed_links) = parse_atom(xml, &resolved_endpoint);
        if let Some((_, search_desc)) = feed_links.iter().find(|(rel, _)| rel == "search") {
            // feed 级链接已按作用域 XML Base 绝对化；这里再 join 一次只用于兜住解析层
            // 保留的畸形原文（既有错误路径），不会把绝对地址改到别的地址。
            let absolute_desc = absolutize(&resolved_endpoint, search_desc)
                .ok_or_else(|| invalid_feed("搜索描述地址非法"))?;
            if self.is_source_url_allowed(context, endpoint, &absolute_desc)
                && let Ok((descriptor_url, desc)) =
                    self.get_limited(context, &absolute_desc, 256 * 1024).await
            {
                if let Some(tpl) = extract_atom_template(&String::from_utf8_lossy(&desc)) {
                    let url = Self::apply_template(&tpl, query, &descriptor_url)
                        .ok_or_else(|| invalid_feed("搜索描述模板含不支持的必填变量或格式无效"))?;
                    if !self.is_source_url_allowed(context, endpoint, &url) {
                        return Err(invalid_feed("搜索地址解析失败"));
                    }
                    self.remember_template(endpoint, tpl, descriptor_url);
                    return Ok(url);
                }
            }
        }
        if let Some(url) = gutenberg_search_mirrors(query)
            .into_iter()
            .next()
            .filter(|_| is_gutenberg_endpoint(endpoint))
        {
            if self.is_source_url_allowed(context, endpoint, &url) {
                return Ok(url);
            }
        }
        let sep = if endpoint.contains('?') { '&' } else { '?' };
        let url = absolutize(
            endpoint,
            &format!("{endpoint}{sep}query={}", urlencode(query)),
        )
        .filter(|url| self.is_source_url_allowed(context, endpoint, url))
        .ok_or_else(|| invalid_feed("搜索地址解析失败"))?;
        Ok(url)
    }

    pub async fn search_entries(
        &self,
        source_id: Option<&str>,
        endpoint: &str,
        query: &str,
        limit: u32,
    ) -> Result<Vec<OpdsEntry>, AppError> {
        // 一次搜索只解析一次权威上下文：整轮请求（含模板/描述文件/条目页）使用同一份
        // 来源身份、端口策略与权威 origin。
        let context = self.request_context(source_id).await?;
        let mut urls: Vec<String> = Vec::new();
        // Gutenberg：www 镜像优先（m. 对 search.opds 常 504），成功则跳过 OpenSearch。
        if is_gutenberg_endpoint(endpoint) {
            urls.extend(gutenberg_search_mirrors(query));
        } else {
            urls.push(self.search_url_with(&context, endpoint, query).await?);
        }
        let mut last_err: Option<AppError> = None;
        if let Some(entries) = self
            .try_search_urls(&context, endpoint, &urls, limit, &mut last_err)
            .await
        {
            return Ok(entries);
        }
        if is_gutenberg_endpoint(endpoint) {
            if let Ok(url) = self.search_url_with(&context, endpoint, query).await {
                let extras: Vec<String> = with_gutenberg_host_fallbacks(&url)
                    .into_iter()
                    .filter(|u| !urls.contains(u))
                    .collect();
                if let Some(entries) = self
                    .try_search_urls(&context, endpoint, &extras, limit, &mut last_err)
                    .await
                {
                    return Ok(entries);
                }
            }
        }
        Err(last_err.unwrap_or_else(|| invalid_feed("目录服务不可达")))
    }

    /// 返回首个有效 feed，包括空结果；只有传输或解析失败才尝试镜像。
    /// 获取链接和 HTTP 身份须属于目录来源；封面另由图片代理校验。
    async fn try_search_urls(
        &self,
        context: &OpdsRequestContext,
        source_endpoint: &str,
        urls: &[String],
        limit: u32,
        last_err: &mut Option<AppError>,
    ) -> Option<Vec<OpdsEntry>> {
        for url in urls {
            match self.get_feed_with(context, url, None).await {
                Ok((resolved_url, body)) => {
                    let xml = String::from_utf8_lossy(&body);
                    if !valid_search_feed(&xml) {
                        *last_err = Some(invalid_feed("目录返回了无效的 Atom 搜索结果"));
                        continue;
                    }
                    let mut entries: Vec<OpdsEntry> =
                        book_candidates(parse_atom(&xml, &resolved_url).0)
                            .into_iter()
                            .filter(|entry| {
                                self.candidate_stays_on_source(context, source_endpoint, entry)
                            })
                            .collect();
                    // 空结果同样是**成功读到的**事实：截断后返回，不再继续试下一个候选。
                    entries.truncate(limit as usize);
                    return Some(entries);
                }
                Err(err) => *last_err = Some(err),
            }
        }
        None
    }

    /// 一条候选是否仍然留在本次请求的来源权威内。
    ///
    /// 判定对象是条目上**真正会被请求的地址**（获取链接、封面），而不是 Atom 身份：
    /// `urn:uuid:...` 与 `tag:...` 是完全合法的 OPDS 身份，对它们套 URL 策略只能得到
    /// 「不是 http(s)」这一与安全无关的结论。反过来，一个**看起来像 URL** 的身份必须
    /// 接受同样的权威校验——它是 `detail` 路径会去抓的条目页，不能借身份字段把请求引到
    /// 别的 origin。
    fn candidate_stays_on_source(
        &self,
        context: &OpdsRequestContext,
        source_endpoint: &str,
        entry: &OpdsEntry,
    ) -> bool {
        for target in entry.fetchable_urls() {
            if !self.is_source_url_allowed(context, source_endpoint, target) {
                return false;
            }
        }
        if entry.identity_is_http_url() {
            return self.is_source_url_allowed(context, source_endpoint, &entry.entry_id);
        }
        // Gutenberg 的导入链路必须重新抓取 HTTP 条目页。仅搜索的自定义
        // 目录可以保留 URN/tag 身份，不能把它们暴露为可导入候选。
        context.identity() != OpdsSourceIdentity::BuiltinGutenberg
    }

    /// 条目页 → 只解析单条目元数据，不获取 EPUB 正文。
    pub async fn fetch_entry_metadata(
        &self,
        source_id: Option<&str>,
        entry_page_url: &str,
    ) -> Result<OpdsEntry, AppError> {
        let context = self.request_context(source_id).await?;
        self.fetch_entry_metadata_with(&context, entry_page_url, None)
            .await
    }

    /// `transfer` 只在**显式下载**的获取上下文中存在：同一次受控获取里先读条目页、
    /// 再读 EPUB，条目页字节必须计入同一次网络限速，但不得计入产物进度。
    async fn fetch_entry_metadata_with(
        &self,
        context: &OpdsRequestContext,
        entry_page_url: &str,
        transfer: Option<&RemoteTransferContext<'_>>,
    ) -> Result<OpdsEntry, AppError> {
        let (page_url, body) = self
            .get_feed_with_host_fallback(context, entry_page_url, transfer)
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
        let context = self.request_context(source_id).await?;
        let entry = self
            .fetch_entry_metadata_with(&context, entry_page_url, None)
            .await?;
        let href = entry
            .epub_href
            .clone()
            .ok_or_else(|| invalid_feed("该条目没有 EPUB 获取链接"))?;
        let data = self
            .get_limited_with_host_fallback(&context, &href, BOOK_CAP_BYTES as usize)
            .await?;
        Ok((entry, data.to_vec()))
    }
}

/// 搜索调用方不传分页参数，因此模板里只出现 searchTerms 时地址与旧行为逐字节一致；
/// 分页变量取「第 1 页」的一致默认值，避免把空串塞进 `startIndex`/`count` 造成服务端
/// 解析失败或溢出（OpenSearch 未给 count 时的标准默认是每页 10 条，这里沿用常见的
/// 20 条目录页大小）。
const OPENSEARCH_DEFAULT_COUNT: u32 = 20;
const OPENSEARCH_DEFAULT_START_INDEX: u32 = 1;
/// 展开后模板地址的长度上界：既限制输入量，也保证描述文件给出的模板不能借展开把
/// 请求放大到任意长度。
const MAX_TEMPLATE_EXPANDED_BYTES: usize = 8 * 1024;

/// 按 OpenSearch 1.1 语法展开搜索模板。
///
/// - `{searchTerms}` 与 `{searchTerms?}` → 百分号编码后的关键字。
/// - `{count}` / `{startIndex}` / `{startPage}` → 上述有界默认值，三者同属第 1 页。
/// - `{language}` / `{inputEncoding}` / `{outputEncoding}` → 编码 `UTF-8`，语言 `*`
///   （OpenSearch 约定的「未指定」通配）。
/// - 其余 `{name?}` 可选变量省略；其余 `{name}` 必填变量视为模板不可用。
/// - 花括号不配对、空变量名、变量名含非 ASCII 字母数字字符、展开结果超过
///   `MAX_TEMPLATE_EXPANDED_BYTES` 时不可用。
fn expand_search_template(template: &str, query: &str) -> Option<String> {
    if template.len() > MAX_TEMPLATE_EXPANDED_BYTES || query.len() > MAX_TEMPLATE_EXPANDED_BYTES {
        return None;
    }
    let terms = urlencode(query);
    let count = OPENSEARCH_DEFAULT_COUNT.to_string();
    let start_index = OPENSEARCH_DEFAULT_START_INDEX.to_string();
    // count 决定整页大小，startIndex 从 1 起计，因此第 1 页的 startPage 恒为 1。
    let start_page = "1".to_owned();
    let mut out = String::with_capacity(template.len().min(MAX_TEMPLATE_EXPANDED_BYTES));
    let mut rest = template;
    while let Some(open) = rest.find(['{', '}']) {
        if rest[open..].starts_with('}') {
            return None;
        }
        out.push_str(&rest[..open]);
        let after = &rest[open + 1..];
        let close = after.find('}')?;
        let (name, optional) = match after[..close].strip_suffix('?') {
            Some(base) => (base, true),
            None => (&after[..close], false),
        };
        if name.is_empty() || !name.bytes().all(|b| b.is_ascii_alphanumeric() || b"_:.-".contains(&b)) {
            return None;
        }
        let value = match name {
            "searchTerms" => Some(terms.as_str()),
            "count" => Some(count.as_str()),
            "startIndex" => Some(start_index.as_str()),
            "startPage" => Some(start_page.as_str()),
            "language" => Some("*"),
            "inputEncoding" | "outputEncoding" => Some("UTF-8"),
            _ if optional => None,
            _ => return None,
        };
        if let Some(value) = value {
            out.push_str(value);
        }
        if out.len() > MAX_TEMPLATE_EXPANDED_BYTES {
            return None;
        }
        rest = &after[close + 1..];
    }
    out.push_str(rest);
    (out.len() <= MAX_TEMPLATE_EXPANDED_BYTES).then_some(out)
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

/// 校验 OPDS 请求地址。
///
/// 策略由**请求上下文**决定，而不是由调用方随手传进来的字符串：已登记的自定义 OPDS
/// 书源可以使用用户来源端口策略（任意非零端口），内置 Gutenberg 只允许公开站点的
/// HTTPS 主机且**不允许任何显式端口（含 `:443`）**，匿名请求使用固定严格策略。
/// 每个手工跟随的重定向都会再次进入本函数，因此端口与主机的判定逐跳成立。userinfo、
/// fragment 与畸形 authority 由共享策略统一拒绝。
fn validate_opds_url(raw: &str, context: &OpdsRequestContext) -> Result<reqwest::Url, AppError> {
    let safe = parse_http_url(raw, context.policy()).map_err(|_| invalid_feed("目录地址不安全"))?;
    let host = safe.host().to_owned();
    let url = safe.into_url();
    if context.identity() == OpdsSourceIdentity::BuiltinGutenberg
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

/// Feed-discovered URLs remain inside the authority that owns the configured
/// OPDS source. Gutenberg is the one deliberate two-host exception because
/// its official mobile and desktop mirrors are interchangeable and anonymous.
fn same_opds_authority(
    origin: &reqwest::Url,
    target: &reqwest::Url,
    context: &OpdsRequestContext,
) -> bool {
    let Some(origin_host) = origin.host_str() else {
        return false;
    };
    let Some(target_host) = target.host_str() else {
        return false;
    };
    let origin_host = origin_host.trim_end_matches('.').to_ascii_lowercase();
    let target_host = target_host.trim_end_matches('.').to_ascii_lowercase();
    let same_port = origin.port_or_known_default() == target.port_or_known_default();
    if !same_port || origin.scheme() != target.scheme() {
        return false;
    }
    if context.identity() == OpdsSourceIdentity::BuiltinGutenberg {
        matches!(
            origin_host.as_str(),
            "www.gutenberg.org" | "m.gutenberg.org"
        ) && matches!(
            target_host.as_str(),
            "www.gutenberg.org" | "m.gutenberg.org"
        )
    } else {
        origin_host == target_host
    }
}

/// Feed-discovered URL checks need the request's own policy (and its test fixture), so they
/// live on the client rather than on a free function.
impl OpdsClient {
    /// 模板 / 描述文件 / 条目身份发现的地址是否仍属于本次请求的来源权威。
    ///
    /// 两侧都先过**本次请求的策略**（与真正发请求时同一个函数，因此测试 fixture
    /// 的行为与其它跳完全一致），再比较 authority。
    fn is_source_url_allowed(
        &self,
        context: &OpdsRequestContext,
        origin: &str,
        candidate: &str,
    ) -> bool {
        let Ok(origin) = self.validate_request_url(origin, context) else {
            return false;
        };
        let Ok(candidate) = self.validate_request_url(candidate, context) else {
            return false;
        };
        same_opds_authority(&origin, &candidate, context)
    }
}

fn validate_builtin_gutenberg_url(raw: &str) -> Result<reqwest::Url, AppError> {
    validate_opds_url(
        raw,
        &OpdsRequestContext::builtin_gutenberg(OPDS_SOURCE_GUTENBERG),
    )
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

/// Return a complete EPUB for ordinary Reader loads, or a safe inclusive byte
/// slice for the initial resource-protocol probe.  Gutenberg does not provide
/// a dependable Range endpoint for every mirror, so the provider obtains one
/// bounded object and slices it in memory.  The online cap intentionally stays
/// at the 32 MiB protocol/parser boundary; larger books remain download-first.
fn bounded_epub_session_body(
    bytes: Vec<u8>,
    range: Option<RemoteByteRange>,
) -> Result<RemoteSessionBody, AppError> {
    let total = u64::try_from(bytes.len()).map_err(|_| invalid_feed("EPUB 大小无效"))?;
    if total == 0 || total > ONLINE_EPUB_CAP_BYTES as u64 {
        return Err(invalid_feed("EPUB 超出在线阅读大小限制"));
    }
    let Some(range) = range else {
        return Ok(RemoteSessionBody {
            mime_type: EPUB_MIME.to_owned(),
            bytes,
            total_size: total,
            content_range: None,
            accept_ranges: true,
        });
    };
    if range.start >= total {
        return Err(AppError::new(
            "RANGE_INVALID",
            ErrorKind::Validation,
            "远端正文的范围请求无效",
            false,
        ));
    }
    let requested_end = range.end.unwrap_or(total - 1);
    if requested_end < range.start {
        return Err(AppError::new(
            "RANGE_INVALID",
            ErrorKind::Validation,
            "远端正文的范围请求无效",
            false,
        ));
    }
    let end = requested_end.min(total - 1);
    let start = usize::try_from(range.start).map_err(|_| invalid_feed("范围请求无效"))?;
    let end_exclusive = usize::try_from(end + 1).map_err(|_| invalid_feed("范围请求无效"))?;
    Ok(RemoteSessionBody {
        mime_type: EPUB_MIME.to_owned(),
        bytes: bytes[start..end_exclusive].to_vec(),
        total_size: total,
        content_range: Some(RemoteContentRange {
            start: range.start,
            end,
            total,
        }),
        accept_ranges: true,
    })
}

/// 内置 OPDS 来源判定：当前**只有一个**内置预设（Gutenberg），按 ID 常量精确匹配，
/// 匿名访问、固定公开 HTTPS 主机白名单。
///
/// 自定义身份不在这里判定，也不由名字或前缀长相推断：它来自注册表正向确认的记录
/// （种类 + 登记端点 origin 快照，见 `request_context`），因此订阅源 / 漫画库 / TVBox
/// 配置等其它 `custom_` 家族与未登记 ID 都拿不到用户来源策略与凭据。
fn is_builtin_opds(source_id: &str) -> bool {
    OPDS_SOURCE_IDS.contains(&source_id)
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
        // 权威上下文来自**注册表**：只有确实登记为自定义 OPDS 书源的 `sourceId` 才拿到
        // 用户来源策略与凭据，未知/其它 `custom_` 家族会在这里 fail closed。
        //
        // 复核登记版本和 origin 后解析当前凭据引用；用户名取自版本化载荷。
        let context = self.client.request_context(Some(source_id)).await?;
        // A candidate handle may only refer to an entry on the configured
        // source authority.  Do not let a caller replace the registered
        // endpoint with an arbitrary public URL; credential injection is bound
        // to the registered endpoint origin inside the client, not to whatever
        // endpoint the caller passed here.
        let source_endpoint = validate_opds_url(endpoint, &context)?;
        let entry_url = validate_opds_url(external_id, &context)?;
        if !same_opds_authority(&source_endpoint, &entry_url, &context) {
            return Err(invalid_feed("目录条目不属于已配置来源"));
        }
        let entry = self
            .client
            .fetch_entry_metadata_with(&context, entry_url.as_str(), None)
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
            comic_catalog: None,
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
        self.acquire_inner(source_key, remote_id, destination, None)
            .await
    }

    async fn acquire_controlled(
        &self,
        source_key: &str,
        remote_id: &str,
        destination: &Path,
        transfer: &RemoteTransferContext<'_>,
    ) -> Result<RemoteAcquiredFile, AppError> {
        self.acquire_inner(source_key, remote_id, destination, Some(transfer))
            .await
    }
}

impl OpdsCatalogProvider {
    async fn acquire_inner(
        &self,
        source_key: &str,
        remote_id: &str,
        destination: &Path,
        transfer: Option<&RemoteTransferContext<'_>>,
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
        let context = OpdsRequestContext::builtin_gutenberg(source_key);
        // 条目页与 EPUB 属于**同一次**受控获取：元数据正文只计网络字节与限速，
        // 随后 EPUB 的产物进度从 0 开始，二者绝不能相加成完成度。
        let entry = self
            .client
            .fetch_entry_metadata_with(&context, remote_url.as_str(), transfer)
            .await?;
        let href = entry
            .epub_href
            .as_deref()
            .ok_or_else(|| invalid_feed("该条目没有 EPUB 获取链接"))?;
        let href = validate_builtin_gutenberg_url(href)?;
        let bytes = self
            .client
            .get_limited_with_host_fallback_with(
                &context,
                href.as_str(),
                BOOK_CAP_BYTES as usize,
                transfer,
                true,
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

/// Gutenberg EPUBs are also readable in an online Reader session.  The
/// provider deliberately keeps this path separate from `RemoteAcquisitionPort`:
/// no file is created and no Offline Resource is registered when the reader
/// probes or fetches the session URI.  We fetch a bounded, validated EPUB in
/// infrastructure and only return the requested byte slice to the controlled
/// resource protocol.
#[async_trait]
impl RemoteSessionPort for OpdsCatalogProvider {
    async fn read(
        &self,
        source_key: &str,
        remote_id: &str,
        range: Option<RemoteByteRange>,
    ) -> Result<RemoteSessionBody, AppError> {
        if source_key != OPDS_SOURCE_GUTENBERG {
            return Err(AppError::new(
                "INVALID_ARGUMENT",
                ErrorKind::Validation,
                "未知远端书籍来源",
                false,
            ));
        }

        // The persisted remote identity is the Gutenberg OPDS entry page, not
        // a caller-supplied download URL.  Re-validate it before every read,
        // then resolve the EPUB link from the current entry metadata.
        let entry_url = validate_builtin_gutenberg_url(remote_id)?;
        let context = OpdsRequestContext::builtin_gutenberg(source_key);
        let entry = self
            .client
            .fetch_entry_metadata_with(&context, entry_url.as_str(), None)
            .await?;
        let epub_href = entry
            .epub_href
            .as_deref()
            .ok_or_else(|| invalid_feed("该条目没有 EPUB 在线阅读链接"))?;
        let epub_url = validate_builtin_gutenberg_url(epub_href)?;
        let bytes = self
            .client
            .get_limited_with_host_fallback(&context, epub_url.as_str(), ONLINE_EPUB_CAP_BYTES)
            .await?;
        validate_epub_payload(&bytes)?;
        bounded_epub_session_body(bytes, range)
    }
}

/// Download Worker 使用的远端来源路由。OPDS Provider 与 MangaDex/arXiv/
/// Europe PMC/Wikisource Provider 都实现同一个 Application Port；组合根只需
/// 注入此路由器即可，Worker 不需要知道具体 Infrastructure 类型。
pub struct RoutingRemoteAcquisitionPort {
    opds: Arc<OpdsCatalogProvider>,
    online: Arc<dyn RemoteAcquisitionPort>,
    /// 用户登记的自托管漫画库（Komga/Kavita）获取端口。未注入时漫画库下载
    /// 明确失败（fail closed），不会落到固定来源的 `online` 路由器。
    comic_library: Option<Arc<dyn RemoteAcquisitionPort>>,
}

impl RoutingRemoteAcquisitionPort {
    pub fn new(opds: Arc<OpdsCatalogProvider>, online: Arc<dyn RemoteAcquisitionPort>) -> Self {
        Self {
            opds,
            online,
            comic_library: None,
        }
    }

    /// 注入自托管漫画库获取端口（组合根必须与 `SourceImportService` 注入的
    /// 漫画库 Provider 指向同一个实例）。
    pub fn with_comic_library(mut self, provider: Arc<dyn RemoteAcquisitionPort>) -> Self {
        self.comic_library = Some(provider);
        self
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
        } else if matches!(
            source_key,
            haven_application::services::source_import::KOMGA_SOURCE_KEY
                | haven_application::services::source_import::KAVITA_SOURCE_KEY
        ) {
            let provider = self.comic_library.as_ref().ok_or_else(|| {
                AppError::new(
                    "SOURCE_UNAVAILABLE",
                    ErrorKind::Network,
                    "自托管漫画库尚未接入当前运行实例",
                    true,
                )
            })?;
            provider.acquire(source_key, remote_id, destination).await
        } else {
            self.online
                .acquire(source_key, remote_id, destination)
                .await
        }
    }

    /// 受控获取必须**逐分支转发**：若落到端口默认实现，有限速率会被静默绕过。
    async fn acquire_controlled(
        &self,
        source_key: &str,
        remote_id: &str,
        destination: &Path,
        transfer: &RemoteTransferContext<'_>,
    ) -> Result<RemoteAcquiredFile, AppError> {
        if source_key == OPDS_SOURCE_GUTENBERG {
            self.opds
                .acquire_controlled(source_key, remote_id, destination, transfer)
                .await
        } else if matches!(
            source_key,
            haven_application::services::source_import::KOMGA_SOURCE_KEY
                | haven_application::services::source_import::KAVITA_SOURCE_KEY
        ) {
            let provider = self.comic_library.as_ref().ok_or_else(|| {
                AppError::new(
                    "SOURCE_UNAVAILABLE",
                    ErrorKind::Network,
                    "自托管漫画库尚未接入当前运行实例",
                    true,
                )
            })?;
            provider
                .acquire_controlled(source_key, remote_id, destination, transfer)
                .await
        } else {
            self.online
                .acquire_controlled(source_key, remote_id, destination, transfer)
                .await
        }
    }
}

/// Routes remote Reader sessions to the provider that owns the source key.
/// Keeping this router in infrastructure means `SessionService` and the Tauri
/// protocol only depend on the narrow `RemoteSessionPort` contract.
pub struct RoutingRemoteSessionPort {
    opds: Arc<OpdsCatalogProvider>,
    online: Arc<dyn RemoteSessionPort>,
}

impl RoutingRemoteSessionPort {
    pub fn new(opds: Arc<OpdsCatalogProvider>, online: Arc<dyn RemoteSessionPort>) -> Self {
        Self { opds, online }
    }
}

#[async_trait]
impl RemoteSessionPort for RoutingRemoteSessionPort {
    async fn read(
        &self,
        source_key: &str,
        remote_id: &str,
        range: Option<RemoteByteRange>,
    ) -> Result<RemoteSessionBody, AppError> {
        if source_key == OPDS_SOURCE_GUTENBERG {
            self.opds.read(source_key, remote_id, range).await
        } else {
            self.online.read(source_key, remote_id, range).await
        }
    }
}

/// 路由远端漫画页 Provider：固定来源（MangaDex）与用户登记的漫画库各自处理
/// 自己的 source_key，`ComicPageService` 只依赖一个窄端口。
pub struct RoutingRemoteComicPageProvider {
    online: Arc<dyn haven_application::services::comic::RemoteComicPageProvider>,
    comic_library: Arc<dyn haven_application::services::comic::RemoteComicPageProvider>,
}

impl RoutingRemoteComicPageProvider {
    pub fn new(
        online: Arc<dyn haven_application::services::comic::RemoteComicPageProvider>,
        comic_library: Arc<dyn haven_application::services::comic::RemoteComicPageProvider>,
    ) -> Self {
        Self {
            online,
            comic_library,
        }
    }

    fn owner(
        &self,
        session: &haven_application::services::session::PreparedSession,
    ) -> &Arc<dyn haven_application::services::comic::RemoteComicPageProvider> {
        match &session.source {
            haven_application::services::session::PreparedSessionSource::Remote {
                source_key,
                ..
            } if matches!(
                source_key.as_str(),
                haven_application::services::source_import::KOMGA_SOURCE_KEY
                    | haven_application::services::source_import::KAVITA_SOURCE_KEY
            ) =>
            {
                &self.comic_library
            }
            _ => &self.online,
        }
    }
}

#[async_trait]
impl haven_application::services::comic::RemoteComicPageProvider
    for RoutingRemoteComicPageProvider
{
    async fn inspect(
        &self,
        session: &haven_application::services::session::PreparedSession,
    ) -> Result<Vec<haven_application::services::comic::PreparedComicPage>, AppError> {
        self.owner(session).inspect(session).await
    }

    async fn read_page(
        &self,
        session: &haven_application::services::session::PreparedSession,
        page: &haven_application::services::comic::PreparedComicPage,
    ) -> Result<haven_application::services::comic::ComicPageBody, AppError> {
        self.owner(session).read_page(session, page).await
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

/// 用户登记目录（OPDS/CMS10）共用的受控 Artwork 投影。
///
/// 搜索阶段只做两件事：把上游响应里**已经存在**的图片字段交给既有的
/// `ImageProxyRepository::register`（URL 形状、主机与长度策略都在那里），并把返回的
/// opaque id 投影成 `haven://artwork/<id>`。搜索**不会**为每条结果额外抓一张图——抓取由
/// 渲染时的受控 Artwork 缓存负责，登记本身不发起任何图片请求。
///
/// 这里刻意不携带目录凭据：要求 Cookie / API key 才能取到的图片在下游抓取时会诚实地失败
/// 并回退到无海报，而不是把目录 secret 交给图片路径。缺失、被策略拒绝或登记失败都只让
/// 卡片少一张海报，元数据照常保留。
pub(crate) async fn register_catalog_artwork(
    proxy: Option<&dyn ImageProxyRepository>,
    source_id: &str,
    poster: Option<&str>,
) -> Option<String> {
    let (Some(proxy), Some(poster)) = (proxy, poster) else {
        return None;
    };
    let poster = poster.trim();
    if poster.is_empty() {
        return None;
    }
    let id = proxy.register(source_id, poster).await.ok()?;
    controlled_artwork_uri(&id)
}

/// 搜索卡片的海报投影（OPDS 与 CMS10 共用的唯一口径）。
///
/// 取消之后不再新增登记：已登记过的 opaque 身份不会被删除（已保存的库记录可能引用
/// 它们），但取消之后的条目不再产生新的映射。缺地址 / 被策略拒绝 / 登记失败都只是
/// 「这张卡片没有海报」，元数据保持原样。
pub(crate) async fn projected_poster(
    proxy: Option<&dyn ImageProxyRepository>,
    source_id: &str,
    pic: Option<&str>,
    is_cancelled: &(dyn Fn() -> bool + Send + Sync),
) -> Option<String> {
    if is_cancelled() {
        return None;
    }
    register_catalog_artwork(proxy, source_id, pic).await
}

/// 受控 Artwork URI：`haven://artwork/<opaque>`。字符集与 `mapper::work_card` 的
/// `controlled_artwork_uri` 以及前端的 `artworkRequestUri` 保持一致；形状不合法的 id
/// 一律不进入 Wire。
pub(crate) fn controlled_artwork_uri(id: &str) -> Option<String> {
    let id = id.trim();
    if id.is_empty()
        || id.len() > 64
        || !id
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
    {
        return None;
    }
    Some(format!("haven://artwork/{id}"))
}

/// 每个 OPDS 来源一个参与者实例；分类声明为 Book。
pub struct OpdsSearchParticipant {
    source_id: String,
    registry: SourceRegistryService,
    client: Arc<OpdsClient>,
    /// 受控图片登记端口。未注入时搜索照常工作，只是没有海报。
    image_proxy: Option<Arc<dyn ImageProxyRepository>>,
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
            image_proxy: None,
        }
    }

    /// 注入受控 Artwork 登记端口（`image_proxy` migration 015 的 ImageProxyRepository）。
    pub fn with_image_proxy(mut self, proxy: Arc<dyn ImageProxyRepository>) -> Self {
        self.image_proxy = Some(proxy);
        self
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
        // 只有内置 OPDS ID，以及**确实登记为自定义 OPDS 书源**的 ID 才参与：订阅源 /
        // 漫画库 / TVBox 配置家族与未登记 ID 安静跳过，一次请求都不发（组合缺口，不是告警）。
        if !is_opds_source_id(dispatched_id)
            && !matches!(
                SourceRegistryService::custom_kind_from_source_id(dispatched_id),
                Some(CustomSourceKind::Opds)
            )
        {
            return Ok(Vec::new());
        }
        let Some(endpoint) = self.registry.endpoint(dispatched_id).await? else {
            return Ok(Vec::new());
        };
        if is_cancelled() {
            return Ok(Vec::new());
        }
        let entries = self
            .client
            .search_entries(Some(dispatched_id), &endpoint, query, limit)
            .await?;
        let mut cards: Vec<haven_application::wire::WorkCardDto> =
            Vec::with_capacity(entries.len());
        for entry in entries {
            // 图片来源身份用**分发到的来源 ID**（后端生成的自定义身份或内置 ID），
            // 与卡片/候选句柄的来源一致。
            let poster_uri = projected_poster(
                self.image_proxy.as_deref(),
                dispatched_id,
                entry.pic.as_deref(),
                is_cancelled,
            )
            .await;
            cards.push(opds_card(dispatched_id, entry, poster_uri));
        }
        Ok(cards)
    }
}

/// 单条 OPDS 条目 → 搜索卡片。
///
/// 候选句柄里绝不出现可直接调用的远端地址：来源键只用于后端路由，条目身份先做百分号
/// 编码，只有导入服务在来源策略校验之后才会解码。海报只接收已经登记过的受控 URI
/// （`None` = 没有海报）。
fn opds_card(
    dispatched_id: &str,
    entry: OpdsEntry,
    poster_uri: Option<String>,
) -> haven_application::wire::WorkCardDto {
    haven_application::wire::WorkCardDto {
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
        poster_uri,
        backdrop_uri: None,
        release_year: None,
        rating_value: None,
        rating_scale: None,
        favorite: false,
        progress: None,
        primary_action: None,
        external_ids: Vec::new(),
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
        // 用户登记的 RSS/Atom 订阅源属于 `custom_` 家族，但不是 OPDS 书源。它们的
        // 条目身份与地址形态与 OPDS 完全不同，绝不能落到下面的 OPDS 兜底：那会把
        // 订阅条目身份当成 OPDS 条目地址去请求。订阅源候选由
        // `SourceImportService` 显式注入的订阅源 Provider 处理，这里 fail closed。
        if SourceRegistryService::is_feed_source_id(source_id) {
            return Err(AppError::new(
                "INVALID_ARGUMENT",
                ErrorKind::Validation,
                "未知来源目录",
                false,
            ));
        }
        // 用户登记的自托管漫画库（Komga/Kavita）同样属于 `custom_` 家族，但既不是
        // OPDS 书源、也不是订阅源。它们的目录/章节/页面身份由专用 Provider 按动态
        // sourceId 处理（`SourceImportService` 显式注入），这里 fail closed，避免把
        // 漫画库身份当成 OPDS 条目地址去请求。
        if SourceRegistryService::is_comic_library_source_id(source_id) {
            return Err(AppError::new(
                "INVALID_ARGUMENT",
                ErrorKind::Validation,
                "未知来源目录",
                false,
            ));
        }
        // 用户导入的 TVBox / FongMi 配置来源同样属于 `custom_` 家族，但它既不是 OPDS 书源、
        // 也没有任何已实现的搜索/播放路径。它必须在**路由层**就被排除，而不是让 OPDS 兜底
        // 去请求一个配置文件地址（客户端也会按登记种类 fail closed，这里是第二道闸门）。
        if SourceRegistryService::is_tvbox_source_id(source_id) {
            return Err(AppError::new(
                "INVALID_ARGUMENT",
                ErrorKind::Validation,
                "未知来源目录",
                false,
            ));
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

    async fn comic_chapter_catalog(
        &self,
        source_id: &str,
        endpoint: &str,
        external_id: &str,
    ) -> Result<Option<ComicChapterCatalog>, AppError> {
        self.route(source_id)?
            .comic_chapter_catalog(source_id, endpoint, external_id)
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
    use super::fixture::{
        ArtworkOutcome, FixtureResponse, FixtureServer, RecordingImageProxy,
        closed_loopback_origin, counting_resolver, opds_entry_with_poster, provider_endpoint,
        seeded_opds_registry,
    };
    use super::*;
    use std::io::{Cursor, Write as _};
    use zip::write::SimpleFileOptions;
    use zip::{CompressionMethod, ZipWriter};

    #[test]
    fn expands_opensearch_optional_terms_and_paging_without_literal_braces() {
        let expanded = OpdsClient::apply_template(
            "search?q={searchTerms?}&count={count?}&start={startIndex}&page={startPage?}&extra={vendor:unused?}",
            "a & 中",
            "https://catalog.example.invalid/opensearch/description.xml",
        ).unwrap();
        let url = reqwest::Url::parse(&expanded).unwrap();
        let pairs: std::collections::HashMap<_, _> = url.query_pairs().into_owned().collect();
        assert_eq!(url.path(), "/opensearch/search");
        assert_eq!(pairs.get("q").map(String::as_str), Some("a & 中"));
        assert_eq!(pairs.get("count").map(String::as_str), Some("20"));
        assert_eq!(pairs.get("start").map(String::as_str), Some("1"));
        assert_eq!(pairs.get("page").map(String::as_str), Some("1"));
        assert_eq!(pairs.get("extra").map(String::as_str), Some(""));
        for template in ["?q={unknown}", "?q={searchTerms", "?q=}", "?q={?}"] {
            assert!(OpdsClient::apply_template(template, "q", "https://catalog.example.invalid/").is_none());
        }
    }

    #[tokio::test]
    async fn cached_opensearch_template_uses_the_same_expansion_and_reports_invalid_required_variables() {
        let endpoint = "https://catalog.example.invalid/opds";
        let (registry, source) = seeded_opds_registry(endpoint, false).await;
        let client = OpdsClient::new().unwrap().with_source_registry(registry);
        let context = client.request_context(Some(&source)).await.unwrap();
        client.remember_template(endpoint, "?q={searchTerms?}&count={count?}".into(), endpoint.into());
        let url = client.search_url_with(&context, endpoint, "test").await.unwrap();
        assert!(url.contains("q=test&count=20"));
        client.remember_template(endpoint, "?q={unknownRequired}".into(), endpoint.into());
        assert!(client.search_url_with(&context, endpoint, "test").await.is_err());
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

    #[tokio::test]
    async fn custom_opds_candidates_stay_on_the_registered_authority() {
        let (registry, source) =
            seeded_opds_registry("https://catalog.example.invalid/opds", false).await;
        let client = OpdsClient::new().unwrap().with_source_registry(registry);
        let context = client.request_context(Some(&source)).await.unwrap();
        assert!(client.is_source_url_allowed(
            &context,
            "https://catalog.example.invalid/opds",
            "https://catalog.example.invalid/books/84.opds"
        ));
        assert!(!client.is_source_url_allowed(
            &context,
            "https://catalog.example.invalid/opds",
            "https://other.example.invalid/books/84.opds"
        ));
        assert!(!client.is_source_url_allowed(
            &context,
            "https://catalog.example.invalid/opds",
            "https://user:secret@catalog.example.invalid/books/84.opds"
        ));
        // 内置 Gutenberg 继续使用它自己的两主机例外。
        let gutenberg = OpdsRequestContext::builtin_gutenberg(OPDS_SOURCE_GUTENBERG);
        assert!(client.is_source_url_allowed(
            &gutenberg,
            "https://www.gutenberg.org/ebooks/search.opds",
            "https://m.gutenberg.org/ebooks/84.opds"
        ));
        assert!(!client.is_source_url_allowed(
            &gutenberg,
            "https://www.gutenberg.org/ebooks/search.opds",
            "https://evil.example.invalid/ebooks/84.opds"
        ));
        // 已登记的自定义书源可以使用任意非零端口：端口是用户自己的选择，
        // 但**别的**端口仍然不是同一个权威。
        let (port_registry, port_source) =
            seeded_opds_registry("https://catalog.example.invalid:8096/opds", false).await;
        let port_client = OpdsClient::new()
            .unwrap()
            .with_source_registry(port_registry);
        let port_context = port_client
            .request_context(Some(&port_source))
            .await
            .unwrap();
        assert!(port_client.is_source_url_allowed(
            &port_context,
            "https://catalog.example.invalid:8096/opds",
            "https://catalog.example.invalid:8096/books/84.opds"
        ));
        assert!(!port_client.is_source_url_allowed(
            &port_context,
            "https://catalog.example.invalid:8096/opds",
            "https://catalog.example.invalid:8097/books/84.opds"
        ));
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

    #[test]
    fn online_epub_session_returns_complete_or_bounded_ranges() {
        let body = bounded_epub_session_body(b"0123456789".to_vec(), None).unwrap();
        assert_eq!(body.mime_type, EPUB_MIME);
        assert_eq!(body.bytes, b"0123456789");
        assert_eq!(body.total_size, 10);
        assert!(body.content_range.is_none());
        assert!(body.accept_ranges);

        let body = bounded_epub_session_body(
            b"0123456789".to_vec(),
            Some(RemoteByteRange {
                start: 2,
                end: Some(5),
            }),
        )
        .unwrap();
        assert_eq!(body.bytes, b"2345");
        assert_eq!(
            body.content_range,
            Some(RemoteContentRange {
                start: 2,
                end: 5,
                total: 10,
            })
        );
    }

    #[test]
    fn online_epub_session_rejects_invalid_ranges() {
        for range in [
            RemoteByteRange {
                start: 10,
                end: None,
            },
            RemoteByteRange {
                start: 5,
                end: Some(4),
            },
        ] {
            assert!(bounded_epub_session_body(b"0123456789".to_vec(), Some(range)).is_err());
        }
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
    <title>Direct &amp; Download Book</title>
    <author><name>Anon &amp; Co</name></author>
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
        assert_eq!(books[1].title, "Direct & Download Book");
        assert_eq!(books[1].author.as_deref(), Some("Anon & Co"));
    }

    // ---- 自定义源凭据与用户来源请求上下文 ----
    //
    // 回环夹具、来源登记夹具与凭据解析器替身都在 `super::fixture`（`cfg(test)`），
    // 与 CMS10 适配器的测试共用同一份；这里只保留请求上下文本身的用例。

    #[tokio::test]
    async fn credentials_are_bound_to_the_registered_origin_and_never_look_up_foreign_hops() {
        // 登记端点是 HTTPS（配了凭据的来源只允许 HTTPS）：这里只需要 origin 判定，
        // 不做任何真实连接。
        let secure_origin = closed_loopback_origin().replacen("http://", "https://", 1);
        let (registry, source_id) =
            seeded_opds_registry(&provider_endpoint(&secure_origin), true).await;
        let calls = Arc::new(std::sync::Mutex::new(Vec::new()));
        let client = OpdsClient::new()
            .unwrap()
            .with_source_registry(registry)
            .with_credential_resolver(counting_resolver(calls.clone()));

        let authoritative = reqwest::Url::parse(&format!("{secure_origin}/feed")).unwrap();
        let context = client
            .request_context(Some(&source_id))
            .await
            .expect("已登记的自定义 OPDS 书源必须得到上下文");
        assert_eq!(
            hop_credential(&context, &authoritative).unwrap(),
            HopCredential::Authorized,
            "权威登记 origin 上的一跳允许带凭据"
        );
        assert_eq!(
            client
                .resolve_credential(&context, &authoritative)
                .await
                .unwrap()
                .as_deref(),
            Some("p@ss:word")
        );
        // 这里只验证凭据引用归属；HTTP Basic 用户名由版本化凭据载荷提供。
        assert_eq!(calls.lock().unwrap().clone(), vec![source_id.clone()]);

        // 另一个 origin（被篡改的目录链接/模板/重定向）不许继承凭据：连解析都不做。
        let foreign = reqwest::Url::parse(&format!("{}/feed", closed_loopback_origin())).unwrap();
        assert_eq!(
            hop_credential(&context, &foreign).unwrap(),
            HopCredential::Anonymous
        );
        assert!(
            client
                .resolve_credential(&context, &foreign)
                .await
                .unwrap()
                .is_none(),
            "跨 origin 的一跳不得解析 secret"
        );
        assert_eq!(
            calls.lock().unwrap().len(),
            1,
            "跨 origin 的一跳不得再次调用凭据解析器"
        );

        // 明文 HTTP 上的权威 origin：在发出请求之前就以固定错误 fail closed，
        // 而且连 secret 都不会去解析。
        let plaintext = closed_loopback_origin();
        let (plain_registry, plain_source) =
            seeded_opds_registry(&provider_endpoint(&plaintext), true).await;
        let plain_client = OpdsClient::new()
            .unwrap()
            .with_loopback_fixture()
            .with_source_registry(plain_registry)
            .with_credential_resolver(counting_resolver(calls.clone()));
        let plain_context = plain_client
            .request_context(Some(&plain_source))
            .await
            .unwrap();
        let error = plain_client
            .get_feed(&plain_context, &format!("{plaintext}/feed"))
            .await
            .expect_err("配了凭据的来源不得在明文 HTTP 上发请求");
        assert_eq!(error.code().as_str(), "SECURITY_POLICY_DENIED");
        assert_eq!(
            calls.lock().unwrap().len(),
            1,
            "明文拒绝发生在解析 secret 之前"
        );
    }

    /// 越界的一跳：请求地址不属于**登记端点 origin**，必须在建立任何连接之前 fail closed。
    /// 夹具服务器记录被接受的 TCP 连接数，因此「有没有真的发出请求」是一个可断言的事实，
    /// 而不是靠时序猜测。
    #[tokio::test]
    async fn foreign_origin_requests_never_carry_catalog_credentials() {
        let server = FixtureServer::start(|_| {
            vec![(
                "/feed".to_owned(),
                FixtureResponse::atom(r#"<feed xmlns="http://www.w3.org/2005/Atom"/>"#),
            )]
        });
        let url = format!("{}/feed", server.origin());
        // 登记端点在**另一个**回环 origin 上：本次请求不是权威 origin，必须 fail closed，
        // 绝不能把目录凭据发给一个不属于该来源的地址。
        let (registry, source_id) =
            seeded_opds_registry(&provider_endpoint(&closed_loopback_origin()), true).await;
        let calls = Arc::new(std::sync::Mutex::new(Vec::new()));
        let client = OpdsClient::new()
            .unwrap()
            .with_loopback_fixture()
            .with_source_registry(registry)
            .with_credential_resolver(counting_resolver(calls.clone()));

        let context = client.request_context(Some(&source_id)).await.unwrap();
        let error = client
            .get_feed(&context, &url)
            .await
            .expect_err("越界的一跳必须 fail closed");
        assert_eq!(error.code().as_str(), "SOURCE_UNAVAILABLE");
        assert!(
            calls.lock().unwrap().is_empty(),
            "越界的一跳不得解析 secret"
        );
        assert_eq!(server.connections(), 0, "越界的一跳不得建立任何 socket");
    }

    #[tokio::test]
    async fn anonymous_and_builtin_requests_stay_anonymous() {
        let server = FixtureServer::start(|_| {
            vec![(
                "/feed".to_owned(),
                FixtureResponse::atom(r#"<feed xmlns="http://www.w3.org/2005/Atom"/>"#),
            )]
        });
        let url = format!("{}/feed", server.origin());
        let (registry, _source_id) =
            seeded_opds_registry(&provider_endpoint(server.origin()), true).await;
        let calls = Arc::new(std::sync::Mutex::new(Vec::new()));
        let client = OpdsClient::new()
            .unwrap()
            .with_loopback_fixture()
            .with_source_registry(registry)
            .with_credential_resolver(counting_resolver(calls.clone()));

        // 匿名请求（没有来源身份）：即使注入了注册表与解析器，也不解析凭据。
        let anonymous = OpdsRequestContext::anonymous();
        client
            .get_feed(&anonymous, &url)
            .await
            .expect("匿名请求照常可用");

        // 内置 Gutenberg：身份固定为内置，凭据解析永远不参与。
        let builtin = OpdsRequestContext::builtin_gutenberg(OPDS_SOURCE_GUTENBERG);
        client
            .get_feed(&builtin, &url)
            .await
            .expect("内置来源照常可用");

        assert_eq!(server.connections(), 2, "两次请求各建立一次连接");
        assert_eq!(server.requests().len(), 2);
        for request in server.requests() {
            assert_eq!(
                request.header("authorization"),
                None,
                "匿名/内置请求必须不带凭据"
            );
        }
        assert!(calls.lock().unwrap().is_empty(), "内置/匿名不得解析凭据");
    }

    #[tokio::test]
    async fn wrong_family_and_unknown_ids_never_reach_the_credential_resolver() {
        let calls = Arc::new(std::sync::Mutex::new(Vec::new()));
        let (registry, _opds_source) =
            seeded_opds_registry(&provider_endpoint(&closed_loopback_origin()), true).await;
        let client = OpdsClient::new()
            .unwrap()
            .with_loopback_fixture()
            .with_source_registry(registry)
            .with_credential_resolver(counting_resolver(calls.clone()));

        // 其它 `custom_` 家族与未登记/家族外 ID：fail closed，绝不解析凭据，也绝不因为
        // 名字看起来像就拿到用户来源策略或登记端点。
        for source_id in [
            "custom_feed_0123456789ab",
            "custom_komga_0123456789ab",
            "custom_kavita_0123456789ab",
            "custom_tvbox_0123456789ab",
            "custom_unknown000000",
            "mangadex",
            "custom_",
        ] {
            let denied = client.request_context(Some(source_id)).await;
            assert!(denied.is_err(), "{source_id} 不得得到用户来源上下文");
            assert_eq!(
                denied.unwrap_err().code().as_str(),
                "INVALID_ARGUMENT",
                "{source_id} 必须以未知来源 fail closed"
            );
        }
        assert!(
            calls.lock().unwrap().is_empty(),
            "错误家族/未知身份一次都不许调用凭据解析器"
        );
        // 同一个客户端对**已登记**的 OPDS 来源仍然工作：上面的拒绝不是「客户端整体失效」。
        let (ok_registry, ok_source) =
            seeded_opds_registry(&provider_endpoint(&closed_loopback_origin()), true).await;
        let ok_client = OpdsClient::new()
            .unwrap()
            .with_source_registry(ok_registry)
            .with_credential_resolver(counting_resolver(calls.clone()));
        assert!(ok_client.request_context(Some(&ok_source)).await.is_ok());
    }

    #[tokio::test]
    async fn context_change_during_credential_resolution_fails_closed() {
        // 登记端点是 HTTPS：这一跳本来会注入凭据，因此必须走到解析 + 复核那一步。
        let secure_origin = closed_loopback_origin().replacen("http://", "https://", 1);
        let (registry, source_id) =
            seeded_opds_registry(&provider_endpoint(&secure_origin), true).await;
        // 解析器在 await 期间被暂停；测试借这段时间把来源端点改掉。
        let (resolver, entered, release) = super::fixture::paused_resolver(Some("p@ss:word"));
        let client = Arc::new(
            OpdsClient::new()
                .unwrap()
                .with_loopback_fixture()
                .with_source_registry(registry.clone())
                .with_credential_resolver(resolver),
        );
        let context = client
            .request_context(Some(&source_id))
            .await
            .expect("已登记的自定义 OPDS 书源必须得到上下文");
        let url = format!("{secure_origin}/feed");

        let task = tokio::spawn({
            let client = client.clone();
            let context = context.clone();
            let url = url.clone();
            async move { client.get_feed(&context, &url).await }
        });
        entered.await.expect("解析器必须被调用并暂停");

        // 解析期间来源被改到另一个端点（配置版本随之作废）。
        registry
            .update_custom_source(haven_application::wire::SourceUpdateRequest {
                source_id: source_id.clone(),
                display_name: None,
                endpoint: Some("https://other.example.invalid/opds".to_owned()),
            })
            .await
            .unwrap();
        let _ = release.send(());

        let error = task
            .await
            .unwrap()
            .expect_err("上下文在解析期间变化时必须 fail closed");
        assert_eq!(error.code().as_str(), "SOURCE_UNAVAILABLE");
        assert!(
            error.user_message().contains("变更"),
            "必须是可识别的上下文失效错误，实际: {}",
            error.user_message()
        );
        // 旧上下文不再权威：解析出来的 secret 没有被用于任何请求。
        let refreshed = client
            .request_context(Some(&source_id))
            .await
            .expect("来源仍登记为自定义 OPDS 书源");
        assert!(!context.is_current_with(&refreshed));
    }

    // ---- 用户登记目录的受控 Artwork 投影 ----
    //
    // 登记结果替身与 OPDS 条目构造器在 `super::fixture`；整条搜索链路的用例在
    // `super::catalog_tests`（它们需要一个真实的离线目录服务器，而不仅是 helper）。

    #[tokio::test]
    async fn opds_search_cards_project_only_registered_controlled_artwork() {
        let proxy = RecordingImageProxy::new(ArtworkOutcome::Registered);
        let not_cancelled = || false;
        let source_id = "custom_0123456789ab";
        let pic = "https://catalog.example.invalid/covers/9.png";

        let poster =
            projected_poster(Some(proxy.as_ref()), source_id, Some(pic), &not_cancelled).await;
        assert_eq!(poster.as_deref(), Some("haven://artwork/artwork-1"));
        // 登记用的是**分发到的来源身份**与上游解析出来的绝对图片地址。
        assert_eq!(proxy.calls(), vec![(source_id.to_owned(), pic.to_owned())]);

        let card = opds_card(
            source_id,
            opds_entry_with_poster("https://catalog.example.invalid/books/9.opds", Some(pic)),
            poster,
        );
        assert_eq!(
            card.poster_uri.as_deref(),
            Some("haven://artwork/artwork-1")
        );
        assert!(
            card.work_id
                .starts_with(haven_application::services::OPDS_CANDIDATE_PREFIX)
        );
        // Wire 上只有 opaque 地址：远端图片地址不出现。
        let wire = serde_json::to_string(&card).unwrap();
        assert!(wire.contains("haven://artwork/artwork-1"));
        assert!(!wire.contains("covers/9.png"));

        // 没有图片字段的条目根本不进入登记路径，卡片保持无海报。
        assert_eq!(
            projected_poster(Some(proxy.as_ref()), source_id, None, &not_cancelled).await,
            None
        );
        assert_eq!(proxy.calls().len(), 1, "缺失的海报不得进入登记路径");
        assert_eq!(
            opds_card(
                source_id,
                opds_entry_with_poster("https://catalog.example.invalid/books/10.opds", None),
                None,
            )
            .poster_uri,
            None
        );
    }

    #[tokio::test]
    async fn opds_search_cancellation_stops_further_artwork_registration() {
        let proxy = RecordingImageProxy::new(ArtworkOutcome::Registered);
        let not_cancelled = || false;
        let cancelled = || true;
        let source_id = "custom_0123456789ab";

        // 取消之前正常登记。
        assert_eq!(
            projected_poster(
                Some(proxy.as_ref()),
                source_id,
                Some("https://catalog.example.invalid/1.png"),
                &not_cancelled,
            )
            .await
            .as_deref(),
            Some("haven://artwork/artwork-1")
        );
        // 取消之后不再登记新的映射，但条目本身仍然可以投影成卡片（元数据保留）。
        assert_eq!(
            projected_poster(
                Some(proxy.as_ref()),
                source_id,
                Some("https://catalog.example.invalid/2.png"),
                &cancelled,
            )
            .await,
            None
        );
        assert_eq!(proxy.calls().len(), 1, "取消之后不得继续登记图片");
        let card = opds_card(
            source_id,
            opds_entry_with_poster(
                "https://catalog.example.invalid/books/2.opds",
                Some("https://catalog.example.invalid/2.png"),
            ),
            None,
        );
        assert_eq!(card.poster_uri, None);
        assert_eq!(card.title, "有封面的书");
    }

    #[tokio::test]
    async fn artwork_projection_is_optional_and_never_echoes_the_upstream_url() {
        // 没有注入端口 / 没有图片字段：不登记、不报错、卡片保持原样。
        let not_cancelled = || false;
        assert_eq!(
            projected_poster(
                None,
                "cms10",
                Some("https://img.example.invalid/p.jpg"),
                &not_cancelled
            )
            .await,
            None
        );
        let proxy = RecordingImageProxy::new(ArtworkOutcome::Registered);
        assert_eq!(
            projected_poster(Some(proxy.as_ref()), "cms10", None, &not_cancelled).await,
            None
        );
        assert_eq!(
            projected_poster(Some(proxy.as_ref()), "cms10", Some("   "), &not_cancelled).await,
            None
        );
        assert!(proxy.calls().is_empty(), "缺失的海报地址不得进入登记路径");

        // 被策略拒绝 / 登记失败：返回 None，错误事实里没有目标地址。
        for outcome in [ArtworkOutcome::Rejected, ArtworkOutcome::Failed] {
            let proxy = RecordingImageProxy::new(outcome);
            let secret = "https://img.example.invalid/p.jpg?token=fixture-secret";
            assert_eq!(
                projected_poster(Some(proxy.as_ref()), "cms10", Some(secret), &not_cancelled).await,
                None,
                "登记失败时海报必须是 None"
            );
            assert_eq!(
                proxy.calls(),
                vec![("cms10".to_owned(), secret.to_owned())],
                "上游地址只进入登记端口，不进 Wire"
            );
        }

        // opaque id 的形状判定：畸形 id 不进入 Wire。
        assert_eq!(
            controlled_artwork_uri("0f7a4a1e-0f2f-4a4f-9c4f-3c1b0f0a4b6c").as_deref(),
            Some("haven://artwork/0f7a4a1e-0f2f-4a4f-9c4f-3c1b0f0a4b6c")
        );
        for rejected in [
            "",
            "../etc",
            "a b",
            "https://evil.example/x.jpg",
            &"a".repeat(65),
        ] {
            assert_eq!(
                controlled_artwork_uri(rejected),
                None,
                "非法 id: {rejected}"
            );
        }
    }

    /// 家族之外 / 未登记的自定义身份：参与者安静跳过，既不登记也不发请求。
    #[tokio::test]
    async fn artwork_is_only_registered_for_the_dispatched_source_identity() {
        let (registry, source_id) =
            seeded_opds_registry("https://catalog.example.invalid/opds", false).await;
        let proxy = RecordingImageProxy::new(ArtworkOutcome::Registered);
        let client = Arc::new(
            OpdsClient::new()
                .unwrap()
                .with_source_registry(registry.clone()),
        );
        let participant =
            OpdsSearchParticipant::new(CUSTOM_OPDS_ID_PREFIX.to_owned(), registry.clone(), client)
                .with_image_proxy(proxy.clone());

        let not_cancelled = || false;
        for foreign in [
            "custom_feed_0123456789ab",
            "custom_komga_0123456789ab",
            "custom_kavita_0123456789ab",
            "custom_tvbox_0123456789ab",
            "custom_unknown000000",
            "mangadex",
        ] {
            assert!(
                participant
                    .search_for(foreign, "任意", 10, &not_cancelled)
                    .await
                    .unwrap()
                    .is_empty(),
                "{foreign} 不得被 OPDS 参与者接管"
            );
        }
        assert!(
            proxy.calls().is_empty(),
            "错误家族/未登记身份不得登记任何图片"
        );
        assert!(
            registry
                .endpoint("custom_unknown000000")
                .await
                .unwrap()
                .is_none(),
            "未登记身份没有端点可用"
        );
        assert!(registry.endpoint(&source_id).await.unwrap().is_some());
    }

    /// 回归（RSS/Atom Task 1）：`custom_feed_` 属于 `custom_` 家族，但订阅源绝不是
    /// OPDS 书源。目录路由器必须 fail closed，而不是把订阅条目身份当作 OPDS 条目
    /// 地址交给 OPDS Provider 去请求。
    #[tokio::test]
    async fn feed_sources_never_fall_back_to_the_opds_catalog() {
        let online: Arc<dyn SourceCatalogProvider> =
            Arc::new(crate::online_sources::OnlineCatalogProvider::new(Arc::new(
                crate::online_sources::OnlineContentClient::new().unwrap(),
            )));
        let router = RoutingSourceCatalogProvider::new(
            Arc::new(Cms10CatalogProvider::new(Arc::new(
                crate::cms10::Cms10Client::new().unwrap(),
            ))),
            Arc::new(OpdsCatalogProvider::new(Arc::new(
                OpdsClient::new().unwrap(),
            ))),
            online,
        );

        assert!(
            router.route("custom_0123456789ab").is_ok(),
            "普通自定义 OPDS 书源仍然走 OPDS 兜底"
        );
        assert!(
            router.route("custom_feed_0123456789ab").is_err(),
            "订阅源不得落到 OPDS 兜底"
        );
        let error = router
            .detail(
                "custom_feed_0123456789ab",
                "",
                "0123456789abcdef0123456789abcdef",
            )
            .await
            .unwrap_err();
        assert_eq!(error.code().as_str(), "INVALID_ARGUMENT");
    }
}
