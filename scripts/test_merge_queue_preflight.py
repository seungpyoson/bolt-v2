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

import ci_provenance
import git_remote_utils
from ci_workflow_hygiene_test_helpers import (
    clone_fixture_repo,
    count_trace2_children,
    count_trace2_maintenance_children,
    init_fixture_repo,
    load_provenance,
    load_verifier,
    replace_once,
    replace_once_after,
    run_repo_git,
    run_verifier_main_with_no_mistakes,
    yaml_scalar_literal,
)
from git_maintenance import GIT_AUTO_MAINTENANCE_SUPPRESSION_CONFIG


REPO_ROOT = pathlib.Path(__file__).resolve().parents[1]
SCRIPT_PATH = REPO_ROOT / "scripts" / "merge_queue_preflight.py"
MERGIFY_YML = (REPO_ROOT / ".mergify.yml").read_text(encoding="utf-8")
EXPECTED_RESIDUAL_RISKS = [
    "base_or_head_drift_after_preflight",
    "post_merge_config_or_workflow_changes",
    "queue_metadata_drift",
    "live_queue_ordering",
    "reset_on_external_merge",
    "max_parallel_checks_cost",
]
EXPECTED_RESIDUAL_RISK_MESSAGES: dict[str, str] = {}
def mergify_queue_max_batch_size(queue_rule: str) -> int:
    return ci_provenance.MERGIFY_CONFIG_EXPECTATIONS["queue_rules"][queue_rule]["batch_size"]


def mergify_queue_conditions(queue_rule: str) -> list[str]:
    return list(ci_provenance.MERGIFY_CONFIG_EXPECTATIONS["queue_rules"][queue_rule]["queue_conditions"])


def mergify_required_reviewer() -> str:
    return ci_provenance.MERGIFY_CONFIG_EXPECTATIONS["required_reviewer"]


def mergify_required_merge_conditions() -> list[str]:
    expectations = ci_provenance.MERGIFY_CONFIG_EXPECTATIONS
    return [f"approved-reviews-by = {expectations['required_reviewer']}"]


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


def git(cwd: pathlib.Path, *args: str) -> str:
    return run_repo_git(cwd, *args).strip()


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
        init_fixture_repo(self.remote, "--bare")
        clone_fixture_repo(self.remote, self.repo)
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
            "message": EXPECTED_RESIDUAL_RISK_MESSAGES.get(reason_code, reason_code),
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
            "max_batch_size": mergify_queue_max_batch_size(queue_rule),
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
            input_timeout_seconds=17,
        )
        loaded = module.load_config(config)
    assert_equal(loaded.input_timeout_seconds, 17, "input timeout config")


def assert_real_preflight_config_loads() -> None:
    module = load_preflight_module()
    loaded = module.load_config(REPO_ROOT / "ci" / "rust-verification.toml")
    assert_equal(loaded.origin, "origin", "real preflight origin")
    assert_equal(loaded.base, "main", "real preflight base")


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
        "queue_rules[].merge_conditions": "required_reviewer_evidence",
        "queue_rules[].branch_protection_injection_mode": "explicit_support_or_inconclusive",
        "queue_rules[].batch_size": "scalar_single_pr_model",
        "queue_rules[].batch_max_wait_time": "explicit_support_or_inconclusive",
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
        "base_mismatch": ("identity", "pr", "inconclusive"),
        "head_mismatch": ("identity", "pr", "blocked"),
        "head_fetch_failed": ("identity", "pr", "inconclusive"),
        "head_unavailable": ("identity", "pr", "inconclusive"),
        "metadata_unavailable": ("readiness", "pr", "inconclusive"),
        "readiness_failed": ("readiness", "pr", "blocked"),
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
    *,
    input_timeout_seconds: int = 30,
) -> pathlib.Path:
    path = root / "preflight.toml"
    write(
        path,
        "[merge_queue_preflight]\n"
        'origin = "origin"\n'
        'base = "main"\n'
        "\n"
        "[merge_queue_preflight.timeouts]\n"
        f"input_seconds = {input_timeout_seconds}\n",
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
    approving_reviewers: tuple[str, ...] | None = None,
) -> dict[str, object]:
    reviewers = (mergify_required_reviewer(),) if approving_reviewers is None else approving_reviewers
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
            for reviewer in reviewers
        ],
        "title": "one",
        "url": "https://example.invalid/pull/1",
    }


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


def assert_advisory_check_matrix_does_not_affect_admission() -> None:
    variants = {
        "green": [
            {"name": "gate", "state": "SUCCESS", "bucket": "pass", "workflow": "CI"},
        ],
        "failed": [
            {"name": "gate", "state": "FAILURE", "bucket": "fail", "workflow": "CI"},
        ],
        "missing": [],
        "skipped": [
            {"name": "gate", "state": "SKIPPED", "bucket": "skipped", "workflow": "CI"},
        ],
        "cancelled": [
            {"name": "gate", "state": "CANCELLED", "bucket": "cancel", "workflow": "CI"},
        ],
        "unavailable": [
            {"name": "gate", "state": "SUCCESS", "bucket": "pass", "workflow": "CI"},
        ],
    }
    with tempfile.TemporaryDirectory() as tmp:
        root = pathlib.Path(tmp)
        fixture = GitFixture(root)
        head = fixture.make_pr(1, {"one.txt": "one\n"})
        decisions: dict[str, tuple[int, str]] = {}
        for label, checks in variants.items():
            variant_root = root / label
            variant_root.mkdir()
            bin_dir = write_fake_gh(
                variant_root,
                views={1: approved_pr_view(head)},
                checks={1: checks},
                required_checks={1: checks},
                check_exit_codes={1: 127 if label == "unavailable" else 0},
            )
            result = run_preflight_with_gh(fixture.repo, fixture.remote, bin_dir, "1")
            payload = parse_json(result.stdout)
            decisions[label] = (result.returncode, str(payload["verdict"]))
    expected = (0, "queue_as_one_wave")
    assert_equal(decisions, {label: expected for label in variants}, "advisory check matrix")


def assert_post_cutover_mergify_contract_is_review_only_and_single_pr() -> None:
    expectations = ci_provenance.MERGIFY_CONFIG_EXPECTATIONS
    if "required_checks" in expectations:
        raise AssertionError("post-cutover Mergify expectations must not mirror required CI checks")
    required_condition = f"approved-reviews-by = {expectations['required_reviewer']}"
    config = MERGIFY_YML
    if "check-success" in config:
        raise AssertionError("post-cutover Mergify config must not contain check-success predicates")
    for queue_name, queue in expectations["queue_rules"].items():
        assert_equal(queue["batch_size"], 1, f"{queue_name} single-PR batch")
        assert_equal(
            queue["branch_protection_injection_mode"],
            "none",
            f"{queue_name} branch-protection injection",
        )
    assert_equal(
        config.count("    branch_protection_injection_mode: none\n"),
        2,
        "disabled branch-protection injection",
    )
    assert_equal(
        config.count(f"      - {required_condition}\n"),
        2,
        "reviewer-only merge conditions",
    )


def assert_queue_ci_and_verifier_flags_are_removed() -> None:
    module = load_preflight_module()
    base_args = [
        "--expected-base-sha",
        "a" * 40,
        "--expected-head-sha",
        f"1={'b' * 40}",
        "1",
    ]
    for flag_args in (
        ["--verifier-profile", "none"],
        ["--run-verifier", "just fmt-check"],
    ):
        stderr = io.StringIO()
        try:
            with contextlib.redirect_stderr(stderr):
                module.parser().parse_args([*flag_args, *base_args])
        except SystemExit as exc:
            assert_equal(exc.code, 4, f"removed flag {flag_args[0]} exit")
        else:
            raise AssertionError(f"{flag_args[0]} must not remain a queue admission flag")


def assert_preflight_config_is_identity_only() -> None:
    module = load_preflight_module()
    loaded = module.load_config(REPO_ROOT / "ci" / "rust-verification.toml")
    assert_equal(
        set(loaded.__dataclass_fields__),
        {"origin", "base", "input_timeout_seconds"},
        "post-cutover preflight config fields",
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


def assert_private_fetches_do_not_write_checkout_refs() -> None:
    with tempfile.TemporaryDirectory() as tmp:
        fixture = GitFixture(pathlib.Path(tmp))
        fixture.make_pr(1, {"one.txt": "one\n"})
        checkout_preflight_refs = fixture.repo / ".git" / "refs" / "preflight"
        checkout_preflight_refs.mkdir(parents=True, exist_ok=True)
        original_mode = checkout_preflight_refs.stat().st_mode
        checkout_preflight_refs.chmod(0o500)
        try:
            rc, stdout, _ = run_preflight(
                fixture.repo,
                fixture.remote,
                "1",
                expect_success=False,
            )
        finally:
            checkout_preflight_refs.chmod(original_mode)
        payload = parse_json(stdout)
        assert_equal(rc, 3, "checkout-ref-blocked no-gh rc")
        assert_equal(set(payload["pr_heads"].keys()), {"1"}, "private fetches must not need checkout refs")
        leaked_refs = git(fixture.repo, "for-each-ref", "--format=%(refname)", "refs/preflight")
        assert_equal(leaked_refs, "", "checkout preflight refs must stay empty")


def assert_private_fetches_resolve_checkout_remote_names() -> None:
    with tempfile.TemporaryDirectory() as tmp:
        fixture = GitFixture(pathlib.Path(tmp))
        head = fixture.make_pr(1, {"one.txt": "one\n"})
        git(fixture.repo, "remote", "set-url", "origin", "../origin.git")
        checkout_preflight_refs = fixture.repo / ".git" / "refs" / "preflight"
        checkout_preflight_refs.mkdir(parents=True, exist_ok=True)
        original_mode = checkout_preflight_refs.stat().st_mode
        checkout_preflight_refs.chmod(0o500)
        try:
            command = [
                sys.executable,
                str(SCRIPT_PATH),
                "--origin",
                "origin",
                "--base",
                "main",
                "--expected-base-sha",
                fixture.base,
                "--expected-head-sha",
                f"1={head}",
                "--no-gh",
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
        finally:
            checkout_preflight_refs.chmod(original_mode)
        payload = parse_json(result.stdout)
        assert_equal(result.returncode, 3, "remote-name private fetch no-gh rc")
        assert_equal(set(payload["pr_heads"].keys()), {"1"}, "private fetches must resolve checkout remote names")


def assert_private_fetch_resolves_checkout_remote_to_url_without_private_remote() -> None:
    module = load_preflight_module()
    with tempfile.TemporaryDirectory() as tmp:
        fixture = GitFixture(pathlib.Path(tmp))
        git(fixture.repo, "remote", "set-url", "origin", "../origin.git")
        private_fetch = module.PrivateFetchRefs.create(fixture.repo, 10)
        try:
            resolved = private_fetch.fetch_origin("origin")
            assert_equal(resolved, str(fixture.remote.resolve()), "private fetch origin URL")
            configured_remotes = git(private_fetch.git_repo, "remote")
            assert_equal(configured_remotes, "", "private fetch must not configure temp remotes")
        finally:
            private_fetch.cleanup()


def assert_private_fetch_repo_persists_auto_maintenance_suppression() -> None:
    module = load_preflight_module()
    with tempfile.TemporaryDirectory() as tmp:
        fixture = GitFixture(pathlib.Path(tmp))
        private_fetch = module.PrivateFetchRefs.create(fixture.repo, 10)
        try:
            for key, expected in GIT_AUTO_MAINTENANCE_SUPPRESSION_CONFIG:
                result = module.git(
                    private_fetch.git_repo,
                    "config",
                    "--local",
                    "--get",
                    key,
                    check=False,
                    timeout_seconds=10,
                )
                assert_equal(result.returncode, 0, f"private fetch repo {key} status")
                assert_equal(
                    result.stdout.strip(), expected, f"private fetch repo {key} value"
                )
        finally:
            private_fetch.cleanup()


def assert_private_fetch_sha_spawns_no_background_maintenance() -> None:
    module = load_preflight_module()
    with tempfile.TemporaryDirectory() as tmp:
        root = pathlib.Path(tmp)
        fixture = GitFixture(root)
        requested = fixture.make_pr(1, {"one.txt": "one\n"})
        private_fetch = module.PrivateFetchRefs.create(fixture.repo, 10)
        trace = root / "private-fetch-trace.json"
        previous_trace = os.environ.get("GIT_TRACE2_EVENT")
        try:
            os.environ["GIT_TRACE2_EVENT"] = str(trace)
            fetched = private_fetch.fetch_sha(str(fixture.remote), requested, "probe")
            trace_children = count_trace2_children(trace)
            maintenance_children = count_trace2_maintenance_children(trace)
        finally:
            if previous_trace is None:
                os.environ.pop("GIT_TRACE2_EVENT", None)
            else:
                os.environ["GIT_TRACE2_EVENT"] = previous_trace
            private_fetch.cleanup()

        assert_equal(fetched, requested, "private fetch SHA")
        if trace_children == 0:
            raise AssertionError("private fetch Trace2 log recorded no child events")
        assert_equal(
            maintenance_children,
            0,
            "private fetch maintenance children before cleanup",
        )


def assert_private_fetch_resolves_raw_relative_origin_path() -> None:
    with tempfile.TemporaryDirectory() as tmp:
        fixture = GitFixture(pathlib.Path(tmp))
        head = fixture.make_pr(1, {"one.txt": "one\n"})
        command = [
            sys.executable,
            str(SCRIPT_PATH),
            "--origin",
            "../origin.git",
            "--base",
            "main",
            "--expected-base-sha",
            fixture.base,
            "--expected-head-sha",
            f"1={head}",
            "--no-gh",
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
        assert_equal(result.returncode, 3, "raw-relative-origin no-gh rc")
        assert_equal(set(payload["pr_heads"].keys()), {"1"}, "raw relative --origin path must fetch PR heads")


def assert_private_fetch_resolves_raw_bare_git_origin_path() -> None:
    with tempfile.TemporaryDirectory() as tmp:
        fixture = GitFixture(pathlib.Path(tmp))
        head = fixture.make_pr(1, {"one.txt": "one\n"})
        os.symlink(fixture.remote, fixture.repo / "origin.git")
        command = [
            sys.executable,
            str(SCRIPT_PATH),
            "--origin",
            "origin.git",
            "--base",
            "main",
            "--expected-base-sha",
            fixture.base,
            "--expected-head-sha",
            f"1={head}",
            "--no-gh",
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
        assert_equal(result.returncode, 3, "raw-bare-git-origin no-gh rc")
        assert_equal(set(payload["pr_heads"].keys()), {"1"}, "origin.git --origin path must fetch PR heads")


def loose_object_mtimes(repo: pathlib.Path) -> dict[str, int]:
    objects = repo / ".git" / "objects"
    mtimes: dict[str, int] = {}
    for shard in objects.iterdir():
        if not shard.is_dir() or len(shard.name) != 2:
            continue
        for path in shard.iterdir():
            if path.is_file():
                mtimes[str(path.relative_to(objects))] = path.stat().st_mtime_ns
    return mtimes


def assert_private_fetches_do_not_freshen_checkout_objects() -> None:
    with tempfile.TemporaryDirectory() as tmp:
        fixture = GitFixture(pathlib.Path(tmp))
        fixture.make_pr(1, {"one.txt": "one\n"})
        objects = [
            path
            for shard in (fixture.repo / ".git" / "objects").iterdir()
            if shard.is_dir() and len(shard.name) == 2
            for path in shard.iterdir()
            if path.is_file()
        ]
        old_ns = 1_700_000_000_000_000_000
        for path in objects:
            os.utime(path, ns=(old_ns, old_ns))
        before = loose_object_mtimes(fixture.repo)
        rc, stdout, _ = run_preflight(
            fixture.repo,
            fixture.remote,
            "1",
            expect_success=False,
        )
        payload = parse_json(stdout)
        assert_equal(rc, 3, "object-mtime no-gh rc")
        assert_equal(set(payload["pr_heads"].keys()), {"1"}, "object-mtime private fetch pr heads")
        after = loose_object_mtimes(fixture.repo)
        assert_equal(after, before, "private preflight must not freshen checkout loose objects")


def assert_remote_url_normalization_uses_shared_helper() -> None:
    source = SCRIPT_PATH.read_text(encoding="utf-8")
    if "REMOTE_URL_SCHEME_RE =" in source or "\ndef fetchable_remote_url(" in source:
        raise AssertionError("remote URL normalization must live in one shared helper")


def assert_shared_remote_url_normalization_matrix() -> None:
    with tempfile.TemporaryDirectory() as tmp:
        source_repo = pathlib.Path(tmp) / "repo"
        source_repo.mkdir()
        absolute = str((source_repo.parent / "origin.git").resolve())
        cases = {
            "https://example.invalid/org/repo.git": "https://example.invalid/org/repo.git",
            "ssh://example.invalid/org/repo.git": "ssh://example.invalid/org/repo.git",
            "git@example.invalid:org/repo.git": "git@example.invalid:org/repo.git",
            "file:///tmp/origin.git": "file:///tmp/origin.git",
            absolute: absolute,
            "~/origin.git": "~/origin.git",
            "../origin.git": str((source_repo / "../origin.git").resolve(strict=False)),
            "./origin.git": str((source_repo / "./origin.git").resolve(strict=False)),
            "origin.git": str((source_repo / "origin.git").resolve(strict=False)),
        }
        for value, expected in cases.items():
            assert_equal(
                git_remote_utils.fetchable_remote_url(value, source_repo),
                expected,
                f"fetchable remote URL for {value}",
            )
        assert_equal(
            git_remote_utils.fetchable_origin_argument("origin", source_repo),
            "origin",
            "bare remote name must stay a remote name",
        )
        assert_equal(
            git_remote_utils.fetchable_origin_argument("../origin.git", source_repo),
            str((source_repo / "../origin.git").resolve(strict=False)),
            "raw relative origin path must resolve from checkout",
        )
        assert_equal(
            git_remote_utils.fetchable_origin_argument("origin.git", source_repo),
            str((source_repo / "origin.git").resolve(strict=False)),
            "raw bare .git origin path must resolve from checkout",
        )


def assert_github_actions_auth_helper_fails_without_actions_identity() -> None:
    for env in (
        {"GITHUB_ACTIONS": "true", "GITHUB_REPOSITORY": "owner/repo"},
        {"GITHUB_ACTIONS": "true", "GITHUB_TOKEN": "token"},
    ):
        try:
            git_remote_utils.github_actions_git_auth_env(
                "https://github.com/owner/repo.git",
                env,
            )
        except RuntimeError as exc:
            if "GITHUB_TOKEN and GITHUB_REPOSITORY are both required in GitHub Actions" not in str(exc):
                raise AssertionError(f"unexpected missing identity error: {exc}") from exc
        else:
            raise AssertionError(f"GitHub Actions identity must fail closed for {env!r}")


def assert_github_actions_auth_helper_keeps_local_ambient_auth_optional() -> None:
    auth = git_remote_utils.github_actions_git_auth_env(
        "https://github.com/owner/repo.git",
        {},
    )
    assert_equal(auth, {}, "outside Actions, missing ambient GitHub identity stays unauthenticated")


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
            }
        ],
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
        )
        result = run_preflight_with_gh(fixture.repo, fixture.remote, bin_dir, "1", "2")
        assert_equal(result.returncode, 1, "queue routing rc")
        payload = parse_json(result.stdout)
        assert readiness_ready_finding(1, hotfix_head) in payload["findings"], (
            payload["findings"]
        )
        assert readiness_ready_finding(2, default_head) in payload["findings"], (
            payload["findings"]
        )
        assert mergify_queue_route_finding(1, "hotfix", ["hotfix"], mergify_queue_conditions("hotfix")) in payload["findings"], (
            payload["findings"]
        )
        assert mergify_queue_route_finding(2, "default", [], mergify_queue_conditions("default")) in payload["findings"], payload["findings"]
        assert mergify_required_reviewer_finding(
            "hotfix",
            [mergify_required_reviewer()],
            mergify_required_merge_conditions(),
        ) in payload["findings"], payload["findings"]
        assert mergify_required_reviewer_finding(
            "default",
            [mergify_required_reviewer()],
            mergify_required_merge_conditions(),
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
        max_batch_size = mergify_queue_max_batch_size("default")
        assert_equal(
            [batch["prs"] for batch in payload["batches"]],
            [[pr] for pr in range(1, 12)],
            "default queue above max size-valid batches",
        )
        assert mergify_queue_batch_above_max_finding("default", list(heads), max_batch_size) in payload["findings"], payload["findings"]


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


def assert_selected_mergify_reviewer_must_approve_at_runtime() -> None:
    with tempfile.TemporaryDirectory() as tmp:
        root = pathlib.Path(tmp)
        fixture = GitFixture(root)
        head = fixture.make_pr(1, {"one.txt": "one\n"})
        bin_dir = write_fake_gh(
            root,
            views={1: approved_pr_view(head, approving_reviewers=("other-reviewer",))},
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
        assert_equal(evidence["required_reviewers"], [mergify_required_reviewer()], "required reviewers")
        assert_equal(evidence["approved_reviewers"], ["other-reviewer"], "approved reviewers")
        assert_equal(payload["lane_statuses"]["readiness"], "blocked", "required reviewer lane")


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


def mergify_scalar_line(indent: str, key: str, value: object) -> str:
    return f"{indent}{key}: {yaml_scalar_literal(value)}\n"

def mergify_queue_batch_size_error(queue_name: str, batch_size: object) -> str:
    return f"{queue_name} batch_size must be {batch_size}"

def assert_mergify_config_gaps_are_reported() -> None:
    verifier = load_verifier()
    provenance = load_provenance()
    expectations = provenance.MERGIFY_CONFIG_EXPECTATIONS
    merge_queue_scalars = expectations["merge_queue"]
    queue_rules = expectations["queue_rules"]
    priority_rules = expectations["priority_rules"]
    required_reviewer = expectations["required_reviewer"]
    hotfix_queue = queue_rules["hotfix"]
    default_queue = queue_rules["default"]
    hotfix_priority = priority_rules["hotfix"]
    mergify_config = (REPO_ROOT / ".mergify.yml").read_text()
    baseline_errors = verifier.verify_mergify_config(mergify_config)
    if baseline_errors:
        raise AssertionError(f"real .mergify.yml must be clean, got: {baseline_errors}")

    result, output = run_verifier_main_with_no_mistakes(
        "commands:\n  test: just source-fence-static\n",
        write_mergify_config=False,
    )
    if result == 0 or ".mergify.yml is required for Mergify queue governance" not in output:
        raise AssertionError(f"verifier main must reject a missing .mergify.yml, got: {result}, {output!r}")

    hotfix_rule_start = mergify_config.index("  # Exceptional path only. Normal merge sessions use the default queue below.\n")
    default_rule_start = mergify_config.index("  - name: default\n")
    priority_rules_start = mergify_config.index("\npriority_rules:\n")
    hotfix_rule_block = mergify_config[hotfix_rule_start:default_rule_start]
    default_rule_block = mergify_config[default_rule_start:priority_rules_start]
    swapped_queue_rules = (
        mergify_config[:hotfix_rule_start]
        + default_rule_block
        + hotfix_rule_block
        + mergify_config[priority_rules_start:]
    )

    mutations = [
        (
            "missing max_parallel_checks",
            replace_once(
                mergify_config,
                mergify_scalar_line("  ", "max_parallel_checks", merge_queue_scalars["max_parallel_checks"]),
                "",
            ),
            f"merge_queue.max_parallel_checks must be {merge_queue_scalars['max_parallel_checks']}",
        ),
        (
            "reset disabled",
            replace_once(
                mergify_config,
                mergify_scalar_line("  ", "reset_on_external_merge", merge_queue_scalars["reset_on_external_merge"]),
                "  reset_on_external_merge: never\n",
            ),
            f"merge_queue.reset_on_external_merge must be {merge_queue_scalars['reset_on_external_merge']}",
        ),
        (
            "autoqueue enabled",
            replace_once(
                mergify_config,
                "  - name: default\n",
                "  - name: default\n    autoqueue: true\n",
            ),
            "default must not define unsupported key autoqueue",
        ),
        (
            "pull request rules enabled",
            mergify_config + "\npull_request_rules:\n  - name: autoqueue\n",
            "manual queueing only",
        ),
        (
            "merge protections enabled",
            mergify_config + "\nmerge_protections:\n  - name: autoqueue\n",
            "manual queueing only",
        ),
        (
            "defaults override enabled",
            mergify_config + "\ndefaults:\n  queue_rule:\n    batch_size: 1\n",
            "manual queueing only",
        ),
        (
            "remote config inheritance enabled",
            mergify_config + "\nextends: shared/mergify-config\n",
            "manual queueing only",
        ),
        (
            "commands restrictions inheritance enabled",
            mergify_config + "\ncommands_restrictions:\n  queue:\n    conditions: []\n",
            "manual queueing only",
        ),
        (
            "unknown top-level key enabled",
            mergify_config + "\nshared:\n  queue_branch_prefix: custom/merge-queue/\n",
            "unsupported top-level key shared",
        ),
        (
            "yaml merge key enabled",
            replace_once_after(
                mergify_config,
                "  - name: default\n",
                "    queue_conditions: []\n",
                "    <<: *default_queue\n    queue_conditions: []\n",
            ),
            "YAML merge key is forbidden",
        ),
        (
            "duplicate queue_rules top level",
            mergify_config + "\nqueue_rules:\n  - name: default\n",
            "duplicate key queue_rules",
        ),
        (
            "queue rule order swapped",
            swapped_queue_rules,
            "queue_rules must define exactly hotfix followed by default",
        ),
        (
            "quoted-name extra queue rule",
            replace_once(
                mergify_config,
                "  - name: default\n",
                "  - \"name\": sneaky\n"
                "    queue_conditions: []\n"
                "    merge_conditions: []\n"
                "    branch_protection_injection_mode: merge\n"
                "    batch_size: 1\n"
                "    batch_max_wait_time: 30 seconds\n"
                "    batch_max_failure_resolution_attempts: 0\n"
                "    checks_timeout: 150 minutes\n"
                "    draft_bot_account: null\n"
                "    merge_method: squash\n\n"
                "  - name: default\n",
            ),
            "queue_rules must define exactly hotfix followed by default",
        ),
        (
            "name-not-first extra queue rule",
            replace_once(
                mergify_config,
                "  - name: default\n",
                "  - queue_conditions: []\n"
                "    name: sneaky\n"
                "    merge_conditions: []\n"
                "    branch_protection_injection_mode: merge\n"
                "    batch_size: 1\n"
                "    batch_max_wait_time: 30 seconds\n"
                "    batch_max_failure_resolution_attempts: 0\n"
                "    checks_timeout: 150 minutes\n"
                "    draft_bot_account: null\n"
                "    merge_method: squash\n\n"
                "  - name: default\n",
            ),
            "queue_rules must define exactly hotfix followed by default",
        ),
        (
            "duplicate default queue conditions",
            replace_once(
                mergify_config,
                "  - name: default\n    queue_conditions: []\n",
                "  - name: default\n    queue_conditions: []\n    queue_conditions:\n      - check-success = gate\n",
            ),
            "duplicate key queue_conditions",
        ),
        (
            "quoted duplicate default queue conditions",
            replace_once(
                mergify_config,
                "  - name: default\n    queue_conditions: []\n",
                "  - name: default\n    queue_conditions: []\n    \"queue_conditions\":\n      - check-success = gate\n",
            ),
            "duplicate key queue_conditions",
        ),
        (
            "wide-indent merge queue unsupported key",
            replace_once(
                mergify_config,
                "merge_queue:\n"
                + mergify_scalar_line("  ", "max_parallel_checks", merge_queue_scalars["max_parallel_checks"])
                + mergify_scalar_line("  ", "reset_on_external_merge", merge_queue_scalars["reset_on_external_merge"]),
                "merge_queue:\n"
                + mergify_scalar_line("    ", "max_parallel_checks", merge_queue_scalars["max_parallel_checks"])
                + mergify_scalar_line("    ", "reset_on_external_merge", merge_queue_scalars["reset_on_external_merge"])
                + "    skip_intermediate_results: true\n",
            ),
            "merge_queue must not define unsupported key skip_intermediate_results",
        ),
        (
            "default custom queue branch prefix",
            replace_once_after(
                mergify_config,
                "  - name: default\n",
                "    queue_conditions: []\n",
                "    queue_conditions: []\n    queue_branch_prefix: custom/merge-queue/\n",
            ),
            "default must not define unsupported key queue_branch_prefix",
        ),
        (
            "default editable queue branch",
            replace_once_after(
                mergify_config,
                "  - name: default\n",
                "    queue_conditions: []\n",
                "    queue_conditions: []\n    allow_queue_branch_edit: true\n",
            ),
            "default must not define unsupported key allow_queue_branch_edit",
        ),
        (
            "queue conditions require gate",
            replace_once(
                mergify_config,
                "    queue_conditions: []\n",
                "    queue_conditions:\n      - check-success = gate\n",
            ),
            "default queue_conditions must be []",
        ),
        (
            "hotfix queue conditions changed",
            replace_once_after(
                mergify_config,
                "  - name: hotfix\n    queue_conditions:\n",
                "      - label = hotfix\n",
                "      - label = urgent\n",
            ),
            "hotfix queue_conditions must be ['label = hotfix']",
        ),
        (
            "default missing reviewer condition",
            replace_once_after(
                mergify_config,
                "  - name: default\n",
                f"      - approved-reviews-by = {required_reviewer}\n",
                "",
            ),
            "default merge_conditions must be a list",
        ),
        (
            "hotfix missing reviewer condition",
            replace_once(
                mergify_config,
                f"      - approved-reviews-by = {required_reviewer}\n",
                "",
            ),
            "hotfix merge_conditions must be a list",
        ),
        (
            "default extra merge condition",
            replace_once_after(
                mergify_config,
                "  - name: default\n",
                f"      - approved-reviews-by = {required_reviewer}\n",
                f"      - approved-reviews-by = {required_reviewer}\n      - label = queue-proof\n",
            ),
            f"default merge_conditions must require only {required_reviewer}",
        ),
        (
            "queue-time injection",
            replace_once(
                mergify_config,
                mergify_scalar_line(
                    "    ",
                    "branch_protection_injection_mode",
                    hotfix_queue["branch_protection_injection_mode"],
                ),
                "    branch_protection_injection_mode: queue\n",
            ),
            f"hotfix branch_protection_injection_mode must be {hotfix_queue['branch_protection_injection_mode']}",
        ),
        (
            "default batch widened",
            replace_once_after(
                mergify_config,
                "  - name: default\n",
                mergify_scalar_line("    ", "batch_size", default_queue["batch_size"]),
                mergify_scalar_line("    ", "batch_size", default_queue["batch_size"] + 1),
            ),
            mergify_queue_batch_size_error("default", default_queue["batch_size"]),
        ),
        (
            "hotfix batch widened",
            replace_once(
                mergify_config,
                mergify_scalar_line("    ", "batch_size", hotfix_queue["batch_size"]),
                mergify_scalar_line("    ", "batch_size", hotfix_queue["batch_size"] + 1),
            ),
            mergify_queue_batch_size_error("hotfix", hotfix_queue["batch_size"]),
        ),
        (
            "default wait shortened",
            replace_once(
                mergify_config,
                mergify_scalar_line("    ", "batch_max_wait_time", default_queue["batch_max_wait_time"]),
                "    batch_max_wait_time: 30 seconds\n",
            ),
            f"default batch_max_wait_time must be {default_queue['batch_max_wait_time']}",
        ),
        (
            "hotfix wait lengthened",
            replace_once(
                mergify_config,
                mergify_scalar_line("    ", "batch_max_wait_time", hotfix_queue["batch_max_wait_time"]),
                "    batch_max_wait_time: 60 minutes\n",
            ),
            f"hotfix batch_max_wait_time must be {hotfix_queue['batch_max_wait_time']}",
        ),
        (
            "hotfix failure split enabled",
            replace_once(
                mergify_config,
                mergify_scalar_line(
                    "    ",
                    "batch_max_failure_resolution_attempts",
                    hotfix_queue["batch_max_failure_resolution_attempts"],
                ),
                mergify_scalar_line(
                    "    ",
                    "batch_max_failure_resolution_attempts",
                    default_queue["batch_max_failure_resolution_attempts"],
                ),
            ),
            f"hotfix batch_max_failure_resolution_attempts must be {hotfix_queue['batch_max_failure_resolution_attempts']}",
        ),
        (
            "default failure split disabled",
            replace_once_after(
                mergify_config,
                "  - name: default\n",
                mergify_scalar_line(
                    "    ",
                    "batch_max_failure_resolution_attempts",
                    default_queue["batch_max_failure_resolution_attempts"],
                ),
                mergify_scalar_line(
                    "    ",
                    "batch_max_failure_resolution_attempts",
                    hotfix_queue["batch_max_failure_resolution_attempts"],
                ),
            ),
            f"default batch_max_failure_resolution_attempts must be {default_queue['batch_max_failure_resolution_attempts']}",
        ),
        (
            "duplicate default wait",
            replace_once_after(
                mergify_config,
                "  - name: default\n",
                mergify_scalar_line("    ", "batch_max_wait_time", default_queue["batch_max_wait_time"]),
                mergify_scalar_line("    ", "batch_max_wait_time", default_queue["batch_max_wait_time"])
                + "    batch_max_wait_time: 30 seconds\n",
            ),
            "duplicate key batch_max_wait_time",
        ),
        (
            "unbounded timeout",
            replace_once(
                mergify_config,
                mergify_scalar_line("    ", "checks_timeout", hotfix_queue["checks_timeout"]),
                "    checks_timeout: auto\n",
            ),
            f"hotfix checks_timeout must be {hotfix_queue['checks_timeout']}",
        ),
        (
            "default timeout lowered",
            replace_once_after(
                mergify_config,
                "  - name: default\n",
                mergify_scalar_line("    ", "checks_timeout", default_queue["checks_timeout"]),
                "    checks_timeout: 60 minutes\n",
            ),
            f"default checks_timeout must be {default_queue['checks_timeout']}",
        ),
        (
            "draft impersonation",
            replace_once(
                mergify_config,
                mergify_scalar_line("    ", "draft_bot_account", hotfix_queue["draft_bot_account"]),
                '    draft_bot_account: "{{ author }}"\n',
            ),
            f"hotfix draft_bot_account must be {yaml_scalar_literal(hotfix_queue['draft_bot_account'])}",
        ),
        (
            "non-squash merge",
            replace_once(
                mergify_config,
                mergify_scalar_line("    ", "merge_method", hotfix_queue["merge_method"]),
                "    merge_method: merge\n",
            ),
            f"hotfix merge_method must be {hotfix_queue['merge_method']}",
        ),
        (
            "duplicate hotfix merge method",
            replace_once(
                mergify_config,
                "    merge_method: squash\n",
                "    merge_method: squash\n    merge_method: merge\n",
            ),
            "duplicate key merge_method",
        ),
        (
            "priority rules removed",
            replace_once(
                mergify_config,
                "\npriority_rules:\n"
                "  - name: hotfix\n"
                "    conditions:\n"
                "      - label = hotfix\n"
                + mergify_scalar_line("    ", "priority", hotfix_priority["priority"])
                + mergify_scalar_line(
                    "    ",
                    "allow_checks_interruption",
                    hotfix_priority["allow_checks_interruption"],
                ),
                "\n",
            ),
            "must define priority_rules",
        ),
        (
            "hotfix priority condition changed",
            replace_once_after(
                mergify_config,
                "priority_rules:\n  - name: hotfix\n    conditions:\n",
                "      - label = hotfix\n",
                "      - label = urgent\n",
            ),
            "hotfix priority conditions must be ['label = hotfix']",
        ),
        (
            "hotfix priority lowered",
            replace_once(
                mergify_config,
                mergify_scalar_line("    ", "priority", hotfix_priority["priority"]),
                "    priority: high\n",
            ),
            f"hotfix priority must be {hotfix_priority['priority']}",
        ),
        (
            "hotfix interruption disabled",
            replace_once(
                mergify_config,
                mergify_scalar_line(
                    "    ",
                    "allow_checks_interruption",
                    hotfix_priority["allow_checks_interruption"],
                ),
                "    allow_checks_interruption: false\n",
            ),
            f"hotfix allow_checks_interruption must be {yaml_scalar_literal(hotfix_priority['allow_checks_interruption'])}",
        ),
        (
            "quoted extra priority rule",
            replace_once(
                mergify_config,
                "priority_rules:\n  - name: hotfix\n",
                "priority_rules:\n  - \"name\": sneaky\n"
                "    conditions:\n"
                "      - label = hotfix\n"
                "    priority: 1\n"
                "    allow_checks_interruption: false\n"
                "  - name: hotfix\n",
            ),
            "priority_rules must define exactly hotfix",
        ),
    ]
    for label, mutated, expected in mutations:
        errors = verifier.verify_mergify_config(mutated)
        if not any(expected in error for error in errors):
            raise AssertionError(
                f"expected .mergify.yml {label} error containing {expected!r}, got: {errors}"
            )

def main() -> int:
    assert_advisory_check_matrix_does_not_affect_admission()
    assert_post_cutover_mergify_contract_is_review_only_and_single_pr()
    assert_queue_ci_and_verifier_flags_are_removed()
    assert_preflight_config_is_identity_only()
    assert_mergify_config_gaps_are_reported()
    assert_contract_result_reduces_findings_by_table()
    assert_preflight_input_timeout_is_config_driven()
    assert_real_preflight_config_loads()
    assert_git_and_gh_use_input_timeout()
    assert_gh_timeout_is_preflight_error()
    assert_merge_tree_timeout_is_preflight_error()
    assert_input_failure_matrix_is_declarative()
    assert_mergify_config_field_handling_is_declarative()
    assert_preflight_artifact_classification_is_declarative()
    assert_preflight_artifact_finding_uses_classification_table()
    assert_contract_evaluator_reduces_normalized_evidence()
    assert_mergify_config_snapshot_uses_base_blob()
    assert_fetches_use_private_refs_without_fetch_head()
    assert_private_fetches_do_not_write_checkout_refs()
    assert_private_fetches_resolve_checkout_remote_names()
    assert_private_fetch_resolves_checkout_remote_to_url_without_private_remote()
    assert_private_fetch_repo_persists_auto_maintenance_suppression()
    assert_private_fetch_sha_spawns_no_background_maintenance()
    assert_private_fetch_resolves_raw_relative_origin_path()
    assert_private_fetch_resolves_raw_bare_git_origin_path()
    assert_private_fetches_do_not_freshen_checkout_objects()
    assert_remote_url_normalization_uses_shared_helper()
    assert_shared_remote_url_normalization_matrix()
    assert_github_actions_auth_helper_fails_without_actions_identity()
    assert_github_actions_auth_helper_keeps_local_ambient_auth_optional()
    assert_unsupported_mergify_queue_condition_does_not_match()
    assert_unsupported_mergify_queue_condition_route_is_inconclusive()
    assert_mergify_queue_routing_uses_pr_labels()
    assert_default_queue_above_max_is_split_advised()
    assert_invalid_mergify_config_does_not_route()
    assert_stale_base_sha_is_inconclusive()
    assert_unavailable_base_ref_is_inconclusive()
    assert_stale_expected_head_sha_blocks_pr()
    assert_pr_that_conflicts_with_base_is_blocked()
    assert_head_oid_mismatch_blocks_pr()
    assert_wrong_base_ref_is_inconclusive()
    assert_selected_mergify_reviewer_must_approve_at_runtime()
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
