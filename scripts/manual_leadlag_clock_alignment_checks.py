#!/usr/bin/env -S uv run --script
# /// script
# requires-python = ">=3.11"
# dependencies = [
#     "duckdb==1.4.1",
#     "polars==1.34.0",
#     "pyarrow==22.0.0",
#     "numpy==2.3.4",
#     "requests==2.32.5",
#     "lz4==4.4.4",
#     "websockets==15.0.1",
#     "ntplib==0.4.0",
# ]
# ///
"""Manual self-tests for the #633 clock-alignment research guards.

Run with `uv run --script scripts/manual_leadlag_clock_alignment_checks.py`; these
research-script dependency checks are intentionally outside source-fence CI.
"""

from __future__ import annotations

import argparse
import asyncio
import pathlib
import sys
import tempfile
from contextlib import contextmanager
from typing import Any, Callable, Iterator


REPO_ROOT = pathlib.Path(__file__).resolve().parents[1]
SCRIPTS_DIR = REPO_ROOT / "scripts"


def ensure_test_imports_available() -> None:
    scripts_dir = str(SCRIPTS_DIR)
    if scripts_dir not in sys.path:
        sys.path.insert(0, scripts_dir)


ensure_test_imports_available()

import leadlag_clock_alignment as lca  # noqa: E402
import leadlag_session4 as s4  # noqa: E402
import polars as pl  # noqa: E402


@contextmanager
def patched_attrs(patches: list[tuple[object, str, object]]) -> Iterator[None]:
    originals = [(obj, name, getattr(obj, name)) for obj, name, _ in patches]
    for obj, name, value in patches:
        setattr(obj, name, value)
    try:
        yield
    finally:
        for obj, name, original in reversed(originals):
            setattr(obj, name, original)


def expect_system_exit(func: Callable[[], Any], expected: str) -> None:
    try:
        func()
    except SystemExit as exc:
        if expected not in str(exc):
            raise AssertionError(f"expected {expected!r} in {exc!r}") from exc
    else:
        raise AssertionError(f"expected SystemExit containing {expected!r}")


def reset_pm_clock_registry() -> None:
    s4.PM_CLOCK_RESOLVED.clear()


def test_corrected_clock_compensates_wall_step() -> None:
    wall_ms = 100_000.0
    mono_ms = 1_000.0

    def time_ns() -> int:
        return int(wall_ms * 1_000_000)

    def monotonic_ns() -> int:
        return int(mono_ms * 1_000_000)

    with patched_attrs([(lca.time, "time_ns", time_ns), (lca.time, "monotonic_ns", monotonic_ns)]):
        clock = lca.CorrectedClock(initial_offset_ms=10.0)
        wall_ms = 100_100.0
        mono_ms = 1_100.0
        before_step = clock.now()
        if before_step != 100_110.0:
            raise AssertionError(f"unexpected corrected time before step: {before_step}")

        wall_ms = 102_200.0
        mono_ms = 1_200.0
        dropped = clock.now()
        if dropped is not None:
            raise AssertionError("step sample must be dropped")
        if not clock.steps:
            raise AssertionError("clock step must be recorded")

        wall_ms = 102_300.0
        mono_ms = 1_300.0
        after_step = clock.now()
        if after_step != 100_310.0:
            raise AssertionError(f"corrected time must stay monotonic across wall step: {after_step}")


def test_select_pm_clock_guards() -> None:
    reset_pm_clock_registry()
    old_cache = pl.DataFrame({"asset_id": ["tok"], "ts_ms": [10], "price": [0.5]})
    selected = s4.select_pm_clock(old_cache, "auto", "old")
    if selected.columns != ["asset_id", "ts_ms", "price"]:
        raise AssertionError(f"old auto cache must stay on receive clock: {selected.columns}")
    if s4.PM_CLOCK_RESOLVED["old"] != "receive":
        raise AssertionError("old auto cache must register receive clock")

    reset_pm_clock_registry()
    expect_system_exit(
        lambda: s4.select_pm_clock(old_cache, "venue", "old"),
        "pm-clock=venue but extracts lack ts_venue_ms",
    )

    reset_pm_clock_registry()
    null_venue = pl.DataFrame({"asset_id": ["tok"], "ts_ms": [10], "ts_venue_ms": [None]})
    expect_system_exit(
        lambda: s4.select_pm_clock(null_venue, "auto", "mixed"),
        "under pm-clock=auto (resolved venue)",
    )

    reset_pm_clock_registry()
    new_cache = pl.DataFrame({"asset_id": ["tok"], "ts_ms": [10], "ts_venue_ms": [9]})
    s4.select_pm_clock(old_cache, "receive", "same")
    expect_system_exit(
        lambda: s4.select_pm_clock(new_cache, "venue", "same"),
        "PM clock changed within one run",
    )

    reset_pm_clock_registry()
    expect_system_exit(s4.pm_clock_provenance, "no PM extracts loaded")
    s4.PM_CLOCK_RESOLVED.update({"pm_tob": "receive", "pm_trades": "venue"})
    expect_system_exit(s4.pm_clock_provenance, "mixed PM clocks within one run")


def test_mixed_cache_concat_raises_guided_error() -> None:
    with tempfile.TemporaryDirectory() as tmp:
        root = pathlib.Path(tmp)
        old_dir = root / "pm_trades" / "2026-04-22"
        new_dir = root / "pm_trades" / "2026-04-23"
        old_dir.mkdir(parents=True)
        new_dir.mkdir(parents=True)
        pl.DataFrame({"asset_id": ["tok"], "ts_ms": [10], "price": [0.5]}).write_parquet(old_dir / "old.parquet")
        pl.DataFrame({"asset_id": ["tok"], "ts_ms": [20], "ts_venue_ms": [19], "price": [0.6]}).write_parquet(
            new_dir / "new.parquet"
        )
        reset_pm_clock_registry()
        expect_system_exit(
            lambda: s4.load_trades(root, ["2026-04-22", "2026-04-23"], {"tok"}, "auto"),
            "pm_trades: mixed ts_venue_ms presence",
        )


def test_mixed_cache_receive_mode_preserves_receive_timestamps() -> None:
    with tempfile.TemporaryDirectory() as tmp:
        root = pathlib.Path(tmp)
        old_dir = root / "pm_trades" / "2026-04-22"
        new_dir = root / "pm_trades" / "2026-04-23"
        old_dir.mkdir(parents=True)
        new_dir.mkdir(parents=True)
        pl.DataFrame({"asset_id": ["tok"], "ts_ms": [10], "price": [0.5]}).write_parquet(old_dir / "old.parquet")
        pl.DataFrame({"asset_id": ["tok"], "ts_ms": [20], "ts_venue_ms": [19], "price": [0.6]}).write_parquet(
            new_dir / "new.parquet"
        )
        reset_pm_clock_registry()
        trades = s4.load_trades(root, ["2026-04-22", "2026-04-23"], {"tok"}, "receive")
        if trades["ts_ms"].to_list() != [10, 20]:
            raise AssertionError("receive mode must preserve collector receive timestamps")
        if "ts_venue_ms" in trades.columns:
            raise AssertionError("receive mode must drop venue timestamp column before concat")


def test_lake_missing_checkpoint_names_date() -> None:
    payload = {
        "rows": [["price_change", 1, 0, 1.0, [1.0] * len(lca.OFFSET_PERCENTILES), 1.0]],
        "hourly_med_min": 1.0,
        "hourly_med_max": 1.0,
    }

    def no_rename(self: pathlib.Path, target: pathlib.Path) -> None:
        _ = (self, target)

    with tempfile.TemporaryDirectory() as tmp:
        args = argparse.Namespace(dates="2026-04-22", workdir=tmp, report=None)
        with patched_attrs(
            [
                (lca, "lake_connect", lambda: object()),
                (lca, "lake_scan_date", lambda _con, _date: payload),
                (lca.Path, "rename", no_rename),
            ]
        ):
            try:
                lca.cmd_lake(args)
            except FileNotFoundError as exc:
                raise AssertionError("missing lake checkpoint must fail with guided SystemExit") from exc
            except SystemExit as exc:
                if "missing checkpoint for 2026-04-22" not in str(exc):
                    raise AssertionError(f"unexpected lake checkpoint error: {exc}") from exc
            else:
                raise AssertionError("missing lake checkpoint must stop report assembly")


async def _empty_probe(_asset: str, _deadline: float, _offsets: dict[str, list[float]], _clock: lca.CorrectedClock) -> None:
    return None


async def _empty_reanchor(_clock: lca.CorrectedClock, _deadline: float) -> None:
    return None


def test_live_probe_rejects_zero_sample_sources() -> None:
    with patched_attrs(
        [
            (lca, "probe_polymarket", _empty_probe),
            (lca, "probe_bybit", _empty_probe),
            (lca, "probe_hyperliquid", _empty_probe),
            (lca, "ntp_reanchor", _empty_reanchor),
        ]
    ):
        expect_system_exit(
            lambda: asyncio.run(lca.run_live_probe("btc", 0.0, lca.CorrectedClock(0.0))),
            "captured zero samples",
        )


async def _dead_probe(_asset: str, _deadline: float, _offsets: dict[str, list[float]], _clock: lca.CorrectedClock) -> None:
    raise RuntimeError("boom")


async def _sleeping_probe(
    _asset: str, _deadline: float, _offsets: dict[str, list[float]], _clock: lca.CorrectedClock
) -> None:
    await asyncio.sleep(10.0)


async def _sleeping_reanchor(_clock: lca.CorrectedClock, _deadline: float) -> None:
    await asyncio.sleep(10.0)


def test_live_probe_surfaces_dead_task_before_deadline() -> None:
    async def run_with_timeout() -> None:
        await asyncio.wait_for(lca.run_live_probe("btc", 10.0, lca.CorrectedClock(0.0)), timeout=0.2)

    with patched_attrs(
        [
            (lca, "probe_polymarket", _dead_probe),
            (lca, "probe_bybit", _sleeping_probe),
            (lca, "probe_hyperliquid", _sleeping_probe),
            (lca, "ntp_reanchor", _sleeping_reanchor),
        ]
    ):
        try:
            asyncio.run(run_with_timeout())
        except TimeoutError as exc:
            raise AssertionError("dead probe task must surface before the run deadline") from exc
        except SystemExit as exc:
            if "polymarket: RuntimeError: boom" not in str(exc):
                raise AssertionError(f"unexpected dead probe error: {exc}") from exc
        else:
            raise AssertionError("dead probe task must raise SystemExit")


def main() -> int:
    test_corrected_clock_compensates_wall_step()
    test_select_pm_clock_guards()
    test_mixed_cache_concat_raises_guided_error()
    test_mixed_cache_receive_mode_preserves_receive_timestamps()
    test_lake_missing_checkpoint_names_date()
    test_live_probe_rejects_zero_sample_sources()
    test_live_probe_surfaces_dead_task_before_deadline()
    print("OK: lead-lag clock-alignment self-tests passed.")
    return 0


if __name__ == "__main__":
    import lane_governor

    lane_governor.acquire()
    sys.exit(main())
