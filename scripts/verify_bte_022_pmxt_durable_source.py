#!/usr/bin/env python3
"""Verify BTE-022 PMXT durable-source status stays fail-closed."""

from __future__ import annotations

import argparse
import fnmatch
import hashlib
import json
import sys
import tomllib
from pathlib import Path


REPO_ROOT = Path(__file__).resolve().parent.parent
REFERENCE_ROOT = Path("specs/023-nt-research-analytics-platform/reference")
PMXT_SOURCE_PROOF_SPEC = REFERENCE_ROOT / "backfill-source-proofs/pmxt-polymarket-v2-current/source-universe-source-proofs.toml"
PMXT_SOURCE_MANIFEST = (
    REFERENCE_ROOT
    / "backfill-source-universe-object-manifests/pmxt-polymarket-v2-current/manifest/source-universe-object-manifest.json"
)
PMXT_CATEGORY_MANIFEST = (
    REFERENCE_ROOT
    / "backfill-source-universe-object-manifests/pmxt-polymarket-v2-current/category-manifests/pmxt-polymarket-v2-object-manifest-orderbook.json"
)
PMXT_ARCHIVE_INDEX_MANIFEST = (
    REFERENCE_ROOT / "source-archive-index-manifests/pmxt-polymarket-v2-current/manifest/source-archive-index-manifest.json"
)
PMXT_CONVERSION_QUEUE_SPEC = (
    REFERENCE_ROOT / "source-universe-conversion-queues/pmxt-polymarket-v2-current/source-universe-conversion-queue.toml"
)
PMXT_SOURCE_PROOF_FIXTURE = (
    REFERENCE_ROOT / "source-proof-fixture.binary-option.polymarket-pmxt-official-free-pending.v1.json"
)
SOURCE_PROOF_ACCEPTANCE_CONTRACT = Path("crates/backtesting-vertical-slice/src/source_proof.rs")
SOURCE_PROOF_ADMISSIBILITY_CONTRACT = Path("crates/backtesting-vertical-slice/src/source_proof_admissibility.rs")
VENUE_LEDGER_SPEC = (
    REFERENCE_ROOT
    / "venue-scale-conversion-acceptance-ledgers/binance-bybit-pmxt-current/venue-scale-conversion-acceptance-ledger.toml"
)
BTE_022_STATUS = REFERENCE_ROOT / "source-proof-nt-catalog-mapping-status.backtesting-engine-022.2026-06-08.json"
PMXT_DURABLE_STATUS = REFERENCE_ROOT / "source-proof-pmxt-durable-source-selection-status.2026-06-16.json"
GITIGNORE = Path(".gitignore")
JUSTFILE = Path("justfile")

SOURCE_PROOF_SPEC_PENDING_CHECKS = {
    "instrument_universe",
    "coverage",
    "retention_freshness",
    "completeness",
    "cost",
    "storage",
}
SOURCE_PROOF_SPEC_PASSED_CHECKS = {
    "source_access",
    "license",
    "schema",
    "time_semantics",
    "granularity",
    "nt_mapping",
}
ONE_OFF_FIXTURE_PENDING_CHECKS = {
    "coverage",
    "retention_freshness",
    "completeness",
    "cost",
    "storage",
}
ONE_OFF_FIXTURE_PASSED_CHECKS = {
    "source_access",
    "license",
    "schema",
    "time_semantics",
    "instrument_universe",
    "granularity",
    "nt_mapping",
}
PMXT_EVICTION_PATTERNS = (
    "specs/023-nt-research-analytics-platform/reference/source-universe-conversion-queues/pmxt-polymarket-v2-current/queue/*.json",
    "specs/023-nt-research-analytics-platform/reference/backfill-source-proofs/pmxt-polymarket-v2-current/*.json",
    "specs/023-nt-research-analytics-platform/reference/venue-scale-conversion-acceptance-ledgers/*/ledger/*.json",
)
PMXT_EVICTION_REPRESENTATIVES = {
    PMXT_EVICTION_PATTERNS[0]: "specs/023-nt-research-analytics-platform/reference/source-universe-conversion-queues/pmxt-polymarket-v2-current/queue/source-universe-conversion-queue.json",
    PMXT_EVICTION_PATTERNS[1]: "specs/023-nt-research-analytics-platform/reference/backfill-source-proofs/pmxt-polymarket-v2-current/source-universe-source-proof-set.json",
    PMXT_EVICTION_PATTERNS[2]: "specs/023-nt-research-analytics-platform/reference/venue-scale-conversion-acceptance-ledgers/binance-bybit-pmxt-current/ledger/venue-scale-conversion-acceptance-ledger.json",
}
JUSTFILE_COMMANDS = (
    "python3 scripts/test_verify_bte_022_pmxt_durable_source.py",
    "python3 scripts/verify_bte_022_pmxt_durable_source.py",
)
SOURCE_FENCE_STATIC_COMMANDS = ("python3 scripts/run_fences.py",)
JUSTFILE_RECIPES = ("verify-bte-022-pmxt-durable-source",)
STATUS_HASH_TARGETS = (
    (("source_proof_set_spec",), PMXT_SOURCE_PROOF_SPEC),
    (("committed_input_hashes", "source_universe_manifest"), PMXT_SOURCE_MANIFEST),
    (("committed_input_hashes", "category_manifest"), PMXT_CATEGORY_MANIFEST),
    (("committed_input_hashes", "archive_index_manifest"), PMXT_ARCHIVE_INDEX_MANIFEST),
    (("committed_input_hashes", "conversion_queue_spec"), PMXT_CONVERSION_QUEUE_SPEC),
    (("committed_input_hashes", "venue_acceptance_ledger_spec"), VENUE_LEDGER_SPEC),
    (("committed_input_hashes", "pending_source_fixture"), PMXT_SOURCE_PROOF_FIXTURE),
    (("source_proof_admissibility_status", "proof_fixture"), PMXT_SOURCE_PROOF_FIXTURE),
    (("source_proof_admissibility_status", "acceptance_contract"), SOURCE_PROOF_ACCEPTANCE_CONTRACT),
    (("source_proof_admissibility_status", "admissibility_contract"), SOURCE_PROOF_ADMISSIBILITY_CONTRACT),
)
PMXT_SOURCE_PROOF_SPEC_KEYS = (
    "claim_limit",
    "cost_ref",
    "coverage_end_utc",
    "coverage_start_utc",
    "fidelity_class",
    "gap_policy_id",
    "l2_replay_evidence",
    "license_ref",
    "license_scope",
    "manifest_table_family",
    "output_dir",
    "proof_set_id",
    "raw_sample_selection",
    "requested_end_utc",
    "requested_start_utc",
    "required_checks",
    "retention_ref",
    "schema_sample_policy",
    "source_binding",
    "source_bindings_path",
    "source_candidate_class",
    "source_selection_status",
    "status",
    "table_family",
    "usage_scope",
    "venue",
)
PMXT_SOURCE_PROOF_SPEC_L2_EVIDENCE_KEYS = (
    "order_book_delta_ref",
    "sufficient_snapshot_cadence_ref",
)
PMXT_SOURCE_PROOF_SPEC_BINDING_KEYS = (
    "category_manifest_path",
    "instrument_universe_id",
    "product_category",
    "source_binding",
    "source_proof_id",
)
REQUIRED_CHECK_ENTRY_KEYS = ("evidence_ref", "outcome")
DURABLE_STATUS_KEYS = (
    "bte_022_can_close",
    "claim_limits",
    "committed_input_hashes",
    "durable_source_selection_status",
    "generated_artifact_policy",
    "guard_verification",
    "manifest_scope",
    "observed_at_utc",
    "passed_required_checks",
    "pending_required_checks",
    "remaining_blockers",
    "schema_version",
    "source_accepted_proof_count",
    "source_binding",
    "source_proof_admissibility_status",
    "source_proof_count",
    "source_proof_set_spec",
    "task_id",
)
DURABLE_STATUS_SOURCE_PROOF_SPEC_KEYS = (
    "fidelity_class",
    "path",
    "sha256",
    "source_selection_status",
    "status",
    "usage_scope",
)
DURABLE_STATUS_COMMITTED_INPUT_HASHES_KEYS = (
    "archive_index_manifest",
    "category_manifest",
    "conversion_queue_spec",
    "pending_source_fixture",
    "source_universe_manifest",
    "venue_acceptance_ledger_spec",
)
DURABLE_STATUS_SOURCE_PROOF_ADMISSIBILITY_KEYS = (
    "acceptance_contract",
    "acceptance_error",
    "admissibility_contract",
    "blocking_issues",
    "current_contract_deserializes",
    "expected_record_status",
    "missing_current_contract_fields",
    "proof_fixture",
    "proof_uri",
    "source_binding",
    "source_proof_id",
    "source_selection_status",
    "status",
    "usage_scope",
)
DURABLE_STATUS_HASH_ENTRY_KEYS = ("path", "sha256")
DURABLE_STATUS_MANIFEST_SCOPE_KEYS = (
    "accepted_bytes",
    "object_count",
    "source_accepted_proof_count",
    "verified_head_count",
)
DURABLE_STATUS_GENERATED_ARTIFACT_POLICY_KEYS = (
    "gitignore_refs",
    "reason",
    "status",
)
DURABLE_STATUS_GUARD_VERIFICATION_KEYS = (
    "script",
    "self_test",
    "source_fence_static",
)
DURABLE_STATUS_CLAIM_LIMITS = (
    "This records a durable-source guardrail, not an accepted durable PMXT source.",
    "The PMXT full-current universe must remain blocked while source_accepted_proof_count is zero.",
    "Generated queue/proof/ledger bulk JSON remains evicted; the committed TOML/status/verifier chain is the reviewable guard.",
    "This does not prove expanded coverage, object gates, conversion run plans, broad backfill efficiency, or dynamic tick-size replay.",
)
DURABLE_STATUS_REMAINING_BLOCKERS = (
    "durable_source_selection_unproven",
    "expanded_tranche_coverage_and_cost_unproven",
    "dynamic_tick_size_replay_unproven",
    "broad_backfill_efficiency_unproven",
)
BTE_REMAINING_BLOCKERS = (
    "expanded_tranche_coverage_and_cost_unproven",
    "dynamic_tick_size_replay_unproven",
    "durable_source_selection_unproven",
    "broad_backfill_efficiency_unproven",
)
BTE_DURABLE_GUARD_STATUS = "code_guardrail_added_actual_pmxt_accepted_source_proof_unproven"
BTE_DURABLE_GUARD_EVIDENCE = (
    "RED-GATED crates/backtesting-vertical-slice/src/venue_scale_conversion_acceptance.rs unit regression source_only_status_rejects_unaccepted_source_proof_set documents that a source-only universe with a referenced source proof set but zero accepted proofs must fail validation.",
    "GREEN-GATED venue-scale conversion acceptance validation now receives source_proof_count and source_accepted_proof_count and rejects SourceOnly when source_proof_count > 0 and source_accepted_proof_count == 0.",
    "REGRESSION crates/backtesting-vertical-slice/tests/backtesting_vertical_slice_venue_scale_conversion_acceptance.rs source_proof_set_rejects_accepted_count_above_total_count documents that source proof sets with accepted_proof_count > proof_count must fail before status accounting.",
    "repo://specs/023-nt-research-analytics-platform/reference/venue-scale-conversion-acceptance-ledgers/binance-bybit-pmxt-current/venue-scale-conversion-acceptance-ledger.toml explicitly lists missing_accepted_source_proof on pmxt-polymarket-full-current-data while source_accepted_proof_count remains 0.",
    "repo://specs/023-nt-research-analytics-platform/reference/venue-scale-conversion-acceptance-ledgers/binance-bybit-pmxt-current/venue-scale-conversion-acceptance-ledger.toml keeps pmxt-polymarket-full-current-data blocked while repo://specs/023-nt-research-analytics-platform/reference/source-proof-pmxt-durable-source-selection-status.2026-06-16.json pins source_accepted_proof_count=0.",
    "repo://specs/023-nt-research-analytics-platform/reference/source-proof-pmxt-durable-source-selection-status.2026-06-16.json records the source-controlled PMXT durable-source guard: one pending proof in the committed TOML spec, zero accepted proofs, generated bulk JSON evicted by policy, and source-fence static coverage via scripts/verify_bte_022_pmxt_durable_source.py.",
    "repo://specs/023-nt-research-analytics-platform/reference/source-proof-pmxt-durable-source-selection-status.2026-06-16.json source-fences the PMXT pending fixture against crates/backtesting-vertical-slice/src/source_proof_admissibility.rs and crates/backtesting-vertical-slice/src/source_proof.rs as current_contract_rejected with current contract fields present, acceptance_failed because raw_sample_uri must be staged to s3:// before canonical acceptance, and explicit one_off_backfill_data usage that cannot be promoted to canonical source proof input.",
    "STATIC-GATED scripts/verify_bte_022_pmxt_durable_source.py rejects drift from pending/one_off_backfill_data PMXT source proof state, missing pmxt-polymarket-full-current-data blocking issues, BTE-022 close claims, and source-fence wiring gaps.",
)
BTE_DURABLE_GUARD_CLAIM_LIMITS = (
    "This proves a venue-scale source-only status cannot be backed by an explicitly unaccepted source proof set.",
    "This proves source proof set summary counts fail closed when accepted_proof_count exceeds proof_count.",
    "This proves the PMXT durable-source state has a compact source-fenced guard even though generated PMXT bulk JSON artifacts remain evicted.",
    "This does not accept any PMXT source proof.",
    "This does not prove expanded PMXT coverage, cost, object gates, conversion run plans, or dynamic tick-size replay.",
    "This does not authorize broad PMXT backfill.",
)
BTE_DURABLE_GUARD_KEYS = ("claim_limits", "evidence", "status")
BTE_STATUS_KEYS = (
    "accepted_trade_replay_runtime_recheck",
    "bolt_current_limitations",
    "bounded_first_proof_selector_status",
    "bounded_l2_backtestnode_status",
    "bounded_l2_catalog_hash_status",
    "bounded_l2_manifest_mapping_status",
    "bounded_l2_result_contract_status",
    "broad_backfill_efficiency_object_selection_metadata_status",
    "broad_backfill_source_usage_scope_status",
    "bte_022_can_close",
    "current_reconciliation",
    "decision",
    "durable_source_selection_source_only_guardrail_status",
    "dynamic_tick_size_replay_guardrail_status",
    "_".join(("first", "proof", "policy", "status")),
    "next_required_evidence",
    "non_hardcoding_decision",
    "nt_capability_evidence",
    "old_artifact_recommendation",
    "pmxt_one_off_conversion_metadata_status",
    "recorded_at",
    "remaining_blockers",
    "schema_version",
    "source_mapping_status",
    "status",
    "task_id",
)
BTE_STATUS_STATUS = "open_pmxt_one_off_current_artifact_proven_broad_backfill_blocked"
BTE_STATUS_DECISION = (
    "Do not start broad PMXT backfill. PMXT may proceed only as one-off backfill evidence after the chosen selected-source sample is "
    "converted into NT-native data classes, written to ParquetDataCatalog under the artifact root, queried back, consumed by BacktestNode, "
    "and bound to a result contract."
)
SOURCE_PROOF_ACCEPTANCE_SNIPPETS = (
    'ensure_staged_s3_uri("raw_sample_uri", &self.raw_sample_uri)?',
    'uri.starts_with("s3://")',
    "fn validate_source_selection(proof: &SourceProofReport) -> Result<(), AcceptanceError>",
    "proof.usage_scope == SourceProofUsageScope::OneOffBackfillData",
    "return Err(AcceptanceError::OneOffBackfillDataNotCanonical);",
    "one_off_backfill_data source proofs cannot be accepted as canonical source proof input",
)
SOURCE_PROOF_ADMISSIBILITY_SNIPPETS = (
    '"acceptance_scope",',
    "SourceProofAdmissibilityIssue::MissingCurrentContractField",
    "SourceProofAdmissibilityStatus::CurrentContractRejected",
    "SourceProofAdmissibilityIssue::AcceptanceFailed",
    "acceptance_error: Some(error.to_string())",
)


def read_text(root: Path, rel_path: Path, findings: list[str]) -> str:
    path = root / rel_path
    if not path.exists():
        findings.append(f"{rel_path}: file is missing")
        return ""
    return path.read_text(encoding="utf-8")


def read_json(root: Path, rel_path: Path, findings: list[str]) -> dict:
    text = read_text(root, rel_path, findings)
    if not text:
        return {}
    try:
        value = json.loads(text)
    except json.JSONDecodeError as error:
        findings.append(f"{rel_path}: invalid JSON: {error}")
        return {}
    if not isinstance(value, dict):
        findings.append(f"{rel_path}: expected JSON object")
        return {}
    return value


def read_toml(root: Path, rel_path: Path, findings: list[str]) -> dict:
    text = read_text(root, rel_path, findings)
    if not text:
        return {}
    try:
        value = tomllib.loads(text)
    except tomllib.TOMLDecodeError as error:
        findings.append(f"{rel_path}: invalid TOML: {error}")
        return {}
    if not isinstance(value, dict):
        findings.append(f"{rel_path}: expected TOML object")
        return {}
    return value


def file_sha256(root: Path, rel_path: Path, findings: list[str]) -> str:
    path = root / rel_path
    if not path.exists():
        findings.append(f"{rel_path}: cannot hash missing file")
        return ""
    return hashlib.sha256(path.read_bytes()).hexdigest()


def repo_uri(rel_path: Path) -> str:
    return f"repo://{rel_path.as_posix()}"


def nested_mapping(value: dict, keys: tuple[str, ...], rel_path: Path, findings: list[str]) -> dict:
    current: object = value
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


def require_equal(rel_path: Path, label: str, actual: object, expected: object, findings: list[str]) -> None:
    if actual != expected:
        findings.append(f"{rel_path}: {label} must be {expected!r}, got {actual!r}")


def require_contains(rel_path: Path, label: str, values: object, expected: str, findings: list[str]) -> None:
    if not isinstance(values, list) or expected not in values:
        findings.append(f"{rel_path}: {label} must include `{expected}`")


def require_text_contains(rel_path: Path, label: str, text: str, expected: str, findings: list[str]) -> None:
    if expected not in text:
        findings.append(f"{rel_path}: {label} must contain `{expected}`")


def require_list_equal(rel_path: Path, label: str, actual: object, expected: tuple[str, ...], findings: list[str]) -> None:
    if not isinstance(actual, list):
        findings.append(f"{rel_path}: {label} must be a list")
        return
    if actual != list(expected):
        findings.append(f"{rel_path}: {label} must be {list(expected)!r}, got {actual!r}")


def require_keys_equal(rel_path: Path, label: str, actual: object, expected: tuple[str, ...], findings: list[str]) -> None:
    if not isinstance(actual, dict):
        findings.append(f"{rel_path}: {label} must be an object")
        return
    actual_keys = tuple(sorted(actual))
    expected_keys = tuple(sorted(expected))
    if actual_keys != expected_keys:
        findings.append(f"{rel_path}: {label} keys must be {list(expected_keys)!r}, got {list(actual_keys)!r}")


def check_required_checks(
    rel_path: Path,
    checks: dict,
    passed_checks: set[str],
    pending_checks: set[str],
    findings: list[str],
) -> None:
    if not isinstance(checks, dict):
        findings.append(f"{rel_path}: required_checks must be an object")
        return
    expected_checks = passed_checks | pending_checks
    actual_checks = set(checks)
    if actual_checks != expected_checks:
        findings.append(
            f"{rel_path}: required_checks keys must be {sorted(expected_checks)!r}, got {sorted(actual_checks)!r}"
        )
    for check in sorted(expected_checks):
        entry = checks.get(check)
        if not isinstance(entry, dict):
            findings.append(f"{rel_path}: required_checks.{check} must be an object")
            continue
        require_keys_equal(rel_path, f"required_checks.{check}", entry, REQUIRED_CHECK_ENTRY_KEYS, findings)
        acceptance_keys = acceptance_provenance_keys(entry)
        if acceptance_keys:
            findings.append(f"{rel_path}: required_checks.{check} must not carry acceptance provenance keys {acceptance_keys!r}")
        expected_outcome = "passed" if check in passed_checks else "pending"
        outcome = entry.get("outcome")
        if outcome != expected_outcome:
            findings.append(f"{rel_path}: required_checks.{check}.outcome must remain `{expected_outcome}`, got {outcome!r}")


def acceptance_provenance_keys(values: dict) -> list[str]:
    return sorted(key for key in values if "accepted" in key.lower() or "acceptance" in key.lower())


def find_pmxt_full_universes(ledger_spec: dict, findings: list[str]) -> list[dict]:
    universes = []
    venues = ledger_spec.get("venue", [])
    if not isinstance(venues, list):
        findings.append(f"{VENUE_LEDGER_SPEC}: venue must be an array")
        return []
    for venue in venues:
        if not isinstance(venue, dict):
            findings.append(f"{VENUE_LEDGER_SPEC}: venue entries must be objects")
            continue
        if venue.get("venue") != "pmxt":
            continue
        venue_universes = venue.get("universe", [])
        if not isinstance(venue_universes, list):
            findings.append(f"{VENUE_LEDGER_SPEC}: venue.universe must be an array")
            continue
        for universe in venue_universes:
            if not isinstance(universe, dict):
                findings.append(f"{VENUE_LEDGER_SPEC}: venue.universe entries must be objects")
                continue
            if universe.get("universe_id") == "pmxt-polymarket-full-current-data":
                universes.append(universe)
    return universes


def active_gitignore_patterns(gitignore: str) -> tuple[set[str], dict[str, bool]]:
    patterns: set[str] = set()
    representative_ignored = {pattern: False for pattern in PMXT_EVICTION_PATTERNS}
    for line in gitignore.splitlines():
        stripped = line.strip()
        if not stripped or stripped.startswith("#"):
            continue
        negated = stripped.startswith("!")
        pattern = stripped[1:] if negated else stripped
        pattern_for_match = pattern.lstrip("/")
        if not negated:
            patterns.add(pattern)
        for required_pattern, representative in PMXT_EVICTION_REPRESENTATIVES.items():
            if fnmatch.fnmatchcase(representative, pattern_for_match):
                representative_ignored[required_pattern] = not negated
    return patterns, representative_ignored


def just_recipe_commands(justfile: str, recipe_name: str) -> list[str]:
    commands: list[str] = []
    in_recipe = False
    for line in justfile.splitlines():
        stripped = line.strip()
        if not in_recipe:
            if line.startswith(f"{recipe_name}:"):
                in_recipe = True
            continue
        if stripped and not line[:1].isspace():
            break
        if not stripped or stripped.startswith("#"):
            continue
        commands.append(stripped)
    return commands


def check_status_hashes(root: Path, durable_status: dict, findings: list[str]) -> None:
    for key_path, rel_path in STATUS_HASH_TARGETS:
        entry = nested_mapping(durable_status, key_path, PMXT_DURABLE_STATUS, findings)
        label = ".".join(key_path)
        if key_path[0] in {"committed_input_hashes", "source_proof_admissibility_status"}:
            require_keys_equal(PMXT_DURABLE_STATUS, label, entry, DURABLE_STATUS_HASH_ENTRY_KEYS, findings)
        require_equal(PMXT_DURABLE_STATUS, f"{label}.path", entry.get("path"), repo_uri(rel_path), findings)
        require_equal(
            PMXT_DURABLE_STATUS,
            f"{label}.sha256",
            entry.get("sha256"),
            file_sha256(root, rel_path, findings),
            findings,
        )


def check_durable_status_artifact(durable_status: dict, findings: list[str]) -> None:
    require_keys_equal(PMXT_DURABLE_STATUS, "top-level", durable_status, DURABLE_STATUS_KEYS, findings)
    require_equal(
        PMXT_DURABLE_STATUS,
        "schema_version",
        durable_status.get("schema_version"),
        "source-proof-pmxt-durable-source-selection-status.v1",
        findings,
    )
    require_equal(PMXT_DURABLE_STATUS, "task_id", durable_status.get("task_id"), "BACKTESTING_ENGINE-022", findings)
    require_equal(
        PMXT_DURABLE_STATUS,
        "source_binding",
        durable_status.get("source_binding"),
        "polymarket-parquet-archive-index",
        findings,
    )

    source_proof_status = nested_mapping(durable_status, ("source_proof_set_spec",), PMXT_DURABLE_STATUS, findings)
    require_keys_equal(PMXT_DURABLE_STATUS, "source_proof_set_spec", source_proof_status, DURABLE_STATUS_SOURCE_PROOF_SPEC_KEYS, findings)
    require_equal(PMXT_DURABLE_STATUS, "source_proof_set_spec.status", source_proof_status.get("status"), "pending", findings)
    require_equal(
        PMXT_DURABLE_STATUS,
        "source_proof_set_spec.source_selection_status",
        source_proof_status.get("source_selection_status"),
        "PENDING_MORE_PROOF",
        findings,
    )
    require_equal(
        PMXT_DURABLE_STATUS,
        "source_proof_set_spec.usage_scope",
        source_proof_status.get("usage_scope"),
        "one_off_backfill_data",
        findings,
    )
    require_equal(PMXT_DURABLE_STATUS, "source_proof_set_spec.fidelity_class", source_proof_status.get("fidelity_class"), "L2_REPLAY", findings)

    admissibility_status = nested_mapping(
        durable_status,
        ("source_proof_admissibility_status",),
        PMXT_DURABLE_STATUS,
        findings,
    )
    require_keys_equal(
        PMXT_DURABLE_STATUS,
        "source_proof_admissibility_status",
        admissibility_status,
        DURABLE_STATUS_SOURCE_PROOF_ADMISSIBILITY_KEYS,
        findings,
    )
    for hash_key in ("proof_fixture", "acceptance_contract", "admissibility_contract"):
        hash_entry = nested_mapping(
            admissibility_status,
            (hash_key,),
            PMXT_DURABLE_STATUS,
            findings,
        )
        require_keys_equal(
            PMXT_DURABLE_STATUS,
            f"source_proof_admissibility_status.{hash_key}",
            hash_entry,
            DURABLE_STATUS_HASH_ENTRY_KEYS,
            findings,
        )
    require_equal(
        PMXT_DURABLE_STATUS,
        "source_proof_admissibility_status.status",
        admissibility_status.get("status"),
        "source_fenced_current_contract_rejected",
        findings,
    )
    require_equal(
        PMXT_DURABLE_STATUS,
        "source_proof_admissibility_status.proof_uri",
        admissibility_status.get("proof_uri"),
        repo_uri(PMXT_SOURCE_PROOF_FIXTURE),
        findings,
    )
    require_equal(
        PMXT_DURABLE_STATUS,
        "source_proof_admissibility_status.current_contract_deserializes",
        admissibility_status.get("current_contract_deserializes"),
        True,
        findings,
    )
    require_equal(
        PMXT_DURABLE_STATUS,
        "source_proof_admissibility_status.expected_record_status",
        admissibility_status.get("expected_record_status"),
        "current_contract_rejected",
        findings,
    )
    require_list_equal(
        PMXT_DURABLE_STATUS,
        "source_proof_admissibility_status.blocking_issues",
        admissibility_status.get("blocking_issues"),
        ("acceptance_failed",),
        findings,
    )
    require_list_equal(
        PMXT_DURABLE_STATUS,
        "source_proof_admissibility_status.missing_current_contract_fields",
        admissibility_status.get("missing_current_contract_fields"),
        (),
        findings,
    )
    require_equal(
        PMXT_DURABLE_STATUS,
        "source_proof_admissibility_status.acceptance_error",
        admissibility_status.get("acceptance_error"),
        'raw_sample_uri must be a staged s3:// URI, got "https://r2v2.pmxt.dev/polymarket_orderbook_2026-05-20T22.parquet"',
        findings,
    )
    require_equal(
        PMXT_DURABLE_STATUS,
        "source_proof_admissibility_status.usage_scope",
        admissibility_status.get("usage_scope"),
        "one_off_backfill_data",
        findings,
    )
    require_equal(
        PMXT_DURABLE_STATUS,
        "source_proof_admissibility_status.source_selection_status",
        admissibility_status.get("source_selection_status"),
        "PENDING_MORE_PROOF",
        findings,
    )
    require_equal(
        PMXT_DURABLE_STATUS,
        "source_proof_admissibility_status.source_binding",
        admissibility_status.get("source_binding"),
        "polymarket-parquet-archive-index",
        findings,
    )
    require_equal(
        PMXT_DURABLE_STATUS,
        "source_proof_admissibility_status.source_proof_id",
        admissibility_status.get("source_proof_id"),
        "source-proof-polymarket-pmxt-v2-orderbook-binary-option-pending-2026-06-08",
        findings,
    )

    committed_input_hashes = nested_mapping(durable_status, ("committed_input_hashes",), PMXT_DURABLE_STATUS, findings)
    require_keys_equal(PMXT_DURABLE_STATUS, "committed_input_hashes", committed_input_hashes, DURABLE_STATUS_COMMITTED_INPUT_HASHES_KEYS, findings)
    for hash_key, hash_entry in committed_input_hashes.items():
        require_keys_equal(PMXT_DURABLE_STATUS, f"committed_input_hashes.{hash_key}", hash_entry, DURABLE_STATUS_HASH_ENTRY_KEYS, findings)

    manifest_scope = nested_mapping(durable_status, ("manifest_scope",), PMXT_DURABLE_STATUS, findings)
    require_keys_equal(PMXT_DURABLE_STATUS, "manifest_scope", manifest_scope, DURABLE_STATUS_MANIFEST_SCOPE_KEYS, findings)
    require_equal(PMXT_DURABLE_STATUS, "manifest_scope.object_count", manifest_scope.get("object_count"), 1351, findings)
    require_equal(PMXT_DURABLE_STATUS, "manifest_scope.verified_head_count", manifest_scope.get("verified_head_count"), 1351, findings)
    require_equal(PMXT_DURABLE_STATUS, "manifest_scope.accepted_bytes", manifest_scope.get("accepted_bytes"), 557_815_904_970, findings)
    require_equal(PMXT_DURABLE_STATUS, "manifest_scope.source_accepted_proof_count", manifest_scope.get("source_accepted_proof_count"), 0, findings)

    require_list_equal(
        PMXT_DURABLE_STATUS,
        "pending_required_checks",
        durable_status.get("pending_required_checks"),
        (
            "instrument_universe",
            "coverage",
            "retention_freshness",
            "completeness",
            "cost",
            "storage",
        ),
        findings,
    )
    require_list_equal(
        PMXT_DURABLE_STATUS,
        "passed_required_checks",
        durable_status.get("passed_required_checks"),
        (
            "source_access",
            "license",
            "schema",
            "time_semantics",
            "granularity",
            "nt_mapping",
        ),
        findings,
    )

    generated_policy = nested_mapping(durable_status, ("generated_artifact_policy",), PMXT_DURABLE_STATUS, findings)
    require_keys_equal(PMXT_DURABLE_STATUS, "generated_artifact_policy", generated_policy, DURABLE_STATUS_GENERATED_ARTIFACT_POLICY_KEYS, findings)
    require_equal(PMXT_DURABLE_STATUS, "generated_artifact_policy.status", generated_policy.get("status"), "bulk_json_evicted", findings)
    require_list_equal(PMXT_DURABLE_STATUS, "generated_artifact_policy.gitignore_refs", generated_policy.get("gitignore_refs"), PMXT_EVICTION_PATTERNS, findings)

    guard_verification = nested_mapping(durable_status, ("guard_verification",), PMXT_DURABLE_STATUS, findings)
    require_keys_equal(PMXT_DURABLE_STATUS, "guard_verification", guard_verification, DURABLE_STATUS_GUARD_VERIFICATION_KEYS, findings)
    require_equal(
        PMXT_DURABLE_STATUS,
        "guard_verification.script",
        guard_verification.get("script"),
        "repo://scripts/verify_bte_022_pmxt_durable_source.py",
        findings,
    )
    require_equal(
        PMXT_DURABLE_STATUS,
        "guard_verification.self_test",
        guard_verification.get("self_test"),
        "repo://scripts/test_verify_bte_022_pmxt_durable_source.py",
        findings,
    )
    require_equal(PMXT_DURABLE_STATUS, "guard_verification.source_fence_static", guard_verification.get("source_fence_static"), True, findings)

    require_list_equal(PMXT_DURABLE_STATUS, "claim_limits", durable_status.get("claim_limits"), DURABLE_STATUS_CLAIM_LIMITS, findings)
    require_list_equal(PMXT_DURABLE_STATUS, "remaining_blockers", durable_status.get("remaining_blockers"), DURABLE_STATUS_REMAINING_BLOCKERS, findings)


def check_bte_status_durable_guard_block(bte_status: dict, findings: list[str]) -> None:
    guard_block = nested_mapping(
        bte_status,
        ("durable_source_selection_source_only_guardrail_status",),
        BTE_022_STATUS,
        findings,
    )
    require_keys_equal(
        BTE_022_STATUS,
        "durable_source_selection_source_only_guardrail_status",
        guard_block,
        BTE_DURABLE_GUARD_KEYS,
        findings,
    )
    require_equal(BTE_022_STATUS, "durable_source_selection_source_only_guardrail_status.status", guard_block.get("status"), BTE_DURABLE_GUARD_STATUS, findings)
    require_list_equal(
        BTE_022_STATUS,
        "durable_source_selection_source_only_guardrail_status.evidence",
        guard_block.get("evidence"),
        BTE_DURABLE_GUARD_EVIDENCE,
        findings,
    )
    require_list_equal(
        BTE_022_STATUS,
        "durable_source_selection_source_only_guardrail_status.claim_limits",
        guard_block.get("claim_limits"),
        BTE_DURABLE_GUARD_CLAIM_LIMITS,
        findings,
    )


def check_bte_status_artifact(bte_status: dict, findings: list[str]) -> None:
    require_keys_equal(BTE_022_STATUS, "top-level", bte_status, BTE_STATUS_KEYS, findings)
    require_equal(BTE_022_STATUS, "schema_version", bte_status.get("schema_version"), "source-proof-nt-catalog-mapping-status.v1", findings)
    require_equal(BTE_022_STATUS, "task_id", bte_status.get("task_id"), "BACKTESTING_ENGINE-022", findings)
    require_equal(BTE_022_STATUS, "status", bte_status.get("status"), BTE_STATUS_STATUS, findings)
    require_equal(BTE_022_STATUS, "decision", bte_status.get("decision"), BTE_STATUS_DECISION, findings)
    require_equal(BTE_022_STATUS, "recorded_at", bte_status.get("recorded_at"), "2026-06-08", findings)
    check_bte_status_durable_guard_block(bte_status, findings)


def scan_root(root: Path) -> list[str]:
    root = root.resolve()
    findings: list[str] = []

    proof_spec = read_toml(root, PMXT_SOURCE_PROOF_SPEC, findings)
    source_manifest = read_json(root, PMXT_SOURCE_MANIFEST, findings)
    category_manifest = read_json(root, PMXT_CATEGORY_MANIFEST, findings)
    archive_index = read_json(root, PMXT_ARCHIVE_INDEX_MANIFEST, findings)
    queue_spec = read_toml(root, PMXT_CONVERSION_QUEUE_SPEC, findings)
    fixture = read_json(root, PMXT_SOURCE_PROOF_FIXTURE, findings)
    venue_ledger = read_toml(root, VENUE_LEDGER_SPEC, findings)
    bte_status = read_json(root, BTE_022_STATUS, findings)
    durable_status = read_json(root, PMXT_DURABLE_STATUS, findings)
    gitignore = read_text(root, GITIGNORE, findings)
    justfile = read_text(root, JUSTFILE, findings)
    source_proof_contract = read_text(root, SOURCE_PROOF_ACCEPTANCE_CONTRACT, findings)
    admissibility_contract = read_text(root, SOURCE_PROOF_ADMISSIBILITY_CONTRACT, findings)

    for snippet in SOURCE_PROOF_ACCEPTANCE_SNIPPETS:
        require_text_contains(SOURCE_PROOF_ACCEPTANCE_CONTRACT, "PMXT source-proof acceptance guard", source_proof_contract, snippet, findings)
    for snippet in SOURCE_PROOF_ADMISSIBILITY_SNIPPETS:
        require_text_contains(
            SOURCE_PROOF_ADMISSIBILITY_CONTRACT,
            "PMXT source-proof admissibility classification",
            admissibility_contract,
            snippet,
            findings,
        )

    require_keys_equal(PMXT_SOURCE_PROOF_SPEC, "top-level", proof_spec, PMXT_SOURCE_PROOF_SPEC_KEYS, findings)
    require_equal(PMXT_SOURCE_PROOF_SPEC, "status", proof_spec.get("status"), "pending", findings)
    require_equal(
        PMXT_SOURCE_PROOF_SPEC,
        "source_selection_status",
        proof_spec.get("source_selection_status"),
        "PENDING_MORE_PROOF",
        findings,
    )
    require_equal(PMXT_SOURCE_PROOF_SPEC, "usage_scope", proof_spec.get("usage_scope"), "one_off_backfill_data", findings)
    require_equal(PMXT_SOURCE_PROOF_SPEC, "fidelity_class", proof_spec.get("fidelity_class"), "L2_REPLAY", findings)
    require_equal(
        PMXT_SOURCE_PROOF_SPEC,
        "manifest_table_family",
        proof_spec.get("manifest_table_family"),
        "order_book_snapshot_deltas",
        findings,
    )
    acceptance_keys = acceptance_provenance_keys(proof_spec)
    if acceptance_keys:
        findings.append(f"{PMXT_SOURCE_PROOF_SPEC}: pending PMXT proof must not carry acceptance provenance keys {acceptance_keys!r}")
    l2_replay_evidence = proof_spec.get("l2_replay_evidence")
    require_keys_equal(PMXT_SOURCE_PROOF_SPEC, "l2_replay_evidence", l2_replay_evidence, PMXT_SOURCE_PROOF_SPEC_L2_EVIDENCE_KEYS, findings)
    check_required_checks(
        PMXT_SOURCE_PROOF_SPEC,
        proof_spec.get("required_checks", {}),
        SOURCE_PROOF_SPEC_PASSED_CHECKS,
        SOURCE_PROOF_SPEC_PENDING_CHECKS,
        findings,
    )
    bindings = proof_spec.get("source_binding", [])
    if not isinstance(bindings, list):
        findings.append(f"{PMXT_SOURCE_PROOF_SPEC}: source_binding must be an array")
    elif len(bindings) != 1:
        findings.append(f"{PMXT_SOURCE_PROOF_SPEC}: expected exactly one source_binding entry")
    else:
        binding = bindings[0]
        if not isinstance(binding, dict):
            findings.append(f"{PMXT_SOURCE_PROOF_SPEC}: source_binding entries must be objects")
            binding = {}
        require_keys_equal(PMXT_SOURCE_PROOF_SPEC, "source_binding entry", binding, PMXT_SOURCE_PROOF_SPEC_BINDING_KEYS, findings)
        binding_acceptance_keys = acceptance_provenance_keys(binding)
        if binding_acceptance_keys:
            findings.append(
                f"{PMXT_SOURCE_PROOF_SPEC}: pending PMXT source_binding must not carry acceptance provenance keys {binding_acceptance_keys!r}"
            )
        require_equal(PMXT_SOURCE_PROOF_SPEC, "source_binding", binding.get("source_binding"), "polymarket-parquet-archive-index", findings)
        require_equal(
            PMXT_SOURCE_PROOF_SPEC,
            "source_proof_id",
            binding.get("source_proof_id"),
            "source-proof-pmxt-polymarket-v2-current-orderbook",
            findings,
        )

    require_equal(PMXT_SOURCE_MANIFEST, "object_count", source_manifest.get("object_count"), 1351, findings)
    require_equal(PMXT_SOURCE_MANIFEST, "accepted_bytes", source_manifest.get("accepted_bytes"), 557_815_904_970, findings)
    require_equal(PMXT_CATEGORY_MANIFEST, "object_count", category_manifest.get("object_count"), 1351, findings)
    require_equal(PMXT_CATEGORY_MANIFEST, "accepted_bytes", category_manifest.get("accepted_bytes"), 557_815_904_970, findings)
    require_equal(PMXT_ARCHIVE_INDEX_MANIFEST, "object_count", archive_index.get("object_count"), 1351, findings)
    require_equal(PMXT_ARCHIVE_INDEX_MANIFEST, "verified_head_count", archive_index.get("verified_head_count"), 1351, findings)
    require_equal(
        PMXT_ARCHIVE_INDEX_MANIFEST,
        "total_content_length_bytes",
        archive_index.get("total_content_length_bytes"),
        557_815_904_970,
        findings,
    )
    require_equal(
        PMXT_CONVERSION_QUEUE_SPEC,
        "source_universe_manifest_path",
        queue_spec.get("source_universe_manifest_path"),
        PMXT_SOURCE_MANIFEST.as_posix(),
        findings,
    )

    require_equal(PMXT_SOURCE_PROOF_FIXTURE, "status", fixture.get("status"), "pending", findings)
    require_equal(
        PMXT_SOURCE_PROOF_FIXTURE,
        "source_selection_status",
        fixture.get("source_selection_status"),
        "PENDING_MORE_PROOF",
        findings,
    )
    require_equal(PMXT_SOURCE_PROOF_FIXTURE, "usage_scope", fixture.get("usage_scope"), "one_off_backfill_data", findings)
    acceptance_scope = nested_mapping(fixture, ("acceptance_scope",), PMXT_SOURCE_PROOF_FIXTURE, findings)
    require_equal(PMXT_SOURCE_PROOF_FIXTURE, "acceptance_scope.planned_objects", acceptance_scope.get("planned_objects"), 1, findings)
    require_equal(PMXT_SOURCE_PROOF_FIXTURE, "acceptance_scope.completed_objects", acceptance_scope.get("completed_objects"), 1, findings)
    require_equal(PMXT_SOURCE_PROOF_FIXTURE, "acceptance_scope.failed_objects", acceptance_scope.get("failed_objects"), 0, findings)
    require_equal(PMXT_SOURCE_PROOF_FIXTURE, "acceptance_scope.skipped_objects", acceptance_scope.get("skipped_objects"), 0, findings)
    require_equal(PMXT_SOURCE_PROOF_FIXTURE, "acceptance_scope.accepted_bytes", acceptance_scope.get("accepted_bytes"), 361_365_244, findings)
    require_equal(PMXT_SOURCE_PROOF_FIXTURE, "acceptance_scope.selector_scope_violations", acceptance_scope.get("selector_scope_violations"), 0, findings)
    check_required_checks(
        PMXT_SOURCE_PROOF_FIXTURE,
        fixture.get("required_checks", {}),
        ONE_OFF_FIXTURE_PASSED_CHECKS,
        ONE_OFF_FIXTURE_PENDING_CHECKS,
        findings,
    )
    admissibility_status = nested_mapping(
        durable_status,
        ("source_proof_admissibility_status",),
        PMXT_DURABLE_STATUS,
        findings,
    )
    for label in ("source_proof_id", "source_binding", "usage_scope", "source_selection_status"):
        require_equal(
            PMXT_DURABLE_STATUS,
            f"source_proof_admissibility_status.{label}",
            admissibility_status.get(label),
            fixture.get(label),
            findings,
        )

    pmxt_full_universes = find_pmxt_full_universes(venue_ledger, findings)
    if len(pmxt_full_universes) != 1:
        findings.append(
            f"{VENUE_LEDGER_SPEC}: expected exactly one pmxt-polymarket-full-current-data universe, got {len(pmxt_full_universes)}"
        )
    else:
        pmxt_full = pmxt_full_universes[0]
        require_equal(VENUE_LEDGER_SPEC, "pmxt full status", pmxt_full.get("status"), "blocked", findings)
        for issue in (
            "missing_accepted_source_proof",
            "missing_source_universe_object_gates",
            "missing_source_universe_conversion_run_plan",
            "missing_pmxt_l2_tick_size_epoch_policy",
        ):
            require_contains(VENUE_LEDGER_SPEC, "pmxt full blocking_issues", pmxt_full.get("blocking_issues"), issue, findings)
        require_equal(
            VENUE_LEDGER_SPEC,
            "source_universe_source_proof_set_path",
            pmxt_full.get("source_universe_source_proof_set_path"),
            "specs/023-nt-research-analytics-platform/reference/backfill-source-proofs/pmxt-polymarket-v2-current/source-universe-source-proof-set.json",
            findings,
        )

    blockers = bte_status.get("remaining_blockers")
    if not isinstance(blockers, list):
        findings.append(f"{BTE_022_STATUS}: remaining_blockers must be a list")
    else:
        blocker_names = tuple(blocker.get("blocker") for blocker in blockers if isinstance(blocker, dict))
        if len(blocker_names) != len(blockers):
            findings.append(f"{BTE_022_STATUS}: remaining_blockers entries must be objects with blocker names")
        require_equal(BTE_022_STATUS, "remaining_blockers.blocker_names", blocker_names, BTE_REMAINING_BLOCKERS, findings)
    require_equal(BTE_022_STATUS, "bte_022_can_close", bte_status.get("bte_022_can_close"), False, findings)
    check_bte_status_artifact(bte_status, findings)

    require_equal(
        PMXT_DURABLE_STATUS,
        "durable_source_selection_status",
        durable_status.get("durable_source_selection_status"),
        "blocked_pending_source_proof",
        findings,
    )
    require_equal(PMXT_DURABLE_STATUS, "bte_022_can_close", durable_status.get("bte_022_can_close"), False, findings)
    require_equal(PMXT_DURABLE_STATUS, "source_proof_count", durable_status.get("source_proof_count"), 1, findings)
    require_equal(PMXT_DURABLE_STATUS, "source_accepted_proof_count", durable_status.get("source_accepted_proof_count"), 0, findings)
    check_status_hashes(root, durable_status, findings)
    check_durable_status_artifact(durable_status, findings)

    gitignore_patterns, representative_ignored = active_gitignore_patterns(gitignore)
    for pattern in PMXT_EVICTION_PATTERNS:
        if pattern not in gitignore_patterns:
            findings.append(f"{GITIGNORE}: missing PMXT generated-artifact eviction pattern `{pattern}`")
        if not representative_ignored.get(pattern, False):
            findings.append(
                f"{GITIGNORE}: PMXT generated-artifact eviction pattern `{pattern}` must effectively ignore representative `{PMXT_EVICTION_REPRESENTATIVES[pattern]}`"
            )
    for recipe in JUSTFILE_RECIPES:
        recipe_commands = just_recipe_commands(justfile, recipe)
        for command in JUSTFILE_COMMANDS:
            if command not in recipe_commands:
                findings.append(f"{JUSTFILE}: {recipe} must run {command}")
    source_fence_commands = just_recipe_commands(justfile, "source-fence-static-inner")
    if not source_fence_commands:
        findings.append(f"{JUSTFILE}: missing recipe source-fence-static-inner")
        return findings
    if tuple(source_fence_commands) != SOURCE_FENCE_STATIC_COMMANDS:
        expected = " && ".join(SOURCE_FENCE_STATIC_COMMANDS)
        findings.append(f"{JUSTFILE}: source-fence-static-inner must contain only {expected}")

    return findings


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--root", type=Path, default=REPO_ROOT)
    args = parser.parse_args(argv)

    findings = scan_root(args.root)
    if findings:
        print("FAIL: BTE-022 PMXT durable-source status violations:", file=sys.stderr)
        for finding in findings:
            print(f"  {finding}", file=sys.stderr)
        return 1
    print("OK: BTE-022 PMXT durable-source status passed.")
    return 0


if __name__ == "__main__":
    import lane_governor

    lane_governor.acquire()
    raise SystemExit(main())
