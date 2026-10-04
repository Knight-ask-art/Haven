#!/usr/bin/env python3
"""`tools/skills/pack-skill.py` 的用例（外部 Agent 分发路径）。

这些用例钉住八件事：

1. 产物里的技能正文与仓库里的那份**逐字节相同**（外部 Agent 拿到的不是另一个版本）；
2. manifest 里的 MCP 工具清单等于**冻结的 9 项**（从 `constants.ts` 现读，不是另抄一份）；
3. 打包是**确定性**的：同一份源码重复打包得到逐字节相同的 zip；
4. 前提不成立时**拒绝打包**（契约不过 / 版本错配 / 输出目录落在技能包内部）；
5. `--skill` 必须解析成**仓库内**的目录：绝对路径、`..`、以及穿过符号链接/目录联接的
   路径一律拒绝，且拒绝时不产出任何文件；
6. 版本号必须**文件名安全**：含路径分隔符的版本号会把产物写到 `--out` 之外；
7. 技能包内容走**显式允许清单**：技能目录里的 `.env`、编辑器临时文件、构建产物、
   任意草稿都会让打包失败，且错误里不得出现文件内容；
8. zip 条目名按**字符串**校验，因此"宿主文件系统恰好创建不出这种名字"不是安全依据
   （反斜杠与盘符在 POSIX 上是合法文件名字符，在 Windows 解压时却是分隔符）。

运行：python tools/skills/pack-skill.test.py
"""

from __future__ import annotations

import contextlib
import hashlib
import importlib.util
import io
import json
import re
import shutil
import subprocess
import tempfile
import unittest
import zipfile
import os
from pathlib import Path
from typing import Any


REPOSITORY_ROOT = Path(__file__).parents[2]

# 运行时把内置技能**编译期**嵌进二进制的那张注册表。产物里的 SKILL.md 必须与它嵌进去的
# 是同一个文件——否则"外部 Agent 拿到的技能"与"栖阅自己用的技能"就是两份东西。
BUILTIN_SKILL_REGISTRY = (
    REPOSITORY_ROOT / "后端/crates/haven-infrastructure/src/agent_skill.rs"
)


def _load(name: str, filename: str) -> Any:
    path = Path(__file__).with_name(filename)
    spec = importlib.util.spec_from_file_location(name, path)
    if spec is None or spec.loader is None:
        raise RuntimeError(f"无法加载 {path}")
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


PACK = _load("pack_skill", "pack-skill.py")
CONTRACT = _load("skill_contract_check_for_pack_test", "skill-contract-check.py")

SKILL_ID = "haven-agent-proposal"
SKILL_DIR = REPOSITORY_ROOT / "skills" / SKILL_ID
PRODUCT_VERSION = json.loads(
    (REPOSITORY_ROOT / "前端/app/package.json").read_text(encoding="utf-8")
)["version"]


def _make_minimal_repository(root: Path, version: str = PRODUCT_VERSION) -> Path:
    """一个只够打包器跑完前提检查的最小仓库树。

    刻意复用真实仓库的两份权威文件（`constants.ts` / `bridge.ts`）：工具清单的
    "9 项"约束来自契约校验器，而不是本文件里再写一遍的数字。
    """
    (root / "mcp/haven-mcp/src").mkdir(parents=True)
    shutil.copy(REPOSITORY_ROOT / "mcp/haven-mcp/src/constants.ts", root / "mcp/haven-mcp/src")
    shutil.copy(REPOSITORY_ROOT / "mcp/haven-mcp/src/bridge.ts", root / "mcp/haven-mcp/src")
    (root / "前端/app").mkdir(parents=True)
    (root / "前端/app/package.json").write_text(
        json.dumps({"version": version}), encoding="utf-8"
    )
    (root / "mcp/haven-mcp/package.json").write_text(
        json.dumps({"name": "haven-mcp-server", "version": version}), encoding="utf-8"
    )
    skill = root / "skills" / SKILL_ID
    skill.mkdir(parents=True)
    (skill / "SKILL.md").write_text(
        "---\nname: haven-agent-proposal\ndescription: 最小技能\n---\n\n# 标题\n\n正文。\n",
        encoding="utf-8",
    )
    # 契约校验要求 evals 存在且至少 3 条：最小仓库也必须满足，否则每个用例
    # 都会停在"契约不过"上，反而测不到各自想测的那条拒绝理由。
    (skill / "evals").mkdir()
    (skill / "evals/evals.json").write_text(
        json.dumps(
            {
                "skill_name": SKILL_ID,
                "evals": [
                    {
                        "id": index,
                        "name": f"用例 {index}",
                        "prompt": "提示词",
                        "expected_output": "期望输出",
                        "expectations": ["一条可判定的期望"],
                    }
                    for index in (1, 2, 3)
                ],
            },
            ensure_ascii=False,
        ),
        encoding="utf-8",
    )
    return skill


def _make_directory_link(link: Path, target: Path) -> bool:
    """尽力创建一个目录联接（junction）/ 目录符号链接；平台不允许时返回 False。

    Windows 上优先用 `mklink /J`：目录联接是普通用户就能创建的**重解析点**，
    而 `os.symlink` 需要开发者模式或提权——"没有符号链接"不等于"没有链接"。
    """
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


def _run_main(arguments: list[str]) -> int:
    """安静地跑 `main()`：拒绝路径会把中文原因写到 stderr，测试输出里不用再重复一遍。"""
    with contextlib.redirect_stderr(io.StringIO()):
        return PACK.main(arguments)


class PackSkillTests(unittest.TestCase):
    @unittest.skipUnless(os.name == "nt", "Windows 8.3 path compatibility")
    def test_short_root_and_canonical_skill_paths_preserve_boundary_checks(self) -> None:
        import ctypes

        with tempfile.TemporaryDirectory(prefix="haven_skill_long_directory_") as temporary_directory:
            base = Path(temporary_directory)
            root = base / "repository_with_a_long_name"
            skill = _make_minimal_repository(root)
            buffer = ctypes.create_unicode_buffer(32768)
            get_short_path = ctypes.windll.kernel32.GetShortPathNameW
            get_short_path.argtypes = [ctypes.c_wchar_p, ctypes.c_wchar_p, ctypes.c_uint32]
            get_short_path.restype = ctypes.c_uint32
            length = get_short_path(str(root), buffer, len(buffer))
            if not length or length >= len(buffer) or Path(buffer.value) == root.resolve():
                self.skipTest("8.3 aliases are not enabled for the test directory")
            short_root = Path(buffer.value)
            for candidate in (short_root / "skills" / SKILL_ID, skill.resolve()):
                with self.subTest(candidate=candidate):
                    manifest = PACK.pack(short_root, candidate, base / "out")
                    self.assertEqual(manifest["skill"]["sourcePath"], f"skills/{SKILL_ID}")
            alias = short_root / "skills" / "linked"
            if not _make_directory_link(alias, skill.resolve()):
                self.skipTest("directory junction creation is unavailable")
            with self.assertRaises(PACK.PackError):
                PACK.pack(short_root, alias, base / "rejected")
            self.assertFalse((base / "rejected").exists())

    def test_cli_default_output_reports_success_without_dangling_constants(self) -> None:
        with tempfile.TemporaryDirectory() as temporary_directory:
            base = Path(temporary_directory)
            root = base / "repo"
            _make_minimal_repository(root)
            out = base / "out"
            output = io.StringIO()

            with contextlib.redirect_stdout(output):
                result = PACK.main(
                    ["--root", str(root), "--skill", f"skills/{SKILL_ID}", "--out", str(out)]
                )

            self.assertEqual(result, 0)
            self.assertIn(
                "MCP 工具：9 项（来自 mcp/haven-mcp/src/constants.ts）",
                output.getvalue(),
            )
            self.assertIn("pack-skill: PASS", output.getvalue())

    def test_pack_writes_a_versioned_archive_and_a_self_describing_manifest(self) -> None:
        with tempfile.TemporaryDirectory() as temporary_directory:
            out = Path(temporary_directory)
            manifest = PACK.pack(REPOSITORY_ROOT, SKILL_DIR, out)

            archive = out / f"{SKILL_ID}-{PRODUCT_VERSION}.zip"
            manifest_path = out / f"{SKILL_ID}-{PRODUCT_VERSION}.manifest.json"
            self.assertTrue(archive.is_file())
            self.assertTrue(manifest_path.is_file())
            self.assertEqual(json.loads(manifest_path.read_text(encoding="utf-8")), manifest)

            self.assertEqual(manifest["skill"]["id"], SKILL_ID)
            self.assertEqual(manifest["skill"]["version"], PRODUCT_VERSION)
            self.assertEqual(manifest["skill"]["sourcePath"], f"skills/{SKILL_ID}")
            self.assertEqual(manifest["archive"]["name"], archive.name)
            self.assertEqual(manifest["mcp"]["serverPackage"], "haven-mcp-server")
            self.assertEqual(manifest["mcp"]["serverVersion"], PRODUCT_VERSION)
            # 安装动作属于用户：栖阅不写任何全局 Agent 配置。
            self.assertEqual(manifest["install"]["owner"], "user")
            self.assertIn("不会写你的全局 Agent 配置", manifest["install"]["note"])

    def test_archive_bytes_are_byte_identical_to_the_source_tree(self) -> None:
        """这是"外部 Agent 拿到的那份"与"仓库里那份"同源的可执行证据。"""
        with tempfile.TemporaryDirectory() as temporary_directory:
            out = Path(temporary_directory)
            manifest = PACK.pack(REPOSITORY_ROOT, SKILL_DIR, out)

            with zipfile.ZipFile(out / manifest["archive"]["name"]) as archive:
                names = sorted(archive.namelist())
                expected = sorted(
                    f"{SKILL_ID}/{path.relative_to(SKILL_DIR).as_posix()}"
                    for path in SKILL_DIR.rglob("*")
                    if path.is_file()
                )
                self.assertEqual(names, expected)
                for name in names:
                    relative = name[len(f"{SKILL_ID}/") :]
                    self.assertEqual(
                        archive.read(name),
                        (SKILL_DIR / relative).read_bytes(),
                        f"{name} 与源码树不一致",
                    )

            # manifest 的逐文件摘要必须与刚刚比对过的字节一致。
            for entry in manifest["skill"]["files"]:
                data = (SKILL_DIR / entry["path"]).read_bytes()
                self.assertEqual(entry["bytes"], len(data))
                self.assertEqual(entry["sha256"], hashlib.sha256(data).hexdigest())

    def test_manifest_carries_exactly_the_frozen_mcp_tool_contract(self) -> None:
        with tempfile.TemporaryDirectory() as temporary_directory:
            manifest = PACK.pack(REPOSITORY_ROOT, SKILL_DIR, Path(temporary_directory))

            tools, error = CONTRACT.frozen_tool_names(REPOSITORY_ROOT)
            self.assertIsNone(error)
            self.assertEqual(manifest["mcp"]["toolNames"], tools)
            self.assertEqual(len(tools), 9)
            # 清单里不能出现任何写/批准类工具：技能面向的工具集与冻结集合是同一个。
            for name in manifest["mcp"]["toolNames"]:
                for forbidden in ("apply", "approve", "write", "delete", "secret"):
                    self.assertNotIn(forbidden, name)

    def test_packing_twice_produces_byte_identical_artifacts(self) -> None:
        with tempfile.TemporaryDirectory() as first_directory:
            with tempfile.TemporaryDirectory() as second_directory:
                first = PACK.pack(REPOSITORY_ROOT, SKILL_DIR, Path(first_directory))
                second = PACK.pack(REPOSITORY_ROOT, SKILL_DIR, Path(second_directory))

                archive_name = first["archive"]["name"]
                self.assertEqual(
                    (Path(first_directory) / archive_name).read_bytes(),
                    (Path(second_directory) / archive_name).read_bytes(),
                )
                self.assertEqual(
                    (Path(first_directory) / f"{SKILL_ID}-{PRODUCT_VERSION}.manifest.json").read_bytes(),
                    (Path(second_directory) / f"{SKILL_ID}-{PRODUCT_VERSION}.manifest.json").read_bytes(),
                )
                self.assertEqual(first["skill"]["contentSha256"], second["skill"]["contentSha256"])

    def test_content_digest_changes_when_the_document_changes(self) -> None:
        """摘要必须对内容敏感，否则"同一个 version"背后可以有无数个包。"""
        with tempfile.TemporaryDirectory() as temporary_directory:
            root = Path(temporary_directory)
            skill = _make_minimal_repository(root)
            out = root / "out"

            first = PACK.pack(root, skill, out)
            (skill / "SKILL.md").write_text(
                "---\nname: haven-agent-proposal\ndescription: 最小技能\n---\n\n# 标题\n\n正文改过了。\n",
                encoding="utf-8",
            )
            second = PACK.pack(root, skill, out)

            self.assertNotEqual(first["skill"]["contentSha256"], second["skill"]["contentSha256"])

    def test_the_packed_document_is_the_one_the_runtime_embeds(self) -> None:
        """产物里的 SKILL.md == 运行时 `include_str!` 嵌进去的那个文件。

        这是"两条消费路径用同一份受信任内置内容"的可执行证据链的最后一环：

        - 打包器：zip 条目 == 源码树字节（`test_archive_bytes_are_byte_identical_to_the_source_tree`）；
        - 运行时：`include_str!` 嵌进去的 == 源码树字节（Rust 侧 `cargo test` 已有断言）；
        - 本用例把两端接起来：产物字节 == `include_str!` 指向的那个文件的字节。

        少了这一环，"两条路径同源"就只是文档里的一句话。
        """
        source = BUILTIN_SKILL_REGISTRY.read_text(encoding="utf-8")
        embedded = re.findall(r'include_str!\(\s*"([^"]+)"\s*\)', source)
        self.assertTrue(embedded, f"{BUILTIN_SKILL_REGISTRY}: 没有找到 include_str!")

        # `include_str!` 的相对路径是相对**包含它的那个源文件**解析的。
        resolved = sorted(
            (BUILTIN_SKILL_REGISTRY.parent / relative).resolve() for relative in embedded
        )
        # 同一个文件被多次嵌入是允许的；但每个嵌入路径都必须落在这份技能上。
        self.assertEqual(
            set(resolved),
            {(SKILL_DIR / "SKILL.md").resolve()},
            "运行时嵌入的必须是仓库里这份 SKILL.md",
        )

        with tempfile.TemporaryDirectory() as temporary_directory:
            out = Path(temporary_directory)
            manifest = PACK.pack(REPOSITORY_ROOT, SKILL_DIR, out)

            with zipfile.ZipFile(out / manifest["archive"]["name"]) as archive:
                packaged = archive.read(f"{SKILL_ID}/SKILL.md")

            runtime_bytes = resolved[0].read_bytes()
            self.assertEqual(packaged, runtime_bytes, "产物正文与运行时嵌入的字节不一致")
            # manifest 的逐文件摘要同样必须指向这一份字节。
            entry = next(
                item for item in manifest["skill"]["files"] if item["path"] == "SKILL.md"
            )
            self.assertEqual(entry["sha256"], hashlib.sha256(runtime_bytes).hexdigest())

    def test_python_and_rust_agree_on_the_runtime_document_limits(self) -> None:
        """字符上限是**跨语言常量**：Python 侧放宽一点，运行时就拒绝加载。

        因此这些数字从 Rust 源码现读后与 Python 侧比对，而不是各写一份——
        两份常量迟早会分叉，而分叉的后果是"打包通过、用户装上去打不开应用"。
        """
        domain_source = (
            REPOSITORY_ROOT / "后端/crates/haven-domain/src/agent_skill.rs"
        ).read_text(encoding="utf-8")

        def rust_usize(name: str) -> int:
            match = re.search(rf"const {name}: usize = ([0-9_]+);", domain_source)
            if match is None:
                raise AssertionError(f"haven-domain: 找不到 {name}")
            return int(match.group(1).replace("_", ""))

        self.assertEqual(
            rust_usize("AGENT_SKILL_INSTRUCTIONS_MAX_CHARS"),
            CONTRACT.MAX_INSTRUCTIONS_CHARS,
            "正文上限：Python 契约校验器与运行时必须同值",
        )
        self.assertEqual(
            rust_usize("AGENT_SKILL_DESCRIPTION_MAX_CHARS"),
            CONTRACT.MAX_DESCRIPTION_CHARS,
        )
        self.assertEqual(rust_usize("AGENT_SKILL_ID_MAX_CHARS"), CONTRACT.MAX_NAME_CHARS)

        # 摘要绑定域参与 preimage：两侧不同就等于"启用记录永远 stale"。
        domain = re.search(
            r'const AGENT_SKILL_HASH_DOMAIN: &str = "([^"]+)";', domain_source
        )
        self.assertIsNotNone(domain, "haven-domain: 找不到 AGENT_SKILL_HASH_DOMAIN")
        self.assertEqual(domain.group(1), "haven:agent-skill:v1")

    def test_the_runtime_content_digest_is_computed_over_the_native_projection(self) -> None:
        """启用记录绑定的是**原生投影**的摘要，不是整份文件的摘要。

        这里只做 Python 侧的复刻一致性：投影 → 域前缀 + `\\n` + 投影字节 → SHA-256。
        Rust 侧的对应结论由 `cargo test`（`haven-domain` 的
        `the_content_hash_binds_to_the_native_projection_not_the_raw_file`）钉住；
        本用例保证两侧的**算法形状**一致，否则同一份内容会算出两个摘要、启用记录永远 stale。
        """
        projection, error = CONTRACT.native_projection(SKILL_DIR)
        self.assertIsNone(error, f"原生投影必须可计算：{error}")
        self.assertIsNotNone(projection)
        raw = (SKILL_DIR / "SKILL.md").read_bytes().decode("utf-8")

        def digest(text: str) -> str:
            hasher = hashlib.sha256()
            hasher.update(b"haven:agent-skill:v1")
            hasher.update(b"\n")
            hasher.update(text.encode("utf-8"))
            return hasher.hexdigest()

        native_hash = digest(projection)
        self.assertRegex(native_hash, r"^[0-9a-f]{64}$")
        self.assertNotEqual(
            native_hash, digest(raw), "投影与原文必须是两个摘要，否则内容绑定形同虚设"
        )
        self.assertLess(
            len(projection), len(raw), "原生投影应当真的比原文短（外部段落被摘掉）"
        )
        # 外部段落里的 MCP 工作流不得进入原生投影。
        self.assertNotIn("get_system_capabilities", projection)

    def test_a_document_the_runtime_would_refuse_to_load_is_not_packed(self) -> None:
        """正文控制字符 / 超长：运行时会拒绝加载，打包器必须先拒绝产出。

        "打包器放行、运行时拒绝"是最难查的一类失败：用户拿到一个装上去就打不开应用的包。
        """
        cases = {
            "control-char-in-body": (
                "---\nname: haven-agent-proposal\ndescription: 最小技能\n---\n\n正文\x07。\n"
            ),
            "oversized-body": (
                "---\nname: haven-agent-proposal\ndescription: 最小技能\n---\n\n"
                + "字" * (CONTRACT.MAX_INSTRUCTIONS_CHARS + 1)
                + "\n"
            ),
        }

        for label, document in cases.items():
            with self.subTest(case=label), tempfile.TemporaryDirectory() as temporary_directory:
                root = Path(temporary_directory)
                skill = _make_minimal_repository(root)
                # 走字节写入：`write_text` 在 Windows 上会做换行转换，测不到原始内容。
                (skill / "SKILL.md").write_bytes(document.encode("utf-8"))
                out = root / "out"

                with self.assertRaises(PACK.PackError) as raised:
                    PACK.pack(root, skill, out)

                self.assertIn("技能契约校验未通过", str(raised.exception))
                self.assertFalse(out.exists(), "拒绝路径不得留下任何产物")

    def test_a_skill_that_fails_the_contract_is_not_packed(self) -> None:
        with tempfile.TemporaryDirectory() as temporary_directory:
            root = Path(temporary_directory)
            skill = _make_minimal_repository(root)
            # 缺 description：契约校验器会拒绝，因此不应该产出任何产物。
            (skill / "SKILL.md").write_text(
                "---\nname: haven-agent-proposal\n---\n\n# 标题\n", encoding="utf-8"
            )
            out = root / "out"

            with self.assertRaises(PACK.PackError) as raised:
                PACK.pack(root, skill, out)

            self.assertIn("技能契约校验未通过", str(raised.exception))
            self.assertFalse(out.exists(), "拒绝路径不得留下任何产物")

    def test_a_version_mismatch_between_product_and_mcp_server_is_rejected(self) -> None:
        with tempfile.TemporaryDirectory() as temporary_directory:
            root = Path(temporary_directory)
            skill = _make_minimal_repository(root)
            (root / "mcp/haven-mcp/package.json").write_text(
                json.dumps({"name": "haven-mcp-server", "version": "9.9.9"}), encoding="utf-8"
            )

            with self.assertRaises(PACK.PackError) as raised:
                PACK.pack(root, skill, root / "out")

            self.assertIn("版本不一致", str(raised.exception))

    def test_refuses_to_write_the_artifact_inside_the_skill_package(self) -> None:
        """产物写进技能包，下一轮打包就会把上一轮的产物也打进去。"""
        with self.assertRaises(PACK.PackError):
            PACK.pack(REPOSITORY_ROOT, SKILL_DIR, SKILL_DIR / "dist")

    def test_refuses_to_follow_symlinks_out_of_the_package(self) -> None:
        with tempfile.TemporaryDirectory() as temporary_directory:
            root = Path(temporary_directory)
            skill = _make_minimal_repository(root)
            outside = root / "outside.txt"
            outside.write_text("包外的文件", encoding="utf-8")
            link = skill / "link.md"
            try:
                link.symlink_to(outside)
            except (OSError, NotImplementedError):
                self.skipTest("当前平台不允许创建符号链接")

            class ContractProbe:
                called = False

                def check_skill(self, _root: Path, _skill: Path) -> list[str]:
                    self.called = True
                    return []

            contract = ContractProbe()
            with self.assertRaises(PACK.PackError):
                PACK.pack(root, skill, root / "out", contract=contract)
            self.assertFalse(contract.called, "符号链接必须在契约读取任何文件前拒绝")


    def test_the_package_keeps_both_audiences_and_is_not_the_native_projection(self) -> None:
        """打包只做字节搬运：外部客户端必须拿到**整份**文档，包括外部段落。

        `SKILL.md` 用受众标记划出了两条运行时各自的段落，栖阅自己只消费原生投影。
        哪天有人"顺手"把投影写进产物，外部 Agent 就会安静地丢掉整段 MCP 工作流——
        因此在产物字节上直接钉住。
        """
        with tempfile.TemporaryDirectory() as temporary_directory:
            out = Path(temporary_directory)
            manifest = PACK.pack(REPOSITORY_ROOT, SKILL_DIR, out)

            with zipfile.ZipFile(out / manifest["archive"]["name"]) as archive:
                document = archive.read(f"{SKILL_ID}/SKILL.md").decode("utf-8")

            self.assertIn("<!-- haven:audience=native -->", document)
            self.assertIn("<!-- haven:audience=external -->", document)
            self.assertIn("<!-- /haven:audience -->", document)
            # 外部段落里的工具名必须还在：原生投影会把整段丢掉。
            self.assertIn("get_system_capabilities", document)
            self.assertEqual(document.encode("utf-8"), (SKILL_DIR / "SKILL.md").read_bytes())

    # ---- `--skill` 选择器：必须是仓库内的相对路径 ----

    def test_the_skill_selector_must_be_a_repository_local_relative_path(self) -> None:
        """绝对路径 / 盘符 / UNC / `..`：`pathlib` 的 `/` 会把它们变成仓库外的目录。

        `Path("repo") / "/etc"` 就是 `/etc`（右边是绝对路径时左边被丢掉），`..` 也不会被
        拦住。少了这条校验，`--skill /anywhere` 会把机器上的任意目录当成技能包发出去。
        """
        selectors = [
            "/etc",
            "C:/Windows",
            "C:Windows",
            "\\\\server\\share\\skill",
            "..",
            "../outside",
            "skills/../../outside",
            "skills/./../../outside",
            "~/skill",
            "",
            "   ",
        ]
        for selector in selectors:
            with self.subTest(selector=selector):
                with self.assertRaises(PACK.PackError):
                    PACK.resolve_skill_dir(REPOSITORY_ROOT, selector)

    def test_a_valid_skill_outside_the_repository_is_rejected_without_artifacts(self) -> None:
        """仓库外放一份**完全合法**的技能：拒绝必须来自位置校验，而不是"契约没过"。

        放一份契约能过的技能是刻意的：否则"退出码 1"既可能来自位置校验，也可能来自契约
        校验，用例就变成了自我印证。
        """
        with tempfile.TemporaryDirectory() as temporary_directory:
            base = Path(temporary_directory)
            root = base / "repo"
            _make_minimal_repository(root)
            outside_skill = base / "outside" / "skills" / SKILL_ID
            shutil.copytree(root / "skills" / SKILL_ID, outside_skill)
            out = base / "out"

            for selector in (f"../outside/skills/{SKILL_ID}", str(outside_skill)):
                with self.subTest(selector=selector):
                    self.assertEqual(
                        _run_main(
                            ["--root", str(root), "--skill", selector, "--out", str(out), "--quiet"]
                        ),
                        1,
                        f"{selector}: 必须拒绝",
                    )
                    self.assertFalse(out.exists(), "拒绝路径不得留下任何产物")
            self.assertEqual(sorted(path.name for path in base.iterdir()), ["outside", "repo"])

    def test_a_directory_link_that_leaves_the_repository_is_rejected(self) -> None:
        """目录联接（junction）是普通用户就能建的重解析点，`is_symlink()` 看不见它。

        路径形状完全在仓库内，指向的却是仓库外的一份合法技能。两条入口都要挡住：
        `resolve_skill_dir`（`--skill` 的入口）与 `pack()`（公开入口，调用方可能自己拼路径）。
        """
        with tempfile.TemporaryDirectory() as temporary_directory:
            base = Path(temporary_directory)
            root = base / "repo"
            skill = _make_minimal_repository(root)
            outside = base / "outside"
            shutil.copytree(skill, outside)
            link = root / "skills" / "linked"
            if not _make_directory_link(link, outside):
                self.skipTest("当前平台不允许创建目录联接或符号链接")

            with self.assertRaises(PACK.PackError):
                PACK.resolve_skill_dir(root, "skills/linked")

            out = base / "out"
            with self.assertRaises(PACK.PackError):
                PACK.pack(root, link, out)
            self.assertEqual(
                _run_main(["--root", str(root), "--skill", "skills/linked", "--out", str(out)]),
                1,
            )
            self.assertFalse(out.exists(), "拒绝路径不得留下任何产物")

    def test_a_linked_skill_directory_inside_the_repository_is_rejected(self) -> None:
        """指向**仓库内**的链接同样拒绝：技能包的来源必须可审计（fail closed）。

        这条只有逐段检查能挡住——`resolve()` 之后它仍在仓库里，位置校验会放行。
        """
        with tempfile.TemporaryDirectory() as temporary_directory:
            root = Path(temporary_directory) / "repo"
            skill = _make_minimal_repository(root)
            link = root / "skills" / "alias"
            if not _make_directory_link(link, skill):
                self.skipTest("当前平台不允许创建目录联接或符号链接")

            with self.assertRaises(PACK.PackError):
                PACK.resolve_skill_dir(root, "skills/alias")

            out = root / "out"
            with self.assertRaises(PACK.PackError):
                PACK.pack(root, link, out)
            self.assertEqual(
                _run_main(["--root", str(root), "--skill", "skills/alias", "--out", str(out)]), 1
            )
            self.assertFalse(out.exists(), "拒绝路径不得留下任何产物")

    def test_a_linked_directory_inside_the_skill_is_rejected_not_skipped(self) -> None:
        """技能目录**内部**的链接目录必须被**拒绝**，而不是被悄悄跳过。

        回归点：`collect_files` 曾经用 `Path.rglob`。CPython ≤3.12 的 `rglob` 不递归进
        符号链接目录，于是"技能目录里有一个指向仓库外的 junction"在结果里**完全消失**：
        打包照样成功，而那条"包内不允许链接"的判定从来没跑过。这里直接断言 `collect_files`
        抛错，而不是断言产物里没有它——后者在"被跳过"时也会通过。
        """
        with tempfile.TemporaryDirectory() as temporary_directory:
            base = Path(temporary_directory)
            root = base / "repo"
            skill = _make_minimal_repository(root)
            outside = base / "outside"
            outside.mkdir()
            (outside / "secret.md").write_text("SENTINEL-DO-NOT-SHIP", encoding="utf-8")
            if not _make_directory_link(skill / "assets", outside):
                self.skipTest("当前平台不允许创建目录联接或符号链接")

            with self.assertRaises(PACK.PackError) as raised:
                PACK.collect_files(skill)
            self.assertIn("符号链接", str(raised.exception))
            # 被拒绝的目录内容一个字节都不得出现在错误消息里（消息只带路径）。
            self.assertNotIn("SENTINEL-DO-NOT-SHIP", str(raised.exception))

            # 走完整入口也必须拒绝，且不留产物。
            out = root / "out"
            with self.assertRaises(PACK.PackError):
                PACK.pack(root, skill, out)
            self.assertFalse(out.exists(), "拒绝路径不得留下任何产物")

    def test_a_relative_selector_inside_the_repository_still_resolves(self) -> None:
        """反向证据：放过合法路径的校验才有意义——不然上面的拒绝可以靠"一律拒绝"通过。"""
        self.assertEqual(
            PACK.resolve_skill_dir(REPOSITORY_ROOT, f"skills/{SKILL_ID}"),
            SKILL_DIR.resolve(),
        )
        # Windows 上反斜杠分隔符同样是"仓库内的相对路径"。
        self.assertEqual(
            PACK.resolve_skill_dir(REPOSITORY_ROOT, f"skills\\{SKILL_ID}"),
            SKILL_DIR.resolve(),
        )

    def test_existing_output_symlinks_are_replaced_without_touching_their_targets(self) -> None:
        with tempfile.TemporaryDirectory() as temporary_directory:
            base = Path(temporary_directory)
            root = base / "repo"
            skill = _make_minimal_repository(root)
            out = base / "out"
            out.mkdir()
            archive_victim = base / "archive-victim.bin"
            manifest_victim = base / "manifest-victim.bin"
            archive_victim.write_bytes(b"archive target must survive")
            manifest_victim.write_bytes(b"manifest target must survive")
            archive_link = out / f"{SKILL_ID}-{PRODUCT_VERSION}.zip"
            manifest_link = out / f"{SKILL_ID}-{PRODUCT_VERSION}.manifest.json"
            try:
                archive_link.symlink_to(archive_victim)
                manifest_link.symlink_to(manifest_victim)
            except (OSError, NotImplementedError):
                self.skipTest("当前平台不允许创建文件符号链接")

            manifest = PACK.pack(root, skill, out)

            self.assertEqual(archive_victim.read_bytes(), b"archive target must survive")
            self.assertEqual(manifest_victim.read_bytes(), b"manifest target must survive")
            self.assertFalse(archive_link.is_symlink())
            self.assertFalse(manifest_link.is_symlink())
            self.assertEqual(json.loads(manifest_link.read_text(encoding="utf-8")), manifest)
            with zipfile.ZipFile(archive_link) as archive:
                self.assertEqual(
                    archive.read(f"{SKILL_ID}/SKILL.md"),
                    (skill / "SKILL.md").read_bytes(),
                )

    # ---- 版本号：必须文件名安全 ----

    def test_a_version_that_is_not_filename_safe_is_rejected_without_artifacts(self) -> None:
        """产物文件名是 `<skill-id>-<version>.zip`：含分隔符的版本号会写穿 `--out`。

        `1.0.0/../../escaped` 在朴素实现下会落到 `--out` 的**上一级**。用例逐条断言
        拒绝，并且断言临时目录里（`--out` 之外）没有多出任何文件。
        """
        unsafe_versions = [
            "1.0.0/../../escaped",
            "1.0.0\\..\\escaped",
            "../evil",
            "..",
            "1.0.0/x",
            "1.0.0 ",
            "1.0",
            "v1.0.0",
            "字1.0.0",
        ]
        for version in unsafe_versions:
            with self.subTest(version=version), tempfile.TemporaryDirectory() as temporary_directory:
                base = Path(temporary_directory)
                root = base / "repo"
                skill = _make_minimal_repository(root, version=version)
                out = base / "out"

                with self.assertRaises(PACK.PackError) as raised:
                    PACK.pack(root, skill, out)
                self.assertIn("不能用作产物文件名", str(raised.exception))

                self.assertEqual(
                    _run_main(
                        ["--root", str(root), "--skill", f"skills/{SKILL_ID}", "--out", str(out)]
                    ),
                    1,
                )
                self.assertFalse(out.exists(), "拒绝路径不得留下任何产物")
                self.assertFalse((base / "escaped.zip").exists(), "产物不得写到 --out 之外")
                self.assertEqual(
                    sorted(path.name for path in base.iterdir()),
                    ["repo"],
                    "`--out` 之外不得出现任何文件",
                )

    def test_a_filename_safe_version_still_packs_under_its_own_name(self) -> None:
        """反向证据：合法的版本号（含预发布与 build 元数据）必须照常打包。"""
        for version in ("1.2.3", "1.2.3-beta.1", "1.2.3+build.5"):
            with self.subTest(version=version), tempfile.TemporaryDirectory() as temporary_directory:
                base = Path(temporary_directory)
                root = base / "repo"
                skill = _make_minimal_repository(root, version=version)
                out = base / "out"

                manifest = PACK.pack(root, skill, out)

                self.assertEqual(manifest["skill"]["version"], version)
                self.assertEqual(manifest["mcp"]["serverVersion"], version)
                self.assertEqual(manifest["archive"]["name"], f"{SKILL_ID}-{version}.zip")
                self.assertTrue((out / f"{SKILL_ID}-{version}.zip").is_file())
                self.assertTrue((out / f"{SKILL_ID}-{version}.manifest.json").is_file())

    # ---- 包内容：显式允许清单 ----

    def test_a_secret_file_under_the_skill_directory_is_never_packed(self) -> None:
        """技能目录下的 `.env` 不是技能资产，必须让打包失败，而不是被顺手发出去。

        用例同时断言：错误文案里**没有**那个哨兵值——被拒绝的文件很可能正是密钥，
        把它回显到 CI 日志里等于把"拒绝"变成第二次泄漏。
        """
        sentinel = "HAVEN_PACK_TEST_SENTINEL_2f8c1d"
        for extra in (".env", ".env.local", "secrets.json", "notes.txt", "SKILL.md.bak",
                      ".DS_Store", "evals/evals.json.bak", "dist/skill.zip",
                      "references/sub/deep.md", "references/.hidden.md",
                      "references/mcp-tool-map.md.bak", "evals/extra.json"):
            with self.subTest(extra=extra), tempfile.TemporaryDirectory() as temporary_directory:
                base = Path(temporary_directory)
                root = base / "repo"
                skill = _make_minimal_repository(root)
                target = skill / extra
                target.parent.mkdir(parents=True, exist_ok=True)
                target.write_text(f"HAVEN_API_KEY={sentinel}\n", encoding="utf-8")
                out = base / "out"

                with self.assertRaises(PACK.PackError) as raised:
                    PACK.pack(root, skill, out)

                message = str(raised.exception)
                self.assertIn("允许清单", message)
                self.assertNotIn(sentinel, message, "拒绝消息不得回显被拒绝文件的内容")
                self.assertFalse(out.exists(), "拒绝路径不得留下任何产物")

                self.assertEqual(
                    _run_main(
                        ["--root", str(root), "--skill", f"skills/{SKILL_ID}", "--out", str(out)]
                    ),
                    1,
                )
                self.assertFalse(out.exists(), "拒绝路径不得留下任何产物")

    def test_the_allowlist_covers_every_intended_skill_asset(self) -> None:
        """反向证据：允许清单必须真的放过应当分发的三类文件。

        一个"一律拒绝"的清单也能让上面的用例通过，所以这里在**真实**技能包上断言
        条目集合恰好等于允许清单覆盖到的那三项。
        """
        expected = ["SKILL.md", "evals/evals.json", "references/mcp-tool-map.md"]
        with tempfile.TemporaryDirectory() as temporary_directory:
            out = Path(temporary_directory)
            manifest = PACK.pack(REPOSITORY_ROOT, SKILL_DIR, out)
            self.assertEqual([entry["path"] for entry in manifest["skill"]["files"]], expected)
            with zipfile.ZipFile(out / manifest["archive"]["name"]) as archive:
                self.assertEqual(
                    sorted(name[len(f"{SKILL_ID}/") :] for name in archive.namelist()),
                    expected,
                )

        # 新增一份 reference 也是允许的（清单按形状放行，不按文件名写死）。
        with tempfile.TemporaryDirectory() as temporary_directory:
            root = Path(temporary_directory)
            skill = _make_minimal_repository(root)
            references = skill / "references"
            references.mkdir()
            (references / "extra-notes.md").write_text("# 附注\n", encoding="utf-8")

            manifest = PACK.pack(root, skill, root / "out")

            self.assertIn(
                "references/extra-notes.md",
                [entry["path"] for entry in manifest["skill"]["files"]],
            )

    def test_missing_required_assets_reject_the_package(self) -> None:
        """允许清单里必需的资产缺失时，包不是"一份可安装的技能"。"""
        for missing in ("SKILL.md", "evals/evals.json"):
            with self.subTest(missing=missing), tempfile.TemporaryDirectory() as temporary_directory:
                root = Path(temporary_directory)
                skill = _make_minimal_repository(root)
                (skill / missing).unlink()
                out = root / "out"

                with self.assertRaises(PACK.PackError) as raised:
                    PACK.pack(root, skill, out)

                self.assertIn(missing, str(raised.exception))
                self.assertFalse(out.exists(), "拒绝路径不得留下任何产物")

    def test_archive_entry_names_are_validated_as_strings_not_as_host_paths(self) -> None:
        """zip 条目名的判据必须写在字符串上，而不是"宿主能不能创建出这种名字"。

        POSIX 允许文件名里有反斜杠与冒号，Windows 解压时它们却是分隔符 / 盘符——
        同一个条目名在两侧是两种东西，这就是 zip-slip。这些名字在 Windows 上根本创建
        不出来，所以只能直接对判据本身跑一遍：用例因此与平台无关。
        """
        hostile = [
            "",
            "/etc/passwd",
            "\\windows\\system32",
            "references\\..\\..\\evil.md",
            "C:/Windows/System32/evil.md",
            "C:evil.md",
            "../evil.md",
            "references/../../evil.md",
            "references/./evil.md",
            "references//evil.md",
            "references/evil.md/",
            "references/evil\x00.md",
            "references/evil\n.md",
            "references/evil\x7f.md",
        ]
        for relative in hostile:
            with self.subTest(relative=relative):
                with self.assertRaises(PACK.PackError):
                    PACK.validate_archive_entry(relative)

        # 反向证据：合法条目名照常通过，且原样返回（不做任何规范化）。
        for relative in ("SKILL.md", "evals/evals.json", "references/mcp-tool-map.md"):
            self.assertEqual(PACK.validate_archive_entry(relative), relative)

    def test_write_archive_rejects_a_hostile_skill_id_prefix(self) -> None:
        """`pack()` 是公开入口：`skill_id` 不安全时不得把条目名写到包外。"""
        with tempfile.TemporaryDirectory() as temporary_directory:
            target = Path(temporary_directory) / "archive.zip"
            with self.assertRaises(PACK.PackError):
                PACK.write_archive(target, "../escape", [("SKILL.md", b"x")])
            self.assertFalse(target.exists(), "拒绝路径不得留下任何产物")


if __name__ == "__main__":
    unittest.main()
