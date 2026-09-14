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
    def _errors(self, *files: str) -> list[str]:
        errors: list[str] = []
        tracked = [*PUBLIC_SNAPSHOT_CHECK.REQUIRED_FILES, *files]
        PUBLIC_SNAPSHOT_CHECK.check_public_tree(Path("."), tracked, errors)
        return errors

    def test_public_docs_are_allowed(self) -> None:
        errors = self._errors(
            "docs/README.md",
            "docs/user-guide/getting-started.md",
            "docs/architecture/overview.md",
        )

        self.assertEqual(errors, [])

    def test_internal_document_directories_are_forbidden(self) -> None:
        errors = self._errors(
            "docs/internal/roadmap.md",
            "docs/private/release-notes.md",
            "docs/reviews/security-review.md",
            "docs/superpowers/specs/feature.md",
            "docs/drafts/unpublished.md",
            "docs/project/roadmap.md",
            "docs/tmp/session.md",
            "docs/.tmp/diagnostic.md",
        )

        internal_errors = [
            error for error in errors if "forbidden internal document path" in error
        ]
        self.assertEqual(len(internal_errors), 8)

    def test_existing_non_document_security_boundaries_remain_forbidden(self) -> None:
        errors = self._errors(
            "plan/implementation.md",
            "logs/app.log",
            "tmp/trace.txt",
            "src-tauri/diagnostics/session.json",
            "data/library.sqlite3",
            "captures/browser.har",
        )

        self.assertEqual(len(errors), 9)


if __name__ == "__main__":
    unittest.main()
