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


def status_artifact(module, root: Path, *, bad_hash: bool = False, status_overrides: dict | None = None) -> dict:
    hashes = {}
    for path_tuple, target in module.STATUS_HASH_TARGETS:
        key = path_tuple[-1]
        digest = module.path_sha256(root, target, [])
        hashes[key] = {"path": str(target), "sha256": "bad" if bad_hash and key == "tick_size_change_status" else digest}
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
    source_fence_dynamic = (
        "    python3 scripts/test_verify_bte_022_pmxt_dynamic_tick_size.py\n"
        "    python3 scripts/verify_bte_022_pmxt_dynamic_tick_size.py\n"
    )
    return (
        (dynamic if include_dynamic else "")
        + "source-fence-static: check-workspace\n"
        + "    python3 scripts/test_verify_bte_022_pmxt_durable_source.py\n"
        + ("    python3 scripts/verify_bte_022_pmxt_dynamic_tick_size.py\n" if not include_dynamic else source_fence_dynamic)
    )


def populate(root: Path, module, **overrides) -> None:
    write_file(root, str(module.PMXT_TICK_STATUS), json_text(overrides.get("tick", tick_status())))
    write_file(root, str(module.PMXT_TIMED_AUDIT), json_text(overrides.get("audit", timed_audit())))
    write_file(root, str(module.PMXT_FIRST_UNIVERSE_POLICY), json_text(overrides.get("first", first_universe_policy())))
    write_file(root, str(module.PMXT_SOURCE_PROOF_SPEC), overrides.get("source_proof", source_proof_text()))
    write_file(root, str(module.BTE_022_STATUS), json_text(overrides.get("bte", bte_status(module))))
    write_file(
        root,
        str(module.PMXT_DYNAMIC_STATUS),
        json_text(status_artifact(module, root, bad_hash=overrides.get("bad_hash", False), status_overrides=overrides.get("status_overrides"))),
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


def assert_status_hash_drift_is_a_finding() -> None:
    module = load_verifier()
    with tempfile.TemporaryDirectory() as tmp:
        root = Path(tmp)
        populate(root, module, bad_hash=True)
        findings = module.scan_root(root)
        if not any("tick_size_change_status.sha256" in finding for finding in findings):
            raise AssertionError(f"expected status hash drift finding, got {findings}")


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


def assert_justfile_wiring_is_a_finding() -> None:
    module = load_verifier()
    with tempfile.TemporaryDirectory() as tmp:
        root = Path(tmp)
        populate(root, module, justfile=justfile_text(include_dynamic=False))
        findings = module.scan_root(root)
        if not any("source-fence-static" in finding for finding in findings):
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
        assert_pending_source_ref_in_bte_narrative_is_a_finding,
        assert_missing_bte_blocker_is_a_finding,
        assert_missing_bte_guard_is_a_finding,
        assert_status_hash_drift_is_a_finding,
        assert_dynamic_status_flag_drift_is_a_finding,
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
