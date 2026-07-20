#!/usr/bin/env python3
"""Run ci-lint workflow suites with bounded parallel subprocess workers."""

from __future__ import annotations

import argparse
import dataclasses
import pathlib
import subprocess
import sys
from collections.abc import Iterable, Mapping, Set
from concurrent.futures import ThreadPoolExecutor, as_completed
from typing import TextIO


REPO_ROOT = pathlib.Path(__file__).resolve().parents[1]
DEFAULT_WORKERS = 4
MAX_WORKERS = 6
DEFAULT_TIMEOUT_SECONDS = 900


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
    CiLintSuite(
        "unified-verification-deletion-fence",
        ("python3", "scripts/test_unified_verification_deletion_fence.py"),
    ),
    CiLintSuite("workspace-registry", ("python3", "scripts/test_workspace_registry.py")),
    CiLintSuite("workspace-advisories", ("python3", "scripts/test_workspace_advisories.py")),
    CiLintSuite("repository-preflight", ("python3", "scripts/test_repo_preflight.py")),
    CiLintSuite("repository-format", ("python3", "scripts/test_repo_format.py")),
    CiLintSuite("cargo-command-analysis", ("python3", "scripts/test_cargo_command_analysis.py")),
    CiLintSuite("cargo-shim", ("python3", "-m", "pytest", "scripts/test_cargo_shim.py", "-q")),
    CiLintSuite("ci-test-manifest", ("python3", "scripts/test_ci_test_manifest.py")),
    CiLintSuite("config-validators", ("python3", "scripts/test_config_validators.py")),
    CiLintSuite("run-rust-probe", ("python3", "scripts/test_run_rust_probe.py")),
    CiLintSuite("rust-probe-wrapper", ("python3", "scripts/test_rust_probe_wrapper.py")),
    CiLintSuite("rust-test-targets", ("python3", "scripts/test_rust_test_targets.py")),
    CiLintSuite("merge-queue-operator", ("python3", "scripts/test_merge_queue_operator.py")),
    CiLintSuite("merge-queue-preflight", ("python3", "scripts/test_merge_queue_preflight.py")),
    CiLintSuite("root-bin-sidecars", ("python3", "scripts/test_root_bin_sidecars.py")),
    CiLintSuite("ci-storage-audit", ("python3", "scripts/test_ci_storage_audit.py")),
    CiLintSuite("ci-storage-tripwire", ("python3", "scripts/test_ci_storage_tripwire.py")),
    CiLintSuite("ubicloud-runner-minutes", ("python3", "scripts/test_ubicloud_runner_minutes.py")),
    CiLintSuite("rust-verification", ("python3", "scripts/test_rust_verification.py")),
    CiLintSuite("sandbox-safe-push", ("python3", "scripts/test_sandbox_safe_push.py")),
    CiLintSuite("command-understanding", ("python3", "scripts/test_command_understanding.py")),
    CiLintSuite("rust-verification-decoupling", ("python3", "scripts/test_rust_verification_decoupling.py")),
    CiLintSuite("rust-verification-cache-retention", ("python3", "scripts/test_rust_verification_cache_retention.py")),
    CiLintSuite("sccache-eligibility", ("python3", "scripts/test_sccache_eligibility.py")),
    CiLintSuite("clean-merged-artifacts", ("python3", "scripts/test_clean_merged_artifacts.py")),
    CiLintSuite("developer-tool-storage-hygiene", ("python3", "scripts/test_developer_tool_storage_hygiene.py")),
    CiLintSuite("ci-workflow-hygiene-verifier", ("python3", "scripts/verify_ci_workflow_hygiene.py")),
    CiLintSuite("run-ci-lint-suites", ("python3", "scripts/test_run_ci_lint_suites.py")),
)

GOVERNED_TEST_SUFFIXES = frozenset({".py", ".mjs"})
INACTIVE_TEST_FILENAMES = frozenset(
    {
        "test_host_health_sampler.py",
        "test_host_health_viewer.mjs",
    }
)


def discover_governed_test_files(repo_root: pathlib.Path) -> set[str]:
    scripts = repo_root / "scripts"
    return {
        path.name
        for path in scripts.iterdir()
        if path.is_file()
        and path.name.startswith("test_")
        and path.suffix in GOVERNED_TEST_SUFFIXES
        and path.name not in INACTIVE_TEST_FILENAMES
    }


def validate_exact_test_ownership(discovered: set[str], ownership: Mapping[str, Set[str]]) -> None:
    owners_by_file: dict[str, list[str]] = {}
    for owner, filenames in ownership.items():
        for filename in filenames:
            owners_by_file.setdefault(filename, []).append(owner)
    missing = sorted(discovered - owners_by_file.keys())
    stale = sorted(owners_by_file.keys() - discovered)
    duplicates = {
        filename: sorted(owners)
        for filename, owners in owners_by_file.items()
        if len(owners) != 1
    }
    if missing or stale or duplicates:
        raise ValueError(
            f"unclaimed test suites={missing!r} stale test registrations={stale!r} "
            f"duplicate test ownership={duplicates!r}"
        )


def validate_test_suite_coverage(repo_root: pathlib.Path) -> None:
    scripts = repo_root / "scripts"
    discovered = discover_governed_test_files(repo_root)
    from run_fences import STANDALONE_TEST_FILENAMES

    ownership: dict[str, set[str]] = {}
    for suite in CI_LINT_SUITES:
        ownership[f"ci-lint:{suite.name}"] = {
            pathlib.Path(part).name
            for part in suite.command
            if part.startswith("scripts/test_") and pathlib.Path(part).suffix in GOVERNED_TEST_SUFFIXES
        }
    for path in scripts.glob("test_verify_*.py"):
        if path.with_name(path.name.removeprefix("test_")).is_file() and not path.name.startswith(
            ("test_verify_ai_", "test_verify_ci_")
        ):
            ownership[f"paired-fence:{path.name}"] = {path.name}
    for filename in STANDALONE_TEST_FILENAMES:
        ownership[f"standalone-fence:{filename}"] = {filename}
    validate_exact_test_ownership(
        discovered,
        ownership,
    )


def validate_workers(workers: int) -> int:
    if workers < 1 or workers > MAX_WORKERS:
        raise ValueError(f"workers must be between 1 and {MAX_WORKERS}")
    return workers


def format_seconds(seconds: float) -> str:
    if float(seconds).is_integer():
        return f"{int(seconds)}s"
    return f"{seconds:g}s"


def timeout_output(text: str | bytes | None) -> str:
    if text is None:
        return ""
    if isinstance(text, bytes):
        return text.decode(errors="replace")
    return text


def exception_result(suite: CiLintSuite, exc: Exception, *, context: str = "raised") -> SuiteResult:
    return SuiteResult(
        suite=suite,
        returncode=127,
        stdout="",
        stderr=f"suite {suite.name} {context} {type(exc).__name__}: {exc}\n",
    )


def run_one_suite(
    suite: CiLintSuite,
    repo_root: pathlib.Path,
    timeout_seconds: float = DEFAULT_TIMEOUT_SECONDS,
) -> SuiteResult:
    try:
        result = subprocess.run(
            list(suite.command),
            cwd=repo_root,
            text=True,
            stdout=subprocess.PIPE,
            stderr=subprocess.PIPE,
            timeout=timeout_seconds,
            check=False,
        )
    except subprocess.TimeoutExpired as exc:
        return SuiteResult(
            suite=suite,
            returncode=124,
            stdout=timeout_output(exc.stdout),
            stderr=f"{timeout_output(exc.stderr)}suite {suite.name} timed out after {format_seconds(timeout_seconds)}\n",
        )
    except Exception as exc:
        return exception_result(suite, exc)
    return SuiteResult(suite=suite, returncode=result.returncode, stdout=result.stdout, stderr=result.stderr)


def write_block(stream: TextIO, text: str) -> None:
    if not text:
        return
    stream.write(text)
    if not text.endswith("\n"):
        stream.write("\n")


def flush_streams(stdout: TextIO, stderr: TextIO) -> None:
    stdout.flush()
    stderr.flush()


def print_result(result: SuiteResult, *, stdout: TextIO, stderr: TextIO) -> None:
    stdout.write(f"=== ci-lint suite: {result.suite.name} ===\n")
    stdout.write(f"$ {' '.join(result.suite.command)}\n")
    write_block(stdout, result.stdout)
    write_block(stderr, result.stderr)
    if result.returncode == 0:
        stdout.write(f"OK: {result.suite.name}\n")
    else:
        stderr.write(f"FAIL: {result.suite.name} exited {result.returncode}\n")
    flush_streams(stdout, stderr)


def run_suites(
    suites: Iterable[CiLintSuite] = CI_LINT_SUITES,
    *,
    workers: int = DEFAULT_WORKERS,
    timeout_seconds: float = DEFAULT_TIMEOUT_SECONDS,
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
        futures = {}
        for index, suite in enumerate(suite_list):
            stderr.write(f"START: ci-lint suite {suite.name}\n")
            stderr.flush()
            futures[executor.submit(run_one_suite, suite, repo_root, timeout_seconds)] = (index, suite)
        for future in as_completed(futures):
            index, suite = futures[future]
            try:
                result = future.result()
            except Exception as exc:
                result = exception_result(suite, exc, context="worker raised")
            results_by_index[index] = result
            stderr.write(f"FINISH: ci-lint suite {suite.name} exited {result.returncode}\n")
            stderr.flush()

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
        flush_streams(stdout, stderr)
        return 1
    stdout.write(f"OK: {len(suite_list)} ci-lint suite(s) passed.\n")
    flush_streams(stdout, stderr)
    return 0


def parse_args(argv: list[str]) -> argparse.Namespace:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--workers", type=int, default=DEFAULT_WORKERS)
    parser.add_argument("--timeout-seconds", type=float, default=DEFAULT_TIMEOUT_SECONDS)
    parser.add_argument("--repo-root", type=pathlib.Path, default=REPO_ROOT)
    parser.add_argument("--list", action="store_true", help="print suite commands without running them")
    return parser.parse_args(argv)


def main(argv: list[str] | None = None) -> int:
    args = parse_args(sys.argv[1:] if argv is None else argv)
    validate_test_suite_coverage(args.repo_root)
    if args.list:
        for suite in CI_LINT_SUITES:
            print(" ".join(suite.command))
        return 0
    return run_suites(workers=args.workers, timeout_seconds=args.timeout_seconds, repo_root=args.repo_root)


if __name__ == "__main__":
    raise SystemExit(main())
