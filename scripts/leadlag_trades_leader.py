#!/usr/bin/env -S uv run --script
# /// script
# requires-python = ">=3.12"
# dependencies = [
#   "duckdb==1.4.1",
#   "polars==1.34.0",
#   "pyarrow==22.0.0",
#   "numpy==2.3.4",
#   "requests==2.32.5",
#   "lz4==4.4.4",
# ]
# ///
"""#631: trades-based leader robustness re-run for the #617/#626 lead-lag study.

The baseline study clocks spot moves off Hyperliquid book mids (~0.54s
snapshots) — the only book-level leader overlapping the Polymarket archive
window. If a faster venue moves first, that clock starts late: btc edge would
be understated and the eth/sol/xrp "repriced within 100ms" verdict could be an
artifact of a delayed starting gun. This re-runs the event study with Bybit
spot tick trades (millisecond timestamps) as the leader clock and reports:

  1. lead-time: per matched event, how much earlier the Bybit clock fires
     than the Hyperliquid clock (positive = Bybit leads).
  2. the latency-grid net-edge table (same definitions as #626 §1) under the
     Bybit clock, side by side comparable with the HL-clock baseline.

Subcommands (same cache workdir as the session-4 scripts):
  extract-leader  Bybit spot tick_trades csv.gz -> leader_trades/{date}/{COIN}.parquet
  analyze         lead-time diagnostic + latency-grid edge table (markdown)

Reproduction:
  uv run scripts/leadlag_trades_leader.py extract-leader --dates 2026-04-22:2026-04-28
  uv run scripts/leadlag_trades_leader.py analyze --dates 2026-04-22:2026-04-28 \
      --report /tmp/leadlag-s4/trades_leader.md
"""

from __future__ import annotations

import argparse
import math
import subprocess
import sys
import tempfile
from pathlib import Path

import duckdb
import polars as pl

sys.path.insert(0, str(Path(__file__).resolve().parent))
import leadlag_session4 as s4  # noqa: E402

BYBIT_TRADES_PREFIX = (
    "s3://bolt-parquet/backfill-staging/2026-06-01/bybit/raw/v1/"
    "source=public_archive/family=tick_trades/category=spot"
)
BYBIT_SYMBOL_BY_COIN = {"BTC": "BTCUSDT", "ETH": "ETHUSDT", "SOL": "SOLUSDT", "XRP": "XRPUSDT"}
DELTAS_SECS = (0.1, 0.25, 0.5, 0.75, 1.0, 2.0, 5.0, 10.0)
MARK_HORIZON_SECS = 30  # the session-4 best cell horizon, as in #626
THRESHOLDS_BPS = (5.0, 10.0)
LEADTIME_MATCH_WINDOW_SECS = 15  # HL event within +/- this of a Bybit event = same move


def trades_leader_path(workdir: Path, date: str, coin: str) -> Path:
    return workdir / "leader_trades" / date / f"{coin}.parquet"


# ----- extract-leader: Bybit spot tick trades -> last-price series ----------


def extract_trades_day(workdir: Path, date: str, coin: str) -> str:
    out = trades_leader_path(workdir, date, coin)
    if out.exists():
        return f"{date} {coin} cached"
    out.parent.mkdir(parents=True, exist_ok=True)
    symbol = BYBIT_SYMBOL_BY_COIN[coin]
    keys = s4.list_s3(f"{BYBIT_TRADES_PREFIX}/dt={date}/symbol={symbol}/")
    if not keys:
        return f"{date} {coin}: NO OBJECTS"
    frames = []
    # per-call connection: the module-level duckdb.sql() one is not thread-safe
    with duckdb.connect() as con, tempfile.TemporaryDirectory() as tmp:
        for i, key in enumerate(sorted(keys)):
            local = Path(tmp) / f"obj{i}.csv.gz"
            subprocess.run(
                ["aws", "s3", "cp", f"s3://bolt-parquet/{key}", str(local), "--only-show-errors"],
                check=True,
            )
            frames.append(
                con.sql(
                    "SELECT CAST(timestamp AS BIGINT) AS ts_ms, CAST(price AS DOUBLE) AS mid "
                    f"FROM read_csv_auto('{local}') ORDER BY 1"
                ).pl()
            )
    frame = pl.concat(frames).sort("ts_ms")
    if frame.is_empty():
        raise SystemExit(f"{date} {coin}: staged objects exist but contain no rows")
    day_lo = s4.day_epoch(date) * 1000
    bounds = frame.select(pl.col("ts_ms").min().alias("lo"), pl.col("ts_ms").max().alias("hi")).row(0)
    if not (day_lo <= bounds[0] and bounds[1] < day_lo + 86_400_000):
        raise SystemExit(f"{date} {coin}: timestamps outside day bounds (not ms?): {bounds}")
    raw = frame.height
    # LOCF lookups only need price *changes*; collapse runs of identical prints.
    frame = frame.filter(pl.col("mid").diff().fill_null(value=1.0) != 0.0)
    frame.write_parquet(out)
    return f"{date} {coin}: {frame.height} price changes from {raw} trades, {len(keys)} objects"


def cmd_extract_leader(args: argparse.Namespace) -> None:
    workdir = Path(args.workdir)
    dates = s4.parse_dates(args.dates)
    coins = sorted({s4.LEADER_COIN_BY_ASSET[a] for a in args.assets.split(",")})
    s4.run_pool(
        "extract-leader",
        args.concurrency,
        [lambda d=d, c=c: extract_trades_day(workdir, d, c) for d in dates for c in coins],
    )


# ----- analyze: lead-time diagnostic + latency-grid edge under Bybit clock ---


def load_leader(path: Path) -> s4.LeaderSeries | None:
    return s4.LeaderSeries(pl.read_parquet(path)) if path.exists() else None


def cmd_analyze(args: argparse.Namespace) -> None:
    workdir = Path(args.workdir)
    dates = s4.parse_dates(args.dates)
    assets = args.assets.split(",")
    cycles = s4.load_cycles(workdir, dates, assets)
    cycles_by_key = {(c.asset, c.start): c for c in cycles}
    all_tokens = {c.up_token for c in cycles} | {c.down_token for c in cycles}
    print(f"analyze: {len(cycles)} cycles; loading extracts ...", flush=True)
    books = s4.load_token_books(workdir, dates, all_tokens)

    offsets: dict[tuple[str, float], list[float]] = {}
    counts: dict[tuple[str, float], list[int]] = {}  # [bybit_events, hl_events, matched]
    net: dict[tuple[str, float, float], list[float]] = {}
    pre_move_net: dict[tuple[str, float], list[float]] = {}
    for asset in assets:
        coin = s4.LEADER_COIN_BY_ASSET[asset]
        for date in dates:
            trades = load_leader(trades_leader_path(workdir, date, coin))
            hl = load_leader(s4.leader_path(workdir, date, coin))
            if trades is None or hl is None:
                continue
            base = s4.day_epoch(date)
            for x_bps in THRESHOLDS_BPS:
                bybit_events = s4.detect_events(trades, base, x_bps)
                hl_events = s4.detect_events(hl, base, x_bps)
                cnt = counts.setdefault((asset, x_bps), [0, 0, 0])
                cnt[0] += len(bybit_events)
                cnt[1] += len(hl_events)
                hl_by_dir: dict[int, list[int]] = {1: [], -1: []}
                for t, direction in hl_events:
                    hl_by_dir[direction].append(t)
                for t, direction in bybit_events:
                    if hl_by_dir[direction]:
                        nearest = min((abs(th - t), th) for th in hl_by_dir[direction])
                        if nearest[0] <= LEADTIME_MATCH_WINDOW_SECS:
                            cnt[2] += 1
                            offsets.setdefault((asset, x_bps), []).append(nearest[1] - t)

                    cycle = cycles_by_key.get((asset, (t // s4.CADENCE_SECS) * s4.CADENCE_SECS))
                    if cycle is None or t - cycle.start < s4.MIN_SECS_AFTER_OPEN:
                        continue
                    if cycle.end - t < MARK_HORIZON_SECS + 2:
                        continue
                    token = cycle.up_token if direction > 0 else cycle.down_token
                    book = books.get(token)
                    if book is None:
                        continue
                    pre = book.asof(t - 1)
                    if pre is None:
                        continue
                    if args.mark == "settlement":
                        # mark to the venue's own resolution: bought token pays 1 or 0
                        if cycle.outcome_up is None:
                            continue
                        mark_mid = 1.0 if (cycle.outcome_up == 1) == (direction > 0) else 0.0
                    else:
                        mark = book.asof(t + MARK_HORIZON_SECS)
                        if mark is None:
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

    lead_rows = []
    for asset in assets:
        for x_bps in THRESHOLDS_BPS:
            cnt = counts.get((asset, x_bps))
            if cnt is None:
                continue
            offs = offsets.get((asset, x_bps), [])
            if offs:
                p25, p50, p75 = s4.quantiles(offs)
                ahead = sum(1 for o in offs if o > 0) / len(offs)
                stats = [f"{p50:+.0f}s", f"[{p25:+.0f}, {p75:+.0f}]s", f"{ahead:.0%}"]
            else:
                stats = ["-", "-", "-"]
            lead_rows.append(
                [asset, f"{x_bps:.0f}", f"{cnt[0]:,}", f"{cnt[1]:,}",
                 f"{cnt[2] / cnt[0]:.0%}" if cnt[0] else "-", *stats]
            )
    lead_table = s4.md_table(
        ["asset", "X (bps)", "bybit events", "hl events", "matched",
         "median HL-minus-Bybit", "IQR", "% Bybit first"],
        lead_rows,
    )

    edge_rows = []
    for asset in assets:
        for x_bps in THRESHOLDS_BPS:
            pre = pre_move_net.get((asset, x_bps), [])
            if pre:
                mean, lo, hi = s4.mean_ci(pre)
                edge_rows.append([asset, f"{x_bps:.0f}", "pre-move (t-1s)", f"{len(pre):,}",
                                  f"{mean:+.2f}",
                                  f"[{lo:+.2f}, {hi:+.2f}]" if not math.isnan(lo) else "-"])
            for delta in DELTAS_SECS:
                vals = net.get((asset, x_bps, delta), [])
                if not vals:
                    continue
                mean, lo, hi = s4.mean_ci(vals)
                edge_rows.append([asset, f"{x_bps:.0f}", f"+{delta:g}s", f"{len(vals):,}",
                                  f"{mean:+.2f}",
                                  f"[{lo:+.2f}, {hi:+.2f}]" if not math.isnan(lo) else "-"])
    mark_label = "settlement" if args.mark == "settlement" else f"{MARK_HORIZON_SECS}s"
    edge_table = s4.md_table(
        ["asset", "X (bps)", "entry at detection", f"n (mark {mark_label})",
         "mean net (c)", "95% CI"],
        edge_rows,
    )

    out = (
        f"<!-- section:leadtime (Bybit trades clock vs HL book clock) -->\n{lead_table}\n\n"
        f"<!-- section:tradesleader (net edge, Bybit trades clock) -->\n{edge_table}\n"
    )
    if args.report:
        Path(args.report).write_text(out)
        print(f"analyze: wrote {args.report}")
    else:
        print(out)


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    sub = parser.add_subparsers(dest="cmd", required=True)

    def common(p: argparse.ArgumentParser) -> None:
        p.add_argument("--dates", required=True)
        p.add_argument("--assets", default=s4.DEFAULT_ASSETS)
        p.add_argument("--workdir", default=str(s4.DEFAULT_WORKDIR))

    p = sub.add_parser("extract-leader", help="Bybit spot tick trades -> leader parquets")
    common(p)
    p.add_argument("--concurrency", type=int, default=8)
    p.set_defaults(func=cmd_extract_leader)

    p = sub.add_parser("analyze", help="lead-time diagnostic + latency-grid edge table")
    common(p)
    p.add_argument("--report", default=None)
    p.add_argument(
        "--mark",
        choices=("mid", "settlement"),
        default="mid",
        help="mark events to book mid at +30s (default) or to the cycle's settlement payout; "
        "event-set guards are identical so the two tables are row-comparable",
    )
    p.set_defaults(func=cmd_analyze)

    args = parser.parse_args()
    args.func(args)


if __name__ == "__main__":
    main()
