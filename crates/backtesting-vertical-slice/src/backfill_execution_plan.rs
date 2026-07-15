//! Backfill execution planning gate.
//!
//! This module binds an accepted object-level tranche to one operator run-spec
//! before any payload bytes are fetched. It is a cheap source-proof/object
//! guard, not a downloader or converter.

use std::{
    error::Error,
    fmt, fs,
    path::{Path, PathBuf},
};

use anyhow::{Context, Result as AnyResult, ensure};
use serde::{Deserialize, Serialize};

use crate::{
    backfill_accepted_tranche::{
        BACKFILL_ACCEPTED_TRANCHE_SCHEMA_VERSION, BackfillAcceptedTrancheManifest,
        BackfillAcceptedTrancheObject, BackfillAcceptedTrancheStatus,
    },
    hashing::{is_lowercase_sha256_hex, sha256_hex},
    operator::RunSpec,
    path_resolution::{resolve_existing_path, resolve_output_dir},
    source_proof::SourceProofUsageScope,
};

pub const BACKFILL_EXECUTION_PLAN_SCHEMA_VERSION: &str = "backfill-execution-plan.v1";
pub const BACKFILL_EXECUTION_PLAN_FILE: &str = "backfill-execution-plan.json";

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BackfillExecutionPlanSpec {
    pub plan_id: String,
    pub accepted_tranche_manifest_path: PathBuf,
    pub run_spec_path: PathBuf,
    pub max_source_rows: u64,
    pub max_projected_row_groups: u64,
    pub max_wall_seconds: u64,
    #[serde(default)]
    pub require_object_selection_metadata: bool,
    pub output_dir: PathBuf,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BackfillExecutionWorkBudget {
    pub max_source_rows: u64,
    pub max_projected_row_groups: u64,
    pub max_wall_seconds: u64,
    pub require_object_selection_metadata: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BackfillExecutionPlanStatus {
    Ready,
    Blocked,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BackfillExecutionPlanIssue {
    EmptyPlanId,
    AcceptedTrancheNotAccepted,
    AcceptedTrancheHasBlockingIssues,
    AcceptedTrancheObjectCountNotOne,
    AcceptedTrancheBytesMismatch,
    RunSpecSourceProofMismatch,
    RunSpecSourceUsageScopeMismatch,
    RunSpecSourceBindingMismatch,
    RunSpecTableFamilyMismatch,
    RunSpecRawSampleUriMismatch,
    RunSpecRawSampleHashMismatch,
    RunSpecObjectS3UriMismatch,
    RunSpecObjectSourceUrlMismatch,
    RunSpecObjectShaMismatch,
    RunSpecObjectBytesMismatch,
    RunSpecObjectArchiveDateMismatch,
    RunSpecObjectBudgetTooSmall,
    ExecutionPlanSourceRowBudgetMissing,
    ExecutionPlanProjectedRowGroupBudgetMissing,
    ExecutionPlanWallTimeBudgetMissing,
    ExecutionPlanObjectSelectionMetadataMissing,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BackfillExecutionRunBinding {
    pub run_id: String,
    pub output_prefix: String,
    pub source_proof_id: String,
    pub source_proof_version: u32,
    pub source_binding: String,
    pub table_family: String,
    pub source_usage_scope: SourceProofUsageScope,
    pub raw_sample_uri: String,
    pub raw_sample_hash: String,
    pub accepted_object_s3_uri: String,
    pub accepted_object_source_url: String,
    pub accepted_object_sha256: String,
    pub accepted_object_bytes: u64,
    pub accepted_object_archive_date: String,
    pub max_object_bytes: u64,
    pub max_decoded_bytes: u64,
}

impl BackfillExecutionRunBinding {
    #[must_use]
    pub fn from_run_spec(spec: &RunSpec) -> Self {
        Self {
            run_id: spec.manifest.run_id.clone(),
            output_prefix: spec.manifest.output_prefix.clone(),
            source_proof_id: spec.manifest.source_proof_id.clone(),
            source_proof_version: spec.manifest.source_proof_version,
            source_binding: spec.manifest.venue_binding_key.clone(),
            table_family: spec.source_proof.table_family.clone(),
            source_usage_scope: spec.source_proof.usage_scope,
            raw_sample_uri: spec.source_proof.raw_sample_uri.clone(),
            raw_sample_hash: spec.source_proof.raw_sample_hash.clone(),
            accepted_object_s3_uri: spec.accepted_object.s3_uri.clone(),
            accepted_object_source_url: spec.accepted_object.source_url.clone(),
            accepted_object_sha256: spec.accepted_object.sha256.clone(),
            accepted_object_bytes: spec.accepted_object.bytes,
            accepted_object_archive_date: spec.accepted_object.archive_date.clone(),
            max_object_bytes: spec.converter.raw_payload.max_object_bytes,
            max_decoded_bytes: spec.converter.raw_payload.max_decoded_bytes,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BackfillExecutionPlanObject {
    pub s3_uri: String,
    pub source_url: String,
    pub sha256: String,
    pub bytes: u64,
    pub archive_date: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub source_row_groups: Vec<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub predicate_ref: Option<String>,
}

impl From<&BackfillAcceptedTrancheObject> for BackfillExecutionPlanObject {
    fn from(object: &BackfillAcceptedTrancheObject) -> Self {
        Self {
            s3_uri: object.s3_uri.clone(),
            source_url: object.source_url.clone(),
            sha256: object.sha256.clone(),
            bytes: object.bytes,
            archive_date: object.archive_date.clone(),
            source_row_groups: object.source_row_groups.clone(),
            predicate_ref: object.predicate_ref.clone(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BackfillExecutionPlan {
    pub schema_version: String,
    pub plan_id: String,
    pub status: BackfillExecutionPlanStatus,
    pub accepted_tranche_id: String,
    pub accepted_tranche_manifest_hash: String,
    pub run_spec_hash: String,
    pub operator_run_id: String,
    pub output_prefix: String,
    pub source_proof_id: String,
    pub source_proof_version: u32,
    pub source_binding: String,
    pub table_family: String,
    #[serde(
        default = "default_source_usage_scope",
        skip_serializing_if = "is_canonical_backfill_input"
    )]
    pub source_usage_scope: SourceProofUsageScope,
    pub object_count: u64,
    pub accepted_bytes: u64,
    pub max_object_bytes: u64,
    pub max_decoded_bytes: u64,
    pub max_source_rows: u64,
    pub max_projected_row_groups: u64,
    pub max_wall_seconds: u64,
    #[serde(default, skip_serializing_if = "is_false")]
    pub require_object_selection_metadata: bool,
    pub objects: Vec<BackfillExecutionPlanObject>,
    pub blocking_issues: Vec<BackfillExecutionPlanIssue>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BackfillExecutionPlanArtifact {
    pub path: PathBuf,
    pub content_hash: String,
    pub bytes: u64,
}

/// One strict, cross-bound parse of the three controls retained by an
/// execution-pack record.
#[derive(Debug, Clone)]
pub struct ValidatedBackfillExecutionControls {
    pub run_spec: RunSpec,
    pub accepted_tranche: BackfillAcceptedTrancheManifest,
    pub execution_plan: BackfillExecutionPlan,
    pub run_spec_sha256: String,
    pub accepted_tranche_sha256: String,
    pub execution_plan_sha256: String,
}

/// Validate the accepted-tranche contract independently of plan evaluation.
///
/// # Errors
///
/// Returns an error unless the manifest is an accepted, blocker-free,
/// one-complete-object tranche whose counters and digest fields are exact.
pub fn validate_backfill_accepted_tranche_manifest(
    tranche: &BackfillAcceptedTrancheManifest,
) -> AnyResult<()> {
    ensure!(
        tranche.schema_version == BACKFILL_ACCEPTED_TRANCHE_SCHEMA_VERSION,
        "accepted_tranche schema_version mismatch: expected {}, got {}",
        BACKFILL_ACCEPTED_TRANCHE_SCHEMA_VERSION,
        tranche.schema_version
    );
    ensure!(
        !tranche.tranche_id.trim().is_empty(),
        "accepted_tranche tranche_id must not be empty"
    );
    ensure!(
        tranche.status == BackfillAcceptedTrancheStatus::Accepted,
        "accepted_tranche status must be accepted"
    );
    ensure!(
        tranche.blocking_issues.is_empty(),
        "accepted_tranche must not contain blocking issues"
    );
    ensure!(
        tranche.object_count == 1 && tranche.objects.len() == 1,
        "accepted_tranche must bind exactly one object"
    );
    ensure!(
        !tranche.source_proof_scope_report_id.trim().is_empty(),
        "accepted_tranche source_proof_scope_report_id must not be empty"
    );
    validate_sha256_shape(
        "accepted_tranche source_proof_scope_report_hash",
        &tranche.source_proof_scope_report_hash,
    )?;
    ensure!(
        !tranche.source_proof_id.trim().is_empty() && tranche.source_proof_version > 0,
        "accepted_tranche source proof identity must be complete"
    );
    ensure!(
        !tranche.source_binding.trim().is_empty(),
        "accepted_tranche source_binding must not be empty"
    );
    ensure!(
        !tranche.table_family.trim().is_empty(),
        "accepted_tranche table_family must not be empty"
    );
    ensure!(
        !tranche.parent_manifest_id.trim().is_empty(),
        "accepted_tranche parent_manifest_id must not be empty"
    );

    let object = &tranche.objects[0];
    ensure!(
        !object.s3_uri.trim().is_empty(),
        "accepted_tranche object s3_uri must not be empty"
    );
    ensure!(
        !object.source_url.trim().is_empty(),
        "accepted_tranche object source_url must not be empty"
    );
    validate_sha256_shape("accepted_tranche object sha256", &object.sha256)?;
    ensure!(
        object.bytes > 0,
        "accepted_tranche object bytes must be positive"
    );
    ensure!(
        !object.archive_date.trim().is_empty(),
        "accepted_tranche object archive_date must not be empty"
    );
    ensure!(
        tranche.accepted_bytes == object.bytes,
        "accepted_tranche accepted_bytes {} does not equal object bytes {}",
        tranche.accepted_bytes,
        object.bytes
    );
    validate_object_selection_metadata(
        "accepted_tranche object",
        &object.source_row_groups,
        object.predicate_ref.as_deref(),
        false,
    )?;
    Ok(())
}

/// Validate one execution plan against the exact run-spec bytes submitted to
/// the operator CLI or retained by an execution pack.
///
/// # Errors
///
/// Returns an error for schema, status, scope, binding, object, digest, budget,
/// or selection-metadata drift.
pub fn validate_execution_plan_for_run_spec(
    plan: &BackfillExecutionPlan,
    run_spec_hash: &str,
    spec: &RunSpec,
) -> AnyResult<()> {
    ensure!(
        plan.schema_version == BACKFILL_EXECUTION_PLAN_SCHEMA_VERSION,
        "execution plan schema_version mismatch: expected {}, got {}",
        BACKFILL_EXECUTION_PLAN_SCHEMA_VERSION,
        plan.schema_version
    );
    ensure!(
        !plan.plan_id.trim().is_empty(),
        "execution plan plan_id must not be empty"
    );
    ensure!(
        plan.status == BackfillExecutionPlanStatus::Ready,
        "execution plan status must be ready"
    );
    ensure!(
        plan.blocking_issues.is_empty(),
        "execution plan has blocking issues"
    );
    ensure!(
        !plan.accepted_tranche_id.trim().is_empty(),
        "execution plan accepted_tranche_id must not be empty"
    );
    validate_sha256_shape(
        "execution plan accepted_tranche_manifest_hash",
        &plan.accepted_tranche_manifest_hash,
    )?;
    validate_sha256_shape("execution plan run_spec_hash", &plan.run_spec_hash)?;
    validate_sha256_shape("submitted run_spec_hash", run_spec_hash)?;
    ensure!(
        plan.run_spec_hash == run_spec_hash,
        "execution plan run_spec_hash {} does not match submitted run-spec {run_spec_hash}",
        plan.run_spec_hash
    );
    let binding = BackfillExecutionRunBinding::from_run_spec(spec);
    ensure!(
        plan.operator_run_id == binding.run_id,
        "execution plan operator_run_id mismatch"
    );
    ensure!(
        plan.output_prefix == binding.output_prefix,
        "execution plan output_prefix mismatch"
    );
    ensure!(
        plan.source_proof_id == binding.source_proof_id
            && plan.source_proof_version == binding.source_proof_version,
        "execution plan source proof mismatch"
    );
    ensure!(
        plan.source_binding == binding.source_binding,
        "execution plan source binding mismatch"
    );
    ensure!(
        plan.table_family == binding.table_family,
        "execution plan table_family {} does not match submitted run-spec {}",
        plan.table_family,
        binding.table_family
    );
    ensure!(
        plan.source_usage_scope == binding.source_usage_scope,
        "execution plan source usage scope mismatch"
    );
    ensure!(
        plan.object_count == 1 && plan.objects.len() == 1,
        "execution plan must bind exactly one accepted object"
    );
    let object = &plan.objects[0];
    validate_sha256_shape("execution plan object sha256", &object.sha256)?;
    ensure!(
        plan.accepted_bytes == object.bytes,
        "execution plan accepted_bytes mismatch"
    );
    ensure!(
        object.s3_uri == binding.raw_sample_uri && object.s3_uri == binding.accepted_object_s3_uri,
        "execution plan object URI mismatch"
    );
    ensure!(
        object.sha256 == binding.raw_sample_hash && object.sha256 == binding.accepted_object_sha256,
        "execution plan object hash mismatch"
    );
    ensure!(
        object.source_url == binding.accepted_object_source_url,
        "execution plan object source URL mismatch"
    );
    ensure!(
        object.bytes == binding.accepted_object_bytes,
        "execution plan object byte count mismatch"
    );
    ensure!(
        object.archive_date == binding.accepted_object_archive_date,
        "execution plan object archive date mismatch"
    );
    ensure!(
        plan.max_object_bytes == binding.max_object_bytes && object.bytes <= plan.max_object_bytes,
        "execution plan object byte budget mismatch"
    );
    ensure!(
        plan.max_decoded_bytes == binding.max_decoded_bytes,
        "execution plan decoded byte budget mismatch"
    );
    ensure!(
        plan.max_source_rows > 0,
        "execution plan max_source_rows must be positive"
    );
    ensure!(
        plan.max_projected_row_groups > 0,
        "execution plan max_projected_row_groups must be positive"
    );
    ensure!(
        plan.max_wall_seconds > 0,
        "execution plan max_wall_seconds must be positive"
    );
    validate_object_selection_metadata(
        "execution plan object",
        &object.source_row_groups,
        object.predicate_ref.as_deref(),
        plan.require_object_selection_metadata,
    )?;
    Ok(())
}

/// Strictly parse and cross-bind the retained run spec, accepted tranche, and
/// execution plan using the exact bytes hashed by the execution pack.
///
/// # Errors
///
/// Returns an error for parse failure, an invalid individual artifact, a hash
/// mismatch between artifacts, or any plan which is not exactly the result of
/// evaluating the retained tranche/run-spec with its declared work budget.
pub fn validate_backfill_execution_control_bytes(
    run_spec_bytes: &[u8],
    accepted_tranche_bytes: &[u8],
    execution_plan_bytes: &[u8],
) -> AnyResult<ValidatedBackfillExecutionControls> {
    let run_spec_text =
        std::str::from_utf8(run_spec_bytes).context("decode retained run_spec as UTF-8")?;
    let run_spec: RunSpec =
        toml::from_str(run_spec_text).context("parse retained run_spec TOML")?;
    let accepted_tranche: BackfillAcceptedTrancheManifest =
        serde_json::from_slice(accepted_tranche_bytes)
            .context("parse retained accepted_tranche JSON")?;
    let execution_plan: BackfillExecutionPlan = serde_json::from_slice(execution_plan_bytes)
        .context("parse retained execution_plan JSON")?;
    let run_spec_sha256 = sha256_hex(run_spec_bytes);
    let accepted_tranche_sha256 = sha256_hex(accepted_tranche_bytes);
    let execution_plan_sha256 = sha256_hex(execution_plan_bytes);

    validate_backfill_accepted_tranche_manifest(&accepted_tranche)?;
    validate_execution_plan_for_run_spec(&execution_plan, &run_spec_sha256, &run_spec)?;
    ensure!(
        execution_plan.accepted_tranche_id == accepted_tranche.tranche_id,
        "execution_plan accepted_tranche_id does not match retained accepted_tranche"
    );
    ensure!(
        execution_plan.accepted_tranche_manifest_hash == accepted_tranche_sha256,
        "execution_plan accepted_tranche_manifest_hash does not match retained accepted_tranche bytes"
    );

    let expected_plan = evaluate_backfill_execution_plan(
        execution_plan.plan_id.clone(),
        accepted_tranche_sha256.clone(),
        &accepted_tranche,
        run_spec_sha256.clone(),
        &BackfillExecutionRunBinding::from_run_spec(&run_spec),
        BackfillExecutionWorkBudget {
            max_source_rows: execution_plan.max_source_rows,
            max_projected_row_groups: execution_plan.max_projected_row_groups,
            max_wall_seconds: execution_plan.max_wall_seconds,
            require_object_selection_metadata: execution_plan.require_object_selection_metadata,
        },
    );
    ensure!(
        execution_plan == expected_plan,
        "retained execution_plan is not the exact evaluation of its accepted_tranche, run_spec, and declared work budget"
    );

    Ok(ValidatedBackfillExecutionControls {
        run_spec,
        accepted_tranche,
        execution_plan,
        run_spec_sha256,
        accepted_tranche_sha256,
        execution_plan_sha256,
    })
}

fn validate_sha256_shape(field: &str, value: &str) -> AnyResult<()> {
    ensure!(
        is_lowercase_sha256_hex(value),
        "{field} must be exactly 64 lowercase hexadecimal characters"
    );
    Ok(())
}

fn validate_object_selection_metadata(
    field: &str,
    source_row_groups: &[u64],
    predicate_ref: Option<&str>,
    required: bool,
) -> AnyResult<()> {
    ensure!(
        source_row_groups.windows(2).all(|pair| pair[0] < pair[1]),
        "{field} source_row_groups must be strictly increasing and unique"
    );
    if let Some(predicate_ref) = predicate_ref {
        ensure!(
            !predicate_ref.trim().is_empty(),
            "{field} predicate_ref must not be empty when present"
        );
    }
    ensure!(
        !required || !source_row_groups.is_empty() || predicate_ref.is_some(),
        "{field} requires source_row_groups or predicate_ref selection metadata"
    );
    Ok(())
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BackfillExecutionPlanError {
    ReadSpec { path: String, error: String },
    ParseSpecToml { path: String, error: String },
    ReadAcceptedTrancheManifest { path: String, error: String },
    ParseAcceptedTrancheManifestJson { path: String, error: String },
    ReadRunSpec { path: String, error: String },
    ParseRunSpecToml { path: String, error: String },
    CreateDir { path: String, error: String },
    ReadExisting { path: String, error: String },
    Write { path: String, error: String },
    ExistingArtifactMismatch { path: String },
    Serialize(String),
}

impl fmt::Display for BackfillExecutionPlanError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::ReadSpec { path, error } => {
                write!(f, "read backfill execution-plan spec {path}: {error}")
            }
            Self::ParseSpecToml { path, error } => {
                write!(f, "parse backfill execution-plan spec TOML {path}: {error}")
            }
            Self::ReadAcceptedTrancheManifest { path, error } => {
                write!(f, "read accepted-tranche manifest {path}: {error}")
            }
            Self::ParseAcceptedTrancheManifestJson { path, error } => {
                write!(f, "parse accepted-tranche manifest JSON {path}: {error}")
            }
            Self::ReadRunSpec { path, error } => write!(f, "read run-spec {path}: {error}"),
            Self::ParseRunSpecToml { path, error } => {
                write!(f, "parse run-spec TOML {path}: {error}")
            }
            Self::CreateDir { path, error } => {
                write!(
                    f,
                    "create backfill execution-plan artifact directory {path}: {error}"
                )
            }
            Self::ReadExisting { path, error } => {
                write!(
                    f,
                    "read existing backfill execution-plan artifact {path}: {error}"
                )
            }
            Self::Write { path, error } => {
                write!(f, "write backfill execution-plan artifact {path}: {error}")
            }
            Self::ExistingArtifactMismatch { path } => write!(
                f,
                "dirty backfill execution-plan artifact {path}: existing file content differs"
            ),
            Self::Serialize(error) => {
                write!(f, "serialize backfill execution-plan artifact: {error}")
            }
        }
    }
}

impl Error for BackfillExecutionPlanError {}

#[must_use]
pub fn evaluate_backfill_execution_plan(
    plan_id: impl Into<String>,
    accepted_tranche_manifest_hash: impl Into<String>,
    tranche: &BackfillAcceptedTrancheManifest,
    run_spec_hash: impl Into<String>,
    run_binding: &BackfillExecutionRunBinding,
    work_budget: BackfillExecutionWorkBudget,
) -> BackfillExecutionPlan {
    let plan_id = plan_id.into();
    let accepted_tranche_manifest_hash = accepted_tranche_manifest_hash.into();
    let run_spec_hash = run_spec_hash.into();
    let mut blocking_issues = Vec::new();

    if plan_id.trim().is_empty() {
        blocking_issues.push(BackfillExecutionPlanIssue::EmptyPlanId);
    }
    if tranche.status != BackfillAcceptedTrancheStatus::Accepted {
        blocking_issues.push(BackfillExecutionPlanIssue::AcceptedTrancheNotAccepted);
    }
    if !tranche.blocking_issues.is_empty() {
        blocking_issues.push(BackfillExecutionPlanIssue::AcceptedTrancheHasBlockingIssues);
    }
    if tranche.object_count != 1 || tranche.objects.len() != 1 {
        blocking_issues.push(BackfillExecutionPlanIssue::AcceptedTrancheObjectCountNotOne);
    }
    if tranche
        .objects
        .first()
        .is_some_and(|object| object.bytes != tranche.accepted_bytes)
    {
        blocking_issues.push(BackfillExecutionPlanIssue::AcceptedTrancheBytesMismatch);
    }
    if work_budget.max_source_rows == 0 {
        blocking_issues.push(BackfillExecutionPlanIssue::ExecutionPlanSourceRowBudgetMissing);
    }
    if work_budget.max_projected_row_groups == 0 {
        blocking_issues
            .push(BackfillExecutionPlanIssue::ExecutionPlanProjectedRowGroupBudgetMissing);
    }
    if work_budget.max_wall_seconds == 0 {
        blocking_issues.push(BackfillExecutionPlanIssue::ExecutionPlanWallTimeBudgetMissing);
    }
    if tranche.source_proof_id != run_binding.source_proof_id
        || tranche.source_proof_version != run_binding.source_proof_version
    {
        blocking_issues.push(BackfillExecutionPlanIssue::RunSpecSourceProofMismatch);
    }
    if tranche.source_usage_scope != run_binding.source_usage_scope {
        blocking_issues.push(BackfillExecutionPlanIssue::RunSpecSourceUsageScopeMismatch);
    }
    if tranche.source_binding != run_binding.source_binding {
        blocking_issues.push(BackfillExecutionPlanIssue::RunSpecSourceBindingMismatch);
    }
    if tranche.table_family != run_binding.table_family {
        blocking_issues.push(BackfillExecutionPlanIssue::RunSpecTableFamilyMismatch);
    }

    if let Some(object) = tranche.objects.first() {
        if object.s3_uri != run_binding.raw_sample_uri {
            blocking_issues.push(BackfillExecutionPlanIssue::RunSpecRawSampleUriMismatch);
        }
        if object.sha256 != run_binding.raw_sample_hash {
            blocking_issues.push(BackfillExecutionPlanIssue::RunSpecRawSampleHashMismatch);
        }
        if object.s3_uri != run_binding.accepted_object_s3_uri {
            blocking_issues.push(BackfillExecutionPlanIssue::RunSpecObjectS3UriMismatch);
        }
        if object.source_url != run_binding.accepted_object_source_url {
            blocking_issues.push(BackfillExecutionPlanIssue::RunSpecObjectSourceUrlMismatch);
        }
        if object.sha256 != run_binding.accepted_object_sha256 {
            blocking_issues.push(BackfillExecutionPlanIssue::RunSpecObjectShaMismatch);
        }
        if object.bytes != run_binding.accepted_object_bytes {
            blocking_issues.push(BackfillExecutionPlanIssue::RunSpecObjectBytesMismatch);
        }
        if object.archive_date != run_binding.accepted_object_archive_date {
            blocking_issues.push(BackfillExecutionPlanIssue::RunSpecObjectArchiveDateMismatch);
        }
        if object.bytes > run_binding.max_object_bytes {
            blocking_issues.push(BackfillExecutionPlanIssue::RunSpecObjectBudgetTooSmall);
        }
        if work_budget.require_object_selection_metadata
            && object.source_row_groups.is_empty()
            && object.predicate_ref.is_none()
        {
            blocking_issues
                .push(BackfillExecutionPlanIssue::ExecutionPlanObjectSelectionMetadataMissing);
        }
    }

    let status = if blocking_issues.is_empty() {
        BackfillExecutionPlanStatus::Ready
    } else {
        BackfillExecutionPlanStatus::Blocked
    };
    let objects = if status == BackfillExecutionPlanStatus::Ready {
        tranche
            .objects
            .iter()
            .map(BackfillExecutionPlanObject::from)
            .collect()
    } else {
        Vec::new()
    };

    BackfillExecutionPlan {
        schema_version: BACKFILL_EXECUTION_PLAN_SCHEMA_VERSION.to_string(),
        plan_id,
        status,
        accepted_tranche_id: tranche.tranche_id.clone(),
        accepted_tranche_manifest_hash,
        run_spec_hash,
        operator_run_id: run_binding.run_id.clone(),
        output_prefix: run_binding.output_prefix.clone(),
        source_proof_id: tranche.source_proof_id.clone(),
        source_proof_version: tranche.source_proof_version,
        source_binding: tranche.source_binding.clone(),
        table_family: tranche.table_family.clone(),
        source_usage_scope: tranche.source_usage_scope,
        object_count: objects.len() as u64,
        accepted_bytes: objects.iter().map(|object| object.bytes).sum(),
        max_object_bytes: run_binding.max_object_bytes,
        max_decoded_bytes: run_binding.max_decoded_bytes,
        max_source_rows: work_budget.max_source_rows,
        max_projected_row_groups: work_budget.max_projected_row_groups,
        max_wall_seconds: work_budget.max_wall_seconds,
        require_object_selection_metadata: work_budget.require_object_selection_metadata,
        objects,
        blocking_issues,
    }
}

pub fn write_backfill_execution_plan_from_spec_file(
    spec_path: &Path,
) -> Result<BackfillExecutionPlanArtifact, BackfillExecutionPlanError> {
    let spec_text =
        fs::read_to_string(spec_path).map_err(|error| BackfillExecutionPlanError::ReadSpec {
            path: spec_path.display().to_string(),
            error: error.to_string(),
        })?;
    let spec: BackfillExecutionPlanSpec =
        toml::from_str(&spec_text).map_err(|error| BackfillExecutionPlanError::ParseSpecToml {
            path: spec_path.display().to_string(),
            error: error.to_string(),
        })?;
    let base_dir = spec_path.parent().unwrap_or_else(|| Path::new("."));
    let resolved_accepted_tranche_manifest_path =
        resolve_existing_path(base_dir, &spec.accepted_tranche_manifest_path);
    let (tranche, accepted_tranche_manifest_hash) = read_accepted_tranche_manifest(
        &resolved_accepted_tranche_manifest_path,
        &spec.accepted_tranche_manifest_path,
    )?;
    let resolved_run_spec_path = resolve_existing_path(base_dir, &spec.run_spec_path);
    let (run_spec, run_spec_hash) = read_run_spec(&resolved_run_spec_path, &spec.run_spec_path)?;
    let run_binding = BackfillExecutionRunBinding::from_run_spec(&run_spec);
    let work_budget = BackfillExecutionWorkBudget {
        max_source_rows: spec.max_source_rows,
        max_projected_row_groups: spec.max_projected_row_groups,
        max_wall_seconds: spec.max_wall_seconds,
        require_object_selection_metadata: spec.require_object_selection_metadata,
    };
    let plan = evaluate_backfill_execution_plan(
        spec.plan_id,
        accepted_tranche_manifest_hash,
        &tranche,
        run_spec_hash,
        &run_binding,
        work_budget,
    );
    let output_dir = resolve_output_dir(base_dir, &spec.output_dir);
    write_backfill_execution_plan(&output_dir, &plan)
}

pub fn write_backfill_execution_plan(
    output_dir: &Path,
    plan: &BackfillExecutionPlan,
) -> Result<BackfillExecutionPlanArtifact, BackfillExecutionPlanError> {
    write_backfill_execution_plan_with_overwrite(output_dir, plan, false)
}

pub fn write_backfill_execution_plan_with_overwrite(
    output_dir: &Path,
    plan: &BackfillExecutionPlan,
    overwrite_existing: bool,
) -> Result<BackfillExecutionPlanArtifact, BackfillExecutionPlanError> {
    fs::create_dir_all(output_dir).map_err(|error| BackfillExecutionPlanError::CreateDir {
        path: output_dir.display().to_string(),
        error: error.to_string(),
    })?;
    let path = output_dir.join(BACKFILL_EXECUTION_PLAN_FILE);
    let rewrite = if overwrite_existing {
        crate::reference_artifact::ReferenceArtifactRewrite::OverwriteIfChanged
    } else {
        crate::reference_artifact::ReferenceArtifactRewrite::FailOnDirty
    };
    let written = crate::reference_artifact::write_reference_artifact_with_len_mapped(
        &path,
        BACKFILL_EXECUTION_PLAN_FILE,
        plan,
        rewrite,
        crate::reference_artifact::ReferenceArtifactErrorMappers {
            serialize_error: BackfillExecutionPlanError::Serialize,
            read_existing_error: |path, error| BackfillExecutionPlanError::ReadExisting {
                path,
                error,
            },
            mismatch_error: |path| BackfillExecutionPlanError::ExistingArtifactMismatch { path },
            write_error: |path, error| BackfillExecutionPlanError::Write { path, error },
        },
    )?;

    Ok(BackfillExecutionPlanArtifact {
        path,
        content_hash: written.pin.sha256,
        bytes: written.bytes,
    })
}

fn read_accepted_tranche_manifest(
    path: &Path,
    display_path: &Path,
) -> Result<(BackfillAcceptedTrancheManifest, String), BackfillExecutionPlanError> {
    let bytes = fs::read(path).map_err(|error| {
        BackfillExecutionPlanError::ReadAcceptedTrancheManifest {
            path: display_path.display().to_string(),
            error: error.to_string(),
        }
    })?;
    let hash = sha256_hex(&bytes);
    let manifest = serde_json::from_slice(&bytes).map_err(|error| {
        BackfillExecutionPlanError::ParseAcceptedTrancheManifestJson {
            path: display_path.display().to_string(),
            error: error.to_string(),
        }
    })?;
    Ok((manifest, hash))
}

fn read_run_spec(
    path: &Path,
    display_path: &Path,
) -> Result<(RunSpec, String), BackfillExecutionPlanError> {
    let bytes = fs::read(path).map_err(|error| BackfillExecutionPlanError::ReadRunSpec {
        path: display_path.display().to_string(),
        error: error.to_string(),
    })?;
    let hash = sha256_hex(&bytes);
    let text = std::str::from_utf8(&bytes).map_err(|error| {
        BackfillExecutionPlanError::ParseRunSpecToml {
            path: display_path.display().to_string(),
            error: error.to_string(),
        }
    })?;
    let run_spec =
        toml::from_str(text).map_err(|error| BackfillExecutionPlanError::ParseRunSpecToml {
            path: display_path.display().to_string(),
            error: error.to_string(),
        })?;
    Ok((run_spec, hash))
}
fn is_false(value: &bool) -> bool {
    !*value
}

fn default_source_usage_scope() -> SourceProofUsageScope {
    SourceProofUsageScope::CanonicalBackfillInput
}

fn is_canonical_backfill_input(value: &SourceProofUsageScope) -> bool {
    *value == SourceProofUsageScope::CanonicalBackfillInput
}
