---
doc_id: operations.release-rollback
type: canonical
status: active
owner: release-owner
visibility: public
source_of_truth: release-workflows-and-artifacts
last_reviewed: 2026-10-04
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

## Signed Windows update channel

Stable tags on protected main use the existing updater public key and the
GitHub `releases/latest/download/latest.json` endpoint. The release workflow
first creates a draft, validates version and frontend asset hashes, verifies
the NSIS signature against the application's tracked public key, and extracts
the installer to inspect the shipped executable's embedded build manifest.
Only a verified stable draft is published as latest; prereleases stay drafts.

The updater manifest prefers NSIS. MSI remains a manual-download artifact.
Published releases are immutable: retries cannot overwrite them, and fixes
require a new version. Do not rotate the public key as a shortcut for a failed
signature gate: installed clients depend on that trust chain.

Desktop builds run the frontend build hook. `--verify-bundle` reads the
executable's embedded provenance without creating a window or opening user
data. This proves the shipped asset bundle, not interactive desktop acceptance.
The installation handoff drains the active window's session grants and reading
records before the official updater starts the Windows installer and restarts.

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

Do not expose or distribute a secret while investigating it. Preserve only
redacted evidence and use the appropriate revocation and recovery procedure.
