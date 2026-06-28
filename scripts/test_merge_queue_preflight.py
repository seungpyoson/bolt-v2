#!/usr/bin/env python3
"""Tests for merge_queue_preflight.py."""

from __future__ import annotations

import importlib.util
import json
import os
import pathlib
import shutil
import subprocess
import sys
import tempfile


REPO_ROOT = pathlib.Path(__file__).resolve().parents[1]
SCRIPT_PATH = REPO_ROOT / "scripts" / "merge_queue_preflight.py"
MERGIFY_YML = (REPO_ROOT / ".mergify.yml").read_text(encoding="utf-8")


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
        "base_mismatch": ("identity", "pr", "inconclusive"),
        "head_mismatch": ("identity", "pr", "blocked"),
        "metadata_unavailable": ("readiness", "pr", "inconclusive"),
        "required_check_failed": ("readiness", "pr", "blocked"),
        "required_check_pending": ("readiness", "pr", "inconclusive"),
        "readiness_failed": ("readiness", "pr", "blocked"),
        "verifier_failed": ("verifier", "pr", "blocked"),
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
) -> pathlib.Path:
    rendered_commands = ", ".join(json.dumps(command) for command in commands)
    path = root / "preflight.toml"
    write(
        path,
        "[merge_queue_preflight]\n"
        'origin = "origin"\n'
        'base = "main"\n'
        f"default_verifier_profile = {json.dumps(profile)}\n\n"
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
    failed_views: dict[int, str] | None = None,
) -> pathlib.Path:
    bin_dir = root / "bin"
    bin_dir.mkdir()
    path = bin_dir / "gh"
    write(
        path,
        "#!/usr/bin/env python3\n"
        "import json\n"
        "import sys\n"
        f"views = {views!r}\n"
        f"checks = {(checks or {})!r}\n"
        f"failed_views = {(failed_views or {})!r}\n"
        "args = sys.argv[1:]\n"
        "if len(args) >= 3 and args[0:2] == ['pr', 'view']:\n"
        "    if int(args[2]) in failed_views:\n"
        "        print(failed_views[int(args[2])], file=sys.stderr)\n"
        "        raise SystemExit(1)\n"
        "    print(json.dumps(views[int(args[2])]))\n"
        "elif len(args) >= 3 and args[0:2] == ['pr', 'checks']:\n"
        "    print(json.dumps(checks.get(int(args[2]), [])))\n"
        "else:\n"
        "    raise SystemExit(f'unexpected gh args: {args}')\n",
    )
    path.chmod(0o755)
    return bin_dir


def approved_pr_view(head: str, *, base: str = "main", labels: tuple[str, ...] = ()) -> dict[str, object]:
    return {
        "number": 1,
        "state": "OPEN",
        "isDraft": False,
        "mergeable": "MERGEABLE",
        "reviewDecision": "APPROVED",
        "headRefOid": head,
        "baseRefName": base,
        "labels": [{"name": label} for label in labels],
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
        assert_equal(result.returncode, 3, "queue routing rc")
        payload = parse_json(result.stdout)
        assert mergify_queue_route_finding(1, "hotfix", ["hotfix"], ["label = hotfix"]) in payload["findings"], payload["findings"]
        assert mergify_queue_route_finding(2, "default", [], []) in payload["findings"], payload["findings"]
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
        assert_equal(result.returncode, 3, "default queue above max rc")
        payload = parse_json(result.stdout)
        assert_equal(payload["wave_status"], "split_advised", "default queue above max wave status")
        assert mergify_queue_batch_above_max_finding("default", list(heads), 10) in payload["findings"], payload["findings"]


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
        assert_equal(
            payload["findings"],
            [
                mergify_config_finding(
                    payload["base_sha"],
                    git(fixture.repo, "rev-parse", f"{payload['base_sha']}:.mergify.yml"),
                ),
                mergify_config_valid_finding(
                    payload["base_sha"],
                    git(fixture.repo, "rev-parse", f"{payload['base_sha']}:.mergify.yml"),
                ),
                no_gh_finding(),
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
                mergify_config_finding(
                    payload["base_sha"],
                    git(fixture.repo, "rev-parse", f"{payload['base_sha']}:.mergify.yml"),
                ),
                mergify_config_valid_finding(
                    payload["base_sha"],
                    git(fixture.repo, "rev-parse", f"{payload['base_sha']}:.mergify.yml"),
                ),
                no_gh_finding(),
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
        fixture.make_pr(1, {"safe.txt": "safe\n"})
        fixture.make_pr(2, {"fail.txt": "fail\n"})
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


def assert_configured_verifier_profile_blocks_bad_pr() -> None:
    with tempfile.TemporaryDirectory() as tmp:
        root = pathlib.Path(tmp)
        fixture = GitFixture(root)
        fixture.make_pr(1, {"safe.txt": "safe\n"})
        fixture.make_pr(2, {"fail.txt": "fail\n"})
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
        fixture.make_pr(1, {"fail.txt": "fail\n"})
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
        fixture.make_pr(1, {"safe.txt": "safe\n"})
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
        fixture.make_pr(1, {"fail.txt": "fail\n"})
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
        fixture.make_pr(1, {"fail.txt": "fail\n"})
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
        fixture.make_pr(1, {"one.txt": "one\n"})
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


def assert_invalid_pr_input_is_rejected() -> None:
    with tempfile.TemporaryDirectory() as tmp:
        root = pathlib.Path(tmp)
        fixture = GitFixture(root)
        rc, stdout, stderr = run_preflight(
            fixture.repo,
            fixture.remote,
            "abc",
            expect_success=False,
        )
        assert_equal(rc, 4, "invalid PR rc")
        assert "PR numbers must be positive integers" in stderr, stderr


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


def assert_missing_gh_reports_inconclusive_metadata() -> None:
    with tempfile.TemporaryDirectory() as tmp:
        root = pathlib.Path(tmp)
        fixture = GitFixture(root)
        fixture.make_pr(1, {"one.txt": "one\n"})
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
    assert_check_state_classification_is_table_driven()
    assert_input_failure_matrix_is_declarative()
    assert_mergify_config_field_handling_is_declarative()
    assert_preflight_artifact_classification_is_declarative()
    assert_preflight_artifact_finding_uses_classification_table()
    assert_contract_evaluator_reduces_normalized_evidence()
    assert_mergify_config_snapshot_uses_base_blob()
    assert_mergify_queue_routing_uses_pr_labels()
    assert_default_queue_above_max_is_split_advised()
    assert_clean_prs_batch_together()
    assert_conflicting_pr_starts_later_batch()
    assert_order_dependent_conflict_context_is_reported()
    assert_pr_that_conflicts_with_base_is_blocked()
    assert_verifier_failure_blocks_bad_pr_before_batching()
    assert_configured_verifier_profile_blocks_bad_pr()
    assert_plain_output_includes_verifier_failure_details()
    assert_plain_output_omits_successful_verifier_streams()
    assert_plain_output_bounds_failed_verifier_streams()
    assert_json_output_uses_bounded_verifier_previews()
    assert_head_oid_mismatch_blocks_pr()
    assert_wrong_base_ref_is_inconclusive()
    assert_required_check_pending_is_inconclusive()
    assert_partial_gh_metadata_failure_preserves_other_readiness()
    assert_invalid_pr_input_is_rejected()
    assert_missing_expected_base_sha_is_rejected()
    assert_missing_gh_reports_inconclusive_metadata()
    print("OK: merge_queue_preflight tests passed.")
    return 0


if __name__ == "__main__":
    import lane_governor

    lane_governor.acquire()
    raise SystemExit(main())
