#!/usr/bin/env python3
"""Self-tests for the BTE-022 PMXT durable-source verifier."""

from __future__ import annotations

import importlib.util
import hashlib
import json
import subprocess
import sys
import tempfile
from pathlib import Path


REPO_ROOT = Path(__file__).resolve().parent.parent
SCRIPT = REPO_ROOT / "scripts" / "verify_bte_022_pmxt_durable_source.py"


def load_verifier():
    spec = importlib.util.spec_from_file_location("verify_bte_022_pmxt_durable_source", SCRIPT)
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


def source_proof_spec_text(*, status: str = "pending", usage_scope: str = "one_off_backfill_data") -> str:
    return f"""
proof_set_id = "source-universe-source-proofs-pmxt-polymarket-v2-current"
output_dir = "specs/023-nt-research-analytics-platform/reference/backfill-source-proofs/pmxt-polymarket-v2-current"
source_bindings_path = "specs/023-nt-research-analytics-platform/reference/backfill-source-bindings.v1.toml"
venue = "polymarket"
table_family = "order_book_snapshot_deltas"
manifest_table_family = "order_book_snapshot_deltas"
status = "{status}"
source_candidate_class = "official_free"
source_selection_status = "PENDING_MORE_PROOF"
usage_scope = "{usage_scope}"
fidelity_class = "L2_REPLAY"
requested_start_utc = "2026-04-13T19:00:00Z"
requested_end_utc = "2026-06-10T16:00:00Z"
coverage_start_utc = "2026-04-13T19:00:00Z"
coverage_end_utc = "2026-06-10T16:00:00Z"
license_ref = "https://archive.pmxt.dev/docs/v2-data-overview#license"
license_scope = "public"
retention_ref = "pending://source-proofs/pmxt-polymarket-v2-current/retention-freshness"
cost_ref = "pending://source-proofs/pmxt-polymarket-v2-current/cost"
gap_policy_id = ""
raw_sample_selection = "first_manifest_record"
schema_sample_policy = "raw_sample"

[l2_replay_evidence]
order_book_delta_ref = "repo://specs/023-nt-research-analytics-platform/reference/source-proof-nt-mapping-inspection.polymarket-pmxt-v2-orderbook.2026-06-08.json"
sufficient_snapshot_cadence_ref = "repo://specs/023-nt-research-analytics-platform/reference/source-proof-sample-inspection.polymarket-pmxt-v2-orderbook.2026-06-08.json"

[required_checks.source_access]
outcome = "passed"
evidence_ref = "source"
[required_checks.license]
outcome = "passed"
evidence_ref = "license"
[required_checks.schema]
outcome = "passed"
evidence_ref = "schema"
[required_checks.time_semantics]
outcome = "passed"
evidence_ref = "time"
[required_checks.instrument_universe]
outcome = "pending"
evidence_ref = "instrument"
[required_checks.coverage]
outcome = "pending"
evidence_ref = "coverage"
[required_checks.retention_freshness]
outcome = "pending"
evidence_ref = "retention"
[required_checks.granularity]
outcome = "passed"
evidence_ref = "granularity"
[required_checks.completeness]
outcome = "pending"
evidence_ref = "completeness"
[required_checks.nt_mapping]
outcome = "passed"
evidence_ref = "nt"
[required_checks.cost]
outcome = "pending"
evidence_ref = "cost"
[required_checks.storage]
outcome = "pending"
evidence_ref = "storage"

[[claim_limit]]
id = "pmxt-source-proof-claim-limit-001"
severity = "blocking"
claim = "No canonical, production, or broad NT catalog/backtest input from this pending PMXT L2 source proof."
reason = "The generated proof is manifest-scoped but remains pending until coverage, cost, storage, completeness, and tick-size policy evidence are accepted."
evidence_ref = "source-proof://{{source_proof_id}}/status"

[[source_binding]]
source_binding = "polymarket-parquet-archive-index"
source_proof_id = "source-proof-pmxt-polymarket-v2-current-orderbook"
product_category = "binary-option"
instrument_universe_id = "pmxt-polymarket-v2-current-orderbook"
category_manifest_path = "specs/023-nt-research-analytics-platform/reference/backfill-source-universe-object-manifests/pmxt-polymarket-v2-current/category-manifests/pmxt-polymarket-v2-object-manifest-orderbook.json"
"""


def manifest_json() -> str:
    return """{
  "object_count": 1351,
  "accepted_bytes": 557815904970,
  "category_summaries": [
    {
      "object_count": 1351,
      "compressed_bytes": 557815904970
    }
  ]
}"""


def category_manifest_json() -> str:
    return """{
  "object_count": 1351,
  "accepted_bytes": 557815904970
}"""


def archive_index_json() -> str:
    return """{
  "object_count": 1351,
  "verified_head_count": 1351,
  "total_content_length_bytes": 557815904970
}"""


def source_fixture_json(*, usage_scope: str = "one_off_backfill_data") -> str:
    checks = {
        "source_access": "passed",
        "license": "passed",
        "schema": "passed",
        "time_semantics": "passed",
        "instrument_universe": "passed",
        "coverage": "pending",
        "retention_freshness": "pending",
        "granularity": "passed",
        "completeness": "pending",
        "nt_mapping": "passed",
        "cost": "pending",
        "storage": "pending",
    }
    required_checks = ",\n".join(
        f'    "{name}": {{"outcome": "{outcome}", "evidence_ref": "{name}"}}'
        for name, outcome in checks.items()
    )
    return f"""{{
  "source_proof_id": "source-proof-polymarket-pmxt-v2-orderbook-binary-option-pending-2026-06-08",
  "source_binding": "polymarket-parquet-archive-index",
  "raw_sample_uri": "https://r2v2.pmxt.dev/polymarket_orderbook_2026-05-20T22.parquet",
  "status": "pending",
  "source_selection_status": "PENDING_MORE_PROOF",
  "usage_scope": "{usage_scope}",
  "acceptance_scope": {{
    "planned_objects": 1,
    "completed_objects": 1,
    "failed_objects": 0,
    "skipped_objects": 0,
    "accepted_bytes": 361365244,
    "selector_scope_violations": 0
  }},
  "required_checks": {{
{required_checks}
  }}
}}"""


def venue_ledger_text(*, include_missing_accepted: bool = True) -> str:
    missing = '"missing_accepted_source_proof",' if include_missing_accepted else ""
    return f"""
[[venue]]
venue_id = "pmxt-current-reference"
venue = "pmxt"

[[venue.universe]]
universe_id = "pmxt-polymarket-full-current-data"
scope_label = "Polymarket full current local/archive data"
status = "blocked"
source_universe_source_proof_set_path = "specs/023-nt-research-analytics-platform/reference/backfill-source-proofs/pmxt-polymarket-v2-current/source-universe-source-proof-set.json"
blocking_issues = [
  {missing}
  "missing_source_universe_object_gates",
  "missing_source_universe_conversion_run_plan",
  "missing_pmxt_l2_tick_size_epoch_policy",
]
"""


def bte_status_json(*, can_close: bool = False) -> str:
    guard_status = {
        "status": "code_guardrail_added_actual_pmxt_accepted_source_proof_unproven",
        "evidence": [
            "RED-GATED crates/backtesting-vertical-slice/src/venue_scale_conversion_acceptance.rs unit regression source_only_status_rejects_unaccepted_source_proof_set documents that a source-only universe with a referenced source proof set but zero accepted proofs must fail validation.",
            "GREEN-GATED venue-scale conversion acceptance validation now receives source_proof_count and source_accepted_proof_count and rejects SourceOnly when source_proof_count > 0 and source_accepted_proof_count == 0.",
            "REGRESSION crates/backtesting-vertical-slice/tests/backtesting_vertical_slice_venue_scale_conversion_acceptance.rs source_proof_set_rejects_accepted_count_above_total_count documents that source proof sets with accepted_proof_count > proof_count must fail before status accounting.",
            "repo://specs/023-nt-research-analytics-platform/reference/venue-scale-conversion-acceptance-ledgers/binance-bybit-pmxt-current/venue-scale-conversion-acceptance-ledger.toml explicitly lists missing_accepted_source_proof on pmxt-polymarket-full-current-data while source_accepted_proof_count remains 0.",
            "repo://specs/023-nt-research-analytics-platform/reference/venue-scale-conversion-acceptance-ledgers/binance-bybit-pmxt-current/venue-scale-conversion-acceptance-ledger.toml keeps pmxt-polymarket-full-current-data blocked while repo://specs/023-nt-research-analytics-platform/reference/source-proof-pmxt-durable-source-selection-status.2026-06-16.json pins source_accepted_proof_count=0.",
            "repo://specs/023-nt-research-analytics-platform/reference/source-proof-pmxt-durable-source-selection-status.2026-06-16.json records the source-controlled PMXT durable-source guard: one pending proof in the committed TOML spec, zero accepted proofs, generated bulk JSON evicted by policy, and source-fence static coverage via scripts/verify_bte_022_pmxt_durable_source.py.",
            "repo://specs/023-nt-research-analytics-platform/reference/source-proof-pmxt-durable-source-selection-status.2026-06-16.json source-fences the PMXT pending fixture against crates/backtesting-vertical-slice/src/source_proof_admissibility.rs and crates/backtesting-vertical-slice/src/source_proof.rs as current_contract_rejected with current contract fields present, acceptance_failed because raw_sample_uri must be staged to s3:// before canonical acceptance, and explicit one_off_backfill_data usage that cannot be promoted to canonical source proof input.",
            "STATIC-GATED scripts/verify_bte_022_pmxt_durable_source.py rejects drift from pending/one_off_backfill_data PMXT source proof state, missing pmxt-polymarket-full-current-data blocking issues, BTE-022 close claims, and source-fence wiring gaps.",
        ],
        "claim_limits": [
            "This proves a venue-scale source-only status cannot be backed by an explicitly unaccepted source proof set.",
            "This proves source proof set summary counts fail closed when accepted_proof_count exceeds proof_count.",
            "This proves the PMXT durable-source state has a compact source-fenced guard even though generated PMXT bulk JSON artifacts remain evicted.",
            "This does not accept any PMXT source proof.",
            "This does not prove expanded PMXT coverage, cost, object gates, conversion run plans, or dynamic tick-size replay.",
            "This does not authorize broad PMXT backfill.",
        ],
    }
    return json.dumps(
        {
            "schema_version": "source-proof-nt-catalog-mapping-status.v1",
            "task_id": "BACKTESTING_ENGINE-022",
            "status": "open_pmxt_one_off_current_artifact_proven_broad_backfill_blocked",
            "recorded_at": "2026-06-08",
            "decision": "Do not start broad PMXT backfill. PMXT may proceed only as one-off backfill evidence after the chosen selected-source sample is converted into NT-native data classes, written to ParquetDataCatalog under the artifact root, queried back, consumed by BacktestNode, and bound to a result contract.",
            "current_reconciliation": {},
            "nt_capability_evidence": {},
            "bolt_current_limitations": {},
            "source_mapping_status": {},
            "pmxt_one_off_conversion_metadata_status": {},
            "bounded_first_proof_selector_status": {},
            "_".join(("first", "proof", "policy", "status")): {},
            "bounded_l2_manifest_mapping_status": {},
            "bounded_l2_catalog_hash_status": {},
            "bounded_l2_backtestnode_status": {},
            "bounded_l2_result_contract_status": {},
            "accepted_trade_replay_runtime_recheck": {},
            "broad_backfill_efficiency_object_selection_metadata_status": {},
            "broad_backfill_source_usage_scope_status": {},
            "durable_source_selection_source_only_guardrail_status": guard_status,
            "dynamic_tick_size_replay_guardrail_status": {},
            "non_hardcoding_decision": "",
            "old_artifact_recommendation": {},
            "next_required_evidence": [],
            "remaining_blockers": [
                {"blocker": "expanded_tranche_coverage_and_cost_unproven"},
                {"blocker": "dynamic_tick_size_replay_unproven"},
                {"blocker": "durable_source_selection_unproven"},
                {"blocker": "broad_backfill_efficiency_unproven"},
            ],
            "bte_022_can_close": can_close,
        },
        indent=2,
    )

def file_sha256(root: Path, rel: str) -> str:
    return hashlib.sha256((root / rel).read_bytes()).hexdigest()


def durable_status_json(root: Path) -> str:
    source_proof_spec = (
        "specs/023-nt-research-analytics-platform/reference/backfill-source-proofs/"
        "pmxt-polymarket-v2-current/source-universe-source-proofs.toml"
    )
    source_manifest = (
        "specs/023-nt-research-analytics-platform/reference/backfill-source-universe-object-manifests/"
        "pmxt-polymarket-v2-current/manifest/source-universe-object-manifest.json"
    )
    category_manifest = (
        "specs/023-nt-research-analytics-platform/reference/backfill-source-universe-object-manifests/"
        "pmxt-polymarket-v2-current/category-manifests/pmxt-polymarket-v2-object-manifest-orderbook.json"
    )
    archive_index_manifest = (
        "specs/023-nt-research-analytics-platform/reference/source-archive-index-manifests/"
        "pmxt-polymarket-v2-current/manifest/source-archive-index-manifest.json"
    )
    conversion_queue_spec = (
        "specs/023-nt-research-analytics-platform/reference/source-universe-conversion-queues/"
        "pmxt-polymarket-v2-current/source-universe-conversion-queue.toml"
    )
    venue_acceptance_ledger_spec = (
        "specs/023-nt-research-analytics-platform/reference/venue-scale-conversion-acceptance-ledgers/"
        "binance-bybit-pmxt-current/venue-scale-conversion-acceptance-ledger.toml"
    )
    pending_source_fixture = (
        "specs/023-nt-research-analytics-platform/reference/"
        "source-proof-fixture.binary-option.polymarket-pmxt-official-free-pending.v1.json"
    )
    acceptance_contract = "crates/backtesting-vertical-slice/src/source_proof.rs"
    admissibility_contract = "crates/backtesting-vertical-slice/src/source_proof_admissibility.rs"
    return f"""{{
  "schema_version": "source-proof-pmxt-durable-source-selection-status.v1",
  "task_id": "BACKTESTING_ENGINE-022",
  "observed_at_utc": "2026-06-16T01:55:00Z",
  "durable_source_selection_status": "blocked_pending_source_proof",
  "source_binding": "polymarket-parquet-archive-index",
  "source_proof_set_spec": {{
    "path": "repo://{source_proof_spec}",
    "sha256": "{file_sha256(root, source_proof_spec)}",
    "status": "pending",
    "source_selection_status": "PENDING_MORE_PROOF",
    "usage_scope": "one_off_backfill_data",
    "fidelity_class": "L2_REPLAY"
  }},
  "source_proof_admissibility_status": {{
    "status": "source_fenced_current_contract_rejected",
    "proof_uri": "repo://{pending_source_fixture}",
    "proof_fixture": {{
      "path": "repo://{pending_source_fixture}",
      "sha256": "{file_sha256(root, pending_source_fixture)}"
    }},
    "admissibility_contract": {{
      "path": "repo://{admissibility_contract}",
      "sha256": "{file_sha256(root, admissibility_contract)}"
    }},
    "acceptance_contract": {{
      "path": "repo://{acceptance_contract}",
      "sha256": "{file_sha256(root, acceptance_contract)}"
    }},
    "current_contract_deserializes": true,
    "expected_record_status": "current_contract_rejected",
    "missing_current_contract_fields": [],
    "blocking_issues": [
      "acceptance_failed"
    ],
    "acceptance_error": "raw_sample_uri must be a staged s3:// URI, got \\"https://r2v2.pmxt.dev/polymarket_orderbook_2026-05-20T22.parquet\\"",
    "source_proof_id": "source-proof-polymarket-pmxt-v2-orderbook-binary-option-pending-2026-06-08",
    "source_binding": "polymarket-parquet-archive-index",
    "usage_scope": "one_off_backfill_data",
    "source_selection_status": "PENDING_MORE_PROOF"
  }},
  "committed_input_hashes": {{
    "source_universe_manifest": {{
      "path": "repo://{source_manifest}",
      "sha256": "{file_sha256(root, source_manifest)}"
    }},
    "category_manifest": {{
      "path": "repo://{category_manifest}",
      "sha256": "{file_sha256(root, category_manifest)}"
    }},
    "archive_index_manifest": {{
      "path": "repo://{archive_index_manifest}",
      "sha256": "{file_sha256(root, archive_index_manifest)}"
    }},
    "conversion_queue_spec": {{
      "path": "repo://{conversion_queue_spec}",
      "sha256": "{file_sha256(root, conversion_queue_spec)}"
    }},
    "venue_acceptance_ledger_spec": {{
      "path": "repo://{venue_acceptance_ledger_spec}",
      "sha256": "{file_sha256(root, venue_acceptance_ledger_spec)}"
    }},
    "pending_source_fixture": {{
      "path": "repo://{pending_source_fixture}",
      "sha256": "{file_sha256(root, pending_source_fixture)}"
    }}
  }},
  "manifest_scope": {{
    "object_count": 1351,
    "verified_head_count": 1351,
    "accepted_bytes": 557815904970,
    "source_accepted_proof_count": 0
  }},
  "source_proof_count": 1,
  "source_accepted_proof_count": 0,
  "pending_required_checks": [
    "instrument_universe",
    "coverage",
    "retention_freshness",
    "completeness",
    "cost",
    "storage"
  ],
  "passed_required_checks": [
    "source_access",
    "license",
    "schema",
    "time_semantics",
    "granularity",
    "nt_mapping"
  ],
  "generated_artifact_policy": {{
    "status": "bulk_json_evicted",
    "gitignore_refs": [
      "specs/023-nt-research-analytics-platform/reference/source-universe-conversion-queues/pmxt-polymarket-v2-current/queue/*.json",
      "specs/023-nt-research-analytics-platform/reference/backfill-source-proofs/pmxt-polymarket-v2-current/*.json",
      "specs/023-nt-research-analytics-platform/reference/venue-scale-conversion-acceptance-ledgers/*/ledger/*.json"
    ],
    "reason": "Issue #704 Phase 2 Tier 1 evicts generated source-universe reference JSON artifacts; committed TOML specs, compact status, and static verification are the source-controlled guard."
  }},
  "guard_verification": {{
    "script": "repo://scripts/verify_bte_022_pmxt_durable_source.py",
    "self_test": "repo://scripts/test_verify_bte_022_pmxt_durable_source.py",
    "source_fence_static": true
  }},
  "claim_limits": [
    "This records a durable-source guardrail, not an accepted durable PMXT source.",
    "The PMXT full-current universe must remain blocked while source_accepted_proof_count is zero.",
    "Generated queue/proof/ledger bulk JSON remains evicted; the committed TOML/status/verifier chain is the reviewable guard.",
    "This does not prove expanded coverage, object gates, conversion run plans, broad backfill efficiency, or dynamic tick-size replay."
  ],
  "remaining_blockers": [
    "durable_source_selection_unproven",
    "expanded_tranche_coverage_and_cost_unproven",
    "dynamic_tick_size_replay_unproven",
    "broad_backfill_efficiency_unproven"
  ],
  "bte_022_can_close": false
}}"""


def write_complete_fixture(root: Path) -> None:
    write_file(
        root,
        "specs/023-nt-research-analytics-platform/reference/backfill-source-proofs/pmxt-polymarket-v2-current/source-universe-source-proofs.toml",
        source_proof_spec_text(),
    )
    write_file(
        root,
        "specs/023-nt-research-analytics-platform/reference/backfill-source-universe-object-manifests/pmxt-polymarket-v2-current/manifest/source-universe-object-manifest.json",
        manifest_json(),
    )
    write_file(
        root,
        "specs/023-nt-research-analytics-platform/reference/backfill-source-universe-object-manifests/pmxt-polymarket-v2-current/category-manifests/pmxt-polymarket-v2-object-manifest-orderbook.json",
        category_manifest_json(),
    )
    write_file(
        root,
        "specs/023-nt-research-analytics-platform/reference/source-archive-index-manifests/pmxt-polymarket-v2-current/manifest/source-archive-index-manifest.json",
        archive_index_json(),
    )
    write_file(
        root,
        "specs/023-nt-research-analytics-platform/reference/source-universe-conversion-queues/pmxt-polymarket-v2-current/source-universe-conversion-queue.toml",
        'source_universe_manifest_path = "specs/023-nt-research-analytics-platform/reference/backfill-source-universe-object-manifests/pmxt-polymarket-v2-current/manifest/source-universe-object-manifest.json"\n',
    )
    write_file(
        root,
        "specs/023-nt-research-analytics-platform/reference/source-proof-fixture.binary-option.polymarket-pmxt-official-free-pending.v1.json",
        source_fixture_json(),
    )
    write_file(
        root,
        "specs/023-nt-research-analytics-platform/reference/venue-scale-conversion-acceptance-ledgers/binance-bybit-pmxt-current/venue-scale-conversion-acceptance-ledger.toml",
        venue_ledger_text(),
    )
    write_file(
        root,
        "specs/023-nt-research-analytics-platform/reference/source-proof-nt-catalog-mapping-status.backtesting-engine-022.2026-06-08.json",
        bte_status_json(),
    )
    write_file(
        root,
        ".gitignore",
        "\n".join(
            [
                "specs/023-nt-research-analytics-platform/reference/source-universe-conversion-queues/pmxt-polymarket-v2-current/queue/*.json",
                "specs/023-nt-research-analytics-platform/reference/backfill-source-proofs/pmxt-polymarket-v2-current/*.json",
                "specs/023-nt-research-analytics-platform/reference/venue-scale-conversion-acceptance-ledgers/*/ledger/*.json",
            ]
        ),
    )
    write_file(
        root,
        "justfile",
        (
            "verify-bte-022-pmxt-durable-source:\n"
            "    python3 scripts/test_verify_bte_022_pmxt_durable_source.py\n"
            "    python3 scripts/verify_bte_022_pmxt_durable_source.py\n"
            "source-fence-static-inner:\n"
            "    python3 scripts/test_verify_bte_022_pmxt_durable_source.py\n"
            "    python3 scripts/verify_bte_022_pmxt_durable_source.py\n"
        ),
    )
    write_file(
        root,
        "crates/backtesting-vertical-slice/src/source_proof.rs",
        (
            "fn validate_source_selection(proof: &SourceProofReport) -> Result<(), AcceptanceError> {\n"
            "    ensure_staged_s3_uri(\"raw_sample_uri\", &self.raw_sample_uri)?;\n"
            "    uri.starts_with(\"s3://\");\n"
            "    if proof.usage_scope == SourceProofUsageScope::OneOffBackfillData {\n"
            "        return Err(AcceptanceError::OneOffBackfillDataNotCanonical);\n"
            "    }\n"
            "}\n"
            "\"one_off_backfill_data source proofs cannot be accepted as canonical source proof input\"\n"
        ),
    )
    write_file(
        root,
        "crates/backtesting-vertical-slice/src/source_proof_admissibility.rs",
        (
            '"acceptance_scope",\n'
            "SourceProofAdmissibilityIssue::MissingCurrentContractField;\n"
            "SourceProofAdmissibilityStatus::CurrentContractRejected;\n"
            "SourceProofAdmissibilityIssue::AcceptanceFailed;\n"
            "acceptance_error: Some(error.to_string());\n"
        ),
    )
    write_file(
        root,
        "specs/023-nt-research-analytics-platform/reference/source-proof-pmxt-durable-source-selection-status.2026-06-16.json",
        durable_status_json(root),
    )


def run_script(*args: str) -> subprocess.CompletedProcess[str]:
    return subprocess.run(
        [sys.executable, str(SCRIPT), *args],
        cwd=REPO_ROOT,
        check=False,
        text=True,
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
    )


def test_complete_fixture_passes() -> None:
    verifier = load_verifier()
    with tempfile.TemporaryDirectory() as tmp:
        root = Path(tmp)
        write_complete_fixture(root)
        assert verifier.scan_root(root) == []


def test_json_explanatory_text_can_be_reworded() -> None:
    verifier = load_verifier()
    with tempfile.TemporaryDirectory() as tmp:
        root = Path(tmp)
        write_complete_fixture(root)
        durable_status = json.loads((root / verifier.PMXT_DURABLE_STATUS).read_text(encoding="utf-8"))
        durable_status["claim_limits"] = ["Reworded claim limit; structured status fields remain unchanged."]
        write_file(root, str(verifier.PMXT_DURABLE_STATUS), json.dumps(durable_status, indent=2) + "\n")
        bte_status = json.loads((root / verifier.BTE_022_STATUS).read_text(encoding="utf-8"))
        guard = bte_status["durable_source_selection_source_only_guardrail_status"]
        guard["evidence"] = ["Reworded guard evidence; structured guard status remains unchanged."]
        guard["claim_limits"] = ["Reworded guard claim limit; source-only guard remains unchanged."]
        bte_status["decision"] = "Reworded decision with the same structured status and blocker fields."
        write_file(root, str(verifier.BTE_022_STATUS), json.dumps(bte_status, indent=2) + "\n")

        assert verifier.scan_root(root) == []


def test_accepting_pmxt_source_proof_is_a_finding() -> None:
    verifier = load_verifier()
    with tempfile.TemporaryDirectory() as tmp:
        root = Path(tmp)
        write_complete_fixture(root)
        write_file(
            root,
            "specs/023-nt-research-analytics-platform/reference/backfill-source-proofs/pmxt-polymarket-v2-current/source-universe-source-proofs.toml",
            source_proof_spec_text(status="accepted", usage_scope="canonical_backfill_input"),
        )
        findings = verifier.scan_root(root)
    assert any("status must be 'pending'" in finding for finding in findings)
    assert any("usage_scope must be 'one_off_backfill_data'" in finding for finding in findings)


def test_missing_accepted_source_blocker_is_a_finding() -> None:
    verifier = load_verifier()
    with tempfile.TemporaryDirectory() as tmp:
        root = Path(tmp)
        write_complete_fixture(root)
        write_file(
            root,
            "specs/023-nt-research-analytics-platform/reference/venue-scale-conversion-acceptance-ledgers/binance-bybit-pmxt-current/venue-scale-conversion-acceptance-ledger.toml",
            venue_ledger_text(include_missing_accepted=False),
        )
        findings = verifier.scan_root(root)
    assert any("missing_accepted_source_proof" in finding for finding in findings)


def test_bte_close_claim_is_a_finding() -> None:
    verifier = load_verifier()
    with tempfile.TemporaryDirectory() as tmp:
        root = Path(tmp)
        write_complete_fixture(root)
        write_file(
            root,
            "specs/023-nt-research-analytics-platform/reference/source-proof-nt-catalog-mapping-status.backtesting-engine-022.2026-06-08.json",
            bte_status_json(can_close=True),
        )
        findings = verifier.scan_root(root)
    assert any("bte_022_can_close" in finding for finding in findings)


def test_bte_top_level_status_overclaim_is_a_finding() -> None:
    verifier = load_verifier()
    with tempfile.TemporaryDirectory() as tmp:
        root = Path(tmp)
        write_complete_fixture(root)
        write_file(
            root,
            "specs/023-nt-research-analytics-platform/reference/source-proof-nt-catalog-mapping-status.backtesting-engine-022.2026-06-08.json",
            bte_status_json().replace(
                '"status": "open_pmxt_one_off_current_artifact_proven_broad_backfill_blocked"',
                '"status": "closed_pmxt_accepted_broad_backfill_proven"',
                1,
            ),
        )
        findings = verifier.scan_root(root)
    assert any("status must be" in finding for finding in findings)


def test_bte_status_durable_guard_overclaim_is_a_finding() -> None:
    verifier = load_verifier()
    with tempfile.TemporaryDirectory() as tmp:
        root = Path(tmp)
        write_complete_fixture(root)
        bte_status = json.loads(bte_status_json())
        bte_status["durable_source_selection_source_only_guardrail_status"]["claim_limits"] = []
        write_file(root, str(verifier.BTE_022_STATUS), json.dumps(bte_status, indent=2) + "\n")
        findings = verifier.scan_root(root)
    assert any("durable_source_selection_source_only_guardrail_status.claim_limits" in finding for finding in findings)


def test_bte_status_empty_durable_guard_is_a_finding() -> None:
    verifier = load_verifier()
    with tempfile.TemporaryDirectory() as tmp:
        root = Path(tmp)
        write_complete_fixture(root)
        bte_status = json.loads(bte_status_json())
        bte_status["durable_source_selection_source_only_guardrail_status"] = {}
        write_file(root, str(verifier.BTE_022_STATUS), json.dumps(bte_status, indent=2) + "\n")
        findings = verifier.scan_root(root)
    assert any("durable_source_selection_source_only_guardrail_status.status" in finding for finding in findings)


def test_bte_status_extra_durable_guard_key_is_a_finding() -> None:
    verifier = load_verifier()
    with tempfile.TemporaryDirectory() as tmp:
        root = Path(tmp)
        write_complete_fixture(root)
        bte_status = json.loads(bte_status_json())
        bte_status["durable_source_selection_source_only_guardrail_status"]["accepted_source_proof"] = True
        write_file(root, str(verifier.BTE_022_STATUS), json.dumps(bte_status, indent=2) + "\n")
        findings = verifier.scan_root(root)
    assert any("durable_source_selection_source_only_guardrail_status keys" in finding for finding in findings)


def test_malformed_required_check_is_a_finding() -> None:
    verifier = load_verifier()
    with tempfile.TemporaryDirectory() as tmp:
        root = Path(tmp)
        write_complete_fixture(root)
        write_file(
            root,
            "specs/023-nt-research-analytics-platform/reference/backfill-source-proofs/pmxt-polymarket-v2-current/source-universe-source-proofs.toml",
            source_proof_spec_text().replace(
                '[required_checks.source_access]\noutcome = "passed"\nevidence_ref = "source"',
                'required_checks.source_access = "passed"',
            ),
        )
        findings = verifier.scan_root(root)
    assert any("required_checks.source_access must be an object" in finding for finding in findings)


def test_extra_required_check_is_a_finding() -> None:
    verifier = load_verifier()
    with tempfile.TemporaryDirectory() as tmp:
        root = Path(tmp)
        write_complete_fixture(root)
        write_file(
            root,
            "specs/023-nt-research-analytics-platform/reference/backfill-source-proofs/pmxt-polymarket-v2-current/source-universe-source-proofs.toml",
            f'{source_proof_spec_text()}\n[required_checks.production_promote]\noutcome = "passed"\nevidence_ref = "bad"\n',
        )
        findings = verifier.scan_root(root)
    assert any("required_checks keys" in finding for finding in findings)


def test_required_check_acceptance_provenance_is_a_finding() -> None:
    verifier = load_verifier()
    with tempfile.TemporaryDirectory() as tmp:
        root = Path(tmp)
        write_complete_fixture(root)
        write_file(
            root,
            "specs/023-nt-research-analytics-platform/reference/backfill-source-proofs/pmxt-polymarket-v2-current/source-universe-source-proofs.toml",
            source_proof_spec_text().replace(
                '[required_checks.source_access]\noutcome = "passed"\nevidence_ref = "source"',
                '[required_checks.source_access]\noutcome = "passed"\nevidence_ref = "source"\nsource_accepted_proof_count = 1',
            ),
        )
        findings = verifier.scan_root(root)
    assert any("required_checks.source_access" in finding and "source_accepted_proof_count" in finding for finding in findings)


def test_duplicate_pmxt_full_universe_is_a_finding() -> None:
    verifier = load_verifier()
    with tempfile.TemporaryDirectory() as tmp:
        root = Path(tmp)
        write_complete_fixture(root)
        ledger = f"""{venue_ledger_text()}
[[venue]]
venue_id = "pmxt-current-reference-duplicate"
venue = "pmxt"

[[venue.universe]]
universe_id = "pmxt-polymarket-full-current-data"
status = "accepted"
blocking_issues = []
"""
        write_file(
            root,
            "specs/023-nt-research-analytics-platform/reference/venue-scale-conversion-acceptance-ledgers/binance-bybit-pmxt-current/venue-scale-conversion-acceptance-ledger.toml",
            ledger,
        )
        findings = verifier.scan_root(root)
    assert any("expected exactly one pmxt-polymarket-full-current-data universe" in finding for finding in findings)


def test_acceptance_provenance_key_is_a_finding() -> None:
    verifier = load_verifier()
    with tempfile.TemporaryDirectory() as tmp:
        root = Path(tmp)
        write_complete_fixture(root)
        write_file(
            root,
            "specs/023-nt-research-analytics-platform/reference/backfill-source-proofs/pmxt-polymarket-v2-current/source-universe-source-proofs.toml",
            source_proof_spec_text().replace(
                'fidelity_class = "L2_REPLAY"',
                'fidelity_class = "L2_REPLAY"\nacceptance_record = "must-not-exist"',
            ),
        )
        findings = verifier.scan_root(root)
    assert any("acceptance provenance" in finding for finding in findings)


def test_source_accepted_proof_count_key_is_a_finding() -> None:
    verifier = load_verifier()
    with tempfile.TemporaryDirectory() as tmp:
        root = Path(tmp)
        write_complete_fixture(root)
        write_file(
            root,
            "specs/023-nt-research-analytics-platform/reference/backfill-source-proofs/pmxt-polymarket-v2-current/source-universe-source-proofs.toml",
            source_proof_spec_text().replace(
                'fidelity_class = "L2_REPLAY"',
                'fidelity_class = "L2_REPLAY"\nsource_accepted_proof_count = 1',
            ),
        )
        findings = verifier.scan_root(root)
    assert any("source_accepted_proof_count" in finding for finding in findings)


def test_source_binding_acceptance_provenance_key_is_a_finding() -> None:
    verifier = load_verifier()
    with tempfile.TemporaryDirectory() as tmp:
        root = Path(tmp)
        write_complete_fixture(root)
        write_file(
            root,
            "specs/023-nt-research-analytics-platform/reference/backfill-source-proofs/pmxt-polymarket-v2-current/source-universe-source-proofs.toml",
            source_proof_spec_text().replace(
                'source_proof_id = "source-proof-pmxt-polymarket-v2-current-orderbook"',
                'source_proof_id = "source-proof-pmxt-polymarket-v2-current-orderbook"\naccepted_by = "must-not-exist"',
            ),
        )
        findings = verifier.scan_root(root)
    assert any("source_binding must not carry acceptance provenance" in finding for finding in findings)


def test_malformed_source_binding_is_a_finding() -> None:
    verifier = load_verifier()
    with tempfile.TemporaryDirectory() as tmp:
        root = Path(tmp)
        write_complete_fixture(root)
        write_file(
            root,
            "specs/023-nt-research-analytics-platform/reference/backfill-source-proofs/pmxt-polymarket-v2-current/source-universe-source-proofs.toml",
            source_proof_spec_text()
            .split("[[source_binding]]")[0]
            .replace(
                'source_bindings_path = "specs/023-nt-research-analytics-platform/reference/backfill-source-bindings.v1.toml"',
                'source_bindings_path = "specs/023-nt-research-analytics-platform/reference/backfill-source-bindings.v1.toml"\nsource_binding = 1',
            ),
        )
        findings = verifier.scan_root(root)
    assert any("source_binding must be an array" in finding for finding in findings)


def test_status_hash_drift_is_a_finding() -> None:
    verifier = load_verifier()
    with tempfile.TemporaryDirectory() as tmp:
        root = Path(tmp)
        write_complete_fixture(root)
        write_file(
            root,
            "specs/023-nt-research-analytics-platform/reference/backfill-source-universe-object-manifests/pmxt-polymarket-v2-current/manifest/source-universe-object-manifest.json",
            f"{manifest_json()}\n",
        )
        findings = verifier.scan_root(root)
    assert any("source_universe_manifest.sha256" in finding for finding in findings)


def test_durable_status_nested_acceptance_drift_is_a_finding() -> None:
    verifier = load_verifier()
    with tempfile.TemporaryDirectory() as tmp:
        root = Path(tmp)
        write_complete_fixture(root)
        status_path = (
            "specs/023-nt-research-analytics-platform/reference/"
            "source-proof-pmxt-durable-source-selection-status.2026-06-16.json"
        )
        write_file(
            root,
            status_path,
            durable_status_json(root)
            .replace('"status": "pending"', '"status": "accepted"', 1)
            .replace('"usage_scope": "one_off_backfill_data"', '"usage_scope": "canonical_backfill_input"', 1),
        )
        findings = verifier.scan_root(root)
    assert any("source_proof_set_spec.status" in finding for finding in findings)
    assert any("source_proof_set_spec.usage_scope" in finding for finding in findings)


def test_durable_status_admissibility_drift_is_a_finding() -> None:
    verifier = load_verifier()
    with tempfile.TemporaryDirectory() as tmp:
        root = Path(tmp)
        write_complete_fixture(root)
        durable_status = json.loads(durable_status_json(root))
        admissibility_status = durable_status["source_proof_admissibility_status"]
        admissibility_status["expected_record_status"] = "accept_ready"
        admissibility_status["missing_current_contract_fields"] = ["acceptance_scope"]
        admissibility_status["blocking_issues"] = []
        admissibility_status["acceptance_error"] = None
        write_file(root, str(verifier.PMXT_DURABLE_STATUS), json.dumps(durable_status, indent=2) + "\n")
        findings = verifier.scan_root(root)
    assert any("source_proof_admissibility_status.expected_record_status" in finding for finding in findings)
    assert any("source_proof_admissibility_status.missing_current_contract_fields" in finding for finding in findings)
    assert any("source_proof_admissibility_status.blocking_issues" in finding for finding in findings)
    assert any("source_proof_admissibility_status.acceptance_error" in finding for finding in findings)


def test_durable_status_empty_nested_block_is_a_finding() -> None:
    verifier = load_verifier()
    nested_keys = (
        "source_proof_set_spec",
        "source_proof_admissibility_status",
        "committed_input_hashes",
        "manifest_scope",
        "generated_artifact_policy",
        "guard_verification",
    )
    for key in nested_keys:
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            write_complete_fixture(root)
            durable_status = json.loads(durable_status_json(root))
            durable_status[key] = {}
            write_file(root, str(verifier.PMXT_DURABLE_STATUS), json.dumps(durable_status, indent=2) + "\n")
            findings = verifier.scan_root(root)
        assert any(key in finding for finding in findings), (key, findings)


def test_durable_status_remaining_blockers_drift_is_a_finding() -> None:
    verifier = load_verifier()
    with tempfile.TemporaryDirectory() as tmp:
        root = Path(tmp)
        write_complete_fixture(root)
        status_path = (
            "specs/023-nt-research-analytics-platform/reference/"
            "source-proof-pmxt-durable-source-selection-status.2026-06-16.json"
        )
        write_file(
            root,
            status_path,
            durable_status_json(root).replace(
                '  "remaining_blockers": [\n'
                '    "durable_source_selection_unproven",\n'
                '    "expanded_tranche_coverage_and_cost_unproven",\n'
                '    "dynamic_tick_size_replay_unproven",\n'
                '    "broad_backfill_efficiency_unproven"\n'
                "  ],",
                '  "remaining_blockers": [],',
            ),
        )
        findings = verifier.scan_root(root)
    assert any("remaining_blockers" in finding for finding in findings)


def test_bte_status_remaining_blockers_drift_is_a_finding() -> None:
    verifier = load_verifier()
    with tempfile.TemporaryDirectory() as tmp:
        root = Path(tmp)
        write_complete_fixture(root)
        bte_status = json.loads(bte_status_json())
        bte_status["remaining_blockers"] = [
            blocker for blocker in bte_status["remaining_blockers"]
            if blocker["blocker"] != "broad_backfill_efficiency_unproven"
        ]
        write_file(root, str(verifier.BTE_022_STATUS), json.dumps(bte_status, indent=2) + "\n")
        findings = verifier.scan_root(root)
    assert any("remaining_blockers.blocker_names" in finding for finding in findings)


def test_durable_status_extra_overclaim_key_is_a_finding() -> None:
    verifier = load_verifier()
    with tempfile.TemporaryDirectory() as tmp:
        root = Path(tmp)
        write_complete_fixture(root)
        status_path = (
            "specs/023-nt-research-analytics-platform/reference/"
            "source-proof-pmxt-durable-source-selection-status.2026-06-16.json"
        )
        write_file(
            root,
            status_path,
            durable_status_json(root).replace(
                '  "bte_022_can_close": false',
                '  "pmxt_source_accepted": true,\n  "bte_022_can_close": false',
            ),
        )
        findings = verifier.scan_root(root)
    assert any("top-level keys" in finding for finding in findings)


def test_commented_gitignore_eviction_pattern_is_a_finding() -> None:
    verifier = load_verifier()
    with tempfile.TemporaryDirectory() as tmp:
        root = Path(tmp)
        write_complete_fixture(root)
        write_file(
            root,
            ".gitignore",
            "\n".join(
                [
                    "# specs/023-nt-research-analytics-platform/reference/source-universe-conversion-queues/pmxt-polymarket-v2-current/queue/*.json",
                    "# specs/023-nt-research-analytics-platform/reference/backfill-source-proofs/pmxt-polymarket-v2-current/*.json",
                    "# specs/023-nt-research-analytics-platform/reference/venue-scale-conversion-acceptance-ledgers/*/ledger/*.json",
                ]
            ),
        )
        findings = verifier.scan_root(root)
    assert any(".gitignore" in finding for finding in findings)


def test_gitignore_negation_of_pmxt_artifact_is_a_finding() -> None:
    verifier = load_verifier()
    with tempfile.TemporaryDirectory() as tmp:
        root = Path(tmp)
        write_complete_fixture(root)
        write_file(
            root,
            ".gitignore",
            "\n".join(
                [
                    "specs/023-nt-research-analytics-platform/reference/source-universe-conversion-queues/pmxt-polymarket-v2-current/queue/*.json",
                    "specs/023-nt-research-analytics-platform/reference/backfill-source-proofs/pmxt-polymarket-v2-current/*.json",
                    "!specs/023-nt-research-analytics-platform/reference/backfill-source-proofs/pmxt-polymarket-v2-current/source-universe-source-proof-set.json",
                    "specs/023-nt-research-analytics-platform/reference/venue-scale-conversion-acceptance-ledgers/*/ledger/*.json",
                ]
            ),
        )
        findings = verifier.scan_root(root)
    assert any("must effectively ignore representative" in finding for finding in findings)


def test_gitignore_leading_slash_negation_of_pmxt_artifact_is_a_finding() -> None:
    verifier = load_verifier()
    with tempfile.TemporaryDirectory() as tmp:
        root = Path(tmp)
        write_complete_fixture(root)
        write_file(
            root,
            ".gitignore",
            "\n".join(
                [
                    "specs/023-nt-research-analytics-platform/reference/source-universe-conversion-queues/pmxt-polymarket-v2-current/queue/*.json",
                    "specs/023-nt-research-analytics-platform/reference/backfill-source-proofs/pmxt-polymarket-v2-current/*.json",
                    "!/specs/023-nt-research-analytics-platform/reference/backfill-source-proofs/pmxt-polymarket-v2-current/source-universe-source-proof-set.json",
                    "specs/023-nt-research-analytics-platform/reference/venue-scale-conversion-acceptance-ledgers/*/ledger/*.json",
                ]
            ),
        )
        findings = verifier.scan_root(root)
    assert any("must effectively ignore representative" in finding for finding in findings)


def test_source_fence_command_outside_recipe_is_a_finding() -> None:
    verifier = load_verifier()
    with tempfile.TemporaryDirectory() as tmp:
        root = Path(tmp)
        write_complete_fixture(root)
        write_file(
            root,
            "justfile",
            (
                "verify-bte-022-pmxt-durable-source:\n"
                "    python3 scripts/test_verify_bte_022_pmxt_durable_source.py\n"
                "    python3 scripts/verify_bte_022_pmxt_durable_source.py\n"
                "other:\n"
                "    python3 scripts/test_verify_bte_022_pmxt_durable_source.py\n"
                "source-fence-static-inner:\n"
                "    python3 scripts/verify_bte_022_pmxt_durable_source.py\n"
            ),
        )
        findings = verifier.scan_root(root)
    assert any(
        "source-fence-static-inner must run python3 scripts/test_verify_bte_022_pmxt_durable_source.py" in finding
        for finding in findings
    )


def test_cli_fails_with_actionable_output() -> None:
    with tempfile.TemporaryDirectory() as tmp:
        root = Path(tmp)
        write_complete_fixture(root)
        write_file(
            root,
            "specs/023-nt-research-analytics-platform/reference/source-proof-fixture.binary-option.polymarket-pmxt-official-free-pending.v1.json",
            source_fixture_json(usage_scope="canonical_backfill_input"),
        )
        result = run_script("--root", str(root))
    assert result.returncode == 1
    assert "FAIL:" in result.stderr
    assert "usage_scope" in result.stderr


def main() -> int:
    tests = [
        test_complete_fixture_passes,
        test_json_explanatory_text_can_be_reworded,
        test_accepting_pmxt_source_proof_is_a_finding,
        test_missing_accepted_source_blocker_is_a_finding,
        test_bte_close_claim_is_a_finding,
        test_bte_top_level_status_overclaim_is_a_finding,
        test_bte_status_durable_guard_overclaim_is_a_finding,
        test_bte_status_empty_durable_guard_is_a_finding,
        test_bte_status_extra_durable_guard_key_is_a_finding,
        test_malformed_required_check_is_a_finding,
        test_extra_required_check_is_a_finding,
        test_required_check_acceptance_provenance_is_a_finding,
        test_duplicate_pmxt_full_universe_is_a_finding,
        test_acceptance_provenance_key_is_a_finding,
        test_source_accepted_proof_count_key_is_a_finding,
        test_source_binding_acceptance_provenance_key_is_a_finding,
        test_malformed_source_binding_is_a_finding,
        test_status_hash_drift_is_a_finding,
        test_durable_status_nested_acceptance_drift_is_a_finding,
        test_durable_status_admissibility_drift_is_a_finding,
        test_durable_status_empty_nested_block_is_a_finding,
        test_durable_status_remaining_blockers_drift_is_a_finding,
        test_bte_status_remaining_blockers_drift_is_a_finding,
        test_durable_status_extra_overclaim_key_is_a_finding,
        test_commented_gitignore_eviction_pattern_is_a_finding,
        test_gitignore_negation_of_pmxt_artifact_is_a_finding,
        test_gitignore_leading_slash_negation_of_pmxt_artifact_is_a_finding,
        test_source_fence_command_outside_recipe_is_a_finding,
        test_cli_fails_with_actionable_output,
    ]
    for test in tests:
        test()
    print("OK: BTE-022 PMXT durable-source verifier self-tests passed.")
    return 0


if __name__ == "__main__":
    import lane_governor

    lane_governor.acquire()
    raise SystemExit(main())
