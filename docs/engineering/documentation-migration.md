---
doc_id: engineering.documentation-migration
type: canonical
status: active
owner: repository-steward
visibility: public
source_of_truth: documentation-publication-policy
last_reviewed: 2026-10-04
review_after: 2027-01-04
---

# Documentation Migration

Migration promotes verified, durable knowledge into maintained public documents.
It does not publish private working notes or inventories.

## Destination by subject

| Subject | Public home | Publication requirement |
| --- | --- | --- |
| Product behavior and boundaries | Product documentation | Match current contracts and consumers |
| Runtime layering and security | Architecture documentation | Verify implementation and accepted decisions |
| Architectural rationale | Individual ADR | Record decision, alternatives and consequences |
| Contributor practices | Engineering documentation | Remain portable and tool-neutral |
| Release and recovery policy | Operations documentation | Match actual workflows and artifacts |
| User-facing changes | Changelog and release notes | Describe shipped behavior and completed validation |

## Migration procedure

1. Establish ownership, purpose, lifecycle and visibility.
2. Preserve originals when they carry historical or operational value.
3. Extract only revalidated facts into the appropriate public home.
4. Remove private filenames, machine context, prompts, session procedures and
   development-tool instructions from public prose and navigation.
5. Update public links, publication rules and regression tests together.
6. Validate the final public tree before publication.

Internal catalogs remain private. A public index must not disclose a per-file
list of internal plans, reviews, accounts or development operations.

Changing the current public tree does not erase prior Git commits, published
artifacts, forks or hosting caches. A history change is a separate, scoped
operation with a recovery plan; ordinary documentation migration does not
authorize rewriting a protected branch.

See [Documentation System](documentation-system.md) for metadata and checks.
