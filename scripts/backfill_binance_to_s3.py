#!/usr/bin/env python3
"""Stream a deterministic Binance Data Vision tranche into S3 staging."""

from __future__ import annotations

import argparse
import concurrent.futures
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
import xml.etree.ElementTree as ET
from typing import Any


USER_AGENT = "bolt-v2-binance-backfill/1"
DATA_ROOT = "https://data.binance.vision"
S3_LIST_ROOT = "https://s3-ap-northeast-1.amazonaws.com/data.binance.vision"
DEFAULT_S3_PREFIX = "s3://bolt-parquet/backfill-staging/2026-06-01/binance/"
DEFAULT_START_DATE = "2026-03-01"
DEFAULT_END_DATE = "2026-06-01"
DEFAULT_INTERVAL = "1m"
DEFAULT_BASE_TICKER_FILTER = "BTC,ETH,SOL,XRP,DOGE,HYPE,BNB"
HTTP_RETRY_ATTEMPTS = 5
HTTP_RETRY_SLEEP_SECONDS = 2.0
S3_XML_NS = {"s3": "http://s3.amazonaws.com/doc/2006-03-01/"}

EXCHANGE_INFO_SOURCES = (
    {
        "product": "spot",
        "url": "https://api.binance.com/api/v3/exchangeInfo",
    },
    {
        "product": "futures_um",
        "url": "https://fapi.binance.com/fapi/v1/exchangeInfo",
    },
    {
        "product": "futures_cm",
        "url": "https://dapi.binance.com/dapi/v1/exchangeInfo",
    },
)

ARCHIVE_FAMILIES = (
    {"product": "spot", "frequency": "monthly", "family": "trades"},
    {"product": "spot", "frequency": "monthly", "family": "aggTrades"},
    {"product": "spot", "frequency": "monthly", "family": "klines", "interval": True},
    {"product": "futures_um", "frequency": "monthly", "family": "trades"},
    {"product": "futures_um", "frequency": "monthly", "family": "aggTrades"},
    {"product": "futures_um", "frequency": "monthly", "family": "klines", "interval": True},
    {"product": "futures_um", "frequency": "monthly", "family": "markPriceKlines", "interval": True},
    {"product": "futures_um", "frequency": "monthly", "family": "indexPriceKlines", "interval": True},
    {"product": "futures_um", "frequency": "monthly", "family": "premiumIndexKlines", "interval": True},
    {"product": "futures_um", "frequency": "monthly", "family": "fundingRate"},
    {"product": "futures_um", "frequency": "daily", "family": "metrics"},
    {"product": "futures_cm", "frequency": "monthly", "family": "trades"},
    {"product": "futures_cm", "frequency": "monthly", "family": "aggTrades"},
    {"product": "futures_cm", "frequency": "monthly", "family": "klines", "interval": True},
    {"product": "futures_cm", "frequency": "monthly", "family": "markPriceKlines", "interval": True},
    {"product": "futures_cm", "frequency": "monthly", "family": "indexPriceKlines", "interval": True},
    {"product": "futures_cm", "frequency": "monthly", "family": "premiumIndexKlines", "interval": True},
    {"product": "futures_cm", "frequency": "monthly", "family": "fundingRate"},
    {"product": "futures_cm", "frequency": "daily", "family": "metrics"},
)


def stable_json(value: Any) -> str:
    return json.dumps(value, indent=2, sort_keys=True, ensure_ascii=True) + "\n"


def utc_now() -> str:
    return dt.datetime.now(dt.UTC).replace(microsecond=0).isoformat().replace("+00:00", "Z")


def sha256_bytes(payload: bytes) -> str:
    return hashlib.sha256(payload).hexdigest()


def parse_date(value: str) -> dt.date:
    return dt.date.fromisoformat(value)


def parse_csv_set(value: str | None) -> set[str] | None:
    if not value:
        return None
    return {item.strip().upper() for item in value.split(",") if item.strip()}


def symbol_name_possible_base_match(symbol: str, tickers: set[str] | None) -> list[str]:
    if not tickers:
        return []
    upper = symbol.upper()
    return sorted(ticker for ticker in tickers if upper == ticker or upper.startswith(ticker))


def symbol_metadata_matches_base_ticker(metadata: dict[str, Any] | None, tickers: set[str] | None) -> bool:
    if not tickers:
        return metadata is not None
    if not metadata:
        return False
    base_asset = str(metadata.get("base_asset") or "").upper()
    return base_asset in tickers


def month_iter(start: dt.date, end: dt.date) -> list[str]:
    current = dt.date(start.year, start.month, 1)
    months = []
    while current < end:
        months.append(current.strftime("%Y-%m"))
        if current.month == 12:
            current = dt.date(current.year + 1, 1, 1)
        else:
            current = dt.date(current.year, current.month + 1, 1)
    return months


def family_key(family: dict[str, Any], interval: str) -> str:
    parts = [family["product"], family["frequency"], family["family"]]
    if family.get("interval"):
        parts.append(interval)
    return ".".join(parts)


def product_archive_root(product: str) -> str:
    if product == "spot":
        return "data/spot"
    if product == "futures_um":
        return "data/futures/um"
    if product == "futures_cm":
        return "data/futures/cm"
    raise ValueError(f"Unsupported product: {product}")


def archive_symbol_prefix(family: dict[str, Any]) -> str:
    return f"{product_archive_root(family['product'])}/{family['frequency']}/{family['family']}/"


def archive_object_prefix(family: dict[str, Any], symbol: str, interval: str) -> str:
    prefix = f"{archive_symbol_prefix(family)}{symbol}/"
    if family.get("interval"):
        prefix += f"{interval}/"
    return prefix


def source_url(key: str) -> str:
    return f"{DATA_ROOT}/{key}"


def list_url(prefix: str, delimiter: str | None = None, token: str | None = None, max_keys: int = 1000) -> str:
    values = {
        "list-type": "2",
        "prefix": prefix,
        "max-keys": str(max_keys),
    }
    if delimiter:
        values["delimiter"] = delimiter
    if token:
        values["continuation-token"] = token
    return f"{S3_LIST_ROOT}?{urllib.parse.urlencode(values)}"


def http_request(url: str) -> tuple[int, dict[str, str], bytes]:
    request = urllib.request.Request(url, headers={"User-Agent": USER_AGENT})
    last_error: BaseException | None = None
    for attempt in range(HTTP_RETRY_ATTEMPTS):
        try:
            with urllib.request.urlopen(request, timeout=300) as response:
                return response.status, dict(response.headers.items()), response.read()
        except (urllib.error.URLError, TimeoutError, ConnectionResetError, OSError) as exc:
            last_error = exc
            if attempt + 1 >= HTTP_RETRY_ATTEMPTS:
                break
            time.sleep(HTTP_RETRY_SLEEP_SECONDS * (attempt + 1))
    assert last_error is not None
    raise last_error


def fetch_json(url: str) -> tuple[int, dict[str, str], bytes, Any]:
    status, headers, payload = http_request(url)
    return status, headers, payload, json.loads(payload)


def list_common_prefixes(prefix: str) -> list[str]:
    prefixes: list[str] = []
    token = None
    while True:
        _status, _headers, payload = http_request(list_url(prefix, delimiter="/", token=token))
        root = ET.fromstring(payload)
        prefixes.extend(item.text or "" for item in root.findall("s3:CommonPrefixes/s3:Prefix", S3_XML_NS))
        truncated = (root.findtext("s3:IsTruncated", default="false", namespaces=S3_XML_NS) or "false").lower()
        token = root.findtext("s3:NextContinuationToken", namespaces=S3_XML_NS)
        if truncated != "true" or not token:
            break
    return sorted(prefixes)


def list_objects(prefix: str) -> list[dict[str, Any]]:
    objects: list[dict[str, Any]] = []
    token = None
    while True:
        _status, _headers, payload = http_request(list_url(prefix, token=token))
        root = ET.fromstring(payload)
        for item in root.findall("s3:Contents", S3_XML_NS):
            key = item.findtext("s3:Key", namespaces=S3_XML_NS) or ""
            if not key.endswith(".zip"):
                continue
            objects.append(
                {
                    "key": key,
                    "source_uri": source_url(key),
                    "name": key.rsplit("/", 1)[-1],
                    "size": int(item.findtext("s3:Size", default="0", namespaces=S3_XML_NS) or "0"),
                    "last_modified": item.findtext("s3:LastModified", namespaces=S3_XML_NS),
                    "etag": (item.findtext("s3:ETag", namespaces=S3_XML_NS) or "").strip('"'),
                }
            )
        truncated = (root.findtext("s3:IsTruncated", default="false", namespaces=S3_XML_NS) or "false").lower()
        token = root.findtext("s3:NextContinuationToken", namespaces=S3_XML_NS)
        if truncated != "true" or not token:
            break
    return sorted(objects, key=lambda item: item["key"])


def exchange_symbols(parsed: Any) -> list[str]:
    symbols = parsed.get("symbols", []) if isinstance(parsed, dict) else []
    return sorted(str(item["symbol"]) for item in symbols if isinstance(item, dict) and item.get("symbol"))


def exchange_symbol_metadata(product: str, source_uri: str, parsed: Any) -> dict[str, dict[str, Any]]:
    symbols = parsed.get("symbols", []) if isinstance(parsed, dict) else []
    records: dict[str, dict[str, Any]] = {}
    for item in symbols:
        if not isinstance(item, dict) or not item.get("symbol"):
            continue
        symbol = str(item["symbol"]).upper()
        records[symbol] = {
            "symbol": symbol,
            "base_asset": item.get("baseAsset"),
            "quote_asset": item.get("quoteAsset"),
            "settlement_asset": item.get("marginAsset"),
            "pair": item.get("pair"),
            "contract_type": item.get("contractType"),
            "status": item.get("status") or item.get("contractStatus"),
            "metadata_source": "binance_exchangeInfo",
            "metadata_source_uri": source_uri,
            "product": product,
        }
    return records


def attach_symbol_metadata(record: dict[str, Any], metadata: dict[str, Any]) -> None:
    record.update(
        {
            "base_asset": metadata.get("base_asset"),
            "quote_asset": metadata.get("quote_asset"),
            "settlement_asset": metadata.get("settlement_asset"),
            "pair": metadata.get("pair"),
            "contract_type": metadata.get("contract_type"),
            "symbol_status": metadata.get("status"),
            "symbol_metadata_source": metadata.get("metadata_source"),
            "symbol_metadata_source_uri": metadata.get("metadata_source_uri"),
        }
    )


def quote_assets_by_product(symbol_metadata_by_product: dict[str, dict[str, dict[str, Any]]]) -> dict[str, set[str]]:
    quote_assets: dict[str, set[str]] = {}
    for product, records in symbol_metadata_by_product.items():
        quote_assets[product] = {
            str(item["quote_asset"]).upper()
            for item in records.values()
            if item.get("quote_asset")
        }
    return quote_assets


def infer_archive_symbol_metadata(
    product: str,
    symbol: str,
    base_tickers: set[str] | None,
    quote_assets: set[str],
    source_uri: str,
) -> dict[str, Any] | None:
    if not base_tickers or product not in {"futures_um", "futures_cm"}:
        return None
    upper = symbol.upper()
    for base_asset in sorted(base_tickers, key=len, reverse=True):
        if not upper.startswith(base_asset):
            continue
        suffix = upper[len(base_asset) :]
        if not suffix:
            continue
        contract_type = None
        quote_asset = suffix
        if product in {"futures_um", "futures_cm"} and "_" in suffix:
            quote_asset, contract_suffix = suffix.split("_", 1)
            contract_type = "PERPETUAL" if contract_suffix == "PERP" else "DELIVERY"
        if quote_asset not in quote_assets:
            continue
        settlement_asset = None
        pair = None
        if product == "futures_um":
            settlement_asset = quote_asset
            pair = f"{base_asset}{quote_asset}"
        elif product == "futures_cm":
            settlement_asset = base_asset
            pair = f"{base_asset}{quote_asset}"
        return {
            "symbol": upper,
            "base_asset": base_asset,
            "quote_asset": quote_asset,
            "settlement_asset": settlement_asset,
            "pair": pair,
            "contract_type": contract_type,
            "status": "ARCHIVE_ONLY",
            "metadata_source": "data.binance.vision_archive_symbol",
            "metadata_source_uri": source_uri,
            "product": product,
        }
    return None


def parse_object_date(name: str) -> str | None:
    match = re.search(r"(?P<date>\d{4}-\d{2}-\d{2})", name)
    if match:
        return match.group("date")
    match = re.search(r"(?P<month>\d{4}-\d{2})(?:\.zip|$)", name)
    if match:
        return match.group("month")
    return None


def date_in_window(object_date: str, start: dt.date, end: dt.date) -> bool:
    if len(object_date) == 7:
        return object_date in month_iter(start, end)
    parsed = parse_date(object_date)
    return start <= parsed < end


def s3_uri_for_payload(s3_prefix: str, record: dict[str, Any], payload_hash: str) -> str:
    date_value = record["object_date"]
    parts = [
        s3_prefix.rstrip("/"),
        "raw/v1",
        "source=data.binance.vision",
        f"product={record['product']}",
        f"frequency={record['frequency']}",
        f"family={record['family']}",
        f"symbol={record['symbol']}",
    ]
    if record.get("interval"):
        parts.append(f"interval={record['interval']}")
    parts.extend([f"dt={date_value}", f"object={payload_hash}.zip"])
    return "/".join(parts)


def s3_uri_for_checksum(s3_prefix: str, record: dict[str, Any], checksum_hash: str) -> str:
    date_value = record["object_date"]
    parts = [
        s3_prefix.rstrip("/"),
        "checksums/v1",
        "source=data.binance.vision",
        f"product={record['product']}",
        f"frequency={record['frequency']}",
        f"family={record['family']}",
        f"symbol={record['symbol']}",
    ]
    if record.get("interval"):
        parts.append(f"interval={record['interval']}")
    parts.extend([f"dt={date_value}", f"object={checksum_hash}.CHECKSUM"])
    return "/".join(parts)


def s3_uri_for_exchange_info(s3_prefix: str, product: str, payload_hash: str, generated_date: str) -> str:
    return (
        f"{s3_prefix.rstrip('/')}/raw/v1/source=binance_rest/product={product}/"
        f"family=exchangeInfo/dt={generated_date}/object={payload_hash}.json"
    )


def aws_cp(local_path: pathlib.Path, s3_uri: str) -> None:
    subprocess.run(["aws", "s3", "cp", str(local_path), s3_uri, "--only-show-errors"], check=True)


def aws_head(s3_uri: str) -> dict[str, Any]:
    parsed = urllib.parse.urlparse(s3_uri)
    result = subprocess.run(
        ["aws", "s3api", "head-object", "--bucket", parsed.netloc, "--key", parsed.path.lstrip("/")],
        check=True,
        stdout=subprocess.PIPE,
        text=True,
    )
    return json.loads(result.stdout)


def write_bytes(path: pathlib.Path, payload: bytes) -> str:
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_bytes(payload)
    return sha256_bytes(payload)


def write_json(path: pathlib.Path, value: Any) -> str:
    return write_bytes(path, stable_json(value).encode("utf-8"))


def download_to_temp(work_dir: pathlib.Path, url: str) -> tuple[pathlib.Path, str, int, int, str | None]:
    request = urllib.request.Request(url, headers={"User-Agent": USER_AGENT})
    work_dir.mkdir(parents=True, exist_ok=True)
    fd, temp_name = tempfile.mkstemp(prefix="binance-archive-", suffix=".zip", dir=work_dir)
    hasher = hashlib.sha256()
    total = 0
    try:
        with urllib.request.urlopen(request, timeout=600) as response, os.fdopen(fd, "wb") as handle:
            while True:
                chunk = response.read(1024 * 1024)
                if not chunk:
                    break
                handle.write(chunk)
                hasher.update(chunk)
                total += len(chunk)
            content_type = response.headers.get("Content-Type") or response.headers.get("content-type")
            return pathlib.Path(temp_name), hasher.hexdigest(), total, response.status, content_type
    except BaseException:
        pathlib.Path(temp_name).unlink(missing_ok=True)
        raise


def parse_checksum(payload: bytes) -> str:
    text = payload.decode("utf-8", errors="replace").strip()
    first = text.split()[0] if text.split() else ""
    if not re.fullmatch(r"[0-9a-fA-F]{64}", first):
        raise ValueError(f"Unsupported checksum payload: {text!r}")
    return first.lower()


def build_tranche(
    archive_universes: list[dict[str, Any]],
    start_date: dt.date,
    end_date: dt.date,
    interval: str,
    objects_per_family: int | None,
    base_tickers: set[str] | None,
    symbol_metadata_by_product: dict[str, dict[str, dict[str, Any]]],
    product_quote_assets: dict[str, set[str]],
) -> tuple[list[dict[str, Any]], list[dict[str, Any]], list[dict[str, Any]]]:
    selected: list[dict[str, Any]] = []
    remaining: list[dict[str, Any]] = []
    metadata_gaps: list[dict[str, Any]] = []
    for universe in archive_universes:
        family = universe["family_definition"]
        family_selected: list[dict[str, Any]] = []
        family_remaining_count = 0
        family_remaining_bytes = 0
        product_metadata = symbol_metadata_by_product.get(family["product"], {})
        product_quotes = product_quote_assets.get(family["product"], set())
        family_metadata_gaps: list[dict[str, Any]] = []
        for symbol in universe["archive_symbols"]:
            metadata = product_metadata.get(symbol)
            if metadata is None:
                metadata = infer_archive_symbol_metadata(
                    family["product"],
                    symbol,
                    base_tickers,
                    product_quotes,
                    list_url(archive_symbol_prefix(family), delimiter="/"),
                )
            if not symbol_metadata_matches_base_ticker(metadata, base_tickers):
                possible_matches = symbol_name_possible_base_match(symbol, base_tickers)
                if possible_matches and metadata is None:
                    family_metadata_gaps.append({"symbol": symbol, "possible_base_matches": possible_matches})
                continue
            objects = list_objects(archive_object_prefix(family, symbol, interval))
            window_objects = []
            for item in objects:
                object_date = parse_object_date(item["name"])
                if object_date and date_in_window(object_date, start_date, end_date):
                    window_item = dict(item)
                    window_item.update(
                        {
                            "product": family["product"],
                            "frequency": family["frequency"],
                            "family": family["family"],
                            "symbol": symbol,
                            "interval": interval if family.get("interval") else None,
                            "object_date": object_date,
                            "family_key": family_key(family, interval),
                        }
                    )
                    attach_symbol_metadata(window_item, metadata)
                    window_objects.append(window_item)
            if not window_objects:
                continue
            if objects_per_family is None:
                family_selected.extend(window_objects)
                not_selected: list[dict[str, Any]] = []
            else:
                remaining_slots = objects_per_family - len(family_selected)
                if remaining_slots > 0:
                    family_selected.extend(window_objects[:remaining_slots])
                not_selected = window_objects[remaining_slots:] if remaining_slots > 0 else window_objects
            family_remaining_count += len(not_selected)
            family_remaining_bytes += sum(item["size"] for item in not_selected)
            if objects_per_family is not None and len(family_selected) >= objects_per_family:
                break
        selected.extend(family_selected)
        if family_metadata_gaps:
            metadata_gaps.append(
                {
                    "family_key": family_key(family, interval),
                    "product": family["product"],
                    "frequency": family["frequency"],
                    "family": family["family"],
                    "interval": interval if family.get("interval") else None,
                    "archive_symbols_without_exchange_info_metadata": family_metadata_gaps,
                }
            )
        remaining.append(
            {
                "family_key": family_key(family, interval),
                "product": family["product"],
                "frequency": family["frequency"],
                "family": family["family"],
                "interval": interval if family.get("interval") else None,
                "selected_object_count": len(family_selected),
                "selected_symbols": sorted({item["symbol"] for item in family_selected}),
                "selected_dates": [item["object_date"] for item in family_selected],
                "remaining_count_for_scanned_selected_prefixes": family_remaining_count,
                "remaining_bytes_for_scanned_selected_prefixes": family_remaining_bytes,
                "remaining_all_universe": "enumerated_for_matching_exchangeInfo_base_assets",
            }
        )
        time.sleep(0.15)
    return selected, remaining, metadata_gaps


def fetch_exchange_info(
    local_root: pathlib.Path, s3_prefix: str, generated_date: str
) -> tuple[list[dict[str, Any]], dict[str, dict[str, dict[str, Any]]]]:
    records = []
    metadata_by_product: dict[str, dict[str, dict[str, Any]]] = {}
    for source in EXCHANGE_INFO_SOURCES:
        status, headers, payload, parsed = fetch_json(source["url"])
        payload_hash = sha256_bytes(payload)
        metadata_by_product[source["product"]] = exchange_symbol_metadata(source["product"], source["url"], parsed)
        local_path = (
            local_root
            / "raw"
            / "v1"
            / "source=binance_rest"
            / f"product={source['product']}"
            / "family=exchangeInfo"
            / f"dt={generated_date}"
            / f"object={payload_hash}.json"
        )
        write_bytes(local_path, payload)
        s3_uri = s3_uri_for_exchange_info(s3_prefix, source["product"], payload_hash, generated_date)
        aws_cp(local_path, s3_uri)
        head = aws_head(s3_uri)
        records.append(
            {
                "product": source["product"],
                "source_uri": source["url"],
                "http_status": status,
                "content_type": headers.get("Content-Type") or headers.get("content-type"),
                "payload_hash": payload_hash,
                "bytes": len(payload),
                "symbol_count": len(exchange_symbols(parsed)),
                "sample_symbols": exchange_symbols(parsed)[:20],
                "local_uri": str(local_path),
                "s3_uri": s3_uri,
                "s3_content_length": head.get("ContentLength"),
            }
        )
    return records, metadata_by_product


def build_archive_universes(interval: str) -> list[dict[str, Any]]:
    universes = []
    for family in ARCHIVE_FAMILIES:
        prefixes = list_common_prefixes(archive_symbol_prefix(family))
        symbols = sorted(prefix.rstrip("/").rsplit("/", 1)[-1] for prefix in prefixes)
        universes.append(
            {
                "family_key": family_key(family, interval),
                "product": family["product"],
                "frequency": family["frequency"],
                "family": family["family"],
                "interval": interval if family.get("interval") else None,
                "source_listing_uri": list_url(archive_symbol_prefix(family), delimiter="/"),
                "archive_symbol_count": len(symbols),
                "archive_symbol_sample": symbols[:20],
                "archive_symbols": symbols,
                "family_definition": family,
            }
        )
        time.sleep(0.15)
    return universes


def upload_archive_record(args: argparse.Namespace, record: dict[str, Any]) -> dict[str, Any]:
    local_path = None
    checksum_local_path = None
    started_at = utc_now()
    try:
        local_path, payload_hash, total, status, content_type = download_to_temp(args.work_dir, record["source_uri"])
        checksum_uri = record["source_uri"] + ".CHECKSUM"
        checksum_status, checksum_headers, checksum_payload = http_request(checksum_uri)
        checksum_hash = sha256_bytes(checksum_payload)
        official_checksum = parse_checksum(checksum_payload)
        checksum_matches = official_checksum == payload_hash
        if not checksum_matches:
            raise ValueError(f"Checksum mismatch for {record['source_uri']}")
        s3_uri = s3_uri_for_payload(args.s3_prefix, record, payload_hash)
        checksum_s3_uri = s3_uri_for_checksum(args.s3_prefix, record, checksum_hash)
        aws_cp(local_path, s3_uri)
        checksum_fd, checksum_name = tempfile.mkstemp(prefix="binance-checksum-", suffix=".CHECKSUM", dir=args.work_dir)
        os.close(checksum_fd)
        checksum_local_path = pathlib.Path(checksum_name)
        write_bytes(checksum_local_path, checksum_payload)
        aws_cp(checksum_local_path, checksum_s3_uri)
        payload_head = aws_head(s3_uri) if args.verify_payload_heads else {"ContentLength": total}
        checksum_head = (
            aws_head(checksum_s3_uri) if args.verify_payload_heads else {"ContentLength": len(checksum_payload)}
        )
        return {
            "family_key": record["family_key"],
            "product": record["product"],
            "frequency": record["frequency"],
            "family": record["family"],
            "symbol": record["symbol"],
            "base_asset": record.get("base_asset"),
            "quote_asset": record.get("quote_asset"),
            "settlement_asset": record.get("settlement_asset"),
            "pair": record.get("pair"),
            "contract_type": record.get("contract_type"),
            "symbol_status": record.get("symbol_status"),
            "symbol_metadata_source": record.get("symbol_metadata_source"),
            "symbol_metadata_source_uri": record.get("symbol_metadata_source_uri"),
            "interval": record.get("interval"),
            "object_date": record["object_date"],
            "source_uri": record["source_uri"],
            "source_key": record["key"],
            "source_name": record["name"],
            "source_listed_size_bytes": record["size"],
            "source_last_modified": record.get("last_modified"),
            "source_etag": record.get("etag"),
            "http_status": status,
            "content_type": content_type,
            "payload_hash": payload_hash,
            "official_checksum": official_checksum,
            "checksum_source_uri": checksum_uri,
            "checksum_http_status": checksum_status,
            "checksum_content_type": checksum_headers.get("Content-Type") or checksum_headers.get("content-type"),
            "checksum_payload_hash": checksum_hash,
            "checksum_verified": checksum_matches,
            "bytes": total,
            "started_at": started_at,
            "completed_at": utc_now(),
            "s3_uri": s3_uri,
            "checksum_s3_uri": checksum_s3_uri,
            "s3_content_length": payload_head.get("ContentLength"),
            "checksum_s3_content_length": checksum_head.get("ContentLength"),
            "s3_head_verified": args.verify_payload_heads,
        }
    finally:
        if local_path is not None and not args.keep_temp:
            local_path.unlink(missing_ok=True)
        if checksum_local_path is not None and not args.keep_temp:
            checksum_local_path.unlink(missing_ok=True)


def build_source_proven_symbol_coverage(records: list[dict[str, Any]]) -> list[dict[str, Any]]:
    grouped: dict[tuple[Any, ...], dict[str, Any]] = {}
    for record in records:
        key = (
            record.get("base_asset"),
            record["product"],
            record["symbol"],
            record.get("quote_asset"),
            record.get("settlement_asset"),
            record.get("pair"),
            record.get("contract_type"),
            record.get("symbol_status"),
            record.get("symbol_metadata_source"),
        )
        entry = grouped.setdefault(
            key,
            {
                "base_asset": record.get("base_asset"),
                "product": record["product"],
                "symbol": record["symbol"],
                "quote_asset": record.get("quote_asset"),
                "settlement_asset": record.get("settlement_asset"),
                "pair": record.get("pair"),
                "contract_type": record.get("contract_type"),
                "symbol_status": record.get("symbol_status"),
                "symbol_metadata_source": record.get("symbol_metadata_source"),
                "symbol_metadata_source_uris": set(),
                "families": set(),
                "object_dates": set(),
                "planned_payload_object_count": 0,
                "planned_payload_source_bytes": 0,
            },
        )
        if record.get("symbol_metadata_source_uri"):
            entry["symbol_metadata_source_uris"].add(record["symbol_metadata_source_uri"])
        entry["families"].add(record["family_key"])
        entry["object_dates"].add(record["object_date"])
        entry["planned_payload_object_count"] += 1
        entry["planned_payload_source_bytes"] += record["size"]

    coverage = []
    for entry in grouped.values():
        normalized = dict(entry)
        normalized["families"] = sorted(entry["families"])
        normalized["object_dates"] = sorted(entry["object_dates"])
        normalized["symbol_metadata_source_uris"] = sorted(entry["symbol_metadata_source_uris"])
        coverage.append(normalized)
    return sorted(
        coverage,
        key=lambda item: (
            str(item.get("base_asset") or ""),
            str(item.get("product") or ""),
            str(item.get("symbol") or ""),
        ),
    )


def objects_per_family_limit(value: int) -> int | None:
    return None if value == 0 else value


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--s3-prefix", default=DEFAULT_S3_PREFIX)
    parser.add_argument("--artifact-root", required=True, type=pathlib.Path)
    parser.add_argument("--work-dir", required=True, type=pathlib.Path)
    parser.add_argument("--start-date", default=DEFAULT_START_DATE)
    parser.add_argument("--end-date", default=DEFAULT_END_DATE)
    parser.add_argument("--interval", default=DEFAULT_INTERVAL)
    parser.add_argument("--objects-per-family", type=int, default=0, help="Maximum objects per family; 0 selects all.")
    parser.add_argument("--upload-workers", type=int, default=8)
    parser.add_argument("--verify-payload-heads", action="store_true")
    parser.add_argument(
        "--base-ticker-filter",
        default=DEFAULT_BASE_TICKER_FILTER,
        help="Comma-separated base tickers, for example BTC,ETH,SOL.",
    )
    parser.add_argument("--keep-temp", action="store_true")
    return parser.parse_args()


def main() -> int:
    args = parse_args()
    if not args.s3_prefix.startswith(DEFAULT_S3_PREFIX):
        raise ValueError(f"Refusing to write outside requested Binance staging prefix: {args.s3_prefix}")
    if args.objects_per_family < 0:
        raise ValueError("--objects-per-family must be non-negative; 0 selects all")
    if args.upload_workers < 1:
        raise ValueError("--upload-workers must be positive")

    start_date = parse_date(args.start_date)
    end_date = parse_date(args.end_date)
    if start_date >= end_date:
        raise ValueError("--start-date must be before --end-date")

    generated_at = utc_now()
    generated_date = generated_at[:10]
    run_seed = stable_json(
        {
            "end_date": args.end_date,
            "generated_at": generated_at,
            "interval": args.interval,
            "objects_per_family": objects_per_family_limit(args.objects_per_family),
            "upload_workers": args.upload_workers,
            "verify_payload_heads": args.verify_payload_heads,
            "base_ticker_filter": args.base_ticker_filter,
            "s3_prefix": args.s3_prefix,
            "start_date": args.start_date,
        }
    )
    run_id = "binance-backfill-run-" + sha256_bytes(run_seed.encode("utf-8"))[:16]
    manifest_path = args.artifact_root / "ingest-manifests" / "v1" / f"run={run_id}" / "binance-backfill-manifest.json"
    universe_path = args.artifact_root / "universes" / "v1" / f"run={run_id}" / "binance-universe.json"
    source_note_uri = "https://github.com/binance/binance-public-data"

    args.artifact_root.mkdir(parents=True, exist_ok=True)
    args.work_dir.mkdir(parents=True, exist_ok=True)

    archive_universes = build_archive_universes(args.interval)
    exchange_records, symbol_metadata_by_product = fetch_exchange_info(args.artifact_root, args.s3_prefix, generated_date)
    base_ticker_filter = parse_csv_set(args.base_ticker_filter)
    product_quote_assets = quote_assets_by_product(symbol_metadata_by_product)
    selected_records, remaining_scope, metadata_gaps = build_tranche(
        archive_universes,
        start_date,
        end_date,
        args.interval,
        objects_per_family_limit(args.objects_per_family),
        base_ticker_filter,
        symbol_metadata_by_product,
        product_quote_assets,
    )
    if not selected_records:
        raise RuntimeError("No Binance archive records selected for upload")

    universe_payload = {
        "schema_version": "binance-backfill-universe.v1",
        "run_id": run_id,
        "generated_at": generated_at,
        "source_note_uri": source_note_uri,
        "exchange_info_records": exchange_records,
        "archive_universe_records": [
            {key: value for key, value in item.items() if key not in {"archive_symbols", "family_definition"}}
            for item in archive_universes
        ],
    }
    universe_hash = write_json(universe_path, universe_payload)
    universe_s3_uri = f"{args.s3_prefix.rstrip('/')}/universes/v1/run={run_id}/binance-universe.json"
    aws_cp(universe_path, universe_s3_uri)
    universe_head = aws_head(universe_s3_uri)

    manifest: dict[str, Any] = {
        "schema_version": "binance-backfill-s3-manifest.v1",
        "run_id": run_id,
        "generated_at": generated_at,
        "completed_at": None,
        "source_note_uri": source_note_uri,
        "data_root_uri": DATA_ROOT,
        "s3_listing_root_uri": S3_LIST_ROOT,
        "s3_prefix": args.s3_prefix.rstrip("/") + "/",
        "canonical_s3_write": False,
        "write_mode": "s3_staging",
        "requested_time_range": {"start_utc": f"{args.start_date}T00:00:00Z", "end_utc": f"{args.end_date}T00:00:00Z"},
        "selection": {
            "mode": "all_exchangeInfo_base_asset_matches" if args.objects_per_family == 0 else "deterministic_exchangeInfo_base_asset_tranche",
            "base_ticker_filter": sorted(base_ticker_filter or []),
            "objects_per_family": objects_per_family_limit(args.objects_per_family),
            "symbol_order": "lexicographic_by_official_archive_listing",
            "object_order": "ascending_by_official_archive_key_within_requested_window",
            "interval_for_kline_families": args.interval,
            "symbol_match_rule": "archive symbol must have Binance exchangeInfo baseAsset in base_ticker_filter, except strict futures archive-only pair symbols parsed as approved base plus known quote",
        },
        "upload_workers": args.upload_workers,
        "payload_s3_head_verification": args.verify_payload_heads,
        "excluded_families": [
            "Binance options",
            "historical L2 book deltas",
            "bookDepth and bookTicker families not selected as L2 replay proof",
            "liquidation snapshots",
            "long-short ratio families not selected without separate mapping proof",
        ],
        "exchange_info_records": exchange_records,
        "universe_record": {
            "local_uri": str(universe_path),
            "payload_hash": universe_hash,
            "bytes": universe_path.stat().st_size,
            "s3_uri": universe_s3_uri,
            "s3_content_length": universe_head.get("ContentLength"),
        },
        "source_proven_symbol_coverage": build_source_proven_symbol_coverage(selected_records),
        "planned_payload_object_count": len(selected_records),
        "planned_payload_source_bytes": sum(item["size"] for item in selected_records),
        "payload_records": [],
        "remaining_scope": remaining_scope,
        "metadata_gaps": metadata_gaps,
        "errors": [],
    }
    write_json(manifest_path, manifest)

    with concurrent.futures.ThreadPoolExecutor(max_workers=args.upload_workers) as executor:
        futures = {executor.submit(upload_archive_record, args, record): record for record in selected_records}
        for future in concurrent.futures.as_completed(futures):
            record = futures[future]
            try:
                payload_record = future.result()
                manifest["payload_records"].append(payload_record)
            except (urllib.error.URLError, TimeoutError, OSError, subprocess.CalledProcessError, ValueError) as exc:
                manifest["errors"].append({"source_uri": record.get("source_uri"), "error": repr(exc)})
            write_json(manifest_path, manifest)

    manifest["payload_records"].sort(key=lambda item: item["source_key"])

    manifest["completed_at"] = utc_now()
    manifest["completed_payload_object_count"] = len(manifest["payload_records"])
    manifest["completed_payload_bytes"] = sum(item["bytes"] for item in manifest["payload_records"])
    manifest["completed_payload_shortfall_count"] = (
        manifest["planned_payload_object_count"] - manifest["completed_payload_object_count"]
    )
    manifest["payload_completion_ok"] = manifest["completed_payload_shortfall_count"] == 0
    manifest["completed_checksum_object_count"] = len([item for item in manifest["payload_records"] if item.get("checksum_s3_uri")])
    manifest["uploaded_s3_object_count_including_checksums_and_metadata"] = (
        len(manifest["payload_records"])
        + manifest["completed_checksum_object_count"]
        + len(exchange_records)
        + 1
    )
    manifest["uploaded_s3_bytes_including_checksums_and_metadata"] = (
        manifest["completed_payload_bytes"]
        + sum(item.get("checksum_s3_content_length") or 0 for item in manifest["payload_records"])
        + sum(item["bytes"] for item in exchange_records)
        + (universe_head.get("ContentLength") or 0)
    )
    manifest["manifest_hash_scope"] = "manifest_without_manifest_hash_or_s3_head"
    manifest["manifest_hash"] = sha256_bytes(stable_json(manifest).encode("utf-8"))
    manifest_hash = write_json(manifest_path, manifest)
    manifest_s3_uri = f"{args.s3_prefix.rstrip('/')}/manifests/v1/run={run_id}/binance-backfill-manifest.json"
    aws_cp(manifest_path, manifest_s3_uri)
    manifest_head = aws_head(manifest_s3_uri)
    manifest["manifest_record"] = {
        "local_uri": str(manifest_path),
        "payload_hash": manifest_hash,
        "s3_uri": manifest_s3_uri,
        "s3_content_length": manifest_head.get("ContentLength"),
    }
    final_manifest_hash = write_json(manifest_path, manifest)
    aws_cp(manifest_path, manifest_s3_uri)
    final_manifest_head = aws_head(manifest_s3_uri)

    print(
        stable_json(
            {
                "ok": not manifest["errors"] and manifest["payload_completion_ok"],
                "run_id": run_id,
                "manifest_path": str(manifest_path),
                "manifest_hash": final_manifest_hash,
                "manifest_s3_uri": manifest_s3_uri,
                "manifest_s3_content_length": final_manifest_head.get("ContentLength"),
                "universe_path": str(universe_path),
                "universe_s3_uri": universe_s3_uri,
                "planned_payload_object_count": manifest["planned_payload_object_count"],
                "completed_payload_object_count": manifest["completed_payload_object_count"],
                "completed_payload_shortfall_count": manifest["completed_payload_shortfall_count"],
                "payload_completion_ok": manifest["payload_completion_ok"],
                "completed_payload_bytes": manifest["completed_payload_bytes"],
                "uploaded_s3_object_count_including_checksums_and_metadata": manifest[
                    "uploaded_s3_object_count_including_checksums_and_metadata"
                ]
                + 1,
                "uploaded_s3_bytes_including_checksums_and_metadata": manifest[
                    "uploaded_s3_bytes_including_checksums_and_metadata"
                ]
                + (final_manifest_head.get("ContentLength") or 0),
                "errors": manifest["errors"],
            }
        ),
        end="",
    )
    return 0 if not manifest["errors"] and manifest["payload_completion_ok"] else 1


if __name__ == "__main__":
    raise SystemExit(main())
