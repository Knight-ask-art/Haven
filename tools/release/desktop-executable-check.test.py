from __future__ import annotations

import contextlib
import importlib.util
import io
import json
import subprocess
import tempfile
import unittest
from pathlib import Path
from unittest.mock import patch

spec = importlib.util.spec_from_file_location("desktop_executable_check", Path(__file__).with_name("desktop-executable-check.py"))
assert spec and spec.loader
checker = importlib.util.module_from_spec(spec)
spec.loader.exec_module(checker)


class ShippedExecutableTests(unittest.TestCase):
    def run_probe(self, stdout: bytes = b"", error: Exception | None = None) -> int:
        with tempfile.TemporaryDirectory() as directory:
            info = Path(directory) / "build-info.json"
            info.write_text(json.dumps({"version": "0.1.0", "commit": "a" * 40, "assets": []}), encoding="utf-8")
            with patch.object(checker.subprocess, "run", side_effect=error, return_value=subprocess.CompletedProcess([], 0, stdout)), contextlib.redirect_stdout(io.StringIO()):
                return checker.main(["--exe", str(Path(directory) / "haven.exe"), "--build-info", str(info)])

    def test_exact_embedded_manifest_is_required(self) -> None:
        self.assertEqual(self.run_probe(json.dumps({"version": "0.1.0", "commit": "a" * 40, "assets": []}).encode()), 0)
        self.assertEqual(self.run_probe(json.dumps({"version": "0.1.0", "commit": "b" * 40, "assets": []}).encode()), 1)

    def test_missing_or_invalid_manifest_fails_closed(self) -> None:
        for body in (b"", b"not JSON", b"{}"):
            self.assertEqual(self.run_probe(body), 1)

    def test_failed_or_hanging_process_fails_closed(self) -> None:
        for error in (OSError("missing executable"), subprocess.TimeoutExpired("probe", 20), subprocess.CalledProcessError(1, "probe")):
            self.assertEqual(self.run_probe(error=error), 1)


if __name__ == "__main__":
    unittest.main()
