---
doc_id: engineering.contributing
type: guide
status: active
owner: maintainers
visibility: public
source_of_truth: repository-contribution-policy
last_reviewed: 2026-10-04
review_after: 2027-01-04
---

# Contributor Responsibilities

Responsibilities describe review and integration ownership, not special runtime
permissions.

| Role | Responsibility |
| --- | --- |
| Contributor | Understand the contract, implement a scoped change and provide regression coverage |
| Reviewer | Inspect correctness, compatibility, lifecycle, privacy and evidence |
| Maintainer | Resolve design questions, enforce protected-branch gates and integrate approved changes |
| Release maintainer | Verify version, artifacts, signature and upgrade/recovery behavior |

High-risk changes benefit from review independent of their author. Compilation
or a partial test suite cannot close a confirmed concurrency, transaction or
security defect.

## Before changing code

Read the relevant runtime contracts and consumers. Preserve existing work,
coordinate overlapping changes and avoid unrelated rewrites. State product
behavior and non-goals in the issue or PR.

Production dependencies follow React → feature API/gateway → typed HavenClient
→ Tauri → Application → Domain/Port → Infrastructure. Components do not directly
invoke backend commands, providers or storage.

## Pull request expectations

- Keep the diff focused and stage the intended files explicitly.
- Explain behavior changes, migration and compatibility impact.
- Include completed validation tied to the current revision.
- Protect credentials, user data and unpublished operational records.
- Use generated bindings rather than manually repairing contract drift.
- Wait for the required checks and review before protected-branch integration.

See [Engineering Change Workflow](../TASK_WORKFLOW.md) and
[Release and Rollback](../operations/release-and-rollback.md).
