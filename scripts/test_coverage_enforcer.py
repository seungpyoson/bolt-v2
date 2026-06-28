#!/usr/bin/env python3
"""Tests for coverage_enforcer.py."""

from __future__ import annotations

import contextlib
import dataclasses
import importlib.util
import io
import json
import os
import pathlib
import sys
import tempfile


REPO_ROOT = pathlib.Path(__file__).resolve().parents[1]
SCRIPT_PATH = REPO_ROOT / "scripts" / "coverage_enforcer.py"
CONFIG_PATH = REPO_ROOT / "ci" / "github-actions-runners.toml"
SHA = "a1a6be0d94e887538ebcd9afced6c94046a557d6"
REPO = "seungpyoson/bolt-v2"
APP_ID = 15368


CONFIG_TOML = (
    CONFIG_PATH.read_text(encoding="utf-8")
    .replace("poll_seconds = 30", "poll_seconds = 1")
    .replace("max_watch_seconds = 14400", "max_watch_seconds = 1")
)


CI_WORKFLOW = """
name: CI

on:
  push:
    branches: [main]
    tags: ["v*"]
  pull_request:
    branches: [main]
    types: [opened, synchronize, reopened, ready_for_review, converted_to_draft, edited]
  merge_group:
    types: [checks_requested]

jobs:
  ci-policy:
    outputs:
      gate_name: ${{ steps.policy.outputs.gate_name }}
      backtester_gate_name: ${{ steps.policy.outputs.backtester_gate_name }}
    steps:
      - run: |
          python3 "$policy_script" ci-policy --ref "${{ github.ref }}"

  host-health:
    name: host-health
    if: ${{ !startsWith(github.ref, 'refs/tags/v') }}
    steps:
      - run: python3 scripts/test_host_health.py

  gate:
    name: ${{ needs.ci-policy.outputs.gate_name }}
    steps:
      - if: ${{ needs.ci-policy.outputs.ci_policy_path == 'noop' || needs.ci-policy.outputs.full_ci_deferred == 'true' }}
        run: python3 "$verdict_script" resolve-gate-carry-forward
      - run: |
          if [[ "${{ needs.ci-policy.outputs.ci_policy_path }}" == "noop" ]]; then
            echo noop
          fi
          python3 "$verdict_script" check-ci-gate --full-ci-deferred "${{ needs.ci-policy.outputs.full_ci_deferred }}"
"""


BACKTESTER_WORKFLOW = """
name: Backtester CI

on:
  workflow_dispatch:
  pull_request:
    branches: [main]
    types: [opened, synchronize, reopened, ready_for_review, converted_to_draft, edited]
  push:
    branches: [main]
  merge_group:
    types: [checks_requested]

jobs:
  ci-policy:
    outputs:
      backtester_gate_name: ${{ steps.policy.outputs.backtester_gate_name }}
    steps:
      - run: |
          python3 "$policy_script" ci-policy --ref "${{ github.ref }}"

  clippy:
    if: ${{ needs.ci-policy.outputs.ci_policy_path == 'noop' || needs.ci-policy.outputs.full_ci_deferred == 'true' }}
    steps:
      - run: just bte-clippy

  gate:
    name: ${{ needs.ci-policy.outputs.backtester_gate_name }}
    steps:
      - run: |
          python3 "$verdict_script" check-backtester-gate \\
            --policy-path "${{ needs.ci-policy.outputs.ci_policy_path }}" \\
            --full-ci-deferred "${{ needs.ci-policy.outputs.full_ci_deferred }}"
"""


ACTIONLINT_WORKFLOW = """
name: actionlint

on:
  pull_request:
    branches: [main]
    types: [opened, synchronize, reopened, ready_for_review, edited]
  push:
    branches: [main]
  merge_group:
    types: [checks_requested]

jobs:
  actionlint:
    name: actionlint
    steps:
      - run: actionlint
"""


COVERAGE_WORKFLOW = """
name: Coverage Enforcer

on:
  pull_request:
    branches: [main]
    types: [opened, synchronize, reopened, ready_for_review, converted_to_draft, edited]
  merge_group:
    types: [checks_requested]

jobs:
  coverage-enforcer:
    name: coverage-enforcer
    steps:
      - run: python3 scripts/coverage_enforcer.py
"""


def load_script():
    if not SCRIPT_PATH.exists():
        raise AssertionError(f"missing script: {SCRIPT_PATH}")
    spec = importlib.util.spec_from_file_location("coverage_enforcer", SCRIPT_PATH)
    if spec is None or spec.loader is None:
        raise AssertionError("could not load coverage_enforcer.py")
    module = importlib.util.module_from_spec(spec)
    sys.modules[spec.name] = module
    spec.loader.exec_module(module)
    return module


def write_config(tmpdir: pathlib.Path, text: str = CONFIG_TOML) -> pathlib.Path:
    path = tmpdir / "github-actions-runners.toml"
    path.write_text(text, encoding="utf-8")
    return path


def write_workflows(tmpdir: pathlib.Path) -> pathlib.Path:
    workflow_dir = tmpdir / ".github" / "workflows"
    workflow_dir.mkdir(parents=True)
    (workflow_dir / "ci.yml").write_text(CI_WORKFLOW, encoding="utf-8")
    (workflow_dir / "backtester-ci.yml").write_text(BACKTESTER_WORKFLOW, encoding="utf-8")
    (workflow_dir / "actionlint.yml").write_text(ACTIONLINT_WORKFLOW, encoding="utf-8")
    (workflow_dir / "coverage-enforcer.yml").write_text(COVERAGE_WORKFLOW, encoding="utf-8")
    return workflow_dir


def check_run(
    name: str,
    *,
    run_id: int = 1001,
    check_suite_id: int = 2001,
    app_id: int = APP_ID,
    status: str = "completed",
    conclusion: str | None = "success",
    started_at: str = "2026-06-27T00:00:00Z",
    completed_at: str = "2026-06-27T00:00:00Z",
) -> dict[str, object]:
    return {
        "id": run_id,
        "name": name,
        "status": status,
        "conclusion": conclusion,
        "started_at": started_at,
        "completed_at": completed_at if status == "completed" else None,
        "check_suite": {"id": check_suite_id},
        "app": {"id": app_id},
    }


def pull_request_event(
    *,
    action: str = "opened",
    draft: bool = False,
    fork: bool = False,
    head_ref: str = "feature/ci-watchdog",
    sender_id: int = 12345,
    base_changed: bool = False,
) -> dict[str, object]:
    head_repo = "external/fork" if fork else REPO
    changes: dict[str, object] = {}
    if base_changed:
        changes = {"base": {"ref": {"from": "old-main"}}}
    return {
        "action": action,
        "ref": "refs/pull/123/merge",
        "sender": {"id": sender_id},
        "changes": changes,
        "pull_request": {
            "draft": draft,
            "head": {"sha": SHA, "ref": head_ref, "repo": {"full_name": head_repo}},
            "base": {"repo": {"full_name": REPO}},
        },
    }


def mergify_temp_pr_event() -> dict[str, object]:
    return pull_request_event(
        action="synchronize",
        draft=True,
        head_ref="mergify/merge-queue/1234567890",
        sender_id=37929162,
    )


def merge_group_event() -> dict[str, object]:
    return {
        "action": "checks_requested",
        "ref": "refs/heads/gh-readonly-queue/main/pr-123-abcdef",
        "sender": {"id": 12345},
        "merge_group": {
            "head_sha": SHA,
            "base_sha": "b1b6be0d94e887538ebcd9afced6c94046a557d6",
        },
    }


class FakeGitHub:
    def __init__(self, check_pages: list[list[dict[str, object]]]) -> None:
        self.check_pages = list(check_pages)
        self.requests: list[tuple[str, str, object]] = []

    def json(
        self,
        repo: str,
        token: str,
        path: str,
        query: dict[str, str] | None = None,
        *,
        method: str = "GET",
        data: object = None,
    ) -> dict[str, object]:
        self.requests.append((method, path, data))
        if path == f"commits/{SHA}/check-runs":
            if not self.check_pages:
                raise AssertionError("check-runs polled more times than expected")
            return {"check_runs": self.check_pages.pop(0)}
        if path == "check-runs" and method == "POST":
            raise AssertionError("coverage-enforcer must not publish custom check-runs")
        raise AssertionError(f"unexpected request: {(repo, token, path, query, method, data)!r}")

    def posted_check_runs(self) -> list[dict[str, object]]:
        return [
            data
            for method, path, data in self.requests
            if method == "POST" and path == "check-runs"
        ]


class FakeClock:
    def __init__(self, values: list[float]) -> None:
        self.values = list(values)

    def monotonic(self) -> float:
        if self.values:
            return self.values.pop(0)
        return 999.0

    def sleep(self, _seconds: float) -> None:
        return None


def run_enforcer(
    check_pages: list[list[dict[str, object]]],
    *,
    config_text: str = CONFIG_TOML,
    event: dict[str, object] | None = None,
    clock: FakeClock | None = None,
) -> tuple[object, FakeGitHub]:
    module = load_script()
    fake = FakeGitHub(check_pages)
    clock = clock or FakeClock([0.0, 0.0])
    with tempfile.TemporaryDirectory() as tmp:
        tmpdir = pathlib.Path(tmp)
        config = write_config(tmpdir, config_text)
        workflow_dir = write_workflows(tmpdir)
        result = module.enforce_coverage(
            repo=REPO,
            token="token",
            event=event or pull_request_event(),
            config_path=config,
            workflow_dir=workflow_dir,
            api_json=fake.json,
            monotonic=clock.monotonic,
            sleep=clock.sleep,
        )
    return result, fake


def assert_drift_detects_missing_and_wrong_app() -> None:
    result, fake = run_enforcer(
        [
            [
                check_run("gate"),
                check_run("backtester-gate", app_id=999),
                check_run("actionlint"),
            ]
        ],
        event=merge_group_event(),
        clock=FakeClock([0.0, 2.0]),
    )
    if result.conclusion != "failure":
        raise AssertionError(result)
    if "backtester-gate" not in result.summary or "wrong GitHub App" not in result.summary:
        raise AssertionError(result.summary)
    if "host-health" not in result.summary or "timed out" not in result.summary:
        raise AssertionError(result.summary)
    if fake.posted_check_runs():
        raise AssertionError(fake.requests)


def assert_all_present_and_correct_succeeds() -> None:
    contexts = ("gate", "backtester-gate", "host-health", "actionlint")
    result, fake = run_enforcer(
        [[check_run(context, run_id=index + 1) for index, context in enumerate(contexts)]],
        event=merge_group_event(),
    )
    if result.conclusion != "success" or result.findings:
        raise AssertionError(result)
    if fake.posted_check_runs():
        raise AssertionError(fake.requests)


def assert_iteration_pr_does_not_wait_for_boundary_gates() -> None:
    result, fake = run_enforcer(
        [[check_run("host-health"), check_run("actionlint", run_id=2)]],
        event=pull_request_event(),
    )
    if result.conclusion != "success" or result.findings:
        raise AssertionError(result)
    if "gate" in getattr(result, "expected_contexts", ()):
        raise AssertionError(result)
    if fake.posted_check_runs():
        raise AssertionError(fake.requests)


def assert_merge_boundary_waits_for_required_gates() -> None:
    result, _fake = run_enforcer(
        [[check_run("host-health"), check_run("actionlint", run_id=2)]],
        event=mergify_temp_pr_event(),
        clock=FakeClock([0.0, 2.0]),
    )
    if result.conclusion != "failure":
        raise AssertionError(result)
    for context in ("gate", "backtester-gate"):
        if context not in result.summary:
            raise AssertionError(result.summary)


def assert_newer_partial_gate_pair_fails_closed_over_stale_complete_pair() -> None:
    stale_completed_at = "2026-06-27T00:00:10Z"
    fresh_completed_at = "2026-06-27T00:01:10Z"
    result, fake = run_enforcer(
        [
            [
                check_run("gate", completed_at=stale_completed_at),
                check_run("backtester-gate", completed_at=stale_completed_at),
                check_run("host-health"),
                check_run("actionlint"),
                check_run(
                    "gate-iteration",
                    started_at="2026-06-27T00:01:00Z",
                    completed_at=fresh_completed_at,
                ),
            ]
        ],
        event=merge_group_event(),
        clock=FakeClock([0.0, 2.0]),
    )
    if result.conclusion != "failure":
        raise AssertionError(result)
    if "backtester-gate-iteration" not in result.summary or "timed out" not in result.summary:
        raise AssertionError(result.summary)
    if fake.posted_check_runs():
        raise AssertionError(fake.requests)


def assert_poll_timeout_fails_closed() -> None:
    result, fake = run_enforcer(
        [[check_run("gate", status="in_progress", conclusion=None)]],
        event=merge_group_event(),
        clock=FakeClock([0.0, 2.0]),
    )
    if result.conclusion != "failure":
        raise AssertionError(result)
    if "timed out waiting for terminal check-runs" not in result.summary:
        raise AssertionError(result.summary)
    if fake.posted_check_runs():
        raise AssertionError(fake.requests)


def assert_same_app_reruns_do_not_count_as_duplicate_drift() -> None:
    result, _fake = run_enforcer(
        [
            [
                check_run("host-health", run_id=1, check_suite_id=101),
                check_run("host-health", run_id=2, check_suite_id=102),
                check_run("actionlint", run_id=3, check_suite_id=103),
            ]
        ],
        event=pull_request_event(),
    )
    if result.conclusion != "success" or result.findings:
        raise AssertionError(result)


def assert_wrong_app_rerun_remains_drift_even_with_expected_app_success() -> None:
    result, _fake = run_enforcer(
        [
            [
                check_run("host-health", run_id=1, app_id=999),
                check_run("host-health", run_id=2),
                check_run("actionlint", run_id=3),
            ]
        ],
        event=pull_request_event(),
    )
    if result.conclusion != "failure":
        raise AssertionError(result)
    if "host-health" not in result.summary or "wrong GitHub App" not in result.summary:
        raise AssertionError(result.summary)


def assert_expected_app_failure_without_success_is_drift() -> None:
    result, _fake = run_enforcer(
        [
            [
                check_run("host-health", conclusion="failure"),
                check_run("actionlint", run_id=2),
            ]
        ],
        event=pull_request_event(),
    )
    if result.conclusion != "failure":
        raise AssertionError(result)
    if "host-health" not in result.summary or "successful" not in result.summary:
        raise AssertionError(result.summary)


def assert_latest_expected_app_failure_after_success_is_drift() -> None:
    result, _fake = run_enforcer(
        [
            [
                check_run("host-health", run_id=1, conclusion="success"),
                check_run("host-health", run_id=2, conclusion="failure"),
                check_run("actionlint", run_id=3),
            ]
        ],
        event=pull_request_event(),
    )
    if result.conclusion != "failure":
        raise AssertionError(result)
    if "host-health" not in result.summary or "latest" not in result.summary:
        raise AssertionError(result.summary)


def assert_latest_expected_app_success_after_failure_succeeds() -> None:
    result, _fake = run_enforcer(
        [
            [
                check_run("host-health", run_id=1, conclusion="failure"),
                check_run("host-health", run_id=2, conclusion="success"),
                check_run("actionlint", run_id=3),
            ]
        ],
        event=pull_request_event(),
    )
    if result.conclusion != "success" or result.findings:
        raise AssertionError(result)


def assert_newer_in_progress_expected_app_keeps_context_pending() -> None:
    result, fake = run_enforcer(
        [
            [
                check_run("host-health", run_id=1, conclusion="failure"),
                check_run("host-health", run_id=2, status="in_progress", conclusion=None),
                check_run("actionlint", run_id=3),
            ],
            [
                check_run("host-health", run_id=1, conclusion="failure"),
                check_run("host-health", run_id=2, conclusion="success"),
                check_run("actionlint", run_id=3),
            ],
        ],
        event=pull_request_event(),
        clock=FakeClock([0.0, 0.0, 0.0]),
    )
    if result.conclusion != "success" or result.findings:
        raise AssertionError(result)
    get_requests = [
        request for request in fake.requests if request[0] == "GET" and request[1].endswith("/check-runs")
    ]
    if len(get_requests) != 2:
        raise AssertionError(fake.requests)


def assert_r2_derivation_mismatch_fails() -> None:
    bad_config = CONFIG_TOML.replace(
        "[ci_provenance.required_checks.actionlint]\n"
        "context = \"actionlint\"\n"
        "reporter = \"actionlint.yml\"\n"
        "integration_id = 15368\n"
        "required = true\n"
        "target = true\n"
        "runs_on_tags = false",
        "[ci_provenance.required_checks.actionlint]\n"
        "context = \"actionlint\"\n"
        "reporter = \"actionlint.yml\"\n"
        "integration_id = 15368\n"
        "required = true\n"
        "target = true\n"
        "runs_on_tags = true",
    ).replace(
        "[ci_provenance.required_checks.actionlint.proof_rule]\n"
        "fresh = [\"full\", \"docs\", \"iteration\"]\n"
        "carry_forward = []",
        "[ci_provenance.required_checks.actionlint.proof_rule]\n"
        "fresh = [\"full\", \"docs\", \"iteration\", \"tag_reuse\"]\n"
        "carry_forward = []",
    )
    result, fake = run_enforcer(
        [[check_run(context) for context in ("gate", "backtester-gate", "host-health", "actionlint")]],
        config_text=bad_config,
        event=merge_group_event(),
    )
    if result.conclusion != "failure":
        raise AssertionError(result)
    if "registry/YAML derivation mismatch for actionlint" not in result.summary:
        raise AssertionError(result.summary)
    if fake.posted_check_runs():
        raise AssertionError(fake.requests)


def assert_r2_derives_generic_tag_triggers() -> None:
    module = load_script()
    with tempfile.TemporaryDirectory() as tmp:
        tmpdir = pathlib.Path(tmp)
        config = write_config(tmpdir)
        workflow_dir = write_workflows(tmpdir)
        actionlint_path = workflow_dir / "actionlint.yml"
        actionlint_path.write_text(
            ACTIONLINT_WORKFLOW.replace(
                "  push:\n    branches: [main]\n",
                "  push:\n    branches: [main]\n    tags: ['v*']\n",
            ),
            encoding="utf-8",
        )
        checks = module.load_registry_checks(config)
        findings = module.registry_workflow_derivation_findings(
            checks=checks,
            workflow_dir=workflow_dir,
        )
    if not any("actionlint" in finding and "runs_on_tags" in finding for finding in findings):
        raise AssertionError(findings)


def assert_fork_pr_uses_native_job_without_publishing() -> None:
    result, fake = run_enforcer(
        [[check_run("host-health"), check_run("actionlint", run_id=2)]],
        event=pull_request_event(fork=True),
    )
    if result.conclusion != "success":
        raise AssertionError(result)
    if fake.posted_check_runs():
        raise AssertionError(fake.requests)


def assert_self_reporter_is_excluded_even_when_required() -> None:
    module = load_script()
    with tempfile.TemporaryDirectory() as tmp:
        config_path = write_config(pathlib.Path(tmp))
        config = module.ci_provenance.load_config(config_path)
        coverage_check = config.required_checks["coverage-enforcer"]
        mutated_checks = dict(config.required_checks)
        mutated_checks["coverage-enforcer"] = dataclasses.replace(
            coverage_check,
            required=True,
        )
        mutated_config = dataclasses.replace(
            config,
            required_checks=mutated_checks,
        )
        policy_result = module.policy_result_for_event(
            config=mutated_config,
            event=merge_group_event(),
        )
        contexts = tuple(
            check.context
            for check in module.expected_registry_checks_for_policy(
                config=mutated_config,
                policy_result=policy_result,
            )
        )
    if "coverage-enforcer" in contexts:
        raise AssertionError(contexts)


def assert_docs_only_false_is_not_weaker_for_watchdog_events() -> None:
    module = load_script()
    with tempfile.TemporaryDirectory() as tmp:
        config = write_config(pathlib.Path(tmp))
        events = (pull_request_event(), mergify_temp_pr_event(), merge_group_event())
        for event in events:
            docs_contexts = tuple(
                check.context
                for check in module.expected_registry_checks(
                    config_path=config,
                    event=event,
                    docs_only=True,
                )
            )
            normal_contexts = tuple(
                check.context
                for check in module.expected_registry_checks(
                    config_path=config,
                    event=event,
                    docs_only=False,
                )
            )
            if normal_contexts != docs_contexts:
                raise AssertionError((event, normal_contexts, docs_contexts))


def assert_target_contexts_keep_docs_iteration_equivalent() -> None:
    module = load_script()
    config = module.ci_provenance.load_config(CONFIG_PATH)
    asymmetric = []
    for check in config.required_checks.values():
        if not check.target or check.reporter == "self":
            continue
        fresh = set(check.fresh_event_classes)
        if ("docs" in fresh) != ("iteration" in fresh):
            asymmetric.append(check.context)
    if asymmetric:
        raise AssertionError(asymmetric)


def assert_real_registry_derivation_matches_current_workflows() -> None:
    module = load_script()
    checks = module.load_registry_checks(CONFIG_PATH)
    derived = module.derive_registry_workflow_flags(checks, REPO_ROOT / ".github" / "workflows")
    expected = {
        "gate": (True, True),
        "backtester-gate": (True, True),
        "host-health": (False, False),
        "actionlint": (False, False),
        "coverage-enforcer": (False, False),
    }
    actual = {
        context: (flags.runs_on_tags, flags.supports_carry_forward)
        for context, flags in derived.items()
    }
    if actual != expected:
        raise AssertionError(actual)


def run_cli_for_fork_failure() -> tuple[int, str, str, str]:
    module = load_script()
    event_text = json.dumps(pull_request_event(fork=True))
    with tempfile.TemporaryDirectory() as tmp:
        tmpdir = pathlib.Path(tmp)
        event_path = tmpdir / "event.json"
        summary_path = tmpdir / "summary.md"
        event_path.write_text(event_text, encoding="utf-8")
        config = write_config(tmpdir)
        workflow_dir = write_workflows(tmpdir)

        fake = FakeGitHub(
            [[check_run("host-health"), check_run("actionlint", conclusion="failure")]]
        )
        env_keys = (
            "GITHUB_REPOSITORY",
            "GITHUB_TOKEN",
            "GITHUB_EVENT_PATH",
            "GITHUB_STEP_SUMMARY",
        )
        old_env = {key: os.environ.get(key) for key in env_keys}
        os.environ.update(
            {
                "GITHUB_REPOSITORY": REPO,
                "GITHUB_TOKEN": "token",
                "GITHUB_EVENT_PATH": str(event_path),
                "GITHUB_STEP_SUMMARY": str(summary_path),
            }
        )
        stdout = io.StringIO()
        stderr = io.StringIO()
        try:
            with contextlib.redirect_stdout(stdout), contextlib.redirect_stderr(stderr):
                code = module.main(
                    [
                        "--config",
                        str(config),
                        "--workflow-dir",
                        str(workflow_dir),
                    ],
                    api_json=fake.json,
                )
        finally:
            for key, value in old_env.items():
                if value is None:
                    os.environ.pop(key, None)
                else:
                    os.environ[key] = value
        step_summary = summary_path.read_text(encoding="utf-8")
    return code, stdout.getvalue(), stderr.getvalue(), step_summary


def assert_cli_fork_failure_exits_nonzero_without_publish() -> None:
    code, stdout, stderr, step_summary = run_cli_for_fork_failure()
    if code != 1:
        raise AssertionError((code, stdout, stderr))
    if "actionlint" not in stdout:
        raise AssertionError(stdout)
    if "advisory" not in step_summary or "actionlint" not in step_summary:
        raise AssertionError(step_summary)
    if stderr:
        raise AssertionError(stderr)


def assert_cli_summary_write_failure_warns_without_failing_green_verdict() -> None:
    module = load_script()
    event_text = json.dumps(pull_request_event())
    with tempfile.TemporaryDirectory() as tmp:
        tmpdir = pathlib.Path(tmp)
        event_path = tmpdir / "event.json"
        event_path.write_text(event_text, encoding="utf-8")
        config = write_config(tmpdir)
        workflow_dir = write_workflows(tmpdir)
        fake = FakeGitHub([[check_run("host-health"), check_run("actionlint", run_id=2)]])
        env_keys = (
            "GITHUB_REPOSITORY",
            "GITHUB_TOKEN",
            "GITHUB_EVENT_PATH",
            "GITHUB_STEP_SUMMARY",
        )
        old_env = {key: os.environ.get(key) for key in env_keys}
        try:
            os.environ.update(
                {
                    "GITHUB_REPOSITORY": REPO,
                    "GITHUB_TOKEN": "token",
                    "GITHUB_EVENT_PATH": str(event_path),
                    "GITHUB_STEP_SUMMARY": str(tmpdir / "missing" / "summary.md"),
                }
            )
            stdout = io.StringIO()
            stderr = io.StringIO()
            with contextlib.redirect_stdout(stdout), contextlib.redirect_stderr(stderr):
                code = module.main(
                    [
                        "--config",
                        str(config),
                        "--workflow-dir",
                        str(workflow_dir),
                    ],
                    api_json=fake.json,
                )
        finally:
            for key, value in old_env.items():
                if value is None:
                    os.environ.pop(key, None)
                else:
                    os.environ[key] = value
    if code != 0:
        raise AssertionError((code, stdout.getvalue(), stderr.getvalue()))
    if "could not write GITHUB_STEP_SUMMARY" not in stderr.getvalue():
        raise AssertionError(stderr.getvalue())


def assert_non_object_event_fails_closed() -> None:
    module = load_script()
    try:
        module.head_sha_from_event([])
    except module.CoverageEnforcerError as exc:
        if "event payload is malformed" not in str(exc):
            raise AssertionError(str(exc)) from exc
    else:
        raise AssertionError("non-object event unexpectedly succeeded")


def main() -> int:
    assert_drift_detects_missing_and_wrong_app()
    assert_all_present_and_correct_succeeds()
    assert_iteration_pr_does_not_wait_for_boundary_gates()
    assert_merge_boundary_waits_for_required_gates()
    assert_newer_partial_gate_pair_fails_closed_over_stale_complete_pair()
    assert_poll_timeout_fails_closed()
    assert_same_app_reruns_do_not_count_as_duplicate_drift()
    assert_wrong_app_rerun_remains_drift_even_with_expected_app_success()
    assert_expected_app_failure_without_success_is_drift()
    assert_latest_expected_app_failure_after_success_is_drift()
    assert_latest_expected_app_success_after_failure_succeeds()
    assert_newer_in_progress_expected_app_keeps_context_pending()
    assert_r2_derivation_mismatch_fails()
    assert_r2_derives_generic_tag_triggers()
    assert_fork_pr_uses_native_job_without_publishing()
    assert_self_reporter_is_excluded_even_when_required()
    assert_docs_only_false_is_not_weaker_for_watchdog_events()
    assert_target_contexts_keep_docs_iteration_equivalent()
    assert_real_registry_derivation_matches_current_workflows()
    assert_cli_fork_failure_exits_nonzero_without_publish()
    assert_cli_summary_write_failure_warns_without_failing_green_verdict()
    assert_non_object_event_fails_closed()
    print("OK: coverage enforcer self-tests passed.")
    return 0


if __name__ == "__main__":
    import lane_governor

    lane_governor.acquire()
    sys.exit(main())
