from __future__ import annotations

import importlib.util
import io
import sys
import tempfile
import unittest
from contextlib import redirect_stderr, redirect_stdout
from pathlib import Path


MODULE_PATH = Path(__file__).with_name("bundle-version.py")
SPEC = importlib.util.spec_from_file_location("bundle_version", MODULE_PATH)
if SPEC is None or SPEC.loader is None:
    raise RuntimeError(f"无法加载 bundle 版本模块: {MODULE_PATH}")
BUNDLE_VERSION = importlib.util.module_from_spec(SPEC)
sys.modules[SPEC.name] = BUNDLE_VERSION
SPEC.loader.exec_module(BUNDLE_VERSION)


class BundleVersionTests(unittest.TestCase):
    def test_beta_prerelease_maps_to_numeric_msi_version(self) -> None:
        resolved = BUNDLE_VERSION.resolve("v0.1.0-beta.1")
        self.assertEqual(resolved.release_ref, "v0.1.0-beta.1")
        self.assertEqual(resolved.release_version, "0.1.0-beta.1")
        self.assertEqual(resolved.bundle_version, "0.1.0-1")
        self.assertTrue(resolved.prerelease)

    def test_stable_version_is_passed_through(self) -> None:
        resolved = BUNDLE_VERSION.resolve("v0.1.0")
        self.assertEqual(resolved.release_version, "0.1.0")
        self.assertEqual(resolved.bundle_version, "0.1.0")
        self.assertFalse(resolved.prerelease)

    def test_outputs_are_stable_strings(self) -> None:
        self.assertEqual(
            BUNDLE_VERSION.resolve("v0.1.0-beta.1").outputs(),
            {
                "release_ref": "v0.1.0-beta.1",
                "release_version": "0.1.0-beta.1",
                "bundle_version": "0.1.0-1",
                "prerelease": "true",
            },
        )

    def test_branch_or_bare_version_refs_are_rejected(self) -> None:
        for ref in ("main", "0.1.0-beta.1", "release/0.1.0", "v0.1", "v0.1.0.1"):
            with self.subTest(ref=ref):
                with self.assertRaises(BUNDLE_VERSION.ReleaseVersionError):
                    BUNDLE_VERSION.resolve(ref)

    def test_prerelease_without_numeric_identifier_is_rejected(self) -> None:
        with self.assertRaises(BUNDLE_VERSION.ReleaseVersionError):
            BUNDLE_VERSION.resolve("v0.1.0-beta")

    def test_prerelease_number_above_msi_limit_is_rejected(self) -> None:
        with self.assertRaises(BUNDLE_VERSION.ReleaseVersionError):
            BUNDLE_VERSION.resolve("v0.1.0-beta.65536")

    def test_cli_appends_github_outputs(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            output_path = Path(directory) / "github_output"
            with redirect_stdout(io.StringIO()):
                exit_code = BUNDLE_VERSION.main(
                    ["--ref", "v0.1.0-beta.1", "--github-output", str(output_path)]
                )
            self.assertEqual(exit_code, 0)
            lines = output_path.read_text(encoding="utf-8").splitlines()
            self.assertIn("release_ref=v0.1.0-beta.1", lines)
            self.assertIn("release_version=0.1.0-beta.1", lines)
            self.assertIn("bundle_version=0.1.0-1", lines)
            self.assertIn("prerelease=true", lines)

    def test_cli_reports_invalid_ref(self) -> None:
        stderr = io.StringIO()
        with redirect_stderr(stderr), redirect_stdout(io.StringIO()):
            exit_code = BUNDLE_VERSION.main(["--ref", "main"])
        self.assertEqual(exit_code, 1)
        self.assertIn("v<semver>", stderr.getvalue())

    def test_channel_ordering_collision_is_known_limitation(self) -> None:
        # Known limitation of the current compatibility mapping: the channel
        # name is not encoded in the numeric MSI slot. Keep this explicit until
        # the updater ordering policy is redesigned as a separate task.
        for ref in ("v0.1.0-alpha.1", "v0.1.0-beta.1", "v0.1.0-rc.1"):
            with self.subTest(ref=ref):
                self.assertEqual(
                    BUNDLE_VERSION.resolve(ref).bundle_version, "0.1.0-1"
                )


if __name__ == "__main__":
    unittest.main()
