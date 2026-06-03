#!/usr/bin/env python3
"""Backfill a deterministic Hyperliquid core tranche into S3 staging."""

from __future__ import annotations

import argparse
import concurrent.futures
import csv
import datetime as dt
import hashlib
import json
import pathlib
import re
import shlex
import subprocess
import sys
import tempfile
import time
import urllib.error
import urllib.request
from typing import Any


API_URL = "https://api.hyperliquid.xyz/info"
USER_AGENT = "bolt-v2-hyperliquid-core-backfill/1"
HYPERLIQUID_ARCHIVE_BUCKET = "hyperliquid-archive"
HYPERLIQUID_NODE_BUCKET = "hl-mainnet-node-data"


def stable_json(value: Any) -> str:
    return json.dumps(value, indent=2, sort_keys=True, ensure_ascii=True) + "\n"


def utc_now() -> str:
    return dt.datetime.now(dt.UTC).replace(microsecond=0).isoformat().replace("+00:00", "Z")


def sha256_bytes(payload: bytes) -> str:
    return hashlib.sha256(payload).hexdigest()


def parse_utc_date(value: str) -> dt.date:
    return dt.date.fromisoformat(value)


def yyyymmdd(value: dt.date) -> str:
    return value.strftime("%Y%m%d")


def normalized_s3_prefix(prefix: str) -> str:
    if not prefix.startswith("s3://"):
        raise ValueError("S3 prefix must start with s3://")
    return prefix.rstrip("/")


def parse_s3_uri(uri: str) -> tuple[str, str]:
    if not uri.startswith("s3://"):
        raise ValueError(f"not an S3 URI: {uri}")
    rest = uri[len("s3://") :]
    bucket, _, key = rest.partition("/")
    if not bucket or not key:
        raise ValueError(f"not an object S3 URI: {uri}")
    return bucket, key


def run_command(args: list[str]) -> subprocess.CompletedProcess[str]:
    return subprocess.run(args, check=True, capture_output=True, text=True)


def aws_json(args: list[str]) -> Any:
    completed = run_command(["aws", *args, "--output", "json"])
    if not completed.stdout.strip():
        return None
    return json.loads(completed.stdout)


def aws_list_objects(bucket: str, prefix: str, *, requester_pays: bool, max_keys: int | None = None) -> list[dict[str, Any]]:
    args = ["s3api", "list-objects-v2", "--bucket", bucket, "--prefix", prefix]
    if requester_pays:
        args.extend(["--request-payer", "requester"])
    if max_keys is not None:
        args.extend(["--max-keys", str(max_keys)])
    result = aws_json(args)
    return list((result or {}).get("Contents") or [])


def aws_list_objects_all(bucket: str, prefix: str, *, requester_pays: bool) -> list[dict[str, Any]]:
    objects: list[dict[str, Any]] = []
    token: str | None = None
    while True:
        args = ["s3api", "list-objects-v2", "--bucket", bucket, "--prefix", prefix]
        if requester_pays:
            args.extend(["--request-payer", "requester"])
        if token:
            args.extend(["--continuation-token", token])
        result = aws_json(args) or {}
        objects.extend(list(result.get("Contents") or []))
        token = result.get("NextContinuationToken")
        if not token:
            return objects


def aws_head_object(uri: str, *, requester_pays: bool) -> dict[str, Any]:
    bucket, key = parse_s3_uri(uri)
    args = ["s3api", "head-object", "--bucket", bucket, "--key", key]
    if requester_pays:
        args.extend(["--request-payer", "requester"])
    head = aws_json(args)
    return dict(head or {})


def aws_cp_from_s3(source_uri: str, local_path: pathlib.Path, *, requester_pays: bool) -> None:
    args = ["aws", "s3", "cp", source_uri, str(local_path), "--only-show-errors"]
    if requester_pays:
        args.extend(["--request-payer", "requester"])
    subprocess.run(args, check=True)


def aws_cp_to_s3(local_path: pathlib.Path, dest_uri: str) -> None:
    subprocess.run(["aws", "s3", "cp", str(local_path), dest_uri, "--only-show-errors"], check=True)


def aws_cp_dir_to_s3_filtered(local_dir: pathlib.Path, dest_uri: str, *, include_pattern: str) -> None:
    subprocess.run(
        [
            "aws",
            "s3",
            "cp",
            str(local_dir),
            dest_uri,
            "--recursive",
            "--only-show-errors",
            "--exclude",
            "*",
            "--include",
            include_pattern,
        ],
        check=True,
    )


def aws_cp_s3_to_s3(source_uri: str, dest_uri: str, *, requester_pays: bool) -> None:
    args = ["aws", "s3", "cp", source_uri, dest_uri, "--only-show-errors"]
    if requester_pays:
        args.extend(["--request-payer", "requester"])
    subprocess.run(args, check=True)


def aws_copy_object(*, source_bucket: str, source_key: str, dest_bucket: str, dest_key: str, requester_pays: bool) -> dict[str, Any]:
    args = [
        "aws",
        "s3api",
        "copy-object",
        "--bucket",
        dest_bucket,
        "--key",
        dest_key,
        "--copy-source",
        f"{source_bucket}/{source_key}",
        "--metadata-directive",
        "COPY",
        "--output",
        "json",
    ]
    if requester_pays:
        args.extend(["--request-payer", "requester"])
    completed = subprocess.run(args, check=True, capture_output=True, text=True)
    return json.loads(completed.stdout) if completed.stdout.strip() else {}


def write_bytes(path: pathlib.Path, payload: bytes) -> dict[str, Any]:
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_bytes(payload)
    return {"path": str(path), "bytes": len(payload), "sha256": sha256_bytes(payload)}


def hash_file(path: pathlib.Path) -> tuple[str, int]:
    hasher = hashlib.sha256()
    total = 0
    with path.open("rb") as handle:
        while True:
            chunk = handle.read(1024 * 1024)
            if not chunk:
                break
            hasher.update(chunk)
            total += len(chunk)
    return hasher.hexdigest(), total


def api_post(body: dict[str, Any]) -> tuple[bytes, int, dict[str, str]]:
    payload = stable_json(body).encode("utf-8")
    request = urllib.request.Request(
        API_URL,
        data=payload,
        headers={"Content-Type": "application/json", "User-Agent": USER_AGENT},
        method="POST",
    )
    with urllib.request.urlopen(request, timeout=120) as response:
        return response.read(), response.status, dict(response.headers.items())


def upload_payload(
    *,
    scratch_root: pathlib.Path,
    s3_prefix: str,
    source_family: str,
    local_payload: pathlib.Path,
    suffix: str,
    partitions: dict[str, str],
    source: dict[str, Any],
) -> dict[str, Any]:
    payload_hash, total = hash_file(local_payload)
    partition_text = "/".join(f"{key}={value}" for key, value in partitions.items())
    dest_uri = (
        f"{normalized_s3_prefix(s3_prefix)}/raw/v1/"
        f"source_family={source_family}/{partition_text}/object={payload_hash}{suffix}"
    )
    aws_cp_to_s3(local_payload, dest_uri)
    head = aws_head_object(dest_uri, requester_pays=False)
    return {
        "source_family": source_family,
        "source": source,
        "local_path": str(local_payload),
        "s3_uri": dest_uri,
        "bytes": total,
        "sha256": payload_hash,
        "s3_head_content_length": head.get("ContentLength"),
        "s3_head_etag": head.get("ETag"),
        "uploaded_at": utc_now(),
        "scratch_root": str(scratch_root),
    }


def copy_s3_payload(
    *,
    scratch_root: pathlib.Path,
    s3_prefix: str,
    source_family: str,
    source_uri: str,
    source_head: dict[str, Any],
    suffix: str,
    partitions: dict[str, str],
    requester_pays: bool,
    server_side_copy: bool = False,
) -> dict[str, Any]:
    if server_side_copy:
        identity_payload = stable_json(
            {
                "source_uri": source_uri,
                "source_etag": source_head.get("ETag"),
                "source_content_length": source_head.get("ContentLength"),
            }
        ).encode("utf-8")
        object_identity = sha256_bytes(identity_payload)
        partition_text = "/".join(f"{key}={value}" for key, value in partitions.items())
        dest_uri = (
            f"{normalized_s3_prefix(s3_prefix)}/raw/v1/"
            f"source_family={source_family}/{partition_text}/object={object_identity}{suffix}"
        )
        aws_cp_s3_to_s3(source_uri, dest_uri, requester_pays=requester_pays)
        head = aws_head_object(dest_uri, requester_pays=False)
        total = int(head.get("ContentLength") or source_head.get("ContentLength") or 0)
        return {
            "source_family": source_family,
            "source": {
                "type": "s3_server_side_copy",
                "uri": source_uri,
                "requester_pays": requester_pays,
                "head_content_length": source_head.get("ContentLength"),
                "head_etag": source_head.get("ETag"),
                "head_last_modified": str(source_head.get("LastModified")) if source_head.get("LastModified") else None,
            },
            "local_path": None,
            "s3_uri": dest_uri,
            "bytes": total,
            "sha256": None,
            "object_identity_sha256": object_identity,
            "object_identity_scope": "source_uri_etag_content_length",
            "s3_head_content_length": head.get("ContentLength"),
            "s3_head_etag": head.get("ETag"),
            "uploaded_at": utc_now(),
            "scratch_root": str(scratch_root),
        }
    with tempfile.TemporaryDirectory(prefix="payload-", dir=scratch_root / "downloads") as temp_dir:
        local_path = pathlib.Path(temp_dir) / pathlib.Path(source_uri).name
        aws_cp_from_s3(source_uri, local_path, requester_pays=requester_pays)
        return upload_payload(
            scratch_root=scratch_root,
            s3_prefix=s3_prefix,
            source_family=source_family,
            local_payload=local_path,
            suffix=suffix,
            partitions=partitions,
            source={
                "type": "s3",
                "uri": source_uri,
                "requester_pays": requester_pays,
                "head_content_length": source_head.get("ContentLength"),
                "head_etag": source_head.get("ETag"),
                "head_last_modified": str(source_head.get("LastModified")) if source_head.get("LastModified") else None,
            },
        )


def source_uri(bucket: str, key: str) -> str:
    return f"s3://{bucket}/{key}"


def coin_from_l2_key(key: str) -> str:
    return pathlib.PurePosixPath(key).name.removesuffix(".lz4")


def parse_coin_filter(value: str | None) -> list[str]:
    if not value:
        return []
    return [coin.strip().upper() for coin in value.split(",") if coin.strip()]


def token_name(value: Any) -> str:
    return str(value or "").upper()


def spot_market_descriptors(spot_meta: Any, selected_base_tickers: set[str]) -> list[dict[str, Any]]:
    if not isinstance(spot_meta, dict):
        return []
    tokens = spot_meta.get("tokens")
    universe = spot_meta.get("universe")
    if not isinstance(tokens, list) or not isinstance(universe, list):
        return []
    token_by_index = {item.get("index"): item for item in tokens if isinstance(item, dict)}
    descriptors: list[dict[str, Any]] = []
    for market in universe:
        if not isinstance(market, dict):
            continue
        market_tokens = market.get("tokens")
        if not isinstance(market_tokens, list) or len(market_tokens) < 2:
            continue
        base = token_by_index.get(market_tokens[0])
        quote = token_by_index.get(market_tokens[1])
        if not isinstance(base, dict) or not isinstance(quote, dict):
            continue
        base_ticker = token_name(base.get("name"))
        if selected_base_tickers and base_ticker not in selected_base_tickers:
            continue
        descriptors.append(
            {
                "market_type": "spot",
                "asset": str(market.get("name")),
                "coin": str(market.get("name")),
                "market_index": market.get("index"),
                "base_ticker": base_ticker,
                "quote_ticker": token_name(quote.get("name")),
                "token_indexes": market_tokens,
                "base_token_index": base.get("index"),
                "quote_token_index": quote.get("index"),
                "is_canonical": market.get("isCanonical"),
            }
        )
    return descriptors


def filter_spot_meta_payload(payload: Any, selected_base_tickers: set[str]) -> Any:
    if not selected_base_tickers or not isinstance(payload, dict):
        return payload
    descriptors = spot_market_descriptors(payload, selected_base_tickers)
    selected_market_names = {item["coin"] for item in descriptors}
    selected_token_indexes = {
        token_index
        for item in descriptors
        for token_index in item.get("token_indexes", [])
    }
    filtered = dict(payload)
    universe = payload.get("universe")
    if isinstance(universe, list):
        filtered["universe"] = [
            item for item in universe if isinstance(item, dict) and str(item.get("name")) in selected_market_names
        ]
    tokens = payload.get("tokens")
    if isinstance(tokens, list):
        filtered["tokens"] = [
            item for item in tokens if isinstance(item, dict) and item.get("index") in selected_token_indexes
        ]
    return filtered


def filter_spot_meta_and_asset_ctxs_payload(payload: Any, selected_base_tickers: set[str]) -> Any:
    if not selected_base_tickers or not (isinstance(payload, list) and len(payload) == 2):
        return payload
    meta = filter_spot_meta_payload(payload[0], selected_base_tickers)
    selected_market_names = {item["coin"] for item in spot_market_descriptors(payload[0], selected_base_tickers)}
    contexts = payload[1] if isinstance(payload[1], list) else []
    return [meta, [item for item in contexts if isinstance(item, dict) and str(item.get("coin")) in selected_market_names]]


def market_catalog(
    *,
    perp_meta: Any,
    spot_meta: Any,
    selected_base_tickers: set[str],
    selected_coin_names: set[str],
) -> dict[str, Any]:
    perps: list[dict[str, Any]] = []
    universe = perp_meta.get("universe", []) if isinstance(perp_meta, dict) else []
    for item in universe:
        if not isinstance(item, dict):
            continue
        coin = str(item.get("name"))
        base_ticker = token_name(coin)
        if selected_base_tickers and base_ticker not in selected_base_tickers:
            continue
        if selected_coin_names and not selected_base_tickers and base_ticker not in selected_coin_names:
            continue
        perps.append(
            {
                "market_type": "perpetual",
                "asset": coin,
                "coin": coin,
                "base_ticker": base_ticker,
                "is_delisted": bool(item.get("isDelisted")),
            }
        )
    spot = spot_market_descriptors(spot_meta, selected_base_tickers) if selected_base_tickers else []
    selected_market_names = sorted({item["coin"] for item in perps + spot})
    return {
        "base_ticker_filter": sorted(selected_base_tickers) if selected_base_tickers else None,
        "legacy_coin_filter": sorted(selected_coin_names) if selected_coin_names else None,
        "filter_rule": "base_ticker" if selected_base_tickers else ("coin_name" if selected_coin_names else "none"),
        "perpetual": perps,
        "spot": spot,
        "selected_market_names": selected_market_names,
        "selected_market_count": len(selected_market_names),
    }


def filter_meta_payload(payload: Any, selected_coins: set[str]) -> Any:
    if not selected_coins or not isinstance(payload, dict):
        return payload
    filtered = dict(payload)
    universe = payload.get("universe")
    if isinstance(universe, list):
        filtered["universe"] = [
            item for item in universe if isinstance(item, dict) and str(item.get("name", "")).upper() in selected_coins
        ]
    return filtered


def filter_meta_and_asset_ctxs_payload(payload: Any, selected_coins: set[str]) -> Any:
    if not selected_coins or not (isinstance(payload, list) and len(payload) == 2):
        return payload
    meta = filter_meta_payload(payload[0], selected_coins)
    universe = payload[0].get("universe", []) if isinstance(payload[0], dict) else []
    indexes = [
        index
        for index, item in enumerate(universe)
        if isinstance(item, dict) and str(item.get("name", "")).upper() in selected_coins
    ]
    contexts = payload[1] if isinstance(payload[1], list) else []
    return [meta, [contexts[index] for index in indexes if index < len(contexts)]]


def fetch_and_upload_api_payload(
    *,
    scratch_root: pathlib.Path,
    s3_prefix: str,
    source_family: str,
    request_body: dict[str, Any],
    partitions: dict[str, str],
    selected_coins: set[str] | None = None,
    selected_base_tickers: set[str] | None = None,
) -> tuple[dict[str, Any], Any]:
    started_at = utc_now()
    response_payload, status, headers = api_post(request_body)
    parsed_payload = json.loads(response_payload)
    selected_base_tickers = selected_base_tickers or set()
    if selected_coins:
        if source_family == "meta":
            parsed_payload = filter_meta_payload(parsed_payload, selected_coins)
        elif source_family == "metaAndAssetCtxs":
            parsed_payload = filter_meta_and_asset_ctxs_payload(parsed_payload, selected_coins)
        response_payload = stable_json(parsed_payload).encode("utf-8")
    if selected_base_tickers:
        if source_family == "spotMeta":
            parsed_payload = filter_spot_meta_payload(parsed_payload, selected_base_tickers)
        elif source_family == "spotMetaAndAssetCtxs":
            parsed_payload = filter_spot_meta_and_asset_ctxs_payload(parsed_payload, selected_base_tickers)
        response_payload = stable_json(parsed_payload).encode("utf-8")
    local_path = scratch_root / "api" / source_family / (sha256_bytes(stable_json(request_body).encode("utf-8"))[:16] + ".json")
    write_bytes(local_path, response_payload)
    record = upload_payload(
        scratch_root=scratch_root,
        s3_prefix=s3_prefix,
        source_family=source_family,
        local_payload=local_path,
        suffix=".json",
        partitions=partitions,
        source={
            "type": "https_api",
            "url": API_URL,
            "request_body": request_body,
            "selected_coins": sorted(selected_coins) if selected_coins else None,
            "selected_base_tickers": sorted(selected_base_tickers) if selected_base_tickers else None,
            "http_status": status,
            "content_type": headers.get("Content-Type") or headers.get("content-type"),
            "started_at": started_at,
        },
    )
    return record, parsed_payload


def copy_filtered_asset_ctxs_payload(
    *,
    scratch_root: pathlib.Path,
    s3_prefix: str,
    source_uri: str,
    source_head: dict[str, Any],
    partitions: dict[str, str],
    selected_market_names: set[str],
    selected_base_tickers: set[str],
) -> dict[str, Any]:
    with tempfile.TemporaryDirectory(prefix="payload-", dir=scratch_root / "downloads") as temp_dir:
        temp_path = pathlib.Path(temp_dir)
        source_path = temp_path / pathlib.PurePosixPath(source_uri).name
        csv_path = temp_path / "asset_ctxs.csv"
        filtered_csv_path = temp_path / "asset_ctxs.filtered.csv"
        filtered_lz4_path = temp_path / "asset_ctxs.filtered.csv.lz4"
        aws_cp_from_s3(source_uri, source_path, requester_pays=True)
        with csv_path.open("wb") as decoded:
            subprocess.run(["lz4", "-dc", str(source_path)], check=True, stdout=decoded)
        with csv_path.open(newline="") as source_file, filtered_csv_path.open("w", newline="") as dest_file:
            reader = csv.DictReader(source_file)
            writer = csv.DictWriter(dest_file, fieldnames=reader.fieldnames or [])
            writer.writeheader()
            row_count = 0
            filtered_market_names: set[str] = set()
            for row in reader:
                if str(row.get("coin", "")) in selected_market_names:
                    writer.writerow(row)
                    filtered_market_names.add(str(row.get("coin", "")))
                    row_count += 1
        subprocess.run(["lz4", "-z", "-f", str(filtered_csv_path), str(filtered_lz4_path)], check=True, capture_output=True, text=True)
        record = upload_payload(
            scratch_root=scratch_root,
            s3_prefix=s3_prefix,
            source_family="asset_ctxs",
            local_payload=filtered_lz4_path,
            suffix=".csv.lz4",
            partitions=partitions,
            source={
                "type": "s3_filtered_csv_lz4",
                "uri": source_uri,
                "requester_pays": True,
                "selected_market_names": sorted(selected_market_names),
                "selected_base_tickers": sorted(selected_base_tickers),
                "filtered_market_names": sorted(filtered_market_names),
                "filtered_row_count": row_count,
                "head_content_length": source_head.get("ContentLength"),
                "head_etag": source_head.get("ETag"),
                "head_last_modified": str(source_head.get("LastModified")) if source_head.get("LastModified") else None,
            },
        )
        record["filtered_row_count"] = row_count
        record["filtered_market_names"] = sorted(filtered_market_names)
        return record


def fetch_funding_pages(
    *,
    scratch_root: pathlib.Path,
    s3_prefix: str,
    coin: str,
    start_ms: int,
    end_ms: int,
    sleep_seconds: float,
) -> list[dict[str, Any]]:
    records: list[dict[str, Any]] = []
    cursor = start_ms
    page = 0
    while cursor < end_ms:
        request_body = {"type": "fundingHistory", "coin": coin, "startTime": cursor, "endTime": end_ms}
        record, data = fetch_and_upload_api_payload(
            scratch_root=scratch_root,
            s3_prefix=s3_prefix,
            source_family="fundingHistory",
            request_body=request_body,
            partitions={"coin": safe_partition(coin), "start_ms": str(start_ms), "end_ms": str(end_ms), "page": f"{page:04d}"},
        )
        rows = data if isinstance(data, list) else []
        record["funding_row_count"] = len(rows)
        if rows:
            times = [int(row["time"]) for row in rows if isinstance(row, dict) and "time" in row]
            if times:
                record["min_time_ms"] = min(times)
                record["max_time_ms"] = max(times)
        records.append(record)
        if len(rows) < 500:
            break
        times = [int(row["time"]) for row in rows if isinstance(row, dict) and "time" in row]
        if not times:
            break
        next_cursor = max(times) + 1
        if next_cursor <= cursor:
            break
        cursor = next_cursor
        page += 1
        if sleep_seconds:
            time.sleep(sleep_seconds)
    return records


def safe_partition(value: str) -> str:
    return re.sub(r"[^A-Za-z0-9_.=-]+", "_", value)


def utc_ms(value: dt.datetime) -> int:
    return int(value.timestamp() * 1000)


def select_dates(start_date: dt.date, count: int) -> list[dt.date]:
    return [start_date + dt.timedelta(days=offset) for offset in range(count)]


def write_manifest_checkpoint(
    *,
    manifest: dict[str, Any],
    scratch_root: pathlib.Path,
    s3_prefix: str,
    checkpoint: dict[str, Any],
    upload_to_s3: bool = True,
) -> dict[str, Any]:
    checkpoint_id = safe_partition("_".join(str(value) for value in checkpoint.values()))
    family = str(checkpoint.get("family", ""))
    date = str(checkpoint.get("date", ""))
    hour = checkpoint.get("hour")
    coin = str(checkpoint.get("coin", ""))

    def record_matches(record: dict[str, Any]) -> bool:
        if record.get("source_family") != family:
            return False
        uri = str(record.get("s3_uri") or "")
        if family == "l2Book":
            return f"source_family=l2Book/date={date}/" in uri and f"/{hour}/l2Book/" in uri
        if family == "asset_ctxs":
            return f"source_family=asset_ctxs/date={date}/" in uri
        if family == "fundingHistory":
            return f"source_family=fundingHistory/coin={safe_partition(coin)}/" in uri
        return False

    def issue_matches(issue: dict[str, Any]) -> bool:
        if issue.get("family") != family:
            return False
        if date and str(issue.get("date", "")) != date:
            return False
        if hour is not None and issue.get("hour") != hour:
            return False
        if coin and str(issue.get("coin", "")) != coin:
            return False
        return True

    checkpoint_payload_records = [record for record in manifest.get("payload_records", []) if record_matches(record)]
    checkpoint_gaps = [issue for issue in manifest.get("gaps", []) if issue_matches(issue)]
    checkpoint_errors = [issue for issue in manifest.get("errors", []) if issue_matches(issue)]
    all_records = list(manifest.get("payload_records", []))
    snapshot = {
        "schema_version": "hyperliquid-core-backfill-partial-manifest.v1",
        "parent_schema_version": manifest.get("schema_version"),
        "run_id": manifest["run_id"],
        "partial_manifest": True,
        "generated_at": utc_now(),
        "checkpoint": checkpoint,
        "window_utc": manifest.get("window_utc"),
        "s3_prefix": normalized_s3_prefix(s3_prefix),
        "market_coverage": manifest.get("market_coverage"),
        "payload_records": checkpoint_payload_records,
        "payload_record_count": len(checkpoint_payload_records),
        "payload_bytes": sum(int(item.get("bytes") or 0) for item in checkpoint_payload_records),
        "gaps": checkpoint_gaps,
        "errors": checkpoint_errors,
        "totals_so_far": {
            "payload_record_count": len(all_records),
            "payload_bytes": sum(int(item.get("bytes") or 0) for item in all_records),
            "gap_count": len(manifest.get("gaps", [])),
            "error_count": len(manifest.get("errors", [])),
            "families_uploaded": sorted({item["source_family"] for item in all_records}),
        },
    }
    payload = stable_json(snapshot).encode("utf-8")
    snapshot_sha = sha256_bytes(payload)
    local_path = scratch_root / "manifests" / "progress" / f"{checkpoint_id}.json"
    write_bytes(local_path, payload)
    s3_uri = (
        f"{normalized_s3_prefix(s3_prefix)}/manifests/v1/run={manifest['run_id']}/"
        f"progress/{checkpoint_id}.json"
    )
    if upload_to_s3:
        aws_cp_to_s3(local_path, s3_uri)
    record = {
        "checkpoint": checkpoint,
        "local_manifest": str(local_path),
        "manifest_s3_uri": s3_uri,
        "manifest_sha256": snapshot_sha,
        "uploaded_to_s3": upload_to_s3,
        "written_at": utc_now(),
    }
    manifest.setdefault("partial_manifest_records", []).append(record)
    return record


def upload_manifest_checkpoints(
    *,
    manifest: dict[str, Any],
    scratch_root: pathlib.Path,
    s3_prefix: str,
    include_pattern: str,
    checkpoint_match: dict[str, Any],
) -> None:
    local_dir = scratch_root / "manifests" / "progress"
    dest_uri = f"{normalized_s3_prefix(s3_prefix)}/manifests/v1/run={manifest['run_id']}/progress/"
    aws_cp_dir_to_s3_filtered(local_dir, dest_uri, include_pattern=include_pattern)
    uploaded_at = utc_now()
    for record in manifest.get("partial_manifest_records", []):
        checkpoint = record.get("checkpoint") or {}
        if all(checkpoint.get(key) == value for key, value in checkpoint_match.items()):
            record["uploaded_to_s3"] = True
            record["uploaded_at"] = uploaded_at


def build_manifest(args: argparse.Namespace) -> dict[str, Any]:
    start_date = parse_utc_date(args.start_date)
    end_date = parse_utc_date(args.end_date)
    selected_coins = parse_coin_filter(getattr(args, "coins", None))
    selected_base_tickers = parse_coin_filter(getattr(args, "base_tickers", None))
    start_dt = dt.datetime.combine(start_date, dt.time(), tzinfo=dt.UTC)
    funding_end_dt = start_dt + dt.timedelta(days=args.funding_days)
    if funding_end_dt > dt.datetime.combine(end_date, dt.time(), tzinfo=dt.UTC):
        funding_end_dt = dt.datetime.combine(end_date, dt.time(), tzinfo=dt.UTC)

    run_id_seed = f"{utc_now()} {args.s3_prefix} {args.start_date} {args.end_date}"
    run_id = "hyperliquid-core-" + sha256_bytes(run_id_seed.encode("utf-8"))[:16]
    return {
        "schema_version": "hyperliquid-core-backfill-manifest.v1",
        "run_id": run_id,
        "generated_at": utc_now(),
        "venue": "hyperliquid",
        "product_family": "core",
        "window_utc": {
            "start_inclusive": f"{args.start_date}T00:00:00Z",
            "end_exclusive": f"{args.end_date}T00:00:00Z",
        },
        "write_mode": "s3_staging_only",
        "canonical_s3_write": False,
        "s3_prefix": normalized_s3_prefix(args.s3_prefix),
        "local_scratch_root": str(args.scratch_root),
        "official_sources": {
            "docs_historical_data": "https://hyperliquid.gitbook.io/hyperliquid-docs/historical-data",
            "docs_info_endpoint": "https://hyperliquid.gitbook.io/hyperliquid-docs/for-developers/api/info-endpoint",
            "docs_perpetual_info_endpoint": "https://hyperliquid.gitbook.io/hyperliquid-docs/for-developers/api/info-endpoint/perpetuals",
            "docs_spot_info_endpoint": "https://hyperliquid.gitbook.io/hyperliquid-docs/for-developers/api/info-endpoint/spot",
            "archive_bucket": f"s3://{HYPERLIQUID_ARCHIVE_BUCKET}",
            "node_bucket": f"s3://{HYPERLIQUID_NODE_BUCKET}",
            "api_url": API_URL,
        },
        "tranche": {
            "l2_dates": [yyyymmdd(date) for date in select_dates(start_date, args.l2_days)],
            "l2_hours_per_date": args.l2_hours,
            "asset_ctx_dates": [yyyymmdd(date) for date in select_dates(start_date, args.asset_ctx_days)],
            "funding_start_ms": utc_ms(start_dt),
            "funding_end_ms": utc_ms(funding_end_dt),
            "funding_days": args.funding_days,
            "funding_coin_limit": args.funding_coin_limit,
            "selected_coins": selected_coins,
            "selected_base_tickers": selected_base_tickers,
            "copy_workers": args.copy_workers,
            "node_family": args.node_family,
            "node_date": args.node_date,
            "node_hours": args.node_hours,
        },
        "universe": {},
        "market_coverage": {},
        "payload_records": [],
        "source_listing_records": [],
        "schema_probe_records": [],
        "partial_manifest_records": [],
        "gaps": [],
        "errors": [],
        "commands_or_checks_run": [],
    }


def schema_probe_lz4(source_uri_value: str, scratch_root: pathlib.Path, requester_pays: bool) -> dict[str, Any]:
    probe_dir = scratch_root / "schema-probes"
    probe_dir.mkdir(parents=True, exist_ok=True)
    local_path = probe_dir / pathlib.PurePosixPath(source_uri_value).name
    aws_cp_from_s3(source_uri_value, local_path, requester_pays=requester_pays)
    process = subprocess.Popen(["lz4", "-dc", str(local_path)], stdout=subprocess.PIPE, stderr=subprocess.PIPE)
    assert process.stdout is not None
    first_line = process.stdout.readline().rstrip(b"\n")
    process.terminate()
    _, stderr = process.communicate(timeout=10)
    if process.returncode not in (0, -15):
        raise subprocess.CalledProcessError(process.returncode, ["lz4", "-dc", str(local_path)], stderr=stderr)
    parsed = json.loads(first_line) if first_line else None
    if isinstance(parsed, list) and parsed:
        sample = parsed[0]
    else:
        sample = parsed
    field_names = sorted(sample.keys()) if isinstance(sample, dict) else []
    payload_hash, total = hash_file(local_path)
    local_path.unlink(missing_ok=True)
    return {
        "source_uri": source_uri_value,
        "probe": "lz4_first_json_line",
        "local_bytes": total,
        "local_sha256": payload_hash,
        "top_level_type": type(parsed).__name__,
        "sample_type": type(sample).__name__,
        "sample_field_names": field_names,
        "ok": bool(field_names),
    }


def main() -> int:
    args = parse_args()
    args.s3_prefix = normalized_s3_prefix(args.s3_prefix)
    selected_coins = set(parse_coin_filter(args.coins))
    selected_base_tickers = set(parse_coin_filter(args.base_tickers))
    perp_filter_coins = selected_base_tickers or selected_coins
    selected_market_names: set[str] = set(perp_filter_coins)
    args.scratch_root.mkdir(parents=True, exist_ok=True)
    (args.scratch_root / "downloads").mkdir(parents=True, exist_ok=True)
    manifest = build_manifest(args)
    manifest["commands_or_checks_run"].append(
        "python3 scripts/backfill_hyperliquid_core_to_s3.py " + " ".join(shlex.quote(value) for value in sys.argv[1:])
    )
    meta: Any = {}
    spot_meta: Any = {}

    try:
        meta_record, meta = fetch_and_upload_api_payload(
            scratch_root=args.scratch_root,
            s3_prefix=args.s3_prefix,
            source_family="meta",
            request_body={"type": "meta"},
            partitions={"run": manifest["run_id"]},
            selected_coins=perp_filter_coins,
        )
        manifest["payload_records"].append(meta_record)
        universe = meta.get("universe", []) if isinstance(meta, dict) else []
        universe_names = [item["name"] for item in universe if isinstance(item, dict) and "name" in item]
        active_names = [item["name"] for item in universe if isinstance(item, dict) and "name" in item and not item.get("isDelisted")]
        funding_selected_names = active_names if selected_base_tickers else active_names[: args.funding_coin_limit]
        manifest["universe"] = {
            "source": "official_meta",
            "count": len(universe_names),
            "active_count": len(active_names),
            "delisted_count": len(universe_names) - len(active_names),
            "names": universe_names,
            "active_names": active_names,
            "funding_selected_names": funding_selected_names,
            "selected_coin_filter": sorted(selected_coins),
            "selected_base_ticker_filter": sorted(selected_base_tickers),
        }

        asset_ctx_api_record, asset_ctx_api = fetch_and_upload_api_payload(
            scratch_root=args.scratch_root,
            s3_prefix=args.s3_prefix,
            source_family="metaAndAssetCtxs",
            request_body={"type": "metaAndAssetCtxs"},
            partitions={"run": manifest["run_id"]},
            selected_coins=perp_filter_coins,
        )
        if isinstance(asset_ctx_api, list) and len(asset_ctx_api) == 2 and isinstance(asset_ctx_api[1], list):
            asset_ctx_api_record["asset_context_count"] = len(asset_ctx_api[1])
        manifest["payload_records"].append(asset_ctx_api_record)
    except (urllib.error.URLError, TimeoutError, OSError, subprocess.CalledProcessError, json.JSONDecodeError, KeyError) as exc:
        manifest["errors"].append({"phase": "official_api_meta", "error": repr(exc)})

    if selected_base_tickers:
        try:
            spot_meta_record, spot_meta = fetch_and_upload_api_payload(
                scratch_root=args.scratch_root,
                s3_prefix=args.s3_prefix,
                source_family="spotMeta",
                request_body={"type": "spotMeta"},
                partitions={"run": manifest["run_id"]},
                selected_base_tickers=selected_base_tickers,
            )
            manifest["payload_records"].append(spot_meta_record)
            spot_ctx_record, spot_ctx = fetch_and_upload_api_payload(
                scratch_root=args.scratch_root,
                s3_prefix=args.s3_prefix,
                source_family="spotMetaAndAssetCtxs",
                request_body={"type": "spotMetaAndAssetCtxs"},
                partitions={"run": manifest["run_id"]},
                selected_base_tickers=selected_base_tickers,
            )
            if isinstance(spot_ctx, list) and len(spot_ctx) == 2 and isinstance(spot_ctx[1], list):
                spot_ctx_record["asset_context_count"] = len(spot_ctx[1])
            manifest["payload_records"].append(spot_ctx_record)
        except (urllib.error.URLError, TimeoutError, OSError, subprocess.CalledProcessError, json.JSONDecodeError, KeyError) as exc:
            manifest["errors"].append({"phase": "official_api_spot_meta", "error": repr(exc)})

    coverage = market_catalog(
        perp_meta=meta,
        spot_meta=spot_meta,
        selected_base_tickers=selected_base_tickers,
        selected_coin_names=selected_coins,
    )
    manifest["market_coverage"] = coverage
    if coverage["selected_market_names"]:
        selected_market_names = set(coverage["selected_market_names"])
    archive_market_names = {item["coin"] for item in coverage.get("perpetual", []) if item.get("coin")}
    if not archive_market_names:
        archive_market_names = set(selected_market_names)
    manifest["market_coverage"]["archive_market_names"] = sorted(archive_market_names)

    archive_latest_asset = aws_list_objects(
        HYPERLIQUID_ARCHIVE_BUCKET,
        "asset_ctxs/",
        requester_pays=True,
    )
    if archive_latest_asset:
        latest_asset = archive_latest_asset[-1]
        manifest["source_listing_records"].append({"family": "asset_ctxs_latest_seen", "object": latest_asset})
    latest_market_prefixes = aws_json(
        [
            "s3api",
            "list-objects-v2",
            "--bucket",
            HYPERLIQUID_ARCHIVE_BUCKET,
            "--prefix",
            "market_data/",
            "--delimiter",
            "/",
            "--request-payer",
            "requester",
            "--query",
            "CommonPrefixes[-5:].Prefix",
        ]
    )
    manifest["source_listing_records"].append({"family": "market_data_latest_prefixes_seen", "prefixes": latest_market_prefixes})

    for object_date in select_dates(parse_utc_date(args.asset_ctx_start_date or args.start_date), args.asset_ctx_days):
        key = f"asset_ctxs/{yyyymmdd(object_date)}.csv.lz4"
        uri = source_uri(HYPERLIQUID_ARCHIVE_BUCKET, key)
        try:
            head = aws_head_object(uri, requester_pays=True)
            if archive_market_names:
                record = copy_filtered_asset_ctxs_payload(
                    scratch_root=args.scratch_root,
                    s3_prefix=args.s3_prefix,
                    source_uri=uri,
                    source_head=head,
                    partitions={"date": yyyymmdd(object_date), "markets": safe_partition("_".join(sorted(archive_market_names)))},
                    selected_market_names=archive_market_names,
                    selected_base_tickers=selected_base_tickers,
                )
                missing = sorted(archive_market_names - set(record.get("filtered_market_names") or []))
                if missing:
                    manifest["gaps"].append(
                        {
                            "family": "asset_ctxs",
                            "date": yyyymmdd(object_date),
                            "missing_markets": missing,
                            "reason": "selected archive markets missing from filtered asset_ctxs object",
                        }
                    )
            else:
                record = copy_s3_payload(
                    scratch_root=args.scratch_root,
                    s3_prefix=args.s3_prefix,
                    source_family="asset_ctxs",
                    source_uri=uri,
                    source_head=head,
                    suffix=".csv.lz4",
                    partitions={"date": yyyymmdd(object_date)},
                    requester_pays=True,
                )
            manifest["payload_records"].append(record)
        except (subprocess.CalledProcessError, OSError, json.JSONDecodeError) as exc:
            manifest["gaps"].append({"family": "asset_ctxs", "date": yyyymmdd(object_date), "reason": repr(exc)})
        write_manifest_checkpoint(
            manifest=manifest,
            scratch_root=args.scratch_root,
            s3_prefix=args.s3_prefix,
            checkpoint={"family": "asset_ctxs", "date": yyyymmdd(object_date)},
        )

    for object_date in select_dates(parse_utc_date(args.l2_start_date or args.start_date), args.l2_days):
        date_text = yyyymmdd(object_date)
        date_prefix = f"market_data/{date_text}/"
        try:
            date_probe = aws_list_objects(HYPERLIQUID_ARCHIVE_BUCKET, date_prefix, requester_pays=True, max_keys=1)
        except (subprocess.CalledProcessError, json.JSONDecodeError) as exc:
            for hour in range(args.l2_hours):
                manifest["gaps"].append({"family": "l2Book", "date": date_text, "hour": hour, "reason": repr(exc)})
                write_manifest_checkpoint(
                    manifest=manifest,
                    scratch_root=args.scratch_root,
                    s3_prefix=args.s3_prefix,
                    checkpoint={"family": "l2Book", "date": date_text, "hour": hour},
                    upload_to_s3=False,
                )
            upload_manifest_checkpoints(
                manifest=manifest,
                scratch_root=args.scratch_root,
                s3_prefix=args.s3_prefix,
                include_pattern=f"l2Book_{date_text}_*.json",
                checkpoint_match={"family": "l2Book", "date": date_text},
            )
            continue

        if not date_probe:
            for hour in range(args.l2_hours):
                manifest["gaps"].append(
                    {
                        "family": "l2Book",
                        "date": date_text,
                        "hour": hour,
                        "missing_markets": sorted(archive_market_names),
                        "reason": "source date prefix not listed",
                    }
                )
                manifest["source_listing_records"].append(
                    {
                        "family": "l2Book",
                        "prefix": f"s3://{HYPERLIQUID_ARCHIVE_BUCKET}/market_data/{date_text}/{hour}/l2Book/",
                        "listed_count": 0,
                        "selected_count": 0,
                        "listed_bytes": 0,
                        "selected_bytes": 0,
                        "selected_market_sample": [],
                        "listing_mode": "date_prefix_probe",
                    }
                )
                write_manifest_checkpoint(
                    manifest=manifest,
                    scratch_root=args.scratch_root,
                    s3_prefix=args.s3_prefix,
                    checkpoint={"family": "l2Book", "date": date_text, "hour": hour},
                    upload_to_s3=False,
                )
            upload_manifest_checkpoints(
                manifest=manifest,
                scratch_root=args.scratch_root,
                s3_prefix=args.s3_prefix,
                include_pattern=f"l2Book_{date_text}_*.json",
                checkpoint_match={"family": "l2Book", "date": date_text},
            )
            continue

        selected_by_hour: dict[int, list[dict[str, Any]]] = {
            hour: [
                {"Key": f"market_data/{date_text}/{hour}/l2Book/{coin}.lz4", "coin": coin}
                for coin in sorted(archive_market_names)
            ]
            for hour in range(args.l2_hours)
        }
        selected_for_date = [item for hour in range(args.l2_hours) for item in selected_by_hour[hour]]
        if args.l2_max_objects is not None:
            selected_for_date = selected_for_date[: args.l2_max_objects]
            allowed_keys = {item["Key"] for item in selected_for_date}
            selected_by_hour = {
                hour: [item for item in selected_by_hour[hour] if item["Key"] in allowed_keys]
                for hour in range(args.l2_hours)
            }

        copied_dest_by_key: dict[str, dict[str, Any]] = {}
        copy_results_by_key: dict[str, dict[str, Any]] = {}
        copy_failed_keys: set[str] = set()
        if selected_for_date:
            dest_date_uri = f"{normalized_s3_prefix(args.s3_prefix)}/raw/v1/source_family=l2Book/date={date_text}/"
            dest_bucket, dest_key_prefix = parse_s3_uri(dest_date_uri.rstrip("/") + "/_prefix_marker")
            dest_key_prefix = dest_key_prefix.removesuffix("_prefix_marker")

            def copy_l2_object(item: dict[str, Any]) -> tuple[str, dict[str, Any]]:
                relative_key = str(pathlib.PurePosixPath(str(item["Key"])).relative_to(pathlib.PurePosixPath(date_prefix)))
                dest_key = f"{dest_key_prefix}{relative_key}"
                result = aws_copy_object(
                    source_bucket=HYPERLIQUID_ARCHIVE_BUCKET,
                    source_key=str(item["Key"]),
                    dest_bucket=dest_bucket,
                    dest_key=dest_key,
                    requester_pays=True,
                )
                return dest_key, result

            with concurrent.futures.ThreadPoolExecutor(max_workers=args.copy_workers) as executor:
                futures = {executor.submit(copy_l2_object, item): item for item in selected_for_date}
                for future in concurrent.futures.as_completed(futures):
                    item = futures[future]
                    try:
                        dest_key, result = future.result()
                        copy_results_by_key[dest_key] = result
                    except (subprocess.CalledProcessError, OSError) as exc:
                        copy_failed_keys.add(str(item["Key"]))
                        manifest["gaps"].append(
                            {
                                "family": "l2Book",
                                "date": date_text,
                                "hour": int(pathlib.PurePosixPath(str(item["Key"])).parts[2]),
                                "missing_markets": [str(item["coin"])],
                                "source_uri": source_uri(HYPERLIQUID_ARCHIVE_BUCKET, item["Key"]),
                                "error": repr(exc),
                                "reason": "s3api copy-object failed for selected archive market",
                            }
                        )
            try:
                copied_dest_by_key = {
                    item["Key"]: item
                    for item in aws_list_objects_all(dest_bucket, dest_key_prefix, requester_pays=False)
                }
            except (subprocess.CalledProcessError, json.JSONDecodeError) as exc:
                for item in selected_for_date:
                    manifest["errors"].append(
                        {
                            "family": "l2Book",
                            "source_uri": source_uri(HYPERLIQUID_ARCHIVE_BUCKET, item["Key"]),
                            "error": repr(exc),
                        }
                    )

        for hour in range(args.l2_hours):
            selected = sorted(selected_by_hour[hour], key=lambda item: item["Key"])
            selected_names = {coin_from_l2_key(item["Key"]) for item in selected}
            missing = sorted(archive_market_names - selected_names)
            if missing:
                manifest["gaps"].append(
                    {
                        "family": "l2Book",
                        "date": date_text,
                        "hour": hour,
                        "missing_markets": missing,
                        "reason": "selected archive markets not listed under source prefix",
                    }
                )
            manifest["source_listing_records"].append(
                {
                    "family": "l2Book",
                    "prefix": f"s3://{HYPERLIQUID_ARCHIVE_BUCKET}/market_data/{date_text}/{hour}/l2Book/",
                    "listed_count": None,
                    "selected_count": len(selected),
                    "listed_bytes": None,
                    "selected_bytes": None,
                    "selected_market_sample": [coin_from_l2_key(item["Key"]) for item in selected[:20]],
                    "listing_mode": "copy_object_destination_verification",
                }
            )
            for item in selected:
                key = item["Key"]
                if key in copy_failed_keys:
                    continue
                relative_key = str(pathlib.PurePosixPath(key).relative_to(pathlib.PurePosixPath(date_prefix)))
                dest_uri = f"{normalized_s3_prefix(args.s3_prefix)}/raw/v1/source_family=l2Book/date={date_text}/{relative_key}"
                dest_bucket, dest_key = parse_s3_uri(dest_uri)
                dest_object = copied_dest_by_key.get(dest_key)
                if dest_object is None:
                    manifest["errors"].append({"family": "l2Book", "source_uri": source_uri(HYPERLIQUID_ARCHIVE_BUCKET, key), "error": "copied destination object not listed"})
                    continue
                identity_payload = stable_json(
                    {
                        "source_uri": source_uri(HYPERLIQUID_ARCHIVE_BUCKET, key),
                        "dest_etag": dest_object.get("ETag"),
                        "dest_content_length": dest_object.get("Size"),
                    }
                ).encode("utf-8")
                manifest["payload_records"].append(
                    {
                        "source_family": "l2Book",
                        "source": {
                            "type": "s3api_copy_object",
                            "uri": source_uri(HYPERLIQUID_ARCHIVE_BUCKET, key),
                            "requester_pays": True,
                            "copy_result": copy_results_by_key.get(dest_key),
                        },
                        "local_path": None,
                        "s3_uri": dest_uri,
                        "bytes": int(dest_object.get("Size") if dest_object else item.get("Size") or 0),
                        "sha256": None,
                        "object_identity_sha256": sha256_bytes(identity_payload),
                        "object_identity_scope": "source_uri_dest_etag_content_length",
                        "s3_head_content_length": dest_object.get("Size") if dest_object else None,
                        "s3_head_etag": dest_object.get("ETag") if dest_object else None,
                        "uploaded_at": utc_now(),
                        "scratch_root": str(args.scratch_root),
                    }
                )
            write_manifest_checkpoint(
                manifest=manifest,
                scratch_root=args.scratch_root,
                s3_prefix=args.s3_prefix,
                checkpoint={"family": "l2Book", "date": date_text, "hour": hour},
                upload_to_s3=False,
            )
        upload_manifest_checkpoints(
            manifest=manifest,
            scratch_root=args.scratch_root,
            s3_prefix=args.s3_prefix,
            include_pattern=f"l2Book_{date_text}_*.json",
            checkpoint_match={"family": "l2Book", "date": date_text},
        )

    funding_names = list(manifest.get("universe", {}).get("funding_selected_names") or [])
    if selected_base_tickers:
        funding_names = [coin for coin in funding_names if coin.upper() in selected_base_tickers]
    elif selected_coins:
        funding_names = [coin for coin in funding_names if coin.upper() in selected_coins]
    funding_start_ms = manifest["tranche"]["funding_start_ms"]
    funding_end_ms = manifest["tranche"]["funding_end_ms"]
    for coin in funding_names:
        try:
            records = fetch_funding_pages(
                scratch_root=args.scratch_root,
                s3_prefix=args.s3_prefix,
                coin=coin,
                start_ms=funding_start_ms,
                end_ms=funding_end_ms,
                sleep_seconds=args.api_sleep_seconds,
            )
            manifest["payload_records"].extend(records)
        except (urllib.error.URLError, TimeoutError, OSError, subprocess.CalledProcessError, json.JSONDecodeError, KeyError) as exc:
            manifest["gaps"].append({"family": "fundingHistory", "coin": coin, "reason": repr(exc)})
        write_manifest_checkpoint(
            manifest=manifest,
            scratch_root=args.scratch_root,
            s3_prefix=args.s3_prefix,
            checkpoint={"family": "fundingHistory", "coin": coin},
        )

    if args.node_family:
        for hour in range(args.node_hours):
            key = f"{args.node_family}/hourly/{args.node_date}/{hour}.lz4"
            uri = source_uri(HYPERLIQUID_NODE_BUCKET, key)
            try:
                head = aws_head_object(uri, requester_pays=True)
                if hour == 0:
                    probe = schema_probe_lz4(uri, args.scratch_root, requester_pays=True)
                    manifest["schema_probe_records"].append(probe)
                    if not probe["ok"]:
                        raise ValueError("node schema probe did not produce field names")
                record = copy_s3_payload(
                    scratch_root=args.scratch_root,
                    s3_prefix=args.s3_prefix,
                    source_family=args.node_family,
                    source_uri=uri,
                    source_head=head,
                    suffix=".lz4",
                    partitions={"date": args.node_date, "hour": str(hour)},
                    requester_pays=True,
                )
                manifest["payload_records"].append(record)
            except (subprocess.CalledProcessError, OSError, json.JSONDecodeError, IndexError, ValueError) as exc:
                manifest["gaps"].append({"family": args.node_family, "date": args.node_date, "hour": hour, "reason": repr(exc)})

    if args.probe_node_trades:
        trade_prefix = f"node_trades/hourly/{args.probe_node_trades}/"
        try:
            objects = aws_list_objects(HYPERLIQUID_NODE_BUCKET, trade_prefix, requester_pays=True, max_keys=5)
            manifest["source_listing_records"].append(
                {
                    "family": "node_trades_probe",
                    "prefix": f"s3://{HYPERLIQUID_NODE_BUCKET}/{trade_prefix}",
                    "listed_count": len(objects),
                    "listed_objects": objects,
                    "uploaded": False,
                    "reason": "listing proof only; not uploaded unless schema is promoted for this lane",
                }
            )
        except (subprocess.CalledProcessError, json.JSONDecodeError) as exc:
            manifest["gaps"].append({"family": "node_trades", "date": args.probe_node_trades, "reason": repr(exc)})

    manifest["completed_at"] = utc_now()
    records = list(manifest["payload_records"])
    manifest["completed_object_count"] = len(records)
    manifest["completed_bytes"] = sum(int(item.get("bytes") or 0) for item in records)
    manifest["families_uploaded"] = sorted({item["source_family"] for item in records})
    manifest["remaining_scope"] = {
        "l2Book": "Any requested date/hour gaps are recorded in gaps and bounded by official archive availability.",
        "asset_ctxs": "Any requested date gaps are recorded in gaps and bounded by official archive availability.",
        "fundingHistory": "Requested filtered perpetual coins through the requested end timestamp.",
        "spot": "Current spot metadata and asset contexts only; no source-proven spot historical archive objects were selected by this worker.",
        "node_fills": "Remaining node_fills or node_fills_by_block hours after selected node tranche, after schema policy selection.",
        "node_trades": "Not uploaded in this run; listing proof only unless schema is explicitly accepted for this lane.",
    }
    manifest["commands_or_checks_run"].extend(
        [
            "official API POST /info type=meta",
            "official API POST /info type=metaAndAssetCtxs",
            "official API POST /info type=spotMeta",
            "official API POST /info type=spotMetaAndAssetCtxs",
            "official API POST /info type=fundingHistory for selected official-meta active coins",
        ]
    )

    local_manifest = args.scratch_root / "manifests" / f"{manifest['run_id']}.json"
    manifest_s3_uri = f"{args.s3_prefix}/manifests/v1/run={manifest['run_id']}/hyperliquid-core-backfill-manifest.json"
    manifest["manifest_s3_uri"] = manifest_s3_uri
    manifest["manifest_hash_scope"] = "manifest_without_manifest_sha256"
    manifest_without_hash = dict(manifest)
    manifest_without_hash.pop("manifest_sha256", None)
    manifest_payload = stable_json(manifest_without_hash).encode("utf-8")
    manifest["manifest_sha256"] = sha256_bytes(manifest_payload)
    write_bytes(local_manifest, stable_json(manifest).encode("utf-8"))
    aws_cp_to_s3(local_manifest, manifest_s3_uri)

    summary = {
        "ok": not manifest["errors"],
        "run_id": manifest["run_id"],
        "local_manifest": str(local_manifest),
        "manifest_s3_uri": manifest_s3_uri,
        "completed_object_count": manifest["completed_object_count"],
        "completed_bytes": manifest["completed_bytes"],
        "families_uploaded": manifest["families_uploaded"],
        "gap_count": len(manifest["gaps"]),
        "error_count": len(manifest["errors"]),
    }
    print(stable_json(summary), end="")
    return 0 if not manifest["errors"] else 1


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--scratch-root", required=True, type=pathlib.Path)
    parser.add_argument("--s3-prefix", required=True)
    parser.add_argument("--start-date", required=True)
    parser.add_argument("--end-date", required=True)
    parser.add_argument("--l2-start-date")
    parser.add_argument("--l2-days", type=int, default=1)
    parser.add_argument("--l2-hours", type=int, default=1)
    parser.add_argument("--l2-max-objects", type=int)
    parser.add_argument("--asset-ctx-start-date")
    parser.add_argument("--asset-ctx-days", type=int, default=1)
    parser.add_argument("--funding-days", type=int, default=1)
    parser.add_argument("--funding-coin-limit", type=int, default=20)
    parser.add_argument("--coins")
    parser.add_argument("--base-tickers")
    parser.add_argument("--copy-workers", type=int, default=32)
    parser.add_argument("--api-sleep-seconds", type=float, default=0.05)
    parser.add_argument("--node-family", choices=["node_fills", "node_fills_by_block"])
    parser.add_argument("--node-date", default="20250601")
    parser.add_argument("--node-hours", type=int, default=1)
    parser.add_argument("--probe-node-trades")
    return parser.parse_args()


if __name__ == "__main__":
    raise SystemExit(main())
