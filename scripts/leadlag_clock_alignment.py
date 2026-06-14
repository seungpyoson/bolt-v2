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
this is the RA raw receive-offset fallback, and it sunsets when #677 makes the
converter write `ts_init = capture_time` into the NT catalog.

1. `lake` — per date, the distribution of (timestamp_received - timestamp) for the
   event types the study consumed. Reads only three columns via duckdb httpfs column
   projection; no bulk download. This cross-times EVERY event in the window (the
   stronger variant of the issue's "cross-time an event visible in both streams").

2. `live-probe` — the issue's "capture both feeds live side-by-side" arm, repurposed
   as a venue-clock honesty check: subscribes to the Polymarket CLOB market channel
   (rotating updown cycle tokens), Bybit spot publicTrade, and Hyperliquid l2Book,
   and measures (NTP-corrected local receive time - venue timestamp) per feed. A
   non-negative tight floor bounds each venue clock's skew from true UTC; the lake
   offset then decomposes into transit delay vs clock error. The local clock is
   guarded: per-sample wall-vs-monotonic step detection plus periodic NTP re-anchors.
   A start-only NTP correction is NOT trusted across the run — a mid-run ~2s macOS
   wall-clock step contaminated the first 60-minute capture.

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
STEP_DETECT_MS = 50.0  # wall-vs-monotonic jump beyond this = local clock step
NTP_REANCHOR_SECS = 300  # periodic NTP re-anchor cadence (also liveness output)
NTP_REANCHOR_SAMPLES = 3


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

    for date in dates:
        if not (ckdir / f"{date}.json").exists():
            raise SystemExit(f"lake: missing checkpoint for {date}; re-run lake")

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


def ntp_offset_ms(n_samples: int = NTP_SAMPLES) -> tuple[float, float]:
    """Median (offset, round-trip delay) of n_samples queries, in ms.
    offset = ntp_time - local_time; corrected local = local + offset."""
    import ntplib

    client = ntplib.NTPClient()
    samples = []
    for _ in range(n_samples):
        resp = client.request(NTP_SERVER, version=3, timeout=5)
        samples.append((resp.offset * 1000.0, resp.delay * 1000.0))
        time.sleep(0.5)
    samples.sort()
    mid = samples[len(samples) // 2]
    return mid[0], mid[1]


class CorrectedClock:
    """NTP-anchored wall clock guarded against local clock movement mid-run.

    A start-only NTP correction silently breaks if the OS steps the wall clock
    during the capture (this contaminated the first 60-minute probe: a ~2s macOS
    step shifted all three venues' offsets identically). Guards: every sample
    checks the wall-vs-monotonic delta — a jump beyond STEP_DETECT_MS means the
    wall clock moved, so the anchor is compensated and that sample dropped —
    and periodic NTP re-anchors bound crystal drift; each re-anchor's residual
    is the run's live error bar.
    """

    def __init__(self, initial_offset_ms: float) -> None:
        self.offset_ms = initial_offset_ms  # corrected = wall + offset
        self.delta_anchor = time.time_ns() / 1e6 - time.monotonic_ns() / 1e6
        self.steps: list[tuple[str, float]] = []  # (utc hh:mm:ss, jump_ms)
        self.residuals: list[float] = []

    def now(self) -> float | None:
        wall = time.time_ns() / 1e6
        delta = wall - time.monotonic_ns() / 1e6
        jump = delta - self.delta_anchor
        if abs(jump) > STEP_DETECT_MS:
            self.offset_ms -= jump
            self.delta_anchor = delta
            stamp = time.strftime("%H:%M:%SZ", time.gmtime())
            self.steps.append((stamp, jump))
            print(
                f"live-probe: LOCAL CLOCK STEP {jump:+.0f}ms at {stamp}; compensated, sample dropped",
                flush=True,
            )
            return None
        return wall + self.offset_ms

    def reanchor(self, measured_offset_ms: float) -> float:
        residual = measured_offset_ms - self.offset_ms
        self.residuals.append(residual)
        self.offset_ms = measured_offset_ms
        self.delta_anchor = time.time_ns() / 1e6 - time.monotonic_ns() / 1e6
        return residual


async def ntp_reanchor(clock: CorrectedClock, deadline: float) -> None:
    while time.time() < deadline:
        await asyncio.sleep(min(NTP_REANCHOR_SECS, max(deadline - time.time(), 0.0)))
        if time.time() >= deadline:
            return
        try:
            measured, delay = await asyncio.to_thread(ntp_offset_ms, NTP_REANCHOR_SAMPLES)
        except Exception as exc:  # keep guard-projected clock on NTP failure
            print(
                f"live-probe: NTP re-anchor failed ({type(exc).__name__}: {exc}); keeping projected clock",
                flush=True,
            )
            continue
        residual = clock.reanchor(measured)
        print(
            f"live-probe: NTP re-anchor offset {measured:+.1f}ms (rtt {delay:.1f}ms), residual {residual:+.1f}ms",
            flush=True,
        )


async def keepalive(ws, payload: str | None, secs: int) -> None:
    while True:
        await asyncio.sleep(secs)
        await ws.send(payload if payload is not None else "PING")


async def probe_polymarket(asset: str, deadline: float, offsets: dict[str, list[float]], clock: CorrectedClock) -> None:
    import websockets

    while time.time() < deadline:
        cycle_start = int(time.time()) // s4.CADENCE_SECS * s4.CADENCE_SECS
        try:
            cycle = await asyncio.to_thread(s4.fetch_cycle, asset, cycle_start)
        except Exception as exc:
            # fetch_cycle raises after its internal retries; a sustained Gamma outage
            # must not silently kill PM collection while the other probes continue
            print(f"pm: cycle fetch failed ({type(exc).__name__}: {exc}); retrying in 10s", flush=True)
            await asyncio.sleep(10)
            continue
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
                        now = clock.now()
                        if now is None or raw == "PONG":
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


async def probe_bybit(asset: str, deadline: float, offsets: dict[str, list[float]], clock: CorrectedClock) -> None:
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
                        now = clock.now()
                        if now is None:
                            continue
                        msg = json.loads(raw)
                        for trade in msg.get("data", []) if msg.get("topic", "").startswith("publicTrade.") else []:
                            offsets["bybit_trades"].append(now - int(trade["T"]))
                finally:
                    ping.cancel()
        except Exception as exc:
            print(f"bybit: stream error ({type(exc).__name__}: {exc}); reconnecting", flush=True)
            await asyncio.sleep(2)


async def probe_hyperliquid(asset: str, deadline: float, offsets: dict[str, list[float]], clock: CorrectedClock) -> None:
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
                        now = clock.now()
                        if now is None:
                            continue
                        msg = json.loads(raw)
                        if msg.get("channel") == "l2Book":
                            offsets["hyperliquid_l2book"].append(now - int(msg["data"]["time"]))
                finally:
                    ping.cancel()
        except Exception as exc:
            print(f"hl: stream error ({type(exc).__name__}: {exc}); reconnecting", flush=True)
            await asyncio.sleep(2)


async def run_live_probe(asset: str, minutes: float, clock: CorrectedClock) -> dict[str, list[float]]:
    deadline = time.time() + minutes * 60
    offsets: dict[str, list[float]] = {"polymarket": [], "bybit_trades": [], "hyperliquid_l2book": []}
    tasks = {
        asyncio.create_task(probe_polymarket(asset, deadline, offsets, clock)): "polymarket",
        asyncio.create_task(probe_bybit(asset, deadline, offsets, clock)): "bybit_trades",
        asyncio.create_task(probe_hyperliquid(asset, deadline, offsets, clock)): "hyperliquid_l2book",
        asyncio.create_task(ntp_reanchor(clock, deadline)): "ntp_reanchor",
    }
    done, pending = await asyncio.wait(tasks, return_when=asyncio.FIRST_EXCEPTION)
    dead = {}
    for task in done:
        exc = task.exception()
        if isinstance(exc, BaseException):
            dead[tasks[task]] = exc
    if dead:
        for task in pending:
            task.cancel()
        await asyncio.gather(*pending, return_exceptions=True)
        # a task that escaped its reconnect loop (e.g. a pre-loop failure) must
        # not yield a successful zero-row report for that source
        details = "; ".join(f"{n}: {type(r).__name__}: {r}" for n, r in dead.items())
        raise SystemExit(f"live-probe: probe task(s) died: {details}")
    empty = [source for source, vals in offsets.items() if not vals]
    if empty:
        raise SystemExit(
            f"live-probe: captured zero samples for {', '.join(empty)}; "
            "check subscriptions/event filters or rerun with a longer probe"
        )
    return offsets


def cmd_live_probe(args: argparse.Namespace) -> None:
    clock_offset, ntp_delay = ntp_offset_ms()
    print(f"live-probe: local clock offset vs {NTP_SERVER}: {clock_offset:+.1f}ms (rtt {ntp_delay:.1f}ms)", flush=True)
    clock = CorrectedClock(clock_offset)
    started = time.strftime("%Y-%m-%dT%H:%M:%SZ", time.gmtime())
    offsets = asyncio.run(run_live_probe(args.asset, args.minutes, clock))
    steps_txt = "; ".join(f"{jump:+.0f}ms @ {stamp}" for stamp, jump in clock.steps) or "none"
    residual_txt = (
        f"{len(clock.residuals)} NTP re-anchors, max |residual| {max(abs(r) for r in clock.residuals):.1f}ms"
        if clock.residuals
        else "0 NTP re-anchors"
    )
    lines = [
        "## Live venue-clock probe — NTP-corrected local receive minus venue timestamp",
        "",
        f"Started {started}, duration {args.minutes:.0f}m, asset {args.asset}; "
        f"local clock corrected by {clock_offset:+.1f}ms (NTP rtt {ntp_delay:.1f}ms). "
        "Offsets in ms; a non-negative tight floor bounds venue clock skew from true UTC.",
        "",
        f"Local-clock guard: wall-clock steps detected+compensated: {steps_txt}; {residual_txt}.",
        "",
        STATS_HEADER,
    ]
    for source, vals in offsets.items():
        lines.append(offset_stats_row(source, vals))
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
    p_live.add_argument("--asset", default=LIVE_DEFAULT_ASSET, choices=sorted(s4.LEADER_COIN_BY_ASSET))
    p_live.add_argument("--report")
    p_live.set_defaults(func=cmd_live_probe)

    args = parser.parse_args()
    args.func(args)


if __name__ == "__main__":
    main()
