#!/usr/bin/env python3
"""Self-tests for verify_bte_022_pmxt_storage_proof.py."""

from __future__ import annotations

import importlib.util
import json
import subprocess
import sys
import tempfile
from pathlib import Path


REPO_ROOT = Path(__file__).resolve().parent.parent
SCRIPT = REPO_ROOT / "scripts" / "verify_bte_022_pmxt_storage_proof.py"


def load_verifier():
    spec = importlib.util.spec_from_file_location("verify_bte_022_pmxt_storage_proof", SCRIPT)
    if spec is None or spec.loader is None:
        raise RuntimeError(f"unable to load verifier from {SCRIPT}")
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


def write_file(root: Path, rel: Path, text: str) -> None:
    path = root / rel
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(text, encoding="utf-8")


def write_json(root: Path, rel: Path, value: dict) -> None:
    write_file(root, rel, json.dumps(value, indent=2, sort_keys=True) + "\n")


def fixture_json() -> dict:
    return {
        "source_proof_id": "source-proof-polymarket-pmxt-v2-orderbook-binary-option-pending-2026-06-08",
        "source_binding": "polymarket-parquet-archive-index",
        "usage_scope": "one_off_backfill_data",
        "raw_sample_uri": "https://r2v2.pmxt.dev/polymarket_orderbook_2026-05-20T22.parquet",
        "raw_sample_hash": "0de44455fde7aedd6678fa30cc1ef86ba215eaf70fb3f7b9735510e1371f6567",
        "schema_sample_uri": "repo://specs/023-nt-research-analytics-platform/reference/source-proof-sample-inspection.polymarket-pmxt-v2-orderbook.2026-06-08.json",
        "required_checks": {
            "storage": {
                "outcome": "pending",
                "evidence_ref": "pending artifact-root staging proof",
            }
        },
    }


def source_manifest_json() -> dict:
    return {
        "manifest_id": "backfill-source-universe-object-manifest-pmxt-polymarket-v2-current",
        "universe_id": "backfill-source-universe-pmxt-polymarket-v2-current",
        "object_count": 1351,
        "accepted_bytes": 557815904970,
        "source_archive_index_manifest_id": "source-archive-index-manifest-pmxt-polymarket-v2-current",
        "source_archive_index_snapshot_id": "source-archive-index-snapshot-pmxt-polymarket-v2-current-2026-06-10T15",
        "staging_uri_template": "s3://bolt-parquet/backfill-staging/pmxt/raw/v1/source={source}/family={table_family}/category={category}/dt={archive_date}/object={source_hash}.parquet",
    }


def category_manifest_json() -> dict:
    return {
        "payload_records": [
            {
                "s3_uri": "s3://bolt-parquet/backfill-staging/pmxt/raw/v1/source=polymarket-v2-archive/family=order_book_snapshot_deltas/category=orderbook/dt=2026-05-20T22:00:00Z/object=etag-f99d7c5ea0f65a4ffbb0a51c7a948c0f-44.parquet",
                "source_url": "https://r2v2.pmxt.dev/polymarket_orderbook_2026-05-20T22.parquet",
                "source_hash_algorithm": "r2_multipart_etag",
                "source_hash": "\"f99d7c5ea0f65a4ffbb0a51c7a948c0f-44\"",
                "bytes": 361365244,
                "archive_date": "2026-05-20T22:00:00Z",
                "category": "orderbook",
                "symbol": "POLYMARKET",
                "source_binding": "polymarket-parquet-archive-index",
                "schema_columns": [
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
                ],
            }
        ]
    }


def archive_index_json() -> dict:
    return {
        "records": [
            {
                "page_number": 10,
                "object_label": "polymarket_orderbook_2026-05-20T22",
                "archive_hour_utc": "2026-05-20T22:00:00Z",
                "source_url": "https://r2v2.pmxt.dev/polymarket_orderbook_2026-05-20T22.parquet",
                "listed_size_label": "344.6 MB",
                "http_status": 200,
                "content_length_bytes": 361365244,
                "last_modified": "Wed, 20 May 2026 23:07:44 GMT",
                "etag": "\"f99d7c5ea0f65a4ffbb0a51c7a948c0f-44\"",
            }
        ]
    }


def bte_status_json() -> dict:
    return {
        "bte_022_can_close": False,
        "remaining_blockers": [
            {
                "blocker": "durable_source_selection_unproven",
                "required_evidence": (
                    "repo://specs/023-nt-research-analytics-platform/reference/source-proof-pmxt-storage-proof-status.2026-06-17.json "
                    "records the prior manifest-planned raw sample S3 URI HeadObject 404. "
                    "repo://specs/023-nt-research-analytics-platform/reference/source-proof-pmxt-storage-staging-status.2026-06-17.json "
                    "records that raw sample S3 staging now HeadObject-present while source-proof fixture staging remains blocked and schema_sample_uri remains unstaged. "
                    "STATIC-GATED scripts/verify_bte_022_pmxt_storage_proof.py rejects drift."
                ),
            }
        ],
    }


def justfile_text() -> str:
    return """verify-bte-022-pmxt-storage-proof: check-workspace
    python3 scripts/test_verify_bte_022_pmxt_storage_proof.py
    python3 scripts/verify_bte_022_pmxt_storage_proof.py

source-fence-static-inner: require-local-verification-gate check-workspace require-rust-verification-owner
    python3 scripts/run_fences.py
"""


def status_json(root: Path, module) -> dict:
    fixture = fixture_json()
    source_manifest = source_manifest_json()
    category_record = category_manifest_json()["payload_records"][0]
    archive_record = archive_index_json()["records"][0]
    fixture_hash = module.file_sha256(root, module.PMXT_SOURCE_PROOF_FIXTURE, [])
    source_hash = module.file_sha256(root, module.PMXT_SOURCE_MANIFEST, [])
    category_hash = module.file_sha256(root, module.PMXT_CATEGORY_MANIFEST, [])
    archive_hash = module.file_sha256(root, module.PMXT_ARCHIVE_INDEX_MANIFEST, [])
    return {
        "schema_version": "source-proof-pmxt-storage-proof-status.v1",
        "task_id": "BACKTESTING_ENGINE-022",
        "source_binding": "polymarket-parquet-archive-index",
        "checked_at_utc": "2026-06-16T17:34:15Z",
        "status": "blocked_pmxt_artifact_root_storage_unproven",
        "bte_022_can_close": False,
        "current_source_proof": {
            "path": module.repo_uri(module.PMXT_SOURCE_PROOF_FIXTURE),
            "sha256": fixture_hash,
            "source_proof_id": fixture["source_proof_id"],
            "source_binding": fixture["source_binding"],
            "usage_scope": fixture["usage_scope"],
            "raw_sample_uri": fixture["raw_sample_uri"],
            "raw_sample_hash": fixture["raw_sample_hash"],
            "schema_sample_uri": fixture["schema_sample_uri"],
            "storage_check_outcome": "pending",
        },
        "source_universe_snapshot": {
            "path": module.repo_uri(module.PMXT_SOURCE_MANIFEST),
            "sha256": source_hash,
            **source_manifest,
        },
        "planned_manifest_raw_sample": {
            "path": module.repo_uri(module.PMXT_CATEGORY_MANIFEST),
            "sha256": category_hash,
            **category_record,
        },
        "source_archive_index_head": {
            "path": module.repo_uri(module.PMXT_ARCHIVE_INDEX_MANIFEST),
            "sha256": archive_hash,
            **archive_record,
        },
        "s3_head_check": {
            "status": "not_found",
            "exit_code": 254,
            "error_code": "NotFound",
            "command": (
                "aws s3api head-object --bucket bolt-parquet --key "
                "backfill-staging/pmxt/raw/v1/source=polymarket-v2-archive/family=order_book_snapshot_deltas/category=orderbook/dt=2026-05-20T22:00:00Z/object=etag-f99d7c5ea0f65a4ffbb0a51c7a948c0f-44.parquet "
                "--query {ContentLength:ContentLength,ETag:ETag,LastModified:LastModified} --output json"
            ),
            "bucket": "bolt-parquet",
            "key": "backfill-staging/pmxt/raw/v1/source=polymarket-v2-archive/family=order_book_snapshot_deltas/category=orderbook/dt=2026-05-20T22:00:00Z/object=etag-f99d7c5ea0f65a4ffbb0a51c7a948c0f-44.parquet",
            "expected_content_length_bytes": 361365244,
            "expected_etag": "\"f99d7c5ea0f65a4ffbb0a51c7a948c0f-44\"",
            "observed_error": "An error occurred (404) when calling the HeadObject operation: Not Found",
        },
        "canonical_acceptance_blockers": [
            "manifest_planned_raw_sample_s3_uri_head_object_404",
            "current_source_proof_raw_sample_uri_is_https_not_staged_s3",
            "current_source_proof_schema_sample_uri_is_repo_uri_not_staged_s3",
            "current_source_proof_usage_scope_is_one_off_backfill_data",
            "current_source_proof_storage_check_outcome_is_pending",
            "coverage_retention_freshness_completeness_and_cost_checks_remain_pending",
        ],
        "decision": (
            "Do not accept the PMXT source proof, canonicalize the source, authorize broad backfill, or close BACKTESTING_ENGINE-022 "
            "until raw sample, schema sample, manifest, and evidence artifacts are staged under the artifact root and the source proof storage check is accepted."
        ),
        "guard_verification": {
            "script": "repo://scripts/verify_bte_022_pmxt_storage_proof.py",
            "self_test": "repo://scripts/test_verify_bte_022_pmxt_storage_proof.py",
            "just_recipe": "verify-bte-022-pmxt-storage-proof",
            "source_fence_static_recipe": "source-fence-static-inner",
        },
        "committed_input_hashes": {
            "pmxt_source_proof_fixture": {
                "path": module.repo_uri(module.PMXT_SOURCE_PROOF_FIXTURE),
                "sha256": fixture_hash,
            },
            "pmxt_source_universe_manifest": {
                "path": module.repo_uri(module.PMXT_SOURCE_MANIFEST),
                "sha256": source_hash,
            },
            "pmxt_category_manifest": {
                "path": module.repo_uri(module.PMXT_CATEGORY_MANIFEST),
                "sha256": category_hash,
            },
            "pmxt_archive_index_manifest": {
                "path": module.repo_uri(module.PMXT_ARCHIVE_INDEX_MANIFEST),
                "sha256": archive_hash,
            },
        },
    }


def staging_status_json(root: Path, module) -> dict:
    fixture = fixture_json()
    category_record = category_manifest_json()["payload_records"][0]
    return {
        "schema_version": "source-proof-pmxt-storage-staging-status.v1",
        "task_id": "BACKTESTING_ENGINE-022",
        "source_binding": "polymarket-parquet-archive-index",
        "checked_at_utc": "2026-06-16T18:17:30Z",
        "status": "partial_pmxt_artifact_root_storage_staged_source_proof_fixture_unstaged",
        "bte_022_can_close": False,
        "raw_sample_download_verification": {
            "source_url": fixture["raw_sample_uri"],
            "local_path": "/private/tmp/pmxt-polymarket_orderbook_2026-05-20T22.parquet",
            "bytes": category_record["bytes"],
            "sha256": fixture["raw_sample_hash"],
            "fixture_raw_sample_hash": fixture["raw_sample_hash"],
        },
        "staged_artifacts": [
            {
                "id": "raw_sample",
                "status": "present",
                "source": fixture["raw_sample_uri"],
                "s3_uri": module.RAW_SAMPLE_S3_URI,
                "sha256": fixture["raw_sample_hash"],
                "head_object": {
                    "content_length": category_record["bytes"],
                    "etag": category_record["source_hash"],
                    "last_modified": "2026-06-16T18:16:13+00:00",
                },
            },
            {
                "id": "schema_sample",
                "status": "present",
                "repo_path": module.PMXT_SCHEMA_SAMPLE.as_posix(),
                "s3_uri": module.SCHEMA_SAMPLE_S3_URI,
                "sha256": module.file_sha256(root, module.PMXT_SCHEMA_SAMPLE, []),
                "head_object": {
                    "content_length": module.file_size(root, module.PMXT_SCHEMA_SAMPLE, []),
                    "etag": "\"5e5302ed5ba634be6147f41a50db5f23\"",
                    "last_modified": "2026-06-16T18:17:03+00:00",
                },
            },
            {
                "id": "source_universe_manifest",
                "status": "present",
                "repo_path": module.PMXT_SOURCE_MANIFEST.as_posix(),
                "s3_uri": module.SOURCE_UNIVERSE_MANIFEST_S3_URI,
                "sha256": module.file_sha256(root, module.PMXT_SOURCE_MANIFEST, []),
                "head_object": {
                    "content_length": module.file_size(root, module.PMXT_SOURCE_MANIFEST, []),
                    "etag": "\"86318ae203d219b1f5cd7ccdb38459c9\"",
                    "last_modified": "2026-06-16T18:17:07+00:00",
                },
            },
            {
                "id": "category_manifest",
                "status": "present",
                "repo_path": module.PMXT_CATEGORY_MANIFEST.as_posix(),
                "s3_uri": module.CATEGORY_MANIFEST_S3_URI,
                "sha256": module.file_sha256(root, module.PMXT_CATEGORY_MANIFEST, []),
                "head_object": {
                    "content_length": module.file_size(root, module.PMXT_CATEGORY_MANIFEST, []),
                    "etag": "\"3e9d6a9d2d7147faeee831d55f0adf27\"",
                    "last_modified": "2026-06-16T18:17:06+00:00",
                },
            },
            {
                "id": "archive_index_manifest",
                "status": "present",
                "repo_path": module.PMXT_ARCHIVE_INDEX_MANIFEST.as_posix(),
                "s3_uri": module.ARCHIVE_INDEX_MANIFEST_S3_URI,
                "sha256": module.file_sha256(root, module.PMXT_ARCHIVE_INDEX_MANIFEST, []),
                "head_object": {
                    "content_length": module.file_size(root, module.PMXT_ARCHIVE_INDEX_MANIFEST, []),
                    "etag": "\"29a16340aeb09ccfdac5c5d215a9717f\"",
                    "last_modified": "2026-06-16T18:17:06+00:00",
                },
            },
            {
                "id": "source_proof_fixture",
                "status": "not_found",
                "repo_path": module.PMXT_SOURCE_PROOF_FIXTURE.as_posix(),
                "s3_uri": module.SOURCE_PROOF_FIXTURE_S3_URI,
                "sha256": module.file_sha256(root, module.PMXT_SOURCE_PROOF_FIXTURE, []),
                "head_object": {
                    "exit_code": 254,
                    "error_code": "NotFound",
                    "observed_error": "An error occurred (404) when calling the HeadObject operation: Not Found",
                },
                "upload_status": "blocked_by_approval_reviewer",
                "upload_blocker": "explicit user approval required before uploading workspace source-proof fixture to S3",
            },
        ],
        "current_acceptance_blockers": [
            "source_proof_fixture_not_staged_to_s3",
            "current_source_proof_raw_sample_uri_is_https_not_staged_s3",
            "current_source_proof_schema_sample_uri_is_repo_uri_not_staged_s3",
            "current_source_proof_usage_scope_is_one_off_backfill_data",
            "current_source_proof_storage_check_outcome_is_pending",
            "instrument_universe_coverage_retention_freshness_completeness_and_cost_checks_remain_pending",
            "source_selection_status_is_pending_more_proof",
        ],
        "decision": (
            "The PMXT raw sample, schema inspection, source-universe manifest, category manifest, and archive-index manifest are now staged "
            "and HeadObject-present under the artifact root. Do not accept the PMXT source proof, canonicalize the source, authorize broad backfill, "
            "or close BACKTESTING_ENGINE-022 because the source-proof fixture itself is not staged, the committed source proof still uses HTTPS/repo URIs, "
            "remains one_off_backfill_data, and retains pending source-proof checks."
        ),
        "guard_verification": {
            "script": "repo://scripts/verify_bte_022_pmxt_storage_proof.py",
            "self_test": "repo://scripts/test_verify_bte_022_pmxt_storage_proof.py",
            "just_recipe": "verify-bte-022-pmxt-storage-proof",
            "source_fence_static_recipe": "source-fence-static-inner",
        },
    }


def write_complete_fixture(root: Path, module) -> None:
    write_json(root, module.PMXT_SOURCE_PROOF_FIXTURE, fixture_json())
    write_json(root, module.PMXT_SCHEMA_SAMPLE, {"schema": "sample"})
    write_json(root, module.PMXT_SOURCE_MANIFEST, source_manifest_json())
    write_json(root, module.PMXT_CATEGORY_MANIFEST, category_manifest_json())
    write_json(root, module.PMXT_ARCHIVE_INDEX_MANIFEST, archive_index_json())
    write_json(root, module.BTE_022_STATUS, bte_status_json())
    write_file(root, module.JUSTFILE, justfile_text())
    write_json(root, module.PMXT_STORAGE_STATUS, status_json(root, module))
    write_json(root, module.PMXT_STORAGE_STAGING_STATUS, staging_status_json(root, module))


def with_fixture(test_fn) -> None:
    module = load_verifier()
    with tempfile.TemporaryDirectory() as tmp:
        root = Path(tmp)
        write_complete_fixture(root, module)
        test_fn(root, module)


def read_json(root: Path, rel: Path) -> dict:
    return json.loads((root / rel).read_text(encoding="utf-8"))


def overwrite_json(root: Path, rel: Path, mutator) -> None:
    value = read_json(root, rel)
    mutator(value)
    write_json(root, rel, value)


def test_complete_fixture_passes() -> None:
    def check(root: Path, module) -> None:
        assert module.scan_root(root) == []

    with_fixture(check)


def test_s3_head_present_is_a_finding() -> None:
    def check(root: Path, module) -> None:
        overwrite_json(root, module.PMXT_STORAGE_STATUS, lambda value: value["s3_head_check"].update({"status": "present"}))
        findings = module.scan_root(root)
        assert any("s3_head_check.status" in finding for finding in findings), findings

    with_fixture(check)


def test_missing_bte_storage_reference_is_a_finding() -> None:
    def check(root: Path, module) -> None:
        def mutate(value: dict) -> None:
            value["remaining_blockers"][0]["required_evidence"] = "durable source still pending"

        overwrite_json(root, module.BTE_022_STATUS, mutate)
        findings = module.scan_root(root)
        assert any("source-proof-pmxt-storage-proof-status.2026-06-17.json" in finding for finding in findings), findings

    with_fixture(check)


def test_staged_source_proof_fixture_present_is_a_finding() -> None:
    def check(root: Path, module) -> None:
        def mutate(value: dict) -> None:
            for artifact in value["staged_artifacts"]:
                if artifact["id"] == "source_proof_fixture":
                    artifact["status"] = "present"

        overwrite_json(root, module.PMXT_STORAGE_STAGING_STATUS, mutate)
        findings = module.scan_root(root)
        assert any("source_proof_fixture.status" in finding for finding in findings), findings

    with_fixture(check)


def test_missing_justfile_command_is_a_finding() -> None:
    def check(root: Path, module) -> None:
        write_file(root, module.JUSTFILE, "verify-bte-022-pmxt-storage-proof: check-workspace\n")
        findings = module.scan_root(root)
        assert any("source-fence-static-inner" in finding for finding in findings), findings

    with_fixture(check)


def test_malformed_justfile_recipe_header_is_a_finding() -> None:
    def check(root: Path, module) -> None:
        write_file(
            root,
            module.JUSTFILE,
            """ verify-bte-022-pmxt-storage-proof: check-workspace
    python3 scripts/test_verify_bte_022_pmxt_storage_proof.py
    python3 scripts/verify_bte_022_pmxt_storage_proof.py

source-fence-static-inner: require-local-verification-gate check-workspace require-rust-verification-owner
    python3 scripts/run_fences.py
""",
        )
        findings = module.scan_root(root)
        assert any("verify-bte-022-pmxt-storage-proof missing command" in finding for finding in findings), findings

    with_fixture(check)


def test_committed_hash_drift_is_a_finding() -> None:
    def check(root: Path, module) -> None:
        overwrite_json(root, module.PMXT_STORAGE_STATUS, lambda value: value["committed_input_hashes"]["pmxt_category_manifest"].update({"sha256": "0" * 64}))
        findings = module.scan_root(root)
        assert any("pmxt_category_manifest.sha256" in finding for finding in findings), findings

    with_fixture(check)


def test_cli_fails_with_actionable_output() -> None:
    module = load_verifier()
    with tempfile.TemporaryDirectory() as tmp:
        root = Path(tmp)
        write_complete_fixture(root, module)
        overwrite_json(root, module.PMXT_STORAGE_STATUS, lambda value: value.update({"bte_022_can_close": True}))
        result = subprocess.run(
            [sys.executable, str(SCRIPT), "--root", str(root)],
            text=True,
            stdout=subprocess.PIPE,
            stderr=subprocess.PIPE,
            check=False,
        )
        assert result.returncode == 1
        assert "FINDING:" in result.stderr
        assert "bte_022_can_close" in result.stderr


def main() -> int:
    tests = [
        test_complete_fixture_passes,
        test_s3_head_present_is_a_finding,
        test_missing_bte_storage_reference_is_a_finding,
        test_staged_source_proof_fixture_present_is_a_finding,
        test_missing_justfile_command_is_a_finding,
        test_malformed_justfile_recipe_header_is_a_finding,
        test_committed_hash_drift_is_a_finding,
        test_cli_fails_with_actionable_output,
    ]
    for test in tests:
        test()
    print("verify_bte_022_pmxt_storage_proof self-tests passed.")
    return 0


if __name__ == "__main__":
    import lane_governor

    lane_governor.acquire()
    raise SystemExit(main())
