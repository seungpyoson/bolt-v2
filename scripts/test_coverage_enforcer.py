#!/usr/bin/env python3
"""Tests for coverage_enforcer.py."""

from __future__ import annotations

import contextlib
import importlib.util
import io
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


CONFIG_TOML = """
[ci_provenance]
workflow_name = "CI"
workflow_path = ".github/workflows/ci.yml"

[ci_provenance.api_limits]
workflow_runs_per_page = 100
run_jobs_per_page = 100
run_artifacts_per_page = 100
max_lookback_pages = 10
max_lookback_age_seconds = 2592000

[ci_provenance.merge_readiness]
comment_marker = "bolt-v2-merge-readiness"
poll_seconds = 1
max_watch_seconds = 1

[ci_provenance.gate_names]
gate_required = "gate"
gate_iteration = "gate-iteration"
backtester_required = "backtester-gate"
backtester_iteration = "backtester-gate-iteration"

[ci_provenance.required_checks.gate]
context = "gate"
reporter = "ci.yml gate summary job"
integration_id = 15368
required = true
target = true
runs_on_tags = true
supports_carry_forward = true
arrivals = ["pull_request", "merge_group"]

[ci_provenance.required_checks.gate.proof_rule]
fresh = ["full", "docs", "iteration", "tag_reuse"]
carry_forward = ["defer", "noop"]

[ci_provenance.required_checks.backtester-gate]
context = "backtester-gate"
reporter = "backtester-ci.yml gate job"
integration_id = 15368
required = true
target = true
runs_on_tags = true
supports_carry_forward = true
arrivals = ["pull_request", "merge_group"]

[ci_provenance.required_checks.backtester-gate.proof_rule]
fresh = ["full", "docs", "iteration", "tag_reuse"]
carry_forward = ["defer", "noop"]

[ci_provenance.required_checks.host-health]
context = "host-health"
reporter = "ci.yml host-health lane"
integration_id = 15368
required = true
target = true
runs_on_tags = false
supports_carry_forward = false
arrivals = ["pull_request", "merge_group"]

[ci_provenance.required_checks.host-health.proof_rule]
fresh = ["full", "docs", "iteration", "defer", "noop"]
carry_forward = []

[ci_provenance.required_checks.actionlint]
context = "actionlint"
reporter = "actionlint.yml"
integration_id = 15368
required = true
target = true
runs_on_tags = false
supports_carry_forward = false
arrivals = ["pull_request", "merge_group"]

[ci_provenance.required_checks.actionlint.proof_rule]
fresh = ["full", "docs", "iteration", "defer", "noop"]
carry_forward = []

[ci_provenance.required_checks.coverage-enforcer]
context = "coverage-enforcer"
reporter = "self"
integration_id = 15368
required = false
target = true
runs_on_tags = false
supports_carry_forward = false
arrivals = ["pull_request", "merge_group"]

[ci_provenance.required_checks.coverage-enforcer.proof_rule]
fresh = ["full", "docs", "iteration", "defer", "noop"]
carry_forward = []
"""


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
    app_id: int = APP_ID,
    status: str = "completed",
    conclusion: str | None = "success",
) -> dict[str, object]:
    return {
        "name": name,
        "status": status,
        "conclusion": conclusion,
        "app": {"id": app_id},
    }


def pull_request_event(*, fork: bool = False) -> dict[str, object]:
    head_repo = "external/fork" if fork else REPO
    return {
        "pull_request": {
            "head": {"sha": SHA, "repo": {"full_name": head_repo}},
            "base": {"repo": {"full_name": REPO}},
        }
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
            return {"id": 1001}
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
        clock=FakeClock([0.0, 2.0]),
    )
    if result.conclusion != "failure":
        raise AssertionError(result)
    if "backtester-gate" not in result.summary or "wrong GitHub App" not in result.summary:
        raise AssertionError(result.summary)
    if "host-health" not in result.summary or "timed out" not in result.summary:
        raise AssertionError(result.summary)
    posted = fake.posted_check_runs()
    if len(posted) != 1 or posted[0]["conclusion"] != "failure":
        raise AssertionError(posted)


def assert_all_present_and_correct_succeeds() -> None:
    contexts = ("gate", "backtester-gate", "host-health", "actionlint")
    result, fake = run_enforcer([[check_run(context) for context in contexts]])
    if result.conclusion != "success" or result.findings:
        raise AssertionError(result)
    posted = fake.posted_check_runs()
    if len(posted) != 1:
        raise AssertionError(posted)
    payload = posted[0]
    if payload["name"] != "coverage-enforcer":
        raise AssertionError(payload)
    if payload["head_sha"] != SHA or payload["conclusion"] != "success":
        raise AssertionError(payload)


def assert_iteration_gate_contexts_succeed() -> None:
    result, fake = run_enforcer(
        [
            [
                check_run("gate-iteration"),
                check_run("backtester-gate-iteration"),
                check_run("host-health"),
                check_run("actionlint"),
            ]
        ],
        clock=FakeClock([0.0, 2.0]),
    )
    if result.conclusion != "success" or result.findings:
        raise AssertionError(result)
    posted = fake.posted_check_runs()
    if len(posted) != 1 or posted[0]["conclusion"] != "success":
        raise AssertionError(posted)


def assert_poll_timeout_fails_closed() -> None:
    result, fake = run_enforcer(
        [[check_run("gate", status="in_progress", conclusion=None)]],
        clock=FakeClock([0.0, 2.0]),
    )
    if result.conclusion != "failure":
        raise AssertionError(result)
    if "timed out waiting for terminal check-runs" not in result.summary:
        raise AssertionError(result.summary)
    posted = fake.posted_check_runs()
    if len(posted) != 1 or posted[0]["conclusion"] != "failure":
        raise AssertionError(posted)


def assert_r2_derivation_mismatch_fails() -> None:
    bad_config = CONFIG_TOML.replace(
        "[ci_provenance.required_checks.actionlint]\ncontext = \"actionlint\"",
        "[ci_provenance.required_checks.actionlint]\ncontext = \"actionlint\"",
    ).replace(
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
    )
    result, fake = run_enforcer(
        [[check_run(context) for context in ("gate", "backtester-gate", "host-health", "actionlint")]],
        config_text=bad_config,
    )
    if result.conclusion != "failure":
        raise AssertionError(result)
    if "registry/YAML derivation mismatch for actionlint" not in result.summary:
        raise AssertionError(result.summary)
    posted = fake.posted_check_runs()
    if len(posted) != 1 or posted[0]["conclusion"] != "failure":
        raise AssertionError(posted)


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


def assert_fork_pr_skips_publishing_but_fails_nonzero_semantics() -> None:
    result, fake = run_enforcer(
        [[check_run("gate"), check_run("backtester-gate", app_id=999), check_run("host-health"), check_run("actionlint")]],
        event=pull_request_event(fork=True),
    )
    if result.conclusion != "failure" or result.published:
        raise AssertionError(result)
    if fake.posted_check_runs():
        raise AssertionError(fake.requests)


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


def run_cli_for_fork_failure() -> tuple[int, str, str]:
    module = load_script()
    event_text = """{"pull_request":{"head":{"sha":"%s","repo":{"full_name":"external/fork"}},"base":{"repo":{"full_name":"%s"}}}}""" % (
        SHA,
        REPO,
    )
    with tempfile.TemporaryDirectory() as tmp:
        tmpdir = pathlib.Path(tmp)
        event_path = tmpdir / "event.json"
        event_path.write_text(event_text, encoding="utf-8")
        config = write_config(tmpdir)
        workflow_dir = write_workflows(tmpdir)

        fake = FakeGitHub([[check_run("gate"), check_run("backtester-gate", app_id=999), check_run("host-health"), check_run("actionlint")]])
        old_env = {key: os.environ.get(key) for key in ("GITHUB_REPOSITORY", "GITHUB_TOKEN", "GITHUB_EVENT_PATH")}
        os.environ.update(
            {
                "GITHUB_REPOSITORY": REPO,
                "GITHUB_TOKEN": "token",
                "GITHUB_EVENT_PATH": str(event_path),
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
    return code, stdout.getvalue(), stderr.getvalue()


def assert_cli_fork_failure_exits_nonzero_without_publish() -> None:
    code, stdout, stderr = run_cli_for_fork_failure()
    if code != 1:
        raise AssertionError((code, stdout, stderr))
    if "skipping coverage-enforcer check-run publish for fork PR" not in stdout:
        raise AssertionError(stdout)
    if stderr:
        raise AssertionError(stderr)


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
    assert_iteration_gate_contexts_succeed()
    assert_poll_timeout_fails_closed()
    assert_r2_derivation_mismatch_fails()
    assert_r2_derives_generic_tag_triggers()
    assert_fork_pr_skips_publishing_but_fails_nonzero_semantics()
    assert_real_registry_derivation_matches_current_workflows()
    assert_cli_fork_failure_exits_nonzero_without_publish()
    assert_non_object_event_fails_closed()
    print("OK: coverage enforcer self-tests passed.")
    return 0


if __name__ == "__main__":
    import lane_governor

    lane_governor.acquire()
    sys.exit(main())
