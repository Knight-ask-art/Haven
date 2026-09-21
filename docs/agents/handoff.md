---
doc_id: agents.handoff
type: reference
status: active
owner: repository-steward
visibility: public
source_of_truth: task-workflow
last_reviewed: 2026-09-20
review_after: 2026-12-20
---

# Agent Handoff Template

Use this compact format when stopping, delegating or handing work back:

```text
TASK_HANDOFF

Objective:
Risk:
Repository root:
Branch:
Worktree:
Base SHA:
Current SHA:

Allowed Paths:
Forbidden Paths:
Initial dirty set:
Files changed by this task:

Completed:
Remaining:
Known risks:
Blockers:

Tests and evidence:
  command/action:
  result:
  commit SHA:
  known limits:

Secret/credential status:
Next safe step:
```

An uncommitted handoff must say `Current SHA: working tree` and list the exact
dirty files. A subagent must never report a clean handoff while unrelated dirty
files are present.
