#!/usr/bin/env python3
"""Stage a bounded Deribit public REST backfill tranche into S3 staging."""

from __future__ import annotations

import argparse
import collections
import concurrent.futures
import datetime as dt
import hashlib
import json
import pathlib
import re
import subprocess
import sys
import time
import urllib.error
import urllib.parse
import urllib.request
from typing import Any


APPROVED_S3_PREFIX = "s3://bolt-parquet/backfill-staging/2026-06-01/deribit"
DEFAULT_START_DATE = "2026-03-01"
DEFAULT_END_DATE = "2026-06-01"
DEFAULT_BASE_CURRENCIES = ("BTC", "ETH", "SOL", "XRP", "DOGE", "HYPE", "BNB")
DERIBIT_API_BASE = "https://www.deribit.com/api/v2/public"
USER_AGENT = "bolt-v2-deribit-3m-backfill-to-s3/1"
DEFAULT_HTTP_RETRIES = 8
DEFAULT_REQUEST_DELAY_SECONDS = 0.0
DEFAULT_PROGRESS_EVERY = 500
HTTP_RETRIES = DEFAULT_HTTP_RETRIES
REQUEST_DELAY_SECONDS = DEFAULT_REQUEST_DELAY_SECONDS
PROGRESS_EVERY = DEFAULT_PROGRESS_EVERY
MINUTE_MS = 60_000
HOUR_MS = 3_600_000
DAY_MS = 86_400_000


def utc_now() -> str:
    return dt.datetime.now(dt.UTC).replace(microsecond=0).isoformat().replace("+00:00", "Z")


def stable_json(value: Any) -> str:
    return json.dumps(value, indent=2, sort_keys=True, ensure_ascii=True) + "\n"


def sha256_bytes(payload: bytes) -> str:
    return hashlib.sha256(payload).hexdigest()


def normalize_s3_prefix(prefix: str) -> str:
    normalized = prefix.rstrip("/")
    if normalized != APPROVED_S3_PREFIX:
        raise ValueError(f"S3 prefix must be exactly {APPROVED_S3_PREFIX}")
    return normalized


def parse_date_ms(value: str) -> int:
    parsed = dt.datetime.strptime(value, "%Y-%m-%d").replace(tzinfo=dt.UTC)
    return int(parsed.timestamp() * 1000)


def ms_to_utc(value: int) -> str:
    return dt.datetime.fromtimestamp(value / 1000, dt.UTC).replace(microsecond=0).isoformat().replace("+00:00", "Z")


def safe_segment(value: str) -> str:
    cleaned = re.sub(r"[^A-Za-z0-9._=-]+", "_", value)
    if not cleaned:
        raise ValueError("empty path segment")
    return cleaned


def api_url(method: str, params: dict[str, Any] | None = None) -> str:
    query = urllib.parse.urlencode({k: v for k, v in (params or {}).items() if v is not None})
    return f"{DERIBIT_API_BASE}/{method}" + (f"?{query}" if query else "")


def retry_after_seconds(headers: Any, fallback: float) -> float:
    raw = headers.get("Retry-After") if headers else None
    try:
        return max(float(raw), fallback)
    except (TypeError, ValueError):
        return fallback


def request_bytes(url: str, *, retries: int | None = None, timeout: int = 120) -> tuple[int, dict[str, str], bytes]:
    max_retries = retries if retries is not None else HTTP_RETRIES
    for attempt in range(max_retries):
        request = urllib.request.Request(url, headers={"User-Agent": USER_AGENT})
        try:
            with urllib.request.urlopen(request, timeout=timeout) as response:
                payload = response.read()
                if REQUEST_DELAY_SECONDS > 0:
                    time.sleep(REQUEST_DELAY_SECONDS)
                return response.status, dict(response.headers.items()), payload
        except urllib.error.HTTPError as exc:
            if exc.code not in (429, 503) or attempt + 1 == max_retries:
                raise
            fallback = min(1.0 + attempt * 2.0, 30.0)
            time.sleep(retry_after_seconds(exc.headers, fallback))
        except (urllib.error.URLError, TimeoutError, OSError):
            if attempt + 1 == max_retries:
                raise
            time.sleep(min(0.5 + attempt * 1.5, 8.0))
    raise RuntimeError("unreachable retry state")


def request_json(method: str, params: dict[str, Any] | None = None) -> tuple[str, int, dict[str, str], bytes, dict[str, Any]]:
    url = api_url(method, params)
    status, headers, payload = request_bytes(url)
    parsed = json.loads(payload.decode("utf-8"))
    if "error" in parsed:
        raise RuntimeError(f"Deribit API error for {method}: {parsed['error']}")
    return url, status, headers, payload, parsed


def result_rows(parsed: dict[str, Any]) -> list[Any]:
    result = parsed.get("result")
    if isinstance(result, list):
        return result
    if isinstance(result, dict):
        for key in ("trades", "data", "settlements"):
            rows = result.get(key)
            if isinstance(rows, list):
                return rows
    return []


def row_times(rows: list[Any], keys: tuple[str, ...] = ("timestamp", "tick", "time")) -> list[int]:
    values: list[int] = []
    for row in rows:
        candidate: Any = None
        if isinstance(row, list) and row:
            candidate = row[0]
        elif isinstance(row, dict):
            for key in keys:
                if row.get(key) is not None:
                    candidate = row[key]
                    break
        try:
            values.append(int(candidate))
        except (TypeError, ValueError):
            pass
    return values


def schema_sample(rows: list[Any]) -> dict[str, Any]:
    if not rows:
        return {"row_count": 0}
    first = rows[0]
    if isinstance(first, dict):
        return {"row_count": len(rows), "row_shape": "object", "keys": sorted(first.keys())}
    if isinstance(first, list):
        return {"row_count": len(rows), "row_shape": "array", "field_count": len(first)}
    return {"row_count": len(rows), "row_shape": type(first).__name__}


def coverage_from_rows(rows: list[Any]) -> dict[str, Any]:
    times = row_times(rows)
    if not times:
        return {"returned_start_utc": None, "returned_end_utc": None}
    return {"returned_start_utc": ms_to_utc(min(times)), "returned_end_utc": ms_to_utc(max(times))}


def s3_payload_uri(
    s3_prefix: str,
    *,
    run_id: str,
    family: str,
    attrs: dict[str, str],
    extension: str,
    payload_hash: str,
) -> str:
    parts = [normalize_s3_prefix(s3_prefix), "raw", "v1", f"run={run_id}", f"family={safe_segment(family)}"]
    for key in sorted(attrs):
        parts.append(f"{safe_segment(key)}={safe_segment(attrs[key])}")
    parts.append(f"object={payload_hash}.{extension}")
    return "/".join(parts)


def local_path_for_s3(scratch_root: pathlib.Path, s3_uri: str) -> pathlib.Path:
    parsed = urllib.parse.urlparse(s3_uri)
    return scratch_root / "uploaded-payloads" / parsed.netloc / parsed.path.lstrip("/")


def write_bytes(path: pathlib.Path, payload: bytes) -> dict[str, Any]:
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_bytes(payload)
    return {"local_path": str(path), "bytes": len(payload), "sha256": sha256_bytes(payload)}


def upload_to_s3(local_path: pathlib.Path, s3_uri: str) -> None:
    subprocess.run(["aws", "s3", "cp", str(local_path), s3_uri, "--only-show-errors"], check=True)


def upload_run_payloads_to_s3(scratch_root: pathlib.Path, s3_prefix: str, run_id: str) -> None:
    s3_run_prefix = f"{normalize_s3_prefix(s3_prefix)}/raw/v1/run={run_id}"
    local_run_root = local_path_for_s3(scratch_root, s3_run_prefix)
    if not local_run_root.exists():
        return
    subprocess.run(["aws", "s3", "cp", str(local_run_root), s3_run_prefix, "--recursive", "--only-show-errors"], check=True)


def upload_payload(
    *,
    scratch_root: pathlib.Path,
    s3_prefix: str,
    run_id: str,
    family: str,
    attrs: dict[str, str],
    source_url: str,
    http_status: int,
    headers: dict[str, str],
    payload: bytes,
    extension: str = "json",
    extra: dict[str, Any] | None = None,
) -> dict[str, Any]:
    payload_hash = sha256_bytes(payload)
    s3_uri = s3_payload_uri(s3_prefix, run_id=run_id, family=family, attrs=attrs, extension=extension, payload_hash=payload_hash)
    local_path = local_path_for_s3(scratch_root, s3_uri)
    write_bytes(local_path, payload)
    record: dict[str, Any] = {
        "family": family,
        "attrs": attrs,
        "source_url": source_url,
        "http_status": http_status,
        "content_type": headers.get("Content-Type") or headers.get("content-type"),
        "bytes": len(payload),
        "sha256": payload_hash,
        "local_path": str(local_path),
        "s3_uri": s3_uri,
    }
    if extra:
        record.update(extra)
    return record


def upload_deribit_json(
    *,
    scratch_root: pathlib.Path,
    s3_prefix: str,
    run_id: str,
    method: str,
    params: dict[str, Any] | None,
    family: str,
    attrs: dict[str, str],
    extra: dict[str, Any] | None = None,
) -> tuple[dict[str, Any], dict[str, Any]]:
    url, status, headers, payload, parsed = request_json(method, params)
    rows = result_rows(parsed)
    record_extra = {
        "official_endpoint": f"/api/v2/public/{method}",
        "requested_params": params or {},
        "schema_sample": schema_sample(rows),
        "coverage": coverage_from_rows(rows),
    }
    if extra:
        record_extra.update(extra)
    return (
        upload_payload(
            scratch_root=scratch_root,
            s3_prefix=s3_prefix,
            run_id=run_id,
            family=family,
            attrs=attrs,
            source_url=url,
            http_status=status,
            headers=headers,
            payload=payload,
            extra=record_extra,
        ),
        parsed,
    )


def collect_universe(
    scratch_root: pathlib.Path,
    s3_prefix: str,
    run_id: str,
    currencies: list[str],
) -> tuple[list[dict[str, Any]], dict[str, list[dict[str, Any]]]]:
    records: list[dict[str, Any]] = []
    universe: dict[str, list[dict[str, Any]]] = {"spot": [], "perpetual": [], "future": [], "option": []}
    requested_bases = set(currencies)
    seen: dict[str, set[str]] = {family: set() for family in universe}
    for currency in currencies:
        for kind in ("spot", "future", "option"):
            for expired in (False, True):
                try:
                    record, parsed = upload_deribit_json(
                        scratch_root=scratch_root,
                        s3_prefix=s3_prefix,
                        run_id=run_id,
                        method="get_instruments",
                        params={"currency": currency, "kind": kind, "expired": str(expired).lower()},
                        family="instrument_universe",
                        attrs={"currency": currency, "kind": kind, "expired": str(expired).lower()},
                    )
                    records.append(record)
                    rows = [row for row in result_rows(parsed) if isinstance(row, dict)]
                    for row in rows:
                        if instrument_base(row) not in requested_bases:
                            continue
                        if kind == "future" and row.get("settlement_period") == "perpetual":
                            family = "perpetual"
                        elif kind == "future":
                            family = "future"
                        else:
                            family = kind
                        instrument_name = str(row.get("instrument_name") or "")
                        if not instrument_name or instrument_name in seen[family]:
                            continue
                        seen[family].add(instrument_name)
                        universe[family].append(row)
                except Exception as exc:  # noqa: BLE001 - runner must keep collecting independent surfaces.
                    records.append(error_record("instrument_universe", {"currency": currency, "kind": kind, "expired": str(expired).lower()}, exc))
                time.sleep(0.15)
    return records, universe


def error_record(family: str, attrs: dict[str, str], exc: BaseException) -> dict[str, Any]:
    return {"family": family, "attrs": attrs, "error": repr(exc)}


def map_records(items: list[Any], worker: Any, max_workers: int) -> list[dict[str, Any]]:
    if not items:
        return []
    records: list[dict[str, Any]] = []
    if max_workers <= 1:
        iterator = (worker(item) for item in items)
        for index, record in enumerate(iterator, start=1):
            records.append(record)
            if PROGRESS_EVERY > 0 and (index % PROGRESS_EVERY == 0 or index == len(items)):
                print(f"processed {index}/{len(items)} jobs", file=sys.stderr, flush=True)
        return records
    with concurrent.futures.ThreadPoolExecutor(max_workers=max_workers) as executor:
        for index, record in enumerate(executor.map(worker, items), start=1):
            records.append(record)
            if PROGRESS_EVERY > 0 and (index % PROGRESS_EVERY == 0 or index == len(items)):
                print(f"processed {index}/{len(items)} jobs", file=sys.stderr, flush=True)
    return records


def active_in_window(row: dict[str, Any], start_ms: int, end_ms: int) -> bool:
    creation = int(row.get("creation_timestamp") or 0)
    expiration = int(row.get("expiration_timestamp") or end_ms)
    return creation <= end_ms and expiration >= start_ms


def instrument_currency(row: dict[str, Any]) -> str:
    return str(row.get("currency") or row.get("base_currency") or "").upper()


def instrument_base(row: dict[str, Any]) -> str:
    return str(row.get("base_currency") or row.get("currency") or "").upper()


def instrument_sort_key(row: dict[str, Any]) -> tuple[str, str, str, str, str, str]:
    return (
        instrument_base(row),
        str(row.get("quote_currency") or row.get("counter_currency") or ""),
        str(row.get("settlement_currency") or ""),
        str(row.get("kind") or ""),
        str(row.get("settlement_period") or ""),
        str(row.get("instrument_name") or ""),
    )


def select_instruments(
    universe: dict[str, list[dict[str, Any]]],
    start_ms: int,
    end_ms: int,
    max_per_family_per_currency: int,
) -> dict[str, list[dict[str, Any]]]:
    selected: dict[str, list[dict[str, Any]]] = {}
    for family in ("spot", "perpetual", "future", "option"):
        rows = [row for row in universe.get(family, []) if row.get("instrument_name") and active_in_window(row, start_ms, end_ms)]
        rows.sort(key=instrument_sort_key)
        selected[family] = []
        for currency in sorted({instrument_base(row) for row in rows if instrument_base(row)}):
            currency_rows = [row for row in rows if instrument_base(row) == currency]
            if max_per_family_per_currency > 0:
                currency_rows = currency_rows[:max_per_family_per_currency]
            selected[family].extend(currency_rows)
    return selected


def selected_instrument_coverage(selected: dict[str, list[dict[str, Any]]]) -> dict[str, Any]:
    coverage: dict[str, Any] = {}
    for family, rows in selected.items():
        combo_counts: collections.Counter[tuple[str, str, str, str, str, str]] = collections.Counter(
            (
                instrument_base(row),
                str(row.get("quote_currency") or row.get("counter_currency") or ""),
                str(row.get("settlement_currency") or ""),
                str(row.get("kind") or ""),
                str(row.get("settlement_period") or ""),
                str(row.get("instrument_type") or ""),
            )
            for row in rows
        )
        coverage[family] = {
            "count": len(rows),
            "base_currencies": sorted({instrument_base(row) for row in rows if instrument_base(row)}),
            "quote_currencies": sorted({str(row.get("quote_currency") or row.get("counter_currency") or "") for row in rows if row.get("quote_currency") or row.get("counter_currency")}),
            "settlement_currencies": sorted({str(row.get("settlement_currency") or "") for row in rows if row.get("settlement_currency")}),
            "settlement_periods": sorted({str(row.get("settlement_period") or "") for row in rows if row.get("settlement_period")}),
            "instrument_types": sorted({str(row.get("instrument_type") or "") for row in rows if row.get("instrument_type")}),
            "base_quote_settlement_kind": [
                {
                    "base_currency": base,
                    "quote_currency": quote,
                    "settlement_currency": settlement,
                    "kind": kind,
                    "settlement_period": settlement_period,
                    "instrument_type": instrument_type,
                    "count": count,
                }
                for (base, quote, settlement, kind, settlement_period, instrument_type), count in sorted(combo_counts.items())
            ],
        }
    return coverage


def collect_metadata(
    scratch_root: pathlib.Path,
    s3_prefix: str,
    run_id: str,
    selected: dict[str, list[dict[str, Any]]],
    request_workers: int,
) -> list[dict[str, Any]]:
    jobs = [(product_family, str(row["instrument_name"])) for product_family, rows in selected.items() for row in rows]

    def collect_one(job: tuple[str, str]) -> dict[str, Any]:
        product_family, instrument = job
        try:
            record, _ = upload_deribit_json(
                scratch_root=scratch_root,
                s3_prefix=s3_prefix,
                run_id=run_id,
                method="get_instrument",
                params={"instrument_name": instrument},
                family="instrument_metadata",
                attrs={"product_family": product_family, "instrument": instrument},
            )
            return record
        except Exception as exc:  # noqa: BLE001
            return error_record("instrument_metadata", {"product_family": product_family, "instrument": instrument}, exc)

    return map_records(jobs, collect_one, request_workers)


def collect_market_data(
    scratch_root: pathlib.Path,
    s3_prefix: str,
    run_id: str,
    selected: dict[str, list[dict[str, Any]]],
    *,
    start_ms: int,
    end_ms: int,
    max_bar_hours: int,
    max_trade_hours: int,
    request_workers: int,
) -> list[dict[str, Any]]:
    jobs: list[dict[str, Any]] = []
    bar_end = min(end_ms, start_ms + max_bar_hours * HOUR_MS)
    trade_end = min(end_ms, start_ms + max_trade_hours * HOUR_MS)
    recent_trade_start = max(start_ms, end_ms - DAY_MS)
    for product_family, rows in selected.items():
        for row in rows:
            instrument = str(row["instrument_name"])
            common_attrs = {
                "product_family": product_family,
                "instrument": instrument,
                "window_start": ms_to_utc(start_ms),
                "window_end": ms_to_utc(end_ms),
            }
            for method, family, params in (
                (
                    "get_tradingview_chart_data",
                    "bars_1m",
                    {"instrument_name": instrument, "start_timestamp": start_ms, "end_timestamp": bar_end, "resolution": "1"},
                ),
                (
                    "get_last_trades_by_instrument_and_time",
                    "trades",
                    {
                        "instrument_name": instrument,
                        "start_timestamp": start_ms,
                        "end_timestamp": trade_end,
                        "count": 1000,
                        "sorting": "asc",
                    },
                ),
            ):
                jobs.append(
                    {
                        "method": method,
                        "family": family,
                        "params": params,
                        "attrs": common_attrs | {"bounded_end": ms_to_utc(int(params["end_timestamp"]))},
                    }
                )
            if product_family in ("perpetual", "future", "option"):
                jobs.append(
                    {
                        "method": "get_last_trades_by_instrument_and_time",
                        "family": "trades_recent_probe",
                        "params": {
                            "instrument_name": instrument,
                            "start_timestamp": recent_trade_start,
                            "end_timestamp": end_ms,
                            "count": 1000,
                            "sorting": "asc",
                        },
                        "attrs": common_attrs | {"bounded_start": ms_to_utc(recent_trade_start), "bounded_end": ms_to_utc(end_ms)},
                    }
                )
            if product_family == "perpetual":
                jobs.append(
                    {
                        "method": "get_funding_rate_history",
                        "family": "funding_history",
                        "params": {"instrument_name": instrument, "start_timestamp": start_ms, "end_timestamp": end_ms},
                        "attrs": common_attrs,
                    }
                )
            jobs.append(
                {
                    "method": "ticker",
                    "family": "mark_price_history_probe",
                    "params": {"instrument_name": instrument},
                    "attrs": {"product_family": product_family, "instrument": instrument, "probe_type": "current_ticker"},
                    "extra": {"history_gap": "Deribit public ticker is a current mark/index probe, not a historical mark-price endpoint."},
                }
            )

    def collect_one(job: dict[str, Any]) -> dict[str, Any]:
        try:
            record, _ = upload_deribit_json(
                scratch_root=scratch_root,
                s3_prefix=s3_prefix,
                run_id=run_id,
                method=str(job["method"]),
                params=job["params"],
                family=str(job["family"]),
                attrs=job["attrs"],
                extra=job.get("extra"),
            )
            return record
        except Exception as exc:  # noqa: BLE001
            return error_record(str(job["family"]), job["attrs"], exc)

    return map_records(jobs, collect_one, request_workers)


def collect_reference_data(
    scratch_root: pathlib.Path,
    s3_prefix: str,
    run_id: str,
    currencies: list[str],
    *,
    start_ms: int,
    end_ms: int,
    request_workers: int,
) -> list[dict[str, Any]]:
    records: list[dict[str, Any]] = []
    try:
        record, parsed = upload_deribit_json(
            scratch_root=scratch_root,
            s3_prefix=s3_prefix,
            run_id=run_id,
            method="get_index_price_names",
            params=None,
            family="index_price_names",
            attrs={"scope": "all"},
        )
        records.append(record)
        index_names = [name for name in (parsed.get("result") or []) if isinstance(name, str)]
    except Exception as exc:  # noqa: BLE001
        records.append(error_record("index_price_names", {"scope": "all"}, exc))
        index_names = []
    requested_bases = set(currencies)
    selected_index_names = sorted(
        name
        for name in index_names
        if name.split("_", 1)[0].upper() in requested_bases
    )
    jobs: list[dict[str, Any]] = []
    for index_name in selected_index_names:
        for method, family, params in (
            ("get_index_price", "index", {"index_name": index_name}),
            ("get_delivery_prices", "delivery", {"index_name": index_name, "offset": 0, "count": 100}),
        ):
            jobs.append({"method": method, "family": family, "params": params, "attrs": {"index_name": index_name}})
    for currency in currencies:
        for method, family, params in (
            ("get_historical_volatility", "historical_volatility", {"currency": currency}),
            (
                "get_volatility_index_data",
                "volatility_index",
                {"currency": currency, "start_timestamp": start_ms, "end_timestamp": min(end_ms, start_ms + 999 * MINUTE_MS), "resolution": "1"},
            ),
        ):
            jobs.append(
                {
                    "method": method,
                    "family": family,
                    "params": params,
                    "attrs": {"currency": currency, "window_start": ms_to_utc(start_ms), "window_end": ms_to_utc(end_ms)},
                }
            )
        for settlement_type in ("settlement", "delivery", "bankruptcy"):
            jobs.append(
                {
                    "method": "get_last_settlements_by_currency",
                    "family": "settlements",
                    "params": {"currency": currency, "type": settlement_type, "count": 100},
                    "attrs": {"currency": currency, "settlement_type": settlement_type},
                }
            )

    def collect_one(job: dict[str, Any]) -> dict[str, Any]:
        try:
            record, _ = upload_deribit_json(
                scratch_root=scratch_root,
                s3_prefix=s3_prefix,
                run_id=run_id,
                method=str(job["method"]),
                params=job["params"],
                family=str(job["family"]),
                attrs=job["attrs"],
            )
            return record
        except Exception as exc:  # noqa: BLE001
            return error_record(str(job["family"]), job["attrs"], exc)

    records.extend(map_records(jobs, collect_one, request_workers))
    return records


def manifest_payload(manifest: dict[str, Any]) -> bytes:
    manifest["manifest_hash_scope"] = "manifest_without_manifest_sha256"
    manifest.setdefault("manifest_sha256", "")
    manifest.setdefault("manifest_bytes", 0)
    manifest.setdefault("total_s3_bytes_including_manifest", manifest["bytes_excluding_manifest"])
    for _ in range(10):
        without_hash = dict(manifest)
        without_hash.pop("manifest_sha256", None)
        candidate_hash = sha256_bytes(stable_json(without_hash).encode("utf-8"))
        candidate = dict(manifest)
        candidate["manifest_sha256"] = candidate_hash
        payload = stable_json(candidate).encode("utf-8")
        candidate["manifest_bytes"] = len(payload)
        candidate["total_s3_bytes_including_manifest"] = candidate["bytes_excluding_manifest"] + len(payload)
        if (
            manifest.get("manifest_sha256") == candidate["manifest_sha256"]
            and manifest.get("manifest_bytes") == candidate["manifest_bytes"]
            and manifest.get("total_s3_bytes_including_manifest") == candidate["total_s3_bytes_including_manifest"]
        ):
            manifest.update(candidate)
            return stable_json(manifest).encode("utf-8")
        manifest.update(candidate)
    return stable_json(manifest).encode("utf-8")


def write_manifest(scratch_root: pathlib.Path, manifest: dict[str, Any]) -> pathlib.Path:
    path = scratch_root / "ingest-manifests" / "v1" / f"run={manifest['run_id']}" / "deribit-backfill-manifest.json"
    write_bytes(path, manifest_payload(manifest))
    return path


def parse_csv(value: str) -> list[str]:
    return [item.strip().upper() for item in value.split(",") if item.strip()]


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--s3-prefix", default=APPROVED_S3_PREFIX)
    parser.add_argument("--start-date", default=DEFAULT_START_DATE)
    parser.add_argument("--end-date", default=DEFAULT_END_DATE)
    parser.add_argument("--currencies", default=",".join(DEFAULT_BASE_CURRENCIES))
    parser.add_argument("--scratch-root", type=pathlib.Path)
    parser.add_argument("--max-instruments-per-family", type=int, default=0, help="0 selects every source-proven instrument for each requested base.")
    parser.add_argument("--max-bar-hours", type=int, default=6)
    parser.add_argument("--max-trade-hours", type=int, default=24)
    parser.add_argument("--request-workers", type=int, default=8)
    parser.add_argument("--http-retries", type=int, default=DEFAULT_HTTP_RETRIES)
    parser.add_argument("--request-delay-seconds", type=float, default=DEFAULT_REQUEST_DELAY_SECONDS)
    parser.add_argument("--progress-every", type=int, default=DEFAULT_PROGRESS_EVERY)
    return parser.parse_args()


def main() -> int:
    args = parse_args()
    global HTTP_RETRIES, REQUEST_DELAY_SECONDS, PROGRESS_EVERY
    HTTP_RETRIES = args.http_retries
    REQUEST_DELAY_SECONDS = args.request_delay_seconds
    PROGRESS_EVERY = args.progress_every
    s3_prefix = normalize_s3_prefix(args.s3_prefix)
    start_ms = parse_date_ms(args.start_date)
    end_ms = parse_date_ms(args.end_date)
    if end_ms <= start_ms:
        raise ValueError("--end-date must be after --start-date")
    generated_at = utc_now()
    run_seed = stable_json(
        {
            "generated_at": generated_at,
            "s3_prefix": s3_prefix,
            "start_date": args.start_date,
            "end_date": args.end_date,
            "currencies": args.currencies,
            "max_instruments_per_family": args.max_instruments_per_family,
            "max_bar_hours": args.max_bar_hours,
            "max_trade_hours": args.max_trade_hours,
            "request_workers": args.request_workers,
            "http_retries": args.http_retries,
            "request_delay_seconds": args.request_delay_seconds,
            "progress_every": args.progress_every,
        }
    )
    run_id = "deribit-3m-" + sha256_bytes(run_seed.encode("utf-8"))[:16]
    scratch_root = args.scratch_root or pathlib.Path(f"/private/tmp/bolt-v2-deribit-3m-{run_id}")
    if not str(scratch_root).startswith("/private/tmp/bolt-v2-deribit-3m-"):
        raise ValueError("--scratch-root must be under /private/tmp/bolt-v2-deribit-3m-*")
    scratch_root.mkdir(parents=True, exist_ok=True)
    currencies = parse_csv(args.currencies)

    all_records: list[dict[str, Any]] = []
    universe_records, universe = collect_universe(scratch_root, s3_prefix, run_id, currencies)
    all_records.extend(universe_records)
    selected = select_instruments(universe, start_ms, end_ms, args.max_instruments_per_family)
    all_records.extend(collect_metadata(scratch_root, s3_prefix, run_id, selected, args.request_workers))
    all_records.extend(
        collect_market_data(
            scratch_root,
            s3_prefix,
            run_id,
            selected,
            start_ms=start_ms,
            end_ms=end_ms,
            max_bar_hours=args.max_bar_hours,
            max_trade_hours=args.max_trade_hours,
            request_workers=args.request_workers,
        )
    )
    all_records.extend(collect_reference_data(scratch_root, s3_prefix, run_id, currencies, start_ms=start_ms, end_ms=end_ms, request_workers=args.request_workers))

    uploaded_records = [record for record in all_records if record.get("s3_uri")]
    errors = [record for record in all_records if record.get("error")]
    uploaded_bytes = sum(int(record.get("bytes") or 0) for record in uploaded_records)
    coverage = selected_instrument_coverage(selected)
    source_proven_base_currencies = sorted(
        {instrument_base(row) for rows in selected.values() for row in rows if instrument_base(row)}
    )
    manifest: dict[str, Any] = {
        "schema_version": "deribit-raw-staging-manifest.v1",
        "run_id": run_id,
        "generated_at": generated_at,
        "runner": pathlib.Path(__file__).name,
        "s3_prefix": s3_prefix,
        "window": {"start_date": args.start_date, "end_date": args.end_date, "start_utc": ms_to_utc(start_ms), "end_utc": ms_to_utc(end_ms)},
        "bounds": {
            "requested_base_currencies": currencies,
            "source_proven_base_currencies": source_proven_base_currencies,
            "unproven_requested_base_currencies": sorted(set(currencies) - set(source_proven_base_currencies)),
            "max_instruments_per_family": args.max_instruments_per_family,
            "max_bar_hours": args.max_bar_hours,
            "max_trade_hours": args.max_trade_hours,
            "request_workers": args.request_workers,
            "http_retries": args.http_retries,
            "request_delay_seconds": args.request_delay_seconds,
            "progress_every": args.progress_every,
        },
        "write_policy": {
            "staging_only": True,
            "canonical_writes": False,
            "secrets_required": False,
            "source_native_rest_payloads": True,
        },
        "selected_instruments": {
            family: [row.get("instrument_name") for row in rows] for family, rows in selected.items()
        },
        "selected_instrument_coverage": coverage,
        "universe_counts": {family: len(rows) for family, rows in universe.items()},
        "records": uploaded_records,
        "errors": errors,
        "source_families_uploaded": sorted({str(record["family"]) for record in uploaded_records}),
        "object_count_excluding_manifest": len(uploaded_records),
        "bytes_excluding_manifest": uploaded_bytes,
        "error_count": len(errors),
        "known_gaps": [
            "get_last_trades_by_instrument_and_time is source-native but older target-window trade probes may return empty payloads; recent trade probes are uploaded separately when available.",
            "mark_price_history_probe uses public ticker current mark/index fields; no historical mark-price REST endpoint was proven by this runner.",
            "bars_1m and volatility_index are bounded by runner hour/minute caps for this first tranche, not exhaustive three-month sweeps.",
            "get_last_settlements_by_currency and get_delivery_prices are source count-bound probes, not exhaustive three-month settlement sweeps.",
        ],
    }
    manifest_s3_uri = f"{s3_prefix}/ingest-manifests/v1/run={run_id}/deribit-backfill-manifest.json"
    manifest["manifest_s3_uri"] = manifest_s3_uri
    manifest["total_s3_object_count_including_manifest"] = len(uploaded_records) + 1
    manifest_path = write_manifest(scratch_root, manifest)
    upload_run_payloads_to_s3(scratch_root, s3_prefix, run_id)
    upload_to_s3(manifest_path, manifest_s3_uri)

    print(
        stable_json(
            {
                "run_id": run_id,
                "manifest_path": str(manifest_path),
                "manifest_s3_uri": manifest_s3_uri,
                "manifest_sha256": manifest["manifest_sha256"],
                "object_count_excluding_manifest": len(uploaded_records),
                "total_s3_object_count_including_manifest": len(uploaded_records) + 1,
                "bytes_excluding_manifest": uploaded_bytes,
                "total_s3_bytes_including_manifest": manifest["total_s3_bytes_including_manifest"],
                "source_families_uploaded": manifest["source_families_uploaded"],
                "error_count": len(errors),
                "errors": errors,
                "known_gaps": manifest["known_gaps"],
            }
        )
    )
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
