---
doc_id: agents.claude-code
type: canonical
status: active
owner: repository-steward
visibility: public
source_of_truth: repository-governance-and-agent-delegation
last_reviewed: 2026-09-20
review_after: 2026-12-20
---

# Claude Code Subagent Protocol

Claude Code is a bounded subagent for expensive inspection and well-scoped
implementation. It is not the project owner, release approver or source of
truth.

## Default invocation

The examples in this document use Git Bash. In PowerShell, use `Get-Command
claude` instead of `command -v claude`. Before the first call in a task,
discover the installed binary and version:

```text
command -v claude
claude --version
claude --help
```

For read-only work, use the equivalent of:

```text
claude --print --output-format text --no-session-persistence
  --permission-mode plan --permission-prompts none
  --allowedTools 'Read,Glob,Grep' -- '<canonical task brief>'
```

Do not use `--dangerously-skip-permissions`. Do not put API keys, cookies,
tokens, restricted URLs or user data in the prompt. Do not use `--restricted`
by default when it would hide the repository's configured provider settings.

## Delegation contract

Every prompt uses the canonical [Task Brief](../TASK_WORKFLOW.md#task-brief).
Set `Execution authority` explicitly. “Inspect the project” is not a valid
scope.

## Best use of tokens

Delegate work with high read volume and low decision authority:

- document inventory, headings and link graphs;
- duplicate-fact and stale-path detection;
- test-matrix and contract-consumer audits;
- broad read-only code tracing;
- migration maps and draft checklists;
- independent regression review after an implementation.

Keep small, decision-heavy work in the main thread:

- choosing the canonical source of truth;
- accepting an ADR or changing product scope;
- approving deletion, migration, merge or release;
- deciding whether evidence proves a user-facing capability;
- handling credentials, security incidents and irreversible data changes.

## Context budget

Use a staged prompt:

1. Ask for an inventory and file map.
2. Give the subagent only the relevant paths and findings.
3. Ask for a focused analysis or patch.
4. Request a short evidence table, not a repeated repository summary.

Prefer exact files, symbols and line anchors over “read everything”. Ask for
machine-readable compact output when the result will feed another task. Do not
send prior chat transcripts, full logs or generated bundles when a path and
commit SHA are enough.

## Review and write boundaries

Claude's report, tests or `DONE` claim is not acceptance. The main agent must
inspect the actual diff, run `git diff --check`, run the relevant gates and
verify that only Allowed Paths changed.

If Claude is authorized to edit, use an isolated worktree and a narrow allowlist.
The main agent still owns commit staging, final review and user-facing status.
Claude must not modify secrets, generated IPC output, user dirty files, remote
branches or protected evidence without explicit scope.

`--permission-mode plan` is read-only and must not be reused for an edit task.
After the main agent creates and verifies the isolated worktree, a bounded edit
invocation uses the equivalent of:

```text
claude --print --output-format text --no-session-persistence
  --permission-mode acceptEdits --permission-prompts none
  --allowedTools 'Read,Glob,Grep,Edit,Write' -- '<canonical task brief>'
```

This mode deliberately excludes shell execution. The main agent runs tests and
Git inspection independently. If a task genuinely needs additional tools, name
each one in the task brief rather than broadening the default invocation.
