#!/usr/bin/env python3
r"""把仓库内的 Skill 打成带版本、可校验的分发产物（外部 Agent 分发路径）。

产物是两件东西，放在 `--out` 目录下：

```
<skill-id>-<version>.zip              技能包正文（与仓库里的那份逐字节相同）
<skill-id>-<version>.manifest.json    版本、内容摘要、文件清单、面向的 MCP 工具契约
```

用户把 zip 解压到**自己**所用客户端（Codex / Claude Code / DSH / Pi …）的技能目录。
**本工具不写任何用户级 Agent 配置**，也不猜各客户端的目录约定：它只产出可复制的产物
与一份自描述的清单。栖阅的职责到"提供同一份技能包"为止，安装动作属于用户。

设计约束（每条都对应一个会真实发生的失败）：

1. **只做字节搬运。** 打包过程不做任何转换、清理或重排：打包时"顺手格式化一下"
   会让外部 Agent 拿到的技能与栖阅内嵌的技能变成两份东西。
2. **先校验，后打包。** 打包前跑 `tools/skills/skill-contract-check.py` 的同一套规则
   （frontmatter、行数预算、evals、工具名白名单）；契约不过就不产出产物。
3. **工具清单只有一个事实源。** manifest 里的 MCP 工具名从
   `mcp/haven-mcp/src/constants.ts` 现读，复用契约校验器的解析器，不另抄一份。
4. **版本对不上就拒绝。** 技能包版本取产品版本（`前端/app/package.json`），并要求
   MCP server（`mcp/haven-mcp/package.json`）版本相同——"技能与 server 版本错配"
   是用户最难自查的一类失败。
5. **输出确定性。** 固定条目顺序、固定时间戳、固定压缩方式，因此同一份源码重复打包
   得到逐字节相同的 zip；否则"同一个版本号"背后可以有无数个不同的包。
6. **`--skill` 是仓库内的相对路径，且解析后必须仍在仓库里。** `pathlib` 的 `/` 在右侧是
   绝对路径时会丢掉左边，`..` 也不会被拦住；目录联接（junction）则会在"看起来在仓库内"
   的路径后面接上仓库外的目录。两种情况下打包器都会安静地把**机器上的任意目录**当成
   技能包发出去，而产物看起来完全正常。因此在读取任何文件之前先做位置校验。
7. **版本号进文件名，所以必须是文件名安全的。** 产物文件名是 `<skill-id>-<version>.zip`；
   一个含路径分隔符的版本号会把产物写到 `--out` 之外（`<out>/x/../../evil.zip`）。
8. **只有显式允许的技能资产才会进包。** 包内容是**逐条列出**的（`SKILL.md`、
   `evals/evals.json`、`references/*.md`），技能目录下的其它任何文件——未跟踪的草稿、
   `.env` 之类的密钥、编辑器临时文件、构建产物——都会让打包**失败**，而不是被顺手发出去。
   "在技能目录下"不是"属于技能包"的证据：一个 `git status` 里看不见的 `.env` 被塞进
   分发 zip，装上之后就是一次凭据外泄，而产物看起来完全正常。

常用用法：
    python tools/skills/pack-skill.py --out .tmp/skill-packages
    python tools/skills/pack-skill.py --skill skills/xxx --out /tmp/out --quiet

`--skill` 只接受**仓库内**的相对路径（`skills/xxx` 或 `skills\xxx`）；绝对路径、`..`、
以及穿过符号链接/重解析点的路径一律拒绝，且拒绝时不产出任何文件。

退出码：0 = 产物已写出；1 = 拒绝打包（契约不过 / 版本不一致 / 目标不合法 / 版本号不能
用作文件名 / 技能目录里有不在允许清单内的文件）。Windows 上这些消息是中文：CI 以
`PYTHONUTF8=1` 运行，否则管道里的 stdout/stderr 会用遗留代码页编码并在打印时抛
`UnicodeEncodeError`。
"""

from __future__ import annotations

import argparse
import hashlib
import importlib.util
import json
import os
import re
import stat
import sys
import tempfile
import zipfile
from pathlib import Path
from typing import Any

DEFAULT_SKILL = "skills/haven-agent-proposal"
CONTRACT_CHECKER = "tools/skills/skill-contract-check.py"
PRODUCT_MANIFEST = "前端/app/package.json"
MCP_MANIFEST = "mcp/haven-mcp/package.json"

MANIFEST_FORMAT_VERSION = 1

# ZIP 条目的固定时间戳：zip 的时间字段最早只能是 1980-01-01。
ARCHIVE_TIMESTAMP = (1980, 1, 1, 0, 0, 0)
# 0644，且显式声明为普通文件：解压侧不该从包里继承可执行位或目录位。
ARCHIVE_FILE_MODE = 0o100644
# 不压缩：压缩器实现/版本会影响 DEFLATE 字节，ZIP_STORED 才能让包字节跨平台稳定。
ARCHIVE_COMPRESSION = zipfile.ZIP_STORED

# 版本号会直接拼进产物文件名（`<skill-id>-<version>.zip`），因此必须是**文件名安全**的：
# 含路径分隔符的版本号会把产物写到 `--out` 之外，而产物看起来完全正常。形状与
# `tools/release/bundle-version.py` 的发布 tag 规则同源（那里管 tag，这里管包名），
# 只多允许 SemVer 的 `+build` 元数据。
VERSION_PATTERN = re.compile(r"^\d+\.\d+\.\d+(?:-[0-9A-Za-z.-]+)?(?:\+[0-9A-Za-z.-]+)?$")
MAX_VERSION_CHARS = 128

# `FILE_ATTRIBUTE_REPARSE_POINT`：NTFS 目录联接（junction）与符号链接都带这一位。
# 常量在 `stat` 里只有 Windows 才有，因此给一个 ASCII 兜底值。
FILE_ATTRIBUTE_REPARSE_POINT = getattr(stat, "FILE_ATTRIBUTE_REPARSE_POINT", 0x400)

# 技能包内容的**显式允许清单**。打包器只发送这里列出的文件，别的一律拒绝。
#
# 为什么是 allowlist 而不是 denylist："技能目录下的东西"与"属于技能包的东西"是两件事。
# 一份没被跟踪的 `.env`、一个编辑器交换文件、一次 `dist/` 构建产物，都不在任何合理的
# 拒绝清单里，却都会随着 `rglob('*')` 进包；而这些文件对用户来说是不可见的——产物
# 看起来完全正常。逐条列出来，才能让"这个文件为什么会在包里"变成可回答的问题。
#
# 路径写成包内 POSIX 相对路径：zip 条目名就是从这里来的，因此这两处必须是同一个字符串。
PACKAGE_FILE_RULES: tuple[re.Pattern[str], ...] = (
    re.compile(r"SKILL\.md"),
    re.compile(r"evals/evals\.json"),
    re.compile(r"references/[A-Za-z0-9][A-Za-z0-9._-]*\.md"),
)

# 允许清单里**必须**存在的文件：缺了它们，包就不是一份可安装的技能。
REQUIRED_PACKAGE_FILES = ("SKILL.md", "evals/evals.json")


class PackError(Exception):
    """无法产出可信产物。消息面向人，直接打印。"""


def _load_contract_module() -> Any:
    """加载同目录的契约校验器。

    **不复制**它的规则：契约校验和打包必须对"什么是合法技能"给同一个答案，
    否则会出现"校验器拒绝、打包器照打"这种最难查的分裂。
    """
    path = Path(__file__).with_name("skill-contract-check.py")
    spec = importlib.util.spec_from_file_location("skill_contract_check", path)
    if spec is None or spec.loader is None:
        raise PackError(f"无法加载契约校验器：{path}")
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


def _read_json(root: Path, relative_path: str) -> dict[str, Any]:
    path = root / relative_path
    try:
        value = json.loads(path.read_text(encoding="utf-8"))
    except (OSError, UnicodeError, json.JSONDecodeError) as error:
        raise PackError(f"{relative_path}: 无法读取（{error}）") from error
    if not isinstance(value, dict):
        raise PackError(f"{relative_path}: 期望 JSON 对象")
    return value


def resolve_skill_dir(root: Path, selector: str) -> Path:
    """把 `--skill` 解析成**仓库内**的技能目录；任何不安全的形态都抛 `PackError`。

    为什么不能直接把 `root / selector` 交给 `pack()`：`pathlib` 的 `/` 在右侧是绝对路径时
    会**丢掉**左边（`Path("repo") / "/etc"` 就是 `/etc`），`..` 也不会被拦住。于是
    `--skill /anywhere` 或 `--skill ../../..` 会让打包器读仓库外的目录，并把那里的内容
    当成技能包发出去——而产物看起来完全正常。目录联接更隐蔽：路径本身是仓库内的，
    指向的却是仓库外。

    规则（逐条对应一种真实逃逸）：

    - 必须是**相对路径**：绝对路径、盘符（含 `C:dir` 这种驱动器相对形式）、UNC 前缀
      （`\\\\server\\share`）一律拒绝；
    - 任何一段都不能是 `..`：先拒绝再规范化，避免"规范化之后才落在仓库内"的中间形态；
    - 从 `root` 往下的**每一段**都不能是符号链接或重解析点：`resolve()` 在部分平台上
      不会改写联接，而且"指向仓库内的链接"同样会让技能包的来源不可审计，所以一律 fail closed；
    - 解析后的真实路径必须仍在 `root` 之内、且是一个目录。

    返回**已解析**的绝对路径，供 `pack()` 直接使用。
    """
    root = root.resolve()
    raw = selector.strip()
    if not raw:
        raise PackError("--skill 不能为空")
    if raw.startswith(("/", "\\")) or Path(raw).is_absolute() or re.match(r"^[A-Za-z]:", raw):
        raise PackError(f"--skill 必须是仓库内的相对路径：{selector!r}")

    parts = [part for part in re.split(r"[\\/]+", raw) if part not in ("", ".")]
    if not parts or ".." in parts:
        raise PackError(f"--skill 不能包含 '..' 或指向仓库之外：{selector!r}")

    walked = root
    for part in parts:
        walked = walked / part
        if _is_link_like(walked):
            raise PackError(f"--skill 路径包含符号链接或重解析点：{walked}")

    resolved = root.joinpath(*parts).resolve()
    if resolved == root or root not in resolved.parents:
        raise PackError(f"--skill 必须指向仓库内的目录：{selector!r}")
    if not resolved.is_dir():
        raise PackError(f"--skill 不是一个目录：{selector!r}")
    return resolved


def _is_link_like(path: Path) -> bool:
    """符号链接 / NTFS 目录联接（junction）/ 其他重解析点。

    `Path.is_symlink()` 只认符号链接：Windows 上的目录联接（`mklink /J`，普通用户无需
    提权即可创建）不是符号链接，却同样能把"仓库内的目录"变成仓库外的目录。
    """
    try:
        info = path.lstat()
    except OSError:
        # 不存在（或不可访问）时交给后面的"是不是目录"判定，这里不重复报同一件事。
        return False
    if stat.S_ISLNK(info.st_mode):
        return True
    attributes = getattr(info, "st_file_attributes", 0)
    return bool(attributes & FILE_ATTRIBUTE_REPARSE_POINT)


def _validate_version(version: str, label: str) -> str:
    """版本号会拼进产物文件名，因此必须是文件名安全的。

    刻意**不**做 trim / 规范化：`"1.0.0 "` 会被 Win32 悄悄去掉尾随空格，从而与 `"1.0.0"`
    撞成同一个产物名——静默改写用户的版本号比拒绝它更糟。
    """
    if len(version) > MAX_VERSION_CHARS or VERSION_PATTERN.fullmatch(version) is None:
        raise PackError(
            f"{label}: version {version!r} 不能用作产物文件名"
            "（需要形如 0.1.0 或 0.1.0-beta.1，且不含路径分隔符）"
        )
    return version


def resolve_version(root: Path) -> tuple[str, str]:
    """产品版本 + MCP server 版本必须一致，否则拒绝打包。

    两个版本号都会进产物文件名与 manifest，所以形状先过 `_validate_version`：
    "版本不一致"的检查拦不住一个**两边一致但含 `../`** 的版本号。

    返回 `(版本, MCP server 包名)`。
    """
    product = _read_json(root, PRODUCT_MANIFEST).get("version")
    server_manifest = _read_json(root, MCP_MANIFEST)
    server = server_manifest.get("version")
    if not isinstance(product, str) or not product.strip():
        raise PackError(f"{PRODUCT_MANIFEST}: 缺少非空 version")
    if not isinstance(server, str) or not server.strip():
        raise PackError(f"{MCP_MANIFEST}: 缺少非空 version")
    product = _validate_version(product, PRODUCT_MANIFEST)
    server = _validate_version(server, MCP_MANIFEST)
    if product != server:
        raise PackError(
            f"版本不一致：{PRODUCT_MANIFEST}={product!r} 与 {MCP_MANIFEST}={server!r}；"
            "技能包与 MCP server 必须是同一个版本"
        )
    package_name = server_manifest.get("name")
    if not isinstance(package_name, str) or not package_name.strip():
        raise PackError(f"{MCP_MANIFEST}: 缺少非空 name")
    return product, package_name


def read_frontmatter_name(contract: Any, skill_dir: Path) -> str:
    """从技能文档的 frontmatter 取 id（**不**从目录名推断）。"""
    frontmatter, error = contract.read_skill_frontmatter(skill_dir)
    if error is not None or not isinstance(frontmatter, dict):
        raise PackError(f"SKILL.md frontmatter 无法解析：{error}")
    name = frontmatter.get("name")
    if not isinstance(name, str) or not name.strip():
        raise PackError("SKILL.md frontmatter 缺少非空 name")
    return name.strip()


def is_allowed_package_file(relative: str) -> bool:
    """这个包内相对路径是否属于显式允许的技能资产。"""
    return any(rule.fullmatch(relative) is not None for rule in PACKAGE_FILE_RULES)


def validate_archive_entry(relative: str) -> str:
    """校验一个将要写进 zip 的条目名；不安全就抛 `PackError`。

    条目名是**解压侧**看到的路径，因此这里必须独立于宿主文件系统成立：POSIX 允许文件名
    里出现反斜杠与冒号（`references\\..\\..\\evil.md` 是一个合法文件名），而 Windows 解压
    时反斜杠是分隔符——同一个条目名在两侧是两种东西，这就是经典的 zip-slip。所以判据
    写在**字符串**上，而不是"宿主能不能创建出这个名字"。

    规则：

    - 非空、不以 `/` 或 `\\` 开头（绝对路径）；
    - 不含 `\\`（跨平台分隔符歧义）与 `:`（Windows 盘符 / 数据流）；
    - 不含控制字符（含 NUL）与 DEL；
    - 按 `/` 切分后每一段都非空、不是 `.`、不是 `..`（穿越）。

    刻意**不**做规范化（不折叠 `..`、不删掉 `.`）：规范化会把"要不要接受这个形状"变成
    "规范化之后看起来没问题"，而拒绝一个可疑条目名没有任何代价。
    """
    if not relative:
        raise PackError("技能包内不允许空路径")
    if relative.startswith(("/", "\\")):
        raise PackError(f"技能包内不允许绝对路径：{relative!r}")
    if "\\" in relative:
        raise PackError(f"技能包内路径不得包含反斜杠：{relative!r}")
    if ":" in relative:
        raise PackError(f"技能包内路径不得包含冒号：{relative!r}")
    if any(character < " " or character == "\x7f" for character in relative):
        raise PackError(f"技能包内路径不得包含控制字符：{relative!r}")
    for segment in relative.split("/"):
        if segment in ("", ".", ".."):
            raise PackError(f"技能包内路径不得包含空段或 '..'：{relative!r}")
    return relative


def _walk_entries(skill_dir: Path) -> list[Path]:
    """技能目录下的**全部条目**（含目录本身），不跟随符号链接。

    为什么不用 `Path.rglob`：CPython ≤3.12 的 `rglob` 不会递归进符号链接目录，于是
    "技能目录里有一个指向仓库外的链接目录"这件事在结果里**完全消失**——不是被拒绝，
    而是被静默跳过。跳过与拒绝看起来都能让打包成功，但前者意味着这个判定从来没跑过、
    也无从报告。这里显式逐层枚举，让目录链接本身也进入 `collect_files` 的判定。

    顺序按路径排序，因此枚举结果与文件系统的返回顺序无关。
    """
    pending = [skill_dir]
    entries: list[Path] = []
    while pending:
        current = pending.pop()
        for entry in sorted(current.iterdir()):
            entries.append(entry)
            if not _is_link_like(entry) and entry.is_dir():
                pending.append(entry)
    return entries


def collect_files(skill_dir: Path) -> list[tuple[str, bytes]]:
    """技能包内的全部文件，按**包内 POSIX 路径字符串**排序。

    排序键刻意用字符串而不是 `Path`：`Path` 的排序在不同平台上对大小写与分隔符的处理
    并不一致，而 manifest 与 zip 的条目顺序必须逐字节可复现。

    三条 fail-closed 规则：

    - 符号链接 / 重解析点一律拒绝：一个指向包外的链接会让"技能包的内容"变成"打包机器上的
      任意文件"；
    - 每个条目名都要过 [`validate_archive_entry`]：条目名是解压侧看到的路径，不能只在
      "宿主文件系统恰好不允许这种名字"时才安全；
    - 每个条目名都要在 [`PACKAGE_FILE_RULES`] 里：不在允许清单内的文件（密钥、编辑器
      临时文件、构建产物、任意草稿）会让打包失败，而不是被静默发出去。

    错误消息只带**路径**，绝不回显文件内容——被拒绝的文件很可能正是密钥。
    """
    entries: list[tuple[str, Path]] = []
    for path in _walk_entries(skill_dir):
        if _is_link_like(path):
            raise PackError(f"{path}: 技能包内不允许符号链接或重解析点")
        if path.is_dir():
            continue
        if not path.is_file():
            raise PackError(f"{path}: 技能包内只允许普通文件")
        relative = validate_archive_entry(path.relative_to(skill_dir).as_posix())
        if not is_allowed_package_file(relative):
            allowed = ", ".join(rule.pattern for rule in PACKAGE_FILE_RULES)
            raise PackError(
                f"{relative}: 不在技能包的允许清单内，拒绝打包。"
                f"技能目录下只允许这些文件：{allowed}；"
                "其它文件（密钥、临时文件、构建产物等）不会被分发，请先移出技能目录。"
            )
        entries.append((relative, path))

    collected = {relative for relative, _ in entries}
    for required in REQUIRED_PACKAGE_FILES:
        if required not in collected:
            raise PackError(f"{required}: 技能包缺少必需的 {required}")

    files: list[tuple[str, bytes]] = []
    for relative, path in sorted(entries, key=lambda entry: entry[0]):
        try:
            files.append((relative, path.read_bytes()))
        except OSError:
            # 只报路径：读取失败的常见原因是权限，而把异常文本拼进来会把宿主路径带出去。
            raise PackError(f"{relative}: 无法读取（权限或文件系统错误）") from None
    return files


def package_digest(files: list[tuple[str, bytes]]) -> str:
    """整包摘要：对 `路径 + 内容摘要` 的排序清单求 SHA-256。

    刻意包含路径：只对内容求摘要的话，"把 A 文件的内容挪到 B 文件名下"会得到同一个
    摘要，而那是两个不同的包。
    """
    hasher = hashlib.sha256()
    hasher.update(b"haven:skill-package:v1\n")
    for relative, data in files:
        hasher.update(relative.encode("utf-8"))
        hasher.update(b"\0")
        hasher.update(hashlib.sha256(data).hexdigest().encode("ascii"))
        hasher.update(b"\n")
    return hasher.hexdigest()


def resolve_pack_skill_dir(root: Path, skill_dir: Path) -> Path:
    """验证公开 `pack()` 入口的路径，拒绝目录本身及路径任一段的链接。

    不能先 `resolve()` 再检查：这会丢掉“传入目录本身是链接”的证据，尤其是指向仓库
    内另一目录的链接。先按词法路径逐段 `lstat`，再解析并做最终仓库边界检查。
    """
    lexical_root = root.absolute()
    root = root.resolve()
    candidate = Path(skill_dir)
    if not candidate.is_absolute():
        candidate = lexical_root / candidate
    if ".." in candidate.parts:
        raise PackError(f"技能目录不能包含 '..'：{skill_dir}")
    # Windows TEMP can use an 8.3 ancestor (RUNNER~1) while resolve() returns
    # runneradmin. Preserve the caller's lexical root until link checks finish;
    # a canonical candidate returned by the CLI is also allowed. Do not resolve
    # the candidate early: that would hide an in-repository junction.
    try:
        relative = candidate.relative_to(lexical_root)
        walked = lexical_root
    except ValueError:
        try:
            relative = candidate.relative_to(root)
            walked = root
        except ValueError as error:
            raise PackError(f"{skill_dir}: 技能目录必须在仓库根之内") from error
    if not relative.parts:
        raise PackError(f"{skill_dir}: 仓库根目录本身不是技能目录")

    for part in relative.parts:
        walked = walked / part
        if _is_link_like(walked):
            raise PackError(f"技能路径包含符号链接或重解析点：{walked}")

    try:
        resolved = candidate.resolve(strict=True)
    except OSError as error:
        raise PackError(f"{skill_dir}: 技能目录不存在或无法访问") from error
    if resolved == root or root not in resolved.parents or not resolved.is_dir():
        raise PackError(f"{skill_dir}: 技能目录必须是仓库内的真实目录")
    return resolved


def build_manifest(
    root: Path,
    skill_id: str,
    skill_dir: Path,
    version: str,
    server_package: str,
    archive_name: str,
    files: list[tuple[str, bytes]],
    tool_names: list[str],
    tool_names_source: str,
) -> dict[str, Any]:
    return {
        "formatVersion": MANIFEST_FORMAT_VERSION,
        "skill": {
            "id": skill_id,
            "sourcePath": skill_dir.relative_to(root).as_posix(),
            "version": version,
            "contentSha256": package_digest(files),
            "files": [
                {
                    "path": relative,
                    "bytes": len(data),
                    "sha256": hashlib.sha256(data).hexdigest(),
                }
                for relative, data in files
            ],
        },
        "archive": {"name": archive_name, "format": "zip"},
        "mcp": {
            "serverPackage": server_package,
            "serverVersion": version,
            "toolNames": tool_names,
            "toolNamesSource": tool_names_source,
        },
        # 清单提供完整性摘要，不是签名或官方来源证明；它本身与输入目录同源，不能验证目录来源。
        "provenance": {
            "attested": False,
            "note": "该清单仅描述本次输入的内容与版本一致性，不证明来自 Haven 官方仓库或未被修改。",
        },
        "install": {
            "owner": "user",
            "note": (
                "把 zip 解压到你所用客户端（Codex / Claude Code / DSH / Pi …）的技能目录。"
                "栖阅不会写你的全局 Agent 配置，也不会替你安装。"
            ),
            "requires": (
                "Haven 设置页显式开启「外部 Agent 接入」，并在客户端配置 HAVEN_MCP_BRIDGE=live "
                "与 HAVEN_MCP_ENDPOINT；默认关闭时工具返回 HAVEN_BRIDGE_UNAVAILABLE。"
            ),
        },
    }


def write_archive(target: Path, skill_id: str, files: list[tuple[str, bytes]]) -> None:
    """跨平台确定性写出 zip：固定顺序、时间戳、权限与不压缩存储方式。

    条目名在这里**再校验一次**：`skill_id` 平时由契约校验钉成 hyphen-case，但
    `pack()` 是公开入口（测试可以注入自己的 contract 模块），所以"skill_id 一定安全"
    不能是这里的隐含前提。前缀本身也要过同一套判据，否则 `../` 这类 id 会把条目名
    带到包外。
    """
    prefix = validate_archive_entry(skill_id)
    with zipfile.ZipFile(target, "w", compression=ARCHIVE_COMPRESSION) as archive:
        for relative, data in files:
            info = zipfile.ZipInfo(
                validate_archive_entry(f"{prefix}/{relative}"),
                date_time=ARCHIVE_TIMESTAMP,
            )
            info.create_system = 3
            info.compress_type = ARCHIVE_COMPRESSION
            info.external_attr = ARCHIVE_FILE_MODE << 16
            archive.writestr(info, data)


def pack(
    root: Path,
    skill_dir: Path,
    out_dir: Path,
    contract: Any | None = None,
) -> dict[str, Any]:
    """产出一份可信产物，返回 manifest。任何不满足前提的情况都抛 `PackError`。"""
    contract = contract if contract is not None else _load_contract_module()
    # `pack()` 是公开入口：先拒绝技能目录本身及包内链接，再运行会读取文件的契约检查。
    skill_dir = resolve_pack_skill_dir(root, skill_dir)
    root = root.resolve()
    out_dir = out_dir.resolve()

    if out_dir == skill_dir or skill_dir in out_dir.parents:
        # 兜底：解析后的目录必须仍在仓库内。`resolve_skill_dir` 已经拦过 `--skill`，
        # 但 `pack()` 也是公开入口（测试与其他工具直接调用），且 `manifest.skill.sourcePath`
        # 的 `relative_to(root)` 本来就假定技能目录在仓库里——把假定变成显式拒绝。
        raise PackError(f"{out_dir}: 输出目录不能位于技能目录内部")

    # ① 先按允许清单读取完整文件树（并拒绝链接与不安全条目名），再跑契约校验：
    #    契约校验会遍历技能目录下的文本文件，不能让它先沿链接或先看到不该进包的文件。
    files = collect_files(skill_dir)
    # ② 契约必须通过：不产出"未通过校验但照样能装"的包。
    errors = contract.check_skill(root, skill_dir)
    if errors:
        raise PackError("技能契约校验未通过：\n  - " + "\n  - ".join(errors))

    version, server_package = resolve_version(root)
    # skill_id 也进产物文件名，但契约校验已经把它钉成 hyphen-case（`^[a-z0-9-]+$`，
    # 不以 `-` 开头/结尾、无 `--`、≤64 字符），因此这里不再重复一遍形状判定。
    skill_id = read_frontmatter_name(contract, skill_dir)

    tool_names_source = contract.MCP_CONSTANTS
    tool_names, tool_error = contract.frozen_tool_names(root, tool_names_source)
    if tool_error is not None:
        # 读不到权威工具清单就不能写 manifest：一份"工具名为空"的清单比没有清单更糟。
        raise PackError(tool_error)

    archive_name = f"{skill_id}-{version}.zip"
    manifest = build_manifest(
        root, skill_id, skill_dir, version, server_package, archive_name, files, tool_names,
        tool_names_source,
    )

    out_dir.mkdir(parents=True, exist_ok=True)
    archive_target = out_dir / archive_name
    manifest_name = f"{skill_id}-{version}.manifest.json"
    manifest_target = out_dir / manifest_name
    archive_temp: Path | None = None
    manifest_temp: Path | None = None
    try:
        archive_fd, archive_temp_name = tempfile.mkstemp(
            prefix=f".{archive_name}.", suffix=".tmp", dir=out_dir
        )
        os.close(archive_fd)
        archive_temp = Path(archive_temp_name)

        manifest_fd, manifest_temp_name = tempfile.mkstemp(
            prefix=f".{manifest_name}.", suffix=".tmp", dir=out_dir
        )
        os.close(manifest_fd)
        manifest_temp = Path(manifest_temp_name)

        write_archive(archive_temp, skill_id, files)
        manifest_bytes = (
            json.dumps(manifest, ensure_ascii=False, indent=2, sort_keys=True) + "\n"
        ).encode("utf-8")
        manifest_temp.write_bytes(manifest_bytes)
        # 替换的是最终路径的目录项；若原目标是符号链接，不会写进它的外部目标。
        os.replace(archive_temp, archive_target)
        os.replace(manifest_temp, manifest_target)
    except PackError:
        raise
    except OSError as error:
        raise PackError("无法安全写入技能包产物") from error
    finally:
        for temporary in (archive_temp, manifest_temp):
            if temporary is not None:
                try:
                    temporary.unlink()
                except FileNotFoundError:
                    pass
    return manifest


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description="打包仓库内的 Skill 供外部 Agent 使用")
    parser.add_argument("--root", default=".", help="仓库根目录（默认当前目录）")
    parser.add_argument(
        "--skill",
        default=DEFAULT_SKILL,
        help=f"仓库内的技能目录，相对路径（默认 {DEFAULT_SKILL}）",
    )
    parser.add_argument("--out", required=True, help="产物目录（不存在则创建）")
    parser.add_argument("--quiet", action="store_true", help="只输出结论")
    args = parser.parse_args(argv)

    root = Path(args.root)
    try:
        # 先定位置再读文件：`--skill` 指到仓库外时，打包器会把机器上的任意目录当成技能包。
        skill_dir = resolve_skill_dir(root, args.skill)
        manifest = pack(root, skill_dir, Path(args.out))
    except PackError as error:
        print(f"pack-skill: 拒绝打包\n{error}", file=sys.stderr)
        return 1

    if not args.quiet:
        skill = manifest["skill"]
        print(f"技能：{skill['id']} {skill['version']}")
        print(f"内容摘要：{skill['contentSha256']}")
        print(f"文件：{len(skill['files'])} 个")
        print(
            f"MCP 工具：{len(manifest['mcp']['toolNames'])} 项"
            f"（来自 {manifest['mcp']['toolNamesSource']}）"
        )
    print(f"pack-skill: PASS -> {Path(args.out)}")
    return 0


if __name__ == "__main__":
    sys.exit(main())
