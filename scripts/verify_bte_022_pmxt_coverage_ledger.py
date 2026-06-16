#!/usr/bin/env python3
"""Verify BTE-022 PMXT coverage-ledger status stays fail-closed."""

from __future__ import annotations

import argparse
import hashlib
import json
import sys
from pathlib import Path
from typing import Any


REPO_ROOT = Path(__file__).resolve().parent.parent
REFERENCE_ROOT = Path("specs/023-nt-research-analytics-platform/reference")
BTE_022_STATUS = REFERENCE_ROOT / "source-proof-nt-catalog-mapping-status.backtesting-engine-022.2026-06-08.json"
PMXT_COVERAGE_STATUS = REFERENCE_ROOT / "source-proof-pmxt-coverage-ledger-status.2026-06-09.json"
PMXT_SOURCE_PROOF_FIXTURE = (
    REFERENCE_ROOT / "source-proof-fixture.binary-option.polymarket-pmxt-official-free-pending.v1.json"
)
JUSTFILE = Path("justfile")

JUSTFILE_COMMANDS = (
    "python3 scripts/test_verify_bte_022_pmxt_coverage_ledger.py",
    "python3 scripts/verify_bte_022_pmxt_coverage_ledger.py",
)
JUSTFILE_RECIPES = ("verify-bte-022-pmxt-coverage-ledger", "source-fence-static")

COVERAGE_STATUS_KEYS = (
    "claim_limits",
    "committed_input_hashes",
    "coverage_summary",
    "guard_verification",
    "ledger_run",
    "next_required_evidence",
    "recorded_at_utc",
    "records",
    "schema_version",
    "scope",
    "source_proof",
    "status",
    "task_id",
)
COVERAGE_SCOPE_KEYS = (
    "answer",
    "broad_backfill_allowed",
    "canonical_ready",
    "question",
    "usage_scope",
)
COVERAGE_SOURCE_PROOF_KEYS = (
    "source_binding",
    "source_proof_id",
    "source_proof_path",
    "source_proof_status",
    "source_proof_version",
    "table_family",
)
COVERAGE_HASHES_KEYS = ("pending_source_fixture",)
COVERAGE_HASH_ENTRY_KEYS = ("path", "sha256")
COVERAGE_LEDGER_RUN_KEYS = (
    "command",
    "ledger_file_sha256",
    "ledger_path",
    "payload_downloaded",
    "raw_manifest_mutated",
    "spec_file_sha256",
    "spec_path",
)
COVERAGE_SUMMARY_KEYS = (
    "accepted_bytes",
    "accepted_objects",
    "accepted_records",
    "blocking_issue_count",
    "blocking_issues",
    "canonical_ready_records",
    "coverage_axis",
    "physical_only_bytes",
    "physical_only_objects",
    "rejected_records",
    "total_records",
)
COVERAGE_RECORD_KEYS = (
    "blocking_issues",
    "canonical_ready",
    "record_id",
    "status",
)
COVERAGE_GUARD_KEYS = ("script", "self_test", "source_fence_static")
COVERAGE_CLAIM_LIMITS = (
    "This artifact does not accept PMXT as a durable source.",
    "This artifact does not authorize canonical-ready PMXT backfill.",
    "This artifact does not prove one-year or expanded coverage.",
    "This artifact does not prove dynamic tick-size replay or an accepted bounded-exclusion policy.",
)
COVERAGE_NEXT_REQUIRED_EVIDENCE = (
    "Accept a durable Polymarket SourceProofReport before any coverage ledger can become canonical-ready.",
    "Record expanded coverage, retention/freshness, completeness, storage, and exact accepted-window cost evidence under that accepted source proof.",
    "Prove dynamic tick-size replay or accept a source-proof-bound no-tick-size-change exclusion policy before broad L2 replay claims.",
)
PENDING_FIXTURE_REQUIRED_CHECKS = (
    "coverage",
    "retention_freshness",
    "completeness",
    "cost",
    "storage",
)
BTE_REMAINING_BLOCKERS = (
    "expanded_tranche_coverage_and_cost_unproven",
    "dynamic_tick_size_replay_unproven",
    "durable_source_selection_unproven",
    "broad_backfill_efficiency_unproven",
)
BTE_STATUS_STATUS = "open_pmxt_one_off_current_artifact_proven_broad_backfill_blocked"


def read_text(root: Path, rel_path: Path, findings: list[str]) -> str:
    path = root / rel_path
    if not path.exists():
        findings.append(f"{rel_path}: file is missing")
        return ""
    return path.read_text(encoding="utf-8")


def read_json(root: Path, rel_path: Path, findings: list[str]) -> dict[str, Any]:
    text = read_text(root, rel_path, findings)
    if not text:
        return {}
    try:
        loaded = json.loads(text)
    except json.JSONDecodeError as exc:
        findings.append(f"{rel_path}: invalid JSON: {exc}")
        return {}
    if not isinstance(loaded, dict):
        findings.append(f"{rel_path}: expected top-level JSON object")
        return {}
    return loaded


def file_sha256(root: Path, rel_path: Path, findings: list[str]) -> str:
    path = root / rel_path
    if not path.exists():
        findings.append(f"{rel_path}: cannot hash missing file")
        return ""
    return hashlib.sha256(path.read_bytes()).hexdigest()


def repo_uri(rel_path: Path) -> str:
    return f"repo://{rel_path.as_posix()}"


def require_equal(rel_path: Path, field: str, actual: Any, expected: Any, findings: list[str]) -> None:
    if actual != expected:
        findings.append(f"{rel_path}: {field} must be {expected!r}, got {actual!r}")


def require_keys(rel_path: Path, field: str, actual: Any, expected: tuple[str, ...], findings: list[str]) -> None:
    if not isinstance(actual, dict):
        findings.append(f"{rel_path}: {field} must be an object")
        return
    actual_keys = tuple(sorted(str(key) for key in actual.keys()))
    expected_keys = tuple(sorted(expected))
    if actual_keys != expected_keys:
        findings.append(f"{rel_path}: {field} keys must be {list(expected_keys)!r}, got {list(actual_keys)!r}")


def require_list_equal(rel_path: Path, field: str, actual: Any, expected: tuple[str, ...], findings: list[str]) -> None:
    if actual != list(expected):
        findings.append(f"{rel_path}: {field} must be {list(expected)!r}, got {actual!r}")


def require_contains(rel_path: Path, field: str, actual: Any, expected: str, findings: list[str]) -> None:
    if isinstance(actual, str):
        haystack = actual
    elif isinstance(actual, list):
        haystack = "\n".join(str(item) for item in actual)
    else:
        findings.append(f"{rel_path}: {field} must contain {expected!r}, got non-text {actual!r}")
        return
    if expected not in haystack:
        findings.append(f"{rel_path}: {field} must contain {expected!r}")


def nested_mapping(data: dict[str, Any], keys: tuple[str, ...], rel_path: Path, findings: list[str]) -> dict[str, Any]:
    current: Any = data
    label = ".".join(keys)
    for key in keys:
        if not isinstance(current, dict):
            findings.append(f"{rel_path}: {label} must be an object")
            return {}
        current = current.get(key)
    if not isinstance(current, dict):
        findings.append(f"{rel_path}: {label} must be an object")
        return {}
    return current


def just_recipe_commands(text: str, recipe: str) -> set[str]:
    commands: set[str] = set()
    in_recipe = False
    for raw_line in text.splitlines():
        stripped = raw_line.strip()
        if not in_recipe:
            if raw_line.startswith(f"{recipe}:"):
                in_recipe = True
            continue
        if raw_line and not raw_line.startswith((" ", "\t")):
            break
        if stripped and not stripped.startswith("#"):
            commands.add(stripped)
    return commands


def check_coverage_status(status: dict[str, Any], root: Path, findings: list[str]) -> None:
    require_keys(PMXT_COVERAGE_STATUS, "top-level", status, COVERAGE_STATUS_KEYS, findings)
    require_equal(
        PMXT_COVERAGE_STATUS,
        "schema_version",
        status.get("schema_version"),
        "source-proof-pmxt-coverage-ledger-status.v1",
        findings,
    )
    require_equal(PMXT_COVERAGE_STATUS, "task_id", status.get("task_id"), "BACKTESTING_ENGINE-022", findings)
    require_equal(PMXT_COVERAGE_STATUS, "status", status.get("status"), "rejected_under_pending_source_proof", findings)

    scope = nested_mapping(status, ("scope",), PMXT_COVERAGE_STATUS, findings)
    require_keys(PMXT_COVERAGE_STATUS, "scope", scope, COVERAGE_SCOPE_KEYS, findings)
    require_equal(PMXT_COVERAGE_STATUS, "scope.usage_scope", scope.get("usage_scope"), "one_off_backfill_data", findings)
    require_equal(PMXT_COVERAGE_STATUS, "scope.broad_backfill_allowed", scope.get("broad_backfill_allowed"), False, findings)
    require_equal(PMXT_COVERAGE_STATUS, "scope.canonical_ready", scope.get("canonical_ready"), False, findings)
    require_contains(PMXT_COVERAGE_STATUS, "scope.answer", scope.get("answer"), "both records are rejected", findings)

    source_proof = nested_mapping(status, ("source_proof",), PMXT_COVERAGE_STATUS, findings)
    require_keys(PMXT_COVERAGE_STATUS, "source_proof", source_proof, COVERAGE_SOURCE_PROOF_KEYS, findings)
    require_equal(PMXT_COVERAGE_STATUS, "source_proof.source_proof_path", source_proof.get("source_proof_path"), repo_uri(PMXT_SOURCE_PROOF_FIXTURE), findings)
    require_equal(PMXT_COVERAGE_STATUS, "source_proof.source_proof_id", source_proof.get("source_proof_id"), "source-proof-polymarket-pmxt-v2-orderbook-binary-option-pending-2026-06-08", findings)
    require_equal(PMXT_COVERAGE_STATUS, "source_proof.source_binding", source_proof.get("source_binding"), "polymarket-parquet-archive-index", findings)
    require_equal(PMXT_COVERAGE_STATUS, "source_proof.source_proof_status", source_proof.get("source_proof_status"), "pending", findings)
    require_equal(PMXT_COVERAGE_STATUS, "source_proof.table_family", source_proof.get("table_family"), "order_book_snapshot_deltas", findings)

    hashes = nested_mapping(status, ("committed_input_hashes",), PMXT_COVERAGE_STATUS, findings)
    require_keys(PMXT_COVERAGE_STATUS, "committed_input_hashes", hashes, COVERAGE_HASHES_KEYS, findings)
    pending_fixture = nested_mapping(status, ("committed_input_hashes", "pending_source_fixture"), PMXT_COVERAGE_STATUS, findings)
    require_keys(PMXT_COVERAGE_STATUS, "committed_input_hashes.pending_source_fixture", pending_fixture, COVERAGE_HASH_ENTRY_KEYS, findings)
    require_equal(PMXT_COVERAGE_STATUS, "committed_input_hashes.pending_source_fixture.path", pending_fixture.get("path"), repo_uri(PMXT_SOURCE_PROOF_FIXTURE), findings)
    require_equal(
        PMXT_COVERAGE_STATUS,
        "committed_input_hashes.pending_source_fixture.sha256",
        pending_fixture.get("sha256"),
        file_sha256(root, PMXT_SOURCE_PROOF_FIXTURE, findings),
        findings,
    )

    ledger_run = nested_mapping(status, ("ledger_run",), PMXT_COVERAGE_STATUS, findings)
    require_keys(PMXT_COVERAGE_STATUS, "ledger_run", ledger_run, COVERAGE_LEDGER_RUN_KEYS, findings)
    require_equal(PMXT_COVERAGE_STATUS, "ledger_run.payload_downloaded", ledger_run.get("payload_downloaded"), False, findings)
    require_equal(PMXT_COVERAGE_STATUS, "ledger_run.raw_manifest_mutated", ledger_run.get("raw_manifest_mutated"), False, findings)

    summary = nested_mapping(status, ("coverage_summary",), PMXT_COVERAGE_STATUS, findings)
    require_keys(PMXT_COVERAGE_STATUS, "coverage_summary", summary, COVERAGE_SUMMARY_KEYS, findings)
    require_equal(PMXT_COVERAGE_STATUS, "coverage_summary.coverage_axis", summary.get("coverage_axis"), "timestamp_received", findings)
    require_equal(PMXT_COVERAGE_STATUS, "coverage_summary.total_records", summary.get("total_records"), 2, findings)
    require_equal(PMXT_COVERAGE_STATUS, "coverage_summary.rejected_records", summary.get("rejected_records"), 2, findings)
    require_equal(PMXT_COVERAGE_STATUS, "coverage_summary.accepted_records", summary.get("accepted_records"), 0, findings)
    require_equal(PMXT_COVERAGE_STATUS, "coverage_summary.canonical_ready_records", summary.get("canonical_ready_records"), 0, findings)
    require_equal(PMXT_COVERAGE_STATUS, "coverage_summary.accepted_objects", summary.get("accepted_objects"), 0, findings)
    require_equal(PMXT_COVERAGE_STATUS, "coverage_summary.accepted_bytes", summary.get("accepted_bytes"), 0, findings)
    require_equal(PMXT_COVERAGE_STATUS, "coverage_summary.blocking_issue_count", summary.get("blocking_issue_count"), 2, findings)
    require_list_equal(PMXT_COVERAGE_STATUS, "coverage_summary.blocking_issues", summary.get("blocking_issues"), ("source_proof_not_accepted",), findings)

    records = status.get("records")
    if not isinstance(records, list):
        findings.append(f"{PMXT_COVERAGE_STATUS}: records must be a list")
    else:
        require_equal(PMXT_COVERAGE_STATUS, "records.length", len(records), 2, findings)
        for index, record in enumerate(records):
            label = f"records[{index}]"
            if not isinstance(record, dict):
                findings.append(f"{PMXT_COVERAGE_STATUS}: {label} must be an object")
                continue
            require_keys(PMXT_COVERAGE_STATUS, label, record, COVERAGE_RECORD_KEYS, findings)
            require_equal(PMXT_COVERAGE_STATUS, f"{label}.status", record.get("status"), "rejected", findings)
            require_equal(PMXT_COVERAGE_STATUS, f"{label}.canonical_ready", record.get("canonical_ready"), False, findings)
            require_list_equal(PMXT_COVERAGE_STATUS, f"{label}.blocking_issues", record.get("blocking_issues"), ("source_proof_not_accepted",), findings)

    require_list_equal(PMXT_COVERAGE_STATUS, "claim_limits", status.get("claim_limits"), COVERAGE_CLAIM_LIMITS, findings)
    require_list_equal(PMXT_COVERAGE_STATUS, "next_required_evidence", status.get("next_required_evidence"), COVERAGE_NEXT_REQUIRED_EVIDENCE, findings)

    guard = nested_mapping(status, ("guard_verification",), PMXT_COVERAGE_STATUS, findings)
    require_keys(PMXT_COVERAGE_STATUS, "guard_verification", guard, COVERAGE_GUARD_KEYS, findings)
    require_equal(PMXT_COVERAGE_STATUS, "guard_verification.script", guard.get("script"), "repo://scripts/verify_bte_022_pmxt_coverage_ledger.py", findings)
    require_equal(PMXT_COVERAGE_STATUS, "guard_verification.self_test", guard.get("self_test"), "repo://scripts/test_verify_bte_022_pmxt_coverage_ledger.py", findings)
    require_equal(PMXT_COVERAGE_STATUS, "guard_verification.source_fence_static", guard.get("source_fence_static"), True, findings)


def check_pending_source_fixture(source_fixture: dict[str, Any], findings: list[str]) -> None:
    require_equal(PMXT_SOURCE_PROOF_FIXTURE, "schema_version", source_fixture.get("schema_version"), "backfill-source-proof.v1", findings)
    require_equal(PMXT_SOURCE_PROOF_FIXTURE, "source_proof_id", source_fixture.get("source_proof_id"), "source-proof-polymarket-pmxt-v2-orderbook-binary-option-pending-2026-06-08", findings)
    require_equal(PMXT_SOURCE_PROOF_FIXTURE, "status", source_fixture.get("status"), "pending", findings)
    require_equal(PMXT_SOURCE_PROOF_FIXTURE, "source_selection_status", source_fixture.get("source_selection_status"), "PENDING_MORE_PROOF", findings)
    require_equal(PMXT_SOURCE_PROOF_FIXTURE, "usage_scope", source_fixture.get("usage_scope"), "one_off_backfill_data", findings)
    require_equal(PMXT_SOURCE_PROOF_FIXTURE, "table_family", source_fixture.get("table_family"), "order_book_snapshot_deltas", findings)
    require_equal(PMXT_SOURCE_PROOF_FIXTURE, "source_binding", source_fixture.get("source_binding"), "polymarket-parquet-archive-index", findings)
    required_checks = source_fixture.get("required_checks")
    if not isinstance(required_checks, dict):
        findings.append(f"{PMXT_SOURCE_PROOF_FIXTURE}: required_checks must be an object")
        return
    for check in PENDING_FIXTURE_REQUIRED_CHECKS:
        entry = required_checks.get(check)
        if not isinstance(entry, dict):
            findings.append(f"{PMXT_SOURCE_PROOF_FIXTURE}: required_checks.{check} must be an object")
            continue
        require_equal(PMXT_SOURCE_PROOF_FIXTURE, f"required_checks.{check}.outcome", entry.get("outcome"), "pending", findings)


def check_bte_status(bte_status: dict[str, Any], findings: list[str]) -> None:
    require_equal(BTE_022_STATUS, "task_id", bte_status.get("task_id"), "BACKTESTING_ENGINE-022", findings)
    require_equal(BTE_022_STATUS, "status", bte_status.get("status"), BTE_STATUS_STATUS, findings)
    require_equal(BTE_022_STATUS, "bte_022_can_close", bte_status.get("bte_022_can_close"), False, findings)
    blockers = bte_status.get("remaining_blockers")
    if not isinstance(blockers, list):
        findings.append(f"{BTE_022_STATUS}: remaining_blockers must be a list")
        return
    blocker_names = tuple(item.get("blocker") for item in blockers if isinstance(item, dict))
    require_equal(BTE_022_STATUS, "remaining_blockers.blocker_names", blocker_names, BTE_REMAINING_BLOCKERS, findings)
    coverage_blocker = next(
        (item for item in blockers if isinstance(item, dict) and item.get("blocker") == "expanded_tranche_coverage_and_cost_unproven"),
        None,
    )
    if not isinstance(coverage_blocker, dict):
        findings.append(f"{BTE_022_STATUS}: remaining_blockers must include expanded_tranche_coverage_and_cost_unproven")
        return
    required_evidence = coverage_blocker.get("required_evidence")
    require_contains(BTE_022_STATUS, "remaining_blockers.expanded_tranche_coverage_and_cost_unproven.required_evidence", required_evidence, repo_uri(PMXT_COVERAGE_STATUS), findings)
    require_contains(BTE_022_STATUS, "remaining_blockers.expanded_tranche_coverage_and_cost_unproven.required_evidence", required_evidence, "rejected evidence", findings)
    require_contains(BTE_022_STATUS, "remaining_blockers.expanded_tranche_coverage_and_cost_unproven.required_evidence", required_evidence, "pending source proof", findings)
    require_contains(BTE_022_STATUS, "remaining_blockers.expanded_tranche_coverage_and_cost_unproven.required_evidence", required_evidence, "STATIC-GATED scripts/verify_bte_022_pmxt_coverage_ledger.py", findings)


def check_justfile(justfile_text: str, findings: list[str]) -> None:
    for recipe in JUSTFILE_RECIPES:
        commands = just_recipe_commands(justfile_text, recipe)
        if not commands:
            findings.append(f"{JUSTFILE}: missing recipe {recipe}")
            continue
        for command in JUSTFILE_COMMANDS:
            if command not in commands:
                findings.append(f"{JUSTFILE}: {recipe} must run {command}")


def scan_root(root: Path) -> list[str]:
    root = root.resolve()
    findings: list[str] = []
    coverage_status = read_json(root, PMXT_COVERAGE_STATUS, findings)
    source_fixture = read_json(root, PMXT_SOURCE_PROOF_FIXTURE, findings)
    bte_status = read_json(root, BTE_022_STATUS, findings)
    justfile_text = read_text(root, JUSTFILE, findings)

    check_coverage_status(coverage_status, root, findings)
    check_pending_source_fixture(source_fixture, findings)
    check_bte_status(bte_status, findings)
    check_justfile(justfile_text, findings)
    return findings


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--root", type=Path, default=REPO_ROOT, help="repository root to scan")
    return parser.parse_args()


def main() -> int:
    args = parse_args()
    findings = scan_root(args.root)
    if findings:
        for finding in findings:
            print(f"FAIL: {finding}", file=sys.stderr)
        return 1
    print("OK: BTE-022 PMXT coverage-ledger status passed.")
    return 0


if __name__ == "__main__":
    import lane_governor

    lane_governor.acquire()
    raise SystemExit(main())
