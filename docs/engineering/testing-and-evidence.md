---
doc_id: engineering.testing-evidence
type: canonical
status: active
owner: quality
visibility: public
source_of_truth: CI-and-runtime-gates
last_reviewed: 2026-09-20
review_after: 2026-12-20
---

# Testing and Evidence

## Evidence record

Every meaningful validation result is recorded as:

```text
evidence_id:
commit_sha:
layer: unit | integration | frontend | tauri | desktop | provider | release
command_or_action:
environment:
result: PASS | FAILED | NOT RUN | NOT PROVEN
scope:
known_limits:
```

The commit SHA is mandatory for claims about code. Historical evidence is
labelled historical and cannot close a finding on a newer commit.

## Evidence levels

| Level | Proves | Does not prove |
| --- | --- | --- |
| Static inspection | A code path or contract exists | That a user can complete the flow |
| Unit test | A bounded function/property | IPC, filesystem or WebView behavior |
| Feature/integration test | Multiple layers in the test harness | Windows permissions or real providers |
| Tauri/custom-protocol test | Desktop command/resource wiring | Every WebView2 or installer behavior |
| Real Windows/WebView2 action | A recorded desktop scenario | All machines and all providers |
| Release acceptance | A candidate artifact can be installed and verified | Future changes after that SHA |

Keep `implemented`, `automatically tested`, `desktop verified` and `released`
as separate states.

## Required regression shape

For a confirmed bug, prefer:

```text
reproduce -> failing regression -> minimal fix -> focused test -> wider gate
```

Async UI fixes should cover real Promise/effect/button interaction. Concurrent
Rust fixes should use a barrier, controlled scheduling or an equivalent test,
not only a sequential helper test. Persistent-data fixes should cover fresh,
upgrade, failure and recovery paths.

## Current command families

Use the commands that exist in the checkout and record the exact command. Do
not invent a `pnpm test` or `npm run typecheck` alias when the project uses
`npm run ci:check`, `npm run test`, `npm run build`, `npm run lint`,
`npm run governance:check` and `npm run fixtures:check` under `前端/app`.

Rust, Tauri, Wire, command-manifest, ACL, migration, CodeQL and real desktop
checks are separate evidence categories. A green frontend test run does not
close a Tauri runtime risk.
