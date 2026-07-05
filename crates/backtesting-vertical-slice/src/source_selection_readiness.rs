//! Source-selection readiness gate.
//!
//! This report-only gate makes `BACKTESTING_ENGINE-027` machine-checkable:
//! a source can be selected for canonical backfill only after the accepted
//! `SourceProofReport` proves access, license, sample/schema, NT mapping,
//! fidelity, cost, storage, and claim limits for the requested fixture.

use std::{
    error::Error,
    fmt, fs,
    path::{Path, PathBuf},
};

use serde::{Deserialize, Serialize};

use crate::hashing::sha256_hex;
use crate::source_proof::{
    FixtureType, NtMappingStatus, SourceBindingRegistry, SourceProofFidelityClass,
    SourceProofReport, SourceProofStatus, SourceProofUsageScope, SourceSelectionStatus,
    read_source_binding_registry_from_path,
};

pub const SOURCE_SELECTION_READINESS_SCHEMA_VERSION: &str = "source-selection-readiness-report.v1";
pub const SOURCE_SELECTION_READINESS_REPORT_FILE: &str = "source-selection-readiness-report.json";

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SourceSelectionReadinessSpec {
    pub selection_id: String,
    pub source_bindings_path: PathBuf,
    pub source_proof_path: PathBuf,
    pub output_dir: PathBuf,
    pub required_fixture_type: FixtureType,
    pub required_table_family: String,
    pub allowed_fidelity_classes: Vec<SourceProofFidelityClass>,
    #[serde(default)]
    pub allow_lower_fidelity: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SourceSelectionReadinessStatus {
    Ready,
    Blocked,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SourceSelectionReadinessBlocker {
    EmptySelectionId,
    EmptyRequiredTableFamily,
    EmptyAllowedFidelityClasses,
    SourceProofNotAccepted,
    SourceProofAcceptanceFailed,
    SourceProofUsageScopeNotCanonical,
    SourceProofFixtureMismatch,
    SourceProofTableFamilyMismatch,
    SourceProofFidelityNotAllowed,
    LowerFidelityNotAllowed,
    RequiredChecksUnmet,
    NtMappingNotAccepted,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SourceSelectionReadinessReport {
    pub schema_version: String,
    pub selection_id: String,
    pub status: SourceSelectionReadinessStatus,
    pub source_proof_id: String,
    pub source_proof_version: u32,
    pub source_proof_hash: String,
    pub source_binding: String,
    pub venue: String,
    pub fixture_type: FixtureType,
    pub table_family: String,
    pub fidelity_class: SourceProofFidelityClass,
    pub source_selection_status: SourceSelectionStatus,
    pub usage_scope: SourceProofUsageScope,
    pub required_fixture_type: FixtureType,
    pub required_table_family: String,
    pub allowed_fidelity_classes: Vec<SourceProofFidelityClass>,
    pub allow_lower_fidelity: bool,
    pub source_proof_accepted: bool,
    pub canonical_usage_scope_proven: bool,
    pub source_access_proven: bool,
    pub license_proven: bool,
    pub sample_schema_proven: bool,
    pub time_semantics_proven: bool,
    pub instrument_universe_proven: bool,
    pub coverage_proven: bool,
    pub retention_freshness_proven: bool,
    pub granularity_proven: bool,
    pub completeness_proven: bool,
    pub nt_mapping_proven: bool,
    pub cost_proven: bool,
    pub storage_proven: bool,
    pub claim_limits_recorded: bool,
    pub source_proof_acceptance_error: Option<String>,
    pub unmet_required_checks: Vec<String>,
    pub blockers: Vec<SourceSelectionReadinessBlocker>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SourceSelectionReadinessArtifact {
    pub path: PathBuf,
    pub content_hash: String,
    pub bytes: u64,
}

pub struct SourceSelectionReadinessInput<'a> {
    pub selection_id: &'a str,
    pub source_proof_hash: &'a str,
    pub source_proof: &'a SourceProofReport,
    pub source_bindings_registry: &'a SourceBindingRegistry,
    pub required_fixture_type: FixtureType,
    pub required_table_family: &'a str,
    pub allowed_fidelity_classes: Vec<SourceProofFidelityClass>,
    pub allow_lower_fidelity: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SourceSelectionReadinessError {
    ReadSpec { path: String, error: String },
    ParseSpecToml { path: String, error: String },
    ReadSourceBindings { path: String, error: String },
    ReadSourceProof { path: String, error: String },
    ParseSourceProofJson { path: String, error: String },
    CreateDir { path: String, error: String },
    ReadExisting { path: String, error: String },
    Write { path: String, error: String },
    ExistingArtifactMismatch { path: String },
    Serialize(String),
}

impl fmt::Display for SourceSelectionReadinessError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::ReadSpec { path, error } => {
                write!(f, "read source-selection readiness spec {path}: {error}")
            }
            Self::ParseSpecToml { path, error } => write!(
                f,
                "parse source-selection readiness spec TOML {path}: {error}"
            ),
            Self::ReadSourceBindings { path, error } => {
                write!(f, "read source-bindings registry {path}: {error}")
            }
            Self::ReadSourceProof { path, error } => {
                write!(f, "read source proof {path}: {error}")
            }
            Self::ParseSourceProofJson { path, error } => {
                write!(f, "parse source proof JSON {path}: {error}")
            }
            Self::CreateDir { path, error } => write!(
                f,
                "create source-selection readiness artifact directory {path}: {error}"
            ),
            Self::ReadExisting { path, error } => write!(
                f,
                "read existing source-selection readiness artifact {path}: {error}"
            ),
            Self::Write { path, error } => {
                write!(
                    f,
                    "write source-selection readiness artifact {path}: {error}"
                )
            }
            Self::ExistingArtifactMismatch { path } => write!(
                f,
                "dirty source-selection readiness artifact {path}: existing file content differs"
            ),
            Self::Serialize(error) => {
                write!(f, "serialize source-selection readiness artifact: {error}")
            }
        }
    }
}

impl Error for SourceSelectionReadinessError {}

#[must_use]
pub fn evaluate_source_selection_readiness(
    input: SourceSelectionReadinessInput<'_>,
) -> SourceSelectionReadinessReport {
    let source_proof = input.source_proof;
    let selection_id = input.selection_id.to_string();
    let required_table_family = input.required_table_family.to_string();
    let allowed_fidelity_classes = input.allowed_fidelity_classes;
    let unmet_required_checks = source_proof
        .required_checks
        .unmet()
        .into_iter()
        .map(str::to_string)
        .collect::<Vec<_>>();
    let acceptance_error = source_proof
        .evaluate_acceptance_with_registry(input.source_bindings_registry)
        .err()
        .map(|error| error.to_string());

    let mut blockers = Vec::new();
    if selection_id.trim().is_empty() {
        blockers.push(SourceSelectionReadinessBlocker::EmptySelectionId);
    }
    if required_table_family.trim().is_empty() {
        blockers.push(SourceSelectionReadinessBlocker::EmptyRequiredTableFamily);
    }
    if allowed_fidelity_classes.is_empty() {
        blockers.push(SourceSelectionReadinessBlocker::EmptyAllowedFidelityClasses);
    }
    if source_proof.status != SourceProofStatus::Accepted {
        blockers.push(SourceSelectionReadinessBlocker::SourceProofNotAccepted);
    }
    if acceptance_error.is_some() {
        blockers.push(SourceSelectionReadinessBlocker::SourceProofAcceptanceFailed);
    }
    if source_proof.usage_scope != SourceProofUsageScope::CanonicalBackfillInput {
        blockers.push(SourceSelectionReadinessBlocker::SourceProofUsageScopeNotCanonical);
    }
    if source_proof.fixture_type != input.required_fixture_type {
        blockers.push(SourceSelectionReadinessBlocker::SourceProofFixtureMismatch);
    }
    if source_proof.table_family.trim() != required_table_family.trim() {
        blockers.push(SourceSelectionReadinessBlocker::SourceProofTableFamilyMismatch);
    }
    if !allowed_fidelity_classes
        .iter()
        .any(|fidelity| fidelity == &source_proof.fidelity_class)
    {
        blockers.push(SourceSelectionReadinessBlocker::SourceProofFidelityNotAllowed);
    }
    if !input.allow_lower_fidelity
        && source_proof.source_selection_status == SourceSelectionStatus::AcceptedLowerFidelity
    {
        blockers.push(SourceSelectionReadinessBlocker::LowerFidelityNotAllowed);
    }
    if !unmet_required_checks.is_empty() {
        blockers.push(SourceSelectionReadinessBlocker::RequiredChecksUnmet);
    }
    if source_proof.nt_mapping_status != NtMappingStatus::Accepted
        || unmet_required_checks
            .iter()
            .any(|check| check == "nt_mapping")
    {
        blockers.push(SourceSelectionReadinessBlocker::NtMappingNotAccepted);
    }

    let status = if blockers.is_empty() {
        SourceSelectionReadinessStatus::Ready
    } else {
        SourceSelectionReadinessStatus::Blocked
    };

    SourceSelectionReadinessReport {
        schema_version: SOURCE_SELECTION_READINESS_SCHEMA_VERSION.to_string(),
        selection_id,
        status,
        source_proof_id: source_proof.source_proof_id.clone(),
        source_proof_version: source_proof.source_proof_version,
        source_proof_hash: input.source_proof_hash.to_string(),
        source_binding: source_proof.source_binding.clone(),
        venue: source_proof.venue.clone(),
        fixture_type: source_proof.fixture_type,
        table_family: source_proof.table_family.clone(),
        fidelity_class: source_proof.fidelity_class,
        source_selection_status: source_proof.source_selection_status,
        usage_scope: source_proof.usage_scope,
        required_fixture_type: input.required_fixture_type,
        required_table_family,
        allowed_fidelity_classes,
        allow_lower_fidelity: input.allow_lower_fidelity,
        source_proof_accepted: source_proof.status == SourceProofStatus::Accepted,
        canonical_usage_scope_proven: source_proof.usage_scope
            == SourceProofUsageScope::CanonicalBackfillInput,
        source_access_proven: !unmet_required_checks
            .iter()
            .any(|check| check == "source_access"),
        license_proven: !unmet_required_checks.iter().any(|check| check == "license"),
        sample_schema_proven: !unmet_required_checks.iter().any(|check| check == "schema")
            && !source_proof.raw_sample_uri.trim().is_empty()
            && !source_proof.raw_sample_hash.trim().is_empty()
            && !source_proof.schema_sample_uri.trim().is_empty()
            && !source_proof.schema_sample_hash.trim().is_empty(),
        time_semantics_proven: !unmet_required_checks
            .iter()
            .any(|check| check == "time_semantics"),
        instrument_universe_proven: !unmet_required_checks
            .iter()
            .any(|check| check == "instrument_universe"),
        coverage_proven: !unmet_required_checks
            .iter()
            .any(|check| check == "coverage"),
        retention_freshness_proven: !unmet_required_checks
            .iter()
            .any(|check| check == "retention_freshness"),
        granularity_proven: !unmet_required_checks
            .iter()
            .any(|check| check == "granularity"),
        completeness_proven: !unmet_required_checks
            .iter()
            .any(|check| check == "completeness"),
        nt_mapping_proven: source_proof.nt_mapping_status == NtMappingStatus::Accepted
            && !unmet_required_checks
                .iter()
                .any(|check| check == "nt_mapping"),
        cost_proven: !unmet_required_checks.iter().any(|check| check == "cost")
            && !source_proof.cost_ref.trim().is_empty(),
        storage_proven: !unmet_required_checks.iter().any(|check| check == "storage"),
        claim_limits_recorded: !source_proof.claim_limits.is_empty(),
        source_proof_acceptance_error: acceptance_error,
        unmet_required_checks,
        blockers,
    }
}

pub fn write_source_selection_readiness_report_from_spec_file(
    spec_path: &Path,
) -> Result<SourceSelectionReadinessArtifact, SourceSelectionReadinessError> {
    let spec_path_display = spec_path.display().to_string();
    let spec_text =
        fs::read_to_string(spec_path).map_err(|error| SourceSelectionReadinessError::ReadSpec {
            path: spec_path_display.clone(),
            error: error.to_string(),
        })?;
    let spec: SourceSelectionReadinessSpec = toml::from_str(&spec_text).map_err(|error| {
        SourceSelectionReadinessError::ParseSpecToml {
            path: spec_path_display,
            error: error.to_string(),
        }
    })?;

    let source_bindings_path = spec.source_bindings_path.display().to_string();
    let source_bindings_registry =
        read_source_binding_registry_from_path(&spec.source_bindings_path).map_err(|error| {
            SourceSelectionReadinessError::ReadSourceBindings {
                path: source_bindings_path,
                error: error.to_string(),
            }
        })?;

    let source_proof_path = spec.source_proof_path.display().to_string();
    let source_proof_bytes = fs::read(&spec.source_proof_path).map_err(|error| {
        SourceSelectionReadinessError::ReadSourceProof {
            path: source_proof_path.clone(),
            error: error.to_string(),
        }
    })?;
    let source_proof: SourceProofReport =
        serde_json::from_slice(&source_proof_bytes).map_err(|error| {
            SourceSelectionReadinessError::ParseSourceProofJson {
                path: source_proof_path,
                error: error.to_string(),
            }
        })?;
    let source_proof_hash = sha256_hex(&source_proof_bytes);

    let report = evaluate_source_selection_readiness(SourceSelectionReadinessInput {
        selection_id: &spec.selection_id,
        source_proof_hash: &source_proof_hash,
        source_proof: &source_proof,
        source_bindings_registry: &source_bindings_registry,
        required_fixture_type: spec.required_fixture_type,
        required_table_family: &spec.required_table_family,
        allowed_fidelity_classes: spec.allowed_fidelity_classes,
        allow_lower_fidelity: spec.allow_lower_fidelity,
    });
    write_source_selection_readiness_report(&spec.output_dir, &report)
}

pub fn write_source_selection_readiness_report(
    output_dir: &Path,
    report: &SourceSelectionReadinessReport,
) -> Result<SourceSelectionReadinessArtifact, SourceSelectionReadinessError> {
    fs::create_dir_all(output_dir).map_err(|error| SourceSelectionReadinessError::CreateDir {
        path: output_dir.display().to_string(),
        error: error.to_string(),
    })?;
    let path = output_dir.join(SOURCE_SELECTION_READINESS_REPORT_FILE);
    let written = crate::reference_artifact::write_reference_artifact_with_len_mapped(
        &path,
        SOURCE_SELECTION_READINESS_REPORT_FILE,
        report,
        crate::reference_artifact::ReferenceArtifactRewrite::FailOnDirty,
        crate::reference_artifact::ReferenceArtifactErrorMappers {
            serialize_error: SourceSelectionReadinessError::Serialize,
            read_existing_error: |path, error| SourceSelectionReadinessError::ReadExisting {
                path,
                error,
            },
            mismatch_error: |path| SourceSelectionReadinessError::ExistingArtifactMismatch {
                path,
            },
            write_error: |path, error| SourceSelectionReadinessError::Write { path, error },
        },
    )?;

    Ok(SourceSelectionReadinessArtifact {
        path,
        content_hash: written.pin.sha256,
        bytes: written.bytes,
    })
}
