from __future__ import annotations

import importlib.util
import tempfile
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
        )

        self.assertEqual(errors, [])

    def test_internal_document_directories_are_forbidden(self) -> None:
        errors = self._errors(
            "docs/internal/roadmap.md",
            "docs/private/release-notes.md",
            "docs/plans/example-release.md",
            "docs/reviews/security-review.md",
            "docs/superpowers/specs/feature.md",
            "docs/drafts/unpublished.md",
            "docs/project/roadmap.md",
            "docs/tmp/session.md",
            "docs/.tmp/diagnostic.md",
            "DOCS/PRIVATE/uppercase.md",
            "PLAN/uppercase.md",
            "docs/aegis/work/checkpoint.md",
        )

        internal_errors = [
            error for error in errors if "forbidden internal document path" in error
        ]
        self.assertEqual(len(internal_errors), 11)

    def test_local_agent_instructions_are_not_public_snapshot_files(self) -> None:
        errors = self._errors("AGENTS.md", "nested/AGENTS.md")

        agent_errors = [
            error for error in errors if "local-only agent instruction file" in error
        ]
        self.assertEqual(len(agent_errors), 2)

    def test_development_sop_paths_are_forbidden_without_hiding_product_ai(self) -> None:
        errors = self._errors(
            "docs/agents/claude-code.md",
            "docs/agents/handoff.md",
            "DOCS/AGENTS/roles.md",
            "docs/architecture/CLAUDE_CODE_AGENT_WORKFLOW.md",
        )
        self.assertEqual(
            sum("forbidden internal document path" in error for error in errors), 4
        )
        self.assertEqual(self._errors(
            "docs/architecture/AI_SYSTEM.md",
            "docs/architecture/MCP_EXTERNAL_AGENT_TRANSPORT.md",
            "skills/haven-agent-proposal/SKILL.md",
        ), [])

    def test_public_entrypoints_cannot_republish_developer_cli(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            (root / "CLAUDE.md").write_text(
                "claude --print --permission-mode plan", encoding="utf-8"
            )
            errors: list[str] = []
            PUBLIC_SNAPSHOT_CHECK.check_public_document_content(root, ["CLAUDE.md"], errors)
        self.assertTrue(any("developer-tool operating procedure" in error for error in errors))

    def test_public_content_policy_preserves_product_client_configuration(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            (root / "README.md").write_text(
                "Haven MCP supports Codex and Claude Code via stdio. "
                "claude mcp add haven --scope user", encoding="utf-8"
            )
            errors: list[str] = []
            PUBLIC_SNAPSHOT_CHECK.check_public_document_content(root, ["README.md"], errors)
        self.assertEqual(errors, [])

    def test_public_pr_template_cannot_republish_fullwidth_delegation_fields(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            (root / ".github").mkdir()
            template = ".github/PULL_REQUEST_TEMPLATE.md"
            (root / template).write_text(
                "Allowed Paths：src\nForbidden Paths / 不在范围内：data", encoding="utf-8"
            )
            errors: list[str] = []
            PUBLIC_SNAPSHOT_CHECK.check_public_document_content(root, [template], errors)
        self.assertTrue(any("developer-tool operating procedure" in error for error in errors))

    def test_public_content_is_fail_closed_on_missing_document(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            errors: list[str] = []
            PUBLIC_SNAPSHOT_CHECK.check_public_document_content(
                Path(directory), ["README.md"], errors
            )
        self.assertTrue(any("cannot inspect public document" in error for error in errors))

    def test_public_content_cannot_name_private_drafts_with_cjk_prefix(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            (root / "README.md").write_text(
                "见plan/private-note.md；参考docs/drafts/private-note.md", encoding="utf-8"
            )
            errors: list[str] = []
            PUBLIC_SNAPSHOT_CHECK.check_public_document_content(root, ["README.md"], errors)
        self.assertTrue(any("private record filename or inventory" in error for error in errors))
        self.assertTrue(all("private-note" not in error for error in errors))

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

    def test_generated_bindings_cover_split_wire_modules_without_waiving_missing_types(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            source = root / "后端/crates/haven-application/src/wire"
            bindings = root / "后端/crates/haven-application/bindings"
            generated = root / "前端/app/src/lib/ipc/generated"
            for target in (source, bindings, generated):
                target.mkdir(parents=True)
            (source / "dto.rs").write_text("#[ts(export)]\npub struct CoreDto {}\npub enum InlineKind {}\n#[ts(type = \"string\")]\npub enum LocatorDto {}", encoding="utf-8")
            (source / "generate.rs").write_text("LocatorDto::export_to_string(&config).unwrap()", encoding="utf-8")
            (source / "cloud_storage.rs").write_text("#[ts(export, rename_all = \"camelCase\")]\npub struct CloudDto {}", encoding="utf-8")
            for name in ("CoreDto", "CloudDto", "LocatorDto"):
                (bindings / f"{name}.ts").write_text(f"export type {name} = {{}}", encoding="utf-8")
            (generated / "wire.ts").write_text("// Generated by test; Do not edit\nexport type CoreDto = {};\nexport type CloudDto = {};\nexport type LocatorDto = string;\n", encoding="utf-8")
            errors = []
            PUBLIC_SNAPSHOT_CHECK.check_generated_bindings(root, errors)
            self.assertEqual(errors, [])
            (source / "cloud_storage.rs").write_text("#[ts(export)]\npub struct CloudDto {}\n#[ts(export)]\npub struct MissingDto {}", encoding="utf-8")
            PUBLIC_SNAPSHOT_CHECK.check_generated_bindings(root, errors)
            self.assertIn("Rust DTO names and individual TypeScript bindings differ", errors)


if __name__ == "__main__":
    unittest.main()
