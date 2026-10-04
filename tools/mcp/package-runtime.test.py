#!/usr/bin/env python3
"""`package-runtime.py` 的自测。

每一条用例都对应打包器的一条**拒绝**或一条**承诺**；没有"顺手跑一下不报错"的用例。

为什么要为一个 Python 打包脚本写这么多：它的产物是客户端配置里 `command` 指向的东西。
一旦它悄悄多装了一个本地配置文件、少装了一个生产依赖、或者把可执行目标写成仓库路径，
症状都会出现在**别人的机器上**——而那时已经没有构建日志可看。
"""

from __future__ import annotations

import json
import os
import subprocess
import sys
import tempfile
import unittest
from pathlib import Path


def _load(name: str):
    import importlib.util

    path = Path(__file__).resolve().parent / name
    spec = importlib.util.spec_from_file_location(path.stem, path)
    assert spec is not None and spec.loader is not None
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


PACK = _load("package-runtime.py")

REPOSITORY_ROOT = Path(__file__).resolve().parents[2]
NODE_VERSION = (REPOSITORY_ROOT / ".node-version").read_text(encoding="utf-8").strip()


def _make_directory_link(link: Path, target: Path) -> bool:
    """尽力创建目录联接 / 目录符号链接；平台不允许时返回 False。"""
    if not link.exists():
        try:
            completed = subprocess.run(
                ["cmd", "/c", "mklink", "/J", str(link), str(target)],
                capture_output=True,
                check=False,
            )
        except OSError:
            completed = None
        if completed is not None and completed.returncode == 0 and link.exists():
            return True
    try:
        link.symlink_to(target, target_is_directory=True)
    except (OSError, NotImplementedError):
        return False
    return link.exists()


def _write_json(path: Path, payload: dict) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(json.dumps(payload), encoding="utf-8")


def _make_minimal_repository(root: Path, version: str = "0.1.0-beta.1") -> Path:
    """一个只够打包器跑完前提检查的最小仓库树；返回 MCP server 目录。"""
    # 先建根目录：`.node-version` 是第一个被写下的文件，而它自己没有 `mkdir`。
    root.mkdir(parents=True, exist_ok=True)
    (root / ".node-version").write_text(NODE_VERSION + "\n", encoding="utf-8")
    _write_json(root / "前端/app/package.json", {"name": "app", "version": version})
    mcp_root = root / "mcp/haven-mcp"
    _write_json(
        mcp_root / "package.json",
        {"name": "haven-mcp-server", "version": version, "type": "module"},
    )
    (mcp_root / "dist").mkdir(parents=True)
    (mcp_root / "dist/index.js").write_text("#!/usr/bin/env node\n", encoding="utf-8")
    (mcp_root / "dist/server.js").write_text("export {};\n", encoding="utf-8")
    for package in PACK.REQUIRED_NODE_MODULES:
        _write_json(
            mcp_root / "node_modules" / package / "package.json",
            {"name": package, "version": "1.0.0"},
        )
    return mcp_root


def _stub_node(root: Path, content: bytes = b"MZ-stub-node-runtime") -> Path:
    path = root / "node.exe"
    path.write_bytes(content)
    return path


def _run_main(arguments: list[str]) -> int:
    """安静地跑 `main()`：拒绝路径会把中文原因写到 stderr，测试输出里不用再重复。"""
    import contextlib
    import io

    with contextlib.redirect_stderr(io.StringIO()):
        return PACK.main(arguments)


def _run_cli_with_legacy_codepage(arguments: list[str]) -> subprocess.CompletedProcess[bytes]:
    return subprocess.run(
        [sys.executable, str(REPOSITORY_ROOT / "tools/mcp/package-runtime.py"), *arguments],
        env={**os.environ, "PYTHONIOENCODING": "cp1252:strict", "PYTHONUTF8": "0"},
        capture_output=True,
        check=False,
        timeout=30,
    )


class PackageRuntimeTests(unittest.TestCase):
    def test_repository_pins_an_exact_node_runtime_version(self) -> None:
        self.assertRegex(NODE_VERSION, r"^[0-9]+\.[0-9]+\.[0-9]+$")

    def test_importing_the_packager_does_not_reconfigure_caller_streams(self) -> None:
        streams = (sys.stdout, sys.stderr)
        encodings = tuple(stream.encoding for stream in streams)
        _load("package-runtime.py")
        self.assertIs(sys.stdout, streams[0])
        self.assertIs(sys.stderr, streams[1])
        self.assertEqual(tuple(stream.encoding for stream in streams), encodings)

    def test_cli_packages_non_ascii_paths_with_inherited_legacy_codepage(self) -> None:
        with tempfile.TemporaryDirectory() as temporary_directory:
            base = Path(temporary_directory)
            root = base / "输入 仓库"
            _make_minimal_repository(root)
            node = _stub_node(base)
            for quiet in (False, True):
                with self.subTest(quiet=quiet):
                    out = base / "产物 输出" / ("quiet" if quiet else "verbose")
                    arguments = ["--root", str(root), "--out", str(out), "--node", str(node)]
                    if quiet:
                        arguments.append("--quiet")
                    result = _run_cli_with_legacy_codepage(arguments)
                    stdout = result.stdout.decode("utf-8")
                    stderr = result.stderr.decode("utf-8")
                    self.assertEqual(result.returncode, 0, stderr)
                    self.assertEqual(stderr, "")
                    self.assertIn(f"package-runtime: PASS -> {out}", stdout)
                    self.assertEqual("版本：" in stdout, not quiet)
                    self.assertTrue((out / PACK.BUNDLE_DIR_NAME / PACK.LAUNCHER_NAME).is_file())

    def test_cli_help_is_utf8_with_inherited_legacy_codepage(self) -> None:
        result = _run_cli_with_legacy_codepage(["--help"])
        self.assertEqual(result.returncode, 0, result.stderr.decode("utf-8"))
        self.assertIn("组装随安装包分发的 Haven MCP 运行时", result.stdout.decode("utf-8"))
        self.assertEqual(result.stderr, b"")

    def test_cli_rejection_is_utf8_and_still_fails_with_inherited_legacy_codepage(self) -> None:
        with tempfile.TemporaryDirectory() as temporary_directory:
            base = Path(temporary_directory)
            root = base / "输入 仓库"
            _make_minimal_repository(root)
            out = base / "拒绝 产物"
            result = _run_cli_with_legacy_codepage(
                ["--root", str(root), "--out", str(out), "--node", str(base / "不存在.exe")]
            )
            stderr = result.stderr.decode("utf-8")
            self.assertEqual(result.returncode, 1, stderr)
            self.assertEqual(result.stdout, b"")
            self.assertIn("package-runtime: 拒绝打包", stderr)
            self.assertIn("找不到 Node 运行时二进制", stderr)
            self.assertNotIn("Traceback", stderr)
            self.assertNotIn("UnicodeEncodeError", stderr)
            self.assertFalse((out / PACK.BUNDLE_DIR_NAME).exists())

    def test_the_bundle_carries_the_launcher_runtime_entry_and_production_dependencies(self) -> None:
        with tempfile.TemporaryDirectory() as temporary_directory:
            base = Path(temporary_directory)
            root = base / "repo"
            _make_minimal_repository(root)
            node = _stub_node(base)

            PACK.package_runtime(root, base / "out", node)

            bundle = base / "out" / PACK.BUNDLE_DIR_NAME
            self.assertTrue((bundle / PACK.LAUNCHER_NAME).is_file())
            self.assertTrue((bundle / PACK.RUNTIME_DIR_NAME / PACK.RUNTIME_NAME).is_file())
            self.assertTrue((bundle / PACK.ENTRY_RELATIVE).is_file())
            self.assertTrue((bundle / "package.json").is_file())
            for package in PACK.REQUIRED_NODE_MODULES:
                self.assertTrue((bundle / "node_modules" / package / "package.json").is_file())

    def test_the_launcher_is_relative_to_itself_and_forwards_arguments(self) -> None:
        """启动器必须只依赖**自身所在目录**，并原样转发参数。

        用 `%~dp0` 而不是绝对路径：产物目录要能整体搬走（安装位置因机器而异）。
        用 `%*` 转发：MCP 客户端可能传 `--` 之类的东西，吞掉它们会改变 server 行为。
        """
        text = PACK.launcher_script().decode("ascii")

        self.assertTrue(text.startswith("@echo off\r\n"))
        self.assertIn(f'"%~dp0{PACK.RUNTIME_DIR_NAME}\\{PACK.RUNTIME_NAME}"', text)
        self.assertIn('"%~dp0dist\\index.js"', text)
        self.assertIn("%*", text)
        # 不得依赖 PATH、不得依赖工作目录、不得依赖环境变量。
        self.assertNotIn("cd ", text)
        self.assertNotIn("%CD%", text)
        self.assertNotIn("%PATH%", text)
        # 也不得把仓库路径写死进去。
        self.assertNotIn("mcp/haven-mcp", text)

    def test_the_launcher_is_ascii_so_cmd_cannot_mangle_it(self) -> None:
        """`.cmd` 由 `cmd.exe` 按控制台代码页解释：内容必须是纯 ASCII。

        回归点：脚本里写中文注释却 `encode("ascii")`，打包器会在**每一台**机器上抛
        `UnicodeEncodeError`。改成 `encode("utf-8")` 不是修法——非 UTF-8 代码页下注释
        会变成乱码字节，而带 BOM 的脚本第一行也不再是 `@echo off`。
        """
        raw = PACK.launcher_script()
        raw.decode("ascii")
        self.assertFalse(raw.startswith(b"\xef\xbb\xbf"), "启动器不得带 UTF-8 BOM")

    def test_the_configured_command_is_a_real_executable_and_survives_a_spaced_path(self) -> None:
        """自动配置写下的 command/args 必须在**含空格的安装路径**下也能启动。

        这是"安装后可用"的全部内容：客户端用 `shell: false` 创建进程，因此

        - command 必须是**真实可执行文件**（随包分发的 `node.exe`），不能是 `.cmd`
          启动器——脚本不是可执行映像，`CreateProcess` 创建不了它；
        - 两个参数都是普通绝对路径：客户端按标准 Windows 引号规则包一层即可原样送达。

        用例把模板里的占位符换成一段**带空格**的真实安装路径，验证替换后没有残留
        占位符、没有 shell 元字符，并把两个参数交给一个真实子进程，确认它们各自
        原样到达（而不是被空格切成两段）。
        """
        with tempfile.TemporaryDirectory() as temporary_directory:
            base = Path(temporary_directory)
            root = base / "repo"
            _make_minimal_repository(root)

            manifest = PACK.package_runtime(root, base / "out", _stub_node(base))
            config = manifest["clientConfig"]

            install_root = base / "Program Files" / "Haven 栖阅"
            resolved = [config["command"], *config["args"]]
            for value in resolved:
                self.assertIn(PACK.INSTALLED_RESOURCE_PLACEHOLDER, value)
                self.assertNotIn("<", value.replace(PACK.INSTALLED_RESOURCE_PLACEHOLDER, ""))
            concrete = [
                value.replace(PACK.INSTALLED_RESOURCE_PLACEHOLDER, str(install_root))
                for value in resolved
            ]
            self.assertEqual(
                [Path(value).name for value in concrete],
                [PACK.RUNTIME_NAME, Path(PACK.ENTRY_RELATIVE).name],
            )
            # 比的是**路径语义**，不是分隔符字节：清单模板用 `/` 拼出相对部分，
            # 而 `Path` 在 Windows 上把同一段路径拼成 `\` —— 两者指向同一个文件。
            # 断言"解析到安装后那两个文件"这件事不变，只是不再把分隔符风格也钉进去。
            self.assertEqual(
                Path(concrete[0]),
                install_root / PACK.BUNDLE_DIR_NAME / PACK.RUNTIME_DIR_NAME / PACK.RUNTIME_NAME,
            )
            self.assertEqual(
                Path(concrete[1]),
                install_root / PACK.BUNDLE_DIR_NAME / PACK.ENTRY_RELATIVE,
            )
            # 路径里只有 `/` 与普通字符：不得出现引号、`&`、`|`、`^` 这类需要 shell 解释的东西。
            for value in concrete:
                for hostile in ('"', "&", "|", "^", "<", ">", "%"):
                    self.assertNotIn(hostile, value)

            # 真实子进程往返：两个含空格的参数必须各自完整到达。
            completed = subprocess.run(
                [sys.executable, "-c", "import json,sys;print(json.dumps(sys.argv[1:]))", *concrete],
                capture_output=True,
                check=True,
                text=True,
            )
            self.assertEqual(json.loads(completed.stdout), concrete)

    def test_the_manifest_declares_an_installed_command_not_a_repository_path(self) -> None:
        """配置里的 command 必须是"安装后真实可执行"的东西。

        不是裸 `node`（用户机器上不一定有全局 Node），不是仓库路径（要求用户保留
        checkout），也不是 `.cmd` 启动器（`shell: false` 创建不了脚本）。
        """
        with tempfile.TemporaryDirectory() as temporary_directory:
            base = Path(temporary_directory)
            root = base / "repo"
            _make_minimal_repository(root)

            manifest = PACK.package_runtime(root, base / "out", _stub_node(base))

            config = manifest["clientConfig"]
            placeholder = PACK.INSTALLED_RESOURCE_PLACEHOLDER
            self.assertEqual(
                config["command"],
                f"{placeholder}/{PACK.BUNDLE_DIR_NAME}/{PACK.RUNTIME_DIR_NAME}/{PACK.RUNTIME_NAME}",
            )
            self.assertEqual(
                config["args"],
                [f"{placeholder}/{PACK.BUNDLE_DIR_NAME}/{PACK.ENTRY_RELATIVE}"],
            )
            # 环境变量只有两个，与 README 和设计文档里的约定一致。
            self.assertEqual(
                config["env"],
                {"HAVEN_MCP_BRIDGE": "live", "HAVEN_MCP_ENDPOINT": "<copy-from-haven-ui>"},
            )
            # 命令是随包分发的运行时，不是启动器脚本。
            self.assertNotIn(PACK.LAUNCHER_NAME, config["command"])
            self.assertTrue(config["command"].endswith(PACK.RUNTIME_NAME))
            self.assertEqual(manifest["layout"]["entry"], PACK.ENTRY_RELATIVE)

    def test_manifest_digests_match_the_shipped_bytes(self) -> None:
        with tempfile.TemporaryDirectory() as temporary_directory:
            base = Path(temporary_directory)
            root = base / "repo"
            _make_minimal_repository(root)

            manifest = PACK.package_runtime(root, base / "out", _stub_node(base))

            bundle = base / "out" / PACK.BUNDLE_DIR_NAME
            listed = {entry["path"] for entry in manifest["files"]}
            self.assertIn(PACK.LAUNCHER_NAME, listed)
            self.assertIn(f"{PACK.RUNTIME_DIR_NAME}/{PACK.RUNTIME_NAME}", listed)
            for entry in manifest["files"]:
                shipped = bundle / entry["path"]
                self.assertTrue(shipped.is_file(), f"{entry['path']} 必须在产物里")
                self.assertEqual(shipped.stat().st_size, entry["bytes"], entry["path"])
                if "sha256" in entry:
                    import hashlib

                    self.assertEqual(
                        hashlib.sha256(shipped.read_bytes()).hexdigest(),
                        entry["sha256"],
                        entry["path"],
                    )

    def test_packaging_twice_is_byte_identical(self) -> None:
        with tempfile.TemporaryDirectory() as temporary_directory:
            base = Path(temporary_directory)
            root = base / "repo"
            _make_minimal_repository(root)
            node = _stub_node(base)

            first = PACK.package_runtime(root, base / "one", node)
            second = PACK.package_runtime(root, base / "two", node)

            self.assertEqual(
                json.dumps(first, sort_keys=True), json.dumps(second, sort_keys=True)
            )
            self.assertEqual(
                (base / "one/haven-mcp-runtime-0.1.0-beta.1.manifest.json").read_bytes(),
                (base / "two/haven-mcp-runtime-0.1.0-beta.1.manifest.json").read_bytes(),
            )

    def test_a_version_mismatch_between_product_and_server_is_rejected(self) -> None:
        with tempfile.TemporaryDirectory() as temporary_directory:
            base = Path(temporary_directory)
            root = base / "repo"
            _make_minimal_repository(root)
            _write_json(root / "mcp/haven-mcp/package.json", {"version": "9.9.9"})

            with self.assertRaises(PACK.PackageError) as raised:
                PACK.package_runtime(root, base / "out", _stub_node(base))
            self.assertIn("版本不一致", str(raised.exception))
            self.assertFalse((base / "out" / PACK.BUNDLE_DIR_NAME).exists())

    def test_a_missing_build_output_is_rejected(self) -> None:
        with tempfile.TemporaryDirectory() as temporary_directory:
            base = Path(temporary_directory)
            root = base / "repo"
            mcp_root = _make_minimal_repository(root)
            (mcp_root / "dist/index.js").unlink()

            with self.assertRaises(PACK.PackageError) as raised:
                PACK.package_runtime(root, base / "out", _stub_node(base))
            self.assertIn("已编译", str(raised.exception))

    def test_a_missing_production_dependency_is_rejected(self) -> None:
        with tempfile.TemporaryDirectory() as temporary_directory:
            base = Path(temporary_directory)
            root = base / "repo"
            mcp_root = _make_minimal_repository(root)
            import shutil

            shutil.rmtree(mcp_root / "node_modules/zod")

            with self.assertRaises(PACK.PackageError) as raised:
                PACK.package_runtime(root, base / "out", _stub_node(base))
            self.assertIn("zod", str(raised.exception))

    def test_non_js_files_in_dist_are_rejected(self) -> None:
        """`tsconfig` 关掉了 declaration / sourceMap：多出别的后缀就是构建配置被改过。"""
        with tempfile.TemporaryDirectory() as temporary_directory:
            base = Path(temporary_directory)
            root = base / "repo"
            mcp_root = _make_minimal_repository(root)
            (mcp_root / "dist/index.d.ts").write_text("export {};\n", encoding="utf-8")

            with self.assertRaises(PACK.PackageError) as raised:
                PACK.package_runtime(root, base / "out", _stub_node(base))
            self.assertIn("只允许已编译的 .js", str(raised.exception))

    def test_a_linked_directory_inside_dist_is_rejected_not_skipped(self) -> None:
        """dist 里的链接目录必须被**拒绝**，而不是被静默跳过。

        回归点：逐层枚举换成 `Path.rglob` 的话，CPython ≤3.12 不会递归进符号链接目录，
        于是一个指向仓库外的 junction 会从结果里**消失**——打包照样成功，而"包内不允许
        链接"那条判定从来没跑过。
        """
        with tempfile.TemporaryDirectory() as temporary_directory:
            base = Path(temporary_directory)
            root = base / "repo"
            mcp_root = _make_minimal_repository(root)
            outside = base / "outside"
            outside.mkdir()
            (outside / "leak.js").write_text("SENTINEL-LEAK", encoding="utf-8")
            if not _make_directory_link(mcp_root / "dist/vendor", outside):
                self.skipTest("当前平台不允许创建目录联接或符号链接")

            with self.assertRaises(PACK.PackageError) as raised:
                PACK.package_runtime(root, base / "out", _stub_node(base))
            self.assertIn("符号链接", str(raised.exception))
            self.assertNotIn("SENTINEL-LEAK", str(raised.exception))

    def test_local_configuration_files_are_never_shipped(self) -> None:
        with tempfile.TemporaryDirectory() as temporary_directory:
            base = Path(temporary_directory)
            root = base / "repo"
            mcp_root = _make_minimal_repository(root)
            hostile = {
                "node_modules/zod/.env": "HAVEN_SENTINEL=1\n",
                "node_modules/zod/server.pem": "-----BEGIN " + "PRIVATE KEY-----\nSENTINEL\n",
                "node_modules/.npmrc": "//registry.example/:_authToken=SENTINEL\n",
            }
            for relative, content in hostile.items():
                target = mcp_root / relative
                target.parent.mkdir(parents=True, exist_ok=True)
                target.write_text(content, encoding="utf-8")

            for relative in hostile:
                with self.subTest(path=relative):
                    with self.assertRaises(PACK.PackageError) as raised:
                        PACK.package_runtime(root, base / "out", _stub_node(base))
                    self.assertNotIn("SENTINEL", str(raised.exception))

    def test_output_inside_the_mcp_package_is_rejected(self) -> None:
        with tempfile.TemporaryDirectory() as temporary_directory:
            base = Path(temporary_directory)
            root = base / "repo"
            mcp_root = _make_minimal_repository(root)

            with self.assertRaises(PACK.PackageError):
                PACK.package_runtime(root, mcp_root / "runtime-out", _stub_node(base))

    def test_a_symlinked_node_binary_is_rejected(self) -> None:
        with tempfile.TemporaryDirectory() as temporary_directory:
            base = Path(temporary_directory)
            root = base / "repo"
            _make_minimal_repository(root)
            real = _stub_node(base)
            link = base / "node-link.exe"
            try:
                link.symlink_to(real)
            except (OSError, NotImplementedError):
                self.skipTest("当前平台不允许创建符号链接")

            with self.assertRaises(PACK.PackageError) as raised:
                PACK.package_runtime(root, base / "out", link)
            self.assertIn("符号链接", str(raised.exception))

    def test_a_missing_node_binary_is_rejected(self) -> None:
        with tempfile.TemporaryDirectory() as temporary_directory:
            base = Path(temporary_directory)
            root = base / "repo"
            _make_minimal_repository(root)

            with self.assertRaises(PACK.PackageError):
                PACK.package_runtime(root, base / "out", base / "nope.exe")

    def test_a_node_version_that_is_not_a_plain_version_is_rejected(self) -> None:
        with tempfile.TemporaryDirectory() as temporary_directory:
            base = Path(temporary_directory)
            root = base / "repo"
            _make_minimal_repository(root)

            for hostile in ("22/../../evil", "22 23", ""):
                with self.subTest(version=hostile):
                    with self.assertRaises(PACK.PackageError):
                        PACK.package_runtime(
                            root, base / "out", _stub_node(base), node_version_override=hostile
                        )

    def test_the_cli_reports_success_and_rejects_with_a_nonzero_exit(self) -> None:
        with tempfile.TemporaryDirectory() as temporary_directory:
            base = Path(temporary_directory)
            root = base / "repo"
            _make_minimal_repository(root)
            node = _stub_node(base)
            out = base / "out"

            self.assertEqual(
                _run_main(
                    [
                        "--root",
                        str(root),
                        "--out",
                        str(out),
                        "--node",
                        str(node),
                        "--quiet",
                    ]
                ),
                0,
            )
            self.assertTrue((out / PACK.BUNDLE_DIR_NAME / PACK.LAUNCHER_NAME).is_file())

            # 版本错配时 CLI 必须返回 1，且不留下产物。
            _write_json(root / "mcp/haven-mcp/package.json", {"version": "9.9.9"})
            rejected = base / "rejected"
            self.assertEqual(
                _run_main(
                    [
                        "--root",
                        str(root),
                        "--out",
                        str(rejected),
                        "--node",
                        str(node),
                        "--quiet",
                    ]
                ),
                1,
            )
            self.assertFalse((rejected / PACK.BUNDLE_DIR_NAME).exists())

    def test_the_installer_maps_the_bundle_under_the_same_directory_name(self) -> None:
        """产物的目录名、Tauri 的资源映射与 Rust 侧的路径常量必须是同一个字符串。

        三处里任何一处漂移，症状都是"安装后自动配置写出的命令指向一个不存在的路径"——
        而 CI 全绿、清单看起来完全正常、只有最终用户的客户端起不来。因此这里直接从三份
        源码里**现读**（不另抄一份）并逐字比对。
        """
        config = json.loads(
            (REPOSITORY_ROOT / "src-tauri/tauri.conf.json").read_text(encoding="utf-8")
        )
        resources = config["bundle"]["resources"]
        self.assertEqual(
            resources.get(f"resources/{PACK.BUNDLE_DIR_NAME}"),
            PACK.BUNDLE_DIR_NAME,
            "tauri.conf.json 的 bundle.resources 必须把产物目录映射到安装后的同名目录",
        )

        # Rust 侧算出来的运行时目录用它拼客户端命令，因此必须同名。
        command_source = (
            REPOSITORY_ROOT / "src-tauri/src/commands/mcp_client.rs"
        ).read_text(encoding="utf-8")
        self.assertIn(
            f'pub const RUNTIME_BUNDLE_DIR: &str = "{PACK.BUNDLE_DIR_NAME}";',
            command_source,
            "src-tauri/src/commands/mcp_client.rs 的 RUNTIME_BUNDLE_DIR 与产物目录名不一致",
        )

    def test_the_real_mcp_manifest_matches_what_the_packager_expects(self) -> None:
        """对着**真实**的 `mcp/haven-mcp` 断言入口与依赖名，而不是只在夹具上自证。"""
        manifest = json.loads(
            (REPOSITORY_ROOT / PACK.MCP_MANIFEST).read_text(encoding="utf-8")
        )
        self.assertEqual(manifest["main"], PACK.ENTRY_RELATIVE)
        self.assertEqual(manifest["bin"]["haven-mcp-server"], PACK.ENTRY_RELATIVE)
        dependencies = manifest["dependencies"]
        for required in PACK.REQUIRED_NODE_MODULES:
            self.assertIn(required, dependencies, f"{required} 必须是生产依赖")


if __name__ == "__main__":
    raise SystemExit(unittest.main())
