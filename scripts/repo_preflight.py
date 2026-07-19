#!/usr/bin/env python3
"""Run every governed local non-compile check and report a complete inventory."""

from __future__ import annotations

import argparse
import hashlib
import os
import pathlib
import subprocess
import sys
import time
from dataclasses import dataclass
from typing import Callable, Sequence

from workspace_registry import (
    CHECK_OPERATIONS,
    RegistryError,
    WorkspaceRegistry,
    load_registry,
    reconcile_registry,
    validate_operation_recipes,
)


@dataclass(frozen=True)
class CommandResult:
    operation_id: str
    returncode: int
    duration_seconds: float


@dataclass(frozen=True)
class PreflightReport:
    results: tuple[CommandResult, ...]
    failed_checks: tuple[str, ...]

    @property
    def exit_code(self) -> int:
        return 0 if not self.failed_checks else 1

    def render(self) -> str:
        lines = ["Repository preflight results:"]
        for result in self.results:
            state = "PASS" if result.returncode == 0 else f"FAIL({result.returncode})"
            lines.append(f"- {result.operation_id}: {state} ({result.duration_seconds:.2f}s)")
        if "repository_state_changed" in self.failed_checks:
            lines.append("- repository_state_changed: FAIL")
        lines.extend(
            [
                "",
                "LOCAL NON-COMPILE PREFLIGHT ONLY",
                "Rust build: NOT RUN LOCALLY",
                "Rust clippy: NOT RUN LOCALLY",
                "Rust tests: NOT RUN LOCALLY",
                "Runtime behavior proof: NOT RUN LOCALLY",
            ]
        )
        return "\n".join(lines)


Runner = Callable[[str, tuple[str, ...], pathlib.Path], CommandResult]


def _git(repo: pathlib.Path, *args: str) -> bytes:
    result = subprocess.run(
        ["git", "--no-optional-locks", *args],
        cwd=repo,
        capture_output=True,
        check=False,
    )
    if result.returncode != 0:
        detail = result.stderr.decode(errors="replace").strip() or f"exit {result.returncode}"
        raise RegistryError(f"git {' '.join(args)} failed: {detail}")
    return result.stdout


def repository_state_digest(repo: pathlib.Path) -> str:
    indexed = _git(repo, "ls-files", "-v", "-z").split(b"\0")
    hidden = []
    for raw in indexed:
        if not raw:
            continue
        prefix = chr(raw[0])
        if prefix.islower() or prefix == "S":
            hidden.append(raw[2:].decode("utf-8", errors="surrogateescape"))
    if hidden:
        raise RegistryError(f"repository contains hidden index flags: {', '.join(sorted(hidden))}")
    digest = hashlib.sha256()
    digest.update(_git(repo, "rev-parse", "HEAD"))
    digest.update(_git(repo, "rev-parse", "HEAD^{tree}"))
    digest.update(_git(repo, "diff", "--binary", "HEAD", "--"))
    untracked = _git(repo, "ls-files", "--others", "--exclude-standard", "-z").split(b"\0")
    for raw in sorted(path for path in untracked if path):
        relative = raw.decode("utf-8", errors="surrogateescape")
        path = repo / relative
        digest.update(raw)
        if path.is_symlink():
            digest.update(os.readlink(path).encode("utf-8", errors="surrogateescape"))
        elif path.is_file():
            digest.update(path.read_bytes())
    return digest.hexdigest()


def subprocess_runner(operation_id: str, command: tuple[str, ...], repo: pathlib.Path) -> CommandResult:
    start = time.monotonic()
    result = subprocess.run(command, cwd=repo, check=False, close_fds=True)
    return CommandResult(operation_id, result.returncode, time.monotonic() - start)


def ordered_check_ids(registry: WorkspaceRegistry) -> tuple[str, ...]:
    workspaces = sorted(
        registry.workspaces,
        key=lambda workspace: (workspace.path != pathlib.PurePosixPath("."), workspace.workspace_id),
    )
    checks: list[str] = []
    for workspace in workspaces:
        checks.extend(workspace.cheap_checks)
    checks.extend(registry.repository_checks)
    return tuple(checks)


def run_preflight(
    governance: pathlib.Path,
    subject: pathlib.Path,
    *,
    runner: Runner = subprocess_runner,
) -> PreflightReport:
    registry = load_registry(governance)
    validate_operation_recipes(governance)
    reconcile_registry(subject, registry)
    initial_state = repository_state_digest(subject)
    workspace_paths = {
        workspace.workspace_id: subject / workspace.path
        for workspace in registry.workspaces
    }
    results: list[CommandResult] = []
    failed: list[str] = []
    for operation_id in ordered_check_ids(registry):
        operation = CHECK_OPERATIONS[operation_id]
        workspace = workspace_paths.get(operation.workspace_id, subject)
        result = runner(operation_id, operation.render(governance, subject, workspace), subject)
        results.append(result)
        if result.returncode != 0:
            failed.append(operation_id)
    if repository_state_digest(subject) != initial_state:
        failed.append("repository_state_changed")
    return PreflightReport(tuple(results), tuple(failed))


def parse_args(argv: Sequence[str]) -> argparse.Namespace:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--governance", default=".", help="protected governance repository root")
    parser.add_argument("--subject", default=".", help="subject repository root")
    return parser.parse_args(argv)


def main(argv: Sequence[str]) -> int:
    args = parse_args(argv)
    governance = pathlib.Path(args.governance).expanduser().resolve()
    subject = pathlib.Path(args.subject).expanduser().resolve()
    try:
        report = run_preflight(governance, subject)
    except RegistryError as exc:
        print(f"ERROR: {exc}", file=sys.stderr)
        return 2
    print(report.render())
    return report.exit_code


if __name__ == "__main__":
    raise SystemExit(main(sys.argv[1:]))
