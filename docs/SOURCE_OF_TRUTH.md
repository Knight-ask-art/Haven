---
doc_id: project.source-of-truth
type: canonical
status: active
owner: architecture
visibility: public
source_of_truth: accepted-decisions-and-runtime-contracts
last_reviewed: 2026-10-14
review_after: 2027-01-08
---

# Haven Source of Truth

One fact has one authoritative home. Other documents explain and link to it,
rather than maintaining competing defaults, contracts or capability claims.

## Product boundary

Haven is a Windows-first, local-first desktop client for organizing and
consuming user-controlled digital content. It unifies Works, Editions,
MediaItems, resources, progress, history, favorites, markers, downloads and
controlled source access without creating a central Haven account.

Metadata candidates are not consumable content. A provider result becomes
readable or playable only through the controlled resource path.

Search artwork follows the same resource boundary: adapters register validated
images in the backend and cards carry opaque artwork identities. The
[artwork decision](decisions/ADR-0001-controlled-source-artwork.md) records the
distinction between fixed-provider images and signed user-source images. Image
content requests use checked per-hop addresses and bypass environment HTTP proxies;
local TUN mapping remains a separate trusted routing boundary.

Source credentials have versioned, source-scoped identities and durable cleanup
owned by `SourceRegistryService`. The [source credential decision](decisions/ADR-0002-versioned-source-credentials.md)
defines the source-only revocation and recovery boundary; it does not change AI
or cloud credential-family semantics. OPDS consumers use one coherent source
snapshot, bind credentials to the registered HTTPS origin and recheck that
snapshot after credential resolution before sending.

Local storage locations and cloud drives are separate boundaries. A local
location is a directory on the user's machine; a cloud drive binds a
user-authorized account and one chosen folder, and reading a cloud document pins
a fixed snapshot for that session rather than following a later re-authorization
or a rewritten remote object. The first implemented cloud provider is Google
Drive, read-only PDF only: Haven does not upload, delete remote files, sync or
back up. The OAuth client configuration is a deployment prerequisite, not a
Haven account. Durable decisions for this boundary are recorded internally but
are not yet published as a public ADR; until one exists, the runtime contracts,
migrations, Registry and consumers remain the capability authority.

User-configured source endpoints retain their configured scheme and may target
private IPv4, loopback, IPv6 ULA, shared VPN addresses, and local DNS names on
any nonzero service port. Link-local, unspecified, multicast, cloud metadata,
and other reserved destinations remain denied; literal Fake-IP addresses also
remain denied. Recognized local DNS Fake-IP answers are accepted for hostnames
and the resolved addresses stay pinned for that request. Fixed metadata, AI,
and media-resource requests keep their own policies and do not inherit the
private-address exception. Type-specific HTTPS requirements remain, and source
credentials stay HTTPS-only and bound to the registered origin.

## Authority matrix

| Fact | Authority | Documentation responsibility |
| --- | --- | --- |
| Security and privacy | Security policy and accepted ADR | Explain constraints; never weaken them |
| Runtime behavior | Domain, Application, Infrastructure, schema and tests | Describe behavior within the inspected scope |
| IPC | Rust Wire DTO, command manifest, capability and generated binding | Explain; never hand-edit generated output |
| Product capability | Code, Registry, binding, consumer and appropriate evidence | Distinguish implemented, partial and planned |
| Architecture rationale | Accepted ADR | Preserve decision and alternatives |
| Contribution process | Maintainer policy and public engineering guidance | Define portable, tool-neutral practices |
| Public work status | Issue, PR and current CI run | Describe the scoped change, not runtime proof |
| Visual design | Design system and Figma | Define presentation, not backend authority |
| Release state | Tag, artifact, checksums and signature | Identify the delivered version and verification |
| Historical evidence | Original commit and date | Retain context; do not present it as current truth |

When sources conflict, identify the fact type first. A plan, screenshot or
successful build does not upgrade a planned capability to implemented.

## Product AI authority

Haven's product AI — user-visible name 「栖伴」, settings section 「AI 工作台」
(设置 → 智能) — reads bounded, redacted context through fixed application
services. It can propose intent but does not own Haven permissions. Changes
require a typed proposal, explicit user approval, revision/CAS validation and
a receipt. Skills and external MCP clients cannot grant themselves authority.

An unresolved locator permits selected/pasted text or metadata, not silent
upload of an entire work. The technical boundaries are maintained in
[AI System](architecture/AI_SYSTEM.md) and
[MCP Transport](architecture/MCP_EXTERNAL_AGENT_TRANSPORT.md).

Private development records are not public contracts. Durable conclusions may
be published after revalidation; private filenames and operating instructions
must not be reproduced.
