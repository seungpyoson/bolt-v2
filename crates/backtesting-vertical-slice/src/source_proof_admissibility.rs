//! Source-proof admissibility reporting before broad backfill.
//!
//! This module does not accept, mutate, migrate, or infer source proofs. It
//! classifies staged source-proof JSON against the current typed
//! `SourceProofReport` contract so broad backfill can proceed only from
//! machine-admissible proof records.

use std::{
    collections::BTreeSet,
    error::Error,
    fmt, fs,
    path::{Path, PathBuf},
};

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::source_proof::{
    SourceBindingRegistry, SourceProofReport, read_source_binding_registry_from_path,
};

pub const SOURCE_PROOF_ADMISSIBILITY_SCHEMA_VERSION: &str = "source-proof-admissibility-report.v1";
pub const SOURCE_PROOF_ADMISSIBILITY_REPORT_FILE: &str = "source-proof-admissibility-report.json";

const CURRENT_CONTRACT_TOP_LEVEL_FIELDS: &[&str] = &[
    "source_proof_id",
    "source_proof_version",
    "contract_version",
    "schema_version",
    "status",
    "source_binding",
    "venue",
    "product_family",
    "product_category",
    "table_family",
    "evidence_state",
    "source_candidate_class",
    "source_selection_status",
    "usage_scope",
    "fixture_type",
    "requested_time_range",
    "coverage_time_range",
    "instrument_universe_id",
    "raw_sample_uri",
    "raw_sample_hash",
    "schema_sample_uri",
    "schema_sample_hash",
    "license_ref",
    "license_scope",
    "retention_ref",
    "cost_ref",
    "nt_mapping_status",
    "fidelity_class",
    "l2_replay_evidence",
    "forbidden_claims",
    "claim_limits",
    "acceptance_scope",
    "gap_policy_id",
    "required_checks",
];

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SourceProofAdmissibilityStatus {
    AcceptReady,
    CurrentContractRejected,
    NonCurrentContract,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SourceProofAdmissibilityIssue {
    MissingCurrentContractField,
    CurrentContractDeserializeFailed,
    AcceptanceFailed,
    LegacySourceBindingKeyField,
    LegacyTableFamiliesField,
    LegacyRawPayloadRecordsField,
    LegacyScalarRequiredChecks,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SourceProofAdmissibilityRecord {
    pub proof_uri: String,
    pub status: SourceProofAdmissibilityStatus,
    pub source_proof_id: Option<String>,
    pub source_proof_version: Option<u32>,
    pub source_binding: Option<String>,
    pub current_contract_deserializes: bool,
    pub missing_current_contract_fields: Vec<String>,
    pub blocking_issues: Vec<SourceProofAdmissibilityIssue>,
    pub acceptance_error: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SourceProofAdmissibilitySummary {
    pub total_records: u64,
    pub current_contract_records: u64,
    pub accept_ready_records: u64,
    pub current_contract_rejected_records: u64,
    pub non_current_contract_records: u64,
    pub blocking_issue_count: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SourceProofAdmissibilityReport {
    pub schema_version: String,
    pub report_id: String,
    pub records: Vec<SourceProofAdmissibilityRecord>,
    pub summary: SourceProofAdmissibilitySummary,
}

#[derive(Debug, Clone, PartialEq)]
pub struct SourceProofAdmissibilityJson {
    pub proof_uri: String,
    pub proof: Value,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SourceProofAdmissibilityProofFile {
    pub proof_uri: String,
    pub path: PathBuf,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SourceProofAdmissibilitySpec {
    pub report_id: String,
    pub output_dir: PathBuf,
    pub source_bindings_path: PathBuf,
    #[serde(rename = "source_proof", default)]
    pub source_proofs: Vec<SourceProofAdmissibilityProofFile>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SourceProofAdmissibilityArtifact {
    pub path: PathBuf,
    pub content_hash: String,
    pub bytes: u64,
    pub record_count: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SourceProofAdmissibilityReportError {
    EmptyReportId,
    EmptyProofUri,
    DuplicateProofUri(String),
    Serialize(String),
}

impl fmt::Display for SourceProofAdmissibilityReportError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::EmptyReportId => write!(f, "source-proof admissibility report id is empty"),
            Self::EmptyProofUri => write!(f, "source-proof admissibility proof uri is empty"),
            Self::DuplicateProofUri(proof_uri) => write!(
                f,
                "source-proof admissibility report has duplicate proof uri {proof_uri:?}"
            ),
            Self::Serialize(error) => {
                write!(f, "serialize source-proof admissibility report: {error}")
            }
        }
    }
}

impl Error for SourceProofAdmissibilityReportError {}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SourceProofAdmissibilityWriteError {
    CreateDir { path: String, error: String },
    ReadExisting { path: String, error: String },
    Write { path: String, error: String },
    Serialize(String),
    ExistingArtifactMismatch { path: String },
}

impl fmt::Display for SourceProofAdmissibilityWriteError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::CreateDir { path, error } => {
                write!(
                    f,
                    "create source-proof admissibility artifact directory {path}: {error}"
                )
            }
            Self::ReadExisting { path, error } => {
                write!(
                    f,
                    "read existing source-proof admissibility artifact {path}: {error}"
                )
            }
            Self::Write { path, error } => {
                write!(
                    f,
                    "write source-proof admissibility artifact {path}: {error}"
                )
            }
            Self::Serialize(error) => {
                write!(f, "serialize source-proof admissibility artifact: {error}")
            }
            Self::ExistingArtifactMismatch { path } => write!(
                f,
                "dirty source-proof admissibility artifact {path}: existing file content differs"
            ),
        }
    }
}

impl Error for SourceProofAdmissibilityWriteError {}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SourceProofAdmissibilityFileError {
    ReadSpec {
        path: String,
        error: String,
    },
    ParseSpecToml {
        path: String,
        error: String,
    },
    ReadSourceBindings {
        path: String,
        error: String,
    },
    ReadSourceProof {
        proof_uri: String,
        path: String,
        error: String,
    },
    ParseSourceProofJson {
        proof_uri: String,
        path: String,
        error: String,
    },
    BuildReport(SourceProofAdmissibilityReportError),
    WriteArtifact(SourceProofAdmissibilityWriteError),
}

impl fmt::Display for SourceProofAdmissibilityFileError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::ReadSpec { path, error } => {
                write!(f, "read source-proof admissibility spec {path}: {error}")
            }
            Self::ParseSpecToml { path, error } => write!(
                f,
                "parse source-proof admissibility spec TOML {path}: {error}"
            ),
            Self::ReadSourceBindings { path, error } => {
                write!(f, "read source-bindings registry {path}: {error}")
            }
            Self::ReadSourceProof {
                proof_uri,
                path,
                error,
            } => write!(f, "read source proof {proof_uri} from {path}: {error}"),
            Self::ParseSourceProofJson {
                proof_uri,
                path,
                error,
            } => write!(
                f,
                "parse source proof JSON {proof_uri} from {path}: {error}"
            ),
            Self::BuildReport(error) => {
                write!(f, "build source-proof admissibility report: {error}")
            }
            Self::WriteArtifact(error) => {
                write!(f, "write source-proof admissibility report: {error}")
            }
        }
    }
}

impl Error for SourceProofAdmissibilityFileError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::BuildReport(error) => Some(error),
            Self::WriteArtifact(error) => Some(error),
            _ => None,
        }
    }
}

impl SourceProofAdmissibilityReport {
    pub fn from_json_values(
        report_id: impl Into<String>,
        source_proofs: Vec<SourceProofAdmissibilityJson>,
    ) -> Result<Self, SourceProofAdmissibilityReportError> {
        Self::from_json_values_with_registry(
            report_id,
            source_proofs,
            &crate::source_proof::committed_source_binding_registry(),
        )
    }

    pub fn from_json_values_with_registry(
        report_id: impl Into<String>,
        source_proofs: Vec<SourceProofAdmissibilityJson>,
        registry: &SourceBindingRegistry,
    ) -> Result<Self, SourceProofAdmissibilityReportError> {
        let report_id = report_id.into();
        if report_id.trim().is_empty() {
            return Err(SourceProofAdmissibilityReportError::EmptyReportId);
        }

        let mut proof_uris = BTreeSet::new();
        let mut records = Vec::with_capacity(source_proofs.len());
        for source_proof in source_proofs {
            if source_proof.proof_uri.trim().is_empty() {
                return Err(SourceProofAdmissibilityReportError::EmptyProofUri);
            }
            if !proof_uris.insert(source_proof.proof_uri.clone()) {
                return Err(SourceProofAdmissibilityReportError::DuplicateProofUri(
                    source_proof.proof_uri,
                ));
            }
            records.push(classify_source_proof_json(source_proof, registry));
        }

        let summary = SourceProofAdmissibilitySummary::from_records(&records);
        Ok(Self {
            schema_version: SOURCE_PROOF_ADMISSIBILITY_SCHEMA_VERSION.to_string(),
            report_id,
            records,
            summary,
        })
    }

    pub fn content_hash(&self) -> Result<String, SourceProofAdmissibilityReportError> {
        crate::reference_artifact::canonical_json_sha256(self)
            .map_err(|error| SourceProofAdmissibilityReportError::Serialize(error.to_string()))
    }
}

impl SourceProofAdmissibilitySummary {
    fn from_records(records: &[SourceProofAdmissibilityRecord]) -> Self {
        let mut summary = Self {
            total_records: records.len() as u64,
            current_contract_records: 0,
            accept_ready_records: 0,
            current_contract_rejected_records: 0,
            non_current_contract_records: 0,
            blocking_issue_count: 0,
        };
        for record in records {
            match record.status {
                SourceProofAdmissibilityStatus::AcceptReady => {
                    summary.current_contract_records += 1;
                    summary.accept_ready_records += 1;
                }
                SourceProofAdmissibilityStatus::CurrentContractRejected => {
                    summary.current_contract_records += 1;
                    summary.current_contract_rejected_records += 1;
                }
                SourceProofAdmissibilityStatus::NonCurrentContract => {
                    summary.non_current_contract_records += 1;
                }
            }
            summary.blocking_issue_count = summary
                .blocking_issue_count
                .saturating_add(record.blocking_issues.len() as u64);
        }
        summary
    }
}

pub fn write_source_proof_admissibility_report(
    output_dir: &Path,
    report: &SourceProofAdmissibilityReport,
) -> Result<SourceProofAdmissibilityArtifact, SourceProofAdmissibilityWriteError> {
    fs::create_dir_all(output_dir).map_err(|error| {
        SourceProofAdmissibilityWriteError::CreateDir {
            path: output_dir.display().to_string(),
            error: error.to_string(),
        }
    })?;

    let path = output_dir.join(SOURCE_PROOF_ADMISSIBILITY_REPORT_FILE);
    let written = crate::reference_artifact::write_reference_artifact_with_len_mapped(
        &path,
        SOURCE_PROOF_ADMISSIBILITY_REPORT_FILE,
        report,
        SourceProofAdmissibilityWriteError::Serialize,
        |path, error| SourceProofAdmissibilityWriteError::ReadExisting { path, error },
        |path| SourceProofAdmissibilityWriteError::ExistingArtifactMismatch { path },
        |path, error| SourceProofAdmissibilityWriteError::Write { path, error },
    )?;
    Ok(SourceProofAdmissibilityArtifact {
        path,
        content_hash: written.pin.sha256,
        bytes: written.bytes,
        record_count: report.records.len() as u64,
    })
}

pub fn write_source_proof_admissibility_report_from_spec_file(
    spec_path: &Path,
) -> Result<SourceProofAdmissibilityArtifact, SourceProofAdmissibilityFileError> {
    let path_display = spec_path.display().to_string();
    let spec_text = fs::read_to_string(spec_path).map_err(|error| {
        SourceProofAdmissibilityFileError::ReadSpec {
            path: path_display.clone(),
            error: error.to_string(),
        }
    })?;
    let spec: SourceProofAdmissibilitySpec = toml::from_str(&spec_text).map_err(|error| {
        SourceProofAdmissibilityFileError::ParseSpecToml {
            path: path_display,
            error: error.to_string(),
        }
    })?;
    let source_bindings_path = spec.source_bindings_path.display().to_string();
    let source_bindings_registry =
        read_source_binding_registry_from_path(&spec.source_bindings_path).map_err(|error| {
            SourceProofAdmissibilityFileError::ReadSourceBindings {
                path: source_bindings_path,
                error: error.to_string(),
            }
        })?;
    write_source_proof_admissibility_report_from_files(
        &spec.output_dir,
        spec.report_id,
        spec.source_proofs,
        &source_bindings_registry,
    )
}

pub fn write_source_proof_admissibility_report_from_files(
    output_dir: &Path,
    report_id: impl Into<String>,
    source_proof_files: Vec<SourceProofAdmissibilityProofFile>,
    source_bindings_registry: &SourceBindingRegistry,
) -> Result<SourceProofAdmissibilityArtifact, SourceProofAdmissibilityFileError> {
    let source_proofs = source_proof_files
        .into_iter()
        .map(|source_proof| {
            let SourceProofAdmissibilityProofFile { proof_uri, path } = source_proof;
            let path_display = path.display().to_string();
            let bytes = fs::read(&path).map_err(|error| {
                SourceProofAdmissibilityFileError::ReadSourceProof {
                    proof_uri: proof_uri.clone(),
                    path: path_display.clone(),
                    error: error.to_string(),
                }
            })?;
            let proof: Value = serde_json::from_slice(&bytes).map_err(|error| {
                SourceProofAdmissibilityFileError::ParseSourceProofJson {
                    proof_uri: proof_uri.clone(),
                    path: path_display,
                    error: error.to_string(),
                }
            })?;
            Ok(SourceProofAdmissibilityJson { proof_uri, proof })
        })
        .collect::<Result<Vec<_>, _>>()?;
    let report = SourceProofAdmissibilityReport::from_json_values_with_registry(
        report_id,
        source_proofs,
        source_bindings_registry,
    )
    .map_err(SourceProofAdmissibilityFileError::BuildReport)?;
    write_source_proof_admissibility_report(output_dir, &report)
        .map_err(SourceProofAdmissibilityFileError::WriteArtifact)
}

fn classify_source_proof_json(
    source_proof: SourceProofAdmissibilityJson,
    registry: &SourceBindingRegistry,
) -> SourceProofAdmissibilityRecord {
    let SourceProofAdmissibilityJson { proof_uri, proof } = source_proof;
    let missing_current_contract_fields = missing_current_contract_fields(&proof);
    let source_proof_id = string_field(&proof, "source_proof_id");
    let source_proof_version = u32_field(&proof, "source_proof_version");
    let source_binding = string_field(&proof, "source_binding")
        .or_else(|| string_field(&proof, "source_binding_key"));

    match serde_json::from_value::<SourceProofReport>(proof.clone()) {
        Ok(report) => {
            let mut blocking_issues = Vec::new();
            if !missing_current_contract_fields.is_empty() {
                blocking_issues.push(SourceProofAdmissibilityIssue::MissingCurrentContractField);
            }
            match report.evaluate_acceptance_with_registry(registry) {
                Ok(()) if blocking_issues.is_empty() => SourceProofAdmissibilityRecord {
                    proof_uri,
                    status: SourceProofAdmissibilityStatus::AcceptReady,
                    source_proof_id: Some(report.source_proof_id),
                    source_proof_version: Some(report.source_proof_version),
                    source_binding: Some(report.source_binding),
                    current_contract_deserializes: true,
                    missing_current_contract_fields,
                    blocking_issues,
                    acceptance_error: None,
                },
                Ok(()) => SourceProofAdmissibilityRecord {
                    proof_uri,
                    status: SourceProofAdmissibilityStatus::CurrentContractRejected,
                    source_proof_id: Some(report.source_proof_id),
                    source_proof_version: Some(report.source_proof_version),
                    source_binding: Some(report.source_binding),
                    current_contract_deserializes: true,
                    missing_current_contract_fields,
                    blocking_issues,
                    acceptance_error: None,
                },
                Err(error) => {
                    blocking_issues.push(SourceProofAdmissibilityIssue::AcceptanceFailed);
                    SourceProofAdmissibilityRecord {
                        proof_uri,
                        status: SourceProofAdmissibilityStatus::CurrentContractRejected,
                        source_proof_id: Some(report.source_proof_id),
                        source_proof_version: Some(report.source_proof_version),
                        source_binding: Some(report.source_binding),
                        current_contract_deserializes: true,
                        missing_current_contract_fields,
                        blocking_issues,
                        acceptance_error: Some(error.to_string()),
                    }
                }
            }
        }
        Err(_) => {
            let mut blocking_issues = legacy_shape_issues(&proof);
            if !missing_current_contract_fields.is_empty() {
                blocking_issues.insert(
                    0,
                    SourceProofAdmissibilityIssue::MissingCurrentContractField,
                );
            }
            blocking_issues.push(SourceProofAdmissibilityIssue::CurrentContractDeserializeFailed);
            SourceProofAdmissibilityRecord {
                proof_uri,
                status: SourceProofAdmissibilityStatus::NonCurrentContract,
                source_proof_id,
                source_proof_version,
                source_binding,
                current_contract_deserializes: false,
                missing_current_contract_fields,
                blocking_issues,
                acceptance_error: None,
            }
        }
    }
}

fn missing_current_contract_fields(proof: &Value) -> Vec<String> {
    CURRENT_CONTRACT_TOP_LEVEL_FIELDS
        .iter()
        .copied()
        .filter(|field| proof.get(field).is_none_or(Value::is_null))
        .map(str::to_string)
        .collect()
}

fn legacy_shape_issues(proof: &Value) -> Vec<SourceProofAdmissibilityIssue> {
    let mut issues = Vec::new();
    if proof.get("source_binding_key").is_some() {
        issues.push(SourceProofAdmissibilityIssue::LegacySourceBindingKeyField);
    }
    if proof.get("table_families").is_some() {
        issues.push(SourceProofAdmissibilityIssue::LegacyTableFamiliesField);
    }
    if proof.get("raw_payload_records").is_some() {
        issues.push(SourceProofAdmissibilityIssue::LegacyRawPayloadRecordsField);
    }
    if has_scalar_required_checks(proof) {
        issues.push(SourceProofAdmissibilityIssue::LegacyScalarRequiredChecks);
    }
    issues
}

fn has_scalar_required_checks(proof: &Value) -> bool {
    proof
        .get("required_checks")
        .and_then(Value::as_object)
        .is_some_and(|checks| checks.values().any(|check| !check.is_object()))
}

fn string_field(proof: &Value, field: &str) -> Option<String> {
    proof
        .get(field)
        .and_then(Value::as_str)
        .map(str::to_string)
        .filter(|value| !value.trim().is_empty())
}

fn u32_field(proof: &Value, field: &str) -> Option<u32> {
    let value = proof.get(field)?;
    let raw = value.as_u64()?;
    u32::try_from(raw).ok()
}
