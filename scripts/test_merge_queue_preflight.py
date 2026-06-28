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
) -> tuple[int, str, str]:
    command = [
        sys.executable,
        str(SCRIPT_PATH),
        "--origin",
        str(origin),
        "--base",
        "main",
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
        "metadata_unavailable": ("readiness", "pr", "inconclusive"),
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


def approved_pr_view(head: str, *, base: str = "main") -> dict[str, object]:
    return {
        "number": 1,
        "state": "OPEN",
        "isDraft": False,
        "mergeable": "MERGEABLE",
        "reviewDecision": "APPROVED",
        "headRefOid": head,
        "baseRefName": base,
        "title": "one",
        "url": "https://example.invalid/pull/1",
    }


def run_preflight_with_gh(
    repo: pathlib.Path,
    origin: pathlib.Path,
    bin_dir: pathlib.Path,
    *prs: str,
) -> subprocess.CompletedProcess[str]:
    command = [
        sys.executable,
        str(SCRIPT_PATH),
        "--origin",
        str(origin),
        "--base",
        "main",
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


def assert_clean_prs_batch_together() -> None:
    with tempfile.TemporaryDirectory() as tmp:
        fixture = GitFixture(pathlib.Path(tmp))
        fixture.make_pr(1, {"one.txt": "one\n"})
        fixture.make_pr(2, {"two.txt": "two\n"})

        _, stdout, _ = run_preflight(fixture.repo, fixture.remote, "1", "2")
        payload = parse_json(stdout)

        batches = payload["batches"]
        if batches != [{"index": 1, "prs": [1, 2], "status": "ready", "verifiers": []}]:
            raise AssertionError(batches)
        if payload["blocked_prs"] != []:
            raise AssertionError(payload["blocked_prs"])
        if payload["conflicts"] != []:
            raise AssertionError(payload["conflicts"])


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
        if rc != 1:
            raise AssertionError(rc)
        payload = parse_json(stdout)

        batches = payload["batches"]
        if [batch["prs"] for batch in batches] != [[1], [2]]:
            raise AssertionError(batches)
        conflicts = payload["conflicts"]
        expected = {
            "pr": 2,
            "against_batch": [1],
            "files": ["shared.txt"],
            "type": "batch_conflict",
        }
        if conflicts != [expected]:
            raise AssertionError(conflicts)


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
        if rc != 1:
            raise AssertionError(rc)
        payload = parse_json(stdout)

        batches = payload["batches"]
        if [batch["prs"] for batch in batches] != [[1, 2], [3]]:
            raise AssertionError(batches)
        conflicts = payload["conflicts"]
        if conflicts != [
            {
                "pr": 3,
                "against_batch": [1, 2],
                "files": ["shared.txt"],
                "type": "batch_conflict",
            }
        ]:
            raise AssertionError(conflicts)


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
        if rc != 1:
            raise AssertionError(rc)
        payload = parse_json(stdout)

        if [batch["prs"] for batch in payload["batches"]] != [[1]]:
            raise AssertionError(payload["batches"])
        blocked = payload["blocked_prs"]
        if blocked != [
            {
                "pr": 2,
                "reason": "conflicts with base",
                "files": ["shared.txt"],
                "type": "base_conflict",
            }
        ]:
            raise AssertionError(blocked)
        if payload["findings"] != [
            {
                "lane": "integration",
                "scope": "pr",
                "status": "blocked",
                "reason_code": "base_conflict",
                "message": "base_conflict",
                "evidence": blocked[0],
            }
        ]:
            raise AssertionError(payload["findings"])
        if (payload["verdict"], payload["contract_exit_code"]) != ("blocked", 2):
            raise AssertionError(payload)
        if payload["lane_statuses"]["integration"] != "blocked":
            raise AssertionError(payload["lane_statuses"])


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
        if result.returncode != 1:
            raise AssertionError(
                f"expected verifier failure exit 1, got {result.returncode}\n{result.stdout}\n{result.stderr}"
            )
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
        if result.returncode != 1:
            raise AssertionError(
                f"expected configured verifier failure, got {result.returncode}\n{result.stdout}\n{result.stderr}"
            )
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
        if result.returncode != 1:
            raise AssertionError(f"expected verifier rc=1, got {result.returncode}\n{result.stdout}\n{result.stderr}")
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
        if result.returncode != 0:
            raise AssertionError(f"expected verifier rc=0, got {result.returncode}\n{result.stdout}\n{result.stderr}")
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
        if result.returncode != 1:
            raise AssertionError(f"expected verifier rc=1, got {result.returncode}\n{result.stdout}\n{result.stderr}")
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
        if result.returncode != 1:
            raise AssertionError(f"expected verifier rc=1, got {result.returncode}\n{result.stdout}\n{result.stderr}")
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
        if result.returncode != 1:
            raise AssertionError(
                f"expected head mismatch rc=1, got {result.returncode}\n{result.stdout}\n{result.stderr}"
            )
        payload = parse_json(result.stdout)
        blocked = payload["blocked_prs"]
        if len(blocked) != 1 or blocked[0]["pr"] != 1 or blocked[0]["type"] != "readiness_failed":
            raise AssertionError(blocked)
        if "does not match fetched PR head" not in blocked[0]["reason"]:
            raise AssertionError(blocked)


def assert_wrong_base_ref_blocks_pr() -> None:
    with tempfile.TemporaryDirectory() as tmp:
        root = pathlib.Path(tmp)
        fixture = GitFixture(root)
        head = fixture.make_pr(1, {"one.txt": "one\n"})
        bin_dir = write_fake_gh(
            root,
            views={1: approved_pr_view(head, base="release")},
        )
        result = run_preflight_with_gh(fixture.repo, fixture.remote, bin_dir, "1")
        if result.returncode != 1:
            raise AssertionError(f"expected wrong base rc=1, got {result.returncode}\n{result.stdout}\n{result.stderr}")
        payload = parse_json(result.stdout)
        blocked = payload["blocked_prs"]
        if len(blocked) != 1 or blocked[0]["pr"] != 1 or blocked[0]["type"] != "readiness_failed":
            raise AssertionError(blocked)
        if "PR targets base 'release', expected 'main'" not in blocked[0]["reason"]:
            raise AssertionError(blocked)


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
        if result.returncode != 1:
            raise AssertionError(
                f"expected partial metadata failure rc=1, got {result.returncode}\n{result.stdout}\n{result.stderr}"
            )
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
        if rc != 2:
            raise AssertionError((rc, stdout, stderr))
        if "PR numbers must be positive integers" not in stderr:
            raise AssertionError(stderr)


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
        if result.returncode != 1:
            raise AssertionError(
                f"expected inconclusive metadata rc=1, got {result.returncode}\n{result.stdout}\n{result.stderr}"
            )
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
    assert_wrong_base_ref_blocks_pr()
    assert_partial_gh_metadata_failure_preserves_other_readiness()
    assert_invalid_pr_input_is_rejected()
    assert_missing_gh_reports_inconclusive_metadata()
    print("OK: merge_queue_preflight tests passed.")
    return 0


if __name__ == "__main__":
    import lane_governor

    lane_governor.acquire()
    raise SystemExit(main())
