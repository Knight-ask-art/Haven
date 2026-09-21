---
doc_id: engineering.architecture-change
type: guide
status: active
owner: architecture
visibility: public
source_of_truth: architecture-governance
last_reviewed: 2026-09-20
review_after: 2026-12-20
---

# Architecture Change Protocol

Use this protocol when a change crosses feature boundaries, changes a
persistent format, adds IPC, changes permissions, modifies the resource
protocol, changes a Registry, or changes a release/CI contract.

1. State the current behavior and the invariant that must remain true.
2. Identify the source of truth and the affected layers.
3. Write or update an ADR if the decision is long-lived.
4. Freeze the typed contract before writing UI wiring.
5. Define failure, cancellation, retry, race and rollback paths.
6. Implement through the existing layer chain.
7. Add tests at the boundary where the invariant is enforced.
8. Run an independent quality review after the specification review.

The frontend must not compensate for a missing backend invariant by guessing
IDs, paths, media types, capabilities or permissions. If the allowed scope is
too small to repair the actual invariant, report `NEEDS_CONTEXT` instead of
papering over the defect with retries, local locks or fake state.
