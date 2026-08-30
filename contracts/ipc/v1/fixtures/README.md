# IPC contract fixtures

This directory contains public, sanitized JSON examples for the Haven IPC v1
contract. The browser Mock client imports these examples at build time so that
contributors can exercise the same DTO shapes without a running Tauri process.

These files are contract samples, not acceptance-test material. They do not
contain real user data, local paths, credentials, cookies, provider responses,
diagnostic logs, screenshots, or desktop acceptance evidence. Local test
sources and evidence remain outside the public repository.

When an IPC DTO changes, update the Rust source-of-truth, regenerate the
TypeScript bindings through the repository's generation workflow, and revise
the affected examples together. Keep fixtures deterministic and minimal; do
not add production database exports or captured network payloads here.
