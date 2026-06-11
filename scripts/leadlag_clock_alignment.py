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
"""Lead-lag follow-up (issue #633 item 3): cross-source clock alignment.

The published latency grids (docs/research/leadlag-subsecond-fillability-2026-06-10.md,
docs/research/leadlag-trades-leader-2026-06-11.md) compare leader-venue EXCHANGE
timestamps against Polymarket COLLECTOR RECEIVE timestamps (pmxt `timestamp_received`).
A systematic offset between those clocks shifts the whole sub-second grid. The raw pmxt
objects also carry Polymarket's own venue event timestamp (`timestamp`), which the
extracts never used — so the offset is measurable directly over the study window:

1. `lake` — per date, the distribution of (timestamp_received - timestamp) for the
   event types the study consumed. Reads only three columns via duckdb httpfs column
   projection; no bulk download. This cross-times EVERY event in the window (the
   stronger variant of the issue's "cross-time an event visible in both streams").

2. `live-probe` — the issue's "capture both feeds live side-by-side" arm, repurposed
   as a venue-clock honesty check: subscribes to the Polymarket CLOB market channel
   (rotating updown cycle tokens), Bybit spot publicTrade, and Hyperliquid l2Book,
   and measures (NTP-corrected local receive time - venue timestamp) per feed. A
   non-negative tight floor bounds each venue clock's skew from true UTC; the lake
   offset then decomposes into transit delay vs clock error.

Reproduction:
  uv run scripts/leadlag_clock_alignment.py lake --dates 2026-04-22:2026-04-28 --report <file>
  uv run scripts/leadlag_clock_alignment.py live-probe --minutes 60 --report <file>
"""

from __future__ import annotations

import argparse
import asyncio
import json
import sys
import time
from pathlib import Path

import duckdb

sys.path.insert(0, str(Path(__file__).resolve().parent))
import leadlag_session4 as s4  # noqa: E402
import leadlag_trades_leader as tl  # noqa: E402

OFFSET_EVENT_TYPES = ("price_change", "last_trade_price")
OFFSET_PERCENTILES = (0.05, 0.25, 0.50, 0.75, 0.95, 0.99)
LIVE_DEFAULT_MINUTES = 60
LIVE_DEFAULT_ASSET = "btc"
PM_WS_URL = "wss://ws-subscriptions-clob.polymarket.com/ws/market"
BYBIT_WS_URL = "wss://stream.bybit.com/v5/public/spot"
HL_WS_URL = "wss://api.hyperliquid.xyz/ws"
NTP_SERVER = "pool.ntp.org"
NTP_SAMPLES = 5
PM_PING_SECS = 10
BYBIT_PING_SECS = 20
HL_PING_SECS = 50
CYCLE_GRACE_SECS = 5  # keep listening past cycle end; book events trail settlement


def quantile(sorted_vals: list[float], q: float) -> float:
    if not sorted_vals:
        raise ValueError("quantile of empty list")
    idx = q * (len(sorted_vals) - 1)
    lo = int(idx)
    hi = min(lo + 1, len(sorted_vals) - 1)
    return sorted_vals[lo] + (sorted_vals[hi] - sorted_vals[lo]) * (idx - lo)


def offset_stats_row(label: str, offsets_ms: list[float]) -> str:
    vals = sorted(offsets_ms)
    cells = [label, str(len(vals)), f"{vals[0]:.0f}"]
    cells += [f"{quantile(vals, q):.0f}" for q in OFFSET_PERCENTILES]
    cells.append(f"{vals[-1]:.0f}")
    return "| " + " | ".join(cells) + " |"


STATS_HEADER = (
    "| source | n | min | p5 | p25 | p50 | p75 | p95 | p99 | max |\n"
    "|---|---|---|---|---|---|---|---|---|---|"
)


# ----- lake: receive-vs-venue offset over the study window -------------------


def lake_connect() -> duckdb.DuckDBPyConnection:
    con = duckdb.connect()
    con.execute("INSTALL httpfs; LOAD httpfs; INSTALL aws; LOAD aws;")
    con.execute("CREATE SECRET (TYPE s3, PROVIDER credential_chain);")
    con.execute("SET enable_progress_bar = false;")
    return con


def lake_scan_date(con: duckdb.DuckDBPyConnection, date: str) -> dict:
    glob = f"{s4.PMXT_S3_PREFIX}/dt={date}/*.parquet"
    event_list = ", ".join(f"'{e}'" for e in OFFSET_EVENT_TYPES)
    q_list = ", ".join(str(q) for q in OFFSET_PERCENTILES)
    rows = con.execute(
        f"""
        WITH src AS (
            SELECT event_type,
                   epoch_ms(timestamp_received) - epoch_ms(timestamp) AS off_ms
            FROM read_parquet('{glob}')
            WHERE event_type IN ({event_list})
        )
        SELECT event_type, COUNT(off_ms) AS n,
               COUNT(*) - COUNT(off_ms) AS n_null,
               MIN(off_ms), quantile_cont(off_ms, [{q_list}]), MAX(off_ms)
        FROM src GROUP BY event_type ORDER BY n DESC
        """
    ).fetchall()
    hourly = con.execute(
        f"""
        SELECT hour(timestamp_received) AS hr,
               median(epoch_ms(timestamp_received) - epoch_ms(timestamp)) AS med
        FROM read_parquet('{glob}')
        WHERE event_type = 'price_change' AND timestamp IS NOT NULL
        GROUP BY hr ORDER BY hr
        """
    ).fetchall()
    meds = [float(m) for _, m in hourly]
    return {
        "rows": [
            [event_type, int(n), int(n_null), float(mn), [float(v) for v in qs], float(mx)]
            for event_type, n, n_null, mn, qs, mx in rows
        ],
        "hourly_med_min": min(meds) if meds else None,
        "hourly_med_max": max(meds) if meds else None,
    }


def cmd_lake(args: argparse.Namespace) -> None:
    dates = s4.parse_dates(args.dates)
    ckdir = Path(args.workdir) / "clock_offsets"
    ckdir.mkdir(parents=True, exist_ok=True)
    con: duckdb.DuckDBPyConnection | None = None
    # One date per checkpoint: a whole-window scan is hours of S3 reads and gets killed
    # by command timeouts; per-date JSON makes any rerun resume where it died.
    for date in dates:
        ck = ckdir / f"{date}.json"
        if ck.exists():
            print(f"lake: {date} checkpoint exists, skipping", flush=True)
            continue
        if con is None:
            con = lake_connect()
        print(f"lake: scanning {date} ...", flush=True)
        payload = lake_scan_date(con, date)
        tmp = ck.with_suffix(".json.tmp")
        tmp.write_text(json.dumps(payload))
        tmp.rename(ck)

    lines = [
        "## Cross-source clock offset — pmxt collector receive vs Polymarket venue timestamp",
        "",
        "Offsets in ms: `epoch_ms(timestamp_received) - epoch_ms(timestamp)` per event row.",
        "",
        "| date | event_type | n | null_venue_ts | min | "
        + " | ".join(f"p{int(q * 100)}" for q in OFFSET_PERCENTILES)
        + " | max | hourly p50 range |",
        "|---|---|---|---|---|" + "---|" * len(OFFSET_PERCENTILES) + "---|---|",
    ]
    for date in dates:
        payload = json.loads((ckdir / f"{date}.json").read_text())
        if payload["hourly_med_min"] is not None:
            hourly_range = f"{payload['hourly_med_min']:.0f}..{payload['hourly_med_max']:.0f}"
        else:
            hourly_range = "n/a"
        for event_type, n, n_null, mn, qs, mx in payload["rows"]:
            qcells = " | ".join(f"{v:.0f}" for v in qs)
            rng = hourly_range if event_type == "price_change" else ""
            lines.append(
                f"| {date} | {event_type} | {n} | {n_null} | {mn:.0f} | {qcells} | {mx:.0f} | {rng} |"
            )
    emit_report(lines, args.report)


# ----- live-probe: venue clock honesty vs NTP-corrected local clock ----------


def ntp_offset_ms() -> tuple[float, float]:
    """Median (offset, round-trip delay) of NTP_SAMPLES queries, in ms.
    offset = ntp_time - local_time; corrected local = local + offset."""
    import ntplib

    client = ntplib.NTPClient()
    samples = []
    for _ in range(NTP_SAMPLES):
        resp = client.request(NTP_SERVER, version=3, timeout=5)
        samples.append((resp.offset * 1000.0, resp.delay * 1000.0))
        time.sleep(0.5)
    samples.sort()
    mid = samples[len(samples) // 2]
    return mid[0], mid[1]


def recv_ms(clock_offset_ms: float) -> float:
    return time.time_ns() / 1e6 + clock_offset_ms


async def keepalive(ws, payload: str | None, secs: int) -> None:
    while True:
        await asyncio.sleep(secs)
        await ws.send(payload if payload is not None else "PING")


async def probe_polymarket(asset: str, deadline: float, offsets: dict[str, list[float]], clock_offset_ms: float) -> None:
    import websockets

    while time.time() < deadline:
        cycle_start = int(time.time()) // s4.CADENCE_SECS * s4.CADENCE_SECS
        cycle = await asyncio.to_thread(s4.fetch_cycle, asset, cycle_start)
        if cycle.get("missing"):
            print(f"pm: cycle {cycle_start} missing on Gamma; retrying in 10s", flush=True)
            await asyncio.sleep(10)
            continue
        tokens = [cycle["up_token"], cycle["down_token"]]
        cycle_end = min(cycle["end"] + CYCLE_GRACE_SECS, deadline)
        try:
            async with websockets.connect(PM_WS_URL, open_timeout=20) as ws:
                await ws.send(json.dumps({"type": "market", "assets_ids": tokens}))
                ping = asyncio.create_task(keepalive(ws, None, PM_PING_SECS))
                try:
                    while time.time() < cycle_end:
                        try:
                            raw = await asyncio.wait_for(ws.recv(), timeout=5.0)
                        except TimeoutError:
                            continue  # quiet stream; re-check cycle deadline
                        now = recv_ms(clock_offset_ms)
                        if raw == "PONG":
                            continue
                        msgs = json.loads(raw)
                        for msg in msgs if isinstance(msgs, list) else [msgs]:
                            ts = msg.get("timestamp")
                            if msg.get("event_type") in OFFSET_EVENT_TYPES and ts is not None:
                                offsets["polymarket"].append(now - int(ts))
                finally:
                    ping.cancel()
        except Exception as exc:  # reconnect on any stream error
            print(f"pm: stream error ({type(exc).__name__}: {exc}); reconnecting", flush=True)
            await asyncio.sleep(2)


async def probe_bybit(asset: str, deadline: float, offsets: dict[str, list[float]], clock_offset_ms: float) -> None:
    import websockets

    symbol = tl.BYBIT_SYMBOL_BY_COIN[s4.LEADER_COIN_BY_ASSET[asset]]
    while time.time() < deadline:
        try:
            async with websockets.connect(BYBIT_WS_URL, open_timeout=20) as ws:
                await ws.send(json.dumps({"op": "subscribe", "args": [f"publicTrade.{symbol}"]}))
                ping = asyncio.create_task(keepalive(ws, json.dumps({"op": "ping"}), BYBIT_PING_SECS))
                try:
                    while time.time() < deadline:
                        try:
                            raw = await asyncio.wait_for(ws.recv(), timeout=5.0)
                        except TimeoutError:
                            continue
                        now = recv_ms(clock_offset_ms)
                        msg = json.loads(raw)
                        for trade in msg.get("data", []) if msg.get("topic", "").startswith("publicTrade.") else []:
                            offsets["bybit_trades"].append(now - int(trade["T"]))
                finally:
                    ping.cancel()
        except Exception as exc:
            print(f"bybit: stream error ({type(exc).__name__}: {exc}); reconnecting", flush=True)
            await asyncio.sleep(2)


async def probe_hyperliquid(asset: str, deadline: float, offsets: dict[str, list[float]], clock_offset_ms: float) -> None:
    import websockets

    coin = s4.LEADER_COIN_BY_ASSET[asset]
    sub = {"method": "subscribe", "subscription": {"type": "l2Book", "coin": coin}}
    while time.time() < deadline:
        try:
            async with websockets.connect(HL_WS_URL, open_timeout=20) as ws:
                await ws.send(json.dumps(sub))
                ping = asyncio.create_task(keepalive(ws, json.dumps({"method": "ping"}), HL_PING_SECS))
                try:
                    while time.time() < deadline:
                        try:
                            raw = await asyncio.wait_for(ws.recv(), timeout=5.0)
                        except TimeoutError:
                            continue
                        now = recv_ms(clock_offset_ms)
                        msg = json.loads(raw)
                        if msg.get("channel") == "l2Book":
                            offsets["hyperliquid_l2book"].append(now - int(msg["data"]["time"]))
                finally:
                    ping.cancel()
        except Exception as exc:
            print(f"hl: stream error ({type(exc).__name__}: {exc}); reconnecting", flush=True)
            await asyncio.sleep(2)


async def run_live_probe(asset: str, minutes: float, clock_offset_ms: float) -> dict[str, list[float]]:
    deadline = time.time() + minutes * 60
    offsets: dict[str, list[float]] = {"polymarket": [], "bybit_trades": [], "hyperliquid_l2book": []}
    tasks = [
        asyncio.create_task(probe_polymarket(asset, deadline, offsets, clock_offset_ms)),
        asyncio.create_task(probe_bybit(asset, deadline, offsets, clock_offset_ms)),
        asyncio.create_task(probe_hyperliquid(asset, deadline, offsets, clock_offset_ms)),
    ]
    await asyncio.gather(*tasks, return_exceptions=True)
    return offsets


def cmd_live_probe(args: argparse.Namespace) -> None:
    clock_offset, ntp_delay = ntp_offset_ms()
    print(f"live-probe: local clock offset vs {NTP_SERVER}: {clock_offset:+.1f}ms (rtt {ntp_delay:.1f}ms)", flush=True)
    started = time.strftime("%Y-%m-%dT%H:%M:%SZ", time.gmtime())
    offsets = asyncio.run(run_live_probe(args.asset, args.minutes, clock_offset))
    lines = [
        "## Live venue-clock probe — NTP-corrected local receive minus venue timestamp",
        "",
        f"Started {started}, duration {args.minutes:.0f}m, asset {args.asset}; "
        f"local clock corrected by {clock_offset:+.1f}ms (NTP rtt {ntp_delay:.1f}ms). "
        "Offsets in ms; a non-negative tight floor bounds venue clock skew from true UTC.",
        "",
        STATS_HEADER,
    ]
    for source, vals in offsets.items():
        if vals:
            lines.append(offset_stats_row(source, vals))
        else:
            lines.append(f"| {source} | 0 | — captured nothing | | | | | | | |")
    emit_report(lines, args.report)


def emit_report(lines: list[str], report: str | None) -> None:
    text = "\n".join(lines) + "\n"
    if report:
        Path(report).parent.mkdir(parents=True, exist_ok=True)
        Path(report).write_text(text)
        print(f"wrote {report}", flush=True)
    print(text, flush=True)


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    sub = parser.add_subparsers(dest="cmd", required=True)

    p_lake = sub.add_parser("lake", help="offset distribution over the historical window")
    p_lake.add_argument("--dates", required=True)
    p_lake.add_argument("--workdir", default=str(s4.DEFAULT_WORKDIR))
    p_lake.add_argument("--report")
    p_lake.set_defaults(func=cmd_lake)

    p_live = sub.add_parser("live-probe", help="live venue-clock honesty probe")
    p_live.add_argument("--minutes", type=float, default=LIVE_DEFAULT_MINUTES)
    p_live.add_argument("--asset", default=LIVE_DEFAULT_ASSET)
    p_live.add_argument("--report")
    p_live.set_defaults(func=cmd_live_probe)

    args = parser.parse_args()
    args.func(args)


if __name__ == "__main__":
    main()
