---
doc_id: decisions.index
type: canonical
status: active
owner: architecture
visibility: public
source_of_truth: accepted-ADR-set
last_reviewed: 2026-09-20
review_after: 2026-12-20
---

# Architecture Decisions

Use an ADR for a decision that changes a long-lived invariant, boundary,
persistent format, security rule, release policy or public contract.

Each ADR contains:

```text
Status: proposed | accepted | superseded | rejected
Date:
Owner:
Context:
Decision:
Alternatives considered:
Consequences:
Migration/rollback:
Evidence:
```

An ADR is not a task plan and a task plan is not an ADR. Proposed decisions do
not authorize implementation until the specification review accepts them.

Revalidate an existing decision before promoting it to a public ADR. Preserve
its rationale without publishing private record names or operational details;
do not invent a second conflicting authority for the same invariant.
