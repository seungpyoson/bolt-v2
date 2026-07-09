//! Source-proof candidate shortlisting from current reports.
//!
//! This gate consumes typed [`SourceProofReport`] records only. It is a
//! report-only selection step for follow-up source-proof work; it does not
//! accept proofs, download payloads, convert data, or make canonical backtest
//! input eligible.

use std::{
    error::Error,
    fmt, fs,
    path::{Path, PathBuf},
};

use serde::{Deserialize, Serialize};

use crate::path_resolution::{resolve_existing_path, resolve_output_dir};
use crate::source_proof::{
    FixtureType, SourceCandidateClass, SourceProofFidelityClass, SourceProofReport,
    SourceProofStatus, SourceSelectionStatus,
};

pub const SOURCE_PROOF_SHORTLIST_SCHEMA_VERSION: &str = "source-proof-shortlist-report.v1";
pub const SOURCE_PROOF_SHORTLIST_REPORT_FILE: &str = "source-proof-shortlist-report.json";

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SourceProofShortlistSelection {
    pub allowed_fixture_types: Vec<FixtureType>,
    pub allowed_table_families: Vec<String>,
    pub allowed_candidate_classes: Vec<SourceCandidateClass>,
    pub max_candidates: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SourceProofShortlistInput {
    pub proof_uri: String,
    pub proof: SourceProofReport,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SourceProofShortlistProofFile {
    pub proof_uri: String,
    pub path: PathBuf,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SourceProofShortlistSpec {
    pub shortlist_id: String,
    pub output_dir: PathBuf,
    #[serde(rename = "source_proof", default)]
    pub source_proofs: Vec<SourceProofShortlistProofFile>,
    pub selection: SourceProofShortlistSelection,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SourceProofShortlistStatus {
    CandidatesFound,
    Blocked,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SourceProofShortlistReason {
    EmptyShortlistId,
    EmptySourceProofReports,
    EmptyAllowedFixtureTypes,
    EmptyAllowedTableFamilies,
    EmptyAllowedCandidateClasses,
    InvalidCandidateBudget,
    NoMatchingSourceProofReports,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SourceProofShortlistCandidate {
    pub proof_uri: String,
    pub source_proof_id: String,
    pub source_proof_version: u32,
    pub source_binding: String,
    pub venue: String,
    pub fixture_type: FixtureType,
    pub table_family: String,
    pub status: SourceProofStatus,
    pub source_candidate_class: SourceCandidateClass,
    pub source_selection_status: SourceSelectionStatus,
    pub fidelity_class: SourceProofFidelityClass,
    pub remaining_required_checks: Vec<String>,
    pub claim_limit_count: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SourceProofShortlistReport {
    pub schema_version: String,
    pub shortlist_id: String,
    pub status: SourceProofShortlistStatus,
    pub selection: SourceProofShortlistSelection,
    pub total_reports: u64,
    pub eligible_candidate_count: u64,
    pub candidates: Vec<SourceProofShortlistCandidate>,
    pub blocking_reasons: Vec<SourceProofShortlistReason>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SourceProofShortlistArtifact {
    pub path: PathBuf,
    pub content_hash: String,
    pub bytes: u64,
    pub candidate_count: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SourceProofShortlistError {
    ReadSpec {
        path: String,
        error: String,
    },
    ParseSpecToml {
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
    CreateDir {
        path: String,
        error: String,
    },
    ReadExisting {
        path: String,
        error: String,
    },
    Write {
        path: String,
        error: String,
    },
    ExistingArtifactMismatch {
        path: String,
    },
    Serialize(String),
}

impl fmt::Display for SourceProofShortlistError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::ReadSpec { path, error } => {
                write!(f, "read source-proof shortlist spec {path}: {error}")
            }
            Self::ParseSpecToml { path, error } => {
                write!(f, "parse source-proof shortlist spec TOML {path}: {error}")
            }
            Self::ReadSourceProof {
                proof_uri,
                path,
                error,
            } => write!(
                f,
                "read source-proof shortlist proof {proof_uri} from {path}: {error}"
            ),
            Self::ParseSourceProofJson {
                proof_uri,
                path,
                error,
            } => write!(
                f,
                "parse source-proof shortlist proof {proof_uri} from {path}: {error}"
            ),
            Self::CreateDir { path, error } => write!(
                f,
                "create source-proof shortlist artifact directory {path}: {error}"
            ),
            Self::ReadExisting { path, error } => write!(
                f,
                "read existing source-proof shortlist artifact {path}: {error}"
            ),
            Self::Write { path, error } => {
                write!(f, "write source-proof shortlist artifact {path}: {error}")
            }
            Self::ExistingArtifactMismatch { path } => write!(
                f,
                "dirty source-proof shortlist artifact {path}: existing file content differs"
            ),
            Self::Serialize(error) => {
                write!(f, "serialize source-proof shortlist artifact: {error}")
            }
        }
    }
}

impl Error for SourceProofShortlistError {}

#[must_use]
pub fn evaluate_source_proof_shortlist(
    shortlist_id: impl Into<String>,
    source_proofs: Vec<SourceProofShortlistInput>,
    selection: &SourceProofShortlistSelection,
) -> SourceProofShortlistReport {
    let shortlist_id = shortlist_id.into();
    let mut blocking_reasons = Vec::new();
    if shortlist_id.trim().is_empty() {
        blocking_reasons.push(SourceProofShortlistReason::EmptyShortlistId);
    }
    if source_proofs.is_empty() {
        blocking_reasons.push(SourceProofShortlistReason::EmptySourceProofReports);
    }
    if selection.allowed_fixture_types.is_empty() {
        blocking_reasons.push(SourceProofShortlistReason::EmptyAllowedFixtureTypes);
    }
    if selection.allowed_table_families.is_empty() {
        blocking_reasons.push(SourceProofShortlistReason::EmptyAllowedTableFamilies);
    }
    if selection.allowed_candidate_classes.is_empty() {
        blocking_reasons.push(SourceProofShortlistReason::EmptyAllowedCandidateClasses);
    }
    if selection.max_candidates == 0 {
        blocking_reasons.push(SourceProofShortlistReason::InvalidCandidateBudget);
    }

    let mut candidates = source_proofs
        .iter()
        .filter(|input| is_candidate(input, selection))
        .map(candidate_from_input)
        .collect::<Vec<_>>();
    candidates.sort_by(|left, right| {
        left.remaining_required_checks
            .len()
            .cmp(&right.remaining_required_checks.len())
            .then(left.proof_uri.cmp(&right.proof_uri))
    });

    let eligible_candidate_count = candidates.len() as u64;
    if eligible_candidate_count == 0 && blocking_reasons.is_empty() {
        blocking_reasons.push(SourceProofShortlistReason::NoMatchingSourceProofReports);
    }
    if !blocking_reasons.is_empty() {
        candidates.clear();
    } else {
        candidates.truncate(selection.max_candidates as usize);
    }

    let status = if candidates.is_empty() {
        SourceProofShortlistStatus::Blocked
    } else {
        SourceProofShortlistStatus::CandidatesFound
    };

    SourceProofShortlistReport {
        schema_version: SOURCE_PROOF_SHORTLIST_SCHEMA_VERSION.to_string(),
        shortlist_id,
        status,
        selection: selection.clone(),
        total_reports: source_proofs.len() as u64,
        eligible_candidate_count,
        candidates,
        blocking_reasons,
    }
}

pub fn write_source_proof_shortlist_report_from_spec_file(
    spec_path: &Path,
) -> Result<SourceProofShortlistArtifact, SourceProofShortlistError> {
    let path_display = spec_path.display().to_string();
    let spec_text =
        fs::read_to_string(spec_path).map_err(|error| SourceProofShortlistError::ReadSpec {
            path: path_display.clone(),
            error: error.to_string(),
        })?;
    let spec: SourceProofShortlistSpec =
        toml::from_str(&spec_text).map_err(|error| SourceProofShortlistError::ParseSpecToml {
            path: path_display,
            error: error.to_string(),
        })?;
    let base_dir = spec_path.parent().unwrap_or_else(|| Path::new("."));
    let inputs = spec
        .source_proofs
        .into_iter()
        .map(|source_proof| {
            let SourceProofShortlistProofFile { proof_uri, path } = source_proof;
            let path_display = path.display().to_string();
            let resolved_path = resolve_existing_path(base_dir, &path);
            let bytes = fs::read(&resolved_path).map_err(|error| {
                SourceProofShortlistError::ReadSourceProof {
                    proof_uri: proof_uri.clone(),
                    path: path_display.clone(),
                    error: error.to_string(),
                }
            })?;
            let proof: SourceProofReport = serde_json::from_slice(&bytes).map_err(|error| {
                SourceProofShortlistError::ParseSourceProofJson {
                    proof_uri: proof_uri.clone(),
                    path: path_display,
                    error: error.to_string(),
                }
            })?;
            Ok(SourceProofShortlistInput { proof_uri, proof })
        })
        .collect::<Result<Vec<_>, _>>()?;
    let report = evaluate_source_proof_shortlist(spec.shortlist_id, inputs, &spec.selection);
    let output_dir = resolve_output_dir(base_dir, &spec.output_dir);
    write_source_proof_shortlist_report(&output_dir, &report)
}

pub fn write_source_proof_shortlist_report(
    output_dir: &Path,
    report: &SourceProofShortlistReport,
) -> Result<SourceProofShortlistArtifact, SourceProofShortlistError> {
    fs::create_dir_all(output_dir).map_err(|error| SourceProofShortlistError::CreateDir {
        path: output_dir.display().to_string(),
        error: error.to_string(),
    })?;
    let path = output_dir.join(SOURCE_PROOF_SHORTLIST_REPORT_FILE);
    let written = crate::reference_artifact::write_reference_artifact_with_len_mapped(
        &path,
        SOURCE_PROOF_SHORTLIST_REPORT_FILE,
        report,
        crate::reference_artifact::ReferenceArtifactRewrite::FailOnDirty,
        crate::reference_artifact::ReferenceArtifactErrorMappers {
            serialize_error: SourceProofShortlistError::Serialize,
            read_existing_error: |path, error| SourceProofShortlistError::ReadExisting {
                path,
                error,
            },
            mismatch_error: |path| SourceProofShortlistError::ExistingArtifactMismatch { path },
            write_error: |path, error| SourceProofShortlistError::Write { path, error },
        },
    )?;
    Ok(SourceProofShortlistArtifact {
        path,
        content_hash: written.pin.sha256,
        bytes: written.bytes,
        candidate_count: report.candidates.len() as u64,
    })
}

fn is_candidate(
    input: &SourceProofShortlistInput,
    selection: &SourceProofShortlistSelection,
) -> bool {
    let proof = &input.proof;
    !input.proof_uri.trim().is_empty()
        && selection
            .allowed_fixture_types
            .iter()
            .any(|allowed| allowed == &proof.fixture_type)
        && selection
            .allowed_table_families
            .iter()
            .any(|allowed| allowed == &proof.table_family)
        && selection
            .allowed_candidate_classes
            .iter()
            .any(|allowed| allowed == &proof.source_candidate_class)
        && proof.status != SourceProofStatus::Rejected
        && proof.source_selection_status != SourceSelectionStatus::Rejected
}

fn candidate_from_input(input: &SourceProofShortlistInput) -> SourceProofShortlistCandidate {
    let proof = &input.proof;
    SourceProofShortlistCandidate {
        proof_uri: input.proof_uri.clone(),
        source_proof_id: proof.source_proof_id.clone(),
        source_proof_version: proof.source_proof_version,
        source_binding: proof.source_binding.clone(),
        venue: proof.venue.clone(),
        fixture_type: proof.fixture_type,
        table_family: proof.table_family.clone(),
        status: proof.status,
        source_candidate_class: proof.source_candidate_class,
        source_selection_status: proof.source_selection_status,
        fidelity_class: proof.fidelity_class,
        remaining_required_checks: proof
            .required_checks
            .unmet()
            .into_iter()
            .map(str::to_string)
            .collect(),
        claim_limit_count: proof.claim_limits.len() as u64,
    }
}
