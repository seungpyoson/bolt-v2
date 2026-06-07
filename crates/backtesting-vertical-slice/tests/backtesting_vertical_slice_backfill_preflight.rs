use backtesting_vertical_slice::{
    backfill_coverage::{
        BackfillCoverageIssue, BackfillCoverageLedger, BackfillCoverageRecord,
        BackfillCoverageStatus, BackfillCoverageSummary,
    },
    backfill_preflight::{
        BACKFILL_PREFLIGHT_REPORT_FILE, BackfillPreflightBlockingReason,
        BackfillPreflightSelection, BackfillPreflightStatus, evaluate_backfill_preflight,
        write_backfill_preflight_report_from_spec_file,
    },
};
use std::process::Command;

#[test]
fn backfill_preflight_blocks_when_no_accepted_canonical_ready_record_exists() {
    let ledger = BackfillCoverageLedger {
        schema_version: "backfill-coverage-ledger.v1".to_string(),
        ledger_id: "synthetic-ledger".to_string(),
        records: vec![
            rejected_record("synthetic-rejected-a"),
            physical_only_record("synthetic-inventory-a"),
        ],
        summary: BackfillCoverageSummary {
            total_records: 2,
            accepted_records: 0,
            accepted_with_gaps_records: 0,
            rejected_records: 1,
            physical_only_records: 1,
            canonical_ready_records: 0,
            accepted_objects: 0,
            accepted_bytes: 0,
            skipped_objects: 0,
            physical_only_objects: 4,
            physical_only_bytes: 400,
            blocking_issue_count: 2,
        },
    };

    let report = evaluate_backfill_preflight(
        "synthetic-preflight",
        &ledger,
        &BackfillPreflightSelection {
            max_accepted_objects: 10,
            max_accepted_bytes: 1_000,
            require_canonical_ready: true,
            allow_gaps: false,
        },
    );

    assert_eq!(report.status, BackfillPreflightStatus::Blocked);
    assert!(report.selected_record.is_none());
    assert!(
        report
            .blocking_reasons
            .contains(&BackfillPreflightBlockingReason::NoAcceptedRecords)
    );
    assert!(
        report
            .blocking_reasons
            .contains(&BackfillPreflightBlockingReason::NoCanonicalReadyRecords)
    );
}

#[test]
fn backfill_preflight_selects_bounded_canonical_ready_record_without_source_constants() {
    let ledger = BackfillCoverageLedger {
        schema_version: "backfill-coverage-ledger.v1".to_string(),
        ledger_id: "synthetic-ledger".to_string(),
        records: vec![
            accepted_record("synthetic-large", 7, 700),
            accepted_record("synthetic-small", 3, 300),
        ],
        summary: BackfillCoverageSummary {
            total_records: 2,
            accepted_records: 2,
            accepted_with_gaps_records: 0,
            rejected_records: 0,
            physical_only_records: 0,
            canonical_ready_records: 2,
            accepted_objects: 10,
            accepted_bytes: 1_000,
            skipped_objects: 0,
            physical_only_objects: 0,
            physical_only_bytes: 0,
            blocking_issue_count: 0,
        },
    };

    let report = evaluate_backfill_preflight(
        "synthetic-preflight",
        &ledger,
        &BackfillPreflightSelection {
            max_accepted_objects: 5,
            max_accepted_bytes: 500,
            require_canonical_ready: true,
            allow_gaps: false,
        },
    );

    assert_eq!(report.status, BackfillPreflightStatus::Go);
    let selected = report.selected_record.expect("selected record");
    assert_eq!(selected.record_id, "synthetic-small");
    assert_eq!(selected.table_family, "trades");
    assert_eq!(selected.accepted_objects, 3);
    assert_eq!(selected.accepted_bytes, 300);
}

#[test]
fn backfill_preflight_reads_toml_spec_and_writes_report_idempotently() {
    let dir = tempfile::TempDir::new().expect("temp dir");
    let ledger_path = dir.path().join("ledger.json");
    let output_dir = dir.path().join("out");
    let spec_path = dir.path().join("preflight.toml");
    let ledger = BackfillCoverageLedger {
        schema_version: "backfill-coverage-ledger.v1".to_string(),
        ledger_id: "synthetic-ledger".to_string(),
        records: vec![accepted_record("synthetic-ready", 1, 100)],
        summary: BackfillCoverageSummary {
            total_records: 1,
            accepted_records: 1,
            accepted_with_gaps_records: 0,
            rejected_records: 0,
            physical_only_records: 0,
            canonical_ready_records: 1,
            accepted_objects: 1,
            accepted_bytes: 100,
            skipped_objects: 0,
            physical_only_objects: 0,
            physical_only_bytes: 0,
            blocking_issue_count: 0,
        },
    };
    std::fs::write(
        &ledger_path,
        serde_json::to_vec_pretty(&ledger).expect("ledger json"),
    )
    .expect("write ledger");
    std::fs::write(
        &spec_path,
        format!(
            r#"preflight_id = "synthetic-preflight"
coverage_ledger_path = "{}"
output_dir = "{}"

[selection]
max_accepted_objects = 5
max_accepted_bytes = 500
require_canonical_ready = true
allow_gaps = false
"#,
            ledger_path.display(),
            output_dir.display()
        ),
    )
    .expect("write spec");

    let first = write_backfill_preflight_report_from_spec_file(&spec_path).expect("first report");
    let second = write_backfill_preflight_report_from_spec_file(&spec_path).expect("second report");

    assert_eq!(first.content_hash, second.content_hash);
    assert_eq!(first.path, output_dir.join(BACKFILL_PREFLIGHT_REPORT_FILE));
}

#[test]
fn backfill_preflight_cli_writes_blocked_report_before_conversion_work() {
    let dir = tempfile::TempDir::new().expect("temp dir");
    let ledger_path = dir.path().join("ledger.json");
    let output_dir = dir.path().join("out");
    let spec_path = dir.path().join("preflight.toml");
    let ledger = BackfillCoverageLedger {
        schema_version: "backfill-coverage-ledger.v1".to_string(),
        ledger_id: "synthetic-ledger".to_string(),
        records: vec![rejected_record("synthetic-rejected-a")],
        summary: BackfillCoverageSummary {
            total_records: 1,
            accepted_records: 0,
            accepted_with_gaps_records: 0,
            rejected_records: 1,
            physical_only_records: 0,
            canonical_ready_records: 0,
            accepted_objects: 0,
            accepted_bytes: 0,
            skipped_objects: 0,
            physical_only_objects: 0,
            physical_only_bytes: 0,
            blocking_issue_count: 1,
        },
    };
    std::fs::write(
        &ledger_path,
        serde_json::to_vec_pretty(&ledger).expect("ledger json"),
    )
    .expect("write ledger");
    std::fs::write(
        &spec_path,
        format!(
            r#"preflight_id = "synthetic-preflight"
coverage_ledger_path = "{}"
output_dir = "{}"

[selection]
max_accepted_objects = 5
max_accepted_bytes = 500
require_canonical_ready = true
allow_gaps = false
"#,
            ledger_path.display(),
            output_dir.display()
        ),
    )
    .expect("write spec");

    let binary = std::env::var("CARGO_BIN_EXE_backfill_preflight").expect("binary path");
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
    let report_path = output_dir.join(BACKFILL_PREFLIGHT_REPORT_FILE);
    let report: backtesting_vertical_slice::backfill_preflight::BackfillPreflightReport =
        serde_json::from_slice(&std::fs::read(report_path).expect("report")).expect("report json");
    assert_eq!(report.status, BackfillPreflightStatus::Blocked);
    assert!(report.selected_record.is_none());
}

fn accepted_record(
    record_id: &str,
    accepted_objects: u64,
    accepted_bytes: u64,
) -> BackfillCoverageRecord {
    BackfillCoverageRecord {
        record_id: record_id.to_string(),
        status: BackfillCoverageStatus::Accepted,
        source_binding: Some("synthetic-source-binding".to_string()),
        table_family: Some("trades".to_string()),
        source_proof_id: Some("synthetic-source-proof".to_string()),
        source_proof_version: Some(1),
        canonical_ready: true,
        accepted_objects,
        accepted_bytes,
        skipped_objects: 0,
        physical_only_objects: 0,
        physical_only_bytes: 0,
        blocking_issues: Vec::new(),
    }
}

fn rejected_record(record_id: &str) -> BackfillCoverageRecord {
    BackfillCoverageRecord {
        record_id: record_id.to_string(),
        status: BackfillCoverageStatus::Rejected,
        source_binding: Some("synthetic-source-binding".to_string()),
        table_family: Some("trades".to_string()),
        source_proof_id: Some("synthetic-source-proof".to_string()),
        source_proof_version: Some(1),
        canonical_ready: false,
        accepted_objects: 0,
        accepted_bytes: 0,
        skipped_objects: 0,
        physical_only_objects: 0,
        physical_only_bytes: 0,
        blocking_issues: vec![BackfillCoverageIssue::SourceProofNotAccepted],
    }
}

fn physical_only_record(record_id: &str) -> BackfillCoverageRecord {
    BackfillCoverageRecord {
        record_id: record_id.to_string(),
        status: BackfillCoverageStatus::PhysicalOnly,
        source_binding: None,
        table_family: None,
        source_proof_id: None,
        source_proof_version: None,
        canonical_ready: false,
        accepted_objects: 0,
        accepted_bytes: 0,
        skipped_objects: 0,
        physical_only_objects: 4,
        physical_only_bytes: 400,
        blocking_issues: vec![BackfillCoverageIssue::MissingManifest],
    }
}
