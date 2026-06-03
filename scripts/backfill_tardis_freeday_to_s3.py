#!/usr/bin/env python3
"""Stage a Tardis.dev FREE first-of-month dataset into the Deribit S3 staging area.

Deribit partners with Tardis.dev so that the *first day of each month* of Tardis's
historical datasets is downloadable WITHOUT an API key. That free day includes the
data Deribit's own public API does NOT serve historically: full-granularity order
book, top-of-book quotes, and the consolidated options chain with greeks + IV + OI.

This runner downloads one such free daily CSV.gz dataset and stages it RAW (gzip,
unmodified) into the locked Deribit staging prefix under a `tardis_<data_type>`
family, content-addressed by sha256, with a provenance manifest. It does not parse
or filter the file — the all-currency raw artifact is preserved faithfully, exactly
like the sibling raw-staging runners; scoping to in-scope bases happens later in the
normalization phase.

Datasets API URL (first-of-month is keyless):
  https://datasets.tardis.dev/v1/{exchange}/{data_type}/{yyyy}/{mm}/{dd}/{symbol}.csv.gz

Secrets: ambient AWS CLI only (no SSM, no keys), like the sibling staging scripts.
"""

from __future__ import annotations

import argparse
import datetime as dt
import gzip
import hashlib
import json
import pathlib
import subprocess
import urllib.parse
import urllib.request
from typing import Any


APPROVED_S3_PREFIX = "s3://bolt-parquet/backfill-staging/2026-06-01/deribit"
MANIFEST_SCHEMA_VERSION = "deribit-tardis-freeday-staging-manifest.v1"
DATASETS_BASE = "https://datasets.tardis.dev/v1"
RUN_ID_PREFIX = "deribit-tardis-freeday-"


def utc_now() -> str:
    return dt.datetime.now(dt.timezone.utc).replace(microsecond=0).isoformat().replace("+00:00", "Z")


def stable_json(value: Any) -> str:
    return json.dumps(value, indent=2, sort_keys=True, ensure_ascii=True) + "\n"


def sha256_bytes(payload: bytes) -> str:
    return hashlib.sha256(payload).hexdigest()


def sha256_file_streaming(path: pathlib.Path) -> tuple[str, int]:
    digest = hashlib.sha256()
    total = 0
    with open(path, "rb") as handle:
        for chunk in iter(lambda: handle.read(8 << 20), b""):
            digest.update(chunk)
            total += len(chunk)
    return digest.hexdigest(), total


def normalize_s3_prefix(prefix: str) -> str:
    normalized = prefix.rstrip("/")
    if normalized != APPROVED_S3_PREFIX:
        raise ValueError(f"S3 prefix must be exactly {APPROVED_S3_PREFIX}")
    return normalized


def manifest_payload(manifest: dict[str, Any]) -> bytes:
    """Self-referential fixpoint hash, byte-compatible with the sibling staging scripts."""
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


def dataset_url(exchange: str, data_type: str, date: str, symbol: str) -> str:
    y, m, d = date.split("-")
    return f"{DATASETS_BASE}/{exchange}/{data_type}/{y}/{m}/{d}/{symbol}.csv.gz"


def download(url: str, dest: pathlib.Path) -> None:
    dest.parent.mkdir(parents=True, exist_ok=True)
    # curl streams to disk; -f makes HTTP errors fail loud.
    subprocess.run(["curl", "-sf", "-o", str(dest), url], check=True)


def csv_gz_header_and_rowcount(path: pathlib.Path) -> tuple[list[str], int]:
    """Read the gzip CSV header and count data rows (streaming, no full decompress to disk)."""
    rows = 0
    header: list[str] = []
    with gzip.open(path, "rt", encoding="utf-8", newline="") as fh:
        for i, line in enumerate(fh):
            if i == 0:
                header = line.rstrip("\n").split(",")
            else:
                rows += 1
    return header, rows


def upload_to_s3(local_path: pathlib.Path, s3_uri: str) -> None:
    subprocess.run(["aws", "s3", "cp", str(local_path), s3_uri, "--only-show-errors"], check=True)


def s3_object_exists(s3_uri: str) -> bool:
    parsed = urllib.parse.urlparse(s3_uri)
    completed = subprocess.run(
        ["aws", "s3api", "head-object", "--bucket", parsed.netloc, "--key", parsed.path.lstrip("/")],
        check=False, capture_output=True, text=True,
    )
    return completed.returncode == 0


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument("--exchange", default="deribit")
    parser.add_argument("--data-type", default="options_chain",
                        help="Tardis data type, e.g. options_chain, incremental_book_L2, quotes, trades.")
    parser.add_argument("--date", required=True, help="YYYY-MM-DD; must be the first of a month for the free dataset.")
    parser.add_argument("--symbol", default="OPTIONS", help="Tardis symbol (OPTIONS for the consolidated options chain).")
    parser.add_argument("--local-file", type=pathlib.Path,
                        help="Use an already-downloaded .csv.gz instead of downloading.")
    parser.add_argument("--row-count", type=int, default=None,
                        help="Optional precomputed data-row count (skips the streaming re-count of a huge file).")
    parser.add_argument("--s3-prefix", default=APPROVED_S3_PREFIX)
    parser.add_argument("--scratch-root", type=pathlib.Path,
                        default=pathlib.Path("/private/tmp/bolt-v2-tardis-freeday"))
    args = parser.parse_args()

    s3_prefix = normalize_s3_prefix(args.s3_prefix)
    y, m, d = args.date.split("-")
    if (m, d) and d != "01":
        # The free dataset is the FIRST of the month; refuse a non-first date loudly
        # so we never silently 404 against a paid day.
        raise SystemExit(f"--date must be the first of a month for the free dataset; got {args.date}")

    url = dataset_url(args.exchange, args.data_type, args.date, args.symbol)
    if args.local_file:
        local = args.local_file
        if not local.exists():
            raise SystemExit(f"--local-file not found: {local}")
    else:
        local = args.scratch_root / f"{args.exchange}-{args.data_type}-{args.date}-{args.symbol}.csv.gz"
        print(f"downloading {url} -> {local}", flush=True)
        download(url, local)

    print("hashing (streaming)...", flush=True)
    file_sha256, compressed_bytes = sha256_file_streaming(local)

    print("reading header + row count...", flush=True)
    header, counted_rows = ([], None)
    if args.row_count is not None:
        with gzip.open(local, "rt", encoding="utf-8", newline="") as fh:
            header = fh.readline().rstrip("\n").split(",")
        data_rows = args.row_count
    else:
        header, data_rows = csv_gz_header_and_rowcount(local)

    # Content-addressed key under the locked prefix.
    parts = [
        s3_prefix, "raw", "v1",
        f"family=tardis_{args.data_type}",
        f"exchange={args.exchange}",
        f"data_type={args.data_type}",
        f"date={args.date}",
        f"symbol={args.symbol}",
        f"object={file_sha256}.csv.gz",
    ]
    s3_uri = "/".join(parts)
    run_seed = stable_json({
        "s3_prefix": s3_prefix, "exchange": args.exchange, "data_type": args.data_type,
        "date": args.date, "symbol": args.symbol, "schema_version": MANIFEST_SCHEMA_VERSION,
    })
    run_id = RUN_ID_PREFIX + sha256_bytes(run_seed.encode("utf-8"))[:16]

    print(f"uploading {compressed_bytes} bytes -> {s3_uri}", flush=True)
    upload_to_s3(local, s3_uri)
    if not s3_object_exists(s3_uri):
        raise SystemExit(f"upload verification failed (head-object 404): {s3_uri}")

    record = {
        "family": f"tardis_{args.data_type}",
        "exchange": args.exchange,
        "data_type": args.data_type,
        "date": args.date,
        "symbol": args.symbol,
        "source_url": url,
        "content_type": "application/gzip",
        "compressed_bytes": compressed_bytes,
        "data_row_count": data_rows,
        "columns": header,
        "sha256": file_sha256,
        "s3_uri": s3_uri,
    }
    manifest: dict[str, Any] = {
        "schema_version": MANIFEST_SCHEMA_VERSION,
        "run_id": run_id,
        "generated_at": utc_now(),
        "runner": pathlib.Path(__file__).name,
        "s3_prefix": s3_prefix,
        "source": {
            "vendor": "Tardis.dev",
            "datasets_api": "https://datasets.tardis.dev/v1",
            "free_first_of_month": True,
            "partnership": "Deribit<->Tardis free historical data: first day of each month, no API key",
            "exchange": args.exchange,
            "note": (
                "RAW unmodified gzip CSV as served by Tardis (all currencies, tick-level). "
                "options_chain carries best bid/ask + amounts + iv, mark price + mark_iv, "
                "open_interest, and greeks (delta/gamma/vega/theta/rho) + underlying_price per option. "
                "Scoping to in-scope bases (BTC/ETH/SOL/XRP) happens in the normalization phase."
            ),
        },
        "write_policy": {"staging_only": True, "canonical_writes": False, "secrets_required": False},
        "records": [record],
        "errors": [],
        "object_count_excluding_manifest": 1,
        "bytes_excluding_manifest": compressed_bytes,
    }
    manifest_s3_uri = f"{s3_prefix}/ingest-manifests/v1/run={run_id}/deribit-tardis-freeday-manifest.json"
    manifest["manifest_s3_uri"] = manifest_s3_uri
    manifest["total_s3_object_count_including_manifest"] = 2

    payload = manifest_payload(manifest)
    local_manifest = args.scratch_root / f"{run_id}.manifest.json"
    local_manifest.parent.mkdir(parents=True, exist_ok=True)
    local_manifest.write_bytes(payload)
    upload_to_s3(local_manifest, manifest_s3_uri)

    print(stable_json({
        "run_id": run_id,
        "s3_uri": s3_uri,
        "manifest_s3_uri": manifest_s3_uri,
        "sha256": file_sha256,
        "compressed_bytes": compressed_bytes,
        "data_row_count": data_rows,
        "columns": header,
        "manifest_sha256": manifest["manifest_sha256"],
    }))
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
