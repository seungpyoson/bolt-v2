use backtesting_vertical_slice::{
    source_proof::{
        FixtureType, SourceCandidateClass, SourceProofReport, SourceProofStatus,
        SourceSelectionStatus,
    },
    source_proof_shortlist::{
        SOURCE_PROOF_SHORTLIST_REPORT_FILE, SourceProofShortlistInput,
        SourceProofShortlistSelection, SourceProofShortlistStatus, evaluate_source_proof_shortlist,
        write_source_proof_shortlist_report_from_spec_file,
    },
};

#[test]
fn shortlist_uses_current_source_proof_reports_not_prose_or_legacy_records() {
    let proof = source_proof_report("source-binding-current", FixtureType::PerpsSpot, "trades");
    let report = evaluate_source_proof_shortlist(
        "synthetic-shortlist",
        vec![SourceProofShortlistInput {
            proof_uri: "proof://synthetic/current-source-proof.json".to_string(),
            proof,
        }],
        &SourceProofShortlistSelection {
            allowed_fixture_types: vec![FixtureType::PerpsSpot],
            allowed_table_families: vec!["trades".to_string()],
            allowed_candidate_classes: vec![SourceCandidateClass::OfficialFree],
            max_candidates: 4,
        },
    );

    assert_eq!(report.status, SourceProofShortlistStatus::CandidatesFound);
    assert!(report.blocking_reasons.is_empty());
    assert_eq!(report.candidates.len(), 1);
    let candidate = &report.candidates[0];
    assert_eq!(
        candidate.proof_uri,
        "proof://synthetic/current-source-proof.json"
    );
    assert_eq!(candidate.source_binding, "source-binding-current");
    assert_eq!(candidate.fixture_type, FixtureType::PerpsSpot);
    assert_eq!(candidate.table_family, "trades");
    assert_eq!(candidate.status, SourceProofStatus::Pending);
    assert_eq!(
        candidate.source_selection_status,
        SourceSelectionStatus::PendingMoreProof
    );
    assert!(
        candidate
            .remaining_required_checks
            .contains(&"license".to_string())
    );
    assert!(
        candidate
            .remaining_required_checks
            .contains(&"nt_mapping".to_string())
    );
}

#[test]
fn shortlist_blocks_when_no_current_source_proof_report_matches_selection() {
    let proof = source_proof_report("source-binding-other", FixtureType::BinaryOption, "events");
    let report = evaluate_source_proof_shortlist(
        "synthetic-shortlist",
        vec![SourceProofShortlistInput {
            proof_uri: "proof://synthetic/other-source-proof.json".to_string(),
            proof,
        }],
        &SourceProofShortlistSelection {
            allowed_fixture_types: vec![FixtureType::PerpsSpot],
            allowed_table_families: vec!["trades".to_string()],
            allowed_candidate_classes: vec![SourceCandidateClass::OfficialFree],
            max_candidates: 4,
        },
    );

    assert_eq!(report.status, SourceProofShortlistStatus::Blocked);
    assert!(report.candidates.is_empty());
}

#[test]
fn shortlist_writer_reads_source_proof_files_and_rejects_legacy_json() {
    let dir = tempfile::TempDir::new().expect("temp dir");
    let source_proof_path = dir.path().join("source-proof.json");
    let legacy_path = dir.path().join("legacy.json");
    let output_dir = dir.path().join("out");
    let spec_path = dir.path().join("shortlist.toml");
    let proof = source_proof_report("source-binding-current", FixtureType::PerpsSpot, "trades");
    std::fs::write(
        &source_proof_path,
        serde_json::to_vec_pretty(&proof).expect("proof json"),
    )
    .expect("write proof");
    std::fs::write(
        &spec_path,
        format!(
            r#"shortlist_id = "synthetic-shortlist"
output_dir = "{}"

[[source_proof]]
proof_uri = "proof://synthetic/current-source-proof.json"
path = "{}"

[selection]
allowed_fixture_types = ["perps-spot"]
allowed_table_families = ["trades"]
allowed_candidate_classes = ["official_free"]
max_candidates = 2
"#,
            output_dir.display(),
            source_proof_path.display()
        ),
    )
    .expect("write spec");

    let first = write_source_proof_shortlist_report_from_spec_file(&spec_path).expect("first");
    let second = write_source_proof_shortlist_report_from_spec_file(&spec_path).expect("second");
    assert_eq!(first.content_hash, second.content_hash);
    assert_eq!(
        first.path,
        output_dir.join(SOURCE_PROOF_SHORTLIST_REPORT_FILE)
    );

    let written: backtesting_vertical_slice::source_proof_shortlist::SourceProofShortlistReport =
        serde_json::from_slice(&std::fs::read(first.path).expect("read shortlist"))
            .expect("shortlist json");
    assert_eq!(written.candidates.len(), 1);

    std::fs::write(&legacy_path, br#"{"source_binding_key":"legacy"}"#).expect("write legacy");
    std::fs::write(
        &spec_path,
        format!(
            r#"shortlist_id = "synthetic-shortlist"
output_dir = "{}"

[[source_proof]]
proof_uri = "proof://synthetic/legacy.json"
path = "{}"

[selection]
allowed_fixture_types = ["perps-spot"]
allowed_table_families = ["trades"]
allowed_candidate_classes = ["official_free"]
max_candidates = 2
"#,
            output_dir.display(),
            legacy_path.display()
        ),
    )
    .expect("write legacy spec");
    let err = write_source_proof_shortlist_report_from_spec_file(&spec_path).unwrap_err();
    assert!(
        err.to_string()
            .contains("parse source-proof shortlist proof"),
        "unexpected error: {err}"
    );
}

fn source_proof_report(
    source_binding: &str,
    fixture_type: FixtureType,
    table_family: &str,
) -> SourceProofReport {
    serde_json::from_value(serde_json::json!({
        "source_proof_id": format!("source-proof-{source_binding}"),
        "source_proof_version": 1,
        "contract_version": "backfill-table-contract.v1",
        "schema_version": "backfill-source-proof.v1",
        "status": "pending",
        "source_binding": source_binding,
        "venue": "synthetic-venue",
        "product_family": "synthetic-product-family",
        "product_category": "synthetic-product-category",
        "table_family": table_family,
        "evidence_state": "directly_backfillable",
        "source_candidate_class": "official_free",
        "source_selection_status": "PENDING_MORE_PROOF",
        "fixture_type": fixture_type,
        "requested_time_range": {
            "start_utc": "2025-06-01T00:00:00Z",
            "end_utc": "2026-06-01T00:00:00Z"
        },
        "coverage_time_range": {
            "start_utc": "2026-06-08T00:00:00Z",
            "end_utc": "2026-06-08T00:00:00Z"
        },
        "instrument_universe_id": "synthetic-instrument-universe",
        "raw_sample_uri": "pending://source-proof/raw-sample",
        "raw_sample_hash": "pending-raw-sample",
        "schema_sample_uri": "pending://source-proof/schema-sample",
        "schema_sample_hash": "pending-schema-sample",
        "license_ref": "pending://source-proof/license",
        "retention_ref": "pending://source-proof/retention",
        "cost_ref": "pending://source-proof/cost",
        "nt_mapping_status": "pending",
        "fidelity_class": "TRADE_REPLAY",
        "l2_replay_evidence": {},
        "forbidden_claims": [
            "No NT catalog or backtest input from this pending proof."
        ],
        "claim_limits": [
            {
                "id": "source-proof-claim-limit-001",
                "severity": "blocking",
                "claim": "No NT catalog or backtest input from this pending proof.",
                "reason": "Pending source proof cannot feed canonical backtests.",
                "evidence_ref": "source-proof://synthetic/status"
            }
        ],
        "gap_policy_id": "",
        "required_checks": {
            "source_access": {
                "outcome": "pending",
                "evidence_ref": "pending"
            },
            "license": {
                "outcome": "pending",
                "evidence_ref": "pending"
            },
            "schema": {
                "outcome": "pending",
                "evidence_ref": "pending"
            },
            "time_semantics": {
                "outcome": "pending",
                "evidence_ref": "pending"
            },
            "instrument_universe": {
                "outcome": "pending",
                "evidence_ref": "pending"
            },
            "coverage": {
                "outcome": "pending",
                "evidence_ref": "pending"
            },
            "retention_freshness": {
                "outcome": "pending",
                "evidence_ref": "pending"
            },
            "granularity": {
                "outcome": "pending",
                "evidence_ref": "pending"
            },
            "completeness": {
                "outcome": "pending",
                "evidence_ref": "pending"
            },
            "nt_mapping": {
                "outcome": "pending",
                "evidence_ref": "pending"
            },
            "cost": {
                "outcome": "pending",
                "evidence_ref": "pending"
            },
            "storage": {
                "outcome": "pending",
                "evidence_ref": "pending"
            }
        }
    }))
    .expect("source proof report")
}
