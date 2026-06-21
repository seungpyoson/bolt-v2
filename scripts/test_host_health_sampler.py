#!/usr/bin/env python3
"""Self-tests for the standalone host-health sampler.

Run with ``python3 scripts/test_host_health_sampler.py -v`` for per-test output.

The original suite was a plain function list; it is now a stdlib ``unittest``
suite so each regression has its own named, isolated, ``-v``-visible case. All
prior assertions are preserved as test methods.
"""

from __future__ import annotations

import contextlib
from datetime import datetime
import fcntl
import importlib.util
import io
import json
import os
from pathlib import Path
import subprocess
import sys
import tempfile
import threading
import time
import unittest


REPO_ROOT = Path(__file__).resolve().parents[1]
SCRIPT_PATH = REPO_ROOT / "scripts" / "host_health_sampler.py"


def load_sampler():
    spec = importlib.util.spec_from_file_location("host_health_sampler", SCRIPT_PATH)
    if spec is None or spec.loader is None:
        raise AssertionError(f"could not load sampler from {SCRIPT_PATH}")
    module = importlib.util.module_from_spec(spec)
    sys.modules[spec.name] = module
    spec.loader.exec_module(module)
    return module


def run_sampler(*args: str, cwd: Path | str = REPO_ROOT) -> subprocess.CompletedProcess[str]:
    return subprocess.run(
        [sys.executable, str(SCRIPT_PATH), *args],
        check=False,
        cwd=str(cwd),
        text=True,
        capture_output=True,
    )


class FakeStatvfs:
    f_frsize = 4096
    f_bsize = 8192
    f_blocks = 100
    f_bfree = 20
    f_bavail = 10
    f_files = 1000
    f_favail = 700


EXPECTED_SCHEMA_KEYS = {
    "schema_version",
    "sampled_at",
    "host",
    "platform",
    "service",
    "process",
    "memory",
    "disk",
    "oom_killed",
    "errors",
}


def assert_sample_schema(record: dict[str, object]) -> None:
    if set(record) != EXPECTED_SCHEMA_KEYS:
        raise AssertionError(f"schema keys differ: {sorted(record)}")
    json.dumps(record)


@contextlib.contextmanager
def capture_main(sampler, argv: list[str]):
    """Run ``sampler.main(argv)`` in-process, capturing stdout, stderr and exit.

    Yields a dict that, after the block, holds ``returncode`` (int from main or
    the ``SystemExit`` code), ``stdout`` and ``stderr`` text. ``sys.stdout`` /
    ``sys.stderr`` are restored even if ``main`` calls ``os.dup2`` on fd 1.
    """
    result: dict[str, object] = {}
    out_buf = io.StringIO()
    err_buf = io.StringIO()
    saved_out, saved_err = sys.stdout, sys.stderr
    sys.stdout, sys.stderr = out_buf, err_buf
    try:
        try:
            result["returncode"] = sampler.main(argv)
        except SystemExit as exc:
            result["returncode"] = exc.code
        yield result
    finally:
        sys.stdout, sys.stderr = saved_out, saved_err
        result["stdout"] = out_buf.getvalue()
        result["stderr"] = err_buf.getvalue()


class SamplerBehaviourTests(unittest.TestCase):
    """Existing collector/schema behaviour, ported to unittest."""

    def setUp(self) -> None:
        self.sampler = load_sampler()

    def test_sample_returns_json_serializable_schema(self) -> None:
        record = self.sampler.sample()
        assert_sample_schema(record)

    def test_sample_timestamp_is_utc_and_schema_version_is_two(self) -> None:
        record = self.sampler.sample()
        self.assertEqual(record["schema_version"], 2)
        sampled_at = record["sampled_at"]
        self.assertIsInstance(sampled_at, str)
        self.assertTrue(sampled_at.endswith("+00:00"), sampled_at)
        parsed = datetime.fromisoformat(sampled_at)
        self.assertIsNotNone(parsed.tzinfo)
        self.assertEqual(parsed.utcoffset().total_seconds(), 0)

    def test_oom_derivation_is_corroboration_aware(self) -> None:
        derive = self.sampler.derive_oom_killed
        # systemd-authoritative results stand on their own (cgroup counter unused).
        self.assertIs(derive("oom-kill", None), True)
        self.assertIs(derive("exit-code", None), False)
        self.assertIs(derive("success", 0), False)
        self.assertIsNone(derive(None, None))
        # A clean systemd Result must NOT be flipped to True by a child-process
        # OOM showing up in the cgroup counter (avoid false positives).
        self.assertIs(derive("exit-code", 5), False)
        service = {"result": "exit-code", "exec_main_status": 137}
        self.assertIsNot(derive(service["result"], None), True)

    def test_oom_ambiguous_signal_results_are_never_false(self) -> None:
        # THE BUG FIX: a SIGKILL-from-OOM surfaces as Result="signal"/"core-dump"
        # on systemd <243. Without the cgroup counter it is unknown (None), never
        # a confident False; with a positive cgroup counter it is confirmed True.
        derive = self.sampler.derive_oom_killed
        self.assertIsNone(derive("signal", None))
        self.assertIsNone(derive("signal", 0))
        self.assertIs(derive("signal", 2), True)
        self.assertIs(derive("core-dump", 1), True)
        self.assertIsNone(derive("core-dump", None))
        # Unknown/missing results corroborate the same way.
        self.assertIs(derive("totally-unknown-result", 3), True)
        self.assertIsNone(derive("totally-unknown-result", None))

    def test_cgroup_oom_kills_parse_helper(self) -> None:
        parse = self.sampler.parse_memory_events_oom_kills
        self.assertEqual(parse("low 0\nhigh 0\nmax 0\noom 1\noom_kill 7\n"), 7)
        self.assertIsNone(parse("low 0\nhigh 0\noom 0\n"))
        with self.assertRaises(ValueError):
            parse("oom_kill not-an-int\n")

    def test_cgroup_oom_kills_falls_back_to_v1_path(self) -> None:
        # Only the cgroup-v1 memory.events file exists; the v2 path is absent.
        # The collector must still read the v1 oom_kill counter.
        with tempfile.TemporaryDirectory(prefix="host-health-cgv1.") as temp:
            root = Path(temp)
            v1_dir = root / "memory" / "system.slice" / "bolt-v2.service"
            v1_dir.mkdir(parents=True)
            (v1_dir / "memory.events").write_text("oom 4\noom_kill 9\n", encoding="utf-8")
            saved_v2 = self.sampler.CGROUP_V2_SYSTEM_SLICE
            saved_v1 = self.sampler.CGROUP_V1_MEMORY_SYSTEM_SLICE
            # Point v2 at a guaranteed-absent dir and v1 at the fake tree.
            self.sampler.CGROUP_V2_SYSTEM_SLICE = root / "unified" / "system.slice"
            self.sampler.CGROUP_V1_MEMORY_SYSTEM_SLICE = root / "memory" / "system.slice"
            try:
                value, error = self.sampler.collect_cgroup_oom_kills("bolt-v2")
            finally:
                self.sampler.CGROUP_V2_SYSTEM_SLICE = saved_v2
                self.sampler.CGROUP_V1_MEMORY_SYSTEM_SLICE = saved_v1
            self.assertEqual(value, 9, error)
            self.assertIsNone(error)

    def test_cgroup_oom_kills_v2_path_preferred_over_v1(self) -> None:
        # When both exist, the v2 (unified) counter wins — no v1 regression of the
        # real cgroup-v2 deploy.
        with tempfile.TemporaryDirectory(prefix="host-health-cgv2.") as temp:
            root = Path(temp)
            v2_dir = root / "unified" / "system.slice" / "bolt-v2.service"
            v2_dir.mkdir(parents=True)
            (v2_dir / "memory.events").write_text("oom_kill 3\n", encoding="utf-8")
            v1_dir = root / "memory" / "system.slice" / "bolt-v2.service"
            v1_dir.mkdir(parents=True)
            (v1_dir / "memory.events").write_text("oom_kill 99\n", encoding="utf-8")
            saved_v2 = self.sampler.CGROUP_V2_SYSTEM_SLICE
            saved_v1 = self.sampler.CGROUP_V1_MEMORY_SYSTEM_SLICE
            self.sampler.CGROUP_V2_SYSTEM_SLICE = root / "unified" / "system.slice"
            self.sampler.CGROUP_V1_MEMORY_SYSTEM_SLICE = root / "memory" / "system.slice"
            try:
                value, error = self.sampler.collect_cgroup_oom_kills("bolt-v2")
            finally:
                self.sampler.CGROUP_V2_SYSTEM_SLICE = saved_v2
                self.sampler.CGROUP_V1_MEMORY_SYSTEM_SLICE = saved_v1
            self.assertEqual(value, 3, error)
            self.assertIsNone(error)

    def test_cgroup_identity_is_exact_segment_not_substring(self) -> None:
        match = self.sampler.cgroup_text_matches_unit
        self.assertIs(match("0::/system.slice/bolt-v2.service", "bolt-v2"), True)
        self.assertIs(match("0::/system.slice/bolt-v2-replica.service", "bolt-v2"), False)
        self.assertIs(match("5:memory:/system.slice/bolt-v2.service", "bolt-v2"), True)
        self.assertIs(match("0::/system.slice/bolt-v2.service", "bolt-v2.service"), True)

    def test_disk_math_uses_bavail_frsize_and_df_denominator(self) -> None:
        metrics = self.sampler.disk_metrics_from_statvfs("/data", FakeStatvfs(), 123)
        self.assertEqual(metrics["free_bytes"], FakeStatvfs.f_bavail * FakeStatvfs.f_frsize)
        used = (FakeStatvfs.f_blocks - FakeStatvfs.f_bfree) * FakeStatvfs.f_frsize
        avail = FakeStatvfs.f_bavail * FakeStatvfs.f_frsize
        expected_pct = round(used / (used + avail) * 100, 2)
        wrong_total_pct = round(used / (FakeStatvfs.f_blocks * FakeStatvfs.f_frsize) * 100, 2)
        self.assertNotEqual(
            expected_pct, wrong_total_pct, "fixture must distinguish df denominator from total"
        )
        self.assertEqual(metrics["used_pct"], expected_pct)

    def test_unknown_service_still_emits_valid_json_and_exits_zero(self) -> None:
        result = run_sampler("--service", "definitely-not-a-real-unit-xyz")
        self.assertEqual(result.returncode, 0, result.stderr)
        lines = result.stdout.splitlines()
        self.assertEqual(len(lines), 1, result.stdout)
        record = json.loads(lines[0])
        assert_sample_schema(record)
        service = record["service"]
        if service is not None:
            load_state = service.get("load_state")
            non_null = [v for k, v in service.items() if k != "unit" and v is not None]
            if load_state != "not-found" and non_null:
                raise AssertionError(f"unexpected service data for absent service: {service!r}")

    def test_failed_collector_becomes_null_error_without_exception(self) -> None:
        original = self.sampler.collect_memory

        def fail_memory():
            raise RuntimeError("synthetic memory failure")

        self.sampler.collect_memory = fail_memory
        try:
            record = self.sampler.sample()
        finally:
            self.sampler.collect_memory = original
        self.assertIsNone(record["memory"])
        self.assertTrue(
            any("memory: synthetic memory failure" in e for e in record["errors"]),
            record["errors"],
        )

    def test_disk_path_tempdir_populates_disk_block(self) -> None:
        with tempfile.TemporaryDirectory(prefix="host-health-disk.") as temp:
            record = self.sampler.sample(disk_path=temp)
            disk = record["disk"]
            self.assertEqual(disk["path"], str(Path(temp).resolve()))
            for key in ("total_bytes", "free_bytes", "used_pct"):
                self.assertIsNotNone(disk[key], disk)

    def test_help_works_from_tmp(self) -> None:
        result = run_sampler("--help", cwd=Path("/tmp"))
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertIn("usage:", result.stdout)

    def test_no_args_from_tmp_prints_exactly_one_valid_json_object(self) -> None:
        result = run_sampler(cwd=Path("/tmp"))
        self.assertEqual(result.returncode, 0, result.stderr)
        lines = result.stdout.splitlines()
        self.assertEqual(len(lines), 1, result.stdout)
        record = json.loads(lines[0])
        assert_sample_schema(record)


class WriteSideHardeningTests(unittest.TestCase):
    """CLASS B regressions (#884): the sampler must never crash/hang/exit non-zero
    on the degraded conditions it exists to observe."""

    def setUp(self) -> None:
        self.sampler = load_sampler()

    # B1 -----------------------------------------------------------------
    def test_interval_nan_rejected(self) -> None:
        # Pre-fix: float("nan") parses, main() emits a record then crashes with
        # exit 1 on time.sleep(nan). Post-fix: argparse rejects it (exit 2) with
        # no record emitted.
        with capture_main(self.sampler, ["--interval", "nan"]) as result:
            pass
        self.assertEqual(result["returncode"], 2)
        self.assertEqual(result["stdout"], "", "no record may be emitted on a config error")

    def test_interval_inf_rejected(self) -> None:
        # Guarded against hang: assert the validator/parse_args rejects "inf"
        # directly rather than entering the loop (pre-fix: min(inf,0.5) spins
        # forever). The custom argparse type raises SystemExit(2).
        with contextlib.redirect_stderr(io.StringIO()):
            with self.assertRaises(SystemExit) as ctx:
                self.sampler.parse_args(["--interval", "inf"])
        self.assertEqual(ctx.exception.code, 2)
        with self.assertRaises(self.sampler.argparse.ArgumentTypeError):
            self.sampler.nonnegative_finite_float("inf")

    def test_interval_negative_rejected(self) -> None:
        # Pre-fix: -1 parses, main() emits one record and exits 0.
        with capture_main(self.sampler, ["--interval", "-1"]) as result:
            pass
        self.assertEqual(result["returncode"], 2)
        self.assertEqual(result["stdout"], "")

    # B3 -----------------------------------------------------------------
    def test_unwritable_out_falls_back_to_stdout(self) -> None:
        # Parent of --out is an existing FILE, so path.parent.mkdir raises
        # deterministically (NotADirectoryError on Linux, FileExistsError on
        # macOS — both OSError). Pre-fix: exit 1, nothing on stdout.
        with tempfile.TemporaryDirectory(prefix="host-health-b3.") as temp:
            parent_file = Path(temp) / "parent_is_a_file"
            parent_file.write_text("not a directory")
            target = parent_file / "child.jsonl"
            with capture_main(self.sampler, ["--out", str(target)]) as result:
                pass
        self.assertEqual(result["returncode"], 0, result["stderr"])
        record = json.loads(result["stdout"].strip())
        assert_sample_schema(record)
        self.assertIn("fell back to stdout", result["stderr"])

    def test_out_is_directory_falls_back_to_stdout(self) -> None:
        # --out is an existing directory -> os.open raises IsADirectoryError.
        # Pre-fix: exit 1, nothing on stdout.
        with tempfile.TemporaryDirectory(prefix="host-health-b3dir.") as temp:
            with capture_main(self.sampler, ["--out", temp]) as result:
                pass
        self.assertEqual(result["returncode"], 0, result["stderr"])
        record = json.loads(result["stdout"].strip())
        assert_sample_schema(record)
        self.assertIn("fell back to stdout", result["stderr"])

    # B2 -----------------------------------------------------------------
    def test_held_lock_falls_back_within_deadline(self) -> None:
        # Hold an exclusive flock on the target, then write with a tiny
        # lock-timeout. Pre-fix: blocks forever. Post-fix: returns the fallback
        # warning, emits to stdout, and completes well within a couple seconds.
        with tempfile.TemporaryDirectory(prefix="host-health-b2.") as temp:
            target = Path(temp) / "locked.jsonl"
            hold_fd = os.open(target, os.O_WRONLY | os.O_CREAT | os.O_APPEND, 0o644)
            fcntl.flock(hold_fd, fcntl.LOCK_EX)
            out_buf = io.StringIO()
            box: dict[str, object] = {}

            def call() -> None:
                saved = sys.stdout
                sys.stdout = out_buf
                try:
                    started = time.monotonic()
                    box["warning"] = self.sampler.write_jsonl_line(
                        {"probe": "held-lock"}, str(target), lock_timeout=0.2
                    )
                    box["elapsed"] = time.monotonic() - started
                finally:
                    sys.stdout = saved
                box["done"] = True

            worker = threading.Thread(target=call, daemon=True)
            worker.start()
            worker.join(timeout=3.0)
            try:
                self.assertTrue(box.get("done"), "writer blocked past the bounded deadline")
                self.assertLess(box["elapsed"], 2.0, "writer did not honour the lock timeout")
                self.assertIsInstance(box.get("warning"), str)
                self.assertIn("fell back to stdout", box["warning"])
                record = json.loads(out_buf.getvalue().strip())
                assert_sample_schema_minimal(record)
            finally:
                fcntl.flock(hold_fd, fcntl.LOCK_UN)
                os.close(hold_fd)

    # B4 -----------------------------------------------------------------
    def test_stdout_brokenpipe_exits_cleanly(self) -> None:
        # A gone consumer (stdout.write raises BrokenPipeError) is a normal
        # stream termination. Pre-fix: caught by broad except -> exit 1.
        class BrokenStdout:
            def write(self, _data: str) -> int:
                raise BrokenPipeError("simulated gone consumer")

            def flush(self) -> None:
                raise BrokenPipeError("simulated gone consumer")

        saved_out = sys.stdout
        sys.stdout = BrokenStdout()  # type: ignore[assignment]
        try:
            returncode = self.sampler.main(["--interval", "0.1"])
        finally:
            sys.stdout = saved_out
        self.assertEqual(returncode, 0)

    # B5 -----------------------------------------------------------------
    def test_interval_mode_survives_transient_write_failure(self) -> None:
        # In interval mode a per-sample emit failure must be logged and the loop
        # must continue. Pre-fix: first failure exits 1.
        calls = {"writes": 0, "sleeps": 0}
        original_write = self.sampler.write_jsonl_line
        original_sleep = self.sampler.sleep_until_next_sample

        def failing_write(record, out_path, **kwargs):
            calls["writes"] += 1
            raise self.sampler.RecordUnemittable("synthetic transient write failure")

        def stopping_sleep(seconds, stop_flag):
            calls["sleeps"] += 1
            if calls["sleeps"] >= 2:
                stop_flag.stop = True

        self.sampler.write_jsonl_line = failing_write
        self.sampler.sleep_until_next_sample = stopping_sleep
        try:
            with capture_main(self.sampler, ["--interval", "0.01"]) as result:
                pass
        finally:
            self.sampler.write_jsonl_line = original_write
            self.sampler.sleep_until_next_sample = original_sleep
        self.assertEqual(result["returncode"], 0)
        self.assertGreaterEqual(calls["writes"], 2, "daemon must survive past the first failure")
        self.assertIn("synthetic transient write failure", result["stderr"])


def assert_sample_schema_minimal(record: dict[str, object]) -> None:
    """Schema check for hand-built probe records (no full collector run)."""
    json.dumps(record)
    if not isinstance(record, dict):
        raise AssertionError(f"record is not a dict: {record!r}")


if __name__ == "__main__":
    unittest.main()
