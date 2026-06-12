#!/usr/bin/env python3
"""Self-tests for lane_governor and the local_lane_policy validator (#653)."""

from __future__ import annotations

import importlib.util
import json
import os
import subprocess
import sys
import tempfile
import time
from pathlib import Path


def _load(name: str):
    path = Path(__file__).with_name(f"{name}.py")
    spec = importlib.util.spec_from_file_location(name, path)
    if spec is None or spec.loader is None:
        raise RuntimeError(f"failed to load {path}")
    module = importlib.util.module_from_spec(spec)
    sys.modules[name] = module
    spec.loader.exec_module(module)
    return module


RV = _load("rust_verification")
REPO_ROOT = Path(__file__).resolve().parent.parent
SCRIPTS_DIR = Path(__file__).resolve().parent


def _valid_lane_policy() -> dict:
    return {
        "enabled": True,
        "allowed_ci_env": "GITHUB_ACTIONS",
        "lock_dir": "/tmp/rust-verification-lanes",
        "acquire_timeout_seconds": 900,
        "heartbeat_seconds": 15,
        "poll_interval_seconds": 1,
    }


def _expect_policy_error(data: dict, fragment: str) -> None:
    try:
        RV.validate_local_lane_policy(data)
    except RV.PolicyError as exc:
        assert fragment in str(exc), f"expected {fragment!r} in {exc}"
        return
    raise AssertionError(f"expected PolicyError containing {fragment!r}")


def test_valid_lane_policy_passes() -> None:
    RV.validate_local_lane_policy({"local_lane_policy": _valid_lane_policy()})


def test_missing_lane_policy_rejected() -> None:
    _expect_policy_error({}, "local_lane_policy table is required")


def test_disabled_lane_policy_rejected() -> None:
    policy = _valid_lane_policy()
    policy["enabled"] = False
    _expect_policy_error({"local_lane_policy": policy}, "enabled must be true")


def test_relative_lock_dir_rejected() -> None:
    policy = _valid_lane_policy()
    policy["lock_dir"] = "var/lanes"
    _expect_policy_error({"local_lane_policy": policy}, "absolute path")


def test_env_expansion_lock_dir_rejected() -> None:
    for bad in ("/tmp/$USER/lanes", "~/lanes"):
        policy = _valid_lane_policy()
        policy["lock_dir"] = bad
        _expect_policy_error({"local_lane_policy": policy}, "must not contain")


def test_heartbeat_must_be_below_timeout() -> None:
    policy = _valid_lane_policy()
    policy["heartbeat_seconds"] = 900
    _expect_policy_error({"local_lane_policy": policy}, "less than acquire_timeout_seconds")


def test_non_positive_intervals_rejected() -> None:
    for key in ("acquire_timeout_seconds", "heartbeat_seconds", "poll_interval_seconds"):
        policy = _valid_lane_policy()
        policy[key] = 0
        _expect_policy_error({"local_lane_policy": policy}, key)


def test_repo_policy_file_declares_lane_policy() -> None:
    data = RV.load_policy(REPO_ROOT)
    assert "local_lane_policy" in data, "ci/rust-verification.toml must declare [local_lane_policy]"


# Subprocess runner: acquire, write a sentinel, hold for --hold seconds, exit.
HOLD_RUNNER = """
import sys, time
from pathlib import Path
sys.path.insert(0, sys.argv[1])
import lane_governor
lock_dir, sentinel, hold = sys.argv[2], sys.argv[3], float(sys.argv[4])
handle = lane_governor.acquire(
    "hold-runner", lock_dir=lock_dir, honor_ci_env=False,
    acquire_timeout_seconds=30, heartbeat_seconds=1, poll_interval_seconds=0.1,
)
Path(sentinel).write_text(str(time.time()), encoding="utf-8")
time.sleep(hold)
print("released", time.time())
"""

# Subprocess runner: acquire once, print acquisition wall time, exit immediately.
ONCE_RUNNER = """
import sys, time
sys.path.insert(0, sys.argv[1])
import lane_governor
lock_dir, timeout = sys.argv[2], float(sys.argv[3])
handle = lane_governor.acquire(
    "once-runner", lock_dir=lock_dir, honor_ci_env=False,
    acquire_timeout_seconds=timeout, heartbeat_seconds=1, poll_interval_seconds=0.1,
)
print("acquired", time.time())
"""

# Parent: acquire, then spawn a child runner WITH A SCRUBBED ENV that attempts
# acquire on the same lock dir. The child must pass through (ancestor holds).
PARENT_CHILD_RUNNER = """
import os, subprocess, sys, time
from pathlib import Path
sys.path.insert(0, sys.argv[1])
import lane_governor
scripts_dir, lock_dir = sys.argv[1], sys.argv[2]
handle = lane_governor.acquire(
    "parent-runner", lock_dir=lock_dir, honor_ci_env=False,
    acquire_timeout_seconds=30, heartbeat_seconds=1, poll_interval_seconds=0.1,
)
child_code = (
    "import sys, time; sys.path.insert(0, sys.argv[1]); import lane_governor; "
    "t0 = time.monotonic(); "
    "lane_governor.acquire('child-runner', lock_dir=sys.argv[2], honor_ci_env=False, "
    "acquire_timeout_seconds=20, heartbeat_seconds=1, poll_interval_seconds=0.1); "
    "print('child-done', time.monotonic() - t0)"
)
scrubbed = {"PATH": "/usr/bin:/bin"}
completed = subprocess.run(
    [sys.executable, "-c", child_code, scripts_dir, lock_dir],
    env=scrubbed, text=True, stdout=subprocess.PIPE, stderr=subprocess.PIPE,
)
print("child-rc", completed.returncode)
print(completed.stdout, end="")
sys.stderr.write(completed.stderr)
"""


def _spawn(snippet: str, *args: str, env: dict | None = None) -> subprocess.Popen:
    return subprocess.Popen(
        [sys.executable, "-c", snippet, str(SCRIPTS_DIR), *args],
        text=True, stdout=subprocess.PIPE, stderr=subprocess.PIPE, env=env,
    )


def _wait_for(path: Path, timeout: float = 10.0) -> None:
    deadline = time.monotonic() + timeout
    while time.monotonic() < deadline:
        if path.exists():
            return
        time.sleep(0.05)
    raise AssertionError(f"sentinel {path} never appeared")


def test_uncontended_acquire_is_fast() -> None:
    with tempfile.TemporaryDirectory() as tmp:
        start = time.monotonic()
        proc = _spawn(ONCE_RUNNER, tmp, "30")
        out, err = proc.communicate(timeout=20)
        assert proc.returncode == 0, err
        assert "acquired" in out
        assert time.monotonic() - start < 10, "uncontended acquire must not wait"


def test_second_acquire_queues_until_release() -> None:
    with tempfile.TemporaryDirectory() as tmp:
        sentinel = Path(tmp) / "held"
        holder = _spawn(HOLD_RUNNER, tmp, str(sentinel), "3")
        _wait_for(sentinel)
        t0 = time.monotonic()
        waiter = _spawn(ONCE_RUNNER, tmp, "30")
        out, err = waiter.communicate(timeout=30)
        waited = time.monotonic() - t0
        holder.communicate(timeout=10)
        assert waiter.returncode == 0, err
        assert waited >= 2.0, f"waiter should queue behind holder, waited only {waited:.2f}s"


def test_holder_metadata_written() -> None:
    with tempfile.TemporaryDirectory() as tmp:
        sentinel = Path(tmp) / "held"
        holder = _spawn(HOLD_RUNNER, tmp, str(sentinel), "3")
        _wait_for(sentinel)
        data = RV.load_policy(REPO_ROOT)
        lock_path = Path(tmp) / f"{data['target_namespace']}.lane.lock"
        payload = json.loads(lock_path.read_text(encoding="utf-8"))
        holder.communicate(timeout=10)
        assert payload["pid"] == holder.pid
        assert payload["lane"] == "hold-runner"


def test_timeout_fails_loud_with_holder_info() -> None:
    with tempfile.TemporaryDirectory() as tmp:
        sentinel = Path(tmp) / "held"
        holder = _spawn(HOLD_RUNNER, tmp, str(sentinel), "15")
        _wait_for(sentinel)
        waiter = _spawn(ONCE_RUNNER, tmp, "2")
        out, err = waiter.communicate(timeout=30)
        holder.kill()
        holder.communicate(timeout=10)
        assert waiter.returncode == 1, f"expected exit 1, got {waiter.returncode}"
        assert "FAILED to acquire" in err
        assert "hold-runner" in err, "timeout message must name the holding lane"
        assert str(holder.pid) in err, "timeout message must name the holding pid"


def test_scrubbed_env_child_reenters_while_parent_holds() -> None:
    with tempfile.TemporaryDirectory() as tmp:
        proc = _spawn(PARENT_CHILD_RUNNER, tmp)
        out, err = proc.communicate(timeout=40)
        assert proc.returncode == 0, err
        assert "child-rc 0" in out, f"child must succeed, got: {out}\n{err}"
        line = [l for l in out.splitlines() if l.startswith("child-done")][0]
        elapsed = float(line.split()[1])
        assert elapsed < 5.0, f"child must pass through re-entrantly, took {elapsed:.1f}s"


def main() -> int:
    tests = [
        test_valid_lane_policy_passes,
        test_missing_lane_policy_rejected,
        test_disabled_lane_policy_rejected,
        test_relative_lock_dir_rejected,
        test_env_expansion_lock_dir_rejected,
        test_heartbeat_must_be_below_timeout,
        test_non_positive_intervals_rejected,
        test_repo_policy_file_declares_lane_policy,
        test_uncontended_acquire_is_fast,
        test_second_acquire_queues_until_release,
        test_holder_metadata_written,
        test_timeout_fails_loud_with_holder_info,
        test_scrubbed_env_child_reenters_while_parent_holds,
    ]
    for test in tests:
        test()
    print("OK: lane governor self-tests passed.")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
