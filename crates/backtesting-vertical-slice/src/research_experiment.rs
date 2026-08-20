//! Strict experiment authority, identity, and immutable registration boundary.
//!
//! This module never queries a data provider and never runs replay. It parses
//! one Bolt TOML authority, authenticates production mutations with AWS STS,
//! and registers immutable artifacts through the existing S3/Artifact Index
//! path with credentials resolved only from AWS SSM.

use std::{
    collections::{BTreeMap, BTreeSet},
    fmt, fs,
    path::Path,
    str::FromStr,
};

use aws_config::BehaviorVersion;
use chrono::{DateTime, Utc};
use object_store::{ObjectStore, ObjectStoreExt};
use rust_decimal::Decimal;
use serde::{Deserialize, Deserializer, Serialize, Serializer, de};

use crate::{
    artifact_index::LifecycleState,
    artifact_store::{
        ArtifactIndexCommitPlan as StoreCommitPlan, ArtifactIndexCommitState as StoreCommitState,
        ArtifactIndexEvent as StoreIndexEvent, ArtifactIndexPointerConflict, ArtifactIndexSnapshot,
        ArtifactIndexWriteAuthority, ArtifactIndexWriter, ArtifactKind as StoreArtifactKind,
        ArtifactLifecycleState as StoreLifecycleState, ArtifactLineageRef as StoreLineageRef,
        ArtifactStorageProfile, ArtifactStoreConfig, CreateOnlyArtifactWriter,
        ResolvedArtifactRoot, S3ArtifactStoreCredentials,
    },
    hashing::{is_lowercase_sha256_hex, sha256_hex},
    source_proof::VerifiedRegisteredSourceEvidence,
};

pub const EXPERIMENT_SCHEMA_VERSION: &str = "pump-research-experiment.v1";
pub const CANONICALIZATION_VERSION: &str = "pump-research-canonical-json.v1";
pub const ARTIFACT_SCHEMA_VERSION: &str = "pump-research-artifact.v1";

#[derive(Debug)]
pub enum ExperimentError {
    Io(String),
    Parse(String),
    Validation {
        field: &'static str,
        message: String,
    },
    IdentityUnavailable,
    UnauthorizedPrincipal,
    DirtyArtifact,
    StaleParent,
    Registration(String),
}

impl fmt::Display for ExperimentError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io(message) => write!(f, "experiment definition I/O failed: {message}"),
            Self::Parse(message) => write!(f, "experiment definition parse failed: {message}"),
            Self::Validation { field, message } => {
                write!(f, "invalid experiment field {field}: {message}")
            }
            Self::IdentityUnavailable => write!(f, "AWS STS caller identity is unavailable"),
            Self::UnauthorizedPrincipal => write!(
                f,
                "authenticated principal does not match an authorized TOML role"
            ),
            Self::DirtyArtifact => write!(f, "artifact bytes differ from the registration plan"),
            Self::StaleParent => write!(f, "experiment parent pointer is stale"),
            Self::Registration(message) => write!(f, "experiment registration failed: {message}"),
        }
    }
}

impl std::error::Error for ExperimentError {}

impl ExperimentError {
    #[must_use]
    pub const fn reason_code(&self) -> &'static str {
        match self {
            Self::Io(_) => "io_failed",
            Self::Parse(_) => "parse_failed",
            Self::Validation { .. } => "validation_failed",
            Self::IdentityUnavailable => "identity_unavailable",
            Self::UnauthorizedPrincipal => "unauthorized_principal",
            Self::DirtyArtifact => "dirty_artifact",
            Self::StaleParent => "stale_parent",
            Self::Registration(_) => "registration_failed",
        }
    }
}

fn invalid(field: &'static str, message: impl Into<String>) -> ExperimentError {
    ExperimentError::Validation {
        field,
        message: message.into(),
    }
}

#[derive(Clone, Copy)]
enum ValidationMode {
    Production,
    #[cfg(test)]
    Fixture,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ExperimentState {
    Draft,
    GenesisCommitted,
    DiscoveryCommitted,
    DiscoveryReleased,
    ConfirmationCommitted,
    ConfirmationReleased,
    EnrichmentCommitted,
    ProviderSelectionCommitted,
    MechanismReleased,
    Exploratory,
    Invalidated,
}

impl ExperimentState {
    fn allows_transition_to(self, next: Self) -> bool {
        matches!((self, next), (Self::Draft, Self::Invalidated))
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ExperimentPurpose {
    Exploratory,
    Confirmatory,
    Reproduction,
    Audit,
    MechanismStudy,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum HashAlgorithm {
    Sha256,
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LineageRef {
    pub artifact_kind: StoreArtifactKind,
    pub artifact_id: String,
    pub artifact_version: Option<u32>,
    pub content_hash: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ExperimentVersion {
    pub experiment_id: String,
    pub version_sequence: u32,
    pub parent_version_id: Option<String>,
    pub parent_content_hash: Option<String>,
    pub schema_version: String,
    pub canonicalization_version: String,
    pub hash_algorithm: HashAlgorithm,
    #[serde(skip_serializing)]
    pub created_at: String,
    pub append_role: String,
    pub purpose: ExperimentPurpose,
    pub state: ExperimentState,
    pub lineage_refs: Vec<LineageRef>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RolePurpose {
    Ingestion,
    Disclosure,
    CanonicalEvaluation,
    VerificationReplay,
    Custody,
    ExperimentDecision,
    GovernanceApproval,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PrincipalAuthority {
    AwsSts,
    TestFixture,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RoleBinding {
    pub role_id: String,
    pub purpose: RolePurpose,
    pub authority: PrincipalAuthority,
    pub account_id: String,
    pub principal_arn_prefix: String,
    pub user_id_prefix: String,
    pub credential_scope_ref: String,
    pub can_append_versions: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RoleSeparation {
    pub left_role: String,
    pub right_role: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RolePolicy {
    pub bindings: Vec<RoleBinding>,
    pub required_separations: Vec<RoleSeparation>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RosterStatus {
    EligibleObserved,
    KnownIneligible,
    KnownInsufficientCoverage,
    ExistenceOrCoverageUnknown,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RosterCompleteness {
    ProvenComplete,
    EnumeratedIncomplete,
    Unknown,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum TimeUnitGrain {
    UtcDay,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum OuterRosterRule {
    UnionOfAdmittedVintageInventories,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ReconciliationRule {
    IdentityFirstSourcePrecedenceV1,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum GeneralizationScope {
    EnumeratedRosterWithinConfiguredFrame,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct InventorySourceBinding {
    pub source_ref: String,
    pub source_entry_id: String,
    pub source_entry_version: u32,
    pub source_entry_content_hash: String,
    pub manifest_hash: String,
    pub coverage_hash: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TargetFramePolicy {
    pub frame_id: String,
    pub venue_keys: Vec<String>,
    pub market_family_keys: Vec<String>,
    pub start_time: String,
    pub end_time: String,
    pub time_unit_grain: TimeUnitGrain,
    pub outer_roster_rule: OuterRosterRule,
    pub roster_vintage: String,
    pub inventory_source_refs: Vec<InventorySourceBinding>,
    pub reconciliation_rule: ReconciliationRule,
    pub status_precedence: Vec<RosterStatus>,
    pub generalization_scope: GeneralizationScope,
}

pub const ROSTER_MANIFEST_SCHEMA_VERSION: &str = "pump-research-roster-manifest.v1";

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TimeInterval {
    pub start_time: String,
    pub end_time: String,
}

impl TimeInterval {
    fn parsed(
        &self,
        field: &'static str,
    ) -> Result<(DateTime<Utc>, DateTime<Utc>), ExperimentError> {
        let start = parse_time(field, &self.start_time)?;
        let end = parse_time(field, &self.end_time)?;
        if start >= end {
            return Err(invalid(field, "interval end must follow start"));
        }
        Ok((start, end))
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RosterStatusReason {
    MeasuredCoverage,
    PolicyIneligible,
    InventoryConflict,
    UnverifiedExistence,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CoverageMetrics {
    pub expected_count: u64,
    pub observed_count: u64,
    pub missing_count: u64,
    pub duplicated_count: u64,
    pub interrupted_intervals: Vec<TimeInterval>,
}

impl CoverageMetrics {
    fn validate(
        &self,
        status: RosterStatus,
        reason: RosterStatusReason,
        time_unit: &TimeInterval,
    ) -> Result<(), ExperimentError> {
        if self.observed_count > self.expected_count
            || self.missing_count != self.expected_count - self.observed_count
        {
            return Err(invalid(
                "roster.coverage_metrics",
                "observed and missing counts must exactly partition expected count",
            ));
        }
        let (unit_start, unit_end) = time_unit.parsed("roster.time_unit")?;
        let mut prior_end = None;
        for interval in &self.interrupted_intervals {
            let (start, end) = interval.parsed("roster.coverage_metrics.interrupted_intervals")?;
            if start < unit_start || end > unit_end || prior_end.is_some_and(|prior| start < prior)
            {
                return Err(invalid(
                    "roster.coverage_metrics.interrupted_intervals",
                    "must be ordered, non-overlapping, and contained in the roster unit",
                ));
            }
            prior_end = Some(end);
        }
        let status_consistent = match status {
            RosterStatus::EligibleObserved => {
                reason == RosterStatusReason::MeasuredCoverage
                    && self.observed_count == self.expected_count
                    && self.missing_count == 0
                    && self.duplicated_count == 0
                    && self.interrupted_intervals.is_empty()
            }
            RosterStatus::KnownIneligible => {
                reason == RosterStatusReason::PolicyIneligible && self.observed_count == 0
            }
            RosterStatus::KnownInsufficientCoverage => {
                reason == RosterStatusReason::MeasuredCoverage
                    && (self.missing_count > 0
                        || self.duplicated_count > 0
                        || !self.interrupted_intervals.is_empty())
            }
            RosterStatus::ExistenceOrCoverageUnknown => {
                matches!(
                    reason,
                    RosterStatusReason::UnverifiedExistence | RosterStatusReason::InventoryConflict
                ) && self.missing_count > 0
            }
        };
        if !status_consistent {
            return Err(invalid(
                "roster.coverage_metrics",
                "coverage and reason contradict the declared four-state status",
            ));
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct InventoryObservation {
    pub observation_id: String,
    pub source_ref: String,
    pub frame_id: String,
    pub venue_key: String,
    pub market_family_key: String,
    pub venue_instrument_identity: IdentityNode,
    pub time_unit: TimeInterval,
    pub proposed_status: RosterStatus,
    pub status_reason: RosterStatusReason,
    pub coverage_metrics: CoverageMetrics,
    pub assertion_refs: Vec<String>,
}

pub const INVENTORY_MANIFEST_SCHEMA_VERSION: &str = "pump-research-inventory-manifest.v1";
pub const INVENTORY_COVERAGE_SCHEMA_VERSION: &str = "pump-research-inventory-coverage.v1";

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct InventoryCoverageArtifact {
    pub schema_version: String,
    pub coverage_id: String,
    pub frame_id: String,
    pub source_entry_id: String,
    pub source_entry_version: u32,
    pub source_entry_content_hash: String,
    pub roster_vintage: String,
    pub expected_unit_count: u64,
    pub enumerated_unit_count: u64,
    pub completeness: RosterCompleteness,
    pub evidence_hashes: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct InventoryManifestArtifact {
    pub schema_version: String,
    pub source_ref: String,
    pub frame_id: String,
    pub roster_vintage: String,
    pub coverage: InventoryCoverageArtifact,
    pub observations: Vec<InventoryObservation>,
}

#[derive(Debug, PartialEq, Eq)]
pub struct VerifiedInventoryManifest {
    experiment_content_hash: String,
    target_frame_content_hash: String,
    source_ref: String,
    completeness: RosterCompleteness,
    observations: Vec<InventoryObservation>,
}

#[derive(Debug, PartialEq, Eq)]
pub struct VerifiedInventoryCoverageEvidence {
    experiment_content_hash: String,
    target_frame_content_hash: String,
    source_entry_id: String,
    source_entry_version: u32,
    source_entry_content_hash: String,
    coverage_hash: String,
    expected_unit_count: u64,
    enumerated_roster_hash: String,
    completeness: RosterCompleteness,
    evidence_hashes: Vec<String>,
    verified_assertion_ids: BTreeSet<String>,
    temporal_history_hash: String,
    registered_temporal_head_id: String,
    current_head: bool,
    genesis_commitment_hash: String,
}

impl VerifiedInventoryCoverageEvidence {
    #[cfg(test)]
    fn synthetic(
        experiment_content_hash: &str,
        frame: &TargetFramePolicy,
        binding: &InventorySourceBinding,
        expected_unit_count: u64,
        trusted_observations: &[InventoryObservation],
        completeness: RosterCompleteness,
        evidence_hashes: Vec<String>,
        temporal_evidence: &VerifiedTemporalAssertionHistoryEvidence,
    ) -> Self {
        validate_temporal_assertion_chain(
            &temporal_evidence.registered_history,
            Some(temporal_evidence),
        )
        .expect("synthetic registered temporal history");
        let verified_assertion_ids = trusted_observations
            .iter()
            .flat_map(|observation| observation.assertion_refs.iter().cloned())
            .collect::<BTreeSet<_>>();
        assert!(
            trusted_observations
                .iter()
                .all(|observation| temporal_evidence.verifies_roster_observation(observation))
        );
        Self {
            experiment_content_hash: experiment_content_hash.to_string(),
            target_frame_content_hash: target_frame_content_hash(frame)
                .expect("synthetic target frame"),
            source_entry_id: binding.source_entry_id.clone(),
            source_entry_version: binding.source_entry_version,
            source_entry_content_hash: binding.source_entry_content_hash.clone(),
            coverage_hash: binding.coverage_hash.clone(),
            expected_unit_count,
            enumerated_roster_hash: inventory_roster_key_hash(trusted_observations)
                .expect("synthetic roster keys"),
            completeness,
            evidence_hashes,
            verified_assertion_ids,
            temporal_history_hash: temporal_evidence.history_hash.clone(),
            registered_temporal_head_id: temporal_evidence.registered_head_id.clone(),
            current_head: true,
            genesis_commitment_hash: "a".repeat(64),
        }
    }
}

impl VerifiedRegisteredSourceEvidence {
    /// Loads and verifies inventory bytes only after registered source authority.
    pub fn load_and_verify_inventory_manifest(
        self,
        coverage_evidence: VerifiedInventoryCoverageEvidence,
        experiment_content_hash: &str,
        frame: &TargetFramePolicy,
        binding: &InventorySourceBinding,
        load_manifest: impl FnOnce() -> Result<Vec<u8>, ExperimentError>,
    ) -> Result<VerifiedInventoryManifest, ExperimentError> {
        validate_inventory_manifest_authority(
            experiment_content_hash,
            frame,
            binding,
            &self,
            &coverage_evidence,
        )?;
        let manifest_bytes = load_manifest()?;
        verify_inventory_manifest(
            experiment_content_hash,
            frame,
            binding,
            &manifest_bytes,
            &coverage_evidence,
        )
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RosterConflictReason {
    pub selected_observation_id: String,
    pub rejected_observation_id: String,
    pub selected_status: RosterStatus,
    pub rejected_status: RosterStatus,
    pub selected_reason: RosterStatusReason,
    pub rejected_reason: RosterStatusReason,
    pub selected_coverage_metrics: CoverageMetrics,
    pub rejected_coverage_metrics: CoverageMetrics,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RosterUnit {
    pub roster_unit_id: String,
    pub frame_id: String,
    pub venue_key: String,
    pub market_family_key: String,
    pub venue_instrument_identity: IdentityNode,
    pub time_unit: TimeInterval,
    pub status: RosterStatus,
    pub status_reason: RosterStatusReason,
    pub coverage_metrics: CoverageMetrics,
    pub assertion_refs: Vec<String>,
    pub conflict_reasons: Vec<RosterConflictReason>,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RosterStatusCounts {
    pub eligible_observed: u64,
    pub known_ineligible: u64,
    pub known_insufficient_coverage: u64,
    pub existence_or_coverage_unknown: u64,
}

impl RosterStatusCounts {
    fn record(&mut self, status: RosterStatus) {
        match status {
            RosterStatus::EligibleObserved => self.eligible_observed += 1,
            RosterStatus::KnownIneligible => self.known_ineligible += 1,
            RosterStatus::KnownInsufficientCoverage => {
                self.known_insufficient_coverage += 1;
            }
            RosterStatus::ExistenceOrCoverageUnknown => {
                self.existence_or_coverage_unknown += 1;
            }
        }
    }

    #[must_use]
    pub const fn total(self) -> u64 {
        self.eligible_observed
            + self.known_ineligible
            + self.known_insufficient_coverage
            + self.existence_or_coverage_unknown
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RosterManifest {
    pub schema_version: String,
    pub frame_id: String,
    pub units: Vec<RosterUnit>,
    pub denominator: u64,
    pub status_counts: RosterStatusCounts,
    pub attrition: RosterAttrition,
    pub completeness: RosterCompleteness,
    pub generalization_scope: GeneralizationScope,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RosterAttrition {
    pub starting_denominator: u64,
    pub eligible_observed: u64,
    pub known_ineligible: u64,
    pub known_insufficient_coverage: u64,
    pub existence_or_coverage_unknown: u64,
}

#[derive(Serialize)]
struct RosterUnitIdentity<'a> {
    frame_id: &'a str,
    venue_key: &'a str,
    market_family_key: &'a str,
    venue_instrument_identity_id: &'a str,
    time_unit: &'a TimeInterval,
}

fn verify_inventory_manifest(
    experiment_content_hash: &str,
    frame: &TargetFramePolicy,
    binding: &InventorySourceBinding,
    manifest_bytes: &[u8],
    coverage_evidence: &VerifiedInventoryCoverageEvidence,
) -> Result<VerifiedInventoryManifest, ExperimentError> {
    if sha256_hex(manifest_bytes) != binding.manifest_hash {
        return Err(invalid(
            "target_frame.inventory_source_refs.manifest_hash",
            "does not match the exact inventory manifest bytes",
        ));
    }
    let manifest: InventoryManifestArtifact = serde_json::from_slice(manifest_bytes)
        .map_err(|error| ExperimentError::Parse(error.to_string()))?;
    if manifest.schema_version != INVENTORY_MANIFEST_SCHEMA_VERSION
        || manifest.source_ref != binding.source_ref
        || manifest.frame_id != frame.frame_id
        || manifest.roster_vintage != frame.roster_vintage
        || manifest.coverage.schema_version != INVENTORY_COVERAGE_SCHEMA_VERSION
        || manifest.coverage.frame_id != frame.frame_id
        || manifest.coverage.source_entry_id != binding.source_entry_id
        || manifest.coverage.source_entry_version != binding.source_entry_version
        || manifest.coverage.source_entry_content_hash != binding.source_entry_content_hash
        || manifest.coverage.roster_vintage != frame.roster_vintage
    {
        return Err(invalid(
            "inventory_manifest",
            "manifest identity does not match its hash-bound target-frame binding",
        ));
    }
    for hash_value in [
        binding.source_entry_content_hash.as_str(),
        binding.manifest_hash.as_str(),
        binding.coverage_hash.as_str(),
    ] {
        hash("target_frame.inventory_source_refs", hash_value)?;
    }
    let coverage_hash = inventory_coverage_hash(&manifest.coverage)?;
    if manifest.coverage.coverage_id != format!("coverage-{coverage_hash}")
        || binding.coverage_hash != coverage_hash
        || manifest.coverage.enumerated_unit_count
            != u64::try_from(manifest.observations.len()).map_err(|_| {
                invalid(
                    "inventory_manifest.observations",
                    "observation count does not fit u64",
                )
            })?
        || manifest.coverage.expected_unit_count < manifest.coverage.enumerated_unit_count
        || (manifest.coverage.completeness == RosterCompleteness::ProvenComplete
            && manifest.coverage.expected_unit_count != manifest.coverage.enumerated_unit_count)
        || manifest.coverage.expected_unit_count != coverage_evidence.expected_unit_count
        || manifest.coverage.completeness != coverage_evidence.completeness
    {
        return Err(invalid(
            "inventory_manifest.coverage",
            "coverage proof, counts, or completeness do not match the manifest",
        ));
    }
    unique(
        "inventory_manifest.coverage.evidence_hashes",
        &manifest.coverage.evidence_hashes,
    )?;
    if manifest
        .coverage
        .evidence_hashes
        .iter()
        .any(|value| !is_lowercase_sha256_hex(value))
    {
        return Err(invalid(
            "inventory_manifest.coverage.evidence_hashes",
            "must contain SHA-256 evidence identities",
        ));
    }
    let mut manifest_evidence_hashes = manifest.coverage.evidence_hashes.clone();
    manifest_evidence_hashes.sort();
    let mut verified_evidence_hashes = coverage_evidence.evidence_hashes.clone();
    verified_evidence_hashes.sort();
    if manifest_evidence_hashes != verified_evidence_hashes {
        return Err(invalid(
            "inventory_manifest.coverage.evidence_hashes",
            "do not match the verified registered evidence set",
        ));
    }
    let frame_start = parse_time("target_frame.start_time", &frame.start_time)?;
    let frame_end = parse_time("target_frame.end_time", &frame.end_time)?;
    let mut observation_ids = BTreeSet::new();
    let mut roster_keys = BTreeSet::new();
    for observation in &manifest.observations {
        validate_observation_identity(observation)?;
        if observation.source_ref != binding.source_ref
            || observation.frame_id != frame.frame_id
            || !frame.venue_keys.contains(&observation.venue_key)
            || !frame
                .market_family_keys
                .contains(&observation.market_family_key)
            || !observation_ids.insert(observation.observation_id.as_str())
        {
            return Err(invalid(
                "inventory_manifest.observations",
                "observation identity or scope does not match the bound manifest",
            ));
        }
        let (unit_start, unit_end) = observation.time_unit.parsed("roster.time_unit")?;
        if unit_start < frame_start
            || unit_end > frame_end
            || !roster_keys.insert((
                observation.venue_key.as_str(),
                observation.market_family_key.as_str(),
                observation.venue_instrument_identity.identity_id.as_str(),
                observation.time_unit.clone(),
            ))
        {
            return Err(invalid(
                "inventory_manifest.observations",
                "contains an out-of-frame or duplicate roster key",
            ));
        }
        observation.coverage_metrics.validate(
            observation.proposed_status,
            observation.status_reason,
            &observation.time_unit,
        )?;
        unique("roster.assertion_refs", &observation.assertion_refs)?;
        if observation.assertion_refs.is_empty()
            || observation.assertion_refs.iter().any(|assertion_id| {
                !coverage_evidence
                    .verified_assertion_ids
                    .contains(assertion_id)
            })
        {
            return Err(invalid(
                "roster.assertion_refs",
                "must resolve through the verified coverage evidence",
            ));
        }
    }
    if inventory_roster_key_hash(&manifest.observations)?
        != coverage_evidence.enumerated_roster_hash
    {
        return Err(invalid(
            "inventory_manifest.coverage",
            "enumerated roster does not match the independently verified coverage universe",
        ));
    }
    Ok(VerifiedInventoryManifest {
        experiment_content_hash: experiment_content_hash.to_string(),
        target_frame_content_hash: target_frame_content_hash(frame)?,
        source_ref: manifest.source_ref,
        completeness: manifest.coverage.completeness,
        observations: manifest.observations,
    })
}

fn validate_inventory_manifest_authority(
    experiment_content_hash: &str,
    frame: &TargetFramePolicy,
    binding: &InventorySourceBinding,
    source_evidence: &VerifiedRegisteredSourceEvidence,
    coverage_evidence: &VerifiedInventoryCoverageEvidence,
) -> Result<(), ExperimentError> {
    validate_frame(frame)?;
    hash("experiment_content_hash", experiment_content_hash)?;
    if !source_evidence.matches_registered_source(
        &binding.source_entry_id,
        binding.source_entry_version,
        &binding.source_entry_content_hash,
    ) {
        return Err(invalid(
            "inventory_manifest.source_entry",
            "requires active, current, Genesis-bound registered source evidence",
        ));
    }
    let evidence_observed = parse_time(
        "inventory_manifest.source_evidence.verified_use_time",
        source_evidence.verified_use_time(),
    )?;
    let roster_vintage = parse_time("target_frame.roster_vintage", &frame.roster_vintage)?;
    if roster_vintage > evidence_observed {
        return Err(invalid(
            "inventory_manifest.roster_vintage",
            "cannot follow registered evidence observation time",
        ));
    }
    let frame_hash = target_frame_content_hash(frame)?;
    if coverage_evidence.experiment_content_hash != experiment_content_hash
        || coverage_evidence.target_frame_content_hash != frame_hash
        || coverage_evidence.source_entry_id != binding.source_entry_id
        || coverage_evidence.source_entry_version != binding.source_entry_version
        || coverage_evidence.source_entry_content_hash != binding.source_entry_content_hash
        || coverage_evidence.coverage_hash != binding.coverage_hash
        || !coverage_evidence.current_head
        || !is_lowercase_sha256_hex(&coverage_evidence.genesis_commitment_hash)
        || coverage_evidence.evidence_hashes.is_empty()
        || coverage_evidence
            .evidence_hashes
            .iter()
            .any(|value| !is_lowercase_sha256_hex(value))
        || coverage_evidence.verified_assertion_ids.is_empty()
        || !is_lowercase_sha256_hex(&coverage_evidence.temporal_history_hash)
        || coverage_evidence.registered_temporal_head_id.is_empty()
    {
        return Err(invalid(
            "inventory_manifest.coverage_evidence",
            "must match the current Genesis-bound experiment, frame, source, and coverage",
        ));
    }
    Ok(())
}

fn target_frame_content_hash(frame: &TargetFramePolicy) -> Result<String, ExperimentError> {
    serde_json::to_vec(frame)
        .map(|bytes| sha256_hex(&bytes))
        .map_err(|error| ExperimentError::Registration(error.to_string()))
}

fn inventory_roster_key_hash(
    observations: &[InventoryObservation],
) -> Result<String, ExperimentError> {
    let mut keys = observations
        .iter()
        .map(|observation| {
            (
                observation.venue_key.as_str(),
                observation.market_family_key.as_str(),
                observation.venue_instrument_identity.identity_id.as_str(),
                &observation.time_unit,
            )
        })
        .collect::<Vec<_>>();
    keys.sort();
    serde_json::to_vec(&keys)
        .map(|bytes| sha256_hex(&bytes))
        .map_err(|error| ExperimentError::Registration(error.to_string()))
}

fn inventory_coverage_hash(
    coverage: &InventoryCoverageArtifact,
) -> Result<String, ExperimentError> {
    let mut canonical = coverage.clone();
    canonical.coverage_id.clear();
    canonical.evidence_hashes.sort();
    serde_json::to_vec(&canonical)
        .map(|bytes| sha256_hex(&bytes))
        .map_err(|error| ExperimentError::Registration(error.to_string()))
}

/// Reconciles admitted inventory observations into one deterministic roster R.
///
/// Configured inventory-source precedence is applied first, followed by status
/// precedence and stable observation identity. Every resulting unit contributes
/// exactly once to the released denominator.
pub fn build_roster_manifest(
    experiment_content_hash: &str,
    frame: &TargetFramePolicy,
    manifests: Vec<VerifiedInventoryManifest>,
) -> Result<RosterManifest, ExperimentError> {
    validate_frame(frame)?;
    hash("experiment_content_hash", experiment_content_hash)?;
    if manifests.len() != frame.inventory_source_refs.len() {
        return Err(invalid(
            "roster.inventory_manifests",
            "must contain exactly one verified manifest for every configured source",
        ));
    }
    let frame_start = parse_time("target_frame.start_time", &frame.start_time)?;
    let frame_end = parse_time("target_frame.end_time", &frame.end_time)?;
    let frame_hash = target_frame_content_hash(frame)?;
    let source_precedence = frame
        .inventory_source_refs
        .iter()
        .enumerate()
        .map(|(index, source)| (source.source_ref.as_str(), index))
        .collect::<BTreeMap<_, _>>();
    let status_precedence = frame
        .status_precedence
        .iter()
        .copied()
        .enumerate()
        .map(|(index, status)| (status, index))
        .collect::<BTreeMap<_, _>>();
    type RosterKey = (String, String, IdentityNode, TimeInterval);
    let mut grouped = BTreeMap::<RosterKey, Vec<InventoryObservation>>::new();
    let mut observation_ids = BTreeSet::new();
    let completeness = if manifests
        .iter()
        .any(|manifest| manifest.completeness == RosterCompleteness::Unknown)
    {
        RosterCompleteness::Unknown
    } else if manifests
        .iter()
        .all(|manifest| manifest.completeness == RosterCompleteness::ProvenComplete)
    {
        RosterCompleteness::ProvenComplete
    } else {
        RosterCompleteness::EnumeratedIncomplete
    };
    let mut supplied_sources = BTreeSet::new();
    for manifest in manifests {
        if manifest.experiment_content_hash != experiment_content_hash
            || manifest.target_frame_content_hash != frame_hash
            || !supplied_sources.insert(manifest.source_ref.clone())
            || !source_precedence.contains_key(manifest.source_ref.as_str())
        {
            return Err(invalid(
                "roster.inventory_manifests",
                "contains a duplicate or unconfigured inventory source",
            ));
        }
        for observation in manifest.observations {
            validate_observation_identity(&observation)?;
            for (field, value) in [
                ("roster.observation_id", observation.observation_id.as_str()),
                ("roster.source_ref", observation.source_ref.as_str()),
                (
                    "roster.venue_instrument_identity.identity_id",
                    observation.venue_instrument_identity.identity_id.as_str(),
                ),
            ] {
                required(field, value)?;
            }
            if !observation_ids.insert(observation.observation_id.clone()) {
                return Err(invalid("roster.observation_id", "must be unique"));
            }
            if observation.source_ref != manifest.source_ref
                || observation.frame_id != frame.frame_id
                || !frame.venue_keys.contains(&observation.venue_key)
                || !frame
                    .market_family_keys
                    .contains(&observation.market_family_key)
                || !source_precedence.contains_key(observation.source_ref.as_str())
            {
                return Err(invalid(
                    "roster.observation",
                    "observation lies outside the configured target frame",
                ));
            }
            let (unit_start, unit_end) = observation.time_unit.parsed("roster.time_unit")?;
            if unit_start < frame_start || unit_end > frame_end {
                return Err(invalid(
                    "roster.time_unit",
                    "time unit lies outside the target-frame interval",
                ));
            }
            if matches!(frame.time_unit_grain, TimeUnitGrain::UtcDay)
                && (unit_end - unit_start != chrono::Duration::days(1)
                    || unit_start.time() != chrono::NaiveTime::MIN)
            {
                return Err(invalid(
                    "roster.time_unit",
                    "UTC-day units must be midnight-aligned 24-hour intervals",
                ));
            }
            observation.coverage_metrics.validate(
                observation.proposed_status,
                observation.status_reason,
                &observation.time_unit,
            )?;
            unique("roster.assertion_refs", &observation.assertion_refs)?;
            grouped
                .entry((
                    observation.venue_key.clone(),
                    observation.market_family_key.clone(),
                    observation.venue_instrument_identity.clone(),
                    observation.time_unit.clone(),
                ))
                .or_default()
                .push(observation);
        }
    }
    if grouped.is_empty() {
        return Err(invalid("roster.observations", "must not be empty"));
    }

    let mut units = Vec::with_capacity(grouped.len());
    let mut status_counts = RosterStatusCounts::default();
    for ((venue_key, market_family_key, venue_instrument_identity, time_unit), mut candidates) in
        grouped
    {
        candidates.sort_by(|left, right| {
            (
                source_precedence[left.source_ref.as_str()],
                status_precedence[&left.proposed_status],
                &left.observation_id,
            )
                .cmp(&(
                    source_precedence[right.source_ref.as_str()],
                    status_precedence[&right.proposed_status],
                    &right.observation_id,
                ))
        });
        let selected = candidates
            .first()
            .expect("grouped roster candidates are non-empty");
        let mut assertion_refs = candidates
            .iter()
            .flat_map(|candidate| candidate.assertion_refs.iter().cloned())
            .collect::<Vec<_>>();
        assertion_refs.sort();
        assertion_refs.dedup();
        let conflict_reasons = candidates
            .iter()
            .skip(1)
            .filter(|candidate| {
                candidate.proposed_status != selected.proposed_status
                    || candidate.status_reason != selected.status_reason
                    || candidate.coverage_metrics != selected.coverage_metrics
            })
            .map(|candidate| RosterConflictReason {
                selected_observation_id: selected.observation_id.clone(),
                rejected_observation_id: candidate.observation_id.clone(),
                selected_status: selected.proposed_status,
                rejected_status: candidate.proposed_status,
                selected_reason: selected.status_reason,
                rejected_reason: candidate.status_reason,
                selected_coverage_metrics: selected.coverage_metrics.clone(),
                rejected_coverage_metrics: candidate.coverage_metrics.clone(),
            })
            .collect::<Vec<_>>();
        let identity = RosterUnitIdentity {
            frame_id: &frame.frame_id,
            venue_key: &venue_key,
            market_family_key: &market_family_key,
            venue_instrument_identity_id: &venue_instrument_identity.identity_id,
            time_unit: &time_unit,
        };
        let roster_unit_id = sha256_hex(
            &serde_json::to_vec(&identity)
                .map_err(|error| ExperimentError::Parse(error.to_string()))?,
        );
        status_counts.record(selected.proposed_status);
        units.push(RosterUnit {
            roster_unit_id,
            frame_id: frame.frame_id.clone(),
            venue_key,
            market_family_key,
            venue_instrument_identity,
            time_unit,
            status: selected.proposed_status,
            status_reason: selected.status_reason,
            coverage_metrics: selected.coverage_metrics.clone(),
            assertion_refs,
            conflict_reasons,
        });
    }
    let denominator = u64::try_from(units.len())
        .map_err(|_| invalid("roster.denominator", "does not fit u64"))?;
    if status_counts.total() != denominator {
        return Err(invalid(
            "roster.status_counts",
            "four-state accounting does not equal the denominator",
        ));
    }
    Ok(RosterManifest {
        schema_version: ROSTER_MANIFEST_SCHEMA_VERSION.to_string(),
        frame_id: frame.frame_id.clone(),
        units,
        denominator,
        status_counts,
        attrition: RosterAttrition {
            starting_denominator: denominator,
            eligible_observed: status_counts.eligible_observed,
            known_ineligible: status_counts.known_ineligible,
            known_insufficient_coverage: status_counts.known_insufficient_coverage,
            existence_or_coverage_unknown: status_counts.existence_or_coverage_unknown,
        },
        completeness,
        generalization_scope: frame.generalization_scope,
    })
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum IdentityKind {
    VenueInstrument,
    TokenContract,
    EconomicAsset,
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum IdentityNativeKey {
    VenueInstrument {
        venue_key: String,
        instrument_id: String,
        listing_incarnation: String,
    },
    TokenContract {
        chain_id: String,
        contract_address: String,
    },
    EconomicAsset {
        registry_key: String,
        asset_id: String,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct IdentityNode {
    pub identity_id: String,
    pub native_key: IdentityNativeKey,
}

fn validate_observation_identity(
    observation: &InventoryObservation,
) -> Result<(), ExperimentError> {
    let identity = &observation.venue_instrument_identity;
    validate_identity_native_key(&identity.native_key)?;
    if identity.identity_id != identity_node_id(&identity.native_key) {
        return Err(invalid(
            "roster.venue_instrument_identity.identity_id",
            "must be derived from the typed native identity",
        ));
    }
    match &identity.native_key {
        IdentityNativeKey::VenueInstrument { venue_key, .. }
            if venue_key == &observation.venue_key =>
        {
            Ok(())
        }
        _ => Err(invalid(
            "roster.venue_instrument_identity",
            "must be a venue-instrument incarnation for the observation venue",
        )),
    }
}

impl IdentityNode {
    pub fn new(native_key: IdentityNativeKey) -> Result<Self, ExperimentError> {
        validate_identity_native_key(&native_key)?;
        let identity_id = identity_node_id(&native_key);
        Ok(Self {
            identity_id,
            native_key,
        })
    }

    #[must_use]
    pub const fn identity_kind(&self) -> IdentityKind {
        match &self.native_key {
            IdentityNativeKey::VenueInstrument { .. } => IdentityKind::VenueInstrument,
            IdentityNativeKey::TokenContract { .. } => IdentityKind::TokenContract,
            IdentityNativeKey::EconomicAsset { .. } => IdentityKind::EconomicAsset,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum IdentityMappingStatus {
    Active,
    Superseded,
    Disputed,
    Retracted,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EvidenceConfidence {
    Low,
    Medium,
    High,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum IdentityEvidenceKind {
    VenueMetadata,
    ChainRegistry,
    IssuerDisclosure,
    ArchivalSnapshot,
    TickerLabel,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct IdentityEvidence {
    pub kind: IdentityEvidenceKind,
    pub assertion_id: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "decision", rename_all = "snake_case", deny_unknown_fields)]
pub enum SeriesSpliceRule {
    Allowed {
        transformation: String,
        reason: String,
    },
    Denied {
        reason: String,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct IdentityMapping {
    pub mapping_id: String,
    pub from_identity_id: String,
    pub to_identity_id: String,
    pub valid_time: TimeInterval,
    pub availability_time: String,
    pub retrieval_time: String,
    pub status: IdentityMappingStatus,
    pub confidence: EvidenceConfidence,
    pub evidence: Vec<IdentityEvidence>,
    pub splice_rule: SeriesSpliceRule,
}

impl IdentityMapping {
    #[must_use]
    pub fn evidence_commitment(&self) -> String {
        #[derive(Serialize)]
        struct MappingIdentity<'a> {
            mapping_id: &'a str,
            from_identity_id: &'a str,
            to_identity_id: &'a str,
            valid_time: &'a TimeInterval,
            availability_time: &'a str,
            retrieval_time: &'a str,
            status: IdentityMappingStatus,
            confidence: EvidenceConfidence,
            splice_rule: &'a SeriesSpliceRule,
            evidence_kinds: Vec<IdentityEvidenceKind>,
        }

        let mut evidence_kinds = self
            .evidence
            .iter()
            .map(|evidence| evidence.kind)
            .collect::<Vec<_>>();
        evidence_kinds.sort();
        let identity = MappingIdentity {
            mapping_id: &self.mapping_id,
            from_identity_id: &self.from_identity_id,
            to_identity_id: &self.to_identity_id,
            valid_time: &self.valid_time,
            availability_time: &self.availability_time,
            retrieval_time: &self.retrieval_time,
            status: self.status,
            confidence: self.confidence,
            splice_rule: &self.splice_rule,
            evidence_kinds,
        };
        format!(
            "identity-mapping-{}",
            sha256_hex(
                &serde_json::to_vec(&identity)
                    .expect("typed identity mapping serialization is infallible")
            )
        )
    }
}

pub fn validate_identity_graph(
    nodes: &[IdentityNode],
    mappings: &[IdentityMapping],
    assertions: &[TemporalAssertion],
    registered_evidence: Option<&VerifiedTemporalAssertionHistoryEvidence>,
) -> Result<(), ExperimentError> {
    let mut node_ids = BTreeMap::new();
    let mut native_ids = BTreeSet::new();
    for node in nodes {
        validate_identity_native_key(&node.native_key)?;
        if node.identity_id != identity_node_id(&node.native_key) {
            return Err(invalid(
                "identity.identity_id",
                "must be derived from the typed native identity",
            ));
        }
        if node_ids.insert(node.identity_id.as_str(), node).is_some()
            || !native_ids.insert(node.native_key.clone())
        {
            return Err(invalid("identity", "identity nodes must be unique"));
        }
    }
    let mut mapping_ids = BTreeSet::new();
    let mut active_ranges = BTreeMap::<&str, Vec<(&str, DateTime<Utc>, DateTime<Utc>)>>::new();
    for mapping in mappings {
        required("identity.mapping_id", &mapping.mapping_id)?;
        if !mapping_ids.insert(mapping.mapping_id.as_str())
            || mapping.from_identity_id == mapping.to_identity_id
        {
            return Err(invalid(
                "identity.mapping",
                "mapping identity must be unique",
            ));
        }
        let Some(from) = node_ids.get(mapping.from_identity_id.as_str()) else {
            return Err(invalid("identity.from_identity_id", "unknown identity"));
        };
        let Some(to) = node_ids.get(mapping.to_identity_id.as_str()) else {
            return Err(invalid("identity.to_identity_id", "unknown identity"));
        };
        if from.identity_kind() == to.identity_kind() {
            return Err(invalid(
                "identity.mapping",
                "mapping must connect distinct identity kinds",
            ));
        }
        let (valid_from, valid_until) = mapping.valid_time.parsed("identity.valid_time")?;
        let availability = parse_time("identity.availability_time", &mapping.availability_time)?;
        let retrieval = parse_time("identity.retrieval_time", &mapping.retrieval_time)?;
        if availability > retrieval {
            return Err(invalid(
                "identity.retrieval_time",
                "must not precede evidenced availability",
            ));
        }
        if mapping.status == IdentityMappingStatus::Active {
            let ranges = active_ranges
                .entry(mapping.from_identity_id.as_str())
                .or_default();
            if ranges.iter().any(|(existing_target, start, end)| {
                *existing_target != mapping.to_identity_id.as_str()
                    && valid_from < *end
                    && *start < valid_until
            }) {
                return Err(invalid(
                    "identity.valid_time",
                    "conflicting active identity mappings overlap",
                ));
            }
            ranges.push((mapping.to_identity_id.as_str(), valid_from, valid_until));
        }
        if mapping.evidence.is_empty()
            || mapping
                .evidence
                .iter()
                .all(|evidence| evidence.kind == IdentityEvidenceKind::TickerLabel)
        {
            return Err(invalid(
                "identity.evidence",
                "ticker-only identity joins are forbidden",
            ));
        }
        let mut assertion_ids = BTreeSet::new();
        for evidence in &mapping.evidence {
            required("identity.assertion_id", &evidence.assertion_id)?;
            if !assertion_ids.insert(evidence.assertion_id.as_str()) {
                return Err(invalid("identity.evidence", "duplicate assertion identity"));
            }
        }
        match &mapping.splice_rule {
            SeriesSpliceRule::Allowed {
                transformation,
                reason,
            } => {
                required("identity.splice_rule.transformation", transformation)?;
                required("identity.splice_rule.reason", reason)?;
            }
            SeriesSpliceRule::Denied { reason } => {
                required("identity.splice_rule.reason", reason)?;
            }
        }
    }
    validate_temporal_assertion_chain(assertions, registered_evidence)?;
    let assertion_by_id = assertions
        .iter()
        .map(|assertion| (assertion.assertion_id.as_str(), assertion))
        .collect::<BTreeMap<_, _>>();
    for mapping in mappings {
        for evidence in &mapping.evidence {
            let assertion = assertion_by_id
                .get(evidence.assertion_id.as_str())
                .ok_or_else(|| {
                    invalid(
                        "identity.evidence.assertion_id",
                        "is not in the registered temporal history",
                    )
                })?;
            if assertion.subject_id != mapping.mapping_id
                || assertion.predicate != AssertionPredicate::IdentityMappingEvidence
                || assertion.valid_time != mapping.valid_time
                || assertion.retrieval_time != mapping.retrieval_time
                || assertion
                    .availability_time
                    .as_deref()
                    .unwrap_or(assertion.retrieval_time.as_str())
                    != mapping.availability_time
                || !matches!(
                    &assertion.value,
                    AssertionValue::IdentityMappingEvidence {
                        mapping_commitment,
                        evidence_kind,
                    } if mapping_commitment == &mapping.evidence_commitment()
                        && *evidence_kind == evidence.kind
                )
            {
                return Err(invalid(
                    "identity.evidence",
                    "assertion identity and clocks must bind the mapping",
                ));
            }
            validate_temporal_assertion_claim_use(
                assertions,
                registered_evidence,
                &evidence.assertion_id,
                ClaimUse::RetrospectiveDescriptive,
            )?;
        }
    }
    Ok(())
}

fn validate_identity_native_key(key: &IdentityNativeKey) -> Result<(), ExperimentError> {
    match key {
        IdentityNativeKey::VenueInstrument {
            venue_key,
            instrument_id,
            listing_incarnation,
        } => {
            required("identity.native_key.venue_key", venue_key)?;
            required("identity.native_key.instrument_id", instrument_id)?;
            required(
                "identity.native_key.listing_incarnation",
                listing_incarnation,
            )?;
        }
        IdentityNativeKey::TokenContract {
            chain_id,
            contract_address,
        } => {
            required("identity.native_key.chain_id", chain_id)?;
            required("identity.native_key.contract_address", contract_address)?;
            if contract_address.len() < 16
                || contract_address
                    .chars()
                    .all(|character| character.is_ascii_alphabetic())
            {
                return Err(invalid(
                    "identity.native_key.contract_address",
                    "must be a chain-native contract address, not a ticker label",
                ));
            }
        }
        IdentityNativeKey::EconomicAsset {
            registry_key,
            asset_id,
        } => {
            required("identity.native_key.registry_key", registry_key)?;
            required("identity.native_key.asset_id", asset_id)?;
        }
    }
    Ok(())
}

fn identity_node_id(key: &IdentityNativeKey) -> String {
    format!(
        "identity-{}",
        sha256_hex(&serde_json::to_vec(key).expect("typed identity serialization is infallible"))
    )
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum AssertionValue {
    IdentityMappingEvidence {
        mapping_commitment: String,
        evidence_kind: IdentityEvidenceKind,
    },
    Text {
        value: String,
    },
    Integer {
        value: i64,
    },
    Boolean {
        value: bool,
    },
    Timestamp {
        value: String,
    },
    RosterStatus {
        status: RosterStatus,
        reason: RosterStatusReason,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AssertionPredicate {
    IdentityMappingEvidence,
    ListingStatus,
    RosterStatus,
    Symbol,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AvailabilityStatus {
    ArchivallyAttested,
    RetrievalTimeAttested,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AssertionState {
    Active,
    Corrected,
    Retracted,
    Disputed,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ClaimUse {
    RetrospectiveDescriptive,
    ContemporaneousAvailability,
    Predictive,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TemporalAssertion {
    pub assertion_id: String,
    pub subject_id: String,
    pub predicate: AssertionPredicate,
    pub value: AssertionValue,
    pub valid_time: TimeInterval,
    pub publication_time: Option<String>,
    pub availability_time: Option<String>,
    pub retrieval_time: String,
    pub availability_status: AvailabilityStatus,
    pub revision_of: Option<String>,
    pub assertion_state: AssertionState,
    pub evidence_refs: Vec<String>,
}

impl TemporalAssertion {
    #[must_use]
    pub fn derived_id(&self) -> String {
        #[derive(Serialize)]
        struct AssertionIdentity<'a> {
            subject_id: &'a str,
            predicate: AssertionPredicate,
            value: &'a AssertionValue,
            valid_time: &'a TimeInterval,
            publication_time: &'a Option<String>,
            availability_time: &'a Option<String>,
            retrieval_time: &'a str,
            availability_status: AvailabilityStatus,
            revision_of: &'a Option<String>,
            assertion_state: AssertionState,
            evidence_refs: &'a [String],
        }

        let mut evidence_refs = self.evidence_refs.clone();
        evidence_refs.sort();
        let identity = AssertionIdentity {
            subject_id: &self.subject_id,
            predicate: self.predicate,
            value: &self.value,
            valid_time: &self.valid_time,
            publication_time: &self.publication_time,
            availability_time: &self.availability_time,
            retrieval_time: &self.retrieval_time,
            availability_status: self.availability_status,
            revision_of: &self.revision_of,
            assertion_state: self.assertion_state,
            evidence_refs: &evidence_refs,
        };
        format!(
            "assertion-{}",
            sha256_hex(
                &serde_json::to_vec(&identity)
                    .expect("typed temporal assertion serialization is infallible")
            )
        )
    }

    fn validate(&self) -> Result<(), ExperimentError> {
        for (field, value) in [
            ("assertion.assertion_id", self.assertion_id.as_str()),
            ("assertion.subject_id", self.subject_id.as_str()),
        ] {
            required(field, value)?;
        }
        if self.assertion_id != self.derived_id() {
            return Err(invalid(
                "assertion.assertion_id",
                "must be derived from canonical assertion content",
            ));
        }
        self.valid_time.parsed("assertion.valid_time")?;
        let retrieval = parse_time("assertion.retrieval_time", &self.retrieval_time)?;
        unique("assertion.evidence_refs", &self.evidence_refs)?;
        match self.availability_status {
            AvailabilityStatus::ArchivallyAttested => {
                let publication = self.publication_time.as_deref().ok_or_else(|| {
                    invalid(
                        "assertion.publication_time",
                        "archival evidence requires a time",
                    )
                })?;
                let availability = self.availability_time.as_deref().ok_or_else(|| {
                    invalid(
                        "assertion.availability_time",
                        "archival evidence requires a time",
                    )
                })?;
                let publication = parse_time("assertion.publication_time", publication)?;
                let availability = parse_time("assertion.availability_time", availability)?;
                if publication > availability || availability > retrieval {
                    return Err(invalid(
                        "assertion.availability_time",
                        "publication, availability, and retrieval clocks are inverted",
                    ));
                }
            }
            AvailabilityStatus::RetrievalTimeAttested => {
                if self.publication_time.is_some() || self.availability_time.is_some() {
                    return Err(invalid(
                        "assertion.availability_status",
                        "retrieval-only evidence cannot assert earlier clocks",
                    ));
                }
            }
        }
        match &self.value {
            AssertionValue::IdentityMappingEvidence {
                mapping_commitment, ..
            } => {
                required("assertion.value.mapping_commitment", mapping_commitment)?;
            }
            AssertionValue::Text { value } | AssertionValue::Timestamp { value } => {
                required("assertion.value", value)?;
                if matches!(self.value, AssertionValue::Timestamp { .. }) {
                    parse_time("assertion.value", value)?;
                }
            }
            AssertionValue::Integer { .. }
            | AssertionValue::Boolean { .. }
            | AssertionValue::RosterStatus { .. } => {}
        }
        if !matches!(
            (self.predicate, &self.value),
            (
                AssertionPredicate::IdentityMappingEvidence,
                AssertionValue::IdentityMappingEvidence { .. }
            ) | (
                AssertionPredicate::RosterStatus,
                AssertionValue::RosterStatus { .. }
            ) | (
                AssertionPredicate::ListingStatus | AssertionPredicate::Symbol,
                AssertionValue::Text { .. }
            )
        ) {
            return Err(invalid(
                "assertion.value",
                "must match the typed assertion predicate",
            ));
        }
        if matches!(self.assertion_state, AssertionState::Active) != self.revision_of.is_none() {
            return Err(invalid(
                "assertion.revision_of",
                "active assertions must be original; revisions and retractions must reference prior assertions",
            ));
        }
        Ok(())
    }

    fn validate_for_claim_use(&self, claim_use: ClaimUse) -> Result<(), ExperimentError> {
        self.validate()?;
        if self.availability_status == AvailabilityStatus::RetrievalTimeAttested
            && matches!(
                claim_use,
                ClaimUse::ContemporaneousAvailability | ClaimUse::Predictive
            )
        {
            return Err(invalid(
                "assertion.availability_status",
                "retrieval-time attestation cannot support contemporaneous or predictive claims",
            ));
        }
        Ok(())
    }
}

#[derive(Debug, PartialEq, Eq)]
pub struct VerifiedTemporalAssertionHistoryEvidence {
    history_hash: String,
    registered_head_id: String,
    genesis_commitment_hash: String,
    verified_use_time: String,
    registered_assertions: BTreeMap<String, TemporalAssertion>,
    registered_history: Vec<TemporalAssertion>,
}

impl VerifiedTemporalAssertionHistoryEvidence {
    #[cfg(test)]
    fn synthetic(assertions: &[TemporalAssertion]) -> Self {
        Self {
            history_hash: temporal_assertion_history_hash(assertions),
            registered_head_id: assertions
                .last()
                .expect("synthetic assertion history is non-empty")
                .assertion_id
                .clone(),
            genesis_commitment_hash: "a".repeat(64),
            verified_use_time: assertions
                .last()
                .expect("synthetic assertion history is non-empty")
                .retrieval_time
                .clone(),
            registered_assertions: assertions
                .iter()
                .cloned()
                .map(|assertion| (assertion.assertion_id.clone(), assertion))
                .collect(),
            registered_history: assertions.to_vec(),
        }
    }

    #[cfg(test)]
    fn verifies_roster_observation(&self, observation: &InventoryObservation) -> bool {
        !observation.assertion_refs.is_empty()
            && observation.assertion_refs.iter().all(|assertion_id| {
                self.registered_assertions
                    .get(assertion_id)
                    .is_some_and(|assertion| {
                        assertion.subject_id == observation.venue_instrument_identity.identity_id
                            && assertion.predicate == AssertionPredicate::RosterStatus
                            && assertion.valid_time == observation.time_unit
                            && matches!(
                                &assertion.value,
                                AssertionValue::RosterStatus { status, reason }
                                    if *status == observation.proposed_status
                                        && *reason == observation.status_reason
                            )
                            && !matches!(
                                assertion.assertion_state,
                                AssertionState::Retracted | AssertionState::Disputed
                            )
                            && !self.registered_assertions.values().any(|candidate| {
                                candidate.revision_of.as_deref() == Some(assertion_id.as_str())
                            })
                    })
            })
    }
}

pub fn validate_temporal_assertion_chain(
    assertions: &[TemporalAssertion],
    registered_evidence: Option<&VerifiedTemporalAssertionHistoryEvidence>,
) -> Result<(), ExperimentError> {
    let registered_evidence = registered_evidence.ok_or_else(|| {
        invalid(
            "assertion.registered_evidence",
            "requires a Genesis-bound immutable registered history",
        )
    })?;
    if assertions.is_empty()
        || registered_evidence.history_hash != temporal_assertion_history_hash(assertions)
        || registered_evidence.registered_head_id
            != assertions
                .last()
                .expect("non-empty assertion history")
                .assertion_id
        || !is_lowercase_sha256_hex(&registered_evidence.genesis_commitment_hash)
    {
        return Err(invalid(
            "assertion.registered_evidence",
            "does not match the immutable registered history",
        ));
    }
    let mut prior = BTreeMap::<&str, &TemporalAssertion>::new();
    let mut revised_parents = BTreeSet::new();
    let verified_use_time = parse_time(
        "assertion.registered_evidence.verified_use_time",
        &registered_evidence.verified_use_time,
    )?;
    for assertion in assertions {
        assertion.validate()?;
        if parse_time("assertion.retrieval_time", &assertion.retrieval_time)? > verified_use_time {
            return Err(invalid(
                "assertion.registered_evidence.verified_use_time",
                "cannot precede an assertion retrieval clock",
            ));
        }
        if prior.contains_key(assertion.assertion_id.as_str()) {
            return Err(invalid("assertion.assertion_id", "must be unique"));
        }
        if let Some(revision_of) = &assertion.revision_of {
            if !revised_parents.insert(revision_of.as_str()) {
                return Err(invalid(
                    "assertion.revision_of",
                    "a registered assertion may have only one successor",
                ));
            }
            let parent = prior.get(revision_of.as_str()).ok_or_else(|| {
                invalid(
                    "assertion.revision_of",
                    "must reference an earlier assertion",
                )
            })?;
            if parent.subject_id != assertion.subject_id || parent.predicate != assertion.predicate
            {
                return Err(invalid(
                    "assertion.revision_of",
                    "revision must preserve subject and predicate",
                ));
            }
            let parent_retrieval = parse_time(
                "assertion.revision_of.retrieval_time",
                &parent.retrieval_time,
            )?;
            let child_retrieval =
                parse_time("assertion.retrieval_time", &assertion.retrieval_time)?;
            if child_retrieval <= parent_retrieval {
                return Err(invalid(
                    "assertion.retrieval_time",
                    "a revision must have a strictly later retrieval clock",
                ));
            }
        }
        prior.insert(assertion.assertion_id.as_str(), assertion);
    }
    let claimable_leaves = assertions
        .iter()
        .filter(|assertion| {
            !revised_parents.contains(assertion.assertion_id.as_str())
                && matches!(
                    assertion.assertion_state,
                    AssertionState::Active | AssertionState::Corrected
                )
        })
        .collect::<Vec<_>>();
    for (index, left) in claimable_leaves.iter().enumerate() {
        let (left_start, left_end) = left.valid_time.parsed("assertion.valid_time")?;
        for right in claimable_leaves.iter().skip(index + 1) {
            if left.subject_id != right.subject_id || left.predicate != right.predicate {
                continue;
            }
            let contradictory = match (&left.value, &right.value) {
                (
                    AssertionValue::IdentityMappingEvidence {
                        mapping_commitment: left_commitment,
                        ..
                    },
                    AssertionValue::IdentityMappingEvidence {
                        mapping_commitment: right_commitment,
                        ..
                    },
                ) => left_commitment != right_commitment,
                _ => left.value != right.value,
            };
            if !contradictory {
                continue;
            }
            let (right_start, right_end) = right.valid_time.parsed("assertion.valid_time")?;
            if left_start < right_end && right_start < left_end {
                return Err(invalid(
                    "assertion.assertion_state",
                    "contradictory overlapping current assertions require dispute resolution",
                ));
            }
        }
    }
    Ok(())
}

pub fn validate_temporal_assertion_claim_use(
    assertions: &[TemporalAssertion],
    registered_evidence: Option<&VerifiedTemporalAssertionHistoryEvidence>,
    assertion_id: &str,
    claim_use: ClaimUse,
) -> Result<(), ExperimentError> {
    validate_temporal_assertion_chain(assertions, registered_evidence)?;
    let assertion = assertions
        .iter()
        .find(|assertion| assertion.assertion_id == assertion_id)
        .ok_or_else(|| invalid("assertion.assertion_id", "is not in the registered history"))?;
    if matches!(
        assertion.assertion_state,
        AssertionState::Retracted | AssertionState::Disputed
    ) || assertions
        .iter()
        .any(|candidate| candidate.revision_of.as_deref() == Some(assertion_id))
    {
        return Err(invalid(
            "assertion.assertion_state",
            "retracted, disputed, or superseded assertions cannot support claims",
        ));
    }
    assertion.validate_for_claim_use(claim_use)
}

fn temporal_assertion_history_hash(assertions: &[TemporalAssertion]) -> String {
    sha256_hex(
        &serde_json::to_vec(assertions)
            .expect("typed temporal assertion history serialization is infallible"),
    )
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ArtifactStoreSsmParameters {
    pub region: String,
    pub access_key_id: String,
    pub secret_access_key: String,
    pub session_token: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct StoragePolicy {
    pub artifact_root: String,
    #[serde(skip_serializing)]
    pub artifact_store_config_path: String,
    pub artifact_store_config_hash: String,
    pub producer_project: String,
    pub owner_project: String,
    pub writer_id: String,
    pub snapshot_id_namespace: String,
    pub credential_scope_ref: String,
    pub local_work_max_bytes: u64,
    pub retention_policy: String,
    pub registration_read_retry_limit: u32,
    pub ssm_parameters: ArtifactStoreSsmParameters,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum TimestampVerifierBinding {
    Registered { registry_key: String },
    TestFixture { fixture_id: String },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TimestampPolicy {
    pub verifier: TimestampVerifierBinding,
    pub receipt_schema: String,
    pub custody_anchor_max_interval_seconds: u64,
    pub registry_anchor_max_interval_seconds: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CorrectionPolicy {
    IgnoreLaterRevisions,
    DeterministicSealedIncorporation,
    NewExperimentVersion,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SourcePolicy {
    pub registry_binding: String,
    pub input_vintage_cutoff: String,
    pub zero_paid_queries: bool,
    pub zero_provider_access: bool,
    pub incremental_spend_cap_usd: String,
    pub retain_exact_or_lossless: bool,
    pub correction_policy: CorrectionPolicy,
    pub required_fidelity_classes: Vec<String>,
    pub required_rights: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PartitionsPolicy {
    pub e0_id: String,
    pub e0_start_time: String,
    pub e0_end_time: String,
    pub discovery_partition_id: String,
    pub discovery_start_time: String,
    pub discovery_end_time: String,
    pub evaluation_partition_id: String,
    pub evaluation_start_time: String,
    pub evaluation_end_time: String,
    pub purge_span_seconds: u64,
    pub boundary_policy: String,
    pub censoring_statuses: Vec<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct CanonicalDecimal(Decimal);

impl CanonicalDecimal {
    fn is_positive(self) -> bool {
        self.0 > Decimal::ZERO
    }

    fn is_unit_interval(self) -> bool {
        self.0 > Decimal::ZERO && self.0 <= Decimal::ONE
    }
}

impl Serialize for CanonicalDecimal {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_str(&self.0.normalize().to_string())
    }
}

impl<'de> Deserialize<'de> for CanonicalDecimal {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let value = String::deserialize(deserializer)?;
        let decimal = Decimal::from_str(&value)
            .map_err(|_| de::Error::custom("expected a finite decimal string"))?;
        Ok(Self(decimal.normalize()))
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TriggerCell {
    pub trigger_cell_id: String,
    pub abnormal_return_threshold: CanonicalDecimal,
    pub abnormal_volume_threshold: CanonicalDecimal,
    pub giveback_threshold: CanonicalDecimal,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DetectorPolicy {
    pub detector_id: String,
    pub observation_rule: String,
    pub event_clock: String,
    pub decision_clock: String,
    pub quote_normalization: String,
    pub return_definition: String,
    pub reported_volume_definition: String,
    pub baseline_rule: String,
    pub minimum_coverage_rule: String,
    pub missing_data_rule: String,
    pub interruption_rule: String,
    pub pump_window_seconds: u64,
    pub giveback_window_seconds: u64,
    pub warmup_seconds: u64,
    pub overlap_rule: String,
    pub cooldown_seconds: u64,
    pub tie_rule: String,
    pub deduplication_rule: String,
    pub episode_identity_rule: String,
    pub trigger_cells: Vec<TriggerCell>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ControlsPolicy {
    pub control_policy_id: String,
    pub risk_set_rule: String,
    pub pseudo_anchor_rule: String,
    pub feature_ids: Vec<String>,
    pub feature_cutoff_rule: String,
    pub regime_rule: String,
    pub matching_algorithm: String,
    pub distance_rule: String,
    pub caliper_rule: String,
    pub seed: u64,
    pub reuse_rule: String,
    pub relaxation_rule: String,
    pub balance_rule: String,
    pub missingness_rule: String,
    pub common_support_rule: String,
    pub contamination_rule: String,
    pub unmatched_rule: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AnalysisPolicy {
    pub primary_estimand_id: String,
    pub estimand_ids: Vec<String>,
    pub hypothesis_families: Vec<String>,
    pub unit_of_analysis: String,
    pub dependence_rule: String,
    pub multiplicity_rule: String,
    pub uncertainty_rule: String,
    pub null_result_rule: String,
    pub ordered_diagnostics: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DisclosurePolicy {
    pub program_id: String,
    pub table_ids: Vec<String>,
    pub grouping_ids: Vec<String>,
    pub filter_ids: Vec<String>,
    pub release_schedule: Vec<String>,
    pub maximum_release_count: u32,
    pub suppression_rule: String,
    pub rounding_rule: String,
    pub censoring_rule: String,
    pub boundary_flag_rule: String,
    pub cross_version_accounting_rule: String,
    pub recipient_ids: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum PrimaryCellRule {
    TriggerCell {
        trigger_cell_id: String,
    },
    Aggregation {
        aggregation_id: String,
        trigger_cell_ids: Vec<String>,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ComparisonRule {
    CanonicalJsonSemanticEquality,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum NormalizationRule {
    ExcludeDeclaredVolatileMetadata,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ExclusionRule {
    NoPostCommitExclusions,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum NumericToleranceRule {
    ExactDecimalAndInteger,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum RetryStateMachine {
    IdenticalInputRetryableInfrastructureOnly,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ConfirmationPolicy {
    pub primary_cell: PrimaryCellRule,
    pub comparator: ComparisonRule,
    pub normalization_rule: NormalizationRule,
    pub exclusion_rule: ExclusionRule,
    pub numeric_tolerance: NumericToleranceRule,
    pub retry_state_machine: RetryStateMachine,
    pub maximum_attempts: u32,
    pub failure_code_vocabulary: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EnrichmentPolicy {
    pub enabled: bool,
    pub requires_post_stage_one_authorization: bool,
    pub strata_rule: String,
    pub draw_rule: String,
    pub hypothesis_ids: Vec<String>,
    pub falsifier_ids: Vec<String>,
    pub candidate_packet_schema: String,
    pub content_neutral_ranking_rule: String,
    pub tie_rule: String,
    pub storage_window_rule: String,
    pub provider_selected: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ClaimTier {
    EpisodeDetected,
    NotProven,
    ManipulationAlleged,
    VenueSanctioned,
    MechanismConsistentWith,
    ManipulationProven,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ClaimPolicy {
    pub allowed_tiers: Vec<ClaimTier>,
    pub evidence_rule: String,
    pub forbidden_promotions: Vec<String>,
    pub invalidation_rule: String,
    pub l3_claim_requires_market_by_order: bool,
    pub absence_of_authority_label: ClaimTier,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NtRunPolicy {
    pub manifest_hash: String,
    pub dependency_set_hash: String,
    pub execution_environment_hash: String,
    pub random_seeds: BTreeMap<String, u64>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ExperimentDefinition {
    pub experiment: ExperimentVersion,
    pub target_frame: TargetFramePolicy,
    pub roles: RolePolicy,
    pub storage: StoragePolicy,
    pub timestamp_policy: TimestampPolicy,
    pub source_policy: SourcePolicy,
    pub partitions: PartitionsPolicy,
    pub detector: DetectorPolicy,
    pub controls: ControlsPolicy,
    pub analysis: AnalysisPolicy,
    pub disclosure: DisclosurePolicy,
    pub confirmation: ConfirmationPolicy,
    pub nt_run: NtRunPolicy,
    pub enrichment: EnrichmentPolicy,
    pub claims: ClaimPolicy,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ValidatedExperiment {
    pub definition: ExperimentDefinition,
    pub original_bytes: Vec<u8>,
    pub canonical_semantic_bytes: Vec<u8>,
    pub original_hash: String,
    pub semantic_hash: String,
    pub version_id: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ExperimentValidationSummary {
    pub experiment_id: String,
    pub version_sequence: u32,
    pub version_id: String,
    pub original_hash: String,
    pub semantic_hash: String,
    pub role_count: usize,
    pub trigger_cell_count: usize,
    pub provider_selected: bool,
    pub provider_access_authorized: bool,
    pub incremental_spend_cap_usd: String,
}

impl ValidatedExperiment {
    pub fn summary(&self) -> ExperimentValidationSummary {
        ExperimentValidationSummary {
            experiment_id: self.definition.experiment.experiment_id.clone(),
            version_sequence: self.definition.experiment.version_sequence,
            version_id: self.version_id.clone(),
            original_hash: self.original_hash.clone(),
            semantic_hash: self.semantic_hash.clone(),
            role_count: self.definition.roles.bindings.len(),
            trigger_cell_count: self.definition.detector.trigger_cells.len(),
            provider_selected: self.definition.enrichment.provider_selected,
            provider_access_authorized: !self.definition.source_policy.zero_provider_access,
            incremental_spend_cap_usd: self
                .definition
                .source_policy
                .incremental_spend_cap_usd
                .clone(),
        }
    }

    #[must_use]
    pub fn numeric_rules_hash(&self) -> String {
        #[derive(Serialize)]
        struct NumericRules<'a> {
            schema_version: &'static str,
            partitions: &'a PartitionsPolicy,
            detector: &'a DetectorPolicy,
            controls: &'a ControlsPolicy,
            analysis: &'a AnalysisPolicy,
            disclosure: &'a DisclosurePolicy,
            confirmation: &'a ConfirmationPolicy,
        }

        sha256_hex(
            &serde_json::to_vec(&NumericRules {
                schema_version: "pump-research-numeric-rules-v1",
                partitions: &self.definition.partitions,
                detector: &self.definition.detector,
                controls: &self.definition.controls,
                analysis: &self.definition.analysis,
                disclosure: &self.definition.disclosure,
                confirmation: &self.definition.confirmation,
            })
            .expect("typed numeric-rule serialization is infallible"),
        )
    }
}

pub fn load_and_validate_experiment(path: &Path) -> Result<ValidatedExperiment, ExperimentError> {
    let bytes = fs::read(path).map_err(|error| ExperimentError::Io(error.to_string()))?;
    parse_and_validate(&bytes, ValidationMode::Production)
}

fn parse_and_validate(
    bytes: &[u8],
    mode: ValidationMode,
) -> Result<ValidatedExperiment, ExperimentError> {
    let text =
        std::str::from_utf8(bytes).map_err(|error| ExperimentError::Parse(error.to_string()))?;
    let mut definition: ExperimentDefinition =
        toml::from_str(text).map_err(|error| ExperimentError::Parse(error.to_string()))?;
    validate_definition(&definition, mode)?;
    canonicalize(&mut definition);
    let canonical_semantic_bytes = serde_json::to_vec(&definition)
        .map_err(|error| ExperimentError::Parse(error.to_string()))?;
    let original_hash = sha256_hex(bytes);
    let semantic_hash = sha256_hex(&canonical_semantic_bytes);
    Ok(ValidatedExperiment {
        definition,
        original_bytes: bytes.to_vec(),
        canonical_semantic_bytes,
        original_hash,
        version_id: semantic_hash.clone(),
        semantic_hash,
    })
}

#[cfg(test)]
pub(crate) fn parse_fixture_experiment() -> ValidatedExperiment {
    parse_and_validate(
        include_bytes!("../../../config/research/pump-research-synthetic.toml"),
        ValidationMode::Fixture,
    )
    .expect("repository synthetic experiment fixture must validate")
}

#[cfg(test)]
pub(crate) fn parse_fixture_experiment_with_nt_manifest_and_store_hash(
    manifest_hash: &str,
    store_hash: &str,
) -> ValidatedExperiment {
    let text = include_str!("../../../config/research/pump-research-synthetic.toml")
        .replace(
            "3333333333333333333333333333333333333333333333333333333333333333",
            manifest_hash,
        )
        .replace(
            "2222222222222222222222222222222222222222222222222222222222222222",
            store_hash,
        );
    parse_and_validate(text.as_bytes(), ValidationMode::Fixture)
        .expect("repository synthetic experiment fixture with pinned dependencies must validate")
}

fn validate_definition(
    definition: &ExperimentDefinition,
    mode: ValidationMode,
) -> Result<(), ExperimentError> {
    validate_version(&definition.experiment)?;
    validate_roles(definition, mode)?;
    validate_frame(&definition.target_frame)?;
    validate_storage(&definition.storage, mode)?;
    validate_sources(&definition.source_policy)?;
    validate_partitions(definition)?;
    validate_detector(&definition.detector)?;
    validate_policies(definition)?;
    if definition
        .timestamp_policy
        .custody_anchor_max_interval_seconds
        == 0
        || definition
            .timestamp_policy
            .registry_anchor_max_interval_seconds
            == 0
        || definition.timestamp_policy.receipt_schema.is_empty()
    {
        return Err(invalid(
            "timestamp_policy",
            "missing verifier or positive interval",
        ));
    }
    match &definition.timestamp_policy.verifier {
        TimestampVerifierBinding::Registered { registry_key } => {
            required("timestamp_policy.verifier.registry_key", registry_key)?;
        }
        TimestampVerifierBinding::TestFixture { fixture_id } => {
            required("timestamp_policy.verifier.fixture_id", fixture_id)?;
            if matches!(mode, ValidationMode::Production) {
                return Err(invalid(
                    "timestamp_policy.verifier",
                    "fixture timestamp verifier is unavailable to non-test execution",
                ));
            }
        }
    }
    Ok(())
}

fn validate_version(version: &ExperimentVersion) -> Result<(), ExperimentError> {
    required("experiment.experiment_id", &version.experiment_id)?;
    required("experiment.append_role", &version.append_role)?;
    parse_time("experiment.created_at", &version.created_at)?;
    if version.version_sequence == 0
        || version.schema_version != EXPERIMENT_SCHEMA_VERSION
        || version.canonicalization_version != CANONICALIZATION_VERSION
    {
        return Err(invalid(
            "experiment",
            "unsupported or missing version contract",
        ));
    }
    if version.state != ExperimentState::Draft {
        return Err(invalid(
            "experiment.state",
            "a definition version starts in draft; verified artifacts derive later states",
        ));
    }
    if version.lineage_refs.is_empty() {
        return Err(invalid("experiment.lineage_refs", "cannot be empty"));
    }
    let mut lineage_ids = BTreeSet::new();
    for lineage in &version.lineage_refs {
        required("experiment.lineage_refs.artifact_id", &lineage.artifact_id)?;
        hash(
            "experiment.lineage_refs.content_hash",
            &lineage.content_hash,
        )?;
        if !lineage_ids.insert((lineage.artifact_kind, lineage.artifact_id.as_str())) {
            return Err(invalid(
                "experiment.lineage_refs",
                "duplicate lineage identity",
            ));
        }
    }
    if version.version_sequence == 1 {
        if version.parent_version_id.is_some() || version.parent_content_hash.is_some() {
            return Err(invalid(
                "experiment.parent_version_id",
                "genesis cannot have a parent",
            ));
        }
        return Ok(());
    }
    let parent_id = version
        .parent_version_id
        .as_deref()
        .ok_or_else(|| invalid("experiment.parent_version_id", "required after genesis"))?;
    let parent_hash = version
        .parent_content_hash
        .as_deref()
        .ok_or_else(|| invalid("experiment.parent_content_hash", "required after genesis"))?;
    hash("experiment.parent_version_id", parent_id)?;
    hash("experiment.parent_content_hash", parent_hash)?;
    let parent_artifact = format!("experiment-{}-{parent_id}", version.experiment_id);
    if !version.lineage_refs.iter().any(|lineage| {
        lineage.artifact_kind == StoreArtifactKind::ResearchAnalytics
            && lineage.artifact_id == parent_artifact
            && lineage.content_hash == parent_hash
    }) {
        return Err(invalid(
            "experiment.lineage_refs",
            "exact parent lineage is missing",
        ));
    }
    Ok(())
}

fn validate_roles(
    definition: &ExperimentDefinition,
    mode: ValidationMode,
) -> Result<(), ExperimentError> {
    let mut ids = BTreeSet::new();
    let mut purposes = BTreeSet::new();
    let mut principals = BTreeSet::new();
    let mut credential_scopes = BTreeSet::new();
    for role in &definition.roles.bindings {
        for (field, value) in [
            ("roles.role_id", role.role_id.as_str()),
            ("roles.account_id", role.account_id.as_str()),
            (
                "roles.principal_arn_prefix",
                role.principal_arn_prefix.as_str(),
            ),
            ("roles.user_id_prefix", role.user_id_prefix.as_str()),
            (
                "roles.credential_scope_ref",
                role.credential_scope_ref.as_str(),
            ),
        ] {
            required(field, value)?;
        }
        if !ids.insert(role.role_id.as_str()) || !purposes.insert(role.purpose) {
            return Err(invalid("roles.bindings", "duplicate role id or purpose"));
        }
        if !principals.insert((
            role.account_id.as_str(),
            role.principal_arn_prefix.as_str(),
            role.user_id_prefix.as_str(),
        )) {
            return Err(invalid("roles.bindings", "roles cannot share a principal"));
        }
        if !credential_scopes.insert(role.credential_scope_ref.as_str()) {
            return Err(invalid(
                "roles.bindings.credential_scope_ref",
                "roles cannot share a credential scope",
            ));
        }
        if matches!(mode, ValidationMode::Production)
            && role.authority != PrincipalAuthority::AwsSts
        {
            return Err(invalid(
                "roles.bindings.authority",
                "non-test definitions require aws_sts authority",
            ));
        }
        if matches!(mode, ValidationMode::Production) {
            validate_aws_role_binding(role)?;
        }
    }
    for purpose in [
        RolePurpose::Ingestion,
        RolePurpose::Disclosure,
        RolePurpose::CanonicalEvaluation,
        RolePurpose::VerificationReplay,
        RolePurpose::Custody,
        RolePurpose::ExperimentDecision,
        RolePurpose::GovernanceApproval,
    ] {
        if !purposes.contains(&purpose) {
            return Err(invalid(
                "roles.bindings.purpose",
                "required role is missing",
            ));
        }
    }
    let append = definition
        .roles
        .bindings
        .iter()
        .find(|role| role.role_id == definition.experiment.append_role)
        .ok_or_else(|| invalid("experiment.append_role", "unknown role"))?;
    if !append.can_append_versions {
        return Err(invalid(
            "experiment.append_role",
            "role cannot append versions",
        ));
    }
    if append.credential_scope_ref != definition.storage.credential_scope_ref {
        return Err(invalid(
            "experiment.append_role",
            "append role credential scope does not match registration storage scope",
        ));
    }
    let mut separations = BTreeSet::new();
    for separation in &definition.roles.required_separations {
        if separation.left_role == separation.right_role
            || !ids.contains(separation.left_role.as_str())
            || !ids.contains(separation.right_role.as_str())
        {
            return Err(invalid("roles.required_separations", "invalid role pair"));
        }
        let pair = if separation.left_role < separation.right_role {
            (
                separation.left_role.as_str(),
                separation.right_role.as_str(),
            )
        } else {
            (
                separation.right_role.as_str(),
                separation.left_role.as_str(),
            )
        };
        if !separations.insert(pair) {
            return Err(invalid("roles.required_separations", "duplicate role pair"));
        }
    }
    Ok(())
}

fn validate_aws_role_binding(role: &RoleBinding) -> Result<(), ExperimentError> {
    if role.account_id.len() != 12 || !role.account_id.bytes().all(|byte| byte.is_ascii_digit()) {
        return Err(invalid(
            "roles.bindings.account_id",
            "must be a 12-digit AWS account id",
        ));
    }
    let mut arn = role.principal_arn_prefix.splitn(6, ':');
    let arn_marker = arn.next();
    let partition = arn.next();
    let service = arn.next();
    let region = arn.next();
    let account = arn.next();
    let resource = arn.next();
    if arn_marker != Some("arn")
        || partition.is_none_or(|value| value.is_empty() || value.chars().any(char::is_whitespace))
        || service != Some("sts")
        || region != Some("")
        || account != Some(role.account_id.as_str())
    {
        return Err(invalid(
            "roles.bindings.principal_arn_prefix",
            "must be a structurally valid STS assumed-role ARN prefix",
        ));
    }
    let role_name = resource
        .and_then(|value| value.strip_prefix("assumed-role/"))
        .and_then(|value| value.strip_suffix('/'))
        .filter(|value| !value.is_empty() && !value.contains('/'))
        .ok_or_else(|| {
            invalid(
                "roles.bindings.principal_arn_prefix",
                "must identify exactly one bounded AWS assumed-role name",
            )
        })?;
    if role_name.len() > 64
        || role_name.bytes().any(|byte| {
            !byte.is_ascii_alphanumeric()
                && !matches!(byte, b'_' | b'+' | b'=' | b',' | b'.' | b'@' | b'-')
        })
    {
        return Err(invalid(
            "roles.bindings.principal_arn_prefix",
            "assumed-role name contains unsupported characters",
        ));
    }
    let principal_id = role
        .user_id_prefix
        .strip_suffix(':')
        .filter(|value| {
            value.len() >= 4
                && value
                    .bytes()
                    .all(|byte| byte.is_ascii_uppercase() || byte.is_ascii_digit())
        })
        .ok_or_else(|| {
            invalid(
                "roles.bindings.user_id_prefix",
                "must be a bounded AWS role principal id ending in a colon",
            )
        })?;
    if principal_id.is_empty() {
        return Err(invalid(
            "roles.bindings.user_id_prefix",
            "principal id cannot be empty",
        ));
    }
    Ok(())
}

fn validate_frame(frame: &TargetFramePolicy) -> Result<(), ExperimentError> {
    required("target_frame.frame_id", &frame.frame_id)?;
    unique("target_frame.venue_keys", &frame.venue_keys)?;
    unique("target_frame.market_family_keys", &frame.market_family_keys)?;
    let mut inventory_sources = BTreeSet::new();
    if frame.inventory_source_refs.is_empty() {
        return Err(invalid(
            "target_frame.inventory_source_refs",
            "must not be empty",
        ));
    }
    for binding in &frame.inventory_source_refs {
        for (field, value) in [
            (
                "target_frame.inventory_source_refs.source_ref",
                binding.source_ref.as_str(),
            ),
            (
                "target_frame.inventory_source_refs.source_entry_id",
                binding.source_entry_id.as_str(),
            ),
        ] {
            required(field, value)?;
        }
        for value in [
            &binding.source_entry_content_hash,
            &binding.manifest_hash,
            &binding.coverage_hash,
        ] {
            hash("target_frame.inventory_source_refs", value)?;
        }
        if binding.source_entry_version == 0
            || !inventory_sources.insert(binding.source_ref.as_str())
        {
            return Err(invalid(
                "target_frame.inventory_source_refs",
                "source bindings must be unique and versioned",
            ));
        }
    }
    let start = parse_time("target_frame.start_time", &frame.start_time)?;
    let end = parse_time("target_frame.end_time", &frame.end_time)?;
    parse_time("target_frame.roster_vintage", &frame.roster_vintage)?;
    if start >= end {
        return Err(invalid("target_frame.end_time", "must follow start_time"));
    }
    let statuses = frame
        .status_precedence
        .iter()
        .copied()
        .collect::<BTreeSet<_>>();
    if statuses.len() != 4 || frame.status_precedence.len() != 4 {
        return Err(invalid(
            "target_frame.status_precedence",
            "must contain all four statuses",
        ));
    }
    Ok(())
}

fn validate_storage(storage: &StoragePolicy, mode: ValidationMode) -> Result<(), ExperimentError> {
    if !storage.artifact_root.starts_with("s3://") || storage.artifact_root.ends_with('/') {
        return Err(invalid(
            "storage.artifact_root",
            "must be a canonical S3 root",
        ));
    }
    if matches!(mode, ValidationMode::Production)
        && ["example-", "fixture", "test"]
            .iter()
            .any(|token| storage.artifact_root.contains(token))
    {
        return Err(invalid(
            "storage.artifact_root",
            "test roots are unavailable in production",
        ));
    }
    for (field, value) in [
        (
            "storage.artifact_store_config_path",
            storage.artifact_store_config_path.as_str(),
        ),
        (
            "storage.producer_project",
            storage.producer_project.as_str(),
        ),
        ("storage.owner_project", storage.owner_project.as_str()),
        ("storage.writer_id", storage.writer_id.as_str()),
        (
            "storage.snapshot_id_namespace",
            storage.snapshot_id_namespace.as_str(),
        ),
        (
            "storage.credential_scope_ref",
            storage.credential_scope_ref.as_str(),
        ),
        (
            "storage.retention_policy",
            storage.retention_policy.as_str(),
        ),
        (
            "storage.ssm_parameters.region",
            storage.ssm_parameters.region.as_str(),
        ),
        (
            "storage.ssm_parameters.access_key_id",
            storage.ssm_parameters.access_key_id.as_str(),
        ),
        (
            "storage.ssm_parameters.secret_access_key",
            storage.ssm_parameters.secret_access_key.as_str(),
        ),
    ] {
        required(field, value)?;
    }
    hash(
        "storage.artifact_store_config_hash",
        &storage.artifact_store_config_hash,
    )?;
    for (field, value) in [
        (
            "storage.ssm_parameters.access_key_id",
            storage.ssm_parameters.access_key_id.as_str(),
        ),
        (
            "storage.ssm_parameters.secret_access_key",
            storage.ssm_parameters.secret_access_key.as_str(),
        ),
    ] {
        validate_ssm_parameter_ref(field, value)?;
    }
    if let Some(session_token) = &storage.ssm_parameters.session_token {
        validate_ssm_parameter_ref("storage.ssm_parameters.session_token", session_token)?;
    }
    if storage.local_work_max_bytes == 0 || storage.registration_read_retry_limit == 0 {
        return Err(invalid(
            "storage",
            "bounds and retry limit must be positive",
        ));
    }
    Ok(())
}

fn validate_sources(source: &SourcePolicy) -> Result<(), ExperimentError> {
    required("source_policy.registry_binding", &source.registry_binding)?;
    parse_time(
        "source_policy.input_vintage_cutoff",
        &source.input_vintage_cutoff,
    )?;
    unique(
        "source_policy.required_fidelity_classes",
        &source.required_fidelity_classes,
    )?;
    unique("source_policy.required_rights", &source.required_rights)?;
    if !source.zero_paid_queries
        || !source.zero_provider_access
        || source.incremental_spend_cap_usd != "0"
        || !source.retain_exact_or_lossless
    {
        return Err(invalid(
            "source_policy",
            "current contract requires zero access/spend and reproducible retained inputs",
        ));
    }
    Ok(())
}

fn validate_partitions(definition: &ExperimentDefinition) -> Result<(), ExperimentError> {
    let partitions = &definition.partitions;
    for (field, value) in [
        ("partitions.e0_id", partitions.e0_id.as_str()),
        (
            "partitions.discovery_partition_id",
            partitions.discovery_partition_id.as_str(),
        ),
        (
            "partitions.evaluation_partition_id",
            partitions.evaluation_partition_id.as_str(),
        ),
        (
            "partitions.boundary_policy",
            partitions.boundary_policy.as_str(),
        ),
    ] {
        required(field, value)?;
    }
    unique(
        "partitions.censoring_statuses",
        &partitions.censoring_statuses,
    )?;
    let e0_start = parse_time("partitions.e0_start_time", &partitions.e0_start_time)?;
    let e0_end = parse_time("partitions.e0_end_time", &partitions.e0_end_time)?;
    let discovery_start = parse_time(
        "partitions.discovery_start_time",
        &partitions.discovery_start_time,
    )?;
    let discovery_end = parse_time(
        "partitions.discovery_end_time",
        &partitions.discovery_end_time,
    )?;
    let evaluation_start = parse_time(
        "partitions.evaluation_start_time",
        &partitions.evaluation_start_time,
    )?;
    let evaluation_end = parse_time(
        "partitions.evaluation_end_time",
        &partitions.evaluation_end_time,
    )?;
    let frame_start = parse_time(
        "target_frame.start_time",
        &definition.target_frame.start_time,
    )?;
    let frame_end = parse_time("target_frame.end_time", &definition.target_frame.end_time)?;
    if !(frame_start <= discovery_start
        && discovery_start < discovery_end
        && discovery_end <= evaluation_start
        && e0_start <= evaluation_start
        && evaluation_start < evaluation_end
        && evaluation_end <= e0_end
        && evaluation_end <= frame_end)
    {
        return Err(invalid(
            "partitions",
            "invalid frame/discovery/E0/evaluation ordering",
        ));
    }
    let required_purge = definition
        .detector
        .warmup_seconds
        .max(definition.detector.pump_window_seconds)
        .max(definition.detector.giveback_window_seconds);
    if partitions.purge_span_seconds < required_purge {
        return Err(invalid(
            "partitions.purge_span_seconds",
            "does not cover detector spans",
        ));
    }
    Ok(())
}

fn validate_detector(detector: &DetectorPolicy) -> Result<(), ExperimentError> {
    for (field, value) in [
        ("detector.detector_id", detector.detector_id.as_str()),
        (
            "detector.observation_rule",
            detector.observation_rule.as_str(),
        ),
        ("detector.event_clock", detector.event_clock.as_str()),
        ("detector.decision_clock", detector.decision_clock.as_str()),
        (
            "detector.quote_normalization",
            detector.quote_normalization.as_str(),
        ),
        (
            "detector.return_definition",
            detector.return_definition.as_str(),
        ),
        (
            "detector.reported_volume_definition",
            detector.reported_volume_definition.as_str(),
        ),
        ("detector.baseline_rule", detector.baseline_rule.as_str()),
        (
            "detector.minimum_coverage_rule",
            detector.minimum_coverage_rule.as_str(),
        ),
        (
            "detector.missing_data_rule",
            detector.missing_data_rule.as_str(),
        ),
        (
            "detector.interruption_rule",
            detector.interruption_rule.as_str(),
        ),
        ("detector.overlap_rule", detector.overlap_rule.as_str()),
        ("detector.tie_rule", detector.tie_rule.as_str()),
        (
            "detector.deduplication_rule",
            detector.deduplication_rule.as_str(),
        ),
        (
            "detector.episode_identity_rule",
            detector.episode_identity_rule.as_str(),
        ),
    ] {
        required(field, value)?;
    }
    if detector.pump_window_seconds == 0
        || detector.giveback_window_seconds == 0
        || detector.warmup_seconds == 0
        || detector.cooldown_seconds == 0
        || detector.trigger_cells.is_empty()
    {
        return Err(invalid(
            "detector",
            "durations and trigger cells must be present",
        ));
    }
    let mut ids = BTreeSet::new();
    for cell in &detector.trigger_cells {
        required("detector.trigger_cell_id", &cell.trigger_cell_id)?;
        if !cell.abnormal_return_threshold.is_positive()
            || !cell.abnormal_volume_threshold.is_positive()
            || !cell.giveback_threshold.is_unit_interval()
        {
            return Err(invalid(
                "detector.trigger_cells",
                "return and volume thresholds must be positive and giveback must be in (0, 1]",
            ));
        }
        if !ids.insert(cell.trigger_cell_id.as_str()) {
            return Err(invalid("detector.trigger_cell_id", "duplicate id"));
        }
    }
    Ok(())
}

fn validate_policies(definition: &ExperimentDefinition) -> Result<(), ExperimentError> {
    let controls = &definition.controls;
    for (field, value) in [
        (
            "controls.control_policy_id",
            controls.control_policy_id.as_str(),
        ),
        ("controls.risk_set_rule", controls.risk_set_rule.as_str()),
        (
            "controls.pseudo_anchor_rule",
            controls.pseudo_anchor_rule.as_str(),
        ),
        (
            "controls.feature_cutoff_rule",
            controls.feature_cutoff_rule.as_str(),
        ),
        ("controls.regime_rule", controls.regime_rule.as_str()),
        (
            "controls.matching_algorithm",
            controls.matching_algorithm.as_str(),
        ),
        ("controls.distance_rule", controls.distance_rule.as_str()),
        ("controls.caliper_rule", controls.caliper_rule.as_str()),
        ("controls.reuse_rule", controls.reuse_rule.as_str()),
        (
            "controls.relaxation_rule",
            controls.relaxation_rule.as_str(),
        ),
        ("controls.balance_rule", controls.balance_rule.as_str()),
        (
            "controls.missingness_rule",
            controls.missingness_rule.as_str(),
        ),
        (
            "controls.common_support_rule",
            controls.common_support_rule.as_str(),
        ),
        (
            "controls.contamination_rule",
            controls.contamination_rule.as_str(),
        ),
        ("controls.unmatched_rule", controls.unmatched_rule.as_str()),
    ] {
        required(field, value)?;
    }
    unique("controls.feature_ids", &controls.feature_ids)?;

    let analysis = &definition.analysis;
    required(
        "analysis.primary_estimand_id",
        &analysis.primary_estimand_id,
    )?;
    unique("analysis.estimand_ids", &analysis.estimand_ids)?;
    unique(
        "analysis.hypothesis_families",
        &analysis.hypothesis_families,
    )?;
    unique(
        "analysis.ordered_diagnostics",
        &analysis.ordered_diagnostics,
    )?;
    if !analysis
        .estimand_ids
        .contains(&analysis.primary_estimand_id)
    {
        return Err(invalid("analysis.primary_estimand_id", "unknown estimand"));
    }
    for (field, value) in [
        (
            "analysis.unit_of_analysis",
            analysis.unit_of_analysis.as_str(),
        ),
        (
            "analysis.dependence_rule",
            analysis.dependence_rule.as_str(),
        ),
        (
            "analysis.multiplicity_rule",
            analysis.multiplicity_rule.as_str(),
        ),
        (
            "analysis.uncertainty_rule",
            analysis.uncertainty_rule.as_str(),
        ),
        (
            "analysis.null_result_rule",
            analysis.null_result_rule.as_str(),
        ),
    ] {
        required(field, value)?;
    }

    let disclosure = &definition.disclosure;
    required("disclosure.program_id", &disclosure.program_id)?;
    unique("disclosure.table_ids", &disclosure.table_ids)?;
    unique("disclosure.grouping_ids", &disclosure.grouping_ids)?;
    unique("disclosure.filter_ids", &disclosure.filter_ids)?;
    unique("disclosure.recipient_ids", &disclosure.recipient_ids)?;
    unique("disclosure.release_schedule", &disclosure.release_schedule)?;
    if disclosure.maximum_release_count == 0
        || disclosure.release_schedule.len() != disclosure.maximum_release_count as usize
    {
        return Err(invalid(
            "disclosure.release_schedule",
            "must equal release count",
        ));
    }
    for (field, value) in [
        (
            "disclosure.suppression_rule",
            disclosure.suppression_rule.as_str(),
        ),
        (
            "disclosure.rounding_rule",
            disclosure.rounding_rule.as_str(),
        ),
        (
            "disclosure.censoring_rule",
            disclosure.censoring_rule.as_str(),
        ),
        (
            "disclosure.boundary_flag_rule",
            disclosure.boundary_flag_rule.as_str(),
        ),
        (
            "disclosure.cross_version_accounting_rule",
            disclosure.cross_version_accounting_rule.as_str(),
        ),
    ] {
        required(field, value)?;
    }

    let confirmation = &definition.confirmation;
    if confirmation.maximum_attempts == 0 {
        return Err(invalid("confirmation.maximum_attempts", "must be positive"));
    }
    unique(
        "confirmation.failure_code_vocabulary",
        &confirmation.failure_code_vocabulary,
    )?;
    let trigger_ids = definition
        .detector
        .trigger_cells
        .iter()
        .map(|cell| cell.trigger_cell_id.as_str())
        .collect::<BTreeSet<_>>();
    match &confirmation.primary_cell {
        PrimaryCellRule::TriggerCell { trigger_cell_id } => {
            required("confirmation.primary_cell.trigger_cell_id", trigger_cell_id)?;
            if !trigger_ids.contains(trigger_cell_id.as_str()) {
                return Err(invalid(
                    "confirmation.primary_cell.trigger_cell_id",
                    "unknown detector trigger cell",
                ));
            }
        }
        PrimaryCellRule::Aggregation {
            aggregation_id,
            trigger_cell_ids,
        } => {
            required("confirmation.primary_cell.aggregation_id", aggregation_id)?;
            unique(
                "confirmation.primary_cell.trigger_cell_ids",
                trigger_cell_ids,
            )?;
            if trigger_cell_ids
                .iter()
                .any(|id| !trigger_ids.contains(id.as_str()))
            {
                return Err(invalid(
                    "confirmation.primary_cell.trigger_cell_ids",
                    "unknown detector trigger cell",
                ));
            }
        }
    }

    let enrichment = &definition.enrichment;
    if enrichment.enabled
        || enrichment.provider_selected
        || !enrichment.requires_post_stage_one_authorization
    {
        return Err(invalid(
            "enrichment",
            "current version must be disabled, provider-neutral, and separately authorized",
        ));
    }
    unique("enrichment.hypothesis_ids", &enrichment.hypothesis_ids)?;
    unique("enrichment.falsifier_ids", &enrichment.falsifier_ids)?;
    for (field, value) in [
        ("enrichment.strata_rule", enrichment.strata_rule.as_str()),
        ("enrichment.draw_rule", enrichment.draw_rule.as_str()),
        (
            "enrichment.candidate_packet_schema",
            enrichment.candidate_packet_schema.as_str(),
        ),
        (
            "enrichment.content_neutral_ranking_rule",
            enrichment.content_neutral_ranking_rule.as_str(),
        ),
        ("enrichment.tie_rule", enrichment.tie_rule.as_str()),
        (
            "enrichment.storage_window_rule",
            enrichment.storage_window_rule.as_str(),
        ),
    ] {
        required(field, value)?;
    }

    let claims = &definition.claims;
    if claims.allowed_tiers.is_empty()
        || claims
            .allowed_tiers
            .iter()
            .copied()
            .collect::<BTreeSet<_>>()
            .len()
            != claims.allowed_tiers.len()
        || claims.absence_of_authority_label != ClaimTier::NotProven
        || !claims.l3_claim_requires_market_by_order
    {
        return Err(invalid(
            "claims",
            "invalid tiers, absence label, or L3 gate",
        ));
    }
    required("claims.evidence_rule", &claims.evidence_rule)?;
    required("claims.invalidation_rule", &claims.invalidation_rule)?;
    unique("claims.forbidden_promotions", &claims.forbidden_promotions)?;

    let nt_run = &definition.nt_run;
    for (field, value) in [
        ("nt_run.manifest_hash", nt_run.manifest_hash.as_str()),
        (
            "nt_run.dependency_set_hash",
            nt_run.dependency_set_hash.as_str(),
        ),
        (
            "nt_run.execution_environment_hash",
            nt_run.execution_environment_hash.as_str(),
        ),
    ] {
        hash(field, value)?;
    }
    if nt_run.random_seeds.is_empty()
        || nt_run.random_seeds.get("control-matching") != Some(&definition.controls.seed)
    {
        return Err(invalid(
            "nt_run.random_seeds",
            "must bind the configured control-matching seed",
        ));
    }
    Ok(())
}

fn canonicalize(definition: &mut ExperimentDefinition) {
    definition.experiment.lineage_refs.sort_by(|a, b| {
        (
            a.artifact_kind,
            &a.artifact_id,
            a.artifact_version,
            &a.content_hash,
        )
            .cmp(&(
                b.artifact_kind,
                &b.artifact_id,
                b.artifact_version,
                &b.content_hash,
            ))
    });
    definition
        .roles
        .bindings
        .sort_by(|a, b| a.role_id.cmp(&b.role_id));
    for separation in &mut definition.roles.required_separations {
        if separation.left_role > separation.right_role {
            std::mem::swap(&mut separation.left_role, &mut separation.right_role);
        }
    }
    definition
        .roles
        .required_separations
        .sort_by(|a, b| (&a.left_role, &a.right_role).cmp(&(&b.left_role, &b.right_role)));
    definition.target_frame.venue_keys.sort();
    definition.target_frame.market_family_keys.sort();
    definition.source_policy.required_fidelity_classes.sort();
    definition.source_policy.required_rights.sort();
    definition
        .detector
        .trigger_cells
        .sort_by(|a, b| a.trigger_cell_id.cmp(&b.trigger_cell_id));
    definition.controls.feature_ids.sort();
    definition.analysis.estimand_ids.sort();
    definition.analysis.hypothesis_families.sort();
    definition.disclosure.table_ids.sort();
    definition.disclosure.grouping_ids.sort();
    definition.disclosure.filter_ids.sort();
    definition.disclosure.recipient_ids.sort();
    if let PrimaryCellRule::Aggregation {
        trigger_cell_ids, ..
    } = &mut definition.confirmation.primary_cell
    {
        trigger_cell_ids.sort();
    }
    definition.confirmation.failure_code_vocabulary.sort();
    definition.enrichment.hypothesis_ids.sort();
    definition.enrichment.falsifier_ids.sort();
    definition.claims.allowed_tiers.sort();
    definition.claims.forbidden_promotions.sort();
}

fn required(field: &'static str, value: &str) -> Result<(), ExperimentError> {
    if value.is_empty() || value.trim() != value {
        Err(invalid(
            field,
            "must be non-empty without surrounding whitespace",
        ))
    } else {
        Ok(())
    }
}

fn validate_ssm_parameter_ref(field: &'static str, value: &str) -> Result<(), ExperimentError> {
    required(field, value)?;
    if !value.starts_with('/') || value.chars().any(char::is_whitespace) {
        return Err(invalid(
            field,
            "must be an absolute SSM parameter path without whitespace",
        ));
    }
    Ok(())
}

fn unique(field: &'static str, values: &[String]) -> Result<(), ExperimentError> {
    if values.is_empty() {
        return Err(invalid(field, "cannot be empty"));
    }
    let mut seen = BTreeSet::new();
    for value in values {
        required(field, value)?;
        if !seen.insert(value.as_str()) {
            return Err(invalid(field, "duplicate identifier"));
        }
    }
    Ok(())
}

fn hash(field: &'static str, value: &str) -> Result<(), ExperimentError> {
    if is_lowercase_sha256_hex(value) {
        Ok(())
    } else {
        Err(invalid(field, "must be a lowercase SHA-256 digest"))
    }
}

fn parse_time(field: &'static str, value: &str) -> Result<DateTime<Utc>, ExperimentError> {
    DateTime::parse_from_rfc3339(value)
        .map(|value| value.with_timezone(&Utc))
        .map_err(|_| invalid(field, "must be an RFC3339 timestamp"))
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CallerIdentity {
    pub account_id: String,
    pub arn: String,
    pub user_id: String,
}

pub async fn resolve_sts_caller_identity() -> Result<CallerIdentity, ExperimentError> {
    let config = aws_config::defaults(BehaviorVersion::latest()).load().await;
    let output = aws_sdk_sts::Client::new(&config)
        .get_caller_identity()
        .send()
        .await
        .map_err(|_| ExperimentError::IdentityUnavailable)?;
    Ok(CallerIdentity {
        account_id: output
            .account()
            .filter(|value| !value.is_empty())
            .ok_or(ExperimentError::IdentityUnavailable)?
            .to_string(),
        arn: output
            .arn()
            .filter(|value| !value.is_empty())
            .ok_or(ExperimentError::IdentityUnavailable)?
            .to_string(),
        user_id: output
            .user_id()
            .filter(|value| !value.is_empty())
            .ok_or(ExperimentError::IdentityUnavailable)?
            .to_string(),
    })
}

pub fn match_caller_role<'a>(
    definition: &'a ExperimentDefinition,
    identity: &CallerIdentity,
) -> Result<&'a RoleBinding, ExperimentError> {
    let matches = definition
        .roles
        .bindings
        .iter()
        .filter(|role| {
            role.authority == PrincipalAuthority::AwsSts
                && role.account_id == identity.account_id
                && identity
                    .arn
                    .strip_prefix(&role.principal_arn_prefix)
                    .is_some_and(|session| !session.is_empty() && !session.contains('/'))
                && identity
                    .user_id
                    .strip_prefix(&role.user_id_prefix)
                    .is_some_and(|session| !session.is_empty() && !session.contains(':'))
        })
        .collect::<Vec<_>>();
    if matches.len() == 1 {
        Ok(matches[0])
    } else {
        Err(ExperimentError::UnauthorizedPrincipal)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ResearchArtifactType {
    ExperimentDefinition,
    ExperimentStateTransition,
    RosterManifest,
    SourceRegisterSnapshot,
    CommitmentG,
    CommitmentD,
    CommitmentC,
    CommitmentE,
    CommitmentP,
    TimestampReceipt,
    CustodyEvent,
    CustodyCheckpoint,
    DisclosureProgram,
    DisclosureReceipt,
    EpisodeManifest,
    ExecutionAttempt,
    SemanticComparison,
    ResearchReport,
    UserAuthorization,
    AtomicClaimRegistry,
    InvalidationEvent,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ExperimentStateTransition {
    pub state_transition_id: String,
    pub experiment_id: String,
    pub experiment_version_id: String,
    pub sequence: u32,
    pub from_state: ExperimentState,
    pub to_state: ExperimentState,
    pub previous_state_artifact_id: String,
    pub previous_state_content_hash: String,
    pub authorized_by_role: String,
    pub transition_evidence_refs: Vec<LineageRef>,
    pub recorded_at: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ExperimentInvalidationEvent {
    pub invalidation_id: String,
    pub experiment_id: String,
    pub experiment_version_id: String,
    pub invalidated_artifact_id: String,
    pub invalidated_content_hash: String,
    pub reason_code: String,
    pub authorized_by_role: String,
    pub recorded_at: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EvidenceState {
    Active,
    Quarantined,
    Revoked,
    Expired,
    Invalidated,
}

impl EvidenceState {
    pub fn validate_transition(self, next: Self) -> Result<(), ExperimentError> {
        let valid = match self {
            Self::Active => next != Self::Active,
            Self::Quarantined => matches!(next, Self::Revoked | Self::Expired | Self::Invalidated),
            Self::Revoked | Self::Expired | Self::Invalidated => false,
        };
        if valid {
            Ok(())
        } else {
            Err(invalid(
                "artifact.evidence_state",
                "terminal state cannot be promoted",
            ))
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ResearchArtifactEnvelope {
    pub artifact_schema_version: String,
    pub artifact_type: ResearchArtifactType,
    pub artifact_id: String,
    pub experiment_id: String,
    pub experiment_version_id: String,
    pub artifact_uri: String,
    pub content_hash: String,
    pub semantic_hash: Option<String>,
    pub byte_length: u64,
    pub created_at: String,
    pub created_by_role: String,
    pub lineage_refs: Vec<LineageRef>,
    pub source_entry_refs: Vec<String>,
    pub index_lifecycle_state: LifecycleState,
    pub evidence_state: EvidenceState,
    pub invalidated_by_refs: Vec<String>,
}

impl ResearchArtifactEnvelope {
    pub fn validate(
        &self,
        artifact_root: &ResolvedArtifactRoot,
        roles: &BTreeSet<&str>,
    ) -> Result<(), ExperimentError> {
        for (field, value) in [
            ("artifact.artifact_id", self.artifact_id.as_str()),
            ("artifact.experiment_id", self.experiment_id.as_str()),
            (
                "artifact.experiment_version_id",
                self.experiment_version_id.as_str(),
            ),
            ("artifact.artifact_uri", self.artifact_uri.as_str()),
            ("artifact.created_by_role", self.created_by_role.as_str()),
        ] {
            required(field, value)?;
        }
        hash(
            "artifact.experiment_version_id",
            &self.experiment_version_id,
        )?;
        if self.artifact_schema_version != ARTIFACT_SCHEMA_VERSION
            || self.byte_length == 0
            || !roles.contains(self.created_by_role.as_str())
        {
            return Err(invalid(
                "artifact",
                "unknown schema, empty payload, or unauthorized role",
            ));
        }
        hash("artifact.content_hash", &self.content_hash)?;
        if let Some(semantic_hash) = &self.semantic_hash {
            hash("artifact.semantic_hash", semantic_hash)?;
        }
        parse_time("artifact.created_at", &self.created_at)?;
        let mut lineage_ids = BTreeSet::new();
        for lineage in &self.lineage_refs {
            required("artifact.lineage_refs.artifact_id", &lineage.artifact_id)?;
            hash("artifact.lineage_refs.content_hash", &lineage.content_hash)?;
            if !lineage_ids.insert((lineage.artifact_kind, lineage.artifact_id.as_str())) {
                return Err(invalid(
                    "artifact.lineage_refs",
                    "duplicate lineage identity",
                ));
            }
        }
        for source_ref in &self.source_entry_refs {
            required("artifact.source_entry_refs", source_ref)?;
        }
        for invalidation_ref in &self.invalidated_by_refs {
            required("artifact.invalidated_by_refs", invalidation_ref)?;
        }
        let prefix = format!(
            "{}/experiment-contracts/{}/{}/",
            artifact_root.typed_root(StoreArtifactKind::ResearchAnalytics),
            self.experiment_id,
            self.experiment_version_id,
        );
        if !self.artifact_uri.starts_with(&prefix)
            || self.lineage_refs.is_empty()
            || (self.evidence_state == EvidenceState::Active
                && !self.invalidated_by_refs.is_empty())
        {
            return Err(invalid("artifact", "bad URI, lineage, or lifecycle state"));
        }
        Ok(())
    }
}

#[derive(Debug, Clone)]
pub struct ExperimentRegistrationPlan {
    pub payload_uri: String,
    pub envelope_uri: String,
    pub payload_bytes: Vec<u8>,
    pub envelope_bytes: Vec<u8>,
    pub envelope: ResearchArtifactEnvelope,
    pub store_event: StoreIndexEvent,
    pub expected_parent_version_id: Option<String>,
    pub credential_scope_ref: String,
    pub semantic_hash: String,
}

impl ExperimentRegistrationPlan {
    pub fn verify_clean(&self, payload: &[u8]) -> Result<(), ExperimentError> {
        let envelope =
            serde_json::to_vec(&self.envelope).map_err(|_| ExperimentError::DirtyArtifact)?;
        if payload != self.payload_bytes
            || sha256_hex(payload) != self.envelope.content_hash
            || envelope != self.envelope_bytes
        {
            Err(ExperimentError::DirtyArtifact)
        } else {
            Ok(())
        }
    }
}

pub fn build_registration_plan(
    experiment: &ValidatedExperiment,
    caller_role: &RoleBinding,
    expected_parent_version_id: Option<&str>,
    artifact_root: &ResolvedArtifactRoot,
) -> Result<ExperimentRegistrationPlan, ExperimentError> {
    let definition = &experiment.definition;
    if caller_role.role_id != definition.experiment.append_role || !caller_role.can_append_versions
    {
        return Err(ExperimentError::UnauthorizedPrincipal);
    }
    if caller_role.credential_scope_ref != definition.storage.credential_scope_ref {
        return Err(ExperimentError::UnauthorizedPrincipal);
    }
    if definition.experiment.parent_version_id.as_deref() != expected_parent_version_id {
        return Err(ExperimentError::StaleParent);
    }
    if artifact_root.artifact_root_uri() != definition.storage.artifact_root {
        return Err(invalid(
            "storage.artifact_root",
            "resolved artifact-store root mismatch",
        ));
    }
    let artifact_id = format!(
        "experiment-{}-{}",
        definition.experiment.experiment_id, experiment.version_id
    );
    let base_uri = format!(
        "{}/experiment-contracts/{}/{}",
        artifact_root.typed_root(StoreArtifactKind::ResearchAnalytics),
        definition.experiment.experiment_id,
        experiment.version_id
    );
    let payload_uri = format!("{base_uri}/experiment-definition.toml");
    let envelope_uri = format!("{base_uri}/envelope.json");
    let roles = definition
        .roles
        .bindings
        .iter()
        .map(|role| role.role_id.as_str())
        .collect::<BTreeSet<_>>();
    let envelope = ResearchArtifactEnvelope {
        artifact_schema_version: ARTIFACT_SCHEMA_VERSION.to_string(),
        artifact_type: ResearchArtifactType::ExperimentDefinition,
        artifact_id: artifact_id.clone(),
        experiment_id: definition.experiment.experiment_id.clone(),
        experiment_version_id: experiment.version_id.clone(),
        artifact_uri: payload_uri.clone(),
        content_hash: experiment.original_hash.clone(),
        semantic_hash: Some(experiment.semantic_hash.clone()),
        byte_length: experiment.original_bytes.len() as u64,
        created_at: definition.experiment.created_at.clone(),
        created_by_role: caller_role.role_id.clone(),
        lineage_refs: definition.experiment.lineage_refs.clone(),
        source_entry_refs: definition
            .target_frame
            .inventory_source_refs
            .iter()
            .map(|binding| binding.source_entry_id.clone())
            .collect(),
        index_lifecycle_state: LifecycleState::Active,
        evidence_state: EvidenceState::Active,
        invalidated_by_refs: Vec::new(),
    };
    envelope.validate(artifact_root, &roles)?;
    let envelope_bytes = serde_json::to_vec(&envelope)
        .map_err(|error| ExperimentError::Registration(error.to_string()))?;
    let store_event = StoreIndexEvent {
        schema_version: ARTIFACT_SCHEMA_VERSION.to_string(),
        created_at: definition.experiment.created_at.clone(),
        event_id: artifact_id.clone(),
        artifact_kind: StoreArtifactKind::ResearchAnalytics,
        artifact_id,
        artifact_uri: payload_uri.clone(),
        manifest_uri: envelope_uri.clone(),
        producer_project: definition.storage.producer_project.clone(),
        owner_project: definition.storage.owner_project.clone(),
        content_sha256: experiment.original_hash.clone(),
        lifecycle_state: StoreLifecycleState::Active,
        storage_profile: ArtifactStorageProfile::Active,
        parent_lineage: envelope
            .lineage_refs
            .iter()
            .map(|lineage| StoreLineageRef {
                artifact_kind: lineage.artifact_kind,
                artifact_id: lineage.artifact_id.clone(),
                version: lineage.artifact_version.map(|version| version.to_string()),
                sha256: lineage.content_hash.clone(),
            })
            .collect(),
        commit_state: StoreCommitState::Staged,
    };
    let plan = ExperimentRegistrationPlan {
        payload_uri,
        envelope_uri,
        payload_bytes: experiment.original_bytes.clone(),
        envelope_bytes,
        envelope,
        store_event,
        expected_parent_version_id: expected_parent_version_id.map(str::to_string),
        credential_scope_ref: caller_role.credential_scope_ref.clone(),
        semantic_hash: experiment.semantic_hash.clone(),
    };
    plan.verify_clean(&experiment.original_bytes)?;
    Ok(plan)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum RegistrationDisposition {
    Committed,
    AlreadyCommitted,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct RegistrationSummary {
    pub experiment_id: String,
    pub version_id: String,
    pub artifact_id: String,
    pub content_hash: String,
    pub semantic_hash: String,
    pub snapshot_id: String,
    pub disposition: RegistrationDisposition,
    pub audit_intent_uri: Option<String>,
    pub provider_calls: u64,
    pub incremental_provider_spend_usd: String,
}

pub async fn register_version(
    spec_path: &Path,
    expected_parent_version_id: Option<&str>,
) -> Result<RegistrationSummary, ExperimentError> {
    let experiment = load_and_validate_experiment(spec_path)?;
    let identity = resolve_sts_caller_identity().await?;
    let caller_role = match_caller_role(&experiment.definition, &identity)?;
    let storage = &experiment.definition.storage;
    let store_bytes = fs::read(&storage.artifact_store_config_path)
        .map_err(|error| ExperimentError::Io(error.to_string()))?;
    let store_config = parse_bound_artifact_store_config(storage, &store_bytes)?;
    let artifact_root = store_config.artifact_root();
    let plan = build_registration_plan(
        &experiment,
        caller_role,
        expected_parent_version_id,
        artifact_root,
    )?;
    register_plan(&experiment, &plan, artifact_root).await
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BoundArtifactStoreConfig {
    config_hash: String,
    artifact_root: ResolvedArtifactRoot,
}

impl BoundArtifactStoreConfig {
    #[must_use]
    pub fn config_hash(&self) -> &str {
        &self.config_hash
    }

    #[must_use]
    pub fn artifact_root(&self) -> &ResolvedArtifactRoot {
        &self.artifact_root
    }
}

pub fn parse_bound_artifact_store_config(
    storage: &StoragePolicy,
    bytes: &[u8],
) -> Result<BoundArtifactStoreConfig, ExperimentError> {
    if sha256_hex(bytes) != storage.artifact_store_config_hash {
        return Err(ExperimentError::DirtyArtifact);
    }
    let text =
        std::str::from_utf8(bytes).map_err(|error| ExperimentError::Parse(error.to_string()))?;
    let config: ArtifactStoreConfig =
        toml::from_str(text).map_err(|error| ExperimentError::Parse(error.to_string()))?;
    let root = config
        .resolve()
        .map_err(|error| ExperimentError::Registration(error.to_string()))?;
    if root.artifact_root_uri() != storage.artifact_root {
        return Err(invalid(
            "storage.artifact_root",
            "artifact-store root mismatch",
        ));
    }
    Ok(BoundArtifactStoreConfig {
        config_hash: storage.artifact_store_config_hash.clone(),
        artifact_root: root,
    })
}

async fn register_plan(
    experiment: &ValidatedExperiment,
    plan: &ExperimentRegistrationPlan,
    root: &ResolvedArtifactRoot,
) -> Result<RegistrationSummary, ExperimentError> {
    plan.verify_clean(&experiment.original_bytes)?;
    let storage = &experiment.definition.storage;
    if plan.credential_scope_ref != storage.credential_scope_ref {
        return Err(ExperimentError::UnauthorizedPrincipal);
    }

    let ssm = &storage.ssm_parameters;
    let ssm_config = aws_config::defaults(BehaviorVersion::latest())
        .region(aws_sdk_ssm::config::Region::new(ssm.region.clone()))
        .load()
        .await;
    let ssm_client = aws_sdk_ssm::Client::new(&ssm_config);
    let credentials = S3ArtifactStoreCredentials::new(
        resolve_ssm_parameter(&ssm_client, &ssm.access_key_id).await?,
        resolve_ssm_parameter(&ssm_client, &ssm.secret_access_key).await?,
        match &ssm.session_token {
            Some(path) => Some(resolve_ssm_parameter(&ssm_client, path).await?),
            None => None,
        },
    )
    .map_err(|error| ExperimentError::Registration(error.to_string()))?;
    let object_store = root
        .build_s3_object_store_with_credentials(&credentials)
        .map_err(|error| ExperimentError::Registration(error.to_string()))?;
    let authority = ArtifactIndexWriteAuthority::new(
        storage.writer_id.clone(),
        [StoreArtifactKind::ResearchAnalytics],
    )
    .map_err(|error| ExperimentError::Registration(error.to_string()))?;
    let writer = ArtifactIndexWriter::with_authority(&object_store, authority);
    let observed =
        read_stable_snapshot(&writer, root, storage.registration_read_retry_limit).await?;
    if let Some((pointer, snapshot)) = &observed
        && existing_registration_is_identical(
            experiment,
            plan,
            snapshot,
            &object_store,
            root,
            ValidationMode::Production,
        )
        .await?
    {
        return Ok(RegistrationSummary {
            experiment_id: experiment.definition.experiment.experiment_id.clone(),
            version_id: experiment.version_id.clone(),
            artifact_id: plan.envelope.artifact_id.clone(),
            content_hash: plan.envelope.content_hash.clone(),
            semantic_hash: plan.semantic_hash.clone(),
            snapshot_id: pointer.pointer.snapshot_id.clone(),
            disposition: RegistrationDisposition::AlreadyCommitted,
            audit_intent_uri: None,
            provider_calls: 0,
            incremental_provider_spend_usd: experiment
                .definition
                .source_policy
                .incremental_spend_cap_usd
                .clone(),
        });
    }
    validate_observed_parent(
        experiment,
        observed.as_ref().map(|(_, snapshot)| snapshot),
        &object_store,
        root,
        ValidationMode::Production,
    )
    .await?;
    validate_active_lineage(&writer, root, &plan.store_event.parent_lineage).await?;

    let create_only = CreateOnlyArtifactWriter::new(&object_store);
    let payload_path = root
        .object_path_for_uri(&plan.payload_uri)
        .map_err(|error| ExperimentError::Registration(error.to_string()))?;
    create_only
        .put_create_idempotent(&payload_path, plan.payload_bytes.clone())
        .await
        .map_err(|error| ExperimentError::Registration(error.to_string()))?;
    let envelope_path = root
        .object_path_for_uri(&plan.envelope_uri)
        .map_err(|error| ExperimentError::Registration(error.to_string()))?;
    create_only
        .put_create_idempotent(&envelope_path, plan.envelope_bytes.clone())
        .await
        .map_err(|error| ExperimentError::Registration(error.to_string()))?;

    let outcome = writer
        .commit_event_from_observed_latest(
            root,
            StoreCommitPlan {
                event: plan.store_event.clone(),
                snapshot_ids: vec![registration_snapshot_id(
                    storage,
                    &plan.store_event,
                    observed
                        .as_ref()
                        .map(|(pointer, _)| pointer.pointer.snapshot_id.as_str()),
                )],
                writer_id: storage.writer_id.clone(),
            },
            observed.map(|(pointer, _)| pointer),
        )
        .await
        .map_err(map_registration_commit_error)?;
    Ok(RegistrationSummary {
        experiment_id: experiment.definition.experiment.experiment_id.clone(),
        version_id: experiment.version_id.clone(),
        artifact_id: plan.envelope.artifact_id.clone(),
        content_hash: plan.envelope.content_hash.clone(),
        semantic_hash: plan.semantic_hash.clone(),
        snapshot_id: outcome.snapshot_id,
        disposition: RegistrationDisposition::Committed,
        audit_intent_uri: Some(outcome.audit_intent_uri),
        provider_calls: 0,
        incremental_provider_spend_usd: experiment
            .definition
            .source_policy
            .incremental_spend_cap_usd
            .clone(),
    })
}

fn map_registration_commit_error(error: anyhow::Error) -> ExperimentError {
    if error
        .downcast_ref::<ArtifactIndexPointerConflict>()
        .is_some()
    {
        ExperimentError::StaleParent
    } else {
        ExperimentError::Registration(error.to_string())
    }
}

async fn existing_registration_is_identical(
    experiment: &ValidatedExperiment,
    plan: &ExperimentRegistrationPlan,
    snapshot: &ArtifactIndexSnapshot,
    store: &dyn ObjectStore,
    root: &ResolvedArtifactRoot,
    mode: ValidationMode,
) -> Result<bool, ExperimentError> {
    let Some(row) = snapshot
        .rows
        .iter()
        .find(|row| row.artifact_id == plan.store_event.artifact_id)
    else {
        return Ok(false);
    };
    let expected = crate::artifact_store::ArtifactIndexSnapshotRow::from_event(
        &plan.store_event,
        StoreCommitState::Committed,
    );
    if row != &expected {
        return Err(ExperimentError::DirtyArtifact);
    }
    let existing = load_verified_definition(row, experiment, store, root, mode).await?;
    if existing.experiment.original_bytes != experiment.original_bytes
        || existing.experiment.semantic_hash != experiment.semantic_hash
    {
        return Err(ExperimentError::DirtyArtifact);
    }
    Ok(true)
}

fn registration_snapshot_id(
    storage: &StoragePolicy,
    event: &StoreIndexEvent,
    initial_prior_snapshot_id: Option<&str>,
) -> String {
    #[derive(Serialize)]
    struct SnapshotAttemptIdentity<'a> {
        schema_version: &'static str,
        namespace: &'a str,
        initial_prior_snapshot_id: Option<&'a str>,
        event_id: &'a str,
        artifact_id: &'a str,
        content_sha256: &'a str,
    }

    sha256_hex(
        &serde_json::to_vec(&SnapshotAttemptIdentity {
            schema_version: "pump-research-registration-snapshot.v1",
            namespace: &storage.snapshot_id_namespace,
            initial_prior_snapshot_id,
            event_id: &event.event_id,
            artifact_id: &event.artifact_id,
            content_sha256: &event.content_sha256,
        })
        .expect("typed snapshot-attempt serialization is infallible"),
    )
}

async fn resolve_ssm_parameter(
    client: &aws_sdk_ssm::Client,
    path: &str,
) -> Result<String, ExperimentError> {
    client
        .get_parameter()
        .name(path)
        .with_decryption(true)
        .send()
        .await
        .map_err(|_| ExperimentError::Registration("SSM credential resolution failed".into()))?
        .parameter()
        .and_then(|parameter| parameter.value())
        .filter(|value| !value.is_empty())
        .map(str::to_string)
        .ok_or_else(|| ExperimentError::Registration("SSM credential resolution failed".into()))
}

async fn validate_active_lineage(
    writer: &ArtifactIndexWriter<'_>,
    root: &crate::artifact_store::ResolvedArtifactRoot,
    lineage: &[StoreLineageRef],
) -> Result<(), ExperimentError> {
    for declared in lineage {
        let parent = writer
            .read_committed_row(root, declared.artifact_kind, &declared.artifact_id)
            .await
            .map_err(|error| ExperimentError::Registration(error.to_string()))?
            .ok_or_else(|| invalid("experiment.lineage_refs", "parent is not committed"))?;
        if parent.content_sha256 != declared.sha256
            || parent.lifecycle_state != StoreLifecycleState::Active
        {
            return Err(invalid(
                "experiment.lineage_refs",
                "parent is mismatched or inactive",
            ));
        }
    }
    Ok(())
}

async fn read_stable_snapshot(
    writer: &ArtifactIndexWriter<'_>,
    root: &crate::artifact_store::ResolvedArtifactRoot,
    retry_limit: u32,
) -> Result<
    Option<(
        crate::artifact_store::StoredArtifactIndexPointer,
        crate::artifact_store::ArtifactIndexSnapshot,
    )>,
    ExperimentError,
> {
    for _ in 0..retry_limit {
        let before = writer
            .read_latest_pointer(root, StoreArtifactKind::ResearchAnalytics)
            .await
            .map_err(|error| ExperimentError::Registration(error.to_string()))?;
        let Some(before) = before else {
            if writer
                .read_latest_pointer(root, StoreArtifactKind::ResearchAnalytics)
                .await
                .map_err(|error| ExperimentError::Registration(error.to_string()))?
                .is_none()
            {
                return Ok(None);
            }
            continue;
        };
        let snapshot = writer
            .read_verified_latest_snapshot(root, StoreArtifactKind::ResearchAnalytics)
            .await
            .map_err(|error| ExperimentError::Registration(error.to_string()))?;
        let after = writer
            .read_latest_pointer(root, StoreArtifactKind::ResearchAnalytics)
            .await
            .map_err(|error| ExperimentError::Registration(error.to_string()))?;
        if after.as_ref().map(|item| &item.pointer) == Some(&before.pointer)
            && snapshot.snapshot_id == before.pointer.snapshot_id
        {
            return Ok(Some((before, snapshot)));
        }
    }
    Err(ExperimentError::StaleParent)
}

async fn validate_observed_parent(
    experiment: &ValidatedExperiment,
    snapshot: Option<&ArtifactIndexSnapshot>,
    store: &dyn ObjectStore,
    root: &ResolvedArtifactRoot,
    mode: ValidationMode,
) -> Result<(), ExperimentError> {
    let family_prefix = format!(
        "{}/experiment-contracts/{}/",
        root.typed_root(StoreArtifactKind::ResearchAnalytics),
        experiment.definition.experiment.experiment_id
    );
    let rows = snapshot
        .map(|snapshot| {
            snapshot
                .rows
                .iter()
                .filter(|row| {
                    row.artifact_kind == StoreArtifactKind::ResearchAnalytics
                        && row.artifact_uri.starts_with(&family_prefix)
                })
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    let definition_rows = rows
        .iter()
        .copied()
        .filter(|row| definition_version_from_uri(&family_prefix, &row.artifact_uri).is_some())
        .collect::<Vec<_>>();
    let child = &experiment.definition.experiment;
    let Some(parent_version_id) = child.parent_version_id.as_deref() else {
        return if rows.is_empty() {
            Ok(())
        } else {
            Err(ExperimentError::StaleParent)
        };
    };
    if definition_rows.is_empty() {
        return Err(ExperimentError::StaleParent);
    }
    let expected_hash = child
        .parent_content_hash
        .as_deref()
        .ok_or(ExperimentError::StaleParent)?;
    let mut definitions = BTreeMap::new();
    for row in definition_rows {
        let observed = load_verified_definition(row, experiment, store, root, mode).await?;
        if definitions
            .insert(observed.experiment.version_id.clone(), observed)
            .is_some()
        {
            return Err(ExperimentError::StaleParent);
        }
    }
    let mut genesis_count = 0_usize;
    let mut referenced_parent_versions = BTreeSet::new();
    for observed in definitions.values() {
        let version = &observed.experiment.definition.experiment;
        if version.version_sequence == 1 {
            genesis_count += 1;
            continue;
        }
        let declared_parent_id = version
            .parent_version_id
            .as_deref()
            .ok_or(ExperimentError::StaleParent)?;
        let declared_parent = definitions
            .get(declared_parent_id)
            .ok_or(ExperimentError::StaleParent)?;
        let expected_sequence = declared_parent
            .experiment
            .definition
            .experiment
            .version_sequence
            .checked_add(1)
            .ok_or(ExperimentError::StaleParent)?;
        let expected_parent_artifact_id =
            format!("experiment-{}-{declared_parent_id}", child.experiment_id);
        if version.version_sequence != expected_sequence
            || version.parent_content_hash.as_deref()
                != Some(declared_parent.experiment.original_hash.as_str())
            || !observed.row.parent_lineage.iter().any(|lineage| {
                lineage.artifact_kind == StoreArtifactKind::ResearchAnalytics
                    && lineage.artifact_id == expected_parent_artifact_id
                    && lineage.sha256 == declared_parent.experiment.original_hash
            })
        {
            return Err(ExperimentError::StaleParent);
        }
        referenced_parent_versions.insert(declared_parent_id.to_string());
    }
    if genesis_count != 1 {
        return Err(ExperimentError::StaleParent);
    }
    let heads = definitions
        .iter()
        .filter(|(version_id, _)| !referenced_parent_versions.contains(version_id.as_str()))
        .map(|(_, observed)| observed)
        .collect::<Vec<_>>();
    if heads.len() != 1 {
        return Err(ExperimentError::StaleParent);
    }
    let observed_parent = heads[0];
    let parent = &observed_parent.experiment;
    let definition_row = observed_parent.row;
    if parent.version_id != parent_version_id || parent.original_hash != expected_hash {
        return Err(ExperimentError::StaleParent);
    }
    let version_prefix = format!("{family_prefix}{parent_version_id}/");

    let state_prefix = format!("{version_prefix}state-transitions/");
    let state_rows = rows
        .iter()
        .copied()
        .filter(|row| row.artifact_uri.starts_with(&state_prefix))
        .collect::<Vec<_>>();
    let referenced = state_rows
        .iter()
        .flat_map(|row| {
            row.parent_lineage
                .iter()
                .map(|parent| parent.artifact_id.as_str())
        })
        .collect::<BTreeSet<_>>();
    let heads = state_rows
        .iter()
        .copied()
        .filter(|row| !referenced.contains(row.artifact_id.as_str()))
        .collect::<Vec<_>>();
    let parent_state = if state_rows.is_empty() {
        ExperimentState::Draft
    } else {
        if heads.len() != 1 {
            return Err(ExperimentError::StaleParent);
        }
        load_verified_state_chain(
            heads[0],
            &state_rows,
            definition_row,
            snapshot.ok_or(ExperimentError::StaleParent)?,
            parent,
            store,
            root,
        )
        .await?
    };
    validate_parent_sequence_and_state(child, parent, parent_state)
}

struct ObservedExperimentDefinition<'a> {
    row: &'a crate::artifact_store::ArtifactIndexSnapshotRow,
    experiment: ValidatedExperiment,
}

fn definition_version_from_uri<'a>(family_prefix: &str, uri: &'a str) -> Option<&'a str> {
    let suffix = uri.strip_prefix(family_prefix)?;
    let version_id = suffix.strip_suffix("/experiment-definition.toml")?;
    if version_id.is_empty() || version_id.contains('/') {
        None
    } else {
        Some(version_id)
    }
}

async fn load_verified_definition<'a>(
    row: &'a crate::artifact_store::ArtifactIndexSnapshotRow,
    expected_family: &ValidatedExperiment,
    store: &dyn ObjectStore,
    root: &ResolvedArtifactRoot,
    mode: ValidationMode,
) -> Result<ObservedExperimentDefinition<'a>, ExperimentError> {
    if row.lifecycle_state != StoreLifecycleState::Active
        || row.commit_state != StoreCommitState::Committed
    {
        return Err(ExperimentError::StaleParent);
    }
    let (envelope, payload) = load_indexed_artifact(row, store, root).await?;
    let experiment =
        parse_and_validate(payload.as_ref(), mode).map_err(|_| ExperimentError::StaleParent)?;
    let roles = experiment
        .definition
        .roles
        .bindings
        .iter()
        .map(|role| role.role_id.as_str())
        .collect::<BTreeSet<_>>();
    envelope
        .validate(root, &roles)
        .map_err(|_| ExperimentError::StaleParent)?;
    let definition = &experiment.definition;
    let expected_artifact_id = format!(
        "experiment-{}-{}",
        definition.experiment.experiment_id, experiment.version_id
    );
    let expected_base_uri = format!(
        "{}/experiment-contracts/{}/{}",
        root.typed_root(StoreArtifactKind::ResearchAnalytics),
        definition.experiment.experiment_id,
        experiment.version_id
    );
    if envelope.artifact_type != ResearchArtifactType::ExperimentDefinition
        || envelope.artifact_id != expected_artifact_id
        || envelope.artifact_id != row.artifact_id
        || envelope.experiment_id != definition.experiment.experiment_id
        || envelope.experiment_version_id != experiment.version_id
        || envelope.artifact_uri != format!("{expected_base_uri}/experiment-definition.toml")
        || envelope.artifact_uri != row.artifact_uri
        || row.manifest_uri != format!("{expected_base_uri}/envelope.json")
        || envelope.content_hash != experiment.original_hash
        || envelope.content_hash != row.content_sha256
        || envelope.semantic_hash.as_deref() != Some(experiment.semantic_hash.as_str())
        || envelope.byte_length != payload.len() as u64
        || envelope.created_at != definition.experiment.created_at
        || envelope.created_at != row.created_at
        || envelope.created_by_role != definition.experiment.append_role
        || envelope.index_lifecycle_state != LifecycleState::Active
        || envelope.evidence_state != EvidenceState::Active
        || definition.experiment.experiment_id
            != expected_family.definition.experiment.experiment_id
        || definition.storage.artifact_root != expected_family.definition.storage.artifact_root
        || definition.storage.producer_project
            != expected_family.definition.storage.producer_project
        || definition.storage.owner_project != expected_family.definition.storage.owner_project
        || row.producer_project != definition.storage.producer_project
        || row.owner_project != definition.storage.owner_project
        || !lineage_matches(row, &envelope)
    {
        return Err(ExperimentError::StaleParent);
    }
    Ok(ObservedExperimentDefinition { row, experiment })
}

async fn load_indexed_artifact(
    row: &crate::artifact_store::ArtifactIndexSnapshotRow,
    store: &dyn ObjectStore,
    root: &ResolvedArtifactRoot,
) -> Result<(ResearchArtifactEnvelope, bytes::Bytes), ExperimentError> {
    let envelope_path = root
        .object_path_for_uri(&row.manifest_uri)
        .map_err(|_| ExperimentError::StaleParent)?;
    let envelope_bytes = store
        .get(&envelope_path)
        .await
        .map_err(|_| ExperimentError::StaleParent)?
        .bytes()
        .await
        .map_err(|_| ExperimentError::StaleParent)?;
    let envelope = serde_json::from_slice::<ResearchArtifactEnvelope>(&envelope_bytes)
        .map_err(|_| ExperimentError::StaleParent)?;
    let payload_path = root
        .object_path_for_uri(&row.artifact_uri)
        .map_err(|_| ExperimentError::StaleParent)?;
    let payload = store
        .get(&payload_path)
        .await
        .map_err(|_| ExperimentError::StaleParent)?
        .bytes()
        .await
        .map_err(|_| ExperimentError::StaleParent)?;
    if sha256_hex(payload.as_ref()) != row.content_sha256 {
        return Err(ExperimentError::StaleParent);
    }
    Ok((envelope, payload))
}

fn lineage_matches(
    row: &crate::artifact_store::ArtifactIndexSnapshotRow,
    envelope: &ResearchArtifactEnvelope,
) -> bool {
    let indexed = row
        .parent_lineage
        .iter()
        .map(|item| {
            (
                item.artifact_kind,
                item.artifact_id.clone(),
                item.version.clone(),
                item.sha256.clone(),
            )
        })
        .collect::<BTreeSet<_>>();
    let enveloped = envelope
        .lineage_refs
        .iter()
        .map(|item| {
            (
                item.artifact_kind,
                item.artifact_id.clone(),
                item.artifact_version.map(|version| version.to_string()),
                item.content_hash.clone(),
            )
        })
        .collect::<BTreeSet<_>>();
    indexed.len() == row.parent_lineage.len()
        && enveloped.len() == envelope.lineage_refs.len()
        && indexed == enveloped
}

async fn load_verified_state_chain(
    head: &crate::artifact_store::ArtifactIndexSnapshotRow,
    state_rows: &[&crate::artifact_store::ArtifactIndexSnapshotRow],
    definition_row: &crate::artifact_store::ArtifactIndexSnapshotRow,
    snapshot: &ArtifactIndexSnapshot,
    parent: &ValidatedExperiment,
    store: &dyn ObjectStore,
    root: &ResolvedArtifactRoot,
) -> Result<ExperimentState, ExperimentError> {
    let rows_by_id = state_rows
        .iter()
        .map(|row| (row.artifact_id.as_str(), *row))
        .collect::<BTreeMap<_, _>>();
    if rows_by_id.len() != state_rows.len() {
        return Err(ExperimentError::StaleParent);
    }
    let mut chain = Vec::new();
    let mut visited = BTreeSet::new();
    let mut current_id = head.artifact_id.clone();
    while current_id != definition_row.artifact_id {
        if !visited.insert(current_id.clone()) {
            return Err(ExperimentError::StaleParent);
        }
        let row = rows_by_id
            .get(current_id.as_str())
            .copied()
            .ok_or(ExperimentError::StaleParent)?;
        let transition = load_verified_state_transition(row, snapshot, parent, store, root).await?;
        current_id = transition.previous_state_artifact_id.clone();
        chain.push((row, transition));
    }
    if visited.len() != state_rows.len() {
        return Err(ExperimentError::StaleParent);
    }

    let mut state = ExperimentState::Draft;
    let mut predecessor_id = definition_row.artifact_id.as_str();
    let mut predecessor_hash = definition_row.content_sha256.as_str();
    for (index, (row, transition)) in chain.iter().rev().enumerate() {
        if transition.sequence
            != u32::try_from(index + 1).map_err(|_| ExperimentError::StaleParent)?
            || transition.from_state != state
            || transition.previous_state_artifact_id != predecessor_id
            || transition.previous_state_content_hash != predecessor_hash
            || !state.allows_transition_to(transition.to_state)
        {
            return Err(ExperimentError::StaleParent);
        }
        state = transition.to_state;
        predecessor_id = row.artifact_id.as_str();
        predecessor_hash = row.content_sha256.as_str();
    }
    Ok(state)
}

async fn load_verified_state_transition(
    row: &crate::artifact_store::ArtifactIndexSnapshotRow,
    snapshot: &ArtifactIndexSnapshot,
    parent: &ValidatedExperiment,
    store: &dyn ObjectStore,
    root: &ResolvedArtifactRoot,
) -> Result<ExperimentStateTransition, ExperimentError> {
    if row.lifecycle_state != StoreLifecycleState::Active
        || row.commit_state != StoreCommitState::Committed
    {
        return Err(ExperimentError::StaleParent);
    }
    let (envelope, payload) = load_indexed_artifact(row, store, root).await?;
    let roles = parent
        .definition
        .roles
        .bindings
        .iter()
        .map(|role| role.role_id.as_str())
        .collect::<BTreeSet<_>>();
    envelope
        .validate(root, &roles)
        .map_err(|_| ExperimentError::StaleParent)?;
    let transition = serde_json::from_slice::<ExperimentStateTransition>(&payload)
        .map_err(|_| ExperimentError::StaleParent)?;
    let authorized_role = parent
        .definition
        .roles
        .bindings
        .iter()
        .find(|role| role.role_id == transition.authorized_by_role)
        .ok_or(ExperimentError::StaleParent)?;
    if envelope.artifact_type != ResearchArtifactType::ExperimentStateTransition
        || envelope.artifact_id != row.artifact_id
        || envelope.experiment_id != parent.definition.experiment.experiment_id
        || envelope.experiment_version_id != parent.version_id
        || envelope.artifact_uri != row.artifact_uri
        || envelope.content_hash != row.content_sha256
        || envelope.byte_length != payload.len() as u64
        || envelope.created_by_role != transition.authorized_by_role
        || envelope.index_lifecycle_state != LifecycleState::Active
        || envelope.evidence_state != EvidenceState::Active
        || transition.state_transition_id != row.artifact_id
        || transition.experiment_id != parent.definition.experiment.experiment_id
        || transition.experiment_version_id != parent.version_id
        || transition.sequence == 0
        || transition.from_state == transition.to_state
        || transition.transition_evidence_refs.is_empty()
        || transition.recorded_at != envelope.created_at
        || authorized_role.purpose != RolePurpose::GovernanceApproval
        || !lineage_matches(row, &envelope)
    {
        return Err(ExperimentError::StaleParent);
    }
    parse_time("state_transition.recorded_at", &transition.recorded_at)
        .map_err(|_| ExperimentError::StaleParent)?;
    hash(
        "state_transition.previous_state_content_hash",
        &transition.previous_state_content_hash,
    )
    .map_err(|_| ExperimentError::StaleParent)?;
    let predecessor_matches = envelope.lineage_refs.iter().any(|lineage| {
        lineage.artifact_kind == StoreArtifactKind::ResearchAnalytics
            && lineage.artifact_id == transition.previous_state_artifact_id
            && lineage.content_hash == transition.previous_state_content_hash
    });
    if !predecessor_matches {
        return Err(ExperimentError::StaleParent);
    }
    if envelope.lineage_refs.len() != transition.transition_evidence_refs.len() + 1 {
        return Err(ExperimentError::StaleParent);
    }
    let expected_evidence_type = required_transition_evidence_type(transition.to_state)
        .ok_or(ExperimentError::StaleParent)?;
    let mut evidence_ids = BTreeSet::new();
    let mut typed_evidence_found = false;
    for evidence in &transition.transition_evidence_refs {
        if !evidence_ids.insert((evidence.artifact_kind, evidence.artifact_id.as_str()))
            || evidence.artifact_kind != StoreArtifactKind::ResearchAnalytics
        {
            return Err(ExperimentError::StaleParent);
        }
        let evidence_row = snapshot
            .rows
            .iter()
            .find(|candidate| {
                candidate.artifact_kind == evidence.artifact_kind
                    && candidate.artifact_id == evidence.artifact_id
                    && candidate.content_sha256 == evidence.content_hash
                    && candidate.lifecycle_state == StoreLifecycleState::Active
                    && candidate.commit_state == StoreCommitState::Committed
            })
            .ok_or(ExperimentError::StaleParent)?;
        let (evidence_envelope, evidence_payload) =
            load_indexed_artifact(evidence_row, store, root).await?;
        evidence_envelope
            .validate(root, &roles)
            .map_err(|_| ExperimentError::StaleParent)?;
        if evidence_envelope.artifact_id != evidence_row.artifact_id
            || evidence_envelope.experiment_id != parent.definition.experiment.experiment_id
            || evidence_envelope.experiment_version_id != parent.version_id
            || evidence_envelope.artifact_uri != evidence_row.artifact_uri
            || evidence_envelope.content_hash != evidence_row.content_sha256
            || evidence_envelope.byte_length != evidence_payload.len() as u64
            || evidence_envelope.evidence_state != EvidenceState::Active
            || evidence_envelope.index_lifecycle_state != LifecycleState::Active
            || !lineage_matches(evidence_row, &evidence_envelope)
        {
            return Err(ExperimentError::StaleParent);
        }
        if evidence_envelope.artifact_type == expected_evidence_type {
            validate_transition_evidence_payload(
                expected_evidence_type,
                evidence_envelope,
                evidence_payload.as_ref(),
                parent,
            )?;
            typed_evidence_found = true;
        }
    }
    if !typed_evidence_found {
        return Err(ExperimentError::StaleParent);
    }
    Ok(transition)
}

fn validate_transition_evidence_payload(
    evidence_type: ResearchArtifactType,
    envelope: ResearchArtifactEnvelope,
    payload: &[u8],
    parent: &ValidatedExperiment,
) -> Result<(), ExperimentError> {
    if evidence_type != ResearchArtifactType::InvalidationEvent {
        return Err(ExperimentError::StaleParent);
    }
    let invalidation = serde_json::from_slice::<ExperimentInvalidationEvent>(payload)
        .map_err(|_| ExperimentError::StaleParent)?;
    let role = parent
        .definition
        .roles
        .bindings
        .iter()
        .find(|role| role.role_id == invalidation.authorized_by_role)
        .ok_or(ExperimentError::StaleParent)?;
    if invalidation.invalidation_id != envelope.artifact_id
        || invalidation.experiment_id != parent.definition.experiment.experiment_id
        || invalidation.experiment_version_id != parent.version_id
        || invalidation.invalidated_artifact_id
            != format!(
                "experiment-{}-{}",
                parent.definition.experiment.experiment_id, parent.version_id
            )
        || invalidation.invalidated_content_hash != parent.original_hash
        || invalidation.authorized_by_role != envelope.created_by_role
        || invalidation.recorded_at != envelope.created_at
        || invalidation.reason_code.is_empty()
        || role.purpose != RolePurpose::GovernanceApproval
    {
        return Err(ExperimentError::StaleParent);
    }
    parse_time("invalidation.recorded_at", &invalidation.recorded_at)
        .map_err(|_| ExperimentError::StaleParent)?;
    Ok(())
}

fn required_transition_evidence_type(state: ExperimentState) -> Option<ResearchArtifactType> {
    match state {
        ExperimentState::Invalidated => Some(ResearchArtifactType::InvalidationEvent),
        ExperimentState::Draft
        | ExperimentState::GenesisCommitted
        | ExperimentState::DiscoveryCommitted
        | ExperimentState::DiscoveryReleased
        | ExperimentState::ConfirmationCommitted
        | ExperimentState::ConfirmationReleased
        | ExperimentState::EnrichmentCommitted
        | ExperimentState::ProviderSelectionCommitted
        | ExperimentState::MechanismReleased
        | ExperimentState::Exploratory => None,
    }
}

fn validate_parent_sequence_and_state(
    child: &ExperimentVersion,
    parent: &ValidatedExperiment,
    parent_state: ExperimentState,
) -> Result<(), ExperimentError> {
    let next_sequence = parent
        .definition
        .experiment
        .version_sequence
        .checked_add(1)
        .ok_or(ExperimentError::StaleParent)?;
    if child.parent_version_id.as_deref() != Some(parent.version_id.as_str())
        || child.parent_content_hash.as_deref() != Some(parent.original_hash.as_str())
        || parent.definition.experiment.experiment_id != child.experiment_id
        || child.version_sequence != next_sequence
        || parent_state != ExperimentState::Draft
    {
        return Err(ExperimentError::StaleParent);
    }
    Ok(())
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ResearchNtRunIdentity {
    pub experiment_version_id: String,
    pub experiment_semantic_hash: String,
    pub source_identity: String,
    pub code_version: String,
    pub schema_version: String,
    pub nt_version: String,
    pub catalog_version: String,
    pub dependency_set_hash: String,
    pub execution_environment_hash: String,
    pub numeric_rules_hash: String,
    pub random_seeds: BTreeMap<String, u64>,
}

#[cfg(test)]
mod tests {
    use super::*;

    const FIXTURE_STORE_CONFIG: &str = r#"
artifact_root = "s3://example-bucket/pump-research-fixture"
catalog_projection_manifest_object = "catalog-projection-manifest.json"

[s3]
region = "us-east-1"
conditional_put = "etag"
copy_if_not_exists = "multipart"

[create_only_probe]
prefix = ".writer-probe"
object_name = "sentinel"
copy_source_object_name = "copy-source"
copy_dest_object_name = "copy-dest"

[subpaths]
raw = "raw"
nt_catalog = "nt-catalog"
nt_catalog_synthetic_proof = "nt-catalog-synthetic-proof"
source_proofs = "source-proofs"
backtests = "backtests"
artifact_index = "artifact-index"
research_analytics = "configured-research-analytics"

[lifecycle]
retention = "forever"
default_delete_expiration = "disabled"
storage_profiles = ["active", "archive", "deep_archive"]

[lifecycle.quiet_window_seconds]
raw = 7200
nt_catalog = 7200
source_proofs = 7200
backtests = 3600
artifact_index = 0
research_analytics = 7200

[lifecycle.hot_index]
latest_pointer_storage_profile = "active"
current_snapshot_storage_profile = "active"
"#;

    fn fixture_artifact_root() -> ResolvedArtifactRoot {
        toml::from_str::<ArtifactStoreConfig>(FIXTURE_STORE_CONFIG)
            .expect("store config")
            .resolve()
            .expect("store root")
    }

    fn version_two_fixture(parent: &ValidatedExperiment, frame_id: &str) -> ValidatedExperiment {
        let mut document: toml::Value = toml::from_str(include_str!(
            "../../../config/research/pump-research-synthetic.toml"
        ))
        .expect("fixture TOML value");
        let experiment = document
            .get_mut("experiment")
            .and_then(toml::Value::as_table_mut)
            .expect("experiment table");
        experiment.insert("version_sequence".to_string(), toml::Value::Integer(2));
        experiment.insert(
            "parent_version_id".to_string(),
            toml::Value::String(parent.version_id.clone()),
        );
        experiment.insert(
            "parent_content_hash".to_string(),
            toml::Value::String(parent.original_hash.clone()),
        );
        let mut lineage = toml::map::Map::new();
        lineage.insert(
            "artifact_kind".to_string(),
            toml::Value::String("research-analytics".to_string()),
        );
        lineage.insert(
            "artifact_id".to_string(),
            toml::Value::String(format!(
                "experiment-{}-{}",
                parent.definition.experiment.experiment_id, parent.version_id
            )),
        );
        lineage.insert("artifact_version".to_string(), toml::Value::Integer(1));
        lineage.insert(
            "content_hash".to_string(),
            toml::Value::String(parent.original_hash.clone()),
        );
        experiment
            .get_mut("lineage_refs")
            .and_then(toml::Value::as_array_mut)
            .expect("experiment lineage")
            .push(toml::Value::Table(lineage));
        document
            .get_mut("target_frame")
            .and_then(toml::Value::as_table_mut)
            .expect("target frame")
            .insert(
                "frame_id".to_string(),
                toml::Value::String(frame_id.to_string()),
            );
        let bytes = toml::to_string(&document).expect("version two TOML");
        parse_and_validate(bytes.as_bytes(), ValidationMode::Fixture)
            .expect("version two fixture validates")
    }

    async fn persist_registration_plan(
        plan: &ExperimentRegistrationPlan,
        store: &object_store::memory::InMemory,
        root: &ResolvedArtifactRoot,
    ) -> crate::artifact_store::ArtifactIndexSnapshotRow {
        for (uri, bytes) in [
            (plan.payload_uri.as_str(), plan.payload_bytes.clone()),
            (plan.envelope_uri.as_str(), plan.envelope_bytes.clone()),
        ] {
            let path = root.object_path_for_uri(uri).expect("registration path");
            CreateOnlyArtifactWriter::new(store)
                .put_create_idempotent(&path, bytes)
                .await
                .expect("registration artifact");
        }
        crate::artifact_store::ArtifactIndexSnapshotRow::from_event(
            &plan.store_event,
            StoreCommitState::Committed,
        )
    }

    #[test]
    fn fixture_is_deterministic_and_provider_neutral() {
        let first = parse_fixture_experiment();
        let second = parse_fixture_experiment();
        assert_eq!(
            first.canonical_semantic_bytes,
            second.canonical_semantic_bytes
        );
        assert_eq!(first.semantic_hash, second.semantic_hash);
        assert!(first.definition.source_policy.zero_provider_access);
        assert!(!first.definition.enrichment.provider_selected);
    }

    #[test]
    fn production_rejects_fixture_role_authority_before_external_calls() {
        let bytes = include_bytes!("../../../config/research/pump-research-synthetic.toml");
        let error = parse_and_validate(bytes, ValidationMode::Production).unwrap_err();
        assert!(error.to_string().contains("aws_sts authority"), "{error}");
    }

    #[test]
    fn registration_plan_has_dirty_write_and_parent_teeth() {
        let experiment = parse_fixture_experiment();
        let root = fixture_artifact_root();
        let role = experiment
            .definition
            .roles
            .bindings
            .iter()
            .find(|role| role.role_id == experiment.definition.experiment.append_role)
            .expect("append role");
        let plan = build_registration_plan(&experiment, role, None, &root).expect("plan");
        assert!(
            plan.store_event
                .artifact_uri
                .contains("/configured-research-analytics/v1/experiment-contracts/")
        );
        plan.verify_clean(&experiment.original_bytes)
            .expect("clean");
        assert!(matches!(
            plan.verify_clean(b"dirty"),
            Err(ExperimentError::DirtyArtifact)
        ));
        assert!(matches!(
            build_registration_plan(&experiment, role, Some(&"a".repeat(64)), &root),
            Err(ExperimentError::StaleParent)
        ));
    }

    #[tokio::test]
    async fn exact_committed_registration_is_idempotent_and_dirty_duplicate_fails() {
        let experiment = parse_fixture_experiment();
        let root = fixture_artifact_root();
        let role = experiment
            .definition
            .roles
            .bindings
            .iter()
            .find(|role| role.role_id == experiment.definition.experiment.append_role)
            .expect("append role");
        let plan = build_registration_plan(&experiment, role, None, &root).expect("plan");
        let store = object_store::memory::InMemory::new();
        let row = persist_registration_plan(&plan, &store, &root).await;
        let snapshot = ArtifactIndexSnapshot::new(
            "idempotent-registration",
            StoreArtifactKind::ResearchAnalytics,
            vec![row.clone()],
        )
        .expect("snapshot");
        assert!(
            existing_registration_is_identical(
                &experiment,
                &plan,
                &snapshot,
                &store,
                &root,
                ValidationMode::Fixture,
            )
            .await
            .expect("exact registration")
        );

        let mut dirty = row;
        dirty.content_sha256 = "f".repeat(64);
        let dirty_snapshot = ArtifactIndexSnapshot::new(
            "dirty-registration",
            StoreArtifactKind::ResearchAnalytics,
            vec![dirty],
        )
        .expect("snapshot");
        assert!(matches!(
            existing_registration_is_identical(
                &experiment,
                &plan,
                &dirty_snapshot,
                &store,
                &root,
                ValidationMode::Fixture,
            )
            .await,
            Err(ExperimentError::DirtyArtifact)
        ));
    }

    #[test]
    fn registration_snapshot_id_is_retry_and_prior_collision_safe() {
        let experiment = parse_fixture_experiment();
        let root = fixture_artifact_root();
        let role = experiment
            .definition
            .roles
            .bindings
            .iter()
            .find(|role| role.role_id == experiment.definition.experiment.append_role)
            .expect("append role");
        let plan = build_registration_plan(&experiment, role, None, &root).expect("plan");
        let first = registration_snapshot_id(
            &experiment.definition.storage,
            &plan.store_event,
            Some("prior-a"),
        );
        let repeat = registration_snapshot_id(
            &experiment.definition.storage,
            &plan.store_event,
            Some("prior-a"),
        );
        let rebased = registration_snapshot_id(
            &experiment.definition.storage,
            &plan.store_event,
            Some("prior-b"),
        );
        assert_eq!(first, repeat);
        assert_ne!(first, rebased);
        assert!(is_lowercase_sha256_hex(&first));
    }

    #[test]
    fn pointer_conflict_preserves_typed_stale_parent_reason() {
        let error = map_registration_commit_error(ArtifactIndexPointerConflict.into());
        assert!(matches!(error, ExperimentError::StaleParent));
        assert_eq!(error.reason_code(), "stale_parent");
    }

    #[test]
    fn terminal_evidence_state_rejects_promotion() {
        assert!(
            EvidenceState::Revoked
                .validate_transition(EvidenceState::Active)
                .is_err()
        );
    }

    #[test]
    fn slice_one_allows_only_draft_parent_and_typed_invalidation() {
        let parent = parse_fixture_experiment();
        let mut child = parent.definition.experiment.clone();
        child.version_sequence = 2;
        child.parent_version_id = Some(parent.version_id.clone());
        child.parent_content_hash = Some(parent.original_hash.clone());
        child.lineage_refs.push(LineageRef {
            artifact_kind: StoreArtifactKind::ResearchAnalytics,
            artifact_id: format!("experiment-{}-{}", child.experiment_id, parent.version_id),
            artifact_version: Some(1),
            content_hash: parent.original_hash.clone(),
        });
        validate_version(&child).expect("child definition shape");
        assert!(
            validate_parent_sequence_and_state(&child, &parent, ExperimentState::Invalidated)
                .is_err()
        );
        validate_parent_sequence_and_state(&child, &parent, ExperimentState::Draft)
            .expect("registered draft parent permits the immediate next version");
        assert!(
            validate_parent_sequence_and_state(
                &child,
                &parent,
                ExperimentState::ConfirmationCommitted
            )
            .is_err()
        );

        child.version_sequence = 3;
        assert!(
            validate_parent_sequence_and_state(&child, &parent, ExperimentState::Draft).is_err()
        );
    }

    #[tokio::test]
    async fn registration_loads_parent_bytes_sequence_and_authoritative_state() {
        let parent = parse_fixture_experiment();
        let root = fixture_artifact_root();
        let role = parent
            .definition
            .roles
            .bindings
            .iter()
            .find(|role| role.can_append_versions)
            .expect("append role");
        let plan = build_registration_plan(&parent, role, None, &root).expect("parent plan");
        let store = object_store::memory::InMemory::new();
        let row = persist_registration_plan(&plan, &store, &root).await;
        let snapshot = ArtifactIndexSnapshot::new(
            "parent-snapshot",
            StoreArtifactKind::ResearchAnalytics,
            vec![row],
        )
        .expect("snapshot");
        let mut child = version_two_fixture(&parent, "candidate-child-frame");
        validate_observed_parent(
            &child,
            Some(&snapshot),
            &store,
            &root,
            ValidationMode::Fixture,
        )
        .await
        .expect("registered parent permits immediate child");

        child.definition.experiment.version_sequence = 3;
        assert!(
            validate_observed_parent(
                &child,
                Some(&snapshot),
                &store,
                &root,
                ValidationMode::Fixture,
            )
            .await
            .is_err()
        );

        child.definition.experiment.version_sequence = 2;
        let mut forged_row = snapshot.rows[0].clone();
        forged_row.artifact_id = "forged-state".to_string();
        forged_row.artifact_uri = format!(
            "{}/experiment-contracts/{}/{}/state-transitions/forged.json",
            root.typed_root(StoreArtifactKind::ResearchAnalytics),
            parent.definition.experiment.experiment_id,
            parent.version_id,
        );
        forged_row.manifest_uri = format!("{}-envelope.json", forged_row.artifact_uri);
        let forged_snapshot = ArtifactIndexSnapshot::new(
            "forged-parent-snapshot",
            StoreArtifactKind::ResearchAnalytics,
            vec![snapshot.rows[0].clone(), forged_row],
        )
        .expect("forged snapshot");
        assert!(
            validate_observed_parent(
                &child,
                Some(&forged_snapshot),
                &store,
                &root,
                ValidationMode::Fixture,
            )
            .await
            .is_err()
        );

        let sibling = version_two_fixture(&parent, "concurrently-committed-sibling-frame");
        let sibling_role = sibling
            .definition
            .roles
            .bindings
            .iter()
            .find(|role| role.can_append_versions)
            .expect("sibling append role");
        let sibling_plan =
            build_registration_plan(&sibling, sibling_role, Some(&parent.version_id), &root)
                .expect("sibling plan");
        let sibling_row = persist_registration_plan(&sibling_plan, &store, &root).await;
        let branched_snapshot = ArtifactIndexSnapshot::new(
            "branched-definition-snapshot",
            StoreArtifactKind::ResearchAnalytics,
            vec![snapshot.rows[0].clone(), sibling_row],
        )
        .expect("branched snapshot");
        assert!(
            validate_observed_parent(
                &child,
                Some(&branched_snapshot),
                &store,
                &root,
                ValidationMode::Fixture,
            )
            .await
            .is_err(),
            "a committed sibling makes the declared parent stale"
        );

        let governance_role = parent
            .definition
            .roles
            .bindings
            .iter()
            .find(|role| role.purpose == RolePurpose::GovernanceApproval)
            .expect("governance role");
        let definition_ref = LineageRef {
            artifact_kind: StoreArtifactKind::ResearchAnalytics,
            artifact_id: plan.envelope.artifact_id.clone(),
            artifact_version: Some(1),
            content_hash: parent.original_hash.clone(),
        };
        let base_uri = format!(
            "{}/experiment-contracts/{}/{}",
            root.typed_root(StoreArtifactKind::ResearchAnalytics),
            parent.definition.experiment.experiment_id,
            parent.version_id,
        );
        let evidence_id = "invalidation-evidence-001".to_string();
        let evidence_payload = serde_json::to_vec(&ExperimentInvalidationEvent {
            invalidation_id: evidence_id.clone(),
            experiment_id: parent.definition.experiment.experiment_id.clone(),
            experiment_version_id: parent.version_id.clone(),
            invalidated_artifact_id: plan.envelope.artifact_id.clone(),
            invalidated_content_hash: parent.original_hash.clone(),
            reason_code: "fixture-integrity-failure".to_string(),
            authorized_by_role: governance_role.role_id.clone(),
            recorded_at: parent.definition.experiment.created_at.clone(),
        })
        .expect("invalidation payload");
        let evidence_hash = sha256_hex(&evidence_payload);
        let evidence_uri = format!("{base_uri}/invalidation-event.json");
        let evidence_manifest_uri = format!("{base_uri}/invalidation-envelope.json");
        let evidence_envelope = ResearchArtifactEnvelope {
            artifact_schema_version: ARTIFACT_SCHEMA_VERSION.to_string(),
            artifact_type: ResearchArtifactType::InvalidationEvent,
            artifact_id: evidence_id.clone(),
            experiment_id: parent.definition.experiment.experiment_id.clone(),
            experiment_version_id: parent.version_id.clone(),
            artifact_uri: evidence_uri.clone(),
            content_hash: evidence_hash.clone(),
            semantic_hash: None,
            byte_length: evidence_payload.len() as u64,
            created_at: parent.definition.experiment.created_at.clone(),
            created_by_role: governance_role.role_id.clone(),
            lineage_refs: vec![definition_ref.clone()],
            source_entry_refs: Vec::new(),
            index_lifecycle_state: LifecycleState::Active,
            evidence_state: EvidenceState::Active,
            invalidated_by_refs: Vec::new(),
        };
        let evidence_event = StoreIndexEvent {
            schema_version: ARTIFACT_SCHEMA_VERSION.to_string(),
            created_at: parent.definition.experiment.created_at.clone(),
            event_id: evidence_id.clone(),
            artifact_kind: StoreArtifactKind::ResearchAnalytics,
            artifact_id: evidence_id.clone(),
            artifact_uri: evidence_uri.clone(),
            manifest_uri: evidence_manifest_uri.clone(),
            producer_project: parent.definition.storage.producer_project.clone(),
            owner_project: parent.definition.storage.owner_project.clone(),
            content_sha256: evidence_hash.clone(),
            lifecycle_state: StoreLifecycleState::Active,
            storage_profile: ArtifactStorageProfile::Active,
            parent_lineage: vec![StoreLineageRef {
                artifact_kind: definition_ref.artifact_kind,
                artifact_id: definition_ref.artifact_id.clone(),
                version: definition_ref
                    .artifact_version
                    .map(|version| version.to_string()),
                sha256: definition_ref.content_hash.clone(),
            }],
            commit_state: StoreCommitState::Staged,
        };
        let evidence_row = crate::artifact_store::ArtifactIndexSnapshotRow::from_event(
            &evidence_event,
            StoreCommitState::Committed,
        );
        let evidence_ref = LineageRef {
            artifact_kind: StoreArtifactKind::ResearchAnalytics,
            artifact_id: evidence_id,
            artifact_version: Some(1),
            content_hash: evidence_hash,
        };
        let transition = ExperimentStateTransition {
            state_transition_id: "state-invalidated-001".to_string(),
            experiment_id: parent.definition.experiment.experiment_id.clone(),
            experiment_version_id: parent.version_id.clone(),
            sequence: 1,
            from_state: ExperimentState::Draft,
            to_state: ExperimentState::Invalidated,
            previous_state_artifact_id: plan.envelope.artifact_id.clone(),
            previous_state_content_hash: parent.original_hash.clone(),
            authorized_by_role: governance_role.role_id.clone(),
            transition_evidence_refs: vec![evidence_ref.clone()],
            recorded_at: parent.definition.experiment.created_at.clone(),
        };
        let transition_payload = serde_json::to_vec(&transition).expect("transition payload");
        let transition_hash = sha256_hex(&transition_payload);
        let transition_uri = format!(
            "{base_uri}/state-transitions/{}.json",
            transition.state_transition_id
        );
        let transition_manifest_uri = format!(
            "{base_uri}/state-transitions/{}-envelope.json",
            transition.state_transition_id
        );
        let transition_lineage = vec![definition_ref, evidence_ref];
        let transition_envelope = ResearchArtifactEnvelope {
            artifact_schema_version: ARTIFACT_SCHEMA_VERSION.to_string(),
            artifact_type: ResearchArtifactType::ExperimentStateTransition,
            artifact_id: transition.state_transition_id.clone(),
            experiment_id: transition.experiment_id.clone(),
            experiment_version_id: transition.experiment_version_id.clone(),
            artifact_uri: transition_uri.clone(),
            content_hash: transition_hash.clone(),
            semantic_hash: None,
            byte_length: transition_payload.len() as u64,
            created_at: transition.recorded_at.clone(),
            created_by_role: transition.authorized_by_role.clone(),
            lineage_refs: transition_lineage.clone(),
            source_entry_refs: Vec::new(),
            index_lifecycle_state: LifecycleState::Active,
            evidence_state: EvidenceState::Active,
            invalidated_by_refs: Vec::new(),
        };
        let transition_event = StoreIndexEvent {
            schema_version: ARTIFACT_SCHEMA_VERSION.to_string(),
            created_at: transition.recorded_at.clone(),
            event_id: transition.state_transition_id.clone(),
            artifact_kind: StoreArtifactKind::ResearchAnalytics,
            artifact_id: transition.state_transition_id.clone(),
            artifact_uri: transition_uri.clone(),
            manifest_uri: transition_manifest_uri.clone(),
            producer_project: parent.definition.storage.producer_project.clone(),
            owner_project: parent.definition.storage.owner_project.clone(),
            content_sha256: transition_hash,
            lifecycle_state: StoreLifecycleState::Active,
            storage_profile: ArtifactStorageProfile::Active,
            parent_lineage: transition_lineage
                .iter()
                .map(|lineage| StoreLineageRef {
                    artifact_kind: lineage.artifact_kind,
                    artifact_id: lineage.artifact_id.clone(),
                    version: lineage.artifact_version.map(|version| version.to_string()),
                    sha256: lineage.content_hash.clone(),
                })
                .collect(),
            commit_state: StoreCommitState::Staged,
        };
        let transition_row = crate::artifact_store::ArtifactIndexSnapshotRow::from_event(
            &transition_event,
            StoreCommitState::Committed,
        );
        for (uri, bytes) in [
            (evidence_uri.as_str(), evidence_payload),
            (
                evidence_manifest_uri.as_str(),
                serde_json::to_vec(&evidence_envelope).expect("evidence envelope"),
            ),
            (transition_uri.as_str(), transition_payload),
            (
                transition_manifest_uri.as_str(),
                serde_json::to_vec(&transition_envelope).expect("transition envelope"),
            ),
        ] {
            let path = root.object_path_for_uri(uri).expect("artifact path");
            CreateOnlyArtifactWriter::new(&store)
                .put_create_idempotent(&path, bytes)
                .await
                .expect("state artifact");
        }
        let terminal_snapshot = ArtifactIndexSnapshot::new(
            "terminal-parent-snapshot",
            StoreArtifactKind::ResearchAnalytics,
            vec![snapshot.rows[0].clone(), evidence_row, transition_row],
        )
        .expect("terminal snapshot");
        assert!(
            validate_observed_parent(
                &child,
                Some(&terminal_snapshot),
                &store,
                &root,
                ValidationMode::Fixture,
            )
            .await
            .is_err()
        );
    }

    #[test]
    fn artifact_store_config_bytes_are_hash_bound_before_parse() {
        let experiment = parse_fixture_experiment();
        assert!(matches!(
            parse_bound_artifact_store_config(
                &experiment.definition.storage,
                b"mutated secondary config"
            ),
            Err(ExperimentError::DirtyArtifact)
        ));
    }

    fn roster_test_observation(
        observation_id: &str,
        source_ref: &str,
        instrument_id: &str,
        start_time: &str,
        end_time: &str,
        status: RosterStatus,
    ) -> InventoryObservation {
        let observed_count = match status {
            RosterStatus::EligibleObserved => 10,
            RosterStatus::KnownInsufficientCoverage => 4,
            RosterStatus::KnownIneligible | RosterStatus::ExistenceOrCoverageUnknown => 0,
        };
        let mut observation = InventoryObservation {
            observation_id: observation_id.to_string(),
            source_ref: source_ref.to_string(),
            frame_id: "frame-a".to_string(),
            venue_key: "venue-a".to_string(),
            market_family_key: "perpetual".to_string(),
            venue_instrument_identity: IdentityNode::new(IdentityNativeKey::VenueInstrument {
                venue_key: "venue-a".to_string(),
                instrument_id: instrument_id.to_string(),
                listing_incarnation: format!("listing-{instrument_id}"),
            })
            .expect("venue instrument incarnation"),
            time_unit: TimeInterval {
                start_time: start_time.to_string(),
                end_time: end_time.to_string(),
            },
            proposed_status: status,
            status_reason: match status {
                RosterStatus::KnownIneligible => RosterStatusReason::PolicyIneligible,
                RosterStatus::ExistenceOrCoverageUnknown => RosterStatusReason::UnverifiedExistence,
                RosterStatus::EligibleObserved | RosterStatus::KnownInsufficientCoverage => {
                    RosterStatusReason::MeasuredCoverage
                }
            },
            coverage_metrics: CoverageMetrics {
                expected_count: 10,
                observed_count,
                missing_count: 10 - observed_count,
                duplicated_count: 0,
                interrupted_intervals: Vec::new(),
            },
            assertion_refs: Vec::new(),
        };
        observation.assertion_refs = vec![roster_test_assertion(&observation).assertion_id];
        observation
    }

    fn roster_test_assertion(observation: &InventoryObservation) -> TemporalAssertion {
        let mut assertion = TemporalAssertion {
            assertion_id: String::new(),
            subject_id: observation.venue_instrument_identity.identity_id.clone(),
            predicate: AssertionPredicate::RosterStatus,
            value: AssertionValue::RosterStatus {
                status: observation.proposed_status,
                reason: observation.status_reason,
            },
            valid_time: observation.time_unit.clone(),
            publication_time: Some("2024-12-30T00:00:00Z".to_string()),
            availability_time: Some("2024-12-31T00:00:00Z".to_string()),
            retrieval_time: "2024-12-31T12:00:00Z".to_string(),
            availability_status: AvailabilityStatus::ArchivallyAttested,
            revision_of: None,
            assertion_state: AssertionState::Active,
            evidence_refs: vec!["archive://roster-status".to_string()],
        };
        assertion.assertion_id = assertion.derived_id();
        assertion
    }

    fn roster_test_manifest(
        source_ref: &str,
        source_entry_content_hash: &str,
        observations: Vec<InventoryObservation>,
        completeness: RosterCompleteness,
    ) -> (InventorySourceBinding, Vec<u8>) {
        let mut coverage = InventoryCoverageArtifact {
            schema_version: INVENTORY_COVERAGE_SCHEMA_VERSION.to_string(),
            coverage_id: String::new(),
            frame_id: "frame-a".to_string(),
            source_entry_id: format!("source-{source_ref}"),
            source_entry_version: 1,
            source_entry_content_hash: source_entry_content_hash.to_string(),
            roster_vintage: "2024-12-31T23:59:59Z".to_string(),
            expected_unit_count: observations.len() as u64,
            enumerated_unit_count: observations.len() as u64,
            completeness,
            evidence_hashes: vec!["d".repeat(64)],
        };
        let coverage_hash = inventory_coverage_hash(&coverage).expect("coverage hash");
        coverage.coverage_id = format!("coverage-{coverage_hash}");
        let manifest = InventoryManifestArtifact {
            schema_version: INVENTORY_MANIFEST_SCHEMA_VERSION.to_string(),
            source_ref: source_ref.to_string(),
            frame_id: "frame-a".to_string(),
            roster_vintage: "2024-12-31T23:59:59Z".to_string(),
            coverage,
            observations,
        };
        let bytes = serde_json::to_vec(&manifest).expect("manifest bytes");
        (
            InventorySourceBinding {
                source_ref: source_ref.to_string(),
                source_entry_id: format!("source-{source_ref}"),
                source_entry_version: 1,
                source_entry_content_hash: source_entry_content_hash.to_string(),
                manifest_hash: sha256_hex(&bytes),
                coverage_hash,
            },
            bytes,
        )
    }

    fn roster_test_source_evidence(
        binding: &InventorySourceBinding,
    ) -> VerifiedRegisteredSourceEvidence {
        VerifiedRegisteredSourceEvidence::synthetic(
            &binding.source_entry_id,
            binding.source_entry_version,
            &binding.source_entry_content_hash,
            Vec::new(),
            vec!["d".repeat(64)],
            "2025-01-01T00:00:00Z",
        )
    }

    fn roster_test_coverage_evidence(
        experiment_content_hash: &str,
        frame: &TargetFramePolicy,
        binding: &InventorySourceBinding,
        manifest_bytes: &[u8],
    ) -> VerifiedInventoryCoverageEvidence {
        let manifest: InventoryManifestArtifact =
            serde_json::from_slice(manifest_bytes).expect("inventory manifest");
        let assertions = manifest
            .observations
            .iter()
            .map(roster_test_assertion)
            .collect::<Vec<_>>();
        let temporal_evidence = VerifiedTemporalAssertionHistoryEvidence::synthetic(&assertions);
        VerifiedInventoryCoverageEvidence::synthetic(
            experiment_content_hash,
            frame,
            binding,
            manifest.coverage.expected_unit_count,
            &manifest.observations,
            manifest.coverage.completeness,
            manifest.coverage.evidence_hashes,
            &temporal_evidence,
        )
    }

    #[test]
    fn verified_roster_derives_completeness_denominator_and_source_precedence() {
        let experiment_content_hash = "f".repeat(64);
        let source_hash_a = "a".repeat(64);
        let source_hash_b = "b".repeat(64);
        let (binding_a, bytes_a) = roster_test_manifest(
            "inventory-a",
            &source_hash_a,
            vec![
                roster_test_observation(
                    "a-1",
                    "inventory-a",
                    "instrument-1",
                    "2025-01-01T00:00:00Z",
                    "2025-01-02T00:00:00Z",
                    RosterStatus::KnownInsufficientCoverage,
                ),
                roster_test_observation(
                    "a-2",
                    "inventory-a",
                    "instrument-2",
                    "2025-01-02T00:00:00Z",
                    "2025-01-03T00:00:00Z",
                    RosterStatus::KnownIneligible,
                ),
                roster_test_observation(
                    "a-3",
                    "inventory-a",
                    "instrument-3",
                    "2025-01-01T00:00:00Z",
                    "2025-01-02T00:00:00Z",
                    RosterStatus::ExistenceOrCoverageUnknown,
                ),
            ],
            RosterCompleteness::EnumeratedIncomplete,
        );
        let (binding_b, bytes_b) = roster_test_manifest(
            "inventory-b",
            &source_hash_b,
            vec![roster_test_observation(
                "b-1",
                "inventory-b",
                "instrument-1",
                "2025-01-01T00:00:00Z",
                "2025-01-02T00:00:00Z",
                RosterStatus::EligibleObserved,
            )],
            RosterCompleteness::ProvenComplete,
        );
        let mut frame = TargetFramePolicy {
            frame_id: "frame-a".to_string(),
            venue_keys: vec!["venue-a".to_string()],
            market_family_keys: vec!["perpetual".to_string()],
            start_time: "2025-01-01T00:00:00Z".to_string(),
            end_time: "2025-01-03T00:00:00Z".to_string(),
            time_unit_grain: TimeUnitGrain::UtcDay,
            outer_roster_rule: OuterRosterRule::UnionOfAdmittedVintageInventories,
            roster_vintage: "2024-12-31T23:59:59Z".to_string(),
            inventory_source_refs: vec![binding_a.clone(), binding_b.clone()],
            reconciliation_rule: ReconciliationRule::IdentityFirstSourcePrecedenceV1,
            status_precedence: vec![
                RosterStatus::EligibleObserved,
                RosterStatus::KnownIneligible,
                RosterStatus::KnownInsufficientCoverage,
                RosterStatus::ExistenceOrCoverageUnknown,
            ],
            generalization_scope: GeneralizationScope::EnumeratedRosterWithinConfiguredFrame,
        };
        let manifest_a = roster_test_source_evidence(&binding_a)
            .load_and_verify_inventory_manifest(
                roster_test_coverage_evidence(
                    &experiment_content_hash,
                    &frame,
                    &binding_a,
                    &bytes_a,
                ),
                &experiment_content_hash,
                &frame,
                &binding_a,
                || Ok(bytes_a.clone()),
            )
            .expect("manifest A");
        let manifest_b = roster_test_source_evidence(&binding_b)
            .load_and_verify_inventory_manifest(
                roster_test_coverage_evidence(
                    &experiment_content_hash,
                    &frame,
                    &binding_b,
                    &bytes_b,
                ),
                &experiment_content_hash,
                &frame,
                &binding_b,
                || Ok(bytes_b.clone()),
            )
            .expect("manifest B");
        let first = build_roster_manifest(
            &experiment_content_hash,
            &frame,
            vec![manifest_a, manifest_b],
        )
        .expect("roster");
        assert_eq!(first.denominator, 3);
        assert_eq!(first.status_counts.known_ineligible, 1);
        assert_eq!(first.status_counts.known_insufficient_coverage, 1);
        assert_eq!(first.status_counts.existence_or_coverage_unknown, 1);
        assert_eq!(first.completeness, RosterCompleteness::EnumeratedIncomplete);
        assert_eq!(first.attrition.starting_denominator, 3);
        assert!(first.units.iter().all(|unit| {
            unit.status != RosterStatus::EligibleObserved
                || unit.status_reason == RosterStatusReason::MeasuredCoverage
        }));

        frame.inventory_source_refs.reverse();
        let manifest_a = roster_test_source_evidence(&binding_a)
            .load_and_verify_inventory_manifest(
                roster_test_coverage_evidence(
                    &experiment_content_hash,
                    &frame,
                    &binding_a,
                    &bytes_a,
                ),
                &experiment_content_hash,
                &frame,
                &binding_a,
                || Ok(bytes_a.clone()),
            )
            .expect("manifest A");
        let manifest_b = roster_test_source_evidence(&binding_b)
            .load_and_verify_inventory_manifest(
                roster_test_coverage_evidence(
                    &experiment_content_hash,
                    &frame,
                    &binding_b,
                    &bytes_b,
                ),
                &experiment_content_hash,
                &frame,
                &binding_b,
                || Ok(bytes_b.clone()),
            )
            .expect("manifest B");
        let second = build_roster_manifest(
            &experiment_content_hash,
            &frame,
            vec![manifest_a, manifest_b],
        )
        .expect("roster");
        assert_eq!(second.status_counts.eligible_observed, 1);
        assert_ne!(first, second);
    }

    #[test]
    fn inventory_manifest_rejects_duplicate_cells_and_contradictory_status_coverage() {
        let experiment_content_hash = "f".repeat(64);
        let source_hash = "a".repeat(64);
        let first = roster_test_observation(
            "duplicate-a",
            "inventory-a",
            "instrument-1",
            "2025-01-01T00:00:00Z",
            "2025-01-02T00:00:00Z",
            RosterStatus::KnownInsufficientCoverage,
        );
        let mut duplicate = first.clone();
        duplicate.observation_id = "duplicate-b".to_string();
        let (binding, bytes) = roster_test_manifest(
            "inventory-a",
            &source_hash,
            vec![first.clone(), duplicate],
            RosterCompleteness::EnumeratedIncomplete,
        );
        let frame = TargetFramePolicy {
            frame_id: "frame-a".to_string(),
            venue_keys: vec!["venue-a".to_string()],
            market_family_keys: vec!["perpetual".to_string()],
            start_time: "2025-01-01T00:00:00Z".to_string(),
            end_time: "2025-01-03T00:00:00Z".to_string(),
            time_unit_grain: TimeUnitGrain::UtcDay,
            outer_roster_rule: OuterRosterRule::UnionOfAdmittedVintageInventories,
            roster_vintage: "2024-12-31T23:59:59Z".to_string(),
            inventory_source_refs: vec![binding.clone()],
            reconciliation_rule: ReconciliationRule::IdentityFirstSourcePrecedenceV1,
            status_precedence: vec![
                RosterStatus::EligibleObserved,
                RosterStatus::KnownIneligible,
                RosterStatus::KnownInsufficientCoverage,
                RosterStatus::ExistenceOrCoverageUnknown,
            ],
            generalization_scope: GeneralizationScope::EnumeratedRosterWithinConfiguredFrame,
        };
        let unauthorized = VerifiedRegisteredSourceEvidence::synthetic(
            "different-source-entry",
            1,
            &source_hash,
            Vec::new(),
            vec!["d".repeat(64)],
            "2025-01-01T00:00:00Z",
        );
        let trusted_duplicate_universe = vec![
            first,
            roster_test_observation(
                "independent-cell",
                "inventory-a",
                "instrument-2",
                "2025-01-02T00:00:00Z",
                "2025-01-03T00:00:00Z",
                RosterStatus::KnownInsufficientCoverage,
            ),
        ];
        let trusted_assertions = trusted_duplicate_universe
            .iter()
            .map(roster_test_assertion)
            .collect::<Vec<_>>();
        let trusted_history =
            VerifiedTemporalAssertionHistoryEvidence::synthetic(&trusted_assertions);
        let coverage_evidence = VerifiedInventoryCoverageEvidence::synthetic(
            &experiment_content_hash,
            &frame,
            &binding,
            2,
            &trusted_duplicate_universe,
            RosterCompleteness::EnumeratedIncomplete,
            vec!["d".repeat(64)],
            &trusted_history,
        );
        assert!(
            unauthorized
                .load_and_verify_inventory_manifest(
                    coverage_evidence,
                    &experiment_content_hash,
                    &frame,
                    &binding,
                    || panic!("manifest loader must not run before authority validation"),
                )
                .is_err()
        );
        assert!(
            roster_test_source_evidence(&binding)
                .load_and_verify_inventory_manifest(
                    VerifiedInventoryCoverageEvidence::synthetic(
                        &experiment_content_hash,
                        &frame,
                        &binding,
                        2,
                        &trusted_duplicate_universe,
                        RosterCompleteness::EnumeratedIncomplete,
                        vec!["d".repeat(64)],
                        &trusted_history,
                    ),
                    &experiment_content_hash,
                    &frame,
                    &binding,
                    || Ok(bytes.clone()),
                )
                .is_err()
        );

        let mut contradictory = roster_test_observation(
            "contradictory",
            "inventory-a",
            "instrument-2",
            "2025-01-01T00:00:00Z",
            "2025-01-02T00:00:00Z",
            RosterStatus::KnownInsufficientCoverage,
        );
        contradictory.proposed_status = RosterStatus::EligibleObserved;
        contradictory.assertion_refs = vec![roster_test_assertion(&contradictory).assertion_id];
        let (binding, bytes) = roster_test_manifest(
            "inventory-a",
            &source_hash,
            vec![contradictory],
            RosterCompleteness::EnumeratedIncomplete,
        );
        let mut frame = frame;
        frame.inventory_source_refs = vec![binding.clone()];
        assert!(
            roster_test_source_evidence(&binding)
                .load_and_verify_inventory_manifest(
                    roster_test_coverage_evidence(
                        &experiment_content_hash,
                        &frame,
                        &binding,
                        &bytes,
                    ),
                    &experiment_content_hash,
                    &frame,
                    &binding,
                    || Ok(bytes.clone()),
                )
                .is_err()
        );

        let observed = roster_test_observation(
            "complete-but-unverified",
            "inventory-a",
            "instrument-3",
            "2025-01-01T00:00:00Z",
            "2025-01-02T00:00:00Z",
            RosterStatus::EligibleObserved,
        );
        let (binding, bytes) = roster_test_manifest(
            "inventory-a",
            &source_hash,
            vec![observed.clone()],
            RosterCompleteness::ProvenComplete,
        );
        frame.inventory_source_refs = vec![binding.clone()];
        let independently_expected = vec![
            observed,
            roster_test_observation(
                "missing-from-sparse-manifest",
                "inventory-a",
                "instrument-4",
                "2025-01-02T00:00:00Z",
                "2025-01-03T00:00:00Z",
                RosterStatus::EligibleObserved,
            ),
        ];
        let independently_registered_assertions = independently_expected
            .iter()
            .map(roster_test_assertion)
            .collect::<Vec<_>>();
        let independently_registered_history = VerifiedTemporalAssertionHistoryEvidence::synthetic(
            &independently_registered_assertions,
        );
        let complete_coverage_evidence = VerifiedInventoryCoverageEvidence::synthetic(
            &experiment_content_hash,
            &frame,
            &binding,
            2,
            &independently_expected,
            RosterCompleteness::ProvenComplete,
            vec!["d".repeat(64)],
            &independently_registered_history,
        );
        assert!(
            roster_test_source_evidence(&binding)
                .load_and_verify_inventory_manifest(
                    complete_coverage_evidence,
                    &experiment_content_hash,
                    &frame,
                    &binding,
                    || Ok(bytes.clone()),
                )
                .is_err()
        );

        let trusted_observation = roster_test_observation(
            "registered-status",
            "inventory-a",
            "instrument-5",
            "2025-01-01T00:00:00Z",
            "2025-01-02T00:00:00Z",
            RosterStatus::EligibleObserved,
        );
        let mut forged_observation = trusted_observation.clone();
        forged_observation.assertion_refs = vec!["assertion-forged".to_string()];
        let (binding, bytes) = roster_test_manifest(
            "inventory-a",
            &source_hash,
            vec![forged_observation],
            RosterCompleteness::EnumeratedIncomplete,
        );
        frame.inventory_source_refs = vec![binding.clone()];
        let trusted_assertions = vec![roster_test_assertion(&trusted_observation)];
        let trusted_history =
            VerifiedTemporalAssertionHistoryEvidence::synthetic(&trusted_assertions);
        let coverage_evidence = VerifiedInventoryCoverageEvidence::synthetic(
            &experiment_content_hash,
            &frame,
            &binding,
            1,
            &[trusted_observation],
            RosterCompleteness::EnumeratedIncomplete,
            vec!["d".repeat(64)],
            &trusted_history,
        );
        assert!(
            roster_test_source_evidence(&binding)
                .load_and_verify_inventory_manifest(
                    coverage_evidence,
                    &experiment_content_hash,
                    &frame,
                    &binding,
                    || Ok(bytes.clone()),
                )
                .is_err()
        );
    }

    #[test]
    fn verified_roster_rejects_frame_replay_and_records_same_status_coverage_conflicts() {
        let experiment_content_hash = "f".repeat(64);
        let source_hash_a = "a".repeat(64);
        let source_hash_b = "b".repeat(64);
        let observation_a = roster_test_observation(
            "coverage-a",
            "inventory-a",
            "instrument-1",
            "2025-01-01T00:00:00Z",
            "2025-01-02T00:00:00Z",
            RosterStatus::KnownInsufficientCoverage,
        );
        let mut observation_b = roster_test_observation(
            "coverage-b",
            "inventory-b",
            "instrument-1",
            "2025-01-01T00:00:00Z",
            "2025-01-02T00:00:00Z",
            RosterStatus::KnownInsufficientCoverage,
        );
        observation_b.coverage_metrics.observed_count = 5;
        observation_b.coverage_metrics.missing_count = 5;
        observation_b.assertion_refs = vec![roster_test_assertion(&observation_b).assertion_id];
        let (binding_a, bytes_a) = roster_test_manifest(
            "inventory-a",
            &source_hash_a,
            vec![observation_a],
            RosterCompleteness::EnumeratedIncomplete,
        );
        let (binding_b, bytes_b) = roster_test_manifest(
            "inventory-b",
            &source_hash_b,
            vec![observation_b],
            RosterCompleteness::EnumeratedIncomplete,
        );
        let frame = TargetFramePolicy {
            frame_id: "frame-a".to_string(),
            venue_keys: vec!["venue-a".to_string()],
            market_family_keys: vec!["perpetual".to_string()],
            start_time: "2025-01-01T00:00:00Z".to_string(),
            end_time: "2025-01-03T00:00:00Z".to_string(),
            time_unit_grain: TimeUnitGrain::UtcDay,
            outer_roster_rule: OuterRosterRule::UnionOfAdmittedVintageInventories,
            roster_vintage: "2024-12-31T23:59:59Z".to_string(),
            inventory_source_refs: vec![binding_a.clone(), binding_b.clone()],
            reconciliation_rule: ReconciliationRule::IdentityFirstSourcePrecedenceV1,
            status_precedence: vec![
                RosterStatus::EligibleObserved,
                RosterStatus::KnownIneligible,
                RosterStatus::KnownInsufficientCoverage,
                RosterStatus::ExistenceOrCoverageUnknown,
            ],
            generalization_scope: GeneralizationScope::EnumeratedRosterWithinConfiguredFrame,
        };
        let manifest_a = roster_test_source_evidence(&binding_a)
            .load_and_verify_inventory_manifest(
                roster_test_coverage_evidence(
                    &experiment_content_hash,
                    &frame,
                    &binding_a,
                    &bytes_a,
                ),
                &experiment_content_hash,
                &frame,
                &binding_a,
                || Ok(bytes_a.clone()),
            )
            .expect("manifest A");
        let manifest_b = roster_test_source_evidence(&binding_b)
            .load_and_verify_inventory_manifest(
                roster_test_coverage_evidence(
                    &experiment_content_hash,
                    &frame,
                    &binding_b,
                    &bytes_b,
                ),
                &experiment_content_hash,
                &frame,
                &binding_b,
                || Ok(bytes_b.clone()),
            )
            .expect("manifest B");
        let roster = build_roster_manifest(
            &experiment_content_hash,
            &frame,
            vec![manifest_a, manifest_b],
        )
        .expect("reconciled roster");
        assert_eq!(roster.units[0].conflict_reasons.len(), 1);
        assert_ne!(
            roster.units[0].conflict_reasons[0].selected_coverage_metrics,
            roster.units[0].conflict_reasons[0].rejected_coverage_metrics
        );

        let manifest = roster_test_source_evidence(&binding_a)
            .load_and_verify_inventory_manifest(
                roster_test_coverage_evidence(
                    &experiment_content_hash,
                    &frame,
                    &binding_a,
                    &bytes_a,
                ),
                &experiment_content_hash,
                &frame,
                &binding_a,
                || Ok(bytes_a.clone()),
            )
            .expect("frame-bound manifest");
        let second_manifest = roster_test_source_evidence(&binding_b)
            .load_and_verify_inventory_manifest(
                roster_test_coverage_evidence(
                    &experiment_content_hash,
                    &frame,
                    &binding_b,
                    &bytes_b,
                ),
                &experiment_content_hash,
                &frame,
                &binding_b,
                || Ok(bytes_b.clone()),
            )
            .expect("second frame-bound manifest");
        let mut changed_same_id_frame = frame;
        changed_same_id_frame.status_precedence.reverse();
        assert!(
            build_roster_manifest(
                &experiment_content_hash,
                &changed_same_id_frame,
                vec![manifest, second_manifest],
            )
            .is_err()
        );
    }

    #[test]
    fn roster_distinguishes_relisted_instrument_incarnations() {
        let experiment_content_hash = "f".repeat(64);
        let source_hash = "a".repeat(64);
        let original = roster_test_observation(
            "original-listing",
            "inventory-a",
            "instrument-reused",
            "2025-01-01T00:00:00Z",
            "2025-01-02T00:00:00Z",
            RosterStatus::EligibleObserved,
        );
        let mut relisted = original.clone();
        relisted.observation_id = "relisted-incarnation".to_string();
        relisted.venue_instrument_identity =
            IdentityNode::new(IdentityNativeKey::VenueInstrument {
                venue_key: "venue-a".to_string(),
                instrument_id: "instrument-reused".to_string(),
                listing_incarnation: "listing-relisted".to_string(),
            })
            .expect("relisted identity");
        relisted.assertion_refs = vec![roster_test_assertion(&relisted).assertion_id];
        let (binding, bytes) = roster_test_manifest(
            "inventory-a",
            &source_hash,
            vec![original, relisted],
            RosterCompleteness::EnumeratedIncomplete,
        );
        let frame = TargetFramePolicy {
            frame_id: "frame-a".to_string(),
            venue_keys: vec!["venue-a".to_string()],
            market_family_keys: vec!["perpetual".to_string()],
            start_time: "2025-01-01T00:00:00Z".to_string(),
            end_time: "2025-01-03T00:00:00Z".to_string(),
            time_unit_grain: TimeUnitGrain::UtcDay,
            outer_roster_rule: OuterRosterRule::UnionOfAdmittedVintageInventories,
            roster_vintage: "2024-12-31T23:59:59Z".to_string(),
            inventory_source_refs: vec![binding.clone()],
            reconciliation_rule: ReconciliationRule::IdentityFirstSourcePrecedenceV1,
            status_precedence: vec![
                RosterStatus::EligibleObserved,
                RosterStatus::KnownIneligible,
                RosterStatus::KnownInsufficientCoverage,
                RosterStatus::ExistenceOrCoverageUnknown,
            ],
            generalization_scope: GeneralizationScope::EnumeratedRosterWithinConfiguredFrame,
        };
        let manifest = roster_test_source_evidence(&binding)
            .load_and_verify_inventory_manifest(
                roster_test_coverage_evidence(&experiment_content_hash, &frame, &binding, &bytes),
                &experiment_content_hash,
                &frame,
                &binding,
                || Ok(bytes),
            )
            .expect("incarnation-bound manifest");
        let roster = build_roster_manifest(&experiment_content_hash, &frame, vec![manifest])
            .expect("incarnation-aware roster");
        assert_eq!(roster.denominator, 2);
        assert_ne!(
            roster.units[0].venue_instrument_identity.identity_id,
            roster.units[1].venue_instrument_identity.identity_id
        );
    }

    fn registered_assertion(
        state: AssertionState,
        revision_of: Option<String>,
        retrieval_time: &str,
    ) -> TemporalAssertion {
        let mut assertion = TemporalAssertion {
            assertion_id: String::new(),
            subject_id: "instrument-a".to_string(),
            predicate: AssertionPredicate::ListingStatus,
            value: AssertionValue::Text {
                value: "listed".to_string(),
            },
            valid_time: TimeInterval {
                start_time: "2025-01-01T00:00:00Z".to_string(),
                end_time: "2025-02-01T00:00:00Z".to_string(),
            },
            publication_time: Some("2024-12-31T20:00:00Z".to_string()),
            availability_time: Some("2024-12-31T21:00:00Z".to_string()),
            retrieval_time: retrieval_time.to_string(),
            availability_status: AvailabilityStatus::ArchivallyAttested,
            revision_of,
            assertion_state: state,
            evidence_refs: vec!["archive://listing-status".to_string()],
        };
        assertion.assertion_id = assertion.derived_id();
        assertion
    }

    fn identity_mapping_assertion(
        mapping: &IdentityMapping,
        evidence_kind: IdentityEvidenceKind,
    ) -> TemporalAssertion {
        let mut assertion = registered_assertion(
            AssertionState::Active,
            None,
            mapping.retrieval_time.as_str(),
        );
        assertion.subject_id = mapping.mapping_id.clone();
        assertion.predicate = AssertionPredicate::IdentityMappingEvidence;
        assertion.value = AssertionValue::IdentityMappingEvidence {
            mapping_commitment: mapping.evidence_commitment(),
            evidence_kind,
        };
        assertion.valid_time = mapping.valid_time.clone();
        assertion.availability_time = Some(mapping.availability_time.clone());
        assertion.assertion_id = assertion.derived_id();
        assertion
    }

    #[test]
    fn identity_graph_distinguishes_listing_incarnations_and_requires_registered_assertions() {
        let old_listing = IdentityNode::new(IdentityNativeKey::VenueInstrument {
            venue_key: "venue-a".to_string(),
            instrument_id: "instrument-reused".to_string(),
            listing_incarnation: "listing-2024".to_string(),
        })
        .expect("old listing");
        let new_listing = IdentityNode::new(IdentityNativeKey::VenueInstrument {
            venue_key: "venue-a".to_string(),
            instrument_id: "instrument-reused".to_string(),
            listing_incarnation: "listing-2025".to_string(),
        })
        .expect("new listing");
        assert_ne!(old_listing.identity_id, new_listing.identity_id);
        let asset = IdentityNode::new(IdentityNativeKey::EconomicAsset {
            registry_key: "asset-registry".to_string(),
            asset_id: "asset-a".to_string(),
        })
        .expect("asset");
        let mut mappings = [
            (&old_listing, "mapping-old", "2025-01-03T00:00:00Z"),
            (&new_listing, "mapping-new", "2025-01-04T00:00:00Z"),
        ]
        .into_iter()
        .map(|(listing, mapping_id, retrieval_time)| IdentityMapping {
            mapping_id: mapping_id.to_string(),
            from_identity_id: listing.identity_id.clone(),
            to_identity_id: asset.identity_id.clone(),
            valid_time: TimeInterval {
                start_time: "2025-01-01T00:00:00Z".to_string(),
                end_time: "2025-02-01T00:00:00Z".to_string(),
            },
            availability_time: "2024-12-31T21:00:00Z".to_string(),
            retrieval_time: retrieval_time.to_string(),
            status: IdentityMappingStatus::Active,
            confidence: EvidenceConfidence::High,
            evidence: vec![IdentityEvidence {
                kind: IdentityEvidenceKind::VenueMetadata,
                assertion_id: String::new(),
            }],
            splice_rule: SeriesSpliceRule::Denied {
                reason: "Listing incarnations remain separate series.".to_string(),
            },
        })
        .collect::<Vec<_>>();
        let assertions = mappings
            .iter()
            .map(|mapping| identity_mapping_assertion(mapping, mapping.evidence[0].kind))
            .collect::<Vec<_>>();
        for (mapping, assertion) in mappings.iter_mut().zip(&assertions) {
            mapping.evidence[0].assertion_id = assertion.assertion_id.clone();
        }
        let evidence = VerifiedTemporalAssertionHistoryEvidence::synthetic(&assertions);
        validate_identity_graph(
            &[old_listing.clone(), new_listing.clone(), asset.clone()],
            &mappings,
            &assertions,
            Some(&evidence),
        )
        .expect("registered mapping assertions");

        let mut multi_evidence_mapping = mappings[0].clone();
        multi_evidence_mapping.evidence.push(IdentityEvidence {
            kind: IdentityEvidenceKind::ChainRegistry,
            assertion_id: String::new(),
        });
        let mut venue_assertion = identity_mapping_assertion(
            &multi_evidence_mapping,
            IdentityEvidenceKind::VenueMetadata,
        );
        venue_assertion.evidence_refs = vec!["archive://venue-metadata".to_string()];
        venue_assertion.assertion_id = venue_assertion.derived_id();
        let mut chain_assertion = identity_mapping_assertion(
            &multi_evidence_mapping,
            IdentityEvidenceKind::ChainRegistry,
        );
        chain_assertion.evidence_refs = vec!["archive://chain-registry".to_string()];
        chain_assertion.assertion_id = chain_assertion.derived_id();
        multi_evidence_mapping.evidence[0].assertion_id = venue_assertion.assertion_id.clone();
        multi_evidence_mapping.evidence[1].assertion_id = chain_assertion.assertion_id.clone();
        let multi_assertions = vec![venue_assertion, chain_assertion];
        let multi_evidence = VerifiedTemporalAssertionHistoryEvidence::synthetic(&multi_assertions);
        validate_identity_graph(
            &[old_listing.clone(), asset.clone()],
            &[multi_evidence_mapping.clone()],
            &multi_assertions,
            Some(&multi_evidence),
        )
        .expect("each evidence kind is bound to its registered assertion");
        let first_kind = multi_evidence_mapping.evidence[0].kind;
        multi_evidence_mapping.evidence[0].kind = multi_evidence_mapping.evidence[1].kind;
        multi_evidence_mapping.evidence[1].kind = first_kind;
        assert!(
            validate_identity_graph(
                &[old_listing.clone(), asset.clone()],
                &[multi_evidence_mapping],
                &multi_assertions,
                Some(&multi_evidence),
            )
            .is_err()
        );

        let mut tampered = mappings.clone();
        tampered[0].confidence = EvidenceConfidence::Low;
        assert!(
            validate_identity_graph(
                &[old_listing.clone(), new_listing.clone(), asset.clone()],
                &tampered,
                &assertions,
                Some(&evidence),
            )
            .is_err()
        );
        let mut relabeled = mappings;
        relabeled[0].evidence[0].kind = IdentityEvidenceKind::IssuerDisclosure;
        assert!(
            validate_identity_graph(
                &[old_listing.clone(), new_listing.clone(), asset.clone()],
                &relabeled,
                &assertions,
                Some(&evidence),
            )
            .is_err()
        );
        let mut retimed = relabeled;
        retimed[0].evidence[0].kind = IdentityEvidenceKind::VenueMetadata;
        retimed[0].valid_time.end_time = "2025-02-02T00:00:00Z".to_string();
        assert!(
            validate_identity_graph(
                &[old_listing, new_listing, asset],
                &retimed,
                &assertions,
                Some(&evidence),
            )
            .is_err()
        );
    }

    #[test]
    fn temporal_append_requires_content_identity_registered_head_and_monotonic_clock() {
        let original = registered_assertion(AssertionState::Active, None, "2025-01-02T00:00:00Z");
        let mut correction = registered_assertion(
            AssertionState::Corrected,
            Some(original.assertion_id.clone()),
            "2025-01-03T00:00:00Z",
        );
        correction.publication_time = None;
        correction.availability_time = None;
        correction.availability_status = AvailabilityStatus::RetrievalTimeAttested;
        correction.assertion_id = correction.derived_id();
        let history = vec![original.clone(), correction.clone()];
        let evidence = VerifiedTemporalAssertionHistoryEvidence::synthetic(&history);
        validate_temporal_assertion_chain(&history, Some(&evidence))
            .expect("registered append-only history");
        validate_temporal_assertion_claim_use(
            &history,
            Some(&evidence),
            &correction.assertion_id,
            ClaimUse::RetrospectiveDescriptive,
        )
        .expect("registered retrospective claim");
        assert!(
            validate_temporal_assertion_claim_use(
                &history,
                Some(&evidence),
                &original.assertion_id,
                ClaimUse::RetrospectiveDescriptive,
            )
            .is_err()
        );
        assert!(
            validate_temporal_assertion_claim_use(
                &history,
                Some(&evidence),
                &correction.assertion_id,
                ClaimUse::Predictive,
            )
            .is_err()
        );

        let retraction = registered_assertion(
            AssertionState::Retracted,
            Some(correction.assertion_id.clone()),
            "2025-01-04T00:00:00Z",
        );
        let retracted_history = vec![original.clone(), correction.clone(), retraction.clone()];
        let retracted_evidence =
            VerifiedTemporalAssertionHistoryEvidence::synthetic(&retracted_history);
        validate_temporal_assertion_chain(&retracted_history, Some(&retracted_evidence))
            .expect("registered retraction");
        for assertion_id in [
            &original.assertion_id,
            &correction.assertion_id,
            &retraction.assertion_id,
        ] {
            assert!(
                validate_temporal_assertion_claim_use(
                    &retracted_history,
                    Some(&retracted_evidence),
                    assertion_id,
                    ClaimUse::RetrospectiveDescriptive,
                )
                .is_err()
            );
        }

        let mut forged = history.clone();
        forged[1].assertion_id = "caller-selected-id".to_string();
        assert!(validate_temporal_assertion_chain(&forged, Some(&evidence)).is_err());

        let same_clock = registered_assertion(
            AssertionState::Corrected,
            Some(original.assertion_id.clone()),
            "2025-01-02T00:00:00Z",
        );
        let non_monotonic = vec![original.clone(), same_clock];
        let evidence = VerifiedTemporalAssertionHistoryEvidence::synthetic(&non_monotonic);
        assert!(validate_temporal_assertion_chain(&non_monotonic, Some(&evidence)).is_err());

        let fork_a = registered_assertion(
            AssertionState::Corrected,
            Some(original.assertion_id.clone()),
            "2025-01-03T00:00:00Z",
        );
        let mut fork_b = registered_assertion(
            AssertionState::Corrected,
            Some(original.assertion_id.clone()),
            "2025-01-04T00:00:00Z",
        );
        fork_b.value = AssertionValue::Text {
            value: "delisted".to_string(),
        };
        fork_b.assertion_id = fork_b.derived_id();
        let forked = vec![original, fork_a, fork_b];
        let forked_evidence = VerifiedTemporalAssertionHistoryEvidence::synthetic(&forked);
        assert!(validate_temporal_assertion_chain(&forked, Some(&forked_evidence)).is_err());

        let active_listed =
            registered_assertion(AssertionState::Active, None, "2025-01-02T00:00:00Z");
        let mut active_delisted = active_listed.clone();
        active_delisted.value = AssertionValue::Text {
            value: "delisted".to_string(),
        };
        active_delisted.retrieval_time = "2025-01-03T00:00:00Z".to_string();
        active_delisted.assertion_id = active_delisted.derived_id();
        let contradictory_roots = vec![active_listed.clone(), active_delisted.clone()];
        let contradictory_evidence =
            VerifiedTemporalAssertionHistoryEvidence::synthetic(&contradictory_roots);
        for assertion in &contradictory_roots {
            assert!(
                validate_temporal_assertion_claim_use(
                    &contradictory_roots,
                    Some(&contradictory_evidence),
                    &assertion.assertion_id,
                    ClaimUse::RetrospectiveDescriptive,
                )
                .is_err()
            );
        }

        let mut mapping_a = active_listed.clone();
        mapping_a.subject_id = "mapping-a".to_string();
        mapping_a.predicate = AssertionPredicate::IdentityMappingEvidence;
        mapping_a.value = AssertionValue::IdentityMappingEvidence {
            mapping_commitment: "commitment-a".to_string(),
            evidence_kind: IdentityEvidenceKind::VenueMetadata,
        };
        mapping_a.assertion_id = mapping_a.derived_id();
        let mut mapping_b = mapping_a.clone();
        mapping_b.value = AssertionValue::IdentityMappingEvidence {
            mapping_commitment: "commitment-b".to_string(),
            evidence_kind: IdentityEvidenceKind::ChainRegistry,
        };
        mapping_b.retrieval_time = "2025-01-03T00:00:00Z".to_string();
        mapping_b.evidence_refs = vec!["archive://conflicting-mapping".to_string()];
        mapping_b.assertion_id = mapping_b.derived_id();
        let conflicting_mappings = vec![mapping_a, mapping_b];
        let conflicting_mapping_evidence =
            VerifiedTemporalAssertionHistoryEvidence::synthetic(&conflicting_mappings);
        assert!(
            validate_temporal_assertion_claim_use(
                &conflicting_mappings,
                Some(&conflicting_mapping_evidence),
                &conflicting_mappings[0].assertion_id,
                ClaimUse::RetrospectiveDescriptive,
            )
            .is_err()
        );

        let mut roster_text = active_listed.clone();
        roster_text.predicate = AssertionPredicate::RosterStatus;
        roster_text.assertion_id = roster_text.derived_id();
        let roster_text_history = vec![roster_text];
        let roster_text_evidence =
            VerifiedTemporalAssertionHistoryEvidence::synthetic(&roster_text_history);
        assert!(
            validate_temporal_assertion_chain(&roster_text_history, Some(&roster_text_evidence))
                .is_err()
        );

        let mut untyped_mapping = active_listed;
        untyped_mapping.predicate = AssertionPredicate::IdentityMappingEvidence;
        untyped_mapping.assertion_id = untyped_mapping.derived_id();
        let untyped_mapping_history = vec![untyped_mapping];
        let untyped_mapping_evidence =
            VerifiedTemporalAssertionHistoryEvidence::synthetic(&untyped_mapping_history);
        assert!(
            validate_temporal_assertion_chain(
                &untyped_mapping_history,
                Some(&untyped_mapping_evidence)
            )
            .is_err()
        );
    }
}
