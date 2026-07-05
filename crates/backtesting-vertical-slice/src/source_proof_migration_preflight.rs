//! Source-proof migration candidate preflight.
//!
//! This report-only gate consumes the legacy derivability report and selects at
//! most one bounded source-proof candidate for current-contract migration. It
//! does not accept, mutate, download, convert, or infer venue-specific facts.

use std::{
    error::Error,
    fmt, fs,
    path::{Path, PathBuf},
};

use serde::{Deserialize, Serialize};

use crate::{
    source_proof::{EvidenceState, SourceBindingRegistry, read_source_binding_registry_from_path},
    source_proof_legacy_derivability::{
        SourceProofLegacyDerivabilityIssue, SourceProofLegacyDerivabilityRecord,
        SourceProofLegacyDerivabilityReport, SourceProofLegacyDerivableField,
    },
};

pub const SOURCE_PROOF_MIGRATION_PREFLIGHT_SCHEMA_VERSION: &str =
    "source-proof-migration-preflight-report.v1";
pub const SOURCE_PROOF_MIGRATION_PREFLIGHT_REPORT_FILE: &str =
    "source-proof-migration-preflight-report.json";

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SourceProofMigrationPreflightSpec {
    pub preflight_id: String,
    pub derivability_report_path: PathBuf,
    pub source_bindings_path: PathBuf,
    pub output_dir: PathBuf,
    pub selection: SourceProofMigrationPreflightSelection,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SourceProofMigrationPreflightSelection {
    pub allowed_table_families: Vec<String>,
    pub required_derivable_fields: Vec<SourceProofLegacyDerivableField>,
    pub max_raw_payload_records: u64,
    pub max_accepted_bytes_from_s3: u64,
    pub require_single_table_family: bool,
    pub require_s3_bound_payloads: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SourceProofMigrationPreflightStatus {
    CandidateFound,
    Blocked,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SourceProofMigrationPreflightReason {
    EmptyPreflightId,
    EmptyDerivabilityReport,
    EmptyAllowedTableFamilies,
    InvalidRawPayloadBudget,
    InvalidByteBudget,
    NoEligibleCandidate,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SourceProofMigrationPreflightCandidate {
    pub proof_uri: String,
    pub source_proof_id: String,
    pub source_proof_version: u32,
    pub source_binding: String,
    pub table_family: String,
    pub raw_payload_records: u64,
    pub s3_bound_raw_payload_records: u64,
    pub accepted_bytes_from_s3: u64,
    pub derivable_fields: Vec<SourceProofLegacyDerivableField>,
    pub remaining_acceptance_blockers: Vec<SourceProofLegacyDerivabilityIssue>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SourceProofMigrationPreflightReport {
    pub schema_version: String,
    pub preflight_id: String,
    pub derivability_report_id: String,
    pub status: SourceProofMigrationPreflightStatus,
    pub selection: SourceProofMigrationPreflightSelection,
    pub total_records: u64,
    pub eligible_candidate_count: u64,
    pub selected_candidate: Option<SourceProofMigrationPreflightCandidate>,
    pub blocking_reasons: Vec<SourceProofMigrationPreflightReason>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SourceProofMigrationPreflightArtifact {
    pub path: PathBuf,
    pub content_hash: String,
    pub bytes: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SourceProofMigrationPreflightError {
    ReadSpec { path: String, error: String },
    ParseSpecToml { path: String, error: String },
    ReadSourceBindings { path: String, error: String },
    ReadDerivabilityReport { path: String, error: String },
    ParseDerivabilityReportJson { path: String, error: String },
    CreateDir { path: String, error: String },
    ReadExisting { path: String, error: String },
    Write { path: String, error: String },
    ExistingArtifactMismatch { path: String },
    Serialize(String),
}

impl fmt::Display for SourceProofMigrationPreflightError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::ReadSpec { path, error } => {
                write!(
                    f,
                    "read source-proof migration preflight spec {path}: {error}"
                )
            }
            Self::ParseSpecToml { path, error } => write!(
                f,
                "parse source-proof migration preflight spec TOML {path}: {error}"
            ),
            Self::ReadSourceBindings { path, error } => {
                write!(f, "read source-bindings registry {path}: {error}")
            }
            Self::ReadDerivabilityReport { path, error } => {
                write!(f, "read source-proof derivability report {path}: {error}")
            }
            Self::ParseDerivabilityReportJson { path, error } => write!(
                f,
                "parse source-proof derivability report JSON {path}: {error}"
            ),
            Self::CreateDir { path, error } => write!(
                f,
                "create source-proof migration preflight artifact directory {path}: {error}"
            ),
            Self::ReadExisting { path, error } => write!(
                f,
                "read existing source-proof migration preflight artifact {path}: {error}"
            ),
            Self::Write { path, error } => {
                write!(
                    f,
                    "write source-proof migration preflight artifact {path}: {error}"
                )
            }
            Self::ExistingArtifactMismatch { path } => write!(
                f,
                "dirty source-proof migration preflight artifact {path}: existing file content differs"
            ),
            Self::Serialize(error) => {
                write!(
                    f,
                    "serialize source-proof migration preflight artifact: {error}"
                )
            }
        }
    }
}

impl Error for SourceProofMigrationPreflightError {}

pub fn evaluate_source_proof_migration_preflight_with_registry(
    preflight_id: impl Into<String>,
    derivability_report: &SourceProofLegacyDerivabilityReport,
    selection: &SourceProofMigrationPreflightSelection,
    source_bindings_registry: &SourceBindingRegistry,
) -> SourceProofMigrationPreflightReport {
    let preflight_id = preflight_id.into();
    let mut blocking_reasons = Vec::new();
    if preflight_id.trim().is_empty() {
        blocking_reasons.push(SourceProofMigrationPreflightReason::EmptyPreflightId);
    }
    if derivability_report.records.is_empty() {
        blocking_reasons.push(SourceProofMigrationPreflightReason::EmptyDerivabilityReport);
    }
    if selection.allowed_table_families.is_empty() {
        blocking_reasons.push(SourceProofMigrationPreflightReason::EmptyAllowedTableFamilies);
    }
    if selection.max_raw_payload_records == 0 {
        blocking_reasons.push(SourceProofMigrationPreflightReason::InvalidRawPayloadBudget);
    }
    if selection.max_accepted_bytes_from_s3 == 0 {
        blocking_reasons.push(SourceProofMigrationPreflightReason::InvalidByteBudget);
    }

    let mut eligible = derivability_report
        .records
        .iter()
        .filter(|record| is_eligible(record, selection))
        .map(|record| selected_candidate(record, source_bindings_registry))
        .collect::<Vec<_>>();
    eligible.sort_by(|left, right| {
        left.remaining_acceptance_blockers
            .len()
            .cmp(&right.remaining_acceptance_blockers.len())
            .then(
                left.accepted_bytes_from_s3
                    .cmp(&right.accepted_bytes_from_s3),
            )
            .then(left.raw_payload_records.cmp(&right.raw_payload_records))
            .then(left.proof_uri.cmp(&right.proof_uri))
    });

    if eligible.is_empty() && blocking_reasons.is_empty() {
        blocking_reasons.push(SourceProofMigrationPreflightReason::NoEligibleCandidate);
    }

    let selected_candidate = if blocking_reasons.is_empty() {
        eligible.first().cloned()
    } else {
        None
    };
    let status = if selected_candidate.is_some() {
        SourceProofMigrationPreflightStatus::CandidateFound
    } else {
        SourceProofMigrationPreflightStatus::Blocked
    };

    SourceProofMigrationPreflightReport {
        schema_version: SOURCE_PROOF_MIGRATION_PREFLIGHT_SCHEMA_VERSION.to_string(),
        preflight_id,
        derivability_report_id: derivability_report.report_id.clone(),
        status,
        selection: selection.clone(),
        total_records: derivability_report.summary.total_records,
        eligible_candidate_count: eligible.len() as u64,
        selected_candidate,
        blocking_reasons,
    }
}

pub fn write_source_proof_migration_preflight_report(
    output_dir: &Path,
    report: &SourceProofMigrationPreflightReport,
) -> Result<SourceProofMigrationPreflightArtifact, SourceProofMigrationPreflightError> {
    fs::create_dir_all(output_dir).map_err(|error| {
        SourceProofMigrationPreflightError::CreateDir {
            path: output_dir.display().to_string(),
            error: error.to_string(),
        }
    })?;
    let path = output_dir.join(SOURCE_PROOF_MIGRATION_PREFLIGHT_REPORT_FILE);
    let written = crate::reference_artifact::write_reference_artifact_with_len_mapped(
        &path,
        SOURCE_PROOF_MIGRATION_PREFLIGHT_REPORT_FILE,
        report,
        SourceProofMigrationPreflightError::Serialize,
        |path, error| SourceProofMigrationPreflightError::ReadExisting { path, error },
        |path| SourceProofMigrationPreflightError::ExistingArtifactMismatch { path },
        |path, error| SourceProofMigrationPreflightError::Write { path, error },
    )?;
    Ok(SourceProofMigrationPreflightArtifact {
        path,
        content_hash: written.pin.sha256,
        bytes: written.bytes,
    })
}

pub fn write_source_proof_migration_preflight_report_from_spec_file(
    spec_path: &Path,
) -> Result<SourceProofMigrationPreflightArtifact, SourceProofMigrationPreflightError> {
    let path = spec_path.display().to_string();
    let spec_text = fs::read_to_string(spec_path).map_err(|error| {
        SourceProofMigrationPreflightError::ReadSpec {
            path: path.clone(),
            error: error.to_string(),
        }
    })?;
    let spec: SourceProofMigrationPreflightSpec = toml::from_str(&spec_text).map_err(|error| {
        SourceProofMigrationPreflightError::ParseSpecToml {
            path: path.clone(),
            error: error.to_string(),
        }
    })?;
    let source_bindings_path = spec.source_bindings_path.display().to_string();
    let source_bindings_registry =
        read_source_binding_registry_from_path(&spec.source_bindings_path).map_err(|error| {
            SourceProofMigrationPreflightError::ReadSourceBindings {
                path: source_bindings_path,
                error: error.to_string(),
            }
        })?;
    let report_path = spec.derivability_report_path.display().to_string();
    let report_bytes = fs::read(&spec.derivability_report_path).map_err(|error| {
        SourceProofMigrationPreflightError::ReadDerivabilityReport {
            path: report_path.clone(),
            error: error.to_string(),
        }
    })?;
    let derivability_report: SourceProofLegacyDerivabilityReport =
        serde_json::from_slice(&report_bytes).map_err(|error| {
            SourceProofMigrationPreflightError::ParseDerivabilityReportJson {
                path: report_path,
                error: error.to_string(),
            }
        })?;
    let report = evaluate_source_proof_migration_preflight_with_registry(
        spec.preflight_id,
        &derivability_report,
        &spec.selection,
        &source_bindings_registry,
    );
    write_source_proof_migration_preflight_report(&spec.output_dir, &report)
}

fn is_eligible(
    record: &SourceProofLegacyDerivabilityRecord,
    selection: &SourceProofMigrationPreflightSelection,
) -> bool {
    if record
        .source_binding
        .as_deref()
        .unwrap_or("")
        .trim()
        .is_empty()
        || record
            .source_proof_id
            .as_deref()
            .unwrap_or("")
            .trim()
            .is_empty()
        || record.source_proof_version.is_none()
    {
        return false;
    }
    if selection.require_single_table_family && record.table_families.len() != 1 {
        return false;
    }
    let Some(table_family) = record.table_families.first() else {
        return false;
    };
    if !selection
        .allowed_table_families
        .iter()
        .any(|allowed| allowed == table_family)
    {
        return false;
    }
    if selection.require_s3_bound_payloads
        && record.raw_payload_records != record.s3_bound_raw_payload_records
    {
        return false;
    }
    if record.raw_payload_records == 0
        || record.raw_payload_records > selection.max_raw_payload_records
        || record.accepted_bytes_from_s3 == 0
        || record.accepted_bytes_from_s3 > selection.max_accepted_bytes_from_s3
    {
        return false;
    }
    selection
        .required_derivable_fields
        .iter()
        .all(|required| record.derivable_fields.contains(required))
}

fn selected_candidate(
    record: &SourceProofLegacyDerivabilityRecord,
    source_bindings_registry: &SourceBindingRegistry,
) -> SourceProofMigrationPreflightCandidate {
    let mut remaining_acceptance_blockers = record.blocking_issues.clone();
    for issue in source_binding_metadata_blockers(record, source_bindings_registry) {
        if !remaining_acceptance_blockers.contains(&issue) {
            remaining_acceptance_blockers.push(issue);
        }
    }
    SourceProofMigrationPreflightCandidate {
        proof_uri: record.proof_uri.clone(),
        source_proof_id: record.source_proof_id.clone().unwrap_or_default(),
        source_proof_version: record.source_proof_version.unwrap_or_default(),
        source_binding: record.source_binding.clone().unwrap_or_default(),
        table_family: record.table_families.first().cloned().unwrap_or_default(),
        raw_payload_records: record.raw_payload_records,
        s3_bound_raw_payload_records: record.s3_bound_raw_payload_records,
        accepted_bytes_from_s3: record.accepted_bytes_from_s3,
        derivable_fields: record.derivable_fields.clone(),
        remaining_acceptance_blockers,
    }
}

fn source_binding_metadata_blockers(
    record: &SourceProofLegacyDerivabilityRecord,
    source_bindings_registry: &SourceBindingRegistry,
) -> Vec<SourceProofLegacyDerivabilityIssue> {
    let mut blockers = Vec::new();
    let Some(source_binding) = record.source_binding.as_deref() else {
        return blockers;
    };
    let Some(venue) = record.venue.as_deref() else {
        blockers.push(SourceProofLegacyDerivabilityIssue::MissingVenue);
        return blockers;
    };
    let Some(metadata) = source_bindings_registry.source_binding_metadata(source_binding, venue)
    else {
        blockers.push(SourceProofLegacyDerivabilityIssue::UnknownSourceBinding);
        return blockers;
    };
    match record.product_family.as_deref() {
        Some(product_family) if product_family == metadata.product_family => {}
        Some(_) => {
            blockers.push(SourceProofLegacyDerivabilityIssue::SourceBindingProductFamilyMismatch)
        }
        None => blockers.push(SourceProofLegacyDerivabilityIssue::MissingProductFamily),
    }
    match record.evidence_state {
        Some(evidence_state) => {
            if evidence_state != metadata.evidence_state {
                blockers
                    .push(SourceProofLegacyDerivabilityIssue::SourceBindingEvidenceStateMismatch);
            }
            if !is_backfillable_evidence_state(evidence_state) {
                blockers.push(SourceProofLegacyDerivabilityIssue::EvidenceStateNotBackfillable);
            }
        }
        None => blockers.push(SourceProofLegacyDerivabilityIssue::MissingEvidenceState),
    }
    if let Some(table_family) = record.table_families.first()
        && !metadata.table_families.is_empty()
        && !metadata
            .table_families
            .iter()
            .any(|configured| configured == table_family)
    {
        blockers.push(SourceProofLegacyDerivabilityIssue::SourceBindingTableFamilyMismatch);
    }
    blockers
}

fn is_backfillable_evidence_state(evidence_state: EvidenceState) -> bool {
    matches!(
        evidence_state,
        EvidenceState::DirectlyBackfillable | EvidenceState::OwnerArchiveBackfillable
    )
}
