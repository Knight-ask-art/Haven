# Haven Claude Code entrypoint

When present in the checkout, Claude Code must read `AGENTS.md` and the
relevant documents under `docs/` before acting. `AGENTS.md` contains
repository-local safety rules;
`docs/agents/claude-code.md` contains the Claude Code delegation contract.

This file is intentionally thin. It is an entrypoint, not a second source of
truth. When it conflicts with `AGENTS.md`, the repository rules and the user's
explicit instruction win.

Every task uses the canonical brief in
`docs/TASK_WORKFLOW.md#task-brief` and the execution rules in
`docs/agents/claude-code.md`.

Read-only inspection is the default. Do not edit, commit, push, merge, delete,
or clean anything unless the current task explicitly authorizes that operation.
