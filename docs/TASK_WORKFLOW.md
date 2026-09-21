---
doc_id: engineering.task-workflow
type: guide
status: active
owner: maintainers
visibility: public
source_of_truth: repository-contribution-policy
last_reviewed: 2026-10-04
review_after: 2027-01-04
---

# Engineering Change Workflow

A change should be small enough to review, test and integrate without obscuring
its purpose.

```text
Intent → Scope → Implement → Test → Review → PR → CI → Merge
```

## Intent and scope

Describe the user-visible outcome or engineering defect, its affected modules
and non-goals. Inspect the current contracts and working tree before editing.
Coordinate overlapping changes and preserve unrelated work.

## Implement and test

Follow the existing layer boundaries. Keep generated bindings derived from
their canonical definitions and do not create duplicate state owners.
Add regression coverage for changed behavior and run checks proportional to
the affected surface. Do not weaken assertions to accommodate a defect.

## Review

Review the complete diff, error handling, lifecycle, concurrency, privacy and
compatibility. Separate implementation evidence from automated checks,
interactive desktop validation and release-artifact verification.
Document concrete product limitations without publishing raw diagnostics.

## Pull request and integration

Use an atomic commit and a focused PR description: purpose, behavior changes,
completed validation and relevant compatibility notes. Required CI and review
must apply to the current head commit. Resolve findings before merging through
the protected branch workflow.

After merge, verify the target contains the intended change. Retire the
feature branch only after confirming no unmerged work depends on it.

See [Contributing](engineering/contributing.md),
[Testing and Evidence](engineering/testing-and-evidence.md) and
[Git, Worktrees and Commits](engineering/git-worktree-and-commits.md).
