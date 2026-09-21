---
doc_id: architecture.comic-content-continuity
type: canonical
status: active
owner: architecture
visibility: public
source_of_truth: accepted-design-and-runtime-contracts
last_reviewed: 2026-09-20
review_after: 2026-12-20
---

# Comic Content Identity and Continuity

This document preserves the long-lived identity and progress invariants for
comic catalogs. It does not claim that every UI operation or provider scenario
has completed desktop acceptance. Current capability still requires code,
contract, test and runtime evidence as defined in
[Source of Truth](../SOURCE_OF_TRUTH.md).

## Identity model

| Object | Meaning | Stable responsibility |
| --- | --- | --- |
| Work | Logical title across sources | Owns the user-facing title and source relationships |
| Work Source Reference | One provider's remote title identity | Preserves `source_key` and remote work identity |
| Edition | A content version | Separates language, translation line, scan group and color mode |
| MediaItem | One locally addressable chapter instance | Supplies the local route and controlled resource context |
| Chapter Source Reference | A provider chapter identity | Preserves source, remote work and remote chapter identity |
| Comic Progress Subject | Stable reading continuity | Owns progress shared by equivalent local chapter instances |
| Migration Receipt | Auditable progress movement | Records strategy, evidence, revisions and undo boundary |

A provider ID is source evidence, not a permanent progress identity. Different
remote chapter IDs may join one progress subject only when backend evidence is
sufficient. Original source identities remain intact.

## Edition boundary

An Edition profile consists of:

```text
language
translation_line
scan_group
color_mode
```

Known conflicting values create distinct Editions. `Unknown` is not a wildcard:
known versus unknown remains unresolved, while unknown versus unknown does not
alone force a split. Mirror or proxy labels describe provenance and do not create
an Edition without a real content difference.

Cross-language, cross-translation-line, cross-scan-group and known color-mode
differences do not share progress by default.

## Catalog authority

Media Detail and Comic Reader consume the same Work-level catalog produced by
Application and Domain logic. The backend owns:

- chapter order and stable local navigation targets;
- Edition membership and source summaries;
- availability, refresh and truncation states;
- previous and next local MediaItem identities;
- progress and migration evidence.

The frontend may filter and render this projection. It must not infer chapter
identity from a title, chapter number, page count, array order or provider URL,
and it must navigate with the backend-provided local `mediaItemId`.

## Refresh and availability

Refreshing a source is user initiated unless a separately accepted feature says
otherwise. A failed source refresh preserves the last valid local catalog. A
truncated refresh cannot mark unseen old chapters as missing.

The catalog distinguishes at least:

| State | Meaning |
| --- | --- |
| Available | A controlled local resource can be opened |
| TemporarilyUnavailable | Previously known content cannot currently be obtained |
| ExternalOnly | A source reports content, but Haven has no controlled resource |
| Unknown | Evidence is insufficient to claim availability |
| Missing | A complete refresh no longer observes the source item |
| RefreshFailed | The current refresh failed; prior valid data remains |
| Truncated | The response is incomplete and cannot establish absence |

One source failure must not erase another source's valid chapter.

## Progress continuity and migration

Matching and migration are backend responsibilities. Evidence strength descends
from exact remote identity and authoritative content keys, through full or
partial page identity, to metadata-only candidates. Metadata alone may propose a
candidate but does not authorize a silent identity merge.

When pages change, migration attempts these strategies in order:

```text
stable page key
  -> content fingerprint
  -> nearest surviving page
  -> proportional fallback
  -> no target
```

The result exposes its strategy and confidence. Low confidence is visible and
recoverable, not silently treated as exact. Existing target progress is preserved
unless the user explicitly accepts replacement. Every write and undo observes
revision/CAS semantics; a later user read must not be overwritten by an old
migration receipt.

## Layer and security boundary

The production path remains:

```text
React UI
  -> Feature Hook / Action
  -> Feature API / Gateway
  -> Typed HavenClient
  -> Tauri Command
  -> Application
  -> Domain / Port
  -> Infrastructure
```

React does not access a provider, SQLite, source object, request header, cookie,
grant, signed URL or arbitrary file path. Reader routes use local MediaItem
identity, and page bytes use the controlled resource/session protocol.

## Evidence boundary

Unit and integration tests can prove identity, ordering, transaction and CAS
properties. They cannot prove the real MangaDex network path, Tauri command,
WebView2 drawer interaction, resource session or Windows lifecycle. Those remain
separate desktop/runtime evidence tied to an exact commit.
