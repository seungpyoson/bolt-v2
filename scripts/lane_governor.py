#!/usr/bin/env python3
"""Per-repo single-flight governor for CPU-heavy local verifier lanes (#653).

Every governed script (scripts/verify_*.py, scripts/test_*.py) calls
``acquire()`` as the first executable statement of its ``__main__`` block.
Policy lives in ci/rust-verification.toml [local_lane_policy]. The lock path
is committed and environment-independent so every checkout, worktree, and
agent harness of this repo contends on the same machine-level file. CI
(allowed_ci_env present) bypasses the lock. A waiter whose lock holder is one
of its own process ancestors proceeds without the lock: the ancestor already
serializes the repo. Coverage is enforced by scripts/verify_lane_governance.py.
"""

from __future__ import annotations

import fcntl
import json
import os
import subprocess
import sys
import time
from pathlib import Path

_SCRIPTS_DIR = Path(__file__).resolve().parent
if str(_SCRIPTS_DIR) not in sys.path:
    sys.path.insert(0, str(_SCRIPTS_DIR))

import rust_verification

REPO_ROOT = _SCRIPTS_DIR.parent

# Handles held for the lifetime of the process; flock releases on exit/kill.
_HELD_HANDLES: list[object] = []


class LaneLockTimeout(SystemExit):
    """Raised (exit code 1) when the lane lock is not acquired in time."""


def _parent_pid(pid: int) -> int | None:
    try:
        completed = subprocess.run(
            ["ps", "-o", "ppid=", "-p", str(pid)],
            text=True,
            stdout=subprocess.PIPE,
            stderr=subprocess.DEVNULL,
            check=False,
        )
    except OSError:
        return None
    raw = completed.stdout.strip()
    if not raw:
        return None
    try:
        value = int(raw)
    except ValueError:
        return None
    return value if value > 0 else None


def holder_is_ancestor(holder_pid: int) -> bool:
    pid: int | None = os.getpid()
    seen: set[int] = set()
    while pid is not None and pid > 1 and pid not in seen:
        if pid == holder_pid:
            return True
        seen.add(pid)
        pid = os.getppid() if pid == os.getpid() else _parent_pid(pid)
    return False


def _read_holder(lock_path: Path) -> dict:
    try:
        payload = json.loads(lock_path.read_text(encoding="utf-8"))
    except (OSError, ValueError):
        return {}
    return payload if isinstance(payload, dict) else {}


def acquire(
    lane: str | None = None,
    *,
    lock_dir: str | os.PathLike[str] | None = None,
    honor_ci_env: bool = True,
    acquire_timeout_seconds: float | None = None,
    heartbeat_seconds: float | None = None,
    poll_interval_seconds: float | None = None,
):
    """Acquire the per-repo lane lock; return the held handle, or None.

    None is returned when governance does not apply: CI environment, or the
    current holder is an ancestor process (re-entrant call).
    """
    policy = rust_verification.load_policy(REPO_ROOT)
    lane_policy = policy["local_lane_policy"]
    if honor_ci_env and os.environ.get(lane_policy["allowed_ci_env"]):
        return None
    if {"-h", "--help"}.intersection(sys.argv[1:]):
        # A help invocation does no heavy work and must never queue behind a
        # multi-minute holder (A4).
        return None
    label = lane or Path(sys.argv[0]).name or "unknown-lane"
    directory = Path(lock_dir) if lock_dir is not None else Path(lane_policy["lock_dir"])
    timeout = (
        acquire_timeout_seconds
        if acquire_timeout_seconds is not None
        else lane_policy["acquire_timeout_seconds"]
    )
    heartbeat = heartbeat_seconds if heartbeat_seconds is not None else lane_policy["heartbeat_seconds"]
    poll = poll_interval_seconds if poll_interval_seconds is not None else lane_policy["poll_interval_seconds"]
    directory.mkdir(parents=True, exist_ok=True)
    lock_path = directory / f"{policy['target_namespace']}.lane.lock"
    handle = open(lock_path, "a+", encoding="utf-8")
    started = time.monotonic()
    last_heartbeat = started
    last_busy_holder_pid: int | None = None
    while True:
        try:
            fcntl.flock(handle.fileno(), fcntl.LOCK_EX | fcntl.LOCK_NB)
        except OSError:
            holder = _read_holder(lock_path)
            holder_pid = holder.get("pid")
            if (
                isinstance(holder_pid, int)
                and holder_pid == last_busy_holder_pid
                and holder_is_ancestor(holder_pid)
            ):
                # Same holder pid observed on two consecutive busy polls (A6):
                # closes the window where a new holder has the flock but has
                # not yet written its metadata, which would otherwise let a
                # waiter pass through on the PREVIOUS holder's pid.
                handle.close()
                return None
            last_busy_holder_pid = holder_pid if isinstance(holder_pid, int) else None
            now = time.monotonic()
            waited = now - started
            if waited >= timeout:
                handle.close()
                print(
                    f"lane-governor: FAILED to acquire {lock_path} after {waited:.0f}s; "
                    f"held by pid {holder.get('pid')} lane {holder.get('lane')!r}. "
                    "Another CPU-heavy local verifier lane is running; retry when it "
                    "finishes, or raise [local_lane_policy].acquire_timeout_seconds.",
                    file=sys.stderr,
                )
                raise LaneLockTimeout(1)
            if now - last_heartbeat >= heartbeat:
                print(
                    f"lane-governor: waiting for {lock_path} held by pid "
                    f"{holder.get('pid')} lane {holder.get('lane')!r} ({waited:.0f}s elapsed)",
                    file=sys.stderr,
                )
                last_heartbeat = now
            time.sleep(poll)
            continue
        handle.seek(0)
        handle.truncate()
        json.dump({"pid": os.getpid(), "lane": label, "started_at": time.time()}, handle)
        handle.write("\n")
        handle.flush()
        _HELD_HANDLES.append(handle)
        return handle
