"""Validate the files and runtime metadata allowed in the public snapshot.

This is a release check, not a product test suite. It intentionally inspects
Git's tracked tree so ignored local diagnostics, plans, and acceptance evidence
cannot affect the result on a developer machine. Product unit/integration test
source is expected to be public and is therefore allowed in the tracked tree.
"""

from __future__ import annotations

import json
import re
import subprocess
import sys
from pathlib import Path
from typing import Any


REQUIRED_FILES = {
    "README.md",
    "CHANGELOG.md",
    "SOURCES.md",
    "LICENSE",
    "SECURITY.md",
    "rust-toolchain.toml",
    ".node-version",
    ".editorconfig",
    ".github/workflows/ci.yml",
    ".github/workflows/codeql.yml",
    ".github/dependabot.yml",
    "README.en.md",
    "后端/crates/haven-application/resources/builtin-sources.json",
    "前端/app/src/lib/ipc/generated/wire.ts",
    "contracts/ipc/v1/fixtures/README.md",
}
FORBIDDEN_ROOT_SEGMENTS = {"docs", "plan", "测试", "参考项目", "logs", "tmp", ".tmp"}
FORBIDDEN_PUBLIC_PREFIXES = (
    "src-tauri/icons/android/",
    "src-tauri/icons/ios/",
)
FORBIDDEN_LOCAL_ONLY_PREFIXES = (
    "后端/crates/haven-infrastructure/tests/metadata_sources_live_diagnostic.rs",
    "后端/crates/haven-infrastructure/tests/opds_live_diagnostic.rs",
    "前端/app/src/spike/foliate/foliate-js/tests/",
)
FORBIDDEN_SUFFIXES = (
    ".map",
    ".log",
    ".out",
    ".trace",
    ".dump",
    ".har",
    ".db",
    ".sqlite",
    ".sqlite3",
)
FORBIDDEN_STATUS_WORDS = (
    "目录登记",
    "待接入",
    "未接入",
    "unimplemented",
    "not implemented",
)
SOURCE_CATEGORIES = {"video", "book", "comic", "periodical"}
SOURCE_KINDS = {"metadata", "stream", "download"}


def tracked_files(root: Path) -> list[str]:
    result = subprocess.run(
        ["git", "ls-files", "-z"],
        cwd=root,
        check=True,
        capture_output=True,
    )
    return [item for item in result.stdout.decode("utf-8").split("\0") if item]


def add_error(errors: list[str], message: str) -> None:
    errors.append(message)


def check_public_tree(root: Path, files: list[str], errors: list[str]) -> None:
    tracked = set(files)
    for required in sorted(REQUIRED_FILES - tracked):
        add_error(errors, f"required public file is not tracked: {required}")

    for relative in files:
        path = Path(relative)
        normalized = relative.replace("\\", "/")
        if path.parts and path.parts[0] in FORBIDDEN_ROOT_SEGMENTS:
            add_error(errors, f"forbidden public root path is tracked: {normalized}")
        if any(normalized.startswith(prefix) for prefix in FORBIDDEN_PUBLIC_PREFIXES):
            add_error(errors, f"unsupported mobile asset is tracked: {normalized}")
        if any(normalized.startswith(prefix) for prefix in FORBIDDEN_LOCAL_ONLY_PREFIXES):
            add_error(errors, f"local-only diagnostic or upstream test is tracked: {normalized}")
        forbidden_directory_names = {
            "diagnostics",
            "diagnostic",
            "logs",
            "tmp",
            ".tmp",
        }
        if any(part.lower() in forbidden_directory_names for part in path.parts):
            add_error(errors, f"local diagnostic directory is tracked: {normalized}")
        lower = normalized.lower()
        if any(lower.endswith(suffix) for suffix in FORBIDDEN_SUFFIXES):
            add_error(errors, f"diagnostic or source-map artifact is tracked: {normalized}")


def read_json(root: Path, relative: str, errors: list[str]) -> dict[str, Any] | None:
    try:
        value = json.loads((root / relative).read_text(encoding="utf-8"))
    except (OSError, UnicodeError, json.JSONDecodeError) as error:
        add_error(errors, f"{relative}: invalid JSON ({error})")
        return None
    if not isinstance(value, dict):
        add_error(errors, f"{relative}: expected a JSON object")
        return None
    return value


def check_sources(root: Path, errors: list[str]) -> None:
    relative = "后端/crates/haven-application/resources/builtin-sources.json"
    manifest = read_json(root, relative, errors)
    if manifest is None:
        return
    if manifest.get("schemaVersion") != 1:
        add_error(errors, f"{relative}: schemaVersion must be 1")

    categories = manifest.get("categories")
    category_ids = (
        {
            item.get("id")
            for item in categories
            if isinstance(item, dict) and isinstance(item.get("id"), str)
        }
        if isinstance(categories, list)
        else set()
    )
    if category_ids != SOURCE_CATEGORIES:
        add_error(errors, f"{relative}: categories must be exactly {sorted(SOURCE_CATEGORIES)}")

    sources = manifest.get("sources")
    if not isinstance(sources, list) or not sources:
        add_error(errors, f"{relative}: sources must be a non-empty array")
        return

    ids: list[str] = []
    counts = {category: 0 for category in SOURCE_CATEGORIES}
    implementation_files = [
        root / "后端/crates/haven-infrastructure/src/metadata_sources.rs",
        root / "后端/crates/haven-infrastructure/src/opds.rs",
        root / "后端/crates/haven-infrastructure/src/cms10.rs",
    ]
    implementations = "\n".join(
        file.read_text(encoding="utf-8") for file in implementation_files if file.exists()
    )

    for index, source in enumerate(sources):
        if not isinstance(source, dict):
            add_error(errors, f"{relative}: source #{index + 1} must be an object")
            continue
        source_id = source.get("sourceId")
        if not isinstance(source_id, str) or not source_id:
            add_error(errors, f"{relative}: source #{index + 1} has no sourceId")
            continue
        ids.append(source_id)
        source_categories = source.get("categories")
        if not isinstance(source_categories, list) or not source_categories:
            add_error(errors, f"{relative}: {source_id} must declare categories")
        else:
            for category in source_categories:
                if category not in SOURCE_CATEGORIES:
                    add_error(errors, f"{relative}: {source_id} has unknown category {category!r}")
                else:
                    counts[category] += 1
        if source.get("mode") not in {"single", "collection"}:
            add_error(errors, f"{relative}: {source_id} has invalid mode")
        kinds = source.get("kinds")
        if not isinstance(kinds, list) or not kinds or any(kind not in SOURCE_KINDS for kind in kinds):
            add_error(errors, f"{relative}: {source_id} has invalid kinds")
        notes = source.get("notes")
        if not isinstance(notes, str) or not notes.strip():
            add_error(errors, f"{relative}: {source_id} needs a user-facing notes field")
        elif any(word.lower() in notes.lower() for word in FORBIDDEN_STATUS_WORDS):
            add_error(errors, f"{relative}: {source_id} contains a pending/unimplemented status")
        source_marker = re.compile(rf"[\"']{re.escape(source_id)}[\"']")
        if source_marker.search(implementations) is None:
            add_error(errors, f"{relative}: {source_id} has no checked-in provider implementation marker")

    if len(ids) != len(set(ids)):
        add_error(errors, f"{relative}: sourceId values must be unique")
    for category, count in sorted(counts.items()):
        if count < 3:
            add_error(errors, f"{relative}: category {category} has only {count} sources; expected at least 3")


def check_generated_bindings(root: Path, errors: list[str]) -> None:
    wire_path = root / "前端/app/src/lib/ipc/generated/wire.ts"
    dto_path = root / "后端/crates/haven-application/src/wire/dto.rs"
    bindings_dir = root / "后端/crates/haven-application/bindings"
    try:
        wire = wire_path.read_text(encoding="utf-8")
        dto = dto_path.read_text(encoding="utf-8")
    except (OSError, UnicodeError) as error:
        add_error(errors, f"generated binding inputs cannot be read: {error}")
        return
    if "Generated by" not in wire or "Do not edit" not in wire:
        add_error(errors, "generated wire.ts is missing its generated-file guard")

    rust_names = set(re.findall(r"^pub\s+(?:struct|enum|type)\s+([A-Za-z0-9_]+)", dto, re.MULTILINE))
    binding_names = {path.stem for path in bindings_dir.glob("*.ts")} if bindings_dir.exists() else set()
    wire_names = set(re.findall(r"^export\s+(?:type|interface)\s+([A-Za-z0-9_]+)", wire, re.MULTILINE))
    if rust_names != binding_names:
        add_error(errors, "Rust DTO names and individual TypeScript bindings differ")
    if binding_names != wire_names:
        add_error(errors, "individual TypeScript bindings and generated wire.ts exports differ")


def main() -> int:
    root = Path(__file__).resolve().parents[2]
    errors: list[str] = []
    try:
        files = tracked_files(root)
    except (OSError, subprocess.CalledProcessError) as error:
        print(f"public-snapshot-check: unable to inspect Git tree: {error}", file=sys.stderr)
        return 2
    check_public_tree(root, files, errors)
    check_sources(root, errors)
    check_generated_bindings(root, errors)
    if errors:
        for error in errors:
            print(f"public-snapshot-check: {error}", file=sys.stderr)
        return 1
    print(f"public-snapshot-check: OK ({len(files)} tracked public files)")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
