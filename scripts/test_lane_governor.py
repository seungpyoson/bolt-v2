#!/usr/bin/env python3
"""Self-tests for lane_governor and the local_lane_policy validator (#653)."""

from __future__ import annotations

import errno
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


def test_poll_interval_must_not_exceed_heartbeat() -> None:
    policy = _valid_lane_policy()
    policy["heartbeat_seconds"] = 5
    policy["poll_interval_seconds"] = 6
    _expect_policy_error(
        {"local_lane_policy": policy},
        "poll_interval_seconds must be less than or equal to heartbeat_seconds",
    )


def test_non_positive_intervals_rejected() -> None:
    for key in ("acquire_timeout_seconds", "heartbeat_seconds", "poll_interval_seconds"):
        policy = _valid_lane_policy()
        policy[key] = 0
        _expect_policy_error({"local_lane_policy": policy}, key)


def test_repo_policy_file_declares_lane_policy() -> None:
    data = RV.load_policy(REPO_ROOT)
    assert "local_lane_policy" in data, "ci/rust-verification.toml must declare [local_lane_policy]"


def test_subcrate_lane_policy_matches_repo_policy() -> None:
    data = RV.load_policy(REPO_ROOT)
    subcrate = RV.load_policy(REPO_ROOT / "crates/backtesting-vertical-slice")
    assert subcrate["local_lane_policy"] == data["local_lane_policy"]


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

FAIL_FAST_RUNNER = """
import sys, time
sys.path.insert(0, sys.argv[1])
import lane_governor
lock_dir = sys.argv[2]
t0 = time.monotonic()
lane_governor.acquire(
    "fail-fast-runner", lock_dir=lock_dir, honor_ci_env=False,
    acquire_timeout_seconds=30, heartbeat_seconds=1, poll_interval_seconds=0.1,
    fail_fast=True,
)
print("unexpected-acquired", time.monotonic() - t0)
"""

LOCAL_GATE_HOLD_RUNNER = """
import sys, time
from pathlib import Path
sys.path.insert(0, sys.argv[1])
import lane_governor
lock_dir, sentinel, hold = sys.argv[2], sys.argv[3], float(sys.argv[4])
handle = lane_governor.acquire(
    "local-gate:external", lock_dir=lock_dir, honor_ci_env=False,
    acquire_timeout_seconds=30, heartbeat_seconds=1, poll_interval_seconds=0.1,
)
Path(sentinel).write_text(str(time.time()), encoding="utf-8")
time.sleep(hold)
print("released", time.time())
"""

FORGED_GATE_ENV_RUNNER = """
import os, sys, time
sys.path.insert(0, sys.argv[1])
import lane_governor
lock_dir = sys.argv[2]
os.environ[lane_governor.LOCAL_VERIFICATION_GATE_ENV] = "1"
t0 = time.monotonic()
lane_governor.acquire(
    "forged-gate-env-runner", lock_dir=lock_dir, honor_ci_env=False,
    acquire_timeout_seconds=1, heartbeat_seconds=1, poll_interval_seconds=0.1,
)
print("unexpected-acquired", time.monotonic() - t0)
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

GRANDCHILD_RUNNER = """
import sys, time
sys.path.insert(0, sys.argv[1])
import lane_governor
t0 = time.monotonic()
lane_governor.acquire(
    "grandchild-runner", lock_dir=sys.argv[2], honor_ci_env=False,
    acquire_timeout_seconds=2, heartbeat_seconds=1, poll_interval_seconds=0.1,
)
print("grandchild-done", time.monotonic() - t0)
"""

INTERMEDIATE_CHILD_RUNNER = """
import subprocess, sys
scripts_dir, lock_dir, grandchild_code = sys.argv[1], sys.argv[2], sys.argv[3]
completed = subprocess.run(
    [sys.executable, "-c", grandchild_code, scripts_dir, lock_dir],
    env={"PATH": "/usr/bin:/bin"}, text=True,
    stdout=subprocess.PIPE, stderr=subprocess.PIPE,
)
print("grandchild-rc", completed.returncode)
print(completed.stdout, end="")
sys.stderr.write(completed.stderr)
"""

GRANDPARENT_CHILD_RUNNER = """
import subprocess, sys
sys.path.insert(0, sys.argv[1])
import lane_governor
scripts_dir, lock_dir = sys.argv[1], sys.argv[2]
intermediate_code, grandchild_code = sys.argv[3], sys.argv[4]
handle = lane_governor.acquire(
    "grandparent-runner", lock_dir=lock_dir, honor_ci_env=False,
    acquire_timeout_seconds=30, heartbeat_seconds=1, poll_interval_seconds=0.1,
)
completed = subprocess.run(
    [sys.executable, "-c", intermediate_code, scripts_dir, lock_dir, grandchild_code],
    env={"PATH": "/usr/bin:/bin"}, text=True,
    stdout=subprocess.PIPE, stderr=subprocess.PIPE,
)
print("middle-rc", completed.returncode)
print(completed.stdout, end="")
sys.stderr.write(completed.stderr)
"""

CI_RUNNER = """
import sys, time
sys.path.insert(0, sys.argv[1])
import lane_governor
t0 = time.monotonic()
result = lane_governor.acquire(
    "ci-runner", lock_dir=sys.argv[2],
    acquire_timeout_seconds=20, heartbeat_seconds=1, poll_interval_seconds=0.1,
)
print("ci-result", result is None, time.monotonic() - t0)
"""

CI_FAIL_FAST_RUNNER = """
import sys, time
sys.path.insert(0, sys.argv[1])
import lane_governor
t0 = time.monotonic()
result = lane_governor.acquire(
    "ci-fail-fast-runner", lock_dir=sys.argv[2],
    acquire_timeout_seconds=20, heartbeat_seconds=1, poll_interval_seconds=0.1,
    fail_fast=True,
)
print("ci-fail-fast-result", result is None, time.monotonic() - t0)
"""

CI_FALSE_RUNNER = """
import sys, time
sys.path.insert(0, sys.argv[1])
import lane_governor
t0 = time.monotonic()
result = lane_governor.acquire(
    "ci-false-runner", lock_dir=sys.argv[2],
    acquire_timeout_seconds=1, heartbeat_seconds=1, poll_interval_seconds=0.1,
)
print("ci-false-result", result is None, time.monotonic() - t0)
"""

HELP_RUNNER = """
import sys, time
scripts_dir, lock_dir = sys.argv[1], sys.argv[2]
sys.path.insert(0, scripts_dir)
import lane_governor
sys.argv = ["verify_sample.py", "--help"]
t0 = time.monotonic()
result = lane_governor.acquire(
    "help-runner", lock_dir=lock_dir, honor_ci_env=False,
    acquire_timeout_seconds=20, heartbeat_seconds=1, poll_interval_seconds=0.1,
)
print("help-result", result is None, time.monotonic() - t0)
"""

HELP_BROKEN_REPO_RUNNER = """
import sys, time
from pathlib import Path
scripts_dir, repo_root = sys.argv[1], sys.argv[2]
sys.path.insert(0, scripts_dir)
import lane_governor
lane_governor.REPO_ROOT = Path(repo_root)
sys.argv = ["verify_sample.py", "--help"]
t0 = time.monotonic()
result = lane_governor.acquire("help-runner", honor_ci_env=False)
print("help-result", result is None, time.monotonic() - t0)
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
        assert "acquired" in out
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


def test_fail_fast_refuses_busy_lane_without_queueing() -> None:
    with tempfile.TemporaryDirectory() as tmp:
        sentinel = Path(tmp) / "held"
        holder = _spawn(HOLD_RUNNER, tmp, str(sentinel), "10")
        _wait_for(sentinel)
        start = time.monotonic()
        waiter = _spawn(FAIL_FAST_RUNNER, tmp)
        out, err = waiter.communicate(timeout=10)
        elapsed = time.monotonic() - start
        holder.kill()
        holder.communicate(timeout=10)
        assert waiter.returncode == 1, f"fail-fast waiter must refuse busy lane: {out}"
        assert elapsed < 2.0, f"fail-fast waiter queued for {elapsed:.2f}s"
        assert "already running" in err
        assert "hold-runner" in err
        assert str(holder.pid) in err


def test_release_closes_and_unregisters_held_handle() -> None:
    lane_governor = _load("lane_governor")
    with tempfile.TemporaryDirectory() as tmp:
        baseline = len(lane_governor._HELD_HANDLES)
        handle = lane_governor.acquire("release-runner", lock_dir=tmp, honor_ci_env=False)
        assert handle in lane_governor._HELD_HANDLES
        lane_governor.release(handle)
        assert handle.closed
        assert handle not in lane_governor._HELD_HANDLES

        reacquired = lane_governor.acquire("release-runner-2", lock_dir=tmp, honor_ci_env=False)
        lane_governor.release(reacquired)
        assert len(lane_governor._HELD_HANDLES) == baseline


def test_unrelated_holder_does_not_reenter() -> None:
    with tempfile.TemporaryDirectory() as tmp:
        sentinel = Path(tmp) / "held"
        holder = _spawn(HOLD_RUNNER, tmp, str(sentinel), "10")
        _wait_for(sentinel)
        waiter = _spawn(ONCE_RUNNER, tmp, "1")
        out, err = waiter.communicate(timeout=10)
        holder.kill()
        holder.communicate(timeout=10)
        assert waiter.returncode == 1, f"unrelated holder must not pass through: {out}"
        assert "FAILED to acquire" in err


def test_forged_gate_env_does_not_reenter_unrelated_local_gate_holder() -> None:
    with tempfile.TemporaryDirectory() as tmp:
        sentinel = Path(tmp) / "held"
        holder = _spawn(LOCAL_GATE_HOLD_RUNNER, tmp, str(sentinel), "10")
        _wait_for(sentinel)
        waiter = _spawn(FORGED_GATE_ENV_RUNNER, tmp)
        out, err = waiter.communicate(timeout=10)
        holder.kill()
        holder.communicate(timeout=10)
        assert waiter.returncode == 1, f"unrelated local gate holder must not pass through: {out}"
        assert "FAILED to acquire" in err
        assert "local-gate:external" in err


def test_unexpected_flock_error_fails_immediately() -> None:
    lane_governor = _load("lane_governor")
    with tempfile.TemporaryDirectory() as tmp:
        original_flock = lane_governor.fcntl.flock

        def broken_flock(*_args) -> None:
            raise OSError(errno.EINVAL, "bad file descriptor")

        lane_governor.fcntl.flock = broken_flock
        try:
            try:
                lane_governor.acquire(
                    "broken-flock",
                    lock_dir=tmp,
                    honor_ci_env=False,
                    acquire_timeout_seconds=30,
                    heartbeat_seconds=1,
                    poll_interval_seconds=0.1,
                )
            except OSError as exc:
                assert exc.errno == errno.EINVAL
                return
            raise AssertionError("unexpected flock errors must not be treated as contention")
        finally:
            lane_governor.fcntl.flock = original_flock


def test_scrubbed_env_child_reenters_while_parent_holds() -> None:
    with tempfile.TemporaryDirectory() as tmp:
        proc = _spawn(PARENT_CHILD_RUNNER, tmp)
        out, err = proc.communicate(timeout=40)
        assert proc.returncode == 0, err
        assert "child-rc 0" in out, f"child must succeed, got: {out}\n{err}"
        line = [l for l in out.splitlines() if l.startswith("child-done")][0]
        elapsed = float(line.split()[1])
        assert elapsed < 5.0, f"child must pass through re-entrantly, took {elapsed:.1f}s"


def test_scrubbed_env_grandchild_reenters_while_grandparent_holds() -> None:
    with tempfile.TemporaryDirectory() as tmp:
        proc = _spawn(GRANDPARENT_CHILD_RUNNER, tmp, INTERMEDIATE_CHILD_RUNNER, GRANDCHILD_RUNNER)
        out, err = proc.communicate(timeout=40)
        assert proc.returncode == 0, err
        assert "middle-rc 0" in out, f"intermediate child must succeed, got: {out}\n{err}"
        assert "grandchild-rc 0" in out, f"grandchild must succeed, got: {out}\n{err}"
        line = [l for l in out.splitlines() if l.startswith("grandchild-done")][0]
        elapsed = float(line.split()[1])
        assert elapsed < 5.0, f"grandchild must pass through re-entrantly, took {elapsed:.1f}s"


def test_ci_env_bypasses_lock() -> None:
    with tempfile.TemporaryDirectory() as tmp:
        sentinel = Path(tmp) / "held"
        holder = _spawn(HOLD_RUNNER, tmp, str(sentinel), "10")
        _wait_for(sentinel)
        env = dict(os.environ)
        env["GITHUB_ACTIONS"] = "true"
        ci = _spawn(CI_RUNNER, tmp, env=env)
        out, err = ci.communicate(timeout=20)
        holder.kill()
        holder.communicate(timeout=10)
        assert ci.returncode == 0, err
        flag, elapsed = out.split()[1], float(out.split()[2])
        assert flag == "True", "CI bypass must return None without locking"
        assert elapsed < 5.0, "CI bypass must not wait"


def test_ci_env_bypasses_fail_fast_lock() -> None:
    with tempfile.TemporaryDirectory() as tmp:
        sentinel = Path(tmp) / "held"
        holder = _spawn(HOLD_RUNNER, tmp, str(sentinel), "10")
        _wait_for(sentinel)
        env = dict(os.environ)
        env["GITHUB_ACTIONS"] = "true"
        ci = _spawn(CI_FAIL_FAST_RUNNER, tmp, env=env)
        out, err = ci.communicate(timeout=20)
        holder.kill()
        holder.communicate(timeout=10)
        assert ci.returncode == 0, err
        flag, elapsed = out.split()[1], float(out.split()[2])
        assert flag == "True", "CI bypass must return None before fail-fast lock refusal"
        assert elapsed < 5.0, "CI fail-fast bypass must not wait"


def test_ci_false_env_does_not_bypass_lock() -> None:
    with tempfile.TemporaryDirectory() as tmp:
        sentinel = Path(tmp) / "held"
        holder = _spawn(HOLD_RUNNER, tmp, str(sentinel), "10")
        _wait_for(sentinel)
        env = dict(os.environ)
        env["GITHUB_ACTIONS"] = "false"
        ci = _spawn(CI_FALSE_RUNNER, tmp, env=env)
        out, err = ci.communicate(timeout=10)
        holder.kill()
        holder.communicate(timeout=10)
        assert ci.returncode == 1, f"GITHUB_ACTIONS=false must not bypass the lane lock: {out}"
        assert "FAILED to acquire" in err


def test_help_invocation_bypasses_lock() -> None:
    with tempfile.TemporaryDirectory() as tmp:
        sentinel = Path(tmp) / "held"
        holder = _spawn(HOLD_RUNNER, tmp, str(sentinel), "10")
        _wait_for(sentinel)
        helper = _spawn(HELP_RUNNER, tmp)
        out, err = helper.communicate(timeout=20)
        holder.kill()
        holder.communicate(timeout=10)
        assert helper.returncode == 0, err
        flag, elapsed = out.split()[1], float(out.split()[2])
        assert flag == "True", "--help must not take or wait for the lane lock"
        assert elapsed < 5.0, "--help fast-path must not wait"


def test_help_invocation_bypasses_policy_load() -> None:
    with tempfile.TemporaryDirectory() as tmp:
        broken_repo = Path(tmp) / "missing-policy-repo"
        broken_repo.mkdir()
        helper = _spawn(HELP_BROKEN_REPO_RUNNER, str(broken_repo))
        out, err = helper.communicate(timeout=20)
        assert helper.returncode == 0, err
        flag, elapsed = out.split()[1], float(out.split()[2])
        assert flag == "True", "--help must return None without loading policy"
        assert elapsed < 5.0, "--help fast-path must not wait"


def main() -> int:
    tests = [
        test_valid_lane_policy_passes,
        test_missing_lane_policy_rejected,
        test_disabled_lane_policy_rejected,
        test_relative_lock_dir_rejected,
        test_env_expansion_lock_dir_rejected,
        test_heartbeat_must_be_below_timeout,
        test_poll_interval_must_not_exceed_heartbeat,
        test_non_positive_intervals_rejected,
        test_repo_policy_file_declares_lane_policy,
        test_subcrate_lane_policy_matches_repo_policy,
        test_uncontended_acquire_is_fast,
        test_second_acquire_queues_until_release,
        test_holder_metadata_written,
        test_timeout_fails_loud_with_holder_info,
        test_fail_fast_refuses_busy_lane_without_queueing,
        test_release_closes_and_unregisters_held_handle,
        test_unrelated_holder_does_not_reenter,
        test_forged_gate_env_does_not_reenter_unrelated_local_gate_holder,
        test_unexpected_flock_error_fails_immediately,
        test_scrubbed_env_child_reenters_while_parent_holds,
        test_scrubbed_env_grandchild_reenters_while_grandparent_holds,
        test_ci_env_bypasses_lock,
        test_ci_env_bypasses_fail_fast_lock,
        test_ci_false_env_does_not_bypass_lock,
        test_help_invocation_bypasses_lock,
        test_help_invocation_bypasses_policy_load,
    ]
    for test in tests:
        test()
    print("OK: lane governor self-tests passed.")
    return 0


if __name__ == "__main__":
    import lane_governor

    lane_governor.acquire()
    raise SystemExit(main())
