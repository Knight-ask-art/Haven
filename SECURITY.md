# Security policy

## Reporting a vulnerability

Please do not publish credentials, signed URLs, private files, or a full
diagnostic export in a public issue. Use the repository's private security
contact (GitHub Security Advisories, when enabled) or contact the maintainers
before disclosing a vulnerability publicly.

For ordinary reproducible application failures, use the in-app redacted report
preview and the [bug report template](https://github.com/Knight-ask-art/Haven/issues/new?template=bug_report.yml).
The report workflow never asks for a GitHub token and opens a fixed issue URL
only after explicit user confirmation.

## Supported versions

Only the latest tagged release and the current `main` branch receive security
fixes while the project is in its `v0.1.0-beta.1` public-preview phase.

## Safe diagnostics

Diagnostic exports are intentionally bounded and redacted. Do not add passwords,
cookies, authorization headers, signed URLs, full local paths, media content or
search text to an issue. If a report contains sensitive data, delete it locally
and notify the maintainers before sharing any replacement evidence.
