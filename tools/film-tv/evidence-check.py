#!/usr/bin/env python3
"""Validate and execute the film/TV four-layer evidence contract.

This tool deliberately keeps functional acceptance separate from build and
deployment.  It can execute deterministic local/CI checks and validate an
operator-supplied runtime or release record.  Records contain hashes and
stable identifiers only; command output is never written to a record.
"""

from __future__ import annotations

import argparse
import hashlib
import json
import os
import platform
import re
import shutil
import subprocess
import sys
import time
from datetime import datetime, timezone
from pathlib import Path
from typing import Any
from urllib.parse import quote
from urllib.request import Request, urlopen


ROOT = Path(__file__).resolve().parents[2]
MATRIX_PATH = ROOT / "contracts" / "film-tv" / "acceptance-matrix.json"
LAYERS = ("local", "ci", "runtime", "release")
EXECUTION_LAYERS = ("contract", *LAYERS)
STATUSES = ("not-accepted", "partial", "pass", "fail", "blocked")
RESULTS = ("pass", "fail", "not-run", "blocked")
SHA256_RE = re.compile(r"^[0-9a-f]{64}$")
COMMIT_RE = re.compile(r"^[0-9a-f]{40}$")
CI_BLOCKED_RUN_IDS = {"blocked-local-run", "blocked-ci-run"}
CI_REPOSITORY_RE = re.compile(r"^[^/\s]+/[^/\s]+$")
# These IDs are the minimum contract authority for this branch.  The matrix
# remains the detailed source of truth, while this small independent floor
# prevents a matrix-only edit from silently deleting an entire capability,
# check, or runtime/release scenario and still passing its own self-consistency
# checks.  Adding a new item is allowed; removing one requires changing this
# checker in the same reviewed change.
MINIMUM_CAPABILITY_IDS = frozenset(
    {
        "multi_source_search",
        "source_protocols",
        "work_identity",
        "playback_hls",
        "proxy_security",
        "subtitles",
        "playback_progress",
        "local_cache",
        "offline_download",
    }
)
MINIMUM_CHECK_IDS = frozenset(
    {
        "FTV-LCL-FIXTURE-SOURCE-001",
        "FTV-LCL-FE-FIXTURES-001",
        "FTV-LCL-FE-FOCUS-001",
        "FTV-LCL-FE-SUBTITLE-001",
        "FTV-LCL-RUST-FIXTURES-001",
        "FTV-LCL-RUST-SEARCH-001",
        "FTV-LCL-RUST-SOURCE-001",
        "FTV-LCL-COMMON-NETWORK-001",
        "FTV-LCL-RUST-IMPORT-001",
        "FTV-LCL-RUST-IMPORT-INTEGRATION-001",
        "FTV-LCL-RUST-STREAM-001",
        "FTV-LCL-RUST-PROGRESS-001",
        "FTV-LCL-RUST-DOWNLOAD-001",
        "FTV-LCL-TAURI-STREAM-001",
        "FTV-LCL-TAURI-RESOURCE-001",
        "FTV-LCL-TAURI-PROGRESS-001",
    }
)
MINIMUM_SCENARIO_IDS = frozenset(
    {
        "FTV-RUN-SEARCH-001",
        "FTV-RUN-SEARCH-002",
        "FTV-RUN-SEARCH-003",
        "FTV-RUN-SEARCH-004",
        "FTV-RUN-SOURCE-001",
        "FTV-RUN-SOURCE-002",
        "FTV-RUN-SOURCE-003",
        "FTV-RUN-SOURCE-004",
        "FTV-RUN-IDENTITY-001",
        "FTV-RUN-IDENTITY-002",
        "FTV-RUN-IDENTITY-003",
        "FTV-RUN-PLAY-001",
        "FTV-RUN-PLAY-002",
        "FTV-RUN-PLAY-003",
        "FTV-RUN-PLAY-004",
        "FTV-RUN-PLAY-005",
        "FTV-RUN-SEC-001",
        "FTV-RUN-SEC-002",
        "FTV-RUN-SEC-003",
        "FTV-RUN-SEC-004",
        "FTV-RUN-SEC-005",
        "FTV-RUN-SUB-001",
        "FTV-RUN-SUB-002",
        "FTV-RUN-SUB-003",
        "FTV-RUN-SUB-004",
        "FTV-RUN-PROGRESS-001",
        "FTV-RUN-PROGRESS-002",
        "FTV-RUN-PROGRESS-003",
        "FTV-RUN-PROGRESS-004",
        "FTV-RUN-PROGRESS-005",
        "FTV-RUN-CACHE-001",
        "FTV-RUN-CACHE-002",
        "FTV-RUN-CACHE-003",
        "FTV-RUN-CACHE-004",
        "FTV-RUN-DOWNLOAD-001",
        "FTV-RUN-DOWNLOAD-002",
        "FTV-RUN-DOWNLOAD-003",
        "FTV-RUN-DOWNLOAD-004",
        "FTV-RUN-DOWNLOAD-005",
        "FTV-REL-SEARCH-001",
        "FTV-REL-SOURCE-001",
        "FTV-REL-IDENTITY-001",
        "FTV-REL-PLAY-001",
        "FTV-REL-SEC-001",
        "FTV-REL-SUB-001",
        "FTV-REL-PROGRESS-001",
        "FTV-REL-CACHE-001",
        "FTV-REL-DOWNLOAD-001",
    }
)
SENSITIVE_KEY_RE = re.compile(
    r"(?:token|secret|password|cookie|authorization|credential|signed[_-]?url|raw[_-]?url|private[_-]?key)",
    re.IGNORECASE,
)
SENSITIVE_VALUE_PATTERNS = (
    re.compile(r"https?://", re.IGNORECASE),
    re.compile(r"file://", re.IGNORECASE),
    re.compile(r"[A-Za-z]:[\\/]"),
    re.compile(r"\\\\[^\\/]+[\\/][^\\/]+"),
    # Reject any POSIX absolute path, not only common system-directory
    # prefixes. Evidence records may be shared outside the developer machine,
    # so an arbitrary path such as /opt/app/private is still sensitive even
    # when it is not under /home, /tmp, or /workspace.
    re.compile(r"(?<![A-Za-z0-9._\-/])/(?!/)(?:[^\s/]+/)*[^\s]*"),
    re.compile(r"(?:^|\s)/(?:Users|home|var|tmp|private|mnt|workspace)(?:/|\s|$)", re.IGNORECASE),
)


class EvidenceError(ValueError):
    """A user-fixable evidence-contract error."""


def fail(message: str) -> None:
    raise EvidenceError(message)


def load_json(path: Path) -> dict[str, Any]:
    try:
        value = json.loads(path.read_text(encoding="utf-8"))
    except FileNotFoundError as error:
        fail(f"JSON 文件不存在: {path}")
    except json.JSONDecodeError as error:
        fail(f"JSON 无法解析 {path}:{error.lineno}:{error.colno}: {error.msg}")
    if not isinstance(value, dict):
        fail(f"JSON 根节点必须是对象: {path}")
    return value


def require_string(value: Any, name: str, *, non_empty: bool = True) -> str:
    if not isinstance(value, str) or (non_empty and not value.strip()):
        fail(f"{name} 必须是{'非空' if non_empty else ''}字符串")
    return value


def require_list(value: Any, name: str) -> list[Any]:
    if not isinstance(value, list):
        fail(f"{name} 必须是数组")
    return value


def require_object(value: Any, name: str) -> dict[str, Any]:
    if not isinstance(value, dict):
        fail(f"{name} 必须是对象")
    return value


def path_inside_root(relative: str, name: str, *, must_exist: bool = True) -> Path:
    candidate = (ROOT / relative).resolve()
    try:
        candidate.relative_to(ROOT)
    except ValueError:
        fail(f"{name} 必须位于仓库根目录内: {relative}")
    if must_exist and not candidate.exists():
        fail(f"{name} 不存在: {relative}")
    return candidate


def validate_matrix(matrix: dict[str, Any]) -> dict[str, Any]:
    if matrix.get("schemaVersion") != 1:
        fail("acceptance-matrix.schemaVersion 必须为 1")
    if matrix.get("feature") != "film-tv":
        fail("acceptance-matrix.feature 必须为 film-tv")

    completion = require_object(matrix.get("completionRule"), "completionRule")
    required_layers = completion.get("capabilityPassRequires")
    if required_layers != list(LAYERS):
        fail("completionRule.capabilityPassRequires 必须按 local/ci/runtime/release 完整列出")
    for field in ("buildIsNotAcceptanceFor", "deploymentIsNotAcceptanceFor"):
        if completion.get(field) != list(LAYERS):
            fail(f"completionRule.{field} 必须明确排除四层功能验收")

    layers = require_object(matrix.get("layers"), "layers")
    for layer in LAYERS:
        layer_value = require_object(layers.get(layer), f"layers.{layer}")
        require_string(layer_value.get("purpose"), f"layers.{layer}.purpose")
        require_string(layer_value.get("passRule"), f"layers.{layer}.passRule")
        require_string(layer_value.get("owner"), f"layers.{layer}.owner")
        require_string(layer_value.get("freshness"), f"layers.{layer}.freshness")

    capabilities = require_list(matrix.get("capabilities"), "capabilities")
    if not capabilities:
        fail("capabilities 不能为空；影视合同至少要保留一个受验收能力")
    capability_ids: set[str] = set()
    capability_by_id: dict[str, dict[str, Any]] = {}
    for index, capability in enumerate(capabilities):
        item = require_object(capability, f"capabilities[{index}]")
        capability_id = require_string(item.get("id"), f"capabilities[{index}].id")
        if capability_id in capability_ids:
            fail(f"能力 ID 重复: {capability_id}")
        capability_ids.add(capability_id)
        capability_by_id[capability_id] = item
        require_string(item.get("label"), f"capabilities[{index}].label")
        if item.get("acceptanceStatus") not in STATUSES:
            fail(f"{capability_id}.acceptanceStatus 不是允许的状态")
        required_checks = require_object(item.get("requiredChecks"), f"{capability_id}.requiredChecks")
        for layer in ("local", "ci"):
            ids = require_list(required_checks.get(layer), f"{capability_id}.requiredChecks.{layer}")
            for check_id in ids:
                require_string(check_id, f"{capability_id}.requiredChecks.{layer}[]")
        scenario_ids = require_list(item.get("requiredScenarios"), f"{capability_id}.requiredScenarios")
        for scenario_id in scenario_ids:
            require_string(scenario_id, f"{capability_id}.requiredScenarios[]")

    checks = require_list(matrix.get("checks"), "checks")
    check_ids: set[str] = set()
    check_by_id: dict[str, dict[str, Any]] = {}
    for index, check in enumerate(checks):
        item = require_object(check, f"checks[{index}]")
        check_id = require_string(item.get("id"), f"checks[{index}].id")
        if check_id in check_ids:
            fail(f"检查 ID 重复: {check_id}")
        check_ids.add(check_id)
        check_by_id[check_id] = item
        check_layers = require_list(item.get("layers"), f"{check_id}.layers")
        if not check_layers or any(layer not in ("local", "ci") for layer in check_layers):
            fail(f"{check_id}.layers 只能是 local/ci 的非空数组")
        if item.get("kind") != "command":
            fail(f"{check_id}.kind 必须为 command；计划项不能伪装成本地通过")
        cwd = require_string(item.get("cwd"), f"{check_id}.cwd", non_empty=False)
        path_inside_root(cwd or ".", f"{check_id}.cwd")
        argv = require_list(item.get("argv"), f"{check_id}.argv")
        if not argv or any(not isinstance(argument, str) or not argument for argument in argv):
            fail(f"{check_id}.argv 必须是非空字符串数组")
        require_string(item.get("purpose"), f"{check_id}.purpose")
        covered = require_list(item.get("covers"), f"{check_id}.covers")
        if not covered or any(capability_id not in capability_ids for capability_id in covered):
            fail(f"{check_id}.covers 引用了未知能力或为空")

    scenarios = require_list(matrix.get("scenarios"), "scenarios")
    scenario_ids: set[str] = set()
    scenario_by_id: dict[str, dict[str, Any]] = {}
    for index, scenario in enumerate(scenarios):
        item = require_object(scenario, f"scenarios[{index}]")
        scenario_id = require_string(item.get("id"), f"scenarios[{index}].id")
        if scenario_id in scenario_ids:
            fail(f"场景 ID 重复: {scenario_id}")
        scenario_ids.add(scenario_id)
        scenario_by_id[scenario_id] = item
        layer = item.get("layer")
        if layer not in ("runtime", "release"):
            fail(f"{scenario_id}.layer 必须为 runtime 或 release")
        capability_id = require_string(item.get("capability"), f"{scenario_id}.capability")
        if capability_id not in capability_ids:
            fail(f"{scenario_id}.capability 引用了未知能力")
        for field in ("name", "requiredObservation"):
            require_string(item.get(field), f"{scenario_id}.{field}")

    for capability_id, capability in capability_by_id.items():
        required_checks = capability["requiredChecks"]
        for layer in ("local", "ci"):
            for check_id in required_checks[layer]:
                if check_id not in check_ids:
                    fail(f"{capability_id} 引用了未知检查: {check_id}")
                if layer not in check_by_id[check_id]["layers"]:
                    fail(f"{capability_id} 的 {check_id} 未声明支持 {layer}")
        for scenario_id in capability["requiredScenarios"]:
            if scenario_id not in scenario_ids:
                fail(f"{capability_id} 引用了未知场景: {scenario_id}")
            scenario = scenario_by_id[scenario_id]
            if scenario["capability"] != capability_id:
                fail(f"{scenario_id} 的 capability 与 {capability_id} 不一致")

    missing_capabilities = sorted(MINIMUM_CAPABILITY_IDS - capability_ids)
    if missing_capabilities:
        fail(f"影视合同缺少受保护的能力: {missing_capabilities}")
    missing_checks = sorted(MINIMUM_CHECK_IDS - check_ids)
    if missing_checks:
        fail(f"影视合同缺少受保护的 local/CI 检查: {missing_checks}")
    missing_scenarios = sorted(MINIMUM_SCENARIO_IDS - scenario_ids)
    if missing_scenarios:
        fail(f"影视合同缺少受保护的 runtime/release 场景: {missing_scenarios}")

    matrix["_indexes"] = {
        "capabilityById": capability_by_id,
        "checkById": check_by_id,
        "scenarioById": scenario_by_id,
    }
    return matrix


def contains_sensitive_record_data(value: Any, location: str = "record") -> str | None:
    """Reject raw credentials, URLs and absolute paths in evidence records."""

    if isinstance(value, dict):
        for key, child in value.items():
            if SENSITIVE_KEY_RE.search(str(key)):
                return f"{location}.{key} 使用了禁止的敏感字段名"
            issue = contains_sensitive_record_data(child, f"{location}.{key}")
            if issue:
                return issue
    elif isinstance(value, list):
        for index, child in enumerate(value):
            issue = contains_sensitive_record_data(child, f"{location}[{index}]")
            if issue:
                return issue
    elif isinstance(value, str):
        for pattern in SENSITIVE_VALUE_PATTERNS:
            if pattern.search(value):
                return f"{location} 包含 URL、绝对路径或文件 URI；只允许摘要和哈希"
    return None


def canonical_record_bytes(record: dict[str, Any]) -> bytes:
    """Return stable bytes used for a record's content digest.

    recordSha256 is deliberately excluded so the digest is not recursive.
    The digest is over the complete structured record, not over a path or a
    human-readable rendering, which lets a release bundle verify references
    without storing sensitive file locations.
    """

    unsigned = dict(record)
    unsigned.pop("recordSha256", None)
    return json.dumps(
        unsigned,
        ensure_ascii=False,
        separators=(",", ":"),
        sort_keys=True,
    ).encode("utf-8")


def record_sha256(record: dict[str, Any]) -> str:
    return hashlib.sha256(canonical_record_bytes(record)).hexdigest()


def file_sha256(path: Path) -> str:
    digest = hashlib.sha256()
    try:
        with path.open("rb") as source:
            for chunk in iter(lambda: source.read(1024 * 1024), b""):
                digest.update(chunk)
    except OSError as error:
        fail(f"无法读取候选文件以计算 SHA-256: {error}")
    return digest.hexdigest()


def parse_rfc3339(value: str, name: str) -> None:
    try:
        datetime.fromisoformat(value.replace("Z", "+00:00"))
    except ValueError:
        fail(f"{name} 必须是 RFC3339 时间")


def validate_record(record: dict[str, Any], matrix: dict[str, Any], expected_layer: str | None) -> None:
    issue = contains_sensitive_record_data(record)
    if issue:
        fail(issue)
    if record.get("schemaVersion") != 1:
        fail("证据记录 schemaVersion 必须为 1")
    layer = record.get("layer")
    if layer not in LAYERS:
        fail("证据记录 layer 必须为 local/ci/runtime/release")
    if expected_layer and layer != expected_layer:
        fail(f"证据记录 layer={layer} 与要求的 {expected_layer} 不一致")
    record_digest = require_string(record.get("recordSha256"), "record.recordSha256")
    if not SHA256_RE.fullmatch(record_digest):
        fail("record.recordSha256 必须是 64 位 SHA-256")
    if record_digest != record_sha256(record):
        fail("record.recordSha256 与证据记录内容不匹配")
    capability = require_string(record.get("capability"), "record.capability")
    valid_capabilities = set(matrix["_indexes"]["capabilityById"])
    if capability != "all" and capability not in valid_capabilities:
        fail(f"证据记录引用了未知能力: {capability}")
    status = record.get("status")
    if status not in STATUSES:
        fail("证据记录 status 不是允许的状态")
    commit = require_string(record.get("commit"), "record.commit")
    if not COMMIT_RE.fullmatch(commit):
        fail("证据记录 commit 必须是 40 位 Git SHA")
    if layer in ("local", "ci"):
        worktree_sha = require_string(record.get("worktreeSha256"), "record.worktreeSha256")
        if not SHA256_RE.fullmatch(worktree_sha):
            fail("record.worktreeSha256 必须是 64 位 SHA-256")
    else:
        candidate_sha = require_string(record.get("candidateSha256"), "record.candidateSha256")
        if not SHA256_RE.fullmatch(candidate_sha):
            fail("record.candidateSha256 必须是 64 位 SHA-256")
    verified_at = require_string(record.get("verifiedAt"), "record.verifiedAt")
    parse_rfc3339(verified_at, "record.verifiedAt")
    environment = require_object(record.get("environment"), "record.environment")
    for field in ("os", "mode", "fixtureSet"):
        require_string(environment.get(field), f"record.environment.{field}")

    entries = require_list(record.get("checks"), "record.checks")
    seen: set[str] = set()
    known_checks = matrix["_indexes"]["checkById"]
    known_scenarios = matrix["_indexes"]["scenarioById"]
    for index, entry in enumerate(entries):
        item = require_object(entry, f"record.checks[{index}]")
        entry_id = require_string(item.get("id"), f"record.checks[{index}].id")
        if entry_id in seen:
            fail(f"证据记录检查 ID 重复: {entry_id}")
        seen.add(entry_id)
        if entry_id not in known_checks and entry_id not in known_scenarios:
            fail(f"证据记录引用未知检查/场景: {entry_id}")
        result = item.get("result")
        if result not in RESULTS:
            fail(f"{entry_id}.result 不是允许的结果")
        if result == "pass":
            digest = item.get("outputSha256") or item.get("observationSha256")
            if not isinstance(digest, str) or not SHA256_RE.fullmatch(digest):
                fail(f"{entry_id} 通过时必须有 64 位摘要哈希")

    if layer in ("local", "ci"):
        if capability != "all":
            fail(f"{layer} 记录必须使用 capability=all")
        expected_ids = {
            check_id
            for check_id, check in matrix["_indexes"]["checkById"].items()
            if layer in check["layers"]
        }
        if seen != expected_ids:
            missing = sorted(expected_ids - seen)
            extra = sorted(seen - expected_ids)
            fail(f"{layer} 记录检查集合不完整；missing={missing}, extra={extra}")
        if layer == "ci":
            ci_run_id = require_string(record.get("ciRunId"), "record.ciRunId")
            if ci_run_id in CI_BLOCKED_RUN_IDS and status == "blocked":
                require_string(record.get("blockedReason"), "record.blockedReason")
            elif not ci_run_id.isdigit() or int(ci_run_id) <= 0:
                fail("record.ciRunId 必须是正数 GitHub Actions run ID")
            else:
                if environment.get("mode") != "github-actions":
                    fail("有效 CI 记录必须使用 environment.mode=github-actions")
                repository = require_string(record.get("ciRepository"), "record.ciRepository")
                if CI_REPOSITORY_RE.fullmatch(repository) is None:
                    fail("record.ciRepository 必须是 owner/repository 形式")
                if record.get("ciRunVerified") is not True:
                    fail("有效 CI 记录必须有 ciRunVerified=true")
                ci_head_sha = require_string(record.get("ciHeadSha"), "record.ciHeadSha")
                if ci_head_sha != commit:
                    fail("record.ciHeadSha 必须与 record.commit 一致")
        if status == "pass" and any(entry.get("result") != "pass" for entry in entries):
            fail(f"{layer} status=pass 但存在非 pass 检查")
    else:
        if capability == "all":
            fail(f"{layer} 记录必须绑定一个具体能力")
        capability_record = matrix["_indexes"]["capabilityById"][capability]
        required_ids = {
            scenario_id
            for scenario_id in capability_record["requiredScenarios"]
            if matrix["_indexes"]["scenarioById"][scenario_id]["layer"] == layer
        }
        if seen != required_ids:
            missing = sorted(required_ids - seen)
            extra = sorted(seen - required_ids)
            fail(f"{layer} 记录场景集合不完整；missing={missing}, extra={extra}")
        if status == "pass" and any(entry.get("result") != "pass" for entry in entries):
            fail(f"{layer} status=pass 但存在非 pass 场景")

    if status == "pass" and any(entry.get("result") != "pass" for entry in entries):
        fail("status=pass 不能包含非 pass 检查")
    if status == "fail" and not any(entry.get("result") == "fail" for entry in entries):
        fail("status=fail 必须至少包含一个 fail 结果")
    if status in ("partial", "not-accepted") and entries and all(entry.get("result") == "pass" for entry in entries):
        fail(f"status={status} 不能把全量 pass 检查降级为非通过状态")
    if status == "blocked" and entries and not any(entry.get("result") in ("blocked", "not-run") for entry in entries):
        require_string(record.get("blockedReason"), "record.blockedReason")

    if layer == "release":
        evidence_refs = require_object(record.get("evidenceRefs"), "record.evidenceRefs")
        for ref_layer in ("local", "ci", "runtime"):
            reference = require_object(
                evidence_refs.get(ref_layer), f"record.evidenceRefs.{ref_layer}"
            )
            reference_digest = require_string(
                reference.get("recordSha256"),
                f"record.evidenceRefs.{ref_layer}.recordSha256",
            )
            if not SHA256_RE.fullmatch(reference_digest):
                fail(
                    f"record.evidenceRefs.{ref_layer}.recordSha256 必须是 64 位 SHA-256"
                )
        artifact_sha = require_string(record.get("artifactSha256"), "record.artifactSha256")
        if not SHA256_RE.fullmatch(artifact_sha):
            fail("record.artifactSha256 必须是 64 位 SHA-256")
        rollback = require_object(record.get("rollback"), "record.rollback")
        if rollback.get("available") is not True:
            fail("release 记录必须确认 rollback.available=true")
        if rollback.get("tested") is not True:
            fail("release 记录必须确认 rollback.tested=true；未演练回滚不能标记 release pass")
        if not isinstance(record.get("enabled"), bool):
            fail("record.enabled 必须明确记录是否启用")
        if status == "pass" and record.get("enabled") is not True:
            fail("release status=pass 必须明确 enabled=true；未启用候选只能是 partial/not-accepted")


def validate_bundle(
    matrix: dict[str, Any],
    *,
    local_path: Path,
    ci_path: Path,
    runtime_path: Path,
    release_path: Path,
    candidate_path: Path,
    artifact_path: Path,
) -> None:
    """Validate cross-record identity for a release acceptance bundle.

    Individual record validation protects shape and redaction. This second
    gate proves that the release record actually refers to the supplied local,
    CI and runtime records, and that both candidate and artifact hashes match
    bytes available to the operator. Paths are CLI inputs only and never enter
    a record.
    """

    local = load_json(local_path)
    ci = load_json(ci_path)
    runtime = load_json(runtime_path)
    release = load_json(release_path)
    validate_record(local, matrix, "local")
    validate_record(ci, matrix, "ci")
    validate_record(runtime, matrix, "runtime")
    validate_record(release, matrix, "release")

    if any(record.get("status") != "pass" for record in (local, ci, runtime, release)):
        fail("release bundle 的 local/ci/runtime/release 记录必须全部 status=pass")
    capability = release["capability"]
    if local["capability"] != "all" or ci["capability"] != "all":
        fail("release bundle 的 local/ci 记录必须使用 capability=all")
    if runtime["capability"] != capability:
        fail("release bundle 的 runtime 能力与 release 能力不一致")
    for name, record in (("local", local), ("ci", ci), ("runtime", runtime)):
        if record["commit"] != release["commit"]:
            fail(f"release bundle 的 {name} 记录 commit 与 release 不一致")
        expected_digest = release["evidenceRefs"][name]["recordSha256"]
        if expected_digest != record["recordSha256"]:
            fail(f"release bundle 的 {name} 记录摘要与 evidenceRefs 不一致")
    if local["worktreeSha256"] != ci["worktreeSha256"]:
        fail("release bundle 的 local/ci 工作树指纹不一致；不能拼接不同代码状态的证据")
    if runtime["candidateSha256"] != release["candidateSha256"]:
        fail("release bundle 的 runtime candidateSha256 与 release 不一致")
    if file_sha256(candidate_path) != release["candidateSha256"]:
        fail("候选对象文件 SHA-256 与 release.candidateSha256 不一致")
    if file_sha256(artifact_path) != release["artifactSha256"]:
        fail("候选制品文件 SHA-256 与 release.artifactSha256 不一致")


def now_rfc3339() -> str:
    return datetime.now(timezone.utc).isoformat(timespec="seconds").replace("+00:00", "Z")


def git_value(*args: str, default: str = "") -> str:
    try:
        completed = subprocess.run(
            ["git", *args],
            cwd=ROOT,
            check=True,
            capture_output=True,
            text=True,
            encoding="utf-8",
            errors="replace",
        )
    except (OSError, subprocess.CalledProcessError):
        return default
    return completed.stdout.strip()


def worktree_sha256() -> str:
    """Hash the exact dirty worktree subject without storing its contents.

    ``git rev-parse HEAD`` identifies only the last commit. Local work often
    includes staged/unstaged changes and newly created contract/source files,
    so include the binary diff from HEAD plus every non-ignored untracked file.
    The resulting digest is evidence metadata only; no path or file content is
    written to the record.
    """

    try:
        diff = subprocess.run(
            ["git", "diff", "--binary", "--no-ext-diff", "HEAD", "--"],
            cwd=ROOT,
            check=True,
            capture_output=True,
        ).stdout
        untracked = subprocess.run(
            ["git", "ls-files", "--others", "--exclude-standard", "-z"],
            cwd=ROOT,
            check=True,
            capture_output=True,
        ).stdout
    except (OSError, subprocess.CalledProcessError) as error:
        fail(f"无法计算工作树指纹: {error}")

    digest = hashlib.sha256()
    digest.update(b"git-diff-head\0")
    digest.update(diff)
    for raw_path in sorted(path for path in untracked.split(b"\0") if path):
        relative = os.fsdecode(raw_path)
        candidate = (ROOT / relative).resolve()
        try:
            candidate.relative_to(ROOT)
        except ValueError:
            fail(f"未跟踪文件越过仓库根目录，无法计算工作树指纹: {relative}")
        if not candidate.is_file():
            continue
        digest.update(b"untracked-path\0")
        digest.update(raw_path)
        digest.update(b"\0untracked-bytes\0")
        try:
            with candidate.open("rb") as source:
                for chunk in iter(lambda: source.read(1024 * 1024), b""):
                    digest.update(chunk)
        except OSError as error:
            fail(f"无法读取未跟踪文件以计算工作树指纹: {error}")
    return digest.hexdigest()


def redact_output(value: str) -> str:
    # Redact an entire header line/value first.  Consuming the whole value is
    # important for `Authorization: Bearer <secret>` and multi-value cookies;
    # replacing only the first token leaves the actual credential in logs.
    redacted = re.sub(
        r"(?i)(\b(?:authorization|proxy-authorization|cookie|set-cookie)\b\s*[:=]\s*)[^\r\n]*",
        r"\1<redacted>",
        value,
    )
    redacted = re.sub(
        r"(?i)(\b(?:token|secret|password|credential|api[-_]?key|access[-_]?token|refresh[-_]?token|private[-_]?key|signed[-_]?url)\b\s*[:=]\s*)(?:\"[^\"]*\"|'[^']*'|[^\s,;]+)",
        r"\1<redacted>",
        redacted,
    )
    redacted = re.sub(r"(?i)\bBearer\s+\S+", "Bearer <redacted>", redacted)
    redacted = re.sub(r"https?://\S+", "<url-redacted>", redacted, flags=re.IGNORECASE)
    redacted = re.sub(r"file://\S+", "<path-redacted>", redacted, flags=re.IGNORECASE)
    redacted = re.sub(r"[A-Za-z]:[\\/][^\r\n\s]*", "<path-redacted>", redacted)
    redacted = re.sub(r"\\\\[^\\/]+[\\/][^\r\n\s]*", "<path-redacted>", redacted)
    # Failure tails are still diagnostic material.  Hide arbitrary POSIX
    # absolute paths as well as the Windows forms above.
    redacted = re.sub(
        r"(?<![A-Za-z0-9._\-/])/(?!/)(?:[^\s/]+/)*[^\s]*",
        "<path-redacted>",
        redacted,
    )
    return redacted


def resolve_command(argv: list[str]) -> list[str]:
    executable = argv[0]
    resolved = shutil.which(executable)
    if resolved is None and os.name == "nt":
        resolved = shutil.which(f"{executable}.cmd") or shutil.which(f"{executable}.exe")
    if resolved is None:
        fail(f"找不到本地检查依赖: {executable}")
    return [resolved, *argv[1:]]


def run_command_check(check: dict[str, Any], timeout_seconds: int) -> dict[str, Any]:
    check_id = check["id"]
    command = resolve_command(list(check["argv"]))
    cwd = path_inside_root(check["cwd"] or ".", f"{check_id}.cwd")
    started = time.monotonic()
    timed_out = False
    try:
        completed = subprocess.run(
            command,
            cwd=cwd,
            capture_output=True,
            text=True,
            encoding="utf-8",
            errors="replace",
            timeout=timeout_seconds,
            check=False,
        )
        exit_code = completed.returncode
        output = f"{completed.stdout}\n{completed.stderr}"
    except subprocess.TimeoutExpired as error:
        timed_out = True
        exit_code = None
        output = f"{error.stdout or ''}\n{error.stderr or ''}"
    except OSError as error:
        exit_code = None
        output = str(error)
    duration_ms = int((time.monotonic() - started) * 1000)
    digest = hashlib.sha256(output.encode("utf-8", errors="replace")).hexdigest()
    result = "pass" if exit_code == 0 and not timed_out else "fail"
    print(f"[{result.upper()}] {check_id} ({duration_ms} ms)")
    if result == "fail":
        tail = "\n".join(redact_output(output).splitlines()[-8:]).strip()
        if tail:
            print(tail, file=sys.stderr)
        if timed_out:
            print(f"{check_id}: 超过 {timeout_seconds}s，已标记 fail", file=sys.stderr)
    return {
        "id": check_id,
        "result": result,
        "exitCode": exit_code,
        "durationMs": duration_ms,
        "outputSha256": digest,
        "timedOut": timed_out,
    }


def verify_github_actions_run(run_id: str, commit: str) -> str | None:
    """Verify the run identity with GitHub without persisting the token.

    ``GITHUB_ACTIONS`` is an environment hint, not an attestation.  The CI
    generator therefore requires a short-lived workflow token and checks the
    run belongs to the current repository and includes the checked-out commit
    (including the merge/head forms used for pull requests).
    """

    token = os.environ.get("GITHUB_TOKEN", "").strip()
    repository = os.environ.get("GITHUB_REPOSITORY", "").strip()
    if not token:
        return "GitHub Actions 上下文缺少 GITHUB_TOKEN；不能证明 run 来源"
    if CI_REPOSITORY_RE.fullmatch(repository) is None:
        return "GitHub Actions 上下文缺少有效 GITHUB_REPOSITORY"
    api_url = os.environ.get("GITHUB_API_URL", "https://api.github.com").strip().rstrip("/")
    # This repository is hosted on github.com.  Do not send the workflow token
    # to an arbitrary caller-provided HTTP endpoint just because it is stored
    # in GITHUB_API_URL; GHES support must add an explicit trusted host policy.
    if api_url != "https://api.github.com":
        return "GitHub Actions API 地址不是本仓库允许的 GitHub API"
    request_url = (
        f"{api_url}/repos/{quote(repository, safe='/')}/actions/runs/{int(run_id)}"
    )
    request = Request(
        request_url,
        headers={
            "Accept": "application/vnd.github+json",
            "Authorization": f"Bearer {token}",
            "X-GitHub-Api-Version": "2022-11-28",
            "User-Agent": "haven-film-tv-evidence-check",
        },
    )
    try:
        with urlopen(request, timeout=15) as response:
            if getattr(response, "status", 200) != 200:
                return "GitHub Actions API 未返回成功状态"
            payload = json.loads(response.read().decode("utf-8"))
    except (OSError, ValueError, json.JSONDecodeError):
        return "GitHub Actions API 查询失败；不能证明 run 来源"
    if not isinstance(payload, dict):
        return "GitHub Actions API 响应不是对象"
    if payload.get("id") != int(run_id):
        return "GitHub Actions API 返回的 run ID 不匹配"
    run_repository = payload.get("repository")
    if not isinstance(run_repository, dict) or run_repository.get("full_name") != repository:
        return "GitHub Actions run 不属于当前仓库"
    observed_shas: set[str] = set()
    for key in ("head_sha", "merge_commit_sha"):
        value = payload.get(key)
        if isinstance(value, str):
            observed_shas.add(value)
    workflow_sha = os.environ.get("GITHUB_SHA", "").strip()
    if workflow_sha:
        observed_shas.add(workflow_sha)
    pull_requests = payload.get("pull_requests")
    if isinstance(pull_requests, list):
        for pull_request in pull_requests:
            if not isinstance(pull_request, dict):
                continue
            for key in ("merge_commit_sha",):
                value = pull_request.get(key)
                if isinstance(value, str):
                    observed_shas.add(value)
            head = pull_request.get("head")
            if isinstance(head, dict) and isinstance(head.get("sha"), str):
                observed_shas.add(head["sha"])
    if commit not in observed_shas:
        return "GitHub Actions run 与当前检出的 commit 不匹配"
    return None


def resolve_ci_run_id(commit: str | None = None) -> tuple[str, str | None]:
    """Return a CI run identity only from an actual GitHub Actions context.

    A numeric ``GITHUB_RUN_ID`` is not sufficient evidence on a developer
    machine because environment variables can be supplied by the caller.  The
    workflow still needs to produce the final artifact, but this generation
    gate prevents a local invocation from writing a record that looks like a
    successful CI run.
    """

    if os.environ.get("GITHUB_ACTIONS", "").strip().lower() != "true":
        return (
            "blocked-local-run",
            "GITHUB_ACTIONS 不是 true；本地执行不能冒充 CI 证据",
        )
    ci_run_id = os.environ.get("GITHUB_RUN_ID", "").strip()
    if not ci_run_id.isdigit() or int(ci_run_id) <= 0:
        return (
            "blocked-ci-run",
            "GitHub Actions 上下文缺少有效 GITHUB_RUN_ID",
        )
    if commit is None:
        commit = git_value("rev-parse", "HEAD")
    verification_error = verify_github_actions_run(ci_run_id, commit)
    if verification_error:
        return ci_run_id, verification_error
    return ci_run_id, None


def write_record(path: Path, record: dict[str, Any]) -> None:
    if "recordSha256" not in record:
        record["recordSha256"] = record_sha256(record)
    issue = contains_sensitive_record_data(record)
    if issue:
        fail(f"生成证据记录前发现敏感数据: {issue}")
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(json.dumps(record, ensure_ascii=False, indent=2) + "\n", encoding="utf-8")
    print(f"证据摘要已写入: {path}")


def execute_layer(matrix: dict[str, Any], layer: str, output: Path | None, timeout_seconds: int) -> int:
    check_entries = [
        check
        for check in matrix["_indexes"]["checkById"].values()
        if layer in check["layers"]
    ]
    results: list[dict[str, Any]] = []
    for check in check_entries:
        try:
            result = run_command_check(check, timeout_seconds)
        except EvidenceError as error:
            result = {
                "id": check["id"],
                "result": "fail",
                "exitCode": None,
                "durationMs": 0,
                "outputSha256": hashlib.sha256(str(error).encode("utf-8")).hexdigest(),
                "timedOut": False,
            }
            print(f"[FAIL] {check['id']}: {error}", file=sys.stderr)
        results.append(result)

    all_pass = bool(results) and all(item["result"] == "pass" for item in results)
    status = "pass" if all_pass else "fail"
    commit = git_value("rev-parse", "HEAD")
    if not COMMIT_RE.fullmatch(commit):
        print("无法取得 40 位 HEAD SHA，记录标记为 fail", file=sys.stderr)
        status = "fail"
        commit = (commit or "0")[:40].ljust(40, "0")
    record: dict[str, Any] = {
        "schemaVersion": 1,
        "layer": layer,
        "capability": "all",
        "status": status,
        "verifiedAt": now_rfc3339(),
        "commit": commit,
        "worktreeSha256": worktree_sha256(),
        "environment": {
            "os": platform.platform(),
            "mode": "github-actions" if layer == "ci" else "developer-machine",
            "fixtureSet": "acceptance-matrix@1",
        },
        "checks": results,
        "generator": "tools/film-tv/evidence-check.py",
    }
    if layer == "ci":
        ci_run_id, blocked_reason = resolve_ci_run_id(commit)
        record["ciRunId"] = ci_run_id
        if blocked_reason:
            print(blocked_reason, file=sys.stderr)
            record["status"] = "blocked"
            record["blockedReason"] = blocked_reason
        else:
            record["ciRepository"] = os.environ.get("GITHUB_REPOSITORY", "").strip()
            record["ciHeadSha"] = commit
            record["ciRunVerified"] = True
    record["recordSha256"] = record_sha256(record)
    try:
        validate_record(record, matrix, layer)
    except EvidenceError as error:
        print(f"生成的 {layer} 记录未通过自身校验: {error}", file=sys.stderr)
        if layer == "ci" and record.get("ciRunId") in CI_BLOCKED_RUN_IDS:
            # Keep the record shape inspectable.  The blocked record cannot
            # validate as CI evidence outside a real GitHub Actions runner.
            record["status"] = "blocked"
            record["blockedReason"] = record.get(
                "blockedReason", "CI 运行上下文不可证明；不能冒充 CI 证据"
            )
        else:
            record["status"] = "fail"
    if output:
        write_record(output, record)
    print(f"{layer} evidence: {record['status'].upper()} ({len(results)} checks)")
    return 0 if record["status"] == "pass" else 1


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser(description="Haven 影视四层证据检查器")
    parser.add_argument("--layer", choices=EXECUTION_LAYERS, help="执行或校验的证据层")
    parser.add_argument("--output", type=Path, help="local/ci 执行结果的 JSON 输出路径")
    parser.add_argument("--validate-record", type=Path, help="校验已存在的 runtime/release 或 local/ci JSON 记录")
    parser.add_argument(
        "--validate-bundle",
        action="store_true",
        help="交叉校验 local/ci/runtime/release 记录及候选对象、候选制品",
    )
    parser.add_argument("--local-record", type=Path, help="bundle 中的 local 记录")
    parser.add_argument("--ci-record", type=Path, help="bundle 中的 ci 记录")
    parser.add_argument("--runtime-record", type=Path, help="bundle 中的 runtime 记录")
    parser.add_argument("--release-record", type=Path, help="bundle 中的 release 记录")
    parser.add_argument("--candidate-file", type=Path, help="runtime 验证的候选对象")
    parser.add_argument("--artifact-file", type=Path, help="release 验证的候选制品")
    parser.add_argument("--timeout-seconds", type=int, default=900, help="单项本地/CI 检查超时，默认 900 秒")
    return parser.parse_args()


def main() -> int:
    args = parse_args()
    try:
        matrix = validate_matrix(load_json(MATRIX_PATH))
        if args.timeout_seconds <= 0:
            fail("--timeout-seconds 必须为正数")
        if args.validate_bundle:
            bundle_paths = {
                "local": args.local_record,
                "ci": args.ci_record,
                "runtime": args.runtime_record,
                "release": args.release_record,
                "candidate": args.candidate_file,
                "artifact": args.artifact_file,
            }
            missing = [name for name, path in bundle_paths.items() if path is None]
            if missing:
                fail(f"--validate-bundle 缺少参数: {', '.join(missing)}")
            validate_bundle(
                matrix,
                local_path=args.local_record,
                ci_path=args.ci_record,
                runtime_path=args.runtime_record,
                release_path=args.release_record,
                candidate_path=args.candidate_file,
                artifact_path=args.artifact_file,
            )
            print("影视 release bundle 交叉校验通过")
            return 0
        if args.validate_record:
            if not args.validate_record.exists():
                fail(f"证据记录不存在: {args.validate_record}")
            record = load_json(args.validate_record)
            validate_record(record, matrix, args.layer)
            print(f"证据记录校验通过: layer={record['layer']} status={record['status']}")
            return 0
        if args.output and args.layer not in ("local", "ci"):
            fail("--output 只用于生成 local/ci 执行摘要")
        if args.layer == "runtime" or args.layer == "release":
            fail("runtime/release 必须使用 --validate-record 校验人工/桌面验收记录")
        if args.layer == "local" or args.layer == "ci":
            return execute_layer(matrix, args.layer, args.output, args.timeout_seconds)
        print(
            f"film-tv evidence contract PASS: {len(matrix['_indexes']['capabilityById'])} capabilities, "
            f"{len(matrix['_indexes']['checkById'])} local/CI checks, "
            f"{len(matrix['_indexes']['scenarioById'])} runtime/release scenarios"
        )
        return 0
    except EvidenceError as error:
        print(f"film-tv evidence contract FAIL: {error}", file=sys.stderr)
        return 1


if __name__ == "__main__":
    raise SystemExit(main())
