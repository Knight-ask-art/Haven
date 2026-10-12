//! Infrastructure-only helpers for bounded outbound HTTP requests.
//!
//! URL syntax and literal-address policy live in `haven-common`.  This module
//! owns the I/O half of that boundary: resolve a hostname once for one
//! request, reject unsafe answers, and pin the resulting addresses in reqwest.
//! Public addresses and the reserved fake-IP range used by local DNS proxies are
//! accepted for hostnames; callers must repeat the operation after every
//! manually followed redirect.

use std::net::{IpAddr, SocketAddr};

use haven_common::network::{
    HttpUrlPolicy, fixture_loopback_allowed, is_local_proxy_fake_ip, is_publicly_routable,
    is_user_source_local_ip_allowed, parse_http_url,
    ai_provider_loopback_allowed,
};
use reqwest::{ClientBuilder, Url};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum HttpTargetError {
    Invalid,
    ResolveFailed,
    UnsafeAddress,
}

#[derive(Debug, Clone)]
pub(crate) struct ResolvedHttpTarget {
    pub(crate) url: Url,
    /// The exact DNS name used by the URL, when the target is a domain.  This
    /// keeps TLS SNI/Host semantics while `resolve_to_addrs` pins the socket
    /// destinations.
    pub(crate) dns_name: Option<String>,
    pub(crate) addresses: Vec<SocketAddr>,
}

/// Parse one URL and resolve its host.  Every call performs a fresh lookup;
/// redirect callers therefore get DNS-rebinding protection at each hop.
pub(crate) async fn resolve_public_http_target(
    raw: &str,
    policy: HttpUrlPolicy,
) -> Result<ResolvedHttpTarget, HttpTargetError> {
    let safe = parse_http_url(raw, policy).map_err(|_| HttpTargetError::Invalid)?;
    let port = safe
        .as_url()
        .port_or_known_default()
        .ok_or(HttpTargetError::Invalid)?;
    let host = safe.host().to_owned();
    let dns_name = host
        .parse::<IpAddr>()
        .is_err()
        .then(|| safe.as_url().host_str().unwrap_or(safe.host()).to_owned());
    let addresses = if let Ok(ip) = host.parse::<IpAddr>() {
        vec![SocketAddr::new(ip, port)]
    } else {
        resolve_public_addresses(&host, port, policy).await?
    };
    if addresses.is_empty() {
        return Err(HttpTargetError::ResolveFailed);
    }
    if addresses
        .iter()
        .any(|address| !is_allowed_dns_address(&host, address.ip(), policy))
    {
        return Err(HttpTargetError::UnsafeAddress);
    }
    Ok(ResolvedHttpTarget {
        url: safe.into_url(),
        dns_name,
        addresses,
    })
}

/// Resolve via the platform resolver in async I/O, then validate every returned
/// address under the request policy. Rejecting a mixed safe/unsafe answer set is
/// intentional: using only the first public answer would make resolver ordering
/// part of the security decision.
pub(crate) async fn resolve_public_addresses(
    host: &str,
    port: u16,
    policy: HttpUrlPolicy,
) -> Result<Vec<SocketAddr>, HttpTargetError> {
    let addresses = tokio::net::lookup_host((host, port))
        .await
        .map_err(|_| HttpTargetError::ResolveFailed)?
        .collect::<Vec<_>>();
    validate_resolved_addresses(host, port, policy, addresses)
}

/// Sort and validate resolver output. Kept separate from the platform lookup so
/// empty-answer behavior can be tested without depending on local DNS/proxy rules.
fn validate_resolved_addresses(
    host: &str,
    _port: u16,
    policy: HttpUrlPolicy,
    mut addresses: Vec<SocketAddr>,
) -> Result<Vec<SocketAddr>, HttpTargetError> {
    addresses.sort_unstable();
    addresses.dedup();
    if addresses.is_empty() {
        return Err(HttpTargetError::ResolveFailed);
    }
    if addresses
        .iter()
        .any(|address| !is_allowed_dns_address(host, address.ip(), policy))
    {
        return Err(HttpTargetError::UnsafeAddress);
    }
    Ok(addresses)
}

/// Allow the recognized IPv4/IPv6 local-proxy Fake-IP prefixes only when they
/// were returned for a hostname. Local DNS proxies map public domains into
/// these ranges and route those connections through the user's configured
/// network stack. Fake-IP literals remain rejected. Only canonical AI loopback
/// literals are allowed by the AI policy; private LAN/loopback answers are
/// accepted only for explicitly configured sources.
fn is_allowed_dns_address(host: &str, ip: IpAddr, policy: HttpUrlPolicy) -> bool {
    is_publicly_routable(ip)
        || fixture_loopback_allowed(host, ip)
        || (policy == HttpUrlPolicy::AiProviderEndpoint && ai_provider_loopback_allowed(host, ip))
        || (host.parse::<IpAddr>().is_err() && is_local_proxy_fake_ip(ip))
        || (policy == HttpUrlPolicy::UserProvidedSource && is_user_source_local_ip_allowed(ip))
}

/// Apply the transport controls that make `target.addresses` authoritative.
/// Redirects must already be disabled on the builder supplied by the caller.
pub(crate) fn pin_client_builder(
    builder: ClientBuilder,
    target: &ResolvedHttpTarget,
) -> ClientBuilder {
    let builder = builder.no_proxy();
    if let Some(dns_name) = target.dns_name.as_deref() {
        builder.resolve_to_addrs(dns_name, &target.addresses)
    } else {
        builder
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn ai_literal_loopback_is_resolved_without_dns_or_proxy_bypass() {
        for (raw, expected) in [
            ("http://127.0.0.1:3123/v1", "127.0.0.1:3123"),
            ("http://[::1]:11434/v1", "[::1]:11434"),
        ] {
            let target = resolve_public_http_target(raw, HttpUrlPolicy::AiProviderEndpoint)
                .await
                .unwrap();
            assert_eq!(target.dns_name, None);
            assert_eq!(
                target.addresses,
                vec![expected.parse::<SocketAddr>().unwrap()]
            );
            assert!(
                pin_client_builder(reqwest::Client::builder(), &target)
                    .build()
                    .is_ok()
            );
        }
        for raw in [
            "http://localhost:3123/v1",
            "http://127.0.0.2:3123/v1",
            "http://192.168.1.10:3123/v1",
        ] {
            assert!(
                resolve_public_http_target(raw, HttpUrlPolicy::AiProviderEndpoint)
                    .await
                    .is_err()
            );
        }
        assert!(
            resolve_public_http_target("http://[::1]:3123/v1", HttpUrlPolicy::MediaResource)
                .await
                .is_err()
        );
    }

    #[tokio::test]
    async fn loopback_resolution_is_rejected_even_when_the_host_resolves() {
        assert_eq!(
            resolve_public_addresses("localhost", 80, HttpUrlPolicy::SourceEndpoint).await,
            Err(HttpTargetError::UnsafeAddress)
        );
    }

    #[test]
    fn empty_resolution_is_not_treated_as_public() {
        assert_eq!(
            validate_resolved_addresses(
                "does-not-exist.invalid",
                443,
                HttpUrlPolicy::MediaResource,
                Vec::new(),
            ),
            Err(HttpTargetError::ResolveFailed)
        );
    }

    #[test]
    fn fake_ip_answers_are_allowed_for_hostnames_without_widening_fixed_policies() {
        let fake_ip: IpAddr = "198.18.2.168".parse().unwrap();
        let upper_fake_ip: IpAddr = "198.19.255.254".parse().unwrap();
        let fake_ipv6: IpAddr = "fdfe:dcba:9876::ee".parse().unwrap();
        let private_ip: IpAddr = "192.168.1.10".parse().unwrap();

        for policy in [
            HttpUrlPolicy::MediaResource,
            HttpUrlPolicy::SourceEndpoint,
            HttpUrlPolicy::AiProviderEndpoint,
            HttpUrlPolicy::UserProvidedSource,
        ] {
            assert!(
                is_allowed_dns_address("api.example.com", fake_ip, policy),
                "hostname fake-IP answer should be allowed for {policy:?}"
            );
            assert!(
                is_allowed_dns_address("api.example.com", upper_fake_ip, policy),
                "the upper RFC 2544 range should be allowed for {policy:?}"
            );
            assert!(
                is_allowed_dns_address("api.example.com", fake_ipv6, policy),
                "the configured local-proxy IPv6 prefix should be allowed for {policy:?}"
            );
            assert!(
                !is_allowed_dns_address("198.18.2.168", fake_ip, policy),
                "literal fake-IP address must stay rejected for {policy:?}"
            );
            assert!(
                !is_allowed_dns_address("198.19.255.254", upper_fake_ip, policy),
                "upper-range literal fake-IP must stay rejected for {policy:?}"
            );
            assert!(
                !is_allowed_dns_address("fdfe:dcba:9876::ee", fake_ipv6, policy),
                "IPv6 fake-IP literals must remain blocked for {policy:?}"
            );
        }
        assert!(!is_allowed_dns_address(
            "api.example.com",
            private_ip,
            HttpUrlPolicy::SourceEndpoint
        ));
        assert!(is_allowed_dns_address(
            "nas.local",
            private_ip,
            HttpUrlPolicy::UserProvidedSource
        ));
        assert!(!is_allowed_dns_address(
            "nas.local",
            private_ip,
            HttpUrlPolicy::SourceEndpoint
        ));
        assert!(!is_allowed_dns_address(
            "nas.local",
            "169.254.1.1".parse().unwrap(),
            HttpUrlPolicy::UserProvidedSource
        ));
    }
}
