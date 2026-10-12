//! Artwork 专用出站策略。
//!
//! Artwork 的来源 URL 来自 Provider/用户配置，不能因为“已经登记”就直接
//! 认为安全。该模块把 URL 形状、来源 Host、系统 DNS 结果和实际请求收敛在
//! 同一个边界内：校验的产物是 [`ArtworkTarget`]——唯一能构造请求的类型——
//! 它携带已检查的 URL、**URL 原文里的主机名**和**精确的 SocketAddr 集合**。
//! 每一跳新建的客户端用 `resolve_to_addrs` 把连接固定到该集合，因此应用层
//! 与传输层都不会再做第二次 DNS 解析，不存在“校验一次、再解析一次”的重绑定
//! 窗口。
//!
//! 部分 Windows 代理使用 Fake-IP（198.18/15 加固定 ULA 前缀），这时必须用
//! 独立的公共 DoH 证明 Host 的真实解析仍然是公网地址，不能直接放行整个保留
//! 网段。DoH 只回答“是否可以固定”，绝不提供要固定的地址：系统解析出的
//! Fake-IP 会被原样固定，由系统 TUN/路由把连接映射到真实目的地。这是一个受
//! 信任的本地映射边界，**不是**远端最终地址的证明。

use std::collections::HashMap;
use std::fmt;
use std::net::{IpAddr, SocketAddr};
use std::sync::Arc;
use std::time::Duration;

use haven_common::network::{
    is_local_proxy_fake_ip as is_known_fake_ip, is_publicly_routable, validate_host,
};
use haven_common::{AppError, ErrorKind};
use reqwest::header::{ACCEPT, HeaderMap, HeaderValue, REFERER, USER_AGENT};
use reqwest::redirect::Policy;
use reqwest::{RequestBuilder, Url};
use serde::Deserialize;
use tokio::sync::{Mutex, OwnedSemaphorePermit, Semaphore};
use tokio::time::Instant;

use crate::db::repos::image_proxy::{
    MAX_ARTWORK_TARGET_URL_LEN, allows_signed_query, has_sensitive_query,
};
use crate::http_security::{ResolvedHttpTarget, pin_client_builder};

const CONNECT_TIMEOUT: Duration = Duration::from_millis(1_500);
const REQUEST_TIMEOUT: Duration = Duration::from_secs(3);
/// 校验阶段的系统解析独立于请求客户端，客户端的超时覆盖不到它，因此这里必须
/// 有自带上限，否则一个挂死的解析器会永久占住抓取并发位。
const RESOLVE_TIMEOUT: Duration = Duration::from_millis(1_500);
const DOH_CONNECT_TIMEOUT: Duration = Duration::from_millis(1_200);
const DOH_REQUEST_TIMEOUT: Duration = Duration::from_millis(1_500);
const DOH_MAX_RESPONSE_BYTES: u64 = 64 * 1024;
const DOH_PRIMARY: &str = "https://cloudflare-dns.com/dns-query";
const DOH_FALLBACK: &str = "https://dns.google/resolve";
const PROOF_MIN_TTL: Duration = Duration::from_secs(60);
const PROOF_MAX_TTL: Duration = Duration::from_secs(10 * 60);
const PROOF_TEMPORARY_TTL: Duration = Duration::from_secs(30);
const PROOF_UNSAFE_TTL: Duration = Duration::from_secs(10 * 60);
const MAX_CONCURRENT_FETCHES: usize = 6;
const BROWSER_USER_AGENT: &str = "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/120.0.0.0 Safari/537.36";
const IMAGE_ACCEPT: &str = "image/avif,image/webp,image/png,image/jpeg,*/*;q=0.5";
const DOH_USER_AGENT: &str = "Haven-Artwork-DNS-Proof/0.1";

#[derive(Clone)]
pub(super) struct ArtworkNetwork {
    /// DoH 证明专用客户端。它保留系统代理设置（证明必须走真实网络路径），
    /// 只用于 DNS 证明，不承载任何图片内容请求。
    doh_client: reqwest::Client,
    proofs: ProofTable,
    fetch_slots: Arc<Semaphore>,
}

/// 一次“已校验、已解析”的出站目标，也是构造 Artwork 请求的唯一入口。
///
/// 字段都是私有的，只有 [`ArtworkNetwork::resolve_remote_target`] 能构造它，
/// 因此不存在“先建请求、后补校验”的调用顺序。
#[derive(Clone)]
pub(super) struct ArtworkTarget {
    /// 已通过形状与来源作用域检查的请求 URL。
    url: Url,
    /// `resolve_to_addrs` 的键：URL 里的主机原文（域名时才有，含可能的尾点
    /// 形式）。reqwest 用 URI authority 里的名字查询覆盖表，键换成归一化形式
    /// 会静默落回系统解析，DNS 固定就失效了。
    pinned_host: Option<String>,
    /// 精确固定的连接目的地集合。
    addresses: Vec<SocketAddr>,
    /// `validate_host` 归一化后的主机：只用于来源作用域比较与目标自洽性检查。
    normalized_host: String,
    source_id: Option<String>,
}

/// 手写的 `Debug`：URL（可能带 CDN 签名查询串）、主机原文、固定地址与来源标识
/// 都属于敏感材料，一旦被渲染进日志、错误上报或 panic 信息就无法收回。诊断
/// 路径（`unwrap_err`、断言失败信息）依赖 `ArtworkTarget: Debug`，因此实现必须
/// 存在，但只输出固定类型标签与已检查地址数量，不输出任何字段值。
impl fmt::Debug for ArtworkTarget {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ArtworkTarget")
            .field("pinned_addresses", &self.addresses.len())
            .finish()
    }
}

impl ArtworkTarget {
    pub(super) fn url(&self) -> &Url {
        &self.url
    }

    /// 只有已校验目标能构造请求：每一跳新建客户端、关闭自动重定向，把 DNS
    /// 覆盖固定为 [`Self::addresses`]，并通过 `no_proxy()` 停用 reqwest 的
    /// 显式代理配置。
    ///
    /// 进程级 `no_proxy()` 不影响系统 TUN/路由层的 Fake-IP 映射，这正是
    /// “固定系统解析出的 Fake-IP 地址”仍然能到达真实目的地的原因；它不代表
    /// 我们已经验证了 TUN 背后的远端最终地址。
    pub(super) fn build_request(&self) -> Result<RequestBuilder, AppError> {
        // 目标自洽性：请求 URL 的主机必须仍然归一化到已批准的主机，避免任何
        // 路径构造出“URL 与来源作用域不一致”的请求。
        let host_matches = self
            .url
            .host_str()
            .and_then(|raw| validate_host(raw).ok())
            .is_some_and(|host| host == self.normalized_host);
        if !host_matches {
            return Err(artwork_security("图片主机不在来源允许列表"));
        }
        let builder = reqwest::Client::builder()
            .redirect(Policy::none())
            .connect_timeout(CONNECT_TIMEOUT)
            .timeout(REQUEST_TIMEOUT);
        let pinned = ResolvedHttpTarget {
            url: self.url.clone(),
            dns_name: self.pinned_host.clone(),
            addresses: self.addresses.clone(),
        };
        let client = pin_client_builder(builder, &pinned)
            .build()
            .map_err(|_| client_init_error())?;
        Ok(client
            .get(self.url.clone())
            .headers(source_headers(self.source_id.as_deref())))
    }
}

type ProofSlot = Arc<Mutex<Option<CachedProof>>>;
type ProofTable = Arc<Mutex<HashMap<String, ProofSlot>>>;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ProofVerdict {
    Public,
    Unsafe,
    Temporary,
}

#[derive(Debug, Clone, Copy)]
struct CachedProof {
    verdict: ProofVerdict,
    expires_at: Instant,
}

/// 系统解析结果的去向。DoH 证明只决定“是否允许固定”，**从不改变要固定的
/// 地址**，因此地址集合在这里就已经确定。
#[derive(Debug, Clone, PartialEq, Eq)]
enum ResolutionDecision {
    /// 整组答案都是可路由公网地址 → 逐字固定。
    Pin(Vec<SocketAddr>),
    /// 整组答案都是系统 Fake-IP → 先做 DoH 公网证明，通过后原样固定这组地址。
    RequirePublicProof(Vec<SocketAddr>),
    /// 空集合、混合集合、内网/保留/云元数据地址 → 整体拒绝。
    Deny,
}

#[derive(Debug)]
enum DohQueryError {
    Unsafe,
    Temporary,
}

#[derive(Debug)]
struct DohReply {
    ips: Vec<IpAddr>,
    ttl_seconds: u64,
}

#[derive(Debug, Deserialize)]
struct DohResponse {
    #[serde(rename = "Status")]
    status: Option<u16>,
    #[serde(rename = "Question")]
    question: Option<Vec<DohQuestion>>,
    #[serde(rename = "Answer")]
    answer: Option<Vec<DohAnswer>>,
}

#[derive(Debug, Deserialize)]
struct DohQuestion {
    #[serde(rename = "name")]
    name: Option<String>,
}

#[derive(Debug, Deserialize)]
struct DohAnswer {
    #[serde(rename = "type")]
    record_type: Option<u16>,
    #[serde(rename = "TTL")]
    ttl: Option<u64>,
    #[serde(rename = "data")]
    data: Option<String>,
}

impl ArtworkNetwork {
    pub(super) fn new() -> Result<Self, AppError> {
        // Artwork 内容请求没有可复用的客户端：目的地由每一跳的已检查目标
        // 决定，任何“预先建好的、未固定地址的”客户端都会重新打开
        // 校验与解析之间的重绑定窗口。
        let doh_client = reqwest::Client::builder()
            .redirect(Policy::none())
            .connect_timeout(DOH_CONNECT_TIMEOUT)
            .timeout(DOH_REQUEST_TIMEOUT)
            .user_agent(DOH_USER_AGENT)
            .build()
            .map_err(|_| client_init_error())?;
        Ok(Self {
            doh_client,
            proofs: Arc::new(Mutex::new(HashMap::new())),
            fetch_slots: Arc::new(Semaphore::new(MAX_CONCURRENT_FETCHES)),
        })
    }

    pub(super) async fn acquire_fetch_slot(&self) -> Option<OwnedSemaphorePermit> {
        // The semaphore is deliberately local to Artwork.  It prevents a cold
        // trending page from opening dozens of simultaneous TLS connections to
        // one public image host while leaving WebDAV/stream/AI clients alone.
        self.fetch_slots.clone().acquire_owned().await.ok()
    }

    /// 校验一跳并返回唯一可发请求的目标。
    ///
    /// 重定向的每一跳都必须重新调用：URL 形状、来源作用域与 DNS 都会重新
    /// 判定，不会复用上一跳的信任或地址。
    pub(super) async fn resolve_remote_target(
        &self,
        url: &Url,
        source_id: Option<&str>,
        registered_host: Option<&str>,
    ) -> Result<ArtworkTarget, AppError> {
        let host = checked_source_host(url, source_id, registered_host)?;
        let port = url
            .port_or_known_default()
            .ok_or_else(|| artwork_security("图片端口无效"))?;

        // 字面地址本身就是连接目标：它必须自己通过公网判定，绝不能被提升进
        // 域名的 Fake-IP/DoH 证明路径，也不能借用“看起来已登记”的信任。
        if let Ok(ip) = host.parse::<IpAddr>() {
            if !is_publicly_routable(ip) {
                return Err(artwork_security("图片主机解析到禁止的网络地址"));
            }
            return Ok(compose_target(
                url,
                source_id,
                host,
                vec![SocketAddr::new(ip, port)],
            ));
        }

        let addresses = lookup_addresses(&host, port).await?;
        match decide_resolution(addresses) {
            ResolutionDecision::Deny => Err(artwork_security("图片主机解析到禁止的网络地址")),
            ResolutionDecision::Pin(addresses) => {
                Ok(compose_target(url, source_id, host, addresses))
            }
            ResolutionDecision::RequirePublicProof(addresses) => {
                self.prove_public_host(&host).await?;
                // 证明通过只说明该域名在公共 DNS 上解析到公网地址，既不
                // 放行任何保留网段，也不替换地址：固定的仍然是系统解析出的
                // Fake-IP，TUN/路由才能把它映射到真实目的地。
                Ok(compose_target(url, source_id, host, addresses))
            }
        }
    }

    async fn prove_public_host(&self, host: &str) -> Result<(), AppError> {
        let slot = {
            let mut proofs = self.proofs.lock().await;
            proofs
                .entry(host.to_owned())
                .or_insert_with(|| Arc::new(Mutex::new(None)))
                .clone()
        };
        let mut cached = slot.lock().await;
        let now = Instant::now();
        if let Some(proof) = *cached
            && proof.expires_at > now
        {
            return proof_to_result(proof.verdict);
        }

        let (verdict, ttl) = match self.query_public_dns(host).await {
            Ok(ttl) => (ProofVerdict::Public, clamp_proof_ttl(ttl)),
            Err(DohQueryError::Unsafe) => (ProofVerdict::Unsafe, PROOF_UNSAFE_TTL),
            Err(DohQueryError::Temporary) => (ProofVerdict::Temporary, PROOF_TEMPORARY_TTL),
        };
        *cached = Some(CachedProof {
            verdict,
            expires_at: Instant::now() + ttl,
        });
        proof_to_result(verdict)
    }

    async fn query_public_dns(&self, host: &str) -> Result<Duration, DohQueryError> {
        match self.query_doh_endpoint(DOH_PRIMARY, host).await {
            Ok(ttl) => Ok(ttl),
            Err(DohQueryError::Unsafe) => Err(DohQueryError::Unsafe),
            Err(DohQueryError::Temporary) => self.query_doh_endpoint(DOH_FALLBACK, host).await,
        }
    }

    async fn query_doh_endpoint(
        &self,
        endpoint: &str,
        host: &str,
    ) -> Result<Duration, DohQueryError> {
        let (a, aaaa) = tokio::join!(
            self.query_doh_type(endpoint, host, 1),
            self.query_doh_type(endpoint, host, 28),
        );
        if matches!(a, Err(DohQueryError::Unsafe)) || matches!(aaaa, Err(DohQueryError::Unsafe)) {
            return Err(DohQueryError::Unsafe);
        }
        let a = a?;
        let aaaa = aaaa?;
        let mut ips = a.ips;
        ips.extend(aaaa.ips);
        // DoH 没有资格返回不安全地址：任何一个不可路由的答案都让整次证明
        // 变成 Unsafe，而不是“取其中可用的那个”。
        if ips.is_empty() || ips.iter().any(|ip| !is_publicly_routable(*ip)) {
            return Err(DohQueryError::Unsafe);
        }
        Ok(Duration::from_secs(
            a.ttl_seconds.min(aaaa.ttl_seconds).max(1),
        ))
    }

    async fn query_doh_type(
        &self,
        endpoint: &str,
        host: &str,
        record_type: u16,
    ) -> Result<DohReply, DohQueryError> {
        let mut url = Url::parse(endpoint).map_err(|_| DohQueryError::Temporary)?;
        url.query_pairs_mut()
            .append_pair("name", host)
            .append_pair("type", &record_type.to_string());
        let response = self
            .doh_client
            .get(url)
            .header("accept", "application/dns-json")
            .send()
            .await
            .map_err(|_| DohQueryError::Temporary)?;
        if !response.status().is_success()
            || response
                .content_length()
                .is_some_and(|size| size > DOH_MAX_RESPONSE_BYTES)
        {
            return Err(DohQueryError::Temporary);
        }
        // Do not call `Response::bytes()` here: without a trustworthy
        // Content-Length that would buffer an attacker-controlled body before
        // the size check. Consume chunks and stop at the hard cap instead.
        let mut bytes = Vec::new();
        let mut response = response;
        while let Some(chunk) = response
            .chunk()
            .await
            .map_err(|_| DohQueryError::Temporary)?
        {
            if bytes.len() + chunk.len() > DOH_MAX_RESPONSE_BYTES as usize {
                return Err(DohQueryError::Temporary);
            }
            bytes.extend_from_slice(&chunk);
        }
        let payload: DohResponse =
            serde_json::from_slice(&bytes).map_err(|_| DohQueryError::Temporary)?;
        let question_matches = payload.question.as_deref().is_some_and(|questions| {
            questions.iter().any(|question| {
                question
                    .name
                    .as_deref()
                    .is_some_and(|name| normalize_dns_name(name) == host)
            })
        });
        if !question_matches {
            return Err(DohQueryError::Unsafe);
        }
        match payload.status.unwrap_or(u16::MAX) {
            0 | 3 => {}
            _ => return Err(DohQueryError::Temporary),
        }

        let mut ips = Vec::new();
        let mut ttl_seconds = u64::MAX;
        for answer in payload.answer.unwrap_or_default() {
            if answer.record_type != Some(record_type) {
                continue;
            }
            let data = answer.data.ok_or(DohQueryError::Unsafe)?;
            let ip = data.parse::<IpAddr>().map_err(|_| DohQueryError::Unsafe)?;
            ips.push(ip);
            ttl_seconds = ttl_seconds.min(answer.ttl.unwrap_or(60));
        }
        Ok(DohReply {
            ips,
            ttl_seconds: if ttl_seconds == u64::MAX {
                60
            } else {
                ttl_seconds
            },
        })
    }
}

/// URL 形状 + 来源作用域检查，返回 `validate_host` 归一化后的主机名。
///
/// 显式登记 = 来源标识与主机作用域同时存在（`register` 写入的两列）。只有这种
/// 映射才允许携带 CDN 签名查询串；缺少来源登记的旧记录即使 URL 看起来是公网
/// 地址也继续 fail closed，不因为“地址长得合法”获得网络授权。豆瓣按主机族
/// 自动授权，因此不在此列。
///
/// 归一化后的主机只用于作用域比较：`validate_host` 同时覆盖 localhost、单标签
/// 主机、`.local`/`.internal` 与不可路由的字面地址。
fn checked_source_host(
    url: &Url,
    source_id: Option<&str>,
    registered_host: Option<&str>,
) -> Result<String, AppError> {
    let explicitly_registered =
        registered_host.is_some() && source_id.is_some_and(allows_signed_query);
    if !matches!(url.scheme(), "http" | "https")
        || !url.username().is_empty()
        || url.password().is_some()
        || url.fragment().is_some()
        || url.as_str().len() > MAX_ARTWORK_TARGET_URL_LEN
        || (!explicitly_registered && has_sensitive_query(url))
    {
        return Err(artwork_security("图片地址不符合出站策略"));
    }
    let raw_host = url
        .host_str()
        .ok_or_else(|| artwork_security("图片地址缺少主机"))?;
    let host = validate_host(raw_host).map_err(|_| artwork_security("图片主机不在来源允许列表"))?;
    let registered = registered_host.and_then(|value| validate_host(value).ok());
    if (source_id == Some("douban") && host != "doubanio.com" && !host.ends_with(".doubanio.com"))
        || (source_id != Some("douban") && registered.as_deref() != Some(host.as_str()))
    {
        return Err(artwork_security("图片主机不在来源允许列表"));
    }
    Ok(host)
}

/// 系统解析。客户端的连接超时覆盖不到这一步，因此结果必须有独立上限；空集合
/// 交给 [`decide_resolution`] 拒绝。
async fn lookup_addresses(host: &str, port: u16) -> Result<Vec<SocketAddr>, AppError> {
    let lookup = tokio::net::lookup_host((host, port));
    let mut addresses = tokio::time::timeout(RESOLVE_TIMEOUT, lookup)
        .await
        .map_err(|_| artwork_security("图片主机解析失败"))?
        .map_err(|_| artwork_security("图片主机解析失败"))?
        .collect::<Vec<_>>();
    // 排序去重让固定集合稳定可断言，并避免同一地址重复占用连接尝试。
    addresses.sort_unstable();
    addresses.dedup();
    Ok(addresses)
}

/// 把“已检查的主机 + 已确定的地址集合”组合成唯一可发请求的目标。
fn compose_target(
    url: &Url,
    source_id: Option<&str>,
    normalized_host: String,
    addresses: Vec<SocketAddr>,
) -> ArtworkTarget {
    // 字面地址本身就是连接目标，不需要（也不允许）DNS 覆盖键；域名一律使用
    // URL 原文里的主机名，逐字匹配 reqwest 实际查询的名字。
    let request_host = url.host_str().unwrap_or(normalized_host.as_str());
    let pin_key = normalized_host
        .parse::<IpAddr>()
        .is_err()
        .then(|| request_host.to_owned());
    ArtworkTarget {
        url: url.clone(),
        pinned_host: pin_key,
        addresses,
        normalized_host,
        source_id: source_id.map(str::to_owned),
    }
}

/// 三态判定：整组公网 → 固定；整组 Fake-IP → 需证明后固定；其余（空、混合、
/// 含内网/保留/云元数据地址）→ 拒绝。
fn decide_resolution(addresses: Vec<SocketAddr>) -> ResolutionDecision {
    let mut seen = false;
    let mut fake = false;
    let mut public = false;
    for ip in addresses.iter().map(|address| address.ip()) {
        seen = true;
        if is_known_fake_ip(ip) {
            fake = true;
            continue;
        }
        if !is_publicly_routable(ip) {
            return ResolutionDecision::Deny;
        }
        public = true;
    }
    if !seen {
        ResolutionDecision::Deny
    } else if fake && !public {
        ResolutionDecision::RequirePublicProof(addresses)
    } else if !fake && public {
        ResolutionDecision::Pin(addresses)
    } else {
        ResolutionDecision::Deny
    }
}

fn normalize_dns_name(value: &str) -> String {
    value.trim_end_matches('.').to_ascii_lowercase()
}

fn clamp_proof_ttl(value: Duration) -> Duration {
    value.clamp(PROOF_MIN_TTL, PROOF_MAX_TTL)
}

fn source_headers(source_id: Option<&str>) -> HeaderMap {
    let mut headers = HeaderMap::new();
    headers.insert(USER_AGENT, HeaderValue::from_static(BROWSER_USER_AGENT));
    headers.insert(ACCEPT, HeaderValue::from_static(IMAGE_ACCEPT));
    if source_id == Some("douban") {
        headers.insert(REFERER, HeaderValue::from_static("https://m.douban.com/"));
    }
    headers
}

fn proof_to_result(verdict: ProofVerdict) -> Result<(), AppError> {
    match verdict {
        ProofVerdict::Public => Ok(()),
        ProofVerdict::Unsafe => Err(artwork_security("图片主机解析到禁止的网络地址")),
        ProofVerdict::Temporary => Err(AppError::new(
            "ARTWORK_DNS_PROOF_FAILED",
            ErrorKind::Network,
            "图片来源暂时不可用",
            true,
        )),
    }
}

fn client_init_error() -> AppError {
    AppError::new(
        "ARTWORK_CLIENT_INIT_FAILED",
        ErrorKind::Internal,
        "图片缓存客户端初始化失败",
        false,
    )
}

fn artwork_security(message: &'static str) -> AppError {
    AppError::new(
        "SECURITY_POLICY_DENIED",
        ErrorKind::Security,
        message,
        false,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn socket(value: &str) -> SocketAddr {
        SocketAddr::new(value.parse().unwrap(), 443)
    }

    #[test]
    fn fake_ip_resolution_is_narrow_and_fail_closed() {
        let system_answers = vec![socket("198.18.0.105"), socket("fdfe:dcba:9876::ee")];
        let proof_route = ResolutionDecision::RequirePublicProof(system_answers.clone());
        assert_eq!(decide_resolution(system_answers), proof_route);
        assert_eq!(
            decide_resolution(vec![socket("1.1.1.1")]),
            ResolutionDecision::Pin(vec![socket("1.1.1.1")])
        );
        let mixed = vec![socket("198.18.0.105"), socket("1.1.1.1")];
        assert_eq!(decide_resolution(mixed), ResolutionDecision::Deny);
        let private = vec![socket("198.18.0.105"), socket("10.0.0.1")];
        assert_eq!(decide_resolution(private), ResolutionDecision::Deny);
        assert_eq!(decide_resolution(Vec::new()), ResolutionDecision::Deny);
        assert!(is_known_fake_ip("198.19.255.255".parse().unwrap()));
        assert!(!is_known_fake_ip("198.20.0.1".parse().unwrap()));
        assert!(!is_known_fake_ip("fdfe:dcba:9877::1".parse().unwrap()));
    }

    #[test]
    fn mixed_empty_private_and_reserved_answers_are_all_denied() {
        for addresses in [
            vec![socket("198.18.0.105"), socket("1.1.1.1")],
            vec![socket("fdfe:dcba:9876::1"), socket("1.1.1.1")],
            vec![socket("10.0.0.1")],
            vec![socket("100.64.0.1")],
            vec![socket("100.127.255.254")],
            vec![socket("169.254.169.254")],
            vec![socket("192.0.2.5")],
            vec![socket("198.51.100.7")],
            vec![socket("203.0.113.9")],
            vec![socket("240.0.0.1")],
            vec![socket("2001:db8::1")],
            vec![socket("fc00::1")],
            Vec::new(),
        ] {
            assert_eq!(
                decide_resolution(addresses.clone()),
                ResolutionDecision::Deny,
                "{addresses:?}"
            );
        }
    }

    #[test]
    fn canonical_public_predicate_rejects_reserved_and_fake_answers() {
        for value in [
            "127.0.0.1",
            "10.0.0.1",
            "169.254.169.254",
            "198.18.0.1",
            "fdfe:dcba:9876::1",
            "224.0.0.1",
        ] {
            assert!(!is_publicly_routable(value.parse().unwrap()), "{value}");
        }
        // 比“排除私有网段”更窄的公共判定：CGNAT、云元数据别名、基准测试
        // 网段、文档网段与保留段都不算 DoH 证明的“公网证据”。
        for value in [
            "100.64.0.1",
            "192.0.0.1",
            "198.51.100.7",
            "203.0.113.9",
            "255.255.255.255",
            "0.0.0.0",
            "fe80::1",
            "::1",
            "ff02::1",
        ] {
            assert!(!is_publicly_routable(value.parse().unwrap()), "{value}");
        }
        assert!(is_publicly_routable("180.97.198.41".parse().unwrap()));
        assert!(is_publicly_routable("1.1.1.1".parse().unwrap()));
    }

    #[test]
    fn pinned_host_always_matches_the_request_url_host() {
        // 固定键必须是 reqwest 实际查询的名字（URL authority 里的原文）。写成
        // 归一化形式会让覆盖表查不到键，从而静默回落到系统解析。
        for raw in [
            "https://img.example.invalid/poster.jpg",
            "https://IMG.Example.Invalid./poster.jpg",
        ] {
            let url: Url = raw.parse().unwrap();
            let addresses = vec![socket("93.184.216.34"), socket("93.184.216.35")];
            let target = compose_target(
                &url,
                Some("cms10"),
                "img.example.invalid".to_owned(),
                addresses.clone(),
            );
            assert_eq!(target.pinned_host.as_deref(), url.host_str(), "{raw}");
            assert_eq!(target.normalized_host, "img.example.invalid", "{raw}");
            assert_eq!(target.addresses, addresses, "{raw}");
            assert_eq!(target.url.as_str(), url.as_str(), "{raw}");
            assert!(target.build_request().is_ok(), "{raw}");
        }
    }

    #[test]
    fn fake_ip_answers_are_retained_and_never_replaced_by_proof_answers() {
        let url: Url = "https://img.doubanio.com/poster.jpg".parse().unwrap();
        let system_answers = vec![socket("198.18.0.105"), socket("fdfe:dcba:9876::ee")];
        let retained = match decide_resolution(system_answers.clone()) {
            ResolutionDecision::RequirePublicProof(retained) => retained,
            other => panic!("整组系统 Fake-IP 必须走公共证明路径: {other:?}"),
        };
        // 证明只回答“允许固定”，不提供地址：组合出的目标仍然固定系统解析出的
        // Fake-IP，TUN/路由才能把它映射到真实目的地。
        let host = "img.doubanio.com";
        let target = compose_target(&url, Some("douban"), host.to_owned(), retained);
        assert_eq!(target.addresses, system_answers);
        assert_eq!(target.pinned_host.as_deref(), Some(host));
        for address in &target.addresses {
            assert!(is_known_fake_ip(address.ip()));
        }
        assert!(
            target.build_request().is_ok(),
            "Fake-IP 目标同样只能通过已校验目标构造请求"
        );
    }

    #[test]
    fn literal_addresses_never_get_a_dns_override_key() {
        let url: Url = "https://1.1.1.1/a.jpg".parse().unwrap();
        let target = compose_target(
            &url,
            Some("cms10"),
            "1.1.1.1".to_owned(),
            vec![socket("1.1.1.1")],
        );
        assert_eq!(target.pinned_host, None);
        assert_eq!(target.addresses, vec![socket("1.1.1.1")]);
        assert!(target.build_request().is_ok());
    }

    #[test]
    fn source_headers_are_fixed_and_never_accept_remote_headers() {
        let douban = source_headers(Some("douban"));
        assert_eq!(douban.get(USER_AGENT).unwrap(), BROWSER_USER_AGENT);
        assert_eq!(douban.get(ACCEPT).unwrap(), IMAGE_ACCEPT);
        assert_eq!(douban.get(REFERER).unwrap(), "https://m.douban.com/");
        assert!(douban.get("cookie").is_none());
        assert!(douban.get("authorization").is_none());

        let cms = source_headers(Some("cms10"));
        assert!(cms.get(REFERER).is_none());
    }

    #[test]
    fn dns_names_are_compared_without_case_or_terminal_dot() {
        assert_eq!(
            normalize_dns_name("IMG1.DOUBANIO.COM."),
            "img1.doubanio.com"
        );
    }

    #[test]
    fn proof_ttl_is_bounded() {
        assert_eq!(clamp_proof_ttl(Duration::from_secs(1)), PROOF_MIN_TTL);
        assert_eq!(clamp_proof_ttl(Duration::from_secs(600)), PROOF_MAX_TTL);
        assert_eq!(clamp_proof_ttl(Duration::from_secs(9999)), PROOF_MAX_TTL);
    }

    #[test]
    fn host_shape_guard_rejects_local_and_internal_names() {
        for raw in [
            "http://localhost/poster.jpg",
            "http://instance-data/poster.jpg",
            "http://metadata.google.internal/poster.jpg",
            "http://printer.local/poster.jpg",
            "http://service.internal/poster.jpg",
            "http://single-label/poster.jpg",
        ] {
            let url: Url = raw.parse().unwrap();
            let result = checked_source_host(&url, Some("cms10"), url.host_str());
            assert!(result.is_err(), "{raw}");
        }
    }

    #[tokio::test]
    async fn signed_query_is_allowed_only_for_explicitly_registered_mappings() {
        let network = ArtworkNetwork::new().unwrap();
        // 公网字面地址不触发 DNS 解析，因此这些断言是确定性的。
        let signed: Url = "https://1.1.1.1/a.jpg?token=opaque&expires=1"
            .parse()
            .unwrap();
        assert!(
            network
                .resolve_remote_target(&signed, Some("cms10"), Some("1.1.1.1"))
                .await
                .is_ok(),
            "显式登记的映射必须接受签名查询串"
        );
        let target = network
            .resolve_remote_target(&signed, Some("cms10"), Some("1.1.1.1"))
            .await
            .unwrap();
        assert_eq!(target.pinned_host, None, "字面地址不得带 DNS 覆盖键");
        assert_eq!(target.addresses, vec![socket("1.1.1.1")]);

        // 登记的来源标识存在但主机作用域不匹配 → 仍然拒绝。
        let error = network
            .resolve_remote_target(&signed, Some("cms10"), Some("img.example.com"))
            .await
            .unwrap_err();
        assert_eq!(error.code().as_str(), "SECURITY_POLICY_DENIED");

        // 缺少来源登记的旧记录：地址看起来是公网地址也不得获得网络授权。
        for (source_id, registered_host) in [(None, None), (None, Some("1.1.1.1"))] {
            assert!(
                network
                    .resolve_remote_target(&signed, source_id, registered_host)
                    .await
                    .is_err(),
                "缺少来源登记时必须 fail closed"
            );
        }

        // 超长地址即使已登记也拒绝。
        let oversized: Url = format!(
            "https://1.1.1.1/a.jpg?token={}",
            "a".repeat(MAX_ARTWORK_TARGET_URL_LEN)
        )
        .parse()
        .unwrap();
        assert!(
            network
                .resolve_remote_target(&oversized, Some("cms10"), Some("1.1.1.1"))
                .await
                .is_err()
        );

        // 非 http(s)、凭据 userinfo 与 fragment 依旧拒绝。
        for raw in [
            "file:///etc/passwd",
            "https://user:pass@1.1.1.1/a.jpg",
            "https://1.1.1.1/a.jpg#fragment",
        ] {
            let url: Url = raw.parse().unwrap();
            assert!(
                network
                    .resolve_remote_target(&url, Some("cms10"), Some("1.1.1.1"))
                    .await
                    .is_err(),
                "不得放行: {raw}"
            );
        }

        // 豆瓣靠主机族自动授权，没有显式主机作用域语义，继续拒绝签名串。
        let douban: Url = "https://img.doubanio.com/a.jpg?sig=opaque".parse().unwrap();
        assert!(
            network
                .resolve_remote_target(&douban, Some("douban"), Some("img.doubanio.com"))
                .await
                .is_err()
        );
    }

    #[tokio::test]
    async fn literal_fake_ip_is_denied_without_entering_the_proof_path() {
        let network = ArtworkNetwork::new().unwrap();
        // 字面 Fake-IP/保留地址必须直接拒绝：它们不能借用域名的 DoH 证明，
        // 也不能因为来源登记里写着同一个字面地址而被提升为可信。
        for raw in [
            "http://198.18.0.1/a.jpg",
            "http://[fdfe:dcba:9876::1]/a.jpg",
            "http://169.254.169.254/latest/meta-data",
        ] {
            let url: Url = raw.parse().unwrap();
            let error = network
                .resolve_remote_target(&url, Some("cms10"), url.host_str())
                .await
                .unwrap_err();
            assert_eq!(error.code().as_str(), "SECURITY_POLICY_DENIED", "{raw}");
        }
    }

    #[tokio::test]
    async fn host_scope_is_checked_before_any_lookup() {
        let network = ArtworkNetwork::new().unwrap();
        // 未登记 / 登记主机不匹配都必须在解析之前失败：这些域名不存在，
        // 断言因此与 DNS 无关。
        let url: Url = "https://not-registered.invalid/poster.jpg".parse().unwrap();
        for registered in [None, Some("other.example.com")] {
            let error = network
                .resolve_remote_target(&url, Some("cms10"), registered)
                .await
                .unwrap_err();
            assert_eq!(error.code().as_str(), "SECURITY_POLICY_DENIED");
        }
    }

    #[tokio::test]
    async fn denied_targets_never_echo_the_url_host_or_query() {
        let network = ArtworkNetwork::new().unwrap();
        let secret = "fixture-signature-opaque";
        let raw = format!("https://img.doubanio.com/p.jpg?signature={secret}&host=evil.example");
        let url: Url = raw.parse().unwrap();
        let error = network
            .resolve_remote_target(&url, Some("douban"), Some("img.doubanio.com"))
            .await
            .unwrap_err();
        let rendered = format!(
            "{} {} {error} {error:?}",
            error.code().as_str(),
            error.user_message()
        );
        assert!(!rendered.contains(secret), "错误文案不得包含查询串");
        assert!(
            !rendered.contains("doubanio.com"),
            "错误文案不得包含目标主机"
        );
        assert!(!rendered.contains("evil.example"));
    }

    #[test]
    fn artwork_target_debug_omits_url_query_host_addresses_and_source() {
        let token = "fixture-signature-opaque";
        let raw = format!("https://img.doubanio.com/poster.jpg?signature={token}&extra=1");
        let url: Url = raw.parse().unwrap();
        let target = compose_target(
            &url,
            Some("cms10"),
            "img.doubanio.com".to_owned(),
            vec![socket("93.184.216.34")],
        );
        // `{:?}` 与 `{:#?}` 都必须脱敏：诊断输出会进入日志、错误上报与 panic
        // 信息，字段值一旦被渲染出去就无法收回。
        for rendered in [format!("{target:?}"), format!("{target:#?}")] {
            assert!(
                rendered.contains("ArtworkTarget"),
                "仍然必须能看出类型: {rendered}"
            );
            for leaked in [
                token,
                "doubanio.com",
                "signature",
                "/poster.jpg",
                "93.184.216.34",
                "cms10",
            ] {
                assert!(
                    !rendered.contains(leaked),
                    "Debug 泄露 {leaked}: {rendered}"
                );
            }
        }
        // 脱敏只发生在 Debug 渲染上：字段本身仍可读，且 `unwrap_err` 这类诊断
        // 仍然依赖 `ArtworkTarget` 实现 `Debug`。
        assert_eq!(target.url().as_str(), raw);
        assert_eq!(target.normalized_host, "img.doubanio.com");
        assert_eq!(target.source_id.as_deref(), Some("cms10"));
        assert_eq!(target.addresses, vec![socket("93.184.216.34")]);
        assert!(target.build_request().is_ok());
    }

    #[tokio::test]
    async fn literal_loopback_stays_rejected_by_production_validation() {
        let network = ArtworkNetwork::new().unwrap();
        // test-only 的环回固定地址只能经由私有 compose_target 注入；生产入口
        // 仍然拒绝字面环回地址。字面地址不触发系统解析，因此断言与 DNS 无关。
        for raw in ["http://127.0.0.1/a.jpg", "http://127.0.0.1:8443/a.jpg"] {
            let url: Url = raw.parse().unwrap();
            let error = network
                .resolve_remote_target(&url, Some("cms10"), url.host_str())
                .await
                .unwrap_err();
            assert_eq!(error.code().as_str(), "SECURITY_POLICY_DENIED", "{raw}");
        }
    }
}

/// 物理传输回归独立成模块：它需要本地监听与真实 socket I/O，与上面的纯策略
/// 断言不是同一层证据。
#[cfg(test)]
#[path = "artwork_network/transport_tests.rs"]
mod transport_tests;
