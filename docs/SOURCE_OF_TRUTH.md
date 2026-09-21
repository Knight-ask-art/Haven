---
doc_id: project.source-of-truth
type: canonical
status: active
owner: architecture
visibility: public
source_of_truth: accepted-decisions-and-runtime-contracts
last_reviewed: 2026-09-20
review_after: 2026-12-20
---

# Haven Source of Truth

This document defines where a fact belongs. It does not copy every runtime
field, default or task. One fact has one authoritative home; other documents
link to it.

## Product boundary

Haven is a Windows-first, local-first desktop client for organizing and
consuming user-controlled digital content. It unifies Works, Editions,
MediaItems, resources, progress, history, favorites, markers, downloads and
controlled source access without creating a central Haven account.

The product must distinguish metadata candidates from consumable content. A
provider result is not readable or playable until the controlled resource path
proves that capability.

Current product boundaries and unsupported capabilities remain described by the
runtime contracts and the existing AI/design plans. A plan alone never upgrades
an unimplemented feature to `implemented`.

## Authority matrix

There is no unsafe single global ranking for every type of fact. Use the row for
the fact being evaluated:

| Fact | Authority | What a document may do |
| --- | --- | --- |
| Security and privacy constraint | Security policy and accepted ADR | Explain and link; never weaken |
| Actual runtime behavior | Domain/Application/Infrastructure, schema and tests | Describe observed behavior and evidence |
| IPC contract | Rust Wire DTO, command manifest, capability and generated binding | Explain the generated contract; never hand-edit generated output |
| Product capability status | Code + Registry + binding + consumer + test/runtime evidence | Report `implemented`, `partial` or `planned` honestly |
| Architecture rationale | Accepted ADR | Explain the decision and rejected alternatives |
| Engineering workflow and agent delegation | Public engineering/agent governance, with stricter repository-local operator rules when present | Define repeatable workflow without publishing machine-specific instructions |
| Task state | One active status ledger plus linked PR/CI | Track execution; never prove runtime capability |
| Visual design | Design system and Figma | Define presentation, not backend authority |
| CI/release state | Actual workflow run, tag, artifact and signature | Record exact SHA and result |
| Historical review | Dated, commit-bound review evidence | Preserve context; never overwrite current truth |

When sources conflict, first identify the fact type, then use the authority in
this table. Security and explicit user instructions can stop an operation but
do not turn a design proposal into an implementation.

## Product and engineering agents

Engineering-agent governance is documented under [agents](agents/). The Haven
Copilot is a product capability and is documented separately under [product](product/).
An engineering agent may edit code under a task scope; the product Copilot
cannot inherit those permissions.

The product Copilot remains context-bound: the model may propose intent but does
not own Haven permissions. Reads use a bounded context and fixed application
services. Writes require a typed proposal, explicit user approval, CAS and a
receipt. An unresolved locator degrades to selected/pasted text or metadata and
must not silently upload an entire work.

## Current transition facts

- The current checkout has an existing `AGENTS.md`, Git governance, Registry and
  AI runtime material. This documentation system extends them; it does not
  silently replace them.
- The active ledger path, cutover state and freshness are declared only by
  [Work and Status](work/README.md).
- `docs/reviews/` remains historical evidence even when its findings are
  resolved. Resolved findings may be linked from a new canonical document, but
  the original evidence is not rewritten to change history.
