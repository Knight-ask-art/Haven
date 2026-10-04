//! TVBox / FongMi 配置 URL 的受控获取与预览（纯 Rust 基础设施切片）。
//!
//! 这个模块只做一件事：把一个调用方给出的 HTTP(S) 配置地址，通过仓库既有的出站
//! 安全边界有界地取回正文，交给 [`crate::tvbox_config::parse_config`] 解析，并且
//! **只**返回 [`TvboxConfigSummary`]。它不落库、不缓存、不注册来源、不生成 IPC DTO、
//! 不涉及任何设置页或前端。
//!
//! Intent lock（本切片不越界）：
//! - 只发 GET。请求由本模块自行构造，**不接受任何调用方提供的 header**，因此
//!   Cookie / Authorization 既不会被发送，也不会跨重定向被携带；每一跳都用全新的
//!   client 与全新的 request，重定向前后的请求之间没有共享的 header 状态。
//! - URL 语法与字面地址策略完全复用 `haven_common::network`：每一跳都会重新执行
//!   [`parse_http_url`]（`HttpUrlPolicy::SourceEndpoint`），拒绝非 http(s) scheme、
//!   userinfo、fragment、回环/私网/单标签/`.local`/`.internal` 主机、非白名单端口
//!   （80/443/8080/8443）。本模块不新增加宽或更严的例外。
//! - 每一跳都重新做 DNS 解析与地址固定（[`resolve_public_http_target`] +
//!   [`pin_client_builder`]），因此重定向不会绕过 DNS rebinding 防护；重定向手动
//!   跟随，上限固定且很小，自动重定向与代理都被显式关闭。
//! - 正文大小上限在**流式读取时**执行，而不是只看 `Content-Length`；声明长度超限
//!   的响应不会被读取正文。
//! - 错误、`Debug`、日志里都不出现原始 URL、重定向 Location、响应正文或任何配置值。
//!   所有错误文案要么是固定字符串，要么来自 [`crate::tvbox_config`] 已经过滤过的稳定
//!   错误码；返回值只有已过滤的摘要。
//! - 不解释数字 `type` 语义，不下载/加载/执行配置声明的 JS / JAR / Python，也不调用
//!   任何外部解析服务；这些都在解析层被识别为标记，本模块不改变其结论。
//!
//! 网络访问收在 [`TvboxConfigTransport`] 端口后面：生产实现是
//! [`HttpTvboxConfigTransport`]，测试注入进程内 fake，因此重定向、状态、大小上限与
//! 解析失败语义无需真实网络即可验证。
//!
//! 本模块的入口是 [`fetch_config_preview`]（只回摘要）与 [`fetch_config_bytes`]
//! （一并回原文，供保存用例落库）。Application 侧的预览用例通过
//! [`HttpTvboxConfigPreview`] 消费前者，保存用例通过 [`HttpTvboxConfigImport`] 消费
//! 后者；两个适配器都只做「安全摘要/原文 → Application 事实」的搬运，不会把站点名、
//! header 名/值、`ext` 内容或未识别字段名带出去。原文只流向 SQLite 缓存。

use std::time::Duration;

use async_trait::async_trait;
use reqwest::Url;

use haven_application::services::tvbox_config_import::{TvboxConfigFetch, TvboxConfigImportPort};
use haven_application::services::tvbox_config_preview::{
    TvboxConfigImplementationKind, TvboxConfigPreviewFacts, TvboxConfigPreviewPort,
};
use haven_common::network::{HttpUrlPolicy, parse_http_url};
use haven_common::{AppError, ErrorKind};

use crate::http_security::{pin_client_builder, resolve_public_http_target};
use crate::tvbox_config::{
    MAX_CONFIG_BYTES, TvboxConfigSummary, TvboxImplementationKind, TvboxSiteKind, parse_config,
};

/// 重定向跟随上限（与仓库其它固定来源一致）。
const MAX_REDIRECTS: usize = 3;
/// 单次请求的整体超时。
const REQUEST_TIMEOUT: Duration = Duration::from_secs(10);
/// 连接建立超时。
const CONNECT_TIMEOUT: Duration = Duration::from_secs(3);
/// 固定 User-Agent。刻意不含任何用户可控内容。
const USER_AGENT: &str = concat!(
    "Haven/",
    env!("CARGO_PKG_VERSION"),
    " (tvbox config preview)"
);
/// 固定 Accept。TVBox/FongMi 配置是 JSON 正文。
const ACCEPT: &str = "application/json,text/plain;q=0.9,*/*;q=0.1";

/// 有界获取一个用户的 TVBox / FongMi 配置地址，并只返回安全摘要。
///
/// 成功时返回 [`TvboxConfigSummary`]；任何失败都返回不含 URL、正文或配置值的
/// [`AppError`]。网络与安全失败使用本模块的稳定错误码
/// （`SOURCE_UNAVAILABLE` / `SECURITY_POLICY_DENIED`），解析失败原样透传
/// [`crate::tvbox_config`] 的 `TVBOX_CONFIG_*` 错误码。
pub async fn fetch_config_preview(url: &str) -> Result<TvboxConfigSummary, AppError> {
    fetch_config_preview_with(&HttpTvboxConfigTransport, url).await
}

/// 与 [`fetch_config_preview`] 走完全相同的受控路径，但把**原始字节**一并交回。
///
/// 只有保存/导入用例需要原文（它要把 last-known-good 落进 SQLite）；预览只需要摘要。
/// 两条路径共用同一个实现，因此 URL 策略、逐跳 DNS 固定、重定向上限、大小上限与
/// 解析规则不会分叉。调用方必须把返回的字节当作不可信数据：不得写入日志、错误文案
/// 或 IPC。
pub(crate) async fn fetch_config_bytes(
    url: &str,
) -> Result<(Vec<u8>, TvboxConfigSummary), AppError> {
    fetch_config_bytes_with(&HttpTvboxConfigTransport, url).await
}

/// 单跳传输端口。实现负责一次 GET 的网络、DNS 解析与地址固定；重定向跟随、状态、
/// 大小上限与解析由 [`fetch_config_preview_with`] 统一负责。
#[async_trait]
trait TvboxConfigTransport: Send + Sync {
    /// 对已经过 URL 策略校验的地址发起一次 GET。
    async fn get(&self, url: &str) -> Result<TvboxHopResponse, AppError>;
}

/// 一次 GET 的响应元数据与流式正文。刻意不实现 `Debug`：`location` 与正文都属于
/// 远端配置，不能出现在日志里。
struct TvboxHopResponse {
    status: u16,
    /// 重定向目标原文；只在状态码为 3xx 时被使用。
    location: Option<String>,
    /// 远端声明的正文长度（可能缺失，例如 chunked）。
    content_length: Option<u64>,
    body: Box<dyn TvboxBodyStream>,
}

/// 逐块读取的正文端口，让大小上限可以在**流式**过程中执行。
#[async_trait]
trait TvboxBodyStream: Send {
    /// 返回下一块；`None` 表示正文结束。
    async fn next_chunk(&mut self) -> Result<Option<Vec<u8>>, AppError>;
}

/// 重定向循环 + 大小上限 + 解析。传输通过端口注入，便于确定性测试。
async fn fetch_config_preview_with<T>(
    transport: &T,
    raw_url: &str,
) -> Result<TvboxConfigSummary, AppError>
where
    T: TvboxConfigTransport + ?Sized,
{
    fetch_config_bytes_with(transport, raw_url)
        .await
        .map(|(_, summary)| summary)
}

/// [`fetch_config_preview_with`] 的实现体：成功时同时返回正文原文与安全摘要。
async fn fetch_config_bytes_with<T>(
    transport: &T,
    raw_url: &str,
) -> Result<(Vec<u8>, TvboxConfigSummary), AppError>
where
    T: TvboxConfigTransport + ?Sized,
{
    let mut current = validate_request_url(raw_url)?;
    for _ in 0..=MAX_REDIRECTS {
        let mut response = transport.get(current.as_str()).await?;
        if is_redirection(response.status) {
            let location = response
                .location
                .as_deref()
                .ok_or_else(|| fetch_unavailable("TVBox 配置重定向地址无效"))?;
            let next = current
                .join(location)
                .map_err(|_| fetch_unavailable("TVBox 配置重定向地址无效"))?;
            // 每一跳都重跑完整 URL 策略；DNS 部分由传输实现在建立连接前完成。
            current = validate_request_url(next.as_str())?;
            continue;
        }
        if !is_success(response.status) {
            return Err(fetch_unavailable("TVBox 配置返回异常状态"));
        }
        if response
            .content_length
            .is_some_and(|length| length > MAX_CONFIG_BYTES as u64)
        {
            return Err(fetch_unavailable("TVBox 配置响应超出大小上限"));
        }
        let body = read_bounded_body(&mut *response.body).await?;
        // 解析错误已经是不含配置值的稳定错误码，原样透传。
        let config = parse_config(&body)?;
        return Ok((body, config.summary()));
    }
    Err(fetch_unavailable("TVBox 配置重定向次数过多"))
}

/// 按 [`MAX_CONFIG_BYTES`] 流式累积正文；超限立即失败，不继续读取剩余块。
async fn read_bounded_body(body: &mut dyn TvboxBodyStream) -> Result<Vec<u8>, AppError> {
    let mut buffer = Vec::with_capacity(64 * 1024);
    while let Some(chunk) = body.next_chunk().await? {
        if buffer.len().saturating_add(chunk.len()) > MAX_CONFIG_BYTES {
            return Err(fetch_unavailable("TVBox 配置响应超出大小上限"));
        }
        buffer.extend_from_slice(&chunk);
    }
    Ok(buffer)
}

/// 施加 `HttpUrlPolicy::SourceEndpoint` 的语法与字面地址策略，并规范化成可 join 的 URL。
fn validate_request_url(raw: &str) -> Result<Url, AppError> {
    parse_http_url(raw, HttpUrlPolicy::SourceEndpoint)
        .map(|safe| safe.into_url())
        .map_err(|_| security_denied("TVBox 配置地址不安全"))
}

/// 与 `reqwest::StatusCode::is_redirection` 等价的 3xx 判断。
fn is_redirection(status: u16) -> bool {
    (300..400).contains(&status)
}

fn is_success(status: u16) -> bool {
    (200..300).contains(&status)
}

/// 生产传输实现：每跳重新解析并固定 DNS，只发 GET，关闭自动重定向与代理。
struct HttpTvboxConfigTransport;

#[async_trait]
impl TvboxConfigTransport for HttpTvboxConfigTransport {
    async fn get(&self, url: &str) -> Result<TvboxHopResponse, AppError> {
        let target = resolve_public_http_target(url, HttpUrlPolicy::SourceEndpoint)
            .await
            .map_err(|_| security_denied("TVBox 配置地址解析不安全"))?;
        let builder = reqwest::Client::builder()
            .connect_timeout(CONNECT_TIMEOUT)
            .timeout(REQUEST_TIMEOUT)
            // 在 Windows 上保持行为可预测，同时保留 HTTPS 证书校验。
            .http1_only()
            // 重定向由上面的循环显式跟随，每一跳都重新校验。
            .redirect(reqwest::redirect::Policy::none())
            .user_agent(USER_AGENT);
        // `pin_client_builder` 同时设置 `no_proxy()`，并让 `target.addresses` 成为
        // 唯一的连接目的地。
        let client = pin_client_builder(builder, &target)
            .build()
            .map_err(|_| internal_error("TVBox 配置客户端初始化失败"))?;
        let response = client
            .get(target.url.clone())
            .header(reqwest::header::ACCEPT, ACCEPT)
            .send()
            .await
            .map_err(|_| fetch_unavailable("TVBox 配置暂时不可达"))?;
        let status = response.status().as_u16();
        let location = response
            .headers()
            .get(reqwest::header::LOCATION)
            .and_then(|value| value.to_str().ok())
            .map(str::to_owned);
        let content_length = response.content_length();
        Ok(TvboxHopResponse {
            status,
            location,
            content_length,
            body: Box::new(ReqwestBody { response }),
        })
    }
}

/// 把 `reqwest::Response` 适配成逐块端口。
struct ReqwestBody {
    response: reqwest::Response,
}

#[async_trait]
impl TvboxBodyStream for ReqwestBody {
    async fn next_chunk(&mut self) -> Result<Option<Vec<u8>>, AppError> {
        self.response
            .chunk()
            .await
            .map(|chunk| chunk.map(|bytes| bytes.to_vec()))
            .map_err(|_| fetch_unavailable("TVBox 配置读取中断"))
    }
}

// ---------- Application 预览端口 ----------

/// [`TvboxConfigPreviewPort`] 的生产实现。
///
/// 它不新增任何网络策略：受控 GET、逐跳 DNS 固定、重定向上限、响应大小上限与解析
/// 全部复用 [`fetch_config_preview`]，这里只把已经过滤过的摘要搬运成 Application 事实。
pub struct HttpTvboxConfigPreview;

#[async_trait]
impl TvboxConfigPreviewPort for HttpTvboxConfigPreview {
    async fn fetch_preview(&self, url: &str) -> Result<TvboxConfigPreviewFacts, AppError> {
        let summary = fetch_config_preview(url).await?;
        Ok(preview_facts(&summary))
    }
}

// ---------- Application 导入端口 ----------

/// [`TvboxConfigImportPort`] 的生产实现（保存/导入路径）。
///
/// 与预览适配器共用 [`fetch_config_bytes`]：同一套 URL 策略、逐跳 DNS 固定、重定向
/// 上限、大小上限与解析规则，只是额外把**原文**交回 Application。原文只允许流向
/// SQLite 缓存，不得进入 IPC、日志或错误文案。
pub struct HttpTvboxConfigImport;

#[async_trait]
impl TvboxConfigImportPort for HttpTvboxConfigImport {
    async fn fetch_config(&self, url: &str) -> Result<TvboxConfigFetch, AppError> {
        let (body, summary) = fetch_config_bytes(url).await?;
        Ok(TvboxConfigFetch::new(body, preview_facts(&summary)))
    }
}

/// 摘要 → Application 事实。
///
/// 逐项只搬运计数与形态：站点名、header 名、`ext` 形态、实现种类之外的字段一概不
/// 回传；未识别顶层字段只保留**条数**，连字段名都不出去。站点分类只按解析层已经
/// 定好的 `kind`（依据 `api` 形态），这里不重新解释数字 `type`。
fn preview_facts(summary: &TvboxConfigSummary) -> TvboxConfigPreviewFacts {
    let mut http_endpoint_site_count = 0usize;
    let mut spider_site_count = 0usize;
    let mut unclassified_site_count = 0usize;
    for site in &summary.sites {
        match site.kind {
            TvboxSiteKind::HttpEndpoint => http_endpoint_site_count += 1,
            TvboxSiteKind::Spider(_) => spider_site_count += 1,
            TvboxSiteKind::Unclassified => unclassified_site_count += 1,
        }
    }
    TvboxConfigPreviewFacts {
        site_count: summary.site_count,
        live_count: summary.live_count,
        parser_count: summary.parser_count,
        skipped_site_rows: summary.skipped_site_rows,
        skipped_live_rows: summary.skipped_live_rows,
        skipped_parser_rows: summary.skipped_parser_rows,
        spider_configured: summary.spider.is_some(),
        spider_kind: summary.spider.map(implementation_kind),
        spider_has_integrity_digest: summary.spider_has_integrity_digest,
        http_endpoint_site_count,
        spider_site_count,
        unclassified_site_count,
        // 这些名字来自解析层的固定白名单（wallpaper/rules/flags/doh/ads/logo/epg/
        // danmaku），不是配置里的任意键，因此可以出现在预览里。
        opaque_top_level_field_names: summary
            .opaque_top_level_fields
            .iter()
            .map(|field| field.name.clone())
            .collect(),
        unrecognized_top_level_field_count: summary.unknown_top_level_fields.len(),
        withheld_top_level_field_count: summary.withheld_top_level_field_count,
    }
}

fn implementation_kind(kind: TvboxImplementationKind) -> TvboxConfigImplementationKind {
    match kind {
        TvboxImplementationKind::Jar => TvboxConfigImplementationKind::Jar,
        TvboxImplementationKind::JavaScript => TvboxConfigImplementationKind::JavaScript,
        TvboxImplementationKind::Python => TvboxConfigImplementationKind::Python,
        TvboxImplementationKind::JsonManifest => TvboxConfigImplementationKind::JsonManifest,
        TvboxImplementationKind::Unidentified => TvboxConfigImplementationKind::Unidentified,
    }
}

// ---------- 错误 ----------

fn security_denied(message: &'static str) -> AppError {
    AppError::new(
        "SECURITY_POLICY_DENIED",
        ErrorKind::Security,
        message,
        false,
    )
}

/// 固定安全文案：绝不回显配置地址、重定向目标或远端响应片段。
fn fetch_unavailable(message: &'static str) -> AppError {
    AppError::new("SOURCE_UNAVAILABLE", ErrorKind::Network, message, true)
}

fn internal_error(message: &'static str) -> AppError {
    AppError::new("INTERNAL_ERROR", ErrorKind::Internal, message, false)
}

#[cfg(test)]
mod tests {
    use std::collections::VecDeque;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::{Arc, Mutex};

    use super::*;

    const SAMPLE_CONFIG: &str = r#"{
      "spider": "./spider.jar;md5;deadbeef",
      "sites": [
        {"name": "示例站点", "api": "https://api.example.invalid/vod", "type": 1,
         "headers": {"Authorization": "Bearer hidden"}},
        {"name": "Spider 站点", "api": "csp_Demo", "type": 3, "ext": "./js/demo.js"}
      ],
      "lives": [{"name": "示例直播", "url": "https://live.example.invalid/list.m3u"}],
      "parses": [{"name": "示例解析", "type": 1, "url": "https://parse.example.invalid/api"}],
      "vendorExtension": "opaque-value"
    }"#;

    /// 一个预置的单跳结果。
    enum FakeHop {
        Redirect(u16, &'static str),
        Status(u16),
        Body {
            chunks: Vec<Vec<u8>>,
            content_length: Option<u64>,
        },
    }

    impl FakeHop {
        fn ok(bytes: &[u8]) -> Self {
            FakeHop::Body {
                chunks: vec![bytes.to_vec()],
                content_length: Some(bytes.len() as u64),
            }
        }

        fn chunked(chunks: Vec<Vec<u8>>) -> Self {
            FakeHop::Body {
                chunks,
                content_length: None,
            }
        }
    }

    struct FakeTransport {
        hops: Mutex<VecDeque<FakeHop>>,
        requests: Mutex<Vec<String>>,
        /// 已经交给调用方的正文块数，用于证明上限是流式执行的。
        delivered_chunks: Arc<AtomicUsize>,
    }

    impl FakeTransport {
        fn new(hops: Vec<FakeHop>) -> Self {
            Self {
                hops: Mutex::new(hops.into()),
                requests: Mutex::new(Vec::new()),
                delivered_chunks: Arc::new(AtomicUsize::new(0)),
            }
        }

        fn requests(&self) -> Vec<String> {
            self.requests.lock().expect("requests lock").clone()
        }

        fn delivered_chunks(&self) -> usize {
            self.delivered_chunks.load(Ordering::SeqCst)
        }
    }

    #[async_trait]
    impl TvboxConfigTransport for FakeTransport {
        async fn get(&self, url: &str) -> Result<TvboxHopResponse, AppError> {
            self.requests
                .lock()
                .expect("requests lock")
                .push(url.to_owned());
            let hop = self
                .hops
                .lock()
                .expect("hops lock")
                .pop_front()
                .expect("fake transport exhausted");
            Ok(match hop {
                FakeHop::Redirect(status, location) => TvboxHopResponse {
                    status,
                    location: Some(location.to_owned()),
                    content_length: None,
                    body: Box::new(FakeBody::empty()),
                },
                FakeHop::Status(status) => TvboxHopResponse {
                    status,
                    location: None,
                    content_length: None,
                    body: Box::new(FakeBody::empty()),
                },
                FakeHop::Body {
                    chunks,
                    content_length,
                } => TvboxHopResponse {
                    status: 200,
                    location: None,
                    content_length,
                    body: Box::new(FakeBody {
                        chunks: chunks.into(),
                        delivered: Arc::clone(&self.delivered_chunks),
                    }),
                },
            })
        }
    }

    struct FakeBody {
        chunks: VecDeque<Vec<u8>>,
        delivered: Arc<AtomicUsize>,
    }

    impl FakeBody {
        fn empty() -> Self {
            Self {
                chunks: VecDeque::new(),
                delivered: Arc::new(AtomicUsize::new(0)),
            }
        }
    }

    #[async_trait]
    impl TvboxBodyStream for FakeBody {
        async fn next_chunk(&mut self) -> Result<Option<Vec<u8>>, AppError> {
            let chunk = self.chunks.pop_front();
            if chunk.is_some() {
                self.delivered.fetch_add(1, Ordering::SeqCst);
            }
            Ok(chunk)
        }
    }

    fn error_code(error: &AppError) -> &str {
        error.code().0.as_str()
    }

    #[tokio::test]
    async fn previews_a_valid_config_without_exposing_values() {
        let transport = FakeTransport::new(vec![FakeHop::ok(SAMPLE_CONFIG.as_bytes())]);
        let summary =
            fetch_config_preview_with(&transport, "https://config.example.invalid/tvbox.json")
                .await
                .expect("合法配置应返回摘要");

        assert_eq!(summary.site_count, 2);
        assert_eq!(summary.live_count, 1);
        assert_eq!(summary.parser_count, 1);
        assert!(summary.spider_has_integrity_digest);
        assert_eq!(
            transport.requests(),
            vec!["https://config.example.invalid/tvbox.json".to_owned()]
        );

        // 摘要（含 Debug）不得出现 URL、header 值或未知字段值。
        let rendered = format!("{summary:?}");
        for secret in [
            "example.invalid",
            "Bearer hidden",
            "opaque-value",
            "csp_Demo",
            "./js/demo.js",
        ] {
            assert!(!rendered.contains(secret), "摘要不得暴露原始配置值");
        }
    }

    /// 保存路径与预览路径必须看到同一份字节：同一个受控获取，只是多回一个原文。
    #[tokio::test]
    async fn the_byte_path_returns_the_same_validated_body_as_the_preview_path() {
        let transport = FakeTransport::new(vec![FakeHop::ok(SAMPLE_CONFIG.as_bytes())]);
        let (body, summary) =
            fetch_config_bytes_with(&transport, "https://config.example.invalid/tvbox.json")
                .await
                .expect("合法配置应返回原文与摘要");

        assert_eq!(body, SAMPLE_CONFIG.as_bytes());
        assert_eq!(summary.site_count, 2);
        assert_eq!(
            transport.requests(),
            vec!["https://config.example.invalid/tvbox.json".to_owned()]
        );
    }

    /// 取回成功但解析失败时，一个字节都不会交给调用方。
    #[tokio::test]
    async fn the_byte_path_yields_nothing_when_parsing_fails() {
        let transport = FakeTransport::new(vec![FakeHop::ok(b"not json at all")]);
        let error = fetch_config_bytes_with(&transport, "https://config.example.invalid/a.json")
            .await
            .expect_err("解析失败必须整体失败");
        assert_eq!(error_code(&error), crate::tvbox_config::CODE_INVALID_JSON);
        assert!(!format!("{error:?}").contains("example.invalid"));
    }

    #[tokio::test]
    async fn accepts_a_leading_bom_like_the_parser_does() {
        let mut bytes = vec![0xEF, 0xBB, 0xBF];
        bytes.extend_from_slice(SAMPLE_CONFIG.as_bytes());
        let transport = FakeTransport::new(vec![FakeHop::ok(&bytes)]);
        let summary =
            fetch_config_preview_with(&transport, "https://config.example.invalid/a.json")
                .await
                .expect("带 BOM 的配置应成功");
        assert_eq!(summary.site_count, 2);
    }

    #[tokio::test]
    async fn surfaces_the_parser_error_for_invalid_json() {
        let transport = FakeTransport::new(vec![FakeHop::ok(b"not json at all")]);
        let error = fetch_config_preview_with(&transport, "https://config.example.invalid/a.json")
            .await
            .expect_err("非法 JSON 应失败");
        assert_eq!(error_code(&error), crate::tvbox_config::CODE_INVALID_JSON);
        assert!(!format!("{error:?}").contains("example.invalid"));
    }

    #[tokio::test]
    async fn follows_redirects_and_revalidates_each_hop() {
        let transport = FakeTransport::new(vec![
            FakeHop::Redirect(302, "https://cdn.example.invalid/nested/tvbox.json"),
            FakeHop::Redirect(301, "/final.json"),
            FakeHop::ok(SAMPLE_CONFIG.as_bytes()),
        ]);
        let summary =
            fetch_config_preview_with(&transport, "https://config.example.invalid/a.json")
                .await
                .expect("重定向应被跟随");

        assert_eq!(summary.site_count, 2);
        assert_eq!(
            transport.requests(),
            vec![
                "https://config.example.invalid/a.json".to_owned(),
                "https://cdn.example.invalid/nested/tvbox.json".to_owned(),
                "https://cdn.example.invalid/final.json".to_owned(),
            ]
        );
    }

    #[tokio::test]
    async fn stops_after_the_redirect_limit() {
        let transport = FakeTransport::new(vec![
            FakeHop::Redirect(302, "https://config.example.invalid/1.json"),
            FakeHop::Redirect(302, "https://config.example.invalid/2.json"),
            FakeHop::Redirect(302, "https://config.example.invalid/3.json"),
            FakeHop::Redirect(302, "https://config.example.invalid/4.json"),
            // 若实现多跟一跳，就会取到这里；测试会因此看到 5 次请求而失败。
            FakeHop::ok(SAMPLE_CONFIG.as_bytes()),
        ]);
        let error = fetch_config_preview_with(&transport, "https://config.example.invalid/a.json")
            .await
            .expect_err("超过重定向上限应失败");

        assert_eq!(error_code(&error), "SOURCE_UNAVAILABLE");
        assert_eq!(transport.requests().len(), MAX_REDIRECTS + 1);
    }

    #[tokio::test]
    async fn rejects_unsafe_targets_before_any_request() {
        let transport = FakeTransport::new(Vec::new());
        for url in [
            "ftp://config.example.invalid/tvbox.json",
            "file:///etc/hosts",
            "https://user:secret@config.example.invalid/tvbox.json",
            "https://config.example.invalid/tvbox.json#frag",
            "http://127.0.0.1/tvbox.json",
            "http://10.0.0.1/tvbox.json",
            "http://192.168.1.1/tvbox.json",
            "http://169.254.169.254/latest/meta-data",
            "https://localhost/tvbox.json",
            "https://config.local/tvbox.json",
            "https://config.internal/tvbox.json",
            "https://tvbox/tvbox.json",
            "https://config.example.invalid:12345/tvbox.json",
        ] {
            let error = fetch_config_preview_with(&transport, url)
                .await
                .expect_err("不安全地址必须被拒绝");
            assert_eq!(error_code(&error), "SECURITY_POLICY_DENIED", "URL: {url}");
            // 错误文案同样不得回显被拒绝的地址。
            assert!(!format!("{error:?}").contains("example.invalid"));
        }
        assert!(
            transport.requests().is_empty(),
            "被拒绝的地址不应触发任何网络请求"
        );
    }

    #[tokio::test]
    async fn rejects_a_redirect_that_leaves_the_policy() {
        let transport = FakeTransport::new(vec![FakeHop::Redirect(
            302,
            "http://169.254.169.254/latest/meta-data",
        )]);
        let error = fetch_config_preview_with(&transport, "https://config.example.invalid/a.json")
            .await
            .expect_err("重定向到不安全地址应失败");
        assert_eq!(error_code(&error), "SECURITY_POLICY_DENIED");
        assert_eq!(transport.requests().len(), 1);
        assert!(!format!("{error:?}").contains("169.254"));
    }

    #[tokio::test]
    async fn rejects_failed_statuses_without_parsing() {
        let transport = FakeTransport::new(vec![FakeHop::Status(500)]);
        let error = fetch_config_preview_with(&transport, "https://config.example.invalid/a.json")
            .await
            .expect_err("非成功状态应失败");
        assert_eq!(error_code(&error), "SOURCE_UNAVAILABLE");
        assert_eq!(transport.delivered_chunks(), 0);

        let transport = FakeTransport::new(vec![FakeHop::Status(404)]);
        let error = fetch_config_preview_with(&transport, "https://config.example.invalid/a.json")
            .await
            .expect_err("404 应失败");
        assert_eq!(error_code(&error), "SOURCE_UNAVAILABLE");
    }

    #[tokio::test]
    async fn a_redirect_without_a_location_is_an_error() {
        let transport = FakeTransport::new(vec![FakeHop::Status(302)]);
        let error = fetch_config_preview_with(&transport, "https://config.example.invalid/a.json")
            .await
            .expect_err("缺少 Location 的重定向应失败");
        assert_eq!(error_code(&error), "SOURCE_UNAVAILABLE");
    }

    #[tokio::test]
    async fn rejects_a_declared_length_over_the_cap_without_reading() {
        let transport = FakeTransport::new(vec![FakeHop::Body {
            chunks: vec![b"{}".to_vec()],
            content_length: Some(MAX_CONFIG_BYTES as u64 + 1),
        }]);
        let error = fetch_config_preview_with(&transport, "https://config.example.invalid/a.json")
            .await
            .expect_err("声明超限应失败");
        assert_eq!(error_code(&error), "SOURCE_UNAVAILABLE");
        assert_eq!(transport.delivered_chunks(), 0, "声明超限时不应读取正文");
    }

    #[tokio::test]
    async fn stops_reading_an_oversized_chunked_body() {
        // 12 × 1 MiB = 12 MiB；上限 8 MiB，因此读到第 9 块时就会失败。
        const CHUNK: usize = 1024 * 1024;
        let total_chunks = 12;
        let chunks: Vec<Vec<u8>> = (0..total_chunks).map(|_| vec![b'x'; CHUNK]).collect();
        let transport = FakeTransport::new(vec![FakeHop::chunked(chunks)]);

        let error = fetch_config_preview_with(&transport, "https://config.example.invalid/a.json")
            .await
            .expect_err("超过 8 MiB 的 chunked 正文应失败");
        assert_eq!(error_code(&error), "SOURCE_UNAVAILABLE");
        assert!(
            transport.delivered_chunks() < total_chunks,
            "超限后不应继续读取剩余正文（已读 {} 块）",
            transport.delivered_chunks()
        );
    }

    #[tokio::test]
    async fn accepts_a_body_exactly_at_the_cap() {
        // 一份刚好等于上限、但内容为空白前缀的正文：大小通过，解析按自身规则失败。
        let mut bytes = vec![b' '; MAX_CONFIG_BYTES];
        bytes[0] = b'{';
        bytes[1] = b'}';
        let transport = FakeTransport::new(vec![FakeHop::chunked(vec![bytes])]);
        let summary =
            fetch_config_preview_with(&transport, "https://config.example.invalid/a.json")
                .await
                .expect("恰好等于上限的正文不应被大小上限拒绝");
        assert_eq!(summary.site_count, 0);
    }

    #[tokio::test]
    async fn reports_extremely_long_redirect_chains_without_leaking_targets() {
        let transport = FakeTransport::new(vec![
            FakeHop::Redirect(302, "https://a.example.invalid/1"),
            FakeHop::Redirect(302, "https://b.example.invalid/2"),
            FakeHop::Redirect(302, "https://c.example.invalid/3"),
            FakeHop::Redirect(302, "https://d.example.invalid/4"),
        ]);
        let error = fetch_config_preview_with(&transport, "https://config.example.invalid/a.json")
            .await
            .expect_err("重定向链应超限");
        let rendered = format!("{error:?} {error}");
        for secret in [
            "a.example.invalid",
            "d.example.invalid",
            "config.example.invalid",
        ] {
            assert!(!rendered.contains(secret), "错误不得暴露重定向地址");
        }
    }

    // ---------- Application 预览端口适配 ----------

    #[test]
    fn preview_facts_keep_only_counts_and_shapes() {
        let summary = parse_config(SAMPLE_CONFIG.as_bytes())
            .expect("示例配置应可解析")
            .summary();
        let facts = preview_facts(&summary);

        assert_eq!(facts.site_count, 2);
        assert_eq!(facts.live_count, 1);
        assert_eq!(facts.parser_count, 1);
        assert_eq!(facts.skipped_site_rows, 0);
        assert!(facts.spider_configured);
        assert_eq!(facts.spider_kind, Some(TvboxConfigImplementationKind::Jar));
        assert!(facts.spider_has_integrity_digest);
        // 分类只依据 `api` 形态：一个 http 端点 + 一个 `csp_` spider 前缀。
        assert_eq!(facts.http_endpoint_site_count, 1);
        assert_eq!(facts.spider_site_count, 1);
        assert_eq!(facts.unclassified_site_count, 0);
        // 顶层里只有 `vendorExtension` 未识别；名字与取值都不回传。
        assert!(facts.opaque_top_level_field_names.is_empty());
        assert_eq!(facts.unrecognized_top_level_field_count, 1);
        assert_eq!(facts.withheld_top_level_field_count, 0);

        // 事实（含 Debug）不得带出站点名、端点、header 值、ext 内容或未知字段值。
        let rendered = format!("{facts:?}");
        for secret in [
            "example.invalid",
            "Bearer hidden",
            "csp_Demo",
            "./js/demo.js",
            "opaque-value",
            "示例站点",
            "示例直播",
            "示例解析",
            "vendorExtension",
        ] {
            assert!(!rendered.contains(secret), "预览事实不得暴露原始配置值");
        }
    }

    #[test]
    fn preview_facts_expose_only_whitelisted_opaque_field_names() {
        let config = r#"{
          "sites": [],
          "wallpaper": "./bg.png",
          "epg": "https://epg.example.invalid/list",
          "vendorExtension": {"nested": "vendor-only-value"}
        }"#;
        let summary = parse_config(config.as_bytes())
            .expect("配置应可解析")
            .summary();
        let facts = preview_facts(&summary);

        // 白名单字段名可以出现（它们是固定集合，不是配置里的任意键）。顺序由解析层
        // 决定且稳定，这里只比较集合，避免把排序细节钉进契约。
        let mut names = facts.opaque_top_level_field_names.clone();
        names.sort();
        assert_eq!(names, vec!["epg".to_owned(), "wallpaper".to_owned()]);
        // 未识别键只留条数。
        assert_eq!(facts.unrecognized_top_level_field_count, 1);
        let rendered = format!("{facts:?}");
        for secret in [
            "vendorExtension",
            "vendor-only-value",
            "nested",
            "epg.example.invalid",
            "./bg.png",
        ] {
            assert!(
                !rendered.contains(secret),
                "预览事实不得暴露非白名单字段或原始值"
            );
        }
    }

    #[test]
    fn implementation_kinds_map_one_to_one() {
        assert_eq!(
            implementation_kind(TvboxImplementationKind::Jar),
            TvboxConfigImplementationKind::Jar
        );
        assert_eq!(
            implementation_kind(TvboxImplementationKind::JavaScript),
            TvboxConfigImplementationKind::JavaScript
        );
        assert_eq!(
            implementation_kind(TvboxImplementationKind::Python),
            TvboxConfigImplementationKind::Python
        );
        assert_eq!(
            implementation_kind(TvboxImplementationKind::JsonManifest),
            TvboxConfigImplementationKind::JsonManifest
        );
        assert_eq!(
            implementation_kind(TvboxImplementationKind::Unidentified),
            TvboxConfigImplementationKind::Unidentified
        );
    }
}
