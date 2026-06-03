#!/usr/bin/env python3
"""Build a machine-readable coverage ledger for the one-off seven-token backfill.

This reconciles physically-present S3 objects against ingest-manifest payload
bindings at the S3-object-key level. Reconciliation is dedup-correct across
retry runs (a key bound by several manifests is counted once) and every byte
figure is anchored to the actual S3 object size from the live listing, never to
manifest-claimed byte counts (which can drift from what was really uploaded).

Buckets, per the one-off backfill acceptance goal:
  - accepted               physical DATA objects bound (by s3_uri) to >=1
                           accepted-binding manifest.
  - unaccepted_physical    physical DATA objects bound to no accepted-binding
                           manifest (duplicate retry runs, local-staging-only
                           manifests, out-of-scope selectors, orphan uploads).
  - failed_or_gap          manifests reporting errors>0 or gaps>0 (the
                           successfully-uploaded objects they bind are still
                           accepted; the errors/gaps record missing data).
  - zero_payload           manifests that bind zero payload objects.

Provenance objects (manifests, checksums, source-proofs, universes, progress
checkpoints) are tracked separately and are never counted as accepted data.

A manifest is "accepted-binding" when its write_mode is an S3 staging mode, it
has zero selector-scope violations, and it binds at least one payload to a
concrete s3_uri. Errors/gaps do NOT disqualify the objects that did upload
(partial-accepted semantics, matching the handoff ledger), but the manifest is
also listed in the failed_or_gap bucket so the gaps stay visible.

Usage:
  # one venue prefix -> one fragment (run these concurrently, one per prefix)
  backfill_coverage_ledger.py --prefix <venue_prefix> --out <fragment.json>

  # merge all fragments -> final ledger + human status table
  backfill_coverage_ledger.py --combine --fragments-dir <dir> \
      --out <ledger.json> --table-out <status.md>

Bucket and staging prefix default to the live one-off staging location and may
be overridden on the command line.
"""
from __future__ import annotations

import argparse
import json
import os
import re
import subprocess
import sys
import tempfile
from collections import defaultdict

DEFAULT_BUCKET = "bolt-parquet"
DEFAULT_STAGING_PREFIX = "backfill-staging/2026-06-01"

# One-off scope (the status log narrows the generic "last 3 months" to this
# explicit inclusive window). Used only to annotate in-window date coverage and
# to flag out-of-scope objects; it does not change manifest-binding logic.
SCOPE_BASES = ("BTC", "ETH", "SOL", "XRP", "DOGE", "HYPE", "BNB")
WINDOW_START = "2026-03-01"
WINDOW_END_INCLUSIVE = "2026-06-01"

# Only an explicit data-date partition (dt=, date=, object_date=, bounded_end=,
# window_start=) is trusted. A bare-token fallback is deliberately NOT used: it
# matches 8-digit runs inside content-hash object filenames (object=<sha256>),
# producing garbage dates. Every parsed date is also calendar-validated.
_PART_DATE_RE = re.compile(r"(?:dt|date|object_date|bounded_end|window_start)=(\d{4}-\d{2}-\d{2}|\d{8})")

# Venue prefix -> user-facing venue name. PMXT is preserved only in the source
# binding / provenance; it is never reported as a separate venue.
VENUE_NAME = {
    "binance": "Binance",
    "okx": "OKX",
    "bybit": "Bybit",
    "deribit": "Deribit",
    "hyperliquid-core": "Hyperliquid core",
    "hyperliquid-core-targeted-btc-eth-sol-xrp-doge-hype-bnb": "Hyperliquid core",
    "hyperliquid-hip3": "Hyperliquid HIP-3",
    "hyperliquid-hip4": "Hyperliquid HIP-4",
    "polymarket-pmxt-v2-streaming": "Polymarket (PMXT source)",
    "polymarket-pmxt-v2-page1": "Polymarket (PMXT source)",
    "source-proof-v3": "Cross-venue source proofs",
}

PROVENANCE_MARKERS = (
    "/manifests/",
    "/ingest-manifests/",
    "/checksums/",
    "/source-proof",   # matches source-proof/ and source-proofs/
    "/universes/",
    "/progress/",
)


def run(cmd: list[str]) -> str:
    res = subprocess.run(cmd, capture_output=True, text=True)
    if res.returncode != 0:
        raise RuntimeError(f"command failed ({res.returncode}): {' '.join(cmd)}\n{res.stderr.strip()}")
    return res.stdout


def list_physical(bucket: str, prefix: str) -> dict[str, int]:
    """Return {key: size_bytes} for every object under s3://bucket/prefix/."""
    out = run(["aws", "s3", "ls", f"s3://{bucket}/{prefix}/", "--recursive"])
    objects: dict[str, int] = {}
    for line in out.splitlines():
        parts = line.split()
        if len(parts) < 4:
            continue
        # "<date> <time> <size> <key...>"
        try:
            size = int(parts[2])
        except ValueError:
            continue
        key = " ".join(parts[3:])
        if key.endswith("/"):
            continue
        objects[key] = size
    return objects


def is_provenance(key: str, strip_prefix: str = "") -> bool:
    # Classify on the path *relative to the venue prefix* so the venue-prefix
    # segment itself can never collide with a provenance marker. Without this,
    # a venue named e.g. "source-proof-v3" makes every key contain the
    # "/source-proof" marker substring, forcing all DATA objects into the
    # provenance bucket and zeroing accepted coverage.
    rel = key
    if strip_prefix and key.startswith(strip_prefix):
        rel = key[len(strip_prefix):]
    if rel.endswith("manifest.json") or rel.endswith("manifest.json.sha256"):
        return True
    return any(m in rel for m in PROVENANCE_MARKERS)


def key_from_s3_uri(s3_uri: str, bucket: str) -> str | None:
    if not isinstance(s3_uri, str):
        return None
    pre = f"s3://{bucket}/"
    if s3_uri.startswith(pre):
        return s3_uri[len(pre):]
    m = re.match(r"^s3://[^/]+/(.+)$", s3_uri)
    return m.group(1) if m else None


def _len(v) -> int:
    return len(v) if isinstance(v, list) else 0


def norm_date(token: str) -> str | None:
    """Normalise YYYYMMDD/YYYY-MM-DD to YYYY-MM-DD; return None if not a
    plausible calendar date (guards against hash digits parsed as dates)."""
    import datetime as _dt
    if len(token) == 8 and token.isdigit():
        token = f"{token[:4]}-{token[4:6]}-{token[6:]}"
    try:
        d = _dt.date.fromisoformat(token)
    except ValueError:
        return None
    return d.isoformat() if 2024 <= d.year <= 2027 else None


def key_date(key: str, strip_prefix: str = "") -> str | None:
    """Best-effort data date from a key, ignoring the staging-prefix date."""
    if strip_prefix and key.startswith(strip_prefix):
        key = key[len(strip_prefix):]
    m = _PART_DATE_RE.search(key)
    return norm_date(m.group(1)) if m else None


def window_days() -> list[str]:
    """All inclusive calendar days in the scope window (YYYY-MM-DD)."""
    import datetime as _dt
    s = _dt.date.fromisoformat(WINDOW_START)
    e = _dt.date.fromisoformat(WINDOW_END_INCLUSIVE)
    out, cur = [], s
    while cur <= e:
        out.append(cur.isoformat())
        cur += _dt.timedelta(days=1)
    return out


def is_aggregate_selector(key: str) -> bool:
    """A key whose okx-style selector is an aggregate (e.g. selector=ALL_SWAP)
    is not base-ticker-scoped evidence and must not count as accepted."""
    m = re.search(r"/selector=([^/]+)/", key)
    if not m:
        return False
    sel = m.group(1)
    return sel.upper().startswith("ALL")


def parse_manifest(doc: dict, bucket: str) -> dict:
    """Normalise one manifest into common fields regardless of venue schema."""
    schema = doc.get("schema_version", "unknown")
    run_id = doc.get("run_id", "unknown")
    write_mode = str(doc.get("write_mode", ""))

    payload_keys: set[str] = set()
    payload_count = 0
    payload_bytes_claimed = 0
    error_count = 0
    gap_count = 0
    selector_violations = 0
    families: list[str] = []

    def collect(records, uri_field):
        for r in records or []:
            if not isinstance(r, dict):
                continue
            k = key_from_s3_uri(r.get(uri_field), bucket)
            if k:
                payload_keys.add(k)

    if schema == "binance-backfill-s3-manifest.v1":
        collect(doc.get("payload_records"), "s3_uri")
        payload_count = int(doc.get("completed_payload_object_count", 0))
        payload_bytes_claimed = int(doc.get("completed_payload_bytes", 0))
        error_count = _len(doc.get("errors"))
        gap_count = _len(doc.get("metadata_gaps"))
        families = sorted({r.get("family") for r in doc.get("payload_records", []) if isinstance(r, dict) and r.get("family")})
    elif schema == "okx-raw-staging-manifest.v1":
        collect(doc.get("payload_records"), "s3_uri")
        counts = doc.get("counts", {}) or {}
        payload_count = int(counts.get("payload_object_count", 0))
        payload_bytes_claimed = int(counts.get("payload_bytes", 0))
        error_count = int(counts.get("error_count", _len(doc.get("errors"))))
        sel = doc.get("selector_scope", {}) or {}
        selector_violations = _len(sel.get("payload_selector_scope_violations"))
        families = sorted({f"{r.get('family')}:{r.get('inst_type')}" for r in doc.get("payload_records", []) if isinstance(r, dict) and r.get("family")})
    elif schema == "deribit-raw-staging-manifest.v1":
        collect(doc.get("records"), "s3_uri")
        payload_count = int(doc.get("object_count_excluding_manifest", 0))
        payload_bytes_claimed = int(doc.get("bytes_excluding_manifest", 0))
        error_count = int(doc.get("error_count", _len(doc.get("errors"))))
        gap_count = _len(doc.get("known_gaps"))
        families = sorted(doc.get("source_families_uploaded", []) or [])
    elif schema == "bybit-backfill-s3-manifest.v1":
        collect(doc.get("payload_records"), "s3_uri")
        payload_count = int(doc.get("object_count_excluding_manifest", 0))
        payload_bytes_claimed = int(doc.get("bytes_excluding_manifest", 0))
        error_count = _len(doc.get("errors"))
        gap_count = _len(doc.get("remaining_work"))  # open REST pagination = gap
        families = sorted(doc.get("source_families_uploaded", []) or [])
    elif schema == "hyperliquid-core-backfill-manifest.v1":
        collect(doc.get("payload_records"), "s3_uri")
        payload_count = int(doc.get("completed_object_count", 0))
        payload_bytes_claimed = int(doc.get("completed_bytes", 0))
        error_count = _len(doc.get("errors"))
        gap_count = _len(doc.get("gaps"))
        families = sorted(doc.get("families_uploaded", []) or [])
    elif schema in ("hyperliquid-hip3-s3-staging-manifest.v1", "hyperliquid-hip4-s3-staging-manifest.v1"):
        collect(doc.get("uploads"), "s3_uri")
        counts = doc.get("counts", {}) or {}
        payload_count = int(counts.get("uploaded_objects_without_manifest", counts.get("raw_payloads", 0)))
        payload_bytes_claimed = int((doc.get("bytes", {}) or {}).get("uploaded_bytes_without_manifest", 0))
        error_count = int(counts.get("errors", _len(doc.get("errors"))))
        gap_count = _len(doc.get("gaps_and_unproven_families"))
        families = sorted(doc.get("source_families_uploaded", []) or [])
    elif schema in ("backfill-archive-s3-manifest.v1",  # polymarket streaming
                    "backfill-archive-s3-acceptance-manifest.v1"):  # debt promotion
        collect(doc.get("s3_payload_records"), "s3_uri")
        payload_count = int(doc.get("completed_object_count", 0))
        payload_bytes_claimed = int(doc.get("completed_bytes", 0))
        error_count = _len(doc.get("errors"))
        families = sorted(doc.get("families", [])) or ([doc["family"]] if doc.get("family") else [])
    elif schema == "backfill-archive-objects-manifest.v1":  # polymarket page1 (local-staging)
        collect(doc.get("raw_payload_records"), "s3_uri")  # no s3_uri -> empty
        payload_count = int(doc.get("completed_object_count", 0))
        payload_bytes_claimed = int(doc.get("completed_bytes", 0))
        error_count = _len(doc.get("errors"))
        fam = doc.get("family")
        families = [fam] if fam else []
    elif schema in ("backfill-ingest-manifest.v1",):  # source-proof-v3 (local-staging)
        collect(doc.get("raw_payload_records"), "s3_uri")
        payload_count = _len(doc.get("raw_payload_records"))
        error_count = _len(doc.get("errors"))
    else:
        # Generic fallback: try common payload list / uri field names.
        for lk in ("payload_records", "records", "uploads", "s3_payload_records", "raw_payload_records"):
            if isinstance(doc.get(lk), list):
                collect(doc.get(lk), "s3_uri")
                payload_count = max(payload_count, len(doc[lk]))
        error_count = _len(doc.get("errors")) or int(doc.get("error_count", 0))

    # The presence of concrete s3_uri payload bindings is itself the proof of S3
    # staging (some venues, e.g. deribit, omit a top-level write_mode and instead
    # carry write_policy.staging_only). Only an explicit local-staging / dry-run
    # mode disqualifies a manifest that does bind objects to S3.
    accepted_binding = (
        len(payload_keys) > 0
        and selector_violations == 0
        and write_mode not in ("local_staging", "dry_run")
    )

    return {
        "run_id": run_id,
        "schema_version": schema,
        "write_mode": write_mode or "(unset)",
        "payload_count_claimed": payload_count,
        "payload_bytes_claimed": payload_bytes_claimed,
        "bound_keys_count": len(payload_keys),
        "error_count": error_count,
        "gap_count": gap_count,
        "selector_violations": selector_violations,
        "accepted_binding": accepted_binding,
        "families": families,
        "_payload_keys": payload_keys,  # stripped before serialisation
    }


def build_fragment(bucket: str, staging_prefix: str, venue_prefix: str) -> dict:
    full_prefix = f"{staging_prefix}/{venue_prefix}"
    physical = list_physical(bucket, full_prefix)

    manifest_keys = [
        k for k in physical
        if k.endswith("manifest.json") and "/progress/" not in k
    ]

    manifests = []
    bound_keys: set[str] = set()
    with tempfile.TemporaryDirectory(prefix="ledger-mf-") as td:
        for i, mk in enumerate(sorted(manifest_keys)):
            local = os.path.join(td, f"m{i}.json")
            try:
                run(["aws", "s3", "cp", f"s3://{bucket}/{mk}", local, "--quiet"])
                with open(local) as f:
                    doc = json.load(f)
            except Exception as e:  # noqa: BLE001 - record, never crash the venue
                manifests.append({"run_id": mk, "schema_version": "PARSE_ERROR",
                                  "error": str(e), "accepted_binding": False,
                                  "payload_count_claimed": 0, "error_count": 0,
                                  "gap_count": 0, "selector_violations": 0,
                                  "write_mode": "(unreadable)", "families": [],
                                  "bound_keys_count": 0, "payload_bytes_claimed": 0})
                continue
            parsed = parse_manifest(doc, bucket)
            parsed["manifest_key"] = mk
            keys = parsed.pop("_payload_keys")
            if parsed["accepted_binding"]:
                bound_keys |= keys
            manifests.append(parsed)

    # Strip only the venue prefix (not its trailing slash) so the relative path
    # retains a leading "/" for marker anchoring while the prefix segment itself
    # cannot match a provenance marker.
    data_keys = {k for k in physical if not is_provenance(k, full_prefix)}
    prov_keys = set(physical) - data_keys

    # Aggregate-selector objects (e.g. okx selector=ALL_SWAP) are bound by a
    # manifest but are not base-ticker-scoped, so they cannot count as accepted
    # in-scope coverage; reclassify them to unaccepted with an explicit reason.
    aggregate_keys = {k for k in data_keys if is_aggregate_selector(k)}
    accepted_keys = (data_keys & bound_keys) - aggregate_keys
    unaccepted_keys = (data_keys - bound_keys) | (bound_keys & aggregate_keys & data_keys)

    # bound keys not physically present = manifest references a missing object
    dangling = bound_keys - set(physical)

    def total(keys):
        return {"objects": len(keys), "bytes": sum(physical[k] for k in keys if k in physical)}

    # Breakdown of unaccepted data by second-level sub-prefix under the venue.
    unacc_breakdown: dict[str, dict] = defaultdict(lambda: {"objects": 0, "bytes": 0})
    rel_start = len(full_prefix) + 1
    for k in unaccepted_keys:
        rel = k[rel_start:] if k.startswith(full_prefix + "/") else k
        seg = rel.split("/", 1)[0] if "/" in rel else rel
        unacc_breakdown[seg]["objects"] += 1
        unacc_breakdown[seg]["bytes"] += physical[k]

    failed_or_gap = [
        {"run_id": m["run_id"], "manifest_key": m.get("manifest_key"),
         "error_count": m["error_count"], "gap_count": m["gap_count"],
         "selector_violations": m["selector_violations"],
         "accepted_binding": m["accepted_binding"]}
        for m in manifests
        if m["error_count"] > 0 or m["gap_count"] > 0 or m["selector_violations"] > 0
    ]
    zero_payload = [
        {"run_id": m["run_id"], "manifest_key": m.get("manifest_key"),
         "schema_version": m["schema_version"], "write_mode": m["write_mode"]}
        for m in manifests
        if m["bound_keys_count"] == 0 and m["payload_count_claimed"] == 0
        and m["schema_version"] != "PARSE_ERROR"
    ]

    families = sorted({f for m in manifests for f in m.get("families", [])})

    # In-window date coverage from accepted data keys (best-effort: depends on a
    # dt=/date token in the key; venues without one report dated_objects=0).
    win = set(window_days())
    sp = full_prefix + "/"
    accepted_dates = {d for k in accepted_keys if (d := key_date(k, sp))}
    in_window = sorted(accepted_dates & win)
    all_data_dates = {d for k in data_keys if (d := key_date(k, sp))}
    scope_coverage = {
        "window_start": WINDOW_START,
        "window_end_inclusive": WINDOW_END_INCLUSIVE,
        "window_total_days": len(win),
        "accepted_dated_objects": sum(1 for k in accepted_keys if key_date(k, sp)),
        "accepted_distinct_dates": len(accepted_dates),
        "accepted_in_window_days": len(in_window),
        "accepted_missing_window_days": len(win - accepted_dates),
        "accepted_date_min": min(accepted_dates) if accepted_dates else None,
        "accepted_date_max": max(accepted_dates) if accepted_dates else None,
        "out_of_window_accepted_dates": sorted(accepted_dates - win),
        "all_data_date_min": min(all_data_dates) if all_data_dates else None,
        "all_data_date_max": max(all_data_dates) if all_data_dates else None,
        "missing_window_days_sample": sorted(win - accepted_dates)[:40],
    }

    return {
        "venue_prefix": venue_prefix,
        "reported_venue": VENUE_NAME.get(venue_prefix, venue_prefix),
        "s3_prefix": f"s3://{bucket}/{full_prefix}/",
        "physical": {
            "total_objects": len(physical),
            "total_bytes": sum(physical.values()),
            "data": total(data_keys),
            "provenance": total(prov_keys),
        },
        "accepted": total(accepted_keys),
        "aggregate_selector_excluded": {**total(aggregate_keys),
                                        "sample_keys": sorted(list(aggregate_keys))[:10]},
        "scope_coverage": scope_coverage,
        "unaccepted_physical": {
            **total(unaccepted_keys),
            "by_subprefix": dict(sorted(unacc_breakdown.items(),
                                        key=lambda kv: kv[1]["bytes"], reverse=True)),
            "sample_keys": sorted(list(unaccepted_keys))[:15],
        },
        "dangling_manifest_refs": {"objects": len(dangling),
                                   "sample_keys": sorted(list(dangling))[:15]},
        "manifest_count": len(manifests),
        "accepted_binding_manifest_count": sum(1 for m in manifests if m["accepted_binding"]),
        "failed_or_gap_manifests": failed_or_gap,
        "zero_payload_manifests": zero_payload,
        "families": families,
        "manifests": manifests,
    }


def combine(fragments_dir: str, bucket: str) -> dict:
    frags = []
    for fn in sorted(os.listdir(fragments_dir)):
        if fn.endswith(".json"):
            with open(os.path.join(fragments_dir, fn)) as f:
                frags.append(json.load(f))

    venues: dict[str, dict] = defaultdict(lambda: {
        "prefixes": [], "physical_objects": 0, "physical_bytes": 0,
        "data_objects": 0, "data_bytes": 0,
        "provenance_objects": 0, "provenance_bytes": 0,
        "accepted_objects": 0, "accepted_bytes": 0,
        "unaccepted_objects": 0, "unaccepted_bytes": 0,
        "aggregate_excluded_objects": 0,
        "failed_or_gap_manifest_count": 0, "zero_payload_manifest_count": 0,
        "accepted_in_window_days": 0, "window_total_days": 0,
        "accepted_date_min": None, "accepted_date_max": None,
        "families": set(),
    })
    for fr in frags:
        v = venues[fr["reported_venue"]]
        v["prefixes"].append(fr["venue_prefix"])
        v["physical_objects"] += fr["physical"]["total_objects"]
        v["physical_bytes"] += fr["physical"]["total_bytes"]
        v["data_objects"] += fr["physical"]["data"]["objects"]
        v["data_bytes"] += fr["physical"]["data"]["bytes"]
        v["provenance_objects"] += fr["physical"]["provenance"]["objects"]
        v["provenance_bytes"] += fr["physical"]["provenance"]["bytes"]
        v["accepted_objects"] += fr["accepted"]["objects"]
        v["accepted_bytes"] += fr["accepted"]["bytes"]
        v["unaccepted_objects"] += fr["unaccepted_physical"]["objects"]
        v["unaccepted_bytes"] += fr["unaccepted_physical"]["bytes"]
        v["aggregate_excluded_objects"] += fr.get("aggregate_selector_excluded", {}).get("objects", 0)
        v["failed_or_gap_manifest_count"] += len(fr["failed_or_gap_manifests"])
        v["zero_payload_manifest_count"] += len(fr["zero_payload_manifests"])
        sc = fr.get("scope_coverage", {})
        v["accepted_in_window_days"] = max(v["accepted_in_window_days"], sc.get("accepted_in_window_days", 0))
        v["window_total_days"] = max(v["window_total_days"], sc.get("window_total_days", 0))
        for key, agg in (("accepted_date_min", min), ("accepted_date_max", max)):
            d = sc.get(key)
            if d:
                v[key] = d if v[key] is None else agg(v[key], d)
        v["families"] |= set(fr["families"])
    for v in venues.values():
        v["families"] = sorted(v["families"])

    sums = lambda f: sum(v[f] for v in venues.values())
    grand = {
        "physical_objects": sums("physical_objects"),
        "physical_bytes": sums("physical_bytes"),
        "data_objects": sums("data_objects"),
        "data_bytes": sums("data_bytes"),
        "provenance_objects": sums("provenance_objects"),
        "provenance_bytes": sums("provenance_bytes"),
        "accepted_objects": sums("accepted_objects"),
        "accepted_bytes": sums("accepted_bytes"),
        "unaccepted_objects": sums("unaccepted_objects"),
        "unaccepted_bytes": sums("unaccepted_bytes"),
        "aggregate_excluded_objects": sums("aggregate_excluded_objects"),
    }
    return {
        "ledger_version": "backfill-coverage-ledger.v1",
        "bucket": bucket,
        "naming_rule": "PMXT reported as 'Polymarket (PMXT source)'; PMXT kept only in source/provenance.",
        "byte_anchoring": "All byte figures are live S3 object sizes, not manifest-claimed bytes.",
        "grand_total": grand,
        "by_reported_venue": dict(sorted(venues.items())),
        "fragments": frags,
    }


def gib(n: int) -> str:
    return f"{n/1024**3:,.2f} GiB"


def write_table(ledger: dict, path: str) -> None:
    rows = []
    for venue, v in ledger["by_reported_venue"].items():
        dr = "n/a"
        if v["accepted_date_min"]:
            dr = f"{v['accepted_date_min']}..{v['accepted_date_max']}"
        wd = f"{v['accepted_in_window_days']}/{v['window_total_days']}" if v["window_total_days"] else "n/a"
        rows.append(
            f"| {venue} | {', '.join(v['prefixes'])} | {v['data_objects']:,} | "
            f"{gib(v['data_bytes'])} | {v['accepted_objects']:,} | {gib(v['accepted_bytes'])} | "
            f"{v['unaccepted_objects']:,} | {gib(v['unaccepted_bytes'])} | {dr} | {wd} | "
            f"{v['provenance_objects']:,} | {v['failed_or_gap_manifest_count']} | {v['zero_payload_manifest_count']} |"
        )
    g = ledger["grand_total"]
    lines = [
        "# Backfill Coverage Ledger (machine-derived) - human view",
        "",
        f"Generated by `scripts/backfill_coverage_ledger.py`. Ledger schema "
        f"`backfill-coverage-ledger.v1`. Bucket `{ledger['bucket']}`.",
        "",
        "Rules baked into these numbers:",
        "- **Bytes are live S3 object sizes**, never manifest-claimed bytes.",
        "- **Object-key reconciliation**: a key bound by >=1 accepted-binding "
        "manifest is counted once (dedup-correct across retry runs).",
        "- **`accepted + unaccepted = data objects`** (not physical). Physical = "
        "data + provenance (manifests, checksums, source-proofs, progress shards).",
        f"- **Scope window** {WINDOW_START}..{WINDOW_END_INCLUSIVE} inclusive "
        f"({len(window_days())} days); bases {', '.join(SCOPE_BASES)}.",
        "- **Aggregate selectors** (e.g. okx `selector=ALL_SWAP`) are excluded "
        f"from accepted ({g['aggregate_excluded_objects']} objects moved to unaccepted).",
        "- PMXT is reported as `Polymarket (PMXT source)`.",
        "",
        "| Venue | S3 prefix(es) | Data objs | Data bytes | Accepted objs | "
        "Accepted bytes | Unaccepted objs | Unaccepted bytes | Accepted date range | "
        "In-window days | Provenance objs | Failed/gap mf | Zero-payload mf |",
        "|---|---|--:|--:|--:|--:|--:|--:|:--|:--:|--:|--:|--:|",
        *rows,
        f"| **TOTAL** | — | **{g['data_objects']:,}** | **{gib(g['data_bytes'])}** | "
        f"**{g['accepted_objects']:,}** | **{gib(g['accepted_bytes'])}** | "
        f"**{g['unaccepted_objects']:,}** | **{gib(g['unaccepted_bytes'])}** | — | — | "
        f"**{g['provenance_objects']:,}** | — | — |",
        "",
        f"Physical grand total (data + provenance): **{g['physical_objects']:,} objects "
        f"/ {gib(g['physical_bytes'])}** ({g['physical_bytes']:,} bytes).",
        "",
    ]
    with open(path, "w") as f:
        f.write("\n".join(lines))


def main() -> int:
    ap = argparse.ArgumentParser(description=__doc__)
    ap.add_argument("--bucket", default=DEFAULT_BUCKET)
    ap.add_argument("--staging-prefix", default=DEFAULT_STAGING_PREFIX)
    ap.add_argument("--prefix", help="single venue prefix -> fragment")
    ap.add_argument("--combine", action="store_true")
    ap.add_argument("--fragments-dir")
    ap.add_argument("--out", required=True)
    ap.add_argument("--table-out")
    args = ap.parse_args()

    if args.combine:
        if not args.fragments_dir:
            ap.error("--combine requires --fragments-dir")
        ledger = combine(args.fragments_dir, args.bucket)
        with open(args.out, "w") as f:
            json.dump(ledger, f, indent=2)
        if args.table_out:
            write_table(ledger, args.table_out)
        g = ledger["grand_total"]
        print(f"COMBINED: physical {g['physical_objects']:,} objs / {gib(g['physical_bytes'])}; "
              f"accepted {g['accepted_objects']:,} objs / {gib(g['accepted_bytes'])}; "
              f"unaccepted {g['unaccepted_objects']:,} objs / {gib(g['unaccepted_bytes'])}")
        return 0

    if not args.prefix:
        ap.error("provide --prefix <venue> or --combine")
    frag = build_fragment(args.bucket, args.staging_prefix, args.prefix)
    with open(args.out, "w") as f:
        json.dump(frag, f, indent=2)
    a, u, p = frag["accepted"], frag["unaccepted_physical"], frag["physical"]
    print(f"{args.prefix}: physical {p['total_objects']:,} objs / {gib(p['total_bytes'])}; "
          f"accepted {a['objects']:,} / {gib(a['bytes'])}; "
          f"unaccepted {u['objects']:,} / {gib(u['bytes'])}; "
          f"failed/gap mf {len(frag['failed_or_gap_manifests'])}; "
          f"zero-payload mf {len(frag['zero_payload_manifests'])}")
    return 0


if __name__ == "__main__":
    sys.exit(main())
