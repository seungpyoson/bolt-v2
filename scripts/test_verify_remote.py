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
            refused_cargo_subcommands = ["b", "bench", "build", "c", "check", "clippy", "d", "doc", "fetch", "install", "nextest", "r", "run", "rustc", "t", "test", "zigbuild"]

            [local_lane_policy]
            enabled = true
            allowed_ci_env = "GITHUB_ACTIONS"
            lock_dir = "/tmp/rust-verification-lanes"
            acquire_timeout_seconds = 1800
            heartbeat_seconds = 15
            poll_interval_seconds = 1

            [remote_verification]
            poll_interval_seconds = 1
            checks_appear_timeout_seconds = {checks_timeout}
            overall_timeout_seconds = {overall_timeout}
            diagnostic_log_max_lines = 160
            diagnostic_log_max_bytes = 20000
            diagnostic_unavailable_notice_interval_polls = 4

            [sandbox_safe_push]
            remote = "origin"

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
    (repo / "ci" / "github-actions-runners.toml").write_text(
        textwrap.dedent(
            """\
            [ci_provenance]
            workflow_name = "CI"
            workflow_path = ".github/workflows/ci.yml"

            [ci_provenance.dispatch]
            run_name_iteration = "CI [dispatch:iteration]"
            proof_gate_job = "gate"

            [ci_provenance.gate_names]
            gate_required = "gate"
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


def assert_verify_remote_dispatch_config_rejects_unsafe_gate_names() -> None:
    owner = load_owner_module()
    with tempfile.TemporaryDirectory() as tmp:
        repo = pathlib.Path(tmp) / "repo"
        repo.mkdir()
        write_policy(repo)
        runners = repo / "ci" / "github-actions-runners.toml"
        runners.write_text(
            runners.read_text(encoding="utf-8").replace(
                'gate_required = "gate"',
                'gate_required = "gate "',
            ),
            encoding="utf-8",
        )
        config, error = owner.ci_provenance_dispatch_config(repo)
    if config is not None:
        raise AssertionError(config)
    expected = "ci_provenance.gate_names.gate_required must be a GitHub Actions output-safe check name"
    if error != expected:
        raise AssertionError(error)


def assert_diagnostic_excerpt_is_bounded_and_masked() -> None:
    owner = load_owner_module()
    text = (
        "\x1b[31mline0\x1b[0m\n"
        "TOKEN=abc123\n"
        "password: secret-value\n"
        "PASSWORD: correct horse battery staple\n"
        "API_KEY = api-secret\n"
        "PRIVATE_KEY=private-key-value\n"
        "WALLET_KEY=wallet-key-value\n"
        "SIGNING_KEY=signing-key-value\n"
        "SEED_PHRASE=seed phrase words\n"
        "PASSPHRASE: pass phrase value\n"
        "CREDENTIAL=credential value\n"
        "MNEMONIC: correct horse battery staple\n"
        "AWS_SECRET_ACCESS_KEY=awssecret\n"
        "Authorization: Bearer secretvalue\n"
        "line3\n"
        "line4\n"
    )
    excerpt = owner.diagnostic_log_excerpt(
        text,
        max_lines=20,
        max_bytes=1000,
    )
    if "\x1b" in excerpt:
        raise AssertionError(excerpt)
    secrets = (
        "abc123",
        "secret-value",
        "correct horse battery staple",
        "api-secret",
        "private-key-value",
        "wallet-key-value",
        "signing-key-value",
        "seed phrase words",
        "pass phrase value",
        "credential value",
        "awssecret",
        "secretvalue",
    )
    if any(secret in excerpt for secret in secrets):
        raise AssertionError(excerpt)
    if len(excerpt.splitlines()) > 20:
        raise AssertionError(excerpt)
    if "TOKEN=<redacted>" not in excerpt:
        raise AssertionError(excerpt)
    if "password: <redacted>" not in excerpt:
        raise AssertionError(excerpt)
    if "horse battery staple" in excerpt:
        raise AssertionError(excerpt)
    if "API_KEY = <redacted>" not in excerpt:
        raise AssertionError(excerpt)
    if "PRIVATE_KEY=<redacted>" not in excerpt:
        raise AssertionError(excerpt)
    if "WALLET_KEY=<redacted>" not in excerpt:
        raise AssertionError(excerpt)
    if "SIGNING_KEY=<redacted>" not in excerpt:
        raise AssertionError(excerpt)
    if "SEED_PHRASE=<redacted>" not in excerpt:
        raise AssertionError(excerpt)
    if "PASSPHRASE: <redacted>" not in excerpt:
        raise AssertionError(excerpt)
    if "CREDENTIAL=<redacted>" not in excerpt:
        raise AssertionError(excerpt)
    if "MNEMONIC: <redacted>" not in excerpt:
        raise AssertionError(excerpt)
    byte_capped = owner.diagnostic_log_excerpt(
        "prefix\n" + ("x" * 300),
        max_lines=10,
        max_bytes=40,
    )
    if len(byte_capped.encode("utf-8")) > 40:
        raise AssertionError(byte_capped)
    ansi_stripped = owner.diagnostic_log_excerpt(
        "\x1b[31mansi-visible\x1b[0m\nplain",
        max_lines=10,
        max_bytes=200,
    )
    if "\x1b" in ansi_stripped or "ansi-visible" not in ansi_stripped:
        raise AssertionError(ansi_stripped)


def assert_secret_redaction_leaves_common_key_labels_readable() -> None:
    owner = load_owner_module()
    text = (
        "primary key: id\n"
        "FOREIGN_KEY=orders.id\n"
        "cache_key=account:42\n"
        "sort_key: ascending\n"
        "key: value mapping\n"
        "PUBLIC_KEY=ssh-rsa AAAA\n"
        "SEEDED=true\n"
    )
    excerpt = owner.diagnostic_log_excerpt(text, max_lines=20, max_bytes=1000)
    for expected in text.strip().splitlines():
        if expected not in excerpt:
            raise AssertionError(excerpt)
    if "<redacted>" in excerpt:
        raise AssertionError(excerpt)


def assert_job_log_failed_treats_ansi_whitespace_as_unavailable() -> None:
    owner = load_owner_module()
    original_run_capture = owner.run_capture
    try:
        owner.run_capture = lambda _argv, repo: types.SimpleNamespace(returncode=0, stdout="\x1b[0m  \n", stderr="")
        log_text, error = owner.job_log_failed(pathlib.Path("."), 123)
    finally:
        owner.run_capture = original_run_capture
    if log_text is not None:
        raise AssertionError(log_text)
    if error != "failed job log is not available yet":
        raise AssertionError(error)


def assert_run_attempt_accepts_positive_ints_only() -> None:
    owner = load_owner_module()
    if owner.run_attempt({"attempt": 2}) != 2:
        raise AssertionError("integer attempt rejected")
    if owner.run_attempt({"attempt": "3"}) != 3:
        raise AssertionError("string attempt rejected")
    if owner.run_attempt({"attempt": True}) is not None:
        raise AssertionError("boolean attempt accepted")
    if owner.run_attempt({"attempt": 0}) is not None:
        raise AssertionError("zero attempt accepted")
    if owner.run_attempt({"attempt": -1}) is not None:
        raise AssertionError("negative attempt accepted")
    if owner.run_attempt({"attempt": "1.0"}) is not None:
        raise AssertionError("non-decimal attempt accepted")


def assert_job_database_id_accepts_numeric_database_id_or_id() -> None:
    owner = load_owner_module()
    if owner.job_database_id({"databaseId": 11}) != 11:
        raise AssertionError("integer databaseId rejected")
    if owner.job_database_id({"databaseId": "12"}) != 12:
        raise AssertionError("string databaseId rejected")
    if owner.job_database_id({"id": 13}) != 13:
        raise AssertionError("integer id fallback rejected")
    if owner.job_database_id({"id": "14"}) != 14:
        raise AssertionError("string id fallback rejected")
    if owner.job_database_id({"databaseId": True}) is not None:
        raise AssertionError("boolean databaseId accepted")
    if owner.job_database_id({"id": True}) is not None:
        raise AssertionError("boolean id accepted")
    if owner.job_database_id({"id": "-15"}) is not None:
        raise AssertionError("negative string id accepted")


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
                    ("branch", "--show-current"): ("feature", None),
                    ("config", "branch.feature.remote"): ("origin", None),
                    ("config", "branch.feature.merge"): ("refs/heads/feature", None),
                    ("ls-remote", "--heads", "origin", "feature"): (
                        "def\trefs/heads/feature",
                        None,
                    ),
                }[args],
                "HEAD to be pushed",
            ),
            (
                lambda _repo, *args: {
                    ("status", "--porcelain", "--untracked-files=normal"): ("", None),
                    ("rev-parse", "HEAD"): ("abc", None),
                    ("branch", "--show-current"): ("feature", None),
                    ("config", "branch.feature.remote"): (None, "no upstream"),
                    ("remote",): ("origin", None),
                    ("remote", "get-url", "--push", "--all", "origin"): ("https://example.invalid/push.git", None),
                    ("ls-remote", "--heads", "origin", "feature"): ("", None),
                    ("ls-remote", "--heads", "https://example.invalid/push.git", "feature"): ("", None),
                }[args],
                "just sandbox-safe-push",
            ),
            (
                lambda _repo, *args: {
                    ("status", "--porcelain", "--untracked-files=normal"): ("", None),
                    ("rev-parse", "HEAD"): ("abc", None),
                    ("branch", "--show-current"): ("feature", None),
                    ("config", "branch.feature.remote"): (None, "no upstream"),
                    ("remote",): ("fork\nupstream", None),
                }[args],
                "sandbox_safe_push.remote origin is not among configured Git remotes",
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


def assert_remote_fallback_helpers_handle_empty_outputs() -> None:
    owner = load_owner_module()
    original_git_output = owner.git_output
    try:
        owner.git_output = lambda _repo, *args: {
            ("remote",): (None, None),
        }[args]
        remote, error = owner.fallback_push_remote(REPO_ROOT, command_name="verify-remote")
        if remote is not None or error != "verify-remote requires a configured Git remote":
            raise AssertionError((remote, error))

        owner.git_output = lambda _repo, *args: {
            ("remote",): ("", None),
        }[args]
        remote, error = owner.fallback_push_remote(REPO_ROOT, command_name="verify-remote")
        if remote is not None or error != "verify-remote requires a configured Git remote":
            raise AssertionError((remote, error))

        owner.git_output = lambda _repo, *args: {
            ("ls-remote", "--heads", "origin", "feature"): (None, None),
        }[args]
        head, error = owner.live_remote_branch_head(REPO_ROOT, remote="origin", branch="feature")
        if head is not None or error is not None:
            raise AssertionError((head, error))

        owner.git_output = lambda _repo, *args: {
            ("ls-remote", "--heads", "origin", "feature"): ("", None),
        }[args]
        head, error = owner.live_remote_branch_head(REPO_ROOT, remote="origin", branch="feature")
        if head is not None or error is not None:
            raise AssertionError((head, error))
    finally:
        owner.git_output = original_git_output


def assert_verify_remote_accepts_same_name_remote_without_local_upstream() -> None:
    owner = load_owner_module()
    calls: list[tuple[str, ...]] = []

    def fake_git_output(_repo: pathlib.Path, *args: str) -> tuple[str | None, str | None]:
        calls.append(args)
        return {
            ("status", "--porcelain", "--untracked-files=normal"): ("", None),
            ("rev-parse", "HEAD"): ("abc", None),
            ("branch", "--show-current"): ("feature", None),
            ("config", "branch.feature.remote"): (None, "no upstream"),
            ("remote",): ("fork\norigin", None),
            ("remote", "get-url", "--push", "--all", "origin"): ("https://example.invalid/push.git", None),
            ("ls-remote", "--heads", "https://example.invalid/push.git", "feature"): (
                "abc\trefs/heads/feature",
                None,
            ),
        }[args]

    original_git_output = owner.git_output
    try:
        owner.git_output = fake_git_output
        head, branch, error = owner.ensure_verify_remote_preconditions(REPO_ROOT)
    finally:
        owner.git_output = original_git_output

    if (head, branch, error) != ("abc", "feature", None):
        raise AssertionError((head, branch, error))
    if ("ls-remote", "--heads", "https://example.invalid/push.git", "feature") not in calls:
        raise AssertionError(calls)


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


def assert_pr_lookup_preserves_gh_errors() -> None:
    owner = load_owner_module()
    original_load_json_command = owner.load_json_command
    try:
        owner.load_json_command = lambda _argv, repo: (None, 'no pull requests found for branch "feature"')
        _pr, no_pr_error = owner.pr_for_current_branch(REPO_ROOT, "feature")
        if no_pr_error is None or "gh pr create --draft" not in no_pr_error:
            raise AssertionError(no_pr_error)

        owner.load_json_command = lambda _argv, repo: (None, "HTTP 401: Bad credentials")
        _pr, auth_error = owner.pr_for_current_branch(REPO_ROOT, "feature")
        if auth_error is None or "Bad credentials" not in auth_error or "gh pr create --draft" in auth_error:
            raise AssertionError(auth_error)

        owner.load_json_command = lambda _argv, repo: (None, "gh is required for remote verification")
        _pr, missing_gh_error = owner.pr_for_current_branch(REPO_ROOT, "feature")
        if missing_gh_error is None or "gh is required" not in missing_gh_error:
            raise AssertionError(missing_gh_error)

        owner.load_json_command = lambda _argv, repo: ({"headRefOid": "abc", "state": "MERGED"}, None)
        _pr, merged_error = owner.pr_for_current_branch(REPO_ROOT, "feature")
        if merged_error is None or "stale branch" not in merged_error:
            raise AssertionError(merged_error)
    finally:
        owner.load_json_command = original_load_json_command


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
        original_run_list = owner.workflow_run_list
        original_run_view = owner.workflow_run_view
        original_jobs = owner.workflow_run_jobs
        original_sleep = owner.time.sleep
        try:
            owner.ensure_verify_remote_preconditions = lambda _repo: ("abc", "feature", None)
            owner.pr_for_current_branch = lambda _repo, _branch: (
                {"headRefOid": "abc", "url": "https://example.invalid/pr/1", "number": 1, "state": "OPEN", "isDraft": False},
                None,
            )
            calls = iter(
                [
                    (
                        [
                            {
                                "databaseId": 101,
                                "attempt": 1,
                                "event": "pull_request",
                                "headSha": "abc",
                                "status": "in_progress",
                                "conclusion": None,
                                "createdAt": "2026-06-13T00:00:00Z",
                                "url": "https://example.invalid/run",
                            }
                        ],
                        None,
                    ),
                    (
                        [
                            {
                                "databaseId": 101,
                                "attempt": 1,
                                "event": "pull_request",
                                "headSha": "abc",
                                "status": "completed",
                                "conclusion": "success",
                                "createdAt": "2026-06-13T00:00:00Z",
                                "url": "https://example.invalid/run",
                            }
                        ],
                        None,
                    ),
                ]
            )
            owner.workflow_run_list = lambda _repo, _dispatch_config, _branch: next(calls)
            owner.workflow_run_view = lambda _repo, _run_id: (
                {
                    "databaseId": 101,
                    "attempt": 1,
                    "event": "pull_request",
                    "headSha": "abc",
                    "status": "completed",
                    "conclusion": "success",
                    "createdAt": "2026-06-13T00:00:00Z",
                    "url": "https://example.invalid/run",
                },
                None,
            )
            owner.workflow_run_jobs = lambda _repo, _run_id, _attempt: (
                [{"name": "gate", "status": "completed", "conclusion": "success"}],
                None,
            )
            owner.time.sleep = lambda _seconds: None
            result, output = run_cmd_verify_remote(owner, repo)
        finally:
            owner.ensure_verify_remote_preconditions = original_preconditions
            owner.pr_for_current_branch = original_pr
            owner.workflow_run_list = original_run_list
            owner.workflow_run_view = original_run_view
            owner.workflow_run_jobs = original_jobs
            owner.time.sleep = original_sleep
    if result != 0 or "OK: remote full CI passed" not in output:
        raise AssertionError((result, output))


def assert_verify_remote_uses_latest_full_run_over_stale_deferred_run() -> None:
    owner = load_owner_module()
    with tempfile.TemporaryDirectory() as tmp:
        repo = pathlib.Path(tmp) / "repo"
        repo.mkdir()
        write_policy(repo)
        original_preconditions = owner.ensure_verify_remote_preconditions
        original_pr = owner.pr_for_current_branch
        original_run_list = owner.workflow_run_list
        original_jobs = owner.workflow_run_jobs
        try:
            owner.ensure_verify_remote_preconditions = lambda _repo: ("abc", "feature", None)
            owner.pr_for_current_branch = lambda _repo, _branch: (
                {"headRefOid": "abc", "url": "https://example.invalid/pr/1", "number": 1, "state": "OPEN", "isDraft": False},
                None,
            )
            owner.workflow_run_list = lambda _repo, _dispatch_config, _branch: (
                [
                    {
                        "databaseId": 201,
                        "attempt": 1,
                        "event": "pull_request",
                        "headSha": "abc",
                        "status": "completed",
                        "conclusion": "failure",
                        "createdAt": "2026-06-13T00:00:00Z",
                        "url": "https://example.invalid/stale-deferred",
                    },
                    {
                        "databaseId": 202,
                        "attempt": 1,
                        "event": "pull_request",
                        "headSha": "abc",
                        "status": "completed",
                        "conclusion": "success",
                        "createdAt": "2026-06-13T00:01:00Z",
                        "url": "https://example.invalid/full-ci",
                    },
                ],
                None,
            )
            owner.workflow_run_jobs = lambda _repo, _run_id, _attempt: (
                [{"name": "gate", "status": "completed", "conclusion": "success"}],
                None,
            )
            result, output = run_cmd_verify_remote(owner, repo)
        finally:
            owner.ensure_verify_remote_preconditions = original_preconditions
            owner.pr_for_current_branch = original_pr
            owner.workflow_run_list = original_run_list
            owner.workflow_run_jobs = original_jobs
    if result != 0 or "full-ci" not in output:
        raise AssertionError((result, output))


def assert_verify_remote_ready_pr_requires_required_gate_job() -> None:
    owner = load_owner_module()
    with tempfile.TemporaryDirectory() as tmp:
        repo = pathlib.Path(tmp) / "repo"
        repo.mkdir()
        write_policy(repo)
        original_preconditions = owner.ensure_verify_remote_preconditions
        original_pr = owner.pr_for_current_branch
        original_run_list = owner.workflow_run_list
        original_jobs = owner.workflow_run_jobs
        try:
            owner.ensure_verify_remote_preconditions = lambda _repo: ("abc", "feature", None)
            owner.pr_for_current_branch = lambda _repo, _branch: (
                {"headRefOid": "abc", "url": "https://example.invalid/pr/1", "number": 1, "state": "OPEN", "isDraft": False},
                None,
            )
            owner.workflow_run_list = lambda _repo, _dispatch_config, _branch: (
                [
                    {
                        "databaseId": 203,
                        "attempt": 1,
                        "event": "pull_request",
                        "headSha": "abc",
                        "status": "completed",
                        "conclusion": "success",
                        "createdAt": "2026-06-13T00:02:00Z",
                        "url": "https://example.invalid/noop",
                    }
                ],
                None,
            )
            owner.workflow_run_jobs = lambda _repo, _run_id, _attempt: (
                [{"name": "gate-noop", "status": "completed", "conclusion": "success"}],
                None,
            )
            result, output = run_cmd_verify_remote(owner, repo)
        finally:
            owner.ensure_verify_remote_preconditions = original_preconditions
            owner.pr_for_current_branch = original_pr
            owner.workflow_run_list = original_run_list
            owner.workflow_run_jobs = original_jobs
    if result != 1 or "pull_request run lacks successful required gate job" not in output:
        raise AssertionError((result, output))


def assert_verify_remote_rejects_unknown_success_event() -> None:
    owner = load_owner_module()
    with tempfile.TemporaryDirectory() as tmp:
        repo = pathlib.Path(tmp) / "repo"
        repo.mkdir()
        stderr = io.StringIO()
        stdout = io.StringIO()
        with contextlib.redirect_stdout(stdout), contextlib.redirect_stderr(stderr):
            result = owner.evaluate_full_ci_run(
                repo,
                {
                    "databaseId": 204,
                    "attempt": 1,
                    "event": "merge_group",
                    "headSha": "abc",
                    "status": "completed",
                    "conclusion": "success",
                    "createdAt": "2026-06-13T00:02:00Z",
                    "url": "https://example.invalid/merge-group",
                },
                dispatch_config={
                    "proof_gate_job": "gate",
                },
                head="abc",
                pr_url="https://example.invalid/pr/1",
            )
    output = stdout.getvalue() + stderr.getvalue()
    if result != 1 or "unsupported workflow event 'merge_group'" not in output:
        raise AssertionError((result, output))


def assert_verify_remote_draft_pr_rejects_manual_full_ci() -> None:
    owner = load_owner_module()
    with tempfile.TemporaryDirectory() as tmp:
        repo = pathlib.Path(tmp) / "repo"
        repo.mkdir()
        write_policy(repo)
        original_preconditions = owner.ensure_verify_remote_preconditions
        original_pr = owner.pr_for_current_branch
        try:
            owner.ensure_verify_remote_preconditions = lambda _repo: ("abc", "feature", None)
            owner.pr_for_current_branch = lambda _repo, _branch: (
                {"headRefOid": "abc", "url": "https://example.invalid/pr/1", "number": 1, "state": "OPEN", "isDraft": True},
                None,
            )
            result, output = run_cmd_verify_remote(owner, repo)
        finally:
            owner.ensure_verify_remote_preconditions = original_preconditions
            owner.pr_for_current_branch = original_pr
    if result != 2 or "draft PRs cannot run full CI through workflow_dispatch" not in output:
        raise AssertionError((result, output))
    if "just rust-probe" not in output:
        raise AssertionError((result, output))


def assert_verify_remote_rejects_branch_advance_during_watch() -> None:
    owner = load_owner_module()
    with tempfile.TemporaryDirectory() as tmp:
        repo = pathlib.Path(tmp) / "repo"
        repo.mkdir()
        write_policy(repo)
        original_preconditions = owner.ensure_verify_remote_preconditions
        original_pr = owner.pr_for_current_branch
        original_run_list = owner.workflow_run_list
        original_sleep = owner.time.sleep
        try:
            owner.ensure_verify_remote_preconditions = lambda _repo: ("abc", "feature", None)
            pr_calls = iter(
                [
                    ({"headRefOid": "abc", "url": "https://example.invalid/pr/1", "number": 1, "state": "OPEN", "isDraft": False}, None),
                    ({"headRefOid": "def", "url": "https://example.invalid/pr/1", "number": 1, "state": "OPEN", "isDraft": False}, None),
                ]
            )
            owner.pr_for_current_branch = lambda _repo, _branch: next(pr_calls)
            owner.workflow_run_list = lambda _repo, _dispatch_config, _branch: (
                [
                    {
                        "databaseId": 301,
                        "event": "pull_request",
                        "headSha": "abc",
                        "status": "completed",
                        "conclusion": "success",
                        "createdAt": "2026-06-13T00:00:00Z",
                    }
                ],
                None,
            )
            owner.time.sleep = lambda _seconds: None
            result, output = run_cmd_verify_remote(owner, repo)
        finally:
            owner.ensure_verify_remote_preconditions = original_preconditions
            owner.pr_for_current_branch = original_pr
            owner.workflow_run_list = original_run_list
            owner.time.sleep = original_sleep
    if result != 2 or "advanced during watch" not in output:
        raise AssertionError((result, output))


def assert_verify_remote_reports_failing_full_ci_run() -> None:
    owner = load_owner_module()
    with tempfile.TemporaryDirectory() as tmp:
        repo = pathlib.Path(tmp) / "repo"
        repo.mkdir()
        write_policy(repo)
        original_preconditions = owner.ensure_verify_remote_preconditions
        original_pr = owner.pr_for_current_branch
        original_run_list = owner.workflow_run_list
        original_sleep = owner.time.sleep
        try:
            owner.ensure_verify_remote_preconditions = lambda _repo: ("abc", "feature", None)
            owner.pr_for_current_branch = lambda _repo, _branch: (
                {"headRefOid": "abc", "url": "https://example.invalid/pr/1", "number": 1, "state": "OPEN", "isDraft": False},
                None,
            )
            calls = iter(
                [
                    (
                        [
                            {
                                "databaseId": 400,
                                "event": "pull_request",
                                "headSha": "abc",
                                "status": "completed",
                                "conclusion": "failure",
                                "createdAt": "2026-06-13T00:00:00Z",
                                "url": "https://example.invalid/stale-deferred",
                            }
                        ],
                        None,
                    ),
                    (
                        [
                            {
                                "databaseId": 400,
                                "event": "pull_request",
                                "headSha": "abc",
                                "status": "completed",
                                "conclusion": "failure",
                                "createdAt": "2026-06-13T00:00:00Z",
                                "url": "https://example.invalid/stale-deferred",
                            },
                            {
                                "databaseId": 401,
                                "event": "pull_request",
                                "headSha": "abc",
                                "status": "completed",
                                "conclusion": "failure",
                                "createdAt": "2026-06-13T00:01:00Z",
                                "url": "https://example.invalid/run",
                            },
                        ],
                        None,
                    ),
                ]
            )
            owner.workflow_run_list = lambda _repo, _dispatch_config, _branch: next(calls)
            owner.time.sleep = lambda _seconds: None
            result, output = run_cmd_verify_remote(owner, repo)
        finally:
            owner.ensure_verify_remote_preconditions = original_preconditions
            owner.pr_for_current_branch = original_pr
            owner.workflow_run_list = original_run_list
            owner.time.sleep = original_sleep
    if result != 1 or "Remote full CI failed" not in output or "workflow run 401" not in output:
        raise AssertionError((result, output))


def assert_verify_remote_rechecks_head_before_reporting_failed_run() -> None:
    owner = load_owner_module()
    with tempfile.TemporaryDirectory() as tmp:
        repo = pathlib.Path(tmp) / "repo"
        repo.mkdir()
        write_policy(repo)
        original_preconditions = owner.ensure_verify_remote_preconditions
        original_pr = owner.pr_for_current_branch
        original_run_list = owner.workflow_run_list
        original_sleep = owner.time.sleep
        try:
            owner.ensure_verify_remote_preconditions = lambda _repo: ("abc", "feature", None)
            pr_calls = iter(
                [
                    ({"headRefOid": "abc", "url": "https://example.invalid/pr/1", "number": 1, "state": "OPEN", "isDraft": False}, None),
                    ({"headRefOid": "abc", "url": "https://example.invalid/pr/1", "number": 1, "state": "OPEN", "isDraft": False}, None),
                    ({"headRefOid": "def", "url": "https://example.invalid/pr/1", "number": 1, "state": "OPEN", "isDraft": False}, None),
                ]
            )
            owner.pr_for_current_branch = lambda _repo, _branch: next(pr_calls)
            calls = iter(
                [
                    (
                        [
                            {
                                "databaseId": 500,
                                "event": "pull_request",
                                "headSha": "abc",
                                "status": "completed",
                                "conclusion": "failure",
                                "createdAt": "2026-06-13T00:00:00Z",
                                "url": "https://example.invalid/stale-deferred",
                            }
                        ],
                        None,
                    ),
                    (
                        [
                            {
                                "databaseId": 500,
                                "event": "pull_request",
                                "headSha": "abc",
                                "status": "completed",
                                "conclusion": "failure",
                                "createdAt": "2026-06-13T00:00:00Z",
                                "url": "https://example.invalid/stale-deferred",
                            },
                            {
                                "databaseId": 501,
                                "event": "pull_request",
                                "headSha": "abc",
                                "status": "completed",
                                "conclusion": "failure",
                                "createdAt": "2026-06-13T00:01:00Z",
                                "url": "https://example.invalid/run",
                            },
                        ],
                        None,
                    ),
                ]
            )
            owner.workflow_run_list = lambda _repo, _dispatch_config, _branch: next(calls)
            owner.time.sleep = lambda _seconds: None
            result, output = run_cmd_verify_remote(owner, repo)
        finally:
            owner.ensure_verify_remote_preconditions = original_preconditions
            owner.pr_for_current_branch = original_pr
            owner.workflow_run_list = original_run_list
            owner.time.sleep = original_sleep
    if result != 2 or "advanced during watch" not in output or "Remote full CI failed" in output:
        raise AssertionError((result, output))


def assert_verify_remote_rechecks_head_before_failed_job_diagnostics() -> None:
    owner = load_owner_module()
    with tempfile.TemporaryDirectory() as tmp:
        repo = pathlib.Path(tmp) / "repo"
        repo.mkdir()
        write_policy(repo)

        original_preconditions = owner.ensure_verify_remote_preconditions
        original_pr = owner.pr_for_current_branch
        original_run_list = owner.workflow_run_list
        original_run_view = owner.workflow_run_view
        original_emit = owner.emit_failed_job_diagnostics
        original_sleep = owner.time.sleep
        emitted: list[int] = []

        try:
            owner.ensure_verify_remote_preconditions = lambda _repo: ("abc", "feature", None)
            pr_calls = iter(
                [
                    (
                        {
                            "headRefOid": "abc",
                            "url": "https://example.invalid/pr/1",
                            "number": 1,
                            "state": "OPEN",
                            "isDraft": False,
                        },
                        None,
                    ),
                    (
                        {
                            "headRefOid": "abc",
                            "url": "https://example.invalid/pr/1",
                            "number": 1,
                            "state": "OPEN",
                            "isDraft": False,
                        },
                        None,
                    ),
                    (
                        {
                            "headRefOid": "def",
                            "url": "https://example.invalid/pr/1",
                            "number": 1,
                            "state": "OPEN",
                            "isDraft": False,
                        },
                        None,
                    ),
                ]
            )
            owner.pr_for_current_branch = lambda _repo, _branch: next(pr_calls)
            pending_run = {
                "databaseId": 701,
                "attempt": 1,
                "event": "pull_request",
                "headSha": "abc",
                "status": "in_progress",
                "conclusion": None,
                "createdAt": "2026-06-13T00:00:00Z",
                "url": "https://example.invalid/run",
            }
            owner.workflow_run_list = lambda _repo, _dispatch_config, _branch: ([pending_run], None)
            owner.workflow_run_view = lambda _repo, _run_id: (pending_run, None)
            owner.emit_failed_job_diagnostics = lambda **kwargs: emitted.append(int(kwargs["run"]["databaseId"]))
            owner.time.sleep = lambda _seconds: None

            result, output = run_cmd_verify_remote(owner, repo)
        finally:
            owner.ensure_verify_remote_preconditions = original_preconditions
            owner.pr_for_current_branch = original_pr
            owner.workflow_run_list = original_run_list
            owner.workflow_run_view = original_run_view
            owner.emit_failed_job_diagnostics = original_emit
            owner.time.sleep = original_sleep

    if result != 2 or "advanced during watch" not in output:
        raise AssertionError((result, output))
    if emitted:
        raise AssertionError(emitted)


def assert_verify_remote_run_list_api_error_fails_closed() -> None:
    owner = load_owner_module()
    with tempfile.TemporaryDirectory() as tmp:
        repo = pathlib.Path(tmp) / "repo"
        repo.mkdir()
        write_policy(repo, checks_timeout=1, overall_timeout=3)
        original_preconditions = owner.ensure_verify_remote_preconditions
        original_pr = owner.pr_for_current_branch
        original_run_list = owner.workflow_run_list
        original_sleep = owner.time.sleep
        try:
            owner.ensure_verify_remote_preconditions = lambda _repo: ("abc", "feature", None)
            owner.pr_for_current_branch = lambda _repo, _branch: (
                {"headRefOid": "abc", "url": "https://example.invalid/pr/1", "number": 1, "state": "OPEN", "isDraft": False},
                None,
            )
            owner.workflow_run_list = lambda _repo, _dispatch_config, _branch: (None, "API rate limit exceeded")
            owner.time.sleep = lambda _seconds: None
            result, output = run_cmd_verify_remote(owner, repo)
        finally:
            owner.ensure_verify_remote_preconditions = original_preconditions
            owner.pr_for_current_branch = original_pr
            owner.workflow_run_list = original_run_list
            owner.time.sleep = original_sleep
    if result != 2 or "API rate limit exceeded" not in output:
        raise AssertionError((result, output))


def assert_verify_remote_no_matching_run_times_out() -> None:
    owner = load_owner_module()
    with tempfile.TemporaryDirectory() as tmp:
        repo = pathlib.Path(tmp) / "repo"
        repo.mkdir()
        write_policy(repo, checks_timeout=1, overall_timeout=3)
        original_preconditions = owner.ensure_verify_remote_preconditions
        original_pr = owner.pr_for_current_branch
        original_run_list = owner.workflow_run_list
        original_sleep = owner.time.sleep
        original_monotonic = owner.time.monotonic
        try:
            owner.ensure_verify_remote_preconditions = lambda _repo: ("abc", "feature", None)
            owner.pr_for_current_branch = lambda _repo, _branch: (
                {"headRefOid": "abc", "url": "https://example.invalid/pr/1", "number": 1, "state": "OPEN", "isDraft": False},
                None,
            )
            owner.workflow_run_list = lambda _repo, _dispatch_config, _branch: ([], None)
            owner.time.sleep = lambda _seconds: None
            current_time = 0.0

            def mock_monotonic() -> float:
                nonlocal current_time
                current_time += 1.0
                return current_time

            owner.time.monotonic = mock_monotonic
            result, output = run_cmd_verify_remote(owner, repo)
        finally:
            owner.ensure_verify_remote_preconditions = original_preconditions
            owner.pr_for_current_branch = original_pr
            owner.workflow_run_list = original_run_list
            owner.time.sleep = original_sleep
            owner.time.monotonic = original_monotonic
    if result != 2 or "no matching full-CI workflow run appeared" not in output:
        raise AssertionError((result, output))


def assert_verify_remote_rechecks_head_before_no_matching_run_timeout() -> None:
    owner = load_owner_module()
    with tempfile.TemporaryDirectory() as tmp:
        repo = pathlib.Path(tmp) / "repo"
        repo.mkdir()
        write_policy(repo, checks_timeout=1, overall_timeout=5)
        original_preconditions = owner.ensure_verify_remote_preconditions
        original_pr = owner.pr_for_current_branch
        original_run_list = owner.workflow_run_list
        original_sleep = owner.time.sleep
        original_monotonic = owner.time.monotonic
        try:
            owner.ensure_verify_remote_preconditions = lambda _repo: ("abc", "feature", None)
            pr_calls = iter(
                [
                    ({"headRefOid": "abc", "url": "https://example.invalid/pr/1", "number": 1, "state": "OPEN", "isDraft": False}, None),
                    ({"headRefOid": "def", "url": "https://example.invalid/pr/1", "number": 1, "state": "OPEN", "isDraft": False}, None),
                ]
            )
            owner.pr_for_current_branch = lambda _repo, _branch: next(pr_calls)
            owner.workflow_run_list = lambda _repo, _dispatch_config, _branch: ([], None)
            owner.time.sleep = lambda _seconds: None
            current_time = 0.0

            def mock_monotonic() -> float:
                nonlocal current_time
                current_time += 1.0
                return current_time

            owner.time.monotonic = mock_monotonic
            result, output = run_cmd_verify_remote(owner, repo)
        finally:
            owner.ensure_verify_remote_preconditions = original_preconditions
            owner.pr_for_current_branch = original_pr
            owner.workflow_run_list = original_run_list
            owner.time.sleep = original_sleep
            owner.time.monotonic = original_monotonic
    if result != 2 or "advanced during watch" not in output or "no matching full-CI workflow run appeared" in output:
        raise AssertionError((result, output))


def assert_verify_remote_rechecks_head_before_overall_timeout() -> None:
    owner = load_owner_module()
    with tempfile.TemporaryDirectory() as tmp:
        repo = pathlib.Path(tmp) / "repo"
        repo.mkdir()
        write_policy(repo, checks_timeout=1, overall_timeout=2)
        original_preconditions = owner.ensure_verify_remote_preconditions
        original_pr = owner.pr_for_current_branch
        original_run_list = owner.workflow_run_list
        original_run_view = owner.workflow_run_view
        original_sleep = owner.time.sleep
        original_monotonic = owner.time.monotonic
        try:
            owner.ensure_verify_remote_preconditions = lambda _repo: ("abc", "feature", None)
            pr_calls = iter(
                [
                    ({"headRefOid": "abc", "url": "https://example.invalid/pr/1", "number": 1, "state": "OPEN", "isDraft": False}, None),
                    ({"headRefOid": "abc", "url": "https://example.invalid/pr/1", "number": 1, "state": "OPEN", "isDraft": False}, None),
                    ({"headRefOid": "def", "url": "https://example.invalid/pr/1", "number": 1, "state": "OPEN", "isDraft": False}, None),
                ]
            )
            owner.pr_for_current_branch = lambda _repo, _branch: next(pr_calls)
            owner.workflow_run_list = lambda _repo, _dispatch_config, _branch: (
                [
                    {
                        "databaseId": 601,
                        "event": "pull_request",
                        "headSha": "abc",
                        "status": "in_progress",
                        "conclusion": None,
                        "createdAt": "2026-06-13T00:00:00Z",
                    }
                ],
                None,
            )
            owner.workflow_run_view = lambda _repo, _run_id: (
                {
                    "databaseId": 601,
                    "event": "pull_request",
                    "headSha": "abc",
                    "status": "in_progress",
                    "conclusion": None,
                    "createdAt": "2026-06-13T00:00:00Z",
                },
                None,
            )
            owner.time.sleep = lambda _seconds: None
            current_time = 0.0

            def mock_monotonic() -> float:
                nonlocal current_time
                current_time += 1.0
                return current_time

            owner.time.monotonic = mock_monotonic
            result, output = run_cmd_verify_remote(owner, repo)
        finally:
            owner.ensure_verify_remote_preconditions = original_preconditions
            owner.pr_for_current_branch = original_pr
            owner.workflow_run_list = original_run_list
            owner.workflow_run_view = original_run_view
            owner.time.sleep = original_sleep
            owner.time.monotonic = original_monotonic
    if result != 2 or "advanced during watch" not in output or "timed out waiting" in output:
        raise AssertionError((result, output))


def assert_failed_job_diagnostics_retries_unavailable_and_reports_once() -> None:
    owner = load_owner_module()
    original_jobs = owner.workflow_run_jobs
    original_log = owner.job_log_failed
    try:
        owner.workflow_run_jobs = lambda _repo, _run_id, _attempt: (
            [
                {
                    "id": 11,
                    "name": "nextest archive",
                    "status": "completed",
                    "conclusion": "failure",
                    "url": "https://example.invalid/job/11",
                }
            ],
            None,
        )
        log_results = iter(
            [
                (None, "failed job log is not available yet"),
                (None, "failed job log is not available yet"),
                ("line0\nTOKEN=abc123\npanic details\n", None),
                ("this should not print\n", None),
            ]
        )
        owner.job_log_failed = lambda _repo, _job_id: next(log_results)
        state = owner.RemoteFailureDiagnosticsState()
        policy = {
            "diagnostic_log_max_lines": 20,
            "diagnostic_log_max_bytes": 2000,
            "diagnostic_unavailable_notice_interval_polls": 3,
        }
        stderr = io.StringIO()
        with tempfile.TemporaryDirectory() as tmp, contextlib.redirect_stderr(stderr):
            repo = pathlib.Path(tmp) / "repo"
            repo.mkdir()
            for _ in range(4):
                owner.emit_failed_job_diagnostics(
                    repo=repo,
                    run={"databaseId": 101, "attempt": 1},
                    state=state,
                    remote_policy=policy,
                )
        output = stderr.getvalue()
    finally:
        owner.workflow_run_jobs = original_jobs
        owner.job_log_failed = original_log

    if output.count("CI failed job: nextest archive") != 2:
        raise AssertionError(output)
    if output.count("job_log=unavailable yet") != 1:
        raise AssertionError(output)
    if "panic details" not in output:
        raise AssertionError(output)
    if "abc123" in output or "this should not print" in output:
        raise AssertionError(output)


def assert_failed_job_diagnostics_treats_empty_log_as_unavailable() -> None:
    owner = load_owner_module()
    original_jobs = owner.workflow_run_jobs
    original_log = owner.job_log_failed
    try:
        owner.workflow_run_jobs = lambda _repo, _run_id, _attempt: (
            [
                {
                    "databaseId": 12,
                    "name": "nextest archive",
                    "status": "completed",
                    "conclusion": "failure",
                    "url": "https://example.invalid/job/12",
                }
            ],
            None,
        )
        log_results = iter([("", None), ("panic details\n", None)])
        owner.job_log_failed = lambda _repo, _job_id: next(log_results)
        state = owner.RemoteFailureDiagnosticsState()
        policy = {
            "diagnostic_log_max_lines": 20,
            "diagnostic_log_max_bytes": 2000,
            "diagnostic_unavailable_notice_interval_polls": 3,
        }
        stderr = io.StringIO()
        with tempfile.TemporaryDirectory() as tmp, contextlib.redirect_stderr(stderr):
            repo = pathlib.Path(tmp) / "repo"
            repo.mkdir()
            for _ in range(2):
                owner.emit_failed_job_diagnostics(
                    repo=repo,
                    run={"databaseId": 101, "attempt": 1},
                    state=state,
                    remote_policy=policy,
                )
        output = stderr.getvalue()
    finally:
        owner.workflow_run_jobs = original_jobs
        owner.job_log_failed = original_log

    if "job_log=unavailable yet" not in output:
        raise AssertionError(output)
    if "panic details" not in output:
        raise AssertionError(output)
    if "<empty failed job log>" in output:
        raise AssertionError(output)


def assert_failed_job_diagnostics_treats_ansi_whitespace_log_as_unavailable() -> None:
    owner = load_owner_module()
    original_jobs = owner.workflow_run_jobs
    original_log = owner.job_log_failed
    try:
        owner.workflow_run_jobs = lambda _repo, _run_id, _attempt: (
            [
                {
                    "databaseId": 13,
                    "name": "nextest archive",
                    "status": "completed",
                    "conclusion": "failure",
                    "url": "https://example.invalid/job/13",
                }
            ],
            None,
        )
        log_results = iter([("\x1b[0m  \n \x1b[31m \n", None), ("REAL_LOG_LINE\n", None)])
        owner.job_log_failed = lambda _repo, _job_id: next(log_results)
        state = owner.RemoteFailureDiagnosticsState()
        policy = {
            "diagnostic_log_max_lines": 20,
            "diagnostic_log_max_bytes": 2000,
            "diagnostic_unavailable_notice_interval_polls": 3,
        }
        stderr = io.StringIO()
        with tempfile.TemporaryDirectory() as tmp, contextlib.redirect_stderr(stderr):
            repo = pathlib.Path(tmp) / "repo"
            repo.mkdir()
            for _ in range(2):
                owner.emit_failed_job_diagnostics(
                    repo=repo,
                    run={"databaseId": 101, "attempt": 1},
                    state=state,
                    remote_policy=policy,
                )
        output = stderr.getvalue()
    finally:
        owner.workflow_run_jobs = original_jobs
        owner.job_log_failed = original_log

    if "job_log=unavailable yet" not in output:
        raise AssertionError(output)
    if "REAL_LOG_LINE" not in output:
        raise AssertionError(output)
    if "<empty failed job log>" in output:
        raise AssertionError(output)


def assert_failed_job_diagnostics_requires_run_attempt() -> None:
    owner = load_owner_module()
    original_jobs = owner.workflow_run_jobs
    try:
        def unexpected_jobs(_repo: pathlib.Path, _run_id: int, _attempt: int | None) -> tuple[list[dict[str, object]] | None, str | None]:
            raise AssertionError("workflow_run_jobs should not be called without a valid attempt")

        owner.workflow_run_jobs = unexpected_jobs
        state = owner.RemoteFailureDiagnosticsState()
        policy = {
            "diagnostic_log_max_lines": 20,
            "diagnostic_log_max_bytes": 2000,
            "diagnostic_unavailable_notice_interval_polls": 3,
        }
        stderr = io.StringIO()
        with tempfile.TemporaryDirectory() as tmp, contextlib.redirect_stderr(stderr):
            repo = pathlib.Path(tmp) / "repo"
            repo.mkdir()
            result = owner.emit_failed_job_diagnostics(
                repo=repo,
                run={"databaseId": 101},
                state=state,
                remote_policy=policy,
            )
        output = stderr.getvalue()
    finally:
        owner.workflow_run_jobs = original_jobs

    if result is not False:
        raise AssertionError(result)
    if "workflow run attempt missing" not in output:
        raise AssertionError(output)


def assert_failed_job_diagnostics_throttles_jobs_unavailable_notice() -> None:
    owner = load_owner_module()
    original_jobs = owner.workflow_run_jobs
    try:
        owner.workflow_run_jobs = lambda _repo, _run_id, _attempt: (None, "jobs unavailable")
        state = owner.RemoteFailureDiagnosticsState()
        policy = {
            "diagnostic_log_max_lines": 20,
            "diagnostic_log_max_bytes": 2000,
            "diagnostic_unavailable_notice_interval_polls": 3,
        }
        stderr = io.StringIO()
        with tempfile.TemporaryDirectory() as tmp, contextlib.redirect_stderr(stderr):
            repo = pathlib.Path(tmp) / "repo"
            repo.mkdir()
            for _ in range(5):
                owner.emit_failed_job_diagnostics(
                    repo=repo,
                    run={"databaseId": 101, "attempt": 1},
                    state=state,
                    remote_policy=policy,
                )
        output = stderr.getvalue()
    finally:
        owner.workflow_run_jobs = original_jobs

    if output.count("CI failed-job diagnostics unavailable: jobs unavailable") != 2:
        raise AssertionError(output)


def assert_verify_remote_reports_failed_job_while_run_is_in_progress() -> None:
    owner = load_owner_module()
    with tempfile.TemporaryDirectory() as tmp:
        repo = pathlib.Path(tmp) / "repo"
        repo.mkdir()
        write_policy(repo)

        original_preconditions = owner.ensure_verify_remote_preconditions
        original_pr = owner.pr_for_current_branch
        original_run_list = owner.workflow_run_list
        original_run_view = owner.workflow_run_view
        original_emit = owner.emit_failed_job_diagnostics
        original_sleep = owner.time.sleep

        emitted_states: list[str] = []

        try:
            owner.ensure_verify_remote_preconditions = lambda _repo: ("abc", "feature", None)
            owner.pr_for_current_branch = lambda _repo, _branch: (
                {
                    "headRefOid": "abc",
                    "url": "https://example.invalid/pr/1",
                    "number": 1,
                    "state": "OPEN",
                    "isDraft": False,
                },
                None,
            )
            owner.workflow_run_list = lambda _repo, _dispatch_config, _branch: (
                [
                    {
                        "databaseId": 101,
                        "attempt": 1,
                        "event": "pull_request",
                        "headSha": "abc",
                        "status": "in_progress",
                        "conclusion": None,
                        "createdAt": "2026-06-13T00:00:00Z",
                        "url": "https://example.invalid/run",
                    }
                ],
                None,
            )
            views = iter(
                [
                    {
                        "databaseId": 101,
                        "attempt": 1,
                        "event": "pull_request",
                        "headSha": "abc",
                        "status": "in_progress",
                        "conclusion": None,
                        "createdAt": "2026-06-13T00:00:00Z",
                        "url": "https://example.invalid/run",
                    },
                    {
                        "databaseId": 101,
                        "attempt": 1,
                        "event": "pull_request",
                        "headSha": "abc",
                        "status": "completed",
                        "conclusion": "failure",
                        "createdAt": "2026-06-13T00:00:00Z",
                        "url": "https://example.invalid/run",
                    },
                ]
            )
            owner.workflow_run_view = lambda _repo, _run_id: (next(views), None)

            def fake_emit_failed_job_diagnostics(*, run: dict[str, object], **_kwargs: object) -> None:
                emitted_states.append(str(run["status"]))
                print("CI failed job: nextest archive", file=sys.stderr)

            owner.emit_failed_job_diagnostics = fake_emit_failed_job_diagnostics
            owner.time.sleep = lambda _seconds: None

            result, output = run_cmd_verify_remote(owner, repo)
        finally:
            owner.ensure_verify_remote_preconditions = original_preconditions
            owner.pr_for_current_branch = original_pr
            owner.workflow_run_list = original_run_list
            owner.workflow_run_view = original_run_view
            owner.emit_failed_job_diagnostics = original_emit
            owner.time.sleep = original_sleep

    if "in_progress" not in emitted_states:
        raise AssertionError(emitted_states)
    if "CI failed job: nextest archive" not in output:
        raise AssertionError(output)
    if result != 1:
        raise AssertionError((result, output))


def main() -> int:
    assert_verify_remote_dispatch_config_rejects_unsafe_gate_names()
    assert_diagnostic_excerpt_is_bounded_and_masked()
    assert_secret_redaction_leaves_common_key_labels_readable()
    assert_job_log_failed_treats_ansi_whitespace_as_unavailable()
    assert_run_attempt_accepts_positive_ints_only()
    assert_job_database_id_accepts_numeric_database_id_or_id()
    assert_verify_remote_precondition_errors()
    assert_remote_fallback_helpers_handle_empty_outputs()
    assert_verify_remote_accepts_same_name_remote_without_local_upstream()
    assert_verify_remote_pr_errors()
    assert_pr_lookup_preserves_gh_errors()
    assert_pr_checks_allows_pending_exit_code_with_json()
    assert_verify_remote_waits_then_passes()
    assert_verify_remote_uses_latest_full_run_over_stale_deferred_run()
    assert_verify_remote_ready_pr_requires_required_gate_job()
    assert_verify_remote_rejects_unknown_success_event()
    assert_verify_remote_draft_pr_rejects_manual_full_ci()
    assert_verify_remote_rejects_branch_advance_during_watch()
    assert_verify_remote_reports_failing_full_ci_run()
    assert_verify_remote_rechecks_head_before_reporting_failed_run()
    assert_verify_remote_rechecks_head_before_failed_job_diagnostics()
    assert_verify_remote_run_list_api_error_fails_closed()
    assert_verify_remote_no_matching_run_times_out()
    assert_verify_remote_rechecks_head_before_no_matching_run_timeout()
    assert_verify_remote_rechecks_head_before_overall_timeout()
    assert_failed_job_diagnostics_retries_unavailable_and_reports_once()
    assert_failed_job_diagnostics_treats_empty_log_as_unavailable()
    assert_failed_job_diagnostics_treats_ansi_whitespace_log_as_unavailable()
    assert_failed_job_diagnostics_requires_run_attempt()
    assert_failed_job_diagnostics_throttles_jobs_unavailable_notice()
    assert_verify_remote_reports_failed_job_while_run_is_in_progress()
    print("OK: remote verification watcher self-tests passed.")
    return 0


if __name__ == "__main__":
    import lane_governor

    lane_governor.acquire()
    sys.exit(main())
