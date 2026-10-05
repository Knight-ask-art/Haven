"""Produce the public Windows updater feed for a Haven release.

The published ``latest.json`` must describe an artifact that the installed
client can actually verify: one immutable GitHub release download URL for the
compiled NSIS ``-setup.exe`` and the signature sidecar emitted next to it by
the pinned bundler.  This producer is driven by those two local files, so the
release workflow never has to guess what an upstream action wrote.

Tag/version coherence is delegated to :mod:`bundle-version`, the canonical
resolver already used by the release workflow, instead of restating a SemVer
policy here.

Boundary: the module reads only the installer and signature paths it is given,
writes only the requested output (atomically, UTF-8) and embeds no keys.  It
never opens a network connection, runs a shell, launches the installer or
touches any other path.
"""
from __future__ import annotations

import argparse
import base64
import binascii
import importlib.util
import json
import os
import re
import stat
import sys
import tempfile
from dataclasses import dataclass
from datetime import datetime, timezone
from pathlib import Path
from typing import Any
from urllib.parse import quote


# Console output stays ASCII so a Windows console with a legacy code page can
# never turn a release failure into a UnicodeEncodeError.
PREFIX = "make-updater-manifest"

# Keep the artefact floor identical to the checker that validates the upload:
# a real NSIS installer is far larger, and a truncated download is not signed
# by anything.
MINIMUM_INSTALLER_BYTES = 1024
NSIS_SUFFIX = "-setup.exe"

PUBLIC_RELEASE_HOST = "https://github.com"

# Tauri resolves a different target key depending on how the application was
# installed, so the feed must answer both with the same signed NSIS artifact.
WINDOWS_TARGETS = ("windows-x86_64", "windows-x86_64-nsis")

BUNDLE_VERSION_FILE = "bundle-version.py"

_REPOSITORY_PATTERN = re.compile(
    r"[A-Za-z0-9](?:[A-Za-z0-9._-]*[A-Za-z0-9])?/[A-Za-z0-9](?:[A-Za-z0-9._-]*[A-Za-z0-9])?"
)
_BASE64_PATTERN = re.compile(r"[A-Za-z0-9+/]+={0,2}")

# The sidecar is base64 of the minisign signature box the client decodes and
# verifies, so the decoded payload must be that box and not arbitrary text.
_MINISIGN_PREFIX = b"untrusted comment:"
_SIGNATURE_FORMAT_ERROR = "signature sidecar is not a base64 minisign signature"

_bundle_version_module: Any = None


class UpdaterFeedError(ValueError):
    """Raised when local artifacts cannot become a publishable updater feed.

    Messages are deliberately free of caller supplied values: they are printed
    on consoles and in CI logs whose encoding is not under our control.
    """


@dataclass(frozen=True)
class UpdaterFeed:
    version: str
    repository: str
    tag: str
    installer_name: str
    signature: str
    notes: str
    pub_date: str

    def artifact_url(self) -> str:
        """Return the immutable public download URL for the installer."""

        encoded = quote(self.installer_name, safe="")
        return (
            f"{PUBLIC_RELEASE_HOST}/{self.repository}/releases/download/"
            f"{self.tag}/{encoded}"
        )

    def payload(self) -> dict[str, Any]:
        artifact = {"signature": self.signature, "url": self.artifact_url()}
        return {
            "notes": self.notes,
            "platforms": {target: dict(artifact) for target in WINDOWS_TARGETS},
            "pub_date": self.pub_date,
            "version": self.version,
        }


def _load_bundle_version() -> Any:
    """Load the canonical release version resolver next to this module."""

    global _bundle_version_module
    if _bundle_version_module is not None:
        return _bundle_version_module

    module_path = Path(__file__).with_name(BUNDLE_VERSION_FILE)
    spec = importlib.util.spec_from_file_location(
        "haven_bundle_version", module_path
    )
    if spec is None or spec.loader is None:
        raise UpdaterFeedError("cannot load the canonical release version resolver")
    module = importlib.util.module_from_spec(spec)
    sys.modules.setdefault(spec.name, module)
    spec.loader.exec_module(module)
    _bundle_version_module = module
    return module


def _resolve_tag(tag: str) -> Any:
    module = _load_bundle_version()
    try:
        return module.resolve(tag)
    except module.ReleaseVersionError as error:
        raise UpdaterFeedError(
            "release tag is not an explicit v<semver> tag"
        ) from error


def _is_junction(path: Path) -> bool:
    checker = getattr(os.path, "isjunction", None)
    if checker is None:
        return False
    try:
        return bool(checker(path))
    except OSError:
        return True


def _regular_file(path: Path) -> bool:
    """Report whether ``path`` is an existing regular file that is not a link."""

    try:
        info = path.lstat()
    except OSError:
        return False
    if not stat.S_ISREG(info.st_mode):
        return False
    return not _is_junction(path)


def _read_signature(path: Path) -> str:
    if not _regular_file(path):
        raise UpdaterFeedError(
            "signature sidecar is missing or not a regular file"
        )
    try:
        raw = path.read_bytes()
    except OSError as error:
        raise UpdaterFeedError("signature sidecar is unreadable") from error
    try:
        text = raw.decode("utf-8").strip()
    except UnicodeDecodeError as error:
        raise UpdaterFeedError("signature sidecar is not UTF-8 text") from error
    if not text:
        raise UpdaterFeedError("signature sidecar is empty")
    if _BASE64_PATTERN.fullmatch(text) is None:
        raise UpdaterFeedError(_SIGNATURE_FORMAT_ERROR)
    try:
        box = base64.b64decode(text, validate=True)
    except (binascii.Error, ValueError) as error:
        raise UpdaterFeedError(_SIGNATURE_FORMAT_ERROR) from error
    if not box.startswith(_MINISIGN_PREFIX):
        raise UpdaterFeedError(_SIGNATURE_FORMAT_ERROR)
    # Only surrounding whitespace is dropped; the text is otherwise preserved
    # verbatim because the client decodes exactly this string.
    return text


def _require_installer(path: Path) -> None:
    if not _regular_file(path):
        raise UpdaterFeedError(
            "installer is missing or not a regular compiled setup file"
        )
    try:
        size = path.stat().st_size
        with path.open("rb") as stream:
            magic = stream.read(2)
    except OSError as error:
        raise UpdaterFeedError("installer is unreadable") from error
    if size < MINIMUM_INSTALLER_BYTES or magic != b"MZ":
        raise UpdaterFeedError("installer is not a compiled Windows setup executable")


def _utc_now() -> str:
    return datetime.now(timezone.utc).strftime("%Y-%m-%dT%H:%M:%SZ")


def produce(
    installer: Path,
    signature: Path,
    version: str,
    repository: str,
    tag: str,
    notes: str = "",
) -> UpdaterFeed:
    """Validate local release artifacts and describe the updater feed.

    ``version`` must be the canonical Windows bundle version the tag resolves
    to, because that is the version the installed application reports and the
    value the release workflow hands to the updater manifest checker.
    """

    resolved = _resolve_tag(tag)
    if version != resolved.bundle_version:
        raise UpdaterFeedError(
            "version must be the canonical Windows bundle version of the tag"
        )
    if _REPOSITORY_PATTERN.fullmatch(repository) is None:
        raise UpdaterFeedError("repository must be an OWNER/REPO pair")
    if not isinstance(notes, str):
        raise UpdaterFeedError("release notes must be plain text")

    installer = Path(installer)
    signature = Path(signature)
    name = installer.name
    if name in {"", ".", ".."} or not name.endswith(NSIS_SUFFIX):
        raise UpdaterFeedError(
            "installer must be the NSIS -setup.exe produced by the Windows bundle"
        )
    if "/" in name or "\\" in name:
        raise UpdaterFeedError("installer name must be a plain file name")
    if signature.name != f"{name}.sig":
        raise UpdaterFeedError("signature sidecar must be named <installer>.sig")

    _require_installer(installer)
    signature_text = _read_signature(signature)

    return UpdaterFeed(
        version=version,
        repository=repository,
        tag=tag,
        installer_name=name,
        signature=signature_text,
        notes=notes,
        pub_date=_utc_now(),
    )


def write_manifest(output: Path, payload: dict[str, Any]) -> None:
    """Write ``payload`` as UTF-8 JSON to ``output`` and to nothing else."""

    output = Path(output)
    directory = output.parent
    if not directory.is_dir():
        raise UpdaterFeedError("output directory does not exist")
    if output.is_symlink() or _is_junction(output):
        raise UpdaterFeedError("output must not be a link")
    if output.is_dir():
        raise UpdaterFeedError("output must be a file path")

    data = (
        json.dumps(payload, ensure_ascii=False, indent=2, sort_keys=True) + "\n"
    ).encode("utf-8")

    descriptor, temporary_name = tempfile.mkstemp(
        dir=directory, prefix=".updater-manifest-", suffix=".tmp"
    )
    temporary = Path(temporary_name)
    try:
        with os.fdopen(descriptor, "wb") as stream:
            stream.write(data)
            stream.flush()
            os.fsync(stream.fileno())
        os.replace(temporary, output)
    except BaseException:
        temporary.unlink(missing_ok=True)
        raise


def main(arguments: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(
        description="Produce the Windows updater manifest from a signed NSIS installer"
    )
    parser.add_argument(
        "--installer", type=Path, required=True, help="compiled NSIS -setup.exe"
    )
    parser.add_argument(
        "--signature", type=Path, required=True, help="matching .sig sidecar"
    )
    parser.add_argument(
        "--version", required=True, help="canonical Windows bundle version"
    )
    parser.add_argument("--repository", required=True, help="OWNER/REPO")
    parser.add_argument("--tag", required=True, help="public release tag, e.g. v0.1.0")
    parser.add_argument("--output", type=Path, required=True, help="latest.json")
    parser.add_argument("--notes", default="", help="release notes as plain text")
    options = parser.parse_args(arguments)

    try:
        feed = produce(
            installer=options.installer,
            signature=options.signature,
            version=options.version,
            repository=options.repository,
            tag=options.tag,
            notes=options.notes,
        )
        write_manifest(options.output, feed.payload())
    except UpdaterFeedError as error:
        print(f"{PREFIX}: {error}", file=sys.stderr)
        return 1
    except OSError:
        print(f"{PREFIX}: unable to write the updater manifest", file=sys.stderr)
        return 1

    print(f"{PREFIX}: wrote the Windows updater manifest for {feed.version}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
