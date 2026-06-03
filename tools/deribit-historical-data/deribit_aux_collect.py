"""
Standalone Deribit options AUXILIARY data collector (window-bounded).

Collects the four auxiliary data families that sit alongside the trade/quote
window data (which a separate fetcher already wrote under
``/private/tmp/deribit-window-data/<ASSET>/option/``):

    1. dvol         per-currency DVOL (volatility index) hourly candles
    2. settlements  per-currency delivery + settlement records
    3. metadata     per-asset option contract metadata (from saved instruments.json)
    4. mark_candles per-instrument 1-minute MARK-price OHLCV candles

CRITICAL host routing (verified empirically, 2026-06-03):
    * DVOL  (get_volatility_index_data)      -> https://www.deribit.com
        the history host returns HTTP 400 for this endpoint.
    * settlements (get_last_settlements_by_currency) -> https://www.deribit.com
    * mark candles (get_tradingview_chart_data) -> https://history.deribit.com

Output parquet lives under ``/private/tmp/deribit-window-aux/<family>/``.

Run via the tool's venv (polars + httpx live there):

    uv run --directory /private/tmp/deribit-historical-data \
        python deribit_aux_collect.py --family dvol

CLI:
    --family {dvol,settlements,metadata,mark_candles}   (required)
    --asset  BTC|ETH|SOL|XRP   (optional; default = the family's full asset set)
    --limit  N                 (mark_candles only: cap #instruments, for proving)

FAIL LOUD: every HTTP call goes through ``response.raise_for_status()``. No
silent ``except: pass``. A genuine "no data" answer from the API (empty result,
``status == 'no_data'``, or a known-absent currency) is recorded as a count and
does NOT crash; an unexpected HTTP/transport error propagates and aborts.
"""
from __future__ import annotations

import argparse
import json
import logging
import time
from pathlib import Path
from typing import Any

import httpx
import polars as pl

logger = logging.getLogger("deribit_aux_collect")

# ---------------------------------------------------------------------------
# Window + constants (single source of truth for the run).
# ---------------------------------------------------------------------------
WINDOW_START_MS = 1777420800000  # Apr 29 2026 00:00:00 UTC
WINDOW_END_MS = 1780272000000    # Jun 01 2026 00:00:00 UTC (half-open)

# Per-endpoint host routing. The history host 400s on DVOL/settlements; the www
# host serves them. Mark candles only exist on the history host.
WWW_HOST = "https://www.deribit.com/api/v2/public"
HISTORY_HOST = "https://history.deribit.com/api/v2/public"

# Window-data instrument lists the trade fetcher already wrote.
WINDOW_DATA_ROOT = Path("/private/tmp/deribit-window-data")
# Where this collector writes.
AUX_ROOT = Path("/private/tmp/deribit-window-aux")

# Which currencies each family covers.
DVOL_CURRENCIES = ["BTC", "ETH"]  # SOL/XRP DVOL is typically absent on Deribit.
# Settlement currencies: coin-margined BTC/ETH plus USDC (linears/options for
# SOL/XRP and friends settle in USDC on Deribit).
SETTLEMENT_CURRENCIES = ["BTC", "ETH", "USDC"]
ASSETS = ["BTC", "ETH", "SOL", "XRP"]

# get_tradingview_chart_data returns at most this many points per call, so the
# full 47,520-minute window must be paged by advancing start_timestamp.
# (Verified: a full-window call returns exactly 5001 trailing points.)
CHART_MAX_POINTS = 5000
CHART_RESOLUTION_MIN = 1  # minutes
MS_PER_MIN = 60_000

# Settlement paging page size (max the endpoint accepts).
SETTLEMENT_PAGE_COUNT = 1000


# ---------------------------------------------------------------------------
# HTTP client: one host-agnostic GET with a global <=18 req/s limiter and
# fail-loud raise_for_status. The caller passes the full host per call so a
# single client serves both hosts.
# ---------------------------------------------------------------------------
class RateLimitedClient:
    """httpx client wrapper enforcing a global request rate and fail-loud errors."""

    def __init__(self, max_rps: float = 18.0, timeout: float = 60.0) -> None:
        self._client = httpx.Client(timeout=timeout)
        self._min_interval = 1.0 / max_rps
        self._last_ts = 0.0

    def __enter__(self) -> "RateLimitedClient":
        return self

    def __exit__(self, *exc: object) -> None:
        self._client.close()

    def _throttle(self) -> None:
        now = time.monotonic()
        wait = self._min_interval - (now - self._last_ts)
        if wait > 0:
            time.sleep(wait)
        self._last_ts = time.monotonic()

    def get_result(self, host: str, endpoint: str, params: dict[str, Any]) -> Any:
        """GET ``{host}/{endpoint}`` and return ``result``. Raises on any HTTP error."""
        self._throttle()
        resp = self._client.get(f"{host}/{endpoint}", params=params)
        resp.raise_for_status()  # FAIL LOUD — no silent swallow.
        body = resp.json()
        if "error" in body:
            # Deribit signals API-level errors in the body even on HTTP 200.
            raise RuntimeError(
                f"Deribit API error for {endpoint} {params}: {body['error']}"
            )
        return body["result"]


# ---------------------------------------------------------------------------
# Shared helpers.
# ---------------------------------------------------------------------------
def _out_dir(family: str) -> Path:
    d = AUX_ROOT / family
    d.mkdir(parents=True, exist_ok=True)
    return d


def _in_window(ts: int) -> bool:
    return WINDOW_START_MS <= ts < WINDOW_END_MS


def _load_instruments(asset: str) -> list[dict[str, Any]]:
    """Load the already-saved in-window option instrument list for an asset."""
    path = WINDOW_DATA_ROOT / asset / "option" / "instruments.json"
    if not path.exists():
        raise SystemExit(f"Missing saved instrument list: {path}")
    with open(path) as fh:
        data = json.load(fh)
    if not isinstance(data, list):
        raise SystemExit(f"Expected a JSON list in {path}, got {type(data).__name__}")
    return data


# ---------------------------------------------------------------------------
# Family 1: DVOL.
# ---------------------------------------------------------------------------
def collect_dvol(client: RateLimitedClient, currencies: list[str]) -> None:
    out = _out_dir("dvol")
    for currency in currencies:
        params = {
            "currency": currency,
            "start_timestamp": WINDOW_START_MS,
            "end_timestamp": WINDOW_END_MS,
            "resolution": 3600,  # hourly
        }
        # SOL/XRP (and any future absent currency) may 400 or return empty. Treat
        # a 400 as genuine no-data; let any other HTTP error propagate.
        try:
            result = client.get_result(WWW_HOST, "get_volatility_index_data", params)
        except httpx.HTTPStatusError as exc:
            if exc.response.status_code == 400:
                logger.warning(
                    "DVOL %s: HTTP 400 (currency has no DVOL index) — recorded as no-data.",
                    currency,
                )
                _write_empty_parquet(
                    out / f"{currency}.parquet",
                    ["timestamp", "open", "high", "low", "close"],
                )
                continue
            raise

        rows = result.get("data", [])  # [[ts, open, high, low, close], ...]
        in_window = [r for r in rows if _in_window(int(r[0]))]
        if not in_window:
            logger.warning("DVOL %s: 0 in-window rows — recorded as no-data.", currency)
            _write_empty_parquet(
                out / f"{currency}.parquet",
                ["timestamp", "open", "high", "low", "close"],
            )
            continue

        df = pl.DataFrame(
            in_window,
            schema=["timestamp", "open", "high", "low", "close"],
            orient="row",
        ).with_columns(pl.col("timestamp").cast(pl.Int64))
        df = df.sort("timestamp")
        df.write_parquet(out / f"{currency}.parquet")
        ts = df["timestamp"]
        logger.info(
            "DVOL %s (www host): %d rows, ts [%d, %d].",
            currency,
            df.height,
            ts.min(),
            ts.max(),
        )


def _write_empty_parquet(path: Path, columns: list[str]) -> None:
    pl.DataFrame(schema={c: pl.Float64 for c in columns}).write_parquet(path)


# ---------------------------------------------------------------------------
# Family 2: settlements.
# ---------------------------------------------------------------------------
def collect_settlements(client: RateLimitedClient, currencies: list[str]) -> None:
    out = _out_dir("settlements")
    for currency in currencies:
        records: list[dict[str, Any]] = []
        for stype in ("delivery", "settlement"):
            records.extend(_page_settlements(client, currency, stype))

        if not records:
            logger.warning(
                "settlements %s: 0 in-window records — recorded as no-data.", currency
            )
            pl.DataFrame(schema={"timestamp": pl.Int64, "type": pl.Utf8}).write_parquet(
                out / f"{currency}.parquet"
            )
            continue

        # Flatten: every settlement record is already a flat dict; the field set
        # varies by type (delivery vs settlement carry slightly different keys).
        # polars unions the columns and fills missing with null.
        df = pl.DataFrame(records, infer_schema_length=None).sort("timestamp")
        df.write_parquet(out / f"{currency}.parquet")
        ts = df["timestamp"]
        logger.info(
            "settlements %s (www host): %d rows, ts [%d, %d], types=%s.",
            currency,
            df.height,
            ts.min(),
            ts.max(),
            df["type"].unique().to_list(),
        )


def _page_settlements(
    client: RateLimitedClient, currency: str, stype: str
) -> list[dict[str, Any]]:
    """Page get_last_settlements_by_currency for one type, keep in-window rows.

    The endpoint returns settlements newest-first and ``search_start_timestamp``
    is an UPPER seek bound: it returns records at/before that time, working
    backward. (Verified: anchoring at WINDOW_START_MS returns only records OLDER
    than the window — 0 in-window rows — because the latest real settlements are
    newer than the window. The correct anchor is the window's UPPER bound.) We
    therefore seek from WINDOW_END_MS, page backward via ``continuation``, and
    stop once a page's rows fall entirely before the window start or the cursor
    is exhausted.
    """
    kept: list[dict[str, Any]] = []
    continuation: str | None = None
    while True:
        params: dict[str, Any] = {
            "currency": currency,
            "type": stype,
            "count": SETTLEMENT_PAGE_COUNT,
            "search_start_timestamp": WINDOW_END_MS,
        }
        if continuation:
            params["continuation"] = continuation
        result = client.get_result(
            WWW_HOST, "get_last_settlements_by_currency", params
        )
        settlements = result.get("settlements", [])
        if not settlements:
            break

        page_max_ts = max(int(s["timestamp"]) for s in settlements)
        for s in settlements:
            ts = int(s["timestamp"])
            if _in_window(ts):
                kept.append(s)

        continuation = result.get("continuation") or None
        if not continuation:
            break
        # Records are newest-first; once an entire page predates the window start
        # there is nothing older worth fetching.
        if page_max_ts < WINDOW_START_MS:
            break
    return kept


# ---------------------------------------------------------------------------
# Family 3: metadata (no network).
# ---------------------------------------------------------------------------
_METADATA_FIELDS = [
    "instrument_name",
    "strike",
    "option_type",
    "expiration_timestamp",
    "creation_timestamp",
    "contract_size",
    "tick_size",
    "settlement_currency",
    "settlement_period",
    "base_currency",
    "quote_currency",
    "min_trade_amount",
    "instrument_id",
]


def collect_metadata(assets: list[str]) -> None:
    out = _out_dir("metadata")
    for asset in assets:
        instruments = _load_instruments(asset)
        rows = [{f: inst.get(f) for f in _METADATA_FIELDS} for inst in instruments]
        df = pl.DataFrame(rows, infer_schema_length=None).sort("instrument_name")
        df.write_parquet(out / f"{asset}.parquet")
        logger.info(
            "metadata %s (local): %d rows, columns=%s.",
            asset,
            df.height,
            df.columns,
        )


# ---------------------------------------------------------------------------
# Family 4: mark candles.
# ---------------------------------------------------------------------------
def collect_mark_candles(
    client: RateLimitedClient, assets: list[str], limit: int | None
) -> None:
    out = _out_dir("mark_candles")
    for asset in assets:
        instruments = _load_instruments(asset)
        names = sorted(i["instrument_name"] for i in instruments)
        if limit is not None:
            names = names[:limit]

        marker_dir = out / f".done_{asset}"
        marker_dir.mkdir(parents=True, exist_ok=True)
        part_dir = out / f".parts_{asset}"
        part_dir.mkdir(parents=True, exist_ok=True)

        n_ok = 0
        n_nodata = 0
        for name in names:
            done_marker = marker_dir / f"{name}.done"
            if done_marker.exists():
                n_ok += 1  # already collected (may have been empty; marker = settled)
                continue

            rows = _fetch_mark_candles(client, name)
            if rows:
                pl.DataFrame(
                    rows,
                    schema=[
                        "instrument_name",
                        "timestamp",
                        "open",
                        "high",
                        "low",
                        "close",
                        "volume",
                    ],
                    orient="row",
                ).write_parquet(part_dir / f"{name}.parquet")
                n_ok += 1
            else:
                n_nodata += 1
            done_marker.write_bytes(b"")

        # Merge per-instrument parts into the asset-level parquet.
        parts = sorted(part_dir.glob("*.parquet"))
        if parts:
            merged = pl.concat([pl.read_parquet(p) for p in parts]).sort(
                ["instrument_name", "timestamp"]
            )
            merged.write_parquet(out / f"{asset}.parquet")
            logger.info(
                "mark_candles %s (history host): %d instruments with data, "
                "%d no-data, %d total rows.",
                asset,
                n_ok,
                n_nodata,
                merged.height,
            )
        else:
            pl.DataFrame(
                schema={"instrument_name": pl.Utf8, "timestamp": pl.Int64}
            ).write_parquet(out / f"{asset}.parquet")
            logger.info(
                "mark_candles %s (history host): 0 instruments with data, "
                "%d no-data.",
                asset,
                n_nodata,
            )


def _fetch_mark_candles(
    client: RateLimitedClient, instrument: str
) -> list[list[Any]]:
    """Fetch all in-window 1-min MARK OHLCV candles for one option, paging the cap.

    get_tradingview_chart_data caps at ~5001 trailing points per call, so we page
    by advancing start_timestamp in CHART_MAX_POINTS-minute steps across the
    window. For options the OHLC arrays are MARK price. Returns a list of
    [instrument_name, ts, open, high, low, close, volume] rows.
    """
    seen_ts: set[int] = set()
    rows: list[list[Any]] = []
    step_ms = CHART_MAX_POINTS * CHART_RESOLUTION_MIN * MS_PER_MIN
    cursor = WINDOW_START_MS
    while cursor < WINDOW_END_MS:
        chunk_end = min(cursor + step_ms, WINDOW_END_MS)
        params = {
            "instrument_name": instrument,
            "start_timestamp": cursor,
            "end_timestamp": chunk_end,
            "resolution": CHART_RESOLUTION_MIN,
        }
        result = client.get_result(HISTORY_HOST, "get_tradingview_chart_data", params)
        status = result.get("status")
        ticks = result.get("ticks", [])
        if status != "ok" or not ticks:
            # 'no_data' for this chunk (e.g. before the instrument traded).
            cursor = chunk_end
            continue

        opens = result["open"]
        highs = result["high"]
        lows = result["low"]
        closes = result["close"]
        volumes = result["volume"]
        for i, ts in enumerate(ticks):
            ts = int(ts)
            if not _in_window(ts) or ts in seen_ts:
                continue
            seen_ts.add(ts)
            rows.append(
                [instrument, ts, opens[i], highs[i], lows[i], closes[i], volumes[i]]
            )
        cursor = chunk_end
    rows.sort(key=lambda r: r[1])
    return rows


# ---------------------------------------------------------------------------
# CLI.
# ---------------------------------------------------------------------------
def _setup_logging() -> None:
    logging.basicConfig(
        level=logging.INFO,
        format="%(asctime)s %(levelname)s %(name)s: %(message)s",
    )


def main() -> None:
    parser = argparse.ArgumentParser(
        description="Window-bounded Deribit auxiliary data collector."
    )
    parser.add_argument(
        "--family",
        choices=["dvol", "settlements", "metadata", "mark_candles"],
        required=True,
    )
    parser.add_argument(
        "--asset",
        choices=ASSETS + SETTLEMENT_CURRENCIES,
        default=None,
        help="Restrict to a single asset/currency (default = the family's full set).",
    )
    parser.add_argument(
        "--limit",
        type=int,
        default=None,
        help="mark_candles only: cap the number of instruments (for proving).",
    )
    args = parser.parse_args()

    _setup_logging()
    logger.info(
        "Family=%s asset=%s window=[%d, %d)",
        args.family,
        args.asset or "ALL",
        WINDOW_START_MS,
        WINDOW_END_MS,
    )

    if args.family == "metadata":
        assets = [args.asset] if args.asset else ASSETS
        collect_metadata(assets)
        return

    with RateLimitedClient() as client:
        if args.family == "dvol":
            currencies = [args.asset] if args.asset else DVOL_CURRENCIES
            collect_dvol(client, currencies)
        elif args.family == "settlements":
            currencies = [args.asset] if args.asset else SETTLEMENT_CURRENCIES
            collect_settlements(client, currencies)
        elif args.family == "mark_candles":
            assets = [args.asset] if args.asset else ASSETS
            collect_mark_candles(client, assets, args.limit)


if __name__ == "__main__":
    main()
