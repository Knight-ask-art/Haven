from __future__ import annotations

import importlib.util
import io
import sys
import tempfile
import unittest
from contextlib import redirect_stderr, redirect_stdout
from datetime import date
from pathlib import Path


MODULE_PATH = Path(__file__).with_name("check.py")
SPEC = importlib.util.spec_from_file_location("docs_check", MODULE_PATH)
if SPEC is None or SPEC.loader is None:
    raise RuntimeError(f"无法加载 documentation checker: {MODULE_PATH}")
DOCS_CHECK = importlib.util.module_from_spec(SPEC)
sys.modules[SPEC.name] = DOCS_CHECK
SPEC.loader.exec_module(DOCS_CHECK)


class DocumentationCheckerTests(unittest.TestCase):
    def test_public_indexes_are_included_but_raw_history_is_excluded(self) -> None:
        relative_paths = {
            path.relative_to(DOCS_CHECK.DOCS).as_posix()
            for path in DOCS_CHECK.public_documents()
        }

        self.assertIn("plans/README.md", relative_paths)
        self.assertIn("reviews/README.md", relative_paths)
        self.assertNotIn(
            "plans/2026-09-15-v1.0.0-release-readiness-plan.md",
            relative_paths,
        )
        self.assertNotIn("reviews/2026-09-05-product-review.md", relative_paths)
        self.assertNotIn(
            "superpowers/specs/2026-09-10-comic-reading-center-design.md",
            relative_paths,
        )
        grandfathered = {
            path.relative_to(DOCS_CHECK.DOCS).as_posix()
            for path in DOCS_CHECK.grandfathered_public_documents()
        }
        self.assertEqual(
            grandfathered,
            set(),
        )

    def test_curated_documents_have_unique_ids_and_valid_links(self) -> None:
        documents = []
        errors = []
        seen_ids: dict[str, Path] = {}

        for path in DOCS_CHECK.public_documents():
            document, parse_errors = DOCS_CHECK.parse_document(path)
            errors.extend(parse_errors)
            self.assertIsNotNone(document)
            if document is None:
                continue
            documents.append(document)
            doc_id = document.fields.get("doc_id")
            self.assertNotIn(doc_id, seen_ids)
            if doc_id:
                seen_ids[doc_id] = path
            errors.extend(DOCS_CHECK.check_links(document))
            errors.extend(DOCS_CHECK.check_body(document))

        self.assertGreaterEqual(len(documents), 1)
        self.assertEqual(errors, [])

    def test_duplicate_metadata_field_is_rejected(self) -> None:
        with tempfile.TemporaryDirectory(dir=DOCS_CHECK.ROOT) as temporary:
            path = Path(temporary) / "duplicate.md"
            path.write_text(
                "\n".join(
                    (
                        "---",
                        "doc_id: fixture.duplicate",
                        "type: guide",
                        "status: active",
                        "owner: first-owner",
                        "owner: second-owner",
                        "visibility: public",
                        "source_of_truth: fixture",
                        "last_reviewed: 2026-09-20",
                        "review_after: 2026-12-20",
                        "---",
                        "# Fixture",
                    )
                ),
                encoding="utf-8",
            )

            _, errors = DOCS_CHECK.parse_document(path)

        self.assertTrue(any("duplicate metadata field: owner" in error for error in errors))

    def test_no_legacy_document_is_public_by_exception(self) -> None:
        self.assertEqual(DOCS_CHECK.grandfathered_public_documents(), [])

    def test_public_body_rejects_private_paths_signed_urls_and_markers(self) -> None:
        with tempfile.TemporaryDirectory(dir=DOCS_CHECK.ROOT) as temporary:
            path = Path(temporary) / "fixture.md"
            path.write_text("fixture", encoding="utf-8")
            document = DOCS_CHECK.Document(
                path=path,
                fields={"visibility": "public"},
                body=(
                    "C:/Users/example/.claude/plans/task.md "
                    "https://example.test/file?signature=secret "
                    "ghp_" "123456789012345678901234567890\n"
                    "Cookie: session=raw-value"
                ),
            )

            errors = DOCS_CHECK.check_body(document)

        self.assertTrue(any("private runtime path" in error for error in errors))
        self.assertTrue(any("signed URL" in error for error in errors))
        self.assertTrue(any("credential marker" in error for error in errors))
        self.assertTrue(any("raw cookie header" in error for error in errors))

    def test_public_body_allows_custom_protocol_uri(self) -> None:
        document = DOCS_CHECK.Document(
            path=DOCS_CHECK.ROOT / "docs" / "README.md",
            fields={"visibility": "public"},
            body="haven-resource://session/<uuid>",
        )

        self.assertEqual(DOCS_CHECK.check_body(document), [])

    def test_public_body_rejects_file_uri_case_insensitively(self) -> None:
        document = DOCS_CHECK.Document(
            path=DOCS_CHECK.ROOT / "docs" / "README.md",
            fields={"visibility": "public"},
            body="FILE:///private-document.md",
        )

        errors = DOCS_CHECK.check_body(document)

        self.assertTrue(any("absolute path or file URI" in error for error in errors))

    def test_curated_surface_rejects_internal_visibility(self) -> None:
        document = DOCS_CHECK.Document(
            path=DOCS_CHECK.ROOT / "docs" / "README.md",
            fields={"visibility": "internal"},
            body="",
        )

        errors = DOCS_CHECK.check_surface(document)

        self.assertEqual(len(errors), 1)
        self.assertIn("must declare visibility: public", errors[0])

    def test_links_cannot_escape_the_repository(self) -> None:
        document = DOCS_CHECK.Document(
            path=DOCS_CHECK.ROOT / "docs" / "README.md",
            fields={"visibility": "public"},
            body="[outside](../../outside-secret.txt)",
        )

        errors = DOCS_CHECK.check_links(document)

        self.assertEqual(len(errors), 1)
        self.assertIn("link escapes repository", errors[0])

    def test_public_link_cannot_target_private_repository_material(self) -> None:
        document = DOCS_CHECK.Document(
            path=DOCS_CHECK.ROOT / "docs" / "README.md",
            fields={"visibility": "public"},
            body="[private instructions](../AGENTS.md)",
        )

        errors = DOCS_CHECK.check_links(document)

        self.assertEqual(len(errors), 1)
        self.assertIn("public link targets a private path", errors[0])

    def test_public_link_cannot_target_ignored_internal_material(self) -> None:
        document = DOCS_CHECK.Document(
            path=DOCS_CHECK.ROOT / "docs" / "README.md",
            fields={"visibility": "public"},
            body=(
                "[tool notes](../tools/README.md)\n"
                "[design notes](../design-qa.md)\n"
                "[frontend rules](../前端/设计原则/README.md)"
            ),
        )

        errors = DOCS_CHECK.check_links(document)

        self.assertEqual(len(errors), 3)
        self.assertTrue(all("public link targets a private path" in error for error in errors))

    def test_custom_protocol_markdown_link_is_not_treated_as_a_file(self) -> None:
        document = DOCS_CHECK.Document(
            path=DOCS_CHECK.ROOT / "docs" / "README.md",
            fields={"visibility": "public"},
            body="[session](haven-resource://session/<uuid>)",
        )

        self.assertEqual(DOCS_CHECK.check_links(document), [])
        self.assertIsNotNone(
            DOCS_CHECK.URI_WITH_AUTHORITY_RE.match("haven-resource://session/<uuid>")
        )
        self.assertIsNone(DOCS_CHECK.URI_WITH_AUTHORITY_RE.match("C:/Users/example"))
        self.assertIsNone(DOCS_CHECK.URI_WITH_AUTHORITY_RE.match(r"C:\Users\example"))

    def test_active_document_past_review_date_emits_stale_warning(self) -> None:
        document = DOCS_CHECK.Document(
            path=DOCS_CHECK.ROOT / "docs" / "README.md",
            fields={"status": "active", "review_after": "2026-01-01"},
            body="",
        )

        warnings = DOCS_CHECK.check_freshness(
            document,
            today=date(2026, 9, 20),
        )

        self.assertEqual(len(warnings), 1)
        self.assertIn("STALE_REVIEW", warnings[0])

    def test_non_active_document_does_not_emit_stale_warning(self) -> None:
        document = DOCS_CHECK.Document(
            path=DOCS_CHECK.ROOT / "docs" / "README.md",
            fields={"status": "archived", "review_after": "2026-01-01"},
            body="",
        )

        warnings = DOCS_CHECK.check_freshness(
            document,
            today=date(2026, 9, 20),
        )

        self.assertEqual(warnings, [])

    def test_local_record_must_be_named_in_its_register(self) -> None:
        with tempfile.TemporaryDirectory(dir=DOCS_CHECK.ROOT) as temporary:
            root = Path(temporary)
            records = root / "records"
            records.mkdir()
            record = records / "old-review.md"
            record.write_text("historical", encoding="utf-8")
            register = root / "register.md"
            register.write_text("# Register\n", encoding="utf-8")

            errors = DOCS_CHECK.check_record_register(
                records,
                register,
                recursive=False,
                excluded_names=frozenset(),
            )
            register.write_text(
                f"# Register\n\n`{record.relative_to(DOCS_CHECK.ROOT).as_posix()}`\n",
                encoding="utf-8",
            )
            registered_errors = DOCS_CHECK.check_record_register(
                records,
                register,
                recursive=False,
                excluded_names=frozenset(),
            )

        self.assertEqual(len(errors), 1)
        self.assertIn("local record is missing", errors[0])
        self.assertEqual(registered_errors, [])

    def test_plan_register_is_bidirectional_when_local_corpus_exists(self) -> None:
        with tempfile.TemporaryDirectory(dir=DOCS_CHECK.ROOT) as temporary:
            root = Path(temporary)
            plans = root / "plan"
            plans.mkdir()
            current = plans / "current.md"
            current.write_text("# Current\n", encoding="utf-8")
            register = root / "register.md"
            missing_path = (plans / "missing.md").relative_to(DOCS_CHECK.ROOT).as_posix()
            register.write_text(
                f"| `{missing_path}` | stale entry |\n",
                encoding="utf-8",
            )

            errors = DOCS_CHECK.check_plan_register(plans, register)
            current_path = current.relative_to(DOCS_CHECK.ROOT).as_posix()
            register.write_text(
                f"| `{current_path}` | registered |\n",
                encoding="utf-8",
            )
            registered_errors = DOCS_CHECK.check_plan_register(plans, register)

        self.assertEqual(len(errors), 2)
        self.assertTrue(any("local plan is missing" in error for error in errors))
        self.assertTrue(any("has no local file" in error for error in errors))
        self.assertEqual(registered_errors, [])

    def test_pending_ledger_cutover_rejects_public_active_ledger(self) -> None:
        work_index = DOCS_CHECK.Document(
            path=DOCS_CHECK.ROOT / "docs" / "work" / "README.md",
            fields={
                "doc_id": "work.index",
                "active_ledger": "plan/IMPLEMENTATION_STATUS.md",
                "ledger_cutover": "pending",
                "ledger_state": "stale",
                "ledger_as_of": "2026-09-15",
            },
            body="",
        )
        public_ledger = DOCS_CHECK.Document(
            path=DOCS_CHECK.ROOT / "docs" / "work" / "STATUS.md",
            fields={"doc_id": "work.status", "status": "active"},
            body="",
        )

        errors, warnings = DOCS_CHECK.check_ledger_policy([work_index, public_ledger])

        self.assertTrue(any("pending cutover forbids" in error for error in errors))
        self.assertTrue(any("STALE_LEDGER" in warning for warning in warnings))

    def test_completed_ledger_cutover_requires_declared_public_ledger(self) -> None:
        work_index = DOCS_CHECK.Document(
            path=DOCS_CHECK.ROOT / "docs" / "work" / "README.md",
            fields={
                "doc_id": "work.index",
                "active_ledger": "docs/work/STATUS.md",
                "ledger_cutover": "complete",
                "ledger_state": "current",
                "ledger_as_of": "2026-09-20",
            },
            body="",
        )
        public_ledger = DOCS_CHECK.Document(
            path=DOCS_CHECK.ROOT / "docs" / "work" / "STATUS.md",
            fields={"doc_id": "work.status", "status": "active"},
            body="",
        )

        errors, warnings = DOCS_CHECK.check_ledger_policy([work_index, public_ledger])

        self.assertEqual(errors, [])
        self.assertEqual(warnings, [])

    def test_warnings_are_emitted_even_when_errors_exist(self) -> None:
        stdout = io.StringIO()
        stderr = io.StringIO()

        with redirect_stdout(stdout), redirect_stderr(stderr):
            result = DOCS_CHECK.emit_results(
                ["broken"],
                ["stale"],
                curated_count=1,
                grandfathered_count=1,
            )

        self.assertEqual(result, 1)
        self.assertIn("ERROR: broken", stderr.getvalue())
        self.assertIn("WARNING: stale", stderr.getvalue())

    def test_current_local_records_are_all_registered(self) -> None:
        self.assertEqual(DOCS_CHECK.check_local_record_registers(), [])


if __name__ == "__main__":
    unittest.main()
