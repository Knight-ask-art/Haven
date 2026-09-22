from __future__ import annotations

import importlib.util
import tempfile
import unittest
from pathlib import Path


MODULE_PATH = Path(__file__).with_name("skill-contract-check.py")
SPEC = importlib.util.spec_from_file_location("skill_contract_check", MODULE_PATH)
if SPEC is None or SPEC.loader is None:
    raise RuntimeError(f"无法加载 Skill 契约校验模块: {MODULE_PATH}")
SKILL_CHECK = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(SKILL_CHECK)

REPOSITORY_ROOT = Path(__file__).parents[2]
REAL_SKILL = REPOSITORY_ROOT / "skills/haven-agent-proposal"

FROZEN_TOOLS = (
    "get_system_capabilities",
    "get_settings_snapshot",
    "get_setting_sources",
    "get_resource_preference_snapshot",
    "get_library_summary",
    "get_media_capabilities",
    "get_onboarding_state",
    "propose_settings_patch",
    "propose_resource_preference_patch",
)

CAPABILITIES = (
    "settings_read",
    "settings_proposal",
    "library_summary_read",
    "metadata_proposal",
    "rename_proposal",
    "secret_read",
    "filesystem_write",
)


def _write_fixture_mcp(root: Path, tools: tuple[str, ...] = FROZEN_TOOLS) -> None:
    """写一份最小但形状正确的 MCP 源码，供校验器读取权威清单。"""
    constants = root / SKILL_CHECK.MCP_CONSTANTS
    constants.parent.mkdir(parents=True, exist_ok=True)
    listed = ",\n  ".join(f'"{tool}"' for tool in tools)
    constants.write_text(
        "export const TOOL_NAMES = [\n  " + listed + ",\n] as const;\n",
        encoding="utf-8",
    )

    bridge = root / SKILL_CHECK.MCP_BRIDGE
    bridge.parent.mkdir(parents=True, exist_ok=True)
    fields = "\n    ".join(f"{name}: boolean;" for name in CAPABILITIES)
    bridge.write_text(
        "export interface HavenCapabilityReport {\n"
        "  agent_api_version: number;\n"
        "  capabilities: {\n"
        f"    {fields}\n"
        "  };\n"
        "}\n",
        encoding="utf-8",
    )


def _write_skill(
    root: Path,
    skill: str = "skills/fixture-skill",
    *,
    name: str = "fixture-skill",
    description: str = "A fixture skill used by the contract check tests.",
    body: str = "读取 `get_system_capabilities` 后如实回答。\n",
    extra_files: dict[str, str] | None = None,
    evals: str | None = None,
    frontmatter_extra: str = "",
) -> Path:
    skill_dir = root / skill
    skill_dir.mkdir(parents=True, exist_ok=True)
    skill_dir.joinpath("SKILL.md").write_text(
        f"---\nname: {name}\ndescription: {description}\n{frontmatter_extra}---\n\n# Fixture\n\n{body}",
        encoding="utf-8",
    )
    if evals is None:
        evals = (
            '{"skill_name": "'
            + name
            + '", "evals": ['
            + ",".join(
                '{"id": %d, "name": "n", "prompt": "p", "expected_output": "o", '
                '"expectations": ["x"]}' % (index + 1)
                for index in range(SKILL_CHECK.MIN_EVALS)
            )
            + "]}"
        )
    (skill_dir / "evals").mkdir(exist_ok=True)
    (skill_dir / "evals/evals.json").write_text(evals, encoding="utf-8")

    for relative, content in (extra_files or {}).items():
        target = skill_dir / relative
        target.parent.mkdir(parents=True, exist_ok=True)
        target.write_text(content, encoding="utf-8")

    return skill_dir


class SkillContractCheckTests(unittest.TestCase):
    def test_repository_skill_passes(self) -> None:
        errors = SKILL_CHECK.check_skill(REPOSITORY_ROOT, REAL_SKILL)

        self.assertEqual(errors, [])

    def test_frozen_tool_list_is_read_from_mcp_source(self) -> None:
        with tempfile.TemporaryDirectory() as temporary_directory:
            root = Path(temporary_directory)
            _write_fixture_mcp(root, tools=FROZEN_TOOLS[:8])
            skill_dir = _write_skill(root)

            errors = SKILL_CHECK.check_skill(root, skill_dir)

            # 清单不是 9 项时报错，而不是"少了几项也照样过"。
            self.assertTrue(any("期望 9 项" in error for error in errors), errors)

    def test_unknown_tool_name_is_rejected(self) -> None:
        with tempfile.TemporaryDirectory() as temporary_directory:
            root = Path(temporary_directory)
            _write_fixture_mcp(root)
            skill_dir = _write_skill(
                root,
                body="先调用 `get_system_capabilities`，然后调用 `apply_settings_patch` 完成写入。\n",
            )

            errors = SKILL_CHECK.check_skill(root, skill_dir)

            self.assertTrue(
                any("apply_settings_patch" in error for error in errors),
                errors,
            )

    def test_unknown_tool_name_is_allowed_in_a_prohibition(self) -> None:
        with tempfile.TemporaryDirectory() as temporary_directory:
            root = Path(temporary_directory)
            _write_fixture_mcp(root)
            skill_dir = _write_skill(
                root,
                body="❌ 调用 `apply_settings_patch` 或 `metadata_patch`。本技能不这样做。\n",
            )

            errors = SKILL_CHECK.check_skill(root, skill_dir)

            self.assertEqual(errors, [])

    def test_capability_names_are_not_treated_as_tools(self) -> None:
        with tempfile.TemporaryDirectory() as temporary_directory:
            root = Path(temporary_directory)
            _write_fixture_mcp(root)
            skill_dir = _write_skill(
                root,
                body='能力位 `metadata_proposal` 与 `filesystem_write` 恒为 false。\n',
            )

            errors = SKILL_CHECK.check_skill(root, skill_dir)

            self.assertEqual(errors, [])

    def test_oversized_skill_md_is_rejected(self) -> None:
        with tempfile.TemporaryDirectory() as temporary_directory:
            root = Path(temporary_directory)
            _write_fixture_mcp(root)
            skill_dir = _write_skill(root, body="行\n" * SKILL_CHECK.MAX_SKILL_LINES)

            errors = SKILL_CHECK.check_skill(root, skill_dir)

            self.assertTrue(any("行数超限" in error for error in errors), errors)

    def test_frontmatter_name_must_be_hyphen_case(self) -> None:
        with tempfile.TemporaryDirectory() as temporary_directory:
            root = Path(temporary_directory)
            _write_fixture_mcp(root)
            skill_dir = _write_skill(root, name="Fixture_Skill")

            errors = SKILL_CHECK.check_skill(root, skill_dir)

            self.assertTrue(any("hyphen-case" in error for error in errors), errors)

    def test_frontmatter_rejects_unexpected_keys(self) -> None:
        with tempfile.TemporaryDirectory() as temporary_directory:
            root = Path(temporary_directory)
            _write_fixture_mcp(root)
            skill_dir = _write_skill(root, frontmatter_extra="allowed-tools: Bash\n")

            errors = SKILL_CHECK.check_skill(root, skill_dir)

            self.assertTrue(any("多余键" in error for error in errors), errors)

    def test_missing_description_is_rejected(self) -> None:
        with tempfile.TemporaryDirectory() as temporary_directory:
            root = Path(temporary_directory)
            _write_fixture_mcp(root)
            skill_dir = _write_skill(root, description="")

            errors = SKILL_CHECK.check_skill(root, skill_dir)

            self.assertTrue(any("description" in error for error in errors), errors)

    def test_evals_skill_name_must_match_frontmatter(self) -> None:
        with tempfile.TemporaryDirectory() as temporary_directory:
            root = Path(temporary_directory)
            _write_fixture_mcp(root)
            skill_dir = _write_skill(
                root,
                evals='{"skill_name": "something-else", "evals": '
                '[{"id":1,"name":"n","prompt":"p","expected_output":"o","expectations":["x"]}]}',
            )

            errors = SKILL_CHECK.check_skill(root, skill_dir)

            self.assertTrue(any("skill_name" in error for error in errors), errors)

    def test_too_few_evals_are_rejected(self) -> None:
        with tempfile.TemporaryDirectory() as temporary_directory:
            root = Path(temporary_directory)
            _write_fixture_mcp(root)
            skill_dir = _write_skill(
                root,
                evals='{"skill_name": "fixture-skill", "evals": '
                '[{"id":1,"name":"n","prompt":"p","expected_output":"o","expectations":["x"]}]}',
            )

            errors = SKILL_CHECK.check_skill(root, skill_dir)

            self.assertTrue(any("eval 数量不足" in error for error in errors), errors)

    def test_integer_eval_ids_are_accepted(self) -> None:
        with tempfile.TemporaryDirectory() as temporary_directory:
            root = Path(temporary_directory)
            _write_fixture_mcp(root)
            skill_dir = _write_skill(
                root,
                evals='{"skill_name": "fixture-skill", "evals": ['
                '{"id":1,"name":"n","prompt":"p","expected_output":"o","expectations":["x"]},'
                '{"id":2,"name":"n","prompt":"p","expected_output":"o","expectations":["x"]},'
                '{"id":3,"name":"n","prompt":"p","expected_output":"o","expectations":["x"]}]}',
            )

            errors = SKILL_CHECK.check_skill(root, skill_dir)

            self.assertEqual(errors, [])

    def test_string_eval_ids_are_rejected(self) -> None:
        with tempfile.TemporaryDirectory() as temporary_directory:
            root = Path(temporary_directory)
            _write_fixture_mcp(root)
            skill_dir = _write_skill(
                root,
                evals='{"skill_name": "fixture-skill", "evals": ['
                '{"id":"1","name":"n","prompt":"p","expected_output":"o","expectations":["x"]},'
                '{"id":2,"name":"n","prompt":"p","expected_output":"o","expectations":["x"]},'
                '{"id":3,"name":"n","prompt":"p","expected_output":"o","expectations":["x"]}]}',
            )

            errors = SKILL_CHECK.check_skill(root, skill_dir)

            self.assertTrue(any("id 必须是整数" in error for error in errors), errors)

    def test_boolean_eval_ids_are_rejected(self) -> None:
        with tempfile.TemporaryDirectory() as temporary_directory:
            root = Path(temporary_directory)
            _write_fixture_mcp(root)
            # Python 里 `isinstance(True, int)` 为真，而 JSON 的 `true` 会解析成 `True`。
            # 不显式排除 bool 的话，`"id": true` 会被当成合法的整数 id。
            skill_dir = _write_skill(
                root,
                evals='{"skill_name": "fixture-skill", "evals": ['
                '{"id":true,"name":"n","prompt":"p","expected_output":"o","expectations":["x"]},'
                '{"id":2,"name":"n","prompt":"p","expected_output":"o","expectations":["x"]},'
                '{"id":3,"name":"n","prompt":"p","expected_output":"o","expectations":["x"]}]}',
            )

            errors = SKILL_CHECK.check_skill(root, skill_dir)

            self.assertTrue(any("id 必须是整数" in error for error in errors), errors)

    def test_duplicate_integer_eval_ids_are_rejected(self) -> None:
        with tempfile.TemporaryDirectory() as temporary_directory:
            root = Path(temporary_directory)
            _write_fixture_mcp(root)
            skill_dir = _write_skill(
                root,
                evals='{"skill_name": "fixture-skill", "evals": ['
                '{"id":1,"name":"n","prompt":"p","expected_output":"o","expectations":["x"]},'
                '{"id":1,"name":"n","prompt":"p","expected_output":"o","expectations":["x"]},'
                '{"id":2,"name":"n","prompt":"p","expected_output":"o","expectations":["x"]}]}',
            )

            errors = SKILL_CHECK.check_skill(root, skill_dir)

            self.assertTrue(any("id 重复" in error for error in errors), errors)

    def test_missing_or_null_eval_ids_are_rejected(self) -> None:
        # `entry.get("id")` 对「键缺失」与「显式 null」都返回 None。两条路径都必须被拒绝：
        # 不能因为拿不到值就当成 0，也不能静默跳过这条 eval。
        # 其余两条 eval 保持整数 id，确保报错只来自被测的那一条。
        prefix = '{"skill_name": "fixture-skill", "evals": ['
        common_tail = (
            '{"id":2,"name":"n","prompt":"p","expected_output":"o","expectations":["x"]},'
            '{"id":3,"name":"n","prompt":"p","expected_output":"o","expectations":["x"]}]}'
        )
        cases = {
            "missing": prefix
            + '{"name":"n","prompt":"p","expected_output":"o","expectations":["x"]},'
            + common_tail,
            "null": prefix
            + '{"id":null,"name":"n","prompt":"p","expected_output":"o","expectations":["x"]},'
            + common_tail,
        }

        for label, payload in cases.items():
            with self.subTest(case=label), tempfile.TemporaryDirectory() as temporary_directory:
                root = Path(temporary_directory)
                _write_fixture_mcp(root)
                skill_dir = _write_skill(root, evals=payload)

                errors = SKILL_CHECK.check_skill(root, skill_dir)

                self.assertTrue(any("id 必须是整数" in error for error in errors), errors)

    def test_unparsable_evals_are_rejected(self) -> None:
        with tempfile.TemporaryDirectory() as temporary_directory:
            root = Path(temporary_directory)
            _write_fixture_mcp(root)
            skill_dir = _write_skill(root, evals="{not json")

            errors = SKILL_CHECK.check_skill(root, skill_dir)

            self.assertTrue(any("无法解析" in error for error in errors), errors)

    def test_empty_expectations_are_rejected(self) -> None:
        with tempfile.TemporaryDirectory() as temporary_directory:
            root = Path(temporary_directory)
            _write_fixture_mcp(root)
            skill_dir = _write_skill(
                root,
                evals='{"skill_name": "fixture-skill", "evals": ['
                '{"id":1,"name":"n","prompt":"p","expected_output":"o","expectations":[]},'
                '{"id":2,"name":"n","prompt":"p","expected_output":"o","expectations":["x"]},'
                '{"id":3,"name":"n","prompt":"p","expected_output":"o","expectations":["x"]}]}',
            )

            errors = SKILL_CHECK.check_skill(root, skill_dir)

            self.assertTrue(any("expectations" in error for error in errors), errors)

    def test_missing_mcp_source_fails_instead_of_skipping(self) -> None:
        with tempfile.TemporaryDirectory() as temporary_directory:
            root = Path(temporary_directory)
            skill_dir = _write_skill(root)  # 故意不写 MCP 源码

            errors = SKILL_CHECK.check_skill(root, skill_dir)

            # 读不到权威清单时必须失败：静默跳过会让这条检查变成摆设。
            self.assertTrue(any("无法读取 MCP 常量文件" in error for error in errors), errors)

    def test_repository_skill_declares_the_frozen_nine_tools(self) -> None:
        text = (REAL_SKILL / "SKILL.md").read_text(encoding="utf-8")
        references = (REAL_SKILL / "references/mcp-tool-map.md").read_text(encoding="utf-8")

        for tool in FROZEN_TOOLS:
            self.assertIn(tool, text + references, f"技能文档缺少工具 {tool}")

    def test_repository_skill_never_grants_apply_or_approve(self) -> None:
        combined = "\n".join(
            path.read_text(encoding="utf-8")
            for path in sorted(REAL_SKILL.rglob("*.md"))
        )

        # 允许出现"不得 apply"这类禁止说明，但不允许出现授予使用的祈使句。
        for forbidden in ("调用 `apply", "调用 `approve", "使用 `apply", "使用 `approve"):
            self.assertNotIn(forbidden, combined)


if __name__ == "__main__":
    unittest.main()
