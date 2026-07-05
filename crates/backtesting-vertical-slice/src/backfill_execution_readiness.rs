//! Execution-plan readiness gate for accepted backfill tranches.
//!
//! This report-only gate consumes the current accepted-tranche and
//! execution-plan artifacts. It is the cheap final check before an operator is
//! allowed to fetch a payload object, convert it, project it into the NT
//! catalog, or run NT backtesting.

use std::{
    error::Error,
    fmt, fs,
    path::{Path, PathBuf},
};

use serde::{Deserialize, Serialize};

use crate::hashing::sha256_hex;
use crate::{
    artifact_index::ArtifactKind,
    artifact_index_commit_proof::ArtifactIndexCommitProofReport,
    backfill_accepted_tranche::{BackfillAcceptedTrancheManifest, BackfillAcceptedTrancheStatus},
    backfill_execution_plan::{BackfillExecutionPlan, BackfillExecutionPlanStatus},
    run_manifest::ArtifactSubpath,
    source_catalog_mapping_readiness::{
        SourceCatalogMappingReadinessReport, SourceCatalogMappingReadinessStatus,
    },
    source_proof::SourceProofUsageScope,
    source_selection_readiness::{SourceSelectionReadinessReport, SourceSelectionReadinessStatus},
};

pub const BACKFILL_EXECUTION_READINESS_SCHEMA_VERSION: &str =
    "backfill-execution-readiness-report.v2";
pub const BACKFILL_EXECUTION_READINESS_REPORT_FILE: &str =
    "backfill-execution-readiness-report.json";

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BackfillExecutionReadinessSpec {
    pub readiness_id: String,
    pub accepted_tranche_manifest_path: PathBuf,
    pub execution_plan_path: PathBuf,
    pub output_dir: PathBuf,
    pub required_table_family: String,
    pub required_nt_data_type: String,
    pub required_source_usage_scope: SourceProofUsageScope,
    pub supported_data_paths: Vec<BackfillExecutionReadinessSupportedDataPath>,
    #[serde(default)]
    pub artifact_index_commit_required: bool,
    #[serde(default)]
    pub required_artifact_index_kind: Option<ArtifactKind>,
    #[serde(default)]
    pub artifact_index_commit_proof_report_path: Option<PathBuf>,
    #[serde(default)]
    pub source_selection_readiness_required: bool,
    #[serde(default)]
    pub source_selection_readiness_report_path: Option<PathBuf>,
    #[serde(default)]
    pub source_catalog_mapping_readiness_required: bool,
    #[serde(default)]
    pub source_catalog_mapping_readiness_report_path: Option<PathBuf>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BackfillExecutionReadinessSupportedDataPath {
    pub table_family: String,
    pub nt_data_type: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BackfillExecutionReadinessStatus {
    Ready,
    Blocked,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BackfillExecutionReadinessBlocker {
    EmptyReadinessId,
    EmptyRequiredTableFamily,
    EmptyRequiredNtDataType,
    EmptySupportedDataPaths,
    UnsupportedRequiredNtDataType,
    UnsupportedRequiredTableFamilyDataType,
    AcceptedTrancheNotAccepted,
    AcceptedTrancheHasBlockingIssues,
    AcceptedTrancheObjectCountNotOne,
    AcceptedTrancheBytesMismatch,
    ExecutionPlanNotReady,
    ExecutionPlanHasBlockingIssues,
    ExecutionPlanAcceptedTrancheMismatch,
    ExecutionPlanAcceptedTrancheHashMismatch,
    ExecutionPlanSourceProofMismatch,
    ExecutionPlanSourceBindingMismatch,
    ExecutionPlanTableFamilyMismatch,
    ExecutionPlanObjectCountMismatch,
    ExecutionPlanAcceptedBytesMismatch,
    ExecutionPlanObjectMismatch,
    RequiredTableFamilyMismatch,
    ArtifactIndexCommitProofRequiredButMissing,
    ArtifactIndexCommitProofKindRequiredButMissing,
    ArtifactIndexCommitProofKindMismatch,
    ArtifactIndexCommitProofArtifactRootMismatch,
    ArtifactIndexCommitMechanicsUnproven,
    ArtifactIndexProducerIamScopeUnproven,
    SourceSelectionReadinessRequiredButMissing,
    SourceSelectionReadinessNotReady,
    SourceSelectionReadinessHasBlockers,
    SourceSelectionReadinessNotProven,
    SourceSelectionReadinessUsageScopeMismatch,
    SourceSelectionReadinessSourceProofMismatch,
    SourceSelectionReadinessSourceBindingMismatch,
    SourceSelectionReadinessTableFamilyMismatch,
    SourceCatalogMappingReadinessRequiredButMissing,
    SourceCatalogMappingReadinessNotReady,
    SourceCatalogMappingReadinessHasBlockers,
    SourceCatalogMappingReadinessNotProven,
    SourceCatalogMappingReadinessSourceProofMismatch,
    SourceCatalogMappingReadinessSourceBindingMismatch,
    SourceCatalogMappingReadinessTableFamilyMismatch,
    SourceCatalogMappingReadinessNtDataTypeMismatch,
    SourceCatalogMappingReadinessUsageScopeMismatch,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BackfillExecutionReadinessReport {
    pub schema_version: String,
    pub readiness_id: String,
    pub status: BackfillExecutionReadinessStatus,
    pub required_table_family: String,
    pub required_nt_data_type: String,
    pub required_source_usage_scope: SourceProofUsageScope,
    pub supported_data_paths: Vec<BackfillExecutionReadinessSupportedDataPath>,
    pub artifact_index_commit_required: bool,
    pub required_artifact_index_kind: Option<ArtifactKind>,
    pub artifact_index_commit_proof_id: Option<String>,
    pub artifact_index_commit_proof_hash: Option<String>,
    pub artifact_index_direct_s3_commit_proven: Option<bool>,
    pub artifact_index_producer_iam_scope_proven: Option<bool>,
    pub source_selection_readiness_required: bool,
    pub source_selection_readiness_id: Option<String>,
    pub source_selection_readiness_hash: Option<String>,
    pub source_selection_readiness_status: Option<SourceSelectionReadinessStatus>,
    pub source_catalog_mapping_readiness_required: bool,
    pub source_catalog_mapping_readiness_id: Option<String>,
    pub source_catalog_mapping_readiness_hash: Option<String>,
    pub source_catalog_mapping_readiness_status: Option<SourceCatalogMappingReadinessStatus>,
    pub accepted_tranche_id: String,
    pub accepted_tranche_manifest_hash: String,
    pub execution_plan_id: String,
    pub execution_plan_hash: String,
    pub operator_run_id: String,
    pub source_proof_id: String,
    pub source_proof_version: u32,
    pub source_binding: String,
    pub table_family: String,
    pub object_count: u64,
    pub accepted_bytes: u64,
    pub blockers: Vec<BackfillExecutionReadinessBlocker>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BackfillExecutionReadinessArtifact {
    pub path: PathBuf,
    pub content_hash: String,
    pub bytes: u64,
}

pub struct BackfillExecutionReadinessInput<'a> {
    pub readiness_id: &'a str,
    pub accepted_tranche_manifest_hash: &'a str,
    pub tranche: &'a BackfillAcceptedTrancheManifest,
    pub execution_plan_hash: &'a str,
    pub plan: &'a BackfillExecutionPlan,
    pub required_table_family: &'a str,
    pub required_nt_data_type: &'a str,
    pub required_source_usage_scope: SourceProofUsageScope,
    pub supported_data_paths: Vec<BackfillExecutionReadinessSupportedDataPath>,
    pub artifact_index_commit_required: bool,
    pub required_artifact_index_kind: Option<ArtifactKind>,
    pub artifact_index_commit_proof_report_hash: Option<&'a str>,
    pub artifact_index_commit_proof_report: Option<&'a ArtifactIndexCommitProofReport>,
    pub source_selection_readiness_required: bool,
    pub source_selection_readiness_report_hash: Option<&'a str>,
    pub source_selection_readiness_report: Option<&'a SourceSelectionReadinessReport>,
    pub source_catalog_mapping_readiness_required: bool,
    pub source_catalog_mapping_readiness_report_hash: Option<&'a str>,
    pub source_catalog_mapping_readiness_report: Option<&'a SourceCatalogMappingReadinessReport>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BackfillExecutionReadinessError {
    ReadSpec { path: String, error: String },
    ParseSpecToml { path: String, error: String },
    ReadAcceptedTrancheManifest { path: String, error: String },
    ParseAcceptedTrancheManifestJson { path: String, error: String },
    ReadExecutionPlan { path: String, error: String },
    ParseExecutionPlanJson { path: String, error: String },
    ReadArtifactIndexCommitProofReport { path: String, error: String },
    ParseArtifactIndexCommitProofReportJson { path: String, error: String },
    ReadSourceSelectionReadinessReport { path: String, error: String },
    ParseSourceSelectionReadinessReportJson { path: String, error: String },
    ReadSourceCatalogMappingReadinessReport { path: String, error: String },
    ParseSourceCatalogMappingReadinessReportJson { path: String, error: String },
    CreateDir { path: String, error: String },
    ReadExisting { path: String, error: String },
    Write { path: String, error: String },
    ExistingArtifactMismatch { path: String },
    Serialize(String),
}

impl fmt::Display for BackfillExecutionReadinessError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::ReadSpec { path, error } => {
                write!(f, "read backfill execution-readiness spec {path}: {error}")
            }
            Self::ParseSpecToml { path, error } => {
                write!(
                    f,
                    "parse backfill execution-readiness spec TOML {path}: {error}"
                )
            }
            Self::ReadAcceptedTrancheManifest { path, error } => {
                write!(f, "read backfill accepted-tranche manifest {path}: {error}")
            }
            Self::ParseAcceptedTrancheManifestJson { path, error } => write!(
                f,
                "parse backfill accepted-tranche manifest JSON {path}: {error}"
            ),
            Self::ReadExecutionPlan { path, error } => {
                write!(f, "read backfill execution-plan {path}: {error}")
            }
            Self::ParseExecutionPlanJson { path, error } => {
                write!(f, "parse backfill execution-plan JSON {path}: {error}")
            }
            Self::ReadArtifactIndexCommitProofReport { path, error } => {
                write!(f, "read Artifact Index commit proof report {path}: {error}")
            }
            Self::ParseArtifactIndexCommitProofReportJson { path, error } => write!(
                f,
                "parse Artifact Index commit proof report JSON {path}: {error}"
            ),
            Self::ReadSourceSelectionReadinessReport { path, error } => {
                write!(f, "read source-selection readiness report {path}: {error}")
            }
            Self::ParseSourceSelectionReadinessReportJson { path, error } => write!(
                f,
                "parse source-selection readiness report JSON {path}: {error}"
            ),
            Self::ReadSourceCatalogMappingReadinessReport { path, error } => write!(
                f,
                "read source catalog-mapping readiness report {path}: {error}"
            ),
            Self::ParseSourceCatalogMappingReadinessReportJson { path, error } => write!(
                f,
                "parse source catalog-mapping readiness report JSON {path}: {error}"
            ),
            Self::CreateDir { path, error } => write!(
                f,
                "create backfill execution-readiness artifact directory {path}: {error}"
            ),
            Self::ReadExisting { path, error } => write!(
                f,
                "read existing backfill execution-readiness artifact {path}: {error}"
            ),
            Self::Write { path, error } => {
                write!(
                    f,
                    "write backfill execution-readiness artifact {path}: {error}"
                )
            }
            Self::ExistingArtifactMismatch { path } => write!(
                f,
                "dirty backfill execution-readiness artifact {path}: existing file content differs"
            ),
            Self::Serialize(error) => {
                write!(
                    f,
                    "serialize backfill execution-readiness artifact: {error}"
                )
            }
        }
    }
}

impl Error for BackfillExecutionReadinessError {}

#[must_use]
pub fn evaluate_backfill_execution_readiness(
    input: BackfillExecutionReadinessInput<'_>,
) -> BackfillExecutionReadinessReport {
    let readiness_id = input.readiness_id.to_string();
    let accepted_tranche_manifest_hash = input.accepted_tranche_manifest_hash.to_string();
    let execution_plan_hash = input.execution_plan_hash.to_string();
    let required_table_family = input.required_table_family.to_string();
    let required_nt_data_type = input.required_nt_data_type.to_string();
    let required_source_usage_scope = input.required_source_usage_scope;
    let required_table_family_trimmed = required_table_family.trim();
    let required_nt_data_type_trimmed = required_nt_data_type.trim();
    let tranche = input.tranche;
    let plan = input.plan;
    let supported_data_paths = input.supported_data_paths;
    let artifact_index_commit_required = input.artifact_index_commit_required;
    let required_artifact_index_kind = input.required_artifact_index_kind;
    let artifact_index_commit_proof_report = input.artifact_index_commit_proof_report;
    let artifact_index_commit_proof_hash = input
        .artifact_index_commit_proof_report_hash
        .map(str::to_string);
    let artifact_index_commit_proof_id =
        artifact_index_commit_proof_report.map(|report| report.proof_id.clone());
    let artifact_index_direct_s3_commit_proven =
        artifact_index_commit_proof_report.map(|report| report.direct_s3_commit_proven);
    let artifact_index_producer_iam_scope_proven =
        artifact_index_commit_proof_report.map(|report| report.producer_iam_scope_proven);
    let source_selection_readiness_required = input.source_selection_readiness_required;
    let source_selection_readiness_report = input.source_selection_readiness_report;
    let source_selection_readiness_hash = input
        .source_selection_readiness_report_hash
        .map(str::to_string);
    let source_selection_readiness_id =
        source_selection_readiness_report.map(|report| report.selection_id.clone());
    let source_selection_readiness_status =
        source_selection_readiness_report.map(|report| report.status);
    let source_catalog_mapping_readiness_required = input.source_catalog_mapping_readiness_required;
    let source_catalog_mapping_readiness_report = input.source_catalog_mapping_readiness_report;
    let source_catalog_mapping_readiness_hash = input
        .source_catalog_mapping_readiness_report_hash
        .map(str::to_string);
    let source_catalog_mapping_readiness_id =
        source_catalog_mapping_readiness_report.map(|report| report.readiness_id.clone());
    let source_catalog_mapping_readiness_status =
        source_catalog_mapping_readiness_report.map(|report| report.status);
    let mut blockers = Vec::new();

    if readiness_id.trim().is_empty() {
        blockers.push(BackfillExecutionReadinessBlocker::EmptyReadinessId);
    }
    if required_table_family_trimmed.is_empty() {
        blockers.push(BackfillExecutionReadinessBlocker::EmptyRequiredTableFamily);
    }
    if supported_data_paths.is_empty() {
        blockers.push(BackfillExecutionReadinessBlocker::EmptySupportedDataPaths);
    }
    if required_nt_data_type_trimmed.is_empty() {
        blockers.push(BackfillExecutionReadinessBlocker::EmptyRequiredNtDataType);
    } else if !supported_data_paths
        .iter()
        .any(|path| path.nt_data_type.trim() == required_nt_data_type_trimmed)
    {
        blockers.push(BackfillExecutionReadinessBlocker::UnsupportedRequiredNtDataType);
    } else if !supported_data_paths.iter().any(|path| {
        path.table_family.trim() == required_table_family_trimmed
            && path.nt_data_type.trim() == required_nt_data_type_trimmed
    }) {
        blockers.push(BackfillExecutionReadinessBlocker::UnsupportedRequiredTableFamilyDataType);
    }

    if tranche.status != BackfillAcceptedTrancheStatus::Accepted {
        blockers.push(BackfillExecutionReadinessBlocker::AcceptedTrancheNotAccepted);
    }
    if !tranche.blocking_issues.is_empty() {
        blockers.push(BackfillExecutionReadinessBlocker::AcceptedTrancheHasBlockingIssues);
    }
    if tranche.object_count != 1 || tranche.objects.len() != 1 {
        blockers.push(BackfillExecutionReadinessBlocker::AcceptedTrancheObjectCountNotOne);
    }
    if tranche
        .objects
        .first()
        .is_some_and(|object| object.bytes != tranche.accepted_bytes)
    {
        blockers.push(BackfillExecutionReadinessBlocker::AcceptedTrancheBytesMismatch);
    }
    if plan.status != BackfillExecutionPlanStatus::Ready {
        blockers.push(BackfillExecutionReadinessBlocker::ExecutionPlanNotReady);
    }
    if !plan.blocking_issues.is_empty() {
        blockers.push(BackfillExecutionReadinessBlocker::ExecutionPlanHasBlockingIssues);
    }
    if plan.accepted_tranche_id != tranche.tranche_id {
        blockers.push(BackfillExecutionReadinessBlocker::ExecutionPlanAcceptedTrancheMismatch);
    }
    if plan.accepted_tranche_manifest_hash != accepted_tranche_manifest_hash {
        blockers.push(BackfillExecutionReadinessBlocker::ExecutionPlanAcceptedTrancheHashMismatch);
    }
    if plan.source_proof_id != tranche.source_proof_id
        || plan.source_proof_version != tranche.source_proof_version
    {
        blockers.push(BackfillExecutionReadinessBlocker::ExecutionPlanSourceProofMismatch);
    }
    if plan.source_binding != tranche.source_binding {
        blockers.push(BackfillExecutionReadinessBlocker::ExecutionPlanSourceBindingMismatch);
    }
    if plan.table_family != tranche.table_family {
        blockers.push(BackfillExecutionReadinessBlocker::ExecutionPlanTableFamilyMismatch);
    }
    if plan.table_family != required_table_family_trimmed
        || tranche.table_family != required_table_family_trimmed
    {
        blockers.push(BackfillExecutionReadinessBlocker::RequiredTableFamilyMismatch);
    }
    if plan.object_count != tranche.object_count
        || plan.objects.len() != tranche.objects.len()
        || plan.object_count as usize != plan.objects.len()
    {
        blockers.push(BackfillExecutionReadinessBlocker::ExecutionPlanObjectCountMismatch);
    }
    if plan.accepted_bytes != tranche.accepted_bytes {
        blockers.push(BackfillExecutionReadinessBlocker::ExecutionPlanAcceptedBytesMismatch);
    }
    if !plan
        .objects
        .iter()
        .zip(tranche.objects.iter())
        .all(|(plan_object, tranche_object)| {
            plan_object.s3_uri == tranche_object.s3_uri
                && plan_object.source_url == tranche_object.source_url
                && plan_object.sha256 == tranche_object.sha256
                && plan_object.bytes == tranche_object.bytes
                && plan_object.archive_date == tranche_object.archive_date
        })
    {
        blockers.push(BackfillExecutionReadinessBlocker::ExecutionPlanObjectMismatch);
    }
    if artifact_index_commit_required {
        match artifact_index_commit_proof_report {
            None => blockers.push(
                BackfillExecutionReadinessBlocker::ArtifactIndexCommitProofRequiredButMissing,
            ),
            Some(proof_report) => {
                match required_artifact_index_kind {
                    None => blockers.push(
                        BackfillExecutionReadinessBlocker::ArtifactIndexCommitProofKindRequiredButMissing,
                    ),
                    Some(required_kind) if proof_report.artifact_kind != required_kind => blockers
                        .push(
                            BackfillExecutionReadinessBlocker::ArtifactIndexCommitProofKindMismatch,
                        ),
                    Some(_) => {}
                }
                if !artifact_index_commit_mechanics_proven(proof_report) {
                    blockers.push(
                        BackfillExecutionReadinessBlocker::ArtifactIndexCommitMechanicsUnproven,
                    );
                }
                if !artifact_index_proof_root_matches_plan_output(
                    &plan.output_prefix,
                    &proof_report.artifact_root,
                ) {
                    blockers.push(
                        BackfillExecutionReadinessBlocker::ArtifactIndexCommitProofArtifactRootMismatch,
                    );
                }
                if !proof_report.producer_iam_scope_proven {
                    blockers.push(
                        BackfillExecutionReadinessBlocker::ArtifactIndexProducerIamScopeUnproven,
                    );
                }
            }
        }
    }
    if source_selection_readiness_required {
        match source_selection_readiness_report {
            None => blockers.push(
                BackfillExecutionReadinessBlocker::SourceSelectionReadinessRequiredButMissing,
            ),
            Some(readiness) => {
                if readiness.status != SourceSelectionReadinessStatus::Ready {
                    blockers
                        .push(BackfillExecutionReadinessBlocker::SourceSelectionReadinessNotReady);
                }
                if !readiness.blockers.is_empty() {
                    blockers.push(
                        BackfillExecutionReadinessBlocker::SourceSelectionReadinessHasBlockers,
                    );
                }
                if !source_selection_readiness_proven(readiness) {
                    blockers
                        .push(BackfillExecutionReadinessBlocker::SourceSelectionReadinessNotProven);
                }
                if readiness.usage_scope != required_source_usage_scope {
                    blockers.push(
                        BackfillExecutionReadinessBlocker::SourceSelectionReadinessUsageScopeMismatch,
                    );
                }
                if readiness.source_proof_id != tranche.source_proof_id
                    || readiness.source_proof_version != tranche.source_proof_version
                    || readiness.source_proof_id != plan.source_proof_id
                    || readiness.source_proof_version != plan.source_proof_version
                {
                    blockers.push(
                        BackfillExecutionReadinessBlocker::SourceSelectionReadinessSourceProofMismatch,
                    );
                }
                if readiness.source_binding != tranche.source_binding
                    || readiness.source_binding != plan.source_binding
                {
                    blockers.push(
                        BackfillExecutionReadinessBlocker::SourceSelectionReadinessSourceBindingMismatch,
                    );
                }
                if readiness.table_family != tranche.table_family
                    || readiness.table_family != plan.table_family
                    || readiness.required_table_family.trim() != required_table_family_trimmed
                {
                    blockers.push(
                        BackfillExecutionReadinessBlocker::SourceSelectionReadinessTableFamilyMismatch,
                    );
                }
            }
        }
    }
    if source_catalog_mapping_readiness_required {
        match source_catalog_mapping_readiness_report {
            None => blockers.push(
                BackfillExecutionReadinessBlocker::SourceCatalogMappingReadinessRequiredButMissing,
            ),
            Some(readiness) => {
                if readiness.status != SourceCatalogMappingReadinessStatus::Ready {
                    blockers.push(
                        BackfillExecutionReadinessBlocker::SourceCatalogMappingReadinessNotReady,
                    );
                }
                if !readiness.blockers.is_empty() {
                    blockers.push(
                        BackfillExecutionReadinessBlocker::SourceCatalogMappingReadinessHasBlockers,
                    );
                }
                if !readiness.nt_catalog_mapping_proven {
                    blockers.push(
                        BackfillExecutionReadinessBlocker::SourceCatalogMappingReadinessNotProven,
                    );
                }
                if readiness.source_proof_id != tranche.source_proof_id
                    || readiness.source_proof_version != tranche.source_proof_version
                    || readiness.source_proof_id != plan.source_proof_id
                    || readiness.source_proof_version != plan.source_proof_version
                {
                    blockers.push(
                        BackfillExecutionReadinessBlocker::SourceCatalogMappingReadinessSourceProofMismatch,
                    );
                }
                if readiness.source_binding != tranche.source_binding
                    || readiness.source_binding != plan.source_binding
                {
                    blockers.push(
                        BackfillExecutionReadinessBlocker::SourceCatalogMappingReadinessSourceBindingMismatch,
                    );
                }
                if readiness.required_table_family.trim() != required_table_family_trimmed
                    || readiness.required_table_family != tranche.table_family
                    || readiness.required_table_family != plan.table_family
                {
                    blockers.push(
                        BackfillExecutionReadinessBlocker::SourceCatalogMappingReadinessTableFamilyMismatch,
                    );
                }
                if !readiness
                    .required_nt_data_types
                    .iter()
                    .any(|data_type| data_type.trim() == required_nt_data_type_trimmed)
                {
                    blockers.push(
                        BackfillExecutionReadinessBlocker::SourceCatalogMappingReadinessNtDataTypeMismatch,
                    );
                }
                if !readiness
                    .allowed_usage_scopes
                    .contains(&required_source_usage_scope)
                    || readiness.observed_usage_scope != Some(required_source_usage_scope)
                {
                    blockers.push(
                        BackfillExecutionReadinessBlocker::SourceCatalogMappingReadinessUsageScopeMismatch,
                    );
                }
            }
        }
    }

    let status = if blockers.is_empty() {
        BackfillExecutionReadinessStatus::Ready
    } else {
        BackfillExecutionReadinessStatus::Blocked
    };

    BackfillExecutionReadinessReport {
        schema_version: BACKFILL_EXECUTION_READINESS_SCHEMA_VERSION.to_string(),
        readiness_id,
        status,
        required_table_family,
        required_nt_data_type,
        required_source_usage_scope,
        supported_data_paths,
        artifact_index_commit_required,
        required_artifact_index_kind,
        artifact_index_commit_proof_id,
        artifact_index_commit_proof_hash,
        artifact_index_direct_s3_commit_proven,
        artifact_index_producer_iam_scope_proven,
        source_selection_readiness_required,
        source_selection_readiness_id,
        source_selection_readiness_hash,
        source_selection_readiness_status,
        source_catalog_mapping_readiness_required,
        source_catalog_mapping_readiness_id,
        source_catalog_mapping_readiness_hash,
        source_catalog_mapping_readiness_status,
        accepted_tranche_id: tranche.tranche_id.clone(),
        accepted_tranche_manifest_hash,
        execution_plan_id: plan.plan_id.clone(),
        execution_plan_hash,
        operator_run_id: plan.operator_run_id.clone(),
        source_proof_id: tranche.source_proof_id.clone(),
        source_proof_version: tranche.source_proof_version,
        source_binding: tranche.source_binding.clone(),
        table_family: tranche.table_family.clone(),
        object_count: plan.object_count,
        accepted_bytes: plan.accepted_bytes,
        blockers,
    }
}

pub fn write_backfill_execution_readiness_report(
    output_dir: &Path,
    report: &BackfillExecutionReadinessReport,
) -> Result<BackfillExecutionReadinessArtifact, BackfillExecutionReadinessError> {
    fs::create_dir_all(output_dir).map_err(|error| BackfillExecutionReadinessError::CreateDir {
        path: output_dir.display().to_string(),
        error: error.to_string(),
    })?;
    let path = output_dir.join(BACKFILL_EXECUTION_READINESS_REPORT_FILE);
    let written = crate::reference_artifact::write_reference_artifact_with_len_mapped(
        &path,
        BACKFILL_EXECUTION_READINESS_REPORT_FILE,
        report,
        crate::reference_artifact::ReferenceArtifactRewrite::FailOnDirty,
        crate::reference_artifact::ReferenceArtifactErrorMappers {
            serialize_error: BackfillExecutionReadinessError::Serialize,
            read_existing_error: |path, error| BackfillExecutionReadinessError::ReadExisting {
                path,
                error,
            },
            mismatch_error: |path| BackfillExecutionReadinessError::ExistingArtifactMismatch {
                path,
            },
            write_error: |path, error| BackfillExecutionReadinessError::Write { path, error },
        },
    )?;
    Ok(BackfillExecutionReadinessArtifact {
        path,
        content_hash: written.pin.sha256,
        bytes: written.bytes,
    })
}

pub fn write_backfill_execution_readiness_report_from_spec_file(
    spec_path: &Path,
) -> Result<BackfillExecutionReadinessArtifact, BackfillExecutionReadinessError> {
    let spec_text = fs::read_to_string(spec_path).map_err(|error| {
        BackfillExecutionReadinessError::ReadSpec {
            path: spec_path.display().to_string(),
            error: error.to_string(),
        }
    })?;
    let spec: BackfillExecutionReadinessSpec = toml::from_str(&spec_text).map_err(|error| {
        BackfillExecutionReadinessError::ParseSpecToml {
            path: spec_path.display().to_string(),
            error: error.to_string(),
        }
    })?;
    let (tranche, tranche_hash) =
        read_accepted_tranche_manifest(&spec.accepted_tranche_manifest_path)?;
    let (plan, plan_hash) = read_execution_plan(&spec.execution_plan_path)?;
    let artifact_index_commit_proof_report =
        match spec.artifact_index_commit_proof_report_path.as_deref() {
            Some(path) => {
                let (report, hash) = read_artifact_index_commit_proof_report(path)?;
                Some((report, hash))
            }
            None => None,
        };
    let source_selection_readiness_report =
        match spec.source_selection_readiness_report_path.as_deref() {
            Some(path) => {
                let (report, hash) = read_source_selection_readiness_report(path)?;
                Some((report, hash))
            }
            None => None,
        };
    let source_catalog_mapping_readiness_report =
        match spec.source_catalog_mapping_readiness_report_path.as_deref() {
            Some(path) => {
                let (report, hash) = read_source_catalog_mapping_readiness_report(path)?;
                Some((report, hash))
            }
            None => None,
        };
    let report = evaluate_backfill_execution_readiness(BackfillExecutionReadinessInput {
        readiness_id: &spec.readiness_id,
        accepted_tranche_manifest_hash: &tranche_hash,
        tranche: &tranche,
        execution_plan_hash: &plan_hash,
        plan: &plan,
        required_table_family: &spec.required_table_family,
        required_nt_data_type: &spec.required_nt_data_type,
        required_source_usage_scope: spec.required_source_usage_scope,
        supported_data_paths: spec.supported_data_paths,
        artifact_index_commit_required: spec.artifact_index_commit_required,
        required_artifact_index_kind: spec.required_artifact_index_kind,
        artifact_index_commit_proof_report_hash: artifact_index_commit_proof_report
            .as_ref()
            .map(|(_, hash)| hash.as_str()),
        artifact_index_commit_proof_report: artifact_index_commit_proof_report
            .as_ref()
            .map(|(report, _)| report),
        source_selection_readiness_required: spec.source_selection_readiness_required,
        source_selection_readiness_report_hash: source_selection_readiness_report
            .as_ref()
            .map(|(_, hash)| hash.as_str()),
        source_selection_readiness_report: source_selection_readiness_report
            .as_ref()
            .map(|(report, _)| report),
        source_catalog_mapping_readiness_required: spec.source_catalog_mapping_readiness_required,
        source_catalog_mapping_readiness_report_hash: source_catalog_mapping_readiness_report
            .as_ref()
            .map(|(_, hash)| hash.as_str()),
        source_catalog_mapping_readiness_report: source_catalog_mapping_readiness_report
            .as_ref()
            .map(|(report, _)| report),
    });
    write_backfill_execution_readiness_report(&spec.output_dir, &report)
}

fn read_accepted_tranche_manifest(
    path: &Path,
) -> Result<(BackfillAcceptedTrancheManifest, String), BackfillExecutionReadinessError> {
    let bytes = fs::read(path).map_err(|error| {
        BackfillExecutionReadinessError::ReadAcceptedTrancheManifest {
            path: path.display().to_string(),
            error: error.to_string(),
        }
    })?;
    let hash = sha256_hex(&bytes);
    let manifest = serde_json::from_slice(&bytes).map_err(|error| {
        BackfillExecutionReadinessError::ParseAcceptedTrancheManifestJson {
            path: path.display().to_string(),
            error: error.to_string(),
        }
    })?;
    Ok((manifest, hash))
}

fn read_execution_plan(
    path: &Path,
) -> Result<(BackfillExecutionPlan, String), BackfillExecutionReadinessError> {
    let bytes =
        fs::read(path).map_err(|error| BackfillExecutionReadinessError::ReadExecutionPlan {
            path: path.display().to_string(),
            error: error.to_string(),
        })?;
    let hash = sha256_hex(&bytes);
    let plan = serde_json::from_slice(&bytes).map_err(|error| {
        BackfillExecutionReadinessError::ParseExecutionPlanJson {
            path: path.display().to_string(),
            error: error.to_string(),
        }
    })?;
    Ok((plan, hash))
}

fn read_artifact_index_commit_proof_report(
    path: &Path,
) -> Result<(ArtifactIndexCommitProofReport, String), BackfillExecutionReadinessError> {
    let bytes = fs::read(path).map_err(|error| {
        BackfillExecutionReadinessError::ReadArtifactIndexCommitProofReport {
            path: path.display().to_string(),
            error: error.to_string(),
        }
    })?;
    let hash = sha256_hex(&bytes);
    let report = serde_json::from_slice(&bytes).map_err(|error| {
        BackfillExecutionReadinessError::ParseArtifactIndexCommitProofReportJson {
            path: path.display().to_string(),
            error: error.to_string(),
        }
    })?;
    Ok((report, hash))
}

fn read_source_selection_readiness_report(
    path: &Path,
) -> Result<(SourceSelectionReadinessReport, String), BackfillExecutionReadinessError> {
    let bytes = fs::read(path).map_err(|error| {
        BackfillExecutionReadinessError::ReadSourceSelectionReadinessReport {
            path: path.display().to_string(),
            error: error.to_string(),
        }
    })?;
    let hash = sha256_hex(&bytes);
    let report = serde_json::from_slice(&bytes).map_err(|error| {
        BackfillExecutionReadinessError::ParseSourceSelectionReadinessReportJson {
            path: path.display().to_string(),
            error: error.to_string(),
        }
    })?;
    Ok((report, hash))
}

fn read_source_catalog_mapping_readiness_report(
    path: &Path,
) -> Result<(SourceCatalogMappingReadinessReport, String), BackfillExecutionReadinessError> {
    let bytes = fs::read(path).map_err(|error| {
        BackfillExecutionReadinessError::ReadSourceCatalogMappingReadinessReport {
            path: path.display().to_string(),
            error: error.to_string(),
        }
    })?;
    let hash = sha256_hex(&bytes);
    let report = serde_json::from_slice(&bytes).map_err(|error| {
        BackfillExecutionReadinessError::ParseSourceCatalogMappingReadinessReportJson {
            path: path.display().to_string(),
            error: error.to_string(),
        }
    })?;
    Ok((report, hash))
}

fn artifact_index_commit_mechanics_proven(report: &ArtifactIndexCommitProofReport) -> bool {
    report.direct_s3_commit_proven
        && report.prior_pointer_etag_observed
        && report.final_pointer_etag_observed
        && report.event_create_only_proven
        && report.snapshot_create_only_proven
        && report.audit_epoch_create_only_proven
        && report.latest_pointer_create_only_proven
        && report.latest_pointer_update_if_match_proven
        && report.stale_etag_update_rejected
        && report.latest_pointer_readback_proven
        && report.snapshot_readback_proven
}

fn artifact_index_proof_root_matches_plan_output(
    plan_output_prefix: &str,
    proof_artifact_root: &str,
) -> bool {
    let Some(plan_artifact_root) = artifact_root_from_backtests_output_prefix(plan_output_prefix)
    else {
        return false;
    };
    let plan_artifact_root = plan_artifact_root.trim_end_matches('/');
    let proof_artifact_root = proof_artifact_root.trim_end_matches('/');
    let proof_sandbox_root = format!(
        "{}/{}/proofs",
        plan_artifact_root,
        ArtifactSubpath::ArtifactIndex.as_str()
    );
    proof_artifact_root == plan_artifact_root
        || uri_is_under_root(proof_artifact_root, &proof_sandbox_root)
}

fn artifact_root_from_backtests_output_prefix(output_prefix: &str) -> Option<&str> {
    let output_prefix = output_prefix.trim_end_matches('/');
    let backtests_subpath = format!("/{}/", ArtifactSubpath::Backtests.as_str());
    let (artifact_root, run_path) = output_prefix.split_once(&backtests_subpath)?;
    (!artifact_root.is_empty() && !run_path.is_empty()).then_some(artifact_root)
}

fn uri_is_under_root(uri: &str, root: &str) -> bool {
    let uri = uri.trim_end_matches('/');
    let root = root.trim_end_matches('/');
    uri == root
        || uri
            .strip_prefix(root)
            .is_some_and(|suffix| suffix.starts_with('/'))
}

fn source_selection_readiness_proven(report: &SourceSelectionReadinessReport) -> bool {
    report.source_proof_accepted
        && report.canonical_usage_scope_proven
        && report.source_access_proven
        && report.license_proven
        && report.sample_schema_proven
        && report.time_semantics_proven
        && report.instrument_universe_proven
        && report.coverage_proven
        && report.retention_freshness_proven
        && report.granularity_proven
        && report.completeness_proven
        && report.nt_mapping_proven
        && report.cost_proven
        && report.storage_proven
        && report.claim_limits_recorded
        && report.source_proof_acceptance_error.is_none()
        && report.unmet_required_checks.is_empty()
}
