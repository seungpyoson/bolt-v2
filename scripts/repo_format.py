#!/usr/bin/env python3
"""Run every registered workspace formatter from protected governance."""

from __future__ import annotations

import argparse
import pathlib
import subprocess
import sys
import time
from dataclasses import dataclass
from typing import Callable, Sequence

from workspace_registry import (
    CHECK_OPERATIONS,
    RegistryError,
    load_registry,
    reconcile_registry,
    validate_operation_recipes,
)


@dataclass(frozen=True)
class FormatResult:
    operation_id: str
    returncode: int
    duration_seconds: float


@dataclass(frozen=True)
class FormatReport:
    results: tuple[FormatResult, ...]
    failed_operations: tuple[str, ...]

    @property
    def exit_code(self) -> int:
        return 0 if not self.failed_operations else 1

    def render(self) -> str:
        lines = ["Repository formatter results:"]
        for result in self.results:
            state = "PASS" if result.returncode == 0 else f"FAIL({result.returncode})"
            lines.append(f"- {result.operation_id}: {state} ({result.duration_seconds:.2f}s)")
        return "\n".join(lines)


Runner = Callable[[str, tuple[str, ...], pathlib.Path], int]


def subprocess_runner(_operation_id: str, command: tuple[str, ...], subject: pathlib.Path) -> int:
    return subprocess.run(command, cwd=subject, check=False, close_fds=True).returncode


def run_format(
    governance: pathlib.Path,
    subject: pathlib.Path,
    *,
    runner: Runner = subprocess_runner,
) -> FormatReport:
    registry = load_registry(governance)
    validate_operation_recipes(governance)
    reconcile_registry(subject, registry)
    workspaces = sorted(
        registry.workspaces,
        key=lambda workspace: (workspace.path != pathlib.PurePosixPath("."), workspace.workspace_id),
    )
    results: list[FormatResult] = []
    failed: list[str] = []
    for workspace in workspaces:
        operation_id = workspace.formatter_write
        operation = CHECK_OPERATIONS[operation_id]
        started = time.monotonic()
        workspace_path = subject / workspace.path
        returncode = runner(operation_id, operation.render(governance, subject, workspace_path), subject)
        result = FormatResult(operation_id, returncode, time.monotonic() - started)
        results.append(result)
        if returncode != 0:
            failed.append(operation_id)
    return FormatReport(tuple(results), tuple(failed))


def parse_args(argv: Sequence[str]) -> argparse.Namespace:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--governance", default=".")
    parser.add_argument("--subject", default=".")
    return parser.parse_args(argv)


def main(argv: Sequence[str]) -> int:
    args = parse_args(argv)
    governance = pathlib.Path(args.governance).expanduser().resolve()
    subject = pathlib.Path(args.subject).expanduser().resolve()
    try:
        report = run_format(governance, subject)
    except RegistryError as exc:
        print(f"ERROR: {exc}", file=sys.stderr)
        return 2
    print(report.render())
    return report.exit_code


if __name__ == "__main__":
    raise SystemExit(main(sys.argv[1:]))
