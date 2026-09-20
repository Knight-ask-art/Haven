---
doc_id: docs.index
type: canonical
status: active
owner: repository-steward
visibility: public
source_of_truth: documentation-system
last_reviewed: 2026-09-20
review_after: 2026-12-20
---

# Haven Documentation

This is the public documentation entrypoint for Haven (栖阅). The directory
uses semantic names rather than numeric ordering. `docs/README.md` is the
navigation map; it is not a second product specification.

## Start here

1. [Source of Truth](SOURCE_OF_TRUTH.md): stable product scope, boundaries and
   authority rules.
2. [Architecture Roadmap](ARCHITECTURE_ROADMAP.md): the architecture map and
   staged documentation migration.
3. [Task Workflow](TASK_WORKFLOW.md): the Vibe Coding loop from intent to
   verified merge.
4. [Documentation System](engineering/documentation-system.md): document
   ownership, lifecycle, visibility and freshness.
5. [Git and Worktrees](engineering/git-worktree-and-commits.md): safe isolation,
   commit discipline and cleanup.
6. [Testing and Evidence](engineering/testing-and-evidence.md): what each kind
   of validation can and cannot prove.
7. [Documentation Migration](engineering/documentation-migration.md): the
   inventory and treatment of legacy plans, reviews and internal material.
8. [Plan Archive Index](plans/README.md) and
   [Review Evidence Index](reviews/README.md): public lifecycle maps for
   internal historical records.

## Areas

| Area | Purpose | Entry point |
| --- | --- | --- |
| Product | Scope, content model and product boundaries | [product](product/README.md) |
| Architecture | Frontend, backend, IPC, storage and media boundaries | [architecture](architecture/README.md) |
| Engineering | Development, testing, dependencies and documentation rules | [engineering](engineering/README.md) |
| Design | Design system, accessibility and Figma boundaries | [design](design/README.md) |
| Operations | GitHub, CI, release, rollback and incidents | [operations](operations/README.md) |
| Agents | Engineering-agent roles, Claude Code and handoffs | [agents](agents/README.md) |
| Decisions | Accepted long-lived architectural decisions | [decisions](decisions/README.md) |
| Reference | Terminology and generated references | [reference](reference/README.md) |
| Work | Task workflow and the status-ledger transition | [work](work/README.md) |

## Document planes

The public repository contains stable, non-sensitive engineering and product
documentation. Internal reviews, raw diagnostics, worktree inventories and
environment-specific evidence belong in the private engineering workspace.

The existing locations remain available during migration:

| Existing location | Current role | Target treatment |
| --- | --- | --- |
| `plan/` | Local planning and current implementation ledger | Migrate stable facts; keep one live ledger until explicit cutover |
| `docs/plans/` | Public lifecycle index plus internal dated plans | Promote durable decisions; never execute a stale baseline |
| `docs/reviews/` | Public evidence index plus internal review records | Keep raw evidence append-only; never treat it as current capability truth |
| `docs/superpowers/` | Internal legacy execution material | Keep all material private; extract durable decisions selectively and register local records in the migration map |
| `docs/internal/` and `docs/private/` | Local-only material | Never publish |

No document is removed merely because a newer document exists. A migration map,
link update and explicit cleanup decision are required.

## Public documentation rules

- Do not include secrets, tokens, cookies, signed URLs, user data, absolute
  local paths or raw diagnostic bundles.
- Claims about current capability must link to code, a contract, a Registry,
  tests or runtime evidence.
- Historical review evidence must include its commit/date boundary and must not
  be presented as proof for a different HEAD.
- Generated documents identify their generator and are never hand-edited.
- New documents use the metadata fields defined in
  [Documentation System](engineering/documentation-system.md).

## Local check

From the repository root, run both checks:

```text
python tools/docs/check.py
python tools/docs/check.test.py
python tools/release/public-snapshot-check.py
python tools/release/public-snapshot-check.test.py
```

The documentation checker validates curated public content and, when local
legacy directories exist, checks that every record is named in its lifecycle
register. The snapshot checker reads Git's tracked index, so a newly authored
required public file can pass the first check and still fail the second until
it is tracked; arbitrary untracked files are outside both public surfaces.
Before a commit, also run
`git diff --cached --check` on the exact staged allowlist.
