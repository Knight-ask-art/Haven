from __future__ import annotations

import importlib.util
import tempfile
import unittest
from pathlib import Path


MODULE_PATH = Path(__file__).with_name("version-check.py")
SPEC = importlib.util.spec_from_file_location("version_check", MODULE_PATH)
if SPEC is None or SPEC.loader is None:
    raise RuntimeError(f"无法加载版本检查模块: {MODULE_PATH}")
VERSION_CHECK = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(VERSION_CHECK)

REPOSITORY_ROOT = Path(__file__).parents[2]


class VersionCheckTests(unittest.TestCase):
    def test_release_manifests_use_one_first_party_version(self) -> None:
        errors = VERSION_CHECK.validate(REPOSITORY_ROOT, "0.1.0-beta.1")

        self.assertEqual(errors, [])

    def test_only_first_party_manifests_are_checked(self) -> None:
        with tempfile.TemporaryDirectory() as temporary_directory:
            root = Path(temporary_directory)
            (root / "前端/app").mkdir(parents=True)
            (root / "src-tauri").mkdir()
            (root / "后端/crates/haven-infrastructure").mkdir(parents=True)
            (root / "后端").mkdir(exist_ok=True)

            (root / "前端/app/package.json").write_text(
                '{"version":"0.1.0-beta.1"}', encoding="utf-8"
            )
            (root / "前端/app/package-lock.json").write_text(
                '{"version":"0.1.0-beta.1","packages":{"":{"version":"0.1.0-beta.1"}}}',
                encoding="utf-8",
            )
            (root / "src-tauri/tauri.conf.json").write_text(
                '{"version":"0.1.0-beta.1"}', encoding="utf-8"
            )
            (root / "src-tauri/Cargo.toml").write_text(
                '[package]\nname="haven-tauri"\nversion="0.1.0-beta.1"\n', encoding="utf-8"
            )
            (root / "后端/Cargo.toml").write_text(
                '[workspace.package]\nversion="0.1.0-beta.1"\n', encoding="utf-8"
            )
            (root / "后端/crates/haven-infrastructure/Cargo.toml").write_text(
                '[package]\nname="haven-infrastructure"\nversion="0.1.0-beta.1"\n'
                '[dependencies]\n'
                'haven-application={version="0.1.0-beta.1",path="../haven-application"}\n',
                encoding="utf-8",
            )

            errors = VERSION_CHECK.validate(root, "0.1.0-beta.1")

            self.assertEqual(errors, [])


if __name__ == "__main__":
    unittest.main()
