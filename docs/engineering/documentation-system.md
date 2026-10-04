---
doc_id: engineering.documentation-system
type: canonical
status: active
owner: repository-steward
visibility: public
source_of_truth: documentation-system
last_reviewed: 2026-09-20
review_after: 2026-12-20
---

# Documentation System

## One fact, one home

Do not solve drift by copying a paragraph into more places. Put the fact in its
authoritative document, then link to it from guides, plans, reviews and README
files.

Stable public documents live under `docs/`. Local planning, raw evidence and
internal review material may remain outside the public snapshot. A document
must declare its visibility before it is moved or published.

## Lifecycle

```text
draft -> active -> deprecated -> superseded -> archived
```

- `draft`: proposal; cannot be used as an implementation contract.
- `active`: current document with an owner and review date.
- `deprecated`: still useful, but a replacement is named.
- `superseded`: retained only for navigation or historical context.
- `archived`: no longer used for current decisions.

Plans are temporary unless they contain a durable decision. Reviews are
append-only evidence. An issue or PR is not a replacement for a stable
architecture document.

## Metadata contract

Every curated public document has:

```yaml
doc_id: unique.stable.id
type: canonical | guide | decision | reference | generated | historical
status: draft | active | deprecated | superseded | archived
owner: role-or-team
visibility: public | internal
source_of_truth: stable identifier for the authoritative artifact or fact class
last_reviewed: YYYY-MM-DD
review_after: YYYY-MM-DD
```

`doc_id` is stable across a rename. A path is not an identity. Generated docs
also declare `generated_from` and `generated_by`.

`source_of_truth` is not a closed enum. It names the authority precisely enough
to resolve drift, for example `runtime-contracts`, `accepted-decisions`,
`GitHub-settings-and-workflows` or `review-evidence-lifecycle-index`.

Private execution state and operational catalogs are not public metadata.
Public work descriptions refer to issues, pull requests and release notes.

## File placement and naming

- Give maintained documents stable semantic names in lowercase kebab case, such
  as `testing-and-evidence.md`. A renamed path keeps the same `doc_id`.
- Use a date prefix only for time-bound plans, reviews, incident records and
  release evidence. Their recorded baseline remains part of their identity.
- Do not create names such as `final.md`, `final-v2.md`, `new-plan.md` or
  `latest-review.md`; lifecycle metadata and indexes express replacement.
- Put one `README.md` index at an area boundary. An index routes readers and
  records lifecycle; it does not duplicate the area's canonical content.
- Put generated files only at a path owned by their generator and declare
  `generated_from` and `generated_by`. Never hand-edit generated output.
- Keep raw logs, private filenames, development-tool procedures and operational
  inventories outside the curated public tree. Product AI/MCP integration
  documentation remains public when it describes Haven's user-facing contract.

## Migration map

Before moving existing files, create a map with:

```text
old_path
new_path
doc_id
classification
visibility
owner
action: keep | rewrite | merge | supersede | archive | delete
reason
links_checked
```

Internal migration maps remain private. A completed review is not silently
rewritten into a current status page; its conclusions are extracted into a new
canonical document and the original remains historical evidence.

## Freshness

Freshness is a review obligation, not an automatic truth downgrade. When a
document is past `review_after`, the checker reports `STALE_REVIEW` and the
owner must either refresh it, mark it historical, or link the replacement.

All audit claims must include an `as_of` commit or date. Historical test output
cannot prove a different HEAD.

## Required checks

The `python tools/docs/check.py` command verifies:

- unique `doc_id` and valid metadata;
- Markdown link targets and private-path boundaries;
- every curated public document declares public visibility;
- no secrets, signed URLs, cookies or absolute local paths;
- no developer-tool operating procedures or private per-file catalogs;
- generated-document metadata (`generated_from` and `generated_by`);
- stale review warnings;
- local lifecycle and ledger checks use private registers when private records
  are present; no public index is required to list them.

The public snapshot check additionally protects required public files, local-only
directories, generated artifacts and the distinction between repository-local
agent instructions and public documentation.
