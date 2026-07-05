//! Source-binding coverage over a backfill ledger.
//!
//! This report-only gate answers whether configured source bindings for a
//! required table family appear in the coverage ledger. It does not infer
//! bindings from prefixes or venue names.

use std::{
    collections::{BTreeMap, BTreeSet},
    error::Error,
    fmt, fs,
    path::{Path, PathBuf},
};

use serde::{Deserialize, Serialize};

use crate::backfill_coverage::{BackfillCoverageLedger, BackfillCoverageStatus};
use crate::source_proof::{SourceBindingRegistry, read_source_binding_registry_from_path};

pub const BACKFILL_BINDING_COVERAGE_SCHEMA_VERSION: &str = "backfill-binding-coverage-report.v1";
pub const BACKFILL_BINDING_COVERAGE_REPORT_FILE: &str = "backfill-binding-coverage-report.json";

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BackfillBindingCoverageSpec {
    pub report_id: String,
    pub source_bindings_path: PathBuf,
    pub coverage_ledger_path: PathBuf,
    pub output_dir: PathBuf,
    pub required_table_families: Vec<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BackfillBindingCoverageStatus {
    Ready,
    Blocked,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BackfillBindingCoverageIssue {
    EmptyReportId,
    EmptyRequiredTableFamilies,
    NoConfiguredBindingForRequiredTableFamily,
    NoLedgerRecordsForRequiredTableFamily,
    RequiredBindingWithoutCanonicalReadyCoverage,
    RequiredBindingWithoutAcceptedCoverage,
    EmptySourceBindingRecords,
    MissingTableFamilyRecords,
    UnconfiguredSourceBindingRecords,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BackfillBindingCoverageBinding {
    pub key: String,
    pub table_families: Vec<String>,
    pub required_table_family_match: bool,
    pub ledger_record_count: u64,
    pub canonical_ready_record_count: u64,
    pub accepted_record_count: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BackfillBindingCoverageReport {
    pub schema_version: String,
    pub report_id: String,
    pub status: BackfillBindingCoverageStatus,
    pub required_table_families: Vec<String>,
    pub configured_required_binding_count: u64,
    pub ledger_records_for_required_bindings: u64,
    pub empty_source_binding_record_count: u64,
    #[serde(default)]
    pub missing_table_family_record_count: u64,
    pub unconfigured_source_bindings: Vec<String>,
    pub bindings: Vec<BackfillBindingCoverageBinding>,
    pub blocking_issues: Vec<BackfillBindingCoverageIssue>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BackfillBindingCoverageArtifact {
    pub path: PathBuf,
    pub content_hash: String,
    pub bytes: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BackfillBindingCoverageError {
    ReadSpec { path: String, error: String },
    ParseSpecToml { path: String, error: String },
    ReadSourceBindings { path: String, error: String },
    ParseSourceBindingsToml { path: String, error: String },
    ReadLedger { path: String, error: String },
    ParseLedgerJson { path: String, error: String },
    CreateDir { path: String, error: String },
    ReadExisting { path: String, error: String },
    Write { path: String, error: String },
    ExistingArtifactMismatch { path: String },
    Serialize(String),
}

impl fmt::Display for BackfillBindingCoverageError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::ReadSpec { path, error } => {
                write!(f, "read backfill binding coverage spec {path}: {error}")
            }
            Self::ParseSpecToml { path, error } => write!(
                f,
                "parse backfill binding coverage spec TOML {path}: {error}"
            ),
            Self::ReadSourceBindings { path, error } => {
                write!(f, "read backfill source bindings {path}: {error}")
            }
            Self::ParseSourceBindingsToml { path, error } => {
                write!(f, "parse backfill source bindings TOML {path}: {error}")
            }
            Self::ReadLedger { path, error } => {
                write!(f, "read backfill coverage ledger {path}: {error}")
            }
            Self::ParseLedgerJson { path, error } => {
                write!(f, "parse backfill coverage ledger JSON {path}: {error}")
            }
            Self::CreateDir { path, error } => write!(
                f,
                "create backfill binding coverage artifact directory {path}: {error}"
            ),
            Self::ReadExisting { path, error } => write!(
                f,
                "read existing backfill binding coverage artifact {path}: {error}"
            ),
            Self::Write { path, error } => {
                write!(
                    f,
                    "write backfill binding coverage artifact {path}: {error}"
                )
            }
            Self::ExistingArtifactMismatch { path } => write!(
                f,
                "dirty backfill binding coverage artifact {path}: existing file content differs"
            ),
            Self::Serialize(error) => {
                write!(f, "serialize backfill binding coverage artifact: {error}")
            }
        }
    }
}

impl Error for BackfillBindingCoverageError {}

#[derive(Debug, Clone, PartialEq, Eq)]
struct SourceBinding {
    key: String,
    table_families: Vec<String>,
}

pub fn evaluate_backfill_binding_coverage(
    report_id: impl Into<String>,
    source_bindings_toml: &str,
    ledger: &BackfillCoverageLedger,
    required_table_families: Vec<String>,
) -> Result<BackfillBindingCoverageReport, BackfillBindingCoverageError> {
    let report_id = report_id.into();
    let source_bindings = parse_source_bindings(source_bindings_toml, "inline")?;
    Ok(evaluate_backfill_binding_coverage_from_bindings(
        report_id,
        source_bindings,
        ledger,
        required_table_families,
    ))
}

pub fn write_backfill_binding_coverage_report_from_spec_file(
    spec_path: &Path,
) -> Result<BackfillBindingCoverageArtifact, BackfillBindingCoverageError> {
    let spec_path_display = spec_path.display().to_string();
    let spec_text =
        fs::read_to_string(spec_path).map_err(|error| BackfillBindingCoverageError::ReadSpec {
            path: spec_path_display.clone(),
            error: error.to_string(),
        })?;
    let spec: BackfillBindingCoverageSpec = toml::from_str(&spec_text).map_err(|error| {
        BackfillBindingCoverageError::ParseSpecToml {
            path: spec_path_display,
            error: error.to_string(),
        }
    })?;
    let source_bindings_path = spec.source_bindings_path.display().to_string();
    let registry =
        read_source_binding_registry_from_path(&spec.source_bindings_path).map_err(|error| {
            BackfillBindingCoverageError::ReadSourceBindings {
                path: source_bindings_path.clone(),
                error: error.to_string(),
            }
        })?;
    let source_bindings = source_bindings_from_registry(&registry, &source_bindings_path)?;
    let ledger_path = spec.coverage_ledger_path.display().to_string();
    let ledger_bytes = fs::read(&spec.coverage_ledger_path).map_err(|error| {
        BackfillBindingCoverageError::ReadLedger {
            path: ledger_path.clone(),
            error: error.to_string(),
        }
    })?;
    let ledger: BackfillCoverageLedger =
        serde_json::from_slice(&ledger_bytes).map_err(|error| {
            BackfillBindingCoverageError::ParseLedgerJson {
                path: ledger_path,
                error: error.to_string(),
            }
        })?;
    let report = evaluate_backfill_binding_coverage_from_bindings(
        spec.report_id,
        source_bindings,
        &ledger,
        spec.required_table_families,
    );
    write_backfill_binding_coverage_report(&spec.output_dir, &report)
}

pub fn write_backfill_binding_coverage_report(
    output_dir: &Path,
    report: &BackfillBindingCoverageReport,
) -> Result<BackfillBindingCoverageArtifact, BackfillBindingCoverageError> {
    fs::create_dir_all(output_dir).map_err(|error| BackfillBindingCoverageError::CreateDir {
        path: output_dir.display().to_string(),
        error: error.to_string(),
    })?;
    let path = output_dir.join(BACKFILL_BINDING_COVERAGE_REPORT_FILE);
    let written = crate::reference_artifact::write_reference_artifact_with_len_mapped(
        &path,
        BACKFILL_BINDING_COVERAGE_REPORT_FILE,
        report,
        BackfillBindingCoverageError::Serialize,
        |path, error| BackfillBindingCoverageError::ReadExisting { path, error },
        |path| BackfillBindingCoverageError::ExistingArtifactMismatch { path },
        |path, error| BackfillBindingCoverageError::Write { path, error },
    )?;
    Ok(BackfillBindingCoverageArtifact {
        path,
        content_hash: written.pin.sha256,
        bytes: written.bytes,
    })
}

fn evaluate_backfill_binding_coverage_from_bindings(
    report_id: String,
    source_bindings: Vec<SourceBinding>,
    ledger: &BackfillCoverageLedger,
    required_table_families: Vec<String>,
) -> BackfillBindingCoverageReport {
    let configured_required_binding_count = source_bindings
        .iter()
        .filter(|binding| {
            binding.table_families.iter().any(|family| {
                required_table_families
                    .iter()
                    .any(|required| required == family)
            })
        })
        .count() as u64;

    let mut ledger_counts: BTreeMap<(String, String), (u64, u64, u64)> = BTreeMap::new();
    let mut observed_source_bindings: BTreeSet<String> = BTreeSet::new();
    let mut empty_source_binding_record_count = 0_u64;
    let mut missing_table_family_record_count = 0_u64;
    for record in &ledger.records {
        let Some(source_binding) = record.source_binding.as_deref() else {
            empty_source_binding_record_count += 1;
            continue;
        };
        if source_binding.trim().is_empty() {
            empty_source_binding_record_count += 1;
            continue;
        }
        observed_source_bindings.insert(source_binding.to_string());
        let Some(table_family) = record.table_family.as_deref() else {
            missing_table_family_record_count += 1;
            continue;
        };
        if table_family.trim().is_empty() {
            missing_table_family_record_count += 1;
            continue;
        }
        let entry = ledger_counts
            .entry((source_binding.to_string(), table_family.to_string()))
            .or_default();
        entry.0 = entry.0.saturating_add(1);
        if record.canonical_ready {
            entry.1 = entry.1.saturating_add(1);
        }
        if matches!(
            record.status,
            BackfillCoverageStatus::Accepted | BackfillCoverageStatus::AcceptedWithGaps
        ) {
            entry.2 = entry.2.saturating_add(1);
        }
    }

    let configured_keys = source_bindings
        .iter()
        .map(|binding| binding.key.as_str())
        .collect::<Vec<_>>();
    let unconfigured_source_bindings = observed_source_bindings
        .iter()
        .filter(|key| !configured_keys.contains(&key.as_str()))
        .cloned()
        .collect::<Vec<_>>();

    let bindings = source_bindings
        .into_iter()
        .map(|binding| {
            let required_table_family_match = binding.table_families.iter().any(|family| {
                required_table_families
                    .iter()
                    .any(|required| required == family)
            });
            let (ledger_record_count, canonical_ready_record_count, accepted_record_count) =
                binding
                    .table_families
                    .iter()
                    .filter(|family| {
                        required_table_families
                            .iter()
                            .any(|required| required == *family)
                    })
                    .filter_map(|family| ledger_counts.get(&(binding.key.clone(), family.clone())))
                    .copied()
                    .fold((0_u64, 0_u64, 0_u64), |acc, count| {
                        (
                            acc.0.saturating_add(count.0),
                            acc.1.saturating_add(count.1),
                            acc.2.saturating_add(count.2),
                        )
                    });
            BackfillBindingCoverageBinding {
                key: binding.key,
                table_families: binding.table_families,
                required_table_family_match,
                ledger_record_count,
                canonical_ready_record_count,
                accepted_record_count,
            }
        })
        .collect::<Vec<_>>();

    let ledger_records_for_required_bindings = bindings
        .iter()
        .filter(|binding| binding.required_table_family_match)
        .map(|binding| binding.ledger_record_count)
        .sum::<u64>();

    let mut blocking_issues = Vec::new();
    if report_id.trim().is_empty() {
        blocking_issues.push(BackfillBindingCoverageIssue::EmptyReportId);
    }
    if required_table_families.is_empty() {
        blocking_issues.push(BackfillBindingCoverageIssue::EmptyRequiredTableFamilies);
    }
    if configured_required_binding_count == 0 {
        blocking_issues
            .push(BackfillBindingCoverageIssue::NoConfiguredBindingForRequiredTableFamily);
    } else if ledger_records_for_required_bindings == 0 {
        blocking_issues.push(BackfillBindingCoverageIssue::NoLedgerRecordsForRequiredTableFamily);
    } else {
        if bindings.iter().any(|binding| {
            binding.required_table_family_match
                && binding.ledger_record_count > 0
                && binding.canonical_ready_record_count == 0
        }) {
            blocking_issues
                .push(BackfillBindingCoverageIssue::RequiredBindingWithoutCanonicalReadyCoverage);
        }
        if bindings.iter().any(|binding| {
            binding.required_table_family_match
                && binding.ledger_record_count > 0
                && binding.accepted_record_count == 0
        }) {
            blocking_issues
                .push(BackfillBindingCoverageIssue::RequiredBindingWithoutAcceptedCoverage);
        }
    }
    if empty_source_binding_record_count > 0 {
        blocking_issues.push(BackfillBindingCoverageIssue::EmptySourceBindingRecords);
    }
    if missing_table_family_record_count > 0 {
        blocking_issues.push(BackfillBindingCoverageIssue::MissingTableFamilyRecords);
    }
    if !unconfigured_source_bindings.is_empty() {
        blocking_issues.push(BackfillBindingCoverageIssue::UnconfiguredSourceBindingRecords);
    }

    let status = if blocking_issues.is_empty() {
        BackfillBindingCoverageStatus::Ready
    } else {
        BackfillBindingCoverageStatus::Blocked
    };

    BackfillBindingCoverageReport {
        schema_version: BACKFILL_BINDING_COVERAGE_SCHEMA_VERSION.to_string(),
        report_id,
        status,
        required_table_families,
        configured_required_binding_count,
        ledger_records_for_required_bindings,
        empty_source_binding_record_count,
        missing_table_family_record_count,
        unconfigured_source_bindings,
        bindings,
        blocking_issues,
    }
}

fn parse_source_bindings(
    source_bindings_toml: &str,
    path: &str,
) -> Result<Vec<SourceBinding>, BackfillBindingCoverageError> {
    // One typed parse owns the registry schema (source_proof.rs); a malformed
    // entry is a loud parse error here, never a silently dropped binding.
    let registry = SourceBindingRegistry::from_toml_str(source_bindings_toml).map_err(|error| {
        BackfillBindingCoverageError::ParseSourceBindingsToml {
            path: path.to_string(),
            error: error.to_string(),
        }
    })?;
    source_bindings_from_registry(&registry, path)
}

fn source_bindings_from_registry(
    registry: &SourceBindingRegistry,
    path: &str,
) -> Result<Vec<SourceBinding>, BackfillBindingCoverageError> {
    registry
        .all_binding_metadata()
        .into_iter()
        .enumerate()
        .map(|(index, metadata)| {
            let key = metadata.key.trim().to_string();
            if key.is_empty() {
                return Err(BackfillBindingCoverageError::ParseSourceBindingsToml {
                    path: path.to_string(),
                    error: format!("source_binding entry {index} has an empty key"),
                });
            }
            Ok(SourceBinding {
                key,
                table_families: metadata
                    .table_families
                    .iter()
                    .map(|family| family.trim())
                    .filter(|family| !family.is_empty())
                    .map(str::to_string)
                    .collect(),
            })
        })
        .collect()
}
