"""Verify the current desktop frontend provenance and its shipped UI styles."""
from __future__ import annotations

import argparse
import hashlib
import json
import re
from pathlib import Path


def validate(dist: Path, version: str, commit: str | None = None, clean: bool = False) -> list[str]:
    errors: list[str] = []
    try:
        info = json.loads((dist / "build-info.json").read_text(encoding="utf-8"))
        if info.get("schemaVersion") != 1 or info.get("version") != version:
            errors.append("frontend build version does not match the desktop product")
        if not re.fullmatch(r"[0-9a-f]{40}", info.get("commit", "")):
            errors.append("frontend build commit is invalid")
        if commit and info.get("commit") != commit:
            errors.append("frontend build commit differs from the release commit")
        if clean and info.get("dirty") is not False:
            errors.append("release frontend was built from an uncommitted tree")
        entries = info.get("assets")
        if not isinstance(entries, list) or not entries:
            return errors + ["frontend build has no asset manifest"]
        css = ""
        seen: set[str] = set()
        for entry in entries:
            relative = entry.get("path", "")
            if not isinstance(relative, str) or not relative or "\\" in relative:
                errors.append("invalid asset path")
                continue
            file = (dist / relative).resolve()
            if not file.is_relative_to(dist.resolve()) or relative in seen:
                errors.append("duplicate or out-of-root asset path")
                continue
            seen.add(relative)
            data = file.read_bytes()
            if len(data) != entry.get("byteSize") or hashlib.sha256(data).hexdigest() != entry.get("sha256"):
                errors.append("frontend asset differs from the build manifest")
            if relative.startswith("assets/") and relative.endswith(".css"):
                css += data.decode("utf-8")
        if "index.html" not in seen:
            errors.append("frontend entry point is missing")
        for selector in (".settings-navigation__text", ".sources-settings", ".search-results"):
            if selector not in css:
                errors.append(f"production UI style missing: {selector}")
    except (OSError, ValueError, TypeError, AttributeError):
        errors.append("unable to read valid desktop frontend provenance")
    return errors


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--dist", type=Path, default=Path(__file__).resolve().parents[2] / "前端/app/dist")
    parser.add_argument("--version", required=True)
    parser.add_argument("--commit")
    parser.add_argument("--clean", action="store_true")
    options = parser.parse_args()
    errors = validate(options.dist, options.version, options.commit, options.clean)
    for error in errors:
        print(f"desktop-assets-check: {error}")
    if errors:
        return 1
    print("desktop-assets-check: current Settings and Search assets, version, commit and hashes verified")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
