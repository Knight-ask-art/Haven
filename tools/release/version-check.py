from __future__ import annotations

import argparse
import json
import sys
import tomllib
from pathlib import Path
from typing import Any


FIRST_PARTY_PACKAGE_NAMES = {
    "haven-application",
    "haven-common",
    "haven-domain",
    "haven-infrastructure",
    "haven-tauri",
}


def _read_json(root: Path, relative_path: str) -> tuple[dict[str, Any] | None, str | None]:
    try:
        value = json.loads((root / relative_path).read_text(encoding="utf-8"))
    except (OSError, UnicodeError, json.JSONDecodeError) as error:
        return None, f"{relative_path}: unable to read JSON ({error})"
    if not isinstance(value, dict):
        return None, f"{relative_path}: expected a JSON object"
    return value, None


def _read_toml(root: Path, relative_path: str) -> tuple[dict[str, Any] | None, str | None]:
    try:
        value = tomllib.loads((root / relative_path).read_text(encoding="utf-8"))
    except (OSError, UnicodeError, tomllib.TOMLDecodeError) as error:
        return None, f"{relative_path}: unable to read TOML ({error})"
    return value, None


def _record(errors: list[str], relative_path: str, actual: Any, expected: str) -> None:
    if actual != expected:
        errors.append(
            f"{relative_path}: expected first-party version {expected!r}, got {actual!r}"
        )


def _package_version(manifest: dict[str, Any], relative_path: str) -> Any:
    package = manifest.get("package")
    if not isinstance(package, dict):
        return None
    version = package.get("version")
    if isinstance(version, dict) and version.get("workspace") is True:
        return "workspace"
    return version


def _check_cargo_manifests(root: Path, expected: str, errors: list[str]) -> None:
    manifest_paths = [Path("src-tauri/Cargo.toml"), Path("后端/Cargo.toml")]
    manifest_paths.extend(
        path.relative_to(root)
        for path in sorted((root / "后端/crates").glob("*/Cargo.toml"))
    )

    workspace_version: str | None = None
    for path in manifest_paths:
        relative_path = path.as_posix()
        manifest, error = _read_toml(root, relative_path)
        if error:
            errors.append(error)
            continue

        if relative_path == "后端/Cargo.toml":
            workspace = manifest.get("workspace")
            workspace_package = workspace.get("package") if isinstance(workspace, dict) else None
            actual = workspace_package.get("version") if isinstance(workspace_package, dict) else None
            _record(errors, relative_path, actual, expected)
            if isinstance(actual, str):
                workspace_version = actual
            continue

        actual = _package_version(manifest, relative_path)
        if actual == "workspace":
            if workspace_version != expected:
                errors.append(
                    f"{relative_path}: version.workspace=true but workspace version is "
                    f"{workspace_version!r}, expected {expected!r}"
                )
        else:
            _record(errors, relative_path, actual, expected)

        dependencies = manifest.get("dependencies")
        if not isinstance(dependencies, dict):
            continue
        for dependency_name, dependency in dependencies.items():
            if not dependency_name.startswith("haven-") or not isinstance(dependency, dict):
                continue
            if "path" in dependency and "version" in dependency:
                _record(
                    errors,
                    f"{relative_path} dependency {dependency_name}",
                    dependency["version"],
                    expected,
                )


def _check_cargo_lock(root: Path, expected: str, errors: list[str]) -> None:
    lock_package_names = {
        "后端/Cargo.lock": FIRST_PARTY_PACKAGE_NAMES - {"haven-tauri"},
        "src-tauri/Cargo.lock": FIRST_PARTY_PACKAGE_NAMES,
    }
    for relative_path, expected_names in lock_package_names.items():
        lock_path = root / relative_path
        if not lock_path.exists():
            continue
        lock, error = _read_toml(root, relative_path)
        if error:
            errors.append(error)
            continue
        packages = lock.get("package") if lock is not None else None
        if not isinstance(packages, list):
            errors.append(f"{relative_path}: package entries are missing")
            continue
        seen: set[str] = set()
        for package in packages:
            if not isinstance(package, dict):
                continue
            name = package.get("name")
            if name not in expected_names:
                continue
            seen.add(name)
            _record(
                errors,
                f"{relative_path} package {name}",
                package.get("version"),
                expected,
            )
        missing = expected_names - seen
        for name in sorted(missing):
            errors.append(f"{relative_path}: first-party package {name!r} is missing")


def _check_tauri(root: Path, expected: str, errors: list[str]) -> None:
    config, error = _read_json(root, "src-tauri/tauri.conf.json")
    if error:
        errors.append(error)
    elif config is not None:
        _record(errors, "src-tauri/tauri.conf.json version", config.get("version"), expected)


def _check_frontend(root: Path, expected: str, errors: list[str]) -> None:
    package, error = _read_json(root, "前端/app/package.json")
    if error:
        errors.append(error)
    elif package is not None:
        _record(errors, "前端/app/package.json version", package.get("version"), expected)

    lock, error = _read_json(root, "前端/app/package-lock.json")
    if error:
        errors.append(error)
    elif lock is not None:
        _record(errors, "前端/app/package-lock.json version", lock.get("version"), expected)
        packages = lock.get("packages")
        root_package = packages.get("") if isinstance(packages, dict) else None
        root_version = root_package.get("version") if isinstance(root_package, dict) else None
        _record(errors, "前端/app/package-lock.json root version", root_version, expected)


def validate(repository_root: Path, expected_version: str) -> list[str]:
    errors: list[str] = []
    _check_frontend(repository_root, expected_version, errors)
    _check_tauri(repository_root, expected_version, errors)
    _check_cargo_manifests(repository_root, expected_version, errors)
    _check_cargo_lock(repository_root, expected_version, errors)
    return errors


def main(arguments: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description="Check first-party release versions")
    parser.add_argument("--version", default="0.1.0")
    parser.add_argument(
        "--root",
        type=Path,
        default=Path(__file__).resolve().parents[2],
        help="repository root (defaults to the repository containing this script)",
    )
    options = parser.parse_args(arguments)
    errors = validate(options.root.resolve(), options.version)
    if errors:
        for error in errors:
            print(f"version-check: {error}", file=sys.stderr)
        return 1
    print(f"version-check: first-party manifests agree on {options.version}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
