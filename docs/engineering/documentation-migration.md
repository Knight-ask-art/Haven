---
doc_id: engineering.documentation-migration
type: canonical
status: active
owner: repository-steward
visibility: public
source_of_truth: documentation-system-and-git-tree
last_reviewed: 2026-09-20
review_after: 2026-12-20
---

# Documentation Migration Map

This map separates stable public documentation from local planning and
historical evidence. It is a navigation and ownership record, not a claim that
every source listed below is current.

| Existing path | Target or canonical home | Classification | Visibility | Action | Reason |
| --- | --- | --- | --- | --- | --- |
| `plan/IMPLEMENTATION_STATUS.md` | `docs/work/STATUS.md` after explicit cutover; `docs/work/README.md` is the transition pointer | Live task ledger | Internal | keep | There must be one live ledger; do not create a competing status board. |
| `plan/` | Relevant `docs/engineering/`, `docs/operations/` or `docs/decisions/` document | Local planning corpus | Internal | extract durable facts | Plans are not runtime truth and should not be copied wholesale. |
| `docs/plans/README.md` | Same path | Public lifecycle index | Public | maintain | The index is safe to publish; raw dated plans remain internal. |
| `docs/plans/*.md` except `README.md` | Canonical engineering, operations or decision document | Dated plan archive | Internal | map, then keep or archive | Completed plans retain context; durable decisions move to a stable home. |
| `docs/reviews/README.md` | Same path | Public evidence index | Public | maintain | The index exposes baselines and lifecycle without publishing raw diagnostics. |
| `docs/reviews/*.md` except `README.md` | Canonical document only for durable conclusions | Historical review evidence | Internal | keep append-only | A review is bound to its original commit and must not be rewritten as current truth. |
| `docs/superpowers/` | `docs/architecture/`, `docs/engineering/` or an accepted ADR | Legacy execution material | Internal, except the exact grandfathered public design record | extract selectively | Keep the public exception content-scanned and add no new public files in this tree. |
| `AGENTS.md` | Repository-local governance | Agent instruction file | Local | keep outside public snapshot | It may contain machine-specific or operator-only context. |
| `CLAUDE.md` | `docs/agents/claude-code.md` | Agent entrypoint | Public | keep thin | The entrypoint points to the detailed delegation contract without duplicating it. |
| `docs/` canonical files | `docs/README.md` navigation | Stable public documentation | Public | keep and review | These files are the maintained public documentation plane. |

## Migration rules

1. Inventory a file before moving or deleting it.
2. Assign one `doc_id`, owner, visibility and lifecycle state.
3. Extract durable facts; do not paste entire plans or reviews into a new
   document.
4. Update links and run the documentation checker before changing visibility.
5. Preserve historical evidence unless an explicit owner approves deletion and
   a recovery path exists.

The migration is complete only when the live ledger has an approved replacement,
all active links point to the new home and the public snapshot check passes.

## Root plan corpus register

The ignored `plan/` directory contains the local documents listed below. The
classification records their treatment; it does not publish them or assert that their
implementation claims match the current checkout. A target home is a migration
destination, not evidence that extraction has already happened.

| Local record | Classification and current lifecycle | Target home | Action and boundary |
| --- | --- | --- | --- |
| `plan/README.md` | Legacy product overview; superseded as a documentation entrypoint | `README.md`, `docs/product/` | Extract any still-current product facts, then archive. Do not preserve its project-status prose as current evidence. |
| `plan/PRODUCT.md` | Draft canonical candidate | `docs/product/` | Reconcile scope and non-goals with code and accepted decisions, then split into maintained product documents. |
| `plan/INFORMATION_ARCHITECTURE.md` | Draft canonical candidate | `docs/product/`, `docs/design/` | Extract navigation and page hierarchy only after checking the current router and Registry. |
| `plan/HAVEN_DESIGN_SYSTEM.md` | Draft canonical candidate | `docs/design/` | Reconcile tokens and components with the current frontend and Figma evidence before promotion. |
| `plan/TECHNICAL_ARCHITECTURE.md` | Draft canonical candidate | `docs/architecture/` | Extract only dependency directions and boundaries verified against current manifests and code. |
| `plan/DOMAIN_MODEL.md` | Draft canonical candidate | `docs/architecture/` | Migrate domain invariants by bounded topic; Domain/Application/schema remain runtime authority. |
| `plan/FRONTEND_BACKEND_CONTRACT.md` | Active interim contract (`v1.2`) | `docs/architecture/`, `docs/reference/` | Keep as an internal normative input until each section is checked against Wire, commands and consumers. Do not bulk-copy placeholders. |
| `plan/SOURCE_ENGINE.md` | Draft canonical candidate | `docs/architecture/` | Extract the restricted Source boundary and accepted contracts; keep unimplemented DSL and update behavior marked planned. |
| `plan/SOURCES_INVENTORY.md` | Time-bound research inventory from 2026-08 | Private research; curated results may enter `docs/reference/` | Revalidate legality, availability and implementation before reuse. Raw endpoints and risk notes remain internal. |
| `plan/MEDIA_ENGINES.md` | Draft canonical candidate | `docs/architecture/` | Split by player, reader, comic and periodical engine after checking real format support. |
| `plan/LIBRARY_AND_STORAGE.md` | Draft canonical candidate | `docs/architecture/` | Extract stable resource and storage lifecycle invariants; do not promote planned cloud support. |
| `plan/DOWNLOAD_SYSTEM.md` | Draft canonical candidate | `docs/architecture/` | Reconcile scheduler and offline lifecycle claims with Application, ports and repositories before promotion. |
| `plan/SYNC_AND_STATE.md` | Draft future-system specification | `docs/architecture/`, `docs/decisions/` | Keep planned until a foundation task and accepted ADR establish a real sync contract. |
| `plan/AI_SYSTEM.md` | Draft product and architecture specification | `docs/product/`, `docs/architecture/`, `docs/decisions/` | Extract privacy, boundary and no-autonomous-write decisions; keep provider, OCR and RAG proposals explicitly planned. |
| `plan/AI_AGENT_RUNTIME.md` | Draft contract bound to a 2026-09-17 local worktree | `docs/product/ai-boundaries.md`, `docs/architecture/` | Rebaseline against the current checkout before extracting runtime contracts or implementation state. |
| `plan/AI_AGENT_IMPLEMENTATION_PLAN.md` | Partially checked execution plan bound to a local worktree | `docs/plans/README.md` for any successor plan | Supersede this run after preserving evidence; unchecked phases require a new baseline and acceptance. |
| `plan/SECURITY_PRIVACY_COMPLIANCE.md` | Draft high-priority policy candidate | `SECURITY.md`, `docs/decisions/`, `docs/architecture/` | Keep its stricter constraints during migration, but verify each implementation claim before calling it active policy. |
| `plan/DEVELOPMENT_ROADMAP.md` | Legacy execution/governance monolith; superseded in structure | `docs/ARCHITECTURE_ROADMAP.md`, `docs/TASK_WORKFLOW.md`, `docs/agents/`, `docs/operations/` | Extract unique gates and ownership rules, then archive; do not create a second task ledger. |
| `plan/GIT_GOVERNANCE.md` | Historical transition runbook with obsolete repository-state facts | `docs/engineering/git-worktree-and-commits.md`, `docs/operations/github.md` | Preserve useful safety rules, archive old parent-repository diagnostics and never execute stale migration steps. |
| `plan/DECISIONS.md` | Legacy owner-decision log | `docs/decisions/` | Convert still-binding decisions into individual ADRs after code and owner validation; retain the log as history. |
| `plan/IMPLEMENTATION_STATUS.md` | Designated live ledger; path and freshness are declared by [Work and Status](../work/README.md) | `docs/work/STATUS.md` after explicit cutover | Keep it as the only ledger until a single reviewed cutover updates every link. |
| `plan/AZURE-FREE-TIER-USAGE.md` | Private environment and account operations record | Private operations vault, never public documentation | Keep ignored, review exposed identifiers and credential references, and archive or delete only with owner authorization. |
| `plan/SETTINGS_OVERVIEW_LAYOUT_CONTENT.md` | Unbaselined design plan with unchecked acceptance | `docs/design/`; successor plans indexed by `docs/plans/README.md` | Rebaseline against the current Settings Registry and UI before execution; sample values are not production facts. |
| `plan/THEME_SYSTEM_PLAN.md` | Planned foundation design; implementation explicitly out of scope | `docs/design/`, `docs/decisions/` | Keep planned. Promote only an accepted model and real runtime contract, not the unchecked plan. |
| `plan/SETTINGS_PERSONALIZATION_CAPABILITY_AUDIT.md` | Historical audit bound to `main@b008f3b` on 2026-09-20 | `docs/engineering/`, `docs/architecture/` | Preserve as internal evidence and extract durable Registry rules; re-audit changed code before using capability conclusions. |

Several legacy specifications reference `plan/SEARCH_AND_METADATA.md`, but that
file is absent from the local corpus. Treat those links as an unresolved legacy
integrity gap; do not reconstruct the missing contract from downstream prose.

## Completed extractions

| Legacy record | Canonical destination | Result |
| --- | --- | --- |
| `docs/superpowers/specs/2026-09-10-comic-reading-center-design.md` | [Comic Content Identity and Continuity](../architecture/comic-content-continuity.md) | Durable identity, catalog, migration and safety invariants extracted. The legacy copy remains as an explicitly grandfathered public-history file until cleanup is separately authorized. |

The internal implementation plan and verification records for that work remain
under `docs/superpowers/`. They are not current capability evidence and are not
part of the public snapshot. No new file may use this grandfathered path.

## Legacy file register

The plans and review indexes own their per-file records. The remaining legacy
execution files are mapped here:

| old_path | new_path | doc_id | classification | visibility | owner | action | reason | links_checked |
| --- | --- | --- | --- | --- | --- | --- | --- | --- |
| `docs/superpowers/specs/2026-09-10-comic-reading-center-design.md` | `docs/architecture/comic-content-continuity.md` | `legacy.comic-continuity.spec.2026-09-10` | accepted design history | grandfathered public | architecture | extract and retain | Cleanup needs separate authorization; canonical invariants now have a maintained home. | yes, 2026-09-20 |
| `docs/superpowers/plans/2026-09-10-comic-reading-center.md` | `docs/architecture/comic-content-continuity.md` | `legacy.comic-continuity.plan.2026-09-10` | execution plan | internal | feature-owner | archive | Its implementation snapshots and checkbox state are not current task truth. | sampled, 2026-09-20 |
| `docs/superpowers/verification/2026-09-10-comic-reading-center-gates.md` | `docs/engineering/testing-and-evidence.md` | `legacy.comic-continuity.gates.2026-09-10` | historical verification | internal | quality | archive | Results are tied to old working-tree baselines and cannot serve as a fresh gate. | sampled, 2026-09-20 |
| `docs/superpowers/verification/2026-09-14-content-capability-audit.md` | `docs/SOURCE_OF_TRUTH.md` | `legacy.content-capability-audit.2026-09-14` | capability audit | internal | quality | archive | Durable evidence rules were promoted; quantitative findings remain historical. | sampled, 2026-09-20 |
