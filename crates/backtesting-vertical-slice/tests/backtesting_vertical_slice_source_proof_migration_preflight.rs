use std::process::Command;

use backtesting_vertical_slice::{
    source_proof::{EvidenceState, SourceBindingRegistry},
    source_proof_legacy_derivability::{
        SourceProofLegacyDerivabilityIssue, SourceProofLegacyDerivabilityRecord,
        SourceProofLegacyDerivabilityReport, SourceProofLegacyDerivabilitySummary,
        SourceProofLegacyDerivableField,
    },
    source_proof_migration_preflight::{
        SOURCE_PROOF_MIGRATION_PREFLIGHT_REPORT_FILE, SourceProofMigrationPreflightReason,
        SourceProofMigrationPreflightSelection, SourceProofMigrationPreflightStatus,
        evaluate_source_proof_migration_preflight_with_registry,
        write_source_proof_migration_preflight_report_from_spec_file,
    },
};

const SYNTHETIC_VENUE: &str = "synthetic-venue";
const SYNTHETIC_PRODUCT_FAMILY: &str = "synthetic-product-family";
const SYNTHETIC_OTHER_PRODUCT_FAMILY: &str = "synthetic-other-product-family";
const SYNTHETIC_TABLE_FAMILY: &str = "synthetic-table-family";
const SYNTHETIC_OTHER_TABLE_FAMILY: &str = "synthetic-other-table-family";
const SYNTHETIC_EVIDENCE_STATE: &str = "directly_backfillable";

#[test]
fn migration_preflight_blocks_when_required_table_family_has_no_candidate() {
    let report = derivability_report(vec![record(
        "proof://synthetic/instruments.json",
        "synthetic-instruments",
        SYNTHETIC_OTHER_TABLE_FAMILY,
        1,
        100,
    )]);

    let registry = source_binding_registry(&[binding(
        "synthetic-instruments",
        SYNTHETIC_PRODUCT_FAMILY,
        SYNTHETIC_OTHER_TABLE_FAMILY,
    )]);
    let preflight = evaluate_source_proof_migration_preflight_with_registry(
        "synthetic-migration-preflight",
        &report,
        &selection(vec![SYNTHETIC_TABLE_FAMILY]),
        &registry,
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
            SYNTHETIC_TABLE_FAMILY,
            1,
            900,
        ),
        record(
            "proof://synthetic/small.json",
            "synthetic-small",
            SYNTHETIC_TABLE_FAMILY,
            1,
            100,
        ),
    ]);

    let registry = source_binding_registry(&[
        binding(
            "synthetic-large",
            SYNTHETIC_PRODUCT_FAMILY,
            SYNTHETIC_TABLE_FAMILY,
        ),
        binding(
            "synthetic-small",
            SYNTHETIC_PRODUCT_FAMILY,
            SYNTHETIC_TABLE_FAMILY,
        ),
    ]);
    let preflight = evaluate_source_proof_migration_preflight_with_registry(
        "synthetic-migration-preflight",
        &report,
        &selection(vec![SYNTHETIC_TABLE_FAMILY]),
        &registry,
    );

    assert_eq!(
        preflight.status,
        SourceProofMigrationPreflightStatus::CandidateFound
    );
    let candidate = preflight.selected_candidate.expect("candidate");
    assert_eq!(candidate.source_binding, "synthetic-small");
    assert_eq!(candidate.table_family, SYNTHETIC_TABLE_FAMILY);
    assert_eq!(candidate.accepted_bytes_from_s3, 100);
    assert_eq!(
        candidate.remaining_acceptance_blockers,
        vec![SourceProofLegacyDerivabilityIssue::LicenseNotPassed]
    );
}

#[test]
fn migration_preflight_reports_source_binding_product_family_mismatch() {
    let report = derivability_report(vec![
        record(
            "proof://synthetic/metadata-mismatch.json",
            "synthetic-metadata-mismatch",
            SYNTHETIC_TABLE_FAMILY,
            1,
            100,
        )
        .with_product_family(SYNTHETIC_OTHER_PRODUCT_FAMILY),
    ]);
    let registry = source_binding_registry(&[binding(
        "synthetic-metadata-mismatch",
        SYNTHETIC_PRODUCT_FAMILY,
        SYNTHETIC_TABLE_FAMILY,
    )]);

    let preflight = evaluate_source_proof_migration_preflight_with_registry(
        "synthetic-migration-preflight",
        &report,
        &selection(vec![SYNTHETIC_TABLE_FAMILY]),
        &registry,
    );

    assert_eq!(
        preflight.status,
        SourceProofMigrationPreflightStatus::CandidateFound
    );
    let candidate = preflight.selected_candidate.expect("candidate");
    assert!(
        candidate
            .remaining_acceptance_blockers
            .contains(&SourceProofLegacyDerivabilityIssue::SourceBindingProductFamilyMismatch)
    );
}

#[test]
fn migration_preflight_prefers_candidate_with_fewer_remaining_blockers() {
    let report = derivability_report(vec![
        record(
            "proof://synthetic/smaller-metadata-mismatch.json",
            "synthetic-smaller-metadata-mismatch",
            SYNTHETIC_TABLE_FAMILY,
            1,
            50,
        )
        .with_product_family(SYNTHETIC_OTHER_PRODUCT_FAMILY),
        record(
            "proof://synthetic/larger-clean-metadata.json",
            "synthetic-larger-clean-metadata",
            SYNTHETIC_TABLE_FAMILY,
            1,
            100,
        ),
    ]);
    let registry = source_binding_registry(&[
        binding(
            "synthetic-smaller-metadata-mismatch",
            SYNTHETIC_PRODUCT_FAMILY,
            SYNTHETIC_TABLE_FAMILY,
        ),
        binding(
            "synthetic-larger-clean-metadata",
            SYNTHETIC_PRODUCT_FAMILY,
            SYNTHETIC_TABLE_FAMILY,
        ),
    ]);

    let preflight = evaluate_source_proof_migration_preflight_with_registry(
        "synthetic-migration-preflight",
        &report,
        &selection(vec![SYNTHETIC_TABLE_FAMILY]),
        &registry,
    );

    assert_eq!(
        preflight.status,
        SourceProofMigrationPreflightStatus::CandidateFound
    );
    let candidate = preflight.selected_candidate.expect("candidate");
    assert_eq!(candidate.source_binding, "synthetic-larger-clean-metadata");
    assert_eq!(candidate.accepted_bytes_from_s3, 100);
    assert_eq!(
        candidate.remaining_acceptance_blockers,
        vec![SourceProofLegacyDerivabilityIssue::LicenseNotPassed]
    );
}

#[test]
fn migration_preflight_treats_non_backfillable_evidence_state_as_blocker() {
    let report = derivability_report(vec![
        record(
            "proof://synthetic/smaller-bounded-current-only.json",
            "synthetic-smaller-bounded-current-only",
            SYNTHETIC_TABLE_FAMILY,
            1,
            50,
        )
        .with_evidence_state(EvidenceState::BoundedOrCurrentOnly),
        record(
            "proof://synthetic/larger-directly-backfillable.json",
            "synthetic-larger-directly-backfillable",
            SYNTHETIC_TABLE_FAMILY,
            1,
            100,
        ),
    ]);
    let registry = source_binding_registry(&[
        binding_with_evidence_state(
            "synthetic-smaller-bounded-current-only",
            SYNTHETIC_PRODUCT_FAMILY,
            SYNTHETIC_TABLE_FAMILY,
            "bounded_or_current_only",
        ),
        binding(
            "synthetic-larger-directly-backfillable",
            SYNTHETIC_PRODUCT_FAMILY,
            SYNTHETIC_TABLE_FAMILY,
        ),
    ]);

    let preflight = evaluate_source_proof_migration_preflight_with_registry(
        "synthetic-migration-preflight",
        &report,
        &selection(vec![SYNTHETIC_TABLE_FAMILY]),
        &registry,
    );

    assert_eq!(
        preflight.status,
        SourceProofMigrationPreflightStatus::CandidateFound
    );
    let candidate = preflight.selected_candidate.expect("candidate");
    assert_eq!(
        candidate.source_binding,
        "synthetic-larger-directly-backfillable"
    );
    assert_eq!(
        candidate.remaining_acceptance_blockers,
        vec![SourceProofLegacyDerivabilityIssue::LicenseNotPassed]
    );
}

#[test]
fn migration_preflight_reads_toml_spec_and_writes_report_idempotently() {
    let dir = tempfile::TempDir::new().expect("temp dir");
    let report_path = dir.path().join("derivability.json");
    let source_bindings_path = dir.path().join("source-bindings.toml");
    let output_dir = dir.path().join("out");
    let spec_path = dir.path().join("migration-preflight.toml");
    let report = derivability_report(vec![record(
        "proof://synthetic/ready.json",
        "synthetic-ready",
        SYNTHETIC_TABLE_FAMILY,
        1,
        100,
    )]);
    std::fs::write(
        &report_path,
        serde_json::to_vec_pretty(&report).expect("report json"),
    )
    .expect("write report");
    std::fs::write(
        &source_bindings_path,
        source_binding_registry_toml(&[binding(
            "synthetic-ready",
            SYNTHETIC_PRODUCT_FAMILY,
            SYNTHETIC_TABLE_FAMILY,
        )]),
    )
    .expect("write source bindings");
    std::fs::write(
        &spec_path,
        format!(
            r#"preflight_id = "synthetic-migration-preflight"
derivability_report_path = "{}"
source_bindings_path = "{}"
output_dir = "{}"

[selection]
allowed_table_families = ["{SYNTHETIC_TABLE_FAMILY}"]
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
            source_bindings_path.display(),
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
    let source_bindings_path = dir.path().join("source-bindings.toml");
    let output_dir = dir.path().join("out");
    let spec_path = dir.path().join("migration-preflight.toml");
    let report = derivability_report(vec![record(
        "proof://synthetic/instruments.json",
        "synthetic-instruments",
        SYNTHETIC_OTHER_TABLE_FAMILY,
        1,
        100,
    )]);
    std::fs::write(
        &report_path,
        serde_json::to_vec_pretty(&report).expect("report json"),
    )
    .expect("write report");
    std::fs::write(
        &source_bindings_path,
        source_binding_registry_toml(&[binding(
            "synthetic-instruments",
            SYNTHETIC_PRODUCT_FAMILY,
            SYNTHETIC_OTHER_TABLE_FAMILY,
        )]),
    )
    .expect("write source bindings");
    std::fs::write(
        &spec_path,
        format!(
            r#"preflight_id = "synthetic-migration-preflight"
derivability_report_path = "{}"
source_bindings_path = "{}"
output_dir = "{}"

[selection]
allowed_table_families = ["{SYNTHETIC_TABLE_FAMILY}"]
required_derivable_fields = ["source_binding", "table_family"]
max_raw_payload_records = 1
max_accepted_bytes_from_s3 = 1000
require_single_table_family = true
require_s3_bound_payloads = true
"#,
            report_path.display(),
            source_bindings_path.display(),
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
            blocking_issue_counts: Vec::new(),
            table_family_counts: Vec::new(),
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
        venue: Some(SYNTHETIC_VENUE.to_string()),
        product_family: Some(SYNTHETIC_PRODUCT_FAMILY.to_string()),
        evidence_state: Some(EvidenceState::DirectlyBackfillable),
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

trait RecordFixtureExt {
    fn with_product_family(self, product_family: &str) -> Self;
    fn with_evidence_state(self, evidence_state: EvidenceState) -> Self;
}

impl RecordFixtureExt for SourceProofLegacyDerivabilityRecord {
    fn with_product_family(mut self, product_family: &str) -> Self {
        self.product_family = Some(product_family.to_string());
        self
    }

    fn with_evidence_state(mut self, evidence_state: EvidenceState) -> Self {
        self.evidence_state = Some(evidence_state);
        self
    }
}

#[derive(Debug, Clone, Copy)]
struct SyntheticBinding<'a> {
    source_binding: &'a str,
    product_family: &'a str,
    table_family: &'a str,
    evidence_state: &'a str,
}

fn binding<'a>(
    source_binding: &'a str,
    product_family: &'a str,
    table_family: &'a str,
) -> SyntheticBinding<'a> {
    binding_with_evidence_state(
        source_binding,
        product_family,
        table_family,
        SYNTHETIC_EVIDENCE_STATE,
    )
}

fn binding_with_evidence_state<'a>(
    source_binding: &'a str,
    product_family: &'a str,
    table_family: &'a str,
    evidence_state: &'a str,
) -> SyntheticBinding<'a> {
    SyntheticBinding {
        source_binding,
        product_family,
        table_family,
        evidence_state,
    }
}

fn source_binding_registry(entries: &[SyntheticBinding<'_>]) -> SourceBindingRegistry {
    SourceBindingRegistry::from_toml_str(&source_binding_registry_toml(entries))
        .expect("source binding registry")
}

fn source_binding_registry_toml(entries: &[SyntheticBinding<'_>]) -> String {
    entries
        .iter()
        .map(|entry| {
            let source_binding = entry.source_binding;
            let product_family = entry.product_family;
            let table_family = entry.table_family;
            let evidence_state = entry.evidence_state;
            format!(
                r#"[[source_binding]]
key = "{source_binding}"
venue = "{SYNTHETIC_VENUE}"
product_family = "{product_family}"
source_uri = "https://synthetic.invalid/data"
evidence_state = "{evidence_state}"
table_families = ["{table_family}"]
"#
            )
        })
        .collect::<Vec<_>>()
        .join("\n")
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
