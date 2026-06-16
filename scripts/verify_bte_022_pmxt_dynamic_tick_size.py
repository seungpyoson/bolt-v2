#!/usr/bin/env python3
"""Verify BTE-022 PMXT dynamic tick-size replay stays fail-closed."""

from __future__ import annotations

import argparse
import hashlib
import json
import sys
import tomllib
from pathlib import Path
from typing import Any


REPO_ROOT = Path(__file__).resolve().parent.parent
REFERENCE_ROOT = Path("specs/023-nt-research-analytics-platform/reference")
BTE_022_STATUS = REFERENCE_ROOT / "source-proof-nt-catalog-mapping-status.backtesting-engine-022.2026-06-08.json"
PMXT_TICK_STATUS = REFERENCE_ROOT / "source-proof-pmxt-polymarket-tick-size-change-status.2026-06-08.json"
PMXT_TIMED_AUDIT = REFERENCE_ROOT / "source-proof-pmxt-polymarket-timed-instrument-replay-nt-audit.2026-06-09.json"
PMXT_FIRST_UNIVERSE_POLICY = REFERENCE_ROOT / "source-proof-pmxt-polymarket-first-proof-universe-policy.2026-06-08.json"
PMXT_SOURCE_PROOF_SPEC = REFERENCE_ROOT / "backfill-source-proofs/pmxt-polymarket-v2-current/source-universe-source-proofs.toml"
PMXT_DYNAMIC_STATUS = REFERENCE_ROOT / "source-proof-pmxt-dynamic-tick-size-replay-status.2026-06-16.json"
JUSTFILE = Path("justfile")
FIRST_SELECTION_KEY = "_".join(("selected", "first", "proof", "policy"))

NT_REVISION = "6e059dcbb59ac1e582132fc431a581936c216c3c"
JUSTFILE_COMMANDS = (
    "python3 scripts/test_verify_bte_022_pmxt_dynamic_tick_size.py",
    "python3 scripts/verify_bte_022_pmxt_dynamic_tick_size.py",
)
JUSTFILE_RECIPES = ("verify-bte-022-pmxt-dynamic-tick-size", "source-fence-static")
STATUS_HASH_TARGETS = (
    (("committed_input_hashes", "tick_size_change_status"), PMXT_TICK_STATUS),
    (("committed_input_hashes", "timed_instrument_replay_audit"), PMXT_TIMED_AUDIT),
    (("committed_input_hashes", "first_universe_policy"), PMXT_FIRST_UNIVERSE_POLICY),
    (("committed_input_hashes", "pmxt_source_proof_spec"), PMXT_SOURCE_PROOF_SPEC),
)
STATUS_KEYS = (
    "bounded_no_tick_size_change_first_proof_allowed",
    "bte_022_can_close",
    "claim_limits",
    "committed_input_hashes",
    "dynamic_tick_size_replay_status",
    "guard_verification",
    "observed_at_utc",
    "pmxt_full_l2_with_tick_size_change_can_be_accepted_now",
    "remaining_blockers",
    "schema_version",
    "source_binding",
    "standard_backtestnode_catalog_replay_supports_dynamic_instrument_any",
    "task_id",
    "timed_instrument_epoch_replay_accepted",
)
STATUS_HASH_KEYS = (
    "first_universe_policy",
    "pmxt_source_proof_spec",
    "tick_size_change_status",
    "timed_instrument_replay_audit",
)
HASH_ENTRY_KEYS = ("path", "sha256")
GUARD_VERIFICATION_KEYS = ("script", "self_test", "source_fence_static")
SOURCE_PROOF_PENDING_FORBIDDEN_L2_REFS = (
    "no_tick_size_change_universe_ref",
    "timed_instrument_epoch_replay_ref",
)
REMAINING_BLOCKERS = (
    "dynamic_tick_size_replay_unproven",
    "expanded_tranche_coverage_and_cost_unproven",
    "durable_source_selection_unproven",
    "broad_backfill_efficiency_unproven",
)
BTE_REMAINING_BLOCKERS = (
    "expanded_tranche_coverage_and_cost_unproven",
    "dynamic_tick_size_replay_unproven",
    "durable_source_selection_unproven",
    "broad_backfill_efficiency_unproven",
)
CLAIM_LIMITS = (
    "Standard BacktestNode catalog replay must not be used to claim PMXT dynamic tick-size replay.",
    "InstrumentStatus and InstrumentClose auxiliary streams do not substitute for timed InstrumentAny instrument-definition epochs.",
    "The bounded PMXT first proof may use only a source-proof-bound no-tick-size-change universe and cannot prove full PMXT L2 acceptance.",
    "The pending PMXT source proof must remain blocked until either timed InstrumentAny replay is accepted or an accepted no-tick-size-change universe scope closes the claim.",
    "This does not authorize broad PMXT backfill.",
)
BTE_DYNAMIC_GUARD_STATUS = "source_fenced_blocked_standard_backtestnode_no_timed_instrument_any"
BTE_DYNAMIC_GUARD_EVIDENCE = (
    "repo://specs/023-nt-research-analytics-platform/reference/source-proof-pmxt-polymarket-tick-size-change-status.2026-06-08.json records that standard BacktestNode catalog replay cannot schedule timed InstrumentAny instrument-definition epochs.",
    "repo://specs/023-nt-research-analytics-platform/reference/source-proof-pmxt-polymarket-timed-instrument-replay-nt-audit.2026-06-09.json records the pinned NT audit: InstrumentAny storage exists, but BacktestDataConfig/Data replay has no timed InstrumentAny stream.",
    "repo://specs/023-nt-research-analytics-platform/reference/source-proof-pmxt-polymarket-first-proof-universe-policy.2026-06-08.json permits only the bounded no-tick-size-change first proof and keeps full PMXT L2 acceptance open.",
    "repo://specs/023-nt-research-analytics-platform/reference/backfill-source-proofs/pmxt-polymarket-v2-current/source-universe-source-proofs.toml keeps the PMXT source proof pending and blocks dynamic tick-size replay acceptance through claim_limit pmxt-source-proof-claim-limit-002.",
    "STATIC-GATED scripts/verify_bte_022_pmxt_dynamic_tick_size.py rejects drift from the blocked dynamic tick-size replay decision, missing BTE-022 blocker text, source-proof overclaiming, and source-fence wiring gaps.",
)
BTE_DYNAMIC_GUARD_CLAIM_LIMITS = (
    "This proves the current repo state records dynamic tick-size replay as blocked for standard BacktestNode catalog replay.",
    "This proves the bounded PMXT first proof is limited to source-proof-selected assets/windows with tick_size_change_rows == 0.",
    "This proves the pending PMXT source proof has not accepted timed InstrumentAny replay or full PMXT L2 with tick_size_change rows.",
    "This does not implement timed InstrumentAny replay.",
    "This does not close BTE-022 or authorize broad PMXT backfill.",
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


def read_toml(root: Path, rel_path: Path, findings: list[str]) -> dict[str, Any]:
    text = read_text(root, rel_path, findings)
    if not text:
        return {}
    try:
        loaded = tomllib.loads(text)
    except tomllib.TOMLDecodeError as exc:
        findings.append(f"{rel_path}: invalid TOML: {exc}")
        return {}
    if not isinstance(loaded, dict):
        findings.append(f"{rel_path}: expected top-level TOML table")
        return {}
    return loaded


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


def require_absent(rel_path: Path, field: str, actual: Any, forbidden: str, findings: list[str]) -> None:
    if isinstance(actual, str):
        haystack = actual
    elif isinstance(actual, list):
        haystack = "\n".join(str(item) for item in actual)
    else:
        findings.append(f"{rel_path}: {field} must be text when checking absence of {forbidden!r}, got {actual!r}")
        return
    if forbidden in haystack:
        findings.append(f"{rel_path}: {field} must not contain pending source-proof ref {forbidden!r}")


def path_sha256(root: Path, rel_path: Path, findings: list[str]) -> str:
    path = root / rel_path
    if not path.exists():
        findings.append(f"{rel_path}: cannot hash missing file")
        return ""
    return hashlib.sha256(path.read_bytes()).hexdigest()


def nested_value(data: dict[str, Any], path: tuple[str, ...]) -> Any:
    current: Any = data
    for key in path:
        if not isinstance(current, dict):
            return None
        current = current.get(key)
    return current


def just_recipe_commands(text: str, recipe: str) -> list[str]:
    commands: list[str] = []
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
            commands.append(stripped)
    return commands


def check_status(status: dict[str, Any], root: Path, findings: list[str]) -> None:
    require_keys(PMXT_DYNAMIC_STATUS, "top-level", status, STATUS_KEYS, findings)
    require_equal(PMXT_DYNAMIC_STATUS, "schema_version", status.get("schema_version"), "source-proof-pmxt-dynamic-tick-size-replay-status.v1", findings)
    require_equal(PMXT_DYNAMIC_STATUS, "task_id", status.get("task_id"), "BACKTESTING_ENGINE-022", findings)
    require_equal(PMXT_DYNAMIC_STATUS, "source_binding", status.get("source_binding"), "polymarket-parquet-archive-index", findings)
    require_equal(
        PMXT_DYNAMIC_STATUS,
        "dynamic_tick_size_replay_status",
        status.get("dynamic_tick_size_replay_status"),
        "blocked_standard_backtestnode_no_timed_instrument_any",
        findings,
    )
    require_equal(PMXT_DYNAMIC_STATUS, "standard_backtestnode_catalog_replay_supports_dynamic_instrument_any", status.get("standard_backtestnode_catalog_replay_supports_dynamic_instrument_any"), False, findings)
    require_equal(PMXT_DYNAMIC_STATUS, "timed_instrument_epoch_replay_accepted", status.get("timed_instrument_epoch_replay_accepted"), False, findings)
    require_equal(PMXT_DYNAMIC_STATUS, "bounded_no_tick_size_change_first_proof_allowed", status.get("bounded_no_tick_size_change_first_proof_allowed"), True, findings)
    require_equal(PMXT_DYNAMIC_STATUS, "pmxt_full_l2_with_tick_size_change_can_be_accepted_now", status.get("pmxt_full_l2_with_tick_size_change_can_be_accepted_now"), False, findings)
    require_equal(PMXT_DYNAMIC_STATUS, "bte_022_can_close", status.get("bte_022_can_close"), False, findings)
    require_list_equal(PMXT_DYNAMIC_STATUS, "remaining_blockers", status.get("remaining_blockers"), REMAINING_BLOCKERS, findings)
    require_list_equal(PMXT_DYNAMIC_STATUS, "claim_limits", status.get("claim_limits"), CLAIM_LIMITS, findings)

    hashes = status.get("committed_input_hashes")
    require_keys(PMXT_DYNAMIC_STATUS, "committed_input_hashes", hashes, STATUS_HASH_KEYS, findings)
    if isinstance(hashes, dict):
        for path_tuple, target in STATUS_HASH_TARGETS:
            entry = nested_value(status, path_tuple)
            require_keys(PMXT_DYNAMIC_STATUS, ".".join(path_tuple), entry, HASH_ENTRY_KEYS, findings)
            if isinstance(entry, dict):
                require_equal(PMXT_DYNAMIC_STATUS, ".".join(path_tuple) + ".path", entry.get("path"), str(target), findings)
                require_equal(PMXT_DYNAMIC_STATUS, ".".join(path_tuple) + ".sha256", entry.get("sha256"), path_sha256(root, target, findings), findings)

    guard = status.get("guard_verification")
    require_keys(PMXT_DYNAMIC_STATUS, "guard_verification", guard, GUARD_VERIFICATION_KEYS, findings)
    if isinstance(guard, dict):
        require_equal(PMXT_DYNAMIC_STATUS, "guard_verification.script", guard.get("script"), "repo://scripts/verify_bte_022_pmxt_dynamic_tick_size.py", findings)
        require_equal(PMXT_DYNAMIC_STATUS, "guard_verification.self_test", guard.get("self_test"), "repo://scripts/test_verify_bte_022_pmxt_dynamic_tick_size.py", findings)
        require_equal(PMXT_DYNAMIC_STATUS, "guard_verification.source_fence_static", guard.get("source_fence_static"), True, findings)


def check_tick_status(tick_status: dict[str, Any], findings: list[str]) -> None:
    require_equal(PMXT_TICK_STATUS, "schema_version", tick_status.get("schema_version"), "source-proof-pmxt-polymarket-tick-size-change-status.v1", findings)
    require_equal(PMXT_TICK_STATUS, "task_id", tick_status.get("task_id"), "BACKTESTING_ENGINE-022", findings)
    require_equal(PMXT_TICK_STATUS, "status", tick_status.get("status"), "open_standard_backtestnode_catalog_replay_does_not_support_dynamic_instrument_epoch", findings)
    require_equal(PMXT_TICK_STATUS, "pinned_nt_revision", tick_status.get("pinned_nt_revision"), NT_REVISION, findings)
    scope = tick_status.get("scope")
    if isinstance(scope, dict):
        require_equal(PMXT_TICK_STATUS, "scope.standard_backtestnode_catalog_replay_supports_timed_instrument_any", scope.get("standard_backtestnode_catalog_replay_supports_timed_instrument_any"), False, findings)
        require_equal(PMXT_TICK_STATUS, "scope.not_implementation", scope.get("not_implementation"), True, findings)
    else:
        findings.append(f"{PMXT_TICK_STATUS}: scope must be an object")
    current = tick_status.get("current_decision")
    if isinstance(current, dict):
        require_equal(PMXT_TICK_STATUS, "current_decision.standard_backtestnode_catalog_replay_supports_dynamic_instrument_any", current.get("standard_backtestnode_catalog_replay_supports_dynamic_instrument_any"), False, findings)
        require_equal(PMXT_TICK_STATUS, "current_decision.tick_size_change_policy_can_close", current.get("tick_size_change_policy_can_close"), False, findings)
        require_equal(PMXT_TICK_STATUS, "current_decision.first_proof_exclusion_policy_can_close", current.get("first_proof_exclusion_policy_can_close"), True, findings)
        require_equal(PMXT_TICK_STATUS, "current_decision.bte_022_can_close", current.get("bte_022_can_close"), False, findings)
        require_equal(PMXT_TICK_STATUS, "current_decision.broad_backfill_allowed", current.get("broad_backfill_allowed"), False, findings)
        require_contains(PMXT_TICK_STATUS, "current_decision.next_required_evidence", current.get("next_required_evidence"), "timed InstrumentAny replay mechanism", findings)
    else:
        findings.append(f"{PMXT_TICK_STATUS}: current_decision must be an object")
    sample = tick_status.get("pmxt_sample_evidence")
    if isinstance(sample, dict):
        require_equal(PMXT_TICK_STATUS, "pmxt_sample_evidence.event_type", sample.get("event_type"), "tick_size_change", findings)
        require_equal(PMXT_TICK_STATUS, "pmxt_sample_evidence.row_count", sample.get("row_count"), 419, findings)
        require_equal(PMXT_TICK_STATUS, "pmxt_sample_evidence.distinct_assets", sample.get("distinct_assets"), 343, findings)
    else:
        findings.append(f"{PMXT_TICK_STATUS}: pmxt_sample_evidence must be an object")
    require_contains(PMXT_TICK_STATUS, "bte_manifest_surface_evidence", tick_status.get("bte_manifest_surface_evidence"), "InstrumentStatus and InstrumentClose are Data enum replay items, not InstrumentAny", findings)


def check_timed_audit(audit: dict[str, Any], findings: list[str]) -> None:
    require_equal(PMXT_TIMED_AUDIT, "schema_version", audit.get("schema_version"), 1, findings)
    require_equal(PMXT_TIMED_AUDIT, "artifact", audit.get("artifact"), "source-proof-pmxt-polymarket-timed-instrument-replay-nt-audit", findings)
    require_equal(PMXT_TIMED_AUDIT, "task", audit.get("task"), "BACKTESTING_ENGINE-022", findings)
    require_equal(PMXT_TIMED_AUDIT, "nt_revision", audit.get("nt_revision"), NT_REVISION, findings)
    require_contains(PMXT_TIMED_AUDIT, "answer", audit.get("answer"), "No. Pinned NT can store multiple InstrumentAny snapshots", findings)
    decisions = audit.get("decisions")
    if isinstance(decisions, dict):
        require_equal(PMXT_TIMED_AUDIT, "decisions.standard_backtestnode_catalog_replay_has_timed_instrument_any", decisions.get("standard_backtestnode_catalog_replay_has_timed_instrument_any"), False, findings)
        require_equal(PMXT_TIMED_AUDIT, "decisions.instrument_status_or_close_can_substitute_for_tick_size_change", decisions.get("instrument_status_or_close_can_substitute_for_tick_size_change"), False, findings)
        require_equal(PMXT_TIMED_AUDIT, "decisions.pmxt_full_l2_with_tick_size_change_can_be_accepted_now", decisions.get("pmxt_full_l2_with_tick_size_change_can_be_accepted_now"), False, findings)
        require_equal(PMXT_TIMED_AUDIT, "decisions.bounded_no_tick_size_change_pmxt_first_proof_can_continue", decisions.get("bounded_no_tick_size_change_pmxt_first_proof_can_continue"), True, findings)
        require_equal(PMXT_TIMED_AUDIT, "decisions.bte_022_can_close", decisions.get("bte_022_can_close"), False, findings)
    else:
        findings.append(f"{PMXT_TIMED_AUDIT}: decisions must be an object")
    require_contains(PMXT_TIMED_AUDIT, "rejected_paths", audit.get("rejected_paths"), "ignore_tick_size_change_rows", findings)
    require_contains(PMXT_TIMED_AUDIT, "next_required_evidence", audit.get("next_required_evidence"), "timed InstrumentAny replay mechanism", findings)


def check_first_universe_policy(policy: dict[str, Any], findings: list[str]) -> None:
    require_equal(PMXT_FIRST_UNIVERSE_POLICY, "schema_version", policy.get("schema_version"), "source-proof-pmxt-polymarket-first-proof-universe-policy.v1", findings)
    require_equal(PMXT_FIRST_UNIVERSE_POLICY, "task_id", policy.get("task_id"), "BACKTESTING_ENGINE-022", findings)
    require_equal(PMXT_FIRST_UNIVERSE_POLICY, "source_binding", policy.get("source_binding"), "polymarket-parquet-archive-index", findings)
    require_equal(PMXT_FIRST_UNIVERSE_POLICY, "status", policy.get("status"), "first_proof_exclusion_policy_selected_tdd_proven_for_selector_artifacts", findings)
    require_contains(PMXT_FIRST_UNIVERSE_POLICY, "claim_limits", policy.get("claim_limits"), "Does not prove dynamic tick-size replay.", findings)
    current = policy.get("current_decision")
    if isinstance(current, dict):
        require_equal(PMXT_FIRST_UNIVERSE_POLICY, "current_decision.tick_size_dynamic_replay_can_close", current.get("tick_size_dynamic_replay_can_close"), False, findings)
        require_equal(PMXT_FIRST_UNIVERSE_POLICY, "current_decision.bte_022_can_close", current.get("bte_022_can_close"), False, findings)
        require_equal(PMXT_FIRST_UNIVERSE_POLICY, "current_decision.broad_backfill_allowed", current.get("broad_backfill_allowed"), False, findings)
    else:
        findings.append(f"{PMXT_FIRST_UNIVERSE_POLICY}: current_decision must be an object")
    selected = policy.get(FIRST_SELECTION_KEY)
    if isinstance(selected, dict):
        require_contains(PMXT_FIRST_UNIVERSE_POLICY, f"{FIRST_SELECTION_KEY}.selector_predicate", selected.get("selector_predicate"), "tick_size_change_rows == 0", findings)
        require_contains(PMXT_FIRST_UNIVERSE_POLICY, f"{FIRST_SELECTION_KEY}.required_manifest_bindings", selected.get("required_manifest_bindings"), "excluded_tick_change_event_count", findings)
    else:
        findings.append(f"{PMXT_FIRST_UNIVERSE_POLICY}: {FIRST_SELECTION_KEY} must be an object")
    evidence = policy.get("pmxt_one_object_evidence")
    if isinstance(evidence, dict):
        counts = evidence.get("instrument_universe_counts", {})
        require_equal(PMXT_FIRST_UNIVERSE_POLICY, "pmxt_one_object_evidence.instrument_universe_counts.assets_with_tick_change", counts.get("assets_with_tick_change") if isinstance(counts, dict) else None, 343, findings)
        eligible = evidence.get("eligible_first_proof_assets", {})
        require_equal(PMXT_FIRST_UNIVERSE_POLICY, "pmxt_one_object_evidence.eligible_first_proof_assets.eligible_assets", eligible.get("eligible_assets") if isinstance(eligible, dict) else None, 823, findings)
    else:
        findings.append(f"{PMXT_FIRST_UNIVERSE_POLICY}: pmxt_one_object_evidence must be an object")


def check_source_proof(spec: dict[str, Any], findings: list[str]) -> None:
    require_equal(PMXT_SOURCE_PROOF_SPEC, "status", spec.get("status"), "pending", findings)
    require_equal(PMXT_SOURCE_PROOF_SPEC, "source_selection_status", spec.get("source_selection_status"), "PENDING_MORE_PROOF", findings)
    require_equal(PMXT_SOURCE_PROOF_SPEC, "usage_scope", spec.get("usage_scope"), "one_off_backfill_data", findings)
    require_equal(PMXT_SOURCE_PROOF_SPEC, "fidelity_class", spec.get("fidelity_class"), "L2_REPLAY", findings)
    l2 = spec.get("l2_replay_evidence")
    if not isinstance(l2, dict):
        findings.append(f"{PMXT_SOURCE_PROOF_SPEC}: l2_replay_evidence must be an object")
    else:
        for forbidden in SOURCE_PROOF_PENDING_FORBIDDEN_L2_REFS:
            if forbidden in l2:
                findings.append(f"{PMXT_SOURCE_PROOF_SPEC}: l2_replay_evidence.{forbidden} must remain absent until accepted broad PMXT replay evidence exists")
    claim_limits = spec.get("claim_limit")
    require_contains(PMXT_SOURCE_PROOF_SPEC, "claim_limit", claim_limits, "No dynamic tick-size replay claim until NT-native timed instrument-epoch replay", findings)


def check_bte_status(bte_status: dict[str, Any], findings: list[str]) -> None:
    require_equal(BTE_022_STATUS, "task_id", bte_status.get("task_id"), "BACKTESTING_ENGINE-022", findings)
    require_equal(BTE_022_STATUS, "status", bte_status.get("status"), "open_pmxt_one_off_current_artifact_proven_broad_backfill_blocked", findings)
    require_equal(BTE_022_STATUS, "bte_022_can_close", bte_status.get("bte_022_can_close"), False, findings)
    dynamic = bte_status.get("dynamic_tick_size_replay_guardrail_status")
    if not isinstance(dynamic, dict):
        findings.append(f"{BTE_022_STATUS}: dynamic_tick_size_replay_guardrail_status must be an object")
    else:
        require_equal(BTE_022_STATUS, "dynamic_tick_size_replay_guardrail_status.status", dynamic.get("status"), BTE_DYNAMIC_GUARD_STATUS, findings)
        require_list_equal(BTE_022_STATUS, "dynamic_tick_size_replay_guardrail_status.evidence", dynamic.get("evidence"), BTE_DYNAMIC_GUARD_EVIDENCE, findings)
        require_list_equal(BTE_022_STATUS, "dynamic_tick_size_replay_guardrail_status.claim_limits", dynamic.get("claim_limits"), BTE_DYNAMIC_GUARD_CLAIM_LIMITS, findings)
    blockers = bte_status.get("remaining_blockers")
    if not isinstance(blockers, list):
        findings.append(f"{BTE_022_STATUS}: remaining_blockers must be a list")
        return
    blocker_names = tuple(item.get("blocker") for item in blockers if isinstance(item, dict))
    if len(blocker_names) != len(blockers):
        findings.append(f"{BTE_022_STATUS}: remaining_blockers entries must be objects with blocker names")
    require_equal(BTE_022_STATUS, "remaining_blockers.blocker_names", blocker_names, BTE_REMAINING_BLOCKERS, findings)
    dynamic_blocker = next((item for item in blockers if isinstance(item, dict) and item.get("blocker") == "dynamic_tick_size_replay_unproven"), None)
    if not isinstance(dynamic_blocker, dict):
        findings.append(f"{BTE_022_STATUS}: remaining_blockers must include dynamic_tick_size_replay_unproven")
    else:
        require_contains(BTE_022_STATUS, "remaining_blockers.dynamic_tick_size_replay_unproven.required_evidence", dynamic_blocker.get("required_evidence"), "A separate NT BacktestNode/catalog proof", findings)
        require_contains(BTE_022_STATUS, "remaining_blockers.dynamic_tick_size_replay_unproven.required_evidence", dynamic_blocker.get("required_evidence"), "does not prove dynamic tick-size replay", findings)
        for forbidden in SOURCE_PROOF_PENDING_FORBIDDEN_L2_REFS:
            require_absent(BTE_022_STATUS, "remaining_blockers.dynamic_tick_size_replay_unproven.required_evidence", dynamic_blocker.get("required_evidence"), forbidden, findings)
    require_contains(BTE_022_STATUS, "next_required_evidence", bte_status.get("next_required_evidence"), "Separate dynamic tick-size replay proof", findings)


def check_justfile(root: Path, findings: list[str]) -> None:
    justfile = read_text(root, JUSTFILE, findings)
    for recipe in JUSTFILE_RECIPES:
        recipe_commands = just_recipe_commands(justfile, recipe)
        for command in JUSTFILE_COMMANDS:
            if command not in recipe_commands:
                findings.append(f"{JUSTFILE}: {recipe} must run {command}")


def scan_root(root: Path) -> list[str]:
    findings: list[str] = []
    status = read_json(root, PMXT_DYNAMIC_STATUS, findings)
    tick_status = read_json(root, PMXT_TICK_STATUS, findings)
    timed_audit = read_json(root, PMXT_TIMED_AUDIT, findings)
    first_policy = read_json(root, PMXT_FIRST_UNIVERSE_POLICY, findings)
    source_proof = read_toml(root, PMXT_SOURCE_PROOF_SPEC, findings)
    bte_status = read_json(root, BTE_022_STATUS, findings)

    check_status(status, root, findings)
    check_tick_status(tick_status, findings)
    check_timed_audit(timed_audit, findings)
    check_first_universe_policy(first_policy, findings)
    check_source_proof(source_proof, findings)
    check_bte_status(bte_status, findings)
    check_justfile(root, findings)
    return findings


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--root", type=Path, default=REPO_ROOT)
    args = parser.parse_args(argv)

    findings = scan_root(args.root)
    if findings:
        print("FAIL: BTE-022 PMXT dynamic tick-size status violations:", file=sys.stderr)
        for finding in findings:
            print(f"  {finding}", file=sys.stderr)
        return 1
    print("OK: BTE-022 PMXT dynamic tick-size status passed.")
    return 0


if __name__ == "__main__":
    import lane_governor

    lane_governor.acquire()
    raise SystemExit(main())
