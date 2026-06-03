#!/usr/bin/env python3
"""Stage the user-provided Chainlink up/down 5-minute cycle parquet into S3 staging
and write a content-digested provenance manifest.

The dataset is a local mirror tree:
    <local_root>/<asset>-5m-cycles/dt=YYYY-MM-DD/<market_slug>.parquet
Each parquet holds one 5-minute up/down cycle's 1-second reference prices
(Chainlink Data Streams 1m anchor, Binance-filled). `dt` is a Hive partition key
in the path, not an in-file column.

This is a standalone one-off staging utility that runs OUTSIDE the Rust binary; it
uses the ambient AWS CLI for object I/O exactly like the sibling backfill_*_to_s3.py
scripts. It does not touch SSM and never handles secrets.

Two object trees live under the locked S3 prefix:
  1. Mirror parquet:  {prefix}/<asset>-5m-cycles/dt=YYYY-MM-DD/<slug>.parquet
  2. Manifest:        {prefix}/manifests/v1/chainlink-updown-5m-ingest-manifest.json

Default mode is --manifest-only: the parquet are assumed already synced (this script
verifies local<->S3 counts) and only the manifest is (re)generated and uploaded. Pass
--sync-data to (idempotently) `aws s3 sync` the local tree up first.
"""

from __future__ import annotations

import argparse
import datetime as dt
import hashlib
import json
import pathlib
import subprocess
import sys
from typing import Any


APPROVED_S3_PREFIX = "s3://bolt-parquet/backfill-staging/2026-06-01/chainlink"
SCHEMA_VERSION = "chainlink-updown-5m-ingest-manifest.v1"
MANIFEST_NAME = "chainlink-updown-5m-ingest-manifest.json"
DEFAULT_LOCAL_ROOT = "/Users/spson/Downloads/chainlink"
CYCLES_SUFFIX = "-5m-cycles"
PRODUCT_FAMILY = "updown-5m-cycles"
RUN_ID_PREFIX = "chainlink-5m-"
DIGEST_METHOD = (
    "partition_digest = sha256 of the concatenation, in filename-sorted order, of "
    "'{filename}:{sha256_hex}\\n' for every parquet in the partition; "
    "asset_digest = sha256 of the concatenation, in dt-sorted order, of "
    "'{dt}:{partition_digest}\\n' for every partition of the asset."
)


def utc_now() -> str:
    return dt.datetime.now(dt.timezone.utc).replace(microsecond=0).isoformat().replace("+00:00", "Z")


def stable_json(value: Any) -> str:
    return json.dumps(value, indent=2, sort_keys=True, ensure_ascii=True) + "\n"


def sha256_bytes(payload: bytes) -> str:
    return hashlib.sha256(payload).hexdigest()


def sha256_file(path: pathlib.Path) -> tuple[str, int]:
    digest = hashlib.sha256()
    total = 0
    with open(path, "rb") as handle:
        for chunk in iter(lambda: handle.read(1 << 20), b""):
            digest.update(chunk)
            total += len(chunk)
    return digest.hexdigest(), total


def normalize_s3_prefix(prefix: str) -> str:
    normalized = prefix.rstrip("/")
    if normalized != APPROVED_S3_PREFIX:
        raise ValueError(f"S3 prefix must be exactly {APPROVED_S3_PREFIX}")
    return normalized


def manifest_payload(manifest: dict[str, Any]) -> bytes:
    """Self-referential fixpoint hash, byte-compatible with the sibling backfill scripts."""
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


def discover_assets(local_root: pathlib.Path) -> list[str]:
    assets = []
    for child in sorted(local_root.iterdir()):
        if child.is_dir() and child.name.endswith(CYCLES_SUFFIX):
            assets.append(child.name[: -len(CYCLES_SUFFIX)])
    if not assets:
        raise SystemExit(f"No <asset>{CYCLES_SUFFIX} directories under {local_root}")
    return assets


def scan_partition(part_dir: pathlib.Path) -> dict[str, Any]:
    """Return per-partition object count, bytes, and content digest. Skips non-parquet junk."""
    entries = []
    byte_total = 0
    for path in sorted(part_dir.glob("*.parquet")):
        file_hash, size = sha256_file(path)
        entries.append((path.name, file_hash))
        byte_total += size
    blob = "".join(f"{name}:{file_hash}\n" for name, file_hash in entries)
    return {
        "object_count": len(entries),
        "byte_total": byte_total,
        "partition_digest": sha256_bytes(blob.encode("utf-8")),
    }


def sample_schema(local_root: pathlib.Path, assets: list[str]) -> dict[str, Any]:
    """Best-effort in-file schema + source tag from one sample parquet. pyarrow optional."""
    try:
        import pyarrow.parquet as pq  # noqa: PLC0415
    except Exception as exc:  # pragma: no cover - environment dependent
        return {"available": False, "reason": f"pyarrow unavailable: {exc!r}"}
    for asset in assets:
        parts = sorted((local_root / f"{asset}{CYCLES_SUFFIX}").glob("dt=*"))
        for part in parts:
            files = sorted(part.glob("*.parquet"))
            if not files:
                continue
            # ParquetFile reads the file's own columns only — no Hive partition
            # inference, so the dt=YYYY-MM-DD path segment is NOT added as a column.
            parquet_file = pq.ParquetFile(files[0])
            arrow_schema = parquet_file.schema_arrow
            schema = {name: str(arrow_schema.field(name).type) for name in arrow_schema.names}
            table = parquet_file.read()
            row = {k: v[0] for k, v in table.slice(0, 1).to_pydict().items()} if table.num_rows else {}
            return {
                "available": True,
                "sample_object": str(files[0].relative_to(local_root)),
                "in_file_columns": schema,
                "partition_keys": ["dt"],
                "rows_in_sample_object": table.num_rows,
                "source_tag": row.get("source"),
                "resolution": row.get("resolution"),
                "market_slug_sample": row.get("market_slug"),
            }
    return {"available": False, "reason": "no parquet found to sample"}


def s3_asset_count(asset: str) -> int:
    uri = f"{APPROVED_S3_PREFIX}/{asset}{CYCLES_SUFFIX}/"
    result = subprocess.run(
        ["aws", "s3", "ls", "--recursive", uri],
        check=True, capture_output=True, text=True,
    )
    return sum(1 for line in result.stdout.splitlines() if line.rstrip().endswith(".parquet"))


def sync_data(local_root: pathlib.Path) -> None:
    """Idempotent mirror upload of the local tree to the locked prefix (excludes junk)."""
    subprocess.run(
        ["aws", "s3", "sync", str(local_root), APPROVED_S3_PREFIX,
         "--exclude", "*", "--include", "*.parquet", "--only-show-errors"],
        check=True,
    )


def upload_manifest(local_path: pathlib.Path, s3_uri: str) -> None:
    subprocess.run(["aws", "s3", "cp", str(local_path), s3_uri, "--only-show-errors"], check=True)


def build_manifest(local_root: pathlib.Path, assets: list[str], *, verify_s3: bool) -> dict[str, Any]:
    partitions: list[dict[str, Any]] = []
    asset_summaries: dict[str, Any] = {}
    object_count = 0
    byte_total = 0

    for asset in assets:
        asset_dir = local_root / f"{asset}{CYCLES_SUFFIX}"
        part_dirs = sorted(p for p in asset_dir.glob("dt=*") if p.is_dir())
        dts = [p.name.split("=", 1)[1] for p in part_dirs]
        asset_objects = 0
        asset_bytes = 0
        part_digests: list[tuple[str, str]] = []
        for part_dir in part_dirs:
            part_dt = part_dir.name.split("=", 1)[1]
            scanned = scan_partition(part_dir)
            partitions.append({
                "asset": asset,
                "product_family": PRODUCT_FAMILY,
                "dt": part_dt,
                "object_count": scanned["object_count"],
                "byte_total": scanned["byte_total"],
                "partition_digest": scanned["partition_digest"],
                "s3_partition_prefix": f"{APPROVED_S3_PREFIX}/{asset}{CYCLES_SUFFIX}/{part_dir.name}/",
            })
            part_digests.append((part_dt, scanned["partition_digest"]))
            asset_objects += scanned["object_count"]
            asset_bytes += scanned["byte_total"]
        asset_blob = "".join(f"{d}:{h}\n" for d, h in sorted(part_digests))
        asset_summaries[asset] = {
            "partition_count": len(part_dirs),
            "object_count": asset_objects,
            "byte_total": asset_bytes,
            "dt_min": min(dts) if dts else None,
            "dt_max": max(dts) if dts else None,
            "asset_digest": sha256_bytes(asset_blob.encode("utf-8")),
        }
        object_count += asset_objects
        byte_total += asset_bytes

    all_dts = [s["dt_min"] for s in asset_summaries.values() if s["dt_min"]] + \
              [s["dt_max"] for s in asset_summaries.values() if s["dt_max"]]

    s3_verification: dict[str, Any] = {"checked": verify_s3, "per_asset": {}, "all_match": None}
    if verify_s3:
        all_match = True
        for asset in assets:
            s3n = s3_asset_count(asset)
            localn = asset_summaries[asset]["object_count"]
            match = s3n == localn
            all_match = all_match and match
            s3_verification["per_asset"][asset] = {"local": localn, "s3": s3n, "match": match}
        s3_verification["all_match"] = all_match

    run_seed = stable_json({
        "s3_prefix": APPROVED_S3_PREFIX,
        "schema_version": SCHEMA_VERSION,
        "local_root": str(local_root),
        "assets": assets,
    })
    run_id = RUN_ID_PREFIX + sha256_bytes(run_seed.encode("utf-8"))[:16]

    manifest: dict[str, Any] = {
        "schema_version": SCHEMA_VERSION,
        "run_id": run_id,
        "generated_at": utc_now(),
        "runner": pathlib.Path(__file__).name,
        "s3_prefix": APPROVED_S3_PREFIX,
        "source": {
            "venue": "chainlink",
            "product_family": PRODUCT_FAMILY,
            "description": (
                "Chainlink Data Streams 1m anchor, Binance-filled, 5-minute up/down cycle "
                "reference prices at 1-second resolution. User-provided dataset."
            ),
            "ingested_from": str(local_root),
            "ingest_method": "S3 mirror tree of user-provided parquet (aws s3 sync)",
        },
        "layout": {
            "type": "mirror-tree",
            "pattern": "{asset}-5m-cycles/dt=YYYY-MM-DD/{market_slug}.parquet",
            "content_addressed": False,
            "note": (
                "Object names are market slugs (NOT content-addressed). dt is a Hive "
                "partition key in the path, not an in-file column."
            ),
        },
        "schema": sample_schema(local_root, assets),
        "coverage": {
            "dt_min": min(all_dts) if all_dts else None,
            "dt_max": max(all_dts) if all_dts else None,
            "per_asset_dt_range": {a: [s["dt_min"], s["dt_max"]] for a, s in asset_summaries.items()},
            "note": "Coverage is per-asset; alts start later than BTC/ETH. Derived from dt partitions.",
        },
        "digest_method": DIGEST_METHOD,
        "assets": asset_summaries,
        "partitions": partitions,
        "totals": {
            "asset_count": len(assets),
            "partition_count": len(partitions),
            "object_count": object_count,
            "byte_total": byte_total,
        },
        "s3_verification": s3_verification,
        "write_policy": {
            "staging_only": True,
            "canonical_writes": False,
            "secrets_required": False,
        },
        "errors": [],
        "object_count_excluding_manifest": object_count,
        "bytes_excluding_manifest": byte_total,
    }
    manifest["manifest_s3_uri"] = f"{APPROVED_S3_PREFIX}/manifests/v1/{MANIFEST_NAME}"
    manifest["total_s3_object_count_including_manifest"] = object_count + 1
    return manifest


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--local-root", default=DEFAULT_LOCAL_ROOT)
    parser.add_argument("--s3-prefix", default=APPROVED_S3_PREFIX)
    parser.add_argument("--sync-data", action="store_true",
                        help="aws s3 sync the local parquet tree up first (idempotent).")
    parser.add_argument("--no-verify-s3", action="store_true",
                        help="Skip the local<->S3 object-count cross-check.")
    parser.add_argument("--no-upload-manifest", action="store_true",
                        help="Build + write the manifest locally but do not upload it.")
    args = parser.parse_args()

    normalize_s3_prefix(args.s3_prefix)
    local_root = pathlib.Path(args.local_root).expanduser()
    if not local_root.is_dir():
        raise SystemExit(f"local root not found: {local_root}")

    assets = discover_assets(local_root)
    print(f"assets: {assets}", flush=True)

    if args.sync_data:
        print("syncing local parquet -> S3 (idempotent)...", flush=True)
        sync_data(local_root)

    manifest = build_manifest(local_root, assets, verify_s3=not args.no_verify_s3)

    if manifest["s3_verification"]["checked"] and not manifest["s3_verification"]["all_match"]:
        # Fail loud: the manifest must not claim a complete staging that S3 does not back.
        print("S3 VERIFICATION FAILED:", file=sys.stderr)
        print(json.dumps(manifest["s3_verification"], indent=2), file=sys.stderr)
        raise SystemExit(2)

    payload = manifest_payload(manifest)
    local_manifest = local_root / MANIFEST_NAME
    local_manifest.write_bytes(payload)
    print(f"wrote manifest: {local_manifest} ({len(payload)} bytes, "
          f"sha256={manifest['manifest_sha256']})", flush=True)

    if not args.no_upload_manifest:
        upload_manifest(local_manifest, manifest["manifest_s3_uri"])
        print(f"uploaded manifest -> {manifest['manifest_s3_uri']}", flush=True)

    print(json.dumps({
        "run_id": manifest["run_id"],
        "object_count": manifest["totals"]["object_count"],
        "byte_total": manifest["totals"]["byte_total"],
        "s3_all_match": manifest["s3_verification"]["all_match"],
        "manifest_sha256": manifest["manifest_sha256"],
    }, indent=2), flush=True)
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
