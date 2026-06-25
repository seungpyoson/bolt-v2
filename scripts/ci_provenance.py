#!/usr/bin/env python3
"""Emit and resolve CI provenance evidence."""

from __future__ import annotations

import argparse
import dataclasses
import datetime
import hashlib
import io
import json
import os
import pathlib
import re
import sys
import tomllib
import urllib.error
import urllib.parse
import urllib.request
import zipfile


REPO_ROOT = pathlib.Path(__file__).resolve().parents[1]
DEFAULT_CONFIG = REPO_ROOT / "ci" / "github-actions-runners.toml"
SUPPORTED_MODES = {
    "check-backtester-gate",
    "check-ci-gate",
    "ci-policy",
    "emit-full-ci",
    "resolve-gate-carry-forward",
    "resolve-exact-sha",
    "resolve-fingerprint",
    "validate-record",
}
POLICY_VALUES = {"full", "docs", "defer", "iteration", "noop", "tag_reuse"}
GATE_NAME_KEYS = (
    "gate_required",
    "gate_iteration",
    "gate_dispatch_full",
    "backtester_required",
    "backtester_iteration",
    "backtester_dispatch_full",
)
POLICY_ROWS = (
    "draft_pr_synchronize",
    "draft_pr_opened",
    "draft_pr_reopened",
    "draft_pr_edited",
    "converted_to_draft",
    "ready_pr",
    "ready_pr_edited_no_base",
    "ready_pr_reopened",
    "ready_for_review",
    "docs",
    "workflow_dispatch",
    "workflow_dispatch_full_ci",
    "main_push",
    "merge_group",
    "mergify_temp_pr",
    "tag",
    "unknown_event",
)
POLICY_REQUIRED_VALUES = {
    "draft_pr_synchronize": "defer",
    "draft_pr_opened": "defer",
    "draft_pr_reopened": "defer",
    "draft_pr_edited": "defer",
    "converted_to_draft": "defer",
    "ready_pr": "full",
    "ready_for_review": "full",
    "docs": "docs",
    "workflow_dispatch": "iteration",
    "workflow_dispatch_full_ci": "full",
    "main_push": "full",
    "merge_group": "full",
    "mergify_temp_pr": "full",
    "tag": "tag_reuse",
    "unknown_event": "full",
}
POLICY_REQUIRED_MESSAGES = {
    "workflow_dispatch_full_ci": "ci_provenance.policy.workflow_dispatch_full_ci must remain full",
}
POLICY_ALLOWED_VALUES = {
    "ready_pr_edited_no_base": {"noop", "full"},
    "ready_pr_reopened": {"noop", "full"},
}
FORBIDDEN_DOCS_SAFE_PATH_PATTERNS = frozenset({"docs/**", "specs/**"})
SHA_RE = re.compile(r"^[0-9a-f]{40}$")
DIGEST_RE = re.compile(r"^[0-9a-f]{64}$")
NEXTEST_FINGERPRINT_RE = re.compile(
    r"^nextest-archive-v(?P<schema>[1-9][0-9]*)-"
    r"(?P<os>[A-Za-z0-9_.-]+)-"
    r"(?P<arch>[A-Za-z0-9_.-]+)-"
    r"(?P<profile>[A-Za-z0-9_.-]+)-profile-shards-"
    r"(?P<shards>[1-9][0-9]*)-"
    r"(?P<digest>[0-9a-f]{64})$"
)
GITHUB_API_HEADERS = {
    "Accept": "application/vnd.github+json",
    "X-GitHub-Api-Version": "2022-11-28",
}
GITHUB_API_REDIRECT_HEADERS = {"authorization", "accept", "x-github-api-version"}


class ProvenanceError(RuntimeError):
    """Raised when provenance evidence is absent, malformed, or unsafe."""


@dataclasses.dataclass(frozen=True)
class JobConfig:
    logical_name: str
    check_name: str | None
    check_name_template: str | None
    shard_count: int | None
    conditional: str | None


@dataclasses.dataclass(frozen=True)
class ProvenanceConfig:
    schema_version: int
    artifact_name_template: str
    workflow_key: str
    workflow_name: str
    workflow_path: str
    fingerprint_source: str
    fingerprint_artifact_prefix: str
    fingerprint_workflow: str
    required_jobs: tuple[str, ...]
    conditional_jobs: tuple[str, ...]
    conditional_job_outputs: dict[str, str]
    jobs: dict[str, JobConfig]
    deploy_artifact_name: str
    deploy_source_event: str
    deploy_source_branch: str
    deploy_require_gate_check: bool
    dispatch_workflow_input: str
    dispatch_run_name_default: str
    dispatch_run_name_full: str
    dispatch_run_name_iteration: str
    dispatch_proof_gate_job: str
    workflow_runs_per_page: int
    run_jobs_per_page: int
    run_artifacts_per_page: int
    max_lookback_pages: int
    max_lookback_age_seconds: int
    artifact_retention_days: int
    policy: dict[str, str]
    gate_names: dict[str, str]
    mergify_temp_pr_head_ref_prefix: str
    docs_safe_paths: tuple[str, ...]
    docs_forbidden_ignored_build_paths: tuple[str, ...]
    docs_non_heavy_required_jobs: tuple[str, ...]
    force_full_ci: bool
    ignore_emit_failure: bool


@dataclasses.dataclass(frozen=True)
class CiPolicyResult:
    ci_policy_path: str
    full_ci_required: bool
    full_ci_deferred: bool
    gate_name: str
    backtester_gate_name: str
    expected_event_class: str
    reason: str


@dataclasses.dataclass(frozen=True)
class ResolvedEvidence:
    run: dict[str, object]
    artifact: dict[str, object]
    record: dict[str, object]


@dataclasses.dataclass(frozen=True)
class FingerprintReuseResolution:
    reuse_found: bool
    source_run_id: str
    source_sha: str
    source_artifact_id: str
    reason: str


@dataclasses.dataclass(frozen=True)
class GateCarryForwardResolution:
    carry_forward_verified: bool
    source_run_id: str
    source_sha: str
    gate_name: str
    reason: str


def require_table(parent: dict[str, object], key: str, prefix: str) -> dict[str, object]:
    value = parent.get(key)
    if not isinstance(value, dict):
        raise ProvenanceError(f"{prefix}.{key} must be a table")
    return value


def require_string(parent: dict[str, object], key: str, prefix: str) -> str:
    value = parent.get(key)
    if not isinstance(value, str) or not value:
        raise ProvenanceError(f"{prefix}.{key} must be a non-empty string")
    return value


def github_actions_output_safe_check_name(value: str) -> bool:
    return (
        value == value.strip()
        and "${{" not in value
        and "}}" not in value
        and all(char not in "\r\n" and 32 <= ord(char) < 127 for char in value)
    )


def require_gate_name(parent: dict[str, object], key: str, prefix: str) -> str:
    value = require_string(parent, key, prefix)
    if not github_actions_output_safe_check_name(value):
        raise ProvenanceError(f"{prefix}.{key} must be a GitHub Actions output-safe check name")
    return value


def gate_name_collision_errors(gate_names: dict[str, str]) -> list[str]:
    errors: list[str] = []
    keys = (
        "gate_required",
        "backtester_required",
        "gate_iteration",
        "backtester_iteration",
        "gate_dispatch_full",
        "backtester_dispatch_full",
    )
    seen: dict[str, str] = {}
    for key in keys:
        value = gate_names.get(key)
        if value is None:
            continue
        previous = seen.get(value)
        if previous is not None:
            errors.append(f"ci_provenance.gate_names.{key} must not equal {previous}")
        else:
            seen[value] = key
    return errors


def require_positive_int(parent: dict[str, object], key: str, prefix: str) -> int:
    value = parent.get(key)
    if not isinstance(value, int) or value <= 0:
        raise ProvenanceError(f"{prefix}.{key} must be a positive integer")
    return value


def require_string_list(parent: dict[str, object], key: str, prefix: str) -> tuple[str, ...]:
    value = parent.get(key)
    if not isinstance(value, list) or not all(isinstance(item, str) and item for item in value):
        raise ProvenanceError(f"{prefix}.{key} must be a non-empty string list")
    return tuple(value)


def policy_contract_errors(policy: dict[str, object]) -> list[str]:
    errors: list[str] = []
    missing_contract = sorted(
        set(POLICY_ROWS) - set(POLICY_REQUIRED_VALUES) - set(POLICY_ALLOWED_VALUES)
    )
    if missing_contract:
        errors.append(
            "ci_provenance.policy rows must define required or allowed contract: "
            + ", ".join(missing_contract)
        )
    for row, expected in POLICY_REQUIRED_VALUES.items():
        if policy.get(row) != expected:
            errors.append(
                POLICY_REQUIRED_MESSAGES.get(
                    row,
                    f"ci_provenance.policy.{row} must be {expected}",
                )
            )
    for row, allowed in POLICY_ALLOWED_VALUES.items():
        if policy.get(row) not in allowed:
            errors.append(
                f"ci_provenance.policy.{row} must be one of {sorted(allowed)!r}"
            )
    return errors


def docs_safe_path_contract_errors(safe_paths: tuple[str, ...]) -> list[str]:
    errors: list[str] = []
    for forbidden in sorted(FORBIDDEN_DOCS_SAFE_PATH_PATTERNS):
        if forbidden in safe_paths:
            errors.append(
                f"ci_provenance.docs.safe_paths must not include build-input path {forbidden}"
            )
    if len(set(safe_paths)) != len(safe_paths):
        errors.append("ci_provenance.docs.safe_paths must not contain duplicates")
    return errors


def load_toml(path: pathlib.Path) -> dict[str, object]:
    try:
        return tomllib.loads(path.read_text(encoding="utf-8"))
    except FileNotFoundError as exc:
        raise ProvenanceError(f"config missing: {path}") from exc
    except tomllib.TOMLDecodeError as exc:
        raise ProvenanceError(f"config is invalid TOML: {exc}") from exc
    except OSError as exc:
        raise ProvenanceError(f"config could not be read: {exc}") from exc


def canonical_json_value(value: object) -> object:
    if isinstance(value, dict):
        return {
            key: canonical_json_value(value[key])
            for key in sorted(value)
            if isinstance(key, str)
        }
    if isinstance(value, list):
        return [canonical_json_value(item) for item in value]
    return value


def provenance_config_payload(path: pathlib.Path = DEFAULT_CONFIG) -> dict[str, object]:
    data = load_toml(path)
    ci_provenance = data.get("ci_provenance")
    meter = data.get("meter")
    if not isinstance(ci_provenance, dict):
        raise ProvenanceError("missing [ci_provenance]")
    if not isinstance(meter, dict):
        raise ProvenanceError("missing [meter]")
    fingerprint_source = ci_provenance.get("fingerprint_source")
    if fingerprint_source != "meter":
        raise ProvenanceError("ci_provenance.fingerprint_source must be meter")
    return {
        "ci_provenance": canonical_json_value(ci_provenance),
        "meter": canonical_json_value(
            {
                "fingerprint_artifact_prefix": meter.get("fingerprint_artifact_prefix"),
                "fingerprint_workflow": meter.get("fingerprint_workflow"),
            }
        ),
    }


def provenance_config_digest(path: pathlib.Path = DEFAULT_CONFIG) -> str:
    payload = provenance_config_payload(path)
    encoded = json.dumps(payload, sort_keys=True, separators=(",", ":")).encode("utf-8")
    return hashlib.sha256(encoded).hexdigest()


def load_config(path: pathlib.Path = DEFAULT_CONFIG) -> ProvenanceConfig:
    data = load_toml(path)
    meter = data.get("meter")
    if not isinstance(meter, dict):
        raise ProvenanceError("missing [meter]")
    ci_provenance = data.get("ci_provenance")
    if not isinstance(ci_provenance, dict):
        raise ProvenanceError("missing [ci_provenance]")
    if ci_provenance.get("schema_version") != 1:
        raise ProvenanceError("ci_provenance.schema_version must be 1")

    duplicated_fingerprint_keys = {
        "fingerprint_artifact_prefix",
        "fingerprint_workflow",
    } & set(ci_provenance)
    if duplicated_fingerprint_keys:
        names = ", ".join(sorted(duplicated_fingerprint_keys))
        raise ProvenanceError(f"[ci_provenance] must reference [meter] fingerprint keys, duplicated {names}")

    artifact_name_template = require_string(
        ci_provenance, "artifact_name_template", "ci_provenance"
    )
    if "{run_attempt}" not in artifact_name_template:
        raise ProvenanceError("ci_provenance.artifact_name_template must include {run_attempt}")

    full_ci = require_table(ci_provenance, "full_ci", "ci_provenance")
    required_jobs = require_string_list(full_ci, "required_jobs", "ci_provenance.full_ci")
    conditional_jobs = require_string_list(
        full_ci, "conditional_jobs", "ci_provenance.full_ci"
    )
    conditional_job_outputs = full_ci.get("conditional_job_outputs")
    if not isinstance(conditional_job_outputs, dict) or not all(
        isinstance(key, str) and isinstance(value, str)
        for key, value in conditional_job_outputs.items()
    ):
        raise ProvenanceError("ci_provenance.full_ci.conditional_job_outputs must map strings")

    job_tables = require_table(full_ci, "jobs", "ci_provenance.full_ci")
    jobs: dict[str, JobConfig] = {}
    for job in (*required_jobs, *conditional_jobs):
        job_table = job_tables.get(job)
        if not isinstance(job_table, dict):
            raise ProvenanceError(f"ci_provenance.full_ci.jobs.{job} missing")
        check_name = job_table.get("check_name")
        check_name_template = job_table.get("check_name_template")
        if check_name is not None and (not isinstance(check_name, str) or not check_name):
            raise ProvenanceError(f"ci_provenance.full_ci.jobs.{job}.check_name must be a non-empty string")
        if check_name_template is not None and (
            not isinstance(check_name_template, str) or not check_name_template
        ):
            raise ProvenanceError(
                f"ci_provenance.full_ci.jobs.{job}.check_name_template must be a non-empty string"
            )
        if check_name is None and check_name_template is None:
            raise ProvenanceError(
                f"ci_provenance.full_ci.jobs.{job} must define check_name or check_name_template"
            )
        shard_count = job_table.get("shard_count")
        if check_name_template is not None:
            if not isinstance(shard_count, int) or shard_count <= 0:
                raise ProvenanceError(f"ci_provenance.full_ci.jobs.{job}.shard_count must be a positive integer")
            if "{shard}" not in check_name_template:
                raise ProvenanceError(
                    f"ci_provenance.full_ci.jobs.{job}.check_name_template must include {{shard}}"
                )
        elif shard_count is not None:
            raise ProvenanceError(f"ci_provenance.full_ci.jobs.{job}.shard_count requires check_name_template")
        conditional = job_table.get("conditional")
        if conditional is not None and (not isinstance(conditional, str) or not conditional):
            raise ProvenanceError(f"ci_provenance.full_ci.jobs.{job}.conditional must be a non-empty string")
        if job in conditional_jobs and conditional != conditional_job_outputs.get(job):
            raise ProvenanceError(f"ci_provenance.full_ci.jobs.{job}.conditional must match conditional_job_outputs")
        jobs[job] = JobConfig(
            logical_name=job,
            check_name=check_name,
            check_name_template=check_name_template,
            shard_count=shard_count if isinstance(shard_count, int) else None,
            conditional=conditional,
        )

    deploy = require_table(ci_provenance, "deploy", "ci_provenance")
    dispatch = require_table(ci_provenance, "dispatch", "ci_provenance")
    gate_names_table = require_table(ci_provenance, "gate_names", "ci_provenance")
    docs_table = require_table(ci_provenance, "docs", "ci_provenance")
    mergify = require_table(ci_provenance, "mergify", "ci_provenance")
    api_limits = require_table(ci_provenance, "api_limits", "ci_provenance")
    artifacts = require_table(ci_provenance, "artifacts", "ci_provenance")
    policy_table = require_table(ci_provenance, "policy", "ci_provenance")
    overrides = require_table(policy_table, "override", "ci_provenance.policy")

    retention_days = require_positive_int(artifacts, "retention_days", "ci_provenance.artifacts")
    max_lookback_age_seconds = require_positive_int(
        api_limits, "max_lookback_age_seconds", "ci_provenance.api_limits"
    )
    if max_lookback_age_seconds > retention_days * 24 * 60 * 60:
        raise ProvenanceError("max lookback age must not exceed artifact retention")

    unexpected_policy_keys = sorted(set(policy_table) - set(POLICY_ROWS) - {"override"})
    if unexpected_policy_keys:
        raise ProvenanceError(f"ci_provenance.policy has unexpected keys: {unexpected_policy_keys!r}")

    policy: dict[str, str] = {}
    for row in POLICY_ROWS:
        value = policy_table.get(row)
        if value not in POLICY_VALUES:
            raise ProvenanceError(
                f"ci_provenance.policy.{row} must be full, docs, defer, iteration, noop, or tag_reuse"
            )
        policy[row] = value
    contract_errors = policy_contract_errors(policy)
    if contract_errors:
        raise ProvenanceError("; ".join(contract_errors))

    dispatch_run_name_default = require_string(dispatch, "run_name_default", "ci_provenance.dispatch")
    dispatch_run_name_full = require_string(dispatch, "run_name_full", "ci_provenance.dispatch")
    dispatch_run_name_iteration = require_string(dispatch, "run_name_iteration", "ci_provenance.dispatch")
    dispatch_proof_gate_job = require_string(dispatch, "proof_gate_job", "ci_provenance.dispatch")
    if dispatch_run_name_full == dispatch_run_name_iteration:
        raise ProvenanceError("ci_provenance.dispatch run_name_full and run_name_iteration must differ")

    gate_names = {
        key: require_gate_name(gate_names_table, key, "ci_provenance.gate_names")
        for key in GATE_NAME_KEYS
    }
    if dispatch_proof_gate_job != gate_names["gate_required"]:
        raise ProvenanceError("ci_provenance.dispatch.proof_gate_job must match required gate name")
    gate_name_errors = gate_name_collision_errors(gate_names)
    if gate_name_errors:
        raise ProvenanceError("; ".join(gate_name_errors))

    docs_safe_paths = require_string_list(docs_table, "safe_paths", "ci_provenance.docs")
    docs_forbidden_ignored_build_paths = require_string_list(
        docs_table,
        "forbidden_ignored_build_paths",
        "ci_provenance.docs",
    )
    docs_non_heavy_required_jobs = require_string_list(
        docs_table,
        "non_heavy_required_jobs",
        "ci_provenance.docs",
    )
    docs_path_errors = docs_safe_path_contract_errors(docs_safe_paths)
    if docs_path_errors:
        raise ProvenanceError("; ".join(docs_path_errors))
    unknown_non_heavy = sorted(set(docs_non_heavy_required_jobs) - set(required_jobs))
    if unknown_non_heavy:
        raise ProvenanceError(
            "ci_provenance.docs.non_heavy_required_jobs must be configured full-CI jobs: "
            + ", ".join(unknown_non_heavy)
        )

    force_full_ci = overrides.get("force_full_ci")
    ignore_emit_failure = overrides.get("ignore_emit_failure")
    if not isinstance(force_full_ci, bool):
        raise ProvenanceError("ci_provenance.policy.override.force_full_ci must be boolean")
    if not isinstance(ignore_emit_failure, bool):
        raise ProvenanceError("ci_provenance.policy.override.ignore_emit_failure must be boolean")

    return ProvenanceConfig(
        schema_version=1,
        artifact_name_template=artifact_name_template,
        workflow_key=require_string(ci_provenance, "workflow_key", "ci_provenance"),
        workflow_name=require_string(ci_provenance, "workflow_name", "ci_provenance"),
        workflow_path=require_string(ci_provenance, "workflow_path", "ci_provenance"),
        fingerprint_source=require_string(ci_provenance, "fingerprint_source", "ci_provenance"),
        fingerprint_artifact_prefix=require_string(meter, "fingerprint_artifact_prefix", "meter"),
        fingerprint_workflow=require_string(meter, "fingerprint_workflow", "meter"),
        required_jobs=required_jobs,
        conditional_jobs=conditional_jobs,
        conditional_job_outputs=dict(conditional_job_outputs),
        jobs=jobs,
        deploy_artifact_name=require_string(deploy, "artifact_name", "ci_provenance.deploy"),
        deploy_source_event=require_string(deploy, "require_source_event", "ci_provenance.deploy"),
        deploy_source_branch=require_string(deploy, "require_source_branch", "ci_provenance.deploy"),
        deploy_require_gate_check=deploy.get("require_gate_check") is True,
        dispatch_workflow_input=require_string(dispatch, "workflow_input", "ci_provenance.dispatch"),
        dispatch_run_name_default=dispatch_run_name_default,
        dispatch_run_name_full=dispatch_run_name_full,
        dispatch_run_name_iteration=dispatch_run_name_iteration,
        dispatch_proof_gate_job=dispatch_proof_gate_job,
        workflow_runs_per_page=require_positive_int(
            api_limits, "workflow_runs_per_page", "ci_provenance.api_limits"
        ),
        run_jobs_per_page=require_positive_int(
            api_limits, "run_jobs_per_page", "ci_provenance.api_limits"
        ),
        run_artifacts_per_page=require_positive_int(
            api_limits, "run_artifacts_per_page", "ci_provenance.api_limits"
        ),
        max_lookback_pages=require_positive_int(
            api_limits, "max_lookback_pages", "ci_provenance.api_limits"
        ),
        max_lookback_age_seconds=max_lookback_age_seconds,
        artifact_retention_days=retention_days,
        policy=policy,
        gate_names=gate_names,
        mergify_temp_pr_head_ref_prefix=require_string(
            mergify, "temp_pr_head_ref_prefix", "ci_provenance.mergify"
        ),
        docs_safe_paths=docs_safe_paths,
        docs_forbidden_ignored_build_paths=docs_forbidden_ignored_build_paths,
        docs_non_heavy_required_jobs=docs_non_heavy_required_jobs,
        force_full_ci=force_full_ci,
        ignore_emit_failure=ignore_emit_failure,
    )


def parse_bool(value: str) -> bool:
    normalized = value.strip().lower()
    if normalized == "true":
        return True
    if normalized == "false":
        return False
    raise ProvenanceError(f"expected boolean true or false, got {value!r}")


def require_job_result(
    job_results: dict[str, str],
    job: str,
    expected: str,
    message: str | None = None,
) -> None:
    actual = job_results.get(job)
    if actual != expected:
        raise ProvenanceError(message or f"{job} did not resolve {expected}: {actual}")


def require_job_result_in(
    job_results: dict[str, str],
    job: str,
    expected: set[str],
    message: str | None = None,
) -> None:
    actual = job_results.get(job)
    if actual not in expected:
        expected_text = ", ".join(sorted(expected))
        raise ProvenanceError(message or f"{job} did not resolve one of {expected_text}: {actual}")


def require_jobs_skipped(job_results: dict[str, str], jobs: tuple[str, ...], label: str) -> None:
    for job in jobs:
        actual = job_results.get(job)
        if actual != "skipped":
            if actual is None:
                raise ProvenanceError(f"{job} missing or not skipped during {label}: {actual}")
            raise ProvenanceError(f"{job} unexpectedly ran during {label}: {actual}")


CI_HEAVY_JOBS = (
    "deny",
    "clippy",
    "check-aarch64",
    "source-fence",
    "nextest-fingerprint",
    "test-archive",
    "nextest-fingerprint-reuse",
    "test",
    "build",
)


def require_verified_carry_forward(carry_forward_verified: bool) -> None:
    if not carry_forward_verified:
        raise ProvenanceError("verified carry-forward proof is required for this policy path")


def evaluate_ci_gate_verdict(
    *,
    policy_path: str,
    expected_event_class: str,
    full_ci_deferred: bool,
    ignore_emit_failure: bool,
    reuse_found: bool,
    carry_forward_verified: bool,
    job_results: dict[str, str],
    build_required: bool,
) -> str:
    require_job_result(job_results, "ci-policy", "success", "ci-policy did not succeed")
    require_job_result(job_results, "detector", "success", "detector did not succeed")
    if full_ci_deferred != (policy_path == "defer"):
        raise ProvenanceError("full_ci_deferred must match policy_path == 'defer'")
    if ignore_emit_failure:
        raise ProvenanceError("ignore_emit_failure cannot satisfy the required gate")

    if policy_path == "tag_reuse":
        if expected_event_class != "tag_reuse":
            raise ProvenanceError(f"tag reuse CI policy outside resolver-permitted event class {expected_event_class!r}")
        require_job_result(job_results, "same-sha-main-evidence", "success", "same-sha-main-evidence did not succeed")
        require_jobs_skipped(
            job_results,
            (
                "deny",
                "clippy",
                "source-fence",
                "nextest-fingerprint",
                "test-archive",
                "nextest-fingerprint-reuse",
                "test",
                "build",
                "ci-provenance-emit",
            ),
            "tag reuse",
        )
        require_job_result(job_results, "check-aarch64", "success", "check-aarch64 did not succeed during tag reuse")
        return "tag reuse proof passed"

    require_job_result(
        job_results,
        "same-sha-main-evidence",
        "skipped",
        "same-sha-main-evidence unexpectedly ran outside tag reuse",
    )

    if policy_path == "iteration":
        if expected_event_class != "iteration":
            raise ProvenanceError(f"iteration CI policy outside resolver-permitted event class {expected_event_class!r}")
        require_jobs_skipped(job_results, (*CI_HEAVY_JOBS, "ci-provenance-emit"), "iteration")
        return "iteration CI policy; no required full proof published by this run"

    if policy_path == "defer" or full_ci_deferred:
        if expected_event_class != "defer":
            raise ProvenanceError(f"deferred CI policy outside resolver-permitted event class {expected_event_class!r}")
        require_jobs_skipped(job_results, (*CI_HEAVY_JOBS, "ci-provenance-emit"), "defer")
        require_verified_carry_forward(carry_forward_verified)
        return "deferred CI policy carried forward prior same-SHA gate proof"

    if policy_path == "noop":
        if expected_event_class != "noop":
            raise ProvenanceError(f"noop CI policy outside resolver-permitted event class {expected_event_class!r}")
        require_jobs_skipped(job_results, (*CI_HEAVY_JOBS, "ci-provenance-emit"), "noop")
        require_verified_carry_forward(carry_forward_verified)
        return "no-code CI policy carried forward prior same-SHA gate proof"

    if policy_path == "docs":
        if expected_event_class != "docs":
            raise ProvenanceError(f"docs CI policy outside resolver-permitted event class {expected_event_class!r}")
        require_jobs_skipped(job_results, CI_HEAVY_JOBS, "docs")
        require_job_result(job_results, "ci-provenance-emit", "success", "ci-provenance-emit did not succeed for docs")
        return "docs CI proof passed"

    if policy_path != "full":
        raise ProvenanceError(f"unknown CI policy path {policy_path!r}")
    if expected_event_class != "full":
        raise ProvenanceError(f"full CI policy outside resolver-permitted event class {expected_event_class!r}")

    if reuse_found:
        require_job_result(
            job_results,
            "nextest-fingerprint",
            "success",
            "nextest fingerprint did not succeed during reuse",
        )
        require_job_result(
            job_results,
            "nextest-fingerprint-reuse",
            "success",
            "nextest fingerprint reuse resolver did not succeed",
        )
        require_job_result(
            job_results,
            "test-archive",
            "skipped",
            "test-archive unexpectedly ran during nextest fingerprint reuse",
        )
        require_job_result(
            job_results,
            "ci-provenance-emit",
            "skipped",
            "ci-provenance-emit unexpectedly ran during nextest fingerprint reuse",
        )
    else:
        emit_result = job_results.get("ci-provenance-emit")
        if emit_result != "success" and not ignore_emit_failure:
            raise ProvenanceError("ci-provenance-emit did not succeed")
        require_job_result(
            job_results,
            "nextest-fingerprint",
            "success",
            "nextest fingerprint did not succeed",
        )
        require_job_result(job_results, "test-archive", "success", "test-archive did not succeed")

    for job in ("deny", "clippy", "check-aarch64", "source-fence", "test"):
        require_job_result(job_results, job, "success", f"{job} did not succeed")
    if build_required:
        require_job_result(job_results, "build", "success", "build did not succeed when build_required=true")
    else:
        require_job_result_in(
            job_results,
            "build",
            {"success", "skipped"},
            f"build produced unexpected result {job_results.get('build')!r} when build_required=false",
        )
    return "full CI proof passed"


def evaluate_backtester_gate_verdict(
    *,
    policy_path: str,
    expected_event_class: str,
    full_ci_deferred: bool,
    job_results: dict[str, str],
    bvs_changed: bool,
) -> str:
    require_job_result(job_results, "ci-policy", "success", "bvs-ci-policy did not succeed")
    require_job_result(job_results, "detect", "success", "bvs-detect did not succeed")
    if full_ci_deferred != (policy_path == "defer"):
        raise ProvenanceError("backtester full_ci_deferred must match policy_path == 'defer'")

    if policy_path == "iteration":
        if expected_event_class != "iteration":
            raise ProvenanceError(f"backtester iteration CI policy outside resolver-permitted event class {expected_event_class!r}")
        require_job_result_in(job_results, "fmt", {"success", "skipped"}, "bvs-fmt did not succeed or skip during iteration")
        require_jobs_skipped(job_results, ("clippy", "test-archive", "test"), "backtester iteration")
        return "backtester iteration CI policy; no required full proof published by this run"

    if not bvs_changed:
        allowed_no_crate_paths = frozenset({"full", "noop", "defer"})
        if policy_path not in allowed_no_crate_paths:
            raise ProvenanceError(f"backtester no-crate path does not support policy_path {policy_path!r}")
        if expected_event_class != policy_path:
            raise ProvenanceError(
                "backtester no-crate path requires expected_event_class to match "
                f"policy_path {policy_path!r}, got {expected_event_class!r}"
            )
        require_job_result_in(job_results, "fmt", {"success", "skipped"}, "bvs-fmt did not succeed or skip on non-crate PR")
        require_jobs_skipped(job_results, ("clippy", "test-archive", "test"), "backtester no-crate")
        return "backtester no-crate proof passed"

    if policy_path == "noop":
        if expected_event_class != "noop":
            raise ProvenanceError(f"backtester noop CI policy outside resolver-permitted event class {expected_event_class!r}")
        for job, label in (
            ("fmt", "bvs-fmt"),
            ("clippy", "bvs-clippy"),
            ("test-archive", "bvs-test archive"),
            ("test", "bvs-test"),
        ):
            require_job_result(job_results, job, "success", f"{label} did not succeed")
        return "backtester no-code policy recomputed proof passed"

    if policy_path == "defer" or full_ci_deferred:
        if expected_event_class != "defer":
            raise ProvenanceError(f"backtester deferred CI policy outside resolver-permitted event class {expected_event_class!r}")
        for job, label in (
            ("fmt", "bvs-fmt"),
            ("clippy", "bvs-clippy"),
            ("test-archive", "bvs-test archive"),
            ("test", "bvs-test"),
        ):
            require_job_result(job_results, job, "success", f"{label} did not succeed")
        return "backtester deferred policy recomputed proof passed"

    if policy_path != "full":
        raise ProvenanceError(f"unknown backtester CI policy path {policy_path!r}")
    if expected_event_class != "full":
        raise ProvenanceError(f"backtester full CI policy outside resolver-permitted event class {expected_event_class!r}")
    for job, label in (
        ("fmt", "bvs-fmt"),
        ("clippy", "bvs-clippy"),
        ("test-archive", "bvs-test archive"),
        ("test", "bvs-test"),
    ):
        require_job_result(job_results, job, "success", f"{label} did not succeed")
    return "backtester lanes passed"


def expected_event_class_for(reason: str, path: str) -> str:
    if reason == "docs" or path == "docs":
        return "docs"
    if reason in {
        "draft_pr_synchronize",
        "draft_pr_opened",
        "draft_pr_reopened",
        "draft_pr_edited",
        "converted_to_draft",
    }:
        return "defer"
    if reason in {"ready_pr_edited_no_base", "ready_pr_reopened"} and path == "noop":
        return "noop"
    if reason == "workflow_dispatch":
        return "iteration"
    if reason == "tag":
        return "tag_reuse"
    if reason in {"ready_pr", "ready_for_review"} and path == "iteration":
        return "iteration"
    return "full"


def gate_name_suffix_for(event_name: str, reason: str, path: str) -> str:
    if event_name == "workflow_dispatch" and reason == "workflow_dispatch_full_ci":
        return "dispatch_full"
    if event_name == "workflow_dispatch":
        return "iteration"
    if path in POLICY_VALUES:
        return "required"
    raise ProvenanceError(f"cannot resolve gate name for ci_policy_path {path!r}")


def mergify_temp_pr_matches(
    *,
    event_name: str,
    pull_request_draft: bool,
    pull_request_head_ref: str,
    temp_pr_head_ref_prefix: str,
) -> bool:
    return (
        event_name == "pull_request"
        and pull_request_draft
        and pull_request_head_ref.startswith(temp_pr_head_ref_prefix)
    )


MERGIFY_TEMP_PR_FULL_ACTIONS = frozenset({"opened", "synchronize", "reopened", "ready_for_review"})


def bool_like(value: bool | str) -> bool:
    if isinstance(value, bool):
        return value
    return value.strip().lower() == "true"


def mergify_temp_pr_requires_full_ci(
    *,
    event_action: str,
    pull_request_base_changed: bool | str,
) -> bool:
    base_changed = bool_like(pull_request_base_changed)
    return event_action in MERGIFY_TEMP_PR_FULL_ACTIONS or (event_action == "edited" and base_changed)


def evaluate_ci_policy(
    config: ProvenanceConfig,
    *,
    event_name: str,
    event_action: str,
    pull_request_draft: bool,
    pull_request_head_ref: str = "",
    pull_request_base_changed: bool = False,
    workflow_dispatch_full_ci: str = "",
    docs_only: bool = False,
    ref: str,
) -> CiPolicyResult:
    mergify_temp_pr = mergify_temp_pr_matches(
        event_name=event_name,
        pull_request_draft=pull_request_draft,
        pull_request_head_ref=pull_request_head_ref,
        temp_pr_head_ref_prefix=config.mergify_temp_pr_head_ref_prefix,
    )
    if event_name == "merge_group":
        # The merge queue validates the exact to-be-merged commit on a temporary
        # gh-readonly-queue ref. Resolve on event_name alone so the queue ref
        # shape can never be misclassified as a tag or main_push; always run full.
        path = config.policy["merge_group"]
        reason = "merge_group"
    elif event_name == "push" and ref.startswith("refs/tags/v"):
        path = config.policy["tag"]
        reason = "tag"
    elif event_name == "workflow_dispatch":
        if workflow_dispatch_full_ci == "true":
            path = config.policy["workflow_dispatch_full_ci"]
            reason = "workflow_dispatch_full_ci"
        else:
            path = config.policy["workflow_dispatch"]
            reason = "workflow_dispatch"
    elif event_name == "push" and ref == "refs/heads/main":
        path = config.policy["main_push"]
        reason = "main_push"
    elif event_name == "pull_request":
        if config.force_full_ci:
            path = "full"
            reason = "force_full_ci"
        elif mergify_temp_pr and mergify_temp_pr_requires_full_ci(
            event_action=event_action,
            pull_request_base_changed=pull_request_base_changed,
        ):
            path = config.policy["mergify_temp_pr"]
            reason = "mergify_temp_pr"
        elif event_action == "ready_for_review":
            if pull_request_draft:
                raise ProvenanceError("ready_for_review cannot be on a draft PR")
            path = config.policy["ready_for_review"]
            reason = "ready_for_review"
        elif not pull_request_draft and event_action == "edited" and not pull_request_base_changed:
            path = config.policy["ready_pr_edited_no_base"]
            reason = "ready_pr_edited_no_base"
        elif not pull_request_draft and event_action == "reopened":
            path = config.policy["ready_pr_reopened"]
            reason = "ready_pr_reopened"
        elif not pull_request_draft:
            path = config.policy["ready_pr"]
            reason = "ready_pr"
        elif event_action == "opened":
            path = config.policy["draft_pr_opened"]
            reason = "draft_pr_opened"
        elif event_action == "synchronize":
            path = config.policy["draft_pr_synchronize"]
            reason = "draft_pr_synchronize"
        elif event_action == "reopened":
            path = config.policy["draft_pr_reopened"]
            reason = "draft_pr_reopened"
        elif event_action == "edited":
            path = config.policy["draft_pr_edited"]
            reason = "draft_pr_edited"
        elif event_action == "converted_to_draft":
            path = config.policy["converted_to_draft"]
            reason = "converted_to_draft"
        else:
            path = config.policy["unknown_event"]
            reason = "unknown_event"
    else:
        path = config.policy["unknown_event"]
        reason = "unknown_event"

    if event_name == "pull_request" and docs_only and path == "full" and reason not in {
        "force_full_ci",
        "mergify_temp_pr",
    }:
        path = config.policy["docs"]
        reason = "docs"

    if path not in POLICY_VALUES:
        raise ProvenanceError(f"resolved invalid ci_policy_path {path!r}")
    gate_name_suffix = gate_name_suffix_for(event_name, reason, path)
    return CiPolicyResult(
        ci_policy_path=path,
        full_ci_required=path == "full",
        full_ci_deferred=path == "defer",
        gate_name=config.gate_names[f"gate_{gate_name_suffix}"],
        backtester_gate_name=config.gate_names[f"backtester_{gate_name_suffix}"],
        expected_event_class=expected_event_class_for(reason, path),
        reason=reason,
    )


def load_json(path: pathlib.Path) -> dict[str, object]:
    try:
        data = json.loads(path.read_text(encoding="utf-8"))
    except FileNotFoundError as exc:
        raise ProvenanceError(f"record missing: {path}") from exc
    except json.JSONDecodeError as exc:
        raise ProvenanceError(f"record is invalid JSON: {exc}") from exc
    except OSError as exc:
        raise ProvenanceError(f"record could not be read: {exc}") from exc
    if not isinstance(data, dict):
        raise ProvenanceError("record must be a JSON object")
    return data


def parse_timestamp(value: str) -> datetime.datetime:
    try:
        parsed = datetime.datetime.fromisoformat(value.replace("Z", "+00:00"))
    except ValueError as exc:
        raise ProvenanceError(f"invalid timestamp {value!r}") from exc
    if parsed.tzinfo is None:
        parsed = parsed.replace(tzinfo=datetime.timezone.utc)
    return parsed.astimezone(datetime.timezone.utc)


def normalized_redirect_port(parsed: urllib.parse.ParseResult) -> int | None:
    try:
        explicit_port = parsed.port
    except ValueError:
        return None
    if explicit_port is not None:
        return explicit_port
    if parsed.scheme == "https":
        return 443
    if parsed.scheme == "http":
        return 80
    return None


def redirect_preserves_github_api_headers(old_url: str, new_url: str) -> bool:
    old = urllib.parse.urlparse(old_url)
    new = urllib.parse.urlparse(new_url)
    old_host = (old.hostname or "").lower()
    new_host = (new.hostname or "").lower()
    return (
        old.scheme == new.scheme == "https"
        and old_host == new_host
        and normalized_redirect_port(old) == normalized_redirect_port(new)
        and old.username == new.username
        and old.password == new.password
    )


class SafeGitHubRedirectHandler(urllib.request.HTTPRedirectHandler):
    def redirect_request(self, req, fp, code, msg, headers, newurl):
        redirected = super().redirect_request(req, fp, code, msg, headers, newurl)
        if redirected is None:
            return None
        if not redirect_preserves_github_api_headers(req.full_url, redirected.full_url):
            for header in tuple(redirected.headers):
                if header.lower() in GITHUB_API_REDIRECT_HEADERS:
                    redirected.remove_header(header)
        return redirected


def open_github_api_request(request: urllib.request.Request, *, timeout: int):
    opener = urllib.request.build_opener(SafeGitHubRedirectHandler())
    return opener.open(request, timeout=timeout)


def github_api_json(
    repo: str,
    token: str,
    path: str,
    query: dict[str, str] | None = None,
) -> dict[str, object]:
    url = f"https://api.github.com/repos/{repo}/{path}"
    if query:
        url += "?" + urllib.parse.urlencode(query)
    request = urllib.request.Request(
        url,
        headers={
            "Authorization": f"Bearer {token}",
            **GITHUB_API_HEADERS,
        },
    )
    try:
        with open_github_api_request(request, timeout=30) as response:
            payload = json.loads(response.read().decode("utf-8"))
    except (
        urllib.error.URLError,
        urllib.error.HTTPError,
        UnicodeDecodeError,
        json.JSONDecodeError,
    ) as exc:
        raise ProvenanceError(f"GitHub API request failed for {path}: {exc}") from exc
    if not isinstance(payload, dict):
        raise ProvenanceError(f"GitHub API payload for {path} is malformed")
    return payload


def github_api_bytes(repo: str, token: str, url: str) -> bytes:
    request = urllib.request.Request(
        url,
        headers={
            "Authorization": f"Bearer {token}",
            **GITHUB_API_HEADERS,
        },
    )
    try:
        with open_github_api_request(request, timeout=30) as response:
            return response.read()
    except (urllib.error.URLError, urllib.error.HTTPError) as exc:
        raise ProvenanceError(f"GitHub API download failed for {url}: {exc}") from exc


def artifact_record_from_zip(payload: bytes) -> dict[str, object]:
    try:
        with zipfile.ZipFile(io.BytesIO(payload)) as archive:
            names = [name for name in archive.namelist() if name == "ci-provenance.json"]
            if len(names) != 1:
                raise ProvenanceError("provenance artifact must contain exactly one ci-provenance.json")
            record = json.loads(archive.read(names[0]).decode("utf-8"))
    except zipfile.BadZipFile as exc:
        raise ProvenanceError("provenance artifact archive is malformed") from exc
    except (UnicodeDecodeError, json.JSONDecodeError) as exc:
        raise ProvenanceError("ci-provenance.json is invalid JSON") from exc
    if not isinstance(record, dict):
        raise ProvenanceError("ci-provenance.json must contain a JSON object")
    return record


def as_text(value: object) -> str:
    return "" if value is None else str(value)


def positive_int_value(value: object, field: str) -> int:
    if isinstance(value, int) and value > 0:
        return value
    if isinstance(value, str) and value.isdecimal() and int(value) > 0:
        return int(value)
    raise ProvenanceError(f"{field} must be a positive integer")


def require_complete_first_page(
    payload: dict[str, object],
    items: list[object],
    *,
    per_page: int,
    label: str,
) -> None:
    total_count = payload.get("total_count")
    if total_count is None:
        if len(items) >= per_page:
            raise ProvenanceError(f"{label} page is saturated")
        return
    if type(total_count) is not int or total_count < len(items):
        raise ProvenanceError(f"{label} total_count is malformed")
    if total_count > len(items):
        raise ProvenanceError(f"{label} page is saturated")


def sha256_file(path: pathlib.Path) -> str:
    if path.is_symlink():
        raise ProvenanceError(f"file must not be a symlink for digest: {path}")
    try:
        return hashlib.sha256(path.read_bytes()).hexdigest()
    except FileNotFoundError as exc:
        raise ProvenanceError(f"file missing for digest: {path}") from exc
    except OSError as exc:
        raise ProvenanceError(f"file could not be read for digest: {path}: {exc}") from exc


def workflow_file_digest(config: ProvenanceConfig, workflow_file: pathlib.Path | None = None) -> str:
    if workflow_file is None:
        workflow_file = REPO_ROOT / config.workflow_path
    return sha256_file(workflow_file)


def require_record_string(record: dict[str, object], key: str) -> str:
    value = record.get(key)
    if not isinstance(value, str) or not value:
        raise ProvenanceError(f"record {key} must be a non-empty string")
    return value


def require_record_sha(record: dict[str, object], key: str) -> str:
    value = require_record_string(record, key)
    if SHA_RE.fullmatch(value) is None:
        raise ProvenanceError(f"record {key} must be a 40-character lowercase hex SHA")
    return value


def require_record_digest(record: dict[str, object], key: str) -> str:
    value = require_record_string(record, key)
    if DIGEST_RE.fullmatch(value) is None:
        raise ProvenanceError(f"record {key} must be a sha256 hex digest")
    return value


def require_positive_record_id(record: dict[str, object], key: str) -> None:
    value = record.get(key)
    if isinstance(value, int) and value > 0:
        return
    if isinstance(value, str) and value.isdecimal() and int(value) > 0:
        return
    raise ProvenanceError(f"record {key} must be a positive integer or numeric string")


def validate_created_at(value: object) -> None:
    if not isinstance(value, str) or not value:
        raise ProvenanceError("record created_at must be a non-empty timestamp")
    try:
        datetime.datetime.fromisoformat(value.replace("Z", "+00:00"))
    except ValueError as exc:
        raise ProvenanceError("record created_at must be ISO-8601") from exc


def validate_pull_request_metadata(record: dict[str, object]) -> None:
    pull_request = record.get("pull_request")
    if not isinstance(pull_request, dict):
        raise ProvenanceError("record pull_request must be an object")
    number = pull_request.get("number")
    base_sha = pull_request.get("base_sha")
    if record.get("event") == "pull_request":
        if not isinstance(number, int) or number <= 0:
            raise ProvenanceError("record pull_request.number must be positive for pull_request events")
        if not isinstance(base_sha, str) or SHA_RE.fullmatch(base_sha) is None:
            raise ProvenanceError("record pull_request.base_sha must be a SHA for pull_request events")
    elif number is not None or base_sha is not None:
        raise ProvenanceError("record pull_request metadata must be null outside pull_request events")


def validate_required_jobs(record: dict[str, object], config: ProvenanceConfig, kind: str) -> None:
    required_jobs = record.get("required_jobs")
    if not isinstance(required_jobs, dict):
        raise ProvenanceError("record required_jobs must be an object")
    if set(required_jobs) != set(config.required_jobs):
        raise ProvenanceError("record required_jobs must match configured full-CI jobs")
    for job, conclusion in required_jobs.items():
        if kind == "docs-ci":
            if job in config.docs_non_heavy_required_jobs:
                if conclusion != "success":
                    raise ProvenanceError(f"record docs required job {job} must be success")
            elif conclusion != "skipped":
                raise ProvenanceError(f"record docs required job {job} must be skipped")
        elif conclusion != "success":
            raise ProvenanceError(f"record required_jobs.{job} must be success")


def validate_conditional_jobs(record: dict[str, object], config: ProvenanceConfig, kind: str) -> None:
    conditional_jobs = record.get("conditional_jobs")
    if not isinstance(conditional_jobs, dict):
        raise ProvenanceError("record conditional_jobs must be an object")
    if set(conditional_jobs) != set(config.conditional_jobs):
        raise ProvenanceError("record conditional_jobs must match configured conditional jobs")
    for job, payload in conditional_jobs.items():
        if not isinstance(payload, dict):
            raise ProvenanceError(f"record conditional_jobs.{job} must be an object")
        if not isinstance(payload.get("required"), bool):
            raise ProvenanceError(f"record conditional_jobs.{job}.required must be boolean")
        result = payload.get("result")
        if result is not None and not isinstance(result, str):
            raise ProvenanceError(f"record conditional_jobs.{job}.result must be string or null")
        if kind == "docs-ci" and (payload.get("required") is not False or result != "skipped"):
            raise ProvenanceError(f"record docs conditional job {job} must be not required and skipped")


def validate_record_schema(
    record: dict[str, object],
    config: ProvenanceConfig,
    *,
    config_path: pathlib.Path = DEFAULT_CONFIG,
    expected_workflow_digest: str | None = None,
) -> None:
    if record.get("schema_version") != config.schema_version:
        raise ProvenanceError(f"unknown provenance schema {record.get('schema_version')!r}")
    kind = record.get("kind")
    if kind not in {"full-ci", "docs-ci"}:
        raise ProvenanceError("record kind must be full-ci or docs-ci")
    require_record_string(record, "repository")
    if require_record_string(record, "workflow_path") != config.workflow_path:
        raise ProvenanceError("record workflow_path does not match config")
    workflow_digest = require_record_digest(record, "workflow_digest")
    if expected_workflow_digest is None:
        expected_workflow_digest = workflow_file_digest(config)
    if workflow_digest != expected_workflow_digest:
        raise ProvenanceError("record workflow_digest does not match workflow bytes")
    config_digest = require_record_digest(record, "provenance_config_digest")
    if config_digest != provenance_config_digest(config_path):
        raise ProvenanceError("record provenance_config_digest does not match config")
    require_record_sha(record, "head_sha")
    require_record_sha(record, "tested_sha")
    require_positive_record_id(record, "run_id")
    require_positive_record_id(record, "run_attempt")
    require_positive_record_id(record, "check_suite_id")
    event = require_record_string(record, "event")
    head_branch = record.get("head_branch")
    if head_branch is not None and not isinstance(head_branch, str):
        raise ProvenanceError("record head_branch must be string or null")
    if event == "push" and not head_branch:
        raise ProvenanceError("record head_branch must be present for push events")
    if kind == "docs-ci" and event != "pull_request":
        raise ProvenanceError("docs-ci records must come from pull_request events")
    validate_pull_request_metadata(record)
    validate_required_jobs(record, config, kind)
    validate_conditional_jobs(record, config, kind)
    nextest_fingerprint = record.get("nextest_fingerprint")
    if nextest_fingerprint is not None and (
        not isinstance(nextest_fingerprint, str) or not nextest_fingerprint
    ):
        raise ProvenanceError("record nextest_fingerprint must be string or null")
    validate_created_at(record.get("created_at"))


def validate_exact_sha_record(
    record: dict[str, object],
    config: ProvenanceConfig,
    *,
    requested_sha: str,
    config_path: pathlib.Path = DEFAULT_CONFIG,
    expected_workflow_digest: str | None = None,
) -> None:
    validate_record_schema(
        record,
        config,
        config_path=config_path,
        expected_workflow_digest=expected_workflow_digest,
    )
    if SHA_RE.fullmatch(requested_sha) is None:
        raise ProvenanceError("requested_sha must be a 40-character lowercase hex SHA")
    if record.get("event") == "pull_request":
        raise ProvenanceError("pull_request provenance cannot validate exact-SHA reuse for a PR head")
    if record.get("event") != config.deploy_source_event:
        raise ProvenanceError(f"record event must be {config.deploy_source_event}")
    if record.get("head_branch") != config.deploy_source_branch:
        raise ProvenanceError(f"record head_branch must be {config.deploy_source_branch}")
    if record.get("head_sha") != requested_sha or record.get("tested_sha") != requested_sha:
        raise ProvenanceError("record head_sha and tested_sha must match requested exact SHA")


def provenance_artifact_name(config: ProvenanceConfig, run_attempt: int) -> str:
    try:
        return config.artifact_name_template.format(run_attempt=run_attempt)
    except KeyError as exc:
        raise ProvenanceError("ci_provenance.artifact_name_template has unsupported placeholders") from exc


def run_matches_exact_sha(
    run: dict[str, object],
    config: ProvenanceConfig,
    requested_sha: str,
    current_run_id: int | str | None,
) -> bool:
    if current_run_id is not None and as_text(run.get("id")) == as_text(current_run_id):
        return False
    return (
        as_text(run.get("path")) == config.workflow_path
        and as_text(run.get("event")) == config.deploy_source_event
        and as_text(run.get("head_branch")) == config.deploy_source_branch
        and as_text(run.get("head_sha")) == requested_sha
        and as_text(run.get("status")) == "completed"
        and as_text(run.get("conclusion")) == "success"
    )


def run_matches_fingerprint_reuse(
    run: dict[str, object],
    config: ProvenanceConfig,
    current_run_id: int | str | None,
) -> bool:
    if current_run_id is not None and as_text(run.get("id")) == as_text(current_run_id):
        return False
    return (
        as_text(run.get("path")) == config.workflow_path
        and as_text(run.get("event")) == config.deploy_source_event
        and as_text(run.get("head_branch")) == config.deploy_source_branch
        and as_text(run.get("status")) == "completed"
        and as_text(run.get("conclusion")) == "success"
    )


def workflow_runs_path(config: ProvenanceConfig) -> str:
    workflow_file = pathlib.PurePosixPath(config.workflow_path).name
    return f"actions/workflows/{workflow_file}/runs"


def workflow_digest_from_github(
    repo: str,
    token: str,
    config: ProvenanceConfig,
    tested_sha: str,
    api_bytes,
) -> str:
    url = f"https://raw.githubusercontent.com/{repo}/{tested_sha}/{config.workflow_path}"
    return hashlib.sha256(api_bytes(repo, token, url)).hexdigest()


def validate_artifact_run_metadata(
    artifact: dict[str, object],
    run: dict[str, object],
    *,
    label: str,
) -> None:
    run_id = as_text(run.get("id"))
    if artifact.get("expired") is not False:
        raise ProvenanceError(f"source run {run_id} {label} artifact expired or has unknown expiry state")
    workflow_run = artifact.get("workflow_run")
    if not isinstance(workflow_run, dict):
        raise ProvenanceError(f"source run {run_id} {label} artifact workflow_run payload is malformed")
    if as_text(workflow_run.get("id")) != run_id:
        raise ProvenanceError(f"{label} artifact run ID does not match source run {run_id}")
    if as_text(workflow_run.get("head_sha")) != as_text(run.get("head_sha")):
        raise ProvenanceError(f"{label} artifact SHA does not match source run {run_id}")


def validate_artifact_metadata(
    artifact: dict[str, object],
    run: dict[str, object],
    config: ProvenanceConfig,
    requested_sha: str,
) -> None:
    validate_artifact_run_metadata(artifact, run, label="provenance")
    workflow_run = artifact.get("workflow_run")
    if not isinstance(workflow_run, dict):
        raise ProvenanceError("provenance artifact workflow_run payload is malformed")
    if as_text(workflow_run.get("head_branch")) != config.deploy_source_branch:
        raise ProvenanceError(
            f"artifact branch is {as_text(workflow_run.get('head_branch'))}, expected {config.deploy_source_branch}"
        )
    if as_text(workflow_run.get("head_sha")) != requested_sha:
        raise ProvenanceError(
            f"artifact SHA {as_text(workflow_run.get('head_sha'))} does not match expected {requested_sha}"
        )


def parse_nextest_fingerprint(value: object, *, label: str) -> str:
    if not isinstance(value, str) or not value:
        raise ProvenanceError(f"malformed {label} fingerprint")
    if NEXTEST_FINGERPRINT_RE.fullmatch(value) is None:
        raise ProvenanceError(f"malformed {label} fingerprint")
    return value


def fingerprint_from_artifact_name(artifact: dict[str, object], config: ProvenanceConfig) -> str | None:
    name = artifact.get("name")
    if not isinstance(name, str) or not name.startswith(config.fingerprint_artifact_prefix):
        return None
    suffix = name[len(config.fingerprint_artifact_prefix):]
    if not suffix:
        raise ProvenanceError("malformed source fingerprint artifact")
    return parse_nextest_fingerprint(f"nextest-archive-{suffix}", label="source artifact")


def validate_record_matches_run(record: dict[str, object], run: dict[str, object]) -> None:
    checks = (
        ("run_id", "id"),
        ("run_attempt", "run_attempt"),
        ("check_suite_id", "check_suite_id"),
        ("event", "event"),
        ("head_branch", "head_branch"),
        ("head_sha", "head_sha"),
    )
    for record_key, run_key in checks:
        if as_text(record.get(record_key)) != as_text(run.get(run_key)):
            raise ProvenanceError(f"record {record_key} does not match source run {run_key}")


def expanded_check_names(config: ProvenanceConfig, logical_job: str) -> tuple[str, ...]:
    job = config.jobs[logical_job]
    if job.check_name is not None:
        return (job.check_name,)
    if job.check_name_template is None or job.shard_count is None:
        raise ProvenanceError(f"ci_provenance.full_ci.jobs.{logical_job} has no check name mapping")
    return tuple(
        job.check_name_template.format(shard=shard, shard_count=job.shard_count)
        for shard in range(1, job.shard_count + 1)
    )


def jobs_by_name(jobs_payload: dict[str, object]) -> dict[str, dict[str, object]]:
    jobs = jobs_payload.get("jobs")
    if not isinstance(jobs, list):
        raise ProvenanceError("run jobs payload is malformed")
    by_name: dict[str, dict[str, object]] = {}
    for job in jobs:
        if not isinstance(job, dict):
            raise ProvenanceError("run jobs payload is malformed")
        name = job.get("name")
        if not isinstance(name, str) or not name:
            raise ProvenanceError("run jobs payload has malformed job name")
        if name in by_name:
            raise ProvenanceError(f"run jobs payload has duplicate job name {name}")
        by_name[name] = job
    return by_name


def require_job_success(by_name: dict[str, dict[str, object]], check_name: str) -> None:
    job = by_name.get(check_name)
    if job is None:
        raise ProvenanceError(f"missing required job {check_name}")
    status = job.get("status")
    conclusion = job.get("conclusion")
    if status != "completed" or conclusion != "success":
        raise ProvenanceError(f"required job {check_name} was {status!r}/{conclusion!r}")


def validate_job_evidence(
    jobs_payload: dict[str, object],
    config: ProvenanceConfig,
    record: dict[str, object],
    *,
    deploy_reuse_requested: bool,
) -> None:
    by_name = jobs_by_name(jobs_payload)
    for logical_job in config.required_jobs:
        for check_name in expanded_check_names(config, logical_job):
            require_job_success(by_name, check_name)

    conditional_jobs = record.get("conditional_jobs")
    if not isinstance(conditional_jobs, dict):
        raise ProvenanceError("record conditional_jobs must be an object")
    for logical_job in config.conditional_jobs:
        job_config = config.jobs[logical_job]
        if job_config.check_name is None:
            raise ProvenanceError(f"ci_provenance.full_ci.jobs.{logical_job}.check_name missing")
        payload = conditional_jobs.get(logical_job)
        if not isinstance(payload, dict):
            raise ProvenanceError(f"record conditional_jobs.{logical_job} must be an object")
        required = payload.get("required")
        if not isinstance(required, bool):
            raise ProvenanceError(f"record conditional_jobs.{logical_job}.required must be boolean")
        job = by_name.get(job_config.check_name)
        if required:
            require_job_success(by_name, job_config.check_name)
            continue
        if deploy_reuse_requested:
            if job is None:
                raise ProvenanceError(f"missing required job {job_config.check_name}")
            if job.get("status") != "completed" or job.get("conclusion") != "success":
                raise ProvenanceError("deploy reuse requires build success")
            continue
        if job is None:
            continue
        conclusion = job.get("conclusion")
        status = job.get("status")
        if status != "completed" or conclusion not in {"success", "skipped"}:
            raise ProvenanceError(
                f"conditional job {job_config.check_name} was {status!r}/{conclusion!r}"
            )


def resolve_exact_sha_evidence(
    *,
    repo: str,
    token: str,
    requested_sha: str,
    config: ProvenanceConfig,
    config_path: pathlib.Path = DEFAULT_CONFIG,
    current_run_id: int | str | None = None,
    api_json=github_api_json,
    api_bytes=github_api_bytes,
    now: datetime.datetime | None = None,
) -> ResolvedEvidence:
    if SHA_RE.fullmatch(requested_sha) is None:
        raise ProvenanceError("requested_sha must be a 40-character lowercase hex SHA")
    if now is None:
        now = datetime.datetime.now(datetime.timezone.utc)
    cutoff = now - datetime.timedelta(seconds=config.max_lookback_age_seconds)
    candidates: list[dict[str, object]] = []
    last_page_len = 0

    for page in range(1, config.max_lookback_pages + 1):
        runs_payload = api_json(
            repo,
            token,
            "actions/runs",
            {
                "event": config.deploy_source_event,
                "branch": config.deploy_source_branch,
                "head_sha": requested_sha,
                "per_page": str(config.workflow_runs_per_page),
                "page": str(page),
                "sort": "created",
                "direction": "desc",
            },
        )
        runs = runs_payload.get("workflow_runs")
        if not isinstance(runs, list):
            raise ProvenanceError("workflow runs payload is malformed")
        last_page_len = len(runs)
        if not runs:
            break
        # Scan the whole page before applying the age cutoff so a stale item
        # cannot hide a fresh candidate when API ordering is imperfect.
        page_has_fresh_run = False
        page_has_old_run = False
        for run in runs:
            if not isinstance(run, dict):
                raise ProvenanceError("workflow runs payload is malformed")
            created_at = run.get("created_at")
            if not isinstance(created_at, str):
                raise ProvenanceError("workflow run created_at must be a string")
            if parse_timestamp(created_at) < cutoff:
                page_has_old_run = True
                continue
            page_has_fresh_run = True
            if run_matches_exact_sha(run, config, requested_sha, current_run_id):
                candidates.append(run)
        if page_has_old_run and not page_has_fresh_run and not candidates:
            raise ProvenanceError("lookback age limit exhausted before candidate evidence was found")
        if candidates or len(runs) < config.workflow_runs_per_page:
            break

    if not candidates:
        if last_page_len >= config.workflow_runs_per_page:
            raise ProvenanceError("lookback page limit exhausted before candidate evidence was found")
        raise ProvenanceError(f"no candidate provenance evidence found for exact SHA {requested_sha}")

    candidates.sort(
        key=lambda run: (
            positive_int_value(run.get("run_attempt"), "workflow run run_attempt"),
            as_text(run.get("updated_at")),
            positive_int_value(run.get("id"), "workflow run id"),
        ),
        reverse=True,
    )

    artifact_by_attempt: dict[int, dict[str, object]] = {}
    run_by_attempt: dict[int, dict[str, object]] = {}
    for run in candidates:
        run_id = positive_int_value(run.get("id"), "workflow run id")
        run_attempt = positive_int_value(run.get("run_attempt"), "workflow run run_attempt")
        artifacts_payload = api_json(
            repo,
            token,
            f"actions/runs/{run_id}/artifacts",
            {"per_page": str(config.run_artifacts_per_page)},
        )
        artifacts = artifacts_payload.get("artifacts")
        if not isinstance(artifacts, list):
            raise ProvenanceError(f"source run {run_id} artifacts payload is malformed")
        require_complete_first_page(
            artifacts_payload,
            artifacts,
            per_page=config.run_artifacts_per_page,
            label=f"source run {run_id} artifacts",
        )
        expected_name = provenance_artifact_name(config, run_attempt)
        matches = [
            artifact
            for artifact in artifacts
            if isinstance(artifact, dict) and as_text(artifact.get("name")) == expected_name
        ]
        if len(matches) > 1:
            raise ProvenanceError(f"source run {run_id} has ambiguous provenance artifacts for attempt {run_attempt}")
        if not matches:
            continue
        if run_attempt in artifact_by_attempt:
            raise ProvenanceError(f"multiple provenance artifacts for attempt {run_attempt}")
        artifact_by_attempt[run_attempt] = matches[0]
        run_by_attempt[run_attempt] = run

    if not artifact_by_attempt:
        raise ProvenanceError(f"no candidate provenance artifact found for exact SHA {requested_sha}")

    for run_attempt in sorted(artifact_by_attempt, reverse=True):
        artifact = artifact_by_attempt[run_attempt]
        run = run_by_attempt[run_attempt]
        validate_artifact_metadata(artifact, run, config, requested_sha)
        archive_url = require_record_string(artifact, "archive_download_url")
        record = artifact_record_from_zip(api_bytes(repo, token, archive_url))
        if positive_int_value(record.get("run_attempt"), "record run_attempt") != run_attempt:
            raise ProvenanceError("record run_attempt does not match source run attempt")
        tested_sha = require_record_sha(record, "tested_sha")
        expected_workflow_digest = workflow_digest_from_github(
            repo, token, config, tested_sha, api_bytes
        )
        validate_exact_sha_record(
            record,
            config,
            requested_sha=requested_sha,
            config_path=config_path,
            expected_workflow_digest=expected_workflow_digest,
        )
        validate_record_matches_run(record, run)
        run_id = positive_int_value(run.get("id"), "workflow run id")
        jobs_payload = api_json(
            repo,
            token,
            f"actions/runs/{run_id}/jobs",
            {"per_page": str(config.run_jobs_per_page)},
        )
        jobs = jobs_payload.get("jobs")
        if not isinstance(jobs, list):
            raise ProvenanceError(f"source run {run_id} jobs payload is malformed")
        require_complete_first_page(
            jobs_payload,
            jobs,
            per_page=config.run_jobs_per_page,
            label=f"source run {run_id} jobs",
        )
        validate_job_evidence(jobs_payload, config, record, deploy_reuse_requested=True)
        return ResolvedEvidence(run=run, artifact=artifact, record=record)

    raise ProvenanceError(f"no valid provenance evidence found for exact SHA {requested_sha}")


def run_matches_gate_carry_forward(
    run: dict[str, object],
    *,
    requested_sha: str,
    current_run_id: int | str | None,
    workflow_path: str,
) -> bool:
    if current_run_id is not None and as_text(run.get("id")) == as_text(current_run_id):
        return False
    return (
        as_text(run.get("path")) == workflow_path
        and as_text(run.get("event")) == "pull_request"
        and as_text(run.get("head_sha")) == requested_sha
    )


def validate_gate_carry_forward_provenance(
    *,
    repo: str,
    token: str,
    run: dict[str, object],
    requested_sha: str,
    base_sha: str,
    config: ProvenanceConfig,
    config_path: pathlib.Path,
    api_json,
    api_bytes,
) -> None:
    if SHA_RE.fullmatch(base_sha) is None:
        raise ProvenanceError("base_sha must be a 40-character lowercase hex SHA")
    run_id = positive_int_value(run.get("id"), "workflow run id")
    run_attempt = positive_int_value(run.get("run_attempt"), "workflow run run_attempt")
    artifacts_payload = api_json(
        repo,
        token,
        f"actions/runs/{run_id}/artifacts",
        {"per_page": str(config.run_artifacts_per_page)},
    )
    artifacts = artifacts_payload.get("artifacts")
    if not isinstance(artifacts, list):
        raise ProvenanceError(f"source run {run_id} artifacts payload is malformed")
    require_complete_first_page(
        artifacts_payload,
        artifacts,
        per_page=config.run_artifacts_per_page,
        label=f"source run {run_id} artifacts",
    )
    expected_name = provenance_artifact_name(config, run_attempt)
    matches = [
        artifact
        for artifact in artifacts
        if isinstance(artifact, dict) and as_text(artifact.get("name")) == expected_name
    ]
    if len(matches) != 1:
        if not matches:
            raise ProvenanceError(f"source run {run_id} has no provenance artifact")
        raise ProvenanceError(f"source run {run_id} has ambiguous provenance artifacts")
    artifact = matches[0]
    validate_artifact_run_metadata(artifact, run, label="provenance")
    archive_url = require_record_string(artifact, "archive_download_url")
    record = artifact_record_from_zip(api_bytes(repo, token, archive_url))
    if positive_int_value(record.get("run_attempt"), "record run_attempt") != run_attempt:
        raise ProvenanceError("record run_attempt does not match source run attempt")
    tested_sha = require_record_sha(record, "tested_sha")
    expected_workflow_digest = workflow_digest_from_github(
        repo,
        token,
        config,
        tested_sha,
        api_bytes,
    )
    validate_record_schema(
        record,
        config,
        config_path=config_path,
        expected_workflow_digest=expected_workflow_digest,
    )
    validate_record_matches_run(record, run)
    if record.get("event") != "pull_request":
        raise ProvenanceError("carry-forward provenance must come from a pull_request run")
    if record.get("head_sha") != requested_sha:
        raise ProvenanceError("carry-forward provenance head_sha does not match requested SHA")
    pull_request = record.get("pull_request")
    if not isinstance(pull_request, dict):
        raise ProvenanceError("carry-forward provenance pull_request metadata is malformed")
    if pull_request.get("base_sha") != base_sha:
        raise ProvenanceError("base_sha does not match current PR base")


def missing_gate_carry_forward_provenance_error(exc: ProvenanceError, run_id: int) -> bool:
    return str(exc) == f"source run {run_id} has no provenance artifact"


def resolve_gate_carry_forward(
    *,
    repo: str,
    token: str,
    requested_sha: str,
    base_sha: str,
    current_run_id: int | str | None,
    gate_name: str,
    workflow_path: str,
    config: ProvenanceConfig,
    config_path: pathlib.Path = DEFAULT_CONFIG,
    require_provenance_base: bool,
    api_json=github_api_json,
    api_bytes=github_api_bytes,
    now: datetime.datetime | None = None,
) -> GateCarryForwardResolution:
    if SHA_RE.fullmatch(requested_sha) is None:
        raise ProvenanceError("requested_sha must be a 40-character lowercase hex SHA")
    if not github_actions_output_safe_check_name(gate_name):
        raise ProvenanceError("gate_name must be a GitHub Actions output-safe check name")
    if now is None:
        now = datetime.datetime.now(datetime.timezone.utc)
    cutoff = now - datetime.timedelta(seconds=config.max_lookback_age_seconds)
    last_error = f"no prior successful {gate_name} check found for exact SHA {requested_sha}"

    candidates: list[dict[str, object]] = []
    for page in range(1, config.max_lookback_pages + 1):
        runs_payload = api_json(
            repo,
            token,
            "actions/runs",
            {
                "event": "pull_request",
                "head_sha": requested_sha,
                "per_page": str(config.workflow_runs_per_page),
                "page": str(page),
                "sort": "created",
                "direction": "desc",
            },
        )
        runs = runs_payload.get("workflow_runs")
        if not isinstance(runs, list):
            raise ProvenanceError("workflow runs payload is malformed")
        if not runs:
            break
        for run in runs:
            if not isinstance(run, dict):
                raise ProvenanceError("workflow runs payload is malformed")
            created_at = run.get("created_at")
            if not isinstance(created_at, str):
                raise ProvenanceError("workflow run created_at must be a string")
            matches = run_matches_gate_carry_forward(
                run,
                requested_sha=requested_sha,
                current_run_id=current_run_id,
                workflow_path=workflow_path,
            )
            if parse_timestamp(created_at) < cutoff:
                # The age cutoff bounds which prior *success* may be carried
                # forward: a carried success must still have its provenance
                # artifact within retention. A completed non-success run, by
                # contrast, dominates by its conclusion alone -- that signal is
                # in the run metadata and does not depend on artifact retention.
                # Keep an old same-SHA run that was re-run to failure so the
                # dominance sort below still blocks a newer carry-forward; skip
                # only old successes and not-yet-completed runs.
                if (
                    matches
                    and as_text(run.get("status")) == "completed"
                    and as_text(run.get("conclusion")) != "success"
                ):
                    candidates.append(run)
                continue
            if matches:
                candidates.append(run)
        # Old failures legitimately live on all-old created_at pages after a
        # later success page. Keep scanning through the bounded lookback so a
        # re-run failure can dominate by updated_at; the residual depth bound is
        # max_lookback_pages, the same bound that limits the success search.
        if len(runs) < config.workflow_runs_per_page:
            break

    candidates.sort(
        key=lambda run: (
            as_text(run.get("updated_at")),
            positive_int_value(run.get("id"), "workflow run id"),
        ),
        reverse=True,
    )
    saw_successful_gate_without_provenance = False
    for run in candidates:
        run_id = positive_int_value(run.get("id"), "workflow run id")
        status = as_text(run.get("status"))
        conclusion = as_text(run.get("conclusion"))
        if status != "completed" or conclusion != "success":
            if saw_successful_gate_without_provenance:
                last_error = f"older same-SHA carry-forward run {run_id} was {status!r}/{conclusion!r}"
                continue
            raise ProvenanceError(
                f"newest same-SHA carry-forward run {run_id} was {status!r}/{conclusion!r}"
            )
        jobs_payload = api_json(
            repo,
            token,
            f"actions/runs/{run_id}/jobs",
            {"per_page": str(config.run_jobs_per_page)},
        )
        jobs = jobs_payload.get("jobs")
        if not isinstance(jobs, list):
            raise ProvenanceError(f"source run {run_id} jobs payload is malformed")
        require_complete_first_page(
            jobs_payload,
            jobs,
            per_page=config.run_jobs_per_page,
            label=f"source run {run_id} jobs",
        )
        try:
            require_job_success(jobs_by_name(jobs_payload), gate_name)
        except ProvenanceError as exc:
            if saw_successful_gate_without_provenance:
                last_error = f"older same-SHA carry-forward run {run_id} did not prove {gate_name}: {exc}"
                continue
            raise ProvenanceError(
                f"newest same-SHA carry-forward run {run_id} did not prove {gate_name}: {exc}"
            ) from exc
        if require_provenance_base:
            try:
                validate_gate_carry_forward_provenance(
                    repo=repo,
                    token=token,
                    run=run,
                    requested_sha=requested_sha,
                    base_sha=base_sha,
                    config=config,
                    config_path=config_path,
                    api_json=api_json,
                    api_bytes=api_bytes,
                )
            except ProvenanceError as exc:
                if missing_gate_carry_forward_provenance_error(exc, run_id):
                    saw_successful_gate_without_provenance = True
                    last_error = str(exc)
                    continue
                if saw_successful_gate_without_provenance:
                    last_error = str(exc)
                    continue
                raise
        return GateCarryForwardResolution(
            carry_forward_verified=True,
            source_run_id=str(run_id),
            source_sha=requested_sha,
            gate_name=gate_name,
            reason=f"matched prior {gate_name} run {run_id}",
        )

    raise ProvenanceError(last_error)


def no_fingerprint_reuse(reason: str) -> FingerprintReuseResolution:
    return FingerprintReuseResolution(
        reuse_found=False,
        source_run_id="",
        source_sha="",
        source_artifact_id="",
        reason=reason,
    )


def matching_artifacts(
    artifacts: list[object],
    *,
    name: str | None = None,
    prefix: str | None = None,
) -> list[dict[str, object]]:
    matches: list[dict[str, object]] = []
    for artifact in artifacts:
        if not isinstance(artifact, dict):
            continue
        artifact_name = artifact.get("name")
        if not isinstance(artifact_name, str):
            continue
        if name is not None and artifact_name == name:
            matches.append(artifact)
        elif prefix is not None and artifact_name.startswith(prefix):
            matches.append(artifact)
    return matches


def artifact_id_text(artifact: dict[str, object]) -> str:
    return str(positive_int_value(artifact.get("id"), "artifact id"))


def validate_fingerprint_candidate(
    *,
    repo: str,
    token: str,
    run: dict[str, object],
    current_fingerprint: str,
    config: ProvenanceConfig,
    config_path: pathlib.Path,
    api_json,
    api_bytes,
) -> FingerprintReuseResolution:
    run_id = positive_int_value(run.get("id"), "workflow run id")
    run_attempt = positive_int_value(run.get("run_attempt"), "workflow run run_attempt")
    artifacts_payload = api_json(
        repo,
        token,
        f"actions/runs/{run_id}/artifacts",
        {"per_page": str(config.run_artifacts_per_page)},
    )
    artifacts = artifacts_payload.get("artifacts")
    if not isinstance(artifacts, list):
        return no_fingerprint_reuse(f"source run {run_id} artifacts payload is malformed")
    try:
        require_complete_first_page(
            artifacts_payload,
            artifacts,
            per_page=config.run_artifacts_per_page,
            label=f"source run {run_id} artifacts",
        )
    except ProvenanceError as exc:
        return no_fingerprint_reuse(str(exc))

    fingerprint_matches = matching_artifacts(artifacts, prefix=config.fingerprint_artifact_prefix)
    if len(fingerprint_matches) != 1:
        if not fingerprint_matches:
            return no_fingerprint_reuse(f"source run {run_id} has no fingerprint artifact")
        return no_fingerprint_reuse(f"source run {run_id} has ambiguous fingerprint artifacts")
    fingerprint_artifact = fingerprint_matches[0]

    expected_name = provenance_artifact_name(config, run_attempt)
    provenance_matches = matching_artifacts(artifacts, name=expected_name)
    if len(provenance_matches) != 1:
        if not provenance_matches:
            return no_fingerprint_reuse(f"source run {run_id} has no provenance artifact")
        return no_fingerprint_reuse(f"source run {run_id} has ambiguous provenance artifacts")
    provenance_artifact = provenance_matches[0]

    try:
        validate_artifact_run_metadata(fingerprint_artifact, run, label="fingerprint")
        validate_artifact_run_metadata(provenance_artifact, run, label="provenance")
        artifact_fingerprint = fingerprint_from_artifact_name(fingerprint_artifact, config)
        archive_url = require_record_string(provenance_artifact, "archive_download_url")
        record = artifact_record_from_zip(api_bytes(repo, token, archive_url))
        if positive_int_value(record.get("run_attempt"), "record run_attempt") != run_attempt:
            return no_fingerprint_reuse("record run_attempt does not match source run attempt")
        tested_sha = require_record_sha(record, "tested_sha")
        expected_workflow_digest = workflow_digest_from_github(
            repo, token, config, tested_sha, api_bytes
        )
        validate_record_schema(
            record,
            config,
            config_path=config_path,
            expected_workflow_digest=expected_workflow_digest,
        )
        if require_record_digest(record, "workflow_digest") != workflow_file_digest(config):
            return no_fingerprint_reuse(f"source run {run_id} workflow digest does not match current workflow")
        validate_record_matches_run(record, run)
        record_fingerprint = parse_nextest_fingerprint(
            record.get("nextest_fingerprint"), label="source record"
        )
        if artifact_fingerprint != record_fingerprint:
            return no_fingerprint_reuse(f"source run {run_id} fingerprint artifact does not match provenance")
        if record_fingerprint != current_fingerprint:
            return no_fingerprint_reuse(f"source run {run_id} fingerprint does not match current run")
    except ProvenanceError as exc:
        return no_fingerprint_reuse(str(exc))

    jobs_payload = api_json(
        repo,
        token,
        f"actions/runs/{run_id}/jobs",
        {"per_page": str(config.run_jobs_per_page)},
    )
    jobs = jobs_payload.get("jobs")
    if not isinstance(jobs, list):
        return no_fingerprint_reuse(f"source run {run_id} jobs payload is malformed")
    try:
        require_complete_first_page(
            jobs_payload,
            jobs,
            per_page=config.run_jobs_per_page,
            label=f"source run {run_id} jobs",
        )
        validate_job_evidence(jobs_payload, config, record, deploy_reuse_requested=False)
    except ProvenanceError as exc:
        return no_fingerprint_reuse(str(exc))

    return FingerprintReuseResolution(
        reuse_found=True,
        source_run_id=str(run_id),
        source_sha=require_record_sha(record, "tested_sha"),
        source_artifact_id=artifact_id_text(provenance_artifact),
        reason=f"matched source run {run_id}",
    )


def resolve_fingerprint_reuse(
    *,
    repo: str,
    token: str,
    current_fingerprint: str | None,
    current_run_id: int | str | None,
    config: ProvenanceConfig,
    config_path: pathlib.Path = DEFAULT_CONFIG,
    api_json=github_api_json,
    api_bytes=github_api_bytes,
    now: datetime.datetime | None = None,
) -> FingerprintReuseResolution:
    if current_fingerprint is None:
        return no_fingerprint_reuse("missing current fingerprint")
    try:
        parsed_current = parse_nextest_fingerprint(current_fingerprint, label="current")
    except ProvenanceError:
        return no_fingerprint_reuse("malformed current fingerprint")
    if now is None:
        now = datetime.datetime.now(datetime.timezone.utc)
    cutoff = now - datetime.timedelta(seconds=config.max_lookback_age_seconds)
    last_reason = "no prior successful CI run with matching fingerprint"
    candidates: list[dict[str, object]] = []
    page_limit_exhausted = False

    try:
        for page in range(1, config.max_lookback_pages + 1):
            runs_payload = api_json(
                repo,
                token,
                workflow_runs_path(config),
                {
                    "per_page": str(config.workflow_runs_per_page),
                    "page": str(page),
                    "sort": "created",
                    "direction": "desc",
                },
            )
            runs = runs_payload.get("workflow_runs")
            if not isinstance(runs, list):
                raise ProvenanceError("workflow runs payload is malformed")
            if not runs:
                break
            page_has_fresh_run = False
            page_has_old_run = False
            for run in runs:
                if not isinstance(run, dict):
                    raise ProvenanceError("workflow runs payload is malformed")
                created_at = run.get("created_at")
                if not isinstance(created_at, str):
                    raise ProvenanceError("workflow run created_at must be a string")
                if parse_timestamp(created_at) < cutoff:
                    page_has_old_run = True
                    continue
                page_has_fresh_run = True
                if run_matches_fingerprint_reuse(run, config, current_run_id):
                    candidates.append(run)
            if page_has_old_run and not page_has_fresh_run:
                last_reason = "lookback age limit exhausted before reusable fingerprint evidence was found"
                break
            if len(runs) < config.workflow_runs_per_page:
                break
        else:
            page_limit_exhausted = True
    except ProvenanceError as exc:
        return no_fingerprint_reuse(f"fingerprint reuse lookup failed: {exc}")

    if page_limit_exhausted:
        last_reason = "lookback page limit exhausted before reusable fingerprint evidence was found"

    try:
        candidates.sort(
            key=lambda run: (
                as_text(run.get("created_at")),
                as_text(run.get("updated_at")),
                positive_int_value(run.get("id"), "workflow run id"),
            ),
            reverse=True,
        )
        for run in candidates:
            result = validate_fingerprint_candidate(
                repo=repo,
                token=token,
                run=run,
                current_fingerprint=parsed_current,
                config=config,
                config_path=config_path,
                api_json=api_json,
                api_bytes=api_bytes,
            )
            if result.reuse_found:
                return result
            last_reason = result.reason
    except ProvenanceError as exc:
        return no_fingerprint_reuse(f"fingerprint reuse lookup failed: {exc}")

    return no_fingerprint_reuse(last_reason)


def output_resolution_lines(result: FingerprintReuseResolution) -> str:
    values = {
        "reuse_found": str(result.reuse_found).lower(),
        "source_run_id": result.source_run_id,
        "source_sha": result.source_sha,
        "source_artifact_id": result.source_artifact_id,
        "reason": result.reason.replace("\n", " "),
    }
    return "".join(f"{key}={value}\n" for key, value in values.items())


def require_env(name: str) -> str:
    value = os.environ.get(name)
    if not value:
        raise ProvenanceError(f"missing required environment variable {name}")
    return value


def parse_key_value(value: str) -> tuple[str, str]:
    if "=" not in value:
        raise ProvenanceError(f"expected key=value, got {value!r}")
    key, parsed_value = value.split("=", 1)
    if not key:
        raise ProvenanceError(f"expected non-empty key in {value!r}")
    return key, parsed_value


def parse_job_result_values(values: list[str]) -> dict[str, str]:
    results: dict[str, str] = {}
    for value in values:
        key, parsed_value = parse_key_value(value)
        if key in results:
            raise ProvenanceError(f"duplicate --job result for {key}")
        results[key] = parsed_value
    if not results:
        raise ProvenanceError("at least one --job result is required")
    return results


def parse_required_job_results(
    values: list[str],
    config: ProvenanceConfig,
    *,
    ci_policy_path: str = "full",
) -> dict[str, str]:
    results = dict(parse_key_value(value) for value in values)
    expected = set(config.required_jobs)
    if set(results) != expected:
        missing = sorted(expected - set(results))
        extra = sorted(set(results) - expected)
        raise ProvenanceError(f"required job result keys mismatch; missing={missing} extra={extra}")
    for job, result in results.items():
        if ci_policy_path == "docs":
            if job in config.docs_non_heavy_required_jobs:
                if result != "success":
                    raise ProvenanceError(f"docs required job {job} did not succeed: {result}")
            elif result != "skipped":
                raise ProvenanceError(f"docs required job {job} must be skipped: {result}")
        elif result != "success":
            raise ProvenanceError(f"required job {job} did not succeed: {result}")
    return results


def parse_conditional_job_results(
    values: list[str],
    config: ProvenanceConfig,
    *,
    ci_policy_path: str = "full",
) -> dict[str, dict[str, object]]:
    parsed = dict(parse_key_value(value) for value in values)
    conditional_jobs: dict[str, dict[str, object]] = {}
    for job in config.conditional_jobs:
        required_key = f"{job}.required"
        result_key = f"{job}.result"
        if required_key not in parsed or result_key not in parsed:
            raise ProvenanceError(f"conditional job {job} must provide required and result")
        required = parse_bool(parsed[required_key])
        result = parsed[result_key]
        if ci_policy_path == "docs":
            if required or result != "skipped":
                raise ProvenanceError(f"docs conditional job {job} must be not required and skipped")
            conditional_jobs[job] = {"required": required, "result": result}
            continue
        if required and result != "success":
            raise ProvenanceError(f"conditional job {job} did not succeed while required: {result}")
        if not required and result not in {"success", "skipped"}:
            raise ProvenanceError(f"conditional job {job} had unexpected result while not required: {result}")
        conditional_jobs[job] = {"required": required, "result": result}
    expected_keys = {f"{job}.required" for job in config.conditional_jobs} | {
        f"{job}.result" for job in config.conditional_jobs
    }
    extra = sorted(set(parsed) - expected_keys)
    if extra:
        raise ProvenanceError(f"unexpected conditional job keys: {extra}")
    return conditional_jobs


def pull_request_metadata_from_env(event_name: str) -> dict[str, object]:
    if event_name != "pull_request":
        return {"number": None, "base_sha": None}
    number = os.environ.get("PR_NUMBER")
    base_sha = os.environ.get("PR_BASE_SHA")
    if not number or not number.isdecimal():
        raise ProvenanceError("PR_NUMBER must be set for pull_request provenance")
    if base_sha is None or SHA_RE.fullmatch(base_sha) is None:
        raise ProvenanceError("PR_BASE_SHA must be set for pull_request provenance")
    return {"number": int(number), "base_sha": base_sha}


def emit_full_ci_record(
    *,
    config: ProvenanceConfig,
    config_path: pathlib.Path,
    ci_policy_path: str = "full",
    workflow_file: pathlib.Path | None = None,
    required_job_values: list[str],
    conditional_job_values: list[str],
    nextest_fingerprint: str | None,
    api_json=github_api_json,
) -> dict[str, object]:
    if ci_policy_path not in {"full", "docs"}:
        raise ProvenanceError(f"emit-full-ci only supports full or docs policy paths, got {ci_policy_path!r}")
    repo = require_env("GITHUB_REPOSITORY")
    token = require_env("GITHUB_TOKEN")
    run_id = require_env("GITHUB_RUN_ID")
    run_attempt = require_env("GITHUB_RUN_ATTEMPT")
    tested_sha = require_env("GITHUB_SHA")
    event_name = require_env("GITHUB_EVENT_NAME")
    run_payload = api_json(repo, token, f"actions/runs/{run_id}", None)
    head_sha = as_text(run_payload.get("head_sha"))
    if SHA_RE.fullmatch(head_sha) is None:
        raise ProvenanceError("current workflow run head_sha is malformed")
    check_suite_id = run_payload.get("check_suite_id")
    positive_int_value(check_suite_id, "current workflow run check_suite_id")
    head_branch = run_payload.get("head_branch")
    if head_branch is not None and not isinstance(head_branch, str):
        raise ProvenanceError("current workflow run head_branch is malformed")

    if not nextest_fingerprint:
        nextest_fingerprint = None
    if nextest_fingerprint is not None:
        nextest_fingerprint = parse_nextest_fingerprint(nextest_fingerprint, label="current")
    workflow_digest = workflow_file_digest(config, workflow_file)

    record = {
        "schema_version": config.schema_version,
        "kind": "docs-ci" if ci_policy_path == "docs" else "full-ci",
        "repository": repo,
        "workflow_path": config.workflow_path,
        "workflow_digest": workflow_digest,
        "provenance_config_digest": provenance_config_digest(config_path),
        "head_sha": head_sha,
        "tested_sha": tested_sha,
        "run_id": positive_int_value(run_id, "GITHUB_RUN_ID"),
        "run_attempt": positive_int_value(run_attempt, "GITHUB_RUN_ATTEMPT"),
        "check_suite_id": positive_int_value(check_suite_id, "current workflow run check_suite_id"),
        "event": event_name,
        "head_branch": head_branch,
        "pull_request": pull_request_metadata_from_env(event_name),
        "required_jobs": parse_required_job_results(
            required_job_values,
            config,
            ci_policy_path=ci_policy_path,
        ),
        "conditional_jobs": parse_conditional_job_results(
            conditional_job_values,
            config,
            ci_policy_path=ci_policy_path,
        ),
        "nextest_fingerprint": nextest_fingerprint,
        "created_at": datetime.datetime.now(datetime.timezone.utc).isoformat().replace("+00:00", "Z"),
    }
    validate_record_schema(
        record,
        config,
        config_path=config_path,
        expected_workflow_digest=workflow_digest,
    )
    return record


def parser_for_mode(mode: str) -> argparse.ArgumentParser:
    parser = argparse.ArgumentParser(prog=f"ci_provenance.py {mode}")
    parser.add_argument("--config", type=pathlib.Path, default=DEFAULT_CONFIG)
    if mode == "ci-policy":
        parser.add_argument("--event-name", required=True)
        parser.add_argument("--event-action", default="")
        parser.add_argument("--pull-request-draft", default="false")
        parser.add_argument("--pull-request-head-ref", default="")
        parser.add_argument("--pull-request-base-changed", default="false")
        parser.add_argument("--workflow-dispatch-full-ci", default="")
        parser.add_argument("--docs-only", default="false")
        parser.add_argument("--ref", required=True)
    if mode == "check-ci-gate":
        parser.add_argument("--policy-path", required=True)
        parser.add_argument("--expected-event-class", required=True)
        parser.add_argument("--full-ci-deferred", default="false")
        parser.add_argument("--ignore-emit-failure", default="false")
        parser.add_argument("--reuse-found", default="false")
        parser.add_argument("--carry-forward-verified", default="false")
        parser.add_argument("--build-required", default="false")
        parser.add_argument("--job", action="append", default=[])
    if mode == "check-backtester-gate":
        parser.add_argument("--policy-path", required=True)
        parser.add_argument("--expected-event-class", required=True)
        parser.add_argument("--full-ci-deferred", default="false")
        parser.add_argument("--bvs-changed", default="false")
        parser.add_argument("--job", action="append", default=[])
    if mode == "resolve-gate-carry-forward":
        parser.add_argument("--repo")
        parser.add_argument("--token")
        parser.add_argument("--sha", required=True)
        parser.add_argument("--base-sha", default="")
        parser.add_argument("--current-run-id")
        parser.add_argument("--gate-name", required=True)
        parser.add_argument("--workflow-path", required=True)
        parser.add_argument("--require-provenance-base", default="false")
    if mode == "emit-full-ci":
        parser.add_argument("--output", type=pathlib.Path)
        parser.add_argument("--ci-policy-path", default="full")
        parser.add_argument("--workflow-file", type=pathlib.Path)
        parser.add_argument("--required-job", action="append", default=[])
        parser.add_argument("--conditional-job", action="append", default=[])
        parser.add_argument("--nextest-fingerprint")
    if mode == "validate-record":
        parser.add_argument("--record", type=pathlib.Path, required=True)
    if mode == "resolve-exact-sha":
        parser.add_argument("--repo")
        parser.add_argument("--token")
        parser.add_argument("--sha")
        parser.add_argument("--current-run-id")
    if mode == "resolve-fingerprint":
        parser.add_argument("--repo")
        parser.add_argument("--token")
        parser.add_argument("--current-run-id")
        parser.add_argument("--current-fingerprint")
    return parser


def main(argv: list[str] | None = None) -> int:
    if argv is None:
        argv = sys.argv[1:]
    if not argv or argv[0] in {"-h", "--help"}:
        modes = ", ".join(sorted(SUPPORTED_MODES))
        print(f"Usage: ci_provenance.py <mode> [options]\nSupported modes: {modes}", file=sys.stderr)
        return 2
    mode, rest = argv[0], argv[1:]
    if mode not in SUPPORTED_MODES:
        print(f"ERROR: unknown mode {mode!r}", file=sys.stderr)
        return 2

    parser = parser_for_mode(mode)
    try:
        args = parser.parse_args(rest)
        config = load_config(args.config)
        if mode == "ci-policy":
            result = evaluate_ci_policy(
                config,
                event_name=args.event_name,
                event_action=args.event_action,
                pull_request_draft=parse_bool(args.pull_request_draft),
                pull_request_head_ref=args.pull_request_head_ref,
                pull_request_base_changed=parse_bool(args.pull_request_base_changed),
                workflow_dispatch_full_ci=args.workflow_dispatch_full_ci,
                docs_only=parse_bool(args.docs_only),
                ref=args.ref,
            )
            print(f"ci_policy_path={result.ci_policy_path}")
            print(f"full_ci_required={str(result.full_ci_required).lower()}")
            print(f"full_ci_deferred={str(result.full_ci_deferred).lower()}")
            print(f"gate_name={result.gate_name}")
            print(f"backtester_gate_name={result.backtester_gate_name}")
            print(f"expected_event_class={result.expected_event_class}")
            print(f"reason={result.reason}")
            print(f"ignore_emit_failure={str(config.ignore_emit_failure).lower()}")
        elif mode == "check-ci-gate":
            print(
                evaluate_ci_gate_verdict(
                    policy_path=args.policy_path,
                    expected_event_class=args.expected_event_class,
                    full_ci_deferred=parse_bool(args.full_ci_deferred),
                    ignore_emit_failure=parse_bool(args.ignore_emit_failure),
                    reuse_found=parse_bool(args.reuse_found),
                    carry_forward_verified=parse_bool(args.carry_forward_verified),
                    job_results=parse_job_result_values(args.job),
                    build_required=parse_bool(args.build_required),
                )
            )
        elif mode == "check-backtester-gate":
            print(
                evaluate_backtester_gate_verdict(
                    policy_path=args.policy_path,
                    expected_event_class=args.expected_event_class,
                    full_ci_deferred=parse_bool(args.full_ci_deferred),
                    job_results=parse_job_result_values(args.job),
                    bvs_changed=parse_bool(args.bvs_changed),
                )
            )
        elif mode == "resolve-gate-carry-forward":
            result = resolve_gate_carry_forward(
                repo=args.repo or require_env("GITHUB_REPOSITORY"),
                token=args.token or require_env("GITHUB_TOKEN"),
                requested_sha=args.sha,
                base_sha=args.base_sha,
                current_run_id=args.current_run_id,
                gate_name=args.gate_name,
                workflow_path=args.workflow_path,
                config=config,
                config_path=args.config,
                require_provenance_base=parse_bool(args.require_provenance_base),
            )
            print(f"carry_forward_verified={str(result.carry_forward_verified).lower()}")
            print(f"source_run_id={result.source_run_id}")
            print(f"source_sha={result.source_sha}")
            print(f"gate_name={result.gate_name}")
            print(f"reason={result.reason}")
        elif mode == "emit-full-ci":
            record = emit_full_ci_record(
                config=config,
                config_path=args.config,
                ci_policy_path=args.ci_policy_path,
                workflow_file=args.workflow_file,
                required_job_values=args.required_job,
                conditional_job_values=args.conditional_job,
                nextest_fingerprint=args.nextest_fingerprint,
            )
            encoded = json.dumps(record, sort_keys=True, indent=2) + "\n"
            if args.output is None:
                print(encoded, end="")
            else:
                args.output.write_text(encoded, encoding="utf-8")
                print(f"wrote {args.output}")
        elif mode == "validate-record":
            validate_record_schema(load_json(args.record), config, config_path=args.config)
            print("record valid")
        elif mode == "resolve-exact-sha":
            evidence = resolve_exact_sha_evidence(
                repo=args.repo or require_env("GITHUB_REPOSITORY"),
                token=args.token or require_env("GITHUB_TOKEN"),
                requested_sha=args.sha or require_env("GITHUB_SHA"),
                config=config,
                config_path=args.config,
                current_run_id=args.current_run_id or os.environ.get("GITHUB_RUN_ID"),
            )
            print(json.dumps(evidence.record, sort_keys=True))
        elif mode == "resolve-fingerprint":
            result = resolve_fingerprint_reuse(
                repo=args.repo or require_env("GITHUB_REPOSITORY"),
                token=args.token or require_env("GITHUB_TOKEN"),
                current_fingerprint=args.current_fingerprint,
                current_run_id=args.current_run_id or require_env("GITHUB_RUN_ID"),
                config=config,
                config_path=args.config,
            )
            print(output_resolution_lines(result), end="")
        return 0
    except ProvenanceError as exc:
        print(f"ERROR: {exc}", file=sys.stderr)
        return 1


if __name__ == "__main__":
    raise SystemExit(main())
