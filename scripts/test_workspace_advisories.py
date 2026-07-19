#!/usr/bin/env python3
"""Self-tests for aggregate workspace advisory coverage."""

from __future__ import annotations

import pathlib

from test_workspace_registry import fixture_repo
from workspace_advisories import run_advisories


def main() -> int:
    tmp, repo = fixture_repo(include_bvs=True)
    calls: list[tuple[str, tuple[str, ...]]] = []

    def runner(workspace_id: str, command: tuple[str, ...], _cwd: pathlib.Path) -> int:
        calls.append((workspace_id, command))
        return 1 if workspace_id == "bolt_v2" else 0

    try:
        results = run_advisories(repo, repo, runner=runner)
        if [workspace_id for workspace_id, _ in calls] != ["backtesting_vertical_slice", "bolt_v2"]:
            raise AssertionError(calls)
        repos = {command[4] for _, command in calls}
        if repos != {str(repo), str(repo / "crates/backtesting-vertical-slice")}:
            raise AssertionError(repos)
        if results != (("backtesting_vertical_slice", 0), ("bolt_v2", 1)):
            raise AssertionError(results)
    finally:
        tmp.cleanup()
    print("OK: workspace advisory coverage tests passed.")
    return 0


if __name__ == "__main__":
    import lane_governor

    lane_governor.acquire()
    raise SystemExit(main())
