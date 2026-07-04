#!/usr/bin/env python3
"""Verify BTE-022 PMXT broad-backfill efficiency stays blocked."""

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
PMXT_BROAD_STATUS = REFERENCE_ROOT / "source-proof-pmxt-broad-backfill-efficiency-status.2026-06-09.json"
PMXT_COVERAGE_STATUS = REFERENCE_ROOT / "source-proof-pmxt-coverage-ledger-status.2026-06-09.json"
PMXT_DURABLE_STATUS = REFERENCE_ROOT / "source-proof-pmxt-durable-source-selection-status.2026-06-16.json"
PMXT_DYNAMIC_STATUS = REFERENCE_ROOT / "source-proof-pmxt-dynamic-tick-size-replay-status.2026-06-16.json"
JUSTFILE = Path("justfile")

JUSTFILE_COMMANDS = (
    "python3 scripts/test_verify_bte_022_pmxt_broad_backfill_efficiency.py",
    "python3 scripts/verify_bte_022_pmxt_broad_backfill_efficiency.py",
)
SOURCE_FENCE_STATIC_COMMANDS = ("python3 scripts/run_fences.py",)
JUSTFILE_RECIPES = ("verify-bte-022-pmxt-broad-backfill-efficiency",)

STATUS_INPUTS = (
    ("coverage_ledger_status", PMXT_COVERAGE_STATUS),
    ("durable_source_selection_status", PMXT_DURABLE_STATUS),
    ("dynamic_tick_size_replay_status", PMXT_DYNAMIC_STATUS),
)
BROAD_STATUS_KEYS = (
    "bte_022_can_close",
    "claim_limits",
    "committed_input_hashes",
    "created_date",
    "decision",
    "external_source_evidence",
    "guard_verification",
    "nt_surface_evidence",
    "repo_code_evidence",
    "required_before_any_broad_backfill",
    "root_cause",
    "schema_version",
    "source_binding",
    "status",
    "table_family",
    "verified_at_utc",
)
HASH_ENTRY_KEYS = ("path", "sha256")
GUARD_KEYS = ("script", "self_test", "source_fence_static")
ROOT_CAUSE_KEYS = (
    "current_bounded_improvement",
    "previous_slow_backfill_failure_mode",
    "remaining_efficiency_gap",
    "why_repeating_it_is_unsafe",
)
EXTERNAL_SOURCE_KEYS = ("pmxt_archive", "polymarket_official_api")
PMXT_ARCHIVE_KEYS = (
    "canonical_object_head_probe",
    "current_index_sample",
    "documented_event_types",
    "documented_fast_predicates",
    "documented_row_group_rows",
    "documented_sort_order",
    "docs_uri",
    "index_uri",
    "license",
    "observed_2026_06_09",
)
HEAD_PROBE_KEYS = (
    "accept_ranges",
    "command",
    "content_length_bytes",
    "finding",
    "last_modified",
    "observed_utc",
    "status",
)
REPO_CODE_SECTIONS = (
    "bounded_row_group_projection",
    "bounded_full_object_work",
    "no_download_coverage_gate",
    "accepted_tranche_budget_gate",
    "object_selection_metadata_gate",
    "source_usage_scope_pre_payload_gate",
)
REQUIRED_BEFORE_BROAD_BACKFILL = (
    "Accept a durable Polymarket SourceProofReport for the exact source/window/table_family.",
    "Keep the first gate manifest/index-only: coverage, freshness, completeness, bytes, object count, and cost must be recorded before payload download.",
    "Replace broad full-object hashing in planning/projection with accepted object hashes from staging or Artifact Index records; full payload reads belong only inside bounded accepted tranches.",
    "Carry object-level row-group or predicate metadata from the accepted source proof into the execution plan so selected-source projection does not rediscover it by rescanning payloads; current code carries source_row_groups/predicate_ref and can require them, but accepted PMXT source-proof manifests with that metadata remain unproven.",
    "Carry source_usage_scope from accepted source proof through source-proof scope, accepted tranche, materialized run-spec, and execution plan; current code does this and blocks run-spec mismatches, but PMXT still lacks accepted canonical source proof.",
    "Reject source-only venue-scale acceptance when an explicit source proof set has zero accepted proofs; current code enforces this guardrail, but PMXT full-current still has source_accepted_proof_count=0 and remains blocked.",
    "Set explicit object byte, row count, projected row-group count, and wall-time budgets in the accepted tranche plan; backfill_execution_plan now carries row, projected-row-group, and wall-time budgets, while first_proof_event_count_ledger and selected_source_slice enforce source parquet byte budgets before payload-scale work.",
    "Prove dynamic tick-size replay through NT BacktestNode/catalog, or accept a source-proof-bound no-tick-size-change exclusion policy before full L2 replay claims; generic SourceProofReport L2Replay acceptance now requires l2_replay_evidence.no_tick_size_change_universe_ref or l2_replay_evidence.timed_instrument_epoch_replay_ref.",
    "Use GitHub CI as the broad verification gate for committed changes; avoid broad local cargo test runs as the default verification path.",
)
CLAIM_LIMITS = (
    "This artifact does not accept PMXT as a durable source.",
    "This artifact does not authorize broad PMXT/Polymarket backfill.",
    "This artifact does not prove expanded coverage, exact broad cost, or production throughput.",
    "This artifact does not prove dynamic tick-size replay.",
)
BTE_REMAINING_BLOCKERS = (
    "expanded_tranche_coverage_and_cost_unproven",
    "dynamic_tick_size_replay_unproven",
    "durable_source_selection_unproven",
    "broad_backfill_efficiency_unproven",
)


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
    elif isinstance(actual, dict):
        haystack = json.dumps(actual, sort_keys=True)
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


def check_committed_hashes(status: dict[str, Any], root: Path, findings: list[str]) -> None:
    hashes = nested_mapping(status, ("committed_input_hashes",), PMXT_BROAD_STATUS, findings)
    require_keys(PMXT_BROAD_STATUS, "committed_input_hashes", hashes, tuple(key for key, _ in STATUS_INPUTS), findings)
    for key, rel_path in STATUS_INPUTS:
        entry = nested_mapping(status, ("committed_input_hashes", key), PMXT_BROAD_STATUS, findings)
        require_keys(PMXT_BROAD_STATUS, f"committed_input_hashes.{key}", entry, HASH_ENTRY_KEYS, findings)
        require_equal(PMXT_BROAD_STATUS, f"committed_input_hashes.{key}.path", entry.get("path"), repo_uri(rel_path), findings)
        require_equal(
            PMXT_BROAD_STATUS,
            f"committed_input_hashes.{key}.sha256",
            entry.get("sha256"),
            file_sha256(root, rel_path, findings),
            findings,
        )


def check_broad_status(status: dict[str, Any], root: Path, findings: list[str]) -> None:
    require_keys(PMXT_BROAD_STATUS, "top-level", status, BROAD_STATUS_KEYS, findings)
    require_equal(PMXT_BROAD_STATUS, "schema_version", status.get("schema_version"), "source-proof-pmxt-broad-backfill-efficiency-status.v1", findings)
    require_equal(PMXT_BROAD_STATUS, "source_binding", status.get("source_binding"), "polymarket-parquet-archive-index", findings)
    require_equal(PMXT_BROAD_STATUS, "table_family", status.get("table_family"), "order_book_snapshot_deltas", findings)
    require_equal(
        PMXT_BROAD_STATUS,
        "status",
        status.get("status"),
        "open_broad_backfill_efficiency_unproven_bounded_one_off_guardrails_present",
        findings,
    )
    require_equal(PMXT_BROAD_STATUS, "bte_022_can_close", status.get("bte_022_can_close"), False, findings)
    require_contains(PMXT_BROAD_STATUS, "decision", status.get("decision"), "Do not start broad PMXT/Polymarket L2 backfill", findings)
    require_contains(PMXT_BROAD_STATUS, "decision", status.get("decision"), "lacks accepted source proof", findings)
    require_list_equal(PMXT_BROAD_STATUS, "required_before_any_broad_backfill", status.get("required_before_any_broad_backfill"), REQUIRED_BEFORE_BROAD_BACKFILL, findings)
    require_list_equal(PMXT_BROAD_STATUS, "claim_limits", status.get("claim_limits"), CLAIM_LIMITS, findings)
    check_committed_hashes(status, root, findings)

    guard = nested_mapping(status, ("guard_verification",), PMXT_BROAD_STATUS, findings)
    require_keys(PMXT_BROAD_STATUS, "guard_verification", guard, GUARD_KEYS, findings)
    require_equal(PMXT_BROAD_STATUS, "guard_verification.script", guard.get("script"), "repo://scripts/verify_bte_022_pmxt_broad_backfill_efficiency.py", findings)
    require_equal(PMXT_BROAD_STATUS, "guard_verification.self_test", guard.get("self_test"), "repo://scripts/test_verify_bte_022_pmxt_broad_backfill_efficiency.py", findings)
    require_equal(PMXT_BROAD_STATUS, "guard_verification.source_fence_static", guard.get("source_fence_static"), True, findings)

    root_cause = nested_mapping(status, ("root_cause",), PMXT_BROAD_STATUS, findings)
    require_keys(PMXT_BROAD_STATUS, "root_cause", root_cause, ROOT_CAUSE_KEYS, findings)
    require_contains(PMXT_BROAD_STATUS, "root_cause.previous_slow_backfill_failure_mode", root_cause.get("previous_slow_backfill_failure_mode"), "payloads to scan", findings)
    require_contains(PMXT_BROAD_STATUS, "root_cause.why_repeating_it_is_unsafe", root_cause.get("why_repeating_it_is_unsafe"), "accepted source proof is still pending", findings)
    require_contains(PMXT_BROAD_STATUS, "root_cause.current_bounded_improvement", root_cause.get("current_bounded_improvement"), "source_row_groups", findings)
    require_contains(PMXT_BROAD_STATUS, "root_cause.current_bounded_improvement", root_cause.get("current_bounded_improvement"), "max_source_parquet_bytes", findings)
    require_contains(PMXT_BROAD_STATUS, "root_cause.current_bounded_improvement", root_cause.get("current_bounded_improvement"), "max_projected_row_groups", findings)
    require_contains(PMXT_BROAD_STATUS, "root_cause.remaining_efficiency_gap", root_cause.get("remaining_efficiency_gap"), "manifest/index-owned coverage", findings)

    external = nested_mapping(status, ("external_source_evidence",), PMXT_BROAD_STATUS, findings)
    require_keys(PMXT_BROAD_STATUS, "external_source_evidence", external, EXTERNAL_SOURCE_KEYS, findings)
    pmxt = nested_mapping(status, ("external_source_evidence", "pmxt_archive"), PMXT_BROAD_STATUS, findings)
    require_keys(PMXT_BROAD_STATUS, "external_source_evidence.pmxt_archive", pmxt, PMXT_ARCHIVE_KEYS, findings)
    require_equal(PMXT_BROAD_STATUS, "external_source_evidence.pmxt_archive.documented_row_group_rows", pmxt.get("documented_row_group_rows"), 1_048_576, findings)
    require_contains(PMXT_BROAD_STATUS, "external_source_evidence.pmxt_archive.license", pmxt.get("license"), "CC BY 4.0", findings)
    head = nested_mapping(status, ("external_source_evidence", "pmxt_archive", "canonical_object_head_probe"), PMXT_BROAD_STATUS, findings)
    require_keys(PMXT_BROAD_STATUS, "external_source_evidence.pmxt_archive.canonical_object_head_probe", head, HEAD_PROBE_KEYS, findings)
    require_equal(PMXT_BROAD_STATUS, "external_source_evidence.pmxt_archive.canonical_object_head_probe.status", head.get("status"), 200, findings)
    require_equal(PMXT_BROAD_STATUS, "external_source_evidence.pmxt_archive.canonical_object_head_probe.content_length_bytes", head.get("content_length_bytes"), 361_365_244, findings)
    require_equal(PMXT_BROAD_STATUS, "external_source_evidence.pmxt_archive.canonical_object_head_probe.accept_ranges", head.get("accept_ranges"), "bytes", findings)
    require_contains(PMXT_BROAD_STATUS, "external_source_evidence.pmxt_archive.canonical_object_head_probe.finding", head.get("finding"), "before payload work", findings)

    repo_code = nested_mapping(status, ("repo_code_evidence",), PMXT_BROAD_STATUS, findings)
    require_keys(PMXT_BROAD_STATUS, "repo_code_evidence", repo_code, REPO_CODE_SECTIONS, findings)
    for section in REPO_CODE_SECTIONS:
        entries = repo_code.get(section)
        if not isinstance(entries, list) or not entries:
            findings.append(f"{PMXT_BROAD_STATUS}: repo_code_evidence.{section} must be a non-empty list")
            continue
        for index, entry in enumerate(entries):
            if not isinstance(entry, dict):
                findings.append(f"{PMXT_BROAD_STATUS}: repo_code_evidence.{section}[{index}] must be an object")
                continue
            require_keys(PMXT_BROAD_STATUS, f"repo_code_evidence.{section}[{index}]", entry, ("evidence", "path"), findings)
    require_contains(PMXT_BROAD_STATUS, "repo_code_evidence.object_selection_metadata_gate", repo_code.get("object_selection_metadata_gate"), "predicate_ref", findings)
    require_contains(PMXT_BROAD_STATUS, "repo_code_evidence.source_usage_scope_pre_payload_gate", repo_code.get("source_usage_scope_pre_payload_gate"), "source_usage_scope", findings)
    require_contains(PMXT_BROAD_STATUS, "repo_code_evidence.no_download_coverage_gate", repo_code.get("no_download_coverage_gate"), repo_uri(PMXT_COVERAGE_STATUS), findings)


def check_dependency_statuses(
    coverage: dict[str, Any],
    durable: dict[str, Any],
    dynamic: dict[str, Any],
    findings: list[str],
) -> None:
    require_equal(PMXT_COVERAGE_STATUS, "status", coverage.get("status"), "rejected_under_pending_source_proof", findings)
    coverage_scope = nested_mapping(coverage, ("scope",), PMXT_COVERAGE_STATUS, findings)
    require_equal(PMXT_COVERAGE_STATUS, "scope.broad_backfill_allowed", coverage_scope.get("broad_backfill_allowed"), False, findings)
    require_equal(PMXT_COVERAGE_STATUS, "scope.canonical_ready", coverage_scope.get("canonical_ready"), False, findings)
    coverage_summary = nested_mapping(coverage, ("coverage_summary",), PMXT_COVERAGE_STATUS, findings)
    require_equal(PMXT_COVERAGE_STATUS, "coverage_summary.accepted_records", coverage_summary.get("accepted_records"), 0, findings)
    require_equal(PMXT_COVERAGE_STATUS, "coverage_summary.canonical_ready_records", coverage_summary.get("canonical_ready_records"), 0, findings)
    require_equal(PMXT_COVERAGE_STATUS, "coverage_summary.accepted_objects", coverage_summary.get("accepted_objects"), 0, findings)
    require_equal(PMXT_COVERAGE_STATUS, "coverage_summary.accepted_bytes", coverage_summary.get("accepted_bytes"), 0, findings)

    require_equal(PMXT_DURABLE_STATUS, "durable_source_selection_status", durable.get("durable_source_selection_status"), "blocked_pending_source_proof", findings)
    require_equal(PMXT_DURABLE_STATUS, "source_proof_count", durable.get("source_proof_count"), 1, findings)
    require_equal(PMXT_DURABLE_STATUS, "source_accepted_proof_count", durable.get("source_accepted_proof_count"), 0, findings)
    require_equal(PMXT_DURABLE_STATUS, "bte_022_can_close", durable.get("bte_022_can_close"), False, findings)
    proof_set = nested_mapping(durable, ("source_proof_set_spec",), PMXT_DURABLE_STATUS, findings)
    require_equal(PMXT_DURABLE_STATUS, "source_proof_set_spec.status", proof_set.get("status"), "pending", findings)
    require_equal(PMXT_DURABLE_STATUS, "source_proof_set_spec.usage_scope", proof_set.get("usage_scope"), "one_off_backfill_data", findings)
    manifest_scope = nested_mapping(durable, ("manifest_scope",), PMXT_DURABLE_STATUS, findings)
    require_equal(PMXT_DURABLE_STATUS, "manifest_scope.source_accepted_proof_count", manifest_scope.get("source_accepted_proof_count"), 0, findings)

    require_equal(PMXT_DYNAMIC_STATUS, "dynamic_tick_size_replay_status", dynamic.get("dynamic_tick_size_replay_status"), "blocked_standard_backtestnode_no_timed_instrument_any", findings)
    require_equal(PMXT_DYNAMIC_STATUS, "standard_backtestnode_catalog_replay_supports_dynamic_instrument_any", dynamic.get("standard_backtestnode_catalog_replay_supports_dynamic_instrument_any"), False, findings)
    require_equal(PMXT_DYNAMIC_STATUS, "timed_instrument_epoch_replay_accepted", dynamic.get("timed_instrument_epoch_replay_accepted"), False, findings)
    require_equal(PMXT_DYNAMIC_STATUS, "bounded_no_tick_size_change_first_proof_allowed", dynamic.get("bounded_no_tick_size_change_first_proof_allowed"), True, findings)
    require_equal(PMXT_DYNAMIC_STATUS, "pmxt_full_l2_with_tick_size_change_can_be_accepted_now", dynamic.get("pmxt_full_l2_with_tick_size_change_can_be_accepted_now"), False, findings)
    require_equal(PMXT_DYNAMIC_STATUS, "bte_022_can_close", dynamic.get("bte_022_can_close"), False, findings)


def check_bte_status(bte_status: dict[str, Any], findings: list[str]) -> None:
    require_equal(BTE_022_STATUS, "task_id", bte_status.get("task_id"), "BACKTESTING_ENGINE-022", findings)
    require_equal(BTE_022_STATUS, "status", bte_status.get("status"), "open_pmxt_one_off_current_artifact_proven_broad_backfill_blocked", findings)
    require_equal(BTE_022_STATUS, "bte_022_can_close", bte_status.get("bte_022_can_close"), False, findings)
    blockers = bte_status.get("remaining_blockers")
    if not isinstance(blockers, list):
        findings.append(f"{BTE_022_STATUS}: remaining_blockers must be a list")
        return
    blocker_names = tuple(item.get("blocker") for item in blockers if isinstance(item, dict))
    require_equal(BTE_022_STATUS, "remaining_blockers.blocker_names", blocker_names, BTE_REMAINING_BLOCKERS, findings)
    broad_blocker = next((item for item in blockers if isinstance(item, dict) and item.get("blocker") == "broad_backfill_efficiency_unproven"), None)
    if not isinstance(broad_blocker, dict):
        findings.append(f"{BTE_022_STATUS}: remaining_blockers must include broad_backfill_efficiency_unproven")
        return
    required_evidence = broad_blocker.get("required_evidence")
    require_contains(BTE_022_STATUS, "remaining_blockers.broad_backfill_efficiency_unproven.required_evidence", required_evidence, repo_uri(PMXT_BROAD_STATUS), findings)
    require_contains(BTE_022_STATUS, "remaining_blockers.broad_backfill_efficiency_unproven.required_evidence", required_evidence, "Broad payload work still must not start", findings)
    require_contains(BTE_022_STATUS, "remaining_blockers.broad_backfill_efficiency_unproven.required_evidence", required_evidence, "source_row_groups/predicate_ref", findings)
    require_contains(BTE_022_STATUS, "remaining_blockers.broad_backfill_efficiency_unproven.required_evidence", required_evidence, "STATIC-GATED scripts/verify_bte_022_pmxt_broad_backfill_efficiency.py", findings)
    require_contains(BTE_022_STATUS, "remaining_blockers.broad_backfill_efficiency_unproven.required_evidence", required_evidence, "dependency status hashes", findings)
    require_contains(BTE_022_STATUS, "remaining_blockers.broad_backfill_efficiency_unproven.required_evidence", required_evidence, "source-fence wiring gaps", findings)


def check_justfile(justfile_text: str, findings: list[str]) -> None:
    for recipe in JUSTFILE_RECIPES:
        commands = just_recipe_commands(justfile_text, recipe)
        if not commands:
            findings.append(f"{JUSTFILE}: missing recipe {recipe}")
            continue
        for command in JUSTFILE_COMMANDS:
            if command not in commands:
                findings.append(f"{JUSTFILE}: {recipe} must run {command}")
    source_fence_commands = just_recipe_commands(justfile_text, "source-fence-static-inner")
    if not source_fence_commands:
        findings.append(f"{JUSTFILE}: missing recipe source-fence-static-inner")
        return
    for command in SOURCE_FENCE_STATIC_COMMANDS:
        if command not in source_fence_commands:
            findings.append(f"{JUSTFILE}: source-fence-static-inner must run {command}")


def scan_root(root: Path) -> list[str]:
    root = root.resolve()
    findings: list[str] = []
    broad_status = read_json(root, PMXT_BROAD_STATUS, findings)
    coverage_status = read_json(root, PMXT_COVERAGE_STATUS, findings)
    durable_status = read_json(root, PMXT_DURABLE_STATUS, findings)
    dynamic_status = read_json(root, PMXT_DYNAMIC_STATUS, findings)
    bte_status = read_json(root, BTE_022_STATUS, findings)
    justfile_text = read_text(root, JUSTFILE, findings)

    check_broad_status(broad_status, root, findings)
    check_dependency_statuses(coverage_status, durable_status, dynamic_status, findings)
    check_bte_status(bte_status, findings)
    check_justfile(justfile_text, findings)
    return findings


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--root", type=Path, default=REPO_ROOT, help="repository root to scan")
    args = parser.parse_args(argv)

    findings = scan_root(args.root)
    if findings:
        print("FAIL: BTE-022 PMXT broad-backfill efficiency status violations:", file=sys.stderr)
        for finding in findings:
            print(f"  {finding}", file=sys.stderr)
        return 1
    print("OK: BTE-022 PMXT broad-backfill efficiency status passed.")
    return 0


if __name__ == "__main__":
    import lane_governor

    lane_governor.acquire()
    raise SystemExit(main())
