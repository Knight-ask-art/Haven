from __future__ import annotations
import copy
import importlib.util
import tempfile
import unittest
from pathlib import Path

spec = importlib.util.spec_from_file_location("updater_manifest_check", Path(__file__).with_name("updater-manifest-check.py"))
module = importlib.util.module_from_spec(spec)
spec.loader.exec_module(module)
NAME = "Haven_0.1.0_x64-setup.exe"


class UpdaterManifestTests(unittest.TestCase):
    def fixture(self, directory: str):
        root = Path(directory)
        (root / NAME).write_bytes(b"MZ" + b"fixture" * 200)
        (root / f"{NAME}.sig").write_text("fixture-signature\n", encoding="utf-8")
        value = {"version": "0.1.0", "notes": "verified release", "platforms": {"windows-x86_64": {"url": f"https://github.com/test/Haven/releases/download/v0.1.0/{NAME}", "signature": "fixture-signature"}}}
        return root, value

    def validate(self, value, root):
        return module.validate(value, root, "0.1.0", "test/Haven", "v0.1.0")

    def test_accepts_only_matching_nsis_metadata(self):
        with tempfile.TemporaryDirectory() as directory:
            root, value = self.fixture(directory)
            self.assertEqual(self.validate(value, root), [])

    def test_rejects_wrong_version_and_signature(self):
        with tempfile.TemporaryDirectory() as directory:
            root, value = self.fixture(directory)
            value["version"] = "0.0.9"
            value["platforms"]["windows-x86_64"]["signature"] = "wrong"
            self.assertEqual(len(self.validate(value, root)), 2)

    def test_rejects_msi_external_hosts_query_and_path_escape(self):
        urls = ["https://github.com/test/Haven/releases/download/v0.1.0/a.msi", "http://github.com/test/Haven/releases/download/v0.1.0/a-setup.exe", "https://example.invalid/a-setup.exe", f"https://github.com/test/Haven/releases/download/v0.1.0/{NAME}?value=fixture", "https://github.com/test/Haven/releases/download/v0.1.0/..%2fescape-setup.exe"]
        with tempfile.TemporaryDirectory() as directory:
            root, value = self.fixture(directory)
            for url in urls:
                with self.subTest(url=url):
                    variant = copy.deepcopy(value)
                    variant["platforms"]["windows-x86_64"]["url"] = url
                    self.assertTrue(self.validate(variant, root))

    def test_rejects_missing_or_non_executable_binary(self):
        with tempfile.TemporaryDirectory() as directory:
            root, value = self.fixture(directory)
            (root / NAME).write_bytes(b"HTML" + b"x" * 2000)
            self.assertTrue(self.validate(value, root))
            (root / NAME).unlink()
            self.assertTrue(self.validate(value, root))

    def test_missing_platform_fails_closed(self):
        with tempfile.TemporaryDirectory() as directory:
            root, value = self.fixture(directory)
            value["platforms"] = {}
            self.assertTrue(self.validate(value, root))


if __name__ == "__main__":
    unittest.main()
