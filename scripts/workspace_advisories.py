#!/usr/bin/env python3
"""Check advisories for every registered workspace lockfile."""

from __future__ import annotations

import argparse
import pathlib
import subprocess
import sys
from collections.abc import Callable, Sequence

from workspace_registry import RegistryError, load_registry, reconcile_registry


Runner = Callable[[str, tuple[str, ...], pathlib.Path], int]


def subprocess_runner(_workspace_id: str, command: tuple[str, ...], cwd: pathlib.Path) -> int:
    return subprocess.run(command, cwd=cwd, check=False, close_fds=True).returncode


def run_advisories(
    governance: pathlib.Path,
    subject: pathlib.Path,
    *,
    runner: Runner = subprocess_runner,
) -> tuple[tuple[str, int], ...]:
    registry = load_registry(governance)
    reconcile_registry(subject, registry)
    owner = governance / "scripts/rust_verification.py"
    results: list[tuple[str, int]] = []
    for workspace in sorted(registry.workspaces, key=lambda item: item.workspace_id):
        workspace_root = subject / pathlib.Path(workspace.path.as_posix())
        command = (
            "python3",
            str(owner),
            "cargo",
            "--repo",
            str(workspace_root),
            "--",
            "deny",
            "check",
            "advisories",
        )
        results.append((workspace.workspace_id, runner(workspace.workspace_id, command, governance)))
    return tuple(results)


def main(argv: Sequence[str]) -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--governance", default=".")
    parser.add_argument("--subject", default=".")
    args = parser.parse_args(argv)
    governance = pathlib.Path(args.governance).resolve()
    subject = pathlib.Path(args.subject).resolve()
    try:
        results = run_advisories(governance, subject)
    except RegistryError as exc:
        print(f"ERROR: {exc}", file=sys.stderr)
        return 2
    for workspace_id, returncode in results:
        print(f"{workspace_id}: {'PASS' if returncode == 0 else f'FAIL({returncode})'}")
    return 0 if all(returncode == 0 for _, returncode in results) else 1


if __name__ == "__main__":
    raise SystemExit(main(sys.argv[1:]))
