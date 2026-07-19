#!/usr/bin/env python3
"""Behavior tests for the fixed governance-owned final-review runner."""

from __future__ import annotations

import json
import os
import pathlib
import sys
import tempfile
import time
from unittest import mock

import final_review_runner as runner
from final_review_runner import (
    FINAL_REVIEW_OBLIGATIONS,
    Obligation,
    execute_command,
    registered_workspace_roots,
    run_obligations,
)
from test_workspace_registry import fixture_repo


EXPECTED_OBLIGATION_IDS = (
    "preflight",
    "host-health",
    "host-health-viewer",
    "root-clippy",
    "root-aarch64",
    "root-build",
    "root-archive",
    "root-cache-release",
    "root-tests",
    "root-special-proofs",
    "bvs-clippy",
    "bvs-archive",
    "bvs-cache-release",
    "bvs-s3-smoke",
    "bvs-tests",
)

EXPECTED_PHASES = {
    "static": ("preflight", "host-health", "host-health-viewer"),
    "root-analysis": ("root-clippy", "root-aarch64", "root-build"),
    "root-tests": ("root-archive", "root-cache-release", "root-tests", "root-special-proofs"),
    "bvs-analysis": ("bvs-clippy",),
    "bvs-tests": ("bvs-archive", "bvs-cache-release", "bvs-s3-smoke", "bvs-tests"),
}


def assert_fixed_obligation_inventory_is_complete() -> None:
    ids = tuple(obligation.obligation_id for obligation in FINAL_REVIEW_OBLIGATIONS)
    if ids != EXPECTED_OBLIGATION_IDS:
        raise AssertionError(ids)
    commands = {obligation.obligation_id: obligation.command for obligation in FINAL_REVIEW_OBLIGATIONS}
    for obligation_id in ("root-tests", "bvs-tests"):
        if "--no-fail-fast" not in commands[obligation_id]:
            raise AssertionError(f"{obligation_id} lost complete failure collection")
    if {"workflow-lint", "actionlint"} & set(ids):
        raise AssertionError("final review duplicates workflow lint already owned by preflight")


def assert_fixed_phases_partition_the_inventory() -> None:
    actual = {
        phase: tuple(obligation.obligation_id for obligation in obligations)
        for phase, obligations in runner.FINAL_REVIEW_PHASES.items()
    }
    if actual != EXPECTED_PHASES:
        raise AssertionError(actual)
    flattened = tuple(
        obligation_id
        for phase_ids in actual.values()
        for obligation_id in phase_ids
    )
    if flattened != EXPECTED_OBLIGATION_IDS:
        raise AssertionError(flattened)


def assert_preflight_uses_the_public_gate_owner() -> None:
    command = runner.FINAL_REVIEW_PHASES["static"][0].command
    expected_prefix = (
        "python3",
        "{governance}/scripts/local_verification_gate.py",
        "preflight",
        "--",
        "python3",
        "{governance}/scripts/repo_preflight.py",
    )
    if command[: len(expected_prefix)] != expected_prefix:
        raise AssertionError(command)


def assert_archive_phases_release_only_their_managed_cache() -> None:
    commands = {
        obligation.obligation_id: obligation.command
        for obligation in FINAL_REVIEW_OBLIGATIONS
    }
    expected = {
        "root-cache-release": ("python3", "{owner}", "cargo", "--repo", "{subject}", "--", "clean"),
        "bvs-cache-release": ("python3", "{owner}", "cargo", "--repo", "{bvs}", "--", "clean"),
    }
    for obligation_id, command in expected.items():
        if commands.get(obligation_id) != command:
            raise AssertionError((obligation_id, commands.get(obligation_id)))


def assert_all_fixed_obligations_run_and_failures_remain_raw() -> None:
    with tempfile.TemporaryDirectory() as tmp:
        root = pathlib.Path(tmp)
        calls: list[tuple[tuple[str, ...], pathlib.Path]] = []

        def execute(command: tuple[str, ...], cwd: pathlib.Path, log: pathlib.Path, _timeout_seconds: float) -> int:
            calls.append((command, cwd))
            log.write_text("observed\n", encoding="utf-8")
            return 7 if command == ("fail",) else 0

        records = run_obligations(
            (
                Obligation("first", ("fail",), pathlib.Path("subject")),
                Obligation("second", ("pass",), pathlib.Path("governance")),
            ),
            governance=root / "governance",
            subject=root / "subject",
            head_sha="a" * 40,
            run_id="12",
            run_attempt="3",
            output=root / "out",
            execute=execute,
            timeout_seconds=30,
        )
        if [record["conclusion"] for record in records] != ["failure", "success"]:
            raise AssertionError(records)
        if [call[0] for call in calls] != [("fail",), ("pass",)]:
            raise AssertionError(calls)
        if any("failed_tests" in record for record in records):
            raise AssertionError(records)
        persisted = json.loads((root / "out" / "records.json").read_text(encoding="utf-8"))
        if persisted != records:
            raise AssertionError(persisted)


def assert_subject_commands_receive_no_github_credentials() -> None:
    with tempfile.TemporaryDirectory() as tmp:
        root = pathlib.Path(tmp)
        captured: dict[str, object] = {}

        process = mock.Mock()
        process.pid = 123
        process.wait.return_value = 0

        def fake_popen(*args: object, **kwargs: object) -> mock.Mock:
            captured.update(kwargs)
            return process

        with (
            mock.patch.dict(
                os.environ,
                {"GITHUB_TOKEN": "secret", "GH_TOKEN": "secret", "SAFE_VALUE": "kept"},
                clear=False,
            ),
            mock.patch("final_review_runner.subprocess.Popen", side_effect=fake_popen),
            mock.patch("final_review_runner.os.killpg") as killpg,
        ):
            execute_command(("subject-command",), root, root / "command.log", 30)

        env = captured.get("env")
        if not isinstance(env, dict):
            raise AssertionError(captured)
        if "GITHUB_TOKEN" in env or "GH_TOKEN" in env:
            raise AssertionError("subject command inherited GitHub credentials")
        if env.get("SAFE_VALUE") != "kept":
            raise AssertionError(env)
        killpg.assert_called_once_with(123, runner.signal.SIGKILL)


def assert_spawn_exception_is_recorded_and_siblings_continue() -> None:
    with tempfile.TemporaryDirectory() as tmp:
        root = pathlib.Path(tmp)
        calls: list[tuple[str, ...]] = []

        def execute(command: tuple[str, ...], _cwd: pathlib.Path, log: pathlib.Path, _timeout_seconds: float) -> int:
            calls.append(command)
            if command == ("explode",):
                raise FileNotFoundError("missing executable")
            log.write_text("passed\n", encoding="utf-8")
            return 0

        records = run_obligations(
            (
                Obligation("first", ("explode",), pathlib.Path("subject")),
                Obligation("second", ("pass",), pathlib.Path("subject")),
            ),
            governance=root / "governance",
            subject=root / "subject",
            head_sha="a" * 40,
            run_id="12",
            run_attempt="3",
            output=root / "out",
            execute=execute,
            timeout_seconds=30,
        )
        if calls != [("explode",), ("pass",)]:
            raise AssertionError(calls)
        if [record["conclusion"] for record in records] != ["infrastructure_failure", "success"]:
            raise AssertionError(records)
        expected = json.loads((root / "out" / "expected.json").read_text(encoding="utf-8"))
        if expected != {
            "obligation_ids": ["first", "second"],
            "head_sha": "a" * 40,
            "run_id": "12",
            "run_attempt": "3",
        }:
            raise AssertionError(expected)


def assert_execute_command_records_timeout() -> None:
    with tempfile.TemporaryDirectory() as tmp:
        root = pathlib.Path(tmp)
        log = root / "timeout.log"
        try:
            execute_command(
                (sys.executable, "-c", "import time; time.sleep(1)"),
                root,
                log,
                0.05,
            )
        except TimeoutError:
            pass
        else:
            raise AssertionError("timed-out command returned normally")
        timeout_log = log.read_text(encoding="utf-8")
        if "timed out after 0.05 seconds" not in timeout_log:
            raise AssertionError(timeout_log)


def assert_successful_command_cannot_leave_background_writer() -> None:
    with tempfile.TemporaryDirectory() as tmp:
        root = pathlib.Path(tmp)
        marker = root / "background-writer-ran"
        child = (
            "import pathlib,sys,time; "
            "time.sleep(0.2); "
            "pathlib.Path(sys.argv[1]).write_text('tampered', encoding='utf-8')"
        )
        parent = (
            "import subprocess,sys; "
            "subprocess.Popen([sys.executable, '-c', sys.argv[1], sys.argv[2]])"
        )
        result = execute_command(
            (sys.executable, "-c", parent, child, str(marker)),
            root,
            root / "background.log",
            30,
        )
        if result != 0:
            raise AssertionError(result)
        time.sleep(0.4)
        if marker.exists():
            raise AssertionError("successful command left a background writer alive")


def assert_timeout_is_recorded_and_siblings_continue() -> None:
    with tempfile.TemporaryDirectory() as tmp:
        root = pathlib.Path(tmp)
        governance = root / "governance"
        subject = root / "subject"
        governance.mkdir()
        subject.mkdir()
        calls: list[tuple[str, ...]] = []

        def execute(command: tuple[str, ...], _cwd: pathlib.Path, log: pathlib.Path, _timeout_seconds: float) -> int:
            calls.append(command)
            if command == ("timeout",):
                raise TimeoutError("command timed out after 0.05 seconds")
            log.write_text("passed\n", encoding="utf-8")
            return 0

        records = run_obligations(
            (
                Obligation("first", ("timeout",), pathlib.Path("subject")),
                Obligation("second", ("pass",), pathlib.Path("subject")),
            ),
            governance=governance,
            subject=subject,
            head_sha="a" * 40,
            run_id="12",
            run_attempt="3",
            output=root / "out",
            execute=execute,
            timeout_seconds=30,
        )
        if calls != [("timeout",), ("pass",)]:
            raise AssertionError(calls)
        if [record["conclusion"] for record in records] != ["infrastructure_failure", "success"]:
            raise AssertionError(records)
        timeout_log = (root / "out/logs/first.log").read_text(encoding="utf-8")
        if "timed out after 0.05 seconds" not in timeout_log:
            raise AssertionError(timeout_log)


def assert_final_review_uses_registry_workspace_paths() -> None:
    bvs_path = "components/relocated-bvs"
    tmp, repo = fixture_repo(include_bvs=True, bvs_path=bvs_path)
    try:
        roots = registered_workspace_roots(repo, repo)
        if roots["bolt_v2"] != repo.resolve():
            raise AssertionError(roots)
        if roots["backtesting_vertical_slice"] != (repo / bvs_path).resolve():
            raise AssertionError(roots)
    finally:
        tmp.cleanup()


def main() -> int:
    assert_fixed_obligation_inventory_is_complete()
    assert_fixed_phases_partition_the_inventory()
    assert_preflight_uses_the_public_gate_owner()
    assert_archive_phases_release_only_their_managed_cache()
    assert_all_fixed_obligations_run_and_failures_remain_raw()
    assert_subject_commands_receive_no_github_credentials()
    assert_spawn_exception_is_recorded_and_siblings_continue()
    assert_execute_command_records_timeout()
    assert_successful_command_cannot_leave_background_writer()
    assert_timeout_is_recorded_and_siblings_continue()
    assert_final_review_uses_registry_workspace_paths()
    print("OK: final-review runner tests passed.")
    return 0


if __name__ == "__main__":
    import lane_governor

    lane_governor.acquire()
    raise SystemExit(main())
