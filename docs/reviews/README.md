---
doc_id: work.review-evidence-index
type: historical
status: active
owner: quality
visibility: public
source_of_truth: review-evidence-lifecycle-index
last_reviewed: 2026-09-20
review_after: 2026-12-20
---

# Review Evidence Index

This index exposes the baseline and lifecycle of internal review records without
publishing raw diagnostics, local paths or temporary evidence. A review proves
only what was inspected at its recorded commit and date.

Raw reviews are append-only internal evidence. They are not edited later to make
old findings look resolved. Resolution is established by a newer commit, a
regression test and a fresh review.

Because raw records are internal, they may retain historical machine-local links
needed to interpret old evidence. Those links are not portable and are never a
reason to publish the raw file; public conclusions must be rewritten without
local paths.

## Inventory

| Record ID | Internal record | Owner | Review baseline | Finding set | Lifecycle | Links checked | Current use |
| --- | --- | --- | --- | --- | --- | --- | --- |
| `review.product.r01-r10.2026-09-05` | `docs/reviews/2026-09-05-product-review.md` | quality | `001556a`, with affected paths checked through `f22d5d7` | R01-R10 | archived | internal-only, sampled 2026-09-20 | Original product-flow findings. Later fixes and reviews exist; current resolution must be revalidated against the active SHA. |
| `review.pr18.b01-b09.2026-09-05` | `docs/reviews/2026-09-05-pr18-review.md` | quality | PR #18 head `4e13bad` | B01-B09 and R01-R10 recheck | superseded | internal-only, sampled 2026-09-20 | Replaced as PR-state evidence by the second PR #18 recheck; retained for traceability. |
| `review.pr18.c01-c04.2026-09-06` | `docs/reviews/2026-09-06-pr18-recheck.md` | quality | PR #18 head `beab46a` | C01-C04 | archived | internal-only, sampled 2026-09-20 | Final recorded PR #18 recheck in this archive. It does not describe later branches or the current `main`. |
| `review.product.r11-r14.2026-09-05` | `docs/reviews/2026-09-05-product-review-followup.md` | quality | `beab46a` | R11-R14 | archived | internal-only, sampled 2026-09-20 | Follow-up on action identity and production entry points. Later implementation does not retroactively change this evidence. |

Lifecycle here describes the review document, not whether every finding is fixed.
Use `archived` when a review run is complete and `superseded` when a newer review
replaces its PR or branch-state conclusion.

## Resolution protocol

To close a finding on current code, record:

```text
finding_id
review_baseline
fix_commit
regression_test
fresh_gate_result
independent_reviewer
desktop_or_release_limit
```

If any field is missing, report the finding as `NOT PROVEN` rather than assuming
that later code or a green unrelated test closed it.
