---
doc_id: work.plan-archive-index
type: historical
status: active
owner: repository-steward
visibility: public
source_of_truth: plan-lifecycle-index
last_reviewed: 2026-09-20
review_after: 2026-12-20
---

# Plan Archive Index

This index records the lifecycle of dated plans without publishing their raw
working notes. A plan describes intended work against a named baseline. It is
not proof that the work exists, passed tests or shipped.

Raw plans in this directory are internal. Only this index belongs in the public
snapshot.

## Lifecycle rules

- `active`: executable against its stated baseline and owned by a current task.
- `completed`: its acceptance was verified and linked to immutable evidence.
- `blocked`: work cannot continue without a named dependency or decision.
- `superseded`: its baseline or approach is stale; create a new plan before work.
- `archived`: retained only for historical context.

A checkbox is not completion evidence. Closing a plan requires a commit or
artifact identity, the relevant gates and an explicit record of runtime limits.

## Inventory

| Record ID | Internal record | Owner | Baseline | Lifecycle | Durable destination | Links checked | Disposition |
| --- | --- | --- | --- | --- | --- | --- | --- |
| `plan.release-readiness.v1.0.0.2026-09-15` | `docs/plans/2026-09-15-v1.0.0-release-readiness-plan.md` | release-owner | `origin/main@7d0758e` | superseded | [Release and Rollback](../operations/release-and-rollback.md) | internal-only, sampled 2026-09-20 | The release invariants remain useful, but the executable checklist must be re-baselined before any release-candidate work. No completion claim is carried forward. |
| `plan.agent-approval-token.2026-09-17` | `docs/plans/2026-09-17-agent-approval-token-plan.md` | agent-platform | `origin/main@2d9c25b` | archived | [Documentation System](../engineering/documentation-system.md) | internal-only, sampled 2026-09-20 | Historical implementation plan retained in Git history; its contract claims require a fresh baseline and are not public capability evidence. |
| `plan.agent-typed-ipc-ui.2026-09-17` | `docs/plans/2026-09-17-agent-typed-ipc-ui-plan.md` | agent-platform | `origin/main@2d9c25b` | archived | [Documentation System](../engineering/documentation-system.md) | internal-only, sampled 2026-09-20 | Historical implementation plan retained in Git history; its interface claims require a fresh baseline and are not public capability evidence. |

## Adding or closing a plan

1. Give the plan a baseline SHA, owner, scope, non-goals and acceptance.
2. Link durable architecture or policy instead of copying it into the plan.
3. Update this index when the lifecycle changes.
4. Promote accepted long-lived decisions to `docs/decisions/` or the relevant
   canonical area.
5. Keep raw logs, machine paths, credentials and user data out of GitHub.

The live implementation ledger remains the one named in
[Work and Status](../work/README.md); this archive is not a second status board.
