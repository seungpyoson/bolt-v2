#!/usr/bin/env python3
"""Self-tests for the BTE-022 PMXT dynamic tick-size verifier."""

from __future__ import annotations

import importlib.util
import json
import subprocess
import sys
import tempfile
from pathlib import Path


REPO_ROOT = Path(__file__).resolve().parent.parent
SCRIPT = REPO_ROOT / "scripts" / "verify_bte_022_pmxt_dynamic_tick_size.py"
FIRST_SELECTION_KEY = "_".join(("selected", "first", "proof", "policy"))
FIRST_SELECTION_PREDICATE_REF = f"{FIRST_SELECTION_KEY}.selector_predicate"


def load_verifier():
    spec = importlib.util.spec_from_file_location("verify_bte_022_pmxt_dynamic_tick_size", SCRIPT)
    if spec is None or spec.loader is None:
        raise AssertionError(f"failed to load {SCRIPT}")
    module = importlib.util.module_from_spec(spec)
    sys.modules[spec.name] = module
    spec.loader.exec_module(module)
    return module


def write_file(root: Path, rel: str, text: str) -> None:
    path = root / rel
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(text, encoding="utf-8")


def json_text(data: dict) -> str:
    return json.dumps(data, indent=2) + "\n"


def source_proof_text(*, status: str = "pending", timed_ref: bool = False) -> str:
    timed_line = (
        'timed_instrument_epoch_replay_ref = "repo://specs/023-nt-research-analytics-platform/reference/timed-replay.json"\n'
        if timed_ref
        else ""
    )
    return f"""proof_set_id = "source-universe-source-proofs-pmxt-polymarket-v2-current"
output_dir = "specs/023-nt-research-analytics-platform/reference/backfill-source-proofs/pmxt-polymarket-v2-current"
source_bindings_path = "specs/023-nt-research-analytics-platform/reference/backfill-source-bindings.v1.toml"
venue = "polymarket"
table_family = "order_book_snapshot_deltas"
manifest_table_family = "order_book_snapshot_deltas"
status = "{status}"
source_candidate_class = "official_free"
source_selection_status = "PENDING_MORE_PROOF"
usage_scope = "one_off_backfill_data"
fidelity_class = "L2_REPLAY"

[l2_replay_evidence]
order_book_delta_ref = "repo://specs/023-nt-research-analytics-platform/reference/source-proof-nt-mapping-inspection.polymarket-pmxt-v2-orderbook.2026-06-08.json"
sufficient_snapshot_cadence_ref = "repo://specs/023-nt-research-analytics-platform/reference/source-proof-sample-inspection.polymarket-pmxt-v2-orderbook.2026-06-08.json"
{timed_line}
[[claim_limit]]
id = "pmxt-source-proof-claim-limit-002"
severity = "blocking"
claim = "No dynamic tick-size replay claim until NT-native timed instrument-epoch replay or a source-proof-bound no-tick-size-change universe is accepted."
reason = "The PMXT source includes tick_size_change fields and the current broad replay policy remains unaccepted."
evidence_ref = "repo://specs/023-nt-research-analytics-platform/reference/source-proof-pmxt-polymarket-tick-size-change-status.2026-06-08.json"
"""


def one_off_fixture(
    *,
    usage_scope: str = "one_off_backfill_data",
    include_no_tick_ref: bool = True,
    timed_ref: bool = False,
) -> dict:
    l2_replay_evidence = {
        "order_book_delta_ref": "repo://specs/023-nt-research-analytics-platform/reference/source-proof-nt-mapping-inspection.polymarket-pmxt-v2-orderbook.2026-06-08.json",
        "sufficient_snapshot_cadence_ref": "repo://specs/023-nt-research-analytics-platform/reference/source-proof-sample-inspection.polymarket-pmxt-v2-orderbook.2026-06-08.json",
    }
    if include_no_tick_ref:
        l2_replay_evidence["no_tick_size_change_universe_ref"] = (
            "repo://specs/023-nt-research-analytics-platform/reference/"
            "source-proof-pmxt-polymarket-first-proof-universe-policy.2026-06-08.json "
            f"{FIRST_SELECTION_PREDICATE_REF}"
        )
    if timed_ref:
        l2_replay_evidence["timed_instrument_epoch_replay_ref"] = (
            "repo://specs/023-nt-research-analytics-platform/reference/timed-replay.json"
        )
    return {
        "status": "pending",
        "source_selection_status": "PENDING_MORE_PROOF",
        "usage_scope": usage_scope,
        "nt_mapping_status": "accepted",
        "l2_replay_evidence": l2_replay_evidence,
        "claim_limits": [
            {
                "id": "binary-option-pmxt-source-proof-claim-limit-005",
                "claim": "No dynamic tick-size replay claim until NT-native instrument-epoch replay is proven.",
            }
        ],
        "required_checks": {
            "coverage": {"outcome": "pending"},
            "retention_freshness": {"outcome": "pending"},
            "completeness": {"outcome": "pending"},
            "cost": {"outcome": "pending"},
            "storage": {"outcome": "pending"},
        },
    }


def tick_status(*, supports_dynamic: bool = False) -> dict:
    return {
        "schema_version": "source-proof-pmxt-polymarket-tick-size-change-status.v1",
        "task_id": "BACKTESTING_ENGINE-022",
        "source_binding": "polymarket-parquet-archive-index",
        "status": "open_standard_backtestnode_catalog_replay_does_not_support_dynamic_instrument_epoch",
        "pinned_nt_revision": "6e059dcbb59ac1e582132fc431a581936c216c3c",
        "scope": {
            "standard_backtestnode_catalog_replay_supports_timed_instrument_any": False,
            "not_implementation": True,
        },
        "pmxt_sample_evidence": {
            "event_type": "tick_size_change",
            "row_count": 419,
            "distinct_assets": 343,
        },
        "bte_manifest_surface_evidence": [
            "InstrumentStatus and InstrumentClose are Data enum replay items, not InstrumentAny instrument-definition updates, so this does not close tick_size_change epoch replay."
        ],
        "current_decision": {
            "standard_backtestnode_catalog_replay_supports_dynamic_instrument_any": supports_dynamic,
            "tick_size_change_policy_can_close": False,
            "first_proof_exclusion_policy_can_close": True,
            "bte_022_can_close": False,
            "broad_backfill_allowed": False,
            "next_required_evidence": "For full L2 acceptance, implement a timed InstrumentAny replay mechanism.",
        },
    }


def timed_audit() -> dict:
    return {
        "schema_version": 1,
        "artifact": "source-proof-pmxt-polymarket-timed-instrument-replay-nt-audit",
        "task": "BACKTESTING_ENGINE-022",
        "nt_revision": "6e059dcbb59ac1e582132fc431a581936c216c3c",
        "answer": "No. Pinned NT can store multiple InstrumentAny snapshots, but standard BacktestNode cannot replay them as timed data.",
        "decisions": {
            "standard_backtestnode_catalog_replay_has_timed_instrument_any": False,
            "instrument_status_or_close_can_substitute_for_tick_size_change": False,
            "pmxt_full_l2_with_tick_size_change_can_be_accepted_now": False,
            "bounded_no_tick_size_change_pmxt_first_proof_can_continue": True,
            "bte_022_can_close": False,
        },
        "rejected_paths": [{"path": "ignore_tick_size_change_rows"}],
        "next_required_evidence": ["For full PMXT L2 acceptance, implement a timed InstrumentAny replay mechanism."],
    }


def first_universe_policy(*, can_close: bool = False) -> dict:
    return {
        "schema_version": "source-proof-pmxt-polymarket-first-proof-universe-policy.v1",
        "task_id": "BACKTESTING_ENGINE-022",
        "source_binding": "polymarket-parquet-archive-index",
        "status": "first_proof_exclusion_policy_selected_tdd_proven_for_selector_artifacts",
        "claim_limits": ["Does not prove dynamic tick-size replay."],
        FIRST_SELECTION_KEY: {
            "selector_predicate": ["tick_size_change_rows == 0"],
            "required_manifest_bindings": ["excluded_tick_change_event_count"],
        },
        "pmxt_one_object_evidence": {
            "instrument_universe_counts": {"assets_with_tick_change": 343},
            "eligible_first_proof_assets": {"eligible_assets": 823},
        },
        "current_decision": {
            "tick_size_dynamic_replay_can_close": can_close,
            "bte_022_can_close": False,
            "broad_backfill_allowed": False,
        },
    }


def selected_selector_report(module, *, excluded_event_families: list[str] | None = None) -> dict:
    return {
        "schema_version": "first-proof-selector-report.v1",
        "selector_id": "pmxt-polymarket-first-proof-2026-05-20T22-gamma-backed",
        "status": "selected",
        "selection": {
            "required_event_families": list(module.SELECTED_REQUIRED_EVENT_FAMILIES),
            "excluded_event_families": list(module.SELECTED_EXCLUDED_EVENT_FAMILIES)
            if excluded_event_families is None
            else excluded_event_families,
            "candidate_asset_ids": [module.SELECTED_ASSET_ID],
            "row_budget": 1000,
            "max_selected_assets": 1,
        },
        "event_count_ledger_hash": "985808244f540656dc5021703f2a2d9ae9a93305ebb5afe0b05f45a58027f00a",
        "total_assets": 71593,
        "eligible_assets": 6,
        "selected_assets": [
            {
                "asset_id": module.SELECTED_ASSET_ID,
                "replay_rows": 5,
                "source_row_groups": list(module.SELECTED_ASSET_SOURCE_ROW_GROUPS),
            }
        ],
        "selected_asset_ids_hash": module.SELECTED_ASSET_IDS_HASH,
        "excluded_event_asset_count": 0,
        "excluded_event_row_count": 0,
        "blocking_issues": [],
    }


def selected_source_report(module, selector_hash: str, *, usage_scope: str = "one_off_backfill_data") -> dict:
    return {
        "schema_version": "selected-source-slice-report.v1",
        "source_parquet_sha256": module.SELECTED_SOURCE_PARQUET_SHA256,
        "selector_report_sha256": selector_hash,
        "usage_scope": usage_scope,
        "source_rows": 64877467,
        "source_row_groups": 62,
        "projected_row_groups": 1,
        "selected_rows": 5,
        "selected_asset_count": 1,
        "selected_asset_ids_hash": module.SELECTED_ASSET_IDS_HASH,
        "output_parquet_sha256": module.SELECTED_OUTPUT_PARQUET_SHA256,
    }


def selected_source_slice(module, selector_hash: str) -> dict:
    return {
        "schema_version": "source-proof-pmxt-selected-source-slice.v3",
        "usage_scope": "one_off_backfill_data",
        "source": {
            "sha256": module.SELECTED_SOURCE_PARQUET_SHA256,
            "source_rows": 64877467,
            "source_row_groups": 62,
        },
        "metadata_candidate_probe": {
            "selected_asset_id": module.SELECTED_ASSET_ID,
            "gamma_backed_candidate_count": 6,
        },
        "selector": {
            "report_sha256": selector_hash,
            "status": "selected",
            "eligible_assets": 6,
            "selected_asset_count": 1,
            "selected_asset_ids_hash": module.SELECTED_ASSET_IDS_HASH,
            "selected_assets": [
                {
                    "asset_id": module.SELECTED_ASSET_ID,
                    "replay_rows": 5,
                    "source_row_groups": list(module.SELECTED_ASSET_SOURCE_ROW_GROUPS),
                }
            ],
            "selection": {
                "required_event_families": list(module.SELECTED_REQUIRED_EVENT_FAMILIES),
                "excluded_event_families": list(module.SELECTED_EXCLUDED_EVENT_FAMILIES),
            },
        },
        "selected_source_slice": {
            "usage_scope": "one_off_backfill_data",
            "output_parquet_sha256": module.SELECTED_OUTPUT_PARQUET_SHA256,
            "source_rows": 64877467,
            "source_row_groups": 62,
            "projected_row_groups": 1,
            "selected_rows": 5,
            "selected_asset_count": 1,
            "event_types": list(module.SELECTED_EVENT_TYPES),
            "event_type_rows": {
                "book": 1,
                "last_trade_price": 1,
                "price_change": 3,
            },
        },
    }


def selected_artifact_status(module, root: Path) -> dict:
    return {
        "status": module.SELECTED_SOURCE_ARTIFACT_STATUS,
        "selector_report": {
            "path": str(module.PMXT_SELECTED_SELECTOR_REPORT),
            "sha256": module.path_sha256(root, module.PMXT_SELECTED_SELECTOR_REPORT, []),
        },
        "selected_source_report": {
            "path": str(module.PMXT_SELECTED_SOURCE_REPORT),
            "sha256": module.path_sha256(root, module.PMXT_SELECTED_SOURCE_REPORT, []),
        },
        "selected_source_slice_status": {
            "path": str(module.PMXT_SELECTED_SOURCE_SLICE_STATUS),
            "sha256": module.path_sha256(root, module.PMXT_SELECTED_SOURCE_SLICE_STATUS, []),
        },
        "usage_scope": "one_off_backfill_data",
        "selected_asset_count": 1,
        "selected_rows": 5,
        "source_row_groups": 62,
        "projected_row_groups": 1,
        "selected_asset_id": module.SELECTED_ASSET_ID,
        "selected_asset_ids_hash": module.SELECTED_ASSET_IDS_HASH,
        "selected_asset_source_row_groups": list(module.SELECTED_ASSET_SOURCE_ROW_GROUPS),
        "event_types": list(module.SELECTED_EVENT_TYPES),
        "excluded_event_families": list(module.SELECTED_EXCLUDED_EVENT_FAMILIES),
        "dynamic_tick_size_replay_proven": False,
        "broad_backfill_allowed": False,
        "bte_022_can_close": False,
    }


def bte_status(module, *, include_guard: bool = True) -> dict:
    blockers = []
    for blocker in module.BTE_REMAINING_BLOCKERS:
        required_evidence = f"{blocker} remains required before BTE-022 can close."
        if blocker == "dynamic_tick_size_replay_unproven":
            required_evidence = "A separate NT BacktestNode/catalog proof that does not prove dynamic tick-size replay."
        blockers.append({"blocker": blocker, "required_evidence": required_evidence})
    status = {
        "task_id": "BACKTESTING_ENGINE-022",
        "status": "open_pmxt_one_off_current_artifact_proven_broad_backfill_blocked",
        "bte_022_can_close": False,
        "remaining_blockers": blockers,
        "next_required_evidence": ["Separate dynamic tick-size replay proof before full PMXT Polymarket L2 acceptance."],
    }
    if include_guard:
        status["dynamic_tick_size_replay_guardrail_status"] = {
            "status": module.BTE_DYNAMIC_GUARD_STATUS,
            "evidence": list(module.BTE_DYNAMIC_GUARD_EVIDENCE),
            "claim_limits": list(module.BTE_DYNAMIC_GUARD_CLAIM_LIMITS),
        }
    return status


def status_artifact(
    module,
    root: Path,
    *,
    bad_hash_key: str | None = None,
    status_overrides: dict | None = None,
) -> dict:
    hashes = {}
    for path_tuple, target in module.STATUS_HASH_TARGETS:
        key = path_tuple[-1]
        digest = module.path_sha256(root, target, [])
        hashes[key] = {"path": str(target), "sha256": "bad" if key == bad_hash_key else digest}
    status = {
        "schema_version": "source-proof-pmxt-dynamic-tick-size-replay-status.v1",
        "task_id": "BACKTESTING_ENGINE-022",
        "source_binding": "polymarket-parquet-archive-index",
        "observed_at_utc": "2026-06-16T00:00:00Z",
        "dynamic_tick_size_replay_status": "blocked_standard_backtestnode_no_timed_instrument_any",
        "standard_backtestnode_catalog_replay_supports_dynamic_instrument_any": False,
        "timed_instrument_epoch_replay_accepted": False,
        "bounded_no_tick_size_change_first_proof_allowed": True,
        "pmxt_full_l2_with_tick_size_change_can_be_accepted_now": False,
        "bte_022_can_close": False,
        "bounded_selected_source_artifact_status": selected_artifact_status(module, root),
        "committed_input_hashes": hashes,
        "guard_verification": {
            "script": "repo://scripts/verify_bte_022_pmxt_dynamic_tick_size.py",
            "self_test": "repo://scripts/test_verify_bte_022_pmxt_dynamic_tick_size.py",
            "source_fence_static": True,
        },
        "claim_limits": list(module.CLAIM_LIMITS),
        "remaining_blockers": list(module.REMAINING_BLOCKERS),
    }
    if status_overrides:
        status.update(status_overrides)
    return status


def justfile_text(*, include_dynamic: bool = True) -> str:
    dynamic = (
        "verify-bte-022-pmxt-dynamic-tick-size: check-workspace\n"
        "    python3 scripts/test_verify_bte_022_pmxt_dynamic_tick_size.py\n"
        "    python3 scripts/verify_bte_022_pmxt_dynamic_tick_size.py\n\n"
    )
    source_fence_dynamic = "    python3 scripts/run_fences.py\n"
    return (
        (dynamic if include_dynamic else "")
        + "source-fence-static-inner: check-workspace\n"
        + ("    python3 scripts/verify_bte_022_pmxt_dynamic_tick_size.py\n" if not include_dynamic else source_fence_dynamic)
    )


def populate(root: Path, module, **overrides) -> None:
    write_file(root, str(module.PMXT_TICK_STATUS), json_text(overrides.get("tick", tick_status())))
    write_file(root, str(module.PMXT_TIMED_AUDIT), json_text(overrides.get("audit", timed_audit())))
    write_file(root, str(module.PMXT_FIRST_UNIVERSE_POLICY), json_text(overrides.get("first", first_universe_policy())))
    write_file(root, str(module.PMXT_SOURCE_PROOF_SPEC), overrides.get("source_proof", source_proof_text()))
    write_file(root, str(module.PMXT_SOURCE_PROOF_FIXTURE), json_text(overrides.get("source_fixture", one_off_fixture())))
    write_file(root, str(module.BTE_022_STATUS), json_text(overrides.get("bte", bte_status(module))))
    write_file(root, str(module.PMXT_SELECTED_SELECTOR_REPORT), json_text(overrides.get("selected_selector", selected_selector_report(module))))
    selector_hash = module.path_sha256(root, module.PMXT_SELECTED_SELECTOR_REPORT, [])
    write_file(
        root,
        str(module.PMXT_SELECTED_SOURCE_REPORT),
        json_text(overrides.get("selected_report", selected_source_report(module, selector_hash))),
    )
    write_file(
        root,
        str(module.PMXT_SELECTED_SOURCE_SLICE_STATUS),
        json_text(overrides.get("selected_slice", selected_source_slice(module, selector_hash))),
    )
    write_file(
        root,
        str(module.PMXT_DYNAMIC_STATUS),
        json_text(
            status_artifact(
                module,
                root,
                bad_hash_key=overrides.get("bad_hash_key", "tick_size_change_status" if overrides.get("bad_hash", False) else None),
                status_overrides=overrides.get("status_overrides"),
            )
        ),
    )
    write_file(root, "justfile", overrides.get("justfile", justfile_text()))


def assert_clean_fixture_passes() -> None:
    module = load_verifier()
    with tempfile.TemporaryDirectory() as tmp:
        root = Path(tmp)
        populate(root, module)
        findings = module.scan_root(root)
        if findings:
            raise AssertionError(f"expected clean fixture, got {findings}")


def assert_dynamic_tick_overclaim_is_a_finding() -> None:
    module = load_verifier()
    with tempfile.TemporaryDirectory() as tmp:
        root = Path(tmp)
        populate(root, module, tick=tick_status(supports_dynamic=True))
        findings = module.scan_root(root)
        if not any("standard_backtestnode_catalog_replay_supports_dynamic_instrument_any" in finding for finding in findings):
            raise AssertionError(f"expected dynamic replay overclaim finding, got {findings}")


def assert_timed_ref_in_pending_source_proof_is_a_finding() -> None:
    module = load_verifier()
    with tempfile.TemporaryDirectory() as tmp:
        root = Path(tmp)
        populate(root, module, source_proof=source_proof_text(timed_ref=True))
        findings = module.scan_root(root)
        if not any("timed_instrument_epoch_replay_ref" in finding for finding in findings):
            raise AssertionError(f"expected timed replay source-proof finding, got {findings}")


def assert_bounded_fixture_scope_drift_is_a_finding() -> None:
    module = load_verifier()
    with tempfile.TemporaryDirectory() as tmp:
        root = Path(tmp)
        populate(root, module, source_fixture=one_off_fixture(usage_scope="canonical_backfill_input"))
        findings = module.scan_root(root)
        if not any("usage_scope" in finding for finding in findings):
            raise AssertionError(f"expected one-off fixture usage-scope finding, got {findings}")


def assert_bounded_fixture_timed_ref_is_a_finding() -> None:
    module = load_verifier()
    with tempfile.TemporaryDirectory() as tmp:
        root = Path(tmp)
        populate(root, module, source_fixture=one_off_fixture(timed_ref=True))
        findings = module.scan_root(root)
        if not any("timed_instrument_epoch_replay_ref" in finding for finding in findings):
            raise AssertionError(f"expected one-off fixture timed replay finding, got {findings}")


def assert_bounded_fixture_missing_no_tick_policy_is_a_finding() -> None:
    module = load_verifier()
    with tempfile.TemporaryDirectory() as tmp:
        root = Path(tmp)
        populate(root, module, source_fixture=one_off_fixture(include_no_tick_ref=False))
        findings = module.scan_root(root)
        if not any("no_tick_size_change_universe_ref" in finding for finding in findings):
            raise AssertionError(f"expected one-off fixture no-tick policy finding, got {findings}")


def assert_selected_source_status_overclaim_is_a_finding() -> None:
    module = load_verifier()
    with tempfile.TemporaryDirectory() as tmp:
        root = Path(tmp)
        populate(root, module)
        status_path = root / module.PMXT_DYNAMIC_STATUS
        status = json.loads(status_path.read_text(encoding="utf-8"))
        status["bounded_selected_source_artifact_status"]["dynamic_tick_size_replay_proven"] = True
        status_path.write_text(json_text(status), encoding="utf-8")
        findings = module.scan_root(root)
        if not any("dynamic_tick_size_replay_proven" in finding for finding in findings):
            raise AssertionError(f"expected selected-source overclaim finding, got {findings}")


def assert_selected_source_missing_tick_exclusion_is_a_finding() -> None:
    module = load_verifier()
    with tempfile.TemporaryDirectory() as tmp:
        root = Path(tmp)
        populate(root, module, selected_selector=selected_selector_report(module, excluded_event_families=[]))
        findings = module.scan_root(root)
        if not any("selection.excluded_event_families" in finding for finding in findings):
            raise AssertionError(f"expected selected-source tick exclusion finding, got {findings}")


def assert_selected_source_selector_hash_drift_is_a_finding() -> None:
    module = load_verifier()
    with tempfile.TemporaryDirectory() as tmp:
        root = Path(tmp)
        populate(root, module, selected_report=selected_source_report(module, "bad"))
        findings = module.scan_root(root)
        if not any("selector_report_sha256" in finding for finding in findings):
            raise AssertionError(f"expected selected-source selector hash finding, got {findings}")


def assert_selected_source_event_row_total_drift_is_a_finding() -> None:
    module = load_verifier()
    with tempfile.TemporaryDirectory() as tmp:
        root = Path(tmp)
        populate(root, module)
        slice_path = root / module.PMXT_SELECTED_SOURCE_SLICE_STATUS
        selected_slice = json.loads(slice_path.read_text(encoding="utf-8"))
        selected_slice["selected_source_slice"]["event_type_rows"]["price_change"] = 4
        slice_path.write_text(json_text(selected_slice), encoding="utf-8")
        findings = module.scan_root(root)
        if not any("selected_source_slice.event_type_rows.total" in finding for finding in findings):
            raise AssertionError(f"expected selected-source event row total finding, got {findings}")


def assert_selected_source_status_hash_drift_is_a_finding() -> None:
    module = load_verifier()
    with tempfile.TemporaryDirectory() as tmp:
        root = Path(tmp)
        populate(root, module)
        status_path = root / module.PMXT_DYNAMIC_STATUS
        status = json.loads(status_path.read_text(encoding="utf-8"))
        status["bounded_selected_source_artifact_status"]["selected_source_report"]["sha256"] = "bad"
        status_path.write_text(json_text(status), encoding="utf-8")
        findings = module.scan_root(root)
        if not any("bounded_selected_source_artifact_status.selected_source_report.sha256" in finding for finding in findings):
            raise AssertionError(f"expected selected-source status hash drift finding, got {findings}")


def assert_selected_source_status_path_drift_is_a_finding() -> None:
    module = load_verifier()
    with tempfile.TemporaryDirectory() as tmp:
        root = Path(tmp)
        populate(root, module)
        status_path = root / module.PMXT_DYNAMIC_STATUS
        status = json.loads(status_path.read_text(encoding="utf-8"))
        status["bounded_selected_source_artifact_status"]["selected_source_report"]["path"] = "wrong/path.json"
        status_path.write_text(json_text(status), encoding="utf-8")
        findings = module.scan_root(root)
        if not any("bounded_selected_source_artifact_status.selected_source_report.path" in finding for finding in findings):
            raise AssertionError(f"expected selected-source status path drift finding, got {findings}")


def assert_status_hash_path_drift_is_a_finding() -> None:
    module = load_verifier()
    with tempfile.TemporaryDirectory() as tmp:
        root = Path(tmp)
        populate(root, module)
        status_path = root / module.PMXT_DYNAMIC_STATUS
        status = json.loads(status_path.read_text(encoding="utf-8"))
        status["committed_input_hashes"]["selected_source_report"]["path"] = "wrong/path.json"
        status_path.write_text(json_text(status), encoding="utf-8")
        findings = module.scan_root(root)
        if not any("committed_input_hashes.selected_source_report.path" in finding for finding in findings):
            raise AssertionError(f"expected committed-input path drift finding, got {findings}")


def assert_pending_source_ref_in_bte_narrative_is_a_finding() -> None:
    module = load_verifier()
    for forbidden_ref in module.SOURCE_PROOF_PENDING_FORBIDDEN_L2_REFS:
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            bte = bte_status(module)
            dynamic = next(item for item in bte["remaining_blockers"] if item["blocker"] == "dynamic_tick_size_replay_unproven")
            dynamic["required_evidence"] += f" {forbidden_ref}"
            populate(root, module, bte=bte)
            findings = module.scan_root(root)
            if not any(forbidden_ref in finding for finding in findings):
                raise AssertionError(f"expected forbidden BTE narrative source-proof ref finding for {forbidden_ref}, got {findings}")


def assert_missing_bte_blocker_is_a_finding() -> None:
    module = load_verifier()
    with tempfile.TemporaryDirectory() as tmp:
        root = Path(tmp)
        bte = bte_status(module)
        bte["remaining_blockers"] = [
            item for item in bte["remaining_blockers"]
            if item["blocker"] != "expanded_tranche_coverage_and_cost_unproven"
        ]
        populate(root, module, bte=bte)
        findings = module.scan_root(root)
        if not any("remaining_blockers.blocker_names" in finding for finding in findings):
            raise AssertionError(f"expected missing BTE blocker finding, got {findings}")


def assert_missing_bte_guard_is_a_finding() -> None:
    module = load_verifier()
    with tempfile.TemporaryDirectory() as tmp:
        root = Path(tmp)
        populate(root, module, bte=bte_status(module, include_guard=False))
        findings = module.scan_root(root)
        if not any("dynamic_tick_size_replay_guardrail_status" in finding for finding in findings):
            raise AssertionError(f"expected missing BTE guard finding, got {findings}")


def assert_bte_guard_content_drift_is_a_finding() -> None:
    module = load_verifier()
    cases = (
        ("status", "accepted_dynamic_tick_size_replay"),
        ("evidence", ["drift"]),
        ("claim_limits", ["drift"]),
    )
    for field, value in cases:
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            bte = bte_status(module)
            bte["dynamic_tick_size_replay_guardrail_status"][field] = value
            populate(root, module, bte=bte)
            findings = module.scan_root(root)
            if not any(f"dynamic_tick_size_replay_guardrail_status.{field}" in finding for finding in findings):
                raise AssertionError(f"expected BTE guard {field} drift finding, got {findings}")


def assert_status_hash_drift_is_a_finding() -> None:
    module = load_verifier()
    for path_tuple, _target in module.STATUS_HASH_TARGETS:
        key = path_tuple[-1]
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            populate(root, module, bad_hash_key=key)
            findings = module.scan_root(root)
            if not any(f"{key}.sha256" in finding for finding in findings):
                raise AssertionError(f"expected status hash drift finding for {key}, got {findings}")


def assert_dynamic_status_flag_drift_is_a_finding() -> None:
    module = load_verifier()
    cases = (
        ("dynamic_tick_size_replay_status", "accepted_dynamic_tick_size_replay"),
        ("standard_backtestnode_catalog_replay_supports_dynamic_instrument_any", True),
        ("timed_instrument_epoch_replay_accepted", True),
        ("bounded_no_tick_size_change_first_proof_allowed", False),
        ("pmxt_full_l2_with_tick_size_change_can_be_accepted_now", True),
        ("bte_022_can_close", True),
    )
    for field, value in cases:
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            populate(root, module, status_overrides={field: value})
            findings = module.scan_root(root)
            if not any(field in finding for finding in findings):
                raise AssertionError(f"expected dynamic status drift finding for {field}, got {findings}")


def assert_dynamic_status_observed_at_format_is_a_finding() -> None:
    module = load_verifier()
    with tempfile.TemporaryDirectory() as tmp:
        root = Path(tmp)
        populate(root, module, status_overrides={"observed_at_utc": "2026-06-16 08:05:49"})
        findings = module.scan_root(root)
        if not any("observed_at_utc" in finding for finding in findings):
            raise AssertionError(f"expected observed_at_utc format finding, got {findings}")


def assert_justfile_wiring_is_a_finding() -> None:
    module = load_verifier()
    with tempfile.TemporaryDirectory() as tmp:
        root = Path(tmp)
        populate(root, module, justfile=justfile_text(include_dynamic=False))
        findings = module.scan_root(root)
        if not any("source-fence-static-inner" in finding for finding in findings):
            raise AssertionError(f"expected source-fence wiring finding, got {findings}")


def assert_script_cli_fails_closed_on_fixture_drift() -> None:
    module = load_verifier()
    with tempfile.TemporaryDirectory() as tmp:
        root = Path(tmp)
        populate(root, module, source_proof=source_proof_text(status="accepted"))
        result = subprocess.run(
            [sys.executable, str(SCRIPT), "--root", str(root)],
            text=True,
            stdout=subprocess.PIPE,
            stderr=subprocess.PIPE,
            check=False,
        )
        if result.returncode == 0:
            raise AssertionError("script should fail when PMXT source proof is accepted")
        if "status must be 'pending'" not in result.stderr:
            raise AssertionError(result.stderr)


def main() -> int:
    tests = (
        assert_clean_fixture_passes,
        assert_dynamic_tick_overclaim_is_a_finding,
        assert_timed_ref_in_pending_source_proof_is_a_finding,
        assert_bounded_fixture_scope_drift_is_a_finding,
        assert_bounded_fixture_timed_ref_is_a_finding,
        assert_bounded_fixture_missing_no_tick_policy_is_a_finding,
        assert_selected_source_status_overclaim_is_a_finding,
        assert_selected_source_missing_tick_exclusion_is_a_finding,
        assert_selected_source_selector_hash_drift_is_a_finding,
        assert_selected_source_event_row_total_drift_is_a_finding,
        assert_selected_source_status_hash_drift_is_a_finding,
        assert_selected_source_status_path_drift_is_a_finding,
        assert_status_hash_path_drift_is_a_finding,
        assert_pending_source_ref_in_bte_narrative_is_a_finding,
        assert_missing_bte_blocker_is_a_finding,
        assert_missing_bte_guard_is_a_finding,
        assert_bte_guard_content_drift_is_a_finding,
        assert_status_hash_drift_is_a_finding,
        assert_dynamic_status_flag_drift_is_a_finding,
        assert_dynamic_status_observed_at_format_is_a_finding,
        assert_justfile_wiring_is_a_finding,
        assert_script_cli_fails_closed_on_fixture_drift,
    )
    for test in tests:
        test()
    print("OK: BTE-022 PMXT dynamic tick-size verifier self-tests passed.")
    return 0


if __name__ == "__main__":
    import lane_governor

    lane_governor.acquire()
    raise SystemExit(main())
