# /// script
# requires-python = ">=3.11"
# dependencies = [
#     "duckdb==1.4.1",
#     "polars==1.34.0",
#     "pyarrow==22.0.0",
#     "numpy==2.3.4",
#     "requests==2.32.5",
#     "lz4==4.4.4",
# ]
# ///
"""Lead-lag follow-up (issue #626): sub-second repricing + displayed-size fillability.

Companion to scripts/leadlag_session4.py (imported as a module; same caches under
~/.cache/bolt-leadlag-session4/). Two questions left open by the session-4 report
(docs/research/leadlag-taker-edge-2026-06-10.md):

1. `subsecond` — the session-4 study was 1-second granular; eth/sol/xrp books fully
   reprice "within 1s". Polymarket tob extracts carry millisecond timestamps, so this
   measures the net taker edge entering at the ask observable at t0+delta for sub-second
   deltas: at what reaction latency does each asset's edge survive?
   Caveat carried over: the leader is ~0.54s-cadence snapshots, so t0 (detection) lags
   the true move start by up to ~0.54s; deltas are reaction latency AFTER detection.

2. `extract-sizes` + `fillability` — the GO verdict assumed the displayed ask is
   fillable. This re-extracts the tob stream WITH the level-delta price/size columns and
   measures the displayed size (shares and $ notional) resting at the best ask at signal
   time — an upper bound on what an IOC can capture without walking the book. This is
   the RA raw receive-offset fallback and sunsets when #677 makes the converter write
   `ts_init = capture_time` into the NT catalog.

Reproduction (after the session-4 extracts exist):
  uv run scripts/leadlag_subsecond.py subsecond     --dates 2026-04-22:2026-04-28
  uv run scripts/leadlag_subsecond.py extract-sizes --dates 2026-04-22:2026-04-28
  uv run scripts/leadlag_subsecond.py fillability   --dates 2026-04-22:2026-04-28
"""

from __future__ import annotations

import argparse
import math
import subprocess
import sys
import tempfile
from pathlib import Path

import duckdb
import numpy as np
import polars as pl

sys.path.insert(0, str(Path(__file__).resolve().parent))
import leadlag_session4 as s4  # noqa: E402

DELTAS_SECS = (0.1, 0.25, 0.5, 0.75, 1.0, 2.0)
MARK_HORIZON_SECS = 30  # the session-4 best cell horizon
SIZE_PROBE_DELTA_SECS = 0.25  # fillability probe: size at ask shortly after detection
SUBSECOND_THRESHOLDS_BPS = (5.0, 10.0)  # X=20 had <=4 events in window; skip
# Event-clock registry for `fillability --leader`: maps the CLI choice to the
# leader-parquet subdir under the workdir (same <subdir>/<date>/<COIN>.parquet
# layout for every clock). Missing keys must raise (fail loud), never default.
LEADER_SUBDIRS = {"hl": "leader", "trades": "leader_trades"}


def sized_tob_path(workdir: Path, date: str, stem: str) -> Path:
    return workdir / "pm_tob_sized" / date / f"{stem}.parquet"


# ----- subsecond: net edge vs reaction latency -------------------------------


def cmd_subsecond(args: argparse.Namespace) -> None:
    workdir = Path(args.workdir)
    dates = s4.parse_dates(args.dates)
    assets = args.assets.split(",")
    cycles = s4.load_cycles(workdir, dates, assets)
    cycles_by_key = {(c.asset, c.start): c for c in cycles}
    all_tokens = {c.up_token for c in cycles} | {c.down_token for c in cycles}
    print(f"subsecond: {len(cycles)} cycles; loading extracts ...", flush=True)
    books = s4.load_token_books(workdir, dates, all_tokens, pm_clock=args.pm_clock)

    net: dict[tuple[str, float, float], list[float]] = {}
    pre_move_net: dict[tuple[str, float], list[float]] = {}
    for asset in assets:
        for date in dates:
            leader_file = s4.leader_path(workdir, date, s4.LEADER_COIN_BY_ASSET[asset])
            if not leader_file.exists():
                print(f"subsecond: SKIPPING {asset} {date}: no leader file {leader_file}", flush=True)
                continue
            leader = s4.LeaderSeries(pl.read_parquet(leader_file))
            base = s4.day_epoch(date)
            for x_bps in SUBSECOND_THRESHOLDS_BPS:
                for t, direction in s4.detect_events(leader, base, x_bps):
                    cycle = cycles_by_key.get((asset, (t // s4.CADENCE_SECS) * s4.CADENCE_SECS))
                    if cycle is None or t - cycle.start < s4.MIN_SECS_AFTER_OPEN:
                        continue
                    if cycle.end - t < MARK_HORIZON_SECS + 2:
                        continue
                    token = cycle.up_token if direction > 0 else cycle.down_token
                    book = books.get(token)
                    if book is None:
                        continue
                    mark = book.asof(t + MARK_HORIZON_SECS)
                    pre = book.asof(t - 1)
                    if mark is None or pre is None:
                        continue
                    mark_mid = (mark[0] + mark[1]) / 2.0
                    fee_pre = s4.taker_fee_dollars(cycle.taker_fee_rate, pre[1])
                    pre_move_net.setdefault((asset, x_bps), []).append(
                        (mark_mid - pre[1] - fee_pre) * 100
                    )
                    for delta in DELTAS_SECS:
                        entry = book.asof(t + delta)
                        if entry is None:
                            continue
                        fee = s4.taker_fee_dollars(cycle.taker_fee_rate, entry[1])
                        net.setdefault((asset, x_bps, delta), []).append(
                            (mark_mid - entry[1] - fee) * 100
                        )

    rows = []
    for asset in assets:
        for x_bps in SUBSECOND_THRESHOLDS_BPS:
            pre = pre_move_net.get((asset, x_bps), [])
            if pre:
                mean, lo, hi = s4.mean_ci(pre)
                rows.append([asset, f"{x_bps:.0f}", "pre-move (t-1s)", f"{len(pre):,}",
                             f"{mean:+.2f}", f"[{lo:+.2f}, {hi:+.2f}]" if not math.isnan(lo) else "-"])
            for delta in DELTAS_SECS:
                vals = net.get((asset, x_bps, delta), [])
                if not vals:
                    continue
                mean, lo, hi = s4.mean_ci(vals)
                rows.append([asset, f"{x_bps:.0f}", f"+{delta:g}s", f"{len(vals):,}",
                             f"{mean:+.2f}", f"[{lo:+.2f}, {hi:+.2f}]" if not math.isnan(lo) else "-"])
    table = s4.md_table(
        ["asset", "X (bps)", "entry at detection", f"n (mark {MARK_HORIZON_SECS}s)",
         "mean net (c)", "95% CI"],
        rows,
    )
    out = f"<!-- pm-clock: {s4.pm_clock_provenance()} -->\n<!-- section:subsecond -->\n{table}\n"
    if args.report:
        Path(args.report).write_text(out)
        print(f"subsecond: wrote {args.report}")
    else:
        print(out)


# ----- extract-sizes: tob re-extraction with level-delta size columns --------


def extract_sized_object(workdir: Path, date: str, key: str, tokens: list[str]) -> str:
    stem = Path(key).name.split("=")[-1].removesuffix(".parquet")[:16]
    out = sized_tob_path(workdir, date, stem)
    if out.exists():
        return f"{date}/{stem} cached"
    out.parent.mkdir(parents=True, exist_ok=True)
    token_list = ",".join(f"'{t}'" for t in tokens)
    with tempfile.TemporaryDirectory() as tmp:
        local = Path(tmp) / "obj.parquet"
        subprocess.run(
            ["aws", "s3", "cp", f"s3://bolt-parquet/{key}", str(local), "--only-show-errors"],
            check=True,
        )
        with duckdb.connect() as con:
            con.execute("SET threads=4;")
            frame = con.execute(
                f"""
                SELECT asset_id, CAST(epoch_ms(timestamp_received) AS BIGINT) AS ts_ms,
                       CAST(epoch_ms(timestamp) AS BIGINT) AS ts_venue_ms,
                       CAST(best_bid AS DOUBLE) AS best_bid, CAST(best_ask AS DOUBLE) AS best_ask,
                       CAST(price AS DOUBLE) AS level_price, CAST(size AS DOUBLE) AS level_size,
                       side
                FROM read_parquet('{local}')
                WHERE event_type = 'price_change' AND asset_id IN ({token_list})
                ORDER BY asset_id, ts_ms
                """
            ).pl()
    frame.write_parquet(out)
    return f"{date}/{stem}: rows={frame.height}"


def cmd_extract_sizes(args: argparse.Namespace) -> None:
    workdir = Path(args.workdir)
    dates = s4.parse_dates(args.dates)
    cycles = s4.load_cycles(workdir, dates, args.assets.split(","))
    jobs = []
    for date in dates:
        tokens = s4.tokens_for_day(cycles, s4.day_epoch(date))
        if not tokens:
            continue
        keys = s4.list_s3(f"{s4.PMXT_S3_PREFIX}/dt={date}/")
        if not keys:
            raise SystemExit(f"no pmxt staging objects for dt={date}")
        jobs += [(date, key, tokens) for key in keys]
    s4.run_pool(
        "extract-sizes",
        args.concurrency,
        [lambda d=d, k=k, t=t: extract_sized_object(workdir, d, k, t) for d, k, t in jobs],
    )


# ----- fillability: displayed size at the best ask at signal time ------------


class SizedBook:
    """Per-token level-delta stream; displayed size at the current best ask."""

    def __init__(self, frame: pl.DataFrame) -> None:
        self.ts = frame["ts_ms"].to_numpy()
        self.best_ask = frame["best_ask"].to_numpy()
        self.level_price = frame["level_price"].to_numpy()
        self.level_size = frame["level_size"].to_numpy()
        self.side = frame["side"].to_numpy()

    def ask_and_size(self, t_secs: float, lookback_secs: float = 120.0) -> tuple[float, float] | None:
        t_ms = int(t_secs * 1000)
        idx = int(np.searchsorted(self.ts, t_ms, side="right")) - 1
        if idx < 0 or t_ms - int(self.ts[idx]) > s4.MAX_QUOTE_AGE_SECS * 1000:
            return None
        ask = float(self.best_ask[idx])
        if not (0.0 < ask < 1.0):
            return None
        lo_ms = t_ms - int(lookback_secs * 1000)
        lo_idx = int(np.searchsorted(self.ts, lo_ms, side="left"))
        if lo_idx > idx:
            return None
        mask = (self.side[lo_idx : idx + 1] == "SELL") & (self.level_price[lo_idx : idx + 1] == ask)
        matching = np.flatnonzero(mask)
        if matching.size > 0:
            return ask, float(self.level_size[lo_idx + matching[-1]])
        return None


def cmd_fillability(args: argparse.Namespace) -> None:
    workdir = Path(args.workdir)
    dates = s4.parse_dates(args.dates)
    assets = args.assets.split(",")
    cycles = s4.load_cycles(workdir, dates, assets)
    cycles_by_key = {(c.asset, c.start): c for c in cycles}
    token_set = {c.up_token for c in cycles} | {c.down_token for c in cycles}

    # One day at a time: a full-window load is ~900M level-delta rows and does not
    # fit in memory; per-day is ~130M rows and the event evaluation is day-local.
    sizes: dict[tuple[str, float], list[tuple[float, float]]] = {}
    for date in dates:
        directory = workdir / "pm_tob_sized" / date
        paths = sorted(directory.glob("*.parquet")) if directory.exists() else []
        if not paths:
            raise SystemExit(f"no pm_tob_sized extracts for {date}; run `extract-sizes` first")
        print(f"fillability: {date} loading {len(paths)} sized extracts ...", flush=True)
        frames = [pl.read_parquet(p).filter(pl.col("asset_id").is_in(list(token_set))) for p in paths]
        merged = s4.select_pm_clock(
            s4.concat_pm_extract_frames(frames, args.pm_clock, f"pm_tob_sized/{date}"),
            args.pm_clock,
            f"pm_tob_sized/{date}",
        ).sort("asset_id", "ts_ms")
        sized = {key[0]: SizedBook(f) for key, f in merged.partition_by("asset_id", as_dict=True).items()}
        del merged
        base = s4.day_epoch(date)
        for asset in assets:
            # --leader selects the event clock (see LEADER_SUBDIRS): "hl" = HL
            # book-mid snapshots (#626 baseline), "trades" = tick-trades parquets
            # built by leadlag_trades_leader.py extract-leader (#631 fast clock).
            # The clock is a parameter, not an asset- or venue-specific code path.
            subdir = LEADER_SUBDIRS[args.leader]
            leader_file = workdir / subdir / date / f"{s4.LEADER_COIN_BY_ASSET[asset]}.parquet"
            if not leader_file.exists():
                # loud, or a short-coverage clock (bybit lake ends mid-window)
                # silently shrinks the analyzed window inside a full-window label
                print(f"fillability: SKIPPING {asset} {date}: no {args.leader} leader file {leader_file}", flush=True)
                continue
            leader = s4.LeaderSeries(pl.read_parquet(leader_file))
            for x_bps in SUBSECOND_THRESHOLDS_BPS:
                for t, direction in s4.detect_events(leader, base, x_bps):
                    cycle = cycles_by_key.get((asset, (t // s4.CADENCE_SECS) * s4.CADENCE_SECS))
                    if cycle is None or t - cycle.start < s4.MIN_SECS_AFTER_OPEN:
                        continue
                    token = cycle.up_token if direction > 0 else cycle.down_token
                    book = sized.get(token)
                    if book is None:
                        continue
                    probe = book.ask_and_size(t + SIZE_PROBE_DELTA_SECS)
                    if probe is None:
                        continue
                    ask, size = probe
                    sizes.setdefault((asset, x_bps), []).append((size, size * ask))
        del sized

    rows = []
    for asset in assets:
        for x_bps in SUBSECOND_THRESHOLDS_BPS:
            vals = sizes.get((asset, x_bps), [])
            if not vals:
                continue
            shares = sorted(v[0] for v in vals)
            notion = sorted(v[1] for v in vals)
            n = len(vals)
            rows.append(
                [asset, f"{x_bps:.0f}", f"{n:,}",
                 f"{shares[n // 2]:,.0f}", f"{shares[n // 4]:,.0f}",
                 f"${notion[n // 2]:,.0f}", f"${notion[n // 4]:,.0f}",
                 f"{100 * sum(1 for v in notion if v >= 100) / n:.0f}%",
                 f"{100 * sum(1 for v in notion if v >= 1000) / n:.0f}%"]
            )
    table = s4.md_table(
        ["asset", "X (bps)", "events", "median shares", "p25 shares",
         "median notional", "p25 notional", "≥$100", "≥$1000"],
        rows,
    )
    out = (
        f"<!-- pm-clock: {s4.pm_clock_provenance()} -->\n"
        f"<!-- section:fillability (size at best ask {SIZE_PROBE_DELTA_SECS}s after "
        f"detection, {args.leader} clock) -->\n"
        f"{table}\n"
    )
    if args.report:
        Path(args.report).write_text(out)
        print(f"fillability: wrote {args.report}")
    else:
        print(out)


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    sub = parser.add_subparsers(dest="command", required=True)

    def common(p: argparse.ArgumentParser) -> None:
        p.add_argument("--dates", required=True)
        p.add_argument("--assets", default=s4.DEFAULT_ASSETS)
        p.add_argument("--workdir", default=str(s4.DEFAULT_WORKDIR))
        p.add_argument("--report", default="", help="write tables to this file instead of stdout")

    def pm_clock_flag(p: argparse.ArgumentParser) -> None:
        p.add_argument(
            "--pm-clock",
            choices=s4.PM_CLOCK_CHOICES,
            default=s4.DEFAULT_PM_CLOCK,
            help="PM event clock: venue (offset-free), receive (published studies), auto=venue when extracted",
        )

    p_sub = sub.add_parser("subsecond", help="net edge vs reaction latency (sub-second)")
    common(p_sub)
    pm_clock_flag(p_sub)
    p_sub.set_defaults(func=cmd_subsecond)

    p_ext = sub.add_parser("extract-sizes", help="re-extract tob with level size columns")
    common(p_ext)
    p_ext.add_argument("--concurrency", type=int, default=4)
    p_ext.set_defaults(func=cmd_extract_sizes)

    p_fill = sub.add_parser("fillability", help="displayed size at the ask at signal time")
    common(p_fill)
    pm_clock_flag(p_fill)
    p_fill.add_argument(
        "--leader",
        choices=("hl", "trades"),
        default="hl",
        help="event clock: 'hl' = HL book-mid snapshots (#626 baseline), 'trades' = "
        "tick-trades leader parquets from leadlag_trades_leader.py (#631/#633)",
    )
    p_fill.set_defaults(func=cmd_fillability)

    args = parser.parse_args()
    args.func(args)


if __name__ == "__main__":
    main()
