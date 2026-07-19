#!/usr/bin/env python3
"""Push the current HEAD without updating local remote-tracking refs."""

from __future__ import annotations

import argparse
import os
import pathlib
import shlex
import subprocess
import sys
import tomllib
import urllib.parse
from collections.abc import Callable
from typing import Any

from git_remote_utils import isolated_git_transport_environment


CONFIG_RELATIVE_PATH = pathlib.Path("ci/rust-verification.toml")


class PushError(RuntimeError):
    pass


def redact(text: str, values: tuple[str, ...]) -> str:
    redacted = text
    for value in values:
        if value:
            redacted = redacted.replace(value, "<remote-url>")
    return redacted


def command_error(
    argv: list[str],
    result: subprocess.CompletedProcess[str],
    *,
    redact_values: tuple[str, ...] = (),
) -> str:
    command = " ".join(shlex.quote(arg) for arg in argv)
    parts = [f"{command} failed with exit code {result.returncode}"]
    stdout = redact(result.stdout.strip(), redact_values)
    stderr = redact(result.stderr.strip(), redact_values)
    if stdout:
        parts.append(f"stdout:\n{stdout}")
    if stderr:
        parts.append(f"stderr:\n{stderr}")
    return "\n".join(parts)


def run_git(
    repo: pathlib.Path,
    args: list[str],
    *,
    display_args: list[str] | None = None,
    redact_values: tuple[str, ...] = (),
) -> subprocess.CompletedProcess[str]:
    argv = ["git", "--no-optional-locks", *args]
    env = isolated_git_transport_environment(os.environ)
    env["GIT_TERMINAL_PROMPT"] = "0"
    try:
        result = subprocess.run(argv, cwd=repo, capture_output=True, text=True, env=env)
    except FileNotFoundError as exc:
        raise PushError("git is required for sandbox-safe push") from exc
    if result.returncode != 0:
        raise PushError(command_error(display_args or argv, result, redact_values=redact_values))
    return result


def git_output(repo: pathlib.Path, *args: str) -> str:
    return run_git(repo, list(args)).stdout.strip()


def load_config_with_tomllib(path: pathlib.Path) -> dict[str, Any]:
    try:
        with path.open("rb") as handle:
            loaded = tomllib.load(handle)
    except tomllib.TOMLDecodeError as exc:
        raise PushError(f"{CONFIG_RELATIVE_PATH} is invalid TOML: {exc}") from exc
    if not isinstance(loaded, dict):
        raise PushError(f"{CONFIG_RELATIVE_PATH} must contain a TOML table")
    return loaded


def load_config(repo: pathlib.Path) -> dict[str, Any]:
    path = repo / CONFIG_RELATIVE_PATH
    if not path.is_file():
        raise PushError(f"{CONFIG_RELATIVE_PATH} is required")
    return load_config_with_tomllib(path)


def validate_remote_name(remote: str) -> None:
    invalid = (
        not remote
        or remote.startswith("-")
        or any(char.isspace() for char in remote)
        or any(char in remote for char in "\\^:?*[]~@{}")
        or "//" in remote
        or ".." in remote
    )
    if invalid:
        raise PushError("sandbox_safe_push.remote must be a safe git remote name")


def configured_remote(repo: pathlib.Path) -> str:
    config = load_config(repo)
    push_config = config.get("sandbox_safe_push")
    if not isinstance(push_config, dict):
        raise PushError(f"{CONFIG_RELATIVE_PATH} must declare [sandbox_safe_push]")
    remote = push_config.get("remote")
    if not isinstance(remote, str) or not remote:
        raise PushError("sandbox_safe_push.remote must be a non-empty string")
    validate_remote_name(remote)
    return remote


def validate_branch(branch: str) -> None:
    invalid = (
        not branch
        or branch.startswith(("/", ".", "-"))
        or branch.endswith(("/", "."))
        or "//" in branch
        or ".." in branch
        or any(char.isspace() or ord(char) < 32 or ord(char) == 127 for char in branch)
        or any(char in branch for char in "\\^:?*[]~@{}")
    )
    if invalid:
        raise PushError("target branch must be a safe git branch")
    for part in branch.split("/"):
        if not part or part.startswith(".") or part.endswith(".lock"):
            raise PushError("target branch must be a safe git branch")


def current_branch(repo: pathlib.Path) -> str:
    branch = git_output(repo, "branch", "--show-current")
    if not branch:
        raise PushError("sandbox-safe push requires the current checkout to be a named branch")
    return branch


def require_clean_worktree(repo: pathlib.Path) -> None:
    status = git_output(repo, "status", "--porcelain", "--untracked-files=normal")
    if status:
        raise PushError("sandbox-safe push requires a clean worktree, including untracked files")
    index_listing = git_output(repo, "ls-files", "-v")
    hidden = [line[2:] for line in index_listing.splitlines() if line and line[0] in {"h", "s", "S"}]
    if hidden:
        preview = ", ".join(hidden[:3])
        raise PushError(f"sandbox-safe push rejects hidden index flags (assume-unchanged/skip-worktree): {preview}")


def run_preflight(repo: pathlib.Path) -> None:
    try:
        result = subprocess.run(["just", "preflight"], cwd=repo, check=False, close_fds=True)
    except FileNotFoundError as exc:
        raise PushError("just is required for sandbox-safe push preflight") from exc
    if result.returncode != 0:
        raise PushError(f"repository preflight failed with exit code {result.returncode}")


def remote_url(repo: pathlib.Path, remote: str) -> str:
    result = run_git(repo, ["remote", "get-url", "--push", "--all", remote])
    urls = [line for line in result.stdout.splitlines() if line]
    if len(urls) != 1:
        raise PushError(f"remote {remote!r} must have exactly one push URL")
    url = urls[0]
    validate_push_url(url)
    return url


def validate_push_url(url: str) -> None:
    parsed = urllib.parse.urlsplit(url)
    has_http_userinfo = parsed.scheme in ("http", "https") and parsed.username is not None
    if parsed.password is not None or has_http_userinfo or parsed.query or parsed.fragment:
        raise PushError(
            "Git push URLs must not contain embedded credentials; use a credential helper or SSH agent auth"
        )


def live_remote_branch_head(repo: pathlib.Path, *, url: str, branch: str) -> str | None:
    result = run_git(
        repo,
        ["ls-remote", "--heads", "--", url, branch],
        display_args=["git", "ls-remote", "--heads", "--", "<remote-url>", branch],
        redact_values=(url,),
    )
    refs = result.stdout.strip()
    for line in refs.splitlines():
        fields = line.split()
        if len(fields) >= 2 and fields[1] == f"refs/heads/{branch}":
            return fields[0]
    return None


def push_head(
    repo: pathlib.Path,
    *,
    remote: str,
    branch: str,
    preflight_runner: Callable[[pathlib.Path], None] = run_preflight,
) -> str:
    require_clean_worktree(repo)
    url = remote_url(repo, remote)
    captured_branch = current_branch(repo)
    validate_branch(captured_branch)
    if branch != captured_branch:
        raise PushError("sandbox-safe push publishes only the current named branch")
    head = git_output(repo, "rev-parse", "HEAD")
    tree = git_output(repo, "rev-parse", "HEAD^{tree}")
    preflight_runner(repo)
    require_clean_worktree(repo)
    if current_branch(repo) != captured_branch:
        raise PushError("current branch changed while repository preflight was running")
    current_head = git_output(repo, "rev-parse", "HEAD")
    current_tree = git_output(repo, "rev-parse", "HEAD^{tree}")
    if (current_head, current_tree) != (head, tree):
        raise PushError("HEAD or its tree changed while repository preflight was running")
    refspec = f"{head}:refs/heads/{branch}"
    run_git(
        repo,
        ["push", "--", url, refspec],
        display_args=["git", "push", "--", "<remote-url>", refspec],
        redact_values=(url,),
    )
    remote_head = live_remote_branch_head(repo, url=url, branch=branch)
    if remote_head is None:
        raise PushError(f"remote branch {remote}/{branch} was not found after push")
    if remote_head != head:
        raise PushError(f"remote branch {remote}/{branch} is at {remote_head}, expected {head}")
    return head


def parse_args(argv: list[str]) -> argparse.Namespace:
    parser = argparse.ArgumentParser(description=__doc__)
    return parser.parse_args(argv)


def main(argv: list[str]) -> int:
    args = parse_args(argv)
    del args
    repo = pathlib.Path.cwd().resolve()
    try:
        top_level = pathlib.Path(git_output(repo, "rev-parse", "--show-toplevel")).resolve()
        if top_level != repo:
            raise PushError("sandbox-safe push must run from the current repository root")
        remote = configured_remote(repo)
        branch = current_branch(repo)
        validate_branch(branch)
        head = push_head(repo, remote=remote, branch=branch)
    except PushError as exc:
        print(f"ERROR: {exc}", file=sys.stderr)
        return 2
    print(f"OK: pushed {head} to {remote}/{branch}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main(sys.argv[1:]))
