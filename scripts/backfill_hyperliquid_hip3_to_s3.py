#!/usr/bin/env python3
"""Fetch Hyperliquid HIP-3 source proofs and stage them in S3."""

from __future__ import annotations

import argparse
import datetime as dt
import hashlib
import json
import pathlib
import re
import subprocess
import time
import urllib.error
import urllib.request
from typing import Any


INFO_URL = "https://api.hyperliquid.xyz/info"
ALLOWED_S3_PREFIX = "s3://bolt-parquet/backfill-staging/2026-06-01/hyperliquid-hip3"
SCRATCH_NAME_PREFIX = "bolt-v2-hyperliquid-hip3-backfill-"
USER_AGENT = "bolt-v2-hyperliquid-hip3-backfill/1"
PRODUCT_FAMILY = "hip3_perpetual"
VENUE = "hyperliquid"
SOURCE_DOCS = [
    "https://hyperliquid.gitbook.io/hyperliquid-docs/for-developers/api/info-endpoint/perpetuals",
    "https://hyperliquid.gitbook.io/hyperliquid-docs/for-developers/api/asset-ids",
]


def stable_json(value: Any) -> str:
    return json.dumps(value, indent=2, sort_keys=True, ensure_ascii=True) + "\n"


def compact_json(value: Any) -> str:
    return json.dumps(value, separators=(",", ":"), sort_keys=True, ensure_ascii=True)


def utc_now() -> str:
    return dt.datetime.now(dt.UTC).replace(microsecond=0).isoformat().replace("+00:00", "Z")


def parse_csv_set(value: str | None) -> set[str] | None:
    if not value:
        return None
    return {item.strip().upper() for item in value.split(",") if item.strip()}


def instrument_matches_base_ticker(row: dict[str, Any], tickers: set[str] | None) -> bool:
    if not tickers:
        return True
    for key in ("local_instrument_name", "instrument_name", "coin"):
        upper = str(row.get(key) or "").upper()
        _, _, local = upper.partition(":")
        if upper in tickers or local in tickers:
            return True
        if any(upper.startswith(f"{ticker}-") or local.startswith(f"{ticker}-") for ticker in tickers):
            return True
    return False


def sha256_bytes(payload: bytes) -> str:
    return hashlib.sha256(payload).hexdigest()


def parse_utc(value: str) -> dt.datetime:
    if not value.endswith("Z"):
        raise ValueError(f"UTC timestamp must end in Z: {value}")
    parsed = dt.datetime.fromisoformat(value[:-1] + "+00:00")
    return parsed.astimezone(dt.UTC)


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
    safe = re.sub(r"[^A-Za-z0-9._=-]+", "_", value)
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
                parsed = json.loads(payload)
                return response.status, headers, payload, parsed
        except urllib.error.HTTPError as exc:
            if exc.code not in {429, 500, 502, 503, 504} or attempt >= max_retries:
                raise
            time.sleep(retry_base_sleep_seconds * (2**attempt))
            attempt += 1


def upload_file(local: pathlib.Path, target: str) -> None:
    subprocess.run(["aws", "s3", "cp", str(local), target, "--only-show-errors"], check=True)


def upload_record(local: pathlib.Path, target: str) -> dict[str, Any]:
    upload_file(local, target)
    stat = local.stat()
    return {
        "local_path": str(local),
        "s3_uri": target,
        "bytes": stat.st_size,
        "sha256": sha256_bytes(local.read_bytes()),
    }


def source_family_slug(body: dict[str, Any]) -> str:
    request_type = str(body["type"])
    if request_type == "metaAndAssetCtxs":
        return f"info.metaAndAssetCtxs.dex={partition_value(str(body['dex']))}"
    if request_type == "fundingHistory":
        return f"info.fundingHistory.coin={partition_value(str(body['coin']))}"
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
    path = local_path(
        root,
        "raw",
        "v1",
        f"source_family={partition_value(family)}",
        f"run={run_id}",
        f"payload={payload_hash}.json",
    )
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


def current_metadata_from_perp_dexs(perp_dexs: list[Any]) -> list[dict[str, Any]]:
    records = []
    for dex_index, item in enumerate(perp_dexs):
        if item is None:
            continue
        if not isinstance(item, dict):
            raise ValueError(f"unexpected perpDexs item at index {dex_index}: {type(item).__name__}")
        records.append(
            {
                "venue": VENUE,
                "product_family": PRODUCT_FAMILY,
                "dex_index": dex_index,
                "dex_name": item.get("name"),
                "full_name": item.get("fullName"),
                "deployer": item.get("deployer"),
                "oracle_updater": item.get("oracleUpdater"),
                "fee_recipient": item.get("feeRecipient"),
                "streaming_open_interest_cap_count": len(item.get("assetToStreamingOiCap", [])),
                "funding_multiplier_count": len(item.get("assetToFundingMultiplier", [])),
                "raw": item,
            }
        )
    return records


def instruments_from_meta_ctx(
    dex_record: dict[str, Any],
    meta: dict[str, Any],
    contexts: list[Any],
    generated_at: str,
) -> list[dict[str, Any]]:
    rows = []
    universe = meta.get("universe", [])
    if not isinstance(universe, list):
        raise ValueError(f"universe is not a list for dex {dex_record['dex_name']}")
    for instrument_index, instrument in enumerate(universe):
        if not isinstance(instrument, dict):
            continue
        context = contexts[instrument_index] if instrument_index < len(contexts) else None
        instrument_name = str(instrument.get("name"))
        dex_prefix, _, local_name = instrument_name.partition(":")
        rows.append(
            {
                "venue": VENUE,
                "product_family": PRODUCT_FAMILY,
                "dex_name": dex_record["dex_name"],
                "dex_index": dex_record["dex_index"],
                "instrument_index": instrument_index,
                "asset_id": 100000 + int(dex_record["dex_index"]) * 10000 + instrument_index,
                "instrument_name": instrument_name,
                "dex_prefix_from_name": dex_prefix if local_name else None,
                "local_instrument_name": local_name if local_name else instrument_name,
                "is_delisted": bool(instrument.get("isDelisted", False)),
                "sz_decimals": instrument.get("szDecimals"),
                "max_leverage": instrument.get("maxLeverage"),
                "margin_table_id": instrument.get("marginTableId"),
                "margin_mode": instrument.get("marginMode"),
                "growth_mode": instrument.get("growthMode"),
                "last_growth_mode_change_time": instrument.get("lastGrowthModeChangeTime"),
                "collateral_token": meta.get("collateralToken"),
                "current_context": context if isinstance(context, dict) else None,
                "snapshot_at": generated_at,
                "raw_instrument": instrument,
            }
        )
    return rows


def funding_rows_from_payload(
    dex_name: str,
    instrument_name: str,
    requested_start: str,
    requested_end: str,
    payload: Any,
) -> list[dict[str, Any]]:
    if not isinstance(payload, list):
        raise ValueError(f"fundingHistory payload for {instrument_name} is not a list")
    rows = []
    for item in payload:
        if not isinstance(item, dict):
            continue
        row_time = item.get("time")
        rows.append(
            {
                "venue": VENUE,
                "product_family": PRODUCT_FAMILY,
                "dex_name": dex_name,
                "instrument_name": instrument_name,
                "source_family": "info.fundingHistory",
                "requested_start_utc": requested_start,
                "requested_end_utc": requested_end,
                "time": row_time,
                "time_utc": utc_from_millis(row_time) if isinstance(row_time, int) else None,
                "coin": item.get("coin"),
                "funding_rate": item.get("fundingRate"),
                "premium": item.get("premium"),
                "raw": item,
            }
        )
    return rows


def coverage_record(
    dex_name: str,
    instrument_name: str,
    requested_start: str,
    requested_end: str,
    raw_record: dict[str, Any],
    payload: Any,
) -> dict[str, Any]:
    rows = payload if isinstance(payload, list) else []
    times = [item.get("time") for item in rows if isinstance(item, dict) and isinstance(item.get("time"), int)]
    first_time = min(times) if times else None
    last_time = max(times) if times else None
    return {
        "venue": VENUE,
        "product_family": PRODUCT_FAMILY,
        "dex_name": dex_name,
        "instrument_name": instrument_name,
        "source_family": "info.fundingHistory",
        "requested_start_utc": requested_start,
        "requested_end_utc": requested_end,
        "row_count": len(rows),
        "first_time": first_time,
        "first_time_utc": utc_from_millis(first_time),
        "last_time": last_time,
        "last_time_utc": utc_from_millis(last_time),
        "raw_payload_hash": raw_record["payload_hash"],
        "raw_bytes": raw_record["bytes"],
        "coverage_statement": (
            "bounded_response_rows_only"
            if rows
            else "no_rows_returned_for_requested_bounded_range"
        ),
    }


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--s3-prefix", required=True)
    parser.add_argument("--scratch-root", required=True, type=pathlib.Path)
    parser.add_argument("--window-start-utc", required=True)
    parser.add_argument("--window-end-utc", required=True)
    parser.add_argument("--funding-start-utc", required=True)
    parser.add_argument("--funding-end-utc", required=True)
    parser.add_argument("--max-funding-instruments", type=int)
    parser.add_argument("--request-sleep-seconds", type=float, default=0.05)
    parser.add_argument("--max-retries", type=int, default=6)
    parser.add_argument("--retry-base-sleep-seconds", type=float, default=1.0)
    parser.add_argument("--base-ticker-filter", help="Comma-separated base tickers, for example BTC,ETH,SOL.")
    return parser.parse_args()


def main() -> int:
    args = parse_args()
    s3_prefix = require_allowed_s3_prefix(args.s3_prefix)
    scratch_root = require_scratch_root(args.scratch_root)
    window_start_ms = millis(args.window_start_utc)
    window_end_ms = millis(args.window_end_utc)
    funding_start_ms = millis(args.funding_start_utc)
    funding_end_ms = millis(args.funding_end_utc)
    if not (window_start_ms <= funding_start_ms < funding_end_ms <= window_end_ms):
        raise ValueError("funding tranche must be within the requested UTC window")

    generated_at = utc_now()
    run_seed = f"{generated_at}|{s3_prefix}|{args.funding_start_utc}|{args.funding_end_utc}"
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

    perp_dexs_raw, perp_dexs = fetch_and_record({"type": "perpDexs"})
    if not isinstance(perp_dexs, list):
        raise ValueError("perpDexs response is not a list")
    all_perp_metas_raw, all_perp_metas = fetch_and_record({"type": "allPerpMetas"})
    if not isinstance(all_perp_metas, list):
        raise ValueError("allPerpMetas response is not a list")

    dex_records = current_metadata_from_perp_dexs(perp_dexs)
    instrument_rows: list[dict[str, Any]] = []
    meta_ctx_raw_by_dex: dict[str, dict[str, Any]] = {}
    meta_ctx_counts: list[dict[str, Any]] = []

    for dex_record in dex_records:
        dex_name = str(dex_record["dex_name"])
        raw_record, parsed = fetch_and_record({"type": "metaAndAssetCtxs", "dex": dex_name})
        meta_ctx_raw_by_dex[dex_name] = raw_record
        if not (isinstance(parsed, list) and len(parsed) == 2 and isinstance(parsed[0], dict) and isinstance(parsed[1], list)):
            raise ValueError(f"unexpected metaAndAssetCtxs response for dex {dex_name}")
        rows = instruments_from_meta_ctx(dex_record, parsed[0], parsed[1], generated_at)
        instrument_rows.extend(rows)
        meta_ctx_counts.append(
            {
                "dex_name": dex_name,
                "dex_index": dex_record["dex_index"],
                "universe_count": len(parsed[0].get("universe", [])),
                "asset_context_count": len(parsed[1]),
                "raw_payload_hash": raw_record["payload_hash"],
            }
        )
        time.sleep(args.request_sleep_seconds)

    base_tickers = parse_csv_set(args.base_ticker_filter)
    if base_tickers:
        instrument_rows = [row for row in instrument_rows if instrument_matches_base_ticker(row, base_tickers)]

    selected_instruments = instrument_rows
    if args.max_funding_instruments is not None:
        selected_instruments = selected_instruments[: args.max_funding_instruments]

    funding_rows: list[dict[str, Any]] = []
    funding_coverage: list[dict[str, Any]] = []
    for row in selected_instruments:
        body = {
            "type": "fundingHistory",
            "coin": row["instrument_name"],
            "startTime": funding_start_ms,
            "endTime": funding_end_ms,
        }
        try:
            raw_record, parsed = fetch_and_record(body)
            funding_rows.extend(
                funding_rows_from_payload(
                    str(row["dex_name"]),
                    str(row["instrument_name"]),
                    args.funding_start_utc,
                    args.funding_end_utc,
                    parsed,
                )
            )
            funding_coverage.append(
                coverage_record(
                    str(row["dex_name"]),
                    str(row["instrument_name"]),
                    args.funding_start_utc,
                    args.funding_end_utc,
                    raw_record,
                    parsed,
                )
            )
        except Exception as exc:  # noqa: BLE001 - manifest records per-instrument source failures.
            errors.append({"source_family": "info.fundingHistory", "instrument_name": row["instrument_name"], "error": repr(exc)})
        time.sleep(args.request_sleep_seconds)

    dex_artifact = write_json(
        local_path(scratch_root, "staged", "v1", f"table=dex_universe", f"run={run_id}", "part-000000.json"),
        dex_records,
    )
    instrument_artifact = write_jsonl(
        local_path(scratch_root, "staged", "v1", f"table=instrument_universe", f"run={run_id}", "part-000000.jsonl"),
        instrument_rows,
    )
    funding_artifact = write_jsonl(
        local_path(scratch_root, "staged", "v1", f"table=funding_rates", f"run={run_id}", "part-000000.jsonl"),
        funding_rows,
    )
    coverage_artifact = write_json(
        local_path(scratch_root, "source-proof", "v1", f"run={run_id}", "funding-coverage.json"),
        funding_coverage,
    )

    source_proof = {
        "schema_version": "hyperliquid-hip3-source-proof.v1",
        "run_id": run_id,
        "generated_at": generated_at,
        "venue": VENUE,
        "product_family": PRODUCT_FAMILY,
        "requested_window": {"start_utc": args.window_start_utc, "end_utc": args.window_end_utc},
        "funding_tranche": {"start_utc": args.funding_start_utc, "end_utc": args.funding_end_utc},
        "official_source_docs": SOURCE_DOCS,
        "official_api_url": INFO_URL,
        "source_families_uploaded": [
            "info.perpDexs",
            "info.allPerpMetas",
            "info.metaAndAssetCtxs",
            "info.fundingHistory",
        ],
        "raw_payload_records": raw_records,
        "dex_count": len(dex_records),
        "instrument_count": len(instrument_rows),
        "funding_instrument_request_count": len(selected_instruments),
        "funding_row_count": len(funding_rows),
        "funding_coverage": funding_coverage,
        "current_metadata_counts": meta_ctx_counts,
        "gaps_and_unproven_families": [
            "all-fills archive parsing is pending",
            "dex-qualified official archive coverage is pending",
            "one-year HIP-3 level-two replay is not claimed",
            "order book deltas are not uploaded",
            "trades are not uploaded",
            "funding upload is limited to the bounded tranche named in this proof",
            "empty funding responses prove no rows returned for that request only, not listing age",
        ],
    }
    source_proof_artifact = write_json(
        local_path(scratch_root, "source-proof", "v1", f"run={run_id}", "source-proof.json"),
        source_proof,
    )

    local_artifacts = [
        (
            pathlib.Path(perp_dexs_raw["local_path"]),
            s3_uri(s3_prefix, "raw", "v1", "source_family=info.perpDexs", f"run={run_id}", pathlib.Path(perp_dexs_raw["local_path"]).name),
        ),
        (
            pathlib.Path(all_perp_metas_raw["local_path"]),
            s3_uri(s3_prefix, "raw", "v1", "source_family=info.allPerpMetas", f"run={run_id}", pathlib.Path(all_perp_metas_raw["local_path"]).name),
        ),
    ]
    for record in raw_records:
        local = pathlib.Path(record["local_path"])
        if local in {item[0] for item in local_artifacts}:
            continue
        local_artifacts.append(
            (
                local,
                s3_uri(
                    s3_prefix,
                    "raw",
                    "v1",
                    f"source_family={partition_value(str(record['source_family']))}",
                    f"run={run_id}",
                    local.name,
                ),
            )
        )
    local_artifacts.extend(
        [
            (
                pathlib.Path(dex_artifact["path"]),
                s3_uri(s3_prefix, "staged", "v1", "table=dex_universe", f"run={run_id}", "part-000000.json"),
            ),
            (
                pathlib.Path(instrument_artifact["path"]),
                s3_uri(s3_prefix, "staged", "v1", "table=instrument_universe", f"run={run_id}", "part-000000.jsonl"),
            ),
            (
                pathlib.Path(funding_artifact["path"]),
                s3_uri(s3_prefix, "staged", "v1", "table=funding_rates", f"run={run_id}", "part-000000.jsonl"),
            ),
            (
                pathlib.Path(coverage_artifact["path"]),
                s3_uri(s3_prefix, "source-proof", "v1", f"run={run_id}", "funding-coverage.json"),
            ),
            (
                pathlib.Path(source_proof_artifact["path"]),
                s3_uri(s3_prefix, "source-proof", "v1", f"run={run_id}", "source-proof.json"),
            ),
        ]
    )

    for local, target in local_artifacts:
        uploads.append(upload_record(local, target))

    manifest_s3 = s3_uri(s3_prefix, "manifests", "v1", f"run={run_id}", "manifest.json")
    manifest = {
        "schema_version": "hyperliquid-hip3-s3-staging-manifest.v1",
        "run_id": run_id,
        "generated_at": generated_at,
        "completed_at": utc_now(),
        "venue": VENUE,
        "product_family": PRODUCT_FAMILY,
        "s3_prefix": s3_prefix + "/",
        "canonical_s3_write": False,
        "write_mode": "s3_staging",
        "requested_window": {"start_utc": args.window_start_utc, "end_utc": args.window_end_utc},
        "funding_tranche": {"start_utc": args.funding_start_utc, "end_utc": args.funding_end_utc},
        "source_families_uploaded": source_proof["source_families_uploaded"],
        "counts": {
            "dex_records": len(dex_records),
            "instrument_records": len(instrument_rows),
            "funding_instruments_requested": len(selected_instruments),
            "funding_rows": len(funding_rows),
            "raw_payloads": len(raw_records),
            "uploaded_objects_without_manifest": len(uploads),
            "errors": len(errors),
        },
        "bytes": {
            "raw_payload_bytes": sum(record["bytes"] for record in raw_records),
            "staged_dex_bytes": dex_artifact["bytes"],
            "staged_instrument_bytes": instrument_artifact["bytes"],
            "staged_funding_bytes": funding_artifact["bytes"],
            "source_proof_bytes": source_proof_artifact["bytes"],
            "funding_coverage_bytes": coverage_artifact["bytes"],
            "uploaded_bytes_without_manifest": sum(record["bytes"] for record in uploads),
        },
        "uploads": uploads,
        "local_artifacts_root": str(scratch_root),
        "manifest_s3_uri": manifest_s3,
        "manifest_hash_scope": "manifest_without_manifest_sha256",
        "source_proof_s3_uri": s3_uri(s3_prefix, "source-proof", "v1", f"run={run_id}", "source-proof.json"),
        "funding_coverage_s3_uri": s3_uri(s3_prefix, "source-proof", "v1", f"run={run_id}", "funding-coverage.json"),
        "staged_dex_s3_uri": s3_uri(s3_prefix, "staged", "v1", "table=dex_universe", f"run={run_id}", "part-000000.json"),
        "staged_instrument_s3_uri": s3_uri(s3_prefix, "staged", "v1", "table=instrument_universe", f"run={run_id}", "part-000000.jsonl"),
        "staged_funding_s3_uri": s3_uri(s3_prefix, "staged", "v1", "table=funding_rates", f"run={run_id}", "part-000000.jsonl"),
        "gaps_and_unproven_families": source_proof["gaps_and_unproven_families"],
        "errors": errors,
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
        "gaps_and_unproven_families": manifest["gaps_and_unproven_families"],
        "local_artifacts_root": str(scratch_root),
    }
    print(stable_json(summary), end="")
    return 0 if not errors else 1


if __name__ == "__main__":
    raise SystemExit(main())
