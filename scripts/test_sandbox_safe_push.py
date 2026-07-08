#!/usr/bin/env python3
"""Self-tests for sandbox-safe Git branch publishing."""

from __future__ import annotations

import importlib.util
import os
import pathlib
import subprocess
import sys
import tempfile
import textwrap
import tomllib


REPO_ROOT = pathlib.Path(__file__).resolve().parents[1]
SCRIPT = REPO_ROOT / "scripts" / "sandbox_safe_push.py"
BRANCH = "codex/sandbox-safe-push"


def load_helper_module() -> object:
    spec = importlib.util.spec_from_file_location("sandbox_safe_push_under_test", SCRIPT)
    if spec is None or spec.loader is None:
        raise AssertionError("unable to load sandbox_safe_push.py")
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


def run(command: list[str], *, cwd: pathlib.Path | None = None, check: bool = True) -> subprocess.CompletedProcess[str]:
    result = subprocess.run(command, cwd=cwd, capture_output=True, text=True)
    if check and result.returncode != 0:
        raise AssertionError(
            f"command failed: {command}\nstdout:\n{result.stdout}\nstderr:\n{result.stderr}"
        )
    return result


def git(repo: pathlib.Path, *args: str, check: bool = True) -> subprocess.CompletedProcess[str]:
    return run(["git", "-C", str(repo), *args], check=check)


def write_policy(repo: pathlib.Path, *, remote: str = "origin") -> None:
    policy = repo / "ci" / "rust-verification.toml"
    policy.parent.mkdir(parents=True, exist_ok=True)
    policy.write_text(
        textwrap.dedent(
            f"""\
            schema_version = 2
            project_id = "bolt-v2"
            target_namespace = "bolt-v2"

            [sandbox_safe_push]
            remote = "{remote}"
            """
        ),
        encoding="utf-8",
    )


def assert_fallback_config_loader_matches_tomllib_for_full_policy() -> None:
    helper = load_helper_module()
    helper.tomllib = None
    path = REPO_ROOT / "ci" / "rust-verification.toml"

    with path.open("rb") as handle:
        expected = tomllib.load(handle)
    parsed = helper.load_config(REPO_ROOT)
    if parsed != expected:
        raise AssertionError("sandbox_safe_push fallback config loader must match tomllib for full policy")


def init_work_repo(tmp: pathlib.Path, *, remote_path: pathlib.Path) -> pathlib.Path:
    repo = tmp / "repo"
    run(["git", "-c", "init.defaultBranch=main", "init", str(repo)])
    git(repo, "config", "user.name", "Sandbox Push Test")
    git(repo, "config", "user.email", "sandbox-push@example.invalid")
    git(repo, "checkout", "-b", BRANCH)
    (repo / "README.md").write_text("sandbox-safe push test\n", encoding="utf-8")
    write_policy(repo)
    git(repo, "add", "README.md", "ci/rust-verification.toml")
    git(repo, "commit", "-m", "seed branch")
    git(repo, "remote", "add", "origin", str(remote_path))
    return repo


def run_helper(repo: pathlib.Path, *args: str) -> subprocess.CompletedProcess[str]:
    return run(
        [sys.executable, str(SCRIPT), "--repo", str(repo), *args],
        check=False,
    )


def assert_push_uses_url_without_remote_tracking_write() -> None:
    with tempfile.TemporaryDirectory() as tmp_raw:
        tmp = pathlib.Path(tmp_raw)
        bare = tmp / "origin.git"
        run(["git", "-c", "init.defaultBranch=main", "init", "--bare", str(bare)])
        repo = init_work_repo(tmp, remote_path=bare)

        result = run_helper(repo)
        if result.returncode != 0:
            raise AssertionError(result.stderr)

        head = git(repo, "rev-parse", "HEAD").stdout.strip()
        remote_ref = run(["git", "ls-remote", "--heads", str(bare), BRANCH]).stdout.strip()
        if remote_ref != f"{head}\trefs/heads/{BRANCH}":
            raise AssertionError(remote_ref)

        local_tracking = git(
            repo,
            "show-ref",
            "--verify",
            "--quiet",
            f"refs/remotes/origin/{BRANCH}",
            check=False,
        )
        if local_tracking.returncode == 0:
            raise AssertionError("sandbox-safe push must not update local remote-tracking refs")
        if f"OK: pushed {head} to origin/{BRANCH}" not in result.stdout:
            raise AssertionError(result.stdout)


def assert_push_uses_configured_push_url() -> None:
    with tempfile.TemporaryDirectory() as tmp_raw:
        tmp = pathlib.Path(tmp_raw)
        fetch_bare = tmp / "fetch.git"
        push_bare = tmp / "push.git"
        run(["git", "-c", "init.defaultBranch=main", "init", "--bare", str(fetch_bare)])
        run(["git", "-c", "init.defaultBranch=main", "init", "--bare", str(push_bare)])
        repo = init_work_repo(tmp, remote_path=fetch_bare)
        git(repo, "remote", "set-url", "--push", "origin", str(push_bare))

        result = run_helper(repo)
        if result.returncode != 0:
            raise AssertionError(result.stderr)

        head = git(repo, "rev-parse", "HEAD").stdout.strip()
        push_ref = run(["git", "ls-remote", "--heads", str(push_bare), BRANCH]).stdout.strip()
        if push_ref != f"{head}\trefs/heads/{BRANCH}":
            raise AssertionError(push_ref)
        fetch_ref = run(["git", "ls-remote", "--heads", str(fetch_bare), BRANCH]).stdout.strip()
        if fetch_ref:
            raise AssertionError(fetch_ref)


def assert_multiple_push_urls_fail_closed() -> None:
    with tempfile.TemporaryDirectory() as tmp_raw:
        tmp = pathlib.Path(tmp_raw)
        fetch_bare = tmp / "fetch.git"
        push_one = tmp / "push-one.git"
        push_two = tmp / "push-two.git"
        run(["git", "-c", "init.defaultBranch=main", "init", "--bare", str(fetch_bare)])
        run(["git", "-c", "init.defaultBranch=main", "init", "--bare", str(push_one)])
        run(["git", "-c", "init.defaultBranch=main", "init", "--bare", str(push_two)])
        repo = init_work_repo(tmp, remote_path=fetch_bare)
        git(repo, "remote", "set-url", "--add", "--push", "origin", str(push_one))
        git(repo, "remote", "set-url", "--add", "--push", "origin", str(push_two))

        result = run_helper(repo)
        if result.returncode != 2:
            raise AssertionError(result)
        if "exactly one push URL" not in result.stderr:
            raise AssertionError(result.stderr)
        for remote in (push_one, push_two):
            remote_ref = run(["git", "ls-remote", "--heads", str(remote), BRANCH]).stdout.strip()
            if remote_ref:
                raise AssertionError(remote_ref)


def assert_push_url_rejects_embedded_credentials() -> None:
    helper = load_helper_module()
    calls: list[tuple[str, ...]] = []

    def fake_run_git(
        _repo: pathlib.Path,
        args: list[str],
        *,
        display_args: list[str] | None = None,
        redact_values: tuple[str, ...] = (),
    ) -> subprocess.CompletedProcess[str]:
        del display_args, redact_values
        args_tuple = tuple(args)
        calls.append(args_tuple)
        outputs = {
            ("status", "--porcelain", "--untracked-files=normal"): "",
            ("rev-parse", "HEAD"): "a" * 40,
            ("remote", "get-url", "--push", "--all", "origin"): "https://token@example.invalid/repo.git",
        }
        if args_tuple not in outputs:
            raise AssertionError(f"unexpected git call after credential URL: {args_tuple}")
        return subprocess.CompletedProcess(["git", *args], 0, outputs[args_tuple], "")

    original_run_git = helper.run_git
    try:
        helper.run_git = fake_run_git
        try:
            helper.push_head(pathlib.Path("."), remote="origin", branch=BRANCH)
        except helper.PushError as exc:
            if "must not contain embedded credentials" not in str(exc):
                raise AssertionError(str(exc)) from exc
        else:
            raise AssertionError("credential-bearing push URL was accepted")
    finally:
        helper.run_git = original_run_git

    if any(call[0] in ("push", "ls-remote") for call in calls):
        raise AssertionError(calls)


def assert_push_url_allows_ssh_usernames() -> None:
    helper = load_helper_module()

    for url in ("ssh://git@example.invalid/repo.git", "git@example.invalid:repo.git"):
        try:
            helper.validate_push_url(url)
        except helper.PushError as exc:
            raise AssertionError((url, str(exc))) from exc


def assert_rejects_unsafe_branch_before_push() -> None:
    with tempfile.TemporaryDirectory() as tmp_raw:
        tmp = pathlib.Path(tmp_raw)
        bare = tmp / "origin.git"
        run(["git", "-c", "init.defaultBranch=main", "init", "--bare", str(bare)])
        repo = init_work_repo(tmp, remote_path=bare)

        result = run_helper(repo, "--branch", "bad branch")
        if result.returncode != 2:
            raise AssertionError(result)
        if "must be a safe git branch" not in result.stderr:
            raise AssertionError(result.stderr)
        remote_ref = run(["git", "ls-remote", "--heads", str(bare), BRANCH]).stdout.strip()
        if remote_ref:
            raise AssertionError(remote_ref)


def assert_push_errors_redact_remote_url() -> None:
    with tempfile.TemporaryDirectory() as tmp_raw:
        tmp = pathlib.Path(tmp_raw)
        secret_remote = tmp / "secret-token-origin.git"
        repo = init_work_repo(tmp, remote_path=secret_remote)

        result = run_helper(repo)
        if result.returncode != 2:
            raise AssertionError(result)
        combined = result.stdout + result.stderr
        if str(secret_remote) in combined:
            raise AssertionError(combined)
        if "<remote-url>" not in combined:
            raise AssertionError(combined)


def assert_git_commands_use_option_boundary_before_remote_url() -> None:
    helper = load_helper_module()
    url = "--receive-pack=/tmp/evil"
    head = "a" * 40
    refspec = f"HEAD:refs/heads/{BRANCH}"
    calls: list[tuple[tuple[str, ...], tuple[str, ...], tuple[str, ...]]] = []

    def fake_run_git(
        _repo: pathlib.Path,
        args: list[str],
        *,
        display_args: list[str] | None = None,
        redact_values: tuple[str, ...] = (),
    ) -> subprocess.CompletedProcess[str]:
        args_tuple = tuple(args)
        calls.append((args_tuple, tuple(display_args or ()), redact_values))
        outputs = {
            ("status", "--porcelain", "--untracked-files=normal"): "",
            ("rev-parse", "HEAD"): head,
            ("remote", "get-url", "--push", "--all", "origin"): url,
            ("push", "--", url, refspec): "",
            ("ls-remote", "--heads", "--", url, BRANCH): f"{head}\trefs/heads/{BRANCH}\n",
        }
        if args_tuple not in outputs:
            raise AssertionError(f"unexpected git call without option boundary: {args_tuple}")
        return subprocess.CompletedProcess(["git", *args], 0, outputs[args_tuple], "")

    original_run_git = helper.run_git
    try:
        helper.run_git = fake_run_git
        pushed_head = helper.push_head(pathlib.Path("."), remote="origin", branch=BRANCH)
    finally:
        helper.run_git = original_run_git

    if pushed_head != head:
        raise AssertionError(pushed_head)
    expected_push = ("push", "--", url, refspec)
    expected_ls_remote = ("ls-remote", "--heads", "--", url, BRANCH)
    if expected_push not in [call[0] for call in calls]:
        raise AssertionError(calls)
    if expected_ls_remote not in [call[0] for call in calls]:
        raise AssertionError(calls)
    for args, display_args, redact_values in calls:
        if args == expected_push and display_args != ("git", "push", "--", "<remote-url>", refspec):
            raise AssertionError(display_args)
        if args == expected_ls_remote and display_args != (
            "git",
            "ls-remote",
            "--heads",
            "--",
            "<remote-url>",
            BRANCH,
        ):
            raise AssertionError(display_args)
        if args in (expected_push, expected_ls_remote) and redact_values != (url,):
            raise AssertionError(redact_values)


def assert_requires_clean_worktree() -> None:
    with tempfile.TemporaryDirectory() as tmp_raw:
        tmp = pathlib.Path(tmp_raw)
        bare = tmp / "origin.git"
        run(["git", "-c", "init.defaultBranch=main", "init", "--bare", str(bare)])
        repo = init_work_repo(tmp, remote_path=bare)
        (repo / "untracked.txt").write_text("dirty\n", encoding="utf-8")

        result = run_helper(repo)
        if result.returncode != 2:
            raise AssertionError(result)
        if "requires a clean worktree" not in result.stderr:
            raise AssertionError(result.stderr)


def assert_git_prompt_is_forced_off() -> None:
    helper = load_helper_module()
    captured_env: dict[str, str] = {}

    def fake_run(
        argv: list[str],
        *,
        cwd: pathlib.Path,
        capture_output: bool,
        text: bool,
        env: dict[str, str],
    ) -> subprocess.CompletedProcess[str]:
        captured_env.update(env)
        return subprocess.CompletedProcess(argv, 0, "", "")

    original_run = helper.subprocess.run
    original_prompt = os.environ.get("GIT_TERMINAL_PROMPT")
    try:
        os.environ["GIT_TERMINAL_PROMPT"] = "1"
        helper.subprocess.run = fake_run
        helper.run_git(pathlib.Path("."), ["status"])
    finally:
        helper.subprocess.run = original_run
        if original_prompt is None:
            os.environ.pop("GIT_TERMINAL_PROMPT", None)
        else:
            os.environ["GIT_TERMINAL_PROMPT"] = original_prompt

    if captured_env.get("GIT_TERMINAL_PROMPT") != "0":
        raise AssertionError(captured_env.get("GIT_TERMINAL_PROMPT"))


def main() -> int:
    assert_fallback_config_loader_matches_tomllib_for_full_policy()
    assert_push_uses_url_without_remote_tracking_write()
    assert_push_uses_configured_push_url()
    assert_multiple_push_urls_fail_closed()
    assert_push_url_rejects_embedded_credentials()
    assert_push_url_allows_ssh_usernames()
    assert_rejects_unsafe_branch_before_push()
    assert_push_errors_redact_remote_url()
    assert_git_commands_use_option_boundary_before_remote_url()
    assert_requires_clean_worktree()
    assert_git_prompt_is_forced_off()
    print("OK: sandbox-safe push tests passed.")
    return 0


if __name__ == "__main__":
    import lane_governor

    lane_governor.acquire()
    raise SystemExit(main())
