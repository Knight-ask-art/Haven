#!/usr/bin/env python3
"""Unit tests for the four-layer evidence checker."""

from __future__ import annotations

import importlib.util
import json
import tempfile
import unittest
from pathlib import Path
from unittest.mock import patch


ROOT = Path(__file__).resolve().parents[2]
SPEC = importlib.util.spec_from_file_location("film_tv_evidence_check", Path(__file__).with_name("evidence-check.py"))
if SPEC is None or SPEC.loader is None:
    raise RuntimeError("无法加载 evidence-check.py")
module = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(module)


PASS_SHA = "0" * 64
PASS_COMMIT = "1" * 40


class EvidenceCheckerTests(unittest.TestCase):
    @classmethod
    def setUpClass(cls) -> None:
        cls.matrix = module.validate_matrix(module.load_json(module.MATRIX_PATH))

    def local_record(self, **overrides):
        checks = [
            {
                "id": check_id,
                "result": "pass",
                "exitCode": 0,
                "durationMs": 1,
                "outputSha256": PASS_SHA,
                "timedOut": False,
            }
            for check_id in self.matrix["_indexes"]["checkById"]
        ]
        record = {
            "schemaVersion": 1,
            "layer": "local",
            "capability": "all",
            "status": "pass",
            "verifiedAt": "2026-09-02T00:00:00Z",
            "commit": PASS_COMMIT,
            "worktreeSha256": PASS_SHA,
            "environment": {
                "os": "test",
                "mode": "developer-machine",
                "fixtureSet": "acceptance-matrix@1",
            },
            "checks": checks,
            "generator": "test",
        }
        record.update(overrides)
        record["recordSha256"] = module.record_sha256(record)
        return record

    def test_matrix_has_four_layers_and_nine_unaccepted_capabilities(self):
        self.assertEqual(module.LAYERS, ("local", "ci", "runtime", "release"))
        self.assertEqual(len(self.matrix["_indexes"]["capabilityById"]), 9)
        self.assertIn("FTV-LCL-COMMON-NETWORK-001", self.matrix["_indexes"]["checkById"])
        self.assertIn(
            "FTV-LCL-COMMON-NETWORK-001",
            self.matrix["_indexes"]["capabilityById"]["proxy_security"]["requiredChecks"]["local"],
        )
        self.assertTrue(
            all(
                capability["acceptanceStatus"] == "not-accepted"
                for capability in self.matrix["_indexes"]["capabilityById"].values()
            )
        )

    def test_local_record_requires_every_command_check(self):
        record = self.local_record()
        module.validate_record(record, self.matrix, "local")
        record["checks"] = record["checks"][:-1]
        with self.assertRaises(module.EvidenceError):
            module.validate_record(record, self.matrix, "local")

    def test_ci_record_cannot_be_created_without_a_run_id(self):
        record = self.local_record(layer="ci")
        with self.assertRaises(module.EvidenceError):
            module.validate_record(record, self.matrix, "ci")
        record["ciRunId"] = "12345"
        record["environment"] = {
            "os": "test",
            "mode": "github-actions",
            "fixtureSet": "acceptance-matrix@1",
        }
        record["ciRepository"] = "Knight-ask-art/Haven"
        record["ciHeadSha"] = PASS_COMMIT
        record["ciRunVerified"] = True
        record["recordSha256"] = module.record_sha256(record)
        module.validate_record(record, self.matrix, "ci")

    def test_ci_generation_requires_github_actions_context(self):
        with patch.dict(module.os.environ, {"GITHUB_RUN_ID": "12345"}, clear=True):
            self.assertEqual(
                module.resolve_ci_run_id(),
                (
                    "blocked-local-run",
                    "GITHUB_ACTIONS 不是 true；本地执行不能冒充 CI 证据",
                ),
            )
        with patch.dict(
            module.os.environ,
            {
                "GITHUB_ACTIONS": "true",
                "GITHUB_RUN_ID": "12345",
                "GITHUB_REPOSITORY": "Knight-ask-art/Haven",
                "GITHUB_SHA": PASS_COMMIT,
            },
            clear=True,
        ):
            self.assertEqual(
                module.resolve_ci_run_id(PASS_COMMIT),
                ("12345", "GitHub Actions 上下文缺少 GITHUB_TOKEN；不能证明 run 来源"),
            )

    def test_ci_generation_binds_run_to_github_repository_and_commit(self):
        class FakeResponse:
            status = 200

            def __enter__(self):
                return self

            def __exit__(self, exc_type, exc_value, traceback):
                return False

            def read(self):
                return json.dumps(
                    {
                        "id": 12345,
                        "head_sha": PASS_COMMIT,
                        "repository": {"full_name": "Knight-ask-art/Haven"},
                    }
                ).encode("utf-8")

        with patch.dict(
            module.os.environ,
            {
                "GITHUB_ACTIONS": "true",
                "GITHUB_RUN_ID": "12345",
                "GITHUB_REPOSITORY": "Knight-ask-art/Haven",
                "GITHUB_SHA": PASS_COMMIT,
                "GITHUB_TOKEN": "test-token",
            },
            clear=True,
        ), patch.object(module, "urlopen", return_value=FakeResponse()) as open_url:
            self.assertEqual(module.resolve_ci_run_id(PASS_COMMIT), ("12345", None))
            request = open_url.call_args.args[0]
            self.assertEqual(
                request.full_url,
                "https://api.github.com/repos/Knight-ask-art/Haven/actions/runs/12345",
            )
            self.assertEqual(request.get_header("Authorization"), "Bearer test-token")

    def test_ci_generation_does_not_send_token_to_untrusted_api_url(self):
        with patch.dict(
            module.os.environ,
            {
                "GITHUB_ACTIONS": "true",
                "GITHUB_RUN_ID": "12345",
                "GITHUB_REPOSITORY": "Knight-ask-art/Haven",
                "GITHUB_SHA": PASS_COMMIT,
                "GITHUB_TOKEN": "test-token",
                "GITHUB_API_URL": "https://attacker.invalid",
            },
            clear=True,
        ), patch.object(module, "urlopen") as open_url:
            self.assertEqual(
                module.resolve_ci_run_id(PASS_COMMIT),
                ("12345", "GitHub Actions API 地址不是本仓库允许的 GitHub API"),
            )
            open_url.assert_not_called()

    def test_failure_tail_redacts_full_headers_and_posix_paths(self):
        output = (
            "Authorization: Bearer first-secret second-secret\n"
            "Cookie: session=secret-value; refresh=another-secret\n"
            "diagnostic=/opt/haven/private-state\n"
        )
        redacted = module.redact_output(output)
        self.assertNotIn("first-secret", redacted)
        self.assertNotIn("second-secret", redacted)
        self.assertNotIn("session=secret-value", redacted)
        self.assertNotIn("another-secret", redacted)
        self.assertNotIn("/opt/haven/private-state", redacted)

    def test_matrix_rejects_removal_of_protected_contract_items(self):
        matrix = module.load_json(module.MATRIX_PATH)
        matrix["scenarios"] = [
            scenario
            for scenario in matrix["scenarios"]
            if scenario["id"] != "FTV-RUN-PLAY-002"
        ]
        with self.assertRaises(module.EvidenceError):
            module.validate_matrix(matrix)

    def test_local_record_requires_worktree_fingerprint(self):
        record = self.local_record()
        record.pop("worktreeSha256")
        with self.assertRaises(module.EvidenceError):
            module.validate_record(record, self.matrix, "local")

    def test_record_digest_covers_mutated_content(self):
        record = self.local_record()
        module.validate_record(record, self.matrix, "local")
        record["status"] = "fail"
        with self.assertRaises(module.EvidenceError):
            module.validate_record(record, self.matrix, "local")

    def test_local_ci_invocation_is_explicitly_blocked(self):
        record = self.local_record(
            layer="ci",
            status="blocked",
            ciRunId="blocked-local-run",
            blockedReason="GITHUB_RUN_ID 缺失；本地执行不能冒充 CI 证据",
        )
        module.validate_record(record, self.matrix, "ci")

    def test_runtime_record_rejects_raw_url(self):
        record = {
            "schemaVersion": 1,
            "layer": "runtime",
            "capability": "subtitles",
            "status": "partial",
            "verifiedAt": "2026-09-02T00:00:00Z",
            "commit": PASS_COMMIT,
            "candidateSha256": PASS_SHA,
            "environment": {
                "os": "test",
                "mode": "candidate",
                "fixtureSet": "acceptance-matrix@1",
            },
            "checks": [
                {
                    "id": scenario_id,
                    "result": "not-run",
                }
                for scenario_id in self.matrix["_indexes"]["capabilityById"]["subtitles"]["requiredScenarios"]
                if self.matrix["_indexes"]["scenarioById"][scenario_id]["layer"] == "runtime"
            ],
            "observedSource": "https://fixture.invalid/never-store-this",
        }
        with self.assertRaises(module.EvidenceError):
            module.validate_record(record, self.matrix, "runtime")

    def test_runtime_record_rejects_any_posix_absolute_path(self):
        record = {
            "schemaVersion": 1,
            "layer": "runtime",
            "capability": "subtitles",
            "status": "partial",
            "verifiedAt": "2026-09-02T00:00:00Z",
            "commit": PASS_COMMIT,
            "candidateSha256": PASS_SHA,
            "environment": {
                "os": "test",
                "mode": "candidate",
                "fixtureSet": "acceptance-matrix@1",
            },
            "checks": [
                {
                    "id": scenario_id,
                    "result": "not-run",
                }
                for scenario_id in self.matrix["_indexes"]["capabilityById"]["subtitles"]["requiredScenarios"]
                if self.matrix["_indexes"]["scenarioById"][scenario_id]["layer"] == "runtime"
            ],
            "observation": "diagnostic=/opt/haven/private-state",
        }
        with self.assertRaises(module.EvidenceError):
            module.validate_record(record, self.matrix, "runtime")

    def test_release_pass_requires_runtime_reference_and_enablement(self):
        capability = "multi_source_search"
        scenario_ids = self.matrix["_indexes"]["capabilityById"][capability]["requiredScenarios"]
        record = {
            "schemaVersion": 1,
            "layer": "release",
            "capability": capability,
            "status": "pass",
            "verifiedAt": "2026-09-02T00:00:00Z",
            "commit": PASS_COMMIT,
            "candidateSha256": PASS_SHA,
            "environment": {
                "os": "test",
                "mode": "candidate",
                "fixtureSet": "acceptance-matrix@1",
            },
            "checks": [
                {"id": scenario_id, "result": "pass", "observationSha256": PASS_SHA}
                for scenario_id in scenario_ids
                if self.matrix["_indexes"]["scenarioById"][scenario_id]["layer"] == "release"
            ],
            "evidenceRefs": {
                "local": {"recordSha256": PASS_SHA},
                "ci": {"recordSha256": PASS_SHA},
                "runtime": {"recordSha256": PASS_SHA},
            },
            "artifactSha256": PASS_SHA,
            "rollback": {"available": True, "tested": True},
            "enabled": True,
        }
        record["recordSha256"] = module.record_sha256(record)
        module.validate_record(record, self.matrix, "release")
        record["evidenceRefs"].pop("runtime")
        with self.assertRaises(module.EvidenceError):
            module.validate_record(record, self.matrix, "release")

    def test_release_pass_requires_a_tested_rollback(self):
        record = self.local_record(
            layer="release",
            capability="multi_source_search",
            status="pass",
            candidateSha256=PASS_SHA,
            checks=[
                {"id": "FTV-REL-SEARCH-001", "result": "pass", "observationSha256": PASS_SHA}
            ],
            evidenceRefs={
                "local": {"recordSha256": PASS_SHA},
                "ci": {"recordSha256": PASS_SHA},
                "runtime": {"recordSha256": PASS_SHA},
            },
            artifactSha256=PASS_SHA,
            rollback={"available": True, "tested": False},
            enabled=True,
        )
        with self.assertRaises(module.EvidenceError):
            module.validate_record(record, self.matrix, "release")

    def test_release_bundle_matches_records_and_artifact_bytes(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            candidate = root / "candidate.bin"
            artifact = root / "artifact.bin"
            candidate.write_bytes(b"candidate")
            artifact.write_bytes(b"artifact")
            candidate_sha = module.file_sha256(candidate)
            artifact_sha = module.file_sha256(artifact)

            local = self.local_record()
            ci = self.local_record(
                layer="ci",
                ciRunId="12345",
                environment={
                    "os": "test",
                    "mode": "github-actions",
                    "fixtureSet": "acceptance-matrix@1",
                },
                ciRepository="Knight-ask-art/Haven",
                ciHeadSha=PASS_COMMIT,
                ciRunVerified=True,
            )
            runtime_scenarios = [
                scenario_id
                for scenario_id in self.matrix["_indexes"]["capabilityById"]["multi_source_search"][
                    "requiredScenarios"
                ]
                if self.matrix["_indexes"]["scenarioById"][scenario_id]["layer"] == "runtime"
            ]
            runtime = {
                "schemaVersion": 1,
                "layer": "runtime",
                "capability": "multi_source_search",
                "status": "pass",
                "verifiedAt": "2026-09-02T00:00:00Z",
                "commit": PASS_COMMIT,
                "candidateSha256": candidate_sha,
                "environment": {
                    "os": "test",
                    "mode": "candidate",
                    "fixtureSet": "acceptance-matrix@1",
                },
                "checks": [
                    {
                        "id": scenario_id,
                        "result": "pass",
                        "observationSha256": PASS_SHA,
                    }
                    for scenario_id in runtime_scenarios
                ],
            }
            runtime["recordSha256"] = module.record_sha256(runtime)
            release_scenario = "FTV-REL-SEARCH-001"
            release = {
                "schemaVersion": 1,
                "layer": "release",
                "capability": "multi_source_search",
                "status": "pass",
                "verifiedAt": "2026-09-02T00:00:00Z",
                "commit": PASS_COMMIT,
                "candidateSha256": candidate_sha,
                "environment": {
                    "os": "test",
                    "mode": "candidate",
                    "fixtureSet": "acceptance-matrix@1",
                },
                "checks": [
                    {
                        "id": release_scenario,
                        "result": "pass",
                        "observationSha256": PASS_SHA,
                    }
                ],
                "evidenceRefs": {
                    "local": {"recordSha256": local["recordSha256"]},
                    "ci": {"recordSha256": ci["recordSha256"]},
                    "runtime": {"recordSha256": runtime["recordSha256"]},
                },
                "artifactSha256": artifact_sha,
                "rollback": {"available": True, "tested": True},
                "enabled": True,
            }
            release["recordSha256"] = module.record_sha256(release)

            paths = {
                "local": root / "local.json",
                "ci": root / "ci.json",
                "runtime": root / "runtime.json",
                "release": root / "release.json",
            }
            for name, record in (
                ("local", local),
                ("ci", ci),
                ("runtime", runtime),
                ("release", release),
            ):
                paths[name].write_text(json.dumps(record), encoding="utf-8")

            module.validate_bundle(
                self.matrix,
                local_path=paths["local"],
                ci_path=paths["ci"],
                runtime_path=paths["runtime"],
                release_path=paths["release"],
                candidate_path=candidate,
                artifact_path=artifact,
            )

            ci["worktreeSha256"] = "2" * 64
            ci["recordSha256"] = module.record_sha256(ci)
            release["evidenceRefs"]["ci"]["recordSha256"] = ci["recordSha256"]
            release["recordSha256"] = module.record_sha256(release)
            paths["ci"].write_text(json.dumps(ci), encoding="utf-8")
            paths["release"].write_text(json.dumps(release), encoding="utf-8")
            with self.assertRaises(module.EvidenceError):
                module.validate_bundle(
                    self.matrix,
                    local_path=paths["local"],
                    ci_path=paths["ci"],
                    runtime_path=paths["runtime"],
                    release_path=paths["release"],
                    candidate_path=candidate,
                    artifact_path=artifact,
                )


if __name__ == "__main__":
    unittest.main(verbosity=2)
