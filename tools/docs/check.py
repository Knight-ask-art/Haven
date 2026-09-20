"""Validate the curated public documentation surface.

This checker is deliberately small and dependency-free so it can run in local
Vibe Coding sessions and in the Public CI Windows runner. It does not mutate
files or decide whether a historical review is correct.
"""

from __future__ import annotations

import re
import sys
from dataclasses import dataclass
from datetime import date
from pathlib import Path


ROOT = Path(__file__).resolve().parents[2]
DOCS = ROOT / "docs"
SKIP_DIRS = {
    "internal",
    "private",
    "reviews",
    "plans",
    "superpowers",
    "drafts",
    "project",
    "tmp",
    ".tmp",
}
PUBLIC_INDEXES = {
    Path("plans/README.md"),
    Path("reviews/README.md"),
}
GRANDFATHERED_PUBLIC_DOCUMENTS = {
    Path("superpowers/specs/2026-09-10-comic-reading-center-design.md"),
}
LOCAL_RECORD_REGISTERS = (
    (DOCS / "plans", DOCS / "plans" / "README.md", False, frozenset({"README.md"})),
    (DOCS / "reviews", DOCS / "reviews" / "README.md", False, frozenset({"README.md"})),
    (DOCS / "superpowers", DOCS / "engineering" / "documentation-migration.md", True, frozenset()),
)
MAX_PUBLIC_DOCUMENT_BYTES = 64 * 1024
REQUIRED_FIELDS = {
    "doc_id",
    "type",
    "status",
    "owner",
    "visibility",
    "source_of_truth",
    "last_reviewed",
    "review_after",
}
ALLOWED_TYPES = {"canonical", "guide", "decision", "reference", "generated", "historical"}
ALLOWED_STATUSES = {"draft", "active", "deprecated", "superseded", "archived"}
ALLOWED_VISIBILITY = {"public", "internal"}
LINK_RE = re.compile(r"\[[^\]]*\]\(([^)]+)\)")
URI_WITH_AUTHORITY_RE = re.compile(r"^[A-Za-z][A-Za-z0-9+.-]*://")
ABSOLUTE_PATH_RE = re.compile(
    r"(?:(?<![A-Za-z0-9+.-])[A-Za-z]:[\\/]|file://)",
    re.IGNORECASE,
)
PRIVATE_PATH_RE = re.compile(
    r"(?:\\\\|(?i:(?:^|[\s(`])/(?:Users|home|mnt|tmp|var)/)|"
    r"(?i:(?:^|[\s(`/\\])(?:\.claude|\.codex|\.fastctx)[/\\]))"
)
SIGNED_URL_RE = re.compile(
    r"(?i)(?:[?&](?:token|signature|sig|x-amz-signature|expires|expires_at)="
    r"|authorization\s*:\s*bearer\s+)"
)
SECRET_MARKER_RE = re.compile(
    r"-----BEGIN (?:RSA|EC|OPENSSH|DSA|PRIVATE) KEY-----|"
    r"(?:ghp|gho|ghs|ghu|ghr)_[A-Za-z0-9]{20,}|"
    r"github_pat_[A-Za-z0-9_]{20,}|AKIA[0-9A-Z]{16}"
)
COOKIE_HEADER_RE = re.compile(r"(?im)^\s*(?:cookie|set-cookie)\s*:")
PRIVATE_DIRECTORY_NAMES = {
    ".superpowers",
    ".tmp",
    "diagnostic",
    "diagnostics",
    "logs",
    "tmp",
}
PRIVATE_ROOT_PATHS = {
    "design-qa.md",
    "tools/readme.md",
    "后端/readme.md",
    "前端/app/update_rules.py",
    "前端/前端开发要求.txt",
    "前端/前端开发要点.txt",
}
PRIVATE_ROOT_PREFIXES = {
    "plan",
    "参考项目",
    "前端/设计原则",
    "前端/tokens-setup",
    "测试",
}


@dataclass(frozen=True)
class Document:
    path: Path
    fields: dict[str, str]
    body: str


def public_documents() -> list[Path]:
    if not DOCS.exists():
        return []
    return sorted(
        path
        for path in DOCS.rglob("*.md")
        if path.relative_to(DOCS) in PUBLIC_INDEXES
        or path.relative_to(DOCS).parts[0] not in SKIP_DIRS
    )


def grandfathered_public_documents() -> list[Path]:
    return sorted(DOCS / relative for relative in GRANDFATHERED_PUBLIC_DOCUMENTS)


def parse_document(path: Path) -> tuple[Document | None, list[str]]:
    lines = path.read_text(encoding="utf-8").splitlines()
    errors: list[str] = []
    if not lines or lines[0].strip() != "---":
        errors.append(f"{path.relative_to(ROOT)}: missing YAML front matter")
        return None, errors

    try:
        end = next(index for index, line in enumerate(lines[1:], start=1) if line.strip() == "---")
    except StopIteration:
        errors.append(f"{path.relative_to(ROOT)}: unterminated YAML front matter")
        return None, errors

    fields: dict[str, str] = {}
    for line in lines[1:end]:
        if not line.strip() or line.lstrip().startswith("#"):
            continue
        if ":" not in line:
            errors.append(f"{path.relative_to(ROOT)}: invalid metadata line: {line}")
            continue
        key, value = line.split(":", 1)
        normalized_key = key.strip()
        if normalized_key in fields:
            errors.append(
                f"{path.relative_to(ROOT)}: duplicate metadata field: {normalized_key}"
            )
        fields[normalized_key] = value.strip().strip("\"'")

    missing = REQUIRED_FIELDS - fields.keys()
    if missing:
        errors.append(f"{path.relative_to(ROOT)}: missing metadata: {', '.join(sorted(missing))}")
    empty = sorted(field for field in REQUIRED_FIELDS & fields.keys() if not fields[field])
    if empty:
        errors.append(f"{path.relative_to(ROOT)}: empty metadata: {', '.join(empty)}")
    if fields.get("type") not in ALLOWED_TYPES:
        errors.append(f"{path.relative_to(ROOT)}: invalid type: {fields.get('type', '<missing>')}")
    if fields.get("status") not in ALLOWED_STATUSES:
        errors.append(f"{path.relative_to(ROOT)}: invalid status: {fields.get('status', '<missing>')}")
    if fields.get("visibility") not in ALLOWED_VISIBILITY:
        errors.append(
            f"{path.relative_to(ROOT)}: invalid visibility: {fields.get('visibility', '<missing>')}"
        )

    for field in ("last_reviewed", "review_after"):
        value = fields.get(field)
        if value is None:
            continue
        try:
            date.fromisoformat(value)
        except ValueError:
            errors.append(f"{path.relative_to(ROOT)}: {field} must be YYYY-MM-DD: {value}")

    if fields.get("type") == "generated":
        for field in ("generated_from", "generated_by"):
            if not fields.get(field):
                errors.append(f"{path.relative_to(ROOT)}: generated document missing {field}")

    return Document(path, fields, "\n".join(lines[end + 1 :])), errors


def is_private_repository_target(candidate: Path) -> bool:
    relative = candidate.relative_to(ROOT)
    if relative.name.casefold() == "agents.md":
        return True
    normalized = relative.as_posix().casefold()
    if any(part.casefold() in PRIVATE_DIRECTORY_NAMES for part in relative.parts):
        return True
    if normalized in PRIVATE_ROOT_PATHS:
        return True
    if any(
        normalized == prefix or normalized.startswith(f"{prefix}/")
        for prefix in PRIVATE_ROOT_PREFIXES
    ):
        return True
    if not relative.parts or relative.parts[0].casefold() != "docs":
        return False

    docs_relative = Path(*relative.parts[1:])
    if docs_relative in PUBLIC_INDEXES or docs_relative in GRANDFATHERED_PUBLIC_DOCUMENTS:
        return False
    return bool(docs_relative.parts and docs_relative.parts[0].casefold() in SKIP_DIRS)


def check_links(document: Document) -> list[str]:
    errors: list[str] = []
    for raw_target in LINK_RE.findall(document.body):
        target = raw_target.strip().split("#", 1)[0]
        if (
            not target
            or URI_WITH_AUTHORITY_RE.match(target)
            or target.startswith(("mailto:", "codex:"))
        ):
            continue
        candidate = (document.path.parent / target).resolve()
        if ROOT not in candidate.parents and candidate != ROOT:
            errors.append(
                f"{document.path.relative_to(ROOT)}: link escapes repository: {raw_target}"
            )
            continue
        if is_private_repository_target(candidate):
            errors.append(
                f"{document.path.relative_to(ROOT)}: public link targets a private path: "
                f"{raw_target}"
            )
            continue
        if not candidate.exists():
            errors.append(
                f"{document.path.relative_to(ROOT)}: broken link target: {raw_target}"
            )
    return errors


def check_body(document: Document) -> list[str]:
    errors: list[str] = []
    relative = document.path.relative_to(ROOT)
    try:
        size = document.path.stat().st_size
    except OSError as error:
        errors.append(f"{relative}: cannot stat document: {error}")
        return errors
    if size > MAX_PUBLIC_DOCUMENT_BYTES:
        errors.append(
            f"{relative}: public document exceeds {MAX_PUBLIC_DOCUMENT_BYTES} bytes"
        )
    if document.fields.get("visibility") == "public":
        if ABSOLUTE_PATH_RE.search(document.body):
            errors.append(f"{relative}: public document contains an absolute path or file URI")
        if PRIVATE_PATH_RE.search(document.body):
            errors.append(f"{relative}: public document contains a private runtime path")
        if SIGNED_URL_RE.search(document.body):
            errors.append(f"{relative}: public document contains a signed URL or bearer token")
        if SECRET_MARKER_RE.search(document.body):
            errors.append(f"{relative}: public document contains a credential marker")
        if COOKIE_HEADER_RE.search(document.body):
            errors.append(f"{relative}: public document contains a raw cookie header")
    return errors


def check_surface(document: Document) -> list[str]:
    if document.fields.get("visibility") == "public":
        return []
    return [
        f"{document.path.relative_to(ROOT)}: curated public surface must declare visibility: public"
    ]


def check_freshness(document: Document, *, today: date | None = None) -> list[str]:
    if document.fields.get("status") != "active":
        return []
    review_after = document.fields.get("review_after")
    if review_after is None:
        return []
    try:
        deadline = date.fromisoformat(review_after)
    except ValueError:
        return []
    current = today or date.today()
    if deadline >= current:
        return []
    return [
        f"STALE_REVIEW: {document.path.relative_to(ROOT)} was due {review_after}"
    ]


def check_record_register(
    directory: Path,
    register: Path,
    *,
    recursive: bool,
    excluded_names: frozenset[str],
) -> list[str]:
    """Require local-only records to be named in their public lifecycle register."""

    if not directory.exists():
        return []
    try:
        register_text = register.read_text(encoding="utf-8")
    except (OSError, UnicodeError) as error:
        return [f"{register.relative_to(ROOT)}: cannot read local record register ({error})"]

    candidates = directory.rglob("*.md") if recursive else directory.glob("*.md")
    errors: list[str] = []
    for path in sorted(candidates):
        if path.name in excluded_names:
            continue
        relative = path.relative_to(ROOT).as_posix()
        if f"`{relative}`" not in register_text:
            errors.append(
                f"{relative}: local record is missing from "
                f"{register.relative_to(ROOT).as_posix()}"
            )
    return errors


def check_plan_register(
    plan_directory: Path = ROOT / "plan",
    register: Path = DOCS / "engineering" / "documentation-migration.md",
) -> list[str]:
    """Compare the local plan corpus with the committed root-plan register."""

    if not plan_directory.exists():
        return []
    try:
        register_text = register.read_text(encoding="utf-8")
    except (OSError, UnicodeError) as error:
        return [f"{register.relative_to(ROOT)}: cannot read plan register ({error})"]

    plan_prefix = plan_directory.relative_to(ROOT).as_posix()
    register_pattern = re.compile(
        rf"(?m)^\| `({re.escape(plan_prefix)}/[^`/]+\.md)` \|"
    )
    actual = {
        path.relative_to(ROOT).as_posix()
        for path in plan_directory.glob("*.md")
    }
    registered = set(register_pattern.findall(register_text))
    errors = [
        f"{path}: local plan is missing from {register.relative_to(ROOT).as_posix()}"
        for path in sorted(actual - registered)
    ]
    errors.extend(
        f"{path}: plan register entry has no local file"
        for path in sorted(registered - actual)
    )
    return errors


def check_local_record_registers() -> list[str]:
    errors = check_plan_register()
    for directory, register, recursive, excluded_names in LOCAL_RECORD_REGISTERS:
        errors.extend(
            check_record_register(
                directory,
                register,
                recursive=recursive,
                excluded_names=excluded_names,
            )
        )
    return errors


def check_ledger_policy(documents: list[Document]) -> tuple[list[str], list[str]]:
    errors: list[str] = []
    warnings: list[str] = []
    work_indexes = [
        document for document in documents if document.fields.get("doc_id") == "work.index"
    ]
    active_ledgers = [
        document
        for document in documents
        if document.fields.get("doc_id") == "work.status"
        and document.fields.get("status") == "active"
    ]

    if len(work_indexes) != 1:
        return ["exactly one work.index document must declare the active ledger"], warnings

    work_index = work_indexes[0]
    relative_index = work_index.path.relative_to(ROOT)
    declared = work_index.fields.get("active_ledger")
    cutover = work_index.fields.get("ledger_cutover")
    state = work_index.fields.get("ledger_state")
    as_of = work_index.fields.get("ledger_as_of")

    if cutover == "pending":
        if declared != "plan/IMPLEMENTATION_STATUS.md":
            errors.append(
                f"{relative_index}: pending cutover must declare "
                "plan/IMPLEMENTATION_STATUS.md as active_ledger"
            )
        if active_ledgers:
            errors.append(
                f"{relative_index}: pending cutover forbids an active public work.status ledger"
            )
    elif cutover == "complete":
        if len(active_ledgers) != 1:
            errors.append(
                f"{relative_index}: completed cutover requires exactly one active work.status ledger"
            )
        elif declared != active_ledgers[0].path.relative_to(ROOT).as_posix():
            errors.append(
                f"{relative_index}: active_ledger does not match the active work.status document"
            )
    else:
        errors.append(f"{relative_index}: ledger_cutover must be pending or complete")

    if state not in {"current", "stale"}:
        errors.append(f"{relative_index}: ledger_state must be current or stale")
    if as_of is None:
        errors.append(f"{relative_index}: missing ledger_as_of")
    else:
        try:
            date.fromisoformat(as_of)
        except ValueError:
            errors.append(f"{relative_index}: ledger_as_of must be YYYY-MM-DD: {as_of}")
    if state == "stale":
        warnings.append(
            f"STALE_LEDGER: {declared or '<missing>'} requires re-baselining "
            f"(last recorded {as_of or '<missing>'})"
        )

    return errors, warnings


def emit_results(
    errors: list[str],
    warnings: list[str],
    *,
    curated_count: int,
    grandfathered_count: int,
) -> int:
    for error in errors:
        print(f"ERROR: {error}", file=sys.stderr)
    for warning in warnings:
        print(f"WARNING: {warning}", file=sys.stderr)
    if errors:
        return 1

    print(
        "Documentation check passed: "
        f"{curated_count} curated public documents, "
        f"{grandfathered_count} grandfathered public documents, "
        f"{len(warnings)} warnings"
    )
    return 0


def main() -> int:
    errors: list[str] = []
    warnings: list[str] = []
    documents: list[Document] = []
    seen_ids: dict[str, Path] = {}

    for path in public_documents():
        document, parse_errors = parse_document(path)
        errors.extend(parse_errors)
        if document is None:
            continue
        documents.append(document)
        doc_id = document.fields.get("doc_id")
        if doc_id in seen_ids:
            errors.append(
                f"duplicate doc_id {doc_id}: {seen_ids[doc_id].relative_to(ROOT)} and {path.relative_to(ROOT)}"
            )
        elif doc_id:
            seen_ids[doc_id] = path
        errors.extend(check_links(document))
        errors.extend(check_body(document))
        errors.extend(check_surface(document))
        warnings.extend(check_freshness(document))

    grandfathered = grandfathered_public_documents()
    for path in grandfathered:
        try:
            body = path.read_text(encoding="utf-8")
        except (OSError, UnicodeError) as error:
            errors.append(f"{path.relative_to(ROOT)}: cannot read public history ({error})")
            continue
        document = Document(path=path, fields={"visibility": "public"}, body=body)
        errors.extend(check_links(document))
        errors.extend(check_body(document))

    ledger_errors, ledger_warnings = check_ledger_policy(documents)
    errors.extend(ledger_errors)
    warnings.extend(ledger_warnings)

    errors.extend(check_local_record_registers())

    return emit_results(
        errors,
        warnings,
        curated_count=len(documents),
        grandfathered_count=len(grandfathered),
    )


if __name__ == "__main__":
    raise SystemExit(main())
