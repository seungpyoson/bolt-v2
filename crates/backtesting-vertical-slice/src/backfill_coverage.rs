//! Run-level backfill coverage ledger.
//!
//! This is the pre-normalization gate for staged backfill runs. It classifies
//! normalized manifest facts and optional physical S3 inventory summaries before
//! any payload download, canonical write, NT catalog projection, or backtest.

use serde::{Deserialize, Serialize};

use crate::source_proof::SourceProofStatus;

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
