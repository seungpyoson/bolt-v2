#!/usr/bin/env python3
"""Tests for merge_queue_preflight.py."""

from __future__ import annotations

import contextlib
import importlib.util
import io
import json
import os
import pathlib
import select
import shutil
import subprocess
import sys
import tempfile
import time


REPO_ROOT = pathlib.Path(__file__).resolve().parents[1]
SCRIPT_PATH = REPO_ROOT / "scripts" / "merge_queue_preflight.py"
MERGIFY_YML = (REPO_ROOT / ".mergify.yml").read_text(encoding="utf-8")
EXPECTED_RESIDUAL_RISKS = [
    "full_ci_result",
    "mergify_proof_pr_behavior",
    "remote_runner_availability",
    "flaky_checks_and_external_services",
    "base_or_head_drift_after_preflight",
    "post_merge_config_or_workflow_changes",
    "queue_metadata_drift",
    "live_queue_ordering",
    "reset_on_external_merge",
    "max_parallel_checks_cost",
]
SOURCE_PR_CHECK_WORKFLOWS = {
    "gate-iteration": "CI",
    "backtester-gate-iteration": "Backtester CI",
    "actionlint": "actionlint",
    "host-health": "CI",
}
SOURCE_CHECK_ALIASES = {
    "gate": "gate-iteration",
    "backtester-gate": "backtester-gate-iteration",
}
MERGIFY_QUEUE_MAX_BATCH_SIZE = {"hotfix": 1, "default": 10}


def expected_head_sha_args(
    origin: pathlib.Path,
    pr_args: tuple[str, ...],
) -> list[str]:
    head_shas = {
        int(pr): git(origin, "rev-parse", f"refs/pull/{pr}/head")
        for pr in pr_args
    }
    return [
        item
        for pr, sha in head_shas.items()
        for item in ("--expected-head-sha", f"{pr}={sha}")
    ]


def run(command: list[str], cwd: pathlib.Path) -> subprocess.CompletedProcess[str]:
    return subprocess.run(
        command,
        cwd=cwd,
        check=True,
        text=True,
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
    )


def git(cwd: pathlib.Path, *args: str) -> str:
    return run(["git", *args], cwd).stdout.strip()


def write(path: pathlib.Path, text: str) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(text, encoding="utf-8")


def commit(repo: pathlib.Path, message: str) -> str:
    git(repo, "add", ".")
    git(repo, "commit", "-m", message)
    return git(repo, "rev-parse", "HEAD")


class GitFixture:
    def __init__(self, root: pathlib.Path) -> None:
        self.root = root
        self.remote = root / "origin.git"
        self.repo = root / "repo"
        git(root, "init", "--bare", str(self.remote))
        git(root, "clone", str(self.remote), str(self.repo))
        git(self.repo, "config", "user.email", "preflight@example.invalid")
        git(self.repo, "config", "user.name", "Merge Queue Preflight Test")
        write(self.repo / "shared.txt", "base\n")
        write(self.repo / ".mergify.yml", MERGIFY_YML)
        self.base = commit(self.repo, "base")
        git(self.repo, "branch", "-M", "main")
        git(self.repo, "push", "origin", "main")

    def make_pr(self, number: int, edits: dict[str, str]) -> str:
        branch = f"pr-{number}"
        git(self.repo, "checkout", "-B", branch, "main")
        for path, text in edits.items():
            write(self.repo / path, text)
        head = commit(self.repo, f"pr {number}")
        git(self.repo, "push", "origin", f"HEAD:refs/pull/{number}/head")
        git(self.repo, "checkout", "main")
        return head


def run_preflight(
    repo: pathlib.Path,
    origin: pathlib.Path,
    *args: str,
    expect_success: bool = True,
    expected_base_sha: str | None = None,
) -> tuple[int, str, str]:
    command = [
        sys.executable,
        str(SCRIPT_PATH),
        "--origin",
        str(origin),
        "--base",
        "main",
        "--expected-base-sha",
        expected_base_sha or git(repo, "rev-parse", "main"),
        *expected_head_sha_args(origin, args),
        "--no-gh",
        "--verifier-profile",
        "none",
        "--json",
        *args,
    ]
    result = subprocess.run(
        command,
        cwd=repo,
        text=True,
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
        check=False,
    )
    if expect_success and result.returncode != 0:
        raise AssertionError(
            f"preflight failed unexpectedly: rc={result.returncode}\nSTDOUT:\n{result.stdout}\nSTDERR:\n{result.stderr}"
        )
    if not expect_success and result.returncode == 0:
        raise AssertionError(f"preflight unexpectedly passed: {result.stdout}")
    return result.returncode, result.stdout, result.stderr


def parse_json(stdout: str) -> dict[str, object]:
    payload = json.loads(stdout)
    if not isinstance(payload, dict):
        raise AssertionError(payload)
    return payload


def assert_equal(actual: object, expected: object, label: str) -> None:
    assert actual == expected, (label, actual, expected)


def no_gh_finding() -> dict[str, object]:
    return {
        "lane": "readiness",
        "scope": "run",
        "status": "inconclusive",
        "reason_code": "readiness_disabled_by_no_gh",
        "message": "--no-gh disables authoritative readiness evidence",
        "evidence": {"use_gh": False},
    }


def residual_risk_findings() -> list[dict[str, object]]:
    return [
        {
            "lane": "residual_risk",
            "scope": "run",
            "status": "residual_risk",
            "reason_code": reason_code,
            "message": reason_code,
            "evidence": {},
        }
        for reason_code in EXPECTED_RESIDUAL_RISKS
    ]


def mergify_config_finding(base_sha: str, blob_sha: str) -> dict[str, object]:
    return {
        "lane": "mergify_config",
        "scope": "run",
        "status": "ready",
        "reason_code": "mergify_config_snapshot_read",
        "message": ".mergify.yml snapshot read from expected base",
        "evidence": {
            "path": ".mergify.yml",
            "base_sha": base_sha,
            "blob_sha": blob_sha,
            "git_returncode": 0,
            "git_stderr": "",
        },
    }


def mergify_config_valid_finding(base_sha: str, blob_sha: str) -> dict[str, object]:
    return {
        "lane": "mergify_config",
        "scope": "run",
        "status": "ready",
        "reason_code": "mergify_config_valid",
        "message": ".mergify.yml snapshot satisfies Mergify config contract",
        "evidence": {
            "path": ".mergify.yml",
            "base_sha": base_sha,
            "blob_sha": blob_sha,
            "validator": "verify_ci_workflow_hygiene.verify_mergify_config",
            "git_returncode": 0,
            "git_stderr": "",
            "errors": [],
        },
    }


def mergify_queue_route_finding(pr: int, queue_rule: str, labels: list[str], queue_conditions: list[str]) -> dict[str, object]:
    return {
        "lane": "mergify_config",
        "scope": "pr",
        "status": "ready",
        "reason_code": "mergify_queue_route_selected",
        "message": f"PR #{pr} routes to Mergify queue rule {queue_rule}",
        "evidence": {
            "pr": pr,
            "queue_rule": queue_rule,
            "labels": labels,
            "queue_conditions": queue_conditions,
            "max_batch_size": MERGIFY_QUEUE_MAX_BATCH_SIZE[queue_rule],
        },
    }


def mergify_queue_proof_source_finding(
    queue_rule: str,
    queue_conditions: list[str],
    merge_conditions: list[str],
) -> dict[str, object]:
    return {
        "lane": "mergify_config",
        "scope": "queue",
        "status": "ready",
        "reason_code": "mergify_queue_proof_source",
        "message": f"Mergify queue rule {queue_rule} uses queue proof context",
        "evidence": {
            "queue_rule": queue_rule,
            "proof_source": "queue_proof_pr",
            "queue_conditions": queue_conditions,
            "merge_conditions": merge_conditions,
        },
    }


def mergify_required_reviewer_finding(
    queue_rule: str,
    reviewers: list[str],
    merge_conditions: list[str],
) -> dict[str, object]:
    return {
        "lane": "mergify_config",
        "scope": "queue",
        "status": "ready",
        "reason_code": "mergify_required_reviewer",
        "message": f"Mergify queue rule {queue_rule} requires review from {', '.join(reviewers)}",
        "evidence": {
            "queue_rule": queue_rule,
            "reviewers": reviewers,
            "merge_conditions": merge_conditions,
        },
    }


def mergify_queue_batch_above_max_finding(queue_rule: str, prs: list[int], max_batch_size: int) -> dict[str, object]:
    return {
        "lane": "mergify_config",
        "scope": "queue",
        "status": "ready",
        "reason_code": "mergify_queue_batch_above_max",
        "message": f"Mergify queue rule {queue_rule} selected {len(prs)} PRs above max batch size {max_batch_size}",
        "evidence": {
            "queue_rule": queue_rule,
            "prs": prs,
            "selected_count": len(prs),
            "max_batch_size": max_batch_size,
        },
    }


def mergify_queue_batch_below_min_finding(
    queue_rule: str,
    prs: list[int],
    min_batch_size: int,
    batch_max_wait_time: str,
) -> dict[str, object]:
    return {
        "lane": "mergify_config",
        "scope": "queue",
        "status": "ready",
        "reason_code": "mergify_queue_batch_below_min_wait",
        "message": f"Mergify queue rule {queue_rule} selected {len(prs)} PRs below min batch size {min_batch_size}",
        "evidence": {
            "queue_rule": queue_rule,
            "prs": prs,
            "selected_count": len(prs),
            "min_batch_size": min_batch_size,
            "batch_max_wait_time": batch_max_wait_time,
        },
    }


def stale_base_finding(expected_base_sha: str, actual_base_sha: str) -> dict[str, object]:
    return {
        "lane": "identity",
        "scope": "run",
        "status": "inconclusive",
        "reason_code": "stale_base",
        "message": "expected base SHA differs from live base branch",
        "evidence": {
            "expected_base_sha": expected_base_sha,
            "actual_base_sha": actual_base_sha,
        },
    }


def matching_base_finding(base_sha: str) -> dict[str, object]:
    return {
        "lane": "identity",
        "scope": "run",
        "status": "ready",
        "reason_code": "base_identity_ready",
        "message": "expected base SHA matches live base branch",
        "evidence": {
            "expected_base_sha": base_sha,
            "actual_base_sha": base_sha,
        },
    }


def matching_head_finding(pr: int, head_sha: str) -> dict[str, object]:
    return {
        "lane": "identity",
        "scope": "pr",
        "status": "ready",
        "reason_code": "head_identity_ready",
        "message": "expected PR head SHA matches fetched PR head",
        "evidence": {
            "pr": pr,
            "expected_head_sha": head_sha,
            "actual_head_sha": head_sha,
        },
    }


def integration_batch_ready_finding(batch: dict[str, object]) -> dict[str, object]:
    return {
        "lane": "integration",
        "scope": "batch",
        "status": "ready",
        "reason_code": "integration_batch_ready",
        "message": f"batch {batch['index']} synthetic merge is conflict-free",
        "evidence": {
            "index": batch["index"],
            "prs": batch["prs"],
        },
    }


def verifier_batch_ready_finding(batch: dict[str, object]) -> dict[str, object]:
    return {
        "lane": "verifier",
        "scope": "batch",
        "status": "ready",
        "reason_code": "verifier_batch_ready",
        "message": f"batch {batch['index']} verifier commands passed",
        "evidence": {
            "index": batch["index"],
            "prs": batch["prs"],
            "verifiers": batch["verifiers"],
        },
    }


def readiness_ready_finding(
    pr: int,
    head: str,
    checks: list[dict[str, object]] | None = None,
) -> dict[str, object]:
    return {
        "lane": "readiness",
        "scope": "pr",
        "status": "ready",
        "reason_code": "readiness_ready",
        "message": f"PR #{pr} has authoritative readiness metadata with no warnings",
        "evidence": {
            "pr": pr,
            "baseRefName": "main",
            "headRefOid": head,
            "mergeable": "MERGEABLE",
            "reviewDecision": "APPROVED",
            "checks": checks or [],
        },
    }


def stale_head_finding(pr: int, expected_head_sha: str, actual_head_sha: str) -> dict[str, object]:
    return {
        "lane": "identity",
        "scope": "pr",
        "status": "blocked",
        "reason_code": "stale_head",
        "message": "expected PR head SHA differs from fetched PR head",
        "evidence": {
            "pr": pr,
            "expected_head_sha": expected_head_sha,
            "actual_head_sha": actual_head_sha,
        },
    }


def load_preflight_module() -> object:
    spec = importlib.util.spec_from_file_location("merge_queue_preflight", SCRIPT_PATH)
    if spec is None or spec.loader is None:
        raise AssertionError("merge_queue_preflight module spec unavailable")
    module = importlib.util.module_from_spec(spec)
    sys.modules[spec.name] = module
    spec.loader.exec_module(module)
    return module


def assert_contract_result_reduces_findings_by_table() -> None:
    module = load_preflight_module()
    findings = [
        {
            "lane": "readiness",
            "scope": "pr",
            "status": "inconclusive",
            "reason_code": "metadata_unavailable",
            "message": "metadata missing",
            "evidence": {"pr": 1},
        },
        {
            "lane": "integration",
            "scope": "pr",
            "status": "blocked",
            "reason_code": "base_conflict",
            "message": "conflict",
            "evidence": {"pr": 2},
        },
        {
            "lane": "residual_risk",
            "scope": "run",
            "status": "residual_risk",
            "reason_code": "proof_only_check",
            "message": "proof context not yet run",
            "evidence": {},
        },
    ]
    result = module.contract_result(findings, wave_status="ready")
    expected_lane_statuses = {
        "mergify_config": "inconclusive",
        "identity": "inconclusive",
        "readiness": "inconclusive",
        "integration": "blocked",
        "verifier": "inconclusive",
    }
    if result != {
        "verdict": "blocked",
        "exit_code": 2,
        "lane_statuses": expected_lane_statuses,
    }:
        raise AssertionError(result)
    if module.contract_result([], wave_status="ready") != {
        "verdict": "inconclusive",
        "exit_code": 3,
        "lane_statuses": {
            "mergify_config": "inconclusive",
            "identity": "inconclusive",
            "readiness": "inconclusive",
            "integration": "inconclusive",
            "verifier": "inconclusive",
        },
    }:
        raise AssertionError("missing ready evidence must be inconclusive")
    ready_findings = [
        {
            "lane": lane,
            "scope": "run",
            "status": "ready",
            "reason_code": f"{lane}_ready",
            "message": "ready",
            "evidence": {},
        }
        for lane in expected_lane_statuses
    ]
    ready_result = module.contract_result(ready_findings, wave_status="ready")
    if (ready_result["verdict"], ready_result["exit_code"]) != ("queue_as_one_wave", 0):
        raise AssertionError(ready_result)
    split_result = module.contract_result(ready_findings, wave_status="split_advised")
    if (split_result["verdict"], split_result["exit_code"]) != ("split_advised", 1):
        raise AssertionError(split_result)


def assert_preflight_input_timeout_is_config_driven() -> None:
    module = load_preflight_module()
    with tempfile.TemporaryDirectory() as tmp:
        config = write_preflight_config(
            pathlib.Path(tmp),
            "none",
            [],
            input_timeout_seconds=17,
        )
        loaded = module.load_config(config)
    assert_equal(loaded.input_timeout_seconds, 17, "input timeout config")


def assert_source_check_alias_targets_must_have_workflows() -> None:
    module = load_preflight_module()
    with tempfile.TemporaryDirectory() as tmp:
        config = write_preflight_config(pathlib.Path(tmp), "none", [])
        write(
            config,
            config.read_text(encoding="utf-8").replace(
                '"gate" = "gate-iteration"',
                '"gate" = "missing-source-check"',
            ),
        )
        try:
            module.load_config(config)
        except module.PreflightError as exc:
            expected = (
                "config.merge_queue_preflight.source_check_aliases.gate target "
                "'missing-source-check' must exist in "
                "config.merge_queue_preflight.required_check_workflows"
            )
            if expected not in str(exc):
                raise AssertionError(str(exc))
        else:
            raise AssertionError("missing source check workflow did not fail config load")


def assert_source_check_evidence_fallback_is_precise() -> None:
    module = load_preflight_module()
    metadata = {"headRefOid": "1" * 40}
    check = passing_source_pr_check("gate-iteration")
    legacy_fallback = module.mergify_required_check_finding(
        merge_check="gate",
        source_check="gate-iteration",
        readiness={"metadata": metadata, "checks": [check]},
        expected_workflow="CI",
    )
    assert_equal(legacy_fallback, None, "missing source_checks falls back to checks")

    for label, source_checks in (("empty", []), ("none", None)):
        finding = module.mergify_required_check_finding(
            merge_check="gate",
            source_check="gate-iteration",
            readiness={
                "metadata": metadata,
                "checks": [check],
                "source_checks": source_checks,
            },
            expected_workflow="CI",
        )
        if finding is None:
            raise AssertionError(f"{label} source_checks must fail closed")
        evidence = finding["evidence"]
        assert_equal(finding["reason_code"], "required_check_missing", f"{label} source_checks reason")
        assert_equal(evidence["check_name"], "gate-iteration", f"{label} source check name")
        assert_equal(evidence["merge_condition_check"], "gate", f"{label} merge condition")


def assert_git_and_gh_use_input_timeout() -> None:
    module = load_preflight_module()
    calls: list[dict[str, object]] = []
    original_run_command = module.run_command

    def fake_run_command(args: list[str], **kwargs: object) -> object:
        calls.append({"args": tuple(args), **kwargs})
        return module.CommandResult(tuple(args), 0, "{}", "")

    module.run_command = fake_run_command
    try:
        module.git(REPO_ROOT, "status", timeout_seconds=17)
        module.gh_json(["pr", "view", "1"], timeout_seconds=17)
    finally:
        module.run_command = original_run_command
    assert_equal(calls[0]["args"], ("git", "status"), "git command args")
    assert_equal(calls[0]["timeout_seconds"], 17, "git timeout")
    assert_equal(calls[1]["args"], ("gh", "pr", "view", "1"), "gh command args")
    assert_equal(calls[1]["timeout_seconds"], 17, "gh timeout")
    assert_equal(calls[1]["process_group"], True, "gh process group")


def assert_gh_timeout_is_preflight_error() -> None:
    module = load_preflight_module()
    original_run_command = module.run_command

    def fake_run_command(args: list[str], **_kwargs: object) -> object:
        return module.CommandResult(
            tuple(args),
            -9,
            "",
            "command timed out after 17 seconds\n",
            failure_type="timeout",
        )

    module.run_command = fake_run_command
    try:
        try:
            module.gh_json(["pr", "view", "1"], timeout_seconds=17)
        except module.PreflightError as exc:
            assert "timed out after 17 seconds" in str(exc), exc
        else:
            raise AssertionError("gh timeout did not raise PreflightError")
    finally:
        module.run_command = original_run_command


def assert_merge_tree_timeout_is_preflight_error() -> None:
    module = load_preflight_module()
    original_git = module.git

    def fake_git(_repo: pathlib.Path, *args: str, **_kwargs: object) -> object:
        return module.CommandResult(
            ("git", *args),
            -9,
            "",
            "command timed out after 17 seconds\n",
            failure_type="timeout",
        )

    module.git = fake_git
    try:
        try:
            module.merge_tree(REPO_ROOT, "a" * 40, "b" * 40, 17)
        except module.PreflightError as exc:
            assert "git merge-tree timed out after 17 seconds" in str(exc), exc
        else:
            raise AssertionError("merge-tree timeout did not raise PreflightError")
    finally:
        module.git = original_git


def assert_check_state_classification_is_table_driven() -> None:
    module = load_preflight_module()
    cases = {
        "success": ("ready", "required_check_ready"),
        "pass": ("ready", "required_check_ready"),
        "failure": ("blocked", "required_check_failed"),
        "error": ("blocked", "required_check_failed"),
        "cancelled": ("blocked", "required_check_failed"),
        "action-required": ("blocked", "required_check_failed"),
        "startup failure": ("blocked", "required_check_failed"),
        "pending": ("inconclusive", "required_check_pending"),
        "queued": ("inconclusive", "required_check_pending"),
        "requested": ("inconclusive", "required_check_pending"),
        "waiting": ("inconclusive", "required_check_pending"),
        "in-progress": ("inconclusive", "required_check_pending"),
        "skipped": ("inconclusive", "required_check_skipped"),
        "neutral": ("inconclusive", "required_check_skipped"),
        "missing": ("inconclusive", "required_check_missing"),
        "unknown-new-state": ("inconclusive", "required_check_unknown"),
    }
    for raw_state, expected in cases.items():
        finding = module.classify_required_check_state(
            check_name="gate",
            raw_state=raw_state,
            expected_head="a" * 40,
            actual_head="a" * 40,
            evidence={"workflow": "CI", "job": "gate"},
        )
        observed = (finding["status"], finding["reason_code"])
        if observed != expected:
            raise AssertionError((raw_state, observed, expected))
    stale = module.classify_required_check_state(
        check_name="gate",
        raw_state="success",
        expected_head="a" * 40,
        actual_head="b" * 40,
        evidence={"workflow": "CI", "job": "gate"},
    )
    if (stale["status"], stale["reason_code"]) != ("inconclusive", "required_check_stale"):
        raise AssertionError(stale)


def assert_input_failure_matrix_is_declarative() -> None:
    module = load_preflight_module()
    expected = {
        "absent_input": ("usage_error", "preflight_usage_error", 4),
        "absent_evidence": ("lane_finding", "inconclusive", 3),
        "empty_input": ("usage_error", "preflight_usage_error", 4),
        "invalid": ("usage_error", "preflight_usage_error", 4),
        "stale_base": ("lane_finding", "inconclusive", 3),
        "stale_head": ("lane_finding", "blocked", 2),
        "unavailable": ("lane_finding", "inconclusive", 3),
        "timeout": ("lane_finding", "inconclusive", 3),
        "ambiguous": ("lane_finding", "inconclusive", 3),
    }
    if module.INPUT_FAILURE_CLASSIFICATIONS != expected:
        raise AssertionError(module.INPUT_FAILURE_CLASSIFICATIONS)


def assert_mergify_config_field_handling_is_declarative() -> None:
    module = load_preflight_module()
    expected = {
        "merge_queue.max_parallel_checks": "residual_cost_impact",
        "merge_queue.reset_on_external_merge": "residual_post_preflight_invalidation",
        "queue_rules[].name": "required_unique_queue_identity",
        "queue_rules[].queue_conditions": "effective_pr_to_queue_routing",
        "queue_rules[].merge_conditions": "required_reviewer_and_check_evidence",
        "queue_rules[].branch_protection_injection_mode": "explicit_support_or_inconclusive",
        "queue_rules[].batch_size": "batch_min_max_scalar_model",
        "queue_rules[].batch_max_wait_time": "below_min_wait_model",
        "queue_rules[].batch_max_failure_resolution_attempts": "explicit_support_or_inconclusive",
        "queue_rules[].checks_timeout": "residual_proof_time_risk",
        "queue_rules[].draft_bot_account": "explicit_support_or_inconclusive",
        "queue_rules[].merge_method": "explicit_support_or_inconclusive",
        "priority_rules[].conditions": "effective_routing_priority_conditions",
        "priority_rules[].name": "required_unique_priority_identity",
        "priority_rules[].priority": "residual_live_order_risk",
        "priority_rules[].allow_checks_interruption": "residual_interruption_risk",
    }
    if module.MERGIFY_CONFIG_FIELD_HANDLING != expected:
        raise AssertionError(module.MERGIFY_CONFIG_FIELD_HANDLING)


def assert_preflight_artifact_classification_is_declarative() -> None:
    module = load_preflight_module()
    expected = {
        "base_conflict": ("integration", "pr", "blocked"),
        "batch_conflict": ("integration", "batch", "ready"),
        "batch_verifier_failed": ("verifier", "batch", "ready"),
        "batch_verifier_timeout": ("verifier", "batch", "inconclusive"),
        "batch_verifier_unavailable": ("verifier", "batch", "inconclusive"),
        "base_mismatch": ("identity", "pr", "inconclusive"),
        "head_mismatch": ("identity", "pr", "blocked"),
        "head_fetch_failed": ("identity", "pr", "inconclusive"),
        "head_unavailable": ("identity", "pr", "inconclusive"),
        "metadata_unavailable": ("readiness", "pr", "inconclusive"),
        "required_check_failed": ("readiness", "pr", "blocked"),
        "required_check_pending": ("readiness", "pr", "inconclusive"),
        "required_check_skipped": ("readiness", "pr", "inconclusive"),
        "required_check_missing": ("readiness", "pr", "inconclusive"),
        "required_check_unknown": ("readiness", "pr", "inconclusive"),
        "required_check_stale": ("readiness", "pr", "inconclusive"),
        "required_check_wrong_workflow": ("readiness", "pr", "inconclusive"),
        "readiness_failed": ("readiness", "pr", "blocked"),
        "verifier_failed": ("verifier", "pr", "blocked"),
        "verifier_timeout": ("verifier", "pr", "inconclusive"),
        "verifier_unavailable": ("verifier", "pr", "inconclusive"),
    }
    if module.PREFLIGHT_ARTIFACT_CLASSIFICATIONS != expected:
        raise AssertionError(module.PREFLIGHT_ARTIFACT_CLASSIFICATIONS)


def assert_preflight_artifact_finding_uses_classification_table() -> None:
    module = load_preflight_module()
    artifact = {
        "type": "batch_conflict",
        "pr": 2,
        "against_batch": [1],
        "files": ["shared.txt"],
    }
    expected = {
        "lane": "integration",
        "scope": "batch",
        "status": "ready",
        "reason_code": "batch_conflict",
        "message": "batch_conflict",
        "evidence": artifact,
    }
    finding = module.preflight_artifact_finding(artifact)
    if finding != expected:
        raise AssertionError(finding)


def ready_contract_findings() -> tuple[dict[str, object], ...]:
    return tuple(
        {
            "lane": lane,
            "scope": "run",
            "status": "ready",
            "reason_code": f"{lane}_ready",
            "message": "ready",
            "evidence": {},
        }
        for lane in ("mergify_config", "identity", "readiness", "integration", "verifier")
    )


def assert_contract_evaluator_reduces_normalized_evidence() -> None:
    module = load_preflight_module()
    ready = ready_contract_findings()
    no_gh_finding = {
        "lane": "readiness",
        "scope": "run",
        "status": "inconclusive",
        "reason_code": "readiness_disabled_by_no_gh",
        "message": "--no-gh disables authoritative readiness evidence",
        "evidence": {"use_gh": False},
    }
    scenarios = {
        "clean_authoritative": (
            module.ContractEvidence(findings=ready, artifacts=(), wave_status="ready"),
            ("queue_as_one_wave", 0),
        ),
        "no_gh_inconclusive": (
            module.ContractEvidence(findings=(*ready, no_gh_finding), artifacts=(), wave_status="ready"),
            ("inconclusive", 3),
        ),
        "base_conflict_blocked": (
            module.ContractEvidence(
                findings=ready,
                artifacts=({"type": "base_conflict", "pr": 2, "reason": "conflicts with base"},),
                wave_status="ready",
            ),
            ("blocked", 2),
        ),
        "batch_conflict_split_advised": (
            module.ContractEvidence(
                findings=ready,
                artifacts=({"type": "batch_conflict", "pr": 2, "against_batch": [1]},),
                wave_status="split_advised",
            ),
            ("split_advised", 1),
        ),
        "metadata_unavailable_inconclusive": (
            module.ContractEvidence(
                findings=ready,
                artifacts=({"type": "metadata_unavailable", "pr": 1, "reason": "gh unavailable"},),
                wave_status="ready",
            ),
            ("inconclusive", 3),
        ),
        "required_check_pending_inconclusive": (
            module.ContractEvidence(
                findings=ready,
                artifacts=({"type": "required_check_pending", "pr": 1, "reason": "pending"},),
                wave_status="ready",
            ),
            ("inconclusive", 3),
        ),
        "base_mismatch_inconclusive": (
            module.ContractEvidence(
                findings=ready,
                artifacts=({"type": "base_mismatch", "pr": 1, "reason": "wrong base"},),
                wave_status="ready",
            ),
            ("inconclusive", 3),
        ),
    }
    for name, (evidence, expected) in scenarios.items():
        evaluation = module.evaluate_preflight_contract(evidence)
        observed = (evaluation["verdict"], evaluation["exit_code"])
        if observed != expected:
            raise AssertionError((name, observed, expected, evaluation))


def write_preflight_config(
    root: pathlib.Path,
    profile: str,
    commands: list[str],
    *,
    verifier_stream_max_lines: int = 40,
    verifier_stream_max_bytes: int = 4000,
    input_timeout_seconds: int = 30,
    verifier_timeout_seconds: int = 60,
) -> pathlib.Path:
    rendered_commands = ", ".join(json.dumps(command) for command in commands)
    rendered_workflows = "\n".join(
        f"{json.dumps(name)} = {json.dumps(workflow)}"
        for name, workflow in SOURCE_PR_CHECK_WORKFLOWS.items()
    )
    rendered_aliases = "\n".join(
        f"{json.dumps(name)} = {json.dumps(alias)}"
        for name, alias in SOURCE_CHECK_ALIASES.items()
    )
    path = root / "preflight.toml"
    write(
        path,
        "[merge_queue_preflight]\n"
        'origin = "origin"\n'
        'base = "main"\n'
        f"default_verifier_profile = {json.dumps(profile)}\n\n"
        "[merge_queue_preflight.timeouts]\n"
        f"input_seconds = {input_timeout_seconds}\n"
        f"verifier_seconds = {verifier_timeout_seconds}\n\n"
        "[merge_queue_preflight.required_check_workflows]\n"
        f"{rendered_workflows}\n\n"
        "[merge_queue_preflight.source_check_aliases]\n"
        f"{rendered_aliases}\n\n"
        "[merge_queue_preflight.output]\n"
        f"verifier_stream_max_lines = {verifier_stream_max_lines}\n"
        f"verifier_stream_max_bytes = {verifier_stream_max_bytes}\n\n"
        f"[merge_queue_preflight.verifier_profiles.{profile}]\n"
        f"commands = [{rendered_commands}]\n",
    )
    return path


def write_fake_gh(
    root: pathlib.Path,
    *,
    views: dict[int, dict[str, object]],
    checks: dict[int, list[dict[str, object]]] | None = None,
    required_checks: dict[int, list[dict[str, object]]] | None = None,
    failed_views: dict[int, str] | None = None,
    check_exit_codes: dict[int, int] | None = None,
) -> pathlib.Path:
    bin_dir = root / "bin"
    bin_dir.mkdir()
    path = bin_dir / "gh"
    checks_by_pr = {pr: [] for pr in views} | (checks or {})
    required_checks_by_pr = checks_by_pr | (required_checks or {})
    check_exit_codes_by_pr = {pr: 0 for pr in views} | (check_exit_codes or {})
    write(
        path,
        "#!/usr/bin/env python3\n"
        "import json\n"
        "import sys\n"
        f"views = {views!r}\n"
        f"checks = {checks_by_pr!r}\n"
        f"required_checks = {required_checks_by_pr!r}\n"
        f"failed_views = {(failed_views or {})!r}\n"
        f"check_exit_codes = {check_exit_codes_by_pr!r}\n"
        "args = sys.argv[1:]\n"
        "if len(args) >= 3 and args[0:2] == ['pr', 'view']:\n"
        "    if int(args[2]) in failed_views:\n"
        "        print(failed_views[int(args[2])], file=sys.stderr)\n"
        "        raise SystemExit(1)\n"
        "    print(json.dumps(views[int(args[2])]))\n"
        "elif len(args) >= 3 and args[0:2] == ['pr', 'checks']:\n"
        "    pr = int(args[2])\n"
        "    selected = required_checks if '--required' in args else checks\n"
        "    print(json.dumps(selected[pr]))\n"
        "    raise SystemExit(check_exit_codes[pr])\n"
        "else:\n"
        "    raise SystemExit(f'unexpected gh args: {args}')\n",
    )
    path.chmod(0o755)
    return bin_dir


def approved_pr_view(
    head: str,
    *,
    base: str = "main",
    labels: tuple[str, ...] = (),
    approving_reviewers: tuple[str, ...] = ("sp-reviewer",),
) -> dict[str, object]:
    return {
        "number": 1,
        "state": "OPEN",
        "isDraft": False,
        "mergeable": "MERGEABLE",
        "reviewDecision": "APPROVED",
        "headRefOid": head,
        "baseRefName": base,
        "labels": [{"name": label} for label in labels],
        "reviews": [
            {"author": {"login": reviewer}, "state": "APPROVED"}
            for reviewer in approving_reviewers
        ],
        "title": "one",
        "url": "https://example.invalid/pull/1",
    }


def passing_source_pr_check(name: str = "gate-iteration") -> dict[str, object]:
    return {
        "name": name,
        "state": "SUCCESS",
        "bucket": "pass",
        "workflow": SOURCE_PR_CHECK_WORKFLOWS[name],
    }


def passing_source_pr_checks(*prs: int) -> dict[int, list[dict[str, object]]]:
    return {
        pr: [
            passing_source_pr_check(name)
            for name in SOURCE_PR_CHECK_WORKFLOWS
        ]
        for pr in prs
    }


def passing_source_pr_branch_required_checks() -> list[dict[str, object]]:
    return [
        passing_source_pr_check("actionlint"),
        passing_source_pr_check("host-health"),
    ]


def run_preflight_with_gh(
    repo: pathlib.Path,
    origin: pathlib.Path,
    bin_dir: pathlib.Path,
    *prs: str,
    expected_base_sha: str | None = None,
) -> subprocess.CompletedProcess[str]:
    command = [
        sys.executable,
        str(SCRIPT_PATH),
        "--origin",
        str(origin),
        "--base",
        "main",
        "--expected-base-sha",
        expected_base_sha or git(repo, "rev-parse", "main"),
        *expected_head_sha_args(origin, prs),
        "--verifier-profile",
        "none",
        "--json",
        *prs,
    ]
    env = os.environ.copy()
    env["PATH"] = f"{bin_dir}{os.pathsep}{env['PATH']}"
    return subprocess.run(
        command,
        cwd=repo,
        env=env,
        text=True,
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
        check=False,
    )


def assert_mergify_config_snapshot_uses_base_blob() -> None:
    with tempfile.TemporaryDirectory() as tmp:
        fixture = GitFixture(pathlib.Path(tmp))
        fixture.make_pr(1, {"one.txt": "one\n"})
        base_blob = git(fixture.repo, "rev-parse", f"{fixture.base}:.mergify.yml")
        write(fixture.repo / ".mergify.yml", "not: [valid\n")

        rc, stdout, _ = run_preflight(
            fixture.repo,
            fixture.remote,
            "1",
            expect_success=False,
        )
        assert_equal(rc, 3, "mergify snapshot no-gh rc")
        payload = parse_json(stdout)
        assert mergify_config_finding(fixture.base, base_blob) in payload["findings"], payload["findings"]
        assert mergify_config_valid_finding(fixture.base, base_blob) in payload["findings"], payload["findings"]


def assert_fetches_use_private_refs_without_fetch_head() -> None:
    source = SCRIPT_PATH.read_text(encoding="utf-8")
    if "FETCH_HEAD" in source:
        raise AssertionError("merge_queue_preflight.py must not read FETCH_HEAD")
    if "--no-tags" not in source:
        raise AssertionError("merge_queue_preflight.py must not mutate shared tag refs")
    with tempfile.TemporaryDirectory() as tmp:
        fixture = GitFixture(pathlib.Path(tmp))
        fixture.make_pr(1, {"one.txt": "one\n"})
        remote_only_tag = "preflight-remote-base"
        git(fixture.repo, "tag", remote_only_tag, fixture.base)
        git(fixture.repo, "push", "origin", f"refs/tags/{remote_only_tag}")
        git(fixture.repo, "tag", "-d", remote_only_tag)
        fetch_head = fixture.repo / ".git" / "FETCH_HEAD"
        sentinel = "sentinel fetch head\n"
        write(fetch_head, sentinel)
        rc, stdout, _ = run_preflight(
            fixture.repo,
            fixture.remote,
            "1",
            expect_success=False,
        )
        payload = parse_json(stdout)
        assert_equal(rc, 3, "private fetch no-gh rc")
        assert_equal(set(payload["pr_heads"].keys()), {"1"}, "private fetch pr heads")
        assert_equal(fetch_head.read_text(encoding="utf-8"), sentinel, "FETCH_HEAD must not change")
        fetched_tag = git(fixture.repo, "tag", "--list", remote_only_tag)
        assert_equal(fetched_tag, "", "private fetch must not auto-follow remote tags")
        leaked_refs = git(fixture.repo, "for-each-ref", "--format=%(refname)", "refs/preflight")
        assert_equal(leaked_refs, "", "private fetch refs must be cleaned up")


def assert_unsupported_mergify_queue_condition_does_not_match() -> None:
    module = load_preflight_module()
    assert_equal(
        module.mergify_queue_condition_labels({"queue_conditions": []}),
        frozenset(),
        "empty Mergify queue conditions",
    )
    assert_equal(
        module.mergify_queue_condition_labels({"queue_conditions": ["label = hotfix"]}),
        frozenset({"hotfix"}),
        "label Mergify queue condition",
    )
    assert_equal(
        module.mergify_queue_condition_labels({"queue_conditions": 5}),
        None,
        "scalar Mergify queue conditions",
    )
    assert_equal(
        module.selected_mergify_queue_rule(
            {"queue_rules": [{"name": "unsupported", "queue_conditions": ["author = bot"]}]},
            (),
        ),
        None,
        "unsupported Mergify queue condition",
    )
    assert_equal(
        module.selected_mergify_queue_rule(
            {
                "queue_rules": [
                    {"name": "unsupported", "queue_conditions": ["author = bot"]},
                    {"name": "default", "queue_conditions": []},
                ]
            },
            (),
        ),
        None,
        "unsupported Mergify queue condition before default",
    )


def assert_unsupported_mergify_queue_condition_route_is_inconclusive() -> None:
    module = load_preflight_module()
    findings = module.available_mergify_config_route_and_batch_findings(
        config={
            "queue_rules": [
                {
                    "name": "unsupported",
                    "queue_conditions": ["author = bot"],
                    "merge_conditions": [],
                    "batch_size": 1,
                    "batch_max_wait_time": "1 minute",
                },
                {
                    "name": "default",
                    "queue_conditions": [],
                    "merge_conditions": [],
                    "batch_size": 1,
                    "batch_max_wait_time": "1 minute",
                }
            ]
        },
        readiness=[
            {
                "pr": 1,
                "metadata": {"labels": []},
                "checks": [],
            }
        ],
        required_check_workflows=SOURCE_PR_CHECK_WORKFLOWS,
        source_check_aliases=SOURCE_CHECK_ALIASES,
    )
    assert_equal(
        findings,
        (
            {
                "lane": "mergify_config",
                "scope": "pr",
                "status": "inconclusive",
                "reason_code": "mergify_queue_route_unavailable",
                "message": "PR #1 does not match a supported Mergify queue rule",
                "evidence": {"pr": 1, "labels": []},
            },
        ),
        "unsupported Mergify route production findings",
    )


def assert_mergify_queue_routing_uses_pr_labels() -> None:
    with tempfile.TemporaryDirectory() as tmp:
        root = pathlib.Path(tmp)
        fixture = GitFixture(root)
        hotfix_head = fixture.make_pr(1, {"hotfix.txt": "hotfix\n"})
        default_head = fixture.make_pr(2, {"default.txt": "default\n"})
        bin_dir = write_fake_gh(
            root,
            views={
                1: approved_pr_view(hotfix_head, labels=("hotfix",)),
                2: approved_pr_view(default_head),
            },
            checks=passing_source_pr_checks(1, 2),
        )
        result = run_preflight_with_gh(fixture.repo, fixture.remote, bin_dir, "1", "2")
        assert_equal(result.returncode, 1, "queue routing rc")
        payload = parse_json(result.stdout)
        assert readiness_ready_finding(1, hotfix_head, passing_source_pr_checks(1)[1]) in payload["findings"], (
            payload["findings"]
        )
        assert readiness_ready_finding(2, default_head, passing_source_pr_checks(2)[2]) in payload["findings"], (
            payload["findings"]
        )
        assert mergify_queue_route_finding(1, "hotfix", ["hotfix"], ["label = hotfix"]) in payload["findings"], (
            payload["findings"]
        )
        assert mergify_queue_route_finding(2, "default", [], []) in payload["findings"], payload["findings"]
        assert mergify_queue_proof_source_finding(
            "hotfix",
            ["label = hotfix"],
            [
                "approved-reviews-by = sp-reviewer",
                "check-success = gate",
                "check-success = backtester-gate",
                "check-success = actionlint",
                "check-success = host-health",
            ],
        ) in payload["findings"], payload["findings"]
        assert mergify_required_reviewer_finding(
            "hotfix",
            ["sp-reviewer"],
            [
                "approved-reviews-by = sp-reviewer",
                "check-success = gate",
                "check-success = backtester-gate",
                "check-success = actionlint",
                "check-success = host-health",
            ],
        ) in payload["findings"], payload["findings"]
        assert mergify_queue_proof_source_finding(
            "default",
            [],
            [
                "approved-reviews-by = sp-reviewer",
                "check-success = gate",
                "check-success = backtester-gate",
                "check-success = actionlint",
                "check-success = host-health",
            ],
        ) in payload["findings"], payload["findings"]
        assert mergify_required_reviewer_finding(
            "default",
            ["sp-reviewer"],
            [
                "approved-reviews-by = sp-reviewer",
                "check-success = gate",
                "check-success = backtester-gate",
                "check-success = actionlint",
                "check-success = host-health",
            ],
        ) in payload["findings"], payload["findings"]
        assert_equal(
            [batch["prs"] for batch in payload["batches"]],
            [[1], [2]],
            "mixed queue size-valid batches",
        )
        assert_equal(payload["wave_status"], "split_advised", "mixed queue wave status")


def assert_default_queue_above_max_is_split_advised() -> None:
    with tempfile.TemporaryDirectory() as tmp:
        root = pathlib.Path(tmp)
        fixture = GitFixture(root)
        heads = {
            pr: fixture.make_pr(pr, {f"default-{pr}.txt": f"default {pr}\n"})
            for pr in range(1, 12)
        }
        bin_dir = write_fake_gh(
            root,
            views={pr: approved_pr_view(head) for pr, head in heads.items()},
            checks=passing_source_pr_checks(*heads),
        )
        result = run_preflight_with_gh(
            fixture.repo,
            fixture.remote,
            bin_dir,
            *(str(pr) for pr in heads),
        )
        assert_equal(result.returncode, 1, "default queue above max rc")
        payload = parse_json(result.stdout)
        assert_equal(payload["wave_status"], "split_advised", "default queue above max wave status")
        assert_equal(
            [batch["prs"] for batch in payload["batches"]],
            [list(range(1, 11)), [11]],
            "default queue above max size-valid batches",
        )
        assert mergify_queue_batch_above_max_finding("default", list(heads), 10) in payload["findings"], payload["findings"]


def assert_default_queue_below_min_reports_wait_behavior() -> None:
    with tempfile.TemporaryDirectory() as tmp:
        root = pathlib.Path(tmp)
        fixture = GitFixture(root)
        head = fixture.make_pr(1, {"default.txt": "default\n"})
        bin_dir = write_fake_gh(
            root,
            views={1: approved_pr_view(head)},
            checks=passing_source_pr_checks(1),
        )
        result = run_preflight_with_gh(fixture.repo, fixture.remote, bin_dir, "1")
        assert_equal(result.returncode, 0, "default queue below min rc")
        payload = parse_json(result.stdout)
        assert_equal(payload["wave_status"], "ready", "default queue below min wave status")
        assert_equal((payload["verdict"], payload["contract_exit_code"]), ("queue_as_one_wave", 0), "default queue below min contract")
        assert mergify_queue_batch_below_min_finding("default", [1], 2, "5 minutes") in payload["findings"], payload["findings"]


def assert_invalid_mergify_config_does_not_route() -> None:
    with tempfile.TemporaryDirectory() as tmp:
        root = pathlib.Path(tmp)
        fixture = GitFixture(root)
        write(
            fixture.repo / ".mergify.yml",
            MERGIFY_YML.replace("label = hotfix", "label = urgent"),
        )
        base = commit(fixture.repo, "unsupported mergify route")
        git(fixture.repo, "push", "origin", "main")
        head = fixture.make_pr(1, {"one.txt": "one\n"})
        bin_dir = write_fake_gh(root, views={1: approved_pr_view(head)})
        result = run_preflight_with_gh(
            fixture.repo,
            fixture.remote,
            bin_dir,
            "1",
            expected_base_sha=base,
        )
        assert_equal(result.returncode, 3, "invalid mergify config rc")
        payload = parse_json(result.stdout)
        assert_equal(payload["lane_statuses"]["mergify_config"], "inconclusive", "invalid mergify lane")
        reason_codes = {finding["reason_code"] for finding in payload["findings"]}
        if "mergify_config_invalid" not in reason_codes:
            raise AssertionError(payload["findings"])
        if "mergify_queue_route_selected" in reason_codes:
            raise AssertionError(payload["findings"])


def assert_stale_base_sha_is_inconclusive() -> None:
    with tempfile.TemporaryDirectory() as tmp:
        fixture = GitFixture(pathlib.Path(tmp))
        fixture.make_pr(1, {"one.txt": "one\n"})
        expected_base = fixture.base
        write(fixture.repo / "advance-main.txt", "advance\n")
        actual_base = commit(fixture.repo, "advance main")
        git(fixture.repo, "push", "origin", "main")

        rc, stdout, _ = run_preflight(
            fixture.repo,
            fixture.remote,
            "1",
            expect_success=False,
            expected_base_sha=expected_base,
        )
        payload = parse_json(stdout)
        assert_equal(rc, 3, "stale base rc")
        assert stale_base_finding(expected_base, actual_base) in payload["findings"], payload["findings"]
        assert_equal(payload["lane_statuses"]["identity"], "inconclusive", "stale base identity lane")


def assert_unavailable_base_ref_is_inconclusive() -> None:
    with tempfile.TemporaryDirectory() as tmp:
        root = pathlib.Path(tmp)
        fixture = GitFixture(root)
        head = fixture.make_pr(1, {"one.txt": "one\n"})
        command = [
            sys.executable,
            str(SCRIPT_PATH),
            "--origin",
            str(root / "missing-origin.git"),
            "--base",
            "main",
            "--expected-base-sha",
            fixture.base,
            "--expected-head-sha",
            f"1={head}",
            "--no-gh",
            "--verifier-profile",
            "none",
            "--json",
            "1",
        ]
        result = subprocess.run(
            command,
            cwd=fixture.repo,
            text=True,
            stdout=subprocess.PIPE,
            stderr=subprocess.PIPE,
            check=False,
        )
        assert_equal(result.returncode, 3, "unavailable base rc")
        payload = parse_json(result.stdout)
        assert_equal(payload["actual_base_sha"], None, "unavailable base actual sha")
        assert_equal(payload["lane_statuses"]["identity"], "inconclusive", "unavailable base identity lane")
        reason_codes = {finding["reason_code"] for finding in payload["findings"]}
        if "base_unavailable" not in reason_codes:
            raise AssertionError(payload["findings"])


def assert_stale_expected_head_sha_blocks_pr() -> None:
    with tempfile.TemporaryDirectory() as tmp:
        root = pathlib.Path(tmp)
        fixture = GitFixture(root)
        actual_head = fixture.make_pr(1, {"one.txt": "one\n"})
        stale_head = "0" * 40
        command = [
            sys.executable,
            str(SCRIPT_PATH),
            "--origin",
            str(fixture.remote),
            "--base",
            "main",
            "--expected-base-sha",
            git(fixture.repo, "rev-parse", "main"),
            "--expected-head-sha",
            f"1={stale_head}",
            "--no-gh",
            "--verifier-profile",
            "none",
            "--json",
            "1",
        ]
        result = subprocess.run(
            command,
            cwd=fixture.repo,
            text=True,
            stdout=subprocess.PIPE,
            stderr=subprocess.PIPE,
            check=False,
        )
        payload = parse_json(result.stdout)
        assert_equal(result.returncode, 2, "stale head rc")
        assert_equal(payload["expected_pr_heads"], {"1": stale_head}, "expected PR heads")
        assert_equal(payload["pr_heads"], {"1": actual_head}, "actual PR heads")
        assert stale_head_finding(1, stale_head, actual_head) in payload["findings"], payload["findings"]
        assert_equal(payload["lane_statuses"]["identity"], "blocked", "stale head identity lane")
        assert_equal(payload["blocked_prs"][0]["type"], "head_mismatch", "stale head blocked type")
        assert_equal(payload["batches"], [], "stale head batches")


def assert_clean_prs_batch_together() -> None:
    with tempfile.TemporaryDirectory() as tmp:
        fixture = GitFixture(pathlib.Path(tmp))
        fixture.make_pr(1, {"one.txt": "one\n"})
        fixture.make_pr(2, {"two.txt": "two\n"})

        rc, stdout, _ = run_preflight(
            fixture.repo,
            fixture.remote,
            "1",
            "2",
            expect_success=False,
        )
        assert_equal(rc, 3, "clean no-gh rc")
        payload = parse_json(stdout)
        assert_equal(payload["verdict"], "inconclusive", "clean no-gh verdict")
        assert_equal(payload["residual_risks"], EXPECTED_RESIDUAL_RISKS, "clean residual risks")
        assert_equal(
            payload["findings"],
            [
                matching_base_finding(payload["base_sha"]),
                matching_head_finding(1, payload["pr_heads"]["1"]),
                matching_head_finding(2, payload["pr_heads"]["2"]),
                mergify_config_finding(
                    payload["base_sha"],
                    git(fixture.repo, "rev-parse", f"{payload['base_sha']}:.mergify.yml"),
                ),
                mergify_config_valid_finding(
                    payload["base_sha"],
                    git(fixture.repo, "rev-parse", f"{payload['base_sha']}:.mergify.yml"),
                ),
                no_gh_finding(),
                *residual_risk_findings(),
                integration_batch_ready_finding(payload["batches"][0]),
                verifier_batch_ready_finding(payload["batches"][0]),
            ],
            "clean no-gh findings",
        )

        assert_equal(
            payload["batches"],
            [{"index": 1, "prs": [1, 2], "status": "ready", "verifiers": []}],
            "clean batches",
        )
        assert_equal(payload["blocked_prs"], [], "clean blocked_prs")
        assert_equal(payload["conflicts"], [], "clean conflicts")


def assert_clean_prs_verify_final_batch_once() -> None:
    with tempfile.TemporaryDirectory() as tmp:
        root = pathlib.Path(tmp)
        fixture = GitFixture(root)
        first_head = fixture.make_pr(1, {"one.txt": "one\n"})
        second_head = fixture.make_pr(2, {"two.txt": "two\n"})
        third_head = fixture.make_pr(3, {"three.txt": "three\n"})
        counter = root / "verifier-count.txt"
        verifier = root / "count_verifier.py"
        write(
            verifier,
            "from pathlib import Path\n"
            f"counter = Path({str(counter)!r})\n"
            "value = int(counter.read_text(encoding='utf-8')) if counter.exists() else 0\n"
            "counter.write_text(str(value + 1), encoding='utf-8')\n",
        )
        command = [
            sys.executable,
            str(SCRIPT_PATH),
            "--origin",
            str(fixture.remote),
            "--base",
            "main",
            "--expected-base-sha",
            fixture.base,
            "--expected-head-sha",
            f"1={first_head}",
            "--expected-head-sha",
            f"2={second_head}",
            "--expected-head-sha",
            f"3={third_head}",
            "--no-gh",
            "--verifier-profile",
            "none",
            "--run-verifier",
            f"{sys.executable} {verifier}",
            "--json",
            "1",
            "2",
            "3",
        ]
        result = subprocess.run(
            command,
            cwd=fixture.repo,
            text=True,
            stdout=subprocess.PIPE,
            stderr=subprocess.PIPE,
            check=False,
        )
        assert_equal(result.returncode, 3, "clean batch verifier no-gh rc")
        payload = parse_json(result.stdout)
        assert_equal([batch["prs"] for batch in payload["batches"]], [[1, 2, 3]], "clean final batch")
        assert_equal(counter.read_text(encoding="utf-8"), "1", "clean batch verifier runs")


def assert_conflicting_pr_starts_later_batch() -> None:
    with tempfile.TemporaryDirectory() as tmp:
        fixture = GitFixture(pathlib.Path(tmp))
        fixture.make_pr(1, {"shared.txt": "first\n"})
        fixture.make_pr(2, {"shared.txt": "second\n"})

        rc, stdout, _ = run_preflight(
            fixture.repo,
            fixture.remote,
            "1",
            "2",
            expect_success=False,
        )
        assert_equal(rc, 3, "batch conflict rc")
        payload = parse_json(stdout)

        expected = {
            "pr": 2,
            "against_batch": [1],
            "files": ["shared.txt"],
            "type": "batch_conflict",
        }
        assert_equal(
            [batch["prs"] for batch in payload["batches"]],
            [[1], [2]],
            "batch conflict batches",
        )
        assert_equal(payload["conflicts"], [expected], "batch conflict artifacts")


def assert_ready_batch_conflict_is_split_advised() -> None:
    with tempfile.TemporaryDirectory() as tmp:
        root = pathlib.Path(tmp)
        fixture = GitFixture(root)
        first_head = fixture.make_pr(1, {"shared.txt": "first\n"})
        second_head = fixture.make_pr(2, {"shared.txt": "second\n"})
        bin_dir = write_fake_gh(
            root,
            views={
                1: approved_pr_view(first_head),
                2: approved_pr_view(second_head),
            },
            checks=passing_source_pr_checks(1, 2),
        )
        result = run_preflight_with_gh(fixture.repo, fixture.remote, bin_dir, "1", "2")
        assert_equal(result.returncode, 1, "ready batch conflict rc")
        payload = parse_json(result.stdout)

        expected = {
            "pr": 2,
            "against_batch": [1],
            "files": ["shared.txt"],
            "type": "batch_conflict",
        }
        assert_equal(
            [batch["prs"] for batch in payload["batches"]],
            [[1], [2]],
            "ready batch conflict batches",
        )
        assert_equal(payload["conflicts"], [expected], "ready batch conflict artifacts")
        assert_equal(payload["wave_status"], "split_advised", "ready batch conflict wave status")
        assert_equal(
            (payload["verdict"], payload["contract_exit_code"]),
            ("split_advised", 1),
            "ready batch conflict contract",
        )


def assert_order_dependent_conflict_context_is_reported() -> None:
    with tempfile.TemporaryDirectory() as tmp:
        fixture = GitFixture(pathlib.Path(tmp))
        fixture.make_pr(1, {"one.txt": "one\n"})
        fixture.make_pr(2, {"shared.txt": "second\n"})
        fixture.make_pr(3, {"shared.txt": "third\n"})

        rc, stdout, _ = run_preflight(
            fixture.repo,
            fixture.remote,
            "1",
            "2",
            "3",
            expect_success=False,
        )
        assert_equal(rc, 3, "order-dependent conflict rc")
        payload = parse_json(stdout)

        assert_equal(
            [batch["prs"] for batch in payload["batches"]],
            [[1, 2], [3]],
            "order-dependent conflict batches",
        )
        assert_equal(
            payload["conflicts"],
            [
                {
                    "pr": 3,
                    "against_batch": [1, 2],
                    "files": ["shared.txt"],
                    "type": "batch_conflict",
                }
            ],
            "order-dependent conflict artifacts",
        )


def assert_pr_that_conflicts_with_base_is_blocked() -> None:
    with tempfile.TemporaryDirectory() as tmp:
        fixture = GitFixture(pathlib.Path(tmp))
        fixture.make_pr(1, {"one.txt": "one\n"})
        fixture.make_pr(2, {"shared.txt": "stale branch change\n"})
        git(fixture.repo, "checkout", "main")
        write(fixture.repo / "shared.txt", "new main\n")
        commit(fixture.repo, "advance main")
        git(fixture.repo, "push", "origin", "main")

        rc, stdout, _ = run_preflight(
            fixture.repo,
            fixture.remote,
            "1",
            "2",
            expect_success=False,
        )
        assert_equal(rc, 2, "base conflict rc")
        payload = parse_json(stdout)

        blocked = payload["blocked_prs"]
        expected_blocked = [
            {
                "pr": 2,
                "reason": "conflicts with base",
                "files": ["shared.txt"],
                "type": "base_conflict",
            }
        ]
        assert_equal([batch["prs"] for batch in payload["batches"]], [[1]], "base conflict batches")
        assert_equal(blocked, expected_blocked, "base conflict blocked_prs")
        assert_equal(
            payload["findings"],
            [
                matching_base_finding(payload["base_sha"]),
                matching_head_finding(1, payload["pr_heads"]["1"]),
                matching_head_finding(2, payload["pr_heads"]["2"]),
                mergify_config_finding(
                    payload["base_sha"],
                    git(fixture.repo, "rev-parse", f"{payload['base_sha']}:.mergify.yml"),
                ),
                mergify_config_valid_finding(
                    payload["base_sha"],
                    git(fixture.repo, "rev-parse", f"{payload['base_sha']}:.mergify.yml"),
                ),
                no_gh_finding(),
                *residual_risk_findings(),
                integration_batch_ready_finding(payload["batches"][0]),
                verifier_batch_ready_finding(payload["batches"][0]),
                {
                    "lane": "integration",
                    "scope": "pr",
                    "status": "blocked",
                    "reason_code": "base_conflict",
                    "message": "base_conflict",
                    "evidence": blocked[0],
                }
            ],
            "base conflict findings",
        )
        assert_equal((payload["verdict"], payload["contract_exit_code"]), ("blocked", 2), "base conflict contract")
        assert_equal(payload["lane_statuses"]["integration"], "blocked", "base conflict integration lane")


def assert_verifier_failure_blocks_bad_pr_before_batching() -> None:
    with tempfile.TemporaryDirectory() as tmp:
        root = pathlib.Path(tmp)
        fixture = GitFixture(root)
        safe_head = fixture.make_pr(1, {"safe.txt": "safe\n"})
        fail_head = fixture.make_pr(2, {"fail.txt": "fail\n"})
        verifier = root / "reject_fail_file.py"
        write(
            verifier,
            "from pathlib import Path\n"
            "import sys\n"
            "if Path('fail.txt').exists():\n"
            "    print('fail.txt is not allowed')\n"
            "    sys.exit(7)\n",
        )

        command = [
            sys.executable,
            str(SCRIPT_PATH),
            "--origin",
            str(fixture.remote),
            "--base",
            "main",
            "--expected-base-sha",
            fixture.base,
            "--expected-head-sha",
            f"1={safe_head}",
            "--expected-head-sha",
            f"2={fail_head}",
            "--no-gh",
            "--verifier-profile",
            "none",
            "--run-verifier",
            f"{sys.executable} {verifier}",
            "--json",
            "1",
            "2",
        ]
        result = subprocess.run(
            command,
            cwd=fixture.repo,
            text=True,
            stdout=subprocess.PIPE,
            stderr=subprocess.PIPE,
            check=False,
        )
        assert_equal(result.returncode, 2, "verifier failure rc")
        payload = parse_json(result.stdout)
        if [batch["prs"] for batch in payload["batches"]] != [[1]]:
            raise AssertionError(payload["batches"])
        blocked = payload["blocked_prs"]
        if len(blocked) != 1 or blocked[0]["pr"] != 2 or blocked[0]["type"] != "verifier_failed":
            raise AssertionError(blocked)
        if "fail.txt is not allowed" not in blocked[0]["stdout_preview"]:
            raise AssertionError(blocked)


def assert_batch_first_fallback_excludes_poisoned_pr_and_reverifies_remainder() -> None:
    with tempfile.TemporaryDirectory() as tmp:
        root = pathlib.Path(tmp)
        fixture = GitFixture(root)
        first_head = fixture.make_pr(1, {"one.txt": "one\n"})
        poison_head = fixture.make_pr(2, {"poison.txt": "poison\n", "shared.txt": "poison\n"})
        third_head = fixture.make_pr(3, {"shared.txt": "third\n", "three.txt": "three\n"})
        bin_dir = write_fake_gh(
            root,
            views={
                1: approved_pr_view(first_head),
                2: approved_pr_view(poison_head),
                3: approved_pr_view(third_head),
            },
            checks=passing_source_pr_checks(1, 2, 3),
        )
        log = root / "verifier-log.txt"
        verifier = root / "reject_poison.py"
        write(
            verifier,
            "from pathlib import Path\n"
            "import sys\n"
            f"log = Path({str(log)!r})\n"
            "files = ','.join(sorted(path.name for path in Path('.').glob('*.txt')))\n"
            "with log.open('a', encoding='utf-8') as stream:\n"
            "    stream.write(files + '\\n')\n"
            "if Path('poison.txt').exists():\n"
            "    print('poison.txt is not allowed')\n"
            "    sys.exit(7)\n",
        )
        command = [
            sys.executable,
            str(SCRIPT_PATH),
            "--origin",
            str(fixture.remote),
            "--base",
            "main",
            "--expected-base-sha",
            fixture.base,
            "--expected-head-sha",
            f"1={first_head}",
            "--expected-head-sha",
            f"2={poison_head}",
            "--expected-head-sha",
            f"3={third_head}",
            "--verifier-profile",
            "none",
            "--run-verifier",
            f"{sys.executable} {verifier}",
            "--json",
            "1",
            "2",
            "3",
        ]
        env = os.environ.copy()
        env["PATH"] = f"{bin_dir}{os.pathsep}{env['PATH']}"
        result = subprocess.run(
            command,
            cwd=fixture.repo,
            env=env,
            text=True,
            stdout=subprocess.PIPE,
            stderr=subprocess.PIPE,
            check=False,
        )
        assert_equal(result.returncode, 2, "poison fallback rc")
        payload = parse_json(result.stdout)
        assert_equal([batch["prs"] for batch in payload["batches"]], [[1, 3]], "poison fallback batches")
        blocked = payload["blocked_prs"]
        if len(blocked) != 1 or blocked[0]["pr"] != 2 or blocked[0]["type"] != "verifier_failed":
            raise AssertionError(blocked)
        if "poison.txt is not allowed" not in blocked[0]["stdout_preview"]:
            raise AssertionError(blocked)
        if payload["conflicts"]:
            raise AssertionError(payload["conflicts"])
        runs = log.read_text(encoding="utf-8").splitlines()
        assert_equal(runs[0], "one.txt,poison.txt,shared.txt", "initial batch-first verifier run")
        if "one.txt,shared.txt,three.txt" not in runs:
            raise AssertionError(runs)


def assert_fallback_recombines_survivors_after_batch_max_split() -> None:
    with tempfile.TemporaryDirectory() as tmp:
        root = pathlib.Path(tmp)
        fixture = GitFixture(root)
        heads = {
            pr: fixture.make_pr(pr, {"poison.txt" if pr == 2 else f"pr{pr:02}.txt": f"pr {pr}\n"})
            for pr in range(1, 12)
        }
        bin_dir = write_fake_gh(
            root,
            views={pr: approved_pr_view(head) for pr, head in heads.items()},
            checks=passing_source_pr_checks(*heads),
        )
        log = root / "verifier-log.txt"
        verifier = root / "reject_poison.py"
        write(
            verifier,
            "from pathlib import Path\n"
            "import sys\n"
            f"log = Path({str(log)!r})\n"
            "files = ','.join(sorted(path.name for path in Path('.').glob('*.txt')))\n"
            "with log.open('a', encoding='utf-8') as stream:\n"
            "    stream.write(files + '\\n')\n"
            "if Path('poison.txt').exists():\n"
            "    print('poison.txt is not allowed')\n"
            "    sys.exit(7)\n",
        )
        command = [
            sys.executable,
            str(SCRIPT_PATH),
            "--origin",
            str(fixture.remote),
            "--base",
            "main",
            "--expected-base-sha",
            fixture.base,
            "--expected-head-sha",
            f"1={heads[1]}",
            *[
                item
                for pr in range(2, 12)
                for item in ("--expected-head-sha", f"{pr}={heads[pr]}")
            ],
            "--verifier-profile",
            "none",
            "--run-verifier",
            f"{sys.executable} {verifier}",
            "--json",
            *(str(pr) for pr in range(1, 12)),
        ]
        env = os.environ.copy()
        env["PATH"] = f"{bin_dir}{os.pathsep}{env['PATH']}"
        result = subprocess.run(
            command,
            cwd=fixture.repo,
            env=env,
            text=True,
            stdout=subprocess.PIPE,
            stderr=subprocess.PIPE,
            check=False,
        )
        assert_equal(result.returncode, 2, "poison fallback max split rc")
        payload = parse_json(result.stdout)
        assert_equal(
            [batch["prs"] for batch in payload["batches"]],
            [[1, 3, 4, 5, 6, 7, 8, 9, 10, 11]],
            "poison fallback max split batches",
        )
        blocked = payload["blocked_prs"]
        if len(blocked) != 1 or blocked[0]["pr"] != 2 or blocked[0]["type"] != "verifier_failed":
            raise AssertionError(blocked)
        runs = log.read_text(encoding="utf-8").splitlines()
        if "poison.txt" not in runs[0] or "pr11.txt" in runs[0]:
            raise AssertionError(runs[0])
        if "poison.txt" in runs[-1] or "pr11.txt" not in runs[-1]:
            raise AssertionError(runs)


def assert_fallback_replaces_suffix_optimistic_conflicts() -> None:
    with tempfile.TemporaryDirectory() as tmp:
        root = pathlib.Path(tmp)
        fixture = GitFixture(root)
        heads = {
            1: fixture.make_pr(1, {"poison.txt": "poison\n"}),
            2: fixture.make_pr(2, {"shared.txt": "second\n", "two.txt": "two\n"}),
            3: fixture.make_pr(3, {"three.txt": "three\n"}),
            4: fixture.make_pr(4, {"four.txt": "four\n"}),
            5: fixture.make_pr(5, {"shared.txt": "fifth\n", "five.txt": "five\n"}),
        }
        bin_dir = write_fake_gh(
            root,
            views={
                1: approved_pr_view(heads[1], labels=("hotfix",)),
                **{pr: approved_pr_view(head) for pr, head in heads.items() if pr != 1},
            },
            checks=passing_source_pr_checks(*heads),
        )
        verifier = root / "reject_poison.py"
        write(
            verifier,
            "from pathlib import Path\n"
            "import sys\n"
            "if Path('poison.txt').exists():\n"
            "    print('poison.txt is not allowed')\n"
            "    sys.exit(7)\n",
        )
        command = [
            sys.executable,
            str(SCRIPT_PATH),
            "--origin",
            str(fixture.remote),
            "--base",
            "main",
            "--expected-base-sha",
            fixture.base,
            *[
                item
                for pr, head in heads.items()
                for item in ("--expected-head-sha", f"{pr}={head}")
            ],
            "--verifier-profile",
            "none",
            "--run-verifier",
            f"{sys.executable} {verifier}",
            "--json",
            *(str(pr) for pr in heads),
        ]
        env = os.environ.copy()
        env["PATH"] = f"{bin_dir}{os.pathsep}{env['PATH']}"
        result = subprocess.run(
            command,
            cwd=fixture.repo,
            env=env,
            text=True,
            stdout=subprocess.PIPE,
            stderr=subprocess.PIPE,
            check=False,
        )
        assert_equal(result.returncode, 2, "suffix conflict replacement rc")
        payload = parse_json(result.stdout)
        blocked = payload["blocked_prs"]
        if len(blocked) != 1 or blocked[0]["pr"] != 1 or blocked[0]["type"] != "verifier_failed":
            raise AssertionError(blocked)
        assert_equal(
            [batch["prs"] for batch in payload["batches"]],
            [[2, 3, 4], [5]],
            "suffix conflict replacement batches",
        )
        conflicts = payload["conflicts"]
        expected = [
            {
                "pr": 5,
                "against_batch": [2, 3, 4],
                "files": ["shared.txt"],
                "type": "batch_conflict",
            }
        ]
        assert_equal(conflicts, expected, "suffix conflict replacement conflicts")


def install_synthetic_run_fences(fixture: GitFixture, log: pathlib.Path, body: str = "") -> None:
    git(fixture.repo, "checkout", "main")
    script = fixture.repo / "scripts" / "run_fences.py"
    write(
        script,
        "#!/usr/bin/env python3\n"
        "from pathlib import Path\n"
        "import sys\n"
        f"log = Path({str(log)!r})\n"
        "with log.open('a', encoding='utf-8') as stream:\n"
        "    stream.write(' '.join(sys.argv[1:]) + '\\n')\n"
        f"{body}"
        "raise SystemExit(0)\n",
    )
    script.chmod(0o755)
    fixture.base = commit(fixture.repo, "install synthetic run_fences")
    git(fixture.repo, "push", "origin", "main")


def install_synthetic_source_fence_static(fixture: GitFixture, log: pathlib.Path, gate_log: pathlib.Path) -> None:
    git(fixture.repo, "checkout", "main")
    write(
        fixture.repo / "scripts" / "local_verification_gate.py",
        "#!/usr/bin/env python3\n"
        "from pathlib import Path\n"
        "import subprocess\n"
        "import sys\n"
        f"log = Path({str(gate_log)!r})\n"
        "with log.open('a', encoding='utf-8') as stream:\n"
        "    stream.write(' '.join(sys.argv[1:]) + '\\n')\n"
        "separator = sys.argv.index('--')\n"
        "raise SystemExit(subprocess.run(sys.argv[separator + 1:]).returncode)\n",
    )
    write(
        fixture.repo / "justfile",
        "source-fence-static:\n"
        "    python3 scripts/local_verification_gate.py source-fence-static -- just source-fence-static-inner\n\n"
        "source-fence-static-fences-only:\n"
        "    python3 scripts/local_verification_gate.py source-fence-static-fences-only -- just source-fence-static-fences-only-inner\n\n"
        "[private]\n"
        "source-fence-static-inner:\n"
        "    python3 scripts/run_fences.py\n\n"
        "[private]\n"
        "source-fence-static-fences-only-inner:\n"
        "    python3 scripts/run_fences.py --fences-only\n",
    )
    fixture.base = commit(fixture.repo, "install synthetic source fence static")
    git(fixture.repo, "push", "origin", "main")


def run_preflight_with_config(
    fixture: GitFixture,
    config: pathlib.Path,
    heads: Mapping[int, str],
) -> subprocess.CompletedProcess[str]:
    command = [
        sys.executable,
        str(SCRIPT_PATH),
        "--origin",
        str(fixture.remote),
        "--base",
        "main",
        "--expected-base-sha",
        fixture.base,
        *[
            item
            for pr, head in heads.items()
            for item in ("--expected-head-sha", f"{pr}={head}")
        ],
        "--config",
        str(config),
        "--no-gh",
        "--json",
        *(str(pr) for pr in heads),
    ]
    return subprocess.run(
        command,
        cwd=fixture.repo,
        text=True,
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
        check=False,
    )


def read_line_with_timeout(stream, timeout_seconds: float) -> str | None:
    readable, _, _ = select.select([stream], [], [], timeout_seconds)
    if not readable:
        return None
    return stream.readline()


def wait_for_stderr_line(
    process: subprocess.Popen[str],
    needle: str,
    timeout_seconds: float,
) -> tuple[str | None, list[str]]:
    deadline = time.monotonic() + timeout_seconds
    lines: list[str] = []
    while time.monotonic() < deadline:
        line = read_line_with_timeout(process.stderr, max(0.0, deadline - time.monotonic()))
        if line is None:
            break
        if line == "":
            break
        lines.append(line)
        if needle in line:
            return line, lines
    return None, lines


def stop_process(process: subprocess.Popen[str]) -> None:
    if process.poll() is None:
        process.kill()
        process.communicate()


def assert_preflight_source_fence_profile_selects_fences_only_by_scripts_diff() -> None:
    with tempfile.TemporaryDirectory() as tmp:
        root = pathlib.Path(tmp)
        fixture = GitFixture(root)
        log = root / "run-fences-argv.log"
        install_synthetic_run_fences(fixture, log)
        config = write_preflight_config(root, "static", [f"{sys.executable} scripts/run_fences.py"])

        nonscripts_head = fixture.make_pr(1, {"one.txt": "one\n"})
        result = run_preflight_with_config(fixture, config, {1: nonscripts_head})
        assert_equal(result.returncode, 3, "non-scripts source fence profile rc")
        first_run = log.read_text(encoding="utf-8").splitlines()[-1]
        if "--fences-only" not in first_run:
            raise AssertionError(first_run)

        scripts_head = fixture.make_pr(2, {"scripts/changed.py": "VALUE = 1\n"})
        result = run_preflight_with_config(fixture, config, {2: scripts_head})
        assert_equal(result.returncode, 3, "scripts source fence profile rc")
        second_run = log.read_text(encoding="utf-8").splitlines()[-1]
        if "--fences-only" in second_run:
            raise AssertionError(second_run)

        direct_config = write_preflight_config(root, "static", ["./scripts/run_fences.py"])
        direct_head = fixture.make_pr(3, {"direct.txt": "direct\n"})
        result = run_preflight_with_config(fixture, direct_config, {3: direct_head})
        assert_equal(result.returncode, 3, "direct source fence profile rc")
        direct_run = log.read_text(encoding="utf-8").splitlines()[-1]
        if "--fences-only" not in direct_run:
            raise AssertionError(direct_run)

        gate_log = root / "source-fence-gate.log"
        install_synthetic_source_fence_static(fixture, log, gate_log)
        just_config = write_preflight_config(root, "static", ["just source-fence-static"])
        just_head = fixture.make_pr(4, {"just.txt": "just\n"})
        result = run_preflight_with_config(fixture, just_config, {4: just_head})
        assert_equal(result.returncode, 3, "just source fence profile rc")
        just_run = log.read_text(encoding="utf-8").splitlines()[-1]
        if "--fences-only" not in just_run:
            raise AssertionError(just_run)
        gate_run = gate_log.read_text(encoding="utf-8").splitlines()[-1]
        if gate_run != "source-fence-static-fences-only -- just source-fence-static-fences-only-inner":
            raise AssertionError(gate_run)


def assert_scripts_pr_fence_regression_uses_full_profile() -> None:
    with tempfile.TemporaryDirectory() as tmp:
        root = pathlib.Path(tmp)
        fixture = GitFixture(root)
        log = root / "run-fences-regression.log"
        install_synthetic_run_fences(
            fixture,
            log,
            "if '--fences-only' not in sys.argv and Path('scripts/fence_regression.txt').exists():\n"
            "    print('full profile caught scripts fence regression')\n"
            "    raise SystemExit(7)\n",
        )
        config = write_preflight_config(root, "static", [f"{sys.executable} scripts/run_fences.py"])
        head = fixture.make_pr(1, {"scripts/fence_regression.txt": "regression\n"})

        result = run_preflight_with_config(fixture, config, {1: head})
        assert_equal(result.returncode, 2, "scripts fence regression rc")
        payload = parse_json(result.stdout)
        blocked = payload["blocked_prs"]
        if len(blocked) != 1 or blocked[0]["pr"] != 1 or blocked[0]["type"] != "verifier_failed":
            raise AssertionError(blocked)
        if "full profile caught scripts fence regression" not in blocked[0]["stdout_preview"]:
            raise AssertionError(blocked)
        last_run = log.read_text(encoding="utf-8").splitlines()[-1]
        if "--fences-only" in last_run:
            raise AssertionError(last_run)


def assert_verifier_progress_breadcrumb_precedes_final_output() -> None:
    with tempfile.TemporaryDirectory() as tmp:
        root = pathlib.Path(tmp)
        fixture = GitFixture(root)
        head = fixture.make_pr(1, {"one.txt": "one\n"})
        verifier = root / "slow_success.py"
        write(
            verifier,
            "import time\n"
            "time.sleep(5)\n",
        )
        command = [
            sys.executable,
            str(SCRIPT_PATH),
            "--origin",
            str(fixture.remote),
            "--base",
            "main",
            "--expected-base-sha",
            fixture.base,
            "--expected-head-sha",
            f"1={head}",
            "--no-gh",
            "--verifier-profile",
            "none",
            "--run-verifier",
            f"{sys.executable} {verifier}",
            "--json",
            "1",
        ]
        process = subprocess.Popen(
            command,
            cwd=fixture.repo,
            text=True,
            stdout=subprocess.PIPE,
            stderr=subprocess.PIPE,
        )
        try:
            line, lines = wait_for_stderr_line(process, "merge_queue_preflight: verifier running:", 15.0)
            if line is None or "merge_queue_preflight: verifier running:" not in line:
                raise AssertionError(lines)
            stdout_line = read_line_with_timeout(process.stdout, 0.1)
            if stdout_line not in (None, ""):
                raise AssertionError(stdout_line)
            stdout, _stderr = process.communicate(timeout=20)
        finally:
            stop_process(process)
        assert_equal(process.returncode, 3, "streaming breadcrumb rc")
        payload = parse_json(stdout)
        assert_equal([batch["prs"] for batch in payload["batches"]], [[1]], "streaming breadcrumb batches")


def assert_first_verifier_failure_breadcrumb_arrives_before_later_batch_finishes() -> None:
    with tempfile.TemporaryDirectory() as tmp:
        root = pathlib.Path(tmp)
        fixture = GitFixture(root)
        first_head = fixture.make_pr(1, {"shared.txt": "first\n", "fail.txt": "fail\n"})
        second_head = fixture.make_pr(2, {"shared.txt": "second\n", "slow.txt": "slow\n"})
        verifier = root / "fail_then_slow.py"
        write(
            verifier,
            "from pathlib import Path\n"
            "import sys\n"
            "import time\n"
            "if Path('fail.txt').exists():\n"
            "    print('fail marker rejected')\n"
            "    sys.exit(7)\n"
            "if Path('slow.txt').exists():\n"
            "    time.sleep(10)\n",
        )
        command = [
            sys.executable,
            str(SCRIPT_PATH),
            "--origin",
            str(fixture.remote),
            "--base",
            "main",
            "--expected-base-sha",
            fixture.base,
            "--expected-head-sha",
            f"1={first_head}",
            "--expected-head-sha",
            f"2={second_head}",
            "--no-gh",
            "--verifier-profile",
            "none",
            "--run-verifier",
            f"{sys.executable} {verifier}",
            "--json",
            "1",
            "2",
        ]
        process = subprocess.Popen(
            command,
            cwd=fixture.repo,
            text=True,
            stdout=subprocess.PIPE,
            stderr=subprocess.PIPE,
        )
        try:
            line, lines = wait_for_stderr_line(process, "merge_queue_preflight: verifier failed:", 15.0)
            if line is None:
                raise AssertionError(lines)
            if "exit 7" not in line:
                raise AssertionError(line)
            stdout, _stderr = process.communicate(timeout=30)
        finally:
            stop_process(process)
        assert_equal(process.returncode, 2, "first failure breadcrumb rc")
        payload = parse_json(stdout)
        blocked = payload["blocked_prs"]
        if len(blocked) != 1 or blocked[0]["pr"] != 1 or blocked[0]["type"] != "verifier_failed":
            raise AssertionError(blocked)


def assert_missing_verifier_executable_is_inconclusive() -> None:
    with tempfile.TemporaryDirectory() as tmp:
        root = pathlib.Path(tmp)
        fixture = GitFixture(root)
        head = fixture.make_pr(1, {"one.txt": "one\n"})
        command = [
            sys.executable,
            str(SCRIPT_PATH),
            "--origin",
            str(fixture.remote),
            "--base",
            "main",
            "--expected-base-sha",
            fixture.base,
            "--expected-head-sha",
            f"1={head}",
            "--no-gh",
            "--verifier-profile",
            "none",
            "--run-verifier",
            "missing-preflight-verifier-command",
            "--json",
            "1",
        ]
        result = subprocess.run(
            command,
            cwd=fixture.repo,
            text=True,
            stdout=subprocess.PIPE,
            stderr=subprocess.PIPE,
            check=False,
        )
        assert_equal(result.returncode, 3, "missing verifier executable rc")
        payload = parse_json(result.stdout)
        blocked = payload["blocked_prs"]
        if len(blocked) != 1 or blocked[0]["pr"] != 1 or blocked[0]["type"] != "verifier_unavailable":
            raise AssertionError(payload)
        assert_equal(payload["lane_statuses"]["verifier"], "inconclusive", "missing verifier lane")
        assert_equal((payload["verdict"], payload["contract_exit_code"]), ("inconclusive", 3), "missing verifier contract")


def assert_unexpected_exception_is_not_split_advised() -> None:
    module = load_preflight_module()

    def broken_preflight(**_kwargs: object) -> tuple[dict[str, object], int]:
        raise RuntimeError("boom")

    original_preflight = module.preflight
    with tempfile.TemporaryDirectory() as tmp:
        root = pathlib.Path(tmp)
        config = write_preflight_config(root, "none", [])
        module.preflight = broken_preflight
        try:
            stderr = io.StringIO()
            with contextlib.redirect_stderr(stderr):
                result = module.main(
                    [
                        "--config",
                        str(config),
                        "--expected-base-sha",
                        "a" * 40,
                        "--expected-head-sha",
                        f"1={'b' * 40}",
                        "--no-gh",
                        "--json",
                        "1",
                    ]
                )
        finally:
            module.preflight = original_preflight
    assert_equal(result, 4, "unexpected exception rc")
    if "internal preflight failure" not in stderr.getvalue():
        raise AssertionError(stderr.getvalue())


def assert_verifier_timeout_is_inconclusive() -> None:
    with tempfile.TemporaryDirectory() as tmp:
        root = pathlib.Path(tmp)
        fixture = GitFixture(root)
        head = fixture.make_pr(1, {"one.txt": "one\n"})
        verifier = root / "slow_verifier.py"
        write(
            verifier,
            "import time\n"
            "time.sleep(5)\n",
        )
        config = write_preflight_config(
            root,
            "slow",
            [f"{sys.executable} {verifier}"],
            verifier_timeout_seconds=1,
        )
        command = [
            sys.executable,
            str(SCRIPT_PATH),
            "--origin",
            str(fixture.remote),
            "--base",
            "main",
            "--expected-base-sha",
            fixture.base,
            "--expected-head-sha",
            f"1={head}",
            "--config",
            str(config),
            "--no-gh",
            "--json",
            "1",
        ]
        try:
            result = subprocess.run(
                command,
                cwd=fixture.repo,
                text=True,
                stdout=subprocess.PIPE,
                stderr=subprocess.PIPE,
                check=False,
                timeout=4,
            )
        except subprocess.TimeoutExpired as exc:
            raise AssertionError("preflight did not enforce verifier timeout") from exc
        assert_equal(result.returncode, 3, "verifier timeout rc")
        payload = parse_json(result.stdout)
        blocked = payload["blocked_prs"]
        if len(blocked) != 1 or blocked[0]["pr"] != 1 or blocked[0]["type"] != "verifier_timeout":
            raise AssertionError(payload)
        assert_equal(payload["lane_statuses"]["verifier"], "inconclusive", "verifier timeout lane")
        assert_equal((payload["verdict"], payload["contract_exit_code"]), ("inconclusive", 3), "verifier timeout contract")


def assert_batch_verifier_failure_is_split_advised() -> None:
    with tempfile.TemporaryDirectory() as tmp:
        root = pathlib.Path(tmp)
        fixture = GitFixture(root)
        first_head = fixture.make_pr(1, {"one.txt": "one\n"})
        second_head = fixture.make_pr(2, {"two.txt": "two\n"})
        verifier = root / "reject_combined.py"
        write(
            verifier,
            "from pathlib import Path\n"
            "import sys\n"
            "if Path('one.txt').exists() and Path('two.txt').exists():\n"
            "    print('combined batch rejected')\n"
            "    sys.exit(7)\n",
        )
        bin_dir = write_fake_gh(
            root,
            views={
                1: approved_pr_view(first_head),
                2: approved_pr_view(second_head),
            },
            checks=passing_source_pr_checks(1, 2),
        )
        command = [
            sys.executable,
            str(SCRIPT_PATH),
            "--origin",
            str(fixture.remote),
            "--base",
            "main",
            "--expected-base-sha",
            fixture.base,
            "--expected-head-sha",
            f"1={first_head}",
            "--expected-head-sha",
            f"2={second_head}",
            "--verifier-profile",
            "none",
            "--run-verifier",
            f"{sys.executable} {verifier}",
            "--json",
            "1",
            "2",
        ]
        env = os.environ.copy()
        env["PATH"] = f"{bin_dir}{os.pathsep}{env['PATH']}"
        result = subprocess.run(
            command,
            cwd=fixture.repo,
            env=env,
            text=True,
            stdout=subprocess.PIPE,
            stderr=subprocess.PIPE,
            check=False,
        )
        assert_equal(result.returncode, 1, "batch verifier failure rc")
        payload = parse_json(result.stdout)
        assert_equal([batch["prs"] for batch in payload["batches"]], [[1], [2]], "batch verifier failure batches")
        conflicts = payload["conflicts"]
        if len(conflicts) != 1 or conflicts[0]["pr"] != 2 or conflicts[0]["type"] != "batch_verifier_failed":
            raise AssertionError(payload)
        assert_equal(payload["wave_status"], "split_advised", "batch verifier failure wave status")
        assert_equal((payload["verdict"], payload["contract_exit_code"]), ("split_advised", 1), "batch verifier failure contract")


def assert_configured_verifier_profile_blocks_bad_pr() -> None:
    with tempfile.TemporaryDirectory() as tmp:
        root = pathlib.Path(tmp)
        fixture = GitFixture(root)
        safe_head = fixture.make_pr(1, {"safe.txt": "safe\n"})
        fail_head = fixture.make_pr(2, {"fail.txt": "fail\n"})
        verifier = root / "reject_fail_file.py"
        write(
            verifier,
            "from pathlib import Path\n"
            "import sys\n"
            "if Path('fail.txt').exists():\n"
            "    print('configured verifier rejected fail.txt')\n"
            "    sys.exit(7)\n",
        )
        config = write_preflight_config(root, "strict", [f"{sys.executable} {verifier}"])

        command = [
            sys.executable,
            str(SCRIPT_PATH),
            "--origin",
            str(fixture.remote),
            "--base",
            "main",
            "--expected-base-sha",
            fixture.base,
            "--expected-head-sha",
            f"1={safe_head}",
            "--expected-head-sha",
            f"2={fail_head}",
            "--no-gh",
            "--config",
            str(config),
            "--json",
            "1",
            "2",
        ]
        result = subprocess.run(
            command,
            cwd=fixture.repo,
            text=True,
            stdout=subprocess.PIPE,
            stderr=subprocess.PIPE,
            check=False,
        )
        assert_equal(result.returncode, 2, "configured verifier failure rc")
        payload = parse_json(result.stdout)
        blocked = payload["blocked_prs"]
        if len(blocked) != 1 or blocked[0]["pr"] != 2 or blocked[0]["type"] != "verifier_failed":
            raise AssertionError(blocked)
        if "configured verifier rejected fail.txt" not in blocked[0]["stdout_preview"]:
            raise AssertionError(blocked)


def assert_plain_output_includes_verifier_failure_details() -> None:
    with tempfile.TemporaryDirectory() as tmp:
        root = pathlib.Path(tmp)
        fixture = GitFixture(root)
        head = fixture.make_pr(1, {"fail.txt": "fail\n"})
        verifier = root / "reject_fail_file.py"
        write(
            verifier,
            "from pathlib import Path\n"
            "import sys\n"
            "if Path('fail.txt').exists():\n"
            "    print('plain verifier rejected fail.txt')\n"
            "    sys.exit(7)\n",
        )
        command = [
            sys.executable,
            str(SCRIPT_PATH),
            "--origin",
            str(fixture.remote),
            "--base",
            "main",
            "--expected-base-sha",
            fixture.base,
            "--expected-head-sha",
            f"1={head}",
            "--no-gh",
            "--verifier-profile",
            "none",
            "--run-verifier",
            f"{sys.executable} {verifier}",
            "1",
        ]
        result = subprocess.run(
            command,
            cwd=fixture.repo,
            text=True,
            stdout=subprocess.PIPE,
            stderr=subprocess.PIPE,
            check=False,
        )
        assert_equal(result.returncode, 2, "plain verifier failure rc")
        if f"verifier {sys.executable} {verifier}: exit 7" not in result.stdout:
            raise AssertionError(result.stdout)
        if "plain verifier rejected fail.txt" not in result.stdout:
            raise AssertionError(result.stdout)


def assert_plain_output_omits_successful_verifier_streams() -> None:
    with tempfile.TemporaryDirectory() as tmp:
        root = pathlib.Path(tmp)
        fixture = GitFixture(root)
        head = fixture.make_pr(1, {"safe.txt": "safe\n"})
        verifier = root / "successful_verifier.py"
        write(
            verifier,
            "print('success output should stay quiet')\n",
        )
        command = [
            sys.executable,
            str(SCRIPT_PATH),
            "--origin",
            str(fixture.remote),
            "--base",
            "main",
            "--expected-base-sha",
            fixture.base,
            "--expected-head-sha",
            f"1={head}",
            "--no-gh",
            "--verifier-profile",
            "none",
            "--run-verifier",
            f"{sys.executable} {verifier}",
            "1",
        ]
        result = subprocess.run(
            command,
            cwd=fixture.repo,
            text=True,
            stdout=subprocess.PIPE,
            stderr=subprocess.PIPE,
            check=False,
        )
        assert_equal(result.returncode, 3, "plain successful verifier no-gh rc")
        if f"verifier {sys.executable} {verifier}: exit 0" not in result.stdout:
            raise AssertionError(result.stdout)
        if "success output should stay quiet" in result.stdout:
            raise AssertionError(result.stdout)


def assert_plain_output_bounds_failed_verifier_streams() -> None:
    with tempfile.TemporaryDirectory() as tmp:
        root = pathlib.Path(tmp)
        fixture = GitFixture(root)
        head = fixture.make_pr(1, {"fail.txt": "fail\n"})
        verifier = root / "noisy_failure.py"
        write(
            verifier,
            "import sys\n"
            "for line in ['line-1', 'line-2', 'line-3']:\n"
            "    print(line)\n"
            "sys.exit(7)\n",
        )
        config = write_preflight_config(
            root,
            "strict",
            [f"{sys.executable} {verifier}"],
            verifier_stream_max_lines=2,
            verifier_stream_max_bytes=200,
        )
        command = [
            sys.executable,
            str(SCRIPT_PATH),
            "--origin",
            str(fixture.remote),
            "--base",
            "main",
            "--expected-base-sha",
            fixture.base,
            "--expected-head-sha",
            f"1={head}",
            "--no-gh",
            "--config",
            str(config),
            "1",
        ]
        result = subprocess.run(
            command,
            cwd=fixture.repo,
            text=True,
            stdout=subprocess.PIPE,
            stderr=subprocess.PIPE,
            check=False,
        )
        assert_equal(result.returncode, 2, "plain bounded verifier failure rc")
        if "line-1" not in result.stdout or "line-2" not in result.stdout:
            raise AssertionError(result.stdout)
        if "line-3" in result.stdout:
            raise AssertionError(result.stdout)
        if "truncated" not in result.stdout:
            raise AssertionError(result.stdout)


def assert_json_output_uses_bounded_verifier_previews() -> None:
    with tempfile.TemporaryDirectory() as tmp:
        root = pathlib.Path(tmp)
        fixture = GitFixture(root)
        head = fixture.make_pr(1, {"fail.txt": "fail\n"})
        verifier = root / "noisy_json_failure.py"
        write(
            verifier,
            "import sys\n"
            "for line in ['json-line-1', 'json-line-2', 'json-line-3']:\n"
            "    print(line)\n"
            "sys.exit(7)\n",
        )
        config = write_preflight_config(
            root,
            "strict",
            [f"{sys.executable} {verifier}"],
            verifier_stream_max_lines=2,
            verifier_stream_max_bytes=200,
        )
        command = [
            sys.executable,
            str(SCRIPT_PATH),
            "--origin",
            str(fixture.remote),
            "--base",
            "main",
            "--expected-base-sha",
            fixture.base,
            "--expected-head-sha",
            f"1={head}",
            "--no-gh",
            "--config",
            str(config),
            "--json",
            "1",
        ]
        result = subprocess.run(
            command,
            cwd=fixture.repo,
            text=True,
            stdout=subprocess.PIPE,
            stderr=subprocess.PIPE,
            check=False,
        )
        assert_equal(result.returncode, 2, "json bounded verifier failure rc")
        payload = parse_json(result.stdout)
        blocked = payload["blocked_prs"]
        if len(blocked) != 1:
            raise AssertionError(blocked)
        verifier_result = blocked[0]
        if "stdout" in verifier_result or "stderr" in verifier_result:
            raise AssertionError(verifier_result)
        if verifier_result.get("stdout_preview") != "json-line-1\njson-line-2":
            raise AssertionError(verifier_result)
        if verifier_result.get("stdout_truncated") is not True:
            raise AssertionError(verifier_result)
        if "json-line-3" in json.dumps(verifier_result):
            raise AssertionError(verifier_result)


def assert_head_oid_mismatch_blocks_pr() -> None:
    with tempfile.TemporaryDirectory() as tmp:
        root = pathlib.Path(tmp)
        fixture = GitFixture(root)
        head = fixture.make_pr(1, {"one.txt": "one\n"})
        bin_dir = write_fake_gh(
            root,
            views={1: approved_pr_view("0" * 40)},
        )
        result = run_preflight_with_gh(fixture.repo, fixture.remote, bin_dir, "1")
        assert_equal(result.returncode, 2, "head mismatch rc")
        payload = parse_json(result.stdout)
        blocked = payload["blocked_prs"]
        assert_equal(len(blocked), 1, "head mismatch blocked count")
        assert_equal(blocked[0]["pr"], 1, "head mismatch pr")
        assert_equal(blocked[0]["type"], "head_mismatch", "head mismatch type")
        assert_equal(payload["lane_statuses"]["identity"], "blocked", "head mismatch identity lane")


def assert_wrong_base_ref_is_inconclusive() -> None:
    with tempfile.TemporaryDirectory() as tmp:
        root = pathlib.Path(tmp)
        fixture = GitFixture(root)
        head = fixture.make_pr(1, {"one.txt": "one\n"})
        bin_dir = write_fake_gh(
            root,
            views={1: approved_pr_view(head, base="release")},
        )
        result = run_preflight_with_gh(fixture.repo, fixture.remote, bin_dir, "1")
        assert_equal(result.returncode, 3, "wrong base rc")
        payload = parse_json(result.stdout)
        blocked = payload["blocked_prs"]
        assert_equal(len(blocked), 1, "wrong base blocked count")
        assert_equal(blocked[0]["pr"], 1, "wrong base pr")
        assert_equal(blocked[0]["type"], "base_mismatch", "wrong base type")
        assert_equal(payload["lane_statuses"]["identity"], "inconclusive", "wrong base identity lane")
        assert_equal((payload["verdict"], payload["contract_exit_code"]), ("inconclusive", 3), "wrong base contract")


def assert_source_pr_iteration_checks_are_queue_admitted() -> None:
    with tempfile.TemporaryDirectory() as tmp:
        root = pathlib.Path(tmp)
        fixture = GitFixture(root)
        head = fixture.make_pr(1, {"one.txt": "one\n"})
        bin_dir = write_fake_gh(
            root,
            views={1: approved_pr_view(head)},
            checks=passing_source_pr_checks(1),
            required_checks={
                1: passing_source_pr_branch_required_checks(),
            },
        )
        result = run_preflight_with_gh(fixture.repo, fixture.remote, bin_dir, "1")
        assert_equal(result.returncode, 0, "source PR iteration checks rc")
        payload = parse_json(result.stdout)
        assert_equal(payload["wave_status"], "ready", "source PR iteration checks wave")
        assert_equal(
            (payload["verdict"], payload["contract_exit_code"]),
            ("queue_as_one_wave", 0),
            "source PR iteration checks contract",
        )
        readiness = payload["readiness"][0]
        assert_equal(
            sorted(check["name"] for check in readiness["checks"]),
            ["actionlint", "host-health"],
            "source PR branch-required checks",
        )
        assert_equal(
            sorted(check["name"] for check in readiness["source_checks"]),
            [
                "actionlint",
                "backtester-gate-iteration",
                "gate-iteration",
                "host-health",
            ],
            "source PR all-check evidence",
        )
        missing_gate_findings = [
            finding
            for finding in payload["findings"]
            if finding["reason_code"] == "required_check_missing"
            and (
                finding["evidence"].get("check_name") in {"gate", "backtester-gate"}
                or finding["evidence"].get("merge_condition_check") in {"gate", "backtester-gate"}
            )
        ]
        assert_equal(missing_gate_findings, [], "source PR must not require merge proof gates")


def assert_aliased_source_check_pending_is_inconclusive_at_runtime() -> None:
    with tempfile.TemporaryDirectory() as tmp:
        root = pathlib.Path(tmp)
        fixture = GitFixture(root)
        head = fixture.make_pr(1, {"one.txt": "one\n"})
        checks = [
            {
                **check,
                "state": "PENDING",
                "bucket": "pending",
            }
            if check["name"] == "gate-iteration"
            else check
            for check in passing_source_pr_checks(1)[1]
        ]
        bin_dir = write_fake_gh(
            root,
            views={1: approved_pr_view(head)},
            checks={1: checks},
            required_checks={1: passing_source_pr_branch_required_checks()},
        )
        result = run_preflight_with_gh(fixture.repo, fixture.remote, bin_dir, "1")
        assert_equal(result.returncode, 3, "aliased pending source check rc")
        payload = parse_json(result.stdout)
        pending_findings = [
            finding
            for finding in payload["findings"]
            if finding["reason_code"] == "required_check_pending"
        ]
        if len(pending_findings) != 1:
            raise AssertionError(payload["findings"])
        evidence = pending_findings[0]["evidence"]
        assert_equal(evidence["check_name"], "gate-iteration", "pending source check name")
        assert_equal(evidence["merge_condition_check"], "gate", "pending merge condition")
        assert_equal(payload["lane_statuses"]["readiness"], "inconclusive", "aliased pending readiness lane")
        assert_equal(
            (payload["verdict"], payload["contract_exit_code"]),
            ("inconclusive", 3),
            "aliased pending source check contract",
        )


def assert_aliased_source_check_failure_blocks_at_runtime() -> None:
    with tempfile.TemporaryDirectory() as tmp:
        root = pathlib.Path(tmp)
        fixture = GitFixture(root)
        head = fixture.make_pr(1, {"one.txt": "one\n"})
        checks = [
            {
                **check,
                "state": "FAILURE",
                "bucket": "fail",
            }
            if check["name"] == "gate-iteration"
            else check
            for check in passing_source_pr_checks(1)[1]
        ]
        bin_dir = write_fake_gh(
            root,
            views={1: approved_pr_view(head)},
            checks={1: checks},
            required_checks={1: passing_source_pr_branch_required_checks()},
        )
        result = run_preflight_with_gh(fixture.repo, fixture.remote, bin_dir, "1")
        assert_equal(result.returncode, 2, "aliased failed source check rc")
        payload = parse_json(result.stdout)
        failed_findings = [
            finding
            for finding in payload["findings"]
            if finding["reason_code"] == "required_check_failed"
        ]
        if len(failed_findings) != 1:
            raise AssertionError(payload["findings"])
        evidence = failed_findings[0]["evidence"]
        assert_equal(evidence["check_name"], "gate-iteration", "failed source check name")
        assert_equal(evidence["merge_condition_check"], "gate", "failed merge condition")
        assert_equal(payload["lane_statuses"]["readiness"], "blocked", "aliased failed readiness lane")
        assert_equal(
            (payload["verdict"], payload["contract_exit_code"]),
            ("blocked", 2),
            "aliased failed source check contract",
        )


def assert_required_merge_proof_check_on_source_pr_is_inconclusive() -> None:
    with tempfile.TemporaryDirectory() as tmp:
        root = pathlib.Path(tmp)
        fixture = GitFixture(root)
        head = fixture.make_pr(1, {"one.txt": "one\n"})
        bin_dir = write_fake_gh(
            root,
            views={1: approved_pr_view(head)},
            checks=passing_source_pr_checks(1),
            required_checks={
                1: [
                    {"name": "gate", "state": "PENDING", "bucket": "pending", "workflow": "CI"},
                    {
                        "name": "backtester-gate",
                        "state": "PENDING",
                        "bucket": "pending",
                        "workflow": "Backtester CI",
                    },
                    *passing_source_pr_branch_required_checks(),
                ]
            },
        )
        result = run_preflight_with_gh(fixture.repo, fixture.remote, bin_dir, "1")
        assert_equal(result.returncode, 3, "source PR required merge proof check rc")
        payload = parse_json(result.stdout)
        blocked = payload["blocked_prs"]
        assert_equal(len(blocked), 1, "source PR required merge proof check blocked count")
        assert_equal(blocked[0]["type"], "required_check_pending", "source PR required proof block")
        assert_equal(
            (payload["verdict"], payload["contract_exit_code"]),
            ("inconclusive", 3),
            "source PR required merge proof check contract",
        )


def assert_required_check_pending_is_inconclusive() -> None:
    with tempfile.TemporaryDirectory() as tmp:
        root = pathlib.Path(tmp)
        fixture = GitFixture(root)
        head = fixture.make_pr(1, {"one.txt": "one\n"})
        bin_dir = write_fake_gh(
            root,
            views={1: approved_pr_view(head)},
            checks={1: [{"name": "gate", "state": "PENDING", "bucket": "pending", "workflow": "CI"}]},
        )
        result = run_preflight_with_gh(fixture.repo, fixture.remote, bin_dir, "1")
        assert_equal(result.returncode, 3, "pending check rc")
        payload = parse_json(result.stdout)
        blocked = payload["blocked_prs"]
        assert_equal(len(blocked), 1, "pending check blocked count")
        assert_equal(blocked[0]["pr"], 1, "pending check pr")
        assert_equal(blocked[0]["type"], "required_check_pending", "pending check type")
        assert_equal(payload["lane_statuses"]["readiness"], "inconclusive", "pending check readiness lane")
        assert_equal((payload["verdict"], payload["contract_exit_code"]), ("inconclusive", 3), "pending check contract")


def assert_required_check_neutral_is_inconclusive_at_runtime() -> None:
    with tempfile.TemporaryDirectory() as tmp:
        root = pathlib.Path(tmp)
        fixture = GitFixture(root)
        head = fixture.make_pr(1, {"one.txt": "one\n"})
        bin_dir = write_fake_gh(
            root,
            views={1: approved_pr_view(head)},
            checks={1: [{"name": "gate", "state": "NEUTRAL", "bucket": "pass", "workflow": "CI"}]},
        )
        result = run_preflight_with_gh(fixture.repo, fixture.remote, bin_dir, "1")
        assert_equal(result.returncode, 3, "neutral check rc")
        payload = parse_json(result.stdout)
        blocked = payload["blocked_prs"]
        assert_equal(len(blocked), 1, "neutral check blocked count")
        assert_equal(blocked[0]["pr"], 1, "neutral check pr")
        assert_equal(blocked[0]["type"], "required_check_skipped", "neutral check type")
        assert_equal(payload["lane_statuses"]["readiness"], "inconclusive", "neutral check readiness lane")
        assert_equal((payload["verdict"], payload["contract_exit_code"]), ("inconclusive", 3), "neutral check contract")


def assert_empty_required_checks_are_inconclusive_at_runtime() -> None:
    with tempfile.TemporaryDirectory() as tmp:
        root = pathlib.Path(tmp)
        fixture = GitFixture(root)
        head = fixture.make_pr(1, {"one.txt": "one\n"})
        bin_dir = write_fake_gh(root, views={1: approved_pr_view(head)})
        result = run_preflight_with_gh(fixture.repo, fixture.remote, bin_dir, "1")
        assert_equal(result.returncode, 3, "empty required checks rc")
        payload = parse_json(result.stdout)
        blocked = payload["blocked_prs"]
        assert_equal(len(blocked), 1, "empty required checks blocked count")
        assert_equal(blocked[0]["pr"], 1, "empty required checks pr")
        assert_equal(blocked[0]["type"], "required_check_missing", "empty required checks type")
        assert_equal(
            payload["lane_statuses"]["readiness"],
            "inconclusive",
            "empty required checks readiness lane",
        )
        assert_equal(
            (payload["verdict"], payload["contract_exit_code"]),
            ("inconclusive", 3),
            "empty required checks contract",
        )


def assert_selected_mergify_check_missing_is_inconclusive_at_runtime() -> None:
    with tempfile.TemporaryDirectory() as tmp:
        root = pathlib.Path(tmp)
        fixture = GitFixture(root)
        head = fixture.make_pr(1, {"one.txt": "one\n"})
        bin_dir = write_fake_gh(
            root,
            views={1: approved_pr_view(head)},
            checks={1: [passing_source_pr_check("gate-iteration")]},
        )
        result = run_preflight_with_gh(fixture.repo, fixture.remote, bin_dir, "1")
        assert_equal(result.returncode, 3, "missing selected Mergify check rc")
        payload = parse_json(result.stdout)
        missing_findings = [
            finding
            for finding in payload["findings"]
            if finding["reason_code"] == "required_check_missing"
        ]
        missing_contexts = {
            (
                finding["evidence"]["check_name"],
                finding["evidence"].get("merge_condition_check"),
                finding["evidence"]["pr"],
                finding["evidence"]["queue_rule"],
            )
            for finding in missing_findings
        }
        assert_equal(
            missing_contexts,
            {
                ("backtester-gate-iteration", "backtester-gate", 1, "default"),
                ("actionlint", None, 1, "default"),
                ("host-health", None, 1, "default"),
            },
            "missing selected Mergify check context",
        )
        readiness_findings = [
            finding
            for finding in payload["findings"]
            if finding["reason_code"] == "readiness_ready"
        ]
        assert_equal(readiness_findings, [], "missing selected Mergify check readiness-ready findings")
        assert_equal(
            payload["lane_statuses"]["readiness"],
            "inconclusive",
            "missing selected Mergify check readiness lane",
        )
        assert_equal(
            (payload["verdict"], payload["contract_exit_code"]),
            ("inconclusive", 3),
            "missing selected Mergify check contract",
        )


def assert_required_check_missing_identity_is_inconclusive_at_runtime() -> None:
    cases = (
        ("missing name", {"state": "SUCCESS", "bucket": "pass", "workflow": "CI"}),
        ("missing workflow", {"name": "gate", "state": "SUCCESS", "bucket": "pass"}),
    )
    for label, check in cases:
        with tempfile.TemporaryDirectory() as tmp:
            root = pathlib.Path(tmp)
            fixture = GitFixture(root)
            head = fixture.make_pr(1, {"one.txt": "one\n"})
            bin_dir = write_fake_gh(
                root,
                views={1: approved_pr_view(head)},
                checks={1: [check]},
            )
            result = run_preflight_with_gh(fixture.repo, fixture.remote, bin_dir, "1")
            assert_equal(result.returncode, 3, f"{label} check identity rc")
            payload = parse_json(result.stdout)
            blocked = payload["blocked_prs"]
            assert_equal(len(blocked), 1, f"{label} check identity blocked count")
            assert_equal(blocked[0]["pr"], 1, f"{label} check identity pr")
            assert_equal(blocked[0]["type"], "required_check_unknown", f"{label} check identity type")
            assert_equal(
                payload["lane_statuses"]["readiness"],
                "inconclusive",
                f"{label} check identity readiness lane",
            )
            assert_equal(
                (payload["verdict"], payload["contract_exit_code"]),
                ("inconclusive", 3),
                f"{label} check identity contract",
            )


def assert_required_check_wrong_workflow_is_inconclusive_at_runtime() -> None:
    with tempfile.TemporaryDirectory() as tmp:
        root = pathlib.Path(tmp)
        fixture = GitFixture(root)
        head = fixture.make_pr(1, {"one.txt": "one\n"})
        checks = passing_source_pr_checks(1)[1]
        checks[0] = {**checks[0], "workflow": "Wrong CI"}
        bin_dir = write_fake_gh(
            root,
            views={1: approved_pr_view(head)},
            checks={1: checks},
        )
        result = run_preflight_with_gh(fixture.repo, fixture.remote, bin_dir, "1")
        assert_equal(result.returncode, 3, "wrong check workflow rc")
        payload = parse_json(result.stdout)
        wrong_workflow_findings = [
            finding
            for finding in payload["findings"]
            if finding["reason_code"] == "required_check_wrong_workflow"
        ]
        if len(wrong_workflow_findings) != 1:
            raise AssertionError(payload["findings"])
        evidence = wrong_workflow_findings[0]["evidence"]
        assert_equal(evidence["check_name"], "gate-iteration", "wrong check workflow name")
        assert_equal(evidence["merge_condition_check"], "gate", "wrong check merge condition")
        assert_equal(evidence["workflow"], "Wrong CI", "wrong check workflow actual")
        assert_equal(evidence["expected_workflow"], "CI", "wrong check workflow expected")
        readiness_findings = [
            finding
            for finding in payload["findings"]
            if finding["reason_code"] == "readiness_ready"
        ]
        assert_equal(readiness_findings, [], "wrong check workflow readiness-ready findings")
        assert_equal(payload["lane_statuses"]["readiness"], "inconclusive", "wrong check workflow lane")


def assert_selected_mergify_reviewer_must_approve_at_runtime() -> None:
    with tempfile.TemporaryDirectory() as tmp:
        root = pathlib.Path(tmp)
        fixture = GitFixture(root)
        head = fixture.make_pr(1, {"one.txt": "one\n"})
        bin_dir = write_fake_gh(
            root,
            views={1: approved_pr_view(head, approving_reviewers=("other-reviewer",))},
            checks=passing_source_pr_checks(1),
        )
        result = run_preflight_with_gh(fixture.repo, fixture.remote, bin_dir, "1")
        assert_equal(result.returncode, 2, "required reviewer identity rc")
        payload = parse_json(result.stdout)
        reviewer_findings = [
            finding
            for finding in payload["findings"]
            if finding["reason_code"] == "mergify_required_reviewer_missing"
        ]
        if len(reviewer_findings) != 1:
            raise AssertionError(payload["findings"])
        evidence = reviewer_findings[0]["evidence"]
        assert_equal(evidence["pr"], 1, "required reviewer pr")
        assert_equal(evidence["queue_rule"], "default", "required reviewer queue")
        assert_equal(evidence["required_reviewers"], ["sp-reviewer"], "required reviewers")
        assert_equal(evidence["approved_reviewers"], ["other-reviewer"], "approved reviewers")
        assert_equal(payload["lane_statuses"]["readiness"], "blocked", "required reviewer lane")


def assert_required_check_exit_code_one_stays_readiness_failure() -> None:
    with tempfile.TemporaryDirectory() as tmp:
        root = pathlib.Path(tmp)
        fixture = GitFixture(root)
        head = fixture.make_pr(1, {"one.txt": "one\n"})
        bin_dir = write_fake_gh(
            root,
            views={1: approved_pr_view(head)},
            checks={1: [{"name": "gate", "state": "FAILURE", "bucket": "fail", "workflow": "CI"}]},
            check_exit_codes={1: 1},
        )
        result = run_preflight_with_gh(fixture.repo, fixture.remote, bin_dir, "1")
        assert_equal(result.returncode, 2, "failed check rc")
        payload = parse_json(result.stdout)
        blocked = payload["blocked_prs"]
        assert_equal(len(blocked), 1, "failed check blocked count")
        assert_equal(blocked[0]["pr"], 1, "failed check pr")
        assert_equal(blocked[0]["type"], "required_check_failed", "failed check type")
        assert_equal(payload["lane_statuses"]["readiness"], "blocked", "failed check readiness lane")
        assert_equal((payload["verdict"], payload["contract_exit_code"]), ("blocked", 2), "failed check contract")


def assert_required_check_exit_code_two_stays_readiness_failure() -> None:
    with tempfile.TemporaryDirectory() as tmp:
        root = pathlib.Path(tmp)
        fixture = GitFixture(root)
        head = fixture.make_pr(1, {"one.txt": "one\n"})
        bin_dir = write_fake_gh(
            root,
            views={1: approved_pr_view(head)},
            checks={1: [{"name": "gate", "state": "CANCELLED", "bucket": "cancel", "workflow": "CI"}]},
            check_exit_codes={1: 2},
        )
        result = run_preflight_with_gh(fixture.repo, fixture.remote, bin_dir, "1")
        assert_equal(result.returncode, 2, "cancelled check rc")
        payload = parse_json(result.stdout)
        blocked = payload["blocked_prs"]
        assert_equal(len(blocked), 1, "cancelled check blocked count")
        assert_equal(blocked[0]["pr"], 1, "cancelled check pr")
        assert_equal(blocked[0]["type"], "required_check_failed", "cancelled check type")
        assert_equal(payload["lane_statuses"]["readiness"], "blocked", "cancelled check readiness lane")
        assert_equal((payload["verdict"], payload["contract_exit_code"]), ("blocked", 2), "cancelled check contract")


def assert_partial_gh_metadata_failure_preserves_other_readiness() -> None:
    with tempfile.TemporaryDirectory() as tmp:
        root = pathlib.Path(tmp)
        fixture = GitFixture(root)
        head = fixture.make_pr(1, {"one.txt": "one\n"})
        fixture.make_pr(2, {"two.txt": "two\n"})
        bin_dir = write_fake_gh(
            root,
            views={1: approved_pr_view(head)},
            failed_views={2: "simulated metadata failure"},
            checks=passing_source_pr_checks(1),
        )
        result = run_preflight_with_gh(fixture.repo, fixture.remote, bin_dir, "1", "2")
        assert_equal(result.returncode, 3, "partial metadata failure rc")
        payload = parse_json(result.stdout)
        readiness = {item["pr"]: item for item in payload["readiness"]}
        if "metadata" not in readiness[1]:
            raise AssertionError(payload)
        if readiness[2].get("metadata_unavailable") is not True:
            raise AssertionError(payload)
        blocked = payload["blocked_prs"]
        if len(blocked) != 1 or blocked[0]["pr"] != 2 or blocked[0]["type"] != "metadata_unavailable":
            raise AssertionError(payload)
        if [batch["prs"] for batch in payload["batches"]] != [[1]]:
            raise AssertionError(payload)
        warnings = payload.get("metadata_warnings")
        if not isinstance(warnings, list) or "PR #2" not in warnings[0]:
            raise AssertionError(payload)


def assert_fetch_failure_after_readiness_is_inconclusive_not_tool_error() -> None:
    with tempfile.TemporaryDirectory() as tmp:
        root = pathlib.Path(tmp)
        fixture = GitFixture(root)
        head = fixture.make_pr(1, {"one.txt": "one\n"})
        missing_head = "1" * 40
        bin_dir = write_fake_gh(
            root,
            views={
                1: approved_pr_view(head),
                2: approved_pr_view(missing_head),
            },
            checks=passing_source_pr_checks(1, 2),
        )
        command = [
            sys.executable,
            str(SCRIPT_PATH),
            "--origin",
            str(fixture.remote),
            "--base",
            "main",
            "--expected-base-sha",
            fixture.base,
            "--expected-head-sha",
            f"1={head}",
            "--expected-head-sha",
            f"2={missing_head}",
            "--verifier-profile",
            "none",
            "--json",
            "1",
            "2",
        ]
        env = os.environ.copy()
        env["PATH"] = str(bin_dir) + os.pathsep + env["PATH"]
        result = subprocess.run(
            command,
            cwd=fixture.repo,
            env=env,
            text=True,
            stdout=subprocess.PIPE,
            stderr=subprocess.PIPE,
            check=False,
        )
        assert_equal(result.returncode, 3, "missing fetch rc")
        payload = parse_json(result.stdout)
        blocked = payload["blocked_prs"]
        if len(blocked) != 1 or blocked[0]["pr"] != 2 or blocked[0]["type"] != "head_unavailable":
            raise AssertionError(payload)
        assert_equal(
            blocked[0]["reason"],
            "PR #2 head ref was not found; ensure the PR exists and has a fetchable head",
            "missing fetch reason",
        )
        if [batch["prs"] for batch in payload["batches"]] != [[1]]:
            raise AssertionError(payload)
        assert_equal(payload["lane_statuses"]["identity"], "inconclusive", "missing fetch identity lane")
        assert_equal((payload["verdict"], payload["contract_exit_code"]), ("inconclusive", 3), "missing fetch contract")


def assert_non_missing_head_fetch_failure_is_inspection_error() -> None:
    module = load_preflight_module()
    with tempfile.TemporaryDirectory() as tmp:
        root = pathlib.Path(tmp)
        fixture = GitFixture(root)
        fixture.make_pr(1, {"one.txt": "one\n"})
        fetch_refs = module.PrivateFetchRefs.create(fixture.repo, 30)
        try:
            heads, blocks = module.fetch_available_pr_heads(
                fetch_refs=fetch_refs,
                origin=str(root / "not-a-remote.git"),
                requested=(1,),
                blocked_numbers=set(),
            )
        finally:
            fetch_refs.cleanup()
        assert_equal(heads, {}, "inspection error heads")
        if len(blocks) != 1 or blocks[0]["pr"] != 1 or blocks[0]["type"] != "head_fetch_failed":
            raise AssertionError(blocks)
        if "head ref was not found" in str(blocks[0]["reason"]):
            raise AssertionError(blocks)


def assert_invalid_pr_input_is_rejected() -> None:
    with tempfile.TemporaryDirectory() as tmp:
        root = pathlib.Path(tmp)
        fixture = GitFixture(root)
        command = [
            sys.executable,
            str(SCRIPT_PATH),
            "--origin",
            str(fixture.remote),
            "--base",
            "main",
            "--expected-base-sha",
            git(fixture.repo, "rev-parse", "main"),
            "--expected-head-sha",
            f"1={'0' * 40}",
            "--no-gh",
            "--verifier-profile",
            "none",
            "--json",
            "abc",
        ]
        result = subprocess.run(
            command,
            cwd=fixture.repo,
            text=True,
            stdout=subprocess.PIPE,
            stderr=subprocess.PIPE,
            check=False,
        )
        assert_equal(result.returncode, 4, "invalid PR rc")
        assert "PR numbers must be positive integers" in result.stderr, result.stderr


def assert_missing_expected_base_sha_is_rejected() -> None:
    with tempfile.TemporaryDirectory() as tmp:
        root = pathlib.Path(tmp)
        fixture = GitFixture(root)
        fixture.make_pr(1, {"one.txt": "one\n"})
        command = [
            sys.executable,
            str(SCRIPT_PATH),
            "--origin",
            str(fixture.remote),
            "--base",
            "main",
            "--no-gh",
            "--verifier-profile",
            "none",
            "--json",
            "1",
        ]
        result = subprocess.run(
            command,
            cwd=fixture.repo,
            text=True,
            stdout=subprocess.PIPE,
            stderr=subprocess.PIPE,
            check=False,
        )
        assert_equal(result.returncode, 4, "missing expected base sha rc")
        assert "--expected-base-sha" in result.stderr, result.stderr


def assert_missing_expected_head_sha_is_rejected() -> None:
    with tempfile.TemporaryDirectory() as tmp:
        root = pathlib.Path(tmp)
        fixture = GitFixture(root)
        fixture.make_pr(1, {"one.txt": "one\n"})
        command = [
            sys.executable,
            str(SCRIPT_PATH),
            "--origin",
            str(fixture.remote),
            "--base",
            "main",
            "--expected-base-sha",
            git(fixture.repo, "rev-parse", "main"),
            "--no-gh",
            "--verifier-profile",
            "none",
            "--json",
            "1",
        ]
        result = subprocess.run(
            command,
            cwd=fixture.repo,
            text=True,
            stdout=subprocess.PIPE,
            stderr=subprocess.PIPE,
            check=False,
        )
        assert_equal(result.returncode, 4, "missing expected head sha rc")
        assert "the following arguments are required: --expected-head-sha" in result.stderr, result.stderr


def assert_missing_gh_reports_inconclusive_metadata() -> None:
    with tempfile.TemporaryDirectory() as tmp:
        root = pathlib.Path(tmp)
        fixture = GitFixture(root)
        head = fixture.make_pr(1, {"one.txt": "one\n"})
        bin_dir = root / "bin"
        bin_dir.mkdir()
        git_bin = shutil.which("git")
        if git_bin is None:
            raise AssertionError("git executable not found")
        (bin_dir / "git").symlink_to(git_bin)
        command = [
            sys.executable,
            str(SCRIPT_PATH),
            "--origin",
            str(fixture.remote),
            "--base",
            "main",
            "--expected-base-sha",
            fixture.base,
            "--expected-head-sha",
            f"1={head}",
            "--verifier-profile",
            "none",
            "--json",
            "1",
        ]
        env = os.environ.copy()
        env["PATH"] = str(bin_dir)
        result = subprocess.run(
            command,
            cwd=fixture.repo,
            env=env,
            text=True,
            stdout=subprocess.PIPE,
            stderr=subprocess.PIPE,
            check=False,
        )
        assert_equal(result.returncode, 3, "missing gh metadata rc")
        payload = parse_json(result.stdout)
        warnings = payload.get("metadata_warnings")
        if not isinstance(warnings, list) or not warnings:
            raise AssertionError(payload)
        if "gh executable not found" not in warnings[0]:
            raise AssertionError(warnings)
        blocked = payload["blocked_prs"]
        if len(blocked) != 1 or blocked[0]["pr"] != 1 or blocked[0]["type"] != "metadata_unavailable":
            raise AssertionError(payload)
        if payload["batches"] != []:
            raise AssertionError(payload["batches"])


def main() -> int:
    assert_contract_result_reduces_findings_by_table()
    assert_preflight_input_timeout_is_config_driven()
    assert_source_check_alias_targets_must_have_workflows()
    assert_source_check_evidence_fallback_is_precise()
    assert_git_and_gh_use_input_timeout()
    assert_gh_timeout_is_preflight_error()
    assert_merge_tree_timeout_is_preflight_error()
    assert_check_state_classification_is_table_driven()
    assert_input_failure_matrix_is_declarative()
    assert_mergify_config_field_handling_is_declarative()
    assert_preflight_artifact_classification_is_declarative()
    assert_preflight_artifact_finding_uses_classification_table()
    assert_contract_evaluator_reduces_normalized_evidence()
    assert_mergify_config_snapshot_uses_base_blob()
    assert_fetches_use_private_refs_without_fetch_head()
    assert_unsupported_mergify_queue_condition_does_not_match()
    assert_unsupported_mergify_queue_condition_route_is_inconclusive()
    assert_mergify_queue_routing_uses_pr_labels()
    assert_default_queue_above_max_is_split_advised()
    assert_default_queue_below_min_reports_wait_behavior()
    assert_invalid_mergify_config_does_not_route()
    assert_stale_base_sha_is_inconclusive()
    assert_unavailable_base_ref_is_inconclusive()
    assert_stale_expected_head_sha_blocks_pr()
    assert_clean_prs_batch_together()
    assert_clean_prs_verify_final_batch_once()
    assert_conflicting_pr_starts_later_batch()
    assert_ready_batch_conflict_is_split_advised()
    assert_order_dependent_conflict_context_is_reported()
    assert_pr_that_conflicts_with_base_is_blocked()
    assert_verifier_failure_blocks_bad_pr_before_batching()
    assert_batch_first_fallback_excludes_poisoned_pr_and_reverifies_remainder()
    assert_fallback_recombines_survivors_after_batch_max_split()
    assert_fallback_replaces_suffix_optimistic_conflicts()
    assert_preflight_source_fence_profile_selects_fences_only_by_scripts_diff()
    assert_scripts_pr_fence_regression_uses_full_profile()
    assert_verifier_progress_breadcrumb_precedes_final_output()
    assert_first_verifier_failure_breadcrumb_arrives_before_later_batch_finishes()
    assert_missing_verifier_executable_is_inconclusive()
    assert_unexpected_exception_is_not_split_advised()
    assert_verifier_timeout_is_inconclusive()
    assert_batch_verifier_failure_is_split_advised()
    assert_configured_verifier_profile_blocks_bad_pr()
    assert_plain_output_includes_verifier_failure_details()
    assert_plain_output_omits_successful_verifier_streams()
    assert_plain_output_bounds_failed_verifier_streams()
    assert_json_output_uses_bounded_verifier_previews()
    assert_head_oid_mismatch_blocks_pr()
    assert_wrong_base_ref_is_inconclusive()
    assert_source_pr_iteration_checks_are_queue_admitted()
    assert_aliased_source_check_pending_is_inconclusive_at_runtime()
    assert_aliased_source_check_failure_blocks_at_runtime()
    assert_required_merge_proof_check_on_source_pr_is_inconclusive()
    assert_required_check_pending_is_inconclusive()
    assert_required_check_neutral_is_inconclusive_at_runtime()
    assert_empty_required_checks_are_inconclusive_at_runtime()
    assert_selected_mergify_check_missing_is_inconclusive_at_runtime()
    assert_required_check_missing_identity_is_inconclusive_at_runtime()
    assert_required_check_wrong_workflow_is_inconclusive_at_runtime()
    assert_selected_mergify_reviewer_must_approve_at_runtime()
    assert_required_check_exit_code_one_stays_readiness_failure()
    assert_required_check_exit_code_two_stays_readiness_failure()
    assert_partial_gh_metadata_failure_preserves_other_readiness()
    assert_fetch_failure_after_readiness_is_inconclusive_not_tool_error()
    assert_non_missing_head_fetch_failure_is_inspection_error()
    assert_invalid_pr_input_is_rejected()
    assert_missing_expected_base_sha_is_rejected()
    assert_missing_expected_head_sha_is_rejected()
    assert_missing_gh_reports_inconclusive_metadata()
    print("OK: merge_queue_preflight tests passed.")
    return 0


if __name__ == "__main__":
    import lane_governor

    lane_governor.acquire()
    raise SystemExit(main())
