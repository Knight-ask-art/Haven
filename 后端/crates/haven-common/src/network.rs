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

/// Context-specific policy name. Both current contexts share the same safe
/// default port set; keeping the context explicit prevents a future relaxed
/// rule from being accidentally reused by the media proxy.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HttpUrlPolicy {
    SourceEndpoint,
    MediaResource,
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
    let host = validate_host(host)?;
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
    if let Ok(ip) = host.parse::<IpAddr>() {
        if is_publicly_routable(ip) || fixture_loopback_allowed(&host, ip) {
            return Ok(host);
        }
        return Err(HttpUrlError::UnsafeHost);
    }

    // A single-label name is commonly a workstation/container/LAN alias. It
    // cannot be distinguished from an internal resolver target here, so it
    // is rejected until an explicit trusted-source policy exists.
    let labels: Vec<&str> = host.split('.').collect();
    if labels.len() < 2
        || host.len() > 253
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
    if host == "localhost"
        || host.ends_with(".localhost")
        || host.ends_with(".local")
        || host == "metadata.google.internal"
        || host.ends_with(".internal")
    {
        return Err(HttpUrlError::UnsafeHost);
    }
    Ok(host)
}

/// Literal IP policy shared by URL parsing and DNS-pinning callers.
pub fn is_publicly_routable(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(ip) => is_public_ipv4(ip),
        IpAddr::V6(ip) => is_public_ipv6(ip),
    }
}

fn allowed_port(_policy: HttpUrlPolicy, port: u16, host: &str) -> bool {
    // Keep the set deliberately small.  8080/8443 retain common self-hosted
    // media endpoints without allowing arbitrary service ports.
    if matches!(port, 80 | 443 | 8080 | 8443) {
        return true;
    }
    host.parse::<IpAddr>()
        .is_ok_and(|ip| fixture_loopback_allowed(host, ip) && (49152..=65535).contains(&port))
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
}
