#!/usr/bin/env python3
"""Stage a deterministic Bybit backfill tranche into the approved S3 prefix."""

from __future__ import annotations

import argparse
import datetime as dt
import gzip
import hashlib
import json
import os
import pathlib
import re
import subprocess
import tempfile
import time
import urllib.error
import urllib.parse
import urllib.request
from typing import Any


USER_AGENT = "bolt-v2-bybit-backfill-to-s3/1"
APPROVED_S3_PREFIX = "s3://bolt-parquet/backfill-staging/2026-06-01/bybit"
API_BASE = "https://api.bybit.com/v5/market"
PUBLIC_ARCHIVE_BASE = "https://public.bybit.com"
WINDOW_START = ""
WINDOW_END = ""
START_MS = 0
END_MS = 0
MINUTE_MS = 60_000
DAY_MS = 86_400_000


def utc_now() -> str:
    return dt.datetime.now(dt.UTC).replace(microsecond=0).isoformat().replace("+00:00", "Z")


def stable_json(value: Any) -> str:
    return json.dumps(value, indent=2, sort_keys=True, ensure_ascii=True) + "\n"


def sha256_bytes(payload: bytes) -> str:
    return hashlib.sha256(payload).hexdigest()


def normalized_s3_prefix(prefix: str) -> str:
    clean = prefix.rstrip("/")
    if clean != APPROVED_S3_PREFIX:
        raise ValueError(f"S3 prefix must be exactly {APPROVED_S3_PREFIX}")
    return clean


def ms_to_utc(ms: int) -> str:
    return dt.datetime.fromtimestamp(ms / 1000, dt.UTC).replace(microsecond=0).isoformat().replace("+00:00", "Z")


def parse_utc_ms(value: str) -> int:
    normalized = value.replace("Z", "+00:00")
    parsed = dt.datetime.fromisoformat(normalized)
    if parsed.tzinfo is None:
        raise ValueError(f"UTC timestamp must include timezone: {value}")
    return int(parsed.timestamp() * 1000)


def configure_window(start_utc: str, end_utc: str) -> None:
    global WINDOW_START, WINDOW_END, START_MS, END_MS
    START_MS = parse_utc_ms(start_utc)
    END_MS = parse_utc_ms(end_utc)
    if END_MS <= START_MS:
        raise ValueError("--window-end-utc must be later than --window-start-utc")
    WINDOW_START = ms_to_utc(START_MS)
    WINDOW_END = ms_to_utc(END_MS)


def parse_date(value: str) -> dt.date:
    return dt.date.fromisoformat(value)


def date_range(start_date: dt.date, end_date: dt.date) -> list[str]:
    if end_date < start_date:
        raise ValueError("archive end date must be on or after archive start date")
    days = (end_date - start_date).days
    return [(start_date + dt.timedelta(days=offset)).isoformat() for offset in range(days + 1)]


def safe_segment(value: str) -> str:
    return re.sub(r"[^A-Za-z0-9._=-]+", "_", value)


def transient_payload_path(root: pathlib.Path, payload_hash: str, extension: str) -> pathlib.Path:
    return root / "transient-payloads" / f"{payload_hash}.{safe_segment(extension)}"


def write_bytes(path: pathlib.Path, payload: bytes) -> str:
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_bytes(payload)
    return sha256_bytes(payload)


def request_bytes(url: str, timeout: int = 60) -> tuple[int, dict[str, str], bytes]:
    request = urllib.request.Request(url, headers={"User-Agent": USER_AGENT})
    with urllib.request.urlopen(request, timeout=timeout) as response:
        return response.status, dict(response.headers.items()), response.read()


def head_url(url: str, timeout: int = 20) -> tuple[int, dict[str, str]]:
    request = urllib.request.Request(url, method="HEAD", headers={"User-Agent": USER_AGENT})
    with urllib.request.urlopen(request, timeout=timeout) as response:
        return response.status, dict(response.headers.items())


def api_url(path: str, params: dict[str, Any]) -> str:
    return f"{API_BASE}/{path}?" + urllib.parse.urlencode({k: v for k, v in params.items() if v is not None})


def append_cursor(url: str, cursor: str) -> str:
    parsed = urllib.parse.urlparse(url)
    query = dict(urllib.parse.parse_qsl(parsed.query, keep_blank_values=True))
    query["cursor"] = cursor
    return urllib.parse.urlunparse(parsed._replace(query=urllib.parse.urlencode(query)))


def json_rows(payload: bytes) -> list[Any]:
    parsed = json.loads(payload.decode("utf-8"))
    result = parsed.get("result")
    if isinstance(result, list):
        return result
    if isinstance(result, dict):
        rows = result.get("list")
        return rows if isinstance(rows, list) else []
    return []


def response_cursor(payload: bytes) -> str | None:
    parsed = json.loads(payload.decode("utf-8"))
    result = parsed.get("result")
    if not isinstance(result, dict):
        return None
    cursor = result.get("nextPageCursor")
    return cursor if cursor else None


def fetch_paginated_json(url: str, max_pages: int | None = None) -> list[dict[str, Any]]:
    records: list[dict[str, Any]] = []
    seen: set[str] = set()
    page_url = url
    page = 1
    while True:
        status, headers, payload = request_bytes(page_url)
        parsed = json.loads(payload.decode("utf-8"))
        if parsed.get("retCode") != 0:
            raise RuntimeError(f"Bybit API error for {page_url}: {parsed.get('retCode')} {parsed.get('retMsg')}")
        records.append(
            {
                "url": page_url,
                "page": page,
                "status": status,
                "headers": headers,
                "payload": payload,
                "rows": json_rows(payload),
            }
        )
        if max_pages is not None and page >= max_pages:
            return records
        cursor = response_cursor(payload)
        if not cursor:
            return records
        if cursor in seen:
            raise RuntimeError(f"Bybit pagination loop at cursor {cursor}")
        seen.add(cursor)
        page += 1
        page_url = append_cursor(url, cursor)
        time.sleep(0.12)


def s3_raw_uri(prefix: str, source: str, family: str, attrs: dict[str, str], extension: str, payload_hash: str) -> str:
    parts = [normalized_s3_prefix(prefix), "raw/v1", f"source={source}", f"family={family}"]
    for key in sorted(attrs):
        parts.append(f"{safe_segment(key)}={safe_segment(attrs[key])}")
    parts.append(f"object={payload_hash}.{extension}")
    return "/".join(parts)


def upload_to_s3(local_path: pathlib.Path, s3_uri: str) -> None:
    subprocess.run(["aws", "s3", "cp", str(local_path), s3_uri, "--only-show-errors"], check=True)


def upload_payload(
    *,
    local_root: pathlib.Path,
    s3_prefix: str,
    source: str,
    family: str,
    attrs: dict[str, str],
    extension: str,
    payload: bytes,
    source_url: str,
    http_status: int,
    headers: dict[str, str],
    extra: dict[str, Any] | None = None,
) -> dict[str, Any]:
    payload_hash = sha256_bytes(payload)
    s3_uri = s3_raw_uri(s3_prefix, source, family, attrs, extension, payload_hash)
    local_path = transient_payload_path(local_root, payload_hash, extension)
    write_bytes(local_path, payload)
    try:
        upload_to_s3(local_path, s3_uri)
    finally:
        local_path.unlink(missing_ok=True)
    record: dict[str, Any] = {
        "source": source,
        "family": family,
        "attrs": attrs,
        "source_url": source_url,
        "http_status": http_status,
        "content_type": headers.get("Content-Type") or headers.get("content-type"),
        "bytes": len(payload),
        "sha256": payload_hash,
        "local_retention": "deleted_after_s3_upload",
        "s3_uri": s3_uri,
    }
    if extra:
        record.update(extra)
    return record


def collect_instrument_universe(
    local_root: pathlib.Path,
    s3_prefix: str,
    option_base_coins: set[str] | None,
    errors: list[dict[str, str]],
) -> tuple[list[dict[str, Any]], dict[str, list[dict[str, Any]]]]:
    payload_records: list[dict[str, Any]] = []
    universes: dict[str, list[dict[str, Any]]] = {"spot": [], "linear": [], "inverse": [], "option": []}
    option_requests = sorted(option_base_coins) if option_base_coins else ["BTC", "ETH", "SOL"]
    requests: list[tuple[str, str | None]] = [
        ("spot", None),
        ("linear", None),
        ("inverse", None),
    ]
    requests.extend(("option", base_coin) for base_coin in option_requests)
    for category, base_coin in requests:
        url = api_url("instruments-info", {"category": category, "limit": 1000, "baseCoin": base_coin})
        try:
            pages = fetch_paginated_json(url)
        except (urllib.error.URLError, TimeoutError, OSError, RuntimeError, json.JSONDecodeError) as exc:
            errors.append(
                {
                    "scope": "instrument_universe",
                    "category": category,
                    "baseCoin": base_coin or "all",
                    "error": repr(exc),
                }
            )
            continue
        rows_for_request: list[dict[str, Any]] = []
        for page in pages:
            rows = [row for row in page["rows"] if isinstance(row, dict)]
            rows_for_request.extend(rows)
            payload_records.append(
                upload_payload(
                    local_root=local_root,
                    s3_prefix=s3_prefix,
                    source="rest",
                    family="instrument_universe",
                    attrs={
                        "category": category,
                        "baseCoin": base_coin or "all",
                        "page": str(page["page"]),
                    },
                    extension="json",
                    payload=page["payload"],
                    source_url=page["url"],
                    http_status=page["status"],
                    headers=page["headers"],
                    extra={
                        "row_count": len(rows),
                        "official_endpoint": "/v5/market/instruments-info",
                        "pagination": "nextPageCursor",
                    },
                )
            )
        universes[category].extend(rows_for_request)
        time.sleep(0.12)
    return payload_records, universes


def launched_before_window(row: dict[str, Any]) -> bool:
    try:
        launch = int(row.get("launchTime") or 0)
    except (TypeError, ValueError):
        return False
    return launch <= END_MS


def archive_url(category: str, symbol: str, archive_date: str) -> str:
    if category == "spot":
        return f"{PUBLIC_ARCHIVE_BASE}/spot/{symbol}/{symbol}_{archive_date}.csv.gz"
    return f"{PUBLIC_ARCHIVE_BASE}/trading/{symbol}/{symbol}{archive_date}.csv.gz"


def parse_csv_set(value: str | None) -> set[str] | None:
    if not value:
        return None
    return {item.strip().upper() for item in value.split(",") if item.strip()}


def row_matches_base_ticker(row: dict[str, Any], tickers: set[str] | None) -> bool:
    if not tickers:
        return True
    base_coin = (row.get("baseCoin") or row.get("baseCoinName") or "").upper()
    if base_coin in tickers:
        return True
    symbol = (row.get("symbol") or "").upper()
    return any(
        symbol == ticker
        or symbol.startswith(f"{ticker}US")
        or symbol.startswith(f"{ticker}-")
        for ticker in tickers
    )


def filter_universes_by_base_tickers(
    universes: dict[str, list[dict[str, Any]]],
    tickers: set[str] | None,
) -> dict[str, list[dict[str, Any]]]:
    if not tickers:
        return universes
    return {
        category: [row for row in rows if row_matches_base_ticker(row, tickers)]
        for category, rows in universes.items()
    }


def choose_archive_symbols(
    category: str,
    rows: list[dict[str, Any]],
    archive_date: str,
    symbol_limit: int,
) -> tuple[list[tuple[str, str, dict[str, str]]], dict[str, Any]]:
    candidates = sorted({row["symbol"] for row in rows if row.get("symbol") and launched_before_window(row)})
    last_error = ""
    selected: list[tuple[str, str, dict[str, str]]] = []
    for symbol in candidates:
        url = archive_url(category, symbol, archive_date)
        try:
            status, headers = head_url(url)
            if status == 200:
                selected.append((symbol, url, headers))
                if len(selected) >= symbol_limit:
                    break
        except (urllib.error.URLError, TimeoutError, OSError) as exc:
            last_error = repr(exc)
    if not selected:
        raise RuntimeError(f"No archive object found for {category} on {archive_date}; candidates={len(candidates)} last_error={last_error}")
    return selected, {
        "category": category,
        "candidate_count": len(candidates),
        "selected_count": len(selected),
        "selection_rule": "launched_before_window_end_with_existing_public_archive_head_200",
        "symbol_limit": symbol_limit,
        "last_error": last_error,
    }


def archive_schema_sample(path: pathlib.Path) -> dict[str, Any]:
    with gzip.open(path, "rt", encoding="utf-8", errors="replace", newline="") as handle:
        header = handle.readline().strip()
        first_row = handle.readline().strip()
    return {
        "header": header,
        "header_columns": header.split(",") if header else [],
        "first_data_row_sha256": sha256_bytes(first_row.encode("utf-8")) if first_row else None,
        "first_data_row_field_count": len(first_row.split(",")) if first_row else 0,
    }


def download_archive(local_root: pathlib.Path, url: str) -> tuple[pathlib.Path, bytes, int, dict[str, str]]:
    status, headers, payload = request_bytes(url, timeout=300)
    payload_hash = sha256_bytes(payload)
    suffix = pathlib.Path(urllib.parse.urlparse(url).path).name
    local_path = local_root / "downloaded-archives" / f"{payload_hash}-{suffix}"
    write_bytes(local_path, payload)
    return local_path, payload, status, headers


def stage_archive_tranche(
    local_root: pathlib.Path,
    s3_prefix: str,
    universes: dict[str, list[dict[str, Any]]],
    archive_dates: list[str],
    symbol_limit_per_category: int,
) -> list[dict[str, Any]]:
    records: list[dict[str, Any]] = []
    for archive_date in archive_dates:
        for category in ("spot", "linear", "inverse"):
            archive_symbols, selection_summary = choose_archive_symbols(
                category,
                universes[category],
                archive_date,
                symbol_limit_per_category,
            )
            for symbol, url, head_headers in archive_symbols:
                downloaded_path, payload, status, headers = download_archive(local_root, url)
                schema = archive_schema_sample(downloaded_path)
                records.append(
                    upload_payload(
                        local_root=local_root,
                        s3_prefix=s3_prefix,
                        source="public_archive",
                        family="tick_trades",
                        attrs={"category": category, "symbol": symbol, "dt": archive_date},
                        extension="csv.gz",
                        payload=payload,
                        source_url=url,
                        http_status=status,
                        headers=headers,
                        extra={
                            "symbol": symbol,
                            "category": category,
                            "archive_date": archive_date,
                            "schema_sample": schema,
                            "head_content_length": head_headers.get("Content-Length") or head_headers.get("content-length"),
                            "selection_summary": selection_summary,
                        },
                    )
                )
                downloaded_path.unlink(missing_ok=True)
                time.sleep(0.12)
    return records


def first_symbol(rows: list[dict[str, Any]]) -> str:
    candidates = sorted({row["symbol"] for row in rows if row.get("symbol") and launched_before_window(row)})
    if not candidates:
        raise RuntimeError("No launched-before-window symbol found")
    return candidates[0]


def first_symbol_with_kline(category: str, rows: list[dict[str, Any]]) -> str:
    candidates = sorted({row["symbol"] for row in rows if row.get("symbol") and launched_before_window(row)})
    if not candidates:
        raise RuntimeError(f"No candidate symbol found for {category}")
    for symbol in candidates:
        url = api_url(
            "kline",
            {"category": category, "symbol": symbol, "interval": "1", "start": START_MS, "end": START_MS + 999 * MINUTE_MS, "limit": 1},
        )
        try:
            _, _, payload = request_bytes(url)
            parsed = json.loads(payload.decode("utf-8"))
            if parsed.get("retCode") == 0 and json_rows(payload):
                return symbol
        except (urllib.error.URLError, TimeoutError, OSError, json.JSONDecodeError):
            continue
        time.sleep(0.05)
    return candidates[0]


def symbols_with_kline(category: str, rows: list[dict[str, Any]]) -> list[str]:
    candidates = sorted({row["symbol"] for row in rows if row.get("symbol") and launched_before_window(row)})
    selected: list[str] = []
    for symbol in candidates:
        url = api_url(
            "kline",
            {"category": category, "symbol": symbol, "interval": "1", "start": START_MS, "end": START_MS + 999 * MINUTE_MS, "limit": 1},
        )
        try:
            _, _, payload = request_bytes(url)
            parsed = json.loads(payload.decode("utf-8"))
            if parsed.get("retCode") == 0 and json_rows(payload):
                selected.append(symbol)
        except (urllib.error.URLError, TimeoutError, OSError, json.JSONDecodeError):
            continue
        time.sleep(0.05)
    return selected or candidates


def rest_schema(rows: list[Any]) -> dict[str, Any]:
    if not rows:
        return {"row_count": 0}
    first = rows[0]
    if isinstance(first, dict):
        return {"row_count": len(rows), "row_shape": "object", "keys": sorted(first.keys())}
    if isinstance(first, list):
        return {"row_count": len(rows), "row_shape": "array", "field_count": len(first)}
    return {"row_count": len(rows), "row_shape": type(first).__name__}


def row_times(rows: list[Any], field: str | None = None) -> list[int]:
    values: list[int] = []
    for row in rows:
        value: Any = None
        if isinstance(row, list) and row:
            value = row[0]
        elif isinstance(row, dict) and field:
            value = row.get(field)
        try:
            values.append(int(value))
        except (TypeError, ValueError):
            continue
    return values


def coverage_from_times(times: list[int]) -> dict[str, Any]:
    if not times:
        return {"returned_start_utc": None, "returned_end_utc": None}
    return {"returned_start_utc": ms_to_utc(min(times)), "returned_end_utc": ms_to_utc(max(times))}


def time_windows(step_ms: int) -> list[tuple[int, int]]:
    windows: list[tuple[int, int]] = []
    cursor = START_MS
    while cursor < END_MS:
        window_end = min(cursor + step_ms - 1, END_MS - 1)
        windows.append((cursor, window_end))
        cursor = window_end + 1
    return windows


def fetch_one_rest_payload(
    local_root: pathlib.Path,
    s3_prefix: str,
    *,
    path: str,
    family: str,
    category: str,
    symbol: str,
    params: dict[str, Any],
    time_field: str | None = None,
    attrs_extra: dict[str, str] | None = None,
) -> dict[str, Any]:
    url = api_url(path, params)
    status, headers, payload = request_bytes(url)
    parsed = json.loads(payload.decode("utf-8"))
    if parsed.get("retCode") != 0:
        raise RuntimeError(f"Bybit API error for {family}: {parsed.get('retCode')} {parsed.get('retMsg')}")
    rows = json_rows(payload)
    attrs = {
        "category": category,
        "symbol": symbol,
        "window_start": WINDOW_START,
        "window_end": ms_to_utc(int(params.get("end") or params.get("endTime") or END_MS)),
    }
    if attrs_extra:
        attrs.update(attrs_extra)
    return upload_payload(
        local_root=local_root,
        s3_prefix=s3_prefix,
        source="rest",
        family=family,
        attrs=attrs,
        extension="json",
        payload=payload,
        source_url=url,
        http_status=status,
        headers=headers,
        extra={
            "symbol": symbol,
            "category": category,
            "requested_params": params,
            "schema_sample": rest_schema(rows),
            "coverage": coverage_from_times(row_times(rows, time_field)),
        },
    )


def maybe_fetch_one_rest_payload(
    errors: list[dict[str, str]],
    local_root: pathlib.Path,
    s3_prefix: str,
    *,
    path: str,
    family: str,
    category: str,
    symbol: str,
    params: dict[str, Any],
    time_field: str | None = None,
    attrs_extra: dict[str, str] | None = None,
) -> dict[str, Any] | None:
    try:
        return fetch_one_rest_payload(
            local_root,
            s3_prefix,
            path=path,
            family=family,
            category=category,
            symbol=symbol,
            params=params,
            time_field=time_field,
            attrs_extra=attrs_extra,
        )
    except (urllib.error.URLError, TimeoutError, OSError, RuntimeError, json.JSONDecodeError) as exc:
        errors.append(
            {
                "scope": "rest_payload",
                "family": family,
                "category": category,
                "symbol": symbol,
                "error": repr(exc),
            }
        )
        return None


def append_if_present(records: list[dict[str, Any]], record: dict[str, Any] | None) -> None:
    if record is not None:
        records.append(record)


def stage_rest_tranche(
    local_root: pathlib.Path,
    s3_prefix: str,
    universes: dict[str, list[dict[str, Any]]],
    errors: list[dict[str, str]],
) -> list[dict[str, Any]]:
    records: list[dict[str, Any]] = []
    spot_symbols = symbols_with_kline("spot", universes["spot"])
    linear_symbols = symbols_with_kline("linear", universes["linear"])
    inverse_symbols = symbols_with_kline("inverse", universes["inverse"])
    minute_windows = time_windows(1000 * MINUTE_MS)
    daily_windows = time_windows(200 * DAY_MS)
    funding_windows = time_windows(70 * DAY_MS)
    for category, symbols in (("spot", spot_symbols), ("linear", linear_symbols), ("inverse", inverse_symbols)):
        for symbol in symbols:
            for page_start, page_end in minute_windows:
                append_if_present(
                    records,
                    maybe_fetch_one_rest_payload(
                        errors,
                        local_root,
                        s3_prefix,
                        path="kline",
                        family="kline_1m",
                        category=category,
                        symbol=symbol,
                        params={"category": category, "symbol": symbol, "interval": "1", "start": page_start, "end": page_end, "limit": 1000},
                        attrs_extra={"page_start": ms_to_utc(page_start), "page_end": ms_to_utc(page_end)},
                    ),
                )
                time.sleep(0.12)
    for category, symbols in (("linear", linear_symbols), ("inverse", inverse_symbols)):
        for symbol in symbols:
            for path, family in (("mark-price-kline", "mark_price_kline_1m"), ("index-price-kline", "index_price_kline_1m")):
                for page_start, page_end in minute_windows:
                    append_if_present(
                        records,
                        maybe_fetch_one_rest_payload(
                            errors,
                            local_root,
                            s3_prefix,
                            path=path,
                            family=family,
                            category=category,
                            symbol=symbol,
                            params={"category": category, "symbol": symbol, "interval": "1", "start": page_start, "end": page_end, "limit": 1000},
                            attrs_extra={"page_start": ms_to_utc(page_start), "page_end": ms_to_utc(page_end)},
                        ),
                    )
                    time.sleep(0.12)
            for page_start, page_end in funding_windows:
                append_if_present(
                    records,
                    maybe_fetch_one_rest_payload(
                        errors,
                        local_root,
                        s3_prefix,
                        path="funding/history",
                        family="funding_rate",
                        category=category,
                        symbol=symbol,
                        params={"category": category, "symbol": symbol, "startTime": page_start, "endTime": page_end, "limit": 200},
                        time_field="fundingRateTimestamp",
                        attrs_extra={"page_start": ms_to_utc(page_start), "page_end": ms_to_utc(page_end)},
                    ),
                )
                time.sleep(0.12)
            for page_start, page_end in daily_windows:
                append_if_present(
                    records,
                    maybe_fetch_one_rest_payload(
                        errors,
                        local_root,
                        s3_prefix,
                        path="open-interest",
                        family="open_interest_1d",
                        category=category,
                        symbol=symbol,
                        params={"category": category, "symbol": symbol, "intervalTime": "1d", "startTime": page_start, "endTime": page_end, "limit": 200},
                        time_field="timestamp",
                        attrs_extra={"intervalTime": "1d", "page_start": ms_to_utc(page_start), "page_end": ms_to_utc(page_end)},
                    ),
                )
                time.sleep(0.12)
    for symbol in linear_symbols:
        for page_start, page_end in minute_windows:
            append_if_present(
                records,
                maybe_fetch_one_rest_payload(
                    errors,
                    local_root,
                    s3_prefix,
                    path="premium-index-price-kline",
                    family="premium_index_price_kline_1m",
                    category="linear",
                    symbol=symbol,
                    params={"category": "linear", "symbol": symbol, "interval": "1", "start": page_start, "end": page_end, "limit": 1000},
                    attrs_extra={"page_start": ms_to_utc(page_start), "page_end": ms_to_utc(page_end)},
                ),
            )
            time.sleep(0.12)
    return records


def option_base_coins_from_universe(universes: dict[str, list[dict[str, Any]]]) -> list[str]:
    return sorted({row.get("baseCoin") for row in universes.get("option", []) if row.get("baseCoin")})


def option_quote_pairs_from_universe(universes: dict[str, list[dict[str, Any]]]) -> list[tuple[str, str]]:
    pairs: set[tuple[str, str]] = set()
    for row in universes.get("option", []):
        base_coin = row.get("baseCoin")
        quote_coin = row.get("quoteCoin") or row.get("settleCoin")
        if base_coin and quote_coin:
            pairs.add((base_coin, quote_coin))
    return sorted(pairs)


def stage_delivery(
    local_root: pathlib.Path,
    s3_prefix: str,
    universes: dict[str, list[dict[str, Any]]],
    errors: list[dict[str, str]],
) -> list[dict[str, Any]]:
    records: list[dict[str, Any]] = []
    requests: list[tuple[str, str | None]] = [("linear", None), ("inverse", None)]
    requests.extend(("option", base_coin) for base_coin in option_base_coins_from_universe(universes))
    for category, base_coin in requests:
        url = api_url("delivery-price", {"category": category, "baseCoin": base_coin, "limit": 200})
        try:
            pages = fetch_paginated_json(url, max_pages=1)
        except (urllib.error.URLError, TimeoutError, OSError, RuntimeError, json.JSONDecodeError) as exc:
            errors.append(
                {
                    "scope": "delivery_price",
                    "category": category,
                    "baseCoin": base_coin or "all",
                    "error": repr(exc),
                }
            )
            continue
        for page in pages[:1]:
            rows = [row for row in page["rows"] if isinstance(row, dict)]
            records.append(
                upload_payload(
                    local_root=local_root,
                    s3_prefix=s3_prefix,
                    source="rest",
                    family="delivery_price",
                    attrs={"category": category, "baseCoin": base_coin or "all", "page": str(page["page"])},
                    extension="json",
                    payload=page["payload"],
                    source_url=page["url"],
                    http_status=page["status"],
                    headers=page["headers"],
                    extra={
                        "category": category,
                        "baseCoin": base_coin,
                        "schema_sample": rest_schema(rows),
                        "coverage": coverage_from_times(row_times(rows, "deliveryTime")),
                        "pagination": "nextPageCursor",
                    },
                )
            )
        time.sleep(0.12)
    return records


def stage_historical_volatility(
    local_root: pathlib.Path,
    s3_prefix: str,
    universes: dict[str, list[dict[str, Any]]],
    errors: list[dict[str, str]],
) -> list[dict[str, Any]]:
    records: list[dict[str, Any]] = []
    hv_windows = time_windows(30 * DAY_MS)
    for base_coin, quote_coin in option_quote_pairs_from_universe(universes):
        for page_start, page_end in hv_windows:
            url = api_url(
                "historical-volatility",
                {
                    "category": "option",
                    "baseCoin": base_coin,
                    "quoteCoin": quote_coin,
                    "period": 7,
                    "startTime": page_start,
                    "endTime": page_end,
                },
            )
            try:
                status, headers, payload = request_bytes(url)
                parsed = json.loads(payload.decode("utf-8"))
                if parsed.get("retCode") != 0:
                    raise RuntimeError(f"Bybit API error for historical volatility: {parsed.get('retCode')} {parsed.get('retMsg')}")
            except (urllib.error.URLError, TimeoutError, OSError, RuntimeError, json.JSONDecodeError) as exc:
                errors.append(
                    {
                        "scope": "historical_volatility",
                        "category": "option",
                        "baseCoin": base_coin,
                        "quoteCoin": quote_coin,
                        "page_start": ms_to_utc(page_start),
                        "page_end": ms_to_utc(page_end),
                        "error": repr(exc),
                    }
                )
                continue
            rows = json_rows(payload)
            records.append(
                upload_payload(
                    local_root=local_root,
                    s3_prefix=s3_prefix,
                    source="rest",
                    family="historical_volatility",
                    attrs={
                        "category": "option",
                        "baseCoin": base_coin,
                        "quoteCoin": quote_coin,
                        "period": "7",
                        "page_start": ms_to_utc(page_start),
                        "page_end": ms_to_utc(page_end),
                    },
                    extension="json",
                    payload=payload,
                    source_url=url,
                    http_status=status,
                    headers=headers,
                    extra={
                        "category": "option",
                        "baseCoin": base_coin,
                        "quoteCoin": quote_coin,
                        "requested_params": {
                            "category": "option",
                            "baseCoin": base_coin,
                            "quoteCoin": quote_coin,
                            "period": 7,
                            "startTime": page_start,
                            "endTime": page_end,
                        },
                        "schema_sample": rest_schema(rows),
                        "coverage": coverage_from_times(row_times(rows, "time")),
                    },
                )
            )
            time.sleep(0.12)
    return records


def universe_summary(universes: dict[str, list[dict[str, Any]]]) -> dict[str, Any]:
    summary: dict[str, Any] = {}
    for category, rows in universes.items():
        symbols = sorted({row.get("symbol") for row in rows if row.get("symbol")})
        summary[category] = {
            "count": len(symbols),
            "sample": symbols[:10],
        }
        if category == "option":
            summary[category]["baseCoin_counts"] = {
                base: sum(1 for row in rows if row.get("baseCoin") == base) for base in sorted({row.get("baseCoin") for row in rows if row.get("baseCoin")})
            }
    return summary


def finalized_manifest_payload(manifest: dict[str, Any]) -> bytes:
    manifest["manifest_hash_scope"] = "manifest_without_manifest_hash"
    manifest.setdefault("manifest_hash", "")
    manifest.setdefault("manifest_bytes", 0)
    manifest.setdefault("total_s3_bytes_including_manifest", manifest["bytes_excluding_manifest"])
    for _ in range(10):
        hash_scope = {key: value for key, value in manifest.items() if key != "manifest_hash"}
        candidate_hash = sha256_bytes(stable_json(hash_scope).encode("utf-8"))
        candidate = dict(manifest)
        candidate["manifest_hash"] = candidate_hash
        payload = stable_json(candidate).encode("utf-8")
        candidate["manifest_bytes"] = len(payload)
        candidate["total_s3_bytes_including_manifest"] = candidate["bytes_excluding_manifest"] + len(payload)
        if (
            manifest.get("manifest_hash") == candidate["manifest_hash"]
            and manifest.get("manifest_bytes") == candidate["manifest_bytes"]
            and manifest.get("total_s3_bytes_including_manifest") == candidate["total_s3_bytes_including_manifest"]
        ):
            manifest.update(candidate)
            return payload
        manifest.update(candidate)
    return stable_json(manifest).encode("utf-8")


def write_manifest(local_root: pathlib.Path, manifest: dict[str, Any]) -> pathlib.Path:
    path = local_root / "ingest-manifests" / "v1" / f"run={manifest['run_id']}" / "bybit-backfill-manifest.json"
    payload = finalized_manifest_payload(manifest)
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_bytes(payload)
    return path


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--s3-prefix", default=APPROVED_S3_PREFIX)
    parser.add_argument("--scratch-root", type=pathlib.Path)
    parser.add_argument("--window-start-utc", required=True)
    parser.add_argument("--window-end-utc", required=True)
    parser.add_argument("--archive-date", default="2025-06-01")
    parser.add_argument("--archive-start-date")
    parser.add_argument("--archive-end-date")
    parser.add_argument("--archive-symbol-limit-per-category", type=int, default=1)
    parser.add_argument("--base-ticker-filter", help="Comma-separated base tickers, for example BTC,ETH,SOL.")
    parser.add_argument("--skip-archive", action="store_true")
    parser.add_argument("--skip-rest", action="store_true")
    parser.add_argument("--skip-delivery", action="store_true")
    parser.add_argument("--skip-historical-volatility", action="store_true")
    return parser.parse_args()


def main() -> int:
    args = parse_args()
    configure_window(args.window_start_utc, args.window_end_utc)
    s3_prefix = normalized_s3_prefix(args.s3_prefix)
    archive_dates = (
        date_range(parse_date(args.archive_start_date), parse_date(args.archive_end_date))
        if args.archive_start_date and args.archive_end_date
        else [args.archive_date]
    )
    generated_at = utc_now()
    run_seed = stable_json(
        {
            "archive_dates": archive_dates,
            "archive_symbol_limit_per_category": args.archive_symbol_limit_per_category,
            "base_ticker_filter": args.base_ticker_filter,
            "generated_at": generated_at,
            "s3_prefix": s3_prefix,
            "skip_archive": args.skip_archive,
            "skip_delivery": args.skip_delivery,
            "skip_historical_volatility": args.skip_historical_volatility,
            "skip_rest": args.skip_rest,
            "window_end_utc": WINDOW_END,
            "window_start_utc": WINDOW_START,
        }
    )
    run_id = "bybit-backfill-run-" + sha256_bytes(run_seed.encode("utf-8"))[:16]
    local_root = args.scratch_root or pathlib.Path(f"/private/tmp/bolt-v2-bybit-backfill-{run_id}")
    if not str(local_root).startswith("/private/tmp/bolt-v2-bybit-backfill-"):
        raise ValueError("--scratch-root must be under /private/tmp/bolt-v2-bybit-backfill-*")
    local_root.mkdir(parents=True, exist_ok=True)

    records: list[dict[str, Any]] = []
    errors: list[dict[str, str]] = []
    universes: dict[str, list[dict[str, Any]]] = {"spot": [], "linear": [], "inverse": [], "option": []}
    try:
        base_tickers = parse_csv_set(args.base_ticker_filter)
        universe_records, universes = collect_instrument_universe(local_root, s3_prefix, base_tickers, errors)
        records.extend(universe_records)
        universes = filter_universes_by_base_tickers(universes, base_tickers)
        if not args.skip_archive:
            records.extend(stage_archive_tranche(local_root, s3_prefix, universes, archive_dates, args.archive_symbol_limit_per_category))
        if not args.skip_rest:
            records.extend(stage_rest_tranche(local_root, s3_prefix, universes, errors))
        if not args.skip_delivery:
            records.extend(stage_delivery(local_root, s3_prefix, universes, errors))
        if not args.skip_historical_volatility:
            records.extend(stage_historical_volatility(local_root, s3_prefix, universes, errors))
    except (urllib.error.URLError, TimeoutError, OSError, subprocess.CalledProcessError, RuntimeError, ValueError, json.JSONDecodeError) as exc:
        errors.append({"error": repr(exc)})

    uploaded_bytes = sum(record["bytes"] for record in records)
    completed_at = utc_now()
    manifest: dict[str, Any] = {
        "schema_version": "bybit-backfill-s3-manifest.v1",
        "run_id": run_id,
        "generated_at": generated_at,
        "completed_at": completed_at,
        "venue": "bybit",
        "s3_prefix": s3_prefix,
        "canonical_s3_write": False,
        "write_mode": "s3_staging_only",
        "requested_window": {"start_utc": WINDOW_START, "end_utc": WINDOW_END},
        "execution_selection": {
            "archive": not args.skip_archive,
            "delivery": not args.skip_delivery,
            "historical_volatility": not args.skip_historical_volatility,
            "rest": not args.skip_rest,
        },
        "local_root": str(local_root),
        "official_source_evidence": {
            "instruments_info": "https://bybit-exchange.github.io/docs/v5/market/instrument",
            "kline": "https://bybit-exchange.github.io/docs/v5/market/kline",
            "mark_price_kline": "https://bybit-exchange.github.io/docs/v5/market/mark-kline",
            "index_price_kline": "https://bybit-exchange.github.io/docs/api-explorer/v5/market/index-kline",
            "premium_index_price_kline": "https://bybit-exchange.github.io/docs/v5/market/premium-index-kline",
            "funding_history": "https://bybit-exchange.github.io/docs/v5/market/history-fund-rate",
            "open_interest": "https://bybit-exchange.github.io/docs/v5/market/open-interest",
            "delivery_price": "https://bybit-exchange.github.io/docs/v5/market/delivery-price",
            "historical_volatility": "https://bybit-exchange.github.io/docs/v5/market/iv",
            "public_archive_root": "https://public.bybit.com/",
        },
        "universe_summary": universe_summary(universes),
        "payload_records": records,
        "object_count_excluding_manifest": len(records),
        "bytes_excluding_manifest": uploaded_bytes,
        "source_families_uploaded": sorted({record["family"] for record in records}),
        "remaining_work": [
            "Full all-symbol archive tick trade staging remains outside this one-off ticker-set job.",
            "Full REST pagination beyond the first endpoint page per source family remains outside this ASAP tranche.",
            "Historical L2 deltas, liquidations, and options trade archives were not staged because this run did not prove exact official URL and schema coverage.",
            "Expired or delisted historical instrument universe coverage was not proven beyond instruments returned by the current public instruments-info endpoint.",
            "Option delivery-price calls are only claimed for option base coins present in the filtered instruments-info universe.",
        ],
        "errors": errors,
    }
    manifest_s3_uri = f"{s3_prefix}/ingest-manifests/v1/run={run_id}/bybit-backfill-manifest.json"
    manifest["manifest_s3_uri"] = manifest_s3_uri
    manifest["total_s3_object_count_including_manifest"] = len(records) + 1
    manifest_path = write_manifest(local_root, manifest)
    upload_to_s3(manifest_path, manifest_s3_uri)

    print(
        stable_json(
            {
                "ok": not errors,
                "run_id": run_id,
                "local_manifest": str(manifest_path),
                "manifest_s3_uri": manifest_s3_uri,
                "object_count_excluding_manifest": len(records),
                "total_s3_object_count_including_manifest": len(records) + 1,
                "bytes_excluding_manifest": uploaded_bytes,
                "total_s3_bytes_including_manifest": manifest["total_s3_bytes_including_manifest"],
                "source_families_uploaded": manifest["source_families_uploaded"],
                "errors": errors,
            }
        ),
        end="",
    )
    return 0 if not errors else 1


if __name__ == "__main__":
    raise SystemExit(main())
