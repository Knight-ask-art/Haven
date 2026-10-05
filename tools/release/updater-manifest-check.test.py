from __future__ import annotations
import base64
import copy
import importlib.util
import io
import json
import os
import re
import sys
import tempfile
import unittest
from contextlib import redirect_stderr, redirect_stdout
from pathlib import Path
from urllib.parse import quote, unquote, urlsplit

spec = importlib.util.spec_from_file_location("updater_manifest_check", Path(__file__).with_name("updater-manifest-check.py"))
module = importlib.util.module_from_spec(spec)
spec.loader.exec_module(module)

producer_spec = importlib.util.spec_from_file_location("make_updater_manifest", Path(__file__).with_name("make-updater-manifest.py"))
producer = importlib.util.module_from_spec(producer_spec)
sys.modules[producer_spec.name] = producer
producer_spec.loader.exec_module(producer)

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


BINARY = b"MZ" + b"\x90" * 4096
# The sidecar is base64 of the minisign signature box, exactly what the pinned
# bundler writes next to the installer and what the client decodes.
MINISIGN_BOX = (
    "untrusted comment: signature from tauri secret key\n"
    "RWRZ1rQ0ZP4b0m8v0N3s0mZ9Z0k8Q0m3RWRZ1rQ0ZP4b0m8v0N3s0mZ9Z0k8Q0m3\n"
    "trusted comment: timestamp:1700000000\tfile:Haven_0.1.0_x64-setup.exe\n"
    "0Vf8m0X2p0m2s0m9k0Q0m3Z0k8Q0m3Z0k8Q0m3Z0k8Q0m3Z0k8Q0m3Z0k8Q0m3\n"
)
FAKE_SIGNATURE = base64.b64encode(MINISIGN_BOX.encode("utf-8")).decode("ascii")
PUB_DATE_PATTERN = re.compile(r"^\d{4}-\d{2}-\d{2}T\d{2}:\d{2}:\d{2}Z$")


class UpdaterManifestProducerTests(unittest.TestCase):
    def test_release_produces_a_public_feed_from_the_uploaded_asset_names(self):
        workflow = (Path(__file__).resolve().parents[2] / ".github/workflows/release.yml").read_text(encoding="utf-8")
        self.assertIn("uploadUpdaterJson: false", workflow)
        generation = workflow.index("- name: Produce the public Windows updater manifest")
        verification = workflow.index("- name: Download and verify draft update artifacts")
        publication = workflow.index("- name: Publish verified stable update")
        source = workflow[generation:verification]
        download = source.index("gh release download")
        discover = source.index("Get-ChildItem -LiteralPath $destination")
        produce = source.index("python tools/release/make-updater-manifest.py")
        upload = source.index("gh release upload")
        self.assertLess(download, discover)
        self.assertLess(discover, produce)
        self.assertLess(produce, upload)
        self.assertIn("--installer $installer[0].FullName", source)
        self.assertNotIn("artifactPaths", source)
        self.assertLess(verification, publication)
        for gate in ("updater-manifest-check.py", "--example verify_updater", "desktop-executable-check.py"):
            self.assertIn(gate, workflow[verification:publication])

    def layout(
        self,
        directory,
        installer_name=NAME,
        signature_name=None,
        installer_bytes=BINARY,
        signature_text=FAKE_SIGNATURE,
    ):
        root = Path(directory)
        installer = root / installer_name
        installer.write_bytes(installer_bytes)
        signature = root / (signature_name or f"{installer_name}.sig")
        if signature_text is not None:
            signature.write_text(signature_text, encoding="utf-8")
        return root, installer, signature

    def run_producer(
        self,
        installer,
        signature,
        output,
        version="0.1.0",
        repository="test/Haven",
        tag="v0.1.0",
        notes="fixture notes",
    ):
        stdout, stderr = io.StringIO(), io.StringIO()
        with redirect_stdout(stdout), redirect_stderr(stderr):
            code = producer.main(
                [
                    "--installer", str(installer),
                    "--signature", str(signature),
                    "--version", version,
                    "--repository", repository,
                    "--tag", tag,
                    "--output", str(output),
                    "--notes", notes,
                ]
            )
        return code, stdout.getvalue(), stderr.getvalue()

    def test_producer_output_passes_the_existing_checker(self):
        with tempfile.TemporaryDirectory() as directory:
            root, installer, signature = self.layout(directory)
            output = root / "latest.json"
            code, stdout, stderr = self.run_producer(installer, signature, output)
            self.assertEqual((code, stderr), (0, ""))
            self.assertIn("make-updater-manifest:", stdout)
            value = json.loads(output.read_text(encoding="utf-8"))
            self.assertEqual(
                module.validate(value, root, "0.1.0", "test/Haven", "v0.1.0"), []
            )

    def test_feed_shape_matches_the_client_contract(self):
        with tempfile.TemporaryDirectory() as directory:
            root, installer, signature = self.layout(directory)
            output = root / "latest.json"
            self.run_producer(installer, signature, output, notes="修复了搜索\n第二行")
            value = json.loads(output.read_text(encoding="utf-8"))
            self.assertEqual(value["version"], "0.1.0")
            self.assertEqual(value["notes"], "修复了搜索\n第二行")
            self.assertRegex(value["pub_date"], PUB_DATE_PATTERN)
            self.assertEqual(
                sorted(value["platforms"]), ["windows-x86_64", "windows-x86_64-nsis"]
            )
            expected = dict(value["platforms"]["windows-x86_64"])
            self.assertEqual(value["platforms"]["windows-x86_64-nsis"], expected)
            self.assertEqual(expected["signature"], FAKE_SIGNATURE)
            self.assertEqual(
                expected["url"],
                f"https://github.com/test/Haven/releases/download/v0.1.0/{NAME}",
            )

    def test_space_and_non_ascii_installer_name_is_percent_encoded(self):
        name = "Haven 0.1.0 便携-setup.exe"
        with tempfile.TemporaryDirectory() as directory:
            root, installer, signature = self.layout(directory, installer_name=name)
            output = root / "latest.json"
            code, _, stderr = self.run_producer(installer, signature, output)
            self.assertEqual((code, stderr), (0, ""))
            value = json.loads(output.read_text(encoding="utf-8"))
            url = value["platforms"]["windows-x86_64"]["url"]
            self.assertEqual(url, f"https://github.com/test/Haven/releases/download/v0.1.0/{quote(name, safe='')}")
            encoded = urlsplit(url).path.rsplit("/", 1)[-1]
            self.assertEqual(encoded, encoded.encode("ascii").decode("ascii"))
            self.assertNotIn(" ", encoded)
            self.assertEqual(unquote(encoded), name)
            self.assertEqual(
                module.validate(value, root, "0.1.0", "test/Haven", "v0.1.0"), []
            )

    def test_signature_is_preserved_with_only_surrounding_whitespace_trimmed(self):
        with tempfile.TemporaryDirectory() as directory:
            root, installer, signature = self.layout(
                directory, signature_text=f" \n{FAKE_SIGNATURE}\n\n"
            )
            output = root / "latest.json"
            self.run_producer(installer, signature, output)
            value = json.loads(output.read_text(encoding="utf-8"))
            self.assertEqual(
                value["platforms"]["windows-x86_64"]["signature"], FAKE_SIGNATURE
            )
            self.assertEqual(
                module.validate(value, root, "0.1.0", "test/Haven", "v0.1.0"), []
            )

    def test_notes_are_written_as_plain_utf8_text(self):
        with tempfile.TemporaryDirectory() as directory:
            root, installer, signature = self.layout(directory)
            output = root / "latest.json"
            self.run_producer(installer, signature, output, notes="更新 说明")
            raw = output.read_bytes()
            self.assertNotIn(b"\\u", raw)
            self.assertTrue(raw.endswith(b"\n"))
            self.assertIn("更新 说明", raw.decode("utf-8"))

    def test_prerelease_tag_requires_the_canonical_bundle_version(self):
        with tempfile.TemporaryDirectory() as directory:
            root, installer, signature = self.layout(directory)
            output = root / "latest.json"
            code, _, stderr = self.run_producer(
                installer, signature, output, version="0.1.0-beta.1", tag="v0.1.0-beta.1"
            )
            self.assertEqual(code, 1)
            self.assertIn("make-updater-manifest:", stderr)
            self.assertFalse(output.exists())

            code, _, stderr = self.run_producer(
                installer, signature, output, version="0.1.0-1", tag="v0.1.0-beta.1"
            )
            self.assertEqual((code, stderr), (0, ""))
            value = json.loads(output.read_text(encoding="utf-8"))
            self.assertEqual(value["version"], "0.1.0-1")
            self.assertEqual(
                module.validate(value, root, "0.1.0-1", "test/Haven", "v0.1.0-beta.1"),
                [],
            )

    def test_rejects_versions_that_do_not_match_the_tag(self):
        with tempfile.TemporaryDirectory() as directory:
            root, installer, signature = self.layout(directory)
            output = root / "latest.json"
            for version in ("0.0.9", "1.0.0", "v0.1.0", "0.1.0 ", "0.1.0.1"):
                with self.subTest(version=version):
                    code, _, stderr = self.run_producer(
                        installer, signature, output, version=version
                    )
                    self.assertEqual(code, 1)
                    self.assertIn("bundle version", stderr)
                    self.assertFalse(output.exists())

    def test_rejects_invalid_tags_and_repositories(self):
        with tempfile.TemporaryDirectory() as directory:
            root, installer, signature = self.layout(directory)
            output = root / "latest.json"
            for tag in ("main", "0.1.0", "release/0.1.0", "v0.1", "v0.1.0.1", "v0.1.0 "):
                with self.subTest(tag=tag):
                    code, _, stderr = self.run_producer(
                        installer, signature, output, tag=tag
                    )
                    self.assertEqual(code, 1)
                    self.assertIn("v<semver>", stderr)
            for repository in (
                "",
                "owner",
                "owner/repo/extra",
                "owner/re po",
                "owner/repo?x=1",
                "https://github.com/owner/repo",
                "owner/repo#fragment",
            ):
                with self.subTest(repository=repository):
                    code, _, stderr = self.run_producer(
                        installer, signature, output, repository=repository
                    )
                    self.assertEqual(code, 1)
                    self.assertIn("OWNER/REPO", stderr)
            self.assertFalse(output.exists())

    def test_rejects_installers_that_are_not_the_windows_nsis_setup(self):
        with tempfile.TemporaryDirectory() as directory:
            root, installer, signature = self.layout(directory)
            output = root / "latest.json"
            for name in (
                "Haven_0.1.0_x64.msi",
                "Haven_0.1.0_x64-setup.exe.bak",
                "setup.exe",
                "Haven_0.1.0_x64-setup",
            ):
                with self.subTest(name=name):
                    candidate = root / name
                    candidate.write_bytes(BINARY)
                    sidecar = root / f"{name}.sig"
                    sidecar.write_text(FAKE_SIGNATURE, encoding="utf-8")
                    code, _, stderr = self.run_producer(candidate, sidecar, output)
                    self.assertEqual(code, 1)
                    self.assertIn("-setup.exe", stderr)
            self.assertFalse(output.exists())

    def test_rejects_a_misnamed_signature_sidecar(self):
        with tempfile.TemporaryDirectory() as directory:
            root, installer, signature = self.layout(
                directory, signature_name="other.sig"
            )
            output = root / "latest.json"
            code, _, stderr = self.run_producer(installer, signature, output)
            self.assertEqual(code, 1)
            self.assertIn("<installer>.sig", stderr)
            self.assertFalse(output.exists())

    def test_rejects_missing_invalid_or_linked_installer(self):
        replacements = {
            "missing": None,
            "not-an-executable": b"<html>" + b"x" * 4096,
            "truncated": b"MZ",
            "directory": b"",
        }
        for label, replacement in replacements.items():
            with self.subTest(installer=label), tempfile.TemporaryDirectory() as directory:
                root, installer, signature = self.layout(directory)
                installer.unlink()
                if label == "directory":
                    installer.mkdir()
                elif replacement is not None:
                    installer.write_bytes(replacement)
                output = root / "latest.json"
                code, _, stderr = self.run_producer(installer, signature, output)
                self.assertEqual(code, 1)
                self.assertIn("make-updater-manifest:", stderr)
                self.assertFalse(output.exists())

        with tempfile.TemporaryDirectory() as directory:
            root, installer, signature = self.layout(directory)
            linked = root / f"linked{os.sep}{NAME}"
            linked.parent.mkdir()
            try:
                os.symlink(installer, linked)
            except (OSError, NotImplementedError) as error:
                self.skipTest(f"symbolic links are unavailable: {error}")
            code, _, stderr = self.run_producer(linked, signature, root / "latest.json")
            self.assertEqual(code, 1)
            self.assertIn("regular compiled setup file", stderr)

    def test_rejects_absent_empty_malformed_and_linked_signature(self):
        with tempfile.TemporaryDirectory() as directory:
            root, installer, signature = self.layout(directory)
            output = root / "latest.json"

            signature.unlink()
            self.assertEqual(self.run_producer(installer, signature, output)[0], 1)

            for label, text, expected in (
                ("empty", "", "empty"),
                ("whitespace", "   \n\n", "empty"),
                ("plain-text", "fixture-signature", "minisign signature"),
                ("bad-length", "AAAAA", "minisign signature"),
                ("not-a-box", base64.b64encode(b"hello").decode("ascii"), "minisign signature"),
                ("wrapped", f"dW50cnVzdGVkIGNvbW1lbnQ6\n{FAKE_SIGNATURE}\n", "minisign signature"),
            ):
                with self.subTest(signature=label):
                    signature.write_text(text, encoding="utf-8")
                    code, _, stderr = self.run_producer(installer, signature, output)
                    self.assertEqual(code, 1)
                    self.assertIn(expected, stderr)

            with self.subTest(signature="non-utf8"):
                signature.write_bytes(b"\xff\xfe\x00\x01" + b"m" * 64)
                code, _, stderr = self.run_producer(installer, signature, output)
                self.assertEqual(code, 1)
                self.assertIn("UTF-8", stderr)

            with self.subTest(signature="empty-file"):
                signature.write_bytes(b"")
                self.assertEqual(self.run_producer(installer, signature, output)[0], 1)

            with self.subTest(signature="missing"):
                signature.unlink()
                code, _, stderr = self.run_producer(installer, signature, output)
                self.assertEqual(code, 1)
                self.assertIn("regular file", stderr)

            self.assertFalse(output.exists())

        with tempfile.TemporaryDirectory() as directory:
            root, installer, signature = self.layout(directory, signature_name="real.sig")
            canonical = root / f"{NAME}.sig"
            try:
                os.symlink(signature, canonical)
            except (OSError, NotImplementedError) as error:
                self.skipTest(f"symbolic links are unavailable: {error}")
            code, _, stderr = self.run_producer(
                installer, canonical, root / "latest.json"
            )
            self.assertEqual(code, 1)
            self.assertIn("regular file", stderr)

    def test_output_is_written_atomically_and_only_to_the_requested_path(self):
        with tempfile.TemporaryDirectory() as directory:
            root, installer, signature = self.layout(directory)
            output = root / "latest.json"
            for _ in range(2):
                code, _, stderr = self.run_producer(installer, signature, output)
                self.assertEqual((code, stderr), (0, ""))
            self.assertEqual(
                sorted(path.name for path in root.iterdir()),
                sorted([NAME, f"{NAME}.sig", "latest.json"]),
            )
            self.assertEqual(installer.read_bytes(), BINARY)
            self.assertEqual(
                signature.read_text(encoding="utf-8"), FAKE_SIGNATURE
            )

    def test_rejects_a_missing_output_directory_and_a_linked_output(self):
        with tempfile.TemporaryDirectory() as directory:
            root, installer, signature = self.layout(directory)
            output = root / "missing" / "latest.json"
            code, _, stderr = self.run_producer(installer, signature, output)
            self.assertEqual(code, 1)
            self.assertIn("output directory", stderr)
            self.assertFalse(output.parent.exists())

            directory_output = root / "latest.json"
            directory_output.mkdir()
            code, _, stderr = self.run_producer(installer, signature, directory_output)
            self.assertEqual(code, 1)
            self.assertIn("file path", stderr)

            target = root / "target.json"
            target.write_text("{}", encoding="utf-8")
            linked = root / "linked.json"
            try:
                os.symlink(target, linked)
            except (OSError, NotImplementedError) as error:
                self.skipTest(f"symbolic links are unavailable: {error}")
            code, _, stderr = self.run_producer(installer, signature, linked)
            self.assertEqual(code, 1)
            self.assertIn("link", stderr)
            self.assertEqual(target.read_text(encoding="utf-8"), "{}")

    def test_importing_the_producer_does_not_reconfigure_global_streams(self):
        stdout, stderr, stdin = sys.stdout, sys.stderr, sys.stdin
        encodings = (stdout.encoding, stderr.encoding)
        fresh_spec = importlib.util.spec_from_file_location(
            "make_updater_manifest_fresh",
            Path(__file__).with_name("make-updater-manifest.py"),
        )
        fresh = importlib.util.module_from_spec(fresh_spec)
        sys.modules[fresh_spec.name] = fresh
        fresh_spec.loader.exec_module(fresh)
        self.assertIs(sys.stdout, stdout)
        self.assertIs(sys.stderr, stderr)
        self.assertIs(sys.stdin, stdin)
        self.assertEqual((sys.stdout.encoding, sys.stderr.encoding), encodings)

    def test_cli_output_stays_ascii_under_a_legacy_code_page(self):
        with tempfile.TemporaryDirectory() as directory:
            root, installer, signature = self.layout(directory)
            output = root / "latest.json"
            signature.unlink()

            def legacy_run():
                raw_out, raw_err = io.BytesIO(), io.BytesIO()
                stream_out = io.TextIOWrapper(raw_out, encoding="cp1252", errors="strict")
                stream_err = io.TextIOWrapper(raw_err, encoding="cp1252", errors="strict")
                try:
                    with redirect_stdout(stream_out), redirect_stderr(stream_err):
                        code = producer.main(
                            [
                                "--installer", str(installer),
                                "--signature", str(signature),
                                "--version", "0.1.0",
                                "--repository", "test/Haven",
                                "--tag", "v0.1.0",
                                "--output", str(output),
                                "--notes", "版本说明",
                            ]
                        )
                    stream_out.flush()
                    stream_err.flush()
                finally:
                    stream_out.detach()
                    stream_err.detach()
                text = raw_out.getvalue().decode("cp1252") + raw_err.getvalue().decode("cp1252")
                text.encode("ascii")
                return code, text

            code, text = legacy_run()
            self.assertEqual(code, 1)
            self.assertIn("make-updater-manifest:", text)

            signature.write_text(FAKE_SIGNATURE, encoding="utf-8")
            code, text = legacy_run()
            self.assertEqual(code, 0)
            self.assertIn("make-updater-manifest:", text)

    def test_help_and_documented_cli_arguments_exist(self):
        parser_text = io.StringIO()
        with redirect_stdout(parser_text):
            with self.assertRaises(SystemExit) as raised:
                producer.main(["--help"])
        self.assertEqual(raised.exception.code, 0)
        help_text = parser_text.getvalue()
        for option in (
            "--installer",
            "--signature",
            "--version",
            "--repository",
            "--tag",
            "--output",
            "--notes",
        ):
            self.assertIn(option, help_text)


if __name__ == "__main__":
    unittest.main()
