# Live2D Assets

- `models/` contains the Live2D model packages bundled with Haven.
- `vendor/` is generated at build time for the local Cubism 2 runtime.

The bundled model packages come from the public `live2d-widget-models`
distribution. Package versions, integrity values, upstream source and license
obligations are recorded in the repository root's
[`THIRD_PARTY_NOTICES.md`](../../../../THIRD_PARTY_NOTICES.md). The models are
third-party assets and are not relicensed as Haven MIT code.

Future user imports must be copied into Haven's writable application-data
Live2D directory. The shared model catalog can then merge those entries with
the bundled models; installed MSIX assets under this directory are read-only.
