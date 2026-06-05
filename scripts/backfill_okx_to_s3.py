#!/usr/bin/env python3
"""Stage a bounded OKX historical-download tranche into S3 staging."""

from __future__ import annotations

import argparse
import datetime as dt
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
from dataclasses import dataclass
from typing import Any


ALLOWED_S3_PREFIX = "s3://bolt-parquet/backfill-staging/2026-06-01/okx"
DEFAULT_SCRATCH_ROOT = pathlib.Path("/private/tmp/bolt-v2-okx-3m-backfill")
OKX_HISTORICAL_PAGE_URL = "https://www.okx.com/en-gb/historical-data"
OKX_DOWNLOAD_LINK_URL = "https://www.okx.com/priapi/v5/broker/public/trade-data/download-link"
OKX_PUBLIC_INSTRUMENTS_URL = "https://www.okx.com/api/v5/public/instruments"
OKX_PUBLIC_UNDERLYING_URL = "https://www.okx.com/api/v5/public/underlying"
USER_AGENT = "bolt-v2-okx-3m-backfill/1"
DEFAULT_BASE_TICKER_FILTER = "BTC,ETH,SOL,XRP,DOGE,HYPE,BNB"
REQUEST_RETRY_COUNT = 24
RETRYABLE_HTTP_STATUS_CODES = {429, 500, 502, 503, 504}
BASE_RETRY_SLEEP_SECONDS = 2.0
MAX_RETRY_SLEEP_SECONDS = 45.0

MODULE_TRADES = "1"
MODULE_CANDLESTICKS = "2"
MODULE_FUNDING_RATES = "3"
MODULE_ORDER_BOOK_400 = "4"

INSTRUMENT_TYPES = ("SPOT", "SWAP", "FUTURES")


@dataclass(frozen=True)
class DownloadTarget:
    family: str
    module: str
    inst_type: str


@dataclass(frozen=True)
class DownloadSelection:
    target: DownloadTarget
    selector: str
    query: dict[str, Any]
    response: dict[str, Any]
    response_s3_uri: str
    estimated_size_mb: float


def stable_json(value: Any) -> str:
    return json.dumps(value, indent=2, sort_keys=True, ensure_ascii=True) + "\n"


def utc_now() -> str:
    return dt.datetime.now(dt.UTC).replace(microsecond=0).isoformat().replace("+00:00", "Z")


def sha256_bytes(payload: bytes) -> str:
    return hashlib.sha256(payload).hexdigest()


def retry_sleep_seconds(attempt: int, response_headers: Any | None = None) -> float:
    retry_after_header = response_headers.get("Retry-After") if response_headers else None
    if retry_after_header:
        try:
            return min(float(retry_after_header), MAX_RETRY_SLEEP_SECONDS)
        except ValueError:
            pass
    return min(BASE_RETRY_SLEEP_SECONDS + attempt * BASE_RETRY_SLEEP_SECONDS, MAX_RETRY_SLEEP_SECONDS)


def normalize_s3_prefix(prefix: str) -> str:
    normalized = prefix.rstrip("/")
    if normalized != ALLOWED_S3_PREFIX:
        raise ValueError(f"S3 prefix must be exactly {ALLOWED_S3_PREFIX}")
    return normalized


def sanitize_selector(selector: str) -> str:
    sanitized = re.sub(r"[^A-Za-z0-9._=-]+", "_", selector)
    if not sanitized:
        raise ValueError("empty selector")
    return sanitized


def build_s3_payload_uri(
    s3_prefix: str,
    *,
    family: str,
    inst_type: str,
    selector: str,
    date_utc: str,
    payload_hash: str,
    extension: str,
) -> str:
    return (
        f"{normalize_s3_prefix(s3_prefix)}/raw/v1/"
        f"family={family}/inst_type={inst_type}/selector={sanitize_selector(selector)}/"
        f"dt={date_utc}/object={payload_hash}.{extension}"
    )


def build_s3_source_uri(s3_prefix: str, run_id: str, name: str) -> str:
    return f"{normalize_s3_prefix(s3_prefix)}/source-proofs/v1/run={run_id}/{name}"


def build_s3_manifest_uri(s3_prefix: str, run_id: str, name: str) -> str:
    return f"{normalize_s3_prefix(s3_prefix)}/manifests/v1/run={run_id}/{name}"


def date_ms(date_utc: str, *, end_of_day: bool) -> str:
    parsed = dt.datetime.strptime(date_utc, "%Y-%m-%d").replace(tzinfo=dt.UTC)
    if end_of_day:
        parsed = parsed + dt.timedelta(days=1) - dt.timedelta(milliseconds=1)
    return str(int(parsed.timestamp() * 1000))


def build_download_query(*, module: str, inst_type: str, selectors: list[str], date_utc: str) -> dict[str, Any]:
    if inst_type == "SPOT":
        inst_query = {"instIdList": selectors}
    else:
        inst_query = {"instFamilyList": selectors}
    timestamp = date_ms(date_utc, end_of_day=module == MODULE_ORDER_BOOK_400)
    return {
        "module": module,
        "instType": inst_type,
        "instQueryParam": inst_query,
        "dateQuery": {"dateAggrType": "daily", "begin": timestamp, "end": timestamp},
    }


def request_bytes(
    url: str,
    *,
    method: str = "GET",
    json_body: dict[str, Any] | None = None,
    retries: int | None = None,
) -> bytes:
    headers = {"User-Agent": USER_AGENT}
    data = None
    if json_body is not None:
        data = stable_json(json_body).encode("utf-8")
        headers["Content-Type"] = "application/json"
    retry_count = retries if retries is not None else REQUEST_RETRY_COUNT
    for attempt in range(retry_count):
        request = urllib.request.Request(url, data=data, method=method, headers=headers)
        try:
            with urllib.request.urlopen(request, timeout=120) as response:
                payload = response.read()
            if payload.startswith(b'{"msg":"Too Many Requests"') or b'"code":"50011"' in payload[:200]:
                raise RuntimeError("okx_rate_limited")
            return payload
        except urllib.error.HTTPError as error:
            if error.code not in RETRYABLE_HTTP_STATUS_CODES or attempt + 1 == retry_count:
                raise
            time.sleep(retry_sleep_seconds(attempt, error.headers))
        except (urllib.error.URLError, TimeoutError, RuntimeError):
            if attempt + 1 == retry_count:
                raise
            time.sleep(retry_sleep_seconds(attempt))
    raise RuntimeError("unreachable request retry state")


def request_json(url: str, *, method: str = "GET", json_body: dict[str, Any] | None = None) -> dict[str, Any]:
    payload = request_bytes(url, method=method, json_body=json_body)
    parsed = json.loads(payload.decode("utf-8"))
    if parsed.get("code") not in (None, "0", 0):
        raise RuntimeError(f"OKX nonzero response code for {url}: {parsed.get('code')}")
    return parsed


def write_bytes(path: pathlib.Path, payload: bytes) -> dict[str, Any]:
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_bytes(payload)
    return {"path": str(path), "bytes": len(payload), "sha256": sha256_bytes(payload)}


def write_json(path: pathlib.Path, value: dict[str, Any]) -> dict[str, Any]:
    return write_bytes(path, stable_json(value).encode("utf-8"))


def upload_to_s3(local_path: pathlib.Path, s3_uri: str) -> None:
    subprocess.run(["aws", "s3", "cp", str(local_path), s3_uri, "--only-show-errors"], check=True)


def upload_artifact(local_path: pathlib.Path, s3_uri: str) -> dict[str, Any]:
    upload_to_s3(local_path, s3_uri)
    payload = local_path.read_bytes()
    return {"local_path": str(local_path), "s3_uri": s3_uri, "bytes": len(payload), "sha256": sha256_bytes(payload)}


def public_instruments_url(inst_type: str, *, underlying: str | None = None) -> str:
    params = {"instType": inst_type}
    if underlying:
        params["uly"] = underlying
    return f"{OKX_PUBLIC_INSTRUMENTS_URL}?{urllib.parse.urlencode(params)}"


def fetch_universe(scratch_root: pathlib.Path, s3_prefix: str, run_id: str) -> tuple[dict[str, Any], list[dict[str, Any]]]:
    source_uploads: list[dict[str, Any]] = []
    universe: dict[str, Any] = {
        "source": "okx_public_instruments_and_underlying",
        "fetched_at": utc_now(),
        "instrument_endpoints": {},
        "instrument_counts": {},
        "spot_inst_ids": [],
        "swap_inst_families": [],
        "futures_inst_families": [],
        "option_underlyings": [],
        "option_inst_families": [],
        "option_inst_ids_count": 0,
    }

    for inst_type in INSTRUMENT_TYPES:
        url = public_instruments_url(inst_type)
        payload = request_json(url)
        local = scratch_root / "source-proofs" / "okx-public" / f"instruments-{inst_type}.json"
        write_json(local, payload)
        source_uploads.append(
            upload_artifact(local, build_s3_source_uri(s3_prefix, run_id, f"okx-public-instruments-{inst_type}.json"))
            | {"source_url": url}
        )
        rows = payload.get("data") or []
        universe["instrument_endpoints"][inst_type] = url
        universe["instrument_counts"][inst_type] = len(rows)
        if inst_type == "SPOT":
            universe["spot_inst_ids"] = sorted({row.get("instId", "") for row in rows if row.get("instId")})
        elif inst_type == "SWAP":
            universe["swap_inst_families"] = sorted({row.get("instFamily", "") for row in rows if row.get("instFamily")})
        elif inst_type == "FUTURES":
            universe["futures_inst_families"] = sorted(
                {row.get("instFamily", "") for row in rows if row.get("instFamily")}
            )
        time.sleep(1.2)

    underlying_url = f"{OKX_PUBLIC_UNDERLYING_URL}?{urllib.parse.urlencode({'instType': 'OPTION'})}"
    underlyings_payload = request_json(underlying_url)
    local_underlyings = scratch_root / "source-proofs" / "okx-public" / "underlying-OPTION.json"
    write_json(local_underlyings, underlyings_payload)
    source_uploads.append(
        upload_artifact(local_underlyings, build_s3_source_uri(s3_prefix, run_id, "okx-public-underlying-OPTION.json"))
        | {"source_url": underlying_url}
    )
    option_underlyings = sorted(
        {
            item
            for row in (underlyings_payload.get("data") or [])
            for item in (row if isinstance(row, list) else [row])
            if isinstance(item, str) and item
        }
    )
    universe["option_underlyings"] = option_underlyings
    universe["instrument_endpoints"]["OPTION_UNDERLYING"] = underlying_url

    option_families: set[str] = set()
    option_inst_ids: set[str] = set()
    option_counts: dict[str, int] = {}
    for underlying in option_underlyings:
        url = public_instruments_url("OPTION", underlying=underlying)
        payload = request_json(url)
        local = scratch_root / "source-proofs" / "okx-public" / f"instruments-OPTION-{sanitize_selector(underlying)}.json"
        write_json(local, payload)
        source_uploads.append(
            upload_artifact(
                local,
                build_s3_source_uri(s3_prefix, run_id, f"okx-public-instruments-OPTION-{sanitize_selector(underlying)}.json"),
            )
            | {"source_url": url}
        )
        rows = payload.get("data") or []
        option_counts[underlying] = len(rows)
        for row in rows:
            if row.get("instFamily"):
                option_families.add(row["instFamily"])
            if row.get("instId"):
                option_inst_ids.add(row["instId"])
        time.sleep(1.2)
    universe["instrument_counts"]["OPTION_BY_UNDERLYING"] = option_counts
    universe["option_inst_families"] = sorted(option_families)
    universe["option_inst_ids_count"] = len(option_inst_ids)
    return universe, source_uploads


def fetch_historical_page_proof(scratch_root: pathlib.Path, s3_prefix: str, run_id: str) -> list[dict[str, Any]]:
    uploads: list[dict[str, Any]] = []
    page_payload = request_bytes(OKX_HISTORICAL_PAGE_URL)
    page_path = scratch_root / "source-proofs" / "okx-historical-page.html"
    write_bytes(page_path, page_payload)
    uploads.append(
        upload_artifact(page_path, build_s3_source_uri(s3_prefix, run_id, "okx-historical-page.html"))
        | {"source_url": OKX_HISTORICAL_PAGE_URL}
    )
    text = page_payload.decode("utf-8", errors="replace")
    match = re.search(r'https://www\.okx\.com/cdn/assets/okfe/broker-center/brokerHistory/index\.[^"]+\.js', text)
    if match:
        js_url = match.group(0)
        js_payload = request_bytes(js_url)
        js_path = scratch_root / "source-proofs" / "okx-broker-history.js"
        write_bytes(js_path, js_payload)
        uploads.append(
            upload_artifact(js_path, build_s3_source_uri(s3_prefix, run_id, "okx-broker-history.js"))
            | {"source_url": js_url}
        )
    return uploads


def target_candidates(universe: dict[str, Any], target: DownloadTarget) -> list[str]:
    if target.inst_type == "SPOT":
        return list(universe["spot_inst_ids"])
    if target.inst_type == "SWAP":
        return list(universe["swap_inst_families"])
    if target.inst_type == "FUTURES":
        return list(universe["futures_inst_families"])
    if target.inst_type == "OPTION":
        return list(universe["option_underlyings"])
    raise ValueError(f"unsupported inst_type {target.inst_type}")


def selector_base_ticker(selector: str) -> str:
    return selector.upper().split("-", 1)[0]


def selector_matches_base_ticker(selector: str, tickers: set[str] | None) -> bool:
    if not tickers:
        return True
    return selector_base_ticker(selector) in tickers


def selector_metadata(selector: str) -> dict[str, Any]:
    parts = selector.upper().split("-")
    return {
        "selector": selector,
        "base_ticker": selector_base_ticker(selector),
        "denomination": "-".join(parts[1:]) if len(parts) > 1 else None,
    }


def selector_query_values(target: DownloadTarget, selector: str) -> list[str]:
    return [selector]


def first_group_details(response: dict[str, Any]) -> list[dict[str, Any]]:
    details = ((response.get("data") or {}).get("details") or [])
    groups: list[dict[str, Any]] = []
    for detail in details:
        for group in detail.get("groupDetails") or []:
            if group.get("url"):
                groups.append(group)
    return groups


def estimate_size_mb(response: dict[str, Any]) -> float:
    total = 0.0
    for group in first_group_details(response):
        try:
            total += float(group.get("sizeMB") or 0.0)
        except (TypeError, ValueError):
            pass
    if total:
        return total
    try:
        return float(((response.get("data") or {}).get("totalSizeMB")) or 0.0)
    except (TypeError, ValueError):
        return 0.0


def target_specs(*, family_filter: set[str] | None = None, inst_type_filter: set[str] | None = None) -> list[DownloadTarget]:
    specs: list[DownloadTarget] = []
    for inst_type in ("SPOT", "SWAP", "FUTURES", "OPTION"):
        specs.append(DownloadTarget("trades", MODULE_TRADES, inst_type))
    for inst_type in ("SPOT", "SWAP", "FUTURES", "OPTION"):
        specs.append(DownloadTarget("candlesticks", MODULE_CANDLESTICKS, inst_type))
    specs.append(DownloadTarget("funding_rates", MODULE_FUNDING_RATES, "SWAP"))
    for inst_type in ("SPOT", "SWAP", "FUTURES", "OPTION"):
        specs.append(DownloadTarget("order_book_400", MODULE_ORDER_BOOK_400, inst_type))
    if family_filter is not None:
        specs = [spec for spec in specs if spec.family in family_filter]
    if inst_type_filter is not None:
        specs = [spec for spec in specs if spec.inst_type in inst_type_filter]
    return specs


def resolve_download_selections(
    scratch_root: pathlib.Path,
    s3_prefix: str,
    run_id: str,
    universe: dict[str, Any],
    *,
    tranche_date: str,
    max_candidate_scan: int,
    max_response_size_mb: float,
    family_filter: set[str] | None,
    inst_type_filter: set[str] | None,
    base_ticker_filter: set[str] | None,
) -> tuple[list[DownloadSelection], list[dict[str, Any]], list[dict[str, Any]]]:
    selections: list[DownloadSelection] = []
    source_uploads: list[dict[str, Any]] = []
    resolution_log: list[dict[str, Any]] = []
    for target in target_specs(family_filter=family_filter, inst_type_filter=inst_type_filter):
        target_selections: list[DownloadSelection] = []
        candidates = target_candidates(universe, target)
        candidates = [selector for selector in candidates if selector_matches_base_ticker(selector, base_ticker_filter)]
        candidates = candidates[:max_candidate_scan]
        target_log: dict[str, Any] = {
            "family": target.family,
            "module": target.module,
            "inst_type": target.inst_type,
            "candidate_count": len(candidates),
            "candidate_selectors": [selector_metadata(selector) for selector in candidates],
            "attempts": [],
        }
        for selector in candidates:
            query_selectors = selector_query_values(target, selector)
            query = build_download_query(
                module=target.module,
                inst_type=target.inst_type,
                selectors=query_selectors,
                date_utc=tranche_date,
            )
            response = request_json(OKX_DOWNLOAD_LINK_URL, method="POST", json_body=query)
            size_mb = estimate_size_mb(response)
            group_count = len(first_group_details(response))
            attempt = {
                "selector": selector,
                "selector_metadata": selector_metadata(selector),
                "query_selectors": query_selectors,
                "group_count": group_count,
                "estimated_size_mb": size_mb,
                "response_code": response.get("code"),
            }
            target_log["attempts"].append(attempt)
            response_name = f"download-link-{target.family}-{target.inst_type}-{sanitize_selector(selector)}-{tranche_date}.json"
            response_path = scratch_root / "source-proofs" / "okx-download-link" / response_name
            write_json(response_path, {"request": query, "response": response, "source_url": OKX_DOWNLOAD_LINK_URL})
            response_s3_uri = build_s3_source_uri(s3_prefix, run_id, response_name)
            source_uploads.append(upload_artifact(response_path, response_s3_uri) | {"source_url": OKX_DOWNLOAD_LINK_URL})
            if group_count and (max_response_size_mb <= 0 or size_mb <= max_response_size_mb):
                selected = DownloadSelection(target, selector, query, response, response_s3_uri, size_mb)
                target_selections.append(selected)
                attempt["selected"] = True
            else:
                attempt["skipped_reason"] = "over_size_cap" if group_count else "no_group_details"
            time.sleep(1.2)
        if target_selections:
            selections.extend(target_selections)
            target_log["selected_selectors"] = [item.selector for item in target_selections]
            target_log["selected_estimated_size_mb"] = sum(item.estimated_size_mb for item in target_selections)
        else:
            target_log["selected_selectors"] = []
        resolution_log.append(target_log)
        time.sleep(1.2)
    return selections, source_uploads, resolution_log


def extension_from_url(url: str) -> str:
    path = urllib.parse.urlparse(url).path
    suffix = pathlib.Path(path).suffix.lower().lstrip(".")
    if suffix:
        return re.sub(r"[^a-z0-9]+", "", suffix)
    return "bin"


def download_payload(url: str, work_dir: pathlib.Path) -> tuple[pathlib.Path, str, int]:
    request = urllib.request.Request(url, headers={"User-Agent": USER_AGENT})
    work_dir.mkdir(parents=True, exist_ok=True)
    for attempt in range(REQUEST_RETRY_COUNT):
        fd, temp_name = tempfile.mkstemp(prefix="okx-payload-", suffix=".partial", dir=work_dir)
        hasher = hashlib.sha256()
        total = 0
        try:
            with urllib.request.urlopen(request, timeout=600) as response:
                with os.fdopen(fd, "wb") as handle:
                    while True:
                        chunk = response.read(1024 * 1024)
                        if not chunk:
                            break
                        handle.write(chunk)
                        hasher.update(chunk)
                        total += len(chunk)
            payload_hash = hasher.hexdigest()
            final_path = pathlib.Path(temp_name).with_name(f"{payload_hash}.{extension_from_url(url)}")
            pathlib.Path(temp_name).replace(final_path)
            return final_path, payload_hash, total
        except urllib.error.HTTPError as error:
            pathlib.Path(temp_name).unlink(missing_ok=True)
            if error.code not in RETRYABLE_HTTP_STATUS_CODES or attempt + 1 == REQUEST_RETRY_COUNT:
                raise
            time.sleep(retry_sleep_seconds(attempt, error.headers))
        except (urllib.error.URLError, TimeoutError):
            pathlib.Path(temp_name).unlink(missing_ok=True)
            if attempt + 1 == REQUEST_RETRY_COUNT:
                raise
            time.sleep(retry_sleep_seconds(attempt))
        except BaseException:
            pathlib.Path(temp_name).unlink(missing_ok=True)
            raise
    raise RuntimeError("unreachable payload retry state")


def payload_date_from_group(group: dict[str, Any], fallback_date: str) -> str:
    filename = group.get("filename") or ""
    match = re.search(r"(20\d{2})-(\d{2})-(\d{2})", filename)
    if match:
        return "-".join(match.groups())
    date_ts = group.get("dateTs")
    if date_ts:
        try:
            return dt.datetime.fromtimestamp(int(date_ts) / 1000, tz=dt.UTC).strftime("%Y-%m-%d")
        except (TypeError, ValueError):
            pass
    return fallback_date


def download_and_upload_payloads(
    selections: list[DownloadSelection],
    s3_prefix: str,
    work_dir: pathlib.Path,
    *,
    tranche_date: str,
) -> tuple[list[dict[str, Any]], list[dict[str, Any]]]:
    payload_records: list[dict[str, Any]] = []
    errors: list[dict[str, Any]] = []
    for selection in selections:
        for group in first_group_details(selection.response):
            source_url = group["url"]
            started_at = utc_now()
            local_path: pathlib.Path | None = None
            try:
                local_path, payload_hash, total = download_payload(source_url, work_dir)
                object_date = payload_date_from_group(group, tranche_date)
                s3_uri = build_s3_payload_uri(
                    s3_prefix,
                    family=selection.target.family,
                    inst_type=selection.target.inst_type,
                    selector=selection.selector,
                    date_utc=object_date,
                    payload_hash=payload_hash,
                    extension=extension_from_url(source_url),
                )
                upload_to_s3(local_path, s3_uri)
                payload_records.append(
                    {
                        "family": selection.target.family,
                        "module": selection.target.module,
                        "inst_type": selection.target.inst_type,
                        "selector": selection.selector,
                        "date_utc": object_date,
                        "source_url": source_url,
                        "source_filename": group.get("filename"),
                        "source_size_mb": group.get("sizeMB"),
                        "payload_sha256": payload_hash,
                        "bytes": total,
                        "s3_uri": s3_uri,
                        "download_link_response_s3_uri": selection.response_s3_uri,
                        "started_at": started_at,
                        "completed_at": utc_now(),
                    }
                )
            except (urllib.error.URLError, TimeoutError, OSError, subprocess.CalledProcessError) as exc:
                errors.append(
                    {
                        "family": selection.target.family,
                        "inst_type": selection.target.inst_type,
                        "selector": selection.selector,
                        "source_url": source_url,
                        "error": repr(exc),
                    }
                )
            finally:
                if local_path is not None:
                    local_path.unlink(missing_ok=True)
    return payload_records, errors


def build_manifest(
    *,
    run_id: str,
    args: argparse.Namespace,
    universe: dict[str, Any],
    page_source_uploads: list[dict[str, Any]],
    universe_source_uploads: list[dict[str, Any]],
    download_source_uploads: list[dict[str, Any]],
    resolution_log: list[dict[str, Any]],
    payload_records: list[dict[str, Any]],
    errors: list[dict[str, Any]],
) -> dict[str, Any]:
    families_uploaded = sorted({(record["family"], record["inst_type"]) for record in payload_records})
    base_ticker_filter = parse_filter(args.base_ticker_filter)
    selector_scope_violations = [
        {
            "family": record["family"],
            "inst_type": record["inst_type"],
            "selector": record["selector"],
            "base_ticker": selector_base_ticker(record["selector"]),
        }
        for record in payload_records
        if base_ticker_filter and not selector_matches_base_ticker(record["selector"], base_ticker_filter)
    ]
    manifest: dict[str, Any] = {
        "schema_version": "okx-raw-staging-manifest.v1",
        "run_id": run_id,
        "generated_at": utc_now(),
        "write_mode": "s3_staging",
        "canonical_s3_write": False,
        "s3_prefix": normalize_s3_prefix(args.s3_prefix),
        "requested_window_utc": {
            "start": f"{args.start_date}T00:00:00Z",
            "end": f"{args.end_date}T00:00:00Z",
            "end_semantics": "exclusive",
        },
        "tranche": {
            "mode": "deterministic_first_available_daily_file_per_source_family",
            "date_utc": args.tranche_date,
            "max_candidate_scan": args.max_candidate_scan,
            "max_response_size_mb": args.max_response_size_mb,
        },
        "official_sources": {
            "historical_page": OKX_HISTORICAL_PAGE_URL,
            "download_link_endpoint": OKX_DOWNLOAD_LINK_URL,
            "public_instruments_endpoint": OKX_PUBLIC_INSTRUMENTS_URL,
            "public_underlying_endpoint": OKX_PUBLIC_UNDERLYING_URL,
            "historical_page_families_proven": [
                "trade history from September 2021 onwards",
                "candlestick history from July 2023 onwards",
                "funding rates from March 2022 onwards",
                "high-resolution L2 order book data from March 2023 onwards",
            ],
        },
        "excluded_or_unproven": [
            "5000-level L2 is not used; the OKX page code sets the 5000-level minimum date to 2025-11-01.",
            "Full three-month all-universe completion is not claimed by this bounded tranche.",
            "Current public instrument endpoints do not prove delisted/window-expired instruments beyond what the endpoints return.",
            "No normalized NT catalog, canonical raw root, or table-contract acceptance is claimed.",
        ],
        "universe": universe,
        "selector_scope": {
            "matching_rule": "selector base is the uppercase substring before the first '-' and must exactly match the approved base set",
            "approved_base_tickers": sorted(base_ticker_filter or []),
            "payload_selector_bases": sorted({selector_base_ticker(record["selector"]) for record in payload_records}),
            "payload_selector_scope_violations": selector_scope_violations,
        },
        "families_uploaded": [{"family": family, "inst_type": inst_type} for family, inst_type in families_uploaded],
        "resolution_log": resolution_log,
        "source_proof_uploads": page_source_uploads + universe_source_uploads + download_source_uploads,
        "payload_records": payload_records,
        "errors": errors,
        "counts": {
            "payload_object_count": len(payload_records),
            "payload_bytes": sum(record["bytes"] for record in payload_records),
            "source_proof_object_count": len(page_source_uploads) + len(universe_source_uploads) + len(download_source_uploads),
            "source_proof_bytes": sum(
                record["bytes"] for record in page_source_uploads + universe_source_uploads + download_source_uploads
            ),
            "error_count": len(errors),
        },
    }
    manifest["manifest_hash_scope"] = "manifest_without_manifest_sha256"
    manifest["manifest_sha256"] = sha256_bytes(stable_json(manifest).encode("utf-8"))
    return manifest


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--s3-prefix", default=ALLOWED_S3_PREFIX)
    parser.add_argument("--scratch-root", type=pathlib.Path, default=DEFAULT_SCRATCH_ROOT)
    parser.add_argument("--start-date", default="2026-03-01")
    parser.add_argument("--end-date", default="2026-06-03")
    parser.add_argument("--tranche-date", default="2026-03-01")
    parser.add_argument("--max-candidate-scan", type=int, default=40)
    parser.add_argument("--max-response-size-mb", type=float, default=25.0)
    parser.add_argument("--request-retry-count", type=int, default=REQUEST_RETRY_COUNT)
    parser.add_argument("--base-retry-sleep-seconds", type=float, default=BASE_RETRY_SLEEP_SECONDS)
    parser.add_argument("--max-retry-sleep-seconds", type=float, default=MAX_RETRY_SLEEP_SECONDS)
    parser.add_argument("--family-filter", help="Comma-separated family filter for supplemental runs.")
    parser.add_argument("--inst-type-filter", help="Comma-separated instrument type filter for supplemental runs.")
    parser.add_argument(
        "--base-ticker-filter",
        default=DEFAULT_BASE_TICKER_FILTER,
        help="Comma-separated base tickers, for example BTC,ETH,SOL.",
    )
    parser.add_argument("--work-dir", type=pathlib.Path)
    return parser.parse_args()


def parse_filter(raw: str | None) -> set[str] | None:
    if not raw:
        return None
    values = {item.strip().upper() for item in raw.split(",") if item.strip()}
    return values or None


def parse_family_filter(raw: str | None) -> set[str] | None:
    if not raw:
        return None
    values = {item.strip().lower() for item in raw.split(",") if item.strip()}
    return values or None


def main() -> int:
    global REQUEST_RETRY_COUNT, BASE_RETRY_SLEEP_SECONDS, MAX_RETRY_SLEEP_SECONDS

    args = parse_args()
    REQUEST_RETRY_COUNT = args.request_retry_count
    BASE_RETRY_SLEEP_SECONDS = args.base_retry_sleep_seconds
    MAX_RETRY_SLEEP_SECONDS = args.max_retry_sleep_seconds
    normalize_s3_prefix(args.s3_prefix)
    generated_at = utc_now()
    run_fingerprint = stable_json(
        {
            "generated_at": generated_at,
            "tranche_date": args.tranche_date,
            "s3_prefix": args.s3_prefix,
            "family_filter": args.family_filter,
            "inst_type_filter": args.inst_type_filter,
            "base_ticker_filter": args.base_ticker_filter,
            "request_retry_count": args.request_retry_count,
            "base_retry_sleep_seconds": args.base_retry_sleep_seconds,
            "max_retry_sleep_seconds": args.max_retry_sleep_seconds,
            "scratch_root": str(args.scratch_root),
            "process_id": os.getpid(),
        }
    )
    run_id = "okx-3m-" + sha256_bytes(run_fingerprint.encode("utf-8"))[:16]
    scratch_root = args.scratch_root
    work_dir = args.work_dir or scratch_root / "downloads" / run_id
    scratch_root.mkdir(parents=True, exist_ok=True)

    page_uploads = fetch_historical_page_proof(scratch_root, args.s3_prefix, run_id)
    universe, universe_uploads = fetch_universe(scratch_root, args.s3_prefix, run_id)
    selections, download_uploads, resolution_log = resolve_download_selections(
        scratch_root,
        args.s3_prefix,
        run_id,
        universe,
        tranche_date=args.tranche_date,
        max_candidate_scan=args.max_candidate_scan,
        max_response_size_mb=args.max_response_size_mb,
        family_filter=parse_family_filter(args.family_filter),
        inst_type_filter=parse_filter(args.inst_type_filter),
        base_ticker_filter=parse_filter(args.base_ticker_filter),
    )
    payload_records, errors = download_and_upload_payloads(selections, args.s3_prefix, work_dir, tranche_date=args.tranche_date)
    manifest = build_manifest(
        run_id=run_id,
        args=args,
        universe=universe,
        page_source_uploads=page_uploads,
        universe_source_uploads=universe_uploads,
        download_source_uploads=download_uploads,
        resolution_log=resolution_log,
        payload_records=payload_records,
        errors=errors,
    )
    manifest_path = scratch_root / "manifests" / "v1" / f"run={run_id}" / "okx-raw-staging-manifest.json"
    manifest_s3_uri = build_s3_manifest_uri(args.s3_prefix, run_id, "okx-raw-staging-manifest.json")
    manifest["manifest_s3_uri"] = manifest_s3_uri
    manifest["manifest_sha256"] = sha256_bytes(stable_json(manifest).encode("utf-8"))
    write_json(manifest_path, manifest)
    upload_artifact(manifest_path, manifest_s3_uri)
    summary = {
        "ok": not errors and bool(payload_records),
        "run_id": run_id,
        "manifest_path": str(manifest_path),
        "manifest_s3_uri": manifest_s3_uri,
        "manifest_sha256": manifest["manifest_sha256"],
        "payload_object_count": manifest["counts"]["payload_object_count"],
        "payload_bytes": manifest["counts"]["payload_bytes"],
        "source_proof_object_count": manifest["counts"]["source_proof_object_count"],
        "source_proof_bytes": manifest["counts"]["source_proof_bytes"],
        "error_count": manifest["counts"]["error_count"],
        "selector_scope_violations": len(manifest["selector_scope"]["payload_selector_scope_violations"]),
        "families_uploaded": manifest["families_uploaded"],
    }
    print(stable_json(summary), end="")
    return 0 if summary["ok"] else 1


if __name__ == "__main__":
    raise SystemExit(main())
