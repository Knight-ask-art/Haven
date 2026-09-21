---
doc_id: docs.index
type: canonical
status: active
owner: repository-steward
visibility: public
source_of_truth: documentation-system
last_reviewed: 2026-10-04
review_after: 2027-01-04
---

# Haven Documentation

This is the public documentation entrypoint for Haven (栖阅). It covers the
product, technical contracts and contributor guidance; it is not a second
product specification.

## Start here

1. [Source of Truth](SOURCE_OF_TRUTH.md): product boundaries and technical authority.
2. [Architecture Roadmap](ARCHITECTURE_ROADMAP.md): runtime layers and design constraints.
3. [Engineering Change Workflow](TASK_WORKFLOW.md): scope, implementation, tests, review and integration.
4. [Contributing](engineering/contributing.md): contributor, reviewer and maintainer responsibilities.
5. [Documentation System](engineering/documentation-system.md): ownership, lifecycle and publication rules.
6. [Testing and Evidence](engineering/testing-and-evidence.md): validation scope and evidence quality.
7. [Release and Rollback](operations/release-and-rollback.md): signed delivery and recovery.

## Areas

| Area | Purpose | Entry point |
| --- | --- | --- |
| Product | Scope, content model and user-facing boundaries | [product](product/README.md) |
| Architecture | Frontend, backend, IPC, storage, media and product AI/MCP contracts | [architecture](architecture/README.md) |
| Engineering | Contributing, tests, dependencies and documentation | [engineering](engineering/README.md) |
| Design | Design system, accessibility and presentation | [design](design/README.md) |
| Operations | GitHub, CI, release, rollback and incidents | [operations](operations/README.md) |
| Decisions | Accepted architectural decisions | [decisions](decisions/README.md) |
| Reference | Terminology and generated references | [reference](reference/README.md) |
| Work | Public issue and pull-request status | [work](work/README.md) |

## Public documentation boundary

Publish durable product behavior, architecture, security constraints and
general contributor guidance. Haven's own AI/MCP interfaces are product
contracts and may be documented here.

Developer-tool operating procedures, orchestration prompts, session-management
instructions, private inventories and raw diagnostics are not public
documentation. Neither their contents nor their per-file catalogs belong in
public indexes. Preserve needed originals privately.

Never publish secrets, tokens, cookies, signed URLs, user data or machine-local
paths. Capability claims must follow real contracts, bindings and consumers;
a plan, design or historical test result cannot prove current behavior.
Generated documents identify their generator and must not be hand-edited.

## Checks

From the repository root:

```text
python tools/docs/check.py
python tools/docs/check.test.py
python tools/release/public-snapshot-check.py
python tools/release/public-snapshot-check.test.py
```

These checks validate the public documentation and tracked publication tree.
Before committing, review the exact staged paths and run
`git diff --cached --check`. Publication policy and migration guidance are in
[Documentation System](engineering/documentation-system.md) and
[Documentation Migration](engineering/documentation-migration.md).
