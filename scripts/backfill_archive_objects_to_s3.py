#!/usr/bin/env python3
"""Stream archive objects from a source-proof file into an S3 staging prefix."""

from __future__ import annotations

import argparse
import datetime as dt
import hashlib
import json
import os
import pathlib
import subprocess
import tempfile
import urllib.error
import urllib.request
from typing import Any


USER_AGENT = "bolt-v2-backfill-archive-objects-to-s3/1"


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


def normalized_s3_prefix(prefix: str) -> str:
    if not prefix.startswith("s3://"):
        raise ValueError("--s3-prefix must start with s3://")
    return prefix.rstrip("/")


def s3_object_uri(prefix: str, proof: dict[str, Any], family: str, object_date: str, payload_hash: str) -> str:
    return (
        f"{normalized_s3_prefix(prefix)}/raw/v1/"
        f"source_binding={proof['source_binding_key']}/"
        f"fixture={proof['fixture']}/"
        f"family={family}/"
        f"dt={object_date}/"
        f"object={payload_hash}.parquet"
    )


def download_to_temp(work_dir: pathlib.Path, archive_object: dict[str, Any]) -> tuple[pathlib.Path, str, int, int, str | None]:
    request = urllib.request.Request(archive_object["url"], headers={"User-Agent": USER_AGENT})
    with urllib.request.urlopen(request, timeout=300) as response:
        status = response.status
        content_type = response.headers.get("Content-Type") or response.headers.get("content-type")
        hasher = hashlib.sha256()
        total = 0
        work_dir.mkdir(parents=True, exist_ok=True)
        fd, temp_name = tempfile.mkstemp(prefix="archive-object-", suffix=".parquet", dir=work_dir)
        try:
            with os.fdopen(fd, "wb") as handle:
                while True:
                    chunk = response.read(1024 * 1024)
                    if not chunk:
                        break
                    handle.write(chunk)
                    hasher.update(chunk)
                    total += len(chunk)
            return pathlib.Path(temp_name), hasher.hexdigest(), total, status, content_type
        except BaseException:
            pathlib.Path(temp_name).unlink(missing_ok=True)
            raise


def upload_to_s3(local_path: pathlib.Path, s3_uri: str) -> None:
    subprocess.run(
        ["aws", "s3", "cp", str(local_path), s3_uri, "--only-show-errors"],
        check=True,
    )


def write_local_manifest(path: pathlib.Path, manifest: dict[str, Any]) -> str:
    payload = stable_json(manifest).encode("utf-8")
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_bytes(payload)
    return sha256_bytes(payload)


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--source-proof", required=True, type=pathlib.Path)
    parser.add_argument("--s3-prefix", required=True)
    parser.add_argument("--local-manifest-root", required=True, type=pathlib.Path)
    parser.add_argument("--family", required=True)
    parser.add_argument("--offset", type=int, default=0)
    parser.add_argument("--max-objects", type=int)
    parser.add_argument("--work-dir", required=True, type=pathlib.Path)
    parser.add_argument("--keep-temp", action="store_true")
    return parser.parse_args()


def main() -> int:
    args = parse_args()
    proof = json.loads(args.source_proof.read_text())
    archive_objects = proof["summary"].get("archive_objects", [])
    selected_objects = archive_objects[args.offset :]
    if args.max_objects is not None:
        selected_objects = selected_objects[: args.max_objects]

    generated_at = utc_now()
    run_seed = stable_json(
        {
            "family": args.family,
            "generated_at": generated_at,
            "max_objects": args.max_objects,
            "offset": args.offset,
            "s3_prefix": normalized_s3_prefix(args.s3_prefix),
            "source_proof_id": proof["source_proof_id"],
        }
    )
    run_id = "archive-s3-run-" + sha256_bytes(run_seed.encode("utf-8"))[:16]
    manifest: dict[str, Any] = {
        "schema_version": "backfill-archive-s3-manifest.v1",
        "contract_version": proof["contract_version"],
        "run_id": run_id,
        "generated_at": generated_at,
        "s3_prefix": normalized_s3_prefix(args.s3_prefix),
        "canonical_s3_write": False,
        "write_mode": "s3_staging",
        "source_proof_id": proof["source_proof_id"],
        "source_proof_version": proof["source_proof_version"],
        "source_binding": proof["source_binding_key"],
        "venue": proof["venue"],
        "product_family": proof["product_family"],
        "family": args.family,
        "offset": args.offset,
        "planned_object_count": len(selected_objects),
        "planned_source_bytes": sum(item.get("size_bytes") or 0 for item in selected_objects),
        "s3_payload_records": [],
        "errors": [],
    }
    manifest_path = args.local_manifest_root / "ingest-manifests" / "v1" / f"run={run_id}" / "archive-s3-manifest.json"

    for archive_object in selected_objects:
        local_path = None
        started_at = utc_now()
        try:
            local_path, payload_hash, total, status, content_type = download_to_temp(args.work_dir, archive_object)
            object_date = parse_object_date(archive_object["name"])
            s3_uri = s3_object_uri(args.s3_prefix, proof, args.family, object_date, payload_hash)
            upload_to_s3(local_path, s3_uri)
            manifest["s3_payload_records"].append(
                {
                    "source_binding": proof["source_binding_key"],
                    "venue": proof["venue"],
                    "product_family": proof["product_family"],
                    "family": args.family,
                    "source_uri": archive_object["url"],
                    "source_name": archive_object["name"],
                    "source_size_text": archive_object.get("size_text"),
                    "source_size_bytes": archive_object.get("size_bytes"),
                    "http_status": status,
                    "content_type": content_type,
                    "payload_hash": payload_hash,
                    "bytes": total,
                    "started_at": started_at,
                    "completed_at": utc_now(),
                    "s3_uri": s3_uri,
                }
            )
        except (urllib.error.URLError, TimeoutError, OSError, subprocess.CalledProcessError) as exc:
            manifest["errors"].append({"source_uri": archive_object.get("url"), "error": repr(exc)})
        finally:
            if local_path is not None and not args.keep_temp:
                local_path.unlink(missing_ok=True)
        write_local_manifest(manifest_path, manifest)

    manifest["completed_at"] = utc_now()
    manifest["completed_object_count"] = len(manifest["s3_payload_records"])
    manifest["completed_bytes"] = sum(item["bytes"] for item in manifest["s3_payload_records"])
    manifest["manifest_hash_scope"] = "manifest_without_manifest_hash"
    manifest["manifest_hash"] = hashlib.sha256(stable_json(manifest).encode("utf-8")).hexdigest()
    write_local_manifest(manifest_path, manifest)
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
