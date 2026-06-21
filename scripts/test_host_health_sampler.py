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
from types import SimpleNamespace
import typing
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
        # Only the cgroup-v1 counter file exists; the v2 path is absent. The
        # collector must still read the v1 oom_kill counter. FIX C: cgroup-v1
        # exposes oom_kill in memory.oom_control (NOT memory.events), so the v1
        # fixture is laid down as memory.oom_control with the systemd-style body.
        with tempfile.TemporaryDirectory(prefix="host-health-cgv1.") as temp:
            root = Path(temp)
            v1_dir = root / "memory" / "system.slice" / "bolt-v2.service"
            v1_dir.mkdir(parents=True)
            (v1_dir / "memory.oom_control").write_text(
                "oom_kill_disable 0\nunder_oom 0\noom_kill 9\n", encoding="utf-8"
            )
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
            # FIX C: the v1 counter lives in memory.oom_control. Lay it down with a
            # divergent value so a v2-over-v1 regression (reading v1) is caught.
            v1_dir = root / "memory" / "system.slice" / "bolt-v2.service"
            v1_dir.mkdir(parents=True)
            (v1_dir / "memory.oom_control").write_text("oom_kill 99\n", encoding="utf-8")
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


class ReviewRound2HardeningTests(unittest.TestCase):
    """PR #886 review round 2 (#884): partial-write loss, non-strict JSON,
    stderr-flips-exit, py3.8 import, UnicodeDecodeError, pid-recycle TOCTOU,
    and catalog-path single-source discovery."""

    def setUp(self) -> None:
        self.sampler = load_sampler()

    # ITEM 1 -------------------------------------------------------------
    def test_partial_os_write_rolls_back_to_clean_boundary(self) -> None:
        # A short os.write (writes a PREFIX then can make no further progress)
        # must surface as OSError so write_jsonl_line falls back to stdout instead
        # of silently truncating the JSONL line. Pre-fix: the single os.write call
        # ignored its short return -> truncation, no exception, no fallback.
        #
        # F1 hardening: the partial bytes must NOT survive on disk. The writer
        # rolls the file back (ftruncate) to the last clean record boundary so the
        # next O_APPEND cannot glue onto a fragment. Here the boundary is one
        # previously-written complete record; after the failed partial write the
        # file must still contain ONLY that record, with no trailing fragment, and
        # every line must parse as JSON.
        captured: dict[str, object] = {}
        # Capture the real os.write BEFORE patching: patching self.sampler.os.write
        # mutates the shared os module, so a bare os.write inside short_write would
        # re-enter short_write instead of landing a byte.
        real_write = os.write

        def short_write(fd, data):
            # Write only the first byte, then refuse further progress (return 0).
            if not captured.get("first_done"):
                captured["first_done"] = True
                real_write(fd, data[:1])  # land a genuine prefix on disk
                return 1
            return 0  # no progress -> writer must raise

        with tempfile.TemporaryDirectory(prefix="host-health-item1.") as temp:
            target = Path(temp) / "out.jsonl"
            # Lay down one CLEAN complete record first; this is the boundary the
            # rollback must preserve.
            self.sampler.write_to_file(
                '{"probe":"first"}\n', str(target), lock_timeout=0.2
            )
            boundary_bytes = target.read_bytes()

            saved_write = self.sampler.os.write
            self.sampler.os.write = short_write
            try:
                with self.assertRaises(OSError):
                    self.sampler.write_to_file(
                        '{"probe":"item1"}\n', str(target), lock_timeout=0.2
                    )
            finally:
                self.sampler.os.write = saved_write

            # The file was rolled back to the clean boundary: it holds ONLY the
            # previously-written complete record(s), with NO trailing fragment.
            on_disk = target.read_bytes()
            self.assertEqual(on_disk, boundary_bytes)
            # Every line in the file parses (no corruption).
            for raw in on_disk.decode("utf-8").splitlines():
                json.loads(raw)
            self.assertEqual(
                [json.loads(r) for r in on_disk.decode("utf-8").splitlines()],
                [{"probe": "first"}],
            )

    def test_next_append_after_partial_write_stays_parseable(self) -> None:
        # The real downstream regression: after a partial-write failure rolls the
        # file back, the NEXT sample appends a fresh complete record and EVERY line
        # in the file must still parse. Pre-fix (fragment left behind) the next
        # O_APPEND glues onto the fragment, producing one unparseable concatenated
        # line.
        captured: dict[str, object] = {}
        # Capture the real os.write before patching (see sibling test above).
        real_write = os.write

        def short_write(fd, data):
            if not captured.get("first_done"):
                captured["first_done"] = True
                real_write(fd, data[:1])  # land a genuine prefix on disk
                return 1
            return 0  # no progress -> writer must raise

        with tempfile.TemporaryDirectory(prefix="host-health-item1c.") as temp:
            target = Path(temp) / "out.jsonl"
            self.sampler.write_to_file(
                '{"probe":"first"}\n', str(target), lock_timeout=0.2
            )

            saved_write = self.sampler.os.write
            self.sampler.os.write = short_write
            try:
                with self.assertRaises(OSError):
                    self.sampler.write_to_file(
                        '{"probe":"partial"}\n', str(target), lock_timeout=0.2
                    )
            finally:
                self.sampler.os.write = saved_write

            # Next sample: a fresh COMPLETE record with the real os.write restored.
            self.sampler.write_to_file(
                '{"probe":"second"}\n', str(target), lock_timeout=0.2
            )

            lines = target.read_bytes().decode("utf-8").splitlines()
            parsed = [json.loads(raw) for raw in lines]  # raises on any corruption
            self.assertIn({"probe": "second"}, parsed)
            self.assertEqual(parsed, [{"probe": "first"}, {"probe": "second"}])

    def test_partial_write_makes_caller_fall_back_to_stdout(self) -> None:
        # End-to-end: with a short os.write, write_jsonl_line must emit the FULL
        # line to stdout (never a truncated fragment) and return the fallback
        # warning so exit stays 0.
        state = {"calls": 0}
        real_write = os.write

        def short_then_zero(fd, data):
            # Only sabotage the regular-file sink (fd != 1); let stdout writes
            # (item-2 path uses sys.stdout.write, not os.write) be unaffected.
            state["calls"] += 1
            if state["calls"] == 1:
                real_write(fd, data[:1])
                return 1
            return 0

        with tempfile.TemporaryDirectory(prefix="host-health-item1b.") as temp:
            target = Path(temp) / "out.jsonl"
            out_buf = io.StringIO()
            saved_write = self.sampler.os.write
            saved_stdout = sys.stdout
            self.sampler.os.write = short_then_zero
            sys.stdout = out_buf
            try:
                warning = self.sampler.write_jsonl_line(
                    {"probe": "item1b"}, str(target), lock_timeout=0.2
                )
            finally:
                self.sampler.os.write = saved_write
                sys.stdout = saved_stdout
        self.assertIsInstance(warning, str)
        self.assertIn("fell back to stdout", warning)
        # Full, untruncated record on stdout.
        record = json.loads(out_buf.getvalue().strip())
        self.assertEqual(record["probe"], "item1b")

    def test_baseexception_mid_write_rolls_back_to_clean_boundary(self) -> None:
        # A BaseException raised after a real prefix lands must still roll back
        # the file before re-raising. Pre-fix: except OSError missed this path,
        # leaving the prefix on disk for the next O_APPEND to glue onto.
        state = {"calls": 0}
        real_write = os.write

        def prefix_then_interrupt(fd, data):
            state["calls"] += 1
            if state["calls"] == 1:
                prefix = data[:1]
                real_write(fd, prefix)
                return len(prefix)
            raise KeyboardInterrupt("simulated interrupt after partial write")

        with tempfile.TemporaryDirectory(prefix="host-health-fix-c.") as temp:
            target = Path(temp) / "out.jsonl"
            self.sampler.write_to_file(
                '{"probe":"first"}\n', str(target), lock_timeout=0.2
            )
            clean_boundary = target.stat().st_size

            saved_write = self.sampler.os.write
            self.sampler.os.write = prefix_then_interrupt
            try:
                with self.assertRaises(KeyboardInterrupt):
                    self.sampler.write_to_file(
                        '{"probe":"interrupted"}\n', str(target), lock_timeout=0.2
                    )
            finally:
                self.sampler.os.write = saved_write

            self.assertEqual(target.stat().st_size, clean_boundary)
            lines = target.read_bytes().decode("utf-8").splitlines()
            self.assertEqual([json.loads(raw) for raw in lines], [{"probe": "first"}])

    # F7 -----------------------------------------------------------------
    def test_redirect_stdout_to_devnull_never_raises_on_open_failure(self) -> None:
        # redirect_stdout_to_devnull runs as fail-safe cleanup after a BrokenPipe
        # (record already in the file sink). If os.open(/dev/null) fails (fd
        # exhaustion / no /dev/null) it MUST swallow the OSError -- an escaping
        # error would flip a clean exit to non-zero. Pre-fix: the os.open call was
        # unguarded and the OSError propagated.
        #
        # sys.stdout must expose a real fileno() so we get past the early return
        # into the guarded os.open; a temp file's stream provides one.
        saved_open = self.sampler.os.open

        def boom_on_devnull(path, *args, **kwargs):
            if path == os.devnull:
                raise OSError("simulated fd exhaustion opening /dev/null")
            return saved_open(path, *args, **kwargs)

        with tempfile.TemporaryDirectory(prefix="host-health-f7.") as temp:
            stdout_path = Path(temp) / "stdout.txt"
            saved_stdout = sys.stdout
            with open(stdout_path, "w", encoding="utf-8") as real_stream:
                sys.stdout = real_stream
                self.sampler.os.open = boom_on_devnull
                try:
                    # Must return normally, NOT raise.
                    self.assertIsNone(self.sampler.redirect_stdout_to_devnull())
                finally:
                    self.sampler.os.open = saved_open
                    sys.stdout = saved_stdout

    def test_main_final_flush_oserror_preserves_file_sink_success(self) -> None:
        # A file sink can already hold the complete record when final stdout
        # cleanup runs. A late EIO from stdout.flush must not demote that success
        # to a non-zero exit. Pre-fix: OSError escaped main()'s finally block.
        class EioStdout:
            def write(self, data):
                return len(data)

            def flush(self):
                raise OSError(5, "EIO")

        with tempfile.TemporaryDirectory(prefix="host-health-fix-a.") as temp:
            target = Path(temp) / "out.jsonl"
            saved_stdout = sys.stdout
            sys.stdout = EioStdout()
            try:
                returncode = self.sampler.main(["--out", str(target)])
            finally:
                sys.stdout = saved_stdout

            self.assertEqual(returncode, 0)
            lines = target.read_text(encoding="utf-8").splitlines()
            self.assertEqual(len(lines), 1)
            record = json.loads(lines[0])
            assert_sample_schema(record)

    def test_main_final_flush_non_oserror_preserves_file_sink_success(self) -> None:
        # Same clean-exit guarantee for non-OSError flush failures such as a
        # closed/test-double stdout raising ValueError during shutdown cleanup.
        class ClosedStdout:
            def write(self, data):
                return len(data)

            def flush(self):
                raise ValueError("I/O operation on closed file")

        with tempfile.TemporaryDirectory(prefix="host-health-fix-a-value.") as temp:
            target = Path(temp) / "out.jsonl"
            saved_stdout = sys.stdout
            sys.stdout = ClosedStdout()
            try:
                returncode = self.sampler.main(["--out", str(target)])
            finally:
                sys.stdout = saved_stdout

            self.assertEqual(returncode, 0)
            lines = target.read_text(encoding="utf-8").splitlines()
            self.assertEqual(len(lines), 1)
            record = json.loads(lines[0])
            assert_sample_schema(record)

    # F8 -----------------------------------------------------------------
    def test_os_close_failure_keeps_record_in_file_no_stdout_dup(self) -> None:
        # A successful os.write followed by an os.close that raises OSError must
        # NOT be treated as a file-sink failure: the record landed in the file, so
        # write_jsonl_line must return None (no "fell back to stdout" warning) and
        # must NOT also emit the record to stdout. Pre-fix: the unguarded os.close
        # in write_to_file's finally propagated the OSError, write_jsonl_line's
        # except OSError caught it, and the record was duplicated to stdout.
        with tempfile.TemporaryDirectory(prefix="host-health-f8.") as temp:
            target = Path(temp) / "out.jsonl"
            target_fds: set[int] = set()
            saved_open = self.sampler.os.open
            saved_close = self.sampler.os.close

            def tracking_open(path, *args, **kwargs):
                fd = saved_open(path, *args, **kwargs)
                # Record only the fd opened against the target file so we raise on
                # close of exactly that fd, never on unrelated fds.
                if os.fspath(path) == str(target):
                    target_fds.add(fd)
                return fd

            def close_raises_on_target(fd):
                if fd in target_fds:
                    target_fds.discard(fd)
                    # Close it for real so we leak nothing, then raise as if the
                    # close syscall itself failed.
                    saved_close(fd)
                    raise OSError("simulated os.close failure after write")
                return saved_close(fd)

            out_buf = io.StringIO()
            saved_stdout = sys.stdout
            self.sampler.os.open = tracking_open
            self.sampler.os.close = close_raises_on_target
            sys.stdout = out_buf
            try:
                result = self.sampler.write_jsonl_line(
                    {"probe": "f8"}, str(target), lock_timeout=0.2
                )
            finally:
                self.sampler.os.open = saved_open
                self.sampler.os.close = saved_close
                sys.stdout = saved_stdout
            # Read the file while the temp dir still exists (assertions below run
            # after the dir is torn down).
            on_disk = target.read_bytes()
            stdout_dump = out_buf.getvalue()

        # Success: no fallback warning returned.
        self.assertIsNone(result)
        # The record is in the file, intact.
        lines = on_disk.decode("utf-8").splitlines()
        self.assertEqual([json.loads(raw) for raw in lines], [{"probe": "f8"}])
        # It was NOT also duplicated to stdout.
        self.assertEqual(stdout_dump, "")

    # ITEM 2 -------------------------------------------------------------
    def test_non_finite_floats_emit_strict_json_null(self) -> None:
        # A record with NaN/Infinity nested in a dict AND a list must serialise to
        # strict JSON (no NaN/Infinity tokens) with the non-finite values nulled.
        # Pre-fix: json.dumps(allow_nan=True) emitted bare NaN/Infinity tokens.
        record = {
            "schema_version": 2,
            "memory": {"mem_available_bytes": float("nan")},
            "series": [1.0, float("inf"), float("-inf"), 3.0],
            "nested": {"deep": [{"x": float("nan")}]},
        }
        out_buf = io.StringIO()
        saved_stdout = sys.stdout
        sys.stdout = out_buf
        try:
            result = self.sampler.write_jsonl_line(record, None)
        finally:
            sys.stdout = saved_stdout
        self.assertIsNone(result)
        line = out_buf.getvalue().strip()
        # No non-finite tokens leaked into the wire format.
        self.assertNotIn("NaN", line)
        self.assertNotIn("Infinity", line)
        # Strict parse must succeed and the non-finite values became null.
        parsed = json.loads(line)
        self.assertIsNone(parsed["memory"]["mem_available_bytes"])
        self.assertEqual(parsed["series"], [1.0, None, None, 3.0])
        self.assertIsNone(parsed["nested"]["deep"][0]["x"])

    def test_sanitize_non_finite_is_recursive_and_pure(self) -> None:
        sanitize = self.sampler.sanitize_non_finite
        self.assertIsNone(sanitize(float("nan")))
        self.assertIsNone(sanitize(float("inf")))
        self.assertEqual(sanitize(1.5), 1.5)
        self.assertEqual(sanitize({"a": float("nan"), "b": 2}), {"a": None, "b": 2})
        self.assertEqual(sanitize([float("inf"), "s", 4]), [None, "s", 4])

    # ITEM 3 -------------------------------------------------------------
    def test_stderr_write_failure_does_not_flip_exit_code(self) -> None:
        # One-shot, --out failing (so a warning is produced) AND a stderr that
        # raises on every write. The record reached stdout, so exit MUST stay 0;
        # pre-fix the unguarded print(file=sys.stderr) was caught by the loop's
        # broad except -> return 1.
        class RaisingStderr:
            def write(self, _data):
                raise OSError("stderr is full")

            def flush(self):
                raise OSError("stderr is full")

        with tempfile.TemporaryDirectory(prefix="host-health-item3.") as temp:
            # Parent of --out is a regular file -> file sink fails -> stdout
            # fallback + warning -> log_stderr is exercised.
            parent_file = Path(temp) / "parent_is_a_file"
            parent_file.write_text("not a directory")
            target = parent_file / "child.jsonl"
            out_buf = io.StringIO()
            saved_out, saved_err = sys.stdout, sys.stderr
            sys.stdout, sys.stderr = out_buf, RaisingStderr()
            try:
                returncode = self.sampler.main(["--out", str(target)])
            except SystemExit as exc:  # pragma: no cover - main returns int
                returncode = exc.code
            finally:
                sys.stdout, sys.stderr = saved_out, saved_err
        self.assertEqual(returncode, 0)
        record = json.loads(out_buf.getvalue().strip())
        assert_sample_schema(record)

    def test_log_stderr_swallows_write_failure(self) -> None:
        class RaisingStderr:
            def write(self, _data):
                raise OSError("boom")

            def flush(self):
                raise OSError("boom")

        saved_err = sys.stderr
        sys.stderr = RaisingStderr()
        try:
            # Must not raise.
            self.sampler.log_stderr("anything")
        finally:
            sys.stderr = saved_err

    # ITEM 4 -------------------------------------------------------------
    def test_collector_result_alias_is_importable_runtime_value(self) -> None:
        # The module imported successfully (it's loaded in setUp); assert the
        # CollectorResult alias is a usable runtime typing value.
        alias = self.sampler.CollectorResult
        # typing.Tuple[...] exposes __origin__ == tuple and the element types in
        # __args__; the second element is Optional[str] == Union[str, None].
        self.assertEqual(alias.__origin__, tuple)
        args = alias.__args__
        self.assertIs(args[0], typing.Any)
        self.assertIn(str, args[1].__args__)
        self.assertIn(type(None), args[1].__args__)

    def test_collector_result_alias_rhs_is_py38_safe(self) -> None:
        # Differential guard for the py<3.10 import crash: the CollectorResult
        # assignment's RHS is evaluated EAGERLY at import, so it must NOT use the
        # PEP 604 ``X | Y`` union or the builtin-generic ``tuple[...]`` subscript
        # (both TypeError on Python 3.8/3.9). Inspect the AST of the actual source
        # so this fails on the pre-fix HEAD regardless of the running interpreter.
        import ast

        source = SCRIPT_PATH.read_text(encoding="utf-8")
        tree = ast.parse(source)
        rhs = None
        for node in ast.walk(tree):
            if isinstance(node, ast.Assign):
                targets = {t.id for t in node.targets if isinstance(t, ast.Name)}
                if "CollectorResult" in targets:
                    rhs = node.value
                    break
        self.assertIsNotNone(rhs, "CollectorResult assignment not found in source")
        offenders = []
        for sub in ast.walk(rhs):
            # PEP 604 union: ``A | B`` is a BinOp with a BitOr op.
            if isinstance(sub, ast.BinOp) and isinstance(sub.op, ast.BitOr):
                offenders.append("PEP 604 'X | Y' union")
            # Builtin-generic subscript: ``tuple[...]`` / ``list[...]`` etc.
            if isinstance(sub, ast.Subscript) and isinstance(sub.value, ast.Name):
                if sub.value.id in {"tuple", "list", "dict", "set", "frozenset", "type"}:
                    offenders.append(f"builtin-generic '{sub.value.id}[...]'")
        self.assertEqual(
            offenders,
            [],
            f"CollectorResult RHS uses py3.10+ runtime syntax: {offenders}",
        )

    # ITEM 8 -------------------------------------------------------------
    def test_malformed_utf8_memory_events_yields_null_error(self) -> None:
        # A non-UTF-8 memory.events raises UnicodeDecodeError (a ValueError, not
        # OSError). Pre-fix it escaped collect_cgroup_oom_kills' try. Post-fix it
        # degrades to null+error and the surrounding sample still emits.
        with tempfile.TemporaryDirectory(prefix="host-health-item8.") as temp:
            root = Path(temp)
            v2_dir = root / "unified" / "system.slice" / "bolt-v2.service"
            v2_dir.mkdir(parents=True)
            # 0xFF is not valid UTF-8.
            (v2_dir / "memory.events").write_bytes(b"oom_kill 1\n\xff\xfe bad bytes\n")
            saved_v2 = self.sampler.CGROUP_V2_SYSTEM_SLICE
            saved_v1 = self.sampler.CGROUP_V1_MEMORY_SYSTEM_SLICE
            self.sampler.CGROUP_V2_SYSTEM_SLICE = root / "unified" / "system.slice"
            self.sampler.CGROUP_V1_MEMORY_SYSTEM_SLICE = root / "memory" / "system.slice"
            try:
                value, error = self.sampler.collect_cgroup_oom_kills("bolt-v2")
            finally:
                self.sampler.CGROUP_V2_SYSTEM_SLICE = saved_v2
                self.sampler.CGROUP_V1_MEMORY_SYSTEM_SLICE = saved_v1
        self.assertIsNone(value)
        self.assertIsNotNone(error)
        self.assertIn("decode error", error)

    def test_collect_service_uses_utf8_replace_and_preserves_service_block(self) -> None:
        # Simulate the locale-dependent decode path that used to null the whole
        # service block. With explicit UTF-8 + replacement decoding, systemctl
        # output remains parseable and collect_service keeps the signal.
        #
        # FIX B: collect_service now uses subprocess.Popen + a bounded
        # communicate(timeout=...) (instead of subprocess.run) so a wedged child
        # cannot trigger an unbounded post-kill wait(). The encoding/errors must
        # still be passed UTF-8 + replace, so this patches Popen and asserts both.
        captured: dict[str, object] = {}
        systemctl_output = (
            "LoadState=loaded\n"
            "ActiveState=active\n"
            "SubState=running\n"
            "Result=success\n"
            "NRestarts=3\n"
            "MainPID=123\n"
            "ExecMainPID=123\n"
            "ExecMainCode=0\n"
            "ExecMainStatus=0\n"
            "ExecMainStartTimestamp=Mon 2026-06-22 01:02:03 UTC\n"
            "InvocationID=abc123\n"
        )

        class FakeProc:
            def __init__(self, command, **kwargs):
                captured["command"] = command
                captured.update(kwargs)
                self.returncode = None
                if kwargs.get("encoding") != "utf-8" or kwargs.get("errors") != "replace":
                    raise UnicodeDecodeError(
                        "utf-8",
                        b"LoadState=loaded\nInvocationID=\xff\n",
                        30,
                        31,
                        "simulated locale decode failure",
                    )

            def communicate(self, timeout=None):
                captured["timeout"] = timeout
                self.returncode = 0
                return systemctl_output, ""

            def kill(self):  # pragma: no cover - not exercised on the happy path
                pass

            def poll(self):  # pragma: no cover - not exercised on the happy path
                return self.returncode

        saved_popen = self.sampler.subprocess.Popen
        self.sampler.subprocess.Popen = FakeProc
        try:
            service, error = self.sampler.collect_service("bolt-v2.service")
        finally:
            self.sampler.subprocess.Popen = saved_popen

        self.assertIsNone(error)
        self.assertIsInstance(service, dict)
        self.assertEqual(service["unit"], "bolt-v2.service")
        self.assertEqual(service["load_state"], "loaded")
        self.assertEqual(service["active_state"], "active")
        self.assertEqual(service["sub_state"], "running")
        self.assertEqual(service["result"], "success")
        self.assertEqual(service["n_restarts"], 3)
        self.assertEqual(service["main_pid"], 123)
        self.assertEqual(service["invocation_id"], "abc123")
        self.assertEqual(captured["encoding"], "utf-8")
        self.assertEqual(captured["errors"], "replace")
        # The bounded inner timeout is honoured (no unbounded wait on the child).
        self.assertEqual(captured["timeout"], self.sampler.SYSTEMCTL_TIMEOUT_SECONDS)

    # ITEM 10 ------------------------------------------------------------
    def test_collect_process_rechecks_identity_after_reads(self) -> None:
        # The pid-recycle TOCTOU guard: identity_ok True on the first (pre-read)
        # check, False on the post-read re-check -> the pid was recycled mid-read,
        # so collect_process must DISCARD the metrics and return None + a recycle
        # error. Pre-fix it returned the wrong process's metrics. Platform-
        # agnostic: every I/O helper is stubbed and the platform gate is spoofed
        # to "linux" so the ONLY behaviour under test is the re-check.
        sampler = self.sampler
        calls = {"identity": 0}

        def identity(pid, unit):
            calls["identity"] += 1
            # First call (pre-read) ok; second call (post-read) recycled.
            return (True, None) if calls["identity"] == 1 else (
                False,
                f"pid recycled: /proc/{pid}/cgroup did not contain unit {unit!r}",
            )

        saved = {
            "identity": sampler.process_identity_ok,
            "status": sampler.parse_proc_status,
            "fd": sampler.read_fd_count,
            "limit": sampler.read_fd_limit_soft,
            "platform": sampler.sys.platform,
        }
        sampler.process_identity_ok = identity
        sampler.parse_proc_status = lambda pid: ({"VmRSS": 1024}, None)
        sampler.read_fd_count = lambda pid: (3, None)
        sampler.read_fd_limit_soft = lambda pid: (1024, None)
        sampler.sys.platform = "linux"
        try:
            value, error = sampler.collect_process("bolt-v2", 4321, None)
        finally:
            sampler.process_identity_ok = saved["identity"]
            sampler.parse_proc_status = saved["status"]
            sampler.read_fd_count = saved["fd"]
            sampler.read_fd_limit_soft = saved["limit"]
            sampler.sys.platform = saved["platform"]
        self.assertEqual(calls["identity"], 2, "identity must be re-checked after reads")
        self.assertIsNone(value, "recycled pid metrics must be discarded")
        self.assertIn("recycled", error)

    # ITEM 12 ------------------------------------------------------------
    def test_catalog_discovery_reads_root_toml_value(self) -> None:
        # A fake <tmp>/config/root.toml with a DIVERGENT catalog_directory value
        # must be discovered (proving discovery reads the file, not the fallback).
        with tempfile.TemporaryDirectory(prefix="host-health-item12.") as temp:
            root = Path(temp)
            (root / "config").mkdir()
            (root / "config" / "root.toml").write_text(
                "[persistence]\n"
                'catalog_directory = "/discovered/catalog/path"\n'
                'required_catalog_prefix = "/discovered"\n',
                encoding="utf-8",
            )
            found = self.sampler.discover_catalog_directory(start=root)
            self.assertEqual(found, "/discovered/catalog/path")

    def test_catalog_discovery_falls_back_when_absent(self) -> None:
        # An isolated subtree under the tempdir with NO config/root.toml at the
        # start dir or any ancestor (up to /) -> discovery yields the standalone
        # fallback constant. Using ``start=`` confines the search to this subtree
        # (the script's own repo ancestors are not consulted).
        with tempfile.TemporaryDirectory(prefix="host-health-item12b.") as temp:
            isolated = Path(temp) / "a" / "b" / "c"
            isolated.mkdir(parents=True)
            found = self.sampler.discover_catalog_directory(start=isolated)
        self.assertEqual(found, self.sampler.DISK_PATH_FALLBACK)
        self.assertEqual(
            self.sampler.DISK_PATH_FALLBACK, "/srv/bolt-v2/var/bolt-v3-live/catalog"
        )

    def test_extract_catalog_directory_ignores_neighbor_key(self) -> None:
        extract = self.sampler.extract_catalog_directory
        toml_text = (
            "[persistence]\n"
            'catalog_directory = "/a/b/c"\n'
            'required_catalog_prefix = "/a"\n'
        )
        self.assertEqual(extract(toml_text), "/a/b/c")
        # The neighbouring key must never be picked up by accident.
        self.assertEqual(extract('required_catalog_prefix = "/only/prefix"\n'), None)
        # Single-quote form is accepted too.
        self.assertEqual(extract("catalog_directory = '/single'\n"), "/single")


class FakeStatvfsCustom:
    """A minimal statvfs stand-in with caller-supplied block fields.

    Only the attributes ``disk_metrics_from_statvfs`` reads are provided; the
    inode fields are fixed sentinels (irrelevant to the used_pct math under test).
    """

    def __init__(self, f_blocks: int, f_bfree: int, f_bavail: int, f_frsize: int = 4096) -> None:
        self.f_frsize = f_frsize
        self.f_bsize = f_frsize
        self.f_blocks = f_blocks
        self.f_bfree = f_bfree
        self.f_bavail = f_bavail
        self.f_files = 1000
        self.f_favail = 700


class ReviewRound3HardeningTests(unittest.TestCase):
    """Review-driven CLASS fixes B/A/C/E: bounded execution, stdout sink-failure
    classification, cgroup-v1 oom counter file, and disk:null on a vanished
    catalog path plus a metric-sanity guard."""

    def setUp(self) -> None:
        self.sampler = load_sampler()

    # FIX B --------------------------------------------------------------
    def test_run_collector_times_out_slow_collector_within_deadline(self) -> None:
        # A collector that sleeps well past the outer deadline must surface as a
        # null + "timed out" error within roughly the deadline, and must NOT hang
        # the caller (the daemon worker thread is abandoned). Pre-fix: run_collector
        # called the function directly with no deadline, so this would block for the
        # full sleep duration (here: never, in test time).
        #
        # We shrink the constant to keep the test fast; the behaviour (bounded by
        # COLLECTOR_TIMEOUT_SECONDS, daemon thread abandoned) is identical.
        saved_timeout = self.sampler.COLLECTOR_TIMEOUT_SECONDS
        self.sampler.COLLECTOR_TIMEOUT_SECONDS = 0.2
        finished = threading.Event()

        def slow_collector():
            time.sleep(30)  # far longer than the (shrunk) deadline
            finished.set()
            return "should-never-return", None

        try:
            started = time.monotonic()
            value, error = self.sampler.run_collector("slow", slow_collector)
            elapsed = time.monotonic() - started
        finally:
            self.sampler.COLLECTOR_TIMEOUT_SECONDS = saved_timeout

        self.assertIsNone(value)
        self.assertIsNotNone(error)
        self.assertIn("timed out", error)
        self.assertIn("slow", error)
        # Bounded: it returned in roughly the deadline, not after the 30s sleep.
        self.assertLess(elapsed, 5.0, "run_collector did not honour the outer deadline")
        # The slow worker never completed (it was abandoned, not joined).
        self.assertFalse(finished.is_set(), "slow collector should have been abandoned")

    def test_run_with_deadline_returns_value_and_reraises_exception(self) -> None:
        # Fast function: its return value comes back unchanged.
        result = self.sampler.run_with_deadline(lambda a, b: a + b, 5.0, 2, 3)
        self.assertEqual(result, 5)

        # A function that raises: the SAME exception instance/type is re-raised in
        # the caller (not swallowed, not wrapped). Pre-fix there was no helper at
        # all; this pins the contract that the worker's exception propagates.
        sentinel = ValueError("collector blew up")

        def boom():
            raise sentinel

        with self.assertRaises(ValueError) as ctx:
            self.sampler.run_with_deadline(boom, 5.0)
        self.assertIs(ctx.exception, sentinel)

    def test_collect_service_timeout_does_not_wait_after_kill(self) -> None:
        # FIX B: on TimeoutExpired collect_service must kill the child and return
        # the timeout error WITHOUT a second blocking communicate()/wait(). Pre-fix
        # subprocess.run's internal post-kill wait() could hang forever on a
        # D-state child. We assert: timeout error returned, kill() called, and NO
        # blocking wait()/second communicate() was issued.
        events: dict[str, int] = {"communicate": 0, "kill": 0, "wait": 0}

        class WedgedProc:
            def __init__(self, command, **kwargs):
                self.returncode = None

            def communicate(self, timeout=None):
                events["communicate"] += 1
                # First (and only) communicate times out, as a wedged child would.
                raise subprocess.TimeoutExpired(cmd="systemctl", timeout=timeout)

            def kill(self):
                events["kill"] += 1

            def wait(self, timeout=None):  # must never be called post-kill
                events["wait"] += 1
                raise AssertionError("collect_service issued a blocking wait() after kill")

            def poll(self):
                return None

        saved_popen = self.sampler.subprocess.Popen
        self.sampler.subprocess.Popen = WedgedProc
        try:
            service, error = self.sampler.collect_service("bolt-v2.service")
        finally:
            self.sampler.subprocess.Popen = saved_popen

        self.assertIsNone(service)
        self.assertIsNotNone(error)
        self.assertIn("timed out", error)
        self.assertEqual(events["communicate"], 1, "must not re-issue a blocking communicate")
        self.assertEqual(events["kill"], 1, "the wedged child must be killed")
        self.assertEqual(events["wait"], 0, "must not block on wait() after kill")

    # FIX B (sink side) --------------------------------------------------
    @unittest.skipUnless(hasattr(os, "mkfifo"), "os.mkfifo not available on this platform")
    def test_fifo_sink_with_no_reader_falls_back_to_stdout(self) -> None:
        # A --out that resolves to a FIFO with no reader must NOT hang the writer.
        # Pre-fix: write_to_file's blocking os.open(O_WRONLY) on a readerless FIFO
        # blocks forever (and the call is not deadline-wrapped), so write_jsonl_line
        # never returns. Post-fix: O_NONBLOCK makes the open raise ENXIO (OSError)
        # immediately, write_jsonl_line falls back to stdout and returns the
        # file-sink-failed warning with the record on stdout.
        #
        # Run under a watchdog thread so a regression fails fast instead of hanging
        # the whole suite. We also shrink SINK_TIMEOUT_SECONDS so that even if the
        # open path somehow blocked, the deadline backstop would fire quickly.
        sampler = self.sampler
        saved_sink_timeout = sampler.SINK_TIMEOUT_SECONDS
        sampler.SINK_TIMEOUT_SECONDS = 0.5
        with tempfile.TemporaryDirectory(prefix="host-health-fifo.") as temp:
            fifo_path = Path(temp) / "sink.fifo"
            os.mkfifo(fifo_path)
            out_buf = io.StringIO()
            box: dict[str, object] = {}

            def call() -> None:
                saved_stdout = sys.stdout
                sys.stdout = out_buf
                try:
                    box["warning"] = sampler.write_jsonl_line(
                        {"probe": "fifo-no-reader"}, str(fifo_path)
                    )
                finally:
                    sys.stdout = saved_stdout
                box["done"] = True

            worker = threading.Thread(target=call, daemon=True)
            worker.start()
            worker.join(timeout=5.0)
            try:
                self.assertTrue(box.get("done"), "writer hung on a readerless FIFO sink")
                self.assertIsInstance(box.get("warning"), str)
                self.assertIn("fell back to stdout", box["warning"])
                record = json.loads(out_buf.getvalue().strip())
                self.assertEqual(record["probe"], "fifo-no-reader")
            finally:
                sampler.SINK_TIMEOUT_SECONDS = saved_sink_timeout

    @unittest.skipUnless(hasattr(os, "mkfifo"), "os.mkfifo not available on this platform")
    def test_write_to_file_fifo_no_reader_raises_fast_pins_o_nonblock(self) -> None:
        # Isolates O_NONBLOCK from the deadline wrap. Calls write_to_file DIRECTLY
        # (not through write_jsonl_line), so run_with_deadline is NOT in the path
        # and cannot mask a blocking open. With O_NONBLOCK, os.open(O_WRONLY) on a
        # readerless FIFO raises ENXIO (OSError) immediately, so write_to_file
        # fails fast. Pre-fix (O_NONBLOCK removed): the blocking open hangs forever,
        # the watchdog never sees box["done"], and this test fails fast instead of
        # hanging the suite. This pins O_NONBLOCK alone, while
        # test_stalled_file_sink_bounded_by_deadline_falls_back_to_stdout pins the
        # deadline wrap.
        sampler = self.sampler
        with tempfile.TemporaryDirectory(prefix="host-health-fifo-direct.") as temp:
            fifo_path = Path(temp) / "sink.fifo"
            os.mkfifo(fifo_path)
            box: dict[str, object] = {}

            def call() -> None:
                try:
                    sampler.write_to_file(
                        '{"probe":"fifo-direct"}\n',
                        str(fifo_path),
                        sampler.FLOCK_TIMEOUT_SECONDS,
                    )
                    box["raised"] = None
                except OSError as exc:
                    box["raised"] = exc
                box["done"] = True

            worker = threading.Thread(target=call, daemon=True)
            worker.start()
            worker.join(timeout=5.0)
            self.assertTrue(
                box.get("done"),
                "write_to_file hung on a readerless FIFO (O_NONBLOCK missing)",
            )
            self.assertIsInstance(
                box.get("raised"),
                OSError,
                "readerless-FIFO open must raise OSError (ENXIO) immediately",
            )

    def test_stalled_file_sink_bounded_by_deadline_falls_back_to_stdout(self) -> None:
        # A file sink that STALLS (e.g. an EBS/disk hang during os.open/os.write)
        # must be bounded by SINK_TIMEOUT_SECONDS and fall back to stdout, not hang.
        # We monkeypatch write_to_file to sleep far longer than a shrunk
        # SINK_TIMEOUT_SECONDS. Pre-fix: write_jsonl_line called write_to_file
        # DIRECTLY (unwrapped), so this sleep would block the writer for the full
        # 30s (effectively forever). Post-fix: run_with_deadline raises TimeoutError
        # (a subclass of OSError) at the deadline, the existing OSError handler
        # falls back to stdout, and the call returns within roughly the deadline.
        sampler = self.sampler
        saved_write_to_file = sampler.write_to_file
        saved_sink_timeout = sampler.SINK_TIMEOUT_SECONDS
        sampler.SINK_TIMEOUT_SECONDS = 0.2

        def stalling_write_to_file(record_line, out_path, lock_timeout):
            time.sleep(30)  # far longer than the shrunk deadline

        out_buf = io.StringIO()
        box: dict[str, object] = {}

        def call() -> None:
            saved_stdout = sys.stdout
            sys.stdout = out_buf
            try:
                started = time.monotonic()
                box["warning"] = sampler.write_jsonl_line(
                    {"probe": "stalled-sink"}, "/tmp/host-health-stall-irrelevant.jsonl"
                )
                box["elapsed"] = time.monotonic() - started
            finally:
                sys.stdout = saved_stdout
            box["done"] = True

        sampler.write_to_file = stalling_write_to_file
        try:
            worker = threading.Thread(target=call, daemon=True)
            worker.start()
            worker.join(timeout=5.0)
            self.assertTrue(box.get("done"), "writer hung on a stalled file sink")
            self.assertLess(box["elapsed"], 3.0, "writer did not honour the sink deadline")
            self.assertIsInstance(box.get("warning"), str)
            self.assertIn("fell back to stdout", box["warning"])
            record = json.loads(out_buf.getvalue().strip())
            self.assertEqual(record["probe"], "stalled-sink")
        finally:
            sampler.write_to_file = saved_write_to_file
            sampler.SINK_TIMEOUT_SECONDS = saved_sink_timeout

    # FIX A --------------------------------------------------------------
    def test_primary_stdout_closed_value_error_is_record_unemittable(self) -> None:
        # out_path is None (primary streaming stdout). A CLOSED stdout raises
        # ValueError("I/O operation on closed file"), which is NOT an OSError.
        # Post-fix the primary catch is (OSError, ValueError) so this becomes
        # RecordUnemittable. Pre-fix only OSError was caught, so the ValueError
        # escaped to main()'s generic handler (wrong path).
        class ClosedStdout:
            def write(self, _data):
                raise ValueError("I/O operation on closed file")

            def flush(self):
                raise ValueError("I/O operation on closed file")

        saved_stdout = sys.stdout
        sys.stdout = ClosedStdout()
        try:
            with self.assertRaises(self.sampler.RecordUnemittable):
                self.sampler.write_jsonl_line({"probe": "closed-primary"}, None)
        finally:
            sys.stdout = saved_stdout

    def test_fallback_stdout_brokenpipe_is_record_unemittable(self) -> None:
        # File sink fails (forced via write_to_file raising OSError) AND the
        # fallback stdout raises BrokenPipeError. Post-fix: the record reached NO
        # sink, so this is RecordUnemittable. Pre-fix: the fallback branch re-raised
        # BrokenPipeError as a "clean" termination, which main() turned into exit 0
        # with the record silently lost.
        saved_write_to_file = self.sampler.write_to_file

        def failing_write_to_file(record_line, out_path, lock_timeout):
            raise OSError("simulated file sink failure")

        class BrokenStdout:
            def write(self, _data):
                raise BrokenPipeError("simulated gone consumer on fallback")

            def flush(self):
                raise BrokenPipeError("simulated gone consumer on fallback")

        saved_stdout = sys.stdout
        self.sampler.write_to_file = failing_write_to_file
        sys.stdout = BrokenStdout()
        try:
            with self.assertRaises(self.sampler.RecordUnemittable):
                self.sampler.write_jsonl_line(
                    {"probe": "fallback-brokenpipe"}, "/tmp/host-health-irrelevant.jsonl"
                )
        finally:
            sys.stdout = saved_stdout
            self.sampler.write_to_file = saved_write_to_file

    def test_fallback_stdout_brokenpipe_yields_exit_1_through_main(self) -> None:
        # End-to-end via main([...]) in one-shot: a file sink that fails and a
        # fallback stdout BrokenPipe must exit 1 (record unemittable), NOT 0.
        # Pre-fix the fallback BrokenPipe was treated as clean -> exit 0.
        saved_write_to_file = self.sampler.write_to_file

        def failing_write_to_file(record_line, out_path, lock_timeout):
            raise OSError("simulated file sink failure")

        class BrokenStdout:
            def write(self, _data):
                raise BrokenPipeError("simulated gone consumer on fallback")

            def flush(self):
                raise BrokenPipeError("simulated gone consumer on fallback")

        saved_out, saved_err = sys.stdout, sys.stderr
        err_buf = io.StringIO()
        self.sampler.write_to_file = failing_write_to_file
        sys.stdout = BrokenStdout()
        sys.stderr = err_buf
        try:
            returncode = self.sampler.main(["--out", "/tmp/host-health-irrelevant.jsonl"])
        except SystemExit as exc:  # pragma: no cover - main returns int
            returncode = exc.code
        finally:
            sys.stdout, sys.stderr = saved_out, saved_err
            self.sampler.write_to_file = saved_write_to_file
        self.assertEqual(returncode, 1)

    def test_primary_stdout_brokenpipe_still_propagates_clean_exit_zero(self) -> None:
        # Regression guard: a PRIMARY (out_path None) stdout BrokenPipe is still a
        # clean stream termination -> main() exits 0. This must NOT be reclassified
        # as RecordUnemittable by the FIX A change.
        class BrokenStdout:
            def write(self, _data):
                raise BrokenPipeError("simulated gone streaming consumer")

            def flush(self):
                raise BrokenPipeError("simulated gone streaming consumer")

        saved_stdout = sys.stdout
        sys.stdout = BrokenStdout()
        try:
            returncode = self.sampler.main(["--interval", "0.1"])
        finally:
            sys.stdout = saved_stdout
        self.assertEqual(returncode, 0)

    # FIX C --------------------------------------------------------------
    def test_cgroup_v1_reads_oom_control_not_memory_events(self) -> None:
        # A fake cgroup-v1 hierarchy exposing the counter in memory.oom_control
        # (the REAL v1 file) with the v2 path absent must yield (3, None). Pre-fix
        # the v1 candidate pointed at memory.events, which cgroup-v1 does not carry
        # the oom_kill counter in, so the collector returned (None, <unavailable>).
        with tempfile.TemporaryDirectory(prefix="host-health-fixc.") as temp:
            root = Path(temp)
            v1_dir = root / "memory" / "system.slice" / "bolt-v2.service"
            v1_dir.mkdir(parents=True)
            (v1_dir / "memory.oom_control").write_text(
                "oom_kill_disable 0\nunder_oom 0\noom_kill 3\n", encoding="utf-8"
            )
            saved_v2 = self.sampler.CGROUP_V2_SYSTEM_SLICE
            saved_v1 = self.sampler.CGROUP_V1_MEMORY_SYSTEM_SLICE
            # v2 points at a guaranteed-absent dir; v1 at the fake oom_control tree.
            self.sampler.CGROUP_V2_SYSTEM_SLICE = root / "unified" / "system.slice"
            self.sampler.CGROUP_V1_MEMORY_SYSTEM_SLICE = root / "memory" / "system.slice"
            try:
                value, error = self.sampler.collect_cgroup_oom_kills("bolt-v2")
            finally:
                self.sampler.CGROUP_V2_SYSTEM_SLICE = saved_v2
                self.sampler.CGROUP_V1_MEMORY_SYSTEM_SLICE = saved_v1
        self.assertEqual(value, 3, error)
        self.assertIsNone(error)

    # FIX E --------------------------------------------------------------
    def test_collect_disk_missing_requested_path_emits_null(self) -> None:
        # A requested catalog path that does not exist must yield (None, <error
        # mentioning the requested path missing>) WITHOUT measuring a parent
        # directory. Pre-fix collect_disk statvfs'd the ancestor and returned a
        # non-None disk dict (the ancestor's fullness) for a path that has
        # vanished.
        missing = "/definitely/missing/host-health-" + str(os.getpid()) + "/catalog"
        value, error = self.sampler.collect_disk(missing)
        self.assertIsNone(value)
        self.assertIsNotNone(error)
        self.assertIn("missing", error)
        self.assertIn(missing, error)
        # The error must NOT claim it measured an ancestor: disk is null, so a
        # "measured nearest existing ancestor" message would contradict the null
        # disk and read as if ancestor data existed (a live smoke run caught the
        # stale wording).
        self.assertNotIn("ancestor", error)
        self.assertIn("disk metrics unavailable", error)

    def test_collect_disk_existing_path_still_measures(self) -> None:
        # Guard the other half of FIX E: an EXISTING path still produces a disk
        # dict with no path_error (unchanged behaviour).
        with tempfile.TemporaryDirectory(prefix="host-health-fixe-ok.") as temp:
            value, error = self.sampler.collect_disk(temp)
        self.assertIsNotNone(value)
        self.assertIsNone(error)
        self.assertIsNotNone(value["used_pct"])

    def test_disk_metrics_non_physical_free_blocks_nulls_used_pct(self) -> None:
        # FIX E5: f_bfree > f_blocks is non-physical (more free than total) and
        # would compute a NEGATIVE used_pct. The guard nulls it. Pre-fix the math
        # produced a negative used_pct the viewer reads as green.
        fake = FakeStatvfsCustom(f_blocks=100, f_bfree=120, f_bavail=110)
        metrics = self.sampler.disk_metrics_from_statvfs("/data", fake, 7)
        self.assertIsNone(metrics["used_pct"])

    def test_disk_metrics_used_pct_clamped_to_100(self) -> None:
        # FIX E5: a fully-used filesystem (no blocks available to the user) reports
        # exactly 100, and the clamp guarantees used_pct can never round above 100.
        # f_blocks=100, f_bfree=0 -> used=100 blocks; f_bavail=0 -> avail=0;
        # denominator=100 blocks; used_pct=100.
        fake_full = FakeStatvfsCustom(f_blocks=100, f_bfree=0, f_bavail=0)
        metrics_full = self.sampler.disk_metrics_from_statvfs("/data", fake_full, 7)
        self.assertEqual(metrics_full["used_pct"], 100)
        self.assertLessEqual(metrics_full["used_pct"], 100)

    def test_disk_metrics_clamp_caps_overflow_used_pct(self) -> None:
        # Round-4 tightening: the previous overflow construction used a negative
        # f_bavail to make raw df-denominator math exceed 100. Negative available
        # blocks are now rejected by the physical-invariant guard before clamping,
        # because the viewer would otherwise render a corrupt metric as healthy.
        class OverflowStatvfs(FakeStatvfsCustom):
            pass

        fake = OverflowStatvfs(f_blocks=100, f_bfree=1, f_bavail=-10)
        metrics = self.sampler.disk_metrics_from_statvfs("/data", fake, 7)
        self.assertIsNone(metrics["used_pct"])


class ReviewRound4HardeningTests(unittest.TestCase):
    """Round-4 visibility-only hardening: bounded stdout, broad sink-failure
    classification, stricter disk invariants, pre-existing fragment quarantine,
    cgroup OOM independence, malformed restart surfacing, and TOML comments."""

    def setUp(self) -> None:
        self.sampler = load_sampler()

    def test_primary_stdout_full_pipe_times_out_as_record_unemittable(self) -> None:
        sampler = self.sampler
        saved_sink_timeout = sampler.SINK_TIMEOUT_SECONDS
        saved_stdout = sys.stdout
        read_fd, write_fd = os.pipe()
        original_flags = fcntl.fcntl(write_fd, fcntl.F_GETFL)
        fcntl.fcntl(write_fd, fcntl.F_SETFL, original_flags | os.O_NONBLOCK)
        try:
            while True:
                try:
                    os.write(write_fd, b"x" * 65536)
                except BlockingIOError:
                    break
        finally:
            fcntl.fcntl(write_fd, fcntl.F_SETFL, original_flags)

        class PipeStdout:
            def write(self, data):
                return os.write(write_fd, data.encode("utf-8"))

            def flush(self):
                return None

        box: dict[str, object] = {}

        def call() -> None:
            sys.stdout = PipeStdout()
            try:
                sampler.write_jsonl_line({"payload": "x" * 70000}, None)
            except BaseException as exc:  # noqa: BLE001 - captured for assertion
                box["raised"] = exc
            finally:
                sys.stdout = saved_stdout
            box["done"] = True

        sampler.SINK_TIMEOUT_SECONDS = 0.2
        worker = threading.Thread(target=call, daemon=True)
        started = time.monotonic()
        try:
            worker.start()
            worker.join(timeout=sampler.SINK_TIMEOUT_SECONDS + 2.0)
            elapsed = time.monotonic() - started
            finished_before_cleanup = not worker.is_alive()
        finally:
            sampler.SINK_TIMEOUT_SECONDS = saved_sink_timeout
            sys.stdout = saved_stdout
            os.close(read_fd)
            os.close(write_fd)
            worker.join(timeout=1.0)

        self.assertTrue(finished_before_cleanup, "primary stdout write hung on a full pipe")
        self.assertLess(elapsed, 3.0)
        self.assertIsInstance(box.get("raised"), sampler.RecordUnemittable)

    def test_file_sink_thread_start_failure_falls_back_to_stdout(self) -> None:
        sampler = self.sampler
        saved_thread = sampler.threading.Thread
        saved_stdout = sys.stdout
        out_buf = io.StringIO()
        starts = {"count": 0}

        class StartFailsOnceThread(saved_thread):
            def start(self):
                starts["count"] += 1
                if starts["count"] == 1:
                    raise RuntimeError("can't start new thread")
                return super().start()

        sampler.threading.Thread = StartFailsOnceThread
        sys.stdout = out_buf
        try:
            with tempfile.TemporaryDirectory(prefix="host-health-thread-start.") as temp:
                warning = sampler.write_jsonl_line(
                    {"probe": "thread-start-fallback"},
                    str(Path(temp) / "health.jsonl"),
                )
        finally:
            sys.stdout = saved_stdout
            sampler.threading.Thread = saved_thread

        self.assertIsInstance(warning, str)
        self.assertIn("fell back to stdout", warning)
        self.assertEqual(json.loads(out_buf.getvalue())["probe"], "thread-start-fallback")

    def test_primary_stdout_none_is_record_unemittable(self) -> None:
        saved_stdout = sys.stdout
        sys.stdout = None
        try:
            with self.assertRaises(self.sampler.RecordUnemittable):
                self.sampler.write_jsonl_line({"probe": "stdout-none"}, None)
        finally:
            sys.stdout = saved_stdout

    def test_shutdown_flush_is_deadline_bounded(self) -> None:
        # The record reaches the --out FILE sink, so stdout is touched ONLY by
        # main()'s finally flush. With a stdout whose flush blocks forever, an
        # unbounded finally flush would wedge shutdown (a held BufferedWriter lock
        # from an abandoned writer is the real-world trigger). The bounded flush
        # must abandon it and let main() return. signal.signal only works on the
        # main thread, so it is stubbed to run main() in a worker.
        sampler = self.sampler
        saved_timeout = sampler.SINK_TIMEOUT_SECONDS
        saved_stdout = sys.stdout
        saved_signal = sampler.signal.signal
        sampler.SINK_TIMEOUT_SECONDS = 0.3
        sampler.signal.signal = lambda *args, **kwargs: None

        class BlockingFlushStdout:
            def write(self, data):
                return len(data)

            def flush(self):
                time.sleep(30)  # blocks well past the deadline; thread abandoned

        box: dict[str, object] = {}
        try:
            with tempfile.TemporaryDirectory(prefix="host-health-shutdown.") as temp:
                out_path = Path(temp) / "health.jsonl"

                def call() -> None:
                    sys.stdout = BlockingFlushStdout()
                    try:
                        box["rc"] = sampler.main(["--out", str(out_path)])
                    except BaseException as exc:  # noqa: BLE001 - captured
                        box["raised"] = exc
                    finally:
                        sys.stdout = saved_stdout
                    box["done"] = True

                worker = threading.Thread(target=call, daemon=True)
                started = time.monotonic()
                worker.start()
                worker.join(timeout=sampler.SINK_TIMEOUT_SECONDS + 4.0)
                finished = not worker.is_alive()
                elapsed = time.monotonic() - started
                content = out_path.read_text(encoding="utf-8").strip()
        finally:
            sampler.SINK_TIMEOUT_SECONDS = saved_timeout
            sampler.signal.signal = saved_signal
            sys.stdout = saved_stdout

        self.assertTrue(finished, "shutdown flush hung on a blocked stdout")
        self.assertLess(elapsed, 6.0)
        self.assertEqual(box.get("rc"), 0)
        self.assertIsNone(box.get("raised"))
        # The record still reached the file sink (shutdown bounding did not drop it).
        self.assertEqual(json.loads(content.splitlines()[-1])["schema_version"], 2)

    def test_disk_metrics_rejects_non_physical_bavail_and_preserves_normal_case(self) -> None:
        impossible = SimpleNamespace(
            f_blocks=100,
            f_bfree=100,
            f_bavail=200,
            f_frsize=4096,
            f_files=10,
            f_favail=10,
        )
        impossible_metrics = self.sampler.disk_metrics_from_statvfs("/data", impossible, 7)
        self.assertIsNone(impossible_metrics["used_pct"])

        half_full = SimpleNamespace(
            f_blocks=100,
            f_bfree=50,
            f_bavail=50,
            f_frsize=4096,
            f_files=10,
            f_favail=5,
        )
        half_full_metrics = self.sampler.disk_metrics_from_statvfs("/data", half_full, 7)
        self.assertEqual(half_full_metrics["used_pct"], 50.0)

    def test_write_to_file_quarantines_preexisting_trailing_fragment(self) -> None:
        with tempfile.TemporaryDirectory(prefix="host-health-fragment.") as temp:
            out_path = Path(temp) / "health.jsonl"
            out_path.write_text('{"partial":', encoding="utf-8")

            self.sampler.write_to_file(
                '{"probe":"second"}\n',
                str(out_path),
                self.sampler.FLOCK_TIMEOUT_SECONDS,
            )

            lines = [line for line in out_path.read_text(encoding="utf-8").splitlines() if line]

        self.assertEqual(lines, ['{"partial":', '{"probe":"second"}'])
        self.assertEqual(json.loads(lines[-1])["probe"], "second")

    def test_sample_collects_cgroup_oom_when_service_collection_fails(self) -> None:
        sampler = self.sampler
        saved_collectors = {
            "collect_host": sampler.collect_host,
            "collect_service": sampler.collect_service,
            "collect_cgroup_oom_kills": sampler.collect_cgroup_oom_kills,
            "collect_process": sampler.collect_process,
            "collect_memory": sampler.collect_memory,
            "collect_disk": sampler.collect_disk,
        }
        calls = {"cgroup": 0}

        def collect_service(_unit):
            return None, "timed out"

        def collect_cgroup_oom_kills(_unit):
            calls["cgroup"] += 1
            return 9, None

        sampler.collect_host = lambda: ("host", None)
        sampler.collect_service = collect_service
        sampler.collect_cgroup_oom_kills = collect_cgroup_oom_kills
        sampler.collect_process = lambda _unit, _main_pid, _exec_main_pid: (None, None)
        sampler.collect_memory = lambda: (None, None)
        sampler.collect_disk = lambda _path: (None, None)
        try:
            record = sampler.sample()
        finally:
            for name, value in saved_collectors.items():
                setattr(sampler, name, value)

        self.assertEqual(calls["cgroup"], 1)
        self.assertIs(record["oom_killed"], sampler.derive_oom_killed(None, 9))
        self.assertIn("service: timed out", record["errors"])

    def test_collect_service_reports_malformed_nrestarts(self) -> None:
        sampler = self.sampler

        class FakeProc:
            returncode = 0

            def communicate(self, timeout=None):
                return (
                    "LoadState=loaded\n"
                    "ActiveState=active\n"
                    "SubState=running\n"
                    "Result=success\n"
                    "NRestarts=not-an-int\n"
                    "MainPID=123\n"
                    "ExecMainPID=123\n"
                    "ExecMainCode=0\n"
                    "ExecMainStatus=0\n"
                    "ExecMainStartTimestamp=Mon 2026-01-01 00:00:00 UTC\n"
                    "InvocationID=abc\n",
                    "",
                )

        saved_popen = sampler.subprocess.Popen
        sampler.subprocess.Popen = lambda _command, **_kwargs: FakeProc()
        try:
            service, error = sampler.collect_service("bolt-v2")
        finally:
            sampler.subprocess.Popen = saved_popen

        self.assertIsInstance(service, dict)
        self.assertIsNone(service["n_restarts"])
        # A malformed restart count must NOT also drop the later fields: the
        # remaining systemd properties are still parsed before the fail-loud
        # return (guards against regressing to an early return).
        self.assertEqual(service["main_pid"], 123)
        self.assertEqual(service["invocation_id"], "abc")
        self.assertIsNotNone(error)
        self.assertIn("NRestarts malformed", error)

    def test_extract_catalog_directory_allows_trailing_comment(self) -> None:
        self.assertEqual(
            self.sampler.extract_catalog_directory('catalog_directory = "/srv/foo" # data dir'),
            "/srv/foo",
        )


if __name__ == "__main__":
    import lane_governor

    lane_governor.acquire()
    unittest.main()
