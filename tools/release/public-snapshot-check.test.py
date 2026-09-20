from __future__ import annotations

import importlib.util
import unittest
from pathlib import Path


MODULE_PATH = Path(__file__).with_name("public-snapshot-check.py")
SPEC = importlib.util.spec_from_file_location("public_snapshot_check", MODULE_PATH)
if SPEC is None or SPEC.loader is None:
    raise RuntimeError(f"无法加载 public snapshot check 模块: {MODULE_PATH}")
PUBLIC_SNAPSHOT_CHECK = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(PUBLIC_SNAPSHOT_CHECK)


class PublicSnapshotTreeTests(unittest.TestCase):
    def _errors(self, *files: str, omit: set[str] | None = None) -> list[str]:
        errors: list[str] = []
        tracked = [
            *(PUBLIC_SNAPSHOT_CHECK.REQUIRED_FILES - (omit or set())),
            *files,
        ]
        PUBLIC_SNAPSHOT_CHECK.check_public_tree(Path("."), tracked, errors)
        return errors

    def test_missing_required_public_file_is_rejected(self) -> None:
        missing = "docs/README.md"

        errors = self._errors(omit={missing})

        self.assertIn(f"required public file is not tracked: {missing}", errors)

    def test_public_docs_are_allowed(self) -> None:
        errors = self._errors(
            "docs/README.md",
            "docs/user-guide/getting-started.md",
            "docs/architecture/overview.md",
            "docs/plans/README.md",
            "docs/reviews/README.md",
            "docs/superpowers/specs/2026-09-10-comic-reading-center-design.md",
        )

        self.assertEqual(errors, [])

    def test_internal_document_directories_are_forbidden(self) -> None:
        errors = self._errors(
            "docs/internal/roadmap.md",
            "docs/private/release-notes.md",
            "docs/plans/2026-09-15-release.md",
            "docs/reviews/security-review.md",
            "docs/superpowers/specs/feature.md",
            "docs/drafts/unpublished.md",
            "docs/project/roadmap.md",
            "docs/tmp/session.md",
            "docs/.tmp/diagnostic.md",
            "DOCS/PRIVATE/uppercase.md",
            "PLAN/uppercase.md",
        )

        internal_errors = [
            error for error in errors if "forbidden internal document path" in error
        ]
        self.assertEqual(len(internal_errors), 10)

    def test_local_agent_instructions_are_not_public_snapshot_files(self) -> None:
        errors = self._errors("AGENTS.md", "nested/AGENTS.md")

        agent_errors = [
            error for error in errors if "local-only agent instruction file" in error
        ]
        self.assertEqual(len(agent_errors), 2)

    def test_existing_non_document_security_boundaries_remain_forbidden(self) -> None:
        errors = self._errors(
            "plan/implementation.md",
            "logs/app.log",
            "tmp/trace.txt",
            "src-tauri/diagnostics/session.json",
            "data/library.sqlite3",
            "captures/browser.har",
            ".env",
            "config/private.key",
        )

        self.assertEqual(len(errors), 11)


if __name__ == "__main__":
    unittest.main()
