//! Infrastructure-only helpers for bounded outbound HTTP requests.
//!
//! URL syntax and literal-address policy live in `haven-common`.  This module
//! owns the I/O half of that boundary: resolve a hostname once for one
//! request, reject any non-public answer, and pin the resulting addresses in
//! reqwest.  Callers must repeat the operation after every manually followed
//! redirect.

use std::net::{IpAddr, SocketAddr};

use haven_common::network::{
    HttpUrlPolicy, fixture_loopback_allowed, is_publicly_routable, parse_http_url,
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
        resolve_public_addresses(&host, port).await?
    };
    if addresses.is_empty() {
        return Err(HttpTargetError::ResolveFailed);
    }
    if addresses.iter().any(|address| {
        !is_publicly_routable(address.ip()) && !fixture_loopback_allowed(&host, address.ip())
    }) {
        return Err(HttpTargetError::UnsafeAddress);
    }
    Ok(ResolvedHttpTarget {
        url: safe.into_url(),
        dns_name,
        addresses,
    })
}

/// Resolve via the platform resolver in async I/O, then fail closed if any
/// returned address is not publicly routable.  Rejecting the whole answer set
/// is intentional: using only the first public answer would make resolver
/// ordering part of the security decision.
pub(crate) async fn resolve_public_addresses(
    host: &str,
    port: u16,
) -> Result<Vec<SocketAddr>, HttpTargetError> {
    let mut addresses = tokio::net::lookup_host((host, port))
        .await
        .map_err(|_| HttpTargetError::ResolveFailed)?
        .collect::<Vec<_>>();
    addresses.sort_unstable();
    addresses.dedup();
    if addresses.is_empty() {
        return Err(HttpTargetError::ResolveFailed);
    }
    if addresses.iter().any(|address| {
        !is_publicly_routable(address.ip()) && !fixture_loopback_allowed(host, address.ip())
    }) {
        return Err(HttpTargetError::UnsafeAddress);
    }
    Ok(addresses)
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
    async fn loopback_resolution_is_rejected_even_when_the_host_resolves() {
        assert_eq!(
            resolve_public_addresses("localhost", 80).await,
            Err(HttpTargetError::UnsafeAddress)
        );
    }

    #[tokio::test]
    async fn missing_domain_resolution_is_not_treated_as_public() {
        assert!(
            resolve_public_http_target(
                "https://does-not-exist.invalid/video.m3u8",
                HttpUrlPolicy::MediaResource,
            )
            .await
            .is_err()
        );
    }
}
