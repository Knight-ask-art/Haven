---
doc_id: work.index
type: guide
status: active
owner: integration-owner
visibility: public
source_of_truth: task-ledger
last_reviewed: 2026-09-20
review_after: 2026-12-20
active_ledger: plan/IMPLEMENTATION_STATUS.md
ledger_cutover: pending
ledger_state: stale
ledger_as_of: 2026-09-15
---

# Work and Status

There must be exactly one live implementation ledger.

During the documentation migration, that ledger remains
`plan/IMPLEMENTATION_STATUS.md`. Do not create a competing status file in this
directory. Once the migration owner approves the cutover, the ledger moves to
`docs/work/STATUS.md` in one reviewed change and all task links are updated.

The metadata above is the single declaration of the active path, cutover state
and freshness. `ledger_state: stale` means the ledger must be re-baselined before
assigning new work or citing an existing row as current. It does not authorize a
second ledger or an unreviewed cutover.

GitHub Issues and PRs link to the ledger entry and carry discussion, review and
CI state. They do not duplicate the full ledger or prove product capability.

Each task entry must include an owner, risk, Allowed Paths, Forbidden Paths,
acceptance, verifier, current status, evidence and next step. “Done” requires
the evidence states in [Testing and Evidence](../engineering/testing-and-evidence.md),
not only a commit message.
