from __future__ import annotations

import argparse
import json
import re
import sys
from dataclasses import dataclass
from pathlib import Path


# Release jobs must be driven by an explicit public tag. A branch name or a
# bare version must never become a published GitHub Release tag.
_REF_PATTERN = re.compile(r"^v(?P<version>\d+\.\d+\.\d+(?:-[0-9A-Za-z.]+)?)$")
_PRERELEASE_PATTERN = re.compile(
    r"^(?P<base>\d+\.\d+\.\d+)-(?P<channel>[0-9A-Za-z]+)\.(?P<number>\d+)$"
)

MAX_MSI_PRERELEASE_NUMBER = 65535


class ReleaseVersionError(ValueError):
    """Raised when a public release ref cannot become an MSI version."""


@dataclass(frozen=True)
class ReleaseVersion:
    release_ref: str
    release_version: str
    bundle_version: str
    prerelease: bool

    def outputs(self) -> dict[str, str]:
        return {
            "release_ref": self.release_ref,
            "release_version": self.release_version,
            "bundle_version": self.bundle_version,
            "prerelease": "true" if self.prerelease else "false",
        }


def to_bundle_version(release_version: str) -> str:
    """Map a public SemVer version to the Windows MSI bundle version.

    Windows MSI requires the optional prerelease identifier to be numeric. The
    current release contract therefore keeps the public channel in the Git
    tag, but maps its final numeric component only:
    ``0.1.0-beta.1`` -> ``0.1.0-1``.
    """

    if "-" not in release_version:
        return release_version

    match = _PRERELEASE_PATTERN.fullmatch(release_version)
    if match is None:
        raise ReleaseVersionError(
            "MSI prerelease versions must end with a numeric identifier "
            f"(for example 0.1.0-beta.1); got {release_version}"
        )

    number = int(match.group("number"))
    if number > MAX_MSI_PRERELEASE_NUMBER:
        raise ReleaseVersionError(
            f"MSI prerelease number must be <= {MAX_MSI_PRERELEASE_NUMBER}; got {number}"
        )

    return f"{match.group('base')}-{number}"


def resolve(release_ref: str) -> ReleaseVersion:
    """Resolve an explicit ``v<semver>`` release tag."""

    match = _REF_PATTERN.fullmatch(release_ref)
    if match is None:
        raise ReleaseVersionError(
            "release ref must be an explicit v<semver> tag "
            f"(for example v0.1.0-beta.1); got {release_ref!r}"
        )

    release_version = match.group("version")
    return ReleaseVersion(
        release_ref=release_ref,
        release_version=release_version,
        bundle_version=to_bundle_version(release_version),
        prerelease="-" in release_version,
    )


def main(arguments: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(
        description="Resolve a public release tag to Windows MSI bundle versions"
    )
    parser.add_argument(
        "--ref", required=True, help="public release tag, e.g. v0.1.0-beta.1"
    )
    parser.add_argument("--github-output", type=Path, default=None)
    options = parser.parse_args(arguments)

    try:
        resolved = resolve(options.ref)
    except ReleaseVersionError as error:
        print(f"bundle-version: {error}", file=sys.stderr)
        return 1

    outputs = resolved.outputs()
    if options.github_output is not None:
        with options.github_output.open("a", encoding="utf-8") as handle:
            for key, value in outputs.items():
                handle.write(f"{key}={value}\n")
    print(json.dumps(outputs, ensure_ascii=False, sort_keys=True))
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
