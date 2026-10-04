from __future__ import annotations
import hashlib
import importlib.util
import json
import tempfile
import unittest
from pathlib import Path

spec = importlib.util.spec_from_file_location("desktop_assets_check", Path(__file__).with_name("desktop-assets-check.py"))
module = importlib.util.module_from_spec(spec)
spec.loader.exec_module(module)
COMMIT = "a" * 40


class DesktopAssetsTests(unittest.TestCase):
    def fixture(self, directory: str):
        root = Path(directory)
        (root / "assets").mkdir()
        data = {"index.html": b"<div id='root'></div>", "assets/app.css": b".settings-navigation__text{}.sources-settings{}.search-results{}", "assets/app.js": b"export default 1"}
        entries = []
        for name, content in data.items():
            (root / name).write_bytes(content)
            entries.append({"path": name, "byteSize": len(content), "sha256": hashlib.sha256(content).hexdigest()})
        info = {"schemaVersion": 1, "version": "0.1.0", "commit": COMMIT, "dirty": False, "assets": entries}
        (root / "build-info.json").write_text(json.dumps(info), encoding="utf-8")
        return root, info

    def test_current_clean_build(self):
        with tempfile.TemporaryDirectory() as directory:
            root, _ = self.fixture(directory)
            self.assertEqual(module.validate(root, "0.1.0", COMMIT, True), [])

    def test_rejects_stale_commit_version_and_dirty_release(self):
        with tempfile.TemporaryDirectory() as directory:
            root, info = self.fixture(directory)
            info["dirty"] = True
            (root / "build-info.json").write_text(json.dumps(info), encoding="utf-8")
            errors = module.validate(root, "0.2.0", "b" * 40, True)
            self.assertEqual(len(errors), 3)

    def test_rejects_modified_or_missing_product_styles(self):
        with tempfile.TemporaryDirectory() as directory:
            root, _ = self.fixture(directory)
            (root / "assets/app.css").write_bytes(b".old-ui{}")
            errors = module.validate(root, "0.1.0")
            self.assertTrue(any("differs" in e for e in errors))
            self.assertTrue(any(".search-results" in e for e in errors))

    def test_rejects_path_escape_and_duplicate_manifest(self):
        with tempfile.TemporaryDirectory() as directory:
            root, info = self.fixture(directory)
            info["assets"].append({"path": "../escape", "byteSize": 0, "sha256": ""})
            info["assets"].append(info["assets"][0])
            (root / "build-info.json").write_text(json.dumps(info), encoding="utf-8")
            self.assertEqual(sum("out-of-root" in e for e in module.validate(root, "0.1.0")), 2)

    def test_malformed_manifest_fails_closed(self):
        with tempfile.TemporaryDirectory() as directory:
            root, _ = self.fixture(directory)
            (root / "build-info.json").write_text("[]", encoding="utf-8")
            self.assertTrue(module.validate(root, "0.1.0"))


if __name__ == "__main__":
    unittest.main()
