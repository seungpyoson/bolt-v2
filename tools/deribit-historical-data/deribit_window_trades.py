"""
Standalone WINDOW-BOUNDED Deribit trade fetcher.

Unlike the tool's default path (which walks ``trade_seq`` from 1 and pulls the
*entire* lifetime of every instrument — years / multiple GB for BTC-PERPETUAL),
this script fetches ONLY a bounded time window using Deribit's time-bounded
endpoint:

    GET /public/get_last_trades_by_instrument_and_time
    params: instrument_name, start_timestamp (ms), end_timestamp (ms),
            count (<=1000), sorting=asc
    returns: result.trades (ascending by time), result.has_more (bool)

It reuses the tool's patched config (``deribit_fetcher.config.settings``, which
reads CURRENCY / QUERY_CURRENCY / BASE_CURRENCY / WINDOW_START_MS / WINDOW_END_MS
from the environment) and ``DeribitClient.get_instruments`` (already filters by
BASE_CURRENCY and the instrument-lifetime window). It writes JSONL in the exact
on-disk shape ``scripts/gen_parquet.py`` consumes — one raw trade object per line
into ``data/<CURRENCY>/<kind>/<instrument_name>.jsonl`` — so the existing merge
step works unchanged.

Run via the tool's venv, e.g.:

    CURRENCY=BTC WINDOW_START_MS=1777420800000 WINDOW_END_MS=1777422600000 \
        uv run --directory /private/tmp/deribit-historical-data \
        python deribit_window_trades.py --kind future
"""
from __future__ import annotations

import argparse
import asyncio
import logging
from pathlib import Path

import orjson

from deribit_fetcher.client import DeribitClient
from deribit_fetcher.config import settings
from deribit_fetcher.log import setup_logging

logger = logging.getLogger("deribit_window_trades")

# Deribit time-bounded trade endpoint. Lives on the history host (already the
# base_url of DeribitClient via settings.BASE_URL).
_ENDPOINT = "/get_last_trades_by_instrument_and_time"

# Max trades the endpoint returns per page.
_PAGE_COUNT = 1000


def _resolve_window() -> tuple[int, int]:
    """Half-open window [start, end) in epoch ms. Both bounds are required."""
    ws = settings.WINDOW_START_MS
    we = settings.WINDOW_END_MS
    if ws is None or we is None:
        raise SystemExit(
            "WINDOW_START_MS and WINDOW_END_MS env vars are both required "
            "(epoch ms). This fetcher is window-bounded by design."
        )
    if we <= ws:
        raise SystemExit(
            f"WINDOW_END_MS ({we}) must be greater than WINDOW_START_MS ({ws})."
        )
    return ws, we


async def _page_trades_by_time(
    client: DeribitClient,
    instrument: str,
    start_ms: int,
    end_ms: int,
) -> int:
    """
    Fetch all trades for ``instrument`` in the half-open window [start_ms, end_ms)
    and write them to ``data/<CURRENCY>/<kind-dir>/<instrument>.jsonl``.

    Returns the number of in-window trades written. FAIL LOUD: any HTTP error
    raised by ``client._fetch`` (which calls ``response.raise_for_status()``)
    propagates — no silent except-pass.

    Resumability:
      * a sibling ``<instrument>.jsonl.done`` marker means "already complete" —
        callers skip such instruments before calling this function.
      * we write to ``<instrument>.jsonl.partial`` and atomically rename to the
        final path only after the whole window is captured, so a stale full-
        history ``<instrument>.jsonl`` is never appended to, and a crashed run
        leaves no half-written final file.
    """
    out_dir = settings.BASE_DIR / _kind_dir(instrument)
    out_dir.mkdir(parents=True, exist_ok=True)
    final_path = out_dir / f"{instrument}.jsonl"
    partial_path = out_dir / f"{instrument}.jsonl.partial"
    done_path = out_dir / f"{instrument}.jsonl.done"

    # Dedup across pages by trade_id. Same-ms trades straddle the page boundary
    # because we advance the cursor to the last trade's timestamp, so the first
    # trades of the next page can repeat the last ms of the previous page.
    seen_ids: set[str] = set()
    written = 0

    # Truncate any prior partial so a resumed-but-incomplete run starts clean.
    with open(partial_path, "wb") as out:
        cursor = start_ms
        while True:
            params = {
                "instrument_name": instrument,
                "start_timestamp": cursor,
                "end_timestamp": end_ms,
                "count": _PAGE_COUNT,
                "sorting": "asc",
            }
            data = await client._fetch(_ENDPOINT, params)
            result = data["result"]
            trades = result["trades"]
            has_more = result["has_more"]

            if not trades:
                break

            # Trades are ascending by time; the last element carries the page's
            # max timestamp. Capture it before filtering so the cursor advances
            # even if every trade on this page is out-of-window or a dup.
            page_last_ts = trades[-1]["timestamp"]

            lines: list[bytes] = []
            for trade in trades:
                ts = trade["timestamp"]
                # Half-open window: start <= ts < end.
                if ts < start_ms or ts >= end_ms:
                    continue
                tid = trade["trade_id"]
                if tid in seen_ids:
                    continue
                seen_ids.add(tid)
                lines.append(orjson.dumps(trade))

            if lines:
                out.write(b"\n".join(lines) + b"\n")
                written += len(lines)

            # Stop conditions: no more data upstream, or we've paged at/over the
            # window end (next trades would all be >= end_ms anyway).
            if not has_more:
                break
            if page_last_ts >= end_ms:
                break

            # Advance the cursor. Normally to the last trade's timestamp so the
            # next page resumes at the same ms (overlap handled by trade_id
            # dedup). If a single ms holds a full page (cursor would not move),
            # step +1 ms to guarantee forward progress and termination — any
            # trades at that exact ms we already captured are dedup'd anyway.
            next_cursor = page_last_ts
            if next_cursor <= cursor:
                next_cursor = cursor + 1
            cursor = next_cursor

    # Atomically publish: replace any stale final file, then drop the marker.
    partial_path.replace(final_path)
    done_path.write_bytes(b"")

    return written


def _kind_dir(_instrument: str) -> str:
    """Subdirectory under data/<CURRENCY>/ for the current run's kind."""
    # Set once in main() before any instrument is processed.
    return _KIND_DIR_HOLDER["kind"]


_KIND_DIR_HOLDER: dict[str, str] = {"kind": ""}


def _is_done(instrument: str) -> bool:
    done_path = settings.BASE_DIR / _KIND_DIR_HOLDER["kind"] / f"{instrument}.jsonl.done"
    return done_path.exists()


async def run(kind: str) -> None:
    start_ms, end_ms = _resolve_window()
    _KIND_DIR_HOLDER["kind"] = kind

    logger.info(
        "Window fetch: CURRENCY=%s QUERY_CURRENCY=%s BASE_CURRENCY=%s kind=%s "
        "window=[%d, %d)",
        settings.CURRENCY,
        settings.QUERY_CURRENCY,
        settings.BASE_CURRENCY,
        kind,
        start_ms,
        end_ms,
    )

    async with DeribitClient() as client:
        # Enumerate in-window instruments. get_instruments already filters by
        # BASE_CURRENCY and the lifetime window, and persists instruments.json.
        instruments = await client.get_instruments(settings.QUERY_CURRENCY, kind)
        names = sorted(i["instrument_name"] for i in instruments)
        logger.info("Enumerated %d in-window %s instruments.", len(names), kind)

        total_written = 0
        for name in names:
            if _is_done(name):
                logger.info("Skip (already complete): %s", name)
                continue
            written = await _page_trades_by_time(client, name, start_ms, end_ms)
            total_written += written
            logger.info("%s: wrote %d in-window trades.", name, written)

        logger.info(
            "Done. %d instruments, %d total in-window trades written.",
            len(names),
            total_written,
        )


def main() -> None:
    parser = argparse.ArgumentParser(
        description=(
            "Window-bounded Deribit trade fetcher. Reads CURRENCY / "
            "QUERY_CURRENCY / BASE_CURRENCY / WINDOW_START_MS / WINDOW_END_MS "
            "from the environment (same as the tool)."
        )
    )
    parser.add_argument(
        "--kind",
        choices=["future", "option"],
        required=True,
        help="Instrument kind to fetch.",
    )
    args = parser.parse_args()

    setup_logging()
    asyncio.run(run(args.kind))


if __name__ == "__main__":
    main()
