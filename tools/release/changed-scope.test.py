from __future__ import annotations

import os
import shutil
import subprocess
import tempfile
import textwrap
import unittest
from pathlib import Path


ROOT = Path(__file__).resolve().parents[2]


def classifier_script(workflow: str) -> str:
    lines = (ROOT / ".github/workflows" / workflow).read_text(encoding="utf-8").splitlines()
    start = lines.index("      - name: Classify changed paths")
    run = next(index for index in range(start, len(lines)) if lines[index] == "        run: |")
    script = []
    for line in lines[run + 1 :]:
        if line and not line.startswith("          "):
            break
        script.append(line)
    return textwrap.dedent("\n".join(script))


def bash_executable() -> str:
    if os.name == "nt":
        installed = Path(os.environ.get("ProgramFiles", "C:/Program Files")) / "Git/bin/bash.exe"
        if installed.is_file():
            return str(installed)
    executable = shutil.which("bash")
    if executable is None:
        raise RuntimeError("Bash is required to exercise the workflow classifiers")
    return executable


class ChangedScopeTests(unittest.TestCase):
    def classify(
        self, workflow: str, paths: str | list[str], *, missing_base: bool = False
    ) -> dict[str, str]:
        with tempfile.TemporaryDirectory(prefix="haven-scope-test-") as temporary:
            repository = Path(temporary)

            def git(*arguments: str) -> str:
                result = subprocess.run(
                    ["git", "-c", "user.name=Scope Test", "-c", "user.email=scope@example.invalid", *arguments],
                    cwd=repository,
                    check=True,
                    capture_output=True,
                    encoding="utf-8",
                )
                return result.stdout.strip()

            git("init", "--quiet")
            git("config", "core.quotepath", "true")
            git("commit", "--quiet", "--allow-empty", "-m", "base")
            base = git("rev-parse", "HEAD")
            for path in [paths] if isinstance(paths, str) else paths:
                changed = repository / path
                changed.parent.mkdir(parents=True, exist_ok=True)
                changed.write_text("fixture\n", encoding="utf-8")
                git("add", "--", path)
            git("commit", "--quiet", "-m", "change")
            head = git("rev-parse", "HEAD")
            output = repository / "scope-output.txt"
            environment = {
                **os.environ,
                "EVENT_NAME": "pull_request",
                "BASE_SHA": "missing-commit" if missing_base else base,
                "HEAD_SHA": head,
                "GITHUB_OUTPUT": output.as_posix(),
            }
            result = subprocess.run(
                [bash_executable(), "--noprofile", "--norc", "-e", "-o", "pipefail", "-c", classifier_script(workflow)],
                cwd=repository,
                env=environment,
                capture_output=True,
                encoding="utf-8",
            )
            if missing_base:
                self.assertNotEqual(result.returncode, 0, "Invalid Git input must fail closed")
                self.assertFalse(output.exists(), "Failed classification must not emit false scope flags")
                return {}
            self.assertEqual(result.returncode, 0, result.stderr)
            return dict(line.split("=", 1) for line in output.read_text(encoding="utf-8").splitlines())

    def test_chinese_frontend_paths_select_frontend_and_javascript(self) -> None:
        self.assertEqual(self.classify("ci.yml", "前端/app/example.tsx")["frontend"], "true")
        self.assertEqual(self.classify("codeql.yml", "前端/app/example.tsx")["javascript"], "true")

    def test_chinese_backend_paths_select_backend_and_rust(self) -> None:
        self.assertEqual(self.classify("ci.yml", "后端/crates/example.rs")["backend"], "true")
        self.assertEqual(self.classify("codeql.yml", "后端/crates/example.rs")["rust"], "true")

    def test_spaces_do_not_split_a_chinese_path(self) -> None:
        self.assertEqual(self.classify("ci.yml", "前端/app/space name.tsx")["frontend"], "true")
        self.assertEqual(self.classify("codeql.yml", "前端/app/space name.tsx")["javascript"], "true")

    def test_multiple_paths_select_both_frontend_and_backend(self) -> None:
        paths = ["前端/app/example.tsx", "后端/crates/example.rs"]
        ci = self.classify("ci.yml", paths)
        self.assertEqual(ci["frontend"], "true")
        self.assertEqual(ci["backend"], "true")
        codeql = self.classify("codeql.yml", paths)
        self.assertEqual(codeql["javascript"], "true")
        self.assertEqual(codeql["rust"], "true")

    def test_invalid_base_commit_fails_instead_of_skipping_checks(self) -> None:
        for workflow in ("ci.yml", "codeql.yml"):
            with self.subTest(workflow=workflow):
                self.classify(workflow, "前端/app/example.tsx", missing_base=True)

    def test_documentation_does_not_select_unrelated_components(self) -> None:
        self.assertTrue(all(value == "false" for value in self.classify("ci.yml", "docs/example.md").values()))
        self.assertTrue(all(value == "false" for value in self.classify("codeql.yml", "docs/example.md").values()))

    def test_workflow_changes_select_every_component(self) -> None:
        self.assertTrue(all(value == "true" for value in self.classify("ci.yml", ".github/workflows/ci.yml").values()))
        self.assertTrue(all(value == "true" for value in self.classify("codeql.yml", ".github/workflows/codeql.yml").values()))


if __name__ == "__main__":
    unittest.main()
