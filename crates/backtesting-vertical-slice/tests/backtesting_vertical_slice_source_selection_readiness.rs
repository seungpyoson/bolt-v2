use backtesting_vertical_slice::{
    source_proof::{
        CONTRACT_VERSION, CheckOutcome, FixtureType, NtMappingStatus, RequiredCheck,
        SOURCE_PROOF_SCHEMA_VERSION, SourceBindingRegistry, SourceProofFidelityClass,
        SourceProofReport, SourceProofUsageScope,
    },
    source_selection_readiness::{
        SourceSelectionReadinessBlocker, SourceSelectionReadinessInput,
        SourceSelectionReadinessStatus, evaluate_source_selection_readiness,
    },
};

#[test]
fn source_selection_readiness_accepts_generic_accepted_proof_with_recorded_evidence() {
    let proof = accepted_source_proof();
    let report = evaluate_source_selection_readiness(SourceSelectionReadinessInput {
        selection_id: "selection-readiness-synthetic",
        source_proof_hash: "source-proof-hash",
        source_proof: &proof,
        source_bindings_registry: &source_bindings_registry(),
        required_fixture_type: FixtureType::PerpsSpot,
        required_table_family: "trades",
        allowed_fidelity_classes: vec![SourceProofFidelityClass::TradeReplay],
        allow_lower_fidelity: true,
    });

    assert_eq!(report.status, SourceSelectionReadinessStatus::Ready);
    assert!(report.blockers.is_empty());
    assert!(report.source_proof_accepted);
    assert!(report.canonical_usage_scope_proven);
    assert!(report.source_access_proven);
    assert!(report.license_proven);
    assert!(report.sample_schema_proven);
    assert!(report.nt_mapping_proven);
    assert!(report.cost_proven);
    assert!(report.storage_proven);
    assert!(report.claim_limits_recorded);
}

#[test]
fn source_selection_readiness_blocks_one_off_bootstrap_data_from_durable_selection() {
    let mut proof = accepted_source_proof();
    proof.usage_scope = SourceProofUsageScope::OneOffBackfillData;

    let report = evaluate_source_selection_readiness(SourceSelectionReadinessInput {
        selection_id: "selection-readiness-synthetic",
        source_proof_hash: "source-proof-hash",
        source_proof: &proof,
        source_bindings_registry: &source_bindings_registry(),
        required_fixture_type: FixtureType::PerpsSpot,
        required_table_family: "trades",
        allowed_fidelity_classes: vec![SourceProofFidelityClass::TradeReplay],
        allow_lower_fidelity: true,
    });

    assert_eq!(report.status, SourceSelectionReadinessStatus::Blocked);
    assert!(
        report
            .blockers
            .contains(&SourceSelectionReadinessBlocker::SourceProofUsageScopeNotCanonical)
    );
    assert!(
        report
            .blockers
            .contains(&SourceSelectionReadinessBlocker::SourceProofAcceptanceFailed)
    );
}

#[test]
fn source_selection_readiness_blocks_when_nt_mapping_proof_is_missing() {
    let mut proof = accepted_source_proof();
    proof.nt_mapping_status = NtMappingStatus::Pending;
    proof.required_checks.nt_mapping = RequiredCheck {
        outcome: CheckOutcome::Pending,
        evidence_ref: "pending://nt-mapping".to_string(),
        expires_at_utc: None,
    };

    let report = evaluate_source_selection_readiness(SourceSelectionReadinessInput {
        selection_id: "selection-readiness-synthetic",
        source_proof_hash: "source-proof-hash",
        source_proof: &proof,
        source_bindings_registry: &source_bindings_registry(),
        required_fixture_type: FixtureType::PerpsSpot,
        required_table_family: "trades",
        allowed_fidelity_classes: vec![SourceProofFidelityClass::TradeReplay],
        allow_lower_fidelity: true,
    });

    assert_eq!(report.status, SourceSelectionReadinessStatus::Blocked);
    assert_eq!(report.unmet_required_checks, vec!["nt_mapping"]);
    assert!(!report.nt_mapping_proven);
    assert!(
        report
            .blockers
            .contains(&SourceSelectionReadinessBlocker::NtMappingNotAccepted)
    );
    assert!(
        report
            .blockers
            .contains(&SourceSelectionReadinessBlocker::RequiredChecksUnmet)
    );
}

fn source_bindings_registry() -> SourceBindingRegistry {
    SourceBindingRegistry::from_toml_str(
        r#"
[[source_binding]]
key = "synthetic-source-trades"
venue = "synthetic-venue"
product_family = "spot"
market_structure_fixture = "perps-spot"
source_uri = "https://source.example.test/data/{symbol}/{dt}.zip"
evidence_state = "directly_backfillable"
table_families = ["trades"]
"#,
    )
    .expect("source binding registry")
}

fn accepted_source_proof() -> SourceProofReport {
    let forbidden_claim =
        "No execution-quality, queue-position, fillability, or book-liquidity claims.";
    serde_json::from_value(serde_json::json!({
        "source_proof_id": "source-proof-synthetic-trades",
        "source_proof_version": 1,
        "contract_version": CONTRACT_VERSION,
        "schema_version": SOURCE_PROOF_SCHEMA_VERSION,
        "status": "accepted",
        "source_binding": "synthetic-source-trades",
        "venue": "synthetic-venue",
        "product_family": "spot",
        "product_category": "spot",
        "table_family": "trades",
        "evidence_state": "directly_backfillable",
        "source_candidate_class": "official_free",
        "source_selection_status": "ACCEPTED_LOWER_FIDELITY",
        "usage_scope": "canonical_backfill_input",
        "fixture_type": "perps-spot",
        "requested_time_range": {
            "start_utc": "2026-03-01T00:00:00Z",
            "end_utc": "2026-03-02T00:00:00Z"
        },
        "coverage_time_range": {
            "start_utc": "2026-03-01T00:00:00Z",
            "end_utc": "2026-03-02T00:00:00Z"
        },
        "instrument_universe_id": "synthetic-instrument-universe",
        "raw_sample_uri": "s3://artifact-root/raw/synthetic/object.zip",
        "raw_sample_hash": "raw-sample-hash",
        "schema_sample_uri": "s3://artifact-root/source-proofs/synthetic/schema.json",
        "schema_sample_hash": "schema-sample-hash",
        "license_ref": "s3://artifact-root/source-proofs/synthetic/license.txt",
        "license_scope": "public",
        "retention_ref": "s3://artifact-root/source-proofs/synthetic/retention.json",
        "cost_ref": "s3://artifact-root/source-proofs/synthetic/cost.json",
        "nt_mapping_status": "accepted",
        "fidelity_class": "TRADE_REPLAY",
        "l2_replay_evidence": {},
        "forbidden_claims": [forbidden_claim],
        "claim_limits": [{
            "id": "synthetic-claim-limit-001",
            "severity": "blocking",
            "claim": forbidden_claim,
            "reason": "Trade replay cannot prove execution-quality order-book behavior.",
            "evidence_ref": "s3://artifact-root/source-proofs/synthetic/claim-limits.json"
        }],
        "acceptance_scope": {
            "planned_objects": 1,
            "completed_objects": 1,
            "failed_objects": 0,
            "skipped_objects": 0,
            "accepted_bytes": 100,
            "selector_scope_violations": 0
        },
        "gap_policy_id": "",
        "required_checks": {
            "source_access": {
                "outcome": "passed",
                "evidence_ref": "s3://artifact-root/source-proofs/synthetic/source-access.json"
            },
            "license": {
                "outcome": "passed",
                "evidence_ref": "s3://artifact-root/source-proofs/synthetic/license.txt"
            },
            "schema": {
                "outcome": "passed",
                "evidence_ref": "s3://artifact-root/source-proofs/synthetic/schema.json"
            },
            "time_semantics": {
                "outcome": "passed",
                "evidence_ref": "s3://artifact-root/source-proofs/synthetic/time.json"
            },
            "instrument_universe": {
                "outcome": "passed",
                "evidence_ref": "s3://artifact-root/source-proofs/synthetic/instruments.json"
            },
            "coverage": {
                "outcome": "passed",
                "evidence_ref": "s3://artifact-root/source-proofs/synthetic/coverage.json"
            },
            "retention_freshness": {
                "outcome": "passed",
                "evidence_ref": "s3://artifact-root/source-proofs/synthetic/retention.json"
            },
            "granularity": {
                "outcome": "passed",
                "evidence_ref": "s3://artifact-root/source-proofs/synthetic/granularity.json"
            },
            "completeness": {
                "outcome": "passed",
                "evidence_ref": "s3://artifact-root/source-proofs/synthetic/completeness.json"
            },
            "nt_mapping": {
                "outcome": "passed",
                "evidence_ref": "s3://artifact-root/source-proofs/synthetic/nt-mapping.json"
            },
            "cost": {
                "outcome": "passed",
                "evidence_ref": "s3://artifact-root/source-proofs/synthetic/cost.json"
            },
            "storage": {
                "outcome": "passed",
                "evidence_ref": "s3://artifact-root/source-proofs/synthetic/storage.json"
            }
        },
        "acceptance_mode": "manual",
        "accepted_by": "synthetic-source-proof-operator",
        "accepted_at": "2026-03-02T00:00:00Z"
    }))
    .expect("accepted source proof")
}
