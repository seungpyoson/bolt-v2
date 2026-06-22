#!/usr/bin/env python3
"""Self-tests for the local verification front-door gate (#740)."""

from __future__ import annotations

import gc
import importlib.util
import subprocess
import sys
import tempfile
import time
from pathlib import Path


SCRIPTS_DIR = Path(__file__).resolve().parent


def _load(name: str):
    path = SCRIPTS_DIR / f"{name}.py"
    spec = importlib.util.spec_from_file_location(name, path)
    if spec is None or spec.loader is None:
        raise RuntimeError(f"failed to load {path}")
    module = importlib.util.module_from_spec(spec)
    sys.modules[name] = module
    spec.loader.exec_module(module)
    return module


GATE = _load("local_verification_gate")

CHEAP_FRONT_DOOR_GATE = "source-fence-static"
HEAVY_FRONT_DOOR_GATE = "unlisted-heavy-gate"


HOLD_RUNNER = """
import sys, time
from pathlib import Path
sys.path.insert(0, sys.argv[1])
import lane_governor
lock_dir, sentinel = sys.argv[2], sys.argv[3]
lane_governor.acquire(
    "held-local-gate", lock_dir=lock_dir, honor_ci_env=False,
    acquire_timeout_seconds=30, heartbeat_seconds=1, poll_interval_seconds=0.1,
)
Path(sentinel).write_text("held", encoding="utf-8")
time.sleep(10)
"""


def _spawn(code: str, *args: str) -> subprocess.Popen:
    return subprocess.Popen(
        [sys.executable, "-c", code, str(SCRIPTS_DIR), *args],
        text=True,
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
    )


def _wait_for(path: Path, timeout: float = 10.0) -> None:
    deadline = time.monotonic() + timeout
    while time.monotonic() < deadline:
        if path.exists():
            return
        time.sleep(0.05)
    raise AssertionError(f"timed out waiting for {path}")


def _child_marker_command(child_marker: Path) -> list[str]:
    code = (
        "from pathlib import Path; import sys; "
        "Path(sys.argv[1]).write_text('ran', encoding='utf-8')"
    )
    return [sys.executable, "-c", code, str(child_marker)]


def test_busy_heavy_gate_refuses_without_running_child() -> None:
    with tempfile.TemporaryDirectory() as tmp:
        sentinel = Path(tmp) / "held"
        child_marker = Path(tmp) / "child-ran"
        holder = _spawn(HOLD_RUNNER, tmp, str(sentinel))
        _wait_for(sentinel)
        start = time.monotonic()
        try:
            rc = GATE.run_gate(
                HEAVY_FRONT_DOOR_GATE,
                _child_marker_command(child_marker),
                lock_dir=tmp,
                honor_ci_env=False,
            )
        finally:
            holder.kill()
            holder.communicate(timeout=10)
        elapsed = time.monotonic() - start
        assert rc == 1
        assert elapsed < 2.0, f"busy gate must fail fast, took {elapsed:.2f}s"
        assert not child_marker.exists(), "competing gate must not launch child work"


def test_busy_cheap_gate_runs_child_concurrently() -> None:
    with tempfile.TemporaryDirectory() as tmp:
        sentinel = Path(tmp) / "held"
        child_marker = Path(tmp) / "child-ran"
        holder = _spawn(HOLD_RUNNER, tmp, str(sentinel))
        _wait_for(sentinel)
        start = time.monotonic()
        try:
            rc = GATE.run_gate(
                CHEAP_FRONT_DOOR_GATE,
                _child_marker_command(child_marker),
                lock_dir=tmp,
                honor_ci_env=False,
            )
        finally:
            holder.kill()
            holder.communicate(timeout=10)
        elapsed = time.monotonic() - start
        assert rc == 0
        assert elapsed < 2.0, f"cheap gate must not wait for the heavy lane: {elapsed:.2f}s"
        assert child_marker.read_text(encoding="utf-8") == "ran"


def test_child_verifier_reenters_under_parent_gate() -> None:
    with tempfile.TemporaryDirectory() as tmp:
        marker = Path(tmp) / "child-acquired"
        child_code = """
import sys
from pathlib import Path
sys.path.insert(0, sys.argv[1])
import lane_governor
lane_governor.acquire(
    "child-verifier", lock_dir=sys.argv[2], honor_ci_env=False,
    acquire_timeout_seconds=5, heartbeat_seconds=1, poll_interval_seconds=0.1,
)
Path(sys.argv[3]).write_text("ok", encoding="utf-8")
"""
        rc = GATE.run_gate(
            HEAVY_FRONT_DOOR_GATE,
            [sys.executable, "-c", child_code, str(SCRIPTS_DIR), tmp, str(marker)],
            lock_dir=tmp,
            honor_ci_env=False,
        )
        assert rc == 0
        assert marker.read_text(encoding="utf-8") == "ok"


def test_child_verifier_reenters_parent_gate_without_extra_poll_sleep() -> None:
    with tempfile.TemporaryDirectory() as tmp:
        marker = Path(tmp) / "child-elapsed"
        child_code = """
import sys, time
from pathlib import Path
sys.path.insert(0, sys.argv[1])
import lane_governor
t0 = time.monotonic()
lane_governor.acquire(
    "child-verifier", lock_dir=sys.argv[2], honor_ci_env=False,
    acquire_timeout_seconds=5, heartbeat_seconds=5, poll_interval_seconds=1,
)
Path(sys.argv[3]).write_text(str(time.monotonic() - t0), encoding="utf-8")
"""
        rc = GATE.run_gate(
            HEAVY_FRONT_DOOR_GATE,
            [sys.executable, "-c", child_code, str(SCRIPTS_DIR), tmp, str(marker)],
            lock_dir=tmp,
            honor_ci_env=False,
        )
        assert rc == 0
        elapsed = float(marker.read_text(encoding="utf-8"))
        assert elapsed < 0.9, f"child gate re-entry must not sleep for a poll interval: {elapsed:.2f}s"


def test_gate_keeps_acquired_handle_alive_until_child_exits() -> None:
    closed: list[str] = []
    released: list[str] = []

    class Handle:
        def close(self) -> None:
            closed.append("closed")

        def __del__(self) -> None:
            released.append("released")

    def fake_acquire(*args, **kwargs):
        return Handle()

    def fake_run(command, env, check, close_fds):
        gc.collect()
        assert close_fds is True
        assert not released, "gate must retain lane lock handle while child runs"
        assert not closed, "gate must close lane lock handle after child exits"
        return subprocess.CompletedProcess(command, 0)

    original_acquire = GATE.lane_governor.acquire
    original_run = GATE.subprocess.run
    try:
        GATE.lane_governor.acquire = fake_acquire
        GATE.subprocess.run = fake_run
        rc = GATE.run_gate(CHEAP_FRONT_DOOR_GATE, ["child"], honor_ci_env=False)
    finally:
        GATE.subprocess.run = original_run
        GATE.lane_governor.acquire = original_acquire
    assert rc == 0
    assert closed == ["closed"]


def main() -> int:
    tests = [
        test_busy_heavy_gate_refuses_without_running_child,
        test_busy_cheap_gate_runs_child_concurrently,
        test_child_verifier_reenters_under_parent_gate,
        test_child_verifier_reenters_parent_gate_without_extra_poll_sleep,
        test_gate_keeps_acquired_handle_alive_until_child_exits,
    ]
    for test in tests:
        test()
    print("OK: local verification gate self-tests passed.")
    return 0


if __name__ == "__main__":
    import lane_governor

    lane_governor.acquire()
    raise SystemExit(main())
