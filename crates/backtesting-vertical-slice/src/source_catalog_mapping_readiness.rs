//! Source-to-NT catalog mapping readiness gate.
//!
//! This report-only gate makes `BACKTESTING_ENGINE-022` machine-checkable:
//! a selected source/table family can reach backfill execution only after the
//! configured mapping evidence proves the required NT data classes and catalog
//! path status for that exact source binding.

use std::{
    collections::BTreeMap,
    error::Error,
    fmt, fs,
    path::{Path, PathBuf},
};

use serde::{Deserialize, Serialize};

use crate::hashing::sha256_hex;
use crate::source_proof::SourceProofUsageScope;

pub const SOURCE_CATALOG_MAPPING_READINESS_SCHEMA_VERSION: &str =
    "source-catalog-mapping-readiness-report.v2";
pub const SOURCE_CATALOG_MAPPING_READINESS_REPORT_FILE: &str =
    "source-catalog-mapping-readiness-report.json";

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SourceCatalogMappingReadinessSpec {
    pub readiness_id: String,
    pub catalog_mapping_evaluation_path: PathBuf,
    pub output_dir: PathBuf,
    pub source_proof_id: String,
    pub source_proof_version: u32,
    pub source_binding: String,
    pub required_table_family: String,
    pub required_nt_data_types: Vec<String>,
    #[serde(default)]
    pub required_claim_evidence_refs: Vec<String>,
    pub allowed_current_bte_statuses: Vec<String>,
    pub allowed_parquet_catalog_statuses: Vec<String>,
    pub allowed_usage_scopes: Vec<SourceProofUsageScope>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SourceCatalogMappingReadinessStatus {
    Ready,
    Blocked,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SourceCatalogMappingReadinessBlocker {
    EmptyReadinessId,
    EmptySourceProofId,
    EmptySourceBinding,
    EmptyRequiredTableFamily,
    EmptyRequiredNtDataTypes,
    EmptyAllowedCurrentBteStatuses,
    EmptyAllowedParquetCatalogStatuses,
    EmptyAllowedUsageScopes,
    MappingEntryNotFound,
    DuplicateMappingEntries,
    TableFamilyMismatch,
    RequiredNtDataTypeMissing,
    RequiredNtDataTypeEvidenceMissing,
    RequiredClaimEvidenceMissing,
    SourceProofMismatch,
    UsageScopeMissing,
    UsageScopeNotAllowed,
    CurrentBteStatusNotAllowed,
    ParquetCatalogStatusNotAllowed,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SourceCatalogMappingReadinessReport {
    pub schema_version: String,
    pub readiness_id: String,
    pub status: SourceCatalogMappingReadinessStatus,
    pub catalog_mapping_evaluation_hash: String,
    pub source_proof_id: String,
    pub source_proof_version: u32,
    pub source_binding: String,
    pub required_table_family: String,
    pub required_nt_data_types: Vec<String>,
    #[serde(default)]
    pub required_claim_evidence_refs: Vec<String>,
    pub allowed_current_bte_statuses: Vec<String>,
    pub allowed_parquet_catalog_statuses: Vec<String>,
    pub allowed_usage_scopes: Vec<SourceProofUsageScope>,
    pub observed_source_proof_id: Option<String>,
    pub observed_source_proof_version: Option<u32>,
    pub observed_source_binding: Option<String>,
    pub observed_table_family: Option<String>,
    pub observed_usage_scope: Option<SourceProofUsageScope>,
    pub observed_nt_data_types: Vec<String>,
    pub observed_nt_data_type_evidence_refs: BTreeMap<String, Vec<String>>,
    #[serde(default)]
    pub observed_claim_evidence_refs: BTreeMap<String, Vec<String>>,
    pub observed_current_bte_status: Option<String>,
    pub observed_parquet_catalog_status: Option<String>,
    pub nt_catalog_mapping_proven: bool,
    pub blockers: Vec<SourceCatalogMappingReadinessBlocker>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SourceCatalogMappingStatusEntry {
    #[serde(default)]
    pub source_proof_id: Option<String>,
    #[serde(default)]
    pub source_proof_version: Option<u32>,
    pub source_binding: String,
    #[serde(default)]
    pub usage_scope: Option<SourceProofUsageScope>,
    pub table_family: String,
    pub candidate_nt_data_classes: Vec<String>,
    #[serde(default)]
    pub nt_data_class_evidence_refs: BTreeMap<String, Vec<String>>,
    #[serde(default)]
    pub claim_evidence_refs: BTreeMap<String, Vec<String>>,
    pub current_bte_status: String,
    pub parquet_catalog_status: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SourceCatalogMappingReadinessArtifact {
    pub path: PathBuf,
    pub content_hash: String,
    pub bytes: u64,
}

pub struct SourceCatalogMappingReadinessInput<'a> {
    pub readiness_id: &'a str,
    pub catalog_mapping_evaluation_hash: &'a str,
    pub source_sample_mapping_status: &'a [SourceCatalogMappingStatusEntry],
    pub source_proof_id: &'a str,
    pub source_proof_version: u32,
    pub source_binding: &'a str,
    pub required_table_family: &'a str,
    pub required_nt_data_types: Vec<String>,
    pub required_claim_evidence_refs: Vec<String>,
    pub allowed_current_bte_statuses: Vec<String>,
    pub allowed_parquet_catalog_statuses: Vec<String>,
    pub allowed_usage_scopes: Vec<SourceProofUsageScope>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SourceCatalogMappingReadinessError {
    ReadSpec { path: String, error: String },
    ParseSpecToml { path: String, error: String },
    ReadCatalogMappingEvaluation { path: String, error: String },
    ParseCatalogMappingEvaluationJson { path: String, error: String },
    CreateDir { path: String, error: String },
    ReadExisting { path: String, error: String },
    Write { path: String, error: String },
    ExistingArtifactMismatch { path: String },
    Serialize(String),
}

impl fmt::Display for SourceCatalogMappingReadinessError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::ReadSpec { path, error } => {
                write!(
                    f,
                    "read source catalog-mapping readiness spec {path}: {error}"
                )
            }
            Self::ParseSpecToml { path, error } => write!(
                f,
                "parse source catalog-mapping readiness spec TOML {path}: {error}"
            ),
            Self::ReadCatalogMappingEvaluation { path, error } => {
                write!(f, "read source catalog-mapping evaluation {path}: {error}")
            }
            Self::ParseCatalogMappingEvaluationJson { path, error } => write!(
                f,
                "parse source catalog-mapping evaluation JSON {path}: {error}"
            ),
            Self::CreateDir { path, error } => write!(
                f,
                "create source catalog-mapping readiness artifact directory {path}: {error}"
            ),
            Self::ReadExisting { path, error } => write!(
                f,
                "read existing source catalog-mapping readiness artifact {path}: {error}"
            ),
            Self::Write { path, error } => write!(
                f,
                "write source catalog-mapping readiness artifact {path}: {error}"
            ),
            Self::ExistingArtifactMismatch { path } => write!(
                f,
                "dirty source catalog-mapping readiness artifact {path}: existing file content differs"
            ),
            Self::Serialize(error) => write!(
                f,
                "serialize source catalog-mapping readiness artifact: {error}"
            ),
        }
    }
}

impl Error for SourceCatalogMappingReadinessError {}

#[must_use]
pub fn evaluate_source_catalog_mapping_readiness(
    input: SourceCatalogMappingReadinessInput<'_>,
) -> SourceCatalogMappingReadinessReport {
    let readiness_id = input.readiness_id.to_string();
    let source_proof_id = input.source_proof_id.to_string();
    let source_proof_version = input.source_proof_version;
    let source_binding = input.source_binding.to_string();
    let required_table_family = input.required_table_family.to_string();
    let required_nt_data_types = input.required_nt_data_types;
    let required_claim_evidence_refs = input.required_claim_evidence_refs;
    let allowed_current_bte_statuses = input.allowed_current_bte_statuses;
    let allowed_parquet_catalog_statuses = input.allowed_parquet_catalog_statuses;
    let allowed_usage_scopes = input.allowed_usage_scopes;

    let source_proof_id_trimmed = source_proof_id.trim();
    let source_binding_trimmed = source_binding.trim();
    let required_table_family_trimmed = required_table_family.trim();
    let required_nt_data_type_values = required_nt_data_types
        .iter()
        .map(|value| value.trim())
        .filter(|value| !value.is_empty())
        .collect::<Vec<_>>();
    let required_claim_evidence_ref_values = required_claim_evidence_refs
        .iter()
        .map(|value| value.trim())
        .filter(|value| !value.is_empty())
        .collect::<Vec<_>>();

    let binding_entries = input
        .source_sample_mapping_status
        .iter()
        .filter(|entry| entry.source_binding.trim() == source_binding_trimmed)
        .collect::<Vec<_>>();
    let table_entries = binding_entries
        .iter()
        .copied()
        .filter(|entry| entry.table_family.trim() == required_table_family_trimmed)
        .collect::<Vec<_>>();
    let observed_entry = table_entries
        .first()
        .copied()
        .or_else(|| binding_entries.first().copied());

    let mut blockers = Vec::new();
    if readiness_id.trim().is_empty() {
        blockers.push(SourceCatalogMappingReadinessBlocker::EmptyReadinessId);
    }
    if source_proof_id_trimmed.is_empty() {
        blockers.push(SourceCatalogMappingReadinessBlocker::EmptySourceProofId);
    }
    if source_binding_trimmed.is_empty() {
        blockers.push(SourceCatalogMappingReadinessBlocker::EmptySourceBinding);
    }
    if required_table_family_trimmed.is_empty() {
        blockers.push(SourceCatalogMappingReadinessBlocker::EmptyRequiredTableFamily);
    }
    if required_nt_data_type_values.len() != required_nt_data_types.len() {
        blockers.push(SourceCatalogMappingReadinessBlocker::EmptyRequiredNtDataTypes);
    }
    if required_nt_data_types.is_empty() {
        blockers.push(SourceCatalogMappingReadinessBlocker::EmptyRequiredNtDataTypes);
    }
    if required_claim_evidence_ref_values.len() != required_claim_evidence_refs.len() {
        blockers.push(SourceCatalogMappingReadinessBlocker::RequiredClaimEvidenceMissing);
    }
    if allowed_current_bte_statuses
        .iter()
        .map(|value| value.trim())
        .filter(|value| !value.is_empty())
        .count()
        != allowed_current_bte_statuses.len()
        || allowed_current_bte_statuses.is_empty()
    {
        blockers.push(SourceCatalogMappingReadinessBlocker::EmptyAllowedCurrentBteStatuses);
    }
    if allowed_parquet_catalog_statuses
        .iter()
        .map(|value| value.trim())
        .filter(|value| !value.is_empty())
        .count()
        != allowed_parquet_catalog_statuses.len()
        || allowed_parquet_catalog_statuses.is_empty()
    {
        blockers.push(SourceCatalogMappingReadinessBlocker::EmptyAllowedParquetCatalogStatuses);
    }
    if allowed_usage_scopes.is_empty() {
        blockers.push(SourceCatalogMappingReadinessBlocker::EmptyAllowedUsageScopes);
    }
    if binding_entries.is_empty() && !source_binding_trimmed.is_empty() {
        blockers.push(SourceCatalogMappingReadinessBlocker::MappingEntryNotFound);
    }
    if !binding_entries.is_empty()
        && table_entries.is_empty()
        && !required_table_family_trimmed.is_empty()
    {
        blockers.push(SourceCatalogMappingReadinessBlocker::TableFamilyMismatch);
    }
    if table_entries.len() > 1 {
        blockers.push(SourceCatalogMappingReadinessBlocker::DuplicateMappingEntries);
    }

    if let Some(entry) = observed_entry
        && table_entries.len() == 1
    {
        let observed_nt_data_types = entry
            .candidate_nt_data_classes
            .iter()
            .map(|value| value.trim())
            .collect::<Vec<_>>();
        if !required_nt_data_type_values.iter().all(|required| {
            observed_nt_data_types
                .iter()
                .any(|observed| observed == required)
        }) {
            blockers.push(SourceCatalogMappingReadinessBlocker::RequiredNtDataTypeMissing);
        }
        if required_nt_data_type_values.iter().any(|required| {
            match entry
                .nt_data_class_evidence_refs
                .iter()
                .find(|(data_type, _)| data_type.trim() == *required)
            {
                Some((_, refs)) => {
                    refs.is_empty()
                        || refs
                            .iter()
                            .any(|evidence_ref| evidence_ref.trim().is_empty())
                }
                None => true,
            }
        }) {
            blockers.push(SourceCatalogMappingReadinessBlocker::RequiredNtDataTypeEvidenceMissing);
        }
        if required_claim_evidence_ref_values.iter().any(|required| {
            !entry
                .claim_evidence_refs
                .values()
                .flat_map(|refs| refs.iter())
                .map(|evidence_ref| evidence_ref.trim())
                .any(|evidence_ref| evidence_ref == *required)
        }) {
            blockers.push(SourceCatalogMappingReadinessBlocker::RequiredClaimEvidenceMissing);
        }
        if entry.source_proof_id.as_deref().map(str::trim) != Some(source_proof_id_trimmed)
            || entry.source_proof_version != Some(source_proof_version)
        {
            blockers.push(SourceCatalogMappingReadinessBlocker::SourceProofMismatch);
        }
        if let Some(usage_scope) = entry.usage_scope {
            if !allowed_usage_scopes.contains(&usage_scope) {
                blockers.push(SourceCatalogMappingReadinessBlocker::UsageScopeNotAllowed);
            }
        } else {
            blockers.push(SourceCatalogMappingReadinessBlocker::UsageScopeMissing);
        }
        if !allowed_current_bte_statuses
            .iter()
            .any(|status| status.trim() == entry.current_bte_status.trim())
        {
            blockers.push(SourceCatalogMappingReadinessBlocker::CurrentBteStatusNotAllowed);
        }
        if !allowed_parquet_catalog_statuses
            .iter()
            .any(|status| status.trim() == entry.parquet_catalog_status.trim())
        {
            blockers.push(SourceCatalogMappingReadinessBlocker::ParquetCatalogStatusNotAllowed);
        }
    }

    let status = if blockers.is_empty() {
        SourceCatalogMappingReadinessStatus::Ready
    } else {
        SourceCatalogMappingReadinessStatus::Blocked
    };
    let nt_catalog_mapping_proven = status == SourceCatalogMappingReadinessStatus::Ready;

    SourceCatalogMappingReadinessReport {
        schema_version: SOURCE_CATALOG_MAPPING_READINESS_SCHEMA_VERSION.to_string(),
        readiness_id,
        status,
        catalog_mapping_evaluation_hash: input.catalog_mapping_evaluation_hash.to_string(),
        source_proof_id,
        source_proof_version,
        source_binding,
        required_table_family,
        required_nt_data_types,
        required_claim_evidence_refs,
        allowed_current_bte_statuses,
        allowed_parquet_catalog_statuses,
        allowed_usage_scopes,
        observed_source_proof_id: observed_entry.and_then(|entry| entry.source_proof_id.clone()),
        observed_source_proof_version: observed_entry.and_then(|entry| entry.source_proof_version),
        observed_source_binding: observed_entry.map(|entry| entry.source_binding.clone()),
        observed_table_family: observed_entry.map(|entry| entry.table_family.clone()),
        observed_usage_scope: observed_entry.and_then(|entry| entry.usage_scope),
        observed_nt_data_types: observed_entry
            .map(|entry| entry.candidate_nt_data_classes.clone())
            .unwrap_or_default(),
        observed_nt_data_type_evidence_refs: observed_entry
            .map(|entry| entry.nt_data_class_evidence_refs.clone())
            .unwrap_or_default(),
        observed_claim_evidence_refs: observed_entry
            .map(|entry| entry.claim_evidence_refs.clone())
            .unwrap_or_default(),
        observed_current_bte_status: observed_entry.map(|entry| entry.current_bte_status.clone()),
        observed_parquet_catalog_status: observed_entry
            .map(|entry| entry.parquet_catalog_status.clone()),
        nt_catalog_mapping_proven,
        blockers,
    }
}

pub fn write_source_catalog_mapping_readiness_report_from_spec_file(
    spec_path: &Path,
) -> Result<SourceCatalogMappingReadinessArtifact, SourceCatalogMappingReadinessError> {
    let spec_path_display = spec_path.display().to_string();
    let spec_text = fs::read_to_string(spec_path).map_err(|error| {
        SourceCatalogMappingReadinessError::ReadSpec {
            path: spec_path_display.clone(),
            error: error.to_string(),
        }
    })?;
    let spec: SourceCatalogMappingReadinessSpec = toml::from_str(&spec_text).map_err(|error| {
        SourceCatalogMappingReadinessError::ParseSpecToml {
            path: spec_path_display,
            error: error.to_string(),
        }
    })?;
    let evaluation_path = spec.catalog_mapping_evaluation_path.display().to_string();
    let evaluation_bytes = fs::read(&spec.catalog_mapping_evaluation_path).map_err(|error| {
        SourceCatalogMappingReadinessError::ReadCatalogMappingEvaluation {
            path: evaluation_path.clone(),
            error: error.to_string(),
        }
    })?;
    let evaluation: SourceCatalogMappingEvaluation = serde_json::from_slice(&evaluation_bytes)
        .map_err(
            |error| SourceCatalogMappingReadinessError::ParseCatalogMappingEvaluationJson {
                path: evaluation_path,
                error: error.to_string(),
            },
        )?;
    let evaluation_hash = sha256_hex(&evaluation_bytes);

    let report = evaluate_source_catalog_mapping_readiness(SourceCatalogMappingReadinessInput {
        readiness_id: &spec.readiness_id,
        catalog_mapping_evaluation_hash: &evaluation_hash,
        source_sample_mapping_status: &evaluation.source_sample_mapping_status,
        source_proof_id: &spec.source_proof_id,
        source_proof_version: spec.source_proof_version,
        source_binding: &spec.source_binding,
        required_table_family: &spec.required_table_family,
        required_nt_data_types: spec.required_nt_data_types,
        required_claim_evidence_refs: spec.required_claim_evidence_refs,
        allowed_current_bte_statuses: spec.allowed_current_bte_statuses,
        allowed_parquet_catalog_statuses: spec.allowed_parquet_catalog_statuses,
        allowed_usage_scopes: spec.allowed_usage_scopes,
    });
    write_source_catalog_mapping_readiness_report(&spec.output_dir, &report)
}

pub fn write_source_catalog_mapping_readiness_report(
    output_dir: &Path,
    report: &SourceCatalogMappingReadinessReport,
) -> Result<SourceCatalogMappingReadinessArtifact, SourceCatalogMappingReadinessError> {
    fs::create_dir_all(output_dir).map_err(|error| {
        SourceCatalogMappingReadinessError::CreateDir {
            path: output_dir.display().to_string(),
            error: error.to_string(),
        }
    })?;
    let path = output_dir.join(SOURCE_CATALOG_MAPPING_READINESS_REPORT_FILE);
    let written = crate::reference_artifact::write_reference_artifact_with_len_mapped(
        &path,
        SOURCE_CATALOG_MAPPING_READINESS_REPORT_FILE,
        report,
        crate::reference_artifact::ReferenceArtifactRewrite::FailOnDirty,
        crate::reference_artifact::ReferenceArtifactErrorMappers {
            serialize_error: SourceCatalogMappingReadinessError::Serialize,
            read_existing_error: |path, error| SourceCatalogMappingReadinessError::ReadExisting {
                path,
                error,
            },
            mismatch_error: |path| SourceCatalogMappingReadinessError::ExistingArtifactMismatch {
                path,
            },
            write_error: |path, error| SourceCatalogMappingReadinessError::Write { path, error },
        },
    )?;
    Ok(SourceCatalogMappingReadinessArtifact {
        path,
        content_hash: written.pin.sha256,
        bytes: written.bytes,
    })
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
struct SourceCatalogMappingEvaluation {
    source_sample_mapping_status: Vec<SourceCatalogMappingStatusEntry>,
}
