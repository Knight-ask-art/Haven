---
doc_id: engineering.task-workflow
type: guide
status: active
owner: repository-steward
visibility: public
source_of_truth: repository-governance-and-current-git-workflow
last_reviewed: 2026-09-20
review_after: 2026-12-20
---

# Vibe Coding Task Workflow

Vibe coding is welcome at the exploration layer. The repository still needs a
small, explicit control loop before exploration becomes a durable change.

```text
Intent
  -> Snapshot
  -> Risk and scope
  -> Isolated work
  -> Small implementation
  -> Focused verification
  -> Diff review
  -> Commit when authorized
  -> PR and CI
  -> Independent review
  -> Merge
  -> Fresh target verification
  -> Cleanup
```

## The three working modes

| Mode | Use | Required output |
| --- | --- | --- |
| Explore | Search, prototype, understand behavior | Notes, no product claim, no broad edits |
| Implement | A scoped code or doc change | Allowed paths, tests, diff and handoff |
| Integrate | Commit, PR, merge, cleanup or release | Explicit authorization, exact refs and gates |

An agent may move from Explore to Implement only after stating the changed
surface and non-goals. It may move to Integrate only when the user explicitly
authorizes the Git operation.

## Task brief

Every task starts with:

```text
Objective:
Mode: Explore | Implement | Integrate
Execution authority: read-only | draft-only | scoped edits | scoped Git
Context:
Non-goals:
Risk: LOW | MEDIUM | HIGH | CRITICAL
Allowed Paths:
Forbidden Paths:
Expected files/modules:
Acceptance:
Tests:
Evidence:
```

This is the canonical task brief. Agent entrypoints and subagent prompts link to
it instead of maintaining shorter copies.

Do not use `R0-R3` as a second risk vocabulary. Priority, impact, complexity
and risk are separate fields.

## Vibe coding guardrails

- Search before editing; read the surrounding contract before inventing an API.
- Keep an experiment distinguishable from production code and delete or isolate
  throwaway output before integration.
- Stop scope expansion at the first unrelated defect. Record a follow-up instead
  of fixing it silently.
- Add a regression test for a confirmed bug when practical.
- Never weaken an assertion, skip a test or change expected behavior only to make
  generated code pass.
- Report `PASS`, `FAILED`, `NOT RUN` and `NOT PROVEN` separately.

## Completion vocabulary

Use these independent states:

```text
CODE_PRESENT
AUTOMATED_TESTED
INDEPENDENTLY_REVIEWED
DESKTOP_RUNTIME_VERIFIED
RELEASE_VERIFIED
```

“Coding complete” or “tests pass” is never enough to claim a Windows/Tauri
feature is production-complete.
