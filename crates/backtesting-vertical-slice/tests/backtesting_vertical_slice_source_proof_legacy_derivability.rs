use std::fs;

use backtesting_vertical_slice::source_proof_legacy_derivability::{
    SOURCE_PROOF_LEGACY_DERIVABILITY_REPORT_FILE, SourceProofLegacyDerivabilityIssue,
    SourceProofLegacyDerivabilityJson, SourceProofLegacyDerivabilityReport,
    SourceProofLegacyDerivableField, write_source_proof_legacy_derivability_report_from_spec_file,
};

const SYNTHETIC_SOURCE_BINDING: &str = "synthetic-source-binding";
const SYNTHETIC_TABLE_FAMILY: &str = "synthetic-table-family";
const SYNTHETIC_SECOND_TABLE_FAMILY: &str = "synthetic-second-table-family";

fn legacy_source_proof() -> serde_json::Value {
    serde_json::json!({
        "source_proof_id": "source-proof-synthetic-legacy",
        "source_proof_version": 1,
        "status": "pending",
        "source_binding_key": SYNTHETIC_SOURCE_BINDING,
        "venue": "synthetic",
        "product_family": "spot",
        "evidence_state": "directly_backfillable",
        "fixture": "mixed",
        "source_time_range": {
            "start_utc": "2026-01-01T00:00:00Z",
            "end_utc": "2026-01-02T00:00:00Z"
        },
        "forbidden_claims": [
            "No execution-quality claims."
        ],
        "raw_payload_records": [
            {
                "bytes": 123,
                "payload_hash": "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
                "source_binding": SYNTHETIC_SOURCE_BINDING,
                "uri": "/tmp/synthetic/raw/object.json"
            }
        ],
        "required_checks": {
            "fidelity": "pending",
            "forbidden_claims": "pending",
            "license": "source_specific_review_required",
            "nt_mapping": "pending",
            "raw_payload_hash": "passed",
            "schema_sample": "captured",
            "time_range": "declared"
        },
        "table_families": [SYNTHETIC_TABLE_FAMILY]
    })
}

fn legacy_acceptance_manifest() -> serde_json::Value {
    serde_json::json!({
        "run_id": "source-proof-legacy-acceptance-synthetic",
        "write_mode": "s3_staging",
        "canonical_s3_write": false,
        "completed_object_count": 1,
        "completed_bytes": 123,
        "s3_payload_records": [
            {
                "s3_uri": "s3://synthetic-staging/raw/object.json",
                "payload_hash": "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
                "bytes": 123,
                "source_binding": SYNTHETIC_SOURCE_BINDING
            }
        ]
    })
}

#[test]
fn legacy_derivability_reports_structural_fields_without_accepting_source_proof() {
    let report = SourceProofLegacyDerivabilityReport::from_json_values(
        "legacy-derivability-synthetic",
        &legacy_acceptance_manifest(),
        vec![SourceProofLegacyDerivabilityJson {
            proof_uri: "proof://synthetic/legacy-source-proof.json".to_string(),
            proof: legacy_source_proof(),
        }],
    )
    .expect("legacy derivability report builds");

    assert_eq!(report.summary.total_records, 1);
    assert_eq!(report.summary.s3_bound_records, 1);
    assert_eq!(report.summary.single_table_family_records, 1);
    assert_eq!(report.summary.acceptance_blocked_records, 1);
    assert_eq!(report.summary.table_family_counts.len(), 1);
    assert_eq!(
        report.summary.table_family_counts[0].table_family,
        SYNTHETIC_TABLE_FAMILY
    );
    assert_eq!(report.summary.table_family_counts[0].count, 1);
    assert_eq!(
        report
            .summary
            .blocking_issue_counts
            .iter()
            .map(|entry| (entry.issue, entry.count))
            .collect::<Vec<_>>(),
        vec![
            (SourceProofLegacyDerivabilityIssue::LicenseNotPassed, 1),
            (SourceProofLegacyDerivabilityIssue::NtMappingNotPassed, 1),
            (SourceProofLegacyDerivabilityIssue::FidelityNotPassed, 1),
            (
                SourceProofLegacyDerivabilityIssue::ForbiddenClaimsNotPassed,
                1
            ),
            (SourceProofLegacyDerivabilityIssue::SchemaSampleNotPassed, 1),
        ]
    );

    let record = report.records.first().expect("record");
    assert_eq!(
        record.source_binding.as_deref(),
        Some(SYNTHETIC_SOURCE_BINDING)
    );
    assert_eq!(
        record.source_proof_id.as_deref(),
        Some("source-proof-synthetic-legacy")
    );
    assert_eq!(record.raw_payload_records, 1);
    assert_eq!(record.s3_bound_raw_payload_records, 1);
    assert_eq!(record.accepted_bytes_from_s3, 123);
    assert!(
        record
            .derivable_fields
            .contains(&SourceProofLegacyDerivableField::SourceBinding)
    );
    assert!(
        record
            .derivable_fields
            .contains(&SourceProofLegacyDerivableField::TableFamily)
    );
    assert!(
        record
            .derivable_fields
            .contains(&SourceProofLegacyDerivableField::FixtureType)
    );
    assert!(
        record
            .derivable_fields
            .contains(&SourceProofLegacyDerivableField::RequestedTimeRange)
    );
    assert!(
        record
            .derivable_fields
            .contains(&SourceProofLegacyDerivableField::CoverageTimeRange)
    );
    assert!(
        record
            .derivable_fields
            .contains(&SourceProofLegacyDerivableField::RawSampleUri)
    );
    assert!(
        record
            .derivable_fields
            .contains(&SourceProofLegacyDerivableField::RawSampleHash)
    );
    assert!(
        record
            .derivable_fields
            .contains(&SourceProofLegacyDerivableField::AcceptanceScope)
    );
    assert!(
        record
            .derivable_fields
            .contains(&SourceProofLegacyDerivableField::ClaimLimits)
    );
    assert_eq!(
        record.blocking_issues,
        vec![
            SourceProofLegacyDerivabilityIssue::LicenseNotPassed,
            SourceProofLegacyDerivabilityIssue::NtMappingNotPassed,
            SourceProofLegacyDerivabilityIssue::FidelityNotPassed,
            SourceProofLegacyDerivabilityIssue::ForbiddenClaimsNotPassed,
            SourceProofLegacyDerivabilityIssue::SchemaSampleNotPassed,
        ]
    );
}

#[test]
fn legacy_derivability_reports_multitable_and_unbound_payloads() {
    let mut proof = legacy_source_proof();
    proof["table_families"] =
        serde_json::json!([SYNTHETIC_TABLE_FAMILY, SYNTHETIC_SECOND_TABLE_FAMILY]);
    proof["raw_payload_records"][0]["payload_hash"] =
        serde_json::json!("bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb");

    let report = SourceProofLegacyDerivabilityReport::from_json_values(
        "legacy-derivability-unbound",
        &legacy_acceptance_manifest(),
        vec![SourceProofLegacyDerivabilityJson {
            proof_uri: "proof://synthetic/unbound-source-proof.json".to_string(),
            proof,
        }],
    )
    .expect("legacy derivability report builds");

    let record = report.records.first().expect("record");
    assert_eq!(record.raw_payload_records, 1);
    assert_eq!(record.s3_bound_raw_payload_records, 0);
    assert_eq!(record.accepted_bytes_from_s3, 0);
    assert!(
        record
            .blocking_issues
            .contains(&SourceProofLegacyDerivabilityIssue::NotExactlyOneTableFamily)
    );
    assert!(
        record
            .blocking_issues
            .contains(&SourceProofLegacyDerivabilityIssue::RawPayloadNotFullyS3Bound)
    );
    assert!(
        !record
            .derivable_fields
            .contains(&SourceProofLegacyDerivableField::RawSampleUri)
    );
    assert!(
        !record
            .derivable_fields
            .contains(&SourceProofLegacyDerivableField::TableFamily)
    );
}

#[test]
fn legacy_derivability_reads_toml_spec_and_writes_report() {
    let dir = tempfile::TempDir::new().expect("temp dir");
    let proof_path = dir.path().join("source-proof.json");
    fs::write(
        &proof_path,
        serde_json::to_vec(&legacy_source_proof()).expect("serialize proof"),
    )
    .expect("write proof");
    let acceptance_path = dir.path().join("acceptance-manifest.json");
    fs::write(
        &acceptance_path,
        serde_json::to_vec(&legacy_acceptance_manifest()).expect("serialize acceptance manifest"),
    )
    .expect("write acceptance manifest");
    let output_dir = dir.path().join("legacy-derivability");
    let spec_path = dir.path().join("legacy-derivability.toml");
    fs::write(
        &spec_path,
        format!(
            r#"
report_id = "legacy-derivability-spec"
output_dir = "{}"
acceptance_manifest_path = "{}"

[[source_proof]]
proof_uri = "proof://synthetic/source-proof.json"
path = "{}"
"#,
            output_dir.display(),
            acceptance_path.display(),
            proof_path.display()
        ),
    )
    .expect("write spec");

    let artifact = write_source_proof_legacy_derivability_report_from_spec_file(&spec_path)
        .expect("write legacy derivability report from spec");

    assert_eq!(artifact.record_count, 1);
    assert_eq!(
        artifact.path,
        output_dir.join(SOURCE_PROOF_LEGACY_DERIVABILITY_REPORT_FILE)
    );
    let report: SourceProofLegacyDerivabilityReport =
        serde_json::from_slice(&fs::read(&artifact.path).expect("read report"))
            .expect("parse report");
    assert_eq!(report.report_id, "legacy-derivability-spec");
    assert_eq!(report.summary.s3_bound_records, 1);
}
