#!/usr/bin/env python3
"""Fail-closed guard for BTE-022 PMXT artifact-root storage proof."""

from __future__ import annotations

import argparse
import hashlib
import json
import sys
from pathlib import Path


REPO_ROOT = Path(__file__).resolve().parent.parent
REFERENCE_ROOT = Path("specs/023-nt-research-analytics-platform/reference")
PMXT_STORAGE_STATUS = REFERENCE_ROOT / "source-proof-pmxt-storage-proof-status.2026-06-17.json"
PMXT_STORAGE_STAGING_STATUS = (
    REFERENCE_ROOT / "source-proof-pmxt-storage-staging-status.2026-06-17.json"
)
BTE_022_STATUS = (
    REFERENCE_ROOT / "source-proof-nt-catalog-mapping-status.backtesting-engine-022.2026-06-08.json"
)
PMXT_SOURCE_PROOF_FIXTURE = (
    REFERENCE_ROOT / "source-proof-fixture.binary-option.polymarket-pmxt-official-free-pending.v1.json"
)
PMXT_SCHEMA_SAMPLE = (
    REFERENCE_ROOT / "source-proof-sample-inspection.polymarket-pmxt-v2-orderbook.2026-06-08.json"
)
PMXT_SOURCE_MANIFEST = (
    REFERENCE_ROOT
    / "backfill-source-universe-object-manifests/pmxt-polymarket-v2-current/manifest/source-universe-object-manifest.json"
)
PMXT_CATEGORY_MANIFEST = (
    REFERENCE_ROOT
    / "backfill-source-universe-object-manifests/pmxt-polymarket-v2-current/category-manifests/pmxt-polymarket-v2-object-manifest-orderbook.json"
)
PMXT_ARCHIVE_INDEX_MANIFEST = (
    REFERENCE_ROOT
    / "source-archive-index-manifests/pmxt-polymarket-v2-current/manifest/source-archive-index-manifest.json"
)
JUSTFILE = Path("justfile")

EXPECTED_SAMPLE_URL = "https://r2v2.pmxt.dev/polymarket_orderbook_2026-05-20T22.parquet"
EXPECTED_SOURCE_BINDING = "polymarket-parquet-archive-index"
EXPECTED_STATUS = "blocked_pmxt_artifact_root_storage_unproven"
EXPECTED_STAGING_STATUS = "partial_pmxt_artifact_root_storage_staged_source_proof_fixture_unstaged"
EXPECTED_CHECKED_AT_UTC = "2026-06-16T17:34:15Z"
EXPECTED_STAGING_CHECKED_AT_UTC = "2026-06-16T18:17:30Z"
EXPECTED_TASK_ID = "BACKTESTING_ENGINE-022"
RAW_SAMPLE_S3_URI = "s3://bolt-parquet/backfill-staging/pmxt/raw/v1/source=polymarket-v2-archive/family=order_book_snapshot_deltas/category=orderbook/dt=2026-05-20T22:00:00Z/object=etag-f99d7c5ea0f65a4ffbb0a51c7a948c0f-44.parquet"
SCHEMA_SAMPLE_S3_URI = "s3://bolt-parquet/backfill-staging/pmxt/source-proofs/v1/source=polymarket-v2-archive/family=order_book_snapshot_deltas/category=orderbook/dt=2026-05-20T22:00:00Z/schema/source-proof-sample-inspection.polymarket-pmxt-v2-orderbook.2026-06-08.json"
SOURCE_UNIVERSE_MANIFEST_S3_URI = "s3://bolt-parquet/backfill-staging/pmxt/source-proofs/v1/source=polymarket-v2-archive/family=order_book_snapshot_deltas/category=orderbook/dt=2026-05-20T22:00:00Z/manifest/source-universe-object-manifest.json"
CATEGORY_MANIFEST_S3_URI = "s3://bolt-parquet/backfill-staging/pmxt/source-proofs/v1/source=polymarket-v2-archive/family=order_book_snapshot_deltas/category=orderbook/dt=2026-05-20T22:00:00Z/manifest/pmxt-polymarket-v2-object-manifest-orderbook.json"
ARCHIVE_INDEX_MANIFEST_S3_URI = "s3://bolt-parquet/backfill-staging/pmxt/source-proofs/v1/source=polymarket-v2-archive/family=order_book_snapshot_deltas/category=orderbook/dt=2026-05-20T22:00:00Z/manifest/source-archive-index-manifest.json"
SOURCE_PROOF_FIXTURE_S3_URI = "s3://bolt-parquet/backfill-staging/pmxt/source-proofs/v1/source=polymarket-v2-archive/family=order_book_snapshot_deltas/category=orderbook/dt=2026-05-20T22:00:00Z/proof/source-proof-fixture.binary-option.polymarket-pmxt-official-free-pending.v1.json"
EXPECTED_SCHEMA_COLUMNS = (
    "timestamp_received",
    "timestamp",
    "market",
    "asset_id",
    "bids",
    "asks",
    "price",
    "size",
    "old_tick_size",
    "new_tick_size",
)
JUSTFILE_COMMANDS = (
    "python3 scripts/test_verify_bte_022_pmxt_storage_proof.py",
    "python3 scripts/verify_bte_022_pmxt_storage_proof.py",
)
SOURCE_FENCE_STATIC_COMMANDS = ("python3 scripts/run_fences.py",)
JUSTFILE_RECIPES = ("verify-bte-022-pmxt-storage-proof",)

TOP_LEVEL_KEYS = (
    "schema_version",
    "task_id",
    "source_binding",
    "checked_at_utc",
    "status",
    "bte_022_can_close",
    "current_source_proof",
    "source_universe_snapshot",
    "planned_manifest_raw_sample",
    "source_archive_index_head",
    "s3_head_check",
    "canonical_acceptance_blockers",
    "decision",
    "guard_verification",
    "committed_input_hashes",
)
CURRENT_SOURCE_PROOF_KEYS = (
    "path",
    "sha256",
    "source_proof_id",
    "source_binding",
    "usage_scope",
    "raw_sample_uri",
    "raw_sample_hash",
    "schema_sample_uri",
    "storage_check_outcome",
)
SOURCE_UNIVERSE_KEYS = (
    "path",
    "sha256",
    "manifest_id",
    "universe_id",
    "object_count",
    "accepted_bytes",
    "source_archive_index_manifest_id",
    "source_archive_index_snapshot_id",
    "staging_uri_template",
)
PLANNED_SAMPLE_KEYS = (
    "path",
    "sha256",
    "s3_uri",
    "source_url",
    "source_hash_algorithm",
    "source_hash",
    "bytes",
    "archive_date",
    "category",
    "symbol",
    "source_binding",
    "schema_columns",
)
ARCHIVE_HEAD_KEYS = (
    "path",
    "sha256",
    "page_number",
    "object_label",
    "archive_hour_utc",
    "source_url",
    "listed_size_label",
    "http_status",
    "content_length_bytes",
    "last_modified",
    "etag",
)
S3_HEAD_KEYS = (
    "status",
    "exit_code",
    "error_code",
    "command",
    "bucket",
    "key",
    "expected_content_length_bytes",
    "expected_etag",
    "observed_error",
)
GUARD_VERIFICATION_KEYS = ("script", "self_test", "just_recipe", "source_fence_static_recipe")
STAGING_TOP_LEVEL_KEYS = (
    "schema_version",
    "task_id",
    "source_binding",
    "checked_at_utc",
    "status",
    "bte_022_can_close",
    "raw_sample_download_verification",
    "staged_artifacts",
    "current_acceptance_blockers",
    "decision",
    "guard_verification",
)
RAW_DOWNLOAD_KEYS = ("source_url", "local_path", "bytes", "sha256", "fixture_raw_sample_hash")
STAGED_ARTIFACT_KEYS = ("id", "status", "source", "s3_uri", "sha256", "head_object")
STAGED_REPO_ARTIFACT_KEYS = ("id", "status", "repo_path", "s3_uri", "sha256", "head_object")
STAGED_FIXTURE_ARTIFACT_KEYS = (
    "id",
    "status",
    "repo_path",
    "s3_uri",
    "sha256",
    "head_object",
    "upload_status",
    "upload_blocker",
)
HEAD_PRESENT_KEYS = ("content_length", "etag", "last_modified")
HEAD_NOT_FOUND_KEYS = ("exit_code", "error_code", "observed_error")
HASH_TARGETS = {
    "pmxt_source_proof_fixture": PMXT_SOURCE_PROOF_FIXTURE,
    "pmxt_source_universe_manifest": PMXT_SOURCE_MANIFEST,
    "pmxt_category_manifest": PMXT_CATEGORY_MANIFEST,
    "pmxt_archive_index_manifest": PMXT_ARCHIVE_INDEX_MANIFEST,
}
STAGED_REPO_ARTIFACTS = {
    "schema_sample": (PMXT_SCHEMA_SAMPLE, SCHEMA_SAMPLE_S3_URI),
    "source_universe_manifest": (PMXT_SOURCE_MANIFEST, SOURCE_UNIVERSE_MANIFEST_S3_URI),
    "category_manifest": (PMXT_CATEGORY_MANIFEST, CATEGORY_MANIFEST_S3_URI),
    "archive_index_manifest": (PMXT_ARCHIVE_INDEX_MANIFEST, ARCHIVE_INDEX_MANIFEST_S3_URI),
}
BTE_DURABLE_BLOCKER_SNIPPETS = (
    "repo://specs/023-nt-research-analytics-platform/reference/source-proof-pmxt-storage-proof-status.2026-06-17.json",
    "repo://specs/023-nt-research-analytics-platform/reference/source-proof-pmxt-storage-staging-status.2026-06-17.json",
    "prior manifest-planned raw sample S3 URI HeadObject 404",
    "raw sample S3 staging now HeadObject-present",
    "source-proof fixture staging remains blocked",
    "schema_sample_uri",
    "STATIC-GATED scripts/verify_bte_022_pmxt_storage_proof.py",
)


def read_text(root: Path, rel_path: Path, findings: list[str]) -> str:
    try:
        return (root / rel_path).read_text(encoding="utf-8")
    except OSError as exc:
        findings.append(f"{rel_path}: unable to read file: {exc}")
        return ""


def read_json(root: Path, rel_path: Path, findings: list[str]) -> dict:
    text = read_text(root, rel_path, findings)
    if not text:
        return {}
    try:
        value = json.loads(text)
    except json.JSONDecodeError as exc:
        findings.append(f"{rel_path}: invalid JSON: {exc}")
        return {}
    if not isinstance(value, dict):
        findings.append(f"{rel_path}: expected top-level JSON object")
        return {}
    return value


def file_sha256(root: Path, rel_path: Path, findings: list[str]) -> str:
    try:
        return hashlib.sha256((root / rel_path).read_bytes()).hexdigest()
    except OSError as exc:
        findings.append(f"{rel_path}: unable to hash file: {exc}")
        return ""


def file_size(root: Path, rel_path: Path, findings: list[str]) -> int:
    try:
        return (root / rel_path).stat().st_size
    except OSError as exc:
        findings.append(f"{rel_path}: unable to stat file: {exc}")
        return -1


def repo_uri(rel_path: Path) -> str:
    return f"repo://{rel_path.as_posix()}"


def require_equal(rel_path: Path, label: str, actual: object, expected: object, findings: list[str]) -> None:
    if actual != expected:
        findings.append(f"{rel_path}: {label} expected {expected!r}, found {actual!r}")


def require_keys_equal(
    rel_path: Path, label: str, actual: object, expected: tuple[str, ...], findings: list[str]
) -> None:
    if not isinstance(actual, dict):
        findings.append(f"{rel_path}: {label} expected object")
        return
    actual_keys = set(actual.keys())
    expected_keys = set(expected)
    if actual_keys != expected_keys:
        findings.append(
            f"{rel_path}: {label} keys expected {sorted(expected_keys)!r}, found {sorted(actual_keys)!r}"
        )


def require_list_equal(
    rel_path: Path, label: str, actual: object, expected: tuple[str, ...], findings: list[str]
) -> None:
    if not isinstance(actual, list):
        findings.append(f"{rel_path}: {label} expected list")
        return
    require_equal(rel_path, label, tuple(actual), expected, findings)


def require_text_contains(rel_path: Path, label: str, text: object, expected: str, findings: list[str]) -> None:
    if not isinstance(text, str) or expected not in text:
        findings.append(f"{rel_path}: {label} must contain {expected!r}")


def nested_mapping(value: dict, key: str, rel_path: Path, findings: list[str]) -> dict:
    nested = value.get(key)
    if not isinstance(nested, dict):
        findings.append(f"{rel_path}: {key} expected object")
        return {}
    return nested


def split_s3_uri(uri: str) -> tuple[str, str]:
    if not uri.startswith("s3://"):
        return "", ""
    bucket_and_key = uri.removeprefix("s3://")
    bucket, _, key = bucket_and_key.partition("/")
    return bucket, key


def find_record(
    rel_path: Path,
    records: object,
    key: str,
    value: str,
    label: str,
    findings: list[str],
) -> dict:
    if not isinstance(records, list):
        findings.append(f"{rel_path}: {label} expected list")
        return {}
    matches = [record for record in records if isinstance(record, dict) and record.get(key) == value]
    if len(matches) != 1:
        findings.append(f"{rel_path}: {label} expected exactly one {key}={value!r}, found {len(matches)}")
        return {}
    return matches[0]


def just_recipe_commands(justfile: str, recipe_name: str) -> list[str]:
    commands: list[str] = []
    in_recipe = False
    for line in justfile.splitlines():
        stripped = line.strip()
        if stripped and not line.startswith((" ", "\t")) and ":" in stripped:
            in_recipe = stripped.split(":", 1)[0] == recipe_name
            continue
        if not in_recipe:
            continue
        if not stripped or stripped.startswith("#"):
            continue
        if line.startswith((" ", "\t")):
            commands.append(stripped)
        else:
            break
    return commands


def check_input_hashes(root: Path, status: dict, findings: list[str]) -> None:
    hashes = nested_mapping(status, "committed_input_hashes", PMXT_STORAGE_STATUS, findings)
    for name, rel_path in HASH_TARGETS.items():
        entry = hashes.get(name)
        if not isinstance(entry, dict):
            findings.append(f"{PMXT_STORAGE_STATUS}: committed_input_hashes.{name} expected object")
            continue
        require_keys_equal(PMXT_STORAGE_STATUS, f"committed_input_hashes.{name}", entry, ("path", "sha256"), findings)
        require_equal(PMXT_STORAGE_STATUS, f"committed_input_hashes.{name}.path", entry.get("path"), repo_uri(rel_path), findings)
        require_equal(
            PMXT_STORAGE_STATUS,
            f"committed_input_hashes.{name}.sha256",
            entry.get("sha256"),
            file_sha256(root, rel_path, findings),
            findings,
        )


def check_current_source_proof(root: Path, status: dict, fixture: dict, findings: list[str]) -> None:
    current = nested_mapping(status, "current_source_proof", PMXT_STORAGE_STATUS, findings)
    require_keys_equal(PMXT_STORAGE_STATUS, "current_source_proof", current, CURRENT_SOURCE_PROOF_KEYS, findings)
    require_equal(PMXT_STORAGE_STATUS, "current_source_proof.path", current.get("path"), repo_uri(PMXT_SOURCE_PROOF_FIXTURE), findings)
    require_equal(
        PMXT_STORAGE_STATUS,
        "current_source_proof.sha256",
        current.get("sha256"),
        file_sha256(root, PMXT_SOURCE_PROOF_FIXTURE, findings),
        findings,
    )
    for key in (
        "source_proof_id",
        "source_binding",
        "usage_scope",
        "raw_sample_uri",
        "raw_sample_hash",
        "schema_sample_uri",
    ):
        require_equal(PMXT_STORAGE_STATUS, f"current_source_proof.{key}", current.get(key), fixture.get(key), findings)
    storage_check = fixture.get("required_checks", {}).get("storage", {})
    require_equal(
        PMXT_STORAGE_STATUS,
        "current_source_proof.storage_check_outcome",
        current.get("storage_check_outcome"),
        storage_check.get("outcome"),
        findings,
    )
    require_equal(PMXT_STORAGE_STATUS, "current_source_proof.usage_scope", current.get("usage_scope"), "one_off_backfill_data", findings)
    require_text_contains(PMXT_STORAGE_STATUS, "current_source_proof.raw_sample_uri", current.get("raw_sample_uri"), "https://", findings)
    require_text_contains(PMXT_STORAGE_STATUS, "current_source_proof.schema_sample_uri", current.get("schema_sample_uri"), "repo://", findings)
    require_equal(PMXT_STORAGE_STATUS, "current_source_proof.storage_check_outcome", current.get("storage_check_outcome"), "pending", findings)


def check_source_universe(root: Path, status: dict, source_manifest: dict, findings: list[str]) -> None:
    universe = nested_mapping(status, "source_universe_snapshot", PMXT_STORAGE_STATUS, findings)
    require_keys_equal(PMXT_STORAGE_STATUS, "source_universe_snapshot", universe, SOURCE_UNIVERSE_KEYS, findings)
    require_equal(PMXT_STORAGE_STATUS, "source_universe_snapshot.path", universe.get("path"), repo_uri(PMXT_SOURCE_MANIFEST), findings)
    require_equal(
        PMXT_STORAGE_STATUS,
        "source_universe_snapshot.sha256",
        universe.get("sha256"),
        file_sha256(root, PMXT_SOURCE_MANIFEST, findings),
        findings,
    )
    for key in SOURCE_UNIVERSE_KEYS[2:]:
        require_equal(
            PMXT_STORAGE_STATUS,
            f"source_universe_snapshot.{key}",
            universe.get(key),
            source_manifest.get(key),
            findings,
        )


def check_planned_manifest_sample(root: Path, status: dict, category_manifest: dict, findings: list[str]) -> None:
    planned = nested_mapping(status, "planned_manifest_raw_sample", PMXT_STORAGE_STATUS, findings)
    require_keys_equal(PMXT_STORAGE_STATUS, "planned_manifest_raw_sample", planned, PLANNED_SAMPLE_KEYS, findings)
    require_equal(PMXT_STORAGE_STATUS, "planned_manifest_raw_sample.path", planned.get("path"), repo_uri(PMXT_CATEGORY_MANIFEST), findings)
    require_equal(
        PMXT_STORAGE_STATUS,
        "planned_manifest_raw_sample.sha256",
        planned.get("sha256"),
        file_sha256(root, PMXT_CATEGORY_MANIFEST, findings),
        findings,
    )
    record = find_record(
        PMXT_CATEGORY_MANIFEST,
        category_manifest.get("payload_records"),
        "source_url",
        EXPECTED_SAMPLE_URL,
        "payload_records",
        findings,
    )
    for key in PLANNED_SAMPLE_KEYS[2:]:
        if key == "schema_columns":
            require_list_equal(PMXT_STORAGE_STATUS, "planned_manifest_raw_sample.schema_columns", planned.get(key), EXPECTED_SCHEMA_COLUMNS, findings)
        else:
            require_equal(PMXT_STORAGE_STATUS, f"planned_manifest_raw_sample.{key}", planned.get(key), record.get(key), findings)


def check_archive_index_head(root: Path, status: dict, archive_manifest: dict, findings: list[str]) -> None:
    head = nested_mapping(status, "source_archive_index_head", PMXT_STORAGE_STATUS, findings)
    require_keys_equal(PMXT_STORAGE_STATUS, "source_archive_index_head", head, ARCHIVE_HEAD_KEYS, findings)
    require_equal(PMXT_STORAGE_STATUS, "source_archive_index_head.path", head.get("path"), repo_uri(PMXT_ARCHIVE_INDEX_MANIFEST), findings)
    require_equal(
        PMXT_STORAGE_STATUS,
        "source_archive_index_head.sha256",
        head.get("sha256"),
        file_sha256(root, PMXT_ARCHIVE_INDEX_MANIFEST, findings),
        findings,
    )
    record = find_record(
        PMXT_ARCHIVE_INDEX_MANIFEST,
        archive_manifest.get("records"),
        "source_url",
        EXPECTED_SAMPLE_URL,
        "records",
        findings,
    )
    for key in ARCHIVE_HEAD_KEYS[2:]:
        require_equal(PMXT_STORAGE_STATUS, f"source_archive_index_head.{key}", head.get(key), record.get(key), findings)


def check_s3_head(status: dict, findings: list[str]) -> None:
    planned = nested_mapping(status, "planned_manifest_raw_sample", PMXT_STORAGE_STATUS, findings)
    s3_check = nested_mapping(status, "s3_head_check", PMXT_STORAGE_STATUS, findings)
    require_keys_equal(PMXT_STORAGE_STATUS, "s3_head_check", s3_check, S3_HEAD_KEYS, findings)
    bucket, key = split_s3_uri(str(planned.get("s3_uri", "")))
    require_equal(PMXT_STORAGE_STATUS, "s3_head_check.status", s3_check.get("status"), "not_found", findings)
    require_equal(PMXT_STORAGE_STATUS, "s3_head_check.exit_code", s3_check.get("exit_code"), 254, findings)
    require_equal(PMXT_STORAGE_STATUS, "s3_head_check.error_code", s3_check.get("error_code"), "NotFound", findings)
    require_equal(PMXT_STORAGE_STATUS, "s3_head_check.bucket", s3_check.get("bucket"), bucket, findings)
    require_equal(PMXT_STORAGE_STATUS, "s3_head_check.key", s3_check.get("key"), key, findings)
    require_equal(
        PMXT_STORAGE_STATUS,
        "s3_head_check.expected_content_length_bytes",
        s3_check.get("expected_content_length_bytes"),
        planned.get("bytes"),
        findings,
    )
    require_equal(
        PMXT_STORAGE_STATUS,
        "s3_head_check.expected_etag",
        s3_check.get("expected_etag"),
        planned.get("source_hash"),
        findings,
    )
    command = s3_check.get("command")
    for snippet in ("aws s3api head-object", "--bucket bolt-parquet", key, "--query", "--output json"):
        require_text_contains(PMXT_STORAGE_STATUS, "s3_head_check.command", command, snippet, findings)
    require_text_contains(PMXT_STORAGE_STATUS, "s3_head_check.observed_error", s3_check.get("observed_error"), "HeadObject", findings)
    require_text_contains(PMXT_STORAGE_STATUS, "s3_head_check.observed_error", s3_check.get("observed_error"), "404", findings)


def check_blockers_and_decision(status: dict, findings: list[str]) -> None:
    blockers = status.get("canonical_acceptance_blockers")
    expected_blockers = (
        "manifest_planned_raw_sample_s3_uri_head_object_404",
        "current_source_proof_raw_sample_uri_is_https_not_staged_s3",
        "current_source_proof_schema_sample_uri_is_repo_uri_not_staged_s3",
        "current_source_proof_usage_scope_is_one_off_backfill_data",
        "current_source_proof_storage_check_outcome_is_pending",
        "coverage_retention_freshness_completeness_and_cost_checks_remain_pending",
    )
    require_list_equal(PMXT_STORAGE_STATUS, "canonical_acceptance_blockers", blockers, expected_blockers, findings)
    decision = status.get("decision")
    for snippet in (
        "Do not accept the PMXT source proof",
        "raw sample, schema sample, manifest, and evidence artifacts are staged under the artifact root",
        "BACKTESTING_ENGINE-022",
    ):
        require_text_contains(PMXT_STORAGE_STATUS, "decision", decision, snippet, findings)


def check_guard_verification(status: dict, justfile: str, findings: list[str]) -> None:
    guard = nested_mapping(status, "guard_verification", PMXT_STORAGE_STATUS, findings)
    require_keys_equal(PMXT_STORAGE_STATUS, "guard_verification", guard, GUARD_VERIFICATION_KEYS, findings)
    require_equal(PMXT_STORAGE_STATUS, "guard_verification.script", guard.get("script"), repo_uri(Path("scripts/verify_bte_022_pmxt_storage_proof.py")), findings)
    require_equal(PMXT_STORAGE_STATUS, "guard_verification.self_test", guard.get("self_test"), repo_uri(Path("scripts/test_verify_bte_022_pmxt_storage_proof.py")), findings)
    require_equal(PMXT_STORAGE_STATUS, "guard_verification.just_recipe", guard.get("just_recipe"), "verify-bte-022-pmxt-storage-proof", findings)
    require_equal(PMXT_STORAGE_STATUS, "guard_verification.source_fence_static_recipe", guard.get("source_fence_static_recipe"), "source-fence-static-inner", findings)
    for recipe in JUSTFILE_RECIPES:
        commands = just_recipe_commands(justfile, recipe)
        for command in JUSTFILE_COMMANDS:
            if command not in commands:
                findings.append(f"{JUSTFILE}: {recipe} missing command {command!r}")
    source_fence_commands = just_recipe_commands(justfile, "source-fence-static-inner")
    if not source_fence_commands:
        findings.append(f"{JUSTFILE}: missing recipe source-fence-static-inner")
        return
    if tuple(source_fence_commands) != SOURCE_FENCE_STATIC_COMMANDS:
        expected = " && ".join(SOURCE_FENCE_STATIC_COMMANDS)
        findings.append(f"{JUSTFILE}: source-fence-static-inner must contain only {expected}")


def artifact_by_id(status: dict, artifact_id: str, findings: list[str]) -> dict:
    artifacts = status.get("staged_artifacts")
    if not isinstance(artifacts, list):
        findings.append(f"{PMXT_STORAGE_STAGING_STATUS}: staged_artifacts expected list")
        return {}
    matches = [artifact for artifact in artifacts if isinstance(artifact, dict) and artifact.get("id") == artifact_id]
    if len(matches) != 1:
        findings.append(
            f"{PMXT_STORAGE_STAGING_STATUS}: staged_artifacts expected exactly one id={artifact_id!r}, found {len(matches)}"
        )
        return {}
    return matches[0]


def check_head_present(artifact_id: str, head: dict, content_length: int, etag: str, findings: list[str]) -> None:
    require_keys_equal(PMXT_STORAGE_STAGING_STATUS, f"{artifact_id}.head_object", head, HEAD_PRESENT_KEYS, findings)
    require_equal(
        PMXT_STORAGE_STAGING_STATUS,
        f"{artifact_id}.head_object.content_length",
        head.get("content_length"),
        content_length,
        findings,
    )
    require_equal(PMXT_STORAGE_STAGING_STATUS, f"{artifact_id}.head_object.etag", head.get("etag"), etag, findings)
    require_text_contains(
        PMXT_STORAGE_STAGING_STATUS,
        f"{artifact_id}.head_object.last_modified",
        head.get("last_modified"),
        "2026-06-16T18:",
        findings,
    )


def check_storage_staging_status(root: Path, staging_status: dict, fixture: dict, category_manifest: dict, findings: list[str]) -> None:
    require_keys_equal(PMXT_STORAGE_STAGING_STATUS, "top-level", staging_status, STAGING_TOP_LEVEL_KEYS, findings)
    require_equal(
        PMXT_STORAGE_STAGING_STATUS,
        "schema_version",
        staging_status.get("schema_version"),
        "source-proof-pmxt-storage-staging-status.v1",
        findings,
    )
    require_equal(PMXT_STORAGE_STAGING_STATUS, "task_id", staging_status.get("task_id"), EXPECTED_TASK_ID, findings)
    require_equal(
        PMXT_STORAGE_STAGING_STATUS,
        "source_binding",
        staging_status.get("source_binding"),
        EXPECTED_SOURCE_BINDING,
        findings,
    )
    require_equal(
        PMXT_STORAGE_STAGING_STATUS,
        "checked_at_utc",
        staging_status.get("checked_at_utc"),
        EXPECTED_STAGING_CHECKED_AT_UTC,
        findings,
    )
    require_equal(PMXT_STORAGE_STAGING_STATUS, "status", staging_status.get("status"), EXPECTED_STAGING_STATUS, findings)
    require_equal(PMXT_STORAGE_STAGING_STATUS, "bte_022_can_close", staging_status.get("bte_022_can_close"), False, findings)

    planned = find_record(
        PMXT_CATEGORY_MANIFEST,
        category_manifest.get("payload_records"),
        "source_url",
        EXPECTED_SAMPLE_URL,
        "payload_records",
        findings,
    )
    raw_download = nested_mapping(staging_status, "raw_sample_download_verification", PMXT_STORAGE_STAGING_STATUS, findings)
    require_keys_equal(PMXT_STORAGE_STAGING_STATUS, "raw_sample_download_verification", raw_download, RAW_DOWNLOAD_KEYS, findings)
    require_equal(PMXT_STORAGE_STAGING_STATUS, "raw_sample_download_verification.source_url", raw_download.get("source_url"), EXPECTED_SAMPLE_URL, findings)
    require_equal(PMXT_STORAGE_STAGING_STATUS, "raw_sample_download_verification.bytes", raw_download.get("bytes"), planned.get("bytes"), findings)
    require_equal(PMXT_STORAGE_STAGING_STATUS, "raw_sample_download_verification.sha256", raw_download.get("sha256"), fixture.get("raw_sample_hash"), findings)
    require_equal(
        PMXT_STORAGE_STAGING_STATUS,
        "raw_sample_download_verification.fixture_raw_sample_hash",
        raw_download.get("fixture_raw_sample_hash"),
        fixture.get("raw_sample_hash"),
        findings,
    )

    raw = artifact_by_id(staging_status, "raw_sample", findings)
    require_keys_equal(PMXT_STORAGE_STAGING_STATUS, "raw_sample", raw, STAGED_ARTIFACT_KEYS, findings)
    require_equal(PMXT_STORAGE_STAGING_STATUS, "raw_sample.status", raw.get("status"), "present", findings)
    require_equal(PMXT_STORAGE_STAGING_STATUS, "raw_sample.source", raw.get("source"), EXPECTED_SAMPLE_URL, findings)
    require_equal(PMXT_STORAGE_STAGING_STATUS, "raw_sample.s3_uri", raw.get("s3_uri"), RAW_SAMPLE_S3_URI, findings)
    require_equal(PMXT_STORAGE_STAGING_STATUS, "raw_sample.sha256", raw.get("sha256"), fixture.get("raw_sample_hash"), findings)
    check_head_present(
        "raw_sample",
        nested_mapping(raw, "head_object", PMXT_STORAGE_STAGING_STATUS, findings),
        planned.get("bytes"),
        planned.get("source_hash"),
        findings,
    )

    for artifact_id, (rel_path, s3_uri) in STAGED_REPO_ARTIFACTS.items():
        artifact = artifact_by_id(staging_status, artifact_id, findings)
        require_keys_equal(PMXT_STORAGE_STAGING_STATUS, artifact_id, artifact, STAGED_REPO_ARTIFACT_KEYS, findings)
        require_equal(PMXT_STORAGE_STAGING_STATUS, f"{artifact_id}.status", artifact.get("status"), "present", findings)
        require_equal(PMXT_STORAGE_STAGING_STATUS, f"{artifact_id}.repo_path", artifact.get("repo_path"), rel_path.as_posix(), findings)
        require_equal(PMXT_STORAGE_STAGING_STATUS, f"{artifact_id}.s3_uri", artifact.get("s3_uri"), s3_uri, findings)
        require_equal(
            PMXT_STORAGE_STAGING_STATUS,
            f"{artifact_id}.sha256",
            artifact.get("sha256"),
            file_sha256(root, rel_path, findings),
            findings,
        )
        require_equal(
            PMXT_STORAGE_STAGING_STATUS,
            f"{artifact_id}.head_object.content_length",
            nested_mapping(artifact, "head_object", PMXT_STORAGE_STAGING_STATUS, findings).get("content_length"),
            file_size(root, rel_path, findings),
            findings,
        )
        require_text_contains(
            PMXT_STORAGE_STAGING_STATUS,
            f"{artifact_id}.head_object.last_modified",
            nested_mapping(artifact, "head_object", PMXT_STORAGE_STAGING_STATUS, findings).get("last_modified"),
            "2026-06-16T18:",
            findings,
        )

    fixture_artifact = artifact_by_id(staging_status, "source_proof_fixture", findings)
    require_keys_equal(
        PMXT_STORAGE_STAGING_STATUS,
        "source_proof_fixture",
        fixture_artifact,
        STAGED_FIXTURE_ARTIFACT_KEYS,
        findings,
    )
    require_equal(PMXT_STORAGE_STAGING_STATUS, "source_proof_fixture.status", fixture_artifact.get("status"), "not_found", findings)
    require_equal(PMXT_STORAGE_STAGING_STATUS, "source_proof_fixture.repo_path", fixture_artifact.get("repo_path"), PMXT_SOURCE_PROOF_FIXTURE.as_posix(), findings)
    require_equal(PMXT_STORAGE_STAGING_STATUS, "source_proof_fixture.s3_uri", fixture_artifact.get("s3_uri"), SOURCE_PROOF_FIXTURE_S3_URI, findings)
    require_equal(
        PMXT_STORAGE_STAGING_STATUS,
        "source_proof_fixture.sha256",
        fixture_artifact.get("sha256"),
        file_sha256(root, PMXT_SOURCE_PROOF_FIXTURE, findings),
        findings,
    )
    fixture_head = nested_mapping(fixture_artifact, "head_object", PMXT_STORAGE_STAGING_STATUS, findings)
    require_keys_equal(PMXT_STORAGE_STAGING_STATUS, "source_proof_fixture.head_object", fixture_head, HEAD_NOT_FOUND_KEYS, findings)
    require_equal(PMXT_STORAGE_STAGING_STATUS, "source_proof_fixture.head_object.exit_code", fixture_head.get("exit_code"), 254, findings)
    require_equal(PMXT_STORAGE_STAGING_STATUS, "source_proof_fixture.head_object.error_code", fixture_head.get("error_code"), "NotFound", findings)
    require_text_contains(PMXT_STORAGE_STAGING_STATUS, "source_proof_fixture.head_object.observed_error", fixture_head.get("observed_error"), "404", findings)
    require_equal(
        PMXT_STORAGE_STAGING_STATUS,
        "source_proof_fixture.upload_status",
        fixture_artifact.get("upload_status"),
        "blocked_by_approval_reviewer",
        findings,
    )
    require_text_contains(
        PMXT_STORAGE_STAGING_STATUS,
        "source_proof_fixture.upload_blocker",
        fixture_artifact.get("upload_blocker"),
        "explicit user approval",
        findings,
    )

    expected_blockers = (
        "source_proof_fixture_not_staged_to_s3",
        "current_source_proof_raw_sample_uri_is_https_not_staged_s3",
        "current_source_proof_schema_sample_uri_is_repo_uri_not_staged_s3",
        "current_source_proof_usage_scope_is_one_off_backfill_data",
        "current_source_proof_storage_check_outcome_is_pending",
        "instrument_universe_coverage_retention_freshness_completeness_and_cost_checks_remain_pending",
        "source_selection_status_is_pending_more_proof",
    )
    require_list_equal(
        PMXT_STORAGE_STAGING_STATUS,
        "current_acceptance_blockers",
        staging_status.get("current_acceptance_blockers"),
        expected_blockers,
        findings,
    )
    decision = staging_status.get("decision")
    for snippet in (
        "raw sample, schema inspection, source-universe manifest, category manifest, and archive-index manifest are now staged",
        "source-proof fixture itself is not staged",
        "Do not accept the PMXT source proof",
        "one_off_backfill_data",
    ):
        require_text_contains(PMXT_STORAGE_STAGING_STATUS, "decision", decision, snippet, findings)
    check_guard_verification(staging_status, read_text(root, JUSTFILE, findings), findings)


def check_bte_status(bte_status: dict, findings: list[str]) -> None:
    require_equal(BTE_022_STATUS, "bte_022_can_close", bte_status.get("bte_022_can_close"), False, findings)
    blockers = bte_status.get("remaining_blockers")
    if not isinstance(blockers, list):
        findings.append(f"{BTE_022_STATUS}: remaining_blockers expected list")
        return
    durable = [
        blocker
        for blocker in blockers
        if isinstance(blocker, dict) and blocker.get("blocker") == "durable_source_selection_unproven"
    ]
    if len(durable) != 1:
        findings.append(f"{BTE_022_STATUS}: expected exactly one durable_source_selection_unproven blocker, found {len(durable)}")
        return
    evidence = durable[0].get("required_evidence")
    for snippet in BTE_DURABLE_BLOCKER_SNIPPETS:
        require_text_contains(BTE_022_STATUS, "durable_source_selection_unproven.required_evidence", evidence, snippet, findings)


def scan_root(root: Path) -> list[str]:
    findings: list[str] = []
    status = read_json(root, PMXT_STORAGE_STATUS, findings)
    staging_status = read_json(root, PMXT_STORAGE_STAGING_STATUS, findings)
    fixture = read_json(root, PMXT_SOURCE_PROOF_FIXTURE, findings)
    source_manifest = read_json(root, PMXT_SOURCE_MANIFEST, findings)
    category_manifest = read_json(root, PMXT_CATEGORY_MANIFEST, findings)
    archive_manifest = read_json(root, PMXT_ARCHIVE_INDEX_MANIFEST, findings)
    bte_status = read_json(root, BTE_022_STATUS, findings)
    justfile = read_text(root, JUSTFILE, findings)

    require_keys_equal(PMXT_STORAGE_STATUS, "top-level", status, TOP_LEVEL_KEYS, findings)
    require_equal(PMXT_STORAGE_STATUS, "schema_version", status.get("schema_version"), "source-proof-pmxt-storage-proof-status.v1", findings)
    require_equal(PMXT_STORAGE_STATUS, "task_id", status.get("task_id"), EXPECTED_TASK_ID, findings)
    require_equal(PMXT_STORAGE_STATUS, "source_binding", status.get("source_binding"), EXPECTED_SOURCE_BINDING, findings)
    require_equal(PMXT_STORAGE_STATUS, "checked_at_utc", status.get("checked_at_utc"), EXPECTED_CHECKED_AT_UTC, findings)
    require_equal(PMXT_STORAGE_STATUS, "status", status.get("status"), EXPECTED_STATUS, findings)
    require_equal(PMXT_STORAGE_STATUS, "bte_022_can_close", status.get("bte_022_can_close"), False, findings)

    check_input_hashes(root, status, findings)
    check_current_source_proof(root, status, fixture, findings)
    check_source_universe(root, status, source_manifest, findings)
    check_planned_manifest_sample(root, status, category_manifest, findings)
    check_archive_index_head(root, status, archive_manifest, findings)
    check_s3_head(status, findings)
    check_blockers_and_decision(status, findings)
    check_guard_verification(status, justfile, findings)
    check_storage_staging_status(root, staging_status, fixture, category_manifest, findings)
    check_bte_status(bte_status, findings)
    return findings


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--root", type=Path, default=REPO_ROOT, help="Repository root to scan.")
    args = parser.parse_args(argv)
    findings = scan_root(args.root)
    if findings:
        for finding in findings:
            print(f"FINDING: {finding}", file=sys.stderr)
        return 1
    print("BTE-022 PMXT storage proof guard passed.")
    return 0


if __name__ == "__main__":
    import lane_governor

    lane_governor.acquire()
    raise SystemExit(main())
