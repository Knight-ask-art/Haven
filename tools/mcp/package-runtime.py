#!/usr/bin/env python3
"""组装可随安装包分发的 Haven MCP 运行时（启动器 + Node 运行时 + server 产物）。

规范：[`docs/architecture/MCP_EXTERNAL_AGENT_TRANSPORT.md`](../../docs/architecture/MCP_EXTERNAL_AGENT_TRANSPORT.md) §9.1.1。

**解决什么问题。** 客户端配置里的 `command` 必须是"安装后真实可执行"的东西。
把 `node` 当命令，等于要求已安装用户自己装一个全局 Node（`engines.node` 声明的是
`>= 22.12.0`，见 `mcp/haven-mcp/package.json`）；把仓库路径当参数，
等于要求用户保留一份源码 checkout。两者都不是"安装后可用"。因此这里产出一个
**自包含目录**：

```text
<out>/haven-mcp/
  runtime/node.exe           ← 随包分发的 Node 运行时（自动配置写进客户端的 command）
  dist/**.js                 ← 已编译的 MCP server（自动配置写进客户端的 args）
  node_modules/**            ← 生产依赖
  package.json               ← 版本与入口声明
  haven-mcp-launcher.cmd     ← 人工/CI 用的启动器（**不是**自动配置写的那条命令）
<out>/haven-mcp-runtime-<version>.manifest.json
```

launcher 用 `%~dp0`（脚本自身所在目录）定位运行时与入口，因此整目录可以搬到任何位置，
不需要环境变量、不需要 PATH、不需要用户装任何东西。自动配置写的是**真实可执行文件**
`runtime/node.exe` + 入口 `dist/index.js`：`.cmd` 不是可执行映像，`shell: false` 的进程
创建不了它，而 `cmd.exe /D /S /C` 需要的那层引号由客户端决定、不由我们决定。

**这个脚本不执行 Node**，也不联网：它只做确定性的字节搬运与形状校验。Node 版本由
`--node-version`（默认取仓库根的 `.node-version`）声明，实际二进制的版本一致性由 CI
在打包后直接运行 `<bundle>/runtime/node.exe --version` 断言——那一步需要真的跑起来，
不属于本脚本。

**安全契约**（与 `tools/skills/pack-skill.py` 同一套思路：只搬运，不解释）：

- 技能/运行时目录里的符号链接与重解析点一律拒绝。一个指向包外的链接会让"分发的产物"
  变成"打包机器上的任意文件"；
- `dist/**` 只允许 `*.js`：`tsconfig.json` 关掉了 `declaration` 与 `sourceMap`，因此
  多出任何其它后缀都意味着构建配置被改过或混进了临时文件；
- 整个产物里不允许出现 `.env` / `.npmrc` / `*.pem` / `*.key` / `*.map` / `*.log` 这类
  本地配置与调试产物。它们不在 `dist` 下也可能出现在 `node_modules` 里；
- 产物只写到 `--out` 指定的目录，拒绝写到输入树内部；
- manifest 里**没有时间戳**：同一份输入重复打包得到逐字节相同的 manifest。

退出码：0 成功，1 被拒绝。
"""

from __future__ import annotations

import argparse
import hashlib
import json
import os
import re
import shutil
import stat
import sys
import tempfile
from pathlib import Path

DEFAULT_MCP_ROOT = "mcp/haven-mcp"
PRODUCT_MANIFEST = "前端/app/package.json"
MCP_MANIFEST = "mcp/haven-mcp/package.json"
NODE_VERSION_FILE = ".node-version"

MANIFEST_FORMAT_VERSION = 1

# 产物内部的固定布局。客户端配置里写的 command 就是 `<安装根>/<BUNDLE_DIR_NAME>/<LAUNCHER_NAME>`。
BUNDLE_DIR_NAME = "haven-mcp"
LAUNCHER_NAME = "haven-mcp-launcher.cmd"
RUNTIME_DIR_NAME = "runtime"
RUNTIME_NAME = "node.exe"
ENTRY_RELATIVE = "dist/index.js"

# 清单里的"安装根"占位符。打包器不知道用户把应用装到哪里，因此清单给的是模板；
# **自动配置必须把它换成真实路径**——把占位符原样写进客户端配置等于写了一条永远
# 启动不了的命令，而错误要等到客户端启动 MCP server 时才暴露。
INSTALLED_RESOURCE_PLACEHOLDER = "<installed-resource-dir>"

# `dist/**` 的允许清单：编译产物只可能是 `.js`。
DIST_FILE_PATTERN = re.compile(r"[A-Za-z0-9_./@-]+\.js")

# 生产依赖里必须真的存在这些包，否则打出来的包启动即失败。
REQUIRED_NODE_MODULES = ("@modelcontextprotocol/sdk", "zod")

# 任何位置都不允许出现的本地配置 / 凭据产物。
#
# 刻意**不**把 `.ts` 或 `.map` 放进来：`node_modules` 里的 `.d.ts` 与 sourcemap 是依赖
# 自带的正常文件，把它们判成违规只会让打包在某个无关的依赖升级后突然失败——那会训练
# 维护者去放宽这条检查，而不是修真正的问题。这里只挡"凭据与本地配置"这一类。
FORBIDDEN_FILE_NAMES = (".env", ".npmrc", ".DS_Store")
FORBIDDEN_SUFFIXES = (".log", ".pem", ".key", ".p12", ".pfx")

VERSION_PATTERN = re.compile(r"^\d+\.\d+\.\d+(?:-[0-9A-Za-z.-]+)?(?:\+[0-9A-Za-z.-]+)?$")
MAX_VERSION_CHARS = 128

FILE_ATTRIBUTE_REPARSE_POINT = getattr(stat, "FILE_ATTRIBUTE_REPARSE_POINT", 0x400)


class PackageError(Exception):
    """打包被拒绝。消息里只带路径与原因，绝不回显文件内容。"""


def _is_link_like(path: Path) -> bool:
    """符号链接，或 Windows 上的目录联接 / 其它重解析点。"""
    try:
        info = path.lstat()
    except OSError:
        return False
    if stat.S_ISLNK(info.st_mode):
        return True
    attributes = getattr(info, "st_file_attributes", 0)
    return bool(attributes & FILE_ATTRIBUTE_REPARSE_POINT)


def _resolve_inside(root: Path, relative: str, *, label: str) -> Path:
    """把仓库内的相对路径解析成真实目录，逐段拒绝链接与 `..`。"""
    raw = relative.strip()
    if not raw:
        raise PackageError(f"{label} 不能为空")
    if raw.startswith(("/", "\\")) or Path(raw).is_absolute() or re.match(r"^[A-Za-z]:", raw):
        raise PackageError(f"{label} 必须是仓库内的相对路径：{relative!r}")

    parts = [part for part in re.split(r"[\\/]+", raw) if part not in ("", ".")]
    if not parts or ".." in parts:
        raise PackageError(f"{label} 不能包含 '..' 或指向仓库之外：{relative!r}")

    walked = root
    for part in parts:
        walked = walked / part
        if _is_link_like(walked):
            raise PackageError(f"{label} 路径包含符号链接或重解析点：{walked}")

    resolved = root.joinpath(*parts).resolve()
    if resolved == root or root not in resolved.parents:
        raise PackageError(f"{label} 必须指向仓库内的目录：{relative!r}")
    if not resolved.is_dir():
        raise PackageError(f"{label} 不是一个目录：{relative!r}")
    return resolved


def _read_json(root: Path, relative: str) -> dict:
    try:
        value = json.loads((root / relative).read_text(encoding="utf-8"))
    except (OSError, UnicodeError, json.JSONDecodeError) as error:
        raise PackageError(f"{relative}: 无法解析（{error}）") from None
    if not isinstance(value, dict):
        raise PackageError(f"{relative}: 顶层必须是对象")
    return value


def resolve_version(root: Path) -> str:
    """产品版本；要求与 MCP server 版本一致。

    版本错配是用户最难自查的一类失败："装上去的启动器"与"栖阅期望的 server"不是同一份。
    """
    product = _read_json(root, PRODUCT_MANIFEST).get("version")
    server = _read_json(root, MCP_MANIFEST).get("version")
    if not isinstance(product, str) or VERSION_PATTERN.fullmatch(product) is None:
        raise PackageError(f"{PRODUCT_MANIFEST}: version {product!r} 不能用作产物文件名")
    if len(product) > MAX_VERSION_CHARS:
        raise PackageError(f"{PRODUCT_MANIFEST}: version 过长")
    if product != server:
        raise PackageError(
            f"版本不一致：{PRODUCT_MANIFEST}={product!r} 与 {MCP_MANIFEST}={server!r}；"
            "启动器与 MCP server 必须是同一个版本"
        )
    return product


def walk_files(base: Path, *, label: str) -> list[tuple[str, Path]]:
    """`base` 下的全部**普通文件**，键为相对 `base` 的 POSIX 路径。

    显式逐层枚举而不是 `Path.rglob`：CPython ≤3.12 的 `rglob` 不递归进符号链接目录，
    于是"里面有一个指向包外的链接目录"会**静默消失**（不是被拒绝，而是从结果里没了）。
    这里让链接本身也进入判定。
    """
    pending = [base]
    found: list[tuple[str, Path]] = []
    while pending:
        current = pending.pop()
        try:
            children = sorted(current.iterdir())
        except OSError as error:
            raise PackageError(f"{label}: 无法读取 {current}（{error}）") from None
        for child in children:
            if _is_link_like(child):
                raise PackageError(f"{label}: 不允许符号链接或重解析点：{child}")
            if child.is_dir():
                pending.append(child)
                continue
            if not child.is_file():
                raise PackageError(f"{label}: 只允许普通文件：{child}")
            found.append((child.relative_to(base).as_posix(), child))
    found.sort(key=lambda entry: entry[0])
    return found


def _check_forbidden(relative: str, *, label: str) -> None:
    name = relative.rsplit("/", 1)[-1]
    if name in FORBIDDEN_FILE_NAMES:
        raise PackageError(f"{label}: 产物里不允许本地配置文件：{relative}")
    lowered = relative.lower()
    for suffix in FORBIDDEN_SUFFIXES:
        if lowered.endswith(suffix):
            raise PackageError(f"{label}: 产物里不允许调试或本地产物：{relative}")


def _sha256_file(path: Path) -> str:
    """流式摘要：随包分发的 Node 运行时是上百 MB 的二进制，不整个读进内存。"""
    digest = hashlib.sha256()
    with path.open("rb") as handle:
        for chunk in iter(lambda: handle.read(1024 * 1024), b""):
            digest.update(chunk)
    return digest.hexdigest()


def _node_module_key(relative: str) -> str:
    """`node_modules` 下的相对路径 → 它所属的包名（含 scope）。"""
    parts = relative.split("/")
    if len(parts) > 1 and parts[0].startswith("@"):
        return f"{parts[0]}/{parts[1]}"
    return parts[0]


def collect_bundle(mcp_root: Path) -> list[tuple[str, Path]]:
    """产物树的相对路径 → 源文件。顺序固定，因此输出可复现。"""
    entry = mcp_root / ENTRY_RELATIVE
    if not entry.is_file():
        raise PackageError(
            f"{mcp_root / ENTRY_RELATIVE}: 找不到已编译的 MCP server；"
            "请先在 mcp/haven-mcp 里 npm ci && npm run build"
        )

    node_modules = mcp_root / "node_modules"
    if not node_modules.is_dir():
        raise PackageError(f"{node_modules}: 找不到生产依赖；请先 npm ci --omit=dev")
    # `npm` 自己的目录不参与搬运：它只描述依赖图，不需要分发。
    skip_roots = {".bin", ".package-lock.json"}

    package_json = mcp_root / "package.json"
    if not package_json.is_file():
        raise PackageError(f"{package_json}: 缺少 package.json")
    entries: list[tuple[str, Path]] = [("package.json", package_json)]

    for relative, path in walk_files(mcp_root / "dist", label="dist"):
        if DIST_FILE_PATTERN.fullmatch(relative) is None:
            raise PackageError(
                f"dist: 只允许已编译的 .js：{relative}"
                "（tsconfig 关闭了 declaration 与 sourceMap，多出来的后缀说明构建配置被改过）"
            )
        entries.append((f"dist/{relative}", path))

    node_module_keys: set[str] = set()
    for relative, path in walk_files(node_modules, label="node_modules"):
        if relative.split("/", 1)[0] in skip_roots:
            continue
        _check_forbidden(relative, label="node_modules")
        node_module_keys.add(_node_module_key(relative))
        entries.append((f"node_modules/{relative}", path))

    for required in REQUIRED_NODE_MODULES:
        if required not in node_module_keys:
            raise PackageError(
                f"node_modules: 缺少必需的生产依赖 {required!r}；"
                "请用 npm ci --omit=dev 重新安装"
            )

    entries.sort(key=lambda entry: entry[0])
    return entries


def launcher_script() -> bytes:
    """生成 Windows 启动器。

    `%~dp0` 是脚本自身所在目录（带尾随反斜杠），因此整个产物目录可以整体搬走。
    `%*` 原样转发客户端传进来的参数；不写 `endlocal`，退出码自然沿用 node 的。

    **内容保持纯 ASCII**：`.cmd` 由 `cmd.exe` 按控制台代码页解释，中文注释在非 UTF-8
    代码页的机器上会变成乱码字节；而脚本一旦带上 BOM，第一行就不再是 `@echo off`。
    这是本仓库里少数刻意不用中文的地方，理由就是编码。

    这个脚本**不是**自动配置写进客户端的那条命令：`.cmd` 不是可执行映像，`shell: false`
    的进程创建不了它（Rust 侧 `CreateProcess` 失败，Node 新版本直接抛 `EINVAL`），
    而走 `cmd.exe /D /S /C` 又要求把路径再包一层引号——那是客户端的引号，不由我们决定。
    因此 `clientConfig` 指向同目录下真实的 `runtime/node.exe` + `dist/index.js`；
    这里保留脚本是给人工排查、命令行以及 CI 的启动器往返用例用的。
    """
    entry = ENTRY_RELATIVE.replace("/", "\\")
    lines = [
        "@echo off",
        "rem Haven MCP launcher: shipped inside the installer, needs no global Node and no source checkout.",
        "setlocal",
        f'"%~dp0{RUNTIME_DIR_NAME}\\{RUNTIME_NAME}" "%~dp0{entry}" %*',
        "",
    ]
    return "\r\n".join(lines).encode("ascii")


def node_version(root: Path, explicit: str | None) -> str:
    declared = explicit
    if declared is None:
        try:
            declared = (root / NODE_VERSION_FILE).read_text(encoding="utf-8")
        except (OSError, UnicodeError):
            raise PackageError(
                f"读不到 {NODE_VERSION_FILE}；请用 --node-version 显式声明"
            ) from None
    declared = declared.strip()
    if not declared:
        raise PackageError("Node 版本不能为空（用 --node-version 或仓库根的 .node-version）")
    if len(declared) > MAX_VERSION_CHARS or any(ch in declared for ch in "/\\: "):
        raise PackageError(f"Node 版本 {declared!r} 不能用作清单字段")
    return declared


def build_manifest(
    version: str,
    node_version_text: str,
    entries: list[tuple[str, Path]],
    node_path: Path,
) -> dict:
    files = [
        {
            "path": f"{RUNTIME_DIR_NAME}/{RUNTIME_NAME}",
            "bytes": node_path.stat().st_size,
            "sha256": _sha256_file(node_path),
        },
        {"path": LAUNCHER_NAME, "bytes": len(launcher_script())},
    ]
    for relative, path in entries:
        data = path.read_bytes()
        files.append(
            {
                "path": relative,
                "bytes": len(data),
                "sha256": hashlib.sha256(data).hexdigest(),
            }
        )
    files.sort(key=lambda entry: entry["path"])

    return {
        "formatVersion": MANIFEST_FORMAT_VERSION,
        "product": "haven-mcp-runtime",
        "version": version,
        "nodeVersion": node_version_text,
        "layout": {
            "bundleDirectory": BUNDLE_DIR_NAME,
            "launcher": LAUNCHER_NAME,
            "runtime": f"{RUNTIME_DIR_NAME}/{RUNTIME_NAME}",
            "entry": ENTRY_RELATIVE,
        },
        "clientConfig": {
            # 安装后由栖阅把 `<installed-resource-dir>` 换成真实的安装根，因此客户端配置里
            # **不会**出现仓库路径，也不会出现裸 `node`（那是用户机器上的全局运行时，
            # 已安装的机器上不一定有）。
            #
            # 命令是随包分发的**真实可执行文件** + 入口脚本两个普通参数：
            # 客户端用 `shell: false` 直接创建进程，两个含空格的绝对路径由客户端按
            # 标准 Windows 参数引号规则包起来即可原样送达，不需要 `cmd.exe`，也不需要
            # 用户机器上存在任何东西。指向 `.cmd` 启动器做不到这一点（见 `launcher_script`）。
            "command": f"{INSTALLED_RESOURCE_PLACEHOLDER}/{BUNDLE_DIR_NAME}/{RUNTIME_DIR_NAME}/{RUNTIME_NAME}",
            "args": [f"{INSTALLED_RESOURCE_PLACEHOLDER}/{BUNDLE_DIR_NAME}/{ENTRY_RELATIVE}"],
            "env": {
                "HAVEN_MCP_BRIDGE": "live",
                "HAVEN_MCP_ENDPOINT": "<copy-from-haven-ui>",
            },
        },
        "verified": {
            "bundled": True,
            "runtimeExecuted": False,
            "note": (
                "本清单只描述输入内容与版本一致性，不代表启动器已在真实客户端上连通过。"
                "运行时二进制的版本一致性由 CI 直接执行 node.exe --version 断言。"
            ),
        },
        "files": files,
    }


def package_runtime(
    root: Path,
    out_dir: Path,
    node_path: Path,
    *,
    mcp_root_relative: str = DEFAULT_MCP_ROOT,
    node_version_override: str | None = None,
) -> dict:
    root = root.resolve()
    mcp_root = _resolve_inside(root, mcp_root_relative, label="--mcp-root")
    out_dir = out_dir.resolve()

    if out_dir == mcp_root or mcp_root in out_dir.parents:
        raise PackageError(f"{out_dir}: 输出目录不能位于 MCP server 目录内部")

    if _is_link_like(node_path):
        raise PackageError(f"{node_path}: 运行时二进制不能是符号链接或重解析点")
    if not node_path.is_file():
        raise PackageError(f"{node_path}: 找不到 Node 运行时二进制")

    version = resolve_version(root)
    declared_node = node_version(root, node_version_override)
    entries = collect_bundle(mcp_root)

    bundle_root = out_dir / BUNDLE_DIR_NAME
    manifest = build_manifest(version, declared_node, entries, node_path)

    temp_root: Path | None = None
    try:
        out_dir.mkdir(parents=True, exist_ok=True)
        temp_root = Path(tempfile.mkdtemp(prefix=f".{BUNDLE_DIR_NAME}.", dir=out_dir))
        staged = temp_root / BUNDLE_DIR_NAME
        (staged / RUNTIME_DIR_NAME).mkdir(parents=True)
        shutil.copyfile(node_path, staged / RUNTIME_DIR_NAME / RUNTIME_NAME)
        (staged / LAUNCHER_NAME).write_bytes(launcher_script())
        for relative, path in entries:
            target = staged / relative
            target.parent.mkdir(parents=True, exist_ok=True)
            shutil.copyfile(path, target)

        manifest_target = out_dir / f"haven-mcp-runtime-{version}.manifest.json"
        manifest_bytes = (
            json.dumps(manifest, ensure_ascii=False, indent=2, sort_keys=True) + "\n"
        ).encode("utf-8")
        (temp_root / manifest_target.name).write_bytes(manifest_bytes)

        # 先删旧目录再改名：`os.replace` 不能把目录覆盖到非空目录上。
        if bundle_root.exists():
            shutil.rmtree(bundle_root)
        os.replace(staged, bundle_root)
        os.replace(temp_root / manifest_target.name, manifest_target)
    except PackageError:
        raise
    except OSError as error:
        raise PackageError(f"无法安全写入运行时产物（{error}）") from None
    finally:
        if temp_root is not None:
            shutil.rmtree(temp_root, ignore_errors=True)
    return manifest


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description="组装随安装包分发的 Haven MCP 运行时")
    parser.add_argument("--root", default=".", help="仓库根目录（默认当前目录）")
    parser.add_argument("--mcp-root", default=DEFAULT_MCP_ROOT, help="MCP server 包目录")
    parser.add_argument("--out", required=True, help="产物目录（不存在则创建）")
    parser.add_argument(
        "--node",
        required=True,
        help="要随包分发的 Node 运行时二进制（Windows 上是 node.exe）",
    )
    parser.add_argument(
        "--node-version",
        default=None,
        help=f"声明的 Node 版本；默认取仓库根的 {NODE_VERSION_FILE}",
    )
    parser.add_argument("--quiet", action="store_true", help="只输出结论")
    args = parser.parse_args(argv)

    root = Path(args.root)
    try:
        manifest = package_runtime(
            root,
            Path(args.out),
            Path(args.node),
            mcp_root_relative=args.mcp_root,
            node_version_override=args.node_version,
        )
    except PackageError as error:
        print(f"package-runtime: 拒绝打包\n{error}", file=sys.stderr)
        return 1

    if not args.quiet:
        layout = manifest["layout"]
        print(f"版本：{manifest['version']}（Node {manifest['nodeVersion']}）")
        print(f"文件：{len(manifest['files'])} 个")
        print(f"启动器：{layout['bundleDirectory']}/{layout['launcher']}")
        print(f"客户端命令：{manifest['clientConfig']['command']}")
    print(f"package-runtime: PASS -> {Path(args.out)}")
    return 0


if __name__ == "__main__":
    # The CLI owns its byte streams; imported library callers retain theirs.
    sys.stdout.reconfigure(encoding="utf-8", errors="backslashreplace")
    sys.stderr.reconfigure(encoding="utf-8", errors="backslashreplace")
    raise SystemExit(main())
