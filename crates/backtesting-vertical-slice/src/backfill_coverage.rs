//! Run-level backfill coverage ledger.
//!
//! This is the pre-normalization gate for staged backfill runs. It classifies
//! normalized manifest facts and optional physical S3 inventory summaries before
//! any payload download, canonical write, NT catalog projection, or backtest.

use std::{
    collections::BTreeMap,
    error::Error,
    fmt, fs,
    path::{Path, PathBuf},
};

use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};

use crate::source_proof::SourceProofStatus;

pub const BACKFILL_COVERAGE_LEDGER_SCHEMA_VERSION: &str = "backfill-coverage-ledger.v1";
pub const BACKFILL_COVERAGE_LEDGER_FILE: &str = "backfill-coverage-ledger.json";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BackfillWriteMode {
    DryRun,
    LocalStaging,
    S3Staging,
    CanonicalS3,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BackfillCoverageStatus {
    Accepted,
    AcceptedWithGaps,
    Rejected,
    PhysicalOnly,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BackfillCoverageIssue {
    EmptyManifestId,
    EmptySourceBinding,
    MissingSourceProof,
    SourceProofNotAccepted,
    MissingManifest,
    PlannedObjectsNotPositive,
    CompletedObjectsNotPositive,
    CompletedBytesNotPositive,
    FailedObjectsPresent,
    SelectorScopeViolationsPresent,
    PlannedObjectsAccountingMismatch,
    SkippedObjectsWithoutGapPolicy,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BackfillCoverageParseError {
    MissingField(&'static str),
    InvalidField { field: &'static str, value: String },
    UnknownWriteMode(String),
}

impl fmt::Display for BackfillCoverageParseError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::MissingField(field) => {
                write!(f, "backfill coverage manifest missing field {field}")
            }
            Self::InvalidField { field, value } => write!(
                f,
                "backfill coverage manifest field {field} has invalid value {value:?}"
            ),
            Self::UnknownWriteMode(value) => {
                write!(
                    f,
                    "backfill coverage manifest write_mode is unsupported: {value:?}"
                )
            }
        }
    }
}

impl Error for BackfillCoverageParseError {}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BackfillCoverageLedgerError {
    EmptyLedgerId,
    ParseManifest {
        manifest_uri: String,
        source: BackfillCoverageParseError,
    },
    DuplicateManifestId(String),
    DuplicateInventoryId(String),
    Serialize(String),
}

impl fmt::Display for BackfillCoverageLedgerError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::EmptyLedgerId => write!(f, "backfill coverage ledger id must not be empty"),
            Self::ParseManifest {
                manifest_uri,
                source,
            } => write!(
                f,
                "parse backfill coverage manifest {manifest_uri}: {source}"
            ),
            Self::DuplicateManifestId(manifest_id) => {
                write!(
                    f,
                    "backfill coverage ledger has duplicate manifest id {manifest_id:?}"
                )
            }
            Self::DuplicateInventoryId(inventory_id) => {
                write!(
                    f,
                    "backfill coverage ledger has duplicate inventory id {inventory_id:?}"
                )
            }
            Self::Serialize(error) => write!(f, "serialize backfill coverage ledger: {error}"),
        }
    }
}

impl Error for BackfillCoverageLedgerError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::ParseManifest { source, .. } => Some(source),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BackfillCoverageManifestFileError {
    ReadManifest {
        manifest_uri: String,
        path: String,
        error: String,
    },
    ParseManifestJson {
        manifest_uri: String,
        path: String,
        error: String,
    },
    BuildLedger(BackfillCoverageLedgerError),
    WriteArtifact(BackfillCoverageWriteError),
}

impl fmt::Display for BackfillCoverageManifestFileError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::ReadManifest {
                manifest_uri,
                path,
                error,
            } => write!(
                f,
                "read backfill coverage manifest {manifest_uri} from {path}: {error}"
            ),
            Self::ParseManifestJson {
                manifest_uri,
                path,
                error,
            } => write!(
                f,
                "parse backfill coverage manifest JSON {manifest_uri} from {path}: {error}"
            ),
            Self::BuildLedger(error) => write!(f, "build backfill coverage ledger: {error}"),
            Self::WriteArtifact(error) => write!(f, "write backfill coverage ledger: {error}"),
        }
    }
}

impl Error for BackfillCoverageManifestFileError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::BuildLedger(error) => Some(error),
            Self::WriteArtifact(error) => Some(error),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BackfillCoverageWriteError {
    CreateDir { path: String, error: String },
    ReadExisting { path: String, error: String },
    Write { path: String, error: String },
    Serialize(String),
    ExistingArtifactMismatch { path: String },
}

impl fmt::Display for BackfillCoverageWriteError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::CreateDir { path, error } => {
                write!(
                    f,
                    "create backfill coverage artifact directory {path}: {error}"
                )
            }
            Self::ReadExisting { path, error } => {
                write!(
                    f,
                    "read existing backfill coverage artifact {path}: {error}"
                )
            }
            Self::Write { path, error } => {
                write!(f, "write backfill coverage artifact {path}: {error}")
            }
            Self::Serialize(error) => write!(f, "serialize backfill coverage artifact: {error}"),
            Self::ExistingArtifactMismatch { path } => write!(
                f,
                "dirty backfill coverage artifact {path}: existing file content differs"
            ),
        }
    }
}

impl Error for BackfillCoverageWriteError {}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BackfillCoverageManifestEvidence {
    pub manifest_id: String,
    pub source_binding: String,
    pub source_proof_id: Option<String>,
    pub source_proof_version: Option<u32>,
    pub source_proof_status: Option<SourceProofStatus>,
    pub write_mode: BackfillWriteMode,
    pub canonical_s3_write: bool,
    pub planned_objects: u64,
    pub completed_objects: u64,
    pub failed_objects: u64,
    pub skipped_objects: u64,
    pub completed_bytes: u64,
    pub selector_scope_violations: u64,
    pub gap_policy_id: Option<String>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct BackfillCoverageManifestJson {
    pub manifest_uri: String,
    pub summary: Value,
    pub source_proof_status: Option<SourceProofStatus>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BackfillCoverageManifestFile {
    pub manifest_uri: String,
    pub path: PathBuf,
    pub source_proof_status: Option<SourceProofStatus>,
}

impl BackfillCoverageManifestEvidence {
    pub fn from_manifest_json(
        summary: &Value,
        source_proof_status: Option<SourceProofStatus>,
    ) -> Result<Self, BackfillCoverageParseError> {
        let manifest_id =
            required_string(summary, "manifest_id", &[&["manifest_id"], &["run_id"]])?;
        let source_binding = optional_string(summary, &[&["source_binding"]]).unwrap_or_default();
        let source_proof_id = optional_string(summary, &[&["source_proof_id"]]);
        let source_proof_version = optional_u32(
            summary,
            "source_proof_version",
            &[&["source_proof_version"]],
        )?;
        let write_mode =
            parse_write_mode(&required_string(summary, "write_mode", &[&["write_mode"]])?)?;
        let canonical_s3_write =
            optional_bool(summary, "canonical_s3_write", &[&["canonical_s3_write"]])?
                .unwrap_or(false);
        let completed_objects = required_u64(
            summary,
            "completed_objects",
            &[
                &["completed_objects"],
                &["completed_payload_object_count"],
                &["completed_object_count"],
                &["object_count_excluding_manifest"],
                &["counts", "payload_object_count"],
            ],
        )?;
        let failed_objects = optional_u64(
            summary,
            "failed_objects",
            &[
                &["failed_objects"],
                &["failed_payload_object_count"],
                &["counts", "error_count"],
            ],
        )?
        .or_else(|| array_len(summary, &["errors"]))
        .unwrap_or(0);
        let skipped_objects = optional_u64(
            summary,
            "skipped_objects",
            &[&["skipped_objects"], &["skipped_payload_object_count"]],
        )?
        .unwrap_or(0);
        let planned_objects = optional_u64(
            summary,
            "planned_objects",
            &[
                &["planned_objects"],
                &["planned_payload_object_count"],
                &["planned_object_count"],
                &["counts", "planned_payload_object_count"],
            ],
        )?
        .unwrap_or_else(|| {
            completed_objects
                .saturating_add(failed_objects)
                .saturating_add(skipped_objects)
        });
        let completed_bytes = required_u64(
            summary,
            "completed_bytes",
            &[
                &["accepted_bytes"],
                &["completed_bytes"],
                &["completed_payload_bytes"],
                &["bytes_excluding_manifest"],
                &["counts", "payload_bytes"],
            ],
        )?;
        let selector_scope_violations = optional_u64(
            summary,
            "selector_scope_violations",
            &[
                &["selector_scope_violations"],
                &["selector_scope", "selector_scope_violations"],
            ],
        )?
        .or_else(|| {
            array_len(
                summary,
                &["selector_scope", "payload_selector_scope_violations"],
            )
        })
        .unwrap_or(0);
        let gap_policy_id = optional_string(summary, &[&["gap_policy_id"]]);

        Ok(Self {
            manifest_id,
            source_binding,
            source_proof_id,
            source_proof_version,
            source_proof_status,
            write_mode,
            canonical_s3_write,
            planned_objects,
            completed_objects,
            failed_objects,
            skipped_objects,
            completed_bytes,
            selector_scope_violations,
            gap_policy_id,
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BackfillPhysicalInventory {
    pub inventory_id: String,
    pub object_count: u64,
    pub byte_count: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BackfillCoverageRecord {
    pub record_id: String,
    pub status: BackfillCoverageStatus,
    pub source_binding: Option<String>,
    pub source_proof_id: Option<String>,
    pub source_proof_version: Option<u32>,
    pub canonical_ready: bool,
    pub accepted_objects: u64,
    pub accepted_bytes: u64,
    pub skipped_objects: u64,
    pub physical_only_objects: u64,
    pub physical_only_bytes: u64,
    pub blocking_issues: Vec<BackfillCoverageIssue>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BackfillCoverageSummary {
    pub total_records: u64,
    pub accepted_records: u64,
    pub accepted_with_gaps_records: u64,
    pub rejected_records: u64,
    pub physical_only_records: u64,
    pub canonical_ready_records: u64,
    pub accepted_objects: u64,
    pub accepted_bytes: u64,
    pub skipped_objects: u64,
    pub physical_only_objects: u64,
    pub physical_only_bytes: u64,
    pub blocking_issue_count: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BackfillCoverageLedger {
    pub schema_version: String,
    pub ledger_id: String,
    pub records: Vec<BackfillCoverageRecord>,
    pub summary: BackfillCoverageSummary,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BackfillCoverageLedgerArtifact {
    pub path: PathBuf,
    pub content_hash: String,
    pub bytes: u64,
    pub record_count: u64,
}

impl BackfillCoverageLedger {
    pub fn from_manifest_json_summaries(
        ledger_id: impl Into<String>,
        manifest_summaries: Vec<BackfillCoverageManifestJson>,
        inventories: Vec<BackfillPhysicalInventory>,
    ) -> Result<Self, BackfillCoverageLedgerError> {
        let manifests = manifest_summaries
            .into_iter()
            .map(|input| {
                let BackfillCoverageManifestJson {
                    manifest_uri,
                    summary,
                    source_proof_status,
                } = input;
                BackfillCoverageManifestEvidence::from_manifest_json(&summary, source_proof_status)
                    .map_err(|source| BackfillCoverageLedgerError::ParseManifest {
                        manifest_uri,
                        source,
                    })
            })
            .collect::<Result<Vec<_>, _>>()?;
        Self::from_evidence(ledger_id, manifests, inventories)
    }

    pub fn from_evidence(
        ledger_id: impl Into<String>,
        manifests: Vec<BackfillCoverageManifestEvidence>,
        inventories: Vec<BackfillPhysicalInventory>,
    ) -> Result<Self, BackfillCoverageLedgerError> {
        let ledger_id = ledger_id.into();
        if ledger_id.trim().is_empty() {
            return Err(BackfillCoverageLedgerError::EmptyLedgerId);
        }

        let mut inventory_by_id = BTreeMap::new();
        for inventory in inventories {
            let inventory_id = inventory.inventory_id.clone();
            if inventory_by_id
                .insert(inventory_id.clone(), inventory)
                .is_some()
            {
                return Err(BackfillCoverageLedgerError::DuplicateInventoryId(
                    inventory_id,
                ));
            }
        }

        let mut manifest_ids = BTreeMap::new();
        let mut records = Vec::new();
        for manifest in manifests {
            if manifest_ids
                .insert(manifest.manifest_id.clone(), ())
                .is_some()
            {
                return Err(BackfillCoverageLedgerError::DuplicateManifestId(
                    manifest.manifest_id,
                ));
            }
            let inventory = inventory_by_id.remove(&manifest.manifest_id);
            records.push(classify_manifest_coverage(&manifest, inventory.as_ref()));
        }
        for inventory in inventory_by_id.into_values() {
            records.push(classify_physical_inventory(&inventory));
        }

        let summary = BackfillCoverageSummary::from_records(&records);
        Ok(Self {
            schema_version: BACKFILL_COVERAGE_LEDGER_SCHEMA_VERSION.to_string(),
            ledger_id,
            records,
            summary,
        })
    }

    pub fn content_hash(&self) -> Result<String, BackfillCoverageLedgerError> {
        let bytes = serde_json::to_vec(self)
            .map_err(|error| BackfillCoverageLedgerError::Serialize(error.to_string()))?;
        let mut hasher = Sha256::new();
        hasher.update(bytes);
        Ok(hex::encode(hasher.finalize()))
    }
}

pub fn write_coverage_ledger_artifact(
    output_dir: &Path,
    ledger: &BackfillCoverageLedger,
) -> Result<BackfillCoverageLedgerArtifact, BackfillCoverageWriteError> {
    fs::create_dir_all(output_dir).map_err(|error| BackfillCoverageWriteError::CreateDir {
        path: output_dir.display().to_string(),
        error: error.to_string(),
    })?;

    let path = output_dir.join(BACKFILL_COVERAGE_LEDGER_FILE);
    let bytes = serde_json::to_vec_pretty(ledger)
        .map_err(|error| BackfillCoverageWriteError::Serialize(error.to_string()))?;
    if path.exists() {
        let existing =
            fs::read(&path).map_err(|error| BackfillCoverageWriteError::ReadExisting {
                path: path.display().to_string(),
                error: error.to_string(),
            })?;
        if existing != bytes {
            return Err(BackfillCoverageWriteError::ExistingArtifactMismatch {
                path: path.display().to_string(),
            });
        }
    } else {
        fs::write(&path, &bytes).map_err(|error| BackfillCoverageWriteError::Write {
            path: path.display().to_string(),
            error: error.to_string(),
        })?;
    }

    let content_hash = ledger
        .content_hash()
        .map_err(|error| BackfillCoverageWriteError::Serialize(error.to_string()))?;
    Ok(BackfillCoverageLedgerArtifact {
        path,
        content_hash,
        bytes: bytes.len() as u64,
        record_count: ledger.records.len() as u64,
    })
}

pub fn write_coverage_ledger_artifact_from_manifest_files(
    output_dir: &Path,
    ledger_id: impl Into<String>,
    manifest_files: Vec<BackfillCoverageManifestFile>,
    inventories: Vec<BackfillPhysicalInventory>,
) -> Result<BackfillCoverageLedgerArtifact, BackfillCoverageManifestFileError> {
    let manifest_summaries = manifest_files
        .into_iter()
        .map(|input| {
            let BackfillCoverageManifestFile {
                manifest_uri,
                path,
                source_proof_status,
            } = input;
            let path_display = path.display().to_string();
            let bytes = fs::read(&path).map_err(|error| {
                BackfillCoverageManifestFileError::ReadManifest {
                    manifest_uri: manifest_uri.clone(),
                    path: path_display.clone(),
                    error: error.to_string(),
                }
            })?;
            let summary = serde_json::from_slice(&bytes).map_err(|error| {
                BackfillCoverageManifestFileError::ParseManifestJson {
                    manifest_uri: manifest_uri.clone(),
                    path: path_display,
                    error: error.to_string(),
                }
            })?;
            Ok(BackfillCoverageManifestJson {
                manifest_uri,
                summary,
                source_proof_status,
            })
        })
        .collect::<Result<Vec<_>, _>>()?;
    let ledger = BackfillCoverageLedger::from_manifest_json_summaries(
        ledger_id,
        manifest_summaries,
        inventories,
    )
    .map_err(BackfillCoverageManifestFileError::BuildLedger)?;
    write_coverage_ledger_artifact(output_dir, &ledger)
        .map_err(BackfillCoverageManifestFileError::WriteArtifact)
}

impl BackfillCoverageSummary {
    fn from_records(records: &[BackfillCoverageRecord]) -> Self {
        let mut summary = Self {
            total_records: records.len() as u64,
            accepted_records: 0,
            accepted_with_gaps_records: 0,
            rejected_records: 0,
            physical_only_records: 0,
            canonical_ready_records: 0,
            accepted_objects: 0,
            accepted_bytes: 0,
            skipped_objects: 0,
            physical_only_objects: 0,
            physical_only_bytes: 0,
            blocking_issue_count: 0,
        };

        for record in records {
            match record.status {
                BackfillCoverageStatus::Accepted => summary.accepted_records += 1,
                BackfillCoverageStatus::AcceptedWithGaps => {
                    summary.accepted_with_gaps_records += 1;
                }
                BackfillCoverageStatus::Rejected => summary.rejected_records += 1,
                BackfillCoverageStatus::PhysicalOnly => summary.physical_only_records += 1,
            }
            if record.canonical_ready {
                summary.canonical_ready_records += 1;
            }
            summary.accepted_objects = summary
                .accepted_objects
                .saturating_add(record.accepted_objects);
            summary.accepted_bytes = summary.accepted_bytes.saturating_add(record.accepted_bytes);
            summary.skipped_objects = summary
                .skipped_objects
                .saturating_add(record.skipped_objects);
            summary.physical_only_objects = summary
                .physical_only_objects
                .saturating_add(record.physical_only_objects);
            summary.physical_only_bytes = summary
                .physical_only_bytes
                .saturating_add(record.physical_only_bytes);
            summary.blocking_issue_count = summary
                .blocking_issue_count
                .saturating_add(record.blocking_issues.len() as u64);
        }

        summary
    }
}

pub fn classify_physical_inventory(
    inventory: &BackfillPhysicalInventory,
) -> BackfillCoverageRecord {
    BackfillCoverageRecord {
        record_id: inventory.inventory_id.clone(),
        status: BackfillCoverageStatus::PhysicalOnly,
        source_binding: None,
        source_proof_id: None,
        source_proof_version: None,
        canonical_ready: false,
        accepted_objects: 0,
        accepted_bytes: 0,
        skipped_objects: 0,
        physical_only_objects: inventory.object_count,
        physical_only_bytes: inventory.byte_count,
        blocking_issues: vec![BackfillCoverageIssue::MissingManifest],
    }
}

pub fn classify_manifest_coverage(
    manifest: &BackfillCoverageManifestEvidence,
    inventory: Option<&BackfillPhysicalInventory>,
) -> BackfillCoverageRecord {
    let blocking_issues = blocking_issues_for(manifest);
    let accepted = blocking_issues.is_empty();
    let status = if !accepted {
        BackfillCoverageStatus::Rejected
    } else if manifest.skipped_objects > 0 {
        BackfillCoverageStatus::AcceptedWithGaps
    } else {
        BackfillCoverageStatus::Accepted
    };
    let (physical_only_objects, physical_only_bytes) =
        physical_only_delta(manifest, inventory, accepted);

    BackfillCoverageRecord {
        record_id: manifest.manifest_id.clone(),
        status,
        source_binding: Some(manifest.source_binding.clone()),
        source_proof_id: manifest.source_proof_id.clone(),
        source_proof_version: manifest.source_proof_version,
        canonical_ready: accepted
            && manifest.write_mode == BackfillWriteMode::CanonicalS3
            && manifest.canonical_s3_write,
        accepted_objects: if accepted {
            manifest.completed_objects
        } else {
            0
        },
        accepted_bytes: if accepted {
            manifest.completed_bytes
        } else {
            0
        },
        skipped_objects: if accepted {
            manifest.skipped_objects
        } else {
            0
        },
        physical_only_objects,
        physical_only_bytes,
        blocking_issues,
    }
}

fn blocking_issues_for(manifest: &BackfillCoverageManifestEvidence) -> Vec<BackfillCoverageIssue> {
    let mut issues = Vec::new();

    if manifest.manifest_id.trim().is_empty() {
        issues.push(BackfillCoverageIssue::EmptyManifestId);
    }
    if manifest.source_binding.trim().is_empty() {
        issues.push(BackfillCoverageIssue::EmptySourceBinding);
    }
    if manifest
        .source_proof_id
        .as_deref()
        .is_none_or(|value| value.trim().is_empty())
        || manifest.source_proof_version.is_none()
        || manifest.source_proof_status.is_none()
    {
        issues.push(BackfillCoverageIssue::MissingSourceProof);
    } else if manifest.source_proof_status != Some(SourceProofStatus::Accepted) {
        issues.push(BackfillCoverageIssue::SourceProofNotAccepted);
    }
    if manifest.planned_objects == 0 {
        issues.push(BackfillCoverageIssue::PlannedObjectsNotPositive);
    }
    if manifest.completed_objects == 0 {
        issues.push(BackfillCoverageIssue::CompletedObjectsNotPositive);
    }
    if manifest.completed_bytes == 0 {
        issues.push(BackfillCoverageIssue::CompletedBytesNotPositive);
    }
    if manifest.failed_objects != 0 {
        issues.push(BackfillCoverageIssue::FailedObjectsPresent);
    }
    if manifest.selector_scope_violations != 0 {
        issues.push(BackfillCoverageIssue::SelectorScopeViolationsPresent);
    }
    let accounted = manifest
        .completed_objects
        .checked_add(manifest.failed_objects)
        .and_then(|value| value.checked_add(manifest.skipped_objects));
    if accounted != Some(manifest.planned_objects) {
        issues.push(BackfillCoverageIssue::PlannedObjectsAccountingMismatch);
    }
    if manifest.skipped_objects != 0
        && manifest
            .gap_policy_id
            .as_deref()
            .is_none_or(|value| value.trim().is_empty())
    {
        issues.push(BackfillCoverageIssue::SkippedObjectsWithoutGapPolicy);
    }

    issues
}

fn physical_only_delta(
    manifest: &BackfillCoverageManifestEvidence,
    inventory: Option<&BackfillPhysicalInventory>,
    accepted: bool,
) -> (u64, u64) {
    let Some(inventory) = inventory else {
        return (0, 0);
    };
    if !accepted {
        return (inventory.object_count, inventory.byte_count);
    }
    (
        inventory
            .object_count
            .saturating_sub(manifest.completed_objects),
        inventory
            .byte_count
            .saturating_sub(manifest.completed_bytes),
    )
}

fn parse_write_mode(value: &str) -> Result<BackfillWriteMode, BackfillCoverageParseError> {
    match value {
        "dry_run" => Ok(BackfillWriteMode::DryRun),
        "local_staging" => Ok(BackfillWriteMode::LocalStaging),
        "s3_staging" | "s3_staging_only" => Ok(BackfillWriteMode::S3Staging),
        "canonical_s3" => Ok(BackfillWriteMode::CanonicalS3),
        other => Err(BackfillCoverageParseError::UnknownWriteMode(
            other.to_string(),
        )),
    }
}

fn required_string(
    root: &Value,
    field: &'static str,
    paths: &[&[&str]],
) -> Result<String, BackfillCoverageParseError> {
    optional_string(root, paths).ok_or(BackfillCoverageParseError::MissingField(field))
}

fn optional_string(root: &Value, paths: &[&[&str]]) -> Option<String> {
    paths
        .iter()
        .find_map(|path| match value_at_path(root, path) {
            Some(Value::String(value)) => Some(value.clone()),
            _ => None,
        })
}

fn required_u64(
    root: &Value,
    field: &'static str,
    paths: &[&[&str]],
) -> Result<u64, BackfillCoverageParseError> {
    optional_u64(root, field, paths)?.ok_or(BackfillCoverageParseError::MissingField(field))
}

fn optional_u32(
    root: &Value,
    field: &'static str,
    paths: &[&[&str]],
) -> Result<Option<u32>, BackfillCoverageParseError> {
    optional_u64(root, field, paths)?
        .map(|value| {
            u32::try_from(value).map_err(|_| BackfillCoverageParseError::InvalidField {
                field,
                value: value.to_string(),
            })
        })
        .transpose()
}

fn optional_u64(
    root: &Value,
    field: &'static str,
    paths: &[&[&str]],
) -> Result<Option<u64>, BackfillCoverageParseError> {
    paths
        .iter()
        .find_map(|path| value_at_path(root, path))
        .map_or(Ok(None), |value| parse_u64_field(field, value).map(Some))
}

fn optional_bool(
    root: &Value,
    field: &'static str,
    paths: &[&[&str]],
) -> Result<Option<bool>, BackfillCoverageParseError> {
    paths
        .iter()
        .find_map(|path| value_at_path(root, path))
        .map_or(Ok(None), |value| {
            value
                .as_bool()
                .ok_or_else(|| BackfillCoverageParseError::InvalidField {
                    field,
                    value: value.to_string(),
                })
                .map(Some)
        })
}

fn parse_u64_field(field: &'static str, value: &Value) -> Result<u64, BackfillCoverageParseError> {
    match value {
        Value::Number(number) => {
            number
                .as_u64()
                .ok_or_else(|| BackfillCoverageParseError::InvalidField {
                    field,
                    value: value.to_string(),
                })
        }
        Value::String(raw) => {
            raw.parse::<u64>()
                .map_err(|_| BackfillCoverageParseError::InvalidField {
                    field,
                    value: raw.clone(),
                })
        }
        _ => Err(BackfillCoverageParseError::InvalidField {
            field,
            value: value.to_string(),
        }),
    }
}

fn array_len(root: &Value, path: &[&str]) -> Option<u64> {
    value_at_path(root, path)
        .and_then(Value::as_array)
        .map(|values| values.len() as u64)
}

fn value_at_path<'a>(root: &'a Value, path: &[&str]) -> Option<&'a Value> {
    let mut current = root;
    for segment in path {
        current = current.get(*segment)?;
    }
    Some(current)
}
