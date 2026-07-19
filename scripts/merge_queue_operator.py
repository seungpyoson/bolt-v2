#!/usr/bin/env python3
"""Operator entrypoint for preflight-guarded Mergify queueing."""

from __future__ import annotations

import argparse
import contextlib
import dataclasses
import functools
import json
import os
import pathlib
import re
import signal
import subprocess
import sys
import tempfile
import tomllib
from collections.abc import Callable, Iterator, Mapping


SCRIPT_DIR = pathlib.Path(__file__).resolve().parent
if str(SCRIPT_DIR) not in sys.path:
    sys.path.insert(0, str(SCRIPT_DIR))

import config_validators as _cv  # noqa: E402
from git_remote_utils import (  # noqa: E402
    github_https_remote_url as _github_https_remote_url,
    isolated_git_transport_environment,
    require_remote_name as _require_remote_name,
)
from merge_queue_preflight import (  # noqa: E402
    VERDICT_QUEUE_AS_ONE_WAVE,
    positive_pr_number,
)


REPO_ROOT = pathlib.Path(__file__).resolve().parents[1]
DEFAULT_CONFIG = REPO_ROOT / "ci" / "rust-verification.toml"
PREFLIGHT_SCRIPT = REPO_ROOT / "scripts" / "merge_queue_preflight.py"
SHA_RE = re.compile(r"^[0-9a-f]{40}$")


@dataclasses.dataclass(frozen=True)
class CommandResult:
    command: tuple[str, ...]
    returncode: int
    stdout: str
    stderr: str


class OperatorError(Exception):
    """Raised when merge queue operator inputs or commands fail."""


require_table = functools.partial(_cv.require_table, error_cls=OperatorError)
require_string = functools.partial(_cv.require_string, error_cls=OperatorError)
require_positive_int = functools.partial(_cv.require_positive_int, error_cls=OperatorError)
github_https_remote_url = functools.partial(_github_https_remote_url, error_cls=OperatorError)
require_remote_name = functools.partial(_require_remote_name, error_cls=OperatorError)


Runner = Callable[..., CommandResult]


def run_command(
    command: list[str],
    *,
    cwd: pathlib.Path,
    check: bool = False,
    input_text: str | None = None,
    timeout_seconds: int | None = None,
    environment: Mapping[str, str] | None = None,
) -> CommandResult:
    stdin = subprocess.PIPE if input_text is not None else subprocess.DEVNULL
    try:
        process = subprocess.Popen(
            command,
            cwd=cwd,
            stdin=stdin,
            stdout=subprocess.PIPE,
            stderr=subprocess.PIPE,
            text=True,
            start_new_session=True,
            env=environment,
        )
    except FileNotFoundError as exc:
        raise OperatorError(f"{command[0]} is unavailable") from exc
    except OSError as exc:
        raise OperatorError(f"{command[0]} could not start: {exc}") from exc
    try:
        stdout, stderr = process.communicate(input=input_text, timeout=timeout_seconds)
    except subprocess.TimeoutExpired as exc:
        try:
            os.killpg(process.pid, signal.SIGKILL)
        except ProcessLookupError:
            pass
        stdout, stderr = process.communicate()
        raise OperatorError(f"{command[0]} timed out after {timeout_seconds} seconds") from exc
    result = CommandResult(tuple(command), process.returncode, stdout, stderr)
    if check and result.returncode != 0:
        raise OperatorError(result.stderr.strip() or f"{command[0]} exited {result.returncode}")
    return result


@dataclasses.dataclass(frozen=True)
class OperatorConfig:
    origin: str
    base: str
    repository: str
    queue_command: str
    ref_timeout_seconds: int
    queue_timeout_seconds: int


@dataclasses.dataclass(frozen=True)
class PreflightRun:
    payload: dict[str, object]
    returncode: int
    queue_repository: str
    remote_url: str
    environment: Mapping[str, str]
    expected_base_sha: str
    expected_head_sha: str


def load_operator_config(path: pathlib.Path) -> OperatorConfig:
    try:
        root = tomllib.loads(path.read_text(encoding="utf-8"))
    except (OSError, tomllib.TOMLDecodeError) as exc:
        raise OperatorError(f"unable to read config {path}: {exc}") from exc
    settings = require_table(root, "merge_queue_preflight", "config")
    operator = require_table(settings, "operator", "config.merge_queue_preflight")
    repository = require_string(operator, "repository", "config.merge_queue_preflight.operator")
    github_https_remote_url(repository)
    return OperatorConfig(
        origin=require_string(settings, "origin", "config.merge_queue_preflight"),
        base=require_string(settings, "base", "config.merge_queue_preflight"),
        repository=repository,
        queue_command=require_string(operator, "queue_command", "config.merge_queue_preflight.operator"),
        ref_timeout_seconds=require_positive_int(
            operator,
            "ref_timeout_seconds",
            "config.merge_queue_preflight.operator",
        ),
        queue_timeout_seconds=require_positive_int(
            operator,
            "queue_timeout_seconds",
            "config.merge_queue_preflight.operator",
        ),
    )


def parse_ls_remote_sha(output: str, ref: str) -> str:
    fields = output.split()
    if len(fields) < 2 or fields[1] != ref or not SHA_RE.fullmatch(fields[0]):
        raise OperatorError(f"git ls-remote did not return a valid SHA for {ref}")
    return fields[0]


def resolve_remote_url(
    repo: pathlib.Path,
    remote_name: str,
    expected_remote_url: str,
    runner: Runner,
    timeout_seconds: int,
) -> str:
    require_remote_name(remote_name)
    result = runner(
        ["git", "config", "--local", "--get-all", f"remote.{remote_name}.url"],
        cwd=repo,
        check=False,
        timeout_seconds=timeout_seconds,
        environment=isolated_git_transport_environment(os.environ),
    )
    remote_urls = result.stdout.splitlines()
    if result.returncode != 0 or len(remote_urls) != 1 or not remote_urls[0]:
        raise OperatorError("configured Git remote did not resolve to a URL")
    if remote_urls[0] != expected_remote_url:
        raise OperatorError("checkout Git remote does not match configured repository")
    return remote_urls[0]


def preflight_environment(queue_repository: str) -> dict[str, str]:
    environment = isolated_git_transport_environment(os.environ)
    environment["GH_REPO"] = queue_repository
    return environment


def remote_ref_sha(
    repo: pathlib.Path,
    remote_url: str,
    ref: str,
    runner: Runner,
    timeout_seconds: int,
    environment: Mapping[str, str],
) -> str:
    result = runner(
        ["git", "ls-remote", "--exit-code", remote_url, ref],
        cwd=repo,
        check=False,
        timeout_seconds=timeout_seconds,
        environment=environment,
    )
    if result.returncode != 0:
        raise OperatorError(f"configured Git remote did not resolve {ref}")
    return parse_ls_remote_sha(result.stdout, ref)


@contextlib.contextmanager
def immutable_config_snapshot(path: pathlib.Path) -> Iterator[pathlib.Path]:
    try:
        config_bytes = path.read_bytes()
    except OSError as exc:
        raise OperatorError(f"unable to read config {path}: {exc}") from exc
    with tempfile.TemporaryDirectory(prefix="merge-queue-operator-config-") as temp_dir:
        snapshot = pathlib.Path(temp_dir) / "rust-verification.toml"
        snapshot.write_bytes(config_bytes)
        yield snapshot


def expected_head_arg(pr: int, sha: str) -> str:
    return f"{pr}={sha}"


def build_preflight_command(
    *,
    pr: int,
    config: pathlib.Path,
    expected_base_sha: str,
    expected_head_sha: str,
) -> list[str]:
    return [
        "python3",
        str(PREFLIGHT_SCRIPT),
        str(pr),
        "--config",
        str(config),
        "--expected-base-sha",
        expected_base_sha,
        "--expected-head-sha",
        expected_head_arg(pr, expected_head_sha),
        "--json",
    ]


def run_preflight(
    *,
    repo: pathlib.Path,
    pr: int,
    config: pathlib.Path,
    operator_config: OperatorConfig,
    runner: Runner,
) -> PreflightRun:
    remote_url = resolve_remote_url(
        repo,
        operator_config.origin,
        github_https_remote_url(operator_config.repository),
        runner,
        operator_config.ref_timeout_seconds,
    )
    environment = preflight_environment(operator_config.repository)
    expected_base_sha = remote_ref_sha(
        config.parent,
        remote_url,
        f"refs/heads/{operator_config.base}",
        runner,
        operator_config.ref_timeout_seconds,
        environment,
    )
    expected_head_sha = remote_ref_sha(
        config.parent,
        remote_url,
        f"refs/pull/{pr}/head",
        runner,
        operator_config.ref_timeout_seconds,
        environment,
    )
    command = build_preflight_command(
        pr=pr,
        config=config,
        expected_base_sha=expected_base_sha,
        expected_head_sha=expected_head_sha,
    )
    result = runner(command, cwd=repo, environment=environment)
    try:
        payload = json.loads(result.stdout)
    except json.JSONDecodeError as exc:
        raise OperatorError(f"merge queue preflight did not emit valid JSON: {exc}") from exc
    if not isinstance(payload, dict):
        raise OperatorError("merge queue preflight JSON payload must be an object")
    return PreflightRun(
        payload=payload,
        returncode=result.returncode,
        queue_repository=operator_config.repository,
        remote_url=remote_url,
        environment=environment,
        expected_base_sha=expected_base_sha,
        expected_head_sha=expected_head_sha,
    )


def queue_pr(
    pr: int,
    queue_command: str,
    queue_repository: str,
    repo: pathlib.Path,
    runner: Runner,
    timeout_seconds: int,
) -> None:
    runner(
        ["gh", "pr", "comment", str(pr), "--repo", queue_repository, "--body-file", "-"],
        cwd=repo,
        check=True,
        input_text=f"{queue_command}\n",
        timeout_seconds=timeout_seconds,
    )


def operate(
    args: argparse.Namespace,
    *,
    runner: Runner,
    repo: pathlib.Path,
    config: pathlib.Path,
) -> int:
    with immutable_config_snapshot(config) as config_path:
        operator_config = load_operator_config(config_path)
        preflight = run_preflight(
            repo=repo,
            pr=args.pr,
            config=config_path,
            operator_config=operator_config,
            runner=runner,
        )
        verdict = preflight.payload.get("verdict")
        if preflight.returncode == 0 and verdict == VERDICT_QUEUE_AS_ONE_WAVE:
            expected_identity = {
                "requested_prs": [args.pr],
                "expected_base_sha": preflight.expected_base_sha,
                "actual_base_sha": preflight.expected_base_sha,
                "expected_pr_heads": {str(args.pr): preflight.expected_head_sha},
                "pr_heads": {str(args.pr): preflight.expected_head_sha},
            }
            observed_identity = {
                field: preflight.payload.get(field)
                for field in expected_identity
            }
            if observed_identity != expected_identity:
                raise OperatorError("preflight result identity does not match resolved base and PR head")
            current_identity = {
                "base": remote_ref_sha(
                    config_path.parent,
                    preflight.remote_url,
                    f"refs/heads/{operator_config.base}",
                    runner,
                    operator_config.ref_timeout_seconds,
                    preflight.environment,
                ),
                "head": remote_ref_sha(
                    config_path.parent,
                    preflight.remote_url,
                    f"refs/pull/{args.pr}/head",
                    runner,
                    operator_config.ref_timeout_seconds,
                    preflight.environment,
                ),
            }
            if current_identity != {
                "base": preflight.expected_base_sha,
                "head": preflight.expected_head_sha,
            }:
                raise OperatorError("base or PR head changed after preflight")
            queue_pr(
                args.pr,
                operator_config.queue_command,
                preflight.queue_repository,
                repo,
                runner,
                operator_config.queue_timeout_seconds,
            )
            print(f"queued PR #{args.pr}")
            return 0
        print(f"merge queue preflight did not queue: verdict={verdict!r}")
        return preflight.returncode if preflight.returncode != 0 else 4


def parser() -> argparse.ArgumentParser:
    root = argparse.ArgumentParser(prog="merge_queue_operator.py", allow_abbrev=False)
    root.add_argument("pr", type=positive_pr_number)
    return root


def main(
    argv: list[str] | None = None,
    *,
    runner: Runner = run_command,
    repo: pathlib.Path = REPO_ROOT,
    config: pathlib.Path = DEFAULT_CONFIG,
) -> int:
    try:
        return operate(parser().parse_args(argv), runner=runner, repo=repo, config=config)
    except OperatorError as exc:
        print(f"error: {exc}", file=sys.stderr)
        return 4


if __name__ == "__main__":
    raise SystemExit(main())
