#!/usr/bin/env python3
"""Fetch Hyperliquid HIP-4 source proofs and stage them in S3."""

from __future__ import annotations

import argparse
import datetime as dt
import hashlib
import json
import pathlib
import re
import subprocess
import sys
import time
import urllib.error
import urllib.request
from typing import Any


INFO_URL = "https://api.hyperliquid.xyz/info"
ALLOWED_S3_PREFIX = "s3://bolt-parquet/backfill-staging/2026-06-01/hyperliquid-hip4"
SCRATCH_NAME_PREFIX = "bolt-v2-hyperliquid-hip4-backfill-"
USER_AGENT = "bolt-v2-hyperliquid-hip4-backfill/1"
VENUE = "hyperliquid"
PRODUCT_FAMILY = "prediction_market_outcome"
ORDERED_INTERVALS = ["1m", "3m", "5m", "15m", "30m", "1h", "2h", "4h", "8h", "12h", "1d", "3d", "1w", "1M"]
CANDLE_INTERVALS = set(ORDERED_INTERVALS)
MAX_CANDLES_PER_REQUEST = 5000  # official Hyperliquid candleSnapshot response cap
# interval -> milliseconds; used only to size paging spans and advance the cursor.
# 1M is a 30-day nominal span here (sizing only) — actual candle open times come from the API.
INTERVAL_MS = {
    "1m": 60_000, "3m": 180_000, "5m": 300_000, "15m": 900_000, "30m": 1_800_000,
    "1h": 3_600_000, "2h": 7_200_000, "4h": 14_400_000, "8h": 28_800_000, "12h": 43_200_000,
    "1d": 86_400_000, "3d": 259_200_000, "1w": 604_800_000, "1M": 2_592_000_000,
}
assert set(INTERVAL_MS) == CANDLE_INTERVALS, "INTERVAL_MS must cover every candle interval"
SOURCE_DOCS = [
    "https://hyperliquid.gitbook.io/hyperliquid-docs/for-developers/api/asset-ids",
    "https://hyperliquid.gitbook.io/hyperliquid-docs/for-developers/api/info-endpoint",
    "https://hyperliquid.gitbook.io/hyperliquid-docs/historical-data",
]


def stable_json(value: Any) -> str:
    return json.dumps(value, indent=2, sort_keys=True, ensure_ascii=True) + "\n"


def compact_json(value: Any) -> str:
    return json.dumps(value, separators=(",", ":"), sort_keys=True, ensure_ascii=True)


def utc_now() -> str:
    return dt.datetime.now(dt.UTC).replace(microsecond=0).isoformat().replace("+00:00", "Z")


def sha256_bytes(payload: bytes) -> str:
    return hashlib.sha256(payload).hexdigest()


def parse_utc(value: str) -> dt.datetime:
    if not value.endswith("Z"):
        raise ValueError(f"UTC timestamp must end in Z: {value}")
    return dt.datetime.fromisoformat(value[:-1] + "+00:00").astimezone(dt.UTC)


def millis(value: str) -> int:
    return int(parse_utc(value).timestamp() * 1000)


def utc_from_millis(value: int | None) -> str | None:
    if value is None:
        return None
    return dt.datetime.fromtimestamp(value / 1000, tz=dt.UTC).isoformat().replace("+00:00", "Z")


def require_allowed_s3_prefix(prefix: str) -> str:
    normalized = prefix.rstrip("/")
    if normalized != ALLOWED_S3_PREFIX:
        raise ValueError(f"S3 prefix must be exactly {ALLOWED_S3_PREFIX}/")
    return normalized


def require_scratch_root(path: pathlib.Path) -> pathlib.Path:
    resolved = path.resolve()
    if resolved.parent != pathlib.Path("/private/tmp"):
        raise ValueError("scratch root must be directly under /private/tmp")
    if not resolved.name.startswith(SCRATCH_NAME_PREFIX):
        raise ValueError(f"scratch root name must start with {SCRATCH_NAME_PREFIX}")
    return resolved


def partition_value(value: str) -> str:
    text = value.replace("#", "hash_").replace("+", "plus_")
    safe = re.sub(r"[^A-Za-z0-9._=-]+", "_", text)
    return safe.strip("_") or "unknown"


def local_path(root: pathlib.Path, *parts: str) -> pathlib.Path:
    return root.joinpath(*parts)


def s3_uri(prefix: str, *parts: str) -> str:
    return "/".join([prefix.rstrip("/"), *[part.strip("/") for part in parts]])


def write_bytes(path: pathlib.Path, payload: bytes) -> dict[str, Any]:
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_bytes(payload)
    return {"path": str(path), "bytes": len(payload), "sha256": sha256_bytes(payload)}


def write_json(path: pathlib.Path, value: Any) -> dict[str, Any]:
    return write_bytes(path, stable_json(value).encode("utf-8"))


def write_jsonl(path: pathlib.Path, rows: list[dict[str, Any]]) -> dict[str, Any]:
    payload = "".join(compact_json(row) + "\n" for row in rows).encode("utf-8")
    return write_bytes(path, payload)


def post_info(
    body: dict[str, Any],
    max_retries: int,
    retry_base_sleep_seconds: float,
) -> tuple[int, dict[str, str], bytes, Any]:
    attempt = 0
    while True:
        request = urllib.request.Request(
            INFO_URL,
            data=compact_json(body).encode("utf-8"),
            headers={"Content-Type": "application/json", "User-Agent": USER_AGENT},
            method="POST",
        )
        try:
            with urllib.request.urlopen(request, timeout=60) as response:
                payload = response.read()
                headers = {key: value for key, value in response.headers.items()}
                return response.status, headers, payload, json.loads(payload)
        except urllib.error.HTTPError as exc:
            if exc.code not in {429, 500, 502, 503, 504} or attempt >= max_retries:
                raise
            time.sleep(retry_base_sleep_seconds * (2**attempt))
            attempt += 1


def upload_file(local: pathlib.Path, target: str) -> None:
    subprocess.run(["aws", "s3", "cp", str(local), target, "--only-show-errors"], check=True)


def upload_record(local: pathlib.Path, target: str) -> dict[str, Any]:
    upload_file(local, target)
    payload = local.read_bytes()
    return {
        "local_path": str(local),
        "s3_uri": target,
        "bytes": len(payload),
        "sha256": sha256_bytes(payload),
    }


def source_family_slug(body: dict[str, Any]) -> str:
    request_type = str(body["type"])
    if request_type in {"l2Book", "recentTrades"}:
        return f"info.{request_type}.coin={partition_value(str(body['coin']))}"
    if request_type == "candleSnapshot":
        req = body["req"]
        return f"info.candleSnapshot.coin={partition_value(str(req['coin']))}.interval={partition_value(str(req['interval']))}"
    return f"info.{request_type}"


def write_raw_payload(
    root: pathlib.Path,
    run_id: str,
    body: dict[str, Any],
    status: int,
    headers: dict[str, str],
    payload: bytes,
) -> dict[str, Any]:
    payload_hash = sha256_bytes(payload)
    family = source_family_slug(body)
    request_type = str(body["type"])
    parts = ["raw", "v1", f"source_family={partition_value('info.' + request_type)}"]
    if request_type in {"l2Book", "recentTrades"}:
        parts.append(f"coin={partition_value(str(body['coin']))}")
    if request_type == "candleSnapshot":
        req = body["req"]
        parts.extend(
            [
                f"coin={partition_value(str(req['coin']))}",
                f"interval={partition_value(str(req['interval']))}",
            ]
        )
    parts.extend([f"run={run_id}", f"payload={payload_hash}.json"])
    path = local_path(root, *parts)
    write_bytes(path, payload)
    return {
        "source_family": family,
        "request_body": body,
        "http_status": status,
        "content_type": headers.get("Content-Type") or headers.get("content-type"),
        "payload_hash": payload_hash,
        "bytes": len(payload),
        "local_path": str(path),
    }


def parse_description(value: Any) -> dict[str, str]:
    if not isinstance(value, str) or "|" not in value:
        return {}
    fields: dict[str, str] = {}
    for part in value.split("|"):
        key, separator, field_value = part.partition(":")
        if separator and key:
            fields[key] = field_value
    return fields


def outcome_id(value: Any) -> int:
    if isinstance(value, bool):
        raise ValueError("outcome id cannot be bool")
    if isinstance(value, int):
        return value
    if isinstance(value, str) and value.isdigit():
        return int(value)
    raise ValueError(f"invalid outcome id: {value!r}")


def build_universe(outcome_meta: Any, generated_at: str) -> tuple[list[dict[str, Any]], list[dict[str, Any]], list[dict[str, Any]], list[str]]:
    if not isinstance(outcome_meta, dict):
        raise ValueError("outcomeMeta response is not an object")
    raw_outcomes = outcome_meta.get("outcomes", [])
    raw_questions = outcome_meta.get("questions", [])
    if not isinstance(raw_outcomes, list):
        raise ValueError("outcomeMeta outcomes field is not a list")
    if not isinstance(raw_questions, list):
        raise ValueError("outcomeMeta questions field is not a list")

    event_rows: list[dict[str, Any]] = []
    side_rows: list[dict[str, Any]] = []
    question_rows: list[dict[str, Any]] = []
    warnings: list[str] = []

    for index, item in enumerate(raw_outcomes):
        if not isinstance(item, dict):
            warnings.append(f"skipped non-object outcome at index {index}")
            continue
        oid = outcome_id(item.get("outcome"))
        side_specs = item.get("sideSpecs", [])
        if not isinstance(side_specs, list):
            warnings.append(f"outcome {oid} sideSpecs is not a list")
            side_specs = []
        if len(side_specs) != 2:
            warnings.append(f"outcome {oid} has {len(side_specs)} sideSpecs; official asset encoding only permits side 0 and side 1")
        quote_token = item.get("quoteToken")
        parsed_description = parse_description(item.get("description"))
        event_rows.append(
            {
                "venue": VENUE,
                "product_family": PRODUCT_FAMILY,
                "taxonomy": PRODUCT_FAMILY,
                "source_family": "info.outcomeMeta",
                "outcome": oid,
                "outcome_name": item.get("name"),
                "description": item.get("description"),
                "parsed_description": parsed_description,
                "quote_token": quote_token,
                "side_count": len(side_specs),
                "snapshot_at": generated_at,
                "raw_outcome": item,
            }
        )
        for side_index, side_spec in enumerate(side_specs):
            if side_index not in {0, 1}:
                warnings.append(f"skipped outcome {oid} side {side_index}; official asset encoding only permits side 0 and side 1")
                continue
            if not isinstance(side_spec, dict):
                side_spec = {"raw_side_spec": side_spec}
            encoding = 10 * oid + side_index
            side_rows.append(
                {
                    "venue": VENUE,
                    "product_family": PRODUCT_FAMILY,
                    "taxonomy": PRODUCT_FAMILY,
                    "source_family": "info.outcomeMeta",
                    "outcome": oid,
                    "side": side_index,
                    "side_name": side_spec.get("name"),
                    "encoding": encoding,
                    "trade_coin": f"#{encoding}",
                    "token_name": f"+{encoding}",
                    "asset_id": 100_000_000 + encoding,
                    "quote_token": quote_token,
                    "outcome_name": item.get("name"),
                    "description": item.get("description"),
                    "parsed_description": parsed_description,
                    "snapshot_at": generated_at,
                    "raw_side_spec": side_spec,
                }
            )

    for index, item in enumerate(raw_questions):
        question_rows.append(
            {
                "venue": VENUE,
                "product_family": PRODUCT_FAMILY,
                "taxonomy": PRODUCT_FAMILY,
                "source_family": "info.outcomeMeta",
                "question_index": index,
                "snapshot_at": generated_at,
                "raw_question": item,
            }
        )

    return event_rows, side_rows, question_rows, warnings


def l2_snapshot_row(side: dict[str, Any], payload_record: dict[str, Any], payload: Any, generated_at: str) -> dict[str, Any]:
    levels = payload.get("levels") if isinstance(payload, dict) else None
    bids = levels[0] if isinstance(levels, list) and levels else []
    asks = levels[1] if isinstance(levels, list) and len(levels) > 1 else []
    snapshot_time = payload.get("time") if isinstance(payload, dict) and isinstance(payload.get("time"), int) else None
    return {
        "venue": VENUE,
        "product_family": PRODUCT_FAMILY,
        "taxonomy": PRODUCT_FAMILY,
        "source_family": "info.l2Book",
        "outcome": side["outcome"],
        "side": side["side"],
        "trade_coin": side["trade_coin"],
        "quote_token": side["quote_token"],
        "snapshot_time": snapshot_time,
        "snapshot_time_utc": utc_from_millis(snapshot_time),
        "captured_at": generated_at,
        "bid_level_count": len(bids) if isinstance(bids, list) else 0,
        "ask_level_count": len(asks) if isinstance(asks, list) else 0,
        "best_bid_px": bids[0].get("px") if isinstance(bids, list) and bids and isinstance(bids[0], dict) else None,
        "best_ask_px": asks[0].get("px") if isinstance(asks, list) and asks and isinstance(asks[0], dict) else None,
        "raw_payload_hash": payload_record["payload_hash"],
        "raw_levels": levels if isinstance(levels, list) else None,
    }


def recent_trade_rows(
    side: dict[str, Any],
    payload_record: dict[str, Any],
    payload: Any,
    generated_at: str,
) -> tuple[list[dict[str, Any]], dict[str, Any]]:
    rows: list[dict[str, Any]] = []
    if isinstance(payload, list):
        for item in payload:
            if not isinstance(item, dict):
                continue
            row_time = item.get("time") if isinstance(item.get("time"), int) else None
            rows.append(
                {
                    "venue": VENUE,
                    "product_family": PRODUCT_FAMILY,
                    "taxonomy": PRODUCT_FAMILY,
                    "source_family": "info.recentTrades",
                    "outcome": side["outcome"],
                    "side": side["side"],
                    "trade_coin": side["trade_coin"],
                    "quote_token": side["quote_token"],
                    "time": row_time,
                    "time_utc": utc_from_millis(row_time),
                    "captured_at": generated_at,
                    "px": item.get("px"),
                    "sz": item.get("sz"),
                    "trade_side": item.get("side"),
                    "tid": item.get("tid"),
                    "hash": item.get("hash"),
                    "raw_payload_hash": payload_record["payload_hash"],
                    "raw_trade": item,
                }
            )
    times = [row["time"] for row in rows if isinstance(row.get("time"), int)]
    coverage = {
        "source_family": "info.recentTrades",
        "trade_coin": side["trade_coin"],
        "outcome": side["outcome"],
        "side": side["side"],
        "row_count": len(rows),
        "first_time": min(times) if times else None,
        "first_time_utc": utc_from_millis(min(times)) if times else None,
        "last_time": max(times) if times else None,
        "last_time_utc": utc_from_millis(max(times)) if times else None,
        "raw_payload_hash": payload_record["payload_hash"],
        "raw_bytes": payload_record["bytes"],
        "coverage_statement": "current_recent_response_rows_only",
    }
    return rows, coverage


def paged_candle_snapshot(
    coin: str,
    interval: str,
    window_start_ms: int,
    window_end_ms: int,
    fetch: Any,
    request_sleep_seconds: float,
    max_pages: int,
) -> tuple[list[dict[str, Any]], dict[int, str], dict[str, Any]]:
    """Page candleSnapshot across the full window, deduping by open time.

    Each request asks for the full remaining range [cursor, window_end]. The official API
    returns candles oldest-first, capped at MAX_CANDLES_PER_REQUEST. A short (< cap) or empty
    response means every candle in [cursor, window_end] has been captured, so we stop. Only a
    full (== cap) response means the cap was hit and more may exist after the last returned
    candle, so we advance the cursor past it and page forward.

    `fetch` is main()'s fetch_and_record closure: it records each page as its own raw payload
    and returns (raw_record, parsed). Returns the deduped candle list (ascending by open time),
    an open_time -> contributing page payload hash map, and page metadata.
    """
    interval_ms = INTERVAL_MS[interval]
    by_open: dict[int, dict[str, Any]] = {}
    page_hash_by_open: dict[int, str] = {}
    page_payload_hashes: list[str] = []
    raw_bytes_total = 0
    cursor = window_start_ms
    pages = 0
    cap_hit = True  # assume more until a request returns fewer than the cap
    while cursor < window_end_ms and pages < max_pages and cap_hit:
        body = {
            "type": "candleSnapshot",
            "req": {"coin": coin, "interval": interval, "startTime": cursor, "endTime": window_end_ms},
        }
        record, parsed = fetch(body)
        pages += 1
        page_payload_hashes.append(record["payload_hash"])
        raw_bytes_total += record["bytes"]
        rows = parsed if isinstance(parsed, list) else []
        max_open = None
        for item in rows:
            if not isinstance(item, dict):
                continue
            open_time = item.get("t")
            if not isinstance(open_time, int):
                continue
            by_open[open_time] = item
            page_hash_by_open[open_time] = record["payload_hash"]
            if max_open is None or open_time > max_open:
                max_open = open_time
        cap_hit = max_open is not None and len(rows) >= MAX_CANDLES_PER_REQUEST
        if cap_hit:
            next_cursor = max_open + interval_ms
            cursor = next_cursor if next_cursor > cursor else window_end_ms  # guarantee forward progress
        time.sleep(request_sleep_seconds)
    truncated = cap_hit and cursor < window_end_ms  # stopped at --max-candle-pages with the cap still hit
    combined = [by_open[open_time] for open_time in sorted(by_open)]
    page_meta = {
        "pages": pages,
        "truncated": truncated,
        "page_payload_hashes": page_payload_hashes,
        "raw_bytes_total": raw_bytes_total,
    }
    return combined, page_hash_by_open, page_meta


def build_candle_rows(
    side: dict[str, Any],
    combined: list[dict[str, Any]],
    page_hash_by_open: dict[int, str],
    interval: str,
    requested_start: str,
    requested_end: str,
    generated_at: str,
    page_meta: dict[str, Any],
) -> tuple[list[dict[str, Any]], dict[str, Any]]:
    rows: list[dict[str, Any]] = []
    for item in combined:
        open_time = item.get("t") if isinstance(item.get("t"), int) else None
        close_time = item.get("T") if isinstance(item.get("T"), int) else None
        rows.append(
            {
                "venue": VENUE,
                "product_family": PRODUCT_FAMILY,
                "taxonomy": PRODUCT_FAMILY,
                "source_family": "info.candleSnapshot",
                "outcome": side["outcome"],
                "side": side["side"],
                "trade_coin": side["trade_coin"],
                "quote_token": side["quote_token"],
                "interval": interval,
                "requested_start_utc": requested_start,
                "requested_end_utc": requested_end,
                "open_time": open_time,
                "open_time_utc": utc_from_millis(open_time),
                "close_time": close_time,
                "close_time_utc": utc_from_millis(close_time),
                "captured_at": generated_at,
                "open": item.get("o"),
                "high": item.get("h"),
                "low": item.get("l"),
                "close": item.get("c"),
                "volume": item.get("v"),
                "trade_count": item.get("n"),
                "raw_payload_hash": page_hash_by_open.get(open_time) if open_time is not None else None,
                "raw_candle": item,
            }
        )
    times = [row["open_time"] for row in rows if isinstance(row.get("open_time"), int)]
    coverage = {
        "source_family": "info.candleSnapshot",
        "trade_coin": side["trade_coin"],
        "outcome": side["outcome"],
        "side": side["side"],
        "interval": interval,
        "requested_start_utc": requested_start,
        "requested_end_utc": requested_end,
        "row_count": len(rows),
        "first_open_time": min(times) if times else None,
        "first_open_time_utc": utc_from_millis(min(times)) if times else None,
        "last_open_time": max(times) if times else None,
        "last_open_time_utc": utc_from_millis(max(times)) if times else None,
        "pages": page_meta["pages"],
        "truncated": page_meta["truncated"],
        "dedup_candle_count": len(combined),
        "page_payload_hashes": page_meta["page_payload_hashes"],
        "raw_bytes": page_meta["raw_bytes_total"],
        "coverage_statement": "paged_candleSnapshot_full_window_max_5000_candles_per_page_deduped_by_open_time",
    }
    return rows, coverage


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--s3-prefix", required=True)
    parser.add_argument("--scratch-root", required=True, type=pathlib.Path)
    parser.add_argument("--window-start-utc", required=True)
    parser.add_argument("--window-end-utc", required=True)
    parser.add_argument(
        "--candle-interval",
        default=",".join(ORDERED_INTERVALS),
        help="comma-separated candle intervals to page across the full window (default: all intervals)",
    )
    parser.add_argument(
        "--max-candle-pages",
        type=int,
        default=64,
        help="max paged candleSnapshot requests per (coin, interval) before flagging truncation",
    )
    parser.add_argument("--max-outcomes", type=int)
    parser.add_argument("--request-sleep-seconds", type=float, default=0.10)
    parser.add_argument("--max-retries", type=int, default=6)
    parser.add_argument("--retry-base-sleep-seconds", type=float, default=1.0)
    return parser.parse_args()


def main() -> int:
    args = parse_args()
    s3_prefix = require_allowed_s3_prefix(args.s3_prefix)
    scratch_root = require_scratch_root(args.scratch_root)
    window_start_ms = millis(args.window_start_utc)
    window_end_ms = millis(args.window_end_utc)
    if window_start_ms >= window_end_ms:
        raise ValueError("window start must be before window end")
    requested_intervals = [token.strip() for token in args.candle_interval.split(",") if token.strip()]
    unknown = [token for token in requested_intervals if token not in CANDLE_INTERVALS]
    if unknown:
        raise ValueError(f"unsupported candle interval(s): {unknown}")
    if not requested_intervals:
        raise ValueError("at least one candle interval is required")
    # canonical order, deduped
    intervals = [token for token in ORDERED_INTERVALS if token in set(requested_intervals)]
    if args.max_candle_pages < 1:
        raise ValueError("--max-candle-pages must be >= 1")

    generated_at = utc_now()
    run_seed = f"{generated_at}|{s3_prefix}|{args.window_start_utc}|{args.window_end_utc}|{','.join(intervals)}"
    run_id = "run-" + generated_at.replace("-", "").replace(":", "").replace("Z", "Z-") + sha256_bytes(run_seed.encode())[:12]
    scratch_root.mkdir(parents=True, exist_ok=True)

    raw_records: list[dict[str, Any]] = []
    uploads: list[dict[str, Any]] = []
    errors: list[dict[str, Any]] = []

    def fetch_and_record(body: dict[str, Any]) -> tuple[dict[str, Any], Any]:
        status, headers, payload, parsed = post_info(body, args.max_retries, args.retry_base_sleep_seconds)
        record = write_raw_payload(scratch_root, run_id, body, status, headers, payload)
        raw_records.append(record)
        return record, parsed

    outcome_meta_raw, outcome_meta = fetch_and_record({"type": "outcomeMeta"})
    event_rows, side_rows, question_rows, warnings = build_universe(outcome_meta, generated_at)
    if args.max_outcomes is not None:
        allowed_outcomes = {row["outcome"] for row in event_rows[: args.max_outcomes]}
        event_rows = [row for row in event_rows if row["outcome"] in allowed_outcomes]
        side_rows = [row for row in side_rows if row["outcome"] in allowed_outcomes]

    l2_rows: list[dict[str, Any]] = []
    trade_rows: list[dict[str, Any]] = []
    trade_coverage: list[dict[str, Any]] = []
    bar_rows: list[dict[str, Any]] = []
    bar_coverage: list[dict[str, Any]] = []

    for side in side_rows:
        try:
            raw_record, parsed = fetch_and_record({"type": "l2Book", "coin": side["trade_coin"]})
            l2_rows.append(l2_snapshot_row(side, raw_record, parsed, generated_at))
        except Exception as exc:  # noqa: BLE001 - manifest records per-side source failures.
            errors.append({"source_family": "info.l2Book", "trade_coin": side["trade_coin"], "error": repr(exc)})
        time.sleep(args.request_sleep_seconds)

        try:
            raw_record, parsed = fetch_and_record({"type": "recentTrades", "coin": side["trade_coin"]})
            rows, coverage = recent_trade_rows(side, raw_record, parsed, generated_at)
            trade_rows.extend(rows)
            trade_coverage.append(coverage)
        except Exception as exc:  # noqa: BLE001 - manifest records per-side source failures.
            errors.append({"source_family": "info.recentTrades", "trade_coin": side["trade_coin"], "error": repr(exc)})
        time.sleep(args.request_sleep_seconds)

        for interval in intervals:
            try:
                combined, page_hash_by_open, page_meta = paged_candle_snapshot(
                    side["trade_coin"],
                    interval,
                    window_start_ms,
                    window_end_ms,
                    fetch_and_record,
                    args.request_sleep_seconds,
                    args.max_candle_pages,
                )
                rows, coverage = build_candle_rows(
                    side,
                    combined,
                    page_hash_by_open,
                    interval,
                    args.window_start_utc,
                    args.window_end_utc,
                    generated_at,
                    page_meta,
                )
                bar_rows.extend(rows)
                bar_coverage.append(coverage)
            except Exception as exc:  # noqa: BLE001 - manifest records per-side source failures.
                errors.append(
                    {
                        "source_family": "info.candleSnapshot",
                        "trade_coin": side["trade_coin"],
                        "interval": interval,
                        "error": repr(exc),
                    }
                )

    event_artifact = write_jsonl(
        local_path(scratch_root, "staged", "v1", "table=prediction_market_events", f"run={run_id}", "part-000000.jsonl"),
        event_rows,
    )
    outcome_artifact = write_jsonl(
        local_path(scratch_root, "staged", "v1", "table=prediction_market_outcomes", f"run={run_id}", "part-000000.jsonl"),
        side_rows,
    )
    question_artifact = write_jsonl(
        local_path(scratch_root, "staged", "v1", "table=prediction_market_questions", f"run={run_id}", "part-000000.jsonl"),
        question_rows,
    )
    l2_artifact = write_jsonl(
        local_path(scratch_root, "staged", "v1", "table=order_book_snapshots_fixed_depth", f"run={run_id}", "part-000000.jsonl"),
        l2_rows,
    )
    trade_artifact = write_jsonl(
        local_path(scratch_root, "staged", "v1", "table=trades_recent", f"run={run_id}", "part-000000.jsonl"),
        trade_rows,
    )
    bar_artifact = write_jsonl(
        local_path(scratch_root, "staged", "v1", "table=bars", f"run={run_id}", "part-000000.jsonl"),
        bar_rows,
    )
    coverage_artifact = write_json(
        local_path(scratch_root, "source-proof", "v1", f"run={run_id}", "current-coverage.json"),
        {"trade_coverage": trade_coverage, "bar_coverage": bar_coverage},
    )

    source_proof = {
        "schema_version": "hyperliquid-hip4-source-proof.v1",
        "run_id": run_id,
        "generated_at": generated_at,
        "venue": VENUE,
        "product_family": PRODUCT_FAMILY,
        "taxonomy": PRODUCT_FAMILY,
        "requested_window": {"start_utc": args.window_start_utc, "end_utc": args.window_end_utc},
        "official_source_docs": SOURCE_DOCS,
        "official_api_url": INFO_URL,
        "asset_encoding_statement": "encoding = 10 * outcome + side; trade coin #<encoding>; token name +<encoding>; asset id 100000000 + encoding",
        "source_families_uploaded": [
            "info.outcomeMeta",
            "info.l2Book",
            "info.recentTrades",
            "info.candleSnapshot",
        ],
        "raw_payload_records": raw_records,
        "outcome_count": len(event_rows),
        "question_count": len(question_rows),
        "outcome_side_count": len(side_rows),
        "quote_tokens_observed": sorted({str(row["quote_token"]) for row in side_rows if row.get("quote_token")}),
        "l2_snapshot_count": len(l2_rows),
        "recent_trade_row_count": len(trade_rows),
        "bar_row_count": len(bar_rows),
        "candle_intervals": intervals,
        "candle_pages_total": sum(c["pages"] for c in bar_coverage),
        "candle_truncated_series": [
            {"trade_coin": c["trade_coin"], "interval": c["interval"], "pages": c["pages"]}
            for c in bar_coverage
            if c["truncated"]
        ],
        "trade_coverage": trade_coverage,
        "bar_coverage": bar_coverage,
        "warnings": warnings,
        "gaps_and_unproven_families": [
            "historical outcome metadata coverage is not proven; outcomeMeta is current active metadata only",
            "quoteToken is preserved from the official source payload, but downstream quote-token parser fidelity remains pending",
            "one-year outcome level-two replay is not uploaded or claimed",
            "one-year outcome trade replay is not uploaded or claimed",
            "recentTrades payloads are current recent responses only (no info-endpoint path to deeper trade history)",
            "candleSnapshot is paged across the full requested window (<=5000 candles per page, deduped by open time); any series that hit --max-candle-pages is listed in candle_truncated_series",
            "official archive outcome filename coverage is not proven in this run",
            "order book deltas and settlement-history families are not uploaded",
        ],
    }
    source_proof_artifact = write_json(
        local_path(scratch_root, "source-proof", "v1", f"run={run_id}", "source-proof.json"),
        source_proof,
    )

    local_artifacts: list[tuple[pathlib.Path, str]] = []
    for record in raw_records:
        local = pathlib.Path(record["local_path"])
        local_artifacts.append((local, s3_uri(s3_prefix, *local.relative_to(scratch_root).parts)))

    staged_artifacts = [
        ("prediction_market_events", event_artifact),
        ("prediction_market_outcomes", outcome_artifact),
        ("prediction_market_questions", question_artifact),
        ("order_book_snapshots_fixed_depth", l2_artifact),
        ("trades_recent", trade_artifact),
        ("bars", bar_artifact),
    ]
    for _, artifact in staged_artifacts:
        local = pathlib.Path(artifact["path"])
        local_artifacts.append((local, s3_uri(s3_prefix, *local.relative_to(scratch_root).parts)))
    for artifact in [coverage_artifact, source_proof_artifact]:
        local = pathlib.Path(artifact["path"])
        local_artifacts.append((local, s3_uri(s3_prefix, *local.relative_to(scratch_root).parts)))

    for local, target in local_artifacts:
        uploads.append(upload_record(local, target))

    manifest_s3 = s3_uri(s3_prefix, "manifests", "v1", f"run={run_id}", "manifest.json")
    manifest = {
        "schema_version": "hyperliquid-hip4-s3-staging-manifest.v1",
        "run_id": run_id,
        "generated_at": generated_at,
        "completed_at": utc_now(),
        "venue": VENUE,
        "product_family": PRODUCT_FAMILY,
        "taxonomy": PRODUCT_FAMILY,
        "s3_prefix": s3_prefix + "/",
        "canonical_s3_write": False,
        "write_mode": "s3_staging",
        "requested_window": {"start_utc": args.window_start_utc, "end_utc": args.window_end_utc},
        "candle_intervals": intervals,
        "max_candle_pages": args.max_candle_pages,
        "source_families_uploaded": source_proof["source_families_uploaded"],
        "counts": {
            "outcome_records": len(event_rows),
            "question_records": len(question_rows),
            "outcome_side_records": len(side_rows),
            "l2_snapshot_records": len(l2_rows),
            "recent_trade_records": len(trade_rows),
            "bar_records": len(bar_rows),
            "candle_intervals": len(intervals),
            "candle_pages": sum(c["pages"] for c in bar_coverage),
            "candle_truncated_series": sum(1 for c in bar_coverage if c["truncated"]),
            "raw_payloads": len(raw_records),
            "uploaded_objects_without_manifest": len(uploads),
            "errors": len(errors),
        },
        "bytes": {
            "raw_payload_bytes": sum(record["bytes"] for record in raw_records),
            "staged_prediction_market_events_bytes": event_artifact["bytes"],
            "staged_prediction_market_outcomes_bytes": outcome_artifact["bytes"],
            "staged_prediction_market_questions_bytes": question_artifact["bytes"],
            "staged_order_book_snapshots_fixed_depth_bytes": l2_artifact["bytes"],
            "staged_trades_recent_bytes": trade_artifact["bytes"],
            "staged_bars_bytes": bar_artifact["bytes"],
            "source_proof_bytes": source_proof_artifact["bytes"],
            "current_coverage_bytes": coverage_artifact["bytes"],
            "uploaded_bytes_without_manifest": sum(record["bytes"] for record in uploads),
        },
        "uploads": uploads,
        "local_artifacts_root": str(scratch_root),
        "manifest_s3_uri": manifest_s3,
        "source_proof_s3_uri": s3_uri(s3_prefix, "source-proof", "v1", f"run={run_id}", "source-proof.json"),
        "current_coverage_s3_uri": s3_uri(s3_prefix, "source-proof", "v1", f"run={run_id}", "current-coverage.json"),
        "staged_s3_uris": {
            table: s3_uri(s3_prefix, "staged", "v1", f"table={table}", f"run={run_id}", "part-000000.jsonl")
            for table, _ in staged_artifacts
        },
        "gaps_and_unproven_families": source_proof["gaps_and_unproven_families"],
        "warnings": warnings,
        "errors": errors,
        "commands_or_checks_run": [
            "python3 scripts/backfill_hyperliquid_hip4_to_s3.py --help",
            "official API POST /info type=outcomeMeta",
            "official API POST /info type=l2Book for each active outcome side",
            "official API POST /info type=recentTrades for each active outcome side",
            f"official API POST /info type=candleSnapshot paged across the full window for intervals={','.join(intervals)} for each active outcome side",
            "aws s3 cp staged raw/source-proof/staged artifacts to the requested staging prefix",
        ],
        "invocation_argv": sys.argv,
    }
    manifest_without_hash = dict(manifest)
    manifest["manifest_sha256"] = sha256_bytes(stable_json(manifest_without_hash).encode("utf-8"))
    manifest_path = local_path(scratch_root, "manifests", "v1", f"run={run_id}", "manifest.json")
    manifest_artifact = write_json(manifest_path, manifest)
    manifest_upload = upload_record(manifest_path, manifest_s3)

    summary = {
        "ok": not errors,
        "run_id": run_id,
        "s3_prefix": s3_prefix + "/",
        "manifest_s3_uri": manifest_s3,
        "manifest_sha256": manifest["manifest_sha256"],
        "manifest_object_sha256": manifest_artifact["sha256"],
        "counts": manifest["counts"],
        "bytes": manifest["bytes"] | {"manifest_bytes": manifest_upload["bytes"]},
        "source_families_uploaded": manifest["source_families_uploaded"],
        "gaps_and_unproven_families": manifest["gaps_and_unproven_families"],
        "local_artifacts_root": str(scratch_root),
    }
    print(stable_json(summary), end="")
    return 0 if not errors else 1


if __name__ == "__main__":
    raise SystemExit(main())
