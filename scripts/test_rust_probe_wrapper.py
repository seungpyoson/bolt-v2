#!/usr/bin/env python3
"""Self-tests for the managed Rust Probe wrapper and workflow contract."""

from __future__ import annotations

import contextlib
import importlib.util
import io
import pathlib
import subprocess
import sys
import tempfile
import types


REPO_ROOT = pathlib.Path(__file__).resolve().parents[1]
SCRIPT = REPO_ROOT / "scripts" / "rust_verification.py"
WORKFLOW = REPO_ROOT / ".github" / "workflows" / "rust-probe.yml"
CI_WORKFLOW = REPO_ROOT / ".github" / "workflows" / "ci.yml"
POLICY = REPO_ROOT / "ci" / "rust-verification.toml"

HEAD = "a" * 40
BRANCH = "codex/rust-probe-wrapper"


def load_owner_module() -> object:
    spec = importlib.util.spec_from_file_location("rust_verification_rust_probe_under_test", SCRIPT)
    if spec is None or spec.loader is None:
        raise AssertionError("unable to load rust_verification.py")
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


def valid_remote_probe() -> dict:
    return {
        "workflow_name": "Rust Probe",
        "workflow_path": ".github/workflows/rust-probe.yml",
        "poll_interval_seconds": 1,
        "appearance_timeout_seconds": 30,
        "overall_timeout_seconds": 300,
        "active_run_limit": 4,
        "workflow_runs_per_page": 20,
        "allowed_runner_tiers": ["heavy", "light"],
        "mode_runner_tiers": {
            "check-lib": "heavy",
            "check-test-target": "heavy",
            "nextest-no-run-test-target": "heavy",
            "nextest-test-target": "heavy",
            "nextest-test-target-name": "heavy",
        },
        "workflow_timeouts": {
            "probe-heavy": 60,
            "probe-light": 60,
        },
    }


def expect_policy_error(owner: object, remote_probe: dict, fragment: str) -> None:
    try:
        owner.validate_remote_probe_policy({"remote_probe": remote_probe})
    except owner.PolicyError as exc:
        if fragment not in str(exc):
            raise AssertionError(f"expected {fragment!r} in {exc!s}") from exc
        return
    raise AssertionError(f"expected PolicyError containing {fragment!r}")


def assert_remote_probe_policy_validation() -> None:
    owner = load_owner_module()
    owner.validate_remote_probe_policy({"remote_probe": valid_remote_probe()})
    loaded = owner.remote_probe_policy({"remote_probe": valid_remote_probe()})
    if loaded["mode_runner_tiers"]["check-lib"] != "heavy":
        raise AssertionError(loaded)
    if loaded["workflow_timeouts"]["probe-heavy"] != 60:
        raise AssertionError(loaded)

    heavy_only = valid_remote_probe()
    heavy_only["allowed_runner_tiers"] = ["heavy"]
    del heavy_only["workflow_timeouts"]["probe-light"]
    owner.validate_remote_probe_policy({"remote_probe": heavy_only})

    bad = valid_remote_probe()
    bad["mode_runner_tiers"]["check-lib"] = "turbo"
    expect_policy_error(owner, bad, "mode_runner_tiers.check-lib")

    bad = valid_remote_probe()
    del bad["workflow_timeouts"]["probe-light"]
    expect_policy_error(owner, bad, "workflow_timeouts")

    bad = valid_remote_probe()
    bad["appearance_timeout_seconds"] = 300
    bad["overall_timeout_seconds"] = 30
    expect_policy_error(owner, bad, "appearance_timeout_seconds")


def assert_repo_policy_declares_remote_probe() -> None:
    owner = load_owner_module()
    policy = owner.load_policy(REPO_ROOT)
    remote_probe = owner.remote_probe_policy(policy)
    if remote_probe["workflow_name"] != "Rust Probe":
        raise AssertionError(remote_probe)


def workflow_inputs(workflow_text: str) -> set[str]:
    inputs: set[str] = set()
    in_inputs = False
    for line in workflow_text.splitlines():
        if line == "    inputs:":
            in_inputs = True
            continue
        if in_inputs and line.startswith("      ") and not line.startswith("        ") and line.strip().endswith(":"):
            inputs.add(line.strip()[:-1])
            continue
        if in_inputs and line.startswith("  ") and not line.startswith("      "):
            break
    return inputs


def assert_workflow_contract() -> None:
    owner = load_owner_module()
    policy = owner.load_policy(REPO_ROOT)
    remote_probe = owner.remote_probe_policy(policy)
    text = WORKFLOW.read_text(encoding="utf-8")
    expected_inputs = {
        "runner_tier",
        "job_timeout_minutes",
        "ref",
        "expected_sha",
        "probe_id",
        "mode",
        "test_target",
        "test_name",
    }
    actual_inputs = workflow_inputs(text)
    if actual_inputs != expected_inputs:
        raise AssertionError((actual_inputs, expected_inputs))
    if "default: main" in text:
        raise AssertionError("rust-probe ref must not default to main")
    if "group: rust-probe" in text:
        raise AssertionError("rust-probe concurrency must not be a global constant")
    if "group: ${{ github.workflow }}-${{ github.ref }}" not in text:
        raise AssertionError("rust-probe concurrency must be branch-scoped")
    if "run-name:" not in text or "${{ inputs.probe_id }}" not in text:
        raise AssertionError("rust-probe run-name must include probe_id")
    if "\n    timeout-minutes: 60" in text:
        raise AssertionError("rust-probe timeout-minutes must come from [remote_probe.workflow_timeouts]")
    unsupported_marker = "  probe-unsupported-runner-tier:\n"
    unsupported_start = text.find(unsupported_marker)
    if unsupported_start < 0:
        raise AssertionError("rust-probe workflow must fail closed for unsupported runner_tier")
    unsupported_next_job = text.find("\n  probe-", unsupported_start + len(unsupported_marker))
    unsupported_block = text[unsupported_start:] if unsupported_next_job < 0 else text[unsupported_start:unsupported_next_job]
    tier_refusals = " && ".join(f"inputs.runner_tier != '{tier}'" for tier in remote_probe["allowed_runner_tiers"])
    if f"if: ${{{{ {tier_refusals} }}}}" not in unsupported_block:
        raise AssertionError("unsupported runner_tier guard must match [remote_probe].allowed_runner_tiers")
    if "exit 1" not in unsupported_block:
        raise AssertionError("unsupported runner_tier guard must fail the workflow")
    for job in remote_probe["workflow_timeouts"]:
        marker = f"  {job}:\n"
        start = text.find(marker)
        if start < 0:
            raise AssertionError(f"missing job {job}")
        next_job = text.find("\n  probe-", start + len(marker))
        block = text[start:] if next_job < 0 else text[start:next_job]
        if "timeout-minutes: ${{ fromJSON(inputs.job_timeout_minutes) }}" not in block:
            raise AssertionError(f"{job} timeout-minutes must be wrapper-provided from policy")
        if "fetch-depth: 1" not in block:
            raise AssertionError(f"{job} must use shallow checkout")
        if "RUST_PROBE_EXPECTED_SHA: ${{ inputs.expected_sha }}" not in block:
            raise AssertionError(f"{job} must pass expected SHA to runner")
        if "RUST_PROBE_ID: ${{ inputs.probe_id }}" not in block:
            raise AssertionError(f"{job} must pass probe id to runner")


def assert_rust_probe_not_merge_proof() -> None:
    workflow_text = WORKFLOW.read_text(encoding="utf-8")
    ci_text = CI_WORKFLOW.read_text(encoding="utf-8")
    if "pull_request:" in workflow_text or "\npush:" in workflow_text:
        raise AssertionError("Rust Probe must remain workflow_dispatch-only")
    if "rust-probe" in ci_text.lower():
        raise AssertionError("Rust Probe must not be added to full CI or gate needs")
    policy_text = POLICY.read_text(encoding="utf-8")
    full_ci_index = policy_text.find("[ci_provenance.full_ci]")
    if full_ci_index >= 0 and "rust-probe" in policy_text[full_ci_index:].lower():
        raise AssertionError("Rust Probe must not be a full-CI required job")


def assert_parser_exposes_rust_probe() -> None:
    owner = load_owner_module()
    args = owner.build_parser().parse_args(
        ["rust-probe", "--repo", "/tmp/repo", "nextest-test-target-name", "target_name", "test_name"]
    )
    if args.command_name != "rust-probe":
        raise AssertionError(args)
    if args.mode != "nextest-test-target-name" or args.test_target != "target_name" or args.test_name != "test_name":
        raise AssertionError(args)
    with contextlib.redirect_stderr(io.StringIO()):
        args = owner.build_parser().parse_args(["rust-probe", "--repo", "/tmp/repo", "--runner-tier", "policy-tier", "check-lib"])
    if args.runner_tier != "policy-tier":
        raise AssertionError(args)


def assert_preconditions_are_pr_free_and_exact_upstream() -> None:
    owner = load_owner_module()
    pushed_outputs = {
        ("status", "--porcelain", "--untracked-files=normal"): ("", None),
        ("rev-parse", "HEAD"): (HEAD, None),
        ("branch", "--show-current"): (BRANCH, None),
        ("config", f"branch.{BRANCH}.remote"): ("origin", None),
        ("config", f"branch.{BRANCH}.merge"): (f"refs/heads/{BRANCH}", None),
        ("ls-remote", "--heads", "origin", BRANCH): (f"{HEAD}\trefs/heads/{BRANCH}", None),
    }

    def run_with_git_outputs(
        outputs: dict[tuple[str, ...], tuple[str | None, str | None]],
    ) -> tuple[str | None, str | None, str | None, list[tuple[str, ...]]]:
        calls: list[tuple[str, ...]] = []

        def fake_git_output(_repo: pathlib.Path, *args: str) -> tuple[str | None, str | None]:
            calls.append(args)
            if args not in outputs:
                raise AssertionError(f"unexpected git call: {args}")
            return outputs[args]

        original_git_output = owner.git_output
        try:
            owner.git_output = fake_git_output
            head, branch, error = owner.ensure_rust_probe_preconditions(REPO_ROOT)
        finally:
            owner.git_output = original_git_output
        return head, branch, error, calls

    head, branch, error, calls = run_with_git_outputs(pushed_outputs)

    if (head, branch, error) != (HEAD, BRANCH, None):
        raise AssertionError((head, branch, error))
    if any(call and call[0] == "pr" for call in calls):
        raise AssertionError(calls)

    refusal_cases = [
        (
            "dirty worktree",
            {("status", "--porcelain", "--untracked-files=normal"): ("?? scratch.rs", None)},
            "rust-probe requires a clean worktree",
        ),
        (
            "missing upstream",
            {("config", f"branch.{BRANCH}.remote"): ("", None)},
            "rust-probe requires pushed HEAD with an upstream",
        ),
        (
            "unpushed head",
            {("ls-remote", "--heads", "origin", BRANCH): (f"{'b' * 40}\trefs/heads/{BRANCH}", None)},
            "rust-probe requires HEAD to be pushed to the upstream branch",
        ),
    ]
    for label, overrides, fragment in refusal_cases:
        outputs = dict(pushed_outputs)
        outputs.update(overrides)
        head, branch, error, _calls = run_with_git_outputs(outputs)
        if head is not None or branch is not None or error is None or fragment not in error:
            raise AssertionError((label, head, branch, error))


def assert_dispatch_uses_declared_workflow_inputs() -> None:
    owner = load_owner_module()
    policy = owner.remote_probe_policy({"remote_probe": valid_remote_probe()})
    calls: list[list[str]] = []

    def fake_run_capture(argv: list[str], *, repo: pathlib.Path) -> subprocess.CompletedProcess[str]:
        calls.append(argv)
        return subprocess.CompletedProcess(argv, 0, "", "")

    original_run_capture = owner.run_capture
    try:
        owner.run_capture = fake_run_capture
        error = owner.dispatch_rust_probe(
            REPO_ROOT,
            policy,
            branch=BRANCH,
            head=HEAD,
            mode="nextest-test-target-name",
            test_target="target_name",
            test_name="case_name",
            runner_tier="heavy",
            job_timeout_minutes=60,
            probe_id="probe-123",
        )
    finally:
        owner.run_capture = original_run_capture

    if error is not None:
        raise AssertionError(error)
    expected = [
        "gh",
        "workflow",
        "run",
        ".github/workflows/rust-probe.yml",
        "--ref",
        BRANCH,
        "-f",
        "runner_tier=heavy",
        "-f",
        "job_timeout_minutes=60",
        "-f",
        f"ref={HEAD}",
        "-f",
        f"expected_sha={HEAD}",
        "-f",
        "probe_id=probe-123",
        "-f",
        "mode=nextest-test-target-name",
        "-f",
        "test_target=target_name",
        "-f",
        "test_name=case_name",
    ]
    if calls != [expected]:
        raise AssertionError(calls)


def assert_cancelled_probe_is_superseded_not_code_failure() -> None:
    owner = load_owner_module()
    policy = owner.remote_probe_policy({"remote_probe": valid_remote_probe()})
    run = {
        "databaseId": 42,
        "status": "completed",
        "conclusion": "cancelled",
        "displayTitle": "Rust Probe probe-123 check-lib",
        "url": "https://example.invalid/runs/42",
    }
    stderr = io.StringIO()
    with contextlib.redirect_stderr(stderr):
        result = owner.evaluate_rust_probe_run(run, head=HEAD, probe_id="probe-123", remote_policy=policy)
    if result != 2:
        raise AssertionError((result, stderr.getvalue()))
    output = stderr.getvalue()
    if "superseded" not in output or "failed for" in output:
        raise AssertionError(output)


def assert_cmd_rust_probe_dispatches_and_reports_not_proof() -> None:
    owner = load_owner_module()
    calls: list[tuple[str, str, str, str, str, str, int, str]] = []

    original_load_policy = owner.load_policy
    original_preconditions = owner.ensure_rust_probe_preconditions
    original_active = owner.rust_probe_active_run_count
    original_dispatch = owner.dispatch_rust_probe
    original_wait = owner.wait_for_rust_probe_run
    original_probe_id = owner.new_probe_id
    try:
        owner.load_policy = lambda _repo: {"remote_probe": valid_remote_probe()}
        owner.ensure_rust_probe_preconditions = lambda _repo: (HEAD, BRANCH, None)
        owner.rust_probe_active_run_count = lambda _repo, _policy: (0, None)
        owner.new_probe_id = lambda: "probe-123"

        def fake_dispatch(
            _repo: pathlib.Path,
            _policy: dict,
            *,
            branch: str,
            head: str,
            mode: str,
            test_target: str,
            test_name: str,
            runner_tier: str,
            job_timeout_minutes: int,
            probe_id: str,
        ) -> str | None:
            calls.append((branch, head, mode, test_target, test_name, runner_tier, job_timeout_minutes, probe_id))
            return None

        owner.dispatch_rust_probe = fake_dispatch
        owner.wait_for_rust_probe_run = lambda **_kwargs: 0
        stdout = io.StringIO()
        stderr = io.StringIO()
        with contextlib.redirect_stdout(stdout), contextlib.redirect_stderr(stderr):
            result = owner.cmd_rust_probe(
                types.SimpleNamespace(
                    repo=str(REPO_ROOT),
                    mode="check-lib",
                    test_target=None,
                    test_name=None,
                    runner_tier=None,
                )
            )
    finally:
        owner.load_policy = original_load_policy
        owner.ensure_rust_probe_preconditions = original_preconditions
        owner.rust_probe_active_run_count = original_active
        owner.dispatch_rust_probe = original_dispatch
        owner.wait_for_rust_probe_run = original_wait
        owner.new_probe_id = original_probe_id

    if result != 0:
        raise AssertionError((result, stdout.getvalue(), stderr.getvalue()))
    if calls != [(BRANCH, HEAD, "check-lib", "", "", "heavy", 60, "probe-123")]:
        raise AssertionError(calls)
    if "NOT MERGE PROOF" not in stdout.getvalue():
        raise AssertionError(stdout.getvalue())


def main() -> int:
    assert_remote_probe_policy_validation()
    assert_repo_policy_declares_remote_probe()
    assert_workflow_contract()
    assert_rust_probe_not_merge_proof()
    assert_parser_exposes_rust_probe()
    assert_preconditions_are_pr_free_and_exact_upstream()
    assert_dispatch_uses_declared_workflow_inputs()
    assert_cancelled_probe_is_superseded_not_code_failure()
    assert_cmd_rust_probe_dispatches_and_reports_not_proof()
    print("OK: Rust Probe wrapper self-tests passed.")
    return 0


if __name__ == "__main__":
    import lane_governor

    lane_governor.acquire()
    raise SystemExit(main())
