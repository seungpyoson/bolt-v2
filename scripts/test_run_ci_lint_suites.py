#!/usr/bin/env python3
"""Self-tests for the ci-lint workflow suite runner."""

from __future__ import annotations

import contextlib
import importlib.util
import io
import pathlib
import sys


REPO_ROOT = pathlib.Path(__file__).resolve().parents[1]
RUNNER = REPO_ROOT / "scripts" / "run_ci_lint_suites.py"


def load_runner_module() -> object:
    spec = importlib.util.spec_from_file_location("run_ci_lint_suites_under_test", RUNNER)
    if spec is None or spec.loader is None:
        raise AssertionError("unable to load run_ci_lint_suites.py")
    module = importlib.util.module_from_spec(spec)
    sys.modules[spec.name] = module
    spec.loader.exec_module(module)
    return module


def python_command(body: str) -> tuple[str, ...]:
    return (sys.executable, "-c", body)


def test_runner_groups_each_suite_output_and_reports_all_failures() -> None:
    runner = load_runner_module()
    suites = (
        runner.CiLintSuite(
            "first-failure",
            python_command("import sys; print('first stdout'); print('first stderr', file=sys.stderr); raise SystemExit(3)"),
        ),
        runner.CiLintSuite(
            "second-failure",
            python_command("import sys; print('second stdout'); print('second stderr', file=sys.stderr); raise SystemExit(7)"),
        ),
    )
    stream = io.StringIO()

    status = runner.run_suites(suites, workers=2, stdout=stream, stderr=stream)

    output = stream.getvalue()
    if status != 1:
        raise AssertionError(status)
    first_header = output.find("=== ci-lint suite: first-failure ===")
    second_header = output.find("=== ci-lint suite: second-failure ===")
    if first_header == -1 or second_header == -1:
        raise AssertionError(output)
    first_stdout = output.find("first stdout")
    first_stderr = output.find("first stderr")
    second_stdout = output.find("second stdout")
    second_stderr = output.find("second stderr")
    if not (first_header < first_stdout < first_stderr < second_header < second_stdout < second_stderr):
        raise AssertionError(output)
    for expected in ("first-failure exited 3", "second-failure exited 7"):
        if expected not in output:
            raise AssertionError(output)


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
        "python3 scripts/test_verify_ci_workflow_hygiene.py",
        "python3 scripts/test_ci_test_manifest.py",
        "python3 scripts/test_cancel_obsolete_dispatch_runs.py",
        "python3 scripts/test_config_validators.py",
        "python3 scripts/test_run_rust_probe.py",
        "python3 scripts/test_rust_probe_wrapper.py",
        "python3 scripts/test_ci_provenance.py",
        "python3 scripts/test_ci_input_sets.py",
        "python3 scripts/test_rust_test_targets.py",
        "python3 scripts/test_merge_readiness.py",
        "python3 scripts/test_merge_queue_preflight.py",
        "python3 scripts/test_merge_queue_operator.py",
        "python3 scripts/test_coverage_enforcer.py",
        "python3 scripts/test_nextest_fingerprint.py",
        "python3 scripts/test_root_bin_sidecars.py",
        "python3 scripts/test_ci_storage_audit.py",
        "python3 scripts/test_ci_storage_tripwire.py",
        "python3 scripts/test_find_same_sha_main_evidence.py",
        "python3 scripts/test_ubicloud_runner_minutes.py",
        "python3 scripts/test_verify_ci_path_filters.py",
        "python3 scripts/test_rust_verification.py",
        "python3 scripts/test_verify_remote.py",
        "python3 scripts/test_command_understanding.py",
        "python3 scripts/test_rust_verification_decoupling.py",
        "python3 scripts/test_rust_verification_cache_retention.py",
        "python3 scripts/verify_ci_path_filters.py",
        "python3 scripts/verify_ci_workflow_hygiene.py",
        "python3 scripts/test_run_ci_lint_suites.py",
    }
    missing = sorted(expected - commands)
    extra = sorted(commands - expected)
    if missing or extra:
        raise AssertionError(f"missing={missing!r} extra={extra!r}")


def main() -> int:
    tests = (
        test_runner_groups_each_suite_output_and_reports_all_failures,
        test_runner_rejects_unbounded_worker_count_for_default_workflow,
        test_default_suite_table_covers_the_ci_lint_contract,
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
