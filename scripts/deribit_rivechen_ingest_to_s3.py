#!/usr/bin/env python3
"""Stage the patched RiveChen merged-parquet Deribit trade output into S3 staging.

This is a parquet-staging sibling of ``backfill_deribit_to_s3.py``. Where that
runner stages raw Deribit REST payloads, this runner takes the *already-merged*
per-asset/per-kind parquet produced by the patched RiveChen
``deribit-historical-data`` tool (``scripts/gen_parquet.py``), window-filters its
trades, and stages the windowed parquet into the same locked Deribit S3 staging
area using the wrapper's content-addressed key layout and self-referential
manifest schema.

Source binding: the parquet is produced by the RiveChen
``deribit-historical-data`` tool (https://github.com/RiveChen/deribit-historical-data)
fetching the Deribit History API v2 (``https://history.deribit.com/api/v2/public``),
patched for USDC-linear alt assets (CURRENCY/QUERY_CURRENCY/BASE_CURRENCY split)
and an instrument-lifetime ``[WINDOW_START_MS, WINDOW_END_MS)`` filter. Each
trade row's ``timestamp`` field is Deribit trade time in epoch milliseconds.

Secrets: relies entirely on the ambient AWS CLI credential/region resolution in
the shell (default profile / env / instance role), exactly like the sibling
venue staging scripts. No SSM, no profile flag, no credential handling in code.
Run under a Python that has ``polars``/``pyarrow`` (the RiveChen ``.venv``).
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
# Hard-locked staging target (identical to backfill_deribit_to_s3.py).
# A new ingest into the same staging area MUST use exactly this prefix; the
# date segment 2026-06-01 and venue segment deribit are part of the locked
# literal. Do NOT reuse this constant for non-Deribit data.
# ---------------------------------------------------------------------------
APPROVED_S3_PREFIX = "s3://bolt-parquet/backfill-staging/2026-06-01/deribit"

# Manifest schema version for this parquet-staging family. Distinct from the
# REST-payload runner's "deribit-raw-staging-manifest.v1" because the object
# tree and per-record shape differ (one merged parquet per asset/kind vs many
# raw JSON payloads), but it shares the same self-referential fixpoint-hash
# procedure and stable_json serializer so it stays byte-compatible with the
# wrapper's manifest conventions.
MANIFEST_SCHEMA_VERSION = "deribit-rivechen-parquet-staging-manifest.v1"

# Source binding / provenance for the merged parquet (recorded, not hardcoded
# into any runtime path). These describe WHERE the parquet came from.
RIVECHEN_SOURCE = {
    "tool": "RiveChen/deribit-historical-data",
    "tool_repo": "https://github.com/RiveChen/deribit-historical-data",
    "deribit_history_api_base": "https://history.deribit.com/api/v2/public",
    "official_endpoint": "/api/v2/public/get_last_trades_by_instrument",
    "merge_script": "scripts/gen_parquet.py",
    "patch": (
        "CURRENCY/QUERY_CURRENCY/BASE_CURRENCY asset-vs-query split for USDC-linear "
        "alts plus instrument-lifetime [WINDOW_START_MS, WINDOW_END_MS) filter"
    ),
}

# RiveChen merge dedup identity (recorded in the manifest dedup stats).
DEDUP_SUBSET = ("instrument_name", "trade_seq")
DEDUP_KEEP = "first"

# The merged-parquet trade-time field (Deribit epoch-ms trade time).
TRADE_TIME_FIELD = "timestamp"

# Defaults mirror the requested one-off scope. CLI args override every one.
DEFAULT_ASSETS = ("BTC", "ETH", "SOL", "XRP")
DEFAULT_KINDS = ("option", "future")
DEFAULT_WINDOW_START_MS = 1777420800000  # 2026-04-29T00:00:00Z
DEFAULT_WINDOW_END_MS = 1780272000000    # 2026-06-01T00:00:00Z
DEFAULT_INPUT_ROOT = pathlib.Path("/private/tmp/deribit-historical-data/data")

USER_AGENT = "bolt-v2-deribit-rivechen-parquet-staging/1"


# ---------------------------------------------------------------------------
# Verbatim-reused helpers from backfill_deribit_to_s3.py. These MUST stay
# byte-identical in behaviour so manifests serialize/hash compatibly.
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
    """Content-addressed object key, identical layout to the REST-payload
    runner: {prefix}/raw/v1/run={run_id}/family={family}/{k}={v}.../object=<sha>.<ext>.
    The asset/kind layout is carried as sorted key=value path segments via attrs,
    so each object lands under .../asset=<ASSET>/kind=<KIND>/object=<sha>.parquet.
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
    at exactly this bucket/key, and exits non-zero (404) otherwise. Used to verify a
    staged object is really present before deleting its local source."""
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
    """Self-referential fixpoint hash routine reused verbatim from the REST
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
# Parquet window-filter + staging (RiveChen-specific).
# ---------------------------------------------------------------------------
def parquet_source_path(input_root: pathlib.Path, asset: str, kind: str) -> pathlib.Path:
    # Matches the RiveChen merged-parquet output: ./data/<ASSET>/<kind>.parquet
    return input_root / asset / f"{kind}.parquet"


def window_filter_parquet(
    source: pathlib.Path,
    dest: pathlib.Path,
    *,
    window_start_ms: int,
    window_end_ms: int,
) -> dict[str, Any]:
    """Read the merged parquet, keep only trades whose ``timestamp`` (epoch ms)
    is in ``[window_start_ms, window_end_ms)``, write the windowed parquet to
    ``dest`` (zstd, the RiveChen default), and return row-count / dedup stats.

    polars/pyarrow are imported lazily so ``--help`` works without them.
    """
    import polars as pl  # noqa: PLC0415 - lazy so the CLI is importable everywhere.

    lf = pl.scan_parquet(source)
    source_columns = lf.collect_schema().names()
    if TRADE_TIME_FIELD not in source_columns:
        raise ValueError(f"{source} has no '{TRADE_TIME_FIELD}' trade-time column; columns={source_columns}")

    total_rows = int(lf.select(pl.len()).collect().item())
    windowed = lf.filter(
        (pl.col(TRADE_TIME_FIELD) >= window_start_ms) & (pl.col(TRADE_TIME_FIELD) < window_end_ms)
    )
    df = windowed.collect()
    in_window_rows = int(df.height)

    # The RiveChen merge already dedups on (instrument_name, trade_seq); re-assert
    # it here so the staged window carries the same dedup identity and we can
    # report whether the window introduced any duplicate (instrument, seq) pair.
    dedup_subset = [c for c in DEDUP_SUBSET if c in df.columns]
    if dedup_subset:
        deduped = df.unique(subset=dedup_subset, keep=DEDUP_KEEP)
        deduped_rows = int(deduped.height)
    else:
        deduped = df
        deduped_rows = in_window_rows
    duplicates_removed = in_window_rows - deduped_rows

    timestamps = deduped.get_column(TRADE_TIME_FIELD)
    returned_start_ms = int(timestamps.min()) if deduped_rows else None
    returned_end_ms = int(timestamps.max()) if deduped_rows else None
    instrument_count = (
        int(deduped.get_column("instrument_name").n_unique()) if "instrument_name" in deduped.columns else None
    )

    dest.parent.mkdir(parents=True, exist_ok=True)
    # Sort for deterministic output so identical inputs produce an identical
    # parquet body (=> identical sha => idempotent content-addressed key).
    sort_cols = [c for c in ("instrument_name", "trade_seq", TRADE_TIME_FIELD) if c in deduped.columns]
    if sort_cols:
        deduped = deduped.sort(sort_cols)
    deduped.write_parquet(dest, compression="zstd")

    return {
        "schema_columns": source_columns,
        "trade_time_field": TRADE_TIME_FIELD,
        "source_row_count": total_rows,
        "in_window_row_count": in_window_rows,
        "duplicates_removed_in_window": duplicates_removed,
        "staged_row_count": deduped_rows,
        "dedup_subset": dedup_subset,
        "dedup_keep": DEDUP_KEEP,
        "instrument_count": instrument_count,
        "returned_start_utc": ms_to_utc(returned_start_ms) if returned_start_ms is not None else None,
        "returned_end_utc": ms_to_utc(returned_end_ms) if returned_end_ms is not None else None,
        "returned_start_ms": returned_start_ms,
        "returned_end_ms": returned_end_ms,
    }


def stage_asset_kind(
    *,
    scratch_root: pathlib.Path,
    s3_prefix: str,
    run_id: str,
    input_root: pathlib.Path,
    asset: str,
    kind: str,
    window_start_ms: int,
    window_end_ms: int,
) -> dict[str, Any]:
    """Window-filter one <asset>/<kind> merged parquet and stage it. Returns one
    manifest record (uploaded record on success, error record on failure)."""
    family = "rivechen_merged_trades"
    attrs = {"asset": asset, "kind": kind}
    source = parquet_source_path(input_root, asset, kind)
    if not source.exists():
        return {
            "family": family,
            "attrs": attrs,
            "error": repr(FileNotFoundError(f"missing merged parquet: {source}")),
        }

    # Stage the windowed parquet into the scratch mirror first (under a stable
    # temp name), hash it, then move it to its content-addressed local path so
    # the recursive uploader pushes it to the sha-keyed S3 object.
    staging_dir = scratch_root / "windowed" / asset / kind
    staged_tmp = staging_dir / f"{asset}-{kind}.windowed.parquet"
    try:
        stats = window_filter_parquet(
            source,
            staged_tmp,
            window_start_ms=window_start_ms,
            window_end_ms=window_end_ms,
        )
    except Exception as exc:  # noqa: BLE001 - one asset/kind failing must not abort the run.
        return {"family": family, "attrs": attrs, "error": repr(exc)}

    payload = staged_tmp.read_bytes()
    payload_hash = sha256_bytes(payload)
    s3_uri = s3_payload_uri(
        s3_prefix,
        run_id=run_id,
        family=family,
        attrs=attrs,
        extension="parquet",
        payload_hash=payload_hash,
    )
    local_path = local_path_for_s3(scratch_root, s3_uri)
    write_bytes(local_path, payload)
    staged_tmp.unlink(missing_ok=True)

    record: dict[str, Any] = {
        "family": family,
        "attrs": attrs,
        "asset": asset,
        "kind": kind,
        "source_local_path": str(source),
        "source": dict(RIVECHEN_SOURCE),
        "base_currency": asset,
        "settlement_note": (
            "Coin-settled for BTC/ETH; USDC-linear alts (e.g. SOL/XRP) are fetched under "
            "QUERY_CURRENCY=USDC with base_currency filtered to the asset by the RiveChen patch."
        ),
        "window": {
            "start_ms": window_start_ms,
            "end_ms": window_end_ms,
            "start_utc": ms_to_utc(window_start_ms),
            "end_utc": ms_to_utc(window_end_ms),
            "filter": f"[{window_start_ms}, {window_end_ms}) on '{TRADE_TIME_FIELD}' (epoch ms)",
        },
        "content_type": "application/vnd.apache.parquet",
        "bytes": len(payload),
        "sha256": payload_hash,
        "local_path": str(local_path),
        "s3_uri": s3_uri,
        # The run's single recursive `aws s3 cp --recursive` unconditionally
        # (re-)uploads every object staged under the run tree, so every staged
        # record is uploaded in this run -- there is no per-object skip. This
        # flag is set True once the object is staged into the run's upload tree;
        # it reports presence in the staged set, not a conditional upload.
        "staged_object_present": True,
    }
    record.update(stats)
    return record


def parse_csv_upper(value: str) -> list[str]:
    return [item.strip().upper() for item in value.split(",") if item.strip()]


def parse_csv_lower(value: str) -> list[str]:
    return [item.strip().lower() for item in value.split(",") if item.strip()]


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument("--s3-prefix", default=APPROVED_S3_PREFIX)
    parser.add_argument("--assets", default=",".join(DEFAULT_ASSETS), help="Comma-separated asset tickers.")
    parser.add_argument(
        "--kinds",
        default=",".join(DEFAULT_KINDS),
        help="Comma-separated RiveChen merge kinds; each must be one of {future, option}.",
    )
    parser.add_argument("--window-start-ms", type=int, default=DEFAULT_WINDOW_START_MS)
    parser.add_argument("--window-end-ms", type=int, default=DEFAULT_WINDOW_END_MS)
    parser.add_argument(
        "--input-root",
        type=pathlib.Path,
        default=DEFAULT_INPUT_ROOT,
        help="Root holding <ASSET>/<kind>.parquet merged outputs.",
    )
    parser.add_argument("--scratch-root", type=pathlib.Path)
    parser.add_argument(
        "--delete-local",
        action="store_true",
        help=(
            "Delete the local merged source parquet(s) AFTER a verified upload. "
            "OFF by default: the no-flag run RETAINS every source parquet so the "
            "out-of-window trades the window filter drops are never destroyed. "
            "Only pass this when you have an independent copy of the full merged source."
        ),
    )
    return parser.parse_args()


def main() -> int:
    args = parse_args()
    s3_prefix = normalize_s3_prefix(args.s3_prefix)
    assets = parse_csv_upper(args.assets)
    kinds = parse_csv_lower(args.kinds)
    for kind in kinds:
        if kind not in DEFAULT_KINDS:
            raise ValueError(f"--kinds entries must be in {sorted(DEFAULT_KINDS)}; got {kind!r}")
    window_start_ms = args.window_start_ms
    window_end_ms = args.window_end_ms
    if window_end_ms <= window_start_ms:
        raise ValueError("--window-end-ms must be greater than --window-start-ms")
    input_root: pathlib.Path = args.input_root

    generated_at = utc_now()
    # run_id is derived ONLY from the staging parameters that define the staged
    # content (prefix, assets, kinds, window, input root, schema). It deliberately
    # excludes generated_at (wall clock) so identical staging parameters always
    # produce the same run_id, hence the same content-addressed object keys, so a
    # re-run lands on the same keys and the s3-ls idempotency probe can skip them.
    # generated_at is recorded in the manifest body for provenance only.
    run_seed = stable_json(
        {
            "s3_prefix": s3_prefix,
            "assets": args.assets,
            "kinds": args.kinds,
            "window_start_ms": window_start_ms,
            "window_end_ms": window_end_ms,
            "input_root": str(input_root),
            "schema_version": MANIFEST_SCHEMA_VERSION,
        }
    )
    run_id = "deribit-rivechen-" + sha256_bytes(run_seed.encode("utf-8"))[:16]
    scratch_root = args.scratch_root or pathlib.Path(f"/private/tmp/bolt-v2-deribit-rivechen-{run_id}")
    if not str(scratch_root).startswith("/private/tmp/bolt-v2-deribit-rivechen-"):
        raise ValueError("--scratch-root must be under /private/tmp/bolt-v2-deribit-rivechen-*")
    scratch_root.mkdir(parents=True, exist_ok=True)

    records: list[dict[str, Any]] = []
    for asset in assets:
        for kind in kinds:
            records.append(
                stage_asset_kind(
                    scratch_root=scratch_root,
                    s3_prefix=s3_prefix,
                    run_id=run_id,
                    input_root=input_root,
                    asset=asset,
                    kind=kind,
                    window_start_ms=window_start_ms,
                    window_end_ms=window_end_ms,
                )
            )

    uploaded_records = [r for r in records if r.get("s3_uri")]
    errors = [r for r in records if r.get("error")]
    uploaded_bytes = sum(int(r.get("bytes") or 0) for r in uploaded_records)
    staged_row_total = sum(int(r.get("staged_row_count") or 0) for r in uploaded_records)
    in_window_row_total = sum(int(r.get("in_window_row_count") or 0) for r in uploaded_records)

    manifest: dict[str, Any] = {
        "schema_version": MANIFEST_SCHEMA_VERSION,
        "run_id": run_id,
        "generated_at": generated_at,
        "runner": pathlib.Path(__file__).name,
        "s3_prefix": s3_prefix,
        "source": dict(RIVECHEN_SOURCE),
        "window": {
            "start_ms": window_start_ms,
            "end_ms": window_end_ms,
            "start_utc": ms_to_utc(window_start_ms),
            "end_utc": ms_to_utc(window_end_ms),
            "trade_time_field": TRADE_TIME_FIELD,
            "filter": f"[{window_start_ms}, {window_end_ms}) on '{TRADE_TIME_FIELD}' (epoch ms)",
        },
        "bounds": {
            "requested_assets": assets,
            "requested_kinds": kinds,
            "input_root": str(input_root),
            "base_currencies": assets,
        },
        "write_policy": {
            "staging_only": True,
            "canonical_writes": False,
            "secrets_required": False,
            "source_native_rest_payloads": False,
            "merged_parquet_staging": True,
        },
        "source_retention": {
            # RETENTION is the default. The window filter stages only the
            # in-window slice, so deleting the merged source would irreversibly
            # destroy every out-of-window trade. The no-flag run therefore keeps
            # the source; deletion happens only when --delete-local is passed AND
            # the staged object is verified present in S3.
            "default_policy": "retain",
            "delete_requested": args.delete_local,
            "effective_policy": "delete" if args.delete_local else "retain",
            "delete_requires_verified_upload": True,
        },
        "dedup": {
            "subset": list(DEDUP_SUBSET),
            "keep": DEDUP_KEEP,
            "applied_by": "RiveChen gen_parquet.py merge (re-asserted on the windowed slice)",
        },
        "records": uploaded_records,
        "errors": errors,
        "source_families_uploaded": sorted({str(r["family"]) for r in uploaded_records}),
        "object_count_excluding_manifest": len(uploaded_records),
        "bytes_excluding_manifest": uploaded_bytes,
        "staged_row_count_total": staged_row_total,
        "in_window_row_count_total": in_window_row_total,
        "error_count": len(errors),
        "known_gaps": [
            "Trade coverage is bounded by what the RiveChen tool fetched into the merged "
            f"parquet; this runner only window-filters [{window_start_ms}, {window_end_ms}) on "
            f"'{TRADE_TIME_FIELD}' and re-asserts the (instrument_name, trade_seq) dedup, it does "
            "not re-fetch from Deribit.",
            "Only the 'future' and 'option' merge kinds are staged; perpetual/spot are not "
            "separate RiveChen merge outputs.",
        ],
    }
    manifest_s3_uri = f"{s3_prefix}/ingest-manifests/v1/run={run_id}/deribit-backfill-manifest.json"
    manifest["manifest_s3_uri"] = manifest_s3_uri
    manifest["total_s3_object_count_including_manifest"] = len(uploaded_records) + 1
    manifest_path = write_manifest(scratch_root, manifest)

    # Recursive-upload all staged parquet payloads, then the manifest last
    # (same ordering as the REST runner).
    upload_run_payloads_to_s3(scratch_root, s3_prefix, run_id)
    upload_to_s3(manifest_path, manifest_s3_uri)

    # Source-retention policy: RETAIN by default. The window filter stages only
    # the in-window slice, so the merged source holds out-of-window trades that
    # would be irreversibly lost on delete. The no-flag run therefore never
    # touches the source. Deletion happens ONLY when --delete-local is explicitly
    # passed, and even then only after the recursive payload upload + manifest
    # upload succeeded (both raise on non-zero exit) AND each staged object is
    # confirmed present in S3 by an exact-key head-object check.
    deleted_sources: list[str] = []
    delete_skipped: list[dict[str, str]] = []
    for record in uploaded_records:
        source_local = record.get("source_local_path")
        if not source_local:
            continue
        source_path = pathlib.Path(source_local)
        if not args.delete_local:
            delete_skipped.append({"source": source_local, "reason": "retained_default"})
            continue
        if not s3_object_exists(record["s3_uri"]):
            delete_skipped.append({"source": source_local, "reason": "s3_object_not_verified"})
            continue
        if source_path.exists():
            source_path.unlink()
            deleted_sources.append(source_local)
        else:
            delete_skipped.append({"source": source_local, "reason": "already_absent"})

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
                "staged_row_count_total": staged_row_total,
                "in_window_row_count_total": in_window_row_total,
                "source_families_uploaded": manifest["source_families_uploaded"],
                "staged_objects": [
                    {
                        "asset": r["asset"],
                        "kind": r["kind"],
                        "s3_uri": r["s3_uri"],
                        "sha256": r["sha256"],
                        "staged_row_count": r.get("staged_row_count"),
                        "staged_object_present": r.get("staged_object_present"),
                    }
                    for r in uploaded_records
                ],
                "source_retention": manifest["source_retention"],
                "deleted_source_parquet": deleted_sources,
                "delete_skipped": delete_skipped,
                "error_count": len(errors),
                "errors": errors,
                "known_gaps": manifest["known_gaps"],
            }
        )
    )
    return 1 if errors else 0


if __name__ == "__main__":
    raise SystemExit(main())
