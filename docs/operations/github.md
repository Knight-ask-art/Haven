---
doc_id: operations.github
type: canonical
status: active
owner: repository-steward
visibility: public
source_of_truth: GitHub-settings-and-workflows
last_reviewed: 2026-09-20
review_after: 2026-12-20
---

# GitHub Workflow

## Branch and PR lifecycle

```text
Issue or task brief
  -> isolated branch/worktree
  -> focused commits
  -> push only when authorized
  -> Draft PR for early contract discussion
  -> CI and independent review
  -> Ready for merge
  -> merge using repository policy
  -> target-branch verification
  -> branch/worktree cleanup
```

The remote PR head, target branch SHA, CI conclusion and merge state must be
re-read after every remote operation. A local commit or successful push is not
proof that a PR was updated.

## PR description

Every non-trivial PR states:

```text
What
Why
Scope
Not included
Risk
Validation by exact SHA
Runtime evidence and limits
Rollback or recovery
```

Docs-only changes still run public snapshot, link, metadata and secret checks.
Code changes run the affected layer gates. Generated-contract and workflow
changes are treated as full-surface changes.

## CI truth

Required checks must be real, stable and owned. A skipped component job is not a
pass for an affected path. The aggregate check must explain why a job was
skipped. A failing main branch freezes normal merge until the offending change
is fixed or reverted through the repository's allowed PR path.

## GitHub state boundaries

- Issues and Projects track collaboration; they do not prove implementation.
- PR review comments are not durable architecture decisions; accepted decisions
  belong in `docs/decisions/`.
- Actions logs are evidence for a SHA, not a permanent “current green” claim.
- Releases, tags, artifacts and signatures are separate from source merge.
- Do not delete remote branches or close old PRs solely because they are old.
