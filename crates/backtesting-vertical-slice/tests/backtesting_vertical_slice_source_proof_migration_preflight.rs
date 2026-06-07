use std::process::Command;

use backtesting_vertical_slice::{
    source_proof_legacy_derivability::{
        SourceProofLegacyDerivabilityIssue, SourceProofLegacyDerivabilityRecord,
        SourceProofLegacyDerivabilityReport, SourceProofLegacyDerivabilitySummary,
        SourceProofLegacyDerivableField,
    },
    source_proof_migration_preflight::{
        SOURCE_PROOF_MIGRATION_PREFLIGHT_REPORT_FILE, SourceProofMigrationPreflightReason,
        SourceProofMigrationPreflightSelection, SourceProofMigrationPreflightStatus,
        evaluate_source_proof_migration_preflight,
        write_source_proof_migration_preflight_report_from_spec_file,
    },
};

#[test]
fn migration_preflight_blocks_when_required_table_family_has_no_candidate() {
    let report = derivability_report(vec![record(
        "proof://synthetic/instruments.json",
        "synthetic-instruments",
        "instruments",
        1,
        100,
    )]);

    let preflight = evaluate_source_proof_migration_preflight(
        "synthetic-migration-preflight",
        &report,
        &selection(vec!["trades"]),
    );

    assert_eq!(
        preflight.status,
        SourceProofMigrationPreflightStatus::Blocked
    );
    assert!(preflight.selected_candidate.is_none());
    assert!(
        preflight
            .blocking_reasons
            .contains(&SourceProofMigrationPreflightReason::NoEligibleCandidate)
    );
}

#[test]
fn migration_preflight_selects_smallest_matching_candidate_without_source_constants() {
    let report = derivability_report(vec![
        record(
            "proof://synthetic/large.json",
            "synthetic-large",
            "trades",
            1,
            900,
        ),
        record(
            "proof://synthetic/small.json",
            "synthetic-small",
            "trades",
            1,
            100,
        ),
    ]);

    let preflight = evaluate_source_proof_migration_preflight(
        "synthetic-migration-preflight",
        &report,
        &selection(vec!["trades"]),
    );

    assert_eq!(
        preflight.status,
        SourceProofMigrationPreflightStatus::CandidateFound
    );
    let candidate = preflight.selected_candidate.expect("candidate");
    assert_eq!(candidate.source_binding, "synthetic-small");
    assert_eq!(candidate.table_family, "trades");
    assert_eq!(candidate.accepted_bytes_from_s3, 100);
    assert_eq!(
        candidate.remaining_acceptance_blockers,
        vec![SourceProofLegacyDerivabilityIssue::LicenseNotPassed]
    );
}

#[test]
fn migration_preflight_reads_toml_spec_and_writes_report_idempotently() {
    let dir = tempfile::TempDir::new().expect("temp dir");
    let report_path = dir.path().join("derivability.json");
    let output_dir = dir.path().join("out");
    let spec_path = dir.path().join("migration-preflight.toml");
    let report = derivability_report(vec![record(
        "proof://synthetic/ready.json",
        "synthetic-ready",
        "trades",
        1,
        100,
    )]);
    std::fs::write(
        &report_path,
        serde_json::to_vec_pretty(&report).expect("report json"),
    )
    .expect("write report");
    std::fs::write(
        &spec_path,
        format!(
            r#"preflight_id = "synthetic-migration-preflight"
derivability_report_path = "{}"
output_dir = "{}"

[selection]
allowed_table_families = ["trades"]
required_derivable_fields = [
  "source_binding",
  "table_family",
  "fixture_type",
  "requested_time_range",
  "coverage_time_range",
  "raw_sample_uri",
  "raw_sample_hash",
  "acceptance_scope",
  "claim_limits",
]
max_raw_payload_records = 1
max_accepted_bytes_from_s3 = 1000
require_single_table_family = true
require_s3_bound_payloads = true
"#,
            report_path.display(),
            output_dir.display()
        ),
    )
    .expect("write spec");

    let first =
        write_source_proof_migration_preflight_report_from_spec_file(&spec_path).expect("first");
    let second =
        write_source_proof_migration_preflight_report_from_spec_file(&spec_path).expect("second");

    assert_eq!(first.content_hash, second.content_hash);
    assert_eq!(
        first.path,
        output_dir.join(SOURCE_PROOF_MIGRATION_PREFLIGHT_REPORT_FILE)
    );
}

#[test]
fn migration_preflight_cli_writes_blocked_report_for_missing_table_family() {
    let dir = tempfile::TempDir::new().expect("temp dir");
    let report_path = dir.path().join("derivability.json");
    let output_dir = dir.path().join("out");
    let spec_path = dir.path().join("migration-preflight.toml");
    let report = derivability_report(vec![record(
        "proof://synthetic/instruments.json",
        "synthetic-instruments",
        "instruments",
        1,
        100,
    )]);
    std::fs::write(
        &report_path,
        serde_json::to_vec_pretty(&report).expect("report json"),
    )
    .expect("write report");
    std::fs::write(
        &spec_path,
        format!(
            r#"preflight_id = "synthetic-migration-preflight"
derivability_report_path = "{}"
output_dir = "{}"

[selection]
allowed_table_families = ["trades"]
required_derivable_fields = ["source_binding", "table_family"]
max_raw_payload_records = 1
max_accepted_bytes_from_s3 = 1000
require_single_table_family = true
require_s3_bound_payloads = true
"#,
            report_path.display(),
            output_dir.display()
        ),
    )
    .expect("write spec");

    let binary =
        std::env::var("CARGO_BIN_EXE_source_proof_migration_preflight").expect("binary path");
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
    let report_path = output_dir.join(SOURCE_PROOF_MIGRATION_PREFLIGHT_REPORT_FILE);
    let preflight: backtesting_vertical_slice::source_proof_migration_preflight::SourceProofMigrationPreflightReport =
        serde_json::from_slice(&std::fs::read(report_path).expect("preflight report"))
            .expect("preflight json");
    assert_eq!(
        preflight.status,
        SourceProofMigrationPreflightStatus::Blocked
    );
}

fn derivability_report(
    records: Vec<SourceProofLegacyDerivabilityRecord>,
) -> SourceProofLegacyDerivabilityReport {
    SourceProofLegacyDerivabilityReport {
        schema_version: "source-proof-legacy-derivability-report.v1".to_string(),
        report_id: "synthetic-derivability".to_string(),
        summary: SourceProofLegacyDerivabilitySummary {
            total_records: records.len() as u64,
            s3_bound_records: records
                .iter()
                .filter(|record| record.raw_payload_records == record.s3_bound_raw_payload_records)
                .count() as u64,
            single_table_family_records: records
                .iter()
                .filter(|record| record.table_families.len() == 1)
                .count() as u64,
            acceptance_blocked_records: records
                .iter()
                .filter(|record| !record.blocking_issues.is_empty())
                .count() as u64,
            blocking_issue_count: records
                .iter()
                .map(|record| record.blocking_issues.len() as u64)
                .sum(),
        },
        records,
    }
}

fn record(
    proof_uri: &str,
    source_binding: &str,
    table_family: &str,
    raw_payload_records: u64,
    accepted_bytes_from_s3: u64,
) -> SourceProofLegacyDerivabilityRecord {
    SourceProofLegacyDerivabilityRecord {
        proof_uri: proof_uri.to_string(),
        source_proof_id: Some(format!("source-proof-{source_binding}")),
        source_proof_version: Some(1),
        source_binding: Some(source_binding.to_string()),
        legacy_status: Some("pending".to_string()),
        raw_payload_records,
        s3_bound_raw_payload_records: raw_payload_records,
        accepted_bytes_from_s3,
        table_families: vec![table_family.to_string()],
        derivable_fields: vec![
            SourceProofLegacyDerivableField::SourceBinding,
            SourceProofLegacyDerivableField::TableFamily,
            SourceProofLegacyDerivableField::FixtureType,
            SourceProofLegacyDerivableField::RequestedTimeRange,
            SourceProofLegacyDerivableField::CoverageTimeRange,
            SourceProofLegacyDerivableField::RawSampleUri,
            SourceProofLegacyDerivableField::RawSampleHash,
            SourceProofLegacyDerivableField::AcceptanceScope,
            SourceProofLegacyDerivableField::ClaimLimits,
        ],
        blocking_issues: vec![SourceProofLegacyDerivabilityIssue::LicenseNotPassed],
    }
}

fn selection(allowed_table_families: Vec<&str>) -> SourceProofMigrationPreflightSelection {
    SourceProofMigrationPreflightSelection {
        allowed_table_families: allowed_table_families
            .into_iter()
            .map(str::to_string)
            .collect(),
        required_derivable_fields: vec![
            SourceProofLegacyDerivableField::SourceBinding,
            SourceProofLegacyDerivableField::TableFamily,
            SourceProofLegacyDerivableField::FixtureType,
            SourceProofLegacyDerivableField::RequestedTimeRange,
            SourceProofLegacyDerivableField::CoverageTimeRange,
            SourceProofLegacyDerivableField::RawSampleUri,
            SourceProofLegacyDerivableField::RawSampleHash,
            SourceProofLegacyDerivableField::AcceptanceScope,
            SourceProofLegacyDerivableField::ClaimLimits,
        ],
        max_raw_payload_records: 1,
        max_accepted_bytes_from_s3: 1000,
        require_single_table_family: true,
        require_s3_bound_payloads: true,
    }
}
