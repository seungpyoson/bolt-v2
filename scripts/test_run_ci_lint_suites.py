#!/usr/bin/env python3
"""Self-tests for the ci-lint workflow suite runner."""

from __future__ import annotations

import contextlib
import importlib.util
import io
import pathlib
import sys
import tomllib


REPO_ROOT = pathlib.Path(__file__).resolve().parents[1]
RUNNER = REPO_ROOT / "scripts" / "run_ci_lint_suites.py"
WORKFLOW_HYGIENE = REPO_ROOT / "scripts" / "verify_ci_workflow_hygiene.py"


def load_runner_module() -> object:
    spec = importlib.util.spec_from_file_location("run_ci_lint_suites_under_test", RUNNER)
    if spec is None or spec.loader is None:
        raise AssertionError("unable to load run_ci_lint_suites.py")
    module = importlib.util.module_from_spec(spec)
    sys.modules[spec.name] = module
    spec.loader.exec_module(module)
    return module


def load_workflow_hygiene_module() -> object:
    spec = importlib.util.spec_from_file_location("verify_ci_workflow_hygiene_under_test", WORKFLOW_HYGIENE)
    if spec is None or spec.loader is None:
        raise AssertionError("unable to load verify_ci_workflow_hygiene.py")
    module = importlib.util.module_from_spec(spec)
    sys.modules[spec.name] = module
    spec.loader.exec_module(module)
    return module


def python_command(body: str) -> tuple[str, ...]:
    return (sys.executable, "-c", body)


class FlushTrackingStringIO(io.StringIO):
    def __init__(self) -> None:
        super().__init__()
        self.flush_count = 0

    def flush(self) -> None:
        self.flush_count += 1
        super().flush()


def test_runner_groups_each_suite_output_and_reports_all_failures() -> None:
    runner = load_runner_module()
    suites = (
        runner.CiLintSuite(
            "first-failure",
            python_command("import sys; print('first-' + 'stdout'); print('first-' + 'stderr', file=sys.stderr); raise SystemExit(3)"),
        ),
        runner.CiLintSuite(
            "second-failure",
            python_command("import sys; print('second-' + 'stdout'); print('second-' + 'stderr', file=sys.stderr); raise SystemExit(7)"),
        ),
    )
    stdout = io.StringIO()
    stderr = io.StringIO()

    status = runner.run_suites(suites, workers=2, stdout=stdout, stderr=stderr)

    stdout_output = stdout.getvalue()
    stderr_output = stderr.getvalue()
    if status != 1:
        raise AssertionError(status)
    first_header = stdout_output.find("=== ci-lint suite: first-failure ===")
    second_header = stdout_output.find("=== ci-lint suite: second-failure ===")
    if first_header == -1 or second_header == -1:
        raise AssertionError(stdout_output)
    first_stdout = stdout_output.find("first-stdout")
    second_stdout = stdout_output.find("second-stdout")
    if not (first_header < first_stdout < second_header < second_stdout):
        raise AssertionError(stdout_output)
    if "first-stderr" in stdout_output or "second-stderr" in stdout_output:
        raise AssertionError(stdout_output)
    if "first-stdout" in stderr_output or "second-stdout" in stderr_output:
        raise AssertionError(stderr_output)
    first_stderr = stderr_output.find("first-stderr")
    first_failure = stderr_output.find("FAIL: first-failure exited 3")
    second_stderr = stderr_output.find("second-stderr")
    second_failure = stderr_output.find("FAIL: second-failure exited 7")
    if not (first_stderr < first_failure < second_stderr < second_failure):
        raise AssertionError(stderr_output)


def test_runner_flushes_grouped_blocks_and_final_summary() -> None:
    runner = load_runner_module()
    suites = (
        runner.CiLintSuite("first", python_command("print('first')")),
        runner.CiLintSuite("second", python_command("print('second')")),
    )
    stdout = FlushTrackingStringIO()
    stderr = FlushTrackingStringIO()

    status = runner.run_suites(suites, workers=2, stdout=stdout, stderr=stderr)

    if status != 0:
        raise AssertionError(status)
    if stdout.flush_count < 3:
        raise AssertionError(f"grouped stdout blocks and final summary were not flushed: {stdout.flush_count}")
    if stderr.flush_count < 7:
        raise AssertionError(f"grouped stderr blocks and final summary were not flushed: {stderr.flush_count}")


def test_runner_emits_start_finish_breadcrumbs_to_stderr() -> None:
    runner = load_runner_module()
    suites = (
        runner.CiLintSuite("first", python_command("print('first')")),
        runner.CiLintSuite("second", python_command("print('second')")),
    )
    stderr = io.StringIO()

    status = runner.run_suites(suites, workers=2, stdout=io.StringIO(), stderr=stderr)

    if status != 0:
        raise AssertionError(status)
    output = stderr.getvalue()
    for expected in ("first-failure exited 3", "second-failure exited 7"):
        if expected in output:
            raise AssertionError(output)
    for expected in (
        "START: ci-lint suite first",
        "START: ci-lint suite second",
        "FINISH: ci-lint suite first exited 0",
        "FINISH: ci-lint suite second exited 0",
    ):
        if expected not in output:
            raise AssertionError(output)


def test_runner_timeout_is_attributed_to_suite() -> None:
    runner = load_runner_module()
    suites = (runner.CiLintSuite("slow", python_command("import time; time.sleep(60)")),)
    stderr = io.StringIO()

    status = runner.run_suites(suites, workers=1, timeout_seconds=0.01, stdout=io.StringIO(), stderr=stderr)

    output = stderr.getvalue()
    if status != 1:
        raise AssertionError(status)
    for expected in (
        "suite slow timed out after 0.01s",
        "FAIL: slow exited 124",
        "FINISH: ci-lint suite slow exited 124",
    ):
        if expected not in output:
            raise AssertionError(output)


def test_runner_timeout_preserves_partial_output_in_grouped_result() -> None:
    runner = load_runner_module()
    suites = (
        runner.CiLintSuite(
            "partial-timeout",
            python_command(
                "import sys, time; "
                "print('partial-' + 'stdout', flush=True); "
                "print('partial-' + 'stderr', file=sys.stderr, flush=True); "
                "time.sleep(60)"
            ),
        ),
    )
    stdout = io.StringIO()
    stderr = io.StringIO()

    status = runner.run_suites(suites, workers=1, timeout_seconds=1.0, stdout=stdout, stderr=stderr)

    stdout_output = stdout.getvalue()
    stderr_output = stderr.getvalue()
    if status != 1:
        raise AssertionError(status)
    if "=== ci-lint suite: partial-timeout ===" not in stdout_output:
        raise AssertionError(stdout_output)
    if "partial-stdout" not in stdout_output:
        raise AssertionError(stdout_output)
    if "partial-stderr" not in stderr_output:
        raise AssertionError(stderr_output)
    for expected in (
        "suite partial-timeout timed out after 1s",
        "FAIL: partial-timeout exited 124",
        "FINISH: ci-lint suite partial-timeout exited 124",
    ):
        if expected not in stderr_output:
            raise AssertionError(stderr_output)


def test_run_one_suite_turns_unexpected_exceptions_into_attributed_result() -> None:
    runner = load_runner_module()
    original_run = runner.subprocess.run

    def explode(*_args: object, **_kwargs: object) -> object:
        raise RuntimeError("subprocess wrapper exploded")

    try:
        runner.subprocess.run = explode
        result = runner.run_one_suite(
            runner.CiLintSuite("crash", python_command("raise SystemExit(0)")),
            REPO_ROOT,
            timeout_seconds=900,
        )
    finally:
        runner.subprocess.run = original_run

    if result.returncode == 0:
        raise AssertionError(result)
    if "suite crash raised RuntimeError: subprocess wrapper exploded" not in result.stderr:
        raise AssertionError(result.stderr)


def test_runner_future_crashes_are_attributed_and_do_not_abort_battery() -> None:
    runner = load_runner_module()
    original_run_one_suite = runner.run_one_suite

    def maybe_crash(suite: object, repo_root: pathlib.Path, timeout_seconds: float) -> object:
        if suite.name == "crash":
            raise RuntimeError("worker exploded")
        return original_run_one_suite(suite, repo_root, timeout_seconds)

    try:
        runner.run_one_suite = maybe_crash
        stdout = io.StringIO()
        stderr = io.StringIO()
        status = runner.run_suites(
            (
                runner.CiLintSuite("crash", python_command("raise SystemExit(0)")),
                runner.CiLintSuite("ok", python_command("print('ok still ran')")),
            ),
            workers=2,
            stdout=stdout,
            stderr=stderr,
        )
    finally:
        runner.run_one_suite = original_run_one_suite

    if status != 1:
        raise AssertionError(status)
    if "ok still ran" not in stdout.getvalue():
        raise AssertionError(stdout.getvalue())
    for expected in (
        "suite crash worker raised RuntimeError: worker exploded",
        "FAIL: crash exited 127",
    ):
        if expected not in stderr.getvalue():
            raise AssertionError(stderr.getvalue())


def test_runner_rejects_unbounded_worker_count_for_default_workflow() -> None:
    runner = load_runner_module()
    if runner.DEFAULT_WORKERS != 4:
        raise AssertionError(f"default worker count drifted: {runner.DEFAULT_WORKERS}")

    suites = (runner.CiLintSuite("noop", python_command("raise SystemExit(0)")),)
    for workers in (0, 7):
        stream = io.StringIO()
        try:
            runner.run_suites(suites, workers=workers, stdout=stream, stderr=stream)
        except ValueError as exc:
            if "workers must be between 1 and 6" not in str(exc):
                raise AssertionError(exc) from exc
        else:
            raise AssertionError(f"workers={workers} should be rejected")


def test_default_suite_table_covers_the_ci_lint_contract() -> None:
    runner = load_runner_module()
    names = [suite.name for suite in runner.CI_LINT_SUITES]
    duplicate_names = sorted({name for name in names if names.count(name) > 1})
    if duplicate_names:
        raise AssertionError(f"duplicate suite names={duplicate_names!r}")

    commands = {" ".join(suite.command) for suite in runner.CI_LINT_SUITES}
    expected = {
        "python3 scripts/test_unified_verification_deletion_fence.py",
        "python3 scripts/test_workspace_registry.py",
        "python3 scripts/test_workspace_advisories.py",
        "python3 scripts/test_repo_preflight.py",
        "python3 scripts/test_repo_format.py",
        "python3 scripts/test_cargo_command_analysis.py",
        "python3 -m pytest scripts/test_cargo_shim.py -q",
        "python3 scripts/test_ci_test_manifest.py",
        "python3 scripts/test_config_validators.py",
        "python3 scripts/test_run_rust_probe.py",
        "python3 scripts/test_rust_probe_wrapper.py",
        "python3 scripts/test_rust_test_targets.py",
        "python3 scripts/test_merge_queue_operator.py",
        "python3 scripts/test_merge_queue_preflight.py",
        "python3 scripts/test_root_bin_sidecars.py",
        "python3 scripts/test_ci_storage_audit.py",
        "python3 scripts/test_ci_storage_tripwire.py",
        "python3 scripts/test_ubicloud_runner_minutes.py",
        "python3 scripts/test_rust_verification.py",
        "python3 scripts/test_sandbox_safe_push.py",
        "python3 scripts/test_command_understanding.py",
        "python3 scripts/test_rust_verification_decoupling.py",
        "python3 scripts/test_rust_verification_cache_retention.py",
        "python3 scripts/test_sccache_eligibility.py",
        "python3 scripts/test_clean_merged_artifacts.py",
        "python3 scripts/test_developer_tool_storage_hygiene.py",
        "python3 scripts/verify_ci_workflow_hygiene.py",
        "python3 scripts/test_run_ci_lint_suites.py",
    }
    missing = sorted(expected - commands)
    extra = sorted(commands - expected)
    if missing or extra:
        raise AssertionError(f"missing={missing!r} extra={extra!r}")


def test_every_test_module_is_claimed_by_one_execution_surface() -> None:
    runner = load_runner_module()
    discovered = runner.discover_governed_test_files(REPO_ROOT)
    dormant = {
        "test_host_health_sampler.py",
        "test_host_health_viewer.mjs",
    }
    if discovered & dormant:
        raise AssertionError(f"dormant review tests remain in the active ownership census: {discovered & dormant}")
    runner.validate_test_suite_coverage(REPO_ROOT)


def test_active_ci_lint_does_not_inspect_dormant_review_workflows() -> None:
    verifier = load_workflow_hygiene_module()
    active_workflows = verifier.repo_workflow_texts()
    dormant_paths = verifier.DORMANT_REVIEW_WORKFLOW_PATHS
    if active_workflows.keys() & dormant_paths:
        raise AssertionError(f"dormant review workflows remain in active workflow hygiene: {active_workflows.keys() & dormant_paths}")
    runner_config = verifier.load_github_actions_runners_config()
    raw_runner_config = tomllib.loads(
        (REPO_ROOT / "ci" / "github-actions-runners.toml").read_text(encoding="utf-8")
    )
    raw_config_keys = raw_runner_config["workflows"].keys() | raw_runner_config["cargo_build_jobs"].keys()
    loaded_config_keys = runner_config["workflows"].keys() | runner_config["cargo_build_jobs"].keys()
    if raw_config_keys != loaded_config_keys:
        raise AssertionError("active runner configuration still contains filtered dormant tables")
    rust_probe_test = (REPO_ROOT / "scripts" / "test_rust_probe_wrapper.py").read_text(encoding="utf-8")
    if "final-review.yml" in rust_probe_test:
        raise AssertionError("active Rust Probe tests still inspect the dormant Final Review workflow")
    justfile = (REPO_ROOT / "justfile").read_text(encoding="utf-8")
    active_setup = justfile.split("\nsetup:\n", 1)[1]
    if "ci/ai-review.toml" in active_setup:
        raise AssertionError("active setup still depends on dormant review configuration")
    workflow_lint = justfile.split("\nci-lint-workflow-inner ", 1)[1].split("\nworktree ", 1)[0]
    if "repo_workflow_paths" not in workflow_lint or "active workflow discovery returned no workflows" not in workflow_lint or "actionlint \"${workflow_files[@]}\"" not in workflow_lint:
        raise AssertionError("active workflow lint does not exclude dormant review workflows")


def test_duplicate_test_ownership_is_rejected_for_every_surface_pair() -> None:
    runner = load_runner_module()
    surface_names = ("ci-lint", "paired-fence", "standalone-fence")
    for suffix in (".py", ".mjs"):
        filename = f"test_duplicate{suffix}"
        for left_index, left in enumerate(surface_names):
            for right in surface_names[left_index + 1 :]:
                ownership = {name: set() for name in surface_names}
                ownership[left].add(filename)
                ownership[right].add(filename)
                try:
                    runner.validate_exact_test_ownership({filename}, ownership)
                except ValueError as exc:
                    if filename not in str(exc) or left not in str(exc) or right not in str(exc):
                        raise AssertionError((filename, left, right, exc)) from exc
                else:
                    raise AssertionError(f"duplicate ownership accepted for {filename}: {left}, {right}")


def test_duplicate_ci_lint_suite_claims_are_rejected() -> None:
    runner = load_runner_module()
    original = runner.CI_LINT_SUITES
    duplicate = runner.CiLintSuite("duplicate-workspace-registry", ("python3", "scripts/test_workspace_registry.py"))
    try:
        runner.CI_LINT_SUITES = (*original, duplicate)
        try:
            runner.validate_test_suite_coverage(REPO_ROOT)
        except ValueError as exc:
            message = str(exc)
            if "test_workspace_registry.py" not in message or "ci-lint:workspace-registry" not in message or "ci-lint:duplicate-workspace-registry" not in message:
                raise AssertionError(message) from exc
        else:
            raise AssertionError("two CI-lint suites claimed the same governed test")
    finally:
        runner.CI_LINT_SUITES = original


def main() -> int:
    tests = (
        test_runner_groups_each_suite_output_and_reports_all_failures,
        test_runner_flushes_grouped_blocks_and_final_summary,
        test_runner_emits_start_finish_breadcrumbs_to_stderr,
        test_runner_timeout_is_attributed_to_suite,
        test_runner_timeout_preserves_partial_output_in_grouped_result,
        test_run_one_suite_turns_unexpected_exceptions_into_attributed_result,
        test_runner_future_crashes_are_attributed_and_do_not_abort_battery,
        test_runner_rejects_unbounded_worker_count_for_default_workflow,
        test_default_suite_table_covers_the_ci_lint_contract,
        test_every_test_module_is_claimed_by_one_execution_surface,
        test_active_ci_lint_does_not_inspect_dormant_review_workflows,
        test_duplicate_test_ownership_is_rejected_for_every_surface_pair,
        test_duplicate_ci_lint_suite_claims_are_rejected,
    )
    failed = 0
    for test in tests:
        try:
            test()
        except Exception as exc:
            failed = 1
            print(f"FAIL: {test.__name__}: {exc}", file=sys.stderr)
    if failed:
        return 1
    print("OK: ci-lint suite runner self-tests passed.")
    return 0


if __name__ == "__main__":
    import lane_governor

    lane_governor.acquire()
    with contextlib.suppress(KeyboardInterrupt):
        raise SystemExit(main())
    raise SystemExit(130)
