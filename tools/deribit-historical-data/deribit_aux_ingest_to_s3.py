#!/usr/bin/env python3
"""Stage auxiliary Deribit parquet (DVOL, settlements, metadata, mark candles) into S3 staging.

This is an auxiliary-parquet sibling of ``deribit_rivechen_ingest_to_s3.py``.
Where that runner stages the patched RiveChen *merged trade* parquet, this runner
stages the *non-trade* Deribit artefacts that live alongside it -- the DVOL index
candles, instrument settlement records, instrument metadata snapshots, and mark
(option/future) candles -- into the SAME locked Deribit S3 staging area, using the
SAME content-addressed key layout and the SAME self-referential manifest fixpoint
procedure as the trades runner. Only the manifest ``schema_version``, the
``run_id`` prefix, the object ``family``, and the per-object attrs differ; the
serialization, hashing, and key-building helpers are reused verbatim so manifests
stay byte-compatible with the wrapper's conventions.

Layout: the runner walks ``<aux-root>/<family>/*.parquet`` for the four auxiliary
families ``{dvol, settlements, metadata, mark_candles}``. Each discovered parquet
is content-addressed and staged under the single object family
``deribit_options_auxiliary`` with path attrs ``{family_kind: <one of the four>,
scope: <currency-or-asset parsed from the filename>}`` -- e.g.
``.../family=deribit_options_auxiliary/family_kind=dvol/scope=BTC/object=<sha>.parquet``.

Source binding: these auxiliary artefacts are produced from the Deribit History
API v2 (``https://history.deribit.com/api/v2/public``) -- DVOL from
``get_volatility_index_data``, settlements from ``get_settlement_history_by_*``,
metadata from ``get_instrument(s)``, and mark candles from
``get_tradingview_chart_data`` -- by the same RiveChen
``deribit-historical-data`` toolchain that produced the merged trades. The
per-family endpoint is recorded in the manifest as provenance; it is NOT a runtime
path and is never used to fetch.

Secrets: relies entirely on the ambient AWS CLI credential/region resolution in
the shell (default profile / env / instance role), exactly like the sibling venue
staging scripts. No SSM, no profile flag, no credential handling in code. Run
under a Python that has ``polars``/``pyarrow`` (the RiveChen ``.venv``).

Source retention: the local source parquet is RETAINED by default (no delete).
There is no delete path in this runner -- the auxiliary artefacts are small and
the staged copy is content-addressed, so the source is always kept.
"""

from __future__ import annotations

import argparse
import datetime as dt
import hashlib
import json
import pathlib
import re
import subprocess
import sys
import urllib.parse
from typing import Any


# ---------------------------------------------------------------------------
# Hard-locked staging target (identical to deribit_rivechen_ingest_to_s3.py and
# backfill_deribit_to_s3.py). A new ingest into the same staging area MUST use
# exactly this prefix; the date segment 2026-06-01 and venue segment deribit are
# part of the locked literal. Do NOT reuse this constant for non-Deribit data.
# ---------------------------------------------------------------------------
APPROVED_S3_PREFIX = "s3://bolt-parquet/backfill-staging/2026-06-01/deribit"

# Manifest schema version for this auxiliary-parquet-staging family. Distinct
# from the trades runner's "deribit-rivechen-parquet-staging-manifest.v1" and the
# REST-payload runner's "deribit-raw-staging-manifest.v1" because the object tree
# and per-record shape differ (auxiliary DVOL/settlement/metadata/mark-candle
# parquet vs windowed merged trades vs raw JSON payloads). It shares the same
# self-referential fixpoint-hash procedure and stable_json serializer so it stays
# byte-compatible with the wrapper's manifest conventions.
MANIFEST_SCHEMA_VERSION = "deribit-options-auxiliary-staging-manifest.v1"

# The single object family every staged auxiliary parquet lands under. The four
# auxiliary kinds are carried as the ``family_kind`` path attr beneath it, not as
# separate object families, so all auxiliary objects share one family subtree.
DERIBIT_AUX_FAMILY = "deribit_options_auxiliary"

# The auxiliary kinds this runner stages, in fixed iteration order. Each maps to a
# ``<aux-root>/<family_kind>/`` directory of parquet files.
AUX_FAMILY_KINDS = ("dvol", "settlements", "metadata", "mark_candles")

# Source binding / provenance per auxiliary kind (recorded, not hardcoded into any
# runtime path). These describe WHERE the parquet came from -- the Deribit History
# API v2 endpoint each kind is derived from, via the RiveChen toolchain.
DERIBIT_HISTORY_API_BASE = "https://history.deribit.com/api/v2/public"
DERIBIT_HISTORY_API_HOST = "history.deribit.com"
AUX_SOURCE_BINDINGS: dict[str, dict[str, str]] = {
    "dvol": {
        "family_kind": "dvol",
        "endpoint": "/api/v2/public/get_volatility_index_data",
        "host": DERIBIT_HISTORY_API_HOST,
        "description": "Deribit DVOL volatility-index candles (per currency).",
    },
    "settlements": {
        "family_kind": "settlements",
        "endpoint": "/api/v2/public/get_settlement_history_by_currency",
        "host": DERIBIT_HISTORY_API_HOST,
        "description": "Deribit instrument settlement/delivery records (per currency).",
    },
    "metadata": {
        "family_kind": "metadata",
        "endpoint": "/api/v2/public/get_instruments",
        "host": DERIBIT_HISTORY_API_HOST,
        "description": "Deribit instrument metadata snapshot (per currency/asset).",
    },
    "mark_candles": {
        "family_kind": "mark_candles",
        "endpoint": "/api/v2/public/get_tradingview_chart_data",
        "host": DERIBIT_HISTORY_API_HOST,
        "description": "Deribit mark-price candles for options/futures (per scope).",
    },
}

# Common Deribit/UTC timestamp column names, in priority order. The first column
# present in a given parquet is treated as that object's time axis and used to
# report returned_start/end_utc. Deribit epoch timestamps are milliseconds.
TIMESTAMP_FIELD_CANDIDATES = (
    "timestamp",
    "settlement_timestamp",
    "creation_timestamp",
    "tick",
    "ts",
    "time",
    "date",
)

USER_AGENT = "bolt-v2-deribit-options-auxiliary-staging/1"

# Defaults. The aux-root is the only required input; CLI args override it.
DEFAULT_AUX_ROOT = pathlib.Path("/private/tmp/deribit-window-aux")


# ---------------------------------------------------------------------------
# Verbatim-reused helpers from deribit_rivechen_ingest_to_s3.py /
# backfill_deribit_to_s3.py. These MUST stay byte-identical in behaviour so
# manifests serialize/hash compatibly.
# (dt.timezone.utc is used instead of dt.UTC so the script runs under the
#  RiveChen Python 3.10 venv where polars lives; the Z-format output is
#  byte-identical to dt.UTC on 3.11+.)
# ---------------------------------------------------------------------------
def utc_now() -> str:
    return dt.datetime.now(dt.timezone.utc).replace(microsecond=0).isoformat().replace("+00:00", "Z")


def stable_json(value: Any) -> str:
    return json.dumps(value, indent=2, sort_keys=True, ensure_ascii=True) + "\n"


def sha256_bytes(payload: bytes) -> str:
    return hashlib.sha256(payload).hexdigest()


def normalize_s3_prefix(prefix: str) -> str:
    normalized = prefix.rstrip("/")
    if normalized != APPROVED_S3_PREFIX:
        raise ValueError(f"S3 prefix must be exactly {APPROVED_S3_PREFIX}")
    return normalized


def ms_to_utc(value: int) -> str:
    return (
        dt.datetime.fromtimestamp(value / 1000, dt.timezone.utc)
        .replace(microsecond=0)
        .isoformat()
        .replace("+00:00", "Z")
    )


def safe_segment(value: str) -> str:
    cleaned = re.sub(r"[^A-Za-z0-9._=-]+", "_", value)
    if not cleaned:
        raise ValueError("empty path segment")
    return cleaned


def s3_payload_uri(
    s3_prefix: str,
    *,
    run_id: str,
    family: str,
    attrs: dict[str, str],
    extension: str,
    payload_hash: str,
) -> str:
    """Content-addressed object key, identical layout to the trades runner:
    {prefix}/raw/v1/run={run_id}/family={family}/{k}={v}.../object=<sha>.<ext>.
    The auxiliary kind/scope layout is carried as sorted key=value path segments
    via attrs, so each object lands under
    .../family_kind=<KIND>/scope=<SCOPE>/object=<sha>.parquet.
    """
    parts = [normalize_s3_prefix(s3_prefix), "raw", "v1", f"run={run_id}", f"family={safe_segment(family)}"]
    for key in sorted(attrs):
        parts.append(f"{safe_segment(key)}={safe_segment(attrs[key])}")
    parts.append(f"object={payload_hash}.{extension}")
    return "/".join(parts)


def local_path_for_s3(scratch_root: pathlib.Path, s3_uri: str) -> pathlib.Path:
    parsed = urllib.parse.urlparse(s3_uri)
    return scratch_root / "uploaded-payloads" / parsed.netloc / parsed.path.lstrip("/")


def write_bytes(path: pathlib.Path, payload: bytes) -> dict[str, Any]:
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_bytes(payload)
    return {"local_path": str(path), "bytes": len(payload), "sha256": sha256_bytes(payload)}


def upload_to_s3(local_path: pathlib.Path, s3_uri: str) -> None:
    subprocess.run(["aws", "s3", "cp", str(local_path), s3_uri, "--only-show-errors"], check=True)


def upload_run_payloads_to_s3(scratch_root: pathlib.Path, s3_prefix: str, run_id: str) -> None:
    s3_run_prefix = f"{normalize_s3_prefix(s3_prefix)}/raw/v1/run={run_id}"
    local_run_root = local_path_for_s3(scratch_root, s3_run_prefix)
    if not local_run_root.exists():
        return
    subprocess.run(
        ["aws", "s3", "cp", str(local_run_root), s3_run_prefix, "--recursive", "--only-show-errors"],
        check=True,
    )


def s3_object_exists(s3_uri: str) -> bool:
    """Exact-key existence proof via ``aws s3api head-object``. Unlike ``aws s3 ls``
    (a prefix match that can return a different object whose key merely shares this
    URI as a prefix), ``head-object`` succeeds (exit 0) only when an object exists
    at exactly this bucket/key, and exits non-zero (404) otherwise. Used to record
    a staged object is really present in S3."""
    parsed = urllib.parse.urlparse(s3_uri)
    bucket = parsed.netloc
    key = parsed.path.lstrip("/")
    completed = subprocess.run(
        ["aws", "s3api", "head-object", "--bucket", bucket, "--key", key],
        check=False,
        capture_output=True,
        text=True,
    )
    return completed.returncode == 0


def manifest_payload(manifest: dict[str, Any]) -> bytes:
    """Self-referential fixpoint hash routine reused verbatim from the trades
    runner: iterate until manifest_sha256 / manifest_bytes /
    total_s3_bytes_including_manifest are mutually self-consistent."""
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


def write_manifest(scratch_root: pathlib.Path, manifest: dict[str, Any]) -> pathlib.Path:
    path = scratch_root / "ingest-manifests" / "v1" / f"run={manifest['run_id']}" / "deribit-backfill-manifest.json"
    write_bytes(path, manifest_payload(manifest))
    return path


# ---------------------------------------------------------------------------
# Auxiliary parquet discovery + staging (this runner's specifics).
# ---------------------------------------------------------------------------
def scope_from_filename(parquet_path: pathlib.Path) -> str:
    """Derive the ``scope`` attr (currency or asset) from the parquet filename.

    The auxiliary artefacts are named per currency/asset (e.g. ``BTC.parquet``,
    ``ETH-options.parquet``, ``SOL_settlements.parquet``). The scope is the
    leading run of ``[A-Za-z0-9]`` of the stem -- i.e. the ticker up to the first
    separator -- uppercased. If the stem has no leading alphanumeric run, the
    whole sanitized stem is used so nothing collides into an empty scope.
    """
    stem = parquet_path.stem
    match = re.match(r"[A-Za-z0-9]+", stem)
    if match:
        return match.group(0).upper()
    return safe_segment(stem).upper()


def read_parquet_stats(parquet_path: pathlib.Path) -> dict[str, Any]:
    """Read row count and (if a recognised timestamp column exists) the returned
    start/end UTC window from the parquet, treating Deribit epoch timestamps as
    milliseconds. polars/pyarrow are imported lazily so ``--help`` works without
    them."""
    import polars as pl  # noqa: PLC0415 - lazy so the CLI is importable everywhere.

    lf = pl.scan_parquet(parquet_path)
    columns = lf.collect_schema().names()
    row_count = int(lf.select(pl.len()).collect().item())

    time_field = next((c for c in TIMESTAMP_FIELD_CANDIDATES if c in columns), None)
    returned_start_utc: str | None = None
    returned_end_utc: str | None = None
    returned_start_ms: int | None = None
    returned_end_ms: int | None = None
    if time_field is not None and row_count:
        col = lf.select(pl.col(time_field)).collect().get_column(time_field)
        dtype = col.dtype
        if dtype.is_temporal():
            # Native datetime/date column: cast to epoch ms via polars, then format.
            epoch_ms = col.dt.epoch(time_unit="ms")
            min_ms = epoch_ms.min()
            max_ms = epoch_ms.max()
            if min_ms is not None and max_ms is not None:
                returned_start_ms = int(min_ms)
                returned_end_ms = int(max_ms)
        elif dtype.is_numeric():
            min_v = col.min()
            max_v = col.max()
            if min_v is not None and max_v is not None:
                returned_start_ms = int(min_v)
                returned_end_ms = int(max_v)
        # Non-numeric / non-temporal time columns (e.g. string instrument labels
        # misnamed) are recorded as having no derivable window rather than guessed.
        if returned_start_ms is not None:
            returned_start_utc = ms_to_utc(returned_start_ms)
        if returned_end_ms is not None:
            returned_end_utc = ms_to_utc(returned_end_ms)

    return {
        "schema_columns": columns,
        "row_count": row_count,
        "timestamp_field": time_field,
        "returned_start_utc": returned_start_utc,
        "returned_end_utc": returned_end_utc,
        "returned_start_ms": returned_start_ms,
        "returned_end_ms": returned_end_ms,
    }


def stage_aux_parquet(
    *,
    scratch_root: pathlib.Path,
    s3_prefix: str,
    run_id: str,
    family_kind: str,
    source: pathlib.Path,
) -> dict[str, Any]:
    """Content-address one auxiliary parquet and stage it into the run upload tree.
    Returns one manifest record (uploaded record on success, error record on
    failure). The local source is read but NEVER modified or deleted."""
    scope = scope_from_filename(source)
    attrs = {"family_kind": family_kind, "scope": scope}
    binding = AUX_SOURCE_BINDINGS[family_kind]

    try:
        stats = read_parquet_stats(source)
    except Exception as exc:  # noqa: BLE001 - one parquet failing must not abort the run.
        return {
            "family": DERIBIT_AUX_FAMILY,
            "attrs": attrs,
            "family_kind": family_kind,
            "scope": scope,
            "source_local_path": str(source),
            "error": repr(exc),
        }

    payload = source.read_bytes()
    payload_hash = sha256_bytes(payload)
    s3_uri = s3_payload_uri(
        s3_prefix,
        run_id=run_id,
        family=DERIBIT_AUX_FAMILY,
        attrs=attrs,
        extension="parquet",
        payload_hash=payload_hash,
    )
    local_path = local_path_for_s3(scratch_root, s3_uri)
    write_bytes(local_path, payload)

    record: dict[str, Any] = {
        "family": DERIBIT_AUX_FAMILY,
        "attrs": attrs,
        "family_kind": family_kind,
        "scope": scope,
        "source_local_path": str(source),
        "source_endpoint": binding["endpoint"],
        "source_host": binding["host"],
        "source_description": binding["description"],
        "content_type": "application/vnd.apache.parquet",
        "bytes": len(payload),
        "sha256": payload_hash,
        "row_count": stats["row_count"],
        "schema_columns": stats["schema_columns"],
        "timestamp_field": stats["timestamp_field"],
        "returned_start_utc": stats["returned_start_utc"],
        "returned_end_utc": stats["returned_end_utc"],
        "returned_start_ms": stats["returned_start_ms"],
        "returned_end_ms": stats["returned_end_ms"],
        "local_path": str(local_path),
        "s3_uri": s3_uri,
        # The run's single recursive `aws s3 cp --recursive` unconditionally
        # (re-)uploads every object staged under the run tree, so every staged
        # record is uploaded in this run. This flag reports presence in the staged
        # set once the object is written into the run's upload tree.
        "staged_object_present": True,
    }
    return record


def discover_aux_parquets(aux_root: pathlib.Path, family_kinds: list[str]) -> list[tuple[str, pathlib.Path]]:
    """Walk ``<aux-root>/<family_kind>/*.parquet`` for each requested kind, in the
    fixed family-kind order then sorted-by-path within each kind, so discovery is
    deterministic (=> stable run_id seed and stable manifest record order)."""
    discovered: list[tuple[str, pathlib.Path]] = []
    for family_kind in family_kinds:
        family_dir = aux_root / family_kind
        if not family_dir.is_dir():
            continue
        for parquet_path in sorted(family_dir.glob("*.parquet")):
            if parquet_path.is_file():
                discovered.append((family_kind, parquet_path))
    return discovered


def parse_csv_lower(value: str) -> list[str]:
    return [item.strip().lower() for item in value.split(",") if item.strip()]


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument("--s3-prefix", default=APPROVED_S3_PREFIX)
    parser.add_argument(
        "--aux-root",
        type=pathlib.Path,
        default=DEFAULT_AUX_ROOT,
        help="Root holding <family_kind>/*.parquet auxiliary outputs.",
    )
    parser.add_argument(
        "--family-kinds",
        default=",".join(AUX_FAMILY_KINDS),
        help=(
            "Comma-separated auxiliary kinds to stage; each must be one of "
            f"{sorted(AUX_FAMILY_KINDS)}."
        ),
    )
    parser.add_argument("--scratch-root", type=pathlib.Path)
    return parser.parse_args()


def main() -> int:
    args = parse_args()
    s3_prefix = normalize_s3_prefix(args.s3_prefix)
    family_kinds = parse_csv_lower(args.family_kinds)
    for family_kind in family_kinds:
        if family_kind not in AUX_FAMILY_KINDS:
            raise ValueError(f"--family-kinds entries must be in {sorted(AUX_FAMILY_KINDS)}; got {family_kind!r}")
    aux_root: pathlib.Path = args.aux_root

    generated_at = utc_now()

    discovered = discover_aux_parquets(aux_root, family_kinds)

    # The run_id is derived ONLY from the staging parameters that define the staged
    # content -- the prefix, the aux_root, the sorted (family_kind, scope) list of
    # discovered objects, and the schema version. It deliberately EXCLUDES
    # generated_at (wall clock) so identical staging parameters always produce the
    # same run_id, hence the same content-addressed object keys, so a re-run lands
    # on the same keys. generated_at is recorded in the manifest body for
    # provenance only.
    family_scope_pairs = sorted(
        {(family_kind, scope_from_filename(path)) for family_kind, path in discovered}
    )
    run_seed = stable_json(
        {
            "s3_prefix": s3_prefix,
            "aux_root": str(aux_root),
            "family_scope_pairs": [list(pair) for pair in family_scope_pairs],
            "schema_version": MANIFEST_SCHEMA_VERSION,
        }
    )
    run_id = "deribit-aux-" + sha256_bytes(run_seed.encode("utf-8"))[:16]
    scratch_root = args.scratch_root or pathlib.Path(f"/private/tmp/bolt-v2-deribit-aux-{run_id}")
    if not str(scratch_root).startswith("/private/tmp/bolt-v2-deribit-aux-"):
        raise ValueError("--scratch-root must be under /private/tmp/bolt-v2-deribit-aux-*")
    scratch_root.mkdir(parents=True, exist_ok=True)

    records: list[dict[str, Any]] = []
    for family_kind, source in discovered:
        records.append(
            stage_aux_parquet(
                scratch_root=scratch_root,
                s3_prefix=s3_prefix,
                run_id=run_id,
                family_kind=family_kind,
                source=source,
            )
        )

    uploaded_records = [r for r in records if r.get("s3_uri")]
    errors = [r for r in records if r.get("error")]
    uploaded_bytes = sum(int(r.get("bytes") or 0) for r in uploaded_records)
    row_total = sum(int(r.get("row_count") or 0) for r in uploaded_records)

    per_kind_counts: dict[str, int] = {}
    for record in uploaded_records:
        per_kind_counts[record["family_kind"]] = per_kind_counts.get(record["family_kind"], 0) + 1

    manifest: dict[str, Any] = {
        "schema_version": MANIFEST_SCHEMA_VERSION,
        "run_id": run_id,
        "generated_at": generated_at,
        "runner": pathlib.Path(__file__).name,
        "s3_prefix": s3_prefix,
        "object_family": DERIBIT_AUX_FAMILY,
        "source": {
            "tool": "RiveChen/deribit-historical-data",
            "tool_repo": "https://github.com/RiveChen/deribit-historical-data",
            "deribit_history_api_base": DERIBIT_HISTORY_API_BASE,
            "deribit_history_api_host": DERIBIT_HISTORY_API_HOST,
            "per_family_endpoint": {k: v["endpoint"] for k, v in AUX_SOURCE_BINDINGS.items()},
        },
        "bounds": {
            "aux_root": str(aux_root),
            "requested_family_kinds": family_kinds,
            "discovered_object_count": len(discovered),
            "family_scope_pairs": [list(pair) for pair in family_scope_pairs],
        },
        "write_policy": {
            "staging_only": True,
            "canonical_writes": False,
            "secrets_required": False,
            "source_native_rest_payloads": False,
            "merged_parquet_staging": False,
            "auxiliary_parquet_staging": True,
        },
        "source_retention": {
            # RETENTION is unconditional for this runner. The auxiliary artefacts
            # are small and the staged copy is content-addressed; there is no
            # delete path, so the source is always kept.
            "default_policy": "retain",
            "delete_requested": False,
            "effective_policy": "retain",
            "delete_path_exists": False,
        },
        "records": uploaded_records,
        "errors": errors,
        "source_families_uploaded": sorted({str(r["family"]) for r in uploaded_records}),
        "family_kinds_uploaded": sorted({str(r["family_kind"]) for r in uploaded_records}),
        "object_count_per_family_kind": per_kind_counts,
        "object_count_excluding_manifest": len(uploaded_records),
        "bytes_excluding_manifest": uploaded_bytes,
        "row_count_total": row_total,
        "error_count": len(errors),
        "known_gaps": [
            "Auxiliary coverage is bounded by what the RiveChen toolchain wrote into "
            f"<aux-root>/<family_kind>/*.parquet for the families {list(AUX_FAMILY_KINDS)}; "
            "this runner only content-addresses and stages those files, it does not "
            "re-fetch from Deribit.",
            "The 'scope' attr is parsed from the leading alphanumeric run of each "
            "filename stem; a parquet whose scope is not encoded in its filename will "
            "stage under that derived scope rather than a per-row scope.",
            "returned_start/end_utc are reported only when a recognised timestamp "
            f"column is present (one of {list(TIMESTAMP_FIELD_CANDIDATES)}); a parquet "
            "without one (e.g. pure metadata snapshots) stages with a null window.",
        ],
    }
    manifest_s3_uri = f"{s3_prefix}/ingest-manifests/v1/run={run_id}/deribit-backfill-manifest.json"
    manifest["manifest_s3_uri"] = manifest_s3_uri
    manifest["total_s3_object_count_including_manifest"] = len(uploaded_records) + 1
    manifest_path = write_manifest(scratch_root, manifest)

    # Recursive-upload all staged auxiliary parquet payloads, then the manifest
    # last (same ordering as the trades runner).
    upload_run_payloads_to_s3(scratch_root, s3_prefix, run_id)
    upload_to_s3(manifest_path, manifest_s3_uri)

    # Source-retention policy: RETAIN unconditionally. There is no delete path in
    # this runner -- the source parquet is read for hashing/staging and otherwise
    # left untouched. We still verify each staged object is present in S3 by an
    # exact-key head-object check and record that presence for provenance.
    staged_objects_verified: list[dict[str, Any]] = []
    for record in uploaded_records:
        present = s3_object_exists(record["s3_uri"])
        staged_objects_verified.append(
            {
                "family_kind": record["family_kind"],
                "scope": record["scope"],
                "s3_uri": record["s3_uri"],
                "sha256": record["sha256"],
                "row_count": record.get("row_count"),
                "s3_object_verified_present": present,
            }
        )

    print(
        stable_json(
            {
                "run_id": run_id,
                "manifest_path": str(manifest_path),
                "manifest_s3_uri": manifest_s3_uri,
                "manifest_sha256": manifest["manifest_sha256"],
                "object_count_excluding_manifest": len(uploaded_records),
                "total_s3_object_count_including_manifest": len(uploaded_records) + 1,
                "bytes_excluding_manifest": uploaded_bytes,
                "total_s3_bytes_including_manifest": manifest["total_s3_bytes_including_manifest"],
                "row_count_total": row_total,
                "object_count_per_family_kind": per_kind_counts,
                "source_families_uploaded": manifest["source_families_uploaded"],
                "family_kinds_uploaded": manifest["family_kinds_uploaded"],
                "staged_objects": staged_objects_verified,
                "source_retention": manifest["source_retention"],
                "error_count": len(errors),
                "errors": errors,
                "known_gaps": manifest["known_gaps"],
            }
        )
    )
    return 1 if errors else 0


if __name__ == "__main__":
    raise SystemExit(main())
