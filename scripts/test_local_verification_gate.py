#!/usr/bin/env python3
"""Self-tests for the local verification front-door gate (#740)."""

from __future__ import annotations

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


def test_busy_gate_refuses_without_running_child() -> None:
    with tempfile.TemporaryDirectory() as tmp:
        sentinel = Path(tmp) / "held"
        child_marker = Path(tmp) / "child-ran"
        holder = _spawn(HOLD_RUNNER, tmp, str(sentinel))
        _wait_for(sentinel)
        start = time.monotonic()
        try:
            code = (
                "from pathlib import Path; import sys; "
                "Path(sys.argv[1]).write_text('ran', encoding='utf-8')"
            )
            rc = GATE.run_gate(
                "source-fence-static",
                [sys.executable, "-c", code, str(child_marker)],
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
            "source-fence-static",
            [sys.executable, "-c", child_code, str(SCRIPTS_DIR), tmp, str(marker)],
            lock_dir=tmp,
            honor_ci_env=False,
        )
        assert rc == 0
        assert marker.read_text(encoding="utf-8") == "ok"


def main() -> int:
    tests = [
        test_busy_gate_refuses_without_running_child,
        test_child_verifier_reenters_under_parent_gate,
    ]
    for test in tests:
        test()
    print("OK: local verification gate self-tests passed.")
    return 0


if __name__ == "__main__":
    import lane_governor

    lane_governor.acquire()
    raise SystemExit(main())
