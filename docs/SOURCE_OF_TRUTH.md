---
doc_id: project.source-of-truth
type: canonical
status: active
owner: architecture
visibility: public
source_of_truth: accepted-decisions-and-runtime-contracts
last_reviewed: 2026-10-04
review_after: 2027-01-04
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

Haven's product AI reads bounded, redacted context through fixed application
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
