---
doc_id: decisions.controlled-source-artwork
type: decision
status: accepted
owner: architecture
visibility: public
source_of_truth: artwork-registration-and-outbound-policy
last_reviewed: 2026-10-08
review_after: 2027-01-08
---

# Controlled source artwork

Status: accepted

Date: 2026-10-08

Owner: architecture

## Context

Search providers may return a poster in the same response as metadata. Discarding
that field leaves a result with a generic cover even when the upstream provides
an image. Some user-configured catalogs return public CDN URLs with expiry or
signature parameters. Treating every such parameter as invalid prevents their
images from loading, while exposing raw addresses to the frontend would bypass
the controlled resource boundary.

## Decision

Provider adapters validate image fields and register them through
`ImageProxyRepository`. Search cards expose only `haven://artwork/<opaque>`;
`ArtworkCache` owns lazy fetching, format validation and local variants. Search
does not issue a separate image request for every result. Missing or rejected
images preserve the card and its existing category-cover fallback.

Fixed metadata providers validate against their own image-host allowlists and
continue to reject sensitive query parameters. Signed artwork is allowed only
for the explicitly supported CMS/OPDS/user OPDS/Feed registration paths, never
for an unknown or newly added source merely because it calls `register`.
Registration stores the source identity and normalized host. It grants neither
local-network access nor permission to follow a redirect to a different host.

At each fetch hop the resource owner validates URL shape and the registered host,
then produces an opaque checked target containing the exact resolved socket set.
Only that target builds the content request. The DNS override uses the request
URL's actual hostname, including its terminal-dot form, and content clients disable
environment HTTP proxies. Redirects require a fresh checked target. DNS lookup,
connection, request, body size and concurrency each retain bounded limits.

Public-address checks share the common outbound predicate. Literal Fake-IP
addresses are denied. An all-Fake-IP system DNS answer set requires a bounded
public DoH proof; the request still pins the original system addresses so the local
TUN mapping can work. Mixed public/Fake-IP or unsafe answers are denied. The local
TUN is a trusted routing boundary: the DoH proof does not prove the final destination
chosen by that mapping, and the application does not grant general private-network
access. The proof client retains its existing proxy behavior.

Legacy host-based inference does not acquire signed-query rights. Original URLs
and query values remain in backend storage and outbound requests; they are not
returned in IPC, errors or diagnostic messages. Checked-target diagnostic output
contains only the type and address count, not the URL, host, socket or source identity.

## Alternatives considered

- Frontend remote-image URLs: bypass controlled resource authorization and expose
  signed query values to the WebView.
- Reject every signed URL: preserves the previous policy but needlessly breaks
  otherwise compatible public CDN images.
- Permit signatures for every registered source: makes future provider additions
  silently inherit a permission their adapter did not explicitly establish.
- Validate DNS and then use a separately resolved client: permits a second lookup
  to select an unchecked destination.
- Replace Fake-IP with public DoH addresses: bypasses the local TUN mapping the
  user expects and treats DNS proof as a transport configuration.

## Consequences

Image availability remains independent of metadata search success. Cancellation
stops further search-image registration. Existing opaque image identities remain
stable and are not deleted on cancellation or cache clearing because saved
library records may refer to them. Fixed providers and user-configured sources
retain distinct authority boundaries.

Content clients are built per checked hop rather than pooled across image hosts.
This trades cross-request connection reuse for a direct correspondence between
validated addresses and the request. Environment HTTP proxies are not used for
image content; system TUN routing remains available under the boundary above.

## Migration and rollback

No new image table or wire field is required. Existing source/host mapping fields
remain authoritative. Unknown legacy rows keep their previous fail-closed
behavior. Removing a signed-source policy entry disables future fetches of its
signed images without changing saved opaque identities or other source records.
Existing local or malformed mappings remain in storage but are denied at the
request boundary; this decision neither migrates them into trusted sources nor
deletes saved opaque identities.

## Baseline sync

The product's existing frontend-to-IPC and controlled-resource architecture is
unchanged. The registration authority is documented in the Domain port; the
resource boundary is linked from `docs/SOURCE_OF_TRUTH.md`.

## Evidence

- `haven-infrastructure/src/metadata_sources/artwork.rs`: provider field, host,
  path and opaque-URI checks.
- `haven-infrastructure/src/metadata_sources.rs`: injection, registration,
  cancellation and wire-projection regression tests.
- `haven-infrastructure/src/db/repos/image_proxy.rs`: explicit signed-source
  policies, length limits and legacy behavior tests.
- `haven-infrastructure/src/artwork_network.rs`: checked-target construction,
  exact-host DNS override, shared address guards and diagnostic redaction tests.
- `haven-infrastructure/src/artwork_network/transport_tests.rs`: real local TCP
  request/response assertions for bare and terminal-dot override keys.
- `SourceCandidates.artwork.test.tsx`: controlled artwork rendering and fallback.
- `opds/catalog_tests.rs` and `cms10.rs`: search-participant responses project
  registered artwork without per-result image requests.
