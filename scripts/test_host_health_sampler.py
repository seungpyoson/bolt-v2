#!/usr/bin/env python3
"""Self-tests for the standalone host-health sampler."""

from __future__ import annotations

from datetime import datetime
import importlib.util
import json
from pathlib import Path
import subprocess
import sys
import tempfile


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


def assert_sample_schema(record: dict[str, object]) -> None:
    expected_keys = {
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
    if set(record) != expected_keys:
        raise AssertionError(f"schema keys differ: {sorted(record)}")
    json.dumps(record)


def test_sample_returns_json_serializable_schema() -> None:
    sampler = load_sampler()
    record = sampler.sample()
    assert_sample_schema(record)


def test_sample_timestamp_is_utc_and_schema_version_is_two() -> None:
    sampler = load_sampler()
    record = sampler.sample()
    if record["schema_version"] != 2:
        raise AssertionError(f"unexpected schema version: {record['schema_version']!r}")
    sampled_at = record["sampled_at"]
    if not isinstance(sampled_at, str) or not sampled_at.endswith("+00:00"):
        raise AssertionError(f"sampled_at is not UTC ISO text: {sampled_at!r}")
    parsed = datetime.fromisoformat(sampled_at)
    if parsed.tzinfo is None or parsed.utcoffset().total_seconds() != 0:
        raise AssertionError(f"sampled_at is not tz-aware UTC: {sampled_at!r}")


def test_oom_derivation_uses_systemd_result_only() -> None:
    sampler = load_sampler()
    if sampler.derive_oom_killed("oom-kill") is not True:
        raise AssertionError("oom-kill result must derive true")
    if sampler.derive_oom_killed("exit-code") is not False:
        raise AssertionError("exit-code result must derive false")
    if sampler.derive_oom_killed("success") is not False:
        raise AssertionError("success result must derive false")
    if sampler.derive_oom_killed(None) is not None:
        raise AssertionError("missing result must derive null")
    service = {"result": "exit-code", "exec_main_status": 137}
    if sampler.derive_oom_killed(service["result"]) is True:
        raise AssertionError("ExecMainStatus 137 must not be treated as OOM")


def test_disk_math_uses_bavail_frsize_and_df_denominator() -> None:
    sampler = load_sampler()
    metrics = sampler.disk_metrics_from_statvfs("/data", FakeStatvfs(), 123)
    if metrics["free_bytes"] != FakeStatvfs.f_bavail * FakeStatvfs.f_frsize:
        raise AssertionError(f"free_bytes used the wrong field: {metrics}")
    used = (FakeStatvfs.f_blocks - FakeStatvfs.f_bfree) * FakeStatvfs.f_frsize
    avail = FakeStatvfs.f_bavail * FakeStatvfs.f_frsize
    expected_pct = round(used / (used + avail) * 100, 2)
    wrong_total_pct = round(used / (FakeStatvfs.f_blocks * FakeStatvfs.f_frsize) * 100, 2)
    if expected_pct == wrong_total_pct:
        raise AssertionError("test fixture does not distinguish df denominator from total")
    if metrics["used_pct"] != expected_pct:
        raise AssertionError(f"used_pct does not match df convention: {metrics}")


def test_unknown_service_still_emits_valid_json_and_exits_zero() -> None:
    result = run_sampler("--service", "definitely-not-a-real-unit-xyz")
    if result.returncode != 0:
        raise AssertionError(f"expected exit 0, got {result.returncode}\nstderr:\n{result.stderr}")
    lines = result.stdout.splitlines()
    if len(lines) != 1:
        raise AssertionError(f"expected one JSON line, got {len(lines)}: {result.stdout!r}")
    record = json.loads(lines[0])
    assert_sample_schema(record)
    service = record["service"]
    if service is not None:
        load_state = service.get("load_state")
        non_null_values = [value for key, value in service.items() if key != "unit" and value is not None]
        if load_state != "not-found" and non_null_values:
            raise AssertionError(f"unexpected service data for absent service: {service!r}")


def test_failed_collector_becomes_null_error_without_exception() -> None:
    sampler = load_sampler()
    original = sampler.collect_memory

    def fail_memory():
        raise RuntimeError("synthetic memory failure")

    sampler.collect_memory = fail_memory
    try:
        record = sampler.sample()
    finally:
        sampler.collect_memory = original
    if record["memory"] is not None:
        raise AssertionError(f"failed memory collector should produce null block: {record['memory']!r}")
    if not any("memory: synthetic memory failure" in error for error in record["errors"]):
        raise AssertionError(f"missing degraded collector error: {record['errors']!r}")


def test_disk_path_tempdir_populates_disk_block() -> None:
    sampler = load_sampler()
    with tempfile.TemporaryDirectory(prefix="host-health-disk.") as temp:
        record = sampler.sample(disk_path=temp)
        disk = record["disk"]
        if disk["path"] != str(Path(temp).resolve()):
            raise AssertionError(f"disk path was not realpath of temp dir: {disk!r}")
        for key in ("total_bytes", "free_bytes", "used_pct"):
            if disk[key] is None:
                raise AssertionError(f"disk {key} should be populated: {disk!r}")


def test_help_works_from_tmp() -> None:
    result = run_sampler("--help", cwd=Path("/tmp"))
    if result.returncode != 0:
        raise AssertionError(f"--help failed with {result.returncode}\nstderr:\n{result.stderr}")
    if "usage:" not in result.stdout:
        raise AssertionError(f"--help did not print usage: {result.stdout!r}")


def test_no_args_from_tmp_prints_exactly_one_valid_json_object() -> None:
    result = run_sampler(cwd=Path("/tmp"))
    if result.returncode != 0:
        raise AssertionError(f"no-args run failed with {result.returncode}\nstderr:\n{result.stderr}")
    lines = result.stdout.splitlines()
    if len(lines) != 1:
        raise AssertionError(f"expected one JSON line, got {len(lines)}: {result.stdout!r}")
    record = json.loads(lines[0])
    assert_sample_schema(record)


def main() -> int:
    tests = [
        test_sample_returns_json_serializable_schema,
        test_sample_timestamp_is_utc_and_schema_version_is_two,
        test_oom_derivation_uses_systemd_result_only,
        test_disk_math_uses_bavail_frsize_and_df_denominator,
        test_unknown_service_still_emits_valid_json_and_exits_zero,
        test_failed_collector_becomes_null_error_without_exception,
        test_disk_path_tempdir_populates_disk_block,
        test_help_works_from_tmp,
        test_no_args_from_tmp_prints_exactly_one_valid_json_object,
    ]
    for test in tests:
        test()
    print("OK: host-health sampler self-tests passed.")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
