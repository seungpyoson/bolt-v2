#!/usr/bin/env python3
"""Fail-fast front door for repo-local non-compile verification gates (#740)."""

from __future__ import annotations

import os
import shlex
import subprocess
import sys
from collections.abc import Sequence
from pathlib import Path

SCRIPTS_DIR = Path(__file__).resolve().parent
if str(SCRIPTS_DIR) not in sys.path:
    sys.path.insert(0, str(SCRIPTS_DIR))

import lane_governor


GATE_ENV = lane_governor.LOCAL_VERIFICATION_GATE_ENV


def run_gate(
    gate: str,
    command: Sequence[str],
    *,
    lock_dir: str | os.PathLike[str] | None = None,
    honor_ci_env: bool = True,
) -> int:
    if not gate:
        print("local-verification-gate: missing gate name", file=sys.stderr)
        return 2
    if not command:
        print("local-verification-gate: missing command after --", file=sys.stderr)
        return 2
    try:
        held_handle = lane_governor.acquire(
            f"local-gate:{gate}",
            lock_dir=lock_dir,
            honor_ci_env=honor_ci_env,
            fail_fast=True,
        )
    except lane_governor.LaneLockTimeout as exc:
        return int(exc.code or 1)

    rendered = " ".join(shlex.quote(part) for part in command)
    print(f"local-verification-gate: running {gate}: {rendered}", file=sys.stderr)
    env = dict(os.environ)
    env[GATE_ENV] = "1"
    try:
        return subprocess.run(list(command), env=env, check=False, close_fds=True).returncode
    finally:
        lane_governor.release(held_handle)


def main(argv: Sequence[str]) -> int:
    if not argv or "-h" in argv or "--help" in argv:
        print(
            "usage: local_verification_gate.py <gate-name> -- <command> [args...]",
            file=sys.stderr,
        )
        return 0 if argv and {"-h", "--help"}.intersection(argv) else 2
    if "--" not in argv:
        print("local-verification-gate: missing -- command separator", file=sys.stderr)
        return 2
    separator = argv.index("--")
    gate = argv[0]
    command = list(argv[separator + 1 :])
    return run_gate(gate, command)


if __name__ == "__main__":
    raise SystemExit(main(sys.argv[1:]))
