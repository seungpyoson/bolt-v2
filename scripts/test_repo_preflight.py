#!/usr/bin/env python3
"""Self-tests for repository-wide non-compile preflight."""

from __future__ import annotations

import pathlib
import subprocess
import tempfile

from repo_preflight import CommandResult, run_preflight
from test_workspace_registry import fixture_repo
from workspace_registry import CHECK_OPERATIONS


def assert_repository_checks_are_bound_to_subject() -> None:
    governance = pathlib.Path("/tmp/governance")
    subject = pathlib.Path("/tmp/subject")
    for operation_id in ("source_fence_static", "workflow_lint"):
        command = CHECK_OPERATIONS[operation_id].render(governance, subject)
        if str(subject.resolve()) not in command:
            raise AssertionError(f"{operation_id} is not bound to the subject tree: {command}")


def assert_all_checks_run_after_failure() -> None:
    tmp, repo = fixture_repo(include_bvs=True)
    calls: list[str] = []

    def runner(operation_id: str, _command: tuple[str, ...], _repo: pathlib.Path) -> CommandResult:
        calls.append(operation_id)
        return CommandResult(operation_id, 1 if operation_id == "root_fmt_check" else 0, 0.01)

    try:
        report = run_preflight(repo, repo, runner=runner)
        expected = [
            "root_fmt_check",
            "root_deny",
            "bvs_fmt_check",
            "bvs_deny",
            "source_fence_static",
            "workflow_lint",
        ]
        if calls != expected:
            raise AssertionError((calls, expected))
        if report.exit_code != 1 or report.failed_checks != ("root_fmt_check",):
            raise AssertionError(report)
    finally:
        tmp.cleanup()


def assert_workspace_checks_use_registry_paths() -> None:
    bvs_path = "components/relocated-bvs"
    tmp, repo = fixture_repo(include_bvs=True, bvs_path=bvs_path)
    commands: dict[str, tuple[str, ...]] = {}

    def runner(operation_id: str, command: tuple[str, ...], _repo: pathlib.Path) -> CommandResult:
        commands[operation_id] = command
        return CommandResult(operation_id, 0, 0.01)

    try:
        report = run_preflight(repo, repo, runner=runner)
        if report.exit_code != 0:
            raise AssertionError(report)
        expected = str((repo / bvs_path).resolve())
        for operation_id in ("bvs_fmt_check", "bvs_deny"):
            if expected not in commands[operation_id]:
                raise AssertionError(f"{operation_id} ignored registry path: {commands[operation_id]}")
    finally:
        tmp.cleanup()


def assert_success_report_is_explicitly_non_compile() -> None:
    tmp, repo = fixture_repo(include_bvs=True)

    def runner(operation_id: str, _command: tuple[str, ...], _repo: pathlib.Path) -> CommandResult:
        return CommandResult(operation_id, 0, 0.01)

    try:
        report = run_preflight(repo, repo, runner=runner)
        rendered = report.render()
        required = (
            "LOCAL NON-COMPILE PREFLIGHT ONLY",
            "Rust build: NOT RUN LOCALLY",
            "Rust clippy: NOT RUN LOCALLY",
            "Rust tests: NOT RUN LOCALLY",
            "Runtime behavior proof: NOT RUN LOCALLY",
        )
        for text in required:
            if text not in rendered:
                raise AssertionError(rendered)
        if report.exit_code != 0:
            raise AssertionError(report)
    finally:
        tmp.cleanup()


def assert_state_mutation_invalidates_preflight() -> None:
    tmp, repo = fixture_repo(include_bvs=True)

    def runner(operation_id: str, _command: tuple[str, ...], target: pathlib.Path) -> CommandResult:
        if operation_id == "root_deny":
            (target / "Cargo.toml").write_text("[package]\nname='changed'\nversion='0.0.0'\n", encoding="utf-8")
        return CommandResult(operation_id, 0, 0.01)

    try:
        report = run_preflight(repo, repo, runner=runner)
        if report.exit_code != 1 or "repository_state_changed" not in report.failed_checks:
            raise AssertionError(report)
    finally:
        tmp.cleanup()


def assert_hidden_index_flags_are_rejected() -> None:
    for flag in ("--assume-unchanged", "--skip-worktree"):
        tmp, repo = fixture_repo(include_bvs=True)
        try:
            subprocess.run(["git", "-C", str(repo), "update-index", flag, "Cargo.toml"], check=True)
            try:
                run_preflight(repo, repo, runner=lambda *_: CommandResult("unused", 0, 0.0))
            except Exception as exc:
                if "hidden index flags" not in str(exc):
                    raise AssertionError(str(exc)) from exc
            else:
                raise AssertionError(f"preflight accepted {flag}")
        finally:
            tmp.cleanup()


def assert_repository_preflight_registry_is_complete() -> None:
    repo = pathlib.Path(__file__).resolve().parents[1]
    calls: list[str] = []

    def runner(operation_id: str, _command: tuple[str, ...], _repo: pathlib.Path) -> CommandResult:
        calls.append(operation_id)
        return CommandResult(operation_id, 0, 0.01)

    report = run_preflight(repo, repo, runner=runner)
    expected = [
        "root_fmt_check",
        "root_deny",
        "bvs_fmt_check",
        "bvs_deny",
        "source_fence_static",
        "workflow_lint",
    ]
    if calls != expected or report.exit_code != 0:
        raise AssertionError((calls, report))


def assert_partial_local_recipes_are_not_public() -> None:
    repo = pathlib.Path(__file__).resolve().parents[1]
    recipe_headers = {
        line.split(":", 1)[0]
        for line in (repo / "justfile").read_text(encoding="utf-8").splitlines()
        if line and not line[0].isspace() and ":" in line
    }
    forbidden = {
        "fmt-check",
        "bte-fmt-check",
        "deny",
        "source-fence-static",
        "ci-lint-workflow",
    }
    surviving = sorted(recipe_headers & forbidden)
    if surviving:
        raise AssertionError(f"partial public recipes remain: {', '.join(surviving)}")

    config = (repo / ".no-mistakes.yaml").read_text(encoding="utf-8")
    if 'lint: "just preflight"' not in config or 'format: "just fmt"' not in config:
        raise AssertionError(".no-mistakes.yaml must use only aggregate local commands")


def main() -> int:
    assert_repository_checks_are_bound_to_subject()
    assert_all_checks_run_after_failure()
    assert_workspace_checks_use_registry_paths()
    assert_success_report_is_explicitly_non_compile()
    assert_state_mutation_invalidates_preflight()
    assert_hidden_index_flags_are_rejected()
    assert_repository_preflight_registry_is_complete()
    assert_partial_local_recipes_are_not_public()
    print("OK: repository preflight tests passed.")
    return 0


if __name__ == "__main__":
    import lane_governor

    lane_governor.acquire()
    raise SystemExit(main())
