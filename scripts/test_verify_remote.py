#!/usr/bin/env python3
"""Self-tests for remote PR-check verification orchestration."""

from __future__ import annotations

import contextlib
import importlib.util
import io
import json
import pathlib
import subprocess
import sys
import tempfile
import textwrap
import types


REPO_ROOT = pathlib.Path(__file__).resolve().parents[1]
SCRIPT = REPO_ROOT / "scripts" / "rust_verification.py"


def load_owner_module() -> object:
    spec = importlib.util.spec_from_file_location("rust_verification_verify_remote_under_test", SCRIPT)
    if spec is None or spec.loader is None:
        raise AssertionError("unable to load rust_verification.py")
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


def write_policy(repo: pathlib.Path, *, checks_timeout: int = 300, overall_timeout: int = 3600) -> None:
    policy = repo / "ci" / "rust-verification.toml"
    policy.parent.mkdir(parents=True, exist_ok=True)
    policy.write_text(
        textwrap.dedent(
            f"""\
            schema_version = 2
            project_id = "bolt-v2"
            target_namespace = "bolt-v2"

            [local_compile_policy]
            enabled = true
            allowed_ci_env = "GITHUB_ACTIONS"
            break_glass_env = "BOLT_ALLOW_LOCAL_RUST"
            refused_managed_commands = ["test", "clippy", "build"]
            refused_cargo_subcommands = ["bench", "build", "check", "clippy", "doc", "fetch", "install", "nextest", "run", "rustc", "test", "zigbuild"]

            [remote_verification]
            poll_interval_seconds = 1
            checks_appear_timeout_seconds = {checks_timeout}
            overall_timeout_seconds = {overall_timeout}

            [commands]

            [commands.test]
            recipe = "managed-test"

            [commands.clippy]
            recipe = "managed-clippy"

            [commands.build]
            recipe = "managed-build"
            artifact_layout = "cargo"
            profile = "release"
            target = "aarch64-unknown-linux-gnu"
            """
        ),
        encoding="utf-8",
    )


def run_cmd_verify_remote(owner: object, repo: pathlib.Path) -> tuple[int, str]:
    stderr = io.StringIO()
    stdout = io.StringIO()
    with contextlib.redirect_stdout(stdout), contextlib.redirect_stderr(stderr):
        result = owner.cmd_verify_remote(types.SimpleNamespace(repo=str(repo)))
    return result, stdout.getvalue() + stderr.getvalue()


def assert_verify_remote_precondition_errors() -> None:
    owner = load_owner_module()
    with tempfile.TemporaryDirectory() as tmp:
        repo = pathlib.Path(tmp) / "repo"
        repo.mkdir()
        write_policy(repo)
        original_git_output = owner.git_output
        cases = [
            (
                lambda _repo, *args: ("?? scratch.rs", None)
                if args[:2] == ("status", "--porcelain")
                else ("unused", None),
                "clean worktree",
            ),
            (
                lambda _repo, *args: {
                    ("status", "--porcelain", "--untracked-files=normal"): ("", None),
                    ("rev-parse", "HEAD"): ("abc", None),
                    ("rev-parse", "@{u}"): ("def", None),
                    ("branch", "--show-current"): ("feature", None),
                }[args],
                "HEAD to be pushed",
            ),
            (
                lambda _repo, *args: {
                    ("status", "--porcelain", "--untracked-files=normal"): ("", None),
                    ("rev-parse", "HEAD"): ("abc", None),
                    ("rev-parse", "@{u}"): (None, "no upstream"),
                    ("branch", "--show-current"): ("feature", None),
                }[args],
                "git push -u origin HEAD",
            ),
        ]
        try:
            for fake_git_output, expected in cases:
                owner.git_output = fake_git_output
                result, output = run_cmd_verify_remote(owner, repo)
                if result != 2 or expected not in output:
                    raise AssertionError((expected, result, output))
        finally:
            owner.git_output = original_git_output


def assert_verify_remote_pr_errors() -> None:
    owner = load_owner_module()
    with tempfile.TemporaryDirectory() as tmp:
        repo = pathlib.Path(tmp) / "repo"
        repo.mkdir()
        write_policy(repo)
        original_preconditions = owner.ensure_verify_remote_preconditions
        original_pr = owner.pr_for_current_branch
        try:
            owner.ensure_verify_remote_preconditions = lambda _repo: ("abc", "feature", None)
            owner.pr_for_current_branch = lambda _repo, _branch: (None, "verify-remote requires an open or draft PR")
            result, output = run_cmd_verify_remote(owner, repo)
            if result != 2 or "open or draft PR" not in output:
                raise AssertionError((result, output))

            owner.pr_for_current_branch = lambda _repo, _branch: (
                {"headRefOid": "old", "url": "https://example.invalid/pr/1", "number": 1},
                None,
            )
            result, output = run_cmd_verify_remote(owner, repo)
            if result != 2 or "does not match local HEAD" not in output:
                raise AssertionError((result, output))
        finally:
            owner.ensure_verify_remote_preconditions = original_preconditions
            owner.pr_for_current_branch = original_pr


def assert_pr_checks_allows_pending_exit_code_with_json() -> None:
    owner = load_owner_module()
    original_run_capture = owner.run_capture

    def fake_run_capture(argv: list[str], *, repo: pathlib.Path) -> subprocess.CompletedProcess[str]:
        if argv[:3] != ["gh", "pr", "checks"]:
            raise AssertionError(argv)
        return subprocess.CompletedProcess(
            argv,
            8,
            stdout=json.dumps([{"name": "gate", "bucket": "pending", "state": "PENDING"}]),
            stderr="",
        )

    try:
        owner.run_capture = fake_run_capture
        checks, error = owner.pr_checks(REPO_ROOT)
    finally:
        owner.run_capture = original_run_capture
    if error is not None or checks != [{"name": "gate", "bucket": "pending", "state": "PENDING"}]:
        raise AssertionError((checks, error))


def assert_verify_remote_waits_then_passes() -> None:
    owner = load_owner_module()
    with tempfile.TemporaryDirectory() as tmp:
        repo = pathlib.Path(tmp) / "repo"
        repo.mkdir()
        write_policy(repo)
        original_preconditions = owner.ensure_verify_remote_preconditions
        original_pr = owner.pr_for_current_branch
        original_checks = owner.pr_checks
        original_sleep = owner.time.sleep
        try:
            owner.ensure_verify_remote_preconditions = lambda _repo: ("abc", "feature", None)
            owner.pr_for_current_branch = lambda _repo, _branch: (
                {"headRefOid": "abc", "url": "https://example.invalid/pr/1", "number": 1},
                None,
            )
            calls = iter(
                [
                    ([{"name": "gate", "bucket": "pending", "link": "https://example.invalid/run"}], None),
                    ([{"name": "gate", "bucket": "pass", "link": "https://example.invalid/run"}], None),
                ]
            )
            owner.pr_checks = lambda _repo: next(calls)
            owner.time.sleep = lambda _seconds: None
            result, output = run_cmd_verify_remote(owner, repo)
        finally:
            owner.ensure_verify_remote_preconditions = original_preconditions
            owner.pr_for_current_branch = original_pr
            owner.pr_checks = original_checks
            owner.time.sleep = original_sleep
    if result != 0 or "OK: remote checks passed" not in output:
        raise AssertionError((result, output))


def assert_verify_remote_no_checks_times_out() -> None:
    owner = load_owner_module()
    with tempfile.TemporaryDirectory() as tmp:
        repo = pathlib.Path(tmp) / "repo"
        repo.mkdir()
        write_policy(repo, checks_timeout=1, overall_timeout=3)
        original_preconditions = owner.ensure_verify_remote_preconditions
        original_pr = owner.pr_for_current_branch
        original_checks = owner.pr_checks
        original_sleep = owner.time.sleep
        original_monotonic = owner.time.monotonic
        try:
            owner.ensure_verify_remote_preconditions = lambda _repo: ("abc", "feature", None)
            owner.pr_for_current_branch = lambda _repo, _branch: (
                {"headRefOid": "abc", "url": "https://example.invalid/pr/1", "number": 1},
                None,
            )
            owner.pr_checks = lambda _repo: ([], None)
            owner.time.sleep = lambda _seconds: None
            times = iter([0.0, 0.0, 2.0])
            owner.time.monotonic = lambda: next(times)
            result, output = run_cmd_verify_remote(owner, repo)
        finally:
            owner.ensure_verify_remote_preconditions = original_preconditions
            owner.pr_for_current_branch = original_pr
            owner.pr_checks = original_checks
            owner.time.sleep = original_sleep
            owner.time.monotonic = original_monotonic
    if result != 2 or "no PR checks appeared" not in output:
        raise AssertionError((result, output))


def main() -> int:
    assert_verify_remote_precondition_errors()
    assert_verify_remote_pr_errors()
    assert_pr_checks_allows_pending_exit_code_with_json()
    assert_verify_remote_waits_then_passes()
    assert_verify_remote_no_checks_times_out()
    print("OK: remote verification watcher self-tests passed.")
    return 0


if __name__ == "__main__":
    sys.exit(main())
