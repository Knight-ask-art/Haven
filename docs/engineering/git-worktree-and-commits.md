---
doc_id: engineering.git-worktree
type: canonical
status: active
owner: repository-steward
visibility: public
source_of_truth: repository-governance-and-git-policy
last_reviewed: 2026-09-20
review_after: 2026-12-20
---

# Git, Worktrees and Commits

## Before any change

Run and record:

```text
git rev-parse --show-toplevel
git status --short --branch --untracked-files=all
git diff --check
git diff --stat
git branch --show-current
git worktree list --porcelain
```

The initial dirty set is part of the task evidence. Never clean, reset, restore,
stash or overwrite it to make a task look tidy.

## Isolation

Use one branch and, when isolation is useful, one worktree per independently
reviewable task. A worktree is disposable working space, not a historical
archive. Name it after the task, not after an agent or a vague stage:

```text
codex/docs-system-foundation
codex/fix-reader-progress
codex/verify-release-candidate
```

Before creating a worktree, check path ownership and overlap. If two tasks need
the same core file, coordinate a single owner or split the change into ordered
steps; worktrees do not remove merge conflicts.

## Commit policy

The repository does not grant standing commit or push authority to an agent.
Commit and push only when the user or the current task explicitly authorizes
that Git operation.

When authorized:

1. Review `git status`, the unstaged diff and the staged diff.
2. Stage an explicit allowlist, never `git add .` from a mixed worktree.
3. Run `git diff --cached --check`.
4. Verify the staged files belong to the task and contain no secrets or debug
   output.
5. Create an atomic Conventional Commit with one responsibility.
6. Record the commit SHA, tests and remaining unverified boundaries.

Do not add an AI co-author trailer or claim that a commit exists before reading
the resulting SHA.

## PR and merge cleanup

After a PR is merged, verify the target branch contains the change, the target
CI is green and no newer work depends on the branch. Only then may an explicitly
authorized cleanup remove the local branch, remote feature branch or worktree.

Never delete a dirty worktree, an unmerged branch, a release/maintenance branch,
or a branch with unique commits merely because it is old.

## Forbidden defaults

Do not use `git reset --hard`, `git clean`, force push, force branch deletion,
or worktree removal as a convenience operation. A remote branch deletion is a
separate destructive action and needs its own authorization and evidence.
