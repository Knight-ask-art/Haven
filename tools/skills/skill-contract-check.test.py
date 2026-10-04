from __future__ import annotations

import importlib.util
import re
import sys
import tempfile
import unittest
from pathlib import Path
from unittest import mock


MODULE_PATH = Path(__file__).with_name("skill-contract-check.py")
SPEC = importlib.util.spec_from_file_location("skill_contract_check", MODULE_PATH)
if SPEC is None or SPEC.loader is None:
    raise RuntimeError(f"无法加载 Skill 契约校验模块: {MODULE_PATH}")
SKILL_CHECK = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(SKILL_CHECK)

REPOSITORY_ROOT = Path(__file__).parents[2]
REAL_SKILL = REPOSITORY_ROOT / "skills/haven-agent-proposal"
REAL_SKILL_MD = REAL_SKILL / "SKILL.md"

# Provider 的 strict schema 现读来源：原生结构化输出的键名以它为准，不在这里另抄一份。
PROVIDER_SCHEMA = REPOSITORY_ROOT / "后端/crates/haven-infrastructure/src/ai_provider.rs"

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


def _to_snake_case(camel: str) -> str:
    """`fontFamily` -> `font_family`。用来把 Provider 的 camelCase 键名投影成外部 MCP 拼写。"""
    return re.sub(r"(?<!^)(?=[A-Z])", "_", camel).lower()


def _provider_recommendation_keys() -> tuple[list[str], list[str]]:
    """从 Rust Provider 的 `recommendation_json_schema()` 现读必需键：`(外层, settingsPatch)`。

    刻意不另抄一份：抄一份的清单迟早会与真实 schema 分叉，而这条检查的全部意义就是
    "技能正文里的键名 == Provider 真正要求的键名"。
    """
    text = PROVIDER_SCHEMA.read_text(encoding="utf-8")
    body = re.search(
        r"fn recommendation_json_schema\(\) -> Value \{(.*?)\n\}", text, re.DOTALL
    )
    if body is None:
        raise AssertionError(f"{PROVIDER_SCHEMA}: 找不到 recommendation_json_schema()")

    outer = re.search(r'"required"\s*:\s*\[(.*?)\]', body.group(1), re.DOTALL)
    patch = re.search(
        r'"settingsPatch"\s*:\s*\{.*?"required"\s*:\s*\[(.*?)\]',
        body.group(1),
        re.DOTALL,
    )
    if outer is None or patch is None:
        raise AssertionError(f"{PROVIDER_SCHEMA}: 找不到 settingsPatch 的必需键清单")

    def keys(block: str) -> list[str]:
        return re.findall(r'"([A-Za-z0-9_]+)"', block)

    return keys(outer.group(1)), keys(patch.group(1))


def _native_projection(skill: Path = REAL_SKILL) -> str:
    """原生运行时会真正读到的那份正文——**直接调用模块里的实现**，不在这里复刻。

    本地复刻会让"投影到底对不对"有两个答案：测试跟着复刻走，而生产走的是模块。更要紧的是
    "正文从 frontmatter 的闭合行之后开始"这件事必须由被测实现回答，否则测试会继续把元数据
    当成正文，从而对真实的泄漏视而不见。
    """
    projection, error = SKILL_CHECK.native_projection(skill)
    if error is not None or projection is None:
        raise AssertionError(f"原生投影必须可计算：{error}")
    return projection


def _acceptance_errors(document: str) -> list[str]:
    """**完整验收层**的结论：与 Rust `BuiltinAgentSkill::from_document` 同一层。

    分层与 Rust 一一对应：

    - `parse_skill_document` ≈ `_parse_frontmatter`（逐字解析，**不含** id 形状判定）；
    - `AgentSkillId::parse` ≈ `_check_frontmatter` 里的 name 规则。

    因此"引号包起来的 name"该在哪一层被拒，两侧必须是同一层：解析层按字面保留，
    验收层按 id 规则拒绝。把断言放在解析层会要求校验器做一件运行时不做的事。
    """
    with tempfile.TemporaryDirectory() as temporary_directory:
        path = Path(temporary_directory) / "SKILL.md"
        path.write_bytes(document.encode("utf-8"))
        parsed, error = SKILL_CHECK._parse_frontmatter(path)
        if error is not None or not isinstance(parsed, dict):
            return [error or "frontmatter 无法解析"]
        return SKILL_CHECK._check_frontmatter(parsed)


def _reading_patch_field_table(reference: str) -> str:
    """外部参考里"阅读排版 patch 字段"那两张表之间的正文。

    刻意只取**合同表**：提案返回示例里的 `changes[].key`（`reading.fontSize`）是 MCP
    契约里真实的 camelCase 变更键，出现在文档里是正确的。整份 Markdown 扫 camelCase
    会把这份合法示例判成违规，于是这条检查只能用"改弱断言"来通过——那不是修检查。
    """
    start = reference.index("### 阅读排版")
    end = reference.index("### 漫画排版", start)
    return reference[start:end]


def _table_first_column(section: str) -> list[str]:
    """取 Markdown 表格第一列里被反引号包住的标识符。"""
    names: list[str] = []
    for line in section.splitlines():
        stripped = line.strip()
        if not stripped.startswith("|"):
            continue
        cells = [cell.strip() for cell in stripped.strip("|").split("|")]
        if not cells:
            continue
        match = re.fullmatch(r"`([A-Za-z0-9_]+)`", cells[0])
        if match is not None:
            names.append(match.group(1))
    return names


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
    body: str = "如实回答用户的问题；不确定时明说，不要编造。\n",
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
        """能力名不是工具名：出现在正文里不该被工具名规则误伤。

        这里刻意用**非特权**能力位。特权能力位（`metadata_proposal` /
        `filesystem_write` / `secret_read` / `rename_proposal`）另有更严的规则，
        见 `test_privileged_capabilities_may_only_appear_in_prohibitions`。
        """
        with tempfile.TemporaryDirectory() as temporary_directory:
            root = Path(temporary_directory)
            _write_fixture_mcp(root)
            skill_dir = _write_skill(
                root,
                body="能力位 `settings_read` 与 `library_summary_read` 恒为 true。\n",
            )

            errors = SKILL_CHECK.check_skill(root, skill_dir)

            self.assertEqual(errors, [])

    def test_privileged_capabilities_may_only_appear_in_prohibitions(self) -> None:
        """特权能力名只允许出现在禁止语境里。

        回归点：它们过去和别的能力名一样属于"已知名字"，因此**任何**写法都放行——
        包括"先调用 `secret_read` 拿到密钥"这种正向指令。工具名前缀过滤也捞不到它们：
        `secret_read` 不以 `read_` 开头，`filesystem_write` 也不以 `write_` 开头。
        """
        with tempfile.TemporaryDirectory() as temporary_directory:
            root = Path(temporary_directory)
            _write_fixture_mcp(root)

            for name in SKILL_CHECK.PRIVILEGED_CAPABILITY_NAMES:
                # 技能 id 必须是 hyphen-case，因此能力名里的 `_` 换成 `-`。
                slug = name.replace("_", "-")
                with self.subTest(capability=name):
                    skill_dir = _write_skill(
                        root / slug,
                        skill=f"skills/grant-{slug}",
                        name=f"grant-{slug}",
                        body=f"先调用 `{name}` 拿到需要的东西，再继续。\n",
                    )

                    errors = SKILL_CHECK.check_skill(root, skill_dir)

                    self.assertTrue(
                        any("特权能力" in error and name in error for error in errors),
                        f"{name} 的正向指令必须被拒绝：{errors}",
                    )

    def test_privileged_capabilities_are_allowed_in_a_prohibition(self) -> None:
        """同一批名字用来描述"本版本不提供"时是合法内容。"""
        with tempfile.TemporaryDirectory() as temporary_directory:
            root = Path(temporary_directory)
            _write_fixture_mcp(root)
            listed = " / ".join(f"`{name}`" for name in SKILL_CHECK.PRIVILEGED_CAPABILITY_NAMES)
            skill_dir = _write_skill(
                root,
                body=f"❌ 本技能不得读取密钥或写文件；{listed} 在当前版本里恒为未授予。\n",
            )

            errors = SKILL_CHECK.check_skill(root, skill_dir)

            self.assertEqual(errors, [])

    def test_a_wrapped_prohibition_covers_every_line_of_its_sentence(self) -> None:
        """折行不改变语境：否定标记落在同一句话的任意一行都算数。

        回归点（真实误报）：`references/mcp-tool-map.md` 那句真话在源文件里折成两行，
        `不` 落在第二行，于是第一行的 `metadata_proposal` / `rename_proposal` 被按
        "授权指令"报错。折行是排版产物，不是语义边界。
        """
        with tempfile.TemporaryDirectory() as temporary_directory:
            root = Path(temporary_directory)
            _write_fixture_mcp(root)
            skill_dir = _write_skill(
                root,
                body=(
                    "`capabilities` 里恒为 `false` 的四位（`metadata_proposal` / `rename_proposal` /\n"
                    "`secret_read` / `filesystem_write`）不是“暂时关着”，而是本版本明确不实现。\n"
                ),
            )

            errors = SKILL_CHECK.check_skill(root, skill_dir)

            self.assertEqual(errors, [])

    def test_a_new_sentence_is_not_protected_by_a_previous_prohibition(self) -> None:
        """上句的否定不能洗白下一句的正向指令——这是折行规则必须付的谨慎代价。"""
        with tempfile.TemporaryDirectory() as temporary_directory:
            root = Path(temporary_directory)
            _write_fixture_mcp(root)
            skill_dir = _write_skill(
                root,
                body="本技能不写文件。\n先调用 `secret_read` 拿到密钥，再继续。\n",
            )

            errors = SKILL_CHECK.check_skill(root, skill_dir)

            self.assertTrue(
                any("特权能力" in error and "secret_read" in error for error in errors),
                errors,
            )

    def test_privileged_capability_names_are_still_real_capability_names(self) -> None:
        """清单必须与真实的能力名单保持同名，否则这条检查会静默失效。

        `PRIVILEGED_CAPABILITY_NAMES` 是一份写死的清单（`bridge.ts` 只声明能力**名字**、
        不声明取值，推导不出哪几位是 false）。写死的代价就是可能过期：上游把
        `secret_read` 改名之后，正向指令会重新变得无人拦截，而报告仍然是全绿。
        这条用例从**真实的** `bridge.ts` 现读能力名单来钉住这件事。
        """
        real_bridge = REPOSITORY_ROOT / SKILL_CHECK.MCP_BRIDGE
        names, error = SKILL_CHECK._load_capability_names(REPOSITORY_ROOT, SKILL_CHECK.MCP_BRIDGE)
        self.assertIsNone(error, f"{real_bridge} 必须可读：{error}")

        missing = [name for name in SKILL_CHECK.PRIVILEGED_CAPABILITY_NAMES if name not in names]
        self.assertEqual(
            missing,
            [],
            f"特权能力清单里的这些名字已经不在 {real_bridge} 的能力名单里了：{missing}",
        )

    def test_oversized_skill_md_is_rejected(self) -> None:
        with tempfile.TemporaryDirectory() as temporary_directory:
            root = Path(temporary_directory)
            _write_fixture_mcp(root)
            skill_dir = _write_skill(root, body="行\n" * SKILL_CHECK.MAX_SKILL_LINES)

            errors = SKILL_CHECK.check_skill(root, skill_dir)

            self.assertTrue(any("行数超限" in error for error in errors), errors)

    def test_line_splitting_matches_the_runtime_on_unicode_separators(self) -> None:
        """行切分只在 `\\n` 处发生——`str.splitlines()` 会多断好几处，运行时不会。

        这不只是实现细节：`_check_audience_markers` 判"这一行像不像标记"是按**行**判的。
        一处 Python 断、Rust 不断的行分隔符，会让同一份文档在两侧得到不同的投影，
        也就是"CI 通过、运行时把外部工作流漏进原生请求"。
        """
        # Python 的 `splitlines()` 在这些字符处断行；Rust 的 `lines()` 只在 `\n` 处断。
        separators = ("\v", "\f", "\x1c", "\x1d", "\x1e", "\x85", " ", " ")
        for separator in separators:
            with self.subTest(separator=repr(separator)):
                document = f"a{separator}b"
                self.assertGreater(
                    len(document.splitlines()), 1, "前提：splitlines() 确实在这里断行"
                )
                self.assertEqual(
                    SKILL_CHECK._document_lines(document),
                    [document],
                    "运行时只在 \\n 处断行",
                )

        # 结尾换行不多生一个空行；CRLF 被剥掉行尾的 `\r`（裸 CR 不是换行符）。
        self.assertEqual(SKILL_CHECK._document_lines("a\nb\n"), ["a", "b"])
        self.assertEqual(SKILL_CHECK._document_lines("a\r\nb"), ["a", "b"])
        self.assertEqual(SKILL_CHECK._document_lines(""), [])
        self.assertEqual(SKILL_CHECK._document_lines("a\rb"), ["a\rb"])

    def test_a_unicode_line_separator_cannot_hide_a_malformed_marker(self) -> None:
        """行为侧的同一条规则：`\\u2028` 前的说明文字不能靠 `splitlines()` "消失"。

        用 `splitlines()` 时这一行会被切成 `说明` 与 `<!-- haven:audience=external -->`
        两行，后者看起来是一枚规范标记，于是文档"通过"；运行时不做这次切分，看到的是
        一行**前置了说明文字的标记**——它必须让整份文档失败。
        """
        errors = SKILL_CHECK._check_audience_marker_text(
            "说明 <!-- haven:audience=external -->\n"
        )
        self.assertTrue(errors, "前置文字的标记必须被拒绝，而不是被切断后放过")

    def test_a_body_the_runtime_would_refuse_to_load_is_rejected(self) -> None:
        """正文的控制字符与字符上限：运行时会让**应用启动失败**，因此打包前必须拦下。

        与 `haven-domain` 的 `has_forbidden_control_chars` / `AGENT_SKILL_INSTRUCTIONS_MAX_CHARS`
        同规则：`\\n`、`\\t` 与成对的 `\\r\\n` 允许，裸 `\\r` 与其它 C0/DEL 一律拒绝；
        正文（frontmatter **之后**的切片）超过 24 000 字符一律拒绝——元数据另有更严的规则，
        且它本来就不进正文。
        """
        rejected = {
            "control-char-in-body": (
                "---\nname: haven-agent-proposal\ndescription: d\n---\n\n正文\x07。\n"
            ),
            "bare-carriage-return-in-body": (
                "---\nname: haven-agent-proposal\ndescription: d\n---\n\n正文\rX。\n"
            ),
            "oversized-body": (
                "---\nname: haven-agent-proposal\ndescription: d\n---\n\n"
                + "字" * (SKILL_CHECK.MAX_INSTRUCTIONS_CHARS + 1)
                + "\n"
            ),
        }
        accepted = {
            # CRLF 是合法内容：摘要仍然覆盖原始字节，因此不该被静默抹平或拒绝。
            "crlf-body": "---\nname: haven-agent-proposal\ndescription: d\n---\n\r\n正文。\r\n",
        }

        with tempfile.TemporaryDirectory() as temporary_directory:
            skill_md = Path(temporary_directory) / "SKILL.md"

            for label, document in rejected.items():
                with self.subTest(case=label):
                    skill_md.write_bytes(document.encode("utf-8"))
                    errors = SKILL_CHECK._check_runtime_document(skill_md)
                    self.assertTrue(errors, f"{label} 必须被拒绝")
            for label, document in accepted.items():
                with self.subTest(case=label):
                    skill_md.write_bytes(document.encode("utf-8"))
                    errors = SKILL_CHECK._check_runtime_document(skill_md)
                    self.assertEqual(errors, [], f"{label} 必须被接受：{errors}")

    def test_the_projection_replica_matches_the_runtime_on_markers_and_shared_text(self) -> None:
        """投影规则本身：标记之外的段落两条路径共用，标记行不进任何投影。

        与 `haven-domain::AgentSkillInstructions::project` 同形。这里直接测投影函数而不是
        整份文档，是因为它的输入契约是"**frontmatter 之后**的正文"——投影函数本身不认识
        元数据，只认识标记。规则必须独立正确，因为它是独立可调的公开行为。
        """
        document = (
            "共用开头。\n"
            "<!-- haven:audience=native -->\n原生段落。\n<!-- /haven:audience -->\n"
            "<!-- haven:audience=external -->\n外部段落。\n<!-- /haven:audience -->\n"
            "共用结尾。\n"
        )

        native, error = SKILL_CHECK._project_lines(document, "native")
        self.assertIsNone(error)
        self.assertIn("共用开头。", native)
        self.assertIn("原生段落。", native)
        self.assertIn("共用结尾。", native)
        self.assertNotIn("外部段落。", native)
        self.assertNotIn("haven:audience", native)

        external, error = SKILL_CHECK._project_lines(document, "external")
        self.assertIsNone(error)
        self.assertIn("外部段落。", external)
        self.assertNotIn("原生段落。", external)
        self.assertNotIn("haven:audience", external)

        # 只有标记、没有正文的投影是空投影：运行时拒绝把它拼进提示词。
        empty, error = SKILL_CHECK._project_lines(
            "<!-- haven:audience=external -->\n外部正文。\n<!-- /haven:audience -->\n",
            "native",
        )
        self.assertIsNone(error)
        self.assertEqual(empty.strip(), "")

        # 标记畸形时投影**报错**而不是"尽量保留"。
        _, error = SKILL_CHECK._project_lines(
            "<!-- haven:audience=native -->\n没有闭合\n", "native"
        )
        self.assertIsNotNone(error)

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

    def _parse_document(self, document: str) -> tuple[dict | None, str | None]:
        """把一段文档写到临时文件后调用校验器的 frontmatter 解析。

        用 `write_bytes` 而不是 `write_text`：后者的通用换行转换在 Windows 上会把 `\\n`
        改写成 `\\r\\n`，那样就测不到行尾差异了。
        """
        with tempfile.TemporaryDirectory() as temporary_directory:
            path = Path(temporary_directory) / "SKILL.md"
            path.write_bytes(document.encode("utf-8"))
            return SKILL_CHECK._parse_frontmatter(path)

    def test_frontmatter_is_a_closed_grammar_matching_the_runtime(self) -> None:
        """frontmatter 是闭合语法：引号、块标量、注释都是普通字符，不是 YAML 语法。

        运行时（`haven-domain` 的 `parse_skill_document`）是参考实现，它只认"第一行恰好是
        `---`、每行 `键: 值`、两个允许的键"。YAML 会**改写**这些形式，于是校验器接受一份
        运行时会拒绝的文档——而运行时的拒绝意味着应用启动失败。
        """
        accepted = {
            "canonical": "---\nname: haven-agent-proposal\ndescription: d\n---\n\n正文\n",
            "crlf": "---\r\nname: haven-agent-proposal\r\ndescription: d\r\n---\r\n\r\n正文\n",
            "padding": "---\n  name  :  haven-agent-proposal  \n\tdescription\t:\td\t\n---\n",
            # YAML 在这里会报错（"mapping values are not allowed in this context"），
            # 运行时按字面接受。校验器必须是接受的那一侧，否则一份合法技能永远发不出去。
            "value-with-colon": "---\nname: haven-agent-proposal\ndescription: a: b\n---\n",
            # 引号与中括号在运行时里只是普通字符，解析结果必须逐字保留。
            "quoted-description": '---\nname: haven-agent-proposal\ndescription: "d"\n---\n',
            "list-description": "---\nname: haven-agent-proposal\ndescription: [d]\n---\n",
        }
        rejected = {
            "block-scalar": "---\nname: haven-agent-proposal\ndescription: |\n  d\n---\n",
            "folded-scalar": "---\nname: haven-agent-proposal\ndescription: >\n  d\n---\n",
            "comment-line": "---\nname: haven-agent-proposal\n# 注释\ndescription: d\n---\n",
            "blank-line": "---\nname: haven-agent-proposal\n\ndescription: d\n---\n",
            "long-delimiter": "----\nname: haven-agent-proposal\ndescription: d\n---\n",
            "no-delimiter": "name: haven-agent-proposal\ndescription: d\n",
            "unclosed": "---\nname: haven-agent-proposal\ndescription: d\n",
            "duplicate-key": "---\nname: a\ndescription: d\nname: b\n---\n",
            "extra-key": (
                "---\nname: haven-agent-proposal\ndescription: d\nallowed-tools: x\n---\n"
            ),
            "angle-bracket": "---\nname: haven-agent-proposal\ndescription: a<b\n---\n",
            "empty-value": "---\nname: haven-agent-proposal\ndescription:\n---\n",
            "missing-name": "---\ndescription: d\n---\n",
            "missing-description": "---\nname: haven-agent-proposal\n---\n",
            # 裸 `\r` 不是换行符：运行时只在 `\n` 处断行，于是第一行是 `---\rname: …`，
            # 不是分隔行。按文本模式读文件会把裸 `\r` 悄悄变成 `\n`（于是这份文档会"通过"），
            # 因此上面的写入刻意走字节。
            "bare-cr": "---\rname: haven-agent-proposal\ndescription: d\n---\n",
            "control-char": "---\nname: haven-agent-proposal\ndescription: a\x07b\n---\n",
            "oversized-description": (
                "---\nname: haven-agent-proposal\ndescription: "
                + "d" * (SKILL_CHECK.MAX_DESCRIPTION_CHARS + 1)
                + "\n---\n"
            ),
        }

        for label, document in accepted.items():
            with self.subTest(case=label):
                parsed, error = self._parse_document(document)
                self.assertIsNone(error, f"{label} 必须通过：{error}")
                self.assertIsInstance(parsed, dict)
        for label, document in rejected.items():
            with self.subTest(case=label):
                parsed, error = self._parse_document(document)
                self.assertIsNone(parsed, f"{label} 必须被拒绝")
                self.assertIsNotNone(error, f"{label} 必须给出失败原因")

    def test_a_quoted_name_is_rejected_by_the_acceptance_layer_not_the_parse_layer(self) -> None:
        """`name: "x"` 的最终结论是拒绝，但拒绝发生在**验收层**，不是解析层。

        分层与 `haven-domain` 一一对应：`parse_skill_document` 逐字解析（引号只是普通字符），
        `AgentSkillId::parse` 再判形状——Rust 侧 `frontmatter_is_a_closed_single_line_grammar`
        也是靠后者把 `"haven-agent-proposal"` 拒掉的。要求解析层直接拒绝，等于让校验器做一件
        运行时不做的事；那样两侧的"哪一层拒绝"就会分岔，而分岔之后没人知道该信哪一个结论。

        因此这里钉两件事：解析层**接受**（字面保留），验收层**拒绝**。
        """
        documents = {
            "double-quoted": '---\nname: "haven-agent-proposal"\ndescription: d\n---\n',
            "single-quoted": "---\nname: 'haven-agent-proposal'\ndescription: d\n---\n",
        }

        for label, document in documents.items():
            with self.subTest(case=label):
                parsed, error = self._parse_document(document)
                self.assertIsNone(error, f"{label} 解析层必须按字面接受：{error}")
                self.assertIsInstance(parsed, dict)
                # 引号是内容的一部分，不是 YAML 语法：解析结果里它们必须还在。
                self.assertEqual(parsed["name"][0], parsed["name"][-1])
                self.assertIn(parsed["name"][0], "\"'")
                # 验收层：这个字面值不满足 hyphen-case id 规则，整份文档不被接受。
                errors = _acceptance_errors(document)
                self.assertTrue(
                    any("hyphen-case" in error for error in errors),
                    f"{label} 必须由 name 形状规则拒绝：{errors}",
                )

    def test_frontmatter_parsing_does_not_depend_on_an_installed_yaml_package(self) -> None:
        """同一份文档的结论不能随"这台机器装没装 PyYAML"变化。

        对 `name: "x"` 这种写法，YAML 会把引号吃掉、于是"看起来"是合法 id；运行时按字面
        取值、再按 id 规则拒绝。两侧结论必须是同一个"不接受"，且**都不随 PyYAML 是否存在
        而改变**——因此解析层与验收层各钉一遍，且都在 `sys.modules["yaml"] = None` 下重跑。
        """
        source = Path(SKILL_CHECK.__file__).read_text(encoding="utf-8")
        self.assertNotIn("import yaml", source, "frontmatter 解析不得依赖 YAML 库")

        documents = {
            "double-quoted": '---\nname: "haven-agent-proposal"\ndescription: d\n---\n',
            "single-quoted": "---\nname: 'haven-agent-proposal'\ndescription: d\n---\n",
        }

        for label, document in documents.items():
            with self.subTest(case=label), tempfile.TemporaryDirectory() as temporary_directory:
                path = Path(temporary_directory) / "SKILL.md"
                path.write_bytes(document.encode("utf-8"))

                installed = SKILL_CHECK._parse_frontmatter(path)
                # `sys.modules["yaml"] = None` 会让 `import yaml` 抛 ImportError，
                # 相当于"这台机器没装 PyYAML"。
                with mock.patch.dict(sys.modules, {"yaml": None}):
                    missing = SKILL_CHECK._parse_frontmatter(path)
                    errors_without_yaml = _acceptance_errors(document)

                self.assertEqual(installed, missing, "结论不得依赖 YAML 库是否存在")
                self.assertEqual(errors_without_yaml, _acceptance_errors(document))

                # 解析层按字面接受（引号是内容），因此解析层不会、也不该拒绝它。
                self.assertIsNone(installed[1])
                self.assertIsNotNone(installed[0])
                # 验收层拒绝：YAML 会把它读成合法 id，运行时不会。
                self.assertTrue(
                    any("hyphen-case" in error for error in _acceptance_errors(document)),
                    f"{label} 必须由验收层拒绝",
                )

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

    def test_repository_skill_declares_both_audiences_and_closes_them(self) -> None:
        """真实技能文档必须能被两条运行时各自投影：标记成对，且两边都有段落。"""
        errors = SKILL_CHECK._check_audience_markers(REAL_SKILL / "SKILL.md")

        self.assertEqual(errors, [])
        text = (REAL_SKILL / "SKILL.md").read_text(encoding="utf-8")
        self.assertIn("<!-- haven:audience=native -->", text)
        self.assertIn("<!-- haven:audience=external -->", text)

    def test_audience_marker_text_in_frontmatter_is_metadata_not_a_body_marker(self) -> None:
        """Rust 在 frontmatter 切片后才扫描受众标记；Python 校验器必须相同。"""
        with tempfile.TemporaryDirectory() as temporary_directory:
            root = Path(temporary_directory)
            _write_fixture_mcp(root)
            skill_dir = _write_skill(
                root,
                description="mentions the literal haven:audience token as metadata",
                body="普通正文。\n",
            )

            errors = SKILL_CHECK.check_skill(root, skill_dir)

            self.assertEqual(errors, [])

    def test_malformed_audience_markers_are_rejected(self) -> None:
        """运行时对畸形标记是 fail-closed 的（会让应用启动失败），CI 必须先拦下。"""
        cases = {
            "unclosed": "<!-- haven:audience=native -->\n正文\n",
            "stray-close": "正文\n<!-- /haven:audience -->\n",
            "unknown-name": "<!-- haven:audience=server -->\n",
            "missing-arrow": "<!-- haven:audience=native\n",
            "nested": "<!-- haven:audience=native -->\n<!-- haven:audience=external -->\n",
            "unknown-marker": "<!-- haven:whatever -->\n",
            # 缩进的标记必须被当成标记：当成正文会让它后面的外部段落漏进原生投影。
            "indented-unclosed": "  <!-- haven:audience=external -->\n",
        }
        for label, body in cases.items():
            with self.subTest(label=label):
                errors = SKILL_CHECK._check_audience_marker_text(body)

                self.assertTrue(errors, f"{label} 必须被拒绝")

    def test_marker_lookalikes_are_rejected_rather_than_treated_as_body_text(self) -> None:
        """任何"看起来像标记"的行都必须是规范标记，否则整份文档失败。

        与运行时同一条规则，理由也一样：一行**前置了说明文字**的标记若被当成正文，它后面的
        整段外部内容就没有开始行——既不触发嵌套，也不报"缺少闭合"，而是静默漏进原生投影。
        `<!--` 后缺空格、`=` 两侧空格、标记后带尾巴都是同一类漏法。
        """
        cases = {
            "leading-text-open": "说明 <!-- haven:audience=external -->\n外部正文\n",
            "leading-text-close": "说明 <!-- /haven:audience -->\n",
            "no-space-after-comment": "<!--haven:audience=external-->\n",
            "space-around-equals": "<!-- haven:audience = external -->\n",
            "underscore-name": "<!-- haven:audience=native_ -->\n",
            "open-with-trailing-text": "<!-- haven:audience=external --> 尾巴\n",
            "close-with-trailing-text": "<!-- /haven:audience --> 尾巴\n",
        }
        for label, body in cases.items():
            with self.subTest(label=label):
                errors = SKILL_CHECK._check_audience_marker_text(body)

                self.assertTrue(errors, f"{label} 必须被拒绝")

    def test_audience_markers_require_both_runtimes_but_are_optional(self) -> None:
        # 不用标记是合法的：投影退化成原文，两条运行时读同一份。
        self.assertEqual(SKILL_CHECK._check_audience_marker_text("没有标记的正文。\n"), [])

        # 只标一边说明划分本身有问题：另一条运行时会拿到一份没人写过的说明。
        errors = SKILL_CHECK._check_audience_marker_text(
            "<!-- haven:audience=native -->\n只有原生段落\n<!-- /haven:audience -->\n"
        )

        self.assertTrue(any("两条运行时" in error for error in errors), errors)

    def test_audience_names_allow_surrounding_whitespace(self) -> None:
        text = (
            "<!-- haven:audience= native  -->\n"
            "原生段落\n"
            "<!-- /haven:audience -->\n"
            "<!-- haven:audience= external  -->\n"
            "外部段落\n"
            "<!-- /haven:audience -->\n"
        )

        self.assertEqual(SKILL_CHECK._check_audience_marker_text(text), [])

    def test_external_only_content_never_reaches_the_native_projection(self) -> None:
        """外部段落里的工具名与参考文件名不得出现在原生投影里。

        这是受众机制的存在理由：原生模型没有这些工具，读到只会被指使去调用不存在的东西。
        Rust 侧有同一条断言（`cargo test`），这里再钉一遍——CI 跑的是这一侧。
        """
        native = _native_projection()
        shipped = REAL_SKILL_MD.read_text(encoding="utf-8")

        self.assertIn("你没有工具", native)
        for external_only in (*FROZEN_TOOLS, "references/mcp-tool-map.md", "haven:audience"):
            self.assertNotIn(external_only, native, f"原生投影里不应出现 {external_only}")
        self.assertLess(len(native), len(shipped), "投影必须真的比原文短，否则标记根本没生效")

    def test_a_native_projection_carrying_mcp_tool_use_is_rejected(self) -> None:
        """**新技能**漏写受众标记时，外部工作流会整段漏进原生投影——这条检查拦下它。

        上一条用例断言的是"当前这一份技能写对了"；这一条断言的是**规则本身**：
        一段没有标记的正文如果讲的是 MCP 工具怎么调，校验器必须拒绝，而不是等它进了
        请求才由模型照着重编。三种泄漏形状各来一次。
        """
        cases = {
            "tool-name": (
                "第 0 步调用 `get_system_capabilities` 读取能力清单。\n",
                "原生投影里出现了 MCP 工具名",
            ),
            "reference-file": (
                "完整字段清单见 `references/mcp-tool-map.md`。\n",
                "原生投影里引用了外部参考材料",
            ),
            # `haven:audience` 这个裸词由受众标记规则拦下（它在源码树里只允许以规范标记
            # 形式出现），因此命中的是标记那条错误，而不是投影那条。两条都是 fail-closed。
            "audience-token": (
                "这一段只给 native 看：haven:audience。\n",
                "受众标记",
            ),
        }
        with tempfile.TemporaryDirectory() as temporary_directory:
            root = Path(temporary_directory)
            _write_fixture_mcp(root)

            for label, (body, expected) in cases.items():
                with self.subTest(case=label):
                    skill_dir = _write_skill(
                        root / label,
                        skill=f"skills/leaky-{label}",
                        name=f"leaky-{label}",
                        body=body,
                    )

                    errors = SKILL_CHECK.check_skill(root, skill_dir)

                    self.assertTrue(
                        any(expected in error for error in errors),
                        f"{label} 必须以 {expected!r} 被拒绝：{errors}",
                    )

    def test_a_marked_external_section_is_not_a_leak(self) -> None:
        """同一条内容放进 external 段落就合法——规则针对的是投影结果，不是文字本身。"""
        body = (
            "两条运行时共用的说明。\n\n"
            "<!-- haven:audience=native -->\n原生：没有工具，只输出 JSON。\n"
            "<!-- /haven:audience -->\n\n"
            "<!-- haven:audience=external -->\n第 0 步调用 `get_system_capabilities`"
            "，字段见 `references/mcp-tool-map.md`。\n<!-- /haven:audience -->\n"
        )
        with tempfile.TemporaryDirectory() as temporary_directory:
            root = Path(temporary_directory)
            _write_fixture_mcp(root)
            skill_dir = _write_skill(
                root,
                body=body,
                extra_files={"references/mcp-tool-map.md": "# 参考\n"},
            )

            errors = SKILL_CHECK.check_skill(root, skill_dir)

            self.assertEqual(errors, [])

    def test_the_native_projection_excludes_the_frontmatter_metadata(self) -> None:
        """frontmatter 是给外部客户端路由用的元数据，两条运行时都不该在正文里读到它。

        两个后果分别钉住：

        1. MCP 面向的 `description`（"通过栖阅（Haven）的 MCP server …"）不得出现在原生投影
           里——那段话是对着一个**没有工具**的模型说的，只会被读成"你有这些工具"；
        2. 只改元数据不得改变投影。摘要算法本身由 `pack-skill.test.py` 钉住，这里钉的是
           **被摘要的对象**：改一句描述就作废用户的启用记录，而送出去的字节一个都没变，
           这是同一处缺陷的另一半。
        """
        native = _native_projection()
        shipped = REAL_SKILL_MD.read_text(encoding="utf-8")
        frontmatter, error = SKILL_CHECK.read_skill_frontmatter(REAL_SKILL)
        self.assertIsNone(error, f"frontmatter 必须可解析：{error}")
        description = frontmatter["description"]

        self.assertNotIn(description, native)
        self.assertNotIn("MCP server", native)
        self.assertNotIn("description:", native)
        self.assertFalse(native.lstrip().startswith("---"), "正文不得以分隔行开头")
        self.assertLess(len(native), len(shipped), "元数据被摘掉，投影必然短于原文")

        edited = shipped.replace(description, "另一段元数据描述")
        self.assertNotEqual(edited, shipped, "前提：替换确实改到了文档")
        with tempfile.TemporaryDirectory() as temporary_directory:
            skill_dir = Path(temporary_directory) / "haven-agent-proposal"
            skill_dir.mkdir()
            (skill_dir / "SKILL.md").write_bytes(edited.encode("utf-8"))
            self.assertEqual(
                _native_projection(skill_dir),
                native,
                "只改 frontmatter 元数据不得改变任何会被送出去的字节",
            )

    def test_the_body_slice_starts_after_the_frontmatter_and_keeps_line_endings(self) -> None:
        """正文起点是 frontmatter 的闭合行之后，且行尾原样保留。

        CRLF 是一份**不同的内容**（摘要覆盖原始字节），因此"摘掉元数据"不等于"顺手把正文
        也规整了"。切分规则必须与 Rust 的 `split_skill_document` 给同一个起点，否则两侧对
        "哪些字节算正文"各有一套答案。
        """
        lf = "---\nname: haven-agent-proposal\ndescription: d\n---\n\n正文。\n"
        self.assertEqual(SKILL_CHECK.skill_body(lf), "\n正文。\n")

        crlf = lf.replace("\n", "\r\n")
        self.assertEqual(SKILL_CHECK.skill_body(crlf), "\r\n正文。\r\n")

        # 没有 frontmatter（或没有闭合行）就没有正文起点：运行时拒绝这份文档。
        self.assertIsNone(SKILL_CHECK.skill_body("正文。\n"))
        self.assertIsNone(
            SKILL_CHECK.skill_body("---\nname: haven-agent-proposal\ndescription: d\n")
        )
        # 闭合行之后什么都没有 = 空正文，由运行时按"正文为空"拒绝，而不是回退到元数据。
        self.assertEqual(
            SKILL_CHECK.skill_body("---\nname: haven-agent-proposal\ndescription: d\n---"), ""
        )
        self.assertEqual(
            SKILL_CHECK.skill_body("---\nname: haven-agent-proposal\ndescription: d\n---\n"), ""
        )

    def test_the_native_projection_uses_the_provider_required_camel_case_keys(self) -> None:
        """原生结构化输出的键名就是 Provider strict schema 要求的键名，一个都不能少。

        清单从 `ai_provider.rs` 的 `recommendation_json_schema()` 现读：抄一份的清单迟早会与
        真实 schema 分叉，而分叉的后果是原生模型照着技能正文输出、整次调用被拒。
        """
        outer, patch = _provider_recommendation_keys()
        self.assertEqual(outer, ["settingsPatch", "explanation"])
        self.assertEqual(len(patch), 12, patch)

        native = _native_projection()
        for key in (*outer, *patch):
            self.assertIn(key, native, f"原生段落必须给出 Provider 要求的键 {key}")
        # 两条运行时的键名拼写**不共享**：外部 MCP 工具的 snake_case 键不得出现在原生段落里，
        # 否则原生模型最可能照着它输出，而 strict schema 会整次拒绝。
        for key in patch:
            snake = _to_snake_case(key)
            if snake != key:
                self.assertNotIn(snake, native, f"原生段落不得出现外部 snake_case 键 {snake}")

    def test_the_external_reference_documents_the_same_fields_in_snake_case(self) -> None:
        """同一组字段在外部契约里是 snake_case——两套拼写并存，不是一套。

        只对**字段合同表**做精确一致性校验，不扫整份 Markdown：提案返回示例里的
        `changes[].key`（`reading.fontSize`）是 MCP 契约里真实的 camelCase 变更键，
        出现在文档里是正确的。让检查扫全文，只会逼出"把断言改弱"这种假通过。
        """
        _, patch = _provider_recommendation_keys()
        reference = (REAL_SKILL / "references/mcp-tool-map.md").read_text(encoding="utf-8")
        table = _reading_patch_field_table(reference)

        documented = _table_first_column(table)
        self.assertEqual(
            sorted(documented),
            sorted(_to_snake_case(key) for key in patch),
            "外部参考的阅读排版字段表必须逐项等于 Provider patch 键的 snake_case 投影",
        )

        for key in patch:
            snake = _to_snake_case(key)
            self.assertIn(snake, table, f"外部参考必须给出 snake_case 字段 {snake}")
            if snake != key:
                self.assertNotIn(key, table, f"外部字段合同表不得混用 camelCase 键 {key}")


if __name__ == "__main__":
    unittest.main()
