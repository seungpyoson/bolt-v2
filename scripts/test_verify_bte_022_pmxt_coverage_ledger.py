#!/usr/bin/env python3
"""Self-tests for the BTE-022 PMXT coverage-ledger verifier."""

from __future__ import annotations

import importlib.util
import json
import subprocess
import sys
import tempfile
from pathlib import Path


REPO_ROOT = Path(__file__).resolve().parent.parent
SCRIPT = REPO_ROOT / "scripts" / "verify_bte_022_pmxt_coverage_ledger.py"


def load_verifier():
    spec = importlib.util.spec_from_file_location("verify_bte_022_pmxt_coverage_ledger", SCRIPT)
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


def file_sha256(root: Path, rel_path: Path) -> str:
    import hashlib

    return hashlib.sha256((root / rel_path).read_bytes()).hexdigest()


def source_fixture(*, status: str = "pending", usage_scope: str = "one_off_backfill_data") -> dict:
    return {
        "schema_version": "backfill-source-proof.v1",
        "source_proof_id": "source-proof-polymarket-pmxt-v2-orderbook-binary-option-pending-2026-06-08",
        "status": status,
        "source_selection_status": "PENDING_MORE_PROOF",
        "usage_scope": usage_scope,
        "table_family": "order_book_snapshot_deltas",
        "source_binding": "polymarket-parquet-archive-index",
        "required_checks": {
            "coverage": {"outcome": "pending"},
            "retention_freshness": {"outcome": "pending"},
            "completeness": {"outcome": "pending"},
            "cost": {"outcome": "pending"},
            "storage": {"outcome": "pending"},
        },
    }


def coverage_status(module, root: Path, *, overrides: dict | None = None, bad_hash: bool = False) -> dict:
    fixture_hash = file_sha256(root, module.PMXT_SOURCE_PROOF_FIXTURE)
    status = {
        "schema_version": "source-proof-pmxt-coverage-ledger-status.v1",
        "task_id": "BACKTESTING_ENGINE-022",
        "status": "rejected_under_pending_source_proof",
        "recorded_at_utc": "2026-06-08T21:41:52Z",
        "scope": {
            "question": "Can the current PMXT coverage ledger authorize canonical or broad Polymarket L2 backfill?",
            "answer": "No. The ledger is useful rejected evidence: both records are rejected because the PMXT source proof remains pending.",
            "usage_scope": "one_off_backfill_data",
            "broad_backfill_allowed": False,
            "canonical_ready": False,
        },
        "source_proof": {
            "source_proof_path": module.repo_uri(module.PMXT_SOURCE_PROOF_FIXTURE),
            "source_proof_id": "source-proof-polymarket-pmxt-v2-orderbook-binary-option-pending-2026-06-08",
            "source_proof_version": 1,
            "source_binding": "polymarket-parquet-archive-index",
            "source_proof_status": "pending",
            "table_family": "order_book_snapshot_deltas",
        },
        "committed_input_hashes": {
            "pending_source_fixture": {
                "path": module.repo_uri(module.PMXT_SOURCE_PROOF_FIXTURE),
                "sha256": "bad" if bad_hash else fixture_hash,
            }
        },
        "ledger_run": {
            "command": "python3 scripts/rust_verification.py cargo --repo crates/backtesting-vertical-slice -- run --locked --bin backfill_coverage_ledger -- --spec /private/tmp/bte-pmxt-coverage-ledger-source-proof-20260609/pmxt-coverage-ledger.toml",
            "spec_path": "/private/tmp/bte-pmxt-coverage-ledger-source-proof-20260609/pmxt-coverage-ledger.toml",
            "spec_file_sha256": "d6209aa6d444cd12c9ea43bd8b7bf494f05ffb7ffbfb1e42a10a232b24c89134",
            "ledger_path": "/private/tmp/bte-pmxt-coverage-ledger-source-proof-20260609/ledger-output/backfill-coverage-ledger.json",
            "ledger_file_sha256": "7dc20194f826eb9c05ec99c27cdb86057f775e1486d688a73cc2924659a4c2cf",
            "payload_downloaded": False,
            "raw_manifest_mutated": False,
        },
        "coverage_summary": {
            "coverage_axis": "timestamp_received",
            "total_records": 2,
            "rejected_records": 2,
            "accepted_records": 0,
            "canonical_ready_records": 0,
            "accepted_objects": 0,
            "accepted_bytes": 0,
            "physical_only_objects": 0,
            "physical_only_bytes": 0,
            "blocking_issue_count": 2,
            "blocking_issues": ["source_proof_not_accepted"],
        },
        "records": [
            {
                "record_id": "archive-s3-run-00a4deb49a46a973",
                "status": "rejected",
                "canonical_ready": False,
                "blocking_issues": ["source_proof_not_accepted"],
            },
            {
                "record_id": "archive-s3-accept-streaming-orphans-da5876ae6d4b54cc",
                "status": "rejected",
                "canonical_ready": False,
                "blocking_issues": ["source_proof_not_accepted"],
            },
        ],
        "claim_limits": list(module.COVERAGE_CLAIM_LIMITS),
        "next_required_evidence": list(module.COVERAGE_NEXT_REQUIRED_EVIDENCE),
        "guard_verification": {
            "script": "repo://scripts/verify_bte_022_pmxt_coverage_ledger.py",
            "self_test": "repo://scripts/test_verify_bte_022_pmxt_coverage_ledger.py",
            "source_fence_static": True,
        },
    }
    if overrides:
        status.update(overrides)
    return status


def bte_status(module, *, coverage_text: str | None = None, can_close: bool = False) -> dict:
    blockers = []
    for blocker in module.BTE_REMAINING_BLOCKERS:
        if blocker == "expanded_tranche_coverage_and_cost_unproven":
            required_evidence = coverage_text or (
                f"{module.repo_uri(module.PMXT_COVERAGE_STATUS)} records the manifest-only PMXT coverage ledger as rejected evidence: "
                "accepted expanded coverage/cost remains unproven because both records are rejected under pending source proof. "
                "STATIC-GATED scripts/verify_bte_022_pmxt_coverage_ledger.py rejects drift."
            )
        else:
            required_evidence = f"{blocker} remains required before BTE-022 can close."
        blockers.append({"blocker": blocker, "required_evidence": required_evidence})
    return {
        "task_id": "BACKTESTING_ENGINE-022",
        "status": "open_pmxt_one_off_current_artifact_proven_broad_backfill_blocked",
        "bte_022_can_close": can_close,
        "remaining_blockers": blockers,
    }


def justfile_text(*, include_coverage: bool = True) -> str:
    recipe = (
        "verify-bte-022-pmxt-coverage-ledger: check-workspace\n"
        "    python3 scripts/test_verify_bte_022_pmxt_coverage_ledger.py\n"
        "    python3 scripts/verify_bte_022_pmxt_coverage_ledger.py\n\n"
    )
    source_fence = (
        "source-fence-static: check-workspace\n"
        "    python3 scripts/test_verify_bte_022_pmxt_coverage_ledger.py\n"
        "    python3 scripts/verify_bte_022_pmxt_coverage_ledger.py\n"
    )
    if include_coverage:
        return recipe + source_fence
    return "source-fence-static: check-workspace\n    python3 scripts/verify_bte_022_pmxt_coverage_ledger.py\n"


def populate(root: Path, module, **overrides) -> None:
    write_file(root, str(module.PMXT_SOURCE_PROOF_FIXTURE), json_text(overrides.get("fixture", source_fixture())))
    write_file(
        root,
        str(module.PMXT_COVERAGE_STATUS),
        json_text(
            coverage_status(
                module,
                root,
                overrides=overrides.get("coverage_overrides"),
                bad_hash=overrides.get("bad_hash", False),
            )
        ),
    )
    write_file(root, str(module.BTE_022_STATUS), json_text(overrides.get("bte", bte_status(module))))
    write_file(root, "justfile", overrides.get("justfile", justfile_text()))


def assert_clean_fixture_passes() -> None:
    module = load_verifier()
    with tempfile.TemporaryDirectory() as tmp:
        root = Path(tmp)
        populate(root, module)
        findings = module.scan_root(root)
        if findings:
            raise AssertionError(f"expected clean fixture, got {findings}")


def assert_accepted_coverage_status_is_a_finding() -> None:
    module = load_verifier()
    with tempfile.TemporaryDirectory() as tmp:
        root = Path(tmp)
        populate(
            root,
            module,
            coverage_overrides={
                "status": "accepted",
                "scope": {
                    "question": "Can the current PMXT coverage ledger authorize canonical or broad Polymarket L2 backfill?",
                    "answer": "Yes.",
                    "usage_scope": "canonical_backfill_input",
                    "broad_backfill_allowed": True,
                    "canonical_ready": True,
                },
            },
        )
        findings = module.scan_root(root)
        if not any("scope.broad_backfill_allowed" in finding for finding in findings):
            raise AssertionError(f"expected accepted coverage finding, got {findings}")


def assert_accepted_source_fixture_is_a_finding() -> None:
    module = load_verifier()
    with tempfile.TemporaryDirectory() as tmp:
        root = Path(tmp)
        populate(root, module, fixture=source_fixture(status="accepted", usage_scope="canonical_backfill_input"))
        findings = module.scan_root(root)
        if not any("status must be 'pending'" in finding for finding in findings):
            raise AssertionError(f"expected pending fixture finding, got {findings}")


def assert_source_fixture_hash_drift_is_a_finding() -> None:
    module = load_verifier()
    with tempfile.TemporaryDirectory() as tmp:
        root = Path(tmp)
        populate(root, module, bad_hash=True)
        findings = module.scan_root(root)
        if not any("pending_source_fixture.sha256" in finding for finding in findings):
            raise AssertionError(f"expected source fixture hash finding, got {findings}")


def assert_missing_bte_static_gate_text_is_a_finding() -> None:
    module = load_verifier()
    with tempfile.TemporaryDirectory() as tmp:
        root = Path(tmp)
        populate(root, module, bte=bte_status(module, coverage_text="coverage is accepted"))
        findings = module.scan_root(root)
        if not any("STATIC-GATED scripts/verify_bte_022_pmxt_coverage_ledger.py" in finding for finding in findings):
            raise AssertionError(f"expected BTE blocker text finding, got {findings}")


def assert_bte_close_claim_is_a_finding() -> None:
    module = load_verifier()
    with tempfile.TemporaryDirectory() as tmp:
        root = Path(tmp)
        populate(root, module, bte=bte_status(module, can_close=True))
        findings = module.scan_root(root)
        if not any("bte_022_can_close" in finding for finding in findings):
            raise AssertionError(f"expected BTE close finding, got {findings}")


def assert_justfile_wiring_is_a_finding() -> None:
    module = load_verifier()
    with tempfile.TemporaryDirectory() as tmp:
        root = Path(tmp)
        populate(root, module, justfile=justfile_text(include_coverage=False))
        findings = module.scan_root(root)
        if not any("source-fence-static" in finding for finding in findings):
            raise AssertionError(f"expected source-fence wiring finding, got {findings}")


def assert_cli_fails_with_actionable_output() -> None:
    module = load_verifier()
    with tempfile.TemporaryDirectory() as tmp:
        root = Path(tmp)
        populate(root, module, fixture=source_fixture(status="accepted"))
        result = subprocess.run(
            [sys.executable, str(SCRIPT), "--root", str(root)],
            text=True,
            stdout=subprocess.PIPE,
            stderr=subprocess.PIPE,
            check=False,
        )
        if result.returncode == 0:
            raise AssertionError("script should fail when source fixture is accepted")
        if "FAIL:" not in result.stderr or "status" not in result.stderr:
            raise AssertionError(result.stderr)


def main() -> int:
    tests = (
        assert_clean_fixture_passes,
        assert_accepted_coverage_status_is_a_finding,
        assert_accepted_source_fixture_is_a_finding,
        assert_source_fixture_hash_drift_is_a_finding,
        assert_missing_bte_static_gate_text_is_a_finding,
        assert_bte_close_claim_is_a_finding,
        assert_justfile_wiring_is_a_finding,
        assert_cli_fails_with_actionable_output,
    )
    for test in tests:
        test()
    print("OK: BTE-022 PMXT coverage-ledger verifier self-tests passed.")
    return 0


if __name__ == "__main__":
    import lane_governor

    lane_governor.acquire()
    raise SystemExit(main())
