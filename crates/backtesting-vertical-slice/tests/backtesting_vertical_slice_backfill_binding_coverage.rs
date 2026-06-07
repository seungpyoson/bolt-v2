use std::process::Command;

use backtesting_vertical_slice::{
    backfill_binding_coverage::{
        BACKFILL_BINDING_COVERAGE_REPORT_FILE, BackfillBindingCoverageIssue,
        BackfillBindingCoverageStatus, evaluate_backfill_binding_coverage,
        write_backfill_binding_coverage_report_from_spec_file,
    },
    backfill_coverage::{
        BackfillCoverageLedger, BackfillCoverageRecord, BackfillCoverageStatus,
        BackfillCoverageSummary,
    },
};

#[test]
fn binding_coverage_blocks_when_required_table_family_has_no_ledger_records() {
    let source_bindings = synthetic_source_bindings_toml();
    let ledger = ledger(vec![record("synthetic-other-binding", "trades")]);

    let report = evaluate_backfill_binding_coverage(
        "synthetic-binding-coverage",
        &source_bindings,
        &ledger,
        vec!["trades".to_string()],
    )
    .expect("report");

    assert_eq!(report.status, BackfillBindingCoverageStatus::Blocked);
    assert!(
        report
            .blocking_issues
            .contains(&BackfillBindingCoverageIssue::NoLedgerRecordsForRequiredTableFamily)
    );
    let required = report
        .bindings
        .iter()
        .find(|binding| binding.key == "synthetic-native-trades")
        .expect("required binding");
    assert_eq!(required.ledger_record_count, 0);
}

#[test]
fn binding_coverage_reports_bound_records_without_source_constants() {
    let source_bindings = synthetic_source_bindings_toml();
    let ledger = ledger(vec![
        accepted_record("synthetic-native-trades", "trades"),
        record("synthetic-native-trades", "trades"),
    ]);

    let report = evaluate_backfill_binding_coverage(
        "synthetic-binding-coverage",
        &source_bindings,
        &ledger,
        vec!["trades".to_string()],
    )
    .expect("report");

    assert_eq!(report.status, BackfillBindingCoverageStatus::Ready);
    assert!(report.blocking_issues.is_empty());
    let required = report
        .bindings
        .iter()
        .find(|binding| binding.key == "synthetic-native-trades")
        .expect("required binding");
    assert_eq!(required.ledger_record_count, 2);
    assert_eq!(required.accepted_record_count, 1);
}

#[test]
fn binding_coverage_blocks_unscoped_records_for_multi_family_binding() {
    let source_bindings = synthetic_multi_family_source_bindings_toml();
    let ledger = ledger(vec![accepted_unscoped_record(
        "synthetic-multi-family-source",
    )]);

    let report = evaluate_backfill_binding_coverage(
        "synthetic-binding-coverage",
        &source_bindings,
        &ledger,
        vec!["trades".to_string()],
    )
    .expect("report");

    assert_eq!(report.status, BackfillBindingCoverageStatus::Blocked);
    assert!(
        report
            .blocking_issues
            .contains(&BackfillBindingCoverageIssue::NoLedgerRecordsForRequiredTableFamily)
    );
    assert!(
        report
            .blocking_issues
            .contains(&BackfillBindingCoverageIssue::MissingTableFamilyRecords)
    );
    assert_eq!(report.missing_table_family_record_count, 1);
    let required = report
        .bindings
        .iter()
        .find(|binding| binding.key == "synthetic-multi-family-source")
        .expect("required binding");
    assert_eq!(required.ledger_record_count, 0);
}

#[test]
fn binding_coverage_blocks_unconfigured_source_bindings_even_with_required_records() {
    let source_bindings = synthetic_source_bindings_toml();
    let ledger = ledger(vec![
        accepted_record("synthetic-native-trades", "trades"),
        record("synthetic-unconfigured-trades", "trades"),
    ]);

    let report = evaluate_backfill_binding_coverage(
        "synthetic-binding-coverage",
        &source_bindings,
        &ledger,
        vec!["trades".to_string()],
    )
    .expect("report");

    assert_eq!(report.status, BackfillBindingCoverageStatus::Blocked);
    assert!(
        report
            .blocking_issues
            .contains(&BackfillBindingCoverageIssue::UnconfiguredSourceBindingRecords)
    );
    assert_eq!(
        report.unconfigured_source_bindings,
        vec!["synthetic-unconfigured-trades".to_string()]
    );
}

#[test]
fn binding_coverage_blocks_empty_source_bindings_even_with_required_records() {
    let source_bindings = synthetic_source_bindings_toml();
    let ledger = ledger(vec![
        accepted_record("synthetic-native-trades", "trades"),
        empty_binding_record("synthetic-empty-binding-record"),
    ]);

    let report = evaluate_backfill_binding_coverage(
        "synthetic-binding-coverage",
        &source_bindings,
        &ledger,
        vec!["trades".to_string()],
    )
    .expect("report");

    assert_eq!(report.status, BackfillBindingCoverageStatus::Blocked);
    assert!(
        report
            .blocking_issues
            .contains(&BackfillBindingCoverageIssue::EmptySourceBindingRecords)
    );
    assert_eq!(report.empty_source_binding_record_count, 1);
}

#[test]
fn binding_coverage_reads_toml_spec_and_writes_report_idempotently() {
    let dir = tempfile::TempDir::new().expect("temp dir");
    let source_bindings_path = dir.path().join("source-bindings.toml");
    let ledger_path = dir.path().join("ledger.json");
    let output_dir = dir.path().join("out");
    let spec_path = dir.path().join("binding-coverage.toml");
    std::fs::write(&source_bindings_path, synthetic_source_bindings_toml())
        .expect("write bindings");
    std::fs::write(
        &ledger_path,
        serde_json::to_vec_pretty(&ledger(vec![record("synthetic-native-trades", "trades")]))
            .expect("ledger json"),
    )
    .expect("write ledger");
    std::fs::write(
        &spec_path,
        format!(
            r#"report_id = "synthetic-binding-coverage"
source_bindings_path = "{}"
coverage_ledger_path = "{}"
output_dir = "{}"
required_table_families = ["trades"]
"#,
            source_bindings_path.display(),
            ledger_path.display(),
            output_dir.display()
        ),
    )
    .expect("write spec");

    let first = write_backfill_binding_coverage_report_from_spec_file(&spec_path).expect("first");
    let second = write_backfill_binding_coverage_report_from_spec_file(&spec_path).expect("second");

    assert_eq!(first.content_hash, second.content_hash);
    assert_eq!(
        first.path,
        output_dir.join(BACKFILL_BINDING_COVERAGE_REPORT_FILE)
    );
}

#[test]
fn binding_coverage_cli_writes_blocked_report() {
    let dir = tempfile::TempDir::new().expect("temp dir");
    let source_bindings_path = dir.path().join("source-bindings.toml");
    let ledger_path = dir.path().join("ledger.json");
    let output_dir = dir.path().join("out");
    let spec_path = dir.path().join("binding-coverage.toml");
    std::fs::write(&source_bindings_path, synthetic_source_bindings_toml())
        .expect("write bindings");
    std::fs::write(
        &ledger_path,
        serde_json::to_vec_pretty(&ledger(vec![record("synthetic-other-binding", "trades")]))
            .expect("ledger json"),
    )
    .expect("write ledger");
    std::fs::write(
        &spec_path,
        format!(
            r#"report_id = "synthetic-binding-coverage"
source_bindings_path = "{}"
coverage_ledger_path = "{}"
output_dir = "{}"
required_table_families = ["trades"]
"#,
            source_bindings_path.display(),
            ledger_path.display(),
            output_dir.display()
        ),
    )
    .expect("write spec");

    let binary = std::env::var("CARGO_BIN_EXE_backfill_binding_coverage").expect("binary path");
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
    let report_path = output_dir.join(BACKFILL_BINDING_COVERAGE_REPORT_FILE);
    let report: backtesting_vertical_slice::backfill_binding_coverage::BackfillBindingCoverageReport =
        serde_json::from_slice(&std::fs::read(report_path).expect("report"))
            .expect("report json");
    assert_eq!(report.status, BackfillBindingCoverageStatus::Blocked);
}

fn synthetic_source_bindings_toml() -> String {
    r#"schema_version = "backfill-source-bindings.v1"
contract_version = "backfill-table-contract.v1"

[[source_binding]]
key = "synthetic-native-trades"
venue = "synthetic"
product_family = "spot"
fixture = "native-trades"
family = "market_data"
method = "GET"
source_uri = "https://example.invalid/trades/{symbol}/{dt}.csv"
payload_extension = "csv"
extractor = "synthetic_trades"
evidence_state = "directly_backfillable"
table_families = ["trades"]

[[source_binding]]
key = "synthetic-instruments"
venue = "synthetic"
product_family = "spot"
fixture = "mixed"
family = "instrument_universe"
method = "GET"
source_uri = "https://example.invalid/instruments.json"
payload_extension = "json"
extractor = "synthetic_instruments"
evidence_state = "directly_backfillable"
table_families = ["instruments"]
"#
    .to_string()
}

fn synthetic_multi_family_source_bindings_toml() -> String {
    r#"schema_version = "backfill-source-bindings.v1"
contract_version = "backfill-table-contract.v1"

[[source_binding]]
key = "synthetic-multi-family-source"
venue = "synthetic"
product_family = "spot"
fixture = "multi-family"
family = "archive_index"
method = "GET"
source_uri = "https://example.invalid/multi-family/{dt}.json"
payload_extension = "json"
extractor = "synthetic_multi_family"
evidence_state = "directly_backfillable"
table_families = ["instruments", "trades"]
"#
    .to_string()
}

fn ledger(records: Vec<BackfillCoverageRecord>) -> BackfillCoverageLedger {
    BackfillCoverageLedger {
        schema_version: "backfill-coverage-ledger.v1".to_string(),
        ledger_id: "synthetic-ledger".to_string(),
        summary: BackfillCoverageSummary {
            total_records: records.len() as u64,
            accepted_records: 0,
            accepted_with_gaps_records: 0,
            rejected_records: records.len() as u64,
            physical_only_records: 0,
            canonical_ready_records: 0,
            accepted_objects: 0,
            accepted_bytes: 0,
            skipped_objects: 0,
            physical_only_objects: 0,
            physical_only_bytes: 0,
            blocking_issue_count: 0,
        },
        records,
    }
}

fn record(source_binding: &str, table_family: &str) -> BackfillCoverageRecord {
    let mut record = accepted_record(source_binding, table_family);
    record.status = BackfillCoverageStatus::Rejected;
    record.canonical_ready = false;
    record.accepted_objects = 0;
    record.accepted_bytes = 0;
    record
}

fn accepted_record(source_binding: &str, table_family: &str) -> BackfillCoverageRecord {
    BackfillCoverageRecord {
        record_id: format!("record-{source_binding}"),
        status: BackfillCoverageStatus::Accepted,
        source_binding: Some(source_binding.to_string()),
        table_family: Some(table_family.to_string()),
        source_proof_id: None,
        source_proof_version: None,
        canonical_ready: true,
        accepted_objects: 1,
        accepted_bytes: 1,
        skipped_objects: 0,
        physical_only_objects: 0,
        physical_only_bytes: 0,
        blocking_issues: Vec::new(),
    }
}

fn accepted_unscoped_record(source_binding: &str) -> BackfillCoverageRecord {
    let mut record = accepted_record(source_binding, "instruments");
    record.table_family = None;
    record
}

fn empty_binding_record(record_id: &str) -> BackfillCoverageRecord {
    let mut record = accepted_record("synthetic-placeholder", "trades");
    record.record_id = record_id.to_string();
    record.source_binding = None;
    record
}
