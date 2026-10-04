"""Fail closed before publishing a stable Windows update channel."""
from __future__ import annotations

import argparse
import json
from pathlib import Path
from urllib.parse import urlsplit, unquote


def validate(metadata: dict, assets: Path, version: str, repository: str, tag: str) -> list[str]:
    errors: list[str] = []
    if metadata.get("version", "").removeprefix("v") != version:
        errors.append("update metadata version does not match the installed product version")
    platform = metadata.get("platforms", {}).get("windows-x86_64")
    if not isinstance(platform, dict):
        return errors + ["Windows x86_64 update is missing"]
    url = urlsplit(platform.get("url", ""))
    prefix = f"/{repository}/releases/download/{tag}/"
    if url.scheme != "https" or url.hostname != "github.com" or url.username or url.password or url.query or url.fragment or not url.path.startswith(prefix):
        return errors + ["update must use the immutable HTTPS GitHub release artifact"]
    name = unquote(url.path[len(prefix):])
    if not name.endswith("-setup.exe") or "/" in name or "\\" in name:
        return errors + ["automatic Windows updates must select the NSIS installer"]
    installer = assets / name
    signature = assets / f"{name}.sig"
    if not installer.is_file() or installer.stat().st_size < 1024 or installer.read_bytes()[:2] != b"MZ":
        errors.append("Windows installer artifact is missing or invalid")
    if not signature.is_file() or signature.read_text(encoding="utf-8").strip() != platform.get("signature", "").strip():
        errors.append("metadata signature differs from the installer signature sidecar")
    if not isinstance(metadata.get("notes", ""), str):
        errors.append("release notes must be plain text")
    return errors


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--metadata", type=Path, required=True)
    parser.add_argument("--assets", type=Path, required=True)
    parser.add_argument("--version", required=True)
    parser.add_argument("--repository", required=True)
    parser.add_argument("--tag", required=True)
    options = parser.parse_args()
    try:
        value = json.loads(options.metadata.read_text(encoding="utf-8"))
        errors = validate(value, options.assets, options.version, options.repository, options.tag)
    except (OSError, ValueError, TypeError, AttributeError):
        errors = ["malformed updater metadata"]
    for error in errors:
        print(f"updater-manifest-check: {error}")
    if errors:
        return 1
    print("updater-manifest-check: release version, immutable NSIS URL and signature sidecar verified")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
