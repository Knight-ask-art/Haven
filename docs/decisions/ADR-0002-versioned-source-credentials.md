---
doc_id: decisions.versioned-source-credentials
type: decision
status: accepted
owner: architecture
visibility: public
source_of_truth: source-credential-lifecycle
last_reviewed: 2026-10-08
review_after: 2027-01-08
---

# Versioned source credentials and durable cleanup

Status: accepted

Date: 2026-10-08

Owner: architecture

Supersedes: the source-specific delete-first ordering referenced by the source
management contract. Other credential families retain their existing ordering.

## Context

SQLite and the operating system credential store cannot participate in one
transaction. Reusing a source's stable credential slot permits a failed writer
to overwrite another writer's saved secret during compensation. Deleting a
secret before revoking its database reference also leaves interrupted operations
without a durable recovery obligation.

An outbox alone is insufficient: a timed-out staged write may still complete
after cleanup has deleted its slot and forgotten the obligation. Timeouts are
not proof that an external writer has stopped.

## Decision

`SourceRegistryService` owns source credential identity and lifecycle. Each
write allocates a fresh slot bound to the source identity, source kind and a
random version. Only an exact same-source reference is readable or deletable.
Existing valid stable source references remain readable and can be rotated.
Malformed, cross-source and other-family references never authorize access to
the credential store.

Before external writing, a settings transaction persists the staged slot and
reserves cleanup capacity. Adoption atomically verifies the source, kind,
previous reference, source revision and staged identity. A stale operation
cannot adopt its slot or restore a previous secret.

Credential writes, revocation, source removal and cleanup hold a shared external
I/O lease for their whole lifecycle. File-backed assemblies use a nonblocking
operating system file lock derived from the database directory. The lock file
is never removed or replaced. Contention returns a retryable conflict; a failed
file lock never falls back to an in-process mutex. Explicit memory-database
assemblies use an explicitly injected in-process lease. Missing lease injection
fails closed for credential mutations and cleanup, not for read-only listing.

TVBox source registration and its configuration-cache write use the same lease,
acquired after remote fetching and held through registration, cache storage and
compensation. Removal therefore cannot settle an absent cache immediately before
a delayed save recreates it.

Revocation and removal first clear the authoritative reference or source record
and persist cleanup obligations in the same transaction. External deletion
then settles those obligations. A failed external operation or failed dequeue
retains its obligation for an idempotent retry. Startup and periodic bounded
cleanup recover interrupted operations. `credentialDeleted` reports only an
actual deletion, not an already-absent slot.

Cleanup identities are the exact source/reference pair. Capacity-limited staged
retirement keeps entries that do not yet fit durably owned and continues draining
existing obligations. Failed or guarded credential entries rotate to the tail
without being discarded. A still-registered source's configuration cache is
guarded from deletion.

The lease establishes quiescence only for compliant Haven paths. Trusted
credential-store implementations must finish external side effects within
their future's lifetime; detached writes may not outlive the lease. The lease
does not replace database transactions or revision/CAS validation, and does not
control other programs writing directly to the system credential store.

## Alternatives considered

- Shared slots with restore-on-failure: compensation can overwrite a concurrent
  winner and cannot be made atomic with SQLite.
- Delete-first without an outbox: interruption between external deletion and
  database mutation has no durable retry owner.
- Version slots with TTL-only recovery: a delayed live writer can recreate an
  already-cleaned slot after its obligation has been forgotten.
- Process-local arbitration for a file database: independent app instances
  would each believe themselves to be the only external writer.

## Consequences

Logical revocation can succeed before physical deletion. A cleanup error remains
visible and retryable, but revoked credentials are no longer eligible for source
requests. Callers reread authoritative source state after mutations, including
errors that may follow logical revocation. Cleanup has bounded queues and batch
limits and never discards an obligation to accommodate capacity pressure.

Sources share one short external-I/O arbitration boundary. A busy operation must
be retried; unrelated configuration changes continue through database CAS and
invalidate a stale credential writer's adoption fence.

Secrets remain exclusively in the system credential store. Database staging and
cleanup contain opaque identities only; credentials, references and underlying
store errors do not enter IPC, logs or public diagnostics.

OPDS request consumers take source kind, registered origin, configuration revision
and credential presence from one coherent registry snapshot. A source-id prefix
alone grants neither source policy nor secret access. Credentials are eligible
only on the registered HTTPS origin. After asynchronous credential resolution,
the consumer rereads and compares the authoritative snapshot before sending,
including when resolution returns no secret. Changed or removed sources invalidate
that request; the check does not recall a request already sent.

## Migration and rollback

The internal `sources` payload advances to version 4 with default-empty staging
and cleanup queues. Existing source identity, endpoint and enabled state remain.
The wire DTOs and command signatures do not change. Valid legacy references are
retired only after a new slot is adopted. Rollback to the shared-slot lifecycle
requires an explicit migration and must not silently discard pending obligations.

## Compatibility boundary

This decision applies only to source credentials. AI provider, cloud storage and
other credential-family behavior is unchanged. It grants no new network access,
local-file access, shell execution or frontend secret-read capability.

## Baseline sync

`docs/SOURCE_OF_TRUTH.md` links the source lifecycle owner. Source-management
contracts adopt this source-only ordering; other credential families retain
their existing delete-first semantics. The public command signatures and wire
DTOs remain unchanged.

## Evidence

- `source_registry/credentials.rs` and `source_registry/lease.rs`: same-source
  version slots, adoption fences, revocation and bounded durable cleanup.
- `source_credential_lease.rs`: file-lock implementation.
- `tests/source_credential_integration.rs`: transaction, ownership, failure,
  cancellation and independent-assembly scenarios.
- `src-tauri/src/state.rs` and `src-tauri/src/source_background.rs`: production
  composition and cleanup recovery.
- `tvbox_config_import.rs`: save/removal arbitration and missing-lease tests.
- `opds/request.rs`, `opds/request_tests.rs` and `state.rs`: snapshot-bound
  credential consumption and invalidation before sending.
