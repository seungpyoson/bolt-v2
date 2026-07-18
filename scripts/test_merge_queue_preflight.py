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
    "full_ci_result",
    "batch_verifier_scope",
    "source_fence_test_phase_skipped",
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
EXPECTED_RESIDUAL_RISK_MESSAGES = {
    "batch_verifier_scope": "verifier proof is batch-scoped for passing optimistic batches",
    "source_fence_test_phase_skipped": "source-fence fast path may skip fixture test suites for eligible diffs",
}
DEFAULT_SOURCE_FENCE_FULL_PROFILE_PATHSPECS = [
    "scripts",
    "justfile",
    "ci/rust-verification.toml",
    "ci/fail-closed-contracts.toml",
    "ci/fail-closed-exceptions.toml",
    "crates/backtesting-vertical-slice/ci/rust-verification.toml",
]
DEFAULT_SOURCE_FENCE_FENCES_ONLY_REWRITES = {
    "just source-fence-static": "just source-fence-static-fences-only",
}
SOURCE_PR_CHECK_WORKFLOWS = {
    "gate": "CI",
    "backtester-gate": "Backtester CI",
    "actionlint": "actionlint",
    "host-health": "CI",
}
SOURCE_CHECK_ALIASES: dict[str, str] = {}


def mergify_queue_max_batch_size(queue_rule: str) -> int:
    batch_size = ci_provenance.MERGIFY_CONFIG_EXPECTATIONS["queue_rules"][queue_rule]["batch_size"]
    if isinstance(batch_size, dict):
        return batch_size["max"]
    return batch_size


def mergify_queue_min_batch_size(queue_rule: str) -> int:
    batch_size = ci_provenance.MERGIFY_CONFIG_EXPECTATIONS["queue_rules"][queue_rule]["batch_size"]
    if isinstance(batch_size, dict):
        return batch_size["min"]
    return batch_size


def mergify_queue_wait_time(queue_rule: str) -> str:
    return ci_provenance.MERGIFY_CONFIG_EXPECTATIONS["queue_rules"][queue_rule]["batch_max_wait_time"]


def mergify_queue_conditions(queue_rule: str) -> list[str]:
    return list(ci_provenance.MERGIFY_CONFIG_EXPECTATIONS["queue_rules"][queue_rule]["queue_conditions"])


def mergify_required_reviewer() -> str:
    return ci_provenance.MERGIFY_CONFIG_EXPECTATIONS["required_reviewer"]


def mergify_required_merge_conditions() -> list[str]:
    expectations = ci_provenance.MERGIFY_CONFIG_EXPECTATIONS
    return [
        f"approved-reviews-by = {expectations['required_reviewer']}",
        *(f"check-success = {check_name}" for check_name in expectations["required_checks"]),
    ]


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
    return run_repo_git(cwd, *args).strip()


def write(path: pathlib.Path, text: str) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(text, encoding="utf-8")


def commit(repo: pathlib.Path, message: str) -> str:
    git(repo, "add", ".")
    git(repo, "commit", "-m", message)
    return git(repo, "rev-parse", "HEAD")


def preflight_transport_environment(
    repo: pathlib.Path,
    remote: pathlib.Path,
    environ: dict[str, str] | None = None,
) -> dict[str, str]:
    environment = dict(os.environ if environ is None else environ)
    base_path = environment.get("PREFLIGHT_TEST_BASE_PATH", environment.get("PATH", ""))
    real_git = environment.get("PREFLIGHT_TEST_REAL_GIT") or shutil.which(
        "git", path=base_path
    )
    if real_git is None:
        raise AssertionError("git executable unavailable")
    bin_dir = repo.parent / "preflight-transport-bin"
    shim = bin_dir / "git"
    write(
        shim,
        f"#!{sys.executable}\n"
        "import os\n"
        "import pathlib\n"
        "import sys\n"
        "args = sys.argv[1:]\n"
        "source_repo = pathlib.Path(os.environ['PREFLIGHT_TEST_SOURCE_REPO']).resolve()\n"
        "if args == ['config', '--local', '--get-all', 'remote.origin.url'] and pathlib.Path.cwd().resolve() == source_repo:\n"
        "    print(os.environ['PREFLIGHT_TEST_REMOTE_URL'])\n"
        "    raise SystemExit(0)\n"
        "if args and args[0] == 'fetch':\n"
        "    rewrite = f\"url.{os.environ['PREFLIGHT_TEST_REMOTE_PATH']}.insteadOf={os.environ['PREFLIGHT_TEST_REMOTE_URL']}\"\n"
        "    args = ['-c', rewrite, *args]\n"
        "real_git = os.environ['PREFLIGHT_TEST_REAL_GIT']\n"
        "os.execv(real_git, [real_git, *args])\n",
    )
    shim.chmod(0o755)
    environment["PATH"] = f"{bin_dir}{os.pathsep}{base_path}"
    environment["PREFLIGHT_TEST_BASE_PATH"] = base_path
    environment["PREFLIGHT_TEST_REAL_GIT"] = real_git
    environment["PREFLIGHT_TEST_SOURCE_REPO"] = str(repo.resolve())
    environment["PREFLIGHT_TEST_REMOTE_PATH"] = str(remote.resolve())
    environment["PREFLIGHT_TEST_REMOTE_URL"] = "https://github.com/seungpyoson/bolt-v2.git"
    for key in (
        "PATH",
        "PREFLIGHT_TEST_BASE_PATH",
        "PREFLIGHT_TEST_REAL_GIT",
        "PREFLIGHT_TEST_SOURCE_REPO",
        "PREFLIGHT_TEST_REMOTE_PATH",
        "PREFLIGHT_TEST_REMOTE_URL",
    ):
        os.environ[key] = environment[key]
    return environment


class GitFixture:
    def __init__(self, root: pathlib.Path) -> None:
        self.root = root
        self.remote = root / "origin.git"
        self.repo = root / "repo"
        init_fixture_repo(self.remote, "--bare")
        clone_fixture_repo(self.remote, self.repo)
        git(self.repo, "remote", "set-url", "origin", str(self.remote.resolve()))
        git(self.repo, "config", "user.email", "preflight@example.invalid")
        git(self.repo, "config", "user.name", "Merge Queue Preflight Test")
        write(self.repo / "shared.txt", "base\n")
        write(self.repo / ".mergify.yml", MERGIFY_YML)
        self.base = commit(self.repo, "base")
        git(self.repo, "branch", "-M", "main")
        git(self.repo, "push", "origin", "main")
        preflight_transport_environment(self.repo, self.remote)

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
        env=preflight_transport_environment(repo, origin),
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
    assert_equal(
        loaded.source_fence_full_profile_pathspecs,
        tuple(DEFAULT_SOURCE_FENCE_FULL_PROFILE_PATHSPECS),
        "source fence full-profile pathspec config",
    )
    assert_equal(
        loaded.source_fence_fences_only_rewrites,
        DEFAULT_SOURCE_FENCE_FENCES_ONLY_REWRITES,
        "source fence fences-only rewrite config",
    )


def assert_real_preflight_config_loads() -> None:
    module = load_preflight_module()
    loaded = module.load_config(REPO_ROOT / "ci" / "rust-verification.toml")
    assert_equal(
        loaded.repository,
        "github.com/seungpyoson/bolt-v2",
        "real merge queue repository",
    )
    assert_equal(
        loaded.source_fence_full_profile_pathspecs,
        tuple(DEFAULT_SOURCE_FENCE_FULL_PROFILE_PATHSPECS),
        "real source fence full-profile pathspec config",
    )
    assert_equal(
        loaded.source_fence_fences_only_rewrites,
        DEFAULT_SOURCE_FENCE_FENCES_ONLY_REWRITES,
        "real source fence fences-only rewrite config",
    )


def assert_fast_path_config_validation_fails_closed() -> None:
    module = load_preflight_module()
    rewrite_block = (
        "[merge_queue_preflight.source_fence_fences_only_rewrites]\n"
        '"just source-fence-static" = "just source-fence-static-fences-only"\n\n'
    )
    required_checks_block = (
        "[merge_queue_preflight.required_check_workflows]\n"
        '"gate" = "CI"\n'
        '"backtester-gate" = "Backtester CI"\n'
        '"actionlint" = "actionlint"\n'
        '"host-health" = "CI"\n\n'
    )
    cases = [
        (
            "empty required check workflow table",
            lambda text: text.replace(
                required_checks_block,
                "[merge_queue_preflight.required_check_workflows]\n\n",
            ),
            "config.merge_queue_preflight.required_check_workflows must be a non-empty table",
        ),
        (
            "empty source-fence full-profile pathspecs",
            lambda text: text.replace(
                'source_fence_full_profile_pathspecs = ["scripts", "justfile", '
                '"ci/rust-verification.toml", "ci/fail-closed-contracts.toml", '
                '"ci/fail-closed-exceptions.toml", '
                '"crates/backtesting-vertical-slice/ci/rust-verification.toml"]',
                "source_fence_full_profile_pathspecs = []",
            ),
            "config.merge_queue_preflight.source_fence_full_profile_pathspecs must be a non-empty string array",
        ),
        (
            "empty source-fence rewrite table",
            lambda text: text.replace(rewrite_block, "[merge_queue_preflight.source_fence_fences_only_rewrites]\n\n"),
            "config.merge_queue_preflight.source_fence_fences_only_rewrites must be a non-empty table",
        ),
        (
            "malformed source-fence rewrite target",
            lambda text: text.replace(
                '"just source-fence-static" = "just source-fence-static-fences-only"',
                '"just source-fence-static" = "\'"',
            ),
            "target contains an invalid shell command",
        ),
        (
            "source-fence rewrite target is not just",
            lambda text: text.replace(
                '"just source-fence-static" = "just source-fence-static-fences-only"',
                '"just source-fence-static" = "true"',
            ),
            "target must be exactly 'just <public-recipe>'",
        ),
        (
            "source-fence rewrite target is direct script",
            lambda text: text.replace(
                '"just source-fence-static" = "just source-fence-static-fences-only"',
                '"just source-fence-static" = "python3 scripts/run_fences.py --fences-only"',
            ),
            "target must be exactly 'just <public-recipe>'",
        ),
        (
            "source-fence rewrite target is ungated public recipe",
            lambda text: text.replace(
                '"just source-fence-static" = "just source-fence-static-fences-only"',
                '"just source-fence-static" = "just source-fence"',
            ),
            "target 'just source-fence' must route through a configured public local-gate label",
        ),
        (
            "source-fence rewrite target is private inner recipe",
            lambda text: text.replace(
                '"just source-fence-static" = "just source-fence-static-fences-only"',
                '"just source-fence-static" = "just source-fence-static-fences-only-inner"',
            ),
            "must route through a configured public local-gate label",
        ),
        (
            "source-fence rewrite source is private inner recipe",
            lambda text: text.replace(
                '"just source-fence-static" = "just source-fence-static-fences-only"',
                '"just source-fence-static-inner" = "just source-fence-static-fences-only"',
            ),
            "source 'just source-fence-static-inner' must route through a configured public local-gate label",
        ),
        (
            "verifier profile command is source-fence rewrite target",
            lambda text: text.replace(
                "commands = []",
                'commands = ["just source-fence-static-fences-only"]',
            ),
            "config.merge_queue_preflight.verifier_profiles.none.commands "
            "must not use reduced-profile rewrite target 'just source-fence-static-fences-only'",
        ),
        (
            "verifier profile command is path-qualified source-fence rewrite target",
            lambda text: text.replace(
                "commands = []",
                'commands = ["/usr/bin/just source-fence-static-fences-only"]',
            ),
            "config.merge_queue_preflight.verifier_profiles.none.commands "
            "must not use reduced-profile rewrite target '/usr/bin/just source-fence-static-fences-only'",
        ),
        (
            "verifier profile command is source-fence rewrite target with args",
            lambda text: text.replace(
                "commands = []",
                'commands = ["just source-fence-static-fences-only --extra"]',
            ),
            "config.merge_queue_preflight.verifier_profiles.none.commands "
            "must not use reduced-profile rewrite target 'just source-fence-static-fences-only --extra'",
        ),
        (
            "verifier profile command is source-fence rewrite target inner recipe",
            lambda text: text.replace(
                "commands = []",
                'commands = ["just source-fence-static-fences-only-inner"]',
            ),
            "must not use reduced-profile rewrite target 'just source-fence-static-fences-only-inner'",
        ),
        (
            "verifier profile command is direct source-fence reduced script",
            lambda text: text.replace(
                "commands = []",
                'commands = ["python3 scripts/run_fences.py --fences-only"]',
            ),
            "must not use reduced-profile rewrite target 'python3 scripts/run_fences.py --fences-only'",
        ),
        (
            "verifier profile command is direct source-fence reduced script abbreviated",
            lambda text: text.replace(
                "commands = []",
                'commands = ["python3 scripts/run_fences.py --fences"]',
            ),
            "must not use reduced-profile rewrite target 'python3 scripts/run_fences.py --fences'",
        ),
        (
            "verifier profile command is direct source-fence reduced script partial abbreviation",
            lambda text: text.replace(
                "commands = []",
                'commands = ["python3 scripts/run_fences.py --fences-o"]',
            ),
            "must not use reduced-profile rewrite target 'python3 scripts/run_fences.py --fences-o'",
        ),
        (
            "verifier profile command is env-wrapped source-fence rewrite target",
            lambda text: text.replace(
                "commands = []",
                'commands = ["env just source-fence-static-fences-only"]',
            ),
            "must not use reduced-profile rewrite target 'env just source-fence-static-fences-only'",
        ),
        (
            "verifier profile command is shell-wrapped source-fence rewrite target",
            lambda text: text.replace(
                "commands = []",
                'commands = ["bash -c \'just source-fence-static-fences-only\'"]',
            ),
            "must not use shell wrapper syntax",
        ),
        (
            "verifier profile command is shell-positional source-fence rewrite target",
            lambda text: text.replace(
                "commands = []",
                'commands = ["bash -c \'just \\"$@\\"\' _ source-fence-static-fences-only"]',
            ),
            "must not use shell wrapper syntax",
        ),
        (
            "verifier profile command is quoted source-fence rewrite target",
            lambda text, command="bash -c \"just \\'source-fence-static-fences-only\\'\"": text.replace(
                "commands = []",
                f"commands = [{json.dumps(command)}]",
            ),
            "must not use shell wrapper syntax",
        ),
        (
            "verifier profile command is shell-wrapped direct source-fence reduced script",
            lambda text: text.replace(
                "commands = []",
                'commands = ["sh -c \'python3 scripts/run_fences.py --fences-only\'"]',
            ),
            "must not use shell wrapper syntax",
        ),
        (
            "verifier profile command is deeply shell-wrapped source-fence rewrite target",
            lambda text, command='sh -c "sh -c \'just source-fence-static-fences-only\'"': text.replace(
                "commands = []",
                f"commands = [{json.dumps(command)}]",
            ),
            "must not use shell wrapper syntax",
        ),
        (
            "verifier profile command is source-fence rewrite target with just options",
            lambda text: text.replace(
                "commands = []",
                'commands = ["just -f /other/justfile source-fence-static-fences-only"]',
            ),
            "must not use reduced-profile rewrite target "
            "'just -f /other/justfile source-fence-static-fences-only'",
        ),
    ]
    for label, mutate, expected in cases:
        with tempfile.TemporaryDirectory() as tmp:
            config = write_preflight_config(pathlib.Path(tmp), "none", [])
            write(config, mutate(config.read_text(encoding="utf-8")))
            try:
                module.load_config(config)
            except module.PreflightError as exc:
                if expected not in str(exc):
                    raise AssertionError((label, str(exc), expected))
            else:
                raise AssertionError(f"{label} did not fail config load")
    with tempfile.TemporaryDirectory() as tmp:
        config = write_preflight_config(pathlib.Path(tmp), "static", ["just source-fence-static"])
        loaded = module.load_config(config)
    assert_equal(
        loaded.verifier_profiles["static"],
        ("just source-fence-static",),
        "rewrite source profile command remains valid",
    )


def assert_run_verifier_reduced_profile_commands_fail_closed() -> None:
    module = load_preflight_module()
    cases = [
        (
            "just source-fence-static-fences-only",
            "must not use reduced-profile rewrite target",
        ),
        (
            "just source-fence-static-fences-only-inner",
            "must not use reduced-profile rewrite target",
        ),
        (
            "python3 scripts/run_fences.py --fences-only",
            "must not use reduced-profile rewrite target",
        ),
        (
            "python3 scripts/run_fences.py --fences",
            "must not use reduced-profile rewrite target",
        ),
        (
            "python3 scripts/run_fences.py --fences-o",
            "must not use reduced-profile rewrite target",
        ),
        (
            "env just source-fence-static-fences-only",
            "must not use reduced-profile rewrite target",
        ),
        (
            "bash -c 'just source-fence-static-fences-only'",
            "must not use shell wrapper syntax",
        ),
        (
            "bash -c 'just \"$@\"' _ source-fence-static-fences-only",
            "must not use shell wrapper syntax",
        ),
        (
            "sh -c 'python3 scripts/run_fences.py --fences-only'",
            "must not use shell wrapper syntax",
        ),
    ]
    with tempfile.TemporaryDirectory() as tmp:
        config = write_preflight_config(pathlib.Path(tmp), "none", [])
        loaded = module.load_config(config)
    for command, expected in cases:
        try:
            module.verifier_commands(loaded, None, [command])
        except module.PreflightError as exc:
            if expected not in str(exc):
                raise AssertionError((command, str(exc), expected))
        else:
            raise AssertionError(f"--run-verifier accepted reduced-profile command {command!r}")
    assert_equal(
        module.verifier_commands(loaded, None, ["just source-fence-static"]),
        ("just source-fence-static",),
        "--run-verifier rewrite source remains valid",
    )
    assert_equal(
        module.verifier_commands(loaded, None, ["python3 scripts/verify_thing.py"]),
        ("python3 scripts/verify_thing.py",),
        "--run-verifier ordinary command remains valid",
    )


def assert_source_check_alias_targets_must_have_workflows() -> None:
    module = load_preflight_module()
    with tempfile.TemporaryDirectory() as tmp:
        config = write_preflight_config(pathlib.Path(tmp), "none", [])
        write(
            config,
            config.read_text(encoding="utf-8").replace(
                "[merge_queue_preflight.source_check_aliases]\n\n",
                '[merge_queue_preflight.source_check_aliases]\n"gate" = "missing-source-check"\n\n',
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


def assert_real_preflight_config_uses_live_gate_checks() -> None:
    module = load_preflight_module()
    config = module.load_config(REPO_ROOT / "ci" / "rust-verification.toml")
    assert_equal(
        config.required_check_workflows.get("gate"),
        "CI",
        "real preflight config source PR gate workflow",
    )
    assert_equal(
        config.required_check_workflows.get("backtester-gate"),
        "Backtester CI",
        "real preflight config source PR backtester gate workflow",
    )
    stale_checks = {
        "gate-iteration",
        "backtester-gate-iteration",
    } & set(config.required_check_workflows)
    assert_equal(stale_checks, set(), "real preflight config stale iteration checks")
    stale_aliases = {
        "gate",
        "backtester-gate",
    } & set(config.source_check_aliases)
    assert_equal(stale_aliases, set(), "real preflight config stale gate aliases")


def assert_source_check_evidence_fallback_is_precise() -> None:
    module = load_preflight_module()
    metadata = {"headRefOid": "1" * 40}
    check = passing_source_pr_check("gate")
    legacy_fallback = module.mergify_required_check_finding(
        merge_check="gate",
        source_check="gate",
        readiness={"metadata": metadata, "checks": [check]},
        expected_workflow="CI",
    )
    assert_equal(legacy_fallback, None, "missing source_checks falls back to checks")

    for label, source_checks in (("empty", []), ("none", None)):
        finding = module.mergify_required_check_finding(
            merge_check="gate",
            source_check="gate",
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
        assert_equal(evidence["check_name"], "gate", f"{label} source check name")
        assert_equal(evidence.get("merge_condition_check"), None, f"{label} direct check merge condition")


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
    origin: str = "origin",
    base: str = "main",
    source_fence_full_profile_pathspecs: list[str] | None = None,
    source_fence_fences_only_rewrites: dict[str, str] | None = None,
    verifier_stream_max_lines: int = 40,
    verifier_stream_max_bytes: int = 4000,
    input_timeout_seconds: int = 30,
    verifier_timeout_seconds: int = 60,
) -> pathlib.Path:
    rendered_commands = ", ".join(json.dumps(command) for command in commands)
    rendered_pathspecs = ", ".join(
        json.dumps(pathspec)
        for pathspec in (source_fence_full_profile_pathspecs or DEFAULT_SOURCE_FENCE_FULL_PROFILE_PATHSPECS)
    )
    rendered_rewrites = "\n".join(
        f"{json.dumps(source)} = {json.dumps(target)}"
        for source, target in (
            source_fence_fences_only_rewrites or DEFAULT_SOURCE_FENCE_FENCES_ONLY_REWRITES
        ).items()
    )
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
        "[local_lane_policy]\n"
        'cheap_lane_labels = ["local-gate:source-fence-static", "local-gate:source-fence-static-fences-only"]\n\n'
        "[merge_queue_preflight]\n"
        f"origin = {json.dumps(origin)}\n"
        f"base = {json.dumps(base)}\n"
        f"default_verifier_profile = {json.dumps(profile)}\n\n"
        f"source_fence_full_profile_pathspecs = [{rendered_pathspecs}]\n\n"
        "[merge_queue_preflight.operator]\n"
        'repository = "github.com/seungpyoson/bolt-v2"\n\n'
        "[merge_queue_preflight.timeouts]\n"
        f"input_seconds = {input_timeout_seconds}\n"
        f"verifier_seconds = {verifier_timeout_seconds}\n\n"
        "[merge_queue_preflight.required_check_workflows]\n"
        f"{rendered_workflows}\n\n"
        "[merge_queue_preflight.source_check_aliases]\n"
        f"{rendered_aliases}\n\n"
        "[merge_queue_preflight.source_fence_fences_only_rewrites]\n"
        f"{rendered_rewrites}\n\n"
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
        "import os\n"
        "import sys\n"
        f"views = {views!r}\n"
        f"checks = {checks_by_pr!r}\n"
        f"required_checks = {required_checks_by_pr!r}\n"
        f"failed_views = {(failed_views or {})!r}\n"
        f"check_exit_codes = {check_exit_codes_by_pr!r}\n"
        "if os.environ.get('GH_HOST') != 'github.com' or os.environ.get('GH_REPO') != 'github.com/seungpyoson/bolt-v2':\n"
        "    print('GitHub repository identity is not pinned', file=sys.stderr)\n"
        "    raise SystemExit(9)\n"
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


def passing_source_pr_check(name: str = "gate") -> dict[str, object]:
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
        "--expected-base-sha",
        expected_base_sha or git(repo, "rev-parse", "main"),
        *expected_head_sha_args(origin, prs),
        "--verifier-profile",
        "none",
        "--json",
        *prs,
    ]
    env = preflight_transport_environment(repo, origin)
    env["GH_HOST"] = "attacker.invalid"
    env["GH_REPO"] = "attacker.invalid/owner/repository"
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
        checkout_preflight_refs = fixture.repo / ".git" / "refs" / "preflight"
        checkout_preflight_refs.mkdir(parents=True, exist_ok=True)
        original_mode = checkout_preflight_refs.stat().st_mode
        checkout_preflight_refs.chmod(0o500)
        try:
            command = [
                sys.executable,
                str(SCRIPT_PATH),
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
        finally:
            checkout_preflight_refs.chmod(original_mode)
        payload = parse_json(result.stdout)
        assert_equal(result.returncode, 3, "remote-name private fetch no-gh rc")
        assert_equal(set(payload["pr_heads"].keys()), {"1"}, "private fetches must resolve checkout remote names")


def assert_base_fetch_uses_fully_qualified_branch_ref() -> None:
    module = load_preflight_module()

    class RecordingFetchRefs:
        def __init__(self) -> None:
            self.calls: list[tuple[str, str, str]] = []

        def fetch_sha(self, origin: str, source: str, name: str) -> str:
            self.calls.append((origin, source, name))
            return "a" * 40

    fetch_refs = RecordingFetchRefs()
    assert_equal(module.fetch_base(fetch_refs, "origin", "main"), "a" * 40, "base branch sha")
    assert_equal(
        fetch_refs.calls,
        [("origin", "refs/heads/main", "base-main")],
        "qualified base branch fetch",
    )


def assert_origin_identity_drift_is_terminal() -> None:
    with tempfile.TemporaryDirectory() as tmp:
        root = pathlib.Path(tmp)
        fixture = GitFixture(root)
        head = fixture.make_pr(1, {"one.txt": "one\n"})
        alternate = root / "alternate.git"
        init_fixture_repo(alternate, "--bare")
        os.environ["PREFLIGHT_TEST_REMOTE_URL"] = "https://github.com/attacker/repository.git"
        os.environ["PREFLIGHT_TEST_REMOTE_PATH"] = str(alternate)
        command = [
            sys.executable,
            str(SCRIPT_PATH),
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
        assert_equal(result.returncode, 3, "origin identity drift rc")
        payload = parse_json(result.stdout)
        findings = payload["findings"]
        assert any(
            finding["reason_code"] == "base_unavailable"
            and "does not match configured repository" in finding["evidence"]["reason"]
            for finding in findings
        ), findings


def assert_exact_origin_identity_blocks_chained_git_url_rewrites() -> None:
    with tempfile.TemporaryDirectory() as tmp:
        root = pathlib.Path(tmp)
        fixture = GitFixture(root)
        head = fixture.make_pr(1, {"one.txt": "one\n"})
        alternate = root / "alternate.git"
        init_fixture_repo(alternate, "--bare")
        origin_url = "https://github.com/seungpyoson/bolt-v2.git"
        alternate_url = alternate.resolve().as_uri()
        git(fixture.repo, "config", "--local", f"url.{alternate_url}.insteadOf", origin_url)
        git(fixture.repo, "remote", "set-url", "origin", origin_url)
        resolved = git(fixture.repo, "config", "--local", "--get-all", "remote.origin.url")
        assert_equal(resolved, origin_url, "literal configured origin")

        ambient = dict(os.environ)
        ambient["GIT_CONFIG_PARAMETERS"] = (
            f"'url.{alternate_url}.insteadOf'='{origin_url}'"
        )
        ambient["GIT_DIR"] = str(fixture.repo / ".git")
        ambient["GIT_EXEC_PATH"] = str(root / "alternate-exec-path")
        ambient["GIT_SSH_COMMAND"] = "alternate-ssh"
        env = git_remote_utils.isolated_git_transport_environment(ambient)
        command = [
            sys.executable,
            str(SCRIPT_PATH),
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
            env=env,
            text=True,
            stdout=subprocess.PIPE,
            stderr=subprocess.PIPE,
            check=False,
        )
        payload = parse_json(result.stdout)
        assert_equal(result.returncode, 3, "rewrite-protected origin rc")
        assert_equal(payload["actual_base_sha"], fixture.base, "rewrite-protected base")
        assert_equal(payload["pr_heads"], {"1": head}, "rewrite-protected PR head")


def assert_isolated_transport_environment_discards_ambient_git_config() -> None:
    environment = git_remote_utils.isolated_git_transport_environment(
        {
            "GIT_CONFIG_COUNT": "1",
            "GIT_CONFIG_KEY_0": "url.file:///alternate/.insteadOf",
            "GIT_CONFIG_VALUE_0": "https://github.com/example/",
            "GIT_CONFIG_PARAMETERS": "'url.file:///alternate/.insteadOf'='https://github.com/example/'",
            "GIT_DIR": "/tmp/alternate.git",
            "GIT_EXEC_PATH": "/tmp/alternate-exec-path",
            "GIT_SSH_COMMAND": "alternate-ssh",
        }
    )
    assert environment["GIT_CONFIG_COUNT"] == "1", environment
    assert environment["GIT_CONFIG_KEY_0"] == "credential.https://github.com.helper", environment
    assert environment["GIT_CONFIG_VALUE_0"] == "!gh auth git-credential", environment
    assert environment["GIT_CONFIG_NOSYSTEM"] == "1", environment
    assert environment["GIT_CONFIG_GLOBAL"] == os.devnull, environment
    allowed_git_keys = {
        "GIT_CONFIG_NOSYSTEM",
        "GIT_CONFIG_GLOBAL",
        "GIT_CONFIG_COUNT",
        "GIT_CONFIG_KEY_0",
        "GIT_CONFIG_VALUE_0",
    }
    assert {key for key in environment if key.startswith("GIT_")} == allowed_git_keys, environment


def assert_private_fetch_repo_ignores_ambient_git_template() -> None:
    module = load_preflight_module()
    with tempfile.TemporaryDirectory() as tmp:
        root = pathlib.Path(tmp)
        fixture = GitFixture(root)
        template = root / "hostile-template"
        alternate_url = (root / "alternate.git").resolve().as_uri()
        write(
            template / "config",
            "[url \"{alternate_url}\"]\n"
            "\tinsteadOf = https://github.com/seungpyoson/bolt-v2.git\n".format(
                alternate_url=alternate_url
            ),
        )
        previous_template = os.environ.get("GIT_TEMPLATE_DIR")
        os.environ["GIT_TEMPLATE_DIR"] = str(template)
        private_fetch = None
        try:
            private_fetch = module.PrivateFetchRefs.create(fixture.repo, 10)
            configured_rewrites = module.git(
                private_fetch.git_repo,
                "config",
                "--local",
                "--get-regexp",
                r"^url\.",
                check=False,
                timeout_seconds=10,
            )
        finally:
            if previous_template is None:
                os.environ.pop("GIT_TEMPLATE_DIR", None)
            else:
                os.environ["GIT_TEMPLATE_DIR"] = previous_template
            if private_fetch is not None:
                private_fetch.cleanup()
        assert_equal(configured_rewrites.returncode, 1, "private fetch URL rewrite status")
        assert_equal(configured_rewrites.stdout, "", "private fetch URL rewrites")


def assert_synthetic_commit_ignores_ambient_git_repository_override() -> None:
    module = load_preflight_module()
    with tempfile.TemporaryDirectory() as tmp:
        root = pathlib.Path(tmp)
        fixture = GitFixture(root)
        tree = git(fixture.repo, "rev-parse", "HEAD^{tree}")
        parent = git(fixture.repo, "rev-parse", "HEAD")
        previous_git_dir = os.environ.get("GIT_DIR")
        os.environ["GIT_DIR"] = str(root / "hostile.git")
        try:
            commit_sha = module.commit_tree(
                fixture.repo,
                tree,
                [parent],
                "isolated synthetic commit",
                10,
            )
        finally:
            if previous_git_dir is None:
                os.environ.pop("GIT_DIR", None)
            else:
                os.environ["GIT_DIR"] = previous_git_dir
        assert_equal(
            git(fixture.repo, "cat-file", "-t", commit_sha),
            "commit",
            "synthetic commit object type",
        )


def assert_private_fetch_uses_exact_checkout_remote_url_without_private_remote() -> None:
    module = load_preflight_module()
    with tempfile.TemporaryDirectory() as tmp:
        fixture = GitFixture(pathlib.Path(tmp))
        private_fetch = module.PrivateFetchRefs.create(fixture.repo, 10)
        try:
            resolved = private_fetch.fetch_origin("origin")
            assert_equal(
                resolved,
                "https://github.com/seungpyoson/bolt-v2.git",
                "private fetch origin URL",
            )
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
            fetched = private_fetch.fetch_sha("origin", requested, "probe")
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


def assert_origin_and_base_cli_overrides_are_rejected() -> None:
    module = load_preflight_module()
    sha = "0" * 40
    for option, value in (
        ("--origin", "origin"),
        ("--origin", "../origin.git"),
        ("--origin", "https://example.invalid/repo.git"),
        ("--origin", ""),
        ("--base", "main"),
        ("--base", ""),
    ):
        stderr = io.StringIO()
        try:
            with contextlib.redirect_stderr(stderr):
                module.parser().parse_args(
                    [
                        "1",
                        option,
                        value,
                        "--expected-base-sha",
                        sha,
                        "--expected-head-sha",
                        f"1={sha}",
                    ]
                )
        except SystemExit as exc:
            assert_equal(
                exc.code,
                module.PREFLIGHT_USAGE_EXIT_CODE,
                f"{option} override rejection rc",
            )
        else:
            raise AssertionError(f"preflight accepted forbidden CLI override {option}={value!r}")
        if "unrecognized arguments" not in stderr.getvalue():
            raise AssertionError(stderr.getvalue())


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


def assert_verifier_worktrees_do_not_write_checkout_git_metadata() -> None:
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
            "    print('isolated verifier rejected fail.txt')\n"
            "    sys.exit(7)\n",
        )
        config = write_preflight_config(root, "strict", [f"{sys.executable} {verifier}"])
        checkout_worktrees = fixture.repo / ".git" / "worktrees"
        checkout_worktrees.mkdir(parents=True, exist_ok=True)
        original_mode = checkout_worktrees.stat().st_mode
        checkout_worktrees.chmod(0o500)
        try:
            command = [
                sys.executable,
                str(SCRIPT_PATH),
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
        finally:
            checkout_worktrees.chmod(original_mode)
        assert_equal(result.returncode, 2, "checkout-worktrees-blocked verifier failure rc")
        payload = parse_json(result.stdout)
        blocked = payload["blocked_prs"]
        if len(blocked) != 1 or blocked[0]["pr"] != 1 or blocked[0]["type"] != "verifier_failed":
            raise AssertionError(blocked)
        if "isolated verifier rejected fail.txt" not in blocked[0]["stdout_preview"]:
            raise AssertionError(blocked)


def assert_verifier_worktrees_can_read_checkout_object_database() -> None:
    with tempfile.TemporaryDirectory() as tmp:
        root = pathlib.Path(tmp)
        fixture = GitFixture(root)
        head = fixture.make_pr(1, {"one.txt": "one\n"})
        local_only_commit = git(
            fixture.repo,
            "commit-tree",
            fixture.base + "^{tree}",
            "-m",
            "local-only evidence object",
        )
        verifier = root / "require_checkout_object.py"
        write(
            verifier,
            "import subprocess\n"
            "import sys\n"
            "subprocess.run(['git', 'cat-file', '-e', sys.argv[1] + '^{commit}'], check=True)\n",
        )
        config = write_preflight_config(root, "strict", [f"{sys.executable} {verifier} {local_only_commit}"])

        command = [
            sys.executable,
            str(SCRIPT_PATH),
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
        payload = parse_json(result.stdout)
        assert_equal(result.returncode, 3, "checkout-object verifier no-gh rc")
        if payload["blocked_prs"]:
            raise AssertionError(payload["blocked_prs"])
        assert_equal([batch["prs"] for batch in payload["batches"]], [[1]], "checkout-object verifier batch")


def assert_verifier_worktrees_inherit_origin_remote() -> None:
    with tempfile.TemporaryDirectory() as tmp:
        root = pathlib.Path(tmp)
        fixture = GitFixture(root)
        head = fixture.make_pr(1, {"one.txt": "one\n"})
        verifier = root / "require_origin_remote.py"
        write(
            verifier,
            "import subprocess\n"
            "import sys\n"
            "completed = subprocess.run(\n"
            "    ['git', 'remote', 'get-url', 'origin'],\n"
            "    text=True,\n"
            "    stdout=subprocess.PIPE,\n"
            "    stderr=subprocess.PIPE,\n"
            ")\n"
            "if completed.returncode != 0:\n"
            "    sys.stderr.write(completed.stderr)\n"
            "    sys.exit(7)\n"
            "actual = completed.stdout.strip()\n"
            "expected = sys.argv[1]\n"
            "if actual != expected:\n"
            "    print(f'expected origin {expected}, got {actual}', file=sys.stderr)\n"
            "    sys.exit(8)\n"
            "worktree_url = subprocess.run(\n"
            "    ['git', 'config', '--worktree', '--get', 'remote.origin.url'],\n"
            "    text=True,\n"
            "    stdout=subprocess.PIPE,\n"
            "    check=False,\n"
            ")\n"
            "if worktree_url.returncode != 0 or worktree_url.stdout.strip() != expected:\n"
            "    print('origin is not worktree-local', file=sys.stderr)\n"
            "    sys.exit(9)\n"
            "shared_url = subprocess.run(\n"
            "    ['git', 'config', '--local', '--get', 'remote.origin.url'],\n"
            "    text=True,\n"
            "    stdout=subprocess.PIPE,\n"
            "    check=False,\n"
            ")\n"
            "if shared_url.returncode == 0:\n"
            "    print('origin leaked into shared repository config', file=sys.stderr)\n"
            "    sys.exit(10)\n"
            "preserved = subprocess.run(\n"
            "    ['git', 'config', '--get', 'preflight.existing'],\n"
            "    text=True,\n"
            "    stdout=subprocess.PIPE,\n"
            "    check=False,\n"
            ")\n"
            "if preserved.returncode == 0:\n"
            "    print('ambient Git command config leaked into verifier', file=sys.stderr)\n"
            "    sys.exit(11)\n",
        )
        normalized_origin = "https://github.com/seungpyoson/bolt-v2.git"
        config = write_preflight_config(
            root,
            "strict",
            [f"{sys.executable} {verifier} {normalized_origin}"],
        )

        command = [
            sys.executable,
            str(SCRIPT_PATH),
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
        env = os.environ.copy()
        env.update(
            {
                "GIT_CONFIG_COUNT": "1",
                "GIT_CONFIG_KEY_0": "preflight.existing",
                "GIT_CONFIG_VALUE_0": "preserved",
            }
        )
        result = subprocess.run(
            command,
            cwd=fixture.repo,
            text=True,
            stdout=subprocess.PIPE,
            stderr=subprocess.PIPE,
            check=False,
            env=env,
        )
        payload = parse_json(result.stdout)
        if payload["blocked_prs"]:
            raise AssertionError(payload["blocked_prs"])
        assert_equal(result.returncode, 3, "origin-remote verifier no-gh rc")
        assert_equal([batch["prs"] for batch in payload["batches"]], [[1]], "origin-remote verifier batch")
        combined_output = f"{result.stdout}\n{result.stderr}"
        if normalized_origin in combined_output:
            raise AssertionError("normalized origin leaked through verifier command diagnostics")
        assert_equal(
            payload["batches"][0]["verifiers"][0]["command"],
            f"{sys.executable} {verifier} <remote-url>",
            "origin-remote public verifier command",
        )


def assert_verifier_diagnostics_redact_origin_url() -> None:
    with tempfile.TemporaryDirectory() as tmp:
        root = pathlib.Path(tmp)
        fixture = GitFixture(root)
        head = fixture.make_pr(1, {"one.txt": "one\n"})
        verifier = root / "print_origin_remote.py"
        write(
            verifier,
            "import subprocess\n"
            "import sys\n"
            "completed = subprocess.run(\n"
            "    ['git', 'remote', 'get-url', 'origin'],\n"
            "    text=True,\n"
            "    stdout=subprocess.PIPE,\n"
            "    check=True,\n"
            ")\n"
            "origin = completed.stdout.strip()\n"
            "print(f'origin={origin}')\n"
            "print(f'origin-error={origin}', file=sys.stderr)\n"
            "sys.exit(7)\n",
        )
        config = write_preflight_config(root, "strict", [f"{sys.executable} {verifier}"])
        command = [
            sys.executable,
            str(SCRIPT_PATH),
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
        assert_equal(result.returncode, 2, "origin-redaction verifier rc")
        normalized_origin = "https://github.com/seungpyoson/bolt-v2.git"
        combined_output = f"{result.stdout}\n{result.stderr}"
        if normalized_origin in combined_output:
            raise AssertionError("normalized origin leaked through verifier diagnostics")
        payload = parse_json(result.stdout)
        blocked = payload["blocked_prs"]
        if len(blocked) != 1 or blocked[0]["pr"] != 1:
            raise AssertionError(blocked)
        for stream in ("stdout_preview", "stderr_preview"):
            if "<remote-url>" not in blocked[0][stream]:
                raise AssertionError(f"origin placeholder absent from {stream}: {blocked[0][stream]!r}")


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
        assert mergify_queue_route_finding(1, "hotfix", ["hotfix"], mergify_queue_conditions("hotfix")) in payload["findings"], (
            payload["findings"]
        )
        assert mergify_queue_route_finding(2, "default", [], mergify_queue_conditions("default")) in payload["findings"], payload["findings"]
        assert mergify_queue_proof_source_finding(
            "hotfix",
            mergify_queue_conditions("hotfix"),
            mergify_required_merge_conditions(),
        ) in payload["findings"], payload["findings"]
        assert mergify_required_reviewer_finding(
            "hotfix",
            [mergify_required_reviewer()],
            mergify_required_merge_conditions(),
        ) in payload["findings"], payload["findings"]
        assert mergify_queue_proof_source_finding(
            "default",
            mergify_queue_conditions("default"),
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
        max_batch_size = mergify_queue_max_batch_size("default")
        assert_equal(
            [batch["prs"] for batch in payload["batches"]],
            [list(range(1, max_batch_size + 1)), list(range(max_batch_size + 1, 12))],
            "default queue above max size-valid batches",
        )
        assert mergify_queue_batch_above_max_finding("default", list(heads), max_batch_size) in payload["findings"], payload["findings"]


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
        assert mergify_queue_batch_below_min_finding(
            "default",
            [1],
            mergify_queue_min_batch_size("default"),
            mergify_queue_wait_time("default"),
        ) in payload["findings"], payload["findings"]


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
        config = write_preflight_config(root, "none", [], origin="missing-origin")
        command = [
            sys.executable,
            str(SCRIPT_PATH),
            "--config",
            str(config),
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


def assert_batch_verifier_scope_residual_covers_standalone_masking() -> None:
    with tempfile.TemporaryDirectory() as tmp:
        root = pathlib.Path(tmp)
        fixture = GitFixture(root)
        first_head = fixture.make_pr(1, {"bad.txt": "bad\n"})
        second_head = fixture.make_pr(2, {"mask.txt": "mask\n"})
        bin_dir = write_fake_gh(
            root,
            views={
                1: approved_pr_view(first_head),
                2: approved_pr_view(second_head),
            },
            checks=passing_source_pr_checks(1, 2),
        )
        verifier = root / "reject_unmasked_bad.py"
        write(
            verifier,
            "from pathlib import Path\n"
            "import sys\n"
            "if Path('bad.txt').exists() and not Path('mask.txt').exists():\n"
            "    print('bad.txt requires mask.txt')\n"
            "    sys.exit(7)\n",
        )
        command = [
            sys.executable,
            str(SCRIPT_PATH),
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
        assert_equal(result.returncode, 0, "batch-scoped verifier rc")
        payload = parse_json(result.stdout)
        assert_equal(payload["verdict"], "queue_as_one_wave", "batch-scoped verifier verdict")
        assert_equal(payload["batches"][0]["prs"], [1, 2], "batch-scoped verifier batch")
        assert_equal(payload["blocked_prs"], [], "batch-scoped verifier blocked_prs")
        assert_equal(payload["conflicts"], [], "batch-scoped verifier conflicts")
        assert "batch_verifier_scope" in payload["residual_risks"], payload["residual_risks"]
        residual_reason_codes = {
            finding["reason_code"]
            for finding in payload["findings"]
            if finding["lane"] == "residual_risk"
        }
        assert "batch_verifier_scope" in residual_reason_codes, payload["findings"]


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
        survivor_prs = [1, *range(3, 12)]
        max_batch_size = mergify_queue_max_batch_size("default")
        assert_equal(
            [batch["prs"] for batch in payload["batches"]],
            [survivor_prs[:max_batch_size], survivor_prs[max_batch_size:]],
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


def assert_fallback_retains_prefix_suffix_seam_conflict() -> None:
    with tempfile.TemporaryDirectory() as tmp:
        root = pathlib.Path(tmp)
        fixture = GitFixture(root)
        heads = {
            1: fixture.make_pr(1, {"shared.txt": "first\n", "one.txt": "one\n"}),
            2: fixture.make_pr(2, {"shared.txt": "second\n", "two.txt": "two\n"}),
            3: fixture.make_pr(3, {"three.txt": "three\n"}),
        }
        bin_dir = write_fake_gh(
            root,
            views={pr: approved_pr_view(head) for pr, head in heads.items()},
            checks=passing_source_pr_checks(*heads),
        )
        verifier = root / "reject_two_three.py"
        write(
            verifier,
            "from pathlib import Path\n"
            "import sys\n"
            "if Path('two.txt').exists() and Path('three.txt').exists():\n"
            "    print('two.txt and three.txt cannot batch together')\n"
            "    sys.exit(7)\n",
        )
        command = [
            sys.executable,
            str(SCRIPT_PATH),
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
        assert_equal(result.returncode, 1, "prefix suffix seam conflict rc")
        payload = parse_json(result.stdout)
        assert_equal(
            [batch["prs"] for batch in payload["batches"]],
            [[1], [2], [3]],
            "prefix suffix seam conflict batches",
        )
        conflicts = payload["conflicts"]
        expected = [
            {
                "pr": 2,
                "against_batch": [1],
                "files": ["shared.txt"],
                "type": "batch_conflict",
            },
            {
                "pr": 3,
                "against_batch": [2],
                "type": "batch_verifier_failed",
                "classification": "verifier_failed",
                "command": f"{sys.executable} {verifier}",
                "returncode": 7,
                "stdout_preview": "two.txt and three.txt cannot batch together",
                "stdout_truncated": False,
                "stderr_preview": "",
                "stderr_truncated": False,
            },
        ]
        assert_equal(conflicts, expected, "prefix suffix seam conflict artifacts")


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


def open_fifo_gate(path: pathlib.Path) -> int:
    os.mkfifo(path)
    return os.open(path, os.O_RDWR)


def release_fifo_gate(fd: int) -> None:
    os.write(fd, b"x")


def assert_preflight_source_fence_profile_selects_fences_only_by_full_profile_pathspecs() -> None:
    with tempfile.TemporaryDirectory() as tmp:
        root = pathlib.Path(tmp)
        fixture = GitFixture(root)
        log = root / "run-fences-argv.log"
        install_synthetic_run_fences(fixture, log)
        gate_log = root / "source-fence-gate.log"
        install_synthetic_source_fence_static(fixture, log, gate_log)
        config = write_preflight_config(root, "static", ["just source-fence-static"])

        nonscripts_head = fixture.make_pr(1, {"one.txt": "one\n"})
        result = run_preflight_with_config(fixture, config, {1: nonscripts_head})
        assert_equal(result.returncode, 3, "non-scripts source fence profile rc")
        first_run = log.read_text(encoding="utf-8").splitlines()[-1]
        if "--fences-only" not in first_run:
            raise AssertionError(first_run)
        first_gate_run = gate_log.read_text(encoding="utf-8").splitlines()[-1]
        if first_gate_run != "source-fence-static-fences-only -- just source-fence-static-fences-only-inner":
            raise AssertionError(first_gate_run)

        scripts_head = fixture.make_pr(2, {"scripts/changed.py": "VALUE = 1\n"})
        result = run_preflight_with_config(fixture, config, {2: scripts_head})
        assert_equal(result.returncode, 3, "scripts source fence profile rc")
        second_run = log.read_text(encoding="utf-8").splitlines()[-1]
        if "--fences-only" in second_run:
            raise AssertionError(second_run)

        justfile_head = fixture.make_pr(3, {"justfile": "source-fence-static:\n    python3 scripts/run_fences.py\n"})
        result = run_preflight_with_config(fixture, config, {3: justfile_head})
        assert_equal(result.returncode, 3, "justfile source fence profile rc")
        justfile_run = log.read_text(encoding="utf-8").splitlines()[-1]
        if "--fences-only" in justfile_run:
            raise AssertionError(justfile_run)

        root_config_head = fixture.make_pr(4, {"ci/rust-verification.toml": "[local_lane_policy]\n"})
        result = run_preflight_with_config(fixture, config, {4: root_config_head})
        assert_equal(result.returncode, 3, "root lane config source fence profile rc")
        root_config_run = log.read_text(encoding="utf-8").splitlines()[-1]
        if "--fences-only" in root_config_run:
            raise AssertionError(root_config_run)

        bte_config_head = fixture.make_pr(
            5,
            {"crates/backtesting-vertical-slice/ci/rust-verification.toml": "[local_lane_policy]\n"},
        )
        result = run_preflight_with_config(fixture, config, {5: bte_config_head})
        assert_equal(result.returncode, 3, "BTE lane config source fence profile rc")
        bte_config_run = log.read_text(encoding="utf-8").splitlines()[-1]
        if "--fences-only" in bte_config_run:
            raise AssertionError(bte_config_run)

        fail_closed_contracts_head = fixture.make_pr(
            6,
            {"ci/fail-closed-contracts.toml": "[fail_closed_contracts]\n"},
        )
        result = run_preflight_with_config(fixture, config, {6: fail_closed_contracts_head})
        assert_equal(result.returncode, 3, "fail-closed contracts source fence profile rc")
        fail_closed_contracts_run = log.read_text(encoding="utf-8").splitlines()[-1]
        if "--fences-only" in fail_closed_contracts_run:
            raise AssertionError(fail_closed_contracts_run)

        fail_closed_exceptions_head = fixture.make_pr(
            7,
            {"ci/fail-closed-exceptions.toml": "[fail_closed_exceptions]\n"},
        )
        result = run_preflight_with_config(fixture, config, {7: fail_closed_exceptions_head})
        assert_equal(result.returncode, 3, "fail-closed exceptions source fence profile rc")
        fail_closed_exceptions_run = log.read_text(encoding="utf-8").splitlines()[-1]
        if "--fences-only" in fail_closed_exceptions_run:
            raise AssertionError(fail_closed_exceptions_run)

        direct_config = write_preflight_config(root, "static", ["./scripts/run_fences.py"])
        direct_head = fixture.make_pr(8, {"direct.txt": "direct\n"})
        result = run_preflight_with_config(fixture, direct_config, {8: direct_head})
        assert_equal(result.returncode, 3, "direct source fence full profile rc")
        direct_run = log.read_text(encoding="utf-8").splitlines()[-1]
        if "--fences-only" in direct_run:
            raise AssertionError(direct_run)


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
        gate = root / "success-gate.fifo"
        gate_fd = open_fifo_gate(gate)
        write(
            verifier,
            "import os\n"
            f"gate = {str(gate)!r}\n"
            "fd = os.open(gate, os.O_RDONLY)\n"
            "try:\n"
            "    os.read(fd, 1)\n"
            "finally:\n"
            "    os.close(fd)\n",
        )
        command = [
            sys.executable,
            str(SCRIPT_PATH),
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
            release_fifo_gate(gate_fd)
            stdout, _stderr = process.communicate(timeout=20)
        finally:
            stop_process(process)
            os.close(gate_fd)
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
        gate = root / "failure-gate.fifo"
        gate_fd = open_fifo_gate(gate)
        write(
            verifier,
            "from pathlib import Path\n"
            "import os\n"
            "import sys\n"
            f"gate = {str(gate)!r}\n"
            "if Path('fail.txt').exists():\n"
            "    print('fail marker rejected')\n"
            "    sys.exit(7)\n"
            "if Path('slow.txt').exists():\n"
            "    fd = os.open(gate, os.O_RDONLY)\n"
            "    try:\n"
            "        os.read(fd, 1)\n"
            "    finally:\n"
            "        os.close(fd)\n",
        )
        command = [
            sys.executable,
            str(SCRIPT_PATH),
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
            release_fifo_gate(gate_fd)
            stdout, _stderr = process.communicate(timeout=30)
        finally:
            stop_process(process)
            os.close(gate_fd)
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


def assert_source_pr_gate_checks_are_queue_admitted() -> None:
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
        assert_equal(result.returncode, 0, "source PR gate checks rc")
        payload = parse_json(result.stdout)
        assert_equal(payload["wave_status"], "ready", "source PR gate checks wave")
        assert_equal(
            (payload["verdict"], payload["contract_exit_code"]),
            ("queue_as_one_wave", 0),
            "source PR gate checks contract",
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
                "backtester-gate",
                "gate",
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
        assert_equal(missing_gate_findings, [], "source PR must see merge proof gates")


def assert_source_gate_check_pending_is_inconclusive_at_runtime() -> None:
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
            if check["name"] == "gate"
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
        assert_equal(result.returncode, 3, "pending source gate check rc")
        payload = parse_json(result.stdout)
        pending_findings = [
            finding
            for finding in payload["findings"]
            if finding["reason_code"] == "required_check_pending"
        ]
        if len(pending_findings) != 1:
            raise AssertionError(payload["findings"])
        evidence = pending_findings[0]["evidence"]
        assert_equal(evidence["check_name"], "gate", "pending source check name")
        assert_equal(evidence.get("merge_condition_check"), None, "pending direct check merge condition")
        assert_equal(payload["lane_statuses"]["readiness"], "inconclusive", "pending source gate readiness lane")
        assert_equal(
            (payload["verdict"], payload["contract_exit_code"]),
            ("inconclusive", 3),
            "pending source gate check contract",
        )


def assert_source_gate_check_failure_blocks_at_runtime() -> None:
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
            if check["name"] == "gate"
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
        assert_equal(result.returncode, 2, "failed source gate check rc")
        payload = parse_json(result.stdout)
        failed_findings = [
            finding
            for finding in payload["findings"]
            if finding["reason_code"] == "required_check_failed"
        ]
        if len(failed_findings) != 1:
            raise AssertionError(payload["findings"])
        evidence = failed_findings[0]["evidence"]
        assert_equal(evidence["check_name"], "gate", "failed source check name")
        assert_equal(evidence.get("merge_condition_check"), None, "failed direct check merge condition")
        assert_equal(payload["lane_statuses"]["readiness"], "blocked", "failed source gate readiness lane")
        assert_equal(
            (payload["verdict"], payload["contract_exit_code"]),
            ("blocked", 2),
            "failed source gate check contract",
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
            checks={1: [passing_source_pr_check("gate")]},
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
                ("backtester-gate", None, 1, "default"),
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
        assert_equal(evidence["check_name"], "gate", "wrong check workflow name")
        assert_equal(evidence.get("merge_condition_check"), None, "wrong direct check merge condition")
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
        assert_equal(evidence["required_reviewers"], [mergify_required_reviewer()], "required reviewers")
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


def mergify_scalar_line(indent: str, key: str, value: object) -> str:
    return f"{indent}{key}: {yaml_scalar_literal(value)}\n"

def mergify_queue_batch_size_error(queue_name: str, batch_size: object) -> str:
    if isinstance(batch_size, dict):
        return f"{queue_name} batch_size must be min {batch_size['min']} max {batch_size['max']}"
    return f"{queue_name} batch_size must be {batch_size}"

def mergify_max_batch_size(batch_size: object) -> int:
    if isinstance(batch_size, dict):
        return batch_size["max"]
    if not isinstance(batch_size, int):
        raise AssertionError(f"unexpected Mergify batch_size shape: {batch_size!r}")
    return batch_size

def assert_mergify_config_gaps_are_reported() -> None:
    verifier = load_verifier()
    provenance = load_provenance()
    expectations = provenance.MERGIFY_CONFIG_EXPECTATIONS
    merge_queue_scalars = expectations["merge_queue"]
    queue_rules = expectations["queue_rules"]
    priority_rules = expectations["priority_rules"]
    required_reviewer = expectations["required_reviewer"]
    required_checks = expectations["required_checks"]
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
            "default missing gate merge condition",
            replace_once_after(
                mergify_config,
                "  - name: default\n",
                "      - check-success = gate\n",
                "",
            ),
            f"default merge_conditions must require {required_reviewer} and all {len(required_checks)} gates",
        ),
        (
            "hotfix missing gate merge condition",
            replace_once(mergify_config, "      - check-success = gate\n", ""),
            f"hotfix merge_conditions must require {required_reviewer} and all {len(required_checks)} gates",
        ),
        (
            "default extra merge condition",
            replace_once_after(
                mergify_config,
                "  - name: default\n",
                "      - check-success = host-health\n",
                "      - check-success = host-health\n      - label = queue-proof\n",
            ),
            f"default merge_conditions must require {required_reviewer} and all {len(required_checks)} gates",
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
            "default batch min lowered",
            replace_once(mergify_config, f"      min: {default_queue['batch_size']['min']}\n", "      min: 1\n"),
            mergify_queue_batch_size_error("default", default_queue["batch_size"]),
        ),
        (
            "default batch max narrowed",
            replace_once(
                mergify_config,
                f"      max: {default_queue['batch_size']['max']}\n",
                f"      max: {mergify_max_batch_size(default_queue['batch_size']) - 1}\n",
            ),
            mergify_queue_batch_size_error("default", default_queue["batch_size"]),
        ),
        (
            "default batch max duplicated",
            replace_once(
                mergify_config,
                f"      max: {default_queue['batch_size']['max']}\n",
                f"      max: {default_queue['batch_size']['max']}\n"
                f"      max: {mergify_max_batch_size(default_queue['batch_size']) - 1}\n",
            ),
            "duplicate key max",
        ),
        (
            "default batch unknown nested key",
            replace_once(
                mergify_config,
                f"      max: {default_queue['batch_size']['max']}\n",
                f"      max: {default_queue['batch_size']['max']}\n      spread: true\n",
            ),
            "default batch_size must not define unsupported key spread",
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
    assert_mergify_config_gaps_are_reported()
    assert_contract_result_reduces_findings_by_table()
    assert_preflight_input_timeout_is_config_driven()
    assert_real_preflight_config_loads()
    assert_fast_path_config_validation_fails_closed()
    assert_run_verifier_reduced_profile_commands_fail_closed()
    assert_source_check_alias_targets_must_have_workflows()
    assert_real_preflight_config_uses_live_gate_checks()
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
    assert_private_fetches_do_not_write_checkout_refs()
    assert_private_fetches_resolve_checkout_remote_names()
    assert_base_fetch_uses_fully_qualified_branch_ref()
    assert_origin_identity_drift_is_terminal()
    assert_exact_origin_identity_blocks_chained_git_url_rewrites()
    assert_isolated_transport_environment_discards_ambient_git_config()
    assert_private_fetch_repo_ignores_ambient_git_template()
    assert_synthetic_commit_ignores_ambient_git_repository_override()
    assert_private_fetch_uses_exact_checkout_remote_url_without_private_remote()
    assert_private_fetch_repo_persists_auto_maintenance_suppression()
    assert_private_fetch_sha_spawns_no_background_maintenance()
    assert_origin_and_base_cli_overrides_are_rejected()
    assert_private_fetches_do_not_freshen_checkout_objects()
    assert_github_actions_auth_helper_fails_without_actions_identity()
    assert_github_actions_auth_helper_keeps_local_ambient_auth_optional()
    assert_verifier_worktrees_do_not_write_checkout_git_metadata()
    assert_verifier_worktrees_can_read_checkout_object_database()
    assert_verifier_worktrees_inherit_origin_remote()
    assert_verifier_diagnostics_redact_origin_url()
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
    assert_batch_verifier_scope_residual_covers_standalone_masking()
    assert_fallback_recombines_survivors_after_batch_max_split()
    assert_fallback_replaces_suffix_optimistic_conflicts()
    assert_fallback_retains_prefix_suffix_seam_conflict()
    assert_preflight_source_fence_profile_selects_fences_only_by_full_profile_pathspecs()
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
    assert_source_pr_gate_checks_are_queue_admitted()
    assert_source_gate_check_pending_is_inconclusive_at_runtime()
    assert_source_gate_check_failure_blocks_at_runtime()
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
