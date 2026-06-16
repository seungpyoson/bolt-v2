#!/usr/bin/env python3
"""Self-tests for the BTE-022 PMXT broad-backfill efficiency verifier."""

from __future__ import annotations

import importlib.util
import json
import shutil
import subprocess
import sys
import tempfile
from pathlib import Path


REPO_ROOT = Path(__file__).resolve().parent.parent
SCRIPT = REPO_ROOT / "scripts" / "verify_bte_022_pmxt_broad_backfill_efficiency.py"


def load_verifier():
    spec = importlib.util.spec_from_file_location("verify_bte_022_pmxt_broad_backfill_efficiency", SCRIPT)
    if spec is None or spec.loader is None:
        raise AssertionError(f"failed to load {SCRIPT}")
    module = importlib.util.module_from_spec(spec)
    sys.modules[spec.name] = module
    spec.loader.exec_module(module)
    return module


def copy_file(root: Path, rel_path: Path) -> None:
    source = REPO_ROOT / rel_path
    target = root / rel_path
    target.parent.mkdir(parents=True, exist_ok=True)
    shutil.copy2(source, target)


def read_json(root: Path, rel_path: Path) -> dict:
    return json.loads((root / rel_path).read_text(encoding="utf-8"))


def write_json(root: Path, rel_path: Path, data: dict) -> None:
    (root / rel_path).write_text(json.dumps(data, indent=2) + "\n", encoding="utf-8")


def copy_fixture(root: Path, module) -> None:
    for rel_path in (
        module.PMXT_BROAD_STATUS,
        module.PMXT_COVERAGE_STATUS,
        module.PMXT_DURABLE_STATUS,
        module.PMXT_DYNAMIC_STATUS,
        module.BTE_022_STATUS,
        module.JUSTFILE,
    ):
        copy_file(root, rel_path)


def assert_contains(findings: list[str], needle: str) -> None:
    if not any(needle in finding for finding in findings):
        raise AssertionError(f"expected {needle!r} in findings, got {findings}")


def test_complete_fixture_passes() -> None:
    module = load_verifier()
    with tempfile.TemporaryDirectory() as tmp:
        root = Path(tmp)
        copy_fixture(root, module)
        findings = module.scan_root(root)
    if findings:
        raise AssertionError(f"expected clean fixture, got {findings}")


def test_broad_status_overclaim_is_a_finding() -> None:
    module = load_verifier()
    with tempfile.TemporaryDirectory() as tmp:
        root = Path(tmp)
        copy_fixture(root, module)
        broad = read_json(root, module.PMXT_BROAD_STATUS)
        broad["status"] = "accepted_broad_backfill_ready"
        broad["bte_022_can_close"] = True
        broad["decision"] = "Start broad PMXT backfill."
        write_json(root, module.PMXT_BROAD_STATUS, broad)
        findings = module.scan_root(root)
    assert_contains(findings, "status must be")
    assert_contains(findings, "bte_022_can_close")
    assert_contains(findings, "Do not start broad PMXT/Polymarket L2 backfill")


def test_coverage_dependency_overclaim_is_a_finding() -> None:
    module = load_verifier()
    with tempfile.TemporaryDirectory() as tmp:
        root = Path(tmp)
        copy_fixture(root, module)
        coverage = read_json(root, module.PMXT_COVERAGE_STATUS)
        coverage["status"] = "accepted"
        coverage["scope"]["broad_backfill_allowed"] = True
        coverage["coverage_summary"]["accepted_records"] = 2
        write_json(root, module.PMXT_COVERAGE_STATUS, coverage)
        findings = module.scan_root(root)
    assert_contains(findings, "coverage_ledger_status.sha256")
    assert_contains(findings, "scope.broad_backfill_allowed")
    assert_contains(findings, "coverage_summary.accepted_records")


def test_durable_dependency_acceptance_drift_is_a_finding() -> None:
    module = load_verifier()
    with tempfile.TemporaryDirectory() as tmp:
        root = Path(tmp)
        copy_fixture(root, module)
        durable = read_json(root, module.PMXT_DURABLE_STATUS)
        durable["durable_source_selection_status"] = "accepted"
        durable["source_accepted_proof_count"] = 1
        durable["source_proof_set_spec"]["status"] = "accepted"
        durable["manifest_scope"]["source_accepted_proof_count"] = 1
        write_json(root, module.PMXT_DURABLE_STATUS, durable)
        findings = module.scan_root(root)
    assert_contains(findings, "durable_source_selection_status.sha256")
    assert_contains(findings, "source_accepted_proof_count")
    assert_contains(findings, "source_proof_set_spec.status")


def test_dynamic_tick_size_overclaim_is_a_finding() -> None:
    module = load_verifier()
    with tempfile.TemporaryDirectory() as tmp:
        root = Path(tmp)
        copy_fixture(root, module)
        dynamic = read_json(root, module.PMXT_DYNAMIC_STATUS)
        dynamic["dynamic_tick_size_replay_status"] = "accepted"
        dynamic["standard_backtestnode_catalog_replay_supports_dynamic_instrument_any"] = True
        dynamic["timed_instrument_epoch_replay_accepted"] = True
        dynamic["pmxt_full_l2_with_tick_size_change_can_be_accepted_now"] = True
        write_json(root, module.PMXT_DYNAMIC_STATUS, dynamic)
        findings = module.scan_root(root)
    assert_contains(findings, "dynamic_tick_size_replay_status.sha256")
    assert_contains(findings, "timed_instrument_epoch_replay_accepted")
    assert_contains(findings, "pmxt_full_l2_with_tick_size_change_can_be_accepted_now")


def test_missing_bte_static_gate_text_is_a_finding() -> None:
    module = load_verifier()
    with tempfile.TemporaryDirectory() as tmp:
        root = Path(tmp)
        copy_fixture(root, module)
        bte = read_json(root, module.BTE_022_STATUS)
        for blocker in bte["remaining_blockers"]:
            if blocker["blocker"] == "broad_backfill_efficiency_unproven":
                blocker["required_evidence"] = "Broad backfill is ready."
        write_json(root, module.BTE_022_STATUS, bte)
        findings = module.scan_root(root)
    assert_contains(findings, "STATIC-GATED scripts/verify_bte_022_pmxt_broad_backfill_efficiency.py")
    assert_contains(findings, "Broad payload work still must not start")


def test_justfile_wiring_is_a_finding() -> None:
    module = load_verifier()
    with tempfile.TemporaryDirectory() as tmp:
        root = Path(tmp)
        copy_fixture(root, module)
        justfile = (root / module.JUSTFILE).read_text(encoding="utf-8")
        justfile = justfile.replace("    python3 scripts/verify_bte_022_pmxt_broad_backfill_efficiency.py\n", "")
        (root / module.JUSTFILE).write_text(justfile, encoding="utf-8")
        findings = module.scan_root(root)
    assert_contains(findings, "source-fence-static-inner")


def test_cli_fails_with_actionable_output() -> None:
    module = load_verifier()
    with tempfile.TemporaryDirectory() as tmp:
        root = Path(tmp)
        copy_fixture(root, module)
        broad = read_json(root, module.PMXT_BROAD_STATUS)
        broad["status"] = "accepted_broad_backfill_ready"
        write_json(root, module.PMXT_BROAD_STATUS, broad)
        result = subprocess.run(
            [sys.executable, str(SCRIPT), "--root", str(root)],
            text=True,
            stdout=subprocess.PIPE,
            stderr=subprocess.PIPE,
            check=False,
        )
    if result.returncode == 0:
        raise AssertionError("script should fail when broad backfill is accepted")
    if "FAIL:" not in result.stderr or "status" not in result.stderr:
        raise AssertionError(result.stderr)


def main() -> int:
    tests = (
        test_complete_fixture_passes,
        test_broad_status_overclaim_is_a_finding,
        test_coverage_dependency_overclaim_is_a_finding,
        test_durable_dependency_acceptance_drift_is_a_finding,
        test_dynamic_tick_size_overclaim_is_a_finding,
        test_missing_bte_static_gate_text_is_a_finding,
        test_justfile_wiring_is_a_finding,
        test_cli_fails_with_actionable_output,
    )
    for test in tests:
        test()
    print("OK: BTE-022 PMXT broad-backfill efficiency verifier self-tests passed.")
    return 0


if __name__ == "__main__":
    import lane_governor

    lane_governor.acquire()
    raise SystemExit(main())
