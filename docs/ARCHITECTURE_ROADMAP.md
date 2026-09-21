---
doc_id: architecture.roadmap
type: canonical
status: active
owner: architecture
visibility: public
source_of_truth: SOURCE_OF_TRUTH-and-runtime-contracts
last_reviewed: 2026-09-20
review_after: 2026-12-20
---

# Haven Architecture Roadmap

This roadmap describes the documentation and engineering structure. It is not
a promise that every planned capability already exists.

## Runtime layering

Production calls follow:

```text
React UI
  -> Feature Hook / Action
  -> Feature API / Gateway
  -> Typed HavenClient
  -> Tauri Command
  -> Application Service
  -> Domain / Port
  -> Infrastructure
```

React features do not call `invoke`, SQLite, a Source Provider, a Storage
Provider, arbitrary paths or arbitrary URLs directly. Command registration,
permissions and generated bindings must remain aligned.

## Documentation workstreams

| Phase | Result | Gate |
| --- | --- | --- |
| Foundation | Public index, metadata, authority matrix and agent entrypoint | Links and metadata check |
| Migration map | Every existing plan/review/doc has a target, owner and status | No duplicate live ledger |
| Canonical content | Stable product, architecture, engineering, operations and agent docs | Independent review |
| Automation | Link, metadata, visibility, secret-marker and stale-reference checks | CI-compatible command |
| Cutover | Status ledger and active links move to the new system | Explicit owner approval |
| Cleanup | Superseded material is archived or removed only with evidence | `git diff --check`, review and rollback path |

## Product capability workstreams

Product implementation remains feature-oriented. Each cross-layer change gets a
small contract, an owner, a validation matrix and a separate runtime-acceptance
statement. The documentation system must not turn a roadmap item into a
capability claim.

## Design constraints

- Preserve existing visual design when repairing data flow unless a task
  explicitly requests redesign.
- Keep Feature Registry and Settings Registry aligned with real bindings and
  consumers.
- Keep generated IPC files derived from their Rust source and generator.
- Treat persistent data, migrations, credentials, resource protocols and
  release workflows as high-risk changes.
- Keep product Copilot contracts separate from engineering-agent permissions.
