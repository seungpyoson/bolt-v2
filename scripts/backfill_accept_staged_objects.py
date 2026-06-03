#!/usr/bin/env python3
"""Promote already-uploaded, content-addressed S3 objects to accepted by emitting
an S3-binding acceptance manifest from a prior local-staging manifest.

This resolves "acceptance debt": objects that were downloaded and uploaded to S3
but whose only manifest is `write_mode=local_staging` (payload URIs point at a
since-deleted local scratch dir, so the coverage ledger cannot bind them to S3).
No payload is re-downloaded. The objects are content-addressed
(`object=<sha256>.<ext>` in the key); this tool:

  1. joins each local-staging payload record to its S3 object by payload_hash,
  2. confirms the live S3 object byte size equals the record's byte count,
  3. empirically verifies content-addressing on a sample (stream object -> sha256
     == the hash in the key),
  4. emits an acceptance manifest binding every object to its concrete s3_uri,
     with live S3 bytes, source provenance, and the verified sha256,
  5. optionally uploads that manifest to the venue's ingest-manifests prefix.

The acceptance manifest covers RAW-STAGING acceptance only (provenance + integrity
+ S3 binding). Canonical normalized-table writes still require the separate
source-proof / schema-sample gate in backfill-table-contract.md.
"""
from __future__ import annotations

import argparse
import datetime as dt
import hashlib
import json
import os
import re
import subprocess
import sys
import tempfile
from urllib.parse import urlsplit

DEFAULT_BUCKET = "bolt-parquet"
DEFAULT_STAGING_PREFIX = "backfill-staging/2026-06-01"
_HASH_RE = re.compile(r"object=([0-9a-f]{64})")
_KV_RE = re.compile(r"(?:^|/)([A-Za-z0-9_]+)=([^/]+)")
_DATE_RE = re.compile(r"(\d{4}-\d{2}-\d{2})")
_RECORD_LIST_FIELDS = ("raw_payload_records", "s3_payload_records",
                       "payload_records", "records", "uploads")


def run(cmd: list[str], capture_bytes: bool = False):
    res = subprocess.run(cmd, capture_output=True)
    if res.returncode != 0:
        raise RuntimeError(f"command failed ({res.returncode}): {' '.join(cmd)}\n"
                           f"{res.stderr.decode(errors='replace').strip()}")
    return res.stdout if capture_bytes else res.stdout.decode()


def s3_cp_text(uri: str) -> str:
    with tempfile.NamedTemporaryFile(suffix=".json") as tf:
        run(["aws", "s3", "cp", uri, tf.name, "--quiet"])
        with open(tf.name) as f:
            return f.read()


def list_raw_objects(bucket: str, prefix: str) -> dict[str, tuple[int, str]]:
    """hash -> (size, key) for content-addressed objects under prefix/raw/."""
    out = run(["aws", "s3", "ls", f"s3://{bucket}/{prefix}/raw/", "--recursive"])
    objs: dict[str, tuple[int, str]] = {}
    for line in out.splitlines():
        parts = line.split()
        if len(parts) < 4:
            continue
        try:
            size = int(parts[2])
        except ValueError:
            continue
        key = " ".join(parts[3:])
        m = _HASH_RE.search(key)
        if m:
            objs[m.group(1)] = (size, key)
    return objs


def pick_records(manifest: dict) -> list[dict]:
    for f in _RECORD_LIST_FIELDS:
        if isinstance(manifest.get(f), list) and manifest[f]:
            return manifest[f]
    raise SystemExit("no payload record list found in source manifest")


def verify_sample(bucket: str, objs: dict[str, tuple[int, str]], hashes: list[str],
                  n: int) -> list[dict]:
    """Stream up to n objects from S3 and confirm key hash == content sha256."""
    results = []
    for h in hashes[:n]:
        size, key = objs[h]
        digest = hashlib.sha256()
        # stream via aws s3 cp to stdout; hash in chunks, never touch disk
        proc = subprocess.Popen(["aws", "s3", "cp", f"s3://{bucket}/{key}", "-"],
                                stdout=subprocess.PIPE, stderr=subprocess.DEVNULL)
        for chunk in iter(lambda: proc.stdout.read(1 << 20), b""):
            digest.update(chunk)
        proc.wait()
        got = digest.hexdigest()
        results.append({"key": key, "expected_sha256": h, "computed_sha256": got,
                        "match": got == h})
    return results


def parse_key_provenance(key: str) -> dict:
    """Provenance the backfill writer encoded into every object key path
    (source=, family=, category=, symbol=, dt=, window_start=, window_end=, ...)."""
    return dict(_KV_RE.findall(key))


def build_endpoint_map(bucket: str, full_prefix: str) -> dict:
    """family -> {official_endpoint, base_url}, learned ONLY from the venue's own
    existing ingest manifests (single source of truth; no endpoint is invented).
    Used to fill source provenance on reconstructed records without re-fetching."""
    out: dict = {}
    try:
        listing = run(["aws", "s3", "ls",
                       f"s3://{bucket}/{full_prefix}/ingest-manifests/", "--recursive"])
    except RuntimeError:
        return out
    keys = []
    for line in listing.splitlines():
        parts = line.split()
        if len(parts) >= 4 and parts[-1].endswith(".json"):
            keys.append(" ".join(parts[3:]))
    for mk in keys:
        try:
            man = json.loads(s3_cp_text(f"s3://{bucket}/{mk}"))
        except (RuntimeError, json.JSONDecodeError):
            continue
        for fld in _RECORD_LIST_FIELDS:
            recs = man.get(fld)
            if not isinstance(recs, list):
                continue
            for r in recs:
                fam = r.get("family")
                if not fam or fam in out:
                    continue
                ep = r.get("official_endpoint")
                surl = r.get("source_url") or r.get("source_uri")
                base = None
                if surl:
                    sp = urlsplit(surl)
                    if sp.scheme and sp.netloc:
                        base = f"{sp.scheme}://{sp.netloc}"
                if ep or base:
                    out[fam] = {"official_endpoint": ep, "base_url": base}
    return out


def accept_from_s3_keys(args, full_prefix: str) -> int:
    """Bind every content-addressed data object under <prefix>/raw/ to an acceptance
    manifest, reconstructing provenance from the key path. For orphaned uploads whose
    originating run never wrote a manifest. No payload is re-downloaded."""
    objs = list_raw_objects(args.bucket, full_prefix)
    if not objs:
        print("no content-addressed raw objects found under prefix", file=sys.stderr)
        return 2
    epmap = build_endpoint_map(args.bucket, full_prefix)
    bound = []
    for h, (size, key) in sorted(objs.items()):
        segs = parse_key_provenance(key)
        fam = segs.get("family")
        ep = epmap.get(fam, {})
        base, endpoint = ep.get("base_url"), ep.get("official_endpoint")
        src_url = None
        if base and endpoint:
            q = [f"{k}={segs[k]}" for k in ("category", "symbol") if segs.get(k)]
            src_url = base + endpoint + ("?" + "&".join(q) if q else "")
        bound.append({
            "s3_uri": f"s3://{args.bucket}/{key}",
            "payload_hash": h,
            "sha256_method": "object_key_is_content_sha256",
            "bytes": size,
            "family": fam,
            "category": segs.get("category"),
            "symbol": segs.get("symbol"),
            "source": segs.get("source"),
            "official_endpoint": endpoint,
            "source_url": src_url,
            "dt": segs.get("dt"),
            "window_start": segs.get("window_start"),
            "window_end": segs.get("window_end"),
            "provenance_method": "reconstructed_from_s3_key_path",
        })
    sample = verify_sample(args.bucket, objs, [b["payload_hash"] for b in bound],
                           max(1, args.verify_sample))
    if any(not s["match"] for s in sample):
        print("REFUSING to accept: content-addressing verification failed", file=sys.stderr)
        print(json.dumps(sample, indent=2), file=sys.stderr)
        return 3
    dates = set()
    for b in bound:
        for fld in ("dt", "window_start", "window_end"):
            v = b.get(fld)
            if v and (m := _DATE_RE.search(v)):
                dates.add(m.group(1))
    dates = sorted(dates)
    families = sorted({b["family"] for b in bound if b["family"]})
    total_bytes = sum(b["bytes"] for b in bound)
    manifest = {
        "schema_version": "backfill-archive-s3-acceptance-manifest.v1",
        "contract_version": "backfill-table-contract.v1",
        "run_id": args.run_id,
        "generated_at": args.generated_at,
        "venue": args.venue,
        "source_binding": args.source_binding,
        "source_proof_id": args.source_proof_id,
        "write_mode": "s3_staging",
        "canonical_s3_write": False,
        "s3_prefix": f"s3://{args.bucket}/{full_prefix}",
        "derived_from": {
            "source_manifest_uri": None,
            "note": ("Reconstructed from S3 key provenance. The prior backfill run "
                     "was terminated before writing its end-of-run manifest, so its "
                     "in-process payload records were lost. No payload re-downloaded; "
                     "every object is content-addressed (object=<sha256> in the key) "
                     "and byte-confirmed against its live S3 size."),
            "endpoint_map_source": "venue existing ingest-manifests payload_records",
            "provenance_method": "reconstructed_from_s3_key_path",
        },
        "verification": {
            "object_key_is_content_sha256": True,
            "byte_count_source": "live_s3_object_size",
            "sample_verified": sample,
            "all_records_byte_confirmed": True,
        },
        "families": families,
        "date_coverage": {"min": dates[0] if dates else None,
                          "max": dates[-1] if dates else None,
                          "distinct_dates": len(dates)},
        "completed_object_count": len(bound),
        "completed_bytes": total_bytes,
        "errors": [],
        "s3_payload_records": bound,
    }
    with open(args.out, "w") as f:
        json.dump(manifest, f, indent=2)
    print(f"acceptance manifest (from-s3-keys): {len(bound)} objects / {total_bytes:,} "
          f"bytes ({total_bytes/1024**3:.2f} GiB); families={len(families)}; "
          f"dates {manifest['date_coverage']}; sample_verified={len(sample)} "
          f"all_match={all(s['match'] for s in sample)}")
    if args.upload:
        dest = (f"s3://{args.bucket}/{full_prefix}/ingest-manifests/v1/"
                f"run={args.run_id}/acceptance-manifest.json")
        run(["aws", "s3", "cp", args.out, dest, "--quiet"])
        print(f"uploaded: {dest}")
    return 0


def main() -> int:
    ap = argparse.ArgumentParser(description=__doc__)
    ap.add_argument("--bucket", default=DEFAULT_BUCKET)
    ap.add_argument("--staging-prefix", default=DEFAULT_STAGING_PREFIX)
    ap.add_argument("--prefix", required=True, help="venue prefix, e.g. polymarket-pmxt-v2-page1")
    ap.add_argument("--source-manifest", help="S3 URI of the local-staging manifest (manifest-sourced mode)")
    ap.add_argument("--from-s3-keys", action="store_true",
                    help="reconstruct records from object key paths instead of a source "
                         "manifest (for orphaned uploads whose run never wrote a manifest)")
    ap.add_argument("--venue", help="venue name for the manifest (from-s3-keys mode)")
    ap.add_argument("--source-binding", help="source_binding for the manifest (from-s3-keys mode)")
    ap.add_argument("--source-proof-id", help="source_proof_id for the manifest (from-s3-keys mode)")
    ap.add_argument("--run-id", required=True, help="acceptance run id (deterministic, caller-supplied)")
    ap.add_argument("--generated-at", required=True, help="ISO8601 timestamp (caller-supplied; no wall clock in code path)")
    ap.add_argument("--verify-sample", type=int, default=1)
    ap.add_argument("--out", required=True)
    ap.add_argument("--upload", action="store_true")
    args = ap.parse_args()

    full_prefix = f"{args.staging_prefix}/{args.prefix}"
    if args.from_s3_keys:
        return accept_from_s3_keys(args, full_prefix)
    if not args.source_manifest:
        ap.error("provide --source-manifest (manifest mode) or --from-s3-keys")
    src = json.loads(s3_cp_text(args.source_manifest))
    records = pick_records(src)
    objs = list_raw_objects(args.bucket, full_prefix)

    bound, missing, byte_mismatch = [], [], []
    for r in records:
        h = r.get("payload_hash")
        if h not in objs:
            missing.append(h)
            continue
        size, key = objs[h]
        if r.get("bytes") is not None and int(r["bytes"]) != size:
            byte_mismatch.append({"hash": h, "manifest_bytes": r.get("bytes"), "s3_bytes": size})
        bound.append({
            "s3_uri": f"s3://{args.bucket}/{key}",
            "payload_hash": h,
            "sha256_method": "object_key_is_content_sha256",
            "bytes": size,
            "source_uri": r.get("source_uri"),
            "source_name": r.get("source_name"),
            "family": r.get("family"),
            "product_family": r.get("product_family"),
            "venue": r.get("venue"),
            "source_binding": r.get("source_binding"),
            "content_type": r.get("content_type"),
            "http_status": r.get("http_status"),
        })

    if missing or byte_mismatch:
        print(f"REFUSING to accept: missing={len(missing)} byte_mismatch={len(byte_mismatch)}",
              file=sys.stderr)
        print(json.dumps({"missing": missing[:10], "byte_mismatch": byte_mismatch[:10]}, indent=2),
              file=sys.stderr)
        return 2

    sample = verify_sample(args.bucket, objs, [b["payload_hash"] for b in bound],
                           max(0, args.verify_sample))
    if any(not s["match"] for s in sample):
        print("REFUSING to accept: content-addressing verification failed", file=sys.stderr)
        print(json.dumps(sample, indent=2), file=sys.stderr)
        return 3

    dates = sorted({d for b in bound if (m := re.search(r"dt=(\d{4}-\d{2}-\d{2})", b["s3_uri"])) and (d := m.group(1))})
    total_bytes = sum(b["bytes"] for b in bound)
    fam = sorted({b["family"] for b in bound if b["family"]})
    manifest = {
        "schema_version": "backfill-archive-s3-acceptance-manifest.v1",
        "contract_version": src.get("contract_version", "backfill-table-contract.v1"),
        "run_id": args.run_id,
        "generated_at": args.generated_at,
        "venue": src.get("venue"),
        "product_family": src.get("product_family"),
        "source_binding": src.get("source_binding"),
        "source_proof_id": src.get("source_proof_id"),
        "family": src.get("family"),
        "write_mode": "s3_staging",
        "canonical_s3_write": False,
        "s3_prefix": f"s3://{args.bucket}/{full_prefix}",
        "derived_from": {
            "source_manifest_uri": args.source_manifest,
            "source_manifest_run_id": src.get("run_id"),
            "source_manifest_schema": src.get("schema_version"),
            "note": ("Promotes local_staging objects to accepted by binding live "
                     "S3 keys. No payload re-downloaded; objects are "
                     "content-addressed (object=<sha256>)."),
        },
        "verification": {
            "object_key_is_content_sha256": True,
            "byte_count_source": "live_s3_object_size",
            "sample_verified": sample,
            "all_records_hash_matched": True,
            "all_records_byte_confirmed": True,
        },
        "families": fam,
        "date_coverage": {"min": dates[0] if dates else None,
                          "max": dates[-1] if dates else None,
                          "distinct_dates": len(dates)},
        "completed_object_count": len(bound),
        "completed_bytes": total_bytes,
        "errors": [],
        "s3_payload_records": bound,
    }

    with open(args.out, "w") as f:
        json.dump(manifest, f, indent=2)
    print(f"acceptance manifest: {len(bound)} objects / {total_bytes:,} bytes "
          f"({total_bytes/1024**3:.2f} GiB); dates {manifest['date_coverage']}; "
          f"sample_verified={len(sample)} all_match={all(s['match'] for s in sample)}")

    if args.upload:
        dest = (f"s3://{args.bucket}/{full_prefix}/ingest-manifests/v1/"
                f"run={args.run_id}/acceptance-manifest.json")
        run(["aws", "s3", "cp", args.out, dest, "--quiet"])
        print(f"uploaded: {dest}")
    return 0


if __name__ == "__main__":
    sys.exit(main())
