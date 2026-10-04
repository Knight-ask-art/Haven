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
5. **不得授予写权限**：白名单之外的工具名只能出现在**禁止语境**里。判断依据是该行**所在的
   句子**（按空行与句末标点切分，见 `_prohibited_lines`）是否含否定标记（`不` / `禁止` /
   `❌` / `不得` / `永不` / `拒绝` / `never` / `NOT`，即 `NEGATION_MARKERS` 元组的全部 8 项）。
   这样"❌ 做 `metadata_patch`"是合法的说明，而"调用 `apply_settings`"会被抓住。

   语境单位是**句子**而不是物理行：中文文档里一句话经常折行，而折行是排版产物、不是语义
   边界。只看单行会让 `mcp-tool-map.md` 里那句"恒为 `false` 的四位（`metadata_proposal` /
   …）不是暂时关着"的第一行被当成授权指令——一个真实发生过的误报。

   这是护栏而非证明：一句授予性指令若恰好与否定标记同句（例如"不确定时调用
   `apply_settings`"）会被判为禁止说明而放过。真正的权限边界在 MCP 工具清单本身——
   那 9 个工具里没有写方法。
6. **受众标记**：`<!-- haven:audience=native -->` / `=external` 与 `<!-- /haven:audience -->`
   必须闭合（未知受众名、未闭合、嵌套、多余闭合、未知 `<!-- haven:` 标记都是失败），且用了
   标记就必须两条运行时都有段落。**任何"看起来像标记"但不是规范形式的行同样失败**：标记前
   带说明文字、`<!--haven:`（缺空格）、`=` 两侧空格、闭合行后带尾巴——宽松处理它们会让
   外部段落漏进原生投影。运行时对畸形标记是 fail-closed 的（会让应用启动失败），
   因此这里提前给出同一个结论。规则与 `haven-domain` 的投影实现同形。
7. **frontmatter 是闭合语法，且不依赖任何 YAML 库**：第一行必须恰好是 `---`；之后每行是
   `键: 值`（在第一个冒号处切开，两侧 trim）；键只能是 `name` / `description` 且各出现一次；
   值不能为空、不能含尖括号、不能含控制字符。引号、块标量、中括号、`#` 注释这些 YAML 形式
   在这里**都是普通字符**，与 `haven-domain` 的 `parse_skill_document` 给同一个答案——
   装没装 PyYAML 都不改变结论。
8. **运行时正文验收复刻**：正文（frontmatter **之后**的切片，元数据不参与）不超过 24 000
   字符、不含被禁控制字符（`\n` / `\t` 与成对的 `\r\n` 除外），两条受众的投影都非空且同样
   合规。上限与控制字符规则与 `haven-domain` 的 `AgentSkillInstructions` 同值/同形，而"正文
   从 frontmatter 的闭合行之后开始"与它的 `split_skill_document` 同形——frontmatter 的值另有
   一套更严的规则（见第 7 条）。这条不是锦上添花：超限或含控制字符时**运行时拒绝加载**，
   而运行时拒绝意味着应用启动失败——少了它就会出现"打包通过、用户装上去打不开"。两条投影
   都查比运行时更严（运行时不校验外部投影），CI 比运行时更严是安全的，反过来才是分裂。

行切分一律按 `\\n`（`_document_lines`），**不**用 `str.splitlines()`：后者会在 `\\v` / `\\f` /
`\\x1c` / `\\u2028` 等处额外断行，于是同一份文档在两侧被切成不同的行——那正是"校验器接受、
运行时拒绝"以及"外部段落静默漏进原生投影"的来源。

关于**分层**：`_parse_frontmatter` 只对应 Rust 的 `parse_skill_document`（逐字解析，引号是
普通字符，**不含** id 形状判定）；`name` 是否合法 hyphen-case 由 `_check_frontmatter` 判定，
对应 Rust 的 `AgentSkillId::parse`。因此 `name: "x"` 在两侧都是"解析层接受、验收层拒绝"。
把形状判定下沉到解析层会让校验器做一件运行时不做的
事，两侧的结论层次就此分岔。

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
import unicodedata
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

# 注入模型请求的技能正文上限（字符，不是字节）。与 `haven-domain` 的
# `AGENT_SKILL_INSTRUCTIONS_MAX_CHARS` 同值：超限会让**运行时拒绝加载**（应用启动失败），
# 因此契约校验必须给出同一个结论，而不是"打包通过、装上去打不开"。
MAX_INSTRUCTIONS_CHARS = 24_000

FRONTMATTER_DELIMITER = "---"
ALLOWED_FRONTMATTER_KEYS = ("name", "description")
NAME_PATTERN = re.compile(r"^[a-z0-9-]+$")

# 否定标记：出现即认为该行是"禁止说明"而不是"授权指令"。
NEGATION_MARKERS = ("不", "禁止", "❌", "不得", "永不", "拒绝", "never", "NOT")

# 句末标点：中文文档里一句话经常折行，因此"这一行有没有否定标记"必须按**句子**问，
# 而不是按物理行问。空行同样结束一个句子。
SENTENCE_TERMINATORS = "。．.！!？?；;：:"

# 受众标记：同一份技能文档面向两条运行时，用标记划出各自的段落。
# 语法与 Rust 侧 `haven-domain` 的 `AgentSkillInstructions::project` 同形。
AUDIENCE_OPEN_PREFIX = "<!-- haven:audience="
AUDIENCE_OPEN_SUFFIX = "-->"
AUDIENCE_CLOSE = "<!-- /haven:audience -->"
AUDIENCE_NAMES = ("native", "external")
# 两种规范标记共有的子串，以及所有 `haven` 命名空间标记的公共前缀。
AUDIENCE_MARKER_TOKEN = "haven:audience"
HAVEN_MARKER_PREFIX = "<!-- haven:"

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

# **特权能力名：只允许出现在禁止语境里。**
#
# 为什么单独一份，而不是从 `bridge.ts` 现读取值：`HavenCapabilityReport` 只声明能力的
# **名字**与类型 `boolean`，"哪几位恒为 false"这件事写在 Rust 的能力清单投影里。
# 从 TypeScript 接口里推导不出取值；用正则去 Rust 源码里抠布尔值又会在重构时静默失配
# ——那比没有这条检查更糟，因为报告会变成"全绿"。
#
# 因此这里是一份**闭合清单**，并由 `skill-contract-check.test.py` 断言其中每个名字仍然
# 是 `bridge.ts` 里的真实能力名。上游改名会让那条测试失败，而不是让这条检查悄悄失效。
#
# 它们是"能力位"而不是"工具名"，因此不会被 `TOOL_NAME_PREFIXES` 捕获：`secret_read`
# 不以 `read_` 开头，`filesystem_write` 也不以 `write_` 开头。旧规则还会因为它们出现在
# 能力名单里而**直接放行**——于是一句"先调用 `secret_read` 拿到密钥"可以完整通过。
PRIVILEGED_CAPABILITY_NAMES = (
    "metadata_proposal",
    "rename_proposal",
    "secret_read",
    "filesystem_write",
)

# 外部参考材料的引用形状。原生投影里出现它，说明某段"外部才读得到"的内容漏进来了。
REFERENCE_LINK = re.compile(r"references/[A-Za-z0-9][A-Za-z0-9._-]*\.md")

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


def _document_lines(text: str) -> list[str]:
    """按 Rust `str::lines()` 的规则切行。

    刻意**不**用 `str.splitlines()`：它还会在 `\\v` / `\\f` / `\\x1c` / `\\u2028` 等处断行，
    而运行时不会——同一份文档在两侧被切成不同的行，正是"校验器接受、运行时拒绝"的来源。
    这里只在 `\\n` 处断行，并去掉行尾的一个 `\\r`（CRLF 被接受，裸 `\\r` 不是换行符）。
    """
    if not text:
        return []
    lines = text.split("\n")
    if lines[-1] == "":
        # Rust 的 `lines()` 不会为结尾的那个换行再生出一个空行。
        lines.pop()
    return [line[:-1] if line.endswith("\r") else line for line in lines]


def _is_forbidden_control_char(character: str) -> bool:
    """与 Rust `char::is_control()` 同义：Unicode 类别 Cc，`\\n` 与 `\\t` 除外。"""
    if character in ("\n", "\t"):
        return False
    return unicodedata.category(character) == "Cc"


def _has_forbidden_body_control_chars(value: str) -> bool:
    """与 `haven-domain` 的 `has_forbidden_control_chars` 同义。

    与上面的 `_is_forbidden_control_char` **差一处**：正文允许成对的 `\\r\\n`（CRLF 文档
    是合法内容，摘要仍然覆盖原始字节），裸 `\\r` 依然拒绝。frontmatter 那一侧因为已经按行
    剥掉了行尾 `\\r`，所以用严格版本即可。
    """
    position = 0
    while position < len(value):
        character = value[position]
        if character in ("\n", "\t"):
            position += 1
            continue
        if character == "\r" and position + 1 < len(value) and value[position + 1] == "\n":
            position += 1
            continue
        if _is_forbidden_control_char(character):
            return True
        position += 1
    return False


def _strip_line_ending(line: str) -> str:
    """去掉行尾（`\\n` 或 `\\r\\n`）。只用于**判断**这一行是不是标记/分隔行——
    投影输出时写回的是原始行，因此行尾不会被投影悄悄抹平。"""
    if line.endswith("\n"):
        line = line[:-1]
    if line.endswith("\r"):
        line = line[:-1]
    return line


def _project_lines(text: str, audience: str) -> tuple[str | None, str | None]:
    """`haven-domain` 的 `AgentSkillInstructions::project` 的 Python 复刻。

    返回 `(投影, None)` 或 `(None, 失败原因)`。行尾**原样保留**（与 Rust 的
    `split_inclusive('\\n')` 一致），因此投影仍然逐字节可比。
    """
    collected: list[str] = []
    opened: str | None = None
    for raw_line in _split_inclusive_lines(text):
        line = _strip_line_ending(raw_line).strip()

        if line.startswith(AUDIENCE_OPEN_PREFIX):
            if not line.endswith(AUDIENCE_OPEN_SUFFIX):
                return None, "受众标记没有闭合的 -->"
            name = line[len(AUDIENCE_OPEN_PREFIX) : -len(AUDIENCE_OPEN_SUFFIX)].strip()
            if name not in AUDIENCE_NAMES:
                return None, f"受众标记的受众名不在闭合集合里：{name!r}"
            if opened is not None:
                return None, "受众标记不能嵌套"
            opened = name
            continue
        if line == AUDIENCE_CLOSE:
            if opened is None:
                return None, "受众标记闭合行没有对应的开始行"
            opened = None
            continue
        if _looks_like_marker(line):
            return None, "技能文档含非规范或不支持的受众标记"
        if opened is not None and opened != audience:
            continue
        collected.append(raw_line)

    if opened is not None:
        return None, "受众标记没有闭合"
    return "".join(collected), None


def _split_inclusive_lines(text: str) -> list[str]:
    """等价于 Rust `str::split_inclusive('\\n')`：行尾原样保留。

    刻意**不**用 `str.splitlines()`：它会在 `\\v` / `\\f` / `\\x1c` / `\\u2028` 处断行，
    而运行时只在 `\\n` 处断行。
    """
    if not text:
        return []
    parts = text.split("\n")
    return [part + "\n" for part in parts[:-1]] + ([parts[-1]] if parts[-1] else [])


def _check_runtime_document(skill_md: Path) -> list[str]:
    """复刻 `BuiltinAgentSkill::from_document` 的正文验收。

    存在理由：正文有字符上限与控制字符规则，超限或含控制字符时**运行时拒绝加载**——
    而运行时拒绝意味着应用**启动失败**。契约校验器若只查行数，就会出现"打包通过、
    用户装上去打不开"的分裂。

    规则与 Rust 逐条同形：

    - **正文（frontmatter 之后的切片）**非空、≤ `MAX_INSTRUCTIONS_CHARS` 字符、不含被禁
      控制字符。frontmatter 不在其中：它的值由 `_parse_frontmatter` 单独校验（那里的控制
      字符与尖括号规则更严），而它本身不参与投影、字符预算与摘要；
    - 受众标记可投影（畸形标记由 `_check_audience_markers` 单独报，这里不重复）；
    - 两条受众的投影各自非空、≤ 同一上限、不含被禁控制字符。

    **两条投影都查**，比运行时更严：运行时不校验外部投影（外部 Agent 读的是整份文档），
    但打包器会把整份文档发给外部客户端，一份投影不出正文的技能发出去同样是残缺的。
    CI 比运行时更严是安全的，反过来才是分裂。
    """
    try:
        text = skill_md.read_bytes().decode("utf-8")
    except (OSError, UnicodeError) as error:
        return [f"SKILL.md 无法读取（{error}）"]

    body = skill_body(text)
    if body is None:
        # frontmatter 本身的问题由 `_parse_frontmatter` 负责报，这里不重复同一件事。
        return []

    errors: list[str] = []
    if not body.strip():
        errors.append("SKILL.md 正文（frontmatter 之后）为空")
    if len(body) > MAX_INSTRUCTIONS_CHARS:
        errors.append(
            f"SKILL.md 正文超出运行时上限（{len(body)}/{MAX_INSTRUCTIONS_CHARS} 字符）"
        )
    if _has_forbidden_body_control_chars(body):
        errors.append("SKILL.md 正文包含控制字符（运行时会让应用启动失败）")

    for audience in AUDIENCE_NAMES:
        projection, projection_error = _project_lines(body, audience)
        if projection_error is not None:
            # 标记本身的问题由 `_check_audience_markers` 负责，这里不重复报同一件事。
            continue
        if not projection.strip():
            errors.append(f"{audience} 受众投影为空（运行时拒绝加载）")
            continue
        if len(projection) > MAX_INSTRUCTIONS_CHARS:
            errors.append(
                f"{audience} 受众投影超出运行时上限"
                f"（{len(projection)}/{MAX_INSTRUCTIONS_CHARS} 字符）"
            )
        if _has_forbidden_body_control_chars(projection):
            errors.append(f"{audience} 受众投影包含控制字符（运行时拒绝加载）")

    return errors


def skill_body(text: str) -> str | None:
    """frontmatter 闭合行**之后**的正文切片；无法确定正文起点时返回 `None`。

    与 `haven-domain` 的 `split_skill_document` 同形：只在 `\\n` 处断行、行尾 `\\r` 原样保留，
    因此两侧切出的是同一段字符。**只有这一段**参与原生投影、字符预算与内容摘要：
    frontmatter 的 `description` 是给外部 Agent 路由用的元数据，不是任何一条运行时读到的
    说明文字。把它算进去有两个后果——MCP 语义被塞给一个没有工具的模型，以及"只改一句元数据
    描述就作废用户的启用记录"。
    """
    lines = _split_inclusive_lines(text)
    if not lines:
        return None
    if _strip_line_ending(lines[0]) != FRONTMATTER_DELIMITER:
        return None
    consumed = len(lines[0])
    for raw_line in lines[1:]:
        consumed += len(raw_line)
        if _strip_line_ending(raw_line) == FRONTMATTER_DELIMITER:
            return text[consumed:]
    return None


def native_projection(skill_dir: Path) -> tuple[str | None, str | None]:
    """原生运行时会真正读到的那份正文：`(投影, None)` 或 `(None, 失败原因)`。

    打包器与一致性测试靠它比对"仓库里的字节"与"运行时会注入的文字"，因此它必须与
    `haven-domain` 的 `native_projection` 给同一个答案——包括**投影对象是 frontmatter
    之后的正文**这一点。
    """
    try:
        text = skill_dir.joinpath("SKILL.md").read_bytes().decode("utf-8")
    except (OSError, UnicodeError) as error:
        return None, f"SKILL.md 无法读取（{error}）"
    body = skill_body(text)
    if body is None:
        return None, "SKILL.md 的 frontmatter 解析不出正文起点"
    return _project_lines(body, "native")


def _parse_frontmatter(skill_md: Path) -> tuple[dict[str, Any] | None, str | None]:
    """按**闭合语法**解析 frontmatter：`(frontmatter, None)` 或 `(None, 失败原因)`。

    语法与 `haven-domain` 的 `parse_skill_document` 逐条对应，且**不依赖任何 YAML 库**：

    - 第一行必须**恰好**是 `---`；
    - 之后每行是 `键: 值`（在**第一个**冒号处切开），键与值各自 trim；
    - 键只能是 `name` / `description`，各自最多出现一次；
    - 值不能为空、不能含尖括号、不能含控制字符（`\\t` 除外）；
    - 遇到下一个 `---` 结束；没有结束行即失败。

    为什么不用 YAML 解析器：YAML 会把 `name: "x"` 的引号去掉、把 `|` 当块标量、把 `#` 之后
    当注释，于是**校验器接受一份运行时会拒绝的文档**（运行时拒绝意味着应用启动失败）。
    这里刻意保留字面值，与运行时给同一个答案；结论也与装没装 PyYAML 无关。
    """
    try:
        # 按字节读再解码：`read_text()` 会做通用换行转换，把裸 `\r` 变成 `\n`，
        # 而运行时只在 `\n` 处断行。用 read_text 会让运行时拒绝的文档在这里通过。
        text = skill_md.read_bytes().decode("utf-8")
    except (OSError, UnicodeError) as error:
        return None, f"SKILL.md 无法读取（{error}）"

    lines = _document_lines(text)
    if not lines or lines[0] != FRONTMATTER_DELIMITER:
        return None, "SKILL.md 必须以 frontmatter 分隔行（---）开头"

    parsed: dict[str, Any] = {}
    for line in lines[1:]:
        if line == FRONTMATTER_DELIMITER:
            if "name" not in parsed:
                return None, "frontmatter 缺少 name"
            if "description" not in parsed:
                return None, "frontmatter 缺少 description"
            if len(parsed["description"]) > MAX_DESCRIPTION_CHARS:
                return None, (
                    f"frontmatter 的 description 超长"
                    f"（{len(parsed['description'])}/{MAX_DESCRIPTION_CHARS}）"
                )
            return parsed, None

        key, separator, value = line.partition(":")
        if not separator:
            return None, f"frontmatter 行缺少键值分隔符：{line[:60]!r}"
        key = key.strip()
        value = value.strip()
        if key not in ALLOWED_FRONTMATTER_KEYS:
            allowed = " / ".join(ALLOWED_FRONTMATTER_KEYS)
            return None, f"frontmatter 出现多余键：{key!r}（只允许 {allowed}）"
        if not value:
            return None, f"frontmatter 的 {key} 值不能为空"
        if "<" in value or ">" in value:
            return None, f"frontmatter 的 {key} 值不能包含尖括号"
        if any(_is_forbidden_control_char(character) for character in value):
            return None, f"frontmatter 的 {key} 值包含控制字符"
        if key in parsed:
            return None, f"frontmatter 出现重复键：{key}"
        parsed[key] = value

    return None, "frontmatter 没有闭合分隔行"


# ---- 公开入口（供 tools/skills/pack-skill.py 复用，规则只有一份）----


def frozen_tool_names(root: Path, mcp_constants: str = MCP_CONSTANTS) -> tuple[list[str], str | None]:
    """MCP 冻结的工具清单：`(工具名, None)` 或 `([], 失败原因)`。

    打包器要把它写进分发清单，因此必须复用这里**同一个**解析器——两份解析器迟早会
    对同一份 `constants.ts` 给出不同答案。
    """
    return _load_frozen_tools(root, mcp_constants)


def read_skill_frontmatter(skill_dir: Path) -> tuple[dict[str, Any] | None, str | None]:
    """解析 `SKILL.md` 的 frontmatter：`(frontmatter, None)` 或 `(None, 失败原因)`。"""
    return _parse_frontmatter(skill_dir / "SKILL.md")


def _check_frontmatter(frontmatter: dict[str, Any]) -> list[str]:
    errors: list[str] = []

    unexpected = sorted(set(frontmatter) - set(ALLOWED_FRONTMATTER_KEYS))
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


def _ends_sentence(line: str) -> bool:
    """这一行是否结束一个句子（空行同样结束）。"""
    stripped = line.strip()
    if not stripped:
        return True
    return stripped.endswith(tuple(SENTENCE_TERMINATORS))


def _prohibited_lines(lines: list[str]) -> list[bool]:
    """逐行给出"这一行是否处于禁止语境"。

    语境单位是**句子**，不是物理行。`mcp-tool-map.md` 里那句真话
    「`capabilities` 里恒为 `false` 的四位（`metadata_proposal` / `rename_proposal` /
    `secret_read` / `filesystem_write`）不是"暂时关着"…」在源文件里折成两行，否定标记
    （`不`）落在**第二行**；按物理行判定会把第一行里的 `metadata_proposal` 与
    `rename_proposal` 当成授权指令。折行是排版产物，不是语义边界，所以判定必须跨行。

    句子边界 = 空行，或上一行以句末标点结尾。这一条让"❌ 本技能不得写文件。"之类的
    独立句子不能把后面新起的一句"先调用 `secret_read`"洗白——那正是要挡住的东西。
    """
    flags = [False] * len(lines)
    start = 0
    for index, line in enumerate(lines):
        if index != len(lines) - 1 and not _ends_sentence(line):
            continue
        if any(
            marker in entry
            for entry in lines[start : index + 1]
            for marker in NEGATION_MARKERS
        ):
            for position in range(start, index + 1):
                flags[position] = True
        start = index + 1
    return flags


def _check_tool_names(skill_dir: Path, frozen: list[str], capabilities: set[str]) -> list[str]:
    """反引号里"像工具名"的标识符必须来自冻结清单 / 能力名清单，或在禁止语境里。

    特权能力名（见 `PRIVILEGED_CAPABILITY_NAMES`）走**另一条更严的规则**：它们即使出现在
    能力名单里，也只允许出现在禁止语境。正向地"调用某个特权能力"是本技能永远不该写的
    指令——它在本版本里恒为未授予，写出来只会诱导模型编造，或让读者以为它可用。
    """
    errors: list[str] = []
    known = set(frozen) | capabilities

    for path in _iter_text_files(skill_dir):
        try:
            # 行切分必须与运行时同义（只在 `\n` 处断行）：`splitlines()` 会在 ` `
            # 之类的地方额外断行，那会让两侧对"这一行有没有否定标记"给出不同答案。
            lines = _document_lines(path.read_bytes().decode("utf-8"))
        except (OSError, UnicodeError) as error:
            errors.append(f"{path.name}: 无法读取（{error}）")
            continue

        prohibited_lines = _prohibited_lines(lines)
        for number, line in enumerate(lines, start=1):
            prohibited = prohibited_lines[number - 1]
            for token in CODE_SPAN.findall(line):
                candidate = token.strip()
                if candidate in PRIVILEGED_CAPABILITY_NAMES:
                    if not prohibited:
                        errors.append(
                            f"{path.relative_to(skill_dir)}:{number}: 特权能力 {candidate!r} "
                            f"不得出现在非禁止语境的说明里（它在当前版本里恒为未授予）"
                        )
                    continue
                if not TOOL_SHAPED.match(candidate):
                    continue
                if not candidate.startswith(TOOL_NAME_PREFIXES):
                    continue
                if candidate in known:
                    continue
                if prohibited:
                    continue
                errors.append(
                    f"{path.relative_to(skill_dir)}:{number}: 工具形标识符 {candidate!r} "
                    f"既不在 MCP 冻结的 9 项里，也不是已知能力名，且它所在的那句不是禁止说明"
                )

    return errors


def _check_native_projection_is_tool_free(skill_dir: Path, frozen: list[str]) -> list[str]:
    """原生投影里不得出现 MCP 工具用法或外部参考材料。

    为什么是**通用规则**而不是只对当前这一份技能断言：原生运行时的唯一出口是
    「结构化输出 → 待批准提案」，它**没有工具**。一段"MCP 工具怎么调"的文字若落进
    原生投影，模型会照着编造工具调用——而这不是假设：投影完全由受众标记决定，
    新技能只要漏写一次标记，隔离就静默失效。

    `_check_audience_markers` 管的是"标记写没写对"；这条管的是**结果**：整段外部内容
    本来就没标（于是它被当成共用段落）时，标记检查报不出来，投影检查能。
    """
    projection, error = native_projection(skill_dir)
    if error is not None or projection is None:
        # 投影本身不合法时由 `_check_runtime_document` 报错，这里不重复报同一件事。
        return []

    errors: list[str] = []
    for tool in frozen:
        if tool in projection:
            errors.append(
                f"原生投影里出现了 MCP 工具名 {tool!r}：原生运行时没有工具，"
                "这类段落必须放进 external 受众段落"
            )
    if AUDIENCE_MARKER_TOKEN in projection:
        errors.append("原生投影里残留了受众标记：它只该存在于源码树，不该被送进请求")
    leaked = REFERENCE_LINK.search(projection)
    if leaked is not None:
        errors.append(
            f"原生投影里引用了外部参考材料 {leaked.group(0)!r}："
            "原生运行时读不到技能目录，这类引用只对读整份文档的外部 Agent 有意义"
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
        # 与运行时同一套行切分语义：`splitlines()` 会把 ` ` 也当成换行，
        # 于是同一份文档在两侧被切成不同的行数。
        line_count = len(_document_lines(skill_md.read_bytes().decode("utf-8")))
    except (OSError, UnicodeError) as error:
        return [f"SKILL.md 无法读取（{error}）"]
    if line_count >= MAX_SKILL_LINES:
        return [f"SKILL.md 行数超限（{line_count}/{MAX_SKILL_LINES - 1}）"]
    return []


def _looks_like_marker(line: str) -> bool:
    """这一行是否"看起来想写受众标记"。

    与 Rust 侧 `looks_like_marker` 同义：只认规范形式是不够的——一行**前置了说明文字**的标记
    （`说明 <!-- haven:audience=external -->`）会以普通正文的身份进入投影，它后面的整段
    外部内容于是没有开始行，既不触发嵌套、也不报"缺少闭合"，而是静默漏进原生投影。
    """
    return AUDIENCE_MARKER_TOKEN in line or HAVEN_MARKER_PREFIX in line


def _check_audience_marker_text(text: str) -> list[str]:
    """检查传入的正文文本：受众标记必须闭合，且要么不用、要么两条运行时都有段落。

    为什么值得在这里查一遍：运行时（`haven-domain` 的 `AgentSkillInstructions::project`）
    对畸形标记是 fail-closed 的——解析失败会让应用**启动**失败。让 CI 先给出同一条结论，
    比让用户装完之后打不开应用要好。

    标记语法与 Rust 侧同形（两侧对"什么算畸形"必须给同一个答案），并额外多查一条：
    用了标记就必须两条运行时都有段落。CI 比运行时更严是安全的，反过来才是分裂。

    - 未闭合、嵌套、多余闭合、未知受众名、未知 `<!-- haven:` 标记一律失败；
    - 标记按 **trim 后**识别（一个缩进的标记若被当成正文，它后面的整段外部内容会静默漏进
      原生投影）；
    - **任何"看起来像标记"但不是规范形式的行**（标记前带说明文字、`<!--haven:`、`=` 两侧
      空格、闭合行带尾巴）同样失败：宽松处理与"不识别缩进标记"是同一个漏洞；
    - 用了标记就必须同时有 `native` 与 `external` 段——只标一边说明划分本身有问题。
    """
    errors: list[str] = []
    opened: str | None = None
    seen: set[str] = set()
    for number, raw_line in enumerate(_document_lines(text), start=1):
        line = raw_line.strip()
        if line.startswith(AUDIENCE_OPEN_PREFIX):
            if not line.endswith(AUDIENCE_OPEN_SUFFIX):
                errors.append(f"SKILL.md:{number}: 受众标记没有闭合的 -->")
                continue
            name = line[len(AUDIENCE_OPEN_PREFIX) : -len(AUDIENCE_OPEN_SUFFIX)].strip()
            if name not in AUDIENCE_NAMES:
                errors.append(f"SKILL.md:{number}: 未知受众名 {name!r}")
                continue
            if opened is not None:
                errors.append(f"SKILL.md:{number}: 受众标记不能嵌套")
                continue
            opened = name
            seen.add(name)
            continue
        if line == AUDIENCE_CLOSE:
            if opened is None:
                errors.append(f"SKILL.md:{number}: 受众闭合行没有对应的开始行")
                continue
            opened = None
            continue
        if _looks_like_marker(line):
            errors.append(f"SKILL.md:{number}: 技能文档含非规范或不支持的受众标记")

    if opened is not None:
        errors.append(f"SKILL.md: 受众标记（{opened}）没有闭合")
    if seen and seen != set(AUDIENCE_NAMES):
        missing = ", ".join(sorted(set(AUDIENCE_NAMES) - seen))
        errors.append(f"SKILL.md: 使用受众标记时必须两条运行时都有段落，缺少 {missing}")
    return errors


def _check_audience_markers(skill_md: Path) -> list[str]:
    """只检查 frontmatter 之后的正文，与 Rust `split_skill_document` 保持一致。

    元数据由 `_parse_frontmatter` 单独校验，运行时不会把它送进 `AgentSkillInstructions::project`。
    因此 frontmatter 中出现普通文本 `haven:audience` 不能被误判成受众标记。
    """
    try:
        text = skill_md.read_bytes().decode("utf-8")
    except (OSError, UnicodeError) as error:
        return [f"SKILL.md 无法读取（{error}）"]

    body = skill_body(text)
    if body is None:
        # frontmatter 的缺失/格式错误由 `_parse_frontmatter` 负责报告。
        return []
    return _check_audience_marker_text(body)


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
    errors.extend(_check_audience_markers(skill_md))
    errors.extend(_check_runtime_document(skill_md))
    errors.extend(_check_native_projection_is_tool_free(skill_dir, frozen))

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
