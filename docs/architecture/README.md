---
doc_id: architecture.index
type: canonical
status: active
owner: architecture
visibility: public
source_of_truth: runtime-contracts-and-accepted-decisions
last_reviewed: 2026-09-20
review_after: 2026-12-20
---

# Architecture

Architecture documents describe stable boundaries and invariants. They do not
replace the Rust contracts, generated IPC bindings, schema or tests that prove
the current implementation.

## Start here

- [Architecture Roadmap](../ARCHITECTURE_ROADMAP.md) defines the layer map and
  documentation migration stages.
- [Architecture Change Protocol](../engineering/architecture-change.md)
  defines the review sequence for cross-layer changes.
- [Source of Truth](../SOURCE_OF_TRUTH.md) defines which artifact owns each kind
  of fact.
- [Comic Content Identity and Continuity](comic-content-continuity.md) preserves
  the accepted Work, Edition, chapter identity and progress-migration
  invariants extracted from the legacy execution specification.

Changes that cross React, IPC, Application, Domain or Infrastructure boundaries
must preserve the typed contract and state their failure, cancellation, retry,
race and rollback behavior.
