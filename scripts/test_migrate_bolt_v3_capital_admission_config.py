#!/usr/bin/env python3
"""Tests for the Bolt-v3 capital-admission TOML config migrator."""

from __future__ import annotations

import importlib.util
import contextlib
import io
import json
import sys
import tempfile
import tomllib
from pathlib import Path


SCRIPT_PATH = Path(__file__).with_name("migrate_bolt_v3_capital_admission_config.py")
SPEC = importlib.util.spec_from_file_location("migrate_bolt_v3_capital_admission_config", SCRIPT_PATH)
if SPEC is None or SPEC.loader is None:
    raise RuntimeError(f"failed to load {SCRIPT_PATH}")
MIGRATOR = importlib.util.module_from_spec(SPEC)
sys.modules[SPEC.name] = MIGRATOR
SPEC.loader.exec_module(MIGRATOR)


def run_migration(path: Path, *, dry_run: bool = False) -> dict[str, object]:
    argv = [str(path)]
    if dry_run:
        argv.append("--dry-run")
    return MIGRATOR.migrate_cli(argv)


def test_migrates_root_schema_and_real_capital_pool_headers_exactly() -> None:
    temp = tempfile.TemporaryDirectory()
    tmp_path = Path(temp.name)
    path = tmp_path / "root.toml"
    original = """schema_version = 1 # root only
trader_id = "BOLT-001"
strategy_files = ["strategies/binary_oracle.toml"]

# sizing_policy in a comment must remain untouched.
[risk]
default_max_notional_per_order = "10.00"

[[risk.capital_pools]]
pool_id = "polymarket-prediction-live"
venue_id = "POLYMARKET"

[risk.capital_pools.sizing_policy]
min_remaining_pool_balance = "1.00" # keep this comment

[risk.capital_pools.sizing_policy.fee_slippage]
max_fee_liability = "0.10"
max_slippage_liability = "0.20"

[chainlink_data_streams.feed_bindings]
report_schema_version = 3
"""
    expected = """schema_version = 2 # root only
trader_id = "BOLT-001"
strategy_files = ["strategies/binary_oracle.toml"]

# sizing_policy in a comment must remain untouched.
[risk]
default_max_notional_per_order = "10.00"

[[risk.capital_pools]]
pool_id = "polymarket-prediction-live"
venue_id = "POLYMARKET"

[risk.capital_pools.capital_admission_policy]
min_remaining_pool_balance = "1.00" # keep this comment

[risk.capital_pools.capital_admission_policy.fee_slippage]
max_fee_liability = "0.10"
max_slippage_liability = "0.20"

[chainlink_data_streams.feed_bindings]
report_schema_version = 3
"""
    path.write_text(original, encoding="utf-8")

    manifest = run_migration(path)

    assert path.read_text(encoding="utf-8") == expected
    assert len(manifest["changed_files"]) == 1
    parsed = tomllib.loads(path.read_text(encoding="utf-8"))
    assert parsed["schema_version"] == 2
    policy = parsed["risk"]["capital_pools"][0]["capital_admission_policy"]
    assert policy["min_remaining_pool_balance"] == "1.00"
    assert policy["fee_slippage"]["max_fee_liability"] == "0.10"
    temp.cleanup()


def test_migrates_multiple_pool_bare_dotted_and_inline_keys_only_in_pool_context() -> None:
    temp = tempfile.TemporaryDirectory()
    tmp_path = Path(temp.name)
    path = tmp_path / "root.toml"
    original = """schema_version = 1
trader_id = "BOLT-001"
strategy_files = ["strategies/one.toml"]

[[risk.capital_pools]]
pool_id = "pool-a"
sizing_policy.min_remaining_pool_balance = "1.00"
sizing_policy = { fee_slippage = { max_fee_liability = "0.10", max_slippage_liability = "0.20" } }

[[risk.capital_pools]]
pool_id = "pool-b"
  sizing_policy   = { min_remaining_pool_balance = "2.00" }

[unrelated]
sizing_policy = "leave-me"
note = "sizing_policy is a value and must not change"

[[risk.other_pools]]
sizing_policy = "also-leave-me"
"""
    expected = """schema_version = 2
trader_id = "BOLT-001"
strategy_files = ["strategies/one.toml"]

[[risk.capital_pools]]
pool_id = "pool-a"
capital_admission_policy.min_remaining_pool_balance = "1.00"
capital_admission_policy = { fee_slippage = { max_fee_liability = "0.10", max_slippage_liability = "0.20" } }

[[risk.capital_pools]]
pool_id = "pool-b"
  capital_admission_policy   = { min_remaining_pool_balance = "2.00" }

[unrelated]
sizing_policy = "leave-me"
note = "sizing_policy is a value and must not change"

[[risk.other_pools]]
sizing_policy = "also-leave-me"
"""
    path.write_text(original, encoding="utf-8")

    run_migration(path)

    assert path.read_text(encoding="utf-8") == expected
    temp.cleanup()


def test_root_scope_schema_version_only_never_nested_schema_keys() -> None:
    temp = tempfile.TemporaryDirectory()
    tmp_path = Path(temp.name)
    path = tmp_path / "root.toml"
    path.write_text(
        """schema_version = 1
strategy_files = ["strategies/one.toml"]

[risk]
schema_version = 1

[[risk.capital_pools]]
pool_id = "pool-a"

[risk.capital_pools.sizing_policy]
min_remaining_pool_balance = "1.00"

[chainlink_data_streams.feed_bindings]
report_schema_version = 3
""",
        encoding="utf-8",
    )

    run_migration(path)

    migrated = path.read_text(encoding="utf-8")
    assert "schema_version = 2\nstrategy_files" in migrated
    assert "\n[risk]\nschema_version = 1\n" in migrated
    assert "report_schema_version = 3" in migrated
    temp.cleanup()


def test_dry_run_prints_diff_and_manifest_without_mutating() -> None:
    temp = tempfile.TemporaryDirectory()
    tmp_path = Path(temp.name)
    path = tmp_path / "root.toml"
    original = """schema_version = 1
strategy_files = ["strategies/one.toml"]

[[risk.capital_pools]]
pool_id = "pool-a"

[risk.capital_pools.sizing_policy]
min_remaining_pool_balance = "1.00"
"""
    path.write_text(original, encoding="utf-8")

    stdout = io.StringIO()
    with contextlib.redirect_stdout(stdout):
        exit_code = MIGRATOR.main([str(path), "--dry-run"])

    captured = stdout.getvalue()
    assert exit_code == 0
    assert path.read_text(encoding="utf-8") == original
    assert "-schema_version = 1" in captured
    assert "+schema_version = 2" in captured
    assert "capital_admission_policy" in captured
    manifest = json.loads(captured.rsplit("\n", 2)[-2])
    assert manifest["changed_files"][0]["path"] == str(path)
    temp.cleanup()


def test_directory_mode_does_not_mutate_unrelated_clean_merged_config() -> None:
    temp = tempfile.TemporaryDirectory()
    tmp_path = Path(temp.name)
    root = tmp_path / "config"
    root.mkdir()
    bolt = root / "root.toml"
    clean_merged = root / "clean-merged.toml"
    bolt.write_text(
        """schema_version = 1
strategy_files = ["strategies/one.toml"]

[[risk.capital_pools]]
pool_id = "pool-a"

[risk.capital_pools.sizing_policy]
min_remaining_pool_balance = "1.00"
""",
        encoding="utf-8",
    )
    clean_merged.write_text(
        """schema_version = 1

[clean-merged]
enabled = true
""",
        encoding="utf-8",
    )

    manifest = run_migration(root)

    assert len(manifest["changed_files"]) == 1
    assert manifest["changed_files"][0]["path"] == str(bolt)
    assert clean_merged.read_text(encoding="utf-8").startswith("schema_version = 1\n")
    temp.cleanup()


def main() -> int:
    tests = [
        value
        for name, value in sorted(globals().items())
        if name.startswith("test_") and callable(value)
    ]
    for test in tests:
        test()
    return 0


if __name__ == "__main__":
    import lane_governor

    lane_governor.acquire()
    raise SystemExit(main())
