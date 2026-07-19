#!/usr/bin/env python3
"""Self-tests for repository-wide aggregate formatting."""

from __future__ import annotations

import pathlib

from repo_format import run_format
from test_workspace_registry import fixture_repo


def assert_every_registered_formatter_runs_once() -> None:
    tmp, repo = fixture_repo(include_bvs=True)
    calls: list[str] = []

    def runner(operation_id: str, _command: tuple[str, ...], _subject: pathlib.Path) -> int:
        calls.append(operation_id)
        return 0

    try:
        report = run_format(repo, repo, runner=runner)
        expected = ["root_fmt_write", "bvs_fmt_write"]
        if calls != expected:
            raise AssertionError((calls, expected))
        if report.exit_code != 0:
            raise AssertionError(report)
    finally:
        tmp.cleanup()


def assert_formatter_failure_does_not_suppress_siblings() -> None:
    tmp, repo = fixture_repo(include_bvs=True)
    calls: list[str] = []

    def runner(operation_id: str, _command: tuple[str, ...], _subject: pathlib.Path) -> int:
        calls.append(operation_id)
        return 1 if operation_id == "root_fmt_write" else 0

    try:
        report = run_format(repo, repo, runner=runner)
        if calls != ["root_fmt_write", "bvs_fmt_write"]:
            raise AssertionError(calls)
        if report.failed_operations != ("root_fmt_write",):
            raise AssertionError(report)
    finally:
        tmp.cleanup()


def assert_formatter_uses_registry_workspace_path() -> None:
    bvs_path = "components/relocated-bvs"
    tmp, repo = fixture_repo(include_bvs=True, bvs_path=bvs_path)
    commands: dict[str, tuple[str, ...]] = {}

    def runner(operation_id: str, command: tuple[str, ...], _subject: pathlib.Path) -> int:
        commands[operation_id] = command
        return 0

    try:
        report = run_format(repo, repo, runner=runner)
        if report.exit_code != 0:
            raise AssertionError(report)
        expected = str((repo / bvs_path).resolve())
        if expected not in commands["bvs_fmt_write"]:
            raise AssertionError(commands["bvs_fmt_write"])
    finally:
        tmp.cleanup()


def main() -> int:
    assert_every_registered_formatter_runs_once()
    assert_formatter_failure_does_not_suppress_siblings()
    assert_formatter_uses_registry_workspace_path()
    print("OK: repository formatter tests passed.")
    return 0


if __name__ == "__main__":
    import lane_governor

    lane_governor.acquire()
    raise SystemExit(main())
