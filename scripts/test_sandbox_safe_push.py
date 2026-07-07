#!/usr/bin/env python3
"""Self-tests for sandbox-safe Git branch publishing."""

from __future__ import annotations

import pathlib
import subprocess
import sys
import tempfile
import textwrap


REPO_ROOT = pathlib.Path(__file__).resolve().parents[1]
SCRIPT = REPO_ROOT / "scripts" / "sandbox_safe_push.py"
BRANCH = "codex/sandbox-safe-push"


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


def main() -> int:
    assert_push_uses_url_without_remote_tracking_write()
    assert_rejects_unsafe_branch_before_push()
    assert_push_errors_redact_remote_url()
    assert_requires_clean_worktree()
    print("OK: sandbox-safe push tests passed.")
    return 0


if __name__ == "__main__":
    import lane_governor

    lane_governor.acquire()
    raise SystemExit(main())
