#!/usr/bin/env python3
"""Run ci-lint workflow suites with bounded parallel subprocess workers."""

from __future__ import annotations

import argparse
import dataclasses
import pathlib
import subprocess
import sys
from collections.abc import Iterable
from concurrent.futures import ThreadPoolExecutor, as_completed
from typing import TextIO


REPO_ROOT = pathlib.Path(__file__).resolve().parents[1]
DEFAULT_WORKERS = 4
MAX_WORKERS = 6


@dataclasses.dataclass(frozen=True)
class CiLintSuite:
    name: str
    command: tuple[str, ...]


@dataclasses.dataclass(frozen=True)
class SuiteResult:
    suite: CiLintSuite
    returncode: int
    stdout: str
    stderr: str


CI_LINT_SUITES = (
    CiLintSuite("ci-workflow-hygiene-tests", ("python3", "scripts/test_verify_ci_workflow_hygiene.py")),
    CiLintSuite("ci-test-manifest", ("python3", "scripts/test_ci_test_manifest.py")),
    CiLintSuite("cancel-obsolete-dispatch-runs", ("python3", "scripts/test_cancel_obsolete_dispatch_runs.py")),
    CiLintSuite("config-validators", ("python3", "scripts/test_config_validators.py")),
    CiLintSuite("run-rust-probe", ("python3", "scripts/test_run_rust_probe.py")),
    CiLintSuite("rust-probe-wrapper", ("python3", "scripts/test_rust_probe_wrapper.py")),
    CiLintSuite("ci-provenance", ("python3", "scripts/test_ci_provenance.py")),
    CiLintSuite("ci-input-sets", ("python3", "scripts/test_ci_input_sets.py")),
    CiLintSuite("rust-test-targets", ("python3", "scripts/test_rust_test_targets.py")),
    CiLintSuite("merge-readiness", ("python3", "scripts/test_merge_readiness.py")),
    CiLintSuite("merge-queue-preflight", ("python3", "scripts/test_merge_queue_preflight.py")),
    CiLintSuite("merge-queue-operator", ("python3", "scripts/test_merge_queue_operator.py")),
    CiLintSuite("coverage-enforcer", ("python3", "scripts/test_coverage_enforcer.py")),
    CiLintSuite("nextest-fingerprint", ("python3", "scripts/test_nextest_fingerprint.py")),
    CiLintSuite("root-bin-sidecars", ("python3", "scripts/test_root_bin_sidecars.py")),
    CiLintSuite("ci-storage-audit", ("python3", "scripts/test_ci_storage_audit.py")),
    CiLintSuite("ci-storage-tripwire", ("python3", "scripts/test_ci_storage_tripwire.py")),
    CiLintSuite("same-sha-main-evidence", ("python3", "scripts/test_find_same_sha_main_evidence.py")),
    CiLintSuite("ubicloud-runner-minutes", ("python3", "scripts/test_ubicloud_runner_minutes.py")),
    CiLintSuite("verify-ci-path-filters-tests", ("python3", "scripts/test_verify_ci_path_filters.py")),
    CiLintSuite("rust-verification", ("python3", "scripts/test_rust_verification.py")),
    CiLintSuite("verify-remote", ("python3", "scripts/test_verify_remote.py")),
    CiLintSuite("command-understanding", ("python3", "scripts/test_command_understanding.py")),
    CiLintSuite("rust-verification-decoupling", ("python3", "scripts/test_rust_verification_decoupling.py")),
    CiLintSuite("rust-verification-cache-retention", ("python3", "scripts/test_rust_verification_cache_retention.py")),
    CiLintSuite("verify-ci-path-filters", ("python3", "scripts/verify_ci_path_filters.py")),
    CiLintSuite("ci-workflow-hygiene-verifier", ("python3", "scripts/verify_ci_workflow_hygiene.py")),
    CiLintSuite("run-ci-lint-suites", ("python3", "scripts/test_run_ci_lint_suites.py")),
)


def validate_workers(workers: int) -> int:
    if workers < 1 or workers > MAX_WORKERS:
        raise ValueError(f"workers must be between 1 and {MAX_WORKERS}")
    return workers


def run_one_suite(suite: CiLintSuite, repo_root: pathlib.Path) -> SuiteResult:
    try:
        result = subprocess.run(
            list(suite.command),
            cwd=repo_root,
            text=True,
            stdout=subprocess.PIPE,
            stderr=subprocess.PIPE,
            check=False,
        )
    except OSError as exc:
        return SuiteResult(suite=suite, returncode=127, stdout="", stderr=f"{type(exc).__name__}: {exc}\n")
    return SuiteResult(suite=suite, returncode=result.returncode, stdout=result.stdout, stderr=result.stderr)


def write_block(stream: TextIO, text: str) -> None:
    if not text:
        return
    stream.write(text)
    if not text.endswith("\n"):
        stream.write("\n")


def print_result(result: SuiteResult, *, stdout: TextIO, stderr: TextIO) -> None:
    stdout.write(f"=== ci-lint suite: {result.suite.name} ===\n")
    stdout.write(f"$ {' '.join(result.suite.command)}\n")
    write_block(stdout, result.stdout)
    write_block(stderr, result.stderr)
    if result.returncode == 0:
        stdout.write(f"OK: {result.suite.name}\n")
    else:
        stderr.write(f"FAIL: {result.suite.name} exited {result.returncode}\n")


def run_suites(
    suites: Iterable[CiLintSuite] = CI_LINT_SUITES,
    *,
    workers: int = DEFAULT_WORKERS,
    repo_root: pathlib.Path = REPO_ROOT,
    stdout: TextIO = sys.stdout,
    stderr: TextIO = sys.stderr,
) -> int:
    worker_count = validate_workers(workers)
    suite_list = tuple(suites)
    if not suite_list:
        return 0
    results_by_index: dict[int, SuiteResult] = {}
    with ThreadPoolExecutor(max_workers=worker_count) as executor:
        futures = {executor.submit(run_one_suite, suite, repo_root): index for index, suite in enumerate(suite_list)}
        for future in as_completed(futures):
            result = future.result()
            results_by_index[futures[future]] = result

    failures: list[SuiteResult] = []
    for index, suite in enumerate(suite_list):
        result = results_by_index[index]
        print_result(result, stdout=stdout, stderr=stderr)
        if result.returncode != 0:
            failures.append(result)

    if failures:
        stderr.write(f"FAIL: {len(failures)} ci-lint suite(s) failed:\n")
        for result in failures:
            stderr.write(f"- {result.suite.name} exited {result.returncode}\n")
        return 1
    stdout.write(f"OK: {len(suite_list)} ci-lint suite(s) passed.\n")
    return 0


def parse_args(argv: list[str]) -> argparse.Namespace:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--workers", type=int, default=DEFAULT_WORKERS)
    parser.add_argument("--repo-root", type=pathlib.Path, default=REPO_ROOT)
    parser.add_argument("--list", action="store_true", help="print suite commands without running them")
    return parser.parse_args(argv)


def main(argv: list[str] | None = None) -> int:
    args = parse_args(sys.argv[1:] if argv is None else argv)
    if args.list:
        for suite in CI_LINT_SUITES:
            print(" ".join(suite.command))
        return 0
    return run_suites(workers=args.workers, repo_root=args.repo_root)


if __name__ == "__main__":
    raise SystemExit(main())
