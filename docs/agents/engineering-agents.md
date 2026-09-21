---
doc_id: agents.engineering-roles
type: guide
status: active
owner: repository-steward
visibility: public
source_of_truth: task-scope-and-layering
last_reviewed: 2026-09-20
review_after: 2026-12-20
---

# Engineering Agent Roles

Roles are responsibilities, not automatic permissions.

| Role | Owns | Does not decide alone |
| --- | --- | --- |
| Specification Owner | Goal, contract, non-goals and acceptance | Runtime completion |
| Feature Owner | Scoped implementation and focused tests | Merge or release |
| Architecture Reviewer | Layering, invariants, ADR and boundaries | User approval of destructive actions |
| Quality Reviewer | Regression, race, error and evidence review | Claiming desktop acceptance without running it |
| Integration Owner | Staging, conflict resolution, PR and target validation | Hiding unrelated dirty work |
| Repository Steward | Worktree, docs, branch and GitHub hygiene | Rewriting history without authorization |
| Release Owner | Candidate, signing, release and rollback | Declaring an unverified artifact safe |

For high-risk work, the author, quality reviewer and verifier should be
different people or independent runs. A feature owner cannot mark an unresolved
quality finding as closed merely because the code compiles.

## Ownership artifact

Before parallel work, record:

```text
task_id
owner
branch/worktree
allowed_paths
forbidden_paths
overlap_with_active_tasks
reviewer
verifier
```

If overlap is discovered, stop the affected write and coordinate scope. Do not
solve ownership conflict by silently editing the other task's files.
