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
"""Session 4 — lead-lag measurement for the Polymarket up/down taker premise.

Research tooling (NOT the runtime binary, NOT in CI). Produces the numbers for
the session-4 report: spread reality, fee reality, the spot->Polymarket
lead-lag event study, market-implied calibration, and the GO/NO-GO inputs.

Data sources (all read-only, no live-host access required):
  - Strategy-fidelity Polymarket CLOB events (top-of-book + trades): NT
    ParquetDataCatalog reads through the Rust leadlag_catalog_extract helper,
    with catalog URI, instrument aliases, book type, clock, and storage options
    supplied by TOML.
  - Raw fallback: receive-offset latency work only, until #677 writes
    `ts_init = capture_time` and the remaining latency reads can retire.
  - Spot leader mid: Hyperliquid perp l2Book 20-level snapshots (~0.54s cadence)
    from the bolt-parquet staging lake. OKX L2 books in the lake cover
    2026-03-01..03-11 only — zero overlap with Polymarket book coverage — so the
    leader mid is Hyperliquid's (substitution stated in the report).
  - Market identity + settlement: Polymarket Gamma API, slug
    "{asset}-updown-5m-{period_start}" (the runtime slug contract of
    src/bolt_v3_market_families/updown.rs::updown_market_slug).

Reproduction (from repo root; aws CLI must hold read access to bolt-parquet):
  uv run scripts/leadlag_session4.py resolve        --dates 2026-04-22:2026-04-28
  uv run scripts/leadlag_session4.py extract-pm-catalog --dates 2026-04-22:2026-04-28 \
      --catalog-config <leadlag-catalog.toml>
  uv run scripts/leadlag_session4.py extract-leader --dates 2026-04-22:2026-04-28
  uv run scripts/leadlag_session4.py analyze        --dates 2026-04-22:2026-04-28 \
      --report /tmp/leadlag_tables.md
"""

from __future__ import annotations

import argparse
import concurrent.futures as cf
import datetime as dt
import json
import math
import statistics
import subprocess
import tempfile
import threading
import time
import tomllib
from dataclasses import dataclass
from pathlib import Path

import duckdb
import lz4.frame
import numpy as np
import polars as pl
import requests

DEFAULT_WORKDIR = Path.home() / ".cache" / "bolt-leadlag-session4"
DEFAULT_ASSETS = "btc,eth,sol,xrp"
PMXT_S3_PREFIX = (
    "s3://bolt-parquet/backfill-staging/2026-06-01/polymarket-pmxt-v2-streaming/"
    "raw/v1/source_binding=polymarket-parquet-archive-index/fixture=prediction-market/"
    "family=order_book_snapshots_fixed_depth"
)
GAMMA_EVENTS_URL = "https://gamma-api.polymarket.com/events"
HL_L2BOOK_PREFIX = (
    "s3://bolt-parquet/backfill-staging/2026-06-01/"
    "hyperliquid-core-targeted-btc-eth-sol-xrp-doge-hype-bnb/raw/v1/source_family=l2Book"
)
CADENCE_SECS = 300
CYCLE_LOOKBACK_SECS = 900
CYCLE_LOOKAHEAD_SECS = 300
MOVE_THRESHOLDS_BPS = (5.0, 10.0, 20.0)
HORIZONS_SECS = (1, 2, 5, 10, 30, 60)
EVENT_MIN_SEPARATION_SECS = 120  # > max horizon: response windows never overlap
TTE_BUCKETS = ((240, 300), (180, 240), (120, 180), (60, 120), (0, 60))
MIN_SECS_AFTER_OPEN = 10
MAX_QUOTE_AGE_SECS = 10
CAL_PROBE_TTES_SECS = (120, 60, 30)
MAKER_MARK_HORIZONS_SECS = (10, 30, 60)
LEADER_COIN_BY_ASSET = {"btc": "BTC", "eth": "ETH", "sol": "SOL", "xrp": "XRP"}
ENTRY_FEE_BPS_SCALE = 10_000
PM_CLOCK_CHOICES = ("auto", "receive", "venue")
DEFAULT_PM_CLOCK = "auto"


def run_pool(label: str, workers: int, thunks: list) -> None:
    """Run thunks in a thread pool, printing each result; abort the whole pool
    on the first failure (a silent executor drain hides errors for minutes)."""
    with cf.ThreadPoolExecutor(max_workers=workers) as pool:
        futures = [pool.submit(t) for t in thunks]
        try:
            for fut in cf.as_completed(futures):
                print(f"{label}: {fut.result()}", flush=True)
        except BaseException:
            pool.shutdown(wait=False, cancel_futures=True)
            raise


def parse_dates(spec: str) -> list[str]:
    """'2026-04-22:2026-04-28' (inclusive range) or comma list of YYYY-MM-DD."""
    if ":" in spec:
        lo, hi = spec.split(":")
        a, b = dt.date.fromisoformat(lo), dt.date.fromisoformat(hi)
        if b < a:
            raise SystemExit(f"date range end before start: {spec}")
        return [(a + dt.timedelta(days=i)).isoformat() for i in range((b - a).days + 1)]
    return [dt.date.fromisoformat(d).isoformat() for d in spec.split(",")]


def day_epoch(date: str) -> int:
    return int(dt.datetime.fromisoformat(date + "T00:00:00+00:00").timestamp())


def taker_fee_dollars(rate: float, price: float) -> float:
    """Production fee formula (NT polymarket compute_commission, taker side):
    fee = rate * p * (1 - p) dollars per share of $1 payout."""
    return rate * price * (1.0 - price)


@dataclass
class Cycle:
    asset: str
    start: int
    end: int
    up_token: str
    down_token: str
    taker_fee_rate: float  # e.g. 0.10 for takerBaseFee=1000
    outcome_up: int | None  # 1 Up won, 0 Down won, None unresolved/missing


def gamma_path(workdir: Path, asset: str, date: str) -> Path:
    return workdir / "gamma" / f"{asset}_{date}.jsonl"


def tob_path(workdir: Path, date: str, stem: str) -> Path:
    return workdir / "pm_tob" / date / f"{stem}.parquet"


def trades_path(workdir: Path, date: str, stem: str) -> Path:
    return workdir / "pm_trades" / date / f"{stem}.parquet"


def leader_path(workdir: Path, date: str, coin: str) -> Path:
    return workdir / "leader" / date / f"{coin}.parquet"


# ----- resolve: Gamma market identity + settlement per 5-minute cycle -------


_thread_local = threading.local()


def _gamma_session() -> requests.Session:
    # requests.Session is not documented thread-safe; one session per pool thread
    # keeps connection pooling without sharing mutable state across threads.
    if not hasattr(_thread_local, "session"):
        _thread_local.session = requests.Session()
    return _thread_local.session


def fetch_cycle(asset: str, start: int) -> dict:
    slug = f"{asset}-updown-5m-{start}"
    session = _gamma_session()
    for attempt in range(5):
        try:
            resp = session.get(GAMMA_EVENTS_URL, params={"slug": slug}, timeout=30)
            if resp.status_code == 429:
                if attempt == 4:
                    raise RuntimeError(f"Gamma rate limit persisted after 5 attempts for {slug}")
                time.sleep(2.0 * (attempt + 1))
                continue
            resp.raise_for_status()
            events = resp.json()
        except (requests.RequestException, ValueError):
            if attempt == 4:
                raise
            time.sleep(1.0 * (attempt + 1))
            continue
        if not events:
            return {"slug": slug, "asset": asset, "start": start, "missing": True}
        market = events[0]["markets"][0]
        tokens = json.loads(market["clobTokenIds"])
        outcomes = json.loads(market["outcomes"])
        prices = json.loads(market.get("outcomePrices") or "[]")
        up_idx, down_idx = outcomes.index("Up"), outcomes.index("Down")
        outcome_up: int | None = None
        if market.get("closed") and len(prices) == len(outcomes):
            up_price = float(prices[up_idx])
            if up_price in (0.0, 1.0):
                outcome_up = int(up_price)
        return {
            "slug": slug,
            "asset": asset,
            "start": start,
            "end": start + CADENCE_SECS,
            "condition_id": market["conditionId"],
            "up_token": tokens[up_idx],
            "down_token": tokens[down_idx],
            "taker_base_fee": int(market.get("takerBaseFee") or 0),
            "maker_base_fee": int(market.get("makerBaseFee") or 0),
            "fees_enabled": bool(market.get("feesEnabled")),
            "outcome_up": outcome_up,
            "missing": False,
        }
    raise RuntimeError(f"unreachable retry exit for {slug}")


def cmd_resolve(args: argparse.Namespace) -> None:
    workdir = Path(args.workdir)
    for date in parse_dates(args.dates):
        base = day_epoch(date)
        starts = [base + i * CADENCE_SECS for i in range(86_400 // CADENCE_SECS)]
        for asset in args.assets.split(","):
            out = gamma_path(workdir, asset, date)
            if out.exists():
                print(f"resolve: {out} exists, skipping")
                continue
            out.parent.mkdir(parents=True, exist_ok=True)
            with cf.ThreadPoolExecutor(max_workers=args.concurrency) as pool:
                rows = list(pool.map(lambda s: fetch_cycle(asset, s), starts))
            rows.sort(key=lambda r: r["start"])
            missing = sum(1 for r in rows if r["missing"])
            tmp = out.with_suffix(".tmp")
            tmp.write_text("".join(json.dumps(r) + "\n" for r in rows))
            tmp.rename(out)
            print(f"resolve: {asset} {date}: {len(rows) - missing}/{len(rows)} cycles, {missing} missing")


def load_cycles(workdir: Path, dates: list[str], assets: list[str]) -> list[Cycle]:
    cycles: list[Cycle] = []
    for date in dates:
        for asset in assets:
            path = gamma_path(workdir, asset, date)
            if not path.exists():
                raise SystemExit(f"missing gamma cache {path}; run `resolve` first")
            for line in path.read_text().splitlines():
                row = json.loads(line)
                if row.get("missing"):
                    continue
                cycles.append(
                    Cycle(
                        asset=row["asset"],
                        start=row["start"],
                        end=row["end"],
                        up_token=row["up_token"],
                        down_token=row["down_token"],
                        taker_fee_rate=row["taker_base_fee"] / ENTRY_FEE_BPS_SCALE,
                        outcome_up=row["outcome_up"],
                    )
                )
    return cycles


# ----- extract-pm: hourly top-of-book + trades for the up/down tokens -------


def tokens_for_day(cycles: list[Cycle], day_start: int) -> list[str]:
    day_end = day_start + 86_400
    out: set[str] = set()
    for c in cycles:
        if c.start - CYCLE_LOOKBACK_SECS < day_end and c.end + CYCLE_LOOKAHEAD_SECS > day_start:
            out.add(c.up_token)
            out.add(c.down_token)
    return sorted(out)


# ----- extract-pm-catalog: NT catalog top-of-book + trades ------------------


TOB_CATALOG_SCHEMA = {
    "asset_id": pl.String,
    "instrument_id": pl.String,
    "ts_ms": pl.Int64,
    "ts_venue_ms": pl.Int64,
    "best_bid": pl.Float64,
    "best_ask": pl.Float64,
}
TRADES_CATALOG_SCHEMA = {
    "asset_id": pl.String,
    "instrument_id": pl.String,
    "ts_ms": pl.Int64,
    "ts_venue_ms": pl.Int64,
    "price": pl.Float64,
    "size": pl.Float64,
    "side": pl.String,
}


def load_pm_catalog_extract_config(path: str | Path) -> dict:
    config_path = Path(path)
    with config_path.open("rb") as handle:
        config = tomllib.load(handle)
    missing = [key for key in ("reader_bin", "cache_stem") if key not in config]
    if missing:
        raise SystemExit(f"catalog extract config missing required keys: {', '.join(missing)}")
    config["__config_path"] = str(config_path)
    return config


def run_leadlag_catalog_extract(config: dict, kind: str, output: Path) -> None:
    subprocess.run(
        [
            str(config["reader_bin"]),
            "--config",
            str(config["__config_path"]),
            "--kind",
            kind,
            "--output",
            str(output),
        ],
        check=True,
    )


def read_catalog_jsonl(path: Path, schema: dict[str, pl.DataType]) -> pl.DataFrame:
    if path.stat().st_size == 0:
        return pl.DataFrame(schema=schema)
    frame = pl.read_ndjson(path)
    return frame.select([pl.col(name).cast(dtype).alias(name) for name, dtype in schema.items()])


def catalog_fee_rate_by_token(cycles: list[Cycle]) -> dict[str, int]:
    rates: dict[str, int] = {}
    for cycle in cycles:
        fee_rate_bps = int(round(cycle.taker_fee_rate * ENTRY_FEE_BPS_SCALE))
        for token in (cycle.up_token, cycle.down_token):
            existing = rates.get(token)
            if existing is not None and existing != fee_rate_bps:
                raise SystemExit(f"conflicting fee rates for token {token}: {existing} vs {fee_rate_bps}")
            rates[token] = fee_rate_bps
    return rates


def write_catalog_extract_frames(
    workdir: Path,
    dates: list[str],
    cycles: list[Cycle],
    cache_stem: str,
    tob_json: Path,
    trades_json: Path,
) -> str:
    tob = read_catalog_jsonl(tob_json, TOB_CATALOG_SCHEMA)
    trades = read_catalog_jsonl(trades_json, TRADES_CATALOG_SCHEMA)
    fee_rates = catalog_fee_rate_by_token(cycles)
    trades = trades.with_columns(
        pl.col("asset_id").replace_strict(fee_rates, default=None).cast(pl.Int64).alias("fee_rate_bps")
    )

    summaries = []
    for date in dates:
        lo = day_epoch(date) * 1000
        hi = lo + 86_400_000
        tob_day = tob.filter((pl.col("ts_ms") >= lo) & (pl.col("ts_ms") < hi))
        trades_day = trades.filter((pl.col("ts_ms") >= lo) & (pl.col("ts_ms") < hi))
        tob_out = tob_path(workdir, date, cache_stem)
        trades_out = trades_path(workdir, date, cache_stem)
        tob_out.parent.mkdir(parents=True, exist_ok=True)
        trades_out.parent.mkdir(parents=True, exist_ok=True)
        tob_day.write_parquet(tob_out)
        trades_day.write_parquet(trades_out)
        summaries.append(f"{date}/{cache_stem}: tob={tob_day.height} trades={trades_day.height}")
    return "\n".join(summaries)


def cmd_extract_pm_catalog(args: argparse.Namespace) -> None:
    workdir = Path(args.workdir)
    dates = parse_dates(args.dates)
    cycles = load_cycles(workdir, dates, args.assets.split(","))
    config = load_pm_catalog_extract_config(args.catalog_config)
    with tempfile.TemporaryDirectory() as tmp:
        tmpdir = Path(tmp)
        tob_json = tmpdir / "tob.jsonl"
        trades_json = tmpdir / "trades.jsonl"
        run_leadlag_catalog_extract(config, "tob", tob_json)
        run_leadlag_catalog_extract(config, "trades", trades_json)
        print(
            write_catalog_extract_frames(
                workdir,
                dates,
                cycles,
                str(config["cache_stem"]),
                tob_json,
                trades_json,
            ),
            flush=True,
        )


# ----- extract-pm: legacy raw fallback for receive-offset work ---------------


def extract_pm_object(workdir: Path, date: str, key: str, tokens: list[str]) -> str:
    stem = Path(key).name.split("=")[-1].removesuffix(".parquet")[:16]
    tob_out, trades_out = tob_path(workdir, date, stem), trades_path(workdir, date, stem)
    if tob_out.exists() and trades_out.exists():
        return f"{date}/{stem} cached"
    tob_out.parent.mkdir(parents=True, exist_ok=True)
    trades_out.parent.mkdir(parents=True, exist_ok=True)
    token_list = ",".join(f"'{t}'" for t in tokens)
    with tempfile.TemporaryDirectory() as tmp:
        # Whole-object download then local scan: one sequential GET is far faster
        # than duckdb's in-query HTTP range reads on these ~400 MB objects.
        local = Path(tmp) / "obj.parquet"
        subprocess.run(
            ["aws", "s3", "cp", f"s3://bolt-parquet/{key}", str(local), "--only-show-errors"],
            check=True,
        )
        with duckdb.connect() as con:
            con.execute("SET threads=4;")
            raw_tob = con.execute(
                f"""
                SELECT asset_id, CAST(epoch_ms(timestamp_received) AS BIGINT) AS ts_ms,
                       CAST(epoch_ms(timestamp) AS BIGINT) AS ts_venue_ms,
                       CAST(best_bid AS DOUBLE) AS best_bid, CAST(best_ask AS DOUBLE) AS best_ask
                FROM read_parquet('{local}')
                WHERE event_type = 'price_change' AND asset_id IN ({token_list})
                ORDER BY asset_id, ts_ms
                """
            ).pl()
            raw_trades = con.execute(
                f"""
                SELECT asset_id, CAST(epoch_ms(timestamp_received) AS BIGINT) AS ts_ms,
                       CAST(epoch_ms(timestamp) AS BIGINT) AS ts_venue_ms,
                       CAST(price AS DOUBLE) AS price, CAST(size AS DOUBLE) AS size,
                       side, CAST(fee_rate_bps AS INTEGER) AS fee_rate_bps
                FROM read_parquet('{local}')
                WHERE event_type = 'last_trade_price' AND asset_id IN ({token_list})
                ORDER BY asset_id, ts_ms
                """
            ).pl()
    # Keep only top-of-book *changes* (price_change rows repeat the venue-provided
    # best bid/ask on every book delta; consecutive duplicates carry no information).
    if raw_tob.height:
        raw_tob = raw_tob.filter(
            (pl.col("best_bid") != pl.col("best_bid").shift(1).over("asset_id"))
            | (pl.col("best_ask") != pl.col("best_ask").shift(1).over("asset_id"))
            | pl.col("best_bid").shift(1).over("asset_id").is_null()
        )
    raw_tob.write_parquet(tob_out)
    raw_trades.write_parquet(trades_out)
    return f"{date}/{stem}: tob={raw_tob.height} trades={raw_trades.height}"


def cmd_extract_pm(args: argparse.Namespace) -> None:
    workdir = Path(args.workdir)
    dates = parse_dates(args.dates)
    cycles = load_cycles(workdir, dates, args.assets.split(","))
    jobs = []
    for date in dates:
        tokens = tokens_for_day(cycles, day_epoch(date))
        if not tokens:
            continue
        keys = list_s3(f"{PMXT_S3_PREFIX}/dt={date}/")
        if not keys:
            raise SystemExit(f"no pmxt staging objects for dt={date}")
        jobs += [(date, key, tokens) for key in keys]
    run_pool(
        "extract-pm",
        args.concurrency,
        [lambda d=d, k=k, t=t: extract_pm_object(workdir, d, k, t) for d, k, t in jobs],
    )


# ----- extract-leader: Hyperliquid l2Book mid series -------------------------


def list_s3(prefix: str) -> list[str]:
    proc = subprocess.run(
        ["aws", "s3", "ls", "--recursive", prefix], check=True, capture_output=True, text=True
    )
    return [parts[3] for line in proc.stdout.splitlines() if len(parts := line.split()) >= 4]


def extract_leader_day(workdir: Path, date: str, coin: str) -> str:
    out = leader_path(workdir, date, coin)
    if out.exists():
        return f"{date} {coin} cached"
    out.parent.mkdir(parents=True, exist_ok=True)
    keys = [k for k in list_s3(f"{HL_L2BOOK_PREFIX}/date={date.replace('-', '')}/") if f"/coin={coin}/" in k]
    if not keys:
        return f"{date} {coin}: NO OBJECTS"
    rows: list[tuple[int, float, float]] = []
    with tempfile.TemporaryDirectory() as tmp:
        for key in sorted(keys):
            local = Path(tmp) / "obj.lz4"
            subprocess.run(
                ["aws", "s3", "cp", f"s3://bolt-parquet/{key}", str(local), "--only-show-errors"],
                check=True,
            )
            for line in lz4.frame.decompress(local.read_bytes()).splitlines():
                data = json.loads(line)["raw"]["data"]
                levels = data["levels"]
                if levels[0] and levels[1]:
                    rows.append((data["time"], float(levels[0][0]["px"]), float(levels[1][0]["px"])))
    frame = (
        pl.DataFrame(rows, schema={"ts_ms": pl.Int64, "bid": pl.Float64, "ask": pl.Float64}, orient="row")
        .sort("ts_ms")
        .with_columns(((pl.col("bid") + pl.col("ask")) / 2.0).alias("mid"))
    )
    frame.write_parquet(out)
    return f"{date} {coin}: {frame.height} snapshots from {len(keys)} objects"


def cmd_extract_leader(args: argparse.Namespace) -> None:
    workdir = Path(args.workdir)
    dates = parse_dates(args.dates)
    coins = sorted({LEADER_COIN_BY_ASSET[a] for a in args.assets.split(",")})
    run_pool(
        "extract-leader",
        args.concurrency,
        [lambda d=d, c=c: extract_leader_day(workdir, d, c) for d in dates for c in coins],
    )


# ----- analyze: measurements 1-5 ---------------------------------------------


class TokenBook:
    """Sorted top-of-book series for one CLOB token with asof lookup."""

    def __init__(self, frame: pl.DataFrame) -> None:
        self.ts = frame["ts_ms"].to_numpy()
        self.bid = frame["best_bid"].to_numpy()
        self.ask = frame["best_ask"].to_numpy()

    def asof(self, t_secs: float, max_age: float = MAX_QUOTE_AGE_SECS) -> tuple[float, float] | None:
        t_ms = int(t_secs * 1000)
        idx = int(np.searchsorted(self.ts, t_ms, side="right")) - 1
        if idx < 0 or t_ms - int(self.ts[idx]) > max_age * 1000:
            return None
        bid, ask = float(self.bid[idx]), float(self.ask[idx])
        if not (0.0 < bid < 1.0 and 0.0 < ask < 1.0 and ask > bid):
            return None
        return bid, ask


class LeaderSeries:
    """Last-observation mid lookup for one leader coin-day (~0.54s snapshots)."""

    def __init__(self, frame: pl.DataFrame) -> None:
        self.ts = frame["ts_ms"].to_numpy()
        self.mid = frame["mid"].to_numpy()

    def mid_at(self, t_secs: float, max_age: float = 5.0) -> float | None:
        t_ms = int(t_secs * 1000)
        idx = int(np.searchsorted(self.ts, t_ms, side="right")) - 1
        if idx < 0 or t_ms - int(self.ts[idx]) > max_age * 1000:
            return None
        return float(self.mid[idx])


def mean_ci(values: list[float]) -> tuple[float, float, float]:
    n, mean = len(values), statistics.fmean(values)
    if n < 2:
        return mean, math.nan, math.nan
    half = 1.96 * statistics.stdev(values) / math.sqrt(n)
    return mean, mean - half, mean + half


def quantiles(values: list[float]) -> tuple[float, float, float]:
    s = sorted(values)
    n = len(s)
    return s[n // 4], s[n // 2], s[(3 * n) // 4]


def tte_bucket(tte: float) -> str | None:
    for lo, hi in TTE_BUCKETS:
        if lo <= tte < hi:
            return f"{lo}-{hi}s"
    return None


def md_table(headers: list[str], rows: list[list[str]]) -> str:
    out = ["| " + " | ".join(headers) + " |", "|" + "|".join("---" for _ in headers) + "|"]
    out += ["| " + " | ".join(r) + " |" for r in rows]
    return "\n".join(out)


PM_CLOCK_RESOLVED: dict[str, str] = {}  # load label -> "venue" | "receive", this process


def pm_clock_provenance() -> str:
    """The resolved PM clock for stamping into report artifacts. Fails loud if
    loads within this run disagreed — e.g. the per-date sized loader under
    `auto` mixing a re-extracted (venue) date with an old-cache (receive) date,
    a mix no concat schema check can catch because selection is per date."""
    if not PM_CLOCK_RESOLVED:
        raise SystemExit("pm_clock_provenance: no PM extracts loaded in this run")
    resolved = set(PM_CLOCK_RESOLVED.values())
    if len(resolved) > 1:
        raise SystemExit(
            f"mixed PM clocks within one run: {PM_CLOCK_RESOLVED}; "
            "re-extract the window to one cache generation or pin --pm-clock receive"
        )
    return resolved.pop()


def select_pm_clock(frame: pl.DataFrame, pm_clock: str, label: str) -> pl.DataFrame:
    """Substitute the PM event clock ONCE at load time (#633 item 3): downstream code
    always reads `ts_ms`. `venue` uses the Polymarket-stamped ts_venue_ms (offset-free
    vs leader exchange clocks); `receive` keeps the pmxt collector clock (the published
    studies' clock, ~120ms median behind venue); `auto` = venue when the extract carries
    it, else receive (old caches reproduce published numbers unchanged)."""
    if pm_clock not in PM_CLOCK_CHOICES:
        raise SystemExit(f"unknown pm-clock {pm_clock!r}; valid: {','.join(PM_CLOCK_CHOICES)}")
    has_venue = "ts_venue_ms" in frame.columns
    use_venue = pm_clock == "venue" or (pm_clock == "auto" and has_venue)
    if use_venue and not has_venue:
        raise SystemExit(f"{label}: pm-clock=venue but extracts lack ts_venue_ms; re-extract this window")
    if use_venue:
        frame = frame.drop("ts_ms").rename({"ts_venue_ms": "ts_ms"})
        n_null = frame["ts_ms"].null_count()
        if n_null:
            raise SystemExit(
                f"{label}: {n_null} null venue timestamps under pm-clock={pm_clock} (resolved venue); "
                "re-extract this window or pass --pm-clock receive explicitly"
            )
    elif has_venue:
        frame = frame.drop("ts_venue_ms")
    resolved = "venue" if use_venue else "receive"
    prev = PM_CLOCK_RESOLVED.get(label)
    if prev is not None and prev != resolved:
        # an overwrite would mask the mix from pm_clock_provenance(), which only
        # sees final dict values — detect the conflict at record time instead
        raise SystemExit(
            f"{label}: PM clock changed within one run ({prev} -> {resolved}); "
            "re-extract the window to one cache generation or pin --pm-clock receive"
        )
    PM_CLOCK_RESOLVED[label] = resolved
    print(f"{label}: PM event clock = {'venue (ts_venue_ms)' if use_venue else 'receive (ts_ms)'}", flush=True)
    return frame


def concat_pm_extract_frames(frames: list[pl.DataFrame], pm_clock: str, label: str) -> pl.DataFrame:
    has_venue = ["ts_venue_ms" in frame.columns for frame in frames]
    if any(has_venue) and not all(has_venue):
        if pm_clock == "receive":
            return pl.concat([frame.drop("ts_venue_ms") if has else frame for frame, has in zip(frames, has_venue)])
        raise SystemExit(
            f"{label}: mixed ts_venue_ms presence across extracts; "
            "re-extract the window to one cache generation or pass --pm-clock receive explicitly"
        )
    return pl.concat(frames)


def load_token_books(
    workdir: Path, dates: list[str], tokens: set[str], pm_clock: str = DEFAULT_PM_CLOCK
) -> dict[str, TokenBook]:
    frames = []
    for date in dates:
        directory = workdir / "pm_tob" / date
        for path in sorted(directory.glob("*.parquet")) if directory.exists() else []:
            frames.append(pl.read_parquet(path).filter(pl.col("asset_id").is_in(list(tokens))))
    if not frames:
        return {}
    merged = select_pm_clock(concat_pm_extract_frames(frames, pm_clock, "pm_tob"), pm_clock, "pm_tob").sort(
        "asset_id", "ts_ms"
    )
    return {key[0]: TokenBook(f) for key, f in merged.partition_by("asset_id", as_dict=True).items()}


def load_trades(
    workdir: Path, dates: list[str], tokens: set[str], pm_clock: str = DEFAULT_PM_CLOCK
) -> pl.DataFrame:
    frames = []
    for date in dates:
        directory = workdir / "pm_trades" / date
        for path in sorted(directory.glob("*.parquet")) if directory.exists() else []:
            frames.append(pl.read_parquet(path).filter(pl.col("asset_id").is_in(list(tokens))))
    if not frames:
        raise SystemExit("no pm_trades extracts found; run `extract-pm` first")
    return select_pm_clock(concat_pm_extract_frames(frames, pm_clock, "pm_trades"), pm_clock, "pm_trades").sort(
        "asset_id", "ts_ms"
    )


def detect_events(leader: LeaderSeries, day_start: int, threshold_bps: float) -> list[tuple[int, int]]:
    """(t_secs, direction) for 1-second leader mid moves >= threshold, de-overlapped."""
    events: list[tuple[int, int]] = []
    last_kept = -EVENT_MIN_SEPARATION_SECS
    prev_mid: float | None = None
    for t in range(day_start, day_start + 86_400):
        mid = leader.mid_at(t)
        if mid is None:
            prev_mid = None
            continue
        if prev_mid is not None:
            ret_bps = (mid / prev_mid - 1.0) * 1e4
            if abs(ret_bps) >= threshold_bps and t - last_kept >= EVENT_MIN_SEPARATION_SECS:
                events.append((t, 1 if ret_bps > 0 else -1))
                last_kept = t
        prev_mid = mid
    return events


def cmd_analyze(args: argparse.Namespace) -> None:
    workdir = Path(args.workdir)
    dates = parse_dates(args.dates)
    assets = args.assets.split(",")
    cycles = load_cycles(workdir, dates, assets)
    cycles_by_key = {(c.asset, c.start): c for c in cycles}
    all_tokens = {c.up_token for c in cycles} | {c.down_token for c in cycles}

    print(f"analyze: {len(cycles)} cycles, {len(all_tokens)} tokens; loading extracts ...", flush=True)
    books = load_token_books(workdir, dates, all_tokens, pm_clock=args.pm_clock)
    trades = load_trades(workdir, dates, all_tokens, pm_clock=args.pm_clock)
    leaders: dict[tuple[str, str], LeaderSeries] = {}
    for date in dates:
        for asset in assets:
            path = leader_path(workdir, date, LEADER_COIN_BY_ASSET[asset])
            if path.exists():
                leaders[(asset, date)] = LeaderSeries(pl.read_parquet(path))

    coverage_rows = []
    for date in dates:
        tob_dir, tr_dir = workdir / "pm_tob" / date, workdir / "pm_trades" / date
        tob_files = list(tob_dir.glob("*.parquet")) if tob_dir.exists() else []
        n_tob = sum(pl.read_parquet(p).height for p in tob_files)
        n_tr = sum(pl.read_parquet(p).height for p in tr_dir.glob("*.parquet")) if tr_dir.exists() else 0
        n_leader = sum(
            pl.read_parquet(leader_path(workdir, date, LEADER_COIN_BY_ASSET[a])).height
            for a in assets
            if leader_path(workdir, date, LEADER_COIN_BY_ASSET[a]).exists()
        )
        coverage_rows.append([date, str(len(tob_files)), f"{n_tob:,}", f"{n_tr:,}", f"{n_leader:,}"])

    # Section 1: spread reality
    spread_samples: dict[tuple[str, str], list[tuple[float, float]]] = {}
    two_sided: dict[tuple[str, str], list[int]] = {}
    for c in cycles:
        book = books.get(c.up_token)
        if book is None:
            continue
        for t in range(c.start, c.end, args.spread_sample_step):
            bucket = tte_bucket(c.end - t)
            if bucket is None:
                continue
            quote = book.asof(t)
            two_sided.setdefault((c.asset, bucket), []).append(0 if quote is None else 1)
            if quote is None:
                continue
            bid, ask = quote
            mid = (bid + ask) / 2.0
            spread_samples.setdefault((c.asset, bucket), []).append((ask - bid, (ask - bid) / mid * 100))

    spread_rows = []
    for asset in assets:
        for lo, hi in TTE_BUCKETS:
            bucket = f"{lo}-{hi}s"
            samples = spread_samples.get((asset, bucket), [])
            avail = two_sided.get((asset, bucket), [])
            if not samples:
                spread_rows.append([asset, bucket, "0", "-", "-", "-", "-", "0%"])
                continue
            cents = [s[0] * 100 for s in samples]
            p25c, p50c, p75c = quantiles(cents)
            _, p50p, _ = quantiles([s[1] for s in samples])
            spread_rows.append(
                [asset, bucket, f"{len(samples):,}", f"{p50c:.2f}", f"{p25c:.2f}", f"{p75c:.2f}",
                 f"{p50p:.1f}%", f"{100 * statistics.fmean(avail):.0f}%"]
            )

    # Section 2: fee reality
    token_asset = {c.up_token: c.asset for c in cycles} | {c.down_token: c.asset for c in cycles}
    fee_summary = (
        trades.with_columns(pl.col("asset_id").replace_strict(token_asset, default=None).alias("asset"))
        .drop_nulls("asset")
        .group_by("asset", "fee_rate_bps")
        .agg(pl.len().alias("n"))
        .sort("asset", "fee_rate_bps")
    )
    totals = {r["asset"]: r["n"] for r in fee_summary.group_by("asset").agg(pl.col("n").sum()).to_dicts()}
    fee_rows = [
        [r["asset"], str(r["fee_rate_bps"]), f"{r['n']:,}", f"{100 * r['n'] / totals[r['asset']]:.1f}%"]
        for r in fee_summary.to_dicts()
    ]
    gamma_fee_rates = ", ".join(f"{r:.2f}" for r in sorted({c.taker_fee_rate for c in cycles}))

    # Section 3: lead-lag event study.
    # Two entry definitions per event at t (the second the 1s leader move completed):
    #   pre-move    = ask at t-1, the brief's definition (requires sub-second reaction)
    #   executable  = ask at t, what a taker reacting after observing the move gets
    study: dict[tuple[str, float, int], list[float]] = {}
    study_exec: dict[tuple[str, float, int], list[float]] = {}
    study_resp: dict[tuple[str, float, int], list[float]] = {}
    study_tte: dict[tuple[str, float, int, str], list[float]] = {}
    event_counts: dict[tuple[str, float], int] = {}
    for asset in assets:
        for date in dates:
            leader = leaders.get((asset, date))
            if leader is None:
                continue
            base = day_epoch(date)
            for x_bps in MOVE_THRESHOLDS_BPS:
                for t, direction in detect_events(leader, base, x_bps):
                    cycle = cycles_by_key.get((asset, (t // CADENCE_SECS) * CADENCE_SECS))
                    if cycle is None or t - cycle.start < MIN_SECS_AFTER_OPEN:
                        continue
                    token = cycle.up_token if direction > 0 else cycle.down_token
                    book = books.get(token)
                    pre = book.asof(t - 1) if book else None
                    entry = book.asof(t) if book else None
                    if pre is None or entry is None:
                        continue
                    pre_bid, pre_ask = pre
                    pre_mid = (pre_bid + pre_ask) / 2.0
                    entry_ask = entry[1]
                    fee_pre = taker_fee_dollars(cycle.taker_fee_rate, pre_ask)
                    fee_entry = taker_fee_dollars(cycle.taker_fee_rate, entry_ask)
                    event_counts[(asset, x_bps)] = event_counts.get((asset, x_bps), 0) + 1
                    for h in HORIZONS_SECS:
                        if cycle.end - t < h + 2:
                            continue
                        post = book.asof(t + h)
                        if post is None:
                            continue
                        post_mid = (post[0] + post[1]) / 2.0
                        key = (asset, x_bps, h)
                        net_c = (post_mid - pre_ask - fee_pre) * 100
                        study.setdefault(key, []).append(net_c)
                        study_exec.setdefault(key, []).append((post_mid - entry_ask - fee_entry) * 100)
                        study_resp.setdefault(key, []).append((post_mid - pre_mid) * 100)
                        bucket = tte_bucket(cycle.end - t)
                        if bucket:
                            study_tte.setdefault((asset, x_bps, h, bucket), []).append(net_c)

    study_rows = []
    for asset in assets:
        for x_bps in MOVE_THRESHOLDS_BPS:
            for h in HORIZONS_SECS:
                net = study.get((asset, x_bps, h), [])
                if not net:
                    continue
                mean_net, lo, hi = mean_ci(net)
                mean_exec, lo_e, hi_e = mean_ci(study_exec[(asset, x_bps, h)])
                study_rows.append(
                    [asset, f"{x_bps:.0f}", f"{h}", f"{len(net):,}",
                     f"{statistics.fmean(study_resp[(asset, x_bps, h)]):+.2f}", f"{mean_net:+.2f}",
                     f"[{lo:+.2f}, {hi:+.2f}]" if not math.isnan(lo) else "-",
                     f"{mean_exec:+.2f}",
                     f"[{lo_e:+.2f}, {hi_e:+.2f}]" if not math.isnan(lo_e) else "-"]
                )

    # Section 4: calibration (market-implied proxy; model logs live on the host)
    cal_points: dict[tuple[str, int], list[tuple[float, int]]] = {}
    for c in cycles:
        book = books.get(c.up_token)
        if c.outcome_up is None or book is None:
            continue
        for probe in CAL_PROBE_TTES_SECS:
            quote = book.asof(c.end - probe)
            if quote is not None:
                cal_points.setdefault((c.asset, probe), []).append(((quote[0] + quote[1]) / 2.0, c.outcome_up))

    cal_rows = []
    for asset in assets:
        for probe in CAL_PROBE_TTES_SECS:
            pts = cal_points.get((asset, probe), [])
            if pts:
                brier = statistics.fmean((p - y) ** 2 for p, y in pts)
                cal_rows.append([asset, f"{probe}", f"{len(pts):,}", f"{brier:.4f}"])

    reliability: dict[int, list[tuple[float, int]]] = {}
    for pts in cal_points.values():
        for p, y in pts:
            reliability.setdefault(min(int(p * 10), 9), []).append((p, y))
    rel_rows = []
    for b in range(10):
        pts = reliability.get(b, [])
        rel_rows.append(
            [f"{b / 10:.1f}-{(b + 1) / 10:.1f}", f"{len(pts):,}",
             f"{statistics.fmean(p for p, _ in pts):.3f}" if pts else "-",
             f"{statistics.fmean(y for _, y in pts):.3f}" if pts else "-"]
        )

    # Section 5 input: maker counterfactual (passive fill mark-outs).
    # Deterministic every-Nth subsample keeps the python mark-out loop tractable;
    # the report states the sampled count next to the full fill count.
    maker_trades = trades
    if maker_trades.height > args.maker_sample_cap:
        maker_trades = maker_trades.gather_every(maker_trades.height // args.maker_sample_cap + 1)
    maker: dict[tuple[str, int], list[float]] = {}
    for row in maker_trades.to_dicts():
        asset = token_asset.get(row["asset_id"])
        book = books.get(row["asset_id"])
        if asset is None or book is None or row["side"] not in ("BUY", "SELL"):
            continue
        t = row["ts_ms"] / 1000.0
        for h in MAKER_MARK_HORIZONS_SECS:
            post = book.asof(t + h)
            if post is None:
                continue
            post_mid = (post[0] + post[1]) / 2.0
            # BUY aggressor -> maker sold at price; SELL aggressor -> maker bought.
            pnl = (row["price"] - post_mid) if row["side"] == "BUY" else (post_mid - row["price"])
            maker.setdefault((asset, h), []).append(pnl * 100)
    maker_rows = []
    for asset in assets:
        for h in MAKER_MARK_HORIZONS_SECS:
            vals = maker.get((asset, h), [])
            if not vals:
                continue
            mean_pnl, lo, hi = mean_ci(vals)
            _, p50, _ = quantiles(vals)
            maker_rows.append(
                [asset, f"{h}", f"{len(vals):,}", f"{mean_pnl:+.3f}",
                 f"[{lo:+.3f}, {hi:+.3f}]" if not math.isnan(lo) else "-", f"{p50:+.3f}"]
            )

    # Verdict: best horizon per asset
    verdict_rows = []
    best_cells: list[tuple[str, float, int]] = []
    for asset in assets:
        best: tuple[float, float, float, int, float, int] | None = None
        for x_bps in MOVE_THRESHOLDS_BPS:
            for h in HORIZONS_SECS:
                net = study.get((asset, x_bps, h), [])
                if len(net) < args.min_events:
                    continue
                mean_net, lo, hi = mean_ci(net)
                if best is None or mean_net > best[0]:
                    best = (mean_net, lo, hi, len(net), x_bps, h)
        if best is None:
            verdict_rows.append([asset, "-", "-", "-", "-", "-", "NO-GO (insufficient events)"])
            continue
        mean_net, lo, hi, n, x_bps, h = best
        mean_exec, lo_e, _ = mean_ci(study_exec[(asset, x_bps, h)])
        if lo > 0 and lo_e > 0:
            verdict = "GO"
        elif lo > 0:
            verdict = "NO-GO at 1s reaction (pre-move edge only)"
        else:
            verdict = "NO-GO"
        verdict_rows.append(
            [asset, f"X={x_bps:.0f}bps, h={h}s", f"{mean_net:+.2f}",
             f"[{lo:+.2f}, {hi:+.2f}]", f"{mean_exec:+.2f}", f"{n:,}", verdict]
        )
        best_cells.append((asset, x_bps, h))

    tte_lines = []
    for asset, x_bps, h in best_cells:
        rows = []
        for lo_b, hi_b in TTE_BUCKETS:
            bucket = f"{lo_b}-{hi_b}s"
            vals = study_tte.get((asset, x_bps, h, bucket), [])
            if not vals:
                rows.append([bucket, "0", "-", "-"])
                continue
            mean_net, lo, hi = mean_ci(vals)
            rows.append([bucket, f"{len(vals):,}", f"{mean_net:+.2f}",
                         f"[{lo:+.2f}, {hi:+.2f}]" if not math.isnan(lo) else "-"])
        tte_lines.append(
            f"\n**{asset} at X={x_bps:.0f}bps, h={h}s:**\n\n"
            + md_table(["TTE bucket", "n", "mean net (c)", "95% CI"], rows)
        )

    event_count_rows = [
        [asset, f"{x:.0f}", f"{event_counts.get((asset, x), 0):,}"]
        for asset in assets
        for x in MOVE_THRESHOLDS_BPS
    ]

    sections = {
        "coverage": md_table(["date", "pm hours", "tob rows", "trade rows", "leader snapshots"], coverage_rows),
        "spread": md_table(
            ["asset", "TTE bucket", "samples", "median (c)", "p25 (c)", "p75 (c)", "median %mid", "two-sided"],
            spread_rows,
        ),
        "fees": md_table(["asset", "observed fee_rate_bps", "trades", "share"], fee_rows),
        "gamma_fee_rates": f"Gamma takerBaseFee rates across cycles: {gamma_fee_rates}",
        "event_counts": md_table(["asset", "X (bps)", "events evaluated"], event_count_rows),
        "study": md_table(
            ["asset", "X (bps)", "h (s)", "n", "mean response (c)",
             "net pre-move (c)", "pre-move 95% CI", "net executable (c)", "executable 95% CI"],
            study_rows,
        ),
        "calibration": md_table(["asset", "TTE probe (s)", "n", "Brier"], cal_rows),
        "reliability": md_table(["p(up) bucket", "n", "mean p", "realized freq"], rel_rows),
        "maker": (
            f"Total fills in window: {trades.height:,}; mark-outs computed on "
            f"{maker_trades.height:,} sampled fills.\n\n"
            + md_table(["asset", "mark-out h (s)", "fills", "mean pnl (c)", "95% CI (c)", "median (c)"], maker_rows)
        ),
        "verdict": md_table(
            ["asset", "best (X,h)", "max net pre-move (c)", "95% CI", "net executable (c)", "events", "verdict"],
            verdict_rows,
        ),
        "tte_breakdown": "\n".join(tte_lines),
    }
    payload = f"<!-- pm-clock: {pm_clock_provenance()} -->\n" + "\n".join(
        f"<!-- section:{name} -->\n{content}\n" for name, content in sections.items()
    )
    if args.report:
        Path(args.report).write_text(payload)
        print(f"analyze: wrote section tables to {args.report}")
    else:
        print(payload)


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    sub = parser.add_subparsers(dest="command", required=True)

    def common(p: argparse.ArgumentParser) -> None:
        p.add_argument("--dates", required=True, help="YYYY-MM-DD:YYYY-MM-DD range or comma list")
        p.add_argument("--assets", default=DEFAULT_ASSETS)
        p.add_argument("--workdir", default=str(DEFAULT_WORKDIR))

    p_resolve = sub.add_parser("resolve", help="fetch Gamma identity+settlement per cycle")
    common(p_resolve)
    p_resolve.add_argument("--concurrency", type=int, default=8)
    p_resolve.set_defaults(func=cmd_resolve)

    p_pm_catalog = sub.add_parser("extract-pm-catalog", help="extract Polymarket top-of-book + trades from NT catalog")
    common(p_pm_catalog)
    p_pm_catalog.add_argument("--catalog-config", required=True)
    p_pm_catalog.set_defaults(func=cmd_extract_pm_catalog)

    p_pm = sub.add_parser("extract-pm", help="legacy raw fallback for receive-offset work")
    common(p_pm)
    p_pm.add_argument("--concurrency", type=int, default=3)
    p_pm.set_defaults(func=cmd_extract_pm)

    p_leader = sub.add_parser("extract-leader", help="extract Hyperliquid l2Book mid series")
    common(p_leader)
    p_leader.add_argument("--concurrency", type=int, default=4)
    p_leader.set_defaults(func=cmd_extract_leader)

    p_an = sub.add_parser("analyze", help="run measurements 1-5, emit markdown tables")
    common(p_an)
    p_an.add_argument("--report", default="", help="write tables to this file instead of stdout")
    p_an.add_argument("--spread-sample-step", type=int, default=5, help="seconds between spread samples")
    p_an.add_argument("--min-events", type=int, default=30, help="min events for a verdict cell")
    p_an.add_argument("--maker-sample-cap", type=int, default=400_000, help="max fills for maker mark-outs")
    p_an.add_argument(
        "--pm-clock",
        choices=PM_CLOCK_CHOICES,
        default=DEFAULT_PM_CLOCK,
        help="PM event clock: venue (offset-free), receive (published studies), auto=venue when extracted",
    )
    p_an.set_defaults(func=cmd_analyze)

    args = parser.parse_args()
    args.func(args)


if __name__ == "__main__":
    main()
