#!/usr/bin/env python3
"""仓库内 Skill 的契约校验（确定性、无网络、可重复）。

校验对象是 `skills/haven-agent-proposal/`，但除 `--skill` 参数外的规则都是通用的：
一个新 Skill 只要复用它，就能挡住同样几类错误。

规则（每条都对应一个具体的、会真实发生的失败）：

1. frontmatter：`name` / `description` 存在、类型正确、`name` 是 hyphen-case 且 ≤64 字符、
   `description` ≤1024 字符且不含尖括号；只允许这两个键（与 skill-creator 规范一致）。
2. `SKILL.md` 正文 < 500 行（skill-creator 的 progressive disclosure 上限）。
3. `evals/evals.json` 可解析、`skill_name` 与 frontmatter 的 `name` 一致、至少 3 条 eval、
   每条都有整数 `id`（唯一、非 bool）与非空的 `prompt` / `expected_output` / `expectations`。
4. **工具形标识符白名单**：技能包里每一个"像工具名"的反引号标识符，都必须落在
   **两份权威来源的并集**里（实现即 `known = set(frozen) | capabilities`）：
   - MCP 冻结的工具清单 —— 从 `mcp/haven-mcp/src/constants.ts` 的 `TOOL_NAMES` 现读；
   - Haven 已知的能力名（`settings_read` / `metadata_proposal` / `filesystem_write` …）——
     从 `mcp/haven-mcp/src/bridge.ts` 的 `HavenCapabilityReport.capabilities` 现读。

   两者都**不**在本文件里另抄一份：两份硬编码的清单迟早会分叉，而那正是这条检查要防的事。
   之所以要把能力名一起收进白名单，是因为它们和工具名长得一样（`metadata_proposal`、
   `secret_read`），但出现在技能文档里是正确且有信息量的（用来解释"哪些能力位恒为 false"）。
   只认工具清单会让这条检查对能力名误报，而一个会误报的检查很快就会被忽略。
5. **不得授予写权限**：白名单之外的工具名只能出现在**禁止语境**里。判断依据是该行
   是否含否定标记（`不` / `禁止` / `❌` / `不得` / `永不` / `拒绝` / `never` / `NOT`，
   即 `NEGATION_MARKERS` 元组的全部 8 项）。
   这样"❌ 做 `metadata_patch`"是合法的说明，而"调用 `apply_settings`"会被抓住。

   这是护栏而非证明：一行授予性指令若恰好含否定标记（例如"不确定时调用 `apply_settings`"）
   会被判为禁止说明而放过。真正的权限边界在 MCP 工具清单本身——那 9 个工具里没有写方法。

常用用法（示例，不是参数全集；完整参数见 `--help`）：
    python tools/skills/skill-contract-check.py                    # 默认校验本仓库的 skill
    python tools/skills/skill-contract-check.py --skill skills/xxx  # 校验指定 skill
    python tools/skills/skill-contract-check.py --mcp-root 后端/... # 指定 MCP 常量文件（测试用）
    python tools/skills/skill-contract-check.py --mcp-bridge 后端/... # 指定 MCP 端口文件（测试用）

退出码：0 = 全部通过；1 = 有失败项。
"""

from __future__ import annotations

import argparse
import json
import re
import sys
from pathlib import Path
from typing import Any, Iterable

# ---- 常量 ----

DEFAULT_SKILL = "skills/haven-agent-proposal"
MCP_CONSTANTS = "mcp/haven-mcp/src/constants.ts"
MCP_BRIDGE = "mcp/haven-mcp/src/bridge.ts"

MAX_SKILL_LINES = 500
MAX_NAME_CHARS = 64
MAX_DESCRIPTION_CHARS = 1024
MIN_EVALS = 3

ALLOWED_FRONTMATTER_KEYS = {"name", "description"}
NAME_PATTERN = re.compile(r"^[a-z0-9-]+$")

# 否定标记：出现即认为该行是"禁止说明"而不是"授权指令"。
NEGATION_MARKERS = ("不", "禁止", "❌", "不得", "永不", "拒绝", "never", "NOT")

# "像工具名"的前缀。反引号里的 `font_size` / `context_id` / `patch` 不是工具名，
# 但 `apply_settings` / `get_secret` / `metadata_patch` 必须是——或者必须被禁掉。
TOOL_NAME_PREFIXES = (
    "get_",
    "propose_",
    "set_",
    "list_",
    "read_",
    "write_",
    "apply_",
    "approve_",
    "reject_",
    "delete_",
    "update_",
    "create_",
    "remove_",
    "run_",
    "exec_",
    "invoke_",
    "patch_",
    "metadata_",
    "file_",
    "rename_",
    "move_",
    "query_",
    "search_",
)

CODE_SPAN = re.compile(r"`([^`\n]+)`")
TOOL_SHAPED = re.compile(r"^[a-z][a-z0-9_]*$")

# 非 ASCII 的否定词已经覆盖中文；这里补几个英文/符号形式由 NEGATION_MARKERS 处理。


def _load_frozen_tools(root: Path, mcp_constants: str) -> tuple[list[str], str | None]:
    """从 MCP 源码现读冻结的工具清单。

    刻意不在这里再写一份：两份清单会分叉，而分叉之后这条检查就变成了自我印证。
    """
    path = root / mcp_constants
    try:
        text = path.read_text(encoding="utf-8")
    except (OSError, UnicodeError) as error:
        return [], f"{mcp_constants}: 无法读取 MCP 常量文件（{error}）"

    match = re.search(r"export const TOOL_NAMES = \[(.*?)\] as const;", text, re.DOTALL)
    if match is None:
        return [], f"{mcp_constants}: 找不到 TOOL_NAMES 声明"

    tools = re.findall(r'"([a-z0-9_]+)"', match.group(1))
    if len(tools) != 9:
        return [], f"{mcp_constants}: TOOL_NAMES 期望 9 项，实际 {len(tools)} 项"
    return tools, None


def _load_capability_names(root: Path, mcp_bridge: str) -> tuple[set[str], str | None]:
    """从 MCP 的端口类型现读能力名。

    为什么需要：`metadata_proposal` / `rename_proposal` / `secret_read` /
    `filesystem_write` 这些是**能力位**的名字，不是工具名。它们出现在技能文档里
    是正确且有信息量的（用来解释"能力清单里恒为 false 的四位"）。把它们当成
    "自造工具"会让检查变成噪音，而一个会误报的检查很快就会被忽略。
    """
    path = root / mcp_bridge
    try:
        text = path.read_text(encoding="utf-8")
    except (OSError, UnicodeError) as error:
        return set(), f"{mcp_bridge}: 无法读取 MCP 端口文件（{error}）"

    match = re.search(
        r"export interface HavenCapabilityReport \{[^}]*?capabilities:\s*\{(.*?)\};",
        text,
        re.DOTALL,
    )
    if match is None:
        return set(), f"{mcp_bridge}: 找不到 HavenCapabilityReport.capabilities"
    return set(re.findall(r"(\w+)\s*:\s*boolean", match.group(1))), None


def _parse_frontmatter(skill_md: Path) -> tuple[dict[str, Any] | None, str | None]:
    """解析 YAML frontmatter。

    优先用 PyYAML（权威解析）；环境中没有时退回**严格**的两键单行解析器——
    宁可因为格式不符而报错，也不要宽松解析出一个自己都不确定的结论。
    """
    try:
        text = skill_md.read_text(encoding="utf-8")
    except (OSError, UnicodeError) as error:
        return None, f"SKILL.md 无法读取（{error}）"

    if not text.startswith("---"):
        return None, "SKILL.md 缺少 YAML frontmatter"
    match = re.match(r"^---\r?\n(.*?)\r?\n---", text, re.DOTALL)
    if match is None:
        return None, "SKILL.md 的 frontmatter 格式不合法"
    body = match.group(1)

    try:
        import yaml  # type: ignore[import-not-found]
    except ImportError:
        parsed: dict[str, Any] = {}
        for line in body.splitlines():
            if not line.strip() or line.lstrip().startswith("#"):
                continue
            key, separator, value = line.partition(":")
            if not separator:
                return None, f"frontmatter 行无法解析：{line[:60]!r}"
            parsed[key.strip()] = value.strip()
        return parsed, None

    try:
        value = yaml.safe_load(body)
    except yaml.YAMLError as error:  # type: ignore[attr-defined]
        return None, f"frontmatter YAML 不合法（{error}）"
    if not isinstance(value, dict):
        return None, "frontmatter 必须是键值映射"
    return value, None


def _check_frontmatter(frontmatter: dict[str, Any]) -> list[str]:
    errors: list[str] = []

    unexpected = sorted(set(frontmatter) - ALLOWED_FRONTMATTER_KEYS)
    if unexpected:
        errors.append(f"frontmatter 出现多余键：{', '.join(unexpected)}")

    name = frontmatter.get("name")
    if not isinstance(name, str) or not name.strip():
        errors.append("frontmatter 缺少非空字符串 name")
    else:
        name = name.strip()
        if not NAME_PATTERN.match(name):
            errors.append(f"name {name!r} 必须是 hyphen-case（小写字母/数字/连字符）")
        if name.startswith("-") or name.endswith("-") or "--" in name:
            errors.append(f"name {name!r} 不能以连字符开头/结尾或含连续连字符")
        if len(name) > MAX_NAME_CHARS:
            errors.append(f"name 超长（{len(name)}/{MAX_NAME_CHARS}）")

    description = frontmatter.get("description")
    if not isinstance(description, str) or not description.strip():
        errors.append("frontmatter 缺少非空字符串 description")
    else:
        if len(description) > MAX_DESCRIPTION_CHARS:
            errors.append(f"description 超长（{len(description)}/{MAX_DESCRIPTION_CHARS}）")
        if "<" in description or ">" in description:
            errors.append("description 不能包含尖括号")

    return errors


def _iter_text_files(skill_dir: Path) -> Iterable[Path]:
    for path in sorted(skill_dir.rglob("*")):
        if path.is_file() and path.suffix in {".md", ".json"}:
            yield path


def _check_tool_names(skill_dir: Path, frozen: list[str], capabilities: set[str]) -> list[str]:
    """反引号里"像工具名"的标识符必须来自冻结清单 / 能力名清单，或在禁止语境里。"""
    errors: list[str] = []
    known = set(frozen) | capabilities

    for path in _iter_text_files(skill_dir):
        try:
            lines = path.read_text(encoding="utf-8").splitlines()
        except (OSError, UnicodeError) as error:
            errors.append(f"{path.name}: 无法读取（{error}）")
            continue

        for number, line in enumerate(lines, start=1):
            for token in CODE_SPAN.findall(line):
                candidate = token.strip()
                if not TOOL_SHAPED.match(candidate):
                    continue
                if not candidate.startswith(TOOL_NAME_PREFIXES):
                    continue
                if candidate in known:
                    continue
                if any(marker in line for marker in NEGATION_MARKERS):
                    continue
                errors.append(
                    f"{path.relative_to(skill_dir)}:{number}: 工具形标识符 {candidate!r} "
                    f"既不在 MCP 冻结的 9 项里，也不是已知能力名，且该行不是禁止说明"
                )

    return errors


def _check_evals(skill_dir: Path, skill_name: str, frozen: list[str]) -> list[str]:
    errors: list[str] = []
    evals_path = skill_dir / "evals" / "evals.json"
    if not evals_path.exists():
        return [f"缺少 {evals_path.relative_to(skill_dir)}"]

    try:
        payload = json.loads(evals_path.read_text(encoding="utf-8"))
    except (OSError, UnicodeError, json.JSONDecodeError) as error:
        return [f"evals/evals.json 无法解析（{error}）"]

    if not isinstance(payload, dict):
        return ["evals/evals.json 顶层必须是对象"]

    declared = payload.get("skill_name")
    if declared != skill_name:
        errors.append(f"evals.skill_name {declared!r} 与 frontmatter name {skill_name!r} 不一致")

    evals = payload.get("evals")
    if not isinstance(evals, list):
        return errors + ["evals 必须是数组"]
    if len(evals) < MIN_EVALS:
        errors.append(f"eval 数量不足（{len(evals)}/{MIN_EVALS}）")

    seen_ids: set[int] = set()
    for index, entry in enumerate(evals):
        label = f"evals[{index}]"
        if not isinstance(entry, dict):
            errors.append(f"{label} 必须是对象")
            continue

        # eval id 必须是**整数**。两点刻意之处：
        # - 不接受字符串：`"1"` 与 `1` 在 JSON 里是两种东西，混用会让去重变成
        #   "看起来有 5 条、实际按字符串去重"这类安静的错位。
        # - 必须排除 bool：Python 里 `isinstance(True, int)` 为真，而 JSON 的 `true`
        #   会被解析成 `True`。不显式排除的话，`"id": true` 会被当成合法的整数 id。
        eval_id = entry.get("id")
        if isinstance(eval_id, bool) or not isinstance(eval_id, int):
            errors.append(f"{label} 的 id 必须是整数（收到 {type(eval_id).__name__}: {eval_id!r}）")
        elif eval_id in seen_ids:
            errors.append(f"{label} id 重复：{eval_id!r}")
        else:
            seen_ids.add(eval_id)

        for field in ("name", "prompt", "expected_output"):
            value = entry.get(field)
            if not isinstance(value, str) or not value.strip():
                errors.append(f"{label} 缺少非空 {field}")

        expectations = entry.get("expectations")
        if not isinstance(expectations, list) or not expectations:
            errors.append(f"{label} 缺少非空 expectations")
            continue
        for position, expectation in enumerate(expectations):
            if not isinstance(expectation, str) or not expectation.strip():
                errors.append(f"{label}.expectations[{position}] 必须是非空字符串")

    return errors


def _check_skill_line_budget(skill_md: Path) -> list[str]:
    try:
        line_count = len(skill_md.read_text(encoding="utf-8").splitlines())
    except (OSError, UnicodeError) as error:
        return [f"SKILL.md 无法读取（{error}）"]
    if line_count >= MAX_SKILL_LINES:
        return [f"SKILL.md 行数超限（{line_count}/{MAX_SKILL_LINES - 1}）"]
    return []


def check_skill(
    root: Path,
    skill_dir: Path,
    mcp_constants: str = MCP_CONSTANTS,
    mcp_bridge: str = MCP_BRIDGE,
) -> list[str]:
    """返回全部失败项；空列表表示通过。"""
    errors: list[str] = []

    skill_md = skill_dir / "SKILL.md"
    if not skill_md.exists():
        return [f"{skill_md} 不存在"]

    frozen, frozen_error = _load_frozen_tools(root, mcp_constants)
    if frozen_error is not None:
        # 读不到权威清单就无法判定工具名——直接失败，不要退化成"跳过这条检查"。
        return [frozen_error]
    capabilities, capability_error = _load_capability_names(root, mcp_bridge)
    if capability_error is not None:
        return [capability_error]

    frontmatter, frontmatter_error = _parse_frontmatter(skill_md)
    if frontmatter_error is not None:
        errors.append(frontmatter_error)
        frontmatter = {}

    errors.extend(_check_frontmatter(frontmatter))
    errors.extend(_check_skill_line_budget(skill_md))

    name = frontmatter.get("name")
    skill_name = name.strip() if isinstance(name, str) else ""
    errors.extend(_check_evals(skill_dir, skill_name, frozen))
    errors.extend(_check_tool_names(skill_dir, frozen, capabilities))

    return errors


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description="校验仓库内 Skill 的契约")
    parser.add_argument("--root", default=".", help="仓库根目录（默认当前目录）")
    parser.add_argument("--skill", default=DEFAULT_SKILL, help=f"Skill 目录（默认 {DEFAULT_SKILL}）")
    parser.add_argument("--mcp-root", default=MCP_CONSTANTS, help="MCP 常量文件相对路径")
    parser.add_argument("--mcp-bridge", default=MCP_BRIDGE, help="MCP 端口文件相对路径")
    parser.add_argument("--quiet", action="store_true", help="只输出结论")
    args = parser.parse_args(argv)

    root = Path(args.root).resolve()
    skill_dir = (root / args.skill).resolve()

    frozen, frozen_error = _load_frozen_tools(root, args.mcp_root)
    errors = check_skill(root, skill_dir, args.mcp_root, args.mcp_bridge)

    if not args.quiet:
        if frozen_error is None:
            print(f"冻结工具清单（来自 {args.mcp_root}）：{len(frozen)} 项")
        print(f"校验目标：{args.skill}")

    if errors:
        for error in errors:
            print(f"FAIL: {error}", file=sys.stderr)
        print(f"skill-contract-check: 失败（{len(errors)} 项）", file=sys.stderr)
        return 1

    print("skill-contract-check: PASS")
    return 0


if __name__ == "__main__":
    sys.exit(main())
