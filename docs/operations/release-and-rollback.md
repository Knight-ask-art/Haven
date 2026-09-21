---
doc_id: operations.release-rollback
type: canonical
status: active
owner: release-owner
visibility: public
source_of_truth: release-workflows-and-artifacts
last_reviewed: 2026-09-20
review_after: 2026-12-20
---

# Release and Rollback

## Release evidence

A release candidate is tied to one immutable commit and must identify:

```text
version
commit SHA
target OS and WebView2 assumptions
automated gates
desktop acceptance
database/migration state
artifact checksums
signature verification
upgrade path
rollback or recovery path
known limits
```

“Release created” is not the same as “users can safely upgrade”.

## Recovery levers

Do not treat a compile-time feature flag as a universal emergency switch for an
already-installed desktop application. Depending on the failure, the actual
levers are:

1. stop or hold the update/release channel;
2. publish a patched release through the normal signed path;
3. revert the source change through a PR;
4. execute forward data recovery or restore a verified backup;
5. communicate the affected version and user action.

Persistent-data changes require a migration and recovery plan before merge.
Do not promise a down-migration when only forward recovery is safe.

## Secret incident

If a credential is suspected to have entered GitHub, logs or an artifact:

1. stop distribution and avoid further exposure;
2. rotate or revoke the credential first;
3. preserve a redacted incident record;
4. assess whether history rewriting is necessary;
5. repair the source and verify scanners before resuming release.

An agent must not print, copy or “clean up” a secret while investigating it.
