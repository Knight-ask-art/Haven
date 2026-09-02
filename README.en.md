<div align="center">
  <img src="assets/haven-banner.png" alt="Haven" width="720" />

  <img src="assets/haven-icon.png" alt="Haven icon" width="112" />

  <h1>Haven (栖阅)</h1>

  <p>A local-first personal space for stories, media, and reading progress.</p>

  <p>
    <a href="README.md">简体中文</a>
    · <a href="https://github.com/Knight-ask-art/Haven">GitHub</a>
    · <a href="LICENSE">MIT License</a>
  </p>

  <p>
    <img src="https://img.shields.io/badge/Tauri-2-24C8DB?logo=tauri&logoColor=white" alt="Tauri 2" />
    <img src="https://img.shields.io/badge/React-19-61DAFB?logo=react&logoColor=111827" alt="React 19" />
    <img src="https://img.shields.io/badge/Rust-1.94.1-000000?logo=rust&logoColor=white" alt="Rust 1.94.1" />
    <img src="https://img.shields.io/badge/platform-Windows-0078D4?logo=windows&logoColor=white" alt="Windows" />
    <a href="LICENSE"><img src="https://img.shields.io/badge/license-MIT-2ea44f" alt="MIT License" /></a>
  </p>
</div>

## Overview

<b>Haven (栖阅)</b>
Haven (栖阅) is a Local-first desktop reading and multimedia library management
tool.

The project combines a system-level backend with a modern frontend: file
indexing, reading progress, media favorites, history, annotations, and settings
are all powered by local Rust + SQLite to keep performance high and data
private; React is used solely for interaction and presentation.

Core features

Local-first: No account is required; all local reading, media management, and
data organization capabilities are available without signing in.

Rust + SQLite storage engine: Efficiently manages large metadata collections
and local file indexes with low resource usage and responsive queries.

Controlled network security model: Network fetching, remote image loading, and
external Provider calls are restricted by strict source allowlists and resource
isolation policies to reduce privacy risks.

Clean public repository architecture: The open-source repository focuses on the
runnable core product, product test source, build configuration, and public
contract examples, keeping the engineering structure concise and transparent.

## Core capabilities

- Register local folders, scan media, and organize Work → Edition → MediaItem.
- Play local and controlled remote video resources.
- Read TXT, Markdown, EPUB, PDF, and HTML/Article content through controlled
  session resources. PDF rendering uses the bundled PDF.js runtime.
- Read CBZ archives and direct image directories in single-page, double-page,
  strip, and RTL/LTR modes, with bounded preloading and bad-page isolation.
- Keep favorites, progress, playback/reading history, markers, and resource-
  level Reading/Comic preferences with restart recovery.
- Search the local library and enabled built-in sources. A failed source is
  shown as a partial, retryable result instead of failing the entire search.
- Queue offline downloads with restart recovery, storage admission checks, and
  a unified notice center.
- Cache artwork through the backend, use type-specific default-cover fallbacks,
  and self-heal missing cache files. The frontend never fetches third-party
  posters directly.
- Use the `Ctrl+Shift+S` player shortcut to capture a bounded JPEG through a
  Windows save dialog. The default directory is `Downloads/Haven/Screenshots`.
- Apply strict CSP, minimal Tauri capabilities, generated command/ACL checks,
  and redacted structured logs.

## Supported formats and boundaries

The first release targets Windows local content: video, TXT, Markdown, EPUB,
PDF, HTML, CBZ, and image directories. See [CHANGELOG.md](CHANGELOG.md) for
the detailed support matrix, known limitations, and compatibility notes.

Haven does not currently provide external subtitle files, audio-track
selection, or user-controlled HEVC decoding. OCR/translation/AI, Sync, and
full local-data deletion require separate foundations and risk reviews. Auto-
update signing is wired to the Tauri updater, but installable releases are only
available after the maintainer configures signing secrets and publishes a
signed GitHub Release. The beta route is `0.1.0-beta.1`; `0.1.0` is reserved for
the stable release. Long-running playback, very large EPUB/comic libraries,
and extreme download-worker conditions remain user-feedback areas rather than
unsupported guarantees.

This release is Windows desktop only. Android and iOS builds are outside the
current product scope.

## Built-in sources

All built-in sources are expected to provide a real search implementation. The
public capability table, source types, configuration requirements, upstream
limits, and privacy boundary are documented in [SOURCES.md](SOURCES.md).

## Architecture and public fixtures

```text
React UI
  -> Feature Hook / Action
  -> Typed HavenClient
  -> Tauri Command
  -> Application Service
  -> Domain / Repository Port
  -> Infrastructure (SQLite, files, network, Windows)
```

Rust + SQLite are the persistence authority. Browser Mock is explicit and does
not silently become the production Tauri path. Image bytes, video frames, PDF
pages, and comic originals are delivered through controlled resource protocols
or bounded uploads instead of ordinary JSON IPC.

`contracts/ipc/v1/fixtures/` contains deterministic, sanitized public examples
used by the Browser Mock client. These are contract samples, not desktop
acceptance screenshots, diagnostic logs, or real user data.

## Quick start (Windows)

Install Node.js 22, Rust 1.94.1 from [rust-toolchain.toml](rust-toolchain.toml),
and the Windows build tools required by Tauri 2:

```powershell
cd 前端/app
npm install
npm run dev
```

To build an independent custom-protocol desktop executable without a
`localhost:1420` development server:

```powershell
cd 前端/app
npm ci
npm run build
cd ../../src-tauri
cargo build --locked --features custom-protocol
```

The root `build.ps1` script performs the dependency installation, frontend
build, and Tauri build in one command. Use `-SkipInstall` when dependencies are
already present.

## Development checks

Product tests are public source and run in CI. Generated output, local logs,
diagnostic exports, and desktop acceptance evidence are not part of the public
tree.

```powershell
cd 前端/app
npm run ci:check

cd ../../后端
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace --locked
cargo build --workspace

cd ../src-tauri
cargo fmt --all -- --check
cargo clippy --all-targets -- -D warnings
cargo test --locked
cargo build --locked --features custom-protocol
```

## Diagnostics and issue reports

The Settings page can preview and export a redacted diagnostic report. It
contains a report ID, build/system/mode metadata, stable error codes, and a
limited sanitized log summary. It does not include databases, media files,
book contents, search terms, cookies, tokens, signed URLs, full URLs, or
absolute paths. After explicit confirmation, the user can open the fixed Haven
GitHub Issue template and attach the exported report manually.

Haven never stores or sends a GitHub token without user authorization. For a
security issue, follow [SECURITY.md](SECURITY.md) and do not paste credentials
or complete diagnostic exports into a public issue.

## Releases and updates

Source control contains product code, public tests, and build configuration.
Windows installers,
MSI/NSIS archives, updater signatures, `latest.json`, checksums, and release
notes belong in GitHub Releases rather than `main`.

The Tauri updater checks the fixed HTTPS GitHub Release endpoint and verifies
minisign metadata before installation. The signing private key is configured
only as GitHub Actions secrets; it is never committed to this repository.

## License

Haven source code is released under the [MIT License](LICENSE). Vendored
runtime components, Live2D models, fonts, and other third-party material remain
subject to their own licenses and [THIRD_PARTY_NOTICES.md](THIRD_PARTY_NOTICES.md).
