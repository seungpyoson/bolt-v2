#!/usr/bin/env python3
"""Download archive objects listed in a source-proof JSON file."""

from __future__ import annotations

import argparse
import concurrent.futures
import datetime as dt
import hashlib
import json
import os
import pathlib
import shutil
import tempfile
import urllib.error
import urllib.request
from typing import Any


USER_AGENT = "bolt-v2-backfill-archive-objects/1"


def stable_json(value: Any) -> str:
    return json.dumps(value, indent=2, sort_keys=True, ensure_ascii=True) + "\n"


def utc_now() -> str:
    return dt.datetime.now(dt.UTC).replace(microsecond=0).isoformat().replace("+00:00", "Z")


def sha256_bytes(payload: bytes) -> str:
    return hashlib.sha256(payload).hexdigest()


def parse_object_date(name: str) -> str:
    for part in name.split("_"):
        if len(part) >= 10 and part[4:5] == "-" and part[7:8] == "-":
            return part[:10]
    return dt.datetime.now(dt.UTC).strftime("%Y-%m-%d")


def output_path(root: pathlib.Path, source_binding: str, fixture: str, family: str, object_date: str, payload_hash: str) -> pathlib.Path:
    return (
        root
        / "raw"
        / "v1"
        / f"source_binding={source_binding}"
        / f"fixture={fixture}"
        / f"family={family}"
        / f"dt={object_date}"
        / f"object={payload_hash}.parquet"
    )


def download_object(root: pathlib.Path, proof: dict[str, Any], archive_object: dict[str, Any], family: str) -> dict[str, Any]:
    url = archive_object["url"]
    name = archive_object["name"]
    request = urllib.request.Request(url, headers={"User-Agent": USER_AGENT})
    started_at = utc_now()
    with urllib.request.urlopen(request, timeout=300) as response:
        status = response.status
        headers = dict(response.headers.items())
        hasher = hashlib.sha256()
        total = 0
        object_date = parse_object_date(name)
        temp_dir = root / "tmp" / "archive-downloads"
        temp_dir.mkdir(parents=True, exist_ok=True)
        fd, temp_name = tempfile.mkstemp(prefix="download-", suffix=".partial", dir=temp_dir)
        try:
            with os.fdopen(fd, "wb") as handle:
                while True:
                    chunk = response.read(1024 * 1024)
                    if not chunk:
                        break
                    handle.write(chunk)
                    hasher.update(chunk)
                    total += len(chunk)
            payload_hash = hasher.hexdigest()
            path = output_path(root, proof["source_binding_key"], proof["fixture"], family, object_date, payload_hash)
            path.parent.mkdir(parents=True, exist_ok=True)
            if path.exists():
                pathlib.Path(temp_name).unlink()
            else:
                pathlib.Path(temp_name).replace(path)
            return {
                "source_binding": proof["source_binding_key"],
                "venue": proof["venue"],
                "product_family": proof["product_family"],
                "family": family,
                "source_uri": url,
                "source_name": name,
                "source_size_text": archive_object.get("size_text"),
                "source_size_bytes": archive_object.get("size_bytes"),
                "http_status": status,
                "content_type": headers.get("Content-Type") or headers.get("content-type"),
                "payload_hash": payload_hash,
                "bytes": total,
                "started_at": started_at,
                "completed_at": utc_now(),
                "uri": str(path),
            }
        except BaseException:
            pathlib.Path(temp_name).unlink(missing_ok=True)
            raise


def write_manifest(root: pathlib.Path, manifest: dict[str, Any]) -> pathlib.Path:
    run_id = manifest["run_id"]
    path = root / "ingest-manifests" / "v1" / f"run={run_id}" / "archive-objects-manifest.json"
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(stable_json(manifest))
    return path


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--source-proof", required=True, type=pathlib.Path)
    parser.add_argument("--artifact-root", required=True, type=pathlib.Path)
    parser.add_argument("--family", required=True)
    parser.add_argument("--max-objects", type=int)
    parser.add_argument("--max-workers", type=int, default=3)
    parser.add_argument("--reserve-free-gb", type=float, default=5.0)
    return parser.parse_args()


def main() -> int:
    args = parse_args()
    proof = json.loads(args.source_proof.read_text())
    archive_objects = proof["summary"].get("archive_objects", [])
    if args.max_objects is not None:
        archive_objects = archive_objects[: args.max_objects]
    expected_bytes = sum(item.get("size_bytes") or 0 for item in archive_objects)
    free_bytes = shutil.disk_usage(args.artifact_root.parent if args.artifact_root.exists() else args.artifact_root.parent).free
    reserve_bytes = int(args.reserve_free_gb * 1_000_000_000)
    if expected_bytes and free_bytes < expected_bytes + reserve_bytes:
        print(
            stable_json(
                {
                    "ok": False,
                    "error": "insufficient_free_space",
                    "expected_bytes": expected_bytes,
                    "free_bytes": free_bytes,
                    "reserve_bytes": reserve_bytes,
                }
            ),
            end="",
        )
        return 2

    generated_at = utc_now()
    run_id = "archive-objects-run-" + sha256_bytes((generated_at + str(args.artifact_root)).encode("utf-8"))[:16]
    manifest: dict[str, Any] = {
        "schema_version": "backfill-archive-objects-manifest.v1",
        "contract_version": proof["contract_version"],
        "run_id": run_id,
        "generated_at": generated_at,
        "artifact_root": str(args.artifact_root),
        "canonical_s3_write": False,
        "write_mode": "local_staging",
        "source_proof_id": proof["source_proof_id"],
        "source_proof_version": proof["source_proof_version"],
        "source_binding": proof["source_binding_key"],
        "venue": proof["venue"],
        "product_family": proof["product_family"],
        "family": args.family,
        "planned_object_count": len(archive_objects),
        "planned_source_bytes": expected_bytes,
        "raw_payload_records": [],
        "errors": [],
    }

    with concurrent.futures.ThreadPoolExecutor(max_workers=args.max_workers) as executor:
        future_to_object = {
            executor.submit(download_object, args.artifact_root, proof, archive_object, args.family): archive_object
            for archive_object in archive_objects
        }
        for future in concurrent.futures.as_completed(future_to_object):
            archive_object = future_to_object[future]
            try:
                manifest["raw_payload_records"].append(future.result())
            except (urllib.error.URLError, TimeoutError, OSError) as exc:
                manifest["errors"].append({"source_uri": archive_object.get("url"), "error": repr(exc)})
            write_manifest(args.artifact_root, manifest)

    manifest["completed_at"] = utc_now()
    manifest["completed_object_count"] = len(manifest["raw_payload_records"])
    manifest["completed_bytes"] = sum(item["bytes"] for item in manifest["raw_payload_records"])
    manifest["manifest_hash"] = sha256_bytes(stable_json(manifest).encode("utf-8"))
    manifest_path = write_manifest(args.artifact_root, manifest)
    print(
        stable_json(
            {
                "ok": not manifest["errors"],
                "manifest_path": str(manifest_path),
                "manifest_hash": manifest["manifest_hash"],
                "planned_object_count": manifest["planned_object_count"],
                "completed_object_count": manifest["completed_object_count"],
                "completed_bytes": manifest["completed_bytes"],
                "errors": manifest["errors"],
            }
        ),
        end="",
    )
    return 0 if not manifest["errors"] else 1


if __name__ == "__main__":
    raise SystemExit(main())
