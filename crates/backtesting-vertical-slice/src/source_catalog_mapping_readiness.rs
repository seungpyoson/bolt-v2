//! Source-to-NT catalog mapping readiness gate.
//!
//! This report-only gate makes `BACKTESTING_ENGINE-022` machine-checkable:
//! a selected source/table family can reach backfill execution only after the
//! configured mapping evidence proves the required NT data classes and catalog
//! path status for that exact source binding.

use std::{
    error::Error,
    fmt, fs,
    path::{Path, PathBuf},
};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

pub const SOURCE_CATALOG_MAPPING_READINESS_SCHEMA_VERSION: &str =
    "source-catalog-mapping-readiness-report.v1";
pub const SOURCE_CATALOG_MAPPING_READINESS_REPORT_FILE: &str =
    "source-catalog-mapping-readiness-report.json";

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SourceCatalogMappingReadinessSpec {
    pub readiness_id: String,
    pub catalog_mapping_evaluation_path: PathBuf,
    pub output_dir: PathBuf,
    pub source_binding: String,
    pub required_table_family: String,
    pub required_nt_data_types: Vec<String>,
    pub allowed_current_bte_statuses: Vec<String>,
    pub allowed_parquet_catalog_statuses: Vec<String>,
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
    EmptySourceBinding,
    EmptyRequiredTableFamily,
    EmptyRequiredNtDataTypes,
    EmptyAllowedCurrentBteStatuses,
    EmptyAllowedParquetCatalogStatuses,
    MappingEntryNotFound,
    DuplicateMappingEntries,
    TableFamilyMismatch,
    RequiredNtDataTypeMissing,
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
    pub source_binding: String,
    pub required_table_family: String,
    pub required_nt_data_types: Vec<String>,
    pub allowed_current_bte_statuses: Vec<String>,
    pub allowed_parquet_catalog_statuses: Vec<String>,
    pub observed_source_binding: Option<String>,
    pub observed_table_family: Option<String>,
    pub observed_nt_data_types: Vec<String>,
    pub observed_current_bte_status: Option<String>,
    pub observed_parquet_catalog_status: Option<String>,
    pub nt_catalog_mapping_proven: bool,
    pub blockers: Vec<SourceCatalogMappingReadinessBlocker>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SourceCatalogMappingStatusEntry {
    pub source_binding: String,
    pub table_family: String,
    pub candidate_nt_data_classes: Vec<String>,
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
    pub source_binding: &'a str,
    pub required_table_family: &'a str,
    pub required_nt_data_types: Vec<String>,
    pub allowed_current_bte_statuses: Vec<String>,
    pub allowed_parquet_catalog_statuses: Vec<String>,
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
                write!(f, "read source catalog-mapping readiness spec {path}: {error}")
            }
            Self::ParseSpecToml { path, error } => write!(
                f,
                "parse source catalog-mapping readiness spec TOML {path}: {error}"
            ),
            Self::ReadCatalogMappingEvaluation { path, error } => write!(
                f,
                "read source catalog-mapping evaluation {path}: {error}"
            ),
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
    let source_binding = input.source_binding.to_string();
    let required_table_family = input.required_table_family.to_string();
    let required_nt_data_types = input.required_nt_data_types;
    let allowed_current_bte_statuses = input.allowed_current_bte_statuses;
    let allowed_parquet_catalog_statuses = input.allowed_parquet_catalog_statuses;

    let source_binding_trimmed = source_binding.trim();
    let required_table_family_trimmed = required_table_family.trim();
    let required_nt_data_type_values = required_nt_data_types
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

    if let Some(entry) = observed_entry {
        if table_entries.len() == 1 {
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
                blockers.push(
                    SourceCatalogMappingReadinessBlocker::ParquetCatalogStatusNotAllowed,
                );
            }
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
        source_binding,
        required_table_family,
        required_nt_data_types,
        allowed_current_bte_statuses,
        allowed_parquet_catalog_statuses,
        observed_source_binding: observed_entry.map(|entry| entry.source_binding.clone()),
        observed_table_family: observed_entry.map(|entry| entry.table_family.clone()),
        observed_nt_data_types: observed_entry
            .map(|entry| entry.candidate_nt_data_classes.clone())
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
    let spec: SourceCatalogMappingReadinessSpec =
        toml::from_str(&spec_text).map_err(|error| {
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
    let evaluation: SourceCatalogMappingEvaluation =
        serde_json::from_slice(&evaluation_bytes).map_err(|error| {
            SourceCatalogMappingReadinessError::ParseCatalogMappingEvaluationJson {
                path: evaluation_path,
                error: error.to_string(),
            }
        })?;
    let evaluation_hash = sha256_bytes(&evaluation_bytes);

    let report = evaluate_source_catalog_mapping_readiness(SourceCatalogMappingReadinessInput {
        readiness_id: &spec.readiness_id,
        catalog_mapping_evaluation_hash: &evaluation_hash,
        source_sample_mapping_status: &evaluation.source_sample_mapping_status,
        source_binding: &spec.source_binding,
        required_table_family: &spec.required_table_family,
        required_nt_data_types: spec.required_nt_data_types,
        allowed_current_bte_statuses: spec.allowed_current_bte_statuses,
        allowed_parquet_catalog_statuses: spec.allowed_parquet_catalog_statuses,
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
    let bytes = serde_json::to_vec_pretty(report)
        .map_err(|error| SourceCatalogMappingReadinessError::Serialize(error.to_string()))?;
    if path.exists() {
        let existing =
            fs::read(&path).map_err(|error| SourceCatalogMappingReadinessError::ReadExisting {
                path: path.display().to_string(),
                error: error.to_string(),
            })?;
        if existing != bytes {
            return Err(SourceCatalogMappingReadinessError::ExistingArtifactMismatch {
                path: path.display().to_string(),
            });
        }
    } else {
        fs::write(&path, &bytes).map_err(|error| SourceCatalogMappingReadinessError::Write {
            path: path.display().to_string(),
            error: error.to_string(),
        })?;
    }
    Ok(SourceCatalogMappingReadinessArtifact {
        path,
        content_hash: content_hash(report)?,
        bytes: bytes.len() as u64,
    })
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
struct SourceCatalogMappingEvaluation {
    source_sample_mapping_status: Vec<SourceCatalogMappingStatusEntry>,
}

fn content_hash(
    report: &SourceCatalogMappingReadinessReport,
) -> Result<String, SourceCatalogMappingReadinessError> {
    let bytes = serde_json::to_vec(report)
        .map_err(|error| SourceCatalogMappingReadinessError::Serialize(error.to_string()))?;
    Ok(sha256_bytes(&bytes))
}

fn sha256_bytes(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}
