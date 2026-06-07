use std::process::Command;

use backtesting_vertical_slice::{
    backfill_binding_coverage::{
        BackfillBindingCoverageBinding, BackfillBindingCoverageIssue,
        BackfillBindingCoverageReport, BackfillBindingCoverageStatus,
    },
    backfill_preflight::{
        BackfillPreflightBlockingReason, BackfillPreflightSelectedRecord,
        BackfillPreflightSelection, BackfillPreflightStatus,
    },
    backfill_readiness::{
        BACKFILL_READINESS_REPORT_FILE, BackfillReadinessBlocker, BackfillReadinessStatus,
        evaluate_backfill_readiness, write_backfill_readiness_report_from_spec_file,
    },
    source_proof_legacy_derivability::{
        SourceProofLegacyDerivabilityIssue, SourceProofLegacyDerivableField,
    },
    source_proof_migration_preflight::{
        SourceProofMigrationPreflightCandidate, SourceProofMigrationPreflightReason,
        SourceProofMigrationPreflightSelection, SourceProofMigrationPreflightStatus,
    },
};

#[test]
fn readiness_blocks_when_backfill_and_source_proof_preflights_block() {
    let report = evaluate_backfill_readiness(
        "synthetic-readiness",
        blocked_backfill_preflight(),
        blocked_source_proof_preflight(),
        ready_binding_coverage(),
        "trades",
        "TradeTick",
    );

    assert_eq!(report.status, BackfillReadinessStatus::Blocked);
    assert!(
        report
            .blockers
            .contains(&BackfillReadinessBlocker::BackfillPreflightBlocked)
    );
    assert!(
        report
            .blockers
            .contains(&BackfillReadinessBlocker::SourceProofMigrationPreflightBlocked)
    );
    assert!(report.selected_backfill_record.is_none());
    assert!(report.selected_source_proof_candidate.is_none());
}

#[test]
fn readiness_requires_selected_source_proof_table_family_to_match_requested_path() {
    let report = evaluate_backfill_readiness(
        "synthetic-readiness",
        ready_backfill_preflight(),
        candidate_source_proof_preflight("instruments"),
        ready_binding_coverage(),
        "trades",
        "TradeTick",
    );

    assert_eq!(report.status, BackfillReadinessStatus::Blocked);
    assert!(
        report
            .blockers
            .contains(&BackfillReadinessBlocker::SourceProofTableFamilyMismatch)
    );
}

#[test]
fn readiness_blocks_when_binding_coverage_blocks() {
    let report = evaluate_backfill_readiness(
        "synthetic-readiness",
        ready_backfill_preflight(),
        candidate_source_proof_preflight("trades"),
        blocked_binding_coverage(),
        "trades",
        "TradeTick",
    );

    assert_eq!(report.status, BackfillReadinessStatus::Blocked);
    assert!(
        report
            .blockers
            .contains(&BackfillReadinessBlocker::BackfillBindingCoverageBlocked)
    );
}

#[test]
fn readiness_blocks_when_selected_source_bindings_differ() {
    let report = evaluate_backfill_readiness(
        "synthetic-readiness",
        ready_backfill_preflight_with_binding("synthetic-backfill-binding"),
        candidate_source_proof_preflight_with_binding("trades", "synthetic-proof-binding"),
        ready_binding_coverage_for_binding("synthetic-backfill-binding"),
        "trades",
        "TradeTick",
    );

    assert_eq!(report.status, BackfillReadinessStatus::Blocked);
    assert!(
        report
            .blockers
            .contains(&BackfillReadinessBlocker::SelectedSourceBindingMismatch)
    );
}

#[test]
fn readiness_blocks_when_binding_coverage_lacks_selected_binding() {
    let report = evaluate_backfill_readiness(
        "synthetic-readiness",
        ready_backfill_preflight_with_binding("synthetic-selected-binding"),
        candidate_source_proof_preflight_with_binding("trades", "synthetic-selected-binding"),
        ready_binding_coverage_for_binding("synthetic-other-binding"),
        "trades",
        "TradeTick",
    );

    assert_eq!(report.status, BackfillReadinessStatus::Blocked);
    assert!(
        report
            .blockers
            .contains(&BackfillReadinessBlocker::SelectedSourceBindingMissingFromCoverage)
    );
}

#[test]
fn readiness_blocks_when_selected_source_proof_id_differs() {
    let report = evaluate_backfill_readiness(
        "synthetic-readiness",
        ready_backfill_preflight_with_proof("synthetic-source-binding", "source-proof-backfill", 1),
        candidate_source_proof_preflight_with_proof(
            "trades",
            "synthetic-source-binding",
            "source-proof-candidate",
            1,
        ),
        ready_binding_coverage(),
        "trades",
        "TradeTick",
    );

    assert_eq!(report.status, BackfillReadinessStatus::Blocked);
    assert!(
        report
            .blockers
            .contains(&BackfillReadinessBlocker::SelectedSourceProofMismatch)
    );
}

#[test]
fn readiness_blocks_when_selected_source_proof_version_differs() {
    let report = evaluate_backfill_readiness(
        "synthetic-readiness",
        ready_backfill_preflight_with_proof(
            "synthetic-source-binding",
            "source-proof-synthetic",
            1,
        ),
        candidate_source_proof_preflight_with_proof(
            "trades",
            "synthetic-source-binding",
            "source-proof-synthetic",
            2,
        ),
        ready_binding_coverage(),
        "trades",
        "TradeTick",
    );

    assert_eq!(report.status, BackfillReadinessStatus::Blocked);
    assert!(
        report
            .blockers
            .contains(&BackfillReadinessBlocker::SelectedSourceProofMismatch)
    );
}

#[test]
fn readiness_is_ready_when_backfill_and_source_proof_preflights_align() {
    let report = evaluate_backfill_readiness(
        "synthetic-readiness",
        ready_backfill_preflight(),
        candidate_source_proof_preflight("trades"),
        ready_binding_coverage(),
        "trades",
        "TradeTick",
    );

    assert_eq!(report.status, BackfillReadinessStatus::Ready);
    assert!(report.blockers.is_empty());
    assert_eq!(
        report
            .selected_source_proof_candidate
            .expect("candidate")
            .table_family,
        "trades"
    );
}

#[test]
fn readiness_reads_toml_spec_and_writes_report_idempotently() {
    let dir = tempfile::TempDir::new().expect("temp dir");
    let backfill_path = dir.path().join("backfill-preflight.json");
    let source_proof_path = dir.path().join("source-proof-preflight.json");
    let binding_coverage_path = dir.path().join("binding-coverage.json");
    let output_dir = dir.path().join("out");
    let spec_path = dir.path().join("readiness.toml");
    std::fs::write(
        &backfill_path,
        serde_json::to_vec_pretty(&ready_backfill_preflight()).expect("backfill json"),
    )
    .expect("write backfill");
    std::fs::write(
        &source_proof_path,
        serde_json::to_vec_pretty(&candidate_source_proof_preflight("trades"))
            .expect("source proof json"),
    )
    .expect("write source proof");
    std::fs::write(
        &binding_coverage_path,
        serde_json::to_vec_pretty(&ready_binding_coverage()).expect("binding coverage json"),
    )
    .expect("write binding coverage");
    std::fs::write(
        &spec_path,
        format!(
            r#"readiness_id = "synthetic-readiness"
backfill_preflight_report_path = "{}"
source_proof_migration_preflight_report_path = "{}"
backfill_binding_coverage_report_path = "{}"
output_dir = "{}"
required_table_family = "trades"
required_nt_data_type = "TradeTick"
"#,
            backfill_path.display(),
            source_proof_path.display(),
            binding_coverage_path.display(),
            output_dir.display()
        ),
    )
    .expect("write spec");

    let first = write_backfill_readiness_report_from_spec_file(&spec_path).expect("first");
    let second = write_backfill_readiness_report_from_spec_file(&spec_path).expect("second");

    assert_eq!(first.content_hash, second.content_hash);
    assert_eq!(first.path, output_dir.join(BACKFILL_READINESS_REPORT_FILE));
}

#[test]
fn readiness_cli_writes_blocked_report() {
    let dir = tempfile::TempDir::new().expect("temp dir");
    let backfill_path = dir.path().join("backfill-preflight.json");
    let source_proof_path = dir.path().join("source-proof-preflight.json");
    let binding_coverage_path = dir.path().join("binding-coverage.json");
    let output_dir = dir.path().join("out");
    let spec_path = dir.path().join("readiness.toml");
    std::fs::write(
        &backfill_path,
        serde_json::to_vec_pretty(&blocked_backfill_preflight()).expect("backfill json"),
    )
    .expect("write backfill");
    std::fs::write(
        &source_proof_path,
        serde_json::to_vec_pretty(&blocked_source_proof_preflight()).expect("source proof json"),
    )
    .expect("write source proof");
    std::fs::write(
        &binding_coverage_path,
        serde_json::to_vec_pretty(&blocked_binding_coverage()).expect("binding coverage json"),
    )
    .expect("write binding coverage");
    std::fs::write(
        &spec_path,
        format!(
            r#"readiness_id = "synthetic-readiness"
backfill_preflight_report_path = "{}"
source_proof_migration_preflight_report_path = "{}"
backfill_binding_coverage_report_path = "{}"
output_dir = "{}"
required_table_family = "trades"
required_nt_data_type = "TradeTick"
"#,
            backfill_path.display(),
            source_proof_path.display(),
            binding_coverage_path.display(),
            output_dir.display()
        ),
    )
    .expect("write spec");

    let binary = std::env::var("CARGO_BIN_EXE_backfill_readiness").expect("binary path");
    let output = Command::new(binary)
        .arg("--spec")
        .arg(&spec_path)
        .output()
        .expect("run CLI");

    assert!(
        output.status.success(),
        "stdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    let report_path = output_dir.join(BACKFILL_READINESS_REPORT_FILE);
    let report: backtesting_vertical_slice::backfill_readiness::BackfillReadinessReport =
        serde_json::from_slice(&std::fs::read(report_path).expect("readiness report"))
            .expect("readiness json");
    assert_eq!(report.status, BackfillReadinessStatus::Blocked);
}

fn blocked_backfill_preflight()
-> backtesting_vertical_slice::backfill_preflight::BackfillPreflightReport {
    backtesting_vertical_slice::backfill_preflight::BackfillPreflightReport {
        schema_version: "backfill-preflight-report.v1".to_string(),
        preflight_id: "synthetic-backfill-preflight".to_string(),
        coverage_ledger_id: "synthetic-ledger".to_string(),
        status: BackfillPreflightStatus::Blocked,
        selection: BackfillPreflightSelection {
            max_accepted_objects: 1,
            max_accepted_bytes: 100,
            require_canonical_ready: true,
            allow_gaps: false,
        },
        total_records: 1,
        accepted_records: 0,
        accepted_with_gaps_records: 0,
        canonical_ready_records: 0,
        eligible_record_count: 0,
        selected_record: None,
        blocking_reasons: vec![BackfillPreflightBlockingReason::NoAcceptedRecords],
    }
}

fn ready_backfill_preflight()
-> backtesting_vertical_slice::backfill_preflight::BackfillPreflightReport {
    ready_backfill_preflight_with_binding("synthetic-source-binding")
}

fn ready_backfill_preflight_with_binding(
    source_binding: &str,
) -> backtesting_vertical_slice::backfill_preflight::BackfillPreflightReport {
    ready_backfill_preflight_with_proof(source_binding, "source-proof-synthetic", 1)
}

fn ready_backfill_preflight_with_proof(
    source_binding: &str,
    source_proof_id: &str,
    source_proof_version: u32,
) -> backtesting_vertical_slice::backfill_preflight::BackfillPreflightReport {
    backtesting_vertical_slice::backfill_preflight::BackfillPreflightReport {
        schema_version: "backfill-preflight-report.v1".to_string(),
        preflight_id: "synthetic-backfill-preflight".to_string(),
        coverage_ledger_id: "synthetic-ledger".to_string(),
        status: BackfillPreflightStatus::Go,
        selection: BackfillPreflightSelection {
            max_accepted_objects: 1,
            max_accepted_bytes: 100,
            require_canonical_ready: true,
            allow_gaps: false,
        },
        total_records: 1,
        accepted_records: 1,
        accepted_with_gaps_records: 0,
        canonical_ready_records: 1,
        eligible_record_count: 1,
        selected_record: Some(BackfillPreflightSelectedRecord {
            record_id: "synthetic-backfill-record".to_string(),
            source_binding: source_binding.to_string(),
            source_proof_id: source_proof_id.to_string(),
            source_proof_version,
            accepted_objects: 1,
            accepted_bytes: 100,
            skipped_objects: 0,
            canonical_ready: true,
        }),
        blocking_reasons: Vec::new(),
    }
}

fn blocked_source_proof_preflight()
-> backtesting_vertical_slice::source_proof_migration_preflight::SourceProofMigrationPreflightReport
{
    backtesting_vertical_slice::source_proof_migration_preflight::SourceProofMigrationPreflightReport {
        schema_version: "source-proof-migration-preflight-report.v1".to_string(),
        preflight_id: "synthetic-source-proof-preflight".to_string(),
        derivability_report_id: "synthetic-derivability".to_string(),
        status: SourceProofMigrationPreflightStatus::Blocked,
        selection: source_proof_selection(),
        total_records: 1,
        eligible_candidate_count: 0,
        selected_candidate: None,
        blocking_reasons: vec![SourceProofMigrationPreflightReason::NoEligibleCandidate],
    }
}

fn candidate_source_proof_preflight(
    table_family: &str,
) -> backtesting_vertical_slice::source_proof_migration_preflight::SourceProofMigrationPreflightReport
{
    candidate_source_proof_preflight_with_binding(table_family, "synthetic-source-binding")
}

fn candidate_source_proof_preflight_with_binding(
    table_family: &str,
    source_binding: &str,
) -> backtesting_vertical_slice::source_proof_migration_preflight::SourceProofMigrationPreflightReport
{
    candidate_source_proof_preflight_with_proof(
        table_family,
        source_binding,
        "source-proof-synthetic",
        1,
    )
}

fn candidate_source_proof_preflight_with_proof(
    table_family: &str,
    source_binding: &str,
    source_proof_id: &str,
    source_proof_version: u32,
) -> backtesting_vertical_slice::source_proof_migration_preflight::SourceProofMigrationPreflightReport
{
    backtesting_vertical_slice::source_proof_migration_preflight::SourceProofMigrationPreflightReport {
        schema_version: "source-proof-migration-preflight-report.v1".to_string(),
        preflight_id: "synthetic-source-proof-preflight".to_string(),
        derivability_report_id: "synthetic-derivability".to_string(),
        status: SourceProofMigrationPreflightStatus::CandidateFound,
        selection: source_proof_selection(),
        total_records: 1,
        eligible_candidate_count: 1,
        selected_candidate: Some(SourceProofMigrationPreflightCandidate {
            proof_uri: "proof://synthetic/source-proof.json".to_string(),
            source_proof_id: source_proof_id.to_string(),
            source_proof_version,
            source_binding: source_binding.to_string(),
            table_family: table_family.to_string(),
            raw_payload_records: 1,
            s3_bound_raw_payload_records: 1,
            accepted_bytes_from_s3: 100,
            derivable_fields: vec![SourceProofLegacyDerivableField::SourceBinding],
            remaining_acceptance_blockers: vec![
                SourceProofLegacyDerivabilityIssue::LicenseNotPassed,
            ],
        }),
        blocking_reasons: Vec::new(),
    }
}

fn blocked_binding_coverage()
-> backtesting_vertical_slice::backfill_binding_coverage::BackfillBindingCoverageReport {
    let mut report = ready_binding_coverage();
    report.status = BackfillBindingCoverageStatus::Blocked;
    report.blocking_issues = vec![BackfillBindingCoverageIssue::EmptySourceBindingRecords];
    report
}

fn ready_binding_coverage()
-> backtesting_vertical_slice::backfill_binding_coverage::BackfillBindingCoverageReport {
    ready_binding_coverage_for_binding("synthetic-source-binding")
}

fn ready_binding_coverage_for_binding(
    source_binding: &str,
) -> backtesting_vertical_slice::backfill_binding_coverage::BackfillBindingCoverageReport {
    BackfillBindingCoverageReport {
        schema_version: "backfill-binding-coverage-report.v1".to_string(),
        report_id: "synthetic-binding-coverage".to_string(),
        status: BackfillBindingCoverageStatus::Ready,
        required_table_families: vec!["trades".to_string()],
        configured_required_binding_count: 1,
        ledger_records_for_required_bindings: 1,
        empty_source_binding_record_count: 0,
        unconfigured_source_bindings: Vec::new(),
        bindings: vec![BackfillBindingCoverageBinding {
            key: source_binding.to_string(),
            table_families: vec!["trades".to_string()],
            required_table_family_match: true,
            ledger_record_count: 1,
            canonical_ready_record_count: 1,
            accepted_record_count: 1,
        }],
        blocking_issues: Vec::new(),
    }
}

fn source_proof_selection() -> SourceProofMigrationPreflightSelection {
    SourceProofMigrationPreflightSelection {
        allowed_table_families: vec!["trades".to_string()],
        required_derivable_fields: vec![SourceProofLegacyDerivableField::SourceBinding],
        max_raw_payload_records: 1,
        max_accepted_bytes_from_s3: 100,
        require_single_table_family: true,
        require_s3_bound_payloads: true,
    }
}
