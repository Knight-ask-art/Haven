//! Shared outbound HTTP URL policy for source and media boundaries.
//!
//! This module owns the syntax and literal-address part of the policy so
//! SourceRegistry, M3U import, stream opening and the Tauri resource proxy do
//! not silently drift apart. It intentionally does not perform DNS lookup:
//! the I/O owner must resolve a domain and pin the approved addresses at the
//! connection boundary.

use std::fmt;
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};

use url::Url;

/// Context-specific URL policy. Fixed source/media policies retain their
/// existing boundaries. `UserProvidedSource` is the explicit exception for
/// destinations entered by the user in source settings. `AiProviderEndpoint`
/// accepts only explicitly configured canonical 127.0.0.1 / ::1 literals on
/// nonzero ports; it does not authorize LAN discovery or DNS loopback targets.
///
/// One thing this policy deliberately does **not** encode: confidentiality.
/// It accepts `http` because it is shared with callers that have no credential
/// to protect, so the AI provider path adds its own HTTPS requirement on top
/// except for those local-only AI endpoints
/// (`haven_domain::ai_provider::validate_endpoint` for saving,
/// `AiProviderProfile::require_secure_endpoint` before any outbound request).
/// Public AI endpoints and user-provided source requests carrying credentials
/// still require HTTPS.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HttpUrlPolicy {
    SourceEndpoint,
    MediaResource,
    AiProviderEndpoint,
    /// A self-hosted or public endpoint the user typed into the source settings
    /// (custom OPDS / RSS-Atom feed / Komga / Kavita / TVBox config / CMS10 / M3U).
    ///
    /// The user has explicitly chosen this destination, so private LAN, loopback,
    /// unique-local IPv6, shared overlay-network addresses, and local DNS names
    /// are supported for personal/self-hosted servers. Public source requests
    /// retain the same behavior. Link-local, unspecified, multicast, cloud
    /// metadata and other reserved addresses remain rejected. HTTP(S), a
    /// non-empty authority, nonzero port, no user-info/fragment/control
    /// characters/whitespace are still required. This policy is only used for
    /// user-configured sources; fixed metadata and media-resource requests keep
    /// their stricter policies.
    UserProvidedSource,
}

/// One capability/stream owner resolves the outbound URL policy for a
/// server-side source identity, so every downstream I/O boundary (capability
/// projection, grant registration, manifest target resolution, DNS validation)
/// reuses exactly this value instead of re-deriving a policy from a browser
/// URL, an arbitrary string prefix or a client-supplied flag.
///
/// Unscoped resources keep [`HttpUrlPolicy::MediaResource`]; the user-configured
/// source scope is only reached through a persisted, server-owned source
/// identity.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StreamUrlScope {
    /// Default fixed/public media boundary.
    Media,
    /// Streams persisted by a user-configured source (LAN M3U channel).
    UserProvidedSource,
}

impl StreamUrlScope {
    pub fn policy(self) -> HttpUrlPolicy {
        match self {
            Self::Media => HttpUrlPolicy::MediaResource,
            Self::UserProvidedSource => HttpUrlPolicy::UserProvidedSource,
        }
    }

    /// Whether a literal/DNS address is an acceptable destination for this
    /// scope. This is the single address-class decision shared by literal URL
    /// validation and post-DNS pinning, so a user-scoped stream may reach a
    /// private LAN address that the default media policy still denies.
    pub fn allows_address(self, host: &str, ip: IpAddr) -> bool {
        is_allowed_http_address(host, ip, self.policy())
    }
}

/// Errors are intentionally coarse so callers can map them to a safe user
/// message without echoing the rejected URL or its credentials.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HttpUrlError {
    InvalidUrl,
    UnsupportedScheme,
    MissingHost,
    UserInfo,
    Fragment,
    UnsafeHost,
    DisallowedPort,
}

impl fmt::Display for HttpUrlError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::InvalidUrl => "invalid HTTP URL",
            Self::UnsupportedScheme => "unsupported HTTP URL scheme",
            Self::MissingHost => "HTTP URL has no host",
            Self::UserInfo => "HTTP URL user-info is not allowed",
            Self::Fragment => "HTTP URL fragment is not allowed",
            Self::UnsafeHost => "HTTP URL host is not allowed",
            Self::DisallowedPort => "HTTP URL port is not allowed",
        })
    }
}

impl std::error::Error for HttpUrlError {}

/// A parsed URL whose scheme, authority, port and literal host have passed the
/// shared policy. The URL is still not a network authorization by itself:
/// callers must apply source/grant permissions and pin DNS at I/O time.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SafeHttpUrl {
    url: Url,
    host: String,
}

impl SafeHttpUrl {
    pub fn as_url(&self) -> &Url {
        &self.url
    }

    pub fn into_url(self) -> Url {
        self.url
    }

    pub fn as_str(&self) -> &str {
        self.url.as_str()
    }

    /// Normalized host used by allowlists. IPv6 brackets are not included.
    pub fn host(&self) -> &str {
        &self.host
    }
}

/// Parse and validate one absolute HTTP(S) URL.
pub fn parse_http_url(raw: &str, policy: HttpUrlPolicy) -> Result<SafeHttpUrl, HttpUrlError> {
    if raw.is_empty() || raw.chars().any(|ch| ch.is_control() || ch.is_whitespace()) {
        return Err(HttpUrlError::InvalidUrl);
    }
    let url = Url::parse(raw).map_err(|_| HttpUrlError::InvalidUrl)?;
    if !url.scheme().eq_ignore_ascii_case("http") && !url.scheme().eq_ignore_ascii_case("https") {
        return Err(HttpUrlError::UnsupportedScheme);
    }
    if !url.username().is_empty() || url.password().is_some() {
        return Err(HttpUrlError::UserInfo);
    }
    if url.fragment().is_some() {
        return Err(HttpUrlError::Fragment);
    }
    if has_empty_http_authority(raw) {
        return Err(HttpUrlError::MissingHost);
    }
    if has_explicit_empty_port(raw) {
        return Err(HttpUrlError::DisallowedPort);
    }
    let host = url.host_str().ok_or(HttpUrlError::MissingHost)?;
    let literal = host.trim_start_matches('[').trim_end_matches(']');
    let ai_loopback = policy == HttpUrlPolicy::AiProviderEndpoint
        && literal
            .parse::<IpAddr>()
            .is_ok_and(|ip| ai_provider_loopback_allowed(literal, ip));
    let host = if ai_loopback {
        if raw.contains('\\') {
            return Err(HttpUrlError::InvalidUrl);
        }
        // WHATWG URL parsing normalizes aliases such as 127.1 and 2130706433.
        // A local credential destination must be spelled as an explicit literal.
        let authority = raw
            .split_once("://")
            .map(|(_, tail)| tail.split(['/', '?', '#']).next().unwrap_or_default())
            .ok_or(HttpUrlError::UnsafeHost)?;
        let raw_host = if authority.starts_with('[') {
            authority.split_once(']').map(|(host, _)| &host[1..])
        } else {
            Some(authority.split(':').next().unwrap_or_default())
        };
        if raw_host != Some(literal) {
            return Err(HttpUrlError::UnsafeHost);
        }
        literal.to_owned()
    } else if policy == HttpUrlPolicy::UserProvidedSource {
        validate_user_provided_source_host(host)?
    } else {
        validate_host(host)?
    };
    let port = url
        .port_or_known_default()
        .ok_or(HttpUrlError::DisallowedPort)?;
    if !allowed_port(policy, port, &host) {
        return Err(HttpUrlError::DisallowedPort);
    }
    Ok(SafeHttpUrl { url, host })
}

/// WHATWG-compatible parsing can reinterpret an empty special-scheme
/// authority such as `https:///path` as a host-like path segment. Preserve the
/// caller's authority boundary and reject that form explicitly.
fn has_empty_http_authority(raw: &str) -> bool {
    let Some((scheme, remainder)) = raw.split_once("://") else {
        return false;
    };
    if !scheme.eq_ignore_ascii_case("http") && !scheme.eq_ignore_ascii_case("https") {
        return false;
    }
    remainder
        .split_once(['/', '?', '#'])
        .map_or(remainder, |(authority, _)| authority)
        .is_empty()
}

/// `url::Url` treats an explicitly empty port (`https://host:/path`) like an
/// omitted port.  That is too permissive for an authorization boundary: keep
/// the distinction from the raw authority and reject it before applying the
/// scheme's default port.
fn has_explicit_empty_port(raw: &str) -> bool {
    let Some((_, remainder)) = raw.split_once("://") else {
        return false;
    };
    let authority = remainder
        .split_once(['/', '?', '#'])
        .map_or(remainder, |(authority, _)| authority);
    if authority.starts_with('[') {
        let Some(close) = authority.find(']') else {
            return false;
        };
        return authority
            .get(close + 1..)
            .is_some_and(|suffix| suffix == ":");
    }
    authority
        .rsplit_once(':')
        .is_some_and(|(_, port)| port.is_empty())
}

/// Validate an already extracted host before adding it to a grant allowlist.
pub fn validate_host(raw: &str) -> Result<String, HttpUrlError> {
    let host = normalize_host(raw)?;
    if let Ok(ip) = host.parse::<IpAddr>() {
        if is_publicly_routable(ip) || fixture_loopback_allowed(&host, ip) {
            return Ok(host);
        }
        return Err(HttpUrlError::UnsafeHost);
    }

    validate_dns_name(&host)?;
    if host.split('.').count() < 2
        || host == "localhost"
        || host.ends_with(".localhost")
        || host.ends_with(".local")
        || host == "metadata.google.internal"
        || host.ends_with(".internal")
    {
        return Err(HttpUrlError::UnsafeHost);
    }
    Ok(host)
}

/// Host validation for an endpoint the user explicitly configured as a source.
/// Unlike fixed-source/media policies, this accepts common local server names
/// and addresses while still rejecting link-local and reserved IP ranges.
fn validate_user_provided_source_host(raw: &str) -> Result<String, HttpUrlError> {
    let host = normalize_host(raw)?;
    if let Ok(ip) = host.parse::<IpAddr>() {
        if is_publicly_routable(ip) || is_user_source_local_ip_allowed(ip) {
            return Ok(host);
        }
        return Err(HttpUrlError::UnsafeHost);
    }
    validate_dns_name(&host)?;
    // Keep the well-known cloud metadata hostname blocked even when explicitly
    // entered; its link-local target is not a personal catalog endpoint.
    if host == "metadata.google.internal" {
        return Err(HttpUrlError::UnsafeHost);
    }
    Ok(host)
}

fn normalize_host(raw: &str) -> Result<String, HttpUrlError> {
    if raw.is_empty() || raw.chars().any(|ch| ch.is_control() || ch.is_whitespace()) {
        return Err(HttpUrlError::UnsafeHost);
    }
    let host = raw
        .strip_prefix('[')
        .and_then(|value| value.strip_suffix(']'))
        .unwrap_or(raw)
        .trim_end_matches('.')
        .to_ascii_lowercase();
    if host.is_empty() {
        return Err(HttpUrlError::MissingHost);
    }
    Ok(host)
}

fn validate_dns_name(host: &str) -> Result<(), HttpUrlError> {
    let labels: Vec<&str> = host.split('.').collect();
    if host.len() > 253
        || labels.iter().any(|label| {
            label.is_empty()
                || label.len() > 63
                || label.starts_with('-')
                || label.ends_with('-')
                || !label
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-')
        })
    {
        return Err(HttpUrlError::UnsafeHost);
    }
    Ok(())
}

/// Normalized `scheme://host[:port]` origin used by credential-binding checks.
///
/// This is pure syntax normalization (lowercase host, terminal dot removed, the
/// scheme's default port dropped). It performs **no** security decision: callers
/// must still parse every URL through [`parse_http_url`] before acting on it.
/// Two URLs compared through this helper are normalized identically, so a caller
/// that only injects a credential when the request origin equals the registered
/// catalog origin cannot be confused by host case, a trailing dot or an
/// explicitly written default port.
pub fn url_origin(url: &Url) -> Option<String> {
    let scheme = url.scheme().to_ascii_lowercase();
    if scheme != "http" && scheme != "https" {
        return None;
    }
    let host = url.host_str()?.trim_end_matches('.').to_ascii_lowercase();
    if host.is_empty() {
        return None;
    }
    Some(match url.port() {
        Some(port) => format!("{scheme}://{host}:{port}"),
        None => format!("{scheme}://{host}"),
    })
}

/// Raw endpoint text → normalized origin ([`url_origin`]). `None` for anything
/// that is not a parseable absolute http(s) URL.
pub fn origin_of(raw: &str) -> Option<String> {
    Url::parse(raw).ok().and_then(|url| url_origin(&url))
}

/// Literal IP policy shared by URL parsing and DNS-pinning callers.
pub fn is_publicly_routable(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(ip) => is_public_ipv4(ip),
        IpAddr::V6(ip) => is_public_ipv6(ip),
    }
}

/// Apply the request policy to one literal or DNS answer before pinning it.
pub fn is_allowed_http_address(host: &str, ip: IpAddr, policy: HttpUrlPolicy) -> bool {
    is_publicly_routable(ip)
        || fixture_loopback_allowed(host, ip)
        || (policy == HttpUrlPolicy::AiProviderEndpoint && ai_provider_loopback_allowed(host, ip))
        || (host.parse::<IpAddr>().is_err() && is_local_proxy_fake_ip(ip))
        || (policy == HttpUrlPolicy::UserProvidedSource && is_user_source_local_ip_allowed(ip))
}

/// Private destinations are accepted only for an explicitly user-configured
/// source endpoint. Link-local, unspecified, multicast and reserved ranges are
/// intentionally excluded; 100.64.0.0/10 is included for common personal VPNs.
pub fn is_user_source_local_ip_allowed(ip: IpAddr) -> bool {
    if is_local_proxy_fake_ip(ip) || ip == IpAddr::V4(Ipv4Addr::new(100, 100, 100, 200)) {
        return false;
    }
    match ip {
        IpAddr::V4(address) => {
            address.is_private() || address.is_loopback() || is_shared_ipv4(address)
        }
        IpAddr::V6(address) => address.is_loopback() || address.is_unique_local(),
    }
}

/// Only canonical literal loopback AI targets qualify; DNS answers do not.
/// Callers must also check the policy and parse the URL through this module.
pub fn ai_provider_loopback_allowed(host: &str, ip: IpAddr) -> bool {
    (host == "127.0.0.1" && ip == IpAddr::V4(Ipv4Addr::LOCALHOST))
        || (host == "::1" && ip == IpAddr::V6(Ipv6Addr::LOCALHOST))
}

fn allowed_port(policy: HttpUrlPolicy, port: u16, host: &str) -> bool {
    if policy == HttpUrlPolicy::UserProvidedSource {
        return port != 0;
    }
    if policy == HttpUrlPolicy::AiProviderEndpoint
        && host
            .parse::<IpAddr>()
            .is_ok_and(|ip| ai_provider_loopback_allowed(host, ip))
    {
        return port != 0;
    }
    if host
        .parse::<IpAddr>()
        .is_ok_and(|ip| fixture_loopback_allowed(host, ip))
    {
        return matches!(port, 80 | 443 | 8080 | 8443) || (49152..=65535).contains(&port);
    }
    matches!(port, 80 | 443 | 8080 | 8443)
}

/// Recognized local DNS proxy Fake-IP prefixes.
///
/// This classifies addresses but does not grant request access. The transport
/// may accept these addresses as DNS answers for hostnames; URL literals stay
/// rejected, including when entered as a user-configured source.
pub fn is_local_proxy_fake_ip(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(address) => {
            let [first, second, _, _] = address.octets();
            first == 198 && matches!(second, 18 | 19)
        }
        IpAddr::V6(address) => address.segments()[..4] == [0xfdfe, 0xdcba, 0x9876, 0],
    }
}

fn is_shared_ipv4(ip: Ipv4Addr) -> bool {
    let [first, second, _, _] = ip.octets();
    first == 100 && (64..=127).contains(&second)
}

/// Candidate-only network escape hatch for the deterministic local fixture.
///
/// This is intentionally narrower than "localhost" or "private network":
/// only the literal IPv4 loopback address is allowed, and non-standard ports
/// must be in the ephemeral range. The common 80/443/8080/8443 ports remain
/// governed by the normal port allowlist. This is enabled only when the
/// `film-tv-fixture` feature is compiled into the candidate; the default/
/// release build returns false at compile time.
pub fn fixture_loopback_allowed(host: &str, ip: IpAddr) -> bool {
    #[cfg(feature = "film-tv-fixture")]
    {
        return host == "127.0.0.1" && ip == IpAddr::V4(Ipv4Addr::LOCALHOST);
    }
    #[cfg(not(feature = "film-tv-fixture"))]
    {
        let _ = (host, ip);
        false
    }
}

fn is_public_ipv4(ip: Ipv4Addr) -> bool {
    let [a, b, c, d] = ip.octets();
    if a == 0 || a == 10 || a == 127 || a >= 224 {
        return false;
    }
    if a == 100 && (64..=127).contains(&b) {
        return false;
    }
    if a == 169 && b == 254 {
        return false;
    }
    if a == 172 && (16..=31).contains(&b) {
        return false;
    }
    if a == 192 && (b == 0 || b == 2 || b == 168) {
        return false;
    }
    if a == 198 && (b == 18 || b == 19 || b == 51) {
        return false;
    }
    if a == 203 && b == 0 && c == 113 {
        return false;
    }
    // The all-ones broadcast address is covered by a >=224 check in practice,
    // but keep the final octet binding explicit for readability.
    !(a == 255 && b == 255 && c == 255 && d == 255)
}

fn is_public_ipv6(ip: Ipv6Addr) -> bool {
    let segments = ip.segments();
    if ip.is_unspecified() || ip.is_loopback() || ip.is_multicast() {
        return false;
    }
    // fc00::/7 unique-local, fe80::/10 link-local and fec0::/10 deprecated
    // site-local ranges must never be reachable through a media proxy.
    if (segments[0] & 0xfe00) == 0xfc00
        || (segments[0] & 0xffc0) == 0xfe80
        || (segments[0] & 0xffc0) == 0xfec0
    {
        return false;
    }
    // Documentation, benchmarking and ORCHID ranges are not public media
    // destinations and make unsafe fixtures too easy to mistake for real ones.
    if segments[0] == 0x2001
        && ((segments[1] == 0x0db8)
            || (segments[1] == 0x0002 && segments[2] == 0)
            || (segments[1] & 0xfff0) == 0x0010)
    {
        return false;
    }
    if let Some(mapped) = ip.to_ipv4() {
        return is_public_ipv4(mapped);
    }
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ai_loopback_is_explicit_scoped_and_accepts_nonzero_ports() {
        for raw in [
            "http://127.0.0.1:3123/v1",
            "http://127.0.0.1/v1",
            "https://127.0.0.1:11434/v1",
            "http://[::1]:3123/v1",
            "https://[::1]:65535/v1",
        ] {
            assert!(
                parse_http_url(raw, HttpUrlPolicy::AiProviderEndpoint).is_ok(),
                "{raw}"
            );
            for policy in [HttpUrlPolicy::SourceEndpoint, HttpUrlPolicy::MediaResource] {
                // The unrelated opt-in Film/TV fixture remains its own authority.
                #[cfg(not(feature = "film-tv-fixture"))]
                assert!(parse_http_url(raw, policy).is_err(), "{policy:?}: {raw}");
                #[cfg(feature = "film-tv-fixture")]
                if !raw.contains("127.0.0.1") {
                    assert!(parse_http_url(raw, policy).is_err());
                }
            }
        }
    }

    #[test]
    fn ai_loopback_does_not_authorize_aliases_dns_lan_or_invalid_authorities() {
        for raw in [
            "http://127.0.0.1:0/v1",
            "http://[::1]:0/v1",
            "http://127.0.0.1:/v1",
            "http://127.0.0.1:65536/v1",
            "http://127.1:3123/v1",
            "http://2130706433:3123/v1",
            "http://0x7f000001:3123/v1",
            "http://127.0.0.1:3123\\evil/v1",
            "http://[::1]:3123\\evil/v1",
            "http://[0:0:0:0:0:0:0:1]:3123/v1",
            "http://127.0.0.1.:3123/v1",
            "http://127.0.0.2:3123/v1",
            "http://[::ffff:127.0.0.1]:3123/v1",
            "http://localhost:3123/v1",
            "http://gateway.local:3123/v1",
            "http://0.0.0.0:3123/v1",
            "http://[::]:3123/v1",
            "http://192.168.1.10/v1",
            "http://10.0.0.1/v1",
            "http://169.254.169.254/v1",
            "http://user:secret@127.0.0.1:3123/v1",
            "file:///v1",
            "http://127.0.0.1:3123/v1#fragment",
        ] {
            assert!(
                parse_http_url(raw, HttpUrlPolicy::AiProviderEndpoint).is_err(),
                "{raw}"
            );
        }
        assert!(!is_publicly_routable(IpAddr::V4(Ipv4Addr::LOCALHOST)));
        assert!(!is_publicly_routable(IpAddr::V6(Ipv6Addr::LOCALHOST)));
    }

    #[test]
    fn accepts_public_domains_and_common_media_ports() {
        for raw in [
            "https://cdn.example.invalid/video/index.m3u8",
            "http://media.example.invalid:8080/video.mp4",
            "https://[2001:4860:4860::8888]:8443/video.mp4",
        ] {
            let parsed = parse_http_url(raw, HttpUrlPolicy::MediaResource).unwrap();
            assert!(!parsed.host().is_empty());
        }
    }

    #[test]
    fn normalizes_host_without_changing_the_request_url() {
        let parsed = parse_http_url(
            "HTTPS://CDN.Example.Invalid./video.mp4",
            HttpUrlPolicy::SourceEndpoint,
        )
        .unwrap();
        assert_eq!(parsed.host(), "cdn.example.invalid");
        assert_eq!(parsed.as_url().scheme(), "https");
    }

    #[cfg(not(feature = "film-tv-fixture"))]
    #[test]
    fn rejects_unsafe_literal_addresses() {
        for raw in [
            "http://127.0.0.1/video.mp4",
            "http://10.0.0.1/video.mp4",
            "http://172.16.0.1/video.mp4",
            "http://192.168.1.1/video.mp4",
            "http://169.254.169.254/latest/meta-data",
            "http://100.100.100.200/video.mp4",
            "http://198.18.0.1/video.mp4",
            "http://198.19.255.254/video.mp4",
            "https://[fdfe:dcba:9876::1]/video.mp4",
            "https://[::1]/video.mp4",
            "https://[fc00::1]/video.mp4",
            "https://[fe80::1]/video.mp4",
            "https://[2001:db8::1]/video.mp4",
        ] {
            assert_eq!(
                parse_http_url(raw, HttpUrlPolicy::MediaResource),
                Err(HttpUrlError::UnsafeHost),
                "unsafe URL accepted: {raw}"
            );
        }
    }

    #[test]
    fn fake_ip_range_is_rejected_as_a_literal_for_every_http_policy() {
        for policy in [
            HttpUrlPolicy::MediaResource,
            HttpUrlPolicy::SourceEndpoint,
            HttpUrlPolicy::AiProviderEndpoint,
            HttpUrlPolicy::UserProvidedSource,
        ] {
            for raw in [
                "http://198.18.0.1:443/video.mp4",
                "https://198.19.255.254:443/opds",
                "https://[fdfe:dcba:9876::1]:443/opds",
            ] {
                assert!(
                    parse_http_url(raw, policy).is_err(),
                    "Fake-IP must remain blocked as a literal for {policy:?}: {raw}"
                );
            }
        }
    }

    #[test]
    fn local_proxy_fake_ip_classification_is_exact() {
        for raw in ["198.18.0.1", "198.19.255.254", "fdfe:dcba:9876::1"] {
            assert!(is_local_proxy_fake_ip(raw.parse().unwrap()), "{raw}");
        }
        for raw in [
            "198.20.0.1",
            "198.17.255.255",
            "fdfe:dcba:9877::1",
            "fc00::1",
            "10.0.0.1",
        ] {
            assert!(!is_local_proxy_fake_ip(raw.parse().unwrap()), "{raw}");
        }
    }

    #[cfg(feature = "film-tv-fixture")]
    #[test]
    fn fixture_mode_only_accepts_literal_loopback_ephemeral_endpoint() {
        let parsed = parse_http_url(
            "http://127.0.0.1:49152/hls/master.m3u8",
            HttpUrlPolicy::MediaResource,
        )
        .unwrap();
        assert_eq!(parsed.host(), "127.0.0.1");
        assert!(
            parse_http_url(
                "http://127.0.0.1:80/hls/master.m3u8",
                HttpUrlPolicy::MediaResource,
            )
            .is_ok()
        );
        assert!(
            parse_http_url(
                "http://localhost:49152/hls/master.m3u8",
                HttpUrlPolicy::MediaResource,
            )
            .is_err()
        );
        assert!(
            parse_http_url(
                "http://127.0.0.2:49152/hls/master.m3u8",
                HttpUrlPolicy::MediaResource,
            )
            .is_err()
        );
        for raw in [
            "http://10.0.0.1:49152/hls/master.m3u8",
            "http://192.168.1.1:49152/hls/master.m3u8",
            "http://169.254.169.254:49152/latest-meta-data",
        ] {
            assert!(
                parse_http_url(raw, HttpUrlPolicy::MediaResource).is_err(),
                "fixture mode accepted a non-loopback private address: {raw}"
            );
        }
    }

    #[test]
    fn rejects_userinfo_fragments_bad_ports_and_ambiguous_hosts() {
        for raw in [
            "https://user:secret@cdn.example.invalid/video.mp4",
            "https://cdn.example.invalid/video.mp4#fragment",
            "https://cdn.example.invalid:99999/video.mp4",
            "https://cdn.example.invalid:12345/video.mp4",
            "https://cdn.example.invalid:/video.mp4",
            "https://2001:4860:4860::8888/video.mp4",
            "ftp://cdn.example.invalid/video.mp4",
            "https://localhost/video.mp4",
            "https://media.local/video.mp4",
            "https://media.internal/video.mp4",
        ] {
            assert!(
                parse_http_url(raw, HttpUrlPolicy::MediaResource).is_err(),
                "unsafe URL accepted: {raw}"
            );
        }
    }

    #[test]
    fn rejects_single_label_and_malformed_hosts() {
        for raw in [
            "https://localhost:8443/video.mp4",
            "https://media/video.mp4",
            "https://-bad.example/video.mp4",
            "https://bad-.example/video.mp4",
            "https://a..example/video.mp4",
        ] {
            assert!(parse_http_url(raw, HttpUrlPolicy::SourceEndpoint).is_err());
        }
    }

    #[test]
    fn user_provided_source_accepts_public_hosts_on_arbitrary_nonzero_ports() {
        for raw in [
            "https://media.example.invalid:8096/api",
            "https://media.example.invalid:32400/library",
            "https://catalog.example.invalid:5001/opds",
            "https://catalog.example.invalid:9117/feed.xml",
            "http://catalog.example.invalid:9973/api.php",
            "https://media.example.invalid:1/api",
            "https://media.example.invalid:65535/api",
            "https://1.1.1.1:8096/api",
        ] {
            let parsed = parse_http_url(raw, HttpUrlPolicy::UserProvidedSource)
                .unwrap_or_else(|error| panic!("{raw}: {error:?}"));
            assert!(!parsed.host().is_empty());
        }
        // The fixed policies keep their exact previous port set: a self-hosted
        // port is not silently granted to media/source/AI requests.
        for policy in [
            HttpUrlPolicy::SourceEndpoint,
            HttpUrlPolicy::MediaResource,
            HttpUrlPolicy::AiProviderEndpoint,
        ] {
            for raw in [
                "https://media.example.invalid:8096/api",
                "https://media.example.invalid:32400/library",
                "https://catalog.example.invalid:5001/opds",
            ] {
                assert_eq!(
                    parse_http_url(raw, policy),
                    Err(HttpUrlError::DisallowedPort),
                    "{policy:?}: {raw}"
                );
            }
            assert!(parse_http_url("https://media.example.invalid:8080/api", policy).is_ok());
            assert!(parse_http_url("https://media.example.invalid/api", policy).is_ok());
        }
    }

    #[test]
    fn user_provided_source_accepts_local_self_hosted_endpoints() {
        for raw in [
            "http://127.0.0.1:8096/api",
            "http://localhost:32400/library",
            "https://192.168.1.10:5001/opds",
            "http://10.0.0.5:8088/feed.xml",
            "http://172.16.3.4:9973/api.php",
            "https://[::1]:8080/opds",
            "https://[fd12::10]:8096/api",
            "http://nas:5001/opds",
            "http://komga.local:8096/api",
            "https://reader.home.internal:5001/opds",
            "http://100.64.1.10:8096/api",
        ] {
            assert!(
                parse_http_url(raw, HttpUrlPolicy::UserProvidedSource).is_ok(),
                "explicitly configured self-hosted endpoint should be accepted: {raw}"
            );
        }

        // The local endpoint exception is only for destinations the user added
        // as a source; fixed media and metadata requests keep their old boundary.
        for raw in [
            "http://127.0.0.1/api",
            "http://localhost/api",
            "http://192.168.1.10/api",
            "http://nas.local/api",
        ] {
            for policy in [HttpUrlPolicy::SourceEndpoint, HttpUrlPolicy::MediaResource] {
                assert_eq!(
                    parse_http_url(raw, policy),
                    Err(HttpUrlError::UnsafeHost),
                    "local source exception leaked into {policy:?}: {raw}"
                );
            }
        }
        for raw in [
            "http://localhost/api",
            "http://192.168.1.10/api",
            "http://nas.local/api",
        ] {
            assert_eq!(
                parse_http_url(raw, HttpUrlPolicy::AiProviderEndpoint),
                Err(HttpUrlError::UnsafeHost),
                "local source exception leaked into the AI provider policy: {raw}"
            );
        }
    }

    #[test]
    fn user_provided_source_still_denies_link_local_reserved_and_malformed_targets() {
        for raw in [
            // Port 0 is not a service and an explicitly empty port is ambiguous.
            "https://catalog.example.invalid:0/opds",
            "https://catalog.example.invalid:/opds",
            "https://catalog.example.invalid:65536/opds",
            // Link-local / metadata / reserved ranges.
            "http://100.100.100.200:8096/api",
            "http://169.254.169.254:8096/latest/meta-data",
            "http://198.18.0.1:8096/api",
            "http://192.0.0.9:8096/api",
            "https://[fe80::1]:8096/api",
            "https://[2001:db8::1]:8096/api",
            "https://metadata.google.internal:8096/api",
            // Ambiguous authorities and non-http schemes stay rejected.
            "https://user:secret@catalog.example.invalid:8096/opds",
            "https://catalog.example.invalid:8096/opds#fragment",
            "https://catalog.example.invalid:8096/a b",
            "https:///opds",
            "ftp://catalog.example.invalid:8096/opds",
        ] {
            assert!(
                parse_http_url(raw, HttpUrlPolicy::UserProvidedSource).is_err(),
                "unsafe URL accepted by the user-provided source policy: {raw}"
            );
        }
    }

    #[cfg(feature = "film-tv-fixture")]
    #[test]
    fn user_provided_source_allows_local_service_ports_without_widening_media_fixture() {
        for raw in [
            "http://127.0.0.1:49152/hls/master.m3u8",
            "http://127.0.0.1:80/hls/x",
            "http://127.0.0.1:9200/api",
            "http://127.0.0.1:8096/api",
            "http://127.0.0.1:32400/api",
            "http://10.0.0.1:49152/api",
        ] {
            assert!(
                parse_http_url(raw, HttpUrlPolicy::UserProvidedSource).is_ok(),
                "configured source endpoint should allow its nonzero service port: {raw}"
            );
        }
        for raw in [
            "http://127.0.0.1:9200/api",
            "http://127.0.0.1:8096/api",
            "http://127.0.0.1:32400/api",
            "http://127.0.0.2:49152/api",
            "http://10.0.0.1:49152/api",
        ] {
            assert!(
                parse_http_url(raw, HttpUrlPolicy::MediaResource).is_err(),
                "fixture mode leaked a local-service port: {raw}"
            );
        }
    }

    #[test]
    fn origin_normalization_is_scheme_port_and_case_exact() {
        let origin = |raw: &str| url_origin(&Url::parse(raw).unwrap());
        assert_eq!(
            origin("https://Catalog.Example.Invalid./opds"),
            Some("https://catalog.example.invalid".to_owned())
        );
        assert_eq!(
            origin("https://catalog.example.invalid:443/opds"),
            Some("https://catalog.example.invalid".to_owned())
        );
        assert_eq!(
            origin("http://catalog.example.invalid:80/opds"),
            Some("http://catalog.example.invalid".to_owned())
        );
        assert_eq!(
            origin("http://catalog.example.invalid:8096/opds"),
            Some("http://catalog.example.invalid:8096".to_owned())
        );
        assert_eq!(
            origin("https://catalog.example.invalid:8096/opds"),
            Some("https://catalog.example.invalid:8096".to_owned())
        );
        assert_eq!(
            origin("https://[2001:4860:4860::8888]:8443/a"),
            Some("https://[2001:4860:4860::8888]:8443".to_owned())
        );
        // Scheme and port are part of the identity: a plaintext downgrade or a
        // different service port is a different origin, never "the same host".
        assert_ne!(
            origin("https://catalog.example.invalid/opds"),
            origin("http://catalog.example.invalid/opds")
        );
        assert_ne!(
            origin("http://catalog.example.invalid:8096/opds"),
            origin("http://catalog.example.invalid:8097/opds")
        );
        assert_eq!(origin_of("ftp://catalog.example.invalid/a"), None);
        assert_eq!(origin_of("not a url"), None);
    }
}
