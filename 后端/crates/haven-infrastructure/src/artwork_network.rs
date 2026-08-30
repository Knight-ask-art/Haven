//! Artwork 专用出站策略。
//!
//! Artwork 的来源 URL 来自 Provider/用户配置，不能因为“已经登记”就直接
//! 认为安全。该模块把 URL 形状、来源 Host、系统 DNS 结果和实际请求头
//! 收敛在一个边界内。部分 Windows 代理使用 Fake-IP（198.18/15 加固定
//! ULA 前缀），这时必须用独立的公共 DoH 证明 Host 的真实解析仍然是公网
//! 地址，不能直接放行整个保留网段。

use std::collections::HashMap;
use std::net::{IpAddr, SocketAddr};
use std::sync::Arc;
use std::time::Duration;

use haven_common::{AppError, ErrorKind};
use reqwest::header::{ACCEPT, HeaderMap, HeaderValue, REFERER, USER_AGENT};
use reqwest::redirect::Policy;
use reqwest::{RequestBuilder, Url};
use serde::Deserialize;
use tokio::sync::{Mutex, OwnedSemaphorePermit, Semaphore};
use tokio::time::Instant;

use crate::db::repos::image_proxy::{has_sensitive_query, is_private_ip};

const CONNECT_TIMEOUT: Duration = Duration::from_millis(1_500);
const REQUEST_TIMEOUT: Duration = Duration::from_secs(3);
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
    fetch_client: reqwest::Client,
    doh_client: reqwest::Client,
    proofs: ProofTable,
    fetch_slots: Arc<Semaphore>,
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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum ResolutionRoute {
    Public,
    FakeIp,
    Denied,
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
        let fetch_client = reqwest::Client::builder()
            .redirect(Policy::none())
            .connect_timeout(CONNECT_TIMEOUT)
            .timeout(REQUEST_TIMEOUT)
            .build()
            .map_err(|_| client_init_error())?;
        let doh_client = reqwest::Client::builder()
            .redirect(Policy::none())
            .connect_timeout(DOH_CONNECT_TIMEOUT)
            .timeout(DOH_REQUEST_TIMEOUT)
            .user_agent(DOH_USER_AGENT)
            .build()
            .map_err(|_| client_init_error())?;
        Ok(Self {
            fetch_client,
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

    pub(super) fn request(&self, url: &Url, source_id: Option<&str>) -> RequestBuilder {
        self.fetch_client
            .get(url.clone())
            .headers(source_headers(source_id))
    }

    pub(super) async fn validate_remote_url(
        &self,
        url: &Url,
        source_id: Option<&str>,
        registered_host: Option<&str>,
    ) -> Result<(), AppError> {
        if !matches!(url.scheme(), "http" | "https")
            || !url.username().is_empty()
            || url.password().is_some()
            || url.fragment().is_some()
            || has_sensitive_query(url)
        {
            return Err(artwork_security("图片地址不符合出站策略"));
        }
        let host = url
            .host_str()
            .ok_or_else(|| artwork_security("图片地址缺少主机"))?
            .to_ascii_lowercase();
        if host == "localhost"
            || matches!(host.as_str(), "metadata.google.internal" | "instance-data")
            || (source_id == Some("douban")
                && host != "doubanio.com"
                && !host.ends_with(".doubanio.com"))
            || (source_id != Some("douban") && registered_host.is_none_or(|value| value != host))
        {
            return Err(artwork_security("图片主机不在来源允许列表"));
        }

        let port = url
            .port_or_known_default()
            .ok_or_else(|| artwork_security("图片端口无效"))?;
        // A literal IP is already the connection target.  It must never be
        // promoted into the hostname Fake-IP/DoH proof path: a literal
        // benchmark or private address is rejected outright, while a public
        // literal can only be used when the source registration already
        // allowed that exact host.
        if let Ok(ip) = host.parse::<IpAddr>() {
            return if is_public_dns_ip(ip) {
                Ok(())
            } else {
                Err(artwork_security("图片主机解析到禁止的网络地址"))
            };
        }
        let addresses: Vec<SocketAddr> = tokio::net::lookup_host((host.as_str(), port))
            .await
            .map_err(|_| artwork_security("图片主机解析失败"))?
            .collect();
        match classify_addresses(addresses.iter().map(|address| address.ip())) {
            ResolutionRoute::Public => Ok(()),
            ResolutionRoute::FakeIp => self.prove_public_host(&host).await,
            ResolutionRoute::Denied => Err(artwork_security("图片主机解析到禁止的网络地址")),
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
        if ips.is_empty() || ips.iter().any(|ip| !is_public_dns_ip(*ip)) {
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

fn classify_addresses<I>(addresses: I) -> ResolutionRoute
where
    I: IntoIterator<Item = IpAddr>,
{
    let mut seen = false;
    let mut fake = false;
    let mut public = false;
    for ip in addresses {
        seen = true;
        if is_known_fake_ip(ip) {
            fake = true;
            continue;
        }
        if !is_public_dns_ip(ip) {
            return ResolutionRoute::Denied;
        }
        public = true;
    }
    if !seen {
        ResolutionRoute::Denied
    } else if fake && !public {
        ResolutionRoute::FakeIp
    } else if !fake && public {
        ResolutionRoute::Public
    } else {
        ResolutionRoute::Denied
    }
}

fn is_public_dns_ip(ip: IpAddr) -> bool {
    !is_private_ip(ip)
        && !is_known_fake_ip(ip)
        && match ip {
            IpAddr::V4(value) => !value.is_multicast(),
            IpAddr::V6(value) => !value.is_multicast(),
        }
}

fn is_known_fake_ip(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(value) => {
            let octets = value.octets();
            octets[0] == 198 && matches!(octets[1], 18 | 19)
        }
        IpAddr::V6(value) => {
            let segments = value.segments();
            segments[0..4] == [0xfdfe, 0xdcba, 0x9876, 0]
        }
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

    #[test]
    fn fake_ip_detection_is_narrow_and_three_state() {
        assert_eq!(
            classify_addresses([
                "198.18.0.105".parse().unwrap(),
                "fdfe:dcba:9876::ee".parse().unwrap(),
            ]),
            ResolutionRoute::FakeIp
        );
        assert_eq!(
            classify_addresses(["1.1.1.1".parse().unwrap()]),
            ResolutionRoute::Public
        );
        assert_eq!(
            classify_addresses(["198.18.0.105".parse().unwrap(), "1.1.1.1".parse().unwrap()]),
            ResolutionRoute::Denied
        );
        assert_eq!(
            classify_addresses(["198.18.0.105".parse().unwrap(), "10.0.0.1".parse().unwrap()]),
            ResolutionRoute::Denied
        );
        assert_eq!(classify_addresses([]), ResolutionRoute::Denied);
        assert!(is_known_fake_ip("198.19.255.255".parse().unwrap()));
        assert!(!is_known_fake_ip("198.20.0.1".parse().unwrap()));
        assert!(!is_known_fake_ip("fdfe:dcba:9877::1".parse().unwrap()));
    }

    #[test]
    fn public_dns_proof_rejects_reserved_and_fake_addresses() {
        for value in [
            "127.0.0.1",
            "10.0.0.1",
            "169.254.169.254",
            "198.18.0.1",
            "fdfe:dcba:9876::1",
            "224.0.0.1",
        ] {
            assert!(!is_public_dns_ip(value.parse().unwrap()), "{value}");
        }
        assert!(is_public_dns_ip("180.97.198.41".parse().unwrap()));
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
}
