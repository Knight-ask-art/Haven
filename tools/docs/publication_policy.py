"""Shared public-document boundary, distinct from Haven's product AI contracts."""

from __future__ import annotations

import re
from unicodedata import normalize


DEVELOPER_DOCUMENT_PREFIXES = ("docs/agents/",)
DEVELOPER_DOCUMENT_PATHS = frozenset({
    "docs/architecture/claude_code_agent_workflow.md",
})
PRIVATE_INDEX_EXCEPTIONS = frozenset({
    "docs/plans/readme.md",
    "docs/reviews/readme.md",
})
PRIVATE_CATALOG_PATH_RE = re.compile(
    r"(?i)(?<![A-Za-z0-9_])(?:plan|docs[/\\](?:internal|private|aegis|superpowers|"
    r"plans|reviews|agents|drafts|project|tmp|\.tmp))[/\\][^\s\x60|<>\]\)]+\.(?:md|json|txt|toml|ya?ml)\b"
)
DEVELOPER_CLI_RE = re.compile(
    r"(?i)(?:\b(?:claude|codex)(?:\.exe)?\s+--(?:print|continue|resume|"
    r"effort|tools|output-format)\b|"
    r"--(?:permission-mode|permission-prompts|no-session-persistence|"
    r"allowed-?tools|dangerously-skip-permissions)\b)"
)
DEVELOPMENT_WORKFLOW_RE = re.compile(
    r"(?i)(?:vibe\s+coding|\bmain\s+agent\b|\bsubagent\b|"
    r"\bsub-agent\b|(?:Claude\s+Code|Codex).{0,30}(?:子代理|开发工作流|开发流程)|"
    r"主代理.{0,24}(?:派发|复审|交接)|"
    r"\b(?:Allowed\s+Paths|Forbidden\s+Paths|Execution\s+authority)\b[^\n]{0,80}:)"
)
DEVELOPER_WORKFLOW_REFERENCE_RE = re.compile(
    r"(?i)CLAUDE_CODE_AGENT_WORKFLOW\.md"
)


def developer_document_path(relative: str) -> bool:
    normalized = relative.replace("\\", "/").casefold()
    return normalized in DEVELOPER_DOCUMENT_PATHS or any(
        normalized.startswith(prefix) for prefix in DEVELOPER_DOCUMENT_PREFIXES
    )


def disclosure_errors(text: str) -> list[str]:
    """Return categories only, never echo matched private content."""
    text = normalize("NFKC", text)
    errors: list[str] = []
    if DEVELOPER_CLI_RE.search(text) or DEVELOPMENT_WORKFLOW_RE.search(text):
        errors.append("developer-tool operating procedure")
    if DEVELOPER_WORKFLOW_REFERENCE_RE.search(text):
        errors.append("private development-workflow reference")
    if any(
        match.group().replace("\\", "/").casefold() not in PRIVATE_INDEX_EXCEPTIONS
        for match in PRIVATE_CATALOG_PATH_RE.finditer(text)
    ):
        errors.append("private record filename or inventory")
    return errors
