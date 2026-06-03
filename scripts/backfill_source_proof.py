#!/usr/bin/env python3
"""Fetch configured backfill source bindings into hashed local artifacts."""

from __future__ import annotations

import argparse
import datetime as dt
import hashlib
import html
import json
import pathlib
import re
import sys
import time
import tomllib
import urllib.error
import urllib.parse
import urllib.request
from typing import Any


USER_AGENT = "bolt-v2-backfill-source-proof/1"


def utc_now() -> str:
    return dt.datetime.now(dt.UTC).replace(microsecond=0).isoformat().replace("+00:00", "Z")


def today_utc() -> str:
    return dt.datetime.now(dt.UTC).strftime("%Y-%m-%d")


def sha256_bytes(payload: bytes) -> str:
    return hashlib.sha256(payload).hexdigest()


def stable_json(value: Any) -> str:
    return json.dumps(value, indent=2, sort_keys=True, ensure_ascii=True) + "\n"


def write_json(path: pathlib.Path, value: Any) -> str:
    payload = stable_json(value).encode("utf-8")
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_bytes(payload)
    return sha256_bytes(payload)


def request_payload(binding: dict[str, Any]) -> tuple[int, dict[str, str], bytes]:
    method = binding["method"].upper()
    body = None
    headers = {"User-Agent": USER_AGENT}
    if method == "POST":
        body = json.dumps(binding.get("request_json", {}), separators=(",", ":")).encode("utf-8")
        headers["Content-Type"] = "application/json"
    request = urllib.request.Request(binding["source_uri"], data=body, headers=headers, method=method)
    with urllib.request.urlopen(request, timeout=60) as response:
        return response.status, dict(response.headers.items()), response.read()


def append_query(uri: str, values: dict[str, str]) -> str:
    parsed = urllib.parse.urlparse(uri)
    query = dict(urllib.parse.parse_qsl(parsed.query, keep_blank_values=True))
    query.update(values)
    return urllib.parse.urlunparse(parsed._replace(query=urllib.parse.urlencode(query)))


def next_bybit_cursor(payload: bytes) -> str | None:
    parsed = json.loads(payload.decode("utf-8"))
    cursor = parsed.get("result", {}).get("nextPageCursor")
    return cursor or None


def html_page_count(payload: bytes) -> int:
    text = payload.decode("utf-8", errors="replace")
    match = re.search(r"Page\s+1\s+of\s+([0-9]+)", text)
    if match:
        return int(match.group(1))
    match = re.search(r'\\"children\\":\[\\"Page \\",1,\\" of \\",([0-9]+)\]', text)
    if match:
        return int(match.group(1))
    return 1


def fetch_binding(binding: dict[str, Any]) -> list[tuple[str, int, dict[str, str], bytes]]:
    payloads = []
    status, headers, payload = request_payload(binding)
    payloads.append((binding["source_uri"], status, headers, payload))
    if binding.get("pagination") == "html_page_query":
        page_count = html_page_count(payload)
        max_pages = int(binding.get("max_pages") or page_count)
        for page in range(2, min(page_count, max_pages) + 1):
            page_binding = dict(binding)
            page_binding["source_uri"] = append_query(binding["source_uri"], {"page": str(page)})
            status, headers, payload = request_payload(page_binding)
            payloads.append((page_binding["source_uri"], status, headers, payload))
        return payloads
    if binding.get("pagination") != "bybit_next_page_cursor":
        return payloads

    seen_cursors: set[str] = set()
    cursor = next_bybit_cursor(payload)
    while cursor:
        if cursor in seen_cursors:
            raise RuntimeError(f"Bybit pagination loop for {binding['key']}: {cursor}")
        seen_cursors.add(cursor)
        page_binding = dict(binding)
        page_binding["source_uri"] = append_query(binding["source_uri"], {"cursor": cursor})
        status, headers, payload = request_payload(page_binding)
        payloads.append((page_binding["source_uri"], status, headers, payload))
        cursor = next_bybit_cursor(payload)
    return payloads


def raw_path(root: pathlib.Path, binding: dict[str, Any], payload_hash: str, extension: str) -> pathlib.Path:
    return (
        root
        / "raw"
        / "v1"
        / f"source_binding={binding['key']}"
        / f"fixture={binding['fixture']}"
        / f"family={binding['family']}"
        / f"dt={today_utc()}"
        / f"object={payload_hash}.{extension}"
    )


def decode_json(payloads: list[bytes]) -> list[Any]:
    return [json.loads(payload.decode("utf-8")) for payload in payloads]


def sample_summary(rows: list[Any], key: str) -> dict[str, Any]:
    sample = []
    for row in rows[:10]:
        sample.append(row.get(key) if isinstance(row, dict) else row)
    return {"count": len(rows), "sample": sample}


def collect_outcomes(value: Any) -> list[dict[str, Any]]:
    if isinstance(value, dict):
        found: list[dict[str, Any]] = []
        for key, item in value.items():
            if key in {"outcomes", "tokens", "universe"} and isinstance(item, list):
                found.extend([row for row in item if isinstance(row, dict)])
            else:
                found.extend(collect_outcomes(item))
        return found
    if isinstance(value, list):
        found = []
        for item in value:
            found.extend(collect_outcomes(item))
        return found
    return []


def collect_questions(value: Any) -> list[Any]:
    if isinstance(value, dict):
        found = []
        for key, item in value.items():
            if key in {"questions", "markets"} and isinstance(item, list):
                found.extend(item)
            else:
                found.extend(collect_questions(item))
        return found
    if isinstance(value, list):
        found = []
        for item in value:
            found.extend(collect_questions(item))
        return found
    return []


def parse_size_text(text: str) -> str | None:
    match = re.search(r"([0-9]+(?:\.[0-9]+)?)\s*(KB|MB|GB|TB)", text)
    return match.group(0) if match else None


def parse_size_bytes(text: str) -> int | None:
    match = re.search(r"([0-9]+(?:\.[0-9]+)?)\s*(KB|MB|GB|TB)", text)
    if not match:
        return None
    scale = {"KB": 1_000, "MB": 1_000_000, "GB": 1_000_000_000, "TB": 1_000_000_000_000}[match.group(2)]
    return int(float(match.group(1)) * scale)


def parse_parquet_links(text: str) -> list[dict[str, Any]]:
    rows = []
    seen = set()
    link_re = re.compile(
        r'href="(?P<url>https://[^"]+\.parquet)"[^>]*>'
        r"(?P<name>[^<]+\.parquet)\s*</a>(?P<trailing>[^<]*(?:<!-- -->[^<]*)*)"
    )
    for match in link_re.finditer(text):
        url = html.unescape(match.group("url"))
        name = html.unescape(match.group("name")).strip()
        if url in seen:
            continue
        seen.add(url)
        trailing = html.unescape(match.group("trailing")).replace("<!-- -->", " ")
        rows.append(
            {
                "url": url,
                "name": name,
                "size_text": parse_size_text(trailing),
                "size_bytes": parse_size_bytes(trailing),
            }
        )
    if rows:
        return rows

    fallback = re.compile(r"https://[A-Za-z0-9._~:/?#\[\]@!$&'()*+,;=%-]+\.parquet")
    for url in fallback.findall(text):
        if url in seen:
            continue
        seen.add(url)
        rows.append({"url": url, "name": url.rsplit("/", 1)[-1], "size_text": None, "size_bytes": None})
    return rows


def summarize(binding: dict[str, Any], payloads: list[bytes]) -> dict[str, Any]:
    extractor = binding["extractor"]
    if extractor == "binance_exchange_info":
        rows = []
        for parsed in decode_json(payloads):
            rows.extend(parsed.get("symbols", []))
        return sample_summary(rows, "symbol")
    if extractor == "okx_data_inst_id":
        rows = []
        for parsed in decode_json(payloads):
            rows.extend(parsed.get("data", []))
        return sample_summary(rows, "instId")
    if extractor == "okx_data_value":
        flat = []
        for parsed in decode_json(payloads):
            for value in parsed.get("data", []):
                flat.extend(value if isinstance(value, list) else [value])
        return {"count": len(flat), "sample": [str(item) for item in flat[:10]]}
    if extractor == "bybit_result_list_symbol":
        rows = []
        for parsed in decode_json(payloads):
            rows.extend(parsed.get("result", {}).get("list", []))
        return sample_summary(rows, "symbol")
    if extractor == "deribit_result_instrument_name":
        rows = []
        for parsed in decode_json(payloads):
            rows.extend(parsed.get("result", []))
        return sample_summary(rows, "instrument_name")
    if extractor == "hyperliquid_meta_universe_name":
        rows = []
        for parsed in decode_json(payloads):
            rows.extend(parsed.get("universe", []))
        return sample_summary(rows, "name")
    if extractor == "hyperliquid_spot_universe_name":
        rows = []
        for parsed in decode_json(payloads):
            rows.extend(parsed.get("universe", []))
        return sample_summary(rows, "name")
    if extractor == "hyperliquid_perp_dexs":
        parsed = decode_json(payloads)[0]
        rows = parsed if isinstance(parsed, list) else []
        names = []
        asset_count = 0
        for row in rows:
            if isinstance(row, dict):
                names.append(row.get("name"))
                asset_count += len(row.get("assetToStreamingOiCap", []))
            elif row is None:
                names.append(None)
        return {"count": len(rows), "asset_count": asset_count, "sample": names[:10]}
    if extractor == "hyperliquid_outcome_meta":
        parsed = decode_json(payloads)[0]
        outcomes = collect_outcomes(parsed)
        quote_tokens = sorted({str(item.get("quoteToken")) for item in outcomes if item.get("quoteToken")})
        return {
            "count": len(outcomes),
            "question_count": len(collect_questions(parsed)),
            "quote_tokens": quote_tokens,
            "sample": [str(item.get("name") or item.get("title") or item.get("outcome")) for item in outcomes[:10]],
        }
    if extractor == "html_parquet_links":
        archive_objects_by_url = {}
        for payload in payloads:
            text = payload.decode("utf-8", errors="replace")
            for archive_object in parse_parquet_links(text):
                archive_objects_by_url[archive_object["url"]] = archive_object
        archive_objects = list(archive_objects_by_url.values())
        return {
            "count": len(archive_objects),
            "sample": [item["name"] for item in archive_objects[:10]],
            "archive_objects": archive_objects,
            "estimated_total_bytes": sum(item.get("size_bytes") or 0 for item in archive_objects),
        }
    raise ValueError(f"Unsupported extractor: {extractor}")


def source_proof_path(root: pathlib.Path, binding: dict[str, Any], proof_id: str) -> pathlib.Path:
    return (
        root
        / "source-proofs"
        / "v1"
        / f"source_binding={binding['key']}"
        / f"fixture={binding['fixture']}"
        / f"proof={proof_id}"
        / "version=1"
        / "source-proof.json"
    )


def build_source_proof(
    binding: dict[str, Any],
    raw_records: list[dict[str, Any]],
    summary: dict[str, Any],
    run_start: str,
    run_end: str,
    generated_at: str,
) -> dict[str, Any]:
    proof_id = "source-proof-" + sha256_bytes((binding["key"] + generated_at).encode("utf-8"))[:16]
    required_checks = {
        "schema_sample": "captured",
        "raw_payload_hash": "passed",
        "time_range": "declared",
        "license": "pending" if binding["evidence_state"] == "pending_source_proof" else "source_specific_review_required",
        "nt_mapping": "pending",
        "fidelity": "pending",
        "forbidden_claims": "pending",
    }
    return {
        "schema_version": "backfill-source-proof.v1",
        "contract_version": "backfill-table-contract.v1",
        "source_proof_id": proof_id,
        "source_proof_version": 1,
        "status": "pending",
        "generated_at": generated_at,
        "source_binding_key": binding["key"],
        "venue": binding["venue"],
        "product_family": binding["product_family"],
        "fixture": binding["fixture"],
        "family": binding["family"],
        "source_uri": binding["source_uri"],
        "source_time_range": {"start_utc": run_start, "end_utc": run_end},
        "evidence_state": binding["evidence_state"],
        "table_families": binding.get("table_families", []),
        "warning": binding.get("warning"),
        "summary": summary,
        "raw_payload_records": raw_records,
        "required_checks": required_checks,
        "forbidden_claims": [
            "Do not claim one-year completeness for any table family not explicitly proven by source coverage.",
            "Do not convert current-only snapshots into historical deltas.",
            "Do not promote pending source proofs into canonical catalog input.",
        ],
    }


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--config", required=True, type=pathlib.Path)
    parser.add_argument("--artifact-root", required=True, type=pathlib.Path)
    parser.add_argument("--start-utc")
    parser.add_argument("--end-utc")
    parser.add_argument("--binding", action="append", default=[])
    return parser.parse_args()


def main() -> int:
    args = parse_args()
    config = tomllib.loads(args.config.read_text())
    run_start = args.start_utc or config["default_window"]["start_utc"]
    run_end = args.end_utc or config["default_window"]["end_utc"]
    selected = set(args.binding)
    bindings = [item for item in config["source_binding"] if not selected or item["key"] in selected]
    generated_at = utc_now()
    run_id = "source-proof-run-" + sha256_bytes((generated_at + str(args.artifact_root)).encode("utf-8"))[:16]

    manifest: dict[str, Any] = {
        "schema_version": "backfill-ingest-manifest.v1",
        "contract_version": config["contract_version"],
        "source_bindings_version": config["schema_version"],
        "run_id": run_id,
        "generated_at": generated_at,
        "artifact_root": str(args.artifact_root),
        "canonical_s3_write": False,
        "write_mode": "local_staging",
        "requested_time_range": {"start_utc": run_start, "end_utc": run_end},
        "raw_payload_records": [],
        "source_proof_records": [],
        "instrument_universe_records": [],
        "errors": [],
    }

    for binding in bindings:
        try:
            fetched = fetch_binding(binding)
            raw_records = []
            payloads = []
            for index, (source_uri, status, headers, payload) in enumerate(fetched, start=1):
                payload_hash = sha256_bytes(payload)
                path = raw_path(args.artifact_root, binding, payload_hash, binding["payload_extension"])
                path.parent.mkdir(parents=True, exist_ok=True)
                path.write_bytes(payload)
                record = {
                    "source_binding": binding["key"],
                    "venue": binding["venue"],
                    "product_family": binding["product_family"],
                    "family": binding["family"],
                    "source_uri": source_uri,
                    "http_status": status,
                    "content_type": headers.get("Content-Type") or headers.get("content-type"),
                    "payload_hash": payload_hash,
                    "bytes": len(payload),
                    "page": index,
                    "uri": str(path),
                }
                raw_records.append(record)
                manifest["raw_payload_records"].append(record)
                payloads.append(payload)

            summary = summarize(binding, payloads)
            proof = build_source_proof(binding, raw_records, summary, run_start, run_end, generated_at)
            proof_path = source_proof_path(args.artifact_root, binding, proof["source_proof_id"])
            proof_hash = write_json(proof_path, proof)
            manifest["source_proof_records"].append(
                {
                    "source_binding": binding["key"],
                    "source_proof_id": proof["source_proof_id"],
                    "source_proof_version": 1,
                    "status": proof["status"],
                    "payload_hash": proof_hash,
                    "uri": str(proof_path),
                }
            )
            universe = {
                "venue": binding["venue"],
                "product_family": binding["product_family"],
                "source_binding": binding["key"],
                "count": summary.get("count"),
                "sample": summary.get("sample", []),
                "raw_payload_hashes": [record["payload_hash"] for record in raw_records],
                "source_proof_id": proof["source_proof_id"],
                "evidence_state": binding["evidence_state"],
            }
            if "estimated_total_bytes" in summary:
                universe["estimated_total_bytes"] = summary["estimated_total_bytes"]
            if binding.get("warning"):
                universe["warning"] = binding["warning"]
            manifest["instrument_universe_records"].append(universe)
            time.sleep(0.15)
        except (urllib.error.URLError, TimeoutError, RuntimeError, ValueError, json.JSONDecodeError) as exc:
            manifest["errors"].append({"source_binding": binding.get("key"), "error": repr(exc)})

    manifest_path = args.artifact_root / "ingest-manifests" / "v1" / f"run={run_id}" / "manifest.json"
    manifest_hash = write_json(manifest_path, manifest)
    print(
        stable_json(
            {
                "ok": not manifest["errors"],
                "artifact_root": str(args.artifact_root),
                "manifest_path": str(manifest_path),
                "manifest_hash": manifest_hash,
                "raw_payload_records": len(manifest["raw_payload_records"]),
                "source_proof_records": len(manifest["source_proof_records"]),
                "universe_records": len(manifest["instrument_universe_records"]),
                "errors": manifest["errors"],
            }
        ),
        end="",
    )
    return 0 if not manifest["errors"] else 1


if __name__ == "__main__":
    raise SystemExit(main())
